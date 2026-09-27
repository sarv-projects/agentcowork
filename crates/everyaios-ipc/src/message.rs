//! JSON-RPC 2.0 message types used on the wire.
//!
//! A minimal, strict-enough subset: `Request` (with optional `id` → a
//! notification when absent), `Response` (result or error). Field-level
//! validation happens in the app layer; this crate only carries the shape.
//!
//! # Errors are the kernel's, mapped — not re-declared
//!
//! JSON-RPC's `code` is a signed integer; the kernel's taxonomy
//! ([`everyaios_types::error::ErrorCode`]) is a closed set of nine named codes
//! with derived retryability. [`JsonRpcError::from`] maps a taxonomy code onto a
//! stable JSON-RPC code in the implementation-defined `-32000…-32099` range, and
//! [`JsonRpcError::to_boundary`] maps one back. Both directions round-trip for
//! all nine codes, which is what lets a UI switch on either spelling without a
//! translation table it has to keep in sync.
//!
//! Only three codes are shared with the JSON-RPC standard range — the ones the
//! standard already defines with the same meaning — and the mapping is
//! deliberate rather than accidental: a `-32601` from this crate is a routing
//! failure, which is a `NotFound` in the taxonomy, and a `-32700` is an
//! uninterpretable payload, which is `InvalidState`. The other five reserved
//! codes have no taxonomy counterpart and are refused rather than rounded into
//! one.

use everyaios_types::error::{BoundaryError, ErrorCode};
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

    /// The first implementation-defined code, which is where the taxonomy's
    /// stable codes live (`-32000…-32099` is JSON-RPC's reserved range for
    /// "server-defined", so nothing here can collide with a future standard
    /// code).
    pub const APPLICATION_CODE_BASE: i64 = -32000;

    // The nine stable application codes, named so the mapping is data both ways
    // and a peer can switch on the constant instead of the number.
    pub const AUTHORIZATION_DENIED: i64 = Self::APPLICATION_CODE_BASE - 1;
    pub const NOT_FOUND: i64 = Self::APPLICATION_CODE_BASE - 2;
    pub const CONFLICT: i64 = Self::APPLICATION_CODE_BASE - 3;
    pub const UNAVAILABLE: i64 = Self::APPLICATION_CODE_BASE - 4;
    pub const TIMEOUT: i64 = Self::APPLICATION_CODE_BASE - 5;
    pub const INVALID_STATE: i64 = Self::APPLICATION_CODE_BASE - 6;
    pub const GUIDANCE_REQUIRED: i64 = Self::APPLICATION_CODE_BASE - 7;
    pub const REQUIRES_USER_ACTION: i64 = Self::APPLICATION_CODE_BASE - 8;

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

    /// The stable JSON-RPC code for one taxonomy code.
    ///
    /// `Internal` reuses JSON-RPC's own `-32603` rather than taking an
    /// application code: it is the same fact, and a peer that special-cases the
    /// standard range keeps working.
    pub const fn code_for(code: ErrorCode) -> i64 {
        match code {
            ErrorCode::AuthorizationDenied => Self::AUTHORIZATION_DENIED,
            ErrorCode::NotFound => Self::NOT_FOUND,
            ErrorCode::Conflict => Self::CONFLICT,
            ErrorCode::Unavailable => Self::UNAVAILABLE,
            ErrorCode::Timeout => Self::TIMEOUT,
            ErrorCode::InvalidState => Self::INVALID_STATE,
            ErrorCode::GuidanceRequired => Self::GUIDANCE_REQUIRED,
            ErrorCode::RequiresUserAction => Self::REQUIRES_USER_ACTION,
            ErrorCode::Internal => Self::INTERNAL_ERROR,
        }
    }

    /// The taxonomy code a JSON-RPC code denotes, when it denotes one.
    ///
    /// The three shared standard codes map by *meaning*, not by number
    /// coincidence: an unknown method is a target that does not exist, and an
    /// unparseable payload is a document this build cannot interpret. The
    /// remaining reserved codes have no counterpart and are `None`, so a caller
    /// is forced to decide what an unrecognised code means instead of inheriting
    /// a guess.
    pub const fn taxonomy_for(code: i64) -> Option<ErrorCode> {
        match code {
            Self::AUTHORIZATION_DENIED => Some(ErrorCode::AuthorizationDenied),
            Self::NOT_FOUND => Some(ErrorCode::NotFound),
            Self::CONFLICT => Some(ErrorCode::Conflict),
            Self::UNAVAILABLE => Some(ErrorCode::Unavailable),
            Self::TIMEOUT => Some(ErrorCode::Timeout),
            Self::INVALID_STATE => Some(ErrorCode::InvalidState),
            Self::GUIDANCE_REQUIRED => Some(ErrorCode::GuidanceRequired),
            Self::REQUIRES_USER_ACTION => Some(ErrorCode::RequiresUserAction),
            Self::METHOD_NOT_FOUND => Some(ErrorCode::NotFound),
            Self::PARSE_ERROR | Self::INVALID_REQUEST => Some(ErrorCode::InvalidState),
            Self::INTERNAL_ERROR => Some(ErrorCode::Internal),
            _ => None,
        }
    }

    /// The boundary projection of a taxonomy code + message. The message goes
    /// through the kernel's scrubber, so this is a safe thing to put on the wire
    /// even when the caller built it carelessly.
    pub fn from_boundary(error: &BoundaryError) -> Self {
        let mut mapped = Self::new(Self::code_for(error.code), error.message.clone());
        mapped.data = error.cause.clone().map(|cause| {
            serde_json::json!({
                "cause": cause,
                "retryable": error.retryable,
                "retry_class": error.code.retry_class().as_str(),
                "next_steps": error.next_steps,
            })
        });
        if mapped.data.is_none() {
            mapped.data = Some(serde_json::json!({
                "retryable": error.retryable,
                "retry_class": error.code.retry_class().as_str(),
                "next_steps": error.next_steps,
            }));
        }
        mapped
    }

    /// The boundary form of this error, so a JSON-RPC peer and a typed caller
    /// read the same fact the same way.
    pub fn to_boundary(&self) -> BoundaryError {
        let code = Self::taxonomy_for(self.code).unwrap_or(ErrorCode::Internal);
        let mut boundary = BoundaryError::new(code, self.message.clone());
        let mut cause = None;
        let mut steps = Vec::new();
        if let Some(data) = &self.data {
            cause = data
                .get("cause")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            steps = data
                .get("next_steps")
                .and_then(serde_json::Value::as_array)
                .map(|steps| {
                    steps
                        .iter()
                        .filter_map(|step| {
                            let instruction = step.get("instruction")?.as_str()?.to_string();
                            let target = step
                                .get("target")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string);
                            Some(everyaios_types::error::NextStep {
                                instruction,
                                target,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        if let Some(cause) = cause {
            boundary = boundary.with_cause(cause);
        }
        if !steps.is_empty() {
            // A non-actionable code cannot carry steps; dropping them is the
            // honest mapping, and the code still says what happened.
            if let Ok(with_steps) = boundary.clone().try_with_next_steps(steps) {
                boundary = with_steps;
            }
        }
        boundary
    }
}

impl From<ErrorCode> for JsonRpcError {
    fn from(code: ErrorCode) -> Self {
        Self::new(Self::code_for(code), code.as_str())
    }
}

impl From<&BoundaryError> for JsonRpcError {
    fn from(error: &BoundaryError) -> Self {
        Self::from_boundary(error)
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

    #[test]
    fn every_taxonomy_code_has_a_stable_json_rpc_code() {
        for code in ErrorCode::ALL {
            let mapped = JsonRpcError::code_for(code);
            assert_eq!(
                JsonRpcError::taxonomy_for(mapped),
                Some(code),
                "{code} must round-trip"
            );
        }
        for code in ErrorCode::ALL {
            let mapped = JsonRpcError::code_for(code);
            if code == ErrorCode::Internal {
                // `Internal` deliberately reuses the standard code: it is the
                // same fact, and a peer that special-cases `-32603` keeps working.
                assert_eq!(mapped, JsonRpcError::INTERNAL_ERROR);
                continue;
            }
            // The application range is reserved for server-defined codes, so a
            // future standard assignment cannot collide with ours.
            assert!(
                (-32099..=-32000).contains(&mapped),
                "{code} → {mapped} is outside the server-defined range"
            );
        }
        // The three shared standard codes map by meaning.
        assert_eq!(
            JsonRpcError::taxonomy_for(JsonRpcError::METHOD_NOT_FOUND),
            Some(ErrorCode::NotFound)
        );
        assert_eq!(
            JsonRpcError::taxonomy_for(JsonRpcError::PARSE_ERROR),
            Some(ErrorCode::InvalidState)
        );
        // A code with no counterpart is refused, not guessed.
        assert_eq!(JsonRpcError::taxonomy_for(-32099), None);
        assert_eq!(JsonRpcError::taxonomy_for(42), None);
        // The standard codes that do not map are genuinely unmapped.
        for reserved in [-32602, -32604, -32605] {
            assert_eq!(JsonRpcError::taxonomy_for(reserved), None, "{reserved}");
        }
    }

    #[test]
    fn a_boundary_error_survives_the_json_rpc_mapping_in_both_directions() {
        let original = everyaios_types::error::KernelError::conflict("the lease moved")
            .with_diagnostic("lease.rs:220")
            .into_boundary();
        let mapped = JsonRpcError::from(&original);
        assert_eq!(mapped.code, JsonRpcError::code_for(ErrorCode::Conflict));
        // The diagnostic stayed in-process.
        assert!(!serde_json::to_string(&mapped).unwrap().contains("lease.rs"));
        let back = mapped.to_boundary();
        assert_eq!(back.code, original.code);
        assert_eq!(back.message, original.message);
        assert_eq!(back.cause, original.cause);
        assert_eq!(back.retryable, original.retryable);
    }

    #[test]
    fn guidance_survives_the_mapping_with_its_next_steps() {
        let original = everyaios_types::error::KernelError::guidance(
            "the drive connector is not connected",
            vec![everyaios_types::error::NextStep::at(
                "Connect Google Drive",
                "settings/connectors/drive",
            )],
        )
        .into_boundary();
        let mapped = JsonRpcError::from(&original);
        let back = mapped.to_boundary();
        assert_eq!(back.code, ErrorCode::GuidanceRequired);
        assert_eq!(back.next_steps.len(), 1);
        assert_eq!(back.next_steps[0].instruction, "Connect Google Drive");
        assert_eq!(
            back.next_steps[0].target.as_deref(),
            Some("settings/connectors/drive")
        );
    }

    #[test]
    fn a_mapped_message_is_scrubbed_on_the_way_out() {
        // The mapper is a crossing point, so it scrubs even a careless message.
        let careless = everyaios_types::error::KernelError::internal(
            "vault open failed for /home/u/.everyaios/vault.db with api_key=sk-live-1",
        )
        .into_boundary();
        assert!(!careless.message.contains("/home/u"));
        assert!(!careless.message.contains("sk-live-1"));
        let mapped = JsonRpcError::from(&careless);
        assert!(!mapped.message.contains("sk-live-1"));
    }

    #[test]
    fn an_unrecognised_code_becomes_internal_never_a_guess() {
        let foreign = JsonRpcError::new(-32099, "some other server's error");
        assert_eq!(foreign.to_boundary().code, ErrorCode::Internal);
    }
}
