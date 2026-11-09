use std::time::Duration;

use hw_monitor::{Monitor, declaration, proc};
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
    let mut monitor = Monitor::default();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("hw-monitor listens for SIGTERM");
    loop {
        tokio::select! {
            event = unit.next() => match event {
                None | Some(Event::Ended(_)) => return,
                Some(event) => monitor.handle(&event),
            },
            _ = ticker.tick() => {
                let (cpu, memory) = (proc::read_cpu_stats().ok(), proc::read_memory_stats().ok());
                if let Err(err) = monitor.tick(&unit, cpu, memory).await {
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
