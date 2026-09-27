//! agentcowork-search (P8.4) — search & research.
//!
//! The G8 tiered cascade, deep-research tree, multi-channel adapters, and
//! cited report generation. Everything is pure logic over an injected
//! [`SearchTransport`] seam: live SearXNG/DDG/WebSurfx calls are a runtime
//! wiring concern (HTTP via `ureq`), but the cascade routing, caching,
//! circuit-breaking, deep-research tree, and report assembly are fully
//! testable with injected results and no network.
//!
//! - [`SearchResult`] / [`SearchTransport`] — the normalized result + the
//!   injected HTTP seam.
//! - [`G8Cascade`] — the tiered result cache (TTL) → SearXNG →
//!   circuit-breaker fallback, with health-gated instance selection.
//! - [`DeepResearch`] — G2 breadth×depth tree with learnings-up + gap-check.
//! - [`Channel`] adapters (G3): arXiv, GitHub, EDGAR, Reddit — query builders
//!   + result normalizers (pure; live fetch via the transport seam).
//! - [`CitedReport`] — G cited report generation with confidence metrics.
//! - [`ParallelFetchCascade`] — the searxng-mcp 4-tier fetch cascade
//!   (Firecrawl → Crawl4AI → raw → Wayback) with per-page fallback.
//! - [`searx_space`] — the `searx.space` public-instance feed (P55.8): parse,
//!   eligibility filter, latency order, cache, and an honest offline fallback.

pub mod searx_space;

pub use searx_space::{
    FEED_TTL, FeedError, FeedSource, INSTANCES_FEED_URL, InstanceFeedTransport, SearxInstance,
    SearxSpaceFeed, parse_instances,
};

use std::collections::HashMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Normalized result + transport seam
// ---------------------------------------------------------------------------

/// One normalized search hit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
    /// Which tier/source produced this result.
    pub source: String,
}

/// One fetched page (the parallel-fetch cascade output).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchedPage {
    pub url: String,
    pub text: String,
    /// Which fetch tier succeeded ("firecrawl" / "crawl4ai" / "raw" / "wayback").
    pub tier: String,
}

/// The injected HTTP/search seam. Live implementations use `ureq`; tests
/// inject scripted responses so the cascade logic is fully testable offline.
pub trait SearchTransport: Send + Sync {
    /// Query a search backend (SearXNG / DDG / WebSurfx) at the given endpoint.
    fn search(&self, endpoint: &str, query: &str) -> Result<Vec<SearchResult>, String>;
    /// Fetch a page's readable text (the 4-tier fetch cascade calls this per
    /// tier until one succeeds).
    fn fetch(&self, tier: &str, url: &str) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// G8 tiered cascade (Algorithm #33 routing)
// ---------------------------------------------------------------------------

/// A cached query result with a TTL.
#[derive(Debug, Clone)]
struct CacheEntry {
    results: Vec<SearchResult>,
    at: Instant,
}

/// The G8 tiered cascade: cache (5-min TTL) → SearXNG (health-gated) →
/// circuit-breaker fallback. Instance health is tracked so a downed
/// SearXNG instance is skipped for a cooldown period.
#[derive(Debug)]
pub struct G8Cascade {
    cache: std::sync::Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Duration,
    /// Candidate SearXNG endpoints, tried in order (health-gated).
    endpoints: Vec<String>,
    /// Instance health: endpoint → (consecutive failures, cooldown until).
    health: std::sync::Mutex<HashMap<String, (u32, Option<Instant>)>>,
    failure_threshold: u32,
    cooldown: Duration,
}

/// The local-first SearXNG endpoint list (P55.8 — one owner: the default
/// cascade *and* the shell's config resolver both read this, so the local-first
/// posture cannot drift between them). Public instances are never added here;
/// they are an explicit opt-in.
pub const DEFAULT_SEARX_ENDPOINTS: &[&str] = &["http://localhost:8080", "http://localhost:8081"];

impl Default for G8Cascade {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(300),
            DEFAULT_SEARX_ENDPOINTS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            3,
            Duration::from_secs(60),
        )
    }
}

impl G8Cascade {
    pub fn new(
        cache_ttl: Duration,
        endpoints: Vec<String>,
        failure_threshold: u32,
        cooldown: Duration,
    ) -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl,
            endpoints,
            health: std::sync::Mutex::new(HashMap::new()),
            failure_threshold,
            cooldown,
        }
    }

    /// P55.8 — the endpoint list the cascade will try, in order.
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// P55.8 — replace the endpoint list (P55.8: local SearXNG first, then any
    /// public instances the user explicitly opted into). Health state for
    /// endpoints that survived is kept; the result cache is dropped because the
    /// routing decision it recorded may no longer hold.
    pub fn set_endpoints(&mut self, endpoints: Vec<String>) {
        if self.endpoints == endpoints {
            return;
        }
        self.endpoints = endpoints;
        if let Ok(mut health) = self.health.lock() {
            health.retain(|endpoint, _| self.endpoints.contains(endpoint));
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Run the cascade for a query. Returns the source tier in each result.
    pub fn query(
        &self,
        transport: &dyn SearchTransport,
        query: &str,
    ) -> Result<Vec<SearchResult>, String> {
        // Tier 1: cache.
        if let Some(entry) = self.cache.lock().unwrap().get(query) {
            if entry.at.elapsed() < self.cache_ttl {
                return Ok(entry.results.clone());
            }
        }
        // Tier 2: SearXNG (health-gated).
        for endpoint in &self.endpoints {
            if !self.is_healthy(endpoint) {
                continue;
            }
            match transport.search(endpoint, query) {
                Ok(results) => {
                    self.record_success(endpoint);
                    let mut results = results;
                    for r in &mut results {
                        r.source = format!("searxng:{endpoint}");
                    }
                    self.cache.lock().unwrap().insert(
                        query.to_string(),
                        CacheEntry {
                            results: results.clone(),
                            at: Instant::now(),
                        },
                    );
                    return Ok(results);
                }
                Err(_) => self.record_failure(endpoint),
            }
        }
        // Tier 3: circuit-breaker fallback — DDG public (no instance needed).
        match transport.search("ddg", query) {
            Ok(mut results) => {
                for r in &mut results {
                    r.source = "ddg:fallback".to_string();
                }
                Ok(results)
            }
            Err(e) => Err(format!("G8 cascade exhausted: {e}")),
        }
    }

    fn is_healthy(&self, endpoint: &str) -> bool {
        let health = self.health.lock().unwrap();
        match health.get(endpoint) {
            Some((failures, cooldown_until)) => {
                if *failures >= self.failure_threshold {
                    if let Some(until) = cooldown_until {
                        return Instant::now() >= *until;
                    }
                    return false;
                }
                true
            }
            None => true,
        }
    }

    fn record_success(&self, endpoint: &str) {
        self.health.lock().unwrap().remove(endpoint);
    }

    fn record_failure(&self, endpoint: &str) {
        let mut health = self.health.lock().unwrap();
        let entry = health.entry(endpoint.to_string()).or_insert((0, None));
        entry.0 += 1;
        if entry.0 >= self.failure_threshold {
            entry.1 = Some(Instant::now() + self.cooldown);
        }
    }
}

// ---------------------------------------------------------------------------
// Deep research (G2): breadth × depth tree + learnings-up + gap-check
// ---------------------------------------------------------------------------

/// One research node in the breadth×depth tree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResearchNode {
    pub query: String,
    pub depth: usize,
    pub breadth_index: usize,
    pub results: Vec<SearchResult>,
    pub learnings: Vec<String>,
    pub children: Vec<ResearchNode>,
}

/// Configuration for a deep-research run.
#[derive(Debug, Clone)]
pub struct DeepResearchConfig {
    pub breadth: usize,
    pub depth: usize,
    pub max_learnings: usize,
}

impl Default for DeepResearchConfig {
    fn default() -> Self {
        Self {
            breadth: 3,
            depth: 2,
            max_learnings: 20,
        }
    }
}

/// The G2 deep-research tree builder. Learnings bubble up from leaves; a
/// gap-check flags queries whose results share no learnings with siblings
/// (a coverage gap worth a follow-up).
pub struct DeepResearch;

impl DeepResearch {
    /// Build the tree for a seed query. `generate_subqueries` produces the
    /// breadth children for a node from its accumulated learnings (injected so
    /// the tree logic is testable without an LLM).
    pub fn build(
        transport: &dyn SearchTransport,
        cascade: &G8Cascade,
        seed: &str,
        config: &DeepResearchConfig,
        generate_subqueries: &dyn Fn(&str, &[String]) -> Vec<String>,
    ) -> Result<ResearchNode, String> {
        Self::build_node(transport, cascade, seed, 0, 0, config, generate_subqueries)
    }

    fn build_node(
        transport: &dyn SearchTransport,
        cascade: &G8Cascade,
        query: &str,
        depth: usize,
        breadth_index: usize,
        config: &DeepResearchConfig,
        generate_subqueries: &dyn Fn(&str, &[String]) -> Vec<String>,
    ) -> Result<ResearchNode, String> {
        let results = cascade.query(transport, query)?;
        let learnings = Self::extract_learnings(&results, config.max_learnings);
        let mut children = Vec::new();
        if depth + 1 < config.depth {
            let subs = generate_subqueries(query, &learnings);
            for (i, sub) in subs.iter().take(config.breadth).enumerate() {
                let child = Self::build_node(
                    transport,
                    cascade,
                    sub,
                    depth + 1,
                    i,
                    config,
                    generate_subqueries,
                )?;
                children.push(child);
            }
        }
        Ok(ResearchNode {
            query: query.to_string(),
            depth,
            breadth_index,
            results,
            learnings: learnings.clone(),
            children,
        })
    }

    /// Extract short learnings (title + snippet first sentence) from results.
    fn extract_learnings(results: &[SearchResult], max: usize) -> Vec<String> {
        results
            .iter()
            .take(max)
            .map(|r| {
                let snippet = r.snippet.split('.').next().unwrap_or(&r.snippet);
                format!("{} — {}", r.title, snippet)
            })
            .collect()
    }

    /// Gap-check: which leaf queries produced no learnings shared with any
    /// sibling (a coverage gap).
    pub fn gap_check(node: &ResearchNode) -> Vec<String> {
        let mut gaps = Vec::new();
        Self::collect_gaps(node, &mut gaps);
        gaps
    }

    fn collect_gaps(node: &ResearchNode, gaps: &mut Vec<String>) {
        if node.children.is_empty() {
            if node.learnings.is_empty() {
                gaps.push(node.query.clone());
            }
            return;
        }
        for child in &node.children {
            Self::collect_gaps(child, gaps);
        }
    }

    /// Flatten all learnings across the tree (breadth-first).
    pub fn all_learnings(node: &ResearchNode) -> Vec<String> {
        let mut out = node.learnings.clone();
        for child in &node.children {
            out.extend(Self::all_learnings(child));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Multi-channel adapters (G3): arXiv, GitHub, EDGAR, Reddit
// ---------------------------------------------------------------------------

/// A search channel: query-builder + result-normalizer. The transport fetches
/// the raw JSON; the channel normalizes it into [`SearchResult`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Arxiv,
    Github,
    Edgar,
    Reddit,
}

fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push('%');
            out.push(
                char::from_digit((b >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit((b & 0xF) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Channel::Arxiv => "arxiv",
            Channel::Github => "github",
            Channel::Edgar => "edgar",
            Channel::Reddit => "reddit",
        }
    }

    /// Build the endpoint URL + query for this channel. The user-supplied
    /// query is percent-encoded before interpolation so a query can never
    /// inject extra parameters or break the URL shape (bugfix: unsigned
    /// search URL builders).
    pub fn build_query(&self, query: &str) -> String {
        let q = percent_encode_query(query);
        match self {
            Channel::Arxiv => {
                format!("http://export.arxiv.org/api/query?search_query=all:{q}&max_results=5")
            }
            Channel::Github => {
                format!("https://api.github.com/search/repositories?q={q}&per_page=5")
            }
            Channel::Edgar => {
                format!("https://efts.sec.gov/LATEST/search-index?q={q}")
            }
            Channel::Reddit => {
                format!("https://www.reddit.com/search.json?q={q}&limit=5")
            }
        }
    }

    /// Normalize a raw JSON payload (from the transport) into results. The
    /// payload shape is channel-specific; this is pure parsing.
    pub fn normalize(&self, payload: &serde_json::Value) -> Vec<SearchResult> {
        match self {
            Channel::Arxiv => {
                let empty = Vec::new();
                let entries = payload
                    .get("feed")
                    .and_then(|f| f.get("entry"))
                    .and_then(|e| e.as_array())
                    .unwrap_or(&empty);
                entries
                    .iter()
                    .map(|e| SearchResult {
                        url: e
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        title: e
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        snippet: e
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .chars()
                            .take(200)
                            .collect(),
                        source: "arxiv".to_string(),
                    })
                    .collect()
            }
            Channel::Github => {
                let empty = Vec::new();
                let items = payload
                    .get("items")
                    .and_then(|i| i.as_array())
                    .unwrap_or(&empty);
                items
                    .iter()
                    .map(|i| SearchResult {
                        url: i
                            .get("html_url")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        title: i
                            .get("full_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        snippet: i
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        source: "github".to_string(),
                    })
                    .collect()
            }
            Channel::Edgar => {
                let empty = Vec::new();
                let hits = payload
                    .get("hits")
                    .and_then(|h| h.get("hits"))
                    .and_then(|h| h.as_array())
                    .unwrap_or(&empty);
                hits.iter()
                    .map(|h| SearchResult {
                        url: h
                            .get("_source")
                            .and_then(|s| s.get("link"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        title: h
                            .get("_source")
                            .and_then(|s| s.get("entity_name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        snippet: h
                            .get("_source")
                            .and_then(|s| s.get("form_type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        source: "edgar".to_string(),
                    })
                    .collect()
            }
            Channel::Reddit => {
                let empty = Vec::new();
                let posts = payload
                    .pointer("/data/children")
                    .and_then(|c| c.as_array())
                    .unwrap_or(&empty);
                posts
                    .iter()
                    .map(|p| {
                        let d = p.get("data").unwrap_or(&serde_json::Value::Null);
                        SearchResult {
                            url: format!(
                                "https://reddit.com{}",
                                d.get("permalink").and_then(|v| v.as_str()).unwrap_or("")
                            ),
                            title: d
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            snippet: d
                                .get("selftext")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .chars()
                                .take(200)
                                .collect(),
                            source: "reddit".to_string(),
                        }
                    })
                    .collect()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cited report (G): confidence metrics
// ---------------------------------------------------------------------------

/// A cited claim in the report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CitedClaim {
    pub claim: String,
    pub citations: Vec<String>,
}

/// A generated research report with confidence metrics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CitedReport {
    pub title: String,
    pub claims: Vec<CitedClaim>,
    /// Confidence: fraction of claims with ≥1 citation.
    pub confidence: f64,
    /// Number of distinct sources cited.
    pub sources_count: usize,
}

impl CitedReport {
    /// Assemble a report from research learnings + their source URLs.
    /// Learnings without a citation are kept but lower the confidence.
    pub fn assemble(title: &str, learnings: &[(String, Vec<String>)]) -> Self {
        let claims: Vec<CitedClaim> = learnings
            .iter()
            .map(|(claim, citations)| CitedClaim {
                claim: claim.clone(),
                citations: citations.clone(),
            })
            .collect();
        let cited = claims.iter().filter(|c| !c.citations.is_empty()).count();
        let confidence = if claims.is_empty() {
            0.0
        } else {
            cited as f64 / claims.len() as f64
        };
        let sources_count = claims
            .iter()
            .flat_map(|c| c.citations.iter().cloned())
            .collect::<std::collections::HashSet<_>>()
            .len();
        CitedReport {
            title: title.to_string(),
            claims,
            confidence,
            sources_count,
        }
    }

    /// Render the report as Markdown with inline citations.
    pub fn render_markdown(&self) -> String {
        let mut out = format!(
            "# {}\n\n*Confidence: {:.0}% · {} sources*\n\n",
            self.title,
            self.confidence * 100.0,
            self.sources_count
        );
        for (i, claim) in self.claims.iter().enumerate() {
            let cites = if claim.citations.is_empty() {
                "*(uncited)*".to_string()
            } else {
                claim
                    .citations
                    .iter()
                    .enumerate()
                    .map(|(j, url)| format!("[{i}.{j}]({url})"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            out.push_str(&format!("{} {}\n\n", claim.claim, cites));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Parallel top-N fetch cascade (searxng-mcp 4-tier pattern)
// ---------------------------------------------------------------------------

/// The 4-tier fetch cascade: Firecrawl → Crawl4AI → raw → Wayback. Each page
/// tries tiers in order until one returns text.
pub struct ParallelFetchCascade;

/// The fetch tiers, tried in order.
pub const FETCH_TIERS: &[&str] = &["firecrawl", "crawl4ai", "raw", "wayback"];

impl ParallelFetchCascade {
    /// Fetch a set of URLs in parallel-ish order (sequential here; the
    /// coordinator can fan out). Each URL falls back through tiers.
    pub fn fetch_all(transport: &dyn SearchTransport, urls: &[String]) -> Vec<FetchedPage> {
        urls.iter()
            .map(|url| Self::fetch_one(transport, url))
            .collect()
    }

    /// Fetch one URL, falling back through tiers.
    pub fn fetch_one(transport: &dyn SearchTransport, url: &str) -> FetchedPage {
        for tier in FETCH_TIERS {
            if let Ok(text) = transport.fetch(tier, url) {
                if !text.trim().is_empty() {
                    return FetchedPage {
                        url: url.to_string(),
                        text,
                        tier: tier.to_string(),
                    };
                }
            }
        }
        FetchedPage {
            url: url.to_string(),
            text: String::new(),
            tier: "failed".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Site/domain search (SeekStorm-class inverted index, simplified)
// ---------------------------------------------------------------------------

/// A simple in-memory inverted index for site/domain search (SeekStorm-class
/// pattern, simplified): index documents, query by token overlap with TF-IDF.
#[derive(Debug, Clone, Default)]
pub struct SiteIndex {
    docs: Vec<(String, String)>, // (url, text)
    /// token → (doc_index, tf).
    postings: HashMap<String, Vec<(usize, usize)>>,
}

impl SiteIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, url: &str, text: &str) {
        let idx = self.docs.len();
        self.docs.push((url.to_string(), text.to_string()));
        let mut tf: HashMap<String, usize> = HashMap::new();
        for tok in Self::tokenize(text) {
            *tf.entry(tok).or_insert(0) += 1;
        }
        for (tok, count) in tf {
            self.postings.entry(tok).or_default().push((idx, count));
        }
    }

    /// Query the index. Returns (url, score) pairs, best first.
    pub fn search(&self, query: &str, k: usize) -> Vec<(String, f64)> {
        let n = self.docs.len() as f64;
        if n == 0.0 {
            return Vec::new();
        }
        let mut scores: HashMap<usize, f64> = HashMap::new();
        for tok in Self::tokenize(query) {
            let postings = match self.postings.get(&tok) {
                Some(p) => p,
                None => continue,
            };
            let df = postings.len() as f64;
            let idf = (n / df).ln() + 1.0;
            for &(doc_idx, tf) in postings {
                *scores.entry(doc_idx).or_insert(0.0) += tf as f64 * idf;
            }
        }
        let mut ranked: Vec<(usize, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked
            .into_iter()
            .take(k)
            .filter_map(|(idx, score)| self.docs.get(idx).map(|(url, _)| (url.clone(), score)))
            .collect()
    }

    fn tokenize(s: &str) -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1)
            .map(|w| w.to_ascii_lowercase())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Repo-wide engineering (G5): workspace scan → dependency map → test-loop → patch
// ---------------------------------------------------------------------------

/// A dependency edge in the repo-wide dependency map.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DepEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

/// The G5 repo-wide engineering scan. Given a manifest's dependency list,
/// build a dependency map and identify affected files for a patch.
pub struct RepoWideScan;

impl RepoWideScan {
    /// Build a dependency map from a workspace manifest (e.g. Cargo.toml
    /// `[dependencies]` lines, parsed as `crate-name` entries).
    pub fn dependency_map(manifest_deps: &[(String, String)]) -> Vec<DepEdge> {
        manifest_deps
            .iter()
            .map(|(from, to)| DepEdge {
                from: from.clone(),
                to: to.clone(),
                kind: "depends_on".to_string(),
            })
            .collect()
    }

    /// Given a dependency map + a changed crate, return the set of crates
    /// that transitively depend on it (the blast radius for a patch).
    pub fn blast_radius(edges: &[DepEdge], changed: &str) -> Vec<String> {
        let mut affected = std::collections::HashSet::new();
        affected.insert(changed.to_string());
        let mut progress = true;
        while progress {
            progress = false;
            for edge in edges {
                if affected.contains(&edge.to) && affected.insert(edge.from.clone()) {
                    progress = true;
                }
            }
        }
        // Bugfix 7 — the boolean above used to shadow the `changed` param, so
        // the filter compared against the literal "false" and left the changed
        // crate itself in the result. The changed crate is the *trigger*, not
        // part of its own blast radius; exclude it by name.
        affected.into_iter().filter(|c| c != changed).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted transport for deterministic tests.
    struct ScriptedTransport {
        search_responses: Mutex<HashMap<String, Vec<SearchResult>>>,
        fetch_responses: Mutex<HashMap<String, String>>,
    }

    impl ScriptedTransport {
        fn new() -> Self {
            Self {
                search_responses: Mutex::new(HashMap::new()),
                fetch_responses: Mutex::new(HashMap::new()),
            }
        }

        fn with_search(self, key: &str, results: Vec<SearchResult>) -> Self {
            self.search_responses
                .lock()
                .unwrap()
                .insert(key.to_string(), results);
            self
        }

        fn with_fetch(self, tier_url: &str, text: &str) -> Self {
            self.fetch_responses
                .lock()
                .unwrap()
                .insert(tier_url.to_string(), text.to_string());
            self
        }
    }

    impl SearchTransport for ScriptedTransport {
        fn search(&self, endpoint: &str, query: &str) -> Result<Vec<SearchResult>, String> {
            let key = format!("{endpoint}|{query}");
            self.search_responses
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .ok_or_else(|| format!("no scripted response for {key}"))
        }

        fn fetch(&self, tier: &str, url: &str) -> Result<String, String> {
            let key = format!("{tier}|{url}");
            self.fetch_responses
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .ok_or_else(|| format!("no scripted fetch for {key}"))
        }
    }

    fn hit(url: &str, title: &str) -> SearchResult {
        SearchResult {
            url: url.to_string(),
            title: title.to_string(),
            snippet: format!("Snippet about {title}."),
            source: String::new(),
        }
    }

    #[test]
    fn g8_cascade_uses_cache_on_second_query() {
        let t = ScriptedTransport::new().with_search(
            "http://localhost:8080|rust",
            vec![hit("https://r1", "Rust lang")],
        );
        let c = G8Cascade::default();
        let r1 = c.query(&t, "rust").unwrap();
        assert_eq!(r1.len(), 1);
        assert!(r1[0].source.contains("searxng"));
        // Second call hits cache (same results, no transport call needed).
        let r2 = c.query(&t, "rust").unwrap();
        assert_eq!(r2, r1);
    }

    #[test]
    fn g8_cascade_falls_back_to_ddg_when_searxng_fails() {
        // No scripted SearXNG response → failure → DDG fallback.
        let t = ScriptedTransport::new()
            .with_search("ddg|rust", vec![hit("https://ddg1", "Rust via DDG")]);
        let c = G8Cascade::default();
        let r = c.query(&t, "rust").unwrap();
        assert_eq!(r[0].source, "ddg:fallback");
        assert_eq!(r[0].title, "Rust via DDG");
    }

    #[test]
    fn g8_cascade_circuit_breakers_unhealthy_instance() {
        let t =
            ScriptedTransport::new().with_search("ddg|x", vec![hit("https://ddg", "DDG result")]);
        let c = G8Cascade::new(
            Duration::from_secs(300),
            vec!["http://localhost:8080".to_string()],
            2,
            Duration::from_millis(10),
        );
        // Two failures trip the breaker.
        for _ in 0..2 {
            let _ = c.query(&t, "x");
        }
        // Now the instance is in cooldown; the cascade should go straight to DDG.
        let r = c.query(&t, "x").unwrap();
        assert_eq!(r[0].source, "ddg:fallback");
    }

    #[test]
    fn deep_research_builds_tree_and_bubbles_learnings() {
        let t = ScriptedTransport::new()
            .with_search(
                "http://localhost:8080|seed",
                vec![hit("https://s1", "Seed result")],
            )
            .with_search(
                "http://localhost:8080|sub1",
                vec![hit("https://s2", "Sub result 1")],
            )
            .with_search(
                "http://localhost:8080|sub2",
                vec![hit("https://s3", "Sub result 2")],
            );
        let c = G8Cascade::default();
        let cfg = DeepResearchConfig {
            breadth: 2,
            depth: 2,
            max_learnings: 5,
        };
        let tree = DeepResearch::build(&t, &c, "seed", &cfg, &|_q, _learnings| {
            vec!["sub1".to_string(), "sub2".to_string()]
        })
        .unwrap();
        assert_eq!(tree.depth, 0);
        assert_eq!(tree.children.len(), 2);
        assert_eq!(tree.children[0].query, "sub1");
        assert!(!DeepResearch::all_learnings(&tree).is_empty());
    }

    #[test]
    fn deep_research_gap_check_flags_empty_leaves() {
        let t = ScriptedTransport::new()
            .with_search(
                "http://localhost:8080|seed",
                vec![hit("https://s1", "Seed")],
            )
            .with_search("http://localhost:8080|empty", vec![]);
        let c = G8Cascade::default();
        let cfg = DeepResearchConfig {
            breadth: 1,
            depth: 2,
            max_learnings: 5,
        };
        let tree =
            DeepResearch::build(&t, &c, "seed", &cfg, &|_, _| vec!["empty".to_string()]).unwrap();
        let gaps = DeepResearch::gap_check(&tree);
        assert_eq!(gaps, vec!["empty"]);
    }

    #[test]
    fn channel_normalize_arxiv() {
        let payload = serde_json::json!({
            "feed": {
                "entry": [
                    { "id": "http://arxiv.org/1", "title": "Paper A", "summary": "A summary." }
                ]
            }
        });
        let results = Channel::Arxiv.normalize(&payload);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "http://arxiv.org/1");
        assert_eq!(results[0].title, "Paper A");
        assert_eq!(results[0].source, "arxiv");
    }

    #[test]
    fn channel_normalize_github() {
        let payload = serde_json::json!({
            "items": [
                { "html_url": "https://github.com/x/y", "full_name": "x/y", "description": "A repo." }
            ]
        });
        let results = Channel::Github.normalize(&payload);
        assert_eq!(results[0].url, "https://github.com/x/y");
        assert_eq!(results[0].title, "x/y");
    }

    #[test]
    fn channel_query_builders() {
        assert!(
            Channel::Arxiv
                .build_query("transformers")
                .contains("all:transformers")
        );
        assert!(Channel::Github.build_query("rust").contains("q=rust"));
        assert!(Channel::Edgar.build_query("apple").contains("apple"));
        assert!(Channel::Reddit.build_query("ai").contains("q=ai"));
    }

    #[test]
    fn cited_report_assembles_and_renders() {
        let learnings = vec![
            ("Rust is safe".to_string(), vec!["https://r1".to_string()]),
            ("Uncited claim".to_string(), vec![]),
        ];
        let report = CitedReport::assemble("Research Report", &learnings);
        assert_eq!(report.claims.len(), 2);
        assert!((report.confidence - 0.5).abs() < 1e-9);
        assert_eq!(report.sources_count, 1);
        let md = report.render_markdown();
        assert!(md.contains("Confidence: 50%"));
        assert!(md.contains("[0.0](https://r1)"));
        assert!(md.contains("(uncited)"));
    }

    #[test]
    fn parallel_fetch_cascade_falls_back_through_tiers() {
        // Firecrawl + crawl4ai fail; raw succeeds.
        let t = ScriptedTransport::new().with_fetch("raw|https://x", "Page text");
        let page = ParallelFetchCascade::fetch_one(&t, "https://x");
        assert_eq!(page.tier, "raw");
        assert_eq!(page.text, "Page text");
    }

    #[test]
    fn parallel_fetch_cascade_fails_when_all_tiers_fail() {
        let t = ScriptedTransport::new();
        let page = ParallelFetchCascade::fetch_one(&t, "https://x");
        assert_eq!(page.tier, "failed");
        assert!(page.text.is_empty());
    }

    #[test]
    fn site_index_searches_by_token_overlap() {
        let mut idx = SiteIndex::new();
        idx.add("https://a", "The Rust language is safe and fast");
        idx.add("https://b", "Baking bread requires flour and water");
        let results = idx.search("rust safe", 2);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "https://a");
    }

    #[test]
    fn repo_wide_blast_radius() {
        let deps = vec![
            DepEdge {
                from: "core".to_string(),
                to: "memory".to_string(),
                kind: "depends_on".to_string(),
            },
            DepEdge {
                from: "core".to_string(),
                to: "guard".to_string(),
                kind: "depends_on".to_string(),
            },
            DepEdge {
                from: "app".to_string(),
                to: "core".to_string(),
                kind: "depends_on".to_string(),
            },
        ];
        let radius = RepoWideScan::blast_radius(&deps, "memory");
        assert!(radius.contains(&"core".to_string()));
        assert!(radius.contains(&"app".to_string()));
        // The changed crate itself is not part of its own blast radius.
        assert!(!radius.contains(&"memory".to_string()));
    }

    #[test]
    fn search_urls_percent_encode_the_query() {
        // Bugfix — a query must be encoded so it can't inject extra params or
        // a second request into the endpoint URL.
        let evil = "alpha & from=admin & x";
        let github = Channel::Github.build_query(evil);
        assert_eq!(
            github,
            "https://api.github.com/search/repositories?q=alpha%20%26%20from%3Dadmin%20%26%20x&per_page=5"
        );
        let arxiv = Channel::Arxiv.build_query(evil);
        assert!(arxiv.contains("search_query=all:alpha%20%26%20from%3Dadmin"));
        let reddit = Channel::Reddit.build_query(evil);
        assert!(reddit.contains("q=alpha%20%26%20from%3Dadmin%20%26%20x"));
    }
}
