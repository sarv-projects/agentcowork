//! Plan cache (P6.1 — doc 62): don't re-infer a plan you already know. Each
//! stored plan is keyed by a normalized **task signature**; a new goal is
//! matched by cosine similarity over word unigram+bigram shingles (default
//! threshold `0.85`). Version-based invalidation: a rewrite bumps the plan's
//! version, and lookups below `min_version` are ignored. Persisted to
//! `~/.everyaios/plans.db` (JSON — no SQLite dependency).

use crate::blueprint::Blueprint;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// One cached plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    /// Normalized goal text (the signature source).
    pub signature: String,
    pub blueprint: Blueprint,
    pub version: u32,
    /// Wall-clock ms when the entry was stored. The cache is a **memory**
    /// accelerator, not a durability layer, so a hit never outranks live state:
    /// [`PlanCache::is_fresh`] is how a caller refuses a stale inference.
    #[serde(default)]
    pub stored_at_ms: u64,
}

impl PlanEntry {
    /// A hit is fresh only while the entry is younger than `max_age_ms` and its
    /// version is at least `min_version`. An old hit is a *hint*, not a plan.
    pub fn is_fresh(&self, now_ms: u64, max_age_ms: u64, min_version: u32) -> bool {
        self.version >= min_version && now_ms.saturating_sub(self.stored_at_ms) <= max_age_ms
    }
}

#[derive(Debug, Error)]
pub enum PlanCacheError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Persisted {
    version: u32,
    entries: Vec<PlanEntry>,
}

/// An in-memory, file-backable plan cache.
#[derive(Debug, Default)]
pub struct PlanCache {
    entries: Vec<PlanEntry>,
    version: u32,
}

/// Default similarity threshold for a cache hit (doc 62: ~0.85).
pub const DEFAULT_SIMILARITY: f64 = 0.85;

/// Default freshness window for a cached plan.
pub const DEFAULT_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;

/// The freshness verdict for a plan-cache lookup.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum PlanCacheFreshness {
    /// No entry matched.
    Miss,
    /// A matching entry, within its age window and at/above `min_version`.
    Fresh { version: u32, similarity: f64 },
    /// A matching entry that is too old. The caller must re-infer rather than
    /// serve it: a stale plan is a silently wrong plan.
    Stale { version: u32 },
}

/// Normalized tokens of a stored signature (already normalized once, so the
/// lookup does not re-normalize a stored value with a changed normalizer).
fn s2(signature: &str) -> HashMap<String, u32> {
    shingles(
        &signature
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>(),
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl PlanCache {
    pub fn new(version: u32) -> Self {
        Self {
            entries: Vec::new(),
            version,
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Store (or replace) a plan under its signature.
    pub fn store(&mut self, blueprint: Blueprint, version: u32) {
        self.store_at(blueprint, version, now_ms());
    }

    /// Store with an explicit stamp, for a caller that has its own clock.
    pub fn store_at(&mut self, blueprint: Blueprint, version: u32, stored_at_ms: u64) {
        let signature = signature(&blueprint.goal);
        self.entries.retain(|e| e.signature != signature);
        self.entries.push(PlanEntry {
            signature,
            blueprint,
            version,
            stored_at_ms,
        });
    }

    /// Best-matching cached plan at/above `min_similarity` and `min_version`.
    pub fn lookup(&self, goal: &str, min_similarity: f64, min_version: u32) -> Option<&Blueprint> {
        let q = shingles(&normalize(goal));
        let mut best: Option<(f64, &PlanEntry)> = None;
        for e in &self.entries {
            if e.version < min_version {
                continue;
            }
            let s = shingles(&normalize(&e.signature));
            let sim = cosine(&q, &s);
            if best.map(|(b, _)| sim > b).unwrap_or(true) {
                best = Some((sim, e));
            }
        }
        match best {
            Some((sim, e)) if sim >= min_similarity => Some(&e.blueprint),
            _ => None,
        }
    }

    /// The matched entry, so a caller can check freshness rather than just take
    /// a hit. A cache that cannot be interrogated becomes a source of stale
    /// plans.
    pub fn lookup_entry(
        &self,
        goal: &str,
        min_similarity: f64,
        min_version: u32,
    ) -> Option<(&PlanEntry, f64)> {
        let q = shingles(&normalize(goal));
        let mut best: Option<(f64, &PlanEntry)> = None;
        for e in &self.entries {
            if e.version < min_version {
                continue;
            }
            let sim = cosine(&q, &s2(&e.signature));
            if best.map(|(b, _)| sim > b).unwrap_or(true) {
                best = Some((sim, e));
            }
        }
        match best {
            Some((sim, e)) if sim >= min_similarity => Some((e, sim)),
            _ => None,
        }
    }

    /// The freshness verdict for a goal: a hit that is too old is reported as
    /// stale rather than served, so a long-lived cache cannot outlive the truth
    /// it was inferred from.
    pub fn freshness(
        &self,
        goal: &str,
        min_similarity: f64,
        min_version: u32,
        now_ms: u64,
        max_age_ms: u64,
    ) -> PlanCacheFreshness {
        match self.lookup_entry(goal, min_similarity, min_version) {
            None => PlanCacheFreshness::Miss,
            Some((e, sim)) if e.is_fresh(now_ms, max_age_ms, min_version) => {
                PlanCacheFreshness::Fresh {
                    version: e.version,
                    similarity: sim,
                }
            }
            Some((e, _)) => PlanCacheFreshness::Stale { version: e.version },
        }
    }

    /// Drop every entry older than `max_age_ms`. The cache is an accelerator, so
    /// evicting by age is free; keeping an entry longer only risks a stale hit.
    pub fn evict_older_than(&mut self, now_ms: u64, max_age_ms: u64) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|e| now_ms.saturating_sub(e.stored_at_ms) <= max_age_ms);
        before - self.entries.len()
    }

    /// Drop a plan by its signature.
    pub fn invalidate(&mut self, signature: &str) {
        self.entries.retain(|e| e.signature != signature);
    }

    /// Drop every plan older than `min_version` (version-based invalidation).
    pub fn invalidate_below(&mut self, min_version: u32) {
        self.entries.retain(|e| e.version >= min_version);
    }

    /// Bump the cache's epoch version (invalidates lookups below it).
    pub fn bump_version(&mut self) -> u32 {
        self.version += 1;
        self.version
    }

    pub fn save(&self, path: &Path) -> Result<(), PlanCacheError> {
        let persisted = Persisted {
            version: self.version,
            entries: self.entries.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, PlanCacheError> {
        let bytes = std::fs::read(path)?;
        let persisted: Persisted = serde_json::from_slice(&bytes)?;
        Ok(Self {
            entries: persisted.entries,
            version: persisted.version,
        })
    }

    /// The default on-disk location (`~/.everyaios/plans.db`), honoring
    /// `EVERYAIOS_HOME` when set.
    pub fn default_path() -> PathBuf {
        if let Ok(home) = std::env::var("EVERYAIOS_HOME") {
            return PathBuf::from(home).join("plans.db");
        }
        let base = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        base.join(".everyaios").join("plans.db")
    }
}

/// Normalize a goal into tokens (lowercase, alphanumeric-only, stopwords
/// dropped so "and/the" noise doesn't sink the similarity).
fn normalize(s: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of",
        "on", "or", "that", "the", "this", "to", "was", "with",
    ];
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

/// The signature: normalized tokens joined (stable, order-preserving).
pub fn signature(goal: &str) -> String {
    normalize(goal).join(" ")
}

/// Word unigram + bigram shingle counts.
fn shingles(tokens: &[String]) -> HashMap<String, u32> {
    let mut m = HashMap::new();
    for t in tokens {
        *m.entry(t.clone()).or_insert(0) += 1;
    }
    for pair in tokens.windows(2) {
        let key = format!("{}|{}", pair[0], pair[1]);
        *m.entry(key).or_insert(0) += 1;
    }
    m
}

/// Cosine similarity over shingle count vectors (0..=1).
fn cosine(a: &HashMap<String, u32>, b: &HashMap<String, u32>) -> f64 {
    let mut keys: HashSet<&String> = a.keys().collect();
    keys.extend(b.keys());
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for k in keys {
        let va = *a.get(k).unwrap_or(&0) as f64;
        let vb = *b.get(k).unwrap_or(&0) as f64;
        dot += va * vb;
        na += va * va;
        nb += vb * vb;
    }
    let denom = (na * nb).sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{BlueprintTask, VerifyBlock};
    use crate::spec::TaskSpec;

    fn bp(goal: &str) -> Blueprint {
        let mut b = Blueprint::new("bp", goal);
        b.push(BlueprintTask::new(
            TaskSpec::new("a", "do it"),
            VerifyBlock::new(vec![]),
        ));
        b
    }

    #[test]
    fn near_identical_goal_hits_cache() {
        let mut cache = PlanCache::new(1);
        cache.store(bp("audit q3 expenses and update the budget sheet"), 1);
        let hit = cache.lookup(
            "audit Q3 expenses & update budget sheet!",
            DEFAULT_SIMILARITY,
            1,
        );
        assert!(hit.is_some());
    }

    #[test]
    fn unrelated_goal_misses_cache() {
        let mut cache = PlanCache::new(1);
        cache.store(bp("audit q3 expenses"), 1);
        assert!(
            cache
                .lookup("rename all photos by date", DEFAULT_SIMILARITY, 1)
                .is_none()
        );
    }

    #[test]
    fn version_invalidation_blocks_stale_plans() {
        let mut cache = PlanCache::new(1);
        cache.store(bp("audit q3 expenses"), 1);
        assert!(
            cache
                .lookup("audit q3 expenses", DEFAULT_SIMILARITY, 2)
                .is_none()
        );
        assert!(
            cache
                .lookup("audit q3 expenses", DEFAULT_SIMILARITY, 1)
                .is_some()
        );
    }

    #[test]
    fn invalidate_by_signature_and_version() {
        let mut cache = PlanCache::new(0);
        cache.store(bp("audit q3 expenses"), 2);
        cache.store(bp("rename photos"), 4);
        cache.invalidate(&signature("audit q3 expenses"));
        assert_eq!(cache.len(), 1);
        cache.invalidate_below(5);
        assert!(cache.is_empty());
    }

    #[test]
    fn save_and_load_roundtrips() {
        let dir = std::env::temp_dir().join("bp-plancache");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plans.db");
        let mut cache = PlanCache::new(7);
        cache.store(bp("audit q3 expenses"), 3);
        cache.save(&path).unwrap();

        let back = PlanCache::load(&path).unwrap();
        assert_eq!(back.version(), 7);
        assert!(
            back.lookup("audit q3 expenses", DEFAULT_SIMILARITY, 1)
                .is_some()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_path_honors_home_override() {
        // Edition 2024 marks process-environment mutation unsafe.
        unsafe {
            std::env::set_var("EVERYAIOS_HOME", "/tmp/agentcowork-test-home");
        }
        assert_eq!(
            PlanCache::default_path(),
            PathBuf::from("/tmp/agentcowork-test-home/plans.db")
        );
        unsafe {
            std::env::remove_var("EVERYAIOS_HOME");
        }
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;
    use crate::blueprint::{BlueprintTask, VerifyBlock};
    use crate::spec::TaskSpec;

    fn bp(goal: &str) -> Blueprint {
        let mut b = Blueprint::new("bp", goal);
        b.push(BlueprintTask::new(
            TaskSpec::new("a", "do it"),
            VerifyBlock::new(vec![]),
        ));
        b
    }

    #[test]
    fn a_fresh_hit_is_reported_fresh_with_its_version_and_similarity() {
        let mut cache = PlanCache::new(1);
        cache.store_at(bp("audit q3 expenses"), 3, 1_000);
        let v = cache.freshness(
            "audit q3 expenses",
            DEFAULT_SIMILARITY,
            1,
            1_000 + 1_000,
            DEFAULT_MAX_AGE_MS,
        );
        match v {
            PlanCacheFreshness::Fresh { version, .. } => assert_eq!(version, 3),
            other => panic!("expected a fresh hit, got {other:?}"),
        }
    }

    #[test]
    fn a_hit_past_its_age_window_is_reported_stale_not_served() {
        let mut cache = PlanCache::new(1);
        cache.store_at(bp("audit q3 expenses"), 3, 1_000);
        let v = cache.freshness(
            "audit q3 expenses",
            DEFAULT_SIMILARITY,
            1,
            1_000 + DEFAULT_MAX_AGE_MS + 1,
            DEFAULT_MAX_AGE_MS,
        );
        assert_eq!(v, PlanCacheFreshness::Stale { version: 3 });
        // The entry is still there for the plain `lookup` callers, but the
        // freshness verdict is what an honest caller asks.
        assert!(
            cache
                .lookup("audit q3 expenses", DEFAULT_SIMILARITY, 1)
                .is_some()
        );
    }

    #[test]
    fn a_miss_is_a_miss_and_never_a_stale_hit() {
        let cache = PlanCache::new(1);
        assert_eq!(
            cache.freshness("anything", DEFAULT_SIMILARITY, 1, 0, DEFAULT_MAX_AGE_MS),
            PlanCacheFreshness::Miss
        );
    }

    #[test]
    fn a_version_floor_makes_an_entry_invisible_not_stale() {
        let mut cache = PlanCache::new(1);
        cache.store_at(bp("audit q3 expenses"), 3, 1_000);
        // Below the version floor the entry does not participate at all.
        assert_eq!(
            cache.freshness(
                "audit q3 expenses",
                DEFAULT_SIMILARITY,
                4,
                1_000,
                DEFAULT_MAX_AGE_MS
            ),
            PlanCacheFreshness::Miss
        );
    }

    #[test]
    fn age_eviction_drops_only_the_expired_entries() {
        let mut cache = PlanCache::new(1);
        cache.store_at(bp("old goal here"), 1, 1_000);
        cache.store_at(bp("new goal here"), 1, 90_000);
        let now = 100_000;
        let removed = cache.evict_older_than(now, DEFAULT_MAX_AGE_MS);
        // Neither is past a 24 h window yet.
        assert_eq!(removed, 0);
        let removed = cache.evict_older_than(now, 50_000);
        assert_eq!(removed, 1);
        assert_eq!(cache.len(), 1);
        assert!(
            cache
                .lookup("new goal here", DEFAULT_SIMILARITY, 1)
                .is_some()
        );
    }

    #[test]
    fn a_stored_entry_keeps_its_stamp_across_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("bp-plancache-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plans.db");
        let mut cache = PlanCache::new(7);
        cache.store_at(bp("audit q3 expenses"), 3, 1_234);
        cache.save(&path).unwrap();
        let back = PlanCache::load(&path).unwrap();
        match back.freshness(
            "audit q3 expenses",
            DEFAULT_SIMILARITY,
            1,
            1_300,
            DEFAULT_MAX_AGE_MS,
        ) {
            PlanCacheFreshness::Fresh { version, .. } => assert_eq!(version, 3),
            other => panic!("expected a fresh hit after reload, got {other:?}"),
        }
        assert_eq!(
            back.freshness(
                "audit q3 expenses",
                DEFAULT_SIMILARITY,
                1,
                1_234 + DEFAULT_MAX_AGE_MS + 1,
                DEFAULT_MAX_AGE_MS
            ),
            PlanCacheFreshness::Stale { version: 3 }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_legacy_entry_without_a_stamp_reads_as_very_old_rather_than_fresh() {
        // A cache file written before stamps existed: `stored_at_ms` defaults to
        // 0, so the entry is stale at any realistic clock. Never silently fresh.
        let entry = PlanEntry {
            signature: "audit".into(),
            blueprint: bp("audit"),
            version: 1,
            stored_at_ms: 0,
        };
        assert!(!entry.is_fresh(DEFAULT_MAX_AGE_MS + 1, DEFAULT_MAX_AGE_MS, 1));
        assert!(entry.is_fresh(0, DEFAULT_MAX_AGE_MS, 1));
    }
}
