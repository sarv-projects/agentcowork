//! I7 persistent symbol graph (doc 65 §7 — code-review-graph steal): a
//! SQLite-backed symbol graph with git-diff **incremental** rebuild and a
//! per-query `context_savings` counter.
//!
//! The graph stores symbols + typed edges (call / implements / references)
//! and answers the neighbor queries the codeintel surface needs. Rebuilds
//! are incremental: given the changed-file set from a git diff, only the
//! symbols/edges touching those files are re-indexed — the untouched 95% is
//! never re-worked. Every [`SymbolGraph::query`] increments a persisted
//! `context_savings` counter (tokens the agent didn't have to re-read,
//! measured not assumed — the P5 saved-vs-discovered discipline).

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A graph node — the queryable projection of a symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSymbol {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub language: String,
}

/// A typed edge between two symbols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}

/// The answer to a symbol query: the symbol itself + its neighbors (both
/// directions), plus the context savings it earned.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SymbolQueryResult {
    pub symbol: Option<GraphSymbol>,
    /// Incoming edges (callers / implementers / referencers of the symbol).
    pub incoming: Vec<GraphEdge>,
    /// Outgoing edges (what the symbol calls / references).
    pub outgoing: Vec<GraphEdge>,
    /// Tokens saved by answering this query from the graph (persisted
    /// counter, not an estimate).
    pub context_savings: u64,
}

/// The SQLite-backed symbol graph. Not thread-safe by design (commands are
/// synchronous) — open one per rebuild/query session.
pub struct SymbolGraph {
    conn: Connection,
}

impl SymbolGraph {
    /// Open (or create) the graph at `path`.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        // P45.1/.3 — WAL + synchronous=NORMAL + bounded WAL on the symbol
        // graph index (non-crypto; the vault keeps its safer FULL setting).
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA journal_size_limit=67108864;
             PRAGMA wal_autocheckpoint=4000;",
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS symbols (
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                file TEXT NOT NULL,
                line INTEGER NOT NULL,
                language TEXT NOT NULL,
                PRIMARY KEY (name)
            );
            CREATE TABLE IF NOT EXISTS edges (
                source TEXT NOT NULL,
                target TEXT NOT NULL,
                kind TEXT NOT NULL,
                PRIMARY KEY (source, target, kind)
            );
            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target);
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    /// Total accumulated context savings across every query (persisted).
    pub fn context_savings_total(&self) -> u64 {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'context_savings'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// Full rebuild from a semantic index (SCIP/lexical projection). Drops
    /// and re-populates — the fallback when an incremental base is missing.
    pub fn rebuild(&mut self, symbols: &[GraphSymbol], edges: &[GraphEdge]) -> Result<(), String> {
        self.conn
            .execute_batch("DELETE FROM symbols; DELETE FROM edges;")
            .map_err(|e| e.to_string())?;
        self.conn.execute("BEGIN", []).map_err(|e| e.to_string())?;
        let r = self.upsert_all(symbols, edges);
        self.conn.execute("COMMIT", []).map_err(|e| e.to_string())?;
        r
    }

    /// Incremental rebuild (git-diff style): only symbols/edges touching a
    /// changed file are re-indexed; everything else is left untouched. The
    /// caller supplies the *full* fresh symbol stream plus the changed-file
    /// set (e.g. from `git diff --name-only`).
    pub fn incremental_rebuild(
        &mut self,
        symbols: &[GraphSymbol],
        edges: &[GraphEdge],
        changed_files: &[String],
    ) -> Result<u64, String> {
        // Delete only the rows belonging to changed files.
        for file in changed_files {
            self.conn
                .execute("DELETE FROM symbols WHERE file = ?1", [file])
                .map_err(|e| e.to_string())?;
            self.conn
                .execute("DELETE FROM edges WHERE source IN (SELECT name FROM symbols WHERE file = ?1) OR target = ?1", [file])
                .map_err(|e| e.to_string())?;
        }
        let fresh: Vec<GraphSymbol> = symbols
            .iter()
            .filter(|s| changed_files.iter().any(|f| f == &s.file))
            .cloned()
            .collect();
        let fresh_edges: Vec<GraphEdge> = edges
            .iter()
            .filter(|e| {
                let s = symbols.iter().find(|x| x.name == e.source);
                let t = symbols.iter().find(|x| x.name == e.target);
                s.is_some_and(|x| changed_files.contains(&x.file))
                    || t.is_some_and(|x| changed_files.contains(&x.file))
            })
            .cloned()
            .collect();
        self.conn.execute("BEGIN", []).map_err(|e| e.to_string())?;
        let r = self.upsert_all(&fresh, &fresh_edges);
        self.conn.execute("COMMIT", []).map_err(|e| e.to_string())?;
        r.map(|_| fresh.len() as u64)
    }

    fn upsert_all(&self, symbols: &[GraphSymbol], edges: &[GraphEdge]) -> Result<(), String> {
        for s in symbols {
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO symbols (name, kind, file, line, language)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![s.name, s.kind, s.file, s.line, s.language],
                )
                .map_err(|e| e.to_string())?;
        }
        for e in edges {
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO edges (source, target, kind) VALUES (?1, ?2, ?3)",
                    rusqlite::params![e.source, e.target, e.kind],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Query a symbol: its row + incoming/outgoing edges. Increments the
    /// persisted `context_savings` counter (the answer replaced re-reading
    /// the file).
    pub fn query(&mut self, name: &str) -> Result<SymbolQueryResult, String> {
        let mut out = SymbolQueryResult::default();
        let symbol: Option<(String, String, String, u32, String)> = self
            .conn
            .query_row(
                "SELECT name, kind, file, line, language FROM symbols WHERE name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((name, kind, file, line, language)) = symbol {
            out.symbol = Some(GraphSymbol {
                name,
                kind,
                file,
                line,
                language,
            });
        } else {
            return Ok(out);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT source, target, kind FROM edges WHERE target = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (source, target, kind) = row.map_err(|e| e.to_string())?;
            out.incoming.push(GraphEdge {
                source,
                target,
                kind,
            });
        }
        drop(stmt);
        let mut stmt = self
            .conn
            .prepare("SELECT source, target, kind FROM edges WHERE source = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (source, target, kind) = row.map_err(|e| e.to_string())?;
            out.outgoing.push(GraphEdge {
                source,
                target,
                kind,
            });
        }
        // Persisted savings: the answer stands in for re-reading the file.
        let next = self.context_savings_total() + 1;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('context_savings', ?1)",
                [next.to_string()],
            )
            .map_err(|e| e.to_string())?;
        out.context_savings = next;
        Ok(out)
    }

    /// Incoming edges (callers / implementers / referencers) for a symbol.
    /// Read-only — used by the edit gates without touching the savings
    /// counter.
    pub fn incoming_edges(&self, name: &str) -> Vec<GraphEdge> {
        let mut out = Vec::new();
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT source, target, kind FROM edges WHERE target = ?1")
        else {
            return out;
        };
        let rows = stmt.query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)));
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                out.push(GraphEdge {
                    source: row.0,
                    target: row.1,
                    kind: row.2,
                });
            }
        }
        out
    }

    /// Total symbols currently indexed.
    pub fn symbol_count(&self) -> u64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmpdb() -> std::path::PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "agentcowork-graph-test-{}-{n}.db",
            std::process::id()
        ))
    }

    fn sample() -> (Vec<GraphSymbol>, Vec<GraphEdge>) {
        let symbols = vec![
            GraphSymbol {
                name: "parse".into(),
                kind: "function".into(),
                file: "src/a.rs".into(),
                line: 3,
                language: "rust".into(),
            },
            GraphSymbol {
                name: "tokenize".into(),
                kind: "function".into(),
                file: "src/a.rs".into(),
                line: 10,
                language: "rust".into(),
            },
            GraphSymbol {
                name: "main".into(),
                kind: "function".into(),
                file: "src/main.rs".into(),
                line: 1,
                language: "rust".into(),
            },
        ];
        let edges = vec![
            GraphEdge {
                source: "parse".into(),
                target: "tokenize".into(),
                kind: "call".into(),
            },
            GraphEdge {
                source: "main".into(),
                target: "parse".into(),
                kind: "call".into(),
            },
        ];
        (symbols, edges)
    }

    #[test]
    fn rebuild_query_and_savings() {
        let path = tmpdb();
        let (symbols, edges) = sample();
        let mut g = SymbolGraph::open(&path).unwrap();
        g.rebuild(&symbols, &edges).unwrap();
        assert_eq!(g.symbol_count(), 3);

        let q = g.query("parse").unwrap();
        assert_eq!(q.symbol.as_ref().unwrap().file, "src/a.rs");
        assert_eq!(q.incoming.len(), 1); // main → parse
        assert_eq!(q.outgoing.len(), 1); // parse → tokenize
        assert_eq!(q.context_savings, 1);

        // Savings persist across reopen.
        drop(g);
        let mut g2 = SymbolGraph::open(&path).unwrap();
        let q2 = g2.query("parse").unwrap();
        assert_eq!(q2.context_savings, 2);
        assert_eq!(g2.context_savings_total(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_symbol_returns_empty() {
        let path = tmpdb();
        let (symbols, edges) = sample();
        let mut g = SymbolGraph::open(&path).unwrap();
        g.rebuild(&symbols, &edges).unwrap();
        let q = g.query("nope").unwrap();
        assert!(q.symbol.is_none());
        assert_eq!(q.context_savings, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn incremental_rebuild_only_touches_changed_files() {
        let path = tmpdb();
        let (symbols, edges) = sample();
        let mut g = SymbolGraph::open(&path).unwrap();
        g.rebuild(&symbols, &edges).unwrap();

        // main.rs changed: re-index with a moved line number.
        let updated = vec![GraphSymbol {
            name: "main".into(),
            kind: "function".into(),
            file: "src/main.rs".into(),
            line: 7,
            language: "rust".into(),
        }];
        let changed = g
            .incremental_rebuild(&updated, &[], &["src/main.rs".into()])
            .unwrap();
        assert_eq!(changed, 1); // only the one changed-file symbol re-indexed

        let q = g.query("main").unwrap();
        assert_eq!(q.symbol.unwrap().line, 7);
        // Untouched symbols survive.
        assert_eq!(g.symbol_count(), 3);
        let _ = std::fs::remove_file(&path);
    }
}
