use std::sync::Mutex;

use hw_monitor::proc::{CoreStat, MemStat};
use hw_monitor::{Acts, CPU, MEMORY, Monitor, cpu_sample, declaration, memory_sample};
use yoke_sdk::base::Error;
use yoke_sdk::plugin::{Activated, Event};

/// Stands in for the Unit: records what was emitted, on which stream.
#[derive(Default)]
struct Recorder(Mutex<Vec<(String, serde_json::Value)>>);

impl Acts for Recorder {
    async fn emit(&self, stream: &str, payload: Vec<u8>) -> Result<(), Error> {
        let v = serde_json::from_slice(&payload).expect("a sample is JSON");
        self.0.lock().unwrap().push((stream.into(), v));
        Ok(())
    }
}

fn core(i: u32, busy: u64, idle: u64) -> CoreStat {
    CoreStat { core_index: i, busy_jiffies: busy, idle_jiffies: idle }
}

fn mem() -> MemStat {
    MemStat { total_bytes: 1000, available_bytes: 250, cached_bytes: 100, swap_total_bytes: 50, swap_free_bytes: 20 }
}

fn activated(stream: &str) -> Event {
    Event::Activated(Activated { stream: stream.into(), transport: "shared".into(), address: String::new() })
}

#[test]
fn the_manifest_in_the_tree_is_the_one_the_declaration_generates() {
    assert_eq!(declaration().manifest(), include_str!("../com.yoke.hw-monitor.yaml"));
}

#[test]
fn usage_is_measured_between_two_readings() {
    assert_eq!(cpu_sample(&[], &[core(0, 10, 10)]), None);
    let s = cpu_sample(&[core(0, 10, 10), core(1, 0, 0)], &[core(0, 40, 80), core(1, 50, 50)]).unwrap();
    assert_eq!(s.cores[0].usage_percent, 30.0);
    assert_eq!(s.cores[1].usage_percent, 50.0);
    assert_eq!(s.aggregate_usage_percent, 40.0);
}

#[test]
fn memory_used_is_total_less_available() {
    let m = memory_sample(&mem());
    assert_eq!((m.used_bytes, m.usage_percent, m.swap_used_bytes), (750, 75.0, 30));
}

#[tokio::test]
async fn only_active_streams_are_emitted_on() {
    let rec = Recorder::default();
    let mut m = Monitor::default();
    m.tick(&rec, Some(vec![core(0, 0, 0)]), Some(mem())).await.unwrap();
    assert!(rec.0.lock().unwrap().is_empty(), "nothing is activated yet");

    m.handle(&activated(MEMORY));
    m.tick(&rec, Some(vec![core(0, 10, 10)]), Some(mem())).await.unwrap();
    m.handle(&activated(CPU));
    m.tick(&rec, Some(vec![core(0, 20, 20)]), Some(mem())).await.unwrap();
    m.handle(&Event::Stopped { stream: MEMORY.into() });
    m.tick(&rec, Some(vec![core(0, 30, 30)]), Some(mem())).await.unwrap();

    let got: Vec<String> = rec.0.lock().unwrap().iter().map(|(s, _)| s.clone()).collect();
    assert_eq!(got, [MEMORY, CPU, MEMORY, CPU]);
    let (_, cpu) = &rec.0.lock().unwrap()[1];
    assert_eq!(cpu["aggregate_usage_percent"], 50.0);
}

#[test]
fn guest_time_is_not_counted_twice() {
    // A core's line: user nice system idle iowait irq softirq steal guest guest_nice. The kernel already
    // counts guest in user and guest_nice in nice.
    let line = "cpu0 100 0 50 800 50 0 0 0 40 0";
    let c = hw_monitor::proc::core_stat(line).expect("a core's line");
    assert_eq!((c.busy_jiffies, c.idle_jiffies), (150, 850));
}

/// Fails on one stream, records the other.
#[derive(Default)]
struct FailsCpu(Mutex<Vec<String>>);

impl Acts for FailsCpu {
    async fn emit(&self, stream: &str, _payload: Vec<u8>) -> Result<(), Error> {
        if stream == CPU {
            return Err(Error::Transport("closed".into()));
        }
        self.0.lock().unwrap().push(stream.into());
        Ok(())
    }
}

#[tokio::test]
async fn a_failed_emit_costs_only_its_own_stream() {
    let mut m = Monitor::default();
    m.handle(&activated(CPU));
    m.handle(&activated(MEMORY));
    let u = FailsCpu::default();
    let _ = m.tick(&u, Some(vec![core(0, 10, 10)]), Some(mem())).await;
    let _ = m.tick(&u, Some(vec![core(0, 20, 20)]), Some(mem())).await;
    assert_eq!(u.0.lock().unwrap().len(), 2, "memory was emitted on both ticks");
}

#[test]
fn a_read_that_fails_is_said_and_one_that_recovers_too() {
    let mut m = Monitor::default();
    assert_eq!(m.health(true, true), None, "nothing to say while the readings are good");
    let (grade, line) = m.health(false, true).expect("a failed read is said");
    assert!(grade >= 50 && line.contains("/proc/stat"), "{grade} {line}");
    assert_eq!(m.health(false, true), None, "said once, not every second");
    assert_eq!(m.health(true, true).map(|h| h.0), Some(0), "a recovery is said");
}

