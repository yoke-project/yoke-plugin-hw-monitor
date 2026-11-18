//! Version 0.3.0: every reading carries its stream's sequence, so an interface that subscribes late joins
//! the snapshot and the stream without a gap.

use std::sync::Mutex;

use hw_monitor::proc::{CoreStat, MemStat};
use hw_monitor::{Acts, Config, MEMORY, Monitor, SNAPSHOT};
use yoke_sdk::base::Error;
use yoke_sdk::plugin::{Activated, Event, Question};

#[derive(Default)]
struct Rec {
    emitted: Mutex<Vec<(String, serde_json::Value)>>,
    answered: Mutex<Vec<serde_json::Value>>,
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
}

fn mem(available: u64) -> MemStat {
    MemStat { total_bytes: 1000, available_bytes: available, free_bytes: 0, buffers_bytes: 0, cached_bytes: 0, swap_total_bytes: 0, swap_free_bytes: 0 }
}

fn core(busy: u64) -> Vec<CoreStat> {
    vec![CoreStat { core_index: 0, busy_jiffies: busy, idle_jiffies: busy }]
}

#[tokio::test]
async fn a_late_interface_joins_the_snapshot_and_the_stream_without_a_gap() {
    let rec = Rec::default();
    let mut m = Monitor::new(Config::default());
    // Readings before anyone watches still count.
    for i in 0..3 {
        m.tick(&rec, Some(core(i * 10)), Some(mem(500 - i))).await.unwrap();
    }
    // The interface subscribes and starts the stream; two readings flow before its question is answered.
    m.handle(&Event::Activated(Activated { stream: MEMORY.into(), transport: "shared".into(), address: String::new() }));
    m.tick(&rec, Some(core(30)), Some(mem(497))).await.unwrap();
    m.tick(&rec, Some(core(40)), Some(mem(496))).await.unwrap();
    let q = Question { id: "q".into(), r#type: SNAPSHOT.into(), payload: vec![] };
    m.ask(&rec, &q).await.unwrap();
    m.tick(&rec, Some(core(50)), Some(mem(495))).await.unwrap();
    m.tick(&rec, Some(core(60)), Some(mem(494))).await.unwrap();

    let answer = rec.answered.lock().unwrap()[0].clone();
    let at = answer["memory"]["sequence"].as_u64().expect("the snapshot's memory reading carries its sequence");
    assert_eq!(at, 5, "the snapshot reflects the fifth reading of memory");
    assert_eq!(answer["cpu"]["sequence"], 4, "and the fourth of the CPU, the first having nothing to compare");

    // What the interface holds: the frames it received, kept only past the snapshot's sequence.
    let frames: Vec<u64> = rec.emitted.lock().unwrap().iter()
        .filter(|(s, _)| s == MEMORY)
        .map(|(_, v)| v["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(frames, [4, 5, 6, 7], "every frame carries its sequence, counted from the unit's first reading");
    let kept: Vec<u64> = frames.into_iter().filter(|&n| n > at).collect();
    let mut seen = vec![at];
    seen.extend(kept);
    assert!(seen.windows(2).all(|w| w[1] == w[0] + 1), "the snapshot and what follows it leave a gap: {seen:?}");
}
