//! Mock LSP server (P7.1 test fixture).
//!
//! A minimal language server over the real LSP wire (Content-Length framed
//! JSON-RPC over stdio): answers `initialize` with empty capabilities,
//! acknowledges `initialized`, and after `textDocument/didOpen` emits one
//! canned `textDocument/publishDiagnostics` batch — so the live
//! `LspRunner::collect` flow (spawn → initialize → didOpen → diagnostics)
//! is exercised against a *real process*, not an in-process mock.
//!
//! Usage: `mock-lsp-server [diagnostic-message]` — the optional argument
//! sets the message text of the canned diagnostic (default "mock diagnostic").

use agentcowork_codeintel::lsp::{decode_messages, encode_message};
use std::io::{self, Read, Write};

fn main() {
    let message = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mock diagnostic".into());
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buf: Vec<u8> = Vec::new();

    // Read exactly one byte at a time from a locked stdin reader so the
    // framing loop below sees every byte (a buffered reader would swallow
    // past the frame boundary).
    let mut reader = stdin.lock();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) | Err(_) => break, // EOF
            Ok(_) => buf.push(byte[0]),
        }
        let msgs = match decode_messages(&mut buf) {
            Ok(m) => m,
            Err(_) => continue, // malformed frame — skip
        };
        for raw in msgs {
            let v: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let method = v
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let id = v.get("id").cloned();

            match method {
                "initialize" => {
                    let reply = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "capabilities": {} }
                    });
                    write_frame(&mut stdout, &reply);
                }
                "initialized" => {
                    // Notification — nothing to reply.
                }
                "textDocument/didOpen" => {
                    let uri = v
                        .pointer("/params/textDocument/uri")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("file:///unknown");
                    // Emit the canned diagnostics batch for the opened file.
                    let notify = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": uri,
                            "diagnostics": [
                                {
                                    "range": {
                                        "start": { "line": 0, "character": 0 },
                                        "end": { "line": 0, "character": 4 }
                                    },
                                    "severity": 1,
                                    "message": message,
                                    "source": "mock-lsp"
                                }
                            ]
                        }
                    });
                    write_frame(&mut stdout, &notify);
                }
                "shutdown" => {
                    let reply = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": null
                    });
                    write_frame(&mut stdout, &reply);
                }
                "exit" => return,
                _ => {
                    // Unknown request with an id → method-not-found; unknown
                    // notification → ignore.
                    if let Some(rid) = id {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rid,
                            "error": { "code": -32601, "message": "method not found" }
                        });
                        write_frame(&mut stdout, &reply);
                    }
                }
            }
        }
    }
}

fn write_frame(stdout: &mut io::Stdout, value: &serde_json::Value) {
    let _ = stdout.write_all(encode_message(&value.to_string()).as_bytes());
    let _ = stdout.flush();
}
