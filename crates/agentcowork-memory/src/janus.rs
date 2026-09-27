//! Janus structural passes (doc 63 §2.1; the context-reduction pipeline):
//! dedup (exact + near-duplicate blocks), regex collapse (repeated patterns →
//! a single representative), and AST prune (structural trim of bracketed
//! trees). Each pass is deterministic and reports its token savings — the
//! building blocks the coordinator runs before injection.

use crate::fusion::approx_tokens;

/// Result of a structural pass.
#[derive(Debug, Clone, PartialEq)]
pub struct PassResult {
    pub output: String,
    /// Tokens removed (input - output).
    pub saved_tokens: usize,
    /// What the pass removed (for the audit trail).
    pub removed_blocks: usize,
}

/// Pass 1 — dedup: remove exact-duplicate lines/blocks and near-duplicate
/// blocks (same first line + ≥90% line overlap). Keeps first occurrence.
pub fn dedup(input: &str) -> PassResult {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0usize;
    let mut exact: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for block in split_blocks(input) {
        if exact.contains(block) {
            removed += 1;
            continue;
        }
        // Near-dup: same first line, ≥90% of lines already present.
        let lines: Vec<&str> = block.lines().collect();
        let first = lines.first().copied().unwrap_or("");
        if !first.is_empty() {
            let similar = kept.iter().any(|k| {
                let klines: Vec<&str> = k.lines().collect();
                let kfirst = klines.first().copied().unwrap_or("");
                if kfirst != first || klines.is_empty() {
                    return false;
                }
                let shared = lines.iter().filter(|l| klines.contains(l)).count();
                shared * 10 >= lines.len() * 9
            });
            if similar {
                removed += 1;
                continue;
            }
        }
        exact.insert(block);
        kept.push(block);
    }
    // Blocks stay separated by a blank line (structure preserved).
    let output = kept.join("\n\n");
    PassResult {
        saved_tokens: approx_tokens(input).saturating_sub(approx_tokens(&output)),
        removed_blocks: removed,
        output,
    }
}

/// Pass 2 — regex collapse: consecutive lines sharing the same structural
/// skeleton (non-alphanumeric shape, e.g. repeated `...=...` rows) collapse to
/// the first line + a count marker. No user regexes needed.
pub fn regex_collapse(input: &str, min_repeat: usize) -> PassResult {
    let skeleton = |l: &str| -> String {
        l.chars()
            .map(|c| if c.is_alphanumeric() { 'x' } else { c })
            .collect()
    };
    let lines: Vec<&str> = input.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut removed = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let skel = skeleton(line);
        let mut j = i + 1;
        while j < lines.len() && skeleton(lines[j]) == skel && !lines[j].trim().is_empty() {
            j += 1;
        }
        let run = j - i;
        if run >= min_repeat && !skel.trim().is_empty() {
            out.push(format!("{}  … ×{run} (collapsed)", lines[i]));
            removed += run - 1;
            i = j;
        } else {
            out.push(line.to_string());
            i += 1;
        }
    }
    let output = out.join("\n");
    PassResult {
        saved_tokens: approx_tokens(input).saturating_sub(approx_tokens(&output)),
        removed_blocks: removed,
        output,
    }
}

/// Pass 3 — AST prune: for brace-delimited blocks (code, JSON objects),
/// drop the *body* of nodes deeper than `max_depth`, keeping the
/// opening/closing structure (signatures without implementations). Only `{}`
/// counts as structure: parens/brackets belong to signatures and never
/// inflate the nesting depth, so multi-line signatures survive pruning.
pub fn ast_prune(input: &str, max_depth: usize) -> PassResult {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut removed = 0usize;
    let mut suppressed = false;
    for line in input.lines() {
        let open = line.matches('{').count();
        let close = line.matches('}').count();
        let new_depth = depth.saturating_add(open).saturating_sub(close.min(depth));
        // Body lines sit strictly below the structural depth: they start *and*
        // end below `max_depth`. Closing braces that return to the structural
        // level are structure, not body — otherwise `}` lines get dropped.
        let body_line = depth > max_depth && new_depth > max_depth;
        if body_line && !suppressed {
            suppressed = true;
            out.push_str(&format!(
                "{}  … body pruned at depth {max_depth}\n",
                line.trim_start()
            ));
            removed += 1;
        }
        if !body_line {
            out.push_str(line);
            out.push('\n');
        }
        depth = new_depth;
        if depth <= max_depth {
            suppressed = false;
        }
    }
    let output = out;
    PassResult {
        saved_tokens: approx_tokens(input).saturating_sub(approx_tokens(&output)),
        removed_blocks: removed,
        output,
    }
}

/// Run all three passes in order (dedup → collapse → prune).
pub fn run_janus(input: &str, min_repeat: usize, max_depth: usize) -> PassResult {
    let p1 = dedup(input);
    let p2 = regex_collapse(&p1.output, min_repeat);
    let p3 = ast_prune(&p2.output, max_depth);
    PassResult {
        saved_tokens: p1.saved_tokens + p2.saved_tokens + p3.saved_tokens,
        removed_blocks: p1.removed_blocks + p2.removed_blocks + p3.removed_blocks,
        output: p3.output,
    }
}

/// Split into blank-line-separated blocks.
fn split_blocks(input: &str) -> Vec<&str> {
    input
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_removes_exact_duplicates() {
        let input = "alpha\n\nbeta\n\nalpha\n";
        let r = dedup(input);
        assert_eq!(r.output, "alpha\n\nbeta");
        assert!(r.removed_blocks >= 1);
    }

    #[test]
    fn dedup_removes_near_duplicates() {
        // Block 2 shares block 1's first line + 9 of its 10 lines (90% =
        // the overlap threshold) → treated as a near-duplicate.
        let b1 = "header\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9";
        let b2 = "header\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10";
        let input = format!("{b1}\n\n{b2}\n");
        let r = dedup(&input);
        assert_eq!(r.removed_blocks, 1);
        assert!(!r.output.contains("l10"));
    }

    #[test]
    fn regex_collapse_repeated_rows() {
        let input = "a=1\na=2\na=3\na=4\n";
        let r = regex_collapse(input, 3);
        assert_eq!(r.removed_blocks, 3);
        assert!(r.output.contains("×4 (collapsed)"));
    }

    #[test]
    fn regex_collapse_respects_min_repeat() {
        let input = "a=1\na=2\n";
        let r = regex_collapse(input, 3);
        assert_eq!(r.removed_blocks, 0);
        assert_eq!(r.output.trim(), "a=1\na=2");
    }

    #[test]
    fn ast_prune_trims_deep_bodies_keeps_structure() {
        let input = "fn a() {\n  let x = 1;\n  let y = 2;\n}\nfn b() {}\n";
        let r = ast_prune(input, 0);
        assert!(r.output.contains("fn a() {"));
        assert!(r.output.contains("body pruned"));
        assert!(r.output.contains("fn b() {}"));
        assert!(r.removed_blocks >= 1);
    }

    #[test]
    fn janus_pipeline_reports_savings() {
        let input = "x=1\nx=2\nx=3\n\nfn f() {\n  body\n  body\n}\n\nx=1\n";
        let r = run_janus(input, 3, 0);
        assert!(r.removed_blocks >= 2);
        assert!(r.saved_tokens > 0);
    }
}
