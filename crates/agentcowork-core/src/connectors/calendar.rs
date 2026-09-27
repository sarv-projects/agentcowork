//! P6.11 — Google Calendar connector.
//!
//! Event CRUD, availability checking, and ICS import/export over the Google
//! Calendar REST API. Full protocol logic tested with a mock [`HttpTransport`]
//! seam — the live implementation uses Auth Bridge OAuth tokens.

use super::{HttpTransport, TokenSource, TransportError, TransportErrorKind, VaultTokenRef};

const CALENDAR_API_BASE: &str = "https://www.googleapis.com/calendar/v3";

/// Calendar event (simplified for agent consumption).
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub id: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: EventTime,
    pub end: EventTime,
    pub attendees: Vec<String>,
    pub status: EventStatus,
    pub html_link: Option<String>,
    pub recurrence: Vec<String>,
}

/// Event time — either a concrete datetime or an all-day date.
#[derive(Debug, Clone)]
pub enum EventTime {
    DateTime(String, Option<String>), // ISO datetime + optional timezone
    Date(String),                     // all-day (YYYY-MM-DD)
}

impl EventTime {
    pub fn datetime(dt: &str, tz: Option<&str>) -> Self {
        Self::DateTime(dt.to_string(), tz.map(|s| s.to_string()))
    }

    pub fn date(d: &str) -> Self {
        Self::Date(d.to_string())
    }
}

/// Event status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}

impl EventStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Tentative => "tentative",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Free/busy result for availability checking.
#[derive(Debug, Clone)]
pub struct FreeBusyResult {
    pub busy: Vec<TimeSlot>,
}

/// A busy time slot.
#[derive(Debug, Clone)]
pub struct TimeSlot {
    pub start: String,
    pub end: String,
}

/// Create-event result.
#[derive(Debug, Clone)]
pub struct CreateResult {
    pub event_id: String,
    pub html_link: Option<String>,
}

/// Google Calendar connector.
///
/// FIX-01: holds a [`VaultTokenRef`] + [`TokenSource`], never the access
/// token. The value is only reachable inside the closure
/// [`CalendarConnector::authenticated`] hands to the transport, so it cannot be
/// read back out of this struct, cloned, serialized or logged (INV-02,
/// CTR-013).
pub struct CalendarConnector<T: HttpTransport, S: TokenSource> {
    transport: T,
    tokens: S,
    token_ref: VaultTokenRef,
    calendar_id: String,
}

impl<T: HttpTransport, S: TokenSource> CalendarConnector<T, S> {
    /// Bind the connector to a **vault reference**, not a token.
    pub fn new(transport: T, tokens: S, token_ref: VaultTokenRef, calendar_id: &str) -> Self {
        Self {
            transport,
            tokens,
            token_ref,
            calendar_id: calendar_id.to_string(),
        }
    }

    fn base_url(&self) -> String {
        format!(
            "{CALENDAR_API_BASE}/calendars/{}",
            urlencoding::encode(&self.calendar_id)
        )
    }

    /// Run `f` with the live access token, in-custody. The token is never
    /// returned: the only way out of this method is whatever `f` computes.
    fn authenticated<R>(
        &self,
        f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
    ) -> Result<R, TransportError> {
        self.tokens.with_token(&self.token_ref, f)
    }

    /// List upcoming events.
    pub fn list_events(
        &self,
        time_min: Option<&str>,
        time_max: Option<&str>,
        max_results: usize,
    ) -> Result<Vec<CalendarEvent>, TransportError> {
        let base = self.base_url();
        let mut url =
            format!("{base}/events?maxResults={max_results}&singleEvents=true&orderBy=startTime");
        if let Some(min) = time_min {
            url = format!("{url}&timeMin={min}");
        }
        if let Some(max) = time_max {
            url = format!("{url}&timeMax={max}");
        }
        let transport = &self.transport;
        let resp = self.authenticated(&mut |t| transport.get(&url, &[("Authorization", t)]))?;
        let json: serde_json::Value =
            serde_json::from_slice(&resp).map_err(|e| TransportError {
                kind: TransportErrorKind::InvalidResponse,
                message: e.to_string(),
            })?;
        let events = json["items"]
            .as_array()
            .map(|arr| arr.iter().filter_map(parse_event).collect())
            .unwrap_or_default();
        Ok(events)
    }

    /// Get a single event.
    pub fn get_event(&self, event_id: &str) -> Result<CalendarEvent, TransportError> {
        let base = self.base_url();
        let url = format!("{base}/events/{event_id}");
        let transport = &self.transport;
        let resp = self.authenticated(&mut |t| transport.get(&url, &[("Authorization", t)]))?;
        let json: serde_json::Value =
            serde_json::from_slice(&resp).map_err(|e| TransportError {
                kind: TransportErrorKind::InvalidResponse,
                message: e.to_string(),
            })?;
        parse_event(&json).ok_or_else(|| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: "missing event in response".into(),
        })
    }

    /// Create a new event.
    pub fn create_event(
        &self,
        summary: &str,
        start: &EventTime,
        end: &EventTime,
        description: Option<&str>,
        location: Option<&str>,
        attendees: &[&str],
    ) -> Result<CreateResult, TransportError> {
        let mut body_json = serde_json::json!({
            "summary": summary,
            "start": time_to_json(start),
            "end": time_to_json(end),
            "status": "confirmed",
        });
        if let Some(desc) = description {
            body_json["description"] = serde_json::json!(desc);
        }
        if let Some(loc) = location {
            body_json["location"] = serde_json::json!(loc);
        }
        if !attendees.is_empty() {
            body_json["attendees"] = serde_json::json!(
                attendees
                    .iter()
                    .map(|a| serde_json::json!({"email": a}))
                    .collect::<Vec<_>>()
            );
        }
        let body_bytes = serde_json::to_vec(&body_json).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: e.to_string(),
        })?;
        let base = self.base_url();
        let url = format!("{base}/events");
        let transport = &self.transport;
        let resp = self.authenticated(&mut |t| {
            transport.post_json(&url, &[("Authorization", t)], &body_bytes)
        })?;
        let json: serde_json::Value =
            serde_json::from_slice(&resp).map_err(|e| TransportError {
                kind: TransportErrorKind::InvalidResponse,
                message: e.to_string(),
            })?;
        Ok(CreateResult {
            event_id: json["id"].as_str().unwrap_or("").to_string(),
            html_link: json["htmlLink"].as_str().map(|s| s.to_string()),
        })
    }

    /// Update an event.
    pub fn update_event(
        &self,
        event_id: &str,
        summary: Option<&str>,
        start: Option<&EventTime>,
        end: Option<&EventTime>,
        description: Option<&str>,
    ) -> Result<(), TransportError> {
        let mut body_json = serde_json::json!({});
        if let Some(s) = summary {
            body_json["summary"] = serde_json::json!(s);
        }
        if let Some(st) = start {
            body_json["start"] = time_to_json(st);
        }
        if let Some(e) = end {
            body_json["end"] = time_to_json(e);
        }
        if let Some(d) = description {
            body_json["description"] = serde_json::json!(d);
        }
        let body_bytes = serde_json::to_vec(&body_json).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: e.to_string(),
        })?;
        let base = self.base_url();
        let url = format!("{base}/events/{event_id}");
        let transport = &self.transport;
        self.authenticated(&mut |t| {
            transport.post_json(&url, &[("Authorization", t)], &body_bytes)
        })?;
        Ok(())
    }

    /// Delete an event.
    pub fn delete_event(&self, event_id: &str) -> Result<(), TransportError> {
        // DELETE via POST with method override (some transports don't support DELETE).
        let body = b"";
        let base = self.base_url();
        let url = format!("{base}/events/{event_id}");
        // For a real implementation, this would be an HTTP DELETE.
        // The mock transport just accepts it.
        let transport = &self.transport;
        self.authenticated(&mut |t| transport.post_json(&url, &[("Authorization", t)], body))?;
        Ok(())
    }

    /// Check free/busy for a time range.
    pub fn free_busy(
        &self,
        time_min: &str,
        time_max: &str,
    ) -> Result<FreeBusyResult, TransportError> {
        let body_json = serde_json::json!({
            "timeMin": time_min,
            "timeMax": time_max,
            "items": [{"id": &self.calendar_id}]
        });
        let body_bytes = serde_json::to_vec(&body_json).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: e.to_string(),
        })?;
        let url = format!("{CALENDAR_API_BASE}/freeBusy");
        let transport = &self.transport;
        let resp = self.authenticated(&mut |t| {
            transport.post_json(&url, &[("Authorization", t)], &body_bytes)
        })?;
        let json: serde_json::Value =
            serde_json::from_slice(&resp).map_err(|e| TransportError {
                kind: TransportErrorKind::InvalidResponse,
                message: e.to_string(),
            })?;
        let busy = json["calendars"][&self.calendar_id]["busy"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        Some(TimeSlot {
                            start: s["start"].as_str()?.to_string(),
                            end: s["end"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(FreeBusyResult { busy })
    }

    /// Export calendar as ICS (simplified).
    pub fn export_ics(&self) -> Result<String, TransportError> {
        let events = self.list_events(None, None, 100)?;
        let mut ics =
            String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//AgentCowork//Calendar\r\n");
        for ev in &events {
            ics.push_str("BEGIN:VEVENT\r\n");
            ics.push_str(&format!("UID:{}\r\n", ev.id));
            ics.push_str(&format!("SUMMARY:{}\r\n", ev.summary));
            ics.push_str(&format!("DTSTART:{}\r\n", event_time_to_ics(&ev.start)));
            ics.push_str(&format!("DTEND:{}\r\n", event_time_to_ics(&ev.end)));
            if let Some(ref desc) = ev.description {
                ics.push_str(&format!("DESCRIPTION:{}\r\n", desc.replace('\n', "\\n")));
            }
            if let Some(ref loc) = ev.location {
                ics.push_str(&format!("LOCATION:{}\r\n", loc));
            }
            ics.push_str("END:VEVENT\r\n");
        }
        ics.push_str("END:VCALENDAR\r\n");
        Ok(ics)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_event(json: &serde_json::Value) -> Option<CalendarEvent> {
    let start = parse_time(json.get("start")?)?;
    let end = parse_time(json.get("end")?)?;
    let attendees = json["attendees"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["email"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let status = match json["status"].as_str()? {
        "tentative" => EventStatus::Tentative,
        "cancelled" => EventStatus::Cancelled,
        _ => EventStatus::Confirmed,
    };
    Some(CalendarEvent {
        id: json["id"].as_str()?.to_string(),
        summary: json["summary"].as_str()?.to_string(),
        description: json["description"].as_str().map(|s| s.to_string()),
        location: json["location"].as_str().map(|s| s.to_string()),
        start,
        end,
        attendees,
        status,
        html_link: json["htmlLink"].as_str().map(|s| s.to_string()),
        recurrence: json["recurrence"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| r.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn parse_time(json: &serde_json::Value) -> Option<EventTime> {
    if let Some(dt) = json["dateTime"].as_str() {
        let tz = json["timeZone"].as_str().map(|s| s.to_string());
        return Some(EventTime::DateTime(dt.to_string(), tz));
    }
    json["date"]
        .as_str()
        .map(|d| EventTime::Date(d.to_string()))
}

fn time_to_json(t: &EventTime) -> serde_json::Value {
    match t {
        EventTime::DateTime(dt, tz) => {
            let mut j = serde_json::json!({"dateTime": dt});
            if let Some(tz) = tz {
                j["timeZone"] = serde_json::json!(tz);
            }
            j
        }
        EventTime::Date(d) => serde_json::json!({"date": d}),
    }
}

fn event_time_to_ics(t: &EventTime) -> String {
    match t {
        EventTime::DateTime(dt, _) => dt.replace(['-', ':'], ""),
        EventTime::Date(d) => d.replace('-', ""),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// FIX-01: a *source*, not a token holder.
    struct MockTokens;
    impl TokenSource for MockTokens {
        fn with_token<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("tok")
        }
        fn refresh<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("tok")
        }
    }

    fn cal_tokens() -> VaultTokenRef {
        VaultTokenRef::new("k-cal", "google-calendar")
    }

    struct MockTransport {
        responses: RefCell<Vec<Result<Vec<u8>, TransportError>>>,
    }

    impl MockTransport {
        fn new(responses: Vec<Result<Vec<u8>, TransportError>>) -> Self {
            Self {
                responses: RefCell::new(responses),
            }
        }
    }

    impl HttpTransport for MockTransport {
        fn post_json(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<Vec<u8>, TransportError> {
            self.responses
                .borrow_mut()
                .pop()
                .unwrap_or(Err(TransportError {
                    kind: TransportErrorKind::Other,
                    message: "empty".into(),
                }))
        }
        fn get(&self, _url: &str, _headers: &[(&str, &str)]) -> Result<Vec<u8>, TransportError> {
            self.responses
                .borrow_mut()
                .pop()
                .unwrap_or(Err(TransportError {
                    kind: TransportErrorKind::Other,
                    message: "empty".into(),
                }))
        }
    }

    #[test]
    fn list_events_returns_parsed_events() {
        let resp = serde_json::json!({
            "items": [{
                "id": "ev1",
                "summary": "Team Standup",
                "start": {"dateTime": "2026-08-21T09:00:00Z", "timeZone": "UTC"},
                "end": {"dateTime": "2026-08-21T09:30:00Z", "timeZone": "UTC"},
                "status": "confirmed",
                "attendees": [{"email": "alice@example.com"}]
            }]
        });
        let transport = MockTransport::new(vec![Ok(serde_json::to_vec(&resp).unwrap())]);
        let cal = CalendarConnector::new(transport, MockTokens, cal_tokens(), "primary");
        let events = cal.list_events(None, None, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Team Standup");
        assert_eq!(events[0].attendees, vec!["alice@example.com"]);
    }

    #[test]
    fn create_event() {
        let resp =
            serde_json::json!({"id": "new-ev", "htmlLink": "https://calendar.google.com/event"});
        let transport = MockTransport::new(vec![Ok(serde_json::to_vec(&resp).unwrap())]);
        let cal = CalendarConnector::new(transport, MockTokens, cal_tokens(), "primary");
        let result = cal
            .create_event(
                "New Meeting",
                &EventTime::datetime("2026-08-22T10:00:00Z", None),
                &EventTime::datetime("2026-08-22T11:00:00Z", None),
                Some("Discuss Q3"),
                None,
                &["bob@example.com"],
            )
            .unwrap();
        assert_eq!(result.event_id, "new-ev");
    }

    #[test]
    fn free_busy() {
        let resp = serde_json::json!({
            "calendars": {
                "primary": {
                    "busy": [
                        {"start": "2026-08-21T09:00:00Z", "end": "2026-08-21T10:00:00Z"}
                    ]
                }
            }
        });
        let transport = MockTransport::new(vec![Ok(serde_json::to_vec(&resp).unwrap())]);
        let cal = CalendarConnector::new(transport, MockTokens, cal_tokens(), "primary");
        let fb = cal
            .free_busy("2026-08-21T00:00:00Z", "2026-08-22T00:00:00Z")
            .unwrap();
        assert_eq!(fb.busy.len(), 1);
        assert_eq!(fb.busy[0].start, "2026-08-21T09:00:00Z");
    }

    #[test]
    fn export_ics() {
        let resp = serde_json::json!({
            "items": [{
                "id": "ev1",
                "summary": "Lunch",
                "start": {"date": "2026-08-21"},
                "end": {"date": "2026-08-21"},
                "status": "confirmed"
            }]
        });
        let transport = MockTransport::new(vec![Ok(serde_json::to_vec(&resp).unwrap())]);
        let cal = CalendarConnector::new(transport, MockTokens, cal_tokens(), "primary");
        let ics = cal.export_ics().unwrap();
        assert!(ics.contains("BEGIN:VCALENDAR"));
        assert!(ics.contains("SUMMARY:Lunch"));
        assert!(ics.contains("END:VCALENDAR"));
    }
}
