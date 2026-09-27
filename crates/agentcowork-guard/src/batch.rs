//! P47.6 — the universal **BatchTicket** executor primitive (UC-1 "approve
//! all"). The xlsx bulk path minted an ordinary single-op ticket whose args
//! hash covered a whole batch; this module generalizes that to a first-class
//! contract: a `BatchTicket` binds an **exact, immutable change set** —
//! operation list + per-op args hashes + resource identities — and approval
//! covers exactly that set, never an operation category. The executor must
//! present the identical change-set hash to consume the ticket, so neither
//! the agent nor a compromised coordinator can mint additional mutations
//! under a previously approved category.
//!
//! Lifecycle mirrors `AuthorizationTicket` (mint → approve-with-nonce →
//! consume), so the same Guard-2 card surface and audit receipts apply.

use crate::ticket::{ApprovalSource, RiskLevel, TicketState, hash_args, new_approval_nonce};
use serde::{Deserialize, Serialize};

/// One mutation in the change set: the exact operation + args hash + the
/// resource identities it touches (paths, sheet+range, URL, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchOperation {
    pub tool_id: String,
    pub operation: String,
    /// SHA-256 of the serialized args the executor must match for this op.
    pub args_hash: String,
    /// Resource identities the op touches (paths, sheet+range, URL, ...).
    pub resources: Vec<String>,
}

impl BatchOperation {
    pub fn new(
        tool_id: impl Into<String>,
        operation: impl Into<String>,
        args_hash: impl Into<String>,
        resources: Vec<String>,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            operation: operation.into(),
            args_hash: args_hash.into(),
            resources,
        }
    }
}

/// The immutable change set: SHA-256 over the canonical serialization of the
/// operation list (serde preserves Vec order and field order). Any addition,
/// removal, reorder or field change produces a different hash — so the hash
/// the executor presents at consume time must match the hash the human
/// approved at card time.
pub fn change_set_hash(operations: &[BatchOperation]) -> String {
    hash_args(&[&serde_json::to_string(operations).unwrap_or_default()])
}

/// The batch ticket. `change_set_hash` is the immutable binding; everything
/// else is bookkeeping. `operations` is retained so the card can render the
/// exact set, but only the hash is authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchTicket {
    pub ticket_id: String,
    pub agent_id: String,
    pub session_id: String,
    /// SHA-256 over the canonical serialization of `operations`.
    pub change_set_hash: String,
    /// The exact change set the human approved (rendered on the card).
    pub operations: Vec<BatchOperation>,
    /// UNIX ms expiry; 0 = no expiry.
    pub expires_at_ms: u64,
    /// Single-use: `true` (default) — consumed on first valid use.
    pub single_use: bool,
    pub approval_source: ApprovalSource,
    /// Secret bound to the rendered Guard-2 card (same rule as single tickets).
    pub approval_nonce: String,
    /// Highest risk among the set's operations (drives who must approve).
    pub risk: RiskLevel,
    pub audit_seq: u64,
    #[serde(default)]
    pub state: TicketState,
}

impl BatchTicket {
    /// Mint a new pending batch ticket. `change_set_hash` is computed from
    /// the operations, so the caller cannot self-serve a hash that does not
    /// match the set.
    pub fn mint(
        ticket_id: impl Into<String>,
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
        operations: Vec<BatchOperation>,
        risk: RiskLevel,
        audit_seq: u64,
    ) -> Self {
        let change_set_hash = change_set_hash(&operations);
        Self {
            ticket_id: ticket_id.into(),
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            change_set_hash,
            operations,
            expires_at_ms: now_ms() + 60_000,
            single_use: true,
            approval_source: ApprovalSource::Policy,
            approval_nonce: new_approval_nonce(),
            risk,
            audit_seq,
            state: TicketState::Pending,
        }
    }

    /// Is this batch ticket consumable right now (approved + unexpired)?
    pub fn is_valid(&self) -> bool {
        if self.state != TicketState::Approved {
            return false;
        }
        if self.expires_at_ms != 0 && now_ms() > self.expires_at_ms {
            return false;
        }
        true
    }

    /// Does the caller's change-set hash match what was approved?
    pub fn matches_change_set(&self, change_set_hash: &str) -> bool {
        self.change_set_hash == change_set_hash
    }

    /// Consume (single-use). Returns false if not approved, expired, already
    /// used, or the change set does not match the approved one exactly.
    pub fn consume(&mut self, change_set_hash: &str) -> bool {
        if !self.is_valid() || !self.matches_change_set(change_set_hash) {
            return false;
        }
        if self.single_use {
            self.state = TicketState::Used;
        }
        true
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append-only approve/reject receipt for a batch decision (same self-hash
/// discipline as `GuardReceipt`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchReceipt {
    pub receipt_id: String,
    pub ticket_id: String,
    pub session_id: String,
    /// The immutable binding — verifiable against the ticket at audit time.
    pub change_set_hash: String,
    pub operation_count: usize,
    pub action: BatchAction,
    pub ts_ms: u64,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchAction {
    Approve,
    Reject,
}

impl BatchReceipt {
    pub fn new(receipt_id: String, ticket: &BatchTicket, action: BatchAction, ts_ms: u64) -> Self {
        let hash = hash_args(&[
            &receipt_id,
            &ticket.ticket_id,
            &ticket.session_id,
            &ticket.change_set_hash,
            &ticket.operations.len().to_string(),
            match action {
                BatchAction::Approve => "approve",
                BatchAction::Reject => "reject",
            },
            &ts_ms.to_string(),
        ]);
        Self {
            receipt_id,
            ticket_id: ticket.ticket_id.clone(),
            session_id: ticket.session_id.clone(),
            change_set_hash: ticket.change_set_hash.clone(),
            operation_count: ticket.operations.len(),
            action,
            ts_ms,
            hash,
        }
    }
}

/// Batch ticket store with the same single-use + expiry + nonce-bound
/// approval discipline as `TicketStore`.
#[derive(Debug, Default)]
pub struct BatchTicketStore {
    tickets: std::collections::HashMap<String, BatchTicket>,
    receipts: Vec<BatchReceipt>,
}

impl BatchTicketStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mint(&mut self, ticket: BatchTicket) -> String {
        let id = ticket.ticket_id.clone();
        self.tickets.insert(id.clone(), ticket);
        id
    }

    pub fn get(&self, id: &str) -> Option<&BatchTicket> {
        self.tickets.get(id)
    }

    /// P52.x — mutable access for Pending-gated TTL extension (same rule as
    /// single tickets; callers enforce the Pending gate).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut BatchTicket> {
        self.tickets.get_mut(id)
    }

    /// The open batch tickets the approval card renders.
    pub fn pending(&self) -> Vec<&BatchTicket> {
        self.tickets
            .values()
            .filter(|t| t.state == TicketState::Pending)
            .collect()
    }

    pub fn receipts(&self) -> &[BatchReceipt] {
        &self.receipts
    }

    fn record(&mut self, id: &str, action: BatchAction) -> bool {
        let Some(t) = self.tickets.get(id) else {
            return false;
        };
        let seq = self.receipts.len();
        let receipt = BatchReceipt::new(format!("brcpt:{seq}"), t, action, now_ms());
        self.receipts.push(receipt);
        true
    }

    /// Approve only when the card-bound nonce matches (same P10.2 rule as
    /// single tickets — a synthesized ticket id is not an approval
    /// capability). Approves the *whole immutable set*; the executor later
    /// presents the change-set hash, so the approval can never be stretched
    /// to a different or larger set.
    pub fn approve_with_nonce(&mut self, id: &str, nonce: &str) -> bool {
        let ok = match self.tickets.get_mut(id) {
            Some(t)
                if t.state == TicketState::Pending
                    && !nonce.is_empty()
                    && nonce == t.approval_nonce =>
            {
                t.state = TicketState::Approved;
                t.approval_source = ApprovalSource::Human;
                true
            }
            _ => false,
        };
        if ok {
            self.record(id, BatchAction::Approve);
        }
        ok
    }

    /// Internal approval for policy-controlled paths (auto-run under a
    /// standing rule). Human-facing approval must use `approve_with_nonce`.
    pub fn approve(&mut self, id: &str) -> bool {
        let ok = match self.tickets.get_mut(id) {
            Some(t) if t.state == TicketState::Pending => {
                t.state = TicketState::Approved;
                t.approval_source = ApprovalSource::Policy;
                true
            }
            _ => false,
        };
        if ok {
            self.record(id, BatchAction::Approve);
        }
        ok
    }

    /// Reject (card-bound nonce required, same rule as approve).
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
            self.record(id, BatchAction::Reject);
        }
        ok
    }

    pub fn revoke(&mut self, id: &str) {
        if let Some(t) = self.tickets.get_mut(id) {
            t.state = TicketState::Revoked;
        }
    }

    /// The executor call-site: consume the batch ticket, requiring the exact
    /// approved change-set hash. Returns the same error taxonomy as
    /// `TicketStore::use_ticket` so callers handle one error shape.
    pub fn use_batch_ticket(
        &mut self,
        id: &str,
        change_set_hash: &str,
    ) -> Result<(), crate::ticket::TicketError> {
        let Some(t) = self.tickets.get_mut(id) else {
            return Err(crate::ticket::TicketError::Unknown);
        };
        if t.expires_at_ms != 0 && now_ms() > t.expires_at_ms {
            t.state = TicketState::Expired;
            return Err(crate::ticket::TicketError::Expired);
        }
        if t.state == TicketState::Used {
            return Err(crate::ticket::TicketError::AlreadyUsed);
        }
        if t.state == TicketState::Revoked {
            return Err(crate::ticket::TicketError::Revoked);
        }
        if t.state == TicketState::Pending {
            return Err(crate::ticket::TicketError::NotApproved);
        }
        if !t.matches_change_set(change_set_hash) {
            t.state = TicketState::Rejected;
            return Err(crate::ticket::TicketError::ArgsMismatch);
        }
        if t.single_use {
            t.state = TicketState::Used;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops() -> Vec<BatchOperation> {
        vec![
            BatchOperation::new(
                "fs.rename",
                "rename",
                "h-rename-1",
                vec![
                    "/w/Downloads/a.pdf".to_string(),
                    "/w/Downloads/Docs/a.pdf".to_string(),
                ],
            ),
            BatchOperation::new(
                "fs.rename",
                "rename",
                "h-rename-2",
                vec![
                    "/w/Downloads/b.png".to_string(),
                    "/w/Downloads/Images/b.png".to_string(),
                ],
            ),
        ]
    }

    fn minted() -> (BatchTicketStore, String) {
        let mut store = BatchTicketStore::new();
        let t = BatchTicket::mint("bt-1", "agent-1", "sess-1", ops(), RiskLevel::Medium, 7);
        let id = store.mint(t);
        (store, id)
    }

    #[test]
    fn change_set_hash_is_immutable_and_order_sensitive() {
        let a = ops();
        let b = ops();
        // Identical sets hash identically (deterministic).
        assert_eq!(change_set_hash(&a), change_set_hash(&b));
        // Removing an op changes the binding.
        let mut c = a.clone();
        c.pop();
        assert_ne!(change_set_hash(&a), change_set_hash(&c));
        // Reordering changes the binding.
        let mut d = a.clone();
        d.swap(0, 1);
        assert_ne!(change_set_hash(&a), change_set_hash(&d));
        // Mutating one op's args hash changes the binding — the approval can
        // never be stretched to different args.
        let mut e = a.clone();
        e[0].args_hash = "h-renamed".to_string();
        assert_ne!(change_set_hash(&a), change_set_hash(&e));
    }

    #[test]
    fn pending_batch_cannot_be_consumed() {
        let (mut store, id) = minted();
        let cs = store.get(&id).unwrap().change_set_hash.clone();
        assert!(matches!(
            store.use_batch_ticket(&id, &cs),
            Err(crate::ticket::TicketError::NotApproved)
        ));
    }

    #[test]
    fn approve_then_consume_exact_set() {
        let (mut store, id) = minted();
        let nonce = store.get(&id).unwrap().approval_nonce.clone();
        let cs = store.get(&id).unwrap().change_set_hash.clone();
        assert!(store.approve_with_nonce(&id, &nonce));
        // Exact set consumes.
        assert!(store.use_batch_ticket(&id, &cs).is_ok());
        // Single-use: second consume refuses.
        assert!(matches!(
            store.use_batch_ticket(&id, &cs),
            Err(crate::ticket::TicketError::AlreadyUsed)
        ));
        // Receipt recorded.
        assert_eq!(store.receipts().len(), 1);
        assert_eq!(store.receipts()[0].operation_count, 2);
    }

    #[test]
    fn approval_binds_exact_set_never_a_category() {
        let (mut store, id) = minted();
        let nonce = store.get(&id).unwrap().approval_nonce.clone();
        assert!(store.approve_with_nonce(&id, &nonce));
        // The executor presents a *different* change set (e.g. the agent
        // added a third mutation after approval): refused + marked rejected.
        let mut extra = ops();
        extra.push(BatchOperation::new(
            "fs.delete",
            "delete",
            "h-delete",
            vec!["/w/Downloads/secret".to_string()],
        ));
        let other_hash = change_set_hash(&extra);
        assert!(matches!(
            store.use_batch_ticket(&id, &other_hash),
            Err(crate::ticket::TicketError::ArgsMismatch)
        ));
        assert_eq!(store.get(&id).unwrap().state, TicketState::Rejected);
    }

    #[test]
    fn wrong_nonce_cannot_approve() {
        let (mut store, id) = minted();
        assert!(!store.approve_with_nonce(&id, "forged-nonce"));
        assert_eq!(store.get(&id).unwrap().state, TicketState::Pending);
        assert!(store.receipts().is_empty());
    }

    #[test]
    fn reject_binds_nonce_and_records() {
        let (mut store, id) = minted();
        let nonce = store.get(&id).unwrap().approval_nonce.clone();
        assert!(store.reject_with_nonce(&id, &nonce));
        assert_eq!(store.get(&id).unwrap().state, TicketState::Revoked);
        assert_eq!(store.receipts().len(), 1);
        assert_eq!(store.receipts()[0].action, BatchAction::Reject);
    }

    #[test]
    fn revoke_blocks_consume() {
        let (mut store, id) = minted();
        store.approve(&id);
        store.revoke(&id);
        let cs = store.get(&id).unwrap().change_set_hash.clone();
        assert!(matches!(
            store.use_batch_ticket(&id, &cs),
            Err(crate::ticket::TicketError::Revoked)
        ));
    }
}
