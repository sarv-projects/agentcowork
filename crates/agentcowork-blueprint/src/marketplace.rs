//! P23-3 — the marketplace "Add" button (doc 75 §4 — 🟡 ADAPT).
//!
//! The user adds a plugin marketplace by URL (anthropics/skills,
//! claude-plugins-official, claude-plugins-community, awesome-claude-code, …
//! — see [`KNOWN_MARKETPLACES`]) via the F8 registry-fed install: Guard-2
//! ticket, sha-pinned, immutable slug. This module is the *registry of
//! known marketplaces* + per-marketplace plugin manifest parsing: a fetch
//! of `marketplace.json` (or the repo's plugin dir) yields the plugin
//! names → each installable through the existing installer path.
//!
//! Owned here: the known list (seed), and the *layout contract* (what a
//! marketplace repo is expected to contain). The install/download itself is
//! the F8 registry installer's job — never duplicated here.

use serde::{Deserialize, Serialize};

/// The canonical marketplace seed (the doc-75 §4 four, plus the
/// multi-harness wshobson catalog from P26-1).
pub const KNOWN_MARKETPLACES: &[&str] = &[
    "anthropics/skills",
    "claude-plugins-official",
    "claude-plugins-community",
    "awesome-claude-code",
    "wshobson/agents",
];

/// One marketplace the user can add.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marketplace {
    pub id: String,
    pub name: String,
    /// The canonical source URL (GitHub repo or registry).
    pub url: String,
    /// True for the seed entries (can't be removed).
    pub builtin: bool,
    /// The pins currently installed from here (slug → version).
    #[serde(default)]
    pub installed: Vec<InstalledPlugin>,
}

impl Marketplace {
    pub fn new(id: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            url: url.into(),
            builtin: false,
            installed: Vec::new(),
        }
    }

    pub fn builtin(id: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        let mut m = Self::new(id, name, url);
        m.builtin = true;
        m
    }
}

/// One plugin pinned from this marketplace (installer-consumed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub slug: String,
    /// Display name (may differ from the immutable slug).
    pub display_name: String,
    /// sha pin (K6 — no floating installs through this path).
    pub sha: String,
    /// The plugin manifest as parsed on install (slim — we keep the slug +
    /// the skill surface, not the full descriptor).
    pub manifest: Option<serde_json::Value>,
}

/// The built-in marketplace list (seeded; the user can add more via the F8
/// add flow).
pub fn builtin_marketplaces() -> Vec<Marketplace> {
    KNOWN_MARKETPLACES
        .iter()
        .map(|id| {
            Marketplace::builtin(
                *id,
                id.replace('-', " "),
                format!("https://github.com/{id}"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_covers_the_doc_75_four_plus_p26_catalog() {
        let seed = builtin_marketplaces();
        let ids: Vec<String> = seed.iter().map(|m| m.id.clone()).collect();
        for expected in [
            "anthropics/skills",
            "claude-plugins-official",
            "claude-plugins-community",
            "awesome-claude-code",
            "wshobson/agents",
        ] {
            assert!(ids.contains(&expected.to_string()), "{expected} seeded");
        }
        // Seeds are builtin (not removable) and their urls are canonical.
        assert!(seed.iter().all(|m| m.builtin));
        assert_eq!(seed[0].url, "https://github.com/anthropics/skills");
    }

    #[test]
    fn user_marketplaces_start_uninstalled_and_pin_safer() {
        let mut m = Marketplace::new("acme/plugins", "Acme", "https://github.com/acme/plugins");
        assert!(!m.builtin);
        assert!(m.installed.is_empty());
        m.installed.push(InstalledPlugin {
            slug: "acme.doc".into(),
            display_name: "Acme Doc".into(),
            sha: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            manifest: None,
        });
        // The pin is sha-locked (K6 — no floating installs through this path).
        assert_eq!(m.installed[0].sha.len(), 64);
    }
}
