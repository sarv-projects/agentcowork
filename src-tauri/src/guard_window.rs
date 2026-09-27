//! F1 — the dedicated Guard-2 approval surface.
//!
//! Guard-2's human consent used to terminate in the main webview — the same
//! renderer that displays browser views, generative-UI iframes and plugin
//! views. A compromised main renderer could draw a convincing fake approval
//! card over a real pending ticket (the nonce stops ticket *forgery*, not
//! human *deception*).
//!
//! This module gives consent its own webview: a small, always-on-top window
//! that loads only the bundled `guard.html` (a minimal, dependency-free page —
//! never browser content, never iframes, never generative UI), receives its
//! ticket payloads straight from Rust via IPC, and is the *only* surface
//! `guard_respond` accepts. The nonce still binds the card to the ticket; the
//! new invariant is that the card itself lives outside the untrusted
//! renderer.
//!
//! Remaining hardening (unchanged, honestly labeled): a true OS-native dialog
//! is still the long-horizon item — this window is a dedicated webview, not a
//! native dialog — but it is no longer *the main window's* webview.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The label of the dedicated approval window. `guard_respond` refuses any
/// caller that is not this window.
pub const GUARD_WINDOW_LABEL: &str = "guard";

/// Create the approval window if it does not already exist. It loads only the
/// bundled `guard.html` (fixed local asset — the URL can never be redirected
/// to remote or untrusted content) and is pinned above the main window.
pub fn ensure_guard_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(win) = app.get_webview_window(GUARD_WINDOW_LABEL) {
        return Ok(win);
    }
    let win = WebviewWindowBuilder::new(
        app,
        GUARD_WINDOW_LABEL,
        WebviewUrl::App("guard.html".into()),
    )
    .title("AgentCowork — Approval")
    .inner_size(600.0, 720.0)
    .min_inner_size(480.0, 560.0)
    .always_on_top(true)
    .resizable(true)
    .center()
    .build()?;
    Ok(win)
}

/// Show + focus the approval window (used by the main UI when a ticket is
/// waiting, so the consent surface is where the user looks).
pub fn open_guard_window(app: &AppHandle) -> tauri::Result<()> {
    let win = ensure_guard_window(app)?;
    let _ = win.show();
    let _ = win.unminimize();
    let _ = win.set_focus();
    Ok(())
}
