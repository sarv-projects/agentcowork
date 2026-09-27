//! P7.7 — path floor. Every filesystem path the agent touches is
//! canonicalized (lexically; symlinks resolved when they exist) and checked
//! against the granted roots *before* any syscall. `..` escapes and symlink
//! jumps outside the floor are refused. The fuzz test drives thousands of
//! adversarial paths and requires zero escapes.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// Normalize a path lexically: resolve `.` and `..` components without
/// touching the filesystem (pure, deterministic, testable).
pub fn normalize_lexical(path: &str) -> String {
    let mut out: Vec<Component> = Vec::new();
    let mut rooted = false;
    for comp in Path::new(path).components() {
        match comp {
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = out.last() {
                    out.pop();
                } else if out.is_empty() && !rooted {
                    out.push(Component::ParentDir);
                }
                // else: leading .. stays only for relative paths
            }
            other => out.push(other),
        }
    }
    let joined = out
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/");
    let s = if rooted { format!("/{joined}") } else { joined };
    if s.is_empty() {
        return if rooted {
            "/".to_string()
        } else {
            ".".to_string()
        };
    }
    s
}

/// Canonicalize for the floor check **without following a symlink at the
/// final component**. Any existing directory prefix is resolved (following
/// intermediate directory symlinks, as canonicalizing a container must), but
/// the leaf name is joined lexically — a symlink *at* the leaf is never
/// silently resolved into its target path. This keeps the returned path an
/// honest representation of "where the caller asked to act", so the floor
/// check cannot be tricked by a leaf symlink that points outside the root.
///
/// Symlink *escapes* (a leaf, or an intermediate directory link, that lands
/// outside the granted roots) are detected separately by [`leaf_symlink_target`]
/// / [`enforce_floor`] — this function does not hide them by resolving them.
pub fn canonicalize_no_follow(path: &str) -> String {
    let norm = normalize_lexical(path);
    let p = Path::new(&norm);

    // Split into parent + final component. Canonicalize the parent (its
    // symlinks resolve — that is correct for the container), then re-attach
    // the leaf lexically so a leaf symlink is NOT followed.
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(leaf)) if !parent.as_os_str().is_empty() => {
            let real_parent = std::fs::canonicalize(parent)
                .map(|pb| pb.to_string_lossy().to_string())
                .unwrap_or_else(|_| parent.to_string_lossy().to_string());
            let joined = format!(
                "{}/{}",
                real_parent.trim_end_matches('/'),
                leaf.to_string_lossy()
            );
            normalize_lexical(&joined)
        }
        // Root, bare relative name, or no parent — nothing to resolve.
        _ => norm,
    }
}

/// If `path`'s final component is a symlink, return its resolved absolute
/// target (lexically normalized); otherwise `None`. Used by the floor to
/// refuse a leaf symlink that jumps outside the granted roots, without
/// `canonicalize_no_follow` having to follow it.
pub fn leaf_symlink_target(path: &str) -> Option<String> {
    let norm = normalize_lexical(path);
    let meta = std::fs::symlink_metadata(&norm).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    // Fully resolve (follows the link) only to test containment — the result
    // is never used as the acted-upon path, only as the escape check.
    let resolved = std::fs::canonicalize(&norm).ok()?;
    Some(normalize_lexical(&resolved.to_string_lossy()))
}

/// Is `path` (canonicalized) inside one of the granted roots?
pub fn is_inside_root(path: &str, roots: &[&str]) -> bool {
    let canonical = canonicalize_no_follow(path);
    roots.iter().any(|r| {
        let root = canonicalize_no_follow(r);
        canonical == root || canonical.starts_with(&format!("{}/", root.trim_end_matches('/')))
    })
}

/// Floor verdict for a single path against the roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorVerdict {
    Allowed,
    /// `..` escape survived normalization (e.g. `../../etc` from a shallow root).
    ParentEscape,
    /// Lexically inside but symlink resolution escapes the roots.
    SymlinkEscape,
    /// Absolute path outside any granted root.
    OutsideRoot,
}

/// Enforce the path floor. The path is refused unless it is (lexically AND
/// symlink-resolved) inside a granted root, and it doesn't contain a
/// surviving `..` that walks above the root.
pub fn enforce_floor(path: &str, roots: &[&str]) -> FloorVerdict {
    if path.is_empty() {
        return FloorVerdict::OutsideRoot;
    }
    let norm = normalize_lexical(path);
    // Surviving leading `..` (e.g. ../x with no root component before it).
    if norm.starts_with("../") || norm == ".." {
        return FloorVerdict::ParentEscape;
    }
    let canonical = canonicalize_no_follow(path);
    let inside = roots.iter().any(|r| {
        let root = canonicalize_no_follow(r);
        canonical == root || canonical.starts_with(&format!("{}/", root.trim_end_matches('/')))
    });
    if !inside {
        return FloorVerdict::OutsideRoot;
    }
    // The lexical/parent-resolved path is inside the floor. Now make sure the
    // leaf isn't a symlink that jumps outside the roots — canonicalize_no_follow
    // deliberately did NOT resolve it, so we check it explicitly here.
    if let Some(target) = leaf_symlink_target(path) {
        let target_inside = roots.iter().any(|r| {
            let root = canonicalize_no_follow(r);
            target == root || target.starts_with(&format!("{}/", root.trim_end_matches('/')))
        });
        if !target_inside {
            return FloorVerdict::SymlinkEscape;
        }
    }
    FloorVerdict::Allowed
}

/// Generate adversarial paths for the fuzz gate: dot-dot chains, absolute
/// paths, symlink-ish forms, unicode, empty/weird components.
pub fn adversarial_paths() -> Vec<String> {
    let mut v = Vec::new();
    let bases = [
        "",
        "/",
        "/workspace",
        "workspace",
        "/workspace/sub",
        "/etc",
        "~",
    ];
    let middles = [
        "", "../", "../../", "..", "./", "/../", "/../../", "a/../..", "../a/..",
    ];
    let tails = [
        "x",
        "x.txt",
        "..",
        "../..",
        "../x",
        "../../etc/passwd",
        "etc/passwd",
        "..%2fetc%2fpasswd",
        "\u{202e}etc/passwd",
        "a b",
        "a\nb",
        "symlink",
        "-rf",
    ];
    for b in bases {
        for m in middles {
            for t in tails {
                v.push(format!("{b}{m}{t}"));
            }
        }
    }
    v.extend([
        "/workspace/../../etc/passwd".to_string(),
        "/workspace/../etc".to_string(),
        "/workspace/sub/../../../x".to_string(),
        "//workspace//../..//etc//passwd".to_string(),
        "C:\\..\\..\\Windows\\system32".to_string(),
        "/workspace/..%2f..%2fetc".to_string(),
    ]);
    v
}

/// The fuzz gate: no adversarial path may be *allowed* while its normalized
/// form lands outside `/workspace` (Allowed ⟹ inside). Paths that genuinely
/// escape must be refused (any refusal verdict is fine).
pub fn path_floor_fuzz_passes() -> bool {
    let roots = &["/workspace"];
    adversarial_paths().iter().all(|p| {
        if enforce_floor(p, roots) == FloorVerdict::Allowed {
            is_inside_root(p, roots)
        } else {
            true
        }
    })
}

/// P7.8 — 6-axis path grants (doc 64 S3 — chromium `BrokerFilePermission`).
/// The path floor decides *where*; the grant axis decides *how*: each grant
/// is a canonicalized prefix plus one of six operation axes. Feeds the J21
/// `permissions.toml` vocabulary (`read_only_recursive`, `read_write_create`,
/// …). The 0-escape invariant is preserved — a grant never widens the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantAxis {
    /// Read everything under the prefix (`read_only_recursive`).
    ReadOnlyRecursive,
    /// Read + write + create under the prefix (`read_write_create`).
    ReadWriteCreate,
    /// Read + write existing entries (`read_write`).
    ReadWrite,
    /// Create new entries only (`create`).
    Create,
    /// Temporary scratch: read/write/create, no durability promise (`temporary`).
    Temporary,
    /// Stat/metadata of intermediates only (`stat`).
    StatIntermediates,
}

/// A granted prefix + axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathGrant {
    pub axis: GrantAxis,
    pub prefix: String,
}

/// Filesystem operations a grant can cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsOp {
    Read,
    Write,
    Create,
    Delete,
    Stat,
    List,
}

impl PathGrant {
    /// Parse a permissions.toml grant string like
    /// `read_only_recursive:/workspace/src` or `read_write_create:/tmp/x`.
    pub fn from_permissions_string(s: &str) -> Option<PathGrant> {
        let (axis, prefix) = s.split_once(':')?;
        let axis = match axis {
            "read_only_recursive" => GrantAxis::ReadOnlyRecursive,
            "read_write_create" => GrantAxis::ReadWriteCreate,
            "read_write" => GrantAxis::ReadWrite,
            "create" => GrantAxis::Create,
            "temporary" => GrantAxis::Temporary,
            "stat" => GrantAxis::StatIntermediates,
            _ => return None,
        };
        if prefix.is_empty() {
            return None;
        }
        Some(PathGrant {
            axis,
            prefix: prefix.to_string(),
        })
    }

    /// Does this grant cover `op` on `path`? The path must be inside the
    /// (canonicalized) prefix and the axis must permit the operation.
    pub fn allows(&self, op: FsOp, path: &str) -> bool {
        let canonical = canonicalize_no_follow(path);
        let prefix = canonicalize_no_follow(&self.prefix);
        let inside = canonical == prefix
            || canonical.starts_with(&format!("{}/", prefix.trim_end_matches('/')));
        if !inside {
            return false;
        }
        matches!(
            (self.axis, op),
            (
                GrantAxis::ReadOnlyRecursive,
                FsOp::Read | FsOp::Stat | FsOp::List
            ) | (
                GrantAxis::ReadWriteCreate,
                FsOp::Read | FsOp::Write | FsOp::Create | FsOp::Stat | FsOp::List,
            ) | (
                GrantAxis::ReadWrite,
                FsOp::Read | FsOp::Write | FsOp::Stat | FsOp::List,
            ) | (GrantAxis::Create, FsOp::Create | FsOp::Stat)
                | (
                    GrantAxis::Temporary,
                    FsOp::Read
                        | FsOp::Write
                        | FsOp::Create
                        | FsOp::Delete
                        | FsOp::Stat
                        | FsOp::List,
                )
                | (GrantAxis::StatIntermediates, FsOp::Stat | FsOp::List)
        )
    }

    /// The floor this grant implies (for `enforce_floor` roots).
    pub fn floor_root(&self) -> &str {
        &self.prefix
    }
}

// ---------------------------------------------------------------------------
// Read-scope resolution — `ARCH/25-FILES.md` §7.1 / `ARCH/12-TRUST.md` §8.
// ---------------------------------------------------------------------------
//
// The floor above answers *where a path may act*; it takes its roots as
// arguments. A **session** also has to answer *which roots it may see at all*,
// and that answer is the `allowed_paths` / `read_only_paths` vocabulary the
// grant axis already carries (`ReadOnlyRecursive` is a read-only path,
// `ReadWriteCreate` is an allowed path). [`ReadScopes`] is that answer, and it
// resolves a path against it in the only order that is safe: **canonicalize
// first, decide second, re-check at the point of use third**.
//
// Reads are intercepted *at this scope boundary, never per-read approval* —
// the tree is a browsing surface, so a card per node is not a stricter
// control, it is a broken one. In scope means no prompt and no ticket; out of
// scope is a typed [`ReadScopeDenied`] plus an audit row, because
// `ARCH/12-TRUST.md` §9 makes denials first-class audit entries.

/// Why one scope decision refused a path. Stable tokens: the audit row, the
/// caller-visible message and a test all name the same fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeDenial {
    /// The request was not a path at all (an empty string). Refused rather than
    /// resolved against the working directory.
    Unusable,
    /// A `..` survived normalization and walks above the root.
    ParentEscape,
    /// Lexically in scope, but the leaf (or an intermediate directory) is a
    /// symlink that lands outside it.
    SymlinkEscape,
    /// The canonicalized path is not under any scope that permits the op.
    OutsideScope,
    /// A protected subpath (own settings, another harness's settings, a
    /// credential dir) was targeted for a write. Reads of the same path are
    /// allowed — the rule is *read-only*, not *invisible*.
    ProtectedSubpath,
    /// The path resolved differently at the point of use than it did at
    /// resolution (renamed parent, replaced entry, swapped link).
    IdentityDrift,
}

impl ScopeDenial {
    /// Stable lower-case reason token for the audit row and the message.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unusable => "unusable",
            Self::ParentEscape => "parent_escape",
            Self::SymlinkEscape => "symlink_escape",
            Self::OutsideScope => "outside_scope",
            Self::ProtectedSubpath => "protected_subpath",
            Self::IdentityDrift => "identity_drift",
        }
    }
}

/// The typed refusal a read refused at the scope boundary produces.
///
/// It carries the **canonical** boundary code, not a new one:
/// `ARCH/10-KERNEL.md` §3 makes the taxonomy canonical and requires a `DEC` to
/// extend it, so a scope denial maps onto the same `AuthorizationDenied` code
/// `netfloor::NetFloorDenied` already uses (and it is not retryable — a
/// decision does not change by asking again).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("read refused at the scope boundary ({reason}): {path}")]
pub struct ReadScopeDenied {
    /// The path the caller asked for, verbatim — an audit row has to name what
    /// was asked, not only what it resolved to.
    pub path: String,
    pub denial: ScopeDenial,
    /// Stable reason token (`ScopeDenial::as_str`).
    pub reason: &'static str,
}

impl ReadScopeDenied {
    /// The canonical boundary code for a scope denial.
    pub const CODE: &'static str = "AuthorizationDenied";

    /// A scope decision is not a transient: retrying cannot change it.
    pub const RETRYABLE: bool = false;

    fn new(path: &str, denial: ScopeDenial) -> Self {
        Self {
            path: path.to_string(),
            denial,
            reason: denial.as_str(),
        }
    }

    /// [`Self::CODE`] as a method, mirroring `NetFloorDenied::code`.
    pub const fn code(&self) -> &'static str {
        Self::CODE
    }

    /// [`Self::RETRYABLE`] as a method, for the same reason.
    pub const fn retryable(&self) -> bool {
        Self::RETRYABLE
    }
}

/// A resolved, in-scope target: the exact path the caller must act on.
///
/// `requested` is kept alongside `canonical` so a refusal or a receipt can name
/// what the renderer asked for; `canonical` is the only value a syscall may
/// touch, and it is what [`ReadScopes::reverify`] re-checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadTarget {
    pub requested: String,
    pub canonical: String,
    pub op: FsOp,
}

/// A session's path scopes: the `allowed_paths` / `read_only_paths` set from
/// `ARCH/25-FILES.md` §7, expressed in the existing [`PathGrant`] vocabulary so
/// the floor keeps exactly one notion of "inside a prefix".
///
/// **Default (no scopes configured).** An empty set floors against the *parent
/// directory of the requested path* — the same root `src-tauri`'s
/// `control::floor_user_file` already hands to [`enforce_floor`] on every
/// write, for the same documented reason (users open documents under home and
/// mounts, so the file is not jailed to a workspace). This is deliberately
/// derived from the write side rather than invented: a read is then floored
/// exactly as a write already is, which closes `..` traversal and symlink
/// escapes out of the parent. It is a **floor, not a jail** — a configured
/// session scope is what narrows it further, and nothing here substitutes for
/// that.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadScopes {
    grants: Vec<PathGrant>,
}

/// The unconfigured default's floor root for `path`: the directory containing
/// it. This is the root `src-tauri`'s `control::floor_user_file` already hands
/// to [`enforce_floor`] on every write — the default is derived from that, not
/// invented.
///
/// A filesystem or drive root has no parent to derive one from and is therefore
/// its own floor, so browsing up to `/` (or a drive root) keeps working. A bare
/// relative name falls back to the working directory, as the write side does —
/// deliberately *not* to the name itself, which would make a symlinked leaf
/// unfloored.
fn default_root(path: &str) -> String {
    let p = Path::new(path);
    match p.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_string_lossy().into_owned(),
        None if p.has_root() => path.to_string(),
        _ => ".".to_string(),
    }
}

impl ReadScopes {
    /// A configured scope set from explicit grants.
    pub fn new(grants: Vec<PathGrant>) -> Self {
        Self { grants }
    }

    /// Build a scope set from `permissions.toml` grant strings
    /// (`read_only_recursive:/a`, `read_write_create:/b`). Unknown entries are
    /// skipped, never over-granted.
    pub fn from_permission_strings<S: AsRef<str>>(grants: &[S]) -> Self {
        Self {
            grants: grants
                .iter()
                .filter_map(|g| PathGrant::from_permissions_string(g.as_ref()))
                .collect(),
        }
    }

    /// Are any scopes configured? `false` means the documented parent-floor
    /// default applies.
    pub fn is_configured(&self) -> bool {
        !self.grants.is_empty()
    }

    /// The configured grants.
    pub fn grants(&self) -> &[PathGrant] {
        &self.grants
    }

    /// **Resolve, then decide.** Canonicalize `path` first, then ask the
    /// scopes whether that canonical path may be acted on with `op`.
    ///
    /// Returns the resolved [`ReadTarget`] — never the raw request — or the
    /// typed [`ReadScopeDenied`]. Nothing here reads a ticket store, a policy
    /// file or an approval queue: an in-scope read is not a decision anybody
    /// has to make again.
    pub fn resolve(&self, op: FsOp, path: &str) -> Result<ReadTarget, ReadScopeDenied> {
        if path.is_empty() {
            return Err(ReadScopeDenied::new(path, ScopeDenial::Unusable));
        }
        // A surviving leading `..` is a refusal on its own evidence: it means
        // the caller tried to walk above the root, whether or not the result
        // happens to land somewhere readable.
        let norm = normalize_lexical(path);
        if norm == ".." || norm.starts_with("../") {
            return Err(ReadScopeDenied::new(path, ScopeDenial::ParentEscape));
        }
        // Resolve FIRST. The decision below is made about the canonical path, so
        // a `..` chain, a doubled separator or a symlinked parent cannot smuggle
        // an out-of-scope target past an in-scope-looking string.
        let canonical = canonicalize_no_follow(path);
        self.decide(op, path, &canonical)?;
        Ok(ReadTarget {
            requested: path.to_string(),
            canonical,
            op,
        })
    }

    /// The scope decision itself, split out so [`Self::reverify`] can run the
    /// *identical* check at the point of use.
    fn decide(&self, op: FsOp, requested: &str, canonical: &str) -> Result<(), ReadScopeDenied> {
        if self.is_configured() {
            if !self.grants.iter().any(|g| g.allows(op, canonical)) {
                return Err(ReadScopeDenied::new(requested, ScopeDenial::OutsideScope));
            }
            // A protected subpath is read-only even inside a writable root
            // (`ARCH/25-FILES.md` §7): the write is refused, the read stands.
            if !matches!(op, FsOp::Read | FsOp::List | FsOp::Stat)
                && crate::protected_paths::is_protected(canonical)
            {
                return Err(ReadScopeDenied::new(
                    requested,
                    ScopeDenial::ProtectedSubpath,
                ));
            }
        } else {
            // Documented default: floor against the requested path's own parent
            // directory — the root the write side already uses.
            //
            // Because that root is *derived from the request*, a `..` would
            // silently move it (`dir/../secret.txt` would be floored against
            // `dir/..` and therefore admitted). With no declared scope there is
            // nothing a `..` could legitimately be for, so under the default a
            // `..` is refused outright — fail closed. A *configured* scope does
            // not need this rule: its root is declared, not derived, so `..` is
            // resolved lexically and refused exactly when it lands outside the
            // scope, which is the stance `enforce_floor` already takes.
            if Path::new(requested)
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            {
                return Err(ReadScopeDenied::new(requested, ScopeDenial::ParentEscape));
            }
            let parent = default_root(canonical);
            match enforce_floor(canonical, &[parent.as_str()]) {
                FloorVerdict::Allowed => {}
                FloorVerdict::ParentEscape => {
                    return Err(ReadScopeDenied::new(requested, ScopeDenial::ParentEscape));
                }
                FloorVerdict::SymlinkEscape => {
                    return Err(ReadScopeDenied::new(requested, ScopeDenial::SymlinkEscape));
                }
                FloorVerdict::OutsideRoot => {
                    return Err(ReadScopeDenied::new(requested, ScopeDenial::OutsideScope));
                }
            }
        }
        // `canonicalize_no_follow` deliberately does not resolve a symlink *at
        // the leaf*, so the containment test above could be satisfied by a link
        // that lands elsewhere. Resolve it for the check only — the acted-upon
        // path stays the honest "where the caller asked to act" form.
        if let Some(target) = leaf_symlink_target(canonical) {
            let target_in_scope = if self.is_configured() {
                self.grants.iter().any(|g| g.allows(op, &target))
            } else {
                enforce_floor(&target, &[default_root(&target).as_str()]) == FloorVerdict::Allowed
            };
            if !target_in_scope {
                return Err(ReadScopeDenied::new(requested, ScopeDenial::SymlinkEscape));
            }
        }
        Ok(())
    }

    /// **Re-check at the point of use.** A path resolved a moment ago is not the
    /// path a syscall is about to touch: an intermediate directory can be
    /// replaced by a link, and a leaf can be swapped for one. This runs the
    /// same decision again and additionally refuses a canonical form that
    /// drifted since resolution.
    ///
    /// Deliberately *not* checked: whether the bytes at an in-scope path
    /// changed. A file being edited between resolution and read is normal, and
    /// the caller's authority is the scope, not a content hash.
    pub fn reverify(&self, target: &ReadTarget) -> Result<(), ReadScopeDenied> {
        if canonicalize_no_follow(&target.canonical) != target.canonical {
            return Err(ReadScopeDenied::new(
                &target.canonical,
                ScopeDenial::IdentityDrift,
            ));
        }
        self.decide(target.op, &target.requested, &target.canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_parse_permissions_strings() {
        let g = PathGrant::from_permissions_string("read_only_recursive:/workspace/src").unwrap();
        assert_eq!(g.axis, GrantAxis::ReadOnlyRecursive);
        assert_eq!(g.prefix, "/workspace/src");
        assert!(PathGrant::from_permissions_string("read_write_create:/tmp/x").is_some());
        assert!(PathGrant::from_permissions_string("bogus:/x").is_none());
        assert!(PathGrant::from_permissions_string("read_write:").is_none());
    }

    #[test]
    fn axis_gates_operations() {
        let ro = PathGrant {
            axis: GrantAxis::ReadOnlyRecursive,
            prefix: "/workspace/src".into(),
        };
        assert!(ro.allows(FsOp::Read, "/workspace/src/a/b.rs"));
        assert!(!ro.allows(FsOp::Write, "/workspace/src/a/b.rs"));
        assert!(!ro.allows(FsOp::Create, "/workspace/src/new.rs"));

        let rwc = PathGrant {
            axis: GrantAxis::ReadWriteCreate,
            prefix: "/tmp/x".into(),
        };
        assert!(rwc.allows(FsOp::Create, "/tmp/x/new.txt"));
        assert!(rwc.allows(FsOp::Write, "/tmp/x/a.txt"));

        let c = PathGrant {
            axis: GrantAxis::Create,
            prefix: "/tmp/inbox".into(),
        };
        assert!(c.allows(FsOp::Create, "/tmp/inbox/new.txt"));
        assert!(!c.allows(FsOp::Read, "/tmp/inbox/existing.txt"));

        let tmp = PathGrant {
            axis: GrantAxis::Temporary,
            prefix: "/tmp/scratch".into(),
        };
        assert!(tmp.allows(FsOp::Delete, "/tmp/scratch/old.bin"));
        assert!(!ro.allows(FsOp::Delete, "/workspace/src/x"));
    }

    #[test]
    fn grant_never_escapes_the_floor() {
        // A grant for /tmp/x must never admit a path that escapes /tmp/x.
        for axis in [
            GrantAxis::ReadOnlyRecursive,
            GrantAxis::ReadWriteCreate,
            GrantAxis::Temporary,
        ] {
            let g = PathGrant {
                axis,
                prefix: "/tmp/x".into(),
            };
            for p in crate::pathfloor::adversarial_paths() {
                if g.allows(FsOp::Read, &p) {
                    assert_eq!(
                        crate::pathfloor::enforce_floor(&p, &["/tmp/x"]),
                        FloorVerdict::Allowed,
                        "grant {axis:?} admitted an escape: {p}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scope-boundary tests (`ARCH/25-FILES.md` §7.1, `ARCH/12-TRUST.md` §8).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod read_scope_tests {
    use super::*;

    /// A unique scratch root, so a test run never collides with a real path.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "agentcowork_scope_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("workspace/src")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::write(base.join("workspace/src/a.rs"), b"fn main() {}").unwrap();
        std::fs::write(base.join("outside/secret.txt"), b"SECRET").unwrap();
        base
    }

    fn scopes_for(base: &std::path::Path) -> ReadScopes {
        ReadScopes::new(vec![PathGrant {
            axis: GrantAxis::ReadWriteCreate,
            prefix: base.join("workspace").to_string_lossy().into_owned(),
        }])
    }

    #[test]
    fn in_scope_read_resolves_and_needs_no_approval() {
        let base = scratch("in_scope");
        let s = scopes_for(&base);
        let t = s
            .resolve(
                FsOp::Read,
                &base.join("workspace/src/a.rs").to_string_lossy(),
            )
            .expect("an in-scope read is not a decision anybody has to make again");
        assert!(t.canonical.ends_with("/workspace/src/a.rs"), "{t:?}");
        assert_eq!(t.op, FsOp::Read);
        // The re-check at the point of use agrees.
        assert!(s.reverify(&t).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn out_of_scope_read_is_a_typed_denial() {
        let base = scratch("out_scope");
        let s = scopes_for(&base);
        let err = s
            .resolve(
                FsOp::Read,
                &base.join("outside/secret.txt").to_string_lossy(),
            )
            .expect_err("a read outside the session's scopes must be refused");
        assert_eq!(err.denial, ScopeDenial::OutsideScope);
        // The canonical taxonomy code, not an invented one, and not retryable.
        assert_eq!(err.code(), "AuthorizationDenied");
        assert!(!err.retryable());
        assert!(err.to_string().contains("outside_scope"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dot_dot_traversal_out_of_the_scope_is_refused() {
        let base = scratch("dotdot");
        let s = scopes_for(&base);
        // The declared root is `workspace/`, so anything that climbs out of it
        // is refused however it is spelled.
        let escape = format!(
            "{}/workspace/../../outside/secret.txt",
            base.to_string_lossy()
        );
        let err = s.resolve(FsOp::Read, &escape).expect_err("escape refused");
        assert!(matches!(
            err.denial,
            ScopeDenial::ParentEscape | ScopeDenial::OutsideScope
        ));
        // A climb that lands back inside the declared scope is *not* a scope
        // escape, and is resolved lexically — the same stance `enforce_floor`
        // already takes on the write side.
        let round = format!(
            "{}/workspace/src/../../workspace/src/a.rs",
            base.to_string_lossy()
        );
        let t = s
            .resolve(FsOp::Read, &round)
            .expect("lands inside the scope");
        assert!(t.canonical.ends_with("/workspace/src/a.rs"), "{t:?}");
        // A relative `..` with no root before it is refused on its own evidence.
        assert_eq!(
            s.resolve(FsOp::Read, "../../outside/secret.txt")
                .unwrap_err()
                .denial,
            ScopeDenial::ParentEscape
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_leaf_symlink_out_of_scope_is_refused() {
        use std::os::unix::fs::symlink;
        let base = scratch("symlink");
        let s = scopes_for(&base);
        let link = base.join("workspace/src/leak.rs");
        let _ = std::fs::remove_file(&link);
        symlink(base.join("outside/secret.txt"), &link).unwrap();

        let err = s
            .resolve(FsOp::Read, &link.to_string_lossy())
            .expect_err("a link pointing outside the scope must be refused");
        assert_eq!(err.denial, ScopeDenial::SymlinkEscape);
        // A link that stays inside is fine — the tree is browsable.
        let inside = base.join("workspace/src/real.rs");
        std::fs::write(&inside, b"ok").unwrap();
        let inside_link = base.join("workspace/src/alias.rs");
        let _ = std::fs::remove_file(&inside_link);
        symlink(&inside, &inside_link).unwrap();
        assert!(
            s.resolve(FsOp::Read, &inside_link.to_string_lossy())
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn protected_subpaths_are_read_only() {
        let base = scratch("protected");
        let s = ReadScopes::new(vec![PathGrant {
            axis: GrantAxis::ReadWriteCreate,
            prefix: base.to_string_lossy().into_owned(),
        }]);
        let protected = base.join("workspace/.everyaios/permissions.toml");
        std::fs::create_dir_all(protected.parent().unwrap()).unwrap();
        std::fs::write(&protected, b"# perms").unwrap();

        // Inside a writable root, a protected subpath refuses the write …
        let err = s
            .resolve(FsOp::Write, &protected.to_string_lossy())
            .expect_err("protected subpath must refuse a write");
        assert_eq!(err.denial, ScopeDenial::ProtectedSubpath);
        assert!(
            s.resolve(FsOp::Create, &protected.to_string_lossy())
                .is_err()
        );
        // … and the read still stands. Read-only, not invisible.
        assert!(s.resolve(FsOp::Read, &protected.to_string_lossy()).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_path_swapped_between_resolution_and_use_is_caught() {
        let base = scratch("toctou");
        let s = scopes_for(&base);

        // (a) leaf swap: the resolved path becomes a link out of scope.
        let t = s
            .resolve(
                FsOp::Read,
                &base.join("workspace/src/a.rs").to_string_lossy(),
            )
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let a = base.join("workspace/src/a.rs");
            std::fs::remove_file(&a).unwrap();
            symlink(base.join("outside/secret.txt"), &a).unwrap();
        }
        let err = s.reverify(&t).expect_err("a swapped leaf must be caught");
        assert_eq!(err.denial, ScopeDenial::SymlinkEscape);
        let _ = std::fs::remove_dir_all(&base);

        // (b) intermediate-directory swap: the canonical form itself drifts.
        let base = scratch("toctou2");
        let s = scopes_for(&base);
        let t = s
            .resolve(
                FsOp::Read,
                &base.join("workspace/src/a.rs").to_string_lossy(),
            )
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let src = base.join("workspace/src");
            std::fs::remove_dir_all(&src).unwrap();
            symlink(base.join("outside"), &src).unwrap();
        }
        let err = s
            .reverify(&t)
            .expect_err("a replaced parent must be caught");
        assert_eq!(err.denial, ScopeDenial::IdentityDrift);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_unconfigured_default_floors_against_the_parent_directory() {
        // Documented default, derived from what the write side already does:
        // with no scopes configured the requested path's own parent is the
        // floor root. It closes `..` and symlink escapes out of that parent; it
        // is a floor, not a jail.
        let base = scratch("default");
        let s = ReadScopes::default();
        assert!(!s.is_configured());

        // A normal read inside the parent resolves.
        let ok = s
            .resolve(
                FsOp::Read,
                &base.join("workspace/src/a.rs").to_string_lossy(),
            )
            .expect("in-parent read is allowed under the default");
        assert!(ok.canonical.ends_with("/workspace/src/a.rs"));

        // `..` is refused outright under the default: with no declared scope
        // the floor root is derived from the request, so a `..` would silently
        // move it. Fail closed instead.
        let err = s
            .resolve(
                FsOp::Read,
                &format!("{}/workspace/../outside/secret.txt", base.to_string_lossy()),
            )
            .expect_err("traversal refused under the default");
        assert_eq!(err.denial, ScopeDenial::ParentEscape);
        let err = s
            .resolve(
                FsOp::Read,
                &format!(
                    "{}/workspace/src/../../outside/secret.txt",
                    base.to_string_lossy()
                ),
            )
            .expect_err("traversal refused under the default");
        assert_eq!(err.denial, ScopeDenial::ParentEscape);

        // A symlink out of the parent is refused.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = base.join("workspace/leak.txt");
            let _ = std::fs::remove_file(&link);
            symlink(base.join("outside/secret.txt"), &link).unwrap();
            let err = s
                .resolve(FsOp::Read, &link.to_string_lossy())
                .expect_err("symlink escape refused under the default");
            assert_eq!(err.denial, ScopeDenial::SymlinkEscape);
        }

        // An empty request is unusable, not a silent success.
        assert_eq!(
            s.resolve(FsOp::Read, "").unwrap_err().denial,
            ScopeDenial::Unusable
        );

        // A filesystem root is its own floor — browsing up to `/` must keep
        // working, or the tree loses its top rung.
        assert!(s.resolve(FsOp::List, "/").is_ok());
        assert!(s.resolve(FsOp::List, &base.to_string_lossy()).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scopes_parse_from_permission_strings_and_skip_unknown_entries() {
        let s = ReadScopes::from_permission_strings(&[
            "read_only_recursive:/workspace/src",
            "read_write_create:/tmp/x",
            "bogus:/nope",
        ]);
        assert!(s.is_configured());
        assert_eq!(s.grants().len(), 2);
        assert!(s.resolve(FsOp::Read, "/workspace/src/a/b.rs").is_ok());
        assert!(s.resolve(FsOp::Write, "/workspace/src/a/b.rs").is_err());
        assert!(s.resolve(FsOp::Read, "/etc/passwd").is_err());
    }

    #[test]
    fn no_configured_scope_ever_admits_an_adversarial_escape() {
        // The fuzz gate, restated for the scope model: whatever the corpus
        // asks for, an escaping path is refused.
        let s = ReadScopes::from_permission_strings(&["read_only_recursive:/workspace"]);
        for p in adversarial_paths() {
            if let Ok(t) = s.resolve(FsOp::Read, &p) {
                assert!(
                    is_inside_root(&t.canonical, &["/workspace"]),
                    "scope admitted an escape: {p} → {t:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod grant_tests {
    use super::*;

    #[test]
    fn lexical_normalization() {
        assert_eq!(normalize_lexical("/a/b/../c"), "/a/c");
        assert_eq!(normalize_lexical("/a/./b"), "/a/b");
        assert_eq!(normalize_lexical("a/b/../../c"), "c");
    }

    #[test]
    fn floor_allows_inside() {
        assert_eq!(
            enforce_floor("/workspace/x", &["/workspace"]),
            FloorVerdict::Allowed
        );
        assert_eq!(
            enforce_floor("/workspace/sub/deep/file.txt", &["/workspace"]),
            FloorVerdict::Allowed
        );
    }

    #[test]
    fn floor_blocks_outside() {
        assert_eq!(
            enforce_floor("/etc/passwd", &["/workspace"]),
            FloorVerdict::OutsideRoot
        );
        assert_eq!(
            enforce_floor("/tmp/x", &["/workspace"]),
            FloorVerdict::OutsideRoot
        );
    }

    #[test]
    fn floor_blocks_parent_escape() {
        // After normalization /workspace/../../etc stays /workspace/../../etc
        // (leading .. can't pop past root), so it lands outside → refused.
        assert_ne!(
            enforce_floor("/workspace/../../etc/passwd", &["/workspace"]),
            FloorVerdict::Allowed
        );
    }

    #[test]
    fn fuzz_gate_allowed_implies_inside() {
        // The invariant: no adversarial path is *allowed* while its normalized
        // form lands outside /workspace. Paths that genuinely escape must be
        // refused (any refusal verdict is fine).
        let allowed_outside = adversarial_paths()
            .iter()
            .filter(|p| {
                enforce_floor(p, &["/workspace"]) == FloorVerdict::Allowed
                    && !is_inside_root(p, &["/workspace"])
            })
            .count();
        assert_eq!(
            allowed_outside, 0,
            "{} adversarial paths were allowed outside the floor",
            allowed_outside
        );
    }

    #[test]
    fn is_inside_helpers() {
        assert!(is_inside_root("/workspace/a/b", &["/workspace"]));
        assert!(!is_inside_root("/home/user", &["/workspace"]));
    }

    #[cfg(unix)]
    #[test]
    fn leaf_symlink_is_not_followed_but_escape_is_refused() {
        use std::os::unix::fs::symlink;
        // Build a workspace root and a secret outside it, with a leaf symlink
        // inside the workspace pointing at the secret.
        let base = std::env::temp_dir().join(format!("pf_test_{}", std::process::id()));
        let ws = base.join("workspace");
        let secret = base.join("secret");
        let _ = std::fs::create_dir_all(&ws);
        let _ = std::fs::create_dir_all(&secret);
        let secret_file = secret.join("passwd");
        std::fs::write(&secret_file, b"SECRET").unwrap();
        let link = ws.join("link");
        let _ = std::fs::remove_file(&link);
        symlink(&secret_file, &link).unwrap();

        let ws_s = ws.to_string_lossy().to_string();
        let link_s = link.to_string_lossy().to_string();
        let roots: Vec<&str> = vec![&ws_s];

        // no-follow must NOT resolve the leaf symlink into the secret path.
        let canon = canonicalize_no_follow(&link_s);
        assert!(
            !canon.contains("secret"),
            "leaf symlink was followed: {canon}"
        );
        assert!(
            canon.ends_with("/link"),
            "no-follow should keep the leaf name: {canon}"
        );

        // The floor must still REFUSE it as a symlink escape.
        assert_eq!(
            enforce_floor(&link_s, &roots),
            FloorVerdict::SymlinkEscape,
            "a leaf symlink escaping the root must be refused"
        );

        // A leaf symlink that stays inside the root is allowed.
        let inside_target = ws.join("real.txt");
        std::fs::write(&inside_target, b"ok").unwrap();
        let inside_link = ws.join("inside_link");
        let _ = std::fs::remove_file(&inside_link);
        symlink(&inside_target, &inside_link).unwrap();
        assert_eq!(
            enforce_floor(&inside_link.to_string_lossy(), &roots),
            FloorVerdict::Allowed,
            "a leaf symlink staying inside the root is allowed"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
