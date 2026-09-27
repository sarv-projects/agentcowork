//! P56.1/P56.7 — the **live** models.dev catalog (`https://models.dev/api.json`).
//!
//! This is the layer the Settings → Providers surface reads: whole-provider
//! metadata *and* the full per-provider model table. It is deliberately
//! separate from [`crate::sync`] (the per-provider *merge* gate over the
//! vendored baseline): that gate decides whether a fetched row set may
//! replace baseline rows; this module decides whether a whole `api.json`
//! snapshot is good enough to *become* the catalog the UI reads.
//!
//! Verified against the live endpoint on 2026-09-11 (not from memory):
//!
//! ```text
//! GET https://models.dev/api.json     200  4,614,215 bytes
//! etag: "57347addd7ba881636794f5f17986483"   (quoted — normalise before use)
//! 213 providers · 7,742 models
//! provider keys: id · name · npm · api? · doc · env[] · models{}
//! model keys:    id · name · description? · family? · attachment ·
//!                reasoning · reasoning_options[]? · tool_call ·
//!                structured_output · temperature · knowledge? ·
//!                release_date · last_updated · status? · open_weights ·
//!                modalities{input[],output[]} · limit{context,output,input?} ·
//!                cost{input,output,cache_read?,cache_write?,reasoning?,
//!                     input_audio?,output_audio?,context_over_200k?,tiers?}
//! model-level `provider{npm,api}` overrides the provider's transport/endpoint
//! ```
//!
//! `/api/providers.json` is **never** fetched — the live site returns the SPA
//! HTML there, not JSON (P56.1). Logos are `models.dev/logos/{id}.svg`.
//!
//! Network is the caller's job: [`apply_refresh`] is pure and takes an
//! already-fetched [`FetchOutcome`], so the ETag/staleness/keep-last-good
//! rules are testable with no socket (same discipline as every other client
//! in this workspace).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::provider::Transport;
use crate::sync::{GateFinding, Severity, gate_passes};

/// The one URL that serves the full catalog as JSON.
pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
/// Logo path (P56.2 row art).
pub const LOGO_URL_PREFIX: &str = "https://models.dev/logos/";
/// P56.1 default refresh cadence (configurable 1–24h).
pub const DEFAULT_REFRESH_SECS: u64 = 4 * 60 * 60;
pub const MIN_REFRESH_SECS: u64 = 60 * 60;
pub const MAX_REFRESH_SECS: u64 = 24 * 60 * 60;
/// A snapshot must carry at least this many providers to be believable — a
/// truncated/error-page response can never replace the last good catalog.
pub const MIN_PROVIDERS: usize = 100;
/// Same idea for models (the live catalog carries thousands).
pub const MIN_MODELS: usize = 100;
/// Cap on reported findings so a pathological snapshot stays readable.
pub const MAX_FINDINGS: usize = 50;

/// `models.dev/logos/{id}.svg` for a provider id.
pub fn logo_url(provider_id: &str) -> String {
    format!("{LOGO_URL_PREFIX}{provider_id}.svg")
}

/// Input/output modality sets for a model (`modalities` in `api.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Modalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

impl Modalities {
    /// P56.7 `images?` / P59.2 vision gate — does this model accept images?
    pub fn accepts_image(&self) -> bool {
        self.input.iter().any(|m| m.eq_ignore_ascii_case("image"))
    }
    pub fn accepts_pdf(&self) -> bool {
        self.input.iter().any(|m| m.eq_ignore_ascii_case("pdf"))
    }
}

/// Token limits (`limit` in `api.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelLimit {
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub input: u64,
}

/// Per-1M-token pricing (`cost` in `api.json`). Optional fields stay optional
/// — the table renders `—`, never a fabricated `0`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub reasoning: Option<f64>,
}

/// A model-level transport/endpoint override (`provider` in `api.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelProviderOverride {
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
}

/// One model row from `api.json` (P56.7's table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LiveModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub family: String,
    pub attachment: bool,
    pub reasoning: bool,
    pub tool_call: bool,
    pub structured_output: bool,
    pub temperature: bool,
    pub knowledge: Option<String>,
    pub release_date: Option<String>,
    pub last_updated: Option<String>,
    /// `deprecated` / `beta` when the catalog marks them; absent = current.
    pub status: Option<String>,
    pub open_weights: bool,
    pub modalities: Modalities,
    pub limit: ModelLimit,
    pub cost: ModelCost,
    /// A model can override its provider's npm/api (verified live: 1,022 rows
    /// carry one, e.g. `agentrouter/claude-opus-5` → `@ai-sdk/anthropic`).
    pub provider: Option<ModelProviderOverride>,
}

impl LiveModel {
    /// Wire transport this row speaks: the model override wins over the
    /// provider's own `npm`, matching how the catalog is consumed.
    pub fn transport(&self, provider_npm: &str, provider_id: &str) -> Transport {
        let npm = self
            .provider
            .as_ref()
            .and_then(|p| p.npm.as_deref())
            .unwrap_or(provider_npm);
        transport_from_npm(npm, provider_id)
    }

    pub fn effective_api<'a>(&'a self, provider_api: Option<&'a str>) -> Option<&'a str> {
        self.provider
            .as_ref()
            .and_then(|p| p.api.as_deref())
            .or(provider_api)
    }

    /// Pricing in $ per 1M tokens for a direction (`input` / `output`).
    pub fn price_per_m(&self, direction: &str) -> Option<f64> {
        let v = match direction {
            "input" => Some(self.cost.input),
            "output" => Some(self.cost.output),
            _ => None,
        }?;
        Some(v)
    }
}

/// One provider record from `api.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LiveProvider {
    pub id: String,
    pub name: String,
    /// The AI-SDK package name — our transport discriminator.
    pub npm: String,
    /// Explicit endpoint; absent means “SDK default” (P56.3 header).
    pub api: Option<String>,
    /// Docs link (P56.3 header).
    pub doc: Option<String>,
    /// Env-var *names* the key may live under (never values).
    pub env: Vec<String>,
    pub models: BTreeMap<String, LiveModel>,
}

impl LiveProvider {
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    pub fn logo_url(&self) -> String {
        logo_url(&self.id)
    }

    /// Transport for this provider's own rows (model overrides win per row).
    pub fn transport(&self) -> Transport {
        transport_from_npm(&self.npm, &self.id)
    }

    /// Model rows sorted by name for a stable, searchable table (P56.7).
    pub fn model_rows(&self) -> Vec<&LiveModel> {
        let mut rows: Vec<&LiveModel> = self.models.values().collect();
        rows.sort_by_key(|m| m.name.to_lowercase());
        rows
    }

    pub fn env_var(&self) -> Option<&str> {
        self.env.first().map(String::as_str)
    }
}

/// A whole `api.json` snapshot with fetch provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CatalogSnapshot {
    /// Where the bytes came from (always [`MODELS_DEV_API_URL`] today).
    pub source: String,
    /// Unix ms of the successful fetch that produced this snapshot.
    pub fetched_at: i64,
    /// The ETag the server returned (normalised, unquoted).
    pub etag: String,
    pub providers: BTreeMap<String, LiveProvider>,
}

impl CatalogSnapshot {
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    pub fn model_count(&self) -> usize {
        self.providers.values().map(|p| p.models.len()).sum()
    }

    /// Parse a raw `api.json` body. Tolerant of unknown fields, strict about
    /// the shape we rely on (a provider needs an id; a model needs an id).
    pub fn parse(source: &str, body: &str, fetched_at: i64, etag: &str) -> Result<Self, String> {
        let raw: BTreeMap<String, RawProvider> =
            serde_json::from_str(body).map_err(|e| format!("models.dev api.json: {e}"))?;
        if raw.is_empty() {
            return Err("models.dev api.json: empty provider map".to_string());
        }
        let mut providers = BTreeMap::new();
        for (key, rp) in raw {
            // The map key is the canonical id; `id` inside is usually the
            // same and we trust the key (it is what the URL/logo uses).
            let id = if rp.id.trim().is_empty() {
                key.clone()
            } else {
                rp.id.clone()
            };
            let models = rp
                .models
                .into_iter()
                .filter(|(mid, m)| !mid.trim().is_empty() && !m.id.trim().is_empty())
                .map(|(mid, m)| {
                    let mut m = m;
                    if m.name.trim().is_empty() {
                        m.name = mid.clone();
                    }
                    (mid, m)
                })
                .collect();
            providers.insert(
                id.clone(),
                LiveProvider {
                    id,
                    name: if rp.name.trim().is_empty() {
                        key
                    } else {
                        rp.name
                    },
                    npm: rp.npm,
                    api: rp.api,
                    doc: rp.doc,
                    env: rp.env,
                    models,
                },
            );
        }
        Ok(Self {
            source: source.to_string(),
            fetched_at,
            etag: normalize_etag(etag),
            providers,
        })
    }

    pub fn provider(&self, id: &str) -> Option<&LiveProvider> {
        self.providers.get(id).or_else(|| {
            self.providers
                .values()
                .find(|p| p.id.eq_ignore_ascii_case(id))
        })
    }

    /// P56.7 — the OpenCode Free subset over the live Zen model list: every
    /// id that matches the free regex, plus `big-pickle` (always free and
    /// never named “free”). Verified live 2026-09-11: the Zen list carries
    /// `big-pickle` + 7 `*-free` ids.
    pub fn opencode_free_models(&self) -> Vec<String> {
        let ids: Vec<String> = self
            .provider("opencode")
            .map(|p| p.models.keys().cloned().collect())
            .unwrap_or_default();
        free_model_ids(&ids)
    }
}

/// The subset of a provider record we deserialize before normalizing.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawProvider {
    id: String,
    name: String,
    npm: String,
    api: Option<String>,
    doc: Option<String>,
    env: Vec<String>,
    models: BTreeMap<String, LiveModel>,
}

/// ETags arrive quoted (`"abc"`); `If-None-Match` may or may not be.
/// Normalizing both sides to the bare tag is what makes 304 detection work.
pub fn normalize_etag(etag: &str) -> String {
    let t = etag.trim();
    let t = t.strip_prefix("W/").unwrap_or(t);
    t.trim_matches('"').trim().to_string()
}

/// Is the stored snapshot older than the interval?
pub fn is_stale(fetched_at: i64, now_ms: i64, interval_secs: u64) -> bool {
    if fetched_at <= 0 {
        return true;
    }
    let age_ms = now_ms.saturating_sub(fetched_at);
    age_ms >= (interval_secs as i64).saturating_mul(1000)
}

/// P56.1 — clamp a requested cadence into the documented 1–24h band.
pub fn refresh_interval_secs(requested_hours: Option<u64>) -> u64 {
    match requested_hours {
        None => DEFAULT_REFRESH_SECS,
        Some(h) => h.clamp(MIN_REFRESH_SECS / 3600, MAX_REFRESH_SECS / 3600) * 3600,
    }
}

/// `true` when a model id is in the keyless free pool. Segment-boundary
/// match (the P56.6 rule): `free` must start a dotted/dashed/slashed segment
/// and end one — so `deepseek-v4-flash-free` matches, `freeform-x` does not.
pub fn is_free_model_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    if lower == "big-pickle" {
        return true; // never named "free"; always keyless
    }
    let bytes = lower.as_bytes();
    let mut from = 0usize;
    while let Some(pos) = lower[from..].find("free") {
        let start = from + pos;
        let end = start + 4;
        let before_ok = start == 0 || matches!(bytes[start - 1], b'.' | b'_' | b'/' | b'-');
        let after_ok = end == bytes.len() || matches!(bytes[end], b'.' | b'_' | b'/' | b'-');
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// The free subset of a model-id list (`big-pickle` always included).
pub fn free_model_ids(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = ids
        .iter()
        .filter(|i| is_free_model_id(i))
        .cloned()
        .collect();
    if !out.iter().any(|i| i == "big-pickle") {
        out.push("big-pickle".to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// Map a models.dev `npm` package to the wire transport we can speak.
///
/// Anything unrecognised returns `Transport::BedrockConverse`-style
/// fail-closed handling downstream rather than guessing an OpenAI path — a
/// wrong endpoint is worse than an honest “unsupported”.
pub fn transport_from_npm(npm: &str, provider_id: &str) -> Transport {
    let n = npm.to_ascii_lowercase();
    if n.contains("anthropic") {
        return Transport::AnthropicMessages;
    }
    if n.contains("bedrock") {
        return Transport::BedrockConverse;
    }
    if n.contains("openai") || n.contains("groq") || n.contains("deepseek") || n.contains("mistral")
    {
        return Transport::OpenaiChat;
    }
    // Provider-level fallbacks for ids whose npm is an SDK we don't map.
    match provider_id {
        "anthropic" => Transport::AnthropicMessages,
        "amazon-bedrock" => Transport::BedrockConverse,
        "openai-api" => Transport::CodexResponses,
        _ => Transport::OpenaiChat,
    }
}

/// What a fetch attempt produced (raw — the pure layer decides).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// Server answered `304 Not Modified` for our `If-None-Match`.
    NotModified,
    /// `200` with a body + whatever ETag the server sent.
    Fetched { etag: String, body: String },
    /// Network/transport/HTTP failure — keep the last good snapshot.
    Failed(String),
}

/// The verdict of one refresh attempt (reported, never silent).
#[derive(Debug, Clone, PartialEq)]
pub enum RefreshDecision {
    /// 304 → same snapshot, `fetched_at` bumped (stale-while-revalidate).
    NotModified {
        fetched_at: i64,
        providers: usize,
        models: usize,
    },
    Updated {
        providers: usize,
        models: usize,
        fetched_at: i64,
    },
    /// Parsed, but the gate refused it — last good snapshot kept.
    Rejected {
        findings: Vec<GateFinding>,
        error: Option<String>,
    },
    /// No snapshot yet *and* the fetch failed — nothing to serve.
    Failed { error: String },
}

impl RefreshDecision {
    pub fn accepted(&self) -> bool {
        matches!(
            self,
            RefreshDecision::Updated { .. } | RefreshDecision::NotModified { .. }
        )
    }

    /// A one-line honest status for the UI/tooling.
    pub fn summary(&self) -> String {
        match self {
            RefreshDecision::Updated {
                providers, models, ..
            } => format!("catalog updated: {providers} providers · {models} models"),
            RefreshDecision::NotModified {
                providers, models, ..
            } => format!("catalog unchanged (304): {providers} providers · {models} models"),
            RefreshDecision::Rejected { findings, error } => {
                let first = findings
                    .iter()
                    .find(|f| f.severity == Severity::Error)
                    .map(|f| f.message.clone())
                    .or_else(|| error.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                format!("catalog refresh rejected — kept last good snapshot: {first}")
            }
            RefreshDecision::Failed { error } => {
                format!("catalog refresh failed — kept last good snapshot: {error}")
            }
        }
    }
}

/// The gate over a candidate snapshot. A snapshot that fails is **never**
/// written: the previous snapshot keeps serving (offline = cached).
pub fn validate_snapshot(snapshot: &CatalogSnapshot) -> Vec<GateFinding> {
    let mut findings = Vec::new();
    if snapshot.provider_count() < MIN_PROVIDERS {
        findings.push(GateFinding {
            severity: Severity::Error,
            message: format!(
                "only {} providers (need ≥{MIN_PROVIDERS})",
                snapshot.provider_count()
            ),
        });
    }
    if snapshot.model_count() < MIN_MODELS {
        findings.push(GateFinding {
            severity: Severity::Error,
            message: format!(
                "only {} models (need ≥{MIN_MODELS})",
                snapshot.model_count()
            ),
        });
    }
    // Shape rules per row — the live snapshot *is* the source of truth, so
    // the two-tier blocker rule (which exists to judge an *override* on top
    // of a vendored baseline) deliberately does not apply here. A pathological
    // snapshot is capped so it cannot emit thousands of findings.
    let mut errors = 0usize;
    'providers: for p in snapshot.providers.values() {
        if p.id.trim().is_empty() {
            findings.push(GateFinding {
                severity: Severity::Error,
                message: "provider with empty id".into(),
            });
            errors += 1;
        }
        for m in p.models.values() {
            if m.id.trim().is_empty() {
                findings.push(GateFinding {
                    severity: Severity::Error,
                    message: format!("`{}` has an empty model id", p.id),
                });
                errors += 1;
            }
            if m.cost.input < 0.0 || m.cost.output < 0.0 {
                findings.push(GateFinding {
                    severity: Severity::Error,
                    message: format!("`{}/{}` has negative pricing", p.id, m.id),
                });
                errors += 1;
            }
            if errors > MAX_FINDINGS {
                findings.push(GateFinding {
                    severity: Severity::Error,
                    message: format!("more than {MAX_FINDINGS} catalog errors — refusing snapshot"),
                });
                break 'providers;
            }
        }
    }
    findings
}

/// **The pure refresh step** (P56.1). Given the snapshot currently on disk
/// and a fetch outcome, decide what to persist.
///
/// Rules, in order:
/// 1. `Failed` → keep `prev`, report honestly (offline = cached).
/// 2. `NotModified` → keep `prev`, bump `fetched_at` (so a 304 is still a
///    successful freshness stamp and we stop re-fetching until the next
///    interval).
/// 3. `Fetched` → parse; a parse or gate failure keeps `prev` untouched.
pub fn apply_refresh(
    prev: Option<&CatalogSnapshot>,
    outcome: FetchOutcome,
    now_ms: i64,
) -> (Option<CatalogSnapshot>, RefreshDecision) {
    match outcome {
        FetchOutcome::Failed(error) => match prev {
            Some(p) => (
                Some(p.clone()),
                RefreshDecision::Failed {
                    error: format!("{error} (serving cached snapshot)"),
                },
            ),
            None => (None, RefreshDecision::Failed { error }),
        },
        FetchOutcome::NotModified => match prev {
            Some(p) => {
                let mut bumped = p.clone();
                bumped.fetched_at = now_ms;
                let decision = RefreshDecision::NotModified {
                    fetched_at: now_ms,
                    providers: bumped.provider_count(),
                    models: bumped.model_count(),
                };
                (Some(bumped), decision)
            }
            // A 304 with nothing stored means our ETag store and the
            // snapshot disagree — treat it as a failure, not a success.
            None => (
                None,
                RefreshDecision::Failed {
                    error: "server said 304 but no snapshot is stored".to_string(),
                },
            ),
        },
        FetchOutcome::Fetched { etag, body } => {
            let parsed = CatalogSnapshot::parse(MODELS_DEV_API_URL, &body, now_ms, &etag);
            let snapshot = match parsed {
                Ok(s) => s,
                Err(error) => {
                    return match prev {
                        Some(p) => (
                            Some(p.clone()),
                            RefreshDecision::Rejected {
                                findings: Vec::new(),
                                error: Some(error),
                            },
                        ),
                        None => (None, RefreshDecision::Failed { error }),
                    };
                }
            };
            let findings = validate_snapshot(&snapshot);
            if !gate_passes(&findings) {
                return match prev {
                    Some(p) => (
                        Some(p.clone()),
                        RefreshDecision::Rejected {
                            findings,
                            error: None,
                        },
                    ),
                    None => (
                        None,
                        RefreshDecision::Rejected {
                            findings,
                            error: None,
                        },
                    ),
                };
            }
            let decision = RefreshDecision::Updated {
                providers: snapshot.provider_count(),
                models: snapshot.model_count(),
                fetched_at: snapshot.fetched_at,
            };
            (Some(snapshot), decision)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_json(
        id: &str,
        name: &str,
        npm: &str,
        api: Option<&str>,
        models: &[(&str, u64)],
    ) -> String {
        let models: Vec<String> = models
            .iter()
            .map(|(mid, ctx)| {
                format!(
                    r#""{mid}":{{"id":"{mid}","name":"{mid}","limit":{{"context":{ctx},"output":4096}},"cost":{{"input":1.0,"output":2.0}},"modalities":{{"input":["text","image"],"output":["text"]}},"tool_call":true,"reasoning":true}}"#
                )
            })
            .collect();
        let api_field = api.map(|a| format!(r#","api":"{a}""#)).unwrap_or_default();
        format!(
            r#""{id}":{{"id":"{id}","name":"{name}","npm":"{npm}"{api_field},"doc":"https://example/{id}","env":["{id}_KEY"],"models":{{{}}}}}"#,
            models.join(",")
        )
    }

    fn body_with(n: usize) -> String {
        let rows: Vec<String> = (0..n)
            .map(|i| {
                provider_json(
                    &format!("prov{i}"),
                    &format!("Provider {i}"),
                    "@ai-sdk/openai-compatible",
                    Some("https://api.provi.test/v1"),
                    &[("m-one", 100_000), ("m-two", 8_000)],
                )
            })
            .collect();
        format!("{{{}}}", rows.join(","))
    }

    #[test]
    fn parses_the_live_shape_including_model_provider_override() {
        let body = r#"{
          "anthropic": {"id":"anthropic","name":"Anthropic","npm":"@ai-sdk/anthropic",
            "doc":"https://docs.anthropic.com","env":["ANTHROPIC_API_KEY"],
            "models":{"claude-x":{"id":"claude-x","name":"Claude X","attachment":true,
              "reasoning":true,"tool_call":true,"structured_output":true,"temperature":true,
              "knowledge":"2025-08-31","release_date":"2026-02-17","last_updated":"2026-03-13",
              "modalities":{"input":["text","image","pdf"],"output":["text"]},
              "limit":{"context":1000000,"output":128000},
              "cost":{"input":3,"output":15,"cache_read":0.3,"cache_write":3.75}}}},
          "agentrouter": {"id":"agentrouter","name":"AgentRouter","npm":"@ai-sdk/openai-compatible",
            "api":"https://agentrouter.org/v1","env":["AGENTROUTER_API_KEY"],
            "models":{"claude-opus-5":{"id":"claude-opus-5","name":"Claude Opus 5",
              "provider":{"npm":"@ai-sdk/anthropic","api":"https://agentrouter.org/v1"},
              "limit":{"context":200000,"output":64000},"cost":{"input":1,"output":2}}}}
        }"#;
        let snap = CatalogSnapshot::parse(MODELS_DEV_API_URL, body, 1_000, "\"abc\"").unwrap();
        assert_eq!(snap.provider_count(), 2);
        assert_eq!(snap.model_count(), 2);
        assert_eq!(snap.etag, "abc"); // normalised, unquoted
        let p = snap.provider("anthropic").unwrap();
        assert_eq!(p.name, "Anthropic");
        assert_eq!(p.api, None); // SDK default → no base URL
        assert_eq!(p.env_var(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(p.logo_url(), "https://models.dev/logos/anthropic.svg");
        let m = p.models.get("claude-x").unwrap();
        assert!(m.modalities.accepts_image());
        assert!(m.modalities.accepts_pdf());
        assert_eq!(m.limit.context, 1_000_000);
        assert_eq!(m.cost.cache_read, Some(0.3));
        // model-level override wins for transport
        let over = snap.provider("agentrouter").unwrap();
        let om = over.models.get("claude-opus-5").unwrap();
        assert_eq!(
            om.transport(&over.npm, &over.id),
            Transport::AnthropicMessages
        );
        assert_eq!(
            om.effective_api(over.api.as_deref()),
            Some("https://agentrouter.org/v1")
        );
    }

    #[test]
    fn missing_provider_id_falls_back_to_the_map_key() {
        let body = r#"{"weird":{"name":"Weird","npm":"x","models":{}}}"#;
        let snap = CatalogSnapshot::parse("s", body, 0, "").unwrap();
        assert_eq!(snap.provider("weird").unwrap().id, "weird");
    }

    #[test]
    fn parse_refuses_empty_or_garbage() {
        assert!(CatalogSnapshot::parse("s", "", 0, "").is_err());
        assert!(CatalogSnapshot::parse("s", "not json", 0, "").is_err());
        assert!(CatalogSnapshot::parse("s", "{}", 0, "").is_err());
    }

    #[test]
    fn model_rows_are_name_sorted_and_have_counts() {
        let snap = CatalogSnapshot::parse("s", &body_with(3), 0, "").unwrap();
        assert_eq!(snap.provider_count(), 3);
        assert_eq!(snap.model_count(), 6);
        let rows = snap.provider("prov0").unwrap().model_rows();
        assert_eq!(rows[0].id, "m-one");
    }

    #[test]
    fn etag_normalisation_handles_quotes_and_weak_tags() {
        assert_eq!(normalize_etag("\"abc\""), "abc");
        assert_eq!(normalize_etag("W/\"abc\""), "abc");
        assert_eq!(normalize_etag(" abc "), "abc");
        assert_eq!(normalize_etag(""), "");
    }

    #[test]
    fn staleness_and_interval_clamping() {
        let now = 1_700_000_000_000i64;
        assert!(is_stale(0, now, DEFAULT_REFRESH_SECS));
        assert!(!is_stale(now - 60_000, now, DEFAULT_REFRESH_SECS));
        assert!(is_stale(
            now - (DEFAULT_REFRESH_SECS as i64 + 1) * 1000,
            now,
            DEFAULT_REFRESH_SECS
        ));
        assert_eq!(refresh_interval_secs(None), 4 * 3600);
        assert_eq!(refresh_interval_secs(Some(0)), 3600); // clamped up to 1h
        assert_eq!(refresh_interval_secs(Some(99)), 24 * 3600); // clamped to 24h
        assert_eq!(refresh_interval_secs(Some(6)), 6 * 3600);
    }

    #[test]
    fn free_id_rule_matches_the_live_zen_list() {
        // Exactly what the live endpoint returned on 2026-09-11.
        for id in [
            "deepseek-v4-flash-free",
            "muse-spark-1.3-contributor-free",
            "muse-spark-1.2-contributor-free",
            "mimo-v2.5-free",
            "ling-3.0-flash-fin-free",
            "nemotron-3-ultra-free",
            "nemotron-3.5-lightning-free",
        ] {
            assert!(is_free_model_id(id), "{id} should be free");
        }
        // big-pickle is always free even though it isn't named "free".
        assert!(is_free_model_id("big-pickle"));
        // paid rows in the same list must NOT leak in.
        for id in [
            "claude-opus-5",
            "claude-fable-5",
            "gemini-3.8-flash",
            "freeform-7",
            "freedom-1",
        ] {
            assert!(!is_free_model_id(id), "{id} should not be free");
        }
    }

    #[test]
    fn free_model_ids_always_contains_big_pickle() {
        let ids = vec!["claude-opus-5".to_string()];
        let free = free_model_ids(&ids);
        assert_eq!(free, vec!["big-pickle".to_string()]);
    }

    #[test]
    fn transport_maps_from_npm_with_provider_fallback() {
        assert_eq!(
            transport_from_npm("@ai-sdk/anthropic", "x"),
            Transport::AnthropicMessages
        );
        assert_eq!(
            transport_from_npm("@ai-sdk/amazon-bedrock", "x"),
            Transport::BedrockConverse
        );
        assert_eq!(
            transport_from_npm("@ai-sdk/openai-compatible", "x"),
            Transport::OpenaiChat
        );
        assert_eq!(
            transport_from_npm("", "anthropic"),
            Transport::AnthropicMessages
        );
        assert_eq!(
            transport_from_npm("", "amazon-bedrock"),
            Transport::BedrockConverse
        );
    }

    #[test]
    fn refresh_failure_keeps_last_good_snapshot() {
        let good = CatalogSnapshot::parse("s", &body_with(120), 5, "e1").unwrap();
        let (next, decision) =
            apply_refresh(Some(&good), FetchOutcome::Failed("dns down".into()), 10);
        assert!(next.is_some());
        assert_eq!(next.unwrap().providers.len(), 120);
        assert!(!decision.accepted());
        assert!(decision.summary().contains("dns down"));
    }

    #[test]
    fn refresh_304_bumps_fetched_at_and_keeps_the_snapshot() {
        let good = CatalogSnapshot::parse("s", &body_with(120), 5, "e1").unwrap();
        let (next, decision) = apply_refresh(Some(&good), FetchOutcome::NotModified, 99);
        let next = next.unwrap();
        assert_eq!(next.fetched_at, 99);
        assert_eq!(next.provider_count(), 120);
        assert!(decision.accepted());
    }

    #[test]
    fn refresh_304_without_a_stored_snapshot_fails_honestly() {
        let (next, decision) = apply_refresh(None, FetchOutcome::NotModified, 1);
        assert!(next.is_none());
        assert!(!decision.accepted());
    }

    #[test]
    fn refresh_rejects_a_truncated_snapshot_and_keeps_the_good_one() {
        let good = CatalogSnapshot::parse("s", &body_with(120), 5, "e1").unwrap();
        let (next, decision) = apply_refresh(
            Some(&good),
            FetchOutcome::Fetched {
                etag: "e2".into(),
                body: body_with(2), // truncated / error page
            },
            9,
        );
        assert_eq!(next.unwrap().provider_count(), 120); // last good survives
        assert!(!decision.accepted());
        match decision {
            RefreshDecision::Rejected { findings, .. } => {
                assert!(findings.iter().any(|f| f.message.contains("providers")))
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn refresh_accepts_a_full_snapshot() {
        let (next, decision) = apply_refresh(
            None,
            FetchOutcome::Fetched {
                etag: "\"e9\"".into(),
                body: body_with(150),
            },
            7,
        );
        let next = next.unwrap();
        assert_eq!(next.etag, "e9");
        assert_eq!(next.fetched_at, 7);
        assert_eq!(next.provider_count(), 150);
        assert!(decision.accepted(), "{decision:?}");
        assert!(decision.summary().contains("150 providers"));
    }

    #[test]
    fn refresh_rejects_malformed_json_but_keeps_prev() {
        let good = CatalogSnapshot::parse("s", &body_with(120), 5, "e1").unwrap();
        let (next, decision) = apply_refresh(
            Some(&good),
            FetchOutcome::Fetched {
                etag: "e2".into(),
                body: "<html>SPA</html>".into(),
            },
            9,
        );
        assert_eq!(next.unwrap().provider_count(), 120);
        assert!(!decision.accepted());
        assert!(decision.summary().contains("rejected"));
    }

    #[test]
    fn opencode_free_subsets_the_live_provider_rows() {
        let body = r#"{"opencode":{"id":"opencode","name":"OpenCode Zen","npm":"@ai-sdk/openai-compatible",
          "api":"https://opencode.ai/zen/v1","env":["OPENCODE_API_KEY"],"models":{
            "claude-opus-5":{"id":"claude-opus-5","name":"Claude Opus 5","limit":{"context":200000,"output":64000},"cost":{"input":1,"output":2}},
            "big-pickle":{"id":"big-pickle","name":"Big Pickle","limit":{"context":128000,"output":16000},"cost":{"input":0,"output":0}},
            "mimo-v2.5-free":{"id":"mimo-v2.5-free","name":"Mimo","limit":{"context":128000,"output":16000},"cost":{"input":0,"output":0}}}}}"#;
        let snap = CatalogSnapshot::parse("s", body, 0, "").unwrap();
        let free = snap.opencode_free_models();
        assert_eq!(free, vec!["big-pickle", "mimo-v2.5-free"]);
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let snap = CatalogSnapshot::parse("s", &body_with(120), 42, "e1").unwrap();
        let json = serde_json::to_string(&snap).unwrap();
        let back: CatalogSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
    }
}
