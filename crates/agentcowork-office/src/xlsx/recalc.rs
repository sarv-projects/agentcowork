//! IronCalc-backed recalculation — the **100% math-integrity truth engine**
//! (P4.2 / ARCH/04: numeric claims go through IronCalc, never the LLM).
//!
//! The LLM only reads/writes values + formulas; every computed number comes
//! from `recalc()` below. We deliberately do NOT write IronCalc's
//! `save_to_xlsx` back to the user's file (that is a full re-serialize and
//! would violate byte-preservation) — we extract the computed values and
//! the surgical patch writes only the changed cells.

use ironcalc::base::{Model, cell::CellValue as IcValue};
use ironcalc::import::load_from_xlsx_bytes;
use serde::{Deserialize, Serialize};

use super::read::CellValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalcCell {
    /// 1-based.
    pub row: u32,
    /// 1-based.
    pub col: u32,
    pub value: CellValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetValues {
    pub name: String,
    pub cells: Vec<RecalcCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecalcResult {
    pub sheets: Vec<SheetValues>,
    /// Number of formula cells evaluated (reported for audit).
    pub formula_cells: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecalcError(pub String);

impl std::fmt::Display for RecalcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "recalc failed: {}", self.0)
    }
}

/// Load an xlsx from bytes, run the full recalculation, and extract every
/// non-empty computed value. All numbers here are engine-computed.
pub fn recalc(bytes: &[u8]) -> Result<RecalcResult, RecalcError> {
    let workbook = load_from_xlsx_bytes(bytes, "workbook.xlsx", "en", "UTC")
        .map_err(|e| RecalcError(e.to_string()))?;
    let mut model = Model::from_workbook(workbook, "en").map_err(RecalcError)?;
    model.evaluate();

    let mut result = RecalcResult::default();
    let sheet_props = model.get_worksheets_properties();
    for (idx, props) in sheet_props.iter().enumerate() {
        let name = props.name.clone();
        let mut cells = Vec::new();
        for cell in model.get_all_cells() {
            if cell.index as usize != idx {
                continue;
            }
            let row = cell.row as u32;
            let col = cell.column as u32;
            // get_all_cells row/column are 1-based; guard any 0 from the engine.
            if row == 0 || col == 0 {
                continue;
            }
            let v = model
                .get_cell_value_by_index(cell.index, cell.row, cell.column)
                .map_err(RecalcError)?;
            if is_empty(&v) {
                continue;
            }
            // Formula cells: recalc must have produced a value.
            if matches!(
                model.get_cell_formula(cell.index, cell.row, cell.column),
                Ok(Some(_))
            ) {
                result.formula_cells += 1;
            }
            cells.push(RecalcCell {
                row,
                col,
                value: from_ironcalc(&v),
            });
        }
        result.sheets.push(SheetValues { name, cells });
    }
    Ok(result)
}

fn is_empty(v: &IcValue) -> bool {
    matches!(v, IcValue::None)
}

fn from_ironcalc(v: &IcValue) -> CellValue {
    match v {
        IcValue::None => CellValue::Empty,
        IcValue::String(s) => CellValue::Text(s.clone()),
        IcValue::Number(n) => CellValue::Number(*n),
        IcValue::Boolean(b) => CellValue::Bool(*b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal hand-written xlsx: one sheet with a few literals + formulas.
    /// Content-types, workbook, and a sharedStrings part are the minimum
    /// IronCalc's importer needs to load it.
    fn minimal_xlsx(sheet_xml: &str) -> Vec<u8> {
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
        let ss = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="3" uniqueCount="3">
  <si><t>Alpha</t></si><si><t>Beta</t></si><si><t>Gamma</t></si>
</sst>"#;
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
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        for (name, content) in [
            ("[Content_Types].xml", ct),
            ("_rels/.rels", rels),
            ("xl/workbook.xml", wb),
            ("xl/_rels/workbook.xml.rels", wb_rels),
            ("xl/worksheets/sheet1.xml", sheet_xml),
            ("xl/sharedStrings.xml", ss),
            ("xl/styles.xml", styles),
        ] {
            zip.start_file(name, SimpleFileOptions::default())
                .expect("start_file");
            zip.write_all(content.as_bytes()).expect("write");
        }
        zip.finish().expect("finish").into_inner()
    }

    #[test]
    fn recalc_sum_vlookup_if_countif() {
        let sheet = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>10</v></c><c r="B1"><v>20</v></c><c r="C1"><v>30</v></c></row><row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>2</v></c><c r="C2"><v>3</v></c></row><row r="3"><c r="A3"><f>SUM(A1:A2)</f><v>0</v></c></row><row r="4"><c r="A4"><f>IF(B2&gt;1,"yes","no")</f><v>0</v></c></row><row r="5"><c r="A5"><f>COUNTIF(A1:B2,"&gt;5")</f><v>0</v></c></row><row r="6"><c r="A6"><f>VLOOKUP(10,A1:B2,2,FALSE)</f><v>0</v></c></row></sheetData></worksheet>"#;
        let bytes = minimal_xlsx(sheet);
        let res = recalc(&bytes).expect("recalc");
        assert_eq!(res.sheets.len(), 1);
        assert_eq!(res.sheets[0].name, "Sheet1");
        assert_eq!(res.formula_cells, 4);

        let get = |row: u32, col: u32| -> CellValue {
            res.sheets[0]
                .cells
                .iter()
                .find(|c| c.row == row && c.col == col)
                .map(|c| c.value.clone())
                .unwrap_or(CellValue::Empty)
        };
        // SUM(A1:A2) = 10 + 1 = 11
        assert_eq!(get(3, 1), CellValue::Number(11.0));
        // IF(B2>1,"yes","no") = "yes"
        assert_eq!(get(4, 1), CellValue::Text("yes".to_string()));
        // COUNTIF(A1:B2,">5") → 10 and 20 and 30? range is A1:B2 = 10,20,1,2 → 2
        assert_eq!(get(5, 1), CellValue::Number(2.0));
        // VLOOKUP(10, A1:B2, 2, FALSE) → lookup 10 in A1:A2 → row 1, col 2 = 20
        assert_eq!(get(6, 1), CellValue::Number(20.0));
        // VLOOKUP of a missing key → #N/A (honest error propagation)
        let sheet2 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>10</v></c><c r="B1"><v>20</v></c></row><row r="2"><c r="A2"><f>VLOOKUP(99,A1:B1,2,FALSE)</f><v>0</v></c></row></sheetData></worksheet>"#;
        let res2 = recalc(&minimal_xlsx(sheet2)).expect("recalc 2");
        let v = res2.sheets[0]
            .cells
            .iter()
            .find(|c| c.row == 2 && c.col == 1)
            .unwrap();
        assert_eq!(v.value, CellValue::Text("#N/A".to_string()));
    }

    #[test]
    fn recalc_text_and_bool_cells() {
        let sheet = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="b"><v>1</v></c></row></sheetData></worksheet>"#;
        let bytes = minimal_xlsx(sheet);
        let res = recalc(&bytes).expect("recalc");
        let a1 = res.sheets[0]
            .cells
            .iter()
            .find(|c| c.row == 1 && c.col == 1)
            .unwrap();
        assert_eq!(a1.value, CellValue::Text("Alpha".to_string()));
        let b1 = res.sheets[0]
            .cells
            .iter()
            .find(|c| c.row == 1 && c.col == 2)
            .unwrap();
        assert_eq!(b1.value, CellValue::Bool(true));
    }

    #[test]
    fn broken_bytes_rejected() {
        assert!(recalc(b"not a zip").is_err());
    }
}
