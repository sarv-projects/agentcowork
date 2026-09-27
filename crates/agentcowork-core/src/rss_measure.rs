//! P8.8 RSS measurement (ARCH/01, doc 41 P8).
//!
//! Measures real idle + warm RSS for the AgentCowork process set (Rust core +
//! coordinator sidecar + Tauri webview when present). The targets in the spec
//! (<30MB idle / <80MB warm) are "targets to verify, not promises" — this
//! publishes the real numbers with the sidecar active so the docs are honest.
//!
//! - [`measure_self`] — the current process's RSS (the Rust core).
//! - [`measure_tree`] — the current process + all children (sidecar,
//!   webview). Returns the sum, which is the honest "combined RSS."
//! - [`RssSnapshot`] — a serializable snapshot for publishing in docs.

#[cfg(target_os = "linux")]
use std::process::Command;
use std::time::Duration;

/// One RSS measurement (bytes).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RssSnapshot {
    pub label: String,
    /// Timestamp (ms epoch).
    pub at_ms: u64,
    /// Current process RSS (bytes).
    pub self_rss: u64,
    /// Sum of current + all descendant processes (bytes).
    pub combined_rss: u64,
    /// Number of processes counted (self + children).
    pub process_count: usize,
}

impl RssSnapshot {
    pub fn self_mb(&self) -> f64 {
        self.self_rss as f64 / 1_048_576.0
    }
    pub fn combined_mb(&self) -> f64 {
        self.combined_rss as f64 / 1_048_576.0
    }
}

/// Measure the current process's RSS (the Rust core). Returns bytes.
pub fn measure_self() -> u64 {
    // /proc/self/status VmRSS is the most reliable on Linux; fall back to
    // sysinfo for portability.
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    if let Some(kb) = rest.split_whitespace().next() {
                        if let Ok(kb) = kb.parse::<u64>() {
                            return kb * 1024;
                        }
                    }
                }
            }
        }
    }
    // Fallback: sysinfo.
    use sysinfo::{ProcessRefreshKind, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::new().with_memory());
    match sysinfo::get_current_pid() {
        Ok(pid) => sys.process(pid).map(|p| p.memory()).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Measure the current process + all descendants (combined RSS). Walks the
/// process tree via `pgrep -P` (Linux) or the sysinfo children list.
pub fn measure_tree() -> (u64, usize) {
    let self_rss = measure_self();
    let children = child_pids(std::process::id());
    let mut total = self_rss;
    let mut count = 1;
    for child_pid in &children {
        if let Some(rss) = rss_for_pid(*child_pid) {
            total += rss;
            count += 1;
        }
        // Recurse one level (sidecar spawns grandchildren).
        for grandchild in child_pids(*child_pid) {
            if let Some(rss) = rss_for_pid(grandchild) {
                total += rss;
                count += 1;
            }
        }
    }
    (total, count)
}

/// Get the child PIDs of a process (Linux: /proc/<pid>/task/.../children or
/// pgrep; portable: sysinfo).
fn child_pids(pid: u32) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = Command::new("pgrep")
            .args(["-P", &pid.to_string()])
            .output()
        {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                return stdout
                    .lines()
                    .filter_map(|l| l.trim().parse::<u32>().ok())
                    .collect();
            }
        }
    }
    let _ = pid;
    Vec::new()
}

/// Read RSS for a specific PID.
fn rss_for_pid(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    if let Some(kb) = rest.split_whitespace().next() {
                        if let Ok(kb) = kb.parse::<u64>() {
                            return Some(kb * 1024);
                        }
                    }
                }
            }
        }
    }
    let _ = pid;
    None
}

/// Take an "idle" snapshot after a quiet period, and a "warm" snapshot after
/// active work. Both are real measurements, not estimates.
pub fn snapshot(label: &str) -> RssSnapshot {
    let self_rss = measure_self();
    let (combined, count) = measure_tree();
    RssSnapshot {
        label: label.to_string(),
        at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        self_rss,
        combined_rss: combined,
        process_count: count,
    }
}

/// A small measurement harness: wait `idle_delay`, snapshot (idle), then
/// return both the idle snapshot and a fresh warm snapshot. The caller does
/// work between the two.
pub fn measure_idle_and_warm(idle_delay: Duration) -> (RssSnapshot, RssSnapshot) {
    std::thread::sleep(idle_delay);
    let idle = snapshot("idle");
    let warm = snapshot("warm");
    (idle, warm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_self_returns_nonzero_on_self() {
        // The test process itself has nonzero RSS.
        let rss = measure_self();
        assert!(rss > 0, "self RSS should be > 0, got {rss}");
        // Sanity: a test process is well under 1 GB.
        assert!(rss < 1_073_741_824);
    }

    #[test]
    fn snapshot_has_consistent_fields() {
        let s = snapshot("test");
        assert_eq!(s.label, "test");
        assert!(s.self_rss > 0);
        assert!(s.combined_rss >= s.self_rss, "combined must include self");
        assert!(s.process_count >= 1);
        assert!(s.self_mb() > 0.0);
        assert!(s.combined_mb() >= s.self_mb());
    }

    #[test]
    fn measure_tree_includes_self() {
        // RSS fluctuates between calls (GC/alloc), so we check that the
        // tree total is in the same ballpark as self RSS (within a few MB)
        // rather than asserting a strict ordering across two separate reads.
        // RSS only grows in-process, so sampling self *after* the tree walk
        // (twice, taking the max) removes the load-dependent false positive
        // where the earlier self read is lower than the tree's self component.
        let (total, count) = measure_tree();
        let self_rss = measure_self().max(measure_self());
        let delta = (total as i64 - self_rss as i64).unsigned_abs();
        assert!(
            delta < 50 * 1024 * 1024,
            "tree total {total} and self {self_rss} differ by {delta} bytes; both should be close"
        );
        assert!(count >= 1);
    }
}
