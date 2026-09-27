//! Evidence-first loop report (P8.0 — better-harness pattern): post-session
//! findings carry impact / expected-output / scoped-repair /
//! acceptance-checks, and **missing evidence stays explicit** — the I5 loop
//! self-audit (P7.1) feeds this.

use crate::manifest::OutcomeCheck;
use crate::status::CompletionStatus;
use serde::{Deserialize, Serialize};

/// One post-session finding. A finding with empty `evidence` is explicitly
/// unproven — never silently trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// What happened / what the change does.
    pub impact: String,
    /// What the correct result should look like.
    pub expected_output: String,
    /// The scoped repair/follow-up (limited to this finding).
    pub scoped_repair: String,
    /// Deterministic checks that prove the repair worked.
    pub acceptance_checks: Vec<OutcomeCheck>,
    /// Evidence ids proving the finding; empty = missing, stays explicit.
    pub evidence: Vec<String>,
}

impl Finding {
    /// A finding is "proven" only when it carries at least one piece of
    /// evidence and at least one acceptance check.
    pub fn is_proven(&self) -> bool {
        !self.evidence.is_empty() && !self.acceptance_checks.is_empty()
    }

    pub fn has_missing_evidence(&self) -> bool {
        self.evidence.is_empty()
    }
}

/// The evidence-first loop report for a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopReport {
    pub session_id: String,
    pub findings: Vec<Finding>,
    /// The overall completion status (from the verifier).
    pub status: CompletionStatus,
}

impl LoopReport {
    pub fn new(session_id: impl Into<String>, status: CompletionStatus) -> Self {
        Self {
            session_id: session_id.into(),
            findings: Vec::new(),
            status,
        }
    }

    pub fn push(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    /// Findings whose evidence is missing — the explicit "unproven" list.
    pub fn findings_with_missing_evidence(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.has_missing_evidence())
            .collect()
    }

    /// `true` when every finding is proven. Used by the I5 self-audit to
    /// refuse to report "done" while any finding is unproven.
    pub fn is_evidence_complete(&self) -> bool {
        self.findings.iter().all(|f| f.is_proven())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::OutcomeCheck;

    fn proven_finding() -> Finding {
        Finding {
            impact: "added route".into(),
            expected_output: "GET /x returns 200".into(),
            scoped_repair: "add test".into(),
            acceptance_checks: vec![OutcomeCheck::FileExists {
                path: "x.rs".into(),
            }],
            evidence: vec!["hash-1".into()],
        }
    }

    #[test]
    fn proven_finding_has_evidence_and_checks() {
        assert!(proven_finding().is_proven());
        assert!(!proven_finding().has_missing_evidence());
    }

    #[test]
    fn missing_evidence_stays_explicit() {
        let mut r = LoopReport::new(
            "s1",
            CompletionStatus::PartiallyComplete {
                missing: vec!["a".into()],
            },
        );
        r.push(proven_finding());
        r.push(Finding {
            evidence: vec![],
            ..proven_finding()
        });
        assert!(!r.is_evidence_complete());
        assert_eq!(r.findings_with_missing_evidence().len(), 1);
    }

    #[test]
    fn complete_report_has_no_unproven_findings() {
        let mut r = LoopReport::new("s2", CompletionStatus::VerifiedComplete);
        r.push(proven_finding());
        assert!(r.is_evidence_complete());
        assert!(r.findings_with_missing_evidence().is_empty());
    }
}
