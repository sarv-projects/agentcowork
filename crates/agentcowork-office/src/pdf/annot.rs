//! PDF annotations (D8-gap — doc 63 §3): sticky notes (`/Text`) and
//! highlights (`/Highlight`). Form-fill already ships `/AP` regeneration as
//! later work; this adds the note + highlight annotation pass.

use lopdf::{Document, Object, ObjectId, dictionary};

use super::PdfError;

enum AnnotsLoc {
    Inline(ObjectId),
    Array(ObjectId),
    Missing(ObjectId),
}

fn append_annotation(doc: &mut Document, page: u32, annot: Object) -> Result<(), PdfError> {
    let pages: Vec<ObjectId> = doc.page_iter().collect();
    let page_id = pages
        .get((page as usize).saturating_sub(1))
        .copied()
        .ok_or(PdfError::PageNotFound(page))?;
    let annot_id = doc.add_object(annot);

    let loc = {
        let page_dict = doc.get_dictionary(page_id)?;
        match page_dict.get(b"Annots") {
            Ok(Object::Array(_)) => AnnotsLoc::Inline(page_id),
            Ok(Object::Reference(id)) => AnnotsLoc::Array(*id),
            _ => AnnotsLoc::Missing(page_id),
        }
    };
    match loc {
        AnnotsLoc::Inline(page_id) => {
            let d = doc.get_dictionary_mut(page_id)?;
            if let Ok(Object::Array(arr)) = d.get_mut(b"Annots") {
                arr.push(Object::Reference(annot_id));
            }
        }
        AnnotsLoc::Array(arr_id) => {
            let obj = doc.get_object_mut(arr_id)?;
            if let Object::Array(arr) = obj {
                arr.push(Object::Reference(annot_id));
            }
        }
        AnnotsLoc::Missing(page_id) => {
            doc.get_dictionary_mut(page_id)?
                .set("Annots", vec![Object::Reference(annot_id)]);
        }
    }
    Ok(())
}

fn rect_obj([x1, y1, x2, y2]: [f32; 4]) -> Vec<Object> {
    vec![
        Object::Real(x1),
        Object::Real(y1),
        Object::Real(x2),
        Object::Real(y2),
    ]
}

/// Add a sticky-note (free-text) annotation at `rect` with `text`.
pub fn add_text_annotation(
    bytes: &[u8],
    page: u32,
    rect: [f32; 4],
    text: &str,
) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(bytes)?;
    let annot = Object::Dictionary(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => rect_obj(rect),
        "Contents" => Object::string_literal(text),
        "Open" => true,
    });
    append_annotation(&mut doc, page, annot)?;
    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

/// Add a highlight annotation over `rect`.
pub fn add_highlight_annotation(
    bytes: &[u8],
    page: u32,
    rect: [f32; 4],
) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(bytes)?;
    let [x1, y1, x2, y2] = rect;
    // One quad: (top-left, top-right, bottom-left, bottom-right) — 8 numbers.
    let quad_points = vec![
        Object::Real(x1),
        Object::Real(y2),
        Object::Real(x2),
        Object::Real(y2),
        Object::Real(x1),
        Object::Real(y1),
        Object::Real(x2),
        Object::Real(y1),
    ];
    let annot = Object::Dictionary(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Highlight",
        "Rect" => rect_obj(rect),
        "QuadPoints" => quad_points,
    });
    append_annotation(&mut doc, page, annot)?;
    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Document, Object, dictionary};

    fn one_page_pdf() -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = lopdf::Stream::new(
            dictionary! {},
            b"BT /F1 12 Tf 72 720 Td (Hello) Tj ET".to_vec(),
        );
        let content_id = doc.add_object(content);
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => resources_id,
            "Contents" => content_id,
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn adds_sticky_note_annotation() {
        let pdf = one_page_pdf();
        let out = add_text_annotation(&pdf, 1, [10.0, 20.0, 110.0, 40.0], "review this").unwrap();
        let doc = Document::load_mem(&out).unwrap();
        // The page now has an /Annots entry.
        let pages: Vec<ObjectId> = doc.page_iter().collect();
        let page_dict = doc.get_dictionary(pages[0]).unwrap();
        assert!(page_dict.get(b"Annots").is_ok());
    }

    #[test]
    fn adds_highlight_annotation() {
        let pdf = one_page_pdf();
        let out = add_highlight_annotation(&pdf, 1, [10.0, 20.0, 110.0, 40.0]).unwrap();
        let doc = Document::load_mem(&out).unwrap();
        let pages: Vec<ObjectId> = doc.page_iter().collect();
        let page_dict = doc.get_dictionary(pages[0]).unwrap();
        assert!(page_dict.get(b"Annots").is_ok());
    }

    #[test]
    fn missing_page_errors() {
        let pdf = one_page_pdf();
        assert!(matches!(
            add_text_annotation(&pdf, 99, [0.0, 0.0, 1.0, 1.0], "x"),
            Err(PdfError::PageNotFound(99))
        ));
    }
}
