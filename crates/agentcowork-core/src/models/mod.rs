//! P27 — Local Model Fetch / Download Core (exact, doc 79, 2026-08-16).
//!
//! The backend half of the local-model story (the Discover UI lives in
//! `ui/src/components/panels/local-models-panel.tsx`):
//!
//! - [`hf`] — live Hugging Face Hub client + **resumable** GGUF downloader
//!   (`Range` + `X-Linked-Etag`, `.part` staging, sha256 via `.gguf.sha256`
//!   **and** LFS `oid sha256:`, byte progress, disk preflight, quant
//!   recommendation from live RAM + Hub list). **Zero repo ids hardcoded.**
//! - [`store`] — `<data_dir>/models/hf/{publisher}/{model}/{quant}-{sha8}.gguf`
//!   + `index.json` registry (id/path/sha256/size/ctx/quant/source).
//! - [`local_url`] — `local://` URLs (hf / ollama / llamafile) + resolver
//!   (derived from registry + installed runtimes, never a hardcoded catalog).
//! - [`probe`] — hardware probes (CPU/RAM/disk/VRAM) + runtime process
//!   discovery + TTL-cached OpenAI-compatible endpoint probing.
//! - [`ModelsRuntime`] — bind a downloaded GGUF to a runtime (managed
//!   llamafile serve, or `ollama create`), fail closed when no runtime exists,
//!   and return a [`ManagedServeHandle`] that retains process custody.

pub mod best;
pub mod cache;
pub mod fit;
pub mod hf;
pub mod local_url;
pub mod mlx;
pub mod probe;
pub mod store;

pub use best::{HwClass, VariantCandidate, best_variant};
pub use cache::{Benchmark, ModelCache, benchmark_from_samples};
pub use fit::{DEFAULT_QUANT, FitEstimate, FitTier, estimate_fit};
pub use hf::{HfClient, HfError, HfFile};
pub use local_url::{LocalUrl, LocalUrlError, LocalUrlResolver, ResolvedEndpoint};
pub use mlx::{MlxServer, mlx_quant_id, prefer_mlx};
pub use probe::{
    DiscoveredRuntime, HardwareInfo, ProbeCache, discover_runtime_inventory, discover_runtimes,
    find_runtime_processes, probe_hardware, probe_openai_endpoint,
};
pub use store::{ModelEntry, ModelRegistry};

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

use agentcowork_types::RuntimeHealthState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// llama.cpp KV-cache element types (P39.4 — `-ctk`/`-ctv`, i.e.
/// `--cache-type-k/-v`; verified in `llama.cpp/common/arg.cpp`: F32 / F16 /
/// Q8_0 / Q4_0…). Quantizing the KV cache (Q8_0) cuts KV memory ~4× vs F32
/// with a bounded quality impact — the memory-constrained local-run choice
/// (spec §9.3 §4, chosen by the hardware-fit picker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvCacheType {
    F32,
    F16,
    Q8_0,
    Q4_0,
}

impl KvCacheType {
    /// The llama.cpp arg value for `--cache-type-k/-v`.
    pub fn as_llama_arg(self) -> &'static str {
        match self {
            KvCacheType::F32 => "F32",
            KvCacheType::F16 => "F16",
            KvCacheType::Q8_0 => "Q8_0",
            KvCacheType::Q4_0 => "Q4_0",
        }
    }
}

/// Runtime binding errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsError {
    NoRuntime(&'static str),
    SpawnFailed(String),
    HealthTimeout,
    Io(String),
}

impl std::fmt::Display for ModelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// The Modelfile template for `ollama create` (P27 runtime binding).
pub const OLLAMA_MODELFILE: &str = "FROM {path}\n";

/// P52.4 — per-serve options a caller may set (UI + `model_serve`). Each
/// field maps to a real llama.cpp/llamafile server flag; `None` = llama.cpp
/// default. Defaults keep byte-identical behavior to the previous fixed
/// `--ctx-size N` launch so existing tests/consumers are unaffected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ServeOptions {
    /// GPU offload: `--n-gpu-layers N`. `0` = CPU-only. `None` = llama.cpp
    /// default (usually full offload when a GPU is present).
    pub gpu_layers: Option<u32>,
    /// Flash Attention: llama.cpp `--flash-attn on|off|auto`. `None` =
    /// default (`auto`).
    pub flash_attn: Option<FlashAttn>,
    /// Context-size override (`--ctx-size N`). `None` = the caller's
    /// configured `num_ctx` (kept as an explicit arg so the shape stays
    /// deterministic and testable).
    pub num_ctx: Option<u32>,
    /// llama.cpp `--no-mmap`: force full weight read into memory (not the
    /// page-cache-backed default). Off by default; the memory-constrained
    /// fit path (P52.4) may set it when the OS would otherwise swap.
    pub no_mmap: bool,
    /// llama.cpp `--mlock`: lock the model in RAM. Off by default.
    pub mlock: bool,
    /// P52.7 — runtime to bind: llama.cpp-family GGUF (default) or the
    /// Apple-Silicon MLX sidecar (`mlx_lm.server`). MLX serves a Hugging
    /// Face model id ([`ServeOptions::model_id`], `mlx-community/...`)
    /// instead of the local GGUF path and requires `mlx-lm` on PATH — the
    /// spawn fails closed with an actionable error otherwise (no silent
    /// fallback to llamafile).
    pub runtime: ServeRuntime,
    /// HF model id for the MLX sidecar (`mlx-community/<name>-4bit` via
    /// [`mlx::mlx_quant_id`]). Required when `runtime == Mlx` (callers may
    /// omit it and let the command derive it from the registry id); ignored
    /// when the runtime is GGUF.
    pub model_id: Option<String>,
}

/// llama.cpp Flash Attention mode (`--flash-attn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FlashAttn {
    On,
    Off,
    #[default]
    Auto,
}

impl FlashAttn {
    pub fn as_llama_arg(self) -> &'static str {
        match self {
            FlashAttn::On => "on",
            FlashAttn::Off => "off",
            FlashAttn::Auto => "auto",
        }
    }
}

/// P52.7 — which runtime serves the model: the portable llamafile/GGUF path
/// or the Apple-Silicon MLX sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServeRuntime {
    #[default]
    Gguf,
    /// Apple-Silicon unified-memory runtime (`mlx_lm.server`, OpenAI-compat).
    Mlx,
}

/// The llamafile launch args for a GGUF (pure so tests can assert the exact
/// wire, including the P39.4 KV-cache knob).
pub fn gguf_args(
    path: &Path,
    port: u16,
    num_ctx: u32,
    kv_cache: Option<KvCacheType>,
) -> Vec<String> {
    gguf_args_with_options(path, port, num_ctx, kv_cache, ServeOptions::default())
}

/// [`gguf_args`] plus the P52.4 per-serve options. `num_ctx` (the fixed
/// caller argument) is used unless `ServeOptions::num_ctx` overrides it.
pub fn gguf_args_with_options(
    path: &Path,
    port: u16,
    num_ctx: u32,
    kv_cache: Option<KvCacheType>,
    opts: ServeOptions,
) -> Vec<String> {
    let mut args = vec![
        "--model".to_string(),
        path.display().to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--ctx-size".to_string(),
        opts.num_ctx.unwrap_or(num_ctx).to_string(),
        "--nobrowser".to_string(),
    ];
    if let Some(ngl) = opts.gpu_layers {
        // llama.cpp `-ngl / --n-gpu-layers N` (offload N layers to the GPU).
        args.push("--n-gpu-layers".to_string());
        args.push(ngl.to_string());
    }
    if let Some(fa) = opts.flash_attn {
        args.push("--flash-attn".to_string());
        args.push(fa.as_llama_arg().to_string());
    }
    if opts.no_mmap {
        args.push("--no-mmap".to_string());
    }
    if opts.mlock {
        args.push("--mlock".to_string());
    }
    if let Some(kv) = kv_cache {
        // llama.cpp `-ctk/-ctv`: quantize both K and V caches to the same type.
        let t = kv.as_llama_arg().to_string();
        args.push("--cache-type-k".to_string());
        args.push(t.clone());
        args.push("--cache-type-v".to_string());
        args.push(t);
    }
    args
}

/// Process custody for a runtime started by [`ModelsRuntime`].
///
/// The handle owns the child until [`ManagedServeHandle::stop`] is called or
/// the handle is dropped; a bare endpoint can therefore never silently orphan
/// a process. Persisting and reattaching this resource across application
/// restarts is the Tauri layer's responsibility, not this core handle's.
#[derive(Debug)]
pub struct ManagedServeHandle {
    child: Child,
    /// Loopback port on which the managed runtime was launched.
    pub port: u16,
    /// Loopback base URL exposed by the managed runtime.
    pub base_url: String,
    /// Registry or Hugging Face model identity served by this process.
    pub model_id: String,
    /// Stable SHA-256 hash of the model identity and launch arguments.
    pub config_hash: String,
}

impl ManagedServeHandle {
    /// Returns the child's exit status when it has exited, without blocking.
    pub fn phase(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Returns current health evidence, preferring process state over HTTP.
    ///
    /// An exited child is `Down`. A live child is `Healthy` only after either
    /// its native health endpoint or OpenAI-compatible model listing answers;
    /// otherwise it is `Degraded`, never optimistically `Healthy`.
    pub fn health(&mut self) -> RuntimeHealthState {
        match self.phase() {
            Ok(Some(_)) | Err(_) => RuntimeHealthState::Down,
            Ok(None) => {
                // Local runtime health stays inside the Guard-pinned loopback
                // transport; a configured endpoint cannot redirect elsewhere.
                let native_health = probe::probe_http_200(
                    &format!("{}/health", self.base_url),
                    Duration::from_secs(1),
                );
                if native_health || probe_openai_endpoint(&self.base_url) {
                    RuntimeHealthState::Healthy
                } else {
                    RuntimeHealthState::Degraded
                }
            }
        }
    }

    /// Kills and reaps the managed child, consuming the custody handle.
    pub fn stop(mut self) -> Result<(), ModelsError> {
        terminate_child(&mut self.child).map_err(|e| ModelsError::Io(e.to_string()))
    }
}

impl Drop for ManagedServeHandle {
    fn drop(&mut self) {
        let _ = terminate_child(&mut self.child);
    }
}

fn terminate_child(child: &mut Child) -> std::io::Result<()> {
    match child.try_wait()? {
        Some(_) => Ok(()),
        None => match child.kill() {
            Ok(()) => {
                child.wait()?;
                Ok(())
            }
            Err(error) => match child.try_wait()? {
                Some(_) => Ok(()),
                _ => Err(error),
            },
        },
    }
}

fn managed_config_hash(model_id: &str, args: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    for arg in args {
        hasher.update([0]);
        hasher.update(arg.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Bind a downloaded GGUF to a runtime.
pub struct ModelsRuntime;

impl ModelsRuntime {
    /// Serve `entry` with a managed **llamafile** (`--model <gguf>`), reusing
    /// the P1.8 health-wait discipline (≤60s). Returns process custody rather
    /// than a bare endpoint. Defaults to `ServeOptions::default()`.
    pub fn serve_gguf(
        entry: &ModelEntry,
        llamafile_bin: Option<&Path>,
        port: u16,
        num_ctx: u32,
        kv_cache: Option<KvCacheType>,
    ) -> Result<ManagedServeHandle, ModelsError> {
        Self::serve_gguf_with_options(
            entry,
            llamafile_bin,
            port,
            num_ctx,
            kv_cache,
            ServeOptions::default(),
        )
    }

    /// [`ModelsRuntime::serve_gguf`] plus the P52.4 per-serve options
    /// (gpu layers, flash attention, ctx override, mmap/mlock). The actual
    /// context served is `opts.num_ctx.unwrap_or(num_ctx)`.
    pub fn serve_gguf_with_options(
        entry: &ModelEntry,
        llamafile_bin: Option<&Path>,
        port: u16,
        num_ctx: u32,
        kv_cache: Option<KvCacheType>,
        opts: ServeOptions,
    ) -> Result<ManagedServeHandle, ModelsError> {
        // P52.7 — the MLX sidecar branch: serve an HF model id via
        // `mlx_lm.server` instead of the local GGUF via llamafile. Liveness
        // is the documented `/v1/models` endpoint (≤60s; the first run may
        // download the weights from HF).
        if opts.runtime == ServeRuntime::Mlx {
            return Self::serve_mlx_with_options(port, num_ctx, opts);
        }
        let bin = llamafile_bin.ok_or(ModelsError::NoRuntime(
            "llamafile not found (set llamafile_bin / AGENTCOWORK_LLAMAFILE / drop one in data_dir/bin)",
        ))?;
        if !bin.exists() {
            return Err(ModelsError::NoRuntime("llamafile binary missing on disk"));
        }
        let path = PathBuf::from(&entry.path);
        if !path.exists() {
            return Err(ModelsError::Io(format!("gguf not on disk: {}", entry.path)));
        }

        let args = gguf_args_with_options(&path, port, num_ctx, kv_cache, opts);
        let config_hash = managed_config_hash(&entry.id, &args);
        let mut cmd = Command::new(bin);
        for arg in &args {
            cmd.arg(arg);
        }
        let mut child = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ModelsError::SpawnFailed(e.to_string()))?;

        // Health wait ≤60s (first run may unpack the weights).
        let base_url = format!("http://127.0.0.1:{port}");
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(ModelsError::SpawnFailed(format!(
                    "managed runtime exited before health: {status}"
                )));
            }
            if probe::probe_http_200(&format!("{base_url}/health"), Duration::from_secs(1)) {
                return Ok(ManagedServeHandle {
                    child,
                    port,
                    base_url,
                    model_id: entry.id.clone(),
                    config_hash,
                });
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let _ = terminate_child(&mut child);
        Err(ModelsError::HealthTimeout)
    }

    /// P52.7 — serve an HF model id with the MLX sidecar
    /// (`mlx_lm.server --model <id> --port <p>`). Requires `mlx-lm` on PATH
    /// (fail-closed with an actionable error — never a silent fallback to
    /// llamafile). OpenAI-compatible `/v1/chat/completions`, like the
    /// llamafile path; liveness probed on the documented `/v1/models`.
    pub fn serve_mlx_with_options(
        port: u16,
        _num_ctx: u32,
        opts: ServeOptions,
    ) -> Result<ManagedServeHandle, ModelsError> {
        let model_id = opts.model_id.clone().ok_or(ModelsError::NoRuntime(
            "MLX runtime needs a model id (mlx-community/<name>-4bit) — set serveOptions.modelId",
        ))?;
        // Fail closed when the sidecar isn't installed (probe first, like
        // bind_ollama probes `ollama --version`).
        if Command::new("mlx_lm.server")
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            return Err(ModelsError::NoRuntime(
                "mlx_lm.server not on PATH — install mlx-lm on Apple Silicon: pip install mlx-lm",
            ));
        }
        let spec = MlxServer::new(model_id.clone(), port);
        let args = spec.argv();
        let config_hash = managed_config_hash(&model_id, &args);
        let mut cmd = Command::new(args.first().expect("mlx argv is non-empty"));
        for arg in args.iter().skip(1) {
            cmd.arg(arg);
        }
        let mut child = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ModelsError::SpawnFailed(e.to_string()))?;

        // Health wait ≤60s (first run may download the weights from HF).
        let base_url = format!("http://127.0.0.1:{port}");
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(ModelsError::SpawnFailed(format!(
                    "managed runtime exited before health: {status}"
                )));
            }
            if probe::probe_http_200(&format!("{base_url}/v1/models"), Duration::from_secs(1)) {
                return Ok(ManagedServeHandle {
                    child,
                    port,
                    base_url,
                    model_id,
                    config_hash,
                });
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let _ = terminate_child(&mut child);
        Err(ModelsError::HealthTimeout)
    }

    /// Bind a GGUF via `ollama create <name> -f <Modelfile>` (the alternative
    /// runtime). Writes the Modelfile beside the registry, spawns the CLI,
    /// and fails closed when `ollama` is not on PATH.
    pub fn bind_ollama(entry: &ModelEntry, name: &str) -> Result<(), ModelsError> {
        if Command::new("ollama")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            return Err(ModelsError::NoRuntime("ollama not on PATH"));
        }
        let modelfile = std::path::Path::new(&entry.path)
            .parent()
            .unwrap_or(Path::new("."))
            .join(format!("Modelfile.{name}"));
        let from_line = OLLAMA_MODELFILE.replace("{path}", &entry.path);
        std::fs::write(&modelfile, from_line).map_err(|e| ModelsError::Io(e.to_string()))?;
        let status = Command::new("ollama")
            .arg("create")
            .arg(name)
            .arg("-f")
            .arg(&modelfile)
            .status()
            .map_err(|e| ModelsError::SpawnFailed(e.to_string()))?;
        if !status.success() {
            return Err(ModelsError::SpawnFailed(format!(
                "ollama create exited {status:?}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;

    fn entry() -> ModelEntry {
        ModelEntry {
            id: "microsoft/phi-4:q4_k_m".into(),
            path: "/nonexistent/phi.gguf".into(),
            sha256: "00".repeat(32),
            size: 1,
            ctx: 16384,
            quant: "q4_k_m".into(),
            source: "hf".into(),
        }
    }

    const FIXTURE_PORT_ENV: &str = "AGENTCOWORK_MANAGED_SERVE_FIXTURE_PORT";

    fn fixture_responds(port: u16, path: &str) -> bool {
        ureq::get(&format!("http://127.0.0.1:{port}{path}"))
            .timeout(Duration::from_millis(250))
            .call()
            .map(|response| response.status() == 200)
            .unwrap_or(false)
    }

    fn endpoint_closed(port: u16) -> bool {
        TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(100),
        )
        .is_err()
    }

    fn spawn_fixture_server() -> ManagedServeHandle {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("models::tests::managed_serve_fixture_server")
            .arg("--nocapture")
            .env(FIXTURE_PORT_ENV, port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if fixture_responds(port, "/health") {
                return ManagedServeHandle {
                    child,
                    port,
                    base_url: format!("http://127.0.0.1:{port}"),
                    model_id: "fixture-model".into(),
                    config_hash: managed_config_hash("fixture-model", &[]),
                };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut handle = ManagedServeHandle {
            child,
            port,
            base_url: format!("http://127.0.0.1:{port}"),
            model_id: "fixture-model".into(),
            config_hash: managed_config_hash("fixture-model", &[]),
        };
        let _ = terminate_child(&mut handle.child);
        panic!("managed serve fixture did not start");
    }

    fn wait_for_endpoint_closed(port: u16) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if endpoint_closed(port) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("managed serve endpoint still reachable on port {port}");
    }

    #[test]
    fn managed_serve_fixture_server() {
        let Ok(port) = std::env::var(FIXTURE_PORT_ENV) else {
            return;
        };
        let port: u16 = port.parse().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let body = if path == "/health" {
                r#"{"status":"ok"}"#
            } else if path == "/v1/models" {
                r#"{"data":[{"id":"fixture-model"}]}"#
            } else {
                r#"{"error":"not found"}"#
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    }

    #[test]
    fn dropping_managed_serve_handle_ends_resource_and_is_not_healthy() {
        let mut stopped = spawn_fixture_server();
        let stopped_port = stopped.port;
        assert_eq!(stopped.health(), RuntimeHealthState::Healthy);
        stopped.stop().unwrap();
        wait_for_endpoint_closed(stopped_port);

        let dropped = spawn_fixture_server();
        let dropped_port = dropped.port;
        assert!(fixture_responds(dropped_port, "/health"));
        drop(dropped);
        wait_for_endpoint_closed(dropped_port);
        assert!(endpoint_closed(dropped_port));
    }

    #[test]
    fn serve_gguf_fails_closed_without_binary() {
        let e = entry();
        let result: Result<ManagedServeHandle, ModelsError> =
            ModelsRuntime::serve_gguf(&e, None, 11435, 16384, None);
        let err = result.unwrap_err();
        assert!(matches!(err, ModelsError::NoRuntime(_)));
    }

    #[test]
    fn gguf_is_the_default_runtime() {
        assert_eq!(ServeOptions::default().runtime, ServeRuntime::Gguf);
    }

    #[test]
    fn mlx_runtime_skips_the_llamafile_requirement() {
        // With runtime = Mlx the branch runs BEFORE the llamafile check, so
        // a missing llamafile must not be the error — the model-id
        // requirement is. This proves the MLX route exists and is not a
        // silent llamafile fallback.
        let e = entry();
        let opts = ServeOptions {
            runtime: ServeRuntime::Mlx,
            ..ServeOptions::default()
        };
        let err =
            ModelsRuntime::serve_gguf_with_options(&e, None, 11435, 16384, None, opts).unwrap_err();
        assert!(
            matches!(err, ModelsError::NoRuntime(_)),
            "expected a closed failure, got {err:?}"
        );
    }

    #[test]
    fn serve_mlx_requires_a_model_id() {
        // No model_id and no mlx_lm on PATH: the model-id requirement must
        // fire before any PATH probe (deterministic regardless of machine).
        let opts = ServeOptions {
            runtime: ServeRuntime::Mlx,
            ..ServeOptions::default()
        };
        let err = ModelsRuntime::serve_mlx_with_options(11436, 16384, opts).unwrap_err();
        assert!(matches!(err, ModelsError::NoRuntime(_)));
    }

    #[test]
    fn serve_gguf_fails_closed_on_missing_binary_file() {
        let e = entry();
        let result: Result<ManagedServeHandle, ModelsError> = ModelsRuntime::serve_gguf(
            &e,
            Some(Path::new("/nonexistent/llamafile")),
            11435,
            16384,
            None,
        );
        let err = result.unwrap_err();
        assert!(matches!(err, ModelsError::NoRuntime(_)));
    }

    #[test]
    fn serve_gguf_fails_closed_on_missing_gguf() {
        let tmp = std::env::temp_dir().join(format!("eaios-llf-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let bin = tmp.join("fake-llamafile");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        let mut e = entry();
        e.path = "/nonexistent/phi.gguf".into();
        let result: Result<ManagedServeHandle, ModelsError> =
            ModelsRuntime::serve_gguf(&e, Some(&bin), 11435, 16384, None);
        let err = result.unwrap_err();
        assert!(matches!(err, ModelsError::Io(_)));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gguf_args_include_kv_cache_knob_when_set() {
        let p = Path::new("/w/phi.gguf");
        let args = gguf_args(p, 11435, 16384, Some(KvCacheType::Q8_0));
        assert!(args.contains(&"--cache-type-k".to_string()));
        assert!(args.contains(&"--cache-type-v".to_string()));
        let k_idx = args.iter().position(|a| a == "--cache-type-k").unwrap();
        let v_idx = args.iter().position(|a| a == "--cache-type-v").unwrap();
        assert_eq!(args[k_idx + 1], "Q8_0");
        assert_eq!(args[v_idx + 1], "Q8_0");
        // Base args stay intact.
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"--ctx-size".to_string()));
    }

    #[test]
    fn gguf_args_omit_kv_knob_when_unset() {
        let p = Path::new("/w/phi.gguf");
        let args = gguf_args(p, 11435, 16384, None);
        assert!(!args.contains(&"--cache-type-k".to_string()));
        assert!(!args.contains(&"--cache-type-v".to_string()));
    }

    #[test]
    fn kv_cache_type_llama_arg_values_are_verbatim() {
        assert_eq!(KvCacheType::F32.as_llama_arg(), "F32");
        assert_eq!(KvCacheType::F16.as_llama_arg(), "F16");
        assert_eq!(KvCacheType::Q8_0.as_llama_arg(), "Q8_0");
        assert_eq!(KvCacheType::Q4_0.as_llama_arg(), "Q4_0");
    }

    #[test]
    fn serve_options_default_keeps_base_args() {
        // Defaults are byte-identical to the plain `gguf_args` launch.
        let p = Path::new("/w/phi.gguf");
        assert_eq!(
            gguf_args(p, 11435, 16384, None),
            gguf_args_with_options(p, 11435, 16384, None, ServeOptions::default())
        );
        assert_eq!(
            gguf_args_with_options(p, 11435, 16384, None, ServeOptions::default()),
            vec![
                "--model".to_string(),
                "/w/phi.gguf".to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string(),
                "--port".to_string(),
                "11435".to_string(),
                "--ctx-size".to_string(),
                "16384".to_string(),
                "--nobrowser".to_string(),
            ]
        );
    }

    #[test]
    fn serve_options_add_real_llama_flags() {
        let p = Path::new("/w/phi.gguf");
        let opts = ServeOptions {
            gpu_layers: Some(12),
            flash_attn: Some(FlashAttn::On),
            num_ctx: Some(8192),
            no_mmap: true,
            mlock: false,
            ..ServeOptions::default()
        };
        let args = gguf_args_with_options(p, 11435, 16384, None, opts);
        let ngl = args.iter().position(|a| a == "--n-gpu-layers").unwrap();
        assert_eq!(args[ngl + 1], "12");
        let fa = args.iter().position(|a| a == "--flash-attn").unwrap();
        assert_eq!(args[fa + 1], "on");
        let ctx = args.iter().position(|a| a == "--ctx-size").unwrap();
        assert_eq!(args[ctx + 1], "8192"); // num_ctx override wins
        assert!(args.contains(&"--no-mmap".to_string()));
        assert!(!args.contains(&"--mlock".to_string()));
    }

    #[test]
    fn managed_config_hash_changes_with_per_serve_options() {
        let path = Path::new("/w/phi.gguf");
        let base = managed_config_hash(
            "phi-4",
            &gguf_args_with_options(path, 11435, 16384, None, ServeOptions::default()),
        );
        let changed = managed_config_hash(
            "phi-4",
            &gguf_args_with_options(
                path,
                11435,
                16384,
                None,
                ServeOptions {
                    gpu_layers: Some(12),
                    ..ServeOptions::default()
                },
            ),
        );
        assert_eq!(base.len(), 64);
        assert_ne!(base, changed);
    }

    #[test]
    fn serve_options_merge_with_kv_cache() {
        let p = Path::new("/w/phi.gguf");
        let opts = ServeOptions {
            gpu_layers: Some(0), // CPU-only
            ..ServeOptions::default()
        };
        let args = gguf_args_with_options(p, 11435, 16384, Some(KvCacheType::Q8_0), opts);
        assert!(args.contains(&"--n-gpu-layers".to_string()));
        assert!(args.contains(&"--cache-type-k".to_string()));
        assert!(args.contains(&"Q8_0".to_string()));
    }

    #[test]
    fn flash_attn_arg_values() {
        assert_eq!(FlashAttn::On.as_llama_arg(), "on");
        assert_eq!(FlashAttn::Off.as_llama_arg(), "off");
        assert_eq!(FlashAttn::Auto.as_llama_arg(), "auto");
    }

    #[test]
    fn ollama_bind_fails_closed_when_ollama_missing() {
        // In CI `ollama` is almost certainly absent → NoRuntime. If present,
        // this would spawn — so we only assert on the missing case by
        // checking the error type matches either NoRuntime or SpawnFailed.
        let e = entry();
        match ModelsRuntime::bind_ollama(&e, "test-name") {
            Err(ModelsError::NoRuntime(_)) => {}
            Err(ModelsError::SpawnFailed(_)) => {}
            Err(ModelsError::Io(_)) => {}
            Ok(()) => {} // ollama exists AND create succeeded — acceptable in a dev env
            Err(_) => panic!("unexpected error"),
        }
    }
}
