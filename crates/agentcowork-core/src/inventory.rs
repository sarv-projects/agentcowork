//! P37 ticketed AGENTS.md/CLAUDE.md write + live folder inventory: the
//! Cowork-style chips that show the workspace's real state. [`folder_inventory`]
//! scans a directory deterministically (files, dirs, recognized rules
//! files); [`rules_file_content`] builds the AGENTS.md/CLAUDE.md body from
//! an inventory + rules. The *write* itself rides the existing ticketed fs
//! path (`fs_write_ticket`/`fs_write_commit` — P41.3) — this module owns
//! the content + the inventory, never the write.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The live inventory of one folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderInventory {
    pub files: Vec<String>,
    pub dirs: Vec<String>,
    /// Recognized rules files (CLAUDE.md / AGENTS.md / .cursorrules / …).
    pub rules_files: Vec<String>,
    /// Total size in bytes (measured, not assumed).
    pub total_bytes: u64,
}

/// Scan a folder recursively (bounded depth), collecting files + dirs +
/// recognized rules files. Deterministic order (sorted). Skips hidden
/// entries and `target/`/`node_modules/` noise.
pub fn folder_inventory(root: &Path, max_depth: usize) -> Result<FolderInventory, String> {
    let mut inv = FolderInventory::default();
    walk(root, root, max_depth, &mut inv)?;
    inv.files.sort();
    inv.dirs.sort();
    inv.rules_files.sort();
    Ok(inv)
}

fn walk(root: &Path, dir: &Path, depth: usize, inv: &mut FolderInventory) -> Result<(), String> {
    if depth == 0 {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            inv.dirs.push(rel(root, &p));
            walk(root, &p, depth - 1, inv)?;
        } else if p.is_file() {
            if let Ok(md) = std::fs::metadata(&p) {
                inv.total_bytes += md.len();
            }
            let rel = rel(root, &p);
            if is_rules_file(&name) {
                inv.rules_files.push(rel.clone());
            }
            inv.files.push(rel);
        }
    }
    Ok(())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

fn is_rules_file(name: &str) -> bool {
    matches!(
        name,
        "CLAUDE.md" | "AGENTS.md" | ".cursorrules" | "QAGENTS.md" | "settings.json"
    )
}

/// Build the AGENTS.md / CLAUDE.md body from an inventory + user rules —
/// the "rules live on disk, visible to every agent" pattern.
pub fn rules_file_content(inventory: &FolderInventory, rules: &[String]) -> String {
    let mut out = String::from("# Project rules\n\n");
    for r in rules {
        out.push_str(&format!("- {r}\n"));
    }
    out.push_str("\n## Workspace inventory\n");
    for f in &inventory.rules_files {
        out.push_str(&format!("- rules: {f}\n"));
    }
    out.push_str(&format!(
        "- {} files, {} dirs, {:.1} KiB total\n",
        inventory.files.len(),
        inventory.dirs.len(),
        inventory.total_bytes as f64 / 1024.0
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("agentcowork-inv-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::create_dir_all(p.join("target")).unwrap(); // noise — skipped
        std::fs::write(p.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(p.join("AGENTS.md"), "# rules").unwrap();
        std::fs::write(p.join("target/out.bin"), vec![0u8; 100]).unwrap();
        p
    }

    #[test]
    fn inventory_scans_and_skips_noise() {
        let dir = tmpdir("scan");
        let inv = folder_inventory(&dir, 4).unwrap();
        assert!(inv.files.contains(&"src/main.rs".to_string()));
        assert!(inv.rules_files.contains(&"AGENTS.md".to_string()));
        assert!(!inv.files.iter().any(|f| f.contains("target")));
        assert!(inv.total_bytes > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rules_content_includes_inventory_line() {
        let dir = tmpdir("content");
        let inv = folder_inventory(&dir, 4).unwrap();
        let md = rules_file_content(&inv, &["use 4-space indent".into()]);
        assert!(md.contains("use 4-space indent"));
        assert!(md.contains("AGENTS.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
