//! P56.1 — pure catalog request/response shaping, plus the job body that ties
//! a host-owned fetch → pure decision → durable store together.
//!
//! [`HttpFetch::get`] shapes a **conditional** GET of
//! `https://models.dev/api.json` with `If-None-Match` and delegates all socket,
//! DNS, redirect, timeout, and response-boundary work to
//! [`CatalogHttpTransport`]. The four-hour job costs one 304 round-trip when
//! nothing changed instead of re-downloading 4.6 MB.
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
//! `refresh_now` is exercised against a fake host transport in tests (no local
//! server or live network). The catalog crate owns no network client.

use std::time::Duration;

use serde::Serialize;

use crate::live::{FetchOutcome, MODELS_DEV_API_URL, RefreshDecision, apply_refresh};
use crate::store::{CatalogMeta, CatalogStore};

/// Outbound timeout for the catalog fetch (the body is ~4.6 MB).
pub const FETCH_TIMEOUT_SECS: u64 = 30;

/// Maximum catalog response body accepted by the pure catalog layer.
pub const MAX_CATALOG_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// A response returned by the host-owned catalog transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogHttpResponse {
    /// HTTP status returned by the endpoint.
    pub status: u16,
    /// Allowlisted cache metadata (currently only ETag).
    pub etag: Option<String>,
    /// Bounded response body.
    pub body: Vec<u8>,
}

/// Host-owned network boundary. The catalog shapes requests and responses but
/// never resolves hosts, opens sockets, follows redirects, or owns credentials.
pub trait CatalogHttpTransport: Send + Sync {
    /// Perform one bounded GET using the host's egress policy.
    fn get(
        &self,
        url: &str,
        headers: &[(String, String)],
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<CatalogHttpResponse, String>;
}

/// Pure request configuration for the catalog's one JSON snapshot URL.
#[derive(Debug, Clone)]
pub struct HttpFetch {
    timeout: Duration,
    user_agent: String,
    /// Overridable for tests / a mirror. Never a different *path*: the
    /// catalog is only ever one JSON document.
    url: String,
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
        }
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

    /// One conditional GET through the caller's host-owned transport. Never
    /// panics: every failure is a [`FetchOutcome::Failed`].
    pub fn get(&self, transport: &dyn CatalogHttpTransport, etag: Option<&str>) -> FetchOutcome {
        let mut headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), self.user_agent.clone()),
        ];
        if let Some(value) = Self::conditional_header(etag) {
            headers.push(("If-None-Match".to_string(), value));
        }
        let response = match transport.get(
            &self.url,
            &headers,
            self.timeout,
            MAX_CATALOG_RESPONSE_BYTES,
        ) {
            Ok(response) => response,
            Err(error) => return FetchOutcome::Failed(error),
        };
        if response.body.len() > MAX_CATALOG_RESPONSE_BYTES {
            return FetchOutcome::Failed(
                "catalog response exceeded the configured byte limit".into(),
            );
        }
        match response.status {
            304 => FetchOutcome::NotModified,
            200..=299 => match String::from_utf8(response.body) {
                Ok(body) => FetchOutcome::Fetched {
                    etag: response.etag.unwrap_or_default(),
                    body,
                },
                Err(_) => FetchOutcome::Failed("catalog response was not valid UTF-8".into()),
            },
            status => FetchOutcome::Failed(format!("models.dev returned HTTP {status}")),
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
    transport: &dyn CatalogHttpTransport,
) -> EndpointProbe {
    let base = base_url.trim().trim_end_matches('/');
    let url = format!("{base}/models");
    let mut request_headers = vec![
        ("Accept".to_string(), "application/json".to_string()),
        (
            "User-Agent".to_string(),
            format!("AgentCowork/{}", env!("CARGO_PKG_VERSION")),
        ),
    ];
    request_headers.extend(headers.iter().cloned());
    if let Some(k) = key {
        let k = k.trim();
        if !k.is_empty() {
            request_headers.push(if anthropic {
                ("x-api-key".to_string(), k.to_string())
            } else {
                ("Authorization".to_string(), format!("Bearer {k}"))
            });
        }
    }
    match transport.get(&url, &request_headers, Duration::from_secs(20), 1024 * 1024) {
        Ok(response) if response.body.len() <= 1024 * 1024 => {
            let body = String::from_utf8_lossy(&response.body);
            endpoint_probe_result(url, response.status, &body, None)
        }
        Ok(_) => endpoint_probe_result(
            url,
            0,
            "",
            Some("provider response exceeded the configured byte limit"),
        ),
        Err(error) => endpoint_probe_result(url, 0, "", Some(&error)),
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
pub fn refresh_now(
    store: &CatalogStore,
    client: &HttpFetch,
    transport: &dyn CatalogHttpTransport,
    now_ms: i64,
) -> RefreshOutcome {
    let prev_meta = store.load_meta();
    let prev = store.load();
    let etag = prev_meta.as_ref().and_then(CatalogMeta::if_none_match);
    let outcome = client.get(transport, etag);
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
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeTransport {
        response: Mutex<Option<Result<CatalogHttpResponse, String>>>,
        requests: Mutex<Vec<(String, Vec<(String, String)>, Duration, usize)>>,
    }

    impl FakeTransport {
        fn responding(status: u16, body: &str, etag: Option<&str>) -> Self {
            Self {
                response: Mutex::new(Some(Ok(CatalogHttpResponse {
                    status,
                    etag: etag.map(str::to_string),
                    body: body.as_bytes().to_vec(),
                }))),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                response: Mutex::new(Some(Err(message.to_string()))),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl CatalogHttpTransport for FakeTransport {
        fn get(
            &self,
            url: &str,
            headers: &[(String, String)],
            timeout: Duration,
            max_response_bytes: usize,
        ) -> Result<CatalogHttpResponse, String> {
            self.requests.lock().unwrap().push((
                url.to_string(),
                headers.to_vec(),
                timeout,
                max_response_bytes,
            ));
            self.response
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err("fixture response already consumed".into()))
        }
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

    #[test]
    fn probe_reports_models_and_sends_bearer_auth_through_transport() {
        let transport =
            FakeTransport::responding(200, r#"{"data":[{"id":"a"},{"id":"b"},{"id":"c"}]}"#, None);
        let probe = probe_models_endpoint(
            "https://provider.example/v1",
            false,
            &[],
            Some("sk-test"),
            &transport,
        );
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.status, 200);
        assert_eq!(probe.models, 3);
        assert!(probe.message.contains("3 models"));
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests[0].0, "https://provider.example/v1/models");
        assert!(
            requests[0]
                .1
                .iter()
                .any(|(name, value)| { name == "Authorization" && value == "Bearer sk-test" })
        );
        assert_eq!(requests[0].2, Duration::from_secs(20));
    }

    #[test]
    fn probe_uses_x_api_key_for_anthropic_and_counts_models_map() {
        let transport =
            FakeTransport::responding(200, r#"{"models":{"claude-x":{"id":"claude-x"}}}"#, None);
        let probe = probe_models_endpoint(
            "https://provider.example",
            true,
            &[],
            Some("sk-ant"),
            &transport,
        );
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.models, 1);
        let requests = transport.requests.lock().unwrap();
        assert!(
            requests[0]
                .1
                .iter()
                .any(|(name, value)| name == "x-api-key" && value == "sk-ant")
        );
        assert!(
            !requests[0]
                .1
                .iter()
                .any(|(name, _)| name == "Authorization")
        );
    }

    #[test]
    fn probe_reports_a_bad_key_honestly() {
        let transport = FakeTransport::responding(401, r#"{"error":"invalid api key"}"#, None);
        let probe = probe_models_endpoint(
            "https://provider.example",
            false,
            &[],
            Some("sk-wrong"),
            &transport,
        );
        assert!(!probe.ok);
        assert_eq!(probe.status, 401);
        assert!(probe.message.contains("401"), "{probe:?}");
        assert_eq!(probe.models, 0);
    }

    #[test]
    fn probe_sends_no_auth_for_a_keyless_provider() {
        let transport = FakeTransport::responding(200, r#"{"data":[{"id":"big-pickle"}]}"#, None);
        let probe = probe_models_endpoint("https://provider.example", false, &[], None, &transport);
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.models, 1);
        assert!(
            !transport.requests.lock().unwrap()[0]
                .1
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        );
    }

    #[test]
    fn probe_surfaces_transport_failure_without_claiming_a_status() {
        let transport = FakeTransport::failing("Guard refused destination");
        let probe = probe_models_endpoint(
            "https://provider.example",
            false,
            &[],
            Some("sk"),
            &transport,
        );
        assert!(!probe.ok);
        assert_eq!(probe.status, 0);
        assert!(probe.message.contains("transport"), "{probe:?}");
    }

    #[test]
    fn conditional_get_passes_etag_and_enforces_catalog_bounds() {
        let transport = FakeTransport::responding(200, r#"{"providers":{}}"#, Some("e2"));
        let fetch = HttpFetch::new();
        assert!(matches!(
            fetch.get(&transport, Some(" e1 ")),
            FetchOutcome::Fetched { ref etag, .. } if etag == "e2"
        ));
        let requests = transport.requests.lock().unwrap();
        assert!(
            requests[0]
                .1
                .iter()
                .any(|(name, value)| name == "If-None-Match" && value == "e1")
        );
        assert_eq!(requests[0].3, MAX_CATALOG_RESPONSE_BYTES);
        assert_eq!(requests[0].2, Duration::from_secs(FETCH_TIMEOUT_SECS));
    }

    #[test]
    fn conditional_get_handles_not_modified_and_rejects_oversized_or_invalid_body() {
        let not_modified = FakeTransport::responding(304, "", None);
        assert_eq!(
            HttpFetch::new().get(&not_modified, Some("e1")),
            FetchOutcome::NotModified
        );

        let oversized =
            FakeTransport::responding(200, &"x".repeat(MAX_CATALOG_RESPONSE_BYTES + 1), None);
        assert!(matches!(
            HttpFetch::new().get(&oversized, None),
            FetchOutcome::Failed(message) if message.contains("byte limit")
        ));

        let invalid = FakeTransport {
            response: Mutex::new(Some(Ok(CatalogHttpResponse {
                status: 200,
                etag: None,
                body: vec![0xff],
            }))),
            requests: Mutex::new(Vec::new()),
        };
        let _ = invalid;
        assert!(matches!(
            HttpFetch::new().get(&invalid, None),
            FetchOutcome::Failed(message) if message.contains("UTF-8")
        ));
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
    fn failed_fetch_preserves_the_last_good_snapshot() {
        let d = dir("failed");
        let store = CatalogStore::new(&d);
        let snap = CatalogSnapshot::parse("s", &big_body(120), 111, "e1").unwrap();
        let decision = RefreshDecision::Updated {
            providers: 120,
            models: 240,
            fetched_at: 111,
        };
        store.save(&snap, &decision).unwrap();
        let transport = FakeTransport::failing("Guard refused destination");
        let (kept, decision) = apply_refresh(
            Some(&snap),
            HttpFetch::new().get(&transport, Some("e1")),
            999,
        );
        assert!(kept.is_some());
        assert!(!decision.accepted());
        let meta = CatalogMeta::from_snapshot(&snap, &decision);
        assert!(meta.last_failed);
        assert_eq!(meta.fetched_at, 111, "freshness stamp must not move");
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
