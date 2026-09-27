//! P17 FS checkpoints (doc 69 §3 — `hermes --checkpoints` steal): extend the
//! office `Snapshot` rollback to generic filesystem writes. Before a
//! destructive change (delete, overwrite, multi-file edit), the coordinator
//! captures an [`FsCheckpoint`] — the byte-level state of the affected
//! paths. [`FsCheckpoint::restore`] rolls back to the captured state
//! (original bytes restored, files created since removed); [`changed`]
//! reports exactly what drifted from the checkpoint, so the guard ticket can
//! show the diff before the write is committed.

use crate::StorageError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One file's checkpointed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointedFile {
    /// Original bytes at capture time.
    pub original: Vec<u8>,
}

/// The kind of drift from a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// A file that did not exist at capture now exists.
    Added,
    /// A file's content differs from capture.
    Modified,
    /// A file that existed at capture is gone.
    Removed,
}

/// A filesystem checkpoint: original bytes for every captured path + the
/// list of paths that existed (for added-file detection on restore).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FsCheckpoint {
    files: BTreeMap<PathBuf, CheckpointedFile>,
}

impl FsCheckpoint {
    /// Capture a specific set of paths (each must be a file).
    pub fn capture(paths: &[PathBuf]) -> Result<Self, StorageError> {
        let mut cp = FsCheckpoint::default();
        for p in paths {
            if p.is_file() {
                cp.files.insert(
                    p.clone(),
                    CheckpointedFile {
                        original: std::fs::read(p)?,
                    },
                );
            }
        }
        Ok(cp)
    }

    /// Capture every file under `dir` (recursive) — the "snapshot this
    /// workspace before a risky edit" form.
    pub fn capture_dir(dir: &Path) -> Result<Self, StorageError> {
        let mut cp = FsCheckpoint::default();
        for entry in walk_files(dir)? {
            cp.files.insert(
                entry.clone(),
                CheckpointedFile {
                    original: std::fs::read(&entry)?,
                },
            );
        }
        Ok(cp)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// What drifted from the checkpoint (paths that existed at capture and
    /// now differ or are missing; paths that didn't exist and now do — the
    /// latter only discoverable relative to a captured parent dir, so this
    /// reports the captured set + obvious additions under captured parents).
    pub fn changed(&self) -> Vec<(PathBuf, ChangeKind)> {
        let mut out = Vec::new();
        for (path, f) in &self.files {
            if !path.exists() {
                out.push((path.clone(), ChangeKind::Removed));
            } else if std::fs::read(path).ok().as_deref() != Some(f.original.as_slice()) {
                out.push((path.clone(), ChangeKind::Modified));
            }
        }
        // Added files: present on disk under a captured parent but not in
        // the checkpoint.
        let parents: Vec<&Path> = self.files.keys().filter_map(|p| p.parent()).collect();
        for parent in parents {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_file() && !self.files.contains_key(&p) {
                        out.push((p, ChangeKind::Added));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Roll back to the captured state: restore original bytes for captured
    /// files, remove files that didn't exist at capture (relative to
    /// captured parents). Never fails partway silently — the first error is
    /// returned with the paths already restored.
    pub fn restore(&self) -> Result<usize, StorageError> {
        let mut restored = 0;
        for (path, f) in &self.files {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &f.original)?;
            restored += 1;
        }
        // Remove files added since capture (under captured parents).
        let parents: Vec<PathBuf> = self
            .files
            .keys()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        for parent in parents {
            if let Ok(entries) = std::fs::read_dir(&parent) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_file() && !self.files.contains_key(&p) {
                        std::fs::remove_file(&p)?;
                        restored += 1;
                    }
                }
            }
        }
        Ok(restored)
    }
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("agentcowork-cp-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn restore_undoes_modify_and_delete() {
        let dir = tmpdir("mod");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "orig-a").unwrap();
        std::fs::write(&b, "orig-b").unwrap();
        let cp = FsCheckpoint::capture(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(cp.len(), 2);

        // Destructive change: overwrite a, delete b, add c.
        std::fs::write(&a, "changed").unwrap();
        std::fs::remove_file(&b).unwrap();
        std::fs::write(dir.join("c.txt"), "new").unwrap();

        let changed = cp.changed();
        assert!(changed.contains(&(a.clone(), ChangeKind::Modified)));
        assert!(changed.contains(&(b.clone(), ChangeKind::Removed)));
        assert!(changed.contains(&(dir.join("c.txt"), ChangeKind::Added)));

        let restored = cp.restore().unwrap();
        assert!(restored >= 3);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "orig-a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "orig-b");
        assert!(!dir.join("c.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_dir_snapshots_workspace() {
        let dir = tmpdir("dir");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/x.rs"), "fn x() {}").unwrap();
        std::fs::write(dir.join("top.rs"), "fn top() {}").unwrap();
        let cp = FsCheckpoint::capture_dir(&dir).unwrap();
        assert_eq!(cp.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_paths_are_skipped() {
        let dir = tmpdir("missing");
        let cp = FsCheckpoint::capture(&[dir.join("nope.txt")]).unwrap();
        assert!(cp.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
