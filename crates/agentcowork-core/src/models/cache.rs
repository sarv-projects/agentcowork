//! P52.6 — model cache: pinned + LRU weights with TTL expiry, plus a
//! tokens-per-second benchmark helper.
//!
//! [`ModelCache`] tracks which models are resident: `touch` records use
//! (refreshing recency), `pin`/`unpin` protect the user's keep-list, and
//! [`ModelCache::evict_expired`] drops unpinned entries idle longer than
//! `ttl_ms`, returning what it dropped. [`benchmark_from_samples`] turns a
//! (tokens, elapsed-ms) sample into a [`Benchmark`] — the throughput number
//! the picker shows next to each cached model.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Resident-model cache: pinned keep-list + LRU recency + TTL expiry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCache {
    /// Ids the user pinned — never evicted by [`ModelCache::evict_expired`].
    pub pinned: HashSet<String>,
    /// Resident ids, least-recently-used first (touch moves to the back).
    pub lru: Vec<String>,
    /// Idle time after which an unpinned entry expires (ms).
    pub ttl_ms: u64,
    /// Last-use timestamp per resident id (ms, caller clock).
    pub last_used_ms: HashMap<String, u64>,
}

impl ModelCache {
    pub fn new(ttl_ms: u64) -> Self {
        Self {
            pinned: HashSet::new(),
            lru: Vec::new(),
            ttl_ms,
            last_used_ms: HashMap::new(),
        }
    }

    /// Pin `id` (protect from expiry). Also marks it resident so a pinned
    /// model that was never touched still survives eviction.
    pub fn pin(&mut self, id: &str) {
        self.pinned.insert(id.to_string());
        if !self.lru.iter().any(|e| e == id) {
            self.lru.push(id.to_string());
        }
    }

    /// Unpin `id` (it becomes evictable again; stays resident until expiry).
    pub fn unpin(&mut self, id: &str) {
        self.pinned.remove(id);
    }

    /// Record use of `id` at `now_ms`: refreshes recency (back of the LRU)
    /// and the last-used timestamp.
    pub fn touch(&mut self, id: &str, now_ms: u64) {
        self.last_used_ms.insert(id.to_string(), now_ms);
        if let Some(pos) = self.lru.iter().position(|e| e == id) {
            self.lru.remove(pos);
        }
        self.lru.push(id.to_string());
    }

    /// Drop every unpinned entry idle longer than `ttl_ms` as of `now_ms`.
    /// Pinned entries always survive. Returns the evicted ids in LRU order.
    pub fn evict_expired(&mut self, now_ms: u64) -> Vec<String> {
        let mut evicted = Vec::new();
        let mut kept = Vec::with_capacity(self.lru.len());
        for id in std::mem::take(&mut self.lru) {
            let idle = now_ms.saturating_sub(*self.last_used_ms.get(&id).unwrap_or(&now_ms));
            if !self.pinned.contains(&id) && idle > self.ttl_ms {
                self.last_used_ms.remove(&id);
                evicted.push(id);
            } else {
                kept.push(id);
            }
        }
        self.lru = kept;
        evicted
    }
}

/// Throughput sample for one cached model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Benchmark {
    /// Tokens per second.
    pub tok_per_s: f64,
}

/// Throughput from a (tokens, elapsed-ms) sample. Zero elapsed time yields
/// 0 tok/s (never infinity — the picker must render *something* honest).
pub fn benchmark_from_samples(tokens: u64, ms: u64) -> Benchmark {
    if ms == 0 {
        Benchmark { tok_per_s: 0.0 }
    } else {
        Benchmark {
            tok_per_s: tokens as f64 / (ms as f64 / 1000.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_unpinned_expired() {
        let mut c = ModelCache::new(100);
        c.touch("a", 0);
        c.touch("b", 50);
        // At t=200 both are idle past the 100ms TTL → both go, LRU order.
        let evicted = c.evict_expired(200);
        assert_eq!(evicted, vec!["a".to_string(), "b".to_string()]);
        assert!(c.lru.is_empty());
        // Fresh entries within the TTL survive.
        c.touch("c", 200);
        assert!(c.evict_expired(250).is_empty());
        assert_eq!(c.lru, vec!["c".to_string()]);
    }

    #[test]
    fn pinned_survives_eviction() {
        let mut c = ModelCache::new(100);
        c.touch("keep", 0);
        c.pin("keep");
        c.touch("drop", 0);
        let evicted = c.evict_expired(10_000);
        assert_eq!(evicted, vec!["drop".to_string()]);
        assert_eq!(c.lru, vec!["keep".to_string()]);
        // Unpinning makes it evictable again.
        c.unpin("keep");
        let evicted = c.evict_expired(10_000);
        assert_eq!(evicted, vec!["keep".to_string()]);
        assert!(c.lru.is_empty());
    }

    #[test]
    fn benchmark_tps_math() {
        assert_eq!(
            benchmark_from_samples(100, 2000),
            Benchmark { tok_per_s: 50.0 }
        );
        assert_eq!(benchmark_from_samples(1, 1000).tok_per_s, 1.0);
        // Zero time never yields infinity.
        assert_eq!(benchmark_from_samples(100, 0).tok_per_s, 0.0);
    }
}
