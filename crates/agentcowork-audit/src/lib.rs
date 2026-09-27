//! agentcowork-audit — append-only NDJSON event log (ARCH/06 §6.5, J5).
//!
//! Every agent action lands here: tool calls, browser primitives, approvals,
//! token estimates, receipts. One JSON object per line, appended atomically.
//!
//! P0.1 scope: the append-writer primitive + event shape. P2.10 adds the
//! injected recorder ingest + sticky `has_gap` flag; P3.1 adds the replay
//! store and scrubber UI.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod cockpit;
pub mod merkle;
pub mod receipt;
pub mod repair;
pub mod replay;
pub mod retention;
pub mod session_log;

pub use receipt::{
    CostSummary, EffectReceipt, EvidenceRef, Provenance, ReceiptActionRef, ReceiptBuilder,
    VerificationSummary, WorkReceipt,
};
pub use repair::{StartedUnknownClassification, StartedUnknownItem, started_unknown_repair};
pub use session_log::{ForkLineage, ProjectedMessage};

/// One audit event. `kind` is a stable dotted name (e.g. `browser.act`,
/// `guard.blocked`, `vault.rotate`); `payload` is schema-per-kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    /// Monotonic sequence within this file (1-based).
    pub seq: u64,
    /// UNIX timestamp (ms).
    pub ts_ms: u64,
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    /// P3.3 (J14) — distributed-trace linkage: the W3C traceparent ids.
    /// Empty when the event wasn't recorded inside a trace.
    #[serde(default)]
    pub trace_id: String,
    #[serde(default)]
    pub span_id: String,
}

impl AuditEvent {
    pub fn new(kind: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            seq: 0,
            ts_ms: now_ms(),
            kind: kind.into(),
            payload,
            trace_id: String::new(),
            span_id: String::new(),
        }
    }

    /// Attach the trace context of the execution that produced this event.
    pub fn with_trace(mut self, trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        self.trace_id = trace_id.into();
        self.span_id = span_id.into();
        self
    }
}

/// Append-only NDJSON writer. One handle per audit log file; concurrent
/// writers are the ProcessSupervisor's concern (single-writer rule, doc 42).
pub struct AuditWriter {
    file: File,
    seq: u64,
}

impl AuditWriter {
    /// Open (or create) the log at `path` in append mode and resume the
    /// sequence from the last line (or 0).
    pub fn open(path: &Path) -> Result<Self, AuditError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true) // needed by last_seq() to scan the previous tail
            .append(true)
            .open(path)?;
        let seq = last_seq(&file)?;
        Ok(Self { file, seq })
    }

    /// Append one event as a JSON line + `\n`, flushed.
    pub fn write(&mut self, kind: &str, payload: serde_json::Value) -> Result<u64, AuditError> {
        self.write_traced(kind, payload, "", "")
    }

    /// P3.3 (J14) — append an event carrying its trace_id/span_id so the
    /// audit row ties to the span (doc 43: the pipeline is end-to-end
    /// traceable by a single trace_id).
    pub fn write_traced(
        &mut self,
        kind: &str,
        payload: serde_json::Value,
        trace_id: &str,
        span_id: &str,
    ) -> Result<u64, AuditError> {
        self.seq += 1;
        let event = AuditEvent {
            seq: self.seq,
            ts_ms: now_ms(),
            kind: kind.to_string(),
            payload,
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
        };
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        Ok(self.seq)
    }

    /// P45.5 — write a turn's audit lines in one append and one flush, in order.
    /// Each line is still one complete record.
    pub fn write_batch(
        &mut self,
        events: &[(&str, serde_json::Value)],
    ) -> Result<Vec<u64>, AuditError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut body = Vec::new();
        let mut seqs = Vec::with_capacity(events.len());
        for (kind, payload) in events {
            self.seq += 1;
            seqs.push(self.seq);
            let event = AuditEvent {
                seq: self.seq,
                ts_ms: now_ms(),
                kind: (*kind).to_string(),
                payload: payload.clone(),
                trace_id: String::new(),
                span_id: String::new(),
            };
            let mut line = serde_json::to_vec(&event)?;
            line.push(b'\n');
            body.extend_from_slice(&line);
        }
        self.file.write_all(&body)?;
        self.file.flush()?;
        Ok(seqs)
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// Determine the last sequence number in an append-mode file by scanning
/// its final line (best-effort; 0 on empty/corrupt tail).
fn last_seq(file: &File) -> Result<u64, AuditError> {
    use std::io::{Read, Seek};
    let mut buf = Vec::new();
    let mut f = file.try_clone()?;
    f.seek(io::SeekFrom::Start(0))?;
    f.read_to_end(&mut buf)?;
    let mut seq = 0u64;
    for line in buf.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(ev) = serde_json::from_slice::<AuditEvent>(line) {
            seq = ev.seq;
        }
    }
    Ok(seq)
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn appends_and_resumes_sequence() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-audit-test-{}", std::process::id()));
        let path = dir.join("audit.ndjson");
        let _ = std::fs::remove_dir_all(&dir);

        {
            let mut w = AuditWriter::open(&path).unwrap();
            assert_eq!(
                w.write("guard.blocked", serde_json::json!({"cmd": "rm -rf /"}))
                    .unwrap(),
                1
            );
            assert_eq!(
                w.write("browser.act", serde_json::json!({"ref": "e3"}))
                    .unwrap(),
                2
            );
        }

        // Reopen — sequence must resume at 2.
        {
            let mut w = AuditWriter::open(&path).unwrap();
            assert_eq!(w.seq(), 2);
            assert_eq!(w.write("vault.rotate", serde_json::json!({})).unwrap(), 3);
        }

        // Every line parses as a valid AuditEvent, in order.
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        let ev3: AuditEvent = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(ev3.seq, 3);
        assert_eq!(ev3.kind, "vault.rotate");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_batch_keeps_order_in_one_flush() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-audit-batch-{}", std::process::id()));
        let path = dir.join("audit.ndjson");
        let _ = std::fs::remove_dir_all(&dir);
        let mut writer = AuditWriter::open(&path).unwrap();
        let seqs = writer
            .write_batch(&[
                ("turn.start", serde_json::json!({"n": 1})),
                ("turn.tool", serde_json::json!({"n": 2})),
                ("turn.end", serde_json::json!({"n": 3})),
            ])
            .unwrap();
        assert_eq!(seqs, vec![1, 2, 3]);
        drop(writer);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("turn.start"));
        assert!(lines[2].contains("turn.end"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn traced_events_roundtrip_and_legacy_lines_parse() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-audit-trace-{}", std::process::id()));
        let path = dir.join("audit.ndjson");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut w = AuditWriter::open(&path).unwrap();
            // Legacy-shaped line (no trace fields) — must still parse.
            w.write("guard.blocked", serde_json::json!({"cmd": "rm"}))
                .unwrap();
            // P3.3 traced line.
            w.write_traced(
                "browser.act",
                serde_json::json!({"ref": "e3"}),
                "0123456789abcdef0123456789abcdef",
                "fedcba9876543210",
            )
            .unwrap();
        }
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(f)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        let legacy: AuditEvent = serde_json::from_str(&lines[0]).unwrap();
        assert!(legacy.trace_id.is_empty() && legacy.span_id.is_empty());
        let traced: AuditEvent = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(traced.trace_id, "0123456789abcdef0123456789abcdef");
        assert_eq!(traced.span_id, "fedcba9876543210");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_file_starts_at_zero() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-audit-empty-{}", std::process::id()));
        let path = dir.join("audit.ndjson");
        let _ = std::fs::remove_dir_all(&dir);

        let w = AuditWriter::open(&path).unwrap();
        assert_eq!(w.seq(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
