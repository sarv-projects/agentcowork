//! P52.5 — best-variant picker: choose the weight build for this hardware.
//!
//! A model ships several builds (NPU/GPU/CPU-targeted, various quants).
//! [`best_variant`] prefers the build matching the host accelerator class,
//! then the [`crate::models::fit::DEFAULT_QUANT`] (Q4_K_M) build — the same
//! size/quality default the fit estimator assumes. Pure + total: an empty
//! catalog yields `None`, and a host with no matching build falls back to
//! the CPU build (which runs anywhere) rather than failing.

use serde::{Deserialize, Serialize};

/// The host accelerator class (coarse — the runner decides the exact
/// backend; this only selects the weight build).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HwClass {
    /// Neural accelerator (Apple Neural Engine, Intel NPU, …).
    Npu,
    /// Discrete/integrated GPU build.
    Gpu,
    /// Portable CPU build — the universal fallback.
    #[default]
    Cpu,
}

/// One downloadable weight build of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantCandidate {
    /// Source repo (e.g. a Hugging Face `org/name`).
    pub repo: String,
    /// The weight filename within the repo.
    pub file: String,
    /// Which accelerator this build targets.
    pub hw: HwClass,
    /// Quant id (e.g. `Q4_K_M`) — compared case-insensitively.
    pub quant: String,
}

impl VariantCandidate {
    pub fn new(repo: String, file: String, hw: HwClass, quant: String) -> Self {
        Self {
            repo,
            file,
            hw,
            quant,
        }
    }
}

/// Pick the best build for `hw` out of `catalog`.
///
/// 1. Among builds targeting `hw`, prefer the Q4_K_M build, else the first.
/// 2. Otherwise fall back to the CPU builds (same quant preference) — a CPU
///    build runs anywhere, so "no NPU build" is not an error.
/// 3. Otherwise the Q4_K_M build for any hw, else the first build.
/// 4. Empty catalog → `None`.
pub fn best_variant<'a>(
    hw: &HwClass,
    catalog: &'a [VariantCandidate],
) -> Option<&'a VariantCandidate> {
    if catalog.is_empty() {
        return None;
    }
    if let Some(v) = pick(catalog.iter().filter(|c| &c.hw == hw)) {
        return Some(v);
    }
    // Fall back to the portable CPU build when the accelerator has none.
    if !matches!(hw, HwClass::Cpu) {
        if let Some(v) = pick(catalog.iter().filter(|c| c.hw == HwClass::Cpu)) {
            return Some(v);
        }
    }
    pick(catalog.iter())
}

/// Prefer the Q4_K_M build, else the first candidate. The filter preserves
/// catalog order, so ties are deterministic.
fn pick<'a>(mut it: impl Iterator<Item = &'a VariantCandidate>) -> Option<&'a VariantCandidate> {
    let mut first: Option<&'a VariantCandidate> = None;
    for c in it.by_ref() {
        if first.is_none() {
            first = Some(c);
        }
        if c.quant.eq_ignore_ascii_case("Q4_K_M") {
            return Some(c);
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<VariantCandidate> {
        vec![
            VariantCandidate::new(
                "org/model".into(),
                "model-Q8_0.gguf".into(),
                HwClass::Cpu,
                "Q8_0".into(),
            ),
            VariantCandidate::new(
                "org/model".into(),
                "model-Q4_K_M.gguf".into(),
                HwClass::Cpu,
                "Q4_K_M".into(),
            ),
            VariantCandidate::new(
                "org/model".into(),
                "model-npu-Q4_K_M.gguf".into(),
                HwClass::Npu,
                "Q4_K_M".into(),
            ),
        ]
    }

    #[test]
    fn best_variant_prefers_npu_variant() {
        let cat = catalog();
        let v = best_variant(&HwClass::Npu, &cat).unwrap();
        assert_eq!(v.hw, HwClass::Npu);
        assert_eq!(v.file, "model-npu-Q4_K_M.gguf");
    }

    #[test]
    fn falls_back_to_cpu() {
        // No NPU build at all → the portable CPU build (preferring Q4_K_M).
        let cat = vec![
            VariantCandidate::new(
                "org/m".into(),
                "m-q8.gguf".into(),
                HwClass::Cpu,
                "Q8_0".into(),
            ),
            VariantCandidate::new(
                "org/m".into(),
                "m-q4.gguf".into(),
                HwClass::Cpu,
                "Q4_K_M".into(),
            ),
        ];
        let v = best_variant(&HwClass::Npu, &cat).unwrap();
        assert_eq!(v.hw, HwClass::Cpu);
        assert_eq!(v.quant, "Q4_K_M");
        // Empty catalog is None (not a panic).
        assert!(best_variant(&HwClass::Gpu, &[]).is_none());
    }
}
