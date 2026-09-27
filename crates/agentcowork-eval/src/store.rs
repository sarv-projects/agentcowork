//! Evidence-bundle persistent store (P8.0 — the \"runtime bundle store\" half
//! of doc 63 §2.3): [`EvidenceBundle`]s land on disk as JSON, keyed by task
//! id, so a completed run's proof (hashes, validator reports, screenshots,
//! approval events) survives the process and can be audited later.

use crate::evidence::EvidenceBundle;
use std::io;
use std::path::{Path, PathBuf};

/// File extension for stored bundles.
const EXT: &str = "json";

/// An on-disk evidence store rooted at one directory.
#[derive(Debug, Clone)]
pub struct EvidenceStore {
    root: PathBuf,
}

impl EvidenceStore {
    /// Open (creating if needed) a store at `root`.
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// The store's root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file a task's bundle lives in (task ids are slashes-cleaned so a
    /// task id can never escape the root).
    fn path_for(&self, task_id: &str) -> PathBuf {
        let safe: String = task_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join(format!("{safe}.{EXT}"))
    }

    /// Persist a bundle for `task_id` (replacing any prior bundle). Returns
    /// the written path.
    pub fn save(&self, task_id: &str, bundle: &EvidenceBundle) -> io::Result<PathBuf> {
        let path = self.path_for(task_id);
        let json = serde_json::to_vec_pretty(bundle)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        std::fs::write(&path, json)?;
        Ok(path)
    }

    /// Load a task's bundle, or `None` when no bundle was ever saved.
    pub fn load(&self, task_id: &str) -> io::Result<Option<EvidenceBundle>> {
        let path = self.path_for(task_id);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let bundle = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Some(bundle))
    }

    /// List stored task ids (sorted, deterministic).
    pub fn list(&self) -> io::Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(&format!(".{EXT}")) {
                ids.push(stem.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Remove a task's bundle. Returns whether one existed.
    pub fn delete(&self, task_id: &str) -> io::Result<bool> {
        let path = self.path_for(task_id);
        if path.is_file() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::ArtifactHash;
    use crate::manifest::HashAlgorithm;
    use std::sync::atomic::{AtomicU32, Ordering};

    static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_root() -> PathBuf {
        let n = DIR_SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("agentcowork-eval-store-{}-{n}", std::process::id()))
    }

    fn bundle() -> EvidenceBundle {
        let mut b = EvidenceBundle::new();
        b.artifact_hashes.push(ArtifactHash {
            path: "out.xlsx".into(),
            algorithm: HashAlgorithm::Sha256,
            hash: "abc123".into(),
        });
        b.screenshots.push("shot.png".into());
        b
    }

    #[test]
    fn save_load_roundtrip() {
        let root = temp_root();
        let store = EvidenceStore::new(&root).unwrap();
        let path = store.save("desktop.invoice.042", &bundle()).unwrap();
        assert!(path.is_file());
        let loaded = store.load("desktop.invoice.042").unwrap().unwrap();
        assert_eq!(loaded, bundle());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_bundle_loads_none() {
        let root = temp_root();
        let store = EvidenceStore::new(&root).unwrap();
        assert!(store.load("never-saved").unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_and_delete() {
        let root = temp_root();
        let store = EvidenceStore::new(&root).unwrap();
        store.save("a", &bundle()).unwrap();
        store.save("b", &bundle()).unwrap();
        assert_eq!(
            store.list().unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(store.delete("a").unwrap());
        assert!(!store.delete("a").unwrap());
        assert_eq!(store.list().unwrap(), vec!["b".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn task_id_cannot_escape_the_root() {
        let root = temp_root();
        let store = EvidenceStore::new(&root).unwrap();
        store.save("../evil", &bundle()).unwrap();
        // The sanitized file lives inside the root only.
        let files = std::fs::read_dir(&root).unwrap().count();
        assert_eq!(files, 1);
        assert!(!root.join("../evil.json").exists());
        assert!(store.load("../evil").unwrap().is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn corrupt_bundle_errors() {
        let root = temp_root();
        let store = EvidenceStore::new(&root).unwrap();
        let path = store.path_for("bad");
        std::fs::write(&path, "not json").unwrap();
        assert!(store.load("bad").is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
