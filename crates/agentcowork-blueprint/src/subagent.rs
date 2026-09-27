//! Sub-agent orchestration (P6.2 — doc 16 Hermes `delegate_task` + doc 41
//! opencode `task.ts` + DeerFlow `subagent_limit_middleware`).
//!
//! The pure primitives for spawning isolated workers under a parent:
//!
//! - **Fresh context** — a sub-agent starts from its `TaskSpec` (own
//!   conversation, own workspace), never from the parent's transcript.
//! - **`DELEGATE_BLOCKED_TOOLS`** — the canonical toolset a parent withholds
//!   from children (`delegate`/`clarify`/`memory`/`send_message`/`cronjob`);
//!   sub-agents inherit *denies*, never escalated grants. `delegate` being
//!   blocked is also the no-recursive-spawn guard's first line.
//! - **Summary-only return** — the parent receives a [`SubAgentResult`]
//!   (summary + status + artifacts); the child's context is not replayed.
//! - **Limits** — max depth (no recursive spawn), max concurrent (batch
//!   parallel), max total per run — judged by [`DelegationPolicy::admit`]
//!   over the Work graph (I8), never accounted for by a runtime of its own.
//! - **Inter-agent messaging** — peer-review / cross-check / request-sub-
//!   routine / handoff, endpoint-validated.

use crate::blueprint::TaskStatus;
use crate::spec::TaskSpec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The root/parent pseudo-agent id (depth 0, owns delegation).
pub const ROOT_AGENT: &str = "root";

/// The tools a parent always withholds from sub-agents (Hermes
/// `DELEGATE_BLOCKED_TOOLS`). `delegate` first → a sub-agent cannot recurse;
/// `memory`/`send_message`/`cronjob` are parent-level privileges; `clarify`
/// is answered by the parent, not by the child asking the user.
pub const DELEGATE_BLOCKED_TOOLS: [&str; 5] =
    ["delegate", "clarify", "memory", "send_message", "cronjob"];

/// A sub-agent spawn request. Carries a scoped [`TaskSpec`] (the starting
/// context), a per-agent model, and an own workspace — not the parent's
/// conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentSpec {
    pub spec: TaskSpec,
    /// Per-agent model selection (opencode `task.ts`).
    pub model: String,
    /// Own workspace (isolated from the parent).
    pub workspace: String,
    /// Parent agent id (None ⇒ root-spawned, depth 0).
    pub parent_id: Option<String>,
    /// The toolset the parent grants (names).
    pub tools: Vec<String>,
    /// Extra denies beyond `DELEGATE_BLOCKED_TOOLS`.
    pub blocked_tools: Vec<String>,
    /// The depth the child Work lands on, as the caller read it from the Work
    /// graph (a root Work's child is 1). The graph is authoritative; this
    /// field is the spec's copy of the gauge's `child_depth`.
    #[serde(default)]
    pub depth: u32,
}

impl SubAgentSpec {
    pub fn new(spec: TaskSpec, model: impl Into<String>, workspace: impl Into<String>) -> Self {
        Self {
            spec,
            model: model.into(),
            workspace: workspace.into(),
            parent_id: None,
            tools: Vec::new(),
            blocked_tools: Vec::new(),
            depth: 0,
        }
    }

    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_blocked_tools(mut self, blocked: Vec<String>) -> Self {
        self.blocked_tools = blocked;
        self
    }

    /// The child's starting prompt: its spec only. This is the whole point of
    /// fresh-context spawn — the parent's history is never handed down.
    pub fn starting_prompt(&self) -> String {
        self.spec.to_markdown()
    }

    /// The effective toolset: parent grants minus explicit denies minus
    /// `DELEGATE_BLOCKED_TOOLS` (sub-agents inherit denies, not grants).
    pub fn effective_tools(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| !self.blocked_tools.contains(t))
            .filter(|t| !DELEGATE_BLOCKED_TOOLS.contains(&t.as_str()))
            .cloned()
            .collect()
    }
}

/// P36 (B3) — derived child permissions (Kilo default-deny pattern).
///
/// A child never blindly inherits the parent's tool universe: its toolset is
/// `parent grants ∩ (not parent denies) ∩ explicit grants`, and task/todo
/// bookkeeping tools are **default-deny** unless the parent explicitly grants
/// them. `explicit_grants` is the only way a default-denied tool appears.
pub fn derive_child_permissions(
    parent_grants: &[String],
    parent_denies: &[String],
    explicit_grants: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = parent_grants
        .iter()
        .filter(|t| !parent_denies.contains(t))
        .filter(|t| !DELEGATE_BLOCKED_TOOLS.contains(&t.as_str()))
        .filter(|t| explicit_grants.is_empty() || explicit_grants.contains(t))
        .filter(|t| !DEFAULT_DENY_TASK_TOOLS.contains(&t.as_str()) || explicit_grants.contains(t))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Kilo-style default-deny task bookkeeping: a child may not mutate the plan
/// ledger unless the parent hands it the tool explicitly.
pub const DEFAULT_DENY_TASK_TOOLS: [&str; 2] = ["task", "todo"];

/// P36 (B3) — why an inbuilt sub-agent stopped. Surface these as
/// termination events on the audit timeline (Gemini `LocalAgentExecutor`
/// pattern). External agents already have ACP `session/update` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminationReason {
    /// The goal/spec was met (normal completion).
    GoalMet,
    /// The per-agent timeout fired.
    Timeout,
    /// The max-turns iteration budget was exhausted.
    MaxTurns,
    /// Aborted by the parent/user.
    Aborted,
    /// The run failed (tool error, engine error).
    Error,
}

/// P36 (B3) — one termination event; every exit lands one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminationEvent {
    pub task_id: String,
    pub reason: TerminationReason,
    pub detail: Option<String>,
    pub at_step: u32,
}

/// P36 (B3) — the Scout child: a read-only sub-agent that clones its input
/// into a **managed cache** and never writes the user workspace. `read_only`
/// is enforced structurally — the struct has no write surface at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoutSpec {
    pub spec: TaskSpec,
    /// Where the clone lands (managed cache dir, never the user workspace).
    pub cache_root: String,
    pub model: String,
    #[serde(default)]
    pub depth: u32,
}

impl ScoutSpec {
    pub fn new(spec: TaskSpec, cache_root: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            spec,
            cache_root: cache_root.into(),
            model: model.into(),
            depth: 0,
        }
    }

    /// Scouts are structurally read-only: the returned read set is the only
    /// surface. There is no write method on this type.
    pub const fn read_only(&self) -> bool {
        true
    }
}

/// What the parent receives when a sub-agent finishes — summary + status +
/// artifacts. Structurally summary-only: there is no transcript field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub task_id: String,
    pub summary: String,
    pub status: TaskStatus,
    pub artifacts: Vec<String>,
}

/// Resource/depth limits (DeerFlow `subagent_limit_middleware` + Reasonix
/// asymmetric tiering). Defaults match P6.3's budget numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentLimits {
    /// Maximum sub-agent depth (root parent = 0). Children are 1; a child
    /// spawning below this is recursion and is rejected.
    pub max_depth: u32,
    /// Maximum simultaneously-running sub-agents (batch parallel cap).
    pub max_concurrent: u32,
    /// Maximum total spawns per run.
    pub max_total: u32,
}

impl Default for SubAgentLimits {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_concurrent: 3,
            max_total: 6,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SubAgentError {
    #[error("task {task_id} already spawned")]
    DuplicateTask { task_id: String },
    #[error("parent {parent_id:?} unknown for task {task_id}")]
    UnknownParent { task_id: String, parent_id: String },
    #[error("task {task_id} depth {depth} exceeds max_depth {max_depth} (no recursive spawn)")]
    DepthExceeded {
        task_id: String,
        depth: u32,
        max_depth: u32,
    },
    #[error("task {task_id} exceeds concurrent limit ({active} active of {max_concurrent})")]
    ConcurrentLimitExceeded {
        task_id: String,
        active: u32,
        max_concurrent: u32,
    },
    #[error("task {task_id} exceeds total-per-run limit ({total} of {max_total})")]
    TotalLimitExceeded {
        task_id: String,
        total: u32,
        max_total: u32,
    },
    #[error("task {task_id} not found")]
    UnknownTask { task_id: String },
    #[error("message references unknown agent {agent_id:?}")]
    UnknownAgent { agent_id: String },
    #[error(
        "agent {agent_id} is not admissible as a subagent: {readiness} ({reason}) — \
         installed is not ready"
    )]
    NotReady {
        task_id: String,
        agent_id: String,
        readiness: agentcowork_types::AgentReadiness,
        reason: &'static str,
    },
}

/// Inter-agent message kinds (P6.2 — peer-review, cross-check, request
/// sub-routines).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageKind {
    PeerReview,
    CrossCheck,
    RequestSubRoutine,
    Handoff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    pub kind: AgentMessageKind,
    pub body: String,
}

/// What the Work graph says about a delegation request (P71.3a).
///
/// The Work Gateway computes these from the one graph — the child's depth is
/// its parent's link-distance from the root plus one, `active`/`total` are
/// the delegated child Works (any parent) that are not / are terminal — so
/// the policy below has nothing to remember and a restart cannot lose or
/// invent an admission. `active`/`total` are `0` when the caller names no
/// parent Work; that is also the case where only depth can be judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DelegationGauge {
    /// Depth the child Work lands on: parent's link-distance from the root
    /// plus one (a root Work's child is depth 1).
    pub child_depth: u32,
    /// Delegated child Works (any parent) that are not terminal yet.
    pub active: u32,
    /// Delegated child Works (any parent) ever created.
    pub total: u32,
}

/// The spawn **policy** — no executor, no registry, no accumulated state.
///
/// I8 states subagents are child Work/Runs: the durable record of a
/// delegation is the child Work + Run + ephemeral AgentSession the Work
/// Gateway mints, so nothing here may become a second source of truth for
/// which children exist. What remains is the deterministic judgement —
/// depth, concurrency, total — applied to a [`DelegationGauge`] the caller
/// read from the graph. The executor that used to live here (and its
/// `HashMap` of active agents) is deleted: the coordinator runs the LLM turn,
/// the gateway holds the state, and this type only says yes or no.
///
/// A duplicate is refused by the caller from the graph, not here: child Work
/// ids are deterministic (`<parent>/subagent/<task_id>`), so "already
/// spawned" is a lookup, not a remembered set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegationPolicy {
    limits: SubAgentLimits,
}

impl DelegationPolicy {
    pub fn new(limits: SubAgentLimits) -> Self {
        Self { limits }
    }

    pub fn limits(&self) -> SubAgentLimits {
        self.limits
    }

    /// The spawn admission, in one fixed order: **readiness** → depth
    /// (recursion) → concurrent → total. Every arm carries what it judged, so a
    /// refusal is auditable without reading any state back.
    ///
    /// Readiness is judged first and takes a [`agentcowork_types::AgentReadiness`]
    /// value the caller read from the one source (`P71.3f`) — never a boolean
    /// the caller derived. Only [`agentcowork_types::AgentReadiness::Ready`] is
    /// admissible: an installed-but-unauthenticated agent, a degraded agent and
    /// an unprobed agent are all refused, each naming its own state.
    pub fn admit(
        &self,
        task_id: &str,
        gauge: DelegationGauge,
        member_agent_id: &str,
        readiness: agentcowork_types::AgentReadiness,
    ) -> Result<(), SubAgentError> {
        if !readiness.can_delegate() {
            return Err(SubAgentError::NotReady {
                task_id: task_id.to_string(),
                agent_id: member_agent_id.to_string(),
                readiness,
                reason: readiness.summary(),
            });
        }
        if gauge.child_depth > self.limits.max_depth {
            return Err(SubAgentError::DepthExceeded {
                task_id: task_id.to_string(),
                depth: gauge.child_depth,
                max_depth: self.limits.max_depth,
            });
        }
        if gauge.active >= self.limits.max_concurrent {
            return Err(SubAgentError::ConcurrentLimitExceeded {
                task_id: task_id.to_string(),
                active: gauge.active,
                max_concurrent: self.limits.max_concurrent,
            });
        }
        if gauge.total >= self.limits.max_total {
            return Err(SubAgentError::TotalLimitExceeded {
                task_id: task_id.to_string(),
                total: gauge.total,
                max_total: self.limits.max_total,
            });
        }
        Ok(())
    }
}

/// What a parent ever sees of a child (WORK §8): summary + artifacts, never
/// the transcript. The durable record is the child Work's own timeline; this
/// projection is the single shape both delegation seams return to the parent,
/// so "summary-only" stays a rule with one implementation.
///
/// Cancellation is deliberately absent: a cancelled child produces no
/// summary at all — the truth is the child Run's `RunCancelled` event.
pub fn parent_view(result: &SubAgentResult) -> serde_json::Value {
    serde_json::json!({
        "taskId": result.task_id,
        "summary": result.summary,
        "status": result.status,
        "artifacts": result.artifacts,
    })
}

/// Validate an inter-agent message's endpoints (P6.2). Membership is supplied
/// by the caller from the Work graph — the delegating agent plus the live
/// child agent sessions — so the check cannot drift from what actually
/// exists, and no registry is kept here. Delivery stays the coordinator's job.
pub fn validate_message_endpoints(
    msg: &AgentMessage,
    is_known: impl Fn(&str) -> bool,
) -> Result<(), SubAgentError> {
    if !is_known(&msg.from) {
        return Err(SubAgentError::UnknownAgent {
            agent_id: msg.from.clone(),
        });
    }
    if !is_known(&msg.to) {
        return Err(SubAgentError::UnknownAgent {
            agent_id: msg.to.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, model: &str) -> SubAgentSpec {
        SubAgentSpec::new(
            TaskSpec::new(id, format!("goal {id}")),
            model,
            format!("/ws/{id}"),
        )
    }

    #[test]
    fn delegate_blocked_tools_are_denied_by_default() {
        let s = spec("a", "m").with_tools(vec![
            "read".into(),
            "write".into(),
            "delegate".into(),
            "memory".into(),
            "cronjob".into(),
        ]);
        let effective = s.effective_tools();
        assert_eq!(effective, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn explicit_blocked_tools_also_denied() {
        let s = spec("a", "m")
            .with_tools(vec!["read".into(), "write".into()])
            .with_blocked_tools(vec!["write".into()]);
        assert_eq!(s.effective_tools(), vec!["read".to_string()]);
    }

    #[test]
    fn fresh_context_is_spec_only() {
        let s = spec("a", "m").with_tools(vec![]);
        let prompt = s.starting_prompt();
        assert!(prompt.contains("# Task: a"));
        assert!(prompt.contains("goal a"));
        // Never the parent history.
        assert!(!prompt.contains("parent history"));
    }

    fn gauge(child_depth: u32, active: u32, total: u32) -> DelegationGauge {
        DelegationGauge {
            child_depth,
            active,
            total,
        }
    }

    /// The only admissible state (P71.3f).
    fn readiness() -> agentcowork_types::AgentReadiness {
        agentcowork_types::AgentReadiness::Ready
    }

    #[test]
    fn readiness_is_judged_first_and_installed_is_not_ready() {
        use agentcowork_types::AgentReadiness;
        let policy = DelegationPolicy::new(SubAgentLimits::default());
        // Every limit is fine, but the agent is not deployable: the readiness
        // arm answers — installed is not ready, and degraded is not delegable.
        for state in [
            AgentReadiness::Unknown,
            AgentReadiness::Discovered,
            AgentReadiness::Installed,
            AgentReadiness::Launchable,
            AgentReadiness::ProtocolCompatible,
            AgentReadiness::AuthRequired,
            AgentReadiness::Authenticating,
            AgentReadiness::Degraded,
            AgentReadiness::Unavailable,
            AgentReadiness::Failed,
        ] {
            match policy.admit("t", gauge(1, 0, 0), "coder", state) {
                Err(SubAgentError::NotReady {
                    agent_id,
                    readiness,
                    reason,
                    ..
                }) => {
                    assert_eq!(agent_id, "coder");
                    assert_eq!(readiness, state);
                    assert!(!reason.is_empty());
                }
                other => panic!("{state:?} must not be admissible as a subagent: {other:?}"),
            }
        }
        // The readiness arm precedes depth: an unready agent is refused for
        // readiness even when every limit is also exceeded.
        assert!(matches!(
            policy.admit("t", gauge(9, 9, 9), "coder", AgentReadiness::Installed),
            Err(SubAgentError::NotReady { .. })
        ));
        assert!(
            policy
                .admit("t", gauge(1, 0, 0), "coder", AgentReadiness::Ready)
                .is_ok()
        );
    }

    #[test]
    fn admission_judges_the_depth_the_graph_reports() {
        let policy = DelegationPolicy::new(SubAgentLimits::default());
        // A root Work's child is depth 1; its grandchild is depth 2 (== max).
        assert!(
            policy
                .admit("child", gauge(1, 0, 0), "member", readiness())
                .is_ok()
        );
        assert!(
            policy
                .admit("grandchild", gauge(2, 1, 1), "member", readiness())
                .is_ok()
        );
        // Depth 3 would be recursion — refused with the numbers it judged.
        assert!(matches!(
            policy.admit("great-grandchild", gauge(3, 0, 0), "member", readiness()),
            Err(SubAgentError::DepthExceeded {
                depth: 3,
                max_depth: 2,
                ..
            })
        ));
    }

    #[test]
    fn child_permissions_intersect_grants_and_inherit_denies() {
        // P64.4 — the derivation the spawn seam applies: grants intersect,
        // denies are inherited, `delegate` never lands, and `todo` stays
        // default-deny unless the spec itself re-grants it.
        let parent_grants = vec![
            "read".to_string(),
            "write".to_string(),
            "delegate".to_string(),
            "todo".to_string(),
        ];
        let denies = vec!["write".to_string()];
        let derived = derive_child_permissions(&parent_grants, &denies, &parent_grants);
        assert_eq!(derived, vec!["read".to_string(), "todo".to_string()]);
        assert!(
            !derived.iter().any(|t| t == "delegate"),
            "DELEGATE_BLOCKED_TOOLS never land on a child"
        );
        // A child that lists fewer tools gets the intersection, not a superset.
        let child = derive_child_permissions(
            &parent_grants,
            &denies,
            &["read".to_string(), "write".to_string()],
        );
        assert_eq!(child, vec!["read".to_string()]);
    }

    #[test]
    fn a_stricter_max_depth_refuses_a_grandchild() {
        let policy = DelegationPolicy::new(SubAgentLimits {
            max_depth: 1,
            max_concurrent: 4,
            max_total: 10,
        });
        assert!(
            policy
                .admit("child", gauge(1, 0, 0), "member", readiness())
                .is_ok()
        );
        assert!(matches!(
            policy.admit("grandchild", gauge(2, 1, 1), "member", readiness()),
            Err(SubAgentError::DepthExceeded {
                depth: 2,
                max_depth: 1,
                ..
            })
        ));
    }

    #[test]
    fn concurrent_and_total_limits_come_from_the_graph() {
        let policy = DelegationPolicy::new(SubAgentLimits {
            max_depth: 2,
            max_concurrent: 2,
            max_total: 3,
        });
        // Two children running → the concurrent cap is what binds.
        assert!(matches!(
            policy.admit("c", gauge(1, 2, 2), "member", readiness()),
            Err(SubAgentError::ConcurrentLimitExceeded {
                active: 2,
                max_concurrent: 2,
                ..
            })
        ));
        // A finished child frees concurrency, so the *total* cap binds next
        // (three ever-created children, one of them terminal).
        assert!(matches!(
            policy.admit("d", gauge(1, 1, 3), "member", readiness()),
            Err(SubAgentError::TotalLimitExceeded {
                total: 3,
                max_total: 3,
                ..
            })
        ));
    }

    #[test]
    fn admission_order_is_depth_then_concurrent_then_total() {
        let policy = DelegationPolicy::new(SubAgentLimits {
            max_depth: 1,
            max_concurrent: 1,
            max_total: 1,
        });
        // All three would be exceeded → the depth arm answers.
        assert!(matches!(
            policy.admit("t", gauge(2, 5, 9), "member", readiness()),
            Err(SubAgentError::DepthExceeded { .. })
        ));
        // Depth fine → the concurrent arm answers.
        assert!(matches!(
            policy.admit("t", gauge(1, 1, 9), "member", readiness()),
            Err(SubAgentError::ConcurrentLimitExceeded { .. })
        ));
        // Depth and concurrency fine → the total arm answers.
        assert!(matches!(
            policy.admit("t", gauge(1, 0, 1), "member", readiness()),
            Err(SubAgentError::TotalLimitExceeded { .. })
        ));
        // Nothing exceeded → admitted.
        assert!(
            policy
                .admit("t", gauge(1, 0, 0), "member", readiness())
                .is_ok()
        );
    }

    #[test]
    fn parent_view_carries_summary_and_artifacts_only() {
        let result = SubAgentResult {
            task_id: "coder".into(),
            summary: "wrote the summary".into(),
            status: TaskStatus::Done,
            artifacts: vec!["out.md".into()],
        };
        let view = parent_view(&result);
        assert_eq!(view["summary"], "wrote the summary");
        assert_eq!(view["artifacts"][0], "out.md");
        // Summary-only by construction: four keys, none of them the
        // child's transcript or history.
        assert_eq!(view.as_object().unwrap().len(), 4);
        assert!(view.get("transcript").is_none());
        assert!(view.get("history").is_none());
    }

    #[test]
    fn inter_agent_messaging_validates_endpoints_against_the_graph() {
        // Membership comes from the caller (the Work graph), so the check is
        // the same membership the graph would report.
        let known = |id: &str| matches!(id, "a" | "b");

        let ok = AgentMessage {
            from: "a".into(),
            to: "b".into(),
            kind: AgentMessageKind::PeerReview,
            body: "review this".into(),
        };
        assert!(validate_message_endpoints(&ok, known).is_ok());

        let bad = AgentMessage {
            from: "a".into(),
            to: "ghost".into(),
            kind: AgentMessageKind::CrossCheck,
            body: "?".into(),
        };
        assert!(matches!(
            validate_message_endpoints(&bad, known),
            Err(SubAgentError::UnknownAgent { .. })
        ));
    }

    #[test]
    fn fan_out_stops_at_the_concurrent_limit() {
        let policy = DelegationPolicy::new(SubAgentLimits {
            max_depth: 2,
            max_concurrent: 2,
            max_total: 10,
        });
        // Each admitted spawn becomes a child Work, so the gauge the graph
        // reports for the next candidate has one more active child.
        let mut active = 0u32;
        let mut spawned = Vec::new();
        for id in ["a", "b", "c"] {
            match policy.admit(id, gauge(1, active, active), "member", readiness()) {
                Ok(()) => {
                    spawned.push(id.to_string());
                    active += 1;
                }
                Err(e) => {
                    assert!(matches!(e, SubAgentError::ConcurrentLimitExceeded { .. }));
                    break;
                }
            }
        }
        // max_concurrent = 2 → a and b spawn; c hits the concurrent cap.
        assert_eq!(spawned, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn two_spec_driven_agents_different_models_run_a_plan() {
        // P6.2 exit-criterion simulation: a planner (model X) delegates to a
        // coder (model Y) on a verify-gated plan. The Work graph supplies the
        // gauge, the policy judges it, and the planner receives the
        // summary-only projection — never the coder's transcript.
        let policy = DelegationPolicy::new(SubAgentLimits::default());

        let planner = spec("planner", "claude-sonnet");
        let coder = spec("coder", "gpt-5-codex").with_parent("planner");
        // The planner is a root Work's child (depth 1); the coder is depth 2.
        assert!(
            policy
                .admit(&planner.spec.id, gauge(1, 0, 0), "planner", readiness())
                .is_ok()
        );
        assert!(
            policy
                .admit(&coder.spec.id, gauge(2, 1, 1), "coder", readiness())
                .is_ok()
        );

        // Per-agent models are a property of the spec, not of any runtime.
        assert_eq!(planner.model, "claude-sonnet");
        assert_eq!(coder.model, "gpt-5-codex");

        // The coder's derived toolset strips `delegate` → cannot recurse.
        let parent_grants = vec![
            "read".to_string(),
            "write".to_string(),
            "delegate".to_string(),
        ];
        let coder_tools = derive_child_permissions(&parent_grants, &[], &parent_grants);
        assert_eq!(coder_tools, vec!["read".to_string(), "write".to_string()]);

        // The coder finishes → the planner receives the summary-only view.
        let result = SubAgentResult {
            task_id: "coder".into(),
            summary: "implemented /health".into(),
            status: TaskStatus::Done,
            artifacts: vec!["src".into()],
        };
        let view = parent_view(&result);
        assert_eq!(view["summary"], "implemented /health");
        assert_eq!(view["status"], "done");
        assert_eq!(view["taskId"], "coder");
    }
}
