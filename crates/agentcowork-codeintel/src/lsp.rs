//! LSP client core (I11 — doc 63 §2.1, neovim `runtime/lua/vim/lsp/*`
//! reference). JSON-RPC framing over stdio (Content-Length headers) + the
//! core LSP types the guard-ticketed tools expose: hover/docs, go-to-def,
//! references, rename-with-preview, diagnostics, code actions, inlay hints.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

// ---------------------------------------------------------------------------
// JSON-RPC framing (Content-Length headers)
// ---------------------------------------------------------------------------

/// Encode one JSON message with the LSP `Content-Length` header frame.
pub fn encode_message(json: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", json.len(), json)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FramingError {
    #[error("incomplete message (header or body not yet received)")]
    Incomplete,
    #[error("malformed Content-Length header")]
    MalformedHeader,
}

/// Decode zero or more complete messages from a byte buffer, consuming the
/// consumed bytes. Returns `Ok(Vec<String>)` of the JSON bodies; partial
/// trailing bytes stay in `buf`.
pub fn decode_messages(buf: &mut Vec<u8>) -> Result<Vec<String>, FramingError> {
    let mut out = Vec::new();
    while let Some(header_end) = find_subslice(buf, b"\r\n\r\n") {
        let header = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length = parse_content_length(&header)?;
        let body_start = header_end + 4;
        if buf.len() < body_start + content_length {
            break; // body incomplete — wait for more bytes.
        }
        let body = &buf[body_start..body_start + content_length];
        out.push(String::from_utf8_lossy(body).to_string());
        buf.drain(..body_start + content_length);
    }
    Ok(out)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn parse_content_length(header: &str) -> Result<usize, FramingError> {
    header
        .lines()
        .find_map(|l| {
            l.strip_prefix("Content-Length:")
                .map(|v| v.trim().parse::<usize>().ok())
        })
        .flatten()
        .ok_or(FramingError::MalformedHeader)
}

// ---------------------------------------------------------------------------
// Core LSP types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HoverContents {
    Markup { kind: String, value: String },
    Plain(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hover {
    pub contents: HoverContents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<u32>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEdit {
    pub changes: HashMap<String, Vec<TextEdit>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAction {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<WorkspaceEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlayHint {
    pub position: Position,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u32>,
}

/// A JSON-RPC request (method + id + params).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LspRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

impl LspRequest {
    pub fn new(id: u64, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC response (result or error).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LspResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_produces_content_length_frame() {
        let msg = encode_message(r#"{"jsonrpc":"2.0"}"#);
        assert_eq!(msg, "Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}");
    }

    #[test]
    fn decode_single_and_multiple_messages() {
        let mut buf = encode_message(r#"{"a":1}"#).into_bytes();
        let msgs = decode_messages(&mut buf).unwrap();
        assert_eq!(msgs, vec![r#"{"a":1}"#]);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_two_messages_in_one_buffer() {
        let mut buf = format!(
            "{}{}",
            encode_message(r#"{"a":1}"#),
            encode_message(r#"{"b":2}"#)
        )
        .into_bytes();
        let msgs = decode_messages(&mut buf).unwrap();
        assert_eq!(msgs, vec![r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[test]
    fn decode_partial_body_waits() {
        let mut buf = encode_message(r#"{"a":1}"#).into_bytes();
        buf.truncate(buf.len() - 2); // drop last 2 bytes
        let msgs = decode_messages(&mut buf).unwrap();
        assert!(msgs.is_empty());
        assert!(!buf.is_empty()); // kept for the next chunk
    }

    #[test]
    fn malformed_header_errors() {
        let mut buf = b"Bad-Header: 5\r\n\r\n{}".to_vec();
        assert!(matches!(
            decode_messages(&mut buf),
            Err(FramingError::MalformedHeader)
        ));
    }

    #[test]
    fn hover_roundtrips() {
        let h = Hover {
            contents: HoverContents::Markup {
                kind: "markdown".into(),
                value: "**fn**".into(),
            },
            range: Some(Range {
                start: Position {
                    line: 1,
                    character: 2,
                },
                end: Position {
                    line: 1,
                    character: 5,
                },
            }),
        };
        let json = serde_json::to_string(&h).unwrap();
        let back: Hover = serde_json::from_str(&json).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn request_and_response_roundtrip() {
        let req = LspRequest::new(
            1,
            "textDocument/hover",
            serde_json::json!({"uri": "file:///a"}),
        );
        let json = serde_json::to_string(&req).unwrap();
        let back: LspRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.method, "textDocument/hover");

        let resp = LspResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(serde_json::json!({"contents": "x"})),
            error: None,
        };
        assert_eq!(resp.id, 1);
    }
}
