//! agentcowork-codeintel — code intelligence (P7.1, I11 — doc 63 §2.1).
//!
//! - `lsp` — LSP JSON-RPC framing (Content-Length headers) + the core types
//!   (hover, location, diagnostic, text-edit, code-action, inlay-hint).
//! - `semantic` — SCIP-style symbol index + `symbol_where`/`symbol_callers`/
//!   `unused_exports` queries (crux pattern).
//! - `repomap` — repo map: tag extraction, symbol graph, PageRank ranking,
//!   and budget fitting (aider `repomap.py` pattern). Tree-sitter plugs in as
//!   a `TagSource`; the lexical source is the default.
//! - `session` — the LSP session runtime: transport trait, stdio process
//!   transport (spawn + keep-alive), and the initialize/request/notify
//!   lifecycle.
//! - `scip` — SCIP protobuf ingestion: a dependency-free wire-format reader
//!   that decodes a SCIP `Document` into the `SemanticIndex`.

pub mod docs_lookup;
pub mod edit;
pub mod graph;
pub mod graphify;
pub mod lsp;
pub mod lsp_config;
pub mod lsp_runner;
pub mod repo_cache;
pub mod repomap;
pub mod scip;
pub mod scip_watch;
pub mod semantic;
pub mod session;
pub mod warp;

pub use edit::{
    DeleteVerdict, EditRegion, LspCapabilities, LspServerCatalog, LspServerEntry, parse_verify,
    replace_body, safe_delete,
};
pub use graph::{GraphEdge, GraphSymbol, SymbolGraph, SymbolQueryResult};
pub use graphify::{KnowledgeEdge, KnowledgeGraph, KnowledgeKind, KnowledgeNode};
pub use lsp::{
    CodeAction, Diagnostic, FramingError, Hover, HoverContents, InlayHint, Location, LspRequest,
    LspResponse, Position, Range, TextEdit, WorkspaceEdit, decode_messages, encode_message,
};
pub use lsp_config::{DiagnosticBatch, DiagnosticsService, LspConfig, LspServerConfig};
pub use lsp_runner::{LspRunner, LspRunnerError};
pub use repo_cache::{CachedRow, RepoMapCache, map_hash};
pub use repomap::{
    CompositeTagSource, LexicalTagSource, RankedTag, RepoMap, Tag, TagKind, TagSource,
    build_repo_map, build_repo_map_with, extract_tags, extract_tags_with, fit_budget, page_rank,
    rank_tags, ranked_tags, read_source_files,
};
pub use scip::{
    ScipDocument, ScipError, ScipOccurrence, ScipSymbol, parse_document, to_semantic_index,
};
pub use scip_watch::{
    ScipScanReport, ScipWatchState, build_index, find_scip_files, scan_dir, symbol_heat,
};
pub use semantic::{
    OccurrenceRole, RelationKind, Relationship, SemanticIndex, Symbol, SymbolKind, SymbolOccurrence,
};
pub use session::{LspSession, LspSessionError, LspTransport, ProcessTransport};
pub use warp::{
    ChangedChunk, Chunk, ChunkMode, Embedder, FileState, WarpIndex, chunk_text, chunks_for,
    embed_sync, sync_changed,
};
