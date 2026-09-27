//! P8.9 Sync / Export / Wipe (C8 — v2.0 §P8).
//!
//! Local, deterministic export + wipe primitives. The network side
//! (E2E-encrypted sync) stays an opt-in runtime seam; everything here is
//! pure functions over in-memory state so it is fully testable:
//!
//! - [`render_markdown_export`] — messages → Markdown transcript.
//! - [`render_json_export`] — messages → JSON (round-trippable).
//! - [`MemoryMirror`] — Obsidian-compatible `.md` memory mirror with
//!   `[[wiki-link]]`s (a *view surface*, never a second store).
//! - [`WipeScope`] + [`wipe_facts`] / [`wipe_messages`] — per-scope wipe.

use crate::memory_service::StoredFact;

/// A message as exported (role + content + timestamp). Content is the user's
/// own data being exported back to them — this is the one place content is
/// intentionally written out (export is explicit user action).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at_ms: u64,
}

impl ExportMessage {
    pub fn new(role: &str, content: &str) -> Self {
        Self {
            role: role.to_string(),
            content: content.to_string(),
            created_at_ms: 0,
        }
    }
}

/// Render a Markdown transcript of a conversation.
pub fn render_markdown_export(messages: &[ExportMessage]) -> String {
    let mut out = String::from("# Conversation export\n\n");
    for m in messages {
        let who = match m.role.as_str() {
            "user" => "**You**",
            "assistant" => "**Assistant**",
            "system" => "*system*",
            other => other,
        };
        out.push_str(&format!("{who}:\n\n{}\n\n---\n\n", m.content.trim()));
    }
    out
}

/// Render a JSON export (round-trippable back into [`ExportMessage`]s).
pub fn render_json_export(messages: &[ExportMessage]) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(messages)
}

/// One rendered Obsidian note (`(filename, markdown)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsidianNote {
    pub filename: String,
    pub markdown: String,
}

/// Obsidian-compatible memory mirror. Renders each fact as its own `.md`
/// note whose body carries `[[wiki-link]]`s to related facts (shared source
/// session or shared project). This is a *view surface* — the authoritative
/// store stays the memory service, exactly as doc 61 requires.
pub struct MemoryMirror;

impl MemoryMirror {
    /// Render a fact id into an Obsidian-safe wiki link target.
    pub fn wiki_target(id: &str) -> String {
        // `mem:12` → `mem-12` (Obsidian link names can't contain `:`).
        id.replace([':', '/', '\\', '#', '^', '|'], "-")
    }

    /// Render all facts as a set of notes, plus an `_index.md` linking them.
    pub fn render(facts: &[StoredFact]) -> Vec<ObsidianNote> {
        let mut notes = Vec::new();

        // Index note.
        let mut index = String::from("# Memory mirror\n\n");
        for f in facts {
            index.push_str(&format!(
                "- [[{}]] — {} (importance {})\n",
                Self::wiki_target(&f.id),
                first_line(&f.text),
                f.importance
            ));
        }
        notes.push(ObsidianNote {
            filename: "_index.md".to_string(),
            markdown: index,
        });

        // One note per fact, with wiki-links to related facts.
        for f in facts {
            let mut body = format!("# {}\n\n{}\n\n", Self::wiki_target(&f.id), f.text);
            body.push_str(&format!("- importance: {}\n", f.importance));
            body.push_str(&format!("- source: {}\n", f.source));
            if let Some(pid) = &f.project_id {
                body.push_str(&format!("- project: {pid}\n"));
            }
            // Related facts: same session or same project.
            let related: Vec<&StoredFact> = facts
                .iter()
                .filter(|g| {
                    g.id != f.id
                        && (g.session_id == f.session_id
                            || (g.project_id.is_some() && g.project_id == f.project_id))
                })
                .collect();
            if !related.is_empty() {
                body.push_str("\n## Related\n\n");
                for r in related {
                    body.push_str(&format!("- [[{}]]\n", Self::wiki_target(&r.id)));
                }
            }
            notes.push(ObsidianNote {
                filename: format!("{}.md", Self::wiki_target(&f.id)),
                markdown: body,
            });
        }
        notes
    }
}

/// What a wipe covers. `All` also clears connector data and messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WipeScope {
    /// A single chat/session's messages (not memory).
    Chat,
    /// A memory scope (optionally project-scoped) — facts only.
    Memory,
    /// Connector-cached data (fits the connector platform decision).
    Connector,
    /// Everything: messages + memory + connector data.
    All,
}

/// Pure per-scope wipe over facts. Returns the facts that survive.
pub fn wipe_facts(
    facts: Vec<StoredFact>,
    scope: WipeScope,
    project_id: Option<&str>,
) -> Vec<StoredFact> {
    match scope {
        WipeScope::Chat | WipeScope::Connector => facts,
        WipeScope::Memory => match project_id {
            // Project-scoped: remove only that project's facts.
            Some(pid) => facts
                .into_iter()
                .filter(|f| f.project_id.as_deref() != Some(pid))
                .collect(),
            // Global memory wipe clears everything (least surprising).
            None => Vec::new(),
        },
        WipeScope::All => Vec::new(),
    }
}

/// Pure per-scope wipe over messages. Returns the messages that survive.
pub fn wipe_messages(
    messages: Vec<ExportMessage>,
    scope: WipeScope,
    session_id: Option<&str>,
) -> Vec<ExportMessage> {
    match scope {
        WipeScope::Memory | WipeScope::Connector => messages,
        WipeScope::Chat => match session_id {
            // Without a session filter, a chat wipe clears nothing concrete —
            // the caller must scope it. (Kept explicit to avoid silent nukes.)
            None => messages,
            Some(_sid) => Vec::new(),
        },
        WipeScope::All => Vec::new(),
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(id: &str, session: &str, text: &str, project: Option<&str>) -> StoredFact {
        StoredFact {
            id: id.to_string(),
            session_id: session.to_string(),
            text: text.to_string(),
            importance: 5,
            created_at_ms: 0,
            updated_at_ms: 0,
            status: crate::memory_service::FactStatus::Active,
            source: "chat".to_string(),
            source_id: session.to_string(),
            project_id: project.map(|s| s.to_string()),
        }
    }

    #[test]
    fn markdown_export_renders_roles() {
        let out = render_markdown_export(&[
            ExportMessage::new("user", "hi"),
            ExportMessage::new("assistant", "hello"),
        ]);
        assert!(out.contains("**You**"));
        assert!(out.contains("**Assistant**"));
        assert!(out.contains("hi"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn json_export_roundtrips() {
        let msgs = vec![
            ExportMessage::new("user", "a"),
            ExportMessage::new("assistant", "b"),
        ];
        let json = render_json_export(&msgs).unwrap();
        let back: Vec<ExportMessage> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].content, "b");
    }

    #[test]
    fn mirror_generates_index_and_wikilinks() {
        let facts = vec![
            fact("mem:1", "s1", "User likes Rust.", None),
            fact("mem:2", "s1", "User prefers concise answers.", None),
            fact("mem:3", "s2", "User works on desktop apps.", None),
        ];
        let notes = MemoryMirror::render(&facts);
        assert_eq!(notes.len(), 4); // index + 3
        assert_eq!(notes[0].filename, "_index.md");
        assert!(notes[0].markdown.contains("[[mem-1]]"));
        // mem:1 and mem:2 share session s1 → link each other.
        let n1 = notes.iter().find(|n| n.filename == "mem-1.md").unwrap();
        assert!(n1.markdown.contains("[[mem-2]]"));
        assert!(
            !n1.markdown.contains("[[mem-3]]"),
            "different session must not link"
        );
        // Wiki target is Obsidian-safe (no colons).
        assert_eq!(MemoryMirror::wiki_target("mem:1"), "mem-1");
    }

    #[test]
    fn wipe_memory_scope_filters_by_project() {
        let facts = vec![
            fact("mem:1", "s1", "global", None),
            fact("mem:2", "s1", "proj-a", Some("a")),
            fact("mem:3", "s1", "proj-b", Some("b")),
        ];
        let after = wipe_facts(facts.clone(), WipeScope::Memory, Some("a"));
        assert_eq!(after.len(), 2);
        assert!(after.iter().all(|f| f.id != "mem:2"));
        // Global memory wipe clears all memory (project-scoped or not).
        let after_global = wipe_facts(facts.clone(), WipeScope::Memory, None);
        assert!(after_global.is_empty());
    }

    #[test]
    fn wipe_all_clears_everything() {
        let facts = vec![fact("mem:1", "s1", "x", None)];
        let msgs = vec![ExportMessage::new("user", "hi")];
        assert!(wipe_facts(facts.clone(), WipeScope::All, None).is_empty());
        assert!(wipe_messages(msgs.clone(), WipeScope::All, Some("s1")).is_empty());
    }

    #[test]
    fn wipe_chat_requires_session_scope() {
        let msgs = vec![ExportMessage::new("user", "hi")];
        // Unscoped chat wipe is a no-op (explicit, no silent nuke).
        assert_eq!(wipe_messages(msgs.clone(), WipeScope::Chat, None).len(), 1);
        // Scoped chat wipe clears the session.
        assert!(wipe_messages(msgs, WipeScope::Chat, Some("s1")).is_empty());
    }

    #[test]
    fn non_target_scopes_leave_facts_alone() {
        let facts = vec![fact("mem:1", "s1", "x", None)];
        assert_eq!(wipe_facts(facts.clone(), WipeScope::Chat, None).len(), 1);
        assert_eq!(
            wipe_facts(facts.clone(), WipeScope::Connector, None).len(),
            1
        );
    }
}
