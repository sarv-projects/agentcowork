//! P39.2 — semantic context-distillation tier (spec §9.3 §2; LLMLingua-2
//! pattern, pattern-only — microsoft/LLMLingua is MIT, we build a native
//! extractive seam).
//!
//! Token-level pruning of the context before it is sent to the model, as an
//! **optional** third stage behind the landed ratio-based compaction (P5.7).
//! Gating (R4 — measure before optimizing): [`DistillConfig::enabled`] is
//! `false` by default; the profiling evidence that flips it is the published
//! P10.3 RSS/token numbers (`p10_bench.rs`). The tier never serves into
//! mutation paths, and per-effect honesty is preserved: a pruned span is
//! never silently dropped — the output carries a sha256 digest of everything
//! pruned plus a `has_gap` flag, so receipts can still reconstruct what the
//! model did and did not see.

use serde::{Deserialize, Serialize};

/// Approximate token budget: 4 chars per token (documented approximation —
/// the distillation decision is a heuristic, not a tokenizer).
pub const CHARS_PER_TOKEN: usize = 4;

/// The distillation gate. `enabled: false` (the default) = pass-through with
/// zero loss; profiling evidence (P10.3 RSS/token numbers) is what flips it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DistillConfig {
    pub enabled: bool,
    /// Target context fraction to keep (0.0–1.0). 1.0 = keep everything.
    /// LLMLingua-2 reports up to 20× compression (0.05); the safe default
    /// when enabled is 0.5.
    pub ratio: f64,
}

impl Default for DistillConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ratio: 0.5,
        }
    }
}

impl DistillConfig {
    pub fn enabled(ratio: f64) -> Self {
        Self {
            enabled: true,
            ratio: ratio.clamp(0.01, 1.0),
        }
    }

    /// The gate itself: `should_distill` is false unless explicitly enabled.
    pub fn should_distill(&self) -> bool {
        self.enabled && self.ratio < 1.0
    }
}

/// One context block (a retrieved chunk, tool result, or conversation span).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBlock {
    pub id: String,
    pub text: String,
}

/// The distilled form of one block. If the block was pruned, the pruned tail
/// is represented by its sha256 digest — never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistilledBlock {
    pub id: String,
    /// The kept text (the salient prefix — headers/claims/citations live in
    /// the front of a well-formed block).
    pub text: String,
    /// True if any content was pruned from this block.
    pub has_gap: bool,
    /// sha256 of everything pruned from this block (empty when no gap).
    pub pruned_digest: String,
    /// Characters pruned from this block (for the honesty surface).
    pub pruned_chars: usize,
}

/// The distillation result. `has_gap` is sticky at the container level too —
/// any caller that routes this into a prompt must surface the gap to the
/// model (honesty machinery, never a silent truncation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistilledContext {
    pub blocks: Vec<DistilledBlock>,
    pub has_gap: bool,
    pub total_pruned_chars: usize,
    /// Input size in approximate tokens (for the P39.2 profiling gate: what
    /// would have been sent vs what was).
    pub input_tokens_est: usize,
    pub output_tokens_est: usize,
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

fn approx_tokens(chars: usize) -> usize {
    chars.div_ceil(CHARS_PER_TOKEN)
}

/// Distill a context. Disabled config (the default) returns a pass-through
/// with no gap. When enabled, each block over its proportional budget keeps
/// its leading span and digests the pruned tail.
pub fn distill_context(blocks: &[ContextBlock], cfg: &DistillConfig) -> DistilledContext {
    if !cfg.should_distill() {
        return DistilledContext {
            blocks: blocks
                .iter()
                .map(|b| DistilledBlock {
                    id: b.id.clone(),
                    text: b.text.clone(),
                    has_gap: false,
                    pruned_digest: String::new(),
                    pruned_chars: 0,
                })
                .collect(),
            has_gap: false,
            total_pruned_chars: 0,
            input_tokens_est: approx_tokens(blocks.iter().map(|b| b.text.len()).sum()),
            output_tokens_est: approx_tokens(blocks.iter().map(|b| b.text.len()).sum()),
        };
    }

    let total_chars: usize = blocks.iter().map(|b| b.text.len()).sum();
    let budget_chars = (total_chars as f64 * cfg.ratio) as usize;
    let mut pruned_total = 0usize;
    let mut any_gap = false;

    let out: Vec<DistilledBlock> = blocks
        .iter()
        .map(|b| {
            // Retain-not-narrate: a block that fits under the budget is kept
            // whole (small blocks are never squeezed); only a block that
            // alone exceeds the budget is pruned to it — the salient prefix
            // is kept, the tail is digested (never silently dropped). The
            // ratio is a target, not a hard cap.
            if b.text.len() <= budget_chars {
                return DistilledBlock {
                    id: b.id.clone(),
                    text: b.text.clone(),
                    has_gap: false,
                    pruned_digest: String::new(),
                    pruned_chars: 0,
                };
            }
            let kept = &b.text[..budget_chars];
            let pruned = &b.text[budget_chars..];
            pruned_total += pruned.len();
            any_gap = true;
            DistilledBlock {
                id: b.id.clone(),
                text: kept.to_string(),
                has_gap: true,
                pruned_digest: sha256_hex(pruned),
                pruned_chars: pruned.len(),
            }
        })
        .collect();

    DistilledContext {
        input_tokens_est: approx_tokens(total_chars),
        output_tokens_est: approx_tokens(total_chars - pruned_total),
        blocks: out,
        has_gap: any_gap,
        total_pruned_chars: pruned_total,
    }
}

// The mutation-path contract: distilled context is read-only by
// construction — `distill_context` accepts `&[ContextBlock]` and returns
// owned `DistilledContext`; there is no API to write a distilled block back
// into a store, so the tier can never serve into mutation paths.

/// The ordered reduction tiers (`ARCH/16-CONTEXT.md` §4). Pruning strictly
/// precedes compaction, and the token-level distillation tier is the last
/// optional stage behind them — it can never be the first thing that runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReductionTier {
    /// Whole-item drop with the full output persisted (the only legal first
    /// move under budget pressure).
    Prune,
    /// The structured-checkpoint projection over the durable log.
    Compact,
    /// The optional token-level extraction pass. Off by default.
    Distill,
}

/// The stage order, as a function of the gates. A caller that asks "what runs?"
/// gets the order rather than a decision it has to re-derive.
pub fn tier_order(prune_enabled: bool, distill_enabled: bool) -> Vec<ReductionTier> {
    let mut tiers = Vec::with_capacity(3);
    if prune_enabled {
        tiers.push(ReductionTier::Prune);
    }
    tiers.push(ReductionTier::Compact);
    if distill_enabled {
        tiers.push(ReductionTier::Distill);
    }
    tiers
}

/// Whether the pipeline may run the distillation stage at all. The gate is the
/// same opt-in as [`DistillConfig::should_distill`], so there is exactly one
/// place that decides.
pub fn distill_allowed(cfg: &DistillConfig) -> bool {
    cfg.should_distill()
}

#[cfg(test)]
mod tier_tests {
    use super::*;

    #[test]
    fn pruning_strictly_precedes_compaction_in_the_ordered_tiers() {
        let order = tier_order(true, true);
        assert_eq!(
            order,
            vec![
                ReductionTier::Prune,
                ReductionTier::Compact,
                ReductionTier::Distill
            ]
        );
        let prune_at = order
            .iter()
            .position(|t| *t == ReductionTier::Prune)
            .unwrap();
        let compact_at = order
            .iter()
            .position(|t| *t == ReductionTier::Compact)
            .unwrap();
        assert!(
            prune_at < compact_at,
            "compaction before pruning is a defect"
        );
    }

    #[test]
    fn pruning_may_be_switched_off_and_compaction_still_runs() {
        let order = tier_order(false, true);
        assert!(!order.contains(&ReductionTier::Prune));
        assert_eq!(order, vec![ReductionTier::Compact, ReductionTier::Distill]);
    }

    #[test]
    fn the_distillation_tier_is_opt_in_and_never_runs_first() {
        assert_eq!(
            tier_order(true, false),
            vec![ReductionTier::Prune, ReductionTier::Compact]
        );
        let order = tier_order(true, true);
        assert_ne!(order[0], ReductionTier::Distill);
        // The gate is the single decision point.
        assert!(!distill_allowed(&DistillConfig::default()));
        assert!(distill_allowed(&DistillConfig::enabled(0.5)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, text: &str) -> ContextBlock {
        ContextBlock {
            id: id.into(),
            text: text.into(),
        }
    }

    #[test]
    fn disabled_config_is_a_zero_loss_pass_through() {
        let cfg = DistillConfig::default();
        assert!(!cfg.should_distill());
        let blocks = vec![block("a", "hello world ".repeat(100).as_str())];
        let out = distill_context(&blocks, &cfg);
        assert!(!out.has_gap);
        assert_eq!(out.total_pruned_chars, 0);
        assert_eq!(out.blocks[0].text, blocks[0].text);
        assert!(out.blocks[0].pruned_digest.is_empty());
    }

    #[test]
    fn ratio_one_is_a_pass_through_even_when_enabled() {
        let cfg = DistillConfig::enabled(1.0);
        assert!(!cfg.should_distill());
        let out = distill_context(&[block("a", "x".repeat(500).as_str())], &cfg);
        assert!(!out.has_gap);
    }

    #[test]
    fn enabled_prunes_long_blocks_and_flags_the_gap() {
        let cfg = DistillConfig::enabled(0.5);
        let long = "A".repeat(1_000);
        let short = "B".repeat(10);
        let out = distill_context(&[block("long", &long), block("short", &short)], &cfg);
        assert!(out.has_gap);
        let long_block = out.blocks.iter().find(|b| b.id == "long").unwrap();
        assert!(long_block.has_gap);
        assert!(long_block.pruned_chars > 0);
        assert!(long_block.text.len() < long.len());
        // the short block under budget is untouched
        let short_block = out.blocks.iter().find(|b| b.id == "short").unwrap();
        assert!(!short_block.has_gap);
        assert_eq!(short_block.text, short);
    }

    #[test]
    fn pruned_content_is_digested_not_dropped() {
        use sha2::{Digest, Sha256};
        let cfg = DistillConfig::enabled(0.3);
        let text = "0123456789".repeat(50); // 500 chars
        let out = distill_context(&[block("k", &text)], &cfg);
        let b = &out.blocks[0];
        assert!(b.has_gap);
        assert!(!b.pruned_digest.is_empty());
        // reconstruct: kept + pruned must equal the input, and the digest
        // must match the pruned tail exactly (receipt-reconstructible).
        let pruned = &text[b.text.len()..];
        let mut h = Sha256::new();
        h.update(pruned.as_bytes());
        assert_eq!(b.pruned_digest, format!("{:x}", h.finalize()));
        assert_eq!(b.text.len() + b.pruned_chars, text.len());
        assert_eq!(out.total_pruned_chars, b.pruned_chars);
    }

    #[test]
    fn token_estimates_are_reported_for_the_profiling_gate() {
        let cfg = DistillConfig::enabled(0.5);
        let text = "x".repeat(400); // ~100 tokens
        let out = distill_context(&[block("a", &text)], &cfg);
        assert_eq!(out.input_tokens_est, 100);
        assert!(out.output_tokens_est < out.input_tokens_est);
        assert!(out.output_tokens_est > 0);
    }

    #[test]
    fn empty_input_is_an_empty_zero_gap_result() {
        let cfg = DistillConfig::enabled(0.5);
        let out = distill_context(&[], &cfg);
        assert!(out.blocks.is_empty());
        assert!(!out.has_gap);
        assert_eq!(out.total_pruned_chars, 0);
    }
}
