//! OOXML container layer (P4.1 item 1 + 5).
//!
//! A `.docx/.xlsx/.pptx` is a ZIP of XML parts + media. The core promise of
//! surgical editing (ARCH/04 §4.1) is: **rewrite only the parts that
//! changed; every other entry is copied byte-for-byte** — original
//! compression, local headers, macros, custom properties and unknown
//! namespaces all survive untouched.
//!
//! `zip`'s `raw_copy_file` copies an entry's raw (still-compressed) bytes
//! verbatim from the source archive, which is exactly the byte-stability
//! guarantee the conformance oracle checks (P4.5: zip-level diff of
//! untouched parts).

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use zip::read::ZipFile;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("part not found: {0}")]
    PartNotFound(String),
}

/// An open OOXML archive: the raw bytes (for verbatim entry copying) + the
/// parsed entry index.
pub struct OoxmlArchive {
    raw: Vec<u8>,
    archive: ZipArchive<Cursor<Vec<u8>>>,
}

impl OoxmlArchive {
    /// Open from bytes (the whole file, as read from disk).
    pub fn open(bytes: Vec<u8>) -> Result<Self, ArchiveError> {
        let archive = ZipArchive::new(Cursor::new(bytes.clone()))?;
        Ok(Self {
            raw: bytes,
            archive,
        })
    }

    /// Open from a path.
    pub fn open_path(path: &Path) -> Result<Self, ArchiveError> {
        Self::open(std::fs::read(path)?)
    }

    /// All part names, in archive order (the order the writer wrote them).
    pub fn parts(&mut self) -> Result<Vec<String>, ArchiveError> {
        let mut names = Vec::with_capacity(self.archive.len());
        for i in 0..self.archive.len() {
            let file = self.archive.by_index(i)?;
            names.push(file.name().to_string());
        }
        Ok(names)
    }

    /// Number of entries in the container (read from the central directory,
    /// without decompressing anything).
    pub fn entry_count(&mut self) -> Result<usize, ArchiveError> {
        Ok(self.archive.len())
    }

    /// The *uncompressed* size of one entry, without decompressing it. The
    /// bounded-memory policy (see [`crate::limits`]) checks this before a
    /// part is read, so an oversized part is refused instead of allocated.
    pub fn entry_size(&mut self, name: &str) -> Result<u64, ArchiveError> {
        Ok(self.by_name(name)?.size())
    }

    /// Decompressed bytes of one part.
    pub fn read_part(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        let mut file = self.by_name(name)?;
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// The raw compressed bytes of one part's entry — the exact payload the
    /// conformance oracle (P4.5) compares for untouched parts: byte-identical
    /// compressed data proves the entry was never recompressed.
    pub fn raw_entry(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        let (start, len) = {
            let file = self.by_name(name)?;
            (file.data_start() as usize, file.compressed_size() as usize)
        };
        let end = start + len;
        if end > self.raw.len() || start > end {
            return Err(ArchiveError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "corrupt entry bounds",
            )));
        }
        Ok(self.raw[start..end].to_vec())
    }

    fn by_name(&mut self, name: &str) -> Result<ZipFile<'_>, ArchiveError> {
        self.archive.by_name(name).map_err(|e| match e {
            zip::result::ZipError::FileNotFound => ArchiveError::PartNotFound(name.to_string()),
            other => ArchiveError::Zip(other),
        })
    }

    /// Rewrite the archive: `modified` parts are re-deflated fresh, every
    /// other entry is copied verbatim (raw bytes, original compression).
    pub fn save(&mut self, modified: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ArchiveError> {
        self.save_changes(modified, &[], &[])
    }

    /// Rewrite the archive with structural changes (P4.3 slide add/remove):
    /// - `modified` — existing parts rewritten (re-deflated fresh);
    /// - `added` — brand-new parts appended (e.g. a cloned slide);
    /// - `deleted` — existing parts omitted from the output.
    ///
    /// Every other entry is copied verbatim (raw bytes, original compression).
    pub fn save_changes(
        &mut self,
        modified: &[(String, Vec<u8>)],
        added: &[(String, Vec<u8>)],
        deleted: &[String],
    ) -> Result<Vec<u8>, ArchiveError> {
        let names = self.parts()?;
        let modified: std::collections::HashMap<&str, &[u8]> = modified
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let added: std::collections::HashMap<&str, &[u8]> = added
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let deleted: HashSet<&str> = deleted.iter().map(|s| s.as_str()).collect();

        let mut out = Vec::new();
        {
            let mut writer = ZipWriter::new(Cursor::new(&mut out));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for name in &names {
                if deleted.contains(name.as_str()) {
                    continue;
                }
                if let Some(new_bytes) = modified.get(name.as_str()) {
                    writer.start_file(name.clone(), opts)?;
                    writer.write_all(new_bytes)?;
                } else {
                    // Verbatim copy of the entry (header + compressed data).
                    let handle = self.by_name(name)?;
                    writer.raw_copy_file(handle)?;
                }
            }
            for (name, bytes) in &added {
                writer.start_file((*name).to_string(), opts)?;
                writer.write_all(bytes)?;
            }
            writer.finish()?;
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal docx-shaped archive: [Content_Types].xml + one body part.
    pub(crate) fn sample_docx() -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        w.start_file("[Content_Types].xml", opts).unwrap();
        w.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#,
        )
        .unwrap();
        w.start_file("_rels/.rels", opts).unwrap();
        w.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
        )
        .unwrap();
        w.start_file("word/document.xml", opts).unwrap();
        w.write_all(DOCUMENT_XML).unwrap();
        w.finish().unwrap().into_inner()
    }

    /// The body part used by the sample + most docx tests. One paragraph
    /// with two runs, a paragraph with a line break, a table cell paragraph.
    pub(crate) const DOCUMENT_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Hello, </w:t></w:r><w:r><w:t>world!</w:t></w:r></w:p>
    <w:p><w:r><w:t>Line one</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>line two</w:t></w:r></w:p>
    <w:tbl>
      <w:tr><w:tc><w:p><w:r><w:t>cell A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>cell B1</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
    <w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>
  </w:body>
</w:document>"#;

    #[test]
    fn parts_index_lists_entries() {
        let mut a = OoxmlArchive::open(sample_docx()).unwrap();
        let parts = a.parts().unwrap();
        assert_eq!(parts.len(), 3);
        assert!(parts.contains(&"word/document.xml".to_string()));
        assert!(parts.contains(&"[Content_Types].xml".to_string()));
    }

    #[test]
    fn read_part_returns_decompressed_bytes() {
        let mut a = OoxmlArchive::open(sample_docx()).unwrap();
        let xml = a.read_part("word/document.xml").unwrap();
        let s = String::from_utf8(xml).unwrap();
        assert!(s.contains("Hello, "));
        assert!(s.contains("world!"));
    }

    #[test]
    fn missing_part_errors() {
        let mut a = OoxmlArchive::open(sample_docx()).unwrap();
        match a.read_part("word/missing.xml") {
            Err(ArchiveError::PartNotFound(n)) => assert_eq!(n, "word/missing.xml"),
            other => panic!("expected PartNotFound, got {other:?}"),
        }
    }

    #[test]
    fn save_rewrites_only_modified_parts() {
        let original = sample_docx();
        let mut a = OoxmlArchive::open(original.clone()).unwrap();

        // Modify the document part (an actual P4.1 patch — replace world!).
        let doc = a.read_part("word/document.xml").unwrap();
        let patched = crate::docx::patch_first_paragraph(&doc, "Hello, universe!").unwrap();

        let out = a
            .save(&[("word/document.xml".to_string(), patched)])
            .unwrap();

        // Reopen the output: untouched parts must decompress to identical bytes.
        let mut b = OoxmlArchive::open(out).unwrap();
        let ct = b.read_part("[Content_Types].xml").unwrap();
        let mut a2 = OoxmlArchive::open(original).unwrap();
        let ct_orig = a2.read_part("[Content_Types].xml").unwrap();
        assert_eq!(ct, ct_orig, "[Content_Types].xml must be byte-identical");

        let new_doc = b.read_part("word/document.xml").unwrap();
        let s = String::from_utf8(new_doc).unwrap();
        // Run 1 untouched verbatim; run 2 carries the new text.
        assert!(s.contains("<w:t>Hello, </w:t>"));
        assert!(s.contains("<w:t>universe!</w:t>"));
        assert!(!s.contains("<w:t>world!</w:t>"));
    }

    #[test]
    fn raw_copy_preserves_compression_of_untouched_entries() {
        // The untouched entries must keep their original compressed bytes:
        // decompress-then-recompress would produce different bytes, breaking
        // the byte-stability guarantee. Compare the OUTPUT's raw entry bytes
        // for the untouched part against the INPUT's raw entry bytes.
        let original = sample_docx();
        let mut a = OoxmlArchive::open(original.clone()).unwrap();
        let doc = a.read_part("word/document.xml").unwrap();
        let patched = crate::docx::patch_first_paragraph(&doc, "Hello, universe!").unwrap();
        let out = a
            .save(&[("word/document.xml".to_string(), patched)])
            .unwrap();

        let mut in_a = OoxmlArchive::open(original).unwrap();
        let mut out_a = OoxmlArchive::open(out.clone()).unwrap();

        // Raw entry bytes are internal; compare via the archive re-read:
        // entry compressed sizes must match and content must match.
        let in_raw = in_a.raw_entry("[Content_Types].xml").unwrap();
        let out_raw = out_a.raw_entry("[Content_Types].xml").unwrap();
        assert_eq!(in_raw, out_raw, "untouched entry bytes must be verbatim");
    }

    #[test]
    fn save_with_no_modifications_is_identity() {
        let original = sample_docx();
        let mut a = OoxmlArchive::open(original.clone()).unwrap();
        let out = a.save(&[]).unwrap();
        // With zero modifications the container bytes are identical.
        assert_eq!(out, original);
    }
}
