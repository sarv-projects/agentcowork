//! Workbook DSL (doc 28 §5 `workbook-dsl.ts` pattern) — the typed command
//! language the deterministic planner compiles to, and the **formula-shift**
//! engine that rewrites A1 references the way Excel does after structural
//! row/column edits.
//!
//! Shift semantics (from GenOffice `formula-shift.ts`):
//! - refs into the shifted region **move**; `$` markers do NOT pin against
//!   insert/delete (they only matter for copy/fill);
//! - refs into a **deleted region → `#REF!`**;
//! - ranges partially overlapping a deleted region **shrink**;
//! - `Sheet1!B2` / `'My Sheet'!B2` prefixes are only rewritten when the
//!   prefix names the shifted sheet; quoted string literals are skipped and
//!   function names like `LOG10` are protected via boundary checks.

use serde::{Deserialize, Serialize};

use super::address::{CellRef, RangeRef, format_ref};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scalar {
    Number(f64),
    Text(String),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShiftKind {
    InsertRow,
    DeleteRow,
    InsertCol,
    DeleteCol,
}

impl ShiftKind {
    fn is_row(&self) -> bool {
        matches!(self, ShiftKind::InsertRow | ShiftKind::DeleteRow)
    }
    fn is_insert(&self) -> bool {
        matches!(self, ShiftKind::InsertRow | ShiftKind::InsertCol)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillMode {
    /// Repeat one value across the range ("fill B2:B10 with 5").
    Constant,
    /// Copy the top cell down, extending numeric runs by their delta
    /// (1,2,3 → 4,5,…). Non-numeric cells copy verbatim.
    CopyDown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    SetCell {
        address: CellRef,
        value: Scalar,
    },
    SetFormula {
        address: CellRef,
        formula: String,
    },
    ClearRange {
        range: RangeRef,
    },
    RenameSheet {
        from: String,
        to: String,
    },
    SortRange {
        range: RangeRef,
        by_col: u32,
        desc: bool,
    },
    FillRange {
        range: RangeRef,
        mode: FillMode,
        value: Option<Scalar>,
    },
    /// Structural insert/delete; the patch layer rewrites every formula on
    /// the target sheet via `shift_formula`.
    Shift {
        sheet: String,
        kind: ShiftKind,
        at: u32,
        count: u32,
    },
    /// Declared pivot. Execution returns an in-memory grouped summary
    /// (`pivot_result`); writing the summary as a new sheet part is the
    /// office-UI follow-up (P4.7).
    Pivot {
        source: RangeRef,
        group_by: u32,
        aggregate: u32,
        agg: PivotAgg,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotAgg {
    Sum,
    Count,
    Avg,
}

impl std::fmt::Display for PivotAgg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PivotAgg::Sum => write!(f, "sum"),
            PivotAgg::Count => write!(f, "count"),
            PivotAgg::Avg => write!(f, "avg"),
        }
    }
}

/// GenOffice's transaction model: optimistic concurrency via `base_revision`,
/// `transaction_id` for undo, one summary line for the audit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkbookCommandBatch {
    pub dsl_version: u32,
    pub transaction_id: String,
    pub base_revision: u64,
    pub summary: String,
    pub operations: Vec<Operation>,
}

impl WorkbookCommandBatch {
    pub fn new(base_revision: u64, summary: impl Into<String>) -> Self {
        Self {
            dsl_version: 1,
            transaction_id: format!(
                "txn-{base_revision}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ),
            base_revision,
            summary: summary.into(),
            operations: Vec::new(),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Formula shifting
// ────────────────────────────────────────────────────────────────────────

/// Rewrite A1 references in `formula` after a structural row/column edit on
/// the sheet named `target_sheet` (bare refs are on that sheet).
pub fn shift_formula(
    formula: &str,
    target_sheet: &str,
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> String {
    let mut out = String::with_capacity(formula.len() + 8);
    let chars: Vec<char> = formula.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                // Copy the quoted string literal verbatim.
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                i += 1; // closing quote (or end)
                out.push_str(&chars[start..i.min(chars.len())].iter().collect::<String>());
            }
            '\'' => {
                // Possible quoted sheet prefix 'My Sheet'!
                if let Some((consumed, sheet, after)) = quoted_sheet(&chars, i) {
                    if let Some((len, _ref1)) = try_ref(&chars, after) {
                        let ref1 = _ref1;
                        if sheet != target_sheet {
                            // Foreign sheet ref — copy through untouched,
                            // including any `:ref2` range tail.
                            let end = range_tail_end(&chars, after + len);
                            out.push_str(&chars[i..end].iter().collect::<String>());
                            i = end;
                            continue;
                        }
                        // Range?
                        if after + len < chars.len()
                            && chars[after + len] == ':'
                            && after + len + 1 < chars.len()
                            && chars[after + len + 1].is_ascii_alphabetic()
                        {
                            if let Some((len2, ref2)) = try_ref(&chars, after + len + 1) {
                                let (s, e, reff) = shift_range(ref1, ref2, kind, at, count);
                                out.push_str(&format!("'{}'!{}", sheet, fmt_range(s, e, reff)));
                                i = after + len + 1 + len2;
                                continue;
                            }
                        }
                        out.push_str(&format!(
                            "'{}'!{}",
                            sheet,
                            fmt_shifted_ref(ref1, kind, at, count)
                        ));
                        i = after + len;
                        continue;
                    }
                    out.push_str(&chars[i..consumed].iter().collect::<String>());
                    i = consumed;
                    continue;
                }
                out.push(c);
                i += 1;
            }
            '$' if i + 1 < chars.len() && chars[i + 1].is_ascii_alphabetic() => {
                // Absolute marker: treat `$A$3` as one token (`$` does not
                // pin against insert/delete — GenOffice rule).
                if let Some((len, ref1)) = try_ref(&chars, i) {
                    let is_function = i + len < chars.len() && chars[i + len] == '(';
                    if !is_function {
                        if i + len < chars.len()
                            && chars[i + len] == ':'
                            && i + len + 1 < chars.len()
                            && chars[i + len + 1].is_ascii_alphabetic()
                        {
                            if let Some((len2, ref2)) = try_ref(&chars, i + len + 1) {
                                let (s, e, reff) = shift_range(ref1, ref2, kind, at, count);
                                out.push_str(&fmt_range(s, e, reff));
                                i = i + len + 1 + len2;
                                continue;
                            }
                        }
                        out.push_str(&fmt_shifted_ref(ref1, kind, at, count));
                        i += len;
                        continue;
                    }
                }
                out.push(c);
                i += 1;
            }
            _ if c.is_ascii_alphabetic() => {
                // Boundary: not preceded by alnum/_ (avoid partial matches).
                let prev_ok =
                    i == 0 || !(chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_');
                if prev_ok {
                    // Unquoted sheet prefix? `Sheet1!`
                    if let Some((consumed, sheet, after)) = unquoted_sheet(&chars, i) {
                        if let Some((len, ref1)) = try_ref(&chars, after) {
                            if sheet != target_sheet {
                                let end = range_tail_end(&chars, after + len);
                                out.push_str(&chars[i..end].iter().collect::<String>());
                                i = end;
                                continue;
                            }
                            if after + len < chars.len()
                                && chars[after + len] == ':'
                                && after + len + 1 < chars.len()
                                && chars[after + len + 1].is_ascii_alphabetic()
                            {
                                if let Some((len2, ref2)) = try_ref(&chars, after + len + 1) {
                                    let (s, e, reff) = shift_range(ref1, ref2, kind, at, count);
                                    out.push_str(&format!("{}!{}", sheet, fmt_range(s, e, reff)));
                                    i = after + len + 1 + len2;
                                    continue;
                                }
                            }
                            out.push_str(&format!(
                                "{}!{}",
                                sheet,
                                fmt_shifted_ref(ref1, kind, at, count)
                            ));
                            i = after + len;
                            continue;
                        }
                        // Identifier but no ref after '!' — copy as-is.
                        out.push_str(&chars[i..consumed].iter().collect::<String>());
                        i = consumed;
                        continue;
                    }
                    if let Some((len, ref1)) = try_ref(&chars, i) {
                        // Function-name protection: `LOG10(` is not a ref.
                        let is_function = i + len < chars.len() && chars[i + len] == '(';
                        if !is_function {
                            // Range?
                            if i + len < chars.len()
                                && chars[i + len] == ':'
                                && i + len + 1 < chars.len()
                                && chars[i + len + 1].is_ascii_alphabetic()
                            {
                                if let Some((len2, ref2)) = try_ref(&chars, i + len + 1) {
                                    let (s, e, reff) = shift_range(ref1, ref2, kind, at, count);
                                    out.push_str(&fmt_range(s, e, reff));
                                    i = i + len + 1 + len2;
                                    continue;
                                }
                            }
                            out.push_str(&fmt_shifted_ref(ref1, kind, at, count));
                            i += len;
                            continue;
                        }
                    }
                }
                out.push(c);
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `'Quoted Sheet'!` prefix starting at `i` → (consumed, sheet, index-after-!).
fn quoted_sheet(chars: &[char], i: usize) -> Option<(usize, String, usize)> {
    if chars[i] != '\'' {
        return None;
    }
    let mut j = i + 1;
    while j < chars.len() && chars[j] != '\'' {
        j += 1;
    }
    if j >= chars.len() {
        return None;
    }
    // Handle escaped '' inside quoted names.
    let mut name = String::new();
    let mut k = i + 1;
    while k < j {
        if chars[k] == '\'' && k + 1 < j && chars[k + 1] == '\'' {
            name.push('\'');
            k += 2;
        } else {
            name.push(chars[k]);
            k += 1;
        }
    }
    if j + 1 < chars.len() && chars[j + 1] == '!' {
        Some((j + 2, name, j + 2))
    } else {
        None
    }
}

/// Bare `Sheet1!` prefix starting at `i` (identifier chars only).
fn unquoted_sheet(chars: &[char], i: usize) -> Option<(usize, String, usize)> {
    let mut j = i;
    while j < chars.len()
        && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '.')
    {
        j += 1;
    }
    if j <= i || j >= chars.len() || chars[j] != '!' {
        return None;
    }
    let name: String = chars[i..j].iter().collect();
    Some((j + 1, name, j + 1))
}

/// Try to match `[A-Za-z]+[0-9]+` (optionally with `$` markers) at `i`.
/// Returns (consumed, CellRef).
fn try_ref(chars: &[char], i: usize) -> Option<(usize, CellRef)> {
    let mut j = i;
    // optional $ markers + letters
    let mut letters = String::new();
    while j < chars.len() {
        let c = chars[j];
        if c == '$' || c.is_ascii_alphabetic() {
            if c != '$' {
                letters.push(c);
            }
            j += 1;
        } else {
            break;
        }
    }
    if letters.is_empty() {
        return None;
    }
    let mut digits = String::new();
    while j < chars.len() && chars[j].is_ascii_digit() {
        digits.push(chars[j]);
        j += 1;
    }
    if digits.is_empty() {
        return None;
    }
    let col = super::address::col_index(&letters)?;
    let row: u32 = digits.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((j - i, CellRef { row, col }))
}

/// Apply a shift to a single ref → (row, col) or (0,0) for #REF!.
fn shift_ref(cell: CellRef, kind: ShiftKind, at: u32, count: u32) -> (u32, u32) {
    if kind.is_row() {
        if kind.is_insert() {
            if cell.row >= at {
                (cell.row + count, cell.col)
            } else {
                (cell.row, cell.col)
            }
        } else {
            // delete
            if cell.row >= at && cell.row < at + count {
                (0, 0) // #REF!
            } else if cell.row >= at + count {
                (cell.row - count, cell.col)
            } else {
                (cell.row, cell.col)
            }
        }
    } else if kind.is_insert() {
        if cell.col >= at {
            (cell.row, cell.col + count)
        } else {
            (cell.row, cell.col)
        }
    } else if cell.col >= at && cell.col < at + count {
        (0, 0)
    } else if cell.col >= at + count {
        (cell.row, cell.col - count)
    } else {
        (cell.row, cell.col)
    }
}

/// Apply a shift to a range's two endpoints → (start, end) or #REF!.
fn shift_range(
    start: CellRef,
    end: CellRef,
    kind: ShiftKind,
    at: u32,
    count: u32,
) -> ((u32, u32), (u32, u32), bool) {
    let coord = |c: CellRef| if kind.is_row() { c.row } else { c.col };
    let mk = |a: u32, b: u32| {
        if kind.is_row() {
            (
                CellRef {
                    row: a,
                    col: start.col,
                },
                CellRef {
                    row: b,
                    col: end.col,
                },
            )
        } else {
            (
                CellRef {
                    row: start.row,
                    col: a,
                },
                CellRef {
                    row: end.row,
                    col: b,
                },
            )
        }
    };
    let (s, e) = (coord(start), coord(end));
    if kind.is_insert() {
        if s >= at {
            let (ns, ne) = mk(s + count, e + count);
            ((ns.row, ns.col), (ne.row, ne.col), false)
        } else if e >= at {
            let (ns, ne) = mk(s, e + count);
            ((ns.row, ns.col), (ne.row, ne.col), false)
        } else {
            ((start.row, start.col), (end.row, end.col), false)
        }
    } else {
        // delete region [at, at+count)
        if e < at {
            ((start.row, start.col), (end.row, end.col), false)
        } else if s >= at + count {
            let (ns, ne) = mk(s - count, e - count);
            ((ns.row, ns.col), (ne.row, ne.col), false)
        } else if s >= at {
            // start inside deleted region
            if e >= at + count {
                let (ns, ne) = mk(at, e - count);
                ((ns.row, ns.col), (ne.row, ne.col), false)
            } else {
                // fully inside → #REF!
                ((0, 0), (0, 0), true)
            }
        } else {
            // s < at
            if e >= at + count {
                let (ns, ne) = mk(s, e - count);
                ((ns.row, ns.col), (ne.row, ne.col), false)
            } else {
                // shrink: [s, at-1]
                let (ns, ne) = mk(s, at - 1);
                ((ns.row, ns.col), (ne.row, ne.col), false)
            }
        }
    }
}

/// Byte end of a `:ref2` range tail following a ref at `after` (or `after`
/// itself when there is no range) — used to copy foreign-sheet refs whole.
fn range_tail_end(chars: &[char], after: usize) -> usize {
    if after < chars.len()
        && chars[after] == ':'
        && after + 1 < chars.len()
        && chars[after + 1].is_ascii_alphabetic()
    {
        if let Some((len2, _)) = try_ref(chars, after + 1) {
            return after + 1 + len2;
        }
    }
    after
}

fn fmt_shifted_ref(cell: CellRef, kind: ShiftKind, at: u32, count: u32) -> String {
    let (row, col) = shift_ref(cell, kind, at, count);
    fmt_shifted(row, col)
}

fn fmt_shifted(row: u32, col: u32) -> String {
    if row == 0 || col == 0 {
        "#REF!".to_string()
    } else {
        format_ref(None, CellRef { row, col }, false)
    }
}

/// Format a shifted range. A `#REF!` (fully-deleted) range collapses to the
/// single token; otherwise both endpoints are formatted.
fn fmt_range(start: (u32, u32), end: (u32, u32), reff: bool) -> String {
    if reff || start.0 == 0 || start.1 == 0 || end.0 == 0 || end.1 == 0 {
        "#REF!".to_string()
    } else {
        format!(
            "{}:{}",
            format_ref(
                None,
                CellRef {
                    row: start.0,
                    col: start.1
                },
                false
            ),
            format_ref(
                None,
                CellRef {
                    row: end.0,
                    col: end.1
                },
                false
            )
        )
    }
}

// ────────────────────────────────────────────────────────────────────────
// Pivot (in-memory aggregation)
// ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PivotRow {
    pub key: String,
    pub value: f64,
    pub count: u64,
}

/// Aggregate a flat value grid (rows of cells from `read`/recalc) into a
/// grouped summary: group by column `group_by` (1-based within the range),
/// aggregate column `aggregate` with `agg`. Values that aren't numbers are
/// treated as 0 for Sum/Avg; text keys group as-is.
pub fn pivot_result(
    source: &[Vec<super::read::CellValue>],
    group_by: usize,
    aggregate: usize,
    agg: PivotAgg,
) -> Vec<PivotRow> {
    let mut out: Vec<PivotRow> = Vec::new();
    for row in source {
        let key = row.get(group_by).map(|v| v.display()).unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        let n = match row.get(aggregate) {
            Some(super::read::CellValue::Number(n)) => *n,
            _ => 0.0,
        };
        if let Some(existing) = out.iter_mut().find(|r| r.key == key) {
            existing.count += 1;
            existing.value = match agg {
                PivotAgg::Sum | PivotAgg::Avg => existing.value + n,
                PivotAgg::Count => existing.value,
            };
        } else {
            out.push(PivotRow {
                key,
                value: n,
                count: 1,
            });
        }
    }
    if agg == PivotAgg::Avg {
        for r in out.iter_mut() {
            if r.count > 0 {
                r.value /= r.count as f64;
            }
        }
    }
    if agg == PivotAgg::Count {
        for r in out.iter_mut() {
            r.value = r.count as f64;
        }
    }
    out
}

/// Parse "42", "3.5", "TRUE"/"FALSE", `"quoted"`, or bare text.
pub fn parse_scalar(s: &str) -> Scalar {
    let t = s.trim();
    if let Ok(n) = t.parse::<f64>() {
        return Scalar::Number(n);
    }
    let upper = t.to_uppercase();
    if upper == "TRUE" {
        return Scalar::Bool(true);
    }
    if upper == "FALSE" {
        return Scalar::Bool(false);
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return Scalar::Text(t[1..t.len() - 1].to_string());
    }
    Scalar::Text(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::address::parse_range;
    use super::*;

    fn s(f: &str, kind: ShiftKind, at: u32, count: u32) -> String {
        shift_formula(f, "Sheet1", kind, at, count)
    }

    #[test]
    fn insert_rows_move_refs() {
        // insert 2 rows at 3: A3 → A5, A1 stays
        assert_eq!(s("=A1+A3", ShiftKind::InsertRow, 3, 2), "=A1+A5");
        assert_eq!(
            s("=SUM(A1:A10)", ShiftKind::InsertRow, 3, 2),
            "=SUM(A1:A12)"
        );
        // range spanning the insert point extends
        assert_eq!(s("=SUM(A1:A2)", ShiftKind::InsertRow, 2, 1), "=SUM(A1:A3)");
    }

    #[test]
    fn insert_cols_move_refs() {
        assert_eq!(s("=B1", ShiftKind::InsertCol, 2, 1), "=C1");
        assert_eq!(s("=A1:B2", ShiftKind::InsertCol, 1, 1), "=B1:C2");
    }

    #[test]
    fn delete_rows_ref_errors_and_shifts() {
        // delete rows 3..4 (at=3, count=2)
        assert_eq!(s("=A3", ShiftKind::DeleteRow, 3, 2), "=#REF!");
        assert_eq!(s("=A4", ShiftKind::DeleteRow, 3, 2), "=#REF!");
        assert_eq!(s("=A5", ShiftKind::DeleteRow, 3, 2), "=A3");
        assert_eq!(s("=A1", ShiftKind::DeleteRow, 3, 2), "=A1");
        // range fully inside → #REF!
        assert_eq!(s("=SUM(A3:B4)", ShiftKind::DeleteRow, 3, 2), "=SUM(#REF!)");
        // range partially overlapping shrinks
        assert_eq!(s("=SUM(A2:B4)", ShiftKind::DeleteRow, 3, 2), "=SUM(A2:B2)");
        assert_eq!(s("=SUM(A3:B5)", ShiftKind::DeleteRow, 3, 2), "=SUM(A3:B3)");
    }

    #[test]
    fn delete_cols_symmetry() {
        assert_eq!(s("=C1", ShiftKind::DeleteCol, 2, 1), "=B1");
        assert_eq!(s("=B1", ShiftKind::DeleteCol, 2, 1), "=#REF!");
        assert_eq!(s("=A1:C1", ShiftKind::DeleteCol, 2, 1), "=A1:B1");
    }

    #[test]
    fn absolute_markers_do_not_pin() {
        assert_eq!(s("=$A$3", ShiftKind::InsertRow, 3, 2), "=A5");
        assert_eq!(s("=$A$1", ShiftKind::InsertRow, 3, 2), "=A1");
    }

    #[test]
    fn function_names_protected() {
        assert_eq!(s("=LOG10(A1)", ShiftKind::InsertRow, 1, 1), "=LOG10(A2)");
        assert_eq!(s("=LOG10(100)", ShiftKind::InsertRow, 1, 1), "=LOG10(100)");
        assert_eq!(
            s("=SUMIF(A1:A3,\">=10\")", ShiftKind::InsertRow, 1, 1),
            "=SUMIF(A2:A4,\">=10\")"
        );
    }

    #[test]
    fn string_literals_skipped() {
        assert_eq!(
            s("=IF(A1=\"A1\",A1)", ShiftKind::InsertRow, 1, 1),
            "=IF(A2=\"A1\",A2)"
        );
    }

    #[test]
    fn sheet_prefixes_scoped_to_target() {
        assert_eq!(s("=Sheet1!B2", ShiftKind::InsertRow, 2, 1), "=Sheet1!B3");
        assert_eq!(s("=Other!B2", ShiftKind::InsertRow, 2, 1), "=Other!B2");
        assert_eq!(
            s("='My Sheet'!B2", ShiftKind::InsertRow, 2, 1),
            "='My Sheet'!B2"
        );
        assert_eq!(
            s("=SUM('My Sheet'!A1:B2)", ShiftKind::InsertRow, 2, 1),
            "=SUM('My Sheet'!A1:B2)"
        );
        assert_eq!(
            s("=SUM(Sheet1!A1:B2)", ShiftKind::InsertRow, 2, 1),
            "=SUM(Sheet1!A1:B3)"
        );
    }

    #[test]
    fn named_range_untouched() {
        assert_eq!(s("=Budget*2", ShiftKind::InsertRow, 1, 1), "=Budget*2");
        assert_eq!(s("=RATE(10)", ShiftKind::InsertRow, 1, 1), "=RATE(10)");
    }

    #[test]
    fn batch_model_and_scalars() {
        let mut b = WorkbookCommandBatch::new(7, "set A1 to 42");
        b.operations.push(Operation::SetCell {
            address: CellRef { row: 1, col: 1 },
            value: Scalar::Number(42.0),
        });
        assert_eq!(b.dsl_version, 1);
        assert_eq!(b.base_revision, 7);
        assert_eq!(b.summary, "set A1 to 42");
        assert!(b.transaction_id.starts_with("txn-7-"));

        assert_eq!(parse_scalar("42"), Scalar::Number(42.0));
        assert_eq!(parse_scalar("3.5"), Scalar::Number(3.5));
        assert_eq!(parse_scalar("TRUE"), Scalar::Bool(true));
        assert_eq!(parse_scalar("\"hello\""), Scalar::Text("hello".to_string()));
        assert_eq!(
            parse_scalar("hello world"),
            Scalar::Text("hello world".to_string())
        );
    }

    #[test]
    fn pivot_groups_and_aggregates() {
        use super::super::read::CellValue;
        let grid = vec![
            vec![CellValue::Text("east".into()), CellValue::Number(10.0)],
            vec![CellValue::Text("west".into()), CellValue::Number(5.0)],
            vec![CellValue::Text("east".into()), CellValue::Number(20.0)],
            vec![CellValue::Text("north".into()), CellValue::Number(7.0)],
        ];
        let rows = pivot_result(&grid, 0, 1, PivotAgg::Sum);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            PivotRow {
                key: "east".into(),
                value: 30.0,
                count: 2
            }
        );
        let count = pivot_result(&grid, 0, 1, PivotAgg::Count);
        assert_eq!(count.iter().find(|r| r.key == "east").unwrap().value, 2.0);
        let avg = pivot_result(&grid, 0, 1, PivotAgg::Avg);
        assert_eq!(avg.iter().find(|r| r.key == "east").unwrap().value, 15.0);
    }

    #[test]
    fn range_parse_feeds_shift() {
        let (_, r) = parse_range("A2:B4").unwrap();
        assert_eq!(r.start, CellRef { row: 2, col: 1 });
        assert_eq!(r.end, CellRef { row: 4, col: 2 });
    }
}
