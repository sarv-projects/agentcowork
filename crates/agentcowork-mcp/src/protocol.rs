//! P36 (F6/F7 v3.40) — protocol primitives: **resources** = C10 over the
//! wire (uri + mime + bounded preview, never the blob); **elicitation** /
//! MRTR `InputRequired` → Guard-2 card; **sampling** → credential broker
//! only; **roots** = workspace path floor.
//!
//! These are the pure shapes + validation rules; transport glue lives in the
//! server/attach layers.

use serde::{Deserialize, Serialize};

/// The modern MCP spec revision this crate speaks first (DEC-030, ARCH/14 §4).
///
/// It is stateless: no `initialize` session is required, request context rides
/// in `_meta`, and the protocol version travels in a request header.
pub const MODERN_PROTOCOL_REVISION: &str = "2026-07-28";

/// The fallback revision whose contract is `initialize` + session state.
///
/// It is a *supported* fallback, not the default: a modern server is always
/// probed first (ARCH/14 §4).
pub const LEGACY_PROTOCOL_REVISION: &str = "2025-11-25";

/// The mandatory modern discovery method (DEC-030, ARCH/14 §4).
///
/// A stateless client has no handshake to hang discovery off, so
/// `server/discover` answers with everything `initialize` + `tools/list` used
/// to carry.
pub const DISCOVER_METHOD: &str = "server/discover";

/// The header carrying the negotiated revision on a modern request.
pub const PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";

/// The header that must agree with the JSON-RPC `method` of a modern request.
pub const METHOD_HEADER: &str = "Mcp-Method";

/// The header that must agree with `params.name` of a modern `tools/call`.
pub const NAME_HEADER: &str = "Mcp-Name";

/// An MCP resource — the wire form of C10 (pass-by-reference): a uri, a
/// mime type, and a **bounded preview**, never a blob dump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub mime_type: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Bound: preview must stay ≤ this many bytes (C10 budget).
    pub preview_bytes: usize,
    /// The actual bounded content (truncated at `preview_bytes`).
    pub content: String,
}

/// What a server can offer/expose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSpec {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<PromptArg>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptArg {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

/// Elicitation / MRTR `InputRequired`: the server needs a human answer mid-
/// operation. Everything here maps 1:1 onto a Guard-2 card (accept/decline/
/// cancel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRequired {
    pub request_id: String,
    pub kind: InputRequiredKind,
    pub prompt: String,
    pub options: Vec<InputOption>,
    /// The operation's expected idempotency class (doc 53 §4) — retried
    /// requests must carry the same key.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputRequiredKind {
    /// Choose from `options`.
    Select,
    /// Free-text answer.
    Text,
    /// A decision with legal accept/decline paths (Guard-2 style).
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputOption<T = String> {
    pub label: String,
    pub value: T,
}

/// The client's answer to an `InputRequired`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum InputAnswer {
    Accept { value: String },
    Decline { reason: Option<String> },
    Cancel,
}

/// Sampling: `sampling/createMessage` — the server asks the client to call
/// a model. **Broker only**: same budgets/tickets as chat, never the agent's
/// raw key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamplingRequest {
    pub request_id: String,
    pub model: Option<String>,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
}

/// Roots = workspace path floor. A server may only read within an offered
/// root; offering a root implicitly grants that floor (Gate: confirm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootSpec {
    pub name: String,
    /// Absolute path floor (canonical, no `..`).
    pub path: String,
}

impl RootSpec {
    /// Canonicalize + reject escapes. `None` = unsafe root.
    pub fn checked(path: &str) -> Option<Self> {
        let p = std::path::Path::new(path);
        if !p.is_absolute() {
            return None;
        }
        let canon = p.canonicalize().ok()?;
        // No path traversal: parent of the floor is never a floor.
        Some(Self {
            name: canon
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "root".into()),
            path: canon.to_string_lossy().into_owned(),
        })
    }
}

/// The optimizable prompt (MRTR TTL/ETag catalog response, doc 61).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogCache {
    pub etag: String,
    pub ttl_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_is_bounded() {
        let content = "x".repeat(10_000);
        let r = McpResource {
            uri: "file:///tmp/big.csv".into(),
            mime_type: Some("text/csv".into()),
            name: Some("big".into()),
            description: None,
            preview_bytes: 2_000,
            content: content[..2_000].to_string(),
        };
        let _ = &r;
        assert_eq!(r.content.len(), 2_000);
    }

    #[test]
    fn input_required_round_trip() {
        let ir = InputRequired {
            request_id: "op-1".into(),
            kind: InputRequiredKind::Confirm,
            prompt: "Apply the diff to 3 files?".into(),
            options: vec![InputOption {
                label: "Apply".into(),
                value: "yes".into(),
            }],
            idempotency_key: Some("key-1".into()),
        };
        let json = serde_json::to_string(&ir).unwrap();
        let back: InputRequired = serde_json::from_str(&json).unwrap();
        assert_eq!(back.request_id, "op-1");
        assert_eq!(back.idempotency_key.as_deref(), Some("key-1"));
    }

    #[test]
    fn root_rejects_relative_and_traversal() {
        assert!(RootSpec::checked("relative/path").is_none());
        assert!(RootSpec::checked("..").is_none());
        if let Some(ok) = RootSpec::checked(std::env::temp_dir().to_str().unwrap()) {
            assert!(ok.path.starts_with('/'));
        }
    }
}
