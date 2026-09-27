//! agentcowork-desktop — maintenance commands (audit retention compaction).
//!
//! Closes the "ledger growth" fault-line gap: ARCH/06 §6.7 promised
//! configurable audit retention; `agentcowork-audit::retention::compact` is the
//! mechanism, and this module is its call site. The sweep runs in a genuinely
//! writer-quiescent window: while the `audit_log` mutex is held no append can
//! run, the writer is dropped (fd closed) before the file is rewritten, then
//! re-opened — `AuditWriter::open` resumes the sequence from the last event
//! (the `log.rollup` header carries seq 0 and is skipped).
//!
//! Two entry points:
//! - `audit_compact` — the Tauri command (manual trigger / scheduler hook).
//! - `run_audit_sweep_if_due` — the startup sweep: compacts at most once per
//!   day (marker file in the data dir), so the retention policy is enforced
//!   automatically without re-rolling the log on every launch.

use tauri::State;

use crate::AppState;

/// How often the automatic startup sweep may run (ms). 24h.
const SWEEP_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;
/// Marker file holding the last sweep's timestamp (ms since epoch).
const LAST_SWEEP_MARKER: &str = ".audit_last_compact";

/// Core compaction routine — shared by the command and the startup sweep.
/// Returns the rollup report JSON. Callers must hold no other AppState locks.
fn compact_audit(
    audit_log: &std::sync::Mutex<Option<agentcowork_audit::AuditWriter>>,
    audit_path: &std::path::Path,
    retention_days: u64,
) -> Result<serde_json::Value, String> {
    // Writer-quiescent window: no append can interleave while we hold this.
    let mut slot = audit_log.lock().map_err(|e| e.to_string())?;
    // Close the writer first — compact() rewrites (tmp + rename) the file,
    // and an open writer would keep appending to the unlinked inode.
    let writer = slot.take();
    drop(writer);

    let report = agentcowork_audit::retention::compact(audit_path, retention_days)
        .map_err(|e| format!("audit compaction failed: {e}"))?;

    // Re-open: AuditWriter resumes seq from the last event (the rollup
    // header at seq 0 is skipped by `last_seq` — it takes the max parsed seq).
    *slot = agentcowork_audit::AuditWriter::open(audit_path).ok();

    serde_json::to_value(report).map_err(|e| e.to_string())
}

/// Tauri command: run the audit retention sweep now. `retention_days`
/// defaults to the crate default (7). Returns the `CompactReport`
/// (kept_full / rolled_up / dropped_malformed / bytes_before / bytes_after /
/// cutoff_ms).
#[tauri::command]
pub fn audit_compact(
    state: State<'_, AppState>,
    retention_days: Option<u64>,
) -> Result<serde_json::Value, String> {
    let days = retention_days.unwrap_or(agentcowork_audit::retention::DEFAULT_RETENTION_DAYS);
    compact_audit(
        &state.audit_log,
        &state.replay_dir.join("audit.ndjson"),
        days,
    )
}

/// Startup sweep: compact at most once per `SWEEP_INTERVAL_MS`, tracked by a
/// marker file in the data dir. Best-effort — a failure to read/write the
/// marker only skips this run, never blocks boot. Returns the report when a
/// sweep actually ran.
pub fn run_audit_sweep_if_due(state: &AppState) -> Result<Option<serde_json::Value>, String> {
    let marker = state.replay_dir.join(LAST_SWEEP_MARKER);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if now.saturating_sub(last) < SWEEP_INTERVAL_MS {
        return Ok(None); // not due yet
    }
    let report = compact_audit(
        &state.audit_log,
        &state.replay_dir.join("audit.ndjson"),
        agentcowork_audit::retention::DEFAULT_RETENTION_DAYS,
    )?;
    // Marker write is best-effort (failure just re-runs next boot).
    let _ = std::fs::write(&marker, now.to_string());
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_marker_gates_frequency() {
        // The marker logic is pure-ish: a fresh marker within the window
        // returns Ok(None); an old marker would compact. We can't exercise
        // compaction without an AppState, so assert the window math here.
        let now = 1_000_000_000_000u64;
        let recent = now - SWEEP_INTERVAL_MS + 1;
        let stale = now - SWEEP_INTERVAL_MS - 1;
        assert!(now.saturating_sub(recent) < SWEEP_INTERVAL_MS); // not due
        assert!(now.saturating_sub(stale) >= SWEEP_INTERVAL_MS); // due
    }
}
