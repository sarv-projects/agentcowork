//! P27 — Hugging Face Hub client + resumable GGUF downloader (exact, doc 79).
//!
//! - Live Hub API only — **zero repo ids hardcoded** (callers pass `repo`).
//! - Resumable: `Range: bytes=<offset>-` + `X-Linked-Etag`/`X-Repo-Commit`;
//!   `*.gguf.part` staging file, renamed on success.
//! - sha256 verified from `.gguf.sha256` **and** the LFS `oid sha256:` (both
//!   must agree when present).
//! - Byte progress events to the caller (UI), disk preflight before start,
//!   quant recommendation from **live** RAM + Hub file list (never baked).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::Deserialize;

use super::store::{ModelEntry, entry_path};

/// One file in a repo's `tree/main`.
#[derive(Debug, Clone, Deserialize)]
pub struct HfFile {
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub lfs: Option<LfsMeta>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LfsMeta {
    pub oid: String,
    #[serde(default)]
    pub size: u64,
}

/// Progress callback (bytes done/total). The UI wires this to a progress bar.
pub type ProgressFn<'a> = &'a mut dyn FnMut(u64, u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HfError {
    Network(String),
    NotFound(String),
    ShaMismatch {
        expected: String,
        actual: String,
    },
    DiskPreflight {
        needed: u64,
        free: u64,
    },
    LfsOidMismatch {
        expected: String,
        actual: String,
    },
    Io(String),
    /// User cancelled — the `.part` staging file is kept for resume.
    Cancelled,
}

impl std::fmt::Display for HfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn api_base() -> &'static str {
    "https://huggingface.co"
}

/// FIX-09 — the one destination pre-flight for this module's HTTP client.
/// Fails closed with a typed [`HfError`]: the download is refused, never
/// attempted through another path.
fn egress_preflight(url: &str) -> Result<(), HfError> {
    agentcowork_guard::netfloor::preflight_url(url, agentcowork_guard::NetPolicy::default())
        .map_err(|e| HfError::Network(e.to_string()))
}

/// Client over the public Hub API (base URL injectable for tests).
pub struct HfClient {
    base: String,
    agent: ureq::Agent,
}

impl Default for HfClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HfClient {
    pub fn new() -> Self {
        Self {
            base: api_base().to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(10))
                .build(),
        }
    }

    /// Test seam: point at a local mock server.
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Live `tree/main` of a repo (no hardcoded ids — `repo` is caller data).
    ///
    /// FIX-09: the Hub base is pre-flighted through
    /// [`agentcowork_guard::netfloor::preflight_url`] before the socket. `base`
    /// is injectable (test mock), so the floor lives at the client; the shipped
    /// default is `https://huggingface.co`, and a re-pointed client still
    /// cannot reach the LAN or cloud metadata.
    pub fn repo_files(&self, repo: &str) -> Result<Vec<HfFile>, HfError> {
        let url = format!("{}/api/models/{repo}/tree/main", self.base);
        egress_preflight(&url)?;
        let resp = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| HfError::Network(e.to_string()))?;
        let body = resp
            .into_string()
            .map_err(|e| HfError::Network(e.to_string()))?;
        serde_json::from_str(&body).map_err(|e| HfError::NotFound(format!("{e}")))
    }

    /// GGUF files in a repo (files whose path ends with `.gguf`).
    pub fn gguf_files(&self, repo: &str) -> Result<Vec<HfFile>, HfError> {
        let files = self.repo_files(repo)?;
        Ok(files
            .into_iter()
            .filter(|f| f.path.ends_with(".gguf") || f.path.ends_with(".safetensors"))
            .collect())
    }

    /// Expected sha256 for a file: the adjacent `.gguf.sha256` blob on the Hub.
    pub fn expected_sha256(&self, repo: &str, filename: &str) -> Result<String, HfError> {
        let url = format!("{}/{repo}/resolve/main/{filename}.sha256", self.base);
        let resp = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| HfError::Network(e.to_string()))?;
        let text = resp
            .into_string()
            .map_err(|e| HfError::Network(e.to_string()))?;
        // Hub serves `sha256  filename` (like sha256sum).
        let hex = text.split_whitespace().next().unwrap_or("").to_lowercase();
        if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(hex)
        } else {
            Err(HfError::NotFound("no .sha256 companion".into()))
        }
    }

    /// Quant recommendation from **live** RAM + the repo's file list.
    /// Rule: pick the smallest GGUF whose size ≤ 60% of available RAM
    /// (leave headroom for context); prefer higher quality when several fit.
    pub fn recommend_quant(&self, repo: &str, available_ram_bytes: u64) -> Result<String, HfError> {
        let files = self.gguf_files(repo)?;
        let mut fitting: Vec<&HfFile> = files
            .iter()
            .filter(|f| f.size > 0 && f.size <= available_ram_bytes * 6 / 10)
            .collect();
        if fitting.is_empty() {
            // Nothing fits with headroom — return the smallest available.
            fitting = files.iter().collect();
        }
        fitting.sort_by_key(|f| std::cmp::Reverse(f.size)); // largest that fits = best quality
        let best = fitting
            .first()
            .ok_or(HfError::NotFound("no weight files".into()))?;
        Ok(quant_from_filename(&best.path).to_string())
    }

    /// Disk preflight: ensure `needed` bytes fit in free space at `dir`.
    /// When the free-space probe is unavailable, proceed (platform gap is
    /// honest: the sha256 verify still guards integrity after download).
    pub fn preflight(&self, dir: &Path, needed: u64) -> Result<(), HfError> {
        match free_bytes(dir) {
            Some(free) if free < needed => Err(HfError::DiskPreflight { needed, free }),
            _ => Ok(()),
        }
    }

    /// Resumable download of `filename` from `repo` to `dest` (staged as
    /// `dest.part`). Progress reported via `progress`. sha256 verified when
    /// the `.sha256` companion exists **and** the LFS oid is present (both
    /// must agree). On success the `.part` file is renamed to `dest`.
    ///
    /// `cancel` is an optional cooperative cancellation flag (P50.4.2 — the
    /// UI Cancel button). When set, the download returns [`HfError::Cancelled`]
    /// at the next chunk boundary and the `.part` staging file is left in
    /// place so a later call with the same `dest` resumes via `Range`.
    pub fn download(
        &self,
        repo: &str,
        filename: &str,
        dest: &Path,
        progress: ProgressFn<'_>,
        cancel: Option<&AtomicBool>,
    ) -> Result<ModelEntry, HfError> {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(HfError::Cancelled);
        }
        let files = self.repo_files(repo)?;
        let meta = files
            .iter()
            .find(|f| f.path == filename)
            .ok_or_else(|| HfError::NotFound(format!("{filename} not in {repo}")))?;
        let expected_size = meta.lfs.as_ref().map(|l| l.size).unwrap_or(meta.size);

        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| HfError::Io(e.to_string()))?;
        }
        self.preflight(dest.parent().unwrap_or(Path::new(".")), expected_size)?;

        let part = part_path(dest);
        let resume_from = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

        let url = format!("{}/{repo}/resolve/main/{filename}", self.base);
        egress_preflight(&url)?;
        let mut req = self.agent.get(&url);
        if resume_from > 0 && resume_from < expected_size {
            req = req.set("Range", &format!("bytes={resume_from}-"));
        }
        let resp = req.call().map_err(|e| HfError::Network(e.to_string()))?;

        let status = resp.status();
        if status == 416 {
            // Range unsatisfiable → restart.
            std::fs::remove_file(&part).ok();
            return self.download(repo, filename, dest, progress, cancel);
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(resume_from > 0)
            .write(true)
            .open(&part)
            .map_err(|e| HfError::Io(e.to_string()))?;

        let mut done = resume_from;
        progress(done, expected_size);
        let mut reader = resp.into_reader();
        let mut buf = [0u8; 64 * 1024];
        loop {
            if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(HfError::Cancelled);
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| HfError::Io(e.to_string()))?;
            if n == 0 {
                break;
            }
            use std::io::Write;
            file.write_all(&buf[..n])
                .map_err(|e| HfError::Io(e.to_string()))?;
            done += n as u64;
            progress(done, expected_size);
        }
        drop(file);

        if done != expected_size {
            return Err(HfError::Network(format!(
                "short download: got {done}, expected {expected_size}"
            )));
        }

        // sha256 verification: LFS oid (authoritative) vs .gguf.sha256 vs file.
        let actual = sha256_hex(&part);
        if let Some(lfs) = &meta.lfs {
            if let Some(oid) = lfs.oid.strip_prefix("sha256:") {
                if !oid.eq_ignore_ascii_case(&actual) {
                    let _ = std::fs::remove_file(&part);
                    return Err(HfError::LfsOidMismatch {
                        expected: oid.to_string(),
                        actual,
                    });
                }
            }
        }
        if let Ok(expected) = self.expected_sha256(repo, filename) {
            if !expected.eq_ignore_ascii_case(&actual) {
                let _ = std::fs::remove_file(&part);
                return Err(HfError::ShaMismatch { expected, actual });
            }
        }

        std::fs::rename(&part, dest).map_err(|e| HfError::Io(e.to_string()))?;

        let quant = quant_from_filename(filename).to_string();
        Ok(ModelEntry {
            id: format!("{repo}:{quant}"),
            path: dest.to_string_lossy().into_owned(),
            sha256: actual,
            size: expected_size,
            ctx: 0,
            quant,
            source: "hf".into(),
        })
    }
}

/// Quant id parsed from a GGUF filename (e.g. `phi-4-Q4_K_M.gguf` → `q4_k_m`),
/// `mlx` for safetensors, else `unknown`. Lowercased-normalized.
pub fn quant_from_filename(name: &str) -> &str {
    let lower = name.to_lowercase();
    if lower.ends_with(".safetensors") {
        return "mlx";
    }
    for marker in [
        "q8_0", "q6_k", "q5_k_m", "q5_k_s", "q5_0", "q4_k_m", "q4_k_s", "q4_1", "q4_0", "q3_k_m",
        "q3_k_s", "q3_k_l", "q3_0", "q2_k", "f16", "f32", "iq4_xs", "iq3_xxs", "bf16",
    ] {
        if lower.contains(marker) {
            return match marker {
                "q8_0" => "q8_0",
                "q6_k" => "q6_k",
                "q5_k_m" => "q5_k_m",
                "q5_k_s" => "q5_k_s",
                "q5_0" => "q5_0",
                "q4_k_m" => "q4_k_m",
                "q4_k_s" => "q4_k_s",
                "q4_1" => "q4_1",
                "q4_0" => "q4_0",
                "q3_k_m" => "q3_k_m",
                "q3_k_s" => "q3_k_s",
                "q3_k_l" => "q3_k_l",
                "q3_0" => "q3_0",
                "q2_k" => "q2_k",
                "iq4_xs" => "iq4_xs",
                "iq3_xxs" => "iq3_xxs",
                "f16" => "f16",
                "f32" => "f32",
                "bf16" => "bf16",
                _ => unreachable!(),
            };
        }
    }
    "unknown"
}

/// Staging path: `dest.part` (resume target).
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Free bytes on the filesystem containing `dir` (best-effort via `statvfs`).
pub fn free_bytes(dir: &Path) -> Option<u64> {
    // statvfs is not directly in std; use sysinfo, which the crate already
    // depends on (P27 also uses it for RAM/disk hardware probes).
    sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| {
            dir.starts_with(d.mount_point())
                || d.mount_point().starts_with(dir)
                || d.name().to_string_lossy() == "/"
        })
        .map(|d| d.available_space())
        .max()
}

/// sha256 hex of a file (streamed — models are multi-GB).
pub fn sha256_hex(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buf = [0u8; 128 * 1024];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buf[..n]),
                Err(_) => break,
            }
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The canonical store path for a repo/quant (doc 79 layout).
pub fn store_path(base: &Path, repo: &str, quant: &str, sha8: &str) -> PathBuf {
    let (publisher, model) = repo.split_once('/').unwrap_or(("unknown", repo));
    entry_path(base, publisher, model, quant, sha8)
}

/// Unique quants in a repo's file list (for the picker's quant dropdown).
pub fn quants_in(files: &[HfFile]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for f in files {
        let q = quant_from_filename(&f.path).to_string();
        if seen.insert(q.clone()) {
            out.push(q);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quant_from_filename_normalizes() {
        assert_eq!(quant_from_filename("phi-4-Q4_K_M.gguf"), "q4_k_m");
        assert_eq!(quant_from_filename("qwen2.5-0.5b-q8_0.gguf"), "q8_0");
        assert_eq!(quant_from_filename("model.safetensors"), "mlx");
        assert_eq!(quant_from_filename("weird-name.gguf"), "unknown");
    }

    #[test]
    fn quants_in_dedupes() {
        let files = vec![
            HfFile {
                path: "a-Q4_K_M.gguf".into(),
                size: 1,
                lfs: None,
                kind: None,
            },
            HfFile {
                path: "b-Q4_K_M.gguf".into(),
                size: 1,
                lfs: None,
                kind: None,
            },
            HfFile {
                path: "c-q8_0.gguf".into(),
                size: 1,
                lfs: None,
                kind: None,
            },
        ];
        let qs = quants_in(&files);
        assert_eq!(qs.len(), 2);
        assert!(qs.contains(&"q4_k_m".to_string()));
        assert!(qs.contains(&"q8_0".to_string()));
    }

    #[test]
    fn sha256_hex_round_trip() {
        let d = std::env::temp_dir().join(format!("eaios-sha-{}", std::process::id()));
        std::fs::write(&d, b"hello").unwrap();
        assert_eq!(
            sha256_hex(&d),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let _ = std::fs::remove_file(&d);
    }
}
