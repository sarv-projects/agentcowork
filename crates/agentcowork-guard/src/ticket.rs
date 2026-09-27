//! P7.4 — the authorization ticket contract (doc 53 §3). Before any
//! privileged action executes, the coordinator mints a ticket; the executor
//! only runs an action whose ticket is valid, unexpired, unused and
//! matches the operation. Deterministic single-use enforcement keeps a dead
//! or replayed coordinator from double-executing external mutations.

use crate::prescan::ScanTarget;
use crate::profiles::GateAction;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Who approved the action (doc 53 §3 `approval-source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalSource {
    /// Auto-approved under a standing rule (delete=always_ask overrides).
    Policy,
    /// Human clicked approve on a Guard-2 card.
    Human,
    /// User pre-approved via `<data_dir>/permissions.toml`.
    PermissionsToml,
    /// Coordinator granted under least-privilege topology rules.
    Coordinator,
}

/// Risk tier of the operation (drives who must approve).
///
/// **Note (P47.3):** this is the canonical shared type from `agentcowork-types`
/// so the whole workspace grades risk on one scale (re-exported here to keep
/// `agentcowork_guard::RiskLevel` working for existing consumers).
pub use agentcowork_types::RiskLevel;

/// Named risk tiers R0–R4 (H3). Formalizes RiskLevel for cards + catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RiskTier {
    /// Harmless read — auto.
    R0,
    /// Reversible local write — auto per policy.
    R1,
    /// External effect — approval.
    R2,
    /// Destructive — approval.
    R3,
    /// High-privilege credential/network/system — explicit, deny-by-default.
    R4,
}

impl RiskTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::R0 => "R0",
            Self::R1 => "R1",
            Self::R2 => "R2",
            Self::R3 => "R3",
            Self::R4 => "R4",
        }
    }

    pub fn from_risk_and_op(risk: RiskLevel, operation: &str, read_only: bool) -> Self {
        let op = operation.to_lowercase();
        if op.contains("credential") || op.contains("install") || op.contains("estop") {
            return Self::R4;
        }
        if op.contains("network") || op.contains("external") || op.contains("web") {
            return Self::R2;
        }
        if read_only {
            return Self::R0;
        }
        if op.contains("delete") || op.contains("destructive") || risk >= RiskLevel::High {
            return Self::R3;
        }
        match risk {
            RiskLevel::Low => Self::R1,
            RiskLevel::Medium => Self::R1,
            RiskLevel::High => Self::R3,
            RiskLevel::Critical => Self::R4,
        }
    }
}

/// Current ticket lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TicketState {
    /// Minted, awaiting a human decision (or auto-approved under policy).
    #[default]
    Pending,
    /// Human-approved (or policy auto-approved) — consumable by an executor.
    Approved,
    /// Consumed by an executor (single-use).
    Used,
    /// Rejected by the executor (arg-hash mismatch / path outside grant).
    Rejected,
    /// Expired (TTL passed).
    Expired,
    /// Revoked (estop / session abort).
    Revoked,
}

/// The authorization ticket (doc 53 §3 fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationTicket {
    /// Unique ticket id (uuid-ish; caller-generated or via `TicketStore::mint`).
    pub ticket_id: String,
    pub agent_id: String,
    pub session_id: String,
    /// The tool the action targets (e.g. `shell.exec`, `fs.write`).
    pub tool_id: String,
    /// Canonical operation name (`write`, `delete`, `network`, `exec`).
    pub operation: String,
    /// SHA-256 of the serialized arguments the executor must match.
    pub args_hash: String,
    /// Paths the action may touch (empty = none / not path-scoped).
    pub paths: Vec<String>,
    /// UNIX ms expiry; 0 = no expiry.
    pub expires_at_ms: u64,
    /// Single-use: `true` (default) — consumed on first valid use.
    pub single_use: bool,
    pub approval_source: ApprovalSource,
    /// Secret bound to the rendered Guard-2 card. Human approval must present
    /// this nonce; the ticket id alone is not an approval capability.
    #[serde(default)]
    pub approval_nonce: String,
    pub risk: RiskLevel,
    /// Audit sequence of the approval event this ticket refers to.
    pub audit_seq: u64,
    /// Set by the executor on consume/reject/expire.
    #[serde(default)]
    pub state: TicketState,
    /// S0.6 TOCTOU: resource identities captured at mint, re-checked at use.
    #[serde(default)]
    pub bindings: Vec<crate::toctou::ResourceBinding>,
    /// H3 idempotency: bind this ticket to an execution + action.
    #[serde(default)]
    pub execution_id: String,
    #[serde(default)]
    pub action_id: String,
    #[serde(default)]
    pub idempotency_key: String,
}

impl AuthorizationTicket {
    /// Is this ticket consumable right now? A ticket is consumable only once
    /// it has been *approved* (human `approve()` or policy auto-approval) —
    /// a `Pending` ticket is not valid for execution.
    pub fn is_valid(&self) -> bool {
        if self.state != TicketState::Approved {
            return false;
        }
        if self.expires_at_ms != 0 && now_ms() > self.expires_at_ms {
            return false;
        }
        true
    }

    /// Does the caller's hash match either the legacy arguments-only binding
    /// or the server-derived tool/operation/arguments context binding?
    pub fn matches_args(&self, args_hash: &str) -> bool {
        self.args_hash == args_hash || self.matches_context(args_hash)
    }

    /// Does the caller's hash bind to this ticket's canonical tool, operation,
    /// and arguments?
    pub fn matches_context(&self, context_hash: &str) -> bool {
        ticket_context_hash(&self.tool_id, &self.operation, &self.args_hash) == context_hash
    }

    /// Consume the ticket (single-use). Returns false if already used/invalid.
    pub fn consume(&mut self, args_hash: &str) -> bool {
        if !self.is_valid() || !self.matches_args(args_hash) {
            return false;
        }
        if self.matches_context(args_hash) && !self.single_use {
            self.state = TicketState::Rejected;
            return false;
        }
        if self.single_use {
            self.state = TicketState::Used;
        }
        true
    }
}

/// In-memory ticket store with single-use + expiry enforcement.
#[derive(Debug, Default)]
pub struct TicketStore {
    tickets: std::collections::HashMap<String, AuthorizationTicket>,
    /// Append-only approve/reject receipts (P7.5 audit trail).
    receipts: Vec<GuardReceipt>,
}

impl TicketStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The append-only approval/denial audit receipts.
    pub fn receipts(&self) -> &[GuardReceipt] {
        &self.receipts
    }

    fn record(&mut self, id: &str, action: ReceiptAction) -> bool {
        let Some(t) = self.tickets.get(id) else {
            return false;
        };
        let seq = self.receipts.len();
        let receipt = GuardReceipt::new(format!("rcpt:{seq}"), t, action, now_ms());
        self.receipts.push(receipt);
        true
    }

    pub fn mint(&mut self, ticket: AuthorizationTicket) -> String {
        let id = ticket.ticket_id.clone();
        self.tickets.insert(id.clone(), ticket);
        id
    }

    /// Mint honoring the profile gate (P37 — auto-run / full access honored
    /// at ticket mint): `GateAction::Allow` mints in the **Approved** state
    /// (policy auto-run — no Ask card for this mutation); `Ask` mints as
    /// usual (Pending); `Block` refuses to mint at all (the mutation never
    /// gets a ticket). Deterministic.
    pub fn mint_gated(
        &mut self,
        mut ticket: AuthorizationTicket,
        action: GateAction,
    ) -> Result<String, TicketError> {
        match action {
            GateAction::Allow => {
                ticket.state = TicketState::Approved;
                ticket.approval_source = ApprovalSource::Policy;
                Ok(self.mint(ticket))
            }
            GateAction::Ask => Ok(self.mint(ticket)),
            GateAction::Block => Err(TicketError::Blocked),
        }
    }

    /// Look up + consume. Enforces **approval first**, then validity, arg
    /// match and single-use. A `Pending` ticket (minted but not yet approved)
    /// is refused with [`TicketError::NotApproved`] — approval is a hard
    /// prerequisite for consumption, never a side-channel.
    pub fn use_ticket(&mut self, id: &str, args_hash: &str) -> Result<(), TicketError> {
        let Some(t) = self.tickets.get_mut(id) else {
            return Err(TicketError::Unknown);
        };
        if t.expires_at_ms != 0 && now_ms() > t.expires_at_ms {
            t.state = TicketState::Expired;
            return Err(TicketError::Expired);
        }
        match t.state {
            TicketState::Approved => {}
            TicketState::Used => return Err(TicketError::AlreadyUsed),
            TicketState::Revoked => return Err(TicketError::Revoked),
            TicketState::Pending => return Err(TicketError::NotApproved),
            TicketState::Rejected => return Err(TicketError::Rejected),
            TicketState::Expired => return Err(TicketError::Expired),
        }
        if !t.matches_args(args_hash) {
            t.state = TicketState::Rejected;
            return Err(TicketError::ArgsMismatch);
        }
        // Context-bound executor consumption is always single-use. Legacy
        // callers that present only `args_hash` retain their historical
        // `single_use` behavior.
        if t.matches_context(args_hash) && !t.single_use {
            t.state = TicketState::Rejected;
            return Err(TicketError::NotSingleUse);
        }
        if t.single_use {
            t.state = TicketState::Used;
        }
        Ok(())
    }

    pub fn revoke(&mut self, id: &str) {
        if let Some(t) = self.tickets.get_mut(id) {
            t.state = TicketState::Revoked;
        }
    }

    /// P7.5 (Guard-2) — the open tickets the approval card renders: every
    /// `Pending` ticket that has not yet been consumed/rejected/revoked.
    pub fn pending(&self) -> Vec<&AuthorizationTicket> {
        self.tickets
            .values()
            .filter(|t| t.state == TicketState::Pending)
            .collect()
    }

    /// Internal approval for policy-controlled paths (for example, a read-only
    /// tool that is deliberately auto-approved by the executor). Human-facing
    /// approval must use [`Self::approve_with_nonce`].
    pub fn approve(&mut self, id: &str) -> bool {
        self.approve_inner(id, None)
    }

    /// P7.5/P10.2 — approve only when the nonce displayed in the Guard-2 card
    /// matches the ticket. A ticket id copied or synthesized by webview code
    /// is insufficient without the card-bound nonce.
    pub fn approve_with_nonce(&mut self, id: &str, nonce: &str) -> bool {
        self.approve_inner(id, Some(nonce))
    }

    fn approve_inner(&mut self, id: &str, nonce: Option<&str>) -> bool {
        let ok = match self.tickets.get_mut(id) {
            Some(t)
                if t.state == TicketState::Pending
                    && nonce
                        .map(|n| !n.is_empty() && n == t.approval_nonce)
                        .unwrap_or(true) =>
            {
                t.state = TicketState::Approved;
                t.approval_source = ApprovalSource::Human;
                true
            }
            _ => false,
        };
        if ok {
            self.record(id, ReceiptAction::Approve);
        }
        ok
    }

    /// P7.5/P10.2 — reject only when the nonce displayed in the Guard-2 card
    /// matches the ticket. Rejection is also card-bound so stale/synthetic
    /// webview requests cannot alter a live approval state.
    pub fn reject_with_nonce(&mut self, id: &str, nonce: &str) -> bool {
        let ok = match self.tickets.get_mut(id) {
            Some(t)
                if t.state == TicketState::Pending
                    && !nonce.is_empty()
                    && nonce == t.approval_nonce =>
            {
                t.state = TicketState::Revoked;
                true
            }
            _ => false,
        };
        if ok {
            self.record(id, ReceiptAction::Reject);
        }
        ok
    }

    /// Internal rejection used by non-UI control paths.
    pub fn reject(&mut self, id: &str) -> bool {
        let ok = match self.tickets.get_mut(id) {
            Some(t) if t.state == TicketState::Pending => {
                t.state = TicketState::Revoked;
                true
            }
            _ => false,
        };
        if ok {
            self.record(id, ReceiptAction::Reject);
        }
        ok
    }

    pub fn approval_nonce(&self, id: &str) -> Option<&str> {
        self.tickets.get(id).map(|t| t.approval_nonce.as_str())
    }

    pub fn set_bindings(
        &mut self,
        id: &str,
        bindings: Vec<crate::toctou::ResourceBinding>,
    ) -> bool {
        if let Some(t) = self.tickets.get_mut(id) {
            t.bindings = bindings;
            return true;
        }
        false
    }

    pub fn set_execution(&mut self, id: &str, execution_id: &str) -> bool {
        if let Some(t) = self.tickets.get_mut(id) {
            t.execution_id = execution_id.to_string();
            if t.idempotency_key.is_empty() {
                t.idempotency_key = format!("{}:{}:{}", t.session_id, t.tool_id, t.args_hash);
            }
            return true;
        }
        false
    }

    pub fn get(&self, id: &str) -> Option<&AuthorizationTicket> {
        self.tickets.get(id)
    }

    /// P52.x — mutable access for Pending-gated TTL extension (nonce rotation
    /// + expiry bump).
    ///
    /// Callers must enforce the Pending gate themselves; the store does not
    /// change state here.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut AuthorizationTicket> {
        self.tickets.get_mut(id)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TicketError {
    #[error("unknown ticket")]
    Unknown,
    #[error("ticket expired")]
    Expired,
    #[error("ticket already used")]
    AlreadyUsed,
    #[error("ticket revoked")]
    Revoked,
    #[error("ticket not approved (pending human decision)")]
    NotApproved,
    #[error("args hash mismatch")]
    ArgsMismatch,
    #[error("ticket was already rejected")]
    Rejected,
    #[error("context-bound tool consumption requires a single-use ticket")]
    NotSingleUse,
    #[error("mint refused by profile gate (blocked)")]
    Blocked,
}

/// Current unix time in ms (injectable in tests via `set_now_ms`).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A human approve/reject decision, recorded as an append-only audit receipt
/// (P7.5 — "approval/denial audit logging with receipt"). The hash covers
/// every field, so a receipt is tamper-evident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardReceipt {
    pub receipt_id: String,
    pub ticket_id: String,
    pub session_id: String,
    pub tool_id: String,
    pub operation: String,
    pub action: ReceiptAction,
    pub ts_ms: u64,
    /// SHA-256 over the serialized receipt fields (self-hash, keyed on the
    /// fields above in order).
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptAction {
    Approve,
    Reject,
}

impl GuardReceipt {
    /// Build + self-hash a receipt.
    pub fn new(
        receipt_id: String,
        ticket: &AuthorizationTicket,
        action: ReceiptAction,
        ts_ms: u64,
    ) -> Self {
        let hash = hash_args(&[
            &receipt_id,
            &ticket.ticket_id,
            &ticket.session_id,
            &ticket.tool_id,
            &ticket.operation,
            match action {
                ReceiptAction::Approve => "approve",
                ReceiptAction::Reject => "reject",
            },
            &ts_ms.to_string(),
        ]);
        Self {
            receipt_id,
            ticket_id: ticket.ticket_id.clone(),
            session_id: ticket.session_id.clone(),
            tool_id: ticket.tool_id.clone(),
            operation: ticket.operation.clone(),
            action,
            ts_ms,
            hash,
        }
    }
}

/// Generate an unpredictable card-bound approval nonce. It is never sent to
/// the coordinator; only the human-facing card bridge receives it.
pub fn new_approval_nonce() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Stable helper: hash the args for a ticket (deterministic, keyed by
/// serialization order — caller must serialize canonically).
pub fn hash_args(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    let out = h.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Derive the server-side consumption hash that binds a ticket to the exact
/// tool, canonical operation, and argument hash it was minted for.
pub fn ticket_context_hash(tool_id: &str, operation: &str, args_hash: &str) -> String {
    hash_args(&["ticket-context-v1", tool_id, operation, args_hash])
}

/// Convenience: build a ticket for a path-scoped operation.
///
/// The argument count mirrors the doc 53 §3 ticket contract — kept explicit
/// on purpose (a builder would hide required fields).
#[allow(clippy::too_many_arguments)]
pub fn path_ticket(
    ticket_id: &str,
    agent_id: &str,
    session_id: &str,
    tool_id: &str,
    operation: &str,
    paths: &[&str],
    args_hash: &str,
    risk: RiskLevel,
    audit_seq: u64,
) -> AuthorizationTicket {
    AuthorizationTicket {
        ticket_id: ticket_id.to_string(),
        agent_id: agent_id.to_string(),
        session_id: session_id.to_string(),
        tool_id: tool_id.to_string(),
        operation: operation.to_string(),
        args_hash: args_hash.to_string(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        expires_at_ms: now_ms() + 60_000,
        single_use: true,
        approval_source: ApprovalSource::Policy,
        approval_nonce: new_approval_nonce(),
        risk,
        audit_seq,
        state: TicketState::Pending,
        bindings: Vec::new(),
        execution_id: String::new(),
        action_id: tool_id.to_string(),
        idempotency_key: format!("{session_id}:{tool_id}:{args_hash}"),
    }
}

/// Prove ScanTarget is importable here (used by the card renderer in P7.5).
#[allow(dead_code)]
fn _scan_target_roundtrip(t: ScanTarget) -> &'static str {
    t.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str) -> AuthorizationTicket {
        path_ticket(
            id,
            "agent-1",
            "sess-1",
            "fs.delete",
            "delete",
            &["/workspace/x"],
            "h1",
            RiskLevel::High,
            1,
        )
    }

    /// A ticket that has already been human-approved (consumable).
    fn approved(id: &str) -> AuthorizationTicket {
        let mut t = ticket(id);
        t.state = TicketState::Approved;
        t
    }

    #[test]
    fn mint_gated_honors_allow_ask_block() {
        let mut store = TicketStore::new();
        // Allow → mints pre-approved (auto-run honored, no Ask card).
        let id = store.mint_gated(ticket("auto"), GateAction::Allow).unwrap();
        assert!(store.use_ticket(&id, "h1").is_ok());
        // Ask → mints Pending (human decision still required).
        let id2 = store.mint_gated(ticket("ask"), GateAction::Ask).unwrap();
        assert!(matches!(
            store.use_ticket(&id2, "h1"),
            Err(TicketError::NotApproved)
        ));
        // Block → no ticket exists at all.
        assert!(matches!(
            store.mint_gated(ticket("block"), GateAction::Block),
            Err(TicketError::Blocked)
        ));
        assert!(matches!(
            store.use_ticket("block", "h1"),
            Err(TicketError::Unknown)
        ));
    }

    #[test]
    fn r0_to_r4_mapping() {
        assert_eq!(
            RiskTier::from_risk_and_op(RiskLevel::Low, "write", true),
            RiskTier::R0
        );
        assert_eq!(
            RiskTier::from_risk_and_op(RiskLevel::Medium, "write", false),
            RiskTier::R1
        );
        assert_eq!(
            RiskTier::from_risk_and_op(RiskLevel::Medium, "external_network", false),
            RiskTier::R2
        );
        assert_eq!(
            RiskTier::from_risk_and_op(RiskLevel::High, "delete", false),
            RiskTier::R3
        );
        assert_eq!(
            RiskTier::from_risk_and_op(RiskLevel::Critical, "install", false),
            RiskTier::R4
        );
        assert_eq!(
            RiskTier::from_risk_and_op(RiskLevel::Low, "external_network", true),
            RiskTier::R2
        );
    }

    #[test]
    fn single_use_enforced() {
        let mut store = TicketStore::new();
        let id = store.mint(approved("t1"));
        assert!(store.use_ticket(&id, "h1").is_ok());
        assert_eq!(store.use_ticket(&id, "h1"), Err(TicketError::AlreadyUsed));
    }

    #[test]
    fn pending_ticket_requires_approval() {
        let mut store = TicketStore::new();
        // A freshly-minted (Pending) ticket must NOT be consumable — approval
        // is the prerequisite, not an optional side-channel.
        let id = store.mint(ticket("tp"));
        assert_eq!(store.use_ticket(&id, "h1"), Err(TicketError::NotApproved));
        assert!(store.approve(&id));
        assert!(store.use_ticket(&id, "h1").is_ok());
    }

    #[test]
    fn args_mismatch_rejects() {
        let mut store = TicketStore::new();
        let id = store.mint(approved("t2"));
        assert_eq!(
            store.use_ticket(&id, "wrong"),
            Err(TicketError::ArgsMismatch)
        );
        assert_eq!(store.get(&id).unwrap().state, TicketState::Rejected);
    }

    #[test]
    fn revoke_blocks() {
        let mut store = TicketStore::new();
        let id = store.mint(approved("t3"));
        store.revoke(&id);
        assert_eq!(store.use_ticket(&id, "h1"), Err(TicketError::Revoked));
    }

    #[test]
    fn unknown_rejected() {
        let mut store = TicketStore::new();
        assert_eq!(store.use_ticket("nope", "h"), Err(TicketError::Unknown));
    }

    #[test]
    fn expired_ticket_cannot_be_consumed() {
        let mut expired = approved("expired");
        expired.expires_at_ms = 1;
        let context = ticket_context_hash(&expired.tool_id, &expired.operation, &expired.args_hash);
        let mut store = TicketStore::new();
        let id = store.mint(expired);
        assert_eq!(store.use_ticket(&id, &context), Err(TicketError::Expired));
        assert_eq!(store.get(&id).unwrap().state, TicketState::Expired);
    }

    #[test]
    fn context_consumption_binds_tool_operation_and_args() {
        let valid = approved("valid-context");
        let context = ticket_context_hash(&valid.tool_id, &valid.operation, &valid.args_hash);
        let mut store = TicketStore::new();
        let id = store.mint(valid);
        assert!(store.use_ticket(&id, &context).is_ok());
        assert_eq!(store.get(&id).unwrap().state, TicketState::Used);

        for (case, presented) in [
            (
                "wrong-tool",
                ticket_context_hash("fs.write", "delete", "h1"),
            ),
            (
                "wrong-operation",
                ticket_context_hash("fs.delete", "write", "h1"),
            ),
            (
                "wrong-args",
                ticket_context_hash("fs.delete", "delete", "h2"),
            ),
        ] {
            let mut store = TicketStore::new();
            let id = store.mint(approved(case));
            assert_eq!(
                store.use_ticket(&id, &presented),
                Err(TicketError::ArgsMismatch)
            );
            assert_eq!(store.get(&id).unwrap().state, TicketState::Rejected);
            // A rejected ticket cannot later be revived through the legacy raw
            // argument hash.
            assert_eq!(store.use_ticket(&id, "h1"), Err(TicketError::Rejected));
        }
    }

    #[test]
    fn context_consumption_requires_single_use() {
        let mut reusable = approved("reusable");
        reusable.single_use = false;
        let context =
            ticket_context_hash(&reusable.tool_id, &reusable.operation, &reusable.args_hash);
        let mut store = TicketStore::new();
        let id = store.mint(reusable);
        assert_eq!(
            store.use_ticket(&id, &context),
            Err(TicketError::NotSingleUse)
        );
        assert_eq!(store.get(&id).unwrap().state, TicketState::Rejected);
    }

    #[test]
    fn hash_is_deterministic_and_order_sensitive() {
        assert_eq!(hash_args(&["a", "b"]), hash_args(&["a", "b"]));
        assert_ne!(hash_args(&["a", "b"]), hash_args(&["b", "a"]));
    }

    #[test]
    fn pending_lists_open_tickets_and_approve_records_human() {
        let mut store = TicketStore::new();
        let a = store.mint(ticket("ta"));
        let b = store.mint(ticket("tb"));
        store.revoke(&b);
        // Only the still-pending ticket is listed.
        assert_eq!(store.pending().len(), 1);
        assert_eq!(store.pending()[0].ticket_id, "ta");
        // Human approve records the source; consumption still enforces args.
        assert!(store.approve(&a));
        assert_eq!(
            store.get(&a).unwrap().approval_source,
            ApprovalSource::Human
        );
        assert!(store.use_ticket(&a, "h1").is_ok());
        // Approve on a non-pending/unknown ticket is a no-op.
        assert!(!store.approve(&a));
        assert!(!store.approve("ghost"));
    }

    #[test]
    fn nonce_binds_human_approval_to_the_card() {
        let mut store = TicketStore::new();
        let id = store.mint(ticket("nonce"));
        let nonce = store.get(&id).unwrap().approval_nonce.clone();
        assert!(!store.approve_with_nonce(&id, "wrong"));
        assert_eq!(store.get(&id).unwrap().state, TicketState::Pending);
        assert!(store.approve_with_nonce(&id, &nonce));
        assert_eq!(
            store.get(&id).unwrap().approval_source,
            ApprovalSource::Human
        );
    }

    #[test]
    fn approve_and_reject_record_audit_receipts() {
        let mut store = TicketStore::new();
        let a = store.mint(ticket("ta"));
        let b = store.mint(ticket("tb"));

        assert!(store.approve(&a));
        assert!(store.reject(&b));

        let receipts = store.receipts();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0].action, ReceiptAction::Approve);
        assert_eq!(receipts[0].ticket_id, "ta");
        assert_eq!(receipts[1].action, ReceiptAction::Reject);
        assert_eq!(receipts[1].ticket_id, "tb");
        // Reject revoked the ticket.
        assert_eq!(store.get(&b).unwrap().state, TicketState::Revoked);
        // The receipt hash is deterministic over its fields.
        assert_eq!(receipts[0].hash.len(), 64);
        assert_ne!(receipts[0].hash, receipts[1].hash);
        // Reject on a non-pending ticket records nothing.
        assert!(!store.reject(&b));
        assert_eq!(store.receipts().len(), 2);
    }
}
