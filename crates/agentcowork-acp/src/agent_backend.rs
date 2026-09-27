//! P63 — **per-agent model-backend configuration** (the "configure the
//! agent's own provider from the cockpit" matrix).
//!
//! Every external agent accepts its model configuration on one of a small
//! number of channels. This module names them and owns the *pure* decision of
//! which environment variables a spawn should carry — the filesystem, the
//! vault read, and the audit event belong to the caller (the Tauri layer).
//!
//! # Why env and not the agent's config file
//!
//! Injecting env at spawn is strictly safer than writing the agent's config:
//! nothing touches disk, the override lasts exactly one child process, and it
//! is reversible by simply not injecting. Two consequences follow:
//!
//! - Agents whose provider *routing* lives in a config file
//!   ([`BackendChannel::ConfigFileOnly`] — Codex's `[model_providers]`, Cline's
//!   `providers.json`) cannot be configured by env alone. Those are the
//!   protected-path write case (spec §6 #21 / TODO P47.7) and this module
//!   refuses rather than pretending.
//! - [`BackendChannel::FixedEnv`] agents (Claude Code's `ANTHROPIC_*`) take a
//!   *fixed* set of names no matter which provider is behind them — that is the
//!   cc-switch "point Claude Code at my endpoint" case, and it works without
//!   ever editing `~/.claude/settings.json`.
//!
//! # What is deliberately absent
//!
//! No secret-bearing planner API exists here. [`ProviderBinding`] carries
//! non-secret metadata only, and [`plan_env`] can emit only reviewed
//! non-secret child variables. The Tauri layer never sends values back over
//! IPC — only the list of variable *names* that can be injected (for the UI's
//! honesty line). External-agent authentication remains agent-owned.
//!
//! # Provenance of the provider-side names
//!
//! The provider half of the mapping is **not** invented here: it comes from
//! the vendored models.dev directory (`agentcowork-catalog`), whose whole
//! premise is that agent CLIs read these env vars. Callers pass the resolved
//! names in via [`ProviderBinding`], so this crate keeps no catalog
//! dependency and stays unit-testable.

use serde::{Deserialize, Serialize};

/// How an external agent accepts its model configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChannel {
    /// Reads the **provider's own** env vars (`ANTHROPIC_API_KEY`,
    /// `OPENAI_API_KEY`, …) — the models.dev naming. Agents built on the AI
    /// SDK / LiteLLM convention land here.
    ProviderEnv,
    /// Reads a **fixed** set of env names regardless of which provider sits
    /// behind them (Claude Code's `ANTHROPIC_*` triple).
    FixedEnv,
    /// Configured **only** through its own config file; env injection is not
    /// honored. Writing it is the protected-path case (P47.7, post-v1).
    ConfigFileOnly,
    /// Brings its own subscription login. There is nothing for us to
    /// configure, and we must never touch its credentials
    /// (`ARCH/06` §6.x rule 4).
    Subscription,
    /// Not yet verified against the agent's own docs/behaviour. Reported as
    /// unknown rather than guessed — the UI must not offer a control we
    /// cannot honour.
    Unknown,
}

impl BackendChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            BackendChannel::ProviderEnv => "provider_env",
            BackendChannel::FixedEnv => "fixed_env",
            BackendChannel::ConfigFileOnly => "config_file",
            BackendChannel::Subscription => "subscription",
            BackendChannel::Unknown => "unknown",
        }
    }

    /// Can we point this agent at a provider by injecting env at spawn?
    pub fn is_env_injectable(&self) -> bool {
        matches!(self, BackendChannel::ProviderEnv | BackendChannel::FixedEnv)
    }
}

/// One agent's backend contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentBackendSpec {
    /// Registry id (`claude`, `opencode`, …).
    pub agent_id: &'static str,
    pub channel: BackendChannel,
    /// `FixedEnv`: the env names to write, in order. First entry is the API-key
    /// var. Unused for other channels.
    pub api_key_env: &'static [&'static str],
    /// `FixedEnv`: the base-URL var. `None` when the agent has no override.
    pub base_url_env: Option<&'static str>,
    /// `FixedEnv`: the model var.
    pub model_env: Option<&'static str>,
    /// Informational: the file this agent reads (the Tier-3 target). Shown in
    /// the UI so the user knows where a persistent change *would* go.
    pub config_file: Option<&'static str>,
    /// One honest sentence for the UI.
    pub note: &'static str,
}

/// What the caller resolved for one `(agent, provider, model)` binding.
///
/// This type is deliberately metadata-only. It has no field that can carry a
/// vault value, so even a future caller cannot turn the compatibility planner
/// into a secret-injection path. Authentication is owned by the external
/// agent and is established through its own sign-in/configuration flow.
#[derive(Clone, Copy)]
pub struct ProviderBinding<'a> {
    /// Catalog provider id (`anthropic`, `openai`, `deepseek`, …).
    pub provider: &'a str,
    /// Model id, forwarded only when the agent has a model env var.
    pub model: &'a str,
    /// The provider's API-key env name from the catalog
    /// (`ProviderRecord::api_key_env[0]`). It is metadata only and is never
    /// emitted by this module.
    pub key_env: &'a str,
    /// The provider's base URL, if known. It is validated before it can be
    /// returned as a child variable.
    pub base_url: Option<&'a str>,
    /// The provider's own base-URL override env name
    /// (`ProviderRecord::base_url_env`), if it declares one.
    pub base_url_env: Option<&'a str>,
}

impl std::fmt::Debug for ProviderBinding<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderBinding")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("key_env", &self.key_env)
            .field("base_url", &self.base_url.as_deref().map(|_| "<redacted>"))
            .field("base_url_env", &self.base_url_env)
            .finish()
    }
}

/// A safe, typed explanation for a rejected custom endpoint.
///
/// The variants intentionally contain no raw URL or user input. This keeps
/// errors safe to put in IPC, audit records, and logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseUrlError {
    /// The input was empty, malformed, or contained ambiguous whitespace.
    Malformed,
    /// Only `http` and `https` may be used for a model endpoint.
    UnsafeScheme,
    /// Userinfo (`user:password@host`) is never accepted.
    UserInfo,
    /// Query strings and fragments are not valid base-URL components here.
    QueryOrFragment,
    /// The parsed host is private, link-local/metadata, reserved, or another
    /// destination refused by the canonical Guard netfloor.
    PrivateDestination,
    /// The host spelling is syntactically ambiguous (for example a trailing
    /// dot or an empty label).
    AmbiguousHost,
}

impl std::fmt::Display for BaseUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Malformed => "malformed base URL",
            Self::UnsafeScheme => "unsafe URL scheme",
            Self::UserInfo => "userinfo is not allowed",
            Self::QueryOrFragment => "query and fragment components are not allowed",
            Self::PrivateDestination => "private or metadata destination is not allowed",
            Self::AmbiguousHost => "ambiguous URL host",
        };
        f.write_str(message)
    }
}

impl std::error::Error for BaseUrlError {}

fn authority_contains_userinfo(raw: &str) -> bool {
    let Some(scheme_end) = raw.find("://").map(|index| index + 3) else {
        return false;
    };
    let rest = &raw[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..authority_end].contains('@')
}

fn raw_authority_host(raw: &str) -> Option<&str> {
    let scheme_end = raw.find("://").map(|index| index + 3)?;
    let rest = &raw[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest[..authority_end].split('@').next_back()?;
    if authority.starts_with('[') {
        let close = authority.find(']')?;
        return Some(&authority[..=close]);
    }
    Some(authority.split(':').next().unwrap_or(authority))
}

fn valid_domain_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn looks_like_noncanonical_ip_literal(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    (lower.starts_with("0x") || lower.starts_with('0'))
        && lower
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'x' || byte == b'.')
        || (lower.bytes().all(|byte| byte.is_ascii_digit()) && !lower.is_empty())
}

/// Parse, canonicalize, and apply the existing Guard URL/netfloor policy to a
/// model endpoint.
///
/// The returned string is the only URL form the backend planner can emit. No
/// caller-provided spelling is retained on an error path. Loopback remains
/// allowed by the desktop netfloor default for local model runtimes; private,
/// link-local/metadata, CGNAT, local-discovery, multicast, unspecified, and
/// reserved destinations are refused.
pub fn validate_base_url(raw: &str) -> Result<String, BaseUrlError> {
    if raw.is_empty()
        || raw
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || raw.contains('\\')
    {
        return Err(BaseUrlError::Malformed);
    }

    let parsed = url::Url::parse(raw).map_err(|_| BaseUrlError::Malformed)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(BaseUrlError::UnsafeScheme);
    }
    if parsed.cannot_be_a_base()
        || authority_contains_userinfo(raw)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(if parsed.cannot_be_a_base() {
            BaseUrlError::Malformed
        } else {
            BaseUrlError::UserInfo
        });
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(BaseUrlError::QueryOrFragment);
    }

    let Some(host) = parsed.host() else {
        return Err(BaseUrlError::Malformed);
    };
    if let Some(raw_host) = raw_authority_host(raw) {
        let raw_host = raw_host.trim_start_matches('[').trim_end_matches(']');
        if looks_like_noncanonical_ip_literal(raw_host) {
            return Err(BaseUrlError::AmbiguousHost);
        }
    }
    if let url::Host::Domain(domain) = host
        && !valid_domain_name(domain)
    {
        return Err(BaseUrlError::AmbiguousHost);
    }

    let canonical = parsed.to_string();
    let policy = agentcowork_guard::netfloor::NetPolicy::default();
    let Some(host) = parsed.host() else {
        return Err(BaseUrlError::Malformed);
    };
    if !policy.allows(agentcowork_guard::netfloor::classify_url_host(&host)) {
        return Err(BaseUrlError::PrivateDestination);
    }
    if agentcowork_guard::urlfloor::check_url_with_policy(&canonical, &[], policy)
        != agentcowork_guard::urlfloor::UrlVerdict::Allowed
    {
        return Err(BaseUrlError::PrivateDestination);
    }
    Ok(canonical)
}

/// Return a safe endpoint projection for IPC. Valid endpoints are already
/// canonical and contain no userinfo/query/fragment; anything else is replaced
/// by a non-sensitive marker rather than echoed back to the renderer.
pub fn redact_base_url(raw: &str) -> String {
    validate_base_url(raw).unwrap_or_else(|_| "<redacted>".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The agent has no verified contract; we refuse to guess.
    UnknownAgent(String),
    /// A subscription agent — nothing to configure, and its credentials are
    /// out of bounds.
    Subscription(String),
    /// Configured only through its own file; env cannot express it.
    ConfigFileOnly { agent: String, file: String },
    /// An env name or value failed validation (never a silent bad spawn).
    InvalidEnv(String),
    /// A custom endpoint failed the canonical URL/netfloor policy.
    InvalidBaseUrl { reason: BaseUrlError },
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::UnknownAgent(a) => write!(
                f,
                "{a} has no verified model-backend contract — refusing to guess"
            ),
            BackendError::Subscription(a) => write!(
                f,
                "{a} signs in with its own subscription; AgentCowork does not configure its credentials"
            ),
            BackendError::ConfigFileOnly { agent, file } => write!(
                f,
                "{agent} routes providers through {file} — that write needs approval (not an env override)"
            ),
            BackendError::InvalidEnv(m) => write!(f, "invalid env override: {m}"),
            BackendError::InvalidBaseUrl { reason } => {
                write!(f, "invalid base URL: {reason}")
            }
        }
    }
}

impl std::error::Error for BackendError {}

/// An env-var name must be a POSIX-ish identifier. Values are deliberately
/// conservative: a child environment is not a place for smuggled newlines,
/// control characters, or unbounded metadata.
fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn valid_env_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 8192
        && !value.chars().any(|character| character.is_control())
}

fn valid_model_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let secret_shaped = lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xoxb-")
        || lower.starts_with("bearer ");
    !secret_shaped
        && !value.is_empty()
        && value.len() <= 256
        && !value.contains("://")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '/')
        })
}

/// Validate a model identifier before it can be persisted or emitted as a
/// child variable. Model IDs are bounded ASCII identifiers, never URI-shaped
/// metadata or a place to smuggle a credential.
pub fn validate_model_id(model: &str) -> Result<(), BackendError> {
    if valid_model_value(model) {
        Ok(())
    } else {
        Err(BackendError::InvalidEnv(
            "model value is not a bounded identifier".into(),
        ))
    }
}

/// Decide the non-secret env pairs a spawn should carry. Pure.
///
/// The key variable is intentionally never emitted: this planner has no
/// secret input. Only names present in the reviewed child-environment
/// allowlist can produce a pair, and every endpoint is parsed and checked
/// against Guard's URL/netfloor policy before it is returned.
pub fn plan_env(
    spec: &AgentBackendSpec,
    binding: &ProviderBinding<'_>,
) -> Result<Vec<(String, String)>, BackendError> {
    match spec.channel {
        BackendChannel::Unknown => {
            return Err(BackendError::UnknownAgent(spec.agent_id.to_string()));
        }
        BackendChannel::Subscription => {
            return Err(BackendError::Subscription(spec.agent_id.to_string()));
        }
        BackendChannel::ConfigFileOnly => {
            return Err(BackendError::ConfigFileOnly {
                agent: spec.agent_id.to_string(),
                file: spec.config_file.unwrap_or("<unknown>").to_string(),
            });
        }
        BackendChannel::ProviderEnv | BackendChannel::FixedEnv => {}
    }

    // Validate a configured endpoint even when this particular agent has no
    // variable for it. That prevents a future caller from treating an unsafe
    // value as harmless merely because it is currently unexpressed.
    let canonical_base_url = binding
        .base_url
        .map(validate_base_url)
        .transpose()
        .map_err(|reason| BackendError::InvalidBaseUrl { reason })?;

    // Which names does this agent actually read? The key name is metadata
    // only; unlike the old compatibility API it can never produce a value.
    let (key_env, base_env, model_env): (Option<&str>, Option<&str>, Option<&str>) =
        match spec.channel {
            BackendChannel::FixedEnv => (
                spec.api_key_env.first().copied(),
                spec.base_url_env,
                spec.model_env,
            ),
            // ProviderEnv: the agent reads the provider's own names.
            _ => (
                Some(binding.key_env),
                binding.base_url_env,
                None, // provider-env agents pick their own model; we don't guess a var
            ),
        };

    let mut out: Vec<(String, String)> = Vec::new();

    if let Some(name) = key_env
        && !valid_env_name(name)
    {
        return Err(BackendError::InvalidEnv(
            "the provider key variable name is malformed".into(),
        ));
    }
    // No secret is accepted or emitted. An external agent authenticates
    // through its own store/sign-in flow.

    // Base URL: prefer the agent's fixed var; otherwise the provider's own
    // override var. Only a reviewed, non-secret name and a canonical safe URL
    // can produce a pair.
    if let (Some(url), Some(name)) = (canonical_base_url.as_deref(), base_env) {
        if !valid_env_name(name) {
            return Err(BackendError::InvalidEnv(
                "the base URL variable name is malformed".into(),
            ));
        }
        let reviewed = crate::client::ChildEnvName::from_explicit_name(name).map_err(|_| {
            BackendError::InvalidEnv(
                "the base URL variable is not in the reviewed child allowlist".into(),
            )
        })?;
        if !valid_env_value(url) {
            return Err(BackendError::InvalidEnv("bad base URL value".into()));
        }
        out.push((reviewed.as_str().to_string(), url.to_string()));
    }

    if let Some(m) = model_env
        && !binding.model.is_empty()
    {
        if !valid_env_name(m) {
            return Err(BackendError::InvalidEnv(
                "the model variable name is malformed".into(),
            ));
        }
        let reviewed = crate::client::ChildEnvName::from_explicit_name(m).map_err(|_| {
            BackendError::InvalidEnv(
                "the model variable is not in the reviewed child allowlist".into(),
            )
        })?;
        validate_model_id(binding.model)?;
        out.push((reviewed.as_str().to_string(), binding.model.to_string()));
    }

    Ok(out)
}

/// The var names a binding would inject — safe to show in the UI (names only,
/// never values).
pub fn injected_names(spec: &AgentBackendSpec, binding: &ProviderBinding<'_>) -> Vec<String> {
    plan_env(spec, binding)
        .map(|pairs| pairs.into_iter().map(|(k, _)| k).collect())
        .unwrap_or_default()
}

/// Binding elements the agent has **no env var for**, so they cannot be
/// expressed by an override. Reported rather than silently dropped: a
/// `ProviderEnv` agent that declares no base-URL var still reaches its
/// provider (it knows the provider's default), but the user must not be left
/// believing a custom URL took effect.
pub fn unexpressed(spec: &AgentBackendSpec, binding: &ProviderBinding<'_>) -> Vec<&'static str> {
    let mut gaps = Vec::new();
    if !spec.channel.is_env_injectable() {
        return gaps;
    }
    let base_env = match spec.channel {
        BackendChannel::FixedEnv => spec.base_url_env,
        _ => binding.base_url_env,
    };
    if binding.base_url.is_some() && base_env.is_none() {
        gaps.push("base_url");
    }
    let model_env = match spec.channel {
        BackendChannel::FixedEnv => spec.model_env,
        _ => None,
    };
    if !binding.model.is_empty() && model_env.is_none() {
        gaps.push("model");
    }
    gaps
}

// ---------------------------------------------------------------------------
// The matrix.
// ---------------------------------------------------------------------------

const CLAUDE: AgentBackendSpec = AgentBackendSpec {
    agent_id: "claude",
    channel: BackendChannel::FixedEnv,
    api_key_env: &["ANTHROPIC_API_KEY"],
    base_url_env: Some("ANTHROPIC_BASE_URL"),
    model_env: Some("ANTHROPIC_MODEL"),
    config_file: Some("~/.claude/settings.json"),
    note: "Claude Code reads the ANTHROPIC_* env at launch — an API-key endpoint can be pointed at this spawn without editing settings.json. A subscription login is left untouched.",
};

const CODEX: AgentBackendSpec = AgentBackendSpec {
    agent_id: "codex",
    channel: BackendChannel::ConfigFileOnly,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: Some("~/.codex/config.toml"),
    note: "Codex routes providers through ~/.codex/config.toml ([model_providers.<id>] + wire_api). Writing that file needs approval and is not an env override.",
};

const OPENCODE: AgentBackendSpec = AgentBackendSpec {
    agent_id: "opencode",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: Some("~/.config/opencode/opencode.json"),
    note: "OpenCode reads the provider's own env vars; keys added with /connect live in ~/.local/share/opencode/auth.json.",
};

const CLINE: AgentBackendSpec = AgentBackendSpec {
    agent_id: "cline",
    channel: BackendChannel::ConfigFileOnly,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: Some("~/.cline/data/settings/providers.json"),
    note: "Cline's CLI keeps provider config in ~/.cline/data/settings/providers.json. Writing it needs approval and is not an env override.",
};

const AIDER: AgentBackendSpec = AgentBackendSpec {
    agent_id: "aider",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: Some("~/.aider.conf.yml"),
    note: "Aider routes through LiteLLM, which reads the provider's own env vars; a project .env is also honored.",
};

const GOOSE: AgentBackendSpec = AgentBackendSpec {
    agent_id: "goose",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: Some("~/.config/goose/config.yaml"),
    note: "goose reads provider env vars; its own config.yaml selects the default provider.",
};

const HERMES: AgentBackendSpec = AgentBackendSpec {
    agent_id: "hermes",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "Hermes resolves standard provider env keys.",
};

const QWEN_CODE: AgentBackendSpec = AgentBackendSpec {
    agent_id: "qwen-code",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "Qwen Code follows the provider env convention.",
};

const KIMI: AgentBackendSpec = AgentBackendSpec {
    agent_id: "kimi",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "Kimi CLI follows the provider env convention.",
};

const KILO: AgentBackendSpec = AgentBackendSpec {
    agent_id: "kilo",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "Kilo Code follows the provider env convention.",
};

const OPENCLAW: AgentBackendSpec = AgentBackendSpec {
    agent_id: "openclaw",
    channel: BackendChannel::ProviderEnv,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "OpenClaw resolves provider env keys.",
};

const PI: AgentBackendSpec = AgentBackendSpec {
    agent_id: "pi",
    channel: BackendChannel::ConfigFileOnly,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: Some("~/.pi/agent/settings.json"),
    note: "pi configures its own model providers/keys in ~/.pi/agent/settings.json; pi-acp signs in through Terminal Auth (`pi-acp --terminal-login`).",
};

const GROK: AgentBackendSpec = AgentBackendSpec {
    agent_id: "grok",
    channel: BackendChannel::Subscription,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "Grok Build signs in with the xAI account; AgentCowork never touches those credentials.",
};

const GEMINI: AgentBackendSpec = AgentBackendSpec {
    agent_id: "gemini",
    channel: BackendChannel::Subscription,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "Gemini CLI signs in with the Google account.",
};

const COPILOT: AgentBackendSpec = AgentBackendSpec {
    agent_id: "copilot",
    channel: BackendChannel::Subscription,
    api_key_env: &[],
    base_url_env: None,
    model_env: None,
    config_file: None,
    note: "GitHub Copilot signs in with the GitHub account.",
};

/// The verified matrix. An agent absent from this list is [`Unknown`] by
/// construction — the absence is the honest answer, not an oversight.
///
/// [`Unknown`]: BackendChannel::Unknown
pub fn builtin_backend_specs() -> Vec<AgentBackendSpec> {
    vec![
        CLAUDE, CODEX, OPENCODE, CLINE, AIDER, GOOSE, HERMES, QWEN_CODE, KIMI, KILO, OPENCLAW, PI,
        GROK, GEMINI, COPILOT,
    ]
}

/// Look up an agent's contract. Returns a synthesized `Unknown` spec for an
/// agent we have no verified row for, so callers never have to unwrap.
pub fn backend_spec(agent_id: &str) -> AgentBackendSpec {
    builtin_backend_specs()
        .into_iter()
        .find(|s| s.agent_id == agent_id)
        .unwrap_or(AgentBackendSpec {
            agent_id: "",
            channel: BackendChannel::Unknown,
            api_key_env: &[],
            base_url_env: None,
            model_env: None,
            config_file: None,
            note: "No verified model-backend contract for this agent.",
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding<'a>(
        provider: &'a str,
        key_env: &'a str,
        base_url: Option<&'a str>,
        base_url_env: Option<&'a str>,
    ) -> ProviderBinding<'a> {
        ProviderBinding {
            provider,
            model: "some-model",
            key_env,
            base_url,
            base_url_env,
        }
    }

    #[test]
    fn fixed_env_plans_only_reviewed_non_secret_pairs() {
        let b = binding(
            "anthropic",
            "ANTHROPIC_API_KEY",
            Some("https://api.anthropic.com/v1"),
            None,
        );
        let pairs = plan_env(&CLAUDE, &b).unwrap();
        assert_eq!(
            pairs,
            vec![
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "https://api.anthropic.com/v1".to_string()
                ),
                ("ANTHROPIC_MODEL".to_string(), "some-model".to_string()),
            ]
        );
        assert!(pairs.iter().all(|(name, _)| name != "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn provider_env_uses_a_reviewed_non_secret_name_only() {
        let b = binding(
            "groq",
            "GROQ_API_KEY",
            Some("https://api.groq.com/openai/v1"),
            Some("GROQ_BASE_URL"),
        );
        let pairs = plan_env(&OPENCODE, &b).unwrap();
        assert_eq!(
            pairs,
            vec![(
                "GROQ_BASE_URL".to_string(),
                "https://api.groq.com/openai/v1".to_string()
            )]
        );
        assert!(pairs.iter().all(|(name, _)| name != "GROQ_API_KEY"));
    }

    #[test]
    fn no_key_pair_is_emitted_for_a_keyless_binding() {
        let b = binding(
            "anthropic",
            "ANTHROPIC_API_KEY",
            Some("http://127.0.0.1:11434/v1"),
            None,
        );
        let pairs = plan_env(&CLAUDE, &b).unwrap();
        assert_eq!(
            pairs,
            vec![
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "http://127.0.0.1:11434/v1".to_string()
                ),
                ("ANTHROPIC_MODEL".to_string(), "some-model".to_string())
            ]
        );
    }

    #[test]
    fn config_file_agents_refuse_with_the_path() {
        let b = binding("openai", "OPENAI_API_KEY", None, None);
        let err = plan_env(&CODEX, &b).unwrap_err();
        match err {
            BackendError::ConfigFileOnly { file, .. } => {
                assert!(file.contains("config.toml"), "got {file}");
            }
            other => panic!("expected ConfigFileOnly, got {other:?}"),
        }
        assert!(plan_env(&CLINE, &b).is_err());
    }

    #[test]
    fn subscription_agents_are_refused() {
        let b = binding("xai", "XAI_API_KEY", None, None);
        assert!(matches!(
            plan_env(&GROK, &b).unwrap_err(),
            BackendError::Subscription(_)
        ));
    }

    #[test]
    fn unknown_agents_are_refused_not_guessed() {
        let b = binding("anthropic", "ANTHROPIC_API_KEY", None, None);
        let spec = backend_spec("who-is-this");
        assert_eq!(spec.channel, BackendChannel::Unknown);
        assert!(matches!(
            plan_env(&spec, &b).unwrap_err(),
            BackendError::UnknownAgent(_)
        ));
    }

    #[test]
    fn pi_is_config_file_only_with_its_real_settings_path() {
        assert_eq!(PI.channel, BackendChannel::ConfigFileOnly);
        assert_eq!(PI.config_file, Some("~/.pi/agent/settings.json"));
        assert!(PI.note.contains("terminal-login"));
    }

    #[test]
    fn unsafe_base_urls_fail_closed_without_echoing_input() {
        for (raw, expected) in [
            ("not a url", BaseUrlError::Malformed),
            ("file:///etc/passwd", BaseUrlError::UnsafeScheme),
            (
                "https://user:password@example.invalid/v1",
                BaseUrlError::UserInfo,
            ),
            (
                "https://example.invalid/v1?token=exfiltrate",
                BaseUrlError::QueryOrFragment,
            ),
            (
                "https://example.invalid/v1#token=exfiltrate",
                BaseUrlError::QueryOrFragment,
            ),
            (
                "http://169.254.169.254/latest/meta-data/",
                BaseUrlError::PrivateDestination,
            ),
            ("http://192.168.1.10/v1", BaseUrlError::PrivateDestination),
            ("http://2130706433/v1", BaseUrlError::AmbiguousHost),
            ("http://0x7f000001/v1", BaseUrlError::AmbiguousHost),
            ("http://0177.0.0.1/v1", BaseUrlError::AmbiguousHost),
            ("https://@example.invalid/v1", BaseUrlError::UserInfo),
            ("ftp://example.invalid/v1", BaseUrlError::UnsafeScheme),
            ("https://-example.invalid/v1", BaseUrlError::AmbiguousHost),
            ("https://example..invalid/v1", BaseUrlError::AmbiguousHost),
            ("https://example.invalid./v1", BaseUrlError::AmbiguousHost),
        ] {
            assert_eq!(validate_base_url(raw), Err(expected), "raw={raw}");
            let rendered = validate_base_url(raw).unwrap_err().to_string();
            assert!(!rendered.contains("password"));
            assert!(!rendered.contains("exfiltrate"));
            assert_eq!(redact_base_url(raw), "<redacted>");
        }
    }

    #[test]
    fn debug_binding_redacts_endpoint_before_logging() {
        let b = binding(
            "anthropic",
            "ANTHROPIC_API_KEY",
            Some("https://user:password@example.invalid/v1?token=exfiltrate"),
            None,
        );
        let rendered = format!("{b:?}");
        assert!(!rendered.contains("password"));
        assert!(!rendered.contains("exfiltrate"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn safe_public_and_loopback_urls_are_canonicalized() {
        assert_eq!(
            validate_base_url("https://api.example.test/v1"),
            Ok("https://api.example.test/v1".to_string())
        );
        assert_eq!(
            validate_base_url("http://127.0.0.1:11434/v1"),
            Ok("http://127.0.0.1:11434/v1".to_string())
        );
    }

    #[test]
    fn unsafe_url_is_rejected_even_when_the_agent_has_no_base_variable() {
        let b = binding(
            "openai",
            "OPENAI_API_KEY",
            Some("https://example.invalid/v1?token=exfiltrate"),
            None,
        );
        assert!(matches!(
            plan_env(&OPENCODE, &b),
            Err(BackendError::InvalidBaseUrl {
                reason: BaseUrlError::QueryOrFragment
            })
        ));
    }

    #[test]
    fn unreviewed_backend_env_names_fail_closed() {
        let b = binding(
            "custom",
            "CUSTOM_API_KEY",
            Some("https://api.example.test/v1"),
            Some("CUSTOM_BASE_URL"),
        );
        assert!(matches!(
            plan_env(&OPENCODE, &b),
            Err(BackendError::InvalidEnv(_))
        ));
    }

    #[test]
    fn malformed_metadata_names_and_values_are_refused() {
        let b = ProviderBinding {
            provider: "anthropic",
            model: "model\nINJECTED=1",
            key_env: "ANTHROPIC_API_KEY",
            base_url: None,
            base_url_env: None,
        };
        assert!(matches!(
            plan_env(&CLAUDE, &b),
            Err(BackendError::InvalidEnv(_))
        ));
    }

    #[test]
    fn a_base_url_with_no_var_for_it_is_reported_not_silently_dropped() {
        let b = binding(
            "openai",
            "OPENAI_API_KEY",
            Some("https://gateway.example.test/v1"),
            None,
        );
        let pairs = plan_env(&OPENCODE, &b).unwrap();
        assert!(pairs.is_empty());
        assert_eq!(unexpressed(&OPENCODE, &b), vec!["base_url", "model"]);
        let b2 = binding(
            "anthropic",
            "ANTHROPIC_API_KEY",
            Some("https://api.anthropic.com/v1"),
            None,
        );
        assert!(unexpressed(&CLAUDE, &b2).is_empty());
        assert!(unexpressed(&CODEX, &b2).is_empty());
    }

    #[test]
    fn empty_model_does_not_inject_a_model_var() {
        let b = ProviderBinding {
            provider: "anthropic",
            model: "",
            key_env: "ANTHROPIC_API_KEY",
            base_url: None,
            base_url_env: None,
        };
        let pairs = plan_env(&CLAUDE, &b).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn names_only_surface_never_carries_a_key_value() {
        let b = binding(
            "anthropic",
            "ANTHROPIC_API_KEY",
            Some("https://api.anthropic.com/v1"),
            None,
        );
        let names = injected_names(&CLAUDE, &b);
        assert_eq!(names, vec!["ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL"]);
        assert!(names.iter().all(|name| !name.contains("KEY")));
    }

    #[test]
    fn matrix_ids_are_unique_and_absent_agents_are_unknown() {
        let specs = builtin_backend_specs();
        let mut ids: Vec<&str> = specs.iter().map(|s| s.agent_id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate agent id in the matrix");
        assert!(
            specs
                .iter()
                .any(|s| s.channel == BackendChannel::ProviderEnv)
        );
        assert!(specs.iter().any(|s| s.channel == BackendChannel::FixedEnv));
        assert!(
            specs
                .iter()
                .any(|s| s.channel == BackendChannel::ConfigFileOnly)
        );
    }

    #[test]
    fn channels_declare_injectability() {
        assert!(BackendChannel::ProviderEnv.is_env_injectable());
        assert!(BackendChannel::FixedEnv.is_env_injectable());
        assert!(!BackendChannel::ConfigFileOnly.is_env_injectable());
        assert!(!BackendChannel::Subscription.is_env_injectable());
        assert!(!BackendChannel::Unknown.is_env_injectable());
    }
}
