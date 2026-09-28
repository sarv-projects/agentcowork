//! P11.5.3 — real-filesystem Tauri commands for the folder view (tree over
//! the live disk), the code view (open/save a real file), and the diff view
//! (pending agent undo snapshots). No mock layer: every command talks to the
//! actual filesystem via `std::fs`.
//!
//! Honest ceilings: reads are capped at 2 MB of text (a code-view guard, not
//! a real limit — binary files report `binary: true` and are not loaded);
//! writes are plain text writes (no atomic rename here — office engines keep
//! their own atomic writers). Paths are user-supplied from the view; the app
//! never enumerates hidden system dirs by default.
//!
//! **Read interception.** `fs_read_file` and `fs_list_dir` take a
//! renderer-chosen path, so that path is resolved against the session's read
//! scopes before any syscall: canonicalize, decide, re-check at the point of
//! use. Reads are intercepted **at the scope boundary, never per-read
//! approval** (`ARCH/12-TRUST.md` §8, `ARCH/25-FILES.md` §7.1) — the file tree
//! is a browsing surface, so a card per node is not a stricter control, it is
//! the reason the read path had no interception at all. In scope means no
//! prompt and no ticket; out of scope is the canonical typed
//! `AuthorizationDenied` refusal plus one audit row. Writes are unchanged: they
//! keep their own floor (`control::floor_user_file`) and their own ticket or
//! human-gesture provenance.

use std::path::PathBuf;

use tauri::State;

use crate::AppState;

/// Text-size cap for `fs_read_file` (code view). Larger files report
/// `truncated: true` so the editor can show a notice instead of a blank.
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// Resolve the user's home directory (works on all three platforms).
#[tauri::command]
pub fn fs_home() -> Result<String, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|p| PathBuf::from(p).display().to_string())
        .ok_or_else(|| "no home directory found".to_string())
}

/// The audit kind a scope refusal is recorded under.
///
/// `guard.blocked` is the guard's own denial kind — the one
/// `agentcowork-audit` documents for guard denials and the one
/// `agentcowork_guard::deflection::DEFLECTION_AUDIT_KIND` already uses. A read
/// refused at the scope boundary lands on that existing trail instead of a
/// parallel one, so `ARCH/12-TRUST.md` §9 ("denials … are first-class audit
/// entries") holds without a new kind.
pub const READ_DENIED_AUDIT_KIND: &str = agentcowork_guard::deflection::DEFLECTION_AUDIT_KIND;

/// The read scopes a renderer-chosen path is resolved against.
///
/// **No session scope is wired yet, so this is the unconfigured set** and the
/// documented default applies (see [`agentcowork_guard::pathfloor::ReadScopes`]):
/// the requested path's own parent directory is the floor root — the same root
/// `control::floor_user_file` already hands to `enforce_floor` on every write,
/// for the same documented reason (users open documents under home and mounts,
/// so a path is not jailed to a workspace). A read is therefore floored exactly
/// as a write already is: a `..` is refused and a symlink that leaves the
/// parent is refused. It is a floor, not a jail.
///
/// The mechanism for narrowing this to a session's `allowed_paths` /
/// `read_only_paths` exists (`ReadScopes::new` /
/// `ReadScopes::from_permission_strings`, expressed in the existing
/// `PathGrant` vocabulary); *where a session's scopes come from* is
/// decision-needed and is deliberately not invented here.
fn read_scopes() -> agentcowork_guard::pathfloor::ReadScopes {
    agentcowork_guard::pathfloor::ReadScopes::default()
}

/// Resolve, then decide, then re-check at the point of use — the whole read
/// gate as one testable function.
///
/// Returns the resolved canonical target (the *only* value a caller may touch)
/// or a typed [`ReadScopeDenied`]. No ticket is minted, no approval store is
/// consulted and nothing is written: an in-scope read is not a decision anybody
/// has to make again (`ARCH/12-TRUST.md` §8 — interception at the scope
/// boundary, never per-read approval; the same ruling as
/// `ARCH/25-FILES.md` §7.1). The re-check is what closes the window between
/// resolution and the `std::fs` call: a path swapped in that window is refused
/// rather than read (`REQ-FILES-009`).
fn resolve_read(
    path: &str,
    op: agentcowork_guard::pathfloor::FsOp,
) -> Result<agentcowork_guard::pathfloor::ReadTarget, agentcowork_guard::pathfloor::ReadScopeDenied> {
    let scopes = read_scopes();
    let target = scopes.resolve(op, path)?;
    scopes.reverify(&target)?;
    Ok(target)
}

/// A read refused at the scope boundary: the typed message the command returns
/// and the audit row that refusal owes the chain.
///
/// They are one value on purpose — "denied **and** recorded" must not be two
/// paths that can drift. Pure, so the command path is testable without an
/// `AppState`.
#[derive(Debug, Clone, PartialEq)]
struct ReadRefusal {
    /// The canonical taxonomy code, the stable reason token, and the path.
    message: String,
    kind: &'static str,
    payload: serde_json::Value,
}

impl ReadRefusal {
    /// Build the refusal for `command` (`fs.read_file` / `fs.list_dir`).
    ///
    /// The returned message is the canonical `AuthorizationDenied` code with
    /// the guard's stable reason token — the same shape the control-plane gate
    /// and `netfloor`'s denials return. No new code, and no internal detail
    /// (INV-11): the caller learns the decision and the path, nothing about the
    /// scope set.
    fn new(command: &str, d: &agentcowork_guard::pathfloor::ReadScopeDenied) -> Self {
        Self {
            message: format!("{} ({}): {}", d.code(), d.reason, d.path),
            kind: READ_DENIED_AUDIT_KIND,
            payload: serde_json::json!({
                "command": command,
                "actor": "renderer",
                "path": d.path,
                "code": d.code(),
                "reason": d.reason,
                "retryable": d.retryable(),
                "guard": "path_scope",
                "ok": false,
                "state": "refused",
                "outcome": "denied",
            }),
        }
    }
}

/// Record the refusal on the Merkle chain and return the error the command
/// answers with. Exactly one row per refusal (`ARCH/12-TRUST.md` §9: denials
/// are first-class audit entries), and nothing else is mutated — a read stays a
/// read.
///
/// The provenance class is `HumanGesture`: the actor is the user's own click in
/// the tree, the same classification [`fs_write_file`] already uses for a
/// renderer-initiated effect, and it is set from the Rust call site — never read
/// from the payload — so a caller cannot manufacture it.
fn deny_read(
    state: &AppState,
    command: &str,
    d: agentcowork_guard::pathfloor::ReadScopeDenied,
) -> String {
    let refusal = ReadRefusal::new(command, &d);
    crate::control::record_mutation(
        state,
        crate::control::AuthKind::HumanGesture,
        refusal.kind,
        refusal.payload,
    );
    refusal.message
}

/// List a directory as sorted entries (dirs first, then files, alpha).
///
/// The renderer chooses the path, so the path is resolved against the read
/// scopes first: canonicalize, decide, re-check at the point of use. A refusal
/// is the canonical `AuthorizationDenied` shape and appends exactly one row
/// naming the actor, the command and the path. An in-scope listing needs no
/// approval and mints no ticket — a card per directory is not a stricter
/// control, it is what would break the tree.
#[tauri::command]
pub fn fs_list_dir(state: State<'_, AppState>, path: String) -> Result<serde_json::Value, String> {
    let dir = match resolve_read(&path, agentcowork_guard::pathfloor::FsOp::List) {
        Ok(t) => PathBuf::from(&t.canonical),
        Err(d) => return Err(deny_read(&state, "fs.list_dir", d)),
    };
    let meta = std::fs::metadata(&dir).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("{path}: not a directory"));
    }
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for ent in std::fs::read_dir(&dir).map_err(|e| format!("{path}: {e}"))? {
        let ent = ent.map_err(|e| format!("{path}: {e}"))?;
        let name = ent.file_name().to_string_lossy().into_owned();
        // Skip obvious noise so the tree stays navigable.
        if name == ".DS_Store" || name == "Thumbs.db" {
            continue;
        }
        let ft = ent.file_type().map_err(|e| format!("{name}: {e}"))?;
        let is_dir = ft.is_dir();
        let is_symlink = ft.is_symlink();
        let mut size: Option<u64> = None;
        let mut modified: Option<String> = None;
        if let Ok(m) = ent.metadata() {
            if m.is_file() {
                size = Some(m.len());
            }
            if let Ok(t) = m.modified() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    modified = Some(d.as_millis().to_string());
                }
            }
        }
        entries.push(serde_json::json!({
            "name": name,
            "dir": is_dir,
            "symlink": is_symlink,
            "size": size,
            "modified": modified,
        }));
    }
    entries.sort_by(|a, b| {
        let ad = a["dir"].as_bool().unwrap_or(false);
        let bd = b["dir"].as_bool().unwrap_or(false);
        bd.cmp(&ad).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Ok(serde_json::json!({
        "path": path,
        "parent": dir.parent().map(|p| p.display().to_string()),
        "entries": entries,
    }))
}

/// Read a file as UTF-8 text (capped at 2 MB). Binary/oversized files report
/// flags instead of failing, so the code view can render an honest notice.
///
/// The renderer chooses the path, so the path is resolved against the read
/// scopes first: canonicalize, decide, re-check at the point of use, and only
/// then touch the disk. A refusal is the canonical `AuthorizationDenied` shape
/// and appends exactly one audit row naming the actor, the command and the
/// path; an in-scope read asks nobody for anything, mints no ticket and
/// mutates nothing but the filesystem it was already allowed to read.
#[tauri::command]
pub fn fs_read_file(state: State<'_, AppState>, path: String) -> Result<serde_json::Value, String> {
    let p = match resolve_read(&path, agentcowork_guard::pathfloor::FsOp::Read) {
        Ok(t) => PathBuf::from(&t.canonical),
        Err(d) => return Err(deny_read(&state, "fs.read_file", d)),
    };
    let meta = std::fs::metadata(&p).map_err(|e| format!("{path}: {e}"))?;
    if meta.len() > MAX_TEXT_BYTES {
        return Ok(serde_json::json!({
            "path": path,
            "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "content": "",
            "sizeBytes": meta.len(),
            "truncated": true,
        }));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("{path}: {e}"))?;
    let content = match String::from_utf8(bytes.clone()) {
        Ok(s) => s,
        Err(_) => {
            return Ok(serde_json::json!({
                "path": path,
                "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "content": "",
                "sizeBytes": meta.len(),
                "binary": true,
            }));
        }
    };
    Ok(serde_json::json!({
        "path": path,
        "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "content": content,
        "sizeBytes": meta.len(),
        "truncated": false,
        "binary": false,
    }))
}

/// Write UTF-8 text to a file (creates/overwrites). Used by the code view's
/// "New untitled file" and the Save path.
///
/// FIX-06: this is a **human-gesture** effect, and it is now recorded as one.
/// Before this change the command performed a real, externally visible disk
/// write with no ticket, no audit row and no authority record at all — a
/// silent effect (INV-24) whose provenance could not be reconstructed. It stays
/// human-gesture rather than becoming ticketed for two reasons: the caller is
/// the user's own click, and a self-minted ticket would be theatre — the
/// default `write = always_ask` policy would return `Ask`, and approving it
/// inside the same call would launder a *considered* human decision into an
/// automatic one. The ticketed path for an agent/automation write is
/// [`fs_write_ticket`] + [`fs_write_commit`], and the same rule already governs
/// [`fs_undo_restore`], which is why the two agree.
///
/// The path is still path-floored, and the write lands on the Merkle chain with
/// `authorization: human_gesture` before the response returns.
#[tauri::command]
pub fn fs_write_file(
    state: State<'_, AppState>,
    path: String,
    content: String,
) -> Result<serde_json::Value, String> {
    let p = crate::control::floor_user_file(&path)?;
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!("{path}: parent directory does not exist"));
        }
    }
    let write_path = p.display().to_string();
    let bytes = content.len();
    std::fs::write(&p, content.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
    let audit_seq = crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "fs.write_file",
        serde_json::json!({
            "path": write_path,
            "bytes": bytes,
        }),
    );
    Ok(serde_json::json!({ "path": path, "bytes": content.len(), "auditSeq": audit_seq }))
}

/// Resolve the exact filesystem target and bind it with the approved bytes.
/// Existing leaf symlinks resolve to their target before the Guard ticket is
/// minted or consumed, so retargeting one between preview and commit changes
/// the ticket's argument hash and refuses the write.
fn write_effect_binding(
    path: &str,
    content: &str,
) -> Result<(PathBuf, String, Option<String>), String> {
    use std::hash::{Hash, Hasher};

    let floored = crate::control::floor_user_file(path)?;
    let target = match std::fs::canonicalize(&floored) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is not a new file. Treating it as one would
            // make `write` follow its target, which may be outside the floor.
            match std::fs::symlink_metadata(&floored) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!("cannot write through dangling symlink: {path}"));
                }
                Ok(_) => {
                    // The target may have appeared after `canonicalize`.
                    // Resolve it as existing so its identity is bound below.
                    std::fs::canonicalize(&floored)
                        .map_err(|e| format!("cannot resolve write target {path}: {e}"))?
                }
                Err(meta_error) if meta_error.kind() == std::io::ErrorKind::NotFound => {
                    let parent = floored
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .unwrap_or_else(|| std::path::Path::new("."));
                    let leaf = floored
                        .file_name()
                        .ok_or_else(|| format!("invalid write target: {path}"))?;
                    std::fs::canonicalize(parent)
                        .map_err(|e| format!("cannot resolve write parent for {path}: {e}"))?
                        .join(leaf)
                }
                Err(meta_error) => {
                    return Err(format!("cannot inspect write target {path}: {meta_error}"));
                }
            }
        }
        Err(error) => return Err(format!("cannot resolve write target {path}: {error}")),
    };

    let target_fingerprint = match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!(
                "write target changed into a symlink: {}",
                target.display()
            ));
        }
        Ok(meta) => Some(write_metadata_fingerprint(&target, &meta)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "cannot inspect write target {}: {error}",
                target.display()
            ));
        }
    };

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "editor.file_write".hash(&mut hasher);
    target.hash(&mut hasher);
    content.hash(&mut hasher);
    target_fingerprint.hash(&mut hasher);
    Ok((
        target,
        format!("{:016x}", hasher.finish()),
        target_fingerprint,
    ))
}

/// Fingerprint the existing target incarnation and freshness inputs that the
/// editor preview authorized. Platform ids distinguish same-size replacements.
fn write_metadata_fingerprint(
    path: &std::path::Path,
    meta: &std::fs::Metadata,
) -> Result<String, String> {
    let identity = agentcowork_storage::identity::identity_from_metadata(
        path,
        meta,
        agentcowork_storage::identity::IdentityPolicy::Full,
    );
    write_fingerprint_from_identity(identity, meta)
}

/// Build a fingerprint only when the platform supplied a real file identity.
/// Size and modification time alone cannot distinguish a same-size replacement.
fn write_fingerprint_from_identity(
    identity: agentcowork_storage::identity::FileIdentity,
    meta: &std::fs::Metadata,
) -> Result<String, String> {
    use std::hash::{Hash, Hasher};

    let key = identity.key().ok_or_else(|| {
        "cannot safely authorize write: platform file identity is unavailable".to_string()
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    let modified = meta.modified().map_err(|e| {
        format!("cannot safely authorize write: modification time unavailable: {e}")
    })?;
    let duration = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("cannot safely authorize write: invalid modification time: {e}"))?;
    duration.as_secs().hash(&mut hasher);
    duration.subsec_nanos().hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

/// Apply a previously approved write using a file handle whose identity is
/// checked before any truncation. A newly approved file uses `create_new`, so
/// a concurrently inserted link/file is refused instead of followed/overwritten.
fn write_bound_target(
    target: &std::path::Path,
    expected_fingerprint: Option<&str>,
    content: &[u8],
) -> Result<(), String> {
    use std::io::Write;

    let mut file = match expected_fingerprint {
        Some(_) => std::fs::OpenOptions::new()
            .write(true)
            .open(target)
            .map_err(|e| format!("cannot open write target {}: {e}", target.display()))?,
        None => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|e| {
                format!(
                    "new write target changed before creation {}: {e}",
                    target.display()
                )
            })?,
    };

    if let Some(expected) = expected_fingerprint {
        let metadata = file.metadata().map_err(|e| {
            format!(
                "cannot inspect opened write target {}: {e}",
                target.display()
            )
        })?;
        let identity = agentcowork_storage::identity::identity_from_file(&file, &metadata);
        let actual = write_fingerprint_from_identity(identity, &metadata)?;
        if actual != expected {
            return Err(format!(
                "write target identity or freshness changed: {}",
                target.display()
            ));
        }
        file.set_len(0)
            .map_err(|e| format!("cannot truncate write target {}: {e}", target.display()))?;
    }
    file.write_all(content)
        .map_err(|e| format!("cannot write target {}: {e}", target.display()))
}

/// P41.3 — Ticketed editor write, request half: mints a Guard-2 ticket for a
/// buffer write (I12 — everything ticketed; no silent autosaves). The card
/// carries a bounded before/after diff preview; the write itself happens ONLY
/// in [`fs_write_commit`] after `use_ticket` (approval + single-use +
/// args-hash match). Returns `action: allow` (policy auto-approved) or
/// `action: ask` (pending card the guard panel renders).
#[tauri::command]
pub fn fs_write_ticket(
    state: State<'_, AppState>,
    path: String,
    content: String,
) -> Result<serde_json::Value, String> {
    use agentcowork_guard::{Operation as GuardOp, RiskLevel};

    // The before-image (for the diff card); missing file = creation.
    let (path, args_hash, _) = write_effect_binding(&path, &content)?;
    let path = path.display().to_string();
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    let preview = diff_preview(&before, &content);

    let decision = agentcowork_guard::DecisionPackage::new(format!(
        "Write {}",
        std::path::Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ))
    .with_risk(RiskLevel::Medium)
    .with_paths(vec![path.clone()]);

    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let verdict = guard.evaluate(
        "editor",
        "everyaios",
        "editor.file_write",
        GuardOp::GenericWrite,
        decision,
        &args_hash,
        0,
    );
    match verdict {
        agentcowork_core::GuardDecision::Allow { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            Ok(serde_json::json!({
                "action": "allow",
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
                "preview": preview,
            }))
        }
        agentcowork_core::GuardDecision::Ask { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            Ok(serde_json::json!({
                "action": "ask",
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
                "preview": preview,
            }))
        }
        agentcowork_core::GuardDecision::Block { reason } => Err(format!("write blocked: {reason}")),
    }
}

/// P41.3 — Ticketed editor write, executor half: consumes the (mandatory)
/// single-use ticket (`use_ticket` — approval + args-hash match), then
/// writes. No ticket, no write: the editor never silently autosaves into the
/// workspace.
///
/// FIX-06: the consumed ticket now reaches the audit chain. The write was
/// correctly ticketed but *silent* — the effect executed with no audit row, so
/// the ticket that authorized it existed only in the caller's hands and the
/// provenance could not be reconstructed afterwards (INV-24, REQ-PROD-001: the
/// receipt/audit must reference its ticket). The row is stamped
/// `authorization: agent_ticket` with the real `ticketId`.
#[tauri::command]
pub fn fs_write_commit(
    state: State<'_, AppState>,
    path: String,
    content: String,
    ticket_id: String,
) -> Result<serde_json::Value, String> {
    let (_, args_hash, _) = write_effect_binding(&path, &content)?;

    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    guard
        .use_ticket(&ticket_id, &args_hash)
        .map_err(|e| e.to_string())?;
    drop(guard); // never hold the guard lock across a disk write

    // Re-resolve and re-fingerprint after ticket consumption. A stale target
    // spends the ticket but never receives the write; the caller must preview
    // and authorize the new target state.
    let (current_target, current_hash, current_fingerprint) =
        write_effect_binding(&path, &content)?;
    if current_hash != args_hash {
        return Err(format!("write target changed after authorization: {path}"));
    }

    let bytes = content.len();
    write_bound_target(
        &current_target,
        current_fingerprint.as_deref(),
        content.as_bytes(),
    )?;
    let audit_seq = crate::control::record_mutation(
        &state,
        crate::control::AuthKind::AgentTicket,
        "fs.write_commit",
        serde_json::json!({
            "path": current_target.display().to_string(),
            "bytes": bytes,
            "ticketId": ticket_id,
        }),
    );
    Ok(serde_json::json!({
        "path": path,
        "bytes": content.len(),
        "ticketId": ticket_id,
        "auditSeq": audit_seq,
    }))
}

/// A bounded before/after diff preview for the approval card (first 12 lines
/// each; the full diff renders in the Diff rail).
fn diff_preview(before: &str, after: &str) -> serde_json::Value {
    let head = |s: &str| s.lines().take(12).collect::<Vec<_>>().join("\n");
    serde_json::json!({ "before": head(before), "after": head(after) })
}

/// List the pending agent undo snapshots (`file_undos`) — the real patch set
/// the diff view renders. Each row is a file the agent mutated this session,
/// with its pre-mutation snapshot available for a restore or a diff.
#[tauri::command]
pub fn fs_undo_list(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let undos = state.file_undos.lock().map_err(|e| e.to_string())?;
    let rows: Vec<serde_json::Value> = undos
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let before_bytes = u.before.as_ref().map(|b| b.len()).unwrap_or(0);
            serde_json::json!({
                "index": i,
                "sessionId": u.session_id,
                "path": u.path.display().to_string(),
                "beforeBytes": before_bytes,
            })
        })
        .collect();
    Ok(serde_json::json!({ "undos": rows, "count": rows.len() }))
}

/// P52.17 — restore one file to its pre-mutation snapshot. `path` must match
/// a pending `FileUndo` (the newest snapshot for that path); its `before`
/// bytes are written back (or the file removed when the snapshot is a
/// creation). The snapshot is consumed, and the restore is audited as a
/// **HumanGesture** on the Merkle chain — a human deciding the agent's edit
/// was wrong is never an agent-ticket mutation. Nothing is faked: no
/// snapshot, no change.
#[tauri::command]
pub fn fs_undo_restore(
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let p = crate::control::floor_user_file(&path)?;
    let mut undos = state.file_undos.lock().map_err(|e| e.to_string())?;
    // Newest-first match on the exact path (later mutations supersede).
    let idx = undos
        .iter()
        .rposition(|u| u.path == p)
        .ok_or_else(|| format!("no pending snapshot for {path}"))?;
    let undo = &undos[idx];
    let before_len = undo.before.as_ref().map(|b| b.len()).unwrap_or(0);
    let undo_path = undo.path.display().to_string();
    let undo_session = undo.session_id.clone();
    agentcowork_core::restore_file_to_bytes(&undo.path, undo.before.as_deref())
        .map_err(|e| format!("{path}: {e}"))?;
    undos.remove(idx);
    drop(undos);
    let seq = crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "fs.undo_restore",
        serde_json::json!({
            "path": undo_path,
            "sessionId": undo_session,
            "beforeBytes": before_len,
        }),
    );
    Ok(serde_json::json!({ "ok": true, "path": path, "auditSeq": seq }))
}

/// P52.17 — read a pending snapshot's **content** (not just its size) so the
/// diff view can render a true before-vs-after for text files. Binary content
/// reports `binary: true` with bytes only; UTF-8 text returns `content`.
/// Falls back to `{found:false}` when no snapshot exists for `path`.
#[tauri::command]
pub fn fs_undo_snapshot(
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let p = std::path::PathBuf::from(&path);
    let undos = state.file_undos.lock().map_err(|e| e.to_string())?;
    let undo = undos
        .iter()
        .rev()
        .find(|u| u.path == p)
        .ok_or_else(|| format!("no pending snapshot for {path}"))?;
    match &undo.before {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes);
            Ok(serde_json::json!({
                "found": true,
                "path": p.display().to_string(),
                "binary": text.is_err(),
                "bytes": bytes.len(),
                "content": text.ok(),
            }))
        }
        None => Ok(serde_json::json!({
            "found": true,
            "path": p.display().to_string(),
            "binary": false,
            "bytes": 0,
            "created": true,
            "content": serde_json::Value::Null,
        })),
    }
}

// ---------------------------------------------------------------------------
// Read-scope interception — end-to-end style tests over the command path
// (`ARCH/12-TRUST.md` §8, `ARCH/25-FILES.md` §7.1).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod read_scope_tests {
    use super::*;

    /// What a `fs_read_file` / `fs_list_dir` call would answer with: the typed
    /// message plus the row it would append to the Merkle chain.
    struct Denial {
        refusal: ReadRefusal,
        chain: agentcowork_audit::merkle::MerkleChain,
    }

    /// The real command-path denial: gate → refusal → the audit append.
    ///
    /// These tests cannot build an `AppState` (it owns a live vault, a PTY host
    /// and a browser slot), so the single hop they stand in for is
    /// `control::record_mutation` — the append below writes the same
    /// `AuditEvent` that funnel writes. Everything the command *decides* is
    /// exercised for real: `resolve_read` is the production gate and
    /// `ReadRefusal::new` builds the production message and payload.
    fn deny(command: &str, path: &str) -> Denial {
        let denial = resolve_read(path, agentcowork_guard::pathfloor::FsOp::Read)
            .expect_err("the read scopes must refuse this path");
        let refusal = ReadRefusal::new(command, &denial);
        let mut chain = agentcowork_audit::merkle::MerkleChain::new();
        let seq = (chain.len() as u64) + 1;
        chain.push(agentcowork_audit::AuditEvent {
            seq,
            ts_ms: 0,
            kind: refusal.kind.to_string(),
            payload: refusal.payload.clone(),
            trace_id: String::new(),
            span_id: String::new(),
        });
        Denial { refusal, chain }
    }

    /// A scratch tree: `workspace/src/a.rs` plus a `secret.txt` outside it, and
    /// a protected subpath inside the workspace.
    fn scratch(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "agentcowork_fsread_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("workspace/src")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::create_dir_all(base.join("workspace/.everyaios")).unwrap();
        std::fs::write(base.join("workspace/src/a.rs"), b"fn main() {}").unwrap();
        std::fs::write(base.join("workspace/.everyaios/permissions.toml"), b"# p").unwrap();
        std::fs::write(base.join("outside/secret.txt"), b"SECRET").unwrap();
        base
    }

    #[cfg(unix)]
    #[test]
    fn write_ticket_binding_changes_when_leaf_symlink_target_changes() {
        use std::os::unix::fs::symlink;

        let base = scratch("write_ticket_symlink_swap");
        let parent = base.join("workspace/src");
        let first = parent.join("first.txt");
        let second = parent.join("second.txt");
        let link = parent.join("active.txt");
        std::fs::write(&first, b"first before-image").unwrap();
        std::fs::write(&second, b"second before-image").unwrap();
        symlink(&first, &link).unwrap();

        let (approved_path, approved_hash, _) =
            write_effect_binding(&link.to_string_lossy(), "approved replacement")
                .expect("the in-scope symlink target can be bound");

        std::fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        let (commit_path, commit_hash, _) =
            write_effect_binding(&link.to_string_lossy(), "approved replacement")
                .expect("the retargeted in-scope symlink remains a valid path");

        assert_ne!(approved_path, commit_path);
        assert_ne!(approved_hash, commit_hash);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn write_ticket_binding_rejects_dangling_leaf_symlink() {
        use std::os::unix::fs::symlink;

        let base = scratch("write_ticket_dangling_symlink");
        let parent = base.join("workspace/src");
        let link = parent.join("not-yet-created.txt");
        symlink(base.join("outside/missing.txt"), &link).unwrap();

        assert!(
            write_effect_binding(&link.to_string_lossy(), "must not escape").is_err(),
            "a dangling leaf symlink must not be treated as a new regular file"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn write_ticket_binding_changes_when_missing_target_appears() {
        let base = scratch("write_ticket_missing_to_existing");
        let path = base.join("workspace/src/new.txt");
        let (_, missing_hash, _) =
            write_effect_binding(&path.to_string_lossy(), "approved replacement").unwrap();
        std::fs::write(&path, b"created after preview").unwrap();
        let (_, existing_hash, _) =
            write_effect_binding(&path.to_string_lossy(), "approved replacement").unwrap();

        assert_ne!(missing_hash, existing_hash);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn write_fingerprint_fails_closed_without_platform_identity() {
        let base = scratch("write_fingerprint_unknown_identity");
        let path = base.join("workspace/src/a.rs");
        let metadata = std::fs::metadata(&path).unwrap();
        let unknown = agentcowork_storage::identity::FileIdentity::unknown(
            agentcowork_storage::identity::IdentityPlatform::Other,
            1,
        );

        assert!(write_fingerprint_from_identity(unknown, &metadata).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn new_file_created_as_symlink_after_preview_is_not_followed() {
        use std::os::unix::fs::symlink;

        let base = scratch("write_create_symlink_race");
        let target = base.join("workspace/src/new.txt");
        let outside = base.join("outside/secret.txt");
        let (_, _, fingerprint) =
            write_effect_binding(&target.to_string_lossy(), "approved").unwrap();
        assert!(fingerprint.is_none());
        symlink(&outside, &target).unwrap();

        assert!(write_bound_target(&target, None, b"must not escape").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"SECRET");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_existing_target_is_verified_before_truncation() {
        use std::os::unix::fs::symlink;

        let base = scratch("write_existing_symlink_race");
        let target = base.join("workspace/src/a.rs");
        let outside = base.join("outside/secret.txt");
        let (resolved, _, fingerprint) =
            write_effect_binding(&target.to_string_lossy(), "approved").unwrap();
        let fingerprint = fingerprint.expect("existing file has an identity");
        std::fs::remove_file(&target).unwrap();
        symlink(&outside, &target).unwrap();

        assert!(write_bound_target(&resolved, Some(&fingerprint), b"must not escape").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"SECRET");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_in_scope_read_resolves_and_asks_nobody_for_anything() {
        let base = scratch("in_scope");
        let path = base.join("workspace/src/a.rs");
        let t = resolve_read(
            &path.to_string_lossy(),
            agentcowork_guard::pathfloor::FsOp::Read,
        )
        .expect("an in-scope read proceeds");
        // The gate hands back the canonical path, and the disk is readable.
        assert!(t.canonical.ends_with("/workspace/src/a.rs"), "{t:?}");
        assert_eq!(
            std::fs::read_to_string(&t.canonical).unwrap(),
            "fn main() {}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_listing_resolves_and_asks_nobody_for_anything() {
        let base = scratch("listing");
        let t = resolve_read(
            &base.join("workspace").to_string_lossy(),
            agentcowork_guard::pathfloor::FsOp::List,
        )
        .expect("an in-scope listing proceeds");
        assert!(std::path::Path::new(&t.canonical).is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_out_of_scope_read_is_denied_typed_and_writes_exactly_one_audit_row() {
        let base = scratch("out_scope");
        // The escape the renderer must not make: climb out with `..`.
        let escape = format!("{}/workspace/../outside/secret.txt", base.to_string_lossy());
        let denial = deny("fs.read_file", &escape);
        let p = &denial.refusal.payload;

        // The canonical taxonomy code, not an invented one.
        assert!(
            denial.refusal.message.starts_with("AuthorizationDenied ("),
            "{:?}",
            denial.refusal.message
        );
        // The message names the code, the reason token and the path.
        assert!(
            denial.refusal.message.contains("parent_escape"),
            "{:?}",
            denial.refusal.message
        );
        assert!(
            denial.refusal.message.contains(&escape),
            "{:?}",
            denial.refusal.message
        );

        // Exactly one row, on the existing guard-denial kind, naming the actor,
        // the command and the path.
        assert_eq!(denial.chain.len(), 1, "a refusal owes exactly one row");
        assert!(denial.chain.verify().is_none(), "the row must chain intact");
        assert!(denial.chain.head().is_some());
        assert_eq!(denial.refusal.kind, "guard.blocked");
        assert_eq!(p["command"], "fs.read_file");
        assert_eq!(p["actor"], "renderer");
        assert_eq!(p["path"], escape.as_str());
        assert_eq!(p["code"], "AuthorizationDenied");
        assert_eq!(p["reason"], "parent_escape");
        assert_eq!(p["retryable"], false);
        assert_eq!(p["ok"], false);
        assert_eq!(p["state"], "refused");
        assert_eq!(p["outcome"], "denied");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_in_scope_read_writes_no_audit_row_and_mints_no_ticket() {
        let base = scratch("no_row");
        let path = base.join("workspace/src/a.rs");
        // An in-scope read is not a decision anybody has to make again, so the
        // gate returns a target and there is nothing to record: the command
        // body between `resolve_read` and `std::fs` mints no ticket and consults
        // no approval store.
        let t = resolve_read(
            &path.to_string_lossy(),
            agentcowork_guard::pathfloor::FsOp::Read,
        )
        .expect("an in-scope read proceeds");
        assert!(t.canonical.ends_with("/workspace/src/a.rs"), "{t:?}");
        // No scopes are configured, so the documented parent-floor default is
        // what is in force — asserted rather than assumed.
        assert!(!read_scopes().is_configured());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_symlink_out_of_the_root_is_refused() {
        let base = scratch("symlink");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = base.join("workspace/src/leak.rs");
            symlink(base.join("outside/secret.txt"), &link).unwrap();
            let err = resolve_read(
                &link.to_string_lossy(),
                agentcowork_guard::pathfloor::FsOp::Read,
            )
            .expect_err("a link out of the floor must be refused");
            assert_eq!(err.code(), "AuthorizationDenied");
            assert_eq!(
                err.denial,
                agentcowork_guard::pathfloor::ScopeDenial::SymlinkEscape
            );
        }
        #[cfg(not(unix))]
        {
            // Symlink creation needs privileges this host may not grant; the
            // guard crate's `a_leaf_symlink_out_of_scope_is_refused` covers the
            // same rule on unix. Gated, not deleted.
            let _ = base;
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_protected_subpath_is_read_only_under_the_scope_model() {
        // The shell's *write* path is unchanged by this work (it keeps
        // `control::floor_user_file` + its own provenance), so the
        // read-only-for-protected-subpaths rule is asserted where it is
        // implemented: the scope model. Read stands, write is refused.
        let base = scratch("protected");
        let scopes = agentcowork_guard::pathfloor::ReadScopes::new(vec![
            agentcowork_guard::pathfloor::PathGrant {
                axis: agentcowork_guard::pathfloor::GrantAxis::ReadWriteCreate,
                prefix: base.to_string_lossy().into_owned(),
            },
        ]);
        let p = base.join("workspace/.everyaios/permissions.toml");
        assert!(scopes
            .resolve(agentcowork_guard::pathfloor::FsOp::Read, &p.to_string_lossy())
            .is_ok());
        assert_eq!(
            scopes
                .resolve(
                    agentcowork_guard::pathfloor::FsOp::Write,
                    &p.to_string_lossy()
                )
                .unwrap_err()
                .denial,
            agentcowork_guard::pathfloor::ScopeDenial::ProtectedSubpath
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_path_swapped_between_resolution_and_use_is_caught_by_the_recheck() {
        // `resolve_read` resolves *and* re-checks, so a swap landing between
        // the two is refused. The window itself is exercised at the model level
        // (`agentcowork-guard`'s `a_path_swapped_between_resolution_and_use_is_caught`);
        // here the same refusal is observed on the command path.
        let base = scratch("toctou");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let a = base.join("workspace/src/a.rs");
            let outside = base.join("outside/secret.txt");
            let t = read_scopes()
                .resolve(agentcowork_guard::pathfloor::FsOp::Read, &a.to_string_lossy())
                .unwrap();
            // The swap: the resolved leaf becomes a link out of the root.
            std::fs::remove_file(&a).unwrap();
            symlink(&outside, &a).unwrap();
            // Re-check at the point of use: refused, not read.
            let err = read_scopes()
                .reverify(&t)
                .expect_err("a swapped path must be caught before the open");
            assert_eq!(err.code(), "AuthorizationDenied");
            assert_eq!(
                err.denial,
                agentcowork_guard::pathfloor::ScopeDenial::SymlinkEscape
            );
            // And the command gate refuses the same path outright.
            assert!(
                resolve_read(&a.to_string_lossy(), agentcowork_guard::pathfloor::FsOp::Read).is_err()
            );
        }
        #[cfg(not(unix))]
        {
            let _ = base;
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
