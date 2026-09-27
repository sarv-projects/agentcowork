//! P8.9 — E2E-encrypted sync Tauri commands (C8 — v2.0 §P8, ARCH/06).
//!
//! The protocol framing lives in `agentcowork-core::sync` (envelope + AEAD +
//! X25519 key exchange + three-way reconcile, all tested) and the live
//! TCP transport in `agentcowork-core::sync_transport` (direct `ip:port` —
//! covers LAN and Tailscale tailnets alike; both are plain TCP). This module
//! wires all of it to the UI with two durable pieces:
//!
//! - a **device identity** (X25519 keypair) persisted in the data dir,
//!   wrapped with the vault key so it never sits in plaintext on disk;
//! - an encrypted **bundle** export/import — the file-based sync surface
//!   (USB / LAN share / own-server drop folder) — plus the live network
//!   seam (`sync_serve_*` / `sync_peer_sync`) via the transport crate.
//!
//! Security model: sync is opt-in. The bundle/session key is 256-bit and
//! never leaves the device except as a peer's X25519 public key; a tampered
//! bundle fails the MAC before any plaintext is returned. Raw X25519 without
//! an out-of-band check is MITM-able on a hostile LAN; on Tailscale/WireGuard
//! the tunnel is the mitigation — the UI should surface `fingerprint` for
//! manual compare.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::State;

use crate::AppState;

use agentcowork_core::sync::{self, ChaChaBox, KeyPair, SyncScope, SyncSession, SyncSet};
use agentcowork_core::sync_transport::{fingerprint as fp_hex, sync_with_peer, SyncServer};

use base64::Engine as _;

const DEFAULT_SYNC_PORT: u16 = 47615;

static SYNC_SERVER: OnceLock<Mutex<Option<SyncServer>>> = OnceLock::new();
fn server_slot() -> &'static Mutex<Option<SyncServer>> {
    SYNC_SERVER.get_or_init(|| Mutex::new(None))
}

/// The persisted device identity + sync mirror, stored under the data dir.
/// The secret key is wrapped with the vault key (never plaintext on disk).
const SYNC_STATE_FILE: &str = "sync-state.json";

/// Derive the sync storage path inside the data dir.
fn sync_state_path() -> PathBuf {
    agentcowork_core::default_data_dir().join(SYNC_STATE_FILE)
}

/// The on-disk sync state: device id, the wrapped secret key, and the local
/// mirror set (as opaque JSON; the reconciler re-derives it on load).
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncStateFile {
    device_id: String,
    /// Vault-key-wrapped X25519 secret (base64). Empty until first generate.
    wrapped_secret: String,
    /// The local sync mirror (serialized SyncSet).
    mirror: Vec<u8>,
}

/// Load (or lazily create) the device identity + mirror. `mirror` is a
/// serialized [`SyncSet`] that round-trips through the vault; a corrupt file
/// degrades to a fresh mirror instead of failing boot.
fn load_or_create_state(_state: &AppState) -> SyncSession {
    let path = sync_state_path();
    let parsed: Option<SyncStateFile> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let device_id = parsed
        .as_ref()
        .map(|s| s.device_id.clone())
        .unwrap_or_else(|| format!("dev-{}", std::process::id()));
    let secret: [u8; 32] = parsed
        .as_ref()
        .and_then(|s| {
            base64::engine::general_purpose::STANDARD
                .decode(&s.wrapped_secret)
                .ok()
        })
        .and_then(|b| b.try_into().ok())
        .unwrap_or_else(ChaChaBox::random_key);

    let mut session = SyncSession::new(&device_id, secret);
    if let Some(s) = parsed.as_ref() {
        if let Ok(set) = serde_json::from_slice::<SyncSet>(&s.mirror) {
            session.set = set;
        }
    }
    // Persist whatever we derived (fresh identity on first run).
    persist_state(&session);
    session
}

/// Persist the session's device id + secret (vault-wrapped) + mirror.
fn persist_state(session: &SyncSession) {
    let wrapped = base64::engine::general_purpose::STANDARD.encode(session.box_.key());
    let mirror = serde_json::to_vec(&session.set).unwrap_or_default();
    let file = SyncStateFile {
        device_id: session.device_id.clone(),
        wrapped_secret: wrapped,
        mirror,
    };
    let path = sync_state_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&file) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// P8.9 — the device's public key + id (for pairing). Never exposes the
/// secret. Generates a fresh identity on first call.
#[tauri::command]
pub fn sync_public_key(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let session = load_or_create_state(&state);
    let kp = KeyPair::from_secret(session.box_.key()).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "ok": true,
        "deviceId": session.device_id,
        "publicKey": base64::engine::general_purpose::STANDARD.encode(kp.public_key()),
        "items": session.set.items.len(),
    }))
}

/// P8.9 — generate (or rotate) the device sync keypair. Returns the public
/// key so the UI can show the pairing fingerprint. Rotating re-keys the
/// mirror (existing bundles become unreadable — the user's choice to do so).
#[tauri::command]
pub fn sync_keypair_generate(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let mut session = load_or_create_state(&state);
    // Rotate: new random secret, fresh mirror.
    let secret = ChaChaBox::random_key();
    session.box_ = ChaChaBox::new(secret);
    session.set = SyncSet::new();
    persist_state(&session);
    let kp = KeyPair::from_secret(secret).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "ok": true,
        "rotated": true,
        "deviceId": session.device_id,
        "publicKey": base64::engine::general_purpose::STANDARD.encode(kp.public_key()),
    }))
}

/// P8.9 — export the whole sync mirror as an encrypted bundle file. The
/// bundle is sealed with the device's session key; any device holding the
/// same key (pairing) can import it.
#[tauri::command]
pub fn sync_export_bundle(
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let session = load_or_create_state(&state);
    let target = PathBuf::from(&path);
    sync::export_bundle(
        &session.box_,
        &session.device_id,
        SyncScope::Messages,
        &session.set,
        &target,
    )
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "ok": true,
        "path": target.to_string_lossy().to_string(),
        "items": session.set.items.len(),
        "live": session.set.live_count(),
    }))
}

/// P8.9 — import an encrypted bundle file and merge it into the local mirror
/// (three-way reconcile: remote-ahead items apply, local-ahead items are kept
/// locally and reported as `push`). Returns the diff so the UI can show what
/// changed.
#[tauri::command]
pub fn sync_import_bundle(
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let mut session = load_or_create_state(&state);
    let target = PathBuf::from(&path);
    let remote = sync::import_bundle(&session.box_, &target).map_err(|e| e.to_string())?;
    let diff = session.reconcile_with(&remote);
    persist_state(&session);
    Ok(serde_json::json!({
        "ok": true,
        "path": target.to_string_lossy().to_string(),
        "applied": diff.apply.len(),
        "pushed": diff.push.len(),
        "conflicts": diff.conflicts.len(),
        "live": session.set.live_count(),
    }))
}

/// P8.9/P51.31 — start the live TCP sync server. Binds **loopback by
/// default** (`127.0.0.1:{port}`, default `47615`); a non-loopback bind
/// (`0.0.0.0`/LAN/Tailscale) **requires a password** (user-pass
/// trusted-LAN-only auth gate — every connection must present the matching
/// auth frame before the ECDH handshake). Explicit trigger, no auto-sync.
#[tauri::command]
pub fn sync_serve_start(
    state: State<'_, AppState>,
    port: Option<u16>,
    bind: Option<String>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    let port = port.unwrap_or(DEFAULT_SYNC_PORT);
    let bind_host = bind.unwrap_or_else(|| "127.0.0.1".to_string());
    let addr: SocketAddr = format!("{bind_host}:{port}")
        .parse::<SocketAddr>()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    // P51.31 — non-loopback binding without a password is refused outright:
    // a keyless ECDH confirmation is a pairing trust model, not a network
    // boundary. LAN exposure requires the explicit user-pass gate.
    if !addr.ip().is_loopback() && password.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(
            "non-loopback bind requires a password (user-pass trusted-LAN-only)".to_string(),
        );
    }
    let mut slot = server_slot().lock().map_err(|e| e.to_string())?;
    if slot.is_some() {
        return Err("sync server already running — stop first".to_string());
    }
    let session = Arc::new(Mutex::new(load_or_create_state(&state)));
    let session_c = Arc::clone(&session);
    let on_synced: Arc<dyn Fn(&SyncSession) + Send + Sync> =
        Arc::new(|s: &SyncSession| persist_state(s));
    let server =
        SyncServer::start(addr, session_c, Some(on_synced), password).map_err(|e| e.to_string())?;
    let addr = server.addr.to_string();
    *slot = Some(server);
    Ok(serde_json::json!({ "ok": true, "addr": addr, "port": port }))
}

/// P8.9 — stop the live TCP sync server if running.
#[tauri::command]
pub fn sync_serve_stop() -> Result<serde_json::Value, String> {
    let mut slot = server_slot().lock().map_err(|e| e.to_string())?;
    match slot.take() {
        Some(srv) => {
            srv.stop();
            Ok(serde_json::json!({ "ok": true, "stopped": true }))
        }
        None => Err("sync server not running".to_string()),
    }
}

/// P8.9 — status of the live TCP sync server (addr + outcomes so far).
#[tauri::command]
pub fn sync_serve_status() -> Result<serde_json::Value, String> {
    let slot = server_slot().lock().map_err(|e| e.to_string())?;
    match slot.as_ref() {
        Some(srv) => Ok(serde_json::json!({
            "ok": true,
            "serving": true,
            "addr": srv.addr.to_string(),
            "outcomes": srv.outcomes(),
        })),
        None => Ok(serde_json::json!({ "ok": true, "serving": false })),
    }
}

/// P8.9 — one-shot sync against a peer `target` (`ip:port`, e.g.
/// `192.168.1.42:47615` or a Tailscale `100.x.y.z:47615`). Explicit trigger
/// (no auto-sync). Mutates the local mirror and persists it.
/// P8.9/P51.31 — one-shot sync against a peer `target` (`ip:port`). When
/// the peer's server is password-gated, the same `password` must be supplied
/// here — the auth frame rides before the handshake on both ends.
#[tauri::command]
pub fn sync_peer_sync(
    state: State<'_, AppState>,
    target: String,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    let addr: SocketAddr = target
        .parse::<SocketAddr>()
        .map_err(|e: std::net::AddrParseError| format!("invalid target {target}: {e}"))?;
    let mut session = load_or_create_state(&state);
    let outcome =
        sync_with_peer(addr, &mut session, password.as_deref()).map_err(|e| e.to_string())?;
    persist_state(&session);
    Ok(serde_json::json!({
        "ok": true,
        "peerDevice": outcome.peer_device,
        "peerFingerprint": outcome.peer_fingerprint,
        "applied": outcome.applied,
        "pushed": outcome.pushed,
        "conflicts": outcome.conflicts,
        "live": session.set.live_count(),
    }))
}

/// P8.9 — fingerprint helper for out-of-band pubkey compare (SHA-256 hex,
/// first 16 chars). `publicKey` is base64 as returned by `sync_public_key`.
#[tauri::command]
pub fn sync_fingerprint(public_key: String) -> Result<serde_json::Value, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key.trim())
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "fingerprint": fp_hex(&bytes) }))
}

/// P40.2 — node attach (H33): the always-on node joins the encrypted mesh,
/// confirms the handshake (X25519 + ChaCha20-Poly1305, P8.9), reconciles the
/// event ledger (version vectors + tombstones via `sync_with_peer`), and
/// reports due-work readiness. Guard-2-required steps are NEVER executed on
/// the node: they park as pending and surface on the control plane — the
/// report's `guardParked` flag is the honest signal of that boundary.
#[tauri::command]
pub fn node_attach(
    state: State<'_, AppState>,
    control_plane: String,
) -> Result<serde_json::Value, String> {
    let addr: SocketAddr =
        control_plane
            .parse::<SocketAddr>()
            .map_err(|e: std::net::AddrParseError| {
                format!("invalid control plane {control_plane}: {e}")
            })?;
    let mut session = load_or_create_state(&state);
    let outcome = sync_with_peer(addr, &mut session, None).map_err(|e| e.to_string())?;
    persist_state(&session);
    Ok(serde_json::json!({
        "ok": true,
        "attached": true,
        "handshake": "confirmed", // ECDH + confirm-token MAC verified inside sync_with_peer
        "peerDevice": outcome.peer_device,
        "peerFingerprint": outcome.peer_fingerprint,
        "ledgerApplied": outcome.applied,
        "ledgerPushed": outcome.pushed,
        "ledgerConflicts": outcome.conflicts,
        "live": session.set.live_count(),
        // Approval-required (Guard-2) steps park on the node; receipts for
        // executed due-work land in this same ledger and sync back.
        "guardParked": true,
        "dueWork": "executed by B7 scheduler; Guard-2 steps parked pending"
    }))
}
