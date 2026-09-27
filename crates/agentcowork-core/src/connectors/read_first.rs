//! P18-3 — Read-first external-connector posture (doc 70 §2 — `mailwarden`
//! / `Busymail` 🔴 STEAL the pattern).
//!
//! The rule set for the first real external connector (Gmail/IMAP): **read
//! is free, send is a ticket**. No silent outbound — every send executes
//! only after an explicit approval (the Guard-2 shape: single-use, bound to
//! the exact action's args hash). Tokens live in the SQLCipher vault and are
//! referenced, never held, by the connector (the vault crate owns storage;
//! this module declares the reference shape).
//!
//! The policy itself never auto-approves: `approve` requires the caller to
//! present the approval decision (from the Guard-2 card flow); the policy
//! *classifies* the action (open-world vs trusted) so the UI can render the
//! right card.

use serde::{Deserialize, Serialize};

/// What is about to leave the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendKind {
    Email { to: String, subject: String },
    Reply { to: String, subject: String },
    Draft { to: Option<String>, subject: String },
    Calendar { summary: String },
    Other { label: String },
}

impl SendKind {
    /// Human summary for the approval card.
    pub fn summary(&self) -> String {
        match self {
            SendKind::Email { to, subject } => format!("Email to {to}: {subject}"),
            SendKind::Reply { to, subject } => format!("Reply to {to}: {subject}"),
            SendKind::Draft { to, subject } => match to {
                Some(t) => format!("Draft to {t}: {subject}"),
                None => format!("Draft: {subject}"),
            },
            SendKind::Calendar { summary } => format!("Calendar invite: {summary}"),
            SendKind::Other { label } => format!("Send: {label}"),
        }
    }
}

/// A send attempt, described for classification + approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendAction {
    pub kind: SendKind,
    /// Deterministic hash over the full outbound payload (args). The ticket
    /// binds to it: an edited payload invalidates the approval.
    pub args_hash: String,
    /// Trusted (known sender/domain, first-party) vs open-world (anything
    /// the agent discovered). Open-world sends need a stronger card.
    pub open_world: bool,
}

impl SendAction {
    /// Hash the payload deterministically (FNV-1a — no crypto needed for a
    /// binding check; the audit trail uses the same string).
    pub fn hash_payload(parts: &[&str]) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for part in parts {
            for b in part.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h ^= 0xff;
        }
        format!("{h:016x}")
    }
}

/// The Guard-2-shaped approval a send must carry. Single-use: `verify`
/// consumes it (the caller keeps the issued set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendApproval {
    pub ticket_id: String,
    /// Must equal the action's `args_hash` — the card approved *this* payload.
    pub bound_args_hash: String,
    pub reason: String,
}

/// Why a send was refused (never a silent failure — always an honest reason).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendBlocked {
    #[error("no approval presented — outbound sends require a Guard-2 ticket")]
    NoApproval,
    #[error("approval {0} does not match this payload (stale or edited)")]
    StaleApproval(String),
    #[error("approval {0} already used")]
    AlreadyUsed(String),
    #[error(
        "open-world send to an untrusted recipient requires explicit approval (none presented)"
    )]
    OpenWorldUntrusted,
}

/// The read-first policy: read methods pass through, sends must carry a
/// verified single-use approval. Never auto-approves.
#[derive(Debug, Default, Clone)]
pub struct ReadFirstPolicy {
    used_tickets: std::collections::HashSet<String>,
}

impl ReadFirstPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify an action: trusted (first-party, non-open-world) vs the
    /// open-world case that demands the stronger card.
    pub fn classify(&self, action: &SendAction) -> SendClass {
        if action.open_world {
            SendClass::OpenWorld
        } else {
            SendClass::Trusted
        }
    }

    /// Gate a send. `approval` is the decision from the Guard-2 card flow.
    /// Verifies: present, not replayed, bound to the exact payload hash.
    pub fn approve_before_send(
        &mut self,
        action: &SendAction,
        approval: Option<&SendApproval>,
    ) -> Result<(), SendBlocked> {
        if action.open_world && approval.is_none() {
            return Err(SendBlocked::OpenWorldUntrusted);
        }
        let Some(a) = approval else {
            return Err(SendBlocked::NoApproval);
        };
        if a.bound_args_hash != action.args_hash {
            return Err(SendBlocked::StaleApproval(a.ticket_id.clone()));
        }
        if !self.used_tickets.insert(a.ticket_id.clone()) {
            return Err(SendBlocked::AlreadyUsed(a.ticket_id.clone()));
        }
        Ok(())
    }
}

/// The classification the UI renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendClass {
    /// Known sender, first-party payload — standard Guard-2 card.
    Trusted,
    /// Agent-discovered recipient/payload — explicit-approval card.
    OpenWorld,
}

/// The token reference shape: connectors never hold secrets, they hold a
/// vault key id (SQLCipher vault owns the bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultTokenRef {
    pub key_id: String,
    pub service: String,
}

impl VaultTokenRef {
    pub fn new(key_id: &str, service: &str) -> Self {
        Self {
            key_id: key_id.to_string(),
            service: service.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(to: &str, subject: &str, open_world: bool) -> SendAction {
        let payload = format!("{to}\n{subject}\nbody");
        SendAction {
            kind: SendKind::Email {
                to: to.to_string(),
                subject: subject.to_string(),
            },
            args_hash: SendAction::hash_payload(&[&payload]),
            open_world,
        }
    }

    fn approve(action: &SendAction, id: &str) -> SendApproval {
        SendApproval {
            ticket_id: id.to_string(),
            bound_args_hash: action.args_hash.clone(),
            reason: "user confirmed".into(),
        }
    }

    #[test]
    fn read_is_free_send_needs_a_ticket() {
        let mut policy = ReadFirstPolicy::new();
        let action = email("a@example.com", "hello", false);
        // no approval → blocked, honest reason
        assert_eq!(
            policy.approve_before_send(&action, None),
            Err(SendBlocked::NoApproval)
        );
        // with the right ticket → allowed
        policy
            .approve_before_send(&action, Some(&approve(&action, "t1")))
            .unwrap();
    }

    #[test]
    fn approval_is_single_use() {
        let mut policy = ReadFirstPolicy::new();
        let action = email("a@example.com", "hello", false);
        let a = approve(&action, "t1");
        policy.approve_before_send(&action, Some(&a)).unwrap();
        assert_eq!(
            policy.approve_before_send(&action, Some(&a)),
            Err(SendBlocked::AlreadyUsed("t1".into()))
        );
    }

    #[test]
    fn edited_payload_invalidates_the_approval() {
        let mut policy = ReadFirstPolicy::new();
        let original = email("a@example.com", "hello", false);
        let a = approve(&original, "t1");
        // payload edited after approval (subject changed → new hash)
        let edited = email("a@example.com", "hello WORLD", false);
        assert_eq!(
            policy.approve_before_send(&edited, Some(&a)),
            Err(SendBlocked::StaleApproval("t1".into()))
        );
    }

    #[test]
    fn open_world_never_silent() {
        let mut policy = ReadFirstPolicy::new();
        let action = email("stranger@unknown-domain.io", "hi", true);
        assert_eq!(
            policy.approve_before_send(&action, None),
            Err(SendBlocked::OpenWorldUntrusted)
        );
        // even with a ticket, it must be the matching one
        policy
            .approve_before_send(&action, Some(&approve(&action, "t9")))
            .unwrap();
        assert_eq!(policy.classify(&action), SendClass::OpenWorld);
    }

    #[test]
    fn token_ref_never_holds_bytes() {
        let r = VaultTokenRef::new("k-42", "gmail");
        assert_eq!(r.key_id, "k-42");
        assert_eq!(r.service, "gmail");
    }
}
