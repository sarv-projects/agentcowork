//! D7 — `snapshotBefore` rollback (GenOffice hook, doc 28 §2; ARCH/04 §4.4).
//!
//! Before any edit, the coordinator captures the pre-edit bytes. After each
//! successful save it records the new bytes; `undo` restores the original.
//! This is the "one-click undo + crash recovery" guarantee — the pre-edit
//! ZIP is kept in memory (and, at the call site, on disk via `atomic`) until
//! the edit is confirmed.

/// The pre-edit snapshot kept for one-click undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The original file bytes (pre-edit) — the undo target.
    original: Vec<u8>,
    /// The latest saved bytes (post-edit).
    current: Vec<u8>,
}

impl Snapshot {
    /// Capture the original bytes before any edit.
    pub fn capture(original: Vec<u8>) -> Self {
        Self {
            original: original.clone(),
            current: original,
        }
    }

    /// The pre-edit bytes (the one-click undo target).
    pub fn original(&self) -> &[u8] {
        &self.original
    }

    /// The latest bytes (what is on disk after the last save).
    pub fn current(&self) -> &[u8] {
        &self.current
    }

    /// Record the bytes after a successful save. `original` is preserved.
    pub fn record_save(&mut self, saved: Vec<u8>) {
        self.current = saved;
    }

    /// Undo to the pre-edit bytes; `current` becomes `original`.
    pub fn undo(&mut self) -> Vec<u8> {
        self.current.clone_from(&self.original);
        self.current.clone()
    }

    /// Whether an edit has been recorded (i.e. `current != original`).
    pub fn dirty(&self) -> bool {
        self.current != self.original
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_keeps_original_after_save() {
        let mut s = Snapshot::capture(b"before".to_vec());
        s.record_save(b"after".to_vec());
        assert_eq!(s.original(), b"before");
        assert_eq!(s.current(), b"after");
        assert!(s.dirty());
    }

    #[test]
    fn undo_restores_original() {
        let mut s = Snapshot::capture(b"before".to_vec());
        s.record_save(b"after".to_vec());
        let undone = s.undo();
        assert_eq!(undone, b"before");
        assert_eq!(s.current(), b"before");
        assert!(!s.dirty());
    }

    #[test]
    fn fresh_snapshot_is_not_dirty() {
        let s = Snapshot::capture(b"same".to_vec());
        assert!(!s.dirty());
        assert_eq!(s.current(), b"same");
    }
}
