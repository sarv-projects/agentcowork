//! Remote MCP client — OAuth 2.1 + streamable HTTP (ARCH/15 Tier 2).
//!
//! The Connect Store lists remote MCP servers (`StoreKind::RemoteMcp`) but the
//! crate had no way to *talk to* them. This module is the client half:
//!
//! - **Dual era** (DEC-030, ARCH/14 §4): the modern revision `2026-07-28` is
//!   first-class and stateless — the protocol version travels in a header and
//!   request context rides in `_meta` — while the legacy revision `2025-11-25`
//!   stays a supported fallback behind `initialize`. HTTP distinguishes the eras
//!   by classifying the `400` body, the verdict is cached per process/origin,
//!   and every server has an explicit **force-legacy** escape hatch.
//! - **Discovery** per the MCP authorization spec (2026-07-28):
//!   `GET {server}/.well-known/oauth-protected-resource` → resource +
//!   authorization_servers; `GET {auth}/.well-known/oauth-authorization-server`
//!   → endpoints.
//! - **Dynamic client registration** (RFC 7591) when the server offers a
//!   `registration_endpoint` — the local app registers itself on the fly
//!   (public client, PKCE, loopback redirect), so **no pre-registered client
//!   ID is needed** — the ChatGPT "click → sign in → use" path for OSS.
//! - **PKCE (S256)** auth-code flow with a local loopback redirect.
//! - **Streamable HTTP** transport: POST JSON-RPC with
//!   `Accept: application/json, text/event-stream`, parse SSE `data:` frames.
//!
//! Tokens are returned to the caller (the shell stores them in the vault's
//! key ring under `remote-mcp:<server-id>`). Everything here is a pure
//! client over a tiny `HttpTransport` seam so tests use a mock.
//!
//! ## Why the call is sent once
//!
//! Era detection is read-only by construction: it probes `server/discover`,
//! then at most one `initialize`, and never the caller's own method. A failing
//! `tools/call` is therefore **never** re-sent under the other era, because a
//! mutating call that already ran must not run twice (INV-07, ARCH/14 §7).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::protocol::{
    DISCOVER_METHOD, LEGACY_PROTOCOL_REVISION, METHOD_HEADER, MODERN_PROTOCOL_REVISION,
    NAME_HEADER, PROTOCOL_VERSION_HEADER,
};

/// The modern revision this client speaks first.
pub const MODERN_PROTOCOL_VERSION: &str = MODERN_PROTOCOL_REVISION;

/// The legacy fallback revision.
pub const LEGACY_PROTOCOL_VERSION: &str = LEGACY_PROTOCOL_REVISION;

/// The budget one era-detection probe may spend (DEC-030 / ARCH/14 §4: a 10 s
/// cap), on **both** transports.
///
/// This is a probe-scoped deadline, never an operation budget: a slow server
/// must not be able to make a `tools/call` unbounded, and detection must not
/// inherit the caller's timeout. One definition, two call sites — the HTTP
/// probe ([`probe_era`]) and the stdio probe (`attach::AttachedServer::attach`)
/// — so the two transports cannot drift on the cap.
pub const PROBE_BUDGET: Duration = Duration::from_secs(10);

/// The era-cache namespace prefix for a stdio child process.
///
/// DEC-030 says the era is cached "per process/origin".  An *origin* is
/// `scheme://authority`, which is meaningless for a spawned child, so stdio
/// verdicts live under this prefix keyed by the launch fingerprint
/// ([`stdio_era_key`]).  **Spec gap:** neither `ARCH/14` §4 nor DEC-030 names
/// the stdio cache key; this is the conservative reading (one verdict per
/// command line, re-probed when the command is edited) and it is recorded as
/// decision-needed.
pub const STDIO_ERA_KEY_PREFIX: &str = "stdio:";

#[cfg(test)]
use std::collections::HashMap;

/// Well-known protected-resource metadata (`.well-known/oauth-protected-resource`).
/// Wire JSON is snake_case per the MCP authorization spec — no rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedResource {
    #[serde(default)]
    pub resource: String,
    #[serde(default)]
    pub authorization_servers: Vec<String>,
}

/// Authorization-server metadata (`.well-known/oauth-authorization-server`).
/// Wire JSON is snake_case per RFC 8414 — no rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthServerMetadata {
    pub issuer: String,
    #[serde(default)]
    pub authorization_endpoint: String,
    #[serde(default)]
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: String,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub response_types_supported: Vec<String>,
}

/// Result of RFC 7591 dynamic client registration.
/// Wire JSON is snake_case per RFC 7591 — no rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRegistration {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub token_endpoint_auth_method: String,
}

/// A validated remote server target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTarget {
    /// The MCP server URL (the OAuth 2.1 resource server).
    pub url: String,
    /// Authorization server base (authorize + token endpoints).
    pub auth: AuthServerMetadata,
    /// Registered client (dynamic or supplied).
    pub client: ClientRegistration,
    /// Per-server **force-legacy** escape hatch (DEC-030).
    ///
    /// When set, this server is spoken to exclusively in the legacy revision:
    /// detection is skipped entirely and no modern header or `_meta` is sent.
    /// It is the operator's answer to a server that misreports its era.
    pub force_legacy: bool,
}

impl RemoteTarget {
    /// Arm the per-server force-legacy escape hatch for this target.
    pub fn with_force_legacy(mut self, force_legacy: bool) -> Self {
        self.force_legacy = force_legacy;
        self
    }
}

/// Which wire revision a remote server is spoken to (DEC-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEra {
    /// `2026-07-28` — stateless: protocol-version header plus `_meta`.
    Modern,
    /// `2025-11-25` — `initialize` handshake, no modern headers.
    Legacy,
}

impl McpEra {
    /// The revision string this era negotiates.
    pub fn version(self) -> &'static str {
        match self {
            Self::Modern => MODERN_PROTOCOL_VERSION,
            Self::Legacy => LEGACY_PROTOCOL_VERSION,
        }
    }

    /// Does this era carry the modern stateless contract?
    pub fn is_modern(self) -> bool {
        matches!(self, Self::Modern)
    }
}

/// What one probe reply proved about the server's era.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraVerdict {
    /// The server answered the modern contract.
    Modern,
    /// The server refused the modern contract: it is legacy.
    Legacy,
    /// The reply proved neither era (transport loss, auth refusal, 5xx, an
    /// unclassifiable body). The caller retries the other era exactly once and
    /// otherwise keeps the modern default (ARCH/14 §7).
    Inconclusive,
}

/// Where the era now in force came from (observability for ARCH/14 §7's
/// "provider marked incompatible with reason").
///
/// Today a user who hits a misreporting server has no way to see *why* the
/// wrong era was chosen, so the verdict travels with its provenance.  It is
/// read-only: nothing in this crate branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EraSource {
    /// The per-server force-legacy hatch armed; detection was skipped.
    Forced,
    /// A conclusive verdict already cached for this key in this process.
    Cached,
    /// This call's `server/discover` probe concluded the era.
    Probed,
    /// Detection proved nothing and the modern default was kept (DEC-030
    /// precedence: the fallback is modern, never a cached guess).
    Default,
}

impl EraSource {
    /// The stable wire spelling a UI can switch on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forced => "forced",
            Self::Cached => "cached",
            Self::Probed => "probed",
            Self::Default => "default",
        }
    }
}

/// One negotiated era plus the reason it was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraNegotiation {
    pub era: McpEra,
    pub source: EraSource,
}

impl EraNegotiation {
    /// The revision string in force.
    pub fn version(&self) -> &'static str {
        self.era.version()
    }

    /// The read-only wire shape a status surface can render.
    ///
    /// `era` is the **revision string** (not an enum name) because that is
    /// what a client compares against, and `source` is the stable spelling
    /// from [`EraSource::as_str`]. Nothing in this crate reads it back, so it
    /// cannot become a second decision path.
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "era": self.era.version(),
            "eraSource": self.source.as_str(),
        })
    }
}

/// Body markers that only a modern server produces.  A refusal that names the
/// modern revision or its headers is a modern server complaining about
/// something else — never evidence that it is legacy.
///
/// The probed method name is deliberately **not** a marker: a legacy server
/// answers `Method not found: server/discover`, and reading its own echo as a
/// modern signal would pin the origin to the wrong era forever.
const MODERN_MARKERS: &[&str] = &[
    "2026-07-28",
    "mcp-protocol-version",
    "mcp-method",
    "mcp-name",
];

/// Body markers that only a legacy server produces: it does not know the
/// modern method, or it names the legacy revision.
const LEGACY_MARKERS: &[&str] = &["-32601", "method not found", "unknown method", "2025-11-25"];

/// Body markers that mean the probed method does not exist on this server.
const UNKNOWN_METHOD_MARKERS: &[&str] = &["-32601", "method not found", "unknown method"];

/// Status codes that mean "this server refused the request contract", as
/// opposed to an authorization or availability problem that says nothing about
/// the era.
fn status_refuses_modern_contract(status: u16) -> bool {
    matches!(status, 400 | 404 | 405 | 406 | 415 | 422)
}

/// Classify one `server/discover` reply into an era (ARCH/14 §4: HTTP
/// distinguishes the eras from the refusal body).
pub fn classify_era(status: u16, body: &serde_json::Value) -> EraVerdict {
    // A successful answer to a modern method settles it: a 2xx JSON-RPC
    // result can only come from a server that understood the request.
    if is_json_rpc_success(status, body) {
        return EraVerdict::Modern;
    }
    let text = body.to_string().to_ascii_lowercase();
    if MODERN_MARKERS.iter().any(|marker| text.contains(marker)) {
        return EraVerdict::Modern;
    }
    if LEGACY_MARKERS.iter().any(|marker| text.contains(marker)) {
        return EraVerdict::Legacy;
    }
    if status_refuses_modern_contract(status) {
        return EraVerdict::Legacy;
    }
    EraVerdict::Inconclusive
}

/// Classify a `server/discover` reply that has **no HTTP status** — a stdio
/// reply.
///
/// Stdio has no status line, so the modern/legacy decision rests entirely on
/// the JSON-RPC body.  This shim reuses [`classify_era`] with a synthetic
/// `200` so the marker tables cannot drift between the two transports: a
/// `-32601` to `server/discover` is `Legacy` through the same `LEGACY_MARKERS`
/// that classify the HTTP refusal, and a success is `Modern` through the same
/// JSON-RPC success test.  A body that proves neither is `Inconclusive` and is
/// never cached.
pub fn classify_era_body(body: &serde_json::Value) -> EraVerdict {
    classify_era(200, body)
}

/// The one `server/discover` request an era probe sends, on either transport.
///
/// Both probes call this, so the modern `_meta` envelope they carry cannot
/// drift apart: the stateless context rides in `params._meta`
/// ([`build_request`]) and the header set is [`modern_headers`].
pub fn build_discover_request(era: McpEra) -> serde_json::Value {
    build_request(DISCOVER_METHOD, serde_json::json!({}), era, None)
}

/// The header set an era probe sends on the modern revision.
pub fn discover_probe_headers() -> Vec<(String, String)> {
    modern_headers(DISCOVER_METHOD, None)
}

/// Classify the single legacy `initialize` retry.
///
/// This probe only runs when the modern one proved nothing, so it reads the
/// reply for exactly one question: does this server still have `initialize`?
/// A server that answers it is speaking the legacy contract; a server that
/// says the method does not exist is stateless-modern.  Everything else stays
/// inconclusive so no guess is cached.
fn classify_initialize_probe(status: u16, body: &serde_json::Value) -> EraVerdict {
    if is_json_rpc_success(status, body) {
        return EraVerdict::Legacy;
    }
    let text = body.to_string().to_ascii_lowercase();
    if MODERN_MARKERS.iter().any(|marker| text.contains(marker))
        || UNKNOWN_METHOD_MARKERS
            .iter()
            .any(|marker| text.contains(marker))
    {
        return EraVerdict::Modern;
    }
    EraVerdict::Inconclusive
}

fn is_json_rpc_success(status: u16, body: &serde_json::Value) -> bool {
    (200..300).contains(&status) && body.get("error").is_none() && body.get("result").is_some()
}

/// The per-process, per-origin era cache (DEC-030: "era is cached per
/// process/origin").
#[derive(Debug, Default)]
pub struct EraCache {
    by_origin: BTreeMap<String, McpEra>,
}

impl EraCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached era for one origin, if detection already concluded.
    pub fn get(&self, origin: &str) -> Option<McpEra> {
        self.by_origin.get(origin).copied()
    }

    /// Record a conclusive verdict for one origin and return the era now in
    /// force for it.
    pub fn insert(&mut self, origin: &str, era: McpEra) -> McpEra {
        self.by_origin.insert(origin.to_string(), era);
        era
    }

    /// Number of origins with a cached era.
    pub fn len(&self) -> usize {
        self.by_origin.len()
    }

    /// Is no origin cached?
    pub fn is_empty(&self) -> bool {
        self.by_origin.is_empty()
    }

    /// Forget every cached era (a provider restart, a credential change, or a
    /// test that must not inherit a verdict).
    pub fn clear(&mut self) {
        self.by_origin.clear();
    }
}

fn process_era_cache() -> &'static Mutex<EraCache> {
    static CACHE: OnceLock<Mutex<EraCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(EraCache::new()))
}

/// The era cached for `origin` in this process.
pub fn cached_era(origin: &str) -> Option<McpEra> {
    process_era_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(origin))
}

/// Record the conclusive era verdict for `origin` in this process.
pub fn cache_era(origin: &str, era: McpEra) {
    if let Ok(mut cache) = process_era_cache().lock() {
        cache.insert(origin, era);
    }
}

/// Drop every cached era in this process.
pub fn clear_era_cache() {
    if let Ok(mut cache) = process_era_cache().lock() {
        cache.clear();
    }
}

/// Serialize the tests that assert on process-global era state.
///
/// The cache is deliberately process-wide (DEC-030: "cached per
/// process/origin"), so a test that reads it cannot also assert *absence*
/// while a sibling test is populating it. Rust runs unit tests on parallel
/// threads, so that is a real race, not a theoretical one. Holding this guard
/// makes the cache observable to exactly one era test at a time; a poisoned
/// mutex is recovered rather than propagated so one failing test cannot cascade
/// into every other era test.
#[cfg(test)]
pub(crate) fn era_cache_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The cache key for a server URL: `scheme://authority`.
///
/// Two paths on one host share a verdict; a different host never does, so one
/// misdetected server cannot drag another down.
pub fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    format!("{scheme}://{authority}")
}

/// The era-cache key for a stdio child: a digest of its **command
/// fingerprint** (executable plus every argument, in order).
///
/// Two different commands are two different verdicts, and re-probing happens
/// for free when a user edits a command line.  The fingerprint is hashed
/// rather than concatenated because the key is process-global state: the raw
/// argv would retain argument bytes (a token, a path) in a long-lived map.
/// It is deliberately *not* the resolved absolute path — the same command
/// resolved through two `PATH`s is one server, and `resolve_stdio_launch`
/// already owns resolution.
pub fn stdio_era_key(command: &str, args: &[&str]) -> String {
    let mut hasher = Sha256::new();
    // Length-prefix each component so `("a", ["b"])` and `("ab", [])` can
    // never collide.
    for part in std::iter::once(command).chain(args.iter().copied()) {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    format!(
        "{STDIO_ERA_KEY_PREFIX}{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// An in-flight PKCE flow — the shell keeps `state`/`verifier` and opens
/// `auth_url` in the system browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceFlow {
    pub auth_url: String,
    pub state: String,
    pub code_verifier: String,
    pub redirect_uri: String,
}

/// OAuth token response (the fields a public client needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

/// One HTTP reply to a JSON-RPC message: the exact status plus the body.
///
/// A JSON-RPC error is carried in a `4xx` body, so era classification needs
/// the status, not just a parsed value.
#[derive(Debug, Clone, PartialEq)]
pub struct McpResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

/// The HTTP seam — the real `UreqTransport` talks to the wire; tests use a
/// mock. Mirrors the vault's `post_form`/`get_json` pattern but with a trait
/// so the remote-client logic is unit-testable without a socket.
pub trait HttpTransport: Send {
    fn get_json(&self, url: &str) -> Result<serde_json::Value, RemoteError>;
    fn post_form(&self, url: &str, form: &[(&str, &str)])
    -> Result<serde_json::Value, RemoteError>;
    fn post_json(
        &self,
        url: &str,
        bearer: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError>;

    /// POST one JSON-RPC message with per-request headers and read the exact
    /// HTTP status.
    ///
    /// This is the dual-era path: the modern headers are mandatory there, and
    /// era classification needs to see a `400` body rather than a collapsed
    /// transport error.  The default keeps every existing transport compiling
    /// by dropping the headers and reporting `200`; a transport that talks to a
    /// real server **must** override it, because a request that silently loses
    /// `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` can never satisfy the
    /// modern contract it claims to be speaking.
    fn post_json_rpc(
        &self,
        url: &str,
        bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<McpResponse, RemoteError> {
        let _ = headers;
        Ok(McpResponse {
            status: 200,
            body: self.post_json(url, bearer, body)?,
        })
    }

    /// The same POST under an explicit **deadline** — the seam that makes
    /// DEC-030's 10 s probe cap real on the wire.
    ///
    /// The default delegates to [`HttpTransport::post_json_rpc`] and therefore
    /// does **not** enforce the budget.  That is deliberate: a test double must
    /// not have to model a clock, and a transport that talks to a real server
    /// **must** override it.  [`UreqTransport`] does.  A production transport
    /// that inherits the default can stall a probe forever, so the doc contract
    /// is the same one `post_json_rpc` already carries for the modern headers.
    fn post_json_rpc_within(
        &self,
        url: &str,
        bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
        budget: Duration,
    ) -> Result<McpResponse, RemoteError> {
        let _ = budget;
        self.post_json_rpc(url, bearer, headers, body)
    }
}

/// Default transport using `ureq` (same client as the vault).
#[derive(Debug, Clone, Default)]
pub struct UreqTransport;

impl HttpTransport for UreqTransport {
    fn get_json(&self, url: &str) -> Result<serde_json::Value, RemoteError> {
        ureq::get(url)
            .set("Accept", "application/json")
            .call()
            .map_err(|e| RemoteError::Transport(e.to_string()))?
            .into_json()
            .map_err(|e| RemoteError::Transport(e.to_string()))
    }

    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<serde_json::Value, RemoteError> {
        ureq::post(url)
            .set("Accept", "application/json")
            .send_form(form)
            .map_err(|e| RemoteError::Transport(e.to_string()))?
            .into_json()
            .map_err(|e| RemoteError::Transport(e.to_string()))
    }

    fn post_json(
        &self,
        url: &str,
        bearer: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        let mut req = ureq::post(url)
            .set("Accept", "application/json, text/event-stream")
            .set("Content-Type", "application/json");
        if let Some(b) = bearer {
            req = req.set("Authorization", &format!("Bearer {b}"));
        }
        req.send_json(body)
            .map_err(|e| RemoteError::Transport(e.to_string()))?
            .into_json()
            .map_err(|e| RemoteError::Transport(e.to_string()))
    }

    fn post_json_rpc(
        &self,
        url: &str,
        bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<McpResponse, RemoteError> {
        self.post_json_rpc_optional_budget(url, bearer, headers, body, None)
    }

    fn post_json_rpc_within(
        &self,
        url: &str,
        bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
        budget: Duration,
    ) -> Result<McpResponse, RemoteError> {
        self.post_json_rpc_optional_budget(url, bearer, headers, body, Some(budget))
    }
}

impl UreqTransport {
    /// One JSON-RPC POST, optionally bounded by an overall deadline.
    ///
    /// `ureq`'s per-request `timeout` covers connect **and** read, so the
    /// budget is a real bound on a hung server rather than a connect-only one.
    /// A body that is not JSON (an SSE frame, an empty reply) is still
    /// returned verbatim as `Null` so the caller classifies what arrived.
    fn post_json_rpc_optional_budget(
        &self,
        url: &str,
        bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
        budget: Option<Duration>,
    ) -> Result<McpResponse, RemoteError> {
        let mut req = ureq::post(url)
            .set("Accept", "application/json, text/event-stream")
            .set("Content-Type", "application/json");
        if let Some(budget) = budget {
            req = req.timeout(budget);
        }
        for (name, value) in headers {
            req = req.set(name, value);
        }
        if let Some(b) = bearer {
            req = req.set("Authorization", &format!("Bearer {b}"));
        }
        match req.send_json(body) {
            Ok(response) => Ok(McpResponse {
                status: response.status(),
                // A streamable-HTTP reply may legitimately be an SSE frame or
                // an empty body; the caller classifies what arrived.
                body: response.into_json().unwrap_or(serde_json::Value::Null),
            }),
            // A JSON-RPC error arrives as a 4xx *with* a body. Report the
            // refusal instead of collapsing it into a transport failure, or
            // era classification can never see it.
            Err(ureq::Error::Status(status, response)) => Ok(McpResponse {
                status,
                body: response.into_json().unwrap_or(serde_json::Value::Null),
            }),
            // A budget overrun is a transport failure: no reply, no verdict, so
            // the probe stays inconclusive and nothing is cached.
            Err(error) => Err(RemoteError::Transport(error.to_string())),
        }
    }
}

/// Fetch `.well-known/oauth-protected-resource` from a server URL.
pub fn discover_protected_resource(
    server_url: &str,
    http: &dyn HttpTransport,
) -> Result<ProtectedResource, RemoteError> {
    let base = server_url.trim_end_matches('/');
    let wk = format!("{base}/.well-known/oauth-protected-resource");
    let json = http.get_json(&wk)?;
    serde_json::from_value(json).map_err(RemoteError::Json)
}

/// Fetch authorization-server metadata (with the protected-resource fallback:
/// if the resource's `authorization_servers` list is empty, try the server
/// origin itself).
pub fn discover_authorization_server(
    resource: &ProtectedResource,
    server_url: &str,
    http: &dyn HttpTransport,
) -> Result<AuthServerMetadata, RemoteError> {
    let mut candidates: Vec<String> = resource.authorization_servers.clone();
    if candidates.is_empty() {
        // Fallback: same origin, standard well-known path.
        let origin = server_url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("");
        candidates.push(format!("https://{origin}"));
    }
    let mut last_err = None;
    for base in candidates {
        let wk = format!(
            "{}/.well-known/oauth-authorization-server",
            base.trim_end_matches('/')
        );
        match http.get_json(&wk) {
            Ok(json) => match serde_json::from_value::<AuthServerMetadata>(json) {
                Ok(m) => return Ok(m),
                Err(e) => last_err = Some(RemoteError::Json(e)),
            },
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or(RemoteError::Msg(
        "no authorization-server metadata discovered".into(),
    )))
}

/// RFC 7591 dynamic client registration (public client, PKCE, loopback).
pub fn register_dynamic_client(
    registration_endpoint: &str,
    redirect_uri: &str,
    http: &dyn HttpTransport,
) -> Result<ClientRegistration, RemoteError> {
    let body = serde_json::json!({
        "client_name": "AgentCowork",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "scope": ""
    });
    // Dynamic registration POSTs JSON to the registration endpoint (not a form).
    let json = post_json_raw(http, registration_endpoint, &body)?;
    serde_json::from_value(json).map_err(RemoteError::Json)
}

/// The persisted, operator-set options that shape how one server is spoken to.
///
/// Anything here changes the *wire contract*, not whether a call happens right
/// now, so it is a stored property of the server record rather than a
/// per-request argument (DEC-030's force-legacy hatch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectOptions {
    /// Arm the per-server force-legacy escape hatch (DEC-030).
    ///
    /// Persisted on the stored server entry and read by the shell; there is no
    /// user-facing toggle in v1.
    pub force_legacy: bool,
}

/// Full connect handshake: discover resource → discover auth server →
/// register client (or use a supplied one) → return the ready target.
///
/// [`connect_with_options`] is the same handshake with the persisted
/// force-legacy hatch applied.
pub fn connect(server_url: &str, http: &dyn HttpTransport) -> Result<RemoteTarget, RemoteError> {
    connect_with_options(server_url, http, ConnectOptions::default())
}

/// [`connect`], with the stored per-server options applied to the returned
/// target.
///
/// The OAuth handshake is unaffected by the era: the hatch changes which
/// revision later calls are spoken in, never how the target authenticates.
pub fn connect_with_options(
    server_url: &str,
    http: &dyn HttpTransport,
    options: ConnectOptions,
) -> Result<RemoteTarget, RemoteError> {
    if !(server_url.starts_with("https://")
        || server_url.starts_with("http://127.0.0.1")
        || server_url.starts_with("http://localhost"))
    {
        return Err(RemoteError::InsecureUrl(server_url.to_string()));
    }
    let resource = discover_protected_resource(server_url, http)?;
    let auth = discover_authorization_server(&resource, server_url, http)?;
    let redirect_uri = "http://127.0.0.1:0/oauth/callback".to_string();
    let client = if auth.registration_endpoint.is_empty() {
        // No dynamic registration — the caller must supply a client_id.
        return Err(RemoteError::NeedsPreRegisteredClient);
    } else {
        register_dynamic_client(&auth.registration_endpoint, &redirect_uri, http)?
    };
    Ok(RemoteTarget {
        url: server_url.to_string(),
        auth,
        client,
        force_legacy: options.force_legacy,
    })
}

/// Build the PKCE authorize URL + keep state/verifier (the shell stores
/// these while the browser is open, then calls [`exchange_code`]).
pub fn build_authorize_url(
    target: &RemoteTarget,
    redirect_uri: &str,
) -> Result<PkceFlow, RemoteError> {
    let verifier = random_url_b64(32);
    let challenge = code_challenge(&verifier);
    let state = random_hex(16);
    let mut query = String::new();
    push_q(&mut query, "client_id", &target.client.client_id);
    push_q(&mut query, "response_type", "code");
    push_q(&mut query, "redirect_uri", redirect_uri);
    push_q(&mut query, "code_challenge", &challenge);
    push_q(&mut query, "code_challenge_method", "S256");
    push_q(&mut query, "state", &state);
    // Ask for a refresh token so re-auth is silent.
    push_q(&mut query, "scope", "openid");
    let auth_url = format!("{}?{}", target.auth.authorization_endpoint, query);
    Ok(PkceFlow {
        auth_url,
        state,
        code_verifier: verifier,
        redirect_uri: redirect_uri.to_string(),
    })
}

/// Exchange the authorization code for tokens (PKCE).
pub fn exchange_code(
    target: &RemoteTarget,
    flow: &PkceFlow,
    code: &str,
    http: &dyn HttpTransport,
) -> Result<TokenResponse, RemoteError> {
    let form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", flow.redirect_uri.as_str()),
        ("client_id", target.client.client_id.as_str()),
        ("code_verifier", flow.code_verifier.as_str()),
    ];
    let json = http.post_form(&target.auth.token_endpoint, &form)?;
    parse_tokens(&json)
}

/// Refresh an access token.
pub fn refresh_token(
    target: &RemoteTarget,
    refresh: &str,
    http: &dyn HttpTransport,
) -> Result<TokenResponse, RemoteError> {
    let form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", target.client.client_id.as_str()),
    ];
    let json = http.post_form(&target.auth.token_endpoint, &form)?;
    parse_tokens(&json)
}

/// Process-monotonic JSON-RPC request id.
///
/// The revision is stateless, so the id is the only thing that distinguishes
/// two otherwise identical calls — a server that caches by id would otherwise
/// collapse a second `tools/call` into the first one's answer.
fn next_request_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The modern request headers for one JSON-RPC method.
///
/// `Mcp-Name` is only meaningful for `tools/call`; sending it for any other
/// method is a protocol violation the façade rejects.
pub fn modern_headers(method: &str, name: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            PROTOCOL_VERSION_HEADER.to_string(),
            MODERN_PROTOCOL_VERSION.to_string(),
        ),
        (METHOD_HEADER.to_string(), method.to_string()),
    ];
    if method == "tools/call"
        && let Some(name) = name
    {
        headers.push((NAME_HEADER.to_string(), name.to_string()));
    }
    headers
}

/// Build one JSON-RPC request body.
///
/// In the modern era the request is stateless, so the context a session used to
/// carry is passed in `params._meta`: the negotiated revision and, when the
/// caller has one, the idempotency key.  The legacy era keeps the pre-`_meta`
/// body — a legacy server that does not know `_meta` is not a server to probe.
pub fn build_request(
    method: &str,
    params: serde_json::Value,
    era: McpEra,
    idempotency_key: Option<&str>,
) -> serde_json::Value {
    // The legacy body is passed through untouched: a server that predates
    // `_meta` must see exactly the wire shape it always saw.
    let params = match era {
        McpEra::Legacy => params,
        McpEra::Modern => {
            let mut object = match params {
                serde_json::Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };
            let mut meta = serde_json::Map::new();
            meta.insert(
                "protocolVersion".to_string(),
                serde_json::Value::String(era.version().to_string()),
            );
            if let Some(key) = idempotency_key {
                meta.insert(
                    "idempotencyKey".to_string(),
                    serde_json::Value::String(key.to_string()),
                );
            }
            object.insert("_meta".to_string(), serde_json::Value::Object(meta));
            serde_json::Value::Object(object)
        }
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": next_request_id(),
        "method": method,
        "params": params,
    })
}

/// The `params.name` of a `tools/call`, if this request is one.
pub fn tool_name<'a>(method: &str, body: &'a serde_json::Value) -> Option<&'a str> {
    if method != "tools/call" {
        return None;
    }
    body.get("params")?
        .get("name")?
        .as_str()
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
}

/// A line-oriented child process that can be asked for its era.
///
/// The stdio transport is the *only* place the era cache is keyed by something
/// other than an origin ([`stdio_era_key`]), so the cache reads/writes are
/// factored here and the pipe itself stays in `attach`. One probe, one
/// classification, one cache write — the same rules the HTTP path uses.
pub trait StdioEraProbe {
    /// Send one JSON-RPC request and return the parsed reply.
    ///
    /// `budget` is the caller's deadline for this exchange. An implementation
    /// must return rather than block past it; the attach treats a returned
    /// `Err` as *inconclusive* and refuses to continue the handshake (a late
    /// reply would desynchronize the ND-JSON stream).
    fn send(
        &mut self,
        request: &serde_json::Value,
        budget: Duration,
    ) -> Result<serde_json::Value, RemoteError>;
}

/// Resolve the era of a stdio child: cache first, then exactly one
/// `server/discover` probe (DEC-030 / ARCH/14 §4).
///
/// Precedence matches the HTTP path minus the force-legacy hatch, which is an
/// operator property of the *stored remote* record (`store::StoreEntry`) and
/// has no stdio equivalent in v1 — recorded as decision-needed.
///
/// Only a conclusive verdict is cached, and a conclusive verdict is the only
/// thing that ends detection. An inconclusive probe returns
/// [`EraSource::Default`] so the caller can distinguish "the server said so"
/// from "the server said nothing" and decide whether to attempt the legacy
/// fallback.
pub fn negotiate_stdio_era(
    command: &str,
    args: &[&str],
    probe: &mut dyn StdioEraProbe,
) -> EraNegotiation {
    let key = stdio_era_key(command, args);
    if let Some(era) = cached_era(&key) {
        return EraNegotiation {
            era,
            source: EraSource::Cached,
        };
    }
    let request = build_discover_request(McpEra::Modern);
    let Ok(reply) = probe.send(&request, PROBE_BUDGET) else {
        return EraNegotiation {
            era: McpEra::Modern,
            source: EraSource::Default,
        };
    };
    match classify_era_body(&reply) {
        EraVerdict::Modern => {
            cache_era(&key, McpEra::Modern);
            EraNegotiation {
                era: McpEra::Modern,
                source: EraSource::Probed,
            }
        }
        EraVerdict::Legacy => {
            cache_era(&key, McpEra::Legacy);
            EraNegotiation {
                era: McpEra::Legacy,
                source: EraSource::Probed,
            }
        }
        EraVerdict::Inconclusive => EraNegotiation {
            era: McpEra::Modern,
            source: EraSource::Default,
        },
    }
}

/// Resolve which era to speak to `target` (DEC-030 precedence).
///
/// 1. the per-server **force-legacy** escape hatch — detection is skipped;
/// 2. the process cache for this origin;
/// 3. detection: probe `server/discover`, and when that is inconclusive retry
///    the other era exactly once with `initialize`.
///
/// Only a conclusive verdict is cached: an unreachable server must be re-probed
/// next time rather than pinned to a guess.
pub fn negotiate_era(
    target: &RemoteTarget,
    bearer: Option<&str>,
    http: &dyn HttpTransport,
) -> McpEra {
    negotiate_era_detailed(target, bearer, http).era
}

/// [`negotiate_era`], also reporting **why** that era was chosen.
///
/// The source is read-only observability (ARCH/14 §7's "provider marked
/// incompatible with reason"): no code path in this crate branches on it, and
/// it exists so a UI can explain a misreporting server instead of showing an
/// opaque era.
pub fn negotiate_era_detailed(
    target: &RemoteTarget,
    bearer: Option<&str>,
    http: &dyn HttpTransport,
) -> EraNegotiation {
    if target.force_legacy {
        return EraNegotiation {
            era: McpEra::Legacy,
            source: EraSource::Forced,
        };
    }
    let origin = origin_of(&target.url);
    if let Some(era) = cached_era(&origin) {
        return EraNegotiation {
            era,
            source: EraSource::Cached,
        };
    }
    match probe_era(target, bearer, http) {
        Some(era) => {
            cache_era(&origin, era);
            EraNegotiation {
                era,
                source: EraSource::Probed,
            }
        }
        None => EraNegotiation {
            era: McpEra::Modern,
            source: EraSource::Default,
        },
    }
}

/// Probe one origin for its era using read-only methods only.
///
/// The probe is deadline-bounded by [`PROBE_BUDGET`]: a server that accepts
/// the connection and then says nothing must not stall detection for the
/// caller's whole budget. An overrun is a transport failure, so it is
/// inconclusive and nothing is cached.
fn probe_era(
    target: &RemoteTarget,
    bearer: Option<&str>,
    http: &dyn HttpTransport,
) -> Option<McpEra> {
    let discover = build_discover_request(McpEra::Modern);
    let headers = discover_probe_headers();
    let borrowed = header_refs(&headers);
    match http.post_json_rpc_within(&target.url, bearer, &borrowed, &discover, PROBE_BUDGET) {
        Ok(response) => match classify_era(response.status, &response.body) {
            EraVerdict::Modern => Some(McpEra::Modern),
            EraVerdict::Legacy => Some(McpEra::Legacy),
            // The other era gets exactly one retry (ARCH/14 §7).
            EraVerdict::Inconclusive => probe_era_legacy(target, bearer, http),
        },
        Err(_) => probe_era_legacy(target, bearer, http),
    }
}

/// The single legacy retry: an `initialize` handshake for the fallback era.
fn probe_era_legacy(
    target: &RemoteTarget,
    bearer: Option<&str>,
    http: &dyn HttpTransport,
) -> Option<McpEra> {
    let params = serde_json::json!({
        "protocolVersion": LEGACY_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "agentcowork-mcp", "version": env!("CARGO_PKG_VERSION")},
    });
    let initialize = build_request("initialize", params, McpEra::Legacy, None);
    match http.post_json_rpc(&target.url, bearer, &[], &initialize) {
        Ok(response) => match classify_initialize_probe(response.status, &response.body) {
            EraVerdict::Modern => Some(McpEra::Modern),
            EraVerdict::Legacy => Some(McpEra::Legacy),
            EraVerdict::Inconclusive => None,
        },
        Err(_) => None,
    }
}

fn header_refs(headers: &[(String, String)]) -> Vec<(&str, &str)> {
    headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// One streamable-HTTP JSON-RPC call in a known era. Returns the parsed
/// JSON-RPC response body (the caller parses `result`/`error`).
pub fn rpc_in_era(
    target: &RemoteTarget,
    bearer: &str,
    era: McpEra,
    method: &str,
    params: serde_json::Value,
    idempotency_key: Option<&str>,
    http: &dyn HttpTransport,
) -> Result<serde_json::Value, RemoteError> {
    let body = build_request(method, params, era, idempotency_key);
    let headers = if era.is_modern() {
        modern_headers(method, tool_name(method, &body))
    } else {
        Vec::new()
    };
    let borrowed = header_refs(&headers);
    let response = http.post_json_rpc(&target.url, Some(bearer), &borrowed, &body)?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let message = response
        .body
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("HTTP {} with no JSON-RPC error message", response.status));
    Err(RemoteError::Rpc {
        status: response.status,
        method: method.to_string(),
        message,
    })
}

/// One streamable-HTTP JSON-RPC call (tools/list, tools/call, …) in the
/// server's own era.
///
/// Modern first (DEC-030): the era is detected once per origin and cached, the
/// force-legacy hatch outranks everything, and the call is sent exactly once —
/// a refused call is not replayed under the other era, because a mutating
/// `tools/call` that may already have run must not run twice.
pub fn rpc(
    target: &RemoteTarget,
    bearer: &str,
    method: &str,
    params: serde_json::Value,
    http: &dyn HttpTransport,
) -> Result<serde_json::Value, RemoteError> {
    let era = negotiate_era(target, Some(bearer), http);
    rpc_in_era(target, bearer, era, method, params, None, http)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn post_json_raw(
    http: &dyn HttpTransport,
    url: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, RemoteError> {
    // Dynamic registration uses the same transport but with no bearer and a
    // plain JSON Accept — reuse post_json with None.
    http.post_json(url, None, body)
}

fn push_q(out: &mut String, k: &str, v: &str) {
    if !out.is_empty() {
        out.push('&');
    }
    out.push_str(k);
    out.push('=');
    out.push_str(&pct_encode(v));
}

fn random_url_b64(bytes: usize) -> String {
    use base64::Engine as _;
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&buf)
}

fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn code_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest;
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn pct_encode(s: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        if UNRESERVED.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn parse_tokens(json: &serde_json::Value) -> Result<TokenResponse, RemoteError> {
    let access = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RemoteError::Msg("missing access_token in token response".into()))?;
    Ok(TokenResponse {
        access_token: access.to_string(),
        refresh_token: json
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        token_type: json
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        expires_in: json
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600),
        scope: json
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Msg(String),
    #[error("insecure remote URL `{0}` — must be https or loopback")]
    InsecureUrl(String),
    #[error("server has no registration endpoint — supply a pre-registered client_id")]
    NeedsPreRegisteredClient,
    /// The server answered the call with a non-success status.  The status and
    /// the JSON-RPC error message are both preserved so the caller can tell a
    /// refusal (retry elsewhere) from an era mismatch.
    #[error("MCP `{method}` refused with HTTP {status}: {message}")]
    Rpc {
        status: u16,
        method: String,
        message: String,
    },
}

/// One JSON-RPC request as the scripted era transport saw it.
#[cfg(test)]
#[derive(Debug, Clone)]
struct SeenRequest {
    method: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
    bearer: Option<String>,
}

#[cfg(test)]
impl SeenRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A JSON-RPC transport scripted per method: it records every outgoing
/// request (method, headers, body) and answers with the status/body the test
/// chose for that method.  The era tests need to see the wire, which a
/// URL-keyed route table cannot express.
#[cfg(test)]
struct ScriptedHttp {
    replies: HashMap<String, (u16, serde_json::Value)>,
    seen: std::sync::Mutex<Vec<SeenRequest>>,
}

#[cfg(test)]
impl ScriptedHttp {
    fn new(replies: impl IntoIterator<Item = (&'static str, u16, serde_json::Value)>) -> Self {
        Self {
            replies: replies
                .into_iter()
                .map(|(method, status, body)| (method.to_string(), (status, body)))
                .collect(),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<SeenRequest> {
        self.seen.lock().expect("era transport lock").clone()
    }

    fn methods(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.method).collect()
    }
}

#[cfg(test)]
impl HttpTransport for ScriptedHttp {
    fn get_json(&self, _url: &str) -> Result<serde_json::Value, RemoteError> {
        Err(RemoteError::Msg(
            "scripted transport has no GET route".into(),
        ))
    }

    fn post_form(
        &self,
        _url: &str,
        _form: &[(&str, &str)],
    ) -> Result<serde_json::Value, RemoteError> {
        Err(RemoteError::Msg(
            "scripted transport has no form route".into(),
        ))
    }

    fn post_json(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _body: &serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        Err(RemoteError::Msg(
            "scripted transport answers through post_json_rpc".into(),
        ))
    }

    fn post_json_rpc(
        &self,
        _url: &str,
        bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<McpResponse, RemoteError> {
        let method = body
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.seen
            .lock()
            .expect("era transport lock")
            .push(SeenRequest {
                method: method.clone(),
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                body: body.clone(),
                bearer: bearer.map(str::to_string),
            });
        let (status, reply) = self
            .replies
            .get(&method)
            .cloned()
            .ok_or_else(|| RemoteError::Msg(format!("no scripted reply for `{method}`")))?;
        Ok(McpResponse {
            status,
            body: reply,
        })
    }
}

/// A connected target for one test origin (no OAuth round trip).
#[cfg(test)]
fn target_for(url: &str) -> RemoteTarget {
    RemoteTarget {
        url: url.to_string(),
        auth: AuthServerMetadata {
            issuer: "https://auth.example.com".into(),
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            registration_endpoint: String::new(),
            scopes_supported: vec![],
            response_types_supported: vec![],
        },
        client: ClientRegistration {
            client_id: "fixture-client".into(),
            client_secret: String::new(),
            token_endpoint_auth_method: "none".into(),
        },
        force_legacy: false,
    }
}

#[cfg(test)]
fn modern_discover_reply() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"protocolVersion": MODERN_PROTOCOL_VERSION, "tools": []}
    })
}

#[cfg(test)]
fn legacy_unknown_method_reply() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {"code": -32601, "message": "Method not found: server/discover"}
    })
}

#[cfg(test)]
fn legacy_initialize_reply() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"protocolVersion": LEGACY_PROTOCOL_VERSION, "capabilities": {}}
    })
}

/// A convenient test mock that serves canned JSON.
#[cfg(test)]
pub struct MockHttp {
    pub routes: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
impl MockHttp {
    pub fn new(routes: impl IntoIterator<Item = (String, serde_json::Value)>) -> Self {
        Self {
            routes: routes.into_iter().collect(),
        }
    }
    fn route(&self, url: &str) -> Result<serde_json::Value, RemoteError> {
        self.routes
            .get(url)
            .cloned()
            .ok_or_else(|| RemoteError::Msg(format!("no mock for {url}")))
    }
}

#[cfg(test)]
impl HttpTransport for MockHttp {
    fn get_json(&self, url: &str) -> Result<serde_json::Value, RemoteError> {
        self.route(url)
    }
    fn post_form(
        &self,
        url: &str,
        _form: &[(&str, &str)],
    ) -> Result<serde_json::Value, RemoteError> {
        self.route(url)
    }
    fn post_json(
        &self,
        url: &str,
        _bearer: Option<&str>,
        _body: &serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        self.route(url)
    }
}

/// A transport that records the budget each POST was given, so the probe cap
/// can be asserted on the seam rather than inferred from a library default.
#[cfg(test)]
struct BudgetRecordingHttp {
    seen: std::sync::Mutex<Vec<(String, Option<Duration>)>>,
}

#[cfg(test)]
impl BudgetRecordingHttp {
    fn record(
        &self,
        body: &serde_json::Value,
        budget: Option<Duration>,
    ) -> Result<McpResponse, RemoteError> {
        let method = body
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.seen
            .lock()
            .expect("budget lock")
            .push((method.clone(), budget));
        Ok(McpResponse {
            status: 200,
            body: match method.as_str() {
                DISCOVER_METHOD => modern_discover_reply(),
                _ => serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {}}),
            },
        })
    }
}

#[cfg(test)]
impl HttpTransport for BudgetRecordingHttp {
    fn get_json(&self, _url: &str) -> Result<serde_json::Value, RemoteError> {
        Err(RemoteError::Msg("no GET route".into()))
    }

    fn post_form(
        &self,
        _url: &str,
        _form: &[(&str, &str)],
    ) -> Result<serde_json::Value, RemoteError> {
        Err(RemoteError::Msg("no form route".into()))
    }

    fn post_json(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _body: &serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        Err(RemoteError::Msg("no plain route".into()))
    }

    fn post_json_rpc(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<McpResponse, RemoteError> {
        self.record(body, None)
    }

    fn post_json_rpc_within(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _headers: &[(&str, &str)],
        body: &serde_json::Value,
        budget: Duration,
    ) -> Result<McpResponse, RemoteError> {
        self.record(body, Some(budget))
    }
}

/// A scripted stdio peer: one canned reply, then the request log.
#[cfg(test)]
struct ScriptedStdio {
    reply: Option<serde_json::Value>,
    sent: Vec<serde_json::Value>,
    budgets: Vec<Duration>,
}

#[cfg(test)]
impl ScriptedStdio {
    fn new(reply: serde_json::Value) -> Self {
        Self {
            reply: Some(reply),
            sent: Vec::new(),
            budgets: Vec::new(),
        }
    }

    /// A peer that never answers — the hung-server case.
    fn failing() -> Self {
        Self {
            reply: None,
            sent: Vec::new(),
            budgets: Vec::new(),
        }
    }

    fn methods(&self) -> Vec<String> {
        self.sent
            .iter()
            .filter_map(|body| body.get("method").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
impl StdioEraProbe for ScriptedStdio {
    fn send(
        &mut self,
        request: &serde_json::Value,
        budget: Duration,
    ) -> Result<serde_json::Value, RemoteError> {
        self.sent.push(request.clone());
        self.budgets.push(budget);
        self.reply
            .clone()
            .ok_or_else(|| RemoteError::Msg("stdio peer did not answer".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> MockHttp {
        MockHttp::new([
            (
                "https://mcp.example.com/.well-known/oauth-protected-resource".into(),
                serde_json::json!({
                    "resource": "https://mcp.example.com",
                    "authorization_servers": ["https://auth.example.com"]
                }),
            ),
            (
                "https://auth.example.com/.well-known/oauth-authorization-server".into(),
                serde_json::json!({
                    "issuer": "https://auth.example.com",
                    "authorization_endpoint": "https://auth.example.com/authorize",
                    "token_endpoint": "https://auth.example.com/token",
                    "registration_endpoint": "https://auth.example.com/register",
                    "scopes_supported": ["openid"],
                    "response_types_supported": ["code"]
                }),
            ),
            (
                "https://auth.example.com/register".into(),
                serde_json::json!({
                    "client_id": "dyn-client-123",
                    "token_endpoint_auth_method": "none"
                }),
            ),
            (
                "https://auth.example.com/token".into(),
                serde_json::json!({
                    "access_token": "tok-remote-1",
                    "refresh_token": "rt-remote-1",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                    "scope": "openid"
                }),
            ),
            (
                "https://mcp.example.com".into(),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "tools": [] }
                }),
            ),
        ])
    }

    #[test]
    fn connect_discovers_and_registers_dynamic_client() {
        let http = server();
        let t = connect("https://mcp.example.com", &http).unwrap();
        assert_eq!(t.url, "https://mcp.example.com");
        assert_eq!(t.client.client_id, "dyn-client-123");
        assert_eq!(
            t.auth.authorization_endpoint,
            "https://auth.example.com/authorize"
        );
    }

    #[test]
    fn insecure_url_rejected() {
        let http = server();
        assert!(matches!(
            connect("http://evil.example.com/mcp", &http),
            Err(RemoteError::InsecureUrl(_))
        ));
    }

    #[test]
    fn authorize_url_has_pkce_and_state() {
        let http = server();
        let t = connect("https://mcp.example.com", &http).unwrap();
        let flow = build_authorize_url(&t, "http://127.0.0.1:0/oauth/callback").unwrap();
        assert!(
            flow.auth_url
                .starts_with("https://auth.example.com/authorize?")
        );
        assert!(flow.auth_url.contains("code_challenge="));
        assert!(flow.auth_url.contains("code_challenge_method=S256"));
        assert!(flow.auth_url.contains("state="));
        assert!(!flow.auth_url.contains("verifier"));
        // Verifier is stored client-side, never in the URL.
        assert!(!flow.code_verifier.is_empty());
    }

    #[test]
    fn exchange_code_parses_tokens() {
        let http = server();
        let t = connect("https://mcp.example.com", &http).unwrap();
        let flow = build_authorize_url(&t, "http://127.0.0.1:0/oauth/callback").unwrap();
        let tok = exchange_code(&t, &flow, "auth-code-1", &http).unwrap();
        assert_eq!(tok.access_token, "tok-remote-1");
        assert_eq!(tok.refresh_token.as_deref(), Some("rt-remote-1"));
    }

    #[test]
    fn rpc_posts_jsonrpc_and_returns_result() {
        let http = server();
        let t = connect("https://mcp.example.com", &http).unwrap();
        let resp = rpc(
            &t,
            "tok-remote-1",
            "tools/list",
            serde_json::json!({}),
            &http,
        )
        .unwrap();
        assert_eq!(resp["result"]["tools"], serde_json::json!([]));
    }

    // -- DEC-030 dual era ---------------------------------------------------

    #[test]
    fn modern_request_carries_the_protocol_headers_and_meta() {
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 200, modern_discover_reply()),
            (
                "tools/call",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"ok": true}}),
            ),
        ]);
        let target = target_for("https://modern.example.com/mcp");
        let resp = rpc(
            &target,
            "tok-1",
            "tools/call",
            serde_json::json!({"name": "remote.do_thing", "arguments": {"a": 1}}),
            &http,
        )
        .unwrap();
        assert_eq!(resp["result"]["ok"], true);

        let seen = http.seen();
        assert_eq!(
            seen.iter().map(|s| s.method.as_str()).collect::<Vec<_>>(),
            vec![DISCOVER_METHOD, "tools/call"],
            "modern era is probed with server/discover before the call"
        );
        let call = &seen[1];
        assert_eq!(call.bearer.as_deref(), Some("tok-1"));
        // Protocol version header + method + name must all be present.
        assert_eq!(
            call.header(PROTOCOL_VERSION_HEADER),
            Some(MODERN_PROTOCOL_VERSION)
        );
        assert_eq!(call.header(METHOD_HEADER), Some("tools/call"));
        assert_eq!(call.header(NAME_HEADER), Some("remote.do_thing"));
        // The stateless context rides in `_meta`.
        assert_eq!(
            call.body["params"]["_meta"]["protocolVersion"],
            serde_json::json!(MODERN_PROTOCOL_VERSION)
        );
        assert_eq!(call.body["jsonrpc"], "2.0");
    }

    #[test]
    fn modern_non_call_request_has_no_name_header() {
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 200, modern_discover_reply()),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}}),
            ),
        ]);
        rpc(
            &target_for("https://modern-list.example.com/mcp"),
            "tok-1",
            "tools/list",
            serde_json::json!({}),
            &http,
        )
        .unwrap();
        let seen = http.seen();
        let list = seen.last().expect("tools/list request");
        assert_eq!(list.header(METHOD_HEADER), Some("tools/list"));
        assert_eq!(list.header(NAME_HEADER), None);
    }

    #[test]
    fn idempotency_key_rides_in_meta_where_the_facade_reads_it() {
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 200, modern_discover_reply()),
            (
                "tools/call",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {}}),
            ),
        ]);
        rpc_in_era(
            &target_for("https://modern-idem.example.com/mcp"),
            "tok-1",
            McpEra::Modern,
            "tools/call",
            serde_json::json!({"name": "remote.write"}),
            Some("mutation-7"),
            &http,
        )
        .unwrap();
        let seen = http.seen();
        let call = seen.last().expect("tools/call request");
        // The façade reads `params._meta.idempotencyKey` (server.rs), so the key
        // must be in `_meta` and not in a header the remote may not know.
        assert_eq!(
            call.body["params"]["_meta"]["idempotencyKey"],
            serde_json::json!("mutation-7")
        );
        assert_eq!(call.header(NAME_HEADER), Some("remote.write"));
    }

    #[test]
    fn legacy_fallback_is_used_when_the_modern_method_is_unknown() {
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 400, legacy_unknown_method_reply()),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}}),
            ),
        ]);
        let target = target_for("https://legacy.example.com/mcp");
        let resp = rpc(&target, "tok-1", "tools/list", serde_json::json!({}), &http).unwrap();
        assert_eq!(resp["result"]["tools"], serde_json::json!([]));
        assert_eq!(cached_era(&origin_of(&target.url)), Some(McpEra::Legacy));
        let list = http.seen().pop().expect("tools/list request");
        assert!(
            list.headers.is_empty(),
            "a legacy server gets no modern headers"
        );
        assert!(list.body["params"].get("_meta").is_none());
    }

    #[test]
    fn detection_retries_the_other_era_exactly_once() {
        // `server/discover` proves nothing (503), so the client retries the
        // legacy handshake once and stops.
        let http = ScriptedHttp::new([
            (
                DISCOVER_METHOD,
                503,
                serde_json::json!({"error": "upstream unavailable"}),
            ),
            ("initialize", 200, legacy_initialize_reply()),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 3, "result": {"tools": []}}),
            ),
        ]);
        let target = target_for("https://retry.example.com/mcp");
        rpc(&target, "tok-1", "tools/list", serde_json::json!({}), &http).unwrap();
        assert_eq!(
            http.methods(),
            vec![DISCOVER_METHOD, "initialize", "tools/list"],
            "detection probes modern, retries legacy once, then sends the call"
        );
        assert_eq!(cached_era(&origin_of(&target.url)), Some(McpEra::Legacy));
    }

    #[test]
    fn a_stateless_server_that_refuses_initialize_is_classified_modern() {
        // The modern probe proved nothing (503), and the legacy retry says
        // there is no `initialize` — that is a stateless server, not a legacy
        // one, so the retry must not pin the origin to legacy.
        let http = ScriptedHttp::new([
            (
                DISCOVER_METHOD,
                503,
                serde_json::json!({"error": "upstream unavailable"}),
            ),
            (
                "initialize",
                200,
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": {"code": -32601, "message": "Method not found: initialize"}
                }),
            ),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}}),
            ),
        ]);
        let target = target_for("https://stateless.example.com/mcp");
        rpc(&target, "tok-1", "tools/list", serde_json::json!({}), &http).unwrap();
        assert_eq!(
            cached_era(&origin_of(&target.url)),
            Some(McpEra::Modern),
            "a server without `initialize` is modern, never legacy"
        );
        let list = http.seen().pop().expect("tools/list request");
        assert_eq!(list.header(METHOD_HEADER), Some("tools/list"));
    }

    #[test]
    fn an_inconclusive_probe_is_not_cached() {
        let _era = era_cache_test_guard();
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 503, serde_json::json!({"error": "down"})),
            ("initialize", 503, serde_json::json!({"error": "down"})),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}}),
            ),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 3, "result": {"tools": []}}),
            ),
        ]);
        let target = target_for("https://flaky.example.com/mcp");
        rpc(&target, "tok-1", "tools/list", serde_json::json!({}), &http).unwrap();
        assert_eq!(cached_era(&origin_of(&target.url)), None);
        // A second call re-probes instead of trusting the guess.
        rpc(&target, "tok-1", "tools/list", serde_json::json!({}), &http).unwrap();
        assert_eq!(
            http.methods()
                .iter()
                .filter(|m| *m == DISCOVER_METHOD)
                .count(),
            2
        );
    }

    #[test]
    fn force_legacy_skips_detection_entirely() {
        let _era = era_cache_test_guard();
        let http = ScriptedHttp::new([(
            "tools/call",
            200,
            serde_json::json!({"jsonrpc": "2.0", "id": 9, "result": {"ok": true}}),
        )]);
        let target = target_for("https://forced.example.com/mcp").with_force_legacy(true);
        let resp = rpc(
            &target,
            "tok-1",
            "tools/call",
            serde_json::json!({"name": "remote.write"}),
            &http,
        )
        .unwrap();
        assert_eq!(resp["result"]["ok"], true);
        assert_eq!(
            http.methods(),
            vec!["tools/call"],
            "the escape hatch must not spend a detection round trip"
        );
        let call = http.seen().pop().expect("the call");
        assert!(call.headers.is_empty());
        assert!(call.body["params"].get("_meta").is_none());
        assert!(
            cached_era(&origin_of(&target.url)).is_none(),
            "a force-legacy server must not poison the shared origin cache"
        );
    }

    #[test]
    fn era_is_cached_per_origin_not_globally() {
        let modern = ScriptedHttp::new([
            (DISCOVER_METHOD, 200, modern_discover_reply()),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}}),
            ),
        ]);
        let target = target_for("https://cache-a.example.com/mcp");
        rpc(&target, "tok", "tools/list", serde_json::json!({}), &modern).unwrap();
        rpc(&target, "tok", "tools/list", serde_json::json!({}), &modern).unwrap();
        assert_eq!(
            modern
                .methods()
                .iter()
                .filter(|m| *m == DISCOVER_METHOD)
                .count(),
            1,
            "a second call to a known origin must not re-probe"
        );

        // A different host on the same transport is a different verdict.
        let other = target_for("https://cache-b.example.com/mcp");
        assert_eq!(cached_era(&origin_of(&other.url)), None);
        rpc(&other, "tok", "tools/list", serde_json::json!({}), &modern).unwrap();
        assert_eq!(
            modern
                .methods()
                .iter()
                .filter(|m| *m == DISCOVER_METHOD)
                .count(),
            2
        );
    }

    #[test]
    fn two_paths_on_one_origin_share_a_verdict() {
        assert_eq!(
            origin_of("https://Same.Example.com:8443/mcp/v1"),
            origin_of("https://same.example.com:8443/other")
        );
        assert_ne!(
            origin_of("https://a.example.com"),
            origin_of("http://a.example.com")
        );
    }

    #[test]
    fn classify_era_separates_refusal_from_noise() {
        // A 2xx JSON-RPC result settles it.
        assert_eq!(
            classify_era(200, &modern_discover_reply()),
            EraVerdict::Modern
        );
        // A 400 naming an unknown method is the legacy signature.
        assert_eq!(
            classify_era(400, &legacy_unknown_method_reply()),
            EraVerdict::Legacy
        );
        // A 400 that names the modern revision is a *modern* server refusing
        // something else — never downgraded to legacy.
        assert_eq!(
            classify_era(
                400,
                &serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": {"code": -32602, "message": "MCP-Protocol-Version must be 2026-07-28"}
                })
            ),
            EraVerdict::Modern
        );
        // An anonymous contract refusal with no recognizable body.
        assert_eq!(
            classify_era(415, &serde_json::json!({"error": "unsupported"})),
            EraVerdict::Legacy
        );
        // Auth and availability say nothing about the era.
        assert_eq!(
            classify_era(401, &serde_json::json!({"error": "unauthorized"})),
            EraVerdict::Inconclusive
        );
        assert_eq!(
            classify_era(429, &serde_json::json!({"error": "slow down"})),
            EraVerdict::Inconclusive
        );
        assert_eq!(
            classify_era(500, &serde_json::json!({"error": "boom"})),
            EraVerdict::Inconclusive
        );
    }

    #[test]
    fn a_refused_call_is_a_typed_error_and_is_never_retried() {
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 200, modern_discover_reply()),
            (
                "tools/call",
                403,
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 2,
                    "error": {"code": -32000, "message": "guard denied"}
                }),
            ),
        ]);
        let target = target_for("https://refused.example.com/mcp");
        let error = rpc(
            &target,
            "tok-1",
            "tools/call",
            serde_json::json!({"name": "remote.write"}),
            &http,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RemoteError::Rpc {
                status: 403,
                ref method,
                ..
            } if method == "tools/call"
        ));
        assert_eq!(
            http.methods(),
            vec![DISCOVER_METHOD, "tools/call"],
            "a refused mutating call must not be re-sent under the other era"
        );
    }

    #[test]
    fn era_cache_is_an_explicit_value_not_a_global() {
        let mut cache = EraCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.get("https://x.example"), None);
        assert_eq!(
            cache.insert("https://x.example", McpEra::Legacy),
            McpEra::Legacy
        );
        assert_eq!(cache.get("https://x.example"), Some(McpEra::Legacy));
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn the_process_cache_is_shared_and_clearable() {
        let _era = era_cache_test_guard();
        let origin = "https://process-cache.example/mcp";
        assert_eq!(cached_era(&origin_of(origin)), None);
        cache_era(&origin_of(origin), McpEra::Legacy);
        assert_eq!(cached_era(&origin_of(origin)), Some(McpEra::Legacy));
        clear_era_cache();
        assert_eq!(cached_era(&origin_of(origin)), None);
    }

    #[test]
    fn eras_report_their_revisions() {
        assert_eq!(McpEra::Modern.version(), "2026-07-28");
        assert_eq!(McpEra::Legacy.version(), "2025-11-25");
        assert!(McpEra::Modern.is_modern());
        assert!(!McpEra::Legacy.is_modern());
    }

    #[test]
    fn the_legacy_body_is_untouched_and_the_modern_body_gains_only_meta() {
        let args = serde_json::json!({"name": "remote.write", "arguments": {"a": 1}});

        let legacy = build_request("tools/call", args.clone(), McpEra::Legacy, None);
        assert_eq!(legacy["params"], args);
        assert!(legacy["params"].get("_meta").is_none());
        // A non-object params value is the caller's business in the legacy era.
        let positional = build_request(
            "tools/call",
            serde_json::json!(["a", 1]),
            McpEra::Legacy,
            None,
        );
        assert_eq!(positional["params"], serde_json::json!(["a", 1]));

        let modern = build_request("tools/call", args.clone(), McpEra::Modern, Some("k-1"));
        assert_eq!(modern["params"]["name"], "remote.write");
        assert_eq!(modern["params"]["arguments"], serde_json::json!({"a": 1}));
        assert_eq!(
            modern["params"]["_meta"],
            serde_json::json!({"protocolVersion": "2026-07-28", "idempotencyKey": "k-1"})
        );
    }

    #[test]
    fn request_ids_are_not_reused() {
        // A stateless client has no session to disambiguate two identical
        // calls, so the id must be unique per request.
        let first = build_request("tools/list", serde_json::json!({}), McpEra::Modern, None);
        let second = build_request("tools/list", serde_json::json!({}), McpEra::Modern, None);
        assert_ne!(first["id"], second["id"]);
    }

    // -- the stored force-legacy hatch (item 2) ------------------------------

    #[test]
    fn connect_with_options_applies_the_stored_force_legacy_hatch() {
        let http = server();
        let plain = connect("https://mcp.example.com", &http).unwrap();
        assert!(
            !plain.force_legacy,
            "the default handshake must not arm the hatch"
        );
        let forced = connect_with_options(
            "https://mcp.example.com",
            &http,
            ConnectOptions { force_legacy: true },
        )
        .unwrap();
        assert!(forced.force_legacy);
        // The OAuth half of the handshake is unchanged by the era: the hatch
        // changes which revision later calls use, never how we authenticate.
        assert_eq!(forced.client.client_id, plain.client.client_id);
        assert_eq!(forced.url, plain.url);
    }

    #[test]
    fn connect_with_options_still_refuses_an_insecure_url() {
        let http = server();
        assert!(matches!(
            connect_with_options(
                "http://evil.example.com/mcp",
                &http,
                ConnectOptions { force_legacy: true },
            ),
            Err(RemoteError::InsecureUrl(_))
        ));
    }

    #[test]
    fn a_forced_target_reports_forced_without_spending_a_probe() {
        let _era = era_cache_test_guard();
        let http = ScriptedHttp::new([(
            "tools/call",
            200,
            serde_json::json!({"jsonrpc": "2.0", "id": 9, "result": {"ok": true}}),
        )]);
        let target = target_for("https://observed.example.com/mcp").with_force_legacy(true);
        let negotiation = negotiate_era_detailed(&target, Some("tok"), &http);
        assert_eq!(negotiation.era, McpEra::Legacy);
        assert_eq!(negotiation.source, EraSource::Forced);
        assert_eq!(negotiation.version(), LEGACY_PROTOCOL_VERSION);
        assert!(
            http.seen().is_empty(),
            "the source must be reachable without any wire traffic"
        );
        assert_eq!(
            negotiation.to_json(),
            serde_json::json!({"era": "2025-11-25", "eraSource": "forced"})
        );
    }

    #[test]
    fn a_cached_verdict_reports_cached_and_a_probe_reports_probed() {
        let _era = era_cache_test_guard();
        let origin = "https://sources.example.com/mcp";
        clear_era_cache();
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 200, modern_discover_reply()),
            (
                "tools/list",
                200,
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}}),
            ),
        ]);
        let target = target_for(origin);

        let first = negotiate_era_detailed(&target, Some("tok"), &http);
        assert_eq!(first.source, EraSource::Probed);
        assert_eq!(first.era, McpEra::Modern);
        let second = negotiate_era_detailed(&target, Some("tok"), &http);
        assert_eq!(second.source, EraSource::Cached);
        assert_eq!(second.to_json()["eraSource"], "cached");
        clear_era_cache();
    }

    #[test]
    fn an_inconclusive_probe_reports_default_and_names_the_window_spellings() {
        let _era = era_cache_test_guard();
        let http = ScriptedHttp::new([
            (DISCOVER_METHOD, 503, serde_json::json!({"error": "down"})),
            ("initialize", 503, serde_json::json!({"error": "down"})),
        ]);
        let target = target_for("https://silent.example.com/mcp");
        let negotiation = negotiate_era_detailed(&target, Some("tok"), &http);
        assert_eq!(negotiation.source, EraSource::Default);
        assert_eq!(negotiation.era, McpEra::Modern);
        // The four source spellings are the read-only wire vocabulary.
        assert_eq!(EraSource::Forced.as_str(), "forced");
        assert_eq!(EraSource::Cached.as_str(), "cached");
        assert_eq!(EraSource::Probed.as_str(), "probed");
        assert_eq!(EraSource::Default.as_str(), "default");
    }

    // -- the probe budget (item 3) ------------------------------------------

    #[test]
    fn the_probe_is_deadline_bounded_and_the_budget_is_ten_seconds() {
        let _era = era_cache_test_guard();
        assert_eq!(PROBE_BUDGET, Duration::from_secs(10));
        // A transport that records the budget it was handed proves the probe
        // actually asks for the cap rather than relying on library defaults.
        let http = BudgetRecordingHttp {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let target = target_for("https://budget.example.com/mcp");
        negotiate_era(&target, Some("tok"), &http);
        let seen = http.seen.lock().expect("budget lock").clone();
        assert_eq!(
            seen,
            vec![(DISCOVER_METHOD.to_string(), Some(PROBE_BUDGET))]
        );
    }

    #[test]
    fn the_budget_applies_to_the_probe_only() {
        let _era = era_cache_test_guard();
        let http = BudgetRecordingHttp {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let target = target_for("https://budget-call.example.com/mcp");
        rpc(
            &target,
            "tok",
            "tools/call",
            serde_json::json!({"name": "remote.write"}),
            &http,
        )
        .unwrap();
        let seen = http.seen.lock().expect("budget lock").clone();
        assert_eq!(
            seen,
            vec![
                (DISCOVER_METHOD.to_string(), Some(PROBE_BUDGET)),
                // The caller's own operation is never silently bounded by the
                // detection budget.
                ("tools/call".to_string(), None),
            ]
        );
    }

    // -- the stdio era path (item 3) ----------------------------------------

    #[test]
    fn the_stdio_probe_sends_the_same_modern_envelope_as_the_http_probe() {
        let request = build_discover_request(McpEra::Modern);
        assert_eq!(request["method"], DISCOVER_METHOD);
        assert_eq!(request["jsonrpc"], "2.0");
        // The `_meta` builder is shared, so a stdio probe is stateless-legal in
        // exactly the way the HTTP probe is.
        assert_eq!(
            request["params"]["_meta"]["protocolVersion"],
            serde_json::json!(MODERN_PROTOCOL_VERSION)
        );
        let headers = discover_probe_headers();
        assert!(headers.iter().any(
            |(name, value)| name == PROTOCOL_VERSION_HEADER && value == MODERN_PROTOCOL_VERSION
        ));
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == METHOD_HEADER && value == DISCOVER_METHOD)
        );
        // A legacy probe carries no `_meta`: a server that predates it must see
        // the wire shape it always saw.
        let legacy = build_discover_request(McpEra::Legacy);
        assert!(legacy["params"].get("_meta").is_none());
    }

    #[test]
    fn a_stdio_body_is_classified_by_the_same_rules_as_an_http_refusal() {
        // A JSON-RPC success is modern on either transport.
        assert_eq!(
            classify_era_body(&modern_discover_reply()),
            EraVerdict::Modern
        );
        // `-32601` to `server/discover` is the legacy signature.
        assert_eq!(
            classify_era_body(&legacy_unknown_method_reply()),
            EraVerdict::Legacy
        );
        // A body that proves neither stays inconclusive, so it is never cached.
        assert_eq!(
            classify_era_body(&serde_json::json!({"error": "upstream unavailable"})),
            EraVerdict::Inconclusive
        );
    }

    #[test]
    fn the_stdio_era_key_is_the_command_fingerprint() {
        let a = stdio_era_key("npx", &["-y", "@scope/server"]);
        // Same command line ⇒ same verdict, even from a second process.
        assert_eq!(a, stdio_era_key("npx", &["-y", "@scope/server"]));
        // An edited command re-probes.
        assert_ne!(a, stdio_era_key("npx", &["-y", "@scope/other"]));
        assert_ne!(a, stdio_era_key("uvx", &["-y", "@scope/server"]));
        // Component boundaries are length-prefixed, so no two splittings collide.
        assert_ne!(stdio_era_key("ab", &[]), stdio_era_key("a", &["b"]));
        // Namespaced away from an HTTP origin, which can never be equal.
        assert!(a.starts_with(STDIO_ERA_KEY_PREFIX));
        assert_ne!(a, origin_of("https://x.example"));
        // The key must not retain the raw argv: a token in an argument would
        // otherwise live in process-global state.
        assert!(!a.contains("@scope/server"));
    }

    #[test]
    fn a_stdio_probe_concludes_and_caches_its_verdict_under_the_command_key() {
        let _era = era_cache_test_guard();
        clear_era_cache();
        let mut probe = ScriptedStdio::new(modern_discover_reply());
        let verdict = negotiate_stdio_era("npx", &["-y", "@scope/server"], &mut probe);
        assert_eq!(verdict.era, McpEra::Modern);
        assert_eq!(verdict.source, EraSource::Probed);
        assert_eq!(
            probe.sent.len(),
            1,
            "exactly one discover probe per detection"
        );
        assert_eq!(probe.budgets, vec![PROBE_BUDGET]);
        // The request that went out is the shared modern envelope.
        assert_eq!(probe.methods(), vec![DISCOVER_METHOD.to_string()]);
        assert_eq!(
            cached_era(&stdio_era_key("npx", &["-y", "@scope/server"])),
            Some(McpEra::Modern)
        );

        // A second child with the same command line reuses the verdict.
        let mut second = ScriptedStdio::new(serde_json::json!({"error": "must not be asked"}));
        let cached = negotiate_stdio_era("npx", &["-y", "@scope/server"], &mut second);
        assert_eq!(cached.source, EraSource::Cached);
        assert_eq!(second.sent.len(), 0, "a cached verdict must not re-probe");
        clear_era_cache();
    }

    #[test]
    fn a_legacy_stdio_probe_is_cached_as_legacy() {
        let _era = era_cache_test_guard();
        clear_era_cache();
        let mut probe = ScriptedStdio::new(legacy_unknown_method_reply());
        let verdict = negotiate_stdio_era("cat", &[], &mut probe);
        assert_eq!(verdict.era, McpEra::Legacy);
        assert_eq!(verdict.source, EraSource::Probed);
        assert_eq!(cached_era(&stdio_era_key("cat", &[])), Some(McpEra::Legacy));
        clear_era_cache();
    }

    #[test]
    fn a_silent_or_broken_stdio_probe_is_inconclusive_and_not_cached() {
        let _era = era_cache_test_guard();
        clear_era_cache();
        for reply in [
            serde_json::json!({"error": "upstream unavailable"}),
            serde_json::json!({"jsonrpc": "2.0", "id": 1}),
        ] {
            let mut probe = ScriptedStdio::new(reply);
            let verdict = negotiate_stdio_era("quiet", &[], &mut probe);
            assert_eq!(verdict.source, EraSource::Default);
            assert_eq!(verdict.era, McpEra::Modern);
            assert!(cached_era(&stdio_era_key("quiet", &[])).is_none());
        }
        // A transport failure is inconclusive too: no reply, no verdict.
        let mut broken = ScriptedStdio::failing();
        let verdict = negotiate_stdio_era("broken", &[], &mut broken);
        assert_eq!(verdict.source, EraSource::Default);
        assert!(cached_era(&stdio_era_key("broken", &[])).is_none());
        clear_era_cache();
    }

    #[test]
    fn the_stdio_verdict_does_not_disturb_the_http_origin_cache() {
        let _era = era_cache_test_guard();
        clear_era_cache();
        let mut probe = ScriptedStdio::new(modern_discover_reply());
        negotiate_stdio_era("npx", &["-y", "@scope/server"], &mut probe);
        // A stdio child has no origin; the HTTP cache for the same host name
        // must still be empty rather than inheriting a child verdict.
        assert!(cached_era("https://@scope/server").is_none());
        clear_era_cache();
    }
}
