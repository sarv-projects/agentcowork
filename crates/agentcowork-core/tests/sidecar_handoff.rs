//! Packaged-boot verification: the supervisor spawns the **real** coordinator
//! sidecar and hands `stdin`/`stdout` to the shell over the link channel.
//!
//! This is the exact path the desktop shell takes at boot
//! (`pre_spawn_coordinator` → `start_supervisor_with_link` → `connect_chat_relay`),
//! and the thing a blank/offline shell hides: if the handoff never lands, the
//! chat relay is never built, `runtime_status.sidecar` stays `false`, and the
//! UI reports "Coordinator offline — live agent work is unavailable" even
//! though the child is alive and healthy.
//!
//! Live (needs the ~97 MB compiled sidecar), so `#[ignore]` by default:
//!
//! ```text
//! AGENTCOWORK_COORDINATOR_BIN=desktop_app/src-tauri/bin/coordinator \
//!   cargo test -p agentcowork-core --test sidecar_handoff -- --ignored
//! ```
//!
//! It also skips (rather than fails) when no sidecar binary is present, so a
//! checkout without `bun run build` in `packages/coordinator` stays green.

use std::path::PathBuf;
use std::time::Duration;

use agentcowork_ipc::frame;

/// `AGENTCOWORK_COORDINATOR_BIN` wins (the shell's own override), else the
/// workspace build output next to the shell crate.
fn sidecar_bin() -> Option<PathBuf> {
    if let Some(p) = agentcowork_types::env_compat::get("COORDINATOR_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in [
        "../../src-tauri/bin/coordinator",
        "../../packages/coordinator/dist/coordinator",
    ] {
        let p = here.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[test]
#[ignore = "live: spawns the compiled coordinator sidecar"]
fn supervisor_hands_off_the_live_sidecar_link() {
    let Some(bin) = sidecar_bin() else {
        eprintln!("skipped: no coordinator sidecar binary (build packages/coordinator first)");
        return;
    };

    let (mut supervisor, link_rx) = agentcowork_core::start_supervisor_with_link(bin);
    // Lifecycle thread, as the shell runs it. Must be alive or the supervisor
    // never reaches `spawn()` and no handoff is ever sent.
    std::thread::spawn(move || {
        if let Err(e) = supervisor.wait_or_restart() {
            eprintln!("[sidecar_handoff] supervisor ended: {e}");
        }
    });

    let (stdin, mut stdout) = link_rx.recv_timeout(Duration::from_secs(20)).expect(
        "the supervisor must hand stdin/stdout to the shell — without this the \
             chat relay is never built and the UI shows 'Coordinator offline'",
    );

    // The coordinator announces `session/ready` unprompted at boot (it also
    // issues a `usage/recent` request first), so a decoded frame proves the
    // link is live end-to-end — not merely that a child process exists.
    let mut saw_ready = false;
    let mut frames = 0usize;
    while frames < 16 {
        match frame::decode(&mut stdout) {
            Ok(Some(payload)) => {
                frames += 1;
                let v: serde_json::Value = serde_json::from_slice(&payload).unwrap_or_default();
                if v.get("method").and_then(|m| m.as_str()) == Some("session/ready") {
                    assert_eq!(
                        v.get("params")
                            .and_then(|p| p.get("serverName"))
                            .and_then(|n| n.as_str()),
                        Some("@agentcowork/coordinator"),
                        "session/ready must identify the coordinator: {v}"
                    );
                    saw_ready = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("[sidecar_handoff] decode error after {frames} frames: {e:?}");
                break;
            }
        }
    }
    drop(stdin);
    assert!(
        saw_ready,
        "expected `session/ready` on the handed-off link within {frames} frames"
    );
}
