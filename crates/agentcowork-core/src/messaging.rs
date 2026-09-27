//! Messaging Bridges (P6.9 — F13, doc 36 §B, doc 39 §B1, doc 61 §5).
//!
//! Desktop-first scope (ARCH/11 R-1): the agent lives in the *open desktop
//! app* — messages arrive as in-app cards, no headless 24×7 daemon. This
//! module is the **adapter interface** (message-in → agent loop → reply-out)
//! plus a deterministic stub for round-trip testing. Live WhatsApp/Telegram/
//! email transports are the adapter impls (network + credentials), not this
//! file; the memory-reuse + dedupe contracts are enforced here regardless of
//! transport.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A transport-level HTTP call seam (P6.9 adapters). The live adapters use
/// `ureq`; tests inject a loopback mock so the adapter logic (message
/// normalization, dedupe keys, delivery) is exercised without network.
pub trait HttpTransport {
    /// POST `body` to `url` with `content_type`; returns the response body.
    fn post(&mut self, url: &str, content_type: &str, body: &str)
    -> Result<String, MessagingError>;
}

/// `ureq`-backed transport — the production path (outbound network is the
/// adapter's only network touch; credentials never enter the sidecar).
///
/// FIX-09: the destination floor is enforced here, at the socket. The webhook
/// URL is user-configured (a channel the user connected), so the desktop
/// default policy applies — loopback and public hosts pass, the LAN and the
/// always-refused ranges (link-local/cloud metadata, multicast, reserved) do
/// not. A denial is returned as a delivery failure with the floor's reason and
/// there is no direct-client fallback.
#[derive(Debug, Default)]
pub struct UreqTransport;

impl HttpTransport for UreqTransport {
    fn post(
        &mut self,
        url: &str,
        content_type: &str,
        body: &str,
    ) -> Result<String, MessagingError> {
        if let Err(denied) =
            agentcowork_guard::netfloor::preflight_url(url, agentcowork_guard::NetPolicy::default())
        {
            return Err(MessagingError::EgressDenied(denied.to_string()));
        }
        let resp = ureq::post(url)
            .set("Content-Type", content_type)
            .send_string(body)
            .map_err(|_| MessagingError::DeliveryFailed)?;
        resp.into_string()
            .map_err(|_| MessagingError::DeliveryFailed)
    }
}

/// A transport whose deliveries are recorded (tests + in-app card delivery).
#[derive(Debug, Default)]
pub struct RecordingTransport {
    pub calls: Vec<(String, String, String)>,
}

impl HttpTransport for RecordingTransport {
    fn post(
        &mut self,
        url: &str,
        content_type: &str,
        body: &str,
    ) -> Result<String, MessagingError> {
        self.calls
            .push((url.to_string(), content_type.to_string(), body.to_string()));
        // A minimal Telegram-style `{ "ok": true }` response.
        Ok(r#"{"ok":true}"#.into())
    }
}

/// A normalized inbound message (channel-agnostic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Channel id (whatsapp/telegram/email/…).
    pub channel: String,
    /// Sender id (phone/email/username).
    pub from: String,
    /// The message text.
    pub text: String,
    /// Channel-supplied idempotency key (message id) for dedupe.
    pub message_id: String,
    /// Optional conversation/session id for memory reuse.
    pub conversation_id: Option<String>,
}

/// A normalized outbound reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundReply {
    pub to: String,
    pub text: String,
    /// The session id this reply belongs to (memory reuse key).
    pub conversation_id: String,
}

/// The message-in → agent-loop → reply-out adapter contract (F13).
pub trait MessageAdapter {
    /// Human label (for the hub/UI).
    fn channel(&self) -> &str;

    /// Deliver a reply. Returns the channel's delivery id.
    fn send(&mut self, reply: &OutboundReply) -> Result<String, MessagingError>;

    /// Poll for new inbound messages (empty = none). Implementations are
    /// expected to return *only* messages not yet seen (the dispatcher also
    /// dedupes by [`InboundMessage::message_id`]).
    fn receive(&mut self) -> Result<Vec<InboundMessage>, MessagingError>;
}

/// A deterministic stub adapter (F13 test path + in-app card delivery).
#[derive(Debug, Default)]
pub struct StubAdapter {
    pub channel_name: String,
    pub inbox: Vec<InboundMessage>,
    pub sent: Vec<OutboundReply>,
    pub fail_send: bool,
}

impl StubAdapter {
    pub fn new(channel: &str) -> Self {
        Self {
            channel_name: channel.to_string(),
            inbox: Vec::new(),
            sent: Vec::new(),
            fail_send: false,
        }
    }
}

impl MessageAdapter for StubAdapter {
    fn channel(&self) -> &str {
        &self.channel_name
    }
    fn send(&mut self, reply: &OutboundReply) -> Result<String, MessagingError> {
        if self.fail_send {
            return Err(MessagingError::DeliveryFailed);
        }
        self.sent.push(reply.clone());
        Ok(format!("delivery-{}", self.sent.len()))
    }
    fn receive(&mut self) -> Result<Vec<InboundMessage>, MessagingError> {
        Ok(std::mem::take(&mut self.inbox))
    }
}

/// Dedupe inbound messages across a channel (doc 39 §B1 run_policy/dedupe).
#[derive(Debug, Default)]
pub struct Dedupe {
    seen: HashSet<String>,
}

impl Dedupe {
    pub fn new() -> Self {
        Self::default()
    }
    /// Returns the messages not seen before (and marks them seen).
    pub fn filter(&mut self, msgs: Vec<InboundMessage>) -> Vec<InboundMessage> {
        msgs.into_iter()
            .filter(|m| self.seen.insert(m.message_id.clone()))
            .collect()
    }
}

/// The message-in → agent-loop → reply-out dispatcher (in-app card delivery).
pub struct MessageDispatcher {
    /// channel name → adapter.
    adapters: Vec<Box<dyn MessageAdapter>>,
    dedupe: Dedupe,
    /// conversation id → remembered context (memory reuse across sessions).
    memory: HashMap<String, Vec<String>>,
}

impl MessageDispatcher {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
            dedupe: Dedupe::new(),
            memory: HashMap::new(),
        }
    }
}

impl Default for MessageDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageDispatcher {
    pub fn register(&mut self, adapter: Box<dyn MessageAdapter>) {
        self.adapters.push(adapter);
    }

    /// P51.20 — approve-then-read for a messaging thread. A conversation stays
    /// unreadable until a ticket id is recorded for that exact id.
    pub fn dispatch_approved<F>(
        &mut self,
        approved_threads: &std::collections::HashSet<String>,
        mut handler: F,
    ) -> (Vec<OutboundReply>, Vec<String>)
    where
        F: FnMut(&InboundMessage) -> String,
    {
        let mut replies = Vec::new();
        let mut held = Vec::new();
        for adapter in &mut self.adapters {
            let inbound = match adapter.receive() {
                Ok(m) => m,
                Err(_) => continue,
            };
            for msg in self.dedupe.filter(inbound) {
                let conversation = msg
                    .conversation_id
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", msg.channel, msg.from));
                if !approved_threads.contains(&conversation) {
                    held.push(conversation);
                    continue;
                }
                let text = handler(&msg);
                self.memory
                    .entry(conversation.clone())
                    .or_default()
                    .push(msg.text.clone());
                let reply = OutboundReply {
                    to: msg.from.clone(),
                    text,
                    conversation_id: conversation,
                };
                if adapter.send(&reply).is_ok() {
                    replies.push(reply);
                }
            }
        }
        (replies, held)
    }

    /// Drain all adapters, dedupe, and route each message through a handler
    /// closure (the agent loop). The handler returns a reply, which is
    /// delivered back through the same adapter.
    pub fn dispatch<F>(&mut self, handler: F) -> Vec<OutboundReply>
    where
        F: Fn(&InboundMessage) -> String,
    {
        let mut replies = Vec::new();
        for adapter in &mut self.adapters {
            let inbound = match adapter.receive() {
                Ok(m) => m,
                Err(_) => continue,
            };
            for msg in self.dedupe.filter(inbound) {
                let text = handler(&msg);
                let conversation = msg
                    .conversation_id
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", msg.channel, msg.from));
                // Memory reuse: append to the conversation's remembered log.
                self.memory
                    .entry(conversation.clone())
                    .or_default()
                    .push(msg.text.clone());
                let reply = OutboundReply {
                    to: msg.from.clone(),
                    text,
                    conversation_id: conversation,
                };
                if adapter.send(&reply).is_ok() {
                    replies.push(reply);
                }
            }
        }
        replies
    }

    /// The remembered conversation history for memory reuse (F13).
    pub fn remembered(&self, conversation_id: &str) -> Option<&Vec<String>> {
        self.memory.get(conversation_id)
    }
}

/// A durable reminder intent that can be delivered by any registered
/// messaging adapter when its due time arrives. The scheduler owns wake-ups;
/// this type owns dedupe and exact message identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReminder {
    pub id: String,
    pub channel: String,
    pub to: String,
    pub text: String,
    pub due_at_ms: i64,
    pub delivered: bool,
}

/// Small deterministic reminder queue used by the scheduler integration and
/// by the in-app messaging surface. It is intentionally transport-neutral.
#[derive(Debug, Default)]
pub struct ReminderQueue {
    reminders: HashMap<String, MessageReminder>,
}

impl ReminderQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, reminder: MessageReminder) {
        self.reminders.insert(reminder.id.clone(), reminder);
    }

    pub fn due(&self, now_ms: i64) -> Vec<MessageReminder> {
        let mut due: Vec<_> = self
            .reminders
            .values()
            .filter(|r| !r.delivered && r.due_at_ms <= now_ms)
            .cloned()
            .collect();
        due.sort_by(|a, b| (a.due_at_ms, &a.id).cmp(&(b.due_at_ms, &b.id)));
        due
    }

    pub fn mark_delivered(&mut self, id: &str) -> bool {
        if let Some(reminder) = self.reminders.get_mut(id) {
            reminder.delivered = true;
            return true;
        }
        false
    }
}

/// Telegram adapter (P6.9): Bot-API long-polling over the injected HTTP
/// transport. `base_url` defaults to `https://api.telegram.org`; tests point
/// it at a loopback mock. The bot token lives in the vault, never here.
#[derive(Debug)]
pub struct TelegramAdapter<T: HttpTransport> {
    pub channel_name: String,
    pub base_url: String,
    pub token: String,
    transport: T,
    /// Last processed update id (Bot-API offset semantics).
    offset: i64,
}

impl<T: HttpTransport> TelegramAdapter<T> {
    pub fn new(token: impl Into<String>, transport: T) -> Self {
        Self {
            channel_name: "telegram".into(),
            base_url: "https://api.telegram.org".into(),
            token: token.into(),
            transport,
            offset: 0,
        }
    }

    /// Poll `getUpdates` and normalize `message` updates into inbound
    /// messages. A `chat.id` + `message_id` are the dedupe keys.
    pub fn poll(&mut self) -> Result<Vec<InboundMessage>, MessagingError> {
        let url = format!(
            "{}/bot{}/getUpdates?offset={}&timeout=1",
            self.base_url, self.token, self.offset
        );
        let body = self.transport.post(&url, "application/json", "")?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|_| MessagingError::DeliveryFailed)?;
        let mut out = Vec::new();
        if let Some(updates) = v.get("result").and_then(|r| r.as_array()) {
            for u in updates {
                let Some(update_id) = u.get("update_id").and_then(|x| x.as_i64()) else {
                    continue;
                };
                // Client-side dedupe: never reprocess an update id we have
                // already advanced past (the server may not honor `offset`).
                if update_id < self.offset {
                    continue;
                }
                self.offset = self.offset.max(update_id + 1);
                let Some(msg) = u.get("message") else {
                    continue;
                };
                let Some(chat_id) = msg
                    .get("chat")
                    .and_then(|c| c.get("id"))
                    .and_then(|x| x.as_i64())
                else {
                    continue;
                };
                let Some(text) = msg.get("text").and_then(|x| x.as_str()) else {
                    continue;
                };
                let message_id = msg
                    .get("message_id")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(update_id)
                    .to_string();
                out.push(InboundMessage {
                    channel: "telegram".into(),
                    from: format!("tg:{chat_id}"),
                    text: text.to_string(),
                    message_id,
                    conversation_id: Some(format!("tg:{chat_id}")),
                });
            }
        }
        Ok(out)
    }
}

impl<T: HttpTransport> MessageAdapter for TelegramAdapter<T> {
    fn channel(&self) -> &str {
        &self.channel_name
    }
    fn send(&mut self, reply: &OutboundReply) -> Result<String, MessagingError> {
        let chat_id = reply
            .to
            .strip_prefix("tg:")
            .ok_or(MessagingError::DeliveryFailed)?;
        let url = format!("{}/bot{}/sendMessage", self.base_url, self.token);
        let body = serde_json::json!({
            "chat_id": chat_id.parse::<i64>().map_err(|_| MessagingError::DeliveryFailed)?,
            "text": reply.text,
        })
        .to_string();
        self.transport.post(&url, "application/json", &body)?;
        Ok(format!("tg-delivery:{chat_id}"))
    }
    fn receive(&mut self) -> Result<Vec<InboundMessage>, MessagingError> {
        self.poll()
    }
}

/// WhatsApp transport seam (P6.9): the Secure OpenClaw WhatsApp-Web protocol
/// needs a live session + credentials; the adapter contract is implemented
/// here and the wire transport is injected (stub in tests, WebSocket client
/// in production).
pub trait WhatsappTransport {
    /// Deliver one outbound message (channel delivery id on success).
    fn send_text(&mut self, to: &str, text: &str) -> Result<String, MessagingError>;
    /// Return any inbound messages not yet seen.
    fn receive_messages(&mut self) -> Result<Vec<InboundMessage>, MessagingError>;
}

/// WhatsApp adapter over the transport seam (P6.9).
#[derive(Debug)]
pub struct WhatsappAdapter<T: WhatsappTransport> {
    pub channel_name: String,
    transport: T,
}

impl<T: WhatsappTransport> WhatsappAdapter<T> {
    pub fn new(transport: T) -> Self {
        Self {
            channel_name: "whatsapp".into(),
            transport,
        }
    }
}

impl<T: WhatsappTransport> MessageAdapter for WhatsappAdapter<T> {
    fn channel(&self) -> &str {
        &self.channel_name
    }
    fn send(&mut self, reply: &OutboundReply) -> Result<String, MessagingError> {
        self.transport.send_text(&reply.to, &reply.text)
    }
    fn receive(&mut self) -> Result<Vec<InboundMessage>, MessagingError> {
        self.transport.receive_messages()
    }
}

/// Email adapter (P6.9, reuses F14 plumbing): email-in → agent → reply over
/// a [`crate::email::Mailbox`]. Inbound = search/read; outbound = the
/// mailbox send path (guard-ticketed at the service layer).
#[derive(Debug)]
pub struct EmailAdapter<M: crate::email::Mailbox> {
    pub channel_name: String,
    mailbox: M,
    /// Normalized sender for inbound messages.
    pub self_address: String,
}

impl<M: crate::email::Mailbox> EmailAdapter<M> {
    pub fn new(mailbox: M, self_address: impl Into<String>) -> Self {
        Self {
            channel_name: "email".into(),
            mailbox,
            self_address: self_address.into(),
        }
    }
}

impl<M: crate::email::Mailbox> MessageAdapter for EmailAdapter<M> {
    fn channel(&self) -> &str {
        &self.channel_name
    }
    fn send(&mut self, reply: &OutboundReply) -> Result<String, MessagingError> {
        let id = self
            .mailbox
            .send(crate::email::OutgoingEmail {
                to: vec![reply.to.clone()],
                subject: "Re: your message".into(),
                body: reply.text.clone(),
                in_reply_to: None,
            })
            .map_err(|_| MessagingError::DeliveryFailed)?;
        Ok(format!("email-delivery:{id}"))
    }
    fn receive(&mut self) -> Result<Vec<InboundMessage>, MessagingError> {
        // Inbound = everything addressed to self (deterministic stub: the
        // mailbox is the inbox; a live IMAP impl maps folder reads here).
        let all = self
            .mailbox
            .search("")
            .map_err(|_| MessagingError::DeliveryFailed)?;
        Ok(all
            .into_iter()
            .filter(|m| m.to.contains(&self.self_address))
            .map(|m| InboundMessage {
                channel: "email".into(),
                from: m.from,
                text: format!("{} — {}", m.subject, m.body),
                message_id: m.id,
                conversation_id: m.thread_id,
            })
            .collect())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MessagingError {
    #[error("message delivery failed")]
    DeliveryFailed,
    /// FIX-09: the destination floor refused the outbound webhook. The reason
    /// is the floor's own stable token, so the failure is auditable and never
    /// degrades into a silent retry through another client.
    #[error("egress denied: {0}")]
    EgressDenied(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_round_trip_delivers_reply() {
        let mut stub = StubAdapter::new("telegram");
        stub.inbox.push(InboundMessage {
            channel: "telegram".into(),
            from: "u1".into(),
            text: "hello".into(),
            message_id: "m1".into(),
            conversation_id: Some("c1".into()),
        });
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(stub));
        let replies = dispatcher.dispatch(|m| format!("echo: {}", m.text));
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, "echo: hello");
        assert_eq!(replies[0].to, "u1");
    }

    #[test]
    fn thread_read_waits_for_approval() {
        let mut stub = StubAdapter::new("slack");
        stub.inbox.push(InboundMessage {
            channel: "slack".into(),
            from: "u1".into(),
            text: "secret thread".into(),
            message_id: "m2".into(),
            conversation_id: Some("thread-9".into()),
        });
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(stub));
        let (replies, held) =
            dispatcher.dispatch_approved(&std::collections::HashSet::new(), |_| "seen".into());
        assert!(replies.is_empty());
        assert_eq!(held, vec!["thread-9".to_string()]);
    }

    #[test]
    fn dedupe_filters_repeated_message_ids() {
        let mut stub = StubAdapter::new("whatsapp");
        for i in 0..3 {
            stub.inbox.push(InboundMessage {
                channel: "whatsapp".into(),
                from: "u".into(),
                text: format!("m{i}"),
                message_id: "dup".into(), // same id
                conversation_id: None,
            });
        }
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(stub));
        let replies = dispatcher.dispatch(|m| m.text.clone());
        assert_eq!(replies.len(), 1, "duplicate message ids collapse");
    }

    #[test]
    fn memory_reuse_across_sessions() {
        let mut stub = StubAdapter::new("email");
        stub.inbox.push(InboundMessage {
            channel: "email".into(),
            from: "a@b.test".into(),
            text: "first".into(),
            message_id: "m1".into(),
            conversation_id: Some("conv-1".into()),
        });
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(stub));
        dispatcher.dispatch(|m| m.text.clone());
        let remembered = dispatcher.remembered("conv-1").unwrap();
        assert_eq!(remembered, &vec!["first".to_string()]);
    }

    #[test]
    fn reminders_are_ordered_deduped_and_one_shot() {
        let mut queue = ReminderQueue::new();
        queue.upsert(MessageReminder {
            id: "later".into(),
            channel: "telegram".into(),
            to: "u".into(),
            text: "later".into(),
            due_at_ms: 20,
            delivered: false,
        });
        queue.upsert(MessageReminder {
            id: "now".into(),
            channel: "telegram".into(),
            to: "u".into(),
            text: "now".into(),
            due_at_ms: 10,
            delivered: false,
        });
        // Upsert is idempotent by reminder id.
        queue.upsert(MessageReminder {
            id: "now".into(),
            channel: "telegram".into(),
            to: "u".into(),
            text: "updated".into(),
            due_at_ms: 10,
            delivered: false,
        });
        assert_eq!(
            queue
                .due(10)
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["now"]
        );
        assert!(queue.mark_delivered("now"));
        assert!(queue.due(100).iter().all(|r| r.id != "now"));
    }

    // -- P6.9 adapter close-out ---------------------------------------------

    struct StubWhatsapp {
        inbox: Vec<InboundMessage>,
        sent: Vec<String>,
    }

    impl WhatsappTransport for StubWhatsapp {
        fn send_text(&mut self, to: &str, text: &str) -> Result<String, MessagingError> {
            self.sent.push(format!("{to}: {text}"));
            Ok(format!("wa:{}", self.sent.len()))
        }
        fn receive_messages(&mut self) -> Result<Vec<InboundMessage>, MessagingError> {
            Ok(std::mem::take(&mut self.inbox))
        }
    }

    #[test]
    fn whatsapp_adapter_round_trips_over_stub_transport() {
        let wa = WhatsappAdapter::new(StubWhatsapp {
            inbox: vec![InboundMessage {
                channel: "whatsapp".into(),
                from: "+1555".into(),
                text: "hi".into(),
                message_id: "w1".into(),
                conversation_id: Some("wa:+1555".into()),
            }],
            sent: Vec::new(),
        });
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(wa));
        let replies = dispatcher.dispatch(|m| format!("echo: {}", m.text));
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, "echo: hi");
    }

    #[test]
    fn telegram_adapter_normalizes_updates_and_delivers() {
        // The poll path needs a scripted body; delivery uses the recording
        // transport's canned `{ "ok": true }` response.
        struct Scripted {
            body: String,
        }
        impl HttpTransport for Scripted {
            fn post(
                &mut self,
                _url: &str,
                _ct: &str,
                _body: &str,
            ) -> Result<String, MessagingError> {
                Ok(self.body.clone())
            }
        }
        let body = r#"{"ok":true,"result":[
            {"update_id":1,"message":{"message_id":10,"chat":{"id":42},"text":"hello"}},
            {"update_id":2,"message":{"message_id":11,"chat":{"id":43},"text":"world"}}
        ]}"#;
        let mut tg = TelegramAdapter::new("tok", Scripted { body: body.into() });
        let msgs = tg.poll().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].from, "tg:42");
        assert_eq!(msgs[0].text, "hello");
        assert_eq!(msgs[0].message_id, "10");
        // Offset advanced past update 2 → no duplicates on next poll.
        assert_eq!(tg.offset, 3);
        assert!(tg.poll().unwrap().is_empty());
        // Delivery via the recording transport.
        let mut tg2 = TelegramAdapter::new("tok", RecordingTransport::default());
        let id = tg2
            .send(&OutboundReply {
                to: "tg:42".into(),
                text: "re: hello".into(),
                conversation_id: "tg:42".into(),
            })
            .unwrap();
        assert_eq!(id, "tg-delivery:42");
    }

    #[test]
    fn email_adapter_round_trips_via_mailbox() {
        let mut mailbox = crate::email::InMemoryMailbox::new();
        let id = mailbox.seed("alice@x.test", "Subject", "Body");
        // The seed is addressed to me@local — make the adapter's self match.
        let mut adapter = EmailAdapter::new(mailbox, "me@local");
        let inbound = adapter.receive().unwrap();
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].message_id, id);
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(adapter));
        let replies = dispatcher.dispatch(|m| format!("echo: {}", m.text));
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].to, "alice@x.test");
        assert!(replies[0].text.contains("echo:"));
    }

    #[test]
    fn failed_send_is_swallowed_but_other_messages_flow() {
        let mut failing = StubAdapter::new("telegram");
        failing.fail_send = true;
        failing.inbox.push(InboundMessage {
            channel: "telegram".into(),
            from: "u".into(),
            text: "hi".into(),
            message_id: "m1".into(),
            conversation_id: None,
        });
        let mut ok = StubAdapter::new("whatsapp");
        ok.inbox.push(InboundMessage {
            channel: "whatsapp".into(),
            from: "v".into(),
            text: "yo".into(),
            message_id: "m2".into(),
            conversation_id: None,
        });
        let mut dispatcher = MessageDispatcher::new();
        dispatcher.register(Box::new(failing));
        dispatcher.register(Box::new(ok));
        let replies = dispatcher.dispatch(|m| m.text.clone());
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, "yo");
    }
}
