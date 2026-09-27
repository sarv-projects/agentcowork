//! Author-new-deck path (P4.3 — doc 58 `ppt-master` pattern): "make me a deck
//! from this brief" = reason-then-native-shapes. [`author_deck`] builds a
//! minimal-but-valid `.pptx` package from a [`DeckBrief`] (title slide +
//! bullet slides + optional per-slide transition + speaker notes), emitting
//! the full package part set (presentation + master + layout + theme + one
//! slide part per slide) so the result opens cleanly in PowerPoint and
//! LibreOffice. The speaker-notes contract is satisfied by [`speaker_notes`],
//! which returns the `data-slide-id`-keyed `SPEAKER_NOTES` array.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::notes::{SpeakerNotesEntry, build_speaker_notes};
use super::transition::{Transition, set_transition};

/// PresentationML namespace.
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
/// DrawingML namespace.
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// Office relationships namespace.
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Package relationships namespace.
const CT_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT_OFFICE_DOC: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const CT_SLIDE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
const CT_SLIDE_MASTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
const CT_SLIDE_LAYOUT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
const CT_THEME: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";

/// One slide of the deck brief.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckSlide {
    pub title: String,
    pub bullets: Vec<String>,
    pub notes: Option<String>,
    pub transition: Option<Transition>,
}

impl DeckSlide {
    pub fn new(title: impl Into<String>, bullets: Vec<String>) -> Self {
        Self {
            title: title.into(),
            bullets,
            notes: None,
            transition: None,
        }
    }

    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    pub fn with_transition(mut self, transition: Transition) -> Self {
        self.transition = Some(transition);
        self
    }
}

/// The deck brief: a title + the slides.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckBrief {
    pub title: String,
    pub slides: Vec<DeckSlide>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("transition error: {0}")]
    Transition(#[from] super::transition::TransitionError),
}

/// Author a minimal valid `.pptx` from the brief. Returns the package bytes.
pub fn author_deck(brief: &DeckBrief) -> Result<Vec<u8>, AuthorError> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let n = brief.slides.len();
    let slide_ids: Vec<u32> = (0..n).map(|i| 256 + i as u32).collect();
    let slide_rel_ids: Vec<String> = (0..n).map(|i| format!("rId{}", i + 2)).collect();

    w.start_file("[Content_Types].xml", opts)?;
    w.write_all(content_types(n).as_bytes())?;

    w.start_file("_rels/.rels", opts)?;
    w.write_all(root_rels().as_bytes())?;

    w.start_file("ppt/presentation.xml", opts)?;
    w.write_all(presentation_xml(&slide_ids, &slide_rel_ids).as_bytes())?;

    w.start_file("ppt/_rels/presentation.xml.rels", opts)?;
    w.write_all(presentation_rels(&slide_rel_ids).as_bytes())?;

    w.start_file("ppt/slideMasters/slideMaster1.xml", opts)?;
    w.write_all(slide_master_xml().as_bytes())?;

    w.start_file("ppt/slideMasters/_rels/slideMaster1.xml.rels", opts)?;
    w.write_all(slide_master_rels().as_bytes())?;

    w.start_file("ppt/slideLayouts/slideLayout1.xml", opts)?;
    w.write_all(slide_layout_xml().as_bytes())?;

    w.start_file("ppt/slideLayouts/_rels/slideLayout1.xml.rels", opts)?;
    w.write_all(slide_layout_rels().as_bytes())?;

    w.start_file("ppt/theme/theme1.xml", opts)?;
    w.write_all(theme_xml().as_bytes())?;

    for (i, slide) in brief.slides.iter().enumerate() {
        let part = format!("ppt/slides/slide{}.xml", i + 1);
        w.start_file(&part, opts)?;
        w.write_all(slide_xml(slide)?.as_bytes())?;

        let rels_part = format!("ppt/slides/_rels/slide{}.xml.rels", i + 1);
        w.start_file(&rels_part, opts)?;
        w.write_all(slide_rels().as_bytes())?;
    }

    Ok(w.finish()?.into_inner())
}

/// The `SPEAKER_NOTES` array keyed by the slide's `p:sldId` (the
/// `data-slide-id` the presenter view keys on). Parallel to the authored deck.
pub fn speaker_notes(brief: &DeckBrief) -> Vec<SpeakerNotesEntry> {
    let triples: Vec<(String, String, String)> = brief
        .slides
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let id = format!("{}", 256 + i as u32);
            let title = s.title.clone();
            let notes = s.notes.clone().unwrap_or_default();
            (id, title, notes)
        })
        .collect();
    build_speaker_notes(&triples)
}

// ---------------------------------------------------------------------------
// Package part generation
// ---------------------------------------------------------------------------

fn content_types(n_slides: usize) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#,
    );
    for i in 0..n_slides {
        out.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
            i + 1
        ));
    }
    out.push_str("</Types>");
    out
}

fn root_rels() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{CT_RELS}"><Relationship Id="rId1" Type="{CT_OFFICE_DOC}" Target="ppt/presentation.xml"/></Relationships>"#
    )
}

fn presentation_xml(slide_ids: &[u32], slide_rel_ids: &[String]) -> String {
    let mut sld_ids = String::new();
    for (id, rel) in slide_ids.iter().zip(slide_rel_ids.iter()) {
        sld_ids.push_str(&format!("<p:sldId id=\"{id}\" r:id=\"{rel}\"/>"));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="{A}" xmlns:r="{R}" xmlns:p="{P}"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{sld_ids}</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#
    )
}

fn presentation_rels(slide_rel_ids: &[String]) -> String {
    let mut rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{CT_RELS}"><Relationship Id="rId1" Type="{CT_SLIDE_MASTER}" Target="slideMasters/slideMaster1.xml"/>"#
    );
    for (i, rel) in slide_rel_ids.iter().enumerate() {
        rels.push_str(&format!(
            "<Relationship Id=\"{rel}\" Type=\"{CT_SLIDE}\" Target=\"slides/slide{}.xml\"/>",
            i + 1
        ));
    }
    rels.push_str("</Relationships>");
    rels
}

fn slide_master_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="{A}" xmlns:r="{R}" xmlns:p="{P}"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#
    )
}

fn slide_master_rels() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{CT_RELS}"><Relationship Id="rId1" Type="{CT_SLIDE_LAYOUT}" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{CT_THEME}" Target="../theme/theme1.xml"/></Relationships>"#
    )
}

fn slide_layout_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="{A}" xmlns:r="{R}" xmlns:p="{P}" type="blank" preserve="1"><p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
    )
}

fn slide_layout_rels() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{CT_RELS}"><Relationship Id="rId1" Type="{CT_SLIDE_MASTER}" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#
    )
}

fn theme_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="{A}" name="AgentCowork Theme"><a:themeElements><a:clrScheme name="AgentCowork"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4><a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="AgentCowork"><a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="AgentCowork"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
    )
}

fn slide_xml(slide: &DeckSlide) -> Result<String, AuthorError> {
    let title = crate::xml::escape_text(&slide.title);
    let bullets = if slide.bullets.is_empty() {
        "<a:p/>".to_string()
    } else {
        slide
            .bullets
            .iter()
            .map(|b| {
                format!(
                    "<a:p><a:pPr><a:buChar char=\"&#8226;\"/></a:pPr><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r></a:p>",
                    crate::xml::escape_text(b)
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="{A}" xmlns:r="{R}" xmlns:p="{P}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Content Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{bullets}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    );
    if let Some(t) = &slide.transition {
        xml = set_transition(&xml, t)?;
    }
    Ok(xml)
}

fn slide_rels() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{CT_RELS}"><Relationship Id="rId1" Type="{CT_SLIDE_LAYOUT}" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#
    )
}

#[cfg(test)]
mod tests {
    use super::super::transition::{TransitionKind, extract_transition};
    use super::*;
    use crate::pptx::PptxEngine;

    fn brief() -> DeckBrief {
        DeckBrief {
            title: "Q3 Review".into(),
            slides: vec![
                DeckSlide::new("Welcome", vec!["hello".into(), "agenda".into()])
                    .with_notes("Greet everyone"),
                DeckSlide::new("Budget", vec!["numbers".into()])
                    .with_notes("Show the chart")
                    .with_transition(Transition {
                        kind: TransitionKind::Fade,
                        speed: Some("med".into()),
                        advance_ms: None,
                    }),
            ],
        }
    }

    #[test]
    fn author_deck_builds_valid_pptx() {
        let bytes = author_deck(&brief()).unwrap();
        // Opens as a valid OOXML package.
        let e = PptxEngine::open(bytes).unwrap();
        assert_eq!(e.slides().len(), 2);
    }

    #[test]
    fn author_deck_renders_titles_and_bullets() {
        let bytes = author_deck(&brief()).unwrap();
        let mut e = PptxEngine::open(bytes).unwrap();
        let deck = e.render_deck().unwrap();
        assert!(deck.contains("# ppt/slides/slide1.xml"), "{deck}");
        assert!(deck.contains("\"Title 1\""));
        assert!(deck.contains("Welcome"));
        assert!(deck.contains("\u{2022} hello"));
        assert!(deck.contains("\u{2022} agenda"));
        assert!(deck.contains("Budget"));
    }

    #[test]
    fn author_deck_applies_transition() {
        let bytes = author_deck(&brief()).unwrap();
        let mut a = crate::zip::OoxmlArchive::open(bytes).unwrap();
        let xml = String::from_utf8(a.read_part("ppt/slides/slide2.xml").unwrap()).unwrap();
        let t = extract_transition(&xml).unwrap().unwrap();
        assert_eq!(t.kind, TransitionKind::Fade);
        assert_eq!(t.speed.as_deref(), Some("med"));
    }

    #[test]
    fn author_deck_escapes_specials() {
        let mut b = brief();
        b.slides[0].title = "A & B < C".into();
        let bytes = author_deck(&b).unwrap();
        let mut e = PptxEngine::open(bytes).unwrap();
        let deck = e.render_deck().unwrap();
        assert!(deck.contains("A & B < C"), "{deck}");
    }

    #[test]
    fn speaker_notes_keys_by_slide_id() {
        let notes = speaker_notes(&brief());
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].slide_id, "256");
        assert_eq!(notes[0].talk, "Greet everyone");
        assert_eq!(notes[1].slide_id, "257");
        assert_eq!(notes[1].talk, "Show the chart");
        // Synced against the authored slide ids.
        let slide_ids: Vec<String> = (0..2).map(|i| format!("{}", 256 + i)).collect();
        assert!(crate::pptx::notes::validate_slides_notes_sync(&slide_ids, &notes).is_empty());
    }
}
