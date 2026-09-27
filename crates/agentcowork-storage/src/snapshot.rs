//! Immutable arena snapshots + zstd persistence
//! (eDirStat lock-free coordinator @~100ms via `arc_swap`, doc 49 §2).
//!
//! The store holds an `Arc<Snapshot>` behind an `ArcSwap`, so readers get a
//! lock-free, consistent view while a background scanner publishes a new
//! snapshot. Persistence is zstd-compressed JSON (headless snapshot).

use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

use crate::StorageError;
use crate::walk::Arena;

/// One immutable scan result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub created_at: u64,
    pub root: String,
    pub arena: Arena,
}

/// Lock-free snapshot store (`ArcSwap<Arc<Snapshot>>`).
pub struct SnapshotStore {
    inner: ArcSwap<Snapshot>,
}

impl SnapshotStore {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            inner: ArcSwap::from_pointee(snapshot),
        }
    }

    /// Lock-free read of the current snapshot.
    pub fn current(&self) -> Arc<Snapshot> {
        self.inner.load_full()
    }

    /// Atomically publish a new snapshot (readers keep their old one).
    pub fn publish(&self, snapshot: Snapshot) {
        self.inner.store(Arc::new(snapshot));
    }

    /// Persist the current snapshot as zstd-compressed JSON.
    pub fn save_to(&self, path: &Path) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec(&*self.current())?;
        let compressed = zstd::bulk::compress(&bytes, 3)?;
        std::fs::write(path, compressed)?;
        Ok(())
    }

    /// Load a zstd-compressed snapshot.
    pub fn load_from(path: &Path) -> Result<Self, StorageError> {
        let compressed = std::fs::read(path)?;
        let bytes = zstd::bulk::decompress(&compressed, 1 << 26)?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
        Ok(Self::new(snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{ScanOptions, build_arena, scan};
    use std::fs;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "agentcowork-storage-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn zstd_round_trip() {
        let root = tmpdir("snap");
        fs::write(root.join("f.txt"), b"data").unwrap();
        let records = scan(&root, &ScanOptions::default()).unwrap();
        let arena = build_arena(records, &root);

        let snap = Snapshot {
            created_at: 123,
            root: root.to_string_lossy().into_owned(),
            arena,
        };
        let store = SnapshotStore::new(snap.clone());
        let out = root.join("snap.bin");
        store.save_to(&out).unwrap();
        assert!(out.exists());

        let loaded = SnapshotStore::load_from(&out).unwrap();
        assert_eq!(loaded.current().arena, snap.arena);
        assert_eq!(loaded.current().created_at, 123);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_swaps_atomically() {
        let s1 = Snapshot {
            created_at: 1,
            root: "/a".into(),
            arena: Arena::default(),
        };
        let s2 = Snapshot {
            created_at: 2,
            root: "/b".into(),
            arena: Arena::default(),
        };
        let store = SnapshotStore::new(s1);
        assert_eq!(store.current().created_at, 1);
        store.publish(s2);
        assert_eq!(store.current().created_at, 2);
    }

    #[test]
    fn pre_fix_snapshots_still_load_with_an_unknown_identity() {
        // FIX-10 added `identity` to `FileNode`. A snapshot written before that
        // must still deserialize — the missing field defaults to the honest
        // "unknown" identity, never a fabricated zero.
        let legacy = serde_json::json!({
            "created_at": 1,
            "root": "/a",
            "arena": {
                "nodes": [{
                    "id": 0,
                    "parent": 4294967295u64,
                    "name": "a",
                    "path": "/a",
                    "size": 4,
                    "mtime": 0,
                    "is_dir": true,
                    "nlink": 0,
                    "dev": 0,
                    "ino": 0,
                }],
            },
        });
        let snap: Snapshot = serde_json::from_value(legacy).unwrap();
        let node = &snap.arena.nodes[0];
        assert!(
            node.identity.is_unknown(),
            "a legacy node is unknown, not real"
        );
        assert_eq!(node.identity.legacy_parts().1, 0, "never a fabricated id");
        // nlink defaults to 1 so a legacy node can never group as a hardlink.
        assert_eq!(node.identity.nlink(), 1);
    }
}
