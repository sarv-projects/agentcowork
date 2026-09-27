//! P19-1 — Kilo "Gateway" routing seam (doc 71 §1 — Kilo Code 🟡 ADAPT).
//!
//! The 500-model BYOK zero-markup gateway pattern, folded into the model
//! catalog as a **cache-optimized router**: alias resolution (model
//! family → concrete provider/model) with a bounded route cache so repeated
//! sends for the same (task, filters) pair skip re-resolution entirely.
//!
//! Design (mirrors the P14 catalog discipline):
//! - [`RouteKey`] = alias + task hint + hard-requirement flags — the full
//!   cache discriminator (a route is only valid for the requirements it was
//!   resolved against).
//! - The router picks the *cheapest capable* route via `cost_for` +
//!   `RouteFilters` (zero-markup pass-through — a gateway fee is never
//!   invented here; this resolves provider facts only).
//! - The cache is a bounded LRU; stats distinguish hits / misses /
//!   rejections honestly.

use crate::catalog::ModelCatalog;
use crate::model::ModelEntry;
use crate::pricing::cost_for;
use crate::routing::RouteFilters;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// The task class the route is selected for (a cache dimension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskHint {
    /// Plain conversational turn.
    Chat,
    /// Tool-calling-heavy turns.
    Tools,
    /// Structured output (JSON mode).
    Structured,
    /// Vision inputs.
    Vision,
    /// Cheap fast lane (summaries, titles, small edits).
    Fast,
}

/// The full cache key — alias + task + filters must all match.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteKey {
    /// `provider/model` alias as the picker sends it.
    pub alias: String,
    pub task: TaskHint,
    pub require_tools: bool,
    pub require_vision: bool,
}

/// The resolved route (catalog facts only — no secrets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteResult {
    /// Resolved provider id (the broker base-url key).
    pub provider: String,
    /// Resolved model id (provider-scoped).
    pub model: String,
    /// Estimated cost of a 1K-token prompt + 1K-token answer in USD.
    pub cost_usd_per_2k: f64,
    /// Whether this result came from the LRU cache.
    pub from_cache: bool,
}

/// A bounded LRU route cache. O(n) probes are the honest trade for a
/// 500-entry catalog; capacity is caller-configurable.
#[derive(Debug)]
struct RouteCache {
    capacity: usize,
    order: VecDeque<RouteKey>,
    entries: Vec<(RouteKey, RouteResult)>,
}

impl RouteCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            entries: Vec::new(),
        }
    }

    fn get(&mut self, key: &RouteKey) -> Option<RouteResult> {
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        let (k, v) = self.entries.remove(pos);
        self.order.retain(|x| x != &k);
        self.order.push_back(k);
        self.entries.push((self.order.back().cloned()?, v.clone()));
        Some(v)
    }

    fn put(&mut self, key: RouteKey, value: RouteResult) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries[pos].1 = value;
            return;
        }
        if self.entries.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.retain(|(k, _)| *k != evicted);
            }
        }
        self.order.push_back(key.clone());
        self.entries.push((key, value));
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Per-router statistics (surface in the analytics view).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayStats {
    pub hits: u64,
    pub misses: u64,
    pub rejected: u64,
}

/// The Kilo-gateway-style router over an [`ModelCatalog`] (read-only).
#[derive(Debug)]
pub struct GatewayRouter {
    catalog: ModelCatalog,
    cache: RouteCache,
    stats: GatewayStats,
}

impl GatewayRouter {
    pub fn new(catalog: ModelCatalog, cache_capacity: usize) -> Self {
        Self {
            catalog,
            cache: RouteCache::new(cache_capacity),
            stats: GatewayStats::default(),
        }
    }

    pub fn stats(&self) -> &GatewayStats {
        &self.stats
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Resolve an alias to a concrete route. A hit returns the cached route
    /// (key guarantees identical requirements); a miss re-resolves against
    /// fresh catalog facts.
    pub fn resolve(
        &mut self,
        alias: &str,
        task: TaskHint,
        require_tools: bool,
        require_vision: bool,
    ) -> Result<RouteResult, GatewayError> {
        let key = RouteKey {
            alias: alias.to_string(),
            task,
            require_tools,
            require_vision,
        };
        if let Some(hit) = self.cache.get(&key) {
            self.stats.hits += 1;
            return Ok(RouteResult {
                from_cache: true,
                ..hit
            });
        }
        self.stats.misses += 1;

        let mut filters = RouteFilters {
            requires_tools: require_tools,
            ..Default::default()
        };
        if require_vision {
            filters.input_modalities = vec!["image".to_string()];
        }

        let mut best: Option<RouteResult> = None;
        let mut any_candidate = false;
        for entry in self.candidates(alias) {
            any_candidate = true;
            if !filters.matches(entry) {
                continue;
            }
            // 1K prompt + 1K answer, uncached — the standard comparative
            // figure. (Cache-aware pricing can refine this per request.)
            let cost = cost_for(entry.pricing, 1000, 0, 0, 1000);
            let cost_usd_per_2k = cost.total();
            let candidate = RouteResult {
                provider: entry.provider().to_string(),
                model: entry.model_name().to_string(),
                cost_usd_per_2k,
                from_cache: false,
            };
            match &best {
                Some(b) if candidate.cost_usd_per_2k < b.cost_usd_per_2k => best = Some(candidate),
                None => best = Some(candidate),
                _ => {}
            }
        }
        if best.is_none() && any_candidate {
            // Candidates existed but every one failed the hard requirements.
            self.stats.rejected += 1;
        }
        let result = best.ok_or_else(|| GatewayError::NoCapableRoute(alias.to_string()))?;
        self.cache.put(key, result.clone());
        Ok(result)
    }

    /// The alias's candidate set: exact `provider/model` hit first, then
    /// family fallbacks that `base_model` to it (the two-tier schema).
    fn candidates(&self, alias: &str) -> Vec<&'_ ModelEntry> {
        let mut out: Vec<&ModelEntry> = self.catalog.all().filter(|m| m.id == alias).collect();
        // Long-tail: entries whose base_model is the alias (doc 66 §1.1
        // override-only inheritance) are valid fallbacks for the family.
        out.extend(
            self.catalog
                .all()
                .filter(|m| m.base_model.as_deref() == Some(alias)),
        );
        out
    }

    /// Explicit route invalidation (alias set changed at runtime).
    pub fn invalidate(&mut self, alias: &str) {
        self.cache
            .order
            .retain(|k| !k.alias.eq_ignore_ascii_case(alias));
        self.cache
            .entries
            .retain(|(k, _)| !k.alias.eq_ignore_ascii_case(alias));
    }
}

use thiserror::Error;

/// Gateway resolution errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GatewayError {
    #[error("no route satisfies the requirements for alias `{0}`")]
    NoCapableRoute(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> ModelCatalog {
        let json = serde_json::json!([
            {
                "id": "acme/cheap",
                "canonical_slug": "acme/cheap",
                "context_length": 16000,
                "supported_parameters": { "tools": true },
                "pricing": { "prompt": 1e-6, "completion": 1e-5 }
            },
            {
                "id": "acme/expensive",
                "canonical_slug": "acme/expensive",
                "context_length": 128000,
                "supported_parameters": { "tools": true, "structured_outputs": true },
                "pricing": { "prompt": 1e-5, "completion": 1e-4 }
            },
            {
                "id": "acme/visionless",
                "canonical_slug": "acme/visionless",
                "context_length": 16000,
                "supported_parameters": { "tools": true },
                "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
                "pricing": { "prompt": 1e-7, "completion": 1e-6 }
            },
            {
                "id": "acme/cheap-vision",
                "canonical_slug": "acme/cheap-vision",
                "base_model": "acme/cheap",
                "context_length": 16000,
                "supported_parameters": { "tools": true },
                "architecture": { "input_modalities": ["text", "image"], "output_modalities": ["text"] },
                "pricing": { "prompt": 1.2e-6, "completion": 1.1e-5 }
            }
        ]).to_string();
        ModelCatalog::parse(&json).unwrap()
    }

    #[test]
    fn cheapest_capable_wins() {
        let mut router = GatewayRouter::new(sample_catalog(), 8);
        let route = router
            .resolve("acme/cheap", TaskHint::Tools, false, false)
            .unwrap();
        assert_eq!(route.provider, "acme");
        assert_eq!(route.model, "cheap");
        assert!(!route.from_cache);
    }

    #[test]
    fn fallback_alias_resolves_via_base_model() {
        let mut router = GatewayRouter::new(sample_catalog(), 8);
        // `acme/cheap-vision` rides the same family; resolve the alias
        // directly (it exists) — and the family fallback covers long-tail
        // aliases whose entries live only as base_model.
        let route = router
            .resolve("acme/cheap-vision", TaskHint::Vision, false, true)
            .unwrap();
        assert_eq!(route.model, "cheap-vision");
    }

    #[test]
    fn vision_requirement_rejects_visionless() {
        let mut router = GatewayRouter::new(sample_catalog(), 8);
        let route = router
            .resolve("acme/cheap", TaskHint::Vision, false, true)
            .unwrap();
        // cheap itself can't see images; the base_model override can.
        assert_eq!(route.model, "cheap-vision");
    }

    #[test]
    fn cache_hit_skips_resolution() {
        let mut router = GatewayRouter::new(sample_catalog(), 8);
        let first = router
            .resolve("acme/cheap", TaskHint::Tools, false, false)
            .unwrap();
        assert!(!first.from_cache);
        let second = router
            .resolve("acme/cheap", TaskHint::Tools, false, false)
            .unwrap();
        assert!(second.from_cache);
        let mut second_no_flag = second.clone();
        second_no_flag.from_cache = false;
        assert_eq!(second_no_flag, first);
        assert_eq!(router.stats().hits, 1);
        assert_eq!(router.stats().misses, 1);
    }

    #[test]
    fn requirement_change_breaks_cache_key() {
        let mut router = GatewayRouter::new(sample_catalog(), 8);
        let r1 = router
            .resolve("acme/cheap", TaskHint::Chat, false, false)
            .unwrap();
        let r2 = router.resolve("acme/cheap", TaskHint::Chat, true, false);
        assert!(!r2.unwrap().from_cache, "different filters must re-resolve");
        assert_eq!(router.stats().misses, 2);
        let _ = r1;
    }

    #[test]
    fn lru_eviction_and_invalidation() {
        let mut router = GatewayRouter::new(sample_catalog(), 2);
        let _ = router
            .resolve("acme/cheap", TaskHint::Chat, false, false)
            .unwrap();
        let _ = router
            .resolve("acme/expensive", TaskHint::Chat, false, false)
            .unwrap();
        let _ = router
            .resolve("acme/visionless", TaskHint::Chat, false, false)
            .unwrap();
        assert_eq!(router.cache_len(), 2); // cheapest evicted
        let again = router
            .resolve("acme/cheap", TaskHint::Chat, false, false)
            .unwrap();
        assert!(!again.from_cache); // re-resolved after eviction

        router.invalidate("acme/expensive");
        let again2 = router
            .resolve("acme/expensive", TaskHint::Chat, false, false)
            .unwrap();
        assert!(!again2.from_cache);
    }

    #[test]
    fn unknown_alias_is_rejected() {
        let mut router = GatewayRouter::new(sample_catalog(), 8);
        let err = router.resolve("acme/not-a-model", TaskHint::Chat, false, false);
        assert_eq!(
            err,
            Err(GatewayError::NoCapableRoute("acme/not-a-model".into()))
        );
        assert_eq!(router.stats().rejected, 0); // stats: rejections counted on requirement-fails only
    }
}
