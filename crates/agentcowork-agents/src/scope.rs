//! P31.4/5/6 — per-agent scoping: the bundle's declared subsets become the
//! *capability grant*. Running Agent X never loads Agent Y's servers, and a
//! custom agent can't exceed its declared surface (I6 CapabilityGranter
//! semantics live in agentcowork-guard; this module is the bundle-side
//! computation).

use crate::bundle::AgentBundle;
use std::collections::BTreeSet;

/// What one agent is allowed to reach, computed from its bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentScopes {
    /// Exact MCP server subset (tick, never "all").
    pub mcp_servers: BTreeSet<String>,
    /// Exact connector subset.
    pub connectors: BTreeSet<String>,
    /// Exact skills subset.
    pub skills: BTreeSet<String>,
    /// Tool allow-list (empty = engine default) / deny-list.
    pub tools_allow: BTreeSet<String>,
    pub tools_deny: BTreeSet<String>,
}

impl AgentScopes {
    pub fn from_bundle(b: &AgentBundle) -> Self {
        Self {
            mcp_servers: b.mcp_servers.iter().cloned().collect(),
            connectors: b.connectors.iter().cloned().collect(),
            skills: b.skills.iter().cloned().collect(),
            tools_allow: b.tools.allow.iter().cloned().collect(),
            tools_deny: b.tools.deny.iter().cloned().collect(),
        }
    }

    /// Can this agent use this MCP server? Exact-subset rule: only declared.
    pub fn can_use_mcp(&self, server_id: &str) -> bool {
        self.mcp_servers.contains(server_id)
    }

    /// Can this agent attach this connector?
    pub fn can_use_connector(&self, connector_id: &str) -> bool {
        self.connectors.contains(connector_id)
    }

    /// Can this agent run this skill?
    pub fn can_use_skill(&self, skill: &str) -> bool {
        self.skills.contains(skill)
    }

    /// The tool capability check (mirrors I6 CapabilityGranter semantics):
    /// deny wins; allow empty = engine default (everything the host offers);
    /// allow non-empty = the declared subset only.
    pub fn tool_allowed(&self, tool: &str) -> bool {
        if self.tools_deny.contains(tool) {
            return false;
        }
        self.tools_allow.is_empty() || self.tools_allow.contains(tool)
    }

    /// Injected tool list for an agent session: intersection of this scope
    /// and what the host has available.
    pub fn tools_for(&self, host_tools: &[String]) -> Vec<String> {
        host_tools
            .iter()
            .filter(|t| self.tool_allowed(t))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::ToolScope;

    fn bundle() -> AgentBundle {
        let mut b = AgentBundle::new("Scoped");
        b.mcp_servers = vec!["fs".into()];
        b.connectors = vec!["gmail".into()];
        b.skills = vec!["spreadsheet".into()];
        b.tools = ToolScope {
            allow: vec!["fs.read".into()],
            deny: vec!["fs.remove".into()],
        };
        b
    }

    #[test]
    fn exact_subsets_never_all() {
        let s = AgentScopes::from_bundle(&bundle());
        assert!(s.can_use_mcp("fs"));
        assert!(!s.can_use_mcp("db")); // not declared → never loaded
        assert!(s.can_use_connector("gmail"));
        assert!(!s.can_use_connector("slack"));
        assert!(s.can_use_skill("spreadsheet"));
        assert!(!s.can_use_skill("docker"));
    }

    #[test]
    fn allowlist_gates_tools() {
        let s = AgentScopes::from_bundle(&bundle());
        assert!(s.tool_allowed("fs.read"));
        assert!(!s.tool_allowed("fs.remove")); // explicit deny wins
        assert!(!s.tool_allowed("shell")); // not in the allow subset
        // Injected surface = intersection.
        let host = vec![
            "fs.read".to_string(),
            "fs.write".to_string(),
            "shell".to_string(),
        ];
        assert_eq!(s.tools_for(&host), vec!["fs.read"]);
    }

    #[test]
    fn empty_allowlist_inherits_engine_default() {
        let b = AgentBundle::new("Open");
        let s = AgentScopes::from_bundle(&b);
        assert!(s.tool_allowed("anything")); // default = everything the engine offers
    }
}
