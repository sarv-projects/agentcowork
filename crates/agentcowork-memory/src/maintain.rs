//! P36 (C) — `maintain()`-class tools, not only store/retrieve:
//! **analyze references** (orphan / duplicate detection over the store),
//! **update graph** (materialize edges from text), and **decay** (ACT-R
//! retention sweep). Pure primitives; the coordinator schedules them on
//! turn boundaries (never mid-turn).

use crate::actr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What the maintenance pass found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaintainReport {
    pub analyzed: usize,
    pub orphaned: Vec<String>,
    pub near_duplicates: Vec<(String, String, f64)>,
    pub graph_edges_added: usize,
    pub decayed_count: usize,
    pub protected_by_importance: usize,
}

/// The `maintain` tool kinds (mirror the CLI verbs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaintainKind {
    AnalyzeReferences,
    UpdateGraph,
    Decay,
    Full,
}

/// One memory row the analyzer understands.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRow {
    pub id: String,
    pub text: String,
    /// Importance 0..=10 — `>= 8` is protected from forgetting (P5.5).
    pub importance: u8,
    /// Last access (for recency decay).
    pub last_accessed_at: u64,
    /// Access count (for strength).
    pub accesses: u64,
    /// Graph neighbors (already-linked ids).
    pub linked: Vec<String>,
}

/// Analyze references: find orphaned memories (no graph link, never
/// retrieved) and near-duplicates (token-overlap ratio).
pub fn analyze_references(rows: &[MemoryRow]) -> MaintainReport {
    let mut report = MaintainReport {
        analyzed: rows.len(),
        ..Default::default()
    };
    for r in rows {
        if r.linked.is_empty() && r.accesses == 0 {
            report.orphaned.push(r.id.clone());
        }
    }
    // Near-duplicate pairs by overlap on token sets (Jaccard-lite).
    for (i, a) in rows.iter().enumerate() {
        for b in rows.iter().skip(i + 1) {
            let sim = token_overlap(&a.text, &b.text);
            if sim >= 0.8 {
                report
                    .near_duplicates
                    .push((a.id.clone(), b.id.clone(), sim));
            }
        }
    }
    report
}

/// Update graph: derive edges between memories that share significant terms
/// but have no link yet — the `maintain(update-graph)` pass.
pub fn update_graph(rows: &[MemoryRow]) -> MaintainReport {
    let mut report = MaintainReport::default();
    // term → doc ids (len >= 4, lowercase)
    let mut term_docs: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        for t in r.text.split(|c: char| !c.is_alphanumeric()) {
            let t = t.to_lowercase();
            if t.len() >= 4 {
                term_docs.entry(t).or_default().push(i);
            }
        }
    }
    let mut added = std::collections::HashSet::new();
    for ids in term_docs.values() {
        for (i, &a) in ids.iter().enumerate() {
            for &b in ids.iter().skip(i + 1) {
                if a == b {
                    continue;
                }
                let row_a = &rows[a];
                if row_a.linked.contains(&rows[b].id) {
                    continue;
                }
                if added.insert((a.min(b), a.max(b))) {
                    report.graph_edges_added += 1;
                }
            }
        }
    }
    report
}

/// Activation floor below which an unprotected memory is forgotten by the
/// sweep (documented with ACT-R's `activation_threshold` vocabulary).
pub const FORGET_ACTIVATION_THRESHOLD: f64 = 0.05;

/// Decay: ACT-R retention sweep — forget low-activation low-importance rows;
/// `importance >= 8` never auto-forgotten (P5.5 floor). Returns the ids to
/// evict from the fast store.
pub fn decay_sweep(rows: &[MemoryRow], now: u64) -> (MaintainReport, Vec<String>) {
    let mut report = MaintainReport::default();
    let mut evict = Vec::new();
    for r in rows {
        // P5.5 importance floor: never auto-forgot.
        if r.importance >= 8 {
            report.protected_by_importance += 1;
            continue;
        }
        let mem = actr::Memory {
            id: r.id.clone(),
            importance: r.importance,
            strength: r.accesses as f64,
            created_at: r.last_accessed_at,
            last_access: r.last_accessed_at,
            keywords: Vec::new(),
            graph_links: 0,
        };
        let activation = actr::activation(&mem, now, 60.0);
        if activation < FORGET_ACTIVATION_THRESHOLD {
            report.decayed_count += 1;
            evict.push(r.id.clone());
        }
    }
    (report, evict)
}

fn token_overlap(a: &str, b: &str) -> f64 {
    let sa: std::collections::HashSet<String> = a
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_lowercase)
        .collect();
    let sb: std::collections::HashSet<String> = b
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_lowercase)
        .collect();
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    inter as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, text: &str, importance: u8, accesses: u64, linked: &[&str]) -> MemoryRow {
        MemoryRow {
            id: id.into(),
            text: text.into(),
            importance,
            last_accessed_at: 100,
            accesses,
            linked: linked.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn orphan_detection() {
        let rows = vec![
            row("m1", "the quick brown fox", 5, 9, &["m2"]),
            row("m2", "lazy dog", 5, 0, &[]),
        ];
        let rep = analyze_references(&rows);
        assert!(rep.orphaned.contains(&"m2".to_string()));
        assert!(!rep.orphaned.contains(&"m1".to_string()));
    }

    #[test]
    fn near_duplicate_pair() {
        let rows = vec![
            row("a", "meeting with alice about the budget plan", 5, 1, &[]),
            row(
                "b",
                "meeting with alice about the budget plan notes",
                5,
                1,
                &[],
            ),
            row(
                "c",
                "completely unrelated shopping list milk eggs",
                5,
                1,
                &[],
            ),
        ];
        let rep = analyze_references(&rows);
        assert_eq!(rep.near_duplicates.len(), 1);
    }

    #[test]
    fn graph_edges_derived_from_shared_terms() {
        let rows = vec![
            row("a", "rust borrow checker lifetimes", 5, 1, &[]),
            row("b", "rust async lifetimes guide", 5, 1, &[]),
            row("c", "grocery shopping list", 5, 1, &[]),
        ];
        let rep = update_graph(&rows);
        assert_eq!(
            rep.graph_edges_added, 1,
            "a↔b share rust+lifetimes; c links to nobody"
        );
    }

    #[test]
    fn decay_respects_importance_floor() {
        let rows = vec![
            row("prot", "important fact worth keeping", 9, 0, &[]),
            row("weak", "stale unimportant note", 1, 0, &[]),
        ];
        let (rep, evict) = decay_sweep(&rows, 1_000_000);
        assert_eq!(rep.protected_by_importance, 1);
        assert_eq!(evict, vec!["weak".to_string()]);
    }
}
