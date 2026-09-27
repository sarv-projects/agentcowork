//! CSL citation insertion (D2-gap — doc 63 §4.16, obsidian-zotero-integration
//! pattern): cite-while-writing in docx. A minimal CSL renderer for the
//! common reference types (book / article / webpage) in the common styles
//! (APA / Chicago / IEEE). The full CSL spec is enormous; this covers the
//! 90% cite-while-writing path and is extended per-style on demand.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceKind {
    Book,
    Article,
    Webpage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub id: String,
    pub kind: ReferenceKind,
    pub title: String,
    pub authors: Vec<String>,
    pub year: u32,
    /// Book publisher / article journal / webpage site.
    pub publisher: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CslStyle {
    Apa,
    Chicago,
    Ieee,
}

/// "Surname, I." (APA/Chicago) or "I. Surname" (IEEE). The surname is the
/// LAST word; given names become initials.
fn author_list(authors: &[String], style: CslStyle) -> String {
    let fmt = |a: &str| {
        let mut words: Vec<&str> = a.split_whitespace().collect();
        if words.is_empty() {
            return a.to_string();
        }
        let surname = words.pop().unwrap();
        let initials: String = words
            .iter()
            .filter_map(|w| w.chars().next())
            .map(|c| format!("{c}."))
            .collect::<Vec<_>>()
            .join(" ");
        match style {
            CslStyle::Ieee => {
                if initials.is_empty() {
                    surname.to_string()
                } else {
                    format!("{initials} {surname}")
                }
            }
            _ => {
                if initials.is_empty() {
                    surname.to_string()
                } else {
                    format!("{surname}, {initials}")
                }
            }
        }
    };
    match authors.len() {
        0 => String::new(),
        1 => fmt(&authors[0]),
        2 => format!("{} & {}", fmt(&authors[0]), fmt(&authors[1])),
        _ => format!("{} et al.", fmt(&authors[0])),
    }
}

/// In-text citation (e.g. "(Smith, 2024)" APA / "[1]" IEEE / "Smith 2024" Chicago).
pub fn render_citation(reference: &Reference, style: CslStyle) -> String {
    match style {
        CslStyle::Apa => format!(
            "({}, {})",
            author_list(&reference.authors, style),
            reference.year
        ),
        CslStyle::Chicago => format!(
            "{} {}",
            author_list(&reference.authors, style),
            reference.year
        ),
        CslStyle::Ieee => format!("[{}]", reference.id),
    }
}

/// A full bibliography entry.
pub fn render_reference(reference: &Reference, style: CslStyle) -> String {
    let authors = author_list(&reference.authors, style);
    let year = reference.year;
    match (style, reference.kind) {
        (CslStyle::Apa, ReferenceKind::Book) => {
            let pub_ = reference.publisher.as_deref().unwrap_or("n.p.");
            format!("{authors} ({year}). {}. {pub_}.", reference.title)
        }
        (CslStyle::Apa, ReferenceKind::Article) => {
            let j = reference.publisher.as_deref().unwrap_or("n.j.");
            format!("{authors} ({year}). {}. {j}.", reference.title)
        }
        (CslStyle::Apa, ReferenceKind::Webpage) => {
            let url = reference.url.as_deref().unwrap_or("");
            format!("{authors} ({year}). {}. {url}", reference.title)
        }
        (CslStyle::Chicago, ReferenceKind::Book) => {
            let pub_ = reference.publisher.as_deref().unwrap_or("n.p.");
            format!("{authors}. {}. {pub_}, {year}.", reference.title)
        }
        (CslStyle::Chicago, _) => {
            let url = reference.url.as_deref().unwrap_or("");
            format!("{authors}. \"{}.\" {year}. {url}", reference.title)
        }
        (CslStyle::Ieee, ReferenceKind::Book) => {
            let pub_ = reference.publisher.as_deref().unwrap_or("n.p.");
            format!("{authors}, {}, {pub_}, {year}.", reference.title)
        }
        (CslStyle::Ieee, ReferenceKind::Article) => {
            let j = reference.publisher.as_deref().unwrap_or("n.j.");
            format!("{authors}, \"{},\" {j}, {year}.", reference.title)
        }
        (CslStyle::Ieee, ReferenceKind::Webpage) => {
            let url = reference.url.as_deref().unwrap_or("");
            format!("{authors}, \"{}\", {url}, {year}.", reference.title)
        }
    }
}

// ---------------------------------------------------------------------------
// Local reference library + docx insertion (the "library search + insert
// into a docx" wiring half — doc 63 §4.16)
// ---------------------------------------------------------------------------

/// A local reference library (the obsidian-zotero-style source the citation
/// insertion flow searches). In-memory + serde-serializable so the harness
/// can persist it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferenceLibrary {
    references: Vec<Reference>,
}

impl ReferenceLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, reference: Reference) {
        // Idempotent per id — re-inserting replaces.
        self.references.retain(|r| r.id != reference.id);
        self.references.push(reference);
    }

    pub fn get(&self, id: &str) -> Option<&Reference> {
        self.references.iter().find(|r| r.id == id)
    }

    pub fn all(&self) -> &[Reference] {
        &self.references
    }

    pub fn len(&self) -> usize {
        self.references.len()
    }

    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }

    /// Search the library by token overlap on title/authors (case-insensitive,
    /// deterministic order: match count desc, then title asc). `None` query
    /// returns everything.
    pub fn search(&self, query: &str) -> Vec<&Reference> {
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_lowercase())
            .collect();
        let mut scored: Vec<(usize, &Reference)> = self
            .references
            .iter()
            .map(|r| {
                let haystack = format!("{} {}", r.title, r.authors.join(" ")).to_lowercase();
                let hits = terms
                    .iter()
                    .filter(|t| haystack.contains(t.as_str()))
                    .count();
                (hits, r)
            })
            .collect();
        scored.sort_by(|(a, ra), (b, rb)| b.cmp(a).then_with(|| ra.title.cmp(&rb.title)));
        // Only references with at least one matching term are results.
        scored
            .into_iter()
            .filter(|(hits, _)| *hits > 0)
            .map(|(_, r)| r)
            .collect()
    }
}

/// Insert a rendered citation as a new paragraph in a `.docx` (before the
/// body's `w:sectPr` — the standard bibliography anchor). Returns the patched
/// document bytes. `docx_bytes` is the whole `.docx`; only `word/document.xml`
/// is rewritten.
///
/// The rewritten part is verified for `w:fldChar` balance before the archive
/// is rebuilt, so a citation insert can never commit an unbalanced field
/// (ARCH/04 §4.6). The caller gets an error and the original bytes back —
/// never a half-valid package.
pub fn insert_citation_into_docx(
    docx_bytes: &[u8],
    citation_text: &str,
) -> Result<Vec<u8>, CitationInsertError> {
    let mut archive = crate::zip::OoxmlArchive::open(docx_bytes.to_vec())?;
    let doc = archive.read_part("word/document.xml")?;
    let mut xml = String::from_utf8_lossy(&doc).to_string();
    let escaped = citation_text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let para = format!("<w:p><w:r><w:t xml:space=\"preserve\">{escaped}</w:t></w:r></w:p>");
    // Insert before the closing </w:body> (which precedes sectPr? sectPr is a
    // body child — insert before the final sectPr so the paragraph stays in
    // the body flow).
    if let Some(pos) = xml.rfind("</w:sectPr>") {
        xml.insert_str(pos, &para);
    } else if let Some(pos) = xml.rfind("</w:body>") {
        xml.insert_str(pos, &para);
    } else {
        return Err(CitationInsertError::Archive(
            crate::zip::ArchiveError::PartNotFound("no body anchor in word/document.xml".into()),
        ));
    }
    let patched = xml.into_bytes();
    crate::docx::field_balance::verify("word/document.xml", &patched)?;
    Ok(archive.save(&[("word/document.xml".to_string(), patched)])?)
}

/// Failure modes of [`insert_citation_into_docx`]: a package/archive problem,
/// or a field-balance refusal that leaves the source file untouched.
#[derive(Debug, thiserror::Error)]
pub enum CitationInsertError {
    #[error("archive error: {0}")]
    Archive(#[from] crate::zip::ArchiveError),
    #[error("xml error: {0}")]
    Xml(#[from] crate::xml::OfficeXmlError),
    #[error(
        "unbalanced field characters: field #{field} in {part}: {detail} (citation not inserted; the file was not modified)"
    )]
    FieldBalance {
        part: String,
        field: usize,
        detail: String,
    },
    /// Any other engine refusal, carried verbatim so nothing is hidden.
    #[error("{0}")]
    Other(String),
}

impl From<crate::docx::OfficeError> for CitationInsertError {
    fn from(e: crate::docx::OfficeError) -> Self {
        match e {
            crate::docx::OfficeError::FieldBalance {
                part,
                field,
                detail,
            } => CitationInsertError::FieldBalance {
                part,
                field,
                detail,
            },
            crate::docx::OfficeError::Archive(a) => CitationInsertError::Archive(a),
            crate::docx::OfficeError::Xml(x) => CitationInsertError::Xml(x),
            other => CitationInsertError::Other(other.to_string()),
        }
    }
}

/// Render a full bibliography from a reference list.
pub fn render_bibliography(references: &[Reference], style: CslStyle) -> String {
    let mut entries: Vec<String> = references
        .iter()
        .map(|r| render_reference(r, style))
        .collect();
    // IEEE orders by citation id; APA/Chicago alphabetical by first author.
    if style != CslStyle::Ieee {
        entries.sort();
    }
    entries.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Reference {
        Reference {
            id: "1".into(),
            kind: ReferenceKind::Book,
            title: "Designing Agents".into(),
            authors: vec!["Ada Lovelace".into()],
            year: 2024,
            publisher: Some("MIT Press".into()),
            url: None,
        }
    }

    #[test]
    fn apa_book_renders() {
        let out = render_reference(&book(), CslStyle::Apa);
        assert_eq!(out, "Lovelace, A. (2024). Designing Agents. MIT Press.");
    }

    #[test]
    fn ieee_article_renders() {
        let r = Reference {
            id: "2".into(),
            kind: ReferenceKind::Article,
            title: "On Retrieval".into(),
            authors: vec!["Grace Hopper".into()],
            year: 2023,
            publisher: Some("J. AI".into()),
            url: None,
        };
        assert_eq!(
            render_reference(&r, CslStyle::Ieee),
            "G. Hopper, \"On Retrieval,\" J. AI, 2023."
        );
    }

    #[test]
    fn webpage_renders_with_url() {
        let r = Reference {
            id: "3".into(),
            kind: ReferenceKind::Webpage,
            title: "FSRS Wiki".into(),
            authors: vec![],
            year: 2026,
            publisher: None,
            url: Some("https://example.com".into()),
        };
        assert!(render_reference(&r, CslStyle::Apa).contains("https://example.com"));
    }

    #[test]
    fn in_text_citations_vary_by_style() {
        let b = book();
        assert_eq!(render_citation(&b, CslStyle::Apa), "(Lovelace, A., 2024)");
        assert_eq!(render_citation(&b, CslStyle::Chicago), "Lovelace, A. 2024");
        assert_eq!(render_citation(&b, CslStyle::Ieee), "[1]");
    }

    #[test]
    fn two_authors_joined_with_ampersand() {
        let r = Reference {
            authors: vec!["Ada Lovelace".into(), "Grace Hopper".into()],
            ..book()
        };
        assert!(render_reference(&r, CslStyle::Apa).contains("Lovelace, A. & Hopper, G."));
    }

    #[test]
    fn library_insert_get_and_idempotence() {
        let mut lib = ReferenceLibrary::new();
        lib.insert(book());
        assert_eq!(lib.len(), 1);
        assert_eq!(lib.get("1").unwrap().title, "Designing Agents");
        // Re-inserting the same id replaces, not duplicates.
        lib.insert(book());
        assert_eq!(lib.len(), 1);
    }

    #[test]
    fn library_search_ranks_title_and_author_matches() {
        let mut lib = ReferenceLibrary::new();
        lib.insert(book());
        lib.insert(Reference {
            id: "2".into(),
            kind: ReferenceKind::Article,
            title: "On Retrieval Systems".into(),
            authors: vec!["Ada Lovelace".into()],
            year: 2023,
            publisher: None,
            url: None,
        });
        let hits = lib.search("agents");
        assert_eq!(hits[0].id, "1"); // title match
        let by_author = lib.search("lovelace");
        assert_eq!(by_author.len(), 2);
        let none = lib.search("zzz");
        assert!(none.is_empty());
    }

    #[test]
    fn library_serializes() {
        let mut lib = ReferenceLibrary::new();
        lib.insert(book());
        let json = serde_json::to_string(&lib).unwrap();
        let back: ReferenceLibrary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.get("1").unwrap().year, 2024);
    }

    #[test]
    fn insert_citation_appends_paragraph_before_sectpr() {
        let out =
            insert_citation_into_docx(&crate::zip::tests::sample_docx(), "(Lovelace, A., 2024)")
                .unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out).unwrap();
        let xml = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        // The citation paragraph lands before the sectPr, body intact.
        let cite_pos = xml.find("(Lovelace, A., 2024)").unwrap();
        let sect_pos = xml.find("</w:sectPr>").unwrap();
        assert!(cite_pos < sect_pos, "citation must precede sectPr");
        assert!(xml.contains("Hello, ")); // untouched content preserved
    }

    #[test]
    fn insert_citation_escapes_text() {
        let out =
            insert_citation_into_docx(&crate::zip::tests::sample_docx(), "A & B <C>").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out).unwrap();
        let xml = String::from_utf8(a.read_part("word/document.xml").unwrap()).unwrap();
        assert!(xml.contains("A &amp; B &lt;C&gt;"));
    }
}
