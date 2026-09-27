//! P7.7 — session repair: 7-phase validation of a session log for
//! corrupt-session recovery (doc 46 OpenFang pattern). Before a crashed
//! session is resumed, the log is walked through seven phases; any phase
//! failure produces a named [`RepairFinding`] the coordinator can act on
//! (resume from checkpoint, replay idempotent ops, or ask the user).

use crate::session_log::{EventType, IdempotencyClass, SessionEvent};
use std::collections::HashSet;

/// The seven validation phases (doc 46).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 1 — the log parses (every line is a valid event).
    Parse,
    /// 2 — sequence numbers are contiguous from 1.
    Sequence,
    /// 3 — every `ToolStarted` has a matching `ToolCompleted` (or is
    ///     classified for recovery).
    ToolPairing,
    /// 4 — event types appear in a legal order (no `ToolCompleted` before
    ///     its `ToolStarted`, no `CheckpointCommitted` before `PlanCreated`).
    Ordering,
    /// 5 — session/agent ids are consistent across the whole log.
    Identity,
    /// 6 — timestamps are non-decreasing (no clock skew within a session).
    TimeMonotonic,
    /// 7 — the last event is a terminal state (`CheckpointCommitted`,
    ///     `ModelTurnCompleted`) or the log is explicitly incomplete.
    Termination,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::Sequence => "sequence",
            Phase::ToolPairing => "tool-pairing",
            Phase::Ordering => "ordering",
            Phase::Identity => "identity",
            Phase::TimeMonotonic => "time-monotonic",
            Phase::Termination => "termination",
        }
    }
}

/// One named failure from a phase.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairFinding {
    pub phase: Phase,
    /// First offending seq (0 = log-level issue).
    pub seq: u64,
    pub message: String,
}

impl RepairFinding {
    fn new(phase: Phase, seq: u64, message: impl Into<String>) -> Self {
        Self {
            phase,
            seq,
            message: message.into(),
        }
    }
}

/// Result of the 7-phase validation.
#[derive(Debug, Clone)]
pub struct RepairReport {
    pub findings: Vec<RepairFinding>,
    /// Is the log healthy (no findings)?
    pub healthy: bool,
    /// Recommended recovery action.
    pub recommendation: RecoveryAction,
}

impl RepairReport {
    pub fn healthy() -> Self {
        Self {
            findings: Vec::new(),
            healthy: true,
            recommendation: RecoveryAction::Resume,
        }
    }

    pub fn with_findings(findings: Vec<RepairFinding>) -> Self {
        let healthy = findings.is_empty();
        let recommendation = if healthy {
            RecoveryAction::Resume
        } else if findings.iter().any(|f| f.phase == Phase::ToolPairing) {
            RecoveryAction::ReplayIdempotent
        } else if findings
            .iter()
            .any(|f| f.phase == Phase::Parse || f.phase == Phase::Sequence)
        {
            RecoveryAction::RestoreCheckpoint
        } else {
            RecoveryAction::AskUser
        };
        Self {
            findings,
            healthy,
            recommendation,
        }
    }
}

/// What the coordinator should do after repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Nothing wrong — resume.
    Resume,
    /// Re-run idempotent operations (safe retry by args-hash).
    ReplayIdempotent,
    /// Restore the last checkpoint and continue.
    RestoreCheckpoint,
    /// Surface to the user for a decision.
    AskUser,
}

/// Run the 7-phase validation over a session's events.
pub fn validate_session(events: &[SessionEvent]) -> RepairReport {
    let mut findings = Vec::new();

    // Phase 1 — parse (already deserialized; a nil event is a parse failure).
    if events.iter().any(|e| {
        e.seq == 0
            && e.event_type == EventType::UserMessageAdded
            && e.agent.is_empty()
            && events.len() > 1
    }) {
        findings.push(RepairFinding::new(
            Phase::Parse,
            0,
            "log contains a nil/unparseable event",
        ));
    }

    // Phase 2 — contiguous sequence from 1.
    for (i, e) in events.iter().enumerate() {
        let expected = (i as u64) + 1;
        if e.seq != expected {
            findings.push(RepairFinding::new(
                Phase::Sequence,
                e.seq,
                format!("seq {} at position {} (expected {expected})", e.seq, i + 1),
            ));
            break;
        }
    }

    // Phase 3 — tool pairing: every ToolStarted has a ToolCompleted with
    // the same (tool, args_hash) later in the log.
    let started: Vec<&SessionEvent> = events
        .iter()
        .filter(|e| e.event_type == EventType::ToolStarted)
        .collect();
    let mut completed: HashSet<(String, String)> = events
        .iter()
        .filter(|e| e.event_type == EventType::ToolCompleted)
        .map(|e| (e.tool.clone(), e.args_hash.clone()))
        .collect();
    for s in &started {
        if !completed.remove(&(s.tool.clone(), s.args_hash.clone())) {
            findings.push(RepairFinding::new(
                Phase::ToolPairing,
                s.seq,
                format!("ToolStarted {} ({}) has no completion", s.tool, s.args_hash),
            ));
        }
    }

    // Phase 4 — ordering legality.
    let mut saw_plan = false;
    let mut active_tools: HashSet<(String, String)> = HashSet::new();
    for e in events {
        match e.event_type {
            EventType::CheckpointCommitted if !saw_plan => findings.push(RepairFinding::new(
                Phase::Ordering,
                e.seq,
                "CheckpointCommitted before any PlanCreated",
            )),
            EventType::PlanCreated => saw_plan = true,
            EventType::ToolCompleted
                if !active_tools.remove(&(e.tool.clone(), e.args_hash.clone())) =>
            {
                findings.push(RepairFinding::new(
                    Phase::Ordering,
                    e.seq,
                    format!("ToolCompleted {} without active ToolStarted", e.tool),
                ));
            }
            EventType::ToolStarted => {
                active_tools.insert((e.tool.clone(), e.args_hash.clone()));
            }
            _ => {}
        }
    }

    // Phase 5 — identity consistency.
    let mut session_ids: HashSet<&str> = HashSet::new();
    let mut agent_ids: HashSet<&str> = HashSet::new();
    for e in events {
        session_ids.insert(e.session.as_str());
        agent_ids.insert(e.agent.as_str());
    }
    if session_ids.len() > 1 {
        findings.push(RepairFinding::new(
            Phase::Identity,
            0,
            format!("multiple session ids in one log: {:?}", session_ids),
        ));
    }
    if agent_ids.len() > 1 {
        findings.push(RepairFinding::new(
            Phase::Identity,
            0,
            format!("multiple agent ids in one log: {:?}", agent_ids),
        ));
    }

    // Phase 6 — timestamps non-decreasing.
    let mut last_ts = 0u64;
    for e in events {
        if e.ts_ms < last_ts {
            findings.push(RepairFinding::new(
                Phase::TimeMonotonic,
                e.seq,
                format!("ts {} earlier than previous {}", e.ts_ms, last_ts),
            ));
            break;
        }
        last_ts = e.ts_ms;
    }

    // Phase 7 — termination: last event is terminal or the log is empty.
    if let Some(last) = events.last() {
        let terminal = matches!(
            last.event_type,
            EventType::CheckpointCommitted | EventType::ModelTurnCompleted
        );
        if !terminal {
            findings.push(RepairFinding::new(
                Phase::Termination,
                last.seq,
                "log ends on a non-terminal event (interrupted session)",
            ));
        }
    }

    RepairReport::with_findings(findings)
}

/// Convenience: map tool → idempotency class for the replay decision
/// (safe_retry can be re-run; unsafe_retry must ask).
pub fn replayable_tools(events: &[SessionEvent]) -> Vec<String> {
    let mut tools: HashSet<&str> = events
        .iter()
        .filter(|e| e.event_type == EventType::ToolStarted)
        .map(|e| e.tool.as_str())
        .collect();
    let mut v: Vec<String> = tools.drain().map(|t| t.to_string()).collect();
    v.sort();
    v
}

/// v3.39 — classification of an incomplete tool start into a repair plan.
#[derive(Debug, Clone, PartialEq)]
pub struct StartedUnknownItem {
    pub seq: u64,
    pub tool: String,
    pub args_hash: String,
    /// NeverStarted = no dispatch evidence (ToolStarted only, no execution).
    /// StartedUnknown = dispatch happened but completion is missing.
    pub classification: StartedUnknownClassification,
    /// True when the tool is a mutation and started-unknown (needs user
    /// confirmation before replay).
    pub needs_confirmation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedUnknownClassification {
    /// ToolStarted found but no dispatch evidence (no audit row for this
    /// tool+args_hash). Safe to re-run.
    NeverStarted,
    /// ToolStarted found AND an audit row (or other dispatch evidence)
    /// exists but ToolCompleted is missing. Outcome is uncertain — must
    /// not blindly replay a mutation.
    StartedUnknown,
}

/// v3.39 — classify incomplete tool starts into a repair plan.
///
/// `dispatch_evidence` is a set of `(tool, args_hash)` pairs from the
/// audit log that prove dispatch actually happened (e.g. tool.exec rows
/// in `audit.ndjson`). An incomplete tool with no audit evidence is
/// `NeverStarted`; one with evidence is `StartedUnknown`.
pub fn started_unknown_repair(
    incomplete: &[SessionEvent],
    dispatch_evidence: &HashSet<(String, String)>,
) -> Vec<StartedUnknownItem> {
    incomplete
        .iter()
        .map(|ev| {
            let has_dispatch = dispatch_evidence.contains(&(ev.tool.clone(), ev.args_hash.clone()));
            let is_mutation = matches!(
                crate::session_log::classify_tool(&ev.tool),
                IdempotencyClass::UnsafeRetry | IdempotencyClass::SameKey
            );
            let classification = if has_dispatch {
                StartedUnknownClassification::StartedUnknown
            } else {
                StartedUnknownClassification::NeverStarted
            };
            StartedUnknownItem {
                seq: ev.seq,
                tool: ev.tool.clone(),
                args_hash: ev.args_hash.clone(),
                classification,
                needs_confirmation: is_mutation && has_dispatch,
            }
        })
        .collect()
}

/// The 7 phases in order (for the repair UI / audit trail).
pub const PHASES: [Phase; 7] = [
    Phase::Parse,
    Phase::Sequence,
    Phase::ToolPairing,
    Phase::Ordering,
    Phase::Identity,
    Phase::TimeMonotonic,
    Phase::Termination,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(
        seq: u64,
        ts: u64,
        et: EventType,
        tool: &str,
        args: &str,
        session: &str,
        agent: &str,
    ) -> SessionEvent {
        SessionEvent {
            seq,
            ts_ms: ts,
            session: session.to_string(),
            agent: agent.to_string(),
            tool: tool.to_string(),
            args_hash: args.to_string(),
            result_meta: serde_json::Value::Null,
            trace_id: String::new(),
            span_id: String::new(),
            event_type: et,
        }
    }

    fn healthy_log() -> Vec<SessionEvent> {
        vec![
            ev(1, 100, EventType::UserMessageAdded, "", "", "s1", "a1"),
            ev(2, 101, EventType::PlanCreated, "", "", "s1", "a1"),
            ev(3, 102, EventType::ToolStarted, "fs.read", "h1", "s1", "a1"),
            ev(
                4,
                103,
                EventType::ToolCompleted,
                "fs.read",
                "h1",
                "s1",
                "a1",
            ),
            ev(5, 104, EventType::CheckpointCommitted, "", "", "s1", "a1"),
        ]
    }

    #[test]
    fn healthy_log_passes() {
        let r = validate_session(&healthy_log());
        assert!(r.healthy, "findings: {:?}", r.findings);
        assert_eq!(r.recommendation, RecoveryAction::Resume);
    }

    #[test]
    fn missing_completion_detected() {
        let mut log = healthy_log();
        log.remove(3); // drop ToolCompleted
        let r = validate_session(&log);
        assert!(!r.healthy);
        assert!(r.findings.iter().any(|f| f.phase == Phase::ToolPairing));
        assert_eq!(r.recommendation, RecoveryAction::ReplayIdempotent);
    }

    #[test]
    fn sequence_gap_detected() {
        let mut log = healthy_log();
        log[2].seq = 9;
        let r = validate_session(&log);
        assert!(r.findings.iter().any(|f| f.phase == Phase::Sequence));
    }

    #[test]
    fn non_terminal_end_detected() {
        let mut log = healthy_log();
        log.pop(); // remove CheckpointCommitted
        let r = validate_session(&log);
        assert!(r.findings.iter().any(|f| f.phase == Phase::Termination));
    }

    #[test]
    fn identity_mismatch_detected() {
        let mut log = healthy_log();
        log[4].session = "s2".to_string();
        let r = validate_session(&log);
        assert!(r.findings.iter().any(|f| f.phase == Phase::Identity));
    }

    #[test]
    fn time_skew_detected() {
        let mut log = healthy_log();
        log[3].ts_ms = 50; // earlier than seq 2's 101
        let r = validate_session(&log);
        assert!(r.findings.iter().any(|f| f.phase == Phase::TimeMonotonic));
    }

    #[test]
    fn unordered_completion_detected() {
        let log = vec![
            ev(1, 100, EventType::PlanCreated, "", "", "s1", "a1"),
            ev(
                2,
                101,
                EventType::ToolCompleted,
                "fs.read",
                "h1",
                "s1",
                "a1",
            ),
            ev(3, 102, EventType::CheckpointCommitted, "", "", "s1", "a1"),
        ];
        let r = validate_session(&log);
        assert!(r.findings.iter().any(|f| f.phase == Phase::Ordering));
    }

    #[test]
    fn started_unknown_repair_classifies_correctly() {
        // Two incomplete tools: one safe (browser.read) and one mutation (file.write).
        let incomplete = vec![
            ev(
                3,
                102,
                EventType::ToolStarted,
                "browser.read",
                "h_read",
                "s1",
                "a1",
            ),
            ev(
                4,
                103,
                EventType::ToolStarted,
                "file.write",
                "h_write",
                "s1",
                "a1",
            ),
        ];
        // Only file.write has dispatch evidence.
        let mut dispatch = HashSet::new();
        dispatch.insert(("file.write".into(), "h_write".into()));
        let plan = started_unknown_repair(&incomplete, &dispatch);
        assert_eq!(plan.len(), 2);
        // browser.read: no dispatch → NeverStarted, no confirmation needed.
        assert_eq!(
            plan[0].classification,
            StartedUnknownClassification::NeverStarted
        );
        assert!(!plan[0].needs_confirmation);
        // file.write: has dispatch → StartedUnknown, mutation → needs confirmation.
        assert_eq!(
            plan[1].classification,
            StartedUnknownClassification::StartedUnknown
        );
        assert!(plan[1].needs_confirmation);
    }

    #[test]
    fn started_unknown_empty_dispatch_all_never_started() {
        let incomplete = vec![ev(
            3,
            102,
            EventType::ToolStarted,
            "browser.act",
            "h1",
            "s1",
            "a1",
        )];
        let dispatch = HashSet::new();
        let plan = started_unknown_repair(&incomplete, &dispatch);
        assert_eq!(plan.len(), 1);
        assert_eq!(
            plan[0].classification,
            StartedUnknownClassification::NeverStarted
        );
        assert!(!plan[0].needs_confirmation);
    }

    #[test]
    fn started_unknown_safe_tool_with_dispatch_not_needing_confirmation() {
        let incomplete = vec![ev(
            3,
            102,
            EventType::ToolStarted,
            "browser.read",
            "h1",
            "s1",
            "a1",
        )];
        let mut dispatch = HashSet::new();
        dispatch.insert(("browser.read".into(), "h1".into()));
        let plan = started_unknown_repair(&incomplete, &dispatch);
        assert_eq!(
            plan[0].classification,
            StartedUnknownClassification::StartedUnknown
        );
        // Safe tool (read-only) → no confirmation even with dispatch evidence.
        assert!(!plan[0].needs_confirmation);
    }
}
