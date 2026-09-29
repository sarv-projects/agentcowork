//! Credential broker (P1.2, doc 53 §2) — the ONLY place keys leave the vault.
//!
//! The TS sidecar never holds credentials. It sends `{provider, model, body}`
//! to the broker; the broker resolves a key through the [`KeyRing`], injects
//! the auth header, executes the HTTP call, and **zeroizes** every temporary
//! secret buffer. Budget/rate checks all happen here (single choke point).
//!
//! Failure handling (P1.1):
//! - HTTP 429 → [`KeyRing::report_failure`] puts the key into exponential
//!   cooldown; the broker immediately retries with the next key, up to
//!   [`MAX_429_SWITCHES`] switches.
//! - All keys exhausted → aggregated [`BrokerError::AllKeysExhausted`].
//! - No key / unknown provider → fail closed (error before any HTTP attempt).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use crate::Vault;
use crate::guarded_http::{MAX_PROVIDER_RESPONSE_BYTES, provider_agent_for, read_bounded_to};
use crate::keyring::{
    KeyRing, KeyRingError, KeyStatus, MAX_429_SWITCHES, RoutingPolicy, SelectedKey,
};
use crate::ledger::{Pricing, Usage, UsageRow, default_pricing};
use crate::oauth::{OAuthManager, is_oauth_provider};
use crate::session_budget::SessionBudget;
use agentcowork_guard::netfloor::NetPolicy;

/// Default OpenAI-compatible base URLs per provider (override via
/// `ProvidersFile.base_url`).
pub const DEFAULT_BASE_URLS: &[(&str, &str)] = &[
    ("nvidia", "https://integrate.api.nvidia.com/v1"),
    ("openai", "https://api.openai.com/v1"),
    ("anthropic", "https://api.anthropic.com/v1"),
    ("deepseek", "https://api.deepseek.com/v1"),
    ("groq", "https://api.groq.com/openai"),
    // P1.7 (A4): subscription accounts route through the same broker. The
    // stored OAuth tokens are injected as `Authorization: Bearer` by
    // `authorization()` (never `x-api-key`).
    ("chatgpt-pro", "https://chatgpt.com/backend-api/codex/v1"),
    ("copilot", "https://api.githubcopilot.com"),
    ("qwen", "https://portal.qwen.ai/v1"),
];

/// The HTTP dialects the broker can actually speak (P55.5).
///
/// A provider whose resolved transport is **not** listed here is simply never
/// registered as an endpoint, which is the honest failure: the broker then
/// behaves exactly as it did before this existed instead of POSTing an
/// OpenAI-shaped request at an Anthropic/Bedrock path and returning a
/// meaningless 404.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WireTransport {
    /// `POST {base}/chat/completions` (OpenAI-compatible family).
    #[default]
    OpenaiChat,
    /// `POST {base}/messages` (Anthropic Messages, incl. cache_control).
    AnthropicMessages,
}

/// A resolved provider endpoint: where to send, which dialect, which headers
/// (P55.5). Never a secret — the key still comes from the vault ring at send
/// time.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderEndpoint {
    /// Base URL **to the version root** (`…/v1`); the path is appended.
    pub base_url: String,
    pub transport: WireTransport,
    /// Static per-provider headers (e.g. `anthropic-version`).
    pub headers: Vec<(String, String)>,
    /// Inject the per-conversation OpenCode headers (P56.6).
    pub session_headers: bool,
    /// Keyless provider (P56.6 OpenCode Free, local proxies): never send an
    /// `Authorization` header, and do not require a key to exist.
    pub keyless: bool,
    /// Egress floor: this endpoint may reach loopback destinations (a local
    /// runtime or a local proxy). Mirrors the netfloor policy field of the same
    /// name, so the declaration and the floor check cannot drift into two
    /// vocabularies. Private/LAN destinations are never implied by this flag.
    pub allow_loopback: bool,
    /// Egress floor: this endpoint may reach private/LAN destinations. Off by
    /// default — that is the actual SSRF prize, and no provider needs it.
    pub allow_private: bool,
}

// Hand-written so the egress floor matches the platform default
// (`NetPolicy::default()`: loopback permitted, private/LAN refused). A derived
// `Default` would silently start refusing local runtimes, which are a
// first-class desktop workflow — the flag is an explicit opt-*out*, not a
// silent opt-in.
impl Default for ProviderEndpoint {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            transport: WireTransport::OpenaiChat,
            headers: Vec::new(),
            session_headers: false,
            keyless: false,
            allow_loopback: true,
            allow_private: false,
        }
    }
}

impl ProviderEndpoint {
    pub fn openai(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            transport: WireTransport::OpenaiChat,
            ..Default::default()
        }
    }

    pub fn anthropic(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            transport: WireTransport::AnthropicMessages,
            headers: vec![("anthropic-version".to_string(), "2023-06-01".to_string())],
            ..Default::default()
        }
    }

    /// The request URL for this dialect (P55.5 — the path is the transport's,
    /// never a hardcoded `/chat/completions`).
    pub fn request_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.transport {
            WireTransport::OpenaiChat => format!("{base}/chat/completions"),
            WireTransport::AnthropicMessages => format!("{base}/messages"),
        }
    }

    /// Declare that this endpoint may reach a local runtime over loopback. The
    /// flag is the endpoint's own statement of reach, and it is what the egress
    /// floor check reads — a local runtime is therefore explicit rather than
    /// assumed, and no other destination class is implied by it.
    pub fn with_loopback(mut self) -> Self {
        self.allow_loopback = true;
        self
    }

    /// Declare that this endpoint may reach private/LAN destinations. Off by
    /// default; an explicit opt-in per endpoint, never a global setting.
    pub fn with_private_network(mut self) -> Self {
        self.allow_private = true;
        self
    }

    /// The **model-listing** URL for this provider (P44.4 probe).
    ///
    /// Every dialect the broker speaks exposes its listing at `{base}/models`
    /// (OpenAI-compatible and Anthropic alike), so this is deliberately not a
    /// per-transport branch — and it is derived from the provider's own base
    /// URL, which is the only address a probe can reach.
    pub fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim().trim_end_matches('/'))
    }
}

/// P44.4 — the outcome of a broker-performed metadata probe.
///
/// Carries **no credential** — only what the endpoint answered. The body is
/// returned unparsed on purpose: model-listing shape is model-catalog business
/// (`agentcowork-catalog`), and teaching this crate to parse it would put catalog
/// knowledge inside the security boundary for no benefit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsProbe {
    /// Did the endpoint answer with a success status?
    pub ok: bool,
    /// HTTP status; `0` = transport failure (nothing was answered).
    pub status: u16,
    /// The URL that was probed.
    pub url: String,
    /// The response body (empty when nothing was answered).
    pub body: String,
    /// Transport-failure detail — `None` when the endpoint answered.
    pub error: Option<String>,
}

/// Is this URL safe to send a credential to?
///
/// `https` anywhere, or plain `http` to loopback only (a local runtime or a
/// local proxy — where the credential never touches a network). Anything else
/// is refused: a `http://` provider base URL would put the user's key on the
/// wire in cleartext. Same floor the shell's own health probe enforces, and
/// deliberately checked here too, because this is the call that attaches the
/// credential.
pub fn credential_safe_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return false;
    };
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(
            parsed
                .host()
                .map(|host| agentcowork_guard::netfloor::classify_url_host(&host)),
            Some(agentcowork_guard::NetClass::Loopback)
        ),
        _ => false,
    }
}

/// Incremental native function-call fragment (`choices[0].delta.tool_calls`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCallDelta {
    pub index: i64,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// One SSE event from a streaming chat completion.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatStreamEvent {
    /// Text delta from `choices[0].delta.content` (None on non-content events).
    pub delta: Option<String>,
    /// `finish_reason` when the stream finishes an answer.
    pub finish: Option<String>,
    /// Cache-aware usage observed on this chunk (P1.3/A9). OpenAI-compatible
    /// providers echo the full usage object in the final chunk when
    /// `stream_options.include_usage` was requested; Anthropic splits it
    /// (input/cache-write in `message_start`, output in `message_delta`).
    pub usage: Option<Usage>,
    /// Native tool-call fragments on this chunk (OpenAI `delta.tool_calls`).
    pub tool_calls: Vec<ToolCallDelta>,
}

/// The credential broker: key resolution + HTTP execution + scrubbing +
/// cache-aware cost accounting (A9) + per-session budget (J11).
pub struct Broker<'a> {
    ring: KeyRing<'a>,
    /// Vault handle for the append-only `token_usage` ledger.
    vault: &'a Vault,
    base_urls: HashMap<String, String>,
    policy: RoutingPolicy,
    /// Per-provider pricing (defaults from [`default_pricing`]; override via
    /// [`Broker::with_pricing`]).
    pricing: HashMap<String, Pricing>,
    /// Per-session hard $ budget (J11, default $2.00).
    budget: SessionBudget,
    /// P1.7 (A4): when attached, an HTTP 401 on an oauth provider triggers a
    /// token refresh + one retry before the error surfaces (doc 33 §7.4
    /// token lifecycle; failover semantics stay identical to BYOK keys).
    oauth: Option<OAuthManager<'a>>,
    /// P3.3 (J14): extra headers injected into every outbound HTTP request.
    /// Used for distributed-trace linkage (`traceparent`) and any future
    /// cross-boundary propagation.
    extra_headers: HashMap<String, String>,
    /// P55.5: resolved per-provider endpoints (base URL + dialect + headers),
    /// built from the live models.dev catalog + user-config profiles by the
    /// shell. Providers absent here keep the legacy `DEFAULT_BASE_URLS` path.
    endpoints: HashMap<String, ProviderEndpoint>,
}

impl<'a> Broker<'a> {
    pub fn new(vault: &'a Vault) -> Self {
        let mut base_urls = HashMap::new();
        for (provider, url) in DEFAULT_BASE_URLS {
            base_urls.insert((*provider).to_string(), (*url).to_string());
        }
        let mut pricing = HashMap::new();
        for (provider, _) in DEFAULT_BASE_URLS {
            if let Some(p) = default_pricing(provider) {
                pricing.insert((*provider).to_string(), p);
            }
        }
        Self {
            ring: KeyRing::new(vault),
            vault,
            base_urls,
            policy: RoutingPolicy::RoundRobin,
            pricing,
            budget: SessionBudget::default_budget(),
            oauth: None,
            extra_headers: HashMap::new(),
            endpoints: HashMap::new(),
        }
    }

    /// P55.5 — register a resolved endpoint for a provider (base URL +
    /// dialect + headers). Chainable; the shell builds these from the live
    /// catalog + user-config profiles at boot.
    pub fn with_endpoint(mut self, provider: &str, endpoint: ProviderEndpoint) -> Self {
        self.endpoints.insert(provider.to_string(), endpoint);
        self
    }

    /// The endpoint registered for a provider, if any.
    pub fn endpoint(&self, provider: &str) -> Option<&ProviderEndpoint> {
        self.endpoints.get(provider)
    }

    /// The URL a request for this provider goes to: the resolved endpoint's
    /// dialect path when one is registered, else the legacy default
    /// (`{base}/chat/completions`).
    fn request_url(&self, provider: &str) -> Result<String, BrokerError> {
        if let Some(ep) = self.endpoints.get(provider) {
            if !ep.base_url.trim().is_empty() {
                return Ok(ep.request_url());
            }
        }
        let base = self
            .base_urls
            .get(provider)
            .cloned()
            .ok_or_else(|| BrokerError::UnknownProvider(provider.to_string()))?;
        Ok(format!("{base}/chat/completions"))
    }

    /// **P44.4 — probe a provider's model listing with the vault-held
    /// credential.**
    ///
    /// This exists so the *observation* path never needs the plaintext key: the
    /// credential is resolved by the ring, attached to one `GET {base}/models`,
    /// and scrubbed — the caller receives only [`ModelsProbe`] (status, body,
    /// URL). It is the vault-mediated twin of the shell's user-key probe
    /// (`agentcowork_catalog::probe_models_endpoint`), for the case where no key
    /// is in hand because the key already lives in the vault.
    ///
    /// **Scope constraints, all deliberate:**
    ///
    /// - **The URL is the provider's own endpoint.** There is no URL parameter:
    ///   the target comes from the resolved [`ProviderEndpoint`] (or the legacy
    ///   default base URL), so a caller cannot point the credential elsewhere.
    /// - **`https` or loopback only** ([`credential_safe_url`]). A cleartext
    ///   remote endpoint is refused rather than handed the key.
    /// - **Not a turn.** It does not touch the session budget, the usage ledger,
    ///   or key health — no `report_success`, no `report_failure`, no cooldown,
    ///   no cost. A metadata call must not be able to cool a key down, spend a
    ///   daily cap, or `401`-suspend a credential. The credential comes from
    ///   [`KeyRing::reveal_for_metadata_probe`], which likewise skips affinity
    ///   and model filters.
    /// - **Keyless providers send no auth header at all** — the same rule the
    ///   chat path follows for local runtimes and the free overlays.
    pub fn probe_models(
        &self,
        provider: &str,
        timeout: Duration,
    ) -> Result<ModelsProbe, BrokerError> {
        let url = self.models_url(provider)?;
        // The probe is the other credential-bearing path, so it runs the same
        // custody + egress floor checks as the chat path (INV-05, CTR-013).
        if !credential_safe_url(&url) {
            return Err(BrokerError::InsecureEndpoint(provider.to_string()));
        }
        let endpoint = self.endpoints.get(provider).cloned();
        let keyless = endpoint.as_ref().map(|e| e.keyless).unwrap_or(false);
        let key = if keyless {
            None
        } else {
            Some(self.ring.reveal_for_metadata_probe(provider)?)
        };

        let agent = provider_agent_for(&url, self.floor_policy(provider), Some(timeout))
            .map_err(guarded_setup_error)?;
        let mut req = agent.get(&url).set("Accept", "application/json").set(
            "User-Agent",
            &format!("AgentCowork/{}", env!("CARGO_PKG_VERSION")),
        );
        if let Some(ep) = endpoint.as_ref() {
            for (name, value) in &ep.headers {
                req = req.set(name, value);
            }
        }
        if let Some(secret) = key.as_ref() {
            let (name, value) = authorization(provider, secret.as_bytes());
            req = req.set(name, value.as_str());
        }

        Ok(match req.call() {
            Ok(resp) => {
                let status = resp.status();
                let body = read_bounded_to(resp, MAX_PROVIDER_RESPONSE_BYTES)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                let body_ok = body.is_ok();
                ModelsProbe {
                    ok: (200..300).contains(&status) && body_ok,
                    status,
                    url,
                    body: body.clone().unwrap_or_default(),
                    error: body
                        .err()
                        .map(|error| format!("provider response unavailable: {error:?}")),
                }
            }
            // An answered error: the endpoint is reachable, so the status and
            // body (a 401/403/429 explains itself) are the honest observation.
            Err(ureq::Error::Status(status, resp)) => {
                let body = read_bounded_to(resp, MAX_PROVIDER_RESPONSE_BYTES)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                ModelsProbe {
                    ok: false,
                    status,
                    url,
                    body: body.clone().unwrap_or_default(),
                    error: body
                        .err()
                        .map(|error| format!("provider response unavailable: {error:?}")),
                }
            }
            // Nothing was answered at all — status 0 means "no response", never
            // a fabricated HTTP code.
            Err(ureq::Error::Transport(_)) => ModelsProbe {
                ok: false,
                status: 0,
                url,
                body: String::new(),
                error: Some("provider transport failed".into()),
            },
        })
    }

    /// The `GET {base}/models` URL for a provider, from its own resolved
    /// endpoint (falling back to the legacy default base URL).
    pub fn models_url(&self, provider: &str) -> Result<String, BrokerError> {
        if let Some(ep) = self.endpoints.get(provider) {
            if !ep.base_url.trim().is_empty() {
                return Ok(ep.models_url());
            }
        }
        let base = self
            .base_urls
            .get(provider)
            .cloned()
            .ok_or_else(|| BrokerError::UnknownProvider(provider.to_string()))?;
        Ok(format!("{}/models", base.trim().trim_end_matches('/')))
    }

    /// The dialect for a provider (`OpenaiChat` when nothing is registered).
    fn transport(&self, provider: &str) -> WireTransport {
        self.endpoints
            .get(provider)
            .map(|e| e.transport)
            .unwrap_or(WireTransport::OpenaiChat)
    }

    /// The netfloor policy this provider's declared egress permits.
    ///
    /// Derived from the endpoint's own declaration rather than a global
    /// setting, so a provider cannot be quietly widened and a local runtime has
    /// to say so. A provider with no registered endpoint falls back to the
    /// platform default (loopback permitted, private/LAN refused).
    pub fn floor_policy(&self, provider: &str) -> NetPolicy {
        match self.endpoints.get(provider) {
            Some(e) => NetPolicy {
                allow_loopback: e.allow_loopback,
                allow_private: e.allow_private,
                allow_local_names: e.allow_private,
            },
            None => NetPolicy::default(),
        }
    }

    /// The egress check both credential-bearing paths run: a cleartext-remote
    /// refusal (custody) and a netfloor preflight (INV-05), in that order, with
    /// a typed error for each and no fallback route after either.
    pub fn egress_preflight(&self, provider: &str, url: &str) -> Result<(), BrokerError> {
        if !credential_safe_url(url) {
            return Err(BrokerError::InsecureEndpoint(provider.to_string()));
        }
        agentcowork_guard::netfloor::preflight_url(url, self.floor_policy(provider)).map_err(
            |denied| BrokerError::EgressDenied {
                host: denied.url.clone(),
                reason: format!("{:?}", denied.reason),
            },
        )
    }

    /// Attach the OAuth manager so subscription accounts get 401→refresh→
    /// retry semantics (P1.7).
    pub fn with_oauth(mut self, oauth: OAuthManager<'a>) -> Self {
        self.oauth = Some(oauth);
        self
    }

    /// Override the per-1M-token pricing for a provider (A9).
    pub fn with_pricing(mut self, provider: &str, pricing: Pricing) -> Self {
        self.pricing.insert(provider.to_string(), pricing);
        self
    }

    /// Override the per-session $ budget limit (J11; default $2.00).
    pub fn with_session_budget_limit(mut self, limit: f64) -> Self {
        self.budget = SessionBudget::new(limit);
        self
    }

    /// Current session budget limit ($).
    pub fn session_budget_limit(&self) -> f64 {
        self.budget.limit()
    }

    /// $ spent so far by a session (in-memory tracker + ledger both record).
    pub fn session_spent(&self, session: &str) -> f64 {
        self.budget.spent(session)
    }

    /// $ remaining in a session's budget before the next call is refused.
    pub fn session_budget_remaining(&self, session: &str) -> f64 {
        self.budget.remaining(session)
    }

    /// Override the routing policy for key selection.
    pub fn with_policy(mut self, policy: RoutingPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Override a provider's base URL (e.g. from `providers.toml`).
    pub fn with_base_url(mut self, provider: &str, url: impl Into<String>) -> Self {
        self.base_urls.insert(provider.to_string(), url.into());
        self
    }

    /// P3.3 (J14): inject extra headers (e.g. `traceparent`) into every
    /// outbound HTTP request from this broker instance. The caller builds
    /// the header map (e.g. via `TraceContext::inject_headers`) and passes
    /// it here; the broker is decoupled from the tracing crate.
    pub fn with_extra_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.extra_headers.extend(headers);
        self
    }

    /// The key ring (for key-management surfaces + tests).
    pub fn ring(&self) -> &KeyRing<'a> {
        &self.ring
    }

    /// Non-streaming chat completion: `POST {base}/chat/completions`.
    pub fn chat_completion(
        &self,
        provider: &str,
        model: &str,
        session_id: &str,
        mut body: serde_json::Value,
    ) -> Result<serde_json::Value, BrokerError> {
        // P55.5 — the resolved endpoint (base URL + dialect) decides the URL
        // and the request/response shape; nothing here hardcodes a path.
        let endpoint = self.endpoints.get(provider).cloned();
        let transport = self.transport(provider);
        // P1.3 (A9): prompt-cache prefixing — Anthropic gets explicit
        // `cache_control:ephemeral` markers on the stable prefix; OpenAI-
        // compatible providers cache the ≥1024-token prefix automatically.
        annotate_prompt_cache(provider, &mut body);
        if transport == WireTransport::AnthropicMessages {
            body = openai_body_to_anthropic(&body);
        }
        let extra = self.extra_headers.clone();
        self.run_with_failover(
            provider,
            model,
            session_id,
            body,
            |agent, url, key, body| {
                let mut req = agent.post(url).set("Content-Type", "application/json");
                if let Some(k) = key {
                    let (name, value) = authorization(&k.provider, &k.value);
                    req = req.set(name, &value);
                }
                // P3.3 (J14) + P55.5 + P56.6: trace, endpoint and
                // per-conversation session headers.
                let req = decorate(req, &extra, endpoint.as_ref(), session_id);
                let resp = map_ureq_result(req.send_json(body))?;
                Ok(match transport {
                    WireTransport::OpenaiChat => resp,
                    WireTransport::AnthropicMessages => anthropic_response_to_openai(resp),
                })
            },
            // Cache-aware usage from the response's `usage` object (A9).
            |resp: &serde_json::Value| {
                resp.get("usage")
                    .and_then(Usage::from_any)
                    .unwrap_or_default()
            },
        )
    }

    /// Streaming chat completion: forces `stream: true` (+ include_usage so
    /// budgets stay accurate) and returns the parsed SSE event list (deltas +
    /// finish reasons). Usage is extracted from the final SSE chunk when the
    /// provider echoes it back (`stream_options.include_usage`).
    ///
    /// This is the **buffered** form — the whole upstream stream is consumed
    /// before it returns. A caller that has a downstream client to feed (the
    /// A8 local OpenAI-compatible server) wants
    /// [`Broker::chat_completion_stream_cb`] instead, which reports every event
    /// as it is parsed.
    pub fn chat_completion_stream(
        &self,
        provider: &str,
        model: &str,
        session_id: &str,
        body: serde_json::Value,
    ) -> Result<Vec<ChatStreamEvent>, BrokerError> {
        self.chat_completion_stream_cb(provider, model, session_id, body, &mut |_| {})
    }

    /// Streaming chat completion with **incremental delivery** (P9.5/A8).
    ///
    /// Identical contract to [`Broker::chat_completion_stream`] — the same
    /// `stream: true` + `include_usage` body shaping, the same dialect-specific
    /// SSE grammar (OpenAI `chat.completion.chunk` / Anthropic
    /// `content_block_delta`), the same cache-aware usage accounting and J11
    /// budget choke point, the same 429/401 failover — except every parsed
    /// event is also handed to `on_event` **as it is read off the socket**, so
    /// a downstream client sees real token-level streaming rather than one
    /// completed blob.
    ///
    /// `on_event` fires only for a **successful** attempt. A try that fails
    /// (429 → cool the key down and rotate, 401 → suspend the credential) has
    /// not produced a response body yet, so it emits nothing and the failover
    /// loop moves on without the caller having observed a partial answer. A
    /// transport error *mid*-body has already emitted, which the caller must
    /// surface in-band (the A8 server writes an SSE error frame) — that is the
    /// same posture as any real streaming endpoint.
    pub fn chat_completion_stream_cb(
        &self,
        provider: &str,
        model: &str,
        session_id: &str,
        mut body: serde_json::Value,
        on_event: &mut dyn FnMut(&ChatStreamEvent),
    ) -> Result<Vec<ChatStreamEvent>, BrokerError> {
        let endpoint = self.endpoints.get(provider).cloned();
        let transport = self.transport(provider);
        body["stream"] = serde_json::json!(true);
        body["stream_options"] = serde_json::json!({"include_usage": true});
        // P1.3 (A9): same prefixing as the non-streaming path.
        annotate_prompt_cache(provider, &mut body);
        if transport == WireTransport::AnthropicMessages {
            body = openai_body_to_anthropic(&body);
            // Anthropic has no `stream_options`; usage rides message_start /
            // message_delta instead (already parsed below).
            if let Some(obj) = body.as_object_mut() {
                obj.remove("stream_options");
            }
        }
        let extra = self.extra_headers.clone();
        // `run_with_failover`'s runner is `Fn`, but incremental delivery needs a
        // `&mut` callback. The `RefCell` bridges that without widening the
        // failover signature (the runner is invoked sequentially, never
        // concurrently, so the borrow cannot be live twice).
        let on_event_cell = std::cell::RefCell::new(on_event);
        self.run_with_failover(
            provider,
            model,
            session_id,
            body,
            |agent, url, key, body| {
                let mut req = agent.post(url).set("Content-Type", "application/json");
                if let Some(k) = key {
                    let (name, value) = authorization(&k.provider, &k.value);
                    req = req.set(name, &value);
                }
                // P3.3 (J14) + P55.5 + P56.6.
                let req = decorate(req, &extra, endpoint.as_ref(), session_id);
                match req.send_json(body) {
                    // The dialect decides the SSE grammar.
                    Ok(resp) => {
                        let mut cb = on_event_cell.borrow_mut();
                        Ok(match transport {
                            WireTransport::OpenaiChat => {
                                parse_sse_with(BufReader::new(resp.into_reader()), &mut **cb)
                            }
                            WireTransport::AnthropicMessages => parse_sse_anthropic_with(
                                BufReader::new(resp.into_reader()),
                                &mut **cb,
                            ),
                        })
                    }
                    Err(ureq::Error::Status(429, resp)) => Err(BrokerError::RateLimited {
                        retry_after_secs: parse_retry_after(&resp),
                    }),
                    Err(ureq::Error::Status(code, resp)) => {
                        Err(BrokerError::Http(code, read_snippet(resp)))
                    }
                    Err(ureq::Error::Transport(_)) => {
                        Err(BrokerError::Transport("provider transport failed".into()))
                    }
                }
            },
            // Cache-aware usage merged from the stream's usage chunks (A9).
            |events: &Vec<ChatStreamEvent>| usage_from_stream(events.as_slice()),
        )
    }

    /// Shared failover loop: resolve the endpoint → select a key (unless the
    /// endpoint is keyless) → run → on success record health + cache-aware
    /// cost + ledger + session budget.
    ///
    /// P56.8 failover semantics:
    /// * **429** → cool that key down for the provider's own `Retry-After`
    ///   when it sent one (else the ring's exponential backoff), then fail
    ///   over to the next key, bounded by [`MAX_429_SWITCHES`].
    /// * **401/403** → the credential itself is refused: suspend that key so
    ///   it stops being selected and try the next one. **No 5xx rotation** —
    ///   a provider hiccup is not a credential problem and must surface.
    /// * **keyless** → no ring interaction at all (P56.6 OpenCode Free); a 429
    ///   surfaces honestly instead of pretending another key exists.
    ///
    /// J11 choke point: the session budget is checked at the TOP of every
    /// attempt — a session at/over its $ limit is refused before any key is
    /// selected or any HTTP attempt is made.
    fn run_with_failover<T>(
        &self,
        provider: &str,
        model: &str,
        session_id: &str,
        body: serde_json::Value,
        runner: impl Fn(
            &ureq::Agent,
            &str,
            Option<&SelectedKey>,
            serde_json::Value,
        ) -> Result<T, BrokerError>,
        usage_of: impl Fn(&T) -> Usage,
    ) -> Result<T, BrokerError> {
        let url = self.request_url(provider)?;
        // INV-05 / REQ-PROV-008 — bind the checked Guard destination set to the
        // actual HTTP resolver before selecting a credential. This prevents a
        // second DNS resolution from changing the destination and disables
        // redirects. The agent is per-call and cannot be retargeted.
        if !credential_safe_url(&url) {
            return Err(BrokerError::InsecureEndpoint(provider.to_string()));
        }
        let agent = provider_agent_for(&url, self.floor_policy(provider), None)
            .map_err(guarded_setup_error)?;
        let keyless = self
            .endpoints
            .get(provider)
            .map(|e| e.keyless)
            .unwrap_or(false);

        if !self.budget.can_issue(session_id) {
            return Err(BrokerError::SessionBudgetExceeded {
                session: session_id.to_string(),
                limit: self.budget.limit(),
                spent: self.budget.spent(session_id),
            });
        }

        let mut switches = 0u32;
        // P1.7: a 401 on an oauth provider refreshes the token exactly once
        // per call before failover/exhaustion logic takes over.
        let mut refreshed = false;
        loop {
            let key: Option<SelectedKey> = if keyless {
                None
            } else {
                match self.ring.select(provider, model, session_id, self.policy) {
                    Ok(k) => Some(k),
                    Err(KeyRingError::AllKeysExhausted(p)) => {
                        return Err(BrokerError::AllKeysExhausted(p));
                    }
                    Err(e) => return Err(BrokerError::KeyRing(e)),
                }
            };

            match runner(&agent, &url, key.as_ref(), body.clone()) {
                Ok(result) => {
                    let usage = usage_of(&result);
                    // A keyless turn is genuinely $0 — never priced against a
                    // provider table, never ring-recorded (there is no ring
                    // row), but it still lands in the durable ledger.
                    let cost = if keyless {
                        0.0
                    } else {
                        self.cost_of(provider, usage)
                    };
                    let key_id = match &key {
                        Some(k) => {
                            // Success: health + cost on the ring row.
                            self.ring
                                .report_success(&k.opaque_handle)
                                .map_err(BrokerError::KeyRing)?;
                            self.ring
                                .report_usage(&k.opaque_handle, usage.total(), cost)
                                .map_err(BrokerError::KeyRing)?;
                            k.key_id.clone()
                        }
                        None => String::new(),
                    };
                    // One append-only ledger row per call (ARCH/05 §5.6).
                    self.vault
                        .record_usage(&UsageRow {
                            session: session_id.to_string(),
                            provider: provider.to_string(),
                            model: model.to_string(),
                            key_id,
                            usage,
                            cost,
                            tool: None,
                            // Cloud broker turns carry no task scope here —
                            // scoped callers record via `record_usage_scoped`.
                            task_id: String::new(),
                            run_id: String::new(),
                            work_id: String::new(),
                        })
                        .map_err(BrokerError::Vault)?;
                    // J11: settle the session; the next call is refused once
                    // spent ≥ limit.
                    self.budget.settle(session_id, cost);
                    return Ok(result);
                }
                Err(BrokerError::RateLimited { retry_after_secs }) => {
                    let Some(k) = key else {
                        // Keyless: there is no second credential to fail over
                        // to, so surface the 429 with whatever hint came back.
                        return Err(BrokerError::RateLimited { retry_after_secs });
                    };
                    // P56.8: honour the provider's own Retry-After when it sent
                    // one; otherwise the ring's exponential backoff stands.
                    match retry_after_secs {
                        Some(secs) => self
                            .ring
                            .report_failure_retry_after(&k.opaque_handle, secs)
                            .map_err(BrokerError::KeyRing)?,
                        None => self
                            .ring
                            .report_failure(&k.opaque_handle, true)
                            .map_err(BrokerError::KeyRing)?,
                    }
                    switches += 1;
                    if switches > MAX_429_SWITCHES {
                        return Err(BrokerError::AllKeysExhausted(provider.to_string()));
                    }
                }
                Err(e) => {
                    let status = match &e {
                        BrokerError::Http(code, _) => Some(*code),
                        _ => None,
                    };
                    if let Some(k) = &key {
                        // P1.7: on 401 for an oauth-backed provider, refresh
                        // the account's token and retry once — checked BEFORE
                        // the suspend path so a refreshable token is never
                        // thrown away.
                        let refreshable = !refreshed
                            && is_oauth_provider(provider)
                            && self.oauth.as_ref().map(|o| o.enabled()).unwrap_or(false);
                        if refreshable && status == Some(401) {
                            self.ring
                                .report_failure(&k.opaque_handle, false)
                                .map_err(BrokerError::KeyRing)?;
                            let ok = self
                                .oauth
                                .as_ref()
                                .unwrap()
                                .refresh(provider, &k.key_id)
                                .is_ok();
                            if ok {
                                refreshed = true;
                                continue;
                            }
                        }
                        // P56.8: a 401/403 means this credential is refused —
                        // suspend it and try the next key. 5xx never rotates.
                        if matches!(status, Some(401) | Some(403)) {
                            self.ring
                                .set_status(&k.provider, &k.key_id, KeyStatus::Suspended)
                                .map_err(BrokerError::KeyRing)?;
                            switches += 1;
                            if switches > MAX_429_SWITCHES {
                                return Err(e);
                            }
                            continue;
                        }
                        // Everything else: record health, surface honestly.
                        self.ring
                            .report_failure(&k.opaque_handle, false)
                            .map_err(BrokerError::KeyRing)?;
                    }
                    return Err(e);
                }
            }
        }
    }
}

/// P1.3 (A9) — prompt-cache prefixing. Anthropic requires an explicit
/// `cache_control: {"type":"ephemeral"}` marker on the content blocks that
/// should anchor the cache (system + the final message); OpenAI-compatible
/// providers cache the ≥1024-token prefix automatically and need no marker.
/// This mutates the outgoing body in place so the cached prefix is the same
/// byte-stable block the coordinator already keeps above its cache boundary.
fn annotate_prompt_cache(provider: &str, body: &mut serde_json::Value) {
    if provider != "anthropic" {
        return; // OpenAI-compatible: automatic ≥1024-token prefix caching.
    }
    // Anthropic `system` can be a bare string — promote it to a content block
    // list so the ephemeral marker can attach (a plain string is also valid
    // input but cannot carry a cache_control hint).
    if let Some(system) = body.get_mut("system") {
        if let Some(text) = system.as_str().map(str::to_string) {
            *system = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": { "type": "ephemeral" }
            }]);
        }
    }
    // Attach the marker to the last message's final content block (the
    // conventional Anthropic breakpoint; the provider computes the cache from
    // the marked prefix backward).
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        if let Some(last) = messages.last_mut() {
            if let Some(obj) = last.as_object_mut() {
                if let Some(text) = obj
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(str::to_string)
                {
                    obj.insert(
                        "content".into(),
                        serde_json::json!([{
                            "type": "text",
                            "text": text,
                            "cache_control": { "type": "ephemeral" }
                        }]),
                    );
                } else if let Some(arr) = obj.get_mut("content").and_then(|c| c.as_array_mut()) {
                    if let Some(block) = arr.last_mut() {
                        block["cache_control"] = serde_json::json!({ "type": "ephemeral" });
                    }
                }
            }
        }
    }
}

/// Build the auth header for a provider. Returns `(name, value)`; the value
/// buffer is dropped right after the request and its bytes are never logged.
/// The header string is `Zeroizing`-wrapped so the secret bytes are scrubbed
/// when the header buffer is dropped.
fn authorization(provider: &str, secret: &[u8]) -> (&'static str, zeroize::Zeroizing<String>) {
    let secret_str = zeroize::Zeroizing::new(String::from_utf8_lossy(secret).into_owned());
    match provider {
        "anthropic" => ("x-api-key", secret_str),
        _ => (
            "Authorization",
            zeroize::Zeroizing::new(format!("Bearer {}", secret_str.as_str())),
        ),
    }
}

fn map_ureq_result(
    result: Result<ureq::Response, ureq::Error>,
) -> Result<serde_json::Value, BrokerError> {
    match result {
        Ok(resp) => {
            let bytes = read_bounded_to(resp, MAX_PROVIDER_RESPONSE_BYTES).map_err(|error| {
                BrokerError::Transport(format!("provider response unavailable: {error:?}"))
            })?;
            serde_json::from_slice(&bytes)
                .map_err(|_| BrokerError::Transport("provider returned invalid JSON".into()))
        }
        Err(ureq::Error::Status(429, resp)) => Err(BrokerError::RateLimited {
            retry_after_secs: parse_retry_after(&resp),
        }),
        Err(ureq::Error::Status(code, resp)) => Err(BrokerError::Http(code, read_snippet(resp))),
        Err(ureq::Error::Transport(_)) => {
            Err(BrokerError::Transport("provider transport failed".into()))
        }
    }
}

/// P56.8 — parse a 429's `Retry-After` (delta-seconds form only).
///
/// The live OpenCode Zen 429 carries **no** `Retry-After` at all (verified
/// 2026-09-11), so `None` is the common case and the ring's exponential
/// cooldown is what actually paces the retry. HTTP-date values are not
/// interpreted — guessing a client clock skew is worse than no hint.
fn parse_retry_after(resp: &ureq::Response) -> Option<u64> {
    let raw = resp.header("retry-after")?.trim().to_string();
    let secs: u64 = raw.parse().ok()?;
    Some(secs.min(24 * 60 * 60))
}

/// Per-conversation client-identity + session-affinity headers (P56.6,
/// `REQ-PROV-009` / `DEC-035`).
///
/// A gateway-class provider may require (a) a client User-Agent identifying the
/// **actual** client and (b) a session-affinity header carrying one stable value
/// per conversation. The deployed gateway reads the session id from both
/// `X-Session-Id` and `x-opencode-session`; a missing session is a
/// 400 `MissingSessionID`.
///
/// The conversation id is the caller's own `session_id`: a new conversation is a
/// new session, and one conversation keeps one value across every turn
/// (stability across compaction and restarts is the provider plane's
/// responsibility to preserve the same id). The identity is **ours** — never an
/// impersonated agent and never a generic SDK name.
///
/// Public so a host can assert that what goes on the wire matches the identity
/// policy the provider registry declares; the names themselves are also
/// asserted in `agentcowork-core` against `GatewayIdentityPolicy`, so the two
/// vocabularies cannot drift.
pub fn gateway_identity_headers(session_id: &str, request_id: &str) -> Vec<(&'static str, String)> {
    let sid = {
        let t = session_id.trim();
        if t.is_empty() { "agentcowork-anon" } else { t }
    };
    vec![
        ("X-Session-Id", sid.to_string()),
        ("x-opencode-session", sid.to_string()),
        ("x-opencode-request", request_id.to_string()),
        ("x-opencode-client", "cli".to_string()),
        (
            "User-Agent",
            format!("AgentCowork/{}", env!("CARGO_PKG_VERSION")),
        ),
    ]
}

/// A fresh per-request correlation id for [`gateway_identity_headers`].
pub fn new_request_id() -> String {
    format!("req-{:016x}", rand::random::<u64>())
}

/// Per-conversation OpenCode headers (P56.6).
fn session_headers(session_id: &str) -> Vec<(&'static str, String)> {
    gateway_identity_headers(session_id, &new_request_id())
}

/// Header names the broker owns: the client identity and the per-conversation
/// affinity values. A provider endpoint or a trace header may not set them —
/// `ureq` appends a repeated header rather than replacing it, so a second
/// value would ride alongside ours on the wire and a gateway reading the wrong
/// one would see a forged identity or a split session (`REQ-PROV-009`).
const RESERVED_IDENTITY_HEADERS: &[&str] = &[
    "user-agent",
    "x-opencode-session",
    "x-session-id",
    "x-opencode-request",
    "x-opencode-client",
];

/// Is this a header the broker sets itself?
fn is_reserved_identity_header(name: &str) -> bool {
    let lowered = name.trim().to_ascii_lowercase();
    RESERVED_IDENTITY_HEADERS.contains(&lowered.as_str())
}

/// Apply trace + endpoint + session headers to a request builder.
///
/// Order is deliberate and the reserved-name filter is the load-bearing part:
/// caller-supplied headers are applied first, then the endpoint's, then the
/// broker's own identity last — and any attempt to pre-set a reserved name is
/// dropped rather than appended.
fn decorate(
    mut req: ureq::Request,
    extra: &HashMap<String, String>,
    endpoint: Option<&ProviderEndpoint>,
    session_id: &str,
) -> ureq::Request {
    for (k, v) in extra {
        if is_reserved_identity_header(k) {
            continue;
        }
        req = req.set(k, v);
    }
    if let Some(ep) = endpoint {
        for (k, v) in &ep.headers {
            if is_reserved_identity_header(k) {
                continue;
            }
            req = req.set(k, v);
        }
        if ep.session_headers {
            for (k, v) in session_headers(session_id) {
                req = req.set(k, &v);
            }
        }
    }
    req
}

/// Anthropic's API requires `max_tokens`; our relay body does not carry one.
/// 4096 matches the upstream client's default rather than inventing a value.
const ANTHROPIC_DEFAULT_MAX_TOKENS: u64 = 4096;

/// Anthropic message content: keep the block form (which is what
/// `annotate_prompt_cache` produces for the cached prefix) and otherwise pass
/// the plain text through.
fn anthropic_content(content: Option<&serde_json::Value>) -> serde_json::Value {
    match content {
        Some(serde_json::Value::String(s)) => serde_json::json!(s),
        Some(serde_json::Value::Array(blocks)) => {
            let mapped: Vec<serde_json::Value> = blocks
                .iter()
                .filter_map(|b| {
                    let text = b.get("text").and_then(|t| t.as_str())?;
                    let mut out = serde_json::json!({ "type": "text", "text": text });
                    if let Some(cc) = b.get("cache_control") {
                        out["cache_control"] = cc.clone();
                    }
                    Some(out)
                })
                .collect();
            if mapped.is_empty() {
                serde_json::json!("")
            } else {
                serde_json::Value::Array(mapped)
            }
        }
        _ => serde_json::json!(""),
    }
}

/// Translate the relay's OpenAI-shaped request body into Anthropic Messages
/// (P55.5). Deliberately text/tool only — no fabricated image or tool-result
/// mapping, so an unsupported shape degrades to plain text rather than a
/// silently wrong request.
fn openai_body_to_anthropic(body: &serde_json::Value) -> serde_json::Value {
    let mut system = String::new();
    let mut messages: Vec<serde_json::Value> = Vec::new();
    if let Some(arr) = body.get("messages").and_then(|m| m.as_array()) {
        for m in arr {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = anthropic_content(m.get("content"));
            if role == "system" {
                if let Some(s) = content.as_str() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(s);
                }
                continue;
            }
            let role = if role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            messages.push(serde_json::json!({ "role": role, "content": content }));
        }
    }
    let mut out = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::json!("")),
        "messages": messages,
        "max_tokens": body
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS),
    });
    if !system.is_empty() {
        out["system"] = serde_json::json!(system);
    }
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let mapped: Vec<serde_json::Value> = tools
            .iter()
            .filter_map(|t| {
                let f = t.get("function")?;
                let name = f.get("name")?.as_str()?;
                Some(serde_json::json!({
                    "name": name,
                    "description": f.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                    "input_schema": f
                        .get("parameters")
                        .cloned()
                        .unwrap_or(serde_json::json!({ "type": "object" })),
                }))
            })
            .collect();
        if !mapped.is_empty() {
            out["tools"] = serde_json::Value::Array(mapped);
        }
    }
    if let Some(stream) = body.get("stream") {
        out["stream"] = stream.clone();
    }
    if let Some(temp) = body.get("temperature") {
        out["temperature"] = temp.clone();
    }
    out
}

/// Translate an Anthropic Messages response back into the OpenAI shape the
/// relay consumes, including the cache-aware usage fields `Usage::from_any`
/// already understands (`cache_read_input_tokens` / `cache_creation_input_tokens`).
fn anthropic_response_to_openai(resp: serde_json::Value) -> serde_json::Value {
    let blocks = resp.get("content").and_then(|c| c.as_array());
    let text: String = blocks
        .map(|a| {
            a.iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let tool_calls: Vec<serde_json::Value> = blocks
        .map(|a| {
            a.iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .map(|b| {
                    serde_json::json!({
                        "id": b.get("id").cloned().unwrap_or(serde_json::json!("")),
                        "type": "function",
                        "function": {
                            "name": b.get("name").cloned().unwrap_or(serde_json::json!("")),
                            "arguments": b
                                .get("input")
                                .map(|i| i.to_string())
                                .unwrap_or_else(|| "{}".to_string()),
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let finish = match resp.get("stop_reason").and_then(|s| s.as_str()) {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    };
    let usage = resp.get("usage").cloned().unwrap_or(serde_json::json!({}));
    let u64_of = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    serde_json::json!({
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::json!(text) },
                "tool_calls": tool_calls,
            },
            "finish_reason": finish,
        }],
        "usage": {
            "prompt_tokens": u64_of("input_tokens"),
            "completion_tokens": u64_of("output_tokens"),
            "cache_read_input_tokens": u64_of("cache_read_input_tokens"),
            "cache_creation_input_tokens": u64_of("cache_creation_input_tokens"),
        }
    })
}

/// Anthropic SSE → the same [`ChatStreamEvent`] stream the OpenAI parser
/// yields (P55.5). Events are keyed off the JSON `type` field, not the
/// `event:` line, because the payload is the contract.
///
/// Incremental delivery: every parsed event is also handed to `on_event` the
/// moment it is read (P9.5/A8). There is no separate buffered form — the broker
/// is the only non-test caller and always wants the callback.
pub(crate) fn parse_sse_anthropic_with<R: BufRead>(
    mut reader: R,
    on_event: &mut dyn FnMut(&ChatStreamEvent),
) -> Vec<ChatStreamEvent> {
    let mut events = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        let Some(payload) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload.trim()) else {
            continue;
        };
        match value.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "content_block_delta" => {
                if let Some(text) = value
                    .get("delta")
                    .and_then(|d| d.get("text"))
                    .and_then(|t| t.as_str())
                {
                    let ev = ChatStreamEvent {
                        delta: Some(text.to_string()),
                        ..Default::default()
                    };
                    on_event(&ev);
                    events.push(ev);
                }
            }
            "message_delta" => {
                let finish = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|s| s.as_str())
                    .map(|s| {
                        match s {
                            "max_tokens" => "length",
                            "tool_use" => "tool_calls",
                            _ => "stop",
                        }
                        .to_string()
                    });
                let usage = value.get("usage").and_then(Usage::from_any);
                let ev = ChatStreamEvent {
                    finish,
                    usage,
                    ..Default::default()
                };
                on_event(&ev);
                events.push(ev);
            }
            "message_start" => {
                if let Some(usage) = value
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(Usage::from_any)
                {
                    let ev = ChatStreamEvent {
                        usage: Some(usage),
                        ..Default::default()
                    };
                    on_event(&ev);
                    events.push(ev);
                }
            }
            _ => {}
        }
    }
    events
}

fn read_snippet(resp: ureq::Response) -> String {
    let mut bytes = Vec::new();
    let _ = resp.into_reader().take(4096).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).chars().take(200).collect()
}

fn guarded_setup_error(error: crate::guarded_http::EgressSetupError) -> BrokerError {
    BrokerError::EgressDenied {
        host: error.url,
        reason: error.reason.into(),
    }
}
/// Merge the cache-aware usage observed across a stream (A9). OpenAI-compatible
/// providers put the full `usage` object in the LAST SSE chunk when
/// `stream_options.include_usage` was requested; Anthropic splits input/
/// cache-write (`message_start`) from output (`message_delta`) — merge_max
/// keeps every field without double counting.
fn usage_from_stream(events: &[ChatStreamEvent]) -> Usage {
    let mut acc = Usage::default();
    for e in events {
        if let Some(u) = e.usage {
            acc.merge_max(u);
        }
    }
    acc
}

/// Per-provider cost for a call (A9): cached input is never double-billed.
fn cost_of_usage(pricing: &HashMap<String, Pricing>, provider: &str, usage: Usage) -> f64 {
    pricing
        .get(provider)
        .copied()
        .unwrap_or_default()
        .cost_of(usage)
}

/// Parse OpenAI-style SSE stream into events while handing each event to
/// `on_event` as it is read off the socket (P9.5/A8).
pub(crate) fn parse_sse_with<R: BufRead>(
    mut reader: R,
    on_event: &mut dyn FnMut(&ChatStreamEvent),
) -> Vec<ChatStreamEvent> {
    let mut events = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed == "data: [DONE]" {
            break;
        }
        let Some(payload) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        let choice = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first());
        let delta = choice
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
            .map(str::to_string)
            // Anthropic-style SSE (`content_block_delta`): `delta.text`.
            .or_else(|| {
                value
                    .get("delta")
                    .and_then(|d| d.get("text"))
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
            });
        let finish = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|f| f.as_str())
            .map(str::to_string);
        let usage = value
            .get("usage")
            .or_else(|| value.get("message").and_then(|m| m.get("usage")))
            .and_then(Usage::from_any);
        let tool_calls = choice
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("tool_calls"))
            .and_then(|t| t.as_array())
            .map(|arr| arr.iter().filter_map(parse_tool_call_delta).collect())
            .unwrap_or_default();
        let ev = ChatStreamEvent {
            delta,
            finish,
            usage,
            tool_calls,
        };
        on_event(&ev);
        events.push(ev);
    }
    events
}

fn parse_tool_call_delta(v: &serde_json::Value) -> Option<ToolCallDelta> {
    let function = v.get("function");
    let name = function
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string);
    let arguments = function.and_then(|f| f.get("arguments")).and_then(|a| {
        if let Some(s) = a.as_str() {
            Some(s.to_string())
        } else if a.is_object() || a.is_array() {
            Some(a.to_string())
        } else {
            None
        }
    });
    let id = v.get("id").and_then(|s| s.as_str()).map(str::to_string);
    let index = v.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
    if id.is_none() && name.is_none() && arguments.is_none() {
        return None;
    }
    Some(ToolCallDelta {
        index,
        id,
        name,
        arguments,
    })
}

/// Merge streamed `delta.tool_calls` fragments into complete (name, args) pairs.
pub fn assemble_tool_calls(
    events: &[ChatStreamEvent],
    finished_by_length: bool,
) -> Vec<(String, serde_json::Value)> {
    use std::collections::BTreeMap;
    let mut by_index: BTreeMap<i64, (String, String, String)> = BTreeMap::new();
    for ev in events {
        for d in &ev.tool_calls {
            let entry = by_index
                .entry(d.index)
                .or_insert_with(|| (String::new(), String::new(), String::new()));
            if let Some(id) = &d.id {
                if !id.is_empty() {
                    entry.0 = id.clone();
                }
            }
            if let Some(name) = &d.name {
                if !name.is_empty() {
                    entry.1 = name.clone();
                }
            }
            if let Some(args) = &d.arguments {
                entry.2.push_str(args);
            }
        }
    }
    by_index
        .into_values()
        .filter_map(|(_id, name, args)| {
            if name.is_empty() {
                return None;
            }
            // Empty args = a legitimate no-arg tool call → `{}`.
            if args.is_empty() {
                return Some((name, serde_json::json!({})));
            }
            let parsed = serde_json::from_str::<serde_json::Value>(&args);
            match parsed {
                Ok(v) => Some((name, v)),
                Err(_) => {
                    // Fail-closed on truncation (pi stopReason==="length" →
                    // failToolCallsFromTruncatedMessage; spec B1 "fail truncated
                    // tool calls"): a call whose args were cut by the context
                    // limit is borked — never execute it, never hand it the
                    // `_raw` garbage. On a *clean* finish the `_raw` rescue is
                    // still a last-resort for genuinely non-JSON args.
                    if finished_by_length {
                        None
                    } else {
                        Some((name, serde_json::json!({ "_raw": args })))
                    }
                }
            }
        })
        .collect()
}

/// JSON-mode: parse a constrained object into tool calls.
/// Accepts `{"tool":"…","args":{…}}`, `{"name":"…","arguments":{…}}`,
/// and `{"tool_calls":[…]}`. Arbitrary JSON (no tool name) is ignored.
pub fn extract_json_tool_calls(text: &str) -> Vec<(String, serde_json::Value)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let unfenced = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let value = parse_json_object(unfenced);
    match value {
        Some(v) => json_value_to_tool_calls(&v),
        None => Vec::new(),
    }
}

fn parse_json_object(text: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        return Some(v);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

fn json_value_to_tool_calls(value: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    if let Some(arr) = value.get("tool_calls").and_then(|t| t.as_array()) {
        return arr.iter().flat_map(json_value_to_tool_calls).collect();
    }
    let name = value
        .get("tool")
        .and_then(|t| t.as_str())
        .or_else(|| value.get("name").and_then(|t| t.as_str()))
        .or_else(|| {
            value
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
        })
        .unwrap_or("");
    if name.is_empty() {
        return Vec::new();
    }
    let raw = value
        .get("args")
        .or_else(|| value.get("arguments"))
        .or_else(|| value.get("function").and_then(|f| f.get("arguments")));
    let args = match raw {
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({ "_raw": s }))
        }
        Some(v) if v.is_object() => v.clone(),
        Some(v) => serde_json::json!({ "value": v }),
        None => serde_json::json!({}),
    };
    vec![(name.to_string(), args)]
}

/// Extract usage token counts from a completion response (for budgets).
pub fn usage_tokens(response: &serde_json::Value) -> u64 {
    response
        .get("usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("key-ring error: {0}")]
    KeyRing(#[from] KeyRingError),
    #[error("HTTP {0}: {1}")]
    Http(u16, String),
    /// P56.8 — 429. `retry_after_secs` is the provider's own `Retry-After`
    /// hint when it sent one (`None` is honest, not a fabricated number).
    #[error("rate limited (429){}", match .retry_after_secs { Some(s) => format!(" — retry after {s}s"), None => String::new() })]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("transport error: {0}")]
    Transport(String),
    /// P44.4 — the provider's endpoint would send the credential in cleartext.
    #[error("refusing to probe '{0}': its endpoint is neither https nor loopback")]
    InsecureEndpoint(String),
    /// INV-05 / `REQ-PROV-008` — the egress floor refused the destination. The
    /// destination and the reason are named; there is no fallback route, so a
    /// denial is terminal for the call rather than a retry elsewhere.
    #[error("egress denied for {host}: {reason}")]
    EgressDenied { host: String, reason: String },
    #[error("all keys for provider '{0}' exhausted after 429 failover")]
    AllKeysExhausted(String),
    #[error("session '{session}' stopped: ${limit:.2} limit (spent ${spent:.2})")]
    SessionBudgetExceeded {
        session: String,
        limit: f64,
        spent: f64,
    },
    #[error("vault error: {0}")]
    Vault(#[from] crate::VaultError),
}

impl<'a> Broker<'a> {
    /// $ cost of a call under the provider's configured pricing (A9).
    fn cost_of(&self, provider: &str, usage: Usage) -> f64 {
        cost_of_usage(&self.pricing, provider, usage)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeySpec, KeyStatus, Vault};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn vault() -> &'static Vault {
        Box::leak(Box::new(Vault::open_in_memory("test-key").unwrap()))
    }

    fn spec(provider: &str, key_id: &str, value: &str) -> KeySpec {
        KeySpec {
            provider: provider.into(),
            key_id: key_id.into(),
            value: value.as_bytes().to_vec(),
            status: KeyStatus::Primary,
            model_filter: vec![],
            priority: 100,
            daily_token_cap: None,
            daily_cost_cap: None,
        }
    }

    // ---- P44.4 — the vault-mediated metadata probe -------------------------

    #[test]
    fn credential_safe_url_allows_https_and_loopback_only() {
        assert!(credential_safe_url("https://api.openai.com/v1/models"));
        assert!(credential_safe_url("http://127.0.0.1:8080/v1/models"));
        assert!(credential_safe_url("http://localhost:8000/models"));
        assert!(credential_safe_url("http://[::1]:11434/v1/models"));
        // Cleartext to a remote host would put the user's key on the wire.
        assert!(!credential_safe_url("http://api.example.com/v1/models"));
        assert!(!credential_safe_url("ftp://example.com/x"));
        assert!(!credential_safe_url(""));
    }

    #[test]
    fn models_url_prefers_the_resolved_endpoint_then_the_default() {
        let vault = vault();
        assert_eq!(
            Broker::new(vault).models_url("openai").unwrap(),
            "https://api.openai.com/v1/models"
        );
        // A registered endpoint wins, and a trailing slash cannot double up.
        let broker = Broker::new(vault).with_endpoint(
            "openai",
            ProviderEndpoint::openai("https://proxy.example.com/v1/"),
        );
        assert_eq!(
            broker.models_url("openai").unwrap(),
            "https://proxy.example.com/v1/models"
        );
        assert!(matches!(
            broker.models_url("definitely-not-a-provider").unwrap_err(),
            BrokerError::UnknownProvider(_)
        ));
    }

    /// The point of this path: the credential comes from the vault, is attached
    /// by the broker, and never reaches the caller.
    #[test]
    fn probe_models_attaches_the_vault_key_and_returns_the_body() {
        let server = mock_server(|req| {
            assert!(
                req.starts_with("GET /models"),
                "the probe must GET /models: {req}"
            );
            assert!(
                req.to_lowercase()
                    .contains("authorization: bearer sk-probe"),
                "the vault-held key must be attached: {req}"
            );
            (200, r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#.to_string())
        });
        let vault = vault();
        let _ = KeyRing::new(vault)
            .add_key(spec("probe-live", "k", "sk-probe"))
            .unwrap();
        let broker = Broker::new(vault)
            .with_endpoint("probe-live", ProviderEndpoint::openai(server.clone()));
        let probe = broker
            .probe_models("probe-live", Duration::from_secs(5))
            .unwrap();
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.status, 200);
        assert_eq!(probe.url, format!("{server}/models"));
        assert!(probe.body.contains("m1"));
        assert!(probe.error.is_none());
        // The body is returned unparsed — model-listing shape is the catalog
        // crate's business, and this crate must not learn it.
        assert!(probe.body.contains("m2"));
    }

    /// An answered rejection is a real observation, and it must NOT be treated
    /// as a credential failure: a metadata call cannot suspend or cool down the
    /// user's key.
    #[test]
    fn probe_models_records_a_rejection_without_moving_key_health() {
        let server = mock_server(|_| (401, r#"{"error":"invalid api key"}"#.to_string()));
        let vault = vault();
        let ring = KeyRing::new(vault);
        let _ = ring.add_key(spec("probe-401", "k", "sk-bad")).unwrap();
        let broker =
            Broker::new(vault).with_endpoint("probe-401", ProviderEndpoint::openai(server));
        let probe = broker
            .probe_models("probe-401", Duration::from_secs(5))
            .unwrap();
        assert!(!probe.ok);
        assert_eq!(probe.status, 401);
        assert!(
            probe.error.is_none(),
            "the endpoint answered, so this is not a transport failure"
        );

        let info = ring.list("probe-401").unwrap();
        assert_eq!(
            info[0].fail_count, 0,
            "a probe must not mark a credential failed"
        );
        assert!(!info[0].in_cooldown);
        assert_eq!(info[0].last_used_at, 0, "a probe is not usage");
    }

    #[test]
    fn probe_models_reports_a_transport_failure_as_status_zero() {
        // Bind and drop, so the port is closed well before the probe dials it.
        let base = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let a = l.local_addr().unwrap();
            drop(l);
            format!("http://{a}")
        };
        let vault = vault();
        let _ = KeyRing::new(vault)
            .add_key(spec("probe-dead", "k", "sk-dead"))
            .unwrap();
        let broker = Broker::new(vault).with_endpoint("probe-dead", ProviderEndpoint::openai(base));
        let probe = broker
            .probe_models("probe-dead", Duration::from_millis(800))
            .unwrap();
        assert!(!probe.ok);
        assert_eq!(
            probe.status, 0,
            "nothing answered, so no HTTP code is invented"
        );
        assert!(probe.error.is_some());
        assert!(probe.body.is_empty());
    }

    #[test]
    fn probe_models_refuses_a_cleartext_remote_endpoint() {
        let vault = vault();
        let _ = KeyRing::new(vault)
            .add_key(spec("probe-http", "k", "sk-http"))
            .unwrap();
        let broker = Broker::new(vault).with_endpoint(
            "probe-http",
            ProviderEndpoint::openai("http://api.example.com/v1"),
        );
        let err = broker
            .probe_models("probe-http", Duration::from_secs(5))
            .unwrap_err();
        assert!(matches!(err, BrokerError::InsecureEndpoint(_)), "{err:?}");
    }

    /// A keyless provider is probed with no credential at all — and it must
    /// work with zero keys in the vault, which is the local-runtime case.
    #[test]
    fn probe_models_sends_no_auth_header_for_a_keyless_endpoint() {
        let server = mock_server(|req| {
            assert!(
                !req.to_lowercase().contains("authorization"),
                "a keyless probe must send no credential: {req}"
            );
            assert!(!req.to_lowercase().contains("x-api-key"));
            (200, r#"{"data":[{"id":"local-1"}]}"#.to_string())
        });
        let vault = vault();
        let mut ep = ProviderEndpoint::openai(server);
        ep.keyless = true;
        let broker = Broker::new(vault).with_endpoint("probe-keyless", ep);
        let probe = broker
            .probe_models("probe-keyless", Duration::from_secs(5))
            .unwrap();
        assert!(probe.ok, "{probe:?}");
        assert!(probe.body.contains("local-1"));
    }

    /// Anthropic's dialect authenticates with `x-api-key`, exactly as the
    /// chat path does — the probe must not invent a second convention.
    #[test]
    fn probe_models_uses_the_anthropic_header_convention() {
        let server = mock_server(|req| {
            assert!(
                req.to_lowercase().contains("x-api-key: sk-ant"),
                "anthropic keys ride x-api-key: {req}"
            );
            assert!(!req.to_lowercase().contains("authorization"));
            (200, r#"{"models":{"claude-a":{}}}"#.to_string())
        });
        let vault = vault();
        let _ = KeyRing::new(vault)
            .add_key(spec("anthropic", "k", "sk-ant"))
            .unwrap();
        let mut ep = ProviderEndpoint::anthropic(server);
        ep.headers = vec![("anthropic-version".into(), "2023-06-01".into())];
        let broker = Broker::new(vault).with_endpoint("anthropic", ep);
        let probe = broker
            .probe_models("anthropic", Duration::from_secs(5))
            .unwrap();
        assert!(probe.ok, "{probe:?}");
        assert!(probe.body.contains("claude-a"));
    }

    /// Find the first byte offset of `needle` in `haystack`.
    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Spin a fake OpenAI-compatible endpoint. `respond` receives the raw
    /// request (headers + body) and returns `(status, body)`.
    fn mock_server(respond: impl Fn(&str) -> (u16, String) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut s = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Read the full request (headers + body) instead of a single
                // read(), which can return before the body arrives and make
                // body assertions flaky under load.
                let mut buf = Vec::new();
                let mut tmp = [0u8; 16_384];
                let header_end = loop {
                    let n = match s.read(&mut tmp) {
                        Ok(0) | Err(_) => {
                            // Connection closed mid-request — nothing to serve.
                            buf.clear();
                            break None;
                        }
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                        break Some(pos + 4);
                    }
                    if buf.len() > 1_048_576 {
                        break None; // oversized / malformed — give up
                    }
                };
                let Some(header_end) = header_end else {
                    continue;
                };
                // Parse Content-Length and drain the remaining body bytes so
                // the handler sees the complete payload.
                let headers = String::from_utf8_lossy(&buf[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|l| {
                        let l = l.trim_end();
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while buf.len() < header_end + content_length {
                    let n = match s.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                }
                let req = String::from_utf8_lossy(&buf).to_string();
                let (code, body) = respond(&req);
                let reason = if code == 429 {
                    "Too Many Requests"
                } else {
                    "OK"
                };
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

    #[test]
    fn injects_bearer_auth_and_succeeds() {
        let base = mock_server(|req| {
            assert!(
                req.contains("Authorization: Bearer sk-test-123"),
                "auth header missing: {req}"
            );
            assert!(req.contains("/chat/completions"));
            (200, r#"{"id":"x","usage":{"total_tokens":12}}"#.into())
        });
        let vault = vault();
        let broker = Broker::new(vault)
            .with_base_url("nvidia", base)
            .with_policy(RoutingPolicy::Priority);
        broker
            .ring()
            .add_key(spec("nvidia", "nim", "sk-test-123"))
            .unwrap();

        let resp = broker
            .chat_completion(
                "nvidia",
                "meta/llama",
                "s1",
                serde_json::json!({"messages": []}),
            )
            .unwrap();
        assert_eq!(usage_tokens(&resp), 12);
        // Health + usage recorded on the ring.
        let info = broker.ring().list("nvidia").unwrap();
        assert_eq!(info[0].success_count, 1);
        assert_eq!(info[0].fail_count, 0);
        assert!(info[0].tokens_day >= 12);
    }

    #[test]
    fn anthropic_prompt_cache_prefixing() {
        let base = mock_server(|req| {
            assert!(
                req.contains("cache_control"),
                "cache_control marker missing: {req}"
            );
            assert!(req.contains("ephemeral"), "{req}");
            (200, r#"{"usage":{"total_tokens":3}}"#.into())
        });
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("anthropic", base);
        broker
            .ring()
            .add_key(spec("anthropic", "a1", "sk-ant"))
            .unwrap();
        broker
            .chat_completion(
                "anthropic",
                "claude",
                "s1",
                serde_json::json!({
                    "system": "you are helpful",
                    "messages": [{"role": "user", "content": "hi"}]
                }),
            )
            .unwrap();
    }

    #[test]
    fn openai_prompt_cache_not_annotated() {
        // OpenAI-compatible providers cache the ≥1024-token prefix
        // automatically — no cache_control marker must be injected.
        let base = mock_server(|req| {
            assert!(
                !req.contains("cache_control"),
                "must not annotate OpenAI: {req}"
            );
            (200, r#"{"usage":{"total_tokens":3}}"#.into())
        });
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        broker.ring().add_key(spec("nvidia", "k", "sk")).unwrap();
        broker
            .chat_completion(
                "nvidia",
                "m",
                "s1",
                serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
            )
            .unwrap();
    }

    #[test]
    fn anthropic_uses_x_api_key_header() {
        let base = mock_server(|req| {
            assert!(req.contains("x-api-key: sk-ant-secret"), "{req}");
            assert!(!req.contains("Authorization: Bearer"), "{req}");
            (200, "{}".into())
        });
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("anthropic", base);
        broker
            .ring()
            .add_key(spec("anthropic", "a1", "sk-ant-secret"))
            .unwrap();
        broker
            .chat_completion("anthropic", "claude-3-5", "s1", serde_json::json!({}))
            .unwrap();
    }

    // ---- P55.5 / P56.6 / P56.8 -------------------------------------------------

    /// P56.6 — a keyless endpoint (OpenCode Free, local proxies) sends no
    /// credential at all, needs no ring row, and still lands in the ledger at
    /// $0. Before this, the broker required a key before any HTTP attempt.
    #[test]
    fn keyless_endpoint_sends_no_auth_and_still_records_usage() {
        let base = mock_server(|req| {
            assert!(
                !req.to_ascii_lowercase().contains("authorization"),
                "keyless request must not carry auth: {req}"
            );
            assert!(req.contains("POST /chat/completions"), "{req}");
            (
                200,
                r#"{"choices":[{"message":{"content":"ok"}}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#
                    .into(),
            )
        });
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v).with_endpoint(
            "opencode-free",
            ProviderEndpoint {
                base_url: base,
                keyless: true,
                session_headers: true,
                ..Default::default()
            },
        );
        // Deliberately no `add_key` anywhere.
        let resp = broker
            .chat_completion(
                "opencode-free",
                "big-pickle",
                "sess-free",
                serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
            )
            .unwrap();
        let u = Usage::from_any(&resp["usage"]).unwrap();
        assert_eq!(u.total(), 7);
        assert!(broker.ring().list("opencode-free").unwrap().is_empty());
        assert_eq!(broker.session_spent("sess-free"), 0.0);
        // The turn is still auditable: one ledger row, $0, real token counts.
        let rows = v.recent_usage(10).unwrap();
        let row = rows
            .iter()
            .find(|r| r.provider == "opencode-free")
            .expect("keyless turn must land in the durable ledger");
        assert_eq!(row.in_tokens, 5);
        assert_eq!(row.out_tokens, 2);
        assert_eq!(row.cost, 0.0);
    }

    /// P56.6 — the per-conversation OpenCode headers. A missing session is a
    /// 400 upstream, so this is a correctness requirement, not a nicety.
    #[test]
    fn opencode_session_headers_are_injected_per_conversation() {
        let base = mock_server(|req| {
            assert!(req.contains("x-opencode-session: sess-42"), "{req}");
            assert!(req.contains("X-Session-Id: sess-42"), "{req}");
            assert!(req.contains("x-opencode-client: cli"), "{req}");
            assert!(req.contains("x-opencode-request: req-"), "{req}");
            assert!(req.contains("User-Agent: AgentCowork/"), "{req}");
            (200, r#"{"usage":{"total_tokens":1}}"#.into())
        });
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v)
            .with_endpoint(
                "opencode",
                ProviderEndpoint {
                    base_url: base,
                    session_headers: true,
                    ..Default::default()
                },
            )
            .with_policy(RoutingPolicy::Priority);
        broker
            .ring()
            .add_key(spec("opencode", "zen", "sk-zen"))
            .unwrap();
        broker
            .chat_completion(
                "opencode",
                "claude-opus-5",
                "sess-42",
                serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
            )
            .unwrap();
    }

    // ---- INV-05 / REQ-PROV-008 — egress floor + custody at the choke point --

    /// Every credential-bearing path is floor-checked before a socket opens, and
    /// a denial is typed with no fallback route (INV-05 · `REQ-PROV-008`).
    #[test]
    fn egress_is_floor_checked_before_a_credential_is_attached() {
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v);
        // A public https destination is permitted under the platform default.
        assert!(
            broker
                .egress_preflight("openai", "https://api.openai.com/v1/chat/completions")
                .is_ok()
        );
        // Cleartext to a remote host is refused on custody grounds (custody is
        // checked first — the credential must never reach the wire in cleartext).
        assert!(matches!(
            broker.egress_preflight("openai", "http://api.openai.com/v1/chat/completions"),
            Err(BrokerError::InsecureEndpoint(_))
        ));
        // Link-local (which includes cloud metadata) is refused by the floor
        // even over https, and the refusal names the destination.
        let denied = broker
            .egress_preflight("openai", "https://169.254.169.254/v1/chat/completions")
            .expect_err("link-local is always refused");
        match denied {
            BrokerError::EgressDenied { host, reason } => {
                assert!(host.contains("169.254.169.254"), "{host}");
                assert!(!reason.is_empty());
            }
            other => panic!("expected a typed egress denial, got {other:?}"),
        }
    }

    /// A local runtime is permitted only when the endpoint says so, and a
    /// private/LAN destination stays refused unless it is declared.
    #[test]
    fn a_local_runtime_is_reachable_only_through_a_declared_endpoint() {
        let v = Vault::open_in_memory("test-key").unwrap();
        // A provider with no endpoint falls back to the platform default:
        // loopback permitted, private refused.
        let bare = Broker::new(&v);
        assert!(bare.floor_policy("local").allow_loopback);
        assert!(!bare.floor_policy("local").allow_private);
        // An endpoint that opted out of loopback loses it.
        let strict = bare.with_endpoint(
            "remote-only",
            ProviderEndpoint {
                base_url: "https://api.example.test/v1".into(),
                allow_loopback: false,
                ..Default::default()
            },
        );
        assert!(!strict.floor_policy("remote-only").allow_loopback);
        assert!(
            strict
                .egress_preflight("remote-only", "http://127.0.0.1:1234/v1/chat/completions")
                .is_err(),
            "a provider that declares no loopback reach must not reach it"
        );
        // Private/LAN is refused unless declared per endpoint.
        let lan = Broker::new(&v).with_endpoint(
            "lan",
            ProviderEndpoint {
                base_url: "https://api.example.test/v1".into(),
                ..Default::default()
            },
        );
        assert!(
            lan.egress_preflight("lan", "https://10.0.0.5/v1/chat/completions")
                .is_err()
        );
        let lan_opt_in = Broker::new(&v).with_endpoint(
            "lan-ok",
            ProviderEndpoint {
                base_url: "https://api.example.test/v1".into(),
                ..Default::default()
            }
            .with_private_network(),
        );
        assert!(
            lan_opt_in
                .egress_preflight("lan-ok", "https://10.0.0.5/v1/chat/completions")
                .is_ok(),
            "an explicit per-endpoint opt-in is honoured"
        );
    }

    /// A denied egress never becomes a request: the floor runs before the key
    /// ring is touched, so nothing is spent and no header is built.
    #[test]
    fn a_denied_egress_makes_no_request_and_spends_nothing() {
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v).with_endpoint(
            "blocked",
            ProviderEndpoint {
                base_url: "https://169.254.169.254/v1".into(),
                ..Default::default()
            },
        );
        broker
            .ring()
            .add_key(spec("blocked", "k1", "sk-blocked"))
            .unwrap();
        let err = broker
            .chat_completion(
                "blocked",
                "m",
                "s1",
                serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
            )
            .expect_err("the floor refuses before any socket");
        assert!(matches!(err, BrokerError::EgressDenied { .. }));
        // No ledger row, no spend: the call never happened.
        assert_eq!(broker.session_spent("s1"), 0.0);
        assert!(v.recent_usage(10).unwrap().is_empty());
    }

    /// The metadata probe is the other credential-bearing path, so it carries
    /// the same checks.
    #[test]
    fn the_metadata_probe_is_floor_checked_too() {
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v).with_endpoint(
            "blocked",
            ProviderEndpoint {
                base_url: "https://169.254.169.254/v1".into(),
                ..Default::default()
            },
        );
        broker
            .ring()
            .add_key(spec("blocked", "k1", "sk-blocked"))
            .unwrap();
        assert!(matches!(
            broker.probe_models("blocked", std::time::Duration::from_millis(50)),
            Err(BrokerError::EgressDenied { .. })
        ));
    }

    /// REQ-PROV-009 — the identity is ours and cannot be spoofed: an endpoint
    /// that tries to override the User-Agent is overwritten by the broker's own
    /// identity, and the session value is stable across the conversation.
    #[test]
    fn client_identity_cannot_be_spoofed_and_affinity_is_stable() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let base = mock_server(move |req| {
            sink.lock().unwrap().push(req.to_string());
            (200, r#"{"usage":{"total_tokens":1}}"#.into())
        });
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v).with_endpoint(
            "opencode",
            ProviderEndpoint {
                base_url: base,
                session_headers: true,
                // A hostile/naive endpoint trying to impersonate a different
                // client, and to pin its own session.
                headers: vec![
                    ("User-Agent".into(), "SomeOtherClient/9.9".into()),
                    ("x-opencode-session".into(), "forged-session".into()),
                ],
                ..Default::default()
            },
        );
        broker
            .ring()
            .add_key(spec("opencode", "zen", "sk-zen"))
            .unwrap();
        for _ in 0..2 {
            broker
                .chat_completion(
                    "opencode",
                    "some-model",
                    "conv-7",
                    serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
                )
                .unwrap();
        }
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        for req in &requests {
            assert!(
                req.contains("User-Agent: AgentCowork/"),
                "the identity is ours, never an impersonation: {req}"
            );
            assert!(!req.contains("SomeOtherClient"), "{req}");
            assert!(req.contains("x-opencode-session: conv-7"), "{req}");
            assert!(
                !req.contains("forged-session"),
                "a reserved header may not be pre-set by an endpoint: {req}"
            );
            // Exactly one value per identity header — a second value would be a
            // split session or an ambiguous identity.
            assert_eq!(
                req.matches("x-opencode-session:").count(),
                1,
                "the affinity header must appear exactly once: {req}"
            );
            assert_eq!(req.matches("User-Agent:").count(), 1, "{req}");
        }
        // Affinity is one value per conversation; the per-request id differs.
        assert!(requests[0].contains("x-opencode-request: req-"));
        assert_ne!(
            requests[0]
                .split("x-opencode-request: ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next(),
            requests[1]
                .split("x-opencode-request: ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next(),
            "the request id is fresh per request while the session id is not"
        );
    }

    /// The declared identity helper is the single source for the header set, so
    /// a host can assert what goes on the wire.
    #[test]
    fn gateway_identity_headers_are_built_in_one_place() {
        let h = gateway_identity_headers("conv-9", "req-abc");
        let names: Vec<&str> = h.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            names,
            vec![
                "X-Session-Id",
                "x-opencode-session",
                "x-opencode-request",
                "x-opencode-client",
                "User-Agent"
            ]
        );
        let value = |name: &str| {
            h.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(value("X-Session-Id"), "conv-9");
        assert_eq!(value("x-opencode-session"), "conv-9");
        assert_eq!(value("x-opencode-request"), "req-abc");
        assert!(value("User-Agent").starts_with("AgentCowork/"));
        // An empty conversation id still yields a real value, never an empty
        // header (a missing session is a hard error upstream).
        assert_eq!(
            value_of(&gateway_identity_headers("", "r"), "x-opencode-session"),
            "agentcowork-anon"
        );
        assert!(new_request_id().starts_with("req-"));
        assert_ne!(new_request_id(), new_request_id());
    }

    fn value_of(h: &[(&'static str, String)], name: &str) -> String {
        h.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// P55.5 — an Anthropic endpoint posts to `/messages` (never
    /// `/chat/completions`), carries `x-api-key` + `anthropic-version`, and
    /// the OpenAI-shaped body is translated with a required `max_tokens`.
    #[test]
    fn anthropic_transport_posts_to_messages_and_translates_the_body() {
        let base = mock_server(|req| {
            assert!(req.contains("POST /messages"), "wrong path: {req}");
            assert!(!req.contains("/chat/completions"), "{req}");
            assert!(req.contains("x-api-key: sk-ant-secret"), "{req}");
            assert!(req.contains("anthropic-version: 2023-06-01"), "{req}");
            assert!(req.contains("\"max_tokens\":4096"), "{req}");
            assert!(req.contains("\"system\":\"be brief\""), "{req}");
            (
                200,
                r#"{"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":9,"output_tokens":4,"cache_read_input_tokens":3}}"#
                    .into(),
            )
        });
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v).with_endpoint("anthropic", ProviderEndpoint::anthropic(base));
        broker
            .ring()
            .add_key(spec("anthropic", "a1", "sk-ant-secret"))
            .unwrap();
        let resp = broker
            .chat_completion(
                "anthropic",
                "claude-fable-5",
                "s1",
                serde_json::json!({
                    "messages": [
                        {"role": "system", "content": "be brief"},
                        {"role": "user", "content": "hi"}
                    ]
                }),
            )
            .unwrap();
        // Translated back to the OpenAI shape the relay consumes, with the
        // cache-aware usage fields `Usage::from_any` understands.
        assert_eq!(
            resp["choices"][0]["message"]["content"].as_str(),
            Some("hello")
        );
        let u = Usage::from_any(&resp["usage"]).unwrap();
        assert_eq!(u.total(), 13);
        assert_eq!(u.cache_read, 3);
    }

    /// P55.5 — the same endpoint streams with the Anthropic SSE grammar
    /// (`content_block_delta` / `message_delta`), not the OpenAI one.
    #[test]
    fn anthropic_transport_parses_anthropic_sse() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":11,\"cache_creation_input_tokens\":2}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"He\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"llo\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        )
        .to_string();
        let base = mock_server(move |_req| (200, sse.clone()));
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v).with_endpoint("anthropic", ProviderEndpoint::anthropic(base));
        broker
            .ring()
            .add_key(spec("anthropic", "a1", "sk-ant"))
            .unwrap();
        let events = broker
            .chat_completion_stream(
                "anthropic",
                "claude-fable-5",
                "s1",
                serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
            )
            .unwrap();
        let text: String = events.iter().filter_map(|e| e.delta.clone()).collect();
        assert_eq!(text, "Hello");
        assert_eq!(
            events.iter().find_map(|e| e.finish.clone()).as_deref(),
            Some("stop")
        );
        let usage = usage_from_stream(&events);
        assert_eq!(usage.prompt, 11);
        assert_eq!(usage.output, 3);
        assert_eq!(usage.cache_write, 2);
    }

    /// P56.8 — a 401/403 is a credential problem, not a provider hiccup: the
    /// key is suspended (so it stops being selected) and the next key is tried.
    #[test]
    fn auth_failure_suspends_the_key_and_fails_over() {
        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let c = std::sync::Arc::clone(&calls);
        let base = mock_server(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (401, r#"{"error":"bad key"}"#.into())
            } else {
                (200, r#"{"usage":{"total_tokens":3}}"#.into())
            }
        });
        let v = Vault::open_in_memory("test-key").unwrap();
        let broker = Broker::new(&v)
            .with_base_url("nvidia", base)
            .with_policy(RoutingPolicy::Priority);
        broker
            .ring()
            .add_key(spec("nvidia", "bad", "sk-bad"))
            .unwrap();
        broker
            .ring()
            .add_key(spec("nvidia", "good", "sk-good"))
            .unwrap();

        let resp = broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();
        assert_eq!(usage_tokens(&resp), 3);
        let rows = broker.ring().list("nvidia").unwrap();
        assert_eq!(
            rows.iter().filter(|r| r.status == "suspended").count(),
            1,
            "exactly the refused key must be suspended: {rows:?}"
        );
    }

    #[test]
    fn fail_closed_without_keys() {
        let vault = vault();
        let broker = Broker::new(vault);
        let err = broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, BrokerError::KeyRing(KeyRingError::NoKeys(_))));
    }

    #[test]
    fn fail_closed_on_unknown_provider() {
        let vault = vault();
        let broker = Broker::new(vault);
        let err = broker
            .chat_completion("mystery", "m", "s1", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, BrokerError::UnknownProvider(_)));
    }

    #[test]
    fn simulate_429_fails_over_to_next_key() {
        let call = AtomicU32::new(0);
        let base = mock_server(move |_| {
            let n = call.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (429, "rate limited".into())
            } else {
                (200, r#"{"usage":{"total_tokens":5}}"#.into())
            }
        });
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        broker.ring().add_key(spec("nvidia", "k1", "sk-1")).unwrap();
        broker.ring().add_key(spec("nvidia", "k2", "sk-2")).unwrap();

        let resp = broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();
        assert_eq!(usage_tokens(&resp), 5);

        // k1 went into cooldown on the 429; k2 served the request.
        let info = broker.ring().list("nvidia").unwrap();
        let k1 = info.iter().find(|i| i.key_id == "k1").unwrap();
        let k2 = info.iter().find(|i| i.key_id == "k2").unwrap();
        assert!(k1.in_cooldown);
        assert!(k1.fail_count >= 1);
        assert_eq!(k2.success_count, 1);
    }

    #[test]
    fn all_keys_exhausted_after_429_switches() {
        let base = mock_server(|_| (429, "nope".into()));
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        for i in 0..5 {
            broker
                .ring()
                .add_key(spec("nvidia", &format!("k{i}"), "sk"))
                .unwrap();
        }
        let err = broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap_err();
        assert!(
            matches!(err, BrokerError::AllKeysExhausted(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn non_429_error_surfaces_immediately() {
        let base = mock_server(|_| (500, "boom".into()));
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        broker.ring().add_key(spec("nvidia", "k1", "sk")).unwrap();
        let err = broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, BrokerError::Http(500, _)));
        // Health: failure counted, but no cooldown (not a 429).
        let info = broker.ring().list("nvidia").unwrap();
        assert_eq!(info[0].fail_count, 1);
        assert!(!info[0].in_cooldown);
    }

    #[test]
    fn parses_sse_stream() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
            "data: [DONE]\n",
        );
        let events = parse_sse_with(BufReader::new(sse.as_bytes()), &mut |_| {});
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].delta.as_deref(), Some("Hel"));
        assert_eq!(events[1].delta.as_deref(), Some("lo"));
        assert_eq!(events[2].finish.as_deref(), Some("stop"));
        let text: String = events.iter().filter_map(|e| e.delta.clone()).collect();
        assert_eq!(text, "Hello");
    }

    #[test]
    fn parses_sse_native_tool_calls() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search.query\",\"arguments\":\"{\\\"q\\\"\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"hi\\\"}\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]\n",
        );
        let events = parse_sse_with(BufReader::new(sse.as_bytes()), &mut |_| {});
        let calls = assemble_tool_calls(&events, false);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "search.query");
        assert_eq!(calls[0].1["q"], "hi");
        assert_eq!(
            events.last().and_then(|e| e.finish.as_deref()),
            Some("tool_calls")
        );
    }

    #[test]
    fn truncated_tool_call_dropped_on_length_finish() {
        // finish_reason="length" with a tool call whose args are cut mid-JSON.
        // pi / spec-B1 guard: never execute a truncated tool call, never hand
        // the `_raw` garbage. On a clean finish the same args stay a `_raw`
        // rescue (last resort, not silently executed as parsed args).
        let fragment = ChatStreamEvent {
            finish: Some("length".to_string()),
            tool_calls: vec![ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("file_ops.write".to_string()),
                arguments: Some("{\"path\":\"/tmp/f".to_string()), // cut mid-JSON
            }],
            ..Default::default()
        };
        assert!(
            assemble_tool_calls(std::slice::from_ref(&fragment), true).is_empty(),
            "length-truncated call must be dropped (fail-closed)"
        );
        // Same unparsable args on a clean finish stay a `_raw` rescue.
        let calls = assemble_tool_calls(&[fragment], false);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.get("_raw").is_some());
    }

    #[test]
    fn empty_args_yield_empty_object() {
        // A legitimate no-arg tool call (`arguments: ""`) → `{}`, both on a
        // clean finish and regardless of the `_raw`/truncation split.
        let ev = ChatStreamEvent {
            tool_calls: vec![ToolCallDelta {
                index: 0,
                id: Some("c".into()),
                name: Some("get.time".into()),
                arguments: Some(String::new()),
            }],
            ..Default::default()
        };
        let calls = assemble_tool_calls(&[ev], true);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, serde_json::json!({}));
    }

    #[test]
    fn extract_json_tool_calls_b5_shape() {
        let calls = extract_json_tool_calls("{\"tool\":\"weather\",\"args\":{\"city\":\"Paris\"}}");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "weather");
        assert_eq!(calls[0].1["city"], "Paris");
        assert!(extract_json_tool_calls("{\"city\":\"Paris\"}").is_empty());
    }

    #[test]
    fn streaming_roundtrip_collects_deltas_and_usage() {
        // include_usage echo: final chunk carries `usage` (OpenAI-compatible
        // streaming) — the broker must record it against the key's budget.
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi \"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"there\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[],\"usage\":{\"total_tokens\":37}}\n",
            "data: [DONE]\n",
        );
        let base = mock_server(move |_| (200, sse.into()));
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        broker.ring().add_key(spec("nvidia", "k", "sk")).unwrap();
        let events = broker
            .chat_completion_stream("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();
        let text: String = events.iter().filter_map(|e| e.delta.clone()).collect();
        assert_eq!(text, "hi there");
        // Usage from the final chunk hit the key's daily budget.
        let info = broker.ring().list("nvidia").unwrap();
        assert!(info[0].tokens_day >= 37);
    }

    // ---- P1.3: cache-aware costs (A9) + session budget (J11) -----------

    #[test]
    fn cache_aware_usage_lands_in_ledger_and_key_budget() {
        // OpenAI-compatible response with cached input: cost must be computed
        // on BILLABLE input (prompt − cached), and the ledger row + per-key
        // cost_day must reflect the real $.
        let base = mock_server(|_| {
            (
                200,
                r#"{"usage":{"prompt_tokens":100,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":80}}}"#
                    .into(),
            )
        });
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        broker.ring().add_key(spec("nvidia", "nim", "sk")).unwrap();
        broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();

        // Ledger row exists with cache_read recorded.
        assert_eq!(vault.ledger_count().unwrap(), 1);
        // nvidia pricing: in/out $0.50 per 1M. billable = 100−80 = 20 →
        // cost = 20×0.5e-6 + 50×0.5e-6 = 35e-6.
        let spent = vault.session_spend("s1").unwrap();
        let expected = 35e-6;
        assert!(
            (spent - expected).abs() < 1e-12,
            "spent {spent} != {expected}"
        );
        // Per-key budget carries the same cost.
        let info = broker.ring().list("nvidia").unwrap();
        assert!((info[0].cost_day - expected).abs() < 1e-12);
        assert!(info[0].tokens_day >= 150);
        // Broker-side tracker agrees.
        assert!((broker.session_spent("s1") - expected).abs() < 1e-12);
    }

    #[test]
    fn anthropic_cache_tokens_priced_at_cache_rates() {
        let base = mock_server(|_| {
            (
                200,
                r#"{"usage":{"input_tokens":200,"output_tokens":30,"cache_creation_input_tokens":150,"cache_read_input_tokens":40}}"#
                    .into(),
            )
        });
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("anthropic", base);
        broker
            .ring()
            .add_key(spec("anthropic", "a1", "sk-ant"))
            .unwrap();
        broker
            .chat_completion("anthropic", "claude", "s1", serde_json::json!({}))
            .unwrap();
        // Cost = billable(160)×3e-6 + 30×15e-6 + 40×0.3e-6 + 150×3.75e-6.
        let expected = 160.0 * 3e-6 + 30.0 * 15e-6 + 40.0 * 0.3e-6 + 150.0 * 3.75e-6;
        let spent = vault.session_spend("s1").unwrap();
        assert!(
            (spent - expected).abs() < 1e-9,
            "spent {spent} != {expected}"
        );
    }

    #[test]
    fn session_budget_kills_session_and_surfaces_stopped_message() {
        // J11: a $0.000000001 budget — first call succeeds (spent 35e-6 >
        // limit), the NEXT call is refused at the pre-flight choke point.
        let base = mock_server(|_| {
            (
                200,
                r#"{"usage":{"prompt_tokens":100,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":80}}}"#
                    .into(),
            )
        });
        let vault = vault();
        let broker = Broker::new(vault)
            .with_base_url("nvidia", base)
            .with_session_budget_limit(1e-9);
        broker.ring().add_key(spec("nvidia", "nim", "sk")).unwrap();

        broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();
        let err = broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap_err();
        let msg = err.to_string();
        match err {
            BrokerError::SessionBudgetExceeded {
                session,
                limit,
                spent,
            } => {
                assert_eq!(session, "s1");
                assert!((limit - 1e-9).abs() < 1e-18);
                assert!(spent > limit);
                // The UI surface string: "stopped: $X limit ...".
                assert!(msg.contains("stopped:"), "msg: {msg}");
                assert!(msg.contains("limit"), "msg: {msg}");
            }
            other => panic!("expected SessionBudgetExceeded, got {other:?}"),
        }
        // The session is now dead — remaining is $0.
        assert_eq!(broker.session_budget_remaining("s1"), 0.0);
        // Other sessions are unaffected.
        assert!(broker.session_budget_remaining("s2") > 0.0);
    }

    #[test]
    fn streaming_usage_merges_anthropic_shapes() {
        // Anthropic streaming: input/cache-write in message_start, output in
        // message_delta. The broker must merge them into ONE Usage row.
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100,\"cache_creation_input_tokens\":60}}}\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":25}}\n",
            "data: [DONE]\n",
        );
        let base = mock_server(move |_| (200, sse.into()));
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("anthropic", base);
        broker
            .ring()
            .add_key(spec("anthropic", "a1", "sk-ant"))
            .unwrap();
        let events = broker
            .chat_completion_stream("anthropic", "claude", "s1", serde_json::json!({}))
            .unwrap();
        // Content delta came through.
        assert!(events.iter().any(|e| e.delta.as_deref() == Some("hi")));
        // Ledger merged input+cache_write+output.
        assert_eq!(vault.ledger_count().unwrap(), 1);
        let spend = vault.session_spend("s1").unwrap();
        let expected = 100.0 * 3e-6 + 25.0 * 15e-6 + 60.0 * 3.75e-6;
        assert!(
            (spend - expected).abs() < 1e-9,
            "spend {spend} != {expected}"
        );
    }

    #[test]
    fn streaming_callback_delivers_each_event_as_it_is_parsed() {
        // P9.5/A8: the incremental variant hands every parsed event to the
        // caller (the A8 server forwards these as SSE frames) in order, and
        // still returns the same buffered set the blocking variant returns —
        // the usage accountant and the failover loop depend on that return.
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"b\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
            "data: [DONE]\n",
        );
        let base = mock_server(move |_| (200, sse.into()));
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        broker.ring().add_key(spec("nvidia", "nim", "sk")).unwrap();

        let mut seen: Vec<String> = Vec::new();
        let events = broker
            .chat_completion_stream_cb(
                "nvidia",
                "m",
                "s1",
                serde_json::json!({}),
                &mut |e: &ChatStreamEvent| {
                    if let Some(d) = e.delta.as_deref() {
                        seen.push(d.to_string());
                    }
                },
            )
            .unwrap();
        // One callback per parsed event, in wire order.
        assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].delta.as_deref(), Some("a"));
        assert_eq!(events[2].finish.as_deref(), Some("stop"));
    }

    #[test]
    fn anthropic_sse_parser_keys_off_the_payload_type() {
        // The dialect is decided by the JSON `type` field, not the `event:`
        // line — the payload is the contract (P55.5). `tool_use` normalizes to
        // the OpenAI `tool_calls` finish reason so downstream sees one
        // vocabulary regardless of provider.
        let sse = concat!(
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"x\"}}\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n",
        );
        let events = parse_sse_anthropic_with(std::io::BufReader::new(sse.as_bytes()), &mut |_| {});
        assert_eq!(events[0].delta.as_deref(), Some("x"));
        assert_eq!(events[1].finish.as_deref(), Some("tool_calls"));
        assert_eq!(events[1].usage.map(|u| u.output), Some(3));
    }

    #[test]
    fn custom_pricing_override_applies() {
        let base = mock_server(|_| {
            (
                200,
                r#"{"usage":{"prompt_tokens":1000,"completion_tokens":0}}"#.into(),
            )
        });
        let vault = vault();
        // Override: input is FREE — cost must be 0 despite 1000 tokens.
        let broker = Broker::new(vault)
            .with_base_url("nvidia", base)
            .with_pricing(
                "nvidia",
                crate::ledger::Pricing {
                    input_per_m: 0.0,
                    output_per_m: 0.0,
                    cache_read_per_m: 0.0,
                    cache_write_per_m: 0.0,
                },
            );
        broker.ring().add_key(spec("nvidia", "nim", "sk")).unwrap();
        broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();
        assert_eq!(vault.session_spend("s1").unwrap(), 0.0);
        // Tokens still land on the key budget.
        let info = broker.ring().list("nvidia").unwrap();
        assert!(info[0].tokens_day >= 1000);
    }

    #[test]
    fn sealed_channel_never_leaks_secret() {
        // End-to-end sealed-channel check: after a full broker round trip the
        // ONLY credential artifact observable is the opaque handle — the raw
        // secret must not appear in any public surface (list / KeyInfo JSON).
        let base = mock_server(|_| (200, r#"{"usage":{"total_tokens":1}}"#.into()));
        let vault = vault();
        let broker = Broker::new(vault).with_base_url("nvidia", base);
        let handle = broker
            .ring()
            .add_key(spec("nvidia", "k", "sk-super-secret"))
            .unwrap();
        broker
            .chat_completion("nvidia", "m", "s1", serde_json::json!({}))
            .unwrap();
        let info = broker.ring().list("nvidia").unwrap();
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(&handle));
        assert!(!json.contains("sk-super-secret"));
        assert!(!json.to_lowercase().contains("\"value\""));
    }
}
