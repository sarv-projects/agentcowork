//! P11.5.9 — the `// ai!` file-watcher marker (I10; doc 46 Aider).
//!
//! Users drop a `// ai! <instruction>` marker in a source file; the watcher
//! picks it up and auto-submits the instruction (with surrounding context) to
//! the agent. This module is the pure parse/extract half — the notify-crate
//! filesystem watch glue is the storage→core bridge (same seam as P5.4 ghost
//! events), so this stays dependency-free and fully testable.

use serde::{Deserialize, Serialize};

/// One `// ai!` marker found in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMarker {
    /// The instruction after `// ai!` (trimmed).
    pub instruction: String,
    /// 1-based line number of the marker.
    pub line: u32,
    /// Up to `context_before` lines above the marker (trimmed of the marker
    /// line itself) — the local context the agent should see.
    pub context: Vec<String>,
}

/// Supported marker forms: `// ai!`, `# ai!`, `-- ai!`, `<!-- ai! -->`,
/// `; ai!`, `// AI!` (case-insensitive). Both single-line and inline-tail.
fn marker_at(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    for (open, close) in [
        ("// ai!", None),
        ("# ai!", None),
        ("-- ai!", None),
        ("; ai!", None),
        ("<!-- ai!", Some("-->")),
    ] {
        if lower.starts_with(open) {
            let rest = &trimmed[open.len()..];
            let rest = match close {
                Some(c) => rest.split_once(c).map(|(r, _)| r).unwrap_or(rest),
                None => rest,
            };
            let instruction = rest.trim();
            if instruction.is_empty() {
                return Some("");
            }
            return Some(instruction);
        }
    }
    None
}

/// Scan a file's lines for `// ai!` markers, returning each marker with its
/// surrounding context. `max_markers` caps the batch (default 20).
pub fn scan_markers(lines: &[&str], context_before: usize, max_markers: usize) -> Vec<AiMarker> {
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if out.len() >= max_markers {
            break;
        }
        if let Some(instruction) = marker_at(line) {
            let start = idx.saturating_sub(context_before);
            let ctx: Vec<String> = lines[start..idx]
                .iter()
                .map(|l| l.trim_end().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            out.push(AiMarker {
                instruction: instruction.to_string(),
                line: (idx + 1) as u32,
                context: ctx,
            });
        }
    }
    out
}

/// The auto-submit payload sent to the agent (one per marker).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoSubmitPayload {
    pub file: String,
    pub marker: AiMarker,
}

impl AutoSubmitPayload {
    /// Render the prompt the agent receives.
    pub fn to_prompt(&self) -> String {
        let mut p = String::from("A `// ai!` marker was left in the file:\n\n");
        p.push_str(&format!("File: `{}`\n", self.file));
        if !self.marker.context.is_empty() {
            p.push_str("\nContext:\n```\n");
            for l in &self.marker.context {
                p.push_str(l);
                p.push('\n');
            }
            p.push_str("```\n");
        }
        p.push_str(&format!("\nInstruction: {}\n", self.marker.instruction));
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_line_markers() {
        let lines: Vec<&str> = vec![
            "// ai! add a timeout to this fetch",
            "const r = await fetch(url);",
        ];
        let marks = scan_markers(&lines, 0, 20);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].instruction, "add a timeout to this fetch");
        assert_eq!(marks[0].line, 1);
    }

    #[test]
    fn case_insensitive_and_other_comment_styles() {
        let lines: Vec<&str> = vec!["# AI! explain this block", "-- ai! refactor", "; ai! lint"];
        let marks = scan_markers(&lines, 0, 20);
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[0].instruction, "explain this block");
    }

    #[test]
    fn html_comment_form() {
        let lines: Vec<&str> = vec!["<!-- ai! check the a11y of this form -->"];
        let marks = scan_markers(&lines, 0, 20);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].instruction, "check the a11y of this form");
    }

    #[test]
    fn empty_instruction_still_matches() {
        let lines: Vec<&str> = vec!["// ai!"];
        let marks = scan_markers(&lines, 0, 20);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].instruction, "");
    }

    #[test]
    fn context_lines_are_included_above_marker() {
        let lines: Vec<&str> = vec![
            "function fetchData() {",
            "  return http.get('/data');",
            "}",
            "// ai! handle the error case",
            "",
        ];
        let marks = scan_markers(&lines, 3, 20);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].context.len(), 3);
        assert!(marks[0].context[0].contains("fetchData"));
    }

    #[test]
    fn max_markers_caps_batch() {
        let lines: Vec<&str> = vec!["// ai! one", "// ai! two", "// ai! three", "// ai! four"];
        let marks = scan_markers(&lines, 0, 2);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].instruction, "one");
    }

    #[test]
    fn payload_renders_instruction_and_context() {
        let payload = AutoSubmitPayload {
            file: "src/api.ts".into(),
            marker: AiMarker {
                instruction: "add timeout".into(),
                line: 2,
                context: vec!["const r = await fetch(url);".into()],
            },
        };
        let prompt = payload.to_prompt();
        assert!(prompt.contains("src/api.ts"));
        assert!(prompt.contains("add timeout"));
        assert!(prompt.contains("fetch(url)"));
    }
}
