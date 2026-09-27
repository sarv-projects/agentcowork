//! AcroForm form-fill (D4, ARCH/04 §PDF mode 1): set `/V` on text fields.
//!
//! Fields are discovered from the catalog's `/AcroForm` `/Fields` array and
//! walked recursively (parent `Kids` → child fields), building the full
//! dotted name (`parent.child`). Leaf fields whose full name matches a
//! requested key get their `/V` value set. Boundary: text fields only —
//! appearance-stream (`/AP`) regeneration for rich widgets is a later pass.

use lopdf::{Document, Object, ObjectId};

use super::PdfError;

/// Fill AcroForm text fields. `fields` is `(full_field_name, value)`.
pub fn form_fill(bytes: &[u8], fields: &[(String, String)]) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(bytes)?;
    let field_ids = acro_field_ids(&doc)?;
    for id in field_ids {
        fill_field(&mut doc, id, &[], fields)?;
    }
    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

/// The top-level field object ids from the catalog's `/AcroForm` `/Fields`.
fn acro_field_ids(doc: &Document) -> Result<Vec<ObjectId>, PdfError> {
    let catalog = doc.catalog()?;
    let acro = match catalog.get(b"AcroForm") {
        Ok(o) => o.clone(),
        Err(_) => return Err(PdfError::NoAcroForm),
    };
    let (_id, acro) = doc.dereference(&acro)?;
    let fields = acro.as_dict()?.get(b"Fields")?;
    let arr = match fields {
        Object::Array(a) => a.clone(),
        Object::Reference(id) => doc.get_object(*id)?.as_array()?.clone(),
        _ => return Ok(Vec::new()),
    };
    Ok(arr.iter().filter_map(|o| o.as_reference().ok()).collect())
}

/// Recursively fill a field (and its kids), matching the full dotted name.
fn fill_field(
    doc: &mut Document,
    id: ObjectId,
    prefix: &[String],
    fields: &[(String, String)],
) -> Result<(), PdfError> {
    let (name_part, kids): (Option<String>, Vec<ObjectId>) = {
        let d = doc.get_dictionary(id)?;
        let t = d
            .get(b"T")
            .ok()
            .and_then(|o| o.as_str().ok())
            .map(|s| String::from_utf8_lossy(s).into_owned());
        let kids = match d.get(b"Kids") {
            Ok(Object::Array(a)) => a.iter().filter_map(|k| k.as_reference().ok()).collect(),
            _ => Vec::new(),
        };
        (t, kids)
    };

    let mut full = prefix.to_vec();
    if let Some(t) = name_part {
        full.push(t);
    }
    let name = full.join(".");

    if !kids.is_empty() {
        for kid in kids {
            fill_field(doc, kid, &full, fields)?;
        }
        return Ok(());
    }

    if let Some((_, value)) = fields.iter().find(|(n, _)| *n == name) {
        doc.get_dictionary_mut(id)?
            .set("V", Object::string_literal(value.clone()));
    }
    Ok(())
}
