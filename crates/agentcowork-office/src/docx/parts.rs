//! DOCX package structure (P4.1 item 1): content types + relationships.
//!
//! The parts index answers two questions the block tree needs:
//! - what is each part's content type (`[Content_Types].xml` defaults +
//!   overrides)?
//! - which parts are headers/footers of the body (`word/_rels/document.xml.rels`
//!   targets whose relationship type ends in `/header` or `/footer`)?

use std::collections::HashMap;

use crate::xml;

/// One relationship from `word/_rels/document.xml.rels`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rel {
    pub id: String,
    pub rel_type: String,
    pub target: String,
}

/// The parsed package index of a `.docx`.
#[derive(Debug, Default)]
pub struct PartsIndex {
    /// Content type per part name (overrides win over extension defaults).
    content_types: HashMap<String, String>,
    /// Body relationships (headers/footers, images, styles…).
    pub rels: Vec<Rel>,
}

impl PartsIndex {
    /// Parse `[Content_Types].xml` + `word/_rels/document.xml.rels`.
    pub fn parse(
        content_types_xml: &[u8],
        document_rels_xml: Option<&[u8]>,
    ) -> Result<Self, crate::OfficeError> {
        let mut index = PartsIndex::default();

        // Defaults: Extension -> ContentType.
        let ct = xml::parse(content_types_xml)?;
        for node in ct.descendants().filter(|n| n.is_element()) {
            let name = xml::local_name(node);
            match name {
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

        if let Some(rels_xml) = document_rels_xml {
            let rels = xml::parse(rels_xml)?;
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

    /// Relationships whose type ends with `/header` or `/footer`.
    pub fn header_footer_rels(&self) -> impl Iterator<Item = &Rel> {
        self.rels
            .iter()
            .filter(|r| r.rel_type.ends_with("/header") || r.rel_type.ends_with("/footer"))
    }

    /// Resolve a rel target to a part path. Targets are relative to
    /// `word/` for office-document relationships (headers live there).
    pub fn resolve_target(&self, rel: &Rel) -> String {
        if rel.target.starts_with('/') {
            rel.target.trim_start_matches('/').to_string()
        } else {
            format!("word/{}", rel.target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTENT_TYPES: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>
</Types>"#;

    const RELS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="document.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>
<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;

    #[test]
    fn parses_content_types_defaults_and_overrides() {
        let idx = PartsIndex::parse(CONTENT_TYPES, None).unwrap();
        assert_eq!(
            idx.content_type("/word/document.xml").unwrap(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
        );
        // Extension default for plain xml parts.
        assert_eq!(
            idx.content_type("/word/styles.xml").unwrap(),
            "application/xml"
        );
    }

    #[test]
    fn discovers_headers_and_footers_from_rels() {
        let idx = PartsIndex::parse(CONTENT_TYPES, Some(RELS)).unwrap();
        let hf: Vec<String> = idx
            .header_footer_rels()
            .map(|r| idx.resolve_target(r))
            .collect();
        assert_eq!(
            hf,
            vec![
                "word/header1.xml".to_string(),
                "word/footer1.xml".to_string()
            ]
        );
        // Images are not headers/footers.
        assert!(!hf.iter().any(|p| p.contains("image")));
    }

    #[test]
    fn rel_count_parses_all() {
        let idx = PartsIndex::parse(CONTENT_TYPES, Some(RELS)).unwrap();
        assert_eq!(idx.rels.len(), 4);
    }
}
