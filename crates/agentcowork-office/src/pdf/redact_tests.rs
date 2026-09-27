//! Tests for [`super`] — PDF redaction by content removal (FIX-15).
//!
//! The load-bearing assertion in this file is the *extraction* oracle: after a
//! redaction, the removed text must be **absent from the extracted text** of
//! the saved document. A pass that only drew a black box over it would fail
//! every one of those assertions, which is the whole point of the fix.

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A two-page PDF where page 1 carries three text lines at known positions and
/// page 2 carries one, so a test can prove a rect on page 1 leaves page 2 and
/// the untouched lines of page 1 alone.
fn two_page_pdf() -> Vec<u8> {
    fn page(text: &str, y: f32) -> Object {
        // 12pt Courier: every glyph is 600/1000 em wide, so a run of n
        // characters is `n * 7.2` points wide from the origin.
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![72.into(), Object::Real(y)]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
            ],
        };
        // A trailing newline: real producers separate stream boundaries, and
        // the engine re-joins with a separator (see `page_content`).
        let mut bytes = content.encode().unwrap();
        bytes.push(b'\n');
        Object::Stream(Stream::new(dictionary! {}, bytes))
    }

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Courier",
        "FirstChar" => 32,
        "Widths" => vec![Object::Integer(600); 95],
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let c1 = doc.add_object(page("PUBLIC ok line", 720.0));
    let c2 = doc.add_object(page("SECRET-SSN-1234", 700.0));
    let c3 = doc.add_object(page("public footer", 680.0));
    let p1 = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Resources" => resources_id,
        "Contents" => vec![c1.into(), c2.into(), c3.into()],
    });
    let c4 = doc.add_object(page("page two body", 720.0));
    let p2 = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Resources" => resources_id,
        "Contents" => c4,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![p1.into(), p2.into()],
            "Count" => 2,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A one-page PDF whose only content is a full-bleed image drawn at
/// `cm`-placement (100,100) size (200,200), used for the image paths.
fn image_pdf(cover_fully: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    // A 1x1 greyscale image is enough: the test never decodes pixels.
    let img = doc.add_object(
        Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![0u8],
        )
        .with_compression(false),
    );
    let resources_id = doc.add_object(dictionary! {
        "XObject" => dictionary! { "Im0" => img },
    });
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    200.into(),
                    0.into(),
                    0.into(),
                    200.into(),
                    100.into(),
                    100.into(),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Resources" => resources_id,
        "Contents" => content_id,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    let _ = cover_fully;
    out
}

/// A one-page PDF whose text lives inside a form XObject placed at
/// `cm 1 0 0 1 72 700` — the recursion path.
fn form_xobject_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Courier",
        "FirstChar" => 32,
        "Widths" => vec![Object::Integer(600); 95],
    });
    let inner = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![0.into(), 0.into()]),
            Operation::new("Tj", vec![Object::string_literal("FORM-SECRET")]),
            Operation::new("ET", vec![]),
        ],
    };
    let form_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 200.into(), 20.into()],
        },
        inner.encode().unwrap(),
    ));
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Fx0" => form_id },
    });
    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    1.into(),
                    0.into(),
                    0.into(),
                    1.into(),
                    72.into(),
                    700.into(),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"Fx0".to_vec())]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Resources" => resources_id,
        "Contents" => content_id,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A PDF with a sticky-note annotation over the text, for the annotation
/// finding.
fn annotated_pdf() -> Vec<u8> {
    let base = two_page_pdf();
    let mut doc = Document::load_mem(&base).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let annot = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![70.into(), 690.into(), 300.into(), 710.into()],
        "Contents" => Object::string_literal("SSN 123-45-6789"),
    });
    let mut annos = vec![Object::Reference(annot)];
    if let Ok(Object::Array(existing)) = doc.get_dictionary(page).unwrap().get(b"Annots") {
        annos.extend(existing.iter().cloned());
    }
    doc.get_dictionary_mut(page).unwrap().set("Annots", annos);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

// ---------------------------------------------------------------------------
// The load-bearing test: removal, not annotation
// ---------------------------------------------------------------------------

#[test]
fn redacted_text_is_gone_from_the_extracted_text() {
    let original = two_page_pdf();
    let before = extract_page_text(&original, 1).unwrap();
    assert!(before.contains("SECRET-SSN-1234"), "fixture: {before:?}");

    // The secret line starts at x=72, y=700, 12pt Courier: 15 chars * 7.2pt
    // wide, 12pt tall. The rect covers exactly that run.
    let out = redact(&original, &[(1, [70.0, 696.0, 190.0, 704.0])]).unwrap();

    // THE assertion: the text is not extractable any more.
    let after = extract_page_text(&out, 1).unwrap();
    assert!(
        !after.contains("SECRET-SSN-1234"),
        "redaction left extractable content behind: {after:?}"
    );
    // And it is not in the raw file bytes either — nothing is merely covered.
    let raw = String::from_utf8_lossy(&out);
    assert!(
        !raw.contains("SECRET-SSN-1234"),
        "the redacted literal is still present in the file"
    );
    // The neighbouring lines and the other page are untouched.
    assert!(after.contains("PUBLIC ok line"));
    assert!(after.contains("public footer"));
    let p2 = extract_page_text(&out, 2).unwrap();
    assert!(p2.contains("page two body"));
}

#[test]
fn verify_absent_proves_removal_and_fails_loudly_when_it_does_not_hold() {
    let original = two_page_pdf();
    let request = RedactRequest::new(vec![(1, [70.0, 696.0, 190.0, 704.0])])
        .with_verify_absent(["SECRET-SSN-1234".to_string()]);
    let report = redact_checked(&original, &request, &RedactOptions::default()).unwrap();
    assert!(report.removed_anything());
    assert_eq!(report.removed_chars(), "SECRET-SSN-1234".len());
    assert!(report.unremovable.is_empty(), "{:?}", report.unremovable);
    // The post-op extraction evidence is in the receipt.
    let surviving = report.surviving_text.get(&1).expect("page 1 was touched");
    assert!(!surviving.contains("SECRET-SSN-1234"));

    // A target that is not covered by the rect is still present, so the
    // verification hook refuses the whole operation — no silent "redacted".
    let lying = RedactRequest::new(vec![(1, [70.0, 696.0, 190.0, 704.0])])
        .with_verify_absent(["PUBLIC ok line".to_string()]);
    let err = redact_checked(&original, &lying, &RedactOptions::default()).unwrap_err();
    match err {
        PdfError::RemovalUnproven(hits) => {
            assert_eq!(hits, vec![("PUBLIC ok line".to_string(), 1u32)]);
        }
        other => panic!("expected RemovalUnproven, got {other:?}"),
    }
}

#[test]
fn a_rect_that_covers_nothing_is_reported_not_silently_successful() {
    let original = two_page_pdf();
    let report = redact_checked(
        &original,
        &RedactRequest::new(vec![(1, [0.0, 0.0, 20.0, 20.0])]),
        &RedactOptions::default(),
    )
    .unwrap();
    assert!(!report.removed_anything());
    assert_eq!(report.no_intersection, vec![1]);
    assert!(
        report.surviving_text.is_empty(),
        "nothing changed, nothing to verify"
    );
}

#[test]
fn residual_report_declares_the_content_stream_reserialization() {
    // REQ-OFFICE-006: an unavoidable re-serialization is disclosed.
    let original = two_page_pdf();
    let report = redact_checked(
        &original,
        &RedactRequest::new(vec![(1, [70.0, 696.0, 190.0, 704.0])]),
        &RedactOptions::default(),
    )
    .unwrap();
    assert!(
        report
            .residuals
            .contains(&Residual::ReserializedContentStream)
    );
    assert!(report.residuals.contains(&Residual::FontGlyphsNotSubetted));
}

#[test]
fn a_second_pass_over_an_already_redacted_document_is_idempotent() {
    let original = two_page_pdf();
    let rects = vec![(1, [70.0, 696.0, 190.0, 704.0])];
    let once = redact(&original, &rects).unwrap();
    let twice = redact(&once, &rects).unwrap();
    let after = extract_page_text(&twice, 1).unwrap();
    assert!(!after.contains("SECRET-SSN-1234"));
    assert!(after.contains("public footer"));
}

// ---------------------------------------------------------------------------
// Fail-closed behaviour
// ---------------------------------------------------------------------------

#[test]
fn a_partially_covered_image_is_a_finding_and_refuses_by_default() {
    let original = image_pdf(false);
    // The image occupies (100,100)-(300,300); a rect covering only its left
    // half cannot be honoured without destroying the rest of the picture.
    let partial = vec![(1, [90.0, 90.0, 200.0, 310.0])];
    let err = redact(&original, &partial).unwrap_err();
    match err {
        PdfError::Unremovable(findings) => {
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].kind, UnremovableKind::PartialImage);
            assert!(findings[0].detail.contains("Im0"));
        }
        other => panic!("expected Unremovable, got {other:?}"),
    }

    // Under the opt-in policy the caller is told what survived.
    let report = redact_checked(
        &original,
        &RedactRequest::new(partial),
        &RedactOptions {
            unremovable: UnremovablePolicy::Report,
            ..RedactOptions::default()
        },
    )
    .unwrap();
    assert_eq!(report.unremovable.len(), 1);
    assert!(!report.removed_anything());
}

#[test]
fn a_wholly_covered_image_is_removed() {
    let original = image_pdf(true);
    let full = vec![(1, [90.0, 90.0, 310.0, 310.0])];
    let report = redact_checked(
        &original,
        &RedactRequest::new(full),
        &RedactOptions::default(),
    )
    .unwrap();
    assert_eq!(report.removals.len(), 1);
    assert_eq!(report.removals[0].kind, "image-xobject");
    assert_eq!(report.removals[0].name, "Im0");
    assert!(report.unremovable.is_empty());
    // The `Do` is gone from the page content.
    let doc = Document::load_mem(&report.bytes).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let content = String::from_utf8_lossy(&doc.get_page_content(page).unwrap()).into_owned();
    assert!(!content.contains("Do"), "image draw op survived: {content}");
}

#[test]
fn an_annotation_inside_the_area_is_a_finding_not_a_silent_overlap() {
    let original = annotated_pdf();
    let err = redact(&original, &[(1, [65.0, 685.0, 320.0, 712.0])]).unwrap_err();
    match err {
        PdfError::Unremovable(findings) => {
            assert!(
                findings
                    .iter()
                    .any(|f| f.kind == UnremovableKind::Annotation)
            );
        }
        other => panic!("expected Unremovable, got {other:?}"),
    }
}

#[test]
fn stale_redaction_marks_are_dropped_from_a_cleared_page() {
    let original = two_page_pdf();
    let marked = mark_for_redaction(&original, &[(1, [70.0, 696.0, 190.0, 704.0])]).unwrap();
    let doc = Document::load_mem(&marked).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    assert_eq!(doc.get_page_annotations(page).unwrap().len(), 1);

    let cleared = redact(&marked, &[(1, [70.0, 696.0, 190.0, 704.0])]).unwrap();
    let doc = Document::load_mem(&cleared).unwrap();
    let marks = doc
        .get_page_annotations(page)
        .unwrap()
        .iter()
        .filter(|a| {
            a.get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| n == b"Redact")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(marks, 0, "the burn-in mark must not survive a real removal");
}

#[test]
fn form_xobject_text_is_removed_through_recursion() {
    let original = form_xobject_pdf();
    // NOTE: `lopdf::Document::extract_text` walks only the page's own content
    // stream, so it cannot see text inside a form XObject. The oracle for this
    // path is the form stream itself — which is also why the post-op proof in
    // `redact_checked` is documented as a page-content-stream check.
    let form_literal = |bytes: &[u8]| -> usize {
        let doc = Document::load_mem(bytes).unwrap();
        let page = *doc.get_pages().get(&1).unwrap();
        let resources = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .clone();
        let resources = doc
            .dereference(&resources)
            .unwrap()
            .1
            .as_dict()
            .unwrap()
            .clone();
        let xobjects = resources.get(b"XObject").unwrap().clone();
        let xobjects = doc
            .dereference(&xobjects)
            .unwrap()
            .1
            .as_dict()
            .unwrap()
            .clone();
        let form = doc
            .dereference(xobjects.get(b"Fx0").unwrap())
            .unwrap()
            .1
            .as_stream()
            .unwrap()
            .get_plain_content()
            .unwrap();
        String::from_utf8_lossy(&form)
            .matches("(FORM-SECRET)")
            .count()
    };
    assert_eq!(
        form_literal(&original),
        1,
        "fixture: the form holds the text"
    );

    // The form is placed at (72,700) with a 200x20 BBox.
    let out = redact(&original, &[(1, [70.0, 698.0, 280.0, 706.0])]).unwrap();
    assert_eq!(
        form_literal(&out),
        0,
        "the form's show operator survived the redaction"
    );

    // The invocation itself survives so the rest of the form still draws.
    let doc = Document::load_mem(&out).unwrap();
    let page = *doc.get_pages().get(&1).unwrap();
    let content = String::from_utf8_lossy(&doc.get_page_content(page).unwrap()).into_owned();
    assert!(
        content.contains("Do"),
        "the form invocation was dropped: {content}"
    );
}

#[test]
fn form_depth_is_bounded_and_reported() {
    let original = form_xobject_pdf();
    let opts = RedactOptions {
        max_form_depth: 0,
        ..RedactOptions::default()
    };
    let err = redact_checked(
        &original,
        &RedactRequest::new(vec![(1, [70.0, 698.0, 280.0, 706.0])]),
        &opts,
    )
    .unwrap_err();
    match err {
        PdfError::Unremovable(findings) => {
            assert!(
                findings
                    .iter()
                    .any(|f| f.kind == UnremovableKind::FormDepth)
            );
        }
        other => panic!("expected Unremovable, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

#[test]
fn an_empty_request_is_refused() {
    let original = two_page_pdf();
    assert!(matches!(
        redact(&original, &[]),
        Err(PdfError::NothingToRedact)
    ));
}

#[test]
fn a_bad_page_number_is_typed() {
    let original = two_page_pdf();
    assert!(matches!(
        redact(&original, &[(9, [0.0, 0.0, 1.0, 1.0])]),
        Err(PdfError::PageNotFound(9))
    ));
    assert!(matches!(
        redact(&original, &[(0, [0.0, 0.0, 1.0, 1.0])]),
        Err(PdfError::PageNotFound(0))
    ));
}

#[test]
fn a_non_finite_rect_is_refused() {
    let original = two_page_pdf();
    assert!(matches!(
        redact(&original, &[(1, [f32::NAN, 0.0, 1.0, 1.0])]),
        Err(PdfError::InvalidRect(1, _))
    ));
}

#[test]
fn a_mark_only_pass_still_leaves_the_text_in_the_file() {
    // The regression this module exists to prevent: mark-for-redaction is not
    // redaction, which is why the `redact` op runs `redact`, not this.
    let original = two_page_pdf();
    let marked = mark_for_redaction(&original, &[(1, [70.0, 696.0, 190.0, 704.0])]).unwrap();
    let after = extract_page_text(&marked, 1).unwrap();
    assert!(
        after.contains("SECRET-SSN-1234"),
        "mark-for-redaction must not remove content"
    );
}

#[test]
fn an_unparseable_document_is_a_typed_lopdf_error() {
    let err = redact(b"not a pdf at all", &[(1, [0.0, 0.0, 1.0, 1.0])]).unwrap_err();
    assert!(matches!(err, PdfError::Lopdf(_)));
}
