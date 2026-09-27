//! SCIP-style symbol index + queries (I11 — doc 63 §4.6, crux pattern:
//! 66%→96% accuracy / 24% fewer tokens via `symbol_where`/`symbol_callers`/
//! `unused_exports`).
//!
//! The index is JSON-typed here (SCIP protobuf ingestion can layer on top);
//! the query logic is the crux value: compact text answers over a symbol
//! index, grouped references, spawn-only (never "run all and fuse").

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Type,
    Variable,
    Module,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceRole {
    Definition,
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Call,
    Implements,
    References,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolOccurrence {
    pub symbol: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub role: OccurrenceRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub source: String,
    pub target: String,
    pub kind: RelationKind,
}

/// The symbol index (the SCIP Index's query-facing projection).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticIndex {
    pub symbols: Vec<Symbol>,
    pub occurrences: Vec<SymbolOccurrence>,
    pub relationships: Vec<Relationship>,
}

impl SemanticIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// `symbol_where` — every occurrence (definition + references) of a
    /// symbol, grouped, as `file:line:col` rows.
    pub fn symbol_where(&self, symbol: &str) -> Vec<&SymbolOccurrence> {
        self.occurrences
            .iter()
            .filter(|o| o.symbol == symbol)
            .collect()
    }

    /// `symbol_callers` — symbols that call `symbol` (Call relationship where
    /// `symbol` is the target).
    pub fn symbol_callers(&self, symbol: &str) -> Vec<&Symbol> {
        let mut callers: Vec<&Symbol> = self
            .relationships
            .iter()
            .filter(|r| r.target == symbol && r.kind == RelationKind::Call)
            .filter_map(|r| self.symbol(&r.source))
            .collect();
        callers.sort_by_key(|s| s.name.as_str());
        callers.dedup_by_key(|s| s.name.as_str());
        callers
    }

    /// `unused_exports` — symbols with at least one definition and zero
    /// references (dead code candidates).
    pub fn unused_exports(&self) -> Vec<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| {
                let has_definition = self
                    .occurrences
                    .iter()
                    .any(|o| o.symbol == s.name && o.role == OccurrenceRole::Definition);
                let has_reference = self
                    .occurrences
                    .iter()
                    .any(|o| o.symbol == s.name && o.role == OccurrenceRole::Reference);
                has_definition && !has_reference
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> SemanticIndex {
        SemanticIndex {
            symbols: vec![
                Symbol {
                    name: "main".into(),
                    kind: SymbolKind::Function,
                    language: "rust".into(),
                },
                Symbol {
                    name: "helper".into(),
                    kind: SymbolKind::Function,
                    language: "rust".into(),
                },
                Symbol {
                    name: "dead_code".into(),
                    kind: SymbolKind::Function,
                    language: "rust".into(),
                },
            ],
            occurrences: vec![
                SymbolOccurrence {
                    symbol: "main".into(),
                    file: "src/main.rs".into(),
                    line: 1,
                    column: 0,
                    role: OccurrenceRole::Definition,
                },
                SymbolOccurrence {
                    symbol: "helper".into(),
                    file: "src/main.rs".into(),
                    line: 5,
                    column: 0,
                    role: OccurrenceRole::Definition,
                },
                SymbolOccurrence {
                    symbol: "helper".into(),
                    file: "src/main.rs".into(),
                    line: 2,
                    column: 4,
                    role: OccurrenceRole::Reference,
                },
                SymbolOccurrence {
                    symbol: "dead_code".into(),
                    file: "src/lib.rs".into(),
                    line: 9,
                    column: 0,
                    role: OccurrenceRole::Definition,
                },
            ],
            relationships: vec![Relationship {
                source: "main".into(),
                target: "helper".into(),
                kind: RelationKind::Call,
            }],
        }
    }

    #[test]
    fn symbol_where_returns_all_occurrences() {
        let idx = index();
        let occ = idx.symbol_where("helper");
        assert_eq!(occ.len(), 2); // definition + reference
        assert!(occ.iter().all(|o| o.symbol == "helper"));
    }

    #[test]
    fn symbol_callers_finds_main() {
        let idx = index();
        let callers = idx.symbol_callers("helper");
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "main");
    }

    #[test]
    fn unused_exports_finds_dead_code() {
        let idx = index();
        let unused = idx.unused_exports();
        // `main` (entry point) and `dead_code` both have a definition and no
        // references in this fixture, so both are flagged; the query's job is
        // to surface the dead-code candidate.
        assert!(unused.iter().any(|s| s.name == "dead_code"));
    }

    #[test]
    fn empty_index_is_safe() {
        let idx = SemanticIndex::new();
        assert!(idx.symbol_where("x").is_empty());
        assert!(idx.symbol_callers("x").is_empty());
        assert!(idx.unused_exports().is_empty());
    }

    #[test]
    fn index_roundtrips_json() {
        let idx = index();
        let json = serde_json::to_string(&idx).unwrap();
        let back: SemanticIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(back, idx);
    }
}
