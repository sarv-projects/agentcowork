//! Mock ACP agent — a *real spawned process* that speaks the ACP v1 protocol
//! (newline-delimited JSON-RPC over stdio) so live E2E tests can exercise the
//! full spawn → initialize → session/new path without any credentials.
//!
//! Behavior (mirrors the official `@agentclientprotocol/claude-agent-acp`
//! surface for the handshake subset):
//! - `initialize` → `{ protocolVersion: 1, agentCapabilities, agentInfo,
//!   authMethods: [] }`
//! - `session/new` → `{ sessionId: "mock-session-<n>" }`
//! - `session/prompt` → a `session/update` notification with the echo text
//!   (so prompt-driven E2E is possible too)
//! - anything else → JSON-RPC method-not-found error
//!
//! Reads until EOF (the client's shutdown kills us), so a hanging test can
//! never leak the process — stdin EOF ends the loop.

use std::io::{self, BufRead, Write};
use std::process::exit;

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut session_counter = 0u64;
    let mut session_id: Option<String> = None;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<serde_json::Value>(line) else {
            let _ = writeln!(
                stdout,
                "{{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{{\"code\":-32700,\"message\":\"parse error\"}}}}"
            );
            let _ = stdout.flush();
            continue;
        };
        let method = req["method"].as_str().unwrap_or("");
        let id = req["id"].clone();

        match method {
            "initialize" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": 1,
                        "agentCapabilities": {
                            "loadSession": true,
                            "mcpCapabilities": { "http": true, "sse": false }
                        },
                        "agentInfo": { "name": "mock-acp", "title": "Mock ACP Agent", "version": "0.0.1" },
                        "authMethods": []
                    }
                });
                let _ = writeln!(stdout, "{resp}");
            }
            "session/new" => {
                session_counter += 1;
                let created = format!("mock-session-{session_counter}");
                session_id = Some(created.clone());
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "sessionId": created }
                });
                let _ = writeln!(stdout, "{resp}");
            }
            "session/load" => {
                let requested = req["params"]["sessionId"].as_str().unwrap_or("");
                let update = serde_json::json!({
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
                let _ = writeln!(stdout, "{update}");
                let _ = stdout.flush();
                // ACP v1 permits an empty/options-only load result. The
                // client must retain the requested provider id.
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                });
                let _ = writeln!(stdout, "{resp}");
            }
            "session/prompt" => {
                // ACP v1 shape: `params.prompt` is a content-block array; fall
                // back to the legacy flat `params.text` for tolerance.
                let text = req["params"]["prompt"]
                    .as_array()
                    .and_then(|blocks| blocks.iter().find_map(|b| b["text"].as_str()))
                    .or_else(|| req["params"]["text"].as_str())
                    .unwrap_or("");
                // Official ACP v1 notification envelope: `params.update`
                // carries the update discriminator and content.
                let notif = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id.as_deref().unwrap_or(""),
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": [{ "type": "text", "text": format!("echo: {text}") }]
                        }
                    }
                });
                let _ = writeln!(stdout, "{notif}");
                // P71.4 — the prompt result carries the agent's **own** usage
                // report. This is the observation the ledger records with
                // `UsageSource::AgentReport`; a prompt naming `no-usage` (see
                // below) omits the block so the unreported path is drivable
                // too — absence must not read as a measured zero.
                let result = if text.contains("no-usage") {
                    serde_json::json!({ "stopReason": "end_turn" })
                } else {
                    serde_json::json!({
                        "stopReason": "end_turn",
                        "usage": {
                            "inputTokens": 1200,
                            "outputTokens": 180,
                            "cachedReadTokens": 900,
                            "cachedWriteTokens": 40,
                            "costUsd": 0.0125
                        }
                    })
                };
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                });
                let _ = writeln!(stdout, "{resp}");
            }
            _ => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("method not found: {method}") }
                });
                let _ = writeln!(stdout, "{resp}");
            }
        }
        let _ = stdout.flush();
    }
    exit(0);
}
