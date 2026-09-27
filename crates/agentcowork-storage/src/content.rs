//! P18-2 — Full-text *content* search + on-device OCR (doc 70 §2 — `dowse`
//! 🟡 ADAPT). The `SearchIndex` (G7) indexes filenames only; this module adds
//! the content half: an FTS5 table over extracted text, a zero-dep text
//! extractor, and an `OcrEngine` seam for pasted screenshots/images.
//!
//! Honest boundaries: extraction here covers plain text / markdown / HTML.
//! Office formats (docx/xlsx/pptx/pdf) are owned by `agentcowork-office` (the
//! calamine/lopdf engines) and feed this index through the same `insert`
//! path. OCR is a **trait + runtime seam**: the `TesseractCli` engine shells
//! out to a system `tesseract` binary when present and reports
//! `OcrError::Unavailable` otherwise — no OCR engine is bundled, nothing is
//! claimed that is not installed.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::StorageError;

/// One content-search hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContentHit {
    pub path: String,
    pub content: String,
}

/// SQLite FTS5 index over extracted file content (the filename half stays in
/// `SearchIndex`; this is the body half).
pub struct ContentIndex {
    conn: Connection,
}

impl ContentIndex {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StorageError> {
        // P45.1–.3 — WAL + synchronous=NORMAL + bounded WAL + mmap on the
        // read-heavy FTS5 content index. Vault never goes through here.
        crate::pragmas::apply_read_heavy_index(&conn)?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS contents \
             USING fts5(path UNINDEXED, content, tokenize='unicode61');",
        )?;
        Ok(Self { conn })
    }

    /// Index a file's extracted text.
    pub fn insert(&mut self, path: &str, content: &str) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO contents(path, content) VALUES (?1, ?2)",
            params![path, content],
        )?;
        Ok(())
    }

    pub fn insert_batch(
        &mut self,
        entries: impl Iterator<Item = (String, String)>,
    ) -> Result<usize, StorageError> {
        let tx = self.conn.transaction()?;
        let mut n = 0usize;
        {
            let mut stmt = tx.prepare("INSERT INTO contents(path, content) VALUES (?1, ?2)")?;
            for (path, content) in entries {
                stmt.execute(params![path, content])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn remove(&mut self, path: &str) -> Result<(), StorageError> {
        self.conn
            .execute("DELETE FROM contents WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// Full-text content query (prefix + substring via FTS5). Snippets are
    /// the raw FTS5 match highlighting.
    pub fn query(&self, term: &str, limit: usize) -> Result<Vec<ContentHit>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT path, snippet(contents, 1, '[', ']', '…', 24) \
             FROM contents WHERE contents MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![term, limit as i64], |row| {
            Ok(ContentHit {
                path: row.get(0)?,
                content: row.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT count(*) FROM contents", [], |r| r.get(0))
            .unwrap_or(0)
    }
}

/// Zero-dep text extraction from raw file bytes: plain text / markdown /
/// HTML. Returns `None` for binary-ish content (the caller decides whether
/// to route to OCR or to the office engines).
pub fn extract_text(path: &Path, bytes: &[u8]) -> Option<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "txt" | "md" | "markdown" | "log" | "csv" | "json" | "toml" | "yaml" | "yml" => {
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
        "html" | "htm" => Some(strip_html(bytes)),
        _ => None,
    }
}

/// Dependency-free HTML → text (drops script/style content, strips tags,
/// decodes the common entities).
pub fn strip_html(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut skip_depth = 0usize;
    let mut i = 0usize;
    let b = raw.as_bytes();
    while i < b.len() {
        match b[i] {
            b'<' => {
                let lower = raw[i..].to_ascii_lowercase();
                if lower.starts_with("<script") || lower.starts_with("<style") {
                    skip_depth += 1;
                } else if lower.starts_with("</script") || lower.starts_with("</style") {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if skip_depth == 0 {
                    in_tag = true;
                }
            }
            b'>' => in_tag = false,
            _ => {
                if !in_tag && skip_depth == 0 {
                    out.push(b[i] as char);
                }
            }
        }
        i += 1;
    }
    decode_entities(&out)
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// OCR failure modes — honest: `Unavailable` means no engine is installed.
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("no OCR engine available (tesseract not found on PATH)")]
    Unavailable,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tesseract failed: {0}")]
    Failed(String),
}

/// The OCR seam. Real engines (system `tesseract`, onnx models later) plug
/// in here; the index only ever sees text.
pub trait OcrEngine {
    /// Extract text from an image (png/jpeg/...) byte blob.
    fn ocr(&self, image: &[u8]) -> Result<String, OcrError>;
}

/// The honest default: no bundled OCR, never claims a result.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOcr;

impl OcrEngine for NoOcr {
    fn ocr(&self, _image: &[u8]) -> Result<String, OcrError> {
        Err(OcrError::Unavailable)
    }
}

/// System `tesseract` binary wrapper — the documented runtime seam. The
/// binary is located once at construction; execution is a plain subprocess.
#[derive(Debug, Clone)]
pub struct TesseractCli {
    bin: PathBuf,
    available: bool,
}

impl TesseractCli {
    /// Locate `tesseract` on PATH. `available()` reports the truth.
    pub fn detect() -> Self {
        let found = std::process::Command::new("tesseract")
            .arg("--version")
            .output()
            .is_ok();
        Self {
            bin: PathBuf::from("tesseract"),
            available: found,
        }
    }

    pub fn available(&self) -> bool {
        self.available
    }
}

impl OcrEngine for TesseractCli {
    fn ocr(&self, image: &[u8]) -> Result<String, OcrError> {
        if !self.available {
            return Err(OcrError::Unavailable);
        }
        // Temp-file round-trip: tesseract reads paths, not stdin blobs.
        let mut tmp = std::env::temp_dir();
        let name = format!("agentcowork-ocr-{}.png", std::process::id());
        tmp.push(name);
        std::fs::write(&tmp, image)?;
        let out = std::process::Command::new(&self.bin)
            .arg(tmp.to_str().unwrap_or(""))
            .arg("stdout")
            .output()?;
        let _ = std::fs::remove_file(&tmp);
        if !out.status.success() {
            return Err(OcrError::Failed(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bytes(md: &str) -> (PathBuf, Vec<u8>) {
        (PathBuf::from("notes.md"), md.as_bytes().to_vec())
    }

    #[test]
    fn indexes_and_queries_content() {
        let mut idx = ContentIndex::open_in_memory().unwrap();
        idx.insert("a.md", "the quick brown fox").unwrap();
        idx.insert("b.md", "jumps over the lazy dog").unwrap();
        assert_eq!(idx.count(), 2);
        let hits = idx.query("quick", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "a.md");
    }

    #[test]
    fn removes_by_path() {
        let mut idx = ContentIndex::open_in_memory().unwrap();
        idx.insert("a.md", "needle in haystack").unwrap();
        idx.remove("a.md").unwrap();
        assert_eq!(idx.query("needle", 10).unwrap().len(), 0);
    }

    #[test]
    fn extractor_handles_markdown_and_html() {
        let (path, bytes) = sample_bytes("# Title\n\nSome **bold** [link](x) text");
        let out = extract_text(&path, &bytes).unwrap();
        assert!(out.contains("Title"));
        assert!(out.contains("text"));

        let html = b"<html><head><style>p{color:red}</style></head><body><script>alert(1)</script><p>Hello &amp; bye</p></body></html>";
        let out = strip_html(html);
        assert!(out.contains("Hello & bye"));
        assert!(!out.contains("alert"));
        assert!(!out.contains("color"));
    }

    #[test]
    fn extractor_returns_none_for_binary() {
        let (path, bytes) = (PathBuf::from("img.png"), vec![0u8, 137, 80, 78, 71]);
        assert!(extract_text(&path, &bytes).is_none());
    }

    #[test]
    fn no_ocr_is_honest() {
        let no = NoOcr;
        assert!(matches!(no.ocr(&[]), Err(OcrError::Unavailable)));
    }

    #[test]
    fn tesseract_detect_reports_truthfully() {
        // never assumes: either the binary is on PATH or it is not
        let cli = TesseractCli::detect();
        let avail = std::process::Command::new("tesseract")
            .arg("--version")
            .output()
            .is_ok();
        assert_eq!(cli.available(), avail);
    }
}
