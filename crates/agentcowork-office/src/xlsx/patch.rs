//! Surgical xlsx part-patch (P4.2 item 5): edits land in
//! `xl/worksheets/sheetN.xml` + `xl/workbook.xml` (rename) as targeted byte
//! replacements; `xl/sharedStrings.xml` gets a tested append helper for
//! bulk text imports. Untouched parts are copied verbatim by `OoxmlArchive`
//! (byte-stability, ARCH/04).
//!
//! Math integrity: formula cells are written with a `<v>0</v>` placeholder,
//! the patched workbook is recalculated by IronCalc, and the placeholder is
//! then replaced with the **engine-computed** value — the LLM never supplies
//! a number.

use std::collections::HashMap;

use roxmltree::{Document, Node};
use thiserror::Error;

use crate::xml::{escape_text, parse};

use super::address::{CellRef, RangeRef, format_ref};
use super::dsl::{
    FillMode, Operation, PivotRow, Scalar, ShiftKind, WorkbookCommandBatch, pivot_result,
};
use super::recalc::{RecalcResult, recalc};

pub const SPREADSHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Debug, Error)]
pub enum PatchError {
    #[error("xml: {0}")]
    Xml(#[from] crate::xml::OfficeXmlError),
    #[error("part not found: {0}")]
    PartNotFound(String),
    #[error("sheet not found: {0}")]
    SheetNotFound(String),
    #[error("recalc: {0}")]
    Recalc(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive: {0}")]
    Archive(#[from] crate::zip::ArchiveError),
    /// A size ceiling the engine will not attempt to load past (ARCH/04 §4.6).
    #[error(
        "{subject} is {actual} bytes, above the {kind} ceiling of {limit} bytes (refused: the engine will not attempt a load it cannot bound)"
    )]
    TooLarge {
        kind: crate::limits::LimitKind,
        subject: String,
        actual: u64,
        limit: u64,
    },
    /// Entry-count ceiling.
    #[error(
        "package has {count} entries, above the entry-count ceiling of {limit} (refused: the engine will not attempt a load it cannot bound)"
    )]
    TooManyParts { count: usize, limit: usize },
}

impl From<crate::docx::OfficeError> for PatchError {
    fn from(e: crate::docx::OfficeError) -> Self {
        match e {
            crate::docx::OfficeError::TooLarge {
                kind,
                subject,
                actual,
                limit,
            } => PatchError::TooLarge {
                kind,
                subject,
                actual,
                limit,
            },
            crate::docx::OfficeError::TooManyParts { count, limit } => {
                PatchError::TooManyParts { count, limit }
            }
            crate::docx::OfficeError::Archive(a) => PatchError::Archive(a),
            crate::docx::OfficeError::Xml(x) => PatchError::Xml(x),
            other => PatchError::Recalc(other.to_string()),
        }
    }
}

/// Result of applying a batch: the patched archive bytes, the parts that
/// changed (for the audit), and an optional in-memory pivot summary.
#[derive(Debug)]
pub struct PatchOutcome {
    pub bytes: Vec<u8>,
    pub changed_parts: Vec<String>,
    pub pivot: Option<Vec<PivotRow>>,
}

/// Apply a command batch to an xlsx archive.
///
/// `sheet` names the target sheet (ops without a sheet target apply here).
/// Formula cells get IronCalc-computed values (see module docs).
///
/// Uses the process size policy (`PatchLimits::default`).
pub fn apply_batch(
    archive_bytes: &[u8],
    batch: &WorkbookCommandBatch,
    sheet: &str,
) -> Result<PatchOutcome, PatchError> {
    apply_batch_with_limits(
        archive_bytes,
        batch,
        sheet,
        crate::limits::PatchLimits::default(),
    )
}

/// [`apply_batch`] under an explicit size policy: the container, its entry
/// count, and every part the batch reads are checked against the ceilings
/// before they are decompressed.
pub fn apply_batch_with_limits(
    archive_bytes: &[u8],
    batch: &WorkbookCommandBatch,
    sheet: &str,
    limits: crate::limits::PatchLimits,
) -> Result<PatchOutcome, PatchError> {
    limits.check_archive(archive_bytes.len() as u64)?;
    let mut archive = crate::zip::OoxmlArchive::open(archive_bytes.to_vec()).map_err(|e| {
        PatchError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    limits.check_part_count(archive.entry_count()?)?;

    let workbook_xml = read_bounded(&mut archive, &limits, "xl/workbook.xml")?;
    let rels_xml = read_bounded(&mut archive, &limits, "xl/_rels/workbook.xml.rels")?;
    let sst = read_optional_bounded(&mut archive, &limits, "xl/sharedStrings.xml")?;

    let sheet_part = sheet_part_name(&workbook_xml, &rels_xml, sheet)
        .ok_or_else(|| PatchError::SheetNotFound(sheet.to_string()))?;
    let mut sheet_bytes = read_bounded(&mut archive, &limits, &sheet_part)?;

    // Cached values for copy-down fill: top cell + delta.
    let mut fill_seed: Option<(CellRef, Scalar, Option<f64>)> = None;
    let mut formula_writes: Vec<CellRef> = Vec::new();
    let mut pivot_out: Option<Vec<PivotRow>> = None;

    for op in &batch.operations {
        match op {
            Operation::SetCell { address, value } => {
                let cell_xml = cell_xml_for(*address, value, None);
                sheet_bytes = upsert_cell(&sheet_bytes, *address, &cell_xml)?;
            }
            Operation::SetFormula { address, formula } => {
                // Placeholder value; replaced with the IronCalc result after
                // the whole batch is applied + recalculated. `<f>` content
                // follows the Excel convention (no leading '=').
                let f = formula.trim_start_matches('=');
                let cell_xml = format!(
                    "<c r=\"{}\"><f>{}</f><v>0</v></c>",
                    format_addr(*address),
                    escape_text(f)
                );
                sheet_bytes = upsert_cell(&sheet_bytes, *address, &cell_xml)?;
                formula_writes.push(*address);
            }
            Operation::ClearRange { range } => {
                sheet_bytes = clear_range(&sheet_bytes, *range)?;
            }
            Operation::SortRange {
                range,
                by_col,
                desc,
            } => {
                sheet_bytes = sort_range(&sheet_bytes, *range, *by_col, *desc)?;
            }
            Operation::FillRange { range, mode, value } => match mode {
                FillMode::Constant => {
                    let v = value.clone().unwrap_or(Scalar::Text(String::new()));
                    sheet_bytes = fill_constant(&sheet_bytes, *range, &v)?;
                }
                FillMode::CopyDown => {
                    let seed = match &fill_seed {
                        Some(s) => s.clone(),
                        None => {
                            let s = copy_down_seed(&sheet_bytes, *range)?;
                            fill_seed = Some(s.clone());
                            s
                        }
                    };
                    let _ = seed;
                    sheet_bytes = fill_copy_down(&sheet_bytes, *range)?;
                }
            },
            Operation::Shift {
                sheet: sh,
                kind,
                at,
                count,
            } => {
                if sh == sheet {
                    // Formula refs first (they operate on formula text only),
                    // then the physical cell move (rows/cols/dimension/merges).
                    sheet_bytes = shift_formulas(&sheet_bytes, sh, *kind, *at, *count)?;
                    sheet_bytes = shift_structure(&sheet_bytes, *kind, *at, *count)?;
                }
            }
            Operation::RenameSheet { from, to } => {
                let mut wb = workbook_xml.clone();
                rename_sheet(&mut wb, from, to)?;
                // The rename is a workbook.xml-only edit; re-run the rest of
                // the batch through the normal path, then overlay the new
                // workbook part.
                return apply_after_rename(archive_bytes, batch, sheet, &wb, limits);
            }
            Operation::Pivot {
                source,
                group_by,
                aggregate,
                agg,
            } => {
                let grid = read_range_values(&sheet_bytes, sst.as_deref(), *source)?;
                pivot_out = Some(pivot_result(
                    &grid,
                    *group_by as usize,
                    *aggregate as usize,
                    *agg,
                ));
            }
        }
    }

    let changed: Vec<String> = vec![sheet_part.clone()];
    let mut modified: Vec<(String, Vec<u8>)> = vec![(sheet_part.clone(), sheet_bytes.clone())];

    // Recalc the patched workbook, then write engine-computed values into
    // the formula cells this batch wrote.
    let archive_with_placeholders = archive.save(&modified)?;
    let recalc_result = recalc(&archive_with_placeholders).map_err(|e| PatchError::Recalc(e.0))?;
    if !formula_writes.is_empty() {
        let computed = computed_values(&recalc_result, sheet);
        for addr in &formula_writes {
            if let Some(v) = computed.get(addr) {
                sheet_bytes = set_cell_value(&sheet_bytes, *addr, v)?;
            }
        }
        modified.clear();
        modified.push((sheet_part.clone(), sheet_bytes.clone()));
    }

    let bytes = archive.save(&modified)?;
    Ok(PatchOutcome {
        bytes,
        changed_parts: changed,
        pivot: pivot_out,
    })
}

fn apply_after_rename(
    archive_bytes: &[u8],
    batch: &WorkbookCommandBatch,
    sheet: &str,
    workbook_xml: &[u8],
    limits: crate::limits::PatchLimits,
) -> Result<PatchOutcome, PatchError> {
    // Rename is a workbook.xml-only edit: re-run the rest of the ops through
    // the normal path (rename already applied), then swap in the new
    // workbook part. To keep this focused we apply rename + other ops in one
    // pass: run apply_batch without the rename op, then overlay the rename.
    let mut filtered = batch.clone();
    filtered
        .operations
        .retain(|op| !matches!(op, Operation::RenameSheet { .. }));
    let mut outcome = apply_batch_with_limits(archive_bytes, &filtered, sheet, limits)?;
    let mut archive = crate::zip::OoxmlArchive::open(outcome.bytes.clone()).map_err(|e| {
        PatchError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    outcome.bytes = archive.save(&[("xl/workbook.xml".to_string(), workbook_xml.to_vec())])?;
    outcome.changed_parts.push("xl/workbook.xml".to_string());
    Ok(outcome)
}

/// Read one part, refusing a size the policy cannot bound before the
/// decompression allocates anything.
fn read_bounded(
    archive: &mut crate::zip::OoxmlArchive,
    limits: &crate::limits::PatchLimits,
    name: &str,
) -> Result<Vec<u8>, PatchError> {
    match archive.entry_size(name) {
        Ok(size) => {
            limits.check_part(name, size)?;
            Ok(archive.read_part(name)?)
        }
        Err(crate::zip::ArchiveError::PartNotFound(_)) => {
            Err(PatchError::PartNotFound(name.to_string()))
        }
        Err(e) => Err(PatchError::Archive(e)),
    }
}

/// [`read_bounded`], but a missing part is not an error.
fn read_optional_bounded(
    archive: &mut crate::zip::OoxmlArchive,
    limits: &crate::limits::PatchLimits,
    name: &str,
) -> Result<Option<Vec<u8>>, PatchError> {
    match archive.entry_size(name) {
        Ok(size) => {
            limits.check_part(name, size)?;
            Ok(Some(archive.read_part(name)?))
        }
        Err(crate::zip::ArchiveError::PartNotFound(_)) => Ok(None),
        Err(e) => Err(PatchError::Archive(e)),
    }
}

// ────────────────────────────────────────────────────────────────────────
// Sheet part discovery
// ────────────────────────────────────────────────────────────────────────

/// Map a sheet name → `xl/worksheets/sheetN.xml` via workbook.xml +
/// workbook.xml.rels.
pub fn sheet_part_name(workbook_xml: &[u8], rels_xml: &[u8], sheet: &str) -> Option<String> {
    let wb = parse(workbook_xml).ok()?;
    let mut rid = None;
    for node in wb
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "sheet")
    {
        if node.attribute("name") == Some(sheet) {
            rid = node
                .attribute((REL_NS, "id"))
                .or_else(|| node.attribute("id"))
                .map(|s| s.to_string());
            break;
        }
    }
    let rid = rid?;
    let rels = parse(rels_xml).ok()?;
    for node in rels
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
    {
        if node.attribute("Id") == Some(&rid) {
            if let Some(target) = node.attribute("Target") {
                let target = target.trim_start_matches('/');
                return Some(if target.starts_with("worksheets/") {
                    format!("xl/{target}")
                } else {
                    target.to_string()
                });
            }
        }
    }
    None
}

/// Rewrite the `<sheet name>` attribute in workbook.xml.
pub fn rename_sheet(workbook_xml: &mut Vec<u8>, from: &str, to: &str) -> Result<(), PatchError> {
    let doc = parse(workbook_xml)?;
    for node in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "sheet")
    {
        if node.attribute("name") == Some(from) {
            if let Some(attr) = node.attribute_node("name") {
                let range = attr.range_value();
                let new = escape_text(to);
                workbook_xml.splice(range, new.into_bytes());
                return Ok(());
            }
        }
    }
    Err(PatchError::SheetNotFound(from.to_string()))
}

// ────────────────────────────────────────────────────────────────────────
// Cell-level helpers
// ────────────────────────────────────────────────────────────────────────

fn format_addr(cell: CellRef) -> String {
    crate::xlsx::address::col_letter(cell.col)
        .map(|c| format!("{c}{}", cell.row))
        .unwrap_or_else(|| format!("R{}C{}", cell.row, cell.col))
}

/// Build the `<c r="A1" …>…</c>` XML for a scalar write.
fn cell_xml_for(address: CellRef, value: &Scalar, style: Option<&str>) -> String {
    let r = format_addr(address);
    let s = style.map(|s| format!(" s=\"{s}\"")).unwrap_or_default();
    match value {
        Scalar::Number(n) => format!("<c r=\"{r}\"{s}><v>{n}</v></c>"),
        Scalar::Text(t) => format!(
            "<c r=\"{r}\"{s} t=\"inlineStr\"><is><t>{}</t></is></c>",
            escape_text(t)
        ),
        Scalar::Bool(b) => {
            format!(
                "<c r=\"{r}\"{s} t=\"b\"><v>{}</v></c>",
                if *b { 1 } else { 0 }
            )
        }
    }
}

/// Find the existing cell element for a ref (returns the element node).
fn find_cell<'a, 'i>(doc: &'a Document<'i>, address: CellRef) -> Option<Node<'a, 'i>> {
    let want = format_addr(address);
    doc.descendants().find(|n| {
        n.is_element() && n.tag_name().name() == "c" && n.attribute("r") == Some(want.as_str())
    })
}

/// Replace or insert a cell element. Preserves an existing `s` (style)
/// attribute on replacement.
pub fn upsert_cell(
    sheet_bytes: &[u8],
    address: CellRef,
    new_xml: &str,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let mut out = sheet_bytes.to_vec();

    if let Some(cell) = find_cell(&doc, address) {
        // Preserve the style attribute from the old cell.
        let final_xml = if let Some(style) = cell.attribute("s") {
            if new_xml.contains(" s=\"") {
                new_xml.to_string()
            } else {
                // graft style into the new cell: new_xml starts with
                // <c r="A1"...> — insert s= right after the r attribute.
                let r = format_addr(address);
                let s = format!(" s=\"{}\"", style);
                let mut parts = new_xml.splitn(2, '>');
                let head = parts.next().unwrap_or("");
                let tail = parts.next().unwrap_or("");
                let mut head = head.to_string();
                if !head.contains(" s=") {
                    head = head.replacen(&format!("r=\"{r}\""), &format!("r=\"{r}\"{s}"), 1);
                }
                format!("{head}>{tail}")
            }
        } else {
            new_xml.to_string()
        };
        let range = cell.range();
        out.splice(range, final_xml.into_bytes());
        return Ok(out);
    }

    // New cell: insert into the right row, keeping cell order ascending.
    let row_num = address.row;
    let col = address.col;
    let row_node = doc.descendants().find(|n| {
        n.is_element()
            && n.tag_name().name() == "row"
            && n.attribute("r") == Some(row_num.to_string().as_str())
    });
    if let Some(row_node) = row_node {
        // insert before first cell with col > address.col (or before </row>)
        let mut insert_at: Option<usize> = None;
        for child in row_node.children() {
            if child.is_element() && child.tag_name().name() == "c" {
                if let Some(r) = child.attribute("r") {
                    if let Ok((_, cref)) = crate::xlsx::address::parse_ref(r) {
                        if cref.col > col {
                            insert_at = Some(child.range().start);
                            break;
                        }
                    }
                }
            }
        }
        match insert_at {
            Some(pos) => {
                out.splice(pos..pos, new_xml.as_bytes().to_vec());
            }
            None => {
                // before </row> (or expand a self-closing <row/>)
                if let Some(close) = closing_tag_at(text, row_node, "row") {
                    out.splice(close..close, new_xml.as_bytes().to_vec());
                } else {
                    let range = row_node.range();
                    let old = &text[range.clone()];
                    let expanded =
                        old.trim_end_matches("/>").to_string() + ">" + new_xml + "</row>";
                    out.splice(range, expanded.into_bytes());
                }
            }
        }
        return Ok(out);
    }

    // New row: insert before first row with r > address.row, else before
    // </sheetData> (or expand a self-closing <sheetData/>).
    let mut insert_at: Option<usize> = None;
    let mut sheet_data: Option<Node> = None;
    for node in doc.descendants() {
        if node.is_element() && node.tag_name().name() == "sheetData" {
            sheet_data = Some(node);
        }
    }
    let sheet_data = sheet_data.ok_or(PatchError::Xml(crate::xml::OfficeXmlError::Parse(
        roxmltree::Error::NoRootNode,
    )))?;
    let rows: Vec<Node> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "row")
        .collect();
    for rnode in rows {
        if let Some(r) = rnode.attribute("r") {
            if let Ok(n) = r.parse::<u32>() {
                if n > row_num {
                    insert_at = Some(rnode.range().start);
                    break;
                }
            }
        }
    }
    let row_xml = format!("<row r=\"{row_num}\">{new_xml}</row>");
    if let Some(p) = insert_at {
        out.splice(p..p, row_xml.into_bytes());
    } else if let Some(close) = closing_tag_at(text, sheet_data, "sheetData") {
        out.splice(close..close, row_xml.into_bytes());
    } else {
        let range = sheet_data.range();
        let old = &text[range.clone()];
        let expanded = old.trim_end_matches("/>").to_string() + ">" + &row_xml + "</sheetData>";
        out.splice(range, expanded.into_bytes());
    }
    Ok(out)
}

/// Byte offset of the start of an element's closing tag (`</name>`), or
/// `None` when the element is self-closing (`<row/>`).
fn closing_tag_at(text: &str, node: Node, name: &str) -> Option<usize> {
    let range = node.range();
    let inner = &text[range.clone()];
    let close = format!("</{name}>");
    if inner.ends_with(&close) {
        Some(range.end - close.len())
    } else {
        None
    }
}

/// Replace the `<v>` value of a cell with a new value (formula recalc write).
fn set_cell_value(
    sheet_bytes: &[u8],
    address: CellRef,
    value: &str,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let mut out = sheet_bytes.to_vec();
    let Some(cell) = find_cell(&doc, address) else {
        return Ok(out);
    };
    // Replace the whole <v>…</v> (or create one before </c>).
    if let Some(v) = cell
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "v")
    {
        let range = v.range();
        out.splice(range, format!("<v>{value}</v>").into_bytes());
    } else {
        let close = closing_tag_at(text, cell, "c").unwrap_or(cell.range().end);
        out.splice(close..close, format!("<v>{value}</v>").into_bytes());
    }
    Ok(out)
}

/// Extract computed values (formatted) for formula cells on a sheet, keyed
/// by cell ref.
fn computed_values(res: &RecalcResult, sheet: &str) -> HashMap<CellRef, String> {
    let mut map = HashMap::new();
    for s in &res.sheets {
        if s.name == sheet {
            for c in &s.cells {
                let display = c.value.display();
                map.insert(
                    CellRef {
                        row: c.row,
                        col: c.col,
                    },
                    display,
                );
            }
        }
    }
    map
}

// ────────────────────────────────────────────────────────────────────────
// Clear / sort / fill
// ────────────────────────────────────────────────────────────────────────

fn clear_range(sheet_bytes: &[u8], range: RangeRef) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let cells: Vec<Node> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "c")
        .filter(|n| {
            n.attribute("r")
                .and_then(|r| {
                    crate::xlsx::address::parse_ref(r).ok().map(|(_, c)| {
                        c.row >= range.start.row
                            && c.row <= range.end.row
                            && c.col >= range.start.col
                            && c.col <= range.end.col
                    })
                })
                .unwrap_or(false)
        })
        .collect();
    if cells.is_empty() {
        return Ok(sheet_bytes.to_vec());
    }
    // Splice out from the first cell start to the last cell end (they're in
    // document order), leaving the row elements intact.
    let first = cells.first().unwrap().range().start;
    let last = cells.last().unwrap().range().end;
    let mut out = sheet_bytes.to_vec();
    out.drain(first..last);
    Ok(out)
}

/// Parse `<v>`, `t="s"` (shared string), `t="inlineStr"`, `t="b"`, `t="e"`,
/// `t="str"` cell values → CellValue.
fn parse_cell_value(cell: Node, sst: Option<&Document>) -> super::read::CellValue {
    use super::read::CellValue;
    let t = cell.attribute("t").unwrap_or("");
    let v = cell
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "v")
        .map(|c| c.text().unwrap_or("").to_string());
    match t {
        "s" => {
            let idx: usize = v.and_then(|s| s.parse().ok()).unwrap_or(0);
            if let Some(sst) = sst {
                if let Some(si) = sst
                    .descendants()
                    .filter(|n| n.is_element() && n.tag_name().name() == "si")
                    .nth(idx)
                {
                    let mut text = String::new();
                    for tnode in si
                        .descendants()
                        .filter(|n| n.is_element() && n.tag_name().name() == "t")
                    {
                        text.push_str(tnode.text().unwrap_or(""));
                    }
                    return CellValue::Text(text);
                }
            }
            CellValue::Text(format!("[s{idx}]"))
        }
        "inlineStr" => {
            let text: String = cell
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "t")
                .map(|n| n.text().unwrap_or(""))
                .collect();
            CellValue::Text(text)
        }
        "b" => CellValue::Bool(v.as_deref() == Some("1") || v.as_deref() == Some("true")),
        "e" => CellValue::Error(v.unwrap_or_default()),
        "str" => CellValue::Text(v.unwrap_or_default()),
        _ => match v.and_then(|s| s.parse::<f64>().ok()) {
            Some(n) => CellValue::Number(n),
            None => CellValue::Empty,
        },
    }
}

/// Read the values of a rectangular range from the sheet part (0-based
/// row-major grid; used by sort seed + pivot).
fn read_range_values(
    sheet_bytes: &[u8],
    sst: Option<&[u8]>,
    range: RangeRef,
) -> Result<Vec<Vec<super::read::CellValue>>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let sst_doc = match sst {
        Some(b) => Some(
            Document::parse(
                std::str::from_utf8(b)
                    .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?,
            )
            .map_err(crate::xml::OfficeXmlError::Parse)?,
        ),
        None => None,
    };
    let rows = range.end.row - range.start.row + 1;
    let cols = range.end.col - range.start.col + 1;
    let mut grid = vec![vec![super::read::CellValue::Empty; cols as usize]; rows as usize];
    for cell in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "c")
    {
        if let Some(r) = cell.attribute("r") {
            if let Ok((_, cref)) = crate::xlsx::address::parse_ref(r) {
                if cref.row >= range.start.row
                    && cref.row <= range.end.row
                    && cref.col >= range.start.col
                    && cref.col <= range.end.col
                {
                    let (rr, cc) = (
                        (cref.row - range.start.row) as usize,
                        (cref.col - range.start.col) as usize,
                    );
                    grid[rr][cc] = parse_cell_value(cell, sst_doc.as_ref());
                }
            }
        }
    }
    Ok(grid)
}

/// Sort the rows whose `r` falls inside `range` by `by_col` (1-based column
/// within the sheet), Excel-style (numbers first, then text, empties last).
fn sort_range(
    sheet_bytes: &[u8],
    range: RangeRef,
    by_col: u32,
    desc: bool,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;

    // Collect rows fully inside the range, in document order.
    let mut rows: Vec<(u32, Node)> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "row")
        .filter_map(|n| {
            let r = n.attribute("r")?.parse::<u32>().ok()?;
            if r >= range.start.row && r <= range.end.row {
                Some((r, n))
            } else {
                None
            }
        })
        .collect();
    if rows.len() < 2 {
        return Ok(sheet_bytes.to_vec());
    }
    rows.sort_by_key(|(r, _)| *r);
    let first = rows.first().unwrap().1.range().start;
    let last = rows.last().unwrap().1.range().end;

    // Extract sort keys (value of the by_col cell in each row).
    let key = |node: Node| -> (u8, f64, String) {
        let want = format_addr(CellRef {
            row: node
                .attribute("r")
                .and_then(|r| r.parse().ok())
                .unwrap_or(0),
            col: by_col,
        });
        for cell in node
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "c")
        {
            if cell.attribute("r") == Some(want.as_str()) {
                match parse_cell_value(cell, None) {
                    super::read::CellValue::Number(n) => return (1, n, String::new()),
                    super::read::CellValue::Text(s) => return (2, 0.0, s.to_lowercase()),
                    super::read::CellValue::Bool(b) => {
                        return (1, if b { 1.0 } else { 0.0 }, String::new());
                    }
                    _ => return (3, 0.0, String::new()),
                }
            }
        }
        (3, 0.0, String::new())
    };

    let mut annotated: Vec<(u32, Node, (u8, f64, String))> =
        rows.into_iter().map(|(r, n)| (r, n, key(n))).collect();
    annotated.sort_by(|a, b| {
        let ka = &a.2;
        let kb = &b.2;
        ka.0.cmp(&kb.0)
            .then_with(|| ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| ka.2.cmp(&kb.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    if desc {
        annotated.reverse();
    }

    let new_region: String = annotated
        .iter()
        .map(|(_, n, _)| text[n.range()].to_string())
        .collect();

    let mut out = sheet_bytes.to_vec();
    out.splice(first..last, new_region.into_bytes());
    Ok(out)
}

/// Constant fill: write the scalar into every cell in the range (cells that
/// already exist get replaced; new cells are created).
fn fill_constant(
    sheet_bytes: &[u8],
    range: RangeRef,
    value: &Scalar,
) -> Result<Vec<u8>, PatchError> {
    let mut out = sheet_bytes.to_vec();
    for row in range.start.row..=range.end.row {
        for col in range.start.col..=range.end.col {
            let cell = CellRef { row, col };
            let xml = cell_xml_for(cell, value, None);
            out = upsert_cell(&out, cell, &xml)?;
        }
    }
    Ok(out)
}

/// Copy-down fill: read the top cell (and second cell for a numeric delta),
/// extend the numeric run down the column; non-numeric copies verbatim.
fn copy_down_seed(
    sheet_bytes: &[u8],
    range: RangeRef,
) -> Result<(CellRef, Scalar, Option<f64>), PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let read = |ref_: CellRef| -> Option<Scalar> {
        let want = format_addr(ref_);
        for cell in doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "c")
        {
            if cell.attribute("r") == Some(want.as_str()) {
                return match parse_cell_value(cell, None) {
                    super::read::CellValue::Number(n) => Some(Scalar::Number(n)),
                    super::read::CellValue::Text(s) => Some(Scalar::Text(s)),
                    super::read::CellValue::Bool(b) => Some(Scalar::Bool(b)),
                    _ => None,
                };
            }
        }
        None
    };
    let top = range.start;
    let value = read(top).unwrap_or(Scalar::Text(String::new()));
    let mut delta = None;
    if let Scalar::Number(n1) = value {
        if let Some(Scalar::Number(n2)) = read(CellRef {
            row: top.row + 1,
            col: top.col,
        }) {
            if top.row < range.end.row {
                delta = Some(n2 - n1);
            }
        }
    }
    Ok((top, value, delta))
}

fn fill_copy_down(sheet_bytes: &[u8], range: RangeRef) -> Result<Vec<u8>, PatchError> {
    let mut out = sheet_bytes.to_vec();
    let (top, value, delta) = copy_down_seed(&out, range)?;
    for row in (top.row + 1)..=range.end.row {
        let cell = CellRef { row, col: top.col };
        let v = match (&value, delta) {
            (Scalar::Number(n), Some(d)) => Scalar::Number(n + d * (row as f64 - top.row as f64)),
            (other, _) => other.clone(),
        };
        let xml = cell_xml_for(cell, &v, None);
        out = upsert_cell(&out, cell, &xml)?;
    }
    Ok(out)
}

/// Rewrite every formula in the sheet part with the Excel-accurate shift
/// (the structural-edit op keeps formulas correct; physical row/col moves
/// are the in-memory engine's job, P4.7).
fn shift_formulas(
    sheet_bytes: &[u8],
    target_sheet: &str,
    kind: super::dsl::ShiftKind,
    at: u32,
    count: u32,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let mut out = sheet_bytes.to_vec();
    // Collect formula text ranges first (borrow ends before mutation).
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for f in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "f")
    {
        if let Some(tnode) = f.children().find(|c| c.is_text()) {
            let range = tnode.range();
            let old = &text[range.clone()];
            let new = super::dsl::shift_formula(old, target_sheet, kind, at, count);
            if new != old {
                edits.push((range.start, range.end, new));
            }
        }
    }
    for (start, end, new) in edits.into_iter().rev() {
        out.splice(start..end, new.into_bytes());
    }
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────
// Structural row/column shift (physical move)
// ────────────────────────────────────────────────────────────────────────

/// Row number after a row shift; `None` = the row/cell is inside the deleted
/// band and must be removed.
fn row_after(n: u32, kind: ShiftKind, at: u32, count: u32) -> Option<u32> {
    match kind {
        ShiftKind::InsertRow => Some(if n >= at { n + count } else { n }),
        ShiftKind::DeleteRow => {
            if n < at {
                Some(n)
            } else if n < at + count {
                None
            } else {
                Some(n - count)
            }
        }
        _ => Some(n),
    }
}

/// Column number after a column shift; `None` = deleted.
fn col_after(n: u32, kind: ShiftKind, at: u32, count: u32) -> Option<u32> {
    match kind {
        ShiftKind::InsertCol => Some(if n >= at { n + count } else { n }),
        ShiftKind::DeleteCol => {
            if n < at {
                Some(n)
            } else if n < at + count {
                None
            } else {
                Some(n - count)
            }
        }
        _ => Some(n),
    }
}

/// Dispatch the physical part of a [`Operation::Shift`] (cell data moves;
/// [`shift_formulas`] already rewrote formula references).
pub fn shift_structure(
    sheet_bytes: &[u8],
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> Result<Vec<u8>, PatchError> {
    let out = match kind {
        ShiftKind::InsertRow | ShiftKind::DeleteRow => shift_rows(sheet_bytes, kind, at, count)?,
        ShiftKind::InsertCol | ShiftKind::DeleteCol => shift_cols(sheet_bytes, kind, at, count)?,
    };
    let out = shift_dimension(&out, kind, at, count)?;
    shift_merge_cells(&out, kind, at, count)
}

/// Rewrite a `<row>` element's `r` + child `<c>` `r` refs for a row shift;
/// returns `None` when the row itself is deleted.
fn rewrite_row_rows(raw: &str, kind: ShiftKind, at: u32, count: u32) -> Option<String> {
    let doc = Document::parse(raw).ok()?;
    let root = doc.root_element();
    let r: u32 = root.attribute("r")?.parse().ok()?;
    let new_r = row_after(r, kind, at, count)?;
    let mut ops: Vec<(usize, usize, String)> = Vec::new();
    if let Some(attr) = root.attribute_node("r") {
        let rng = attr.range_value();
        ops.push((rng.start, rng.end, new_r.to_string()));
    }
    for cell in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "c")
    {
        if let Some(attr) = cell.attribute_node("r") {
            if let Ok((_, cref)) = crate::xlsx::address::parse_ref(attr.value()) {
                if let Some(nr) = row_after(cref.row, kind, at, count) {
                    if nr != cref.row {
                        let newref = format_ref(
                            None,
                            CellRef {
                                row: nr,
                                col: cref.col,
                            },
                            false,
                        );
                        let rng = attr.range_value();
                        ops.push((rng.start, rng.end, newref));
                    }
                }
            }
        }
    }
    ops.sort_by_key(|(s, _, _)| *s);
    let mut out = raw.to_string();
    for (s, e, new) in ops.into_iter().rev() {
        out.replace_range(s..e, &new);
    }
    Some(out)
}

/// Rewrite a `<row>` element's `<c>` `r` refs for a column shift; cells in
/// the deleted column band are dropped.
fn rewrite_row_cols(raw: &str, kind: ShiftKind, at: u32, count: u32) -> String {
    let doc = match Document::parse(raw) {
        Ok(d) => d,
        Err(_) => return raw.to_string(),
    };
    let mut ops: Vec<(usize, usize, String)> = Vec::new();
    for cell in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "c")
    {
        if let Some(attr) = cell.attribute_node("r") {
            if let Ok((_, cref)) = crate::xlsx::address::parse_ref(attr.value()) {
                match col_after(cref.col, kind, at, count) {
                    Some(nc) if nc != cref.col => {
                        let newref = format_ref(
                            None,
                            CellRef {
                                row: cref.row,
                                col: nc,
                            },
                            false,
                        );
                        let rng = attr.range_value();
                        ops.push((rng.start, rng.end, newref));
                    }
                    None => ops.push((cell.range().start, cell.range().end, String::new())),
                    _ => {}
                }
            }
        }
    }
    ops.sort_by_key(|(s, _, _)| *s);
    let mut out = raw.to_string();
    for (s, e, new) in ops.into_iter().rev() {
        out.replace_range(s..e, &new);
    }
    out
}

/// Replace the `<sheetData>…</sheetData>` inner content with rebuilt rows.
fn replace_sheet_data(
    sheet_bytes: &[u8],
    doc: &Document,
    inner: &str,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let sd = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "sheetData")
        .ok_or(PatchError::Xml(crate::xml::OfficeXmlError::Parse(
            roxmltree::Error::NoRootNode,
        )))?;
    let range = sd.range();
    let raw = &text[range.clone()];
    let open_end =
        raw.find('>')
            .map(|i| i + 1)
            .ok_or(PatchError::Xml(crate::xml::OfficeXmlError::Parse(
                roxmltree::Error::NoRootNode,
            )))?;
    let self_closing = raw[..open_end].trim_end().ends_with("/>");
    let open_tag = if self_closing {
        format!("{}>", raw[..open_end].trim_end_matches('/').trim_end())
    } else {
        raw[..open_end].to_string()
    };
    let rebuilt = format!("{open_tag}{inner}</sheetData>");
    let mut out = sheet_bytes.to_vec();
    out.splice(range, rebuilt.into_bytes());
    Ok(out)
}

fn shift_rows(
    sheet_bytes: &[u8],
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;

    let mut rows: Vec<(u32, Node)> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "row")
        .filter_map(|n| n.attribute("r")?.parse::<u32>().ok().map(|r| (r, n)))
        .collect();
    rows.sort_by_key(|(r, _)| *r);

    let is_insert = matches!(kind, ShiftKind::InsertRow);
    let mut inner = String::new();
    let mut inserted = false;
    for (r, node) in &rows {
        let raw = &text[node.range()];
        if let Some(new_raw) = rewrite_row_rows(raw, kind, at, count) {
            if is_insert && !inserted && *r >= at {
                for i in 0..count {
                    inner.push_str(&format!("<row r=\"{}\"/>", at + i));
                }
                inserted = true;
            }
            inner.push_str(&new_raw);
        }
    }
    if is_insert && !inserted {
        for i in 0..count {
            inner.push_str(&format!("<row r=\"{}\"/>", at + i));
        }
    }

    replace_sheet_data(sheet_bytes, &doc, &inner)
}

fn shift_cols(
    sheet_bytes: &[u8],
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let rows: Vec<Node> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "row")
        .collect();
    let mut out = sheet_bytes.to_vec();
    for row_node in rows.into_iter().rev() {
        let raw = &text[row_node.range()];
        let new_raw = rewrite_row_cols(raw, kind, at, count);
        if new_raw != *raw {
            out.splice(row_node.range(), new_raw.into_bytes());
        }
    }
    Ok(out)
}

/// Shift one axis of a start/end coordinate pair for the dimension ref.
fn shift_axis(start: u32, end: u32, kind: ShiftKind, at: u32, count: u32) -> (u32, u32) {
    match kind {
        ShiftKind::InsertRow | ShiftKind::InsertCol => {
            let s = if start >= at { start + count } else { start };
            let e = if end >= at { end + count } else { end };
            (s, e)
        }
        _ => {
            let del_start = at;
            let del_end = at + count - 1;
            let s = if start >= del_start && start <= del_end {
                at
            } else if start > del_end {
                start - count
            } else {
                start
            };
            let removed_below = if end < del_start {
                0
            } else {
                (end - del_start + 1).min(count)
            };
            (s, (end - removed_below).max(s))
        }
    }
}

/// Best-effort update of the sheet `<dimension ref="…">` after a shift.
fn shift_dimension(
    sheet_bytes: &[u8],
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let Some(dim) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "dimension")
    else {
        return Ok(sheet_bytes.to_vec());
    };
    let Some(attr) = dim.attribute_node("ref") else {
        return Ok(sheet_bytes.to_vec());
    };
    let val = attr.value();
    let Ok((_, range)) = crate::xlsx::address::parse_range(val) else {
        return Ok(sheet_bytes.to_vec());
    };
    let is_row = matches!(kind, ShiftKind::InsertRow | ShiftKind::DeleteRow);
    let (mut sr, mut er) = (range.start.row, range.end.row);
    let (mut sc, mut ec) = (range.start.col, range.end.col);
    if is_row {
        (sr, er) = shift_axis(sr, er, kind, at, count);
    } else {
        (sc, ec) = shift_axis(sc, ec, kind, at, count);
    }
    let start_ref = format_ref(None, CellRef { row: sr, col: sc }, false);
    let end_ref = format_ref(None, CellRef { row: er, col: ec }, false);
    let new_ref = if sr == er && sc == ec {
        start_ref
    } else {
        format!("{start_ref}:{end_ref}")
    };
    let rng = attr.range_value();
    let mut out = sheet_bytes.to_vec();
    out.splice(rng, new_ref.into_bytes());
    Ok(out)
}

/// Shift `<mergeCell ref="…">` ranges; a merge whose range is deleted is
/// dropped and the `count` attribute decremented.
fn shift_merge_cells(
    sheet_bytes: &[u8],
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> Result<Vec<u8>, PatchError> {
    let text = std::str::from_utf8(sheet_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let Some(mc) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "mergeCells")
    else {
        return Ok(sheet_bytes.to_vec());
    };
    let is_row = matches!(kind, ShiftKind::InsertRow | ShiftKind::DeleteRow);
    let mut ops: Vec<(usize, usize, String)> = Vec::new();
    let mut removed = 0usize;
    for cell in mc
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "mergeCell")
    {
        let Some(attr) = cell.attribute_node("ref") else {
            continue;
        };
        let val = attr.value();
        let Ok((_, range)) = crate::xlsx::address::parse_range(val) else {
            continue;
        };
        let (sa, ea) = if is_row {
            (range.start.row, range.end.row)
        } else {
            (range.start.col, range.end.col)
        };
        let new_s = if is_row {
            row_after(sa, kind, at, count)
        } else {
            col_after(sa, kind, at, count)
        };
        let new_e = if is_row {
            row_after(ea, kind, at, count)
        } else {
            col_after(ea, kind, at, count)
        };
        match (new_s, new_e) {
            (Some(ns), Some(ne)) => {
                let (sr, er, sc, ec) = if is_row {
                    (ns, ne, range.start.col, range.end.col)
                } else {
                    (range.start.row, range.end.row, ns, ne)
                };
                let new_ref = if sr == er && sc == ec {
                    format_ref(None, CellRef { row: sr, col: sc }, false)
                } else {
                    format!(
                        "{}:{}",
                        format_ref(None, CellRef { row: sr, col: sc }, false),
                        format_ref(None, CellRef { row: er, col: ec }, false)
                    )
                };
                if new_ref != val {
                    let rng = attr.range_value();
                    ops.push((rng.start, rng.end, new_ref));
                }
            }
            _ => {
                ops.push((cell.range().start, cell.range().end, String::new()));
                removed += 1;
            }
        }
    }
    ops.sort_by_key(|(s, _, _)| *s);
    let mut out = sheet_bytes.to_vec();
    for (s, e, new) in ops.into_iter().rev() {
        out.splice(s..e, new.into_bytes());
    }
    if removed > 0 {
        if let Some(count_attr) = mc.attribute_node("count") {
            let cur: usize = count_attr.value().parse().unwrap_or(0);
            let rng = count_attr.range_value();
            let new_count = cur.saturating_sub(removed);
            let mut out2 = out.clone();
            out2.splice(rng, new_count.to_string().into_bytes());
            out = out2;
        }
    }
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────
// sharedStrings.xml append
// ────────────────────────────────────────────────────────────────────────

/// Append new `<si>` entries to a sharedStrings part, bumping `count` and
/// `uniqueCount`. Returns (new bytes, index of the first appended string).
/// The write path for single-cell text edits uses inline strings (simpler +
/// byte-surgical); this is the shared-strings path for bulk imports.
pub fn append_shared_strings(
    sst_bytes: &[u8],
    new_strings: &[String],
) -> Result<(Vec<u8>, u32), PatchError> {
    let text = std::str::from_utf8(sst_bytes)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc = Document::parse(text).map_err(crate::xml::OfficeXmlError::Parse)?;
    let root = doc.root_element();
    let count: u32 = root
        .attribute("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let unique: u32 = root
        .attribute("uniqueCount")
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    let mut si_xml = String::new();
    for s in new_strings {
        si_xml.push_str(&format!("<si><t>{}</t></si>", escape_text(s)));
    }

    let mut out = sst_bytes.to_vec();
    // Bump count attributes.
    if let Some(attr) = root.attribute_node("count") {
        let range = attr.range_value();
        out.splice(
            range,
            (count + new_strings.len() as u32).to_string().into_bytes(),
        );
    }
    // Re-parse after the first splice to find uniqueCount + insert point.
    let text2 = std::str::from_utf8(&out)
        .map_err(|_| PatchError::Xml(crate::xml::OfficeXmlError::NotUtf8))?;
    let doc2 = Document::parse(text2).map_err(crate::xml::OfficeXmlError::Parse)?;
    let root2 = doc2.root_element();
    if let Some(attr) = root2.attribute_node("uniqueCount") {
        let range = attr.range_value();
        out.splice(
            range,
            (unique + new_strings.len() as u32).to_string().into_bytes(),
        );
    }
    // Insert before </sst>.
    let text_final = out_to_str(&out);
    let close =
        text_final
            .rfind("</sst>")
            .ok_or(PatchError::Xml(crate::xml::OfficeXmlError::Parse(
                roxmltree::Error::NoRootNode,
            )))?;
    out.splice(close..close, si_xml.into_bytes());
    Ok((out, unique))
}

fn out_to_str(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xlsx::address::parse_range;

    /// Minimal xlsx with the parts `apply_batch` needs.
    pub(crate) fn sample_xlsx(sheet_xml: &str, sst: Option<&str>) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
</Types>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
        let wb = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;
        let wb_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#;
        let ss = sst.unwrap_or(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2">
  <si><t>Alpha</t></si><si><t>Beta</t></si>
</sst>"#,
        );
        let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
  <borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
  <cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>
</styleSheet>"#;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in [
            ("[Content_Types].xml", ct),
            ("_rels/.rels", rels),
            ("xl/workbook.xml", wb),
            ("xl/_rels/workbook.xml.rels", wb_rels),
            ("xl/worksheets/sheet1.xml", sheet_xml),
            ("xl/sharedStrings.xml", ss),
            ("xl/styles.xml", styles),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn sheet() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>10</v></c><c r="B1"><v>20</v></c></row><row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>2</v></c></row><row r="3"><c r="A3"><f>SUM(A1:A2)</f><v>0</v></c></row></sheetData></worksheet>"#
    }

    fn batch(ops: Vec<Operation>) -> WorkbookCommandBatch {
        let mut b = WorkbookCommandBatch::new(1, "test batch");
        b.operations = ops;
        b
    }

    #[test]
    fn set_cell_number_and_text() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![
            Operation::SetCell {
                address: CellRef { row: 1, col: 1 },
                value: Scalar::Number(42.0),
            },
            Operation::SetCell {
                address: CellRef { row: 5, col: 1 },
                value: Scalar::Text("new row".into()),
            },
        ]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let sheet = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(sheet.contains("<c r=\"A1\"><v>42</v></c>"), "{sheet}");
        assert!(
            sheet.contains("<c r=\"A5\" t=\"inlineStr\"><is><t>new row</t></is></c>"),
            "{sheet}"
        );
        assert!(sheet.contains("<row r=\"5\">"), "{sheet}");
    }

    #[test]
    fn formula_write_gets_ironcalc_value() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::SetFormula {
            address: CellRef { row: 4, col: 1 },
            formula: "=SUM(A1:A3)".into(),
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let sheet = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        // `<f>` follows the Excel convention (no leading '=').
        assert!(sheet.contains("<c r=\"A4\"><f>SUM(A1:A3)</f>"), "{sheet}");
        // The value must be the engine-computed result (11 + A3's SUM result).
        // A3 = SUM(A1:A2) = 11, so A4 = 10 + 1 + 11 = 22.
        assert!(sheet.contains("<v>22</v>"), "{sheet}");
    }

    #[test]
    fn clear_and_sort_and_fill() {
        let bytes = sample_xlsx(sheet(), None);
        let (_, range) = parse_range("A1:B2").unwrap();
        let b = batch(vec![Operation::SortRange {
            range,
            by_col: 1,
            desc: true,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let sheet = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        // Rows 1,2 (10,1) sorted desc by col A → row 1 has 10 first? desc →
        // 10 then 1: row1 = 10 stays first, row2 = 1. Asc would swap. desc:
        // 10 > 1 → order preserved.
        assert!(sheet.find("<row r=\"1\">").unwrap() < sheet.find("<row r=\"2\">").unwrap());

        // Fill constant into B3:B4
        let (_, rng) = parse_range("B3:B4").unwrap();
        let b2 = batch(vec![Operation::FillRange {
            range: rng,
            mode: FillMode::Constant,
            value: Some(Scalar::Number(7.0)),
        }]);
        let out2 = apply_batch(&bytes, &b2, "Sheet1").unwrap();
        let mut a2 = crate::zip::OoxmlArchive::open(out2.bytes).unwrap();
        let s2 = String::from_utf8(a2.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(s2.contains("<c r=\"B3\"><v>7</v></c>"), "{s2}");
        assert!(s2.contains("<c r=\"B4\"><v>7</v></c>"), "{s2}");

        // Clear A2:B2
        let (_, rng) = parse_range("A2:B2").unwrap();
        let b3 = batch(vec![Operation::ClearRange { range: rng }]);
        let out3 = apply_batch(&bytes, &b3, "Sheet1").unwrap();
        let mut a3 = crate::zip::OoxmlArchive::open(out3.bytes).unwrap();
        let s3 = String::from_utf8(a3.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(!s3.contains("<c r=\"A2\">"), "{s3}");
        assert!(!s3.contains("<c r=\"B2\">"), "{s3}");
    }

    #[test]
    fn shift_rewrites_formulas() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::Shift {
            sheet: "Sheet1".to_string(),
            kind: super::super::dsl::ShiftKind::InsertRow,
            at: 2,
            count: 1,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(s.contains("<f>SUM(A1:A3)</f>"), "{s}");
    }

    #[test]
    fn insert_row_physically_moves_cells() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::Shift {
            sheet: "Sheet1".to_string(),
            kind: ShiftKind::InsertRow,
            at: 2,
            count: 1,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(s.contains("<row r=\"2\"/>"), "{s}"); // empty inserted row
        assert!(s.contains("<c r=\"A3\"><v>1</v></c>"), "{s}"); // old A2 moved down
        assert!(s.contains("<c r=\"B3\"><v>2</v></c>"), "{s}"); // old B2 moved down
        assert!(s.contains("<c r=\"A4\"><f>SUM(A1:A3)</f>"), "{s}"); // formula cell + ref shifted
        assert!(!s.contains("<c r=\"A2\">"), "{s}");
    }

    #[test]
    fn delete_row_physically_removes_and_shifts() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::Shift {
            sheet: "Sheet1".to_string(),
            kind: ShiftKind::DeleteRow,
            at: 2,
            count: 1,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(!s.contains("<row r=\"3\">"), "{s}"); // row 3 gone
        assert!(!s.contains("<v>1</v>"), "{s}"); // old row 2 data gone
        assert!(s.contains("<c r=\"A2\"><f>"), "{s}"); // old A3 moved up to A2
    }

    #[test]
    fn insert_col_physically_moves_cells() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::Shift {
            sheet: "Sheet1".to_string(),
            kind: ShiftKind::InsertCol,
            at: 2,
            count: 1,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(s.contains("<c r=\"C1\"><v>20</v></c>"), "{s}"); // old B1 → C1
        assert!(s.contains("<c r=\"A1\"><v>10</v></c>"), "{s}"); // A unchanged
        assert!(!s.contains("<c r=\"B1\">"), "{s}");
    }

    #[test]
    fn delete_col_physically_removes() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::Shift {
            sheet: "Sheet1".to_string(),
            kind: ShiftKind::DeleteCol,
            at: 2,
            count: 1,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(!s.contains("<c r=\"B"), "{s}"); // B column cells removed
        assert!(s.contains("<c r=\"A1\"><v>10</v></c>"), "{s}");
    }

    #[test]
    fn shift_updates_dimension_and_merges() {
        let sh = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1:C3"/>
  <sheetData><row r="1"><c r="A1"><v>1</v></c><c r="C1"><v>3</v></c></row><row r="3"><c r="A3"><v>9</v></c></row></sheetData>
  <mergeCells count="1"><mergeCell ref="A1:A3"/></mergeCells>
</worksheet>"#;
        let bytes = sample_xlsx(sh, None);
        let b = batch(vec![Operation::Shift {
            sheet: "Sheet1".to_string(),
            kind: ShiftKind::InsertRow,
            at: 2,
            count: 1,
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(s.contains("ref=\"A1:C4\""), "{s}"); // dimension end row 3→4
        assert!(s.contains("<mergeCell ref=\"A1:A4\"/>"), "{s}"); // merge shifted
        assert!(s.contains("<c r=\"A4\"><v>9</v></c>"), "{s}"); // old A3 moved down
    }

    #[test]
    fn rename_sheet_and_pivot() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![
            Operation::RenameSheet {
                from: "Sheet1".into(),
                to: "Budget".into(),
            },
            Operation::Pivot {
                source: parse_range("A1:B3").unwrap().1,
                group_by: 0,
                aggregate: 1,
                agg: super::super::dsl::PivotAgg::Sum,
            },
        ]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let wb = String::from_utf8(a.read_part("xl/workbook.xml").unwrap()).unwrap();
        assert!(wb.contains("name=\"Budget\""), "{wb}");
        let pivot = out.pivot.expect("pivot computed");
        // Group by column A (10,1,11?) — A1=10, A2=1, A3=formula→11; each
        // row is its own group key.
        assert!(pivot.len() >= 3, "{pivot:?}");
        assert!(
            pivot.iter().any(|r| r.key == "10" && r.value == 20.0),
            "{pivot:?}"
        );
    }

    #[test]
    fn untouched_parts_stay_byte_identical() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::SetCell {
            address: CellRef { row: 1, col: 1 },
            value: Scalar::Number(99.0),
        }]);
        let out = apply_batch(&bytes, &b, "Sheet1").unwrap();
        let mut in_a = crate::zip::OoxmlArchive::open(bytes).unwrap();
        let mut out_a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        assert_eq!(
            in_a.raw_entry("xl/sharedStrings.xml").unwrap(),
            out_a.raw_entry("xl/sharedStrings.xml").unwrap()
        );
        assert_eq!(
            in_a.raw_entry("[Content_Types].xml").unwrap(),
            out_a.raw_entry("[Content_Types].xml").unwrap()
        );
    }

    #[test]
    fn append_shared_strings_bumps_counts() {
        let sst = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2">
  <si><t>Alpha</t></si><si><t>Beta</t></si>
</sst>"#;
        let (out, idx) =
            append_shared_strings(sst.as_bytes(), &["Gamma & co".into(), "Delta".into()]).unwrap();
        assert_eq!(idx, 2);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("count=\"4\""), "{s}");
        assert!(s.contains("uniqueCount=\"4\""), "{s}");
        assert!(s.contains("<si><t>Gamma &amp; co</t></si>"), "{s}");
        assert!(s.contains("<si><t>Delta</t></si>"), "{s}");
    }

    // ── ARCH/04 §4.6 — the size ceiling on the xlsx write path ─────────────

    #[test]
    fn apply_batch_refuses_an_oversized_workbook_by_name() {
        let bytes = sample_xlsx(sheet(), None);
        let tight = crate::limits::PatchLimits {
            max_archive_bytes: (bytes.len() - 1) as u64,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = apply_batch_with_limits(&bytes, &batch(vec![]), "Sheet1", tight)
            .err()
            .expect("must refuse");
        assert!(matches!(
            err,
            PatchError::TooLarge {
                kind: crate::limits::LimitKind::ArchiveBytes,
                ..
            }
        ));
    }

    #[test]
    fn apply_batch_refuses_an_oversized_part_by_name() {
        let bytes = sample_xlsx(sheet(), None);
        let tight = crate::limits::PatchLimits {
            max_part_bytes: 1,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = apply_batch_with_limits(&bytes, &batch(vec![]), "Sheet1", tight)
            .err()
            .expect("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("xl/workbook.xml"), "{msg}");
        assert!(matches!(
            err,
            PatchError::TooLarge {
                kind: crate::limits::LimitKind::PartBytes,
                ..
            }
        ));
    }

    #[test]
    fn apply_batch_refuses_a_package_with_too_many_entries() {
        let bytes = sample_xlsx(sheet(), None);
        let tight = crate::limits::PatchLimits {
            max_parts: 2,
            ..crate::limits::PatchLimits::default_policy()
        };
        let err = apply_batch_with_limits(&bytes, &batch(vec![]), "Sheet1", tight)
            .err()
            .expect("must refuse");
        assert!(matches!(err, PatchError::TooManyParts { limit: 2, .. }));
    }

    #[test]
    fn apply_batch_under_the_default_policy_still_writes() {
        let bytes = sample_xlsx(sheet(), None);
        let b = batch(vec![Operation::SetCell {
            address: CellRef { row: 1, col: 1 },
            value: Scalar::Number(7.0),
        }]);
        let out = apply_batch_with_limits(
            &bytes,
            &b,
            "Sheet1",
            crate::limits::PatchLimits::default_policy(),
        )
        .expect("the documented ceilings must not break the normal path");
        let mut a = crate::zip::OoxmlArchive::open(out.bytes).unwrap();
        let s = String::from_utf8(a.read_part("xl/worksheets/sheet1.xml").unwrap()).unwrap();
        assert!(s.contains("<v>7</v>"), "{s}");
    }
}
