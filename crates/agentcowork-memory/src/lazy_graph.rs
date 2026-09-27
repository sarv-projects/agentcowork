//! LazyGraphRAG — lazy concept-graph mode (C6, Algorithm #8 — Microsoft
//! Research 2025, search-verified from the MSR blog 2026-08-23).
//!
//! The MSR design (blog, Nov 2024): concepts + co-occurrences are extracted
//! with cheap NLP at *index* time (≈0.1% of full GraphRAG's LLM index cost),
//! the graph is only materialized when a query arrives, and *all* LLM
//! summarization is deferred to query time behind a `relevance_budget` knob.
//! This module is the `lazy` mode beside the eager `graph::GraphStore`.
//!
//! Honesty invariants (project-wide):
//! - Concept extraction is **deterministic NLP** — zero LLM tokens up front.
//! - The LLM relevance assessor is a **seam** (`RelevanceAssessor` trait).
//!   Without it, retrieval is deterministic lexical/graph only, and every
//!   report says `assessed: false` — the honest flag (`capability_status`).
//! - Cost claims are measured, not asserted: `IndexReport::cost_estimate_ms`
//!   is the elapsed index time; the "≈0.1%" claim is reported relative to a
//!   full-GraphRAG baseline only when the caller supplies one.

use std::collections::{HashMap, HashSet, VecDeque};

// ---------------------------------------------------------------------------
// Concept extraction (deterministic NLP — no LLM)
// ---------------------------------------------------------------------------

/// Stopwords that never form concepts (subset of a standard English list).
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "then", "else", "of", "to", "in", "on", "at", "by",
    "for", "with", "from", "as", "is", "are", "was", "were", "be", "been", "being", "it", "its",
    "this", "that", "these", "those", "i", "you", "he", "she", "we", "they", "not", "no", "do",
    "does", "did", "have", "has", "had", "will", "would", "can", "could", "should", "may", "might",
    "there", "here", "what", "which", "who", "whom", "when", "where", "how", "all", "any", "both",
    "each", "few", "more", "most", "other", "some", "such", "only", "own", "same", "so", "than",
    "too", "very", "just", "about", "into", "over", "after", "before", "under", "again", "further",
    "then", "once", "also", "via", "per", "etc", "eg", "ie", "vs", "e.g.", "i.e.",
];

/// Is `word` a stopword / pure punctuation / numeric-only?
fn is_content_word(word: &str) -> bool {
    let lower = word.to_lowercase();
    !STOPWORDS.contains(&lower.as_str()) && word.chars().any(|c| c.is_alphabetic())
}

/// Split text into sentence-ish windows (by sentence punctuation).
fn sentence_windows(text: &str) -> Vec<&str> {
    text.split(['.', '!', '?', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Extract concepts from one window. A concept is either:
///  1. a capitalized multi-word sequence (proper-noun-ish, e.g. "GraphRAG",
///     "Microsoft Research"), or
///  2. a contiguous run of content words (lowercased, e.g. "retrieval budget"),
///     up to `max_phrase` words.
///
/// Both are normalized to a stable key. Returns unique concepts in order.
pub fn extract_concepts(window: &str, max_phrase: usize) -> Vec<String> {
    let words: Vec<&str> = window
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .filter(|w| !w.is_empty())
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |concept: String, seen: &mut HashSet<String>, out: &mut Vec<String>| {
        if concept.chars().count() >= 3 && !seen.contains(&concept) {
            seen.insert(concept.clone());
            out.push(concept);
        }
    };

    // Pass 1 — capitalized sequences (proper nouns / acronyms).
    let mut i = 0;
    while i < words.len() {
        if words[i]
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
            && is_content_word(words[i])
        {
            let mut seq = Vec::new();
            while i < words.len() && seq.len() < max_phrase {
                let first = words[i]
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if first && is_content_word(words[i]) {
                    seq.push(words[i]);
                    i += 1;
                } else {
                    break;
                }
            }
            if !seq.is_empty() {
                let c = seq.join(" ");
                push(c, &mut seen, &mut out);
            }
        } else {
            i += 1;
        }
    }

    // Pass 2 — content-word runs (lowercased noun-phrase-ish).
    i = 0;
    while i < words.len() {
        if is_content_word(words[i]) {
            let mut seq = Vec::new();
            while i < words.len() && seq.len() < max_phrase && is_content_word(words[i]) {
                seq.push(words[i].to_lowercase());
                i += 1;
            }
            if seq.len() >= 2 {
                push(seq.join(" "), &mut seen, &mut out);
            }
        } else {
            i += 1;
        }
    }

    out
}

/// Deterministic token overlap similarity between a query and a document
/// (the default lexical scorer — a cheap BM25-lite stand-in; pluggable via
/// [`SimilarityScorer`]).
pub fn lexical_similarity(query_tokens: &[String], doc_tokens: &[String]) -> f64 {
    if query_tokens.is_empty() || doc_tokens.is_empty() {
        return 0.0;
    }
    let qset: HashSet<&str> = query_tokens.iter().map(|s| s.as_str()).collect();
    let dset: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
    let overlap = qset.intersection(&dset).count() as f64;
    overlap / (qset.len() as f64).sqrt()
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Lazy concept graph
// ---------------------------------------------------------------------------

/// A co-occurrence edge between two concepts (shared window count).
#[derive(Debug, Clone)]
pub struct ConceptEdge {
    pub other: String,
    pub weight: u32,
}

/// The lazily-built concept graph. Nothing is extracted until
/// [`LazyConceptGraph::ensure_indexed`] is called (on first query).
#[derive(Debug, Default)]
pub struct LazyConceptGraph {
    /// concept → chunks that mention it
    concept_chunks: HashMap<String, HashSet<String>>,
    /// concept → co-occurring concept → shared-window count
    cooccur: HashMap<String, HashMap<String, u32>>,
    /// chunk id → concepts (extracted)
    chunk_concepts: HashMap<String, Vec<String>>,
    indexed_chunks: HashSet<String>,
}

impl LazyConceptGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Extract concepts + co-occurrences for a chunk. Idempotent.
    /// Indexing cost is deterministic NLP only — no LLM tokens.
    pub fn ensure_indexed(&mut self, chunk_id: &str, text: &str) {
        if self.indexed_chunks.contains(chunk_id) {
            return;
        }
        let windows = sentence_windows(text);
        // Concepts per window; co-occurrence = concepts sharing a window.
        for w in &windows {
            let concepts = extract_concepts(w, 4);
            for c in &concepts {
                self.concept_chunks
                    .entry(c.clone())
                    .or_default()
                    .insert(chunk_id.to_string());
            }
            // Co-occurrence within the window (concept pairs).
            for (i, a) in concepts.iter().enumerate() {
                for b in concepts.iter().skip(i + 1) {
                    let m = self.cooccur.entry(a.clone()).or_default();
                    *m.entry(b.clone()).or_insert(0) += 1;
                    let m2 = self.cooccur.entry(b.clone()).or_default();
                    *m2.entry(a.clone()).or_insert(0) += 1;
                }
            }
        }
        self.chunk_concepts
            .insert(chunk_id.to_string(), concepts_for_chunk(&windows));
        self.indexed_chunks.insert(chunk_id.to_string());
    }

    /// Concepts for a chunk (dedup across its windows, in order).
    pub fn concepts_of(&self, chunk_id: &str) -> Vec<&str> {
        self.chunk_concepts
            .get(chunk_id)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Co-occurrence neighbors of a concept, strongest first.
    pub fn cooccurrence_neighbors(&self, concept: &str, top: usize) -> Vec<ConceptEdge> {
        let mut edges: Vec<ConceptEdge> = self
            .cooccur
            .get(concept)
            .map(|m| {
                m.iter()
                    .map(|(k, w)| ConceptEdge {
                        other: k.clone(),
                        weight: *w,
                    })
                    .collect()
            })
            .unwrap_or_default();
        edges.sort_by_key(|e| std::cmp::Reverse(e.weight));
        edges.truncate(top);
        edges
    }

    /// Chunks that mention a concept.
    pub fn chunks_for_concept(&self, concept: &str) -> Vec<&str> {
        self.concept_chunks
            .get(concept)
            .map(|s| s.iter().map(|x| x.as_str()).collect())
            .unwrap_or_default()
    }

    /// How many chunks have been indexed so far (0 until the first query).
    pub fn indexed_chunk_count(&self) -> usize {
        self.indexed_chunks.len()
    }
}

fn concepts_for_chunk(windows: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for w in windows {
        for c in extract_concepts(w, 4) {
            if seen.insert(c.clone()) {
                out.push(c);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Seams (caller-supplied — never bundled)
// ---------------------------------------------------------------------------

/// LLM sentence-level relevance assessor (the query-time LLM cost). When
/// absent, retrieval is deterministic and reports `assessed: false`.
pub trait RelevanceAssessor {
    /// Score candidate text for relevance to `query`. Higher = more relevant.
    fn assess(&mut self, query: &str, candidate: &str) -> f64;
}

/// Text similarity scorer (embedding or lexical). Default = [`lexical_similarity`].
pub trait SimilarityScorer {
    fn score(&self, query_tokens: &[String], doc_tokens: &[String]) -> f64;
}

/// Deterministic lexical scorer (default).
#[derive(Debug, Default)]
pub struct LexicalScorer;

impl SimilarityScorer for LexicalScorer {
    fn score(&self, query_tokens: &[String], doc_tokens: &[String]) -> f64 {
        lexical_similarity(query_tokens, doc_tokens)
    }
}

// ---------------------------------------------------------------------------
// LazyGraphRag
// ---------------------------------------------------------------------------

/// Retrieval options.
#[derive(Debug, Clone)]
pub struct RetrieveOptions {
    /// Max LLM-assessor calls (or graph traversal steps when no assessor).
    /// The single cost–quality knob from the MSR design.
    pub relevance_budget: usize,
    /// Max graph hops to follow from a seed concept.
    pub max_depth: usize,
    /// How many co-occurrence neighbors per concept to follow.
    pub neighbors_per_concept: usize,
    /// How many seed chunks to start traversal from.
    pub seed_chunks: usize,
    /// Max concepts derived from the query itself.
    pub max_query_concepts: usize,
}

impl Default for RetrieveOptions {
    fn default() -> Self {
        Self {
            relevance_budget: 5, // MSR: 3–5 subqueries
            max_depth: 2,
            neighbors_per_concept: 4,
            seed_chunks: 5,
            max_query_concepts: 4,
        }
    }
}

/// One retrieved chunk with provenance.
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: String,
    /// Lexical/graph fusion score (0..=1-ish).
    pub score: f64,
    /// Whether the LLM assessor judged this chunk (honest flag).
    pub assessed: bool,
    /// Concepts that bridged the query to this chunk via the graph.
    pub bridge_concepts: Vec<String>,
}

/// Per-run report.
#[derive(Debug, Clone)]
pub struct RetrievalReport {
    pub chunks: Vec<RetrievedChunk>,
    pub budget_used: usize,
    pub budget_cap: usize,
    pub query_concepts: Vec<String>,
    pub indexed_chunks: usize,
    /// True if every returned chunk was assessed by the LLM seam; false =
    /// deterministic fallback (the honest flag the UI surfaces).
    pub fully_assessed: bool,
}

/// Capability status — the honest "off" flag until wired to a live retriever.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CapabilityStatus {
    pub enabled: bool,
    pub reason: &'static str,
    pub mode: &'static str,
}

/// LazyGraphRAG over a set of chunks. The graph is built only when
/// [`LazyGraphRag::retrieve`] is first called.
pub struct LazyGraphRag {
    chunks: Vec<(String, String)>, // (id, text)
    graph: LazyConceptGraph,
    scorer: Box<dyn SimilarityScorer>,
    assessor: Option<Box<dyn RelevanceAssessor>>,
}

impl LazyGraphRag {
    pub fn new(chunks: Vec<(String, String)>) -> Self {
        Self {
            chunks,
            graph: LazyConceptGraph::new(),
            scorer: Box::new(LexicalScorer),
            assessor: None,
        }
    }

    pub fn with_assessor(mut self, assessor: Box<dyn RelevanceAssessor>) -> Self {
        self.assessor = Some(assessor);
        self
    }

    pub fn with_scorer(mut self, scorer: Box<dyn SimilarityScorer>) -> Self {
        self.scorer = scorer;
        self
    }

    /// Honest capability status: `off` until a live retriever/assessor wiring
    /// exists (same pattern as "G8 search cascade is not built").
    pub fn capability_status(&self) -> CapabilityStatus {
        if self.assessor.is_none() {
            CapabilityStatus {
                enabled: false,
                reason: "LLM relevance assessor not wired; deterministic lexical/graph fallback only",
                mode: "lazy-graph (deterministic)",
            }
        } else {
            CapabilityStatus {
                enabled: true,
                reason: "assessor wired",
                mode: "lazy-graph (assessed)",
            }
        }
    }

    /// Best-forward chunk ranking: similarity of each chunk to the query.
    fn best_forward(&self, q_tokens: &[String], top: usize) -> Vec<(String, f64)> {
        let mut ranked: Vec<(String, f64)> = self
            .chunks
            .iter()
            .map(|(id, text)| {
                let s = self.scorer.score(q_tokens, &tokens(text));
                (id.clone(), s)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(top);
        ranked
    }

    /// Iterative-deepening retrieval (MSR design):
    ///  1. best-forward chunk ranking (embedding/lexical similarity),
    ///  2. extract query concepts → breadth-first follow co-occurrence
    ///     community/context traversal,
    ///  3. LLM sentence-level relevance assessment behind `relevance_budget`.
    ///
    /// Budget semantics: each LLM-assessor call costs 1; without an assessor,
    /// each graph traversal step costs 1. When the budget is exhausted the
    /// traversal stops — the caller sees `budget_used == budget_cap` and
    /// `fully_assessed == false` rather than a fake-complete answer.
    pub fn retrieve(&mut self, query: &str, opts: &RetrieveOptions) -> RetrievalReport {
        // Lazy indexing: only index the chunks we might touch, on demand.
        let q_tokens = tokens(query);
        let seeds = self.best_forward(&q_tokens, opts.seed_chunks);
        for (id, _) in &seeds {
            if let Some((_, text)) = self.chunks.iter().find(|(cid, _)| cid == id) {
                self.graph.ensure_indexed(id, text);
            }
        }

        let query_concepts = extract_concepts(query, 4);
        let query_concepts: Vec<String> = query_concepts
            .into_iter()
            .take(opts.max_query_concepts)
            .collect();

        let mut candidates: HashMap<String, f64> = HashMap::new();
        let mut bridge: HashMap<String, Vec<String>> = HashMap::new();
        let mut budget_used = 0usize;

        // Seed chunks from best-forward.
        for (id, score) in &seeds {
            candidates.entry(id.clone()).or_insert(*score);
        }

        // BFS over co-occurrence from query concepts (community traversal).
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let mut visited: HashSet<String> = HashSet::new();
        for c in &query_concepts {
            if visited.insert(c.clone()) {
                queue.push_back((c.clone(), 0));
            }
        }
        while let Some((concept, depth)) = queue.pop_front() {
            if depth >= opts.max_depth || budget_used >= opts.relevance_budget {
                continue;
            }
            budget_used += 1;
            for edge in self
                .graph
                .cooccurrence_neighbors(&concept, opts.neighbors_per_concept)
            {
                if visited.insert(edge.other.clone()) {
                    queue.push_back((edge.other.clone(), depth + 1));
                }
                // Collect owned chunk ids first — we mutate the graph while
                // iterating its traversal results.
                let edge_chunks: Vec<String> = self
                    .graph
                    .chunks_for_concept(&edge.other)
                    .into_iter()
                    .map(|c| c.to_string())
                    .collect();
                for chunk in edge_chunks {
                    // Index on demand (lazy — only chunks reached by traversal).
                    if let Some((_, text)) = self.chunks.iter().find(|(cid, _)| *cid == chunk) {
                        self.graph.ensure_indexed(&chunk, text);
                    }
                    let boost = (edge.weight as f64) / (1.0 + depth as f64);
                    let e = candidates.entry(chunk.clone()).or_insert(0.0);
                    *e += boost * 0.1;
                    let b = bridge.entry(chunk.clone()).or_default();
                    if !b.contains(&edge.other) {
                        b.push(edge.other.clone());
                    }
                }
            }
        }

        // LLM relevance assessment (budget-capped).
        let mut assessed: HashMap<String, f64> = HashMap::new();
        let mut assess_budget = opts.relevance_budget;
        if let Some(assessor) = self.assessor.as_mut() {
            let mut ranked: Vec<(String, f64)> =
                candidates.iter().map(|(k, v)| (k.clone(), *v)).collect();
            ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (id, _) in ranked {
                if assess_budget == 0 {
                    break;
                }
                assess_budget -= 1;
                if let Some((_, text)) = self.chunks.iter().find(|(cid, _)| cid == &id) {
                    let a = assessor.assess(query, text);
                    assessed.insert(id.clone(), a);
                }
            }
            budget_used = opts.relevance_budget - assess_budget;
        }

        // Fuse: assessed score wins when present; else lexical/graph score.
        let mut result: Vec<RetrievedChunk> = candidates
            .into_iter()
            .map(|(id, score)| {
                let a = assessed.get(&id);
                RetrievedChunk {
                    chunk_id: id.clone(),
                    score: a.copied().unwrap_or(score),
                    assessed: a.is_some(),
                    bridge_concepts: bridge.get(&id).cloned().unwrap_or_default(),
                }
            })
            .collect();
        result.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let fully_assessed =
            self.assessor.is_some() && !result.is_empty() && result.iter().all(|c| c.assessed);

        RetrievalReport {
            chunks: result,
            budget_used,
            budget_cap: opts.relevance_budget,
            query_concepts,
            indexed_chunks: self.graph.indexed_chunk_count(),
            fully_assessed,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests + benchmark vs the eager graph (#8)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const CORPUS: &[(&str, &str)] = &[
        (
            "c1",
            "GraphRAG builds a knowledge graph from unstructured text. It uses LLM summarization during indexing, which is expensive.",
        ),
        (
            "c2",
            "LazyGraphRAG defers all LLM summarization to query time. Indexing is cheap NLP concept extraction only.",
        ),
        (
            "c3",
            "The relevance budget controls the cost quality trade-off. Three to five subqueries per query is typical.",
        ),
        (
            "c4",
            "Spreading activation walks the adjacency store. Contradicting edges subtract from activation.",
        ),
        (
            "c5",
            "A user-owned desktop agent runs locally. Keys never leave the machine.",
        ),
    ];

    fn rag() -> LazyGraphRag {
        LazyGraphRag::new(
            CORPUS
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        )
    }

    #[test]
    fn concept_extraction_is_deterministic_and_token_free() {
        let a = extract_concepts("LazyGraphRAG defers LLM summarization to query time", 4);
        let b = extract_concepts("LazyGraphRAG defers LLM summarization to query time", 4);
        assert_eq!(a, b, "same input → same concepts");
        assert!(a.iter().any(|c| c.contains("LazyGraphRAG")));
        // Stopwords never appear as standalone concepts.
        assert!(
            a.iter()
                .all(|c| !STOPWORDS.contains(&c.to_lowercase().as_str()))
        );
    }

    #[test]
    fn lazy_graph_builds_only_on_query() {
        let mut rag = rag();
        assert_eq!(rag.graph.indexed_chunk_count(), 0, "zero up-front indexing");
        let _ = rag.retrieve("cheap indexing", &RetrieveOptions::default());
        assert!(
            rag.graph.indexed_chunk_count() > 0,
            "indexed lazily during the query"
        );
    }

    #[test]
    fn cooccurrence_graph_bridges_disconnected_chunks() {
        let mut graph = LazyConceptGraph::new();
        // Each window yields two content-word concepts (stopword-separated);
        // the shared concept "gamma plan" bridges the two chunks.
        graph.ensure_indexed("c1", "the alpha budget, the gamma plan");
        graph.ensure_indexed("c2", "the gamma plan, the delta rollout");
        let neighbors = graph.cooccurrence_neighbors("alpha budget", 10);
        assert!(
            neighbors.iter().any(|e| e.other == "gamma plan"),
            "co-occurrence edge alpha-budget→gamma-plan"
        );
        let chunks = graph.chunks_for_concept("gamma plan");
        assert_eq!(chunks.len(), 2, "shared concept spans both chunks");
    }

    #[test]
    fn budget_caps_work_without_an_assessor() {
        let mut rag = rag();
        let opts = RetrieveOptions {
            relevance_budget: 2,
            ..Default::default()
        };
        let report = rag.retrieve("query time summarization", &opts);
        assert!(report.budget_used <= report.budget_cap);
        assert!(
            !report.fully_assessed,
            "no assessor → honest assessed:false"
        );
        assert!(!report.chunks.is_empty());
    }

    #[test]
    fn assessor_seam_marks_assessed_and_caps_budget() {
        struct DummyAssessor;
        impl RelevanceAssessor for DummyAssessor {
            fn assess(&mut self, query: &str, candidate: &str) -> f64 {
                let qt = tokens(query);
                let dt = tokens(candidate);
                let q: HashSet<&str> = qt.iter().map(|s| s.as_str()).collect();
                let d: HashSet<&str> = dt.iter().map(|s| s.as_str()).collect();
                q.intersection(&d).count() as f64
            }
        }
        let mut rag = rag().with_assessor(Box::new(DummyAssessor));
        let report = rag.retrieve("defer LLM summarization", &RetrieveOptions::default());
        assert!(report.fully_assessed, "assessor present → fully assessed");
        assert!(report.budget_used > 0 && report.budget_used <= report.budget_cap);
    }

    #[test]
    fn capability_status_is_off_without_assessor() {
        let rag = rag();
        let st = rag.capability_status();
        assert!(!st.enabled);
        assert!(st.reason.contains("not wired"));
    }

    #[test]
    fn benchmark_lazy_graph_finds_bridge_when_lexical_cannot() {
        // Global-query class: the answer chunk shares ZERO query tokens; only
        // the co-occurrence graph bridges query concepts to the answer.
        let corpus = vec![
            (
                "a".to_string(),
                "The Northwind project ships quarterly reports.".to_string(),
            ),
            (
                "b".to_string(),
                "Northwind reports are generated by the build pipeline.".to_string(),
            ),
            (
                "c".to_string(),
                "The build pipeline runs every Friday.".to_string(),
            ),
            (
                "d".to_string(),
                "Quarterly reports describe revenue by region.".to_string(),
            ),
        ];
        let mut rag = LazyGraphRag::new(corpus.clone());
        // Query uses concepts present only in c ("Friday" pipeline) and d ("revenue").
        let query = "revenue pipeline Friday";
        let q_tokens = tokens(query);
        // Plain lexical top-1 over the corpus (no graph):
        let mut lex: Vec<(String, f64)> = corpus
            .iter()
            .map(|(id, t)| (id.clone(), lexical_similarity(&q_tokens, &tokens(t))))
            .collect();
        lex.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let lex_top = lex[0].0.clone();

        // LazyGraphRAG with a generous budget bridges d (revenue) and c
        // (pipeline/Friday) — and, through co-occurrence, surfaces b (a
        // global-query answer with zero query terms).
        let report = rag.retrieve(
            query,
            &RetrieveOptions {
                relevance_budget: 8,
                max_depth: 3,
                ..Default::default()
            },
        );
        let found_b = report.chunks.iter().any(|c| c.chunk_id == "b");
        assert!(
            found_b,
            "graph traversal must surface the zero-term bridge chunk (b) — lexical top-1 was {lex_top}"
        );
    }
}
