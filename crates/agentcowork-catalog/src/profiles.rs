//! P55.6 + P56.3/.4 — user-configurable provider profiles.
//!
//! The gap this closes: `vault_key_add` stored an API key and **discarded the
//! base URL**, so a custom endpoint (a proxy, a self-hosted gateway, a NIM
//! server, or any of the `api`-less models.dev providers) could be entered in
//! Settings and never actually be used. A profile is the durable half; the
//! vault key is the secret half. Neither contains the other's data.
//!
//! Shape is OpenCode-shaped (P56.4) so the same record serves the custom
//! inference form *and* the verification stamp the activate screen writes:
//!
//! ```text
//! id · name · format(openai-compatible|anthropic|openai-responses)
//! base_url (to /v1 — never with /chat/completions appended)
//! api_key_required (false is valid: keyless local)
//! headers{} · body{} (merge) · temperature?
//! models[{id,name,limit.context,limit.output}]
//! source (user-config | catalog | overlay) · verified_at? · verified_models
//! ```
//!
//! One file (`<data_dir>/providers.json`), atomic writes, and a resolution
//! order the broker follows: **user-config profile → live catalog `api` →
//! shell default**. A profile never carries a secret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::provider::Transport;

/// Wire format for a profile (the P56.4 dropdown).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileFormat {
    /// `POST {base}/chat/completions` (the default).
    #[default]
    OpenaiCompatible,
    /// `POST {base}/messages` (Anthropic Messages).
    Anthropic,
    /// The OpenAI Responses API (`POST {base}/responses`).
    OpenaiResponses,
}

impl ProfileFormat {
    pub fn transport(self) -> Transport {
        match self {
            ProfileFormat::Anthropic => Transport::AnthropicMessages,
            ProfileFormat::OpenaiResponses => Transport::CodexResponses,
            ProfileFormat::OpenaiCompatible => Transport::OpenaiChat,
        }
    }

    /// Parse the string the UI sends; unknown values are rejected (never
    /// silently coerced to the default).
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "openai-compatible" | "openai" | "" => Ok(ProfileFormat::OpenaiCompatible),
            "anthropic" | "anthropic-messages" => Ok(ProfileFormat::Anthropic),
            "openai-responses" | "responses" => Ok(ProfileFormat::OpenaiResponses),
            other => Err(format!("unknown provider format `{other}`")),
        }
    }
}

/// Where the profile came from (P55.6 provenance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileSource {
    /// Entered by the user (custom inference form / base-URL override).
    #[default]
    UserConfig,
    /// Mirrored from the live models.dev catalog.
    Catalog,
    /// A shipped overlay (OpenCode Zen/Go/Free, NVIDIA NIM, local).
    Overlay,
}

/// One model row a profile advertises (`{id,name,limit.context,limit.output}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProfileModel {
    pub id: String,
    pub name: String,
    pub context: u64,
    pub output: u64,
    /// True for the keyless free subset (P56.6 OpenCode Free).
    pub free: bool,
}

/// A durable provider profile — endpoint + shape, never a secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub format: ProfileFormat,
    /// Base URL *to `/v1`* — `/chat/completions` is appended by the broker.
    pub base_url: String,
    /// `false` is valid and meaningful: keyless local / keyless free pools.
    pub api_key_required: bool,
    pub headers: BTreeMap<String, String>,
    /// Extra JSON merged into the request body (provider quirk knobs).
    pub body: serde_json::Value,
    pub temperature: Option<f64>,
    pub models: Vec<ProfileModel>,
    pub source: ProfileSource,
    /// Set by a successful P56.3 activate-screen probe (`GET {api}/models`).
    pub verified_at: Option<String>,
    /// How many models the probe actually observed (honest: 0 = unknown).
    pub verified_models: usize,
    /// True when this provider needs the per-conversation session headers
    /// (OpenCode Zen/Go/Free).
    pub session_headers: bool,
}

impl ProviderProfile {
    /// Normalize a slug the way the UI does (`My Proxy` → `my-proxy`).
    pub fn slug(raw: &str) -> String {
        let s: String = raw
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let s = s.trim_matches('-').to_string();
        if s.is_empty() {
            "custom".to_string()
        } else {
            s
        }
    }

    /// The base URL without a trailing slash, or `None` when unset.
    pub fn normalized_base_url(&self) -> Option<String> {
        let t = self.base_url.trim().trim_end_matches('/');
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }

    /// `base_url` **as entered** must never contain a request path — the
    /// broker appends it. Catching this here is the difference between an
    /// honest 404 and a silent double-path (`/v1/chat/completions/chat/
    /// completions`).
    pub fn base_url_error(&self) -> Option<String> {
        let url = self.normalized_base_url()?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Some("base URL must start with http:// or https://".to_string());
        }
        for bad in [
            "/chat/completions",
            "/completions",
            "/messages",
            "/responses",
        ] {
            if url.ends_with(bad) {
                return Some(format!(
                    "base URL must stop at the version root — remove `{bad}`"
                ));
            }
        }
        None
    }

    pub fn is_usable(&self) -> bool {
        self.base_url_error().is_none() && self.normalized_base_url().is_some()
    }
}

/// The whole profile file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderProfilesFile {
    pub profiles: BTreeMap<String, ProviderProfile>,
}

/// Durable profile store (`<data_dir>/providers.json`).
#[derive(Debug, Clone)]
pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The conventional location inside a data dir.
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        Self::new(dir.as_ref().join("providers.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> ProviderProfilesFile {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or_default()
    }

    pub fn list(&self) -> Vec<ProviderProfile> {
        self.load().profiles.into_values().collect()
    }

    pub fn get(&self, id: &str) -> Option<ProviderProfile> {
        self.load().profiles.get(id).cloned()
    }

    /// Insert or replace a profile. Returns the stored record.
    pub fn upsert(&self, profile: ProviderProfile) -> Result<ProviderProfile, String> {
        if let Some(err) = profile.base_url_error() {
            return Err(err);
        }
        let id = if profile.id.trim().is_empty() {
            ProviderProfile::slug(&profile.name)
        } else {
            ProviderProfile::slug(&profile.id)
        };
        let mut stored = profile;
        stored.id = id;
        if stored.name.trim().is_empty() {
            stored.name = stored.id.clone();
        }
        if stored.api_key_required && stored.base_url.trim().is_empty() {
            return Err("base URL is required for a provider that needs a key".to_string());
        }
        let mut file = self.load();
        file.profiles.insert(stored.id.clone(), stored.clone());
        self.save(&file)?;
        Ok(stored)
    }

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut file = self.load();
        let removed = file.profiles.remove(id).is_some();
        if removed {
            self.save(&file)?;
        }
        Ok(removed)
    }

    pub fn save(&self, file: &ProviderProfilesFile) -> Result<(), String> {
        let body = serde_json::to_string_pretty(file)
            .map_err(|e| format!("provider profiles: serialize: {e}"))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("provider profiles: create {}: {e}", dir.display()))?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, body)
            .map_err(|e| format!("provider profiles: write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("provider profiles: rename {}: {e}", tmp.display()))
    }
}

/// The three shipped OpenCode rows (P56.6) as profiles. Zen and Go are paid
/// catalogs; Free is a **keyless overlay on Zen** — same 4h model list, no
/// `Authorization` header, and the free subset of rows.
pub fn opencode_overlay_profiles() -> Vec<ProviderProfile> {
    let mk = |id: &str, name: &str, base: &str, key_required: bool| ProviderProfile {
        id: id.to_string(),
        name: name.to_string(),
        format: ProfileFormat::OpenaiCompatible,
        base_url: base.to_string(),
        api_key_required: key_required,
        source: ProfileSource::Overlay,
        session_headers: true,
        ..Default::default()
    };
    vec![
        mk(
            "opencode",
            "OpenCode Zen",
            "https://opencode.ai/zen/v1",
            true,
        ),
        mk(
            "opencode-go",
            "OpenCode Go",
            "https://opencode.ai/zen/go/v1",
            true,
        ),
        mk(
            "opencode-free",
            "OpenCode Free",
            "https://opencode.ai/zen/v1",
            false,
        ),
    ]
}

/// P56.5 — NVIDIA NIM as a base-URL override on the first-class `nvidia`
/// catalog provider (default `http://localhost:8000/v1`).
pub fn nvidia_nim_profile() -> ProviderProfile {
    ProviderProfile {
        id: "nvidia".to_string(),
        name: "Nvidia".to_string(),
        format: ProfileFormat::OpenaiCompatible,
        base_url: "http://localhost:8000/v1".to_string(),
        api_key_required: false,
        source: ProfileSource::Overlay,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-catalog-profiles-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn slug_matches_the_ui_transform() {
        assert_eq!(ProviderProfile::slug("My Proxy!"), "my-proxy");
        assert_eq!(ProviderProfile::slug("  "), "custom");
        assert_eq!(ProviderProfile::slug("openai"), "openai");
    }

    #[test]
    fn base_url_rejects_a_request_path() {
        let mut p = ProviderProfile {
            id: "x".into(),
            base_url: "https://api.example.com/v1/chat/completions".into(),
            ..Default::default()
        };
        assert!(p.base_url_error().unwrap().contains("chat/completions"));
        p.base_url = "https://api.example.com/v1".into();
        assert!(p.base_url_error().is_none());
        p.base_url = "api.example.com".into();
        assert!(p.base_url_error().unwrap().contains("http://"));
        p.base_url = "https://api.example.com/v1/".into();
        assert_eq!(
            p.normalized_base_url().unwrap(),
            "https://api.example.com/v1"
        );
    }

    #[test]
    fn round_trip_through_the_store() {
        let d = dir("roundtrip");
        let store = ProfileStore::in_dir(&d);
        assert!(store.list().is_empty());
        let saved = store
            .upsert(ProviderProfile {
                id: "My-Gateway".into(),
                name: "My Gateway".into(),
                base_url: "http://127.0.0.1:4000/v1".into(),
                api_key_required: false,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(saved.id, "my-gateway");
        let back = store.get("my-gateway").unwrap();
        assert_eq!(back.base_url, "http://127.0.0.1:4000/v1");
        assert!(!back.api_key_required);
        assert_eq!(back.source, ProfileSource::UserConfig);
        // replace, not duplicate
        store
            .upsert(ProviderProfile {
                id: "my-gateway".into(),
                base_url: "http://127.0.0.1:4001/v1".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.remove("my-gateway").unwrap());
        assert!(store.list().is_empty());
        assert!(!store.remove("nope").unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn key_requiring_profile_must_have_a_base_url() {
        let d = dir("nokey");
        let store = ProfileStore::in_dir(&d);
        let err = store
            .upsert(ProviderProfile {
                id: "x".into(),
                api_key_required: true,
                ..Default::default()
            })
            .unwrap_err();
        assert!(err.contains("base URL"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn format_parses_the_dropdown_values_and_rejects_junk() {
        assert_eq!(
            ProfileFormat::parse("openai-compatible").unwrap(),
            ProfileFormat::OpenaiCompatible
        );
        assert_eq!(
            ProfileFormat::parse("anthropic").unwrap(),
            ProfileFormat::Anthropic
        );
        assert_eq!(
            ProfileFormat::parse("openai_responses").unwrap(),
            ProfileFormat::OpenaiResponses
        );
        assert!(ProfileFormat::parse("gemini").is_err());
        assert_eq!(
            ProfileFormat::Anthropic.transport(),
            Transport::AnthropicMessages
        );
    }

    #[test]
    fn the_three_opencode_rows_are_separate_and_free_is_keyless() {
        let rows = opencode_overlay_profiles();
        let ids: Vec<&str> = rows.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["opencode", "opencode-go", "opencode-free"]);
        let free = rows.iter().find(|p| p.id == "opencode-free").unwrap();
        assert!(!free.api_key_required);
        assert_eq!(free.base_url, "https://opencode.ai/zen/v1");
        let go = rows.iter().find(|p| p.id == "opencode-go").unwrap();
        assert_eq!(go.base_url, "https://opencode.ai/zen/go/v1");
        assert!(go.api_key_required);
        assert!(rows.iter().all(|p| p.session_headers));
    }

    #[test]
    fn nim_overlay_points_at_localhost() {
        let p = nvidia_nim_profile();
        assert_eq!(p.base_url, "http://localhost:8000/v1");
        assert!(!p.api_key_required);
        assert!(!p.session_headers);
    }
}
