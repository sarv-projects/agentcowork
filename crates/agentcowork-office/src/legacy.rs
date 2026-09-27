//! D8 — legacy binary Office formats (`.doc`/`.xls`/`.ppt`, doc 29 §3a).
//!
//! These are proprietary binary formats with no clean Rust reader. The honest
//! boundary (ARCH/04 §4.2): **convert to modern OOXML on open** via headless
//! LibreOffice, and surface them as **read-only with "edit as new"** — edits
//! always produce modern `.docx`/`.xlsx`/`.pptx`, never the binary original.

use std::path::Path;
use std::process::Command;

use super::conformance::find_soffice;

/// A legacy binary Office format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LegacyKind {
    Doc,
    Xls,
    Ppt,
}

impl LegacyKind {
    /// Detect a legacy binary format from a file path's extension.
    pub fn from_path(path: &Path) -> Option<LegacyKind> {
        match path
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase()
            .as_str()
        {
            "doc" => Some(LegacyKind::Doc),
            "xls" => Some(LegacyKind::Xls),
            "ppt" => Some(LegacyKind::Ppt),
            _ => None,
        }
    }

    /// The `soffice --convert-to` filter + the modern extension it produces.
    pub fn target_format(self) -> (&'static str, &'static str) {
        match self {
            LegacyKind::Doc => ("docx", "docx"),
            LegacyKind::Xls => ("xlsx", "xlsx"),
            LegacyKind::Ppt => ("pptx", "pptx"),
        }
    }
}

/// How a legacy file may be opened. Legacy binaries are always **read-only**;
/// the "edit as new" path converts to modern OOXML first, then edits that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyOpen {
    pub kind: LegacyKind,
    /// Binary legacy formats can't be surgically edited in place.
    pub read_only: bool,
    /// Convert to modern OOXML and edit the converted file instead.
    pub edit_as_new: bool,
}

impl LegacyOpen {
    /// The default surface for a legacy path: read-only, with edit-as-new
    /// offered as the conversion path.
    pub fn for_path(path: &Path) -> Option<Self> {
        LegacyKind::from_path(path).map(|kind| LegacyOpen {
            kind,
            read_only: true,
            edit_as_new: false,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LegacyError {
    #[error("not a legacy binary Office format (.doc/.xls/.ppt)")]
    NotLegacy,
    #[error(
        "LibreOffice (soffice) is not installed or not on PATH — legacy conversion unavailable"
    )]
    NoSoffice,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("LibreOffice conversion failed: {0}")]
    ConversionFailed(String),
}

/// Convert a legacy file to its modern OOXML equivalent via headless
/// LibreOffice. Returns `(converted_file_name, bytes)`.
pub fn convert_to_modern(path: &Path) -> Result<(String, Vec<u8>), LegacyError> {
    let kind = LegacyKind::from_path(path).ok_or(LegacyError::NotLegacy)?;
    let (filter, target_ext) = kind.target_format();
    let soffice = find_soffice().ok_or(LegacyError::NoSoffice)?;

    let outdir = std::env::temp_dir().join(format!(
        "agentcowork-convert-{}-{}",
        std::process::id(),
        super::conformance::seq()
    ));
    std::fs::create_dir_all(&outdir)?;

    let output = Command::new(&soffice)
        .args(["--headless", "--convert-to", filter, "--outdir"])
        .arg(&outdir)
        .arg(path)
        .output()?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&outdir);
        return Err(LegacyError::ConversionFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    // The converted file keeps the stem, with the modern extension.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    let name = format!("{stem}.{target_ext}");
    let bytes = std::fs::read(outdir.join(&name))?;
    let _ = std::fs::remove_dir_all(&outdir);
    Ok((name, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_legacy_extensions() {
        assert_eq!(
            LegacyKind::from_path(Path::new("a.doc")),
            Some(LegacyKind::Doc)
        );
        assert_eq!(
            LegacyKind::from_path(Path::new("a.XLS")),
            Some(LegacyKind::Xls)
        );
        assert_eq!(
            LegacyKind::from_path(Path::new("a.ppt")),
            Some(LegacyKind::Ppt)
        );
        assert_eq!(LegacyKind::from_path(Path::new("a.docx")), None);
        assert_eq!(LegacyKind::from_path(Path::new("a.pdf")), None);
    }

    #[test]
    fn target_format_maps_to_modern_extension() {
        assert_eq!(LegacyKind::Doc.target_format(), ("docx", "docx"));
        assert_eq!(LegacyKind::Xls.target_format(), ("xlsx", "xlsx"));
        assert_eq!(LegacyKind::Ppt.target_format(), ("pptx", "pptx"));
    }

    #[test]
    fn legacy_open_is_read_only_by_default() {
        let open = LegacyOpen::for_path(Path::new("report.doc")).unwrap();
        assert_eq!(open.kind, LegacyKind::Doc);
        assert!(open.read_only);
        assert!(!open.edit_as_new);
    }

    #[test]
    fn non_legacy_is_none() {
        assert!(LegacyOpen::for_path(Path::new("report.docx")).is_none());
    }

    #[test]
    fn convert_requires_legacy_extension() {
        let path = Path::new("report.docx");
        let err = convert_to_modern(path).unwrap_err();
        assert!(matches!(err, LegacyError::NotLegacy));
    }
}
