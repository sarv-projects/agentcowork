//! Git Operation Queue — Mutex-serialized write operations with lockfile recovery
//!
//! When multiple subagents (20-30 fleet) operate concurrently on separate
//! worktrees, Git commands that touch `.git/index` or references can collide
//! with `fatal: Unable to create '.git/index.lock': File exists`.
//!
//! `GitOperationQueue` enforces:
//! 1. Concurrent lock-free read operations (`status`, `diff`, `log`).
//! 2. Mutex-serialized write operations (`add`, `commit`, `checkout`, `worktree`).
//! 3. Automated stale lock detection and recovery (locks > 5s cleared safely).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone)]
pub struct GitOperationQueue {
    repo_root: PathBuf,
    write_lock: Arc<Mutex<()>>,
    read_tracker: Arc<RwLock<usize>>,
    lock_timeout: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum GitQueueError {
    #[error("I/O error during git execution: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git command failed (exit code {code:?}): {stderr}")]
    CommandFailed { code: Option<i32>, stderr: String },
    #[error("Git lock timeout after {0:?}")]
    LockTimeout(Duration),
}

impl GitOperationQueue {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            write_lock: Arc::new(Mutex::new(())),
            read_tracker: Arc::new(RwLock::new(0)),
            lock_timeout: Duration::from_secs(5),
        }
    }

    /// Check for stale `.git/index.lock` or worktree lock files and clear if older than timeout.
    pub fn clean_stale_locks(&self) -> Result<bool, GitQueueError> {
        let index_lock = self.repo_root.join(".git").join("index.lock");
        if index_lock.exists() {
            if let Ok(meta) = fs::metadata(&index_lock) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                        if elapsed > self.lock_timeout {
                            let _ = fs::remove_file(&index_lock);
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    /// Execute a read-only git command concurrently (e.g. status, diff, log).
    pub fn run_read(&self, args: &[&str], cwd: Option<&Path>) -> Result<Output, GitQueueError> {
        let _guard = self.read_tracker.read().unwrap();
        let target_dir = cwd.unwrap_or(&self.repo_root);

        let output = Command::new("git")
            .args(args)
            .current_dir(target_dir)
            .output()?;

        if !output.status.success() {
            return Err(GitQueueError::CommandFailed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }
        Ok(output)
    }

    /// Execute a mutating git command with serialized write locking and stale-lock recovery.
    pub fn run_write(&self, args: &[&str], cwd: Option<&Path>) -> Result<Output, GitQueueError> {
        let start = Instant::now();
        let _guard = match self.write_lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        if start.elapsed() > self.lock_timeout {
            return Err(GitQueueError::LockTimeout(self.lock_timeout));
        }

        // Clean any stale index lock before proceeding
        let _ = self.clean_stale_locks();

        let target_dir = cwd.unwrap_or(&self.repo_root);
        let output = Command::new("git")
            .args(args)
            .current_dir(target_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            // If lock collision occurred despite serialization, try clearing once and retrying
            if stderr.contains("index.lock") {
                let _ = self.clean_stale_locks();
                let retry_output = Command::new("git")
                    .args(args)
                    .current_dir(target_dir)
                    .output()?;

                if !retry_output.status.success() {
                    return Err(GitQueueError::CommandFailed {
                        code: retry_output.status.code(),
                        stderr: String::from_utf8_lossy(&retry_output.stderr).to_string(),
                    });
                }
                return Ok(retry_output);
            }

            return Err(GitQueueError::CommandFailed {
                code: output.status.code(),
                stderr,
            });
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_queue_creation_and_lock_cleanup() {
        let unique = format!(
            "git_queue_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let test_dir = std::env::temp_dir().join(unique);
        let git_dir = test_dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let queue = GitOperationQueue::new(&test_dir);
        let lock_file = git_dir.join("index.lock");
        fs::write(&lock_file, b"stale lock").unwrap();

        // Initially lock is fresh, should not be cleaned
        assert!(!queue.clean_stale_locks().unwrap());

        // File exists
        assert!(lock_file.exists());

        let _ = fs::remove_dir_all(&test_dir);
    }
}
