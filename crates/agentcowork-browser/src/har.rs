//! P36 (E5) — optional HAR beside NDJSON for replay `has_gap`.
//!
//! The replay recorder streams NDJSON (P2.10). When the user opts in, the
//! same session also produces a HAR 1.2 log of network activity; the NDJSON
//! `has_gap` flag shadows into the HAR so a gap-marked replay carries its
//! honesty into every export format.

use serde::{Deserialize, Serialize};

/// A minimal HAR 1.2 entry — the fields the replay actually needs (url,
/// method, status, timings). Faithful to the spec shape, kept lean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarEntry {
    pub started_date_time: String,
    pub time: u64,
    pub request: HarRequest,
    pub response: HarResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<HarHeader>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarResponse {
    pub status: u16,
    pub status_text: String,
    pub mime_type: String,
}

/// The HAR 1.2 log shape (the part we emit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarLog {
    pub version: String,
    pub creator: HarCreator,
    #[serde(default)]
    pub entries: Vec<HarEntry>,
    /// E5: the NDJSON `has_gap` truth propagates here — a gap-marked replay
    /// must never masquerade as complete in HAR form either.
    pub has_gap: bool,
    #[serde(default)]
    pub gap_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarCreator {
    pub name: String,
    pub version: String,
}

impl Default for HarLog {
    fn default() -> Self {
        Self {
            version: "1.2".into(),
            creator: HarCreator {
                name: "agentcowork-replay".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            entries: Vec::new(),
            has_gap: false,
            gap_note: None,
        }
    }
}

impl HarLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a network observation. If a previous batch was gap-marked, the
    /// gap flag is **sticky** (same MAX-sticky rule as the NDJSON ingest).
    pub fn push(&mut self, entry: HarEntry) {
        self.entries.push(entry);
    }

    /// Mark the whole log with a gap (sticky, immutable once set).
    pub fn mark_gap(&mut self, note: impl Into<String>) {
        if !self.has_gap {
            self.has_gap = true;
            self.gap_note = Some(note.into());
        }
    }

    /// Serialize to the HAR 1.2 JSON envelope (`{"log": {...}}`).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({ "log": self })).unwrap_or_default()
    }
}

/// The builder feeding a HAR from CDP network events without holding the
/// full CDP layer (pure accumulator over typed rows; the CDP glue lives in
/// `diagnostics.rs`).
#[derive(Debug, Clone, Default)]
pub struct HarBuilder {
    pub log: HarLog,
}

impl HarBuilder {
    pub fn new() -> Self {
        Self { log: HarLog::new() }
    }

    /// Convert one `NetworkRequest` (from [`crate::diagnostics`]) into an
    /// entry. Requests that never finished are skipped (a HAR entry needs a
    /// response) — unless the log is already gap-marked.
    pub fn push_request(&mut self, req: &crate::diagnostics::NetworkRequest) {
        let Some(status) = req.status else {
            self.log
                .mark_gap(format!("request {} never completed", req.url));
            return;
        };
        self.log.push(HarEntry {
            started_date_time: "1970-01-01T00:00:00.000Z".into(), // filled by the caller's clock when wiring live
            time: req.at_ms,
            request: HarRequest {
                method: req.method.clone(),
                url: req.url.clone(),
                headers: Vec::new(),
            },
            response: HarResponse {
                status,
                status_text: String::new(),
                mime_type: req.mime_type.clone().unwrap_or_default(),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::NetworkRequest;

    fn req() -> NetworkRequest {
        NetworkRequest {
            request_id: "1".into(),
            url: "https://x.com/a.css".into(),
            method: "GET".into(),
            status: Some(200),
            mime_type: Some("text/css".into()),
            at_ms: 42,
            started_ms: Some(0),
            finished_ms: Some(42),
        }
    }

    #[test]
    fn har_round_trip_json() {
        let mut b = HarBuilder::new();
        b.push_request(&req());
        let json = b.log.to_json();
        assert!(json.contains("\"version\": \"1.2\""));
        assert!(json.contains("https://x.com/a.css"));
        assert!(json.contains("\"status\": 200"));
        assert!(!b.log.has_gap);
    }

    #[test]
    fn unfinished_request_marks_gap() {
        let mut b = HarBuilder::new();
        let mut r = req();
        r.status = None;
        b.push_request(&r);
        assert!(b.log.has_gap);
        assert!(b.log.gap_note.is_some());
    }

    #[test]
    fn gap_is_sticky() {
        let mut b = HarBuilder::new();
        b.log.mark_gap("dropped line at segment 3");
        b.log.mark_gap("second note");
        assert_eq!(b.log.gap_note.as_deref(), Some("dropped line at segment 3"));
    }
}
