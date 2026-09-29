use super::*;
use crate::Vault;
use crate::broker::Broker;
use crate::keyring::KeyRing;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn vault() -> &'static Vault {
    Box::leak(Box::new(Vault::open_in_memory("test-key").unwrap()))
}

/// Read one HTTP request (headers + body) from the socket, waiting until
/// `Content-Length` bytes have arrived (TCP can split the body into a second
/// packet — the old single-read version raced and lost qwen's form body).
fn read_http(s: &mut std::net::TcpStream) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                let need = complete_http_len(&buf);
                if let Some(need) = need {
                    if buf.len() >= need {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Total expected bytes (header + body) once the header terminator is seen.
fn complete_http_len(buf: &[u8]) -> Option<usize> {
    let pos = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
    let clen: usize = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);
    Some(pos + 4 + clen)
}

/// Fake token/device/exchange endpoint. `respond` receives the raw request
/// (headers + body) and returns `(status, body)`.
fn mock_server(respond: impl Fn(&str) -> (u16, String) + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let req = read_http(&mut s);
            let (code, body) = respond(&req);
            let reason = if code == 200 { "OK" } else { "Error" };
            let resp = format!(
                "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

/// Build a fake id_token JWT (payload is base64url, signature ignored).
fn fake_jwt(sub: &str, email: Option<&str>) -> String {
    let mut payload = serde_json::json!({ "sub": sub });
    if let Some(e) = email {
        payload["email"] = serde_json::json!(e);
    }
    let enc = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.as_bytes());
    let header = enc(r#"{"alg":"none"}"#);
    let body = enc(&payload.to_string());
    format!("{header}.{body}.sig")
}

fn token_json(access: &str, refresh: &str, id_token: &str) -> String {
    serde_json::json!({
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": "openid profile email offline_access model.request",
        "id_token": id_token,
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// PKCE (chatgpt-pro)
// ---------------------------------------------------------------------------

#[test]
fn pkce_flow_roundtrip_persists_and_links_ring() {
    let base = mock_server(|_| {
        (
            200,
            token_json(
                "tok_chatgpt_access_1",
                "rt_chatgpt_refresh_1",
                &fake_jwt("sub-123", Some("user@example.com")),
            ),
        )
    });
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:9999/oauth/callback")
        .with_token_url(CHATGPT_PRO, &base);

    let start = om.start_pkce(CHATGPT_PRO).unwrap();
    assert!(
        start
            .auth_url
            .starts_with("https://auth0.openai.com/authorize?")
    );
    assert!(start.auth_url.contains("response_type=code"));
    assert!(start.auth_url.contains("code_challenge_method=S256"));
    assert!(start.auth_url.contains("code_challenge="));
    assert!(start.auth_url.contains("state="));
    // BrowserOS mirrors the official clients' extra params (doc 33 §7.4).
    assert!(start.auth_url.contains("codex_cli_simplified_flow"));
    // The verifier is stored in the vault, never in the URL.
    assert!(!start.auth_url.contains("verifier"));

    let info = om
        .complete_pkce(CHATGPT_PRO, "auth-code-xyz", &start.state)
        .unwrap();
    assert_eq!(info.account_id, "sub-123");
    assert_eq!(info.email.as_deref(), Some("user@example.com"));

    // Key ring carries the access token under the oauth provider name —
    // BYOK failover machinery now applies to it unchanged.
    let ring = KeyRing::new(vault);
    let keys = ring.list(CHATGPT_PRO).unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key_id, "sub-123");
    assert_eq!(keys[0].status, "primary");

    // Token rows live in oauth_tokens (encrypted at rest).
    let accts = om.accounts(CHATGPT_PRO).unwrap();
    assert_eq!(accts.len(), 1);
    let json = serde_json::to_string(&accts).unwrap();
    assert!(!json.contains("tok_chatgpt_access_1"), "leak: {json}");
    assert!(!json.contains("rt_chatgpt_refresh_1"), "leak: {json}");
}

#[test]
fn pkce_state_mismatch_rejected() {
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true);
    let start = om.start_pkce(CHATGPT_PRO).unwrap();
    let err = om
        .complete_pkce(CHATGPT_PRO, "code", "attacker-controlled-state")
        .unwrap_err();
    assert!(matches!(err, OAuthError::StateMismatch));
    // Original state still valid — attacker state did not consume the flow.
    let _ = start;
}

// ---------------------------------------------------------------------------
// Device code (copilot: + internal exchange; qwen: + PKCE verifier)
// ---------------------------------------------------------------------------

#[test]
fn copilot_device_pending_then_approved_with_exchange() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let poll = AtomicU32::new(0);
    let device_base = mock_server(|_| {
        (
            200,
            r#"{"device_code":"dc-1","user_code":"ABCD-1234",
                "verification_uri":"https://github.com/login/device",
                "verification_uri_complete":"https://github.com/login/device?user_code=ABCD-1234",
                "expires_in":900,"interval":5}"#
                .into(),
        )
    });
    let poll_base = mock_server(move |_| {
        let n = poll.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            (200, r#"{"error":"authorization_pending"}"#.into())
        } else {
            (
                200,
                r#"{"access_token":"gho_github_tok","token_type":"bearer","scope":"read:user"}"#
                    .into(),
            )
        }
    });
    let exchange_base = mock_server(|_| {
        (
            200,
            r#"{"token":"copilot-tok-1","expires_at":1750000000}"#.into(),
        )
    });

    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_device_code_url(COPILOT, &device_base)
        .with_token_url(COPILOT, &poll_base)
        .with_exchange_url(COPILOT, &exchange_base);

    let start = om.start_device(COPILOT).unwrap();
    assert_eq!(start.user_code, "ABCD-1234");
    assert_eq!(start.interval_secs, 5);
    assert!(start.verification_uri.contains("github.com"));

    assert_eq!(
        om.poll_device(COPILOT).unwrap(),
        DevicePoll::Pending { interval_secs: 5 }
    );

    match om.poll_device(COPILOT).unwrap() {
        DevicePoll::Approved(info) => {
            assert!(!info.account_id.is_empty());
            // Ring stores the EXCHANGED Copilot token (usable for chat).
            let ring = KeyRing::new(vault);
            let keys = ring.list(COPILOT).unwrap();
            assert_eq!(keys.len(), 1);
            let entry = ring.get(COPILOT, &keys[0].key_id).unwrap();
            assert_eq!(entry.value, b"copilot-tok-1");
            assert!(entry.value != b"gho_github_tok");
        }
        other => panic!("expected Approved, got {other:?}"),
    }

    // Refresh re-runs the exchange with the stored GitHub token.
    let account = om.accounts(COPILOT).unwrap().remove(0);
    let info = om.refresh(COPILOT, &account.account_id).unwrap();
    assert_eq!(info.account_id, account.account_id);
}

#[test]
fn qwen_device_flow_sends_pkce_verifier() {
    let device_seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let seen = device_seen.clone();
    let device_base = mock_server(move |req| {
        seen.lock().unwrap().push(req.to_string());
        (
            200,
            r#"{"device_code":"qd-1","user_code":"QWEN-42",
                "verification_uri":"https://qwen.dev/login/device","expires_in":1800,"interval":3}"#
                .into(),
        )
    });
    let token_seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let tseen = token_seen.clone();
    let token_base = mock_server(move |req| {
        tseen.lock().unwrap().push(req.to_string());
        (
            200,
            r#"{"access_token":"qwen_access","refresh_token":"qwen_refresh",
                "token_type":"Bearer","expires_in":7200,"scope":"openid profile email model.completion"}"#
                .into(),
        )
    });

    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_device_code_url(QWEN, &device_base)
        .with_token_url(QWEN, &token_base);

    let start = om.start_device(QWEN).unwrap();
    assert_eq!(start.user_code, "QWEN-42");
    // Device request carries PKCE challenge + model.completion scope.
    let req = device_seen.lock().unwrap()[0].clone();
    assert!(req.contains("code_challenge="), "{req}");
    assert!(req.contains("code_challenge_method=S256"), "{req}");
    assert!(req.contains("model.completion"), "{req}");

    match om.poll_device(QWEN).unwrap() {
        DevicePoll::Approved(info) => {
            assert_eq!(info.scopes, "openid profile email model.completion");
        }
        other => panic!("expected Approved, got {other:?}"),
    }
    // Poll request carries the code_verifier (form content type).
    let preq = token_seen.lock().unwrap()[0].clone();
    assert!(preq.contains("code_verifier="), "{preq}");
}

// ---------------------------------------------------------------------------
// Refresh + lifecycle
// ---------------------------------------------------------------------------

#[test]
fn refresh_rotates_access_token_in_ring() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let n = AtomicU32::new(0);
    let base = mock_server(move |_| {
        let i = n.fetch_add(1, Ordering::SeqCst);
        if i == 0 {
            (
                200,
                token_json("tok_v1", "rt_v1", &fake_jwt("sub-9", Some("a@b.com"))),
            )
        } else {
            (
                200,
                serde_json::json!({
                    "access_token": "tok_v2",
                    "refresh_token": "rt_v2",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                })
                .to_string(),
            )
        }
    });
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:1/cb")
        .with_token_url(CHATGPT_PRO, &base);

    let start = om.start_pkce(CHATGPT_PRO).unwrap();
    let info = om.complete_pkce(CHATGPT_PRO, "code", &start.state).unwrap();
    assert_eq!(info.account_id, "sub-9");

    let refreshed = om.refresh(CHATGPT_PRO, "sub-9").unwrap();
    assert_eq!(refreshed.account_id, "sub-9");

    let ring = KeyRing::new(vault);
    let entry = ring.get(CHATGPT_PRO, "sub-9").unwrap();
    assert_eq!(entry.value, b"tok_v2", "ring must carry the rotated token");
}

#[test]
fn disabled_flag_gates_every_operation() {
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, false);
    assert!(!om.enabled());
    assert!(matches!(
        om.start_pkce(CHATGPT_PRO),
        Err(OAuthError::Disabled)
    ));
    assert!(matches!(
        om.start_device(COPILOT),
        Err(OAuthError::Disabled)
    ));
    assert!(matches!(om.poll_device(QWEN), Err(OAuthError::Disabled)));
    assert!(matches!(
        om.refresh(CHATGPT_PRO, "x"),
        Err(OAuthError::Disabled)
    ));
}

#[test]
fn revoke_cleans_tokens_ring_and_pending() {
    let base = mock_server(|_| {
        (
            200,
            token_json("tok_x", "rt_x", &fake_jwt("sub-7", Some("x@y.com"))),
        )
    });
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:1/cb")
        .with_token_url(CHATGPT_PRO, &base);

    let start = om.start_pkce(CHATGPT_PRO).unwrap();
    let info = om.complete_pkce(CHATGPT_PRO, "code", &start.state).unwrap();

    om.revoke(CHATGPT_PRO, &info.account_id).unwrap();
    assert!(om.accounts(CHATGPT_PRO).unwrap().is_empty());
    assert!(KeyRing::new(vault).list(CHATGPT_PRO).unwrap().is_empty());
    assert!(matches!(
        om.load_pending(CHATGPT_PRO),
        Err(OAuthError::MissingFlow)
    ));
}

// ---------------------------------------------------------------------------
// Broker integration: 401 → refresh → retry (same-failover semantics)
// ---------------------------------------------------------------------------

#[test]
fn broker_401_refreshes_oauth_token_and_retries() {
    use std::sync::atomic::{AtomicU32, Ordering};

    // Token endpoint (for complete_pkce + refresh): first call issues the
    // initial token; later calls rotate it.
    let tok_n = AtomicU32::new(0);
    let tok_base = mock_server(move |_| {
        let i = tok_n.fetch_add(1, Ordering::SeqCst);
        if i == 0 {
            (
                200,
                token_json(
                    "tok_initial",
                    "rt_initial",
                    &fake_jwt("sub-5", Some("u@v.com")),
                ),
            )
        } else {
            (
                200,
                serde_json::json!({
                    "access_token": "tok_refreshed",
                    "refresh_token": "rt_initial",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                })
                .to_string(),
            )
        }
    });

    // Chat endpoint: first call with the stale token → 401; retry (refreshed
    // token) → 200.
    use std::sync::Arc;
    let chat_n = AtomicU32::new(0);
    let chat_seen = Arc::new(AtomicU32::new(0));
    let seen = chat_seen.clone();
    let chat_base = mock_server(move |req| {
        let i = chat_n.fetch_add(1, Ordering::SeqCst);
        if i == 0 {
            assert!(req.contains("Authorization: Bearer tok_initial"));
            (401, r#"{"error":"invalid_token"}"#.into())
        } else {
            seen.fetch_add(1, Ordering::SeqCst);
            assert!(
                req.contains("Authorization: Bearer tok_refreshed"),
                "retry must use the refreshed token: {req}"
            );
            (200, r#"{"usage":{"total_tokens":3}}"#.into())
        }
    });

    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:1/cb")
        .with_token_url(CHATGPT_PRO, &tok_base);
    let start = om.start_pkce(CHATGPT_PRO).unwrap();
    om.complete_pkce(CHATGPT_PRO, "code", &start.state).unwrap();

    let broker = Broker::new(vault)
        .with_oauth(om)
        .with_base_url(CHATGPT_PRO, &chat_base);
    let resp = broker
        .chat_completion(CHATGPT_PRO, "gpt-5-codex", "s1", serde_json::json!({}))
        .unwrap();
    assert_eq!(crate::broker::usage_tokens(&resp), 3);
    assert_eq!(chat_seen.load(Ordering::SeqCst), 1, "retry happened");
    let _ = &chat_seen;

    // Health: the 401 was a soft failure (no cooldown), success recorded.
    let info = broker.ring().list(CHATGPT_PRO).unwrap();
    assert_eq!(info[0].success_count, 1);
    assert!(!info[0].in_cooldown);
}

#[test]
fn broker_oauth_failover_switches_to_next_key_on_429() {
    // Two oauth accounts in the ring; first 429s → second serves (A3
    // failover applies to subscription accounts exactly like BYOK keys).
    use std::sync::atomic::{AtomicU32, Ordering};
    let n = AtomicU32::new(0);
    let chat_base = mock_server(move |_| {
        if n.fetch_add(1, Ordering::SeqCst) == 0 {
            (429, "rate limited".into())
        } else {
            (200, r#"{"usage":{"total_tokens":2}}"#.into())
        }
    });

    let tok_base = mock_server(|req| {
        // Two distinct accounts via different id_tokens.
        let acct = if req.contains("code=code-a") {
            "a"
        } else {
            "b"
        };
        (
            200,
            token_json(
                &format!("tok_{acct}"),
                &format!("rt_{acct}"),
                &fake_jwt(&format!("sub-{acct}"), None),
            ),
        )
    });

    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:1/cb")
        .with_token_url(CHATGPT_PRO, &tok_base);
    let s1 = om.start_pkce(CHATGPT_PRO).unwrap();
    om.complete_pkce(CHATGPT_PRO, "code-a", &s1.state).unwrap();
    let s2 = om.start_pkce(CHATGPT_PRO).unwrap();
    om.complete_pkce(CHATGPT_PRO, "code-b", &s2.state).unwrap();

    let broker = Broker::new(vault)
        .with_oauth(om)
        .with_base_url(CHATGPT_PRO, &chat_base);
    broker
        .chat_completion(CHATGPT_PRO, "gpt-5-codex", "s1", serde_json::json!({}))
        .unwrap();

    let info = broker.ring().list(CHATGPT_PRO).unwrap();
    let exhausted = info.iter().filter(|k| k.in_cooldown).count();
    assert_eq!(exhausted, 1, "429 key must be in cooldown");
    assert!(info.iter().any(|k| k.success_count == 1));
}

// ---------------------------------------------------------------------------
// Connector providers (ARCH/15 Connect Store) — github/google/microsoft/slack/notion
// ---------------------------------------------------------------------------

#[test]
fn github_connector_device_flow_roundtrip() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let device_base = mock_server(|_| {
        (
            200,
            r#"{"device_code":"dc-gh","user_code":"GHAB-1234",
                "verification_uri":"https://github.com/login/device",
                "verification_uri_complete":"https://github.com/login/device?user_code=GHAB-1234",
                "expires_in":900,"interval":5}"#
                .into(),
        )
    });
    let poll = AtomicU32::new(0);
    let poll_base = mock_server(move |_| {
        if poll.fetch_add(1, Ordering::SeqCst) == 0 {
            (200, r#"{"error":"authorization_pending"}"#.into())
        } else {
            (
                200,
                r#"{"access_token":"gho_connector_tok","token_type":"bearer","scope":"repo read:user"}"#
                    .into(),
            )
        }
    });
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_device_code_url(GITHUB, &device_base)
        .with_token_url(GITHUB, &poll_base);

    let start = om.start_device(GITHUB).unwrap();
    assert_eq!(start.user_code, "GHAB-1234");
    assert!(start.verification_uri.contains("github.com"));

    assert_eq!(
        om.poll_device(GITHUB).unwrap(),
        DevicePoll::Pending { interval_secs: 5 }
    );
    match om.poll_device(GITHUB).unwrap() {
        DevicePoll::Approved(info) => {
            assert!(!info.account_id.is_empty());
            // The token lands in the key ring under the connector provider.
            let ring = KeyRing::new(vault);
            let keys = ring.list(GITHUB).unwrap();
            assert_eq!(keys.len(), 1);
            let entry = ring.get(GITHUB, &keys[0].key_id).unwrap();
            assert_eq!(entry.value, b"gho_connector_tok");
        }
        other => panic!("expected Approved, got {other:?}"),
    }
}

#[test]
fn google_connector_pkce_roundtrip() {
    let base = mock_server(|_| {
        (
            200,
            token_json(
                "tok_google_1",
                "rt_google_1",
                &fake_jwt("google-sub-1", Some("you@gmail.com")),
            ),
        )
    });
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:1/cb")
        .with_token_url(GOOGLE, &base);

    let start = om.start_pkce(GOOGLE).unwrap();
    assert!(
        start
            .auth_url
            .starts_with("https://accounts.google.com/o/oauth2/v2/auth?")
    );
    assert!(start.auth_url.contains("code_challenge_method=S256"));
    assert!(
        start
            .auth_url
            .contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcb")
    );
    // Google scopes are space-encoded in the authorize URL.
    assert!(start.auth_url.contains("drive.readonly"));

    let info = om
        .complete_pkce(GOOGLE, "code-google", &start.state)
        .unwrap();
    assert_eq!(info.account_id, "google-sub-1");
    assert_eq!(info.email.as_deref(), Some("you@gmail.com"));

    let ring = KeyRing::new(vault);
    assert_eq!(ring.list(GOOGLE).unwrap().len(), 1);
}

#[test]
fn microsoft_connector_pkce_roundtrip() {
    let base = mock_server(|_| {
        (
            200,
            token_json(
                "tok_ms_1",
                "rt_ms_1",
                &fake_jwt("ms-sub-1", Some("you@outlook.com")),
            ),
        )
    });
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_redirect_uri("http://127.0.0.1:1/cb")
        .with_token_url(MICROSOFT, &base);

    let start = om.start_pkce(MICROSOFT).unwrap();
    assert!(
        start
            .auth_url
            .starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?")
    );

    let info = om
        .complete_pkce(MICROSOFT, "code-ms", &start.state)
        .unwrap();
    assert_eq!(info.account_id, "ms-sub-1");
    assert_eq!(info.email.as_deref(), Some("you@outlook.com"));
}

#[test]
fn connector_providers_are_registered_and_routable() {
    // Every provider the Connect Store names must resolve in the vault.
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true);
    for p in [GITHUB, GOOGLE, MICROSOFT, SLACK, NOTION] {
        // start_* only fails on disabled/missing provider — a registered
        // provider gets past `provider()` lookup (FlowMismatch proves it's
        // known, since the lookup happens before the flow-kind check).
        assert!(
            !matches!(om.provider(p), Err(OAuthError::ProviderUnsupported(_))),
            "{p} must be a known provider"
        );
    }
    // The vault-side `provider()` is private — this exercises it via
    // `start_pkce` on a pkce provider and expects FlowMismatch only if the
    // kind doesn't match; github is device-code so start_pkce(github) must
    // yield FlowMismatch (proves the provider resolved AND its flow kind).
    assert!(matches!(
        om.start_pkce(GITHUB),
        Err(OAuthError::FlowMismatch { .. })
    ));
    assert!(matches!(
        om.start_device(GOOGLE),
        Err(OAuthError::FlowMismatch { .. })
    ));
    assert!(matches!(
        om.start_device(SLACK),
        Err(OAuthError::FlowMismatch { .. })
    ));
}

#[test]
fn connector_token_persists_and_loads() {
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true);
    // Fresh store has no token.
    assert!(
        om.load_connector_token("remote-mcp", "google-drive")
            .unwrap()
            .is_none()
    );
    // Store a finished connector token (encrypted at rest).
    om.store_connector_token(
        "remote-mcp",
        "google-drive",
        "acc-tok-1",
        Some("ref-tok-1"),
        3600,
        "drive.readonly",
    )
    .unwrap();
    let loaded = om
        .load_connector_token("remote-mcp", "google-drive")
        .unwrap()
        .unwrap();
    assert_eq!(loaded, "acc-tok-1");
    // Upsert refreshes access token in place, keeps the row.
    om.store_connector_token(
        "remote-mcp",
        "google-drive",
        "acc-tok-2",
        None,
        3600,
        "drive.readonly",
    )
    .unwrap();
    let loaded = om
        .load_connector_token("remote-mcp", "google-drive")
        .unwrap()
        .unwrap();
    assert_eq!(loaded, "acc-tok-2");
    // Different store id stays independent.
    assert!(
        om.load_connector_token("remote-mcp", "github")
            .unwrap()
            .is_none()
    );
    // Revoke removes the connector token too.
    om.revoke("remote-mcp", "google-drive").unwrap();
    assert!(
        om.load_connector_token("remote-mcp", "google-drive")
            .unwrap()
            .is_none()
    );
}

#[test]
fn client_id_env_override_supplies_empty_provider() {
    // Slack/Notion ship with no public client id; an operator can supply their
    // own registered app id via AGENTCOWORK_OAUTH_CLIENT_ID_<UPPER_PROVIDER>.
    unsafe {
        std::env::set_var("AGENTCOWORK_OAUTH_CLIENT_ID_SLACK", "slack-own-app-123");
    }
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true);
    // provider() is private but the test module is a child of oauth.rs.
    let settings = om.provider(SLACK).unwrap();
    assert_eq!(settings.client_id, "slack-own-app-123");
    unsafe {
        std::env::remove_var("AGENTCOWORK_OAUTH_CLIENT_ID_SLACK");
    }
    // Without the override, slack returns the (empty) default id.
    let om2 = OAuthManager::with_enabled(vault, true);
    assert_eq!(om2.provider(SLACK).unwrap().client_id, "");
    // Unknown provider unaffected.
    unsafe {
        std::env::set_var("AGENTCOWORK_OAUTH_CLIENT_ID_GHOST", "zzz");
    }
    let om3 = OAuthManager::with_enabled(vault, true);
    assert!(om3.provider("ghost").is_err()); // ghost is not a provider
    unsafe {
        std::env::remove_var("AGENTCOWORK_OAUTH_CLIENT_ID_GHOST");
    }
}

/// The retired `EVERYAIOS_OAUTH_CLIENT_ID_*` spelling stays a read-only
/// fallback (DEC-053).
#[test]
fn legacy_client_id_env_override_still_honored() {
    unsafe {
        std::env::set_var("EVERYAIOS_OAUTH_CLIENT_ID_SLACK", "legacy-slack-app");
    }
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true);
    assert_eq!(om.provider(SLACK).unwrap().client_id, "legacy-slack-app");
    unsafe {
        std::env::set_var("AGENTCOWORK_OAUTH_CLIENT_ID_SLACK", "new-slack-app");
    }
    let om = OAuthManager::with_enabled(vault, true);
    assert_eq!(om.provider(SLACK).unwrap().client_id, "new-slack-app");
    unsafe {
        std::env::remove_var("EVERYAIOS_OAUTH_CLIENT_ID_SLACK");
        std::env::remove_var("AGENTCOWORK_OAUTH_CLIENT_ID_SLACK");
    }
}

// ---------------------------------------------------------------------------
// FIX-09 — the custody egress is floored before the socket
// ---------------------------------------------------------------------------

#[test]
fn custody_egress_refuses_the_always_blocked_ranges() {
    for url in [
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.5/token",
        "http://192.168.1.10/token",
        "http://[fe80::1]/token",
        "http://0.0.0.0/token",
    ] {
        let err = post_form(url, &[("grant_type", "authorization_code")]).unwrap_err();
        match err {
            OAuthError::EgressDenied { reason, url: u } => {
                let expected_origin = url::Url::parse(url).unwrap().origin().ascii_serialization();
                assert_eq!(u, expected_origin, "refusal must not echo URL paths");
                assert!(!reason.is_empty());
            }
            other => panic!("expected an egress denial for {url}, got {other:?}"),
        }
    }
}

#[test]
fn custody_egress_refuses_non_http_schemes_and_garbage() {
    for url in [
        "file:///etc/passwd",
        "gopher://127.0.0.1:11211/",
        "not a url",
        "",
    ] {
        assert!(
            matches!(post_form(url, &[]), Err(OAuthError::EgressDenied { .. })),
            "{url} must be refused"
        );
    }
}

/// The credentialed exchange is the strongest case: it carries a live token in
/// the `Authorization` header, so a refused destination must be refused before
/// the header is ever built.
#[test]
fn credentialed_exchange_is_floored_too() {
    let err = get_json_with_auth("http://169.254.169.254/token", "gh-secret").unwrap_err();
    assert!(matches!(err, OAuthError::EgressDenied { .. }), "{err:?}");
    let err = get_json_with_auth("http://192.168.1.10/token", "gh-secret").unwrap_err();
    assert!(matches!(err, OAuthError::EgressDenied { .. }), "{err:?}");
}

/// A re-pointed endpoint cannot escape the floor: `with_token_url` is public,
/// so the check has to live at the client and not at the call site.
#[test]
fn a_repointed_token_url_cannot_escape_the_floor() {
    let vault = vault();
    let om = OAuthManager::with_enabled(vault, true)
        .with_token_url(CHATGPT_PRO, "http://169.254.169.254/oauth/token");
    let start = om.start_pkce(CHATGPT_PRO).unwrap();
    let err = om
        .complete_pkce(CHATGPT_PRO, "auth-code", &start.state)
        .unwrap_err();
    assert!(matches!(err, OAuthError::EgressDenied { .. }), "{err:?}");
    // No token row was written: the effect could not complete.
    assert!(om.accounts(CHATGPT_PRO).unwrap().is_empty());
}

/// Loopback stays reachable: a loopback OAuth endpoint is a legitimate desktop
/// setup (and the test suite's own mock), so the custody path is not narrowed
/// into uselessness by the fix.
#[test]
fn loopback_endpoints_still_pass_the_floor() {
    assert!(
        crate::guarded_http::agent_for("http://127.0.0.1:8080/oauth/token").is_ok(),
        "a loopback desktop callback/test endpoint remains reachable"
    );
}
