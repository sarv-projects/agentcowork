//! P50.4.2 — Local model downloads (Tauri wiring over the P27 backend).
//!
//! The backend (`agentcowork_core::models`) was landed in the P27 queue:
//! [`HfClient`] (resumable `Range` downloads + sha256 verify), [`ModelRegistry`]
//! (`<data_dir>/models/hf/index.json`), `local://` URLs + resolver, and
//! [`ModelsRuntime`] (llamafile serve / `ollama create`). This module is the
//! missing consumer wiring: download start/progress/cancel/resume over Tauri
//! events, registry CRUD, hardware-fit quant recommendation, and runtime
//! binding — all honest-fail when the environment lacks network/runtime.
//!
//! Download lifecycle:
//! 1. `model_download_start` spawns a thread; progress is emitted on
//!    [`MODEL_DOWNLOAD_EVENT`] (throttled to ≥1 MiB per event).
//! 2. The `*.part` staging file lives at
//!    `<data_dir>/models/hf/{publisher}/{model}/{filename}.part` — cancel
//!    keeps it, so a later start with the same repo+filename resumes via
//!    `Range` (416 → clean restart).
//! 3. On success the file is renamed to the canonical
//!    `{quant}-{sha8}.gguf` (or `.safetensors`) path and registered in
//!    `index.json`; the entry is then resolvable as `local://hf/...`.
//! 4. Orphaned `.part` files (crashed/cancelled across restarts) are
//!    reported by `model_downloads` so the UI can offer Resume.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agentcowork_core::models::hf::{part_path, quant_from_filename, HfClient, HfError};
use agentcowork_core::models::store::{ModelEntry, ModelRegistry};
use agentcowork_core::models::{probe_hardware, ManagedServeHandle, ModelsRuntime};
use agentcowork_types::RuntimeHealthState;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;
use crate::MODEL_DOWNLOAD_EVENT;

/// One in-flight download (P50.4.2). The cancel flag is cooperative — the
/// download loop checks it at every chunk boundary and returns
/// `HfError::Cancelled`, leaving the `.part` file for resume.
pub struct ModelDownloadSlot {
    pub repo: String,
    pub filename: String,
    /// Staging dest: `<base>/hf/{publisher}/{model}/{filename}` (the `.part`
    /// lives beside it; the final rename is canonical `{quant}-{sha8}.gguf`).
    pub dest: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub status: Arc<Mutex<DownloadStatus>>,
}

/// Live download status (mirrored to the UI via events + `model_downloads`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct DownloadStatus {
    /// `downloading` | `done` | `error` | `cancelled` | `serving` | `served`
    pub phase: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
    /// Canonical installed path once the registry write landed.
    pub path: Option<String>,
    /// Registry id (`{publisher}/{model}:{quant}`) once installed.
    pub registry_id: Option<String>,
    /// Runtime binding (model_serve): the OpenAI-compatible base URL.
    pub base_url: Option<String>,
}

static DL_COUNTER: AtomicU64 = AtomicU64::new(1);
static SERVE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The process-custody operations the shell needs from a managed model serve.
///
/// Production uses [`ManagedServeHandle`]. The generic registry keeps the
/// lifecycle bookkeeping independently testable with an injected fake process.
pub(crate) trait ManagedServeProcess: Send {
    /// Return the retained child's listening port.
    fn port(&self) -> u16;
    /// Return the retained child's loopback base URL.
    fn base_url(&self) -> &str;
    /// Return the model identity served by the retained child.
    fn model_id(&self) -> &str;
    /// Return the deterministic launch configuration identity.
    fn config_hash(&self) -> &str;
    /// Probe current process/runtime health without releasing custody.
    fn health(&mut self) -> RuntimeHealthState;
    /// Stop and reap the process while consuming custody.
    fn stop_owned(handle: Self) -> Result<(), String>
    where
        Self: Sized;
}

impl ManagedServeProcess for ManagedServeHandle {
    fn port(&self) -> u16 {
        self.port
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn config_hash(&self) -> &str {
        &self.config_hash
    }

    fn health(&mut self) -> RuntimeHealthState {
        ManagedServeHandle::health(self)
    }

    fn stop_owned(handle: Self) -> Result<(), String> {
        ManagedServeHandle::stop(handle).map_err(|error| error.to_string())
    }
}

/// One retained managed serve and its stable shell metadata.
struct ManagedServeEntry<H> {
    kind: String,
    started_at_ms: u64,
    handle: H,
}

/// A point-in-time projection of one retained managed serve.
pub(crate) struct ManagedServeSnapshot {
    pub id: String,
    pub kind: String,
    pub health: RuntimeHealthState,
    pub port: u16,
    pub base_url: String,
    pub model_id: String,
    pub config_hash: String,
    pub started_at_ms: u64,
}

/// The AppState registry that retains every AgentCowork-started model serve.
///
/// Dropping the registry drops every retained handle, which invokes the core
/// handle's RAII child cleanup. Stop removes custody first and calls the
/// handle's explicit stop method outside the registry lock.
pub(crate) struct ManagedServeRegistry<H = ManagedServeHandle> {
    entries: std::collections::HashMap<String, ManagedServeEntry<H>>,
}

impl<H> Default for ManagedServeRegistry<H> {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
        }
    }
}

impl<H: ManagedServeProcess> ManagedServeRegistry<H> {
    /// Retain one newly-started process under a stable serve id.
    pub(crate) fn insert(
        &mut self,
        id: String,
        kind: String,
        started_at_ms: u64,
        handle: H,
    ) -> Result<(), String> {
        if self.entries.contains_key(&id) {
            return Err(format!("managed serve id already exists: {id}"));
        }
        self.entries.insert(
            id,
            ManagedServeEntry {
                kind,
                started_at_ms,
                handle,
            },
        );
        Ok(())
    }

    /// Return whether this exact id names a process in the managed registry.
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// Remove and return one retained process without running provider/network I/O.
    pub(crate) fn remove(&mut self, id: &str) -> Option<H> {
        self.entries.remove(id).map(|entry| entry.handle)
    }

    /// Snapshot every retained process, probing health while the registry is locked.
    pub(crate) fn rows(&mut self) -> Vec<ManagedServeSnapshot> {
        let mut rows: Vec<_> = self
            .entries
            .iter_mut()
            .map(|(id, entry)| ManagedServeSnapshot {
                id: id.clone(),
                kind: entry.kind.clone(),
                health: entry.handle.health(),
                port: entry.handle.port(),
                base_url: entry.handle.base_url().to_string(),
                model_id: entry.handle.model_id().to_string(),
                config_hash: entry.handle.config_hash().to_string(),
                started_at_ms: entry.started_at_ms,
            })
            .collect();
        rows.sort_by(|left, right| left.id.cmp(&right.id));
        rows
    }

    /// Snapshot one retained process without releasing or cloning its handle.
    pub(crate) fn row(&mut self, id: &str) -> Option<ManagedServeSnapshot> {
        let entry = self.entries.get_mut(id)?;
        Some(ManagedServeSnapshot {
            id: id.to_string(),
            kind: entry.kind.clone(),
            health: entry.handle.health(),
            port: entry.handle.port(),
            base_url: entry.handle.base_url().to_string(),
            model_id: entry.handle.model_id().to_string(),
            config_hash: entry.handle.config_hash().to_string(),
            started_at_ms: entry.started_at_ms,
        })
    }
}

fn models_base() -> PathBuf {
    agentcowork_core::default_data_dir().join("models")
}

fn emit_download(
    app: &AppHandle,
    kind: &str,
    id: &str,
    repo: &str,
    filename: &str,
    status: &DownloadStatus,
) {
    let _ = app.emit(
        MODEL_DOWNLOAD_EVENT,
        serde_json::json!({
            "kind": kind,
            "id": id,
            "repo": repo,
            "filename": filename,
            "phase": status.phase,
            "doneBytes": status.done_bytes,
            "totalBytes": status.total_bytes,
            "error": status.error,
            "path": status.path,
            "registryId": status.registry_id,
            "baseUrl": status.base_url,
        }),
    );
}

/// Start (or resume) a Hugging Face GGUF/safetensors download. Idempotent:
/// an already-installed model or an in-flight download for the same file is
/// returned as-is (with `resuming` when a `.part` exists). Runs on a
/// background thread; progress lands on `model-download` events.
#[tauri::command]
pub fn model_download_start(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    filename: String,
) -> Result<serde_json::Value, String> {
    let (publisher, model) = repo
        .split_once('/')
        .filter(|(p, m)| !p.is_empty() && !m.is_empty())
        .ok_or_else(|| format!("repo must be `publisher/model`, got `{repo}`"))?;
    if !filename.ends_with(".gguf") && !filename.ends_with(".safetensors") {
        return Err("only `.gguf` / `.safetensors` files can be downloaded".into());
    }

    let base = models_base();
    let quant = quant_from_filename(&filename).to_string();
    let registry_id = format!("{repo}:{quant}");
    let registry = ModelRegistry::load(base.clone());
    if registry.get(&registry_id).is_some() {
        return Ok(serde_json::json!({
            "ok": true, "alreadyInstalled": true, "id": registry_id,
        }));
    }

    let dest = base.join("hf").join(publisher).join(model).join(&filename);
    let part = part_path(&dest);
    let resume_from = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    // Idempotent: an in-flight download for the same dest is returned as-is.
    {
        let map = state.model_downloads.lock().map_err(|e| e.to_string())?;
        for (id, slot) in map.iter() {
            if slot.dest == dest {
                let status = slot
                    .status
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                return Ok(serde_json::json!({
                    "ok": true, "alreadyInstalled": false, "id": id,
                    "resuming": status.done_bytes > 0,
                }));
            }
        }
    }

    let id = format!("dl-{:04}", DL_COUNTER.fetch_add(1, Ordering::Relaxed));
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(DownloadStatus {
        phase: "downloading".into(),
        done_bytes: resume_from,
        ..Default::default()
    }));
    {
        let mut map = state.model_downloads.lock().map_err(|e| e.to_string())?;
        map.insert(
            id.clone(),
            ModelDownloadSlot {
                repo: repo.clone(),
                filename: filename.clone(),
                dest: dest.clone(),
                cancel: Arc::clone(&cancel),
                status: Arc::clone(&status),
            },
        );
    }

    let app2 = app.clone();
    let status2 = Arc::clone(&status);
    let cancel2 = Arc::clone(&cancel);
    let base2 = base.clone();
    let dest2 = dest.clone();
    let repo2 = repo.clone();
    let filename2 = filename.clone();
    let id2 = id.clone();
    let num_ctx = agentcowork_core::Config::load()
        .unwrap_or_default()
        .local
        .num_ctx;
    let publisher2 = publisher.to_string();
    let model2 = model.to_string();
    let quant2 = quant.clone();

    std::thread::spawn(move || {
        let mut last_emit = resume_from;
        let client = HfClient::new();
        let outcome = client.download(
            &repo2,
            &filename2,
            &dest2,
            &mut |done, total| {
                {
                    let mut s = status2.lock().unwrap_or_else(|e| e.into_inner());
                    s.done_bytes = done;
                    s.total_bytes = total;
                }
                if done - last_emit >= 1_000_000 || done == total {
                    last_emit = done;
                    let s = status2.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    emit_download(&app2, "download", &id2, &repo2, &filename2, &s);
                }
            },
            Some(&cancel2),
        );

        let status3 = Arc::clone(&status2);
        match outcome {
            Ok(mut entry) => {
                // Canonicalize: `{quant}-{sha8}.{ext}` (entry_path's `.gguf`
                // suffix is GGUF-only; safetensors keeps its own extension).
                let sha8 = entry.sha256.get(..8).unwrap_or("").to_string();
                let ext = if filename2.ends_with(".safetensors") {
                    ".safetensors"
                } else {
                    ".gguf"
                };
                let canonical = base2
                    .join("hf")
                    .join(&publisher2)
                    .join(&model2)
                    .join(format!("{quant2}-{sha8}{ext}"));
                if canonical != dest2 {
                    let _ = std::fs::rename(&dest2, &canonical);
                }
                entry.path = canonical.to_string_lossy().into_owned();
                entry.ctx = num_ctx;
                let mut reg = ModelRegistry::load(base2.clone());
                reg.add(entry.clone());
                let _ = reg.save();
                let mut s = status3.lock().unwrap_or_else(|e| e.into_inner());
                s.phase = "done".into();
                s.done_bytes = s.total_bytes;
                s.path = Some(entry.path.clone());
                s.registry_id = Some(entry.id.clone());
                let snapshot = s.clone();
                drop(s);
                emit_download(&app2, "download", &id2, &repo2, &filename2, &snapshot);
            }
            Err(HfError::Cancelled) => {
                let mut s = status3.lock().unwrap_or_else(|e| e.into_inner());
                s.phase = "cancelled".into();
                let snapshot = s.clone();
                drop(s);
                emit_download(&app2, "download", &id2, &repo2, &filename2, &snapshot);
            }
            Err(e) => {
                let mut s = status3.lock().unwrap_or_else(|e| e.into_inner());
                s.phase = "error".into();
                s.error = Some(e.to_string());
                let snapshot = s.clone();
                drop(s);
                emit_download(&app2, "download", &id2, &repo2, &filename2, &snapshot);
            }
        }
        // Terminal states leave the slot queryable for a short window; the
        // UI refreshes `model_downloads` and the map entry is dropped here.
        if let Ok(mut map) = app2.state::<AppState>().model_downloads.lock() {
            map.remove(&id2);
        }
    });

    Ok(serde_json::json!({
        "ok": true, "alreadyInstalled": false, "id": id, "resuming": resume_from > 0,
    }))
}

/// List in-flight downloads + orphaned `.part` files (interrupted across
/// restarts — these can be resumed with `model_download_start`).
#[tauri::command]
pub fn model_downloads(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let map = state.model_downloads.lock().map_err(|e| e.to_string())?;
    let active: Vec<serde_json::Value> = map
        .iter()
        .map(|(id, slot)| {
            let s = slot
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            serde_json::json!({
                "id": id,
                "repo": slot.repo,
                "filename": slot.filename,
                "phase": s.phase,
                "doneBytes": s.done_bytes,
                "totalBytes": s.total_bytes,
                "error": s.error,
                "path": s.path,
                "registryId": s.registry_id,
            })
        })
        .collect();
    drop(map);

    let mut orphans = Vec::new();
    let base = models_base();
    let active_dests: Vec<PathBuf> = {
        let map = state.model_downloads.lock().map_err(|e| e.to_string())?;
        map.values().map(|s| s.dest.clone()).collect()
    };
    walk_parts(&base.join("hf"), &active_dests, &mut orphans);
    Ok(serde_json::json!({ "active": active, "orphans": orphans }))
}

fn walk_parts(dir: &std::path::Path, active: &[PathBuf], out: &mut Vec<serde_json::Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_parts(&path, active, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("part") {
            // The active download writes `.part` at its own dest; only files
            // NOT tracked by an in-flight slot are orphans.
            let staged = path.with_extension("");
            if active.iter().any(|a| a == &staged) {
                continue;
            }
            let done = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let rel = path.strip_prefix(models_base()).unwrap_or(&path);
            out.push(serde_json::json!({
                "dest": staged.to_string_lossy(),
                "rel": rel.to_string_lossy(),
                "doneBytes": done,
            }));
        }
    }
}

/// Cooperative cancel: the download thread stops at the next chunk boundary
/// and keeps the `.part` file (resume via `model_download_start`).
#[tauri::command]
pub fn model_download_cancel(
    state: State<'_, AppState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let map = state.model_downloads.lock().map_err(|e| e.to_string())?;
    match map.get(&id) {
        Some(slot) => {
            slot.cancel.store(true, Ordering::Relaxed);
            Ok(serde_json::json!({ "ok": true, "id": id }))
        }
        None => Err(format!("no active download {id}")),
    }
}

/// Installed models from the canonical registry (`index.json`) + total bytes.
#[tauri::command]
pub fn model_registry_list() -> Result<serde_json::Value, String> {
    let base = models_base();
    let registry = ModelRegistry::load(base.clone());
    let entries: Vec<serde_json::Value> = registry
        .list()
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "path": e.path,
                "sha256": e.sha256,
                "size": e.size,
                "ctx": e.ctx,
                "quant": e.quant,
                "source": e.source,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "models": entries,
        "totalBytes": registry.total_bytes(),
        "baseDir": base.join("hf").to_string_lossy(),
    }))
}

/// Remove a downloaded model: registry entry + file on disk.
#[tauri::command]
pub fn model_registry_remove(id: String) -> Result<serde_json::Value, String> {
    let base = models_base();
    let mut registry = ModelRegistry::load(base.clone());
    let removed = registry
        .remove(&id)
        .ok_or_else(|| format!("model not in registry: {id}"))?;
    let _ = std::fs::remove_file(&removed.path);
    let _ = std::fs::remove_file(part_path(std::path::Path::new(&removed.path)));
    registry.save().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "id": id }))
}

/// Hardware-fit quant recommendation from **live** RAM + the repo's file list
/// (the largest GGUF that fits in 60% of available RAM). Network-dependent.
#[tauri::command]
pub fn model_recommend_quant(repo: String) -> Result<serde_json::Value, String> {
    let hw = probe_hardware();
    let client = HfClient::new();
    let quant = client
        .recommend_quant(&repo, hw.available_ram_bytes)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "quant": quant,
        "availableRamBytes": hw.available_ram_bytes,
    }))
}

/// Stable start response for one AgentCowork-owned model runtime.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelServeStart {
    pub serve_id: String,
    pub port: u16,
    pub base_url: String,
    pub model_id: String,
    pub config_hash: String,
    pub health: RuntimeHealthState,
    pub ownership: String,
}

/// One row in `model_serve_list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelServeListRow {
    pub id: String,
    pub kind: String,
    pub health: RuntimeHealthState,
    pub port: u16,
    pub model: String,
    pub config_hash: String,
    pub started_at: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn parse_serve_options(
    serve_options: Option<serde_json::Value>,
) -> (
    agentcowork_core::models::ServeOptions,
    Option<agentcowork_core::models::KvCacheType>,
) {
    use agentcowork_core::models::{FlashAttn, ServeOptions};

    let mut kv_cache = None;
    let opts = match serve_options {
        Some(value) => {
            let mut parsed =
                serde_json::from_value::<ServeOptions>(value.clone()).unwrap_or_default();
            if let Some(kv) = value.get("kvCache").and_then(serde_json::Value::as_str) {
                kv_cache = match kv.to_ascii_lowercase().as_str() {
                    "q8_0" => Some(agentcowork_core::models::KvCacheType::Q8_0),
                    "q4_0" => Some(agentcowork_core::models::KvCacheType::Q4_0),
                    "f32" => Some(agentcowork_core::models::KvCacheType::F32),
                    _ => Some(agentcowork_core::models::KvCacheType::F16),
                };
            }
            if parsed.num_ctx.is_none() {
                if let Some(num_ctx) = value.get("numCtx").and_then(serde_json::Value::as_u64) {
                    parsed.num_ctx = Some(num_ctx as u32);
                }
            }
            if parsed.flash_attn.is_none() {
                if let Some(flash_attn) = value.get("flashAttn").and_then(serde_json::Value::as_str)
                {
                    parsed.flash_attn = match flash_attn.to_ascii_lowercase().as_str() {
                        "on" => Some(FlashAttn::On),
                        "off" => Some(FlashAttn::Off),
                        _ => Some(FlashAttn::Auto),
                    };
                }
            }
            if parsed.gpu_layers.is_none() {
                if let Some(gpu_layers) = value.get("gpuLayers").and_then(serde_json::Value::as_i64)
                {
                    parsed.gpu_layers = Some(u32::try_from(gpu_layers).unwrap_or(0));
                }
            }
            parsed
        }
        None => ServeOptions::default(),
    };
    (opts, kv_cache)
}

/// Start and retain one managed model serve under a stable shell id.
///
/// The core call is synchronous through its health gate. Its returned handle
/// is moved directly into `AppState.model_serves`; no worker owns or drops it.
pub(crate) fn start_managed_serve(
    state: &AppState,
    id: &str,
    serve_options: Option<serde_json::Value>,
) -> Result<ModelServeStart, String> {
    let base = models_base();
    let registry = ModelRegistry::load(base);
    let entry = registry
        .get(id)
        .cloned()
        .ok_or_else(|| format!("model not in registry: {id}"))?;
    let cfg = agentcowork_core::Config::load().unwrap_or_default();
    let mgr = agentcowork_core::LocalManager::from_config(&cfg);
    let port = cfg.local.llamafile_port;
    let (mut opts, kv_cache) = parse_serve_options(serve_options.clone());

    use agentcowork_core::models::{mlx_quant_id, ServeRuntime};
    let is_mlx = opts.runtime == ServeRuntime::Mlx;
    if is_mlx && opts.model_id.is_none() {
        let hf_part = entry.id.rsplit(':').next().unwrap_or(&entry.id);
        opts.model_id = Some(mlx_quant_id(hf_part));
    }
    let kind = if is_mlx { "mlx" } else { "gguf" };
    let bin = if is_mlx {
        None
    } else {
        Some(mgr.find_llamafile(&cfg.data_dir).ok_or_else(|| {
            "no llamafile binary found — drop one in `<data_dir>/bin` or set `AGENTCOWORK_LLAMAFILE`"
                .to_string()
        })?)
    };

    {
        let mut registry = state
            .model_serves
            .lock()
            .map_err(|error| error.to_string())?;
        if registry.rows().iter().any(|row| row.port == port) {
            return Err(format!(
                "managed runtime port {port} is already retained by AgentCowork"
            ));
        }
    }

    let args_identity = serde_json::to_string(&(
        kind,
        entry.id.as_str(),
        port,
        cfg.local.num_ctx,
        kv_cache,
        &opts,
    ))
    .unwrap_or_default();
    let args_hash = crate::runtime_cmds::runtime_effect_args_hash(
        "runtime.start",
        &format!("{kind}\u{1f}{}\u{1f}{port}\u{1f}{args_identity}", entry.id),
    );
    crate::runtime_cmds::authorize_runtime_effect(
        state,
        "runtime.start",
        &format!("start managed {kind} runtime for {id}"),
        port,
        &args_hash,
    )?;

    let mut handle = ModelsRuntime::serve_gguf_with_options(
        &entry,
        bin.as_deref(),
        port,
        cfg.local.num_ctx,
        kv_cache,
        opts,
    )
    .map_err(|error| format!("managed runtime start failed: {error}"))?;
    let health = handle.health();
    let serve_id = format!("serve-{:04}", SERVE_COUNTER.fetch_add(1, Ordering::Relaxed));
    let result = ModelServeStart {
        serve_id: serve_id.clone(),
        port: handle.port(),
        base_url: handle.base_url.clone(),
        model_id: handle.model_id.clone(),
        config_hash: handle.config_hash.clone(),
        health,
        ownership: "Managed".to_string(),
    };
    state
        .model_serves
        .lock()
        .map_err(|error| error.to_string())?
        .insert(serve_id, kind.to_string(), now_ms(), handle)?;
    crate::control::record_mutation(
        state,
        crate::control::AuthKind::AgentTicket,
        "runtime.start",
        serde_json::json!({
            "serveId": result.serve_id,
            "kind": kind,
            "port": result.port,
            "modelId": result.model_id,
            "configHash": result.config_hash,
            "health": result.health,
            "ownership": result.ownership,
        }),
    );
    Ok(result)
}

/// Bind an installed model to a managed llamafile/MLX runtime.
///
/// The returned process handle is retained by AppState for its whole lifetime;
/// stopping the app drops the registry and invokes the handle's RAII cleanup.
#[tauri::command]
pub fn model_serve(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    serve_options: Option<serde_json::Value>,
) -> Result<ModelServeStart, String> {
    let result = start_managed_serve(&state, &id, serve_options)?;
    let status = DownloadStatus {
        phase: "served".into(),
        base_url: Some(result.base_url.clone()),
        ..Default::default()
    };
    emit_download(
        &app,
        "serve",
        &result.serve_id,
        &result.model_id,
        "",
        &status,
    );
    Ok(result)
}

/// List every model serve still retained in the managed process registry.
#[tauri::command]
pub fn model_serve_list(state: State<'_, AppState>) -> Result<Vec<ModelServeListRow>, String> {
    let mut registry = state
        .model_serves
        .lock()
        .map_err(|error| error.to_string())?;
    Ok(registry
        .rows()
        .into_iter()
        .map(|row| ModelServeListRow {
            id: row.id,
            kind: row.kind,
            health: row.health,
            port: row.port,
            model: row.model_id,
            config_hash: row.config_hash,
            started_at: row.started_at_ms,
        })
        .collect())
}

/// Remove one registered managed serve and stop/reap its retained child.
#[tauri::command]
pub fn model_serve_stop(
    state: State<'_, AppState>,
    serve_id: String,
) -> Result<serde_json::Value, String> {
    crate::runtime_cmds::stop_managed_serve(&state, &serve_id)
}

/// Test seam parity: the registry entry shape the UI expects (used by
/// `model_registry_list` consumers; kept here so the wire shape is pinned
/// next to the commands).
#[allow(dead_code)]
fn _entry_shape(_e: &ModelEntry) {}

/// P52.1 — dry-run fit estimate (no download): file GB + ctx tokens against
/// live hardware. Returns the tier (fits/may_be_slow/wont_fit), the
/// file/KV/total split, and the default quant. Nothing is downloaded.
#[tauri::command]
pub fn model_estimate_fit(file_gb: f64, ctx_tokens: u64) -> Result<serde_json::Value, String> {
    let hw = probe_hardware();
    let ram_gb = hw.available_ram_bytes as f64 / 1_073_741_824.0;
    let vram_gb = hw.gpu_vram_bytes.unwrap_or(0) as f64 / 1_073_741_824.0;
    let est = agentcowork_core::models::fit::estimate_fit(file_gb, ctx_tokens, ram_gb, vram_gb);
    Ok(serde_json::json!({
        "tier": est.tier,
        "fileGb": est.file_gb,
        "kvGb": est.kv_gb,
        "totalGb": est.total_gb,
        "ramGb": ram_gb,
        "vramGb": vram_gb,
        "defaultQuant": agentcowork_core::models::fit::DEFAULT_QUANT,
    }))
}

/// P52.2 — parse a LocalAI-style gallery `index.yaml` (no network, no
/// install). Returns the index or a parse error.
#[tauri::command]
pub fn model_gallery_parse(yaml: String) -> Result<serde_json::Value, String> {
    let index = agentcowork_catalog::gallery::load_index_yaml(&yaml).map_err(|e| e.to_string())?;
    serde_json::to_value(&index).map_err(|e| e.to_string())
}

/// P52.5 — pick the best weight build for this machine from a caller-supplied
/// candidate list (`[{repo,file,hw,quant}]`, hw ∈ npu/gpu/cpu). Pure pick —
/// download still goes through `model_download_start`.
#[tauri::command]
pub fn model_best_pick(
    hw: String,
    candidates: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let hw_class: agentcowork_core::models::best::HwClass =
        serde_json::from_value(serde_json::json!(hw)).map_err(|e| format!("bad hw: {e}"))?;
    let list: Vec<agentcowork_core::models::best::VariantCandidate> =
        serde_json::from_value(candidates).map_err(|e| format!("bad candidates: {e}"))?;
    Ok(agentcowork_core::models::best::best_variant(&hw_class, &list)
        .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
        .unwrap_or(serde_json::Value::Null))
}
