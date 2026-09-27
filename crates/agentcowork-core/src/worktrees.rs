//! P67 Worktree Fleet Manager — Isolated workspace branches for multi-agent subtasks
//!
//! Subagents (e.g. OpenCode, Cline, Claude Code, internal specialists) run in
//! isolated Git worktree branches under `.everyaios/worktrees/task-<id>`.
//!
//! NOTE (DEC-053): the workspace-local `.everyaios/` blackboard keeps its
//! legacy spelling on purpose — worktrees already on disk carry plans,
//! findings and receipts under it, and renaming the directory would orphan
//! them. The *data home* (`~/.agentcowork`) is renamed with a migration;
//! this *workspace-local* layout is persisted state, not brand identity.
//!
//! Each worktree provides:
//! 1. Branch isolation: `task/<id>` branched from current HEAD.
//! 2. Disk capacity check: verified against `WorktreeCap`.
//! 3. Mutex-serialized execution: routed via `GitOperationQueue`.
//! 4. 3-File Blackboard: `task_plan.md`, `findings.md`, and `receipts/` state persistence.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::git_queue::{GitOperationQueue, GitQueueError};
use crate::worktree_cap::{CapVerdict, WorktreeCap};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeLease {
    pub task_id: String,
    pub branch_name: String,
    pub path: PathBuf,
    pub reserved_gib: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("Worktree capacity exceeded (max {max_gib} GiB, used {used_gib} GiB)")]
    CapacityExceeded { max_gib: u64, used_gib: u64 },
    #[error("Git operation failed: {0}")]
    Git(#[from] GitQueueError),
    #[error("Filesystem I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid task id {0:?}: must be [a-z0-9-_./] without traversal")]
    InvalidTaskId(String),
    #[error("receipt payload too large ({bytes} bytes, cap {cap} bytes)")]
    PayloadTooLarge { bytes: usize, cap: usize },
}

// ---------------------------------------------------------------------------
// P64.4 — 3-file blackboard + P64.6 shadow preflight surface
// (ARCH/17 §§17.4.3/17.6.4, SPEC B3/I15)
// ---------------------------------------------------------------------------

/// P64.4 — blackboard file names inside `<worktree>/.everyaios/`.
pub const BLACKBOARD_PLAN: &str = "task_plan.md";
/// P64.4 — blackboard findings file.
pub const BLACKBOARD_FINDINGS: &str = "findings.md";
/// P64.4 — blackboard receipts directory.
pub const BLACKBOARD_RECEIPTS: &str = "receipts";
/// P64 invariants — tool/preflight/receipt payload ceiling (50 KB cap).
pub const MAX_RECEIPT_BYTES: usize = 50 * 1024;

/// P64.4 — validate a sub-agent task id before it touches the filesystem.
/// Fail closed: empty ids, absolute paths, traversal (`..`), separators
/// outside the `task-<id>` segment, and non-portable chars are refused so a
/// crafted id can never escape `.everyaios/worktrees/`.
pub fn validate_task_id(task_id: &str) -> Result<(), WorktreeError> {
    if task_id.is_empty() || task_id.len() > 128 {
        return Err(WorktreeError::InvalidTaskId(task_id.to_string()));
    }
    if task_id.contains("..") || task_id.contains('\\') || task_id.starts_with('/') {
        return Err(WorktreeError::InvalidTaskId(task_id.to_string()));
    }
    // Allow slug chars plus one `/` level (e.g. `scope/task-1`); anything
    // else (absolute, traversal, control chars) is refused.
    if task_id.starts_with('.') || task_id.ends_with('/') || task_id.contains("//") {
        return Err(WorktreeError::InvalidTaskId(task_id.to_string()));
    }
    let ok = task_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if !ok {
        return Err(WorktreeError::InvalidTaskId(task_id.to_string()));
    }
    Ok(())
}

pub struct WorktreeManager {
    repo_root: PathBuf,
    worktrees_dir: PathBuf,
    cap: WorktreeCap,
    git_queue: GitOperationQueue,
}

impl WorktreeManager {
    pub fn new(repo_root: impl Into<PathBuf>, cap: WorktreeCap) -> Self {
        let root = repo_root.into();
        let worktrees_dir = root.join(".everyaios").join("worktrees");
        let git_queue = GitOperationQueue::new(&root);

        Self {
            repo_root: root,
            worktrees_dir,
            cap,
            git_queue,
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Provision a new isolated worktree for a subagent task.
    pub fn provision_worktree(
        &mut self,
        task_id: &str,
        branch_name: Option<&str>,
        estimated_repo_gib: u64,
    ) -> Result<WorktreeLease, WorktreeError> {
        // P64.4 — fail closed on a crafted task id before any disk effect.
        validate_task_id(task_id)?;
        if let Some(b) = branch_name {
            if b.contains("..") || b.starts_with('-') || b.is_empty() {
                return Err(WorktreeError::InvalidTaskId(b.to_string()));
            }
        }
        // 1. Check disk capacity
        if self.cap.reserve(estimated_repo_gib) == CapVerdict::Refused {
            return Err(WorktreeError::CapacityExceeded {
                max_gib: self.cap.max_gib,
                used_gib: self.cap.used_gib,
            });
        }

        // 2. Prepare paths
        fs::create_dir_all(&self.worktrees_dir)?;
        let worktree_path = self.worktrees_dir.join(format!("task-{}", task_id));
        let branch = branch_name
            .map(|b| b.to_string())
            .unwrap_or_else(|| format!("subtask/{}", task_id));

        // 3. Create git worktree via serialized write queue
        let path_str = worktree_path.to_string_lossy().to_string();
        self.git_queue
            .run_write(&["worktree", "add", "-b", &branch, &path_str], None)?;

        // 4. Initialize 3-file blackboard inside worktree
        self.initialize_blackboard(&worktree_path, task_id)?;

        Ok(WorktreeLease {
            task_id: task_id.to_string(),
            branch_name: branch,
            path: worktree_path,
            reserved_gib: estimated_repo_gib.max(self.cap.min_reserve_gib),
        })
    }

    /// Initialize the 3-file blackboard state sync inside the worktree.
    fn initialize_blackboard(
        &self,
        worktree_path: &Path,
        task_id: &str,
    ) -> Result<(), std::io::Error> {
        let blackboard_dir = worktree_path.join(".everyaios");
        fs::create_dir_all(&blackboard_dir)?;
        fs::create_dir_all(blackboard_dir.join("receipts"))?;

        let initial_plan = format!(
            "# Task Plan: {task_id}\n\n## Phase: Initialization\n- [ ] Task execution started\n"
        );
        fs::write(blackboard_dir.join("task_plan.md"), initial_plan)?;

        let initial_findings = format!("# Findings & Discovered Context: {task_id}\n\n");
        fs::write(blackboard_dir.join("findings.md"), initial_findings)?;

        Ok(())
    }

    /// Revert all uncommitted or committed changes in a worktree branch back to the target ref.
    pub fn undo_worktree(
        &mut self,
        lease: &WorktreeLease,
        target_ref: Option<&str>,
    ) -> Result<(), WorktreeError> {
        let path_str = lease.path.to_string_lossy().to_string();
        let target = target_ref.unwrap_or("HEAD~1");
        self.git_queue
            .run_write(&["-C", &path_str, "reset", "--hard", target], None)?;
        Ok(())
    }

    /// Restore a worktree branch from an existing checkpoint SHA.
    pub fn restore_branch(
        &mut self,
        lease: &WorktreeLease,
        checkpoint_sha: &str,
    ) -> Result<(), WorktreeError> {
        let path_str = lease.path.to_string_lossy().to_string();
        self.git_queue.run_write(
            &[
                "-C",
                &path_str,
                "checkout",
                "-B",
                &lease.branch_name,
                checkpoint_sha,
            ],
            None,
        )?;
        Ok(())
    }

    /// Clean up and remove a worktree lease.
    pub fn release_worktree(&mut self, lease: &WorktreeLease) -> Result<(), WorktreeError> {
        let path_str = lease.path.to_string_lossy().to_string();

        // 1. Remove git worktree
        let _ = self
            .git_queue
            .run_write(&["worktree", "remove", "--force", &path_str], None);

        // 2. Remove directory if anything remains
        if lease.path.exists() {
            let _ = fs::remove_dir_all(&lease.path);
        }

        // 3. Release disk reservation
        self.cap.release(lease.reserved_gib);

        Ok(())
    }

    /// P64.4 — the `.everyaios` blackboard directory inside a lease.
    pub fn blackboard_dir(&self, lease: &WorktreeLease) -> PathBuf {
        lease.path.join(".everyaios")
    }

    /// P64.4 — the three blackboard paths for a lease:
    /// `task_plan.md`, `findings.md`, and the `receipts/` directory.
    pub fn blackboard_paths(&self, lease: &WorktreeLease) -> (PathBuf, PathBuf, PathBuf) {
        let dir = self.blackboard_dir(lease);
        (
            dir.join(BLACKBOARD_PLAN),
            dir.join(BLACKBOARD_FINDINGS),
            dir.join(BLACKBOARD_RECEIPTS),
        )
    }

    /// P64.4 — append a JSON effect/audit receipt to `receipts/<id>.json`.
    /// Enforces the 50 KB cap (fail closed — oversized receipts are refused,
    /// never silently truncated, so the audit trail stays exact).
    pub fn append_receipt(
        &self,
        lease: &WorktreeLease,
        receipt_id: &str,
        payload: &serde_json::Value,
    ) -> Result<PathBuf, WorktreeError> {
        validate_task_id(&lease.task_id)?;
        if receipt_id.is_empty()
            || receipt_id.contains("..")
            || receipt_id.contains('/')
            || receipt_id.contains('\\')
        {
            return Err(WorktreeError::InvalidTaskId(receipt_id.to_string()));
        }
        let bytes = serde_json::to_vec_pretty(payload).map_err(|e| {
            WorktreeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            ))
        })?;
        if bytes.len() > MAX_RECEIPT_BYTES {
            return Err(WorktreeError::PayloadTooLarge {
                bytes: bytes.len(),
                cap: MAX_RECEIPT_BYTES,
            });
        }
        let (_, _, receipts) = self.blackboard_paths(lease);
        fs::create_dir_all(&receipts)?;
        let path = receipts.join(format!("{receipt_id}.json"));
        // Atomic write: temp file + rename so a crash never leaves a torn receipt.
        let tmp = receipts.join(format!(".{receipt_id}.tmp-{}", std::process::id()));
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// P64.4 — read the current `task_plan.md` for a lease.
    pub fn read_task_plan(&self, lease: &WorktreeLease) -> Result<String, WorktreeError> {
        let (plan, _, _) = self.blackboard_paths(lease);
        Ok(fs::read_to_string(plan)?)
    }

    /// P64.6 — the shadow-worktree path for a preflight run. The shadow tree
    /// is a sibling of the task worktree (`task-<id>-shadow`) so typecheck /
    /// tests run against a copy, never the user workspace.
    pub fn shadow_worktree_path(&self, task_id: &str) -> Result<PathBuf, WorktreeError> {
        validate_task_id(task_id)?;
        Ok(self.worktrees_dir.join(format!("task-{task_id}-shadow")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_manager_initialization() {
        let unique = format!(
            "worktree_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let test_dir = std::env::temp_dir().join(unique);
        let cap = WorktreeCap::default();
        let mgr = WorktreeManager::new(&test_dir, cap);

        assert_eq!(mgr.cap.max_gib, 8);
        assert_eq!(
            mgr.worktrees_dir,
            test_dir.join(".everyaios").join("worktrees")
        );
    }

    #[test]
    fn test_worktree_lease_serialization() {
        let lease = WorktreeLease {
            task_id: "test-task-1".to_string(),
            branch_name: "subtask/test-task-1".to_string(),
            path: PathBuf::from("/tmp/wt/task-1"),
            reserved_gib: 2,
        };

        let json = serde_json::to_string(&lease).expect("serialize lease");
        let parsed: WorktreeLease = serde_json::from_str(&json).expect("deserialize lease");
        assert_eq!(parsed.task_id, "test-task-1");
        assert_eq!(parsed.reserved_gib, 2);
    }

    #[test]
    fn p64_task_id_validation_fails_closed() {
        assert!(validate_task_id("task-1").is_ok());
        assert!(validate_task_id("scope/task-1").is_ok());
        for bad in [
            "",
            "../escape",
            "..",
            "/abs",
            "a\\b",
            "a//b",
            ".hidden",
            "has space",
            "semi;colon",
        ] {
            assert!(validate_task_id(bad).is_err(), "must refuse {bad:?}");
        }
    }

    #[test]
    fn p64_blackboard_paths_and_receipt_cap() {
        let dir = std::env::temp_dir().join(format!("wt-bb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = WorktreeManager::new(&dir, WorktreeCap::default());
        let lease = WorktreeLease {
            task_id: "t1".to_string(),
            branch_name: "subtask/t1".to_string(),
            path: dir.join("task-t1"),
            reserved_gib: 1,
        };
        std::fs::create_dir_all(lease.path.join(".everyaios").join("receipts")).unwrap();
        let (plan, findings, receipts) = mgr.blackboard_paths(&lease);
        assert!(plan.ends_with(format!(".everyaios/{}", BLACKBOARD_PLAN)));
        assert!(findings.ends_with(format!(".everyaios/{}", BLACKBOARD_FINDINGS)));
        assert!(receipts.ends_with(format!(".everyaios/{}", BLACKBOARD_RECEIPTS)));
        // Small receipt lands atomically.
        let p = mgr
            .append_receipt(&lease, "r1", &serde_json::json!({"ok": true}))
            .unwrap();
        assert!(p.exists());
        // Oversized receipt is refused, never truncated.
        let big = serde_json::json!({"blob": "x".repeat(MAX_RECEIPT_BYTES + 1)});
        assert!(matches!(
            mgr.append_receipt(&lease, "big", &big),
            Err(WorktreeError::PayloadTooLarge { .. })
        ));
        // Traversal receipt ids are refused.
        assert!(
            mgr.append_receipt(&lease, "../evil", &serde_json::json!({}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p64_shadow_path_is_sibling_and_validated() {
        let dir = std::env::temp_dir().join(format!("wt-sh-{}", std::process::id()));
        let mgr = WorktreeManager::new(&dir, WorktreeCap::default());
        let shadow = mgr.shadow_worktree_path("t1").unwrap();
        assert!(shadow.to_string_lossy().contains("task-t1-shadow"));
        assert!(mgr.shadow_worktree_path("../evil").is_err());
    }
}
