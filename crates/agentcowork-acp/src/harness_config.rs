//! P30.7 — **HarnessConfigWriter** (cc-switch pattern, doc 83 §1,
//! reimplemented on our stack). Manages the *provider* config of external
//! agent CLIs (Claude Code · Codex · OpenCode · Cursor · Gemini CLI) from the
//! cockpit — beside ACP-driving, not replacing it. Where ACP talks to a live
//! harness over JSON-RPC, this trait reads/writes each harness's own provider
//! config file (`settings.json` / `config.toml` / `opencode.json`), so the
//! user can point an external agent at the same BYOK providers from one
//! surface.
//!
//! The trait is pure: `read_provider` parses a config document,
//! `set_provider` rewrites it. No filesystem I/O here — the caller decides
//! the path (the Tauri command layer). Keys are written only when a value is
//! present; existing unknown keys are preserved.
//!
//! # Status: **deliberately uncalled** (read this before wiring it)
//!
//! Nothing invokes this module in production, and that is a recorded decision,
//! not an oversight. P30.7's deliverable was the **pure trait**; the impure
//! half — detect → read → preview → Guard → atomic-write → verify →
//! EffectReceipt → rollback — is the **P47.7 ClientCompatibility ring**, which
//! is **post-v1 by ADR** (`TODO.md` P47.7; spec §6 #21). It cannot ship earlier
//! because every path it would write (`.claude/`, `.codex/`, `.cursor/`, …)
//! is a `protected_paths` write floor (`floor:protected-settings`), so it must
//! move through the effect funnel.
//!
//! **What *did* ship for v1 instead:** `agent_backend.rs` / P63 — the same user
//! goal (point an external agent at your provider) solved by injecting the
//! provider's environment **at spawn**, which writes nothing to disk and needs
//! no funnel. Prefer extending that path; treat this module as the Tier-3
//! (disk-projection) adapters for P47.7.
//!
//! Known schema gaps against the real CLIs, to fix when P47.7 opens: OpenCode
//! wants `models` as an **object map** (this writes a string array) and writes
//! no `options.baseURL`/`options.apiKey`/`npm`; Codex needs the
//! `[model_providers.<id>]` table (`name`/`base_url`/`env_key`/`wire_api`)
//! alongside `model_provider`; and `api_key_env` is documented as an env-var
//! **name** but written into the value slot.

use serde_json::{Value, json};

/// What a harness config edit needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    /// Provider id in the harness's vocabulary (`anthropic`, `openai`, …).
    pub provider_id: String,
    /// Optional base URL override (local gateways like Ollama/Kilocode).
    pub base_url: Option<String>,
    /// Optional default model.
    pub model: Option<String>,
    /// Optional env var name that holds the API key (never the key itself).
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessConfigError {
    /// The document is not valid for this harness format.
    InvalidDocument(String),
    /// The provider section already exists with a different key.
    KeyConflict(String),
}

/// One external agent CLI's provider-config contract.
pub trait HarnessConfigWriter {
    /// Stable harness id (matches the ACP catalog ids: `claude`, `codex`, …).
    fn harness_id(&self) -> &'static str;

    /// File name of the config the harness reads (relative to its config
    /// dir — the caller resolves the dir per-platform).
    fn config_file(&self) -> &'static str;

    /// Read the provider config back out of a document.
    fn read_provider(&self, doc: &str) -> Option<ProviderConfig>;

    /// Rewrite the document with `provider` applied. Returns the new
    /// document, preserving everything else.
    fn set_provider(
        &self,
        doc: &str,
        provider: &ProviderConfig,
    ) -> Result<String, HarnessConfigError>;
}

// ---------------------------------------------------------------------------
// Claude Code — `settings.json` (JSON object; provider via `env` + model).
// ---------------------------------------------------------------------------

/// Claude Code: `~/.claude/settings.json` — `{"env": {"ANTHROPIC_MODEL": …}}`.
pub struct ClaudeCodeConfig;

impl HarnessConfigWriter for ClaudeCodeConfig {
    fn harness_id(&self) -> &'static str {
        "claude"
    }
    fn config_file(&self) -> &'static str {
        "settings.json"
    }

    fn read_provider(&self, doc: &str) -> Option<ProviderConfig> {
        let v: Value = serde_json::from_str(doc).ok()?;
        let env = v.get("env")?.as_object()?;
        let model = env.get("ANTHROPIC_MODEL")?.as_str()?.to_string();
        let base_url = env
            .get("ANTHROPIC_BASE_URL")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let api_key_env = env
            .get("ANTHROPIC_API_KEY")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        Some(ProviderConfig {
            provider_id: "anthropic".to_string(),
            base_url,
            model: Some(model),
            api_key_env,
        })
    }

    fn set_provider(
        &self,
        doc: &str,
        provider: &ProviderConfig,
    ) -> Result<String, HarnessConfigError> {
        let mut v: Value = if doc.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(doc)
                .map_err(|e| HarnessConfigError::InvalidDocument(e.to_string()))?
        };
        let env = v
            .as_object_mut()
            .ok_or_else(|| {
                HarnessConfigError::InvalidDocument("settings.json must be a JSON object".into())
            })?
            .entry("env")
            .or_insert_with(|| json!({}));
        let env = env
            .as_object_mut()
            .ok_or_else(|| HarnessConfigError::InvalidDocument("env must be an object".into()))?;
        if let Some(m) = &provider.model {
            env.insert("ANTHROPIC_MODEL".into(), json!(m));
        }
        if let Some(u) = &provider.base_url {
            env.insert("ANTHROPIC_BASE_URL".into(), json!(u));
        }
        if let Some(k) = &provider.api_key_env {
            env.insert("ANTHROPIC_API_KEY".into(), json!(k));
        }
        serde_json::to_string_pretty(&v)
            .map_err(|e| HarnessConfigError::InvalidDocument(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Codex — `config.toml` (TOML; `model` + `model_provider` top-level keys).
// ---------------------------------------------------------------------------

/// Codex CLI: `~/.codex/config.toml` — `model = "…"`, `model_provider = "…"`.
pub struct CodexConfig;

impl HarnessConfigWriter for CodexConfig {
    fn harness_id(&self) -> &'static str {
        "codex"
    }
    fn config_file(&self) -> &'static str {
        "config.toml"
    }

    fn read_provider(&self, doc: &str) -> Option<ProviderConfig> {
        let model = toml_key(doc, "model")?;
        let provider_id = toml_key(doc, "model_provider").unwrap_or_else(|| "openai".to_string());
        Some(ProviderConfig {
            provider_id,
            base_url: None,
            model: Some(model),
            api_key_env: None,
        })
    }

    fn set_provider(
        &self,
        doc: &str,
        provider: &ProviderConfig,
    ) -> Result<String, HarnessConfigError> {
        let mut lines = doc.lines().map(|l| l.to_string()).collect::<Vec<_>>();
        if let Some(m) = &provider.model {
            set_toml_key(&mut lines, "model", m)?;
        }
        set_toml_key(&mut lines, "model_provider", &provider.provider_id)?;
        Ok(lines.join("\n") + "\n")
    }
}

/// Read a top-level TOML key's string value (line-based, no full parser).
fn toml_key(doc: &str, key: &str) -> Option<String> {
    doc.lines().find_map(|l| {
        let l = l.trim();
        let (k, v) = l.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim().trim_matches('"').to_string();
        if v.is_empty() { None } else { Some(v) }
    })
}

/// Set (or add) a top-level TOML key. Existing `[section]` boundaries are
/// respected: the key is inserted before the first section header.
fn set_toml_key(lines: &mut Vec<String>, key: &str, value: &str) -> Result<(), HarnessConfigError> {
    let line = format!("{key} = \"{value}\"");
    if let Some(idx) = lines.iter().position(|l| {
        let t = l.trim();
        t.starts_with(key) && t.split_once('=').is_some_and(|(k, _)| k.trim() == key)
    }) {
        lines[idx] = line;
        return Ok(());
    }
    let insert_at = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    lines.insert(insert_at, line);
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenCode — `opencode.json` (JSON; top-level `provider` object).
// ---------------------------------------------------------------------------

/// OpenCode: `~/.config/opencode/opencode.json` — `{"provider": {…}}`.
pub struct OpenCodeConfig;

impl HarnessConfigWriter for OpenCodeConfig {
    fn harness_id(&self) -> &'static str {
        "opencode"
    }
    fn config_file(&self) -> &'static str {
        "opencode.json"
    }

    fn read_provider(&self, doc: &str) -> Option<ProviderConfig> {
        let v: Value = serde_json::from_str(doc).ok()?;
        let p = v.get("provider")?.as_object()?;
        let provider_id = p.keys().next()?.clone();
        let body = p.get(&provider_id)?;
        let model = body.get("models")?.get(0)?.as_str().map(|s| s.to_string());
        Some(ProviderConfig {
            provider_id,
            base_url: None,
            model,
            api_key_env: None,
        })
    }

    fn set_provider(
        &self,
        doc: &str,
        provider: &ProviderConfig,
    ) -> Result<String, HarnessConfigError> {
        let mut v: Value = if doc.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(doc)
                .map_err(|e| HarnessConfigError::InvalidDocument(e.to_string()))?
        };
        let obj = v.as_object_mut().ok_or_else(|| {
            HarnessConfigError::InvalidDocument("opencode.json must be a JSON object".into())
        })?;
        let providers = obj.entry("provider").or_insert_with(|| json!({}));
        let providers = providers.as_object_mut().ok_or_else(|| {
            HarnessConfigError::InvalidDocument("provider must be an object".into())
        })?;
        let entry = providers
            .entry(provider.provider_id.clone())
            .or_insert_with(|| json!({}));
        if let Some(m) = &provider.model {
            let models = entry
                .as_object_mut()
                .ok_or_else(|| {
                    HarnessConfigError::InvalidDocument("provider body must be an object".into())
                })?
                .entry("models")
                .or_insert_with(|| json!([]));
            if let Some(arr) = models.as_array_mut() {
                if arr.is_empty() {
                    arr.push(json!(m));
                } else {
                    arr[0] = json!(m);
                }
            }
        }
        serde_json::to_string_pretty(&v)
            .map_err(|e| HarnessConfigError::InvalidDocument(e.to_string()))
    }
}

/// The builtin writer set (harness id → writer), for cockpit discovery.
pub fn builtin_writers() -> Vec<&'static dyn HarnessConfigWriter> {
    vec![&ClaudeCodeConfig, &CodexConfig, &OpenCodeConfig]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_roundtrip() {
        let w = ClaudeCodeConfig;
        let cfg = ProviderConfig {
            provider_id: "anthropic".into(),
            base_url: Some("http://127.0.0.1:8080".into()),
            model: Some("claude-sonnet-4".into()),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
        };
        let out = w.set_provider("{}", &cfg).unwrap();
        let back = w.read_provider(&out).unwrap();
        assert_eq!(back.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(back.base_url.as_deref(), Some("http://127.0.0.1:8080"));
        // Preserves unknown keys.
        let out2 = w.set_provider(&out, &cfg).unwrap();
        assert!(out2.contains("claude-sonnet-4"));
    }

    #[test]
    fn codex_toml_roundtrip() {
        let w = CodexConfig;
        let doc = "model = \"gpt-5\"\nmodel_provider = \"openai\"\n\n[experimental]\nfoo = true\n";
        let cfg = ProviderConfig {
            provider_id: "openai".into(),
            base_url: None,
            model: Some("gpt-5.2".into()),
            api_key_env: None,
        };
        let out = w.set_provider(doc, &cfg).unwrap();
        let back = w.read_provider(&out).unwrap();
        assert_eq!(back.model.as_deref(), Some("gpt-5.2"));
        // Section preserved.
        assert!(out.contains("[experimental]"));
        assert!(out.contains("foo = true"));
    }

    #[test]
    fn opencode_roundtrip() {
        let w = OpenCodeConfig;
        let cfg = ProviderConfig {
            provider_id: "ollama".into(),
            base_url: None,
            model: Some("qwen3:8b".into()),
            api_key_env: None,
        };
        let out = w.set_provider("{}", &cfg).unwrap();
        let back = w.read_provider(&out).unwrap();
        assert_eq!(back.provider_id, "ollama");
        assert_eq!(back.model.as_deref(), Some("qwen3:8b"));
    }

    #[test]
    fn builtin_ids_unique() {
        let writers = builtin_writers();
        let mut ids = writers.iter().map(|w| w.harness_id()).collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), writers.len());
    }
}
