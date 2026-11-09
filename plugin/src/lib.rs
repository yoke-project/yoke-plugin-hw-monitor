//! The hw-monitor Plugin: CPU and memory of the machine, on two streams.
//!
//! `proc.rs` reads /proc; this file is the Plugin around it: what it declares, the samples it publishes,
//! and what it does with an event of its Session. A stream is emitted only while the Core has it
//! activated, so the unit keeps the set of active streams and the main loop emits on each tick.

use std::collections::HashSet;

use serde::Serialize;
use yoke_sdk::base::Error;
use yoke_sdk::plugin::{Capability, Declaration, Event, Object, Stream};

pub mod proc;

pub const PLUGIN: &str = "com.yoke.hw-monitor";
pub const CPU: &str = "hw.cpu";
pub const MEMORY: &str = "hw.memory";

/// One stream per figure; a sample lost is replaced by the next one, a second later.
pub fn declaration() -> Declaration {
    let stream = |id: &str| Stream {
        id: id.into(),
        tolerates_loss: true,
        tolerates_reorder: false,
    };
    let publish = |id: &str| Capability {
        name: format!("stream.{id}.publish"),
        governs: Object::Stream(id.into()),
    };
    Declaration {
        id: PLUGIN.into(),
        streams: vec![stream(CPU), stream(MEMORY)],
        capabilities: vec![publish(CPU), publish(MEMORY)],
        ..Default::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreUsage {
    pub core_index: u32,
    pub usage_percent: f32,
}

/// The payload of `hw.cpu`, as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuSample {
    pub aggregate_usage_percent: f32,
    pub cores: Vec<CoreUsage>,
}

/// The payload of `hw.memory`, as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemorySample {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub cached_bytes: u64,
    pub usage_percent: f32,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
    pub swap_used_bytes: u64,
}

/// Usage between two readings of /proc/stat; `None` for the first reading, which has nothing to compare.
pub fn cpu_sample(prev: &[proc::CoreStat], cur: &[proc::CoreStat]) -> Option<CpuSample> {
    if prev.is_empty() || prev.len() != cur.len() {
        return None;
    }
    let cores: Vec<CoreUsage> = cur
        .iter()
        .zip(prev)
        .map(|(c, p)| {
            let total = (c.busy_jiffies + c.idle_jiffies).saturating_sub(p.busy_jiffies + p.idle_jiffies);
            let busy = c.busy_jiffies.saturating_sub(p.busy_jiffies);
            let usage = if total == 0 { 0.0 } else { 100.0 * busy as f32 / total as f32 };
            CoreUsage { core_index: c.core_index, usage_percent: usage }
        })
        .collect();
    let aggregate = cores.iter().map(|c| c.usage_percent).sum::<f32>() / cores.len() as f32;
    Some(CpuSample { aggregate_usage_percent: aggregate, cores })
}

pub fn memory_sample(m: &proc::MemStat) -> MemorySample {
    let used = m.total_bytes.saturating_sub(m.available_bytes);
    MemorySample {
        total_bytes: m.total_bytes,
        available_bytes: m.available_bytes,
        used_bytes: used,
        cached_bytes: m.cached_bytes,
        usage_percent: if m.total_bytes == 0 { 0.0 } else { 100.0 * used as f32 / m.total_bytes as f32 },
        swap_total_bytes: m.swap_total_bytes,
        swap_free_bytes: m.swap_free_bytes,
        swap_used_bytes: m.swap_total_bytes.saturating_sub(m.swap_free_bytes),
    }
}

/// What the Plugin does on its Session, so a test can stand in for the Unit.
pub trait Acts {
    fn emit(&self, stream: &str, payload: Vec<u8>) -> impl Future<Output = Result<(), Error>>;
}

impl Acts for yoke_sdk::plugin::Unit {
    async fn emit(&self, stream: &str, payload: Vec<u8>) -> Result<(), Error> {
        yoke_sdk::plugin::Unit::emit(self, stream, payload).await
    }
}

/// The streams the Core has activated, and the last CPU reading the next sample is measured against.
#[derive(Default)]
pub struct Monitor {
    active: HashSet<String>,
    prev_cpu: Vec<proc::CoreStat>,
    /// Whether the last readings could all be made; a change is what the health says.
    failing: bool,
}

impl Monitor {
    /// Follows the Session: an activation adds a stream, a stop removes it. Nothing else is declared.
    pub fn handle(&mut self, event: &Event) {
        match event {
            Event::Activated(a) => {
                self.active.insert(a.stream.clone());
            }
            Event::Stopped { stream } => {
                self.active.remove(stream);
            }
            _ => {}
        }
    }

    pub fn is_active(&self, stream: &str) -> bool {
        self.active.contains(stream)
    }

    /// One tick: a sample on each active stream, from the readings given. The CPU reading is kept even
    /// when nothing is active, so the first sample after an activation is already a measurement.
    pub async fn tick(
        &mut self,
        u: &impl Acts,
        cpu: Option<Vec<proc::CoreStat>>,
        memory: Option<proc::MemStat>,
    ) -> Result<(), Error> {
        // Each stream on its own: one that cannot be emitted on costs the other nothing.
        let mut first = Ok(());
        if let Some(cur) = cpu {
            let sample = cpu_sample(&self.prev_cpu, &cur);
            self.prev_cpu = cur;
            if let (Some(sample), true) = (sample, self.is_active(CPU)) {
                first = first.and(u.emit(CPU, json(&sample)).await);
            }
        }
        if let (Some(m), true) = (memory, self.is_active(MEMORY)) {
            first = first.and(u.emit(MEMORY, json(&memory_sample(&m))).await);
        }
        first
    }

    /// What the health should say after a tick whose readings went as given, where it changes: a reading
    /// that fails is said once, and so is a recovery.
    pub fn health(&mut self, cpu_read: bool, memory_read: bool) -> Option<(u8, String)> {
        let failing = !(cpu_read && memory_read);
        if failing == self.failing {
            return None;
        }
        self.failing = failing;
        if !failing {
            return Some((0, "reading /proc again".into()));
        }
        let what: Vec<&str> = [(cpu_read, "/proc/stat"), (memory_read, "/proc/meminfo")]
            .iter()
            .filter(|(ok, _)| !ok)
            .map(|(_, f)| *f)
            .collect();
        Some((50, format!("{} cannot be read", what.join(" and "))))
    }
}

fn json(v: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}
