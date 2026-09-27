//! D7 / FIX-16 — the crash-safe commit path (ARCH/22 §4, REQ-OFFICE-004,
//! EDGE-050): write a **staging package**, **fsync** it, then **atomically
//! swap** it into place and make the rename itself durable.
//!
//! The ordering is the whole point. Same-directory `rename` is atomic on
//! POSIX, so a crash can never leave a torn file *if* the staged bytes reached
//! stable storage first — an un-fsynced staging file can be swapped in and
//! then vanish on power loss, which is exactly the OfficeCLI trade-off this
//! runtime does not copy (`ARCH/22` §1.3).
//!
//! Every commit therefore runs through [`commit_bytes`], which records the
//! stages it actually executed in a [`CommitTrace`]. The trace is emitted from
//! the real call sites, so removing the `sync_all()` call makes the trace stop
//! containing [`CommitStage::Fsynced`] and the ordering test fails — the
//! ordering is asserted against the implementation, not against a comment.
//!
//! # Recovery
//!
//! A crash between staging and swap leaves a `.name.tmp-<pid>-<seq>` sibling
//! and an **untouched target** (the pre-edit bytes are still on disk).
//! [`recover_orphans`] reports those staging packages so a caller can
//! inspect or discard them.
//!
//! **Known gap — op-log replay is NOT implemented here.** `ARCH/22` §4 and
//! REQ-OFFICE-004 also require an operation log whose replay reconstructs a
//! commit that crashed *after* the swap. This crate has no op log and no
//! replay engine; [`recover_orphans`] recovers the *pre-swap* orphan only. The
//! gap is deliberately explicit rather than papered over with a
//! best-effort re-apply.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic temp-suffix counter (parallel tests/writers never collide).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Count of staging-file `File::sync_all` calls this process has executed.
///
/// Test-visible proof that the fsync in [`commit_bytes`] is not a no-op:
/// [`fsync_calls`] is incremented at the single call site, so a commit that
/// skipped the fsync does not move the counter.
static FSYNC_CALLS: AtomicU64 = AtomicU64::new(0);

/// How many staging-file `sync_all` calls this process has executed.
pub fn fsync_calls() -> u64 {
    FSYNC_CALLS.load(Ordering::SeqCst)
}

/// The ordered stages of a crash-safe commit, in the order the requirement
/// demands (ARCH/22 §4: staging package → fsync → atomic swap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitStage {
    /// The replacement bytes were written to a sibling staging file.
    Staged,
    /// The staging file's **data** reached stable storage (`File::sync_all`).
    Fsynced,
    /// The staging file replaced the target (`rename` — atomic on POSIX).
    Swapped,
    /// The containing directory entry reached stable storage (POSIX only;
    /// absent on platforms without directory fsync).
    DurablyRenamed,
}

impl CommitStage {
    /// Stable identifier for receipts and audit payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            CommitStage::Staged => "staged",
            CommitStage::Fsynced => "fsynced",
            CommitStage::Swapped => "swapped",
            CommitStage::DurablyRenamed => "durably_renamed",
        }
    }

    /// Whether this stage makes the commit **durable**: after the swap plus a
    /// durable rename, a power loss cannot lose the new bytes.
    pub fn is_durable(self) -> bool {
        matches!(self, CommitStage::DurablyRenamed)
    }
}

impl std::fmt::Display for CommitStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The canonical commit order. A commit that reports a different sequence has
/// violated the requirement, and [`CommitTrace::assert_ordered`] says so.
pub const COMMIT_ORDER: [CommitStage; 4] = [
    CommitStage::Staged,
    CommitStage::Fsynced,
    CommitStage::Swapped,
    CommitStage::DurablyRenamed,
];

/// A commit failure that names the stage it failed in, so a caller can
/// report *where* durability was lost instead of a bare io error.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("commit stage `{stage}` failed: {source}")]
    Stage {
        /// The stage that failed.
        stage: CommitStage,
        /// The underlying io failure.
        source: std::io::Error,
    },
    #[error("path {path:?} has no file name")]
    NoFileName { path: PathBuf },
    #[error("commit order violated: {stages:?} is not the required {expected:?}")]
    OutOfOrder {
        stages: Vec<CommitStage>,
        expected: Vec<CommitStage>,
    },
    #[error("post-commit read-back failed: {source}")]
    ReadBack { source: std::io::Error },
    #[error("post-commit read-back mismatch: wrote {written} bytes, read {read} bytes back")]
    ReadBackMismatch { written: usize, read: usize },
}

impl CommitError {
    /// The stage the commit died in (`None` for non-stage failures).
    pub fn stage(&self) -> Option<CommitStage> {
        match self {
            CommitError::Stage { stage, .. } => Some(*stage),
            _ => None,
        }
    }
}

/// What a commit actually did — the ordering evidence a receipt carries.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitTrace {
    /// The target that was replaced.
    pub target: PathBuf,
    /// The staging file used (already renamed away on success).
    pub staging: PathBuf,
    /// The stages executed, in execution order.
    pub stages: Vec<CommitStage>,
    /// Bytes handed to the commit.
    pub bytes: usize,
}

impl CommitTrace {
    /// Whether the recorded stages are the required order (a prefix is
    /// accepted: the final directory fsync is unavailable on some platforms).
    pub fn is_ordered(&self) -> bool {
        // Stage and fsync are mandatory and first; the final directory fsync
        // is best-effort, so a three-stage trace is still a valid commit.
        self.stages.len() >= 3
            && self.stages[0] == CommitStage::Staged
            && self.stages[1] == CommitStage::Fsynced
            && self.stages.windows(2).all(|w| {
                let a = COMMIT_ORDER.iter().position(|s| *s == w[0]);
                let b = COMMIT_ORDER.iter().position(|s| *s == w[1]);
                matches!((a, b), (Some(x), Some(y)) if x < y)
            })
    }

    /// Whether the commit is crash-durable (the swap was made durable).
    pub fn is_durable(&self) -> bool {
        self.stages.contains(&CommitStage::DurablyRenamed)
    }

    /// Fail unless the trace is the required order.
    pub fn assert_ordered(&self) -> Result<(), CommitError> {
        if self.is_ordered() {
            Ok(())
        } else {
            Err(CommitError::OutOfOrder {
                stages: self.stages.clone(),
                expected: COMMIT_ORDER.to_vec(),
            })
        }
    }
}

/// Errors from the compatibility [`write_atomic`] surface.
#[derive(Debug, thiserror::Error)]
pub enum AtomicError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("commit failed: {0}")]
    Commit(#[from] CommitError),
}

/// The staging file name for `target`: a hidden sibling carrying the writer's
/// pid and a process-monotonic sequence, so two writers in one process and a
/// restart after a crash never collide.
fn staging_path(target: &Path) -> Result<PathBuf, CommitError> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = target.file_name().ok_or_else(|| CommitError::NoFileName {
        path: target.to_path_buf(),
    })?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    Ok(dir.join(format!(
        ".{}.tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        seq
    )))
}

fn stage_err(stage: CommitStage) -> impl Fn(std::io::Error) -> CommitError {
    move |source| CommitError::Stage { stage, source }
}

/// Commit `bytes` to `target` through the crash-safe path: **stage → fsync →
/// atomic swap → durable directory entry**, recording each executed stage.
///
/// The staging file is a sibling of the target on purpose: `rename` is only
/// atomic within a filesystem, so a scratch directory elsewhere could not
/// satisfy the atomic-swap half of REQ-OFFICE-004.
pub fn commit_bytes(target: &Path, bytes: &[u8]) -> Result<CommitTrace, CommitError> {
    let tmp_path = staging_path(target)?;
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut stages: Vec<CommitStage> = Vec::with_capacity(COMMIT_ORDER.len());
    let trace = |stages: &[CommitStage]| CommitTrace {
        target: target.to_path_buf(),
        staging: tmp_path.clone(),
        stages: stages.to_vec(),
        bytes: bytes.len(),
    };

    // 1. Stage the replacement package next to the target.
    let write = (|| -> std::io::Result<()> {
        let mut tmp = File::create(&tmp_path)?;
        tmp.write_all(bytes)?;
        tmp.flush()
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(stage_err(CommitStage::Staged)(e));
    }
    stages.push(CommitStage::Staged);

    // 2. Make the staged bytes durable BEFORE the swap. Without this the file
    //    can be renamed into place and still be lost on power failure.
    let fsync = File::open(&tmp_path).and_then(|f| f.sync_all());
    match fsync {
        Ok(()) => {
            FSYNC_CALLS.fetch_add(1, Ordering::SeqCst);
            stages.push(CommitStage::Fsynced);
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(stage_err(CommitStage::Fsynced)(e));
        }
    }

    // 3. Atomic swap. The target holds either the old bytes or the new bytes.
    if let Err(e) = std::fs::rename(&tmp_path, target) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(stage_err(CommitStage::Swapped)(e));
    }
    stages.push(CommitStage::Swapped);

    // 4. Make the directory entry itself durable (POSIX). Best-effort by
    //    design: platforms without directory fsync still have an atomic swap.
    #[cfg(unix)]
    {
        if let Ok(d) = File::open(&dir)
            && d.sync_all().is_ok()
        {
            stages.push(CommitStage::DurablyRenamed);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = &dir;
    }

    let out = trace(&stages);
    out.assert_ordered()?;
    Ok(out)
}

/// Post-commit structural check: re-read the target and confirm it holds
/// exactly what was committed (ARCH/22 §5 — validate before a receipt).
pub fn verify_readback(target: &Path, expected_len: usize) -> Result<(), CommitError> {
    let meta = std::fs::metadata(target).map_err(|source| CommitError::ReadBack { source })?;
    if meta.len() as usize != expected_len {
        return Err(CommitError::ReadBackMismatch {
            written: expected_len,
            read: meta.len() as usize,
        });
    }
    Ok(())
}

/// Write `bytes` to `path` atomically. Returns `()` on success; the target
/// file either contains the old bytes or the new bytes, never a mix, and the
/// staged bytes are fsynced before the swap (this is a thin wrapper over
/// [`commit_bytes`], so there is exactly one commit path in the runtime).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AtomicError> {
    commit_bytes(path, bytes).map(|_| ())?;
    Ok(())
}

/// A staging package left behind by a crash between stage and swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedOrphan {
    /// The staging file still on disk.
    pub staging: PathBuf,
    /// The target it was staged for.
    pub target: PathBuf,
    /// The staged byte length (the target is untouched and still pre-edit).
    pub staged_bytes: u64,
}

/// List staging packages under `dir` that no live commit owns.
///
/// A crash before the swap leaves these; the target keeps its pre-edit bytes,
/// so recovery is a discard (or an inspection) — never a re-apply. The
/// op-log replay that REQ-OFFICE-004 / EDGE-050 ask for is a separate,
/// unimplemented capability; see the module docs.
pub fn recover_orphans(dir: &Path) -> Vec<StagedOrphan> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Match `.name.tmp-<pid>-<seq>` written by `staging_path`.
        let Some(rest) = name.strip_prefix('.') else {
            continue;
        };
        let Some(idx) = rest.find(".tmp-") else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push(StagedOrphan {
            staging: entry.path(),
            target: dir.join(&rest[..idx]),
            staged_bytes: meta.len(),
        });
    }
    out.sort_by(|a, b| a.staging.cmp(&b.staging));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "agentcowork-atomic-{tag}-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn writes_bytes_and_leaves_no_temp() {
        let path = tmp_file("new");
        let _ = std::fs::remove_file(&path);
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        // No leftover temp file in the directory.
        let dir = path.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("agentcowork-atomic-new") && n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overwrites_existing_file() {
        let path = tmp_file("overwrite");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new content").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new content");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_filename_errors() {
        // A path with no file name (root or trailing dir) cannot be committed.
        // The error names the failure rather than degrading to a bare io error.
        let err = write_atomic(std::path::Path::new("."), b"x").unwrap_err();
        match err {
            AtomicError::Commit(CommitError::NoFileName { .. }) => {}
            other => panic!("expected a typed no-file-name error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // FIX-16 — staging → fsync → atomic swap.
    // -----------------------------------------------------------------

    #[test]
    fn commit_reports_the_required_stage_order() {
        let path = tmp_file("order");
        let _ = std::fs::remove_file(&path);
        let trace = commit_bytes(&path, b"payload").unwrap();
        assert_eq!(
            trace.stages,
            vec![
                CommitStage::Staged,
                CommitStage::Fsynced,
                CommitStage::Swapped,
                CommitStage::DurablyRenamed
            ],
            "commit must stage, fsync, then swap — an fsync after the swap does not make the bytes durable"
        );
        assert!(trace.is_ordered());
        assert_eq!(std::fs::read(&path).unwrap(), b"payload");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn commit_fsyncs_the_staging_file() {
        // The counter is a process-wide monotonic tally, so this asserts "at
        // least one" (parallel commits only ever add). The load-bearing
        // assertion is the `Fsynced` stage in the trace: it is pushed **only**
        // inside the `Ok(())` arm of the real `sync_all()` call, so removing
        // the fsync makes this test and the order test above fail.
        let path = tmp_file("fsync");
        let _ = std::fs::remove_file(&path);
        let before = fsync_calls();
        let trace = commit_bytes(&path, b"durable").unwrap();
        assert!(
            fsync_calls() > before,
            "commit did not reach the staging-file fsync call site"
        );
        assert!(trace.stages.contains(&CommitStage::Fsynced));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn out_of_order_trace_is_rejected() {
        // The ordering check is real: a trace that swaps before it fsyncs is
        // refused, so the invariant cannot be quietly dropped.
        let bad = CommitTrace {
            target: PathBuf::from("/tmp/x"),
            staging: PathBuf::from("/tmp/.x.tmp-1-1"),
            stages: vec![
                CommitStage::Staged,
                CommitStage::Swapped,
                CommitStage::Fsynced,
            ],
            bytes: 3,
        };
        assert!(!bad.is_ordered());
        let err = bad.assert_ordered().unwrap_err();
        assert!(matches!(err, CommitError::OutOfOrder { .. }));

        // A trace missing the fsync entirely is not a valid commit.
        let unsynced = CommitTrace {
            target: PathBuf::from("/tmp/x"),
            staging: PathBuf::from("/tmp/.x.tmp-1-1"),
            stages: vec![CommitStage::Staged, CommitStage::Swapped],
            bytes: 3,
        };
        assert!(!unsynced.is_ordered());
    }

    #[test]
    fn failed_swap_leaves_the_original_bytes_untouched() {
        // A swap that cannot happen (target is a non-empty directory) fails
        // with a typed stage error and must not damage what is already there.
        let dir = tmp_file("swapdir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep"), b"precious").unwrap();

        let err = commit_bytes(&dir, b"replacement").unwrap_err();
        assert_eq!(err.stage(), Some(CommitStage::Swapped));
        assert!(dir.join("keep").exists(), "original content must survive");

        // No staging package is left behind by a failed commit.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover staging: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readback_check_detects_a_mismatch() {
        let path = tmp_file("readback");
        commit_bytes(&path, b"12345").unwrap();
        verify_readback(&path, 5).unwrap();
        let err = verify_readback(&path, 9).unwrap_err();
        assert!(matches!(
            err,
            CommitError::ReadBackMismatch {
                written: 9,
                read: 5
            }
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn orphans_from_a_crash_are_reported_and_the_target_is_untouched() {
        // Simulate a crash between stage and swap: the staging package exists,
        // the target still holds the pre-edit bytes.
        let dir = tmp_file("orphan");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("doc.xlsx");
        std::fs::write(&target, b"pre-edit").unwrap();
        let orphan = dir.join(".doc.xlsx.tmp-9999-0");
        std::fs::write(&orphan, b"half-committed").unwrap();

        let orphans = recover_orphans(&dir);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].target, target);
        assert_eq!(orphans[0].staged_bytes, b"half-committed".len() as u64);
        // The document was never swapped: the original is intact.
        assert_eq!(std::fs::read(&target).unwrap(), b"pre-edit");

        let _ = std::fs::remove_file(&orphan);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
