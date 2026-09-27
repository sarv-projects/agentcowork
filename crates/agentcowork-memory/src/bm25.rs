//! Parallel signal execution (C4 — the "FTS5/BM25 vectorless default +
//! optional vector signal" integration). A [`Bm25Index`] is the vectorless
//! default retrieval signal; [`SignalSource`] is the seam the coordinator
//! runs in parallel (BM25 + optional vector + graph signals), and
//! [`fuse_signals`] merges their ranked hits with reciprocal-rank fusion.

use std::collections::HashMap;

/// BM25 parameters (Okapi).
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// The signal kind a hit came from (BM25 default, optional vector/graph).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Keyword,
    Semantic,
    Graph,
}

/// One scored retrieval hit.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub kind: SignalKind,
    /// Normalized 0..=1 relevance.
    pub confidence: f64,
}

/// One indexed document/chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Bm25Doc {
    pub id: String,
    pub text: String,
}

/// A BM25 index over a corpus (the vectorless default signal).
#[derive(Debug, Clone, Default)]
pub struct Bm25Index {
    docs: Vec<Bm25Doc>,
    /// term -> df (number of docs containing it).
    df: HashMap<String, usize>,
    /// doc index -> term frequencies.
    tf: Vec<HashMap<String, usize>>,
    avg_len: f64,
    /// term -> idf (computed at build time).
    idf: HashMap<String, f64>,
}

impl Bm25Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// Index (or re-index) the corpus. Replaces the prior index.
    pub fn build(&mut self, docs: Vec<Bm25Doc>) {
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut tf: Vec<HashMap<String, usize>> = Vec::with_capacity(docs.len());
        let mut total_len = 0usize;
        for doc in &docs {
            let terms = tokenize(&doc.text);
            let mut per_doc: HashMap<String, usize> = HashMap::new();
            for t in terms {
                *per_doc.entry(t).or_insert(0) += 1;
            }
            for t in per_doc.keys() {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            total_len += doc.text.split_whitespace().count();
            tf.push(per_doc);
        }
        let n = docs.len() as f64;
        let avg_len = if n == 0.0 { 0.0 } else { total_len as f64 / n };
        let idf: HashMap<String, f64> = df
            .iter()
            .map(|(t, d)| {
                (
                    t.clone(),
                    ((n - *d as f64 + 0.5) / (*d as f64 + 0.5) + 1.0).ln(),
                )
            })
            .collect();
        self.docs = docs;
        self.df = df;
        self.tf = tf;
        self.avg_len = avg_len;
        self.idf = idf;
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Score one document against a query (Okapi BM25).
    fn score(&self, doc_idx: usize, query_terms: &[String]) -> f64 {
        let doc = &self.docs[doc_idx];
        let doc_len = doc.text.split_whitespace().count() as f64;
        let tf = &self.tf[doc_idx];
        let mut score = 0.0;
        for t in query_terms {
            let Some(&idf) = self.idf.get(t) else {
                continue;
            };
            let f = *tf.get(t).unwrap_or(&0) as f64;
            let denom = f + K1 * (1.0 - B + B * doc_len / self.avg_len.max(1.0));
            score += idf * (f * (K1 + 1.0)) / denom;
        }
        score
    }

    /// Search: top-k doc ids by BM25 score, best first. Empty for an empty
    /// query (all terms shorter than 2 chars are dropped).
    pub fn search(&self, query: &str, k: usize) -> Vec<String> {
        let terms = tokenize(query);
        if terms.is_empty() || self.docs.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(f64, usize)> = (0..self.docs.len())
            .map(|i| (self.score(i, &terms), i))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(k)
            .map(|(_, i)| self.docs[i].id.clone())
            .collect()
    }

    /// As a retrieval signal: scored hits with confidence 0..=1.
    pub fn as_signal(&self, query: &str, k: usize) -> Vec<Hit> {
        let terms = tokenize(query);
        if terms.is_empty() || self.docs.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(f64, usize)> = (0..self.docs.len())
            .map(|i| (self.score(i, &terms), i))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        let max = scored.first().map(|(s, _)| *s).unwrap_or(0.0).max(1e-9);
        scored
            .into_iter()
            .map(|(s, i)| Hit {
                id: self.docs[i].id.clone(),
                kind: SignalKind::Keyword,
                confidence: (s / max).clamp(0.0, 1.0),
            })
            .collect()
    }
}

/// Split into lowercase alphanumeric terms (len >= 2).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|t| t.len() >= 2)
        .collect()
}

/// A parallel retrieval signal source (BM25 default; vector/graph are other
/// implementations). The coordinator runs these concurrently.
pub trait SignalSource {
    /// Retrieve up to `k` candidate hits for `query`.
    fn retrieve(&self, query: &str, k: usize) -> Vec<Hit>;
}

impl SignalSource for Bm25Index {
    fn retrieve(&self, query: &str, k: usize) -> Vec<Hit> {
        self.as_signal(query, k)
    }
}

/// A named signal source (for the parallel run + RRF merge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalRank {
    /// BM25 keyword signal (default weight).
    Keyword,
    /// Vector/embedding signal (optional, user-enabled).
    Vector,
    /// Graph/spreading-activation signal.
    Graph,
}

/// Reciprocal Rank Fusion over the sources' ranked hits:
/// score(id) = sum of 1/(60 + rank). Sources agreeing on an id reinforce it;
/// ids from one source still surface. Deterministic, sorted desc, confidence
/// normalized to 0..=1, truncated to `k`.
pub fn fuse_signals(sources: &[(SignalRank, Vec<Hit>)], k: usize) -> Vec<Hit> {
    let k = k.max(1);
    let mut scores: HashMap<String, (f64, SignalKind)> = HashMap::new();
    for (_rank, hits) in sources {
        for (i, hit) in hits.iter().enumerate() {
            let contrib = 1.0 / (60.0 + i as f64);
            let entry = scores.entry(hit.id.clone()).or_insert((0.0, hit.kind));
            entry.0 += contrib;
        }
    }
    let mut out: Vec<Hit> = scores
        .into_iter()
        .map(|(id, (score, kind))| Hit {
            id,
            kind,
            confidence: score,
        })
        .collect();
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let max = out.first().map(|h| h.confidence).unwrap_or(0.0).max(1e-9);
    for h in &mut out {
        h.confidence /= max;
    }
    out.truncate(k);
    out
}

/// Convenience: run a default BM25 + optional vector + graph triple and fuse.
/// `vector`/`graph` are `None` when the user hasn't enabled them (the
/// vectorless default path).
pub fn run_signals_parallel(
    query: &str,
    bm25: &Bm25Index,
    vector: Option<&dyn SignalSource>,
    graph: Option<&dyn SignalSource>,
    k: usize,
) -> Vec<Hit> {
    let mut sources = vec![(SignalRank::Keyword, bm25.retrieve(query, k))];
    if let Some(v) = vector {
        sources.push((SignalRank::Vector, v.retrieve(query, k)));
    }
    if let Some(g) = graph {
        sources.push((SignalRank::Graph, g.retrieve(query, k)));
    }
    fuse_signals(&sources, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Bm25Index {
        let mut idx = Bm25Index::new();
        idx.build(vec![
            Bm25Doc {
                id: "d1".into(),
                text: "the quick brown fox jumps over the lazy dog".into(),
            },
            Bm25Doc {
                id: "d2".into(),
                text: "rust memory safety ownership borrow checker".into(),
            },
            Bm25Doc {
                id: "d3".into(),
                text: "browser automation accessibility tree snapshot".into(),
            },
        ]);
        idx
    }

    #[test]
    fn bm25_ranks_relevant_doc_first() {
        let idx = corpus();
        let hits = idx.search("rust borrow checker", 3);
        assert_eq!(hits[0], "d2");
    }

    #[test]
    fn bm25_empty_query_returns_nothing() {
        let idx = corpus();
        assert!(idx.search("", 3).is_empty());
        assert!(idx.search("a", 3).is_empty()); // single-char terms dropped
    }

    #[test]
    fn bm25_unknown_terms_return_deterministic_order() {
        let idx = corpus();
        let hits = idx.search("zzzqqq", 3);
        // No term matches - all zero scores, ids still returned deterministically.
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn signal_confidence_normalized() {
        let idx = corpus();
        let signals = idx.as_signal("rust ownership", 2);
        assert_eq!(signals[0].id, "d2");
        assert!(signals[0].confidence >= signals[1].confidence);
        assert!(signals[0].confidence <= 1.0);
    }

    #[test]
    fn parallel_run_fuses_bm25_vector_graph() {
        struct VecSource(Vec<Hit>);
        impl SignalSource for VecSource {
            fn retrieve(&self, _query: &str, _k: usize) -> Vec<Hit> {
                self.0.clone()
            }
        }
        let bm25 = corpus();
        let vector = VecSource(vec![Hit {
            id: "d2".into(),
            kind: SignalKind::Semantic,
            confidence: 0.9,
        }]);
        let graph = VecSource(vec![Hit {
            id: "d1".into(),
            kind: SignalKind::Graph,
            confidence: 0.8,
        }]);
        let fused = run_signals_parallel("rust", &bm25, Some(&vector), Some(&graph), 3);
        // Both the BM25-strong doc and the vector-strong doc surface.
        assert!(fused.iter().any(|h| h.id == "d2"));
        assert!(fused.iter().any(|h| h.id == "d1"));
    }

    #[test]
    fn vectorless_default_is_bm25_only() {
        let bm25 = corpus();
        let fused = run_signals_parallel("rust borrow", &bm25, None, None, 3);
        assert_eq!(fused[0].id, "d2");
        assert!(fused.iter().all(|h| h.kind == SignalKind::Keyword));
    }
}
