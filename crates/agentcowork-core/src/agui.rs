//! AG-UI live transport (P11.5.11 — H25, doc 50). The Rust twin of the
//! coordinator's `agui.ts` codec: tool calls + UI updates ride ONE JSON
//! channel (~16 event types), with generative-UI payloads inside `artifact`
//! events.
//!
//! Wire contract (matches `agui.ts` byte-for-byte):
//! ```json
//! { "type": "tool_call_created", "id": "c1", "ts": "<iso>", "data": {…} }
//! ```
//! - coordinator → Rust → UI: the coordinator notifies `agui/event`
//!   (`params.line` = the encoded envelope line); the relay forwards it to
//!   the `on_agui` sink → Tauri emits `agui-event` with the raw line.
//! - UI → coordinator: the Tauri `agui_send` command writes an `agui/event`
//!   notification to the sidecar; the coordinator dispatches it (e.g.
//!   `interrupt_resolved` answers an outstanding AG-UI interrupt).
//!
//! **Build state (P11.5.11, deferred post-v1): the Rust/coordinator transport
//! above is complete and tested, but there is no UI consumer yet.**
//! `scripts/ipc-parity.mjs` reports `agui-event` as the one shell-emitted event
//! nothing listens for, and both `agui_send` and `agui_listen` as commands the
//! UI never invokes. Generative UI (H25) is the deferred feature that would
//! consume them — so the emitter is real, the surface above it is not, and
//! nothing should be described as "live" until the UI half lands.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// The AG-UI envelope (wire shape identical to the TS `AguiEnvelope`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AguiEnvelope {
    /// One of the 16 AG-UI event types (snake_case over the wire).
    pub r#type: String,
    /// Correlation id (maps to the framed IPC message id).
    pub id: String,
    /// ISO timestamp.
    pub ts: String,
    /// Event payload (tool calls, artifacts, interrupts, …).
    pub data: serde_json::Value,
}

/// Encode one envelope into a single JSON line (the `agui/event` wire unit).
pub fn encode_agui(r#type: &str, id: &str, ts: &str, data: serde_json::Value) -> String {
    serde_json::to_string(&AguiEnvelope {
        r#type: r#type.to_string(),
        id: id.to_string(),
        ts: ts.to_string(),
        data,
    })
    .unwrap_or_else(|_| r#"{"type":"error","id":"","ts":"","data":{}}"#.to_string())
}

/// Decode one `agui/event` line; `None` on malformed input (forward-compat:
/// an unknown event type still decodes — the `type` is an open string).
pub fn decode_agui(line: &str) -> Option<AguiEnvelope> {
    serde_json::from_str(line).ok()
}

/// Convenience envelope constructors (test + relay ergonomics).
pub fn envelope(r#type: &str, id: &str, ts: &str, data: serde_json::Value) -> AguiEnvelope {
    AguiEnvelope {
        r#type: r#type.into(),
        id: id.into(),
        ts: ts.into(),
        data,
    }
}

/// The UI event sink: receives raw encoded envelope lines.
type AguiSink = Box<dyn Fn(String) + Send>;

/// Forwards `agui/event` lines to the UI (Tauri emits `agui-event`).
#[derive(Clone, Default)]
pub struct AguiRelay {
    on_event: Arc<Mutex<Option<AguiSink>>>,
}

impl AguiRelay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach (or replace) the UI sink. Call once at boot from the Tauri shell.
    pub fn attach(&self, sink: impl Fn(String) + Send + 'static) {
        *self.on_event.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(sink));
    }

    /// Forward one encoded envelope line to the UI (fire-and-forget).
    pub fn forward(&self, line: &str) {
        if let Some(sink) = self
            .on_event
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            sink(line.to_string());
        }
    }

    /// Forward one envelope (encode-then-forward).
    pub fn forward_envelope(&self, env: &AguiEnvelope) {
        self.forward(&serde_json::to_string(env).unwrap_or_default());
    }

    /// True when a sink is attached (the UI is listening).
    pub fn is_attached(&self) -> bool {
        self.on_event
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_matches_wire_shape() {
        let env = envelope(
            "tool_call_created",
            "c1",
            "2026-08-23T00:00:00Z",
            serde_json::json!({ "callId": "t1", "name": "read_file", "args": { "path": "a.rs" } }),
        );
        let line = serde_json::to_string(&env).unwrap();
        let decoded = decode_agui(&line).expect("round-trips");
        assert_eq!(decoded, env);
        assert_eq!(decoded.r#type, "tool_call_created");
        assert_eq!(decoded.data["name"], "read_file");
    }

    #[test]
    fn encode_agui_produces_decodable_line() {
        let line = encode_agui("artifact_created", "c2", "t", serde_json::json!({ "v": 1 }));
        let env = decode_agui(&line).expect("decodes");
        assert_eq!(env.r#type, "artifact_created");
        assert_eq!(env.id, "c2");
    }

    #[test]
    fn malformed_line_decodes_to_none() {
        assert!(decode_agui("not json").is_none());
        assert!(decode_agui("").is_none());
        // Missing `data` → not a valid envelope (matches the TS codec, where
        // `data` is required). Unknown *types* still decode (open string).
        assert!(decode_agui(r#"{"type":"done","id":"x","ts":"t"}"#).is_none());
    }

    #[test]
    fn relay_forwards_only_when_attached() {
        let relay = AguiRelay::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        let got2 = Arc::clone(&got);
        relay.attach(move |line| got2.lock().unwrap().push(line));
        relay.forward("{\"type\":\"done\"}");
        relay.forward_envelope(&envelope(
            "session_created",
            "s1",
            "t",
            serde_json::json!({}),
        ));
        let lines = got.lock().unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("session_created"));
    }

    #[test]
    fn unknown_event_types_still_decode() {
        let line = encode_agui(
            "brand_new_event_kind",
            "x",
            "t",
            serde_json::json!({ "a": 1 }),
        );
        let env = decode_agui(&line).unwrap();
        assert_eq!(env.r#type, "brand_new_event_kind");
    }
}
