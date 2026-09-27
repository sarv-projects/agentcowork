//! P52.1 — static fit estimate: file bytes + KV-cache vs. host memory.
//!
//! The quick, dependency-free answer to "will this model run here?" before
//! the heavier [`crate::hwfit::score_model`] pass. The KV term uses the
//! ~0.5 MB/token rule of thumb for an 8B-class model in F16 (2 × K/V ×
//! layers × d_model × 2 bytes × ctx — a conservative upper bound; smaller
//! models and quantized KV caches use less). Tiers mirror the existing
//! 60%-RAM headroom rule in [`crate::models::hf`] (`recommend_quant`):
//! file + KV within 60% of the budget fits, within 85% may be slow, beyond
//! that will not fit.

use serde::{Deserialize, Serialize};

/// The default quant id the picker offers (GGUF Q4_K_M — the size/quality
/// sweet spot, matching the quant vocabulary in [`crate::models::hf`]).
pub const DEFAULT_QUANT: &str = "Q4_K_M";

/// The fit verdict for one (model, context) pair on one machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitTier {
    /// file + KV within 60% of the memory budget — comfortable headroom.
    #[default]
    Fits,
    /// Within 85% — runs, but expect pressure/swaps on long contexts.
    MayBeSlow,
    /// Beyond 85% — do not attempt (pick a smaller quant or shorter ctx).
    WontFit,
}

/// The split estimate: file bytes vs. KV-cache vs. total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct FitEstimate {
    pub tier: FitTier,
    /// The weight file size (GB).
    pub file_gb: f64,
    /// The KV-cache estimate for `ctx_tokens` (GB).
    pub kv_gb: f64,
    /// `file_gb + kv_gb` (GB) — the term compared against the budget.
    pub total_gb: f64,
}

impl FitEstimate {
    /// `total_gb` as a fraction of the effective budget (see
    /// [`estimate_fit`]); `None` when there is no memory to compare against.
    pub fn ratio_of(&self, ram_gb: f64, vram_gb: f64) -> Option<f64> {
        let budget = effective_budget(ram_gb, vram_gb);
        if budget <= 0.0 {
            None
        } else {
            Some(self.total_gb / budget)
        }
    }
}

/// Estimate whether a `file_gb` model with `ctx_tokens` of context fits in
/// `ram_gb` (+ `vram_gb` when a discrete GPU is present).
///
/// `kv_gb = ctx_tokens × 0.5MB` (the 8B-class F16 rule of thumb above).
/// The budget is `ram + vram` when VRAM is present (weights can offload),
/// else plain RAM. Tiers split at 60% / 85% of the budget.
pub fn estimate_fit(file_gb: f64, ctx_tokens: u64, ram_gb: f64, vram_gb: f64) -> FitEstimate {
    let kv_gb = ctx_tokens as f64 * 0.5 / 1e6;
    let total_gb = file_gb + kv_gb;
    let budget = effective_budget(ram_gb, vram_gb);
    let tier = if budget <= 0.0 {
        FitTier::WontFit
    } else {
        let ratio = total_gb / budget;
        if ratio <= 0.60 {
            FitTier::Fits
        } else if ratio <= 0.85 {
            FitTier::MayBeSlow
        } else {
            FitTier::WontFit
        }
    };
    FitEstimate {
        tier,
        file_gb,
        kv_gb,
        total_gb,
    }
}

fn effective_budget(ram_gb: f64, vram_gb: f64) -> f64 {
    if vram_gb > 0.0 {
        ram_gb + vram_gb
    } else {
        ram_gb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_splits_file_and_kv() {
        // 1M ctx × 0.5MB = 0.5GB of KV on top of a 4GB file.
        let e = estimate_fit(4.0, 1_000_000, 32.0, 0.0);
        assert!((e.file_gb - 4.0).abs() < 1e-9);
        assert!((e.kv_gb - 0.5).abs() < 1e-9);
        assert!((e.total_gb - 4.5).abs() < 1e-9);
        // Zero context means zero KV.
        let e0 = estimate_fit(4.0, 0, 32.0, 0.0);
        assert_eq!(e0.kv_gb, 0.0);
        assert_eq!(e0.total_gb, 4.0);
    }

    #[test]
    fn tiers_at_thresholds() {
        // Budget = 10GB RAM, no VRAM.
        assert_eq!(estimate_fit(6.0, 0, 10.0, 0.0).tier, FitTier::Fits); // 60%
        assert_eq!(estimate_fit(6.01, 0, 10.0, 0.0).tier, FitTier::MayBeSlow);
        assert_eq!(estimate_fit(8.5, 0, 10.0, 0.0).tier, FitTier::MayBeSlow); // 85%
        assert_eq!(estimate_fit(8.51, 0, 10.0, 0.0).tier, FitTier::WontFit);
        // VRAM extends the budget (6GB file + 0 KV vs 8+4=12GB → 50%).
        assert_eq!(estimate_fit(6.0, 0, 8.0, 4.0).tier, FitTier::Fits);
        // No memory at all never fits.
        assert_eq!(estimate_fit(0.0, 0, 0.0, 0.0).tier, FitTier::WontFit);
    }

    #[test]
    fn q4_k_m_is_default() {
        assert_eq!(DEFAULT_QUANT, "Q4_K_M");
        // And it is in the quant vocabulary hf.rs parses from filenames.
        assert_eq!(
            crate::models::hf::quant_from_filename("phi-4-Q4_K_M.gguf"),
            "q4_k_m"
        );
        assert_eq!(
            DEFAULT_QUANT.to_ascii_lowercase(),
            crate::models::hf::quant_from_filename("phi-4-Q4_K_M.gguf")
        );
    }
}
