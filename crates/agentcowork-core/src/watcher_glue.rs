//! Storage→core watcher glue (P5.4 follow-on). The `notify` watcher lives in
//! `agentcowork-storage`, which classifies raw `notify` events (kind + paths)
//! into [`FileEvent`] batches; this module is the final hop onto the core
//! side: `FileEvent → agentcowork_memory::ghost::FsEvent` (delete → tombstone,
//! rename → re-path, modify → no-op) plus the ai!-marker auto-submit path
//! filter (`crate::ai_marker`). No `notify` dependency here — same discipline
//! as the memory crate's contract.

use crate::ai_marker;
use agentcowork_memory::ghost::FsEvent;
use agentcowork_storage::FileEvent;
use std::path::{Path, PathBuf};

/// Map one classified storage event onto the ghost index.
pub fn to_fs_event(event: &FileEvent) -> FsEvent {
    match event {
        FileEvent::Removed { path } => FsEvent::Removed(path.to_string_lossy().into_owned()),
        FileEvent::Renamed { from, to } => FsEvent::Renamed {
            from: from.to_string_lossy().into_owned(),
            to: to.to_string_lossy().into_owned(),
        },
        FileEvent::Modified { path } => FsEvent::Modified(path.to_string_lossy().into_owned()),
    }
}

/// The full bridge: owns the ghost handler (the `MemoryService` seam).
pub struct WatcherBridge<F> {
    /// Applies one ghost event, returns affected refs.
    on_ghost: F,
}

impl<F> WatcherBridge<F>
where
    F: FnMut(&FsEvent) -> usize,
{
    pub fn new(on_ghost: F) -> Self {
        Self { on_ghost }
    }

    /// Feed one classified batch. Returns the total affected refs.
    pub fn on_batch(&mut self, batch: &[FileEvent]) -> usize {
        batch.iter().map(|e| (self.on_ghost)(&to_fs_event(e))).sum()
    }
}

/// ai!-marker auto-submit filter: which paths in a batch carry `// ai!` /
/// `# ai!` markers and should be re-scanned by the ai!-watcher.
pub fn marker_paths(batch: &[FileEvent]) -> Vec<PathBuf> {
    batch
        .iter()
        .filter_map(|e| match e {
            FileEvent::Removed { .. } => None,
            FileEvent::Modified { path } | FileEvent::Renamed { to: path, .. } => {
                let p = path.as_path();
                (p.is_file()
                    && is_source(p)
                    && std::fs::read_to_string(p)
                        .map(|s| {
                            let lines: Vec<&str> = s.lines().collect();
                            !ai_marker::scan_markers(&lines, 0, 1).is_empty()
                        })
                        .unwrap_or(false))
                .then(|| p.to_path_buf())
            }
        })
        .collect()
}

/// Convenience: start the storage `watch_events` loop and bridge classified
/// batches into the core. `on_ghost` owns the `MemoryService`; `on_markers`
/// receives ai!-marker paths for auto-submit. Returns the `WatchHandle`.
pub fn start_watcher(
    roots: Vec<PathBuf>,
    quiet: std::time::Duration,
    mut on_ghost: impl FnMut(&FsEvent) -> usize + Send + 'static,
    mut on_markers: impl FnMut(Vec<PathBuf>) + Send + 'static,
) -> Result<agentcowork_storage::WatchHandle, String> {
    agentcowork_storage::watch_events(roots, quiet, move |batch| {
        let mut bridge = WatcherBridge::new(|e| on_ghost(e));
        let _affected = bridge.on_batch(&batch);
        let markers = marker_paths(&batch);
        if !markers.is_empty() {
            on_markers(markers);
        }
    })
    .map_err(|e| e.to_string())
}

/// True when the path is a plausible source file for ai! markers.
fn is_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "md")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removed(p: &str) -> FileEvent {
        FileEvent::Removed {
            path: PathBuf::from(p),
        }
    }
    fn renamed(from: &str, to: &str) -> FileEvent {
        FileEvent::Renamed {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }
    }
    fn modified(p: &str) -> FileEvent {
        FileEvent::Modified {
            path: PathBuf::from(p),
        }
    }

    #[test]
    fn mapping_preserves_kind() {
        assert_eq!(
            to_fs_event(&removed("/tmp/a.rs")),
            FsEvent::Removed("/tmp/a.rs".into())
        );
        assert_eq!(
            to_fs_event(&renamed("/tmp/old.rs", "/tmp/new.rs")),
            FsEvent::Renamed {
                from: "/tmp/old.rs".into(),
                to: "/tmp/new.rs".into()
            }
        );
        assert_eq!(
            to_fs_event(&modified("/tmp/a.rs")),
            FsEvent::Modified("/tmp/a.rs".into())
        );
    }

    #[test]
    fn bridge_applies_events_and_counts() {
        let mut count = 0usize;
        let mut bridge = WatcherBridge::new(|e| {
            let _ = e;
            count += 1;
            1
        });
        let affected = bridge.on_batch(&[removed("/tmp/a.rs"), modified("/tmp/b.rs")]);
        assert_eq!(affected, 2);
        assert_eq!(count, 2);
    }

    #[test]
    fn marker_filter_picks_marked_files_only() {
        let root = std::env::temp_dir().join(format!("wg-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let marked = root.join("work.rs");
        let plain = root.join("plain.rs");
        std::fs::write(&marked, "// ai! run tests\nfn t() {}").unwrap();
        std::fs::write(&plain, "fn p() {}").unwrap();

        let picked = marker_paths(&[
            modified(marked.to_str().unwrap()),
            modified(plain.to_str().unwrap()),
        ]);
        assert_eq!(picked, vec![marked]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn marker_filter_skips_removals() {
        assert!(marker_paths(&[removed("/tmp/x.rs")]).is_empty());
    }

    #[test]
    fn is_source_extensions() {
        assert!(is_source(Path::new("a.rs")));
        assert!(is_source(Path::new("b.tsx")));
        assert!(!is_source(Path::new("c.png")));
    }
}
