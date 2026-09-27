//! P42.1 — Microsoft Graph connector (spec F14 v2).
//!
//! Mail + calendar + OneDrive/SharePoint + Teams messages over the official
//! Graph endpoints (`graph.microsoft.com/v1.0`), driven through the injectable
//! [`HttpTransport`] seam so the full protocol logic is tested with mock
//! responses. Tokens come from the vault Auth Bridge (user OAuth, `VaultTokenRef`
//! — the connector holds a key id, never the bytes); 401 → refresh → retry
//! once via the [`TokenSource`] seam.
//!
//! Posture (P42.3): **read-only-first**. Reads pass through; writes
//! (`send_mail`, `create_calendar_event`) require a Guard-2-shaped
//! [`SendApproval`] verified by [`ReadFirstPolicy`] — single-use, bound to the
//! exact payload hash, never auto-approved.

use super::read_first::{ReadFirstPolicy, SendAction, SendApproval, SendBlocked, SendKind};
use super::{HttpTransport, TokenSource, TransportError, TransportErrorKind, VaultTokenRef};

/// Simplified mail message for agent consumption.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphMailMessage {
    pub id: String,
    pub subject: String,
    pub from: Option<String>,
    pub to: Vec<String>,
    pub received_at: Option<String>,
    pub body_preview: Option<String>,
    pub body_plain: Option<String>,
}

/// Simplified calendar event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphCalendarEvent {
    pub id: String,
    pub subject: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub location: Option<String>,
    pub is_online_meeting: bool,
}

/// OneDrive/SharePoint drive item (metadata + download URL).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphDriveItem {
    pub id: String,
    pub name: String,
    pub folder: bool,
    pub size: Option<u64>,
    pub download_url: Option<String>,
}

/// Teams chat + its messages.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphChat {
    pub id: String,
    pub topic: Option<String>,
    pub chat_type: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphChatMessage {
    pub id: String,
    pub from: Option<String>,
    pub body: Option<String>,
    pub created_at: Option<String>,
}

const GRAPH_API_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Microsoft Graph connector — stateless protocol logic over injected seams.
///
/// FIX-01: holds a [`VaultTokenRef`] + [`TokenSource`], never the token. The
/// value exists only inside the closure the transport is called from.
pub struct GraphConnector<T: HttpTransport, S: TokenSource> {
    transport: T,
    tokens: S,
    token_ref: VaultTokenRef,
    /// `me` or a user id / tenant-scoped principal.
    principal: String,
}

impl<T: HttpTransport, S: TokenSource> GraphConnector<T, S> {
    /// Bind the connector to a **vault reference**, not a token.
    pub fn new(transport: T, tokens: S, token_ref: VaultTokenRef) -> Self {
        Self {
            transport,
            tokens,
            token_ref,
            principal: "me".into(),
        }
    }

    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = principal.into();
        self
    }

    /// Run `f` with the live access token, in-custody (never returned).
    fn authenticated<R>(
        &self,
        f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
    ) -> Result<R, TransportError> {
        self.tokens.with_token(&self.token_ref, f)
    }

    /// Execute a GET with 401 → refresh → retry once.
    fn get_with_refresh(&mut self, url: &str) -> Result<Vec<u8>, TransportError> {
        let transport = &self.transport;
        match self.authenticated(&mut |t| transport.get(url, &[("Authorization", t)])) {
            Err(e) if e.kind == TransportErrorKind::Auth => {
                self.tokens.refresh(&self.token_ref, &mut |t| {
                    transport.get(url, &[("Authorization", t)])
                })
            }
            other => other,
        }
    }

    fn post_json_with_refresh(
        &mut self,
        url: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, TransportError> {
        let transport = &self.transport;
        match self.authenticated(&mut |t| transport.post_json(url, &[("Authorization", t)], body)) {
            Err(e) if e.kind == TransportErrorKind::Auth => {
                self.tokens.refresh(&self.token_ref, &mut |t| {
                    transport.post_json(url, &[("Authorization", t)], body)
                })
            }
            other => other,
        }
    }

    // ---- reads (read-only-first: free, no ticket) --------------------------

    /// `GET /me/messages?$top=N` — inbox mail, newest first.
    pub fn list_mail(&mut self, top: u32) -> Result<Vec<GraphMailMessage>, TransportError> {
        let url = format!(
            "{GRAPH_API_BASE}/{}/messages?$top={top}&$orderby=receivedDateTime%20desc",
            self.principal
        );
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph mail list: {e}"),
        })?;
        let items = v.get("value").cloned().unwrap_or(serde_json::json!([]));
        let msgs: Vec<GraphMailMessage> = items
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|m| GraphMailMessage {
                        id: m["id"].as_str().unwrap_or_default().into(),
                        subject: m["subject"].as_str().unwrap_or_default().into(),
                        from: m["from"]["emailAddress"]["address"]
                            .as_str()
                            .map(String::from),
                        to: m["toRecipients"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| t["emailAddress"]["address"].as_str())
                                    .map(String::from)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        received_at: m["receivedDateTime"].as_str().map(String::from),
                        body_preview: m["bodyPreview"].as_str().map(String::from),
                        body_plain: m["body"]["content"].as_str().map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(msgs)
    }

    /// `GET /me/messages/{id}` — one message.
    pub fn get_mail(&mut self, id: &str) -> Result<GraphMailMessage, TransportError> {
        let url = format!("{GRAPH_API_BASE}/{}/messages/{id}", self.principal);
        let raw = self.get_with_refresh(&url)?;
        let m: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph mail get: {e}"),
        })?;
        Ok(GraphMailMessage {
            id: m["id"].as_str().unwrap_or_default().into(),
            subject: m["subject"].as_str().unwrap_or_default().into(),
            from: m["from"]["emailAddress"]["address"]
                .as_str()
                .map(String::from),
            to: m["toRecipients"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t["emailAddress"]["address"].as_str())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            received_at: m["receivedDateTime"].as_str().map(String::from),
            body_preview: m["bodyPreview"].as_str().map(String::from),
            body_plain: m["body"]["content"].as_str().map(String::from),
        })
    }

    /// `GET /me/calendarView?startDateTime&endDateTime` — events in a window.
    pub fn list_calendar_events(
        &mut self,
        start: &str,
        end: &str,
    ) -> Result<Vec<GraphCalendarEvent>, TransportError> {
        let url = format!(
            "{GRAPH_API_BASE}/{}/calendarView?startDateTime={start}&endDateTime={end}",
            self.principal
        );
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph calendar: {e}"),
        })?;
        Ok(v.get("value")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|e| GraphCalendarEvent {
                        id: e["id"].as_str().unwrap_or_default().into(),
                        subject: e["subject"].as_str().unwrap_or_default().into(),
                        start: e["start"]["dateTime"].as_str().map(String::from),
                        end: e["end"]["dateTime"].as_str().map(String::from),
                        location: e["location"]["displayName"].as_str().map(String::from),
                        is_online_meeting: e["isOnlineMeeting"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `GET /me/drive/root/children` — OneDrive root listing.
    pub fn list_drive_children(&mut self) -> Result<Vec<GraphDriveItem>, TransportError> {
        let url = format!("{GRAPH_API_BASE}/{}/drive/root/children", self.principal);
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph drive: {e}"),
        })?;
        Ok(v.get("value")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|f| GraphDriveItem {
                        id: f["id"].as_str().unwrap_or_default().into(),
                        name: f["name"].as_str().unwrap_or_default().into(),
                        folder: f["folder"].is_object(),
                        size: f["size"].as_u64(),
                        download_url: f["@microsoft.graph.downloadUrl"].as_str().map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `GET /me/drive/items/{id}` — metadata (content via `download_url`).
    pub fn get_drive_file(&mut self, id: &str) -> Result<GraphDriveItem, TransportError> {
        let url = format!("{GRAPH_API_BASE}/{}/drive/items/{id}", self.principal);
        let raw = self.get_with_refresh(&url)?;
        let f: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph drive get: {e}"),
        })?;
        Ok(GraphDriveItem {
            id: f["id"].as_str().unwrap_or_default().into(),
            name: f["name"].as_str().unwrap_or_default().into(),
            folder: f["folder"].is_object(),
            size: f["size"].as_u64(),
            download_url: f["@microsoft.graph.downloadUrl"].as_str().map(String::from),
        })
    }

    /// `GET /me/chats` — Teams chats.
    pub fn list_chats(&mut self) -> Result<Vec<GraphChat>, TransportError> {
        let url = format!("{GRAPH_API_BASE}/{}/chats", self.principal);
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph chats: {e}"),
        })?;
        Ok(v.get("value")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|c| GraphChat {
                        id: c["id"].as_str().unwrap_or_default().into(),
                        topic: c["topic"].as_str().map(String::from),
                        chat_type: c["chatType"].as_str().unwrap_or_default().into(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `GET /me/chats/{id}/messages` — messages in one Teams chat.
    pub fn list_chat_messages(
        &mut self,
        chat_id: &str,
    ) -> Result<Vec<GraphChatMessage>, TransportError> {
        let url = format!(
            "{GRAPH_API_BASE}/{}/chats/{chat_id}/messages",
            self.principal
        );
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("graph chat messages: {e}"),
        })?;
        Ok(v.get("value")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|m| GraphChatMessage {
                        id: m["id"].as_str().unwrap_or_default().into(),
                        from: m["from"]["user"]["displayName"].as_str().map(String::from),
                        body: m["body"]["content"].as_str().map(String::from),
                        created_at: m["createdDateTime"].as_str().map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    // ---- writes (Guard-2-gated: single-use approval bound to the payload) --

    /// `POST /me/sendMail` — requires a verified [`SendApproval`].
    pub fn send_mail(
        &mut self,
        policy: &mut ReadFirstPolicy,
        to: &[String],
        subject: &str,
        body: &str,
        open_world: bool,
        approval: Option<&SendApproval>,
    ) -> Result<String, SendBlocked> {
        let action = SendAction {
            kind: SendKind::Email {
                to: to.join(", "),
                subject: subject.to_string(),
            },
            args_hash: SendAction::hash_payload(&[&to.join(";"), subject, body]),
            open_world,
        };
        policy.approve_before_send(&action, approval)?;
        let payload = serde_json::json!({
            "message": {
                "subject": subject,
                "body": { "contentType": "text", "content": body },
                "toRecipients": to.iter().map(|t| serde_json::json!({
                    "emailAddress": { "address": t }
                })).collect::<Vec<_>>(),
            },
            "saveToSentItems": true,
        });
        let url = format!("{GRAPH_API_BASE}/{}/sendMail", self.principal);
        let body = serde_json::to_vec(&payload).map_err(|_e| SendBlocked::NoApproval)?;
        self.post_json_with_refresh(&url, &body)
            .map_err(|_e| SendBlocked::NoApproval)?;
        Ok(action.args_hash)
    }

    /// `POST /me/events` — requires a verified [`SendApproval`].
    pub fn create_calendar_event(
        &mut self,
        policy: &mut ReadFirstPolicy,
        subject: &str,
        start: &str,
        end: &str,
        approval: Option<&SendApproval>,
    ) -> Result<String, SendBlocked> {
        let action = SendAction {
            kind: SendKind::Calendar {
                summary: subject.to_string(),
            },
            args_hash: SendAction::hash_payload(&[subject, start, end]),
            open_world: false,
        };
        policy.approve_before_send(&action, approval)?;
        let payload = serde_json::json!({
            "subject": subject,
            "start": { "dateTime": start, "timeZone": "UTC" },
            "end": { "dateTime": end, "timeZone": "UTC" },
        });
        let url = format!("{GRAPH_API_BASE}/{}/events", self.principal);
        let body = serde_json::to_vec(&payload).map_err(|_e| SendBlocked::NoApproval)?;
        self.post_json_with_refresh(&url, &body)
            .map_err(|_e| SendBlocked::NoApproval)?;
        Ok(action.args_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MockTransport {
        responses: std::cell::RefCell<std::collections::VecDeque<(Vec<u8>, TransportErrorKind)>>,
        requests: std::sync::Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn ok(json: serde_json::Value) -> Self {
            let mut q = std::collections::VecDeque::new();
            q.push_back((
                serde_json::to_vec(&json).unwrap(),
                TransportErrorKind::Other,
            ));
            Self {
                responses: std::cell::RefCell::new(q),
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn auth_then_ok(ok: serde_json::Value) -> Self {
            let mut q = std::collections::VecDeque::new();
            q.push_back((Vec::new(), TransportErrorKind::Auth));
            q.push_back((serde_json::to_vec(&ok).unwrap(), TransportErrorKind::Other));
            Self {
                responses: std::cell::RefCell::new(q),
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn next(&self) -> Option<(Vec<u8>, TransportErrorKind)> {
            self.responses.borrow_mut().pop_front()
        }
    }

    impl HttpTransport for MockTransport {
        fn post_json(
            &self,
            url: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<Vec<u8>, TransportError> {
            self.requests.lock().unwrap().push(format!("POST {url}"));
            match self.next() {
                Some((_, TransportErrorKind::Auth)) => Err(TransportError {
                    kind: TransportErrorKind::Auth,
                    message: "401".into(),
                }),
                Some((bytes, _)) => Ok(bytes),
                None => Err(TransportError {
                    kind: TransportErrorKind::InvalidResponse,
                    message: "no mock response".into(),
                }),
            }
        }
        fn get(&self, url: &str, _headers: &[(&str, &str)]) -> Result<Vec<u8>, TransportError> {
            self.requests.lock().unwrap().push(format!("GET {url}"));
            match self.next() {
                Some((_, TransportErrorKind::Auth)) => Err(TransportError {
                    kind: TransportErrorKind::Auth,
                    message: "401".into(),
                }),
                Some((bytes, _)) => Ok(bytes),
                None => Err(TransportError {
                    kind: TransportErrorKind::InvalidResponse,
                    message: "no mock response".into(),
                }),
            }
        }
    }

    /// FIX-01: the mock is a *source*, not a token holder — it resolves the
    /// value inside the closure, exactly like the vault-backed source.
    struct NoRefresh;
    impl TokenSource for NoRefresh {
        fn with_token<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("Bearer tok")
        }
        fn refresh<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("Bearer new-token")
        }
    }

    /// The stale-token variant: a 401 must still trigger exactly one refresh.
    struct StaleTokens;
    impl TokenSource for StaleTokens {
        fn with_token<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("Bearer stale")
        }
        fn refresh<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("Bearer new-token")
        }
    }

    fn graph_tokens() -> VaultTokenRef {
        VaultTokenRef::new("k-graph", "microsoft-graph")
    }

    fn mail_response() -> serde_json::Value {
        serde_json::json!({
            "value": [{
                "id": "m1",
                "subject": "Re: Q3 plan",
                "from": { "emailAddress": { "address": "boss@example.com" } },
                "toRecipients": [{ "emailAddress": { "address": "me@example.com" } }],
                "receivedDateTime": "2026-08-25T09:00:00Z",
                "bodyPreview": "Sounds good",
                "body": { "content": "Sounds good. Ship it." }
            }]
        })
    }

    #[test]
    fn graph_mail_list_parses_and_reads_are_free() {
        let t = MockTransport::ok(mail_response());
        let mut c = GraphConnector::new(t, NoRefresh, graph_tokens());
        let msgs = c.list_mail(10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].subject, "Re: Q3 plan");
        assert_eq!(msgs[0].from.as_deref(), Some("boss@example.com"));
        assert_eq!(msgs[0].body_plain.as_deref(), Some("Sounds good. Ship it."));
    }

    #[test]
    fn graph_401_refreshes_and_retries_once() {
        let t = MockTransport::auth_then_ok(mail_response());
        let mut c = GraphConnector::new(t, StaleTokens, graph_tokens());
        let msgs = c.list_mail(10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, "m1");
    }

    #[test]
    fn graph_send_mail_requires_a_matching_single_use_approval() {
        let t = MockTransport::ok(serde_json::json!({}));
        let mut c = GraphConnector::new(t, NoRefresh, graph_tokens());
        let mut policy = ReadFirstPolicy::new();
        let to = vec!["a@example.com".into()];
        // no approval → blocked honestly
        assert!(matches!(
            c.send_mail(&mut policy, &to, "hi", "body", false, None),
            Err(SendBlocked::NoApproval)
        ));
        // matching approval → allowed (hash over the exact payload the
        // connector hashes: to.join(";"), subject, body)
        let action = SendAction {
            kind: SendKind::Email {
                to: "a@example.com".into(),
                subject: "hi".into(),
            },
            args_hash: SendAction::hash_payload(&["a@example.com", "hi", "body"]),
            open_world: false,
        };
        let approval = SendApproval {
            ticket_id: "t-g1".into(),
            bound_args_hash: action.args_hash.clone(),
            reason: "user confirmed".into(),
        };
        let hash = c
            .send_mail(&mut policy, &to, "hi", "body", false, Some(&approval))
            .unwrap();
        assert_eq!(hash, action.args_hash);
        // single-use: replaying the same ticket fails
        assert!(matches!(
            c.send_mail(&mut policy, &to, "hi", "body", false, Some(&approval)),
            Err(SendBlocked::AlreadyUsed(_))
        ));
    }

    #[test]
    fn graph_calendar_view_and_drive_and_chats_parse() {
        let empty = serde_json::json!({ "value": [] });
        let t = MockTransport {
            responses: std::cell::RefCell::new(std::collections::VecDeque::from(vec![
                (
                    serde_json::to_vec(&empty).unwrap(),
                    TransportErrorKind::Other,
                ),
                (
                    serde_json::to_vec(&empty).unwrap(),
                    TransportErrorKind::Other,
                ),
                (
                    serde_json::to_vec(&empty).unwrap(),
                    TransportErrorKind::Other,
                ),
            ])),
            requests: std::sync::Mutex::new(Vec::new()),
        };
        let mut c = GraphConnector::new(t, NoRefresh, graph_tokens());
        assert!(
            c.list_calendar_events("2026-08-01T00:00:00Z", "2026-09-01T00:00:00Z")
                .unwrap()
                .is_empty()
        );
        assert!(c.list_drive_children().unwrap().is_empty());
        assert!(c.list_chats().unwrap().is_empty());
    }
}
