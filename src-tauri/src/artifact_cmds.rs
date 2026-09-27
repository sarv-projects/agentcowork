//! P15-H29 — local dashboard artifact preview server (Tauri bridge).
//!
//! Wires `agentcowork_script::artifact::serve` (the loopback-only,
//! path-floored, GET-only preview transport) to the UI's `startArtifactServer`
//! / `stopArtifactServer` (`ui/src/lib/artifact.ts`). This is the missing
//! command bridge: the transport was fully built + tested in
//! `agentcowork-script`, and the UI already invokes `artifact_serve` /
//! `artifact_stop` — this module is the wire between them.
//!
//! Safety ("Sidecar proposes, Rust disposes"): serving a workspace folder on
//! a loopback socket is a Guard-2-ticketed effect. `artifact_serve` mints and
//! consumes a single-use ticket in one call (the UI's contract is a bare
//! `serve → port`, so there is no separate approve card round-trip here); a
//! policy `Ask`/`Block` surfaces as an honest error instead of silently
//! serving. The transport itself is loopback-only by construction (binds
//! `127.0.0.1`), GET-only, and path-floored — it can never be reached from the
//! network or read outside the served folder.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use tauri::State;

use crate::AppState;

/// Deterministic args hash for the artifact-serve ticket (must match between
/// evaluate + use_ticket within the single call).
fn serve_args_hash(workspace: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "artifact.serve".hash(&mut h);
    workspace.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Serve a guarded workspace folder on an ephemeral `127.0.0.1:<port>` and
/// return the port. Guard-2-ticketed: the effect is evaluated + the ticket is
/// consumed in this one call (the UI expects `serve → port`). `Ask`/`Block`
/// verdicts return an error the UI surfaces honestly (fail-closed — the live
/// bridge never substitutes demo content; preview fixtures exist only in
/// plain-browser runs outside the shell).
#[tauri::command]
pub fn artifact_serve(state: State<'_, AppState>, workspace: String) -> Result<u16, String> {
    use agentcowork_guard::{Operation as GuardOp, RiskLevel};

    // Resolve + validate the workspace up front so a bad path fails before we
    // ever mint a ticket.
    let dir = PathBuf::from(&workspace);
    if !dir.is_dir() {
        return Err(format!("{workspace}: not a directory"));
    }
    let canonical = std::fs::canonicalize(&dir)
        .map_err(|e| format!("{workspace}: {e}"))?
        .display()
        .to_string();

    let decision = agentcowork_guard::DecisionPackage::new(format!(
        "Serve artifact preview from {}",
        std::path::Path::new(&canonical)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ))
    .with_risk(RiskLevel::Medium)
    .with_paths(vec![canonical.clone()]);

    let args_hash = serve_args_hash(&canonical);

    // Evaluate + consume the ticket while holding the guard lock; never hold
    // it across the socket bind.
    {
        let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
        let verdict = guard.evaluate(
            "artifact",
            "everyaios",
            "artifact.serve",
            GuardOp::GenericWrite,
            decision,
            &args_hash,
            0,
        );
        match verdict {
            agentcowork_core::GuardDecision::Allow { ticket_id } => {
                guard
                    .use_ticket(&ticket_id, &args_hash)
                    .map_err(|e| e.to_string())?;
            }
            agentcowork_core::GuardDecision::Ask { .. } => {
                return Err(
                    "artifact preview needs approval — approve it in the Guard window, then retry"
                        .to_string(),
                );
            }
            agentcowork_core::GuardDecision::Block { reason } => {
                return Err(format!("artifact preview blocked: {reason}"));
            }
        }
    }

    let handle = agentcowork_script::artifact::serve(std::path::Path::new(&canonical))
        .map_err(|e| e.to_string())?;
    let port = handle.port();
    state
        .artifacts
        .lock()
        .map_err(|e| e.to_string())?
        .insert(port, handle);
    Ok(port)
}

/// Stop a running artifact preview server by port (idempotent — an unknown
/// port is a no-op). No ticket: tearing down a loopback preview you started is
/// a trusted teardown, mirroring `browser_stop` / `shell_kill`.
#[tauri::command]
pub fn artifact_stop(state: State<'_, AppState>, port: u16) -> Result<(), String> {
    let handle = state
        .artifacts
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&port);
    if let Some(handle) = handle {
        handle.stop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_hash_is_stable_and_path_sensitive() {
        assert_eq!(serve_args_hash("/w/a"), serve_args_hash("/w/a"));
        assert_ne!(serve_args_hash("/w/a"), serve_args_hash("/w/b"));
    }
}
