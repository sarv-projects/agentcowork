//! H36 (P54/P67) — integrated terminal: profile registry + one real PTY plane.
//!
//! **One plane.** There is exactly one PTY host (`AppState.terminal`). A human
//! tab, an agent `script.run`, and a durable task all spawn through it, and all
//! three appear in the same Shell view — the difference is *provenance*
//! (`TerminalOrigin`), which the UI renders as a read-only, labelled tab so the
//! user can watch the agent work instead of trusting an invisible pipe. The old
//! `shell_cmds.rs` piped `sh -i`/`cmd` path is gone from the product surface.
//!
//! **Authority boundary.** The renderer never sends an executable path — it
//! sends a *profile name*. This module re-runs `agentcowork-core`'s detection on
//! the shell side and resolves the name against that registry, so a
//! compromised renderer cannot ask the shell to spawn an arbitrary binary or a
//! profile the user has not confirmed. Unsafe profiles (world-writable install
//! dirs) are refused until confirmed in `terminal.unsafeConfirmed`.
//!
//! **Audit.** Session lifecycle (`terminal.spawn` / `terminal.kill`) is written
//! to the same Merkle chain as every other effect. Individual keystrokes are
//! deliberately *not* audited: a PTY carries passwords (`sudo`, `ssh`,
//! `gh auth`), and the audit log is retained — writing raw input would turn the
//! ledger into a credential leak. An *agent* command is different: it is the
//! agent's own decision, so it is recorded as such.
//!
//! **Backends.** `Local` and `Wsl` spawn here. `Remote` (P54.7, H33 attach) has
//! no implementation yet and fails closed with `remote_unavailable` — never a
//! silent local fallback, which would run a command on the wrong machine.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
// `Manager` brings `AppHandle::state()` into scope (the `script.run` executor
// reaches the managed shell state to record its own audit entry).
use tauri::{AppHandle, Emitter, Manager, State};

use agentcowork_core::terminal::{
    detect_available_profiles, CommandRecord, CommandTracker, DetectedProfile, Platform, PtyFrame,
    PtyHost, SpawnOpts, TerminalBackend, TerminalConfig, TerminalOrigin, TerminalPlaneObserver,
};
use agentcowork_core::Config;

use crate::AppState;

/// Max PTY dimension accepted from the renderer (a bogus resize would be
/// forwarded to the kernel verbatim).
const MAX_DIM: u16 = 1000;

/// P68.9 — how long `script.run` waits for the shell to report its command
/// finished. The wait returns the moment the shell reports, so this is only the
/// ceiling for a command whose completion the shell never announces (an
/// interactive prompt, for example).
const AGENT_RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// P68.9 — poll interval while waiting for the shell's completion record.
const AGENT_RUN_POLL: Duration = Duration::from_millis(20);

/// P68.9 — how long to wait for output to settle when the session has no shell
/// integration: no completion record will ever arrive, so there is nothing
/// meaningful to block on and we return the retained output as unverified.
const AGENT_RUN_SETTLE: Duration = Duration::from_millis(750);

fn profile_json(p: &DetectedProfile, confirmed: &[String], hide_unsafe: bool) -> serde_json::Value {
    serde_json::json!({
        "profileName": p.profile_name,
        "path": p.path,
        "backend": p.backend,
        "icon": p.icon,
        "source": p.source,
        "wslDistro": p.wsl_distro,
        "isUnsafePath": p.is_unsafe_path,
        "isFromPath": p.is_from_path,
        "isAutoDetected": p.is_auto_detected,
        "isDefault": p.is_default,
        "offered": p.offered(confirmed, hide_unsafe),
    })
}

fn backend_label(b: TerminalBackend) -> &'static str {
    match b {
        TerminalBackend::Local => "local",
        TerminalBackend::Wsl => "wsl",
        TerminalBackend::Remote => "remote",
    }
}

fn origin_label(o: TerminalOrigin) -> &'static str {
    match o {
        TerminalOrigin::Human => "human",
        TerminalOrigin::Agent => "agent",
        TerminalOrigin::Task => "task",
    }
}

/// Current platform key for the registry (`windows` | `linux` | `macos`).
fn platform_key() -> &'static str {
    match Platform::current() {
        Platform::Windows => "windows",
        Platform::Macos => "macos",
        Platform::Linux => "linux",
    }
}

/// Resolve a renderer-supplied profile name against the shell's own detection.
/// Refuses unknown, unsafe-unconfirmed, hidden, and `Remote` profiles.
fn resolve_profile(cfg: &Config, name: &str) -> Result<DetectedProfile, String> {
    let detected = detect_available_profiles(&cfg.terminal);
    let confirmed = cfg.terminal.unsafe_confirmed_names().to_vec();
    let profile = detected
        .into_iter()
        .find(|p| p.profile_name == name)
        .ok_or_else(|| format!("unknown terminal profile: {name}"))?;
    if !profile.offered(&confirmed, cfg.terminal.hidden_unsafe) {
        return Err(format!(
            "terminal profile {name} uses an unconfirmed unsafe install path — \
             confirm it in Settings → Terminal first"
        ));
    }
    if profile.backend == TerminalBackend::Remote {
        // P54.7 — H33 attach is not implemented; fail closed, never local.
        return Err(
            "remote terminal backend not available in this build (H33 attach pending)".into(),
        );
    }
    Ok(profile)
}

/// P54.1 — detected profiles + registry state for the Shell view's `+`
/// dropdown and Settings → Terminal. Detection runs on the shell side; the
/// renderer receives the resolved registry, never the raw config file.
#[tauri::command]
pub fn terminal_profiles() -> Result<serde_json::Value, String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    let confirmed = cfg.terminal.unsafe_confirmed_names().to_vec();
    let hidden = cfg.terminal.hidden_unsafe;
    let detected = detect_available_profiles(&cfg.terminal);
    let profiles: Vec<serde_json::Value> = detected
        .iter()
        .map(|p| profile_json(p, &confirmed, hidden))
        .collect();
    Ok(serde_json::json!({
        "platform": platform_key(),
        "profiles": profiles,
        "defaultProfile": cfg.terminal.default_profile_name(),
        "automationProfile": cfg.terminal.automation_profile_name(),
        "useWslProfiles": cfg.terminal.use_wsl_profiles,
        "unsafeConfirmed": confirmed,
        "hostAbiVersion": TerminalBackend::HOST_ABI_VERSION,
        "shellIntegration": cfg.terminal.shell_integration,
        // P54.7 — honest capability flag for the UI (Remote ships post-v1).
        "remoteBackendAvailable": false,
    }))
}

/// Persist mutated `terminal.*` back to `agentcowork.toml`.
fn save_terminal(mutate: impl FnOnce(&mut TerminalConfig)) -> Result<(), String> {
    let path = Config::config_path().map_err(|e| e.to_string())?;
    let mut cfg = Config::load().map_err(|e| e.to_string())?;
    mutate(&mut cfg.terminal);
    cfg.save(&path).map_err(|e| e.to_string())
}

/// P54.1 — Select Default Profile. Only a profile that detection actually
/// offers may become the default (no typo'd name silently becoming a default).
#[tauri::command]
pub fn terminal_set_default(name: String) -> Result<bool, String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    let confirmed = cfg.terminal.unsafe_confirmed_names().to_vec();
    let ok = detect_available_profiles(&cfg.terminal)
        .iter()
        .any(|p| p.profile_name == name && p.offered(&confirmed, cfg.terminal.hidden_unsafe));
    if !ok {
        return Err(format!("cannot set default: profile {name} is not offered"));
    }
    save_terminal(|t| {
        *t.default_profile.platform_mut() = name;
    })?;
    Ok(true)
}

/// P54.5 — automation profile for tasks / agent `script.run`. `None` clears it
/// (falls back to the interactive default).
#[tauri::command]
pub fn terminal_set_automation(name: Option<String>) -> Result<Option<String>, String> {
    if let Some(n) = &name {
        let cfg = Config::load().map_err(|e| e.to_string())?;
        let confirmed = cfg.terminal.unsafe_confirmed_names().to_vec();
        let ok = detect_available_profiles(&cfg.terminal)
            .iter()
            .any(|p| &p.profile_name == n && p.offered(&confirmed, cfg.terminal.hidden_unsafe));
        if !ok {
            return Err(format!(
                "cannot set automation shell: profile {n} is not offered"
            ));
        }
    }
    save_terminal(|t| {
        let slot = t.automation_profile.platform_mut();
        match name {
            Some(n) => *slot = n,
            // Clearing the platform slot is the only honest "unset" in toml.
            None => t.automation_profile = Default::default(),
        }
    })?;
    let cfg = Config::load().map_err(|e| e.to_string())?;
    Ok(cfg.terminal.automation_profile_name().map(String::from))
}

/// P54.6 — confirm an unsafe install dir so its profile becomes offerable.
#[tauri::command]
pub fn terminal_confirm_unsafe(name: String) -> Result<Vec<String>, String> {
    save_terminal(|t| {
        let list = t.unsafe_confirmed.platform_mut();
        if !list.contains(&name) {
            list.push(name);
        }
    })?;
    let cfg = Config::load().map_err(|e| e.to_string())?;
    Ok(cfg.terminal.unsafe_confirmed_names().to_vec())
}

/// P67 — the `terminal.*` settings the Shell view and Settings render.
#[tauri::command]
pub fn terminal_get_shell_integration() -> Result<bool, String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    Ok(cfg.terminal.shell_integration)
}

/// P67 — toggle OSC 633 shell integration. Off means a plain terminal with no
/// structured facts (no cwd, no exit codes, no command history for the agent).
#[tauri::command]
pub fn terminal_set_shell_integration(enabled: bool) -> Result<bool, String> {
    save_terminal(|t| t.shell_integration = enabled)?;
    let cfg = Config::load().map_err(|e| e.to_string())?;
    Ok(cfg.terminal.shell_integration)
}

/// Start the reader thread for a spawned PTY, mapping `PtyFrame`s onto the
/// `terminal-event` channel. Every spawn path funnels through here so all
/// three provenance kinds stream identically.
fn stream_frames(
    app: AppHandle,
    pty_id: String,
    output: agentcowork_core::terminal::PtyOutput,
) -> Result<(), String> {
    let id = pty_id.clone();
    output
        .stream(move |frame| match frame {
            PtyFrame::Data(bytes) => {
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let _ = app.emit(
                    "terminal-event",
                    serde_json::json!({ "ptyId": id, "kind": "data", "data": data }),
                );
            }
            PtyFrame::Command(rec) => {
                let _ = app.emit(
                    "terminal-event",
                    serde_json::json!({
                        "ptyId": id,
                        "kind": "command",
                        "data": "",
                        "command": {
                            "command": rec.command,
                            "cwd": rec.cwd,
                            "exitCode": rec.exit_code,
                            "output": rec.output,
                            "trusted": rec.trusted,
                            "failed": rec.failed(),
                        },
                    }),
                );
            }
            PtyFrame::Cwd(path) => {
                let _ = app.emit(
                    "terminal-event",
                    serde_json::json!({ "ptyId": id, "kind": "cwd", "data": "", "cwd": path }),
                );
            }
            PtyFrame::Exit(code) => {
                let _ = app.emit(
                    "terminal-event",
                    serde_json::json!({ "ptyId": id, "kind": "exit", "data": "", "code": code }),
                );
            }
        })
        .map_err(|e| format!("start pty reader: {e}"))
}

/// The provenance one spawn records: who acted, under which label, authorized
/// by which ticket, plus the subject payload.
///
/// FIX-06: grouping these keeps the spawn tail's signature honest and forces
/// every spawn path to state its authority explicitly — `ticket_id` is part of
/// the record, so a path that has no ticket has to say so instead of defaulting
/// to an implied one. The audit row must name the ticket that actually
/// authorized the effect, so the provenance is checkable rather than asserted
/// (`ARCH/10-KERNEL.md` §7 — the envelope's idempotency key is
/// `<work_id>:<ticket>`; the audit row carries the same link).
struct SpawnProvenance<'a> {
    origin: TerminalOrigin,
    label: Option<&'a str>,
    ticket_id: Option<&'a str>,
    subject: serde_json::Value,
}

/// Shared spawn tail: record authority, start the reader, and return the id.
fn finish_spawn(
    app: &AppHandle,
    state: &State<'_, AppState>,
    pty_id: String,
    output: agentcowork_core::terminal::PtyOutput,
    prov: SpawnProvenance<'_>,
) -> Result<String, String> {
    stream_frames(app.clone(), pty_id.clone(), output)?;
    // v3.59 governance decision — the audit kind tracks who acted: a user
    // gesture for a human tab, an agent authorization for `script.run`.
    let (kind, subject) = match prov.origin {
        TerminalOrigin::Human => (crate::control::AuthKind::HumanGesture, "terminal.spawn"),
        TerminalOrigin::Agent => (crate::control::AuthKind::AgentTicket, "terminal.agent_run"),
        TerminalOrigin::Task => (
            crate::control::AuthKind::AutomationTicket,
            "terminal.task_run",
        ),
    };
    let mut payload = prov.subject;
    if let Some(l) = prov.label {
        payload["label"] = serde_json::json!(l);
    }
    payload["origin"] = serde_json::json!(origin_label(prov.origin));
    if let Some(tid) = prov.ticket_id {
        payload["ticketId"] = serde_json::json!(tid);
    }
    crate::control::record_mutation(state, kind, subject, payload);
    Ok(pty_id)
}

/// P54.3 — spawn a detected profile into a real PTY (human tab).
#[tauri::command]
pub fn terminal_spawn(
    app: AppHandle,
    state: State<'_, AppState>,
    profile: String,
    cwd: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    let resolved = resolve_profile(&cfg, &profile)?;

    // A renderer-supplied cwd is only honoured when it is an existing dir.
    let cwd = match cwd {
        Some(c) if !c.is_empty() => {
            let path = std::path::PathBuf::from(&c);
            if !path.is_dir() {
                return Err(format!("terminal cwd is not a directory: {c}"));
            }
            Some(c)
        }
        _ => None,
    };

    let rows = rows.clamp(1, MAX_DIM);
    let cols = cols.clamp(1, MAX_DIM);

    let (pty_id, output) = state
        .terminal
        .spawn_with(
            &resolved,
            cwd.as_deref(),
            rows,
            cols,
            SpawnOpts {
                origin: TerminalOrigin::Human,
                integration: cfg.terminal.shell_integration,
            },
        )
        .map_err(|e| e.to_string())?;

    finish_spawn(
        &app,
        &state,
        pty_id,
        output,
        SpawnProvenance {
            origin: TerminalOrigin::Human,
            label: None,
            // A human tab is authorized by the user's own gesture, not a ticket.
            ticket_id: None,
            subject: serde_json::json!({
                "profileId": resolved.profile_name,
                "backend": backend_label(resolved.backend),
                "rows": rows,
                "cols": cols,
            }),
        },
    )
}

/// P68.9 — resolve the shell an *agent* or *task* command runs in: the
/// automation profile, falling back to the interactive default so a fresh
/// install still works.
///
/// One resolution shared by `terminal_run` and the `script.run` executor, so
/// the renderer path and the model path cannot end up on different shells.
fn resolve_automation_profile(cfg: &Config) -> Result<DetectedProfile, String> {
    let name = cfg
        .terminal
        .automation_profile_name()
        .or_else(|| cfg.terminal.default_profile_name())
        .ok_or_else(|| "no automation or default terminal profile is configured".to_string())?;
    resolve_profile(cfg, name)
}

/// P68.9 — non-empty command lines we are about to send. Each one becomes its
/// own shell-reported record, which is what lets the completion wait know how
/// many records to expect.
fn command_line_count(command: &str) -> usize {
    command.lines().filter(|l| !l.trim().is_empty()).count()
}

/// P68.9 — spawn an agent/task command on the **one PTY plane** and hand back
/// its id and tracker.
///
/// `terminal_run` (renderer) and the `script.run` executor (tool loop) both go
/// through here, so profile resolution, provenance, stream wiring and the audit
/// kind have exactly one implementation — an agent command cannot be audited
/// one way from the UI and another way from the model's own tool call.
fn spawn_agent_command(
    app: &AppHandle,
    state: &State<'_, AppState>,
    command: &str,
    rows: Option<u16>,
    cols: Option<u16>,
    prov: SpawnProvenance<'_>,
) -> Result<(String, Arc<Mutex<CommandTracker>>), String> {
    let cfg = Config::load().map_err(|e| e.to_string())?;
    // Prefer the automation profile; fall back to the interactive default so a
    // fresh install still works, and name which one was used in the audit.
    let resolved = resolve_automation_profile(&cfg)?;

    let rows = rows.unwrap_or(24).clamp(1, MAX_DIM);
    let cols = cols.unwrap_or(80).clamp(1, MAX_DIM);

    let (pty_id, output) = state
        .terminal
        .spawn_with(
            &resolved,
            None,
            rows,
            cols,
            SpawnOpts {
                origin: prov.origin,
                integration: cfg.terminal.shell_integration,
            },
        )
        .map_err(|e| e.to_string())?;

    // Grab the tracker before writing: the reader thread can start producing
    // records the instant the first line lands, and a tracker taken afterwards
    // could miss the earliest ones.
    let tracker = state
        .terminal
        .tracker(&pty_id)
        .ok_or_else(|| "terminal: session vanished immediately after spawn".to_string())?;

    // Send the command(s). The shell echoes them and its integration script
    // reports `E` (line) + `D` (exit) back to us.
    for line in command.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        state
            .terminal
            .write(&pty_id, format!("{line}\r").as_bytes())
            .map_err(|e| format!("terminal: write failed: {e}"))?;
    }

    finish_spawn(
        app,
        state,
        pty_id.clone(),
        output,
        SpawnProvenance {
            subject: serde_json::json!({
                "profileId": resolved.profile_name,
                "command": command.trim(),
                "rows": rows,
                "cols": cols,
            }),
            ..prov
        },
    )?;
    Ok((pty_id, tracker))
}

/// FIX-06 — the Guard-2 pre-flight for a renderer-initiated agent/task
/// command.
///
/// `terminal_run` used to spawn the command and then stamp the audit row
/// `authorization: agent_ticket` while **no ticket existed** — a false
/// provenance claim on a real shell effect (INV-01, INV-03, REQ-TRUST-003,
/// REQ-TRUST-005). Any renderer JS could run an arbitrary command line with a
/// forged authority record and no policy decision at all.
///
/// Now the command goes through the same decider as every other effect:
///
/// 1. Guard-1 prescan of the command line (the deterministic destructive
///    blocklist) — a hit is refused outright.
/// 2. A Guard-2 ticket is minted for `Operation::TerminalShell` and consumed
///    immediately. The consumed `ticketId` is what the audit row names, so the
///    provenance is true by construction rather than asserted.
///
/// A `Deny` blocks, and an `Ask` is refused: this command has no approval card,
/// so the only honest answer is to refuse and point the caller at the governed
/// path (the `script.run` tool, which mints a real card). Failing closed is
/// the contract (`ARCH/12-TRUST.md` §11) — an approval is never auto-granted
/// here to make the call succeed.
fn ticket_for_agent_command(
    state: &State<'_, AppState>,
    command: &str,
    origin: TerminalOrigin,
) -> Result<String, String> {
    use agentcowork_guard::{DecisionPackage, Operation as GuardOp, RiskLevel, prescan};
    use std::hash::{Hash, Hasher};

    // 1) Guard-1: the deterministic blocklist, on the exact command line.
    //    `scan_shell` returns the blocklist indices that matched; non-empty
    //    means a known-destructive pattern, which is refused outright (the
    //    same corpus the tool executor's `tool/exec` prescan refuses on).
    let hits = prescan::scan_shell(command);
    if !hits.is_empty() {
        return Err(format!(
            "terminal_run refused: Guard-1 blocked this command (blocklist patterns {hits:?})"
        ));
    }

    let mut decision = DecisionPackage::new(format!("terminal {}", origin_label(origin)));
    decision.risk = RiskLevel::High;
    decision.script_lines = vec![command.to_string()];

    let mut h = std::collections::hash_map::DefaultHasher::new();
    "terminal.command".hash(&mut h);
    command.hash(&mut h);
    origin_label(origin).hash(&mut h);
    let args_hash = format!("{:016x}", h.finish());

    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let verdict = guard.evaluate(
        "terminal",
        "everyaios",
        "terminal.run",
        // `destructive` carries the Guard-1 flag (permissions §
        // `ask_if_destructive`). Every command Guard-1 flags was already
        // refused above, so the flag reaching the policy layer is false here
        // — which is exactly right: the default rule then admits a benign
        // command, while a user's own `terminal_shell = always_ask` /
        // `block` rule in `permissions.toml` still tightens or refuses it.
        GuardOp::TerminalShell { destructive: false },
        decision,
        &args_hash,
        0,
    );
    let ticket_id = match verdict {
        agentcowork_core::GuardDecision::Allow { ticket_id } => ticket_id,
        agentcowork_core::GuardDecision::Ask { .. } => {
            return Err(
                "terminal_run refused: this command needs an approval card — run it through the \
                 governed `script.run` tool so the decision is recorded"
                    .into(),
            );
        }
        agentcowork_core::GuardDecision::Block { reason } => {
            return Err(format!("terminal_run refused: {reason}"));
        }
    };
    // Consume it here so the spawn below runs on a spent, args-bound ticket.
    // A losing concurrent execution cannot reuse it (`EDGE-039`).
    guard
        .use_ticket(&ticket_id, &args_hash)
        .map_err(|e| format!("terminal_run refused: ticket refused: {e}"))?;
    Ok(ticket_id)
}

/// P67 — run a command for the agent / a durable task in a real PTY, on the
/// *automation* profile, and let the shell report it.
///
/// This is P54.5's consumer: instead of hand-rolling `sh -c` with piped stdio,
/// the command runs in the same PTY host, is authorised, and shows up in the
/// Shell view as a labelled read-only tab — so "watch the agent work" is a real
/// property rather than a claim. The command line and exit code come from the
/// shell's own `PS0`/`PROMPT_COMMAND` reporting, not from our guessing.
///
/// Newlines are sent as separate commands on purpose: partially-applied
/// multi-line shell input is not something we can report honestly.
#[tauri::command]
pub fn terminal_run(
    app: AppHandle,
    state: State<'_, AppState>,
    command: String,
    label: Option<String>,
    origin: Option<String>,
    rows: Option<u16>,
    cols: Option<u16>,
) -> Result<String, String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err("terminal_run: command must not be empty".into());
    }
    let origin = match origin.as_deref() {
        Some("task") => TerminalOrigin::Task,
        // Anything else is treated as an agent run; the caller cannot ask for
        // a *human* session through this path (that would launder authority).
        _ => TerminalOrigin::Agent,
    };
    // FIX-06: the ticket is minted and consumed *before* the spawn, and its id
    // is the provenance the audit row records.
    let ticket_id = ticket_for_agent_command(&state, trimmed, origin)?;
    let (pty_id, _tracker) = spawn_agent_command(
        &app,
        &state,
        trimmed,
        rows,
        cols,
        SpawnProvenance {
            origin,
            label: label.as_deref(),
            ticket_id: Some(&ticket_id),
            subject: serde_json::Value::Null,
        },
    )?;
    Ok(pty_id)
}

/// P68.8 — replay a session's retained output after a view reattaches.
///
/// Bytes come back base64-encoded, exactly like a live `terminal-event` frame,
/// so the renderer can hand them to the same decoder and xterm keeps ownership
/// of VT interpretation. `dropped > 0` means the caller's cursor had already
/// been evicted from the ring: the replay is then a *truncated* view and the UI
/// must say so rather than present a seamless scrollback that hides a gap.
#[tauri::command]
pub fn terminal_replay(
    state: State<'_, AppState>,
    pty_id: String,
    from_seq: Option<u64>,
) -> Result<serde_json::Value, String> {
    let replayed = state
        .terminal
        .replay(&pty_id, from_seq)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "ptyId": pty_id,
        "seq": replayed.seq,
        "data": base64::engine::general_purpose::STANDARD.encode(&replayed.bytes),
        "dropped": replayed.dropped,
        "capacity": replayed.capacity,
    }))
}

/// P68.9 — wait until the shell reports `expected` finished commands, or the
/// deadline passes.
///
/// Completion here is the shell's own statement (an `E`/`D` pair attributed to
/// a session nonce), never a quiet-output heuristic: "the command stopped
/// printing" is not "the command finished". A record that cannot be attributed
/// is reported with `trusted: false` and the caller must not present it as
/// fact.
fn await_command_records(
    tracker: &Arc<Mutex<CommandTracker>>,
    expected: usize,
    deadline: Instant,
) -> Option<CommandRecord> {
    loop {
        if let Ok(t) = tracker.lock() {
            if t.len() >= expected {
                if let Some(rec) = t.last() {
                    return Some(rec.clone());
                }
            }
        }
        if Instant::now() >= deadline {
            // The shell never announced completion. Hand back whatever it did
            // report — `exit_code: None` is the absence of evidence, not a
            // success, and the executor surfaces that to the model.
            return tracker.lock().ok().and_then(|t| t.last().cloned());
        }
        std::thread::sleep(AGENT_RUN_POLL);
    }
}

/// P68.9 — the `script.run` executor.
///
/// The agent's command runs on the **one PTY plane** with `Agent` provenance:
/// same host a human tab uses, on the automation profile, audited as
/// `terminal.agent_run`, and rendered as a labelled read-only tab. It blocks
/// until the shell reports the command finished, because a tool result the
/// model cannot see is not a tool result.
///
/// When the session has no shell integration there is no completion signal to
/// wait for, so it returns the session's retained output with `trusted: false`
/// and `exit_code: None` — absent evidence, never an implied success.
///
/// It never spawns a `Human`-origin session: neither the renderer nor the model
/// may launder authority by asking the agent path for a user terminal, and the
/// origin is taken from the caller rather than derived from the command.
pub struct TerminalPlaneExecutor {
    host: Arc<PtyHost>,
    app: AppHandle,
}

impl TerminalPlaneExecutor {
    /// The host is the same `Arc<PtyHost>` the Shell view uses, so a session
    /// spawned here is the session the user sees. Holding it directly keeps the
    /// tool-dispatch path off the managed-state lock while a command runs.
    pub fn new(host: Arc<PtyHost>, app: AppHandle) -> Self {
        Self { host, app }
    }
}

impl agentcowork_core::tools::TerminalExecutor for TerminalPlaneExecutor {
    fn run(
        &self,
        command: &str,
        label: &str,
        origin: TerminalOrigin,
    ) -> Result<agentcowork_core::tools::TerminalRun, String> {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Err("script.run: command must not be empty".into());
        }
        let cfg = Config::load().map_err(|e| e.to_string())?;
        let resolved = resolve_automation_profile(&cfg)?;
        let profile_id = resolved.profile_name.clone();

        let (pty_id, output) = self
            .host
            .spawn_with(
                &resolved,
                None,
                24,
                80,
                SpawnOpts {
                    origin,
                    integration: cfg.terminal.shell_integration,
                },
            )
            .map_err(|e| e.to_string())?;
        let tracker = self
            .host
            .tracker(&pty_id)
            .ok_or_else(|| "script.run: session vanished immediately after spawn".to_string())?;
        let expected = command_line_count(trimmed).max(1);
        for line in trimmed.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            self.host
                .write(&pty_id, format!("{line}\r").as_bytes())
                .map_err(|e| format!("script.run: write failed: {e}"))?;
        }
        // Wiring the reader is what produces the records; do it once the
        // command is on the wire so no frame is emitted before the UI is
        // listening (the ring covers replay for a view that attaches later).
        let state = self.app.state::<AppState>();
        finish_spawn(
            &self.app,
            &state,
            pty_id.clone(),
            output,
            SpawnProvenance {
                origin,
                label: Some(label),
                // The `script.run` ticket was already minted, consumed and
                // audited by the tool executor (`tool/exec` → `tool/commit` →
                // the `tool.exec` Merkle row, which names it). The trait hands
                // the executor no ticket id, so this PTY-lifecycle row does not
                // repeat it — the link lives on the effect row, which is the row
                // that records the effect.
                ticket_id: None,
                subject: serde_json::json!({ "profileId": profile_id, "command": trimmed }),
            },
        )?;

        // Without shell integration no completion record will ever arrive, so
        // there is nothing to block on — settle, then read the ring.
        let integrated = state
            .terminal
            .sessions()
            .into_iter()
            .find(|s| s.pty_id == pty_id)
            .map(|s| s.integration.is_some())
            .unwrap_or(false);
        let record = if integrated {
            await_command_records(&tracker, expected, Instant::now() + AGENT_RUN_TIMEOUT)
        } else {
            std::thread::sleep(AGENT_RUN_SETTLE);
            None
        };

        let (command_line, cwd, exit_code, output, trusted) = match record {
            Some(rec) => (rec.command, rec.cwd, rec.exit_code, rec.output, rec.trusted),
            None => {
                let cwd = tracker
                    .lock()
                    .map(|t| t.cwd().to_string())
                    .unwrap_or_default();
                // The ring is the session's own bytes: honest raw output for a
                // session the shell cannot describe structurally.
                let output = state
                    .terminal
                    .replay(&pty_id, None)
                    .map(|r| String::from_utf8_lossy(&r.bytes).to_string())
                    .unwrap_or_default();
                (trimmed.to_string(), cwd, None, output, false)
            }
        };
        Ok(agentcowork_core::tools::TerminalRun {
            pty_id,
            profile_id,
            command: command_line,
            cwd,
            exit_code,
            output,
            trusted,
        })
    }
}

/// Write raw input (keystrokes / paste) to a live PTY.
#[tauri::command]
pub fn terminal_write(
    state: State<'_, AppState>,
    pty_id: String,
    data: String,
) -> Result<bool, String> {
    state
        .terminal
        .write(&pty_id, data.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// P54.3 — resize the PTY pair (SIGWINCH / ConPTY resize).
#[tauri::command]
pub fn terminal_resize(
    state: State<'_, AppState>,
    pty_id: String,
    rows: u16,
    cols: u16,
) -> Result<bool, String> {
    state
        .terminal
        .resize(&pty_id, rows.clamp(1, MAX_DIM), cols.clamp(1, MAX_DIM))
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Kill + reap a PTY session (tab close / explicit stop).
#[tauri::command]
pub fn terminal_kill(state: State<'_, AppState>, pty_id: String) -> Result<bool, String> {
    let killed = state.terminal.kill(&pty_id).map_err(|e| e.to_string())?;
    if killed {
        crate::control::record_mutation(
            &state,
            crate::control::AuthKind::HumanGesture,
            "terminal.kill",
            serde_json::json!({ "ptyId": pty_id }),
        );
    }
    Ok(killed)
}

/// Live PTY sessions for the tab strip. Carries provenance (`origin`/`label`),
/// the integration quality, and the shell's live cwd.
#[tauri::command]
pub fn terminal_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    // P54.5 — served from the shared plane read model. The agent-facing
    // `terminal/status` RPC arm serializes the *same* struct, so a session row
    // the model is told about cannot describe a different shell than the tab
    // strip draws.
    serde_json::to_value(state.terminal.plane_status()).map_err(|e| e.to_string())
}

/// P67 — the shell's structured command history for one PTY. This is the data
/// behind exit-code decorations, the recent-command picker, and the chat
/// context (`#terminalLastCommand`). Untrusted command lines are reported with
/// `trusted: false` and the UI must not present them as fact.
#[tauri::command]
pub fn terminal_commands(
    state: State<'_, AppState>,
    pty_id: String,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    // P54.5 — shared read model (same rows the relay's `terminal/commands`
    // serves). `no such pty` is a caller bug and stays an error.
    let commands = state
        .terminal
        .commands(&pty_id, limit.unwrap_or(50))
        .map_err(|e| format!("terminal_commands: {e}"))?;
    let cwd = state
        .terminal
        .session(&pty_id)
        .map(|s| s.cwd)
        .unwrap_or_default();
    serde_json::to_value(serde_json::json!({
        "ptyId": pty_id,
        "cwd": cwd,
        "count": commands.len(),
        "commands": commands,
    }))
    .map_err(|e| e.to_string())
}

/// P67 — the last-command context block, exactly as it should be shown to a
/// model: command, cwd, exit code, and output. `null` when the shell has not
/// reported a trusted command yet (never a fabricated empty block).
#[tauri::command]
pub fn terminal_last_command_context(
    state: State<'_, AppState>,
    pty_id: String,
    max_chars: Option<usize>,
) -> Result<Option<String>, String> {
    state
        .terminal
        .last_command(&pty_id, max_chars.unwrap_or(6000))
        .map_err(|e| format!("terminal_last_command_context: {e}"))
}

/// P67 — recent command history as a compact block (terminal-history context,
/// e.g. "why did my last three commands fail?").
#[tauri::command]
pub fn terminal_history_context(
    state: State<'_, AppState>,
    pty_id: String,
    limit: Option<usize>,
    max_chars: Option<usize>,
) -> Result<Option<String>, String> {
    state
        .terminal
        .history(&pty_id, limit.unwrap_or(10), max_chars.unwrap_or(4000))
        .map_err(|e| format!("terminal_history_context: {e}"))
}
