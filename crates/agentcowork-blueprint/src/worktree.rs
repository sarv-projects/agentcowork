//! P20-2 — Superset worktree-per-agent review flow (doc 72 §1 — 🟡 ADAPT).
//!
//! Superset's delta over the existing P17 worktree isolation + H2
//! multiplexing is the **review / open-in-editor loop**: each agent's
//! isolated worktree produces a diff, the orchestrator reviews it, and the
//! human can open the diff in the editor. This module owns the review
//! pipeline over real git worktrees (spawned via the `git` binary — the
//! same zero-libgit2 stance as `agentcowork-core::git_commit`):
//!
//! 1. [`WorktreeSpec`] — one agent's isolated worktree request.
//! 2. [`WorktreeReview`] — the review state machine: collected diff →
//!    per-file verdict (approved / changes-requested / skipped) → merged
//!    decision → open-in-editor payload (the Diff view + Code view seam).
//! 3. [`ensure_worktree`] / [`remove_worktree`] — the real-git lifecycle.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A request to isolate one agent in its own worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSpec {
    /// Agent/session id (also the worktree name: `agent-<id>`).
    pub agent_id: String,
    /// The repo root the worktree is created from.
    pub repo: PathBuf,
    /// The base branch the agent starts from (e.g. `main` or `fleet/…`).
    pub base_branch: String,
}

impl WorktreeSpec {
    /// The conventional worktree path (`.fleet/agent-<id>` under the repo —
    /// matches the P17 `fleet/` branch convention).
    pub fn worktree_path(&self) -> PathBuf {
        self.repo
            .join(".fleet")
            .join(format!("agent-{}", self.agent_id))
    }

    /// The branch the agent's worktree rides on.
    pub fn branch(&self) -> String {
        format!("fleet/{}", self.agent_id)
    }
}

/// Per-file review disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewVerdict {
    /// Not yet dispositioned.
    Unset,
    Approved,
    /// The reviewer wants changes before merge.
    Changed,
    /// Reviewable but intentionally left out of this merge.
    Skipped,
}

/// One changed file in the review set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReview {
    /// Path relative to the worktree root.
    pub path: String,
    /// Lines added / removed (git numstat).
    pub additions: u32,
    pub removals: u32,
    pub verdict: ReviewVerdict,
    /// Optional reviewer note (bounded).
    pub note: Option<String>,
}

impl FileReview {
    fn unset(path: String, additions: u32, removals: u32) -> Self {
        Self {
            path,
            additions,
            removals,
            verdict: ReviewVerdict::Unset,
            note: None,
        }
    }
}

/// The whole-worktree review (Superset's "review + open-in-editor").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeReview {
    pub spec: WorktreeSpec,
    /// Diff against the base branch (collected by [`Self::refresh_diff`]).
    pub changed: Vec<FileReview>,
    pub reviewed: bool,
}

impl WorktreeReview {
    pub fn new(spec: WorktreeSpec) -> Self {
        Self {
            spec,
            changed: Vec::new(),
            reviewed: false,
        }
    }

    /// Collect the diff against the base branch. Real git, best-effort —
    /// a missing binary or broken repo yields an empty diff, never a panic.
    pub fn refresh_diff(&mut self) {
        let wt = self.spec.worktree_path();
        let output = git(&wt, &["diff", "--numstat", &self.spec.base_branch]).unwrap_or_default();
        self.changed = parse_numstat(&output);
    }

    /// Approve every file and mark the review done.
    pub fn approve_all(&mut self) {
        for f in &mut self.changed {
            f.verdict = ReviewVerdict::Approved;
        }
        self.reviewed = true;
    }

    /// Request changes on one file (note bounded to 240 chars).
    pub fn request_changes(&mut self, path: &str, note: &str) -> Result<(), String> {
        let file = self
            .changed
            .iter_mut()
            .find(|f| f.path == path)
            .ok_or_else(|| format!("no such file in review: {path}"))?;
        file.verdict = ReviewVerdict::Changed;
        file.note = Some(note.chars().take(240).collect());
        Ok(())
    }

    /// Dispose every remaining unset file as skipped.
    pub fn skip_rest(&mut self) {
        for f in &mut self.changed {
            if f.verdict == ReviewVerdict::Unset {
                f.verdict = ReviewVerdict::Skipped;
            }
        }
        self.reviewed = true;
    }

    /// Mergeable as-is? (nothing pending Changes-requested or Unset)
    pub fn mergeable(&self) -> bool {
        !self
            .changed
            .iter()
            .any(|f| f.verdict == ReviewVerdict::Changed || f.verdict == ReviewVerdict::Unset)
    }

    /// The open-in-editor payload (Diff view + Code view seam).
    pub fn open_in_editor(&self) -> EditorOpen {
        EditorOpen {
            root: self.spec.worktree_path(),
            files: self.changed.iter().map(|f| f.path.clone()).collect(),
        }
    }
}

/// The payload for the "open in editor" action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorOpen {
    pub root: PathBuf,
    pub files: Vec<String>,
}

/// Run `git <args>` in `cwd`; stdout on success, Err(stderr/io) otherwise.
fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(format!(
            "git {} → {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// Parse `git diff --numstat` output (tab-separated `adds removes path`).
fn parse_numstat(output: &str) -> Vec<FileReview> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let adds = parts.next()?;
            let removes = parts.next()?;
            let path = parts.next()?;
            if path.is_empty() {
                return None;
            }
            Some(FileReview::unset(
                path.to_string(),
                adds.parse().unwrap_or(0),
                removes.parse().unwrap_or(0),
            ))
        })
        .collect()
}

/// Create the worktree for a spec (idempotent — an existing one is ok).
pub fn ensure_worktree(spec: &WorktreeSpec) -> Result<PathBuf, String> {
    let wt = spec.worktree_path();
    if git(&wt, &["rev-parse", "--git-dir"]).is_ok() {
        return Ok(wt);
    }
    // Ensure the fleet branch exists (start = base branch), then branch off.
    let _ = git(
        &spec.repo,
        &["branch", &spec.branch(), spec.base_branch.as_str()],
    );
    git(
        &spec.repo,
        &["worktree", "add", wt.to_str().unwrap_or(""), &spec.branch()],
    )?;
    Ok(wt)
}

/// Remove the worktree and its branch once the run is reviewed/merged.
pub fn remove_worktree(spec: &WorktreeSpec) -> Result<(), String> {
    let wt = spec.worktree_path();
    let _ = git(
        &spec.repo,
        &["worktree", "remove", "--force", wt.to_str().unwrap_or("")],
    );
    let _ = git(&spec.repo, &["branch", "-D", &spec.branch()]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn worktree_path_and_branch_convention() {
        let spec = WorktreeSpec {
            agent_id: "alice".into(),
            repo: PathBuf::from("/repo"),
            base_branch: "main".into(),
        };
        assert_eq!(
            spec.worktree_path(),
            PathBuf::from("/repo/.fleet/agent-alice")
        );
        assert_eq!(spec.branch(), "fleet/alice");
    }

    #[test]
    fn numstat_parsing() {
        let out = "12\t3\tcrates/foo.rs\n1\t1\tREADME.md\n";
        let files = parse_numstat(out);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "crates/foo.rs");
        assert_eq!(files[0].additions, 12);
        assert_eq!(files[1].removals, 1);
        assert_eq!(files[0].verdict, ReviewVerdict::Unset);
    }

    #[test]
    fn review_lifecycle() {
        let mut r = WorktreeReview::new(WorktreeSpec {
            agent_id: "alice".into(),
            repo: PathBuf::from("/repo"),
            base_branch: "main".into(),
        });
        r.changed = vec![
            FileReview::unset("a.rs".into(), 2, 1),
            FileReview::unset("b.rs".into(), 9, 0),
        ];
        assert!(!r.mergeable()); // unset blocks merge
        r.request_changes("a.rs", "rename this").unwrap();
        assert_eq!(r.changed[0].note.as_deref(), Some("rename this"));
        assert!(r.request_changes("missing.rs", "x").is_err());
        r.skip_rest();
        assert!(r.reviewed);
        assert_eq!(r.changed[1].verdict, ReviewVerdict::Skipped);
        assert!(!r.mergeable());
        r.changed[0].verdict = ReviewVerdict::Approved;
        assert!(r.mergeable());
        let open = r.open_in_editor();
        assert_eq!(open.files, vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn approve_all_merges() {
        let mut r = WorktreeReview::new(WorktreeSpec {
            agent_id: "bob".into(),
            repo: PathBuf::from("/repo"),
            base_branch: "main".into(),
        });
        r.changed = vec![FileReview::unset("x.ts".into(), 5, 0)];
        r.approve_all();
        assert!(r.mergeable() && r.reviewed);
    }

    #[test]
    fn real_git_end_to_end() {
        let root = temp_repo("worktree-test");
        let init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !init {
            let _ = std::fs::remove_dir_all(&root);
            eprintln!("git unavailable — skipping live worktree test");
            return;
        }
        std::process::Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(&root)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&root)
            .status()
            .unwrap();
        std::fs::write(root.join("one.txt"), "one\n").unwrap();
        run_git(&root, &["add", "one.txt"]);
        run_git(&root, &["commit", "-qm", "base"]);

        let spec = WorktreeSpec {
            agent_id: "alice".into(),
            repo: root.clone(),
            base_branch: "main".into(),
        };
        let wt = ensure_worktree(&spec).unwrap();
        assert!(wt.is_dir());
        // The agent edits in its own worktree.
        std::fs::write(wt.join("two.txt"), "two\n").unwrap();
        run_git(&wt, &["add", "two.txt"]);
        run_git(&wt, &["commit", "-qm", "agent work"]);

        let mut review = WorktreeReview::new(spec.clone());
        review.refresh_diff();
        assert!(review.changed.iter().any(|f| f.path == "two.txt"));
        assert_eq!(review.changed.iter().map(|f| f.additions).sum::<u32>(), 1);
        let open = review.open_in_editor();
        assert!(open.root.join("two.txt").is_file());

        remove_worktree(&spec).unwrap();
        assert!(!wt.exists());
        // remove_worktree is idempotent-safe (best-effort)
        let _ = remove_worktree(&spec);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let s = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .unwrap();
        assert!(s.success(), "git {args:?} failed in {cwd:?}");
    }
}
