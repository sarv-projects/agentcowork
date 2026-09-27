//! Letta-style memory paging (C2, Algorithm #20 — doc 07, doc 34 §2).
//!
//! Three surfaces — **core** (≤600 tokens), **archival**, **recall** — with
//! agent memory tools (`read`/`write`/`search`/`forget`). Writes are queued to
//! turn boundaries (protecting the prompt-cache prefix); on flush, core
//! overflow pages the lowest-importance entries out to archival.

use crate::fusion::approx_tokens;

pub const CORE_BUDGET_TOKENS: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Core,
    Archival,
    Recall,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub importance: u8,
}

impl MemoryEntry {
    fn tokens(&self) -> usize {
        approx_tokens(&self.content)
    }
}

#[derive(Debug, Clone)]
enum PendingWrite {
    Write(MemoryEntry),
    Forget(String),
}

#[derive(Debug, Default)]
pub struct PagedMemory {
    core: Vec<MemoryEntry>,
    archival: Vec<MemoryEntry>,
    recall: Vec<MemoryEntry>,
    pending: Vec<PendingWrite>,
}

impl PagedMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn core_tokens(&self) -> usize {
        self.core.iter().map(MemoryEntry::tokens).sum()
    }

    pub fn core_len(&self) -> usize {
        self.core.len()
    }

    pub fn archival_len(&self) -> usize {
        self.archival.len()
    }

    pub fn recall_len(&self) -> usize {
        self.recall.len()
    }

    /// Queue a write to be applied at the next turn boundary.
    pub fn write(&mut self, entry: MemoryEntry) {
        self.pending.push(PendingWrite::Write(entry));
    }

    /// Queue a forget.
    pub fn forget(&mut self, id: &str) {
        self.pending.push(PendingWrite::Forget(id.to_string()));
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Apply queued writes/forgets, then page core overflow to archival.
    pub fn flush_writes(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            match p {
                PendingWrite::Write(e) => self.core.push(e),
                PendingWrite::Forget(id) => self.remove_everywhere(&id),
            }
        }
        self.page_overflow();
    }

    /// Read (and promote to the recall surface) by id.
    pub fn read(&mut self, id: &str) -> Option<MemoryEntry> {
        let entry = if let Some(i) = self.core.iter().position(|e| e.id == id) {
            self.core.remove(i)
        } else if let Some(i) = self.archival.iter().position(|e| e.id == id) {
            self.archival.remove(i)
        } else {
            self.recall
                .iter()
                .position(|e| e.id == id)
                .map(|i| self.recall.remove(i))?
        };
        let ret = entry.clone();
        self.recall.push(entry);
        Some(ret)
    }

    /// Substring search across all three surfaces (importance-ordered).
    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let q = query.to_lowercase();
        let mut out: Vec<&MemoryEntry> = self
            .core
            .iter()
            .chain(&self.archival)
            .chain(&self.recall)
            .filter(|e| e.content.to_lowercase().contains(&q))
            .collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.importance));
        out
    }

    /// Every entry across all surfaces (recall surface first, then archival,
    /// then core — the debug/consolidation view).
    pub fn all_entries(&self) -> Vec<&MemoryEntry> {
        self.recall
            .iter()
            .chain(&self.archival)
            .chain(&self.core)
            .collect()
    }

    /// Surface of an entry (for inspection/tests).
    pub fn surface_of(&self, id: &str) -> Option<Surface> {
        if self.core.iter().any(|e| e.id == id) {
            Some(Surface::Core)
        } else if self.archival.iter().any(|e| e.id == id) {
            Some(Surface::Archival)
        } else if self.recall.iter().any(|e| e.id == id) {
            Some(Surface::Recall)
        } else {
            None
        }
    }

    fn remove_everywhere(&mut self, id: &str) {
        self.core.retain(|e| e.id != id);
        self.archival.retain(|e| e.id != id);
        self.recall.retain(|e| e.id != id);
    }

    fn page_overflow(&mut self) {
        while self.core_tokens() > CORE_BUDGET_TOKENS && !self.core.is_empty() {
            // Evict the lowest-importance core entry (stable → oldest first).
            let idx = self
                .core
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.importance)
                .map(|(i, _)| i)
                .unwrap();
            let e = self.core.remove(idx);
            self.archival.push(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, content: &str, importance: u8) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            content: content.to_string(),
            importance,
        }
    }

    #[test]
    fn writes_queue_to_turn_boundary() {
        let mut m = PagedMemory::new();
        m.write(entry("a", "hello world", 5));
        assert_eq!(m.core_len(), 0);
        assert!(m.has_pending());
        m.flush_writes();
        assert_eq!(m.core_len(), 1);
        assert!(!m.has_pending());
    }

    #[test]
    fn core_overflow_pages_lowest_importance() {
        let mut m = PagedMemory::new();
        let big = "x".repeat(CORE_BUDGET_TOKENS * 4); // ≈ 600 tokens
        m.write(entry("important", &big, 9));
        m.write(entry("filler", &big, 2));
        m.flush_writes();
        // Two ~600-token entries overflow core (600); filler (importance 2)
        // is paged out first.
        assert_eq!(m.surface_of("important"), Some(Surface::Core));
        assert_eq!(m.surface_of("filler"), Some(Surface::Archival));
        assert!(m.core_tokens() <= CORE_BUDGET_TOKENS);
    }

    #[test]
    fn read_promotes_to_recall() {
        let mut m = PagedMemory::new();
        m.write(entry("a", "one", 5));
        m.flush_writes();
        assert_eq!(m.surface_of("a"), Some(Surface::Core));
        let got = m.read("a").unwrap();
        assert_eq!(got.id, "a");
        assert_eq!(m.surface_of("a"), Some(Surface::Recall));
        assert_eq!(m.recall_len(), 1);
    }

    #[test]
    fn forget_removes_from_all_surfaces() {
        let mut m = PagedMemory::new();
        m.write(entry("a", "one", 5));
        m.write(entry("b", "two", 5));
        m.flush_writes();
        m.forget("a");
        m.flush_writes();
        assert_eq!(m.surface_of("a"), None);
        assert_eq!(m.surface_of("b"), Some(Surface::Core));
    }

    #[test]
    fn all_entries_spans_surfaces() {
        let mut m = PagedMemory::new();
        m.write(entry("a", "one", 5));
        m.write(entry("b", "two", 5));
        m.flush_writes();
        assert_eq!(m.all_entries().len(), 2);
        let ids: Vec<&str> = m.all_entries().iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"a") && ids.contains(&"b"));
    }

    #[test]
    fn search_spans_surfaces() {
        let mut m = PagedMemory::new();
        m.write(entry("a", "rust memory system", 3));
        m.write(entry("b", "browser automation", 8));
        m.flush_writes();
        let hits = m.search("rust");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }
}
