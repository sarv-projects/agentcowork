//! Cognee-pattern memory API (P5.2 — doc 63 §2.1): `remember` / `recall` /
//! `forget` / `improve` as the coordinator-facing facade over the paged
//! memory store. The four verbs are the durable contract; the paging engine
//! underneath (core/archival/recall surfaces) does the storage.

use crate::paging::{MemoryEntry, PagedMemory};

/// The four Cognee-style memory verbs' result.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallResult {
    /// Entries relevant to the query, best first.
    pub entries: Vec<MemoryEntry>,
}

/// The memory facade.
#[derive(Debug, Default)]
pub struct CogneeMemory {
    store: PagedMemory,
    /// Generation counter — bumped on every write so `improve` can tell what
    /// changed since the last consolidation pass.
    revision: u64,
}

impl CogneeMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// `remember` — store a new fact/memory. Queued writes land at the next
    /// turn boundary (via [`flush`]).
    pub fn remember(&mut self, id: &str, content: &str, importance: u8) {
        self.store.write(MemoryEntry {
            id: id.to_string(),
            content: content.to_string(),
            importance,
        });
        self.revision += 1;
    }

    /// Apply queued remembers/forgets (the turn boundary).
    pub fn flush(&mut self) {
        self.store.flush_writes();
    }

    /// `recall` — retrieve memories relevant to `query` (importance-ordered
    /// substring matches; the BM25/vector signals layer on top via `bm25`).
    pub fn recall(&self, query: &str) -> RecallResult {
        let entries: Vec<MemoryEntry> = self.store.search(query).into_iter().cloned().collect();
        RecallResult { entries }
    }

    /// `recall_all` — every entry across all surfaces (the consolidation view).
    pub fn recall_all(&self) -> Vec<MemoryEntry> {
        self.store.all_entries().into_iter().cloned().collect()
    }

    /// `forget` — remove a memory (queued; applied at the next flush).
    pub fn forget(&mut self, id: &str) {
        self.store.forget(id);
        self.revision += 1;
    }

    /// `improve` — the consolidation pass: promote a high-importance entry
    /// into the core warm set and/or re-rank. The pure core here reports what
    /// *should* happen (the coordinator's improve loop calls this after each
    /// session). Returns the ids touched.
    pub fn improve(&mut self, min_importance_for_core: u8) -> Vec<String> {
        let mut touched = Vec::new();
        // Promote entries the user explicitly wants kept (importance ≥ the
        // floor) by re-writing them with a bumped importance.
        let candidates: Vec<MemoryEntry> = self.recall_all();
        for entry in candidates {
            if entry.importance >= min_importance_for_core {
                // Rewriting an existing id replaces it (idempotent).
                self.remember(&entry.id, &entry.content, entry.importance.max(10));
                touched.push(entry.id);
            }
        }
        self.flush();
        touched
    }

    /// The current revision (how many writes/forgets happened — the delta the
    /// improve loop uses to decide whether a pass is due).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn surface_of(&self, id: &str) -> Option<crate::paging::Surface> {
        self.store.surface_of(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::Surface;

    #[test]
    fn remember_recall_roundtrip() {
        let mut mem = CogneeMemory::new();
        mem.remember("m1", "the launch is on friday", 7);
        mem.flush();
        let r = mem.recall("friday");
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].id, "m1");
    }

    #[test]
    fn recall_orders_by_importance() {
        let mut mem = CogneeMemory::new();
        mem.remember("low", "browser window management tip", 2);
        mem.remember("high", "browser security rule", 9);
        mem.flush();
        let r = mem.recall("browser");
        assert_eq!(r.entries[0].id, "high");
        assert_eq!(r.entries[1].id, "low");
    }

    #[test]
    fn forget_removes() {
        let mut mem = CogneeMemory::new();
        mem.remember("m1", "remember this", 5);
        mem.flush();
        mem.forget("m1");
        mem.flush();
        assert!(mem.recall("remember").entries.is_empty());
        assert_eq!(mem.surface_of("m1"), None);
    }

    #[test]
    fn improve_promotes_important_entries() {
        let mut mem = CogneeMemory::new();
        mem.remember("keep", "important fact", 8);
        mem.remember("drop", "minor note", 1);
        mem.flush();
        let touched = mem.improve(5);
        assert!(touched.contains(&"keep".to_string()));
        assert!(!touched.contains(&"drop".to_string()));
        // The kept entry is now in the core warm set.
        assert_eq!(mem.surface_of("keep"), Some(Surface::Core));
    }

    #[test]
    fn revision_tracks_writes() {
        let mut mem = CogneeMemory::new();
        assert_eq!(mem.revision(), 0);
        mem.remember("a", "x", 5);
        assert_eq!(mem.revision(), 1);
        mem.forget("a");
        assert_eq!(mem.revision(), 2);
    }
}
