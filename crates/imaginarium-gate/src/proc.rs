// SPDX-License-Identifier: AGPL-3.0-or-later

//! Process and machine facts from `/proc` (Linux). Elsewhere every query
//! answers `None`, and the report says "unknown".

use std::time::Duration;

/// Linux reports process times in clock ticks. `_SC_CLK_TCK` is 100 on
/// every architecture Linux supports today; reading it would need libc.
const TICKS_PER_SECOND: f64 = 100.0;

/// The fields of `/proc/self/stat` after the command name, which may itself
/// contain spaces and parentheses.
fn stat_fields() -> Option<Vec<String>> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_owned).collect())
}

fn ticks(field: &str) -> Option<Duration> {
    let ticks: f64 = field.parse().ok()?;
    Some(Duration::from_secs_f64(ticks / TICKS_PER_SECOND))
}

/// CPU time this process used so far, user plus system.
pub(crate) fn cpu_time() -> Option<Duration> {
    let fields = stat_fields()?;
    // Fields 14 and 15 of stat(5); the list starts at field 3.
    Some(ticks(fields.get(11)?)? + ticks(fields.get(12)?)?)
}

/// How long ago the kernel started this process (10 ms resolution), so the
/// cold start includes loading the binary and its libraries.
pub(crate) fn since_exec() -> Option<Duration> {
    let fields = stat_fields()?;
    let started = ticks(fields.get(19)?)?; // field 22: start time after boot
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let uptime: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    Duration::from_secs_f64(uptime).checked_sub(started)
}

/// The peak resident set size of this process.
pub(crate) fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// The CPU model, logical core count, RAM and kernel.
pub(crate) fn machine() -> serde_json::Value {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find(|line| line.starts_with("model name"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, name)| name.trim().to_owned());
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let ram = meminfo
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib * 1024);
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|release| release.trim().to_owned());
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "kernel": kernel,
        "cpu": cpu,
        "logical_cores": std::thread::available_parallelism().map(usize::from).ok(),
        "ram_bytes": ram,
        "session": std::env::var("XDG_SESSION_TYPE").ok(),
        "desktop": std::env::var("XDG_CURRENT_DESKTOP").ok(),
        "wayland": std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()),
    })
}
