//! Hierarchical repo summarization (P5 — doc 63 §4.15, deepwiki-open
//! pattern): summarize-file → summarize-directory → index summaries → answer
//! over summaries. This is the **no-embedding** long-context retrieval path —
//! it composes with the I7 repo-map and feeds the P8.0 eval corpus's
//! raw-vs-compressed-vs-retrieved delta measurement.

/// One file's summary (path + condensed text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSummary {
    pub path: String,
    pub summary: String,
}

/// Key marker lines kept by `summarize_file` beyond the head (code + prose).
const KEY_MARKERS: &[&str] = &[
    "fn ",
    "def ",
    "class ",
    "struct ",
    "enum ",
    "impl ",
    "pub ",
    "func ",
    "function ",
    "import ",
    "export ",
    "# ",
    "## ",
    "### ",
    "type ",
    "interface ",
    "const ",
    "let ",
    "todo",
    "fixme",
    "warning",
    "error",
];

/// Summarize a file: keep the first `head_lines` lines + any key marker lines
/// (function/type definitions, headings, TODOs), bounded to `max_chars`.
pub fn summarize_file(path: &str, content: &str) -> FileSummary {
    let head_lines = 40usize;
    let max_chars = 4000usize;

    let lines: Vec<&str> = content.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let is_head = i < head_lines;
        let is_key = KEY_MARKERS.iter().any(|m| line.trim_start().starts_with(m));
        if !(is_head || is_key) {
            continue;
        }
        if total + line.len() + 1 > max_chars {
            kept.push("…[truncated]…");
            break;
        }
        total += line.len() + 1;
        kept.push(line);
    }

    FileSummary {
        path: path.to_string(),
        summary: kept.join("\n"),
    }
}

/// Summarize a directory: a heading + one bullet per child file summary
/// (first line of each). The summaries are the child indices.
pub fn summarize_directory(name: &str, files: &[FileSummary]) -> String {
    let mut out = format!("# {name}\n");
    for f in files {
        let first = f.summary.lines().next().unwrap_or_default();
        out.push_str(&format!("- {} — {first}\n", f.path));
    }
    out
}

/// Index a set of directory summaries into a single top-level index (the
/// "index summaries" step — a table of contents the answer step scans).
pub fn index_summaries(dir_summaries: &[String]) -> String {
    let mut out = String::from("# Index\n");
    for (i, d) in dir_summaries.iter().enumerate() {
        // Pull just the directory heading + first bullet for the index.
        let heading = d.lines().next().unwrap_or_default();
        out.push_str(&format!("{i}. {heading}\n"));
    }
    out
}

/// Score how well a file summary matches a query (lowercased term overlap —
/// cheap, no embeddings).
fn relevance_score(summary: &str, query_terms: &[String]) -> usize {
    let lower = summary.to_lowercase();
    query_terms
        .iter()
        .filter(|t| lower.contains(t.as_str()))
        .count()
}

/// Answer over summaries: rank files by query-term overlap and return the
/// top `top_k` (most relevant first).
pub fn answer_over_summaries(files: &[FileSummary], query: &str, top_k: usize) -> Vec<FileSummary> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_lowercase())
        .collect();

    let mut scored: Vec<(usize, &FileSummary)> = files
        .iter()
        .map(|f| (relevance_score(&f.summary, &terms), f))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored
        .into_iter()
        .take(top_k)
        .filter(|(score, _)| *score > 0)
        .map(|(_, f)| f.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_file_keeps_head_and_key_markers() {
        let content = "line1\nline2\nfn main() {}\npub fn helper() {}\n# Heading\n";
        let s = summarize_file("src/main.rs", content);
        assert!(s.summary.contains("line1"));
        assert!(s.summary.contains("fn main() {}"));
        assert!(s.summary.contains("# Heading"));
    }

    #[test]
    fn summarize_directory_lists_children() {
        let files = vec![
            summarize_file("a.rs", "fn a() {}"),
            summarize_file("b.rs", "fn b() {}"),
        ];
        let dir = summarize_directory("crate", &files);
        assert!(dir.contains("# crate"));
        assert!(dir.contains("- a.rs"));
        assert!(dir.contains("- b.rs"));
    }

    #[test]
    fn index_summaries_builds_toc() {
        let idx = index_summaries(&["# dir1\n- a".to_string(), "# dir2\n- b".to_string()]);
        assert!(idx.contains("0. # dir1"));
        assert!(idx.contains("1. # dir2"));
    }

    #[test]
    fn answer_ranks_relevant_file_first() {
        let files = vec![
            summarize_file("travel.md", "Berlin trip travel policy and budget"),
            summarize_file("cook.md", "pasta recipe"),
        ];
        let top = answer_over_summaries(&files, "what is the berlin travel budget", 2);
        assert_eq!(top[0].path, "travel.md");
    }

    #[test]
    fn answer_drops_irrelevant_files() {
        let files = vec![summarize_file("cook.md", "pasta recipe")];
        let top = answer_over_summaries(&files, "berlin travel budget", 5);
        assert!(top.is_empty());
    }

    #[test]
    fn summarize_truncates_huge_files() {
        // Long lines (not many short ones) so the 4000-char budget is hit.
        let long_line = "y".repeat(200);
        let big = format!("{long_line}\n").repeat(200);
        let s = summarize_file("big.txt", &big);
        assert!(s.summary.len() <= 4000 + "[truncated]".len() + 20);
        assert!(s.summary.contains("truncated"));
    }
}
