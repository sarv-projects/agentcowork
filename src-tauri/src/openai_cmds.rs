//! P9.5 — Local OpenAI-compatible server: Tauri wiring.
//!
//! Starts / stops the `agentcowork_core::openai_server::OpenAiServer` on a
//! loopback port and reports its base URL + bearer token so the UI can show a
//! copy-paste config for VS Code / Cursor / Continue.
//!
//! The live [`CompletionBackend`] bridges to the **vault broker** — the same
//! single place keys leave the vault for the sidecar path. The server process
//! itself never holds a provider key; it hands `(provider, model, body)` to
//! the broker, which resolves the key, calls the upstream, and returns the
//! OpenAI-shaped response. Local runtimes (ollama/llamafile) route keylessly.
//!
//! Model id resolution: an incoming `model` of `provider/model` is split on
//! the first `/`; a bare id resolves through the config's MODEL_ALIASES, then
//! falls back to treating it as an OpenAI model. `agentcowork-auto` is a
//! sentinel that lets the request pick the default provider/model.

use std::sync::{Arc, Mutex};

use agentcowork_core::{
    ChatCompletionRequest, CompletionBackend, CompletionResult, ModelLister, ModelRow,
    OpenAiServer, StreamPiece, ToolCallFunction, ToolCallOut,
};
use tauri::State;

use crate::AppState;

/// The running server handle stored in `AppState` (None until started).
#[derive(Default)]
pub struct OpenAiServerSlot {
    server: Option<OpenAiServer>,
}

/// Live backend: resolve the model → call the vault broker → shape the result.
struct BrokerBackend {
    vault: Arc<Mutex<agentcowork_vault::Vault>>,
    aliases: std::collections::HashMap<String, String>,
    /// The default provider/model used for the `agentcowork-auto` sentinel.
    default_provider: String,
    default_model: String,
}

impl BrokerBackend {
    /// Split `model` into `(provider, model)`. `provider/model` splits on the
    /// first `/`; aliases resolve first; `agentcowork-auto` → the default.
    fn resolve(&self, model: &str) -> (String, String) {
        if model == "agentcowork-auto" || model.is_empty() {
            return (self.default_provider.clone(), self.default_model.clone());
        }
        // Alias table (config MODEL_ALIASES).
        let resolved = self
            .aliases
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_string());
        if let Some((prov, m)) = resolved.split_once('/') {
            (prov.to_string(), m.to_string())
        } else {
            // Bare model id — default to OpenAI-compatible provider.
            (self.default_provider.clone(), resolved)
        }
    }
}

/// The session id the A8 server files its broker usage under. Fixed on
/// purpose: the local server is one logical client, so its J11 budget is the
/// server's budget rather than a per-request one.
const SERVER_SESSION: &str = "openai-compat-server";

impl BrokerBackend {
    /// Build the upstream request body: messages (including the tool-calling
    /// round trip) pass through, and the client's `tools`/`tool_choice`/
    /// `parallel_tool_calls` are **forwarded unchanged**. Dropping those was the
    /// single reason this endpoint could not serve a tool-calling client.
    ///
    /// The broker adds auth + prompt-cache markers; the server never sees a key.
    fn upstream_body(&self, req: &ChatCompletionRequest, model: &str) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": model,
            "messages": req.messages.iter().map(|m| {
                let mut msg = serde_json::json!({
                    "role": m.role, "content": m.content,
                });
                // A tool result must name the call it answers, and an assistant
                // tool-calling turn must carry its calls back, or the provider
                // cannot reconstruct the conversation.
                if let Some(id) = m.tool_call_id.as_deref() {
                    msg["tool_call_id"] = serde_json::json!(id);
                }
                if let Some(calls) = m.tool_calls.as_ref() {
                    msg["tool_calls"] = calls.clone();
                }
                msg
            }).collect::<Vec<_>>(),
        });
        if let Some(t) = req.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if let Some(mt) = req.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(tools) = req.tools.as_ref() {
            body["tools"] = tools.clone();
        }
        if let Some(tc) = req.tool_choice.as_ref() {
            body["tool_choice"] = tc.clone();
        }
        if let Some(p) = req.parallel_tool_calls {
            body["parallel_tool_calls"] = serde_json::json!(p);
        }
        body
    }

    /// Shape an OpenAI response into the engine result (content + usage +
    /// native tool calls).
    fn shape(resp: &serde_json::Value, provider: &str, model: &str) -> CompletionResult {
        let message = resp.pointer("/choices/0/message");
        let content = message
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let tool_calls = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        let name = t.pointer("/function/name")?.as_str()?.to_string();
                        // `arguments` is a JSON string on the wire; some
                        // OpenAI-compatible servers send a bare object, so an
                        // object is re-encoded rather than dropped.
                        let arguments = match t.pointer("/function/arguments") {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            Some(other) if !other.is_null() => other.to_string(),
                            _ => String::new(),
                        };
                        let id = t
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .unwrap_or_default();
                        Some(ToolCallOut {
                            id,
                            kind: "function".into(),
                            function: ToolCallFunction { name, arguments },
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        CompletionResult {
            content,
            prompt_tokens: resp
                .pointer("/usage/prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            completion_tokens: resp
                .pointer("/usage/completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            model: format!("{provider}/{model}"),
            tool_calls,
        }
    }
}

impl CompletionBackend for BrokerBackend {
    fn complete(&self, req: &ChatCompletionRequest) -> Result<CompletionResult, String> {
        let (provider, model) = self.resolve(&req.model);
        let body = self.upstream_body(req, &model);

        let vault = self.vault.lock().map_err(|e| e.to_string())?;
        let broker = agentcowork_vault::Broker::new(&vault);
        let resp = broker
            .chat_completion(&provider, &model, SERVER_SESSION, body)
            .map_err(|e| e.to_string())?;
        Ok(Self::shape(&resp, &provider, &model))
    }

    /// Real per-chunk delivery: the vault broker's incremental stream reports
    /// each parsed SSE event as it comes off the provider socket, so the A8
    /// server forwards genuine token-level frames instead of one blob. Tool-call
    /// fragments are relayed with their OpenAI `delta.tool_calls` grouping
    /// (`index` + first-fragment `id`/`name`), while `assemble_tool_calls`
    /// reconstructs the aggregate call for the non-terminal result.
    fn stream(
        &self,
        req: &ChatCompletionRequest,
        on_piece: &mut dyn FnMut(StreamPiece),
    ) -> Result<CompletionResult, String> {
        let (provider, model) = self.resolve(&req.model);
        let body = self.upstream_body(req, &model);

        let vault = self.vault.lock().map_err(|e| e.to_string())?;
        let broker = agentcowork_vault::Broker::new(&vault);
        let mut seen_id: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let events = broker
            .chat_completion_stream_cb(
                &provider,
                &model,
                SERVER_SESSION,
                body,
                &mut |ev: &agentcowork_vault::ChatStreamEvent| {
                    if let Some(delta) = ev.delta.as_deref() {
                        if !delta.is_empty() {
                            on_piece(StreamPiece::Content(delta.to_string()));
                        }
                    }
                    for tc in &ev.tool_calls {
                        // Announce each call once (id + name), then stream its
                        // argument fragments — the OpenAI delta contract.
                        let first = seen_id.insert(tc.index);
                        on_piece(StreamPiece::ToolCall {
                            index: tc.index,
                            id: if first { tc.id.clone() } else { None },
                            name: if first { tc.name.clone() } else { None },
                            arguments: tc.arguments.clone(),
                        });
                    }
                },
            )
            .map_err(|e| e.to_string())?;

        // Aggregate usage + the assembled calls from the same buffered events.
        let finished_by_length = events.iter().any(|e| e.finish.as_deref() == Some("length"));
        let content = events
            .iter()
            .filter_map(|e| e.delta.as_deref())
            .collect::<String>();
        let tool_calls = agentcowork_vault::assemble_tool_calls(&events, finished_by_length)
            .into_iter()
            .enumerate()
            .map(|(i, (name, args))| ToolCallOut {
                id: format!("call_{i}"),
                kind: "function".into(),
                function: ToolCallFunction {
                    name,
                    arguments: args.to_string(),
                },
            })
            .collect::<Vec<_>>();
        let usage = events.iter().filter_map(|e| e.usage).fold(
            agentcowork_vault::Usage::default(),
            |mut acc, u| {
                acc.merge_max(u);
                acc
            },
        );

        Ok(CompletionResult {
            content,
            prompt_tokens: usage.prompt,
            completion_tokens: usage.output,
            model: format!("{provider}/{model}"),
            tool_calls,
        })
    }
}

/// Model lister: advertise the `agentcowork-auto` sentinel + configured aliases
/// + any installed local models (ollama/llamafile). Never lists a raw key.
///
/// `owned_by` names the **real owner** of each row: the provider the request
/// routes to, or `local` for an on-machine runtime. It is never `everyaios`
/// — this server is transport, not a model owner, and attributing model rows
/// to a built-in agent identity is what ADR-0005 retires (P71.2a).
struct EngineModels {
    /// `(alias, owner)` — the owner half is the alias's resolved provider.
    aliases: Vec<(String, String)>,
    /// Installed local models, already spelled `runtime/model`.
    local: Vec<String>,
    /// The owner the `agentcowork-auto` sentinel resolves to.
    auto_owner: String,
}

impl ModelLister for EngineModels {
    fn models(&self) -> Vec<ModelRow> {
        let mut rows = vec![ModelRow::new("agentcowork-auto", self.auto_owner.clone())];
        for (alias, owner) in &self.aliases {
            rows.push(ModelRow::new(alias.clone(), owner.clone()));
        }
        for m in &self.local {
            rows.push(ModelRow::new(m.clone(), "local"));
        }
        rows
    }
}

/// Start the local OpenAI-compatible server (idempotent — returns the existing
/// server's details if already running). Loopback-only; returns the base URL +
/// per-process bearer token for the client config.
#[tauri::command]
pub fn openai_server_start(
    state: State<'_, AppState>,
    port: Option<u16>,
) -> Result<serde_json::Value, String> {
    let mut slot = state.openai_server.lock().map_err(|e| e.to_string())?;
    if let Some(existing) = slot.server.as_ref() {
        return Ok(serde_json::json!({
            "running": true,
            "baseUrl": existing.base_url(),
            "token": existing.token(),
            "already": true,
        }));
    }

    let cfg = agentcowork_core::Config::load().unwrap_or_default();
    let aliases = cfg.model_aliases.clone();

    // Default provider/model for the `agentcowork-auto` sentinel: first alias
    // target, else a conservative OpenAI-compatible default.
    let (default_provider, default_model) = aliases
        .values()
        .next()
        .and_then(|v| {
            v.split_once('/')
                .map(|(p, m)| (p.to_string(), m.to_string()))
        })
        .unwrap_or_else(|| ("openai".to_string(), "gpt-4o-mini".to_string()));

    // Installed local models (best-effort; empty if no runtime).
    let local = {
        let mgr = agentcowork_core::LocalManager::from_config(&cfg);
        mgr.list_ollama_models()
            .into_iter()
            .map(|m| format!("ollama/{}", m.name))
            .collect::<Vec<_>>()
    };

    // P71.2a — each listed row carries its real owner, resolved from the alias
    // target's provider half (bare ids fall back to the default provider).
    let alias_rows: Vec<(String, String)> = aliases
        .iter()
        .map(|(alias, target)| {
            let owner = target
                .split_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_else(|| default_provider.clone());
            (alias.clone(), owner)
        })
        .collect();

    let backend: Arc<dyn CompletionBackend> = Arc::new(BrokerBackend {
        vault: Arc::clone(&state.vault),
        aliases,
        default_provider: default_provider.clone(),
        default_model,
    });
    let lister: Arc<dyn ModelLister> = Arc::new(EngineModels {
        aliases: alias_rows,
        local,
        auto_owner: default_provider.clone(),
    });

    let bind = format!("127.0.0.1:{}", port.unwrap_or(0));
    let server = OpenAiServer::serve(&bind, backend, lister).map_err(|e| e.to_string())?;
    let base_url = server.base_url();
    let token = server.token().to_string();
    slot.server = Some(server);

    Ok(serde_json::json!({
        "running": true,
        "baseUrl": base_url,
        "token": token,
        "already": false,
    }))
}

/// Stop the server (idempotent). Dropping the handle closes the listener.
#[tauri::command]
pub fn openai_server_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut slot = state.openai_server.lock().map_err(|e| e.to_string())?;
    slot.server = None;
    Ok(())
}

/// Current server status (running + base URL + token, or stopped).
#[tauri::command]
pub fn openai_server_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let slot = state.openai_server.lock().map_err(|e| e.to_string())?;
    match slot.server.as_ref() {
        Some(s) => Ok(serde_json::json!({
            "running": true,
            "baseUrl": s.base_url(),
            "token": s.token(),
        })),
        None => Ok(serde_json::json!({ "running": false })),
    }
}
