//! Cross-encoder hybrid rerank (Algorithm #19 — doc 32/21): after the
//! bi-encoder signals (BM25 / vector / graph) return top-k candidates, a
//! cross-encoder re-scores each `(query, candidate)` pair and the final
//! ranking blends the retrieval score with the cross-encoder score.
//!
//! [`Reranker`] is the seam for a real on-device cross-encoder (e.g. an ONNX
//! bge-reranker); [`LexicalReranker`] is the deterministic, model-free
//! fallback — joint lexical evidence (exact phrase > bigram > unigram
//! overlap) — that keeps the rerank stage testable with no model loaded.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::bm25::tokenize;

/// One candidate document/chunk to re-score (id + its text).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub text: String,
}

/// Re-rank output: id + final blended score (0..=1), sorted best-first.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedHit {
    pub id: String,
    pub score: f64,
}

/// The cross-encoder seam: score `(query, candidate)` jointly in 0..=1.
pub trait Reranker {
    fn score(&self, query: &str, candidate: &str) -> f64;
}

/// Deterministic lexical cross-encoder (no model). Weights control the
/// evidence mix; defaults favor exact phrases, then ordered bigrams, then
/// unordered unigram overlap.
#[derive(Debug, Clone, Copy)]
pub struct LexicalReranker {
    pub phrase_weight: f64,
    pub bigram_weight: f64,
    pub unigram_weight: f64,
}

impl Default for LexicalReranker {
    fn default() -> Self {
        Self {
            phrase_weight: 0.5,
            bigram_weight: 0.3,
            unigram_weight: 0.2,
        }
    }
}

impl LexicalReranker {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Reranker for LexicalReranker {
    fn score(&self, query: &str, candidate: &str) -> f64 {
        let q = query.trim().to_lowercase();
        let c = candidate.to_lowercase();

        // Exact contiguous phrase match (the strongest joint evidence).
        let phrase = if q.is_empty() || !c.contains(&q) {
            0.0
        } else {
            1.0
        };

        let qt = tokenize(query);
        let ct = tokenize(candidate);

        let bigrams = |t: &[String]| -> HashSet<(String, String)> {
            t.windows(2).map(|w| (w[0].clone(), w[1].clone())).collect()
        };
        let qb = bigrams(&qt);
        let cb = bigrams(&ct);
        let bigram = if qb.is_empty() {
            0.0
        } else {
            qb.intersection(&cb).count() as f64 / qb.len() as f64
        };

        let qset: HashSet<&String> = qt.iter().collect();
        let cset: HashSet<&String> = ct.iter().collect();
        let union = qset.union(&cset).count();
        let inter = qset.intersection(&cset).count();
        let unigram = if union == 0 {
            0.0
        } else {
            inter as f64 / union as f64
        };

        (self.phrase_weight * phrase + self.bigram_weight * bigram + self.unigram_weight * unigram)
            .clamp(0.0, 1.0)
    }
}

/// Hybrid re-rank: `final = alpha * retrieval + beta * cross_encoder`, then
/// re-sort descending. `retrieval` maps candidate id → retrieval score
/// (0..=1); candidates absent from the map score 0 on that axis. Deterministic
/// tie-break by id.
pub fn rerank(
    candidates: &[Candidate],
    retrieval: &HashMap<String, f64>,
    reranker: &dyn Reranker,
    query: &str,
    alpha: f64,
    beta: f64,
    top_k: usize,
) -> Vec<RankedHit> {
    let mut scored: Vec<RankedHit> = candidates
        .iter()
        .map(|c| {
            let retr = retrieval.get(&c.id).copied().unwrap_or(0.0);
            let cross = reranker.score(query, &c.text);
            let score = (alpha * retr + beta * cross).clamp(0.0, 1.0);
            RankedHit {
                id: c.id.clone(),
                score,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    scored.truncate(top_k);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retrieval_map(scores: &[(&str, f64)]) -> HashMap<String, f64> {
        scores.iter().map(|(id, s)| (id.to_string(), *s)).collect()
    }

    #[test]
    fn lexical_ranker_prefers_exact_phrase() {
        let r = LexicalReranker::new();
        let exact = r.score(
            "rust memory safety",
            "we talk about rust memory safety here",
        );
        let scattered = r.score("rust memory safety", "memory is safe but rust is elsewhere");
        assert!(
            exact > scattered,
            "exact {exact} should beat scattered {scattered}"
        );
        assert!((0.0..=1.0).contains(&exact));
    }

    #[test]
    fn lexical_ranker_unigram_overlap() {
        let r = LexicalReranker::new();
        let shared = r.score("browser automation", "browser automation tools");
        let unrelated = r.score("browser automation", "a completely different subject");
        assert!(shared > unrelated);
        assert!((unrelated - 0.0).abs() < 1e-9);
    }

    #[test]
    fn empty_query_scores_zero() {
        let r = LexicalReranker::new();
        assert_eq!(r.score("", "anything at all"), 0.0);
    }

    #[test]
    fn rerank_blends_retrieval_and_cross_encoder() {
        // The retriever ranked A first, but the cross-encoder strongly prefers
        // B — a blended score can reorder them when beta dominates.
        let candidates = vec![
            Candidate {
                id: "a".into(),
                text: "generic filler content".to_string(),
            },
            Candidate {
                id: "b".into(),
                text: "rust borrow checker memory safety".to_string(),
            },
        ];
        let retrieval = retrieval_map(&[("a", 0.9), ("b", 0.5)]);
        let reranker = LexicalReranker::new();
        let hits = rerank(
            &candidates,
            &retrieval,
            &reranker,
            "rust borrow checker",
            0.2,
            0.8,
            2,
        );
        assert_eq!(hits[0].id, "b", "cross-encoder evidence should dominate");
        assert!(hits[0].score <= 1.0);
    }

    #[test]
    fn rerank_truncates_to_top_k() {
        let candidates: Vec<Candidate> = (0..5)
            .map(|i| Candidate {
                id: format!("d{i}"),
                text: format!("doc {i}"),
            })
            .collect();
        let retrieval = retrieval_map(&[]);
        let hits = rerank(
            &candidates,
            &retrieval,
            &LexicalReranker::new(),
            "doc",
            0.5,
            0.5,
            3,
        );
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn rerank_deterministic_tie_break() {
        let candidates = vec![
            Candidate {
                id: "b".into(),
                text: "x".to_string(),
            },
            Candidate {
                id: "a".into(),
                text: "x".to_string(),
            },
        ];
        let hits = rerank(
            &candidates,
            &retrieval_map(&[]),
            &LexicalReranker::new(),
            "x",
            0.0,
            0.0,
            2,
        );
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[1].id, "b");
    }
}
