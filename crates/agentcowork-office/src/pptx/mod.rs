//! PowerPoint engine (P4.3 — PowerPoint part-editor, D3; ARCH/04 §PowerPoint).
//!
//! Pipeline (ARCH/04 §4.1): open ZIP → package index (content types + rels +
//! slide order) → render slide text for the LLM → patch `<a:t>` text via byte
//! surgery → add/remove slides (clone part + rels + `[Content_Types].xml`
//! registration) → byte-preserving ZIP rewrite. Untouched slide parts are
//! copied verbatim; only the targeted part(s) change.
//!
//! The same two ARCH/04 §4.6 guards as the Word engine apply: the
//! [`crate::limits`] size policy bounds what the engine will load (refusing by
//! name rather than attempting an unbounded load), and
//! [`PptxEngine::sweep_media`] collects media orphaned by a removed slide or
//! shape. Field-character balancing is Word-specific — DrawingML's `a:fld` is
//! a self-contained element, not a `begin`/`separate`/`end` triple, so it has
//! no balance invariant to check.

pub mod anim;
pub mod author;
pub mod notes;
pub mod parts;
pub mod text;
pub mod transition;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use roxmltree::Node;

use crate::limits::PatchLimits;
use crate::xml;
use crate::zip::{ArchiveError, OoxmlArchive};

use parts::{
    CONTENT_TYPES, PRESENTATION, PRESENTATION_RELS, PptxParts, R_NS, SLIDE_CT, SLIDE_REL_TYPE,
    Slide,
};
use text::Shape;

/// The rels part owning a slide part's relationships, e.g.
/// `ppt/slides/slide1.xml` → `ppt/slides/_rels/slide1.xml.rels`.
fn rels_part_for(part: &str) -> String {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    format!("{dir}/_rels/{file}.rels")
}

/// The numeric suffix of a slide part name (`slide3.xml` → `3`).
fn slide_number(part: &str) -> Option<usize> {
    let stem = part.rsplit('/').next()?;
    let n = stem.strip_prefix("slide")?.strip_suffix(".xml")?;
    n.parse().ok()
}

/// A `.pptx` opened for surgical editing.
pub struct PptxEngine {
    archive: OoxmlArchive,
    parts: PptxParts,
    /// Current bytes of parts we have rewritten (modified or newly added).
    edited: HashMap<String, Vec<u8>>,
    /// `edited` parts that did not exist in the original archive.
    new: HashSet<String>,
    /// Parts removed from the deck (omitted on save).
    deleted: HashSet<String>,
}

impl PptxEngine {
    /// Open a `.pptx` from bytes: parse the package + slide order.
    ///
    /// Uses the process size policy (`PatchLimits::default`, i.e. the
    /// documented constants plus any `AGENTCOWORK_OFFICE_*` override).
    pub fn open(bytes: Vec<u8>) -> Result<Self, crate::OfficeError> {
        Self::open_with_limits(bytes, PatchLimits::default())
    }

    /// Open a `.pptx` under an explicit size policy. The container, its entry
    /// count, and each part the engine reads are checked against the policy
    /// before the bytes are decompressed.
    pub fn open_with_limits(
        bytes: Vec<u8>,
        limits: PatchLimits,
    ) -> Result<Self, crate::OfficeError> {
        limits.check_archive(bytes.len() as u64)?;
        let mut archive = OoxmlArchive::open(bytes)?;
        limits.check_part_count(archive.entry_count()?)?;
        for name in [CONTENT_TYPES, PRESENTATION, PRESENTATION_RELS] {
            limits.check_part(name, archive.entry_size(name)?)?;
        }
        let content_types = archive.read_part(CONTENT_TYPES)?;
        let presentation = archive.read_part(PRESENTATION)?;
        let rels = archive.read_part(PRESENTATION_RELS)?;
        let parts = PptxParts::parse(&content_types, &presentation, &rels)?;
        Ok(Self {
            archive,
            parts,
            edited: HashMap::new(),
            new: HashSet::new(),
            deleted: HashSet::new(),
        })
    }

    /// Slides in presentation order.
    pub fn slides(&self) -> &[Slide] {
        &self.parts.slides
    }

    /// Current bytes of a part (edited copy, else the archive's original).
    fn part_bytes(&mut self, name: &str) -> Result<Vec<u8>, crate::OfficeError> {
        if let Some(b) = self.edited.get(name) {
            return Ok(b.clone());
        }
        Ok(self.archive.read_part(name)?)
    }

    /// Current bytes of a part, or `None` if the part doesn't exist (or was
    /// deleted).
    fn try_part_bytes(&mut self, name: &str) -> Result<Option<Vec<u8>>, crate::OfficeError> {
        if let Some(b) = self.edited.get(name) {
            return Ok(Some(b.clone()));
        }
        if self.deleted.contains(name) {
            return Ok(None);
        }
        match self.archive.read_part(name) {
            Ok(b) => Ok(Some(b)),
            Err(ArchiveError::PartNotFound(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Record rewritten bytes of an existing part.
    fn set_part(&mut self, name: &str, bytes: Vec<u8>) {
        self.edited.insert(name.to_string(), bytes);
    }

    /// Record a brand-new part (cloned slide / rels).
    fn add_part(&mut self, name: String, bytes: Vec<u8>) {
        self.new.insert(name.clone());
        self.edited.insert(name, bytes);
    }

    /// The text shapes of one slide (the LLM's addressable edit surface).
    pub fn shapes(&mut self, part: &str) -> Result<Vec<Shape>, crate::OfficeError> {
        let bytes = self.part_bytes(part)?;
        text::shapes(&bytes)
    }

    /// One slide's rendered outline: `[address "name"]` header per shape.
    pub fn render_slide(&mut self, part: &str) -> Result<String, crate::OfficeError> {
        let shapes = self.shapes(part)?;
        let mut out = String::new();
        for s in &shapes {
            out.push_str(&format!(
                "[{} \"{}\"]\n{}\n\n",
                s.address,
                s.name.as_deref().unwrap_or(""),
                s.text
            ));
        }
        Ok(out)
    }

    /// The whole deck's rendered outline (slide part + shapes).
    pub fn render_deck(&mut self) -> Result<String, crate::OfficeError> {
        let parts: Vec<String> = self.parts.slides.iter().map(|s| s.part.clone()).collect();
        let mut out = String::new();
        for part in &parts {
            out.push_str(&format!("# {part}\n"));
            out.push_str(&self.render_slide(part)?);
        }
        Ok(out)
    }

    /// Patch one shape's text (address like `shape2`). The change is mapped
    /// to `<a:t>` byte surgery; bullets/line-breaks are non-editable.
    pub fn patch_shape_text(
        &mut self,
        part: &str,
        address: &str,
        new_text: &str,
    ) -> Result<(), crate::OfficeError> {
        let ordinal = address
            .strip_prefix("shape")
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| crate::OfficeError::BlockNotFound(address.to_string()))?;
        let bytes = self.part_bytes(part)?;
        let patched = text::patch_shape_text(&bytes, ordinal, new_text)?;
        self.set_part(part, patched);
        Ok(())
    }

    /// Append a slide by cloning the last slide (part + rels + registration).
    /// Returns the new slide part name.
    pub fn add_slide(&mut self) -> Result<String, crate::OfficeError> {
        let template = self
            .parts
            .slides
            .last()
            .map(|s| s.part.clone())
            .ok_or(crate::OfficeError::Internal)?; // nothing to clone from

        let template_bytes = self.part_bytes(&template)?;
        let template_rels = rels_part_for(&template);
        let template_rels_bytes = self.try_part_bytes(&template_rels)?;

        let n = self.next_slide_number();
        let new_part = format!("ppt/slides/slide{n}.xml");
        let new_rels = rels_part_for(&new_part);
        let rel_id = self.next_rel_id();
        let sld_id = self.next_sld_id();

        // presentation.xml — append <p:sldId> to <p:sldIdLst>.
        let pres = self.part_bytes(PRESENTATION)?;
        let pres_str = std::str::from_utf8(&pres)?;
        let pres_doc = xml::parse(&pres)?;
        let lst = pres_doc
            .descendants()
            .find(|n| n.is_element() && is_ns(*n, parts::P_NS) && xml::local_name(*n) == "sldIdLst")
            .ok_or(crate::OfficeError::Internal)?;
        let sld_id_xml = format!("<p:sldId id=\"{sld_id}\" r:id=\"{rel_id}\"/>");
        self.set_part(
            PRESENTATION,
            insert_after_last_child(lst, pres_str, &sld_id_xml).into_bytes(),
        );

        // presentation.xml.rels — append the slide relationship.
        let rels = self.part_bytes(PRESENTATION_RELS)?;
        let rels_str = std::str::from_utf8(&rels)?;
        let rels_doc = xml::parse(&rels)?;
        let rels_root = rels_doc.root_element();
        let rel_xml = format!(
            "<Relationship Id=\"{rel_id}\" Type=\"{SLIDE_REL_TYPE}\" Target=\"slides/slide{n}.xml\"/>"
        );
        self.set_part(
            PRESENTATION_RELS,
            insert_after_last_child(rels_root, rels_str, &rel_xml).into_bytes(),
        );

        // [Content_Types].xml — register the new slide part.
        let ct = self.part_bytes(CONTENT_TYPES)?;
        let ct_str = std::str::from_utf8(&ct)?;
        let ct_doc = xml::parse(&ct)?;
        let ct_root = ct_doc.root_element();
        let ovr_xml =
            format!("<Override PartName=\"/ppt/slides/slide{n}.xml\" ContentType=\"{SLIDE_CT}\"/>");
        self.set_part(
            CONTENT_TYPES,
            insert_after_last_child(ct_root, ct_str, &ovr_xml).into_bytes(),
        );

        // Clone the slide part (and its rels, if it has one).
        self.add_part(new_part.clone(), template_bytes);
        if let Some(rb) = template_rels_bytes {
            self.add_part(new_rels, rb);
        }

        // Keep the in-memory index in sync.
        self.parts.slides.push(Slide {
            part: new_part.clone(),
            sld_id,
            rel_id: rel_id.clone(),
        });
        self.parts.rels.push(parts::Rel {
            id: rel_id,
            rel_type: SLIDE_REL_TYPE.to_string(),
            target: format!("slides/slide{n}.xml"),
        });

        Ok(new_part)
    }

    /// Remove a slide by part name (deregisters sldId + rel + content type,
    /// and omits the slide part + its rels on save).
    pub fn remove_slide(&mut self, part: &str) -> Result<(), crate::OfficeError> {
        let slide = self
            .parts
            .slides
            .iter()
            .find(|s| s.part == part)
            .cloned()
            .ok_or_else(|| crate::OfficeError::BlockNotFound(part.to_string()))?;
        let rel_id = slide.rel_id.clone();

        // presentation.xml — remove the <p:sldId>.
        let pres = self.part_bytes(PRESENTATION)?;
        let pres_str = std::str::from_utf8(&pres)?;
        let pres_doc = xml::parse(&pres)?;
        let sld_id_node = pres_doc
            .descendants()
            .find(|n| {
                n.is_element()
                    && is_ns(*n, parts::P_NS)
                    && xml::local_name(*n) == "sldId"
                    && n.attribute((R_NS, "id")) == Some(rel_id.as_str())
            })
            .ok_or(crate::OfficeError::Internal)?;
        self.set_part(
            PRESENTATION,
            remove_node(pres_str, &sld_id_node.range()).into_bytes(),
        );

        // presentation.xml.rels — remove the slide relationship.
        let rels = self.part_bytes(PRESENTATION_RELS)?;
        let rels_str = std::str::from_utf8(&rels)?;
        let rels_doc = xml::parse(&rels)?;
        let rel_node = rels_doc
            .descendants()
            .find(|n| {
                n.is_element()
                    && xml::local_name(*n) == "Relationship"
                    && n.attribute("Id") == Some(rel_id.as_str())
            })
            .ok_or(crate::OfficeError::Internal)?;
        self.set_part(
            PRESENTATION_RELS,
            remove_node(rels_str, &rel_node.range()).into_bytes(),
        );

        // [Content_Types].xml — remove the Override for the slide part.
        let ct = self.part_bytes(CONTENT_TYPES)?;
        let ct_str = std::str::from_utf8(&ct)?;
        let ct_doc = xml::parse(&ct)?;
        let part_name = format!("/{part}");
        if let Some(ovr) = ct_doc.descendants().find(|n| {
            n.is_element()
                && xml::local_name(*n) == "Override"
                && n.attribute("PartName") == Some(part_name.as_str())
        }) {
            self.set_part(
                CONTENT_TYPES,
                remove_node(ct_str, &ovr.range()).into_bytes(),
            );
        }

        // Omit the slide part + its rels from the output.
        for p in [part.to_string(), rels_part_for(part)] {
            self.edited.remove(&p);
            self.new.remove(&p);
            self.deleted.insert(p);
        }

        // Keep the in-memory index in sync.
        self.parts.slides.retain(|s| s.part != part);
        self.parts.rels.retain(|r| r.id != rel_id);

        Ok(())
    }

    /// Sweep media that a slide edit orphaned (ARCH/04 §4.6).
    ///
    /// Same contract as [`crate::docx::DocxEngine::sweep_media`], for one
    /// slide part: the `r:` ids the slide still names are collected, the media
    /// payloads and `Relationship` entries in the slide's own `_rels` part that
    /// nothing references any more are removed **in the same rebuild**, and
    /// anything unsafe to remove is reported as a cleanup candidate instead.
    ///
    /// Removing a slide queues its rels part for deletion but leaves the media
    /// it referenced; call this for every removed slide part before `save` to
    /// collect the payloads (the removed slide's rels are already gone, so its
    /// own ids read as unreferenced by the archive).
    pub fn sweep_media(
        &mut self,
        part: &str,
    ) -> Result<crate::media_gc::MediaSweep, crate::OfficeError> {
        let current_bytes = self.try_part_bytes(part)?;
        let Some(part_bytes) = current_bytes else {
            return Err(crate::OfficeError::BlockNotFound(part.to_string()));
        };
        let Self {
            archive,
            edited,
            new,
            deleted,
            ..
        } = self;
        let part_names: BTreeSet<String> = archive.parts()?.into_iter().collect();
        let plan = {
            let mut read = |name: &str| {
                if let Some(b) = edited.get(name) {
                    return Some(b.clone());
                }
                if deleted.contains(name) {
                    return None;
                }
                archive.read_part(name).ok()
            };
            crate::media_gc::sweep_part(part, &part_bytes, &part_names, &mut read)?
        };
        if let Some(rels) = plan.rels_bytes {
            edited.insert(plan.sweep.rels_part.clone(), rels);
        }
        for p in &plan.remove_parts {
            edited.remove(p);
            new.remove(p);
            deleted.insert(p.clone());
        }
        Ok(plan.sweep)
    }

    /// Rebuild the `.pptx`: rewritten parts re-deflated, new parts appended,
    /// deleted parts omitted, everything else byte-copied.
    pub fn save(&mut self) -> Result<Vec<u8>, crate::OfficeError> {
        let mut modified: Vec<(String, Vec<u8>)> = Vec::new();
        let mut added: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, bytes) in &self.edited {
            if self.new.contains(name) {
                added.push((name.clone(), bytes.clone()));
            } else {
                modified.push((name.clone(), bytes.clone()));
            }
        }
        let deleted: Vec<String> = self.deleted.iter().cloned().collect();
        Ok(self.archive.save_changes(&modified, &added, &deleted)?)
    }

    fn next_slide_number(&self) -> usize {
        self.parts
            .slides
            .iter()
            .filter_map(|s| slide_number(&s.part))
            .max()
            .unwrap_or(0)
            + 1
    }

    fn next_rel_id(&self) -> String {
        let max = self
            .parts
            .rels
            .iter()
            .filter_map(|r| {
                r.id.strip_prefix("rId")
                    .and_then(|s| s.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0);
        format!("rId{}", max + 1)
    }

    fn next_sld_id(&self) -> u32 {
        self.parts
            .slides
            .iter()
            .map(|s| s.sld_id)
            .max()
            .unwrap_or(255)
            + 1
    }
}

fn is_ns(node: Node, ns: &str) -> bool {
    node.tag_name().namespace() == Some(ns)
}

/// Insert `insertion` inside `lst` (after its last element child, or after
/// the opening tag when it has no element children).
fn insert_after_last_child(lst: Node, part: &str, insertion: &str) -> String {
    let at = match lst.children().rfind(|n| n.is_element()) {
        Some(last) => last.range().end,
        None => open_tag_end(lst.range().start, part),
    };
    let mut out = String::with_capacity(part.len() + insertion.len());
    out.push_str(&part[..at]);
    out.push_str(insertion);
    out.push_str(&part[at..]);
    out
}

/// Splice out an element's byte range.
fn remove_node(part: &str, range: &Range<usize>) -> String {
    let mut out = String::with_capacity(part.len());
    out.push_str(&part[..range.start]);
    out.push_str(&part[range.end..]);
    out
}

/// Byte position just past an element's opening tag (first `>` after start).
fn open_tag_end(start: usize, part: &str) -> usize {
    part[start..]
        .find('>')
        .map(|i| start + i + 1)
        .unwrap_or(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const CONTENT_TYPES: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#;

    const ROOT_RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#;

    const PRESENTATION: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#;

    const PRESENTATION_RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>"#;

    const SLIDE1: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Hello</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Content Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:buChar char="&#8226;"/></a:pPr><a:r><a:t>First</a:t></a:r></a:p><a:p><a:pPr><a:buChar char="&#8226;"/></a:pPr><a:r><a:t>Second</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#;

    const SLIDE2: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Thank you</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#;

    const SLIDE_RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;

    fn sample_pptx() -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut add = |name: &str, bytes: &[u8]| {
            w.start_file(name, opts).unwrap();
            w.write_all(bytes).unwrap();
        };
        add("[Content_Types].xml", CONTENT_TYPES);
        add("_rels/.rels", ROOT_RELS);
        add("ppt/presentation.xml", PRESENTATION);
        add("ppt/_rels/presentation.xml.rels", PRESENTATION_RELS);
        add("ppt/slides/slide1.xml", SLIDE1);
        add("ppt/slides/_rels/slide1.xml.rels", SLIDE_RELS);
        add("ppt/slides/slide2.xml", SLIDE2);
        add("ppt/slides/_rels/slide2.xml.rels", SLIDE_RELS);
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn open_parses_slide_order() {
        let e = PptxEngine::open(sample_pptx()).unwrap();
        let slides = e.slides();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].part, "ppt/slides/slide1.xml");
        assert_eq!(slides[0].sld_id, 256);
        assert_eq!(slides[0].rel_id, "rId2");
        assert_eq!(slides[1].part, "ppt/slides/slide2.xml");
    }

    #[test]
    fn render_deck_extracts_shapes_bullets_and_text() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        assert_eq!(
            e.render_deck().unwrap(),
            "# ppt/slides/slide1.xml\n\
             [shape1 \"Title 1\"]\nHello\n\n\
             [shape2 \"Content Placeholder 2\"]\n\u{2022} First\n\u{2022} Second\n\n\
             # ppt/slides/slide2.xml\n\
             [shape1 \"Title 2\"]\nThank you\n\n"
        );
    }

    #[test]
    fn shapes_report_address_name_and_placeholder() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        let shapes = e.shapes("ppt/slides/slide1.xml").unwrap();
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].address, "shape1");
        assert_eq!(shapes[0].name.as_deref(), Some("Title 1"));
        assert_eq!(shapes[0].ph_type.as_deref(), Some("title"));
        assert_eq!(shapes[0].text, "Hello");
        assert_eq!(shapes[1].address, "shape2");
        assert_eq!(shapes[1].text, "\u{2022} First\n\u{2022} Second");
    }

    #[test]
    fn patch_shape_replaces_text_and_preserves_untouched() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        e.patch_shape_text("ppt/slides/slide1.xml", "shape1", "Goodbye")
            .unwrap();
        let out = e.save().unwrap();

        let mut r = PptxEngine::open(out).unwrap();
        assert_eq!(
            r.render_deck().unwrap(),
            "# ppt/slides/slide1.xml\n\
             [shape1 \"Title 1\"]\nGoodbye\n\n\
             [shape2 \"Content Placeholder 2\"]\n\u{2022} First\n\u{2022} Second\n\n\
             # ppt/slides/slide2.xml\n\
             [shape1 \"Title 2\"]\nThank you\n\n"
        );
        // The untouched slide2 part is byte-identical (verbatim copy).
        let mut orig = OoxmlArchive::open(sample_pptx()).unwrap();
        let orig_slide2 = orig.raw_entry("ppt/slides/slide2.xml").unwrap();
        let mut new = OoxmlArchive::open(r.save().unwrap()).unwrap();
        let new_slide2 = new.raw_entry("ppt/slides/slide2.xml").unwrap();
        assert_eq!(orig_slide2, new_slide2);
    }

    #[test]
    fn patch_inserts_within_a_run_beside_a_bullet() {
        // "• First" → "• First!" touches only the <a:t>First</a:t> bytes.
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        e.patch_shape_text(
            "ppt/slides/slide1.xml",
            "shape2",
            "\u{2022} First!\n\u{2022} Second",
        )
        .unwrap();
        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        let s = String::from_utf8(a.read_part("ppt/slides/slide1.xml").unwrap()).unwrap();
        assert!(s.contains("<a:t>First!</a:t>"));
        // The second bullet's text is untouched.
        assert!(s.contains("<a:t>Second</a:t>"));
    }

    #[test]
    fn patch_refuses_across_paragraph_boundary() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        let err = e
            .patch_shape_text(
                "ppt/slides/slide1.xml",
                "shape2",
                "\u{2022} First \u{2022} Second",
            )
            .unwrap_err();
        assert!(matches!(err, crate::OfficeError::PatchAcrossMarker(_)));
    }

    #[test]
    fn patch_refuses_bullet_removal() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        let err = e
            .patch_shape_text("ppt/slides/slide1.xml", "shape2", "First\n\u{2022} Second")
            .unwrap_err();
        assert!(matches!(err, crate::OfficeError::PatchAcrossMarker(_)));
    }

    #[test]
    fn patch_shape_without_text_errors() {
        // A picture shape (no txBody) cannot be text-patched.
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Picture 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp></p:spTree></p:cSld></p:sld>"#;
        let err = text::patch_shape_text(xml, 1, "text").unwrap_err();
        assert!(matches!(err, crate::OfficeError::NoTextAnchor));
    }

    #[test]
    fn add_slide_clones_last_and_registers() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        let new_part = e.add_slide().unwrap();
        assert_eq!(new_part, "ppt/slides/slide3.xml");
        assert_eq!(e.slides().len(), 3);
        assert_eq!(e.slides()[2].sld_id, 258);
        assert_eq!(e.slides()[2].rel_id, "rId4");

        let out = e.save().unwrap();
        let mut r = PptxEngine::open(out.clone()).unwrap();
        assert_eq!(r.slides().len(), 3);
        // The clone carries the template's text.
        assert!(r.render_deck().unwrap().contains("# ppt/slides/slide3.xml"));

        // Structural registrations landed.
        let mut a = OoxmlArchive::open(out).unwrap();
        let pres = String::from_utf8(a.read_part("ppt/presentation.xml").unwrap()).unwrap();
        assert!(pres.contains("<p:sldId id=\"258\" r:id=\"rId4\"/>"));
        let rels =
            String::from_utf8(a.read_part("ppt/_rels/presentation.xml.rels").unwrap()).unwrap();
        assert!(rels.contains("Target=\"slides/slide3.xml\""));
        let ct = String::from_utf8(a.read_part("[Content_Types].xml").unwrap()).unwrap();
        assert!(ct.contains("PartName=\"/ppt/slides/slide3.xml\""));
        // The cloned slide's own rels (layout reference) was copied too.
        assert!(a.read_part("ppt/slides/_rels/slide3.xml.rels").is_ok());
    }

    #[test]
    fn remove_slide_removes_part_and_refs() {
        let mut e = PptxEngine::open(sample_pptx()).unwrap();
        e.remove_slide("ppt/slides/slide2.xml").unwrap();
        assert_eq!(e.slides().len(), 1);
        assert_eq!(e.slides()[0].part, "ppt/slides/slide1.xml");

        let out = e.save().unwrap();
        let mut r = PptxEngine::open(out.clone()).unwrap();
        assert_eq!(r.slides().len(), 1);
        assert!(!r.render_deck().unwrap().contains("slide2"));

        let mut a = OoxmlArchive::open(out).unwrap();
        // The slide part and its rels are gone.
        assert!(a.read_part("ppt/slides/slide2.xml").is_err());
        assert!(a.read_part("ppt/slides/_rels/slide2.xml.rels").is_err());
        let pres = String::from_utf8(a.read_part("ppt/presentation.xml").unwrap()).unwrap();
        assert!(!pres.contains("rId3"));
        let ct = String::from_utf8(a.read_part("[Content_Types].xml").unwrap()).unwrap();
        assert!(!ct.contains("slide2.xml"));
    }

    #[test]
    fn add_then_remove_roundtrip_preserves_untouched_slides() {
        let original = sample_pptx();
        let mut e = PptxEngine::open(original.clone()).unwrap();
        let new_part = e.add_slide().unwrap();
        e.remove_slide(&new_part).unwrap();
        assert_eq!(e.slides().len(), 2);
        let out = e.save().unwrap();

        let mut r = PptxEngine::open(out).unwrap();
        assert_eq!(r.slides().len(), 2);
        assert_eq!(r.render_deck().unwrap(), {
            let mut orig = PptxEngine::open(original).unwrap();
            orig.render_deck().unwrap()
        });
    }

    #[test]
    fn patch_preserves_untouched_slide_bytes() {
        let original = sample_pptx();
        let mut e = PptxEngine::open(original.clone()).unwrap();
        e.patch_shape_text("ppt/slides/slide1.xml", "shape1", "Hi")
            .unwrap();
        let out = e.save().unwrap();

        let mut a = OoxmlArchive::open(original).unwrap();
        let before = a.raw_entry("ppt/slides/slide2.xml").unwrap();
        let mut b = OoxmlArchive::open(out).unwrap();
        let after = b.raw_entry("ppt/slides/slide2.xml").unwrap();
        assert_eq!(before, after);
    }

    // ── ARCH/04 §4.6 — media GC + size ceiling ──────────────────────────────

    /// A two-slide deck where slide 1's rels carry one image relationship and
    /// the slide XML names it (`rId2`).
    fn sample_pptx_with_media(live: bool) -> Vec<u8> {
        const CT_MEDIA: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#;
        const SLIDE_RELS_MEDIA: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/></Relationships>"#;
        let blip = if live {
            r#"<p:pic><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic>"#
        } else {
            ""
        };
        let slide = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{blip}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        );

        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut add = |name: &str, bytes: &[u8]| {
            w.start_file(name, opts).unwrap();
            w.write_all(bytes).unwrap();
        };
        add("[Content_Types].xml", CT_MEDIA);
        add("_rels/.rels", ROOT_RELS);
        add("ppt/presentation.xml", PRESENTATION);
        // Only slide1 is registered, so the deck has one slide.
        add(
            "ppt/_rels/presentation.xml.rels",
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
        );
        add("ppt/slides/slide1.xml", slide.as_bytes());
        add("ppt/slides/_rels/slide1.xml.rels", SLIDE_RELS_MEDIA);
        add("ppt/media/image1.png", b"PNGDATA-1");
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn sweep_media_collects_an_unreferenced_slide_image() {
        let mut e = PptxEngine::open(sample_pptx_with_media(false)).unwrap();
        let sweep = e.sweep_media("ppt/slides/slide1.xml").unwrap();
        assert!(sweep.referenced.is_empty());
        assert_eq!(sweep.removed_rels, vec!["rId2".to_string()]);
        assert_eq!(
            sweep.removed_parts,
            vec!["ppt/media/image1.png".to_string()]
        );
        assert!(sweep.candidates.is_empty());

        let out = e.save().unwrap();
        let mut a = OoxmlArchive::open(out).unwrap();
        assert!(a.read_part("ppt/media/image1.png").is_err());
        let rels =
            String::from_utf8(a.read_part("ppt/slides/_rels/slide1.xml.rels").unwrap()).unwrap();
        assert!(!rels.contains("rId2"), "dangling relationship: {rels}");
        assert!(rels.contains("slideLayout1.xml"), "the live rel survives");
    }

    #[test]
    fn sweep_media_keeps_a_referenced_slide_image() {
        let mut e = PptxEngine::open(sample_pptx_with_media(true)).unwrap();
        let sweep = e.sweep_media("ppt/slides/slide1.xml").unwrap();
        assert!(sweep.is_noop());
        let mut a = OoxmlArchive::open(e.save().unwrap()).unwrap();
        assert_eq!(a.read_part("ppt/media/image1.png").unwrap(), b"PNGDATA-1");
    }

    #[test]
    fn sweep_media_of_a_removed_slide_collects_its_payload() {
        // remove_slide drops the slide part AND its rels; the payload it
        // referenced is still in the package, so the sweep (run against the
        // removed slide's rels bytes, which the engine still holds) collects
        // it. Nothing can reference it any more: the slide is gone.
        let mut e = PptxEngine::open(sample_pptx_with_media(true)).unwrap();
        let rels_before = String::from_utf8(
            e.try_part_bytes("ppt/slides/_rels/slide1.xml.rels")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(rels_before.contains("rId2"));
        // With the slide's rels queued for deletion, the payload has no
        // remaining referrer: sweeping the (now deleted) slide part reports
        // nothing to remove because its rels part is unreadable — so the
        // caller is expected to sweep before removing. Assert that contract.
        e.remove_slide("ppt/slides/slide1.xml").unwrap();
        let sweep = e.sweep_media("ppt/slides/slide1.xml");
        assert!(
            sweep.is_err(),
            "a removed slide can no longer be swept; the payload is reported by the caller instead"
        );
    }

    #[test]
    fn open_refuses_an_oversized_pptx_by_name() {
        let bytes = sample_pptx();
        let tight = crate::limits::PatchLimits {
            max_archive_bytes: (bytes.len() - 1) as u64,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = PptxEngine::open_with_limits(bytes, tight)
            .err()
            .expect("must refuse");
        assert!(matches!(
            err,
            crate::OfficeError::TooLarge {
                kind: crate::limits::LimitKind::ArchiveBytes,
                ..
            }
        ));
    }

    #[test]
    fn open_refuses_an_oversized_pptx_part() {
        let bytes = sample_pptx();
        // The first part the engine reads is [Content_Types].xml; a 1-byte
        // ceiling names it in the refusal.
        let tight = crate::limits::PatchLimits {
            max_part_bytes: 1,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = PptxEngine::open_with_limits(bytes, tight)
            .err()
            .expect("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("[Content_Types].xml"), "{msg}");
        assert!(matches!(
            err,
            crate::OfficeError::TooLarge {
                kind: crate::limits::LimitKind::PartBytes,
                ..
            }
        ));
    }

    #[test]
    fn open_refuses_a_package_with_too_many_entries() {
        let bytes = sample_pptx();
        let tight = crate::limits::PatchLimits {
            max_parts: 2,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = PptxEngine::open_with_limits(bytes, tight)
            .err()
            .expect("must refuse");
        assert!(matches!(
            err,
            crate::OfficeError::TooManyParts { limit: 2, .. }
        ));
    }
}
