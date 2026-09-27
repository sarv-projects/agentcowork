//! P44.1–P44.3 — Provider Record + alias + OpenAI-compatible profiles
//! (A11, doc 66 + the Hermes `HERMES_OVERLAYS`/`ALIASES` + OpenCode
//! provider-directory steals).
//!
//! Three pieces, mirroring the catalog crate's "pure + testable" discipline
//! (network/registry access is the caller's job — every seam here is
//! injected):
//!
//! 1. **`ProviderRecord`** — provider identity as a *merged* fact, not a
//!    hardcoded `if provider == …` branch. Resolution order (models.dev →
//!    overlay → user config → plugin profile) is `ProviderRegistry::register`,
//!    where later layers override earlier ones field-by-field.
//! 2. **Alias normalization** — human/legacy names → canonical provider ids
//!    (the Hermes `ALIASES` pattern: claude→anthropic, kimi/moonshot→
//!    kimi-for-coding, glm/z-ai/zhipu→zai, nim/nvidia-nim→nvidia, …).
//! 3. **The OpenAI-compatible profile table** — one transport, many
//!    profiles (OpenCode pattern): each profile is metadata + auth only,
//!    never a bespoke adapter; a user can override any base URL.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The wire-protocol family a provider speaks (the API-mode mapping that
/// decides the protocol adapter). Mirrors the model-provider transports in
/// Hermes' `providers.py` and the AI-SDK families in OpenCode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// `openai-chat`-compatible `/chat/completions` (the largest family:
    /// most BYOK + aggregator providers share this contract).
    OpenaiChat,
    /// Anthropic Messages (`/v1/messages`).
    AnthropicMessages,
    /// The OpenAI Responses/Codex API (`/v1/responses`).
    CodexResponses,
    /// AWS Bedrock Converse.
    BedrockConverse,
}

/// How a provider authenticates. P44.1 keeps this *typed* so the vault/auth
/// bridge can resolve `api_key_env → opaque handle` in Rust only; it never
/// stores the key itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    /// A plain API key, read from the first env var that is set.
    ApiKey,
    /// The key comes from a named env var (the vault handle lives there).
    ApiKeyEnv,
    /// Device OAuth flow (e.g. `nous`, `qwen-oauth`).
    OAuthDeviceCode,
    /// External OAuth (browser) — tokens held server-side/OS, we hold the
    /// refresh grant only.
    OAuthExternal,
    /// An external authenticated process endpoint (e.g. `acp://copilot`).
    ExternalProcess,
    /// AWS SDK credentials (Bedrock).
    AwsSdk,
    /// Google Vertex service account.
    Vertex,
    /// No key needed (`opencode-free` keyless).
    Keyless,
}

/// Whether a provider is a genuine aggregator (OpenRouter-style passthrough
/// of other providers' models) vs a routing-aggregator (OpenCode-Zen /
/// OpenCode-Go flat-namespace reseller). Matters for pricing/ownership:
/// an aggregator's `base_model` rows resolve through the canonical lab set;
/// a reseller's rows belong to it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AggregatorKind {
    /// OpenRouter-style passthrough aggregator.
    IsAggregator,
    /// OpenCode-Zen/Go-style flat-namespace reseller (owns its model rows).
    IsRoutingAggregator,
}

/// Where a provider's identity came from (resolution-order provenance — the
/// A11 `source` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverySource {
    /// The live models.dev catalog (the canonical base).
    ModelsDev,
    /// A Hermes-style overlay on top of models.dev.
    #[default]
    Overlay,
    /// The user's own config (`<data_dir>/providers/`).
    UserConfig,
    /// A provider plugin profile (`plugins/model-providers/<name>/`
    /// equivalent — user plugin overrides bundled providers).
    PluginProfile,
}

/// The full provider identity (A11, P44.1). Everything here is metadata +
/// auth-*shape* — never the credential itself (keys live in the vault and
/// resolve in Rust only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderRecord {
    /// Canonical provider id (everything aliases to this).
    pub id: String,
    /// Human/legacy names that resolve to `id` (P44.2).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Wire-protocol family.
    #[serde(default)]
    pub transport: Option<Transport>,
    /// Authentication shape.
    pub auth: Auth,
    /// Env-var handle(s) the vault reads the key from (first-set wins).
    #[serde(default)]
    pub api_key_env: Vec<String>,
    /// Explicit base URL (when non-probe-able / a known endpoint).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Environment override for the base URL (`base_url_env`), resolved only
    /// at use-time in the caller (Rust) layer.
    #[serde(default)]
    pub base_url_env: Option<String>,
    /// Aggregator classification.
    #[serde(default)]
    pub aggregator: Option<AggregatorKind>,
    /// Where the model rows come from (catalog, plugin, local runtime…).
    #[serde(default)]
    pub models_source: String,
    /// Capability set this provider is known to support (verified later by
    /// P44.4 probes).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Resolution-order provenance.
    #[serde(default)]
    pub source: DiscoverySource,
    /// `source_version` (catalog snapshot / plugin version).
    #[serde(default)]
    pub source_version: String,
    /// `capabilities_verified_at` — populated only by a live P44.4 probe;
    /// absent means "catalog metadata, not yet verified".
    #[serde(default)]
    pub capabilities_verified_at: Option<String>,
    /// The last P44.4 verification report (the *observed* truth routing may
    /// rely on). `hard_caps_verified = false` → the provider is unverified for
    /// at least one advertised hard capability; routing must not rely on it.
    #[serde(default)]
    pub verified_report: Option<crate::probe::VerificationReport>,
    /// A `config_hash` of the resolved identity (so config-driven refreshes
    /// can detect change without comparing the whole record).
    #[serde(default)]
    pub config_hash: String,
}

impl ProviderRecord {
    /// Merge a higher-precedence layer (`src`) into this record (P44.1
    /// resolution-order step): later layers override earlier ones per-field;
    /// never a wholesale replace.
    pub fn merge_from(&mut self, src: &ProviderRecord, source: DiscoverySource) {
        // Aliases are additive — a user layer may add names, not clear them.
        for a in &src.aliases {
            if !self.aliases.contains(a) {
                self.aliases.push(a.clone());
            }
        }
        if !src.name.is_empty() {
            self.name.clone_from(&src.name);
        }
        if src.transport.is_some() {
            self.transport = src.transport;
        }
        // Auth is a shape, always merged from the higher-precedence layer.
        self.auth = src.auth;
        if src.api_key_env.iter().any(|v| !v.is_empty()) {
            self.api_key_env.clone_from(&src.api_key_env);
        }
        if src.base_url.is_some() {
            self.base_url = src.base_url.clone();
        }
        if src.base_url_env.is_some() {
            self.base_url_env = src.base_url_env.clone();
        }
        if src.aggregator.is_some() {
            self.aggregator = src.aggregator;
        }
        if !src.models_source.is_empty() {
            self.models_source.clone_from(&src.models_source);
        }
        if !src.capabilities.is_empty() {
            self.capabilities = src.capabilities.clone();
        }
        // Provenance always takes the higher-precedence source, and the
        // *newest* layer wins the version stamp (a plugin profile bumps it).
        self.source = source;
        if !src.source_version.is_empty() {
            self.source_version.clone_from(&src.source_version);
        }
        // config_hash is recomputed by the caller after a merge chain.
    }

    /// The effective base URL: env-var resolution is the caller's job, so
    /// here we just pick the explicit endpoint or the env *name* (never the
    /// secret). Honest — a None means "probe / require the env override".
    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url.as_deref().or(self.base_url_env.as_deref())
    }
}

// Deliberate: the default is `ApiKeyEnv`, not the first variant (`ApiKey`),
// so `#[derive(Default)]` would change semantics if the enum is reordered.
#[allow(clippy::derivable_impls)]
impl Default for Auth {
    fn default() -> Self {
        // Providers that don't declare auth still get a sane typed shape so
        // `ProviderRecord` is `Default`-constructible: an API-key-env shape.
        Auth::ApiKeyEnv
    }
}

/// Default auth for a transport family — providers that don't declare one
/// still get a sane typed shape (an `ApiKeyEnv` default rather than none).
impl From<Transport> for Auth {
    fn from(t: Transport) -> Self {
        match t {
            Transport::BedrockConverse => Auth::AwsSdk,
            _ => Auth::ApiKeyEnv,
        }
    }
}

/// P44.1 — the merged, resolution-ordered registry.
///
/// Resolution order: models.dev catalog → overlay → user config → plugin
/// profile. The same provider id is merged across layers; the highest
/// precedence layer sets provenance. Secrets never appear here — this is
/// identity metadata only.
#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    by_id: BTreeMap<String, ProviderRecord>,
    by_alias: BTreeMap<String, String>, // normalized alias → canonical id
}

impl ProviderRegistry {
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn all(&self) -> impl Iterator<Item = &ProviderRecord> {
        self.by_id.values()
    }

    /// Register a layer at the given precedence (P44.1 resolution order).
    /// Later/outer calls for the same id override earlier field-by-field and
    /// re-stamp provenance. Rebuilds the alias index at the end.
    pub fn register(&mut self, rec: ProviderRecord) {
        let id = rec.id.clone();
        let source = rec.source;
        match self.by_id.get_mut(&id) {
            Some(existing) => existing.merge_from(&rec, source),
            None => {
                self.by_id.insert(id.clone(), rec);
            }
        }
        self.rebuild_alias_index();
    }

    /// Resolve any alias or canonical id → the canonical `ProviderRecord`.
    /// Case/format-normalized before lookup (P44.2).
    pub fn resolve(&self, name: &str) -> Option<&ProviderRecord> {
        let norm = normalize(name);
        self.by_id.get(&norm).or_else(|| {
            let canon = self.by_alias.get(&norm)?;
            self.by_id.get(canon)
        })
    }

    /// Get by canonical id only (no alias pass) — the router's fast path.
    pub fn get(&self, canon: &str) -> Option<&ProviderRecord> {
        self.by_id.get(&normalize(canon))
    }

    /// The canonical id a name resolves to (picker → config normalization).
    pub fn canonical_id(&self, name: &str) -> Option<&str> {
        let norm = normalize(name);
        if self.by_id.contains_key(&norm) {
            return Some(self.by_id.get(&norm).map(|r| r.id.as_str()).unwrap_or(""));
        }
        self.by_alias.get(&norm).map(|s| s.as_str())
    }

    /// Mark the provider's capabilities as verified by a live probe (P44.4
    /// write-back). Accepts any alias or canonical id. Returns true if
    /// anything changed (routes re-rank).
    pub fn mark_verified(&mut self, canon: &str, verified_at: &str) -> bool {
        let norm = normalize(canon);
        let Some(id) = self
            .by_id
            .contains_key(&norm)
            .then_some(norm.as_str())
            .map(str::to_owned)
            .or_else(|| self.by_alias.get(&norm).cloned())
        else {
            return false;
        };
        match self.by_id.get_mut(&id) {
            Some(r) if r.capabilities_verified_at.as_deref() != Some(verified_at) => {
                r.capabilities_verified_at = Some(verified_at.to_string());
                true
            }
            _ => false,
        }
    }

    /// **P44.4 — apply a live capability probe result** (alias-aware).
    ///
    /// Runs [`crate::probe::verify_report`] over the provider's advertised
    /// hard capabilities (from its catalog/transport metadata) vs. the
    /// observed probe facts, stores the report + stamps
    /// `capabilities_verified_at`, and returns whether the provider is now
    /// *fully* verified (no advertised hard capability was left unconfirmed).
    ///
    /// Returns `None` when `canon`/alias does not resolve. Unknown providers
    /// can be probed by first registering a `ProviderRecord`.
    pub fn apply_probe(
        &mut self,
        canon: &str,
        observed: &crate::probe::ProbeResult,
        verified_at: &str,
    ) -> Option<bool> {
        let norm = normalize(canon);
        let id = self
            .by_id
            .contains_key(&norm)
            .then_some(norm.as_str())
            .map(str::to_owned)
            .or_else(|| self.by_alias.get(&norm).cloned())?;
        let rec = self.by_id.get(&id)?;
        let advertised = crate::probe::AdvertisedHardCaps {
            tools: rec
                .capabilities
                .iter()
                .any(|c| c.eq_ignore_ascii_case("tools") || c.eq_ignore_ascii_case("tool_calling")),
            structured_output: rec.capabilities.iter().any(|c| {
                c.eq_ignore_ascii_case("structured_output")
                    || c.eq_ignore_ascii_case("structured_outputs")
            }),
            codex_responses: rec.transport == Some(Transport::CodexResponses),
        };
        let report = crate::probe::verify_report(&advertised, observed);
        let fully = report.is_fully_verified();
        if let Some(r) = self.by_id.get_mut(&id) {
            r.verified_report = Some(report);
            r.capabilities_verified_at = Some(verified_at.to_string());
        }
        Some(fully)
    }

    /// The last verification stamp for a provider, if any (alias-aware fast
    /// read used by routing — a missing value means "unverified").
    pub fn verified_at(&self, canon: &str) -> Option<&str> {
        self.resolve(canon)?.capabilities_verified_at.as_deref()
    }

    fn rebuild_alias_index(&mut self) {
        self.by_alias.clear();
        for (id, rec) in &self.by_id {
            self.by_id.values().filter(|r| r.id == *id).for_each(|r| {
                for a in &r.aliases {
                    self.by_alias
                        .entry(normalize(a))
                        .or_insert_with(|| id.clone());
                }
            });
            let _ = rec;
        }
    }
}

/// P44.2 — normalize a name for alias lookup (case + common separators).
/// `Claude-Code`, `claude_code`, `CLAUDE` all become `claude-code`.
pub fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

/// The canonical alias map (Hermes `ALIASES` steal, P44.2). Each entry maps
/// a human/legacy/provider-ecosystem name → the canonical id the rest of the
/// system speaks. This is the *default* set; user config + plugin profiles
/// add to it via `ProviderRecord.aliases`.
pub const ALIASES: &[(&str, &str)] = &[
    // Anthropic
    ("claude", "anthropic"),
    ("claude-code", "anthropic"),
    ("claude-agent", "anthropic"),
    // OpenAI / Codex
    ("chatgpt", "openai"),
    ("codex", "openai-api"),
    ("openai-codex", "openai-codex"),
    // Zhipu / Z.ai (zai)
    ("glm", "zai"),
    ("z-ai", "zai"),
    ("zhipu", "zai"),
    // Moonshot (kimi)
    ("kimi", "kimi-for-coding"),
    ("moonshot", "kimi-for-coding"),
    ("moonshotai", "moonshotai"),
    // NVIDIA (nim)
    ("nim", "nvidia"),
    ("nvidia-nim", "nvidia"),
    ("nemotron", "nvidia"),
    // Alibaba (dashscope / qwen)
    ("dashscope", "alibaba"),
    ("aliyun", "alibaba"),
    ("qwen", "alibaba"),
    // Bedrock (aws)
    ("aws", "amazon-bedrock"),
    ("bedrock", "amazon-bedrock"),
    ("vertex", "google-vertex"),
    ("gcp", "google-vertex"),
    // Legacy profile ids → vendored models.dev canonical.
    ("novita", "novita-ai"),
    ("together", "togetherai"),
    // Hugging Face
    ("hf", "huggingface"),
    ("hugging-face", "huggingface"),
    // Vercel AI Gateway
    ("ai-gateway", "vercel"),
    ("aigateway", "vercel"),
    // OpenCode
    ("zen", "opencode"),
    ("opencode-free", "opencode-free"),
    // xAI
    ("grok", "xai"),
    ("x-ai", "xai"),
    // Local runtimes
    ("llamacpp", "local"),
    ("lmstudio", "local"),
    ("ollama", "local"),
];

/// P44.3 — the OpenAI-compatible profile table (OpenCode pattern): one
/// `OpenaiChat` transport, many endpoint/auth profiles. Each entry is
/// metadata + auth-*shape* only — never a bespoke adapter. The known base
/// URL is the reference endpoint; every one is user-overridable (`base_url`
/// / `base_url_env`), so a proxy/private/local deployment just points at
/// itself.
///
/// Verified against the live OpenCode provider directory + Hermes long-tail
/// (`HERMES_OVERLAYS`): the reference base URLs below are the current
/// canonical endpoints.
pub const OPENAI_COMPATIBLE_PROFILES: &[(&str, &str, &str)] = &[
    ("baseten", "Baseten", "https://inference.baseten.co/v1"),
    ("cerebras", "Cerebras", "https://api.cerebras.ai/v1"),
    (
        "deepinfra",
        "Deep Infra",
        "https://api.deepinfra.com/v1/openai",
    ),
    ("deepseek", "DeepSeek", "https://api.deepseek.com/v1"),
    (
        "fireworks",
        "Fireworks AI",
        "https://api.fireworks.ai/inference/v1",
    ),
    ("groq", "Groq", "https://api.groq.com/openai/v1"),
    ("openrouter", "OpenRouter", "https://openrouter.ai/api/v1"),
    ("togetherai", "Together AI", "https://api.together.xyz/v1"),
    ("xai", "xAI", "https://api.x.ai/v1"),
    // the longer Hermes long-tail (also OpenAI-compatible)
    ("novita-ai", "Novita AI", "https://api.novita.ai/v3/openai"),
    ("stepfun", "StepFun", "https://api.stepfun.ai/v1"),
    ("perplexity", "Perplexity", "https://api.perplexity.ai"),
    ("mistral", "Mistral", "https://api.mistral.ai/v1"),
];

impl ProviderRegistry {
    fn lookup_or_default(&mut self, id: String) -> ProviderRecord {
        self.by_id
            .get(&id)
            .cloned()
            .unwrap_or_else(|| ProviderRecord {
                auth: Auth::ApiKeyEnv,
                id: id.clone(),
                ..Default::default()
            })
    }

    fn insert_for_seed(&mut self, rec: ProviderRecord) {
        let source = rec.source;
        match self.by_id.get_mut(&rec.id) {
            Some(existing) => existing.merge_from(&rec, source),
            None => {
                self.by_id.insert(rec.id.clone(), rec);
            }
        }
    }
}

/// Build the base registry: canonical provider ids + the alias map + the
/// OpenAI-compatible profile table. Callers layer overlay / user-config /
/// plugin-profile `ProviderRecord`s on top via [`ProviderRegistry::register`],
/// then `finalize()` before use.
pub fn base_registry() -> ProviderRegistry {
    let mut r = ProviderRegistry::default();

    // 1. Canonical alias layer (adds `aliases` to each canonical id).
    for (alias, canon) in ALIASES {
        let mut rec = r.lookup_or_default((*canon).to_string());
        if !rec.aliases.contains(&alias.to_string()) {
            rec.aliases.push(alias.to_string());
        }
        r.insert_for_seed(rec);
    }

    // 2. VENDORED models.dev provider directory (P44.3) — all 212 providers
    //    (id · name · api-key env · base URL · auth shape · transport family),
    //    the authoritative set OpenCode/Hermes consume. Merged field-by-field
    //    onto the alias layer; NEVER a key, only the env-var NAME.
    for row in crate::provider_seed::PROVIDER_SEED {
        let mut rec = r.lookup_or_default(row.id.to_string());
        rec.name = row.name.to_string();
        rec.transport = Some(row.transport);
        rec.auth = row.auth;
        if rec.base_url.is_none() && !row.base_url.is_empty() {
            rec.base_url = Some(row.base_url.to_string());
        }
        for env in row.env {
            if !rec.api_key_env.contains(&env.to_string()) {
                rec.api_key_env.push(env.to_string());
            }
        }
        rec.source = DiscoverySource::ModelsDev;
        r.insert_for_seed(rec);
    }

    // 3. Overlay corrections the flat models.dev directory doesn't capture:
    //    distinct wire transports + auth shapes for a few first-party ids, and
    //    the local keyless runtime. These override the vendored row per-field.
    let overlay = [
        (
            "anthropic",
            "Anthropic",
            Transport::AnthropicMessages,
            Auth::ApiKeyEnv,
        ),
        ("openai", "OpenAI", Transport::OpenaiChat, Auth::ApiKeyEnv),
        (
            "openai-api",
            "OpenAI API (Responses/Codex)",
            Transport::CodexResponses,
            Auth::ApiKeyEnv,
        ),
        (
            "amazon-bedrock",
            "Amazon Bedrock",
            Transport::BedrockConverse,
            Auth::AwsSdk,
        ),
        (
            "google-vertex",
            "Google Vertex",
            Transport::OpenaiChat,
            Auth::Vertex,
        ),
        ("nous", "Nous", Transport::OpenaiChat, Auth::OAuthDeviceCode),
        (
            "opencode",
            "OpenCode Zen",
            Transport::OpenaiChat,
            Auth::ApiKeyEnv,
        ),
        (
            "opencode-free",
            "OpenCode Free",
            Transport::OpenaiChat,
            Auth::Keyless,
        ),
        (
            "local",
            "Local runtime",
            Transport::OpenaiChat,
            Auth::Keyless,
        ),
    ];
    for (id, name, transport, auth) in overlay {
        let mut rec = r.lookup_or_default(id.to_string());
        rec.name = name.to_string();
        rec.transport = Some(transport);
        rec.auth = auth;
        // `openai-api` shares OpenAI's key; `local` needs no key.
        if id == "openai-api" && !rec.api_key_env.iter().any(|e| e == "OPENAI_API_KEY") {
            rec.api_key_env.push("OPENAI_API_KEY".to_string());
        }
        rec.source = DiscoverySource::Overlay;
        r.insert_for_seed(rec);
    }

    // Aggregator classification (used so the enum is real, and so routing
    // treats these specially): OpenRouter is a passthrough aggregator;
    // OpenCode Zen/Go is a flat-namespace reseller.
    if let Some(rec) = r.by_id.get_mut("openrouter") {
        rec.aggregator = Some(AggregatorKind::IsAggregator);
    }
    if let Some(rec) = r.by_id.get_mut("opencode") {
        rec.aggregator = Some(AggregatorKind::IsRoutingAggregator);
    }
    if let Some(rec) = r.by_id.get_mut("opencode-go") {
        rec.aggregator = Some(AggregatorKind::IsRoutingAggregator);
    }

    r.finalize();
    r
}

impl ProviderRegistry {
    /// Rebuild the alias index — after seeding or bulk-load.
    pub fn finalize(&mut self) {
        self.rebuild_alias_index();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_case_and_separators() {
        assert_eq!(normalize("Claude-Code"), "claude-code");
        assert_eq!(normalize("claude_code"), "claude-code");
        assert_eq!(normalize("  CLAUDE  "), "claude");
    }

    #[test]
    fn resolution_order_merges_field_by_field() {
        let mut r = ProviderRegistry::default();
        // models.dev layer
        r.register(ProviderRecord {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            transport: Some(Transport::OpenaiChat),
            auth: Auth::ApiKeyEnv,
            base_url: Some("https://api.deepseek.com/v1".into()),
            source: DiscoverySource::ModelsDev,
            ..Default::default()
        });
        // user config overrides the name only
        r.register(ProviderRecord {
            id: "deepseek".into(),
            name: "My DeepSeek".into(),
            source: DiscoverySource::UserConfig,
            ..Default::default()
        });
        let rec = r.get("deepseek").unwrap();
        assert_eq!(rec.name, "My DeepSeek");
        // transport + base_url survive the merge (field-by-field, not replace)
        assert_eq!(rec.transport, Some(Transport::OpenaiChat));
        assert_eq!(rec.base_url.as_deref(), Some("https://api.deepseek.com/v1"));
        assert_eq!(rec.source, DiscoverySource::UserConfig);
    }

    #[test]
    fn aliases_resolve_to_canonical_id() {
        let r = base_registry();
        assert_eq!(r.canonical_id("claude"), Some("anthropic"));
        assert_eq!(r.canonical_id("Claude"), Some("anthropic"));
        assert_eq!(r.canonical_id("glm"), Some("zai"));
        assert_eq!(r.canonical_id("moonshot"), Some("kimi-for-coding"));
        assert_eq!(r.canonical_id("nim"), Some("nvidia"));
        assert_eq!(r.canonical_id("qwen"), Some("alibaba"));
        assert_eq!(r.canonical_id("Amazon-Bedrock"), Some("amazon-bedrock"));
        assert_eq!(
            r.resolve("claude").map(|p| p.id.as_str()),
            Some("anthropic")
        );
    }

    #[test]
    fn profile_table_has_transport_and_reference_url() {
        let r = base_registry();
        // The vendored models.dev directory is the source of truth: every
        // seeded provider has a transport + an api-key env handle (or is a
        // keyless local/free runtime). base_url is optional (SDK-default
        // providers carry none in models.dev).
        for row in crate::provider_seed::PROVIDER_SEED {
            let rec = r
                .get(row.id)
                .unwrap_or_else(|| panic!("missing {}", row.id));
            assert!(rec.transport.is_some(), "{} has a transport", row.id);
            let keyless = matches!(rec.auth, Auth::Keyless | Auth::AwsSdk | Auth::Vertex);
            assert!(
                keyless || !rec.api_key_env.is_empty(),
                "{} has an auth env handle or is keyless",
                row.id
            );
        }
    }

    #[test]
    fn user_base_url_override_wins() {
        let mut rr = base_registry();
        rr.register(ProviderRecord {
            id: "groq".into(),
            base_url: Some("http://127.0.0.1:8080/v1".into()),
            base_url_env: Some("GROQ_BASE_URL".into()),
            source: DiscoverySource::UserConfig,
            ..Default::default()
        });
        assert_eq!(
            rr.resolve("groq").unwrap().effective_base_url(),
            Some("http://127.0.0.1:8080/v1")
        );
    }

    #[test]
    fn mark_verified_flips_only_when_changed() {
        let mut r = base_registry();
        assert!(r.mark_verified("anthropic", "t0"));
        assert!(!r.mark_verified("anthropic", "t0"));
        assert!(r.mark_verified("claude", "t1")); // alias resolves
        assert_eq!(
            r.get("anthropic")
                .unwrap()
                .capabilities_verified_at
                .as_deref(),
            Some("t1")
        );
    }

    #[test]
    fn distinct_transports_registered() {
        let r = base_registry();
        assert_eq!(
            r.get("anthropic").unwrap().transport,
            Some(Transport::AnthropicMessages)
        );
        assert_eq!(
            r.get("amazon-bedrock").unwrap().transport,
            Some(Transport::BedrockConverse)
        );
        assert_eq!(r.get("google-vertex").unwrap().auth, Auth::Vertex);
        assert_eq!(r.get("opencode-free").unwrap().auth, Auth::Keyless);

        // The full vendored models.dev directory is present (200+ providers),
        // including the ones OpenCode/Hermes expose.
        assert!(r.len() >= 200, "vendored registry too small: {}", r.len());
        for canonical in [
            "anthropic",
            "openai",
            "openai-api",
            "amazon-bedrock",
            "google-vertex",
            "nous",
            "kimi-for-coding",
            "zai",
            "zai-coding-plan",
            "nvidia",
            "alibaba",
            "huggingface",
            "vercel",
            "local",
            "xai",
            "xiaomi",
            "xiaomi-token-plan-cn",
            "zenmux",
            "zenifra",
            "zhipuai",
            "zhipuai-coding-plan",
            "xpersona",
            "zeldoc",
            "perplexity",
            "deepseek",
            "groq",
            "openrouter",
            "mistral",
        ] {
            assert!(r.get(canonical).is_some(), "missing provider `{canonical}`");
        }
    }
}
