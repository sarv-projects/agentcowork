//! BYOK key-ring surface — list / add / remove keys in the SQLCipher vault.

use agentcowork_vault::{KeyRing, KeySpec, KeyStatus};
use tauri::State;

use crate::AppState;

#[tauri::command]
pub fn vault_keys_list(
    state: State<'_, AppState>,
    provider: Option<String>,
) -> Result<serde_json::Value, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let ring = KeyRing::new(&vault);
    let mut all = Vec::new();
    let providers = if let Some(p) = provider {
        vec![p]
    } else {
        // P50.2.6/P50.3.6 — enumerate the live vault key set instead of a
        // hardcoded provider list, so keys for any provider (xai, mistral,
        // togetherai, cerebras, zai, …) are visible to the gate, the
        // NoProvider card, and the routing feed. Locked/empty vault ⇒ empty.
        ring.providers_with_keys().unwrap_or_default()
    };
    for p in providers {
        if let Ok(rows) = ring.list(&p) {
            all.extend(rows);
        }
    }
    Ok(serde_json::json!({ "keys": all }))
}

/// **P55.6/P56.3** — add (or re-add) a provider key.
///
/// The key itself goes to the vault ring; the *endpoint* half (`baseUrl`,
/// `format`) goes to the durable provider-profile store. Both are needed for
/// a custom provider to actually work: before this, `vault_key_add` stored the
/// key and **dropped the URL**, so a self-hosted/proxy endpoint could be
/// entered in Settings and never be used. `verifiedAt` is the activate
/// screen's probe stamp (P56.3) — set only after a real `GET {api}/models`
/// answered, so a green tick always has evidence behind it.
// The argument list is the IPC contract the Settings → Keys form posts; grouping
// it into a struct would only move the same fields behind one more type and
// break every existing caller for no gain.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn vault_key_add(
    state: State<'_, AppState>,
    provider: String,
    key_id: String,
    value: String,
    priority: Option<u32>,
    base_url: Option<String>,
    format: Option<String>,
    verified_at: Option<String>,
    verified_models: Option<u32>,
    session_headers: Option<bool>,
) -> Result<serde_json::Value, String> {
    if value.trim().is_empty() {
        return Err("key value required".into());
    }
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let ring = KeyRing::new(&vault);
    let handle = ring
        .add_key(KeySpec {
            provider: provider.clone(),
            key_id: key_id.clone(),
            value: value.into_bytes(),
            status: KeyStatus::Primary,
            model_filter: Vec::new(),
            priority: priority.unwrap_or(100),
            daily_token_cap: None,
            daily_cost_cap: None,
        })
        .map_err(|e| e.to_string())?;
    drop(vault);

    // Persist the endpoint half when the caller supplied one. An existing
    // profile is merged field-by-field so a re-key does not wipe the models
    // list or headers the user already configured.
    let mut profile_saved = false;
    let has_base = base_url
        .as_ref()
        .map(|u| !u.trim().is_empty())
        .unwrap_or(false);
    if has_base || format.is_some() {
        let store = agentcowork_catalog::ProfileStore::in_dir(agentcowork_core::default_data_dir());
        let mut profile = store.get(&provider).unwrap_or_default();
        profile.id.clone_from(&provider);
        if profile.name.trim().is_empty() {
            profile.name = provider.clone();
        }
        if let Some(url) = base_url.filter(|u| !u.trim().is_empty()) {
            profile.base_url = url;
        }
        if let Some(f) = format.as_deref() {
            profile.format = agentcowork_catalog::ProfileFormat::parse(f)?;
        }
        profile.api_key_required = true;
        profile.source = agentcowork_catalog::ProfileSource::UserConfig;
        if let Some(stamp) = verified_at {
            profile.verified_at = Some(stamp);
        }
        if let Some(n) = verified_models {
            profile.verified_models = n as usize;
        }
        if let Some(s) = session_headers {
            profile.session_headers = s;
        }
        store.upsert(profile)?;
        profile_saved = true;
    }

    // P71.2c — this used to register the provider's endpoint on the live relay
    // (P63). The relay no longer brokers provider transport (ADR-0005 §2), so
    // there is no dial plan to reconcile; the key's effect is the vault record
    // plus the capability observation the probe writes back (A11).

    Ok(serde_json::json!({
        "ok": true,
        "provider": provider,
        "keyId": key_id,
        "opaqueHandle": handle,
        "profileSaved": profile_saved,
    }))
}

#[tauri::command]
pub fn vault_key_remove(
    state: State<'_, AppState>,
    provider: String,
    key_id: String,
) -> Result<serde_json::Value, String> {
    {
        let vault = state.vault.lock().map_err(|e| e.to_string())?;
        let ring = KeyRing::new(&vault);
        ring.delete_key(&provider, &key_id)
            .map_err(|e| e.to_string())?;
    }
    Ok(serde_json::json!({ "ok": true, "provider": provider, "keyId": key_id }))
}

/// P51.1 — rotate one provider key in place (mints a new opaque handle,
/// zeroes the failure/cooldown counters). The new secret travels once over
/// the already-trusted webview→shell invoke boundary into the vault.
#[tauri::command]
pub fn vault_key_rotate(
    state: State<'_, AppState>,
    provider: String,
    key_id: String,
    value: String,
) -> Result<serde_json::Value, String> {
    if value.trim().is_empty() {
        return Err("refusing to rotate to an empty key".to_string());
    }
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let ring = KeyRing::new(&vault);
    let handle = ring
        .rotate_key(&provider, &key_id, value.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "ok": true,
        "provider": provider,
        "keyId": key_id,
        "opaqueHandle": handle,
    }))
}
