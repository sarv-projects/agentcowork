//! P3.3 — distributed tracing (J14, doc 43 Agno-validated pattern).
//!
//! A shared `trace_id` ties Rust core → coordinator sidecar → provider call →
//! sandbox → audit row. Spans use the OpenTelemetry **W3C `traceparent`**
//! wire format (`00-<trace_id:32hex>-<span_id:16hex>-<flags:2hex>`) — the
//! exact header `@opentelemetry/sdk-node` reads on the sidecar, so the Node
//! coordinator can extract/continue the same trace. `TraceReporter` exports
//! to console + log file now; OTLP/Jaeger is post-v1 (SPEC J14).

use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Service names on the trace boundary (doc 43).
pub const SERVICE_CORE: &str = "agentcowork-core";
pub const SERVICE_COORDINATOR: &str = "coordinator";
pub const SERVICE_BROWSER: &str = "browser-child";
pub const SERVICE_SANDBOX: &str = "script-eval";

/// One exported span record — every doc 43 field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpanRecord {
    pub trace_id: String,
    pub span_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub service_name: String,
    pub service_version: String,
    pub session_id: String,
    pub agent_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<String>,
    pub duration_ms: u64,
    /// `ok` | `error`.
    pub status: String,
    pub ts_ms: u64,
}

/// A trace context: W3C traceparent-compatible ids + sampled flag + parent
/// linkage. Wrap it around a boundary crossing via [`TraceContext::traceparent`].
#[derive(Debug, Clone, PartialEq)]
pub struct TraceContext {
    span_context: SpanContext,
    parent_span_id: Option<SpanId>,
}

impl TraceContext {
    /// A new root trace (random trace_id + fresh span_id).
    pub fn new_root(sampled: bool) -> Self {
        Self {
            span_context: SpanContext::new(
                TraceId::from(rand_u128()),
                SpanId::from(rand_u64()),
                flags(sampled),
                false,
                TraceState::default(),
            ),
            parent_span_id: None,
        }
    }

    /// A child span: same trace_id, fresh span_id, this context as parent.
    pub fn child(&self, sampled: bool) -> Self {
        Self {
            span_context: SpanContext::new(
                self.trace_id(),
                SpanId::from(rand_u64()),
                flags(sampled),
                false,
                TraceState::default(),
            ),
            parent_span_id: Some(self.span_id()),
        }
    }

    pub fn trace_id(&self) -> TraceId {
        self.span_context.trace_id()
    }

    pub fn span_id(&self) -> SpanId {
        self.span_context.span_id()
    }

    pub fn sampled(&self) -> bool {
        self.span_context.trace_flags() == TraceFlags::SAMPLED
    }

    pub fn trace_id_hex(&self) -> String {
        format!("{:032x}", self.trace_id())
    }

    pub fn span_id_hex(&self) -> String {
        format!("{:016x}", self.span_id())
    }

    pub fn parent_span_id_hex(&self) -> Option<String> {
        self.parent_span_id.map(|s| format!("{:016x}", s))
    }

    /// The W3C `traceparent` header value (`00-<32hex>-<16hex>-<2hex>`).
    pub fn traceparent(&self) -> String {
        let flags = if self.sampled() { 0x01u8 } else { 0x00u8 };
        format!(
            "00-{:032x}-{:016x}-{:02x}",
            self.trace_id(),
            self.span_id(),
            flags
        )
    }

    /// Parse a W3C `traceparent` header — the same string the sidecar's
    /// `@opentelemetry/sdk-node` TraceContextPropagator emits/reads.
    pub fn parse_traceparent(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.trim().split('-').collect();
        if parts.len() != 4 || parts[0] != "00" {
            return None;
        }
        let trace_id = TraceId::from_hex(parts[1]).ok()?;
        let span_id = SpanId::from_hex(parts[2]).ok()?;
        if trace_id == TraceId::INVALID || span_id == SpanId::INVALID {
            return None;
        }
        let flags = u8::from_str_radix(parts[3], 16).ok()?;
        let trace_flags = if flags & 0x01 != 0 {
            TraceFlags::SAMPLED
        } else {
            TraceFlags::default()
        };
        Some(Self {
            span_context: SpanContext::new(
                trace_id,
                span_id,
                trace_flags,
                true,
                TraceState::default(),
            ),
            parent_span_id: None,
        })
    }

    /// Inject into HTTP headers (`traceparent` key) — provider/sidecar boundary.
    pub fn inject_headers(&self, headers: &mut HashMap<String, String>) {
        headers.insert("traceparent".to_string(), self.traceparent());
    }

    /// Extract from HTTP headers, if present.
    pub fn extract_headers(headers: &HashMap<String, String>) -> Option<Self> {
        headers
            .get("traceparent")
            .and_then(|v| Self::parse_traceparent(v))
    }
}

fn flags(sampled: bool) -> TraceFlags {
    if sampled {
        TraceFlags::SAMPLED
    } else {
        TraceFlags::default()
    }
}

/// NDJSON span export: console + log file under `<data_dir>/traces/`.
pub struct TraceReporter {
    /// `None` = console-only.
    file: Option<Mutex<File>>,
}

impl TraceReporter {
    pub fn console_only() -> Self {
        Self { file: None }
    }

    pub fn new(data_dir: &Path) -> Result<Self, TraceError> {
        let dir = data_dir.join("traces");
        std::fs::create_dir_all(&dir)?;
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("traces.ndjson"))?;
        Ok(Self {
            file: Some(Mutex::new(f)),
        })
    }

    /// Export one span. Not-sampled traces are dropped (OTel semantics).
    pub fn record(&self, ctx: &TraceContext, span: &SpanRecord) -> Result<(), TraceError> {
        if !ctx.sampled() {
            return Ok(());
        }
        let mut line = serde_json::to_vec(span)?;
        line.push(b'\n');
        if let Some(f) = &self.file {
            let mut f = f.lock().unwrap();
            f.write_all(&line)?;
            f.flush()?;
        } else {
            eprintln!("[trace] {}", String::from_utf8_lossy(&line).trim_end());
        }
        Ok(())
    }

    /// Convenience: build a span record from a finished tool execution.
    pub fn span(&self, ctx: &TraceContext, attrs: SpanAttrs<'_>) -> SpanRecord {
        SpanRecord::from_ctx(ctx, attrs)
    }
}

/// The doc-43 attribute set of one tool-execution span.
#[derive(Debug, Clone)]
pub struct SpanAttrs<'a> {
    pub service_name: &'a str,
    pub service_version: &'a str,
    pub session_id: &'a str,
    pub agent_id: &'a str,
    pub tool_name: &'a str,
    pub permission_decision: Option<&'a str>,
    pub duration_ms: u64,
    pub status: &'a str,
}

impl SpanRecord {
    /// Build the span record for a finished execution under `ctx`.
    pub fn from_ctx(ctx: &TraceContext, attrs: SpanAttrs<'_>) -> Self {
        Self {
            trace_id: ctx.trace_id_hex(),
            span_id: ctx.span_id_hex(),
            parent_span_id: ctx.parent_span_id_hex(),
            service_name: attrs.service_name.to_string(),
            service_version: attrs.service_version.to_string(),
            session_id: attrs.session_id.to_string(),
            agent_id: attrs.agent_id.to_string(),
            tool_name: attrs.tool_name.to_string(),
            permission_decision: attrs.permission_decision.map(String::from),
            duration_ms: attrs.duration_ms,
            status: attrs.status.to_string(),
            ts_ms: now_ms(),
        }
    }
}

fn rand_u64() -> u64 {
    use rand::RngCore;
    rand::thread_rng().next_u64()
}

fn rand_u128() -> u128 {
    use rand::RngCore;
    rand::thread_rng().next_u64() as u128 | ((rand::thread_rng().next_u64() as u128) << 64)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_child_link_ids() {
        let root = TraceContext::new_root(true);
        assert!(root.parent_span_id_hex().is_none());
        let child = root.child(true);
        assert_eq!(child.trace_id(), root.trace_id());
        assert_ne!(child.span_id(), root.span_id());
        assert_eq!(child.parent_span_id_hex(), Some(root.span_id_hex()));
        let grand = child.child(false);
        assert_eq!(grand.trace_id(), root.trace_id());
        assert_eq!(grand.parent_span_id_hex(), Some(child.span_id_hex()));
        assert!(!grand.sampled());
    }

    #[test]
    fn traceparent_roundtrip_and_headers() {
        let ctx = TraceContext::new_root(true);
        let tp = ctx.traceparent();
        assert!(tp.starts_with("00-"));
        assert_eq!(tp.len(), 55);
        let parsed = TraceContext::parse_traceparent(&tp).unwrap();
        assert_eq!(parsed.trace_id(), ctx.trace_id());
        assert_eq!(parsed.span_id(), ctx.span_id());
        assert!(parsed.sampled());

        let mut headers = HashMap::new();
        ctx.inject_headers(&mut headers);
        assert_eq!(headers["traceparent"], tp);
        let extracted = TraceContext::extract_headers(&headers).unwrap();
        assert_eq!(extracted.trace_id_hex(), ctx.trace_id_hex());
        assert_eq!(extracted.span_id_hex(), ctx.span_id_hex());
    }

    #[test]
    fn traceparent_rejects_garbage() {
        assert!(TraceContext::parse_traceparent("").is_none());
        assert!(TraceContext::parse_traceparent("01-abc-def-01").is_none());
        assert!(
            TraceContext::parse_traceparent(
                "00-00000000000000000000000000000000-abcdefabcdefabcd-01"
            )
            .is_none()
        ); // invalid trace id (all zeros)
        assert!(TraceContext::parse_traceparent("00-ff".repeat(20).as_str()).is_none());
    }

    #[test]
    fn not_sampled_is_not_exported() {
        let dir = std::env::temp_dir().join(format!("agentcowork-trace-ns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reporter = TraceReporter::new(&dir).unwrap();
        let ctx = TraceContext::new_root(false); // not sampled
        let span = reporter.span(
            &ctx,
            SpanAttrs {
                service_name: SERVICE_CORE,
                service_version: "0.1.0",
                session_id: "s",
                agent_id: "a",
                tool_name: "browser.act",
                permission_decision: None,
                duration_ms: 12,
                status: "ok",
            },
        );
        reporter.record(&ctx, &span).unwrap();
        let log = dir.join("traces/traces.ndjson");
        // The log file is created eagerly (empty) at open; not-sampled spans
        // must not add a single line to it.
        let body = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(body.is_empty(), "not-sampled spans must not be exported");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reporter_exports_ndjson_to_file() {
        let dir = std::env::temp_dir().join(format!("agentcowork-trace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reporter = TraceReporter::new(&dir).unwrap();
        let ctx = TraceContext::new_root(true).child(true);
        let span = reporter.span(
            &ctx,
            SpanAttrs {
                service_name: SERVICE_SANDBOX,
                service_version: "0.1.0",
                session_id: "sess-1",
                agent_id: "agent-a",
                tool_name: "run",
                permission_decision: Some("granted"),
                duration_ms: 42,
                status: "ok",
            },
        );
        reporter.record(&ctx, &span).unwrap();
        let log = dir.join("traces/traces.ndjson");
        let body = std::fs::read_to_string(&log).unwrap();
        assert!(body.ends_with('\n'));
        let parsed: SpanRecord = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(parsed, span);
        assert_eq!(parsed.service_name, "script-eval");
        assert_eq!(parsed.permission_decision.as_deref(), Some("granted"));
        assert_eq!(parsed.duration_ms, 42);
        assert_eq!(
            parsed.parent_span_id.as_deref(),
            Some(ctx.parent_span_id_hex().unwrap().as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn console_only_reporter_accepts_records() {
        let reporter = TraceReporter::console_only();
        let ctx = TraceContext::new_root(true);
        let span = reporter.span(
            &ctx,
            SpanAttrs {
                service_name: SERVICE_CORE,
                service_version: "0.1.0",
                session_id: "s",
                agent_id: "a",
                tool_name: "search",
                permission_decision: None,
                duration_ms: 5,
                status: "ok",
            },
        );
        reporter.record(&ctx, &span).unwrap();
    }
}
