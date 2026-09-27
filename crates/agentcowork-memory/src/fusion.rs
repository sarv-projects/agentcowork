//! Multi-signal retrieval fusion (Algorithm #18, mem0 pattern) + dedupe +
//! smart snippets + per-type budget caps + RAG chunk-min-size merging
//! (Algorithm #29). Pure scoring logic — the FTS5/vector/graph signals are
//! supplied by callers as ranked hit lists.

use std::collections::HashMap;

/// One retrieval signal: a ranked list of `(id, score)` plus a fusion weight.
#[derive(Debug, Clone, Copy)]
pub struct Signal<'a> {
    pub weight: f64,
    pub hits: &'a [(String, f64)],
}

/// Weighted Reciprocal Rank Fusion: `score(id) = Σ w · 1/(k + rank)`.
/// Ids missing from a signal contribute 0 from that signal. Output is sorted
/// by fused score descending.
pub fn rrf_fuse(signals: &[Signal], k: f64) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for sig in signals {
        for (rank, (id, _)) in sig.hits.iter().enumerate() {
            let contrib = sig.weight / (k + rank as f64 + 1.0);
            *scores.entry(id.clone()).or_insert(0.0) += contrib;
        }
    }
    let mut out: Vec<(String, f64)> = scores.into_iter().collect();
    sort_desc(&mut out);
    out
}

/// Dedupe a raw hit list, keeping the highest score per id.
pub fn dedupe(hits: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut best: HashMap<String, f64> = HashMap::new();
    for (id, s) in hits {
        let e = best.entry(id.clone()).or_insert(*s);
        if *s > *e {
            *e = *s;
        }
    }
    let mut out: Vec<(String, f64)> = best.into_iter().collect();
    sort_desc(&mut out);
    out
}

/// Extract windows around each case-insensitive occurrence of `terms`, with
/// `window` chars of context on either side and ellipsis markers.
pub fn smart_snippets(text: &str, terms: &[&str], window: usize) -> Vec<String> {
    let lower = text.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();

    for term in terms {
        let needle = term.to_lowercase();
        if needle.is_empty() {
            continue;
        }
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(&needle) {
            let abs = start + pos;
            let lo = abs.saturating_sub(window);
            let hi = (abs + needle.chars().count() + window).min(chars.len());
            let mut s = String::new();
            if lo > 0 {
                s.push('…');
            }
            s.extend(&chars[lo..hi]);
            if hi < chars.len() {
                s.push('…');
            }
            out.push(s);
            start = abs + needle.len().max(1);
        }
    }
    out
}

/// The per-type token budgets (P5.1): file 2K, page 1.5K, search 1K,
/// memory 600, tool 1K.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    File,
    Page,
    Search,
    Memory,
    Tool,
}

pub fn budget_tokens(ty: ContentType) -> usize {
    match ty {
        ContentType::File => 2000,
        ContentType::Page => 1500,
        ContentType::Search => 1000,
        ContentType::Memory => 600,
        ContentType::Tool => 1000,
    }
}

/// Rough token estimate (~4 chars/token) — good enough for budget capping
/// before a real tokenizer is wired in.
pub fn approx_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Truncate `text` to fit the budget for `ty`, appending an ellipsis.
pub fn cap_text(text: &str, ty: ContentType) -> String {
    let max_chars = budget_tokens(ty).saturating_mul(4);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let mut s: String = chars[..max_chars].iter().collect();
    s.push('…');
    s
}

fn is_header(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('#')
}

/// Forward-only merge of under-sized chunks (Algorithm #29). A chunk shorter
/// than `min_chars` is merged into the following chunk; `markdown_aware`
/// additionally refuses to merge across a `#`/`##` header boundary.
pub fn merge_small_chunks(
    chunks: &[String],
    min_chars: usize,
    markdown_aware: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if chunks.is_empty() {
        return out;
    }
    let mut acc = chunks[0].clone();
    for c in &chunks[1..] {
        if markdown_aware && is_header(c) {
            out.push(std::mem::take(&mut acc));
            acc = c.clone();
        } else if acc.chars().count() < min_chars {
            acc.push_str("\n\n");
            acc.push_str(c);
        } else {
            out.push(std::mem::take(&mut acc));
            acc = c.clone();
        }
    }
    out.push(acc);
    out
}

fn sort_desc(v: &mut [(String, f64)]) {
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(id, s)| (id.to_string(), *s)).collect()
    }

    #[test]
    fn rrf_fuses_rank_position() {
        let a = hits(&[("x", 9.0), ("y", 8.0)]);
        let b = hits(&[("y", 9.0), ("x", 8.0)]);
        let sigs = [
            Signal {
                weight: 1.0,
                hits: &a,
            },
            Signal {
                weight: 1.0,
                hits: &b,
            },
        ];
        let fused = rrf_fuse(&sigs, 60.0);
        // x and y each appear rank 0 and rank 1 → equal fused scores.
        assert_eq!(fused.len(), 2);
        let x = fused.iter().find(|(id, _)| id == "x").unwrap().1;
        let y = fused.iter().find(|(id, _)| id == "y").unwrap().1;
        assert!((x - y).abs() < 1e-12);
    }

    #[test]
    fn rrf_weight_boosts_signal() {
        let a = hits(&[("only_in_a", 1.0)]);
        let b = hits(&[("only_in_b", 1.0)]);
        let sigs = [
            Signal {
                weight: 3.0,
                hits: &a,
            },
            Signal {
                weight: 1.0,
                hits: &b,
            },
        ];
        let fused = rrf_fuse(&sigs, 60.0);
        assert_eq!(fused[0].0, "only_in_a");
    }

    #[test]
    fn dedupe_keeps_highest() {
        let raw = hits(&[("a", 0.9), ("b", 0.5), ("a", 0.7)]);
        let d = dedupe(&raw);
        assert_eq!(d.len(), 2);
        assert_eq!(d.iter().find(|(id, _)| id == "a").unwrap().1, 0.9);
    }

    #[test]
    fn snippets_window_around_match() {
        let text = "the quick brown fox jumps over the lazy dog";
        let snips = smart_snippets(text, &["fox"], 6);
        assert_eq!(snips.len(), 1);
        assert!(snips[0].contains("fox"));
        assert!(snips[0].contains("brown"));
        assert!(snips[0].contains("jumps"));
    }

    #[test]
    fn budget_caps_and_truncation() {
        assert_eq!(budget_tokens(ContentType::Memory), 600);
        let long = "x".repeat(10_000);
        let capped = cap_text(&long, ContentType::Memory);
        assert_eq!(capped.chars().count(), 600 * 4 + 1); // cap + ellipsis
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn merge_small_chunks_forward() {
        let chunks = vec![
            "tiny".to_string(),
            "also".to_string(),
            "this is a substantially long chunk that exceeds the minimum".to_string(),
        ];
        let merged = merge_small_chunks(&chunks, 40, false);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].starts_with("tiny\n\nalso"));
    }

    #[test]
    fn merge_respects_markdown_headers() {
        let chunks = vec![
            "tiny".to_string(),
            "## Section".to_string(),
            "body text under the header that is long enough on its own".to_string(),
        ];
        let merged = merge_small_chunks(&chunks, 40, true);
        // "tiny" is under-sized but must not cross the "## Section" header.
        assert!(merged[0] == "tiny" || merged[0].starts_with("tiny\n"));
        assert!(merged.iter().any(|c| c.starts_with("## Section")));
    }
}
