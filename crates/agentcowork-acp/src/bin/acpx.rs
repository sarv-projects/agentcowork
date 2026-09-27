//! P51.19 — `acpx`: the AgentCowork ACP driver CLI (F12 harness-driving shape).
//!
//! Drives the user's installed agent CLIs over stdio ACP (Agent Client
//! Protocol) from the terminal, mirroring the OpenClaw harness shape:
//!
//! ```text
//! acpx doctor                         — agent registry + binary availability gate
//! acpx run <agent> <prompt...>        — one prompt in a fresh or named session
//! acpx session list|new|rm            — named-session registry (`<data_dir>/acpx.json`)
//! acpx queue add <name> <prompt>      — enqueue a prompt on a named session
//! acpx queue run [name]               — drain a session's queue sequentially
//! acpx flow <file.md>                 — run a markdown flow file ("> " lines are prompts)
//! ```
//!
//! Flags: `--cwd DIR` (spawn working dir), `--session NAME` (reuse/queue),
//! `--json` (machine-readable line protocol), `--quiet` (suppress human
//! chatter), `--yes` (auto-allow every permission request — the caller's
//! explicit choice; the default is fail-closed deny).
//!
//! The doctor gate runs before every `run`/`flow`/`queue run`: an agent whose
//! launch plan command is not on PATH is refused, never silently skipped.
//!
//! Security: permission requests are **denied by default**; only `--yes`
//! turns them into auto-allow (documented as such on stderr). The driver
//! never reads model credentials — agents keep their own auth.

use agentcowork_acp::{
    AcpSession, ClientInfo, LaunchPlan, LaunchRegistry, PermissionDecision, ProcessTransport,
    PromptOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Session store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionEntry {
    agent: String,
    cwd: String,
    #[serde(default)]
    history: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionStore {
    sessions: BTreeMap<String, SessionEntry>,
}

impl SessionStore {
    fn load() -> Self {
        let path = store_path();
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<(), String> {
        let path = store_path();
        let dir = path.parent().ok_or("no parent dir")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        fs::write(
            &path,
            serde_json::to_string_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
}

fn store_path() -> PathBuf {
    // DEC-053: the one shared data-home rule (new path first, legacy honored
    // as a fallback). This binary resolves; it never migrates — Core owns
    // the one-time migration at startup.
    agentcowork_types::env_compat::data_home().join("acpx.json")
}

// ---------------------------------------------------------------------------
// Doctor gate
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct DoctorReport {
    agent: String,
    command: String,
    on_path: bool,
    protocol: String,
    ok: bool,
}

/// Is `command` resolvable on PATH? (The same check shells use; npx/uvx
/// themselves are the usual commands here.)
fn on_path(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).exists();
    }
    let path = match env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    env::split_paths(&path).any(|dir| dir.join(command).is_file())
}

fn doctor(registry: &LaunchRegistry, json: bool) -> i32 {
    let mut reports = Vec::new();
    let mut all_ok = true;
    for m in &registry.agents {
        let plan = registry.launch_plan(&m.id);
        let (command, ok) = match plan {
            Some(p) => {
                let present = on_path(&p.command);
                (p.command, present)
            }
            None => (String::new(), false),
        };
        if !ok {
            all_ok = false;
        }
        reports.push(DoctorReport {
            agent: m.id.clone(),
            command,
            on_path: ok,
            protocol: format!("{:?}", m.protocol),
            ok,
        });
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({ "doctor": reports, "allOk": all_ok }))
                .unwrap()
        );
    } else {
        for r in &reports {
            println!(
                "{:width$} {:<24} {}",
                r.agent,
                format!("{:?}", r.protocol),
                if r.ok {
                    format!("OK   {}", r.command)
                } else {
                    format!("MISS {}", r.command)
                },
                width = 1
            );
        }
        println!(
            "{}",
            if all_ok {
                "doctor: ALL OK"
            } else {
                "doctor: agents missing — install before run"
            }
        );
    }
    if all_ok { 0 } else { 1 }
}

/// The doctor gate: refuse to run an agent whose command is missing.
fn doctor_gate(registry: &LaunchRegistry, agent: &str, json: bool) -> Result<LaunchPlan, String> {
    let plan = registry
        .launch_plan(agent)
        .ok_or_else(|| format!("unknown agent `{agent}` — see `acpx doctor`"))?;
    if !on_path(&plan.command) {
        return Err(format!(
            "doctor gate: `{}` is not on PATH — install the agent CLI first (`acpx doctor`)",
            plan.command
        ));
    }
    if json {
        println!(
            "{}",
            serde_json::json!({ "type": "doctor", "agent": agent, "command": plan.command, "ok": true })
        );
    }
    Ok(plan)
}

// ---------------------------------------------------------------------------
// Prompt runner
// ---------------------------------------------------------------------------

struct RunOpts<'a> {
    agent: &'a str,
    cwd: &'a str,
    yes: bool,
    json: bool,
    quiet: bool,
    session_name: Option<&'a str>,
}

fn run_prompt(
    plan: &LaunchPlan,
    opts: &RunOpts,
    text: &str,
    store: &mut SessionStore,
) -> Result<PromptOutcome, String> {
    if !opts.quiet && !opts.json {
        eprintln!("acpx: spawning {} in {}", plan.agent_id, opts.cwd);
    }
    let env: Vec<(&str, &str)> = plan
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    let transport = ProcessTransport::spawn(&plan.command, &args, &env)
        .map_err(|e| format!("spawn failed: {e}"))?;
    let mut session = AcpSession::new(transport);
    session
        .initialize(ClientInfo {
            name: "agentcowork-acpx".into(),
            title: "AgentCowork acpx driver".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
        .map_err(|e| format!("initialize failed: {e}"))?;
    session
        .session_new(opts.cwd, Vec::new())
        .map_err(|e| format!("session/new failed: {e}"))?;

    let outcome = session
        .prompt(text, |req| {
            let tool = req.tool_call.title.clone();
            if opts.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "type": "permission",
                        "tool": tool,
                        "toolCallId": req.tool_call.tool_call_id,
                    })
                );
            }
            if opts.yes {
                eprintln!(
                    "acpx: --yes auto-allow `{}` (explicit caller choice)",
                    tool
                );
                PermissionDecision::allow()
            } else {
                eprintln!(
                    "acpx: DENIED permission request `{}` (fail-closed; re-run with --yes to allow)",
                    tool
                );
                PermissionDecision::deny()
            }
        })
        .map_err(|e| format!("prompt failed: {e}"))?;
    session.shutdown();

    if let Some(name) = opts.session_name {
        let entry = store
            .sessions
            .entry(name.to_string())
            .or_insert_with(|| SessionEntry {
                agent: opts.agent.to_string(),
                cwd: opts.cwd.to_string(),
                history: Vec::new(),
            });
        entry.history.push(text.to_string());
        let _ = store.save();
    }
    Ok(outcome)
}

fn emit_outcome(outcome: &PromptOutcome, json: bool, quiet: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "type": "outcome",
                "stopReason": format!("{:?}", outcome.stop_reason),
                "updates": outcome.updates.len(),
                "permissions": outcome.permissions.len(),
                "denied": outcome
                    .permission_decisions
                    .iter()
                    .filter(|d| matches!(d, PermissionDecision::Deny { .. }))
                    .count(),
            }))
            .unwrap()
        );
        return;
    }
    if quiet {
        return;
    }
    for u in &outcome.updates {
        match u.session_update.as_str() {
            "agent_message_chunk" => {
                let text: String = u
                    .content
                    .iter()
                    .filter(|c| c.r#type == "text")
                    .map(|c| c.text.clone())
                    .collect();
                if !text.is_empty() {
                    print!("{text}");
                }
            }
            "tool_call" | "tool_call_update" => {
                eprintln!("acpx: tool {} ({})", u.title, u.tool_call_id);
            }
            other => {
                eprintln!("acpx: {other}");
            }
        }
    }
    println!();
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

fn usage() -> ! {
    eprintln!(
        "acpx — AgentCowork ACP driver (P51.19)\n\
         \n\
         USAGE:\n\
         \x20 acpx doctor [--json]\n\
         \x20 acpx run <agent> <prompt...> [--cwd DIR] [--session NAME] [--yes] [--json] [--quiet]\n\
         \x20 acpx session list | new <name> --agent X [--cwd DIR] | rm <name>\n\
         \x20 acpx queue add <name> <prompt...>\n\
         \x20 acpx queue run [name] [--yes] [--json] [--quiet]\n\
         \x20 acpx flow <file.md> [--agent X] [--cwd DIR] [--session NAME] [--yes] [--json] [--quiet]\n\
         \n\
         FLAGS:\n\
         \x20 --cwd DIR      spawn the agent with this working directory (default: .)\n\
         \x20 --session NAME reuse a named session (history is persisted to <data_dir>/acpx.json)\n\
         \x20 --yes          auto-allow every permission request (explicit caller choice)\n\
         \x20 --json         machine-readable line protocol\n\
         \x20 --quiet        suppress human chatter\n\
         \n\
         FLOW FILES: lines starting with \"> \" are prompts run in order; `#` is a comment.\n\
         SECURITY: permission requests are DENIED by default. Agents keep their own auth;\n\
         acpx never reads model credentials."
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let cmd = args[0].as_str();
    let rest = &args[1..];

    // Parse common flags (order-independent, no dependency on clap).
    let mut cwd = ".".to_string();
    let mut yes = false;
    let mut json = false;
    let mut quiet = false;
    let mut session_name: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i].as_str();
        match a {
            "--cwd" => {
                i += 1;
                cwd = rest.get(i).cloned().unwrap_or_else(|| ".".into());
            }
            "--yes" => yes = true,
            "--json" => json = true,
            "--quiet" => quiet = true,
            "--session" => {
                i += 1;
                session_name = rest.get(i).cloned();
            }
            "--agent" => {
                i += 1;
                agent = rest.get(i).cloned();
            }
            _ if a.starts_with("--") => {
                eprintln!("acpx: unknown flag `{a}`");
                std::process::exit(2);
            }
            _ => positional.push(a.to_string()),
        }
        i += 1;
    }

    let registry = LaunchRegistry::builtin();
    let exit = match cmd {
        "doctor" => doctor(&registry, json),
        "run" => {
            let Some(agent_id) = agent.clone().or_else(|| positional.first().cloned()) else {
                usage();
            };
            let prompt = positional
                .iter()
                .skip(if agent.is_some() { 0 } else { 1 })
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if prompt.trim().is_empty() {
                usage();
            }
            let plan = match doctor_gate(&registry, &agent_id, json) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("acpx: {e}");
                    std::process::exit(1);
                }
            };
            let mut store = SessionStore::load();
            let opts = RunOpts {
                agent: &agent_id,
                cwd: &cwd,
                yes,
                json,
                quiet,
                session_name: session_name.as_deref(),
            };
            match run_prompt(&plan, &opts, &prompt, &mut store) {
                Ok(outcome) => {
                    emit_outcome(&outcome, json, quiet);
                    0
                }
                Err(e) => {
                    eprintln!("acpx: {e}");
                    1
                }
            }
        }
        "session" => match positional.first().map(String::as_str) {
            Some("list") => {
                let store = SessionStore::load();
                if store.sessions.is_empty() {
                    println!("(no named sessions)");
                    0
                } else {
                    for (name, e) in &store.sessions {
                        println!("{name}\t{}\t{}", e.agent, e.cwd);
                    }
                    0
                }
            }
            Some("new") => {
                let Some(name) = positional.get(1) else {
                    usage()
                };
                let Some(agent_id) = agent else { usage() };
                let mut store = SessionStore::load();
                store.sessions.insert(
                    name.clone(),
                    SessionEntry {
                        agent: agent_id,
                        cwd,
                        history: Vec::new(),
                    },
                );
                match store.save() {
                    Ok(()) => {
                        println!("session {name} created");
                        0
                    }
                    Err(e) => {
                        eprintln!("acpx: {e}");
                        1
                    }
                }
            }
            Some("rm") => {
                let Some(name) = positional.get(1) else {
                    usage()
                };
                let mut store = SessionStore::load();
                if store.sessions.remove(name).is_some() {
                    let _ = store.save();
                    println!("session {name} removed");
                    0
                } else {
                    eprintln!("acpx: no session {name}");
                    1
                }
            }
            _ => usage(),
        },
        "queue" => match positional.first().map(String::as_str) {
            Some("add") => {
                let Some(name) = positional.get(1) else {
                    usage()
                };
                let prompt = positional
                    .iter()
                    .skip(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                if prompt.trim().is_empty() {
                    usage();
                }
                let mut store = SessionStore::load();
                let Some(entry) = store.sessions.get_mut(name) else {
                    eprintln!(
                        "acpx: no session {name} — `acpx session new {name} --agent X` first"
                    );
                    return;
                };
                entry.history.push(prompt);
                match store.save() {
                    Ok(()) => {
                        println!("queued on {name}");
                        0
                    }
                    Err(e) => {
                        eprintln!("acpx: {e}");
                        1
                    }
                }
            }
            Some("run") => {
                let target = positional.get(1).cloned();
                let mut store = SessionStore::load();
                let names: Vec<String> = match target {
                    Some(n) => vec![n],
                    None => store.sessions.keys().cloned().collect(),
                };
                let mut code = 0;
                for name in names {
                    // Extract the session state first so no mutable borrow of
                    // the store spans the prompt runs (scoped block ends the
                    // borrow before the loop below).
                    let (entry_agent, entry_cwd, pending) = {
                        let Some(entry) = store.sessions.get_mut(&name) else {
                            eprintln!("acpx: no session {name}");
                            code = 1;
                            continue;
                        };
                        (
                            entry.agent.clone(),
                            entry.cwd.clone(),
                            std::mem::take(&mut entry.history),
                        )
                    };
                    if pending.is_empty() {
                        if !quiet {
                            println!("session {name}: queue empty");
                        }
                        continue;
                    }
                    let plan = match doctor_gate(&registry, &entry_agent, json) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("acpx: {e}");
                            code = 1;
                            continue;
                        }
                    };
                    for prompt in pending {
                        let opts = RunOpts {
                            agent: &entry_agent,
                            cwd: &entry_cwd,
                            yes,
                            json,
                            quiet,
                            session_name: Some(&name),
                        };
                        match run_prompt(&plan, &opts, &prompt, &mut store) {
                            Ok(outcome) => emit_outcome(&outcome, json, quiet),
                            Err(e) => {
                                eprintln!("acpx: {e}");
                                code = 1;
                            }
                        }
                    }
                    let _ = store.save();
                }
                code
            }
            _ => usage(),
        },
        "flow" => {
            let Some(file) = positional.first() else {
                usage()
            };
            let Some(agent_id) = agent else { usage() };
            let text = match fs::read_to_string(file) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("acpx: cannot read flow file: {e}");
                    std::process::exit(1);
                }
            };
            let prompts: Vec<String> = text
                .lines()
                .filter(|l| l.trim_start().starts_with('>'))
                .map(|l| l.trim_start().trim_start_matches('>').trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            if prompts.is_empty() {
                eprintln!("acpx: no \"> \" prompts in {file}");
                std::process::exit(1);
            }
            let plan = match doctor_gate(&registry, &agent_id, json) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("acpx: {e}");
                    std::process::exit(1);
                }
            };
            let mut store = SessionStore::load();
            let mut code = 0;
            for prompt in &prompts {
                if !quiet && !json {
                    eprintln!("acpx: flow step — {prompt}");
                }
                let opts = RunOpts {
                    agent: &agent_id,
                    cwd: &cwd,
                    yes,
                    json,
                    quiet,
                    session_name: session_name.as_deref(),
                };
                match run_prompt(&plan, &opts, prompt, &mut store) {
                    Ok(outcome) => emit_outcome(&outcome, json, quiet),
                    Err(e) => {
                        eprintln!("acpx: {e}");
                        code = 1;
                        break;
                    }
                }
            }
            code
        }
        _ => usage(),
    };
    std::process::exit(exit);
}
