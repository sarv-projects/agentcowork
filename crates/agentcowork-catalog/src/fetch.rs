//! P56.1 — the network half of the catalog refresh, plus the job body that
//! ties fetch → pure decision → durable store together.
//!
//! [`HttpFetch::get`] is the documented HTTP seam: a **conditional** GET of
//! `https://models.dev/api.json` with `If-None-Match`, so the 4h job costs one
//! 304 round-trip when nothing changed instead of re-downloading 4.6 MB.
//!
//! Deliberate choices:
//!
//! * **`/api/providers.json` is never requested** — the live site serves the
//!   SPA HTML there, not JSON (verified 2026-09-10), which would look like a
//!   corrupt catalog.
//! * **The ETag we send is the raw server token** (normalized, unquoted) —
//!   servers accept both, and normalizing both sides is what makes the 304
//!   path fire.
//! * A failed refresh **never deletes the cached snapshot**; it only records
//!   the honest verdict in the meta so `catalog_status` can report it.
//!
//! `refresh_now` is exercised against a local HTTP fixture in tests (no live
//! network), and the live leg is env-gated like every other client here.

use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::live::{FetchOutcome, MODELS_DEV_API_URL, RefreshDecision, apply_refresh};
use crate::store::{CatalogMeta, CatalogStore};

/// Outbound timeout for the catalog fetch (the body is ~4.6 MB).
pub const FETCH_TIMEOUT_SECS: u64 = 30;

/// The catalog's HTTP client (one `ureq` agent, reused across refreshes).
#[derive(Debug, Clone)]
pub struct HttpFetch {
    timeout: Duration,
    user_agent: String,
    /// Overridable for tests / a mirror. Never a different *path*: the
    /// catalog is only ever one JSON document.
    url: String,
    /// P45.7 — one resolution and one reused HTTP agent per session.
    /// `end_session` drops both so the next call cannot reuse a stale address.
    session: Arc<Mutex<HostSession>>,
}

struct HostSession {
    hosts: BTreeMap<String, Vec<String>>,
    lookups: u64,
    agent: Option<ureq::Agent>,
}

impl std::fmt::Debug for HostSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSession")
            .field("hosts", &self.hosts)
            .field("lookups", &self.lookups)
            .field("agent", &self.agent.is_some())
            .finish()
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetch {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(FETCH_TIMEOUT_SECS),
            user_agent: format!("AgentCowork/{}", env!("CARGO_PKG_VERSION")),
            url: MODELS_DEV_API_URL.to_string(),
            session: Arc::new(Mutex::new(HostSession {
                hosts: BTreeMap::new(),
                lookups: 0,
                agent: None,
            })),
        }
    }

    /// Drop cached addresses and the reused agent. The next request resolves again.
    pub fn end_session(&self) {
        let mut session = self.session.lock().expect("dns session");
        session.hosts.clear();
        session.agent = None;
    }

    /// How many times this session has resolved a host. A second call in the
    /// same session does not increase it.
    pub fn resolution_count(&self) -> u64 {
        self.session.lock().expect("dns session").lookups
    }

    fn host_of(url: &str) -> Option<String> {
        let rest = url.split("://").nth(1)?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority);
        let host = host.trim_matches(['[', ']']);
        if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        }
    }

    fn remember_host(session: &mut HostSession, host: &str) {
        if session.hosts.contains_key(host) {
            return;
        }
        let addrs = (host, 443)
            .to_socket_addrs()
            .map(|iter| iter.map(|addr| addr.to_string()).collect())
            .unwrap_or_default();
        session.hosts.insert(host.to_string(), addrs);
        session.lookups = session.lookups.saturating_add(1);
    }

    fn agent(&self) -> ureq::Agent {
        let mut session = self.session.lock().expect("dns session");
        if let Some(host) = Self::host_of(&self.url) {
            Self::remember_host(&mut session, &host);
        }
        if session.agent.is_none() {
            session.agent = Some(ureq::AgentBuilder::new().timeout(self.timeout).build());
        }
        session.agent.clone().expect("agent")
    }

    /// Point the client at a mirror (tests use a loopback fixture).
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The exact `If-None-Match` value for a stored ETag (or `None`).
    pub fn conditional_header(etag: Option<&str>) -> Option<String> {
        let e = etag.unwrap_or("").trim();
        if e.is_empty() {
            None
        } else {
            Some(e.to_string())
        }
    }

    /// One conditional GET. Never panics: every failure is a
    /// [`FetchOutcome::Failed`] with the transport's own message.
    pub fn get(&self, etag: Option<&str>) -> FetchOutcome {
        let agent = self.agent();
        let mut req = agent
            .get(&self.url)
            .set("Accept", "application/json")
            .set("User-Agent", &self.user_agent);
        if let Some(value) = Self::conditional_header(etag) {
            req = req.set("If-None-Match", &value);
        }
        match req.call() {
            Ok(resp) => {
                let etag = resp.header("etag").unwrap_or("").to_string();
                match resp.into_string() {
                    Ok(body) => FetchOutcome::Fetched { etag, body },
                    Err(e) => FetchOutcome::Failed(format!("reading api.json body: {e}")),
                }
            }
            // `ureq` surfaces 3xx as an error; 304 is the one we want.
            Err(ureq::Error::Status(304, _)) => FetchOutcome::NotModified,
            Err(ureq::Error::Status(code, _)) => {
                FetchOutcome::Failed(format!("models.dev returned HTTP {code}"))
            }
            Err(ureq::Error::Transport(t)) => FetchOutcome::Failed(format!("transport: {t}")),
        }
    }
}

/// The result of the P56.3 activate-screen probe.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointProbe {
    pub ok: bool,
    /// HTTP status (0 = transport failure, nothing was answered).
    pub status: u16,
    pub message: String,
    /// How many models the endpoint advertised (0 when unknown).
    pub models: usize,
    pub url: String,
}

/// **P56.3 — the `MetadataOnly` probe.**
///
/// `GET {base}/models`, authenticated exactly the way the provider expects
/// (Anthropic wants `x-api-key`; everyone else `Authorization: Bearer`; a
/// keyless provider must send neither). Read-only by construction: this
/// function cannot persist anything, so a failed probe can never leave a
/// half-configured provider behind.
///
/// Both response shapes in the wild are counted: OpenAI-style `{data:[…]}`
/// and Anthropic/models.dev-style `{models:{…}}`. An empty-but-reachable
/// answer reports `models: 0` rather than pretending to be a failure.
pub fn probe_models_endpoint(
    base_url: &str,
    anthropic: bool,
    headers: &[(String, String)],
    key: Option<&str>,
) -> EndpointProbe {
    let base = base_url.trim().trim_end_matches('/');
    let url = format!("{base}/models");
    // One agent for this probe call. Session reuse for catalog refresh is
    // `HttpFetch::agent`; this probe is a single request.
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build();
    let mut req = agent.get(&url).set("Accept", "application/json").set(
        "User-Agent",
        &format!("AgentCowork/{}", env!("CARGO_PKG_VERSION")),
    );
    for (k, v) in headers {
        req = req.set(k, v);
    }
    if let Some(k) = key {
        let k = k.trim();
        if !k.is_empty() {
            req = if anthropic {
                req.set("x-api-key", k)
            } else {
                req.set("Authorization", &format!("Bearer {k}"))
            };
        }
    }
    match req.call() {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.into_string().unwrap_or_default();
            endpoint_probe_result(url, status, &body, None)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            endpoint_probe_result(url, code, &body, None)
        }
        Err(ureq::Error::Transport(t)) => endpoint_probe_result(url, 0, "", Some(&t.to_string())),
    }
}

/// **Shape one probe result (P44.4)** from an already-performed request.
///
/// The single place a probe outcome becomes an [`EndpointProbe`], so the shell's
/// user-key probe and the vault-mediated one (`agentcowork_vault::Broker::probe_models`,
/// which returns the raw status/body because it owns the credential) cannot
/// disagree about what `ok` / `models` / `message` mean.
///
/// `error` is the transport-failure detail; `None` means the endpoint answered —
/// with any status, including a rejection, which is a real observation rather
/// than a failure to observe. A transport failure reports `status: 0` and
/// `models: 0` and never borrows an HTTP code it did not receive.
pub fn endpoint_probe_result(
    url: String,
    status: u16,
    body: &str,
    error: Option<&str>,
) -> EndpointProbe {
    if let Some(detail) = error {
        return EndpointProbe {
            ok: false,
            status: 0,
            message: format!("transport: {detail}"),
            models: 0,
            url,
        };
    }
    let models = count_models(body);
    if (200..300).contains(&status) {
        return EndpointProbe {
            ok: true,
            status,
            message: if models > 0 {
                format!("{models} models advertised")
            } else {
                "reachable".to_string()
            },
            models,
            url,
        };
    }
    let detail: String = body.chars().take(200).collect();
    EndpointProbe {
        ok: false,
        status,
        message: if detail.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {detail}")
        },
        models: 0,
        url,
    }
}

/// Count advertised models across the two real response shapes.
///
/// Public because the vault-mediated probe path parses the body it received
/// instead of the body it fetched — the counting rule stays in one place.
pub fn count_models(body: &str) -> usize {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return 0;
    };
    v.get("data")
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .or_else(|| v.get("models").and_then(|m| m.as_object()).map(|o| o.len()))
        .unwrap_or(0)
}

/// What one job run produced (decision + the meta that is now on disk).
#[derive(Debug, Clone, PartialEq)]
pub struct RefreshOutcome {
    pub decision: RefreshDecision,
    pub meta: Option<CatalogMeta>,
    /// True when a new snapshot was written to disk.
    pub persisted: bool,
}

impl RefreshOutcome {
    pub fn summary(&self) -> String {
        self.decision.summary()
    }
}

/// The P56.1 job body: load meta → conditional GET → pure decision → persist.
///
/// Persistence rules:
/// * **Accepted + Updated** → write the snapshot and its meta.
/// * **NotModified** → keep the snapshot bytes; bump `fetched_at` in the meta
///   (a 304 is a successful freshness stamp, so the next 4h window starts
///   now, and the next conditional GET reuses the same ETag).
/// * **Rejected / Failed** → write nothing to the snapshot and record the
///   honest verdict in the meta (last good bytes keep serving).
pub fn refresh_now(store: &CatalogStore, client: &HttpFetch, now_ms: i64) -> RefreshOutcome {
    let prev_meta = store.load_meta();
    let prev = store.load();
    let etag = prev_meta.as_ref().and_then(CatalogMeta::if_none_match);
    let outcome = client.get(etag);
    let (snapshot, decision) = apply_refresh(prev.as_ref(), outcome, now_ms);
    let mut persisted = false;

    let meta = match (&snapshot, &decision) {
        (Some(snap), RefreshDecision::Updated { .. }) => {
            persisted = store.save(snap, &decision).map(|_| true).unwrap_or(false);
            store.load_meta()
        }
        (Some(snap), RefreshDecision::NotModified { .. }) => {
            let mut meta = CatalogMeta::from_snapshot(snap, &decision);
            // Keep the previous ETag unless this snapshot carries a newer one.
            if meta.etag.trim().is_empty() {
                if let Some(prev) = &prev_meta {
                    meta.etag.clone_from(&prev.etag);
                }
            }
            let _ = store.save_meta(&meta);
            store.load_meta()
        }
        _ => {
            // Nothing new landed: keep the stored bytes AND the stored
            // freshness stamp; only the verdict moves.
            let mut meta = prev_meta.unwrap_or_default();
            meta.last_decision = Some(decision.summary());
            meta.last_failed = !decision.accepted();
            let _ = store.save_meta(&meta);
            Some(meta)
        }
    };

    RefreshOutcome {
        decision,
        meta,
        persisted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_resolution_per_host_until_end_session() {
        let fetch = HttpFetch::new().with_url("http://127.0.0.1:1/api.json");
        let _ = fetch.get(None);
        let _ = fetch.get(None);
        assert_eq!(fetch.resolution_count(), 1);
        fetch.end_session();
        let _ = fetch.get(None);
        assert_eq!(fetch.resolution_count(), 2);
    }

    use crate::live::CatalogSnapshot;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-catalog-fetch-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn big_body(n: usize) -> String {
        let rows: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    r#""prov{i}":{{"id":"prov{i}","name":"Provider {i}","npm":"@ai-sdk/openai-compatible","api":"https://api.prov{i}.test/v1","env":["P{i}_KEY"],"models":{{"m-one":{{"id":"m-one","name":"M One","limit":{{"context":100000,"output":4096}},"cost":{{"input":1.0,"output":2.0}}}},"m-two":{{"id":"m-two","name":"M Two","limit":{{"context":8000,"output":2048}},"cost":{{"input":0.5,"output":1.0}}}}}}}}"#
                )
            })
            .collect();
        format!("{{{}}}", rows.join(","))
    }

    /// One-shot local HTTP fixture: serve `respond` to the first request and
    /// return the base URL. Keeps the probe tests off the live network.
    fn mock_server(respond: impl Fn(&str) -> (u16, String) + Send + 'static) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (code, body) = respond(&req);
                let head = format!(
                    "HTTP/1.1 {code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(body.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn probe_reports_models_and_sends_bearer_auth() {
        let base = mock_server(|req| {
            assert!(req.contains("GET /models"), "{req}");
            assert!(req.contains("Authorization: Bearer sk-test"), "{req}");
            (
                200,
                r#"{"data":[{"id":"a"},{"id":"b"},{"id":"c"}]}"#.to_string(),
            )
        });
        let probe = probe_models_endpoint(&base, false, &[], Some("sk-test"));
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.status, 200);
        assert_eq!(probe.models, 3);
        assert!(probe.message.contains("3 models"));
    }

    #[test]
    fn probe_uses_x_api_key_for_anthropic_and_counts_the_models_map() {
        let base = mock_server(|req| {
            assert!(req.contains("x-api-key: sk-ant"), "{req}");
            assert!(!req.contains("Authorization: Bearer"), "{req}");
            (
                200,
                r#"{"models":{"claude-x":{"id":"claude-x"}}}"#.to_string(),
            )
        });
        let probe = probe_models_endpoint(&base, true, &[], Some("sk-ant"));
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.models, 1);
    }

    #[test]
    fn probe_reports_a_bad_key_honestly() {
        let base = mock_server(|_req| (401, r#"{"error":"invalid api key"}"#.to_string()));
        let probe = probe_models_endpoint(&base, false, &[], Some("sk-wrong"));
        assert!(!probe.ok);
        assert_eq!(probe.status, 401);
        assert!(probe.message.contains("401"), "{probe:?}");
        assert_eq!(probe.models, 0);
    }

    #[test]
    fn probe_sends_no_auth_for_a_keyless_provider() {
        let base = mock_server(|req| {
            assert!(!req.to_ascii_lowercase().contains("authorization"), "{req}");
            (200, r#"{"data":[{"id":"big-pickle"}]}"#.to_string())
        });
        let probe = probe_models_endpoint(&base, false, &[], None);
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.models, 1);
    }

    #[test]
    fn probe_surfaces_a_dead_endpoint_as_a_transport_failure() {
        let probe = probe_models_endpoint("http://127.0.0.1:1", false, &[], Some("sk"));
        assert!(!probe.ok);
        assert_eq!(probe.status, 0);
        assert!(probe.message.contains("transport"), "{probe:?}");
    }

    #[test]
    fn conditional_header_only_when_an_etag_exists() {
        assert_eq!(HttpFetch::conditional_header(None), None);
        assert_eq!(HttpFetch::conditional_header(Some("  ")), None);
        assert_eq!(
            HttpFetch::conditional_header(Some("abc")),
            Some("abc".to_string())
        );
    }

    #[test]
    fn default_client_targets_the_one_json_url() {
        let c = HttpFetch::new();
        assert_eq!(c.url(), MODELS_DEV_API_URL);
        assert_eq!(c.url(), "https://models.dev/api.json");
    }

    #[test]
    fn failed_fetch_records_verdict_without_touching_the_snapshot() {
        let d = dir("failed");
        let store = CatalogStore::new(&d);
        let snap = CatalogSnapshot::parse("s", &big_body(120), 111, "e1").unwrap();
        let decision = RefreshDecision::Updated {
            providers: 120,
            models: 240,
            fetched_at: 111,
        };
        store.save(&snap, &decision).unwrap();

        // A client pointed at a dead port can't produce a snapshot.
        let client = HttpFetch::new()
            .with_url("http://127.0.0.1:1/api.json")
            .with_timeout(Duration::from_millis(500));
        // apply_refresh keeps the previous snapshot on failure, and the meta
        // records the honest verdict.
        let (kept, decision) = apply_refresh(Some(&snap), client.get(Some("e1")), 999);
        assert!(kept.is_some());
        assert!(!decision.accepted());
        let meta = CatalogMeta::from_snapshot(&snap, &decision);
        assert!(meta.last_failed);
        assert_eq!(meta.fetched_at, 111, "freshness stamp must not move");
        // Store still loads the good snapshot.
        assert_eq!(store.load().unwrap().provider_count(), 120);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refresh_failure_keeps_cached_snapshot_and_updates_meta_only() {
        let d = dir("offline");
        let store = CatalogStore::new(&d);
        let snap = CatalogSnapshot::parse("s", &big_body(120), 111, "e1").unwrap();
        store
            .save(
                &snap,
                &RefreshDecision::Updated {
                    providers: 120,
                    models: 240,
                    fetched_at: 111,
                },
            )
            .unwrap();

        // Simulate the pure failure path the job takes when the GET throws.
        let (next, decision) = apply_refresh(Some(&snap), FetchOutcome::Failed("dns".into()), 500);
        assert!(next.is_some());
        assert!(!decision.accepted());
        let mut meta = store.load_meta().unwrap();
        meta.last_decision = Some(decision.summary());
        meta.last_failed = true;
        store.save_meta(&meta).unwrap();
        let meta = store.load_meta().unwrap();
        assert!(meta.last_failed);
        assert!(meta.last_decision.unwrap().contains("dns"));
        assert_eq!(meta.etag, "e1", "ETag kept for the next conditional GET");
        assert_eq!(store.load().unwrap().fetched_at, 111);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_304_bumps_freshness_but_keeps_the_etag_and_bytes() {
        let d = dir("304");
        let store = CatalogStore::new(&d);
        let snap = CatalogSnapshot::parse("s", &big_body(120), 111, "e1").unwrap();
        store
            .save(
                &snap,
                &RefreshDecision::Updated {
                    providers: 120,
                    models: 240,
                    fetched_at: 111,
                },
            )
            .unwrap();
        let (next, decision) = apply_refresh(Some(&snap), FetchOutcome::NotModified, 777);
        let next = next.unwrap();
        assert!(decision.accepted());
        assert_eq!(next.fetched_at, 777);
        assert_eq!(next.etag, "e1");
        store
            .save(
                &next,
                &RefreshDecision::NotModified {
                    fetched_at: 777,
                    providers: 120,
                    models: 240,
                },
            )
            .unwrap();
        let meta = store.load_meta().unwrap();
        assert_eq!(meta.fetched_at, 777);
        assert_eq!(meta.if_none_match(), Some("e1"));
        assert!(!meta.last_failed);
        let _ = std::fs::remove_dir_all(&d);
    }
}
