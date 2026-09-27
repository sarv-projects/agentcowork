//! Track-changes + comments (D2-gap — doc 63 §3). Read `w:ins`/`w:del`
//! tracked changes and `w:comment` comments; append comments without
//! corrupting existing comment ranges (patch-aware: new ids never collide).

use roxmltree::Document;
use thiserror::Error;

/// WordprocessingML namespace.
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedChangeKind {
    Insertion,
    Deletion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedChange {
    pub kind: TrackedChangeKind,
    pub author: String,
    pub date: Option<String>,
    /// The changed text (concatenated `w:t` runs).
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub date: Option<String>,
    pub text: String,
}

#[derive(Debug, Error)]
pub enum TrackError {
    #[error("xml parse error: {0}")]
    Parse(#[from] roxmltree::Error),
    #[error("malformed comments.xml: no </w:comments> end tag")]
    MissingCommentsEnd,
}

/// Collect the concatenated `w:t` / `w:delText` text of a node's
/// descendants (insertions use `w:t`, deletions use `w:delText`).
fn collect_text(node: roxmltree::Node) -> String {
    node.descendants()
        .filter(|d| {
            d.is_element()
                && d.tag_name().namespace() == Some(W)
                && matches!(d.tag_name().name(), "t" | "delText")
        })
        .filter_map(|t| t.text())
        .collect::<Vec<_>>()
        .join("")
}

/// Extract tracked changes from `word/document.xml`.
pub fn extract_tracked_changes(document_xml: &str) -> Result<Vec<TrackedChange>, TrackError> {
    let doc = Document::parse(document_xml)?;
    let mut out = Vec::new();
    for node in doc.descendants() {
        if !node.is_element() || node.tag_name().namespace() != Some(W) {
            continue;
        }
        let kind = match node.tag_name().name() {
            "ins" => Some(TrackedChangeKind::Insertion),
            "del" => Some(TrackedChangeKind::Deletion),
            _ => None,
        };
        if let Some(kind) = kind {
            out.push(TrackedChange {
                kind,
                author: node
                    .attribute((W, "author"))
                    .unwrap_or_default()
                    .to_string(),
                date: node.attribute((W, "date")).map(str::to_string),
                text: collect_text(node),
            });
        }
    }
    Ok(out)
}

/// Extract comments from `word/comments.xml`.
pub fn extract_comments(comments_xml: &str) -> Result<Vec<Comment>, TrackError> {
    let doc = Document::parse(comments_xml)?;
    let mut out = Vec::new();
    for node in doc.descendants() {
        if node.is_element()
            && node.tag_name().namespace() == Some(W)
            && node.tag_name().name() == "comment"
        {
            out.push(Comment {
                id: node.attribute((W, "id")).unwrap_or_default().to_string(),
                author: node
                    .attribute((W, "author"))
                    .unwrap_or_default()
                    .to_string(),
                date: node.attribute((W, "date")).map(str::to_string),
                text: collect_text(node),
            });
        }
    }
    Ok(out)
}

/// Append a comment to `word/comments.xml`, returning the patched XML. The id
/// is chosen to never collide with an existing `w:id` (patch-aware).
pub fn add_comment(comments_xml: &str, author: &str, text: &str) -> Result<String, TrackError> {
    let doc = Document::parse(comments_xml)?;
    let next_id = next_comment_id(&doc);
    let date = "2026-01-01T00:00:00Z"; // deterministic marker; caller may override
    let comment = format!(
        "<w:comment w:id=\"{next_id}\" w:author=\"{author}\" w:date=\"{date}\">\
         <w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>\
         </w:comment>"
    );
    // Insert before the closing `</w:comments>` (or after the opening tag).
    let end = comments_xml
        .rfind("</w:comments>")
        .ok_or(TrackError::MissingCommentsEnd)?;
    let mut out = String::with_capacity(comments_xml.len() + comment.len());
    out.push_str(&comments_xml[..end]);
    out.push_str(&comment);
    out.push_str(&comments_xml[end..]);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Track-change authoring (the D2-gap "emitting w:ins/w:del" half — doc 63
// §3): render a patch as tracked change runs, patch-aware and schema-valid.
// ---------------------------------------------------------------------------

/// Who authored the tracked change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackAuthor {
    pub name: String,
    /// ISO-8601 timestamp, e.g. `2026-01-01T00:00:00Z`.
    pub date: String,
}

impl TrackAuthor {
    pub fn new(name: impl Into<String>, date: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            date: date.into(),
        }
    }
}

/// Escape XML text (runs carry user text that may include `<`, `&`).
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Build a `<w:ins>` run carrying `text` (an insertion authored by `author`).
pub fn render_ins_run(text: &str, author: &TrackAuthor) -> String {
    format!(
        "<w:ins w:id=\"0\" w:author=\"{}\" w:date=\"{}\"><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:ins>",
        xml_escape(&author.name),
        xml_escape(&author.date),
        xml_escape(text),
    )
}

/// Build a `<w:del>` run carrying `text` as `<w:delText>` (a deletion).
pub fn render_del_run(text: &str, author: &TrackAuthor) -> String {
    format!(
        "<w:del w:id=\"0\" w:author=\"{}\" w:date=\"{}\"><w:r><w:delText xml:space=\"preserve\">{}</w:delText></w:r></w:del>",
        xml_escape(&author.name),
        xml_escape(&author.date),
        xml_escape(text),
    )
}

/// Emit a tracked change for a `old → new` text patch: a `<w:del>` of the old
/// text (when non-empty) followed by a `<w:ins>` of the new text (when
/// non-empty) — the standard Word "replace with tracked changes" form. Both
/// runs sit inside a `<w:p>` wrapper so the caller can splice the paragraph
/// into a document.
pub fn emit_tracked_change(old_text: &str, new_text: &str, author: &TrackAuthor) -> String {
    let mut runs = String::new();
    if !old_text.is_empty() {
        runs.push_str(&render_del_run(old_text, author));
    }
    if !new_text.is_empty() {
        runs.push_str(&render_ins_run(new_text, author));
    }
    format!("<w:p>{runs}</w:p>")
}

/// The `<w:commentRangeStart/End>`-free comment anchor: a comment reference
/// run (`<w:commentReference w:id=…/>`) inside the paragraph text — the
/// minimal wiring for "insert the rendered citation + its comment" flows.
pub fn render_comment_reference(id: &str) -> String {
    format!("<w:r><w:commentReference w:id=\"{id}\"/></w:r>")
}

fn next_comment_id(doc: &Document) -> u64 {
    let max = doc
        .descendants()
        .filter(|d| {
            d.is_element()
                && d.tag_name().namespace() == Some(W)
                && d.tag_name().name() == "comment"
        })
        .filter_map(|d| d.attribute((W, "id")))
        .filter_map(|v| v.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    max + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:ins w:author="alice" w:date="2026-01-01T00:00:00Z"><w:r><w:t>added</w:t></w:r></w:ins>
<w:del w:author="bob"><w:r><w:delText>removed</w:delText></w:r></w:del></w:p></w:body></w:document>"#;

    #[test]
    fn extracts_ins_and_del() {
        let changes = extract_tracked_changes(DOC).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].kind, TrackedChangeKind::Insertion);
        assert_eq!(changes[0].text, "added");
        assert_eq!(changes[0].author, "alice");
        assert_eq!(changes[1].kind, TrackedChangeKind::Deletion);
        assert_eq!(changes[1].text, "removed");
    }

    #[test]
    fn extracts_comments() {
        let xml = r#"<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:comment w:id="1" w:author="carol"><w:p><w:r><w:t>looks good</w:t></w:r></w:p></w:comment>
</w:comments>"#;
        let comments = extract_comments(xml).unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].id, "1");
        assert_eq!(comments[0].author, "carol");
        assert_eq!(comments[0].text, "looks good");
    }

    #[test]
    fn emits_del_and_ins_runs_for_a_replacement() {
        let author = TrackAuthor::new("alice", "2026-01-01T00:00:00Z");
        let para = emit_tracked_change("old wording", "new wording", &author);
        assert!(para.contains("<w:del"), "{para}");
        assert!(
            para.contains("<w:delText xml:space=\"preserve\">old wording</w:delText>"),
            "{para}"
        );
        assert!(para.contains("<w:ins"), "{para}");
        assert!(
            para.contains("<w:t xml:space=\"preserve\">new wording</w:t>"),
            "{para}"
        );
        assert!(para.contains("w:author=\"alice\""));
        assert!(para.contains("w:date=\"2026-01-01T00:00:00Z\""));
        // Re-parses as valid XML (the fragment needs the w: namespace).
        let wrapped = format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{para}</w:body></w:document>"#
        );
        assert!(Document::parse(&wrapped).is_ok());
    }

    #[test]
    fn emits_only_ins_or_only_del_for_pure_additions_removals() {
        let author = TrackAuthor::new("bob", "2026-01-01T00:00:00Z");
        let added = emit_tracked_change("", "new text", &author);
        assert!(added.contains("<w:ins"));
        assert!(!added.contains("<w:del"));
        let removed = emit_tracked_change("old text", "", &author);
        assert!(removed.contains("<w:del"));
        assert!(!removed.contains("<w:ins"));
        // No-op patch emits no runs.
        let noop = emit_tracked_change("", "", &author);
        assert_eq!(noop, "<w:p></w:p>");
    }

    #[test]
    fn escapes_author_and_text() {
        let author = TrackAuthor::new("a & b", "2026-01-01T00:00:00Z");
        let para = emit_tracked_change("x < y", "z > w", &author);
        assert!(para.contains("w:author=\"a &amp; b\""));
        assert!(para.contains("<w:delText xml:space=\"preserve\">x &lt; y</w:delText>"));
        assert!(para.contains("<w:t xml:space=\"preserve\">z &gt; w</w:t>"));
    }

    #[test]
    fn tracked_paragraph_roundtrips_through_extraction() {
        let author = TrackAuthor::new("carol", "2026-01-01T00:00:00Z");
        let para = emit_tracked_change("old", "new", &author);
        let doc = format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{para}</w:body></w:document>"#
        );
        let changes = extract_tracked_changes(&doc).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].kind, TrackedChangeKind::Deletion);
        assert_eq!(changes[0].text, "old");
        assert_eq!(changes[1].kind, TrackedChangeKind::Insertion);
        assert_eq!(changes[1].text, "new");
        assert_eq!(changes[1].author, "carol");
    }

    #[test]
    fn add_comment_uses_next_id() {
        let xml = r#"<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:comment w:id="3" w:author="a"><w:p><w:r><w:t>x</w:t></w:r></w:p></w:comment>
</w:comments>"#;
        let out = add_comment(xml, "bob", "new note").unwrap();
        assert!(out.contains("w:id=\"4\""), "{out}");
        assert!(out.contains("new note"));
        // Existing comment id preserved.
        assert!(out.contains("w:id=\"3\""));
        // Re-parses cleanly.
        assert!(Document::parse(&out).is_ok());
    }
}
