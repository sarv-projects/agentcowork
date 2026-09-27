//! K4 passports (slim) (doc 81 §4): a portable scoped context packet over
//! C10 pass-by-ref + C6 graph + SCIP symbols — model/agent/device handoff
//! honoring scope. The passport is user-owned and portable (the switching
//! *state* advantage): it can be exported, inspected, and revoked, and it
//! can never grant more than its declared [`PassportScope`] — scope is
//! enforced at every handoff, not assumed.

use crate::reference::RefHandle;
use serde::{Deserialize, Serialize};

/// The scope a passport may exercise. Enforcement is the contract: a
/// passport's `refs`/`symbols`/`graph` entries are only meaningful inside
/// this scope; anything else is refused at handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassportScope {
    /// The workspace root this passport is valid for (empty = global).
    pub workspace: String,
    /// Agent ids allowed to consume this passport (empty = any).
    pub allowed_agents: Vec<String>,
    /// Whether provider-network access is included.
    pub network: bool,
}

/// One context entry the passport carries (the C10 pass-by-ref form).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassportEntry {
    pub label: String,
    pub kind: String,
    /// The pass-by-reference handle (preview + bounded payload).
    pub handle: RefHandle,
    /// The C6 graph node this entry anchors to (empty = standalone).
    #[serde(default)]
    pub graph_node: String,
    /// The SCIP symbol this entry refers to (empty = none).
    #[serde(default)]
    pub symbol: String,
}

/// The slim passport: scope + entries + provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPassport {
    pub id: String,
    pub scope: PassportScope,
    pub entries: Vec<PassportEntry>,
    /// The graph summary (C6) — entity names + edge counts, not the graph
    /// itself (the graph stays local; the passport carries the fingerprint).
    pub graph_summary: Vec<String>,
    /// The agent/session that issued it.
    pub issued_by: String,
}

impl ContextPassport {
    /// A passport honors a handoff when the target agent is allowed and the
    /// workspace matches. Fail-closed: unknown workspace or disallowed agent
    /// → refused, never silently widened.
    pub fn honors(&self, agent: &str, workspace: &str) -> bool {
        if !self.scope.allowed_agents.is_empty()
            && !self.scope.allowed_agents.iter().any(|a| a == agent)
        {
            return false;
        }
        if !self.scope.workspace.is_empty() && self.scope.workspace != workspace {
            return false;
        }
        true
    }

    /// The entries a consumer may actually use under a handoff (filtered by
    /// scope — a passport never leaks more than the handoff allows).
    pub fn entries_for(&self, agent: &str, workspace: &str) -> Vec<&PassportEntry> {
        if !self.honors(agent, workspace) {
            return Vec::new();
        }
        self.entries.iter().collect()
    }

    /// Deterministic render — the handoff prompt's context block.
    pub fn render(&self, agent: &str, workspace: &str) -> String {
        let entries = self.entries_for(agent, workspace);
        let mut out = format!(
            "# Context passport {}\nworkspace: {} · agent: {}\n",
            self.id,
            if self.scope.workspace.is_empty() {
                "(global)"
            } else {
                &self.scope.workspace
            },
            agent,
        );
        if self.honors(agent, workspace) {
            out.push_str(&format!("graph: {}\n", self.graph_summary.join(", ")));
            for e in &entries {
                out.push_str(&format!(
                    "- [{}] {} ({} tokens preview, symbol {})\n",
                    e.kind,
                    e.label,
                    e.handle.preview_tokens(),
                    if e.symbol.is_empty() { "-" } else { &e.symbol },
                ));
            }
        } else {
            out.push_str("(scope refused — no entries)\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::{RefKind, make_ref_handle};

    fn passport() -> ContextPassport {
        ContextPassport {
            id: "p-1".into(),
            scope: PassportScope {
                workspace: "/repo".into(),
                allowed_agents: vec!["claude".into()],
                network: false,
            },
            entries: vec![PassportEntry {
                label: "config".into(),
                kind: "file".into(),
                handle: make_ref_handle(
                    "config.json",
                    "config.json",
                    RefKind::File,
                    "{\"k\":\"v\"}",
                    11,
                    None,
                ),
                graph_node: "n1".into(),
                symbol: "Config".into(),
            }],
            graph_summary: vec!["3 entities, 5 edges".into()],
            issued_by: "agentcowork-native".into(),
        }
    }

    #[test]
    fn honors_checks_agent_and_workspace() {
        let p = passport();
        assert!(p.honors("claude", "/repo"));
        assert!(!p.honors("codex", "/repo")); // agent not allowed
        assert!(!p.honors("claude", "/other")); // workspace mismatch
    }

    #[test]
    fn entries_never_leak_outside_scope() {
        let p = passport();
        assert_eq!(p.entries_for("codex", "/repo").len(), 0);
        assert_eq!(p.entries_for("claude", "/repo").len(), 1);
        let rendered = p.render("codex", "/repo");
        assert!(rendered.contains("scope refused"));
        let rendered_ok = p.render("claude", "/repo");
        assert!(rendered_ok.contains("config")); // the entry label
        assert!(rendered_ok.contains("Config")); // the SCIP symbol
    }

    #[test]
    fn serializes_and_roundtrips() {
        let p = passport();
        let json = serde_json::to_string(&p).unwrap();
        let back: ContextPassport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
