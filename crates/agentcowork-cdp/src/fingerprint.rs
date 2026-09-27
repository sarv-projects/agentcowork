//! E14 fingerprint profile (doc 65 §2 — Scrapling Camoufox steal): a
//! coherent browser fingerprint — UA, platform, WebGL vendor, canvas-noise
//! budget — that the browser layer can spoof for behavioral realism when a
//! target site gates on fingerprint (anti-bot). [`FingerprintProfile`] is
//! one coherent identity; [`RotationSet`] holds a handful so the caller can
//! rotate across sessions.
//!
//! Every field is data — no stealth is attempted here. The CDP layer
//! applies it via `Emulation.setUserAgentOverride` +
//! `Page.addScriptToEvaluateOnNewDocument` (canvas noise injected through
//! the script sandbox, which is exactly where the guard's script rules
//! apply).

use serde::{Deserialize, Serialize};

/// One coherent browser identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintProfile {
    /// Full `User-Agent` string.
    pub ua: String,
    /// `platform` reported to `navigator.platform` (e.g. `Win32`, `MacIntel`).
    pub platform: String,
    /// WebGL renderer string (e.g. `ANGLE (NVIDIA, NVIDIA GeForce RTX 3070...)`).
    pub webgl_vendor: String,
    /// Whether to inject deterministic canvas-readback noise (`toDataURL`
    /// returns a stable-but-distinct pixel hash).
    pub canvas_noise: bool,
    /// Hardware-concurrency + device-memory spoof, `(cores, gb)`.
    pub hardware: (u8, u8),
    /// `navigator.languages` reported.
    pub languages: Vec<String>,
}

/// A small rotation set — pick a profile per session so a long crawl doesn't
/// present as one frozen identity across contexts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationSet {
    pub profiles: Vec<FingerprintProfile>,
}

impl RotationSet {
    /// Deterministic rotation: the same (session_index) always picks the
    /// same profile (stable across restarts; no randomness in the kernel).
    pub fn pick(&self, session_index: usize) -> FingerprintProfile {
        if self.profiles.is_empty() {
            // Never empty in practice — callers build from `defaults()`.
            default_profile()
        } else {
            self.profiles[session_index % self.profiles.len()].clone()
        }
    }
}

/// Three coherent modern-Chrome profiles (Win / macOS / Linux). Data-only:
/// UAs are representative Chrome 126 strings.
pub fn defaults() -> RotationSet {
    RotationSet {
        profiles: vec![
            FingerprintProfile {
                ua: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36".into(),
                platform: "Win32".into(),
                webgl_vendor: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into(),
                canvas_noise: true,
                hardware: (8, 8),
                languages: vec!["en-US".into(), "en".into()],
            },
            FingerprintProfile {
                ua: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36".into(),
                platform: "MacIntel".into(),
                webgl_vendor: "ANGLE (Apple, Apple M1, OpenGL 4.1)".into(),
                canvas_noise: true,
                hardware: (8, 16),
                languages: vec!["en-US".into(), "en".into()],
            },
            FingerprintProfile {
                ua: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36".into(),
                platform: "Linux x86_64".into(),
                webgl_vendor: "ANGLE (Mesa, Mesa Intel(R) UHD Graphics 630 (CML GT2), OpenGL 4.5)".into(),
                canvas_noise: false,
                hardware: (4, 4),
                languages: vec!["en-US".into(), "en".into()],
            },
        ],
    }
}

/// The always-available fallback profile (built on demand — `Vec` fields
/// can't be const).
fn default_profile() -> FingerprintProfile {
    FingerprintProfile {
        ua: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36".into(),
        platform: "Linux x86_64".into(),
        webgl_vendor: "ANGLE (Mesa, Mesa Intel(R) UHD Graphics 630 (CML GT2), OpenGL 4.5)".into(),
        canvas_noise: false,
        hardware: (4, 4),
        languages: vec!["en-US".into(), "en".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_is_deterministic() {
        let set = defaults();
        let a = set.pick(0).ua;
        let b = set.pick(0).ua;
        assert_eq!(a, b);
        // And it actually rotates.
        let c = set.pick(1).ua;
        assert_ne!(a, c);
        // Wraps around.
        assert_eq!(set.pick(3).ua, set.pick(0).ua);
    }

    #[test]
    fn profiles_are_coherent() {
        let set = defaults();
        for p in &set.profiles {
            assert!(p.ua.contains("Chrome/"));
            assert!(!p.platform.is_empty());
            assert!(p.webgl_vendor.starts_with("ANGLE"));
            assert!(!p.languages.is_empty());
        }
    }
}
