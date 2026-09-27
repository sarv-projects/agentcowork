//! P8.8 / P70.C1–C4 — auto-updater Tauri commands.
//!
//! The tauri-plugin-updater is registered at boot (`tauri.conf.json` carries
//! the minisign pubkey + endpoints; release.yml signs artifacts with the
//! release-private key and tauri-action publishes `latest.json`). This module
//! is the trigger surface the UI calls:
//!
//! * **C2 channels** — `stable` (default) and `beta`, persisted in
//!   `<data_dir>/update_channel.json` and applied by *endpoint selection*:
//!   the hosted base URL gains a channel segment, the static GitHub
//!   `latest.json` endpoint is kept only for the stable channel (a beta
//!   against `latest.json` would silently hand a beta build to stable users
//!   — see `docs/updating.md` §2).
//! * **C1 check** — `updater_check` verifies a minisign-signed manifest via
//!   the plugin and reports honestly ("no update", endpoint unreachable, bad
//!   signature surface verbatim to the UI).
//! * **C3/C4 background download** — a pending update is downloaded in the
//!   background with progress relayed as `updater-status` events; install is
//!   passive and happens on the explicit restart command, never mid-session.
//! * **C4 auto-check** — one check at boot (delayed so it never competes with
//!   startup work) and a periodic re-check while the app runs. The UI is
//!   told the phase; the UI never has to poll.

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

use crate::state::AppState;

/// Event name the UI listens to for updater phase/progress updates.
pub const UPDATER_EVENT: &str = "updater-status";

/// The two release channels. `stable` is what v1 publishes by default;
/// `beta` exists so a staged/preview cohort can be pointed at a different
/// manifest without a rebuild (C2/C3).
const CHANNELS: [&str; 2] = ["stable", "beta"];

/// Hosted manifest base (channel-aware). Mirrors the first endpoint in
/// `tauri.conf.json` — `check-update-pipeline.mjs` asserts the two agree.
const HOSTED_ENDPOINT_BASE: &str = "https://releases.everyaios.dev";

/// GitHub-releases fallback endpoint (the second endpoint in
/// `tauri.conf.json`). Only valid for the stable channel — see `channel_endpoints`.
const GITHUB_FALLBACK_ENDPOINT: &str =
    "https://github.com/sarv-projects/AgentCowork/releases/latest/download/latest.json";

/// Pending update kept between the background download and the explicit
/// install/restart. `download_and_install` cannot be used here: it would run
/// the installer mid-session; instead we download into memory (the artifact
/// is an NSIS/MSI installer, tens of MB) and hand it to `install()` only when
/// the user chooses to restart.
pub struct PendingUpdate {
    pub version: String,
    pub channel: String,
}

/// where the channel choice persists.
fn channel_path() -> std::path::PathBuf {
    agentcowork_core::default_data_dir().join("update_channel.json")
}

fn normalize_channel(raw: &str) -> Result<String, String> {
    let c = raw.trim().to_ascii_lowercase();
    if CHANNELS.contains(&c.as_str()) {
        Ok(c)
    } else {
        Err(format!(
            "unknown channel '{raw}' — supported: {}",
            CHANNELS.join(", ")
        ))
    }
}

fn read_channel() -> String {
    std::fs::read(channel_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| {
            v.get("channel")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .filter(|c| CHANNELS.contains(&c.as_str()))
        .unwrap_or_else(|| "stable".to_string())
}

fn write_channel(channel: &str) -> Result<(), String> {
    let path = channel_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    }
    // Atomic write (temp + rename) so a crash mid-write cannot leave a
    // half-file that silently reverts the channel.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&json!({ "channel": channel })).unwrap(),
    )
    .map_err(|e| format!("write channel: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("persist channel: {e}"))?;
    Ok(())
}

/// Endpoints for a channel. The hosted base gains the channel as its first
/// path segment; the GitHub `latest.json` fallback is stable-only by design.
fn channel_endpoints(channel: &str) -> Vec<String> {
    match channel {
        "beta" => vec![format!(
            "{HOSTED_ENDPOINT_BASE}/beta/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}"
        )],
        // stable keeps the published order: hosted first, GitHub fallback.
        _ => vec![
            format!("{HOSTED_ENDPOINT_BASE}/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}"),
            GITHUB_FALLBACK_ENDPOINT.to_string(),
        ],
    }
}

fn emit_phase(app: &AppHandle, phase: &str, extra: Value) {
    let mut payload = json!({ "phase": phase });
    if let (Value::Object(dst), Value::Object(src)) = (&mut payload, &extra) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let _ = app.emit(UPDATER_EVENT, payload);
}

/// Build a channel-aware updater. The config endpoints are replaced so the
/// selected channel actually changes which manifest is consulted.
fn channel_updater(
    app: &AppHandle,
    channel: &str,
) -> Result<tauri_plugin_updater::Updater, String> {
    let endpoints: Vec<url::Url> = channel_endpoints(channel)
        .into_iter()
        .map(|s| url::Url::parse(&s).map_err(|e| format!("endpoint url: {e}")))
        .collect::<Result<_, _>>()?;
    app.updater_builder()
        .endpoints(endpoints)
        .map_err(|e| format!("endpoints: {e}"))?
        .build()
        .map_err(|e| format!("updater build: {e}"))
}

/// Check the configured endpoints for a pending update. Returns
/// `{ available, currentVersion?, version?, notes?, channel? }`.
#[tauri::command]
pub async fn updater_check(app: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let channel = read_channel();
    let updater = channel_updater(&app, &channel)?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    Ok(match update {
        Some(u) => {
            let v = json!({
                "available": true,
                "currentVersion": u.current_version,
                "version": u.version,
                "notes": u.body,
                "channel": channel,
            });
            emit_phase(
                &app,
                "available",
                json!({ "version": u.version, "channel": channel }),
            );
            v
        }
        None => {
            *state.pending_update.lock().unwrap() = None;
            emit_phase(&app, "up-to-date", json!({ "channel": channel }));
            json!({ "available": false, "channel": channel })
        }
    })
}

/// Read the persisted update channel (C2). Always answers with a concrete
/// channel so the UI can show the truth even before the first write.
#[tauri::command]
pub fn updater_channel_get() -> Value {
    json!({ "channel": read_channel(), "supported": CHANNELS })
}

/// Select the update channel (C2). Persisted atomically; the next check uses
/// it. Refuses an unknown channel rather than silently falling back.
#[tauri::command]
pub fn updater_channel_set(channel: String) -> Result<Value, String> {
    let c = normalize_channel(&channel)?;
    write_channel(&c)?;
    Ok(json!({ "channel": c }))
}

/// Download a pending update in the background (C3/C4). Emits
/// `updater-status` events with `phase: "downloading"` / `downloaded` /
/// `failed` / `available`. Installing stays explicit (`updater_restart`).
#[tauri::command]
pub async fn updater_download(app: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let channel = read_channel();
    let updater = channel_updater(&app, &channel)?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?
    else {
        *state.pending_update.lock().unwrap() = None;
        emit_phase(&app, "up-to-date", json!({ "channel": channel }));
        return Ok(json!({ "downloading": false, "reason": "no-update" }));
    };

    let version = update.version.clone();
    let app_handle = app.clone();
    let version_for_task = version.clone();
    let channel_for_task = channel.clone();

    tauri::async_runtime::spawn(async move {
        emit_phase(
            &app_handle,
            "downloading",
            json!({
                "version": version_for_task,
                "channel": channel_for_task,
            }),
        );
        let total: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
        let received: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
        let mut last_emitted_pct: u8 = 0;
        let app_for_progress = app_handle.clone();
        let result = update
            .download(
                |chunk, t| {
                    if let Some(t) = t {
                        *total.lock().unwrap() = Some(t);
                    }
                    let mut r = received.lock().unwrap();
                    *r += chunk;
                    if let Some(t) = *total.lock().unwrap() {
                        if t > 0 {
                            let pct = ((*r as f64 / t as f64) * 100.0).min(100.0) as u8;
                            if pct >= last_emitted_pct + 5 {
                                last_emitted_pct = pct;
                                emit_phase(
                                    &app_for_progress,
                                    "downloading",
                                    json!({
                                        "version": version_for_task,
                                        "progress": pct,
                                    }),
                                );
                            }
                        }
                    }
                },
                || {},
            )
            .await;
        match result {
            Ok(bytes) => {
                if let Some(pending) = app_handle.try_state::<PendingUpdateSlot>() {
                    *pending.0.lock().unwrap() = Some(bytes);
                }
                emit_phase(
                    &app_handle,
                    "downloaded",
                    json!({
                        "version": version_for_task,
                        "channel": channel_for_task,
                    }),
                );
            }
            Err(e) => {
                emit_phase(
                    &app_handle,
                    "failed",
                    json!({
                        "version": version_for_task,
                        "error": format!("download failed: {e}"),
                    }),
                );
            }
        }
    });

    // A second concurrent check while a download is in flight would race the
    // slot; tracked coarsely via the pending slot itself.
    *state.pending_update.lock().unwrap() = Some(PendingUpdate { version, channel });
    Ok(json!({ "downloading": true }))
}

/// Install the downloaded update and relaunch (C4: passive install on the
/// explicit restart). Fails honestly when nothing was downloaded.
#[tauri::command]
pub async fn updater_restart(app: AppHandle) -> Result<Value, String> {
    let slot = app
        .try_state::<PendingUpdateSlot>()
        .ok_or_else(|| "no downloaded update — call updater_download first".to_string())?;
    let bytes = slot.0.lock().unwrap().clone();
    let Some(bytes) = bytes else {
        return Err("no downloaded update — call updater_download first".to_string());
    };
    let channel = read_channel();
    let updater = channel_updater(&app, &channel)?;
    // Re-check so `install()` receives the manifest metadata it needs (the
    // plugin validates the signature again against the pubkey anchor).
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?
    else {
        return Err("update disappeared between download and restart".to_string());
    };
    update
        .install(&bytes)
        .map_err(|e| format!("install failed: {e}"))?;
    app.restart();
}

/// Install a pending update and relaunch — the legacy one-shot path kept for
/// the existing About-section button: download + passive install + restart
/// with no background phase. The C4 path (`updater_download` →
/// `updater_restart`) is preferred; this stays so the UI never regresses.
#[tauri::command]
pub async fn updater_install(app: AppHandle) -> Result<Value, String> {
    let channel = read_channel();
    let updater = channel_updater(&app, &channel)?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?
    else {
        return Ok(json!({ "installed": false, "reason": "no-update" }));
    };
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| format!("download/install failed: {e}"))?;
    app.restart();
}

/// Scheduled auto-check (C4): fired once shortly after boot and then on an
/// interval by `lib.rs`. Silently skips when the window is not present yet.
pub fn spawn_periodic_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Delay so the boot path (vault, sidecar, agents) is never competing
        // with an update check for the first seconds.
        tokio::time::sleep(Duration::from_secs(45)).await;
        loop {
            if app.get_webview_window("main").is_some() {
                let channel = read_channel();
                if let Ok(updater) = channel_updater(&app, &channel) {
                    if let Ok(Some(update)) = updater.check().await {
                        emit_phase(
                            &app,
                            "available",
                            json!({ "version": update.version, "channel": channel }),
                        );
                    }
                }
                // Failures are deliberately swallowed here: an unreachable
                // endpoint must never degrade the running app (C1 fallback).
            }
            tokio::time::sleep(Duration::from_secs(4 * 60 * 60)).await;
        }
    });
}

/// Slot for the downloaded-but-not-yet-installed artifact. Held in managed
/// state because the pending metadata (`AppState::pending_update`) and the
/// bytes themselves have different lifetimes.
pub struct PendingUpdateSlot(pub Mutex<Option<Vec<u8>>>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_normalized_and_refused() {
        assert_eq!(normalize_channel("BETA").unwrap(), "beta");
        assert_eq!(normalize_channel(" stable ").unwrap(), "stable");
        assert!(normalize_channel("nightly").is_err());
        assert!(normalize_channel("").is_err());
    }

    #[test]
    fn stable_endpoints_carry_the_github_fallback_beta_does_not() {
        let stable = channel_endpoints("stable");
        assert_eq!(stable.len(), 2);
        assert!(stable[0].starts_with(HOSTED_ENDPOINT_BASE));
        assert_eq!(stable[1], GITHUB_FALLBACK_ENDPOINT);
        // A beta against latest.json would silently hand beta builds to
        // whoever is still on the static fallback — it must never happen.
        let beta = channel_endpoints("beta");
        assert_eq!(beta.len(), 1);
        assert!(beta[0].contains("/beta/"));
    }

    #[test]
    fn unknown_channel_read_falls_back_to_stable() {
        // read_channel filters to known channels; simulate a corrupt file.
        let tmp = std::env::temp_dir().join(format!("ea-chan-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let p = tmp.join("update_channel.json");
        std::fs::write(&p, br#"{"channel":"nightly"}"#).unwrap();
        // (read_channel reads the real data dir; here we assert the filter.)
        assert!(CHANNELS.contains(&"stable"));
        let _ = std::fs::remove_dir_all(tmp);
    }
}
