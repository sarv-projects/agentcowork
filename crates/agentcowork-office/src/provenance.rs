//! P36 (D5) — `DocumentAsset` provenance. Ingest ≠ mutate ≠ render: a
//! document carries its origin, its conversion chain, its content hashes,
//! and its version history so every later surface (patch, recalc, export)
//! can answer "where did this come from and what changed?".
//!
//! Pure data + helpers; the office engines attach one of these when they
//! open a document.

use crate::legacy::LegacyKind;
use serde::{Deserialize, Serialize};

/// How the asset came to be a modern OOXML document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Converter {
    /// Opened natively.
    None,
    /// Converted from a legacy format (doc 29 §3a: .doc/.xls/.ppt → modern).
    Legacy(LegacyKind),
    /// Re-authored structurally (P4.4 author path).
    Authored,
    /// Created fresh (D1/D3 author-new paths).
    Created,
}

/// One mutation of the document (surgical edit / recalc / form-fill).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetVersion {
    pub version: u64,
    pub at_ms: u64,
    pub by: String,
    pub what: String,
    /// Hash of the bytes *after* this version was applied.
    pub extracted_hash: String,
}

/// The canonical provenance record. **Ingest ≠ mutate ≠ render**:
/// `record_mutation` advances the extracted hash; rendering never does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentAsset {
    pub source_uri: Option<String>,
    pub media_type: String,
    pub converter: Converter,
    /// Content hash at ingest (pre-conversion where applicable).
    pub source_hash: Option<String>,
    /// Content hash of the current extracted content (post-edits).
    pub extracted_hash: String,
    /// Version history (append-only, newest last).
    pub versions: Vec<AssetVersion>,
}

impl DocumentAsset {
    pub fn new(
        media_type: impl Into<String>,
        source_hash: Option<String>,
        extracted_hash: impl Into<String>,
    ) -> Self {
        Self {
            source_uri: None,
            media_type: media_type.into(),
            converter: Converter::None,
            source_hash,
            extracted_hash: extracted_hash.into(),
            versions: Vec::new(),
        }
    }

    pub fn with_source(mut self, uri: impl Into<String>) -> Self {
        self.source_uri = Some(uri.into());
        self
    }

    pub fn with_converter(mut self, c: Converter) -> Self {
        self.converter = c;
        self
    }

    /// Record one mutate step. Mutate advances the extracted hash; render
    /// never does (the honest boundary).
    pub fn record_mutation(
        &mut self,
        version: u64,
        at_ms: u64,
        by: &str,
        what: &str,
        extracted_hash: &str,
    ) {
        self.versions.push(AssetVersion {
            version,
            at_ms,
            by: by.to_string(),
            what: what.to_string(),
            extracted_hash: extracted_hash.to_string(),
        });
        self.extracted_hash = extracted_hash.to_string();
    }

    /// Render never mutates: returns the current extracted hash without
    /// recording a version.
    pub fn hash_for_render(&self) -> &str {
        &self.extracted_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_mutate_render_distinct() {
        let mut a = DocumentAsset::new(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Some("src-hash".to_string()),
            "h1",
        );
        a.record_mutation(1, 100, "agent-x", "patch paragraph 3", "h2");
        assert_eq!(a.extracted_hash, "h2");
        assert_eq!(a.versions.len(), 1);
        // Rendering doesn't advance versions.
        let _ = a.hash_for_render();
        assert_eq!(a.versions.len(), 1);
        // Mutate again.
        a.record_mutation(2, 200, "agent-x", "patch paragraph 7", "h3");
        assert_eq!(a.versions.len(), 2);
        assert_eq!(a.extracted_hash, "h3");
    }

    #[test]
    fn legacy_converter_recorded() {
        let a = DocumentAsset::new("text/markdown", None, "h")
            .with_converter(Converter::Legacy(LegacyKind::Doc));
        assert!(matches!(a.converter, Converter::Legacy(LegacyKind::Doc)));
    }

    #[test]
    fn version_history_append_only() {
        let mut a = DocumentAsset::new("text/markdown", None, "v0");
        a.record_mutation(1, 10, "user", "edit 1", "v1");
        a.record_mutation(2, 20, "agent", "edit 2", "v2");
        assert_eq!(a.versions[0].what, "edit 1");
        assert_eq!(a.versions.last().unwrap().extracted_hash, "v2");
    }
}
