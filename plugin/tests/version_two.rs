//! Version 0.2.0: a stable sample format, a snapshot, pressure, and two alarms.

use std::sync::Mutex;

use hw_monitor::proc::{CoreStat, MemStat};
use hw_monitor::{Acts, Config, MEMORY, MEMORY_LOW, CPU_SATURATED, Monitor, PRESSURE, SNAPSHOT, declaration, pressure_of};
use yoke_sdk::base::Error;
use yoke_sdk::plugin::{Activated, Event, Question};

#[derive(Default)]
struct Rec {
    emitted: Mutex<Vec<(String, serde_json::Value)>>,
    answered: Mutex<Vec<serde_json::Value>>,
    reported: Mutex<Vec<(String, u8, String)>>,
}

impl Acts for Rec {
    async fn emit(&self, stream: &str, payload: Vec<u8>) -> Result<(), Error> {
        self.emitted.lock().unwrap().push((stream.into(), serde_json::from_slice(&payload).unwrap()));
        Ok(())
    }
    async fn answer(&self, _q: &Question, payload: Vec<u8>) -> Result<(), Error> {
        self.answered.lock().unwrap().push(serde_json::from_slice(&payload).unwrap());
        Ok(())
    }
    async fn report(&self, occurrence: &str, severity: u8, line: &str) -> Result<(), Error> {
        self.reported.lock().unwrap().push((occurrence.into(), severity, line.into()));
        Ok(())
    }
}

fn mem(available: u64) -> MemStat {
    MemStat { total_bytes: 1000, available_bytes: available, cached_bytes: 0, swap_total_bytes: 0, swap_free_bytes: 0 }
}

fn core(busy: u64, idle: u64) -> Vec<CoreStat> {
    vec![CoreStat { core_index: 0, busy_jiffies: busy, idle_jiffies: idle }]
}

fn activated(stream: &str) -> Event {
    Event::Activated(Activated { stream: stream.into(), transport: "shared".into(), address: String::new() })
}

#[test]
fn what_is_declared_is_each_granted_by_a_capability_of_its_own() {
    let d = declaration();
    assert!(d.queries.contains(&SNAPSHOT.to_string()));
    assert!(d.streams.iter().any(|s| s.id == PRESSURE));
    assert!(d.occurrences.contains(&MEMORY_LOW.to_string()) && d.occurrences.contains(&CPU_SATURATED.to_string()));
    assert_eq!(d.capabilities.len(), d.streams.len() + d.queries.len() + d.occurrences.len());
}

#[tokio::test]
async fn every_sample_states_its_schema_and_its_instant() {
    let rec = Rec::default();
    let mut m = Monitor::new(Config::default());
    m.handle(&activated(MEMORY));
    m.tick(&rec, Some(core(0, 0)), Some(mem(500))).await.unwrap();
    let (_, v) = &rec.emitted.lock().unwrap()[0];
    assert_eq!(v["schema"], 1);
    assert!(v["at_ms"].as_u64().unwrap() > 1_700_000_000_000);
    assert_eq!(v["usage_percent"], 50.0);
}

#[tokio::test]
async fn the_snapshot_answers_the_latest_readings_without_a_stream() {
    let rec = Rec::default();
    let mut m = Monitor::new(Config::default());
    m.tick(&rec, Some(core(0, 0)), Some(mem(250))).await.unwrap();
    m.tick(&rec, Some(core(10, 10)), Some(mem(250))).await.unwrap();
    let q = Question { id: "q".into(), r#type: SNAPSHOT.into(), payload: vec![] };
    m.ask(&rec, &q).await.unwrap();
    let a = &rec.answered.lock().unwrap()[0];
    assert_eq!(a["schema"], 1);
    assert_eq!(a["memory"]["usage_percent"], 75.0);
    assert_eq!(a["cpu"]["aggregate_usage_percent"], 50.0);
    assert!(rec.emitted.lock().unwrap().is_empty(), "nothing is emitted on a stream nobody started");
}

#[test]
fn pressure_reads_the_load_and_says_where_the_kernel_has_none() {
    let p = pressure_of(
        Some("0.90 1.59 1.60 3/2264 1356495"),
        Some("some avg10=1.50 avg60=0.70 avg300=0.04 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0"),
        None,
        None,
    );
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["load"], serde_json::json!([0.9, 1.59, 1.6]));
    assert_eq!(v["cpu"]["some_avg10"], 1.5);
    assert_eq!(v["memory"]["available"], false);
}

#[tokio::test]
async fn memory_low_is_reported_once_it_has_lasted_and_again_only_after_it_cleared() {
    let rec = Rec::default();
    let cfg = Config::from_args(["--memory-low-percent", "10", "--for-seconds", "3"].map(String::from));
    let mut m = Monitor::new(cfg);
    for available in [50, 50, 50, 50, 120, 50, 50, 140, 50, 50, 50] {
        m.tick(&rec, None, Some(mem(available))).await.unwrap();
    }
    let got: Vec<String> = rec.reported.lock().unwrap().iter().map(|r| r.0.clone()).collect();
    // Three seconds below 10 % fires once; 12 % is within the hysteresis and clears nothing; 14 % clears.
    assert_eq!(got, [MEMORY_LOW, MEMORY_LOW]);
    assert!(rec.reported.lock().unwrap()[0].1 >= 30);
}

#[tokio::test]
async fn a_saturated_cpu_is_reported_once_it_has_lasted() {
    let rec = Rec::default();
    let cfg = Config::from_args(["--cpu-saturated-percent", "90", "--for-seconds", "2"].map(String::from));
    let mut m = Monitor::new(cfg);
    let mut busy = 0;
    for _ in 0..4 {
        busy += 100;
        m.tick(&rec, Some(core(busy, 0)), None).await.unwrap();
    }
    assert_eq!(rec.reported.lock().unwrap().iter().filter(|r| r.0 == CPU_SATURATED).count(), 1);
}

#[test]
fn the_arguments_set_the_rate_and_the_thresholds() {
    let c = Config::from_args(["--interval-ms", "500", "--memory-low-percent", "5", "--cpu-saturated-percent", "80", "--for-seconds", "10"].map(String::from));
    assert_eq!((c.interval_ms, c.memory_low_percent, c.cpu_saturated_percent, c.for_seconds), (500, 5.0, 80.0, 10));
    let d = Config::default();
    assert_eq!((d.interval_ms, d.memory_low_percent, d.cpu_saturated_percent, d.for_seconds), (1000, 10.0, 95.0, 30));
}
