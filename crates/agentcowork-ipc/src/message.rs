//! JSON-RPC 2.0 message types used on the wire.
//!
//! A minimal, strict-enough subset: `Request` (with optional `id` → a
//! notification when absent), `Response` (result or error). Field-level
//! validation happens in the app layer; this crate only carries the shape.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 marker — every message declares this.
pub const JSONRPC: &str = "2.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub jsonrpc: String,
    /// Method name (e.g. `chat/stream`, `browser/act`, `vault/rotate`).
    pub method: String,
    /// Positional or named params. `None` = absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Present = a request awaiting a response; absent = notification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
}

impl Request {
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            method: method.into(),
            params: None,
            id: Some(serde_json::json!(next_id())),
        }
    }

    pub fn with_params(mut self, params: serde_json::Value) -> Self {
        self.params = Some(params);
        self
    }

    /// True when this is a notification (no `id`) — fire-and-forget.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl Response {
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id: Some(id),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// Standard JSON-RPC error object (code/message/data).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcError {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INTERNAL_ERROR: i64 = -32603;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            Self::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )
    }
}

/// Monotonic id generator for requests (simple counter; the real sidecar
/// will use a per-session counter seeded from the handshake).
static ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_id() -> u64 {
    ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_with_id() {
        let req = Request::new("browser/snapshot")
            .with_params(serde_json::json!({"mode": "interactive"}));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"browser/snapshot\""));
        assert!(json.contains("\"id\":"));
        assert!(json.contains("\"params\":{\"mode\":\"interactive\"}"));
    }

    #[test]
    fn request_without_id_is_notification() {
        let mut req = Request::new("session/ping");
        req.id = None;
        assert!(req.is_notification());
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn response_ok_and_err_roundtrip() {
        let ok = Response::ok(serde_json::json!(1), serde_json::json!({"ok": true}));
        let back: Response = serde_json::from_str(&serde_json::to_string(&ok).unwrap()).unwrap();
        assert_eq!(back, ok);
        assert!(back.error.is_none());

        let err = Response::err(
            serde_json::json!(1),
            JsonRpcError::METHOD_NOT_FOUND,
            "browser/nope",
        );
        let back: Response = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
        assert_eq!(back.error.unwrap().code, JsonRpcError::METHOD_NOT_FOUND);
    }

    #[test]
    fn method_not_found_helper() {
        let e = JsonRpcError::method_not_found("browser/nope");
        assert_eq!(e.code, -32601);
        assert!(e.message.contains("browser/nope"));
    }
}
