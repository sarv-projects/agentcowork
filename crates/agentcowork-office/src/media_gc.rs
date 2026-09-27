//! Orphaned-media garbage collection (ARCH/04 §4.6 — resource cleanup).
//!
//! Removing a paragraph that holds an image leaves two artefacts behind: the
//! media payload in `word/media/` (or `ppt/media/`, `xl/media/`) and the
//! `Relationship` entry in the part's own `_rels` part that pointed at it.
//! Neither is a corruption in itself, but the pair is what makes a package
//! grow without bound, and a relationship left pointing at a removed part is
//! exactly what makes Office offer to repair the file.
//!
//! The sweep therefore has one hard invariant and one soft goal:
//!
//! - **Hard:** never remove a part that is still referenced, and never leave a
//!   relationship pointing at a removed part. Both removals (the `Relationship`
//!   element and the media part) land in the *same* rebuild, so a package can
//!   never be observed in the inconsistent state between them.
//! - **Soft:** report, but do not remove, anything whose removal would change
//!   a part this engine cannot safely rewrite — see [`CandidateReason`].
//!
//! # Algorithm
//!
//! For one owning part: (1) collect every relationship id the part's XML still
//! names, in the relationships namespace, as an **over-approximation** (any
//! local name counts, so a reference form this code has never seen still keeps
//! its media alive — over-collecting leaks bytes, under-collecting corrupts);
//! (2) parse the part's `_rels` part and resolve each `Target` to a package
//! path relative to the owning part's directory; (3) an unreferenced
//! relationship whose target is an existing **media payload** is an orphan;
//! (4) an orphan media part is removed only when no other `_rels` part in the
//! package targets it and `[Content_Types].xml` carries no explicit `Override`
//! for it — otherwise it is reported as a cleanup candidate; (5) the
//! relationships part is rewritten by splicing out exactly the `Relationship`
//! elements being removed, so every other byte of it survives. If nothing is
//! removed, the relationships part is not rewritten at all.

use std::collections::{BTreeMap, BTreeSet};

use roxmltree::Node;

use crate::docx::OfficeError;
use crate::xml;

/// The office-document relationships namespace (`r:`).
pub const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// The package content-types part.
pub const CONTENT_TYPES: &str = "[Content_Types].xml";

/// Media/binary payload extensions. A payload is only a sweep candidate when
/// it is under a `media/` directory **or** carries one of these extensions;
/// XML and `.rels` parts are never swept by this module.
const MEDIA_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "emf", "wmf", "svg", "webp", "ico", "cur",
    "mp3", "mp4", "m4a", "wav", "aif", "aiff", "avi", "mov", "mpg", "mpeg", "wmv", "ogg", "webm",
    "bin", "emz", "thmx", "ttf", "otf", "eot", "fntdata", "odttf",
];

/// Relationship types that name a media/binary payload — the only ones whose
/// id an XML part is expected to reference with `r:id`/`r:embed`/`r:link`.
///
/// This distinction is load-bearing: a **structural** relationship (a slide's
/// `slideLayout`, a document's `styles`/`settings`/`numbering`, a `header`)
/// is meaningful whether or not the owning XML names it, so it is never
/// treated as an orphan. Only media relationships are swept, which is why a
/// live slide is not reported as "holding an orphan layout relationship".
const MEDIA_REL_SUFFIXES: &[&str] = &[
    "/image",
    "/audio",
    "/video",
    "/media",
    "/oleObject",
    "/package",
];

/// Is this relationship type one that names a media/binary payload?
pub fn is_media_rel_type(rel_type: &str) -> bool {
    MEDIA_REL_SUFFIXES
        .iter()
        .any(|suffix| rel_type.ends_with(suffix))
}

/// Why an orphan was reported instead of removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateReason {
    /// Another part's relationships still target this payload (shared media:
    /// the same image in the body and in a header, say). Removing it would
    /// break that other reference.
    SharedTarget { rels_part: String, rel_id: String },
    /// Some other `_rels` part in the package could not be read or parsed, so
    /// exclusivity cannot be proven. Fail closed: report, do not remove.
    UnresolvedReferrer { rels_part: String },
    /// `[Content_Types].xml` carries an explicit `Override` for this part, so
    /// removing it would also mean rewriting the content-types part — which
    /// this patch is not touching.
    ContentTypeOverride,
    /// The orphan does not point at a media payload (another part kind, or a
    /// target that does not exist). Out of this sweep's scope; reported so it
    /// is never silently dropped.
    NotMedia,
}

impl CandidateReason {
    /// Stable identifier for audit/receipt payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateReason::SharedTarget { .. } => "shared_target",
            CandidateReason::UnresolvedReferrer { .. } => "unresolved_referrer",
            CandidateReason::ContentTypeOverride => "content_type_override",
            CandidateReason::NotMedia => "not_media",
        }
    }
}

impl std::fmt::Display for CandidateReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CandidateReason::SharedTarget { rels_part, rel_id } => {
                write!(f, "still targeted by {rels_part} ({rel_id})")
            }
            CandidateReason::UnresolvedReferrer { rels_part } => {
                write!(f, "exclusivity unprovable: {rels_part} is unreadable")
            }
            CandidateReason::ContentTypeOverride => {
                f.write_str("[Content_Types].xml has an explicit Override for it")
            }
            CandidateReason::NotMedia => f.write_str("not a media payload"),
        }
    }
}

/// An orphan the sweep refused to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupCandidate {
    /// The relationship id that is no longer referenced.
    pub rel_id: String,
    /// The package path the relationship resolved to.
    pub part: String,
    pub reason: CandidateReason,
}

/// What one sweep found and did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaSweep {
    /// The owning part that was swept (e.g. `word/document.xml`).
    pub part: String,
    /// Its relationships part.
    pub rels_part: String,
    /// Relationship ids the part's XML still names.
    pub referenced: BTreeSet<String>,
    /// Relationship ids that are no longer named.
    pub orphan_rels: BTreeSet<String>,
    /// Relationship ids spliced out of the relationships part.
    pub removed_rels: Vec<String>,
    /// Media parts omitted from the rebuilt archive.
    pub removed_parts: Vec<String>,
    /// Orphans reported but not removed.
    pub candidates: Vec<CleanupCandidate>,
}

impl MediaSweep {
    /// True when the sweep changed nothing (the relationships part was left
    /// byte-identical, no part was removed).
    pub fn is_noop(&self) -> bool {
        self.removed_rels.is_empty() && self.removed_parts.is_empty()
    }
}

/// A sweep's executable result: the rewritten relationships part (absent when
/// nothing is removed) and the parts the rebuild must omit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPlan {
    pub sweep: MediaSweep,
    /// New bytes for [`MediaSweep::rels_part`]; `None` = leave it untouched.
    pub rels_bytes: Option<Vec<u8>>,
    /// Media parts the rebuild must omit (atomically, with the rels rewrite).
    pub remove_parts: Vec<String>,
}

/// The rels part that owns a part's relationships, e.g.
/// `word/document.xml` → `word/_rels/document.xml.rels`.
pub fn rels_part_for(part: &str) -> String {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    }
}

/// Resolve a relationship `Target` to a package path, relative to the owning
/// part's directory (`..` and `.` segments normalized). An empty `owner`
/// resolves against the package root (the `_rels/.rels` case).
pub fn resolve_target(target: &str, owner: &str) -> String {
    let target = target.split('#').next().unwrap_or(target);
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut segs: Vec<&str> = owner.split('/').collect();
    segs.pop(); // drop the owner's file name → its directory
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    segs.join("/")
}

/// The part a `_rels` part describes — `ppt/slides/_rels/slide1.xml.rels` →
/// `ppt/slides/slide1.xml`. `None` for the package's root `_rels/.rels`,
/// whose targets resolve against the package root (an empty owner).
pub fn owner_of_rels_part(rels_part: &str) -> Option<String> {
    let (dir, file) = rels_part.rsplit_once('/')?;
    if file == ".rels" {
        return None;
    }
    let dir = dir.strip_suffix("_rels")?.trim_end_matches('/');
    let stem = file.strip_suffix(".rels")?;
    if dir.is_empty() {
        Some(stem.to_string())
    } else {
        Some(format!("{dir}/{stem}"))
    }
}

/// Is `part` a media/binary payload this sweep may collect?
pub fn is_media_part(part: &str) -> bool {
    if part.ends_with(".xml") || part.ends_with(".rels") {
        return false;
    }
    if part.split('/').any(|s| s == "media") {
        return true;
    }
    let ext = part.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    MEDIA_EXTS.contains(&ext.as_str())
}

/// Every relationship id `part_xml` still names, in the relationships
/// namespace. Deliberately an over-approximation: any attribute in `r:` counts
/// (`r:id`, `r:embed`, `r:link`, and any other local name in that namespace).
pub fn referenced_relationship_ids(part_xml: &[u8]) -> Result<BTreeSet<String>, OfficeError> {
    let doc = xml::parse(part_xml)?;
    let mut out = BTreeSet::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        for attr in node.attributes() {
            if attr.namespace() == Some(R_NS) {
                out.insert(attr.value().to_string());
            }
        }
    }
    Ok(out)
}

/// Sweep the media owned by one part.
///
/// `read` supplies the *current* bytes of any package part (patched first,
/// then the original archive), and `part_names` is the full entry list of the
/// package, so the sweep can look at every other `_rels` part before deciding
/// that a payload is exclusively this part's.
pub fn sweep_part(
    part: &str,
    part_xml: &[u8],
    part_names: &BTreeSet<String>,
    read: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<SweepPlan, OfficeError> {
    let referenced = referenced_relationship_ids(part_xml)?;
    let rels_part = rels_part_for(part);
    let mut sweep = MediaSweep {
        part: part.to_string(),
        rels_part: rels_part.clone(),
        referenced: referenced.clone(),
        ..Default::default()
    };

    let Some(rels_bytes) = read(&rels_part) else {
        // No relationships part → nothing can be orphaned through it.
        return Ok(SweepPlan {
            sweep,
            rels_bytes: None,
            remove_parts: Vec::new(),
        });
    };
    let rels_doc = xml::parse(&rels_bytes)?;
    let rels = collect_rels(rels_doc);

    // Orphan media rel ids → the package path they resolve to. Only *media*
    // relationship types are considered: a structural rel (styles, theme,
    // slideLayout, header) is meaningful even when the XML never names it.
    let mut orphan_target: BTreeMap<String, Vec<String>> = BTreeMap::new(); // path -> rel ids
    for rel in &rels {
        if !is_media_rel_type(&rel.rel_type) || referenced.contains(&rel.id) {
            continue;
        }
        sweep.orphan_rels.insert(rel.id.clone());
        if rel.external {
            continue;
        }
        let path = resolve_target(&rel.target, part);
        if !is_media_part(&path) || read(&path).is_none() {
            sweep.candidates.push(CleanupCandidate {
                rel_id: rel.id.clone(),
                part: path,
                reason: CandidateReason::NotMedia,
            });
            continue;
        }
        orphan_target.entry(path).or_default().push(rel.id.clone());
    }

    // Content types: an explicit Override would also need rewriting.
    let overrides = content_type_overrides(read(CONTENT_TYPES).as_deref());

    // Other `_rels` parts: which payloads they target, and whether they are
    // readable at all.
    let mut other_targets: BTreeMap<String, (String, String)> = BTreeMap::new(); // path -> (rels part, rel id)
    let mut unreadable: Vec<String> = Vec::new();
    for name in part_names {
        if name == &rels_part || !name.ends_with(".rels") {
            continue;
        }
        let Some(bytes) = read(name) else {
            unreadable.push(name.clone());
            continue;
        };
        let Ok(doc) = xml::parse(&bytes) else {
            unreadable.push(name.clone());
            continue;
        };
        for rel in collect_rels(doc) {
            if rel.external || !is_media_rel_type(&rel.rel_type) {
                continue;
            }
            // Targets in another part's rels resolve against the part that
            // part describes, not against the rels path itself.
            let owner = owner_of_rels_part(name).unwrap_or_default();
            let path = resolve_target(&rel.target, &owner);
            other_targets
                .entry(path)
                .or_insert_with(|| (name.clone(), rel.id.clone()));
        }
    }

    // Decide each orphan payload.
    let mut removable_rel_ids: BTreeSet<String> = BTreeSet::new();
    let mut remove_parts: Vec<String> = Vec::new();
    for (path, rel_ids) in &orphan_target {
        if let Some((by_rels, by_rel)) = other_targets.get(path) {
            for id in rel_ids {
                sweep.candidates.push(CleanupCandidate {
                    rel_id: id.clone(),
                    part: path.clone(),
                    reason: CandidateReason::SharedTarget {
                        rels_part: by_rels.clone(),
                        rel_id: by_rel.clone(),
                    },
                });
            }
            continue;
        }
        if let Some(unreadable_part) = unreadable.first() {
            for id in rel_ids {
                sweep.candidates.push(CleanupCandidate {
                    rel_id: id.clone(),
                    part: path.clone(),
                    reason: CandidateReason::UnresolvedReferrer {
                        rels_part: unreadable_part.clone(),
                    },
                });
            }
            continue;
        }
        if overrides.contains(path) {
            for id in rel_ids {
                sweep.candidates.push(CleanupCandidate {
                    rel_id: id.clone(),
                    part: path.clone(),
                    reason: CandidateReason::ContentTypeOverride,
                });
            }
            continue;
        }
        removable_rel_ids.extend(rel_ids.iter().cloned());
        remove_parts.push(path.clone());
    }

    // Rewrite the relationships part by splicing out only the removed
    // `Relationship` elements — every other byte survives verbatim.
    let rels_bytes = if removable_rel_ids.is_empty() {
        None
    } else {
        let text = std::str::from_utf8(&rels_bytes)?;
        let doc = xml::parse(&rels_bytes)?;
        let mut edits: Vec<(usize, usize)> = Vec::new();
        for node in doc.descendants().filter(|n| n.is_element()) {
            if xml::local_name(node) != "Relationship" {
                continue;
            }
            let Some(id) = node.attribute("Id") else {
                continue;
            };
            if removable_rel_ids.contains(id) {
                edits.push((node.range().start, node.range().end));
            }
        }
        edits.sort_unstable();
        let mut out = rels_bytes.clone();
        for (start, end) in edits.into_iter().rev() {
            out.splice(start..end, Vec::new());
        }
        // The splice must still be valid XML (it is: only whole elements
        // were removed), and nothing else may have changed.
        let _ = text;
        Some(out)
    };

    sweep.removed_rels = removable_rel_ids.iter().cloned().collect();
    sweep.removed_parts = std::mem::take(&mut remove_parts);
    let remove_parts = sweep.removed_parts.clone();

    Ok(SweepPlan {
        sweep,
        rels_bytes,
        remove_parts,
    })
}

/// One `Relationship` entry, as the sweep needs it.
struct RelEntry {
    id: String,
    rel_type: String,
    target: String,
    external: bool,
}

fn collect_rels(doc: roxmltree::Document) -> Vec<RelEntry> {
    doc.descendants()
        .filter(|n| n.is_element() && xml::local_name(*n) == "Relationship")
        .filter_map(|node| {
            Some(RelEntry {
                id: node.attribute("Id")?.to_string(),
                rel_type: node.attribute("Type").unwrap_or("").to_string(),
                target: node.attribute("Target")?.to_string(),
                external: node
                    .attribute("TargetMode")
                    .map(|m| m.eq_ignore_ascii_case("External"))
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// `PartName` values carrying an explicit `Override` in `[Content_Types].xml`.
fn content_type_overrides(bytes: Option<&[u8]>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(bytes) = bytes else {
        return out;
    };
    let Ok(doc) = xml::parse(bytes) else {
        return out;
    };
    for node in doc.descendants().filter(|n| n.is_element()) {
        if xml::local_name(node) == "Override" {
            if let Some(part) = node.attribute("PartName") {
                out.insert(part.trim_start_matches('/').to_string());
            }
        }
    }
    out
}

/// Local name helper kept private to this module's use of `roxmltree::Node`.
#[allow(dead_code)]
fn is_element(node: Node) -> bool {
    node.is_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.png"/></Relationships>"#;

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="xml" ContentType="application/xml"/></Types>"#;

    /// A docx-shaped package: body with one live image ref, two media parts,
    /// and a rels part whose `rId2` (the removed paragraph's image) is the
    /// orphan.
    struct Pkg {
        body: Vec<u8>,
        read: BTreeSet<String>,
    }

    fn pkg(body: &str) -> Pkg {
        Pkg {
            body: body.as_bytes().to_vec(),
            read: [
                "word/document.xml",
                "word/_rels/document.xml.rels",
                "word/media/image1.png",
                "word/media/image2.png",
                "[Content_Types].xml",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }

    fn reader(p: &Pkg) -> impl FnMut(&str) -> Option<Vec<u8>> + '_ {
        move |name: &str| match name {
            "word/document.xml" => Some(p.body.clone()),
            "word/_rels/document.xml.rels" => Some(RELS.as_bytes().to_vec()),
            "word/media/image1.png" | "word/media/image2.png" => Some(b"PNGDATA".to_vec()),
            "[Content_Types].xml" => Some(CONTENT_TYPES.as_bytes().to_vec()),
            _ => None,
        }
    }

    const LIVE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><w:body><w:p><w:r><w:drawing><wp:inline xmlns:wp="x"><a:blip r:embed="rId1"/></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#;

    #[test]
    fn removes_the_orphan_payload_and_its_rel_entry() {
        let p = pkg(LIVE);
        let mut read = reader(&p);
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert_eq!(plan.sweep.referenced, BTreeSet::from(["rId1".to_string()]));
        assert_eq!(plan.sweep.orphan_rels, BTreeSet::from(["rId2".to_string()]));
        assert_eq!(plan.sweep.removed_rels, vec!["rId2".to_string()]);
        assert_eq!(plan.sweep.removed_parts, vec!["word/media/image2.png"]);
        assert!(plan.sweep.candidates.is_empty());
        assert!(!plan.sweep.is_noop());

        // The rels rewrite drops exactly the one element.
        let new_rels = String::from_utf8(plan.rels_bytes.unwrap()).unwrap();
        assert!(!new_rels.contains("rId2"));
        assert!(new_rels.contains("rId1"));
        assert!(new_rels.contains("media/image1.png"));
        assert_eq!(plan.remove_parts, vec!["word/media/image2.png"]);
    }

    #[test]
    fn a_fully_referenced_package_sweeps_to_a_noop_and_touches_no_bytes() {
        let body = LIVE.replace("r:embed=\"rId1\"", "r:embed=\"rId1\" r:link=\"rId2\"");
        let p = pkg(&body);
        let mut read = reader(&p);
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert!(plan.sweep.is_noop());
        assert!(plan.rels_bytes.is_none(), "rels must stay byte-identical");
        assert!(plan.remove_parts.is_empty());
        assert!(plan.sweep.orphan_rels.is_empty());
    }

    #[test]
    fn a_referenced_part_is_never_removed() {
        let p = pkg(LIVE);
        let mut read = reader(&p);
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert!(
            !plan
                .sweep
                .removed_parts
                .contains(&"word/media/image1.png".to_string())
        );
    }

    #[test]
    fn a_shared_payload_is_reported_not_removed() {
        // A header's rels also targets image2.png → the body does not own it.
        let p = pkg(LIVE);
        let mut read = |name: &str| {
            match name {
            "word/document.xml" => Some(p.body.clone()),
            "word/_rels/document.xml.rels" => Some(RELS.as_bytes().to_vec()),
            "ppt/slides/_rels/slide1.xml.rels" => Some(
                br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="x/image" Target="../../word/media/image2.png"/></Relationships>"#
                    .to_vec(),
            ),
            "word/media/image1.png" | "word/media/image2.png" => Some(b"PNG".to_vec()),
            "[Content_Types].xml" => Some(CONTENT_TYPES.as_bytes().to_vec()),
            _ => None,
        }
        };
        let mut names = p.read.clone();
        names.insert("ppt/slides/_rels/slide1.xml.rels".to_string());
        let plan = sweep_part("word/document.xml", &p.body, &names, &mut read).unwrap();
        assert!(plan.sweep.removed_parts.is_empty());
        assert!(plan.rels_bytes.is_none());
        assert_eq!(plan.sweep.candidates.len(), 1);
        assert_eq!(plan.sweep.candidates[0].part, "word/media/image2.png");
        assert_eq!(
            plan.sweep.candidates[0].reason,
            CandidateReason::SharedTarget {
                rels_part: "ppt/slides/_rels/slide1.xml.rels".to_string(),
                rel_id: "rId9".to_string(),
            }
        );
        assert_eq!(plan.sweep.candidates[0].reason.as_str(), "shared_target");
    }

    #[test]
    fn an_unreadable_other_rels_part_fails_closed() {
        let p = pkg(LIVE);
        let mut read = |name: &str| match name {
            "word/document.xml" => Some(p.body.clone()),
            "word/_rels/document.xml.rels" => Some(RELS.as_bytes().to_vec()),
            "word/media/image1.png" | "word/media/image2.png" => Some(b"PNG".to_vec()),
            "[Content_Types].xml" => Some(CONTENT_TYPES.as_bytes().to_vec()),
            // ppt/slides/_rels/slide1.xml.rels is listed but unreadable.
            _ => None,
        };
        let mut names = p.read.clone();
        names.insert("ppt/slides/_rels/slide1.xml.rels".to_string());
        let plan = sweep_part("word/document.xml", &p.body, &names, &mut read).unwrap();
        assert!(plan.sweep.removed_parts.is_empty());
        assert_eq!(plan.sweep.candidates.len(), 1);
        assert!(matches!(
            plan.sweep.candidates[0].reason,
            CandidateReason::UnresolvedReferrer { .. }
        ));
    }

    #[test]
    fn a_content_type_override_blocks_removal() {
        let ct = CONTENT_TYPES.replace(
            "<Default Extension=\"xml\"",
            "<Override PartName=\"/word/media/image2.png\" ContentType=\"image/png\"/><Default Extension=\"xml\"",
        );
        let p = pkg(LIVE);
        let mut read = |name: &str| match name {
            "word/document.xml" => Some(p.body.clone()),
            "word/_rels/document.xml.rels" => Some(RELS.as_bytes().to_vec()),
            "word/media/image1.png" | "word/media/image2.png" => Some(b"PNG".to_vec()),
            "[Content_Types].xml" => Some(ct.as_bytes().to_vec()),
            _ => None,
        };
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert!(plan.sweep.removed_parts.is_empty());
        assert_eq!(
            plan.sweep.candidates[0].reason,
            CandidateReason::ContentTypeOverride
        );
    }

    #[test]
    fn a_structural_orphan_relationship_is_never_swept() {
        // A `slideLayout`-style relationship is meaningful even though no
        // `r:id` in the owning XML names it: it must not be reported as an
        // orphan and must not be removed.
        let rels = RELS.replace(
            r#"Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.png""#,
            r#"Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml""#,
        );
        let p = pkg(LIVE);
        let mut read = |name: &str| match name {
            "word/document.xml" => Some(p.body.clone()),
            "word/_rels/document.xml.rels" => Some(rels.as_bytes().to_vec()),
            "word/media/image1.png" => Some(b"PNG".to_vec()),
            "word/styles.xml" => Some(b"<w:styles/>".to_vec()),
            "[Content_Types].xml" => Some(CONTENT_TYPES.as_bytes().to_vec()),
            _ => None,
        };
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert!(
            plan.sweep.orphan_rels.is_empty(),
            "{:?}",
            plan.sweep.orphan_rels
        );
        assert!(plan.sweep.candidates.is_empty());
        assert!(plan.sweep.is_noop());
    }

    #[test]
    fn a_media_relationship_with_no_payload_is_reported_not_removed() {
        // An image relationship whose target does not exist: reported, never
        // silently dropped (and the engine must not "remove" a missing part).
        let p = pkg(LIVE);
        let mut read = |name: &str| match name {
            "word/document.xml" => Some(p.body.clone()),
            "word/_rels/document.xml.rels" => Some(RELS.as_bytes().to_vec()),
            "word/media/image1.png" => Some(b"PNG".to_vec()),
            "[Content_Types].xml" => Some(CONTENT_TYPES.as_bytes().to_vec()),
            _ => None,
        };
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert!(plan.sweep.removed_parts.is_empty());
        assert!(plan.rels_bytes.is_none());
        assert_eq!(plan.sweep.candidates.len(), 1);
        assert_eq!(plan.sweep.candidates[0].rel_id, "rId2");
        assert_eq!(plan.sweep.candidates[0].reason, CandidateReason::NotMedia);
    }

    #[test]
    fn a_missing_rels_part_is_a_noop() {
        let p = pkg(LIVE);
        let mut read = |_: &str| None;
        let plan = sweep_part("word/document.xml", &p.body, &p.read, &mut read).unwrap();
        assert!(plan.sweep.is_noop());
        assert_eq!(plan.sweep.rels_part, "word/_rels/document.xml.rels");
    }

    #[test]
    fn target_resolution_handles_absolute_relative_and_dotdot() {
        assert_eq!(
            resolve_target("/word/media/a.png", "word/document.xml"),
            "word/media/a.png"
        );
        assert_eq!(
            resolve_target("media/a.png", "word/document.xml"),
            "word/media/a.png"
        );
        assert_eq!(
            resolve_target("../media/a.png", "ppt/slides/slide1.xml"),
            "ppt/media/a.png"
        );
        // A rels part's targets resolve against the part it *describes*.
        assert_eq!(
            resolve_target(
                "../../word/media/a.png",
                &owner_of_rels_part("ppt/slides/_rels/slide1.xml.rels").unwrap()
            ),
            "word/media/a.png"
        );
        // The package root rels resolve against the package root.
        assert_eq!(
            resolve_target(
                "word/document.xml",
                &owner_of_rels_part("_rels/.rels").unwrap_or_default()
            ),
            "word/document.xml"
        );
        assert_eq!(
            rels_part_for("word/document.xml"),
            "word/_rels/document.xml.rels"
        );
        assert_eq!(
            rels_part_for("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
    }

    #[test]
    fn media_rel_type_detection_covers_only_payloads() {
        assert!(is_media_rel_type(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
        ));
        assert!(is_media_rel_type(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject"
        ));
        assert!(!is_media_rel_type(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles"
        ));
        assert!(!is_media_rel_type(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout"
        ));
        assert!(!is_media_rel_type(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"
        ));
    }

    #[test]
    fn media_detection_excludes_xml_parts() {
        assert!(is_media_part("word/media/image1.png"));
        assert!(is_media_part("ppt/media/movie.mp4"));
        assert!(!is_media_part("word/document.xml"));
        assert!(!is_media_part("word/_rels/document.xml.rels"));
        assert!(!is_media_part("xl/charts/chart1.xml"));
        assert!(is_media_part("word/fonts/font1.odttf"));
    }

    #[test]
    fn reference_collection_over_approximates() {
        // A reference form the sweep has never seen still keeps its media.
        let body = br#"<d xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:weirdNewThing="rId2"/>"#;
        let ids = referenced_relationship_ids(body).unwrap();
        assert!(ids.contains("rId2"));
    }

    /// `(Id, Target)` of every `Relationship` in a rels part — used by the
    /// conformance test that asserts no relationship dangles after a sweep.
    pub(crate) fn rel_id_targets(rels: &str) -> Vec<(String, String)> {
        let doc = crate::xml::parse(rels.as_bytes()).expect("rels part parses");
        collect_rels(doc)
            .into_iter()
            .filter(|r| !r.external)
            .map(|r| (r.id, r.target))
            .collect()
    }
}
