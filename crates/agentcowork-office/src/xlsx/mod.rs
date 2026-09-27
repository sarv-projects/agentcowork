//! Excel engine (P4.2, D2 — doc 28 §4–5, ARCH/04 §Excel).
//!
//! Layering (the doc-58 Univer split): `read` (calamine) + `recalc`
//! (IronCalc) + `dsl`/`planner` (the deterministic command language) +
//! `patch` (surgical `sheetN.xml`/`sharedStrings.xml` writes). The LLM only
//! reads/writes values + formulas; every computed number comes from the
//! IronCalc truth engine (100% math integrity).

pub mod address;
pub mod chart;
pub mod dsl;
pub mod patch;
pub mod planner;
pub mod read;
pub mod recalc;
