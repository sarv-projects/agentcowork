//! P11.5.9 — code-intel Tauri commands: repo-map (I7 RepoMap library wired to
//! the UI), DeepWiki-style file outline, MODEL_ALIASES resolution from
//! `agentcowork.toml`, and the `// ai!` marker scan (I10 watcher feed).

use std::collections::BTreeMap;
use std::path::Path;

use agentcowork_codeintel::repomap::{ranked_tags, RankedTag, TagKind};
use serde::Serialize;
use tauri::State;

use crate::AppState;

/// P11.5.9 — build a repo map for a directory: tags + PageRank over the
/// symbol graph + budget-fit ordering. Deterministic (sorted edges, stable
/// order). The SQLite cache is the coordinator-side follow-on; this command
/// is the live path.
#[tauri::command]
pub fn repomap_build(
    _state: State<'_, AppState>,
    dir: String,
    max_files: Option<usize>,
) -> Result<Vec<RankedTag>, String> {
    let max_files = max_files.unwrap_or(200).min(2000);
    // One implementation, two façades: the walk + PageRank + stable sort lives
    // in `agentcowork-codeintel::repomap`, and the coordinator's
    // `codeintel/repomap` method calls the same function, so the UI command and
    // the agent-facing method cannot drift apart.
    Ok(ranked_tags(Path::new(&dir), max_files))
}

/// One outline entry (DeepWiki `file_outline` pattern — open Rust reference).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineEntry {
    pub symbol: String,
    pub kind: String,
    pub line: u32,
}

/// P11.5.9 — `file_outline`: symbols in document order for one source file.
/// Tree-sitter plugs in as a TagSource later; the lexical extractor is the
/// deterministic default (same as RepoMap).
#[tauri::command]
pub fn file_outline(
    _state: State<'_, AppState>,
    path: String,
) -> Result<Vec<OutlineEntry>, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let file = Path::new(&path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let tags = agentcowork_codeintel::repomap::extract_tags(&content, &file);
    let mut entries: Vec<OutlineEntry> = tags
        .into_iter()
        .map(|t| OutlineEntry {
            symbol: t.symbol,
            kind: match t.kind {
                TagKind::Function => "fn",
                TagKind::Type => "type",
                TagKind::Const => "const",
                TagKind::Module => "mod",
            }
            .into(),
            line: t.line,
        })
        .collect();
    entries.sort_by_key(|e| e.line);
    Ok(entries)
}

/// P11.5.9 — resolve a MODEL_ALIASES short name from `agentcowork.toml`.
#[tauri::command]
pub fn model_aliases_resolve(
    _state: State<'_, AppState>,
    reference: String,
) -> Result<serde_json::Value, String> {
    let cfg = agentcowork_core::config::Config::load().map_err(|e| e.to_string())?;
    let (provider, model) = cfg.resolve_model_alias(&reference, "openai");
    Ok(serde_json::json!({
        "alias": reference,
        "provider": provider,
        "model": model,
        "usedAlias": cfg.model_aliases.contains_key(&reference),
        "aliases": cfg.model_aliases,
    }))
}

/// P11.5.9 — scan a file (or directory, first file) for `// ai!` markers and
/// return the auto-submit payloads (I10 watcher feed — the notify-crate glue
/// is the storage→core bridge; this is the deterministic scan half).
#[tauri::command]
pub fn ai_markers_scan(
    _state: State<'_, AppState>,
    path: String,
) -> Result<Vec<agentcowork_core::ai_marker::AutoSubmitPayload>, String> {
    let p = Path::new(&path);
    let files: Vec<String> = if p.is_dir() {
        // The shared walker (`agentcowork-codeintel`) rather than a local copy,
        // so the marker scan, the UI command, and the coordinator's repo-map
        // method all walk a tree the same way.
        agentcowork_codeintel::repomap::read_source_files(p, 50)
            .into_iter()
            .map(|(f, _)| f)
            .collect()
    } else {
        vec![path.clone()]
    };
    let mut out = Vec::new();
    for f in files {
        let Ok(content) = std::fs::read_to_string(&f) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        for m in agentcowork_core::ai_marker::scan_markers(&lines, 5, 10) {
            out.push(agentcowork_core::ai_marker::AutoSubmitPayload {
                file: f.clone(),
                marker: m,
            });
        }
    }
    Ok(out)
}

/// Helper used by tests/build scripts to count extensions per dir (kept small
/// and unused-public to avoid dead-code warnings in the command surface).
#[allow(dead_code)]
fn _ext_counts(dir: &Path) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                *counts.entry(ext.to_string()).or_insert(0) += 1;
            }
        }
    }
    counts
}
