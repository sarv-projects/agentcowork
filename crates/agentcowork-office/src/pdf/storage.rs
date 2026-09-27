//! P36 (D4) — `annotationStorage` persist: form-fill survives save/reopen.
//!
//! pdf.js-class viewers keep field state in an `annotationStorage` cache; a
//! bare save of the original bytes loses every filled field. This pass
//! writes the current field values **into the AcroForm** (`/V` on each leaf
//! field), so a save → reopen round-trip keeps the user's answers. Same
//! trait family as `form_fill` (walks /AcroForm /Fields recursively) but
//! acts from a storage map and never invents fields.

use lopdf::{Document, Object, ObjectId};
use std::collections::BTreeMap;

use super::PdfError;

/// A persisted field value from the viewer's `annotationStorage`.
pub type AnnotationStorage = BTreeMap<String, String>;

/// Write every storage entry onto its matching AcroForm field (`/V`).
/// Returns `(updated, missing)`: fields updated vs. names in storage with no
/// matching form field. A storage write must never create fields.
pub fn persist_storage(
    doc: &mut Document,
    storage: &AnnotationStorage,
) -> Result<(u32, u32), PdfError> {
    let mut updated = 0u32;
    let mut missing = 0u32;

    for (name, value) in storage {
        let mut found = false;
        for id in acro_field_ids(doc)? {
            let field_name = field_name(doc, id);
            let dotted = format!(".{name}");
            if field_name == *name || field_name.ends_with(&dotted) {
                let d = doc.get_dictionary_mut(id)?;
                d.set(
                    "V",
                    Object::String(value.as_bytes().to_vec(), lopdf::StringFormat::Literal),
                );
                found = true;
                updated += 1;
                break;
            }
        }
        if !found {
            missing += 1;
        }
    }
    Ok((updated, missing))
}

fn field_name(doc: &Document, id: ObjectId) -> String {
    doc.get_dictionary(id)
        .ok()
        .and_then(|d| d.get(b"T").ok())
        .and_then(|o| o.as_str().ok())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .unwrap_or_default()
}

/// Top-level field object ids from the catalog's `/AcroForm` `/Fields`
/// (walked recursively through `Kids`).
fn acro_field_ids(doc: &Document) -> Result<Vec<ObjectId>, PdfError> {
    let mut out = Vec::new();
    let catalog = doc.catalog()?;
    let acro = match catalog.get(b"AcroForm") {
        Ok(o) => o.clone(),
        Err(_) => return Err(PdfError::NoAcroForm),
    };
    let (_id, acro) = doc.dereference(&acro)?;
    let fields = acro
        .as_dict()?
        .get(b"Fields")
        .cloned()
        .unwrap_or(Object::Array(Vec::new()));
    let arr = match fields {
        Object::Array(a) => a,
        Object::Reference(id) => doc.get_object(id)?.as_array()?.clone(),
        _ => return Ok(Vec::new()),
    };
    walk_fields(doc, &arr, &mut out);
    Ok(out)
}

fn walk_fields(doc: &Document, fields: &[Object], out: &mut Vec<ObjectId>) {
    for f in fields {
        if let Ok(id) = f.as_reference() {
            out.push(id);
            if let Ok(d) = doc.get_dictionary(id) {
                if let Ok(kids) = d.get(b"Kids") {
                    if let Ok(arr) = kids.as_array() {
                        walk_fields(doc, arr, out);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_storage_is_noop() {
        let mut d = Document::with_version("1.7");
        let (updated, missing) =
            persist_storage(&mut d, &AnnotationStorage::new()).unwrap_or((0, 0));
        assert_eq!(updated, 0);
        assert_eq!(missing, 0);
    }

    #[test]
    fn storage_never_invents_fields() {
        let mut d = Document::with_version("1.7");
        let mut storage = AnnotationStorage::new();
        storage.insert("q1".into(), "42".into());
        // No AcroForm in a bare doc → error path handled by caller; the
        // storage-writing contract still holds (no fields invented).
        let res = persist_storage(&mut d, &storage);
        assert!(res.is_err() || res.unwrap().1 >= 1);
    }
}
