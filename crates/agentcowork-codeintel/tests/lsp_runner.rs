//! P7.1 — live LSP diagnostics: the `LspRunner` against a *real process*.
//!
//! `CARGO_BIN_EXE_mock-lsp-server` is baked in by Cargo for the `[[bin]]`
//! fixture, so this integration test proves the full spawn → initialize →
//! didOpen → publishDiagnostics flow works over real stdio pipes — the same
//! path a real rust-analyzer / pyright / clangd binary would take.

use agentcowork_codeintel::lsp_config::LspServerConfig;
use agentcowork_codeintel::lsp_runner::LspRunner;

fn mock_server_path() -> &'static str {
    env!("CARGO_BIN_EXE_mock-lsp-server")
}

#[test]
fn lsp_runner_collects_diagnostics_from_a_real_process() {
    let cfg = LspServerConfig {
        command: mock_server_path().to_string(),
        args: vec!["canned:unused-variable".to_string()],
        env: Default::default(),
    };
    let batch = LspRunner::collect(
        &cfg,
        "file:///workspace",
        "file:///workspace/src/main.rs",
        "rust",
        "fn main() { let unused = 1; }",
    )
    .expect("live collect should succeed");

    assert_eq!(batch.uri, "file:///workspace/src/main.rs");
    assert_eq!(batch.diagnostics.len(), 1);
    assert_eq!(batch.diagnostics[0].message, "canned:unused-variable");
    assert_eq!(batch.diagnostics[0].severity, Some(1));
    assert_eq!(batch.diagnostics[0].source.as_deref(), Some("mock-lsp"));
}

#[test]
fn lsp_runner_surfaces_missing_binary() {
    let cfg = LspServerConfig {
        command: "definitely-not-a-real-language-server".to_string(),
        args: vec![],
        env: Default::default(),
    };
    let err = LspRunner::collect(&cfg, "file:///w", "file:///a.rs", "rust", "x")
        .expect_err("missing binary must error");
    assert!(err.to_string().contains("failed to spawn"));
}
