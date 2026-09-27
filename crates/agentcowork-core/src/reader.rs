//! P8.1 Reader (H6 — v2.0 §P1; D5 markitdown-class extraction → RAG + chat overlay).
//!
//! A universal document reader:
//!
//! - [`extract_text`] — pull readable text out of Markdown / plain text /
//!   HTML with **zero dependencies** (safe line/tag processing only). PDF and
//!   EPUB are explicitly *not* implemented in-process — they are documented
//!   seams (`ReaderError::Unsupported`) that the coordinator wires to an
//!   external extractor (e.g. a markitdown-class service), matching the
//!   project's install-gated-binary convention.
//! - [`ReaderIndex`] — a small chunked RAG index over extracted content.
//!   Documents are split into overlapping chunks; retrieval scores chunks by
//!   query-token overlap (deterministic, no embeddings required) and
//!   [`ReaderIndex::chat_overlay`] renders the top chunks as the context
//!   block for a chat overlay on the reader content.

use std::collections::HashMap;

/// Document formats the reader understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderFormat {
    Markdown,
    Plain,
    Html,
    /// Not implemented in-process — a documented extraction seam.
    Pdf,
    /// Not implemented in-process — a documented extraction seam.
    Epub,
}

impl ReaderFormat {
    pub fn from_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "markdown" | "md" => Some(ReaderFormat::Markdown),
            "plain" | "txt" => Some(ReaderFormat::Plain),
            "html" | "htm" => Some(ReaderFormat::Html),
            "pdf" => Some(ReaderFormat::Pdf),
            "epub" => Some(ReaderFormat::Epub),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderError {
    /// PDF/EPUB extraction is not done in-process (documented seam).
    Unsupported(ReaderFormat),
    /// The input was empty or had no extractable text.
    Empty,
}

impl std::fmt::Display for ReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReaderError::Unsupported(fmt) => {
                write!(
                    f,
                    "reader: {fmt:?} extraction is a runtime seam, not in-process"
                )
            }
            ReaderError::Empty => write!(f, "reader: no extractable text"),
        }
    }
}

impl std::error::Error for ReaderError {}

/// Extract readable text from a document body.
pub fn extract_text(format: ReaderFormat, content: &str) -> Result<String, ReaderError> {
    let text = match format {
        ReaderFormat::Plain => content.to_string(),
        ReaderFormat::Markdown => extract_markdown(content),
        ReaderFormat::Html => extract_html(content),
        ReaderFormat::Pdf => return Err(ReaderError::Unsupported(ReaderFormat::Pdf)),
        ReaderFormat::Epub => return Err(ReaderError::Unsupported(ReaderFormat::Epub)),
    };
    let text = collapse_whitespace(&text);
    if text.is_empty() {
        return Err(ReaderError::Empty);
    }
    Ok(text)
}

/// Strip Markdown formatting down to readable text: drop fenced code blocks,
/// images, reference links, and inline code backticks; keep headings/paragraphs.
fn extract_markdown(input: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Drop images `![alt](url)` and links `[text](url)` → keep text.
        let mut line = line.to_string();
        // Images first (so their alt text doesn't survive as a link).
        line = strip_pattern(&line, "![", "](");
        line = strip_pattern(&line, "[", "](");
        // Inline code `x` → x.
        line = line.replace('`', "");
        // Emphasis markers: `**bold**`, `__bold__`, `*em*`, `_em_` → text.
        line = line.replace("**", "").replace("__", "");
        line = line.replace(['*', '_'], "");
        // HTML comments.
        line = strip_html_comments(&line);
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Strip `[text](url)`-style spans, keeping `text`.
fn strip_pattern(line: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        if let Some(end) = after.find(close) {
            // Keep the text between the brackets, drop the (url) part.
            out.push_str(&after[..end]);
            let rest_after = &after[end + close.len()..];
            // Skip to the closing ')' of the URL.
            rest = match rest_after.find(')') {
                Some(close_paren) => &rest_after[close_paren + 1..],
                None => "",
            };
        } else {
            out.push_str(open);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// A dependency-free HTML tag stripper: removes `<...>` tags and decodes the
/// common entities. Not a real HTML parser — good enough for readable text.
fn extract_html(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if in_tag {
            if c == '>' {
                in_tag = false;
            }
            continue;
        }
        if in_script {
            // Skip until the matching close tag. This branch runs BEFORE the
            // generic `<` branch, or the closing tag would be consumed as a
            // plain tag and in_script would swallow the rest of the document.
            // The current char `<` was already consumed, so the window starts
            // after it: match `/script` / `/style`, then drain to the `>`.
            let window: String = chars.clone().take(8).collect();
            let lower = window.to_ascii_lowercase();
            if lower.starts_with("/script") || lower.starts_with("/style") {
                in_script = false;
                for n in chars.by_ref() {
                    if n == '>' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        if c == '<' {
            // Detect <script>/<style> blocks to drop their content.
            let rest: String = chars.clone().take(16).collect();
            let lower = rest.to_ascii_lowercase();
            if lower.starts_with("script") || lower.starts_with("style") {
                in_script = true;
            }
            in_tag = true;
            continue;
        }
        out.push(c);
    }
    decode_entities(&out)
}

fn strip_html_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Decode the common HTML entities.
fn decode_entities(s: &str) -> String {
    let mut out = s.to_string();
    let pairs = [
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&nbsp;", " "),
    ];
    for (from, to) in pairs {
        out = out.replace(from, to);
    }
    out
}

/// Collapse runs of whitespace to single spaces and trim.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// A document added to the reader index.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReaderDocument {
    pub id: String,
    pub title: String,
    pub format: ReaderFormat,
    pub text: String,
}

/// One indexed chunk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReaderChunk {
    pub doc_id: String,
    pub title: String,
    pub index: usize,
    pub text: String,
}

/// A retrieval hit with its relevance score.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReaderHit {
    pub chunk: ReaderChunk,
    pub score: f64,
}

/// A chunked RAG index over reader content. Deterministic, no embeddings:
/// chunks are scored by query-token overlap (token frequency × inverse doc
/// frequency over the chunk set). Good enough for an inline chat overlay.
#[derive(Debug, Clone, Default)]
pub struct ReaderIndex {
    chunks: Vec<ReaderChunk>,
    /// token → number of chunks containing it (for idf).
    df: HashMap<String, u32>,
}

impl ReaderIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Chunk budget (words per chunk) and overlap (words shared with the
    /// previous chunk so a query spanning a boundary still matches).
    pub const CHUNK_WORDS: usize = 160;
    pub const CHUNK_OVERLAP: usize = 24;

    /// Split one document into overlapping word-budgeted chunks and index them.
    pub fn add(&mut self, doc: &ReaderDocument) {
        let words: Vec<&str> = doc.text.split_whitespace().collect();
        if words.is_empty() {
            return;
        }
        let step = Self::CHUNK_WORDS.saturating_sub(Self::CHUNK_OVERLAP).max(1);
        let mut i = 0;
        let mut idx = 0;
        while i < words.len() {
            let end = (i + Self::CHUNK_WORDS).min(words.len());
            let text = words[i..end].join(" ");
            self.index_chunk(ReaderChunk {
                doc_id: doc.id.clone(),
                title: doc.title.clone(),
                index: idx,
                text,
            });
            idx += 1;
            if end == words.len() {
                break;
            }
            i += step;
        }
    }

    fn index_chunk(&mut self, chunk: ReaderChunk) {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tok in tokenize(&chunk.text) {
            if seen.insert(tok.clone()) {
                *self.df.entry(tok).or_insert(0) += 1;
            }
        }
        self.chunks.push(chunk);
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunks(&self) -> &[ReaderChunk] {
        &self.chunks
    }

    /// Retrieve the top `k` chunks for a query, best first.
    pub fn retrieve(&self, query: &str, k: usize) -> Vec<ReaderHit> {
        let q_tokens = tokenize(query);
        if q_tokens.is_empty() || self.chunks.is_empty() {
            return Vec::new();
        }
        let n = self.chunks.len() as f64;
        let mut scored: Vec<(f64, &ReaderChunk)> = Vec::with_capacity(self.chunks.len());
        for chunk in &self.chunks {
            let mut score = 0.0;
            let mut counted: std::collections::HashSet<String> = std::collections::HashSet::new();
            for tok in &q_tokens {
                if !counted.insert(tok.clone()) {
                    continue;
                }
                let df = *self.df.get(tok).unwrap_or(&0);
                if df == 0 {
                    continue;
                }
                let idf = (n / df as f64).ln() + 1.0;
                // tf must use the same tokenization as df (punctuation
                // stripped), or `"water,"` would never match a `water` query.
                let tf = tokenize(&chunk.text).iter().filter(|w| *w == tok).count() as f64;
                score += tf * idf;
            }
            if score > 0.0 {
                scored.push((score, chunk));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(k)
            .map(|(score, chunk)| ReaderHit {
                chunk: chunk.clone(),
                score,
            })
            .collect()
    }

    /// Build the chat-overlay context: the top chunks formatted for the model,
    /// with citations back to the source document + chunk.
    pub fn chat_overlay(&self, query: &str, top_k: usize) -> String {
        let hits = self.retrieve(query, top_k);
        if hits.is_empty() {
            return String::new();
        }
        let mut out = String::from("## Reader context (retrieved)\n");
        for (i, hit) in hits.iter().enumerate() {
            out.push_str(&format!(
                "\n[{i}] from `{}` (chunk {}):\n{}\n",
                hit.chunk.title, hit.chunk.index, hit.chunk.text
            ));
        }
        out
    }
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_extract_roundtrips() {
        let t = extract_text(ReaderFormat::Plain, "  hello   world  ").unwrap();
        assert_eq!(t, "hello world");
        assert_eq!(
            extract_text(ReaderFormat::Plain, "   "),
            Err(ReaderError::Empty)
        );
    }

    #[test]
    fn markdown_strips_formatting_keeps_text() {
        let md = "# Title\n\nSome **bold** and `code`.\n\n![alt text](img.png)\n\n[link text](https://x)\n\n```rust\nfn main() {}\n```\n\nTrailing.\n";
        let t = extract_text(ReaderFormat::Markdown, md).unwrap();
        assert!(t.contains("Title"));
        assert!(t.contains("Some bold and code."));
        assert!(t.contains("link text"));
        assert!(!t.contains("img.png"));
        assert!(!t.contains("fn main"), "code fences must be dropped");
        assert!(t.contains("Trailing."));
    }

    #[test]
    fn html_strips_tags_and_decodes_entities() {
        let html = "<html><head><title>x</title><style>.a{}</style></head><body><h1>Hi</h1><script>alert(1)</script><p>a &amp; b &lt; c</p></body></html>";
        let t = extract_text(ReaderFormat::Html, html).unwrap();
        assert!(t.contains("Hi"));
        assert!(t.contains("a & b < c"));
        assert!(!t.contains("alert"), "script content must be dropped");
        assert!(!t.contains(".a{}"), "style content must be dropped");
        assert!(!t.contains("<h1>"));
    }

    #[test]
    fn pdf_epub_are_seams() {
        assert!(matches!(
            extract_text(ReaderFormat::Pdf, "x"),
            Err(ReaderError::Unsupported(ReaderFormat::Pdf))
        ));
        assert!(matches!(
            extract_text(ReaderFormat::Epub, "x"),
            Err(ReaderError::Unsupported(ReaderFormat::Epub))
        ));
    }

    #[test]
    fn format_from_name() {
        assert_eq!(ReaderFormat::from_name("MD"), Some(ReaderFormat::Markdown));
        assert_eq!(ReaderFormat::from_name("pdf"), Some(ReaderFormat::Pdf));
        assert_eq!(ReaderFormat::from_name("nope"), None);
    }

    #[test]
    fn index_chunks_long_docs_with_overlap() {
        let mut idx = ReaderIndex::new();
        let words: Vec<String> = (0..500).map(|i| format!("word{i}")).collect();
        idx.add(&ReaderDocument {
            id: "d1".into(),
            title: "Long".into(),
            format: ReaderFormat::Plain,
            text: words.join(" "),
        });
        assert!(idx.chunk_count() >= 3, "500 words should make ≥3 chunks");
    }

    #[test]
    fn retrieve_ranks_relevant_chunk_first() {
        let mut idx = ReaderIndex::new();
        idx.add(&ReaderDocument {
            id: "a".into(),
            title: "Rust".into(),
            format: ReaderFormat::Markdown,
            text: "The Rust programming language gives memory safety without garbage collection. Ownership and borrowing rules are checked at compile time."
                .into(),
        });
        idx.add(&ReaderDocument {
            id: "b".into(),
            title: "Cooking".into(),
            format: ReaderFormat::Plain,
            text:
                "Baking bread requires flour, water, yeast and salt. Knead the dough until smooth."
                    .into(),
        });
        let hits = idx.retrieve("memory safety ownership", 2);
        assert_eq!(hits.len(), 1, "only the Rust doc matches the query");
        assert_eq!(
            hits[0].chunk.doc_id, "a",
            "Rust doc must be the only hit for a Rust query"
        );
        // A query matching both docs returns both; the doc with more matching
        // tokens (bread + water) ranks higher via IDF.
        let both = idx.retrieve("rust bread water", 2);
        assert_eq!(both.len(), 2);
        assert_eq!(both[0].chunk.doc_id, "b", "cooking matches bread+water");
        assert_eq!(both[1].chunk.doc_id, "a");
    }

    #[test]
    fn chat_overlay_formats_citations() {
        let mut idx = ReaderIndex::new();
        idx.add(&ReaderDocument {
            id: "a".into(),
            title: "Guide".into(),
            format: ReaderFormat::Plain,
            text: "The quick brown fox jumps over the lazy dog. The fox is quick indeed.".into(),
        });
        let overlay = idx.chat_overlay("quick fox", 1);
        assert!(overlay.contains("Reader context"));
        assert!(overlay.contains("from `Guide` (chunk 0)"));
        assert!(overlay.contains("quick brown fox"));
    }

    #[test]
    fn overlay_empty_when_no_match() {
        let idx = ReaderIndex::new();
        assert!(idx.chat_overlay("anything", 3).is_empty());
    }
}
