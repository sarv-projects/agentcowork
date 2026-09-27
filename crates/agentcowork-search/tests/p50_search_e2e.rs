//! P50.5.2 — Real search E2E (live SearXNG cascade → bounded citable results
//! → cited report; provider outage + offline cases stay honest).
//!
//! Env contract (same discipline as P50.5.1):
//!   AGENTCOWORK_E2E_SEARXNG_URL=http://127.0.0.1:8888  (a live SearXNG instance)
//!   Local instance (verified 2026-09-06):
//!     docker run -d --name searxng -p 8888:8080 -v \
//!       <repo>/scripts/e2e/searxng-settings.yml:/etc/searxng/settings.yml:ro \
//!       searxng/searxng
//!   (the override ships `use_default_settings: true` + a fixed dev secret +
//!   `limiter: false` + `formats: [html, json]` — the image template's
//!   `use_default_settings` marker is REQUIRED or the file replaces the
//!   defaults and /search 500s on missing keys).
//!
//! When unset, the live legs SKIP with a message (release matrix runs them
//! where SearXNG exists). When set, they must PASS against the real endpoint.
//! The outage/offline legs need no network and always run: a dead endpoint
//! must surface `G8 cascade exhausted`, never an empty success, never a hang.

use std::time::Duration;

use agentcowork_search::{CitedReport, G8Cascade, SearchResult, SearchTransport};

/// Live HTTP transport over `ureq`: SearXNG JSON for named endpoints, honest
/// refusal for the `ddg` fallback slot (this harness ships no DDG scraper, so
/// it errors instead of faking results).
struct SearxngTransport;

impl SearchTransport for SearxngTransport {
    fn search(&self, endpoint: &str, query: &str) -> Result<Vec<SearchResult>, String> {
        if endpoint == "ddg" {
            return Err("p50 search E2E transport has no DDG scraper".into());
        }
        let url = format!("{}/search", endpoint.trim_end_matches('/'));
        let resp = ureq::get(&url)
            .query("q", query)
            .query("format", "json")
            .timeout(Duration::from_secs(20))
            .call()
            .map_err(|e| format!("searxng request failed: {e}"))?;
        let body = resp
            .into_string()
            .map_err(|e| format!("searxng read failed: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("searxng bad json: {e}"))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
            for r in arr {
                let url = r
                    .get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                if url.is_empty() {
                    continue;
                }
                out.push(SearchResult {
                    url,
                    title: r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: r
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                    source: endpoint.to_string(),
                });
            }
        }
        Ok(out)
    }

    fn fetch(&self, _tier: &str, url: &str) -> Result<String, String> {
        ureq::get(url)
            .timeout(Duration::from_secs(20))
            .call()
            .map_err(|e| format!("fetch failed: {e}"))?
            .into_string()
            .map_err(|e| format!("fetch read failed: {e}"))
    }
}

fn dead_port_url() -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

#[test]
fn real_search_live_cascade_reports_and_cites() {
    let Some(base) = agentcowork_types::env_compat::get("E2E_SEARXNG_URL") else {
        eprintln!(
            "SKIP P50.5.2 live leg: set AGENTCOWORK_E2E_SEARXNG_URL (e.g. http://127.0.0.1:8888 for SearXNG)"
        );
        return;
    };
    let transport = SearxngTransport;
    let cascade = G8Cascade::new(
        Duration::from_secs(300),
        vec![base.clone()],
        3,
        Duration::from_secs(60),
    );
    // Live cascade → bounded, citable results (every hit carries its URL).
    let hits = cascade
        .query(&transport, "rust programming language")
        .expect("P50.5.2: live SearXNG query failed");
    assert!(
        !hits.is_empty(),
        "P50.5.2: live SearXNG returned zero results"
    );
    assert!(
        hits.iter().all(|h| !h.url.is_empty()),
        "P50.5.2: uncitable hit (empty url) in live results"
    );
    eprintln!("P50.5.2: live cascade returned {} hits", hits.len());

    // Cache bound: the same query inside the TTL replays instead of
    // re-hitting the network (same cardinality, no error).
    let replay = cascade
        .query(&transport, "rust programming language")
        .expect("P50.5.2: cached replay failed");
    assert_eq!(
        replay.len(),
        hits.len(),
        "P50.5.2: cache replay diverged from the live answer"
    );

    // Cited report: claims carry their source URLs and render to markdown.
    let learnings: Vec<(String, Vec<String>)> = hits
        .iter()
        .take(3)
        .map(|h| {
            (
                format!(
                    "{} — {}",
                    h.title,
                    h.snippet.chars().take(120).collect::<String>()
                ),
                vec![h.url.clone()],
            )
        })
        .collect();
    let report = CitedReport::assemble("P50.5.2 probe", &learnings);
    assert!(
        (0.0..=1.0).contains(&report.confidence),
        "P50.5.2: confidence out of range: {}",
        report.confidence
    );
    let md = report.render_markdown();
    assert!(!md.is_empty(), "P50.5.2: empty report markdown");
    for (_, urls) in &learnings {
        for u in urls {
            assert!(
                md.contains(u.as_str()),
                "P50.5.2: citation {u} missing from the rendered report"
            );
        }
    }
    eprintln!(
        "P50.5.2: cited report ({} claims, confidence {:.2}) renders with citations",
        report.claims.len(),
        report.confidence
    );
}

#[test]
fn real_search_outage_is_honest() {
    // Provider outage: the only endpoint is dead. The cascade must exhaust
    // (breaker + ddg-fallback refusal) with an honest error — never an empty
    // Ok that looks like "no results", never a hang.
    let transport = SearxngTransport;
    let cascade = G8Cascade::new(
        Duration::from_secs(300),
        vec![dead_port_url()],
        3,
        Duration::from_secs(60),
    );
    let err = cascade
        .query(&transport, "rust programming language")
        .expect_err("P50.5.2: dead endpoint must not succeed");
    assert!(
        err.contains("exhausted"),
        "P50.5.2: outage must surface cascade exhaustion, got: {err}"
    );
    // Repeated outage keeps failing honestly (breaker path, still an error).
    let err2 = cascade
        .query(&transport, "rust programming language")
        .expect_err("P50.5.2: repeated outage must still fail");
    assert!(
        err2.contains("exhausted"),
        "P50.5.2: repeated outage must stay honest, got: {err2}"
    );
    eprintln!("P50.5.2: outage honestly reported: {err}");
}

#[test]
fn real_search_offline_is_honest() {
    // Offline: unroutable endpoint (nothing listens, nothing resolves on this
    // port). Same contract as outage — honest exhaustion, no fake results.
    let transport = SearxngTransport;
    let cascade = G8Cascade::new(
        Duration::from_secs(0),
        vec!["http://127.0.0.1:9".into()],
        1,
        Duration::from_secs(1),
    );
    let err = cascade
        .query(&transport, "anything")
        .expect_err("P50.5.2: offline query must not succeed");
    assert!(
        err.contains("exhausted"),
        "P50.5.2: offline must surface cascade exhaustion, got: {err}"
    );
    eprintln!("P50.5.2: offline honestly reported: {err}");
}
