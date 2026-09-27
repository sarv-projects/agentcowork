//! P36 — I8/K2 atomic commit: one verified surgical edit = one git commit.
//!
//! The commit is only created after a caller-supplied verification gate
//! passes (the "verified surgical edit" contract — never accept the edit's
//! own "finished" claim). Only the edited paths are staged; nothing else in
//! the working tree is touched. Shells out to `git` (the repo already does
//! this in `git_cmds.ts`); no libgit2 dependency.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub files: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("git unavailable: {0}")]
    GitUnavailable(String),
    #[error("verification gate failed: {0}")]
    VerificationFailed(String),
    #[error("git command failed ({cmd}): {stderr}")]
    CommandFailed { cmd: String, stderr: String },
}

fn run_git(repo: &Path, args: &[&str]) -> Result<String, CommitError> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .map_err(|e| CommitError::GitUnavailable(e.to_string()))?;
    if !out.status.success() {
        return Err(CommitError::CommandFailed {
            cmd: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Commit exactly the verified edited paths, nothing else in the tree.
///
/// Flow: verification gate → `git add -- <:(literal)paths>` → `git commit`.
/// A failed gate refuses everything (no staging, no commit). Duplicate sha
/// before/after is detected and surfaced as a failure (nothing landed).
pub fn commit_verified_edit<F>(
    repo: &Path,
    files: &[&str],
    message: &str,
    verify: F,
) -> Result<CommitInfo, CommitError>
where
    F: FnOnce() -> bool,
{
    if files.is_empty() {
        return Err(CommitError::VerificationFailed(
            "no edited files listed".into(),
        ));
    }
    if !verify() {
        return Err(CommitError::VerificationFailed(
            "edit verification did not pass; nothing was committed".into(),
        ));
    }

    let before = run_git(repo, &["rev-parse", "--short", "HEAD"])?;

    // Stage exactly these paths as literals — never patterns. The literal
    // pathspecs are owned Strings so the temp-drops live long enough.
    let literals: Vec<String> = files.iter().map(|f| format!(":(literal){f}")).collect();
    let mut add: Vec<&str> = vec!["add", "--"];
    add.extend(literals.iter().map(String::as_str));
    run_git(repo, &add)?;

    // `git diff --cached --quiet` (--exit-code): exit 0 = nothing staged,
    // exit 1 = staged changes exist. Refuse the no-op case; a staged diff
    // shows up as CommandFailed here and is exactly what we want to commit.
    let staged = run_git(repo, &["diff", "--cached", "--quiet"]);
    match staged {
        Ok(_) => {
            return Err(CommitError::VerificationFailed(
                "nothing to commit: the verified paths are unchanged".into(),
            ));
        }
        Err(CommitError::CommandFailed { .. }) => {} // staged diff present — proceed
        Err(e) => return Err(e),
    }

    // `git commit` prints its summary to stdout on success — only the HEAD
    // check below tells us whether a commit actually landed.
    run_git(repo, &["commit", "-m", message])?;

    let after = run_git(repo, &["rev-parse", "--short", "HEAD"])?;
    if after == before {
        return Err(CommitError::VerificationFailed(
            "nothing committed (no diff on the verified paths)".into(),
        ));
    }

    Ok(CommitInfo {
        sha: after,
        message: message.to_string(),
        files: files.iter().map(|f| (*f).to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-k2-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-b", "main"]).unwrap();
        run_git(&dir, &["config", "user.email", "dev@agentcowork.local"]).unwrap();
        run_git(&dir, &["config", "user.name", "AgentCowork Test"]).unwrap();
        dir
    }

    #[test]
    fn verified_edit_lands_one_commit() {
        if !git_available() {
            eprintln!("git missing; skipping");
            return;
        }
        let dir = repo("ok");
        let file = dir.join("doc.md");
        fs::write(&file, "before").unwrap();
        run_git(&dir, &["add", "--", "doc.md"]).unwrap();
        run_git(&dir, &["commit", "-m", "base"]).unwrap();

        fs::write(&file, "after").unwrap();
        let mut verified = false;
        let info = commit_verified_edit(&dir, &["doc.md"], "surgical: doc.md", || {
            verified = true;
            true
        })
        .expect("commit ok");
        assert!(verified);
        assert!(!info.sha.is_empty());
        let content = run_git(&dir, &["show", "HEAD:doc.md"]).unwrap();
        assert_eq!(content, "after");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unverified_edit_is_refused() {
        if !git_available() {
            return;
        }
        let dir = repo("unv");
        let file = dir.join("doc.md");
        fs::write(&file, "before").unwrap();
        run_git(&dir, &["add", "--", "doc.md"]).unwrap();
        run_git(&dir, &["commit", "-m", "base"]).unwrap();
        fs::write(&file, "after").unwrap();
        let res = commit_verified_edit(&dir, &["doc.md"], "doc", || false);
        assert!(matches!(res, Err(CommitError::VerificationFailed(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_working_tree_files_not_committed() {
        if !git_available() {
            return;
        }
        let dir = repo("iso");
        let a = dir.join("a.md");
        let b = dir.join("b.md");
        fs::write(&a, "a1").unwrap();
        fs::write(&b, "b1").unwrap();
        run_git(&dir, &["add", "--", "a.md", "b.md"]).unwrap();
        run_git(&dir, &["commit", "-m", "base"]).unwrap();
        fs::write(&a, "a2").unwrap();
        fs::write(&b, "b2").unwrap();

        commit_verified_edit(&dir, &["a.md"], "only a", || true).expect("commit a");
        let b_head = run_git(&dir, &["show", "HEAD:b.md"]).unwrap();
        assert_eq!(b_head, "b1", "b.md must not ride along in a's commit");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_diff_is_refused() {
        if !git_available() {
            return;
        }
        let dir = repo("empty");
        let f = dir.join("f.md");
        fs::write(&f, "same").unwrap();
        run_git(&dir, &["add", "--", "f.md"]).unwrap();
        run_git(&dir, &["commit", "-m", "base"]).unwrap();
        // No change since base → the verified gate passes but nothing lands.
        let res = commit_verified_edit(&dir, &["f.md"], "noop", || true);
        assert!(matches!(res, Err(CommitError::VerificationFailed(_))));
        let _ = fs::remove_dir_all(&dir);
    }
}
