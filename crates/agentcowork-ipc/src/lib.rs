//! agentcowork-ipc — the AgentCowork process contract.
//!
//! Every crossing between the Rust core and the TS coordinator sidecar uses
//! **JSON-RPC 2.0 over stdio with a length-prefix framing**:
//!
//! ```text
//! [u32 LE length][JSON payload]
//! ```
//!
//! - Length prefix is `u32` little-endian (bounded by [`MAX_FRAME_LEN`]).
//! - Oversized payloads are truncated into `ref:` handles at the app layer
//!   (spec C10 pass-by-reference); the transport itself stays bounded.
//! - The handshake mirrors ACP `initialize` (protocolVersion + optional-by-
//!   default capabilities, doc 45) so the contract can evolve without
//!   breaking older sides.
//! - The **base envelope** every `CTR-*` call carries — actor context,
//!   cooperative cancellation with deadline propagation, a ticket-derived
//!   idempotency key for effects, and a versioned `{ ok, value } | { error }`
//!   result — is kernel-owned and lives in `agentcowork_types::envelope`
//!   (`ARCH/10-KERNEL.md` §7). This crate carries JSON-RPC 2.0 framing and
//!   message shape only; callers map envelopes onto JSON-RPC.
//!
//! This is the P0.1 skeleton: framing + JSON-RPC message types + handshake
//! negotiation, all unit-tested. P0.5 wires it into the ProcessSupervisor
//! with backpressure, truncation and latency benchmarks.

pub mod budget;
pub mod channel;
pub mod frame;
pub mod handle;
pub mod message;
#[cfg(unix)]
pub mod socket;

pub use budget::{Budgeted, MessageKind, PayloadBudget, apply_budget, budget_for};
pub use channel::{BoundedChannel, DEFAULT_CAPACITY};
pub use frame::{FrameError, MAX_FRAME_LEN, encode};
pub use handle::{HandleRef, HandleStore, WirePayload};
pub use message::{JsonRpcError, Request, Response};
#[cfg(unix)]
pub use socket::{UnixFrameServer, request, socket_path};

/// Protocol version for the `initialize` handshake — mirrors the ACP
/// integer `protocolVersion` approach (only bumped on breaking changes).
pub const PROTOCOL_VERSION: u32 = 1;

/// Capabilities negotiated at handshake; all optional, default-off.
/// (doc 44 ABI-versioning patch: new capabilities ship additively.)
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    /// Sidecar may stream token deltas for chat (P1.4). Default: off.
    #[serde(default)]
    pub stream_deltas: bool,
    /// Sidecar may request pass-by-reference handles for large payloads (C10). Default: off.
    #[serde(default)]
    pub pass_by_reference: bool,
    /// Extensible: future capabilities are additive, never breaking.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_roundtrip_defaults_off() {
        let caps = Capabilities::default();
        assert!(!caps.stream_deltas);
        let json = serde_json::to_string(&caps).unwrap();
        let back: Capabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, back);
    }

    #[test]
    fn unknown_capabilities_are_tolerated() {
        // Future capability added by a newer peer must not break us.
        let json = r#"{"stream_deltas":true,"fancy_new_thing":42}"#;
        let caps: Capabilities = serde_json::from_str(json).unwrap();
        assert!(caps.stream_deltas);
        assert_eq!(
            caps.extra.get("fancy_new_thing"),
            Some(&serde_json::json!(42))
        );
    }
}
