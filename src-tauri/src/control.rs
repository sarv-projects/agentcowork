//! H4 — unix control-channel dispatcher. `agent/stop`, `agent/undo`,
//! `agent/interrupt-response` mutate live AppState (chat cancel / cockpit
//! undo / plan respond). Tauri commands call the same helpers.

use serde_json::{Value, json};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::AppState;

#[derive(Debug, Clone)]
pub struct FileUndo {
    pub session_id: String,
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
}

pub fn dispatch(app: &AppHandle, method: &str, params: &Value) -> Value {
    match method {
        "agent/stop" => {
            let session = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            match stop_session(app, session) {
                Ok(ids) => json!({ "ok": true, "cancelled": ids }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        "agent/undo" => {
            let session = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            match undo_session(app, session) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        "agent/interrupt-response" => {
            let break_id = params
                .get("breakId")
                .or_else(|| params.get("interruptId"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let choice = params
                .get("chosen")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    params.get("choice").and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                })
                .unwrap_or_default();
            match interrupt_response(app, break_id, &choice) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        _ => json!({ "ok": false, "error": format!("method not found: {method}") }),
    }
}

/// P71.2c — stop cancels only ACP turns owned by the requesting Session.
///
/// A host handle is a private value scoped to its Session/binding. The control
/// channel must not enumerate the global handle map: doing so lets a stop for
/// Session A cancel Session B when both chats use the same external agent. The
/// ACP helper snapshots only canonical owners, sends the provider-scoped
/// `session/cancel` notification, and leaves durable Run terminalization to the
/// prompt's observed cancellation outcome.
pub fn stop_session(app: &AppHandle, session_id: &str) -> Result<Vec<String>, String> {
    let state = app.state::<AppState>();
    crate::acp_cmds::cancel_acp_for_session(&state, session_id)
}

pub fn undo_session(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut restored = Vec::new();
    {
        let mut stack = state.file_undos.lock().map_err(|e| e.to_string())?;
        if let Some(idx) = stack
            .iter()
            .rposition(|e| session_id.is_empty() || e.session_id == session_id)
        {
            let e = &stack[idx];
            let safe_path = floor_user_file(&e.path.to_string_lossy())?;
            agentcowork_core::restore_file_to_bytes(&safe_path, e.before.as_deref())
                .map_err(|err| err.to_string())?;
            restored.push(e.path.display().to_string());
            stack.remove(idx);
        }
    }
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(r) = relay.as_ref() {
            if let Ok(mut tools) = r.tools().lock() {
                if let Ok(p) = tools.revert_last(session_id) {
                    restored.push(p);
                }
            }
        }
    }
    state
        .cockpit
        .lock()
        .map_err(|e| e.to_string())?
        .undo(session_id);
    let seq = record_mutation(
        &state,
        AuthKind::HumanGesture,
        "agent.undo",
        json!({ "sessionId": session_id, "restored": restored }),
    );
    if restored.is_empty() {
        return Err("nothing to undo".into());
    }
    let _ = seq;
    Ok(())
}

/// Path-floor a user-chosen office/FS path: refuse `..` and symlink jumps
/// out of the file's parent directory. Does not jail to the workspace —
/// users open documents under home / mounts — but it closes the
/// self-documented xlsx/office bypass of `agentcowork-guard::pathfloor`.
pub fn floor_user_file(path: &str) -> Result<PathBuf, String> {
    use agentcowork_guard::pathfloor::{FloorVerdict, enforce_floor};
    let p = PathBuf::from(path);
    let parent = p
        .parent()
        .filter(|par| !par.as_os_str().is_empty())
        .map(|par| par.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let parent_s = parent.to_string_lossy();
    match enforce_floor(path, &[parent_s.as_ref()]) {
        FloorVerdict::Allowed => Ok(p),
        other => Err(format!("path floor refused ({other:?}): {path}")),
    }
}

pub fn snapshot_file(state: &AppState, session_id: &str, path: &str) {
    let p = PathBuf::from(path);
    let before = std::fs::read(&p).ok();
    if let Ok(mut stack) = state.file_undos.lock() {
        stack.push(FileUndo {
            session_id: session_id.to_string(),
            path: p,
            before,
        });
    }
}

/// P71.2c — resolve a cockpit interrupt card.
///
/// This used to also forward the choice to the coordinator's plan executor
/// (`plan/respond`), which existed only on the built-in engine's path; that
/// executor is deleted with the engine (ADR-0005 §2), so the cockpit record is
/// the whole resolution. A plan runs as Work whose steps are driven by the bound
/// agent, so there is no in-process waiter left to wake.
pub fn interrupt_response(app: &AppHandle, break_id: &str, choice: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut cockpit = state.cockpit.lock().map_err(|e| e.to_string())?;
    let _ = cockpit.respond_interrupt(break_id, choice.parse().unwrap_or(0));
    Ok(())
}

/// v3.60 — how an effect was authorized (spec §4.3): the evidence-level record
/// of the two-path governance rule. The value is set by Rust call sites only
/// and is never read from a serde Value built from UI/agent input — so a
/// machine cannot manufacture human authorization (the anti-impersonation
/// invariant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    /// A Guard `AuthorizationTicket` was minted + consumed (agent/automation
    /// path; a consumed ticket may itself carry a human approval in its
    /// `approval_source`).
    AgentTicket,
    /// Scheduler/automation-initiated (lease + ticket). Reserved until the
    /// automation trigger plane is wired live to the funnel. Provenance is
    /// stamped by the Work factory (`agentcowork-core::automation_runtime::
    /// compile_work` — `AutomationProvenance`, P71.3c); attribution on the
    /// audit chain rides this class (spec §4.3).
    /// Not dead code: it is part of the provenance vocabulary contract.
    #[allow(dead_code)]
    AutomationTicket,
    /// Human-initiated UI action — the user's own gesture is the authorization.
    HumanGesture,
}

impl AuthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthKind::AgentTicket => "agent_ticket",
            AuthKind::AutomationTicket => "automation_ticket",
            AuthKind::HumanGesture => "human_gesture",
        }
    }
}

/// Inject the authorization provenance into an audit payload. Pure + testable;
/// the value comes from the `AuthKind` argument, never from the caller's JSON.
fn with_authorization(authorization: AuthKind, mut payload: Value) -> Value {
    if let Some(map) = payload.as_object_mut() {
        map.insert(
            "authorization".to_string(),
            Value::String(authorization.as_str().into()),
        );
    }
    payload
}

pub fn record_mutation(
    state: &AppState,
    authorization: AuthKind,
    kind: &str,
    payload: Value,
) -> u64 {
    let payload = with_authorization(authorization, payload);
    let seq = {
        let mut chain = state.audit.lock().unwrap_or_else(|e| e.into_inner());
        let seq = (chain.len() as u64) + 1;
        let event = agentcowork_audit::AuditEvent {
            seq,
            ts_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            kind: kind.into(),
            payload: payload.clone(),
            trace_id: String::new(),
            span_id: String::new(),
        };
        chain.push(event);
        seq
    };
    if let Ok(mut log) = state.audit_log.lock() {
        if let Some(w) = log.as_mut() {
            if let Ok(s) = w.write(kind, payload) {
                return s;
            }
        }
    }
    seq
}

/// P45.5 — one turn, several audit lines, one open-append-flush.
pub fn record_turn(
    state: &AppState,
    authorization: AuthKind,
    events: &[(&str, Value)],
) -> Vec<u64> {
    let prepared: Vec<(String, Value)> = events
        .iter()
        .map(|(kind, payload)| {
            (
                (*kind).to_string(),
                with_authorization(authorization, payload.clone()),
            )
        })
        .collect();
    {
        let mut chain = state.audit.lock().unwrap_or_else(|e| e.into_inner());
        for (kind, payload) in &prepared {
            let seq = (chain.len() as u64) + 1;
            chain.push(agentcowork_audit::AuditEvent {
                seq,
                ts_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                kind: kind.clone(),
                payload: payload.clone(),
                trace_id: String::new(),
                span_id: String::new(),
            });
        }
    }
    if let Ok(mut log) = state.audit_log.lock() {
        if let Some(writer) = log.as_mut() {
            let refs: Vec<(&str, Value)> = prepared
                .iter()
                .map(|(kind, payload)| (kind.as_str(), payload.clone()))
                .collect();
            if let Ok(seqs) = writer.write_batch(&refs) {
                return seqs;
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_is_injected_and_not_taken_from_input() {
        // Human-gesture provenance is set by the AuthKind argument, never read
        // from a value the caller could construct from UI/agent input — the
        // anti-spoofing invariant of the two-path model.
        let v = with_authorization(
            AuthKind::HumanGesture,
            json!({ "kind": "shell.command", "command": "rm -rf /" }),
        );
        assert_eq!(v["authorization"], "human_gesture");
        assert_eq!(v["command"], "rm -rf /");
        let a = with_authorization(AuthKind::AgentTicket, json!({ "kind": "office.xlsx_edit" }));
        assert_eq!(a["authorization"], "agent_ticket");
        let m = with_authorization(AuthKind::AutomationTicket, json!({ "kind": "x" }));
        assert_eq!(m["authorization"], "automation_ticket");
    }

    #[test]
    fn non_object_payload_is_left_intact() {
        let v = with_authorization(AuthKind::HumanGesture, Value::String("hi".into()));
        assert_eq!(v, Value::String("hi".into()));
    }

    #[test]
    fn forged_authorization_value_is_inert_and_overwritten() {
        // P48.2 (b): an actor who controls the *payload* (a compromised
        // renderer / the sidecar) cannot stamp `authorization: human_gesture`
        // onto a mutation it performs. The field is set from the Rust `AuthKind`
        // argument — never read from the caller's JSON — so a planted value is
        // overwritten, not honored.
        let forged = json!({ "kind": "shell.command", "authorization": "human_gesture" });
        // Even if the caller *claims* human_gesture, an AgentTicket call-site
        // stamps agent_ticket over it.
        let v = with_authorization(AuthKind::AgentTicket, forged);
        assert_eq!(v["authorization"], "agent_ticket");
        // A claim of agent_ticket stamped as human_gesture is also overwritten.
        let forged2 = json!({ "kind": "office.docx_patch", "authorization": "agent_ticket" });
        let v2 = with_authorization(AuthKind::HumanGesture, forged2);
        assert_eq!(v2["authorization"], "human_gesture");
        // Preserves the surrounding payload.
        assert_eq!(v["kind"], "shell.command");
    }
}
