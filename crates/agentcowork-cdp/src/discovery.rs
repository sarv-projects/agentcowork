//! Discovery — locate a running browser and its CDP endpoints (P2.1, E1).
//!
//! Chrome/Edge launched with `--remote-debugging-port=0` writes the chosen
//! port + browser WS path to `<user-data-dir>/DevToolsActivePort`
//! (ARCH/08 §8.1 — never trust a fixed port). The HTTP endpoints
//! `/json/version` and `/json/list` expose version + target info. All CDP
//! traffic is loopback-only (doc 33 §5.1 hard loopback guard).

use crate::{AttachMode, BrowserEndpoint, CdpClient, CdpError, TargetInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use url::Url;

/// Hard loopback guard (doc 33 §5.1): CDP endpoints must resolve to
/// 127.0.0.1, localhost, or [::1]. Never speak CDP to a remote host.
pub fn assert_loopback(host: &str) -> Result<(), CdpError> {
    let normalized = host.trim_start_matches('[').trim_end_matches(']');
    let lowered = normalized.to_ascii_lowercase();
    if matches!(lowered.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Ok(());
    }
    if lowered
        .parse::<std::net::IpAddr>()
        .map(|a| a.is_loopback())
        .unwrap_or(false)
    {
        return Ok(());
    }
    Err(CdpError::Security(format!(
        "non-loopback CDP host rejected: {host}"
    )))
}

/// Read the `DevToolsActivePort` file written by a Chrome/Edge instance
/// launched with `--remote-debugging-port=0`.
///
/// File format: line 1 = port, line 2 = browser WS handshake path.
pub fn read_devtools_active_port(user_data_dir: &Path) -> Result<BrowserEndpoint, CdpError> {
    let file = user_data_dir.join("DevToolsActivePort");
    let content = std::fs::read_to_string(&file)
        .map_err(|e| CdpError::Discovery(format!("read {}: {e}", file.display())))?;
    let mut lines = content.lines();
    let port: u16 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| {
            CdpError::Discovery(format!(
                "DevToolsActivePort: no port line in {}",
                file.display()
            ))
        })?;
    let ws_path = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("/devtools/browser/");
    if !ws_path.starts_with('/') {
        return Err(CdpError::Discovery(format!(
            "DevToolsActivePort: malformed ws path {ws_path:?}"
        )));
    }
    Ok(BrowserEndpoint {
        browser_ws_url: format!("ws://127.0.0.1:{port}{ws_path}"),
        version: String::new(),
    })
}

/// Probe `/json/version` on a local port → browser-level WS endpoint +
/// version string. Fails closed on any non-loopback URL.
pub fn probe_browser(port: u16) -> Result<BrowserEndpoint, CdpError> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let body = http_get(&url)?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| CdpError::Discovery(format!("{url}: invalid json: {e}")))?;
    let version = v
        .get("Browser")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ws = v
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| CdpError::Discovery(format!("{url}: no webSocketDebuggerUrl")))?;
    check_loopback_url(ws)?;
    Ok(BrowserEndpoint {
        browser_ws_url: ws.to_string(),
        version,
    })
}

/// Fetch the target list from `/json/list` (HTTP-side discovery; the CDP
/// `Target.getTargets` equivalent lives on `CdpClient`).
pub fn fetch_targets_http(port: u16) -> Result<Vec<TargetInfo>, CdpError> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let body = http_get(&url)?;
    serde_json::from_str(&body)
        .map_err(|e| CdpError::Discovery(format!("{url}: invalid json: {e}")))
}

/// Connect to a browser-level endpoint, negotiating the attach mode from the
/// browser's protocol version (CDP ≥ 1.3 → flattened sessions; older →
/// nested `Target.sendMessageToTarget`). Version skew tolerance: a missing /
/// unparseable version falls back to Flatten.
pub fn connect_to_browser(endpoint: &BrowserEndpoint) -> Result<CdpClient, CdpError> {
    let mode = protocol_version_of(endpoint)
        .map(|v| AttachMode::from_protocol_version(&v))
        .unwrap_or(AttachMode::Flatten);
    CdpClient::connect_with_mode(&endpoint.browser_ws_url, mode, crate::DEFAULT_CALL_TIMEOUT)
}

/// Read `Protocol-Version` from `/json/version` for the endpoint's port.
fn protocol_version_of(endpoint: &BrowserEndpoint) -> Option<String> {
    let port = Url::parse(&endpoint.browser_ws_url).ok()?.port()?;
    let body = http_get(&format!("http://127.0.0.1:{port}/json/version")).ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    v.get("Protocol-Version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Reject any ws URL whose host is not loopback.
fn check_loopback_url(ws_url: &str) -> Result<(), CdpError> {
    let url = Url::parse(ws_url).map_err(|e| CdpError::Discovery(format!("bad ws url: {e}")))?;
    let host = url.host_str().unwrap_or_default();
    assert_loopback(host)
}

/// Discovery-probe HTTP timeout — a dead-but-open port must not hang
/// discovery forever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Small GET helper over ureq with a hard timeout (dead ports must not hang
/// discovery).
pub(crate) fn http_get(url: &str) -> Result<String, CdpError> {
    let resp = ureq::get(url)
        .timeout(PROBE_TIMEOUT)
        .call()
        .map_err(|e| CdpError::Http(format!("GET {url}: {e}")))?;
    resp.into_string()
        .map_err(|e| CdpError::Http(format!("GET {url}: {e}")))
}

// ---------------------------------------------------------------------------
// Electron app discovery (E15 — doc 63 §4.1, agent-browser pattern)
// ---------------------------------------------------------------------------

/// A running Electron app reachable over CDP (VS Code / Slack / Discord /
/// Spotify / Notion …). Electron apps launch with `--remote-debugging-port`;
/// they answer `/json/version` with a `Browser` field starting `Electron/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElectronApp {
    pub port: u16,
    /// The `Browser` string, e.g. `Electron/31.0.0`.
    pub version: String,
    pub browser_ws_url: String,
    pub targets: Vec<TargetInfo>,
}

/// Is a `/json/version` `Browser` string an Electron app (vs Chrome/Edge)?
/// Electron reports `Electron/x.y.z`; the Chromium forks report `Chrome/x`.
pub fn is_electron_version(browser: &str) -> bool {
    browser.starts_with("Electron")
}

/// Parse an Electron app from `/json/version` + `/json/list` bodies (pure —
/// testable without a live port). Fails on a non-Electron `Browser` field.
pub fn electron_from_json(
    port: u16,
    version_body: &str,
    targets_body: &str,
) -> Result<ElectronApp, CdpError> {
    let v: Value = serde_json::from_str(version_body)
        .map_err(|e| CdpError::Discovery(format!("/json/version: invalid json: {e}")))?;
    let browser = v
        .get("Browser")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_electron_version(&browser) {
        return Err(CdpError::Discovery(format!(
            "port {port}: not an Electron app (Browser={browser:?})"
        )));
    }
    let ws = v
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| CdpError::Discovery(format!("port {port}: no webSocketDebuggerUrl")))?;
    check_loopback_url(ws)?;
    let targets: Vec<TargetInfo> = serde_json::from_str(targets_body)
        .map_err(|e| CdpError::Discovery(format!("/json/list: invalid json: {e}")))?;
    Ok(ElectronApp {
        port,
        version: browser,
        browser_ws_url: ws.to_string(),
        targets,
    })
}

/// Probe one local port for a running Electron app.
pub fn probe_electron(port: u16) -> Result<ElectronApp, CdpError> {
    let version_body = http_get(&format!("http://127.0.0.1:{port}/json/version"))?;
    let targets_body = http_get(&format!("http://127.0.0.1:{port}/json/list"))?;
    electron_from_json(port, &version_body, &targets_body)
}

/// Scan a list of candidate ports for Electron apps (best-effort — dead or
/// non-Electron ports are skipped).
pub fn discover_electron_apps(ports: &[u16]) -> Vec<ElectronApp> {
    ports
        .iter()
        .filter_map(|&p| probe_electron(p).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn electron_version_detection() {
        assert!(is_electron_version("Electron/31.0.0"));
        assert!(!is_electron_version("Chrome/120"));
        assert!(!is_electron_version("Edge/120"));
        assert!(!is_electron_version(""));
    }

    #[test]
    fn electron_from_json_parses_and_rejects_non_electron() {
        let version = r#"{"Browser":"Electron/31.0.0","webSocketDebuggerUrl":"ws://127.0.0.1:9229/devtools/browser/x"}"#;
        let list = r#"[{"id":"t1","type":"page","title":"VS Code","url":"vscode://main","webSocketDebuggerUrl":"ws://127.0.0.1:9229/devtools/page/t1"}]"#;
        let app = electron_from_json(9229, version, list).unwrap();
        assert_eq!(app.version, "Electron/31.0.0");
        assert_eq!(app.port, 9229);
        assert_eq!(app.targets.len(), 1);
        assert_eq!(app.targets[0].target_type, crate::TargetType::Page);

        // A Chrome version string must be rejected as not-Electron.
        let chrome = r#"{"Browser":"Chrome/120","webSocketDebuggerUrl":"ws://127.0.0.1:9229/x"}"#;
        let err = electron_from_json(9229, chrome, list).unwrap_err();
        assert!(matches!(err, CdpError::Discovery(_)), "{err:?}");
    }

    #[test]
    fn electron_from_json_rejects_remote_ws_url() {
        let version =
            r#"{"Browser":"Electron/31.0.0","webSocketDebuggerUrl":"ws://evil.example:9229/x"}"#;
        let err = electron_from_json(9229, version, "[]").unwrap_err();
        assert!(matches!(err, CdpError::Security(_)), "{err:?}");
    }

    #[test]
    fn loopback_guard_accepts_localhost_only() {
        assert!(assert_loopback("127.0.0.1").is_ok());
        assert!(assert_loopback("localhost").is_ok());
        assert!(assert_loopback("[::1]").is_ok());
        assert!(assert_loopback("::1").is_ok());
        assert!(assert_loopback("example.com").is_err());
        assert!(assert_loopback("192.168.1.5").is_err());
        assert!(assert_loopback("10.0.0.1").is_err());
    }

    #[test]
    fn active_port_file_parses() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("DevToolsActivePort"),
            "43210\n/devtools/browser/abc123\n",
        )
        .unwrap();
        let ep = read_devtools_active_port(&dir).unwrap();
        assert_eq!(
            ep.browser_ws_url,
            "ws://127.0.0.1:43210/devtools/browser/abc123"
        );
    }

    #[test]
    fn active_port_missing_file_errors() {
        let dir = tempfile_dir();
        let err = read_devtools_active_port(&dir).unwrap_err();
        assert!(matches!(err, CdpError::Discovery(_)), "got {err:?}");
    }

    #[test]
    fn active_port_missing_path_defaults_to_browser() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("DevToolsActivePort"), "43211\n").unwrap();
        let ep = read_devtools_active_port(&dir).unwrap();
        assert_eq!(ep.browser_ws_url, "ws://127.0.0.1:43211/devtools/browser/");
    }

    #[test]
    fn probe_version_and_targets_http() {
        let (port, _keep) = mock_browser_server(Some("1.4"));
        let ep = probe_browser(port).unwrap();
        assert_eq!(ep.version, "Chrome/120");
        assert_eq!(
            ep.browser_ws_url,
            format!("ws://127.0.0.1:{port}/devtools/browser/xyz")
        );
        let targets = fetch_targets_http(port).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target_type, crate::TargetType::Page);
    }

    #[test]
    fn probe_rejects_non_loopback_ws_url() {
        // A version endpoint advertising a remote WS URL must fail closed.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 4096];
                let mut req = String::new();
                let mut header_end = None;
                loop {
                    let n = match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if let Some(pos) = req.find("\r\n\r\n") {
                        header_end = Some(pos);
                        break;
                    }
                }
                let _ = header_end;
                let body =
                    r#"{"Browser":"Chrome/120","webSocketDebuggerUrl":"ws://evil.example:9222/x"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        let err = probe_browser(port).unwrap_err();
        assert!(matches!(err, CdpError::Security(_)), "got {err:?}");
    }

    #[test]
    fn connect_negotiates_flatten_for_modern_protocol() {
        let (port, _keep) = mock_browser_server(Some("1.4"));
        let ep = BrowserEndpoint {
            browser_ws_url: format!("ws://127.0.0.1:{port}/devtools/browser/xyz"),
            version: "Chrome/120".into(),
        };
        let client = connect_to_browser(&ep).unwrap();
        assert_eq!(client.attach_mode(), AttachMode::Flatten);
    }

    #[test]
    fn connect_negotiates_nested_for_old_protocol() {
        let (port, _keep) = mock_browser_server(Some("1.2"));
        let ep = BrowserEndpoint {
            browser_ws_url: format!("ws://127.0.0.1:{port}/devtools/browser/xyz"),
            version: "Chrome/90".into(),
        };
        let client = connect_to_browser(&ep).unwrap();
        assert_eq!(client.attach_mode(), AttachMode::Nested);
    }

    #[test]
    fn connect_falls_back_to_flatten_without_protocol_version() {
        let (port, _keep) = mock_browser_server(None);
        let ep = BrowserEndpoint {
            browser_ws_url: format!("ws://127.0.0.1:{port}/devtools/browser/xyz"),
            version: String::new(),
        };
        let client = connect_to_browser(&ep).unwrap();
        assert_eq!(client.attach_mode(), AttachMode::Flatten);
    }

    // -- helpers -------------------------------------------------------------

    fn tempfile_dir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("agentcowork-cdp-test-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Compute the RFC 6455 Sec-WebSocket-Accept for a client key.
    fn ws_accept(key: &str) -> String {
        use base64::Engine;
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(key.as_bytes());
        hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        let digest = hasher.finalize();
        base64::engine::general_purpose::STANDARD.encode(digest)
    }

    /// A mock browser: `/json/version` (with a given Protocol-Version),
    /// `/json/list`, and real WS handshakes. The port is captured before the
    /// thread starts, so the version body can embed the real port.
    fn mock_browser_server(
        protocol_version: Option<&'static str>,
    ) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut req = String::new();
                let mut buf = [0u8; 8192];
                let mut header_end = None;
                loop {
                    let n = match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if let Some(pos) = req.find("\r\n\r\n") {
                        header_end = Some(pos);
                        break;
                    }
                }
                let Some(pos) = header_end else { continue };
                if req[..pos].contains("Upgrade: websocket") {
                    let key = req[..pos]
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("Sec-WebSocket-Key:")
                                .map(|v| v.trim().to_string())
                        })
                        .unwrap_or_default();
                    let accept = ws_accept(&key);
                    let response = format!(
                        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                    );
                    let _ = s.write_all(response.as_bytes());
                    let _ = s.flush();
                    let mut _drain = [0u8; 4096];
                    while let Ok(n) = s.read(&mut _drain) {
                        if n == 0 {
                            break;
                        }
                    }
                    continue;
                }
                let first = req.lines().next().unwrap_or_default().to_string();
                let (code, body) = if first.contains("/json/version") {
                    let pv = protocol_version
                        .map(|pv| format!(",\"Protocol-Version\":\"{pv}\""))
                        .unwrap_or_default();
                    (
                        200,
                        format!(
                            r#"{{"Browser":"Chrome/120"{pv},"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/xyz"}}"#
                        ),
                    )
                } else if first.contains("/json/list") {
                    (
                        200,
                        r#"[{"id":"t1","type":"page","title":"Example","url":"https://example.com","webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/page/t1"}]"#
                            .to_string(),
                    )
                } else {
                    (404, "not found".to_string())
                };
                let reason = if code == 200 { "OK" } else { "Error" };
                let resp = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        (port, handle)
    }
}
