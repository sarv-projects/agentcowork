//! Hardware-fit picker for local models (P1.8 — doc 58 `llmfit` pattern, doc
//! 61 agent-native retirement note). Detects RAM / CPU / GPU, then scores
//! candidate local models on **fit** (does it fit in RAM?), **speed**
//! (accelerator + size), **quality** (context tier), and a combined 0..=1
//! score — the `recommend --json`-style output the UI feeds the model picker.
//!
//! Scoring is deterministic given a [`HardwareProfile`]; [`detect`] is the
//! best-effort probe of the running machine (sysinfo for RAM/CPU, plus a
//! platform GPU check). Q4_K_M ≈ 0.5 bytes/param means the on-disk size is
//! the RAM-dominant term — a model's quantized bytes must fit within free
//! RAM plus a working/KV-cache headroom factor.

use serde::{Deserialize, Serialize};

/// The GPU/accelerator class (affects local inference speed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuClass {
    /// No accelerator detected — CPU-only inference.
    CpuOnly,
    /// Integrated graphics (shared memory, limited throughput).
    Integrated,
    /// NVIDIA CUDA (nvidia-smi / device present).
    NvidiaCuda,
    /// AMD (ROCm/dri device present).
    Amd,
    /// Apple Silicon (Metal).
    AppleSilicon,
    /// A device is present but its class could not be determined.
    Unknown,
}

impl GpuClass {
    /// Throughput multiplier for the speed score (CPU-only = slowest).
    fn speed_factor(self) -> f64 {
        match self {
            GpuClass::CpuOnly => 0.4,
            GpuClass::Integrated => 0.7,
            GpuClass::NvidiaCuda => 1.0,
            GpuClass::Amd => 0.9,
            GpuClass::AppleSilicon => 1.0,
            GpuClass::Unknown => 0.6,
        }
    }
}

/// The machine the model must fit on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub ram_bytes: u64,
    pub cpu_cores: usize,
    pub gpu: GpuClass,
}

/// One local model candidate to score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalModelCandidate {
    pub name: String,
    /// On-disk (quantized) size in bytes.
    pub size_bytes: u64,
    /// Effective context window (tokens).
    pub context_window: u32,
}

/// The fit verdict for one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelFit {
    pub name: String,
    /// Can the model run with comfortable headroom? (fits in RAM).
    pub fits: bool,
    /// 0..=1 — RAM headroom (1 = tons of room).
    pub fit: f64,
    /// 0..=1 — accelerator + size based throughput.
    pub speed: f64,
    /// 0..=1 — context tier (the 15K floor matters for agent loops).
    pub quality: f64,
    /// The effective context window (for the ≤15–20K warning).
    pub context_window: u32,
    /// Combined 0..=1 recommendation score.
    pub score: f64,
}

/// RAM headroom factor: the model's bytes must leave this much of RAM free
/// (KV cache + working set + OS).
const HEADROOM_FACTOR: f64 = 1.25;
/// Context floor below which an agent loops (doc 33 §7.4).
const CONTEXT_FLOOR: u32 = 15_000;
/// Context ceiling past which quality saturates.
const CONTEXT_CEILING: u32 = 120_000;

/// Score a model against the machine.
pub fn score_model(candidate: &LocalModelCandidate, hw: &HardwareProfile) -> ModelFit {
    let need = (candidate.size_bytes as f64 * HEADROOM_FACTOR) as u64;
    let fits = hw.ram_bytes >= need;

    let fit = if hw.ram_bytes == 0 {
        0.0
    } else {
        (hw.ram_bytes as f64 / need.max(1) as f64).min(1.0)
    };

    // Speed: accelerator factor scaled down by model size (bigger = slower).
    let size_gb = candidate.size_bytes as f64 / 1e9;
    let size_penalty = 1.0 / (1.0 + size_gb / 8.0);
    let speed = (hw.gpu.speed_factor() * size_penalty).clamp(0.0, 1.0);

    // Quality: context tier from the floor to the ceiling.
    let ctx = candidate.context_window;
    let quality = if ctx < CONTEXT_FLOOR {
        0.2 + 0.3 * (ctx as f64 / CONTEXT_FLOOR as f64)
    } else {
        ((ctx as f64 / CONTEXT_CEILING as f64).min(1.0)).clamp(0.5, 1.0)
    };

    // Combined: a non-fitting model is disqualified (score collapses).
    let score = if fits {
        (0.5 * quality + 0.3 * speed + 0.2 * fit).clamp(0.0, 1.0)
    } else {
        fit * 0.25
    };

    ModelFit {
        name: candidate.name.to_string(),
        fits,
        fit,
        speed,
        quality,
        context_window: ctx,
        score,
    }
}

/// Rank candidate models best-first (fits first, then combined score).
pub fn recommend(candidates: &[LocalModelCandidate], hw: &HardwareProfile) -> Vec<ModelFit> {
    let mut out: Vec<ModelFit> = candidates.iter().map(|c| score_model(c, hw)).collect();
    out.sort_by(|a, b| {
        b.fits
            .cmp(&a.fits)
            .then_with(|| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Pick the KV-cache element type for a local run (P39.4 — llama.cpp
/// `-ctk`/`-ctv`). Memory-constrained machines (little RAM relative to the
/// model, or <16 GiB total) quantize the cache (Q8_0 ≈ 8-bit KV, ~4× memory
/// savings vs F16 with a bounded quality impact); roomy machines keep F16
/// (the llama.cpp default — the best quality/memory balance). F32 is never
/// auto-picked: it burns 2× F16's memory for a negligible quality delta, so
/// it stays available only as the user's explicit knob. The picker result
/// rides the `local://` launch spec so the memory-constrained profile never
/// guesses.
pub fn pick_kv_cache_type(hw: &HardwareProfile, model_bytes: u64) -> crate::models::KvCacheType {
    use crate::models::KvCacheType;
    let headroom = if model_bytes == 0 {
        0.0
    } else {
        hw.ram_bytes as f64 / model_bytes as f64
    };
    if headroom < 3.0 || hw.ram_bytes < 16 * 1024 * 1024 * 1024 {
        KvCacheType::Q8_0
    } else {
        KvCacheType::F16
    }
}

/// Probe the running machine (best-effort; never fails — degrades to an empty
/// profile). Uses `sysinfo` for RAM + core count and a platform GPU check.
pub fn detect() -> HardwareProfile {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu();
    let ram_bytes = sys.total_memory();
    let cpu_cores = sys.cpus().len().max(1);
    HardwareProfile {
        ram_bytes,
        cpu_cores,
        gpu: detect_gpu(),
    }
}

/// Best-effort GPU class detection (no subprocess, no panics).
fn detect_gpu() -> GpuClass {
    // Apple Silicon = Metal (arm64 macOS).
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        return GpuClass::AppleSilicon;
    }

    // NVIDIA: CUDA devices via env, or the device node.
    if std::env::var("CUDA_VISIBLE_DEVICES").is_ok_and(|v| !v.is_empty() && v != "-1")
        || std::env::var("NVIDIA_VISIBLE_DEVICES").is_ok_and(|v| !v.is_empty())
        || path_exists("/dev/nvidiactl")
        || path_exists("/proc/driver/nvidia/version")
    {
        return GpuClass::NvidiaCuda;
    }

    // AMD: ROCm/kfd device node or DRI render nodes.
    if path_exists("/dev/kfd") || glob_exists("/dev/dri/renderD*") {
        return GpuClass::Amd;
    }

    GpuClass::CpuOnly
}

fn path_exists(p: &str) -> bool {
    std::path::Path::new(p).exists()
}

fn glob_exists(pattern: &str) -> bool {
    let dir = std::path::Path::new("/dev/dri");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let prefix = pattern.trim_end_matches('*');
    entries
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_machine() -> HardwareProfile {
        HardwareProfile {
            ram_bytes: 32 * 1024 * 1024 * 1024, // 32 GiB
            cpu_cores: 8,
            gpu: GpuClass::AppleSilicon,
        }
    }

    fn small_machine() -> HardwareProfile {
        HardwareProfile {
            ram_bytes: 8 * 1024 * 1024 * 1024, // 8 GiB
            cpu_cores: 4,
            gpu: GpuClass::CpuOnly,
        }
    }

    fn models() -> Vec<LocalModelCandidate> {
        vec![
            LocalModelCandidate {
                name: "qwen2.5:0.5b".into(),
                size_bytes: 397 * 1024 * 1024,
                context_window: 16_384,
            },
            LocalModelCandidate {
                name: "muse-glimmer-30b".into(),
                size_bytes: 18_000 * 1024 * 1024, // ~18 GiB quantized 30B
                context_window: 120_000,
            },
            LocalModelCandidate {
                name: "tiny-ctx".into(),
                size_bytes: 200 * 1024 * 1024,
                context_window: 4_096,
            },
        ]
    }

    #[test]
    fn small_model_fits_big_and_small_machines() {
        let m = models();
        let fit_big = score_model(&m[0], &big_machine());
        assert!(fit_big.fits);
        let fit_small = score_model(&m[0], &small_machine());
        assert!(fit_small.fits);
    }

    #[test]
    fn large_model_rejected_on_small_machine() {
        let m = models();
        let fit = score_model(&m[1], &small_machine());
        assert!(!fit.fits);
        assert!(fit.score < 0.3, "non-fitting score {}", fit.score);
    }

    #[test]
    fn large_model_fits_big_machine() {
        let m = models();
        let fit = score_model(&m[1], &big_machine());
        assert!(fit.fits);
    }

    #[test]
    fn gpu_class_affects_speed() {
        let m = &models()[0];
        let gpu = score_model(m, &big_machine());
        let cpu = score_model(
            m,
            &HardwareProfile {
                gpu: GpuClass::CpuOnly,
                ..big_machine()
            },
        );
        assert!(
            gpu.speed > cpu.speed,
            "gpu {} vs cpu {}",
            gpu.speed,
            cpu.speed
        );
    }

    #[test]
    fn low_context_models_score_poor_quality() {
        let m = models();
        let tiny = score_model(&m[2], &big_machine());
        let normal = score_model(&m[0], &big_machine());
        assert!(tiny.quality < normal.quality);
        assert!(tiny.quality < 0.5);
    }

    #[test]
    fn recommend_orders_fitting_models_first() {
        let ranked = recommend(&models(), &small_machine());
        // The non-fitting 30B model sinks to the bottom.
        assert_eq!(ranked.last().unwrap().name, "muse-glimmer-30b");
        assert!(ranked[0].fits);
        // Best fitting+quality first (the 0.5b has more headroom than tiny-ctx).
        assert_eq!(ranked[0].name, "qwen2.5:0.5b");
    }

    #[test]
    fn detect_never_panics_and_reports_something() {
        let hw = detect();
        assert!(hw.ram_bytes > 0);
        assert!(hw.cpu_cores >= 1);
    }

    #[test]
    fn kv_cache_picker_quantizes_for_memory_constrained() {
        use crate::models::KvCacheType;
        // 8 GiB machine + an 18 GiB model → the KV cache must be quantized.
        assert_eq!(
            pick_kv_cache_type(&small_machine(), 18_000 * 1024 * 1024),
            KvCacheType::Q8_0
        );
        // Roomier 32 GiB machine with a small model → F16 (quality default).
        assert_eq!(
            pick_kv_cache_type(&big_machine(), 397 * 1024 * 1024),
            KvCacheType::F16
        );
        // F32 is never auto-picked (explicit user knob only).
        assert_eq!(
            pick_kv_cache_type(&big_machine(), 100 * 1024 * 1024),
            KvCacheType::F16
        );
        // Unknown model size never panics and falls to the safe Q8_0.
        assert_eq!(pick_kv_cache_type(&small_machine(), 0), KvCacheType::Q8_0);
    }

    #[test]
    fn gpu_speed_factor_ordering() {
        assert!(GpuClass::NvidiaCuda.speed_factor() > GpuClass::CpuOnly.speed_factor());
        assert!(GpuClass::AppleSilicon.speed_factor() > GpuClass::Integrated.speed_factor());
    }

    #[test]
    fn model_fit_serializes() {
        let m = models();
        let fit = score_model(&m[0], &big_machine());
        let json = serde_json::to_string(&fit).unwrap();
        assert!(json.contains("\"fits\":true"));
        let back: ModelFit = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "qwen2.5:0.5b");
    }
}
