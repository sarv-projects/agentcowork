//! P36 — `RouteDecision` + `ProviderObservation` (A7/H9, spec v3.39).
//!
//! The live loop: `ProviderObservation` history → scorer → `RouteDecision`.
//! These are the named types the scorer vocabulary feeds; honesty per
//! ARCH/03: coordinator `router.ts` remains capability-filter + cost-sort —
//! these types are the Rust-side contract the live loop will emit, not a
//! claim that multi-strategy OmniRoute routing is implemented.

use serde::{Deserialize, Serialize};

/// One observed provider round-trip (from the token ledger / health tracking).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderObservation {
    pub provider: String,
    pub model: String,
    pub ok: bool,
    pub latency_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read_tokens: u64,
    pub cost: f64,
    pub health: f64,
    pub quota_remaining: Option<f64>,
    pub recorded_at_ms: u64,
}

impl ProviderObservation {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            ok: true,
            latency_ms: 0,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            cost: 0.0,
            health: 1.0,
            quota_remaining: None,
            recorded_at_ms: 0,
        }
    }

    /// Cached-input aware billable tokens (AI SDK v6 normalization: cached
    /// tokens are excluded from the billed `in` before cost).
    pub fn billable_tokens(&self) -> u64 {
        self.tokens_in.saturating_sub(self.cache_read_tokens)
    }
}

/// The decision a router makes: which provider+model handles this turn, with
/// the evidence trail (`reasons`) and the fallback chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteDecision {
    pub provider: String,
    pub model: String,
    /// Strategy vocabulary names (ARCH/03 `tier.rs` strategies). Never an
    /// invented public strategy — extra OmniRoute modes stay internal scores.
    pub strategy: String,
    /// 0..=1 composite score.
    pub score: f64,
    pub reasons: Vec<String>,
    /// Projected/observed cost for the turn.
    pub cost: f64,
    pub health: f64,
    pub quota: f64,
    /// Cache affinity hint (same key for cache).
    pub cache_affinity: Option<String>,
    /// Ordered fallback chain (provider, model) pairs.
    pub fallback_chain: Vec<(String, String)>,
}

impl RouteDecision {
    pub fn fallback_after(&self) -> Option<&(String, String)> {
        self.fallback_chain.first()
    }
}

/// The deterministic consensus scorer: blends health, quota, cost-inverse,
/// and latency-inverse with the static ARCH/03 weight vocabulary. This is the
/// *named* scoring function consumers call; it is deliberately naive
/// (capability-filter + cost-sort honesty).
pub struct Scorer;

impl Scorer {
    /// Weights mirroring ARCH/03 `DEFAULT_WEIGHTS` (health 0.20, costInv 0.15,
    /// latencyInv 0.12, quota 0.10 … remainder in cache affinity + recency).
    pub fn score(&self, obs: &ProviderObservation, cost_weight: f64, latency_weight: f64) -> f64 {
        if !obs.ok || obs.health <= 0.0 {
            return 0.0;
        }
        let health = 0.20 * obs.health;
        let quota = obs.quota_remaining.unwrap_or(1.0).clamp(0.0, 1.0) * 0.10;
        let cost_inv = if obs.cost <= 0.0 {
            0.15
        } else {
            0.15 * (1.0 / (1.0 + obs.cost))
        };
        let latency_inv = if obs.latency_ms == 0 {
            latency_weight
        } else {
            latency_weight * (1.0 / (1.0 + obs.latency_ms as f64 / 1000.0))
        };
        let cache_bonus = if obs.cache_read_tokens > 0 { 0.08 } else { 0.0 };
        (health + quota + cost_inv * cost_weight + latency_inv + cache_bonus).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_ledger_shapes() {
        let obs = ProviderObservation::new("openai", "gpt-4.1");
        assert!(obs.ok);
        assert_eq!(obs.billable_tokens(), 0);
    }

    #[test]
    fn cached_tokens_excluded_from_bill() {
        let mut obs = ProviderObservation::new("anthropic", "claude-sonnet-4");
        obs.tokens_in = 10_000;
        obs.cache_read_tokens = 8_000;
        assert_eq!(obs.billable_tokens(), 2_000);
    }

    #[test]
    fn failed_observation_scores_zero() {
        let mut obs = ProviderObservation::new("openai", "gpt-5");
        obs.ok = false;
        let s = Scorer.score(&obs, 0.15, 0.12);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn cost_sensitive_scoring() {
        let cheap = ProviderObservation::new("deepseek", "deepseek-chat");
        let mut pricey = ProviderObservation::new("openai", "gpt-5");
        pricey.cost = 10.0;
        let cheap_s = Scorer.score(&cheap, 0.15, 0.12);
        let pricey_s = Scorer.score(&pricey, 0.15, 0.12);
        assert!(cheap_s > pricey_s, "cheap beats pricey at equal health");
    }

    #[test]
    fn fallback_chain_surface() {
        let d = RouteDecision {
            provider: "openai".into(),
            model: "gpt-5".into(),
            strategy: "capability-filter-cost-sort".into(),
            score: 0.8,
            reasons: vec!["tools".into()],
            cost: 0.01,
            health: 1.0,
            quota: 0.9,
            cache_affinity: Some("k1".into()),
            fallback_chain: vec![("anthropic".into(), "claude-sonnet-4".into())],
        };
        assert_eq!(
            d.fallback_after(),
            Some(&("anthropic".into(), "claude-sonnet-4".into()))
        );
    }
}
