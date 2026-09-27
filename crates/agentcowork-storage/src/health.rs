//! D12 — storage health: drive-threshold monitoring + cleanup-plan inputs
//! (doc 52 §Gap-2 D12). Free-space detection is delegated to `sysinfo`
//! (cross-platform `Disks`); the threshold check itself is a pure function so
//! the exit-criterion test needs no real disk.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::StorageError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriveStats {
    pub mount: String,
    pub total: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthStatus {
    pub drive: DriveStats,
    pub used_bytes: u64,
    pub used_pct: f64,
    pub threshold_pct: f64,
    pub over_threshold: bool,
}

/// Pure threshold test (exit-criterion target: flag when a drive is ≥90% full).
pub fn over_threshold(used: u64, total: u64, threshold_pct: f64) -> bool {
    if total == 0 {
        return false;
    }
    (used as f64 / total as f64) * 100.0 >= threshold_pct
}

/// Build a `HealthStatus` from already-known stats (pure).
pub fn health_from_stats(stats: DriveStats, threshold_pct: f64) -> HealthStatus {
    let used = stats.total.saturating_sub(stats.available);
    HealthStatus {
        used_pct: if stats.total == 0 {
            0.0
        } else {
            (used as f64 / stats.total as f64) * 100.0
        },
        used_bytes: used,
        over_threshold: over_threshold(used, stats.total, threshold_pct),
        threshold_pct,
        drive: stats,
    }
}

/// Resolve the disk (longest mount-point prefix) containing `path`.
pub fn drive_stats(path: &Path) -> Result<DriveStats, StorageError> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let target = path.to_string_lossy();

    let mut best: Option<&sysinfo::Disk> = None;
    let mut best_len = 0usize;
    for disk in disks.list() {
        let mp = disk.mount_point().to_string_lossy();
        if target.starts_with(mp.as_ref()) && mp.len() > best_len {
            best = Some(disk);
            best_len = mp.len();
        }
    }

    let disk = best.ok_or_else(|| StorageError::Other("no disk found for path".into()))?;
    Ok(DriveStats {
        mount: disk.mount_point().to_string_lossy().into_owned(),
        total: disk.total_space(),
        available: disk.available_space(),
    })
}

pub fn check_health(path: &Path, threshold_pct: f64) -> Result<HealthStatus, StorageError> {
    Ok(health_from_stats(drive_stats(path)?, threshold_pct))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_and_health() {
        assert!(over_threshold(95, 100, 90.0));
        assert!(!over_threshold(85, 100, 90.0));
        assert!(!over_threshold(0, 0, 90.0));

        let h = health_from_stats(
            DriveStats {
                mount: "/".into(),
                total: 1000,
                available: 50,
            },
            90.0,
        );
        assert!(h.over_threshold);
        assert_eq!(h.used_bytes, 950);
        assert!((h.used_pct - 95.0).abs() < 1e-6);
    }
}
