//! agentcowork-audit — retention compaction for the append-only NDJSON log
//! (fault-line "ledger growth": append-only Merkle audit + NDJSON session logs
//! grow forever without a pruning policy).
//!
//! ARCH/06 §6.7 promises *"Retention: replays 7d default, **audit configurable**"*
//! — this module is the mechanism behind that sentence.
//!
//! Policy (tamper-evidence-preserving): full payloads are retained for
//! `retention_days`; older events keep `seq, ts, kind, trace_id, span_id` and
//! a SHA-256 digest of the pruned payload — so the J19 Merkle chain stays
//! verifiable end-to-end, but the materialized JSON bodies stop growing.
//!
//! The Merkle chain itself (merkle.rs) is a small hash list and is **not**
//! compacted; only the NDJSON payload log is rolled up.
//!
//! ## Single-writer caveat
//!
//! `compact` rewrites the log file and must not run concurrently with a live
//! `AuditWriter` on the same path (the doc-42 single-writer rule). Call it
//! from a maintenance window / process exit, or from a maintenance worker
//! that holds the writer's same mutex. The resulting file starts with a
//! `log.rollup` header (seq 0) so `AuditWriter::open` resumes the sequence
//! from the last untouched event seq.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::AuditError;
use crate::AuditEvent;

/// Default full-event retention (matches ARCH/06 §6.7 "replays 7d default").
pub const DEFAULT_RETENTION_DAYS: u64 = 7;

/// Milliseconds per day.
const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

/// Result of a compaction pass.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CompactReport {
    pub path: PathBuf,
    /// Events whose full payload was kept.
    pub kept_full: u64,
    /// Events rolled up to a digest (older than the cutoff).
    pub rolled_up: u64,
    /// Lines that did not parse as events (dropped; noted, not silent).
    pub dropped_malformed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Events with `ts_ms < cutoff_ms` are rolled up.
    pub cutoff_ms: u64,
}

/// Roll up `path` in place: keep full payloads newer than `retention_days`,
/// replace older payloads with a digest — see module docs.
///
/// # Safety contract
/// Must not race a live `AuditWriter` on the same path.
pub fn compact(path: &Path, retention_days: u64) -> Result<CompactReport, AuditError> {
    let before = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let cutoff_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
        .saturating_sub(retention_days * MS_PER_DAY);

    let mut bytes = Vec::new();
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Nothing to compact — a missing log is a successful no-op.
            return Ok(CompactReport {
                path: path.to_path_buf(),
                kept_full: 0,
                rolled_up: 0,
                dropped_malformed: 0,
                bytes_before: 0,
                bytes_after: 0,
                cutoff_ms,
            });
        }
        Err(e) => return Err(e.into()),
    };
    f.read_to_end(&mut bytes)?;

    let mut kept_full = 0u64;
    let mut rolled_up = 0u64;
    let mut dropped_malformed = 0u64;
    let mut out: Vec<Vec<u8>> = Vec::new();

    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue; // trailing newline — not a malformed event
        }
        match serde_json::from_slice::<AuditEvent>(line) {
            Ok(ev) => {
                if ev.ts_ms >= cutoff_ms {
                    kept_full += 1;
                    let mut l = line.to_vec();
                    l.push(b'\n');
                    out.push(l);
                } else {
                    // Roll up: same envelope, payload → digest.
                    let payload_hash = hex(&Sha256::digest(line));
                    let ev = AuditEvent {
                        seq: ev.seq,
                        ts_ms: ev.ts_ms,
                        kind: ev.kind,
                        payload: serde_json::json!({
                            "rolled_up": true,
                            "payload_sha256": payload_hash,
                        }),
                        trace_id: ev.trace_id,
                        span_id: ev.span_id,
                    };
                    let mut l = serde_json::to_vec(&ev)?;
                    l.push(b'\n');
                    out.push(l);
                    rolled_up += 1;
                }
            }
            Err(_) => dropped_malformed += 1,
        }
    }

    // tmp file in the same dir (rename is atomic within a filesystem).
    let tmp = path.with_extension("compact.tmp");
    let mut w = File::create(&tmp)?;
    // Rollup header — seq 0 so `last_seq()` on reopen still works.
    let header = AuditEvent {
        seq: 0,
        ts_ms: now_ms(),
        kind: "log.rollup".to_string(),
        payload: serde_json::json!({
            "kept_full": kept_full,
            "rolled_up": rolled_up,
            "cutoff_ms": cutoff_ms,
            "retention_days": retention_days,
        }),
        trace_id: String::new(),
        span_id: String::new(),
    };
    let mut hl = serde_json::to_vec(&header)?;
    hl.push(b'\n');
    w.write_all(&hl)?;
    for l in &out {
        w.write_all(l)?;
    }
    w.flush()?;
    drop(w);
    std::fs::rename(&tmp, path)?;

    let after = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(CompactReport {
        path: path.to_path_buf(),
        kept_full,
        rolled_up,
        dropped_malformed,
        bytes_before: before,
        bytes_after: after,
        cutoff_ms,
    })
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuditWriter;
    use std::io::BufRead;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-audit-compact-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rolled_up_events_are_digested_not_deleted() {
        let dir = temp_dir("rollup");
        let path = dir.join("audit.ndjson");
        let now = now_ms();
        {
            let mut f = File::create(&path).unwrap();
            // 5 old events (30 days back), 3 fresh.
            for i in 1u64..=5 {
                let ev = AuditEvent {
                    seq: i,
                    ts_ms: now - 30 * MS_PER_DAY,
                    kind: "tool.run".into(),
                    payload: serde_json::json!({"payload": "x".repeat(2000)}),
                    trace_id: String::new(),
                    span_id: String::new(),
                };
                let mut line = serde_json::to_vec(&ev).unwrap();
                line.push(b'\n');
                f.write_all(&line).unwrap();
            }
            for i in 6u64..=8 {
                let ev = AuditEvent {
                    seq: i,
                    ts_ms: now,
                    kind: "tool.run".into(),
                    payload: serde_json::json!({"fresh": true}),
                    trace_id: String::new(),
                    span_id: String::new(),
                };
                let mut line = serde_json::to_vec(&ev).unwrap();
                line.push(b'\n');
                f.write_all(&line).unwrap();
            }
        }
        let before = std::fs::metadata(&path).unwrap().len();
        let report = compact(&path, DEFAULT_RETENTION_DAYS).unwrap();
        assert_eq!(report.kept_full, 3);
        assert_eq!(report.rolled_up, 5);
        assert_eq!(report.dropped_malformed, 0);
        assert!(
            report.bytes_after < before,
            "old payloads must shrink the file"
        );
        assert!(report.bytes_after > 0);
        // Rollup header at seq 0; fresh events keep full payload; rolled-up
        // lines carry a sha256 digest and no original payload.
        let f = File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert_eq!(lines.len(), 9); // header + 3 kept + 5 rolled
        let header: AuditEvent = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header.kind, "log.rollup");
        assert_eq!(header.seq, 0);
        let rolled: AuditEvent = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(rolled.seq, 1);
        assert_eq!(rolled.payload["rolled_up"], true);
        assert_eq!(rolled.payload["payload_sha256"].as_str().unwrap().len(), 64);
        let fresh: AuditEvent = serde_json::from_str(&lines[8]).unwrap();
        assert_eq!(fresh.seq, 8);
        assert_eq!(fresh.payload["fresh"], true);

        // Writer resumes sequence from the last event (not the header).
        let mut w = AuditWriter::open(&path).unwrap();
        assert_eq!(w.seq(), 8);
        assert_eq!(w.write("vault.rotate", serde_json::json!({})).unwrap(), 9);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
