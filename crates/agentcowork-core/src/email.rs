//! Email & Calendar connectors (P6.11 — F14/F15, doc 50 §6).
//!
//! The connector *tool model* + guard-ticketing classification. The actual
//! network transports (Gmail OAuth, Google Calendar API, IMAP/SMTP, browser
//! session) are adapters; this module defines the tool surface and the
//! mutation split so the Guard-2 gate is mechanical: **send/reply/event-write
//! are mutations** (ticket required), read/search/availability are read-only.
//! Tokens live in `agentcowork-vault` (never in the agent context).
//!
//! P6.11 local-runtime close-out (2026-08-20): the deterministic core is now
//! implemented and tested here — the [`Mailbox`] abstraction, an in-memory
//! [`InMemoryMailbox`] for round-trip tests, the guard-ticketed
//! [`EmailService`] that owns read/search/send/reply/triage over any mailbox,
//! and the calendar-nudge → scheduled-task wiring ([`suggest_nudge`]). The
//! live transports (Gmail OAuth, Google Calendar API, IMAP/SMTP wire,
//! browser-session Gmail/Outlook) remain credential/provider-gated adapters
//! behind [`Mailbox`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The email-tool surface (doc 50 §6 `openonion/email-agent` reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailTool {
    /// Read a mailbox/folder.
    Read,
    /// Search messages (query/filter).
    Search,
    /// Send a message — **mutation** (Guard-2 card).
    Send,
    /// Reply to a thread — **mutation** (Guard-2 card).
    Reply,
    /// Triage/label/archive — **mutation** (Guard-2 card, reversible).
    Triage,
}

impl EmailTool {
    /// Mutations require a ticket; reads do not (they still mint an
    /// auto-allow ticket under Stage 0, but never a human card).
    pub fn is_mutation(self) -> bool {
        !matches!(self, EmailTool::Read | EmailTool::Search)
    }
}

/// The calendar-tool surface (F15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarTool {
    ListEvents,
    CreateEvent,
    UpdateEvent,
    DeleteEvent,
    Availability,
}

impl CalendarTool {
    pub fn is_mutation(self) -> bool {
        matches!(
            self,
            CalendarTool::CreateEvent | CalendarTool::UpdateEvent | CalendarTool::DeleteEvent
        )
    }
}

/// Which transport a connector uses (doc 50 §6 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailProvider {
    /// Gmail via Auth Bridge OAuth (gmail.readonly/send/modify scopes).
    Gmail,
    /// Google Calendar via Auth Bridge OAuth.
    GoogleCalendar,
    /// Provider-agnostic IMAP/SMTP fallback (imapflow/async-imap + lettre).
    Imap,
    /// Browser-session Gmail/Outlook (last resort, brittle).
    BrowserSession,
}

/// IMAP/SMTP config for the provider-agnostic fallback (no secrets here —
/// credentials live in the vault).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapSmtpConfig {
    pub imap_host: String,
    pub imap_port: u16,
    /// IMAP IDLE for inbox push.
    pub idle: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
}

impl Default for ImapSmtpConfig {
    fn default() -> Self {
        Self {
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            idle: true,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 465,
        }
    }
}

/// A normalized email message (connector-agnostic).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailMessage {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub thread_id: Option<String>,
}

/// A normalized outgoing message (what the guard-ticketed send/reply surface
/// carries).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutgoingEmail {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    /// When set, this is a reply to an existing thread id.
    pub in_reply_to: Option<String>,
}

/// Triage actions (doc 50 §6: label/archive/read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageAction {
    Archive,
    Label,
    MarkRead,
    MarkUnread,
}

/// The calendar-nudge suggestion that feeds B7 scheduled tasks (F15).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarNudge {
    /// A cron-style schedule suggestion derived from email context.
    pub cron: String,
    pub goal: String,
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// Mailbox abstraction (P6.11 local-runtime close-out)
// ---------------------------------------------------------------------------

/// Errors from a mailbox transport. Transport-specific failures (network,
/// auth, provider API) map onto these so the guard/service layer stays
/// provider-agnostic.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MailboxError {
    #[error("message {0} not found")]
    NotFound(String),
    #[error("mailbox send failed: {0}")]
    SendFailed(String),
    #[error("mailbox read failed: {0}")]
    ReadFailed(String),
    #[error("not connected to any mailbox")]
    NotConnected,
}

/// The transport-agnostic mailbox contract (P6.11 #4). A live Gmail/IMAP
/// connector implements this; the in-memory implementation powers round-trip
/// tests and the stub F14 exit criterion.
pub trait Mailbox {
    /// Read one message by id.
    fn read(&self, id: &str) -> Result<EmailMessage, MailboxError>;
    /// Search by substring over subject + body (deterministic order: id).
    fn search(&self, query: &str) -> Result<Vec<EmailMessage>, MailboxError>;
    /// Send (or reply-to) a message. Returns the new message id.
    fn send(&mut self, msg: OutgoingEmail) -> Result<String, MailboxError>;
    /// Apply a triage action. Returns true if the message existed.
    fn triage(&mut self, id: &str, action: TriageAction) -> Result<bool, MailboxError>;
}

/// Deterministic in-memory mailbox — the F14 stub round-trip path and the
/// test oracle for every guard-ticketed email tool.
#[derive(Debug, Default)]
pub struct InMemoryMailbox {
    messages: BTreeMap<String, EmailMessage>,
    next_id: u64,
}

impl InMemoryMailbox {
    pub fn new() -> Self {
        Self {
            messages: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// Seed a message (as if it arrived via IMAP/browser).
    pub fn seed(&mut self, from: &str, subject: &str, body: &str) -> String {
        let id = format!("m{}", self.next_id);
        self.next_id += 1;
        self.messages.insert(
            id.clone(),
            EmailMessage {
                id: id.clone(),
                from: from.into(),
                to: vec!["me@local".into()],
                subject: subject.into(),
                body: body.into(),
                thread_id: Some(id.clone()),
            },
        );
        id
    }
}

impl Mailbox for InMemoryMailbox {
    fn read(&self, id: &str) -> Result<EmailMessage, MailboxError> {
        self.messages
            .get(id)
            .cloned()
            .ok_or_else(|| MailboxError::NotFound(id.into()))
    }

    fn search(&self, query: &str) -> Result<Vec<EmailMessage>, MailboxError> {
        let q = query.to_lowercase();
        let mut out: Vec<EmailMessage> = self
            .messages
            .values()
            .filter(|m| {
                q.is_empty()
                    || m.subject.to_lowercase().contains(&q)
                    || m.body.to_lowercase().contains(&q)
                    || m.from.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn send(&mut self, msg: OutgoingEmail) -> Result<String, MailboxError> {
        let id = format!("m{}", self.next_id);
        self.next_id += 1;
        let thread_id = msg.in_reply_to.clone().unwrap_or_else(|| id.clone());
        self.messages.insert(
            id.clone(),
            EmailMessage {
                id: id.clone(),
                from: "me@local".into(),
                to: msg.to.clone(),
                subject: msg.subject,
                body: msg.body,
                thread_id: Some(thread_id),
            },
        );
        Ok(id)
    }

    fn triage(&mut self, id: &str, _action: TriageAction) -> Result<bool, MailboxError> {
        Ok(self.messages.contains_key(id))
    }
}

/// The guard-ticketed email service (P6.11 #4): owns the mutation split.
/// Reads auto-allow; mutations require an approved Guard-2 ticket. The guard
/// seam is injected so the live `GuardService` and the test stub share one
/// contract.
pub trait TicketGate {
    /// Approve a mutation. Returns the consumed ticket id on success.
    fn approve_mutation(&mut self, tool: EmailTool, detail: &str) -> Result<String, String>;
}

/// A ticket gate that always allows (test path + Stage-0 auto-allow). The
/// live path wires `GuardService::evaluate` + `use_ticket` (the executor
/// call-site) — see `guard_service.rs`.
pub struct AutoAllowGate;

impl TicketGate for AutoAllowGate {
    fn approve_mutation(&mut self, _tool: EmailTool, _detail: &str) -> Result<String, String> {
        Ok("auto-allow".into())
    }
}

/// Guard-ticketed email operations over any [`Mailbox`].
pub struct EmailService<M: Mailbox, G: TicketGate> {
    mailbox: M,
    gate: G,
}

impl<M: Mailbox, G: TicketGate> EmailService<M, G> {
    pub fn new(mailbox: M, gate: G) -> Self {
        Self { mailbox, gate }
    }

    pub fn read(&self, id: &str) -> Result<EmailMessage, MailboxError> {
        self.mailbox.read(id)
    }

    pub fn search(&self, query: &str) -> Result<Vec<EmailMessage>, MailboxError> {
        self.mailbox.search(query)
    }

    /// Send is a mutation → must pass the guard gate before the mailbox
    /// write.
    pub fn send(&mut self, msg: OutgoingEmail) -> Result<String, MailboxError> {
        self.gate
            .approve_mutation(EmailTool::Send, &format!("send to {:?}", msg.to))
            .map_err(MailboxError::SendFailed)?;
        self.mailbox.send(msg)
    }

    /// Reply is a mutation → guard-gated, thread-bound.
    pub fn reply(&mut self, thread_id: &str, body: &str) -> Result<String, MailboxError> {
        let original = self.mailbox.read(thread_id)?;
        self.gate
            .approve_mutation(EmailTool::Reply, &format!("reply to {}", original.from))
            .map_err(MailboxError::SendFailed)?;
        self.mailbox.send(OutgoingEmail {
            to: vec![original.from],
            subject: format!("Re: {}", original.subject),
            body: body.into(),
            in_reply_to: Some(thread_id.into()),
        })
    }

    /// Triage is a mutation → guard-gated.
    pub fn triage(&mut self, id: &str, action: TriageAction) -> Result<bool, MailboxError> {
        self.gate
            .approve_mutation(EmailTool::Triage, &format!("{action:?} {id}"))
            .map_err(MailboxError::SendFailed)?;
        self.mailbox.triage(id, action)
    }
}

// ---------------------------------------------------------------------------
// Calendar nudge → scheduled task (P6.11 #5, F15)
// ---------------------------------------------------------------------------

/// Derive a [`CalendarNudge`] from email context (the F15 "suggest schedule
/// from email context" pattern). Deterministic: `confidence` comes from the
/// strength of the evidence, not the model.
pub fn suggest_nudge(goal: &str, evidence: &NudgeEvidence) -> CalendarNudge {
    // Evidence-weighted cron: mention of a deadline day or a recurring
    // interval picks the cadence; otherwise default to a weekday morning
    // check.
    let cron = if evidence.mentions_deadline {
        "0 9 * * 1-5".to_string()
    } else if evidence.recurring_interval_days > 0 {
        format!("0 9 */{} * *", evidence.recurring_interval_days)
    } else {
        "0 9 * * 1-5".to_string()
    };
    let mut confidence: f64 = 0.45;
    if evidence.mentions_deadline {
        confidence += 0.25;
    }
    if evidence.recurring_interval_days > 0 {
        confidence += 0.15;
    }
    if evidence.sender_known {
        confidence += 0.10;
    }
    CalendarNudge {
        cron,
        goal: goal.to_string(),
        confidence: confidence.min(1.0),
    }
}

/// Evidence a nudge derivation can use (email headers + model-extracted
/// fields). Kept struct-level so the deterministic function is testable
/// without a live mailbox.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NudgeEvidence {
    pub mentions_deadline: bool,
    pub recurring_interval_days: u32,
    pub sender_known: bool,
}

/// Classify a raw tool name into the email/calendar tool model (for routing a
/// connector MCP call through the right guard class).
pub fn classify_tool(name: &str) -> Option<ToolSurface> {
    Some(match name {
        "email_read" | "gmail_read" => ToolSurface::Email(EmailTool::Read),
        "email_search" | "gmail_search" => ToolSurface::Email(EmailTool::Search),
        "email_send" | "gmail_send" => ToolSurface::Email(EmailTool::Send),
        "email_reply" | "gmail_reply" => ToolSurface::Email(EmailTool::Reply),
        "email_triage" | "gmail_triage" => ToolSurface::Email(EmailTool::Triage),
        "calendar_list" => ToolSurface::Calendar(CalendarTool::ListEvents),
        "calendar_create" => ToolSurface::Calendar(CalendarTool::CreateEvent),
        "calendar_update" => ToolSurface::Calendar(CalendarTool::UpdateEvent),
        "calendar_delete" => ToolSurface::Calendar(CalendarTool::DeleteEvent),
        "calendar_availability" => ToolSurface::Calendar(CalendarTool::Availability),
        _ => return None,
    })
}

/// The unified tool surface (email or calendar) for guard classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "surface", rename_all = "snake_case")]
pub enum ToolSurface {
    Email(EmailTool),
    Calendar(CalendarTool),
}

impl ToolSurface {
    pub fn is_mutation(self) -> bool {
        match self {
            ToolSurface::Email(t) => t.is_mutation(),
            ToolSurface::Calendar(t) => t.is_mutation(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_mutations_are_send_reply_triage() {
        assert!(!EmailTool::Read.is_mutation());
        assert!(!EmailTool::Search.is_mutation());
        assert!(EmailTool::Send.is_mutation());
        assert!(EmailTool::Reply.is_mutation());
        assert!(EmailTool::Triage.is_mutation());
    }

    #[test]
    fn calendar_mutations_are_write_ops() {
        assert!(!CalendarTool::ListEvents.is_mutation());
        assert!(!CalendarTool::Availability.is_mutation());
        assert!(CalendarTool::CreateEvent.is_mutation());
        assert!(CalendarTool::UpdateEvent.is_mutation());
        assert!(CalendarTool::DeleteEvent.is_mutation());
    }

    #[test]
    fn classify_maps_tool_names() {
        assert_eq!(
            classify_tool("email_send"),
            Some(ToolSurface::Email(EmailTool::Send))
        );
        assert_eq!(
            classify_tool("gmail_search"),
            Some(ToolSurface::Email(EmailTool::Search))
        );
        assert_eq!(
            classify_tool("calendar_create"),
            Some(ToolSurface::Calendar(CalendarTool::CreateEvent))
        );
        assert!(classify_tool("calendar_create").unwrap().is_mutation());
        assert!(!classify_tool("email_read").unwrap().is_mutation());
        assert_eq!(classify_tool("unknown_tool"), None);
    }

    #[test]
    fn default_imap_smtp_is_gmail() {
        let c = ImapSmtpConfig::default();
        assert_eq!(c.imap_host, "imap.gmail.com");
        assert!(c.idle);
    }

    // -- P6.11 local-runtime close-out --------------------------------------

    #[test]
    fn inbox_round_trip_read_search_send_reply_triage() {
        let mut mailbox = InMemoryMailbox::new();
        let id = mailbox.seed("alice@example.com", "Project update", "The deadline moved.");
        let mut svc = EmailService::new(mailbox, AutoAllowGate);

        // Read (no ticket).
        assert_eq!(svc.read(&id).unwrap().from, "alice@example.com");
        // Search (no ticket).
        assert_eq!(svc.search("deadline").unwrap().len(), 1);
        assert_eq!(svc.search("nope").unwrap().len(), 0);
        // Send (mutation, gated).
        let sent = svc
            .send(OutgoingEmail {
                to: vec!["bob@example.com".into()],
                subject: "hi".into(),
                body: "hello".into(),
                in_reply_to: None,
            })
            .unwrap();
        assert_eq!(svc.read(&sent).unwrap().to, vec!["bob@example.com"]);
        // Reply (mutation, thread-bound).
        let replied = svc.reply(&id, "ack").unwrap();
        let reply_msg = svc.read(&replied).unwrap();
        assert_eq!(reply_msg.thread_id.as_deref(), Some(id.as_str()));
        assert_eq!(reply_msg.subject, "Re: Project update");
        // Triage (mutation).
        assert!(svc.triage(&id, TriageAction::Archive).unwrap());
        assert!(!svc.triage("missing", TriageAction::Archive).unwrap());
    }

    #[test]
    fn search_is_case_insensitive_and_ordered() {
        let mut mailbox = InMemoryMailbox::new();
        mailbox.seed("a@x.test", "Alpha", "one");
        mailbox.seed("b@x.test", "Beta", "two");
        let svc = EmailService::new(mailbox, AutoAllowGate);
        let hits = svc.search("ALPHA").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].from, "a@x.test");
    }

    #[test]
    fn missing_message_is_not_found() {
        let svc = EmailService::new(InMemoryMailbox::new(), AutoAllowGate);
        assert_eq!(svc.read("nope"), Err(MailboxError::NotFound("nope".into())));
    }

    #[test]
    fn nudge_confidence_reflects_evidence() {
        let strong = suggest_nudge(
            "Follow up on the proposal",
            &NudgeEvidence {
                mentions_deadline: true,
                recurring_interval_days: 7,
                sender_known: true,
            },
        );
        let weak = suggest_nudge("Follow up", &NudgeEvidence::default());
        assert!(strong.confidence > weak.confidence);
        assert_eq!(strong.cron, "0 9 * * 1-5");
        assert!(strong.confidence <= 1.0);
    }

    #[test]
    fn nudge_recurring_interval_becomes_step_cron() {
        let n = suggest_nudge(
            "Weekly digest",
            &NudgeEvidence {
                mentions_deadline: false,
                recurring_interval_days: 3,
                sender_known: false,
            },
        );
        assert_eq!(n.cron, "0 9 */3 * *");
    }
}
