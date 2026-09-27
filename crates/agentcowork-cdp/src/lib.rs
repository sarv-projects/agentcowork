//! agentcowork-cdp — Chrome DevTools Protocol client (ARCH/08, E1).
//!
//! P2.1 scope: tokio-tungstenite WebSocket transport (`transport`),
//! discovery (`--remote-debugging-port=0` + DevToolsActivePort, `discovery`),
//! system-Chrome/Edge launch + chrome-for-testing fallback (`browser`),
//! loopback-only host restriction and protocol-version tolerance
//! (flatten/nested session modes).
//!
//! E15 (doc 63 §2.1 — agent-browser pattern): `discovery` also carries the
//! Electron discovery loop — `probe_electron(port)`/`discover_electron_apps`
//! (probe localhost debug ports for `Browser: Electron/…` version strings,
//! loopback-guarded), `electron_from_json`/`is_electron_version` pure
//! helpers — so the browser layer can attach to VS Code/Slack/Spotify/etc.

pub mod browser;
pub mod discovery;
pub mod fingerprint;
pub mod pairing;
pub mod transport;

pub use browser::{
    BrowserCandidate, BrowserChannel, BrowserChild, BrowserConfig, BrowserProfileMode,
    CandidateSource, LaunchOptions, channel_candidates, default_profile_dir,
    detect_channel_from_path, discover_installed_browsers, install_chrome_for_testing,
    locate_system_browser, probe_browser_version, resolve_browser_binary, spawn_browser,
};
pub use discovery::{
    ElectronApp, assert_loopback, connect_to_browser, discover_electron_apps, electron_from_json,
    fetch_targets_http, is_electron_version, probe_browser, probe_electron,
    read_devtools_active_port,
};
pub use fingerprint::{FingerprintProfile, RotationSet, defaults as default_fingerprints};
pub use pairing::{
    ProfilePairing, ProfilePairingStore, assert_attach_allowed, chrome_default_user_data_dirs,
    chrome_major_version, is_agentcowork_isolated_profile, is_default_chrome_profile,
};
pub use transport::{AttachMode, CdpClient, CdpEvent, DEFAULT_CALL_TIMEOUT};

use serde::{Deserialize, Serialize};

/// A browser target (tab, page, or worker) as reported by
/// `Target.getTargets` / `/json/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TargetInfo {
    /// Wire field differs by surface: `Target.getTargets` uses `targetId`;
    /// the HTTP `/json/list` endpoint uses `id`. Accept both on read.
    #[serde(rename = "id", alias = "targetId")]
    pub target_id: String,
    /// CDP wire field is `type` (e.g. `page`, `worker`).
    #[serde(rename = "type")]
    pub target_type: TargetType,
    pub title: String,
    pub url: String,
    /// WebSocket debugger URL for this target — serialized with the exact
    /// CDP wire name `webSocketDebuggerUrl`. Absent on some target types
    /// (workers/background pages); always present on pages.
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub ws_url: String,
    /// Frame id (present on iframe/frame targets) — used by the snapshot
    /// engine to stitch child frames inline.
    #[serde(rename = "frameId", default)]
    pub frame_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    Page,
    Tab,
    Iframe,
    Worker,
    /// Any unknown target type (service_worker, background_page, …) — real
    /// Chrome reports more types than the enum models; unknown ones must not
    /// break target listing (version-skew tolerance).
    #[serde(other)]
    Other,
}

/// A session attached to one target (CDP `Target.attachToTarget`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub target_id: String,
}

/// Transport errors.
#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error("discovery failed: {0}")]
    Discovery(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("protocol error: code {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("security: {0}")]
    Security(String),
    #[error("browser not found: {0}")]
    BrowserNotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(String),
}

/// Discovery result of a running Chrome/Edge instance.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserEndpoint {
    /// WS URL of the browser-level endpoint (e.g.
    /// `ws://127.0.0.1:9222/devtools/browser/...`).
    pub browser_ws_url: String,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_info_serializes_like_cdp() {
        let t = TargetInfo {
            target_id: "ABC123".into(),
            target_type: TargetType::Page,
            title: "Example".into(),
            url: "https://example.com".into(),
            ws_url: "ws://127.0.0.1:9222/devtools/page/ABC123".into(),
            frame_id: None,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"type\":\"page\""), "{json}");
        assert!(json.contains("\"webSocketDebuggerUrl\""), "{json}"); // alias handled at P2.1
    }

    #[test]
    fn target_type_roundtrip() {
        for (s, t) in [("page", TargetType::Page), ("worker", TargetType::Worker)] {
            let parsed: TargetType = serde_json::from_str(&format!("\"{s}\"")).unwrap();
            assert_eq!(parsed, t);
        }
    }

    #[test]
    fn error_variants_display() {
        assert!(
            CdpError::BrowserNotFound("none".into())
                .to_string()
                .contains("browser not found")
        );
        assert!(
            CdpError::Timeout("x".into())
                .to_string()
                .contains("timed out")
        );
    }

    #[test]
    fn target_info_accepts_frame_id() {
        let t = TargetInfo {
            target_id: "if1".into(),
            target_type: TargetType::Iframe,
            title: "frame".into(),
            url: "https://example.com/f".into(),
            ws_url: "ws://127.0.0.1:9222/devtools/page/if1".into(),
            frame_id: Some("FRAME-1".into()),
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"frameId\":\"FRAME-1\""), "{json}");
    }
}
