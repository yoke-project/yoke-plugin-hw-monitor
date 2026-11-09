use std::time::Duration;

use hw_monitor::{Config, Monitor, declaration, pressure_of, proc};
use yoke_sdk::plugin::{Event, start};

#[tokio::main]
async fn main() {
    // `hw-monitor --manifest` prints the Manifest the declaration generates, which the bundle carries.
    if std::env::args().nth(1).as_deref() == Some("--manifest") {
        print!("{}", declaration().manifest());
        return;
    }
    let unit = match start(&declaration()).await {
        Ok(unit) => unit,
        Err(err) => {
            eprintln!("hw-monitor: {err}");
            std::process::exit(1);
        }
    };
    let cores = proc::read_cpu_core_count().unwrap_or(0);
    if let Err(err) = unit.health(0, &format!("{cores} logical core(s)")).await {
        eprintln!("hw-monitor: {err}");
    }
    // The rate and the thresholds are the application's, among the unit's arguments.
    let mut monitor = Monitor::new(Config::from_args(std::env::args().skip(1)));
    let mut ticker = tokio::time::interval(Duration::from_millis(monitor.config().interval_ms));
    // After a suspension the missed seconds are skipped, not caught up in a burst.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("hw-monitor listens for SIGTERM");
    loop {
        tokio::select! {
            event = unit.next() => match event {
                None | Some(Event::Ended(_)) => return,
                Some(Event::Question(q)) => {
                    if let Err(err) = monitor.ask(&unit, &q).await {
                        eprintln!("hw-monitor: {err}");
                    }
                }
                Some(event) => monitor.handle(&event),
            },
            _ = ticker.tick() => {
                let (cpu, memory) = (proc::read_cpu_stats().ok(), proc::read_memory_stats().ok());
                if let Some((grade, line)) = monitor.health(cpu.is_some(), memory.is_some()) {
                    if let Err(err) = unit.health(grade, &line).await {
                        eprintln!("hw-monitor: {err}");
                    }
                }
                if let Err(err) = monitor.tick(&unit, cpu, memory).await {
                    eprintln!("hw-monitor: {err}");
                }
                let read = |p: &str| std::fs::read_to_string(p).ok();
                let pressure = pressure_of(
                    read("/proc/loadavg").as_deref(),
                    read("/proc/pressure/cpu").as_deref(),
                    read("/proc/pressure/memory").as_deref(),
                    read("/proc/pressure/io").as_deref(),
                );
                if let Err(err) = monitor.tick_pressure(&unit, pressure).await {
                    eprintln!("hw-monitor: {err}");
                }
            }
            _ = term.recv() => {
                // Asked to stop: close the Session in order, and leave once it has ended.
                let _ = unit.close().await;
                while let Some(event) = unit.next().await {
                    if matches!(event, Event::Ended(_)) {
                        break;
                    }
                }
                return;
            }
        }
    }
}
