//! G9 resource-drop policy (doc 65 §2 — Scrapling steal): block the heavy
//! junk a browsing agent never needs — ad networks, media, fonts — before it
//! crosses the wire. The policy compiles down to `Network.setBlockedURLs`
//! patterns the CDP layer feeds Chrome, complementing the G9 adblock-crate
//! read-cleaner (which strips content *after* fetch; this stops it *at* the
//! network boundary).

use serde::{Deserialize, Serialize};

/// A domain to block (matched as a URL substring — `doubleclick.net` blocks
/// `https://ad.doubleclick.net/...`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain(pub String);

/// The drop policy. `block_ads` is a curated domain list; the two booleans
/// are broad strokes for the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDropPolicy {
    #[serde(default)]
    pub block_ads: Vec<Domain>,
    #[serde(default)]
    pub drop_media: bool,
    #[serde(default)]
    pub drop_fonts: bool,
}

impl ResourceDropPolicy {
    /// A sensible default browsing profile (the same spirit as the read
    /// cleaner): major ad/tracker networks blocked, media + fonts dropped.
    pub fn lean() -> Self {
        Self {
            block_ads: DEFAULT_AD_NETWORKS
                .iter()
                .map(|d| Domain(d.to_string()))
                .collect(),
            drop_media: true,
            drop_fonts: true,
        }
    }

    /// Compile to `Network.setBlockedURLs` URL patterns. Every entry is a
    /// substring pattern Chrome matches against the request URL — blocking
    /// happens before any bytes are fetched.
    pub fn set_blocked_urls(&self) -> Vec<String> {
        let mut patterns: Vec<String> = self.block_ads.iter().map(|d| d.0.clone()).collect();
        if self.drop_media {
            patterns.extend(MEDIA_EXTENSIONS.iter().map(|e| format!("*{e}")));
        }
        if self.drop_fonts {
            patterns.extend(FONT_EXTENSIONS.iter().map(|e| format!("*{e}")));
        }
        patterns
    }

    /// Whether any patterns would be sent (lets the caller skip the CDP call
    /// entirely for an empty policy).
    pub fn is_empty(&self) -> bool {
        self.set_blocked_urls().is_empty()
    }
}

/// The common ad/tracker networks (curated, short — the read cleaner's
/// longer list stays the content-side authority).
const DEFAULT_AD_NETWORKS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adservice.google.com",
    "amazon-adsystem.com",
    "adsystem.com",
    "criteo.com",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
];

const MEDIA_EXTENSIONS: &[&str] = &[
    ".mp4", ".webm", ".ogg", ".mp3", ".wav", ".m4a", ".avi", ".mov", ".m3u8",
];

const FONT_EXTENSIONS: &[&str] = &[".woff", ".woff2", ".ttf", ".otf", ".eot"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_policy_blocks_ads_media_fonts() {
        let p = ResourceDropPolicy::lean();
        let patterns = p.set_blocked_urls();
        assert!(patterns.iter().any(|x| x.contains("doubleclick.net")));
        assert!(patterns.iter().any(|x| x.ends_with(".mp4")));
        assert!(patterns.iter().any(|x| x.ends_with(".woff2")));
        assert!(!p.is_empty());
    }

    #[test]
    fn empty_policy_compiles_to_nothing() {
        let p = ResourceDropPolicy::default();
        assert!(p.is_empty());
        assert_eq!(p.set_blocked_urls(), Vec::<String>::new());
    }

    #[test]
    fn custom_domain_list() {
        let p = ResourceDropPolicy {
            block_ads: vec![Domain("ads.example.com".into())],
            drop_media: false,
            drop_fonts: false,
        };
        assert_eq!(p.set_blocked_urls(), vec!["ads.example.com".to_string()]);
    }
}
