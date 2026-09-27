//! 3-layer cache stack (P1.3 — doc 62): the prompt cache lives in the broker
//! (Anthropic `cache_control:ephemeral` / OpenAI ≥1024-token prefix); this
//! module owns the other two layers, both deterministic, TTL'd, and
//! **read-only-intent gated** (a cache entry produced from a mutation path is
//! never served — "never serves into mutation paths").
//!
//! - [`SemanticCache`] — exact + near-match (token-set Jaccard ≥ threshold)
//!   prompt→response reuse; the embedding-backed cosine path (C5) plugs in via
//!   [`crate::embedding`] when a model is loaded.
//! - [`ResultCache`] — dependency-tagged results invalidated by tag (so a
//!   change to a file/table drops every cached result derived from it).
//!
//! `now` is an epoch-seconds clock passed in so tests are deterministic.

use std::collections::{HashMap, HashSet};

use crate::bm25::tokenize;

/// Semantic (prompt) cache. One entry per normalized prompt; lookups return
/// the nearest entry at or above `sim_threshold` (Jaccard token similarity).
#[derive(Debug, Clone, Default)]
pub struct SemanticCache {
    entries: Vec<SemanticEntry>,
    sim_threshold: f64,
    ttl_secs: u64,
}

#[derive(Debug, Clone)]
struct SemanticEntry {
    prompt: String,
    tokens: HashSet<String>,
    response: String,
    created_at: u64,
    read_only: bool,
}

impl SemanticCache {
    /// `sim_threshold` = minimum Jaccard similarity for a near-match (doc 62's
    /// ~0.92 cosine target is reached with embeddings; the vectorless default
    /// uses token overlap). `ttl_secs` = entry lifetime.
    pub fn new(sim_threshold: f64, ttl_secs: u64) -> Self {
        Self {
            entries: Vec::new(),
            sim_threshold: sim_threshold.clamp(0.0, 1.0),
            ttl_secs,
        }
    }

    /// Store a response for a prompt. `read_only` records the intent: false
    /// entries are retained for accounting but never served by [`get`].
    pub fn put(&mut self, prompt: &str, response: &str, read_only: bool, now: u64) {
        self.evict_expired(now);
        let key = normalize(prompt);
        let tokens: HashSet<String> = tokenize(&key).into_iter().collect();
        self.entries.push(SemanticEntry {
            prompt: key,
            tokens,
            response: response.to_string(),
            created_at: now,
            read_only,
        });
    }

    /// Look up the best non-expired, read-only match for `prompt`: exact
    /// normalized match first, else the highest-Jaccard entry at/above the
    /// threshold. Returns the cached response.
    pub fn get(&mut self, prompt: &str, now: u64) -> Option<&str> {
        self.evict_expired(now);
        let key = normalize(prompt);
        let tokens: HashSet<String> = tokenize(&key).into_iter().collect();

        let mut best: Option<(&SemanticEntry, f64)> = None;
        for e in &self.entries {
            if !e.read_only {
                continue;
            }
            if e.prompt == key {
                return Some(&e.response);
            }
            let sim = jaccard(&tokens, &e.tokens);
            if sim >= self.sim_threshold {
                let better = best.as_ref().map(|(_, s)| sim > *s).unwrap_or(true);
                if better {
                    best = Some((e, sim));
                }
            }
        }
        best.map(|(e, _)| e.response.as_str())
    }

    /// Number of live (non-expired) entries.
    pub fn len(&mut self, now: u64) -> usize {
        self.evict_expired(now);
        self.entries.len()
    }

    pub fn is_empty(&mut self, now: u64) -> bool {
        self.len(now) == 0
    }

    fn evict_expired(&mut self, now: u64) {
        let ttl = self.ttl_secs;
        self.entries
            .retain(|e| now.saturating_sub(e.created_at) < ttl);
    }
}

/// Dependency-tagged result cache: results are keyed by a task signature and
/// tagged with the dependencies they were derived from; invalidating a tag
/// drops every result that depends on it.
#[derive(Debug, Clone, Default)]
pub struct ResultCache {
    entries: HashMap<String, ResultEntry>,
    /// tag → keys that depend on it (for dependency-tagged invalidation).
    tag_index: HashMap<String, HashSet<String>>,
    ttl_secs: u64,
}

#[derive(Debug, Clone)]
struct ResultEntry {
    value: String,
    created_at: u64,
    read_only: bool,
}

impl ResultCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: HashMap::new(),
            tag_index: HashMap::new(),
            ttl_secs,
        }
    }

    /// Store a result keyed by `signature`, derived from `dependencies`.
    pub fn put(
        &mut self,
        signature: &str,
        value: &str,
        dependencies: &[&str],
        read_only: bool,
        now: u64,
    ) {
        for tag in dependencies {
            self.tag_index
                .entry((*tag).to_string())
                .or_default()
                .insert(signature.to_string());
        }
        self.entries.insert(
            signature.to_string(),
            ResultEntry {
                value: value.to_string(),
                created_at: now,
                read_only,
            },
        );
    }

    /// Fetch a non-expired, read-only result by signature.
    pub fn get(&mut self, signature: &str, now: u64) -> Option<&str> {
        self.evict_expired(now);
        self.entries.get(signature).and_then(|e| {
            if e.read_only {
                Some(e.value.as_str())
            } else {
                None
            }
        })
    }

    /// Drop every result tagged with `tag` (dependency-tagged invalidation).
    pub fn invalidate_tag(&mut self, tag: &str) -> usize {
        let keys: Vec<String> = self
            .tag_index
            .remove(tag)
            .map(|s| s.into_iter().collect())
            .unwrap_or_default();
        let mut removed = 0;
        for key in &keys {
            if self.entries.remove(key).is_some() {
                removed += 1;
            }
            // Clean up this key from other tags' indexes.
            for set in self.tag_index.values_mut() {
                set.remove(key);
            }
        }
        removed
    }

    pub fn len(&mut self, now: u64) -> usize {
        self.evict_expired(now);
        self.entries.len()
    }

    fn evict_expired(&mut self, now: u64) {
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| now.saturating_sub(e.created_at) >= self.ttl_secs)
            .map(|(k, _)| k.clone())
            .collect();
        for key in expired {
            self.entries.remove(&key);
            for set in self.tag_index.values_mut() {
                set.remove(&key);
            }
        }
    }
}

/// Normalize a prompt for cache keying (trim + collapse whitespace).
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Jaccard similarity of two token sets in [0, 1] (1 = identical).
fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return if a.is_empty() && b.is_empty() {
            1.0
        } else {
            0.0
        };
    }
    a.intersection(b).count() as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_cache_exact_hit() {
        let mut c = SemanticCache::new(0.85, 3600);
        c.put("what is the capital of France", "Paris", true, 0);
        assert_eq!(c.get("what is the capital of France", 10), Some("Paris"));
    }

    #[test]
    fn semantic_cache_near_match_hit() {
        let mut c = SemanticCache::new(0.5, 3600);
        c.put("how do I fix a slow browser", "answer", true, 0);
        assert_eq!(c.get("fix slow browser", 10), Some("answer"));
    }

    #[test]
    fn semantic_cache_respects_similarity_threshold() {
        let mut c = SemanticCache::new(0.95, 3600);
        c.put("browser automation tools", "x", true, 0);
        assert_eq!(c.get("completely unrelated topic", 10), None);
    }

    #[test]
    fn semantic_cache_never_serves_mutation_entries() {
        let mut c = SemanticCache::new(0.0, 3600);
        c.put("delete all rows", "done", false, 0); // mutation path
        assert_eq!(c.get("delete all rows", 10), None);
        assert_eq!(c.len(10), 1); // still retained for accounting
    }

    #[test]
    fn semantic_cache_ttl_expires() {
        let mut c = SemanticCache::new(0.85, 100);
        c.put("hello", "hi", true, 0);
        assert_eq!(c.get("hello", 99), Some("hi"));
        assert_eq!(c.get("hello", 100), None);
    }

    #[test]
    fn result_cache_dependency_invalidation() {
        let mut c = ResultCache::new(3600);
        c.put("report-q3", "R1", &["budget.xlsx", "receipts"], true, 0);
        c.put("report-q4", "R2", &["budget.xlsx"], true, 0);
        assert_eq!(c.get("report-q3", 10), Some("R1"));
        // A change to budget.xlsx invalidates both derived results.
        let removed = c.invalidate_tag("budget.xlsx");
        assert_eq!(removed, 2);
        assert_eq!(c.get("report-q3", 10), None);
        assert_eq!(c.get("report-q4", 10), None);
    }

    #[test]
    fn result_cache_never_serves_mutation_entries() {
        let mut c = ResultCache::new(3600);
        c.put("write-rows", "ok", &["sheet"], false, 0);
        assert_eq!(c.get("write-rows", 10), None);
    }

    #[test]
    fn result_cache_ttl_expires() {
        let mut c = ResultCache::new(50);
        c.put("k", "v", &[], true, 0);
        assert_eq!(c.get("k", 49), Some("v"));
        assert_eq!(c.get("k", 50), None);
    }

    #[test]
    fn normalize_collapses_whitespace() {
        let mut c = SemanticCache::new(1.0, 3600);
        c.put("a   b\n\tc", "x", true, 0);
        assert_eq!(c.get("a b c", 10), Some("x"));
    }

    #[test]
    fn jaccard_edge_cases() {
        let empty: HashSet<String> = HashSet::new();
        assert_eq!(jaccard(&empty, &empty), 1.0);
        let a: HashSet<String> = ["x".into()].into_iter().collect();
        assert_eq!(jaccard(&a, &empty), 0.0);
    }
}
