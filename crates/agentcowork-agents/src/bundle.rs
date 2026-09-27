//! P31.2 — the agent bundle manifest (`agent.toml`), versioned schema
//! (I6-compatible). One bundle carries persona + engine binding + optional
//! model/provider pin + scoped MCP/connectors/skills/tools + workflows —
//! everything a custom agent is.

use agentcowork_types::{AgentDefinition, AgentId, AgentProtocol, AuthMode, CapabilityId};
use serde::{Deserialize, Serialize};

use crate::registry::slug;

pub const BUNDLE_SCHEMA_VERSION: u32 = 1;

/// The engine the bundle binds to (P31.8): the brain, swappable without
/// touching persona or scopes.
///
/// ADR-0005: in v1 the executable brain is an **external ACP agent**. The
/// built-in `Inbuilt` variant is deferred to post-v1 and is deliberately
/// absent — a bundle can never bind an engine the app does not have. A bundle
/// with **no** binding (`None`) is a draft: it is stored, it is not an agent,
/// and it must not appear in the runnable directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineBinding {
    /// An ACP-installed CLI agent (Claude Code / Codex / …).
    Acp(String),
    /// Model-only: no tools, no engine — chat-only brain.
    ModelOnly,
}

impl EngineBinding {
    /// Stable wire/label spelling: `acp:<agent-id>`, `model-only`, or
    /// `unbound` for the draft state. Never a debug string — the Settings/
    /// registry rows read this verbatim.
    pub fn label(binding: Option<&EngineBinding>) -> String {
        match binding {
            Some(EngineBinding::Acp(id)) => format!("acp:{id}"),
            Some(EngineBinding::ModelOnly) => "model-only".to_string(),
            None => "unbound".to_string(),
        }
    }
}

/// The model pin: `None` = inherit from the chat bar at send time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ModelPin {
    pub fn is_inherited(&self) -> bool {
        self.provider.is_none() && self.model.is_none()
    }

    /// Optional pin: inherited default.
    pub fn inherited() -> Self {
        Self::default()
    }

    pub fn pinned(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: Some(provider.into()),
            model: Some(model.into()),
        }
    }
}

/// Tool allow/deny lists. `allow` empty = inherit whatever the engine offers
/// (agentcowork-guard CapabilityGranter semantics); explicit entries always win.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolScope {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl ToolScope {
    pub fn allows(&self, tool: &str) -> bool {
        if self.deny.contains(&tool.to_string()) {
            return false;
        }
        self.allow.is_empty() || self.allow.contains(&tool.to_string())
    }
}

/// The versioned bundle (P31.2). Round-trips through `agent.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentBundle {
    pub schema_version: u32,
    /// Identity (wizard step 1).
    pub name: String,
    pub emoji: String,
    pub description: String,
    /// Brain (step 2).
    #[serde(default)]
    pub persona: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// The bound brain. `None` = not chosen yet; an unbound bundle is a draft
    /// and cannot act as an agent (`definition()` returns `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineBinding>,
    pub model: ModelPin,
    /// Capabilities (step 3) — exact subsets, never "all".
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub connectors: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: ToolScope,
    /// Workflows (step 4).
    #[serde(default)]
    pub blueprints: Vec<String>,
    #[serde(default)]
    pub automations: Vec<String>,
}

impl AgentBundle {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: BUNDLE_SCHEMA_VERSION,
            name: name.into(),
            emoji: "🤖".into(),
            description: String::new(),
            persona: None,
            system_prompt: None,
            engine: None,
            model: ModelPin::inherited(),
            mcp_servers: Vec::new(),
            connectors: Vec::new(),
            skills: Vec::new(),
            tools: ToolScope::default(),
            blueprints: Vec::new(),
            automations: Vec::new(),
        }
    }

    /// The toolset this bundle's capability grant computes to.
    pub fn effective_tools(&self, engine_offered: &[String]) -> Vec<String> {
        engine_offered
            .iter()
            .filter(|t| self.tools.allows(t))
            .cloned()
            .collect()
    }

    /// Scope check: does this bundle declare this MCP server?
    pub fn declares_mcp(&self, server_id: &str) -> bool {
        self.mcp_servers.iter().any(|s| s == server_id)
    }

    /// Scope check: does this bundle declare this connector?
    pub fn declares_connector(&self, connector_id: &str) -> bool {
        self.connectors.iter().any(|c| c == connector_id)
    }

    /// Project this bundle onto the canonical [`AgentDefinition`] so the
    /// directory has exactly one record shape (P69.D1). The id is the same
    /// slug the bundle store uses, so bundle ⇄ definition ids cannot drift.
    ///
    /// `None` when no engine is bound: a draft bundle is not an agent, and the
    /// directory must not carry a brain that cannot run (ADR-0005 — nothing
    /// depends on a built-in binding being present, and no unbound row may
    /// render as available).
    ///
    /// Auth mode is `Unknown` on purpose: a bundle declares *which engine* it
    /// binds to, not how that engine authenticates — the ACP handshake is the
    /// authoritative source (`ARCH/03-BYOK-KEYRINGS.md` §3.0).
    pub fn definition(&self) -> Option<AgentDefinition> {
        let protocol = match self.engine.as_ref()? {
            EngineBinding::Acp(_) => AgentProtocol::Acp,
            EngineBinding::ModelOnly => AgentProtocol::ModelOnly,
        };
        Some(AgentDefinition {
            id: AgentId::new(slug(&self.name)),
            name: self.name.clone(),
            description: self.description.clone(),
            protocol,
            auth_mode: AuthMode::Unknown,
            capabilities: self
                .skills
                .iter()
                .map(|s| CapabilityId::from(s.as_str()))
                .collect(),
            extension_mechanisms: Vec::new(),
        })
    }

    pub fn from_toml(src: &str) -> Result<Self, String> {
        toml::from_str(src).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_toml() {
        let mut b = AgentBundle::new("Grace");
        b.engine = Some(EngineBinding::Acp("claude-code".into()));
        b.model = ModelPin::pinned("anthropic", "claude-sonnet-4");
        let toml = b.to_toml().unwrap();
        let back = AgentBundle::from_toml(&toml).unwrap();
        assert_eq!(back.name, "Grace");
        assert_eq!(back.engine, Some(EngineBinding::Acp("claude-code".into())));
        assert_eq!(back.model.model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn unbound_bundle_is_a_draft_not_an_agent() {
        // ADR-0005: nothing may present an engine that is not bound. A fresh
        // bundle has no brain; it projects to no `AgentDefinition` at all.
        let b = AgentBundle::new("Draft");
        assert!(b.engine.is_none());
        assert!(b.definition().is_none());
        // An ACP binding projects as an ACP agent.
        let mut bound = b.clone();
        bound.engine = Some(EngineBinding::Acp("claude-code".into()));
        assert_eq!(bound.definition().unwrap().protocol, AgentProtocol::Acp);
        // Model-only is not ACP and must never claim to be.
        let mut mo = b;
        mo.engine = Some(EngineBinding::ModelOnly);
        assert_eq!(mo.definition().unwrap().protocol, AgentProtocol::ModelOnly);
    }

    #[test]
    fn default_is_inherited_model() {
        let b = AgentBundle::new("x");
        assert!(b.model.is_inherited());
    }

    #[test]
    fn tool_scope_allow_empty_means_inherit() {
        let b = AgentBundle::new("x");
        assert!(b.tools.allows("fs.read"));
        assert!(b.tools.allows("anything"));
    }

    #[test]
    fn deny_wins_over_allow() {
        let mut b = AgentBundle::new("x");
        b.tools.allow = vec!["fs.write".into()];
        b.tools.deny = vec!["fs.remove".into()];
        assert!(b.tools.allows("fs.write"));
        assert!(!b.tools.allows("fs.remove"));
    }

    #[test]
    fn effective_tools_filters_by_scope() {
        let mut b = AgentBundle::new("x");
        b.tools.allow = vec!["fs.read".into(), "shell".into()];
        let offered = vec!["fs.read".into(), "fs.write".into(), "shell".into()];
        assert_eq!(b.effective_tools(&offered), vec!["fs.read", "shell"]);
    }
}
