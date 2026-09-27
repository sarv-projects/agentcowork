//! S0.6 / E13 — Chrome session-attachment pairing.
//!
//! Chrome 136+ ignores `--remote-debugging-port` on the *default* profile
//! (developer.chrome.com/blog/remote-debugging-port). AgentCowork therefore:
//!
//! * launches its own isolated profile (`<data_dir>/browser-profile`)
//! * refuses to treat a default-profile CDP endpoint as attachable
//! * requires a one-time pairing for any user-launched *non-default* profile
//! * never falls back to raw-cookie extraction to "make attach work"

use crate::CdpError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One user-launched profile the human has explicitly paired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePairing {
    pub id: String,
    pub user_data_dir: String,
    #[serde(default)]
    pub pid: Option<u32>,
    pub created_ms: u64,
}

/// In-memory pairing registry (persisted by the caller if desired).
#[derive(Debug, Default, Clone)]
pub struct ProfilePairingStore {
    by_id: BTreeMap<String, ProfilePairing>,
}

impl ProfilePairingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<&ProfilePairing> {
        self.by_id.get(id)
    }

    pub fn is_paired_dir(&self, user_data_dir: &Path) -> bool {
        let want = canonicalize_dir(user_data_dir);
        self.by_id
            .values()
            .any(|p| canonicalize_dir(Path::new(&p.user_data_dir)) == want)
    }

    /// Pair a user-launched non-default profile. Default Chrome/Chromium
    /// dirs are refused (Chrome 136+ CDP on those dirs is inert and a
    /// cookie-jar attack surface).
    pub fn pair(
        &mut self,
        user_data_dir: &Path,
        pid: Option<u32>,
    ) -> Result<ProfilePairing, CdpError> {
        if is_default_chrome_profile(user_data_dir) {
            return Err(CdpError::Security(
                "refusing to pair Chrome/Chromium default profile (Chrome 136+ CDP is not attachable; use an isolated profile)".into(),
            ));
        }
        let id = format!(
            "pair-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let p = ProfilePairing {
            id: id.clone(),
            user_data_dir: user_data_dir.to_string_lossy().to_string(),
            pid,
            created_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        };
        self.by_id.insert(id, p.clone());
        Ok(p)
    }
}

/// Chrome/Chromium default user-data directories on this OS.
pub fn chrome_default_user_data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let Some(home) = home else {
            return out;
        };
        #[cfg(target_os = "linux")]
        {
            out.push(home.join(".config/google-chrome"));
            out.push(home.join(".config/chromium"));
            out.push(home.join(".config/google-chrome-beta"));
            out.push(home.join(".config/google-chrome-unstable"));
        }
        #[cfg(target_os = "macos")]
        {
            let app = home.join("Library/Application Support");
            out.push(app.join("Google/Chrome"));
            out.push(app.join("Chromium"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            out.push(local.join("Google/Chrome/User Data"));
            out.push(local.join("Chromium/User Data"));
        }
    }
    out
}

pub fn is_default_chrome_profile(dir: &Path) -> bool {
    let want = canonicalize_dir(dir);
    chrome_default_user_data_dirs()
        .iter()
        .any(|d| canonicalize_dir(d) == want)
}

/// Isolated AgentCowork-owned profile (`<data_dir>/browser-profile` under
/// either the current or the retired data-home spelling — both eras count as
/// ours, so pre-rename profiles keep pairing without a prompt).
pub fn is_agentcowork_isolated_profile(dir: &Path) -> bool {
    let s = canonicalize_dir(dir);
    s.ends_with("browser-profile")
        && (s.contains(".agentcowork")
            || s.contains(".everyaios")
            || s.contains("AGENTCOWORK")
            || s.contains("EVERYAIOS")
            || agentcowork_types::env_compat::get_os("HOME")
                .is_some_and(|h| !h.is_empty() && s.contains(&h.to_string_lossy().into_owned())))
}

/// Parse a Chrome `Browser` version string (`Chrome/136.0.7103.92`) → major.
pub fn chrome_major_version(browser: &str) -> Option<u32> {
    let after = browser.split('/').nth(1).unwrap_or(browser);
    after.split('.').next()?.parse().ok()
}

/// Gate for attaching to a running Chrome via CDP.
///
/// * AgentCowork isolated profile → allowed (no pairing).
/// * Default profile + Chrome ≥ 136 → refused.
/// * Default profile (any version) → refused (never a cookie-extraction fallback).
/// * Other user-launched profile → requires `paired`.
pub fn assert_attach_allowed(
    browser_version: &str,
    user_data_dir: Option<&Path>,
    paired: bool,
) -> Result<(), CdpError> {
    let Some(dir) = user_data_dir else {
        return Err(CdpError::Security(
            "refusing CDP attach without a known user-data-dir (pairing required; raw-cookie fallback is disabled)".into(),
        ));
    };
    if is_agentcowork_isolated_profile(dir) {
        return Ok(());
    }
    if is_default_chrome_profile(dir) {
        let major = chrome_major_version(browser_version).unwrap_or(136);
        return Err(CdpError::Security(format!(
            "refusing Chrome {major}+ default-profile CDP attach (E13); launch an isolated AgentCowork profile or pair a non-default user-data-dir"
        )));
    }
    if !paired {
        return Err(CdpError::Security(
            "user-launched profile requires a one-time pairing before CDP attach".into(),
        ));
    }
    Ok(())
}

fn canonicalize_dir(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_detected() {
        let dirs = chrome_default_user_data_dirs();
        assert!(
            !dirs.is_empty()
                || std::env::var_os("HOME").is_none() && std::env::var_os("USERPROFILE").is_none()
        );
        if let Some(d) = dirs.first() {
            assert!(is_default_chrome_profile(d));
        }
    }

    #[test]
    fn isolated_agentcowork_profile_is_not_default() {
        // Both eras count as ours: the current `~/.agentcowork` and the
        // retired `~/.everyaios` a pre-rename install still uses.
        for dir in [
            PathBuf::from("/home/user/.agentcowork/browser-profile"),
            PathBuf::from("/home/user/.everyaios/browser-profile"),
        ] {
            assert!(!is_default_chrome_profile(&dir));
            assert!(is_agentcowork_isolated_profile(&dir));
        }
    }

    #[test]
    fn chrome_major_parses() {
        assert_eq!(chrome_major_version("Chrome/136.0.7103.92"), Some(136));
        assert_eq!(chrome_major_version("Chrome/141.0.0.0"), Some(141));
    }

    #[test]
    fn default_profile_cdp_refused() {
        let dirs = chrome_default_user_data_dirs();
        let Some(d) = dirs.first() else { return };
        let err = assert_attach_allowed("Chrome/136.0.0.0", Some(d), true).unwrap_err();
        assert!(matches!(err, CdpError::Security(_)), "{err:?}");
    }

    #[test]
    fn isolated_profile_allowed_without_pairing() {
        for dir in [
            PathBuf::from("/tmp/.agentcowork/browser-profile"),
            PathBuf::from("/tmp/.everyaios/browser-profile"),
        ] {
            assert!(assert_attach_allowed("Chrome/141.0.0.0", Some(&dir), false).is_ok());
        }
    }

    #[test]
    fn user_profile_requires_pairing() {
        let dir = PathBuf::from("/tmp/my-custom-chrome-profile");
        let err = assert_attach_allowed("Chrome/141.0.0.0", Some(&dir), false).unwrap_err();
        assert!(matches!(err, CdpError::Security(_)));
        assert!(assert_attach_allowed("Chrome/141.0.0.0", Some(&dir), true).is_ok());
    }

    #[test]
    fn missing_dir_refused_no_cookie_fallback() {
        let err = assert_attach_allowed("Chrome/141.0.0.0", None, false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("raw-cookie") || msg.contains("user-data-dir"),
            "{msg}"
        );
    }

    #[test]
    fn pair_refuses_default_profile() {
        let mut store = ProfilePairingStore::new();
        let dirs = chrome_default_user_data_dirs();
        let Some(d) = dirs.first() else { return };
        assert!(store.pair(d, None).is_err());
    }

    #[test]
    fn pair_accepts_custom_profile() {
        let mut store = ProfilePairingStore::new();
        let dir = PathBuf::from("/tmp/agentcowork-paired-profile");
        let p = store.pair(&dir, Some(42)).unwrap();
        assert!(store.is_paired_dir(&dir));
        assert_eq!(store.get(&p.id).unwrap().pid, Some(42));
    }
}
