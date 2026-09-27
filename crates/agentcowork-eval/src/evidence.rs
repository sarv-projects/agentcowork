//! Evidence bundle (P8.0) — the artifact hashes, validator reports,
//! screenshots, and approval events a completed task must carry. Missing
//! evidence is explicit and downgrades the result: "finished" with no
//! evidence is a fail.

use crate::manifest::{EvidenceRequirement, HashAlgorithm, TaskManifest};
use serde::{Deserialize, Serialize};

/// A hashed artifact (the proof a specific file content existed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactHash {
    pub path: String,
    pub algorithm: HashAlgorithm,
    pub hash: String,
}

/// A recorded user approval for a privileged action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalEvent {
    pub action: String,
    pub approved_by: String,
    pub timestamp_secs: u64,
}

/// The evidence bundle attached to a completed task.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub artifact_hashes: Vec<ArtifactHash>,
    pub validator_reports: Vec<String>,
    pub screenshots: Vec<String>,
    pub approval_events: Vec<ApprovalEvent>,
}

impl EvidenceBundle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Does this bundle satisfy one evidence requirement?
    pub fn satisfies(&self, requirement: EvidenceRequirement) -> bool {
        match requirement {
            EvidenceRequirement::ArtifactSha256 => self
                .artifact_hashes
                .iter()
                .any(|a| a.algorithm == HashAlgorithm::Sha256),
            EvidenceRequirement::SentMessageId => !self.validator_reports.is_empty(),
            EvidenceRequirement::ApprovalEventId => !self.approval_events.is_empty(),
            EvidenceRequirement::Screenshot => !self.screenshots.is_empty(),
            EvidenceRequirement::ValidatorReport => !self.validator_reports.is_empty(),
        }
    }

    /// The manifest's evidence requirements not yet satisfied — the explicit
    /// "missing evidence" list.
    pub fn missing(&self, manifest: &TaskManifest) -> Vec<EvidenceRequirement> {
        manifest
            .evidence
            .iter()
            .copied()
            .filter(|req| !self.satisfies(*req))
            .collect()
    }

    /// `true` when every required piece of evidence is present.
    pub fn is_complete_for(&self, manifest: &TaskManifest) -> bool {
        self.missing(manifest).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Budgets;

    fn manifest(evidence: Vec<EvidenceRequirement>) -> TaskManifest {
        TaskManifest {
            task_id: "t".into(),
            goal: "g".into(),
            required_outcomes: vec![],
            constraints: vec![],
            budgets: Budgets::default(),
            evidence,
        }
    }

    #[test]
    fn empty_bundle_is_incomplete() {
        let m = manifest(vec![EvidenceRequirement::ArtifactSha256]);
        assert!(!EvidenceBundle::new().is_complete_for(&m));
        assert_eq!(
            EvidenceBundle::new().missing(&m),
            vec![EvidenceRequirement::ArtifactSha256]
        );
    }

    #[test]
    fn bundle_satisfies_what_it_has() {
        let mut b = EvidenceBundle::new();
        b.artifact_hashes.push(ArtifactHash {
            path: "out.xlsx".into(),
            algorithm: HashAlgorithm::Sha256,
            hash: "abc".into(),
        });
        let m = manifest(vec![
            EvidenceRequirement::ArtifactSha256,
            EvidenceRequirement::Screenshot,
        ]);
        assert!(!b.is_complete_for(&m));
        assert_eq!(b.missing(&m), vec![EvidenceRequirement::Screenshot]);

        b.screenshots.push("shot.png".into());
        assert!(b.is_complete_for(&m));
    }

    #[test]
    fn approval_and_validator_requirements() {
        let mut b = EvidenceBundle::new();
        b.approval_events.push(ApprovalEvent {
            action: "send_email".into(),
            approved_by: "user".into(),
            timestamp_secs: 1,
        });
        b.validator_reports.push("validator.json".into());
        let m = manifest(vec![
            EvidenceRequirement::ApprovalEventId,
            EvidenceRequirement::ValidatorReport,
        ]);
        assert!(b.is_complete_for(&m));
    }

    #[test]
    fn bundle_roundtrips_serde() {
        let mut b = EvidenceBundle::new();
        b.artifact_hashes.push(ArtifactHash {
            path: "x".into(),
            algorithm: HashAlgorithm::Sha1,
            hash: "h".into(),
        });
        let json = serde_json::to_string(&b).unwrap();
        let back: EvidenceBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }
}
