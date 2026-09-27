//! P2.3 — `read` tool upgrade (doc 55 read.rs semantics).
//!
//! Three paths, in order of preference:
//! 1. **No-browser HTTP path** (fast, no Chrome needed): `Accept:
//!    text/markdown` negotiation, `.md` retry, then a **nearest-ancestor
//!    `llms.txt`/`llms-full.txt` walk** (the agent-browser trick — sites
//!    that ship LLM context expose it at a stable path).
//! 2. **Browser DOM walker** (`BrowserActions::read`) — for pages that need
//!    JS rendering.
//! 3. `--filter` (keep matching lines) and `--outline` (headings+links)
//!    modes apply on any path.
//!
//! Body cap: 2MB (doc 55 read.rs) — larger output is truncated with a note.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 2MB body cap (doc 55 read.rs).
pub const READ_BODY_CAP: usize = 2 * 1024 * 1024;

/// Result of the no-browser read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpReadResult {
    pub markdown: String,
    pub source: ReadSource,
    pub truncated: bool,
}

/// Where the content came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
    /// The URL itself with `Accept: text/markdown`.
    MarkdownNegotiated,
    /// The URL with `.md` appended/retried.
    MdSuffix,
    /// A nearest-ancestor `llms.txt` / `llms-full.txt`.
    LlmsTxt { found_at: String },
    /// Plain HTTP fallback (no markdown variant offered).
    PlainHtml,
    /// Rendered through an engine's DOM walker (tier 1/2, P2.4).
    DomWalked,
    /// Nothing readable was found.
    NotFound,
}

/// Options mirroring agent-browser's `--filter`/`--outline`/`--raw`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReadOptions {
    /// Keep only lines matching this regex (applied to markdown output).
    pub filter: Option<String>,
    /// Headings + links only.
    pub outline: bool,
    /// Raw text (no markdown syntax).
    pub raw: bool,
}

/// Fetch + negotiate markdown from a URL without a browser (doc 55 read.rs).
///
/// `client` is a `ureq::Agent` (caller-owned so tests can inject a mock
/// transport); `base_url` is the page the user asked to read. Returns the
/// negotiated markdown or a structured not-found.
pub fn read_http(
    agent: &ureq::Agent,
    base_url: &str,
    opts: &ReadOptions,
) -> Result<HttpReadResult, Box<ureq::Error>> {
    // 1. Accept: text/markdown on the URL itself.
    let resp = agent
        .get(base_url)
        .set("Accept", "text/markdown, text/plain;q=0.9, text/html;q=0.5")
        .call()?;
    let mut markdown = read_bounded(resp);
    let mut source = ReadSource::MarkdownNegotiated;
    if looks_like_html(&markdown) {
        // 2. Try the `.md` suffix (common for llms.md-style endpoints).
        let md_url = append_md_suffix(base_url);
        if let Ok(r2) = agent.get(&md_url).set("Accept", "text/markdown").call() {
            let md = read_bounded(r2);
            if !looks_like_html(&md) {
                markdown = md;
                source = ReadSource::MdSuffix;
            }
        }
    }
    if looks_like_html(&markdown) {
        // 3. Nearest-ancestor llms.txt / llms-full.txt walk.
        if let Some((llms_url, body)) = walk_llms_txt(agent, base_url) {
            markdown = body;
            source = ReadSource::LlmsTxt { found_at: llms_url };
        } else {
            source = ReadSource::PlainHtml;
        }
    }
    if markdown.trim().is_empty() && !matches!(source, ReadSource::PlainHtml) {
        source = ReadSource::NotFound;
    }
    let truncated = markdown.len() > READ_BODY_CAP;
    // Apply --filter / --outline / --raw on any path (doc 55 semantics).
    let markdown = apply_options(&markdown, opts);
    // G9 read-cleaner (P2.11): strip ad/tracker links + consent walls with the
    // deterministic default filter set before the text reaches the model.
    let cleaned =
        crate::content::clean_markdown(&crate::content::default_filter_set(), base_url, &markdown);
    Ok(HttpReadResult {
        markdown: cleaned.text,
        source,
        truncated,
    })
}

fn read_bounded(resp: ureq::Response) -> String {
    use std::io::Read;
    let mut body = String::new();
    resp.into_reader()
        .take(READ_BODY_CAP as u64 + 1)
        .read_to_string(&mut body)
        .unwrap_or_default();
    body
}

/// Very cheap HTML sniff — markdown pages don't start with `<!doctype html>`.
pub(crate) fn looks_like_html(s: &str) -> bool {
    let head = &s[..s.len().min(512)].to_lowercase();
    head.contains("<!doctype html") || head.contains("<html") || head.contains("<head")
}

/// `https://host/a/b/page` → `https://host/a/b/page.md`-style candidates and
/// ancestor llms.txt paths. Returns (url, body) of the first that yields
/// non-HTML content.
fn walk_llms_txt(agent: &ureq::Agent, base_url: &str) -> Option<(String, String)> {
    let mut path = url_path(base_url);
    // Walk ancestors from the page's directory upward to the root.
    let mut candidates = Vec::new();
    loop {
        let dir = parent_path(&path);
        candidates.push(format!("{dir}llms.txt"));
        candidates.push(format!("{dir}llms-full.txt"));
        if dir.is_empty() || dir == "/" {
            break;
        }
        path = dir;
        // Stop after a few hops — don't walk to a foreign host root forever.
        if candidates.len() >= 8 {
            break;
        }
    }
    for cand in candidates {
        let full = join_url(base_url, &cand);
        if let Ok(resp) = agent.get(&full).set("Accept", "text/markdown").call() {
            let body = read_bounded(resp);
            if !looks_like_html(&body) && !body.trim().is_empty() {
                return Some((full, body));
            }
        }
    }
    None
}

/// Extract the path portion of a URL (with leading slash).
fn url_path(url: &str) -> String {
    url.split("://")
        .nth(1)
        .map(|rest| {
            let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("/");
            path.to_string()
        })
        .unwrap_or_else(|| "/".into())
}

/// `/a/b/page` → `/a/b/`
fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => {
            let parent = &trimmed[..=i];
            if parent.is_empty() {
                "/".into()
            } else {
                parent.into()
            }
        }
        None => "/".into(),
    }
}

/// Append `.md` to the last path segment (before any query/fragment).
fn append_md_suffix(url: &str) -> String {
    let (base, suffix) = split_query_fragment(url);
    let mut s = base.to_string();
    if !s.ends_with('/') {
        s.push_str(".md");
    }
    if let Some(suf) = suffix {
        s.push_str(suf);
    }
    s
}

fn split_query_fragment(url: &str) -> (&str, Option<&str>) {
    match url.find(['?', '#']) {
        Some(i) => (&url[..i], Some(&url[i..])),
        None => (url, None),
    }
}

/// Join a possibly-relative path onto a base URL.
fn join_url(base: &str, path: &str) -> String {
    let (scheme_host, _) = match base.find("://") {
        Some(i) => (&base[..i + 3], &base[i + 3..]),
        None => return path.to_string(),
    };
    let host = base[scheme_host.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let mut out = format!("{scheme_host}{host}");
    if path.starts_with('/') {
        out.push_str(path);
    } else {
        out.push('/');
        out.push_str(path);
    }
    out
}

/// Apply `--filter` / `--outline` / `--raw` post-processing to markdown.
pub fn apply_options(markdown: &str, opts: &ReadOptions) -> String {
    let mut text = markdown.to_string();
    if opts.raw {
        // Crude markdown stripping for the raw mode.
        text = text
            .lines()
            .map(|l| l.trim_start_matches(['#', '>', '-', '*', '|', ' ']).trim())
            .collect::<Vec<_>>()
            .join("\n");
    } else if opts.outline {
        text = text
            .lines()
            .filter(|l| l.starts_with('#') || l.starts_with("- [") || l.starts_with("|"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(filter) = &opts.filter {
        if let Ok(re) = regex::Regex::new(filter) {
            text = text
                .lines()
                .filter(|l| re.is_match(l))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    text
}

/// Route oversized output to a file when the caller asks (OutputFileAccess
/// pattern, doc 33 §6.1) — returns the file path if written.
pub fn maybe_route_to_file(text: &str, dir: &Path, name: &str, cap: usize) -> Option<String> {
    if text.len() <= cap {
        return None;
    }
    let path = dir.join(format!("{name}.md"));
    let _ = std::fs::write(&path, text);
    Some(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_md_suffix_handles_query() {
        assert_eq!(
            append_md_suffix("https://a.com/page?x=1"),
            "https://a.com/page.md?x=1"
        );
        assert_eq!(
            append_md_suffix("https://a.com/docs/guide"),
            "https://a.com/docs/guide.md"
        );
    }

    #[test]
    fn parent_path_walks_up() {
        assert_eq!(parent_path("/a/b/c"), "/a/b/");
        assert_eq!(parent_path("/a/b/"), "/a/");
        assert_eq!(parent_path("/"), "/");
    }

    #[test]
    fn looks_like_html_sniffs() {
        assert!(looks_like_html("<!doctype html><html>"));
        assert!(!looks_like_html("# Markdown title"));
        assert!(!looks_like_html("plain text body"));
    }

    #[test]
    fn apply_options_outline_and_filter() {
        let md = "# H1\n\npara line\n\n## H2\n\n- [link](x)\n\n| a | b |\n";
        let out = apply_options(
            md,
            &ReadOptions {
                outline: true,
                ..Default::default()
            },
        );
        assert!(out.contains("# H1"));
        assert!(out.contains("## H2"));
        assert!(!out.contains("para line"));
        let filt = apply_options(
            md,
            &ReadOptions {
                filter: Some("H2".into()),
                ..Default::default()
            },
        );
        assert!(filt.contains("## H2"));
        assert!(!filt.contains("# H1"));
    }

    #[test]
    fn raw_mode_strips_markdown_syntax() {
        let out = apply_options(
            "# T\n- item\n",
            &ReadOptions {
                raw: true,
                ..Default::default()
            },
        );
        assert!(!out.contains('#'));
    }

    #[test]
    fn maybe_route_to_file_writes_over_cap() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-read-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let big = "x".repeat(100);
        assert!(maybe_route_to_file(&big, &dir, "big", 10).is_some());
        assert!(maybe_route_to_file(&big, &dir, "small", 1000).is_none());
    }

    #[test]
    fn walk_llms_txt_needs_network_so_stays_local() {
        // Pure logic tests above; the HTTP path is exercised live (see
        // live_tests.rs `read_http` against a local mock server).
        assert_eq!(parent_path("/a/b/llms.txt"), "/a/b/");
        assert_eq!(url_path("https://x.example/a/b"), "/a/b");
    }
}
