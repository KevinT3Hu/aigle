//! Linux compute resource discovery. No desktop services.
use crate::budget::GIB;

/// Physical RAM from `/proc/meminfo`; 16 GiB (the minimum spec) if unavailable.
pub fn physical_ram() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| {
                    l.split_whitespace()
                        .nth(1)
                        .and_then(|kb| kb.parse::<u64>().ok())
                        .map(|kb| kb * 1024)
                })
        })
        .unwrap_or(16 * GIB)
}

pub(crate) fn cgroup_limit() -> Option<u64> {
    ["/sys/fs/cgroup/memory.high", "/sys/fs/cgroup/memory.max"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .min()
}

pub(crate) fn memory_pressure() -> Option<String> {
    std::fs::read_to_string("/proc/pressure/memory").ok()
}
