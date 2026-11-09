//! The hw-monitor Plugin: the load on the machine — CPU, memory and pressure — on three streams, as a
//! snapshot when asked, and as two alarms when a threshold has held long enough.
//!
//! `proc.rs` reads /proc; this file is the Plugin around it: what it declares, the samples it publishes,
//! and what it does with an event of its Session. A stream is emitted only while the Core has it
//! activated, so the unit keeps the set of active streams and the main loop emits on each tick.

use std::collections::HashSet;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};
use yoke_sdk::base::Error;
use yoke_sdk::plugin::{Capability, Declaration, Event, Object, Question, Stream};

pub mod proc;

pub const PLUGIN: &str = "com.yoke.hw-monitor";
pub const CPU: &str = "hw.cpu";
pub const MEMORY: &str = "hw.memory";
pub const PRESSURE: &str = "hw.pressure";
/// The latest readings, as one answer.
pub const SNAPSHOT: &str = "hw.snapshot";
/// Available memory below its threshold for long enough.
pub const MEMORY_LOW: &str = "hw.memory.low";
/// The CPU above its threshold for long enough.
pub const CPU_SATURATED: &str = "hw.cpu.saturated";
/// The format every sample and answer states as `schema`.
pub const SCHEMA: u32 = 1;
/// How far a figure must come back past its threshold before an alarm is armed again, in points.
pub const HYSTERESIS: f32 = 3.0;

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
    let answer = |q: &str| Capability { name: format!("query.{q}.answer"), governs: Object::Query(q.into()) };
    let report = |o: &str| Capability { name: format!("event.{o}.report"), governs: Object::Occurrence(o.into()) };
    Declaration {
        id: PLUGIN.into(),
        streams: vec![stream(CPU), stream(MEMORY), stream(PRESSURE)],
        queries: vec![SNAPSHOT.into()],
        occurrences: vec![MEMORY_LOW.into(), CPU_SATURATED.into()],
        capabilities: vec![
            publish(CPU),
            publish(MEMORY),
            publish(PRESSURE),
            answer(SNAPSHOT),
            report(MEMORY_LOW),
            report(CPU_SATURATED),
        ],
        ..Default::default()
    }
}

/// What the application sets among the unit's arguments.
#[derive(Debug, Clone)]
pub struct Config {
    /// How often the machine is read.
    pub interval_ms: u64,
    /// `hw.memory.low` when available memory is below this share of the total.
    pub memory_low_percent: f32,
    /// `hw.cpu.saturated` when the aggregate usage is at or above this.
    pub cpu_saturated_percent: f32,
    /// How long a threshold must hold before its alarm is reported.
    pub for_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { interval_ms: 1000, memory_low_percent: 10.0, cpu_saturated_percent: 95.0, for_seconds: 30 }
    }
}

impl Config {
    /// `--interval-ms N`, `--memory-low-percent P`, `--cpu-saturated-percent P`, `--for-seconds N`; anything
    /// else, or a value that does not read, leaves the default.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut c = Config::default();
        let mut args = args.into_iter();
        while let Some(a) = args.next() {
            let value = args.next();
            let value = value.as_deref();
            match a.as_str() {
                "--interval-ms" => c.interval_ms = value.and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(c.interval_ms),
                "--memory-low-percent" => c.memory_low_percent = value.and_then(|v| v.parse().ok()).unwrap_or(c.memory_low_percent),
                "--cpu-saturated-percent" => c.cpu_saturated_percent = value.and_then(|v| v.parse().ok()).unwrap_or(c.cpu_saturated_percent),
                "--for-seconds" => c.for_seconds = value.and_then(|v| v.parse().ok()).unwrap_or(c.for_seconds),
                _ => {}
            }
        }
        c
    }

    /// How many readings in a row make the duration.
    fn readings(&self) -> u32 {
        (self.for_seconds * 1000).div_ceil(self.interval_ms).max(1) as u32
    }
}

/// One kind of the kernel's pressure (PSI), or that the kernel has none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Psi {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub some_avg10: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub some_avg60: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_avg10: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_avg60: Option<f64>,
}

/// The payload of `hw.pressure`, as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PressureSample {
    /// The load average over 1, 5 and 15 minutes, or nothing where it cannot be read.
    pub load: Option<[f64; 3]>,
    pub cpu: Psi,
    pub memory: Psi,
    pub io: Psi,
}

/// A pressure sample from the text of /proc/loadavg and of /proc/pressure/{cpu,memory,io}, each where read.
pub fn pressure_of(load: Option<&str>, cpu: Option<&str>, memory: Option<&str>, io: Option<&str>) -> PressureSample {
    let load = load.and_then(|l| {
        let v: Vec<f64> = l.split_whitespace().take(3).filter_map(|f| f.parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    });
    let psi = |text: Option<&str>| {
        let Some(text) = text else {
            return Psi { available: false, some_avg10: None, some_avg60: None, full_avg10: None, full_avg60: None };
        };
        let field = |kind: &str, key: &str| {
            text.lines()
                .find(|l| l.starts_with(kind))
                .and_then(|l| l.split_whitespace().find_map(|kv| kv.strip_prefix(key).and_then(|v| v.parse().ok())))
        };
        Psi {
            available: true,
            some_avg10: field("some", "avg10="),
            some_avg60: field("some", "avg60="),
            full_avg10: field("full", "avg10="),
            full_avg60: field("full", "avg60="),
        }
    };
    PressureSample { load, cpu: psi(cpu), memory: psi(memory), io: psi(io) }
}

/// An alarm: reported once its condition has held for the readings it needs, and armed again only once the
/// figure has come back past its threshold by the hysteresis.
#[derive(Debug, Default)]
struct Alarm {
    held: u32,
    reported: bool,
}

impl Alarm {
    /// One reading: whether the condition holds, and whether it has cleared. True when it is to be reported.
    fn step(&mut self, holds: bool, cleared: bool, needed: u32) -> bool {
        if cleared {
            *self = Alarm::default();
            return false;
        }
        if !holds {
            self.held = 0;
            return false;
        }
        self.held += 1;
        if self.held >= needed && !self.reported {
            self.reported = true;
            return true;
        }
        false
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A sample with the format and the instant every payload states.
fn stamped(sample: &impl Serialize, at_ms: u64) -> Value {
    let mut v = serde_json::to_value(sample).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert("schema".into(), json!(SCHEMA));
        m.insert("at_ms".into(), json!(at_ms));
    }
    v
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
    fn answer(&self, _q: &Question, _payload: Vec<u8>) -> impl Future<Output = Result<(), Error>> {
        async { Ok(()) }
    }
    fn report(&self, _occurrence: &str, _severity: u8, _line: &str) -> impl Future<Output = Result<(), Error>> {
        async { Ok(()) }
    }
}

impl Acts for yoke_sdk::plugin::Unit {
    async fn emit(&self, stream: &str, payload: Vec<u8>) -> Result<(), Error> {
        yoke_sdk::plugin::Unit::emit(self, stream, payload).await
    }
    async fn answer(&self, q: &Question, payload: Vec<u8>) -> Result<(), Error> {
        yoke_sdk::plugin::Unit::answer(self, q, payload).await
    }
    async fn report(&self, occurrence: &str, severity: u8, line: &str) -> Result<(), Error> {
        let severity = Some(yoke_sdk::plugin::Severity::of(severity));
        yoke_sdk::plugin::Unit::report(self, occurrence, severity, line, Vec::new()).await
    }
}

/// The streams the Core has activated, and the last CPU reading the next sample is measured against.
#[derive(Default)]
pub struct Monitor {
    config: Config,
    active: HashSet<String>,
    prev_cpu: Vec<proc::CoreStat>,
    /// Whether the last readings could all be made; a change is what the health says.
    failing: bool,
    /// The latest of each, stamped, for the snapshot.
    last_cpu: Option<Value>,
    last_memory: Option<Value>,
    last_pressure: Option<Value>,
    memory_low: Alarm,
    cpu_saturated: Alarm,
}

impl Monitor {
    pub fn new(config: Config) -> Self {
        Monitor { config, ..Default::default() }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Answers `hw.snapshot` with the latest readings; nothing else is declared.
    pub async fn ask(&self, u: &impl Acts, q: &Question) -> Result<(), Error> {
        if q.r#type != SNAPSHOT {
            return Ok(());
        }
        let answer = json!({
            "schema": SCHEMA,
            "at_ms": now_ms(),
            "cpu": self.last_cpu,
            "memory": self.last_memory,
            "pressure": self.last_pressure,
        });
        u.answer(q, answer.to_string().into_bytes()).await
    }

    /// One pressure reading: kept for the snapshot, and emitted where its stream flows.
    pub async fn tick_pressure(&mut self, u: &impl Acts, sample: PressureSample) -> Result<(), Error> {
        let v = stamped(&sample, now_ms());
        self.last_pressure = Some(v.clone());
        if self.is_active(PRESSURE) {
            return u.emit(PRESSURE, v.to_string().into_bytes()).await;
        }
        Ok(())
    }

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
        let at = now_ms();
        let needed = self.config.readings();
        let mut first = Ok(());
        if let Some(cur) = cpu {
            let sample = cpu_sample(&self.prev_cpu, &cur);
            self.prev_cpu = cur;
            if let Some(sample) = sample {
                let v = stamped(&sample, at);
                self.last_cpu = Some(v.clone());
                let limit = self.config.cpu_saturated_percent;
                let usage = sample.aggregate_usage_percent;
                if self.cpu_saturated.step(usage >= limit, usage < limit - HYSTERESIS, needed) {
                    let line = format!("the CPU at {usage:.0} % for {} s", self.config.for_seconds);
                    first = first.and(u.report(CPU_SATURATED, 30, &line).await);
                }
                if self.is_active(CPU) {
                    first = first.and(u.emit(CPU, v.to_string().into_bytes()).await);
                }
            }
        }
        if let Some(m) = memory {
            let sample = memory_sample(&m);
            let v = stamped(&sample, at);
            self.last_memory = Some(v.clone());
            let available = if m.total_bytes == 0 { 100.0 } else { 100.0 * m.available_bytes as f32 / m.total_bytes as f32 };
            let limit = self.config.memory_low_percent;
            if self.memory_low.step(available < limit, available >= limit + HYSTERESIS, needed) {
                let line = format!("{available:.0} % of memory available for {} s", self.config.for_seconds);
                first = first.and(u.report(MEMORY_LOW, 50, &line).await);
            }
            if self.is_active(MEMORY) {
                first = first.and(u.emit(MEMORY, v.to_string().into_bytes()).await);
            }
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


