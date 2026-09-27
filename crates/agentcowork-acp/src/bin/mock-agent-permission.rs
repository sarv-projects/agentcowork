//! P10.1.8 — mock ACP agent that exercises the Guard-2 permission flow.
//!
//! Answers `initialize` / `session/new`, and on `session/prompt` issues a
//! `session/request_permission` request (a tool call the harness must decide),
//! waits for the decision, then reports the chosen option id in a
//! `session/update` notification and ends the turn. This is the fixture for
//! the "spawn agent via ACP → permission → audit → stop" harness-driving E2E
//! test (real CLIs remain credential/install gated).
//!
//! Usage: `mock-agent-permission [name]`

use std::io::{self, BufRead, Write};

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mock-agent-perm".into());
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut session_id: Option<String> = None;

    // Read through a *single* lock/iterator. The permission decision must be
    // consumed from the same `lines()` iterator (re-locking stdin on the same
    // thread does not share buffered data and can deadlock).
    let mut lines = stdin.lock().lines();
    while let Some(line) = lines.next() {
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
                session_id = Some(format!("sess-{name}"));
                reply["result"] =
                    serde_json::json!({ "sessionId": session_id.as_deref().unwrap_or("") });
            }
            "session/prompt" => {
                // Ask the harness for permission to write a file (Guard-2).
                let perm_req = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 9001,
                    "method": "session/request_permission",
                    "params": {
                        "sessionId": session_id.as_deref().unwrap_or(""),
                        "toolCall": {
                            "toolCallId": "tc-1",
                            "title": "write test file",
                            "kind": "write",
                            "content": [
                                { "type": "text", "text": "write /tmp/agentcowork-acp-e2e.txt" }
                            ],
                            "locations": []
                        },
                        "options": [
                            { "optionId": "allow", "kind": "allow_once", "label": "Allow" },
                            { "optionId": "deny", "kind": "reject_once", "label": "Deny" }
                        ]
                    }
                });
                let _ = writeln!(stdout, "{}", perm_req);
                let _ = stdout.flush();
                // Wait for the harness's decision on the same line iterator.
                let decision_line = match lines.next() {
                    Some(Ok(l)) => l,
                    _ => break,
                };
                let decided: serde_json::Value = match serde_json::from_str(decision_line.trim()) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let option_id = decided
                    .get("result")
                    .and_then(|r| r.get("outcome"))
                    .and_then(|o| o.get("optionId"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let notify = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id.as_deref().unwrap_or(""),
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": [{
                                "type": "text",
                                "text": format!("permission resolved: {option_id}")
                            }]
                        }
                    }
                });
                let _ = writeln!(stdout, "{}", notify);
                let _ = stdout.flush();
                reply["result"] = serde_json::json!({ "stopReason": "end_turn" });
            }
            "session/cancel" => {
                continue;
            }
            _ if id.is_some() => {
                reply["error"] = serde_json::json!({
                    "code": -32601,
                    "message": format!("method not found: {method}")
                });
            }
            _ => continue,
        }
        let _ = writeln!(stdout, "{}", serde_json::to_string(&reply).unwrap());
        let _ = stdout.flush();
    }
}
