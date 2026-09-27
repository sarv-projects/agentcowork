//! P5.10 — sidecar-runtime algorithm benchmarks (the "retest built
//! algorithms", "benchmark multi-hop vs plain BM25", and
//! "spreading-activation / phantom-thread / temporal-anticipation" items).
//!
//! These are deterministic regression benchmarks: each asserts a *correctness*
//! property AND prints a timing line (`eprintln!`) so a `--nocapture` run gives
//! a wall-clock signal without a nightly `#[bench]` harness. They exercise the
//! real algorithm cores (bm25, graph, ghost, actr) — the same code the
//! coordinator sidecar calls — so they double as the "does it still work in
//! the desktop runtime" retest.

use crate::actr::{Memory, activation, recency};
use crate::bm25::{Bm25Doc, Bm25Index, Hit, SignalKind, SignalSource, run_signals_parallel};
use crate::ghost::GhostIndex;
use crate::graph::{EdgeType, GraphStore, NodeKind};
use std::time::Instant;

/// P5.10 — consolidated "all built algorithms" smoke test. Exercises every
/// algorithm core once end-to-end in a single process (the same code the
/// sidecar runtime calls). If any algorithm panics or produces an empty/garbage
/// result, this fails — the in-process stand-in for "run all 17 suites in the
/// sidecar runtime" until the literal sidecar-process harness lands.
#[test]
fn smoke_all_algorithms() {
    let start = Instant::now();

    // 1. fusion (Alg #18/#29)
    let a = vec![("x".to_string(), 9.0), ("y".to_string(), 8.0)];
    let b = vec![("y".to_string(), 9.0), ("x".to_string(), 8.0)];
    assert_eq!(
        crate::fusion::rrf_fuse(
            &[
                crate::fusion::Signal {
                    weight: 1.0,
                    hits: &a
                },
                crate::fusion::Signal {
                    weight: 1.0,
                    hits: &b
                },
            ],
            60.0
        )
        .len(),
        2
    );
    assert_eq!(crate::fusion::dedupe(&a).len(), 2);
    assert!(!crate::fusion::smart_snippets("the fox jumps", &["fox"], 4).is_empty());
    assert!(
        crate::fusion::cap_text(
            "x".repeat(10_000).as_str(),
            crate::fusion::ContentType::Memory
        )
        .ends_with('…')
    );
    assert!(crate::fusion::budget_tokens(crate::fusion::ContentType::File) == 2000);
    assert!(!crate::fusion::merge_small_chunks(&["a".into(), "b".into()], 10, false).is_empty());

    // 2. actr (Alg #32)
    let mem = Memory {
        id: "m".into(),
        importance: 10,
        strength: 1.0,
        created_at: 0,
        last_access: 0,
        keywords: vec![],
        graph_links: 0,
    };
    assert!(activation(&mem, 1_000, 3600.0) > 0.0);
    assert!(recency(0) > recency(1_000));

    // 3. compaction (Alg #21) + coordinator lifecycle
    let mut c = crate::compaction::CompactionCoordinator::new(
        crate::compaction::CompactionConfig::default(),
        100,
    );
    c.push_turn(95);
    let no_summarizers: &[&crate::compaction::Summarizer] = &[];
    assert!(
        c.maybe_compact("x".repeat(400).as_str(), no_summarizers)
            .is_some()
    );

    // 4. graph (Alg #6/#30)
    let mut g = crate::graph::GraphStore::new();
    g.add_node("n", crate::graph::NodeKind::Entity, "N");
    g.add_node("m", crate::graph::NodeKind::Entity, "M");
    g.add_edge("n", "m", crate::graph::EdgeType::Supports, 1.0, 0);
    assert_eq!(g.spreading_activation(&["n".into()], 0.5, 2, 5, 0).len(), 2);
    assert_eq!(g.query_depth("n", 2, 0).len(), 1);

    // 5. paging (Alg #20)
    let mut p = crate::paging::PagedMemory::new();
    p.write(crate::paging::MemoryEntry {
        id: "e1".into(),
        content: "hello world entry".into(),
        importance: 5,
    });
    p.flush_writes();
    assert!(p.read("e1").is_some());
    assert!(p.search("world").len() == 1);

    // 6. ghost
    let mut gh = crate::ghost::GhostIndex::new();
    gh.index("/f", "r1");
    assert_eq!(
        gh.apply_fs_event(&crate::ghost::FsEvent::Removed("/f".into())),
        1
    );

    // 7. reference (C10) + query (P5.8)
    let h = crate::reference::make_ref_handle(
        "f",
        "/f.txt",
        crate::reference::RefKind::File,
        "line alpha\nline beta\n",
        22,
        None,
    );
    assert!(h.preview_tokens() <= crate::reference::PREVIEW_BUDGET_TOKENS);
    assert_eq!(
        crate::reference::query_ref("a alpha\nb beta", "alpha", 5).len(),
        1
    );
    assert!(
        crate::reference::bounded_preview("abcdefghij".repeat(10).as_str(), 4, 4)
            .contains("truncated")
    );

    // 8. fsrs (C13)
    let fsrs = crate::fsrs::Fsrs::new(&crate::fsrs::DEFAULT_PARAMETERS).expect("fsrs params");
    let report = crate::fsrs::simulate(&fsrs, &crate::fsrs::SimulationConfig::default());
    assert!(report.total_reviews > 0);

    // 9. classify (Vane)
    let intent = crate::classify::classify("search the web and make a chart of the results");
    assert!(intent.needs_research || intent.needs_tools || intent.needs_widgets);

    // 10. summary (deepwiki-open)
    let fs = crate::summary::summarize_file("/a.rs", "fn main() {}\nlet x = 1;\n// todo: more");
    assert!(fs.summary.contains("fn main"));
    assert!(!crate::summary::answer_over_summaries(&[fs], "main", 3).is_empty());

    // 11. reinforce (FSRS queue)
    let mut q = crate::reinforce::ReviewQueue::new(0.9);
    let cands = crate::reinforce::extract_candidates(
        "This is an important fact. This is another important fact.",
    );
    assert!(!cands.is_empty());
    assert!(q.ingest(cands, 0) > 0);

    // 12. bm25 + parallel fusion
    let mut idx = crate::bm25::Bm25Index::new();
    idx.build(vec![crate::bm25::Bm25Doc {
        id: "d".into(),
        text: "rust borrow checker".into(),
    }]);
    assert_eq!(
        crate::bm25::run_signals_parallel("rust", &idx, None, None, 3)[0].id,
        "d"
    );

    // 13. planner (C7)
    let mut pl = crate::planner::ContextPlanner::new(crate::planner::PlannerConfig::default());
    assert!(pl.inject_warm_set(&p, 100) > 0);

    // 14. janus
    let j = crate::janus::run_janus("dup\ndup\nunique", 2, 2);
    assert!(
        j.removed_blocks >= 1,
        "janus must remove the duplicate block"
    );

    // 15. cognee
    let mut cg = crate::cognee::CogneeMemory::new();
    cg.remember("k", "The capital of France is Paris", 8);
    cg.flush();
    assert!(!cg.recall("capital").entries.is_empty());

    // 16. rtk
    let r = crate::rtk::compress("ls -la", "-rw-r--r-- 1 user user 12 Jan 1 file.txt");
    assert!(!r.output.is_empty());

    // 17. cache (semantic + result)
    let mut sc = crate::cache::SemanticCache::new(0.7, 60);
    sc.put("what is the capital of france", "Paris", true, 0);
    assert!(sc.get("what is the capital of france", 1).is_some());
    let mut rc = crate::cache::ResultCache::new(60);
    rc.put("sig", "out", &["dep"], true, 0);

    // 18. embedding (C5)
    assert!(crate::embedding::cosine(&[1.0, 0.0], &[1.0, 0.0]) > 0.99);

    // 19. rerank (Alg #19)
    let cands = vec![crate::rerank::Candidate {
        id: "c1".into(),
        text: "rust borrow checker".into(),
    }];
    let retr: std::collections::HashMap<String, f64> = [("c1".to_string(), 0.8)].into();
    let reranker = crate::rerank::LexicalReranker::new();
    assert_eq!(
        crate::rerank::rerank(&cands, &retr, &reranker, "rust", 0.5, 0.5, 3).len(),
        1
    );

    // 20. repair (P1.8)
    let rep =
        crate::repair::repair_tool_json("```json\n{\"kind\":\"click\",\"ref_id\":\"e1\",}\n```");
    assert!(
        serde_json::from_str::<serde_json::Value>(&rep.json).is_ok(),
        "repaired json must parse: {}",
        rep.json
    );

    // 21. usage (P8)
    let mut ul = crate::usage::UsageLedger::new();
    ul.set_active("s", "k");
    ul.record_observed(
        crate::usage::UsageSource::AgentReport,
        100,
        10,
        true,
        50,
        0,
        0.0,
    );
    ul.clear_active();
    assert_eq!(ul.total().total_tokens(), 110);

    eprintln!(
        "[smoke] all {} algorithms exercised in {:?}",
        21,
        start.elapsed()
    );
}

/// A graph-backed retrieval signal (spreading activation → ranked hits).
/// Node ids are doc ids; `seed_for` maps a query to a seed node by keyword.
struct GraphSource {
    graph: GraphStore,
    at_time: u64,
}

impl GraphSource {
    fn seed_for(&self, query: &str) -> Option<String> {
        // Seed = the node whose label is mentioned in the query (case-
        // insensitive). The real coordinator uses the classifier/embedding;
        // this is the deterministic keyword seed for the benchmark.
        let q = query.to_lowercase();
        ["d_tower", "d_france", "d_paris"]
            .iter()
            .find(|id| q.contains(&id[2..].replace('_', " ")))
            .map(|s| s.to_string())
    }
}

impl SignalSource for GraphSource {
    fn retrieve(&self, query: &str, k: usize) -> Vec<Hit> {
        let Some(seed) = self.seed_for(query) else {
            return Vec::new();
        };
        self.graph
            .spreading_activation(&[seed], 0.5, 3, k, self.at_time)
            .into_iter()
            .map(|(id, confidence)| Hit {
                id,
                kind: SignalKind::Graph,
                confidence,
            })
            .collect()
    }
}

/// Multi-hop question: "what is the capital of the country with the Eiffel
/// Tower?" The answer (Paris) is two hops from the query term (Eiffel Tower →
/// France → Paris). Plain BM25 sees only the surface match; the graph signal
/// walks the hops. The benchmark asserts the fused retrieval surfaces the
/// multi-hop answer that BM25 alone misses.
#[test]
fn bench_multi_hop_vs_plain_bm25() {
    // The answer doc (d_paris) shares ZERO terms with the query — it is only
    // reachable through the graph hops. Filler docs make the corpus large
    // enough that plain BM25's top-k cannot include d_paris at all.
    let mut docs = vec![Bm25Doc {
        id: "d_tower".into(),
        text: "The Eiffel Tower is a famous landmark".into(),
    }];
    for i in 1..=7 {
        docs.push(Bm25Doc {
            id: format!("f{i}"),
            text: format!("unrelated filler topic number {i}"),
        });
    }
    docs.push(Bm25Doc {
        id: "d_france".into(),
        text: "A country in western Europe".into(),
    });
    docs.push(Bm25Doc {
        id: "d_paris".into(),
        text: "The capital and largest city".into(),
    });

    let mut bm25 = Bm25Index::new();
    bm25.build(docs);

    let mut graph = GraphStore::new();
    graph.add_node("d_tower", NodeKind::Entity, "Eiffel Tower");
    graph.add_node("d_france", NodeKind::Entity, "France");
    graph.add_node("d_paris", NodeKind::Entity, "Paris");
    graph.add_edge("d_tower", "d_france", EdgeType::DerivedFrom, 1.0, 0);
    graph.add_edge("d_france", "d_paris", EdgeType::Supports, 1.0, 0);

    let start = Instant::now();
    let bm25_only = run_signals_parallel("eiffel tower", &bm25, None, None, 5);
    let bm25_elapsed = start.elapsed();
    let fused = run_signals_parallel(
        "eiffel tower",
        &bm25,
        None,
        Some(&GraphSource { graph, at_time: 0 }),
        5,
    );
    let fused_elapsed = start.elapsed();

    // BM25 alone surfaces the surface match, not the 2-hop answer.
    assert!(
        bm25_only.iter().any(|h| h.id == "d_tower"),
        "BM25 must rank the surface match first: {bm25_only:?}"
    );
    assert!(
        !bm25_only.iter().any(|h| h.id == "d_paris"),
        "plain BM25 must NOT surface the multi-hop answer (no hop walk): {bm25_only:?}"
    );

    // Fused (BM25 + graph) walks the hops and surfaces Paris.
    assert!(
        fused.iter().any(|h| h.id == "d_paris"),
        "fused retrieval must surface the multi-hop answer via graph spreading: {fused:?}"
    );

    eprintln!(
        "[bench] multi-hop vs BM25: bm25_only {bm25_elapsed:?} / fused {fused_elapsed:?} (paris surfaced via graph)"
    );
}

/// Spreading-activation benchmark over a ~30-node chain+fan graph: assert the
/// direct neighbor outranks the far leaf and print timing.
#[test]
fn bench_spreading_activation() {
    let mut g = GraphStore::new();
    for i in 0..30 {
        g.add_node(&format!("n{i}"), NodeKind::Entity, &format!("node {i}"));
    }
    for i in 0..29 {
        g.add_edge(
            &format!("n{i}"),
            &format!("n{}", i + 1),
            EdgeType::Supports,
            0.9,
            0,
        );
    }
    // A contradicts edge to exercise lateral inhibition (negative activation).
    g.add_edge("n1", "n5", EdgeType::Contradicts, 1.0, 0);

    let start = Instant::now();
    let out = g.spreading_activation(&["n0".into()], 0.5, 3, 10, 0);
    let elapsed = start.elapsed();

    assert!(!out.is_empty());
    // Direct neighbor n1 outranks the depth-3 leaf (decay).
    let rank1 = out.iter().position(|(id, _)| id == "n1").unwrap();
    let rank3 = out.iter().position(|(id, _)| id == "n3").unwrap();
    assert!(rank1 < rank3, "activation must decay with depth: {out:?}");
    // Contradicted node n5 must not be positively activated.
    assert!(
        out.iter().all(|(id, _)| id != "n5"),
        "contradicted node must be inhibited: {out:?}"
    );

    eprintln!(
        "[bench] spreading_activation: {elapsed:?} (top {:?})",
        &out[..3.min(out.len())]
    );
}

/// Phantom-thread benchmark: ghost-context prevention must atomically evict a
/// deleted file's refs (no phantom re-surfacing) and re-path a rename with
/// zero re-embedding. Assert both + print timing.
#[test]
fn bench_phantom_thread_ghost_eviction() {
    let mut g = GhostIndex::new();
    let start = Instant::now();
    for i in 0..10_000 {
        g.index(&format!("/docs/{i}.md"), &format!("mem:{i}"));
    }
    let index_elapsed = start.elapsed();

    let start = Instant::now();
    let tombstoned = g.tombstone("/docs/42.md");
    let tombstone_elapsed = start.elapsed();

    assert_eq!(tombstoned, vec!["mem:42".to_string()]);
    assert!(g.ids_for("/docs/42.md").is_empty(), "no phantom re-surface");

    let start = Instant::now();
    let moved = g.repath("/docs/43.md", "/renamed/43.md");
    let repath_elapsed = start.elapsed();
    assert_eq!(moved, 1);
    assert_eq!(g.ids_for("/renamed/43.md"), vec!["mem:43".to_string()]);

    eprintln!(
        "[bench] phantom-thread ghost: index {index_elapsed:?} / tombstone {tombstone_elapsed:?} / repath {repath_elapsed:?}"
    );
}

/// Temporal-anticipation benchmark: the bi-temporal graph returns the correct
/// edge version at each valid time, and ACT-R recency/activation produces a
/// monotone decay curve (older = lower). Assert both + print timing.
#[test]
fn bench_temporal_anticipation_and_actr() {
    let mut g = GraphStore::new();
    g.add_node("a", NodeKind::Entity, "A");
    g.add_node("b", NodeKind::Entity, "B");
    // Three versions of the same edge over time.
    g.add_edge("a", "b", EdgeType::Supports, 1.0, 10);
    g.add_edge("a", "b", EdgeType::Supports, 2.0, 20);
    g.add_edge("a", "b", EdgeType::Supports, 3.0, 30);

    let start = Instant::now();
    let w15 = g
        .edge_between("a", "b", EdgeType::Supports, 15)
        .unwrap()
        .weight;
    let w25 = g
        .edge_between("a", "b", EdgeType::Supports, 25)
        .unwrap()
        .weight;
    let w35 = g
        .edge_between("a", "b", EdgeType::Supports, 35)
        .unwrap()
        .weight;
    let graph_elapsed = start.elapsed();
    assert_eq!((w15, w25, w35), (1.0, 2.0, 3.0));

    // ACT-R: older memory → lower activation, monotone decay.
    let start = Instant::now();
    let mem = Memory {
        id: "m".into(),
        importance: 10,
        strength: 1.0,
        created_at: 0,
        last_access: 0,
        keywords: Vec::new(),
        graph_links: 0,
    };
    let a_fresh = activation(&mem, 100, 3600.0);
    let a_old = activation(&mem, 100_000, 3600.0);
    let actr_elapsed = start.elapsed();
    assert!(a_fresh > a_old, "ACT-R activation must decay with age");
    assert!(recency(0) > recency(100_000));

    eprintln!(
        "[bench] temporal-anticipation: graph {graph_elapsed:?} / actr {actr_elapsed:?} (activation fresh={a_fresh:.3} old={a_old:.3})"
    );
}
