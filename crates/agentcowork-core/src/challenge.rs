//! P2.8 Challenge Handler (E12 — ARCH/08 §8.10).
//!
//! Defense-in-depth for captchas of all types, ordered by cost/effectiveness.
//! This module owns the *routing*, the *locally-solvable* class, the
//! *human-in-the-loop* pass-through registry, the *visual-grounding* contract,
//! and the *BYO solver* HTTP client:
//!
//! * **Prevention** lives in the browser tier + session inheritance (E13) and
//!   behavioral realism (E14) — not here.
//! * **Human-in-the-loop pass-through** is the universal default: surface the
//!   tab in the visible webview, the user solves once, cookies → vault. The
//!   registry ([`ChallengeHandler::surface`] / [`resolve_human`]) guarantees a
//!   challenge is solved exactly once (an id can't be redeemed twice).
//! * **Proof-of-Work** (Altcha / Friendly Captcha) is the self-hostable class:
//!   pure SHA-256 leading-zero puzzles solved here, no external calls.
//! * **Visual grounding** (simple visual challenges) is the LLM-assisted
//!   class: snapshot → option list → the model picks a ref/point. This module
//!   defines the request/choice contract ([`parse_grounding_choice`]); the
//!   sidecar makes the actual model call.
//! * **BYO solver APIs** (CapSolver / 2Captcha) are an optional, user-key-
//!   gated escape hatch — never a default, never bundled credit. The HTTP
//!   client ([`solve_captcha`]) is transport-injected so tests need no live
//!   solver.
//! * **Turnstile** (Cloudflare, incl. "hidden" / non-interactive mode) is a
//!   *managed* challenge — NOT locally solvable; routed to human/BYO honestly.
//!
//! The PoW solver implements the Altcha / Friendly Captcha contract: find a
//! nonce `n` in `[0, max_number]` such that `hex(sha256(salt || n))` has
//! `difficulty` leading zero nibbles. (Full Altcha round-trip additionally
//! verifies an HMAC signature; solving the puzzle itself is what "solve
//! locally" requires.)

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Detected challenge family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChallengeKind {
    /// Google reCAPTCHA — managed, human-in-loop.
    Recaptcha,
    /// hCaptcha — managed, human-in-loop.
    HCaptcha,
    /// Cloudflare Turnstile (incl. hidden mode) — managed, human-in-loop/BYO.
    Turnstile,
    /// Altcha — SHA-256 proof-of-work, locally solvable.
    Altcha,
    /// Friendly Captcha — SHA-256 proof-of-work, locally solvable.
    FriendlyCaptcha,
}

impl ChallengeKind {
    pub fn label(self) -> &'static str {
        match self {
            ChallengeKind::Recaptcha => "recaptcha",
            ChallengeKind::HCaptcha => "hcaptcha",
            ChallengeKind::Turnstile => "turnstile",
            ChallengeKind::Altcha => "altcha",
            ChallengeKind::FriendlyCaptcha => "friendly_captcha",
        }
    }
}

/// What to do about a detected challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeResolution {
    /// Solve a proof-of-work puzzle locally (no external calls).
    LocalPow { kind: ChallengeKind },
    /// Surface the tab in the visible webview for the human.
    HumanInLoop { site: String, kind: ChallengeKind },
    /// Snapshot → option list → LLM picks a ref/point (simple visual puzzles).
    VisualGrounding { site: String, kind: ChallengeKind },
    /// Route to a user-configured BYO solver API (their key/credit).
    ByoSolver {
        provider: String,
        kind: ChallengeKind,
    },
}

/// A human-in-the-loop challenge that has been surfaced (awaiting the user).
/// The id is single-use: redeeming it twice is refused.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HumanChallenge {
    pub id: String,
    pub site: String,
    pub kind: ChallengeKind,
    pub created_ms: u64,
}

/// One element option offered to the model during visual grounding (a
/// snapshot ref + its readable label).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GroundingOption {
    pub id: String,
    pub label: String,
}

/// The visual-grounding request the sidecar sends to the model.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VisualGroundingRequest {
    pub kind: ChallengeKind,
    pub site: String,
    pub prompt: String,
    pub options: Vec<GroundingOption>,
}

/// The model's answer to a [`VisualGroundingRequest`], parsed from free text
/// by [`parse_grounding_choice`].
#[derive(Debug, Clone, PartialEq)]
pub enum GroundingChoice {
    /// The model picked one of the offered options by id.
    Option(String),
    /// The model gave a viewport point (pixel x,y).
    Point { x: f64, y: f64 },
    /// The model can't solve it → escalate to human.
    Unsolvable,
}

/// Which BYO solver service to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByoProvider {
    CapSolver,
    TwoCaptcha,
}

impl ByoProvider {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "capsolver" => Some(ByoProvider::CapSolver),
            "2captcha" | "twocaptcha" => Some(ByoProvider::TwoCaptcha),
            _ => None,
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            ByoProvider::CapSolver => "https://api.capsolver.com",
            ByoProvider::TwoCaptcha => "https://api.2captcha.com",
        }
    }

    /// reCAPTCHA v2 token task (no proxy) per provider's task taxonomy.
    fn recaptcha_v2_task_type(self) -> &'static str {
        match self {
            ByoProvider::CapSolver => "ReCaptchaV2TaskProxyLess",
            ByoProvider::TwoCaptcha => "RecaptchaV2TaskProxyless",
        }
    }
}

/// BYO solver errors.
#[derive(Debug, thiserror::Error)]
pub enum ByoSolverError {
    #[error("solver http: {0}")]
    Http(String),
    #[error("solver returned errorId {0}: {1}")]
    Solver(i64, String),
    #[error("solver task timed out after {0} polls")]
    Timeout(u32),
    #[error("solver response missing {0}")]
    Malformed(String),
}

/// Transport seam for the BYO solver HTTP calls. The default [`UreqHttp`] is
/// the real network path; tests inject a scripted transport.
pub trait SolverHttp {
    fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<serde_json::Value, String>;
}

/// The real transport uses Guard-pinned egress. The provider URLs are selected
/// from [`ByoProvider`], and the trait seam remains available for test doubles.
pub struct UreqHttp;

impl SolverHttp for UreqHttp {
    fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let client = agentcowork_guard::egress_http::GuardedHttpClient::new(
            url,
            agentcowork_guard::NetPolicy::default(),
            std::time::Duration::from_secs(20),
        )
        .map_err(|_| "Guard refused the challenge-solver destination".to_string())?;
        let payload = serde_json::to_vec(body).map_err(|_| "invalid solver request".to_string())?;
        if payload.len() > 1024 * 1024 {
            return Err("challenge-solver request exceeds the configured limit".to_string());
        }
        let response = client
            .request(
                "POST",
                url,
                &[
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                ],
                Some(&payload),
                1024 * 1024,
            )
            .map_err(|_| "Guarded challenge-solver request failed".to_string())?;
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "challenge solver returned HTTP {}",
                response.status
            ));
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| "invalid solver response JSON".to_string())
    }
}

/// Stateless challenge router + PoW solver + human-in-loop registry.
#[derive(Default)]
pub struct ChallengeHandler {
    byo_provider: Option<String>,
    /// Surfaced-but-unresolved human challenges (single-use by id).
    pending: Mutex<HashMap<String, HumanChallenge>>,
    next_id: AtomicU64,
}

impl ChallengeHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach an optional BYO solver provider (e.g. "capsolver", "2captcha").
    pub fn with_byo_provider(mut self, provider: impl Into<String>) -> Self {
        self.byo_provider = Some(provider.into());
        self
    }

    /// Heuristic challenge detection from page text (CDP a11y snapshot or HTML).
    /// Case-insensitive; checks PoW markers first so they are never
    /// misclassified as a managed captcha.
    pub fn detect(&self, page_text: &str) -> Option<ChallengeKind> {
        let t = page_text.to_ascii_lowercase();
        if t.contains("altcha") {
            return Some(ChallengeKind::Altcha);
        }
        if t.contains("friendly captcha")
            || t.contains("frc-captcha")
            || t.contains("friendly-challenge")
        {
            return Some(ChallengeKind::FriendlyCaptcha);
        }
        if t.contains("turnstile")
            || t.contains("cf-turnstile")
            || t.contains("challenges.cloudflare.com")
        {
            return Some(ChallengeKind::Turnstile);
        }
        if t.contains("hcaptcha") || t.contains("h-captcha") {
            return Some(ChallengeKind::HCaptcha);
        }
        if t.contains("recaptcha") || t.contains("g-recaptcha") || t.contains("grecaptcha") {
            return Some(ChallengeKind::Recaptcha);
        }
        None
    }

    /// Route a detected challenge to its resolution.
    ///
    /// * Altcha / Friendly Captcha → solved locally (`solve_pow`), even when a
    ///   BYO provider is configured (no point paying for a puzzle we can solve).
    /// * Managed captchas → human-in-loop by default, or BYO when configured.
    pub fn route(&self, kind: ChallengeKind, site: &str) -> ChallengeResolution {
        match kind {
            ChallengeKind::Altcha | ChallengeKind::FriendlyCaptcha => {
                ChallengeResolution::LocalPow { kind }
            }
            _ => match &self.byo_provider {
                Some(provider) => ChallengeResolution::ByoSolver {
                    provider: provider.clone(),
                    kind,
                },
                None => ChallengeResolution::HumanInLoop {
                    site: site.to_string(),
                    kind,
                },
            },
        }
    }

    /// Route a managed challenge, preferring visual grounding for simple
    /// puzzles before falling back to human/BYO. The caller decides whether a
    /// challenge is "simple" (has a snapshot + candidate options); this only
    /// encodes the preference.
    pub fn route_visual(
        &self,
        kind: ChallengeKind,
        site: &str,
        allow_grounding: bool,
    ) -> ChallengeResolution {
        if allow_grounding
            && matches!(
                kind,
                ChallengeKind::Recaptcha | ChallengeKind::HCaptcha | ChallengeKind::Turnstile
            )
        {
            return ChallengeResolution::VisualGrounding {
                site: site.to_string(),
                kind,
            };
        }
        self.route(kind, site)
    }

    // ------------------------------------------------------------------
    // human-in-the-loop registry
    // ------------------------------------------------------------------

    /// Surface a managed challenge for the human (webview). Registers it and
    /// returns the single-use [`HumanChallenge`] the UI shows.
    pub fn surface(&self, kind: ChallengeKind, site: &str) -> HumanChallenge {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let challenge = HumanChallenge {
            id: format!("hc-{id}"),
            site: site.to_string(),
            kind,
            created_ms: now_ms(),
        };
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(challenge.id.clone(), challenge.clone());
        challenge
    }

    /// Redeem a surfaced challenge with the user's solution. Returns `Some` on
    /// first redemption (the id is then consumed — a second redemption is
    /// refused, so a stale/duplicate solve can't be replayed). `None` means
    /// the id is unknown or already redeemed.
    pub fn resolve_human(&self, id: &str) -> Option<HumanChallenge> {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id)
    }

    /// Challenges still awaiting a human (the UI's "needs you" list).
    pub fn pending(&self) -> Vec<HumanChallenge> {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------------
    // visual grounding contract
    // ------------------------------------------------------------------

    /// Build the request the sidecar sends to the model for a visual puzzle.
    pub fn grounding_request(
        &self,
        kind: ChallengeKind,
        site: &str,
        prompt: &str,
        options: Vec<GroundingOption>,
    ) -> VisualGroundingRequest {
        VisualGroundingRequest {
            kind,
            site: site.to_string(),
            prompt: prompt.to_string(),
            options,
        }
    }

    // ------------------------------------------------------------------
    // proof-of-work solver
    // ------------------------------------------------------------------

    /// Solve a SHA-256 proof-of-work puzzle: find `n` in `[0, max_number]`
    /// such that `hex(sha256(salt || n))` has `difficulty` leading zero
    /// nibbles. Returns the nonce, or `None` when no solution exists in range
    /// (honest failure → caller falls back to human-in-loop).
    pub fn solve_pow(salt: &str, max_number: u64, difficulty: u32) -> Option<u64> {
        (0..=max_number).find(|&n| Self::verify_pow(salt, n, difficulty))
    }

    /// Verify a candidate nonce against the difficulty requirement.
    pub fn verify_pow(salt: &str, nonce: u64, difficulty: u32) -> bool {
        let mut h = Sha256::new();
        h.update(salt.as_bytes());
        h.update(nonce.to_string().as_bytes());
        hex_starts_with(&h.finalize(), difficulty)
    }
}

// ---------------------------------------------------------------------------
// Visual grounding: free-text → structured choice
// ---------------------------------------------------------------------------

/// Parse the model's free-text answer into a [`GroundingChoice`].
///
/// Accepted forms (case-insensitive):
/// * an offered option id verbatim → `Option(id)`
/// * `option <id>` / `choose <id>` / `pick <id>` / `<id>` (when the token
///   matches a provided option id) → `Option(id)`
/// * `(<x>, <y>)` / `x <x> y <y>` / `point <x> <y>` → `Point { x, y }`
/// * `unable` / `unsolvable` / `can't` / `cannot` / `none` → `Unsolvable`
///
/// `option_ids` lets us anchor a bare token to a real option id (avoids
/// hallucinated ids). Returns `None` when nothing recognizable is found.
pub fn parse_grounding_choice(text: &str, option_ids: &[String]) -> Option<GroundingChoice> {
    let t = text.trim().to_ascii_lowercase();

    // Explicit unsolvable markers first.
    if t.is_empty()
        || t == "none"
        || t == "unable"
        || t == "unsolvable"
        || t.contains("can't")
        || t.contains("cannot")
        || t.contains("not sure")
    {
        return Some(GroundingChoice::Unsolvable);
    }

    // Coordinate forms: "(x, y)" or "point x y" or "x, y".
    if let Some((x, y)) = parse_point(&t) {
        return Some(GroundingChoice::Point { x, y });
    }

    // Option-id forms.
    for prefix in ["option ", "choose ", "pick ", "select ", "the answer is "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let token = rest.split_whitespace().next().unwrap_or_default();
            if let Some(id) = match_option_id(token, option_ids) {
                return Some(GroundingChoice::Option(id));
            }
        }
    }
    // A single bare token that matches a provided option id.
    let single: Vec<&str> = t.split_whitespace().collect();
    if single.len() == 1 {
        if let Some(id) = match_option_id(single[0], option_ids) {
            return Some(GroundingChoice::Option(id));
        }
    }

    None
}

fn match_option_id(token: &str, option_ids: &[String]) -> Option<String> {
    let token = token
        .trim()
        .trim_matches(|c| c == '\'' || c == '"' || c == '`');
    option_ids
        .iter()
        .find(|id| id.eq_ignore_ascii_case(token))
        .cloned()
}

/// Extract `(x, y)` from free text (handles decimals, negative, and a few
/// common separators). Returns `None` if no coordinate pair is found.
fn parse_point(t: &str) -> Option<(f64, f64)> {
    // "(123, 45)" — the most common model output.
    let paren = t.find('(')?;
    let rest = &t[paren + 1..];
    let close = rest.find(')')?;
    let inner = &rest[..close];
    // ", " produces an empty token between the delimiters — filter it out.
    let mut nums = inner.split([',', ' ']).filter(|s| !s.is_empty());
    let x: f64 = nums.next()?.trim().parse().ok()?;
    let y: f64 = nums.next()?.trim().parse().ok()?;
    Some((x, y))
}

// ---------------------------------------------------------------------------
// BYO solver HTTP (CapSolver / 2Captcha)
// ---------------------------------------------------------------------------

/// Solve a reCAPTCHA-v2-style challenge via a BYO solver (user's own key +
/// credit). `createTask` once, then poll `getTaskResult` up to `max_polls`
/// times with `poll_interval` between polls. Returns the response token.
pub fn solve_captcha(
    http: &dyn SolverHttp,
    provider: ByoProvider,
    api_key: &str,
    site_url: &str,
    site_key: &str,
    poll_interval: Duration,
    max_polls: u32,
) -> Result<String, ByoSolverError> {
    let task_id = create_task(http, provider, api_key, site_url, site_key)?;
    for _ in 0..max_polls {
        std::thread::sleep(poll_interval);
        match poll_task(http, provider, api_key, &task_id)? {
            Some(token) => return Ok(token),
            None => continue, // still processing
        }
    }
    Err(ByoSolverError::Timeout(max_polls))
}

/// `createTask` → the solver's task id.
pub fn create_task(
    http: &dyn SolverHttp,
    provider: ByoProvider,
    api_key: &str,
    site_url: &str,
    site_key: &str,
) -> Result<String, ByoSolverError> {
    let body = serde_json::json!({
        "clientKey": api_key,
        "task": {
            "type": provider.recaptcha_v2_task_type(),
            "websiteURL": site_url,
            "websiteKey": site_key,
        }
    });
    let url = format!("{}/createTask", provider.base_url());
    let resp = http.post_json(&url, &body).map_err(ByoSolverError::Http)?;
    let error_id = resp.get("errorId").and_then(|v| v.as_i64()).unwrap_or(0);
    if error_id != 0 {
        let desc = resp
            .get("errorDescription")
            .or_else(|| resp.get("errorCode"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(ByoSolverError::Solver(error_id, desc));
    }
    resp.get("taskId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ByoSolverError::Malformed("taskId".into()))
}

/// `getTaskResult` → `Some(token)` when ready, `None` while processing.
pub fn poll_task(
    http: &dyn SolverHttp,
    provider: ByoProvider,
    api_key: &str,
    task_id: &str,
) -> Result<Option<String>, ByoSolverError> {
    let body = serde_json::json!({ "clientKey": api_key, "taskId": task_id });
    let url = format!("{}/getTaskResult", provider.base_url());
    let resp = http.post_json(&url, &body).map_err(ByoSolverError::Http)?;
    let error_id = resp.get("errorId").and_then(|v| v.as_i64()).unwrap_or(0);
    if error_id != 0 {
        let desc = resp
            .get("errorDescription")
            .or_else(|| resp.get("errorCode"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        return Err(ByoSolverError::Solver(error_id, desc));
    }
    match resp.get("status").and_then(|v| v.as_str()) {
        Some("ready") => Ok(resp
            .pointer("/solution/gRecaptchaResponse")
            .or_else(|| resp.pointer("/solution/token"))
            .and_then(|v| v.as_str())
            .map(str::to_string)),
        Some("processing") | Some("pending") => Ok(None),
        other => Err(ByoSolverError::Malformed(format!(
            "unexpected status: {other:?}"
        ))),
    }
}

/// Does the SHA-256 digest's hex string start with `zeros` zero nibbles?
fn hex_starts_with(digest: &[u8], zeros: u32) -> bool {
    let mut remaining = zeros as usize;
    for &byte in digest {
        let hi = (byte >> 4) as usize;
        let lo = (byte & 0x0f) as usize;
        if remaining == 0 {
            return true;
        }
        if hi != 0 {
            return false;
        }
        remaining -= 1;
        if remaining == 0 {
            return true;
        }
        if lo != 0 {
            return false;
        }
        remaining -= 1;
    }
    remaining == 0
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "challenge_tests.rs"]
mod tests;
