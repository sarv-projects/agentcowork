//! Field-character balance verification (ARCH/04 §4.6 — field balancing).
//!
//! A Word complex field is a `begin` … `separate` … `end` triple of
//! `w:fldChar` markers (page numbers, TOC, hyperlinks, cross-references, …).
//! A surgical patch that drops or duplicates one of those markers produces a
//! document Word flags as corrupt and offers to repair. Nothing in the
//! `w:t` byte-surgery path can notice that on its own, so this module walks
//! the patched part after every patch and again before commit, and **refuses**
//! when the markers are not balanced and properly nested.
//!
//! The walk is a single stack over `Document::descendants()`, which yields
//! elements in document order:
//!
//! - `begin` pushes a new open field (a 1-based field index);
//! - `separate` requires an open field that has not been separated yet;
//! - `end` pops the innermost open field.
//!
//! Legal shapes, all accepted: `begin … end` (a **dirty field** — the
//! instruction has no cached result and Word recomputes it), `begin …
//! separate … end`, and any nesting of those. Rejected, with the offending
//! part and field index named: a `separate` with no open field, a second
//! `separate` in one field, an `end` with no matching `begin`, and a `begin`
//! that is never closed.
//!
//! Marker detection is namespace-agnostic on the *local* name `fldChar` plus
//! its type attribute, so it works for `w:fldChar` and for any other prefix
//! bound to the same schema, while never matching DrawingML's self-contained
//! `a:fld` (a different element that has no begin/end triple at all).

use roxmltree::Node;

use crate::xml;

/// The three `fldChar` types, in the order the schema defines them.
pub const FIELD_CHAR_TYPES: [&str; 3] = ["begin", "separate", "end"];

/// One field imbalance, located precisely enough to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldBalanceError {
    /// The part the imbalance is in (e.g. `word/document.xml`).
    pub part: String,
    /// 1-based field index: the `begin` that opened the field, or the index
    /// the stray marker *would* have had.
    pub field: usize,
    /// What is wrong, in plain words.
    pub detail: String,
}

impl std::fmt::Display for FieldBalanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unbalanced field characters: field #{} in {}: {}",
            self.field, self.part, self.detail
        )
    }
}

impl std::error::Error for FieldBalanceError {}

/// What the balance walk found in one part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldReport {
    /// Number of complete fields (`begin` … `end`) seen.
    pub fields: usize,
    /// How many of them had no `separate` (legal dirty fields).
    pub dirty: usize,
}

impl FieldReport {
    /// True when the part contains no complex fields at all.
    pub fn is_empty(&self) -> bool {
        self.fields == 0
    }
}

/// What the balance walk can fail with: the part did not parse, or its field
/// markers are not balanced.
#[derive(Debug, thiserror::Error)]
pub enum FieldCheckError {
    /// The part is not parseable XML (reported, never silently passed).
    #[error("part {part} is not parseable XML: {source}")]
    Xml {
        part: String,
        #[source]
        source: crate::xml::OfficeXmlError,
    },
    /// The markers are unbalanced or wrongly nested.
    #[error(transparent)]
    Unbalanced(#[from] FieldBalanceError),
}

/// A field currently open in the walk.
#[derive(Debug, Clone, Copy)]
struct OpenField {
    index: usize,
    separated: bool,
}

/// Walk `bytes` (one WordprocessingML part) and verify field balance.
///
/// Returns the field counts on success, or the first imbalance found.
pub fn check_part(part: &str, bytes: &[u8]) -> Result<FieldReport, FieldCheckError> {
    let doc = xml::parse(bytes).map_err(|source| FieldCheckError::Xml {
        part: part.to_string(),
        source,
    })?;

    let mut open: Vec<OpenField> = Vec::new();
    let mut fields = 0usize;
    let mut dirty = 0usize;

    for node in doc.descendants().filter(|n| n.is_element()) {
        let Some(kind) = field_char_type(node) else {
            continue;
        };
        match kind {
            "begin" => {
                fields += 1;
                open.push(OpenField {
                    index: fields,
                    separated: false,
                });
            }
            "separate" => match open.last_mut() {
                None => {
                    return Err(unbalanced(
                        part,
                        fields + 1,
                        "'separate' marker with no 'begin' marker before it",
                    )
                    .into());
                }
                Some(f) if f.separated => {
                    return Err(unbalanced(
                        part,
                        f.index,
                        "a second 'separate' marker in a field that is already separated",
                    )
                    .into());
                }
                Some(f) => f.separated = true,
            },
            "end" => match open.pop() {
                // A `begin … end` with no `separate` is a legal dirty field.
                Some(f) if !f.separated => dirty += 1,
                Some(_) => {}
                None => {
                    return Err(unbalanced(
                        part,
                        fields + 1,
                        "'end' marker with no matching 'begin' marker",
                    )
                    .into());
                }
            },
            _ => {}
        }
    }

    if let Some(f) = open.last() {
        return Err(unbalanced(
            part,
            f.index,
            "'begin' marker that is never closed by an 'end' marker",
        )
        .into());
    }

    Ok(FieldReport { fields, dirty })
}

/// Same as [`check_part`], flattened into the crate error so a mutating path
/// can propagate a single error type.
pub fn verify(part: &str, bytes: &[u8]) -> Result<FieldReport, crate::OfficeError> {
    match check_part(part, bytes) {
        Ok(r) => Ok(r),
        Err(FieldCheckError::Xml { source, .. }) => Err(crate::OfficeError::Xml(source)),
        Err(FieldCheckError::Unbalanced(e)) => Err(crate::OfficeError::FieldBalance {
            part: e.part,
            field: e.field,
            detail: e.detail,
        }),
    }
}

fn unbalanced(part: &str, field: usize, detail: &str) -> FieldBalanceError {
    FieldBalanceError {
        part: part.to_string(),
        field,
        detail: detail.to_string(),
    }
}

/// The `fldChar` type of a marker element, or `None` when the element is not a
/// field-character marker (including a `fldChar` with an unknown type, which
/// the schema does not define).
fn field_char_type(node: Node) -> Option<&'static str> {
    if node.tag_name().name() != "fldChar" {
        return None;
    }
    let raw = node
        .attribute((xml::W, "fldCharType"))
        .or_else(|| node.attribute("w:fldCharType"))
        .or_else(|| node.attribute("fldCharType"))?;
    FIELD_CHAR_TYPES.into_iter().find(|t| *t == raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(inner: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{inner}</w:body></w:document>"#
        )
        .into_bytes()
    }

    /// The imbalance a check reported (panics on any other outcome).
    fn imbalance(err: FieldCheckError) -> FieldBalanceError {
        match err {
            FieldCheckError::Unbalanced(e) => e,
            other => panic!("expected an imbalance, got {other}"),
        }
    }

    #[test]
    fn complete_field_is_balanced() {
        let xml = part(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:instrText>PAGE</w:instrText></w:r>
               <w:r><w:fldChar w:fldCharType="separate"/></w:r>
               <w:r><w:t>1</w:t></w:r>
               <w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        );
        let r = check_part("word/document.xml", &xml).unwrap();
        assert_eq!(r.fields, 1);
        assert_eq!(r.dirty, 0);
    }

    #[test]
    fn field_without_separate_is_a_legal_dirty_field() {
        // A dirty field (begin … end, no separate) must NOT be rejected.
        let xml = part(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:instrText>DATE</w:instrText></w:r>
               <w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        );
        let r = check_part("word/document.xml", &xml).unwrap();
        assert_eq!(r.fields, 1);
        assert_eq!(r.dirty, 1);
    }

    #[test]
    fn nested_fields_are_balanced() {
        let xml = part(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:instrText>TOC</w:instrText></w:r>
               <w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:instrText>PAGE</w:instrText></w:r>
               <w:r><w:fldChar w:fldCharType="separate"/></w:r>
               <w:r><w:t>1</w:t></w:r>
               <w:r><w:fldChar w:fldCharType="end"/></w:r>
               <w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        );
        let r = check_part("word/document.xml", &xml).unwrap();
        assert_eq!(r.fields, 2);
        assert_eq!(r.dirty, 1);
    }

    #[test]
    fn unclosed_begin_names_the_part_and_field_index() {
        let xml = part(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:t>x</w:t></w:r></w:p>
               <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:instrText>PAGE</w:instrText></w:r>
               <w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        );
        let err = imbalance(check_part("word/header1.xml", &xml).unwrap_err());
        assert_eq!(err.part, "word/header1.xml");
        assert_eq!(err.field, 1, "the innermost unclosed field is reported");
        assert!(err.detail.contains("never closed"), "{err}");
    }

    #[test]
    fn stray_end_is_rejected() {
        let xml = part(r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#);
        let err = imbalance(check_part("word/document.xml", &xml).unwrap_err());
        assert_eq!(err.field, 1);
        assert!(err.detail.contains("no matching 'begin'"), "{err}");
    }

    #[test]
    fn stray_separate_is_rejected() {
        let xml = part(r#"<w:p><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>"#);
        let err = imbalance(check_part("word/document.xml", &xml).unwrap_err());
        assert_eq!(err.field, 1);
        assert!(err.detail.contains("no 'begin'"), "{err}");
    }

    #[test]
    fn duplicate_separate_is_rejected() {
        let xml = part(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>
               <w:r><w:fldChar w:fldCharType="separate"/></w:r>
               <w:r><w:fldChar w:fldCharType="separate"/></w:r>
               <w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        );
        let err = imbalance(check_part("word/document.xml", &xml).unwrap_err());
        assert_eq!(err.field, 1);
        assert!(err.detail.contains("second 'separate'"), "{err}");
    }

    #[test]
    fn a_part_without_fields_reports_zero() {
        let r = check_part("word/document.xml", &crate::zip::tests::DOCUMENT_XML).unwrap();
        assert!(r.is_empty());
        assert_eq!(r.fields, 0);
    }

    #[test]
    fn drawingml_fld_is_not_a_field_character_triple() {
        // `a:fld` is a self-contained element: it must not be read as an
        // unclosed field.
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><a:fld id="{B8B0B1B0-0000-0000-0000-000000000000}" type="slidenum"><a:t>3</a:t></a:fld></p:sld>"#;
        assert!(check_part("ppt/slides/slide1.xml", xml).unwrap().is_empty());
    }

    #[test]
    fn an_unparseable_part_is_reported_not_silently_passed() {
        let err = check_part("word/document.xml", b"<w:document><unclosed>").unwrap_err();
        assert!(matches!(err, FieldCheckError::Xml { .. }), "{err}");
    }
}
