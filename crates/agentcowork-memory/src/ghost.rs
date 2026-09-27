//! Ghost-context prevention (ARCH/07 §7.5.1 — notify-crate pattern).
//!
//! Tracks which memory refs point at each filesystem path so a file delete
//! triggers an **atomic tombstone** (remove the path + all its refs in one
//! operation) and a rename triggers a **re-path** (move the refs with zero
//! re-embedding). The live `notify` wiring lives in the coordinator; this is
//! the pure, testable index it drives.

use std::collections::{HashMap, HashSet};

/// A filesystem event mapped onto ghost operations (P5.4). The storage
/// crate's `notify` watcher maps `notify::EventKind` → this, then drives the
/// index — keeping this crate free of the `notify` dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    /// Path deleted → atomic tombstone (remove path + all its refs).
    Removed(String),
    /// Path renamed → re-path (move refs, zero re-embedding).
    Renamed { from: String, to: String },
    /// Content changed in place → no structural action needed.
    Modified(String),
}

#[derive(Debug, Default)]
pub struct GhostIndex {
    /// path → set of content ids referencing that path.
    entries: HashMap<String, HashSet<String>>,
}

impl GhostIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn index(&mut self, path: &str, id: &str) {
        self.entries
            .entry(path.to_string())
            .or_default()
            .insert(id.to_string());
    }

    /// Ids referencing `path` (sorted).
    pub fn ids_for(&self, path: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .entries
            .get(path)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    pub fn path_count(&self) -> usize {
        self.entries.len()
    }

    /// Atomic tombstone eviction: remove the path and return its ref ids so
    /// the caller can evict the matching FTS5/vec/graph rows in the same
    /// transaction.
    pub fn tombstone(&mut self, path: &str) -> Vec<String> {
        let ids = self.ids_for(path);
        self.entries.remove(path);
        ids
    }

    /// Remove a single id from a path's ref set (per-fact forget — a fact is
    /// forgotten without tombstoning the whole path's refs). Returns whether
    /// the id was present. An emptied set is dropped so `ids_for` stays clean.
    pub fn remove_id(&mut self, path: &str, id: &str) -> bool {
        let mut removed = false;
        if let Some(ids) = self.entries.get_mut(path) {
            removed = ids.remove(id);
            if ids.is_empty() {
                self.entries.remove(path);
            }
        }
        removed
    }

    /// Re-path on rename: move every ref from `old` to `new`, returning how
    /// many ids moved. No content is re-embedded (zero re-embedding).
    pub fn repath(&mut self, old: &str, new: &str) -> usize {
        let moved = match self.entries.remove(old) {
            Some(ids) => ids,
            None => return 0,
        };
        let n = moved.len();
        let target = self.entries.entry(new.to_string()).or_default();
        target.extend(moved);
        n
    }

    /// Apply a filesystem event (P5.4 — the notify→GhostIndex hookup):
    /// `Removed` → tombstone, `Renamed` → repath, `Modified` → no-op.
    /// Returns the number of refs affected (0 for `Modified`).
    pub fn apply_fs_event(&mut self, event: &FsEvent) -> usize {
        match event {
            FsEvent::Removed(path) => self.tombstone(path).len(),
            FsEvent::Renamed { from, to } => self.repath(from, to),
            FsEvent::Modified(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_removes_path_and_returns_ids() {
        let mut g = GhostIndex::new();
        g.index("/docs/a.md", "mem:1");
        g.index("/docs/a.md", "mem:2");
        g.index("/docs/b.md", "mem:3");

        let ids = g.tombstone("/docs/a.md");
        assert_eq!(ids, vec!["mem:1".to_string(), "mem:2".to_string()]);
        assert_eq!(g.ids_for("/docs/a.md"), Vec::<String>::new());
        assert_eq!(g.ids_for("/docs/b.md"), vec!["mem:3".to_string()]);
    }

    #[test]
    fn apply_fs_event_maps_notify_kinds_to_ghost_ops() {
        let mut g = GhostIndex::new();
        g.index("/docs/a.md", "mem:1");
        g.index("/docs/a.md", "mem:2");
        g.index("/docs/b.md", "mem:3");

        // Remove → tombstone (2 refs affected, path gone).
        assert_eq!(g.apply_fs_event(&FsEvent::Removed("/docs/a.md".into())), 2);
        assert!(g.ids_for("/docs/a.md").is_empty());

        // Rename → repath (1 ref moved, zero re-embedding).
        assert_eq!(
            g.apply_fs_event(&FsEvent::Renamed {
                from: "/docs/b.md".into(),
                to: "/docs/c.md".into()
            }),
            1
        );
        assert_eq!(g.ids_for("/docs/c.md"), vec!["mem:3".to_string()]);
        assert!(g.ids_for("/docs/b.md").is_empty());

        // Modify → no structural action.
        assert_eq!(g.apply_fs_event(&FsEvent::Modified("/docs/c.md".into())), 0);
        assert_eq!(g.ids_for("/docs/c.md"), vec!["mem:3".to_string()]);
    }

    #[test]
    fn remove_id_forgets_one_fact_without_tombstoning_the_path() {
        let mut g = GhostIndex::new();
        g.index("memory://s1", "mem:1");
        g.index("memory://s1", "mem:2");

        assert!(g.remove_id("memory://s1", "mem:1"));
        assert_eq!(g.ids_for("memory://s1"), vec!["mem:2".to_string()]);
        assert!(
            !g.remove_id("memory://s1", "mem:1"),
            "double-remove is a no-op"
        );

        // Removing the last id drops the emptied path entirely.
        assert!(g.remove_id("memory://s1", "mem:2"));
        assert_eq!(g.ids_for("memory://s1"), Vec::<String>::new());
        assert_eq!(g.path_count(), 0);
    }

    #[test]
    fn repath_moves_refs_zero_reembedding() {
        let mut g = GhostIndex::new();
        g.index("/old/notes.md", "mem:9");
        g.index("/old/notes.md", "mem:10");

        let moved = g.repath("/old/notes.md", "/new/notes.md");
        assert_eq!(moved, 2);
        assert_eq!(g.ids_for("/old/notes.md"), Vec::<String>::new());
        assert_eq!(
            g.ids_for("/new/notes.md"),
            vec!["mem:10".to_string(), "mem:9".to_string()]
        );
    }
}
