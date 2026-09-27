//! Task-manifest format (P8.0) — a reproducible, typed task contract that
//! converts a vague goal into mechanically checkable outcomes. Mirrors the
//! P8.0 `task/manifest.yaml` shape (`goal`, `required_outcomes`,
//! `constraints`, `evidence`).

use serde::{Deserialize, Serialize};

/// A deterministic required-outcome check. Every check is evaluated against
/// the workspace `base_dir` by the verifier — never by the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum OutcomeCheck {
    /// The path must exist (file or directory).
    FileExists { path: String },
    /// The file's hash must match (catches wrong content with the right name).
    FileHash {
        path: String,
        algorithm: HashAlgorithm,
        expected: String,
    },
    /// The file's text must contain `substring` (exact byte match).
    FileContains { path: String, substring: String },
}

impl OutcomeCheck {
    /// A short, human-readable description for reports.
    pub fn describe(&self) -> String {
        match self {
            OutcomeCheck::FileExists { path } => format!("exists({path})"),
            OutcomeCheck::FileHash {
                path, algorithm, ..
            } => format!("hash({path}, {algorithm:?})"),
            OutcomeCheck::FileContains { path, .. } => format!("contains({path})"),
        }
    }

    /// The path this check inspects (used to resolve against `base_dir`).
    pub fn path(&self) -> &str {
        match self {
            OutcomeCheck::FileExists { path }
            | OutcomeCheck::FileHash { path, .. }
            | OutcomeCheck::FileContains { path, .. } => path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgorithm {
    Sha256,
    Sha1,
}

/// A forbidden-side-effect / safety constraint. A violation is a hard-fail
/// gate regardless of outcome completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "constraint", rename_all = "snake_case")]
pub enum Constraint {
    /// Never send to external domains.
    NeverSendToExternalDomains,
    /// Do not modify these paths (read-only).
    DoNotModify { paths: Vec<String> },
    /// Require user approval before this action.
    RequireApprovalBefore { action: String },
}

/// Hard resource budgets. Exceeding a budget is a constraint violation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Budgets {
    pub max_cost_usd: f64,
    pub max_wall_time_sec: u64,
    pub max_destructive_actions: u32,
}

/// Evidence a completed task must bundle. Missing evidence downgrades the
/// result — "finished" with no evidence is a fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRequirement {
    ArtifactSha256,
    SentMessageId,
    ApprovalEventId,
    Screenshot,
    ValidatorReport,
}

/// The typed task contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskManifest {
    pub task_id: String,
    pub goal: String,
    #[serde(default)]
    pub required_outcomes: Vec<OutcomeCheck>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub budgets: Budgets,
    #[serde(default)]
    pub evidence: Vec<EvidenceRequirement>,
}

impl TaskManifest {
    /// `true` when there is nothing the verifier can check — the task is
    /// unverifiable by construction.
    pub fn has_checkable_outcomes(&self) -> bool {
        !self.required_outcomes.is_empty() || !self.evidence.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_serde() {
        let m = TaskManifest {
            task_id: "t1".into(),
            goal: "do the thing".into(),
            required_outcomes: vec![OutcomeCheck::FileExists {
                path: "out.txt".into(),
            }],
            constraints: vec![Constraint::DoNotModify {
                paths: vec!["raw.xlsx".into()],
            }],
            budgets: Budgets {
                max_cost_usd: 0.15,
                max_wall_time_sec: 600,
                max_destructive_actions: 0,
            },
            evidence: vec![EvidenceRequirement::ArtifactSha256],
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: TaskManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert!(back.has_checkable_outcomes());
    }

    #[test]
    fn empty_manifest_is_unverifiable() {
        let m = TaskManifest {
            task_id: "t2".into(),
            goal: "?".into(),
            required_outcomes: vec![],
            constraints: vec![],
            budgets: Budgets::default(),
            evidence: vec![],
        };
        assert!(!m.has_checkable_outcomes());
    }

    #[test]
    fn check_descriptions_and_paths() {
        let c = OutcomeCheck::FileHash {
            path: "a".into(),
            algorithm: HashAlgorithm::Sha256,
            expected: "x".into(),
        };
        assert_eq!(c.path(), "a");
        assert_eq!(c.describe(), "hash(a, Sha256)");
    }
}
