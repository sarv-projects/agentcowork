//! P2.10 — durable event log + idempotency classes (doc 53 §4, J5/J19/J13).
//!
//! §4.1 invariant: an append-only event log is the source of truth from
//! which any session replays; explicit idempotency semantics ensure a dead
//! coordinator cannot double-execute external mutations.
//!
//! §4.2 — ten event types; §4.3 — idempotency classes declared per
//! operation (safe_retry / unsafe_retry / same_key / confirm_after_uncertain);
//! §4.4 — recovery: any `ToolStarted` without `ToolCompleted` is classified
//! and gets a rerun / resend-with-key / confirmation-card decision.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The durable event types (doc 53 §4.2, stable names) + P5.9/J5 the
/// per-turn context-injection event the Trajectory view filters by source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    UserMessageAdded,
    PlanCreated,
    TaskStarted,
    ToolProposed,
    PermissionGranted,
    ToolStarted,
    ToolCompleted,
    ArtifactWritten,
    ModelTurnCompleted,
    CheckpointCommitted,
    /// P5.9/J5 — a context block (persona / user doc / memory / tool result /
    /// blueprint) was injected into the prompt this turn.
    ContextInjection,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::UserMessageAdded => "UserMessageAdded",
            EventType::PlanCreated => "PlanCreated",
            EventType::TaskStarted => "TaskStarted",
            EventType::ToolProposed => "ToolProposed",
            EventType::PermissionGranted => "PermissionGranted",
            EventType::ToolStarted => "ToolStarted",
            EventType::ToolCompleted => "ToolCompleted",
            EventType::ArtifactWritten => "ArtifactWritten",
            EventType::ModelTurnCompleted => "ModelTurnCompleted",
            EventType::CheckpointCommitted => "CheckpointCommitted",
            EventType::ContextInjection => "ContextInjection",
        }
    }
}

/// The canonical context-injection sources the Trajectory (J5) view filters by.
pub const CONTEXT_SOURCES: [&str; 5] = [
    "persona",
    "user_document",
    "memory",
    "tool_result",
    "blueprint",
];

/// Is `source` one of the canonical injection sources (J5)?
pub fn is_context_source(source: &str) -> bool {
    CONTEXT_SOURCES.contains(&source)
}

/// P5.9/J5 — one context-injection record the Trajectory view renders:
/// which context block (source) was injected into the prompt on a turn, and
/// how much of it. Parsed from a [`EventType::ContextInjection`] event's
/// `result_meta` (`{ source, tokens?, refId? }`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextInjectionRecord {
    pub seq: u64,
    pub ts_ms: u64,
    pub session: String,
    pub agent: String,
    pub source: String,
    /// Injected token estimate (best-effort from `result_meta.tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// The injected block's identity (doc path / memory id / tool name).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ref_id: String,
}

/// One durable session event (one NDJSON line).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEvent {
    pub seq: u64,
    pub ts_ms: u64,
    pub session: String,
    pub agent: String,
    /// Empty for non-tool events.
    #[serde(default)]
    pub tool: String,
    /// sha256 hex of the normalized tool args — the idempotency key.
    #[serde(default)]
    pub args_hash: String,
    #[serde(default)]
    pub result_meta: serde_json::Value,
    /// P3.3 (J14) — trace linkage of the execution that produced this event.
    #[serde(default)]
    pub trace_id: String,
    #[serde(default)]
    pub span_id: String,
    pub event_type: EventType,
}

/// Input for one append (seq/ts assigned by the log).
#[derive(Debug, Clone)]
pub struct EventInput {
    pub event_type: EventType,
    pub session: String,
    pub agent: String,
    pub tool: String,
    pub args_hash: String,
    pub result_meta: serde_json::Value,
    /// P3.3 (J14) — trace linkage (empty when not inside a trace).
    pub trace_id: String,
    pub span_id: String,
}

impl EventInput {
    pub fn new(
        event_type: EventType,
        session: impl Into<String>,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            event_type,
            session: session.into(),
            agent: agent.into(),
            tool: String::new(),
            args_hash: String::new(),
            result_meta: serde_json::Value::Null,
            trace_id: String::new(),
            span_id: String::new(),
        }
    }

    pub fn with_tool(mut self, tool: impl Into<String>, args_hash: impl Into<String>) -> Self {
        self.tool = tool.into();
        self.args_hash = args_hash.into();
        self
    }

    /// P3.3 (J14) — attach the trace context of this execution.
    pub fn with_trace(mut self, trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        self.trace_id = trace_id.into();
        self.span_id = span_id.into();
        self
    }
}

/// Append-only per-session NDJSON log (`<base>/sessions/<session_id>.ndjson`).
pub struct SessionLog {
    path: PathBuf,
    seq: u64,
}

/// P5.9/J5 — enumerate the session ids that have a log file under
/// `<base>/sessions/` (newest-created first).
pub fn list_session_ids(base_dir: &Path) -> Result<Vec<String>, SessionLogError> {
    let dir = base_dir.join("sessions");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "ndjson").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let created = entry
                    .metadata()
                    .map(|m| m.created().ok().or(m.modified().ok()))
                    .ok()
                    .flatten();
                entries.push((stem.to_string(), created));
            }
        }
    }
    entries.sort_by_key(|(_, created)| std::cmp::Reverse(*created));
    Ok(entries.into_iter().map(|(id, _)| id).collect())
}

impl SessionLog {
    pub fn open(base_dir: &Path, session_id: &str) -> Result<Self, SessionLogError> {
        if session_id.trim().is_empty() || session_id.contains('/') || session_id.contains("..") {
            return Err(SessionLogError::InvalidSessionId(session_id.to_string()));
        }
        let dir = base_dir.join("sessions");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{session_id}.ndjson"));
        let seq = if path.exists() {
            let body = fs::read_to_string(&path)?;
            body.lines()
                .filter_map(|l| {
                    if l.trim().is_empty() {
                        return None;
                    }
                    serde_json::from_str::<SessionEvent>(l).ok()
                })
                .map(|e| e.seq)
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        Ok(Self { path, seq })
    }

    fn write_event(&mut self, input: EventInput) -> Result<SessionEvent, SessionLogError> {
        self.seq += 1;
        Ok(SessionEvent {
            seq: self.seq,
            ts_ms: now_ms(),
            session: input.session,
            agent: input.agent,
            tool: input.tool,
            args_hash: input.args_hash,
            result_meta: input.result_meta,
            trace_id: input.trace_id,
            span_id: input.span_id,
            event_type: input.event_type,
        })
    }

    /// Append one event, flush, return its seq.
    pub fn append(&mut self, input: EventInput) -> Result<u64, SessionLogError> {
        let ev = self.write_event(input)?;
        let mut line = serde_json::to_vec(&ev)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(&line)?;
        f.flush()?;
        Ok(ev.seq)
    }

    /// P45.5 — turn-boundary write batching (honesty-preserving). Writes N
    /// events in ONE open/append/flush cycle, in receipt order, returning the
    /// seq of each. The append-only invariant is unchanged: each line is one
    /// complete, independently receiptable event; order is preserved (seq is
    /// assigned on entry, lines are written in that order); a crash mid-cycle
    /// leaves only fully-written newline-terminated lines readable, exactly as
    /// with per-event appends. This is a throughput optimization — it never
    /// weakens durability or reorders receipts.
    pub fn append_batch(
        &mut self,
        inputs: impl IntoIterator<Item = EventInput>,
    ) -> Result<Vec<u64>, SessionLogError> {
        let events: Vec<SessionEvent> = inputs
            .into_iter()
            .map(|i| self.write_event(i))
            .collect::<Result<_, _>>()?;
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut body = Vec::new();
        for ev in &events {
            let mut line = serde_json::to_vec(ev)?;
            line.push(b'\n');
            body.extend_from_slice(&line);
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(&body)?;
        f.flush()?;
        Ok(events.iter().map(|e| e.seq).collect())
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Read the whole log back in order.
    pub fn events(&self) -> Result<Vec<SessionEvent>, SessionLogError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let body = fs::read_to_string(&self.path)?;
        let mut out = Vec::new();
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<SessionEvent>(line) {
                out.push(ev);
            }
        }
        Ok(out)
    }

    /// P5.9/J5 — the context-injection records for this session (the data the
    /// Trajectory view filters by source). Empty when no injections were
    /// recorded; unknown/missing sources are preserved as-is (the view groups
    /// them under an `other` bucket).
    pub fn context_injections(&self) -> Result<Vec<ContextInjectionRecord>, SessionLogError> {
        let events = self.events()?;
        Ok(events
            .into_iter()
            .filter(|e| e.event_type == EventType::ContextInjection)
            .map(|e| {
                let source = e
                    .result_meta
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("other")
                    .to_string();
                let tokens = e.result_meta.get("tokens").and_then(Value::as_u64);
                let ref_id = e
                    .result_meta
                    .get("refId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                ContextInjectionRecord {
                    seq: e.seq,
                    ts_ms: e.ts_ms,
                    session: e.session,
                    agent: e.agent,
                    source,
                    tokens,
                    ref_id,
                }
            })
            .collect())
    }

    /// §4.4: `ToolStarted` events with no matching `ToolCompleted` (by tool +
    /// args_hash, in order).
    pub fn tool_starts_without_completion(&self) -> Result<Vec<SessionEvent>, SessionLogError> {
        let events = self.events()?;
        let mut incomplete = Vec::new();
        for ev in events
            .iter()
            .filter(|e| e.event_type == EventType::ToolStarted)
        {
            let completed = events.iter().any(|c| {
                c.event_type == EventType::ToolCompleted
                    && c.tool == ev.tool
                    && c.args_hash == ev.args_hash
                    && c.seq > ev.seq
            });
            if !completed {
                incomplete.push(ev.clone());
            }
        }
        Ok(incomplete)
    }

    /// v3.39 — project the message history from the append-only event log.
    /// Each `UserMessageAdded` + `ModelTurnCompleted` pair becomes one
    /// `ProjectedMessage`. Tool calls within a turn are counted but do not
    /// produce separate projected messages. The result is a *view* — the
    /// event log is the source of truth.
    pub fn project_messages(&self) -> Result<Vec<ProjectedMessage>, SessionLogError> {
        let events = self.events()?;
        let mut messages = Vec::new();
        let mut turn = 0u64;
        let mut tool_count = 0u32;
        let mut current_content = String::new();
        let mut current_ts = 0u64;
        let mut current_role = String::new();
        let mut in_turn = false;

        for ev in &events {
            match ev.event_type {
                EventType::UserMessageAdded => {
                    turn += 1;
                    current_role = "user".into();
                    current_content = ev
                        .result_meta
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    current_ts = ev.ts_ms;
                    tool_count = 0;
                    in_turn = true;
                }
                EventType::ToolStarted if in_turn => {
                    tool_count += 1;
                }
                EventType::ModelTurnCompleted if in_turn => {
                    let content = ev
                        .result_meta
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    messages.push(ProjectedMessage {
                        turn,
                        role: current_role.clone(),
                        content: current_content.clone(),
                        ts_ms: current_ts,
                        tool_call_count: tool_count,
                    });
                    messages.push(ProjectedMessage {
                        turn,
                        role: "assistant".into(),
                        content,
                        ts_ms: ev.ts_ms,
                        tool_call_count: 0,
                    });
                    in_turn = false;
                    tool_count = 0;
                }
                _ => {}
            }
        }
        Ok(messages)
    }

    /// v3.39 — fork the session at a completed-turn boundary. Returns a
    /// `ForkLineage` record and copies events up to (and including) the
    /// `ModelTurnCompleted` at `fork_at_turn` into a new session log file.
    ///
    /// The forked log is a snapshot — it does NOT diverge from the source.
    /// The caller owns the divergence (new events go to the new log).
    pub fn fork_at_turn(
        &self,
        fork_at_turn: u64,
        new_session_id: &str,
    ) -> Result<ForkLineage, SessionLogError> {
        if new_session_id.trim().is_empty()
            || new_session_id.contains('/')
            || new_session_id.contains("..")
        {
            return Err(SessionLogError::InvalidSessionId(
                new_session_id.to_string(),
            ));
        }
        let events = self.events()?;
        let mut current_turn = 0u64;
        let mut fork_seq = 0u64;
        let mut forked_events = Vec::new();

        for ev in &events {
            match ev.event_type {
                EventType::UserMessageAdded => {
                    current_turn += 1;
                    if current_turn == fork_at_turn {
                        forked_events.push(ev.clone());
                    }
                }
                EventType::ModelTurnCompleted if current_turn == fork_at_turn => {
                    fork_seq = ev.seq;
                    forked_events.push(ev.clone());
                    break;
                }
                _ if current_turn == fork_at_turn => {
                    forked_events.push(ev.clone());
                }
                _ => {}
            }
        }

        if fork_seq == 0 {
            return Err(SessionLogError::InvalidSessionId(format!(
                "turn {fork_at_turn} not found or not completed"
            )));
        }

        // Write the forked events to a new session log.
        // self.path is base_dir/sessions/<id>.ndjson; we need base_dir (two
        // levels up: file → sessions/ → base_dir).
        let base = self.path.parent().and_then(|p| p.parent()).ok_or_else(|| {
            SessionLogError::Io(io::Error::other("cannot determine base directory"))
        })?;
        let mut new_log = SessionLog::open(base, new_session_id)?;
        for ev in forked_events {
            new_log.append(EventInput {
                event_type: ev.event_type,
                session: new_session_id.to_string(),
                agent: ev.agent,
                tool: ev.tool,
                args_hash: ev.args_hash,
                result_meta: ev.result_meta,
                trace_id: ev.trace_id,
                span_id: ev.span_id,
            })?;
        }

        Ok(ForkLineage {
            source_session: self
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
            fork_at_turn,
            fork_at_event_seq: fork_seq,
            new_session_id: new_session_id.to_string(),
            created_at_ms: now_ms(),
        })
    }
}

/// §4.3 — idempotency classes (declared per operation in the tool manifest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyClass {
    /// Read-only / deterministic — retry freely.
    SafeRetry,
    /// Mutates (write, send, execute) — never auto-retry.
    UnsafeRetry,
    /// Retry only with an identical idempotency key; broker dedupes.
    SameKey,
    /// Outcome unknown (network drop mid-mutation) — confirm before retry.
    ConfirmAfterUncertain,
}

impl IdempotencyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            IdempotencyClass::SafeRetry => "safe_retry",
            IdempotencyClass::UnsafeRetry => "unsafe_retry",
            IdempotencyClass::SameKey => "same_key",
            IdempotencyClass::ConfirmAfterUncertain => "confirm_after_uncertain",
        }
    }
}

impl std::str::FromStr for IdempotencyClass {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "safe_retry" => IdempotencyClass::SafeRetry,
            "unsafe_retry" => IdempotencyClass::UnsafeRetry,
            "same_key" => IdempotencyClass::SameKey,
            _ => IdempotencyClass::ConfirmAfterUncertain,
        })
    }
}

/// The tool-manifest classifier (doc 53 §4.3). Read-only/deterministic tools
/// are safe; mutations are unsafe; side-effectful tools that carry a
/// caller-provided key are same_key; anything ambiguous needs confirmation.
pub fn classify_tool(tool: &str) -> IdempotencyClass {
    match tool {
        "browser.snapshot" | "browser.read" | "browser.search" | "search" | "vault.list"
        | "repo.map" | "memory.query" => IdempotencyClass::SafeRetry,
        "browser.act" | "file.write" | "file.delete" | "terminal.exec" | "connector.mutate" => {
            IdempotencyClass::UnsafeRetry
        }
        "connector.send_email" | "payment.charge" | "messaging.send" => IdempotencyClass::SameKey,
        _ => IdempotencyClass::ConfirmAfterUncertain,
    }
}

/// v3.39 — one projected message derived from the event log. The message
/// history is a *view* over the append-only log, not the source of truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectedMessage {
    pub turn: u64,
    pub role: String,
    pub content: String,
    pub ts_ms: u64,
    pub tool_call_count: u32,
}

/// v3.39 — lineage record for a forked session. A fork happens at a
/// completed-turn boundary; the fork inherits the event log up to that
/// point and diverges with a new session id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkLineage {
    pub source_session: String,
    pub fork_at_turn: u64,
    pub fork_at_event_seq: u64,
    pub new_session_id: String,
    pub created_at_ms: u64,
}

/// In-process same_key dedupe: the broker records the result of the first
/// execution of a key and re-sends return it instead of re-executing.
#[derive(Debug, Default)]
pub struct IdempotencyRegistry {
    done: Mutex<HashMap<String, serde_json::Value>>,
}

impl IdempotencyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lookup(&self, key: &str) -> Option<serde_json::Value> {
        self.done.lock().unwrap().get(key).cloned()
    }

    pub fn register(&self, key: &str, result: serde_json::Value) {
        self.done.lock().unwrap().insert(key.to_string(), result);
    }

    pub fn clear(&self) {
        self.done.lock().unwrap().clear();
    }

    pub fn len(&self) -> usize {
        self.done.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What recovery should do with one incomplete tool (doc 53 §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Safe class — re-run freely.
    Rerun,
    /// Same-key class — re-send with the identical idempotency key.
    ResendWithKey { key: String },
    /// Unsafe / ambiguous — surface a confirmation card, never auto-retry.
    ConfirmCard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryDecision {
    pub seq: u64,
    pub tool: String,
    pub args_hash: String,
    pub class: IdempotencyClass,
    pub action: RecoveryAction,
}

/// §4.4 — classify every incomplete tool start into a recovery decision.
pub fn recovery_plan(
    incomplete: &[SessionEvent],
    registry: &IdempotencyRegistry,
) -> Vec<RecoveryDecision> {
    incomplete
        .iter()
        .map(|ev| {
            let class = classify_tool(&ev.tool);
            let action = match class {
                IdempotencyClass::SafeRetry => RecoveryAction::Rerun,
                IdempotencyClass::SameKey => {
                    if registry.lookup(&ev.args_hash).is_some() {
                        // Already executed — safe to treat as done.
                        RecoveryAction::Rerun
                    } else {
                        RecoveryAction::ResendWithKey {
                            key: ev.args_hash.clone(),
                        }
                    }
                }
                IdempotencyClass::UnsafeRetry | IdempotencyClass::ConfirmAfterUncertain => {
                    RecoveryAction::ConfirmCard
                }
            };
            RecoveryDecision {
                seq: ev.seq,
                tool: ev.tool.clone(),
                args_hash: ev.args_hash.clone(),
                class,
                action,
            }
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum SessionLogError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid session id: {0}")]
    InvalidSessionId(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("agentcowork-sesslog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn appends_resumes_and_reads_back() {
        let dir = tmp_dir("basic");
        {
            let mut log = SessionLog::open(&dir, "sess-1").unwrap();
            let mut start = EventInput::new(EventType::TaskStarted, "sess-1", "agent-a");
            start.tool = "browser.act".into();
            start.args_hash = "abc123".into();
            assert_eq!(log.append(start).unwrap(), 1);
            let done = EventInput::new(EventType::ToolCompleted, "sess-1", "agent-a")
                .with_tool("browser.act", "abc123");
            assert_eq!(log.append(done).unwrap(), 2);
        }
        // Reopen — seq resumes, events read back in order.
        let mut log = SessionLog::open(&dir, "sess-1").unwrap();
        assert_eq!(log.seq(), 2);
        let evs = log.events().unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event_type, EventType::TaskStarted);
        assert_eq!(evs[0].args_hash, "abc123");
        assert_eq!(
            log.append(EventInput::new(EventType::PlanCreated, "sess-1", "agent-a"))
                .unwrap(),
            3
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_events_carry_trace_linkage() {
        let dir = tmp_dir("trace");
        let mut log = SessionLog::open(&dir, "sess-1").unwrap();
        log.append(
            EventInput::new(EventType::ToolStarted, "sess-1", "a")
                .with_tool("browser.act", "k1")
                .with_trace("abcdef0123456789abcdef0123456789", "1234567890abcdef"),
        )
        .unwrap();
        let evs = log.events().unwrap();
        assert_eq!(evs[0].trace_id, "abcdef0123456789abcdef0123456789");
        assert_eq!(evs[0].span_id, "1234567890abcdef");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn incomplete_tool_starts_detected() {
        let dir = tmp_dir("incomplete");
        let mut log = SessionLog::open(&dir, "sess-1").unwrap();
        // Completed pair.
        log.append(
            EventInput::new(EventType::ToolStarted, "sess-1", "a").with_tool("browser.read", "k1"),
        )
        .unwrap();
        log.append(
            EventInput::new(EventType::ToolCompleted, "sess-1", "a")
                .with_tool("browser.read", "k1"),
        )
        .unwrap();
        // Orphan start (coordinator died mid-mutation).
        log.append(
            EventInput::new(EventType::ToolStarted, "sess-1", "a")
                .with_tool("connector.send_email", "k2"),
        )
        .unwrap();
        let incomplete = log.tool_starts_without_completion().unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].tool, "connector.send_email");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn classify_matches_doc_53_table() {
        assert_eq!(
            classify_tool("browser.snapshot"),
            IdempotencyClass::SafeRetry
        );
        assert_eq!(classify_tool("file.write"), IdempotencyClass::UnsafeRetry);
        assert_eq!(
            classify_tool("connector.send_email"),
            IdempotencyClass::SameKey
        );
        assert_eq!(
            classify_tool("unknown.tool"),
            IdempotencyClass::ConfirmAfterUncertain
        );
    }

    #[test]
    fn recovery_plan_classifies_actions() {
        let dir = tmp_dir("recovery");
        let mut log = SessionLog::open(&dir, "sess-1").unwrap();
        log.append(
            EventInput::new(EventType::ToolStarted, "sess-1", "a").with_tool("browser.read", "k1"),
        )
        .unwrap();
        log.append(
            EventInput::new(EventType::ToolStarted, "sess-1", "a")
                .with_tool("connector.send_email", "k2"),
        )
        .unwrap();
        log.append(
            EventInput::new(EventType::ToolStarted, "sess-1", "a").with_tool("file.write", "k3"),
        )
        .unwrap();
        let incomplete = log.tool_starts_without_completion().unwrap();
        let registry = IdempotencyRegistry::new();
        let plan = recovery_plan(&incomplete, &registry);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].action, RecoveryAction::Rerun); // safe
        assert_eq!(
            plan[1].action,
            RecoveryAction::ResendWithKey { key: "k2".into() } // same_key
        );
        assert_eq!(plan[2].action, RecoveryAction::ConfirmCard); // unsafe
        // Same-key tool already executed → registry lookup flips to Rerun.
        registry.register("k2", serde_json::json!({"ok": true}));
        let plan2 = recovery_plan(&incomplete, &registry);
        assert_eq!(plan2[1].action, RecoveryAction::Rerun);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_session_id_rejected() {
        let dir = tmp_dir("badid");
        assert!(SessionLog::open(&dir, "../escape").is_err());
        assert!(SessionLog::open(&dir, "a/b").is_err());
        assert!(SessionLog::open(&dir, "").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn idempotency_registry_dedupes() {
        let r = IdempotencyRegistry::new();
        assert!(r.lookup("k").is_none());
        r.register("k", serde_json::json!({"ok": true}));
        assert_eq!(r.lookup("k"), Some(serde_json::json!({"ok": true})));
        r.clear();
        assert!(r.lookup("k").is_none());
    }

    #[test]
    fn context_injections_parse_source_tokens_and_ref() {
        let dir = tmp_dir("trajectory");
        let mut log = SessionLog::open(&dir, "sess-1").unwrap();
        let mut inj = EventInput::new(EventType::ContextInjection, "sess-1", "agent-a");
        inj.result_meta = serde_json::json!({
            "source": "memory",
            "tokens": 412,
            "refId": "mem:3"
        });
        log.append(inj).unwrap();
        // An unrelated non-injection event must be ignored.
        log.append(EventInput::new(EventType::TaskStarted, "sess-1", "agent-a"))
            .unwrap();
        // A second injection with a missing/unknown source → "other".
        let mut inj2 = EventInput::new(EventType::ContextInjection, "sess-1", "agent-b");
        inj2.result_meta = serde_json::json!({ "source": "weird_source" });
        log.append(inj2).unwrap();

        let recs = log.context_injections().unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].source, "memory");
        assert_eq!(recs[0].tokens, Some(412));
        assert_eq!(recs[0].ref_id, "mem:3");
        assert_eq!(recs[1].source, "weird_source");
        assert_eq!(recs[1].tokens, None);
        assert!(is_context_source("memory"));
        assert!(!is_context_source("weird_source"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_session_ids_finds_log_files() {
        let dir = tmp_dir("listsess");
        {
            let mut a = SessionLog::open(&dir, "sess-a").unwrap();
            a.append(EventInput::new(EventType::TaskStarted, "sess-a", "x"))
                .unwrap();
            let mut b = SessionLog::open(&dir, "sess-b").unwrap();
            b.append(EventInput::new(EventType::TaskStarted, "sess-b", "x"))
                .unwrap();
        }
        let ids = list_session_ids(&dir).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"sess-a".to_string()));
        assert!(ids.contains(&"sess-b".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_messages_derives_view_from_event_log() {
        let dir = tmp_dir("project");
        let mut log = SessionLog::open(&dir, "proj-sess").unwrap();
        // Turn 1: user message + tool call + assistant reply.
        log.append(EventInput::new(
            EventType::UserMessageAdded,
            "proj-sess",
            "agent",
        ))
        .unwrap();
        let mut tool_started = EventInput::new(EventType::ToolStarted, "proj-sess", "agent");
        tool_started.tool = "browser.read".into();
        tool_started.args_hash = "h1".into();
        log.append(tool_started).unwrap();
        let mut tool_done = EventInput::new(EventType::ToolCompleted, "proj-sess", "agent");
        tool_done.tool = "browser.read".into();
        tool_done.args_hash = "h1".into();
        log.append(tool_done).unwrap();
        let mut reply = EventInput::new(EventType::ModelTurnCompleted, "proj-sess", "agent");
        reply.result_meta = serde_json::json!({"content": "hello there"});
        log.append(reply).unwrap();
        // Turn 2: user message + assistant reply (no tools).
        log.append(EventInput::new(
            EventType::UserMessageAdded,
            "proj-sess",
            "agent",
        ))
        .unwrap();
        let mut reply2 = EventInput::new(EventType::ModelTurnCompleted, "proj-sess", "agent");
        reply2.result_meta = serde_json::json!({"content": "second reply"});
        log.append(reply2).unwrap();

        let projected = log.project_messages().unwrap();
        assert_eq!(projected.len(), 4);
        // Turn 1 user message.
        assert_eq!(projected[0].turn, 1);
        assert_eq!(projected[0].role, "user");
        assert_eq!(projected[0].tool_call_count, 1);
        // Turn 1 assistant reply.
        assert_eq!(projected[1].turn, 1);
        assert_eq!(projected[1].role, "assistant");
        assert_eq!(projected[1].content, "hello there");
        // Turn 2 user message.
        assert_eq!(projected[2].turn, 2);
        assert_eq!(projected[2].role, "user");
        assert_eq!(projected[2].tool_call_count, 0);
        // Turn 2 assistant reply.
        assert_eq!(projected[3].turn, 2);
        assert_eq!(projected[3].role, "assistant");
        assert_eq!(projected[3].content, "second reply");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fork_at_turn_copies_events_to_new_session() {
        let dir = tmp_dir("fork");
        let mut log = SessionLog::open(&dir, "src-sess").unwrap();
        // Turn 1: user + assistant.
        log.append(EventInput::new(
            EventType::UserMessageAdded,
            "src-sess",
            "a",
        ))
        .unwrap();
        log.append(EventInput::new(
            EventType::ModelTurnCompleted,
            "src-sess",
            "a",
        ))
        .unwrap();
        // Turn 2: user + assistant.
        log.append(EventInput::new(
            EventType::UserMessageAdded,
            "src-sess",
            "a",
        ))
        .unwrap();
        log.append(EventInput::new(
            EventType::ModelTurnCompleted,
            "src-sess",
            "a",
        ))
        .unwrap();
        // Fork at turn 1.
        let lineage = log.fork_at_turn(1, "fork-sess").unwrap();
        assert_eq!(lineage.source_session, "src-sess");
        assert_eq!(lineage.fork_at_turn, 1);
        assert_eq!(lineage.new_session_id, "fork-sess");
        // The forked log has events for turn 1 only.
        let forked = SessionLog::open(&dir, "fork-sess").unwrap();
        let events = forked.events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::UserMessageAdded);
        assert_eq!(events[0].session, "fork-sess");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fork_at_turn_rejects_bad_session_id() {
        let dir = tmp_dir("fork-bad");
        let mut log = SessionLog::open(&dir, "src").unwrap();
        log.append(EventInput::new(EventType::UserMessageAdded, "src", "a"))
            .unwrap();
        log.append(EventInput::new(EventType::ModelTurnCompleted, "src", "a"))
            .unwrap();
        assert!(log.fork_at_turn(1, "../escape").is_err());
        assert!(log.fork_at_turn(1, "a/b").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    // P45.5 — turn-boundary batching preserves the append-only invariant:
    // one write cycle, seq-ordered lines, receipts stable and derivable.
    #[test]
    fn append_batch_writes_one_cycle_in_order_with_stable_receipts() {
        let dir = tmp_dir("batch");
        {
            let mut log = SessionLog::open(&dir, "sess-1").unwrap();
            let seqs = log
                .append_batch([
                    EventInput::new(EventType::TaskStarted, "sess-1", "a"),
                    EventInput::new(EventType::ToolStarted, "sess-1", "a")
                        .with_tool("browser.act", "k1"),
                    EventInput::new(EventType::ToolCompleted, "sess-1", "a")
                        .with_tool("browser.act", "k1"),
                ])
                .unwrap();
            assert_eq!(seqs, vec![1, 2, 3]); // receipts stable, in order
            // A per-event append continues the same monotonic sequence.
            assert_eq!(
                log.append(EventInput::new(EventType::PlanCreated, "sess-1", "a"))
                    .unwrap(),
                4
            );
        }
        // Reopen: seq resumes from the batched writes; events read back in
        // exactly the written order (receipt-stable recovery).
        let log = SessionLog::open(&dir, "sess-1").unwrap();
        assert_eq!(log.seq(), 4);
        let evs = log.events().unwrap();
        let types: Vec<EventType> = evs.iter().map(|e| e.event_type).collect();
        assert_eq!(
            types,
            vec![
                EventType::TaskStarted,
                EventType::ToolStarted,
                EventType::ToolCompleted,
                EventType::PlanCreated,
            ]
        );
        assert_eq!(evs[1].tool, "browser.act");
        assert_eq!(evs[1].args_hash, "k1");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_batch_empty_is_a_noop_without_bumping_seq() {
        let dir = tmp_dir("batch-empty");
        let mut log = SessionLog::open(&dir, "sess-1").unwrap();
        let seqs = log.append_batch(std::iter::empty::<EventInput>()).unwrap();
        assert!(seqs.is_empty());
        assert_eq!(log.seq(), 0);
        assert!(log.events().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
