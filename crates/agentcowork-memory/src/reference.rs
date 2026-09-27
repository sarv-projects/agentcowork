//! Pass-by-reference context (C10 — doc 39 NOOA pass-by-reference).
//!
//! Files/datasets/tool results are exposed as live handles with a **bounded
//! preview** (head/tail + metadata), so the agent references the payload
//! instead of serializing it into context. The query/slice path runs in the
//! E4 script-eval sandbox; this module owns the handle + preview math.

use crate::fusion::approx_tokens;

/// Preview budget — the whole handle must stay ≤ this many tokens (2K).
pub const PREVIEW_BUDGET_TOKENS: usize = 2000;

/// Truncation marker inserted between head and tail.
pub const PREVIEW_MARKER: &str = "\n…[preview truncated — query via script-eval]…\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    File,
    Dataset,
    ToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RefHandle {
    pub id: String,
    pub path: String,
    pub kind: RefKind,
    pub size_bytes: u64,
    pub row_count: Option<u64>,
    pub preview: String,
}

impl RefHandle {
    pub fn preview_tokens(&self) -> usize {
        approx_tokens(&self.preview)
    }
}

/// Head/tail preview: keep `head_tokens` from the front and `tail_tokens` from
/// the back, with a truncation marker in between.
pub fn bounded_preview(data: &str, head_tokens: usize, tail_tokens: usize) -> String {
    let chars: Vec<char> = data.chars().collect();
    let head_chars = head_tokens.saturating_mul(4);
    let tail_chars = tail_tokens.saturating_mul(4);
    if chars.len() <= head_chars + tail_chars {
        return data.to_string();
    }
    let mut out: String = chars[..head_chars].iter().collect();
    out.push_str(PREVIEW_MARKER);
    out.extend(&chars[chars.len() - tail_chars..]);
    out
}

/// P5.8 — query a ref's full payload without serializing it into context:
/// return only the matching lines (capped at `max_hits`). The E4 script-eval
/// `data.query(fn)` primitive calls this; the agent gets the matches, not the
/// payload. Case-insensitive substring match, empty needle → no hits.
pub fn query_ref(data: &str, term: &str, max_hits: usize) -> Vec<String> {
    let needle = term.to_lowercase();
    if needle.is_empty() || max_hits == 0 {
        return Vec::new();
    }
    data.lines()
        .filter(|l| l.to_lowercase().contains(&needle))
        .take(max_hits)
        .map(str::to_string)
        .collect()
}

/// Build a ref handle whose preview fits within the 2K-token budget.
pub fn make_ref_handle(
    id: &str,
    path: &str,
    kind: RefKind,
    data: &str,
    size_bytes: u64,
    row_count: Option<u64>,
) -> RefHandle {
    // Leave room for the truncation marker so head + tail + marker ≤ budget.
    let marker_tokens = approx_tokens(PREVIEW_MARKER);
    let half = (PREVIEW_BUDGET_TOKENS - marker_tokens) / 2;
    let preview = bounded_preview(data, half, half);
    debug_assert!(approx_tokens(&preview) <= PREVIEW_BUDGET_TOKENS);
    RefHandle {
        id: id.to_string(),
        path: path.to_string(),
        kind,
        size_bytes,
        row_count,
        preview,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_megabyte_file_stays_under_budget() {
        // ~10 MB of text → the preview must fit in ≤2K tokens (~8K chars).
        let data = "line of text that is fairly long\n".repeat(400_000);
        assert!(data.len() > 10_000_000);

        let handle = make_ref_handle(
            "f1",
            "/big/file.txt",
            RefKind::File,
            &data,
            data.len() as u64,
            None,
        );
        assert!(handle.preview_tokens() <= PREVIEW_BUDGET_TOKENS);
        // Head and tail are both preserved.
        assert!(handle.preview.starts_with("line of text"));
        assert!(
            handle
                .preview
                .trim_end()
                .ends_with("line of text that is fairly long")
        );
    }

    #[test]
    fn short_data_is_verbatim() {
        let handle = make_ref_handle("s", "/small.txt", RefKind::File, "hello", 5, None);
        assert_eq!(handle.preview, "hello");
        assert!(handle.preview_tokens() <= PREVIEW_BUDGET_TOKENS);
    }

    #[test]
    fn preview_preserves_head_and_tail() {
        let data = "ABCDEFGHIJ".repeat(1000); // 10000 chars
        let p = bounded_preview(&data, 10, 10);
        assert!(p.starts_with("ABCDEFGHIJABCDEFGHIJ"));
        assert!(p.ends_with("ABCDEFGHIJABCDEFGHIJ"));
        assert!(p.contains("truncated"));
    }

    #[test]
    fn query_ref_returns_only_matches_not_payload() {
        let data = "line one alpha\nline two beta\nline three alpha again\nline four";
        let hits = query_ref(data, "alpha", 5);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].contains("alpha"));
        assert!(hits[1].contains("alpha again"));
        // Cap + empty needle.
        assert_eq!(query_ref(data, "alpha", 1).len(), 1);
        assert!(query_ref(data, "", 5).is_empty());
    }
}
