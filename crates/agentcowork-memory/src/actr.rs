//! ACT-R activation + spontaneous recall (Algorithm #32, NOOA nooa-memory
//! pattern — doc 39 forgetting.py). Retention decays with an effective
//! half-life of `half_life × log1p(strength)`; memories with importance ≥ 8
//! are never auto-forgotten; recall fuses semantic + keyword + recency +
//! graph signals; a pre-turn hook derives spontaneous queries from context.

use std::collections::HashMap;

pub const DEFAULT_IMPORTANCE_FLOOR: u8 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Memory {
    pub id: String,
    /// 0–10 salience.
    pub importance: u8,
    /// Base activation strength (≥ 0).
    pub strength: f64,
    /// Unix seconds the memory was created.
    pub created_at: u64,
    /// Unix seconds the memory was last accessed (≥ created_at).
    pub last_access: u64,
    pub keywords: Vec<String>,
    /// Number of typed relational edges (supports/contradicts/derived-from).
    pub graph_links: u32,
}

/// Effective half-life = `half_life × ln(1 + strength)`. Activation decays
/// exponentially with age: `strength · exp(−age / effective_half_life)`.
pub fn activation(mem: &Memory, now: u64, half_life: f64) -> f64 {
    let age = now.saturating_sub(mem.last_access.max(mem.created_at)) as f64;
    let effective = half_life * (1.0 + mem.strength).ln();
    if effective <= 0.0 {
        return 0.0;
    }
    mem.strength * (-age / effective).exp()
}

/// Importance floor: `importance >= floor` ⇒ never auto-forgotten.
pub fn is_protected(mem: &Memory, floor: u8) -> bool {
    mem.importance >= floor
}

/// Which memories survive an auto-forget sweep (protected + still-activated).
pub fn forget_sweep(
    mems: &[Memory],
    now: u64,
    half_life: f64,
    floor: u8,
    activation_threshold: f64,
) -> Vec<&Memory> {
    mems.iter()
        .filter(|m| is_protected(m, floor) || activation(m, now, half_life) >= activation_threshold)
        .collect()
}

/// Keyword-hit fraction: how many query terms match the memory's keywords.
pub fn keyword_hits(mem: &Memory, query_terms: &[&str]) -> f64 {
    if query_terms.is_empty() {
        return 0.0;
    }
    let matched = query_terms
        .iter()
        .filter(|t| {
            mem.keywords
                .iter()
                .any(|k| k.eq_ignore_ascii_case(t.trim()))
        })
        .count();
    matched as f64 / query_terms.len() as f64
}

/// Recency as a 0..1 value that decays with age (in seconds).
pub fn recency(age_secs: u64) -> f64 {
    1.0 / (1.0 + age_secs as f64)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecallWeights {
    pub semantic: f64,
    pub keyword: f64,
    pub recency: f64,
    pub graph: f64,
}

impl Default for RecallWeights {
    fn default() -> Self {
        RecallWeights {
            semantic: 0.4,
            keyword: 0.3,
            recency: 0.2,
            graph: 0.1,
        }
    }
}

/// Fused associative recall score. `semantic_sim` is the embedding cosine
/// (caller-supplied); `kw_hits`, `recency`, and `graph` come from this crate.
pub fn recall_score(
    semantic_sim: f64,
    kw_hits: f64,
    recency: f64,
    graph_links: u32,
    w: &RecallWeights,
) -> f64 {
    let graph = (graph_links as f64).min(1.0);
    w.semantic * semantic_sim + w.keyword * kw_hits + w.recency * recency + w.graph * graph
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "of", "to", "in", "on", "and",
    "or", "for", "with", "this", "that", "it", "as", "at", "by", "from",
];

/// Pre-turn spontaneous-recall hook: derive salient queries from recent
/// context terms (frequency-ordered, stopword-filtered).
pub fn derive_queries(context_terms: &[&str], max: usize) -> Vec<String> {
    let mut freq: HashMap<String, usize> = HashMap::new();
    for raw in context_terms {
        let t = raw.trim().to_lowercase();
        if t.len() < 3 || STOPWORDS.contains(&t.as_str()) {
            continue;
        }
        *freq.entry(t).or_insert(0) += 1;
    }
    let mut v: Vec<(String, usize)> = freq.into_iter().collect();
    v.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| a.0.cmp(&b.0))
    });
    v.truncate(max);
    v.into_iter().map(|(t, _)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(id: &str, importance: u8, strength: f64) -> Memory {
        Memory {
            id: id.into(),
            importance,
            strength,
            created_at: 0,
            last_access: 0,
            keywords: vec![],
            graph_links: 0,
        }
    }

    #[test]
    fn activation_decays_with_age() {
        let m = mem("a", 5, 1.0);
        let fresh = activation(&m, 0, 100.0);
        let old = activation(&m, 1_000_000, 100.0);
        assert!((fresh - 1.0).abs() < 1e-9);
        assert!(old < 0.001);
    }

    #[test]
    fn stronger_memory_decays_slower() {
        let weak = mem("w", 5, 1.0);
        let strong = mem("s", 5, 4.0);
        let now = 10_000;
        assert!(activation(&strong, now, 100.0) > activation(&weak, now, 100.0));
    }

    #[test]
    fn importance_floor_protects() {
        let high = mem("h", 8, 0.01);
        let low = mem("l", 7, 0.01);
        assert!(is_protected(&high, DEFAULT_IMPORTANCE_FLOOR));
        assert!(!is_protected(&low, DEFAULT_IMPORTANCE_FLOOR));

        // A protected memory survives even when its activation is ~0.
        let mems = [high.clone(), low.clone()];
        let survivors = forget_sweep(&mems, 1_000_000, 100.0, 8, 0.5);
        assert!(survivors.iter().any(|m| m.id == "h"));
        assert!(!survivors.iter().any(|m| m.id == "l"));
    }

    #[test]
    fn recall_score_fuses_signals() {
        let w = RecallWeights::default();
        let s = recall_score(0.8, 1.0, 0.5, 0, &w);
        assert!((s - (0.4 * 0.8 + 0.3 * 1.0 + 0.2 * 0.5 + 0.1 * 0.0)).abs() < 1e-9);
    }

    #[test]
    fn keyword_hits_fraction() {
        let mut m = mem("k", 5, 1.0);
        m.keywords = vec!["rust".into(), "memory".into()];
        assert_eq!(keyword_hits(&m, &["rust", "zzz"]), 0.5);
        assert_eq!(keyword_hits(&m, &["rust", "memory"]), 1.0);
    }

    #[test]
    fn derive_queries_ranks_by_frequency() {
        let terms = ["rust", "the", "rust", "memory", "rust", "fusion"];
        let q = derive_queries(&terms, 2);
        assert_eq!(q, vec!["rust".to_string(), "fusion".to_string()]);
    }
}
