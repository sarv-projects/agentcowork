//! P6.7 external-client E2E — the item previously `[NOT DONE — external binary]`.
//!
//! A REAL third-party MCP client — the official `@modelcontextprotocol/inspector`
//! CLI (modelcontextprotocol/inspector, Node >= 22.19) — connects to our
//! standalone server process over stdio (and loopback HTTP) and lists/calls
//! tools. This closes the external-client gap without a credential-gated
//! binary (Claude Code / Codex require auth; the Inspector CLI does not).
//!
//! The connection shape mirrors what real ACP clients do (Zed ↔ Codex "Client
//! MCP servers" over stdio): initialize handshake → tools/list → tools/call.
//!
//! Gated like the other live tests: `AGENTCOWORK_MCP_EXT_CLIENT=1` + node/npx on
//! PATH. The first run downloads the inspector package via npx (network).

use std::io::{BufRead, BufReader};
use std::process::{Command, Output, Stdio};

fn enabled() -> bool {
    if agentcowork_types::env_compat::get("MCP_EXT_CLIENT").as_deref() != Some("1") {
        eprintln!(
            "skipping: set AGENTCOWORK_MCP_EXT_CLIENT=1 to run (needs node/npx on PATH; first run downloads the inspector via npx)"
        );
        return false;
    }
    true
}

fn server_bin() -> String {
    std::env::var("CARGO_BIN_EXE_mcp-standalone-server")
        .expect("CARGO_BIN_EXE_mcp-standalone-server is unset — run via cargo test")
}

/// Run the official Inspector CLI once against the given server target.
fn inspector(server: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new("npx");
    cmd.arg("-y")
        .arg("@modelcontextprotocol/inspector")
        .arg("--cli")
        .arg(server)
        .args(args);
    let out = cmd
        .output()
        .expect("failed to run npx — is Node >= 22.19 with npx on PATH?");
    if !out.status.success() {
        eprintln!(
            "inspector CLI stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    out
}

fn assert_exit_zero(out: &Output) {
    assert!(
        out.status.success(),
        "inspector CLI exited {:?}",
        out.status.code()
    );
}

#[test]
#[ignore]
fn inspector_cli_lists_catalog_over_stdio() {
    if !enabled() {
        return;
    }
    let bin = server_bin();
    let out = inspector(&bin, &["--method", "tools/list", "--format", "json"]);
    assert_exit_zero(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("tools/list output not JSON: {e}\n{stdout}"));
    let tools = v["result"]["tools"].as_array().expect("result.tools array");
    // An external client is advertised the shared plane (task-shaped façades),
    // never the internal catalogue. Derive the expectation from the crate so a
    // façade change cannot leave this env-gated test asserting a stale magic
    // number.
    assert_eq!(
        tools.len(),
        agentcowork_mcp::SHARED_FACADES.len(),
        "façade count"
    );
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default())
        .collect();
    assert!(names.contains(&"browser.extract"), "browser façade present");
    assert!(names.contains(&"office.edit"), "office façade present");
    assert!(
        !names.contains(&"snapshot"),
        "internal primitive not advertised"
    );
    // Wire shape is camelCase (the MCP spec) — a real client keeps entries
    // only when `inputSchema` is present; annotations/ttlMs/etag are stripped
    // by the SDK's result validation (covered by crate unit tests).
    let first = &tools[0];
    assert!(first["name"].is_string(), "name kept");
    assert!(first["description"].is_string(), "description kept");
    assert!(first["inputSchema"].is_object(), "camelCase inputSchema");
}

#[test]
#[ignore]
fn inspector_cli_calls_shared_plane_facade_over_stdio() {
    if !enabled() {
        return;
    }
    let bin = server_bin();
    let out = inspector(
        &bin,
        &[
            "--method",
            "tools/call",
            "--tool-name",
            "browser.extract",
            "--format",
            "json",
        ],
    );
    assert_exit_zero(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("tools/call output not JSON: {e}\n{stdout}"));
    // The standalone harness echoes the tool name back in structuredContent.
    assert_eq!(
        v["result"]["structuredContent"]["tool"], "browser.extract",
        "echoed tool name"
    );
    assert_eq!(
        v["result"]["structuredContent"]["mode"],
        "standalone-test-harness"
    );
    assert!(
        !v["result"]["content"]
            .as_array()
            .is_none_or(|c| c.is_empty())
    );
}

#[test]
#[ignore]
fn inspector_cli_initialize_probe_over_stdio() {
    if !enabled() {
        return;
    }
    let bin = server_bin();
    let out = inspector(&bin, &["--method", "initialize", "--format", "json"]);
    assert_exit_zero(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("initialize output not JSON: {e}\n{stdout}"));
    assert_eq!(v["result"]["serverInfo"]["name"], "agentcowork-mcp");
    assert!(v["result"]["protocolVersion"].is_string());
    assert!(v["result"]["capabilities"]["tools"].is_object());
}

#[test]
#[ignore]
fn inspector_cli_lists_catalog_over_loopback_http() {
    if !enabled() {
        return;
    }
    let bin = server_bin();
    // Bind an OS-assigned loopback port; the server prints the bound port.
    let mut child = Command::new(&bin)
        .arg("--http")
        .arg("0")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp-standalone-server");
    let stderr = child.stderr.take().expect("stderr pipe");
    let port = BufReader::new(stderr)
        .lines()
        .find_map(|line| {
            let line = line.ok()?;
            let idx = line.find("127.0.0.1:")?;
            line[idx + "127.0.0.1:".len()..].parse::<u16>().ok()
        })
        .expect("server printed bound port");
    let url = format!("http://127.0.0.1:{port}");
    let out = inspector(
        &url,
        &[
            "--transport",
            "http",
            "--method",
            "tools/list",
            "--format",
            "json",
        ],
    );
    let _ = child.kill();
    let _ = child.wait();
    if !out.status.success() {
        // The one-shot loopback server is not a full streamable-HTTP endpoint
        // (no SSE GET leg / session reuse); keep this honest rather than
        // flaky: report the failure instead of asserting green.
        panic!(
            "inspector CLI over loopback HTTP failed (exit {:?}) — our server is a one-shot \
             JSON endpoint, not a streamable-HTTP endpoint; see stderr above",
            out.status.code()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("tools/list over HTTP not JSON: {e}\n{stdout}"));
    let tools = v["result"]["tools"].as_array().expect("result.tools array");
    assert_eq!(
        tools.len(),
        agentcowork_mcp::SHARED_FACADES.len(),
        "façade count over HTTP"
    );
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default())
        .collect();
    assert!(names.contains(&"browser.extract"));
}
