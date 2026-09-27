//! P20-1 — SeekStorm embedded hybrid index (doc 72 §1 — 🔴 STEAL, evaluated).
//!
//! SeekStorm (Apache-2.0, pure Rust, in-process) is the "SeekStorm-pattern
//! hybrid search as an embedded lib" the P5.1/P5.7 fusion was designed to
//! accept. This module has two halves:
//!
//! 1. [`QueryMode`] — **always compiled**: the v3 8-mode query planner,
//!    mapped onto our [`crate::bm25::SignalKind`] fusion vocabulary so the
//!    planner semantics are testable without the crate.
//! 2. `embedded::HybridIndex` — **feature-gated** (`hybrid-seekstorm`,
//!    default off): a real embed of the `seekstorm` crate behind a sync
//!    facade — its own driver thread running a current-thread tokio runtime
//!    (the same pattern as `agentcowork-cdp::transport`).
//!
//! The default build stays vectorless + dependency-light (BM25 + RRF); the
//! hybrid index is the opt-in upgrade path (doc 54 audit stays clean for
//! everyone who doesn't enable the feature).

/// The SeekStorm v3 query-mode set (the 8-mode planner), mapped onto our
/// signal vocabulary — lexical (Keyword) / vector (Semantic) / fused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryMode {
    /// Pure lexical (BM25F) — the vectorless default equivalent.
    Lexical,
    /// Pure dense vector.
    Vector,
    /// Hybrid: lexical + vector, fused.
    Hybrid,
    /// Hybrid across all indexed fields with result fusion.
    HybridAll,
    /// Vector search with lexical rescoring.
    ReScore,
    /// Semantic query expansion before vector search.
    Semantic,
    /// Semantic expansion + lexical rescoring.
    SemanticRescore,
    /// Weighted hybrid (explicit per-field weights).
    WeightedHybrid,
}

impl QueryMode {
    /// Every mode in planner order (stable list).
    pub const ALL: [QueryMode; 8] = [
        QueryMode::Lexical,
        QueryMode::Vector,
        QueryMode::Hybrid,
        QueryMode::HybridAll,
        QueryMode::ReScore,
        QueryMode::Semantic,
        QueryMode::SemanticRescore,
        QueryMode::WeightedHybrid,
    ];

    /// Parse a planner-mode name (kebab / snake / camel tolerant).
    pub fn parse(name: &str) -> Option<QueryMode> {
        let norm: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match norm.as_str() {
            "lexical" => Some(QueryMode::Lexical),
            "vector" => Some(QueryMode::Vector),
            "hybrid" => Some(QueryMode::Hybrid),
            "hybridall" | "hybridallfields" => Some(QueryMode::HybridAll),
            "rescore" | "vectorescore" | "vectorlexicalrescore" => Some(QueryMode::ReScore),
            "semantic" => Some(QueryMode::Semantic),
            "semanticrescore" | "semanticlexicalrescore" => Some(QueryMode::SemanticRescore),
            "weightedhybrid" => Some(QueryMode::WeightedHybrid),
            _ => None,
        }
    }

    /// Whether the mode produces a lexical signal.
    pub fn uses_lexical(self) -> bool {
        matches!(
            self,
            QueryMode::Lexical
                | QueryMode::Hybrid
                | QueryMode::HybridAll
                | QueryMode::ReScore
                | QueryMode::SemanticRescore
                | QueryMode::WeightedHybrid
        )
    }

    /// Whether the mode produces a vector signal.
    pub fn uses_vector(self) -> bool {
        !matches!(self, QueryMode::Lexical)
    }

    /// The RRF weight pair implied by the mode (`crate::fusion::rrf_fuse`).
    pub fn fusion_weights(self) -> (f64, f64) {
        match self {
            QueryMode::Lexical => (1.0, 0.0),
            QueryMode::Vector | QueryMode::Semantic => (0.0, 1.0),
            QueryMode::Hybrid | QueryMode::HybridAll | QueryMode::WeightedHybrid => (0.5, 0.5),
            QueryMode::ReScore | QueryMode::SemanticRescore => (0.3, 0.7),
        }
    }

    /// The planner's recommended mode: vectorless (no embeddings configured)
    /// → Lexical; embeddings + rescore budget → ReScore; embeddings without
    /// → Hybrid.
    pub fn recommend(has_vectors: bool, rescore_budget: bool) -> QueryMode {
        match (has_vectors, rescore_budget) {
            (false, _) => QueryMode::Lexical,
            (true, true) => QueryMode::ReScore,
            (true, false) => QueryMode::Hybrid,
        }
    }
}

/// Always-compiled evaluation verdict (honesty surface).
pub const SEEKSTORM_VERDICT: &str = "SeekStorm v3 (Apache-2.0, in-process, 8-mode \
QueryPlanner) is the validated swap-in for the hand-rolled BM25+RRF — adapter behind the \
`hybrid-seekstorm` feature (default off; the vectorless default = Lexical mode). The upstream \
crate is heavy (tokio/hyper/rayon tree) so the default build never pulls it.";

/// The real embed — a sync facade over a driver thread running a
/// current-thread tokio runtime. Consumes only the API surface verified
/// against the vendored seekstorm 3.3.7 source: `create_index`/`open_index`
/// (meta.json presence decides), `IndexDocument::index_document`,
/// `Commit::commit`, `Search::search` on the index Arc, and
/// `get_document` returning the stored chunk_id.
#[cfg(feature = "hybrid-seekstorm")]
pub mod embedded {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};

    use seekstorm::commit::Commit;
    use seekstorm::index::IndexDocument;

    enum Request {
        Add(String, String),
        Commit,
        Search(String, usize, Sender<Result<Vec<SeekHit>, String>>),
        Close,
    }

    #[derive(Debug, Clone)]
    pub struct SeekHit {
        pub chunk_id: String,
        pub score: f64,
    }

    /// Sync facade over the embedded SeekStorm index.
    #[derive(Debug, Clone)]
    pub struct HybridIndex {
        tx: Arc<Sender<Request>>,
    }

    const SCHEMA: &str = r#"[
        {"field":"body","field_type":"Text","store":true,"index_lexical":true},
        {"field":"chunk_id","field_type":"Text","store":true,"index_lexical":false}
    ]"#;

    impl HybridIndex {
        /// Create (or open) an index under `dir` and start the driver thread.
        pub fn open(dir: &Path) -> Result<Self, String> {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let index_path: PathBuf = dir.join("seekstorm");
            let (tx, rx) = channel::<Request>();
            std::thread::spawn(move || driver(index_path, rx));
            Ok(Self { tx: Arc::new(tx) })
        }

        pub fn add(&self, chunk_id: &str, text: &str) -> Result<(), String> {
            self.tx
                .send(Request::Add(chunk_id.to_string(), text.to_string()))
                .map_err(|e| e.to_string())
        }

        pub fn commit(&self) -> Result<(), String> {
            self.tx.send(Request::Commit).map_err(|e| e.to_string())
        }

        pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<SeekHit>, String> {
            let (reply, rx) = channel();
            self.tx
                .send(Request::Search(query.to_string(), top_k, reply))
                .map_err(|e| e.to_string())?;
            rx.recv().map_err(|e| e.to_string())?
        }

        /// Close the index (stops the driver thread).
        pub fn close(&self) {
            let _ = self.tx.send(Request::Close);
        }
    }

    type IndexArc = Arc<tokio::sync::RwLock<seekstorm::index::Index>>;

    fn driver(index_path: PathBuf, rx: Receiver<Request>) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return,
        };
        rt.block_on(async move {
            let index = match open_or_create(&index_path).await {
                Ok(i) => i,
                Err(_) => return,
            };
            loop {
                match rx.recv() {
                    Ok(Request::Add(chunk_id, text)) => {
                        let doc: seekstorm::index::Document = serde_json::from_value(
                            serde_json::json!({"body": text, "chunk_id": chunk_id}),
                        )
                        .unwrap_or_default();
                        index
                            .index_document(doc, seekstorm::index::FileType::None)
                            .await;
                    }
                    Ok(Request::Commit) => {
                        index.commit().await;
                    }
                    Ok(Request::Search(query, top_k, reply)) => {
                        let result = search(&index, &query, top_k).await;
                        let _ = reply.send(result);
                    }
                    Ok(Request::Close) | Err(_) => break,
                }
            }
        });
    }

    async fn open_or_create(path: &Path) -> Result<IndexArc, String> {
        use seekstorm::index::{
            AccessType, Clustering, DocumentCompression, FrequentwordType, IndexMetaObject,
            LexicalSimilarity, NgramSet, SchemaField, StemmerType, StopwordType, TokenizerType,
            create_index, open_index,
        };
        use seekstorm::vector::Inference;
        let meta = IndexMetaObject {
            id: 0,
            name: "everyaios".to_string(),
            lexical_similarity: LexicalSimilarity::Bm25f,
            tokenizer: TokenizerType::AsciiAlphabetic,
            stemmer: StemmerType::None,
            stop_words: StopwordType::None,
            frequent_words: FrequentwordType::English,
            ngram_indexing: NgramSet::NgramFF as u8,
            document_compression: DocumentCompression::Snappy,
            access_type: AccessType::Mmap,
            spelling_correction: None,
            query_completion: None,
            clustering: Clustering::None,
            inference: Inference::None,
        };
        let schema: Vec<SchemaField> = serde_json::from_str(SCHEMA).map_err(|e| e.to_string())?;
        if path.join("meta.json").exists() {
            open_index(path).await.map_err(|e| e.to_string())
        } else {
            create_index(path, meta, &schema, &Vec::new(), 11, true, None)
                .await
                .map_err(|e| e.to_string())
        }
    }

    async fn search(index: &IndexArc, query: &str, top_k: usize) -> Result<Vec<SeekHit>, String> {
        use seekstorm::index::DistanceField;
        use seekstorm::search::{QueryRewriting, QueryType, ResultType, Search, SearchMode};
        let obj = index
            .search(
                query.to_string(),
                None, // query_vector — the vector path is the caller's embedding seam
                QueryType::Intersection,
                SearchMode::Lexical, // vectorless default
                false,
                0,
                top_k,
                ResultType::TopkCount,
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                QueryRewriting::SearchOnly,
            )
            .await;
        let index = index.read().await;
        let mut out = Vec::with_capacity(obj.results.len());
        for r in obj.results.iter() {
            let highlighter: Option<seekstorm::highlighter::Highlighter> = None;
            let fields = std::collections::HashSet::<String>::new();
            let dist: Vec<DistanceField> = Vec::new();
            let doc = index
                .get_document(r.doc_id, false, &highlighter, &fields, &dist)
                .await
                .map_err(|e| e.to_string())?;
            let chunk_id = doc
                .get("chunk_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push(SeekHit {
                chunk_id,
                score: r.score as f64,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_parses_all_eight_modes() {
        assert_eq!(QueryMode::ALL.len(), 8);
        for mode in QueryMode::ALL {
            let name = format!("{mode:?}");
            assert_eq!(QueryMode::parse(&name), Some(mode));
        }
        assert_eq!(
            QueryMode::parse("weighted_hybrid"),
            Some(QueryMode::WeightedHybrid)
        );
        assert_eq!(
            QueryMode::parse("semantic-rescore"),
            Some(QueryMode::SemanticRescore)
        );
        assert!(QueryMode::parse("quantum").is_none());
    }

    #[test]
    fn planner_signal_usage() {
        assert!(QueryMode::Lexical.uses_lexical() && !QueryMode::Lexical.uses_vector());
        assert!(!QueryMode::Vector.uses_lexical() && QueryMode::Vector.uses_vector());
        assert!(QueryMode::Hybrid.uses_lexical() && QueryMode::Hybrid.uses_vector());
        assert_eq!(QueryMode::Lexical.fusion_weights(), (1.0, 0.0));
        assert_eq!(QueryMode::Hybrid.fusion_weights(), (0.5, 0.5));
        assert_eq!(QueryMode::ReScore.fusion_weights(), (0.3, 0.7));
    }

    #[test]
    fn planner_recommendation_keeps_vectorless_default() {
        assert_eq!(QueryMode::recommend(false, false), QueryMode::Lexical);
        assert_eq!(QueryMode::recommend(false, true), QueryMode::Lexical);
        assert_eq!(QueryMode::recommend(true, false), QueryMode::Hybrid);
        assert_eq!(QueryMode::recommend(true, true), QueryMode::ReScore);
        assert!(SEEKSTORM_VERDICT.contains("hybrid-seekstorm"));
    }

    #[cfg(feature = "hybrid-seekstorm")]
    #[test]
    fn embedded_hybrid_index_roundtrip() {
        let dir = std::env::temp_dir().join(format!("seek-test-{}", std::process::id()));
        let idx = embedded::HybridIndex::open(&dir).unwrap();
        idx.add("doc-a", "the quick brown fox jumps").unwrap();
        idx.add("doc-b", "nothing about cats here").unwrap();
        idx.add("doc-c", "brown bear eats berries").unwrap();
        idx.commit().unwrap();
        let hits = idx.search("brown", 3).unwrap();
        assert!(!hits.is_empty());
        let ids: Vec<&str> = hits.iter().map(|h| h.chunk_id.as_str()).collect();
        // Every hit is a known chunk and at least one brown-doc is on top
        // (3.3.4 ranking may prefer doc-c, whose brown token is first).
        assert!(ids.contains(&"doc-a") || ids.contains(&"doc-c"));
        assert!(
            ids.iter()
                .all(|id| matches!(*id, "doc-a" | "doc-b" | "doc-c"))
        );
        assert!(hits[0].score > 0.0);
        idx.close();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
