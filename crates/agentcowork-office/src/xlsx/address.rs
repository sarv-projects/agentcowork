//! Cell-address parsing/formatting for the workbook DSL (doc 28 §5
//! `cell-address.ts` pattern). A1-style coordinates with optional sheet
//! prefix (`Sheet1!B2`, `'My Sheet'!B2`), absolute markers (`$A$1`), and
//! ranges (`A1:B10`).
//!
//! Rows and columns are 1-based internally (matching Excel / IronCalc's
//! `set_user_input`), and `0` is never a valid coordinate.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub fn new(row: u32, col: u32) -> Result<Self, AddressError> {
        if row == 0 || col == 0 {
            return Err(AddressError::OutOfRange);
        }
        Ok(Self { row, col })
    }
}

impl std::fmt::Display for CellRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_ref(None, *self, false))
    }
}

impl std::fmt::Display for RangeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}:{}", self.start, self.end)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeRef {
    pub start: CellRef,
    pub end: CellRef,
}

impl RangeRef {
    /// A single-cell range.
    pub fn single(cell: CellRef) -> Self {
        Self {
            start: cell,
            end: cell,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AddressError {
    #[error("invalid cell reference: {0}")]
    Invalid(String),
    #[error("row/column out of range (must be >= 1)")]
    OutOfRange,
    #[error("range start must be <= end: {0}")]
    Inverted(String),
}

/// Column letter → 1-based index (A=1, Z=26, AA=27).
pub fn col_index(letters: &str) -> Option<u32> {
    let mut idx: u32 = 0;
    for ch in letters.chars() {
        let c = ch.to_ascii_uppercase();
        if !c.is_ascii_alphabetic() {
            return None;
        }
        idx = idx
            .checked_mul(26)?
            .checked_add((c as u32) - ('A' as u32) + 1)?;
    }
    if idx == 0 { None } else { Some(idx) }
}

/// 1-based column index → column letters (1→A, 27→AA).
pub fn col_letter(idx: u32) -> Option<String> {
    if idx == 0 {
        return None;
    }
    let mut n = idx;
    let mut out = Vec::new();
    while n > 0 {
        // Excel's scheme skips the 'A' carry: 26 → Z (not AA).
        let rem = (n - 1) % 26;
        out.push((b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    out.reverse();
    Some(out.into_iter().collect())
}

/// Parse a bare A1-style ref (`B2`, `$A$1`, `AB12`) — no sheet prefix.
fn parse_bare(s: &str) -> Result<CellRef, AddressError> {
    let s = s.trim();
    let letters: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '$')
        .collect();
    let digits: String = s[letters.len()..].to_string();
    if letters.is_empty() || digits.is_empty() {
        return Err(AddressError::Invalid(s.to_string()));
    }
    let col =
        col_index(&letters.replace('$', "")).ok_or_else(|| AddressError::Invalid(s.to_string()))?;
    let row: u32 = digits
        .parse()
        .map_err(|_| AddressError::Invalid(s.to_string()))?;
    CellRef::new(row, col)
}

/// Parse a full ref: optional `Sheet1!` / `'My Sheet'!` prefix + A1 ref.
pub fn parse_ref(s: &str) -> Result<(Option<String>, CellRef), AddressError> {
    let s = s.trim();
    if let Some(bang) = s.rfind('!') {
        let sheet = &s[..bang];
        let cell = &s[bang + 1..];
        let sheet = if sheet.starts_with('\'') && sheet.ends_with('\'') {
            sheet[1..sheet.len() - 1].to_string()
        } else {
            sheet.to_string()
        };
        if sheet.is_empty() {
            return Err(AddressError::Invalid(s.to_string()));
        }
        Ok((Some(sheet), parse_bare(cell)?))
    } else {
        Ok((None, parse_bare(s)?))
    }
}

/// Parse a range (`A1:B10`, `Sheet1!A1:B10`). Both endpoints must share the
/// same sheet prefix; a sheet prefix on the first endpoint applies to both.
pub fn parse_range(s: &str) -> Result<(Option<String>, RangeRef), AddressError> {
    let s = s.trim();
    if let Some(colon) = s.rfind(':') {
        let (head, tail) = (&s[..colon], &s[colon + 1..]);
        let (sheet, start) = parse_ref(head)?;
        let (tail_sheet, end) = parse_ref(tail)?;
        if tail_sheet.is_some() && tail_sheet != sheet {
            return Err(AddressError::Invalid(s.to_string()));
        }
        let range = RangeRef { start, end };
        if start.row > end.row || start.col > end.col {
            return Err(AddressError::Inverted(s.to_string()));
        }
        Ok((sheet, range))
    } else {
        let (sheet, cell) = parse_ref(s)?;
        Ok((sheet, RangeRef::single(cell)))
    }
}

/// Format a ref back to A1 style. `abs` controls `$` markers (DSL never
/// needs them on write, but formula-shift tests do).
pub fn format_ref(sheet: Option<&str>, cell: CellRef, abs: bool) -> String {
    let col = col_letter(cell.col).expect("col >= 1");
    let mut out = String::new();
    if let Some(s) = sheet {
        if s.contains(' ') || s.contains('!') {
            out.push('\'');
            out.push_str(s);
            out.push('\'');
        } else {
            out.push_str(s);
        }
        out.push('!');
    }
    if abs {
        out.push('$');
    }
    out.push_str(&col);
    if abs {
        out.push('$');
    }
    out.push_str(&cell.row.to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn col_letters_round_trip() {
        assert_eq!(col_letter(1).as_deref(), Some("A"));
        assert_eq!(col_letter(26).as_deref(), Some("Z"));
        assert_eq!(col_letter(27).as_deref(), Some("AA"));
        assert_eq!(col_letter(52).as_deref(), Some("AZ"));
        assert_eq!(col_letter(53).as_deref(), Some("BA"));
        assert_eq!(col_letter(702).as_deref(), Some("ZZ"));
        assert_eq!(col_letter(703).as_deref(), Some("AAA"));
        assert_eq!(col_index("A"), Some(1));
        assert_eq!(col_index("z"), Some(26));
        assert_eq!(col_index("AA"), Some(27));
        assert_eq!(col_index("ZZ"), Some(702));
        assert_eq!(col_index("AAA"), Some(703));
        assert_eq!(col_index(""), None);
        assert_eq!(col_index("1A"), None);
    }

    #[test]
    fn parse_bare_refs() {
        let (sheet, cell) = parse_ref("B2").unwrap();
        assert_eq!(sheet, None);
        assert_eq!(cell, CellRef { row: 2, col: 2 });
        let (_, cell) = parse_ref("$A$1").unwrap();
        assert_eq!(cell, CellRef { row: 1, col: 1 });
        let (_, cell) = parse_ref("AB12").unwrap();
        assert_eq!(cell, CellRef { row: 12, col: 28 });
        assert!(parse_ref("").is_err());
        assert!(parse_ref("A0").is_err());
        assert!(parse_ref("1A").is_err());
        assert!(parse_ref("A").is_err());
    }

    #[test]
    fn parse_sheet_prefixed_refs() {
        let (sheet, cell) = parse_ref("Sheet1!B2").unwrap();
        assert_eq!(sheet.as_deref(), Some("Sheet1"));
        assert_eq!(cell, CellRef { row: 2, col: 2 });
        let (sheet, cell) = parse_ref("'My Sheet'!C4").unwrap();
        assert_eq!(sheet.as_deref(), Some("My Sheet"));
        assert_eq!(cell, CellRef { row: 4, col: 3 });
    }

    #[test]
    fn parse_ranges() {
        let (sheet, range) = parse_range("A1:B10").unwrap();
        assert_eq!(sheet, None);
        assert_eq!(range.start, CellRef { row: 1, col: 1 });
        assert_eq!(range.end, CellRef { row: 10, col: 2 });
        let (sheet, range) = parse_range("Sheet1!A1:B10").unwrap();
        assert_eq!(sheet.as_deref(), Some("Sheet1"));
        assert_eq!(range.start, CellRef { row: 1, col: 1 });
        assert_eq!(range.end, CellRef { row: 10, col: 2 });
        // single cell = single-cell range
        let (_, range) = parse_range("C3").unwrap();
        assert_eq!(range.start, range.end);
        // inverted range rejected
        assert!(parse_range("B10:A1").is_err());
    }

    #[test]
    fn format_back_to_a1() {
        assert_eq!(format_ref(None, CellRef { row: 2, col: 2 }, false), "B2");
        assert_eq!(
            format_ref(Some("Sheet1"), CellRef { row: 4, col: 3 }, false),
            "Sheet1!C4"
        );
        assert_eq!(
            format_ref(Some("My Sheet"), CellRef { row: 1, col: 27 }, false),
            "'My Sheet'!AA1"
        );
        assert_eq!(format_ref(None, CellRef { row: 1, col: 1 }, true), "$A$1");
    }
}
