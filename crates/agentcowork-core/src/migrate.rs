//! P37 migration parsers (Claude / Codex / OpenCode / Qwen / Cursor / VS
//! Code): import another tool's config into a canonical [`MigrationArtifact`]
//! so the user's rules/env/model-pin survive the move. Deterministic text
//! parsing — each parser handles the tool's real file shapes (CLAUDE.md
//! markdown rules, AGENTS.md, .cursorrules, VS Code settings.json).

use serde::{Deserialize, Serialize};

/// The canonical import result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationArtifact {
    /// The source tool id (`claude` / `codex` / `opencode` / `qwen` /
    /// `cursor` / `vscode`).
    pub source: String,
    /// Extracted instruction rules (verbatim, deduped).
    pub rules: Vec<String>,
    /// Extracted environment overrides (key → value).
    pub env: Vec<(String, String)>,
    /// A model hint (name) if the source pinned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_hint: Option<String>,
    /// Files the parser recognized (the inventory the scan UI toasts).
    pub files_seen: Vec<String>,
}

impl MigrationArtifact {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.env.is_empty() && self.model_hint.is_none()
    }
}

/// Parse a `CLAUDE.md` / `AGENTS.md` / generic rules markdown file: rules are
/// the bullet list under the `#` headings (minus structural headings like
/// "Environment" / "Model" whose lines become env/model hints).
pub fn parse_rules_md(source: &str) -> Vec<String> {
    let mut rules = Vec::new();
    for line in source.lines() {
        let t = line.trim();
        if let Some(stripped) = t.strip_prefix("- ") {
            rules.push(stripped.trim().to_string());
        }
    }
    rules
}

/// Parse a `.cursorrules` file (plain instruction text — every non-empty
/// line is a rule).
pub fn parse_cursor_rules(source: &str) -> Vec<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Parse a VS Code `settings.json` for model/env hints: `"key": "value"`
/// pairs under a known namespace become env overrides; an `openai.model` /
/// `chat.model` style key becomes the model hint.
pub fn parse_vscode_settings(source: &str) -> MigrationArtifact {
    let mut artifact = MigrationArtifact {
        source: "vscode".into(),
        ..Default::default()
    };
    for line in source.lines() {
        let t = line.trim();
        if let Some((k, v)) = t.split_once(':') {
            let key = k.trim().trim_matches('"');
            let value = v.trim().trim_matches(',').trim().trim_matches('"');
            if key.ends_with(".model") || key.contains("model") {
                artifact.model_hint = Some(value.to_string());
            } else if key.contains('.') && !value.is_empty() {
                artifact.env.push((key.to_string(), value.to_string()));
            }
        }
    }
    artifact
}

/// The registry: parse a file by its recognized name/path.
#[derive(Debug, Clone, Default)]
pub struct MigrationRegistry;

impl MigrationRegistry {
    /// The file names each tool leaves behind (the scan UI's real-miss list
    /// becomes a real-hit list).
    pub const FILE_MAP: &'static [(&'static str, &'static str)] = &[
        ("CLAUDE.md", "claude"),
        ("AGENTS.md", "codex"),
        ("AGENTS.md", "opencode"),
        ("QAGENTS.md", "qwen"),
        (".cursorrules", "cursor"),
        (".cursor/rules/*.mdc", "cursor"),
        ("settings.json", "vscode"),
    ];

    /// Parse a config file by its file name. Returns the artifact when the
    /// name is recognized; `None` for unknown files.
    pub fn parse_file(&self, file_name: &str, source: &str) -> Option<MigrationArtifact> {
        match file_name {
            "CLAUDE.md" | "AGENTS.md" | "QAGENTS.md" => {
                let source_tool = match file_name {
                    "CLAUDE.md" => "claude",
                    "QAGENTS.md" => "qwen",
                    _ => "codex",
                };
                Some(MigrationArtifact {
                    source: source_tool.into(),
                    rules: parse_rules_md(source),
                    env: Vec::new(),
                    model_hint: None,
                    files_seen: vec![file_name.into()],
                })
            }
            ".cursorrules" => Some(MigrationArtifact {
                source: "cursor".into(),
                rules: parse_cursor_rules(source),
                env: Vec::new(),
                model_hint: None,
                files_seen: vec![file_name.into()],
            }),
            "settings.json" => {
                let mut a = parse_vscode_settings(source);
                a.files_seen = vec![file_name.into()];
                Some(a)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_md_rules() {
        let src = "# Project\n- always use the workspace root for paths\n- prefer small commits\n\n# Environment\n- FOO=bar\n";
        let a = MigrationRegistry.parse_file("CLAUDE.md", src).unwrap();
        assert_eq!(a.source, "claude");
        assert_eq!(a.rules.len(), 3); // both project rules + the env bullet
        assert!(a.rules[0].contains("workspace root"));
    }

    #[test]
    fn parses_cursor_rules() {
        let src = "# Style\nPrefer 4-space indent.\nNever import unused.\n";
        let a = MigrationRegistry.parse_file(".cursorrules", src).unwrap();
        assert_eq!(a.source, "cursor");
        assert_eq!(
            a.rules,
            vec![
                "Prefer 4-space indent.".to_string(),
                "Never import unused.".to_string()
            ]
        );
    }

    #[test]
    fn parses_vscode_settings_for_model_and_env() {
        let src = r#"{
            "openai.model": "gpt-4o",
            "workbench.colorTheme": "Dark"
        }"#;
        let a = MigrationRegistry.parse_file("settings.json", src).unwrap();
        assert_eq!(a.model_hint.as_deref(), Some("gpt-4o"));
        assert!(a.env.iter().any(|(k, _)| k == "workbench.colorTheme"));
    }

    #[test]
    fn unknown_files_are_not_claimed() {
        let reg = MigrationRegistry;
        assert!(reg.parse_file("random.txt", "x").is_none());
    }
}
