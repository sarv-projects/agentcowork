//! P63 — **legacy per-agent backend compatibility view**.
//!
//! External agents own their model, account, authentication, and config under
//! ADR-0005. The persisted file remains readable so an older host binding is
//! reported honestly, but this module never reveals an AgentCowork vault-held
//! provider key into a child environment. A legacy request that depends on
//! that key is a typed, explicit unavailability: the agent must authenticate or
//! configure itself until an ADR-approved delegated-bearer mechanism exists.
//!
//! # What remains compatible
//!
//! A binding that needs no secret may still expose its validated non-secret
//! launch variables (for example a model id or base URL). The configuration is
//! never copied into the agent's own files, and the UI is told exactly which
//! names are non-secret launch inputs. `useVaultKey = true` is retained only
//! so old records fail closed with a useful refusal rather than disappearing.
//!
//! # Honesty surface
//!
//! `agent_backend_get` never returns a key or claims one was injected. It
//! reports non-secret variable names, vault-presence as non-activation data,
//! and a refusal that distinguishes an agent's own sign-in requirement from a
//! broken/unsupported host binding.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::catalog_cmds::{provider_rows, resolve_endpoint};
use crate::control::{record_mutation, AuthKind};
use crate::AppState;

/// One agent's chosen provider binding (never a secret).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendConfig {
    /// Catalog provider id (`anthropic`, `openai`, `ollama`, …).
    pub provider: String,
    /// Model id forwarded to agents that have a model variable.
    #[serde(default)]
    pub model: String,
    /// Legacy-only request. New writes with `true` are refused; old records
    /// fail closed because the host will not reveal a vault key to a child.
    #[serde(default)]
    pub use_vault_key: bool,
    /// Base-URL override (defaults to the catalog's endpoint for `provider`).
    #[serde(default)]
    pub base_url: Option<String>,
}

impl std::fmt::Debug for AgentBackendConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBackendConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("use_vault_key", &self.use_vault_key)
            .field("base_url", &self.base_url.as_deref().map(|_| "<redacted>"))
            .finish()
    }
}

type Store = BTreeMap<String, AgentBackendConfig>;

/// `<data_dir>/agent_backend.json` — the one owner of the per-agent choice.
pub fn config_path() -> std::path::PathBuf {
    agentcowork_core::default_data_dir().join("agent_backend.json")
}

fn load() -> Store {
    let store: Store = std::fs::read(config_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // A legacy file may predate the child-environment and URL floors. Drop
    // unsafe records at the owner boundary rather than carrying their raw
    // spelling into status, planning, or the next save.
    store
        .into_iter()
        .filter(|(_, config)| {
            let model_ok =
                config.model.is_empty() || agentcowork_acp::validate_model_id(&config.model).is_ok();
            let base_url_ok = match config.base_url.as_deref() {
                None => true,
                Some(raw) => agentcowork_acp::validate_base_url(raw).is_ok(),
            };
            model_ok && base_url_ok
        })
        .collect()
}

fn save(store: &Store) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(store).map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))
}

fn config_for(agent_id: &str) -> Option<AgentBackendConfig> {
    load().get(agent_id).cloned()
}

/// The env var name the **provider** uses, per the merged catalog rows
/// (seeds + profiles + live snapshot). `None` = no known convention.
fn provider_env_name(state: &AppState, provider: &str) -> Option<String> {
    provider_rows(state)
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(provider))
        .and_then(|r| r.get("env"))
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            agentcowork_catalog::base_registry()
                .get(provider)
                .and_then(|r| r.api_key_env.first().cloned())
        })
}

/// The provider's own base-URL override variable (declared by the catalog),
/// used only for `ProviderEnv` agents.
fn provider_base_url_env(provider: &str) -> Option<String> {
    agentcowork_catalog::base_registry()
        .get(provider)
        .and_then(|r| r.base_url_env.clone())
}

struct BindingParts {
    key_env: String,
    base_url: Option<String>,
    base_url_env: Option<String>,
    keyless: bool,
}

fn binding_parts(state: &AppState, cfg: &AgentBackendConfig) -> Result<BindingParts, String> {
    let key_env = provider_env_name(state, &cfg.provider)
        .ok_or_else(|| format!("no env var name known for provider '{}'", cfg.provider))?;
    let endpoint = resolve_endpoint(state, &cfg.provider);
    let candidate = cfg.base_url.as_deref().or_else(|| {
        endpoint
            .as_ref()
            .map(|e| e.base_url.as_str())
            .filter(|raw| !raw.trim().is_empty())
    });
    let base_url = candidate
        .map(|raw| {
            agentcowork_acp::validate_base_url(raw)
                .map_err(|reason| format!("base URL rejected by Guard/netfloor: {reason}"))
        })
        .transpose()?;
    let keyless = endpoint.as_ref().map(|e| e.keyless).unwrap_or(false);
    Ok(BindingParts {
        key_env,
        base_url,
        base_url_env: provider_base_url_env(&cfg.provider),
        keyless,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SpawnEnvError {
    /// A legacy host binding asked AgentCowork to reveal its own vault-held
    /// credential. No value is accepted as input here, so this decision cannot
    /// manufacture a secret-bearing env pair.
    VaultCredentialUnavailable {
        agent_id: String,
        credential_env: String,
    },
    /// The non-secret compatibility binding is not expressible.
    Backend(agentcowork_acp::BackendError),
}

impl std::fmt::Display for SpawnEnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VaultCredentialUnavailable {
                agent_id,
                credential_env,
            } => write!(
                f,
                "host provider binding unavailable: AgentCowork will not reveal a vault-held \
                 provider key as {credential_env} to {agent_id}; authenticate or configure the \
                 external agent in its own store (no delegated-bearer mechanism is approved)"
            ),
            Self::Backend(error) => write!(f, "{error}"),
        }
    }
}

/// Plan only credential-free launch variables. A legacy vault-key request is
/// refused before [`agentcowork_acp::plan_env`] is called, and the planner has no
/// secret-bearing input at all.
fn plan_spawn_env(
    agent_id: &str,
    spec: &agentcowork_acp::AgentBackendSpec,
    cfg: &AgentBackendConfig,
    parts: &BindingParts,
) -> Result<Vec<(String, String)>, SpawnEnvError> {
    if cfg.use_vault_key && !parts.keyless {
        return Err(SpawnEnvError::VaultCredentialUnavailable {
            agent_id: agent_id.to_string(),
            credential_env: parts.key_env.clone(),
        });
    }

    let binding = agentcowork_acp::ProviderBinding {
        provider: &cfg.provider,
        model: &cfg.model,
        key_env: &parts.key_env,
        base_url: parts.base_url.as_deref(),
        base_url_env: parts.base_url_env.as_deref(),
    };
    agentcowork_acp::plan_env(spec, &binding).map_err(SpawnEnvError::Backend)
}

/// Names-only view of what a launch would inject (safe for IPC).
fn inject_view(
    state: &AppState,
    agent_id: &str,
    cfg: &AgentBackendConfig,
) -> (Vec<String>, Vec<String>, bool, Option<String>) {
    let spec = agentcowork_acp::backend_spec(agent_id);
    if !spec.channel.is_env_injectable() {
        return (Vec::new(), Vec::new(), false, None);
    }
    let Ok(parts) = binding_parts(state, cfg) else {
        return (
            Vec::new(),
            Vec::new(),
            false,
            Some(format!(
                "host provider binding unavailable for {agent_id}: no verified non-secret binding \
                 could be resolved; the external agent must configure itself"
            )),
        );
    };
    let binding = agentcowork_acp::ProviderBinding {
        provider: &cfg.provider,
        model: &cfg.model,
        key_env: &parts.key_env,
        base_url: parts.base_url.as_deref(),
        base_url_env: parts.base_url_env.as_deref(),
    };
    let gaps = agentcowork_acp::unexpressed(&spec, &binding)
        .into_iter()
        .map(str::to_string)
        .collect();

    match plan_spawn_env(agent_id, &spec, cfg, &parts) {
        Ok(pairs) => {
            let names = pairs.into_iter().map(|(name, _)| name).collect();
            // `keyPresent` is a historical UI field. It remains false because
            // vault custody is not activation and no key is ever injected.
            (names, gaps, false, None)
        }
        Err(error) => (Vec::new(), gaps, false, Some(error.to_string())),
    }
}

/// P65.2 — names-only binding view for the Settings agent detail (the
/// `backendBinding` half of `AgentSettings`). Variable names and the
/// `unexpressed` gaps only — the secret never enters this type, and
/// `WritesToAgentConfig` is not even a field: env injection never writes an
/// agent's own config file.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendBindingView {
    pub provider_id: String,
    pub injected_env_names: Vec<String>,
    pub unexpressed: Vec<String>,
    pub key_present: bool,
    pub refusal: Option<String>,
}

pub(crate) fn backend_binding_view(
    state: &AppState,
    agent_id: &str,
) -> Option<AgentBackendBindingView> {
    let cfg = config_for(agent_id)?;
    let (injected, unexpressed, key_present, refusal) = inject_view(state, agent_id, &cfg);
    Some(AgentBackendBindingView {
        provider_id: cfg.provider,
        injected_env_names: injected,
        unexpressed,
        key_present,
        refusal,
    })
}

/// P65.2 — is this a verified, credential-free launch-time binding?
/// A legacy request that depends on a vault key is never managed: the
/// external agent owns that authentication and the host has no delegated
/// bearer mechanism to substitute.
pub(crate) fn has_managed_binding(state: &AppState, agent_id: &str) -> bool {
    let Some(cfg) = config_for(agent_id) else {
        return false;
    };
    let spec = agentcowork_acp::backend_spec(agent_id);
    if !spec.channel.is_env_injectable() {
        return false;
    }
    let (injected, _, _key_present, refusal) = inject_view(state, agent_id, &cfg);
    if refusal.is_some() {
        return false;
    }
    if injected.is_empty() {
        return false;
    }
    // A provider row with no expressible non-secret variable is not a managed
    // binding, even when a legacy record says `use_vault_key = false`.
    true
}

/// Credential-free env pairs a launch of `agent_id` may carry.
///
/// The compatibility signature remains because the live launch caller consumes
/// a vector. The actual decision is typed by [`plan_spawn_env`]: an old
/// vault-key binding is never downgraded to a partial secret injection. It
/// contributes no variables, and the same refusal is exposed by
/// `agent_backend_get`; the ACP handshake then independently reports whether
/// the self-contained agent must authenticate itself.
pub(crate) fn spawn_env_for(state: &AppState, agent_id: &str) -> Vec<(String, String)> {
    let Some(cfg) = config_for(agent_id) else {
        return Vec::new();
    };
    let spec = agentcowork_acp::backend_spec(agent_id);
    if !spec.channel.is_env_injectable() {
        return Vec::new();
    }
    let Ok(parts) = binding_parts(state, &cfg) else {
        return Vec::new();
    };
    plan_spawn_env(agent_id, &spec, &cfg, &parts).unwrap_or_default()
}

fn base_url_status(raw: Option<&str>) -> &'static str {
    match raw {
        None => "absent",
        Some(value) if agentcowork_acp::validate_base_url(value).is_ok() => "validated",
        Some(_) => "redacted",
    }
}

fn backend_status(
    channel: agentcowork_acp::BackendChannel,
    configured: bool,
    refusal: Option<&str>,
) -> (&'static str, bool) {
    let injectable = channel.is_env_injectable() && refusal.is_none();
    let status = if refusal.is_some() {
        "blocked"
    } else if configured {
        "configured"
    } else {
        "unconfigured"
    };
    (status, injectable)
}

/// The full card state for one agent: the contract, the choice, and the
/// names-only injection view.
#[tauri::command]
pub fn agent_backend_get(state: State<'_, AppState>, agent_id: String) -> Value {
    let spec = agentcowork_acp::backend_spec(&agent_id);
    let cfg = config_for(&agent_id);
    let (injected, unexpressed, key_present, refusal) = match &cfg {
        Some(c) => inject_view(&state, &agent_id, c),
        None => (Vec::new(), Vec::new(), false, None),
    };
    let base_url_status = base_url_status(cfg.as_ref().and_then(|c| c.base_url.as_deref()));
    let (status, injectable) = backend_status(spec.channel, cfg.is_some(), refusal.as_deref());
    json!({
        "agentId": agent_id,
        "channel": spec.channel.as_str(),
        // A legacy secret-dependent binding is not launch-injectable. The UI
        // takes the refusal branch instead of claiming a vault key was passed.
        "injectable": injectable,
        "credentialMode": "agent_owned",
        "hostVaultInjection": "unavailable",
        "status": status,
        "note": spec.note,
        "configFile": spec.config_file,
        "configured": cfg.as_ref().map(|c| json!({
            "provider": c.provider,
            "model": c.model,
            "useVaultKey": c.use_vault_key,
            "baseUrl": c
                .base_url
                .as_deref()
                .map(agentcowork_acp::redact_base_url),
            "baseUrlStatus": base_url_status,
        })),
        "injectedEnv": injected,
        "unexpressed": unexpressed,
        "keyPresent": key_present,
        "writesToDisk": false,
        "refusal": refusal,
    })
}

/// Candidate providers for this agent: the merged catalog, flagged with
/// whether a key already exists in the vault (so the card can show "in vault"
/// without ever reading the key).
#[tauri::command]
pub fn agent_backend_providers(state: State<'_, AppState>, _agent_id: String) -> Value {
    let vaulted: Vec<String> = {
        match state.vault.lock() {
            Ok(v) => agentcowork_vault::KeyRing::new(&v)
                .providers_with_keys()
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    };
    let rows: Vec<Value> = provider_rows(&state)
        .into_iter()
        .filter_map(|r| {
            let id = r.get("id")?.as_str()?.to_string();
            let env = r
                .get("env")
                .and_then(|e| e.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let base_url_raw = r
                .get("baseUrl")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let local = base_url_raw.contains("127.0.0.1") || base_url_raw.contains("localhost");
            let base_url = if base_url_raw.is_empty() {
                String::new()
            } else {
                agentcowork_acp::redact_base_url(base_url_raw)
            };
            Some(json!({
                "id": id,
                "name": r.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
                "env": env,
                "baseUrl": base_url,
                "local": local,
                "keyInVault": vaulted.iter().any(|p| p == &id),
                "verifiedAt": r.get("verifiedAt").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect();
    json!({ "providers": rows })
}

/// Set the agent's provider binding. Audited as a human gesture: the user's
/// click is the authorization, and nothing is written to the agent's own files.
#[tauri::command]
pub fn agent_backend_set(
    state: State<'_, AppState>,
    agent_id: String,
    provider: String,
    model: Option<String>,
    use_vault_key: Option<bool>,
    base_url: Option<String>,
) -> Result<Value, String> {
    if provider.trim().is_empty() {
        return Err("provider must not be empty".to_string());
    }
    let spec = agentcowork_acp::backend_spec(&agent_id);
    if spec.channel == agentcowork_acp::BackendChannel::Subscription {
        return Err(format!(
            "{agent_id} signs in with its own subscription — AgentCowork does not configure its credentials"
        ));
    }
    if spec.channel == agentcowork_acp::BackendChannel::Unknown {
        return Err(format!(
            "{agent_id} has no verified model-backend contract — refusing to guess"
        ));
    }

    let model = model.unwrap_or_default();
    if !model.is_empty() {
        agentcowork_acp::validate_model_id(&model).map_err(|error| error.to_string())?;
    }
    let use_vault_key = use_vault_key.unwrap_or(true);
    if use_vault_key {
        return Err(format!(
            "host provider binding unavailable: AgentCowork will not reveal a vault-held provider \
             key to {agent_id}; authenticate or configure the external agent in its own store \
             (no delegated-bearer mechanism is approved)"
        ));
    }
    let base_url = match base_url.filter(|u| !u.trim().is_empty()) {
        Some(raw) => Some(
            agentcowork_acp::validate_base_url(&raw)
                .map_err(|reason| format!("base URL rejected by Guard/netfloor: {reason}"))?,
        ),
        None => None,
    };
    let cfg = AgentBackendConfig {
        provider: provider.clone(),
        model: model.clone(),
        use_vault_key,
        base_url,
    };

    // Fail closed on a provider we cannot name an env var for: silently
    // storing a binding that can never be injected is the dishonest outcome.
    if spec.channel.is_env_injectable() && provider_env_name(&state, &provider).is_none() {
        return Err(format!(
            "no env var name known for provider '{provider}' — it cannot be injected into {agent_id}"
        ));
    }

    let has_base_url_override = cfg.base_url.is_some();
    let mut store = load();
    store.insert(agent_id.clone(), cfg);
    save(&store)?;

    record_mutation(
        &state,
        AuthKind::HumanGesture,
        "agent.backend.set",
        json!({
            "agentId": agent_id,
            "provider": provider,
            "model": model,
            "useVaultKey": use_vault_key,
            "hasBaseUrlOverride": has_base_url_override,
            // Deliberately no base URL / key material here: the audit is a
            // provenance record, not a config store.
            "channel": spec.channel.as_str(),
            "writesToDisk": false,
        }),
    );

    Ok(agent_backend_get(state, agent_id))
}

/// Forget the agent's binding (the agent returns to its own configuration).
#[tauri::command]
pub fn agent_backend_clear(state: State<'_, AppState>, agent_id: String) -> Result<Value, String> {
    let mut store = load();
    store.remove(&agent_id);
    save(&store)?;
    record_mutation(
        &state,
        AuthKind::HumanGesture,
        "agent.backend.clear",
        json!({ "agentId": agent_id }),
    );
    Ok(agent_backend_get(state, agent_id))
}

/// The card's health tick: probe the endpoint the agent would actually use.
/// Our probe of *that* endpoint — never "the agent says it works".
#[tauri::command]
pub fn agent_backend_probe(state: State<'_, AppState>, provider: String) -> Value {
    crate::catalog_cmds::probe_provider(&state, &provider, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(keyless: bool) -> BindingParts {
        BindingParts {
            key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: Some("https://api.anthropic.com/v1".to_string()),
            base_url_env: None,
            keyless,
        }
    }

    fn cfg(use_vault_key: bool) -> AgentBackendConfig {
        AgentBackendConfig {
            provider: "anthropic".to_string(),
            model: "test-model".to_string(),
            use_vault_key,
            base_url: None,
        }
    }

    #[test]
    fn legacy_vault_key_request_is_typed_unavailable_without_an_env_pair() {
        let error = plan_spawn_env(
            "claude",
            &agentcowork_acp::backend_spec("claude"),
            &cfg(true),
            &parts(false),
        )
        .expect_err("a host vault key must never be injected into an ACP child");

        assert_eq!(
            error,
            SpawnEnvError::VaultCredentialUnavailable {
                agent_id: "claude".to_string(),
                credential_env: "ANTHROPIC_API_KEY".to_string(),
            }
        );
        let rendered = error.to_string();
        assert!(rendered.contains("will not reveal"));
        assert!(rendered.contains("own store"));
        assert!(!rendered.to_ascii_uppercase().contains("SENTINEL"));
    }

    #[test]
    fn production_plan_spawn_env_has_no_secret_pair() {
        let pairs = plan_spawn_env(
            "claude",
            &agentcowork_acp::backend_spec("claude"),
            &cfg(false),
            &parts(false),
        )
        .expect("non-secret compatibility binding");

        assert_eq!(
            pairs,
            vec![
                (
                    "ANTHROPIC_BASE_URL".to_string(),
                    "https://api.anthropic.com/v1".to_string()
                ),
                ("ANTHROPIC_MODEL".to_string(), "test-model".to_string()),
            ]
        );
        assert!(pairs.iter().all(|(name, _)| name != "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn keyless_legacy_request_needs_no_credential_and_plans_no_key_pair() {
        let pairs = plan_spawn_env(
            "claude",
            &agentcowork_acp::backend_spec("claude"),
            &cfg(true),
            &parts(true),
        )
        .expect("keyless binding needs no credential");
        assert!(pairs.iter().all(|(name, _)| name != "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn url_rejections_are_typed_and_redacted() {
        for (raw, expected) in [
            (
                "https://user:password@example.invalid/v1",
                agentcowork_acp::BaseUrlError::UserInfo,
            ),
            (
                "https://example.invalid/v1?token=exfiltrate",
                agentcowork_acp::BaseUrlError::QueryOrFragment,
            ),
            (
                "https://example.invalid/v1#token=exfiltrate",
                agentcowork_acp::BaseUrlError::QueryOrFragment,
            ),
            (
                "http://169.254.169.254/latest/meta-data/",
                agentcowork_acp::BaseUrlError::PrivateDestination,
            ),
            (
                "http://192.168.1.10/v1",
                agentcowork_acp::BaseUrlError::PrivateDestination,
            ),
        ] {
            let error = agentcowork_acp::validate_base_url(raw).expect_err("unsafe URL");
            assert_eq!(error, expected);
            let rendered = error.to_string();
            assert!(!rendered.contains("password"));
            assert!(!rendered.contains("exfiltrate"));
            assert_eq!(agentcowork_acp::redact_base_url(raw), "<redacted>");
        }
    }

    #[test]
    fn status_shape_distinguishes_unconfigured_configured_and_blocked() {
        assert_eq!(
            backend_status(agentcowork_acp::BackendChannel::FixedEnv, false, None),
            ("unconfigured", true)
        );
        assert_eq!(
            backend_status(agentcowork_acp::BackendChannel::FixedEnv, true, None),
            ("configured", true)
        );
        assert_eq!(
            backend_status(
                agentcowork_acp::BackendChannel::FixedEnv,
                true,
                Some("refused")
            ),
            ("blocked", false)
        );
        assert_eq!(base_url_status(None), "absent");
        assert_eq!(
            base_url_status(Some("https://api.example.test/v1")),
            "validated"
        );
        assert_eq!(
            base_url_status(Some("https://user:pass@example.invalid/v1")),
            "redacted"
        );
    }
}
