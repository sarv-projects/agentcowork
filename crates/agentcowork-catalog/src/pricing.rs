//! A9 — cache-aware cost engine (doc 66 §1.3): real per-model pricing
//! (including `input_cache_read` / `input_cache_write`) feeding the cost
//! display + the J11 budget gate. No vendor claims — the numbers come from
//! the catalog's pricing fields.

use crate::model::Pricing;
use serde::{Deserialize, Serialize};

/// The cost of one request under the cache-aware model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Uncached input tokens billed at `prompt`.
    pub uncached_input_cost: f64,
    /// Cache-read tokens billed at `input_cache_read`.
    pub cache_read_cost: f64,
    /// Cache-write tokens billed at `input_cache_write`.
    pub cache_write_cost: f64,
    /// Output tokens billed at `completion`.
    pub output_cost: f64,
}

impl CostBreakdown {
    pub fn total(&self) -> f64 {
        self.uncached_input_cost + self.cache_read_cost + self.cache_write_cost + self.output_cost
    }
}

/// Compute the cost breakdown for a request against a model's pricing.
/// `uncached_input` / `cache_read` / `cache_write` are the input token
/// split (they must sum to the total input tokens).
pub fn cost_for(
    pricing: Pricing,
    uncached_input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> CostBreakdown {
    // rates are per-token USD (e.g. 3e-6 = $3 per 1M tokens): cost = tokens × rate
    let per = |tokens: u64, rate: f64| tokens as f64 * rate;
    CostBreakdown {
        uncached_input_cost: per(uncached_input, pricing.prompt),
        cache_read_cost: per(cache_read, pricing.input_cache_read),
        cache_write_cost: per(cache_write, pricing.input_cache_write),
        output_cost: per(output, pricing.completion),
    }
}

/// The cache-aware estimate: given total input tokens + a cache-hit
/// fraction (0..=1) and the write fraction, split the input.
pub fn split_input(
    total_input: u64,
    cache_read_frac: f64,
    cache_write_frac: f64,
) -> (u64, u64, u64) {
    let read = (total_input as f64 * cache_read_frac.clamp(0.0, 1.0)) as u64;
    let write = (total_input as f64 * cache_write_frac.clamp(0.0, 1.0)) as u64;
    let uncached = total_input.saturating_sub(read).saturating_sub(write);
    (uncached, read, write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_read_is_cheaper_than_fresh_input() {
        let p = Pricing {
            prompt: 3e-6,
            completion: 15e-6,
            input_cache_read: 0.3e-6,
            input_cache_write: 2e-6,
            ..Default::default()
        };
        let fresh = cost_for(p, 1_000_000, 0, 0, 0);
        let cached = cost_for(p, 0, 1_000_000, 0, 0);
        assert!(cached.total() < fresh.total());
        assert!((fresh.total() - 3.0).abs() < 1e-9);
        assert!((cached.total() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn split_input_respects_fractions() {
        let (u, r, w) = split_input(1_000_000, 0.5, 0.2);
        assert_eq!((u, r, w), (300_000, 500_000, 200_000));
    }

    #[test]
    fn total_is_the_sum() {
        let p = Pricing {
            prompt: 1e-6,
            completion: 1e-6,
            input_cache_read: 0.5e-6,
            input_cache_write: 1e-6,
            ..Default::default()
        };
        let b = cost_for(p, 100, 200, 300, 400);
        let manual = 100e-6 * 1.0 + 200e-6 * 0.5 + 300e-6 * 1.0 + 400e-6 * 1.0;
        assert!((b.total() - manual).abs() < 1e-12);
    }
}
