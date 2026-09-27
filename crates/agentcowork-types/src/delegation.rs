//! P69.G2 — delegation liveness and cycle facts (`ARCH/RECOVERY.md` §12).
//!
//! `SubAgentLimits { max_depth, max_concurrent, max_total }` bounds *how many*
//! children may exist and *how deep*; it cannot express `A → B → A`, and it
//! carries no liveness signal at all, so a crashed parent's child lease is never
//! reclaimed. This module owns the two missing facts and nothing else:
//!
//! * **cycle** — why a spawn was refused, in the shape of an error that names
//!   the ancestor chain it would have re-entered;
//! * **liveness** — the heartbeat deadline, and what a reclaim of an orphaned
//!   child lease looks like.
//!
//! The state itself is not here. The child-Work chain in the Work graph *is*
//! the durable record of a delegation (I8); the lease is a
//! [`crate::workbench::LeaseAttachment`] the existing coordinator owns; and the
//! post-effect outcome is [`PostEffectOutcome`], which stays `Uncertain` until
//! something is observed. These are the projections a reader renders.

use serde::{Deserialize, Serialize};

use crate::workbench::{LeaseAttachment, PostEffectOutcome};

/// The `RECOVERY.md` §12 deadline: a child run that has not reported a
/// heartbeat within this window is an orphan candidate for reclaim.
pub const SUBAGENT_HEARTBEAT_TIMEOUT_MS: u64 = 120_000;

/// The documented name of the deadline, for error text and doctor output.
pub const SUBAGENT_HEARTBEAT_TIMEOUT_NAME: &str = "SUBAGENT_HEARTBEAT_TIMEOUT";

/// Why a delegation request would have closed a loop in the Work graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleKind {
    /// The child Work to be created is already an **ancestor** of the
    /// delegating Work. Creating it again would make the parent chain cyclic.
    AncestorWork,
    /// The delegating Work is itself a descendant of the requested child
    /// (caller named a Work below itself).
    DescendantWork,
    /// The member's `AgentBindingId` already appears on the delegating Work's
    /// ancestor chain, so the delegation would re-enter a binding that is
    /// already running this very chain.
    AncestorBinding,
    /// The recorded graph is already cyclic — a malformed or tampered journal.
    /// Refused rather than walked.
    RecordedCycle,
}

impl CycleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AncestorWork => "ancestor_work",
            Self::DescendantWork => "descendant_work",
            Self::AncestorBinding => "ancestor_binding",
            Self::RecordedCycle => "recorded_cycle",
        }
    }
}

/// The refusal produced by walking the Work graph, before any child is minted.
///
/// `chain` is the ancestor chain walked, root first, so the audit row can show
/// *which* path was re-entered rather than only that one existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleRefusal {
    pub kind: CycleKind,
    /// Ancestor Work ids, root first, ending with the delegating Work.
    pub chain: Vec<String>,
    /// The binding that would have been re-entered, when the kind is
    /// [`CycleKind::AncestorBinding`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    /// The child Work id the request would have created.
    pub proposed_child_work_id: String,
}

impl CycleRefusal {
    /// The first-class refusal error. Its `Display` begins with the
    /// documented token `CyclicDelegationRefused` so a refusal is greppable in
    /// logs, receipts, and the sidecar's error string alike.
    pub fn as_error(&self) -> CyclicDelegationRefused {
        CyclicDelegationRefused {
            kind: self.kind,
            chain: self.chain.clone(),
            binding_id: self.binding_id.clone(),
            proposed_child_work_id: self.proposed_child_work_id.clone(),
        }
    }
}

/// The refusal itself. Distinct from a generic string error so the caller can
/// audit it and the doctor surface can count it.
///
/// Hand-written `Display`/`Error` rather than a derive: this crate is
/// deliberately dependency-free (newtypes + enums + shapes, no logic and no
/// macros), so the vocabulary a refusal prints is spelled here and cannot drift
/// away from [`CycleKind::as_str`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CyclicDelegationRefused {
    pub kind: CycleKind,
    pub chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    pub proposed_child_work_id: String,
}

impl CyclicDelegationRefused {
    /// The canonical refusal token `RECOVERY.md` §12 names. Every message
    /// starts with it, so a refusal is greppable in logs, receipts, and the
    /// sidecar's error string alike.
    pub const TOKEN: &'static str = "CyclicDelegationRefused";

    /// What the request would have re-entered, in words.
    pub fn reentry(&self) -> &'static str {
        match self.kind {
            CycleKind::AncestorWork => "an ancestor Work on the delegation chain",
            CycleKind::DescendantWork => "its own descendant Work",
            CycleKind::AncestorBinding => "an ancestor AgentBinding already running this chain",
            CycleKind::RecordedCycle => "a Work graph that is already cyclic",
        }
    }
}

impl std::fmt::Display for CyclicDelegationRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}): {} would re-enter {}; chain [{}]",
            Self::TOKEN,
            self.kind.as_str(),
            self.proposed_child_work_id,
            self.reentry(),
            self.chain.join(" > ")
        )
    }
}

impl std::error::Error for CyclicDelegationRefused {}

/// The live liveness record of one delegated child, projected from the child
/// Work's own durable timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentLiveness {
    pub child_work_id: String,
    pub parent_work_id: String,
    /// Last durable heartbeat (or the child's creation, when it never beat).
    pub last_heartbeat_ms: u64,
    /// Whether the child Work has reached a terminal state.
    pub terminal: bool,
    /// The delegation lease covering the child, when one was acquired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseAttachment>,
}

impl SubagentLiveness {
    /// Silence at `now_ms`. Saturating, so a clock step backwards reads as
    /// zero silence rather than a wrapped `u64`.
    pub fn silence_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_heartbeat_ms)
    }

    /// Whether this child is an orphan candidate: non-terminal and silent for
    /// longer than the deadline.
    pub fn is_orphaned_at(&self, now_ms: u64, timeout_ms: u64) -> bool {
        !self.terminal && self.silence_ms(now_ms) > timeout_ms
    }
}

/// The structured timeout receipt a parent receives for a reclaimed child lease
/// (`RECOVERY.md` §12).
///
/// `effect_state` is [`PostEffectOutcome::Uncertain`] by construction: a lease
/// reclaimed on a heartbeat timeout has **no observation** of whether its
/// in-flight effect landed. It is never `Success` and never `Failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentTimeoutReceipt {
    pub child_work_id: String,
    pub parent_work_id: String,
    pub silent_ms: u64,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseAttachment>,
    /// In-flight effects on the child that have no settled outcome. Each is
    /// recorded `uncertain`; none is reported as done or failed.
    pub effect_ids: Vec<String>,
    pub effect_state: PostEffectOutcome,
}

impl SubagentTimeoutReceipt {
    /// The honesty invariant, stated once so a test can assert it against the
    /// type rather than against a call site: a heartbeat-timeout reclaim never
    /// settles an effect.
    pub fn outcome_is_uncertain(&self) -> bool {
        matches!(self.effect_state, PostEffectOutcome::Uncertain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LeaseId;
    use crate::SessionId;
    use crate::workbench::{LeaseState, ResourceKind, TypedResourceKey};

    fn lease() -> LeaseAttachment {
        LeaseAttachment {
            lease_id: LeaseId::new("lease-7"),
            session_scope: SessionId::new("s-1"),
            resource_key: TypedResourceKey::new(ResourceKind::Other, "work:w-1", 1).canonical_key(),
            generation: 1,
            fence: 2,
            state: LeaseState::Uncertain,
        }
    }

    #[test]
    fn the_refusal_error_carries_the_documented_token_first() {
        let refusal = CycleRefusal {
            kind: CycleKind::AncestorWork,
            chain: vec!["w-root".into(), "w-mid".into()],
            binding_id: None,
            proposed_child_work_id: "w-root/subagent/t".into(),
        };
        let text = refusal.as_error().to_string();
        assert!(
            text.starts_with("CyclicDelegationRefused"),
            "a refusal must be greppable by name: {text}"
        );
        assert!(text.contains("w-root/subagent/t"));
        assert!(text.contains("ancestor Work"));
    }

    #[test]
    fn a_binding_refusal_names_the_binding_it_would_re_enter() {
        let refusal = CycleRefusal {
            kind: CycleKind::AncestorBinding,
            chain: vec!["w-root".into()],
            binding_id: Some("binding-1".into()),
            proposed_child_work_id: "w-root/subagent/t".into(),
        };
        assert_eq!(refusal.as_error().binding_id.as_deref(), Some("binding-1"));
        assert!(refusal.as_error().to_string().contains("AgentBinding"));
    }

    #[test]
    fn silence_is_saturating_and_the_orphan_test_is_strict() {
        let live = SubagentLiveness {
            child_work_id: "w-1/subagent/t".into(),
            parent_work_id: "w-1".into(),
            last_heartbeat_ms: 1_000,
            terminal: false,
            lease: None,
        };
        assert_eq!(
            live.silence_ms(1_000 + SUBAGENT_HEARTBEAT_TIMEOUT_MS),
            SUBAGENT_HEARTBEAT_TIMEOUT_MS
        );
        assert!(!live.is_orphaned_at(
            1_000 + SUBAGENT_HEARTBEAT_TIMEOUT_MS,
            SUBAGENT_HEARTBEAT_TIMEOUT_MS
        ));
        assert!(live.is_orphaned_at(
            1_000 + SUBAGENT_HEARTBEAT_TIMEOUT_MS + 1,
            SUBAGENT_HEARTBEAT_TIMEOUT_MS
        ));
        // A clock step backwards reads as zero silence, never a wrapped u64.
        assert_eq!(live.silence_ms(0), 0);
        // A terminal child is never an orphan candidate, however long silent.
        let mut done = live.clone();
        done.terminal = true;
        assert!(!done.is_orphaned_at(u64::MAX, 0));
    }

    #[test]
    fn a_timeout_receipt_is_uncertain_never_done_or_failed() {
        let receipt = SubagentTimeoutReceipt {
            child_work_id: "w-1/subagent/t".into(),
            parent_work_id: "w-1".into(),
            silent_ms: 200_000,
            timeout_ms: SUBAGENT_HEARTBEAT_TIMEOUT_MS,
            lease: Some(lease()),
            effect_ids: vec!["eff-1".into()],
            effect_state: PostEffectOutcome::Uncertain,
        };
        assert!(receipt.outcome_is_uncertain());
        assert_eq!(receipt.timeout_ms, 120_000);
        // The receipt carries only safe lease fields.
        let rendered = serde_json::to_string(&receipt).unwrap();
        assert!(crate::workbench::projection_json_has_no_secrets(&rendered));
    }
}
