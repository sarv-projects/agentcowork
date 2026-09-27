//! P7.7 — Merkle hash-chain upgrade for the audit log (OpenFang pattern).
//!
//! Every event is chained to the previous by hash: `hash_n = SHA256(
//! hash_{n-1} || seq_n || kind_n || payload_n)`. Any edit to an earlier row
//! (or a reordering/replay) breaks the chain at the point of tampering, so
//! the log is tamper-evident: verification tells you *which* row first
//! disagrees with its predecessor, and whether the chain as stored is
//! self-consistent.

use crate::AuditEvent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A chained entry: the event plus its Merkle hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChainedEntry {
    pub event: AuditEvent,
    /// SHA-256 of (prev_hash ‖ seq ‖ kind ‖ canonical payload).
    pub hash: String,
    /// Hash of the previous entry ("" for the first row).
    pub prev_hash: String,
}

/// In-memory Merkle chain (the file-backed writer in [`crate::AuditWriter`]
/// is the durable half; this is the tamper-evidence layer on top of it).
#[derive(Debug, Default, Clone)]
pub struct MerkleChain {
    entries: Vec<ChainedEntry>,
}

/// Canonical bytes hashed for an entry.
fn hash_input(prev: &str, seq: u64, kind: &str, payload: &serde_json::Value) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(prev.as_bytes());
    buf.push(0u8);
    buf.extend_from_slice(seq.to_string().as_bytes());
    buf.push(0u8);
    buf.extend_from_slice(kind.as_bytes());
    buf.push(0u8);
    buf.extend_from_slice(serde_json::to_vec(payload).unwrap_or_default().as_slice());
    buf
}

fn sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl MerkleChain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an event, chaining to the previous row. Returns the entry's hash.
    pub fn push(&mut self, event: AuditEvent) -> String {
        let prev = self
            .entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_default();
        let hash = sha256(&hash_input(&prev, event.seq, &event.kind, &event.payload));
        self.entries.push(ChainedEntry {
            event,
            hash: hash.clone(),
            prev_hash: prev,
        });
        hash
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The latest hash (the chain head — store this externally as the
    /// commitment; any later verification compares against it).
    pub fn head(&self) -> Option<&str> {
        self.entries.last().map(|e| e.hash.as_str())
    }

    /// Verify the whole chain is self-consistent (each row's hash matches
    /// its content + predecessor, and prev_hash links match). Returns the
    /// first bad row index, or None if the chain is intact.
    pub fn verify(&self) -> Option<usize> {
        let mut prev = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.prev_hash != prev {
                return Some(i);
            }
            let expect = sha256(&hash_input(
                &prev,
                e.event.seq,
                &e.event.kind,
                &e.event.payload,
            ));
            if e.hash != expect {
                return Some(i);
            }
            prev = e.hash.clone();
        }
        None
    }

    /// Verify against an externally-stored head commitment (detects a
    /// truncated tail too).
    pub fn verify_against_head(&self, committed_head: &str) -> Result<(), MerkleError> {
        match self.verify() {
            Some(i) => Err(MerkleError::TamperedAt(i)),
            None => {
                if self.head() != Some(committed_head) {
                    Err(MerkleError::HeadMismatch)
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MerkleError {
    #[error("chain tampered at row {0}")]
    TamperedAt(usize),
    #[error("chain head does not match commitment")]
    HeadMismatch,
}

/// Deterministic canonical payload helper: re-serialize the payload so two
/// logically-identical events hash identically regardless of key order.
pub fn canonical_payload(payload: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(payload).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, kind: &str) -> AuditEvent {
        AuditEvent::new(kind, serde_json::json!({"n": seq})).with_trace("t", "s")
    }

    #[test]
    fn chain_verifies_when_intact() {
        let mut c = MerkleChain::new();
        for i in 1..=5 {
            c.push(event(i, "tool.called"));
        }
        assert_eq!(c.verify(), None);
        assert_eq!(c.len(), 5);
        assert!(c.head().is_some());
    }

    #[test]
    fn tampered_middle_row_detected() {
        let mut c = MerkleChain::new();
        for i in 1..=5 {
            c.push(event(i, "tool.called"));
        }
        // Tamper with row 2's payload (sequence numbers are the payload).
        c.entries[1].event.payload = serde_json::json!({"n": 999});
        let bad = c.verify();
        assert_eq!(bad, Some(1));
    }

    #[test]
    fn reordered_rows_detected() {
        let mut c = MerkleChain::new();
        for i in 1..=4 {
            c.push(event(i, "tool.called"));
        }
        c.entries.swap(1, 2);
        let bad = c.verify();
        assert!(bad.is_some());
    }

    #[test]
    fn head_commitment_catches_truncation() {
        let mut c = MerkleChain::new();
        for i in 1..=5 {
            c.push(event(i, "tool.called"));
        }
        let head = c.head().unwrap().to_string();
        assert!(c.verify_against_head(&head).is_ok());
        // Truncate the tail → head mismatch.
        c.entries.pop();
        assert_eq!(c.verify_against_head(&head), Err(MerkleError::HeadMismatch));
    }
}
