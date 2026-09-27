//! P2.4 — Tiered Engine Stack (E10; doc 08 §8.8, doc 55 Obscura/Lightpanda;
//! ARCH/08 §8.8 tier table, ARCH/06 §6.15 containment).
//!
//! Three engine tiers with automatic escalation (E8) — render only as heavy
//! as the task needs:
//!
//! | tier | engine    | cost        | JS  | auth | notes                                  |
//! |------|-----------|-------------|-----|------|----------------------------------------|
//! | 0    | static    | ~0 (no proc)| no  | no   | HTTP + markdown negotiation + llms.txt walk (`read::read_http`), HTML→markdown via `html_to_markdown` (in-tree, licence-clean), SSRF guard at the orchestration layer |
//! | 1    | light     | ~30–60MB RSS| yes | no   | Lightpanda (default) or Obscura — `serve` on loopback, Chrome-compatible CDP, native SSRF/worker containment |
//! | 2    | chrome    | ~300MB+     | yes | yes  | full engine via `agentcowork-cdp::spawn_browser` (login-needed pages, fallback) |
//!
//! Security posture (doc 55 §2 / ARCH/06 §6.15, copied from Obscura):
//! loopback/RFC1918/link-local destinations blocked by default (SSRF),
//! `file://` blocked, bounded CDP connections, workers disabled in light
//! engines (fail-closed), 2MB `--max-output` body cap. Every opt-in is
//! explicit (`EngineConfig::allow_private_network`, `allow_file_access`).

use crate::actions::DOM_WALKER_MARKDOWN;
use crate::capture::CdpSession;
use crate::read::{ReadOptions, ReadSource, looks_like_html, read_http};
use agentcowork_cdp::discovery::connect_to_browser;
use agentcowork_cdp::{
    BrowserEndpoint, CdpClient, CdpError, CdpEvent, LaunchOptions, Session, TargetInfo, TargetType,
    spawn_browser,
};
use agentcowork_guard::netfloor::NetPolicy;
use agentcowork_guard::toctou::{ToctouError, bind_url_with_policy};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashSet, VecDeque};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

/// Which lightweight engine tier 1 prefers (doc 55 — both speak the same
/// Chrome-compatible CDP surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LightEngine {
    #[default]
    Lightpanda,
    Obscura,
}

/// Why the caller is fetching — drives the starting tier (E8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FetchIntent {
    /// Plain content fetch — tier 0 handles most URLs.
    #[default]
    Static,
    /// The page needs JS rendering — start at tier 1.
    NeedsJs,
    /// Authenticated/session page — go straight to tier 2 (Chrome).
    NeedsLogin,
}

/// Which tier produced the result (E8 escalation trace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineTier {
    Static,
    Lightpanda,
    Obscura,
    Chrome,
}

/// Result of a tiered fetch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EngineResult {
    pub tier: EngineTier,
    pub markdown: String,
    /// Provenance: negotiation source (tier 0) or `DomWalked` (tier 1/2).
    pub source: ReadSource,
    pub truncated: bool,
}

/// Configuration for the tiered stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// SSRF opt-in: allow loopback/RFC1918/link-local destinations (doc 55
    /// §2 — blocked by default).
    pub allow_private_network: bool,
    /// `file://` opt-in (doc 55 §2 — blocked by default).
    pub allow_file_access: bool,
    /// Bounded CDP connections for light engines (doc 55:
    /// `DEFAULT_MAX_CONNECTIONS=128`).
    pub max_connections: u32,
    /// Max output bytes (doc 55 `--max-output`; default 2MB read cap).
    pub max_output: usize,
    /// Per-tier timeout.
    pub timeout: Duration,
    /// Preferred light engine.
    pub light_engine: LightEngine,
    /// Explicit light-engine binary paths (defaults to PATH lookup).
    pub lightpanda_bin: Option<PathBuf>,
    pub obscura_bin: Option<PathBuf>,
    /// Browser containment: when non-empty, only these domains may be loaded
    /// (agent-browser `--allowed-domains` semantics, doc 55 §1 / ARCH/06
    /// §6.15) — enforced at the orchestration layer on every tier.
    pub allowed_domains: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            allow_private_network: false,
            allow_file_access: false,
            max_connections: 128,
            max_output: crate::read::READ_BODY_CAP,
            timeout: Duration::from_secs(15),
            light_engine: LightEngine::Lightpanda,
            lightpanda_bin: None,
            obscura_bin: None,
            allowed_domains: Vec::new(),
        }
    }
}

/// Errors from the tiered stack.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no such engine binary: {0} (install it or set the config path)")]
    BinaryNotFound(&'static str),
    #[error("failed to spawn {0}: {1}")]
    Spawn(&'static str, std::io::Error),
    #[error(
        "SSRF guard: private/loopback/link-local destination blocked (set allow_private_network to opt in)"
    )]
    SsrfBlocked,
    #[error("file:// blocked by default (set allow_file_access to opt in)")]
    FileBlocked,
    #[error("DNS resolution failed at the network boundary: {0}")]
    Dns(String),
    #[error("network guard refused a browser request: {0}")]
    NetworkGuard(String),
    #[error("network guard unavailable: {0}")]
    NetworkGuardUnavailable(String),
    #[error("navigation failed: {0}")]
    NavigationFailed(String),
    #[error("unsafe or unavailable HTTP redirect: {0}")]
    UnsafeRedirect(String),
    #[error("bad url: {0}")]
    BadUrl(String),
    #[error("domain not in allowed_domains: {0}")]
    DomainNotAllowed(String),
    #[error("http: {0}")]
    Http(#[from] Box<ureq::Error>),
    #[error("cdp: {0}")]
    Cdp(#[from] CdpError),
    #[error("engine timeout after {0:?}")]
    Timeout(Duration),
    #[error("no readable content")]
    NotFound,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl EngineError {
    /// A heavier tier can plausibly succeed where this tier failed (render
    /// may fix a broken negotiation; a bigger engine may fix a CDP gap).
    fn escalatable(&self) -> bool {
        matches!(
            self,
            EngineError::Http(_)
                | EngineError::Timeout(_)
                | EngineError::NotFound
                | EngineError::Cdp(_)
        )
    }

    /// A capability gap (missing binary / engine failure) rather than a
    /// policy rejection — heavier tiers are worth trying.
    fn is_capability_gap(&self) -> bool {
        matches!(
            self,
            EngineError::BinaryNotFound(_)
                | EngineError::Spawn(_, _)
                | EngineError::Cdp(_)
                | EngineError::NetworkGuardUnavailable(_)
                | EngineError::Io(_)
        )
    }
}

impl From<LightEngine> for EngineTier {
    fn from(e: LightEngine) -> Self {
        match e {
            LightEngine::Lightpanda => EngineTier::Lightpanda,
            LightEngine::Obscura => EngineTier::Obscura,
        }
    }
}

#[derive(Default)]
struct CdpGuardState {
    errors: Vec<String>,
    load_events: u64,
}

/// A CDP session facade that validates every paused request before allowing it
/// to reach the network.
///
/// `Fetch` interception is enabled at the request stage for the whole target,
/// so redirects, subresources, `fetch`, and XSS use the same canonical netfloor
/// decision as top-level navigation. Non-network browser schemes (`about:`,
/// `data:`, and `blob:`) are passed through; every network or unknown scheme
/// is resolved and classified immediately before `Fetch.continueRequest`.
pub struct CdpNetworkGuard {
    client: Arc<CdpClient>,
    session_id: String,
    policy: NetPolicy,
    allowed_domains: Vec<String>,
    state: Mutex<CdpGuardState>,
    retained_events: Mutex<VecDeque<CdpEvent>>,
    guarded_sessions: Mutex<HashSet<String>>,
}

impl CdpNetworkGuard {
    /// Enable request interception and return a guarded CDP facade.
    pub fn enable(
        client: Arc<CdpClient>,
        session_id: &str,
        policy: NetPolicy,
        allowed_domains: Vec<String>,
    ) -> Result<Self, EngineError> {
        client
            .call_session(
                session_id,
                "Fetch.enable",
                json!({
                    "patterns": [{ "urlPattern": "*", "requestStage": "Request" }]
                }),
            )
            .map_err(|e| {
                EngineError::NetworkGuardUnavailable(format!(
                    "Fetch.enable on the browser target failed: {e}"
                ))
            })?;
        Ok(Self {
            client,
            session_id: session_id.to_string(),
            policy,
            allowed_domains: allowed_domains
                .into_iter()
                .map(|d| d.trim_end_matches('.').to_ascii_lowercase())
                .collect(),
            state: Mutex::new(CdpGuardState::default()),
            retained_events: Mutex::new(VecDeque::new()),
            guarded_sessions: Mutex::new(HashSet::from([session_id.to_string()])),
        })
    }

    /// Send a browser-level CDP command.
    pub fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, CdpError> {
        self.client.call(method, params)
    }

    /// Send a target-scoped CDP command.
    pub fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, CdpError> {
        self.ensure_session_guarded(session_id)?;
        self.client.call_session(session_id, method, params)
    }

    /// Attach a child target and install request interception on its session
    /// before the caller can issue any target command.
    pub fn attach(&self, target_id: &str) -> Result<Session, CdpError> {
        let session = self.client.attach(target_id)?;
        self.ensure_session_guarded(&session.session_id)?;
        Ok(session)
    }

    /// List page targets through the underlying client.
    pub fn list_targets(&self) -> Result<Vec<TargetInfo>, CdpError> {
        self.client.list_targets()
    }

    /// Validate and navigate a top-level URL, returning the normalized final
    /// URL after load. CDP's `errorText`, load failure, DNS failure, and a
    /// blocked redirect/subresource are all returned as errors.
    pub fn navigate(
        &self,
        session_id: &str,
        url: &str,
        timeout: Duration,
        require_load_event: bool,
    ) -> Result<String, EngineError> {
        let target = self.validate_top_level_url(url)?;
        if require_load_event {
            self.call_session(session_id, "Page.enable", serde_json::Value::Null)?;
        } else {
            // Light engines have historically differed on Page-domain support;
            // their ready-state fallback below remains available, but Fetch
            // interception is mandatory and was already enabled above.
            let _ = self.call_session(session_id, "Page.enable", serde_json::Value::Null);
        }
        let old_url = self.current_url(session_id).unwrap_or_default();
        let load_baseline = self.load_event_count();
        let response = self.call_session(session_id, "Page.navigate", json!({ "url": target }))?;
        if let Some(error) = response
            .get("errorText")
            .and_then(serde_json::Value::as_str)
            .filter(|error| !error.trim().is_empty())
        {
            return Err(EngineError::NavigationFailed(error.to_string()));
        }

        let started = Instant::now();
        loop {
            self.pump_network_guard();
            if let Some(error) = self.take_network_error() {
                return Err(error);
            }
            if self.load_event_count() > load_baseline {
                break;
            }
            if !require_load_event && started.elapsed() >= Duration::from_millis(250) {
                let current = self.current_url(session_id).unwrap_or_default();
                let ready = self.document_ready(session_id).unwrap_or_default();
                if ready == "complete"
                    && current != "about:blank"
                    && (current != old_url || started.elapsed() >= Duration::from_millis(500))
                {
                    break;
                }
            }
            if started.elapsed() >= timeout {
                return Err(EngineError::Timeout(timeout));
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        self.pump_network_guard();
        if let Some(error) = self.take_network_error() {
            return Err(error);
        }
        let final_url = self.current_url(session_id).unwrap_or(target);
        if final_url == "about:blank" {
            return Err(EngineError::NavigationFailed(
                "page did not leave about:blank".to_string(),
            ));
        }
        self.validate_top_level_url(&final_url)
    }

    /// Process currently queued request pauses while retaining unrelated CDP
    /// events for the normal browser/snapshot consumers.
    pub fn pump_network_guard(&self) {
        self.process_events();
    }

    fn validate_top_level_url(&self, url: &str) -> Result<String, EngineError> {
        self.validate_network_url(url)
    }

    fn validate_request_url(&self, url: &str) -> Result<String, EngineError> {
        let parsed = Url::parse(url).map_err(|e| EngineError::BadUrl(e.to_string()))?;
        if matches!(parsed.scheme(), "about" | "data" | "blob") {
            return Ok(url.to_string());
        }
        self.validate_network_url(url)
    }

    fn validate_network_url(&self, url: &str) -> Result<String, EngineError> {
        let parsed = Url::parse(url).map_err(|e| EngineError::BadUrl(e.to_string()))?;
        if parsed.scheme() == "file" {
            return Err(EngineError::FileBlocked);
        }
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(EngineError::BadUrl(format!(
                "scheme {} not allowed",
                parsed.scheme()
            )));
        }
        let binding = bind_url_with_policy(&parsed.to_string(), &[], self.policy)
            .map_err(map_toctou_error)?;
        self.guard_domain(&binding.url)?;
        Ok(binding.url)
    }

    fn guard_domain(&self, url: &str) -> Result<(), EngineError> {
        guard_domain(url, &self.allowed_domains)
    }

    fn ensure_session_guarded(&self, session_id: &str) -> Result<(), CdpError> {
        if self
            .guarded_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(session_id)
        {
            return Ok(());
        }
        self.client.call_session(
            session_id,
            "Fetch.enable",
            json!({
                "patterns": [{ "urlPattern": "*", "requestStage": "Request" }]
            }),
        )?;
        self.guarded_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.to_string());
        Ok(())
    }

    fn process_events(&self) {
        for event in self.client.drain_events() {
            if event.method == "Fetch.requestPaused" {
                self.process_paused_request(event);
                continue;
            }
            if event.method == "Page.loadEventFired"
                && event.session_id.as_deref() == Some(self.session_id.as_str())
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.load_events = state.load_events.saturating_add(1);
                continue;
            }
            let mut retained = self
                .retained_events
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if retained.len() >= 256 {
                retained.pop_front();
            }
            retained.push_back(event);
        }
    }

    fn process_paused_request(&self, event: CdpEvent) {
        let request_id = event
            .params
            .get("requestId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if request_id.is_empty() {
            self.record_error("Fetch.requestPaused had no requestId".to_string());
            return;
        }
        let request_url = event
            .params
            .pointer("/request/url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let session_id = event
            .session_id
            .as_deref()
            .unwrap_or(self.session_id.as_str());
        match self.validate_request_url(request_url) {
            Ok(_) => {
                if let Err(error) = self.call_session(
                    session_id,
                    "Fetch.continueRequest",
                    json!({ "requestId": request_id }),
                ) {
                    self.record_error(format!(
                        "allowed request could not continue ({request_id}): {error}"
                    ));
                }
            }
            Err(error) => {
                if let Err(fail_error) = self.call_session(
                    session_id,
                    "Fetch.failRequest",
                    json!({
                        "requestId": request_id,
                        "errorReason": "BlockedByClient"
                    }),
                ) {
                    self.record_error(format!(
                        "blocked request could not be failed ({request_id}): {fail_error}"
                    ));
                }
                self.record_error(format!("request paused by network floor: {error}"));
            }
        }
    }

    fn record_error(&self, error: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.errors.len() < 32 {
            state.errors.push(error);
        }
    }

    /// Return and clear the first request blocked since the last check.
    pub fn take_network_error(&self) -> Option<EngineError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.errors.is_empty() {
            return None;
        }
        let errors = std::mem::take(&mut state.errors);
        Some(EngineError::NetworkGuard(format!(
            "{} request(s) refused: {}",
            errors.len(),
            errors.join("; ")
        )))
    }

    fn load_event_count(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .load_events
    }

    fn current_url(&self, session_id: &str) -> Result<String, CdpError> {
        let history = self.call_session(
            session_id,
            "Page.getNavigationHistory",
            serde_json::Value::Null,
        )?;
        let index = history
            .get("currentIndex")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "Page.getNavigationHistory missing currentIndex".to_string(),
            })? as usize;
        history
            .pointer(&format!("/entries/{index}/url"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "Page.getNavigationHistory missing current URL".to_string(),
            })
    }

    fn document_ready(&self, session_id: &str) -> Result<String, CdpError> {
        let response = self.call_session(
            session_id,
            "Runtime.evaluate",
            json!({
                "expression": "document.readyState",
                "returnByValue": true
            }),
        )?;
        Ok(response
            .pointer("/result/value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string())
    }
}

impl CdpSession for CdpNetworkGuard {
    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, CdpError> {
        self.client.call(method, params)
    }

    fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, CdpError> {
        CdpNetworkGuard::call_session(self, session_id, method, params)
    }

    fn attach(&self, target_id: &str) -> Result<Session, CdpError> {
        CdpNetworkGuard::attach(self, target_id)
    }

    fn drain_events(&self) -> Vec<CdpEvent> {
        self.process_events();
        self.retained_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

fn map_toctou_error(error: ToctouError) -> EngineError {
    match error {
        ToctouError::Dns { message, .. } => EngineError::Dns(message),
        ToctouError::Url(message) if message.contains("PrivateDestination") => {
            EngineError::SsrfBlocked
        }
        ToctouError::Url(message) => EngineError::BadUrl(message),
        ToctouError::BlockedIp(_) | ToctouError::Rebind { .. } => EngineError::SsrfBlocked,
        other => EngineError::NetworkGuard(other.to_string()),
    }
}

fn guard_domain(url: &str, allowed_domains: &[String]) -> Result<(), EngineError> {
    if allowed_domains.is_empty() {
        return Ok(());
    }
    let host = Url::parse(url)
        .map_err(|e| EngineError::BadUrl(e.to_string()))?
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let allowed = allowed_domains
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if allowed {
        Ok(())
    } else {
        Err(EngineError::DomainNotAllowed(host))
    }
}

/// The tiered engine stack. Owns no processes; spawns engines per fetch and
/// tears them down on exit.
#[derive(Debug, Clone)]
pub struct TieredEngine {
    pub config: EngineConfig,
    /// Explicit Chrome binary override (falls back to system Chrome).
    pub chrome_binary: Option<PathBuf>,
}

impl TieredEngine {
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            chrome_binary: None,
        }
    }

    pub fn with_chrome_binary(mut self, bin: PathBuf) -> Self {
        self.chrome_binary = Some(bin);
        self
    }

    /// Fetch `url` with the cheapest tier that can satisfy `intent`,
    /// escalating 0→1→2 on failure (E8).
    pub fn fetch(&self, url: &str, intent: FetchIntent) -> Result<EngineResult, EngineError> {
        let start = match intent {
            FetchIntent::NeedsLogin => EngineTier::Chrome,
            FetchIntent::NeedsJs => self.config.light_engine.into(),
            FetchIntent::Static => EngineTier::Static,
        };
        let mut tier = start;
        loop {
            let result = match tier {
                EngineTier::Static => self.static_fetch(url),
                EngineTier::Lightpanda | EngineTier::Obscura => self.light_fetch(url),
                EngineTier::Chrome => self.chrome_fetch(url),
            };
            match result {
                Ok(r) => return Ok(r),
                Err(err) => match self.escalate_from(tier, &err) {
                    Some(next) => tier = next,
                    None => return Err(err),
                },
            }
        }
    }

    /// After `tier` failed with `err`, which tier to try next (None = stop).
    pub fn escalate_from(&self, tier: EngineTier, err: &EngineError) -> Option<EngineTier> {
        match tier {
            EngineTier::Static => {
                // Policy rejections (SSRF/file://domains) are not capability
                // problems — a heavier engine would hit the same wall.
                if err.escalatable() || err.is_capability_gap() {
                    Some(self.config.light_engine.into())
                } else {
                    None
                }
            }
            EngineTier::Lightpanda | EngineTier::Obscura => {
                if err.is_capability_gap() {
                    Some(EngineTier::Chrome)
                } else {
                    None
                }
            }
            EngineTier::Chrome => None,
        }
    }

    // ------------------------------------------------------------------
    // tier 0 — static extraction (no browser)
    // ------------------------------------------------------------------

    fn static_fetch(&self, url: &str) -> Result<EngineResult, EngineError> {
        let mut target = self.validate_url(url)?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(self.config.timeout)
            .timeout_read(self.config.timeout)
            // Redirects are followed below with a netfloor decision before
            // every hop. Never let the HTTP client follow one implicitly.
            .redirects(0)
            .build();
        target = self.follow_safe_redirects(&agent, target)?;
        // Revalidate immediately before the real content read. The resolver
        // snapshot is not a pin, so a DNS answer that changed after redirect
        // discovery is refused here rather than fetched blindly.
        target = self.validate_url(&target)?;
        // read_http does Accept: text/markdown → .md retry → llms.txt walk
        // (doc 55 read.rs). Its generated candidates stay on the validated
        // origin, and the agent still has automatic redirects disabled.
        let mut res = read_http(&agent, &target, &ReadOptions::default())?;
        if res.source == ReadSource::PlainHtml && looks_like_html(&res.markdown) {
            res.markdown = html_to_markdown(&res.markdown);
        }
        let truncated = cap_output(&mut res.markdown, self.config.max_output);
        Ok(EngineResult {
            tier: EngineTier::Static,
            markdown: res.markdown,
            source: res.source,
            truncated,
        })
    }

    /// Canonicalize, contain, resolve, and classify a destination under the
    /// configured policy. DNS errors are failures, never an empty-address pass.
    fn validate_url(&self, url: &str) -> Result<String, EngineError> {
        let parsed = Url::parse(url).map_err(|e| EngineError::BadUrl(e.to_string()))?;
        if parsed.scheme() == "file" {
            return Err(EngineError::FileBlocked);
        }
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(EngineError::BadUrl(format!(
                "scheme {} not allowed",
                parsed.scheme()
            )));
        }
        let normalized = parsed.to_string();
        guard_domain(&normalized, &self.config.allowed_domains)?;
        let policy = if self.config.allow_private_network {
            NetPolicy::local()
        } else {
            NetPolicy::strict()
        };
        Ok(bind_url_with_policy(&normalized, &[], policy)
            .map_err(map_toctou_error)?
            .url)
    }

    fn follow_safe_redirects(
        &self,
        agent: &ureq::Agent,
        mut current: String,
    ) -> Result<String, EngineError> {
        const MAX_REDIRECTS: usize = 10;
        for _ in 0..=MAX_REDIRECTS {
            let response = agent
                .get(&current)
                .call()
                .map_err(|error| EngineError::Http(Box::new(error)))?;
            let status = response.status();
            if !(300..399).contains(&status) {
                return Ok(current);
            }
            let location = response.header("location").ok_or_else(|| {
                EngineError::UnsafeRedirect(format!("HTTP {status} had no Location header"))
            })?;
            let next = Url::parse(&current)
                .map_err(|e| EngineError::BadUrl(e.to_string()))?
                .join(location)
                .map_err(|e| EngineError::BadUrl(e.to_string()))?;
            current = self.validate_url(next.as_str())?;
        }
        Err(EngineError::UnsafeRedirect(format!(
            "more than {MAX_REDIRECTS} redirects"
        )))
    }

    // ------------------------------------------------------------------
    // tier 1 — light engine (Lightpanda / Obscura)
    // ------------------------------------------------------------------

    fn light_fetch(&self, url: &str) -> Result<EngineResult, EngineError> {
        let target = self.validate_url(url)?;
        let spawned = self.spawn_light()?;
        let client = Arc::new(connect_to_browser(&spawned.endpoint)?);
        let tier = self.config.light_engine.into();
        // SpawnedLight's Drop kills + reaps the engine process.
        let res = self.fetch_via_cdp(&client, &target, tier);
        drop(spawned);
        res
    }

    /// Spawn the configured light engine on loopback with its security
    /// defaults, and wait for its Chrome-compatible CDP endpoint.
    fn spawn_light(&self) -> Result<SpawnedLight, EngineError> {
        let port = free_port()?;
        match self.config.light_engine {
            LightEngine::Lightpanda => {
                let bin = self
                    .lightpanda_bin()
                    .filter(|p| p.is_file())
                    .ok_or(EngineError::BinaryNotFound("lightpanda"))?;
                let mut cmd = Command::new(bin);
                cmd.arg("serve")
                    .arg("--host")
                    .arg("127.0.0.1")
                    .arg("--port")
                    .arg(port.to_string());
                // SSRF default: block private networks unless opted in.
                if !self.config.allow_private_network {
                    cmd.arg("--block-private-networks");
                }
                // Worker fail-closed (doc 55 §1) + bounded connections.
                cmd.arg("--disable-workers")
                    .arg("--cdp-max-connections")
                    .arg(self.config.max_connections.to_string());
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
                let child = cmd
                    .spawn()
                    .map_err(|e| EngineError::Spawn("lightpanda", e))?;
                let ws = wait_for_cdp_endpoint(port, self.config.timeout)?;
                Ok(SpawnedLight {
                    child,
                    endpoint: BrowserEndpoint {
                        browser_ws_url: ws,
                        version: "lightpanda".into(),
                    },
                })
            }
            LightEngine::Obscura => {
                let bin = self
                    .obscura_bin()
                    .filter(|p| p.is_file())
                    .ok_or(EngineError::BinaryNotFound("obscura"))?;
                let mut cmd = Command::new(bin);
                cmd.arg("serve")
                    .arg("--host")
                    .arg("127.0.0.1")
                    .arg("--port")
                    .arg(port.to_string());
                // Obscura blocks private networks + file:// by default;
                // both are explicit opt-ins here (doc 55 §2).
                if self.config.allow_private_network {
                    cmd.arg("--allow-private-network");
                }
                if self.config.allow_file_access {
                    cmd.arg("--allow-file-access");
                }
                cmd.arg("--max-connections")
                    .arg(self.config.max_connections.to_string());
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
                let child = cmd.spawn().map_err(|e| EngineError::Spawn("obscura", e))?;
                let ws = wait_for_cdp_endpoint(port, self.config.timeout)?;
                Ok(SpawnedLight {
                    child,
                    endpoint: BrowserEndpoint {
                        browser_ws_url: ws,
                        version: "obscura".into(),
                    },
                })
            }
        }
    }

    fn lightpanda_bin(&self) -> Option<PathBuf> {
        self.config
            .lightpanda_bin
            .clone()
            .or_else(|| find_in_path("lightpanda"))
    }

    fn obscura_bin(&self) -> Option<PathBuf> {
        self.config
            .obscura_bin
            .clone()
            .or_else(|| find_in_path("obscura"))
    }

    // ------------------------------------------------------------------
    // tier 2 — Chrome (full engine)
    // ------------------------------------------------------------------

    fn chrome_fetch(&self, url: &str) -> Result<EngineResult, EngineError> {
        let target = self.validate_url(url)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let profile = TempProfile(std::env::temp_dir().join(format!(
            "agentcowork-chrome-tier-{}-{nanos}",
            std::process::id()
        )));
        let opts = LaunchOptions {
            user_data_dir: profile.0.clone(),
            headless: true,
            browser_binary: self.chrome_binary.clone(),
            // WebRTC containment (ARCH/06 §6.15) — the light engines carry
            // the full fail-closed worker guards natively.
            extra_args: vec![
                "--disable-features=WebRTC".into(),
                "--disable-background-networking".into(),
            ],
            wait_timeout: self.config.timeout,
        };
        let spawned = spawn_browser(&opts)?;
        let client = Arc::new(connect_to_browser(spawned.endpoint())?);
        // BrowserChild's Drop kills + reaps Chrome; TempProfile's Drop
        // removes the profile dir on every exit path.
        let res = self.fetch_via_cdp(&client, &target, EngineTier::Chrome);
        drop(spawned);
        res
    }

    // ------------------------------------------------------------------
    // shared CDP driver (tier 1 and tier 2)
    // ------------------------------------------------------------------

    /// Drive any Chrome-compatible engine: create a page, attach, install the
    /// request-stage network guard, navigate, wait for load, and extract via
    /// the DOM walker.
    fn fetch_via_cdp(
        &self,
        client: &Arc<CdpClient>,
        url: &str,
        tier: EngineTier,
    ) -> Result<EngineResult, EngineError> {
        // Reuse the engine's existing page target when present (both fresh
        // Chrome and Lightpanda's `serve` expose one); create one only as a
        // fallback — Lightpanda doesn't implement Target.createTarget.
        // Note: `call` returns the *unwrapped* CDP result object, so
        // `targetId`/`sessionId` sit at the top level, not under `/result`.
        let targets = client.list_targets()?;
        let target_id = if let Some(target) = targets
            .iter()
            .find(|target| target.target_type == TargetType::Page)
        {
            target.target_id.clone()
        } else {
            let created = client.call("Target.createTarget", json!({ "url": "about:blank" }))?;
            created
                .get("targetId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| CdpError::Protocol {
                    code: -1,
                    message: "Target.createTarget: missing targetId".to_string(),
                })?
        };
        let attached = client.call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )?;
        let session_id = attached
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: format!("Target.attachToTarget: missing sessionId (got {attached})"),
            })?
            .to_string();
        let guarded = CdpNetworkGuard::enable(
            Arc::clone(client),
            &session_id,
            self.net_policy(),
            self.config.allowed_domains.clone(),
        )?;
        let require_load_event = !matches!(tier, EngineTier::Lightpanda | EngineTier::Obscura);
        let final_url =
            guarded.navigate(&session_id, url, self.config.timeout, require_load_event)?;
        guarded.pump_network_guard();
        if let Some(error) = guarded.take_network_error() {
            return Err(error);
        }
        let out = guarded.call_session(
            &session_id,
            "Runtime.evaluate",
            json!({
                "expression": DOM_WALKER_MARKDOWN,
                "returnByValue": true,
            }),
        )?;
        let markdown = out
            .pointer("/result/value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if markdown.trim().is_empty() {
            return Err(EngineError::NotFound);
        }
        // The one-shot engine is about to be torn down. Disabling interception
        // is cleanup only; any error before this point kept requests paused or
        // failed them closed.
        guarded.call_session(&session_id, "Fetch.disable", serde_json::Value::Null)?;
        // G9 read-cleaner (P2.11): strip ad/tracker links + consent walls.
        let cleaned = crate::content::clean_markdown(
            &crate::content::default_filter_set(),
            &final_url,
            &markdown,
        );
        let mut markdown = cleaned.text;
        let truncated = cap_output(&mut markdown, self.config.max_output);
        Ok(EngineResult {
            tier,
            markdown,
            source: ReadSource::DomWalked,
            truncated,
        })
    }

    fn net_policy(&self) -> NetPolicy {
        if self.config.allow_private_network {
            NetPolicy::local()
        } else {
            NetPolicy::strict()
        }
    }
}

/// A spawned light engine plus its discovered CDP endpoint. Killing happens
/// on drop (mirrors `agentcowork_cdp::BrowserChild`).
struct SpawnedLight {
    child: Child,
    endpoint: BrowserEndpoint,
}

impl Drop for SpawnedLight {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Grab a free loopback port (small TOCTOU race — acceptable for a local
/// engine; if the port is taken the spawn fails fast and the caller escalates).
fn free_port() -> Result<u16, EngineError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Poll `/json/version` until the engine serves its `webSocketDebuggerUrl`.
fn wait_for_cdp_endpoint(port: u16, timeout: Duration) -> Result<String, EngineError> {
    let deadline = Instant::now() + timeout;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(2))
        .build();
    let url = format!("http://127.0.0.1:{port}/json/version");
    loop {
        if let Ok(resp) = agent.get(&url).call() {
            if let Ok(body) = resp.into_string() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(ws) = v.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) {
                        return Ok(ws.to_string());
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(EngineError::Timeout(timeout));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// HTML → markdown for tier-0 static reads.
///
/// P70.B6 licence decision: the previous converter (`html2md`) is GPL-3.0+, and
/// a copyleft crate cannot be statically linked into an MIT OR Apache-2.0
/// product without licensing the product differently. The tier-0 need is small
/// — negotiate markdown first (`read_http`), and when only HTML came back,
/// convert the narrow set of elements real pages actually serve — so the
/// conversion lives here (~60 lines, permissively licensed) rather than taking
/// on GPL obligations. Block-level elements become blank-line-separated
/// markdown; headings, emphasis/code, lists, blockquotes and links are
/// handled; scripts/styles and every tag are stripped; HTML entities are
/// decoded. Well-formed single-page markdown is the goal, not full spec
/// coverage — a page that converts badly still yields readable text, and tier
/// 1/2 (real engines) remain the path for fidelity.
fn html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2 + 16);
    let bytes = html.as_bytes();
    let mut i = 0;
    let mut tag = String::new();
    let mut in_tag = false;
    let mut dropping = 0usize; // depth inside <script>/<style>
    // (heading level, active) + emphasis markers emitted at open time.
    let mut link_open = false;
    let mut list_item_open = false;
    let mut blockquote_open = false;
    while i < bytes.len() {
        let c = html[i..].chars().next().unwrap_or('\0');
        let clen = c.len_utf8();
        if c == '<' && !in_tag {
            in_tag = true;
            tag.clear();
            i += 1;
            continue;
        }
        if in_tag {
            if c == '>' {
                in_tag = false;
                let open = !tag.starts_with('/');
                let name = tag
                    .trim_start_matches('/')
                    .split(|ch: char| ch.is_whitespace() || ch == '>')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let void = matches!(
                    name.as_str(),
                    "br" | "hr" | "img" | "input" | "meta" | "link"
                );
                match name.as_str() {
                    "script" | "style" => {
                        if open {
                            dropping += 1;
                        } else {
                            dropping = dropping.saturating_sub(1);
                        }
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = name.as_bytes()[1] - b'0';
                        if open {
                            ensure_blank_line(&mut out);
                            for _ in 0..level {
                                out.push('#');
                            }
                            out.push(' ');
                        } else {
                            ensure_blank_line(&mut out);
                        }
                    }
                    "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav"
                    | "aside" | "figure" | "figcaption" | "table" | "tr" | "br" | "hr" => {
                        if name == "br" {
                            out.push_str("  \n");
                        } else if name == "hr" {
                            ensure_blank_line(&mut out);
                            out.push_str("---");
                            ensure_blank_line(&mut out);
                        } else {
                            ensure_blank_line(&mut out);
                        }
                    }
                    "li" => {
                        if open {
                            ensure_blank_line(&mut out);
                            out.push_str("- ");
                            list_item_open = true;
                        } else {
                            list_item_open = false;
                            out.push('\n');
                        }
                    }
                    "blockquote" => {
                        if open {
                            ensure_blank_line(&mut out);
                            out.push_str("> ");
                            blockquote_open = true;
                        } else {
                            blockquote_open = false;
                            ensure_blank_line(&mut out);
                        }
                    }
                    "strong" | "b" => out.push_str("**"),
                    "em" | "i" => out.push('*'),
                    "code" | "pre" => out.push('`'),
                    "a" => {
                        if open {
                            link_open = true;
                            out.push('[');
                        } else if link_open {
                            // The href is dropped: the readable markdown of a
                            // static page keeps the link text. (Full href
                            // extraction needs attribute parsing; tier 0 is
                            // a text-extraction tier.)
                            out.push(']');
                            link_open = false;
                        }
                    }
                    _ => {}
                }
                void_tag_marker(void);
                i += 1;
                continue;
            }
            tag.push(c);
            i += clen;
            continue;
        }
        if dropping > 0 {
            i += clen;
            continue;
        }
        if c == '&' {
            if let Some(semi) = html[i..].find(';').filter(|&s| s <= 10) {
                let entity = &html[i + 1..i + semi];
                let decoded = decode_entity(entity);
                if let Some(d) = decoded {
                    out.push_str(&d);
                    i += semi + 1;
                    continue;
                }
            }
        }
        if c == '\n' {
            // Collapse runs of newlines outside tags; blank lines are added
            // by ensure_blank_line at block boundaries.
            if !out.ends_with("\n\n") && !out.is_empty() {
                while out.ends_with('\n') {
                    out.pop();
                }
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if c.is_whitespace() && (out.ends_with(' ') || out.ends_with('\n') || out.is_empty()) {
            i += clen;
            continue;
        }
        out.push(c);
        i += clen;
    }
    // If the tail dropped a closing emphasis marker, keep the text readable.
    if list_item_open || blockquote_open {
        out.push('\n');
    }
    out.trim().to_string()
}

fn void_tag_marker(_: bool) {}

fn ensure_blank_line(out: &mut String) {
    if !out.is_empty() && !out.ends_with("\n\n") {
        while out.ends_with('\n') {
            out.pop();
        }
        out.push_str("\n\n");
    }
}

/// The entities real pages use. Numeric forms decode directly; a named entity
/// this converter does not know keeps its literal text (readable, honest).
fn decode_entity(entity: &str) -> Option<String> {
    if let Some(num) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(num, 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    if let Some(num) = entity.strip_prefix('#') {
        return num
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    let named = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        _ => return None,
    };
    Some(named.to_string())
}

/// Enforce the `--max-output` byte cap (doc 55): truncate at a char
/// boundary and append a marker. Returns true when truncated.
fn cap_output(markdown: &mut String, max_output: usize) -> bool {
    if markdown.len() <= max_output {
        return false;
    }
    let mut idx = max_output;
    while idx > 0 && !markdown.is_char_boundary(idx) {
        idx -= 1;
    }
    markdown.truncate(idx);
    markdown.push_str("\n… [truncated]");
    true
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// RAII cleanup for the Chrome tier's temp profile dir — removed on every
/// exit path (including early `?` returns and panics).
struct TempProfile(PathBuf);

impl Drop for TempProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrf_blocks_private_destinations_by_default() {
        let engine = TieredEngine::new(EngineConfig::default());
        for bad in [
            "http://127.0.0.1/x",
            "http://localhost/x",
            "http://10.0.0.1/x",
            "http://192.168.1.1/x",
            "http://172.16.5.5/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/x",
            "http://[::]/x",
            // IPv4-mapped IPv6 bypasses plain is_loopback() — must be blocked.
            "http://[::ffff:127.0.0.1]/x",
            "http://[::ffff:7f00:1]/x",
            "http://[::ffff:10.0.0.1]/x",
            "http://[::ffff:169.254.169.254]/x",
            // Alternate IPv4 encodings normalized before classification.
            "http://2130706433/x",
            "http://0x7f000001/x",
            "http://0177.0.0.1/x",
            // IPv6 unique-local (private) range.
            "http://[fc00::1]/x",
            "http://[fd12:3456::1]/x",
        ] {
            assert!(
                matches!(engine.static_fetch(bad), Err(EngineError::SsrfBlocked)),
                "{bad} should be SSRF-blocked"
            );
        }
        // Opt-in flips the guard: now a private destination fails with a
        // connection error (nothing listening on :1), not SSRF.
        let engine = TieredEngine::new(EngineConfig {
            allow_private_network: true,
            ..Default::default()
        });
        assert!(!matches!(
            engine.static_fetch("http://127.0.0.1:1/x"),
            Err(EngineError::SsrfBlocked)
        ));
    }

    #[test]
    fn file_urls_blocked_by_default() {
        let engine = TieredEngine::new(EngineConfig::default());
        assert!(matches!(
            engine.static_fetch("file:///etc/passwd"),
            Err(EngineError::FileBlocked)
        ));
    }

    #[test]
    fn allowed_domains_containment() {
        let engine = TieredEngine::new(EngineConfig {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        });
        assert!(matches!(
            engine.static_fetch("https://evil.example.net/"),
            Err(EngineError::DomainNotAllowed(_))
        ));
        // Exact host and subdomains pass.
        let r = guard_domain("https://sub.example.com/x", &engine.config.allowed_domains);
        assert!(r.is_ok(), "subdomain should pass: {r:?}");
    }

    #[test]
    fn missing_light_binary_reports_binary_not_found() {
        let engine = TieredEngine::new(EngineConfig {
            lightpanda_bin: Some("/nonexistent/lightpanda".into()),
            obscura_bin: Some("/nonexistent/obscura".into()),
            ..Default::default()
        });
        assert!(matches!(
            engine.light_fetch("https://8.8.8.8/"),
            Err(EngineError::BinaryNotFound(_))
        ));
    }

    #[test]
    fn static_html_converts_to_markdown() {
        let body = "<html><body><h1>Hello</h1><p>World <b>bold</b></p></body></html>";
        let (addr, server) = spawn_http_server(body);
        let engine = TieredEngine::new(EngineConfig {
            allow_private_network: true,
            ..Default::default()
        });
        let res = engine.static_fetch(&format!("http://{addr}/page")).unwrap();
        assert_eq!(res.tier, EngineTier::Static);
        assert!(res.markdown.contains("Hello"), "md: {}", res.markdown);
        assert!(
            res.markdown.contains("bold") || res.markdown.contains("**bold**"),
            "md: {}",
            res.markdown
        );
        drop(server);
    }

    #[test]
    fn redirect_to_metadata_is_blocked_before_the_next_request() {
        let (addr, server) = spawn_redirect_server("http://169.254.169.254/latest/meta-data/");
        let engine = TieredEngine::new(EngineConfig {
            // Loopback is needed for the hermetic source server. The canonical
            // local policy still hard-blocks link-local metadata on redirect.
            allow_private_network: true,
            ..Default::default()
        });
        let err = engine
            .static_fetch(&format!("http://{addr}/redirect"))
            .expect_err("metadata redirect must be refused");
        assert!(matches!(err, EngineError::SsrfBlocked), "{err:?}");
        drop(server);
    }

    #[test]
    fn escalation_rules() {
        let engine = TieredEngine::new(EngineConfig::default());
        // Static failures escalate to the light tier.
        assert_eq!(
            engine.escalate_from(EngineTier::Static, &EngineError::NotFound),
            Some(EngineTier::Lightpanda)
        );
        assert_eq!(
            engine.escalate_from(
                EngineTier::Static,
                &EngineError::Timeout(Duration::from_secs(1))
            ),
            Some(EngineTier::Lightpanda)
        );
        assert_eq!(
            engine.escalate_from(EngineTier::Static, &EngineError::BinaryNotFound("x")),
            Some(EngineTier::Lightpanda)
        );
        // Policy rejections never escalate — a heavier engine hits the same wall.
        assert_eq!(
            engine.escalate_from(EngineTier::Static, &EngineError::SsrfBlocked),
            None
        );
        assert_eq!(
            engine.escalate_from(EngineTier::Static, &EngineError::FileBlocked),
            None
        );
        assert_eq!(
            engine.escalate_from(
                EngineTier::Static,
                &EngineError::DomainNotAllowed("x".into())
            ),
            None
        );
        // DNS failure is a closed decision, not a reason to retry the same
        // destination in a heavier engine.
        assert_eq!(
            engine.escalate_from(EngineTier::Static, &EngineError::Dns("nxdomain".into())),
            None
        );
        // Light capability gaps escalate to Chrome; policy rejections don't.
        assert_eq!(
            engine.escalate_from(
                EngineTier::Lightpanda,
                &EngineError::BinaryNotFound("lightpanda")
            ),
            Some(EngineTier::Chrome)
        );
        assert_eq!(
            engine.escalate_from(
                EngineTier::Lightpanda,
                &EngineError::NetworkGuardUnavailable("no Fetch domain".into())
            ),
            Some(EngineTier::Chrome)
        );
        assert_eq!(
            engine.escalate_from(EngineTier::Obscura, &EngineError::SsrfBlocked),
            None
        );
        // Chrome is terminal.
        assert_eq!(
            engine.escalate_from(EngineTier::Chrome, &EngineError::NotFound),
            None
        );
    }

    #[test]
    fn max_output_caps_and_truncates_at_char_boundary() {
        let mut md = "hello ".repeat(100);
        let truncated = cap_output(&mut md, 32);
        assert!(truncated);
        assert!(md.ends_with("… [truncated]"));
        assert!(md.is_char_boundary(md.len()));
        // Multibyte char right at the cap: byte 3 of "héllo" splits the é.
        let mut s = "héllo wörld".to_string();
        let t = cap_output(&mut s, 3);
        assert!(t);
        assert!(s.is_char_boundary(s.len()));
        assert_eq!(s, "hé\n… [truncated]");
        // Under the cap: untouched.
        let mut s2 = "short".to_string();
        assert!(!cap_output(&mut s2, 100));
        assert_eq!(s2, "short");
    }

    #[test]
    fn tier_from_light_engine_maps() {
        assert_eq!(
            EngineTier::from(LightEngine::Lightpanda),
            EngineTier::Lightpanda
        );
        assert_eq!(EngineTier::from(LightEngine::Obscura), EngineTier::Obscura);
    }

    /// Minimal HTTP server serving `body` for any path (enough for the
    /// negotiation probes read_http makes).
    fn spawn_http_server(
        body: &'static str,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for mut s in listener.incoming().take(20).flatten() {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        (addr, handle)
    }

    fn spawn_redirect_server(
        location: &'static str,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for mut stream in listener.incoming().take(4).flatten() {
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (addr, handle)
    }
}
