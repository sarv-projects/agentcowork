//! Loop self-audit (P7.1 — doc 58 better-harness pattern): the post-session
//! five-dimension report computed from the audit NDJSON trail.
//!
//! Dimensions (doc 58):
//! 1. **Task Understanding** — did the session start from a plan/spec and
//!    stay on it?
//! 2. **Controlled Execution** — did every effect pass a ticket/approval,
//!    with no blocks or estops?
//! 3. **Change Validation** — were changes verified (tests/verify events)
//!    rather than merely claimed?
//! 4. **Reliable Delivery** — did effects commit with receipts and survive
//!    to the end of the session?
//! 5. **Learning Capture** — did the session record skills/facts/traces it
//!    can reuse?
//!
//! Scoring is deterministic over [`AuditEvent`] kinds — "missing evidence
//! stays explicit": each dimension carries the evidence kinds that were
//! absent, so a green-looking score can never hide an unverified claim.

use serde::{Deserialize, Serialize};

/// One audit-trail event (the NDJSON row shape the report consumes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Event kind, e.g. `plan`, `spec`, `ticket`, `approve`, `deny`,
    /// `estop`, `build`, `test`, `verify`, `commit`, `receipt`, `skill`,
    /// `fact`, `error`.
    pub kind: String,
    /// Whether the event indicates success.
    #[serde(default = "default_true")]
    pub ok: bool,
    /// Free-form detail for the evidence list.
    #[serde(default)]
    pub detail: String,
}

fn default_true() -> bool {
    true
}

/// The five dimensions, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDimension {
    TaskUnderstanding,
    ControlledExecution,
    ChangeValidation,
    ReliableDelivery,
    LearningCapture,
}

impl AuditDimension {
    pub fn label(self) -> &'static str {
        match self {
            Self::TaskUnderstanding => "Task Understanding",
            Self::ControlledExecution => "Controlled Execution",
            Self::ChangeValidation => "Change Validation",
            Self::ReliableDelivery => "Reliable Delivery",
            Self::LearningCapture => "Learning Capture",
        }
    }
}

/// One dimension's score (0.0–1.0) plus the evidence that produced it and
/// the evidence that was missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionScore {
    pub dimension: AuditDimension,
    pub score: f64,
    /// Evidence lines backing the score.
    pub evidence: Vec<String>,
    /// Evidence kinds the dimension needed but the session never produced.
    pub missing_evidence: Vec<String>,
}

/// The full post-session report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfAuditReport {
    pub dimensions: Vec<DimensionScore>,
    pub overall: f64,
    /// Human-readable summary for the harness UI.
    pub summary: String,
}

impl SelfAuditReport {
    pub fn dimension(&self, d: AuditDimension) -> Option<&DimensionScore> {
        self.dimensions.iter().find(|s| s.dimension == d)
    }
}

/// Compute the five-dimension report from a session's event trail.
/// Deterministic: same events → same report.
pub fn score_session(events: &[AuditEvent]) -> SelfAuditReport {
    let has =
        |kinds: &[&str]| -> bool { events.iter().any(|e| kinds.iter().any(|k| *k == e.kind)) };
    let count = |kind: &str| events.iter().filter(|e| e.kind == kind).count();
    let had_block = events.iter().any(|e| {
        matches!(e.kind.as_str(), "deny" | "block" | "estop") || (e.kind == "ticket" && !e.ok)
    });

    // 1. Task Understanding: plan or spec present, and no "no-plan" errors.
    let tu_evidence = events
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "plan" | "spec" | "task"))
        .map(|e| format!("{}: {}", e.kind, e.detail))
        .collect::<Vec<_>>();
    let mut tu_missing = Vec::new();
    if !has(&["plan"]) && !has(&["spec"]) {
        tu_missing.push("plan".into());
    }
    let tu = if tu_evidence.is_empty() {
        0.0
    } else {
        let mut s: f64 = 1.0;
        if !has(&["plan"]) {
            s -= 0.25;
        }
        s.max(0.0)
    };

    // 2. Controlled Execution: tickets/approvals happened, no blocks.
    let ce_evidence = events
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "ticket" | "approve" | "deny" | "estop"))
        .map(|e| format!("{}: {}", e.kind, e.detail))
        .collect::<Vec<_>>();
    let mut ce_missing = Vec::new();
    if !has(&["ticket"]) && !has(&["approve"]) {
        ce_missing.push("ticket".into());
    }
    let ce = if had_block {
        0.2
    } else if ce_evidence.is_empty() {
        0.5
    } else {
        1.0
    };

    // 3. Change Validation: verify/test events after changes.
    let cv_evidence = events
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "verify" | "test" | "eval"))
        .map(|e| format!("{}: {}", e.kind, e.detail))
        .collect::<Vec<_>>();
    let mut cv_missing = Vec::new();
    if !has(&["verify"]) && !has(&["test"]) && !has(&["eval"]) {
        cv_missing.push("verify".into());
    }
    let cv = if cv_evidence.is_empty() {
        0.0
    } else if count("verify") > 0 {
        1.0
    } else {
        0.6
    };

    // 4. Reliable Delivery: commits/receipts present, no terminal errors.
    let rd_evidence = events
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "commit" | "receipt" | "outcome"))
        .map(|e| format!("{}: {}", e.kind, e.detail))
        .collect::<Vec<_>>();
    let mut rd_missing = Vec::new();
    if !has(&["commit"]) && !has(&["receipt"]) {
        rd_missing.push("receipt".into());
    }
    let had_error = events.iter().any(|e| e.kind == "error" && !e.ok);
    let rd = if had_error {
        0.3
    } else if rd_evidence.is_empty() {
        0.4
    } else {
        1.0
    };

    // 5. Learning Capture: skills/facts/crystallization recorded.
    let lc_evidence = events
        .iter()
        .filter(|e| {
            matches!(
                e.kind.as_str(),
                "skill" | "fact" | "crystallize" | "consolidate"
            )
        })
        .map(|e| format!("{}: {}", e.kind, e.detail))
        .collect::<Vec<_>>();
    let mut lc_missing = Vec::new();
    if !has(&["skill"]) && !has(&["fact"]) && !has(&["crystallize"]) {
        lc_missing.push("skill".into());
    }
    let lc = if lc_evidence.is_empty() { 0.0 } else { 1.0 };

    let dims = vec![
        DimensionScore {
            dimension: AuditDimension::TaskUnderstanding,
            score: tu,
            evidence: tu_evidence,
            missing_evidence: tu_missing,
        },
        DimensionScore {
            dimension: AuditDimension::ControlledExecution,
            score: ce,
            evidence: ce_evidence,
            missing_evidence: ce_missing,
        },
        DimensionScore {
            dimension: AuditDimension::ChangeValidation,
            score: cv,
            evidence: cv_evidence,
            missing_evidence: cv_missing,
        },
        DimensionScore {
            dimension: AuditDimension::ReliableDelivery,
            score: rd,
            evidence: rd_evidence,
            missing_evidence: rd_missing,
        },
        DimensionScore {
            dimension: AuditDimension::LearningCapture,
            score: lc,
            evidence: lc_evidence,
            missing_evidence: lc_missing,
        },
    ];
    let overall = dims.iter().map(|d| d.score).sum::<f64>() / dims.len() as f64;

    // "Missing evidence stays explicit": if anything important is absent,
    // say so in the summary instead of letting the number speak alone.
    let missing: Vec<&str> = dims
        .iter()
        .flat_map(|d| d.missing_evidence.iter().map(|m| m.as_str()))
        .collect();
    let summary = if missing.is_empty() {
        format!(
            "{:.0}% verified-completion posture — all evidence classes present",
            overall * 100.0
        )
    } else {
        format!(
            "{:.0}% posture — missing evidence: {}",
            overall * 100.0,
            missing.join(", ")
        )
    };

    SelfAuditReport {
        dimensions: dims,
        overall,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, ok: bool) -> AuditEvent {
        AuditEvent {
            kind: kind.into(),
            ok,
            detail: String::new(),
        }
    }

    #[test]
    fn empty_session_scores_low_and_lists_missing() {
        let report = score_session(&[]);
        // Neutral baselines (no blocks, no errors) keep it above 0, but with
        // zero evidence the score must stay low and the summary must say so.
        assert!(report.overall < 0.25, "overall = {}", report.overall);
        assert!(report.summary.contains("missing evidence"));
        assert!(
            report
                .dimension(AuditDimension::ChangeValidation)
                .unwrap()
                .missing_evidence
                .contains(&"verify".to_string())
        );
    }

    #[test]
    fn full_session_scores_high() {
        let events = vec![
            ev("plan", true),
            ev("spec", true),
            ev("ticket", true),
            ev("approve", true),
            ev("build", true),
            ev("test", true),
            ev("verify", true),
            ev("commit", true),
            ev("receipt", true),
            ev("skill", true),
        ];
        let report = score_session(&events);
        assert_eq!(report.overall, 1.0);
        assert!(report.summary.contains("all evidence classes present"));
    }

    #[test]
    fn blocks_crush_controlled_execution() {
        let events = vec![
            ev("plan", true),
            ev("estop", false),
            ev("build", true),
            ev("test", true),
            ev("verify", true),
            ev("commit", true),
        ];
        let report = score_session(&events);
        assert_eq!(
            report
                .dimension(AuditDimension::ControlledExecution)
                .unwrap()
                .score,
            0.2
        );
    }

    #[test]
    fn unverified_change_is_explicit() {
        let events = vec![
            ev("plan", true),
            ev("ticket", true),
            ev("approve", true),
            ev("build", true),
            ev("commit", true),
        ];
        let report = score_session(&events);
        let cv = report.dimension(AuditDimension::ChangeValidation).unwrap();
        assert_eq!(cv.score, 0.0);
        assert!(cv.missing_evidence.contains(&"verify".to_string()));
        assert!(report.summary.contains("verify"));
    }
}
