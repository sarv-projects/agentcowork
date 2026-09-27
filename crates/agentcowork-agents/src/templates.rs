//! P31.3 — the 8 create-agent wizard templates. Each pre-fills a bundle:
//! Identity → Capabilities → Workflows, one click per template.
//!
//! The **brain is deliberately not pre-filled** (ADR-0005): v1's engine is an
//! external ACP agent the user installs, so a template carries no built-in
//! binding and no default CLI — the wizard's Brain step binds one. A template
//! bundle is therefore a draft (`engine: None`) until then.

use crate::bundle::{AgentBundle, ToolScope};

/// The 8 canonical templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTemplate {
    General,
    Coder,
    Researcher,
    EmailTriager,
    DataAnalyst,
    Writer,
    MeetingNotes,
    BrowserOperator,
}

impl AgentTemplate {
    pub const ALL: [AgentTemplate; 8] = [
        AgentTemplate::General,
        AgentTemplate::Coder,
        AgentTemplate::Researcher,
        AgentTemplate::EmailTriager,
        AgentTemplate::DataAnalyst,
        AgentTemplate::Writer,
        AgentTemplate::MeetingNotes,
        AgentTemplate::BrowserOperator,
    ];

    pub fn id(self) -> &'static str {
        match self {
            AgentTemplate::General => "general",
            AgentTemplate::Coder => "coder",
            AgentTemplate::Researcher => "researcher",
            AgentTemplate::EmailTriager => "email-triager",
            AgentTemplate::DataAnalyst => "data-analyst",
            AgentTemplate::Writer => "writer",
            AgentTemplate::MeetingNotes => "meeting-notes",
            AgentTemplate::BrowserOperator => "browser-operator",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentTemplate::General => "General",
            AgentTemplate::Coder => "Coder",
            AgentTemplate::Researcher => "Researcher",
            AgentTemplate::EmailTriager => "Email Triager",
            AgentTemplate::DataAnalyst => "Data Analyst",
            AgentTemplate::Writer => "Writer",
            AgentTemplate::MeetingNotes => "Meeting Notes",
            AgentTemplate::BrowserOperator => "Browser Operator",
        }
    }

    /// Build the pre-filled bundle (name = template label by default; the
    /// wizard step 1 renames it).
    pub fn bundle(self) -> AgentBundle {
        use AgentTemplate::*;
        let mut b = AgentBundle::new(self.label());
        match self {
            General => {
                b.description = "A helpful general-purpose assistant.".into();
            }
            Coder => {
                b.description = "Writes and fixes code with editor + terminal access.".into();
                b.emoji = "👨\u{200d}💻".into();
                b.tools = ToolScope {
                    allow: vec![
                        "fs.read".into(),
                        "fs.write".into(),
                        "shell".into(),
                        "search".into(),
                    ],
                    deny: Vec::new(),
                };
                b.mcp_servers = vec!["filesystem".into()];
            }
            Researcher => {
                b.description = "Deep web + document research with citation cards.".into();
                b.emoji = "🔬".into();
                b.tools = ToolScope {
                    allow: vec!["search".into(), "memory.read".into()],
                    deny: vec!["fs.write".into(), "shell".into()],
                };
            }
            EmailTriager => {
                b.description =
                    "Triage your inbox: summarize, draft, never send without approval.".into();
                b.emoji = "📬".into();
                b.connectors = vec!["gmail".into()];
                b.tools = ToolScope {
                    allow: vec!["email.read".into(), "email.draft".into()],
                    deny: Vec::new(),
                };
            }
            DataAnalyst => {
                b.description = "Sum, pivot, and chart spreadsheets; never invents numbers.".into();
                b.emoji = "📊".into();
                b.tools = ToolScope {
                    allow: vec!["office.read".into(), "office.write".into()],
                    deny: vec!["shell".into()],
                };
            }
            Writer => {
                b.description = "Long-form writing and rewriting with your style memory.".into();
                b.emoji = "✍️".into();
                b.tools = ToolScope {
                    allow: vec!["fs.read".into(), "fs.write".into()],
                    deny: vec!["shell".into()],
                };
            }
            MeetingNotes => {
                b.description = "Turns transcripts into structured notes + action items.".into();
                b.emoji = "📝".into();
                b.tools = ToolScope {
                    allow: vec!["office.write".into()],
                    deny: Vec::new(),
                };
            }
            BrowserOperator => {
                b.description = "Drives the browser: navigate, snapshot, act, verify.".into();
                b.emoji = "🌐".into();
                b.tools = ToolScope {
                    allow: vec![
                        "browser.navigate".into(),
                        "browser.act".into(),
                        "browser.snapshot".into(),
                    ],
                    deny: Vec::new(),
                };
            }
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_templates_prefill_bundles_without_a_brain() {
        for t in AgentTemplate::ALL {
            let b = t.bundle();
            assert!(!b.name.is_empty());
            assert!(!b.description.is_empty());
            // ADR-0005: no template ships a built-in engine binding.
            assert!(b.engine.is_none());
            assert!(b.definition().is_none());
        }
        assert_eq!(AgentTemplate::ALL.len(), 8);
    }

    #[test]
    fn coder_ships_fs_tools() {
        let b = AgentTemplate::Coder.bundle();
        assert!(b.tools.allows("fs.write"));
        assert!(!b.tools.allows("shell.boot"));
    }

    #[test]
    fn email_triager_scopes_gmail() {
        let b = AgentTemplate::EmailTriager.bundle();
        assert!(b.declares_connector("gmail"));
        assert!(!b.declares_connector("slack"));
    }
}
