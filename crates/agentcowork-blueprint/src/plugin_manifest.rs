//! P23-1 — the Claude plugin manifest (`.claude-plugin/plugin.json`,
//! doc 75 §3 — 🟡 ADAPT).
//!
//! The canonical extension packaging format: one directory bundling skills +
//! agents + hooks + MCP + LSP + monitors + themes. This module parses the
//! JSON manifest shape (the slimmer, stricter sibling of our `manifest.toml`
//! ABI) and lays the components out for install. Design decisions:
//!
//! - **Immutable slug** — `name` cannot change once published; `displayName`
//!   is the UI label; `renames` auto-migrates old slugs on next sync.
//! - **Skill-bundle plugins** — `strict: false` + explicit `skills` array +
//!   `source` (`git-subdir` + `sha` pin). Each skill registers as
//!   `<plugin>:<skill>`.
//! - The manifest is *data* — installing it into our registry is the F8
//!   installer's job (Guard-2, sha-pinned). This module owns parse +
//!   validate + the layout map.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The plugin descriptor (`plugin.json` — the doc-75 shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudePluginManifest {
    /// The immutable slug (kebab-case; cannot change once published).
    pub name: String,
    /// Opaque semver-ish version string the vendor publishes.
    pub version: String,
    /// Human label for the UI (immutable slug ≠ display name).
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: String,
    /// The component table — every dir this plugin bundles.
    #[serde(default)]
    pub components: Components,
    /// Old slugs this plugin used to be known as (auto-migrated on sync).
    #[serde(default)]
    pub renames: Vec<String>,
    /// skill-bundle mode: `false` (default) = components dir; `true` = the
    /// plugin IS a skill bundle (skills array explicit).
    #[serde(default)]
    pub strict: bool,
    /// Explicit skill list (skill-bundle plugins).
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    /// Where the plugin came from (`git-subdir` + sha pin, for the
    /// registry installer).
    #[serde(default)]
    pub source: Option<PluginSource>,
}

/// The component table (which dirs a plugin contributes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Components {
    /// `skills/` (each a `SKILL.md` dir) — the I2 skill surface.
    #[serde(default)]
    pub skills: bool,
    /// `agents/` (markdown agent defs with the doc-75 agent fields).
    #[serde(default)]
    pub agents: bool,
    /// `hooks/hooks.json` — P7 profile-gated hooks (event taxonomy ref).
    #[serde(default)]
    pub hooks: bool,
    /// `.mcp.json` — bundled MCP servers started on enable (P22 pattern).
    #[serde(default)]
    pub mcp: bool,
    /// `.lsp.json` — bundled LSP servers (I11 codeintel).
    #[serde(default)]
    pub lsp: bool,
    /// `monitors/monitors.json` — background cmd → stdout notifications
    /// (pairs with B7 heartbeat).
    #[serde(default)]
    pub monitors: bool,
    /// `themes/*.json` — base preset + sparse overrides (UI v2 themes).
    #[serde(default)]
    pub themes: bool,
}

/// A skill bundle entry (registers as `<plugin>:<skill>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRef {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// The pinned source for the registry installer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSource {
    /// e.g. `git+https://github.com/anthropics/claude-plugins-official`.
    pub kind: String,
    pub url: String,
    /// `git-subdir` when the plugin lives in a subdirectory of the repo.
    #[serde(default)]
    pub subdir: Option<String>,
    /// The exact commit/sha the bundle is pinned to (K6).
    #[serde(default)]
    pub sha: Option<String>,
}

/// Errors (fail-closed: a malformed manifest is rejected, never guessed).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginManifestError {
    #[error("missing name (slug is required)")]
    MissingName,
    #[error("invalid name `{0}` (must be [a-z0-9-]+ up to 64 chars)")]
    InvalidName(String),
    #[error("missing version")]
    MissingVersion,
    #[error("no components or skills declared")]
    Empty,
    #[error("skill-bundle must declare skills")]
    StrictRequiresSkills,
}

impl ClaudePluginManifest {
    pub fn parse(source: &str) -> Result<Self, PluginManifestError> {
        let m: ClaudePluginManifest =
            serde_json::from_str(source).map_err(|_| PluginManifestError::Empty)?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if self.name.is_empty() {
            return Err(PluginManifestError::MissingName);
        }
        let valid = self.name.len() <= 64
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            return Err(PluginManifestError::InvalidName(self.name.clone()));
        }
        if self.version.is_empty() {
            return Err(PluginManifestError::MissingVersion);
        }
        let has_components = self.components.skills
            || self.components.agents
            || self.components.hooks
            || self.components.mcp
            || self.components.lsp
            || self.components.monitors
            || self.components.themes;
        if self.strict && self.skills.is_empty() {
            return Err(PluginManifestError::StrictRequiresSkills);
        }
        if !has_components && self.skills.is_empty() {
            return Err(PluginManifestError::Empty);
        }
        Ok(())
    }

    /// The directory layout this plugin expects (used by the installer to
    /// verify the fetch).
    pub fn layout_dirs(&self) -> Vec<String> {
        let mut dirs = Vec::new();
        if self.components.skills || self.strict {
            dirs.push("skills".to_string());
        }
        if self.components.agents {
            dirs.push("agents".to_string());
        }
        if self.components.hooks {
            dirs.push("hooks".to_string());
        }
        if self.components.mcp {
            dirs.push(".mcp.json".to_string());
        }
        if self.components.lsp {
            dirs.push(".lsp.json".to_string());
        }
        if self.components.monitors {
            dirs.push("monitors".to_string());
        }
        if self.components.themes {
            dirs.push("themes".to_string());
        }
        dirs
    }

    /// The registered skill surface: `<plugin>:<skill>` for each declared
    /// skill (skill-bundle) or a single `<plugin>` component skill.
    pub fn registered_skill_names(&self) -> Vec<String> {
        if self.strict || !self.skills.is_empty() {
            self.skills
                .iter()
                .map(|s| format!("{}:{}", self.name, s.name))
                .collect()
        } else if self.components.skills {
            vec![self.name.clone()]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_bundle_json() -> &'static str {
        r#"{
            "name": "doc-skill",
            "version": "1.0.0",
            "displayName": "Document Skills",
            "strict": true,
            "skills": [
                {"name": "pdf", "path": "skills/pdf", "description": "PDF skill"},
                {"name": "docx", "path": "skills/docx"}
            ],
            "source": {"kind": "git", "url": "https://github.com/anthropics/skills", "subdir": "skills/docx", "sha": "abc123"}
        }"#
    }

    fn components_json() -> &'static str {
        r#"{
            "name": "my-toolbox",
            "version": "0.2.1",
            "description": "A toolbox",
            "components": {"skills": true, "mcp": true, "monitors": true},
            "renames": ["old-toolbox"]
        }"#
    }

    #[test]
    fn parses_skill_bundle() {
        let m = ClaudePluginManifest::parse(skill_bundle_json()).unwrap();
        assert_eq!(m.name, "doc-skill");
        assert_eq!(m.display_name.as_deref(), Some("Document Skills"));
        assert!(m.strict);
        assert_eq!(m.skills.len(), 2);
        assert_eq!(
            m.registered_skill_names(),
            vec!["doc-skill:pdf", "doc-skill:docx"]
        );
        assert_eq!(m.layout_dirs(), vec!["skills"]);
    }

    #[test]
    fn parses_components_plugin() {
        let m = ClaudePluginManifest::parse(components_json()).unwrap();
        assert!(m.components.skills && m.components.mcp && m.components.monitors);
        assert_eq!(m.renames, vec!["old-toolbox"]);
        assert_eq!(m.registered_skill_names(), vec!["my-toolbox"]);
    }

    #[test]
    fn validates_fail_closed() {
        assert_eq!(
            ClaudePluginManifest::parse(r#"{"version":"1.0.0"}"#),
            Err(PluginManifestError::Empty)
        );
        assert_eq!(
            ClaudePluginManifest::parse(
                r#"{"name":"Bad Name","version":"1.0.0","skills":[{"name":"x"}]}"#
            ),
            Err(PluginManifestError::InvalidName("Bad Name".into()))
        );
        assert_eq!(
            ClaudePluginManifest::parse(r#"{"name":"strict","version":"1.0.0","strict":true}"#),
            Err(PluginManifestError::StrictRequiresSkills)
        );
    }
}
