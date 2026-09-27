//! Live LSP-backed diagnostics (P7.1 — doc 56 W4, the Warp `lsp` crate /
//! Copilot CLI `lsp-config.json` pattern).
//!
//! [`DiagnosticsService::collect`] drives diagnostics over any [`LspTransport`]
//! (mock in unit tests); [`LspRunner`] is the *live* seam: it takes a parsed
//! [`LspServerConfig`] (command + args + env), spawns the real language server
//! over stdio via [`crate::ProcessTransport`], and runs the collect flow —
//! initialize → didOpen → `publishDiagnostics` — so a configured
//! rust-analyzer / typescript-language-server / pyright / clangd / gopls
//! produces precise errors without full-file context.
//!
//! The runner is transport-generic and loopback-tested against the
//! `mock-lsp-server` fixture binary; actually launching a real language
//! server still requires that binary to be installed on the machine.

use crate::lsp_config::{DiagnosticBatch, DiagnosticsService, LspServerConfig};
use crate::session::{LspSession, LspSessionError, ProcessTransport};

/// Errors from the live LSP runner.
#[derive(Debug, thiserror::Error)]
pub enum LspRunnerError {
    #[error("failed to spawn language server: {0}")]
    Spawn(String),
    #[error("language server session error: {0}")]
    Session(#[from] LspSessionError),
}

/// Spawn a real language server from an `lsp-config.json` entry and collect
/// the `publishDiagnostics` batch for one opened file.
pub struct LspRunner;

impl LspRunner {
    /// Run the full live flow for one file:
    ///
    /// 1. spawn `config.command` with `config.args` (+ `config.env`) over
    ///    stdio;
    /// 2. initialize the session against `root_uri`;
    /// 3. open `uri` with `language` + `text`;
    /// 4. return the first `textDocument/publishDiagnostics` batch.
    pub fn collect(
        config: &LspServerConfig,
        root_uri: &str,
        uri: &str,
        language: &str,
        text: &str,
    ) -> Result<DiagnosticBatch, LspRunnerError> {
        let args: Vec<&str> = config.args.iter().map(|a| a.as_str()).collect();
        let env: Vec<(&str, &str)> = config
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let transport = ProcessTransport::spawn_env(&config.command, &args, &env)
            .map_err(|e| LspRunnerError::Spawn(e.to_string()))?;
        let session = LspSession::new(transport);
        DiagnosticsService::collect(session, root_uri, uri, language, text)
            .map_err(LspRunnerError::from)
    }
}
