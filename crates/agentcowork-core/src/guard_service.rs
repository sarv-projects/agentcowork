//! P7.5 / J21 — the **GuardService**: the single executor-facing call-site
//! that composes the Guard-2 pieces into one deterministic pre-flight:
//!
//! 1. **estop** — pulled ⇒ refuse every privileged action.
//! 2. **hard floors** (P51.16/P51.29/P51.30) — critical `rm`, protected
//!    settings paths, unlocated deletes, human-only floors, and explicit
//!    tool-level denies. Every layer may only tighten.
//! 3. **policy** — `<data_dir>/permissions.toml` (`PermissionsPolicy`) maps
//!    the operation → Allow/Ask/Block.
//! 4. **profile** — minimal/standard/strict raises the human-approval
//!    threshold (`Profile::human_approval_threshold`).
//! 5. **reviewer** (P51.16) — may upgrade Ask→Allow only when configured
//!    (default budget is zero ⇒ never upgrades), never downgrades.
//! 6. **ticket** — an `Ask` mints a single-use [`AuthorizationTicket`] (with
//!    its [`DecisionPackage`] kept for the card), which the executor later
//!    consumes via [`GuardService::use_ticket`] before running.
//!
//! This is the "executor call-site that consumes tickets/estop/profiles" —
//! the coordinator drives it over JSON-RPC (`guard/*`), and the same state is
//! what the Tauri approval cards render.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use agentcowork_guard::{
    AuthorizationTicket, BatchOperation, BatchTicket, BatchTicketStore, DecisionPackage, Estop,
    GuardReceipt, Operation, PermissionsPolicy, PolicyAction, Profile, TicketStore,
    approval_policy::{Approval, ApprovalPolicy},
    floors::HumanFloor,
    protected_paths,
    reviewer::{ReviewOutcome, ReviewerBreaker, ReviewerConfig},
};
use serde::Serialize;
use serde_json::{Value, json};

/// The outcome of a pre-flight evaluation.
///
/// **Ticket-every-effect:** both `Allow` and `Ask` carry a single-use
/// [`AuthorizationTicket`] the executor must consume. `Allow` mints an
/// *already-approved* ticket (policy auto-approved — consumable immediately);
/// `Ask` mints a *pending* ticket the human must approve before the executor
/// can consume it. There is no ticketless mutation path.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum GuardDecision {
    /// Policy auto-approved — the ticket is already `Approved`, consumable now.
    Allow {
        #[serde(rename = "ticketId")]
        ticket_id: String,
    },
    /// A pending ticket was minted — the card renders; the executor waits.
    Ask {
        #[serde(rename = "ticketId")]
        ticket_id: String,
    },
    /// Refused; `reason` is rendered on the card.
    Block { reason: String },
}

/// A pending ticket + its decision package (the full card payload).
///
/// P51.20 — a named one-click permission bundle (Combo). `rules` are
/// canonical `permissions.toml` key → rule-string overrides; `tool_rules`
/// are (tool pattern, allow|ask|deny) entries for the tool allow-list
/// engine. Every bundle leaves the hard floors intact.
struct ComboBundle {
    name: &'static str,
    description: &'static str,
    rules: &'static [(&'static str, &'static str)],
    tool_rules: &'static [(&'static str, &'static str)],
}

const COMBO_BUNDLES: &[ComboBundle] = &[
    ComboBundle {
        name: "approve_then_read",
        description: "Sensitive-scope reads (vault/credential dirs, dotfile secrets, browser session data) require human approval — the default approve-then-read contract.",
        rules: &[("read_sensitive", "always_ask")],
        tool_rules: &[],
    },
    ComboBundle {
        name: "open_reads",
        description: "Sensitive-scope reads resolve without a card. Keeps every mutation gate unchanged.",
        rules: &[("read_sensitive", "always_allow")],
        tool_rules: &[],
    },
    ComboBundle {
        name: "block_sensitive_reads",
        description: "Refuse reads of sensitive scopes outright (vault/credentials/browser session data never reach the model).",
        rules: &[("read_sensitive", "block")],
        tool_rules: &[],
    },
    ComboBundle {
        name: "dev_workflow",
        description: "The everyday coding posture: workspace writes ask, destructive shell asks, new domains ask, sensitive reads ask.",
        rules: &[
            ("read_sensitive", "always_ask"),
            ("write", "always_ask"),
            ("terminal_shell", "ask_if_destructive"),
            ("external_network", "ask_if_new_domain"),
        ],
        tool_rules: &[],
    },
    ComboBundle {
        name: "hardened",
        description: "Everything meaningful asks (writes, deletes, web, multi-file, sensitive reads, new domains, destructive shell) — maximum human-in-the-loop without blocking the product.",
        rules: &[
            ("read_sensitive", "always_ask"),
            ("write", "always_ask"),
            ("delete_files", "always_ask"),
            ("web_action", "always_ask"),
            ("multi_file_edit", "always_ask"),
            ("external_network", "ask_if_new_domain"),
            ("terminal_shell", "ask_if_destructive"),
        ],
        tool_rules: &[],
    },
    ComboBundle {
        name: "browse_only",
        description: "Research mode: reads + browsing free, every mutation refused (write/delete/shell blocked), sensitive reads ask.",
        rules: &[
            ("read_sensitive", "always_ask"),
            ("write", "block"),
            ("delete_files", "block"),
            ("terminal_shell", "block"),
            ("web_action", "always_ask"),
            ("external_network", "ask_if_new_domain"),
        ],
        tool_rules: &[],
    },
];

/// A pending ticket + its decision package (the full card payload).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingGuardCard {
    pub ticket_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub tool_id: String,
    pub operation: String,
    pub paths: Vec<String>,
    pub risk: String,
    pub risk_tier: String,
    pub approval_source: String,
    /// Card-bound nonce required by the human approval command.
    pub approval_nonce: String,
    pub expires_at_ms: u64,
    /// P52.x (guard-UX wave) — why this ticket asked instead of auto-running.
    /// One stable machine-readable code + human sentence, e.g.
    /// `policy:ask op=write` / `profile:standard risk high>=high` /
    /// `confidence 0.62<0.85` / `floor:protected-settings` /
    /// `floor:human-only(op)` / `tier:R4 deny-by-default` /
    /// `reviewer:disabled(budget-zero)`. Skipped when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionPackage>,
}

/// P52.x (guard-UX wave) — the machine-readable why-asked code + human
/// sentence carried on a pending card. Pure data; no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskReason {
    pub code: String,
    pub detail: String,
}

impl AskReason {
    fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }

    /// Render `code detail` for the card line (`Why this asked: …`).
    pub fn render(&self) -> String {
        if self.detail.is_empty() {
            self.code.clone()
        } else {
            format!("{} {}", self.code, self.detail)
        }
    }
}

/// P52.x (guard-UX wave) — a push-style lifecycle event for the shell to fan
/// out over `guard-event`. The shell subscribes via
/// [`GuardService::subscribe_lifecycle`]; snapshots are cloned out from under
/// the lock so slow consumers can never block the executor.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuardLifecycle {
    Minted { ticket_id: String, batch: bool },
    Approved { ticket_id: String, batch: bool },
    Rejected { ticket_id: String, batch: bool },
    Expired { ticket_id: String, batch: bool },
}

/// P52.x (guard-UX wave) — human explanation for a Block reason: the guard
/// class, a safe alternative, and a one-click feedback hook label. Data only.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockExplanation {
    pub class: String,
    pub hint: String,
}

/// The executor's pre-flight state (estop + policy + profile + tickets).
pub struct GuardService {
    tickets: TicketStore,
    /// P47.6 — batch tickets (UC-1 "approve all"): an immutable change set
    /// approved as one unit, consumed via [`GuardService::use_batch_ticket`]
    /// with the exact change-set hash.
    batches: BatchTicketStore,
    policy: PermissionsPolicy,
    estop: Estop,
    profile: Profile,
    /// P51.16 — tool-level allow/ask/deny (deny-wins). Empty by default.
    approval_policy: ApprovalPolicy,
    /// P51.29 — human-only floors (protected-in-project, persistent authority).
    human_floor: HumanFloor,
    /// P51.16 — reviewer auto-allow config (default budget zero ⇒ disabled).
    reviewer_config: ReviewerConfig,
    /// P51.16 — reviewer circuit breaker.
    reviewer_breaker: ReviewerBreaker,
    /// ticket_id → the decision package that produced it (card rendering).
    decisions: HashMap<String, DecisionPackage>,
    /// P52.x — why-asked reason per pending ticket (card rendering).
    reasons: HashMap<String, AskReason>,
    /// P52.x — push-style lifecycle subscribers (shell `guard-event` fan-out).
    /// `std::sync::mpsc` senders: `send` never blocks the executor; a dead
    /// receiver is pruned on next emit.
    lifecycle: Vec<std::sync::mpsc::Sender<GuardLifecycle>>,
    /// Monotonic ticket-id source.
    counter: u64,
    /// H4: waiters blocked on Ask (ACP prompt).
    waiters: HashMap<String, mpsc::Sender<bool>>,
    outcomes: HashMap<String, bool>,
}

impl std::fmt::Debug for GuardService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardService")
            .field("counter", &self.counter)
            .finish_non_exhaustive()
    }
}

impl Default for GuardService {
    fn default() -> Self {
        Self {
            tickets: TicketStore::new(),
            batches: BatchTicketStore::new(),
            policy: PermissionsPolicy::default(),
            estop: Estop::new(),
            profile: Profile::Standard,
            approval_policy: ApprovalPolicy::default(),
            human_floor: HumanFloor::default(),
            reviewer_config: ReviewerConfig::new(1.01, 0),
            reviewer_breaker: ReviewerBreaker::new(3),
            decisions: HashMap::new(),
            reasons: HashMap::new(),
            lifecycle: Vec::new(),
            counter: 0,
            waiters: HashMap::new(),
            outcomes: HashMap::new(),
        }
    }
}

impl GuardService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load policy from a `permissions.toml` file (missing file ⇒ defaults).
    pub fn load_policy_from(&mut self, path: &Path) {
        if let Ok(doc) = std::fs::read_to_string(path) {
            self.policy = PermissionsPolicy::parse(&doc);
        }
    }

    pub fn policy(&self) -> &PermissionsPolicy {
        &self.policy
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub fn set_profile(&mut self, profile: Profile) {
        self.profile = profile;
    }

    /// P44.5 — apply an H34 autonomy level as a `permissions.toml` preset
    /// over the landed policy engine. This is the Rust half of the H34
    /// Autonomy calculator: the chatbar level maps to a fixed rule map +
    /// `min_confidence_for_auto`, and the hard floors (destructive,
    /// financial, new-domain external, high-risk shell) stay Ask/Block in
    /// every preset — never a Guard bypass.
    pub fn set_autonomy_level(&mut self, level: agentcowork_guard::AutonomyPreset) {
        self.policy = PermissionsPolicy::preset(level);
    }

    /// The H34 level currently applied (derived from the policy's shape;
    /// used by the autonomy indicator).
    pub fn autonomy_level(&self) -> agentcowork_guard::AutonomyPreset {
        for level in [
            agentcowork_guard::AutonomyPreset::Sandbox,
            agentcowork_guard::AutonomyPreset::Ask,
            agentcowork_guard::AutonomyPreset::Auto,
            agentcowork_guard::AutonomyPreset::Maximum,
        ] {
            if self.policy.is_preset(level) {
                return level;
            }
        }
        agentcowork_guard::AutonomyPreset::Ask
    }

    pub fn estop(&self) -> &Estop {
        &self.estop
    }

    /// P51.16 — replace the tool-level allow/ask/deny policy (deny-wins).
    pub fn set_approval_policy(&mut self, policy: ApprovalPolicy) {
        self.approval_policy = policy;
    }

    /// P51.29 — replace the human-only floors.
    pub fn set_human_floor(&mut self, floor: HumanFloor) {
        self.human_floor = floor;
    }

    /// P51.16 — configure reviewer auto-allow (default budget zero: disabled).
    pub fn set_reviewer_config(&mut self, config: ReviewerConfig) {
        self.reviewer_config = config;
    }

    /// P51.16 — reset the reviewer circuit breaker (e.g. after human review).
    pub fn reset_reviewer_breaker(&mut self) {
        self.reviewer_breaker.reset();
    }

    /// The **executor pre-flight**. Order matters: estop (hard stop) → policy
    /// (rule map) → profile (risk threshold) → confidence floor. **Every
    /// non-blocked outcome mints a single-use ticket**: `Ask` mints it
    /// `Pending` (human must approve), `Allow` mints it `Approved` (policy
    /// auto-approved). The executor consumes the ticket either way, so there
    /// is no ticketless mutation path.
    // (kept as explicit params — the arg count mirrors the ticket contract,
    // same as `agentcowork-guard::path_ticket`.)
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        session_id: &str,
        agent_id: &str,
        tool_id: &str,
        operation: Operation,
        decision: DecisionPackage,
        args_hash: &str,
        audit_seq: u64,
    ) -> GuardDecision {
        if self.estop.is_pulled() {
            return GuardDecision::Block {
                reason: "estop pulled".to_string(),
            };
        }

        // P51.16/P51.29/P51.30 — hard floors. Every layer may only tighten;
        // none can downgrade a Block or an Ask to Allow.
        let op_name = operation.name();
        let paths = &decision.affected_paths;
        // P51.30: critical `rm` (destructive shell op against /, ~, ., .git/
        // or a protected path) is refused outright.
        let shell_like = tool_id.contains("shell")
            || tool_id.contains("bash")
            || tool_id.contains("exec")
            || tool_id.contains("terminal")
            || tool_id.contains("run");
        if shell_like {
            if let Operation::TerminalShell { destructive: true } = operation {
                let targets: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
                if protected_paths::rm_critical("rm -r", &targets) {
                    return GuardDecision::Block {
                        reason: "critical rm target refused".to_string(),
                    };
                }
            }
        }
        // P51.30: our own settings paths always need a human on mutating
        // ops (Ask, never auto) — removal itself is refused by rm_critical.
        let protected_hit = matches!(
            operation,
            Operation::DeleteFiles
                | Operation::GenericWrite
                | Operation::MultiFileEdit { .. }
                | Operation::TerminalShell { .. }
        ) && paths.iter().any(|p| protected_paths::is_protected(p));
        // P51.29: a delete must name its targets (fail-closed unlocated).
        if matches!(operation, Operation::DeleteFiles) && paths.is_empty() {
            return GuardDecision::Block {
                reason: "delete without located paths refused".to_string(),
            };
        }
        // P51.29: human-only floors (protected-in-project prefixes,
        // persistent-authority ops) force Ask — never auto, any preset.
        let floor_ask = self.human_floor.requires_human(op_name, None)
            || protected_hit
            || paths
                .iter()
                .any(|p| self.human_floor.requires_human(op_name, Some(p)));
        // P51.16: an explicit tool-level Deny always blocks. Allow/Ask arms
        // are advisory (only Deny tightens), so a default-empty policy
        // changes nothing and presets keep their tested behavior.
        if matches!(self.approval_policy.evaluate(tool_id, ""), Approval::Deny) {
            return GuardDecision::Block {
                reason: format!("tool policy denies {tool_id}"),
            };
        }

        let policy_action = self.policy.evaluate(&operation);
        let needs_human = decision.risk >= self.profile.human_approval_threshold();
        // J21: a model-reported confidence below the policy floor forces the
        // auto path to ask, even when the rule map would otherwise allow it.
        let low_confidence = decision
            .confidence
            .map(|c| !self.policy.auto_confidence_ok(c))
            .unwrap_or(false);

        if policy_action == PolicyAction::Block {
            return GuardDecision::Block {
                reason: format!("policy denies {}", operation.name()),
            };
        }

        let tier =
            agentcowork_guard::RiskTier::from_risk_and_op(decision.risk, operation.name(), false);
        // R4 is deny-by-default: even a policy Allow still asks (explicit).
        let r4_ask = tier == agentcowork_guard::RiskTier::R4;
        // P52.x — why-asked: first firing cause wins (stable order), so the
        // card can say `Why this asked: …` instead of leaving the human to
        // guess. Order: floor → policy → tier → profile → confidence.
        let ask_reason = if floor_ask {
            if protected_hit {
                Some(AskReason::new(
                    "floor:protected-settings",
                    "this path is AgentCowork-protected — a human must approve",
                ))
            } else {
                Some(AskReason::new(
                    "floor:human-only",
                    format!("{op_name} needs a human in every preset"),
                ))
            }
        } else if policy_action == PolicyAction::Ask {
            Some(AskReason::new(
                "policy:ask",
                format!("permissions rule for {} says ask", operation.name()),
            ))
        } else if r4_ask {
            Some(AskReason::new(
                "tier:R4",
                "high-privilege credential/network/system — explicit by default",
            ))
        } else if needs_human {
            Some(AskReason::new(
                "profile",
                format!(
                    "profile {} asks at {:?} (this is {:?})",
                    self.profile.as_str(),
                    self.profile.human_approval_threshold(),
                    decision.risk
                ),
            ))
        } else if low_confidence {
            match decision.confidence {
                Some(c) => Some(AskReason::new(
                    "confidence",
                    format!(
                        "model confidence {c:.2} is below the auto floor {:.2}",
                        self.policy.min_confidence_for_auto
                    ),
                )),
                None => Some(AskReason::new(
                    "confidence",
                    "no model confidence reported — fail-closed to ask",
                )),
            }
        } else {
            None
        };
        let mut ask = policy_action == PolicyAction::Ask
            || needs_human
            || low_confidence
            || r4_ask
            || floor_ask;
        // P51.16: the reviewer may upgrade Ask→Allow only when configured
        // (default budget is zero ⇒ never upgrades) — never a downgrade.
        if ask
            && !matches!(policy_action, PolicyAction::Block)
            && matches!(
                agentcowork_guard::reviewer::auto_review(
                    decision.confidence,
                    &self.reviewer_config,
                    &self.reviewer_breaker,
                ),
                ReviewOutcome::AutoAllow
            )
        {
            ask = false;
        }
        self.counter += 1;
        let ticket_id = format!("tkt:{}", self.counter);
        let ticket = AuthorizationTicket {
            ticket_id: ticket_id.clone(),
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            tool_id: tool_id.to_string(),
            operation: operation.name().to_string(),
            args_hash: args_hash.to_string(),
            paths: decision.affected_paths.clone(),
            expires_at_ms: now_ms() + 60_000,
            single_use: true,
            approval_source: agentcowork_guard::ApprovalSource::Policy,
            approval_nonce: agentcowork_guard::ticket::new_approval_nonce(),
            risk: decision.risk,
            audit_seq,
            state: if ask {
                agentcowork_guard::TicketState::Pending
            } else {
                agentcowork_guard::TicketState::Approved
            },
            bindings: Vec::new(),
            execution_id: String::new(),
            action_id: tool_id.to_string(),
            idempotency_key: format!("{session_id}:{tool_id}:{args_hash}"),
        };
        self.tickets.mint(ticket);
        self.decisions.insert(ticket_id.clone(), decision);

        if ask {
            // P52.x — record the why-asked reason for the card. (A reviewer
            // upgrade that flipped ask→Allow lands in the else branch, so an
            // auto-run never carries a stale reason.)
            if let Some(r) = ask_reason.as_ref() {
                self.reasons.insert(ticket_id.clone(), r.clone());
            }
            self.emit_lifecycle(GuardLifecycle::Minted {
                ticket_id: ticket_id.clone(),
                batch: false,
            });
            GuardDecision::Ask { ticket_id }
        } else {
            GuardDecision::Allow { ticket_id }
        }
    }

    /// P47.6 — batch pre-flight (UC-1 "approve all"): the same gate
    /// (estop → policy → profile → confidence) over a whole change set. The
    /// ticket mints against the **immutable change-set hash** — approval
    /// covers exactly the operation list presented, never an operation
    /// category — and the executor consumes it with
    /// [`GuardService::use_batch_ticket`] presenting the identical hash.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_batch(
        &mut self,
        session_id: &str,
        agent_id: &str,
        operations: Vec<BatchOperation>,
        decision: DecisionPackage,
        audit_seq: u64,
    ) -> GuardDecision {
        if self.estop.is_pulled() {
            return GuardDecision::Block {
                reason: "estop pulled".to_string(),
            };
        }
        // The batch is one decision unit: policy evaluates the write class
        // (a batch is inherently a mutation of multiple resources), profile
        // uses the package risk, and the R-tier mapping uses the worst case.
        let policy_action = self.policy.evaluate(&Operation::GenericWrite);
        if policy_action == PolicyAction::Block {
            return GuardDecision::Block {
                reason: "policy denies batch write".to_string(),
            };
        }
        let needs_human = decision.risk >= self.profile.human_approval_threshold();
        let low_confidence = decision
            .confidence
            .map(|c| !self.policy.auto_confidence_ok(c))
            .unwrap_or(false);
        let tier = agentcowork_guard::RiskTier::from_risk_and_op(decision.risk, "batch", false);
        let r4_ask = tier == agentcowork_guard::RiskTier::R4;
        // P52.x — same why-asked order as evaluate (batch has no path floors;
        // it is one write-class decision unit).
        let batch_reason = if policy_action == PolicyAction::Ask {
            Some(AskReason::new(
                "policy:ask",
                "permissions rule for batch write says ask",
            ))
        } else if r4_ask {
            Some(AskReason::new(
                "tier:R4",
                "batch risk is high-privilege — explicit by default",
            ))
        } else if needs_human {
            Some(AskReason::new(
                "profile",
                format!(
                    "profile {} asks at {:?} (this batch is {:?})",
                    self.profile.as_str(),
                    self.profile.human_approval_threshold(),
                    decision.risk
                ),
            ))
        } else if low_confidence {
            match decision.confidence {
                Some(c) => Some(AskReason::new(
                    "confidence",
                    format!(
                        "model confidence {c:.2} is below the auto floor {:.2}",
                        self.policy.min_confidence_for_auto
                    ),
                )),
                None => Some(AskReason::new(
                    "confidence",
                    "no model confidence reported — fail-closed to ask",
                )),
            }
        } else {
            None
        };
        let ask = policy_action == PolicyAction::Ask || needs_human || low_confidence || r4_ask;

        self.counter += 1;
        let ticket_id = format!("btk:{}", self.counter);
        let mut ticket = BatchTicket::mint(
            ticket_id.clone(),
            agent_id,
            session_id,
            operations,
            decision.risk,
            audit_seq,
        );
        if !ask {
            ticket.state = agentcowork_guard::TicketState::Approved;
            ticket.approval_source = agentcowork_guard::ApprovalSource::Policy;
        }
        self.batches.mint(ticket);
        self.decisions.insert(ticket_id.clone(), decision);

        if ask {
            if let Some(r) = batch_reason.as_ref() {
                self.reasons.insert(ticket_id.clone(), r.clone());
            }
            self.emit_lifecycle(GuardLifecycle::Minted {
                ticket_id: ticket_id.clone(),
                batch: true,
            });
            GuardDecision::Ask { ticket_id }
        } else {
            GuardDecision::Allow { ticket_id }
        }
    }

    /// The **executor call-site** that consumes a ticket right before running
    /// a privileged action: estop must be clear, the ticket must be valid and
    /// the args must match (single-use). The executor still runs Guard-1 on
    /// the concrete args (separate gate).
    pub fn use_ticket(&mut self, ticket_id: &str, args_hash: &str) -> Result<(), String> {
        if self.estop.is_pulled() {
            return Err("estop pulled".to_string());
        }
        self.tickets
            .use_ticket(ticket_id, args_hash)
            .map_err(|e| e.to_string())
    }

    /// P47.6 — executor call-site for a batch ticket: estop must be clear and
    /// the presented change-set hash must equal the approved immutable set.
    pub fn use_batch_ticket(
        &mut self,
        ticket_id: &str,
        change_set_hash: &str,
    ) -> Result<(), String> {
        if self.estop.is_pulled() {
            return Err("estop pulled".to_string());
        }
        self.batches
            .use_batch_ticket(ticket_id, change_set_hash)
            .map_err(|e| e.to_string())
    }
    /// The approved change-set hash for a batch ticket (the executor presents
    /// it back at consume time; the card renders it for the human).
    pub fn batch_change_set_hash(&self, ticket_id: &str) -> Option<String> {
        self.batches
            .get(ticket_id)
            .map(|t| t.change_set_hash.clone())
    }

    /// The card-bound approval nonce for a batch ticket (same P10.2 rule as
    /// single tickets — the card bridge presents it back to approve).
    pub fn batch_approval_nonce(&self, ticket_id: &str) -> Option<String> {
        self.batches
            .get(ticket_id)
            .map(|t| t.approval_nonce.clone())
    }

    pub fn set_ticket_bindings(
        &mut self,
        ticket_id: &str,
        bindings: Vec<agentcowork_guard::ResourceBinding>,
    ) -> bool {
        self.tickets.set_bindings(ticket_id, bindings)
    }

    pub fn set_ticket_execution(&mut self, ticket_id: &str, execution_id: &str) -> bool {
        self.tickets.set_execution(ticket_id, execution_id)
    }

    pub fn ticket_bindings(&self, ticket_id: &str) -> Vec<agentcowork_guard::ResourceBinding> {
        self.tickets
            .get(ticket_id)
            .map(|t| t.bindings.clone())
            .unwrap_or_default()
    }

    /// Return the card-bound approval nonce without exposing the ticket
    /// itself. UI surfaces use this only to construct the approval card;
    /// approval still requires the nonce to be presented back to GuardService.
    pub fn approval_nonce(&self, ticket_id: &str) -> Option<&str> {
        self.tickets.approval_nonce(ticket_id)
    }

    pub fn approve(&mut self, ticket_id: &str) -> bool {
        let ok = self.tickets.approve(ticket_id);
        if ok {
            self.signal_ticket(ticket_id, true);
            self.emit_lifecycle(GuardLifecycle::Approved {
                ticket_id: ticket_id.to_string(),
                batch: false,
            });
        }
        ok
    }

    /// Human-facing approval path; the nonce is checked by the ticket store
    /// before the waiter is released.
    pub fn approve_with_nonce(&mut self, ticket_id: &str, nonce: &str) -> bool {
        let ok = self.tickets.approve_with_nonce(ticket_id, nonce);
        if ok {
            self.signal_ticket(ticket_id, true);
            self.emit_lifecycle(GuardLifecycle::Approved {
                ticket_id: ticket_id.to_string(),
                batch: false,
            });
        }
        ok
    }

    pub fn reject(&mut self, ticket_id: &str) -> bool {
        let ok = self.tickets.reject(ticket_id);
        if ok {
            self.signal_ticket(ticket_id, false);
            self.emit_lifecycle(GuardLifecycle::Rejected {
                ticket_id: ticket_id.to_string(),
                batch: false,
            });
        }
        ok
    }

    /// Human-facing rejection path; the nonce is checked by the ticket store.
    pub fn reject_with_nonce(&mut self, ticket_id: &str, nonce: &str) -> bool {
        let ok = self.tickets.reject_with_nonce(ticket_id, nonce);
        if ok {
            self.signal_ticket(ticket_id, false);
            self.emit_lifecycle(GuardLifecycle::Rejected {
                ticket_id: ticket_id.to_string(),
                batch: false,
            });
        }
        ok
    }

    /// P47.6 — human approval of a batch ticket (card-bound nonce, same rule
    /// as single tickets). Approves the whole immutable change set.
    pub fn approve_batch_with_nonce(&mut self, ticket_id: &str, nonce: &str) -> bool {
        let ok = self.batches.approve_with_nonce(ticket_id, nonce);
        if ok {
            self.signal_ticket(ticket_id, true);
            self.emit_lifecycle(GuardLifecycle::Approved {
                ticket_id: ticket_id.to_string(),
                batch: true,
            });
        }
        ok
    }

    /// P47.6 — internal approval for policy-controlled batch paths.
    pub fn approve_batch(&mut self, ticket_id: &str) -> bool {
        let ok = self.batches.approve(ticket_id);
        if ok {
            self.signal_ticket(ticket_id, true);
            self.emit_lifecycle(GuardLifecycle::Approved {
                ticket_id: ticket_id.to_string(),
                batch: true,
            });
        }
        ok
    }

    /// P47.6 — human rejection of a batch ticket (card-bound nonce).
    pub fn reject_batch_with_nonce(&mut self, ticket_id: &str, nonce: &str) -> bool {
        let ok = self.batches.reject_with_nonce(ticket_id, nonce);
        if ok {
            self.signal_ticket(ticket_id, false);
            self.emit_lifecycle(GuardLifecycle::Rejected {
                ticket_id: ticket_id.to_string(),
                batch: true,
            });
        }
        ok
    }

    fn signal_ticket(&mut self, ticket_id: &str, approved: bool) {
        self.outcomes.insert(ticket_id.to_string(), approved);
        if let Some(tx) = self.waiters.remove(ticket_id) {
            let _ = tx.send(approved);
        }
    }

    /// Watch an Ask ticket. If a human already decided, the receiver is
    /// pre-loaded. ACP `acp_prompt` blocks on this instead of deny-and-reprompt.
    pub fn watch_ticket(&mut self, ticket_id: &str) -> Receiver<bool> {
        if let Some(&v) = self.outcomes.get(ticket_id) {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(v);
            return rx;
        }
        let (tx, rx) = mpsc::channel();
        self.waiters.insert(ticket_id.to_string(), tx);
        rx
    }

    pub fn wait_ticket(&mut self, ticket_id: &str, timeout: Duration) -> bool {
        let rx = self.watch_ticket(ticket_id);
        rx.recv_timeout(timeout).unwrap_or(false)
    }

    pub fn pending(&self) -> Vec<PendingGuardCard> {
        self.tickets
            .pending()
            .into_iter()
            .map(|t| PendingGuardCard {
                ticket_id: t.ticket_id.clone(),
                agent_id: t.agent_id.clone(),
                session_id: t.session_id.clone(),
                tool_id: t.tool_id.clone(),
                operation: t.operation.clone(),
                paths: t.paths.clone(),
                risk: format!("{:?}", t.risk).to_lowercase(),
                risk_tier: agentcowork_guard::RiskTier::from_risk_and_op(
                    t.risk,
                    &t.operation,
                    false,
                )
                .as_str()
                .to_string(),
                approval_source: format!("{:?}", t.approval_source).to_lowercase(),
                approval_nonce: t.approval_nonce.clone(),
                expires_at_ms: t.expires_at_ms,
                reason: self.reasons.get(&t.ticket_id).map(|r| r.render()),
                decision: self.decisions.get(&t.ticket_id).cloned(),
            })
            .collect()
    }

    /// P52.x — subscribe a push-style lifecycle listener. Returns the
    /// receiver; the shell bridges each event to a `guard-event` emit.
    /// Senders never block the executor (`mpsc::send` on an unbounded
    /// channel cannot block); dead receivers are pruned on next emit.
    pub fn subscribe_lifecycle(&mut self) -> std::sync::mpsc::Receiver<GuardLifecycle> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.lifecycle.push(tx);
        rx
    }

    /// P52.x — fan one lifecycle snapshot out to every subscriber. Lock-free
    /// w.r.t. consumers: only `send` (non-blocking) happens here.
    fn emit_lifecycle(&mut self, ev: GuardLifecycle) {
        self.lifecycle.retain(|tx| tx.send(ev.clone()).is_ok());
    }

    /// P52.x — extend a Pending ticket's TTL (card `Extend 60s`). Only
    /// Pending tickets; re-mints the card nonce (the old card dies with it),
    /// caps each extension at 60s and total lifetime at 5min from now.
    /// Never touches Approved/Used/Revoked/Expired state or floors.
    pub fn extend_ticket_ttl(
        &mut self,
        ticket_id: &str,
        extra_ms: u64,
    ) -> Result<(u64, String), String> {
        const STEP_CAP_MS: u64 = 60_000;
        const TOTAL_CAP_MS: u64 = 5 * 60_000;
        let extra = extra_ms.min(STEP_CAP_MS);
        if extra == 0 {
            return Err("extension must be > 0ms".to_string());
        }
        let now = now_ms();
        if ticket_id.starts_with("btk:") {
            let t = self
                .batches
                .get_mut(ticket_id)
                .ok_or_else(|| format!("unknown batch ticket: {ticket_id}"))?;
            if t.state != agentcowork_guard::TicketState::Pending {
                return Err("only pending tickets can be extended".to_string());
            }
            let new_exp = (t.expires_at_ms.max(now) + extra).min(now + TOTAL_CAP_MS);
            t.expires_at_ms = new_exp;
            t.approval_nonce = agentcowork_guard::ticket::new_approval_nonce();
            let out = (new_exp, t.approval_nonce.clone());
            self.emit_lifecycle(GuardLifecycle::Minted {
                ticket_id: ticket_id.to_string(),
                batch: true,
            });
            return Ok(out);
        }
        let t = self
            .tickets
            .get_mut(ticket_id)
            .ok_or_else(|| format!("unknown ticket: {ticket_id}"))?;
        if t.state != agentcowork_guard::TicketState::Pending {
            return Err("only pending tickets can be extended".to_string());
        }
        let new_exp = (t.expires_at_ms.max(now) + extra).min(now + TOTAL_CAP_MS);
        t.expires_at_ms = new_exp;
        t.approval_nonce = agentcowork_guard::ticket::new_approval_nonce();
        let out = (new_exp, t.approval_nonce.clone());
        self.emit_lifecycle(GuardLifecycle::Minted {
            ticket_id: ticket_id.to_string(),
            batch: false,
        });
        Ok(out)
    }

    /// P52.x — human explanation for a Block reason (Guard-1 opacity fix).
    /// Pure string mapping over the stable reason vocabulary minted in
    /// `evaluate`/`evaluate_batch`; unknown reasons map to a generic class.
    /// No authority, no policy change.
    pub fn explain_block(reason: &str) -> BlockExplanation {
        let r = reason.to_lowercase();
        if r.contains("estop") {
            BlockExplanation {
                class: "emergency-stop".to_string(),
                hint: "The emergency stop is pulled — reset it in the Guard panel before retrying."
                    .to_string(),
            }
        } else if r.contains("critical rm") {
            BlockExplanation {
                class: "destructive-shell".to_string(),
                hint: "This delete targets /, ~, ., .git or an AgentCowork-protected path. Narrow the target to the workspace folder and retry.".to_string(),
            }
        } else if r.contains("without located paths") {
            BlockExplanation {
                class: "unlocated-delete".to_string(),
                hint: "Deletes must name their targets. Re-run with the exact file or folder path."
                    .to_string(),
            }
        } else if r.contains("tool policy denies") {
            BlockExplanation {
                class: "tool-denied".to_string(),
                hint: "Your tool allow-list denies this tool. Change the rule to ask (or remove the deny) in Guard → Tool Allow-list.".to_string(),
            }
        } else if r.contains("policy denies") {
            BlockExplanation {
                class: "policy-denied".to_string(),
                hint: "permissions.toml denies this operation class. Switch autonomy preset or edit the policy, then retry.".to_string(),
            }
        } else if r.contains("mcp attach blocked") || r.contains("mcp") {
            BlockExplanation {
                class: "connector-blocked".to_string(),
                hint: "The connector attach was refused. Check the server command/args and attach again from Connectors.".to_string(),
            }
        } else {
            BlockExplanation {
                class: "blocked".to_string(),
                hint: "Refused by the guard. Open the Guard panel for the policy detail, or send this to Feedback.".to_string(),
            }
        }
    }

    pub fn receipts(&self) -> Vec<GuardReceipt> {
        self.tickets.receipts().to_vec()
    }

    /// P47.6 — pending batch tickets (the "approve all" card renders these).
    pub fn pending_batches(&self) -> Vec<agentcowork_guard::BatchTicket> {
        self.batches.pending().into_iter().cloned().collect()
    }

    /// P47.6 — append-only batch approval/denial receipts.
    pub fn batch_receipts(&self) -> Vec<agentcowork_guard::BatchReceipt> {
        self.batches.receipts().to_vec()
    }

    /// JSON-RPC dispatch (`guard/*`) for the **Tauri UI / control plane**.
    /// Exposes the full surface (approve/reject/estop/reset/profile) because
    /// the webview is the human-in-the-loop surface.
    pub fn handle(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.handle_inner(method, params, true)
    }

    /// JSON-RPC dispatch for the **coordinator sidecar** (less trusted). The
    /// sidecar may pre-flight (`evaluate`), consume a ticket (`use`), and read
    /// (`pending`/`receipts`/`estop_status`/`policy`) — but it may **not**
    /// approve/reject its own tickets, pull/reset estop, or change the
    /// security profile. Those are human-only control-plane operations.
    pub fn handle_sidecar(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.handle_inner(method, params, false)
    }

    fn handle_inner(
        &mut self,
        method: &str,
        params: &Value,
        control_plane: bool,
    ) -> Result<Value, String> {
        if !control_plane {
            match method {
                "guard/approve" | "guard/reject" | "guard/estop" | "guard/reset"
                | "guard/profile" => {
                    return Err(format!(
                        "{method} is a control-plane operation, not available to the sidecar"
                    ));
                }
                _ => {}
            }
        }
        match method {
            "guard/evaluate" => {
                let session = str_param(params, "sessionId").unwrap_or("default");
                let agent = str_param(params, "agentId").unwrap_or("agent");
                let tool = str_param(params, "toolId").unwrap_or("");
                let op = parse_operation(params)?;
                let decision: DecisionPackage = params
                    .get("decision")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| e.to_string())?
                    .unwrap_or_default();
                let args_hash = str_param(params, "argsHash").unwrap_or("").to_string();
                let audit_seq = params.get("auditSeq").and_then(Value::as_u64).unwrap_or(0);
                let out = self.evaluate(session, agent, tool, op, decision, &args_hash, audit_seq);
                Ok(serde_json::to_value(&out).map_err(|e| e.to_string())?)
            }
            "guard/use" => {
                let id = str_param(params, "ticketId").ok_or("guard/use requires ticketId")?;
                let args_hash = str_param(params, "argsHash").unwrap_or("").to_string();
                self.use_ticket(id, &args_hash)?;
                Ok(json!({ "consumed": true }))
            }
            "guard/approve" => {
                let id = str_param(params, "ticketId").ok_or("guard/approve requires ticketId")?;
                let nonce = str_param(params, "approvalNonce")
                    .ok_or("guard/approve requires approvalNonce")?;
                Ok(json!({ "approved": self.approve_with_nonce(id, nonce) }))
            }
            "guard/reject" => {
                let id = str_param(params, "ticketId").ok_or("guard/reject requires ticketId")?;
                let nonce = str_param(params, "approvalNonce")
                    .ok_or("guard/reject requires approvalNonce")?;
                Ok(json!({ "rejected": self.reject_with_nonce(id, nonce) }))
            }
            "guard/estop" => {
                self.estop.pull();
                Ok(json!({ "pulled": true }))
            }
            "guard/reset" => {
                self.estop.reset();
                Ok(json!({ "pulled": false }))
            }
            "guard/estop_status" => Ok(json!({ "pulled": self.estop.is_pulled() })),
            "guard/pending" => {
                let cards = self.pending();
                Ok(serde_json::to_value(&cards).map_err(|e| e.to_string())?)
            }
            "guard/receipts" => {
                let r: Vec<GuardReceipt> = self.receipts();
                Ok(serde_json::to_value(&r).map_err(|e| e.to_string())?)
            }
            "guard/profile" => {
                if let Some(p) = params.get("profile").and_then(Value::as_str) {
                    let profile = parse_profile(p)?;
                    self.set_profile(profile);
                    Ok(json!({ "profile": p }))
                } else {
                    Ok(json!({ "profile": self.profile().as_str() }))
                }
            }
            "guard/policy" => {
                // Summary of the loaded policy (for the Settings guard panel),
                // incl. the applied H34 autonomy level (P44.5) and the
                // P51.22 tool allow-list rules (deny-wins, args-glob aware).
                let rules: Vec<Value> = self
                    .approval_policy
                    .rules
                    .iter()
                    .map(|(pat, approval)| {
                        json!({
                            "tool": pat.tool,
                            "argsGlob": pat.args_glob,
                            "approval": match approval {
                                Approval::Allow => "allow",
                                Approval::Ask => "ask",
                                Approval::Deny => "deny",
                            },
                        })
                    })
                    .collect();
                Ok(json!({
                    "minConfidenceForAuto": self.policy.min_confidence_for_auto,
                    "userFeedbackLearning": self.policy.user_feedback_learning,
                    "profile": self.profile().as_str(),
                    "estopPulled": self.estop.is_pulled(),
                    "autonomyLevel": self.autonomy_level().as_str(),
                    "approvalRules": rules,
                    // P52.x — live trust state for the panel (no fixtures):
                    // the profile's human-approval threshold name + the
                    // reviewer auto-allow budget (0 = disabled, the default).
                    "humanApprovalThreshold": format!("{:?}", self.profile.human_approval_threshold()).to_lowercase(),
                    "reviewerBudget": self.reviewer_config.max_auto_per_run,
                    "reviewerConfidenceFloor": self.reviewer_config.confidence_floor,
                }))
            }
            "guard/set_policy_rules" => {
                // P51.22 — replace the tool allow-list rules (Allow/Ask/Deny
                // per tool pattern + optional args glob). Only explicit rules
                // are ever tightened; the deny-wins engine and the hard floors
                // (P51.16/29/30) stay untouched, so this can only reduce the
                // auto path — never widen it past the autonomy preset.
                let raw = params
                    .get("rules")
                    .and_then(Value::as_array)
                    .ok_or("guard/set_policy_rules requires rules[]")?;
                let mut rules = Vec::new();
                for r in raw {
                    let tool = r
                        .get("tool")
                        .and_then(Value::as_str)
                        .ok_or("rule requires tool")?
                        .to_string();
                    if tool.is_empty() {
                        return Err("rule tool must not be empty".into());
                    }
                    let args_glob = r
                        .get("argsGlob")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let approval = match r.get("approval").and_then(Value::as_str) {
                        Some("allow") => Approval::Allow,
                        Some("ask") => Approval::Ask,
                        Some("deny") => Approval::Deny,
                        _ => return Err("approval must be allow|ask|deny".into()),
                    };
                    rules.push((
                        agentcowork_guard::approval_policy::ToolPattern::new(
                            &tool,
                            args_glob.as_deref(),
                        ),
                        approval,
                    ));
                }
                self.set_approval_policy(ApprovalPolicy::new(rules));
                Ok(json!({ "applied": raw.len() }))
            }
            // P51.20 — one-click permission bundles (Combos). A combo is a
            // named set of canonical `permissions.toml` rule overrides (plus
            // optional tool allow-list rules) applied through the same
            // engines as every other surface — never a bypass around the
            // hard floors (destructive/delete, secrets, financial, security
            // changes, irreversible external effects stay Ask-or-worse in
            // every bundle).
            "guard/combos" => Ok(json!({
                "combos": COMBO_BUNDLES.iter().map(|c| json!({
                    "name": c.name,
                    "description": c.description,
                    "rules": c.rules,
                    "toolRules": c.tool_rules,
                })).collect::<Vec<_>>(),
            })),
            "guard/apply_combo" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("guard/apply_combo requires name")?;
                let combo = COMBO_BUNDLES
                    .iter()
                    .find(|c| c.name == name)
                    .ok_or_else(|| format!("unknown combo: {name}"))?;
                let rules: Vec<(&'static str, &'static str)> =
                    combo.rules.iter().map(|&(k, v)| (k, v)).collect();
                let applied = self.policy.apply_rules(&rules)?;
                // Tool allow-list rules ride the same deny-wins engine as
                // `guard/set_policy_rules`.
                if !combo.tool_rules.is_empty() {
                    let mut tr = Vec::new();
                    for &(tool, approval) in combo.tool_rules {
                        tr.push((
                            agentcowork_guard::approval_policy::ToolPattern::new(tool, None),
                            match approval {
                                "allow" => Approval::Allow,
                                "ask" => Approval::Ask,
                                _ => Approval::Deny,
                            },
                        ));
                    }
                    self.approval_policy = ApprovalPolicy::new(tr);
                }
                Ok(json!({ "applied": applied, "combo": name }))
            }
            "guard/autonomy" => {
                // The currently applied H34 autonomy level (P44.5 preset) +
                // its confidence floor — what the UI indicator must read.
                let level = self.autonomy_level();
                Ok(json!({
                    "autonomyLevel": level.as_str(),
                    "minConfidenceForAuto": self.policy.min_confidence_for_auto,
                }))
            }
            "guard/set_autonomy" => {
                // Apply an H34 level as a permissions.toml preset (never a
                // Guard bypass — the hard floors stay). Returns the applied
                // level + floor so the UI can confirm.
                let name = str_param(params, "level").ok_or("guard/set_autonomy requires level")?;
                let level = agentcowork_guard::AutonomyPreset::parse(name)
                    .ok_or_else(|| format!("unknown autonomy level: {name}"))?;
                self.set_autonomy_level(level);
                Ok(json!({
                    "autonomyLevel": level.as_str(),
                    "minConfidenceForAuto": self.policy.min_confidence_for_auto,
                }))
            }
            "guard/ticket_status" => {
                let id =
                    str_param(params, "ticketId").ok_or("guard/ticket_status requires ticketId")?;
                match self.tickets.get(id) {
                    Some(t) => Ok(json!({
                        "ticketId": t.ticket_id,
                        "state": format!("{:?}", t.state).to_lowercase(),
                    })),
                    None => Ok(json!({ "ticketId": id, "state": "unknown" })),
                }
            }
            "guard/extend_ttl" => {
                // Control-plane only (human card action): Pending-gated,
                // nonce-rotating TTL extension. The sidecar must never extend
                // its own wait.
                if !control_plane {
                    return Err(
                        "guard/extend_ttl is a control-plane operation, not available to the sidecar"
                            .to_string(),
                    );
                }
                let id =
                    str_param(params, "ticketId").ok_or("guard/extend_ttl requires ticketId")?;
                let extra = params
                    .get("extraMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(60_000);
                let (expires_at_ms, approval_nonce) = self.extend_ticket_ttl(id, extra)?;
                Ok(json!({
                    "ticketId": id,
                    "expiresAtMs": expires_at_ms,
                    "approvalNonce": approval_nonce,
                }))
            }
            "guard/explain_block" => {
                let reason = str_param(params, "reason").unwrap_or("");
                let exp = Self::explain_block(reason);
                Ok(json!({ "class": exp.class, "hint": exp.hint }))
            }
            _ => Err(format!("method not found: {method}")),
        }
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// Current unix time in ms.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn parse_profile(s: &str) -> Result<Profile, String> {
    match s {
        "minimal" => Ok(Profile::Minimal),
        "standard" => Ok(Profile::Standard),
        "strict" => Ok(Profile::Strict),
        other => Err(format!("unknown profile: {other}")),
    }
}

fn parse_operation(params: &Value) -> Result<Operation, String> {
    let name = str_param(params, "operation").ok_or("guard/evaluate requires operation")?;
    Ok(match name {
        "delete" => Operation::DeleteFiles,
        "multi_file_edit" => Operation::MultiFileEdit {
            files: params.get("files").and_then(Value::as_u64).unwrap_or(0) as usize,
        },
        "external_network" => Operation::ExternalNetwork {
            new_domain: params
                .get("newDomain")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        },
        "terminal_shell" => Operation::TerminalShell {
            destructive: params
                .get("destructive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "web_action" => Operation::WebAction,
        "write" => Operation::GenericWrite,
        other => return Err(format!("unknown operation: {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_guard::RiskLevel;

    fn decision(risk: RiskLevel, paths: &[&str]) -> DecisionPackage {
        DecisionPackage::new("test goal")
            .with_risk(risk)
            .with_paths(paths.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn estop_blocks_everything_first() {
        let mut g = GuardService::new();
        g.estop.pull();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::Low, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Block { ref reason } if reason == "estop pulled"));
    }

    #[test]
    fn delete_asks_and_mints_consumable_ticket() {
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/workspace/x"]),
            "args-h",
            7,
        );
        let ticket_id = match d {
            GuardDecision::Ask { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Ask, got {other:?}"),
        };

        // Card payload carries the decision package + the why-asked reason.
        let cards = g.pending();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].ticket_id, ticket_id);
        assert!(cards[0].decision.is_some());
        assert_eq!(cards[0].decision.as_ref().unwrap().goal, "test goal");
        let reason = cards[0].reason.clone().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "every Ask card must carry a why-asked reason"
        );

        // Approve then consume (single-use; args must match).
        assert!(g.approve(&ticket_id));
        assert!(g.use_ticket(&ticket_id, "args-h").is_ok());
        assert!(g.use_ticket(&ticket_id, "args-h").is_err());
    }

    #[test]
    fn p52_why_asked_reason_stable_per_cause() {
        // policy:ask — generic write defaults to always_ask under the
        // default policy.
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/workspace/a.txt"]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Ask { .. }));
        let reason = g.pending()[0].reason.clone().unwrap_or_default();
        assert!(
            reason.starts_with("policy:ask"),
            "expected policy:ask, got {reason}"
        );

        // profile — allow-write policy + high risk still asks via the
        // standard profile threshold.
        let mut g2 = GuardService::new();
        g2.policy = PermissionsPolicy::parse("[permissions]\nwrite = \"allow\"\n");
        let d2 = g2.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::High, &["/workspace/a.txt"]),
            "h",
            0,
        );
        assert!(matches!(d2, GuardDecision::Ask { .. }));
        let r2 = g2.pending()[0].reason.clone().unwrap_or_default();
        assert!(r2.starts_with("profile"), "expected profile, got {r2}");

        // floor — protected settings paths always ask, any preset.
        let mut g3 = GuardService::new();
        g3.policy = PermissionsPolicy::parse("[permissions]\nwrite = \"allow\"\n");
        let d3 = g3.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["~/.agentcowork/vault.db"]),
            "h",
            0,
        );
        assert!(matches!(d3, GuardDecision::Ask { .. }));
        let r3 = g3.pending()[0].reason.clone().unwrap_or_default();
        assert!(r3.starts_with("floor:"), "expected floor:, got {r3}");
    }

    #[test]
    fn p52_lifecycle_hook_fires_on_mint_approve_reject() {
        let mut g = GuardService::new();
        let rx = g.subscribe_lifecycle();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/workspace/x"]),
            "h",
            0,
        );
        let tid = match d {
            GuardDecision::Ask { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Ask, got {other:?}"),
        };
        match rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap() {
            GuardLifecycle::Minted { ticket_id, batch } => {
                assert_eq!(ticket_id, tid);
                assert!(!batch);
            }
            other => panic!("expected Minted, got {other:?}"),
        }
        assert!(g.approve(&tid));
        match rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap() {
            GuardLifecycle::Approved { ticket_id, batch } => {
                assert_eq!(ticket_id, tid);
                assert!(!batch);
            }
            other => panic!("expected Approved, got {other:?}"),
        }

        let d2 = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/workspace/y"]),
            "h2",
            0,
        );
        let tid2 = match d2 {
            GuardDecision::Ask { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Ask, got {other:?}"),
        };
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            GuardLifecycle::Minted { .. }
        ));
        assert!(g.reject(&tid2));
        match rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap() {
            GuardLifecycle::Rejected { ticket_id, batch } => {
                assert_eq!(ticket_id, tid2);
                assert!(!batch);
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn p52_extend_ttl_renonces_and_caps() {
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/workspace/x"]),
            "h",
            0,
        );
        let tid = match d {
            GuardDecision::Ask { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Ask, got {other:?}"),
        };
        let before = g.pending()[0].clone();
        let (exp, nonce) = g.extend_ticket_ttl(&tid, 60_000).unwrap();
        assert!(exp >= before.expires_at_ms);
        assert_ne!(nonce, before.approval_nonce);
        // Old nonce dies with the rotation.
        assert!(!g.approve_with_nonce(&tid, &before.approval_nonce));
        assert!(g.approve_with_nonce(&tid, &nonce));
        // Non-pending tickets refuse extension.
        assert!(g.extend_ticket_ttl(&tid, 60_000).is_err());
        // Unknown tickets refuse extension.
        assert!(g.extend_ticket_ttl("tkt:nope", 60_000).is_err());
        // Zero extension refused.
        let mut g2 = GuardService::new();
        let d2 = g2.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/workspace/x"]),
            "h",
            0,
        );
        let tid2 = match d2 {
            GuardDecision::Ask { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Ask, got {other:?}"),
        };
        assert!(g2.extend_ticket_ttl(&tid2, 0).is_err());
    }

    #[test]
    fn p52_explain_block_covers_block_vocabulary() {
        for (reason, class) in [
            ("estop pulled", "emergency-stop"),
            ("critical rm target refused", "destructive-shell"),
            ("delete without located paths refused", "unlocated-delete"),
            ("tool policy denies shell.exec", "tool-denied"),
            ("policy denies write", "policy-denied"),
        ] {
            let exp = GuardService::explain_block(reason);
            assert_eq!(exp.class, class, "reason: {reason}");
            assert!(!exp.hint.is_empty());
        }
        let generic = GuardService::explain_block("something new");
        assert_eq!(generic.class, "blocked");
    }

    #[test]
    fn p52_sidecar_cannot_extend_ttl() {
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/workspace/x"]),
            "h",
            0,
        );
        let tid = match d {
            GuardDecision::Ask { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Ask, got {other:?}"),
        };
        let out = g.handle_sidecar("guard/extend_ttl", &serde_json::json!({ "ticketId": tid }));
        assert!(out.is_err(), "sidecar must not extend its own wait");
        // Control plane can.
        let ok = g
            .handle("guard/extend_ttl", &serde_json::json!({ "ticketId": tid }))
            .unwrap();
        assert!(ok.get("approvalNonce").is_some());
    }

    #[test]
    fn low_risk_write_auto_allows_under_standard_profile() {
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/workspace/a.txt"]),
            "h",
            0,
        );
        // Generic write defaults to always_ask policy → Ask, still.
        assert!(
            matches!(d, GuardDecision::Ask { .. }),
            "generic write defaults to always_ask policy; got {d:?}"
        );
    }

    #[test]
    fn allow_mints_an_auto_approved_ticket() {
        // An auto-allowed action still mints a single-use ticket, but it is
        // pre-Approved so the executor can consume it without a human wait.
        let mut g = GuardService::new();
        g.policy = PermissionsPolicy::parse("[permissions]\nwrite = \"allow\"\n");
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/workspace/a.txt"]),
            "h",
            0,
        );
        let ticket_id = match d {
            GuardDecision::Allow { ref ticket_id } => ticket_id.clone(),
            other => panic!("expected Allow, got {other:?}"),
        };
        // Directly consumable (already Approved), single-use.
        assert!(g.use_ticket(&ticket_id, "h").is_ok());
        assert!(g.use_ticket(&ticket_id, "h").is_err());
    }

    #[test]
    fn low_confidence_forces_ask() {
        let mut g = GuardService::new();
        g.policy = PermissionsPolicy::parse(
            "[permissions]\nwrite = \"allow\"\nmin_confidence_for_auto = 0.90\n",
        );
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            DecisionPackage::new("g")
                .with_risk(RiskLevel::Low)
                .with_confidence(0.5),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Ask { .. }),
            "low confidence must force Ask; got {d:?}"
        );
    }

    #[test]
    fn sidecar_surface_rejects_control_plane_ops() {
        let mut g = GuardService::new();
        // The sidecar may evaluate + use + read…
        assert!(
            g.handle_sidecar(
                "guard/evaluate",
                &json!({
                    "operation": "delete", "argsHash": "h", "decision": { "risk": "high" }
                })
            )
            .is_ok()
        );
        assert!(g.handle_sidecar("guard/estop_status", &json!({})).is_ok());
        // …but may NOT approve/reset/estop/profile.
        assert!(
            g.handle_sidecar("guard/approve", &json!({ "ticketId": "tkt:1" }))
                .is_err()
        );
        assert!(g.handle_sidecar("guard/reset", &json!({})).is_err());
        assert!(g.handle_sidecar("guard/estop", &json!({})).is_err());
        assert!(
            g.handle_sidecar("guard/profile", &json!({ "profile": "minimal" }))
                .is_err()
        );
        // The control-plane handle still allows them (the UI path).
        assert!(g.handle("guard/estop", &json!({})).is_ok());
    }

    #[test]
    fn strict_profile_raises_threshold() {
        let mut g = GuardService::new();
        g.set_profile(Profile::Strict);
        // Under strict, Medium+ needs human approval even when policy allows.
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Medium, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Ask { .. }));
    }

    #[test]
    fn policy_block_refuses_outright() {
        let mut g = GuardService::new();
        g.policy = PermissionsPolicy::parse("[permissions]\nterminal_shell = \"block\"\n");
        let d = g.evaluate(
            "s1",
            "a1",
            "shell.exec",
            Operation::TerminalShell { destructive: true },
            decision(RiskLevel::High, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Block { .. }));
    }

    #[test]
    fn handle_dispatches_evaluate_and_use() {
        let mut g = GuardService::new();
        let out = g
            .handle(
                "guard/evaluate",
                &json!({
                    "sessionId": "s1", "agentId": "a1", "toolId": "fs.delete",
                    "operation": "delete", "argsHash": "h1", "auditSeq": 3,
                    "decision": { "goal": "rm", "risk": "high", "affectedPaths": ["/w/x"] }
                }),
            )
            .unwrap();
        let ticket_id = out["ticketId"].as_str().unwrap().to_string();
        assert_eq!(out["action"], "ask");

        // A pending ticket must not be consumable until approved.
        assert!(
            g.handle(
                "guard/use",
                &json!({ "ticketId": ticket_id, "argsHash": "h1" })
            )
            .is_err()
        );
        let approval_nonce = g.pending()[0].approval_nonce.clone();
        g.handle(
            "guard/approve",
            &json!({ "ticketId": ticket_id, "approvalNonce": approval_nonce }),
        )
        .unwrap();
        let used = g
            .handle(
                "guard/use",
                &json!({ "ticketId": ticket_id, "argsHash": "h1" }),
            )
            .unwrap();
        assert_eq!(used["consumed"], true);

        // estop then re-evaluate → blocked.
        g.handle("guard/estop", &json!({})).unwrap();
        let blocked = g
            .handle(
                "guard/evaluate",
                &json!({ "operation": "delete", "argsHash": "h2", "decision": { "risk": "high" } }),
            )
            .unwrap();
        assert_eq!(blocked["action"], "block");
    }

    // --- P48.2 adversarial two-path boundary (anti-impersonation) ---

    #[test]
    fn sidecar_cannot_self_approve_its_own_pending_ticket() {
        // (a) + (c): a ticket minted via the sidecar path (Ask) is Pending.
        // The sidecar surface cannot approve it (control-plane refused), so a
        // self-issued approval is impossible — the only pre-approved tickets
        // come from an `Allow` decision the policy authorized deterministically.
        let mut g = GuardService::new();
        let out = g
            .handle_sidecar(
                "guard/evaluate",
                &json!({
                    "operation": "delete", "argsHash": "h",
                    // Located target: the P51.29 unlocated-delete floor
                    // refuses pathless deletes, so this self-approval test
                    // names its target (its subject is the approve path,
                    // not the floor).
                    "decision": { "risk": "high", "affectedPaths": ["/w/x"] }
                }),
            )
            .unwrap();
        let ticket_id = out["ticketId"].as_str().unwrap().to_string();
        assert_eq!(out["action"], "ask");

        // Sidecar cannot approve its own ticket by any control-plane route.
        assert!(
            g.handle_sidecar("guard/approve", &json!({ "ticketId": ticket_id }))
                .is_err()
        );
        // And the pending ticket is not consumable until a real approval (which
        // only the control-plane handle can reach, with the nonce).
        assert!(
            g.handle_sidecar(
                "guard/use",
                &json!({ "ticketId": ticket_id, "argsHash": "h" })
            )
            .is_err()
        );
    }

    #[test]
    fn forged_decision_source_cannot_pre_approve_a_ticket() {
        // (c): the decision package's `approvalSource`/risk are inputs, but the
        // Ask-vs-Allow split is decided by POLICY + profile — a sidecar cannot
        // smuggle "this was already approved" by setting a fake source on the
        // decision it attaches to an Ask-scoped operation.
        let mut g = GuardService::new();
        let sneaky = DecisionPackage::new("rm -rf /")
            .with_risk(RiskLevel::Medium)
            .with_paths(vec!["/w/x".into()]);
        // A Medium delete under default (Standard) profile is below the strict
        // threshold — but a *delete* is still an Ask operation, never Allow,
        // regardless of what the decision source claims.
        let out = g
            .handle(
                "guard/evaluate",
                &json!({
                    "operation": "delete",
                    "argsHash": "h",
                    "decision": serde_json::to_value(&sneaky).unwrap(),
                }),
            )
            .unwrap();
        // Only the control-plane handle can evaluate; still Ask (pending).
        assert_eq!(out["action"], "ask");
        let ticket_id = out["ticketId"].as_str().unwrap().to_string();
        // Not consumable without approval + nonce.
        assert!(
            g.handle(
                "guard/use",
                &json!({ "ticketId": ticket_id, "argsHash": "h" })
            )
            .is_err()
        );
    }

    #[test]
    fn estop_cannot_be_cleared_by_sidecar() {
        // (c): estop is a hard trip; only the control plane can pull it, and a
        // compromised sidecar can neither pull nor clear it.
        let mut g = GuardService::new();
        g.handle("guard/estop", &json!({})).unwrap();
        assert_eq!(
            g.handle_sidecar("guard/estop_status", &json!({})).unwrap()["pulled"],
            true
        );
        // Sidecar sees it pulled but cannot reset it.
        assert!(g.handle_sidecar("guard/reset", &json!({})).is_err());
        // Control-plane reset works (human path only).
        g.handle("guard/reset", &json!({})).unwrap();
        assert_eq!(
            g.handle_sidecar("guard/estop_status", &json!({})).unwrap()["pulled"],
            false
        );
    }

    #[test]
    fn ask_wait_unblocks_on_approve() {
        use std::sync::{Arc, Mutex};
        let g = Arc::new(Mutex::new(GuardService::new()));
        let d = g.lock().unwrap().evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/w/x"]),
            "h",
            0,
        );
        let GuardDecision::Ask { ticket_id } = d else {
            panic!("expected Ask");
        };
        let rx = g.lock().unwrap().watch_ticket(&ticket_id);
        let g2 = Arc::clone(&g);
        let tid = ticket_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            assert!(g2.lock().unwrap().approve(&tid));
        });
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
    }

    #[test]
    fn batch_flow_asks_approves_and_consumes_exact_change_set() {
        let mut g = GuardService::new();
        let ops = vec![
            agentcowork_guard::BatchOperation::new(
                "fs.rename",
                "rename",
                "h-1",
                vec![
                    "/w/Downloads/a.pdf".to_string(),
                    "/w/Docs/a.pdf".to_string(),
                ],
            ),
            agentcowork_guard::BatchOperation::new(
                "fs.rename",
                "rename",
                "h-2",
                vec![
                    "/w/Downloads/b.png".to_string(),
                    "/w/Images/b.png".to_string(),
                ],
            ),
        ];
        let d = g.evaluate_batch(
            "s1",
            "a1",
            ops.clone(),
            decision(RiskLevel::High, &["/w/Downloads"]),
            9,
        );
        let GuardDecision::Ask { ticket_id } = d else {
            panic!("expected Ask");
        };

        // The approved change-set hash is the immutable binding.
        let cs = g.batch_change_set_hash(&ticket_id).unwrap();
        assert_eq!(cs, agentcowork_guard::change_set_hash(&ops));

        // Human approval requires the card-bound nonce.
        assert!(!g.approve_batch_with_nonce(&ticket_id, "forged"));
        assert!(g.approve_batch(&ticket_id));

        // Executor consumes with the exact set.
        assert!(g.use_batch_ticket(&ticket_id, &cs).is_ok());
        // Single-use + a stretched change set are both refused.
        assert!(g.use_batch_ticket(&ticket_id, &cs).is_err());

        // A second batch with one extra op has a different binding.
        let mut ops2 = ops;
        ops2.push(agentcowork_guard::BatchOperation::new(
            "fs.delete",
            "delete",
            "h-3",
            vec!["/w/Downloads/secret".to_string()],
        ));
        assert_ne!(cs, agentcowork_guard::change_set_hash(&ops2));

        // Pending list + receipts exist for the card + audit.
        assert!(g.pending_batches().is_empty());
        assert_eq!(g.batch_receipts().len(), 1);
    }

    #[test]
    fn autonomy_level_presets_drive_the_policy_gate() {
        let mut g = GuardService::new();
        assert_eq!(g.autonomy_level(), agentcowork_guard::AutonomyPreset::Ask);

        // Sandbox: a generic write is blocked outright — no ticket at all.
        g.set_autonomy_level(agentcowork_guard::AutonomyPreset::Sandbox);
        assert_eq!(
            g.autonomy_level(),
            agentcowork_guard::AutonomyPreset::Sandbox
        );
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/w/x"]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Block { .. }));

        // Auto: the same low-risk write is policy-allowed → ticket mints
        // pre-approved (Allow, consumable immediately).
        g.set_autonomy_level(agentcowork_guard::AutonomyPreset::Auto);
        assert_eq!(g.autonomy_level(), agentcowork_guard::AutonomyPreset::Auto);
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/w/x"]),
            "h",
            0,
        );
        let GuardDecision::Allow { ticket_id } = d else {
            panic!("expected Allow under Auto preset");
        };
        assert!(g.use_ticket(&ticket_id, "h").is_ok());

        // The floor holds: delete still asks under Auto.
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::High, &["/w/x"]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Ask { .. }));
    }

    #[test]
    fn autonomy_handle_methods_report_and_apply_presets() {
        let mut g = GuardService::new();
        // Default = Ask preset.
        let out = g.handle("guard/autonomy", &json!({})).unwrap();
        assert_eq!(out["autonomyLevel"], "ask");
        assert_eq!(out["minConfidenceForAuto"], 0.85);

        // Apply auto → reported + floor drops.
        let out = g
            .handle("guard/set_autonomy", &json!({ "level": "auto" }))
            .unwrap();
        assert_eq!(out["autonomyLevel"], "auto");
        assert_eq!(out["minConfidenceForAuto"], 0.75);
        assert_eq!(g.autonomy_level(), agentcowork_guard::AutonomyPreset::Auto);

        // Maximum maps through the wire name.
        let out = g
            .handle("guard/set_autonomy", &json!({ "level": "maximum" }))
            .unwrap();
        assert_eq!(out["autonomyLevel"], "maximum");
        assert_eq!(out["minConfidenceForAuto"], 0.6);

        // Sandbox blocks a write outright (the preset is live on the gate).
        g.handle("guard/set_autonomy", &json!({ "level": "sandbox" }))
            .unwrap();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Block { .. }));

        // Unknown level refuses.
        assert!(
            g.handle("guard/set_autonomy", &json!({ "level": "bogus" }))
                .is_err()
        );

        // guard/policy now carries the autonomy level too.
        let out = g.handle("guard/policy", &json!({})).unwrap();
        assert_eq!(out["autonomyLevel"], "sandbox");
    }

    #[test]
    fn read_operations_plain_allow_sensitive_asks() {
        let mut g = GuardService::new();
        // P51.20 — plain reads are executor-auto-approved (Allow), the
        // sensitive-scope read resolves against the default approve-then-
        // read rule (Ask).
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.read",
            Operation::Read { sensitive: false },
            decision(RiskLevel::Low, &["/w/x"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Allow { .. }),
            "plain read must be Allow"
        );
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.read",
            Operation::Read { sensitive: true },
            decision(RiskLevel::Low, &["/home/u/.ssh/config"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Ask { .. }),
            "sensitive read must Ask under the default approve-then-read rule"
        );
        // The bundle can tighten it to Block — never widen past the preset.
        g.handle(
            "guard/apply_combo",
            &json!({ "name": "block_sensitive_reads" }),
        )
        .unwrap();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.read",
            Operation::Read { sensitive: true },
            decision(RiskLevel::Low, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Block { .. }));
    }

    #[test]
    fn combos_list_and_apply_with_unknown_refusal() {
        let mut g = GuardService::new();
        // The bundle catalog lists every combo with rules + descriptions.
        let out = g.handle("guard/combos", &json!({})).unwrap();
        let combos = out["combos"].as_array().unwrap();
        assert!(combos.len() >= 6);
        let names: Vec<&str> = combos.iter().filter_map(|c| c["name"].as_str()).collect();
        assert!(names.contains(&"approve_then_read"));
        assert!(names.contains(&"dev_workflow"));
        assert!(names.contains(&"browse_only"));

        // browse_only blocks writes and keeps sensitive reads asking.
        g.handle("guard/apply_combo", &json!({ "name": "browse_only" }))
            .unwrap();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Block { .. }));
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.read",
            Operation::Read { sensitive: true },
            decision(RiskLevel::Low, &[]),
            "h",
            0,
        );
        assert!(matches!(d, GuardDecision::Ask { .. }));

        // Unknown combo refuses; the policy is untouched.
        assert!(
            g.handle("guard/apply_combo", &json!({ "name": "nope" }))
                .is_err()
        );
    }

    #[test]
    fn floor_blocks_unlocated_delete() {
        // P51.29: a delete that names no targets is refused fail-closed.
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.delete",
            Operation::DeleteFiles,
            decision(RiskLevel::Low, &[]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Block { ref reason } if reason.contains("located")),
            "unlocated delete must Block, got {d:?}"
        );
    }

    #[test]
    fn set_policy_rules_applies_deny_wins_and_reports() {
        let mut g = GuardService::new();
        let out = g
            .handle(
                "guard/set_policy_rules",
                &json!({
                    "rules": [
                        { "tool": "fs.write", "approval": "deny" },
                        { "tool": "shell", "argsGlob": "rm -rf *", "approval": "deny" },
                        { "tool": "browser.*", "approval": "allow" },
                    ]
                }),
            )
            .unwrap();
        assert_eq!(out["applied"], 3);

        // The deny now blocks the tool at the pre-flight boundary.
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/tmp/a.txt"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Block { .. }),
            "tool-policy deny must block, got {d:?}"
        );

        // Deny with args-glob matches the exact args string (args half of
        // the pattern is exercised here — the guard boundary receives the
        // tool name; the args half is checked via the policy itself).
        let pol = &g.approval_policy;
        assert!(pol.evaluate("shell", "rm -rf *") == Approval::Deny);
        assert!(pol.evaluate("shell", "ls") == Approval::Ask);
        assert!(pol.evaluate("browser.click", "") == Approval::Allow);

        // Bad input is refused (fail-closed, never partial-apply).
        assert!(
            g.handle(
                "guard/set_policy_rules",
                &json!({ "rules": [{ "tool": "", "approval": "deny" }] }),
            )
            .is_err()
        );
        assert!(
            g.handle(
                "guard/set_policy_rules",
                &json!({ "rules": [{ "tool": "x", "approval": "maybe" }] }),
            )
            .is_err()
        );
    }

    #[test]
    fn floor_forces_ask_on_protected_settings_write() {
        // P51.30: our own settings paths always need a human (Ask, never
        // auto) — even under an allow policy.
        let mut g = GuardService::new();
        g.policy = PermissionsPolicy::parse("[permissions]\nwrite = \"allow\"\n");
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/home/u/.agentcowork/permissions.toml"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Ask { .. }),
            "protected settings write must Ask even under allow, got {d:?}"
        );
    }

    #[test]
    fn floor_forces_ask_on_git_hooks_write() {
        // P51.29: protected-in-project prefixes force Ask under any preset.
        let mut g = GuardService::new();
        g.policy = PermissionsPolicy::parse("[permissions]\nwrite = \"allow\"\n");
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/proj/.git/hooks/pre-commit"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Ask { .. }),
            "git-hooks write must Ask even under allow, got {d:?}"
        );
    }

    #[test]
    fn floor_blocks_critical_rm() {
        // P51.30: destructive shell against / is refused outright.
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "shell.exec",
            Operation::TerminalShell { destructive: true },
            decision(RiskLevel::High, &["/"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Block { ref reason } if reason.contains("critical")),
            "critical rm must Block, got {d:?}"
        );
    }

    #[test]
    fn approval_policy_deny_blocks() {
        // P51.16: an explicit tool-level Deny always blocks.
        use agentcowork_guard::approval_policy::{Approval, ApprovalPolicy, ToolPattern};
        let mut g = GuardService::new();
        g.policy = PermissionsPolicy::parse("[permissions]\nwrite = \"allow\"\n");
        g.set_approval_policy(ApprovalPolicy::new(vec![(
            ToolPattern::new("fs.write", None),
            Approval::Deny,
        )]));
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/workspace/a.txt"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Block { ref reason } if reason.contains("denies")),
            "tool deny must Block, got {d:?}"
        );
    }

    #[test]
    fn reviewer_never_upgrades_by_default() {
        // P51.16: default reviewer budget is zero — Ask stays Ask.
        let mut g = GuardService::new();
        let d = g.evaluate(
            "s1",
            "a1",
            "fs.write",
            Operation::GenericWrite,
            decision(RiskLevel::Low, &["/workspace/a.txt"]),
            "h",
            0,
        );
        assert!(
            matches!(d, GuardDecision::Ask { .. }),
            "default reviewer must not upgrade Ask, got {d:?}"
        );
    }

    #[test]
    fn reviewer_upgrades_when_configured() {
        // P51.16: a configured reviewer may upgrade Ask→Allow on confidence.
        use agentcowork_guard::reviewer::ReviewerConfig;
        let mut g = GuardService::new();
        g.set_reviewer_config(ReviewerConfig::new(0.5, 10));
        let mut pkg = decision(RiskLevel::Low, &["/workspace/a.txt"]);
        pkg.confidence = Some(0.9);
        let d = g.evaluate("s1", "a1", "fs.write", Operation::GenericWrite, pkg, "h", 0);
        assert!(
            matches!(d, GuardDecision::Allow { .. }),
            "configured reviewer should upgrade confident Ask, got {d:?}"
        );
    }
}
