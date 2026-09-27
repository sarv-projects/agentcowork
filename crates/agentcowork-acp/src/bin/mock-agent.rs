//! Mock ACP agent (P6.8 two-agent side-by-side test fixture).
//!
//! A minimal ACP v1 agent over newline-delimited stdio: answers
//! `initialize`, `session/new`, and `session/prompt` with canned results, and
//! honors `session/cancel` by ending the current prompt. Used by the
//! side-by-side `ProcessTransport` test so the harness is proven against two
//! *real processes* (real CLIs remain credential/install gated).
//!
//! Usage: `mock-agent [name]` — the optional name is echoed as the agent
//! title so the test can tell the two instances apart.

use std::io::{self, BufRead, Write};

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mock-agent".into());
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut session_id: Option<String> = None;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let id = v.get("id").cloned();
        let mut reply = serde_json::json!({ "jsonrpc": "2.0" });
        if let Some(id) = id.clone() {
            reply["id"] = id;
        }
        match method {
            "initialize" => {
                reply["result"] = serde_json::json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true,
                        "mcpCapabilities": { "http": true, "sse": false }
                    },
                    "agentInfo": { "name": name.clone(), "title": name.clone(), "version": "0.0.1" },
                    "authMethods": []
                });
            }
            "session/new" => {
                session_id = Some(format!("sess-{}", name));
                reply["result"] =
                    serde_json::json!({ "sessionId": session_id.as_deref().unwrap_or("") });
            }
            "session/prompt" => {
                // Simulate one turn: emit a session/update notification, then
                // the prompt result (stop_reason = end_turn).
                let notify = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id.as_deref().unwrap_or(""),
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": [{ "type": "text", "text": "done" }]
                        }
                    }
                });
                let _ = writeln!(stdout, "{}", notify);
                let _ = stdout.flush();
                reply["result"] = serde_json::json!({ "stopReason": "end_turn" });
            }
            "session/cancel" => {
                // Notify cancel accepted; no response expected (notification).
                continue;
            }
            "session/load" => {
                let requested = v
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                session_id = Some(requested.to_string());
                let notify = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": requested,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": [{ "type": "text", "text": "loaded" }]
                        }
                    }
                });
                let _ = writeln!(stdout, "{}", notify);
                let _ = stdout.flush();
                // ACP v1 permits an empty load result; retain the requested id.
                reply["result"] = serde_json::json!({});
            }
            _ if id.is_some() => {
                reply["error"] = serde_json::json!({ "code": -32601, "message": format!("method not found: {method}") });
            }
            _ => continue,
        }
        let _ = writeln!(stdout, "{}", serde_json::to_string(&reply).unwrap());
        let _ = stdout.flush();
    }
}
