//! LSP-config diagnostics pattern (P6.8 — doc 56 W4, the Copilot CLI
//! `lsp-config.json` / Warp `lsp` crate reference).
//!
//! A declarative `lsp-config.json` maps a language/glob to a language-server
//! command + args (e.g. `rust-analyzer`, `typescript-language-server
//! --stdio`, `pyright-langserver --stdio`, `clangd`, `gopls`). The
//! [`DiagnosticsService`] reads that config, spawns the matching server
//! through the existing [`LspSession`] runtime, opens a file, and collects
//! `textDocument/publishDiagnostics` — precise errors without full-file
//! context, exactly the Copilot CLI pattern.
//!
//! The config parsing + spawn wiring is implemented and tested here (with an
//! in-process mock transport); actually launching a real language server
//! binary remains an installed-binary integration (the P6.8 live item).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::lsp::Diagnostic;
use crate::session::{LspSession, LspSessionError, LspTransport};

/// One entry of `lsp-config.json`: which server to spawn for which language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LspServerConfig {
    /// The server binary (e.g. `rust-analyzer`).
    pub command: String,
    /// Args, e.g. `["--stdio"]` for typescript-language-server.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional per-server environment overrides (name → value).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The parsed `lsp-config.json`: language id → server config. Also accepts a
/// `"*"` default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LspConfig {
    #[serde(flatten)]
    pub servers: BTreeMap<String, LspServerConfig>,
}

impl LspConfig {
    pub fn parse(source: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(source)
    }

    /// The server config for a language, falling back to the `*` default.
    pub fn for_language(&self, language: &str) -> Option<&LspServerConfig> {
        self.servers.get(language).or_else(|| self.servers.get("*"))
    }

    /// The known languages in the config (for UI + discovery).
    pub fn languages(&self) -> Vec<&str> {
        self.servers.keys().map(|k| k.as_str()).collect()
    }
}

/// A collected diagnostic batch from one file open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticBatch {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Collects diagnostics for a file by driving an LSP session over any
/// [`LspTransport`] (in tests: a mock transport; in production:
/// [`crate::ProcessTransport`] spawning the configured server).
pub struct DiagnosticsService;

impl DiagnosticsService {
    /// Open `uri` (a `file://…` URI) against the server spawned from `cfg`
    /// and collect the first `publishDiagnostics` batch. This is the
    /// "precise errors without full-file context" flow: initialize → open →
    /// read diagnostics. `root_uri` is the workspace root the server is
    /// initialized against.
    pub fn collect<T: LspTransport>(
        mut session: LspSession<T>,
        root_uri: &str,
        uri: &str,
        language: &str,
        text: &str,
    ) -> Result<DiagnosticBatch, LspSessionError> {
        session.initialize(root_uri, "agentcowork")?;
        session.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language,
                    "version": 1,
                    "text": text
                }
            }),
        )?;
        let raw = session
            .recv_diagnostics()?
            .ok_or_else(|| LspSessionError::ServerError("no publishDiagnostics received".into()))?;
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// A mock LSP transport that answers `initialize` and then emits one
    /// `publishDiagnostics` notification after `didOpen` — no real binary
    /// needed, so the collect flow is fully exercised in-process.
    struct MockLspTransport {
        /// Messages the client sent (for assertions).
        sent: Vec<String>,
        /// The scripted publishDiagnostics notification to emit after open.
        scripted: Option<String>,
    }

    impl MockLspTransport {
        fn new(scripted: Option<String>) -> Self {
            Self {
                sent: Vec::new(),
                scripted,
            }
        }
    }

    impl LspTransport for MockLspTransport {
        fn send(&mut self, json: &str) -> io::Result<()> {
            self.sent.push(json.to_string());
            Ok(())
        }
        fn recv(&mut self) -> io::Result<Option<String>> {
            // First recv answers the `initialize` request.
            let sent_init = self.sent.iter().any(|m| m.contains("\"initialize\""));
            if sent_init && self.sent.len() == 1 {
                return Ok(Some(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#.into(),
                ));
            }
            // After didOpen, emit the scripted diagnostics notification.
            if let Some(diag) = self.scripted.take() {
                return Ok(Some(diag));
            }
            Ok(None)
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        fn shutdown(&mut self) {}
    }

    #[test]
    fn parses_lsp_config_json() {
        let cfg = LspConfig::parse(
            r#"{
                "rust": { "command": "rust-analyzer" },
                "typescript": { "command": "typescript-language-server", "args": ["--stdio"] },
                "python": { "command": "pyright-langserver", "args": ["--stdio"], "env": { "PYRIGHT_PYTHON_FORCE_VERSION": "3.12" } },
                "*": { "command": "clangd" }
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.for_language("rust").unwrap().command, "rust-analyzer");
        assert_eq!(
            cfg.for_language("typescript").unwrap().args,
            vec!["--stdio"]
        );
        assert_eq!(
            cfg.for_language("python").unwrap().env["PYRIGHT_PYTHON_FORCE_VERSION"],
            "3.12"
        );
        // Unknown language falls back to the default.
        assert_eq!(cfg.for_language("cpp").unwrap().command, "clangd");
        assert_eq!(cfg.languages().len(), 4);
    }

    #[test]
    fn collect_returns_scripted_diagnostics() {
        let diag = format!(
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": "file:///a.rs",
                    "diagnostics": [
                        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } },
                          "severity": 1, "message": "unused variable" }
                    ]
                }
            })
        );
        let transport = MockLspTransport::new(Some(diag));
        let session = LspSession::new(transport);
        let batch = DiagnosticsService::collect(
            session,
            "file:///workspace",
            "file:///a.rs",
            "rust",
            "fn main() {}",
        )
        .expect("collect should succeed");
        assert_eq!(batch.uri, "file:///a.rs");
        assert_eq!(batch.diagnostics.len(), 1);
        assert_eq!(batch.diagnostics[0].message, "unused variable");
    }

    #[test]
    fn missing_diagnostics_is_an_error() {
        let transport = MockLspTransport::new(None);
        let session = LspSession::new(transport);
        let err = DiagnosticsService::collect(session, "file:///w", "file:///a.rs", "rust", "x")
            .unwrap_err();
        assert!(matches!(err, LspSessionError::ServerError(_)));
    }
}
