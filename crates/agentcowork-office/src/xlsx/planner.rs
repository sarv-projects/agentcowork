//! Deterministic planner (doc 28 §5 `deterministic-planner.ts` — the star):
//! regex-compiled natural-language → workbook DSL. **Zero tokens for the
//! common ops** (set, formula, rename, sort, fill, clear); anything the
//! regexes can't compile becomes `NeedsLLM` — the permission-gated,
//! audit-flagged fallback that hands the prompt to the model.

use super::address::{parse_range, parse_ref};
use super::dsl::{FillMode, Operation, PivotAgg, WorkbookCommandBatch};

#[derive(Debug, Clone, PartialEq)]
pub enum PlannerOutcome {
    Compiled(WorkbookCommandBatch),
    /// Regex DSL couldn't parse → LLM-direct path. `reason` explains why and
    /// `suggested` mirrors GenOffice's helpful "try one of these" message.
    NeedsLlm {
        reason: String,
        suggested: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannerError(pub String);

impl std::fmt::Display for PlannerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "planner error: {}", self.0)
    }
}

const SUGGESTED: &str = "Try 'set A1 to 42', 'formula B1 = SUM(A1:A10)', 'rename sheet to Budget', \
     'sort Sheet1 by column B descending', 'fill B2:B10 with 5', or 'clear A1:C20'.";

/// Compile a user prompt into a command batch. `base_revision` is the
/// optimistic-concurrency revision of the workbook the edit applies to.
pub fn plan_prompt(prompt: &str, base_revision: u64) -> PlannerOutcome {
    let p = prompt.trim();
    if p.is_empty() {
        return PlannerOutcome::NeedsLlm {
            reason: "empty prompt".to_string(),
            suggested: SUGGESTED.to_string(),
        };
    }

    // set A1 to 42 / set A1 to "text" / set A1 to TRUE
    if let Some(cap) = re_set().captures(p) {
        return compile_set(&cap[1], &cap[2], base_revision);
    }

    // formula B1 = SUM(A1:A10) / set formula B1 = ...
    if let Some(cap) = re_formula().captures(p) {
        return compile_formula(&cap[1], &cap[2], base_revision);
    }

    // rename sheet to Budget
    if let Some(cap) = re_rename().captures(p) {
        let (sheet, to) = (&cap[1], &cap[2]);
        return compile_rename(sheet, to, base_revision);
    }

    // sort Sheet1 by column B descending / sort A1:C10 by column 2 ascending
    if let Some(cap) = re_sort().captures(p) {
        return compile_sort(&cap[1], &cap[2], &cap[3], base_revision);
    }

    // fill B2:B10 with 5 / fill B2:B10 down (copy-down)
    if let Some(cap) = re_fill().captures(p) {
        let value = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let down = cap.get(3).map(|m| m.as_str()).unwrap_or("");
        return compile_fill(&cap[1], value, down, base_revision);
    }

    // clear A1:C20
    if let Some(cap) = re_clear().captures(p) {
        return compile_clear(&cap[1], base_revision);
    }

    // insert row at 5 / delete column C
    if let Some(cap) = re_shift().captures(p) {
        let count = cap.get(4).map(|m| m.as_str()).unwrap_or("");
        return compile_shift(&cap[1], &cap[2], &cap[3], count, base_revision);
    }

    // pivot A1:C100 by column 1 sum column 3
    if let Some(cap) = re_pivot().captures(p) {
        return compile_pivot(&cap[1], &cap[2], &cap[3], &cap[4], base_revision);
    }

    PlannerOutcome::NeedsLlm {
        reason: format!("no deterministic rule matched: {p:?}"),
        suggested: SUGGESTED.to_string(),
    }
}

fn re_set() -> regex::Regex {
    regex::Regex::new(r"(?i)^set\s+([A-Za-z]+[0-9]+)\s+to\s+(.+)$").unwrap()
}
fn re_formula() -> regex::Regex {
    regex::Regex::new(r"(?i)^(?:set\s+)?formula\s+([A-Za-z]+[0-9]+)\s*=\s*(.+)$").unwrap()
}
fn re_rename() -> regex::Regex {
    regex::Regex::new(r"(?i)^rename\s+(?:sheet\s+)?([A-Za-z0-9_ .'-]+)\s+to\s+(.+)$").unwrap()
}
fn re_sort() -> regex::Regex {
    regex::Regex::new(
        r"(?i)^sort\s+([A-Za-z0-9_ .'-]+(?:![A-Z]+[0-9]+:[A-Z]+[0-9]+)?|[A-Z]+[0-9]+:[A-Z]+[0-9]+)\s+by\s+column\s+([A-Z]+|[0-9]+)\s*(ascending|descending)?$",
    )
    .unwrap()
}
fn re_fill() -> regex::Regex {
    regex::Regex::new(r"(?i)^fill\s+([A-Z]+[0-9]+:[A-Z]+[0-9]+)\s+(?:with\s+(.+)|(down))$").unwrap()
}
fn re_clear() -> regex::Regex {
    regex::Regex::new(r"(?i)^clear\s+([A-Z]+[0-9]+:[A-Z]+[0-9]+)$").unwrap()
}
fn re_shift() -> regex::Regex {
    regex::Regex::new(
        r"(?i)^(insert|delete)\s+(row|column)\s+(?:at\s+)?([A-Za-z0-9]+)(?:\s+(\d+))?$",
    )
    .unwrap()
}
fn re_pivot() -> regex::Regex {
    regex::Regex::new(
        r"(?i)^pivot\s+([A-Z]+[0-9]+:[A-Z]+[0-9]+)\s+by\s+column\s+([0-9]+)\s+(sum|count|avg)\s+column\s+([0-9]+)$",
    )
    .unwrap()
}

fn compile_set(address: &str, value: &str, rev: u64) -> PlannerOutcome {
    let (_, cell) = match parse_ref(address) {
        Ok(c) => c,
        Err(e) => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("bad cell address {address:?}: {e}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    let mut batch = WorkbookCommandBatch::new(rev, format!("set {address} to {value}"));
    batch.operations.push(Operation::SetCell {
        address: cell,
        value: super::dsl::parse_scalar(value),
    });
    PlannerOutcome::Compiled(batch)
}

fn compile_formula(address: &str, formula: &str, rev: u64) -> PlannerOutcome {
    let (_, cell) = match parse_ref(address) {
        Ok(c) => c,
        Err(e) => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("bad cell address {address:?}: {e}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    let formula = formula.trim();
    let f = if formula.starts_with('=') {
        formula.to_string()
    } else {
        format!("={formula}")
    };
    let mut batch = WorkbookCommandBatch::new(rev, format!("formula {address} = {formula}"));
    batch.operations.push(Operation::SetFormula {
        address: cell,
        formula: f,
    });
    PlannerOutcome::Compiled(batch)
}

fn compile_rename(from: &str, to: &str, rev: u64) -> PlannerOutcome {
    let mut batch = WorkbookCommandBatch::new(rev, format!("rename sheet {from} to {to}"));
    batch.operations.push(Operation::RenameSheet {
        from: from.trim().to_string(),
        to: to.trim().to_string(),
    });
    PlannerOutcome::Compiled(batch)
}

fn compile_sort(target: &str, by_col: &str, dir: &str, rev: u64) -> PlannerOutcome {
    let (sheet, range) = match parse_range(target) {
        Ok(r) => r,
        Err(e) => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("bad sort range {target:?}: {e}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    // by_col may be a column letter (B) or a 1-based index (2)
    let by_col = match super::address::col_index(by_col) {
        Some(n) => n,
        None => match by_col.parse::<u32>() {
            Ok(n) if n >= 1 => n,
            _ => {
                return PlannerOutcome::NeedsLlm {
                    reason: format!("bad sort column {by_col:?}"),
                    suggested: SUGGESTED.to_string(),
                };
            }
        },
    };
    let desc = dir.eq_ignore_ascii_case("descending");
    let mut batch = WorkbookCommandBatch::new(rev, format!("sort {target} by column {by_col}"));
    batch.operations.push(Operation::SortRange {
        range,
        by_col,
        desc,
    });
    let _ = sheet;
    PlannerOutcome::Compiled(batch)
}

fn compile_fill(range: &str, value: &str, down: &str, rev: u64) -> PlannerOutcome {
    let (_, range) = match parse_range(range) {
        Ok(r) => r,
        Err(e) => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("bad fill range {range:?}: {e}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    let (mode, scalar) = if !down.is_empty() {
        (FillMode::CopyDown, None)
    } else {
        (FillMode::Constant, Some(super::dsl::parse_scalar(value)))
    };
    let mut batch = WorkbookCommandBatch::new(rev, format!("fill {range}"));
    batch.operations.push(Operation::FillRange {
        range,
        mode,
        value: scalar,
    });
    PlannerOutcome::Compiled(batch)
}

fn compile_clear(range: &str, rev: u64) -> PlannerOutcome {
    let (_, range) = match parse_range(range) {
        Ok(r) => r,
        Err(e) => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("bad clear range {range:?}: {e}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    let mut batch = WorkbookCommandBatch::new(rev, format!("clear {range}"));
    batch.operations.push(Operation::ClearRange { range });
    PlannerOutcome::Compiled(batch)
}

fn compile_shift(verb: &str, axis: &str, target: &str, count: &str, rev: u64) -> PlannerOutcome {
    let is_insert = verb.eq_ignore_ascii_case("insert");
    let is_row = axis.eq_ignore_ascii_case("row");
    let at = if is_row {
        target.parse::<u32>().ok()
    } else {
        super::address::col_index(target)
    };
    let Some(at) = at.filter(|a| *a >= 1) else {
        return PlannerOutcome::NeedsLlm {
            reason: format!("bad {axis} position {target:?}"),
            suggested: SUGGESTED.to_string(),
        };
    };
    let count = count.parse::<u32>().unwrap_or(1).max(1);
    let kind = match (is_insert, is_row) {
        (true, true) => super::dsl::ShiftKind::InsertRow,
        (true, false) => super::dsl::ShiftKind::InsertCol,
        (false, true) => super::dsl::ShiftKind::DeleteRow,
        (false, false) => super::dsl::ShiftKind::DeleteCol,
    };
    let mut batch = WorkbookCommandBatch::new(rev, format!("{verb} {axis} at {target}"));
    batch.operations.push(Operation::Shift {
        sheet: "Sheet1".to_string(),
        kind,
        at,
        count,
    });
    PlannerOutcome::Compiled(batch)
}

fn compile_pivot(
    range: &str,
    group_by: &str,
    agg: &str,
    aggregate: &str,
    rev: u64,
) -> PlannerOutcome {
    let (_, range) = match parse_range(range) {
        Ok(r) => r,
        Err(e) => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("bad pivot range {range:?}: {e}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    let (Ok(group_by), Ok(aggregate)) = (
        group_by.parse::<u32>().map(|n| n - 1),
        aggregate.parse::<u32>().map(|n| n - 1),
    ) else {
        return PlannerOutcome::NeedsLlm {
            reason: "pivot columns must be positive numbers".to_string(),
            suggested: SUGGESTED.to_string(),
        };
    };
    let agg = match agg.to_lowercase().as_str() {
        "sum" => PivotAgg::Sum,
        "count" => PivotAgg::Count,
        "avg" => PivotAgg::Avg,
        _ => {
            return PlannerOutcome::NeedsLlm {
                reason: format!("unknown pivot aggregate {agg:?}"),
                suggested: SUGGESTED.to_string(),
            };
        }
    };
    let mut batch = WorkbookCommandBatch::new(
        rev,
        format!("pivot {range} by {group_by} {agg} {aggregate}"),
    );
    batch.operations.push(Operation::Pivot {
        source: range,
        group_by,
        aggregate,
        agg,
    });
    PlannerOutcome::Compiled(batch)
}

#[cfg(test)]
mod tests {
    use super::super::address::CellRef;
    use super::super::dsl::{Operation, Scalar, ShiftKind};
    use super::*;

    #[test]
    fn plans_set_cell() {
        match plan_prompt("set A1 to 42", 3) {
            PlannerOutcome::Compiled(b) => {
                assert_eq!(b.base_revision, 3);
                assert_eq!(b.operations.len(), 1);
                match &b.operations[0] {
                    Operation::SetCell { address, value } => {
                        assert_eq!(*address, CellRef { row: 1, col: 1 });
                        assert_eq!(*value, Scalar::Number(42.0));
                    }
                    other => panic!("expected SetCell, got {other:?}"),
                }
            }
            other => panic!("expected Compiled, got {other:?}"),
        }
        match plan_prompt("set B2 to \"hello\"", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::SetCell { address, value } => {
                    assert_eq!(*address, CellRef { row: 2, col: 2 });
                    assert_eq!(*value, Scalar::Text("hello".into()));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plans_formula() {
        match plan_prompt("formula B1 = SUM(A1:A10)", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::SetFormula { address, formula } => {
                    assert_eq!(*address, CellRef { row: 1, col: 2 });
                    assert_eq!(formula, "=SUM(A1:A10)");
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        // without leading '=' too
        match plan_prompt("formula C3 = IF(A1>1,\"y\",\"n\")", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::SetFormula { formula, .. } => {
                    assert!(formula.starts_with('='));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plans_rename_and_sort() {
        match plan_prompt("rename sheet to Budget", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::RenameSheet { from, to } => {
                    assert_eq!(to, "Budget");
                    let _ = from;
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match plan_prompt("sort A1:C10 by column B descending", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::SortRange { by_col, desc, .. } => {
                    assert_eq!(*by_col, 2);
                    assert!(*desc);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match plan_prompt("sort Sheet1!A1:C10 by column 2 ascending", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::SortRange { by_col, desc, .. } => {
                    assert_eq!(*by_col, 2);
                    assert!(!*desc);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plans_fill_clear_shift() {
        match plan_prompt("fill B2:B10 with 5", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::FillRange { mode, value, .. } => {
                    assert_eq!(*mode, FillMode::Constant);
                    assert_eq!(*value, Some(Scalar::Number(5.0)));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match plan_prompt("fill B2:B10 down", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::FillRange { mode, .. } => {
                    assert_eq!(*mode, FillMode::CopyDown);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match plan_prompt("clear A1:C20", 0) {
            PlannerOutcome::Compiled(b) => {
                assert!(matches!(b.operations[0], Operation::ClearRange { .. }))
            }
            other => panic!("{other:?}"),
        }
        match plan_prompt("insert row at 5", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::Shift {
                    kind, at, count, ..
                } => {
                    assert_eq!(*kind, ShiftKind::InsertRow);
                    assert_eq!(*at, 5);
                    assert_eq!(*count, 1);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match plan_prompt("delete column C", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::Shift { kind, at, .. } => {
                    assert_eq!(*kind, ShiftKind::DeleteCol);
                    assert_eq!(*at, 3);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plans_pivot() {
        match plan_prompt("pivot A1:C100 by column 1 sum column 3", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::Pivot {
                    group_by,
                    aggregate,
                    agg,
                    ..
                } => {
                    assert_eq!(*group_by, 0);
                    assert_eq!(*aggregate, 2);
                    assert_eq!(*agg, PivotAgg::Sum);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unsupported_prompts_fall_back_to_llm() {
        for p in [
            "what's the total of column A?",
            "make a chart of A1:B10",
            "",
            "delete the whole sheet",
            "bold the header row",
        ] {
            match plan_prompt(p, 0) {
                PlannerOutcome::NeedsLlm { reason, suggested } => {
                    assert!(!reason.is_empty());
                    assert!(suggested.contains("set A1 to 42"));
                }
                other => panic!("expected NeedsLlm for {p:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn greedy_set_value_compiles_as_text() {
        // The value group is intentionally greedy (GenOffice parseScalar
        // shape): non-numeric values become text.
        match plan_prompt("set A1 to 42 please kindly", 0) {
            PlannerOutcome::Compiled(b) => match &b.operations[0] {
                Operation::SetCell { value, .. } => {
                    assert_eq!(*value, Scalar::Text("42 please kindly".into()));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }
}
