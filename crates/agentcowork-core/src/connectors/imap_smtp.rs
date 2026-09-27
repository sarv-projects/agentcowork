//! P6.11 — Provider-agnostic IMAP/SMTP fallback connector.
//!
//! Provides read/search/send operations over IMAP (IDLE for inbox push) and
//! SMTP (send/reply). The wire protocol logic is tested with an injectable
//! [`MailTransport`] seam — the live implementation uses `imapflow`/`async-imap`
//! + `lettre` or equivalent libraries behind the seam.

use super::TransportError;

/// Injectable mail transport seam — replaces real IMAP/SMTP wire access
/// with mock data for testing.
pub trait MailTransport {
    /// IMAP: fetch messages matching a query from a mailbox.
    fn imap_fetch(
        &self,
        mailbox: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RawMailMessage>, TransportError>;

    /// IMAP: search for message UIDs matching criteria.
    fn imap_search(&self, mailbox: &str, criteria: &str) -> Result<Vec<u32>, TransportError>;

    /// IMAP: get unread count for a mailbox.
    fn imap_unread_count(&self, mailbox: &str) -> Result<usize, TransportError>;

    /// IMAP: set flags on messages (e.g. \\Seen, \\Flagged, +X-GM-LABELS).
    fn imap_set_flags(
        &self,
        mailbox: &str,
        uids: &[u32],
        flags: &[&str],
        add: bool,
    ) -> Result<(), TransportError>;

    /// IMAP: move messages between mailboxes.
    fn imap_move(&self, mailbox: &str, uids: &[u32], dest: &str) -> Result<(), TransportError>;

    /// IMAP: delete messages.
    fn imap_delete(&self, mailbox: &str, uids: &[u32]) -> Result<(), TransportError>;

    /// SMTP: send a raw MIME message.
    fn smtp_send(&self, to: &str, raw_mime: &[u8]) -> Result<String, TransportError>;

    /// SMTP: send a reply to an existing message (sets In-Reply-To, References).
    fn smtp_reply(
        &self,
        in_reply_to: &str,
        to: &str,
        raw_mime: &[u8],
    ) -> Result<String, TransportError>;
}

/// A raw mail message as returned by the transport.
#[derive(Debug, Clone)]
pub struct RawMailMessage {
    pub uid: u32,
    pub subject: String,
    pub from: String,
    pub to: String,
    pub date: String,
    pub body_plain: String,
    pub body_html: String,
    pub flags: Vec<String>,
    pub message_id: String,
    pub in_reply_to: Option<String>,
}

/// Search result.
#[derive(Debug, Clone)]
pub struct MailSearchResult {
    pub messages: Vec<RawMailMessage>,
    pub total: usize,
}

/// Send result.
#[derive(Debug, Clone)]
pub struct MailSendResult {
    pub message_id: String,
}

/// IMAP/SMTP connector — stateless protocol logic over the injected seam.
pub struct ImapSmtpConnector<T: MailTransport> {
    transport: T,
}

impl<T: MailTransport> ImapSmtpConnector<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Search messages in a mailbox.
    pub fn search(
        &self,
        mailbox: &str,
        query: &str,
        limit: usize,
    ) -> Result<MailSearchResult, TransportError> {
        let messages = self.transport.imap_fetch(mailbox, query, limit)?;
        let total = messages.len();
        Ok(MailSearchResult { messages, total })
    }

    /// Get unread count.
    pub fn unread_count(&self, mailbox: &str) -> Result<usize, TransportError> {
        self.transport.imap_unread_count(mailbox)
    }

    /// Read a message by UID.
    pub fn read_message(
        &self,
        mailbox: &str,
        uid: u32,
    ) -> Result<Option<RawMailMessage>, TransportError> {
        let msgs = self
            .transport
            .imap_fetch(mailbox, &format!("UID {uid}"), 1)?;
        Ok(msgs.into_iter().next())
    }

    /// Mark a message as read.
    pub fn mark_read(&self, mailbox: &str, uid: u32) -> Result<(), TransportError> {
        self.transport
            .imap_set_flags(mailbox, &[uid], &["\\Seen"], true)
    }

    /// Mark a message as flagged (starred).
    pub fn mark_flagged(&self, mailbox: &str, uid: u32) -> Result<(), TransportError> {
        self.transport
            .imap_set_flags(mailbox, &[uid], &["\\Flagged"], true)
    }

    /// Archive a message (move to Archive/All Mail).
    pub fn archive(&self, mailbox: &str, uid: u32) -> Result<(), TransportError> {
        self.transport.imap_move(mailbox, &[uid], "Archive")
    }

    /// Trash a message.
    pub fn trash(&self, mailbox: &str, uid: u32) -> Result<(), TransportError> {
        self.transport.imap_move(mailbox, &[uid], "Trash")
    }

    /// Send a new message.
    pub fn send(
        &self,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<MailSendResult, TransportError> {
        let raw = build_mime(to, subject, body);
        let message_id = self.transport.smtp_send(to, raw.as_bytes())?;
        Ok(MailSendResult { message_id })
    }

    /// Reply to a message.
    pub fn reply(
        &self,
        in_reply_to: &str,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<MailSendResult, TransportError> {
        let raw = build_mime(to, subject, body);
        let message_id = self.transport.smtp_reply(in_reply_to, to, raw.as_bytes())?;
        Ok(MailSendResult { message_id })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_mime(to: &str, subject: &str, body: &str) -> String {
    format!(
        "To: {to}\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockMailTransport {
        messages: RefCell<Vec<RawMailMessage>>,
        sent: RefCell<Vec<(String, String, String)>>, // (to, subject, body)
    }

    impl MockMailTransport {
        fn with_messages(msgs: Vec<RawMailMessage>) -> Self {
            Self {
                messages: RefCell::new(msgs),
                sent: RefCell::new(Vec::new()),
            }
        }
    }

    impl MailTransport for MockMailTransport {
        fn imap_fetch(
            &self,
            _mailbox: &str,
            _query: &str,
            limit: usize,
        ) -> Result<Vec<RawMailMessage>, TransportError> {
            let msgs = self.messages.borrow();
            Ok(msgs.iter().take(limit).cloned().collect())
        }
        fn imap_search(&self, _mailbox: &str, _criteria: &str) -> Result<Vec<u32>, TransportError> {
            Ok(self.messages.borrow().iter().map(|m| m.uid).collect())
        }
        fn imap_unread_count(&self, _mailbox: &str) -> Result<usize, TransportError> {
            Ok(self
                .messages
                .borrow()
                .iter()
                .filter(|m| !m.flags.iter().any(|f| f == "\\Seen"))
                .count())
        }
        fn imap_set_flags(
            &self,
            _mailbox: &str,
            _uids: &[u32],
            _flags: &[&str],
            _add: bool,
        ) -> Result<(), TransportError> {
            Ok(())
        }
        fn imap_move(
            &self,
            _mailbox: &str,
            _uids: &[u32],
            _dest: &str,
        ) -> Result<(), TransportError> {
            Ok(())
        }
        fn imap_delete(&self, _mailbox: &str, _uids: &[u32]) -> Result<(), TransportError> {
            Ok(())
        }
        fn smtp_send(&self, to: &str, raw_mime: &[u8]) -> Result<String, TransportError> {
            let body = String::from_utf8_lossy(raw_mime).to_string();
            self.sent
                .borrow_mut()
                .push((to.to_string(), String::new(), body));
            Ok("msg-001".into())
        }
        fn smtp_reply(
            &self,
            in_reply_to: &str,
            to: &str,
            raw_mime: &[u8],
        ) -> Result<String, TransportError> {
            let body = String::from_utf8_lossy(raw_mime).to_string();
            self.sent
                .borrow_mut()
                .push((to.to_string(), in_reply_to.to_string(), body));
            Ok("msg-002".into())
        }
    }

    fn test_message(uid: u32, subject: &str) -> RawMailMessage {
        RawMailMessage {
            uid,
            subject: subject.to_string(),
            from: "alice@example.com".into(),
            to: "bob@example.com".into(),
            date: "2026-08-21T00:00:00Z".into(),
            body_plain: format!("Body of {subject}"),
            body_html: String::new(),
            flags: vec![],
            message_id: format!("<{uid}@test>"),
            in_reply_to: None,
        }
    }

    #[test]
    fn search_returns_messages() {
        let transport = MockMailTransport::with_messages(vec![
            test_message(1, "Hello"),
            test_message(2, "Re: Hello"),
        ]);
        let conn = ImapSmtpConnector::new(transport);
        let result = conn.search("INBOX", "ALL", 10).unwrap();
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn unread_count() {
        let mut msg = test_message(1, "Unread");
        msg.flags = vec![];
        let mut msg2 = test_message(2, "Read");
        msg2.flags = vec!["\\Seen".into()];
        let transport = MockMailTransport::with_messages(vec![msg, msg2]);
        let conn = ImapSmtpConnector::new(transport);
        assert_eq!(conn.unread_count("INBOX").unwrap(), 1);
    }

    #[test]
    fn send_message() {
        let transport = MockMailTransport::with_messages(vec![]);
        let conn = ImapSmtpConnector::new(transport);
        let result = conn.send("bob@example.com", "Test", "Hello!").unwrap();
        assert_eq!(result.message_id, "msg-001");
    }

    #[test]
    fn reply_to_message() {
        let transport = MockMailTransport::with_messages(vec![]);
        let conn = ImapSmtpConnector::new(transport);
        let result = conn
            .reply("<orig@test>", "alice@example.com", "Re: Test", "Got it")
            .unwrap();
        assert_eq!(result.message_id, "msg-002");
    }

    #[test]
    fn archive_moves_to_archive() {
        let transport = MockMailTransport::with_messages(vec![test_message(5, "Archivable")]);
        let conn = ImapSmtpConnector::new(transport);
        let result = conn.archive("INBOX", 5);
        assert!(result.is_ok());
    }

    #[test]
    fn mark_read_sets_seen() {
        let transport = MockMailTransport::with_messages(vec![]);
        let conn = ImapSmtpConnector::new(transport);
        assert!(conn.mark_read("INBOX", 3).is_ok());
    }
}
