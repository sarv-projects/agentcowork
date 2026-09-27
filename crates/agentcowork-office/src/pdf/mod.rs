//! PDF engine (P4.4 — D4, ARCH/04 §4.2 PDF).
//!
//! Four edit modes, one per operation (the honest "detect the operation,
//! offer the right mode" boundary from ARCH/04 §4.2):
//! 1. **Form fill** — set AcroForm `/V` values (`form::form_fill`).
//! 2. **Text swap** — exact-match single-token replacement in a page's `Tj`
//!    text (`replace_text`) — layout preserved because glyph positions are
//!    untouched; never reflow.
//! 3. **Redaction** — **removal** of the drawing operators that render into a
//!    rectangle (`redact::redact`), with a post-op text-extraction proof
//!    (`redact::redact_checked`). The marking half is available separately as
//!    `redact::mark_for_redaction` and is explicitly *not* redaction.
//! 4. **Re-author** — build a brand-new PDF from text (`author::author_pages`)
//!    for structural edits instead of corrupting the source.
//!
//! Rendering is the webview's job (pdf.js, P4.7 PDF viewer).

pub mod annot;
pub mod author;
pub mod form;
pub mod pages;
pub mod redact;
pub mod storage;

use lopdf::Document;

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("lopdf error: {0}")]
    Lopdf(#[from] lopdf::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("document has no AcroForm")]
    NoAcroForm,
    #[error("page not found: {0}")]
    PageNotFound(u32),
    #[error("nothing to redact: no rectangles were supplied")]
    NothingToRedact,
    #[error("redaction rectangle on page {0} is not finite: {1:?}")]
    InvalidRect(u32, [f32; 4]),
    #[error(
        "the document is encrypted; redaction needs decrypted content streams and this engine does not drive decryption"
    )]
    Encrypted,
    #[error(
        "the content stream of page {page} could not be decoded, so nothing on it was examined"
    )]
    UndecodableContent { page: u32 },
    #[error(
        "redaction refused: {} item(s) intersect the redaction area but cannot be removed by this engine",
        .0.len()
    )]
    Unremovable(Vec<redact::Unremovable>),
    #[error("redaction refused: {} target string(s) survived the removal", .0.len())]
    RemovalUnproven(Vec<(String, u32)>),
}

/// Exact-match text swap on one page's content stream (`Tj` text only).
pub fn replace_text(
    bytes: &[u8],
    page: u32,
    find: &str,
    replace: &str,
) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(bytes)?;
    doc.replace_text(page, find, replace)?;
    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

/// Read-only inspection (H5 PDF viewer): page count + per-page extracted text.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PdfInfo {
    pub pages: u32,
    pub texts: Vec<String>,
}

/// Page count + per-page text (1-indexed).
pub fn inspect(bytes: &[u8]) -> Result<PdfInfo, PdfError> {
    let doc = Document::load_mem(bytes)?;
    let pages = doc.get_pages().len() as u32;
    let texts = (1..=pages)
        .map(|p| doc.extract_text(&[p]).unwrap_or_default())
        .collect();
    Ok(PdfInfo { pages, texts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, dictionary};

    /// A one-page PDF with "Hello" text + an AcroForm field "name".
    fn acro_pdf() -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{Document, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new("Tj", vec![Object::string_literal("Hello")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );

        // AcroForm with one text field named "name".
        let field_id = doc.add_object(dictionary! {
            "FT" => "Tx",
            "T" => Object::string_literal("name"),
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => vec![0.into(), 0.into(), 100.into(), 20.into()],
        });
        let acro_id = doc.add_object(dictionary! {
            "Fields" => vec![field_id.into()],
            "DA" => Object::string_literal("/F1 12 Tf 0 g"),
        });
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => acro_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.compress();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// Read a field's `/V` value back out of a document (test oracle).
    fn field_value(bytes: &[u8], name: &str) -> Option<String> {
        let doc = Document::load_mem(bytes).unwrap();
        let catalog = doc.catalog().unwrap();
        let acro = catalog.get(b"AcroForm").unwrap().clone();
        let (_id, acro) = doc.dereference(&acro).unwrap();
        let fields = acro.as_dict().unwrap().get(b"Fields").unwrap();
        let arr = match fields {
            Object::Array(a) => a.clone(),
            Object::Reference(id) => doc.get_object(*id).unwrap().as_array().unwrap().clone(),
            _ => return None,
        };
        for f in arr {
            let fid = f.as_reference().ok()?;
            let d = doc.get_dictionary(fid).unwrap();
            let t = d
                .get(b"T")
                .ok()
                .and_then(|o| o.as_str().ok())
                .map(|s| String::from_utf8_lossy(s).into_owned());
            if t.as_deref() == Some(name) {
                return d
                    .get(b"V")
                    .ok()
                    .and_then(|o| o.as_str().ok())
                    .map(|s| String::from_utf8_lossy(s).into_owned());
            }
        }
        None
    }

    #[test]
    fn author_pages_builds_readable_pdf() {
        let bytes = author::author_pages(&["Hello world", "Second page"]).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let doc = Document::load_mem(&bytes).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
        let text = doc.extract_text(&[1, 2]).unwrap();
        assert!(text.contains("Hello world"));
        assert!(text.contains("Second page"));
    }

    #[test]
    fn inspect_reports_pages_and_text() {
        let bytes = author::author_pages(&["one", "two"]).unwrap();
        let info = inspect(&bytes).unwrap();
        assert_eq!(info.pages, 2);
        assert!(info.texts[0].contains("one"));
        assert!(info.texts[1].contains("two"));
    }

    #[test]
    fn replace_text_swaps_tj_text() {
        let original = author::author_pages(&["Hello"]).unwrap();
        let swapped = replace_text(&original, 1, "Hello", "Goodbye").unwrap();
        let doc = Document::load_mem(&swapped).unwrap();
        let text = doc.extract_text(&[1]).unwrap();
        assert!(text.contains("Goodbye"));
        assert!(!text.contains("Hello"));
    }

    #[test]
    fn form_fill_sets_field_value() {
        let original = acro_pdf();
        let filled =
            form::form_fill(&original, &[("name".to_string(), "Alice".to_string())]).unwrap();
        assert_eq!(field_value(&filled, "name").as_deref(), Some("Alice"));
    }

    #[test]
    fn form_fill_errors_without_acroform() {
        let original = author::author_pages(&["plain"]).unwrap();
        let err = form::form_fill(&original, &[("name".into(), "Alice".into())]).unwrap_err();
        assert!(matches!(err, PdfError::NoAcroForm));
    }

    #[test]
    fn redact_removes_content_rather_than_annotating() {
        // The v0 defect: `redact` annotated and left the text extractable.
        // The proof is the extraction oracle, not the presence of a shape.
        let original = author::author_pages(&["secret"]).unwrap();
        let doc = Document::load_mem(&original).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let content = String::from_utf8_lossy(&doc.get_page_content(page_id).unwrap()).into_owned();
        // 12pt Courier at (72,720): "secret" is 43.2pt wide, 12pt tall.
        let redacted = redact::redact(&original, &[(1, [70.0, 716.0, 120.0, 724.0])]).unwrap();

        let after = Document::load_mem(&redacted).unwrap();
        let text = after.extract_text(&[1]).unwrap();
        assert!(
            !text.contains("secret"),
            "redaction left text behind: {text:?}"
        );
        let after_content =
            String::from_utf8_lossy(&after.get_page_content(page_id).unwrap()).into_owned();
        assert!(
            !after_content.contains("(secret)"),
            "the show operator survived: {after_content}"
        );
        assert!(content.contains("(secret)"), "fixture sanity");
    }

    #[test]
    fn redact_page_out_of_range_errors() {
        let original = author::author_pages(&["one"]).unwrap();
        let err = redact::redact(&original, &[(9, [0.0, 0.0, 1.0, 1.0])]).unwrap_err();
        assert!(matches!(err, PdfError::PageNotFound(9)));
    }

    #[test]
    fn mark_for_redaction_adds_an_annotated_mark_without_removing() {
        let original = author::author_pages(&["secret"]).unwrap();
        let marked =
            redact::mark_for_redaction(&original, &[(1, [10.0, 20.0, 30.0, 40.0])]).unwrap();
        let doc = Document::load_mem(&marked).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let annots = doc.get_page_annotations(page_id).unwrap();
        assert_eq!(annots.len(), 1);
        assert_eq!(
            annots[0].get(b"Subtype").unwrap().as_name().unwrap(),
            b"Redact"
        );
        // The mark is not a redaction: the text is still there.
        assert!(doc.extract_text(&[1]).unwrap().contains("secret"));
    }
}
