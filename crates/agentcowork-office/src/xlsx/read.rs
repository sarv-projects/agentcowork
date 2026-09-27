//! Fast xlsx reading via `calamine` — sheet names, dimensions, and
//! windowed cell reads (the virtualized 100K+ row table view pulls only the
//! visible window). Pure read path; writes go through the surgical part
//! patch (`xlsx/patch.rs`).

use calamine::{Data, Range, Reader, Xlsx, open_workbook};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

use super::address::{AddressError, RangeRef};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum CellValue {
    #[default]
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(String),
}

impl CellValue {
    /// Display form (what the grid shows; number formatting is P4.7's job).
    pub fn display(&self) -> String {
        match self {
            CellValue::Empty => String::new(),
            CellValue::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{n:.0}")
                } else {
                    format!("{n}")
                }
            }
            CellValue::Text(s) => s.clone(),
            CellValue::Bool(b) => b.to_string(),
            CellValue::Error(e) => format!("#{e}"),
        }
    }
}

impl From<Data> for CellValue {
    fn from(d: Data) -> Self {
        match d {
            Data::Empty => CellValue::Empty,
            Data::Int(i) => CellValue::Number(i as f64),
            Data::Float(f) => CellValue::Number(f),
            Data::String(s) => CellValue::Text(s),
            Data::Bool(b) => CellValue::Bool(b),
            Data::DateTime(dt) => CellValue::Number(dt.as_f64()),
            Data::DateTimeIso(s) => CellValue::Text(s),
            Data::DurationIso(s) => CellValue::Text(s),
            Data::Error(e) => CellValue::Error(e.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetMeta {
    pub name: String,
    /// 1-based dimensions of the used range (row, col) — inclusive.
    pub rows: u32,
    pub cols: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbookMeta {
    pub path: String,
    pub sheets: Vec<SheetMeta>,
}

/// One windowed slice of a sheet: absolute `offset` row (0-based) + up to
/// `limit` rows, each row a fixed-width vec of cell values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetWindow {
    pub sheet: String,
    pub offset: u32,
    pub rows: Vec<Vec<CellValue>>,
    /// Total rows/cols of the sheet (for scroll-range + column headers).
    pub total_rows: u32,
    pub total_cols: u32,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReadError {
    #[error("cannot open workbook {path}: {err}")]
    Open { path: String, err: String },
    #[error("sheet not found: {0}")]
    SheetNotFound(String),
    #[error("{0}")]
    Address(#[from] AddressError),
}

pub fn open(path: &Path) -> Result<WorkbookMeta, ReadError> {
    let mut wb: Xlsx<std::io::BufReader<std::fs::File>> =
        open_workbook(path).map_err(|e: calamine::XlsxError| ReadError::Open {
            path: path.display().to_string(),
            err: e.to_string(),
        })?;
    let mut sheets = Vec::new();
    for name in wb.sheet_names().to_vec() {
        let range = wb.worksheet_range(&name).map_err(|e| ReadError::Open {
            path: path.display().to_string(),
            err: e.to_string(),
        })?;
        let (rows, cols) = dims(&range);
        sheets.push(SheetMeta { name, rows, cols });
    }
    Ok(WorkbookMeta {
        path: path.display().to_string(),
        sheets,
    })
}

/// Read a windowed slice of one sheet. `offset`/`limit` are row-based
/// (0-based offset into the used range, like a paged table). Column width is
/// the sheet's used column count.
pub fn read_window(
    path: &Path,
    sheet: &str,
    offset: u32,
    limit: u32,
) -> Result<SheetWindow, ReadError> {
    let mut wb: Xlsx<std::io::BufReader<std::fs::File>> =
        open_workbook(path).map_err(|e: calamine::XlsxError| ReadError::Open {
            path: path.display().to_string(),
            err: e.to_string(),
        })?;
    let range = wb
        .worksheet_range(sheet)
        .map_err(|_| ReadError::SheetNotFound(sheet.to_string()))?;
    let (start, end) = dims_raw(&range);
    let start_row = start.0;
    let start_col = start.1;
    let end_row = end.0;
    let end_col = end.1;
    let total_rows = if end_row >= start_row {
        end_row - start_row + 1
    } else {
        0
    };
    let total_cols = if end_col >= start_col {
        end_col - start_col + 1
    } else {
        0
    };

    let from = start_row + offset;
    let to = from.saturating_add(limit).saturating_sub(1).min(end_row);
    let mut rows = Vec::new();
    if from <= to && start_col <= end_col {
        // Slice the used range to just the window, then read every cell
        // (empty cells inside the window come back as Data::Empty).
        let window = range.range((from, start_col), (to, end_col));
        for r in 0..window.height() {
            let mut row = Vec::with_capacity(total_cols as usize);
            for c in 0..window.width() {
                let v = window
                    .get((r, c))
                    .cloned()
                    .map(CellValue::from)
                    .unwrap_or(CellValue::Empty);
                row.push(v);
            }
            rows.push(row);
        }
    }

    Ok(SheetWindow {
        sheet: sheet.to_string(),
        offset,
        rows,
        total_rows,
        total_cols,
    })
}

/// Read a specific A1 range from one sheet into a flat cell grid. `range`
/// is 1-based (row/col); the returned rows are trimmed to the range's width
/// and are directly usable by `dsl::pivot_result`.
pub fn read_range(
    path: &Path,
    sheet: &str,
    range: &RangeRef,
) -> Result<Vec<Vec<CellValue>>, ReadError> {
    let mut wb: Xlsx<std::io::BufReader<std::fs::File>> =
        open_workbook(path).map_err(|e: calamine::XlsxError| ReadError::Open {
            path: path.display().to_string(),
            err: e.to_string(),
        })?;
    let full = wb
        .worksheet_range(sheet)
        .map_err(|_| ReadError::SheetNotFound(sheet.to_string()))?;

    let start_row = range.start.row.saturating_sub(1);
    let start_col = range.start.col.saturating_sub(1);
    let end_row = range.end.row.saturating_sub(1);
    let end_col = range.end.col.saturating_sub(1);

    let slice = full.range((start_row, start_col), (end_row, end_col));
    let mut rows = Vec::with_capacity(slice.height());
    for r in 0..slice.height() {
        let mut row = Vec::with_capacity(slice.width());
        for c in 0..slice.width() {
            let v = slice
                .get((r, c))
                .cloned()
                .map(CellValue::from)
                .unwrap_or(CellValue::Empty);
            row.push(v);
        }
        rows.push(row);
    }
    Ok(rows)
}

fn dims(range: &Range<Data>) -> (u32, u32) {
    let (start, end) = dims_raw(range);
    (
        end.0.saturating_sub(start.0) + 1,
        end.1.saturating_sub(start.1) + 1,
    )
}

fn dims_raw(range: &Range<Data>) -> ((u32, u32), (u32, u32)) {
    let start = range.start().unwrap_or((0, 0));
    let end = range.end().unwrap_or((0, 0));
    (start, end)
}
