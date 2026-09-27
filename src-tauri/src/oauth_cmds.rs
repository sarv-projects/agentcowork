//! P1.7 — Tauri OAuth subscription wiring. PKCE loopback + device-code poll.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use agentcowork_vault::oauth::{
    DevicePoll, OAuthManager, CHATGPT_PRO, COPILOT, OAUTH_ENV_FLAG, QWEN,
};
use tauri::State;

use crate::AppState;

#[tauri::command]
pub fn oauth_status() -> serde_json::Value {
    serde_json::json!({
        "enabled": std::env::var(OAUTH_ENV_FLAG).is_ok(),
        "providers": [CHATGPT_PRO, COPILOT, QWEN],
    })
}

#[tauri::command]
pub fn oauth_accounts(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mgr = OAuthManager::new(&vault);
    if !mgr.enabled() {
        return Ok(serde_json::json!({ "enabled": false, "accounts": [] }));
    }
    let mut accounts = Vec::new();
    for p in [CHATGPT_PRO, COPILOT, QWEN] {
        if let Ok(list) = mgr.accounts(p) {
            for a in list {
                accounts.push(serde_json::to_value(a).unwrap_or_default());
            }
        }
    }
    Ok(serde_json::json!({ "enabled": true, "accounts": accounts }))
}

#[tauri::command]
pub fn oauth_start_pkce(
    state: State<'_, AppState>,
    provider: String,
) -> Result<serde_json::Value, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/oauth/callback");
    let start = {
        let vault = state.vault.lock().map_err(|e| e.to_string())?;
        let mgr = OAuthManager::new(&vault).with_redirect_uri(&redirect);
        mgr.start_pkce(&provider).map_err(|e| e.to_string())?
    };
    let vault = Arc::clone(&state.vault);
    let provider_c = provider.clone();
    let redirect_c = redirect.clone();
    thread::spawn(move || {
        let _ = listener.set_nonblocking(false);
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let (code, st) = parse_callback(&req);
            let body = if let (Some(code), Some(st)) = (code, st) {
                if let Ok(v) = vault.lock() {
                    let mgr = OAuthManager::new(&v).with_redirect_uri(&redirect_c);
                    match mgr.complete_pkce(&provider_c, &code, &st) {
                        Ok(_) => {
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body>AgentCowork signed in. You can close this tab.</body></html>"
                        }
                        Err(_) => {
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\r\noauth failed"
                        }
                    }
                } else {
                    "HTTP/1.1 500 Internal Server Error\r\n\r\nvault"
                }
            } else {
                "HTTP/1.1 400 Bad Request\r\n\r\nmissing code"
            };
            let _ = stream.write_all(body.as_bytes());
        }
    });
    Ok(serde_json::json!({
        "authUrl": start.auth_url,
        "state": start.state,
        "redirectUri": redirect,
    }))
}

#[tauri::command]
pub fn oauth_start_device(
    state: State<'_, AppState>,
    provider: String,
) -> Result<serde_json::Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mgr = OAuthManager::new(&vault);
    let start = mgr.start_device(&provider).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "provider": start.provider,
        "userCode": start.user_code,
        "verificationUri": start.verification_uri,
        "verificationUriComplete": start.verification_uri_complete,
        "intervalSecs": start.interval_secs,
        "expiresIn": start.expires_in,
    }))
}

#[tauri::command]
pub fn oauth_poll_device(
    state: State<'_, AppState>,
    provider: String,
) -> Result<serde_json::Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mgr = OAuthManager::new(&vault);
    match mgr.poll_device(&provider).map_err(|e| e.to_string())? {
        DevicePoll::Pending { interval_secs } => Ok(serde_json::json!({
            "status": "pending", "intervalSecs": interval_secs
        })),
        DevicePoll::SlowDown { interval_secs } => Ok(serde_json::json!({
            "status": "slow_down", "intervalSecs": interval_secs
        })),
        DevicePoll::Approved(info) => Ok(serde_json::json!({
            "status": "approved", "account": info
        })),
        DevicePoll::Expired => Ok(serde_json::json!({ "status": "expired" })),
        DevicePoll::Denied => Ok(serde_json::json!({ "status": "denied" })),
    }
}

#[tauri::command]
pub fn oauth_revoke(
    state: State<'_, AppState>,
    provider: String,
    account_id: String,
) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mgr = OAuthManager::new(&vault);
    mgr.revoke(&provider, &account_id)
        .map_err(|e| e.to_string())
}

fn parse_callback(req: &str) -> (Option<String>, Option<String>) {
    let line = req.lines().next().unwrap_or("");
    let q = line.split(' ').nth(1).unwrap_or("");
    let qs = q.split('?').nth(1).unwrap_or("");
    let mut code = None;
    let mut state = None;
    for pair in qs.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        let v = urlencoding_decode(v);
        if k == "code" {
            code = Some(v);
        } else if k == "state" {
            state = Some(v);
        }
    }
    (code, state)
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(h) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(h as char);
                i += 3;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}
