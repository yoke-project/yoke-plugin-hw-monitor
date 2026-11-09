//! Reads CPU and RAM data from Linux /proc pseudo-filesystem.

use std::fs;
use std::io::{self, BufRead};

/// Reads logical CPU core count from /proc/stat at startup.
/// The plugin must NOT hardcode a core count.
pub fn read_cpu_core_count() -> io::Result<usize> {
    let file = fs::File::open("/proc/stat")?;
    let reader = io::BufReader::new(file);
    let count = reader
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| {
            let rest = l.strip_prefix("cpu");
            matches!(rest, Some(r) if r.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count();
    Ok(count)
}

/// Reads every core's line of /proc/stat.
pub fn read_cpu_stats() -> io::Result<Vec<CoreStat>> {
    let content = fs::read_to_string("/proc/stat")?;
    Ok(content.lines().filter_map(core_stat).collect())
}

/// One core's line of /proc/stat — `cpuN user nice system idle iowait irq softirq steal guest guest_nice` —
/// or nothing for any other line. Guest time is already in user and nice, so it is not counted again.
pub fn core_stat(line: &str) -> Option<CoreStat> {
    let rest = line.strip_prefix("cpu")?;
    if !rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let mut parts = rest.split_whitespace();
    let core_index: u32 = parts.next()?.parse().ok()?;
    let fields: Vec<u64> = parts.map(|s| s.parse::<u64>().unwrap_or(0)).collect();
    let field = |i: usize| fields.get(i).copied().unwrap_or(0);
    let idle_jiffies = field(3) + field(4);
    let total: u64 = (0..8).map(field).sum();
    Some(CoreStat { core_index, busy_jiffies: total.saturating_sub(idle_jiffies), idle_jiffies })
}

/// Reads RAM and swap from /proc/meminfo.
pub fn read_memory_stats() -> io::Result<MemStat> {
    let content = fs::read_to_string("/proc/meminfo")?;
    let mut total_bytes = 0u64;
    let mut available_bytes = 0u64;
    let mut cached_bytes = 0u64;
    let mut swap_total_bytes = 0u64;
    let mut swap_free_bytes = 0u64;

    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let val: u64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        // Values in /proc/meminfo are in kB
        match key {
            "MemTotal:"     => total_bytes     = val * 1024,
            "MemAvailable:" => available_bytes  = val * 1024,
            "Cached:"       => cached_bytes     = val * 1024,
            "SwapTotal:"    => swap_total_bytes = val * 1024,
            "SwapFree:"     => swap_free_bytes  = val * 1024,
            _ => {}
        }
    }
    Ok(MemStat { total_bytes, available_bytes, cached_bytes, swap_total_bytes, swap_free_bytes })
}

pub struct CoreStat {
    pub core_index: u32,
    /// CPU time spent in non-idle states since epoch, in jiffies.
    pub busy_jiffies: u64,
    /// CPU idle time since epoch, in jiffies.
    pub idle_jiffies: u64,
}

pub struct MemStat {
    pub total_bytes:      u64,
    pub available_bytes:  u64,
    /// Page cache (from "Cached:" in /proc/meminfo).
    pub cached_bytes:     u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes:  u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_core_count_parses_proc_stat() {
        let count = read_cpu_core_count().expect("/proc/stat must be readable on Linux");
        assert!(count >= 1, "expected at least one CPU core, got {}", count);
    }

    #[test]
    fn test_read_cpu_stats_returns_one_entry_per_core() {
        let core_count = read_cpu_core_count().unwrap();
        let stats = read_cpu_stats().unwrap();
        assert_eq!(stats.len(), core_count);
        for s in &stats {
            assert!(s.busy_jiffies + s.idle_jiffies > 0);
        }
    }

    #[test]
    fn test_read_memory_stats_returns_nonzero_total() {
        let mem = read_memory_stats().unwrap();
        assert!(mem.total_bytes > 0, "MemTotal must be > 0");
        assert!(mem.available_bytes <= mem.total_bytes);
        assert!(mem.cached_bytes <= mem.total_bytes, "cached must not exceed total");
    }
}
