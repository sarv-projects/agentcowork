//! P11.5.3 — LSP diagnostics for the IDE Problems panel. Wraps the existing
//! `agentcowork-codeintel::LspRunner` (already integration-tested against a
//! mock LSP server): spawn the configured server, initialize against the
//! workspace root, open the file, and return the first publishDiagnostics
//! batch. The UI shows real errors/warnings, not mock rows.
//!
//! Honest ceilings: the runner does the initialize→open→diagnostics flow
//! per file (no long-lived session yet — hover/completion/rename stay a
//! session-based follow-up); the server binary must be installed on the
//! user's machine (rust-analyzer / typescript-language-server / pyright).

use agentcowork_codeintel::{LspRunner, LspServerConfig};

/// Language → server config for the built-in LSP mapping.
fn server_for(language: &str) -> Option<LspServerConfig> {
    let (command, args) = match language.to_lowercase().as_str() {
        "rust" | "rs" => ("rust-analyzer", vec![]),
        "typescript" | "ts" | "javascript" | "js" => {
            ("typescript-language-server", vec!["--stdio".to_string()])
        }
        "python" | "py" => ("pyright-langserver", vec!["--stdio".to_string()]),
        _ => return None,
    };
    Some(LspServerConfig {
        command: command.to_string(),
        args,
        env: Default::default(),
    })
}

/// Run the LSP collect flow for one file and return its diagnostics batch.
/// `root` is the workspace dir (or nearest git root); `path` is the file;
/// `text` is the current content (what the editor has).
#[tauri::command]
pub fn lsp_diagnostics(
    root: String,
    path: String,
    language: String,
    text: String,
) -> Result<serde_json::Value, String> {
    let cfg = server_for(&language).ok_or_else(|| {
        format!("no LSP server configured for language '{language}' (rust/ts/python only)")
    })?;
    let uri = format!("file://{}", path);
    let root_uri = format!("file://{}", root);
    let batch = LspRunner::collect(&cfg, &root_uri, &uri, &language, &text)
        .map_err(|e| format!("LSP: {e}"))?;
    // Flatten into the shape the Problems panel renders.
    let rows: Vec<serde_json::Value> = batch
        .diagnostics
        .iter()
        .map(|d| {
            let severity = d.severity.unwrap_or(1); // 1=error, 2=warning, 3=info
            serde_json::json!({
                "path": path,
                "line": d.range.start.line,
                "col": d.range.start.character,
                "severity": severity,
                "severityLabel": match severity { 1 => "error", 2 => "warning", _ => "info" },
                "message": d.message,
                "source": d.source,
            })
        })
        .collect();
    Ok(serde_json::json!({ "rows": rows, "count": rows.len() }))
}
