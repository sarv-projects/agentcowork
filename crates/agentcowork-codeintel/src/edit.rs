//! I11 symbol-editing semantics (doc 65 §8 — serena steal): the
//! deterministic gates before a destructive edit, plus the packaged LSP
//! server catalog (language support as data).
//!
//! - [`safe_delete`] — refuses to delete a symbol while references exist
//!   (a deterministic gate, run before the edit reaches the guard ticket).
//! - [`replace_body`] — parse-verifies the *result* before returning the new
//!   content (balanced-delimiter gate — the deterministic stand-in for a
//!   full parse at this layer).
//! - [`LspServerCatalog`] — the packaged catalog of language servers:
//!   `id` / `command` / `version` / `capabilities`, so language support is
//!   data the coordinator can query, not code it must know.

use serde::{Deserialize, Serialize};

use crate::graph::{GraphEdge, SymbolGraph};

/// The verdict of a pre-edit gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteVerdict {
    /// Safe to delete — no references exist in the graph.
    Safe,
    /// Refused — references exist; the edit would break them.
    Refused { references: Vec<String> },
}

/// Refuse to delete a symbol while the graph shows references to it. The
/// deterministic gate *before* any destructive edit — the caller shows the
/// verdict in the guard ticket; `Refused` never reaches disk.
pub fn safe_delete(graph: &SymbolGraph, symbol: &str) -> DeleteVerdict {
    let incoming: Vec<GraphEdge> = graph.incoming_edges(symbol);
    if incoming.is_empty() {
        DeleteVerdict::Safe
    } else {
        DeleteVerdict::Refused {
            references: incoming.iter().map(|e| e.source.clone()).collect(),
        }
    }
}

/// A positioned edit region inside a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRegion {
    pub start_line: u32,
    pub end_line: u32,
}

/// Parse-verify a candidate new body before it is written: the result must
/// keep every delimiter class balanced (the deterministic stand-in for a
/// full parse at the editing layer). `None` = the candidate is malformed and
/// must not be written.
pub fn parse_verify(new_content: &str) -> Option<()> {
    let mut stack: Vec<char> = Vec::new();
    for c in new_content.chars() {
        match c {
            '(' | '[' | '{' => stack.push(c),
            ')' | ']' | '}' => {
                let open = stack.pop()?;
                let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
                if !pairs.iter().any(|(o, cl)| *o == open && *cl == c) {
                    return None;
                }
            }
            _ => {}
        }
    }
    if stack.is_empty() { Some(()) } else { None }
}

/// Replace a symbol's body between `region` lines with `new_body`, then
/// parse-verify the whole file. Returns the new file content only when the
/// result parses — otherwise `None` and nothing is written.
pub fn replace_body(content: &str, region: EditRegion, new_body: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if region.start_line == 0
        || region.end_line < region.start_line
        || region.end_line > lines.len() as u32
    {
        return None;
    }
    let (start, end) = (region.start_line as usize - 1, region.end_line as usize);
    let mut out: Vec<&str> = Vec::new();
    out.extend_from_slice(&lines[..start]);
    out.push(new_body);
    out.extend_from_slice(&lines[end..]);
    let joined = out.join("\n");
    parse_verify(&joined)?;
    Some(joined)
}

/// What one packaged LSP server advertises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspServerEntry {
    pub id: String,
    /// The command (plus args) that starts the server.
    pub command: String,
    /// The packaged/pinned version (F8 adapter-pinning discipline).
    pub version: String,
    /// Languages this server serves (language ids).
    pub languages: Vec<String>,
    /// Capability flags the coordinator can gate on.
    pub capabilities: LspCapabilities,
}

/// The capability set of an LSP server (what the coordinator may rely on).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspCapabilities {
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub rename: bool,
    pub code_actions: bool,
    pub diagnostics: bool,
}

/// The packaged catalog: language support as data (id/command/version/
/// capabilities), so the coordinator picks a server by querying, not by
/// hard-coding.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LspServerCatalog {
    pub servers: Vec<LspServerEntry>,
}

impl LspServerCatalog {
    /// The built-in catalog (pinned reference versions).
    pub fn builtin() -> Self {
        Self {
            servers: vec![
                LspServerEntry {
                    id: "rust-analyzer".into(),
                    command: "rust-analyzer".into(),
                    version: "2026-08-15".into(),
                    languages: vec!["rust".into()],
                    capabilities: LspCapabilities {
                        hover: true,
                        definition: true,
                        references: true,
                        rename: true,
                        code_actions: true,
                        diagnostics: true,
                    },
                },
                LspServerEntry {
                    id: "typescript-language-server".into(),
                    command: "typescript-language-server --stdio".into(),
                    version: "4.3.3".into(),
                    languages: vec!["typescript".into(), "javascript".into()],
                    capabilities: LspCapabilities {
                        hover: true,
                        definition: true,
                        references: true,
                        rename: true,
                        code_actions: true,
                        diagnostics: true,
                    },
                },
                LspServerEntry {
                    id: "pyright".into(),
                    command: "pyright-langserver --stdio".into(),
                    version: "1.1.379".into(),
                    languages: vec!["python".into()],
                    capabilities: LspCapabilities {
                        hover: true,
                        definition: true,
                        references: true,
                        rename: false,
                        code_actions: false,
                        diagnostics: true,
                    },
                },
                LspServerEntry {
                    id: "gopls".into(),
                    command: "gopls".into(),
                    version: "0.16.2".into(),
                    languages: vec!["go".into()],
                    capabilities: LspCapabilities {
                        hover: true,
                        definition: true,
                        references: true,
                        rename: true,
                        code_actions: true,
                        diagnostics: true,
                    },
                },
            ],
        }
    }

    /// The server for a language, or `None` (the coordinator reports the
    /// gap instead of guessing).
    pub fn for_language(&self, language: &str) -> Option<&LspServerEntry> {
        self.servers
            .iter()
            .find(|s| s.languages.iter().any(|l| l == language))
    }

    pub fn find(&self, id: &str) -> Option<&LspServerEntry> {
        self.servers.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphSymbol, SymbolGraph};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmpdb() -> std::path::PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "agentcowork-edit-test-{}-{n}.db",
            std::process::id()
        ))
    }

    #[test]
    fn safe_delete_refuses_when_referenced() {
        let path = tmpdb();
        let mut g = SymbolGraph::open(&path).unwrap();
        g.rebuild(
            &[GraphSymbol {
                name: "a".into(),
                kind: "function".into(),
                file: "f.rs".into(),
                line: 1,
                language: "rust".into(),
            }],
            &[GraphEdge {
                source: "b".into(),
                target: "a".into(),
                kind: "call".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            safe_delete(&g, "a"),
            DeleteVerdict::Refused {
                references: vec!["b".into()]
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn safe_delete_allows_unreferenced() {
        let path = tmpdb();
        let g = SymbolGraph::open(&path).unwrap();
        assert_eq!(safe_delete(&g, "orphan"), DeleteVerdict::Safe);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replace_body_verifies_and_rejects_malformed() {
        let src = "fn main() {\n    let x = 1;\n}\n";
        let ok = replace_body(
            src,
            EditRegion {
                start_line: 2,
                end_line: 2,
            },
            "    let y = 2;",
        )
        .unwrap();
        assert!(ok.contains("let y = 2;"));
        assert!(parse_verify(&ok).is_some());

        // Unbalanced braces in the replacement → rejected, nothing written.
        let bad = replace_body(
            src,
            EditRegion {
                start_line: 2,
                end_line: 2,
            },
            "    let y = {;",
        );
        assert!(bad.is_none());
    }

    #[test]
    fn catalog_is_queryable_data() {
        let c = LspServerCatalog::builtin();
        assert!(c.for_language("rust").is_some());
        let ts = c.for_language("typescript").unwrap();
        assert!(ts.capabilities.rename);
        let py = c.for_language("python").unwrap();
        assert!(!py.capabilities.rename); // pyright has no rename
        assert!(c.for_language("haskell").is_none());
        assert_eq!(c.find("gopls").unwrap().version, "0.16.2");
    }
}
