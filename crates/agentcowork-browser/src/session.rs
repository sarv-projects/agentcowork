//! P2.7 Session Vault cookie glue (E11/E7/E13 — ARCH/08 §8.9).
//!
//! The vault (`agentcowork-vault`) stores cookies; this module is the CDP half
//! that moves them between the browser and the vault:
//!
//! * [`get_cookies`] / [`set_cookies`] — `Network.getCookies` /
//!   `Network.setCookies` through the `CdpSession` trait (testable without a
//!   real browser).
//! * [`cookie_from_cdp`] / [`cookie_to_cdp`] — pure CDP cookie JSON ↔ vault
//!   [`Cookie`] conversions (CDP `expires` seconds → vault `Option<i64>`;
//!   `0`/`-1` = session cookie → `None`).
//! * [`seal_session`] — capture path 1 (E7): sign-in-in-browser → cookies →
//!   seal to the vault.
//! * [`inject_session`] — vault → `SessionContext` → cookies → browser context.
//! * [`inherit_cookies_from_chrome`] — capture path 2 (E13): attach to the
//!   user's already-running Chrome via its debug port and pull every cookie
//!   (`Browser.getAllCookies`), grouped per site by [`group_cookies_by_site`].

use crate::capture::CdpSession;
use agentcowork_cdp::CdpError;
use agentcowork_vault::{CaptureInput, Cookie, SessionError, SessionVault, TrustLevel};
use serde_json::{Value, json};

/// Errors crossing both the CDP and vault halves of the bridge.
#[derive(Debug, thiserror::Error)]
pub enum SessionBridgeError {
    #[error("cdp: {0}")]
    Cdp(#[from] CdpError),
    #[error("session vault: {0}")]
    Session(#[from] SessionError),
    #[error("attach refused: {0}")]
    Attach(String),
}

/// Parse one CDP cookie object (`Network.getCookies` / `Browser.getAllCookies`
/// element) into a vault [`Cookie`]. Returns `None` for a malformed entry
/// (missing name/value) so a single bad cookie never breaks the whole jar.
///
/// CDP `expires` is seconds-since-epoch; `-1` (and `0`) mark a session cookie
/// and map to `None` (vault has no fixed expiry).
pub fn cookie_from_cdp(v: &Value) -> Option<Cookie> {
    let name = v.get("name")?.as_str()?.to_string();
    let value = v.get("value")?.as_str()?.to_string().into_bytes();
    Some(Cookie {
        name,
        value,
        domain: v
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        path: v
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("/")
            .to_string(),
        expires: match v.get("expires").and_then(Value::as_i64) {
            Some(e) if e > 0 => Some(e),
            _ => None,
        },
        http_only: v.get("httpOnly").and_then(Value::as_bool).unwrap_or(false),
        secure: v.get("secure").and_then(Value::as_bool).unwrap_or(false),
        same_site: v
            .get("sameSite")
            .and_then(Value::as_str)
            .unwrap_or("Lax")
            .to_string(),
    })
}

/// Serialize a vault [`Cookie`] into the CDP `Network.setCookies` element
/// shape. `expires` is omitted for session cookies (CDP treats a missing
/// expiry as session-scoped); `sameSite` is normalized to CDP's enum case.
pub fn cookie_to_cdp(c: &Cookie) -> Value {
    let mut v = json!({
        "name": c.name,
        "value": String::from_utf8_lossy(&c.value),
        "domain": c.domain,
        "path": c.path,
        "httpOnly": c.http_only,
        "secure": c.secure,
        "sameSite": normalize_same_site(&c.same_site),
    });
    if let Some(expires) = c.expires {
        v["expires"] = json!(expires);
    }
    v
}

/// CDP `sameSite` expects `Strict | Lax | None`; be lenient on read (accept
/// lowercase / `no_restriction`).
fn normalize_same_site(s: &str) -> &str {
    match s.to_ascii_lowercase().as_str() {
        "strict" => "Strict",
        "none" | "no_restriction" => "None",
        _ => "Lax",
    }
}

/// `Network.getCookies` → vault cookies (capture path 1, E7). Session-scoped:
/// returns the jar visible to the attached page.
pub fn get_cookies<C: CdpSession>(client: &C, session_id: &str) -> Result<Vec<Cookie>, CdpError> {
    let out = client.call_session(session_id, "Network.getCookies", json!({}))?;
    let list = out
        .get("cookies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(list.iter().filter_map(cookie_from_cdp).collect())
}

/// `Network.setCookies` (plural, one call for the whole jar) → inject vault
/// cookies into the browser context at request time.
pub fn set_cookies<C: CdpSession>(
    client: &C,
    session_id: &str,
    cookies: &[Cookie],
) -> Result<(), CdpError> {
    let list: Vec<Value> = cookies.iter().map(cookie_to_cdp).collect();
    client.call_session(session_id, "Network.setCookies", json!({ "cookies": list }))?;
    Ok(())
}

/// Capture path 1 (E7): read the live jar and seal it to the vault under
/// `(site, account)`. Returns the opaque session id.
pub fn seal_session<C: CdpSession>(
    client: &C,
    session_id: &str,
    vault: &SessionVault,
    site: &str,
    account: &str,
    trust_level: TrustLevel,
    persist: bool,
) -> Result<String, SessionBridgeError> {
    let cookies = get_cookies(client, session_id)?;
    let id = vault.capture(
        site,
        account,
        CaptureInput {
            cookies,
            storage: Vec::new(),
            headers: Vec::new(),
            persist,
            trust_level,
            ttl_secs: None,
        },
    )?;
    Ok(id)
}

/// Injection (E11): pull the sealed context (Trust-Ladder-gated) and push its
/// cookies into the browser context. This is the only path that moves raw
/// cookie values out of the vault.
pub fn inject_session<C: CdpSession>(
    client: &C,
    session_id: &str,
    vault: &SessionVault,
    handle: &str,
    agent_id: &str,
    level: TrustLevel,
) -> Result<(), SessionBridgeError> {
    let ctx = vault.inject(handle, agent_id, level)?;
    set_cookies(client, session_id, &ctx.cookies)?;
    Ok(())
}

/// Capture path 2 (E13 session inheritance): attach to an *already-running*
/// Chrome via its debug port and pull cookies (`Storage.getCookies`).
///
/// Hardening (Chrome 136+ / E13):
/// - AgentCowork-owned isolated profiles attach without pairing.
/// - Default Chrome/Chromium profiles are **refused** (CDP is inert on
///   Chrome 136+ default user-data-dir; we never fall back to raw-cookie
///   extraction to "make it work").
/// - Any other user-launched profile requires a one-time
///   [`ProfilePairingStore`] entry (`paired = true`).
pub fn inherit_cookies_from_chrome(
    port: u16,
    user_data_dir: Option<&std::path::Path>,
    paired: bool,
) -> Result<Vec<(String, Vec<Cookie>)>, SessionBridgeError> {
    let endpoint = agentcowork_cdp::probe_browser(port)?;
    agentcowork_cdp::assert_attach_allowed(&endpoint.version, user_data_dir, paired)
        .map_err(|e| SessionBridgeError::Attach(e.to_string()))?;
    let client = agentcowork_cdp::connect_to_browser(&endpoint)?;
    let out = client.call("Storage.getCookies", json!({}))?;
    let list = out
        .get("cookies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let cookies: Vec<Cookie> = list.iter().filter_map(cookie_from_cdp).collect();
    Ok(group_cookies_by_site(cookies))
}

/// Group a flat cookie list into per-site buckets keyed by the cookie host
/// with any leading `.` stripped. Stable order (first-seen); preserves every
/// cookie within its bucket.
pub fn group_cookies_by_site(cookies: Vec<Cookie>) -> Vec<(String, Vec<Cookie>)> {
    let mut out: Vec<(String, Vec<Cookie>)> = Vec::new();
    for c in cookies {
        let site = c.domain.trim_start_matches('.').to_string();
        match out.iter_mut().find(|(s, _)| *s == site) {
            Some((_, bucket)) => bucket.push(c),
            None => out.push((site, vec![c])),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CdpSession;
    use agentcowork_cdp::Session;
    use agentcowork_vault::{StorageKind, Vault};

    /// Scripted CDP mock: answers `Network.getCookies` with a canned cookie
    /// list and records `Network.setCookies` payloads.
    #[derive(Default)]
    struct MockCookies {
        jar: Value,
        set_calls: std::sync::Mutex<Vec<Value>>,
    }

    impl MockCookies {
        fn jar(mut self, cookies: Value) -> Self {
            self.jar = json!({ "cookies": cookies });
            self
        }
        fn set_calls(&self) -> Vec<Value> {
            self.set_calls.lock().unwrap().clone()
        }
    }

    impl CdpSession for MockCookies {
        fn call(&self, method: &str, _params: Value) -> Result<Value, CdpError> {
            match method {
                "Storage.getCookies" => Ok(self.jar.clone()),
                _ => Err(CdpError::Protocol {
                    code: -1,
                    message: format!("unexpected browser call {method}"),
                }),
            }
        }
        fn call_session(
            &self,
            _session_id: &str,
            method: &str,
            params: Value,
        ) -> Result<Value, CdpError> {
            match method {
                "Network.getCookies" => Ok(self.jar.clone()),
                "Network.setCookies" => {
                    self.set_calls.lock().unwrap().push(params);
                    Ok(json!({}))
                }
                _ => Err(CdpError::Protocol {
                    code: -1,
                    message: format!("unexpected session call {method}"),
                }),
            }
        }
        fn attach(&self, _target_id: &str) -> Result<Session, CdpError> {
            Err(CdpError::Protocol {
                code: -1,
                message: "no attach".into(),
            })
        }
        fn drain_events(&self) -> Vec<agentcowork_cdp::CdpEvent> {
            Vec::new()
        }
    }

    fn sample_jar() -> Value {
        json!([
            {
                "name": "session",
                "value": "tok-123",
                "domain": ".example.com",
                "path": "/",
                "expires": 1893456000,
                "httpOnly": true,
                "secure": true,
                "sameSite": "Lax"
            },
            {
                "name": "theme",
                "value": "dark",
                "domain": "example.com",
                "path": "/",
                "expires": -1,
                "httpOnly": false,
                "secure": false,
                "sameSite": "None"
            }
        ])
    }

    fn in_memory_vault() -> Vault {
        Vault::open_in_memory("session-glue-test-key").expect("open in-memory vault")
    }

    #[test]
    fn cookie_roundtrips_through_cdp_json() {
        let jar = sample_jar();
        let cookies: Vec<Cookie> = jar
            .as_array()
            .unwrap()
            .iter()
            .filter_map(cookie_from_cdp)
            .collect();
        assert_eq!(cookies.len(), 2);

        assert_eq!(cookies[0].name, "session");
        assert_eq!(cookies[0].value, b"tok-123");
        assert_eq!(cookies[0].domain, ".example.com");
        assert_eq!(cookies[0].expires, Some(1893456000));
        assert!(cookies[0].http_only && cookies[0].secure);
        assert_eq!(cookies[0].same_site, "Lax");

        // Session cookie (expires = -1) → None.
        assert_eq!(cookies[1].expires, None);
        assert_eq!(cookies[1].same_site, "None");

        // Serialize back to CDP shape.
        let cdp = cookie_to_cdp(&cookies[0]);
        assert_eq!(cdp.get("name").and_then(Value::as_str), Some("session"));
        assert_eq!(cdp.get("expires").and_then(Value::as_i64), Some(1893456000));
        assert_eq!(cdp.get("httpOnly").and_then(Value::as_bool), Some(true));
        // Session cookie → no expires key.
        let cdp2 = cookie_to_cdp(&cookies[1]);
        assert!(cdp2.get("expires").is_none());
    }

    #[test]
    fn malformed_cookie_is_skipped_not_fatal() {
        let jar = json!([
            { "name": "ok", "value": "1", "domain": "a.com" },
            { "name": "missing-value" },
            { "value": "missing-name" }
        ]);
        let cookies: Vec<Cookie> = jar
            .as_array()
            .unwrap()
            .iter()
            .filter_map(cookie_from_cdp)
            .collect();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "ok");
    }

    #[test]
    fn same_site_is_normalized_for_cdp() {
        assert_eq!(normalize_same_site("strict"), "Strict");
        assert_eq!(normalize_same_site("none"), "None");
        assert_eq!(normalize_same_site("no_restriction"), "None");
        assert_eq!(normalize_same_site("lax"), "Lax");
        assert_eq!(normalize_same_site("garbage"), "Lax");
    }

    #[test]
    fn get_cookies_reads_network_jar() {
        let m = MockCookies::default().jar(sample_jar());
        let cookies = get_cookies(&m, "sess-1").unwrap();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].name, "session");
    }

    #[test]
    fn set_cookies_writes_whole_jar_in_one_call() {
        let m = MockCookies::default();
        let jar = sample_jar();
        let cookies: Vec<Cookie> = jar
            .as_array()
            .unwrap()
            .iter()
            .filter_map(cookie_from_cdp)
            .collect();
        set_cookies(&m, "sess-1", &cookies).unwrap();
        let calls = m.set_calls();
        assert_eq!(calls.len(), 1, "one Network.setCookies call");
        let list = calls[0].get("cookies").and_then(Value::as_array).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn seal_then_inject_roundtrip_through_vault() {
        let v = in_memory_vault();
        let sv = SessionVault::new(&v);
        let m = MockCookies::default().jar(sample_jar());

        // Capture path 1: seal the live jar.
        let id = seal_session(
            &m,
            "sess-1",
            &sv,
            "example.com",
            "work",
            TrustLevel::ReadOnly,
            true,
        )
        .unwrap();

        // Grant + inject: the exact cookie must come back out of the vault.
        sv.grant(&id, "agent-1", TrustLevel::ReadOnly).unwrap();
        inject_session(&m, "sess-1", &sv, &id, "agent-1", TrustLevel::ReadOnly).unwrap();
        let calls = m.set_calls();
        assert_eq!(calls.len(), 1);
        let list = calls[0].get("cookies").and_then(Value::as_array).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].get("name").and_then(Value::as_str), Some("session"));
    }

    #[test]
    fn inject_without_grant_is_denied() {
        let v = in_memory_vault();
        let sv = SessionVault::new(&v);
        let m = MockCookies::default().jar(sample_jar());
        let id = seal_session(
            &m,
            "sess-1",
            &sv,
            "example.com",
            "work",
            TrustLevel::ReadOnly,
            true,
        )
        .unwrap();

        let err =
            inject_session(&m, "sess-1", &sv, &id, "agent-1", TrustLevel::ReadOnly).unwrap_err();
        assert!(matches!(
            err,
            SessionBridgeError::Session(SessionError::AccessDenied { .. })
        ));
        // No injection happened → no setCookies call.
        assert!(m.set_calls().is_empty());
    }

    #[test]
    fn group_cookies_by_site_strips_leading_dot() {
        let jar = sample_jar();
        let cookies: Vec<Cookie> = jar
            .as_array()
            .unwrap()
            .iter()
            .filter_map(cookie_from_cdp)
            .collect();
        let grouped = group_cookies_by_site(cookies);
        // `.example.com` and `example.com` collapse to one bucket.
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, "example.com");
        assert_eq!(grouped[0].1.len(), 2);
    }

    #[test]
    fn group_cookies_by_site_keeps_distinct_hosts_separate() {
        let cookies = vec![
            Cookie {
                name: "a".into(),
                value: b"1".to_vec(),
                domain: ".one.com".into(),
                path: "/".into(),
                expires: None,
                http_only: false,
                secure: false,
                same_site: "Lax".into(),
            },
            Cookie {
                name: "b".into(),
                value: b"2".to_vec(),
                domain: "two.com".into(),
                path: "/".into(),
                expires: None,
                http_only: false,
                secure: false,
                same_site: "Lax".into(),
            },
        ];
        let grouped = group_cookies_by_site(cookies);
        assert_eq!(grouped.len(), 2);
        assert!(grouped.iter().any(|(s, _)| s == "one.com"));
        assert!(grouped.iter().any(|(s, _)| s == "two.com"));
    }

    #[test]
    fn vault_storage_kind_is_exposed_for_future_paths() {
        // Guards the StorageKind import compiles & serde tags are stable —
        // the capture path 1 currently seals cookies only (storage/IndexedDB
        // capture is the CDP `DOMStorage`/`IndexedDB` follow-on).
        assert_eq!(StorageKind::Local.as_str(), "local");
        assert_eq!(StorageKind::Session.as_str(), "session");
        assert_eq!(StorageKind::IndexedDb.as_str(), "indexeddb");
    }
}
