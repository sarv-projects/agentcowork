//! P56.1–P56.7 — the live provider-catalog surface.
//!
//! Three owners, each doing one job:
//!
//! * `agentcowork-catalog::{live, store, fetch}` — parse/validate/persist the
//!   models.dev snapshot (pure + durable).
//! * `agentcowork-catalog::{profiles, provider}` — the user-config profiles and
//!   the vendored provider registry (identity + aliases).
//! * this module — the shell's runtime surface: the 4h refresh job, the
//!   Settings → Providers list, the per-provider model table, the activate
//!   screen's `MetadataOnly` probe, and the endpoint resolution the chat relay
//!   consumes at boot.
//!
//! Nothing here invents a provider: a row exists because the live catalog, the
//! vendored registry, or the user's own profile file says so. Secrets never
//! cross this boundary — a probe carries a key *in*, and only a status back.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentcowork_catalog::{
    apply_observations, base_registry, endpoint_probe_result, refresh_now, Auth, CatalogSnapshot,
    CatalogStore, EndpointProbe, HttpFetch, ObservationStore, ProfileFormat, ProfileModel,
    ProfileSource, ProfileStore, ProviderObservation, ProviderObservationsFile, ProviderProfile,
    ProviderProfilesFile, ProviderRegistry, RefreshDecision, RefreshOutcome, DEFAULT_REFRESH_SECS,
};
use agentcowork_vault::{Broker, KeyRing, ProviderEndpoint, WireTransport};
use serde_json::{json, Value};
use tauri::{Manager, State};

use crate::AppState;

/// P56.1 — the catalog's runtime owner: the durable store plus a refresh gate
/// so the 4h timer and a manual Settings refresh never fetch concurrently.
pub struct CatalogState {
    pub store: CatalogStore,
    refreshing: Mutex<()>,
}

impl CatalogState {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            store: CatalogStore::new(dir),
            refreshing: Mutex::new(()),
        }
    }

    /// Run one refresh (`force` skips the staleness check). Serialized.
    pub fn refresh(&self, force: bool) -> RefreshOutcome {
        let _guard = self.refreshing.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        if !force {
            let meta = self.store.load_meta();
            let fresh = meta
                .as_ref()
                .map(|m| {
                    !agentcowork_catalog::is_stale(
                        m.fetched_at,
                        now,
                        self.store.refresh_interval_secs(),
                    )
                })
                .unwrap_or(false);
            if fresh {
                let snapshot = self.store.load();
                return RefreshOutcome {
                    decision: match snapshot {
                        Some(s) => RefreshDecision::NotModified {
                            fetched_at: s.fetched_at,
                            providers: s.provider_count(),
                            models: s.model_count(),
                        },
                        None => RefreshDecision::Failed {
                            error: "catalog is fresh but no snapshot is stored".to_string(),
                        },
                    },
                    meta,
                    persisted: false,
                };
            }
        }
        refresh_now(&self.store, &HttpFetch::new(), now)
    }

    /// The cheap status half (never parses the snapshot).
    pub fn status(&self) -> Value {
        let meta = self.store.load_meta();
        let now = now_ms();
        let interval = self.store.refresh_interval_secs();
        let (fetched_at, providers, models, last_decision, last_failed, has_snapshot) = match &meta
        {
            Some(m) => (
                m.fetched_at,
                m.providers,
                m.models,
                m.last_decision.clone(),
                m.last_failed,
                self.store.load().is_some(),
            ),
            None => (0, 0, 0, None, false, self.store.load().is_some()),
        };
        json!({
            "source": agentcowork_catalog::MODELS_DEV_API_URL,
            "hasSnapshot": has_snapshot,
            "fetchedAt": fetched_at,
            "stale": agentcowork_catalog::is_stale(fetched_at, now, interval),
            "intervalHours": interval / 3600,
            "providers": providers,
            "models": models,
            "lastDecision": last_decision,
            "lastFailed": last_failed,
            "snapshotBytes": std::fs::metadata(self.store.snapshot_path()).map(|m| m.len()).ok(),
        })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn profile_store() -> ProfileStore {
    ProfileStore::in_dir(agentcowork_core::default_data_dir())
}

/// A registry carrying **runtime truth**: the vendored identity layer with any
/// recorded live observation replayed onto it.
///
/// Every surface that makes a claim about a provider's capabilities or health
/// goes through here, so "advertised" and "observed" cannot drift apart again.
/// A registry built with bare `base_registry()` has no observation history by
/// construction, which is how `verifiedAt` and the routing feed stayed empty.
pub fn observed_registry() -> ProviderRegistry {
    observed_registry_in(&agentcowork_core::default_data_dir())
}

/// The recorded provider observations, as read from disk.
pub fn observation_file() -> ProviderObservationsFile {
    observation_file_in(&agentcowork_core::default_data_dir())
}

// The `_in` forms take the data dir explicitly so the write-back → replay path
// is testable against a temp dir. The no-arg forms above are the production
// entry points; nothing else may read observations from another location.

fn observation_store_in(dir: &std::path::Path) -> ObservationStore {
    ObservationStore::in_dir(dir)
}

fn observation_file_in(dir: &std::path::Path) -> ProviderObservationsFile {
    observation_store_in(dir).load()
}

fn observed_registry_in(dir: &std::path::Path) -> ProviderRegistry {
    let mut registry = base_registry();
    apply_observations(&mut registry, &observation_file_in(dir));
    registry
}

/// P56.2/P56.7 — the merged provider list.
///
/// Sources are layered, lowest precedence first: the vendored registry
/// (identity, aliases, auth shape) → the live models.dev snapshot (name, npm,
/// api, doc, env, model rows) → the shipped overlays (OpenCode Zen/Go/Free,
/// NVIDIA NIM) → the user's own profiles. A row is never fabricated.
pub fn provider_rows(state: &AppState) -> Vec<Value> {
    let registry = observed_registry();
    let observed = observation_file();
    let snapshot = state.catalog.store.load();
    let profiles = profile_store();
    let keyed: std::collections::HashSet<String> = {
        let vault = state.vault.lock().map_err(|e| e.to_string());
        match vault {
            Ok(v) => KeyRing::new(&v)
                .providers_with_keys()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            Err(_) => Default::default(),
        }
    };

    // id → merged row
    let mut rows: BTreeMap<String, Value> = BTreeMap::new();

    for rec in registry.all() {
        let reach = observed.reachability(&rec.id);
        rows.insert(
            rec.id.clone(),
            json!({
                "id": rec.id,
                "name": rec.name,
                "aliases": rec.aliases,
                "env": rec.api_key_env,
                "auth": format!("{:?}", rec.auth).to_lowercase(),
                "transport": rec.transport.map(|t| format!("{t:?}").to_lowercase()),
                "baseUrl": rec.base_url.clone().unwrap_or_default(),
                "docUrl": Value::Null,
                "npm": Value::Null,
                "logoUrl": agentcowork_catalog::logo_url(&rec.id),
                "source": format!("{:?}", rec.source).to_lowercase(),
                "modelIds": Vec::<String>::new(),
                "modelCount": 0,
                "keyConfigured": keyed.contains(&rec.id),
                "profileSource": Value::Null,
                "format": Value::Null,
                "keyless": matches!(rec.auth, agentcowork_catalog::Auth::Keyless),
                "sessionHeaders": false,
                "verifiedAt": rec.capabilities_verified_at.clone(),
                // Runtime truth, kept separate from the verification stamp:
                // "the endpoint answered" and "its capabilities are trusted"
                // are different facts and must not be conflated in the UI.
                "observedAt": reach.as_ref().map(|r| r.observed_at.clone()),
                "reachable": reach.as_ref().map(|r| r.ok),
                "observedModelCount": reach.as_ref().map(|r| r.model_count),
            }),
        );
    }

    if let Some(snap) = &snapshot {
        for p in snap.providers.values() {
            let entry = rows.entry(p.id.clone()).or_insert_with(|| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "aliases": Vec::<String>::new(),
                    "env": Vec::<String>::new(),
                    "auth": "unknown",
                    "transport": Value::Null,
                    "baseUrl": "",
                    "docUrl": Value::Null,
                    "npm": Value::Null,
                    "logoUrl": p.logo_url(),
                    "source": "models-dev-live",
                    "keyConfigured": false,
                    "profileSource": Value::Null,
                    "format": Value::Null,
                    "keyless": false,
                    "sessionHeaders": false,
                    "verifiedAt": Value::Null,
                    "observedAt": Value::Null,
                    "reachable": Value::Null,
                    "observedModelCount": Value::Null,
                })
            });
            entry["name"] = json!(p.name);
            entry["npm"] = json!(p.npm);
            entry["api"] = json!(p.api);
            entry["docUrl"] = json!(p.doc);
            entry["env"] = json!(p.env);
            entry["logoUrl"] = json!(p.logo_url());
            entry["modelCount"] = json!(p.model_count());
            entry["modelIds"] = json!(p.models.keys().cloned().collect::<Vec<_>>());
            entry["transport"] = json!(format!("{:?}", p.transport()).to_lowercase());
            if entry["baseUrl"].as_str().unwrap_or("").is_empty() {
                entry["baseUrl"] = json!(p.api.clone().unwrap_or_default());
            }
            entry["source"] = json!("models-dev-live");
        }
    }

    // Shipped overlays (P56.5/P56.6) then user profiles (P55.6/P56.4).
    let overlays = agentcowork_catalog::opencode_overlay_profiles();
    for profile in overlays.iter().chain(profiles.list().iter()) {
        let entry = rows.entry(profile.id.clone()).or_insert_with(|| {
            json!({
                "id": profile.id,
                "name": profile.name,
                "aliases": Vec::<String>::new(),
                "env": Vec::<String>::new(),
                "auth": if profile.api_key_required { "api_key_env" } else { "keyless" },
                "transport": Value::Null,
                "docUrl": Value::Null,
                "npm": Value::Null,
                "logoUrl": agentcowork_catalog::logo_url(&profile.id),
                "source": "overlay",
                "modelIds": Vec::<String>::new(),
                "modelCount": 0,
                "keyConfigured": false,
                "verifiedAt": Value::Null,
                "observedAt": Value::Null,
                "reachable": Value::Null,
                "observedModelCount": Value::Null,
            })
        });
        entry["name"] = json!(profile.name);
        entry["profileSource"] = json!(format!("{:?}", profile.source).to_lowercase());
        entry["format"] = json!(profile.format);
        entry["keyless"] = json!(!profile.api_key_required);
        entry["sessionHeaders"] = json!(profile.session_headers);
        entry["baseUrl"] = json!(profile.base_url);
        if let Some(v) = &profile.verified_at {
            entry["verifiedAt"] = json!(v);
        }
        if !profile.models.is_empty() {
            entry["modelIds"] = json!(profile
                .models
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>());
            entry["modelCount"] = json!(profile.models.len());
        }
    }

    let mut out: Vec<Value> = rows.into_values().collect();
    out.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    });
    out
}

/// Pre-loaded catalog inputs for endpoint resolution.
///
/// `resolve_endpoint` used to re-read the ~4.6 MB models.dev snapshot, re-read
/// `providers.json`, and rebuild the 212-provider registry **per provider** —
/// and the boot path ran it for every catalog row, so one boot cost hundreds
/// of full parses and minutes of CPU. The relay never installed, and the UI
/// reported "coordinator offline". This context reads each of the three
/// exactly once and resolves any number of providers against it.
struct ResolveCtx {
    snapshot: Option<CatalogSnapshot>,
    profiles: ProviderProfilesFile,
    registry: ProviderRegistry,
}

impl ResolveCtx {
    fn load(state: &AppState) -> Self {
        Self {
            snapshot: state.catalog.store.load(),
            profiles: profile_store().load(),
            registry: observed_registry(),
        }
    }

    fn profile(&self, provider: &str) -> Option<ProviderProfile> {
        self.profiles.profiles.get(provider).cloned()
    }

    /// The base URL a probe should hit, in precedence order: user profile →
    /// live catalog `api` → the vendored registry. `None` is honest — some
    /// models.dev providers ship an SDK-default endpoint we cannot construct
    /// ourselves.
    fn base_url(&self, provider: &str) -> Option<String> {
        if let Some(p) = self.profile(provider) {
            if let Some(url) = p.normalized_base_url() {
                return Some(url);
            }
        }
        if let Some(snap) = &self.snapshot {
            if let Some(p) = snap.provider(provider) {
                if let Some(api) = p.api.clone() {
                    return Some(api.trim_end_matches('/').to_string());
                }
            }
        }
        self.registry
            .resolve(provider)
            .and_then(|r| r.base_url.clone())
    }

    /// Resolve the endpoint for a provider (P55.5) — profile → catalog →
    /// registry.
    fn endpoint(&self, provider: &str) -> Option<ProviderEndpoint> {
        let profile = self.profile(provider);
        let base = self.base_url(provider)?;
        let (transport, headers, session_headers, keyless) = match &profile {
            Some(p) => (
                match p.format {
                    ProfileFormat::Anthropic => WireTransport::AnthropicMessages,
                    ProfileFormat::OpenaiResponses | ProfileFormat::OpenaiCompatible => {
                        WireTransport::OpenaiChat
                    }
                },
                p.headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>(),
                p.session_headers,
                !p.api_key_required,
            ),
            None => {
                let snap_transport = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.provider(provider).map(|p| p.transport()));
                match snap_transport {
                    // Only the dialects the broker can actually speak get an
                    // endpoint; anything else keeps the legacy path rather
                    // than being pointed at a wrong URL.
                    Some(agentcowork_catalog::Transport::AnthropicMessages) => {
                        (WireTransport::AnthropicMessages, Vec::new(), false, false)
                    }
                    Some(agentcowork_catalog::Transport::OpenaiChat) | None => {
                        (WireTransport::OpenaiChat, Vec::new(), false, false)
                    }
                    Some(_) => return None,
                }
            }
        };
        let overlay_session = agentcowork_catalog::opencode_overlay_profiles()
            .iter()
            .find(|p| p.id == provider)
            .map(|p| p.session_headers)
            .unwrap_or(false);
        Some(ProviderEndpoint {
            base_url: base,
            transport,
            headers,
            session_headers: session_headers || overlay_session,
            keyless: keyless
                || self
                    .registry
                    .resolve(provider)
                    .map(|r| matches!(r.auth, Auth::Keyless))
                    .unwrap_or(false),
        })
    }

    /// The providers that can actually execute right now — the only ones the
    /// chat relay needs:
    ///
    /// * providers with a key in the vault (the BYOK set),
    /// * keyless entries (local runtimes + the free overlays),
    /// * the user's own provider profiles.
    ///
    /// Everything else in the catalog is display-only: resolving it would
    /// invent a dial plan for a provider the user never connected.
    fn connected_ids(&self, state: &AppState) -> Vec<String> {
        let mut keyed: Vec<String> = Vec::new();
        if let Ok(vault) = state.vault.lock() {
            if let Ok(k) = KeyRing::new(&vault).providers_with_keys() {
                keyed = k;
            }
        }
        let usable_profiles: Vec<String> = self
            .profiles
            .profiles
            .values()
            .filter(|p| p.is_usable())
            .map(|p| p.id.clone())
            .collect();
        let keyless: Vec<String> = self
            .registry
            .all()
            .filter(|r| matches!(r.auth, Auth::Keyless))
            .map(|r| r.id.clone())
            .chain(
                agentcowork_catalog::opencode_overlay_profiles()
                    .iter()
                    .filter(|p| !p.api_key_required)
                    .map(|p| p.id.clone()),
            )
            .collect();
        connected_ids_from(&keyed, &usable_profiles, &keyless)
    }
}

// P71.2c — `ResolveCtx::is_connected` was deleted with the relay's endpoint map
// (P63's register/retire decision was its only reader). The **connected set**
// itself survives as `connected_ids_from`, because the capability-observation
// sweep still probes exactly what the user connected.

/// Pure union of the three "connected" sources — sorted and de-duplicated. A
/// provider is dialable only if at least one of them names it; the rest of the
/// catalog stays display-only.
fn connected_ids_from(
    keyed: &[String],
    usable_profiles: &[String],
    keyless: &[String],
) -> Vec<String> {
    let mut ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for s in keyed.iter().chain(usable_profiles).chain(keyless) {
        ids.insert(s.as_str());
    }
    ids.into_iter().map(str::to_string).collect()
}

/// The base URL a probe should hit (one catalog read — see [`ResolveCtx`]).
fn probe_base_url(state: &AppState, provider: &str) -> Option<String> {
    ResolveCtx::load(state).base_url(provider)
}

/// Resolve the endpoint for a provider (P55.5) — profile → catalog → registry.
pub fn resolve_endpoint(state: &AppState, provider: &str) -> Option<ProviderEndpoint> {
    ResolveCtx::load(state).endpoint(provider)
}

// P71.2c — `register_endpoint`, `refresh_endpoint_live`, `EndpointAction` and
// `resolve_endpoints` lived here to keep the **chat relay's** resolved-endpoint
// map in step with the connected-provider set (P55.5/P63). That map existed only
// to feed the provider broker, which is deleted with the built-in engine
// (ADR-0005 §2): an external agent owns its own transport, so there is no relay
// dial plan left to reconcile. Endpoint **resolution** below survives because
// the capability probe and the A11 observation write-back still need it — it is
// observability, not execution (`ARCH/ROUTING.md` §5–§6).

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// P56.1 — cheap live status (never parses the 4.6 MB snapshot).
#[tauri::command]
pub fn catalog_status(state: State<'_, AppState>) -> Value {
    state.catalog.status()
}

/// P56.1 — run the refresh job now (`force` skips the staleness check).
#[tauri::command]
pub fn catalog_refresh(state: State<'_, AppState>, force: Option<bool>) -> Value {
    let outcome = state.catalog.refresh(force.unwrap_or(true));
    json!({
        "accepted": outcome.decision.accepted(),
        "persisted": outcome.persisted,
        "summary": outcome.summary(),
        "status": state.catalog.status(),
    })
}

/// P56.1 — the configurable cadence (clamped to 1–24h by the store).
#[tauri::command]
pub fn catalog_set_interval(state: State<'_, AppState>, hours: u64) -> Result<Value, String> {
    let clamped = agentcowork_catalog::refresh_interval_secs(Some(hours));
    state
        .catalog
        .store
        .save_settings(&agentcowork_catalog::CatalogSettings {
            refresh_hours: Some(clamped / 3600),
        })?;
    Ok(json!({
        "ok": true,
        "intervalHours": clamped / 3600,
        "requestedHours": hours,
        "clamped": clamped != hours * 3600,
    }))
}

/// Drop the cached snapshot (Settings → clear). Settings are kept.
#[tauri::command]
pub fn catalog_clear(state: State<'_, AppState>) -> Result<Value, String> {
    state.catalog.store.clear()?;
    Ok(json!({ "ok": true, "status": state.catalog.status() }))
}

/// P56.2 — the merged Settings → Providers list.
#[tauri::command]
pub fn catalog_providers(state: State<'_, AppState>) -> Value {
    let rows = provider_rows(&state);
    json!({
        "providers": rows,
        "status": state.catalog.status(),
        "profiles": profile_store().list(),
    })
}

/// P56.7 — the whole model table for one provider. `opencode-free` returns the
/// keyless free subset (regex + `big-pickle`), which is the only honest list
/// for that row.
#[tauri::command]
pub fn catalog_provider_models(state: State<'_, AppState>, provider: String) -> Value {
    let snap = state.catalog.store.load();
    let mut rows: Vec<Value> = Vec::new();
    let mut free_subset: Option<Vec<String>> = None;

    if let Some(s) = &snap {
        if let Some(p) = s.provider(&provider) {
            // Computed once, used by the filter below and reported in the
            // payload. The previous shape re-borrowed it *inside* the loop
            // through `free_subset.as_ref().unwrap()` — a panic surface that
            // bought nothing.
            let free = s.opencode_free_models();
            let free_only = provider == "opencode-free";
            for m in p.model_rows() {
                if free_only && !free.iter().any(|f| f == &m.id) {
                    continue;
                }
                rows.push(json!({
                    "id": m.id,
                    "name": if m.name.is_empty() { m.id.clone() } else { m.name.clone() },
                    "description": m.description,
                    "family": m.family,
                    "context": m.limit.context,
                    "output": m.limit.output,
                    "priceInput": m.cost.input,
                    "priceOutput": m.cost.output,
                    "cacheRead": m.cost.cache_read,
                    "cacheWrite": m.cost.cache_write,
                    "reasoning": m.reasoning,
                    "toolCall": m.tool_call,
                    "structuredOutput": m.structured_output,
                    "attachment": m.attachment,
                    "temperature": m.temperature,
                    "images": m.modalities.accepts_image(),
                    "pdf": m.modalities.accepts_pdf(),
                    "openWeights": m.open_weights,
                    "knowledge": m.knowledge,
                    "releaseDate": m.release_date,
                    "lastUpdated": m.last_updated,
                    "status": m.status,
                }));
            }
            free_subset = Some(free);
        }
    }

    // A user profile may carry its own model list (P56.4) — that is the
    // authoritative list for that endpoint, not the catalog's.
    let profile_models: Vec<Value> = profile_store()
        .get(&provider)
        .map(|p| {
            p.models
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "name": if m.name.is_empty() { m.id.clone() } else { m.name.clone() },
                        "context": m.context,
                        "output": m.output,
                        "free": m.free,
                        "fromProfile": true,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "provider": provider,
        "models": rows,
        "profileModels": profile_models,
        "count": rows.len() + profile_models.len(),
        "live": snap.is_some(),
        "freeSubset": free_subset,
    })
}

/// P56.3 — the activate screen's `MetadataOnly` probe.
///
/// `GET {base}/models` (Anthropic: `x-api-key` + `anthropic-version`; everyone
/// else: `Authorization: Bearer`; keyless: no auth header at all). Read-only:
/// nothing is persisted here, so a failed probe cannot leave a half-configured
/// provider behind. The UI persists on a tick, with the `verifiedAt` stamp.
#[tauri::command]
pub fn provider_probe(state: State<'_, AppState>, provider: String, key: Option<String>) -> Value {
    probe_provider(&state, &provider, key.as_deref())
}

/// The plain-function form of [`provider_probe`], so other modules (P63's
/// per-agent backend cards) can run the same read-only probe without holding a
/// Tauri `State`. Behaviour is identical — this is pure extraction.
pub fn probe_provider(state: &AppState, provider: &str, key: Option<&str>) -> Value {
    let Some(base) = probe_base_url(state, provider) else {
        return json!({
            "ok": false,
            "status": 0,
            "message": "no endpoint in the catalog for this provider — add a base URL",
            "models": 0,
        });
    };
    let endpoint = resolve_endpoint(state, provider);
    let is_anthropic = endpoint
        .as_ref()
        .map(|e| e.transport == WireTransport::AnthropicMessages)
        .unwrap_or(false);
    let headers = endpoint
        .as_ref()
        .map(|e| e.headers.clone())
        .unwrap_or_default();
    let probe = agentcowork_catalog::probe_models_endpoint(&base, is_anthropic, &headers, key);
    // P44.4 write-back — this is the observation, and it used to be dropped
    // here. Recording it durably (keyed by canonical id) is what lets the
    // registry, the routing feed and the UI report *observed* truth instead of
    // catalog metadata: `verifiedAt` now populates from a real probe, and a
    // failed probe lands as honest error history rather than as silence.
    //
    // It is a reachability observation: it records that the endpoint answered
    // and how many models it served, and it confirms **no** hard capability —
    // `trusted_capabilities` stays empty for it, so nothing becomes routable on
    // the strength of "it responded".
    record_observation(provider, &probe);

    json!({
        "ok": probe.ok,
        "status": probe.status,
        "message": probe.message,
        "models": probe.models,
        "url": probe.url,
    })
}

/// Persist one probe result as the provider's latest observation.
///
/// Best-effort by design: a probe that worked but could not be written down
/// must still return its result to the caller — the observation is an
/// improvement to future reads, never a precondition for this one. A write
/// failure is reported on stderr (never swallowed silently) and does not turn a
/// successful probe into an error.
fn record_observation(provider: &str, probe: &EndpointProbe) {
    let dir = agentcowork_core::default_data_dir();
    // Canonicalize against the same observed registry the reads use.
    let registry = observed_registry_in(&dir);
    record_observation_in(&dir, &registry, provider, probe, now_ms().to_string());
}

/// The testable core of [`record_observation`]: the stamp is passed in so a
/// test can assert an exact value instead of racing the clock, and the registry
/// is passed in so a sweep can canonicalize against the one it already built
/// rather than rebuilding a 200-provider registry per provider.
fn record_observation_in(
    dir: &std::path::Path,
    registry: &ProviderRegistry,
    provider: &str,
    probe: &EndpointProbe,
    stamp: String,
) {
    let obs = ProviderObservation::from_metadata_probe(stamp, probe);
    // Canonicalize through the same registry the reads use, so `claude` and
    // `anthropic` cannot end up as two rows with two different truths.
    if let Err(e) = observation_store_in(dir).record_resolved(registry, provider, obs) {
        eprintln!("provider observation not persisted for {provider}: {e}");
    }
}

// ---- P44.4 — vault-mediated probing (the boot sweep) ----------------------

/// How long one vault-mediated probe may take.
///
/// Bounded because the sweep is sequential: one dead endpoint must not hold the
/// pass open. Shorter than the shell's user-key probe (20s) because nobody is
/// waiting on this one.
const VAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// True while a sweep is running, so two sweeps never probe the same providers
/// concurrently (boot and unlock can race).
static SWEEP_IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Reset the in-flight flag even if the sweep thread panics — a panic must not
/// permanently disable observation.
struct SweepGuard;

impl Drop for SweepGuard {
    fn drop(&mut self) {
        SWEEP_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// **P44.4 — probe a provider with the credential the vault already holds.**
///
/// The user-key path ([`probe_provider`]) needs a key in hand, which only the
/// Settings verify flow has. This is the same observation without one: the
/// broker resolves the credential from the ring, performs one
/// `GET {base}/models` against **the provider's own endpoint**, and reports what
/// was answered. The credential never enters this module.
///
/// `None` means **no observation**, which is not the same as a failed one: no
/// resolvable endpoint, no usable credential (no keys, all suspended, exhausted),
/// or an endpoint that would send the key in cleartext. Recording those as
/// failures would turn "we could not check" into a claim about the provider —
/// exactly the fabrication this path exists to prevent. The reason is logged.
///
/// The vault lock is held across the request (the same posture as the chat
/// path, whose broker guard must outlive the call). The 8-second bound is what
/// keeps that acceptable.
pub fn probe_provider_vault(
    state: &AppState,
    provider: &str,
    endpoint: ProviderEndpoint,
) -> Option<EndpointProbe> {
    let probe = {
        let vault = state.vault.lock().ok()?;
        Broker::new(&vault)
            .with_endpoint(provider, endpoint)
            .probe_models(provider, VAULT_PROBE_TIMEOUT)
    };
    observation_from_probe(provider, probe)
}

/// Turn a broker probe into an observation — or into **no observation**.
///
/// Split out from [`probe_provider_vault`] so the failure policy is testable
/// without an `AppState`: a broker error (no keys, every key suspended or
/// exhausted, an insecure endpoint) yields `None`, never a fabricated failed
/// observation. "We could not check" must not become a claim about the provider.
fn observation_from_probe(
    provider: &str,
    result: Result<agentcowork_vault::ModelsProbe, agentcowork_vault::BrokerError>,
) -> Option<EndpointProbe> {
    match result {
        Ok(p) => Some(endpoint_probe_result(
            p.url,
            p.status,
            &p.body,
            p.error.as_deref(),
        )),
        Err(e) => {
            eprintln!("agentcowork-catalog: not observing {provider}: {e}");
            None
        }
    }
}

/// The sweep body: observe every **connected** provider once.
///
/// Returns how many observations were recorded. Keyless providers and the
/// user's own profiles are included by construction — `connected_ids` is the
/// same set the chat relay gets a dial plan for, so a provider the app will
/// actually call is a provider worth observing.
fn sweep_connected_providers(state: &AppState) -> usize {
    // One context for the whole pass: the registry, snapshot and profiles are
    // read once instead of once per provider.
    let ctx = ResolveCtx::load(state);
    let targets: Vec<(String, ProviderEndpoint)> = ctx
        .connected_ids(state)
        .into_iter()
        .filter_map(|id| ctx.endpoint(&id).map(|ep| (id, ep)))
        .collect();
    let mut recorded = 0;
    let stamp = now_ms().to_string();
    for (id, endpoint) in targets {
        if let Some(probe) = probe_provider_vault(state, &id, endpoint) {
            record_observation_in(
                &agentcowork_core::default_data_dir(),
                &ctx.registry,
                &id,
                &probe,
                stamp.clone(),
            );
            recorded += 1;
        }
    }
    recorded
}

/// The **boot** hook: at most one observation pass per process.
///
/// `connect_chat_relay` runs again on every sidecar respawn, so without this a
/// crash loop would re-dial every connected provider each time. An explicit
/// unlock/setup sweep is still allowed afterwards (the connected set genuinely
/// changed, and that pass is what observes the keyed providers).
pub fn spawn_boot_observation_sweep(app: tauri::AppHandle) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| spawn_observation_sweep(app));
}

/// Observe the connected set once, off the UI thread.
///
/// Network I/O in a background thread with a bounded per-provider timeout, and
/// never two sweeps at once. Called at boot and again whenever the connected set
/// materially changes (unlocking the vault brings every keyed provider into it).
pub fn spawn_observation_sweep(app: tauri::AppHandle) {
    if SWEEP_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    // (the sweep itself continues below)
    std::thread::spawn(move || {
        let _guard = SweepGuard;
        let state = app.state::<AppState>();
        let observed = sweep_connected_providers(&state);
        eprintln!("agentcowork-catalog: observed {observed} connected provider(s)");
    });
}

/// P55.6/P56.4 — the durable provider profiles (no secrets).
#[tauri::command]
pub fn provider_profiles_list() -> Value {
    json!({ "profiles": profile_store().list() })
}

/// P55.6/P56.4 — create or replace a profile (custom inference form / base-URL
/// override / NVIDIA NIM). Rejects a base URL that already carries a request
/// path, because the broker appends the dialect path itself.
#[tauri::command]
/// P71.2c — the profile is durable provider **metadata** now, not a dial plan:
/// the relay map it used to feed is gone, so the command no longer needs the
/// shell state (the parameter is kept because Tauri matches renderer arguments
/// by name against the command signature).
pub fn provider_profile_upsert(
    _state: State<'_, AppState>,
    profile: Value,
) -> Result<Value, String> {
    let format = match profile.get("format").and_then(|f| f.as_str()) {
        Some(f) => ProfileFormat::parse(f)?,
        None => ProfileFormat::OpenaiCompatible,
    };
    let mut headers = BTreeMap::new();
    if let Some(obj) = profile.get("headers").and_then(|h| h.as_object()) {
        for (k, v) in obj {
            if let Some(v) = v.as_str() {
                headers.insert(k.clone(), v.to_string());
            }
        }
    }
    let models: Vec<ProfileModel> = profile
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?.to_string();
                    Some(ProfileModel {
                        name: m
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or(&id)
                            .to_string(),
                        context: m.get("context").and_then(|c| c.as_u64()).unwrap_or(0),
                        output: m.get("output").and_then(|c| c.as_u64()).unwrap_or(0),
                        free: m.get("free").and_then(|f| f.as_bool()).unwrap_or(false),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let id = profile
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();
    let name = profile
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let saved = profile_store().upsert(ProviderProfile {
        id: if id.trim().is_empty() {
            ProviderProfile::slug(&name)
        } else {
            id
        },
        name,
        format,
        base_url: profile
            .get("baseUrl")
            .or_else(|| profile.get("base_url"))
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string(),
        // `keyRequired` defaults to false: a keyless local endpoint is valid
        // and is the common case for the custom form (P56.4).
        api_key_required: profile
            .get("keyRequired")
            .and_then(|k| k.as_bool())
            .unwrap_or(false),
        headers,
        body: profile.get("body").cloned().unwrap_or(json!({})),
        temperature: profile.get("temperature").and_then(|t| t.as_f64()),
        models,
        source: ProfileSource::UserConfig,
        verified_at: profile
            .get("verifiedAt")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        verified_models: profile
            .get("verifiedModels")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        session_headers: profile
            .get("sessionHeaders")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
    })?;
    Ok(json!({ "ok": true, "profile": saved }))
}

#[tauri::command]
pub fn provider_profile_remove(_state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let removed = profile_store().remove(&id)?;
    Ok(json!({ "ok": true, "removed": removed, "id": id }))
}

/// P56.5 — the shipped NVIDIA NIM overlay as a one-click profile.
#[tauri::command]
pub fn provider_nim_profile(base_url: Option<String>) -> Value {
    let mut p = agentcowork_catalog::nvidia_nim_profile();
    if let Some(url) = base_url {
        if !url.trim().is_empty() {
            p.base_url = url;
        }
    }
    json!(p)
}

// ---------------------------------------------------------------------------
// P14.5 — the pure merge gate (kept: it is the documented per-provider path)
// ---------------------------------------------------------------------------

/// The documented refresh plan — what one per-provider sync run would do.
#[tauri::command]
pub fn catalog_sync_plan() -> Value {
    let modules: Vec<Value> = agentcowork_catalog::SYNC_MODULES
        .iter()
        .map(|s| {
            json!({
                "provider": s.provider,
                "source": s.source,
                "writableFields": s.writable_fields,
            })
        })
        .collect();
    json!({
        "modules": modules,
        "plan": agentcowork_catalog::refresh_plan(),
        "liveSource": agentcowork_catalog::MODELS_DEV_API_URL,
        "defaultIntervalHours": DEFAULT_REFRESH_SECS / 3600,
    })
}

/// Run the pure merge gate over a caller-supplied baseline + fetch payload.
#[tauri::command]
pub fn catalog_sync_refresh(
    baseline_json: String,
    fetched_json: String,
    known_labs: Vec<String>,
) -> Result<Value, String> {
    let baseline: Vec<agentcowork_catalog::ModelEntry> = serde_json::from_str(&baseline_json)
        .map_err(|e| format!("catalog refresh: bad baseline JSON: {e}"))?;
    let fetched: Vec<agentcowork_catalog::ModelEntry> = serde_json::from_str(&fetched_json)
        .map_err(|e| format!("catalog refresh: bad fetched JSON: {e}"))?;
    let labs: Vec<&str> = known_labs.iter().map(String::as_str).collect();
    let report = agentcowork_catalog::merge_refresh(&baseline, &fetched, &labs);
    Ok(json!({
        "accepted": report.accepted,
        "fetchedProviders": report.fetched_providers,
        "acceptedEntries": report.accepted_entries,
        "rejectedProviders": report.rejected_providers,
        "findings": report
            .findings
            .iter()
            .map(|f| json!({
                "severity": match f.severity {
                    agentcowork_catalog::Severity::Error => "error",
                    agentcowork_catalog::Severity::Warning => "warning",
                },
                "message": f.message,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// The background job the shell spawns at boot (P56.1): refresh when stale,
/// then re-check every minute so a Settings cadence change takes effect
/// without a restart. Never blocks the UI thread.
pub fn spawn_refresh_job(catalog: Arc<CatalogState>) {
    std::thread::spawn(move || {
        // Boot leg: stale (or empty) → fetch now.
        let _ = catalog.refresh(false);
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let meta = catalog.store.load_meta();
            let interval = catalog.store.refresh_interval_secs();
            let stale = meta
                .as_ref()
                .map(|m| agentcowork_catalog::is_stale(m.fetched_at, now_ms(), interval))
                .unwrap_or(true);
            if stale {
                let _ = catalog.refresh(false);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    // P71.2c — the P63 endpoint-lifecycle tests (`endpoint_action` /
    // `EndpointAction`) are deleted with the relay's endpoint map they decided
    // on: AgentCowork no longer holds a provider dial plan, so there is nothing to
    // register or retire. `connected_ids_from` below is still tested because the
    // **observation** sweep keys on the connected set.
    use super::{
        connected_ids_from, observation_file_in, observation_from_probe, observed_registry_in,
        record_observation_in,
    };
    use agentcowork_catalog::{endpoint_probe_result, ProviderObservation};

    /// P63.2 — the relay resolves only the connected set. A provider is
    /// dialable if it is vault-keyed, keyless, or the user profiled it; a
    /// catalog row that is none of those must stay absent.
    #[test]
    fn connected_set_is_the_union_of_the_three_sources() {
        let keyed = vec!["anthropic".to_string(), "openai".to_string()];
        let profiles = vec!["my-vps".to_string()];
        let keyless = vec!["ollama".to_string(), "opencode-free".to_string()];
        let ids = connected_ids_from(&keyed, &profiles, &keyless);
        assert_eq!(
            ids,
            vec!["anthropic", "my-vps", "ollama", "openai", "opencode-free"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn connected_set_is_deduplicated_and_sorted() {
        let keyed = vec!["openai".to_string(), "anthropic".to_string()];
        let profiles = vec!["openai".to_string()];
        let keyless = vec!["anthropic".to_string()];
        assert_eq!(
            connected_ids_from(&keyed, &profiles, &keyless),
            vec!["anthropic", "openai"]
        );
    }

    #[test]
    fn an_unconnected_catalog_row_is_never_dialable() {
        // `deepseek` exists in the catalog but has no key, no profile and is
        // not keyless — so it is display-only and must not resolve.
        let ids = connected_ids_from(&["anthropic".to_string()], &[], &[]);
        assert!(!ids.iter().any(|id| id == "deepseek"));
    }

    #[test]
    fn an_empty_device_connects_nothing() {
        assert!(connected_ids_from(&[], &[], &[]).is_empty());
    }

    // ---- P44.4 write-back (the C1 gap): a probe must become readable truth ----

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-catalog-cmds-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn probe(ok: bool, status: u16, models: usize) -> agentcowork_catalog::EndpointProbe {
        agentcowork_catalog::EndpointProbe {
            ok,
            status,
            message: if ok { "ok".into() } else { "nope".into() },
            models,
            url: "https://api.anthropic.com/v1/models".into(),
        }
    }

    /// The whole point of the change: a live probe is recorded durably and then
    /// read back as the registry's observed truth — including through the
    /// provider's alias, and surviving a rebuild from `base_registry()`.
    #[test]
    fn a_recorded_probe_becomes_the_registrys_observed_truth() {
        let dir = temp_dir("writeback");
        record_observation_in(
            &dir,
            &observed_registry_in(&dir),
            "claude",
            &probe(true, 200, 3),
            "1700".into(),
        );

        // Durable, keyed by canonical id even though the probe named an alias.
        let file = observation_file_in(&dir);
        assert_eq!(file.providers.len(), 1);
        assert!(file.providers.contains_key("anthropic"));
        let reach = file.reachability("anthropic").expect("reachability");
        assert!(reach.ok);
        assert_eq!(reach.model_count, 3);
        assert_eq!(reach.observed_at, "1700");

        // Replayed onto a freshly built registry — this is what `verifiedAt`
        // and the routing feed read.
        let reg = observed_registry_in(&dir);
        assert_eq!(reg.verified_at("claude"), Some("1700"));
        assert_eq!(reg.verified_at("anthropic"), Some("1700"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second probe replaces the first rather than accumulating, so the stamp
    /// always describes the latest thing that actually happened.
    #[test]
    fn the_latest_probe_is_the_observation() {
        let dir = temp_dir("latest");
        let registry = observed_registry_in(&dir);
        record_observation_in(
            &dir,
            &registry,
            "anthropic",
            &probe(true, 200, 3),
            "1".into(),
        );
        record_observation_in(
            &dir,
            &registry,
            "anthropic",
            &probe(true, 200, 5),
            "2".into(),
        );
        let file = observation_file_in(&dir);
        assert_eq!(file.providers.len(), 1);
        assert_eq!(file.get("anthropic").unwrap().model_count, 5);
        assert_eq!(
            observed_registry_in(&dir).verified_at("anthropic"),
            Some("2")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed probe is real history but must never become a verification.
    #[test]
    fn a_failed_probe_is_history_and_not_a_verification() {
        let dir = temp_dir("failed");
        record_observation_in(
            &dir,
            &observed_registry_in(&dir),
            "anthropic",
            &probe(false, 401, 0),
            "9".into(),
        );

        let file = observation_file_in(&dir);
        let reach = file.reachability("anthropic").expect("reachability");
        assert!(!reach.ok);
        assert_eq!(reach.status, 401);
        assert_eq!(file.verified_count(), 0);
        assert_eq!(file.failed_count(), 1);

        // The registry is left unverified: "we tried" is not "it answered".
        assert!(observed_registry_in(&dir)
            .verified_at("anthropic")
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- P44.4 — the vault-mediated probe (keyed providers, no key in hand) --

    /// The two halves meet here: the broker returns a status + body and never
    /// parses it, and this module shapes the result with the catalog's own rule
    /// — so the shell's user-key probe and the vault-mediated one cannot
    /// disagree about what they observed.
    #[test]
    fn a_vault_probe_is_shaped_by_the_catalogs_own_rule() {
        let probe = endpoint_probe_result(
            "https://api.openai.com/v1/models".into(),
            200,
            r#"{"data":[{"id":"a"},{"id":"b"},{"id":"c"}]}"#,
            None,
        );
        assert!(probe.ok);
        assert_eq!(probe.status, 200);
        assert_eq!(probe.models, 3);
        assert_eq!(probe.message, "3 models advertised");

        // The shaped result is exactly what gets persisted durably.
        let obs = ProviderObservation::from_metadata_probe("1", &probe);
        assert!(obs.ok);
        assert_eq!(obs.model_count, 3);
        assert_eq!(obs.status, 200);

        // An answered rejection is an observation, not a failure to observe…
        let rejected = endpoint_probe_result("u".into(), 401, r#"{"error":"bad key"}"#, None);
        assert!(!rejected.ok);
        assert_eq!(rejected.status, 401);
        assert_eq!(rejected.models, 0);

        // …and nothing answered is status 0 — no HTTP code is invented.
        let dead = endpoint_probe_result("u".into(), 0, "", Some("connection refused"));
        assert_eq!(dead.status, 0);
        assert!(dead.message.contains("connection refused"));
    }

    /// The failure policy that keeps this honest: when the broker cannot probe
    /// (no credential, suspended key, insecure endpoint) there is **no
    /// observation** — not a failed one. A failed observation would be durably
    /// recorded as "this provider did not answer", which is a lie about a
    /// provider nobody asked.
    #[test]
    fn a_broker_error_produces_no_observation_rather_than_a_failed_one() {
        let err = agentcowork_vault::BrokerError::InsecureEndpoint("openai".into());
        assert!(observation_from_probe("openai", Err(err)).is_none());

        let no_keys = agentcowork_vault::BrokerError::AllKeysExhausted("openai".into());
        assert!(observation_from_probe("openai", Err(no_keys)).is_none());
    }

    /// …and when the broker *did* probe, the observation carries the live facts:
    /// the endpoint's own URL, its status, and the body it returned.
    #[test]
    fn a_successful_broker_probe_becomes_the_observation() {
        let probe = observation_from_probe(
            "claude",
            Ok(agentcowork_vault::ModelsProbe {
                ok: true,
                status: 200,
                url: "https://api.anthropic.com/v1/models".into(),
                body: r#"{"models":{"claude-a":{},"claude-b":{}}}"#.into(),
                error: None,
            }),
        )
        .expect("an answered probe is an observation");
        assert!(probe.ok);
        assert_eq!(probe.status, 200);
        assert_eq!(probe.models, 2);
        assert_eq!(probe.url, "https://api.anthropic.com/v1/models");
    }

    /// The observed registry is additive: identity, aliases and transports are
    /// the vendored layer's, never rewritten by an observation.
    #[test]
    fn observing_a_provider_does_not_rewrite_its_identity() {
        let dir = temp_dir("identity");
        record_observation_in(
            &dir,
            &observed_registry_in(&dir),
            "claude",
            &probe(true, 200, 1),
            "3".into(),
        );
        let reg = observed_registry_in(&dir);
        let rec = reg.get("anthropic").expect("record");
        assert_eq!(rec.name, "Anthropic");
        assert_eq!(
            rec.transport,
            Some(agentcowork_catalog::Transport::AnthropicMessages)
        );
        assert!(rec.aliases.iter().any(|a| a == "claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
