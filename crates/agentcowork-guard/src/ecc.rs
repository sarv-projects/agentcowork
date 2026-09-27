//! ECC guardrails (P7.1 — I5, doc 46 ECC profile-gated hooks): the two
//! deterministic policy checks that keep the forge loop from becoming a
//! drive-by mutation machine.
//!
//! - **Plan-before-build** — a mutation may not start until the session holds
//!   a plan that names it (no unplanned edits).
//! - **Session scanning** — a session's event trail is scanned for
//!   guardrail-relevant patterns (unplanned mutations, repeated destructive
//!   ops, estop-adjacent actions, runaway loops) and each finding is
//!   surfaced with a severity so the harness can stop/ask.
//!
//! The policy is data, not code: thresholds live in [`EccPolicy`] (feeds
//! `permissions.toml`-style configuration); the checks are pure functions.

use serde::{Deserialize, Serialize};

/// One normalized session event the scanner understands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Event kind, e.g. `plan`, `build`, `ticket_approved`, `destructive`,
    /// `test`, `verify`, `commit`, `estop`, `rewrite`.
    pub kind: String,
    /// Whether the event succeeded (default true for neutral events).
    #[serde(default = "default_true")]
    pub ok: bool,
    /// Free-form detail (operation name, paths, command, …).
    #[serde(default)]
    pub detail: String,
}

fn default_true() -> bool {
    true
}

/// Severity of a guardrail finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EccSeverity {
    Info,
    Warning,
    Blocking,
}

/// One guardrail finding from a session scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EccFinding {
    pub severity: EccSeverity,
    pub message: String,
    /// Index of the event that produced the finding (or `None` for
    /// aggregate findings).
    #[serde(default)]
    pub event_index: Option<usize>,
}

/// The guardrail policy. Defaults are conservative; the host can relax them
/// through configuration (still never below the hard floors).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EccPolicy {
    /// A build/mutation event is a violation unless a `plan` event preceded
    /// it in the session.
    pub require_plan_before_build: bool,
    /// Max destructive-operation events per session before blocking.
    pub max_destructive_per_session: u32,
    /// Max failed `rewrite` iterations in a row before the loop is flagged.
    pub max_consecutive_rewrites: u32,
    /// Operation kinds treated as destructive for the cap above.
    #[serde(default)]
    pub destructive_kinds: Vec<String>,
}

impl Default for EccPolicy {
    fn default() -> Self {
        Self {
            require_plan_before_build: true,
            max_destructive_per_session: 3,
            max_consecutive_rewrites: 5,
            destructive_kinds: vec![
                "delete".into(),
                "rm".into(),
                "overwrite".into(),
                "drop".into(),
                "format".into(),
            ],
        }
    }
}

/// Verdict of a plan-before-build check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EccVerdict {
    Allowed,
    /// A mutation happened with no plan in the session.
    RequiresPlan,
    /// The session exceeded a hard cap (destructive ops).
    Blocked(String),
}

/// Check whether a `build`-class mutation may proceed given the session's
/// event history. `event_kind` is the kind of the proposed action
/// (`build`, `delete`, `overwrite`, …).
pub fn plan_before_build(
    policy: &EccPolicy,
    events: &[SessionEvent],
    event_kind: &str,
) -> EccVerdict {
    // Hard cap on destructive ops first (independent of the plan rule).
    if policy.destructive_kinds.iter().any(|k| k == event_kind) {
        let destructive_count = events
            .iter()
            .filter(|e| policy.destructive_kinds.iter().any(|k| k == &e.kind))
            .count();
        if destructive_count >= policy.max_destructive_per_session as usize {
            return EccVerdict::Blocked(format!(
                "destructive-op cap reached ({destructive_count} ≥ {})",
                policy.max_destructive_per_session
            ));
        }
    }
    if policy.require_plan_before_build && !events.iter().any(|e| e.kind == "plan") {
        return EccVerdict::RequiresPlan;
    }
    EccVerdict::Allowed
}

/// Scan a full session trail for guardrail findings. Aggregate + per-event
/// findings are returned sorted by severity (blocking first).
pub fn session_scan(policy: &EccPolicy, events: &[SessionEvent]) -> Vec<EccFinding> {
    let mut findings = Vec::new();

    // Unplanned mutations (plan-before-build over the whole trail).
    if policy.require_plan_before_build {
        let has_plan = events.iter().any(|e| e.kind == "plan");
        if !has_plan {
            let mutations: Vec<usize> = events
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    matches!(
                        e.kind.as_str(),
                        "build" | "delete" | "overwrite" | "drop" | "format" | "commit"
                    )
                })
                .map(|(i, _)| i)
                .collect();
            if !mutations.is_empty() {
                findings.push(EccFinding {
                    severity: EccSeverity::Blocking,
                    message: format!(
                        "session has {} mutation(s) with no plan event",
                        mutations.len()
                    ),
                    event_index: mutations.first().copied(),
                });
            }
        }
    }

    // Destructive-op cap.
    let destructive_count = events
        .iter()
        .filter(|e| policy.destructive_kinds.iter().any(|k| k == &e.kind))
        .count();
    if destructive_count > policy.max_destructive_per_session as usize {
        findings.push(EccFinding {
            severity: EccSeverity::Blocking,
            message: format!(
                "destructive ops {destructive_count} exceed session cap {}",
                policy.max_destructive_per_session
            ),
            event_index: None,
        });
    }

    // Failed rewrite streak (runaway loop).
    let mut streak = 0u32;
    for (i, e) in events.iter().enumerate() {
        if e.kind == "rewrite" && !e.ok {
            streak += 1;
            if streak > policy.max_consecutive_rewrites {
                findings.push(EccFinding {
                    severity: EccSeverity::Warning,
                    message: format!(
                        "runaway rewrite loop: {streak} consecutive failed rewrites ending at event {i}"
                    ),
                    event_index: Some(i),
                });
                break;
            }
        } else {
            streak = 0;
        }
    }

    // Estop fired — always worth surfacing.
    if let Some(i) = events.iter().position(|e| e.kind == "estop") {
        findings.push(EccFinding {
            severity: EccSeverity::Warning,
            message: "estop was pulled during the session".into(),
            event_index: Some(i),
        });
    }

    // Verify-after-build discipline: any build/commit without a later verify.
    if let Some(last_mutation) = events
        .iter()
        .rposition(|e| matches!(e.kind.as_str(), "build" | "commit" | "overwrite"))
    {
        let verify_after = events[last_mutation..].iter().any(|e| e.kind == "verify");
        if !verify_after {
            findings.push(EccFinding {
                severity: EccSeverity::Warning,
                message: "last mutation has no subsequent verify event".into(),
                event_index: Some(last_mutation),
            });
        }
    }

    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, ok: bool) -> SessionEvent {
        SessionEvent {
            kind: kind.into(),
            ok,
            detail: String::new(),
        }
    }

    #[test]
    fn build_without_plan_requires_plan() {
        let policy = EccPolicy::default();
        let events = vec![ev("build", true)];
        assert_eq!(
            plan_before_build(&policy, &events, "build"),
            EccVerdict::RequiresPlan
        );
    }

    #[test]
    fn planned_build_allowed() {
        let policy = EccPolicy::default();
        let events = vec![ev("plan", true), ev("build", true)];
        assert_eq!(
            plan_before_build(&policy, &events, "build"),
            EccVerdict::Allowed
        );
    }

    #[test]
    fn destructive_cap_blocks() {
        let policy = EccPolicy {
            max_destructive_per_session: 2,
            ..Default::default()
        };
        let events = vec![ev("plan", true), ev("delete", true), ev("delete", true)];
        assert!(matches!(
            plan_before_build(&policy, &events, "delete"),
            EccVerdict::Blocked(_)
        ));
    }

    #[test]
    fn scan_flags_unplanned_mutations() {
        let policy = EccPolicy::default();
        let events = vec![ev("build", true), ev("commit", true)];
        let findings = session_scan(&policy, &events);
        assert!(findings
            .iter()
            .any(|f| f.message.contains("no plan event") && f.severity == EccSeverity::Blocking));
    }

    #[test]
    fn scan_flags_runaway_rewrite_streak() {
        let policy = EccPolicy {
            max_consecutive_rewrites: 2,
            ..Default::default()
        };
        let events = vec![
            ev("plan", true),
            ev("rewrite", false),
            ev("rewrite", false),
            ev("rewrite", false),
        ];
        let findings = session_scan(&policy, &events);
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("runaway rewrite loop"))
        );
    }

    #[test]
    fn scan_reports_estop_and_missing_verify() {
        let policy = EccPolicy::default();
        let events = vec![ev("plan", true), ev("build", true), ev("estop", false)];
        let findings = session_scan(&policy, &events);
        assert!(findings.iter().any(|f| f.message.contains("estop")));
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("no subsequent verify"))
        );
    }
}
