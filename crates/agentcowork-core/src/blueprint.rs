//! `agents/*.md` blueprints — the declarative agent format (P0.6).
//!
//! Each blueprint is a Markdown file with a TOML frontmatter block. TOML
//! reuses the workspace's existing `toml` crate (no new parser dependency);
//! the `name / model / tools / permissions` fields are exactly the four the
//! spec's blueprint format fixes:
//!
//! ```markdown
//! ---
//! name = "code-reviewer"
//! model = "sonnet-4.5"
//! tools = ["bash", "edit", "grep"]
//! permissions = ["read:workspace", "write:workspace/src"]
//! ---
//!
//! # Code Reviewer
//!
//! Body instructions injected into the system prompt...
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Frontmatter fence delimiter.
pub const FRONTMATTER_DELIMITER: &str = "---";

/// The structured header of a blueprint (name/model/tools/permissions).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BlueprintMeta {
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// A fully-loaded agent blueprint: metadata + markdown body + source path.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentBlueprint {
    pub meta: BlueprintMeta,
    /// Markdown body after the frontmatter (the agent's instructions).
    pub body: String,
    pub path: PathBuf,
}

/// Split `content` into `(frontmatter, body)`. Errors when the leading `---`
/// fence or its closing fence is missing.
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), BlueprintError> {
    let rest = content
        .strip_prefix(FRONTMATTER_DELIMITER)
        .ok_or(BlueprintError::MissingFrontmatter)?;
    let end = rest
        .find(FRONTMATTER_DELIMITER)
        .ok_or(BlueprintError::MissingFrontmatter)?;
    let meta = &rest[..end];
    // Skip the closing fence and any leading blank line of the body.
    let body = rest[end + FRONTMATTER_DELIMITER.len()..].trim_start_matches('\n');
    Ok((meta.trim(), body))
}

/// Parse just the frontmatter of a blueprint file.
pub fn parse_frontmatter(content: &str) -> Result<BlueprintMeta, BlueprintError> {
    let (meta, _) = split_frontmatter(content)?;
    toml::from_str(meta).map_err(BlueprintError::Parse)
}

/// Load one `agents/*.md` blueprint file.
pub fn load_blueprint(path: &Path) -> Result<AgentBlueprint, BlueprintError> {
    let content = std::fs::read_to_string(path).map_err(BlueprintError::Io)?;
    let (meta_str, body) = split_frontmatter(&content)?;
    let meta = toml::from_str(meta_str).map_err(BlueprintError::Parse)?;
    Ok(AgentBlueprint {
        meta,
        body: body.to_string(),
        path: path.to_path_buf(),
    })
}

/// Load every `*.md` blueprint in `dir` (non-recursive; a missing dir is an
/// empty list — agents are opt-in). Results sorted by name for determinism.
pub fn load_all(dir: &Path) -> Result<Vec<AgentBlueprint>, BlueprintError> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(BlueprintError::Io(e)),
    };
    for entry in entries {
        let entry = entry.map_err(BlueprintError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(load_blueprint(&path)?);
        }
    }
    out.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum BlueprintError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("missing frontmatter (expected a `---` block at the top)")]
    MissingFrontmatter,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"---
name = "code-reviewer"
model = "sonnet-4.5"
tools = ["bash", "edit", "grep"]
permissions = ["read:workspace", "write:workspace/src"]
---

# Code Reviewer

Review diffs for correctness before merge.
"#;

    #[test]
    fn parses_full_blueprint() {
        let meta = parse_frontmatter(FULL).expect("parse");
        assert_eq!(meta.name, "code-reviewer");
        assert_eq!(meta.model.as_deref(), Some("sonnet-4.5"));
        assert_eq!(meta.tools, vec!["bash", "edit", "grep"]);
        assert_eq!(
            meta.permissions,
            vec!["read:workspace", "write:workspace/src"]
        );
    }

    #[test]
    fn missing_fields_default_to_empty() {
        let content = "---\nname = \"minimal\"\n---\n\nbody";
        let meta = parse_frontmatter(content).expect("parse");
        assert_eq!(meta.name, "minimal");
        assert_eq!(meta.model, None);
        assert!(meta.tools.is_empty());
        assert!(meta.permissions.is_empty());
    }

    #[test]
    fn missing_frontmatter_is_an_error() {
        assert!(matches!(
            parse_frontmatter("# no frontmatter here"),
            Err(BlueprintError::MissingFrontmatter)
        ));
        assert!(matches!(
            parse_frontmatter("---\nname = \"x\"\n"),
            Err(BlueprintError::MissingFrontmatter)
        ));
    }

    #[test]
    fn load_blueprint_keeps_body_and_path() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-blueprint-{}", std::process::id()));
        let path = dir.join("reviewer.md");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&path, FULL).expect("write");

        let bp = load_blueprint(&path).expect("load");
        assert_eq!(bp.meta.name, "code-reviewer");
        assert!(bp.body.contains("# Code Reviewer"));
        assert_eq!(bp.path, path);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_scans_only_markdown_sorted_by_name() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-blueprints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("zeta.md"), "---\nname = \"zeta\"\n---\n\nbody-z").expect("write");
        std::fs::write(dir.join("alpha.md"), "---\nname = \"alpha\"\n---\n\nbody-a")
            .expect("write");
        std::fs::write(dir.join("notes.txt"), "not a blueprint").expect("write");

        let all = load_all(&dir).expect("load all");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].meta.name, "alpha");
        assert_eq!(all[1].meta.name, "zeta");
        assert!(all[1].body.contains("body-z"));

        // Missing dir → empty list, not an error.
        let missing = dir.join("does-not-exist");
        assert!(load_all(&missing).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_excludes_fences() {
        let (meta, body) = split_frontmatter(FULL).expect("split");
        assert_eq!(
            meta,
            "name = \"code-reviewer\"\nmodel = \"sonnet-4.5\"\ntools = [\"bash\", \"edit\", \"grep\"]\npermissions = [\"read:workspace\", \"write:workspace/src\"]"
        );
        assert!(!body.starts_with("---"));
        assert!(body.starts_with("# Code Reviewer"));
    }
}
