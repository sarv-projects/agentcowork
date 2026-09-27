//! P5.4 — typed filesystem events for the ghost-index bridge.
//!
//! The FTS5 watcher historically emitted only path batches (dropping the
//! event *kind*), which is not enough to drive `memory/ghost` (a delete must
//! tombstone; a rename must re-path). This module classifies `notify` events
//! into [`FileEvent`]s and renders them to the exact `memory/ghost` params
//! shape, so the storage watcher can bridge to the ghost index without a
//! memory-crate dependency.

use notify::Watcher;
use notify::event::{EventKind, ModifyKind, RenameMode};
use std::path::PathBuf;
use std::time::Duration;

/// A classified filesystem event (kind + affected path(s)) — the storage-side
/// mirror of `agentcowork_memory::FsEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEvent {
    Removed { path: PathBuf },
    Renamed { from: PathBuf, to: PathBuf },
    Modified { path: PathBuf },
}

impl FileEvent {
    /// Classify a `notify` event into ghost-index events. Removes → `Removed`;
    /// a rename reported as `Both` → `Renamed { from, to }`; everything else
    /// (creates, data/metadata writes, and unpaired rename halves) → `Modified`
    /// (the ghost index treats `Modified` as a no-op, so an ambiguous event can
    /// never cause a false tombstone).
    pub fn from_notify(kind: &EventKind, paths: &[PathBuf]) -> Vec<FileEvent> {
        match kind {
            EventKind::Remove(_) => paths
                .iter()
                .map(|p| FileEvent::Removed { path: p.clone() })
                .collect(),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if paths.len() >= 2 => {
                vec![FileEvent::Renamed {
                    from: paths[0].clone(),
                    to: paths[1].clone(),
                }]
            }
            _ => paths
                .iter()
                .map(|p| FileEvent::Modified { path: p.clone() })
                .collect(),
        }
    }

    /// The `memory/ghost` params shape (`{kind, path}` or `{kind, from, to}`),
    /// matching what `agentcowork-core`'s `parse_fs_event` consumes.
    pub fn to_ghost_params(&self) -> serde_json::Value {
        match self {
            FileEvent::Removed { path } => serde_json::json!({
                "kind": "removed",
                "path": path.to_string_lossy(),
            }),
            FileEvent::Renamed { from, to } => serde_json::json!({
                "kind": "renamed",
                "from": from.to_string_lossy(),
                "to": to.to_string_lossy(),
            }),
            FileEvent::Modified { path } => serde_json::json!({
                "kind": "modified",
                "path": path.to_string_lossy(),
            }),
        }
    }
}

/// Watch `roots` recursively and call `on_change` with debounced, classified
/// [`FileEvent`] batches (the ghost-index bridge feeds these into
/// `memory/ghost_batch`). Same debounce discipline as `search::watch`.
pub fn watch_events<F>(
    roots: Vec<PathBuf>,
    quiet: Duration,
    mut on_change: F,
) -> Result<crate::search::WatchHandle, crate::StorageError>
where
    F: FnMut(Vec<FileEvent>) + Send + 'static,
{
    // Reuse the path-debouncer: classify events into FileEvents, debounce by
    // path freshness, flush a classified batch after the quiet period.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| crate::StorageError::Notify(e.to_string()))?;

    for r in &roots {
        watcher
            .watch(r.as_path(), notify::RecursiveMode::Recursive)
            .map_err(|e| crate::StorageError::Notify(e.to_string()))?;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let join = std::thread::spawn(move || {
        let mut pending: Vec<FileEvent> = Vec::new();
        let mut last: Option<std::time::Instant> = None;
        let mut flush = |pending: &mut Vec<FileEvent>| {
            if pending.is_empty() {
                return;
            }
            let batch = std::mem::take(pending);
            on_change(batch);
        };
        loop {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(quiet) {
                Ok(Ok(event)) => {
                    let now = std::time::Instant::now();
                    let quiet_elapsed = last
                        .map(|l| now.duration_since(l) >= quiet)
                        .unwrap_or(false);
                    pending.extend(FileEvent::from_notify(&event.kind, &event.paths));
                    if quiet_elapsed {
                        flush(&mut pending);
                    }
                    last = Some(now);
                }
                Ok(Err(_)) => continue,
                Err(_timeout) => {
                    flush(&mut pending);
                }
            }
        }
    });

    Ok(crate::search::WatchHandle::from_parts(stop, join))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{ModifyKind, RemoveKind, RenameMode};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn remove_maps_to_removed() {
        let ev = FileEvent::from_notify(&EventKind::Remove(RemoveKind::File), &[p("/a/b.md")]);
        assert_eq!(ev, vec![FileEvent::Removed { path: p("/a/b.md") }]);
    }

    #[test]
    fn rename_both_maps_to_renamed() {
        let ev = FileEvent::from_notify(
            &EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[p("/a/old.md"), p("/a/new.md")],
        );
        assert_eq!(
            ev,
            vec![FileEvent::Renamed {
                from: p("/a/old.md"),
                to: p("/a/new.md"),
            }]
        );
    }

    #[test]
    fn unpaired_rename_degrades_to_modified() {
        // A From/To-only rename half cannot be reliably paired → Modified
        // (no false tombstone).
        let ev = FileEvent::from_notify(
            &EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            &[p("/a/old.md")],
        );
        assert_eq!(
            ev,
            vec![FileEvent::Modified {
                path: p("/a/old.md")
            }]
        );
    }

    #[test]
    fn create_and_data_write_map_to_modified() {
        let ev = FileEvent::from_notify(
            &EventKind::Create(notify::event::CreateKind::File),
            &[p("/a/new.md")],
        );
        assert_eq!(
            ev,
            vec![FileEvent::Modified {
                path: p("/a/new.md")
            }]
        );
    }

    #[test]
    fn ghost_params_match_memory_ghost_shape() {
        assert_eq!(
            FileEvent::Removed { path: p("/a") }.to_ghost_params(),
            serde_json::json!({ "kind": "removed", "path": "/a" })
        );
        assert_eq!(
            FileEvent::Renamed {
                from: p("/a"),
                to: p("/b")
            }
            .to_ghost_params(),
            serde_json::json!({ "kind": "renamed", "from": "/a", "to": "/b" })
        );
        assert_eq!(
            FileEvent::Modified { path: p("/a") }.to_ghost_params(),
            serde_json::json!({ "kind": "modified", "path": "/a" })
        );
    }
}
