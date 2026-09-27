//! P69.B — named contracts for the planes that previously lived only in prose.
//!
//! Shapes and pure decisions live here. Runtimes that persist or execute them
//! stay in their owning crates. Nothing in this module performs an effect.

use serde::{Deserialize, Serialize};

use crate::{AgentBindingId, SessionId, SpaceId, WorkId, WorkspaceId};

/// A chat may have no project. The space is still required so old sessions
/// can be migrated onto one owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPlacement {
    pub space_id: SpaceId,
    pub project_id: Option<crate::ProjectId>,
    pub workspace_id: WorkspaceId,
    pub session_id: SessionId,
}

/// Sessions stored before Space existed get the caller's space and keep a
/// null project. This does not invent a project.
pub fn migrate_session_without_space(
    session_id: SessionId,
    workspace_id: WorkspaceId,
    space_id: SpaceId,
) -> SessionPlacement {
    SessionPlacement {
        space_id,
        project_id: None,
        workspace_id,
        session_id,
    }
}

/// Rows the adapter negotiates. Absence is `Unknown`, never an implied yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    Supported,
    Negotiated,
    Unsupported,
    Unknown,
}

/// The negotiated matrix. Stored per agent. The kernel does not branch on
/// the agent name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedCapabilityMatrix {
    pub acp_version: String,
    pub session_resume: CapabilitySupport,
    pub mcp: CapabilitySupport,
    pub fs_mediation: CapabilitySupport,
    pub terminal_mediation: CapabilitySupport,
    pub permission_callback: CapabilitySupport,
    pub subagents: CapabilitySupport,
    pub model_switching: CapabilitySupport,
    pub cancellation: CapabilitySupport,
    pub streaming: CapabilitySupport,
    pub context_injection: CapabilitySupport,
    pub tool_interception: CapabilitySupport,
    pub memory_injection: CapabilitySupport,
    pub compaction_hook: CapabilitySupport,
    pub programmatic_control: CapabilitySupport,
}

impl Default for NegotiatedCapabilityMatrix {
    fn default() -> Self {
        Self {
            acp_version: "unknown".into(),
            session_resume: CapabilitySupport::Unknown,
            mcp: CapabilitySupport::Unknown,
            fs_mediation: CapabilitySupport::Unknown,
            terminal_mediation: CapabilitySupport::Unknown,
            permission_callback: CapabilitySupport::Unknown,
            subagents: CapabilitySupport::Unknown,
            model_switching: CapabilitySupport::Unknown,
            cancellation: CapabilitySupport::Unknown,
            streaming: CapabilitySupport::Unknown,
            context_injection: CapabilitySupport::Unknown,
            tool_interception: CapabilitySupport::Unknown,
            memory_injection: CapabilitySupport::Unknown,
            compaction_hook: CapabilitySupport::Unknown,
            programmatic_control: CapabilitySupport::Unknown,
        }
    }
}

/// Method names of the one adapter contract. Adapters implement these; the
/// kernel does not grow a method per product.
pub const ADAPTER_METHODS: &[&str] = &[
    "discover",
    "capabilities",
    "start",
    "create_session",
    "resume_session",
    "close_session",
    "prompt",
    "cancel",
    "inject_context",
    "attach_shared_plane",
    "stream_events",
    "collect_usage",
];

/// A short-lived bridge credential. The record names the credential. It does
/// not hold the secret, and it does not perform an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBridgeRecord {
    pub bridge_id: String,
    pub binding_id: AgentBindingId,
    pub work_id: WorkId,
    pub session_id: SessionId,
    pub expires_at_ms: u64,
}

impl AgentBridgeRecord {
    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }
}

/// One edit to a context surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextOp {
    Retain { node_id: String },
    Inject { node_id: String, tokens: u64 },
    Replace { node_id: String, tokens: u64 },
    Reference { node_id: String, handle: String },
}

/// The bounded projection in front of one binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSurface {
    pub binding_id: AgentBindingId,
    pub token_estimate: u64,
    pub cache_boundary_seq: u64,
    pub ops: Vec<ContextOp>,
}

impl ContextSurface {
    pub fn apply(&mut self, op: ContextOp) {
        match &op {
            ContextOp::Inject { tokens, .. } | ContextOp::Replace { tokens, .. } => {
                self.token_estimate = self.token_estimate.saturating_add(*tokens);
            }
            ContextOp::Retain { .. } | ContextOp::Reference { .. } => {}
        }
        self.ops.push(op);
    }
}

/// Execution provenance for a child or a resumed binding. Not the words the
/// model should continue from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCapsule {
    pub parent_work_id: WorkId,
    pub child_binding_id: AgentBindingId,
    pub workspace_id: WorkspaceId,
    pub snapshot_hash: String,
}

/// What a pack provides and what it cannot run without.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityPackManifest {
    pub id: String,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
    pub optional: Vec<String>,
    pub permissions: Vec<String>,
    pub enabled_by_default: bool,
}

/// Intersection of enabled packs, what the agent supports, and what the
/// session allows. A missing requirement is reported. It does not panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveCapabilities {
    pub enabled: Vec<String>,
    pub degraded: Vec<String>,
}

pub fn effective_capabilities(
    packs: &[CapabilityPackManifest],
    enabled_ids: &[String],
    agent_supports: &[String],
) -> EffectiveCapabilities {
    let mut enabled = Vec::new();
    let mut degraded = Vec::new();
    for pack in packs {
        if !enabled_ids.iter().any(|id| id == &pack.id) && !pack.enabled_by_default {
            continue;
        }
        let missing = pack
            .requires
            .iter()
            .any(|need| !agent_supports.iter().any(|have| have == need));
        if missing {
            degraded.push(pack.id.clone());
        } else {
            for cap in &pack.provides {
                if agent_supports.iter().any(|have| have == cap) && !enabled.contains(cap) {
                    enabled.push(cap.clone());
                }
            }
        }
    }
    EffectiveCapabilities { enabled, degraded }
}

/// A typed reference. The projection stores this, not the file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub id: String,
    pub kind: String,
    pub generation: u64,
}

/// Viewer families the workbench can select without a per-type edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerFamily {
    Office,
    Data,
    Archive,
    Media,
    UnknownBinary,
    LargeFile,
    Pdf,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerRegistration {
    pub family: ViewerFamily,
    pub extensions: Vec<String>,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerRegistry {
    pub viewers: Vec<ViewerRegistration>,
}

impl ViewerRegistry {
    pub fn register(&mut self, viewer: ViewerRegistration) {
        self.viewers.push(viewer);
    }

    /// First registered viewer whose extension matches. Unknown extensions
    /// fall through to [`ViewerFamily::UnknownBinary`] when one is registered.
    pub fn select(&self, extension: &str) -> Option<&ViewerRegistration> {
        let ext = extension.trim_start_matches('.').to_ascii_lowercase();
        self.viewers
            .iter()
            .find(|viewer| {
                viewer
                    .extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&ext))
            })
            .or_else(|| {
                self.viewers
                    .iter()
                    .find(|viewer| viewer.family == ViewerFamily::UnknownBinary)
            })
    }
}

/// Capacity comes from the resolved route, not a global table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoute {
    pub provider: String,
    pub model: String,
    pub transport: String,
    pub context_capacity: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudget {
    pub max_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheBoundary {
    pub stable_prefix_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostLedger {
    pub reported_tokens: u64,
    pub estimated_tokens: u64,
}

/// Replace an oversized tool result with a head, a marker, and a tail, then
/// the caller measures again. This does not call a model.
pub fn prune_tool_result(text: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= head + tail + 16 {
        return text.to_string();
    }
    let head_s: String = chars.iter().take(head).collect();
    let tail_s: String = chars
        .iter()
        .skip(chars.len().saturating_sub(tail))
        .collect();
    format!(
        "{head_s}\n[pruned {omitted} chars]\n{tail_s}",
        omitted = chars.len() - head - tail
    )
}

/// The summary request keeps the same stable prefix bytes.
pub fn summary_request(stable_prefix: &str, dynamic_tail: &str) -> String {
    format!("{stable_prefix}\n<summary_request>\n{dynamic_tail}\n</summary_request>")
}

/// A third-party schema pinned for this session. A different live schema is
/// recorded for the next session and does not replace the pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedToolSchema {
    pub name: String,
    pub pinned_hash: String,
    pub next_session_hash: Option<String>,
}

impl PinnedToolSchema {
    pub fn observe_live(&mut self, live_hash: &str) {
        if live_hash != self.pinned_hash {
            self.next_session_hash = Some(live_hash.to_string());
        }
    }

    pub fn active_hash(&self) -> &str {
        &self.pinned_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorClause {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseOutcome {
    Compiled,
    Unenforceable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledClause {
    pub id: String,
    pub outcome: ClauseOutcome,
}

/// Compile a profile against the hooks this adapter actually exposes.
/// A clause with no hook is `unenforceable`. It is not dropped and it is
/// not reported as applied.
pub fn compile_behavior(
    clauses: &[BehaviorClause],
    exposed_hooks: &[String],
) -> Vec<CompiledClause> {
    clauses
        .iter()
        .map(|clause| CompiledClause {
            id: clause.id.clone(),
            outcome: if exposed_hooks.iter().any(|hook| hook == &clause.id) {
                ClauseOutcome::Compiled
            } else {
                ClauseOutcome::Unenforceable
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_chat_migrates_without_a_project() {
        let placed = migrate_session_without_space(
            SessionId::new("s"),
            WorkspaceId::new("w"),
            SpaceId::new("space"),
        );
        assert!(placed.project_id.is_none());
        assert_eq!(placed.space_id.as_str(), "space");
    }

    #[test]
    fn missing_pack_requirement_degrades() {
        let packs = vec![CapabilityPackManifest {
            id: "office".into(),
            provides: vec!["office.edit".into()],
            requires: vec!["office.edit".into()],
            optional: vec![],
            permissions: vec![],
            enabled_by_default: true,
        }];
        let effect = effective_capabilities(&packs, &[], &[]);
        assert!(effect.enabled.is_empty());
        assert_eq!(effect.degraded, vec!["office".to_string()]);
    }

    #[test]
    fn viewer_registry_selects_without_a_workbench_edit() {
        let mut registry = ViewerRegistry::default();
        registry.register(ViewerRegistration {
            family: ViewerFamily::Data,
            extensions: vec!["json".into(), "csv".into()],
            id: "data".into(),
        });
        registry.register(ViewerRegistration {
            family: ViewerFamily::UnknownBinary,
            extensions: vec![],
            id: "hex".into(),
        });
        assert_eq!(registry.select("csv").unwrap().id, "data");
        assert_eq!(
            registry.select("bin").unwrap().family,
            ViewerFamily::UnknownBinary
        );
    }

    #[test]
    fn prune_keeps_head_and_tail_and_summary_keeps_the_prefix() {
        let pruned = prune_tool_result(&"a".repeat(100), 4, 4);
        assert!(pruned.starts_with("aaaa"));
        assert!(pruned.ends_with("aaaa"));
        assert!(pruned.contains("pruned"));
        let request = summary_request("STABLE", "tail");
        assert!(request.starts_with("STABLE"));
    }

    #[test]
    fn a_changed_schema_waits_for_the_next_session() {
        let mut pin = PinnedToolSchema {
            name: "remote".into(),
            pinned_hash: "a".into(),
            next_session_hash: None,
        };
        pin.observe_live("b");
        assert_eq!(pin.active_hash(), "a");
        assert_eq!(pin.next_session_hash.as_deref(), Some("b"));
    }

    #[test]
    fn an_uncompilable_clause_is_unenforceable() {
        let clauses = vec![BehaviorClause {
            id: "verify_after_change".into(),
            text: "verify after every change".into(),
        }];
        let compiled = compile_behavior(&clauses, &[]);
        assert_eq!(compiled[0].outcome, ClauseOutcome::Unenforceable);
    }

    #[test]
    fn an_expired_bridge_cannot_be_treated_as_live() {
        let bridge = AgentBridgeRecord {
            bridge_id: "b".into(),
            binding_id: AgentBindingId::new("bind"),
            work_id: WorkId::new("work"),
            session_id: SessionId::new("session"),
            expires_at_ms: 10,
        };
        assert!(bridge.expired(10));
        assert!(!bridge.expired(9));
    }
}
