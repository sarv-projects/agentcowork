//! P18-1 — PDF page-level ops (doc 70 §2 — `oxidize-pdf` steal pattern):
//! split / merge / rotate / reorder / delete / extract.
//!
//! lopdf 0.36 exposes no page-tree operations of its own (only
//! `add_page_contents`), so these are implemented directly on the `/Pages`
//! tree: every op ends with a **flat rebuild** — one `/Pages` node whose
//! `/Kids` is the ordered page list, with each page's `/Parent` repointed.
//! A flat tree is a valid PDF structure (viewers traverse it the same way),
//! and it makes split/merge/reorder/delete uniform: choose the ordered page
//! ids, rebuild.
//!
//! Split/merge copy the page **subgraphs** (page dict → `/Contents` streams,
//! `/Resources` fonts/images, ...) into the destination with fresh ids —
//! lopdf has no `merge`, so the copier below is the missing half.

use lopdf::{Dictionary, Document, Object, ObjectId, dictionary};
use std::collections::{HashMap, HashSet};

#[derive(Debug, thiserror::Error)]
pub enum PageOpError {
    #[error("lopdf error: {0}")]
    Lopdf(#[from] lopdf::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("page {0} is out of range (document has {1} pages)")]
    PageOutOfRange(u32, u32),
    #[error("page {0} listed more than once")]
    DuplicatePage(u32),
    #[error("selection leaves no pages")]
    EmptySelection,
    #[error("rotate delta must be a multiple of 90 (got {0})")]
    InvalidDelta(i64),
    #[error("reorder must be a permutation of 1..={0} (got {1} entries)")]
    NotAPermutation(u32, usize),
}

/// Page count (1-indexed numbers).
pub fn page_count(bytes: &[u8]) -> Result<u32, PageOpError> {
    let doc = Document::load_mem(bytes)?;
    Ok(doc.get_pages().len() as u32)
}

/// Split a contiguous 1-based range `start..=end` into a new PDF (pages are
/// renumbered 1..n in the output).
pub fn split(bytes: &[u8], range: std::ops::RangeInclusive<u32>) -> Result<Vec<u8>, PageOpError> {
    let selected: Vec<u32> = range.collect();
    extract_pages(bytes, &selected)
}

/// Extract an arbitrary set of 1-based page numbers (in the given order)
/// into a new PDF. The output is a fresh document: only the page subgraphs
/// are copied, nothing else rides along.
pub fn extract_pages(bytes: &[u8], pages: &[u32]) -> Result<Vec<u8>, PageOpError> {
    let src = Document::load_mem(bytes)?;
    let by_num = src.get_pages();
    let total = by_num.len() as u32;
    let mut seen = HashSet::new();
    let mut ids = Vec::with_capacity(pages.len());
    for &n in pages {
        validate_number(n, total)?;
        if !seen.insert(n) {
            return Err(PageOpError::DuplicatePage(n));
        }
        ids.push(by_num[&n]);
    }
    if ids.is_empty() {
        return Err(PageOpError::EmptySelection);
    }
    let mut dst = Document::with_version("1.5");
    let copied = copy_subgraph(&src, &ids, &mut dst);
    shell(&mut dst, &copied, &src, &ids)?;
    save(&mut dst)
}

/// Merge multiple PDFs in order into one document.
pub fn merge(docs: &[Vec<u8>]) -> Result<Vec<u8>, PageOpError> {
    if docs.is_empty() {
        return Err(PageOpError::EmptySelection);
    }
    let mut out = Document::load_mem(&docs[0])?;
    let mut all_pages = pages_in_order(&out);
    for other in &docs[1..] {
        let src = Document::load_mem(other)?;
        let src_pages = pages_in_order(&src);
        let mapped = copy_subgraph(&src, &src_pages, &mut out);
        // the incoming pages inherit /Resources + /MediaBox from *their*
        // source root; materialize them per-page so the merged pages render
        // with their own fonts, not the first document's.
        materialize_inherited(&mut out, &mapped);
        all_pages.extend(mapped);
    }
    rebuild_flat(&mut out, all_pages)?;
    save(&mut out)
}

/// Rotate pages by `delta` degrees (multiple of 90). `pages` selects which
/// 1-based page numbers rotate; `None` rotates every page. The rotation is
/// written **absolute** onto each page dict (after walking the tree for
/// inherited `/Rotate`), so a page under a rotated parent still lands where
/// the user asked.
pub fn rotate(bytes: &[u8], delta: i64, pages: Option<&[u32]>) -> Result<Vec<u8>, PageOpError> {
    if delta % 90 != 0 {
        return Err(PageOpError::InvalidDelta(delta));
    }
    let mut doc = Document::load_mem(bytes)?;
    let by_num = doc.get_pages();
    let total = by_num.len() as u32;
    let selected: Option<HashSet<u32>> = match pages {
        None => None,
        Some(list) => {
            let mut set = HashSet::new();
            for &n in list {
                validate_number(n, total)?;
                set.insert(n);
            }
            Some(set)
        }
    };
    let mut effective: HashMap<ObjectId, i64> = HashMap::new();
    if let Some(root) = pages_root(&doc) {
        walk_rotation(&doc, root, 0, &mut effective);
    }
    for (&n, &id) in &by_num {
        if let Some(set) = &selected {
            if !set.contains(&n) {
                continue;
            }
        }
        let eff = effective.get(&id).copied().unwrap_or(0);
        let target = (eff + delta).rem_euclid(360);
        let page = doc.get_dictionary_mut(id)?;
        page.set("Rotate", target);
    }
    save(&mut doc)
}

/// Reorder pages: `new_order` is the full 1-based permutation (e.g.
/// `&[3, 1, 2]` for a 3-page doc).
pub fn reorder(bytes: &[u8], new_order: &[u32]) -> Result<Vec<u8>, PageOpError> {
    let mut doc = Document::load_mem(bytes)?;
    let by_num = doc.get_pages();
    let total = by_num.len() as u32;
    if new_order.len() as u32 != total {
        return Err(PageOpError::NotAPermutation(total, new_order.len()));
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::with_capacity(new_order.len());
    for &n in new_order {
        validate_number(n, total)?;
        if !seen.insert(n) {
            return Err(PageOpError::DuplicatePage(n));
        }
        ids.push(by_num[&n]);
    }
    rebuild_flat(&mut doc, ids)?;
    save(&mut doc)
}

/// Delete the given 1-based page numbers; the rest keep their relative order
/// and are renumbered. Refuses to leave an empty document.
pub fn delete_pages(bytes: &[u8], pages: &[u32]) -> Result<Vec<u8>, PageOpError> {
    let mut doc = Document::load_mem(bytes)?;
    let by_num = doc.get_pages();
    let total = by_num.len() as u32;
    let mut doomed = HashSet::new();
    for &n in pages {
        validate_number(n, total)?;
        doomed.insert(n);
    }
    let keep: Vec<ObjectId> = by_num
        .iter()
        .filter(|(n, _)| !doomed.contains(n))
        .map(|(_, id)| *id)
        .collect();
    if keep.is_empty() {
        return Err(PageOpError::EmptySelection);
    }
    rebuild_flat(&mut doc, keep)?;
    save(&mut doc)
}

// ---------------------------------------------------------------------------
// internals

fn save(doc: &mut Document) -> Result<Vec<u8>, PageOpError> {
    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

fn pages_in_order(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

fn validate_number(n: u32, total: u32) -> Result<(), PageOpError> {
    if n == 0 || n > total {
        Err(PageOpError::PageOutOfRange(n, total))
    } else {
        Ok(())
    }
}

fn pages_root(doc: &Document) -> Option<ObjectId> {
    let catalog = doc.catalog().ok()?;
    let pages = catalog.get(b"Pages").ok()?;
    pages.as_reference().ok()
}

/// Build a fresh document shell for split/extract: catalog → the **copied
/// source root** `/Pages` node (so its inherited `/Resources`/`/MediaBox`
/// ride along) → the copied page refs, each `/Parent` repointed at it.
fn shell(
    doc: &mut Document,
    page_ids: &[ObjectId],
    src: &Document,
    src_ids: &[ObjectId],
) -> Result<(), PageOpError> {
    // Reuse the copied source root node: the first copied page's `/Parent`
    // points at it (the copier follows references, so the old node — with
    // its inherited entries — landed in `doc` under a fresh id).
    let root = copied_root(doc, page_ids).unwrap_or_else(|| {
        let id = doc.new_object_id();
        doc.objects
            .insert(id, Object::Dictionary(dictionary! { "Type" => "Pages" }));
        id
    });
    set_kids(doc, root, page_ids)?;
    reparent(doc, page_ids, root);
    let catalog_id = doc.new_object_id();
    doc.objects.insert(
        catalog_id,
        Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => root,
        }),
    );
    doc.trailer.set("Root", catalog_id);
    let _ = (src, src_ids);
    Ok(())
}

/// In-place flat rebuild: **reuse the document's existing root `/Pages`
/// node** (its inherited `/Resources`/`/MediaBox` stay intact — the in-place
/// ops must not orphan them), set its `/Kids` to the ordered page list, and
/// repoint every page's `/Parent`.
fn rebuild_flat(doc: &mut Document, page_ids: Vec<ObjectId>) -> Result<(), PageOpError> {
    let root = match pages_root(doc) {
        Some(id) => id,
        None => {
            let id = doc.new_object_id();
            doc.objects
                .insert(id, Object::Dictionary(dictionary! { "Type" => "Pages" }));
            let catalog = doc.catalog_mut()?;
            catalog.set("Pages", id);
            id
        }
    };
    set_kids(doc, root, &page_ids)?;
    reparent(doc, &page_ids, root);
    Ok(())
}

fn set_kids(doc: &mut Document, node: ObjectId, page_ids: &[ObjectId]) -> Result<(), PageOpError> {
    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let dict = doc.get_dictionary_mut(node)?;
    dict.set("Type", "Pages");
    dict.set("Kids", kids);
    dict.set("Count", page_ids.len() as i64);
    Ok(())
}

fn reparent(doc: &mut Document, page_ids: &[ObjectId], node: ObjectId) {
    for id in page_ids {
        if let Ok(d) = doc.get_dictionary_mut(*id) {
            d.set("Parent", node);
        }
    }
}

/// The `/Pages` node a set of (copied) pages currently point at, if any.
fn copied_root(doc: &Document, page_ids: &[ObjectId]) -> Option<ObjectId> {
    let first = page_ids.first()?;
    let page = doc.get_dictionary(*first).ok()?;
    let parent = page.get(b"Parent").ok()?;
    parent.as_reference().ok()
}

/// Copy `/Resources` + `/MediaBox` from each page's current parent node onto
/// the page itself when the page lacks them — used for merged pages whose
/// inherited entries live on a foreign (now orphaned) root.
fn materialize_inherited(doc: &mut Document, page_ids: &[ObjectId]) {
    let Some(root) = copied_root(doc, page_ids) else {
        return;
    };
    let (resources, media_box) = match doc.get_dictionary(root) {
        Ok(rd) => (
            rd.get(b"Resources").ok().cloned(),
            rd.get(b"MediaBox").ok().cloned(),
        ),
        Err(_) => (None, None),
    };
    for id in page_ids {
        if let Ok(page) = doc.get_dictionary_mut(*id) {
            if page.get(b"Resources").is_err() {
                if let Some(r) = &resources {
                    page.set("Resources", r.clone());
                }
            }
            if page.get(b"MediaBox").is_err() {
                if let Some(m) = &media_box {
                    page.set("MediaBox", m.clone());
                }
            }
        }
    }
}

/// Copy the object subgraphs rooted at `roots` from `src` into `dst` with
/// fresh ids, remapping every reference transitively. Returns the mapped ids
/// in the same order as `roots`. This is lopdf's missing `merge` half — a
/// page's `/Contents` streams, `/Resources` fonts, and anything else it
/// references ride along; nothing else is copied.
fn copy_subgraph(src: &Document, roots: &[ObjectId], dst: &mut Document) -> Vec<ObjectId> {
    let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
    roots
        .iter()
        .map(|id| copy_one(src, *id, dst, &mut map))
        .collect()
}

fn copy_one(
    src: &Document,
    id: ObjectId,
    dst: &mut Document,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> ObjectId {
    if let Some(&mapped) = map.get(&id) {
        return mapped;
    }
    let mapped = dst.new_object_id();
    map.insert(id, mapped);
    if let Some(obj) = src.objects.get(&id) {
        let value = remap_object(src, obj, dst, map);
        dst.objects.insert(mapped, value);
    }
    mapped
}

fn remap_object(
    src: &Document,
    obj: &Object,
    dst: &mut Document,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> Object {
    match obj {
        Object::Reference(id) => Object::Reference(copy_one(src, *id, dst, map)),
        Object::Array(items) => Object::Array(
            items
                .iter()
                .map(|o| remap_object(src, o, dst, map))
                .collect(),
        ),
        Object::Dictionary(d) => {
            let mut nd = Dictionary::new();
            for (k, v) in d.iter() {
                nd.set(k.clone(), remap_object(src, v, dst, map));
            }
            Object::Dictionary(nd)
        }
        Object::Stream(s) => {
            let mut ns = s.clone();
            let mut nd = Dictionary::new();
            for (k, v) in s.dict.iter() {
                nd.set(k.clone(), remap_object(src, v, dst, map));
            }
            ns.dict = nd;
            Object::Stream(ns)
        }
        other => other.clone(),
    }
}

/// Walk the `/Pages` tree recording each page's **effective** rotation
/// (inherited `/Rotate` from ancestor nodes summed with the page's own).
fn walk_rotation(doc: &Document, node: ObjectId, inherited: i64, out: &mut HashMap<ObjectId, i64>) {
    let Ok(dict) = doc.get_dictionary(node) else {
        return;
    };
    let own = dict.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0);
    let effective = inherited + own;
    let is_page = dict.get_type().map(|t| t == b"Page").unwrap_or(false);
    if is_page {
        out.insert(node, effective);
        return;
    }
    if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
        for kid in kids {
            if let Object::Reference(id) = kid {
                walk_rotation(doc, *id, effective, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::{author::author_pages, inspect};

    fn two_page() -> Vec<u8> {
        author_pages(&["alpha", "beta"]).unwrap()
    }

    fn texts(bytes: &[u8]) -> Vec<String> {
        inspect(bytes)
            .unwrap()
            .texts
            .into_iter()
            .map(|t| t.trim().to_string())
            .collect()
    }

    #[test]
    fn page_count_roundtrip() {
        assert_eq!(page_count(&two_page()).unwrap(), 2);
    }

    #[test]
    fn split_keeps_only_the_range() {
        let one = split(&two_page(), 2..=2).unwrap();
        let info = inspect(&one).unwrap();
        assert_eq!(info.pages, 1);
        assert_eq!(texts(&one), vec!["beta"]);
    }

    #[test]
    fn split_rejects_out_of_range() {
        assert!(matches!(
            split(&two_page(), 3..=5),
            Err(PageOpError::PageOutOfRange(3, 2))
        ));
    }

    #[test]
    fn extract_arbitrary_set_in_order() {
        let three = author_pages(&["x", "y", "z"]).unwrap();
        let out = extract_pages(&three, &[3, 1]).unwrap();
        let t = texts(&out);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], "z");
        assert_eq!(t[1], "x");
    }

    #[test]
    fn merge_appends_in_order() {
        let a = author_pages(&["one"]).unwrap();
        let b = author_pages(&["two", "three"]).unwrap();
        let merged = merge(&[a, b]).unwrap();
        let t = texts(&merged);
        assert_eq!(t, vec!["one", "two", "three"]);
    }

    #[test]
    fn rotate_writes_absolute_rotation() {
        let out = rotate(&two_page(), 90, Some(&[1])).unwrap();
        let doc = Document::load_mem(&out).unwrap();
        let by_num = doc.get_pages();
        let page1 = doc.get_dictionary(by_num[&1]).unwrap();
        assert_eq!(
            page1.get(b"Rotate").ok().and_then(|o| o.as_i64().ok()),
            Some(90)
        );
        // page 2 untouched
        let page2 = doc.get_dictionary(by_num[&2]).unwrap();
        assert_eq!(page2.get(b"Rotate").ok(), None);
    }

    #[test]
    fn rotate_rejects_non_multiple_of_90() {
        assert!(matches!(
            rotate(&two_page(), 45, None),
            Err(PageOpError::InvalidDelta(45))
        ));
    }

    #[test]
    fn reorder_swaps_pages() {
        let out = reorder(&two_page(), &[2, 1]).unwrap();
        let t = texts(&out);
        assert_eq!(t, vec!["beta", "alpha"]);
    }

    #[test]
    fn reorder_requires_a_permutation() {
        assert!(matches!(
            reorder(&two_page(), &[1]),
            Err(PageOpError::NotAPermutation(2, 1))
        ));
        assert!(matches!(
            reorder(&two_page(), &[1, 1]),
            Err(PageOpError::DuplicatePage(1))
        ));
    }

    #[test]
    fn delete_pages_renumbers_the_rest() {
        let three = author_pages(&["x", "y", "z"]).unwrap();
        let out = delete_pages(&three, &[2]).unwrap();
        assert_eq!(texts(&out), vec!["x", "z"]);
        assert_eq!(page_count(&out).unwrap(), 2);
    }

    #[test]
    fn delete_refuses_empty_result() {
        assert!(matches!(
            delete_pages(&two_page(), &[1, 2]),
            Err(PageOpError::EmptySelection)
        ));
    }

    #[test]
    fn merged_pages_keep_resources() {
        // author_pages shares one font across pages; after merge the copied
        // page must still render (font object copied with it).
        let a = author_pages(&["a"]).unwrap();
        let b = author_pages(&["b"]).unwrap();
        let merged = merge(&[a, b]).unwrap();
        assert_eq!(texts(&merged), vec!["a", "b"]);
    }
}
