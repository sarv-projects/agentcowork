//! P4.8 (D9–D12, G7) — storage intelligence Tauri commands. These expose the
//! `agentcowork-storage` crate (scan → arena → treemap/large-files/duplicates/
//! cleanup proposals) to the UI. Every destructive action is proposal-only:
//! the crate emits `CleanupAction` plans that Guard-2 must ticket and execute
//! (recycle-bin-aware) — the dashboard never deletes on its own.
//!
//! J16 battery gating: heavy scans (the parallel walker + BLAKE3 dedup) are
//! suppressed while the device is on battery. The lighter reads (health,
//! cached treemap) stay available. `storage_battery` is driven by the same
//! OS power event the scheduler uses.
//!
//! FIX-10 / `REQ-FILES-001`: every response that carries a size-derived number
//! also carries the identity evidence behind it (`identityUnknown`,
//! `reclaimableBytesBacked`, per-group `identityBacked`). The walker used to
//! answer `dev = 0, ino = 0` on non-Unix hosts, which made every Windows
//! duplicate look like a hardlink twin of the first one and reported
//! `reclaimableBytes: 0` — a fabricated number the UI could not tell apart from
//! a real one.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::State;

use crate::AppState;

/// Resolve a user-supplied path (or the workspace default) into a scan root.
fn resolve_root(path: Option<&str>) -> PathBuf {
    match path {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => agentcowork_core::default_data_dir().join("workspace"),
    }
}

/// D12 — drive-health check for the workspace (or a given path). Never gated:
/// this is a cheap `sysinfo` read, always available.
#[tauri::command]
pub fn storage_health(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    let root = resolve_root(path.as_deref());
    let status = agentcowork_storage::check_health(&root, 90.0).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "mount": status.drive.mount,
        "totalBytes": status.drive.total,
        "availableBytes": status.drive.available,
        "usedBytes": status.used_bytes,
        "usedPct": status.used_pct,
        "thresholdPct": status.threshold_pct,
        "overThreshold": status.over_threshold,
        "battery": state.battery.load(Ordering::Relaxed),
    }))
}

/// D9/D10 — scan a directory into the u32-indexed arena and return the
/// squarified treemap (top-level dirs by aggregate size) + file count. The
/// heavy walk is suppressed on battery (returns an empty-but-honest result so
/// the UI can show "scan deferred while on battery").
#[tauri::command]
pub fn storage_scan(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    let root = resolve_root(path.as_deref());
    if state.battery.load(Ordering::Relaxed) {
        return Ok(serde_json::json!({
            "deferred": true,
            "reason": "heavy scan suppressed while on battery (J16)",
            "files": 0,
            "treemap": [],
        }));
    }
    let opts = scan_opts();
    let records = agentcowork_storage::scan(&root, &opts).map_err(|e| e.to_string())?;
    let files = records.iter().filter(|r| !r.is_dir).count();
    // FIX-10: count before the arena takes ownership of `records`; the response
    // reports how many records the OS gave no identity for (never a zero-filled id).
    let identity_unknown = records.iter().filter(|r| r.identity.is_unknown()).count();
    let arena = agentcowork_storage::build_arena(records, &root);
    let root_id = arena.root().unwrap_or(0);
    let rects = agentcowork_storage::treemap_for_dir(&arena, root_id);
    let treemap: Vec<serde_json::Value> = rects
        .into_iter()
        .filter_map(|r| {
            arena.get(r.id).map(|n| {
                serde_json::json!({
                    "id": r.id,
                    "name": n.name,
                    "path": n.path,
                    "size": n.size,
                    "isDir": n.is_dir,
                    "w": r.w,
                    "h": r.h,
                    "color": agentcowork_storage::color_for(&n.name),
                })
            })
        })
        .collect();
    Ok(serde_json::json!({
        "deferred": false,
        "files": files,
        "root": root.to_string_lossy(),
        "treemap": treemap,
        // FIX-10: how many records the OS would not give an identity for.
        // Surfaced as a count (never a silent zero-filled id) so the UI can
        // stay honest about hardlink/reclaim numbers derived from it.
        "identityUnknown": identity_unknown,
        "identityPlatform": agentcowork_storage::IdentityPlatform::current().as_str(),
    }))
}

/// D11 — largest files by size (top-N). Heavy walk; battery-gated like scan.
#[tauri::command]
pub fn storage_large_files(
    state: State<'_, AppState>,
    path: Option<String>,
    top_n: Option<usize>,
) -> Result<serde_json::Value, String> {
    let root = resolve_root(path.as_deref());
    if state.battery.load(Ordering::Relaxed) {
        return Ok(serde_json::json!({ "deferred": true, "files": [] }));
    }
    let records = agentcowork_storage::scan(&root, &scan_opts()).map_err(|e| e.to_string())?;
    let arena = agentcowork_storage::build_arena(records, &root);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let files = agentcowork_storage::find_large_files(
        &arena,
        &agentcowork_storage::FinderOptions {
            top_n: top_n.unwrap_or(50),
            ..Default::default()
        },
        agentcowork_storage::SortBy::SizeDesc,
        now,
    );
    let listed: Vec<serde_json::Value> = files
        .into_iter()
        .map(|n| {
            serde_json::json!({ "name": n.name, "path": n.path, "size": n.size, "isDir": n.is_dir })
        })
        .collect();
    Ok(serde_json::json!({ "deferred": false, "files": listed }))
}

/// D10 — duplicate-file groups (7-stage hash). The heaviest op (BLAKE3);
/// battery-gated like scan.
///
/// FIX-10 / `REQ-FILES-001`: candidates are built from the scan record's
/// resolved platform identity, so hardlink grouping is real on every host and
/// an identity the OS would not give us is reported as *unknown* instead of a
/// zero that collapsed every file into one "copy" (`wastedBytes: 0`). The
/// response carries `identityUnknown` and each group carries `identityBacked`
/// so the UI cannot present an upper bound as a measurement.
#[tauri::command]
pub fn storage_duplicates(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    let root = resolve_root(path.as_deref());
    if state.battery.load(Ordering::Relaxed) {
        return Ok(serde_json::json!({ "deferred": true, "groups": [] }));
    }
    let records = agentcowork_storage::scan(&root, &scan_opts()).map_err(|e| e.to_string())?;
    let identity_unknown = records.iter().filter(|r| r.identity.is_unknown()).count();
    let cands: Vec<agentcowork_storage::DupCandidate> = records
        .iter()
        .filter(|r| !r.is_dir)
        .map(agentcowork_storage::DupCandidate::from_record)
        .collect();
    let groups =
        agentcowork_storage::find_duplicates(&cands, &agentcowork_storage::DedupOptions::default())
            .map_err(|e| e.to_string())?;
    let reclaimable: u64 = groups.iter().map(|g| g.wasted_bytes).sum();
    let reclaimable_backed: u64 = groups
        .iter()
        .filter(|g| g.identity_backed)
        .map(|g| g.wasted_bytes)
        .sum();
    let listed: Vec<serde_json::Value> = groups
        .iter()
        .map(|g| {
            serde_json::json!({
                "size": g.size,
                "wastedBytes": g.wasted_bytes,
                "copies": g.files.len(),
                "hardlinkGroups": g.hardlink_groups,
                "reflinkEligible": g.reflink_eligible,
                "identityBacked": g.identity_backed,
                "files": g.files.iter().map(|f| f.path.to_string_lossy().to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "deferred": false,
        "groups": listed,
        "reclaimableBytes": reclaimable,
        // Bytes we can actually prove; the remainder rests on an identity the
        // OS refused to give us and is an upper bound only.
        "reclaimableBytesBacked": reclaimable_backed,
        "identityUnknown": identity_unknown,
        "identityPlatform": agentcowork_storage::IdentityPlatform::current().as_str(),
    }))
}

/// D12 — cleanup *proposals* (never executed here). Returns Guard-2 decision
/// packages; the actual deletion must go through the ticket model.
#[tauri::command]
pub fn storage_cleanup_proposals(
    state: State<'_, AppState>,
    path: Option<String>,
    top_n: Option<usize>,
) -> Result<serde_json::Value, String> {
    let root = resolve_root(path.as_deref());
    if state.battery.load(Ordering::Relaxed) {
        return Ok(serde_json::json!({ "deferred": true, "proposals": [] }));
    }
    let records = agentcowork_storage::scan(&root, &scan_opts()).map_err(|e| e.to_string())?;
    // One scan feeds both proposal sources, so the duplicate pass inherits the
    // same identity evidence the arena does.
    let dup_groups = {
        let cands: Vec<agentcowork_storage::DupCandidate> = records
            .iter()
            .filter(|r| !r.is_dir)
            .map(agentcowork_storage::DupCandidate::from_record)
            .collect();
        agentcowork_storage::find_duplicates(&cands, &agentcowork_storage::DedupOptions::default())
            .map_err(|e| e.to_string())?
    };
    let arena = agentcowork_storage::build_arena(records, &root);
    let mut proposals = agentcowork_storage::propose_large_files_cleanup(&arena, top_n.unwrap_or(10));
    proposals.extend(agentcowork_storage::propose_duplicate_cleanup(&dup_groups));
    let listed: Vec<serde_json::Value> = proposals.iter().map(|p| p.decision_package()).collect();
    Ok(serde_json::json!({
        "deferred": false,
        "proposals": listed,
        "unbackedProposals": proposals.iter().filter(|p| !p.identity_backed).count(),
    }))
}

/// J16 — set the battery flag (driven by the OS power event; `true` = on
/// battery). Heavy scans read this and defer.
#[tauri::command]
pub fn storage_battery(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    state.battery.store(on, Ordering::Relaxed);
    Ok(())
}

/// Shared scan options for the heavy (battery-gated) commands.
fn scan_opts() -> agentcowork_storage::ScanOptions {
    agentcowork_storage::ScanOptions {
        threads: 1,
        follow_symlinks: false,
        same_filesystem: true,
        min_file_size: 0,
        skip_hidden: true,
    }
}
