//! K1 Proof-Carrying Work Receipts (doc 81 §4): a portable receipt contract
//! over the Merkle chain + GuardReceipt + EV1 evidence. A [`WorkReceipt`]
//! answers the acceptance test — "5 questions in 1 min without chat
//! history": what was the goal, what did you do, what evidence proves it,
//! what did it cost, and can I reproduce it.
//!
//! Every field is required (no silent omission); the receipt self-hashes so
//! an exported receipt can be verified against the audit chain root.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The eleven receipt fields (doc 81 §4 contract).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkReceipt {
    pub receipt_id: String,
    /// The goal (the task's own words).
    pub goal: String,
    /// Inputs (files, URLs, selections, refs) that fed the work.
    pub inputs: Vec<String>,
    /// The plan steps that were executed.
    pub plan: Vec<String>,
    /// Every action taken (tool calls with their ticket ids).
    pub actions: Vec<ReceiptActionRef>,
    /// K1 per-effect proofs (P47.5) — the nested receipt layer. Present for
    /// real-world effects; empty for read-only work.
    #[serde(default)]
    pub effects: Vec<EffectReceipt>,
    /// EV1 evidence (hashes, validator reports, screenshots).
    pub evidence: Vec<EvidenceRef>,
    /// Verification result (what proved completion).
    pub verification: VerificationSummary,
    /// Provenance: agent, session, audit chain root hash.
    pub provenance: Provenance,
    /// The policy the work ran under (profile + gates).
    pub policy: String,
    /// Reproduction recipe (commands/inputs to re-run deterministically).
    pub reproduction: Vec<String>,
    /// Cost: tokens + estimated USD.
    pub cost: CostSummary,
    /// The resulting state (files changed, outputs, hashes).
    pub result_state: Vec<String>,
}

/// One action taken during the work, tied to its guard ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceiptActionRef {
    pub tool_id: String,
    pub ticket_id: String,
    pub args_hash: String,
    /// Effect class (doc-53 idempotency) — set by the change-set layer.
    pub effect_class: String,
}

/// K1 per-effect proof (P47.5). The nested layer of a [`WorkReceipt`]: a
/// machine-checkable record of ONE real-world effect — what was requested,
/// what was actually authorized and applied, the resource before/after, the
/// diff, the rollback handle, and an honest `has_gap` flag when the change
/// could not be fully verified. This is the evidence behind every claim the
/// agent makes, per effect, not just a per-work summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectReceipt {
    pub effect_id: String,
    pub tool_id: String,
    pub ticket_id: String,
    pub args_hash: String,
    /// The operation the coordinator/agent requested.
    pub requested: String,
    /// The operation a Guard ticket actually authorized (should match
    /// `requested`; a mismatch is a red flag an audit reader must see).
    pub authorized: String,
    pub resource: Option<String>,
    /// Pre-condition hash / snapshot reference (for reversible effects).
    pub before_ref: Option<String>,
    /// Post-condition hash / snapshot reference.
    pub after_ref: Option<String>,
    pub diff: Option<String>,
    /// Rollback handle (snapshot id / undo entry) if the effect is reversible.
    pub rollback_ref: Option<String>,
    /// Honesty invariant (doc 05/EV1): true when the effect could not be fully
    /// observed, so the reader must not assume it happened exactly as claimed.
    pub has_gap: bool,
    /// The uncertainty reason when `has_gap` is true (else None).
    pub uncertainty: Option<String>,
}

impl EffectReceipt {
    /// Build with required fields; the honesty fields default to closed.
    pub fn new(
        effect_id: impl Into<String>,
        tool_id: impl Into<String>,
        ticket_id: impl Into<String>,
        args_hash: impl Into<String>,
        requested: impl Into<String>,
        authorized: impl Into<String>,
    ) -> Self {
        Self {
            effect_id: effect_id.into(),
            tool_id: tool_id.into(),
            ticket_id: ticket_id.into(),
            args_hash: args_hash.into(),
            requested: requested.into(),
            authorized: authorized.into(),
            resource: None,
            before_ref: None,
            after_ref: None,
            diff: None,
            rollback_ref: None,
            has_gap: false,
            uncertainty: None,
        }
    }

    /// Set the resource + before/after snapshot refs.
    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }
    pub fn with_refs(mut self, before: impl Into<String>, after: impl Into<String>) -> Self {
        self.before_ref = Some(before.into());
        self.after_ref = Some(after.into());
        self
    }
    pub fn with_diff(mut self, diff: impl Into<String>) -> Self {
        self.diff = Some(diff.into());
        self
    }
    pub fn with_rollback(mut self, rollback: impl Into<String>) -> Self {
        self.rollback_ref = Some(rollback.into());
        self
    }

    /// Mark the effect partially-observed (K1 has_gap honesty).
    pub fn gap(mut self, reason: impl Into<String>) -> Self {
        self.has_gap = true;
        self.uncertainty = Some(reason.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub kind: String,
    pub hash: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationSummary {
    /// e.g. `verified_complete`, `partially_complete`, `failed_safely`.
    pub status: String,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub agent_id: String,
    pub session_id: String,
    /// The Merkle chain root hash this receipt anchors to ("" = unanchored).
    pub chain_root: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostSummary {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub est_cost_usd: f64,
}

impl WorkReceipt {
    /// Deterministic self-hash: SHA-256 over the canonical JSON. The
    /// receipt's own integrity check — any field change breaks it.
    pub fn hash(&self) -> String {
        let canon = serde_json::to_vec(self).unwrap_or_default();
        format!("{:x}", Sha256::digest(canon))
    }

    /// A receipt is sound when its stored `hash` matches a recompute (the
    /// caller stores the hash alongside — e.g. as the Merkle chain root).
    pub fn verify(stored_hash: &str, receipt: &WorkReceipt) -> bool {
        stored_hash == receipt.hash()
    }

    /// The markdown render — answers the 5 questions without chat history.
    pub fn render(&self) -> String {
        format!(
            "# Work receipt {id}\n\n\
             ## Goal\n{goal}\n\n\
             ## Inputs\n{inputs}\n\
             ## Plan\n{plan}\n\
             ## Actions ({n})\n{actions}\n\
             ## Effects ({ne})\n{effects}\n\
             ## Evidence\n{evidence}\n\
             ## Verification\n{status} — {checks}\n\
             ## Provenance\nagent {agent} · session {session} · chain {chain}\n\
             ## Policy\n{policy}\n\
             ## Reproduction\n{reproduction}\n\
             ## Cost\n{in_tok} in / {out_tok} out · ${cost:.4}\n\
             ## Result state\n{result}\n",
            id = self.receipt_id,
            goal = self.goal,
            inputs = bullet(&self.inputs),
            plan = bullet(&self.plan),
            n = self.actions.len(),
            actions = bullet(
                &self
                    .actions
                    .iter()
                    .map(|a| format!(
                        "{} (ticket {}) [{}]",
                        a.tool_id, a.ticket_id, a.effect_class
                    ))
                    .collect::<Vec<_>>()
            ),
            effects = if self.effects.is_empty() {
                "(no side-effecting effects)".into()
            } else {
                self.effects
                    .iter()
                    .map(|e| {
                        let gap = if e.has_gap {
                            format!(
                                " \u{26a0} has_gap{}",
                                e.uncertainty
                                    .as_ref()
                                    .map(|u| format!(" ({u})"))
                                    .unwrap_or_default()
                            )
                        } else {
                            "".into()
                        };
                        format!(
                            "- {}: req={} auth={}{}{}",
                            e.tool_id,
                            e.requested,
                            e.authorized,
                            e.resource
                                .as_ref()
                                .map(|r| format!(" \u{2192} {r}"))
                                .unwrap_or_default(),
                            gap
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
            evidence = bullet(
                &self
                    .evidence
                    .iter()
                    .map(|e| format!(
                        "{}: {} ({})",
                        e.kind,
                        e.description,
                        &e.hash[..e.hash.len().min(12)]
                    ))
                    .collect::<Vec<_>>()
            ),
            status = self.verification.status,
            checks = self.verification.checks.join("; "),
            agent = self.provenance.agent_id,
            session = self.provenance.session_id,
            chain = if self.provenance.chain_root.is_empty() {
                "unanchored".into()
            } else {
                self.provenance.chain_root.clone()
            },
            policy = self.policy,
            reproduction = bullet(&self.reproduction),
            in_tok = self.cost.tokens_in,
            out_tok = self.cost.tokens_out,
            cost = self.cost.est_cost_usd,
            result = bullet(&self.result_state),
            ne = self.effects.len(),
        )
    }
}

fn bullet(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".into()
    } else {
        items
            .iter()
            .map(|i| format!("- {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A builder so the executor assembles the receipt in field order without
/// partial-state bugs (only `build` produces a [`WorkReceipt`]).
#[derive(Debug, Clone, Default)]
pub struct ReceiptBuilder {
    goal: String,
    inputs: Vec<String>,
    plan: Vec<String>,
    actions: Vec<ReceiptActionRef>,
    effects: Vec<EffectReceipt>,
    evidence: Vec<EvidenceRef>,
    verification: Option<VerificationSummary>,
    provenance: Option<Provenance>,
    policy: String,
    reproduction: Vec<String>,
    cost: CostSummary,
    result_state: Vec<String>,
}

impl ReceiptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach one per-effect receipt (K1 P47.5).
    pub fn effect(mut self, e: EffectReceipt) -> Self {
        self.effects.push(e);
        self
    }
    pub fn goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = goal.into();
        self
    }
    pub fn input(mut self, input: impl Into<String>) -> Self {
        self.inputs.push(input.into());
        self
    }
    pub fn plan_step(mut self, step: impl Into<String>) -> Self {
        self.plan.push(step.into());
        self
    }
    pub fn action(mut self, a: ReceiptActionRef) -> Self {
        self.actions.push(a);
        self
    }
    pub fn evidence(mut self, e: EvidenceRef) -> Self {
        self.evidence.push(e);
        self
    }
    pub fn verification(mut self, v: VerificationSummary) -> Self {
        self.verification = Some(v);
        self
    }
    pub fn provenance(mut self, p: Provenance) -> Self {
        self.provenance = Some(p);
        self
    }
    pub fn policy(mut self, policy: impl Into<String>) -> Self {
        self.policy = policy.into();
        self
    }
    pub fn reproduction(mut self, r: impl Into<String>) -> Self {
        self.reproduction.push(r.into());
        self
    }
    pub fn cost(mut self, c: CostSummary) -> Self {
        self.cost = c;
        self
    }
    pub fn result(mut self, r: impl Into<String>) -> Self {
        self.result_state.push(r.into());
        self
    }
    /// `build` requires the verification + provenance halves (a receipt
    /// without them is not proof-carrying).
    pub fn build(self, receipt_id: impl Into<String>) -> Result<WorkReceipt, String> {
        let verification = self.verification.ok_or("missing verification")?;
        let provenance = self.provenance.ok_or("missing provenance")?;
        if self.goal.is_empty() {
            return Err("missing goal".into());
        }
        Ok(WorkReceipt {
            receipt_id: receipt_id.into(),
            goal: self.goal,
            inputs: self.inputs,
            plan: self.plan,
            actions: self.actions,
            effects: self.effects,
            evidence: self.evidence,
            verification,
            provenance,
            policy: self.policy,
            reproduction: self.reproduction,
            cost: self.cost,
            result_state: self.result_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WorkReceipt {
        ReceiptBuilder::new()
            .goal("Fix N+1 query in the parser")
            .input("src/parser.rs")
            .plan_step("Reproduce with the fixture")
            .plan_step("Replace loop with a join")
            .action(ReceiptActionRef {
                tool_id: "fs.write".into(),
                ticket_id: "t-1".into(),
                args_hash: "abc123".into(),
                effect_class: "reversible".into(),
            })
            .evidence(EvidenceRef {
                kind: "file_hash".into(),
                hash: "deadbeef".into(),
                description: "src/parser.rs post-edit hash".into(),
            })
            .verification(VerificationSummary {
                status: "verified_complete".into(),
                checks: vec!["fixture passes".into(), "no N+1 in trace".into()],
            })
            .provenance(Provenance {
                agent_id: "claude".into(),
                session_id: "s-9".into(),
                chain_root: "merkle-root-1".into(),
            })
            .policy("profile=standard")
            .reproduction("cargo test --fixture n1")
            .cost(CostSummary {
                tokens_in: 1000,
                tokens_out: 200,
                est_cost_usd: 0.003,
            })
            .result("src/parser.rs changed (1 file)")
            .build("r-1")
            .unwrap()
    }

    #[test]
    fn hash_changes_with_any_field() {
        let r = sample();
        let h1 = r.hash();
        let mut r2 = r.clone();
        r2.result_state.push("extra".into());
        assert_ne!(h1, r2.hash());
        assert!(WorkReceipt::verify(&h1, &r));
        assert!(!WorkReceipt::verify(&h1, &r2));
    }

    #[test]
    fn build_requires_verification_and_provenance() {
        assert!(ReceiptBuilder::new().goal("x").build("r").is_err());
        assert!(
            ReceiptBuilder::new()
                .goal("x")
                .verification(VerificationSummary {
                    status: "v".into(),
                    checks: vec![]
                })
                .build("r")
                .is_err()
        );
    }

    #[test]
    fn render_answers_the_five_questions() {
        let r = sample();
        let md = r.render();
        for needle in [
            "## Goal",
            "## Actions",
            "## Evidence",
            "## Verification",
            "## Reproduction",
            "## Cost",
        ] {
            assert!(md.contains(needle), "missing {needle}");
        }
        assert!(md.contains("verified_complete"));
        assert!(md.contains("merkle-root-1"));
    }

    // --- P47.5 per-effect receipts (K1 layer) ---

    fn sample_effect() -> EffectReceipt {
        EffectReceipt::new(
            "e-1",
            "fs.write",
            "t-1",
            "abc123",
            "write src/parser.rs",
            "write src/parser.rs",
        )
        .with_resource("src/parser.rs")
        .with_refs("sha-before-1", "sha-after-1")
        .with_diff("@@ -12,4 +12,4 @@")
        .with_rollback("snap-7")
    }

    #[test]
    fn effect_receipt_records_requested_authorized_and_refs() {
        let e = sample_effect();
        assert_eq!(e.requested, "write src/parser.rs");
        assert_eq!(e.authorized, e.requested);
        assert_eq!(e.resource.as_deref(), Some("src/parser.rs"));
        assert_eq!(e.before_ref.as_deref(), Some("sha-before-1"));
        assert_eq!(e.after_ref.as_deref(), Some("sha-after-1"));
        assert_eq!(e.rollback_ref.as_deref(), Some("snap-7"));
        // Honesty defaults closed.
        assert!(!e.has_gap);
        assert!(e.uncertainty.is_none());
    }

    #[test]
    fn effect_receipt_has_gap_marks_uncertainty() {
        let mut e = sample_effect();
        e = e.gap("network payload could not be captured");
        assert!(e.has_gap);
        assert_eq!(
            e.uncertainty.as_deref(),
            Some("network payload could not be captured")
        );
        // A with a gap must not look like a fully-verified effect.
        assert_ne!(e.before_ref, e.after_ref);
    }

    #[test]
    fn work_receipt_carries_nested_effect_receipts_in_hash_and_render() {
        let mut r = sample();
        r.effects.push(sample_effect());

        // The nested layer is integral to the receipt's self-hash: tampering
        // with an effect proof breaks the whole receipt.
        let h = r.hash();
        let mut r2 = r.clone();
        r2.effects[0].authorized = "write DIFFERENT file".into();
        assert_ne!(h, r2.hash());
        assert!(WorkReceipt::verify(&h, &r));
        assert!(!WorkReceipt::verify(&h, &r2));

        // The render surfaces the per-effect proof + has_gap honesty.
        let md = r.render();
        assert!(md.contains("## Effects (1)"));
        assert!(md.contains("req=write src/parser.rs"));
        assert!(md.contains("auth=write src/parser.rs"));

        // An effect marked with a gap renders the honesty flag so a reader
        // never mistakes an unverified effect for one that provably happened.
        let mut gappy = sample();
        gappy
            .effects
            .push(sample_effect().gap("payload not captured"));
        let gmd = gappy.render();
        assert!(gmd.contains("has_gap"));
        assert!(gmd.contains("payload not captured"));
    }
}
