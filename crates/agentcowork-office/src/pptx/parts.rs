//! PPTX package structure (P4.3 item 2): content types + presentation rels +
//! slide ordering from `<p:sldIdLst>`.
//!
//! A `.pptx` is a ZIP whose parts live under `ppt/`. The deck's slide order is
//! not the archive order — it is the order of `<p:sldId>` entries in
//! `ppt/presentation.xml`, each pointing (via `r:id`) at a slide relationship
//! in `ppt/_rels/presentation.xml.rels`, whose `Target` is the slide part.

use std::collections::HashMap;

use crate::xml;

pub const CONTENT_TYPES: &str = "[Content_Types].xml";
pub const PRESENTATION: &str = "ppt/presentation.xml";
pub const PRESENTATION_RELS: &str = "ppt/_rels/presentation.xml.rels";

/// The presentationml namespace (`p:`).
pub const P_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
/// The drawingml namespace (`a:`).
pub const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// The office-document relationships namespace (`r:`).
pub const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Content type of a slide part.
pub const SLIDE_CT: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
/// Relationship type of a slide (presentation → slide).
pub const SLIDE_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";

/// One presentation relationship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rel {
    pub id: String,
    pub rel_type: String,
    pub target: String,
}

/// A slide in presentation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slide {
    /// The slide part, e.g. `ppt/slides/slide1.xml`.
    pub part: String,
    /// The `<p:sldId id="…">` id attribute.
    pub sld_id: u32,
    /// The presentation-rel `Id` (e.g. `rId2`).
    pub rel_id: String,
}

/// The parsed package index of a `.pptx`.
#[derive(Debug, Default)]
pub struct PptxParts {
    /// Content type per part name (overrides win over extension defaults).
    content_types: HashMap<String, String>,
    /// All presentation relationships.
    pub rels: Vec<Rel>,
    /// Slides in `<p:sldIdLst>` order.
    pub slides: Vec<Slide>,
}

impl PptxParts {
    /// Parse `[Content_Types].xml` + `ppt/presentation.xml` +
    /// `ppt/_rels/presentation.xml.rels` into an index.
    pub fn parse(
        content_types_xml: &[u8],
        presentation_xml: &[u8],
        presentation_rels_xml: &[u8],
    ) -> Result<Self, crate::OfficeError> {
        let mut index = PptxParts::default();

        // Content types: Defaults (Extension → ContentType) + Overrides.
        let ct = xml::parse(content_types_xml)?;
        for node in ct.descendants().filter(|n| n.is_element()) {
            match xml::local_name(node) {
                "Default" => {
                    if let (Some(ext), Some(ctype)) =
                        (node.attribute("Extension"), node.attribute("ContentType"))
                    {
                        index
                            .content_types
                            .insert(format!(".{ext}"), ctype.to_string());
                    }
                }
                "Override" => {
                    if let (Some(part), Some(ctype)) =
                        (node.attribute("PartName"), node.attribute("ContentType"))
                    {
                        index
                            .content_types
                            .insert(part.to_string(), ctype.to_string());
                    }
                }
                _ => {}
            }
        }

        // Presentation relationships.
        let rels = xml::parse(presentation_rels_xml)?;
        for node in rels.descendants().filter(|n| n.is_element()) {
            if xml::local_name(node) == "Relationship" {
                if let (Some(id), Some(rel_type), Some(target)) = (
                    node.attribute("Id"),
                    node.attribute("Type"),
                    node.attribute("Target"),
                ) {
                    index.rels.push(Rel {
                        id: id.to_string(),
                        rel_type: rel_type.to_string(),
                        target: target.to_string(),
                    });
                }
            }
        }

        // Slide order from <p:sldIdLst>. Each <p:sldId> has id + r:id; the
        // r:id resolves (via the rels) to a slide part.
        let pres = xml::parse(presentation_xml)?;
        if let Some(lst) = pres.descendants().find(|n| {
            n.is_element()
                && n.tag_name().namespace() == Some(P_NS)
                && xml::local_name(*n) == "sldIdLst"
        }) {
            for sld_id in lst.children().filter(|n| {
                n.is_element()
                    && n.tag_name().namespace() == Some(P_NS)
                    && xml::local_name(*n) == "sldId"
            }) {
                let id: Option<u32> = sld_id.attribute("id").and_then(|s| s.parse().ok());
                let rel_id = sld_id.attribute((R_NS, "id")).map(|s| s.to_string());
                if let (Some(id), Some(rel_id)) = (id, rel_id) {
                    if let Some(rel) = index.rels.iter().find(|r| r.id == rel_id) {
                        if rel.rel_type.ends_with("/slide") {
                            index.slides.push(Slide {
                                part: resolve_target(&rel.target),
                                sld_id: id,
                                rel_id,
                            });
                        }
                    }
                }
            }
        }

        Ok(index)
    }

    /// Content type of a part (override → extension default → unknown).
    pub fn content_type(&self, part: &str) -> Option<&str> {
        self.content_types
            .get(part)
            .or_else(|| {
                let ext = part.rsplit('.').next()?;
                self.content_types.get(&format!(".{ext}"))
            })
            .map(|s| s.as_str())
    }
}

/// Resolve a presentation-rel target to a part path (relative to `ppt/`).
pub fn resolve_target(target: &str) -> String {
    if target.starts_with('/') {
        target.trim_start_matches('/').to_string()
    } else {
        format!("ppt/{target}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTENT_TYPES: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
<Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"#;

    const PRESENTATION: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>
<p:sldIdLst>
<p:sldId id="256" r:id="rId2"/>
<p:sldId id="257" r:id="rId3"/>
</p:sldIdLst>
<p:sldSz cx="12192000" cy="6858000"/>
</p:presentation>"#;

    const RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/>
</Relationships>"#;

    #[test]
    fn parses_slide_order_from_sld_id_list() {
        let idx = PptxParts::parse(CONTENT_TYPES, PRESENTATION, RELS).unwrap();
        assert_eq!(idx.slides.len(), 2);
        assert_eq!(idx.slides[0].part, "ppt/slides/slide1.xml");
        assert_eq!(idx.slides[0].sld_id, 256);
        assert_eq!(idx.slides[0].rel_id, "rId2");
        assert_eq!(idx.slides[1].part, "ppt/slides/slide2.xml");
        assert_eq!(idx.slides[1].sld_id, 257);
        assert_eq!(idx.slides[1].rel_id, "rId3");
    }

    #[test]
    fn content_type_lookup_override_and_default() {
        let idx = PptxParts::parse(CONTENT_TYPES, PRESENTATION, RELS).unwrap();
        assert_eq!(
            idx.content_type("/ppt/slides/slide1.xml").unwrap(),
            SLIDE_CT
        );
        // Extension default for unknown xml parts.
        assert_eq!(
            idx.content_type("/ppt/theme/theme1.xml").unwrap(),
            "application/xml"
        );
    }

    #[test]
    fn slide_master_is_not_a_slide() {
        let idx = PptxParts::parse(CONTENT_TYPES, PRESENTATION, RELS).unwrap();
        // Only /slide relationships become slides; the master is ignored.
        assert!(idx.slides.iter().all(|s| s.part.starts_with("ppt/slides/")));
    }

    #[test]
    fn resolve_target_is_ppt_relative() {
        assert_eq!(resolve_target("slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(
            resolve_target("/ppt/slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
    }
}
