//! SCIP protobuf ingestion (I11 — doc 63 §4.6): a minimal, dependency-free
//! reader for the SCIP `Index` wire format, decoding a `Document` (language,
//! relative path, symbols, occurrences) straight into the [`SemanticIndex`]
//! the `symbol_where`/`symbol_callers`/`unused_exports` queries run over.
//!
//! SCIP is protobuf; instead of pulling in a protobuf runtime, this decodes
//! the wire format directly (varints, tags, length-delimited fields) for the
//! subset of messages the index projection needs. Unknown fields are skipped;
//! unknown enum values map to `Unknown`.

use crate::semantic::{
    OccurrenceRole, RelationKind, Relationship, SemanticIndex, Symbol, SymbolKind, SymbolOccurrence,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScipError {
    #[error("truncated protobuf buffer at offset {0}")]
    Truncated(usize),
    #[error("invalid varint at offset {0}")]
    InvalidVarint(usize),
    #[error("invalid wire type {0} at offset {1}")]
    InvalidWireType(u8, usize),
}

/// A decoded SCIP `Document`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipDocument {
    pub language: String,
    pub relative_path: String,
    pub symbols: Vec<ScipSymbol>,
    pub occurrences: Vec<ScipOccurrence>,
}

/// A decoded SCIP `SymbolInformation` (the fields the index needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipSymbol {
    /// The SCIP symbol string (`scip-rust . pkg path "fn name"`).
    pub symbol: String,
    pub kind: SymbolKind,
    pub display_name: String,
    pub relationships: Vec<ScipRelationship>,
}

/// A decoded SCIP `Relationship`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipRelationship {
    pub symbol: String,
    pub kind: RelationKind,
}

/// A decoded SCIP `Occurrence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipOccurrence {
    /// `[start_line, start_char, end_line, end_char]`.
    pub range: Vec<i32>,
    pub symbol: String,
    pub symbol_roles: u32,
}

// ---------------------------------------------------------------------------
// Protobuf wire primitives (varint / tag / length-delimited)
// ---------------------------------------------------------------------------

fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64, ScipError> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *buf.get(*pos).ok_or(ScipError::Truncated(*pos))?;
        *pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(ScipError::InvalidVarint(*pos));
        }
    }
}

/// Read one field: returns `(field_number, wire_type, payload_start, len)`.
/// `len` is the byte length of the payload (varint payloads are `Some(1..=10)`
/// — the caller re-reads the varint itself).
struct Field {
    number: u32,
    wire_type: u8,
    /// For length-delimited: the payload byte range. For varint: the value.
    start: usize,
    value: u64,
}

fn read_field(buf: &[u8], pos: &mut usize) -> Result<Field, ScipError> {
    let tag = read_varint(buf, pos)?;
    let number = (tag >> 3) as u32;
    let wire_type = (tag & 0x7) as u8;
    match wire_type {
        0 => {
            let value = read_varint(buf, pos)?;
            Ok(Field {
                number,
                wire_type,
                start: *pos,
                value,
            })
        }
        2 => {
            let len = read_varint(buf, pos)? as usize;
            let start = *pos;
            *pos += len;
            if *pos > buf.len() {
                return Err(ScipError::Truncated(start));
            }
            Ok(Field {
                number,
                wire_type,
                start,
                value: len as u64,
            })
        }
        other => Err(ScipError::InvalidWireType(other, *pos)),
    }
}

fn bytes_of<'a>(buf: &'a [u8], f: &Field) -> &'a [u8] {
    &buf[f.start..f.start + f.value as usize]
}

/// Read a length-delimited string field's content.
fn string_of<'a>(buf: &'a [u8], f: &Field) -> &'a str {
    std::str::from_utf8(bytes_of(buf, f)).unwrap_or("")
}

// ---------------------------------------------------------------------------
// SCIP field numbers (from scip.proto)
// ---------------------------------------------------------------------------
// Document { 1: language, 2: relative_path, 3: repeated SymbolInformation
//            symbols, 4: repeated Occurrence occurrences, 5: Metadata }
// SymbolInformation { 1: symbol, 2: enum SymbolKind, 3: display_name,
//                     4: Document* documentation (repeated), 5: enum
//                     Language, 6: repeated Relationship, 7: kind_enum }
// Relationship { 1: symbol, 2: enum RelationshipKind }
// Occurrence { 1: repeated int32 range (packed), 2: symbol, 3: int32
//              symbol_roles, 4: override_documentation, 5: diagnostics }

/// Parse one SCIP `Document` from protobuf bytes.
pub fn parse_document(buf: &[u8]) -> Result<ScipDocument, ScipError> {
    let mut language = String::new();
    let mut relative_path = String::new();
    let mut symbols = Vec::new();
    let mut occurrences = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let f = read_field(buf, &mut pos)?;
        match (f.number, f.wire_type) {
            (1, 2) => language = string_of(buf, &f).to_string(),
            (2, 2) => relative_path = string_of(buf, &f).to_string(),
            (3, 2) => symbols.push(parse_symbol(bytes_of(buf, &f))?),
            (4, 2) => occurrences.push(parse_occurrence(bytes_of(buf, &f))?),
            _ => {} // unknown field — skip (payload already consumed)
        }
    }
    Ok(ScipDocument {
        language,
        relative_path,
        symbols,
        occurrences,
    })
}

fn parse_symbol(buf: &[u8]) -> Result<ScipSymbol, ScipError> {
    let mut symbol = String::new();
    let mut kind = SymbolKind::Unknown;
    let mut display_name = String::new();
    let mut relationships = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let f = read_field(buf, &mut pos)?;
        match (f.number, f.wire_type) {
            (1, 2) => symbol = string_of(buf, &f).to_string(),
            (2, 0) => kind = symbol_kind(f.value as i32),
            (3, 2) => display_name = string_of(buf, &f).to_string(),
            (6, 2) => relationships.push(parse_relationship(bytes_of(buf, &f))?),
            _ => {}
        }
    }
    Ok(ScipSymbol {
        symbol,
        kind,
        display_name,
        relationships,
    })
}

fn parse_relationship(buf: &[u8]) -> Result<ScipRelationship, ScipError> {
    let mut symbol = String::new();
    let mut kind = RelationKind::References;
    let mut pos = 0usize;
    while pos < buf.len() {
        let f = read_field(buf, &mut pos)?;
        match (f.number, f.wire_type) {
            (1, 2) => symbol = string_of(buf, &f).to_string(),
            (2, 0) => kind = relation_kind(f.value as i32),
            _ => {}
        }
    }
    Ok(ScipRelationship { symbol, kind })
}

fn parse_occurrence(buf: &[u8]) -> Result<ScipOccurrence, ScipError> {
    let mut range = Vec::new();
    let mut symbol = String::new();
    let mut symbol_roles = 0u32;
    let mut pos = 0usize;
    while pos < buf.len() {
        let f = read_field(buf, &mut pos)?;
        match (f.number, f.wire_type) {
            // range is `repeated int32` — decoders emit packed (wire type 2).
            (1, 2) => {
                let mut p = f.start;
                let end = f.start + f.value as usize;
                while p < end {
                    let v = read_varint(buf, &mut p)? as i32;
                    range.push(v);
                }
            }
            (1, 0) => range.push(f.value as i32),
            (2, 2) => symbol = string_of(buf, &f).to_string(),
            (3, 0) => symbol_roles = f.value as u32,
            _ => {}
        }
    }
    Ok(ScipOccurrence {
        range,
        symbol,
        symbol_roles,
    })
}

/// SCIP `SymbolKind` enum → our projection (SCIP field 2; unknown maps to
/// `Unknown` so a future SCIP release never breaks the reader).
fn symbol_kind(v: i32) -> SymbolKind {
    match v {
        1 => SymbolKind::Function,
        2 => SymbolKind::Method,
        3 | 4 => SymbolKind::Type, // Class / Interface
        5 => SymbolKind::Variable,
        7 => SymbolKind::Module, // Package/Namespace fall into Module
        _ => SymbolKind::Unknown,
    }
}

/// SCIP `RelationshipKind` enum → our projection. SCIP's `Call` is value 2
/// (0 = Unspecified, 1 = Definition, 2 = Implementation, 3 = Reference,
/// 4 = TypeDefinition, 5 = Termination, 6 = Assignment, 7 = Inheritance,
/// 8 = Override, 9 = TypeAlias, 10 = TypeInstantiation, 11 = Dispatch).
fn relation_kind(v: i32) -> RelationKind {
    match v {
        2 | 4 | 5 => RelationKind::Implements, // Implementation / TypeDefinition / Termination
        3 | 6 | 7 | 8 => RelationKind::References, // Reference / Assignment / Inheritance / Override
        _ => RelationKind::References,
    }
}

/// Convert a decoded SCIP document into the query-facing [`SemanticIndex`].
/// Symbol names keep their full SCIP form; the display name is used when the
/// SCIP symbol string is empty.
pub fn to_semantic_index(doc: &ScipDocument) -> SemanticIndex {
    let mut index = SemanticIndex::new();
    let mut name_by_symbol: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::new();
    for s in &doc.symbols {
        name_by_symbol.insert(s.symbol.as_str(), s.display_name.as_str());
        index.symbols.push(Symbol {
            name: s.symbol.clone(),
            kind: s.kind,
            language: doc.language.clone(),
        });
        for rel in &s.relationships {
            index.relationships.push(Relationship {
                source: s.symbol.clone(),
                target: rel.symbol.clone(),
                kind: rel.kind,
            });
        }
    }
    for o in &doc.occurrences {
        let line = o.range.first().copied().unwrap_or(0).max(0) as u32;
        let column = o.range.get(1).copied().unwrap_or(0).max(0) as u32;
        let role = if o.symbol_roles & 1 != 0 {
            OccurrenceRole::Definition
        } else {
            OccurrenceRole::Reference
        };
        // Skip anonymous/unresolved occurrences (empty symbol).
        if o.symbol.is_empty() {
            continue;
        }
        index.occurrences.push(SymbolOccurrence {
            symbol: o.symbol.clone(),
            file: doc.relative_path.clone(),
            line,
            column,
            role,
        });
    }
    let _ = name_by_symbol; // display names are a later enrichment
    index
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::SymbolOccurrence;

    /// Minimal protobuf writer for building test fixtures (varint fields +
    /// length-delimited strings/messages). This is the *encoding* side used to
    /// prove the decoder round-trips; the decoder itself never depends on it.
    struct Encoder {
        out: Vec<u8>,
    }

    impl Encoder {
        fn new() -> Self {
            Self { out: Vec::new() }
        }
        fn varint(&mut self, v: u64) {
            let mut v = v;
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                self.out.push(byte);
                if v == 0 {
                    break;
                }
            }
        }
        fn tag(&mut self, number: u32, wire_type: u8) {
            self.varint((u64::from(number) << 3) | u64::from(wire_type));
        }
        fn string_field(&mut self, number: u32, s: &str) {
            self.tag(number, 2);
            self.varint(s.len() as u64);
            self.out.extend_from_slice(s.as_bytes());
        }
        fn varint_field(&mut self, number: u32, v: u64) {
            self.tag(number, 0);
            self.varint(v);
        }
        fn message_field(&mut self, number: u32, inner: &[u8]) {
            self.tag(number, 2);
            self.varint(inner.len() as u64);
            self.out.extend_from_slice(inner);
        }
        fn packed_field(&mut self, number: u32, values: &[i32]) {
            let mut payload = Vec::new();
            for v in values {
                let mut e = Encoder::new();
                e.varint(*v as u64);
                payload.extend_from_slice(&e.out);
            }
            self.message_field(number, &payload);
        }
        fn finish(self) -> Vec<u8> {
            self.out
        }
    }

    fn symbol_msg(symbol: &str, kind: i32, display: &str) -> Vec<u8> {
        let mut e = Encoder::new();
        e.string_field(1, symbol);
        e.varint_field(2, kind as u64);
        e.string_field(3, display);
        e.finish()
    }

    fn document_bytes() -> Vec<u8> {
        let mut e = Encoder::new();
        e.string_field(1, "rust"); // language
        e.string_field(2, "src/main.rs"); // relative_path
        e.message_field(3, &symbol_msg("sym::main", 1, "main")); // fn
        e.message_field(3, &symbol_msg("sym::Helper", 3, "Helper")); // class
        // Occurrence: definition of main at 0:0.
        let mut occ = Encoder::new();
        occ.packed_field(1, &[0, 0, 0, 8]);
        occ.string_field(2, "sym::main");
        occ.varint_field(3, 1); // symbol_roles: definition
        e.message_field(4, &occ.finish());
        // Occurrence: reference to Helper at 5:4.
        let mut occ2 = Encoder::new();
        occ2.packed_field(1, &[5, 4, 5, 12]);
        occ2.string_field(2, "sym::Helper");
        occ2.varint_field(3, 0); // reference
        e.message_field(4, &occ2.finish());
        e.finish()
    }

    #[test]
    fn parses_document_fields() {
        let doc = parse_document(&document_bytes()).unwrap();
        assert_eq!(doc.language, "rust");
        assert_eq!(doc.relative_path, "src/main.rs");
        assert_eq!(doc.symbols.len(), 2);
        assert_eq!(doc.symbols[0].symbol, "sym::main");
        assert_eq!(doc.symbols[0].kind, SymbolKind::Function);
        assert_eq!(doc.symbols[0].display_name, "main");
        assert_eq!(doc.symbols[1].kind, SymbolKind::Type);
        assert_eq!(doc.occurrences.len(), 2);
        assert_eq!(doc.occurrences[0].range, vec![0, 0, 0, 8]);
        assert_eq!(doc.occurrences[0].symbol_roles, 1);
        assert_eq!(doc.occurrences[1].range, vec![5, 4, 5, 12]);
    }

    #[test]
    fn unknown_fields_and_kinds_are_tolerated() {
        let mut e = Encoder::new();
        e.string_field(1, "go");
        e.varint_field(99, 7); // unknown field number
        e.message_field(3, &symbol_msg("s::x", 999, "x")); // unknown kind
        let doc = parse_document(&e.finish()).unwrap();
        assert_eq!(doc.language, "go");
        assert_eq!(doc.symbols[0].kind, SymbolKind::Unknown);
    }

    #[test]
    fn converts_to_semantic_index() {
        let doc = parse_document(&document_bytes()).unwrap();
        let idx = to_semantic_index(&doc);
        assert_eq!(idx.symbols.len(), 2);
        assert_eq!(idx.occurrences.len(), 2);
        let occ: Vec<&SymbolOccurrence> = idx
            .occurrences
            .iter()
            .filter(|o| o.symbol == "sym::main")
            .collect();
        assert_eq!(occ.len(), 1);
        assert_eq!(occ[0].role, OccurrenceRole::Definition);
        assert_eq!(occ[0].line, 0);
        assert_eq!(occ[0].column, 0);
        assert_eq!(occ[0].file, "src/main.rs");
        // Queries run directly over the ingested index: `main` has a
        // definition and no references (unused); `Helper` only has a
        // reference occurrence, so it is not a dead-code candidate.
        let unused = idx.unused_exports();
        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].name, "sym::main");
    }

    #[test]
    fn truncated_buffer_errors() {
        let bytes = document_bytes();
        assert!(matches!(
            parse_document(&bytes[..bytes.len() - 3]),
            Err(ScipError::Truncated(_))
        ));
    }

    #[test]
    fn empty_document_is_empty_index() {
        let doc = parse_document(&[]).unwrap();
        assert_eq!(doc.symbols.len(), 0);
        assert!(to_semantic_index(&doc).symbols.is_empty());
    }
}
