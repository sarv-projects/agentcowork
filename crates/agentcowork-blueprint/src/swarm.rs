//! Swarm strategy — pure classification over finished runs (I9).
//!
//! P71.3b re-homing (ADR-0005 "Multiagent survives only if re-homed"):
//! the old `SwarmSession` driver was an execution kernel — a mutable
//! session accumulating `report()` calls with a private verdict state
//! machine. That is exactly what **I9** forbids ("MultiRun is a strategy
//! over Runs, not an execution kernel"). The module now keeps only the
//! strategy contract and a **pure** reduction:
//!
//! - [`SwarmSpec`] — one prompt, N agents, a [`SwarmMode`]. Data only.
//! - [`reduce`] — total function from `(spec, results)` to a
//!   [`SwarmVerdict`]. No state, no I/O, no effect. The caller (the
//!   delegation plane / MultiRun strategy, P71.3e) owns execution: it
//!   spawns one child Work per member via the P71.1 delegation façade,
//!   collects the finished [`MemberResult`]s, and calls `reduce` — once
//!   when complete, or early to circuit-break (an incomplete verdict is
//!   `complete == false`, `winner == None`, the old `abort()` outcome).
//!
//! Member results come from child Work (`SubAgentResult.summary`,
//! `TaskStatus`), not from an ad-hoc reporting channel.
//!
//! Federation (cross-machine sync) remains *data only* — a
//! [`FederationSpec`] describing the remote peer + channels; the live
//! transport is the H18 remote/mobile seam, never claimed here.

use serde::{Deserialize, Serialize};

/// How the swarm merges N answers to one prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SwarmMode {
    /// First healthy answer wins (latency-optimized).
    Race,
    /// Majority agreement required; ties fall back to the best-scored.
    Consensus,
    /// Every completion is kept and merged into a structured digest.
    Ensemble,
}

/// The swarm contract: one prompt, N agents, a mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmSpec {
    pub name: String,
    /// The shared prompt every member runs.
    pub prompt: String,
    /// The member agent ids (fleet members must already exist — the swarm
    /// never fabricates an agent).
    pub agents: Vec<String>,
    pub mode: SwarmMode,
    /// Optional maximum rounds for reconcile (ensemble synthesis).
    #[serde(default = "default_rounds")]
    pub max_rounds: u32,
}

fn default_rounds() -> u32 {
    2
}

impl SwarmSpec {
    pub fn new(
        name: impl Into<String>,
        prompt: impl Into<String>,
        agents: Vec<String>,
        mode: SwarmMode,
    ) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            agents,
            mode,
            max_rounds: default_rounds(),
        }
    }

    /// Reject empty prompts / empty or duplicate members.
    pub fn validate(&self) -> Result<(), SwarmError> {
        if self.prompt.trim().is_empty() {
            return Err(SwarmError::EmptyPrompt);
        }
        if self.agents.is_empty() {
            return Err(SwarmError::NoMembers);
        }
        let mut seen = std::collections::HashSet::new();
        for a in &self.agents {
            if !seen.insert(a.as_str()) {
                return Err(SwarmError::DuplicateMember(a.clone()));
            }
        }
        Ok(())
    }
}

/// One member's answer as the caller collected it from a finished child
/// Work (P71.3a delegation plane): `summary` is `SubAgentResult.summary`,
/// `ok` is "the child reached `TaskStatus::Completed`", and `task_id` is
/// the replay key of the child Work the answer came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberResult {
    /// Which fleet member produced this (must be in `SwarmSpec.agents`;
    /// entries outside the spec are excluded from the verdict).
    pub agent_id: String,
    /// The child Work id this answer came from.
    pub task_id: String,
    /// Short answer/outcome text (what the member returned).
    pub summary: String,
    /// 0.0..=1.0 score if the harness reports one (else 0.5).
    pub score: f64,
    /// true = completed cleanly (a failure is never counted toward
    /// consensus, the race, or the digest).
    pub ok: bool,
}

/// The verdict — computed, never stored. The caller persists it (e.g. in
/// the Work record) if it needs to outlive the call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwarmVerdict {
    /// Members that have not yet reported (spec.agents minus results).
    pub pending: Vec<String>,
    /// True when every member has reported.
    pub complete: bool,
    /// Set only when `complete` (Race: first healthy; Consensus: majority,
    /// tie → best score; Ensemble: first healthy). `None` otherwise — the
    /// early/circuit-broken outcome.
    pub winner: Option<String>,
    /// Ensemble digest (`agent_id: summary` per healthy member), only when
    /// `complete` and mode is Ensemble.
    pub digest: Vec<String>,
}

/// Who may start now, and who waits for a free slot. This is a plan over
/// child Work, not a swarm runtime: nothing here spawns a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParallelFanOut {
    pub start_now: Vec<String>,
    pub deferred: Vec<String>,
    pub refused: Vec<(String, String)>,
}

/// P53.9 — admit installed subagents in parallel up to the existing
/// concurrency cap. Overflow is deferred, not a second executor. An agent
/// that is not ready is refused by name and does not take a slot.
pub fn plan_parallel_fan_out(
    members: &[(String, agentcowork_types::AgentReadiness)],
    policy: &crate::subagent::DelegationPolicy,
    mut gauge: crate::subagent::DelegationGauge,
) -> Result<ParallelFanOut, crate::subagent::SubAgentError> {
    let mut plan = ParallelFanOut {
        start_now: Vec::new(),
        deferred: Vec::new(),
        refused: Vec::new(),
    };
    for (agent_id, readiness) in members {
        if !readiness.can_delegate() {
            plan.refused
                .push((agent_id.clone(), readiness.summary().to_string()));
            continue;
        }
        match policy.admit(agent_id, gauge, agent_id, *readiness) {
            Ok(()) => {
                plan.start_now.push(agent_id.clone());
                gauge.active = gauge.active.saturating_add(1);
                gauge.total = gauge.total.saturating_add(1);
            }
            Err(crate::subagent::SubAgentError::ConcurrentLimitExceeded { .. })
            | Err(crate::subagent::SubAgentError::TotalLimitExceeded { .. }) => {
                plan.deferred.push(agent_id.clone());
            }
            Err(other) => return Err(other),
        }
    }
    Ok(plan)
}

/// Pure reduction: classify `(spec, results)` into a [`SwarmVerdict`].
///
/// Total — never fails. Duplicate entries for one member keep the **first**
/// (the old first-report-wins rule); results whose `agent_id` is not in the
/// spec are excluded. Verdict semantics are evaluated only when every member
/// has reported; a partial result set yields `complete == false`,
/// `winner == None`, empty digest (the caller may call early to abort).
pub fn reduce(spec: &SwarmSpec, results: &[MemberResult]) -> SwarmVerdict {
    // First result per member wins; unknown members are excluded.
    let mut reported: Vec<&MemberResult> = Vec::with_capacity(results.len());
    for r in results {
        if !spec.agents.contains(&r.agent_id) {
            continue;
        }
        if !reported.iter().any(|m| m.agent_id == r.agent_id) {
            reported.push(r);
        }
    }

    let pending: Vec<String> = spec
        .agents
        .iter()
        .filter(|a| !reported.iter().any(|m| &m.agent_id == *a))
        .cloned()
        .collect();
    let complete = pending.is_empty();
    if !complete {
        return SwarmVerdict {
            pending,
            complete: false,
            winner: None,
            digest: Vec::new(),
        };
    }

    let healthy: Vec<&MemberResult> = reported.iter().filter(|m| m.ok).copied().collect();
    let winner = match spec.mode {
        SwarmMode::Race | SwarmMode::Ensemble => {
            // first reported healthy answer
            healthy.first().map(|m| m.agent_id.clone())
        }
        SwarmMode::Consensus => {
            // majority answer (exact text); tie → highest score
            let mut counts: std::collections::HashMap<&str, (usize, f64)> = Default::default();
            for m in &healthy {
                let e = counts.entry(m.summary.as_str()).or_insert((0, 0.0));
                e.0 += 1;
                e.1 = e.1.max(m.score);
            }
            counts
                .into_iter()
                .max_by(|a, b| {
                    a.1.0.cmp(&b.1.0).then_with(|| {
                        a.1.1
                            .partial_cmp(&b.1.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                })
                .and_then(|(summary, _)| {
                    healthy
                        .iter()
                        .find(|m| m.summary == summary)
                        .map(|m| m.agent_id.clone())
                })
        }
    };
    let digest = if spec.mode == SwarmMode::Ensemble {
        healthy
            .iter()
            .map(|m| format!("{}: {}", m.agent_id, m.summary))
            .collect()
    } else {
        Vec::new()
    };

    SwarmVerdict {
        pending,
        complete: true,
        winner,
        digest,
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SwarmError {
    #[error("swarm prompt is empty")]
    EmptyPrompt,
    #[error("swarm has no members")]
    NoMembers,
    #[error("duplicate swarm member `{0}`")]
    DuplicateMember(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(agent: &str, task: &str, summary: &str, score: f64, ok: bool) -> MemberResult {
        MemberResult {
            agent_id: agent.into(),
            task_id: task.into(),
            summary: summary.into(),
            score,
            ok,
        }
    }

    /// All three members healthy, in slice order (alice, bob, carol).
    fn all_healthy() -> Vec<MemberResult> {
        vec![
            result("alice", "w-alice", "a", 0.6, true),
            result("bob", "w-bob", "b", 0.9, true),
            result("carol", "w-carol", "c", 0.8, true),
        ]
    }

    fn spec(mode: SwarmMode) -> SwarmSpec {
        SwarmSpec::new(
            "swarm-1",
            "Summarize the Q3 variance.",
            vec!["alice".into(), "bob".into(), "carol".into()],
            mode,
        )
    }

    #[test]
    fn parallel_fan_out_stops_at_the_concurrency_cap() {
        use crate::subagent::{DelegationGauge, DelegationPolicy, SubAgentLimits};
        let policy = DelegationPolicy::new(SubAgentLimits {
            max_depth: 2,
            max_concurrent: 2,
            max_total: 10,
        });
        let members = vec![
            ("a".into(), agentcowork_types::AgentReadiness::Ready),
            ("b".into(), agentcowork_types::AgentReadiness::Ready),
            ("c".into(), agentcowork_types::AgentReadiness::Ready),
            ("d".into(), agentcowork_types::AgentReadiness::Installed),
        ];
        let plan = plan_parallel_fan_out(
            &members,
            &policy,
            DelegationGauge {
                child_depth: 1,
                ..DelegationGauge::default()
            },
        )
        .unwrap();
        assert_eq!(plan.start_now, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(plan.deferred, vec!["c".to_string()]);
        assert_eq!(plan.refused.len(), 1);
        assert_eq!(plan.refused[0].0, "d");
    }

    #[test]
    fn spec_validation() {
        assert!(
            SwarmSpec::new("x", "", vec![], SwarmMode::Race)
                .validate()
                .is_err()
        );
        assert!(
            SwarmSpec::new("x", "p", vec![], SwarmMode::Race)
                .validate()
                .is_err()
        );
        assert!(
            SwarmSpec::new("x", "p", vec!["a".into(), "a".into()], SwarmMode::Race)
                .validate()
                .is_err()
        );
        assert!(
            SwarmSpec::new("x", "p", vec!["a".into()], SwarmMode::Race)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn race_picks_first_healthy() {
        let v = reduce(&spec(SwarmMode::Race), &all_healthy());
        assert!(v.complete);
        assert!(v.pending.is_empty());
        assert_eq!(v.winner.as_deref(), Some("alice")); // first in slice
        assert!(v.digest.is_empty());
    }

    #[test]
    fn race_ignores_failed_members() {
        let rs = vec![
            result("alice", "w-alice", "fail", 0.0, false),
            result("bob", "w-bob", "b", 0.7, true),
            result("carol", "w-carol", "c", 0.5, false),
        ];
        let v = reduce(&spec(SwarmMode::Race), &rs);
        assert!(v.complete);
        assert_eq!(v.winner.as_deref(), Some("bob")); // first healthy
    }

    #[test]
    fn consensus_takes_majority() {
        let rs = vec![
            result("alice", "w-alice", "42", 0.5, true),
            result("bob", "w-bob", "42", 0.7, true),
            result("carol", "w-carol", "43", 0.9, true),
        ];
        let v = reduce(&spec(SwarmMode::Consensus), &rs);
        assert!(v.complete);
        assert_eq!(v.winner.as_deref(), Some("alice"));
    }

    #[test]
    fn consensus_tie_goes_to_score() {
        let rs = vec![
            result("alice", "w-alice", "42", 0.5, true),
            result("bob", "w-bob", "43", 0.9, true),
            result("carol", "w-carol", "44", 0.3, true),
        ];
        let v = reduce(&spec(SwarmMode::Consensus), &rs);
        assert!(v.complete);
        assert_eq!(v.winner.as_deref(), Some("bob"));
    }

    #[test]
    fn ensemble_builds_digest() {
        let rs = vec![
            result("alice", "w-alice", "a1", 0.5, true),
            result("bob", "w-bob", "b2", 0.5, true),
            result("carol", "w-carol", "x", 0.0, false),
        ];
        let v = reduce(&spec(SwarmMode::Ensemble), &rs);
        assert!(v.complete);
        assert_eq!(v.digest.len(), 2);
        assert_eq!(v.digest[0], "alice: a1");
    }

    #[test]
    fn partial_results_are_not_a_verdict() {
        let rs = vec![result("carol", "w-carol", "c", 0.8, true)];
        let v = reduce(&spec(SwarmMode::Race), &rs);
        assert!(!v.complete);
        assert_eq!(v.pending, vec!["alice".to_string(), "bob".to_string()]);
        assert_eq!(v.winner, None);
        assert!(v.digest.is_empty());
    }

    #[test]
    fn unknown_members_and_duplicates_excluded() {
        let rs = vec![
            result("mallory", "w-mallory", "evil", 1.0, true), // not in spec
            result("alice", "w-alice-2", "second", 0.9, true), // duplicate — first wins
            result("alice", "w-alice-1", "first", 0.5, true),
            result("bob", "w-bob", "b", 0.5, true),
            result("carol", "w-carol", "c", 0.5, true),
        ];
        let v = reduce(&spec(SwarmMode::Race), &rs);
        assert!(v.complete);
        assert_eq!(v.winner.as_deref(), Some("alice"));
        // "first" (the earlier entry) won; mallory never entered.
    }
}
