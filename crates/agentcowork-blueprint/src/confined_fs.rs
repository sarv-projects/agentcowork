//! Bounded, link-aware filesystem removal confined to one root
//! (FIX-05 / `ARCH/12-TRUST.md` §7 · `ARCH/25-FILES.md` §7 · `REQ-SKILL-011`).
//!
//! `SkillStore::delete` used to hand a caller-supplied name to
//! `std::fs::remove_dir_all` with no confinement at all. This module is the
//! single place in the crate that walks a tree recursively, and it holds the
//! three properties the contract requires:
//!
//! 1. **Confinement.** The target is resolved against the *canonical* root and
//!    must live strictly inside it. A `..` walk, an absolute target, or a
//!    symlink/junction that leaves the root is refused typed, before any
//!    unlink.
//! 2. **No link following.** A link inside the tree that resolves outside the
//!    root is a refusal ([`ConfinedError::LinkEscape`]) rather than something
//!    to sweep. The deleter itself is `std::fs::remove_dir_all`, which never
//!    follows a link on either platform (Unix: `openat` + `O_NOFOLLOW`;
//!    Windows: `FILE_FLAG_OPEN_REPARSE_POINT`) — so the pre-flight buys *typed
//!    refusal*, not link safety, and a link planted between the two passes can
//!    only be unlinked, never traversed.
//! 3. **Boundedness.** The pre-flight refuses a tree deeper than
//!    [`MAX_SCAN_DEPTH`] or wider than [`MAX_SCAN_ENTRIES`], so a hostile
//!    package (untrusted input, `ARCH/31-SKILLS-PLUGINS.md` §7) can never turn
//!    an uninstall into an unbounded walk.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// The deepest directory nesting a pre-flight will walk. A package deeper
/// than this is refused, not walked.
pub const MAX_SCAN_DEPTH: usize = 32;

/// The most filesystem entries a pre-flight will visit in one tree.
pub const MAX_SCAN_ENTRIES: usize = 20_000;

/// What a confined pre-flight observed about a tree it approved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemovalReport {
    /// Files, directories and links visited (the pre-flight's own count).
    pub entries: usize,
    /// Directories queued for the walk.
    pub dirs: usize,
    /// Links seen and accepted (each resolved inside the root).
    pub links: usize,
}

/// Why a confined removal was refused. Every variant is a decision the caller
/// can surface; none of them delete anything.
#[derive(Debug, Error)]
pub enum ConfinedError {
    #[error("confinement root `{root}` is not an existing directory")]
    RootInvalid { root: String },
    #[error("`{path}` does not exist")]
    NotFound { path: String },
    #[error("refused to remove the confinement root itself (`{root}`)")]
    TargetIsRoot { root: String },
    #[error(
        "refused to remove `{path}`: it is a link (resolves to `{resolved}`), not a directory inside the root"
    )]
    TargetIsLink { path: String, resolved: String },
    #[error("refused to remove `{path}`: it is not a directory inside the confinement root")]
    NotADirectory { path: String },
    #[error("refused `{path}`: it resolves outside the confinement root `{root}`")]
    Outside { path: String, root: String },
    #[error("refused `{path}`: a link inside the tree leaves the confinement root `{root}`")]
    LinkEscape { path: String, root: String },
    #[error(
        "refused `{path}`: traversal bound exceeded (max_depth={max_depth}, max_entries={max_entries})"
    )]
    BoundExceeded {
        path: String,
        max_depth: usize,
        max_entries: usize,
    },
    #[error("io error at `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl ConfinedError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        ConfinedError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// Resolve `root` to the canonical absolute boundary every candidate path is
/// measured against. Resolving the root once is what makes a symlinked store
/// root (a common dotfile layout) behave like a normal directory.
pub fn canonical_root(root: &Path) -> Result<PathBuf, ConfinedError> {
    let meta = std::fs::metadata(root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfinedError::NotFound {
                path: root.display().to_string(),
            }
        } else {
            ConfinedError::io(root, e)
        }
    })?;
    if !meta.is_dir() {
        return Err(ConfinedError::RootInvalid {
            root: root.display().to_string(),
        });
    }
    std::fs::canonicalize(root).map_err(|e| ConfinedError::io(root, e))
}

/// Component-wise containment: `p` is `root` or lives under it. Both sides are
/// canonical absolute paths, so this is a real prefix test (never a string
/// match — `…/skills-evil` is not inside `…/skills`).
fn inside(root: &Path, p: &Path) -> bool {
    p == root || p.starts_with(root)
}

/// Pre-flight a removal: prove `target` is a real directory strictly inside
/// `root`, that no link inside it leaves the root, and that walking it stays
/// within [`MAX_SCAN_DEPTH`] / [`MAX_SCAN_ENTRIES`]. Nothing is deleted.
pub fn check_removal(root: &Path, target: &Path) -> Result<RemovalReport, ConfinedError> {
    let root_real = canonical_root(root)?;
    let root_s = root_real.display().to_string();

    // The target is never a link: following it would mean the "package" is
    // whatever the link points at (the classic escape).
    let meta = std::fs::symlink_metadata(target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfinedError::NotFound {
                path: target.display().to_string(),
            }
        } else {
            ConfinedError::io(target, e)
        }
    })?;
    if meta.file_type().is_symlink() {
        let resolved = std::fs::canonicalize(target)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| target.display().to_string());
        return Err(ConfinedError::TargetIsLink {
            path: target.display().to_string(),
            resolved,
        });
    }
    if !meta.is_dir() {
        return Err(ConfinedError::NotADirectory {
            path: target.display().to_string(),
        });
    }

    // Resolve the target against the canonical root: a `..` walk or a symlinked
    // component that lands elsewhere fails here, before any traversal.
    let target_real = std::fs::canonicalize(target).map_err(|e| ConfinedError::io(target, e))?;
    if target_real == root_real {
        return Err(ConfinedError::TargetIsRoot { root: root_s });
    }
    if !inside(&root_real, &target_real) {
        return Err(ConfinedError::Outside {
            path: target_real.display().to_string(),
            root: root_s,
        });
    }

    let mut report = RemovalReport::default();
    // Explicit stack (not recursion): depth is a bound we enforce, not a limit
    // we hope the call stack provides.
    let mut stack: Vec<(PathBuf, usize)> = vec![(target_real, 1)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_SCAN_DEPTH {
            return Err(ConfinedError::BoundExceeded {
                path: dir.display().to_string(),
                max_depth: MAX_SCAN_DEPTH,
                max_entries: MAX_SCAN_ENTRIES,
            });
        }
        let entries = std::fs::read_dir(&dir).map_err(|e| ConfinedError::io(&dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| ConfinedError::io(&dir, e))?;
            let path = entry.path();
            report.entries += 1;
            if report.entries > MAX_SCAN_ENTRIES {
                return Err(ConfinedError::BoundExceeded {
                    path: path.display().to_string(),
                    max_depth: MAX_SCAN_DEPTH,
                    max_entries: MAX_SCAN_ENTRIES,
                });
            }
            let meta = std::fs::symlink_metadata(&path).map_err(|e| ConfinedError::io(&path, e))?;
            if meta.file_type().is_symlink() {
                report.links += 1;
                // A dangling link has nothing to escape; the deleter unlinks it.
                match std::fs::canonicalize(&path) {
                    Ok(resolved) if !inside(&root_real, &resolved) => {
                        return Err(ConfinedError::LinkEscape {
                            path: path.display().to_string(),
                            root: root_s,
                        });
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(ConfinedError::io(&path, e)),
                }
                continue; // never traversed
            }
            if meta.is_dir() {
                report.dirs += 1;
                stack.push((path, depth + 1));
            }
        }
    }
    Ok(report)
}

/// Pre-flight ([`check_removal`]) then remove the tree. The pre-flight is
/// where a hostile tree is refused; the delete is `std::fs::remove_dir_all`,
/// which unlinks links instead of following them on every supported platform.
pub fn remove_confined(root: &Path, target: &Path) -> Result<RemovalReport, ConfinedError> {
    let report = check_removal(root, target)?;
    std::fs::remove_dir_all(target).map_err(|e| ConfinedError::io(target, e))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmpdir(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!(
            "agentcowork-confined-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tree(root: &Path, rel: &str, body: &str) -> PathBuf {
        let p = root.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("f.txt"), body).unwrap();
        p
    }

    #[test]
    fn removes_a_confined_tree_and_leaves_the_root() {
        let root = tmpdir("ok");
        let pkg = tree(&root, "pkg", "hello");
        tree(&pkg, "refs/deep", "x");
        let sibling = tree(&root, "sibling", "keep");
        tree(&root, "sibling/nested", "keep me");

        let report = remove_confined(&root, &root.join("pkg")).unwrap();
        // f.txt, refs/, refs/deep/, refs/deep/f.txt
        assert_eq!(report.entries, 4, "every entry under the target is seen");
        assert_eq!(report.dirs, 2, "both nested directories are queued");
        assert!(!pkg.exists(), "the package tree is gone");
        assert!(root.exists(), "the confinement root survives");
        assert!(sibling.join("f.txt").exists(), "siblings are untouched");
        assert!(sibling.join("nested/f.txt").exists());
    }

    #[test]
    fn refuses_the_root_itself() {
        let root = tmpdir("root");
        let err = check_removal(&root, &root).unwrap_err();
        assert!(matches!(err, ConfinedError::TargetIsRoot { .. }), "{err}");
        assert!(root.exists());
    }

    #[test]
    fn refuses_a_missing_target_and_a_non_directory_target() {
        let root = tmpdir("missing");
        let err = check_removal(&root, &root.join("nope")).unwrap_err();
        assert!(matches!(err, ConfinedError::NotFound { .. }), "{err}");
        tree(&root, "plain", "x");
        let err = check_removal(&root, &root.join("plain/f.txt")).unwrap_err();
        assert!(matches!(err, ConfinedError::NotADirectory { .. }), "{err}");
    }

    #[test]
    fn refuses_a_target_outside_the_root() {
        let base = tmpdir("outside");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let victim = tree(&base, "victim", "precious");
        let err = check_removal(&root, &victim).unwrap_err();
        assert!(matches!(err, ConfinedError::Outside { .. }), "{err}");
        assert!(victim.exists(), "nothing outside the root is touched");
    }

    #[test]
    fn refuses_a_link_that_leaves_the_root() {
        let base = tmpdir("link");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let victim = tree(&base, "victim", "precious");
        let pkg = tree(&root, "pkg", "hello");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, pkg.join("escape")).unwrap();

        #[cfg(unix)]
        {
            let err = remove_confined(&root, &pkg).unwrap_err();
            assert!(matches!(err, ConfinedError::LinkEscape { .. }), "{err}");
            assert!(pkg.exists(), "the refused tree is left intact");
            assert!(victim.exists(), "the link target survives");
            assert!(victim.join("f.txt").exists());
        }
        #[cfg(not(unix))]
        {
            let _ = (pkg, victim);
        }
    }

    #[test]
    fn accepts_a_link_that_stays_inside_the_root() {
        let base = tmpdir("innerlink");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let inside = tree(&root, "shared", "data");
        let pkg = tree(&root, "pkg", "hello");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&inside, pkg.join("shared-link")).unwrap();

        #[cfg(unix)]
        {
            let report = remove_confined(&root, &pkg).unwrap();
            assert_eq!(report.links, 1, "the link was seen and accepted");
            assert!(!pkg.exists());
            assert!(inside.join("f.txt").exists(), "the link target stays");
        }
        #[cfg(not(unix))]
        {
            let _ = (pkg, inside);
        }
    }

    #[test]
    fn refuses_a_link_at_the_target_itself() {
        let base = tmpdir("targetlink");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let victim = tree(&base, "victim", "precious");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, root.join("pkg")).unwrap();

        #[cfg(unix)]
        {
            let err = check_removal(&root, &root.join("pkg")).unwrap_err();
            assert!(matches!(err, ConfinedError::TargetIsLink { .. }), "{err}");
            assert!(victim.exists());
        }
        #[cfg(not(unix))]
        {
            let _ = (root, victim);
        }
    }

    #[test]
    fn refuses_a_tree_deeper_than_the_bound() {
        let root = tmpdir("deep");
        let mut deep = root.join("pkg");
        std::fs::create_dir_all(&deep).unwrap();
        for _ in 0..(MAX_SCAN_DEPTH + 2) {
            deep = deep.join("d");
            std::fs::create_dir_all(&deep).unwrap();
        }
        let err = check_removal(&root, &root.join("pkg")).unwrap_err();
        assert!(matches!(err, ConfinedError::BoundExceeded { .. }), "{err}");
        assert!(deep.exists(), "the refused tree is left intact");
    }
}
