//! DOCX engine (P4.1 — Word Block-Patch, D1).
//!
//! Pipeline (ARCH/04 §4.1): open ZIP → parts index + rels → block tree
//! (anchored) → render plain text for the LLM → patch a block's text →
//! **verify field balance** → byte-preserving ZIP rewrite.
//!
//! Two guards sit between a patch and the commit (ARCH/04 §4.6):
//! - [`field_balance`] refuses an unbalanced `w:fldChar` triple *before* the
//!   patched bytes are accepted, so a Word-repair document can never be
//!   produced and the archive stays byte-identical;
//! - [`crate::limits`] refuses a package too large to load, by name, instead
//!   of attempting a load it cannot bound.
//!
//! Orphaned media left by a removed paragraph is swept by [`DocxEngine::sweep_media`].

pub mod blocktree;
pub mod citation;
pub mod field_balance;
pub mod parts;
pub mod patch;
pub mod track;

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::limits::{LoadBudget, PatchLimits};
use crate::zip::OoxmlArchive;
use blocktree::{Block, BlockTree, build_blocks};

/// Errors surfaced by the office engine. The patch failures are the
/// documented "safety fallback" cases (GenOffice returns `null` and the
/// caller rebuilds; we return an error the caller can act on).
#[derive(Debug, thiserror::Error)]
pub enum OfficeError {
    #[error("archive error: {0}")]
    Archive(#[from] crate::zip::ArchiveError),
    #[error("xml error: {0}")]
    Xml(#[from] crate::xml::OfficeXmlError),
    #[error("utf-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("block not found: {0}")]
    BlockNotFound(String),
    #[error(
        "stale edit on block {address}: rendered text no longer matches the part (re-render before editing)"
    )]
    StaleEdit { address: String },
    #[error(
        "edit on block {0} crosses a line break / tab (structural change) — rebuild the paragraph instead"
    )]
    PatchAcrossMarker(String),
    #[error("paragraph has no w:t text anchor")]
    NoTextAnchor,
    #[error("invalid patch range on block {0}")]
    InvalidPatchRange(String),
    #[error(
        "unbalanced field characters: field #{field} in {part}: {detail} (patch refused; the file was not modified)"
    )]
    FieldBalance {
        part: String,
        field: usize,
        detail: String,
    },
    #[error(
        "{subject} is {actual} bytes, above the {kind} ceiling of {limit} bytes (refused: the engine will not attempt a load it cannot bound)"
    )]
    TooLarge {
        kind: crate::limits::LimitKind,
        subject: String,
        actual: u64,
        limit: u64,
    },
    #[error(
        "package has {count} entries, above the entry-count ceiling of {limit} (refused: the engine will not attempt a load it cannot bound)"
    )]
    TooManyParts { count: usize, limit: usize },
    #[error("internal error")]
    Internal,
}

/// Content types + rels part names used by the engine.
const CONTENT_TYPES: &str = "[Content_Types].xml";
const DOCUMENT_RELS: &str = "word/_rels/document.xml.rels";
const BODY_PART: &str = "word/document.xml";

/// A `.docx` opened for surgical editing.
pub struct DocxEngine {
    archive: OoxmlArchive,
    parts: parts::PartsIndex,
    tree: BlockTree,
    /// Current bytes of every part (patches mutate this; `save` rewrites the
    /// archive with only the changed parts).
    current: HashMap<String, Vec<u8>>,
    /// Parts a media sweep removed (omitted from the rebuilt archive, in the
    /// same atomic commit that rewrites the relationships part).
    deleted: HashSet<String>,
}

impl DocxEngine {
    /// Open a `.docx` from bytes: parse the package, build the block tree.
    ///
    /// Uses the process size policy (`PatchLimits::default`, i.e. the
    /// documented constants plus any `AGENTCOWORK_OFFICE_*` override).
    pub fn open(bytes: Vec<u8>) -> Result<Self, OfficeError> {
        Self::open_with_limits(bytes, PatchLimits::default())
    }

    /// Open a `.docx` under an explicit size policy. Every part the engine
    /// loads is charged against the policy's per-part and running-total
    /// ceilings; a refusal names which ceiling and the actual size, and no
    /// parsing happens for the part that was refused.
    pub fn open_with_limits(bytes: Vec<u8>, limits: PatchLimits) -> Result<Self, OfficeError> {
        limits.check_archive(bytes.len() as u64)?;
        let mut archive = OoxmlArchive::open(bytes)?;
        limits.check_part_count(archive.entry_count()?)?;
        let mut budget = LoadBudget::new();

        let content_types = read_bounded(&mut archive, &limits, &mut budget, CONTENT_TYPES)?;
        let document_rels =
            read_optional_bounded(&mut archive, &limits, &mut budget, DOCUMENT_RELS)?;
        let parts = parts::PartsIndex::parse(&content_types, document_rels.as_deref())?;

        let body = read_bounded(&mut archive, &limits, &mut budget, BODY_PART)?;

        // Load header/footer parts referenced by the body rels.
        let mut headers: Vec<(String, Vec<u8>)> = Vec::new();
        for rel in parts.header_footer_rels() {
            let target = parts.resolve_target(rel);
            if let Ok(Some(bytes)) =
                read_optional_bounded(&mut archive, &limits, &mut budget, &target)
            {
                headers.push((target, bytes));
            }
        }

        let tree = build_blocks(&body, BODY_PART, &headers)?;

        let mut current = HashMap::new();
        current.insert(BODY_PART.to_string(), body);
        for (name, bytes) in headers {
            current.insert(name, bytes);
        }

        Ok(Self {
            archive,
            parts,
            tree,
            current,
            deleted: HashSet::new(),
        })
    }

    /// The plain-text render of the whole document (the LLM's edit surface).
    pub fn render_text(&self) -> &str {
        &self.tree.render
    }

    /// Rendered text of one block (what the LLM sees for that address).
    pub fn render_block(&self, address: &str) -> Result<String, OfficeError> {
        let block = self
            .tree
            .find(address)
            .ok_or_else(|| OfficeError::BlockNotFound(address.to_string()))?;
        let xml = self.current.get(&block.part).ok_or(OfficeError::Internal)?;
        let doc = crate::xml::parse(xml)?;
        let para = doc
            .descendants()
            .find(|n| n.range().start == block.range.start && n.range().end == block.range.end)
            .ok_or_else(|| OfficeError::BlockNotFound(address.to_string()))?;
        Ok(blocktree::render_paragraph(para))
    }

    /// Apply an edit to a block's text. The block's *current* rendered text
    /// is used as the expected original (so stale edits are rejected).
    ///
    /// The patched bytes are verified for `w:fldChar` balance **before** they
    /// are accepted, so an unbalanced field never reaches the engine's state
    /// and a later `save` cannot commit it.
    pub fn patch_block(&mut self, address: &str, new_text: &str) -> Result<(), OfficeError> {
        let block = self
            .tree
            .find(address)
            .cloned()
            .ok_or_else(|| OfficeError::BlockNotFound(address.to_string()))?;
        if block.kind != blocktree::BlockKind::Paragraph {
            return Err(OfficeError::Internal); // only paragraphs are patchable in P4.1
        }
        let xml = self
            .current
            .get(&block.part)
            .ok_or(OfficeError::Internal)?
            .clone();
        let expected = self.render_block(address)?;
        let patched = patch::apply_block_patch(&xml, &block, &expected, new_text)?;
        // Refuse an unbalanced field triple: the engine's state (and therefore
        // the archive) stays exactly as it was.
        field_balance::verify(&block.part, &patched)?;
        self.current.insert(block.part, patched);
        // A patch may change the part's length, which shifts the byte range of
        // every later block. Rebuild from the bytes we just wrote so the next
        // `render_block` / `patch_block` addresses the current XML, not the
        // layout the engine saw at `open`.
        self.refresh_tree()?;
        Ok(())
    }

    /// Re-parse the block tree (addresses, kinds, ranges, render) from the
    /// current part bytes.
    fn refresh_tree(&mut self) -> Result<(), OfficeError> {
        let body = self
            .current
            .get(BODY_PART)
            .ok_or(OfficeError::Internal)?
            .clone();
        let mut headers: Vec<(String, Vec<u8>)> = Vec::new();
        for rel in self.parts.header_footer_rels() {
            let target = self.parts.resolve_target(rel);
            if let Some(bytes) = self.current.get(&target) {
                headers.push((target, bytes.clone()));
            }
        }
        self.tree = build_blocks(&body, BODY_PART, &headers)?;
        Ok(())
    }

    /// The block tree (addresses, kinds, parts, ranges).
    pub fn blocks(&self) -> &[Block] {
        &self.tree.blocks
    }

    /// Content type of a part (override → extension default).
    pub fn part_content_type(&self, part: &str) -> Option<&str> {
        self.parts.content_type(part)
    }

    /// Sweep media that a patch orphaned.
    ///
    /// Collects the `r:`-namespace relationship ids the (already patched)
    /// body still names, finds the media payloads and `Relationship` entries
    /// in `word/_rels/document.xml.rels` that no longer resolve to a
    /// reference, and removes both **as one pending change set**: the
    /// relationships part is rewritten by splicing out exactly the removed
    /// `Relationship` elements, and the payloads are queued for omission in
    /// the same `save` that carries the rewrite. A payload whose removal
    /// would change a part the engine cannot safely rewrite (shared media, an
    /// unreadable other `_rels` part, an explicit content-type `Override`) is
    /// reported as a cleanup candidate and nothing is removed for it.
    ///
    /// The sweep never runs implicitly: a caller asks for it, so byte-stability
    /// assertions for patch-only workflows stay exactly as they were.
    pub fn sweep_media(&mut self) -> Result<crate::media_gc::MediaSweep, OfficeError> {
        let body = self
            .current
            .get(BODY_PART)
            .ok_or(OfficeError::Internal)?
            .clone();
        let Self {
            archive,
            current,
            deleted,
            ..
        } = self;
        let part_names: BTreeSet<String> = archive.parts()?.into_iter().collect();
        let plan = {
            let mut read = |name: &str| {
                current
                    .get(name)
                    .cloned()
                    .or_else(|| archive.read_part(name).ok())
            };
            crate::media_gc::sweep_part(BODY_PART, &body, &part_names, &mut read)?
        };
        if let Some(rels) = plan.rels_bytes {
            current.insert(plan.sweep.rels_part.clone(), rels);
        }
        for part in &plan.remove_parts {
            current.remove(part);
            deleted.insert(part.clone());
        }
        Ok(plan.sweep)
    }

    /// Rebuild the `.docx`: only parts changed by patches are re-deflated;
    /// every other entry is copied verbatim.
    ///
    /// Every pending part is re-verified for `w:fldChar` balance first: an
    /// imbalance refuses the commit by name (part + field index) and returns
    /// no bytes at all, so the file on disk is never half-written or corrupt.
    pub fn save(&mut self) -> Result<Vec<u8>, OfficeError> {
        // Verify before anything is produced: a refusal leaves the caller's
        // file (and this archive) byte-identical.
        self.verify_pending_parts()?;
        let modified: Vec<(String, Vec<u8>)> = self
            .current
            .iter()
            .filter(|(_, bytes)| !bytes.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut deleted: Vec<String> = self.deleted.iter().cloned().collect();
        deleted.sort();
        Ok(self.archive.save_changes(&modified, &[], &deleted)?)
    }

    /// Field-balance report for every part this engine has loaded (body +
    /// headers/footers). Empty for a document with no complex fields.
    pub fn field_report(&self) -> Result<Vec<(String, field_balance::FieldReport)>, OfficeError> {
        let mut out = Vec::new();
        let mut names: Vec<&String> = self.current.keys().collect();
        names.sort();
        for name in names {
            let bytes = &self.current[name];
            if bytes.is_empty() {
                continue;
            }
            out.push((name.clone(), field_balance::verify(name, bytes)?));
        }
        Ok(out)
    }

    /// Refuse the commit when any pending part has an unbalanced field.
    /// [`Self::field_report`] returns the first failure by value, so naming the
    /// part and the field index is all the caller needs.
    fn verify_pending_parts(&self) -> Result<(), OfficeError> {
        self.field_report().map(|_| ())
    }
}

/// Read one part, refusing a size the policy cannot bound before the
/// decompression allocates anything.
fn read_bounded(
    archive: &mut OoxmlArchive,
    limits: &PatchLimits,
    budget: &mut LoadBudget,
    name: &str,
) -> Result<Vec<u8>, OfficeError> {
    let size = archive.entry_size(name)?;
    budget.charge(limits, name, size)?;
    Ok(archive.read_part(name)?)
}

/// [`read_bounded`], but a missing part is not an error.
fn read_optional_bounded(
    archive: &mut OoxmlArchive,
    limits: &PatchLimits,
    budget: &mut LoadBudget,
    name: &str,
) -> Result<Option<Vec<u8>>, OfficeError> {
    match archive.entry_size(name) {
        Ok(size) => {
            budget.charge(limits, name, size)?;
            Ok(Some(archive.read_part(name)?))
        }
        Err(crate::zip::ArchiveError::PartNotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Patch the first paragraph of a bare document part (test + helper path):
/// parses `xml`, patches block `p1`, returns the new part bytes.
pub fn patch_first_paragraph(xml: &[u8], new_text: &str) -> Result<Vec<u8>, OfficeError> {
    let tree = blocktree::blocks_of_part(xml, BODY_PART)?;
    let block = tree
        .find("p1")
        .cloned()
        .ok_or_else(|| OfficeError::BlockNotFound("p1".into()))?;
    let doc = crate::xml::parse(xml)?;
    let para = doc
        .descendants()
        .find(|n| n.range().start == block.range.start && n.range().end == block.range.end)
        .ok_or_else(|| OfficeError::BlockNotFound("p1".into()))?;
    let expected = blocktree::render_paragraph(para);
    patch::apply_block_patch(xml, &block, &expected, new_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> DocxEngine {
        DocxEngine::open(crate::zip::tests::sample_docx()).unwrap()
    }

    #[test]
    fn opens_and_renders() {
        let e = engine();
        assert_eq!(
            e.render_text(),
            "Hello, world!\nLine one\nline two\ncell A1 | cell B1\n"
        );
    }

    #[test]
    fn render_block_returns_paragraph_text() {
        let e = engine();
        assert_eq!(e.render_block("p1").unwrap(), "Hello, world!");
        assert_eq!(e.render_block("p2").unwrap(), "Line one\nline two");
        assert_eq!(e.render_block("t1:r1c1:p1").unwrap(), "cell A1");
    }

    #[test]
    fn patch_replaces_text_and_preserves_untouched_bytes() {
        let mut e = engine();
        e.patch_block("p1", "Goodbye, world!").unwrap();
        let out = e.save().unwrap();

        // Reopen: the render shows the new text; the old run is gone.
        let mut reopened = DocxEngine::open(out).unwrap();
        assert_eq!(
            reopened.render_text(),
            "Goodbye, world!\nLine one\nline two\ncell A1 | cell B1\n"
        );
        let xml =
            String::from_utf8(reopened.archive.read_part("word/document.xml").unwrap()).unwrap();
        assert!(!xml.contains("Hello, "));
        // The untouched second paragraph keeps its bytes.
        assert!(xml.contains("Line one"));
    }

    #[test]
    fn patch_minimal_touches_only_the_changed_run() {
        // Two runs: "Hello, " + "world!". Edit to "Hello, universe!" must
        // touch ONLY the second run's text bytes ("world" → "universe").
        let mut e = engine();
        e.patch_block("p1", "Hello, universe!").unwrap();
        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        let s = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        assert!(s.contains("Hello, ")); // first run untouched
        assert!(s.contains("universe!")); // second run patched
        // The exact original w:t for the first run must be present verbatim.
        assert!(s.contains("<w:t>Hello, </w:t>"));
    }

    #[test]
    fn patch_across_line_break_refuses() {
        let mut e = engine();
        // p2 renders "Line one\nline two" — removing the line break is a
        // structural change.
        let err = e.patch_block("p2", "Line one line two").unwrap_err();
        assert!(matches!(err, OfficeError::PatchAcrossMarker(_)));
    }

    #[test]
    fn patch_within_multi_run_paragraph() {
        // p1 "Hello, world!" → "Hi world!" — the change region covers only
        // the tail of run 1; run 2 untouched.
        let mut e = engine();
        e.patch_block("p1", "Hi world!").unwrap();
        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        let s = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        // Run 1 now reads "Hi "; run 2's original bytes preserved exactly.
        assert!(s.contains("<w:t>Hi </w:t>"));
        assert!(s.contains("<w:t>world!</w:t>"));
        assert!(!s.contains("Hello, "));
    }

    #[test]
    fn cell_paragraph_is_patchable() {
        let mut e = engine();
        e.patch_block("t1:r1c1:p1", "cell A1 v2").unwrap();
        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        let s = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        assert!(s.contains("cell A1 v2"));
        assert!(s.contains("cell B1")); // other cell untouched
    }

    #[test]
    fn stale_edit_is_rejected() {
        let mut e = engine();
        // Mutate the part behind the engine's back, then patch. The engine
        // must refuse: either the block range no longer resolves (length
        // changed → BlockNotFound) or the rendered text mismatches (StaleEdit).
        let patched = crate::docx::patch_first_paragraph(
            crate::zip::tests::DOCUMENT_XML,
            "Changed behind your back!",
        )
        .unwrap();
        e.current.insert("word/document.xml".to_string(), patched);
        let err = e.patch_block("p1", "something else").unwrap_err();
        assert!(
            matches!(
                err,
                OfficeError::StaleEdit { .. } | OfficeError::BlockNotFound(_)
            ),
            "stale edit must be rejected, got: {err:?}"
        );
    }

    #[test]
    fn render_and_repatch_after_length_changing_edit() {
        // Regression: a length-changing patch shifts every later block's byte
        // range. Before the tree was refreshed, `render_block` then failed with
        // BlockNotFound and a second edit on a later block addressed stale XML.
        let mut e = engine();
        e.patch_block("p1", "A considerably longer first paragraph than before")
            .unwrap();

        assert_eq!(
            e.render_block("p1").unwrap(),
            "A considerably longer first paragraph than before"
        );
        // Later blocks still resolve at their new offsets.
        assert_eq!(e.render_block("p2").unwrap(), "Line one\nline two");
        assert_eq!(e.render_block("t1:r1c1:p1").unwrap(), "cell A1");
        assert!(e.render_text().starts_with("A considerably longer"));

        // A second edit on a later block is applied to the current XML.
        e.patch_block("p2", "Line one\nline two v2").unwrap();
        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        let s = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        assert!(s.contains("A considerably longer first paragraph than before"));
        assert!(s.contains("line two v2"));
        assert!(
            s.contains("cell B1"),
            "untouched cell must survive two edits"
        );
    }

    #[test]
    fn no_text_anchor_errors() {
        // A paragraph with only a drawing and no w:t cannot be patched.
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:drawing/></w:r></w:p></w:body>
</w:document>"#;
        let err = patch_first_paragraph(xml, "text").unwrap_err();
        assert!(matches!(err, OfficeError::NoTextAnchor));
    }

    #[test]
    fn escaped_text_roundtrips() {
        let mut e = engine();
        e.patch_block("p1", "a & b < c").unwrap();
        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        let s = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        assert!(s.contains("a &amp; b &lt; c"));
    }

    // P10.1.4 — office pipeline E2E: open → edit → save → reopen → verify
    // byte-stability. A no-op save must not change the document part bytes;
    // an edited save must leave every untouched part byte-identical.
    #[test]
    fn byte_stability_noop_save_and_roundtrip() {
        let original = crate::zip::tests::sample_docx();

        // 1. Open → save with NO edits: the document.xml part is byte-identical.
        let mut noop = DocxEngine::open(original.clone()).unwrap();
        let saved = noop.save().unwrap();
        let mut noop_archive = OoxmlArchive::open(saved).unwrap();
        let mut orig_archive = OoxmlArchive::open(original.clone()).unwrap();
        let before = orig_archive.read_part("word/document.xml").unwrap();
        let after = noop_archive.read_part("word/document.xml").unwrap();
        assert_eq!(before, after, "no-op save must not mutate the body part");

        // 2) open → edit → save → reopen: the edit is visible and the
        //    untouched parts stay byte-identical.
        let mut edited = DocxEngine::open(original).unwrap();
        edited.patch_block("p1", "Goodbye, world!").unwrap();
        let out = edited.save().unwrap();
        let reopened = DocxEngine::open(out.clone()).unwrap();
        assert_eq!(
            reopened.render_text(),
            "Goodbye, world!\nLine one\nline two\ncell A1 | cell B1\n"
        );
        let mut reopened_archive = OoxmlArchive::open(out).unwrap();
        // The untouched content-types + rels parts are preserved verbatim.
        let ct = reopened_archive.read_part("[Content_Types].xml").unwrap();
        assert!(String::from_utf8_lossy(&ct).contains("wordprocessingml.document.main+xml"));
        assert!(reopened_archive.read_part("_rels/.rels").is_ok());
        // And the body still parses as a valid document part. The patched
        // middle fragment is contiguous in the XML (the original text is split
        // across runs, so the full string is not — render_text proves the
        // round-trip instead).
        let xml =
            String::from_utf8(reopened_archive.read_part("word/document.xml").unwrap()).unwrap();
        assert!(xml.contains("Goodbye, "));
        assert!(xml.contains("world!"));
    }

    // ── ARCH/04 §4.6 — field balance, media GC, size ceiling ────────────────

    /// A docx whose body has one live image reference (`rId1`) and one page
    /// field; `rId2`'s image is reachable only through a paragraph that is
    /// not in the body, so it is the orphan the sweep must collect.
    fn docx_with_image_and_field(body: &str) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        let mut add = |name: &str, bytes: &[u8]| {
            w.start_file(name, opts).unwrap();
            w.write_all(bytes).unwrap();
        };
        add(
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        );
        add(
            "_rels/.rels",
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        );
        add(
            "word/_rels/document.xml.rels",
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.png"/></Relationships>"#,
        );
        add("word/document.xml", body.as_bytes());
        add("word/media/image1.png", b"PNGDATA-1");
        add("word/media/image2.png", b"PNGDATA-2");
        w.finish().unwrap().into_inner()
    }

    const BODY_WITH_LIVE_IMAGE_AND_FIELD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><w:body><w:p><w:r><w:t>See figure</w:t></w:r><w:r><w:drawing><wp:inline><a:blip r:embed="rId1"/></wp:inline></w:drawing></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:body></w:document>"#;

    #[test]
    fn save_reports_a_balanced_field_in_a_field_bearing_document() {
        let e =
            DocxEngine::open(docx_with_image_and_field(BODY_WITH_LIVE_IMAGE_AND_FIELD)).unwrap();
        let reports = e.field_report().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, "word/document.xml");
        assert_eq!(reports[0].1.fields, 1);
        assert_eq!(reports[0].1.dirty, 0);
        // A document with no complex fields reports nothing to verify.
        let plain = engine();
        assert!(plain.field_report().unwrap()[0].1.is_empty());
    }

    #[test]
    fn patch_into_a_field_paragraph_commits_and_stays_balanced() {
        // The page number is a real field: editing its cached result must keep
        // begin/separate/end intact.
        let mut e =
            DocxEngine::open(docx_with_image_and_field(BODY_WITH_LIVE_IMAGE_AND_FIELD)).unwrap();
        e.patch_block("p2", "2").unwrap();
        let out = e.save().unwrap();
        let reopened = DocxEngine::open(out).unwrap();
        assert_eq!(reopened.render_block("p2").unwrap(), "2");
        let reports = reopened.field_report().unwrap();
        assert_eq!(reports[0].1.fields, 1);
        assert_eq!(reports[0].1.dirty, 0);
    }

    #[test]
    fn an_unbalanced_field_refuses_the_patch_and_leaves_the_archive_intact() {
        // A body whose field is already unbalanced (an `end` with no `begin`)
        // must not be patchable into a committed document.
        let body = BODY_WITH_LIVE_IMAGE_AND_FIELD.replace(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        );
        let original = docx_with_image_and_field(&body);
        let mut source = OoxmlArchive::open(original.clone()).unwrap();
        let source_body = source.read_part(BODY_PART).unwrap();

        let mut e = DocxEngine::open(original).unwrap();
        let err = e.patch_block("p1", "See figure v2").unwrap_err();
        match err {
            OfficeError::FieldBalance {
                part,
                field,
                ref detail,
            } => {
                assert_eq!(part, BODY_PART);
                assert_eq!(field, 1, "the stray 'end' names field #1");
                assert!(detail.contains("end"), "{detail}");
            }
            other => panic!("expected FieldBalance, got {other:?}"),
        }

        // The engine's pending part is byte-identical to the input, so the
        // refused edit left no trace…
        assert_eq!(e.current[BODY_PART], source_body);
        // …and the commit gate refuses too, so no bytes are ever handed back
        // for `write_atomic`: the file on disk cannot change.
        let err = e.save().unwrap_err();
        assert!(
            matches!(err, OfficeError::FieldBalance { field: 1, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn save_refuses_a_part_left_unbalanced_outside_patch_block() {
        // Even if an unbalanced part reached the engine by another route
        // (a stale rels/content-types edit, a future mutation path), the
        // commit gate catches it and returns no bytes.
        let mut e =
            DocxEngine::open(docx_with_image_and_field(BODY_WITH_LIVE_IMAGE_AND_FIELD)).unwrap();
        let unbalanced = BODY_WITH_LIVE_IMAGE_AND_FIELD
            .replace(
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:body>"#,
                r#"</w:p></w:body>"#,
            )
            .into_bytes();
        e.current.insert(BODY_PART.to_string(), unbalanced);
        let err = e.save().unwrap_err();
        assert!(
            matches!(err, OfficeError::FieldBalance { field: 1, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn sweep_media_removes_the_orphan_payload_and_its_relationship() {
        let original = docx_with_image_and_field(BODY_WITH_LIVE_IMAGE_AND_FIELD);
        let mut e = DocxEngine::open(original).unwrap();
        let sweep = e.sweep_media().unwrap();
        assert_eq!(
            sweep.referenced,
            std::collections::BTreeSet::from(["rId1".into()])
        );
        assert_eq!(sweep.removed_rels, vec!["rId2".to_string()]);
        assert_eq!(
            sweep.removed_parts,
            vec!["word/media/image2.png".to_string()]
        );
        assert!(sweep.candidates.is_empty());

        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        // The orphan payload is gone and the rels entry went with it, in the
        // same commit: no relationship points at a missing part.
        assert!(a.read_part("word/media/image2.png").is_err());
        let rels = String::from_utf8(a.read_part("word/_rels/document.xml.rels").unwrap()).unwrap();
        assert!(!rels.contains("rId2"));
        assert!(rels.contains("rId1"), "the live reference survives: {rels}");
        // The still-referenced payload is untouched, and the body still parses.
        assert_eq!(a.read_part("word/media/image1.png").unwrap(), b"PNGDATA-1");
        let reopened = DocxEngine::open(e.save().unwrap()).unwrap();
        assert_eq!(reopened.render_text(), "See figure\n1\n");
    }

    #[test]
    fn sweep_media_is_a_noop_when_every_payload_is_referenced() {
        let original = docx_with_image_and_field(BODY_WITH_LIVE_IMAGE_AND_FIELD);
        // Reference the second image too (r:link, a reference form the sweep
        // over-approximates on purpose).
        let mut e = DocxEngine::open(original).unwrap();
        let patched = BODY_WITH_LIVE_IMAGE_AND_FIELD
            .replace(r#"r:embed="rId1""#, r#"r:embed="rId1" r:link="rId2""#);
        e.current
            .insert(BODY_PART.to_string(), patched.into_bytes());
        let sweep = e.sweep_media().unwrap();
        assert!(sweep.is_noop());
        assert!(sweep.orphan_rels.is_empty());
        let mut a = OoxmlArchive::open(e.save().unwrap()).unwrap();
        assert_eq!(a.read_part("word/media/image2.png").unwrap(), b"PNGDATA-2");
    }

    #[test]
    fn open_refuses_an_archive_over_the_ceiling_by_name() {
        let bytes = crate::zip::tests::sample_docx();
        let tight = crate::limits::PatchLimits {
            max_archive_bytes: (bytes.len() - 1) as u64,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = DocxEngine::open_with_limits(bytes, tight)
            .err()
            .expect("must refuse");
        match err {
            OfficeError::TooLarge {
                kind,
                actual,
                limit,
                ..
            } => {
                assert_eq!(kind, crate::limits::LimitKind::ArchiveBytes);
                assert!(actual > limit);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn open_refuses_an_oversized_part_before_parsing_it() {
        let bytes = crate::zip::tests::sample_docx();
        let mut a = OoxmlArchive::open(bytes.clone()).unwrap();
        let body_size = a.entry_size("word/document.xml").unwrap();
        let tight = crate::limits::PatchLimits {
            max_part_bytes: body_size - 1,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = DocxEngine::open_with_limits(bytes, tight)
            .err()
            .expect("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("word/document.xml"), "{msg}");
        assert!(matches!(
            err,
            OfficeError::TooLarge {
                kind: crate::limits::LimitKind::PartBytes,
                ..
            }
        ));
    }

    #[test]
    fn open_refuses_a_total_part_load_over_the_ceiling() {
        // Each part is under the per-part ceiling; together they are not.
        let bytes = crate::zip::tests::sample_docx();
        let mut a = OoxmlArchive::open(bytes.clone()).unwrap();
        let body = a.entry_size("word/document.xml").unwrap();
        let tight = crate::limits::PatchLimits {
            max_part_bytes: body,
            max_total_part_bytes: body, // only room for one part
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = DocxEngine::open_with_limits(bytes, tight)
            .err()
            .expect("must refuse");
        assert!(matches!(
            err,
            OfficeError::TooLarge {
                kind: crate::limits::LimitKind::TotalPartBytes,
                ..
            }
        ));
    }

    #[test]
    fn open_under_the_default_policy_still_works() {
        // The documented ceilings must not break the normal path.
        let e = DocxEngine::open_with_limits(
            crate::zip::tests::sample_docx(),
            crate::limits::PatchLimits::default_policy(),
        )
        .unwrap();
        assert_eq!(e.render_block("p1").unwrap(), "Hello, world!");
    }
}
