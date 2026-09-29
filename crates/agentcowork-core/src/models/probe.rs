//! P27 — hardware probes + runtime process discovery (exact, doc 79).
//!
//! - Hardware: CPU name, total/available RAM, disk free, GPU VRAM (best-effort
//!   via `nvidia-smi` when present — never required).
//! - Processes: discover llama.cpp / ollama / llamafile processes from the
//!   OS process table, and probe OpenAI-compatible localhost endpoints.
//! - TTL cache so repeated discovery is cheap (default 10s).

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agentcowork_types::{RuntimeHealthState, RuntimeInventoryEntry, RuntimeOwnership};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct HardwareInfo {
    pub cpu_name: String,
    pub total_ram_bytes: u64,
    pub available_ram_bytes: u64,
    pub disk_free_bytes: Option<u64>,
    /// VRAM in bytes; `None` when no GPU tool is present/working.
    pub gpu_vram_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiscoveredRuntime {
    pub name: String, // "ollama" | "llamafile" | "llama.cpp" | ...
    pub pid: Option<u32>,
    pub endpoint: Option<String>, // OpenAI-compatible base URL when probed
}

/// Probe the host hardware.
pub fn probe_hardware() -> HardwareInfo {
    let sys = sysinfo::System::new_all();
    // sysinfo 0.30 returns memory in bytes (not KiB). Multiplying by 1024
    // overstated RAM ~1024× and made `model_estimate_fit` report Fits for
    // every GGUF. Keep this aligned with `hwfit::detect`.
    let total_ram_bytes = sys.total_memory();
    let available_ram_bytes = sys.available_memory();

    let cpu_name = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let disk_free_bytes = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .map(|d| d.available_space())
        .max();

    HardwareInfo {
        cpu_name,
        total_ram_bytes,
        available_ram_bytes,
        disk_free_bytes,
        gpu_vram_bytes: probe_nvidia_vram(),
    }
}

/// Best-effort NVIDIA VRAM via `nvidia-smi` (absent on non-NVIDIA machines).
fn probe_nvidia_vram() -> Option<u64> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mb: f64 = text
        .lines()
        .next()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((mb * 1024.0 * 1024.0) as u64)
}

const RUNTIME_NAMES: &[&str] = &[
    "ollama",
    "llamafile",
    "llama-server",
    "llama.cpp",
    "llama-cli",
];

/// Process-table scan for local AI runtimes (Linux `/proc`, best-effort).
pub fn find_runtime_processes() -> Vec<DiscoveredRuntime> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let pid: u32 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .map(|s| s.replace('\0', " "))
            .unwrap_or_default();
        for rt in RUNTIME_NAMES {
            if name.contains(rt) || cmdline.contains(rt) {
                out.push(DiscoveredRuntime {
                    name: rt.to_string(),
                    pid: Some(pid),
                    endpoint: None,
                });
                break;
            }
        }
    }
    out
}

/// OpenAI-compatible endpoint probe: `GET {base}/v1/models` must 200.
///
/// FIX-09: `base` reaches this function from the renderer's runtime-discovery
/// form, so it is caller-supplied. Guard pins the checked address set into the
/// request, refuses redirects and caps the response; the desktop default still
/// refuses LAN and always-blocked ranges while loopback runtimes remain usable.
/// A denial is simply "not reachable" — the probe has no direct-client fallback.
pub fn probe_openai_endpoint(base: &str) -> bool {
    let url = format!("{base}/v1/models");
    probe_http_200(&url, Duration::from_secs(2))
}

pub(crate) fn probe_http_200(url: &str, timeout: Duration) -> bool {
    let Ok(client) = agentcowork_guard::egress_http::GuardedHttpClient::new(
        url,
        agentcowork_guard::NetPolicy::default(),
        timeout,
    ) else {
        return false;
    };
    client
        .request("GET", url, &[], None, 1024 * 1024)
        .is_ok_and(|response| response.status == 200)
}

/// TTL-cached probe of candidate localhost endpoints (default 10s).
#[derive(Debug, Default)]
pub struct ProbeCache {
    hits: HashMap<String, (bool, Instant)>,
    ttl: Duration,
}

impl ProbeCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            hits: HashMap::new(),
            ttl,
        }
    }

    pub fn with_default_ttl() -> Self {
        Self::new(Duration::from_secs(10))
    }

    /// Probe `base` (cached for the TTL). Deterministic, cheap to call
    /// repeatedly — e.g. every picker open.
    pub fn probe(&mut self, base: &str) -> bool {
        if let Some((ok, at)) = self.hits.get(base) {
            if at.elapsed() < self.ttl {
                return *ok;
            }
        }
        let ok = probe_openai_endpoint(base);
        self.hits.insert(base.to_string(), (ok, Instant::now()));
        ok
    }
}

/// Full discovery: process table + endpoint probes, TTL-cached.
pub fn discover_runtimes(
    cache: &mut ProbeCache,
    candidate_bases: &[String],
) -> Vec<DiscoveredRuntime> {
    let mut out = find_runtime_processes();
    for base in candidate_bases {
        if cache.probe(base) {
            if let Some(existing) = out
                .iter_mut()
                .find(|r| r.name == "ollama" && base.contains("11434"))
            {
                existing.endpoint = Some(base.clone());
            } else if let Some(existing) = out.iter_mut().find(|r| r.name == "llamafile") {
                existing.endpoint = Some(base.clone());
            } else {
                out.push(DiscoveredRuntime {
                    name: "openai-compatible".into(),
                    pid: None,
                    endpoint: Some(base.clone()),
                });
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
struct RuntimeProbeCandidate {
    probe_base: String,
    endpoint: String,
    socket: SocketAddr,
}

fn runtime_probe_candidate(probe_base: &str, endpoint: &str) -> Option<RuntimeProbeCandidate> {
    let authority = probe_base.strip_prefix("http://")?;
    Some(RuntimeProbeCandidate {
        probe_base: probe_base.trim_end_matches('/').to_string(),
        endpoint: endpoint.to_string(),
        socket: authority.parse().ok()?,
    })
}

fn well_known_runtime_candidates() -> Vec<RuntimeProbeCandidate> {
    [
        ("http://127.0.0.1:11434", "http://127.0.0.1:11434"),
        ("http://127.0.0.1:1234", "http://127.0.0.1:1234/v1"),
        ("http://127.0.0.1:1337", "http://127.0.0.1:1337/v1"),
        ("http://127.0.0.1:8000", "http://127.0.0.1:8000/v1"),
    ]
    .into_iter()
    .filter_map(|(probe_base, endpoint)| runtime_probe_candidate(probe_base, endpoint))
    .collect()
}

fn parse_ollama_models(body: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let models = value.get("models")?.as_array()?;
    Some(
        models
            .iter()
            .filter_map(|model| {
                model
                    .get("name")
                    .or_else(|| model.get("model"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
    )
}

fn probe_ollama_models(probe_base: &str) -> Option<Vec<String>> {
    let url = format!("{probe_base}/api/tags");
    let client = agentcowork_guard::egress_http::GuardedHttpClient::new(
        &url,
        agentcowork_guard::NetPolicy::default(),
        Duration::from_secs(2),
    )
    .ok()?;
    let response = client.request("GET", &url, &[], None, 1024 * 1024).ok()?;
    if response.status != 200 {
        return None;
    }
    parse_ollama_models(&String::from_utf8_lossy(&response.body))
}

fn fetch_openai_models(probe_base: &str) -> Option<Vec<String>> {
    let url = format!("{probe_base}/v1/models");
    let client = agentcowork_guard::egress_http::GuardedHttpClient::new(
        &url,
        agentcowork_guard::NetPolicy::default(),
        Duration::from_secs(2),
    )
    .ok()?;
    let response = client.request("GET", &url, &[], None, 1024 * 1024).ok()?;
    if response.status != 200 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&response.body).ok()?;
    let models = value.get("data")?.as_array()?;
    Some(
        models
            .iter()
            .filter_map(|model| {
                model
                    .get("id")
                    .or_else(|| model.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
    )
}

fn inventory_entry(
    discovered: DiscoveredRuntime,
    candidate: &RuntimeProbeCandidate,
    protocol: &str,
    health: RuntimeHealthState,
    models: Vec<String>,
) -> RuntimeInventoryEntry {
    RuntimeInventoryEntry {
        id: format!("{}@{}", discovered.name, candidate.socket),
        kind: discovered.name,
        endpoint: candidate.endpoint.clone(),
        version: None,
        protocol: protocol.to_string(),
        ownership: RuntimeOwnership::External,
        health,
        last_probe_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
        models,
        agent_compatibility: Vec::new(),
    }
}

fn discover_runtime_inventory_from(
    candidates: &[RuntimeProbeCandidate],
) -> Vec<RuntimeInventoryEntry> {
    let mut inventory = Vec::new();
    for candidate in candidates {
        if TcpStream::connect_timeout(&candidate.socket, Duration::from_millis(150)).is_err() {
            continue;
        }

        if let Some(models) = probe_ollama_models(&candidate.probe_base) {
            inventory.push(inventory_entry(
                DiscoveredRuntime {
                    name: "ollama".into(),
                    pid: None,
                    endpoint: Some(candidate.endpoint.clone()),
                },
                candidate,
                "ollama",
                RuntimeHealthState::Observed,
                models,
            ));
            continue;
        }

        if probe_openai_endpoint(&candidate.probe_base) {
            inventory.push(inventory_entry(
                DiscoveredRuntime {
                    name: "generic_openai_compatible".into(),
                    pid: None,
                    endpoint: Some(candidate.endpoint.clone()),
                },
                candidate,
                "openai_compatible",
                RuntimeHealthState::Observed,
                fetch_openai_models(&candidate.probe_base).unwrap_or_default(),
            ));
            continue;
        }

        inventory.push(inventory_entry(
            DiscoveredRuntime {
                name: "generic_openai_compatible".into(),
                pid: None,
                endpoint: Some(candidate.endpoint.clone()),
            },
            candidate,
            "unknown",
            RuntimeHealthState::Unsupported,
            Vec::new(),
        ));
    }
    inventory
}

/// Discovers the canonical inventory of user-run loopback model runtimes.
///
/// Well-known ports are hints only. Runtime kind comes from a protocol
/// handshake; otherwise the entry is `generic_openai_compatible`. Every entry
/// remains externally owned, and successful discovery is only `Observed` until
/// a deeper health probe completes. Model listings never create an
/// agent-compatibility claim.
pub fn discover_runtime_inventory() -> Vec<RuntimeInventoryEntry> {
    discover_runtime_inventory_from(&well_known_runtime_candidates())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_responder(routes: &[(&str, &str)]) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let routes = routes
            .iter()
            .map(|(path, body)| ((*path).to_string(), (*body).to_string()))
            .collect::<Vec<_>>();
        thread::spawn(move || {
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
                let (status, body) = routes
                    .iter()
                    .find(|(route, _)| *route == path)
                    .map(|(_, body)| ("200 OK", body.as_str()))
                    .unwrap_or(("404 Not Found", r#"{"error":"not found"}"#));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        address
    }

    fn candidate_for(address: SocketAddr) -> RuntimeProbeCandidate {
        runtime_probe_candidate(
            &format!("http://{address}"),
            &format!("http://{address}/v1"),
        )
        .unwrap()
    }

    #[test]
    fn runtime_inventory_is_empty_when_no_candidate_is_reachable() {
        assert!(discover_runtime_inventory_from(&[]).is_empty());
    }

    #[test]
    fn ollama_handshake_identifies_runtime_without_claiming_agent_usability() {
        let address = spawn_responder(&[(
            "/api/tags",
            r#"{"models":[{"name":"qwen3:4b"},{"model":"llama3.2:1b"}]}"#,
        )]);
        let inventory = discover_runtime_inventory_from(&[candidate_for(address)]);
        assert_eq!(inventory.len(), 1);
        let runtime = &inventory[0];
        assert_eq!(runtime.kind, "ollama");
        assert_eq!(runtime.protocol, "ollama");
        assert_eq!(runtime.ownership, RuntimeOwnership::External);
        assert_eq!(runtime.health, RuntimeHealthState::Observed);
        assert_eq!(runtime.models, vec!["qwen3:4b", "llama3.2:1b"]);
        assert!(runtime.agent_compatibility.is_empty());
    }

    #[test]
    fn openai_models_handshake_falls_back_to_generic_runtime() {
        let address = spawn_responder(&[("/v1/models", r#"{"data":[{"id":"fixture-model"}]}"#)]);
        let inventory = discover_runtime_inventory_from(&[candidate_for(address)]);
        assert_eq!(inventory.len(), 1);
        let runtime = &inventory[0];
        assert_eq!(runtime.kind, "generic_openai_compatible");
        assert_eq!(runtime.protocol, "openai_compatible");
        assert_eq!(runtime.ownership, RuntimeOwnership::External);
        assert_eq!(runtime.health, RuntimeHealthState::Observed);
        assert_eq!(runtime.models, vec!["fixture-model"]);
        assert!(runtime.agent_compatibility.is_empty());
    }

    #[test]
    fn open_port_without_protocol_handshake_stays_unsupported() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        let inventory = discover_runtime_inventory_from(&[candidate_for(address)]);
        assert_eq!(inventory.len(), 1);
        let runtime = &inventory[0];
        assert_eq!(runtime.kind, "generic_openai_compatible");
        assert_eq!(runtime.protocol, "unknown");
        assert_eq!(runtime.health, RuntimeHealthState::Unsupported);
        assert_ne!(runtime.health, RuntimeHealthState::Healthy);
        assert!(runtime.models.is_empty());
        assert!(runtime.agent_compatibility.is_empty());
    }

    #[test]
    fn probe_cache_honors_ttl() {
        let mut cache = ProbeCache::new(Duration::from_millis(50));
        // localhost with nothing listening → false, cached.
        let base = "http://127.0.0.1:59999".to_string();
        let first = cache.probe(&base);
        let second = cache.probe(&base); // cached (same result)
        assert_eq!(first, second);
        // After the TTL, it re-probes (still false — nothing listening).
        std::thread::sleep(Duration::from_millis(60));
        let third = cache.probe(&base);
        assert!(!third);
    }

    #[test]
    fn hardware_probe_returns_basics() {
        let h = probe_hardware();
        assert!(h.total_ram_bytes > 0, "RAM probe must find something");
        assert!(h.available_ram_bytes <= h.total_ram_bytes);
    }

    #[test]
    fn runtime_name_matching_covers_all() {
        // The RUNTIME_NAMES list is what the process scan keys on.
        assert!(RUNTIME_NAMES.contains(&"ollama"));
        assert!(RUNTIME_NAMES.contains(&"llamafile"));
    }
}
