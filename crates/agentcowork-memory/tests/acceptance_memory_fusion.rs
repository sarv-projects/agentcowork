//! Track 2 / P66.8 — Cognitive memory fusion acceptance suite.
//!
//! Evidence-gated readiness (§17.12.6): this exercises the *public* fusion +
//! ACT-R cores end-to-end on a realistic multi-topic corpus and simulated
//! multi-day time deltas — the vectorless default the product actually ships
//! (BM25/Okapi + reciprocal-rank fusion), plus retention decay and the
//! importance floor:
//!
//! 1. **BM25 retrieval** ranks the on-topic document first and drops
//!    sub-2-char/noise terms.
//! 2. **RRF fusion** reinforces ids that two signals agree on, is deterministic,
//!    and normalizes confidence to 0..=1.
//! 3. **ACT-R retention** decays over multi-day gaps, the importance floor
//!    protects high-salience memories from an auto-forget sweep, and
//!    associative `recall_score` fuses semantic + keyword + recency + graph.
//!
//! Cross-platform: runs on every host. The fused retrieval is pure math over
//! caller-supplied signals (FTS5/vector/graph live in the storage layer); the
//! Windows end-to-end run remains open — see TODO P66.8.

use agentcowork_memory::{
    Bm25Doc, Bm25Index, Hit, Memory, RecallWeights, SignalKind, SignalRank, activation,
    derive_queries, forget_sweep, fuse_signals, keyword_hits, recall_score, recency,
};

const DAY: u64 = 86_400;

fn corpus() -> Vec<Bm25Doc> {
    [
        (
            "doc-rust-own",
            "Rust ownership and the borrow checker: moves, lifetimes, and references",
        ),
        (
            "doc-rust-async",
            "Rust async runtimes: tokio tasks, futures, and executors",
        ),
        (
            "doc-python",
            "Python asyncio event loops, coroutines, and await points",
        ),
        (
            "doc-sqlite",
            "SQLite indexing strategy: covering indexes and query planning",
        ),
        (
            "doc-cooking",
            "Sourdough bread: hydration, fermentation, and oven spring",
        ),
    ]
    .iter()
    .map(|(id, text)| Bm25Doc {
        id: (*id).to_string(),
        text: (*text).to_string(),
    })
    .collect()
}

#[test]
fn bm25_ranks_on_topic_and_rrf_fusion_is_deterministic() {
    let mut index = Bm25Index::new();
    index.build(corpus());
    assert_eq!(index.len(), 5);

    // The ownership doc must win for an ownership/borrow query.
    let ranked = index.search("rust ownership borrow lifetimes", 3);
    assert_eq!(
        ranked.first().map(String::as_str),
        Some("doc-rust-own"),
        "{ranked:?}"
    );

    // Noise / sub-2-char terms yield nothing (never a spurious match).
    assert!(
        index.search("a i", 3).is_empty(),
        "terms < 2 chars are dropped"
    );

    let keyword = index.as_signal("rust ownership borrow", 5);
    assert!(!keyword.is_empty());
    assert!(
        keyword
            .iter()
            .all(|h| h.kind == SignalKind::Keyword && (0.0..=1.0).contains(&h.confidence)),
        "signal confidences must be normalized 0..=1: {keyword:?}"
    );

    // A second signal (vector/graph) that agrees on the ownership doc and
    // contributes one doc the keyword signal missed.
    let vector = vec![
        Hit {
            id: "doc-rust-own".into(),
            kind: SignalKind::Semantic,
            confidence: 0.8,
        },
        Hit {
            id: "doc-rust-async".into(),
            kind: SignalKind::Semantic,
            confidence: 0.6,
        },
    ];
    let fused = fuse_signals(
        &[
            (SignalRank::Keyword, keyword.clone()),
            (SignalRank::Vector, vector.clone()),
        ],
        5,
    );

    // Agreement reinforces: the doc in both lists outranks both singletons.
    let ids: Vec<&str> = fused.iter().map(|h| h.id.as_str()).collect();
    let pos = |id: &str| ids.iter().position(|x| *x == id).expect("present");
    assert!(
        pos("doc-rust-own") < pos("doc-rust-async"),
        "a doc both signals rank must beat one only the vector ranks: {ids:?}"
    );
    assert!(fused.iter().all(|h| (0.0..=1.0).contains(&h.confidence)));

    // Deterministic: same inputs → same order.
    let again = fuse_signals(
        &[(SignalRank::Keyword, keyword), (SignalRank::Vector, vector)],
        5,
    );
    assert_eq!(ids, again.iter().map(|h| h.id.as_str()).collect::<Vec<_>>());
}

#[test]
fn actr_decays_over_multiday_gaps_and_protects_high_importance() {
    // 30-day half-life base → measurable multi-day decay.
    let half_life = 30.0 * DAY as f64;
    let m = Memory {
        id: "m".into(),
        importance: 3,
        strength: 1.0,
        created_at: 0,
        last_access: 0,
        keywords: vec!["rust".into(), "ownership".into()],
        graph_links: 1,
    };

    let a1 = activation(&m, DAY, half_life);
    let a7 = activation(&m, 7 * DAY, half_life);
    let a30 = activation(&m, 30 * DAY, half_life);
    assert!(
        a1 > a7 && a7 > a30,
        "activation must decay monotonically: {a1} {a7} {a30}"
    );
    assert!(
        (a1 - 0.953).abs() < 0.01,
        "1-day activation ≈ 0.953, got {a1}"
    );
    assert!(
        (a30 - 0.236).abs() < 0.01,
        "30-day activation ≈ 0.236, got {a30}"
    );

    // A 30-day sweep: a deeply-faded low-importance memory is forgotten, but
    // one at/above the importance floor is never auto-forgotten.
    let faded = Memory {
        id: "faded".into(),
        strength: 0.05,
        importance: 3,
        ..m.clone()
    };
    let salient = Memory {
        id: "salient".into(),
        strength: 0.0,
        importance: 9,
        ..m.clone()
    };
    let all = [faded, salient];
    let survivors = forget_sweep(&all, 30 * DAY, half_life, 8, 0.05);
    let ids: Vec<&str> = survivors.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["salient"],
        "floor-protected memory must survive; faded one must not"
    );

    // Associative recall fuses the four signals; a strong recent hit beats a
    // weak cold one.
    let w = RecallWeights::default();
    let strong = recall_score(0.9, keyword_hits(&m, &["rust"]), recency(60), 5, &w);
    let weak = recall_score(0.2, keyword_hits(&m, &["python"]), recency(7 * DAY), 0, &w);
    assert!(
        strong > weak,
        "strong fused recall {strong} must beat {weak}"
    );
    assert!((keyword_hits(&m, &["rust", "python"]) - 0.5).abs() < 1e-9);

    // Spontaneous-recall query derivation filters stopwords and orders by
    // frequency (the pre-turn hook).
    let queries = derive_queries(
        &[
            "the", "sqlite", "index", "sqlite", "of", "planning", "index", "sqlite",
        ],
        2,
    );
    assert_eq!(queries, vec!["sqlite".to_string(), "index".to_string()]);
}
