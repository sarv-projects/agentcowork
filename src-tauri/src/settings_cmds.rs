//! P65 — Settings Control Center backend (ARCH/17 §17.12).
//!
//! A **composition surface over authoritative subsystems**, not a new runtime.
//! Every row here is assembled from the subsystem that owns it:
//!
//! | Settings area | Owner (read) | Owner (write) |
//! |---|---|---|
//! | Providers (P65.1) | `catalog_cmds` probes/profiles, vault keyring, observation store | vault ring + profile store + `default_model.json` (this module's one file) |
//! | Agents (P65.2) | `acp_cmds` registry/install truth, `agent_backend_cmds` P63 env | P63 `agent_backend_set/clear` (unchanged) |
//! | Connections (P65.3) | `mcp_cmds` attach state, vault OAuth, store index | existing attach/detach/connect flows (unchanged) |
//! | Schedules (P65.4) | `SchedulerService` via the relay (one scheduler) | `scheduler_enable` et al (unchanged); run-now stays the due-pass path |
//! | Mutations (P65.6) | — | the one funnel below: validate → guard/policy → atomic persist → live apply → reread |
//!
//! Contract rules (frozen, §17.12):
//! - Exact type names: `SettingsReadModel`, `AgentSettings`, `ConnectionRecord`,
//!   `ScheduleSettings`, `InstalledExtension`, `RuntimeLocation`.
//! - Mutation protocol `request → validate → guard/policy → atomic persist →
//!   live apply → reread`, responding `{ appliedLive, restartRequired, state,
//!   health, lastError? }`.
//! - **Discard-optimistic-on-mismatch:** the UI must render the envelope's
//!   reread state, not its optimistic guess. When the authoritative reread
//!   disagrees with what was requested, the envelope carries the real state
//!   and `lastError`; there is no optimistic-only success path.
//! - `modelOwner` is `native | agent | managed`; `writesToAgentConfig` is
//!   always `false` (P63 env injection never writes an agent's own config).
//! - Keys/tokens cross this boundary **by reference only** (`authRef`); raw
//!   secrets are never serialized here.
//! - No second Guard, scheduler, vault, or registry is created here.

use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::control::{record_mutation, AuthKind};
use crate::AppState;

// ---------------------------------------------------------------------------
// Canonical contract types (ARCH/17 §17.12.2–§17.12.4)
// ---------------------------------------------------------------------------

/// The shared row shape every Settings inventory reuses.
///
/// The inventory commands return `serde_json::Value`: each row carries fields
/// beyond this shared core (a provider's `name`/`configured`, an agent's
/// runtime location, and so on). This struct is therefore the frozen §17.12.2
/// *declaration* those rows must stay compatible with, not a value the shell
/// builds at runtime — changing it to be constructed would alter live wire
/// shapes. Not dead code: it is the contract type the wire is checked against,
/// and the field-name test below pins it.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsReadModel {
    pub id: String,
    /// `provider | agent | connection | schedule | extension`.
    pub kind: String,
    /// `discovered | installed | configured | connected | disconnected |
    /// degraded | disabled | unavailable`.
    pub state: String,
    /// `ready | permission_required | missing | failed | unknown`.
    pub health: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub config_hash: String,
    pub applied_live: bool,
    pub restart_required: bool,
}

/// Where an agent runtime was proven to live. A discriminated value, never a
/// display string: discovery is read-only, and `installed / discovered /
/// launchable` stay three distinct facts (provenance + exact path + measured
/// version only, plus `verifiedAt` where a verify probe ran).
// N.B. the container-level `rename_all` on an enum renames the *variants*
// only (`Managed` → `managed`); it does NOT rename the fields inside struct
// variants. Each variant therefore carries its own `rename_all = "camelCase"`
// so `install_root`/`linux_path`/`windows_launcher` serialize as
// `installRoot`/`linuxPath`/`windowsLauncher` per ARCH/17 §17.12.4. Without it
// this was the one struct in the module that emitted snake_case to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeLocation {
    #[serde(rename_all = "camelCase")]
    Managed {
        executable: String,
        install_root: String,
        /// Measured version only; `""` = installed but version never measured.
        version: String,
    },
    #[serde(rename_all = "camelCase")]
    WindowsPath { executable: String, source: String },
    #[serde(rename_all = "camelCase")]
    WindowsRegistry { executable: String, source: String },
    #[serde(rename_all = "camelCase")]
    UserPath { executable: String, source: String },
    #[serde(rename_all = "camelCase")]
    PackageManager {
        manager: String,
        package: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Wsl {
        distro: String,
        linux_path: String,
        windows_launcher: String,
    },
    #[serde(rename_all = "camelCase")]
    Unavailable { reason: String },
}

/// The P63 binding as the Settings agent detail reports it: variable **names**
/// only, never values. `writesToAgentConfig` is always `false`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendBindingView {
    pub provider_id: String,
    pub injected_env_names: Vec<String>,
    pub unexpressed: Vec<String>,
    pub writes_to_agent_config: bool,
    pub key_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

/// One live ACP `configOption` row, reshaped to the contract vocabulary. The
/// agent owns this vocabulary; the native catalog is never substituted for it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigOptionView {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// One session capability-loadout row. A loadout, not a tool dump: defaults
/// come from live install/health/policy state, changes apply to the next
/// turn/run, and the row set is frozen into the Work manifest at creation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLoadoutRow {
    pub capability_id: String,
    pub source: String,
    /// `native | shared`.
    pub native_or_shared: String,
    pub enabled: bool,
    pub health: String,
    pub scope: String,
    pub requires_approval: bool,
    /// Fixed: changes take effect on the next turn/run.
    pub applies_from: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettings {
    pub agent_id: String,
    pub installed: bool,
    /// `inbuilt | acp | mcp`.
    pub protocol: String,
    /// Canonical auth-mode spelling (P69.C11, home `agentcowork_types::AuthMode`):
    /// `subscription | api_key | local | keyless | unknown`. The legacy
    /// `local_cli` spelling is deleted — `local` means local inference on this
    /// machine (`ARCH/03` §3.0), not "open source".
    pub auth_mode: String,
    pub native_capabilities: Vec<String>,
    pub shared_capabilities: Vec<String>,
    /// `native | agent | managed` — authoritative. `managed` only for a
    /// verified launch-time (P63) binding. Selecting a model from the native
    /// catalog while an external agent is active is forbidden (refused by
    /// `settings_default_model_set`).
    pub model_owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_binding: Option<BackendBindingView>,
    pub config_options: Vec<AgentConfigOptionView>,
    /// `ready | sign_in_required | api_key_required | local_cli |
    /// not_installed | unavailable | health_failed`.
    pub readiness: String,
    pub location: RuntimeLocation,
    pub session_loadout: Vec<SessionLoadoutRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRecord {
    pub id: String,
    /// `remote_mcp | oauth_connector | native_adapter | message_channel`.
    pub kind: String,
    /// `stdio | http | oauth | api_key | browser_session | native`.
    pub transport: String,
    pub scopes: Vec<String>,
    pub enabled_consumers: Vec<String>,
    /// `discovered | installed | connected | disconnected | degraded | revoked`.
    pub state: String,
    pub health: String,
    /// Opaque vault reference only (e.g. `vault:oauth:<p>:<id>`); never a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_ref: Option<String>,
    pub config_hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSettings {
    pub id: String,
    /// Human label owned by the scheduler job. Display only — the durable
    /// identity is [`ScheduleSettings::id`]. Present because the Settings
    /// surface must not show an opaque id where the Automations center shows
    /// a name (§17.12.2 is a baseline; §17.12.4/.5 set the precedent that the
    /// read models carry the fields the owning surface needs).
    pub name: String,
    /// `cron | interval | event | webhook`.
    pub trigger: String,
    /// The session/Work this schedule reawakens (frozen manifest per run).
    pub target: String,
    pub chief_agent_id: String,
    pub capability_scope: Vec<String>,
    pub autonomy: String,
    pub budget: String,
    pub network_policy: String,
    pub timezone: String,
    pub enabled: bool,
    pub config_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<u64>,
    /// Completed-run counter as the scheduler reports it (display only).
    pub runs: u64,
    /// `idle | running | paused | failed | disabled`.
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledExtension {
    pub id: String,
    /// `skill | plugin | mcp | acp | hook | tool`.
    pub kind: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abi_version: Option<u32>,
    pub provenance: String,
    pub digest: String,
    pub signature_status: String,
    pub capabilities_requested: Vec<String>,
    pub capabilities_granted: Vec<String>,
    pub bound_agents: Vec<String>,
    /// `lazy | active | disabled`.
    pub activation: String,
    pub health: String,
}

/// The standard mutation envelope (§17.12.3). Field order matches the
/// contract: `{ appliedLive, restartRequired, state, health, lastError? }`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsMutationResult {
    pub applied_live: bool,
    pub restart_required: bool,
    pub state: String,
    pub health: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Small shared helpers (pure)
// ---------------------------------------------------------------------------

/// Change-detection hash for `configHash` rows. NOT a security boundary — a
/// `DefaultHasher` fingerprint so the UI can tell "config changed" without
/// comparing payloads. Never used for auth, tickets, or integrity.
fn config_hash_of(canonical: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn failed_mutation(state: &str, health: &str, err: String) -> SettingsMutationResult {
    SettingsMutationResult {
        applied_live: false,
        restart_required: false,
        state: state.to_string(),
        health: health.to_string(),
        last_error: Some(err),
    }
}

/// P65.6 — the single mutation funnel.
///
/// `request → validate → guard/policy → atomic persist → live apply → reread`.
/// The `op` closure performs validate→persist→apply→reread against the owning
/// subsystem and returns the authoritative reread; the funnel stamps the
/// Merkle/audit row (human gesture is the authorization for Settings writes)
/// and maps any failure into the standard envelope — never an optimistic-only
/// success. Policy-widening ops must additionally route through the
/// `guard/set_policy_rules`-compatible tightening path inside `op` (see
/// `guard_cmds::guard_set_policy_rules`); this funnel never bypasses it.
fn settings_mutate(
    state: &AppState,
    audit_kind: &str,
    audit_payload: Value,
    fail_state: &str,
    op: impl FnOnce() -> Result<SettingsMutationResult, String>,
) -> SettingsMutationResult {
    match op() {
        Ok(envelope) => {
            record_mutation(state, AuthKind::HumanGesture, audit_kind, audit_payload);
            envelope
        }
        Err(e) => failed_mutation(fail_state, "failed", e),
    }
}

/// Atomic JSON write (tmp + rename) shared by Settings-owned files. The files
/// owned here (`default_model.json`) are the one owner of their choice; vault
/// secrets and agent configs are never written through this path.
fn atomic_write_json(path: &std::path::Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))
}

// ---------------------------------------------------------------------------
// P65.1 — ProviderSettingsRow assembler
// ---------------------------------------------------------------------------

/// Display-only ordering for the `Popular` group. Not a capability claim —
/// membership here grants nothing; it only orders the inventory.
const POPULAR_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "deepseek",
    "xai",
    "mistralai",
    "qwen",
    "meta-llama",
    "ollama",
    "opencode",
];

/// Pure provider state rule. `connected` requires a live observation
/// (`reachable == Some(true)`) on top of configuration — a catalog entry is
/// never `connected`, and a failed probe is `disconnected`, never silently
/// `configured`.
fn provider_state(
    key_configured: bool,
    has_profile: bool,
    keyless: bool,
    reachable: Option<bool>,
) -> (&'static str, &'static str) {
    let configured = key_configured || has_profile || keyless;
    match (configured, reachable) {
        (true, Some(true)) => ("connected", "ready"),
        (_, Some(false)) if configured => ("disconnected", "failed"),
        (true, _) => ("configured", "permission_required"),
        (false, _) => ("discovered", "unknown"),
    }
}

/// Pure grouping: `Configured` (vault key or usable profile or keyless
/// endpoint), then `Popular` (display order, excluding configured), then
/// `All` (everything, stable order). Inputs must be pre-sorted for a stable
/// `All` list.
fn split_provider_groups(
    all_sorted: &[String],
    configured: &std::collections::BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let popular: Vec<String> = POPULAR_PROVIDERS
        .iter()
        .map(|s| s.to_string())
        .filter(|id| all_sorted.iter().any(|a| a == id) && !configured.contains(id))
        .collect();
    let rest: Vec<String> = all_sorted
        .iter()
        // `id` is `&&String` here (iter + filter), so deref once for the
        // `Borrow<str>` lookup; a bare `&&String` has no matching `Borrow`.
        .filter(|id| !configured.contains(*id) && !popular.iter().any(|p| p == *id))
        .cloned()
        .collect();
    (popular, rest)
}

fn profile_store() -> agentcowork_catalog::ProfileStore {
    agentcowork_catalog::ProfileStore::in_dir(agentcowork_core::default_data_dir())
}

fn default_model_path() -> std::path::PathBuf {
    agentcowork_core::default_data_dir().join("default_model.json")
}

fn read_default_model() -> Value {
    std::fs::read(default_model_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(Value::Null)
}

/// Pure default-model validation. The provider must be a known catalog id and
/// the model non-empty; when the catalog actually knows that provider's models
/// (snapshot or profile), the model must be among them. When offline with no
/// known models, the choice is accepted (reread stays honest about health).
fn validate_default_model(
    provider: &str,
    model: &str,
    known_providers: &[String],
    known_models: Option<&[String]>,
) -> Result<(), String> {
    if provider.trim().is_empty() {
        return Err("provider must not be empty".to_string());
    }
    if model.trim().is_empty() {
        return Err("model must not be empty".to_string());
    }
    if !known_providers.iter().any(|p| p == provider) {
        return Err(format!("unknown provider '{provider}'"));
    }
    if let Some(models) = known_models {
        if !models.is_empty() && !models.iter().any(|m| m == model) {
            return Err(format!("unknown model '{model}' for provider '{provider}'"));
        }
    }
    Ok(())
}

fn known_models_for(state: &AppState, provider: &str) -> Option<Vec<String>> {
    let mut models: Vec<String> = Vec::new();
    if let Some(snap) = state.catalog.store.load() {
        // A loaded snapshot that does not carry this provider means "unknown",
        // not "no models" — `?` keeps that distinction a `None`.
        let p = snap.provider(provider)?;
        models.extend(p.models.keys().cloned());
    }
    let store = profile_store();
    if let Some(profile) = store.get(provider) {
        models.extend(profile.models.iter().map(|m| m.id.clone()));
    }
    Some(models)
}

/// P65.1 — the searchable provider inventory: `Configured / Popular / All`
/// grouping plus the persisted default model. Secrets never appear: rows carry
/// `keyConfigured` booleans, never key material.
#[tauri::command]
pub fn settings_providers_list(state: State<'_, AppState>) -> Value {
    let rows = crate::catalog_cmds::provider_rows(&state);
    let store = profile_store();
    let mut configured_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out: Vec<Value> = Vec::with_capacity(rows.len());
    for mut row in rows {
        let id = row
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let key_configured = row
            .get("keyConfigured")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let keyless = row
            .get("keyless")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let has_profile = store.get(&id).map(|p| p.is_usable()).unwrap_or(false);
        let reachable = row.get("reachable").and_then(|v| v.as_bool());
        let (st, health) = provider_state(key_configured, has_profile, keyless, reachable);
        if key_configured || has_profile {
            configured_ids.insert(id.clone());
        }
        let hash = config_hash_of(&format!(
            "{id}|{key_configured}|{has_profile}|{reachable:?}|{}",
            row.get("verifiedAt")
                .map(|v| v.to_string())
                .unwrap_or_default()
        ));
        if let Some(obj) = row.as_object_mut() {
            obj.insert("state".to_string(), json!(st));
            obj.insert("health".to_string(), json!(health));
            obj.insert("configHash".to_string(), json!(hash));
            obj.insert("appliedLive".to_string(), json!(true));
            obj.insert("restartRequired".to_string(), json!(false));
        }
        out.push(row);
    }
    let all_sorted: Vec<String> = out
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let configured: Vec<String> = configured_ids.iter().cloned().collect();
    let (popular, rest) = split_provider_groups(&all_sorted, &configured_ids);
    json!({
        "providers": out,
        "groups": { "configured": configured, "popular": popular, "all": rest },
        "defaultModel": read_default_model(),
        "status": state.catalog.status(),
    })
}

#[tauri::command]
pub fn settings_default_model_get() -> Value {
    json!({ "defaultModel": read_default_model() })
}

/// P65.1 — persist the default model, then reread. Selecting a native-catalog
/// model while an external (agent-owned-model) chief is the configured
/// `primary_chief` is refused: `modelOwner` is authoritative (§17.12.5).
#[tauri::command]
pub fn settings_default_model_set(
    state: State<'_, AppState>,
    provider: String,
    model: String,
) -> SettingsMutationResult {
    let rows = crate::catalog_cmds::provider_rows(&state);
    let known: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let known_models = known_models_for(&state, &provider);
    settings_mutate(
        &state,
        "settings.default_model.set",
        json!({ "provider": provider, "model": model }),
        "configured",
        || {
            // Native-vs-external two-plane guard: the native catalog must not
            // override an external agent's own model surface. With the built-in
            // identity retired (P71.2a) every pin names an external agent, so
            // an unset pin is the only pass-through — no spelling unlocks a
            // native model while an agent is bound (ADR-0005 §5).
            if let Ok(cfg) = agentcowork_core::Config::load() {
                if !cfg.primary_chief.trim().is_empty() {
                    return Err(format!(
                        "primary chief '{}' owns its model — selecting a native catalog model while an external agent is active is forbidden",
                        cfg.primary_chief
                    ));
                }
            }
            validate_default_model(&provider, &model, &known, known_models.as_deref())?;
            atomic_write_json(
                &default_model_path(),
                &json!({ "provider": provider, "model": model }),
            )?;
            // Reread: the file we just wrote is the authority.
            let reread = read_default_model();
            let ok = reread.get("provider").and_then(|v| v.as_str()) == Some(provider.as_str())
                && reread.get("model").and_then(|v| v.as_str()) == Some(model.as_str());
            if !ok {
                return Err(
                    "reread mismatch after persist — optimistic state discarded".to_string()
                );
            }
            Ok(SettingsMutationResult {
                applied_live: true,
                restart_required: false,
                state: "configured".to_string(),
                health: "ready".to_string(),
                last_error: None,
            })
        },
    )
}

// ---------------------------------------------------------------------------
// P65.2 — AgentSettings assembler
// ---------------------------------------------------------------------------

/// Pure `modelOwner` rule: `managed` only when a verified launch-time (P63)
/// binding exists, otherwise the agent owns its model (ADR-0005 §D6 — there
/// is no inbuilt engine for the native catalog to serve).
fn model_owner_for(has_verified_binding: bool) -> &'static str {
    if has_verified_binding {
        "managed"
    } else {
        "agent"
    }
}

/// Pure readiness rule over occupancy + auth facts. Never claims `ready`
/// without occupancy; subscription agents need an explicit sign-in.
fn agent_readiness(
    installed: bool,
    auth_mode: &str,
    live_handle: bool,
    live_auth_required: bool,
    key_available: bool,
) -> &'static str {
    if !installed {
        return "not_installed";
    }
    if live_handle && !live_auth_required {
        return "ready";
    }
    if live_handle && live_auth_required {
        return "sign_in_required";
    }
    match auth_mode {
        "subscription" => "sign_in_required",
        "api_key" => {
            if key_available {
                "ready"
            } else {
                "api_key_required"
            }
        }
        // Readiness vocabulary (a UI state, distinct from the auth-mode wire
        // contract): a local-inference agent renders as `local_cli`.
        // The deleted `local_cli` auth spelling is deliberately NOT accepted
        // here — callers pass the canonical `AuthMode::as_str()` value.
        "local" => "local_cli",
        "keyless" => "ready",
        _ => "unavailable",
    }
}

fn auth_mode_for(manifest: &agentcowork_acp::HarnessManifest) -> &'static str {
    // P69.C11 — one canonical serializer: the enum's own spelling, exactly as
    // `ui/src/lib/acp.ts` declares it. No hand-maintained second mapping.
    manifest.auth_mode.as_str()
}

fn protocol_for(manifest: &agentcowork_acp::HarnessManifest) -> &'static str {
    // The registry's launch plane is ACP only (ADR-0005 §D1 — external agents
    // are the v1 engines).
    let _ = manifest;
    "acp"
}

/// Map the shell's install-provenance JSON into the discriminated
/// `RuntimeLocation`. Catalog membership is never occupancy: `unavailable`
/// unless a probe produced an executable. A non-Windows `path` probe maps to
/// `user_path` (the contract's Windows-first vocabulary).
fn runtime_location_from_json(v: &Value) -> RuntimeLocation {
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("unavailable");
    let exe = v
        .get("executable")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();
    let source = v
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("path_probe")
        .to_string();
    match kind {
        "managed" => {
            let root = std::path::Path::new(&exe)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            RuntimeLocation::Managed {
                executable: exe,
                install_root: root,
                version: v
                    .get("version")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            }
        }
        "windows_path" => RuntimeLocation::WindowsPath {
            executable: exe,
            source,
        },
        "windows_registry" => RuntimeLocation::WindowsRegistry {
            executable: exe,
            source,
        },
        // Non-Windows PATH discoveries and explicit user paths share the
        // `user_path` arm: an explicit, probed filesystem location.
        "path" | "user_path" => RuntimeLocation::UserPath {
            executable: exe,
            source,
        },
        "package_manager" => RuntimeLocation::PackageManager {
            manager: v
                .get("manager")
                .and_then(|m| m.as_str())
                .unwrap_or("npx")
                .to_string(),
            package: v
                .get("package")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string(),
            version: v
                .get("version")
                .and_then(|x| x.as_str())
                .map(str::to_string),
        },
        "wsl" => RuntimeLocation::Wsl {
            distro: v
                .get("distro")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string(),
            linux_path: v
                .get("linuxPath")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string(),
            windows_launcher: "wsl.exe".to_string(),
        },
        _ => RuntimeLocation::Unavailable {
            reason: v
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("no probed executable")
                .to_string(),
        },
    }
}

fn native_caps() -> Vec<String> {
    // Every v1 engine is an external agent (ADR-0005 §D1): its native plane
    // is its own loop/tools/model/permissions, never AgentCowork's.
    vec![
        "own-loop".into(),
        "own-tools".into(),
        "own-model".into(),
        "own-permissions".into(),
    ]
}

fn shared_caps() -> Vec<String> {
    vec![
        "office-facade".into(),
        "browser-facade".into(),
        "computer-use-facade".into(),
        "memory-api".into(),
        "work".into(),
    ]
}

/// Session loadout rows: native rows first, then explicitly available shared
/// rows. Defaults come from live install/health state; every row applies from
/// the next turn/run and is frozen into the Work manifest at creation — never
/// retrofitted onto an in-flight run.
fn session_loadout_for(ready: bool, health: &str) -> Vec<SessionLoadoutRow> {
    let mut rows = Vec::new();
    for cap in native_caps() {
        rows.push(SessionLoadoutRow {
            capability_id: cap,
            source: "agent-native".to_string(),
            native_or_shared: "native".to_string(),
            enabled: ready,
            health: health.to_string(),
            scope: "session".to_string(),
            requires_approval: false,
            applies_from: "next-turn".to_string(),
        });
    }
    for cap in shared_caps() {
        // Ticketed families (computer use, connectors, browser operation)
        // require approval; read-shaped façades do not.
        let privileged = cap.contains("computer-use")
            || cap.contains("browser-facade")
            || cap.contains("office-facade");
        rows.push(SessionLoadoutRow {
            capability_id: cap,
            source: "agentcowork-shared".to_string(),
            native_or_shared: "shared".to_string(),
            enabled: ready,
            health: health.to_string(),
            scope: "session".to_string(),
            requires_approval: privileged,
            applies_from: "next-turn".to_string(),
        });
    }
    rows
}

fn build_agent_settings(
    state: &AppState,
    manifest: &agentcowork_acp::HarnessManifest,
) -> AgentSettings {
    let installed = crate::acp_cmds::agent_installed(&manifest.id);
    let auth_mode = auth_mode_for(manifest).to_string();

    // Live ACP truth: any handle launched under this agent id, plus its
    // agent-owned config options (never the native catalog).
    let (live_handle, live_auth_required, config_options): (
        bool,
        bool,
        Vec<AgentConfigOptionView>,
    ) = match state.acp_sessions.lock() {
        Ok(sessions) => {
            let mut found = (false, false, Vec::new());
            for h in sessions.values() {
                if h.agent_id == manifest.id {
                    found.0 = true;
                    found.1 = h.auth_required;
                    found.2 = h
                        .config_options
                        .iter()
                        .map(|o| AgentConfigOptionView {
                            id: o.id.clone(),
                            name: o.name.clone(),
                            value: if o.current_value.is_null() {
                                None
                            } else {
                                Some(o.current_value.clone())
                            },
                            options: if o.options.is_empty() {
                                None
                            } else {
                                Some(o.options.iter().map(|x| x.name.clone()).collect())
                            },
                        })
                        .collect();
                    break;
                }
            }
            found
        }
        Err(_) => (false, false, Vec::new()),
    };

    // P63 binding names (never values); `writesToAgentConfig` is always false.
    let binding = crate::agent_backend_cmds::backend_binding_view(state, &manifest.id).map(|b| {
        BackendBindingView {
            provider_id: b.provider_id,
            injected_env_names: b.injected_env_names,
            unexpressed: b.unexpressed,
            writes_to_agent_config: false,
            key_present: b.key_present,
            refusal: b.refusal,
        }
    });
    let verified_binding = binding
        .as_ref()
        .map(|b| b.refusal.is_none() && !b.provider_id.is_empty())
        .unwrap_or(false)
        && crate::agent_backend_cmds::has_managed_binding(state, &manifest.id);
    let model_owner = model_owner_for(verified_binding).to_string();

    // Key availability for readiness (boolean only — never the key).
    let key_available = binding.as_ref().map(|b| b.key_present).unwrap_or(false);
    let readiness = agent_readiness(
        installed,
        &auth_mode,
        live_handle,
        live_auth_required,
        key_available,
    )
    .to_string();
    let health = if readiness == "ready" {
        "ready"
    } else if readiness == "not_installed" || readiness == "unavailable" {
        "missing"
    } else {
        "permission_required"
    }
    .to_string();

    // Occupancy proof: the one provenance builder in `acp_cmds` (install
    // record / PATH / App-Paths / WSL probe) mapped into the discriminated
    // union. Catalog membership alone never claims occupancy.
    let location_json = crate::acp_cmds::runtime_location_for(&manifest.id);
    AgentSettings {
        agent_id: manifest.id.clone(),
        installed,
        protocol: protocol_for(manifest).to_string(),
        auth_mode,
        native_capabilities: native_caps(),
        shared_capabilities: shared_caps(),
        model_owner,
        backend_binding: binding,
        config_options,
        readiness: readiness.clone(),
        location: runtime_location_from_json(&location_json),
        session_loadout: session_loadout_for(readiness == "ready", &health),
    }
}

/// P65.2 — installed/discovered runtime inventory with two-plane detail:
/// native model/auth/config first, then explicitly available shared rows.
#[tauri::command]
pub fn settings_agents_list(state: State<'_, AppState>) -> Value {
    let registry = crate::acp_cmds::launch_registry();
    let agents: Vec<Value> = registry
        .agents
        .iter()
        .map(|m| serde_json::to_value(build_agent_settings(&state, m)).unwrap_or(Value::Null))
        .collect();
    json!({ "agents": agents })
}

#[tauri::command]
pub fn settings_agent_get(state: State<'_, AppState>, agent_id: String) -> Result<Value, String> {
    let registry = crate::acp_cmds::launch_registry();
    let manifest = registry
        .get(&agent_id)
        .ok_or_else(|| format!("unknown agent id: {agent_id}"))?;
    serde_json::to_value(build_agent_settings(&state, manifest)).map_err(|e| e.to_string())
}

/// P65.2 — the session loadout freeze shape for the Work manifest: the exact
/// rows a run creation must snapshot. Fetched live here; frozen at Work
/// creation so later Settings edits cannot mutate an in-flight run.
#[tauri::command]
pub fn settings_agent_loadout(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<Value, String> {
    let registry = crate::acp_cmds::launch_registry();
    let manifest = registry
        .get(&agent_id)
        .ok_or_else(|| format!("unknown agent id: {agent_id}"))?;
    let settings = build_agent_settings(&state, manifest);
    Ok(json!({
        "agentId": agent_id,
        "modelOwner": settings.model_owner,
        "frozenAt": "work-creation",
        "appliesFrom": "next-turn",
        "loadout": settings.session_loadout,
    }))
}

// ---------------------------------------------------------------------------
// P65.3 — ConnectionRecord assembler
// ---------------------------------------------------------------------------

/// Pure connected rule: `connected` requires a live proof (live child, stored
/// token, live account row). No proof → `disconnected`/`discovered`, never a
/// false `connected`. A live-but-unhealthy proof degrades instead of passing.
fn connection_state(
    has_live_proof: bool,
    ever_seen: bool,
    healthy: bool,
) -> (&'static str, &'static str) {
    match (has_live_proof, ever_seen, healthy) {
        (true, _, true) => ("connected", "ready"),
        (true, _, false) => ("degraded", "failed"),
        (false, true, _) => ("disconnected", "unknown"),
        (false, false, _) => ("discovered", "unknown"),
    }
}

fn remote_token_present(state: &AppState, store_id: &str) -> bool {
    if let Ok(tokens) = state.mcp_remote_tokens.lock() {
        if tokens.contains_key(store_id) {
            return true;
        }
    }
    if let Ok(vault) = state.vault.lock() {
        let mgr = agentcowork_vault::oauth::OAuthManager::new(&vault);
        if let Ok(Some(_)) = mgr.load_connector_token("remote-mcp", store_id) {
            return true;
        }
    }
    false
}

/// P65.3 — unify the MCP store, OAuth connectors, and native adapters into
/// `ConnectionRecord` groups. `connected` always has a live proof behind it:
///
/// - stdio servers: the live child map (`mcp_live`);
/// - remote MCP: a stored bearer token (session or vault);
/// - OAuth: a vault account row;
/// - the native adapter: the inbuilt catalog (always live).
/// Restart, failed handshake, revoke, and detach can therefore never leave a
/// false `connected` row or a live tool behind.
#[tauri::command]
pub fn settings_connections_list(state: State<'_, AppState>) -> Value {
    let mut out: Vec<Value> = Vec::new();

    // Native adapter: the inbuilt catalog is live by construction.
    {
        let total = agentcowork_mcp::all_tools().len();
        let hash = config_hash_of(&format!("native|{total}"));
        let row = ConnectionRecord {
            id: "agentcowork-native".to_string(),
            kind: "native_adapter".to_string(),
            transport: "native".to_string(),
            scopes: vec!["browser".to_string(), "storage".to_string()],
            enabled_consumers: vec!["chief".to_string(), "all-agents".to_string()],
            state: "connected".to_string(),
            health: "ready".to_string(),
            auth_ref: None,
            config_hash: hash,
        };
        out.push(serde_json::to_value(row).unwrap_or(Value::Null));
    }

    // Attached stdio servers: identity rows + live-child proof.
    {
        let attached = state
            .mcp_servers
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default();
        let live: std::collections::HashSet<String> = state
            .mcp_live
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut names: Vec<String> = attached.keys().cloned().collect();
        names.sort();
        for name in names {
            if let Some(row) = attached.get(&name) {
                let live_proof = live.contains(&name);
                // A live child that never produced a tool list is degraded,
                // not connected: no callable tool may hide behind the row.
                let healthy = live_proof && !row.tool_names.is_empty();
                let (st, health) = connection_state(live_proof, true, healthy);
                let hash = config_hash_of(&format!(
                    "{}|{}|{}",
                    name,
                    row.transport,
                    row.tool_names.join(",")
                ));
                out.push(
                    serde_json::to_value(ConnectionRecord {
                        id: name.clone(),
                        kind: "remote_mcp".to_string(),
                        transport: row.transport.clone(),
                        scopes: row.tool_names.clone(),
                        enabled_consumers: if live_proof {
                            vec!["chief".to_string()]
                        } else {
                            Vec::new()
                        },
                        state: st.to_string(),
                        health: health.to_string(),
                        auth_ref: None,
                        config_hash: hash,
                    })
                    .unwrap_or(Value::Null),
                );
            }
        }
    }

    // Store index: remote-MCP entries (token proof) + flat connectors
    // (discovery until a token exists). Installed-vs-marketplace stays split:
    // a store entry is never occupancy by itself.
    {
        use agentcowork_mcp::{StoreIndex, StoreKind};
        let index = StoreIndex::bundled();
        for entry in index.entries() {
            if entry.id == "agentcowork-native" {
                continue;
            }
            let is_remote = matches!(entry.kind, StoreKind::RemoteMcp);
            if !is_remote {
                let hash = config_hash_of(&format!("store|{}|connector", entry.id));
                out.push(
                    serde_json::to_value(ConnectionRecord {
                        id: entry.id.clone(),
                        kind: "oauth_connector".to_string(),
                        transport: "oauth".to_string(),
                        scopes: entry.consent.scopes_plain.clone(),
                        enabled_consumers: Vec::new(),
                        state: "discovered".to_string(),
                        health: "unknown".to_string(),
                        auth_ref: None,
                        config_hash: hash,
                    })
                    .unwrap_or(Value::Null),
                );
                continue;
            }
            let proof = remote_token_present(&state, &entry.id);
            let (st, health) = connection_state(proof, true, proof);
            let consumers = if proof {
                if entry.consent.indexes_into_memory {
                    vec!["chief".to_string(), "memory-index".to_string()]
                } else {
                    vec!["chief".to_string()]
                }
            } else {
                Vec::new()
            };
            let hash = config_hash_of(&format!("store|{}|{}", entry.id, proof));
            out.push(
                serde_json::to_value(ConnectionRecord {
                    id: entry.id.clone(),
                    kind: "remote_mcp".to_string(),
                    transport: "http".to_string(),
                    scopes: entry.consent.scopes_plain.clone(),
                    enabled_consumers: consumers,
                    state: st.to_string(),
                    health: health.to_string(),
                    auth_ref: proof.then(|| format!("vault:remote-mcp:{}", entry.id)),
                    config_hash: hash,
                })
                .unwrap_or(Value::Null),
            );
        }
    }

    // OAuth accounts: one row per vault account (handle-only view — the row
    // exists because a token is stored, so `connected` is proof-backed).
    {
        use agentcowork_vault::oauth::{OAuthManager, CHATGPT_PRO, COPILOT, QWEN};
        let accounts: Vec<agentcowork_vault::oauth::OAuthAccountInfo> = match state.vault.lock() {
            Ok(vault) => {
                let mgr = OAuthManager::new(&vault);
                if !mgr.enabled() {
                    Vec::new()
                } else {
                    let mut all = Vec::new();
                    for p in [CHATGPT_PRO, COPILOT, QWEN] {
                        if let Ok(list) = mgr.accounts(p) {
                            all.extend(list);
                        }
                    }
                    all
                }
            }
            Err(_) => Vec::new(),
        };
        for a in accounts {
            let hash = config_hash_of(&format!("oauth|{}|{}", a.provider, a.account_id));
            out.push(
                serde_json::to_value(ConnectionRecord {
                    id: format!("{}:{}", a.provider, a.account_id),
                    kind: "oauth_connector".to_string(),
                    transport: "oauth".to_string(),
                    scopes: vec![a.scopes.clone()],
                    enabled_consumers: vec!["chief".to_string()],
                    state: "connected".to_string(),
                    health: "unknown".to_string(),
                    auth_ref: Some(format!("vault:oauth:{}:{}", a.provider, a.account_id)),
                    config_hash: hash,
                })
                .unwrap_or(Value::Null),
            );
        }
    }

    out.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("id").and_then(|v| v.as_str()).unwrap_or(""))
    });
    json!({ "connections": out })
}

// ---------------------------------------------------------------------------
// P65.4 — ScheduleSettings envelope
// ---------------------------------------------------------------------------

/// Trigger mapping. `window` (a civil-day primitive above raw cron) reports as
/// `cron` — the due-cycle it feeds — rather than inventing a fifth trigger.
fn schedule_trigger_kind(trigger: &Value) -> &'static str {
    match trigger.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "interval" => "interval",
        "event" => "event",
        "webhook" => "webhook",
        _ => "cron",
    }
}

/// Pure run-state mapping: disabled wins over the machine state so a paused or
/// running job that the user switched off reads `disabled`, never `running`.
fn schedule_state(enabled: bool, run_state: &str) -> &'static str {
    if !enabled {
        return "disabled";
    }
    match run_state {
        "running" => "running",
        "paused" => "paused",
        "failed" => "failed",
        _ => "idle",
    }
}

fn capability_scope_for(steps: &Value) -> Vec<String> {
    let mut scope: Vec<String> = Vec::new();
    if let Some(arr) = steps.as_array() {
        for s in arr {
            let fam = match s.get("step").and_then(|v| v.as_str()).unwrap_or("") {
                "run_code" => "script",
                "online_search" => "search",
                "email" => "connector:email",
                "calendar" => "connector:calendar",
                _ => continue,
            };
            if !scope.iter().any(|x| x == fam) {
                scope.push(fam.to_string());
            }
        }
    }
    scope.sort();
    scope
}

fn guard_autonomy_level(state: &AppState) -> String {
    // Best-effort read of the live autonomy level; `unknown` when the service
    // cannot answer — never a fabricated level.
    if let Ok(mut svc) = state.guard_service.lock() {
        if let Ok(v) = svc.handle("guard/autonomy", &json!({})) {
            for key in ["level", "autonomy", "preset"] {
                if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                    if !s.is_empty() {
                        return s.to_string();
                    }
                }
            }
        }
    }
    "unknown".to_string()
}

fn schedule_settings_for(state: &AppState, job: &Value) -> ScheduleSettings {
    let id = job
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let trigger = job.get("trigger").cloned().unwrap_or(Value::Null);
    let policy = job.get("policy").cloned().unwrap_or(Value::Null);
    let enabled = job
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let run_state = job
        .get("state")
        .and_then(|s| s.get("state"))
        .and_then(|s| s.as_str())
        .unwrap_or("idle");
    let max_runs = policy
        .get("maxRunsPerHour")
        .and_then(|v| v.as_u64())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unbounded".to_string());
    let battery = policy
        .get("suppressOnBattery")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let chief = agentcowork_core::Config::load()
        .map(|c| c.primary_chief)
        .unwrap_or_else(|_| "inbuilt".to_string());
    let timezone = job
        .get("currentRun")
        .and_then(|r| r.get("timezone"))
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .unwrap_or("unspecified")
        .to_string();
    let canonical = serde_json::to_string(job).unwrap_or_default();
    ScheduleSettings {
        id: id.clone(),
        name: job
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        trigger: schedule_trigger_kind(&trigger).to_string(),
        target: job
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        chief_agent_id: chief,
        capability_scope: capability_scope_for(job.get("steps").unwrap_or(&Value::Null)),
        autonomy: guard_autonomy_level(state),
        budget: if battery {
            format!("{max_runs}/hr, battery-aware")
        } else {
            format!("{max_runs}/hr")
        },
        network_policy: policy
            .get("scope")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string(),
        timezone,
        enabled,
        config_hash: config_hash_of(&canonical),
        next_run_at: job.get("nextRunAt").and_then(|v| v.as_u64()),
        last_run_at: job.get("lastRunAt").and_then(|v| v.as_u64()),
        runs: job.get("runs").and_then(|v| v.as_u64()).unwrap_or(0),
        state: schedule_state(enabled, run_state).to_string(),
    }
}

fn scheduler_jobs(state: &AppState) -> Result<Vec<Value>, String> {
    // The one scheduler lives behind the relay; without the sidecar there is
    // no schedule truth to report — fail honestly, never an empty success.
    let handle = crate::scheduler_cmds::scheduler_handle(state)?;
    let svc = handle.lock().map_err(|e| e.to_string())?;
    Ok(svc
        .list()
        .iter()
        .map(|j| serde_json::to_value(j).unwrap_or(Value::Null))
        .collect())
}

/// P65.4 — schedule inventory with the Settings envelope. `run-now` stays the
/// existing due-pass (`scheduler_run_now`); normal Work/Run promotion is out
/// of scope. Edits affect future runs only: the in-flight `currentRun`
/// snapshot is frozen at run creation and never rewritten here.
#[tauri::command]
pub fn settings_schedules_list(state: State<'_, AppState>) -> Result<Value, String> {
    let jobs = scheduler_jobs(&state)?;
    let schedules: Vec<Value> = jobs
        .iter()
        .map(|j| serde_json::to_value(schedule_settings_for(&state, j)).unwrap_or(Value::Null))
        .collect();
    Ok(json!({ "schedules": schedules }))
}

#[tauri::command]
pub fn settings_schedule_get(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let jobs = scheduler_jobs(&state)?;
    let job = jobs
        .iter()
        .find(|j| j.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        .ok_or_else(|| format!("unknown schedule: {id}"))?;
    serde_json::to_value(schedule_settings_for(&state, job)).map_err(|e| e.to_string())
}

/// P65.4 + P65.6 — enable/disable through the funnel. Only the `enabled` flag
/// is touched; trigger/steps/policy (and the frozen in-flight snapshot) are
/// never rewritten by this path.
#[tauri::command]
pub fn settings_schedule_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> SettingsMutationResult {
    settings_mutate(
        &state,
        "settings.schedule.set_enabled",
        json!({ "id": id, "enabled": enabled }),
        "idle",
        || {
            if id.trim().is_empty() {
                return Err("schedule id must not be empty".to_string());
            }
            let handle = crate::scheduler_cmds::scheduler_handle(&state)?;
            {
                let mut svc = handle.lock().map_err(|e| e.to_string())?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                svc.set_enabled(&id, enabled, now)
                    .map_err(|e| e.to_string())?;
            }
            // Reread authoritative state; discard optimism on mismatch.
            let jobs = scheduler_jobs(&state)?;
            let job = jobs
                .iter()
                .find(|j| j.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
                .ok_or_else(|| {
                    "schedule vanished after update — optimistic state discarded".to_string()
                })?;
            let settings = schedule_settings_for(&state, job);
            if settings.enabled != enabled {
                return Err("reread mismatch after update — optimistic state discarded".to_string());
            }
            Ok(SettingsMutationResult {
                applied_live: true,
                restart_required: false,
                state: settings.state,
                health: "ready".to_string(),
                last_error: None,
            })
        },
    )
}

// ---------------------------------------------------------------------------
// InstalledExtension read model (P65.5 surface is contract-only here)
// ---------------------------------------------------------------------------

/// Read-only installed-extension rows over the one skill store plus attached
/// MCP rows. Discovery (signed index) never implies installed: a row exists
/// only for bytes on disk or a live attach. Full install/disable/remove flows
/// stay in `skills_cmds` / `mcp_cmds`; this is the Settings read shape.
#[tauri::command]
pub fn settings_extensions_list(state: State<'_, AppState>) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let root = agentcowork_types::env_compat::get_os("SKILLS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(agentcowork_blueprint::SkillStore::default_home);
    let store = agentcowork_blueprint::SkillStore::new(root);
    if let Ok(skills) = store.scan() {
        for s in skills {
            let granted: Vec<String> = s
                .manifest
                .tools
                .iter()
                .filter(|t| crate::skills_cmds::RUNTIME_CAPABILITY_ALLOWLIST.contains(&t.as_str()))
                .cloned()
                .collect();
            let tampered = store.root().join(&s.manifest.name).join("SKILL.md");
            // `is_tampered` already reports `Option<bool>`: `None` means the
            // skill has no install-time pin, so no integrity claim can be made.
            // Unwrapping that would collapse an honest "unknown" into a claim
            // the store cannot support.
            let tampered = std::fs::read(&tampered)
                .ok()
                .and_then(|b| store.is_tampered(&s.manifest.name, &b));
            let hash = config_hash_of(&format!("skill|{}|{}", s.manifest.name, s.manifest.version));
            out.push(
                serde_json::to_value(InstalledExtension {
                    id: s.manifest.name.clone(),
                    kind: "skill".to_string(),
                    version: s.manifest.version.clone(),
                    abi_version: None,
                    provenance: s.manifest.author.clone(),
                    digest: String::new(),
                    signature_status: if s.manifest.author == "agentcowork-store" {
                        "verified-store".to_string()
                    } else {
                        "unsigned-local".to_string()
                    },
                    capabilities_requested: s.manifest.tools.clone(),
                    capabilities_granted: granted,
                    bound_agents: Vec::new(),
                    activation: "lazy".to_string(),
                    health: match tampered {
                        Some(true) => "failed".to_string(),
                        _ => "unknown".to_string(),
                    },
                })
                .unwrap_or(Value::Null),
            );
            let _ = hash;
        }
    }
    // A poisoned lock yields no attached rows rather than a defaulted-empty
    // claim; `map(..)` already returns the `Result` the `if let` matches on.
    if let Ok(attached) = state.mcp_servers.lock().map(|m| m.clone()) {
        let mut names: Vec<String> = attached.keys().cloned().collect();
        names.sort();
        for name in names {
            if let Some(row) = attached.get(&name) {
                out.push(
                    serde_json::to_value(InstalledExtension {
                        id: name.clone(),
                        kind: "mcp".to_string(),
                        version: String::new(),
                        abi_version: None,
                        provenance: "user-attached".to_string(),
                        digest: String::new(),
                        signature_status: "unsigned-local".to_string(),
                        capabilities_requested: row.tool_names.clone(),
                        capabilities_granted: row.tool_names.clone(),
                        bound_agents: Vec::new(),
                        activation: "lazy".to_string(),
                        health: "unknown".to_string(),
                    })
                    .unwrap_or(Value::Null),
                );
            }
        }
    }
    out.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("id").and_then(|v| v.as_str()).unwrap_or(""))
    });
    json!({ "extensions": out })
}

// ---------------------------------------------------------------------------
// Tests (pure rules + contract shapes; no vault/network/sidecar)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn provider_state_never_invents_connected() {
        // Catalog-only: discovered, never connected.
        assert_eq!(
            provider_state(false, false, false, None),
            ("discovered", "unknown")
        );
        // Key but never observed: configured, not connected.
        assert_eq!(
            provider_state(true, false, false, None),
            ("configured", "permission_required")
        );
        // Observed reachable: connected.
        assert_eq!(
            provider_state(true, false, false, Some(true)),
            ("connected", "ready")
        );
        // Failed probe on a configured provider: disconnected/failed.
        assert_eq!(
            provider_state(true, false, false, Some(false)),
            ("disconnected", "failed")
        );
        // Keyless endpoint with a usable profile counts as configured.
        assert_eq!(
            provider_state(false, true, false, None),
            ("configured", "permission_required")
        );
    }

    #[test]
    fn groups_split_configured_popular_rest() {
        let all = vec![
            "anthropic".to_string(),
            "deepseek".to_string(),
            "my-vps".to_string(),
            "openai".to_string(),
            "zzz".to_string(),
        ];
        let configured: BTreeSet<String> = ["my-vps".to_string()].into_iter().collect();
        let (popular, rest) = split_provider_groups(&all, &configured);
        // The Popular group follows the curated `POPULAR_PROVIDERS` display
        // order (anthropic, openai, google, deepseek, …) because that is the
        // order `groups.popular` is emitted to the UI in. It is deliberately
        // *not* the alphabetical `all` order: `openai` is curated ahead of
        // `deepseek`. This assertion previously expected alphabetical order and
        // had never run, because the module was not part of the build.
        assert_eq!(popular, vec!["anthropic", "openai", "deepseek"]);
        // `rest` is everything else, preserving the caller's input order.
        assert_eq!(rest, vec!["zzz"]);
        // A configured popular id leaves the popular group.
        let configured2: BTreeSet<String> = ["anthropic".to_string()].into_iter().collect();
        let (popular2, _) = split_provider_groups(&all, &configured2);
        assert!(!popular2.contains(&"anthropic".to_string()));
    }

    #[test]
    fn default_model_validation_is_fail_closed() {
        let known = vec!["anthropic".to_string(), "openai".to_string()];
        assert!(validate_default_model("", "m", &known, None).is_err());
        assert!(validate_default_model("anthropic", "", &known, None).is_err());
        assert!(validate_default_model("nope", "m", &known, None).is_err());
        // Known models constrain the choice.
        let models = vec!["claude-x".to_string()];
        assert!(validate_default_model("anthropic", "claude-x", &known, Some(&models)).is_ok());
        assert!(validate_default_model("anthropic", "other", &known, Some(&models)).is_err());
        // Offline (no known models): accept, reread stays honest.
        assert!(validate_default_model("anthropic", "anything", &known, Some(&[])).is_ok());
        assert!(validate_default_model("anthropic", "anything", &known, None).is_ok());
    }

    /// P71.2a — the built-in engine is retired (ADR-0005 §2), so an agent's
    /// model surface is never attributed to the host: the column reports
    /// `managed` when AgentCowork holds a verified binding for it and `agent`
    /// otherwise. The old `native` branch existed only for the built-in row.
    #[test]
    fn model_owner_rule() {
        assert_eq!(model_owner_for(true), "managed");
        assert_eq!(model_owner_for(false), "agent");
    }

    /// P71.2a — the rule no longer takes an `is_builtin` argument: the built-in
    /// engine is retired (ADR-0005 §2), so every row is judged on install
    /// occupancy, the live handshake and the auth mode alone.
    #[test]
    fn readiness_never_claims_ready_without_occupancy() {
        assert_eq!(
            agent_readiness(false, "api_key", false, false, true),
            "not_installed"
        );
        assert_eq!(
            agent_readiness(true, "keyless", false, false, true),
            "ready"
        );
        assert_eq!(
            agent_readiness(true, "subscription", false, false, false),
            "sign_in_required"
        );
        assert_eq!(
            agent_readiness(true, "api_key", false, false, false),
            "api_key_required"
        );
        assert_eq!(
            agent_readiness(true, "api_key", false, false, true),
            "ready"
        );
        assert_eq!(
            agent_readiness(true, "local", false, false, false),
            "local_cli"
        );
        // A live handle outranks install facts (P71.3f).
        assert_eq!(
            agent_readiness(true, "api_key", true, true, true),
            "sign_in_required"
        );
        assert_eq!(agent_readiness(true, "api_key", true, false, true), "ready");
        assert_eq!(
            agent_readiness(true, "mystery", false, false, false),
            "unavailable"
        );
    }

    #[test]
    fn runtime_location_mapping_keeps_provenance_distinct() {
        // Non-Windows PATH probes land on user_path, never windows_path.
        let loc = runtime_location_from_json(&json!({
            "kind": "path", "source": "path_probe", "executable": "/usr/bin/opencode"
        }));
        assert!(matches!(loc, RuntimeLocation::UserPath { .. }));
        let loc = runtime_location_from_json(&json!({
            "kind": "managed", "source": "agentcowork_install",
            "executable": "/data/agents/x/y", "version": "1.2.3"
        }));
        match loc {
            RuntimeLocation::Managed {
                install_root,
                version,
                ..
            } => {
                assert_eq!(version, "1.2.3");
                assert!(!install_root.is_empty());
            }
            other => panic!("expected managed, got {other:?}"),
        }
        let loc = runtime_location_from_json(&json!({
            "kind": "wsl", "distro": "Ubuntu", "linuxPath": "/usr/bin/aider"
        }));
        assert!(matches!(loc, RuntimeLocation::Wsl { .. }));
        // Unknown kinds fail closed to unavailable, never a guessed path.
        let loc = runtime_location_from_json(&json!({ "kind": "teleport" }));
        assert!(matches!(loc, RuntimeLocation::Unavailable { .. }));
        let loc = runtime_location_from_json(&json!({}));
        assert!(matches!(loc, RuntimeLocation::Unavailable { .. }));
    }

    #[test]
    fn connection_state_never_false_connected() {
        assert_eq!(connection_state(true, true, true), ("connected", "ready"));
        assert_eq!(connection_state(true, true, false), ("degraded", "failed"));
        assert_eq!(
            connection_state(false, true, true),
            ("disconnected", "unknown")
        );
        assert_eq!(
            connection_state(false, false, false),
            ("discovered", "unknown")
        );
    }

    #[test]
    fn schedule_trigger_and_state_mapping() {
        assert_eq!(
            schedule_trigger_kind(&json!({"type": "interval"})),
            "interval"
        );
        assert_eq!(schedule_trigger_kind(&json!({"type": "event"})), "event");
        assert_eq!(
            schedule_trigger_kind(&json!({"type": "webhook"})),
            "webhook"
        );
        assert_eq!(schedule_trigger_kind(&json!({"type": "cron"})), "cron");
        // Window reports as the cron due-cycle it feeds.
        assert_eq!(schedule_trigger_kind(&json!({"type": "window"})), "cron");
        assert_eq!(schedule_trigger_kind(&json!({})), "cron");
        // Disabled wins over the machine state.
        assert_eq!(schedule_state(false, "running"), "disabled");
        assert_eq!(schedule_state(true, "running"), "running");
        assert_eq!(schedule_state(true, "paused"), "paused");
        assert_eq!(schedule_state(true, "failed"), "failed");
        assert_eq!(schedule_state(true, "idle"), "idle");
    }

    #[test]
    fn capability_scope_derives_from_step_families() {
        let steps = json!([
            {"step": "run_code", "language": "js", "code": "x"},
            {"step": "online_search", "query": "y"},
            {"step": "email", "to": [], "subject": "", "body": ""},
        ]);
        assert_eq!(
            capability_scope_for(&steps),
            vec!["connector:email", "script", "search"]
        );
        assert!(capability_scope_for(&Value::Null).is_empty());
    }

    #[test]
    fn mutation_envelope_uses_contract_field_names() {
        let ok = SettingsMutationResult {
            applied_live: true,
            restart_required: false,
            state: "connected".to_string(),
            health: "ready".to_string(),
            last_error: None,
        };
        let v = serde_json::to_value(ok).unwrap();
        for key in ["appliedLive", "restartRequired", "state", "health"] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert!(v.get("lastError").is_none());
        let err = failed_mutation("idle", "failed", "boom".to_string());
        let v = serde_json::to_value(err).unwrap();
        assert_eq!(v["appliedLive"], false);
        assert_eq!(v["lastError"], "boom");
    }

    /// §17.12.2 — the shared row shape serializes camelCase, and `lastError` is
    /// **omitted** (not `null`) when absent, so the UI treats "no error" as a
    /// missing key rather than an empty one.
    ///
    /// This is also what keeps `SettingsReadModel` reachable: it is the
    /// contract-named row shape every Settings inventory reuses, but the
    /// commands build their rows as `serde_json::Value` (provider rows arrive
    /// that way from `catalog_cmds`), so without a use site the type is dead
    /// code — which `clippy -D warnings` in CI would reject.
    #[test]
    fn settings_read_model_uses_contract_field_names() {
        let row = SettingsReadModel {
            id: "anthropic".to_string(),
            kind: "provider".to_string(),
            state: "configured".to_string(),
            health: "ready".to_string(),
            last_error: None,
            config_hash: config_hash_of("x"),
            applied_live: true,
            restart_required: false,
        };
        let v = serde_json::to_value(&row).unwrap();
        for key in [
            "id",
            "kind",
            "state",
            "health",
            "configHash",
            "appliedLive",
            "restartRequired",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert!(
            v.get("lastError").is_none(),
            "an absent error must be omitted, not serialized as null"
        );

        let failed = SettingsReadModel {
            last_error: Some("probe failed".to_string()),
            ..row
        };
        let v = serde_json::to_value(&failed).unwrap();
        assert_eq!(v["lastError"], "probe failed");
        assert_eq!(v["appliedLive"], true);
    }

    #[test]
    fn agent_settings_shape_keeps_writes_flag_false() {
        let b = BackendBindingView {
            provider_id: "anthropic".to_string(),
            injected_env_names: vec!["ANTHROPIC_API_KEY".to_string()],
            unexpressed: Vec::new(),
            writes_to_agent_config: false,
            key_present: true,
            refusal: None,
        };
        let v = serde_json::to_value(b).unwrap();
        assert_eq!(v["writesToAgentConfig"], false);
        assert_eq!(v["providerId"], "anthropic");
    }

    #[test]
    fn config_hash_is_stable_and_sensitive() {
        assert_eq!(config_hash_of("a"), config_hash_of("a"));
        assert_ne!(config_hash_of("a"), config_hash_of("b"));
    }

    /// P71.2a — the loadout row no longer takes an `is_builtin` flag: every
    /// row's native side is the **bound agent's** own surface (ADR-0005 §2).
    #[test]
    fn loadout_rows_apply_from_next_turn() {
        let rows = session_loadout_for(true, "ready");
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| r.applies_from == "next-turn"));
        assert!(rows.iter().any(|r| r.native_or_shared == "native"));
        assert!(rows.iter().any(|r| r.native_or_shared == "shared"));
    }
}
