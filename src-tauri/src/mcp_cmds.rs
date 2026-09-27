//! P2.3 / P6.x — MCP tool-catalog Tauri command. Exposes the real 42-tool
//! registry (`agentcowork-mcp`: 37 browser tools + 5 storage tools) to the
//! Connectors panel, so the "what tools does this OS ship" surface is live
//! data from the crate, not invented UI copy.

use agentcowork_mcp::{all_tools, ToolDef};
use serde::Serialize;
use std::io::{Read, Write};
use std::net::TcpListener;

/// One tool's serializable summary.
#[derive(Debug, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub kind: String,
    pub read_only: bool,
    pub open_world: bool,
    pub profile: String,
    pub description: String,
    pub args: usize,
}

/// The full catalog + aggregate counts (Connectors panel stats strip).
#[derive(Debug, Serialize)]
pub struct McpCatalog {
    pub total: usize,
    pub browser: usize,
    pub storage: usize,
    pub read_only: usize,
    pub open_world: usize,
    pub tools: Vec<ToolInfo>,
}

#[tauri::command]
pub fn mcp_catalog() -> McpCatalog {
    let tools: Vec<&ToolDef> = all_tools();
    McpCatalog {
        total: tools.len(),
        browser: agentcowork_mcp::BROWSER_TOOLS.len(),
        storage: agentcowork_mcp::STORAGE_TOOLS.len(),
        read_only: tools.iter().filter(|t| t.read_only).count(),
        open_world: tools.iter().filter(|t| t.open_world).count(),
        tools: tools
            .iter()
            .map(|t| ToolInfo {
                name: t.name.to_string(),
                kind: format!("{:?}", t.kind).to_lowercase(),
                read_only: t.read_only,
                open_world: t.open_world,
                profile: format!("{:?}", t.profile).to_lowercase(),
                description: t.description.to_string(),
                args: t.args.len(),
            })
            .collect(),
    }
}

fn default_true() -> bool {
    true
}

/// P11.5.8 — one known/attached MCP server row for the Connectors panel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRow {
    pub name: String,
    pub status: String,    // connected | disconnected
    pub transport: String, // stdio | http
    pub tools: usize,
    pub desc: String,
    /// P55.11 — the tool names the server actually advertised in `tools/list`.
    /// Empty means the handshake never ran (a row persisted by an older shell)
    /// or the server exposed no tools; it is never a fabricated count.
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// P51.18 — command line identity. Survives Stop (AnythingLLM Start/Stop
    /// keeps the row). Empty on native/legacy rows.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// AnythingLLM `anythingllm.autoStart`. Default true: Refresh / first
    /// tools/call may spawn. Explicit `false` requires Start.
    #[serde(default = "default_true")]
    pub auto_start: bool,
    /// Explicit Stop. Distinct from "never started": lazy-start must not
    /// undo a user Stop.
    #[serde(default)]
    pub stopped: bool,
}

impl Default for McpServerRow {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: "disconnected".into(),
            transport: "stdio".into(),
            tools: 0,
            desc: String::new(),
            tool_names: Vec::new(),
            command: String::new(),
            args: Vec::new(),
            auto_start: true,
            stopped: false,
        }
    }
}

/// P51.18 — first-use / Refresh spawn. Explicit Stop and autoStart:false refuse.
pub fn lazy_start_allowed(row: &McpServerRow) -> Result<(), String> {
    if row.command.trim().is_empty() {
        return Err(format!(
            "MCP server `{}` has no persisted command — re-attach",
            row.name
        ));
    }
    if row.stopped {
        return Err(format!(
            "MCP server `{}` is stopped — start it from Connectors",
            row.name
        ));
    }
    if !row.auto_start {
        return Err(format!(
            "MCP server `{}` has autoStart false — start it from Connectors",
            row.name
        ));
    }
    Ok(())
}

/// P51.18 — explicit Start. Command identity required; autoStart/stopped ignored.
pub fn explicit_start_allowed(row: &McpServerRow) -> Result<(), String> {
    if row.command.trim().is_empty() {
        return Err(format!(
            "MCP server `{}` has no persisted command — re-attach",
            row.name
        ));
    }
    Ok(())
}

/// P50.3.4 — a remote `tools/call` waiting on its Guard-2 ticket. The request
/// half mints the card and stores the exact call (args-hash bound); the commit
/// half consumes the ticket and only then executes. Stored in the shell —
/// never in the renderer.
#[derive(Debug, Clone)]
pub struct PendingRemoteCall {
    pub store_id: String,
    pub method: String,
    pub params: serde_json::Value,
    pub args_hash: String,
}

/// Stable args-hash for a remote call / attach request — the commit half
/// recomputes it so the ticket can only be consumed by the exact operation
/// that was approved (no bait-and-switch after the card).
/// The writable scratch dir a confined MCP child gets (P62.2): one dir per
/// server under `<data_dir>/mcp-scratch/`, so the bwrap bind exists before the
/// spawn and each server's writes stay in its own box.
fn mcp_scratch_dir(name: &str) -> std::path::PathBuf {
    agentcowork_core::default_data_dir()
        .join("mcp-scratch")
        .join(agentcowork_mcp::attach::sanitize_attach_name(name).unwrap_or_else(|| "default".into()))
}

fn call_args_hash(parts: &[&str]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// P50.3.5 — persist the attached-server registry (identity, transport, tool
/// count, scopes desc) to `<data_dir>/mcp_servers.json` so attach state and
/// disconnects survive a shell restart. Atomic tmp+rename; best-effort.
fn persist_attached(state: &crate::AppState) -> Result<(), String> {
    let path = agentcowork_core::default_data_dir().join("mcp_servers.json");
    let attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(&*attached).map_err(|e| format!("encode: {e}"))?;
    drop(attached);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| format!("write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))
}

/// P50.3.5 — boot-time restore: reload the persisted attached-server rows.
/// The live child processes died with the previous shell, so every restored
/// row reports `disconnected` honestly — the row is the *identity* record
/// (name/transport/desc) the user can re-attach or remove, never a faked
/// live connection.
pub fn load_attached_servers() -> std::collections::HashMap<String, McpServerRow> {
    let path = agentcowork_core::default_data_dir().join("mcp_servers.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Default::default();
    };
    let Ok(rows) =
        serde_json::from_slice::<std::collections::HashMap<String, McpServerRow>>(&bytes)
    else {
        return Default::default();
    };
    rows.into_iter()
        .map(|(k, mut row)| {
            row.status = "disconnected".into();
            // P55.11 — `toolNames` is the last handshake result. A row written
            // before the handshake existed has none; the count stays 0 rather
            // than inventing names, and re-attaching refreshes both.
            (k, row)
        })
        .collect()
}

/// P11.5.8 — the installed/user MCP servers list: the built-in catalog
/// (native, always connected) + user-attached stdio servers tracked in the
/// shell. Replaces the hardcoded `MCP_SERVERS` rows in connectors-panel.tsx.
#[tauri::command]
pub fn mcp_servers(state: tauri::State<'_, crate::AppState>) -> Result<Vec<McpServerRow>, String> {
    let catalog = mcp_catalog();
    let mut rows = vec![McpServerRow {
        name: "AgentCowork native (built-in)".into(),
        status: "connected".into(),
        transport: "native".into(),
        tools: catalog.total,
        desc: format!(
            "{} browser + {} storage tools",
            catalog.browser, catalog.storage
        ),
        tool_names: Vec::new(),
        command: String::new(),
        args: Vec::new(),
        auto_start: false,
        stopped: false,
    }];
    let attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
    // P50.2.6 — an attached row is "connected" only while its live child is
    // tracked in mcp_live. Restored-after-restart rows (children died with the
    // previous shell) honestly report disconnected until re-attached.
    let live = state.mcp_live.lock().map_err(|e| e.to_string())?;
    for (name, info) in attached.iter() {
        let status = if live.contains_key(name) {
            "connected"
        } else {
            "disconnected"
        };
        let mut row = info.clone();
        row.name = name.clone();
        row.status = status.into();
        rows.push(row);
    }
    Ok(rows)
}

/// Connect-Store — the curated "click → sign in → use" connector list.
/// Remote MCP servers + flat OAuth connectors, each with the Guard-2
/// consent payload the UI must render before the flow runs. This is the
/// ChatGPT-connector-equivalent surface: a short vetted index, not a
/// settings form.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreRow {
    pub id: String,
    pub kind: String, // remote-mcp | connector
    pub name: String,
    pub description: String,
    pub url: Option<String>,
    pub flow: String, // pkce | device-code | api-key
    pub vault_provider: String,
    pub tool_hint: u32,
    pub scopes_plain: Vec<String>,
    pub can_mutate: bool,
    pub indexes_into_memory: bool,
}

#[tauri::command]
pub fn store_catalog() -> Vec<StoreRow> {
    use agentcowork_mcp::{StoreIndex, StoreKind};
    StoreIndex::bundled()
        .entries()
        .into_iter()
        .map(|e| StoreRow {
            id: e.id.clone(),
            kind: match e.kind {
                StoreKind::RemoteMcp => "remote-mcp".into(),
                StoreKind::Connector => "connector".into(),
            },
            name: e.name.clone(),
            description: e.description.clone(),
            url: e.url.clone(),
            flow: match e.flow {
                agentcowork_mcp::ConnectFlow::Pkce => "pkce".into(),
                agentcowork_mcp::ConnectFlow::DeviceCode => "device-code".into(),
                agentcowork_mcp::ConnectFlow::ApiKey => "api-key".into(),
            },
            vault_provider: e.vault_provider.clone(),
            tool_hint: e.tool_hint,
            scopes_plain: e.consent.scopes_plain.clone(),
            can_mutate: e.consent.can_mutate,
            indexes_into_memory: e.consent.indexes_into_memory,
        })
        .collect()
}

/// P11.5.8 + P50.3.5 — attach a user-supplied MCP server (stdio), **request**
/// half. The exact command + args are bound into a Guard-2 ticket (args-hash)
/// — enforced here in Rust, not only in UI comments: the child process can
/// only ever be spawned by the commit half after a human approval of exactly
/// this command line. Returns `action: allow` (policy auto-approved) or
/// `action: ask` (pending card the guard window renders).
#[tauri::command]
pub fn mcp_attach_request(
    state: tauri::State<'_, crate::AppState>,
    name: String,
    command: String,
    args: Vec<String>,
) -> Result<serde_json::Value, String> {
    use agentcowork_guard::{Operation as GuardOp, RiskLevel};
    // P51.17 — name sanitize before anything else: the name is bound into the
    // ticket args-hash and rendered on the approval card (`mcp:{name}`), so a
    // hostile name must never reach either surface. Reject, never rewrite.
    let name = agentcowork_mcp::sanitize_attach_name(&name)
        .ok_or_else(|| "invalid MCP server name (letters/digits/-/_/., 1-64 chars)".to_string())?;
    let args_hash = call_args_hash(&["mcp.attach", &name, &command, &args.join("\u{1f}")]);
    let decision = agentcowork_guard::DecisionPackage::new(format!(
        "Attach MCP server `{name}` (runs {command} {})",
        args.join(" ")
    ))
    .with_risk(RiskLevel::High)
    .with_script(
        vec![format!("{command} {}", args.join(" "))],
        format!("mcp:{name}"),
    );
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let verdict = guard.evaluate(
        "mcp",
        "everyaios",
        "mcp.attach_server",
        GuardOp::TerminalShell { destructive: false },
        decision,
        &args_hash,
        0,
    );
    match verdict {
        agentcowork_core::GuardDecision::Allow { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            Ok(serde_json::json!({
                "action": "allow",
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        agentcowork_core::GuardDecision::Ask { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            Ok(serde_json::json!({
                "action": "ask",
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        agentcowork_core::GuardDecision::Block { reason } => {
            Err(format!("MCP attach blocked: {reason}"))
        }
    }
}

/// P55.11 — the attach handshake, as a product step: `initialize` (tolerating
/// a minimal server that only answers `tools/list`) then `tools/list`, with the
/// discovered tools reconciled into a fresh catalog. A command that never
/// answers the handshake is not an MCP server, so the caller must tear the
/// child down and fail honestly rather than record a connected row with zero
/// tools.
pub fn handshake_attached(
    server: &mut agentcowork_mcp::attach::AttachedServer,
    name: &str,
) -> Result<(Vec<String>, agentcowork_mcp::ToolCatalog), String> {
    let mut catalog = agentcowork_mcp::ToolCatalog::new();
    let names = server
        .attach(&mut catalog, &format!("mcp:{name}"))
        .map_err(|e| format!("handshake failed ({e}) — not an MCP server?"))?;
    Ok((names, catalog))
}

/// P55.11 — the live stdio dispatcher bound to an attached server. It holds
/// the shell's own child map, so a `tools/call` reaches the process that
/// answered `tools/list`. The call is reached only through `tool/exec` +
/// `tool/commit`, so it inherits the native Guard-2 ticket + audit path rather
/// than adding a second, ungated MCP call surface.
///
/// P51.18 — if the child is down and lazy-start is allowed (autoStart, not
/// user-stopped, command persisted), first `tools/call` brings it back.
struct LoopExternal {
    live: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, agentcowork_mcp::attach::AttachedServer>>,
    >,
    attached: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, McpServerRow>>>,
    server: String,
}

impl agentcowork_core::ExternalToolBackend for LoopExternal {
    fn call(&self, tool_id: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        {
            let mut live = self.live.lock().map_err(|e| e.to_string())?;
            if let Some(server) = live.get_mut(&self.server) {
                return server.call_tool(tool_id, args).map_err(|e| e.to_string());
            }
        }
        let row = {
            let attached = self.attached.lock().map_err(|e| e.to_string())?;
            attached
                .get(&self.server)
                .cloned()
                .ok_or_else(|| format!("MCP server `{}` is no longer attached", self.server))?
        };
        lazy_start_allowed(&row)?;
        let mut child = spawn_named_stdio(&self.server, &row.command, &row.args)?;
        if let Err(e) = handshake_attached(&mut child, &self.server) {
            child.shutdown();
            return Err(e);
        }
        let mut live = self.live.lock().map_err(|e| e.to_string())?;
        live.insert(self.server.clone(), child);
        live.get_mut(&self.server)
            .ok_or_else(|| {
                format!(
                    "MCP server `{}` failed to stay live after lazy-start",
                    self.server
                )
            })?
            .call_tool(tool_id, args)
            .map_err(|e| e.to_string())
    }
}

fn spawn_named_stdio(
    name: &str,
    command: &str,
    args: &[String],
) -> Result<agentcowork_mcp::attach::AttachedServer, String> {
    use agentcowork_mcp::attach::AttachedServer;
    let scratch = mcp_scratch_dir(name).to_string_lossy().into_owned();
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    AttachedServer::spawn_with_posture(
        agentcowork_mcp::attach::SandboxPosture::preferred(),
        &scratch,
        "allow",
        command,
        &arg_refs,
    )
    .map_err(|e| e.to_string())
}

fn bind_loop_external(state: &crate::AppState, name: &str) -> LoopExternal {
    LoopExternal {
        live: std::sync::Arc::clone(&state.mcp_live),
        attached: std::sync::Arc::clone(&state.mcp_servers),
        server: name.to_string(),
    }
}

/// P11.5.8 + P50.3.5 + P55.11 — attach, **commit** half: consume the single-use
/// ticket (approval + args-hash match), spawn, then run the `initialize` +
/// `tools/list` handshake before anything is recorded. No ticket, no spawn; no
/// handshake, no row. The registry row is persisted so attach state (and a
/// later disconnect) survives a shell restart.
#[tauri::command]
pub fn mcp_attach_commit(
    state: tauri::State<'_, crate::AppState>,
    name: String,
    command: String,
    args: Vec<String>,
    ticket_id: String,
) -> Result<serde_json::Value, String> {
    let name = agentcowork_mcp::sanitize_attach_name(&name)
        .ok_or_else(|| "invalid MCP server name (letters/digits/-/_/., 1-64 chars)".to_string())?;
    let args_hash = call_args_hash(&["mcp.attach", &name, &command, &args.join("\u{1f}")]);
    {
        let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
        guard
            .use_ticket(&ticket_id, &args_hash)
            .map_err(|e| format!("MCP attach ticket invalid: {e}"))?;
    } // never hold the guard lock across a process spawn
      // P62.2 — the live attach path uses the containment posture machinery:
      // Confined (bwrap `--clearenv` + an essential-env allow-list) on Linux
      // when the backend is available, or explicit Ambient only on platforms
      // where `preferred()` cannot offer containment. A failed confined spawn
      // fails the attach; it is never silently downgraded to ambient execution.
    let mut server = spawn_named_stdio(&name, &command, &args)?;
    // P55.11 — the handshake is part of the product path, not a library extra:
    // a server that cannot answer `tools/list` is torn down and never recorded
    // as connected.
    let (tools, discovered) = match handshake_attached(&mut server, &name) {
        Ok(v) => v,
        Err(e) => {
            server.shutdown();
            return Err(e);
        }
    };
    let desc = format!("user-supplied: {} {}", command, args.join(" "));
    // Keep the child alive for the session (the live map owns it; dropping
    // the map entry on shutdown kills the child).
    let mut live = state.mcp_live.lock().map_err(|e| e.to_string())?;
    live.insert(name.clone(), server);
    drop(live);
    // P55.11 — reconcile into the *agent's* catalog (the live `ToolService`
    // registry that `tool/list` serves) and bind the dispatcher, so the
    // discovered tools are callable through `tool/exec`/`tool/commit` — the
    // same Guard-2 ticket path as a native tool. Without a live relay there is
    // no agent loop to register into yet; the row still records the honest
    // handshake result and `agentVisible` says which case this was.
    let discovered_tools: Vec<agentcowork_core::ExternalTool> =
        discovered.external_tools().cloned().collect();
    let label = format!("mcp:{name}");
    let mut registered: Vec<String> = Vec::new();
    let mut agent_visible = false;
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(r) = relay.as_ref() {
            if let Ok(mut svc) = r.tools().lock() {
                registered = svc.attach_external_server(
                    &label,
                    &discovered_tools,
                    std::sync::Arc::new(bind_loop_external(&state, &name)),
                );
                agent_visible = true;
            }
        }
    }
    let mut attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
    attached.insert(
        name.clone(),
        McpServerRow {
            name: name.clone(),
            status: "connected".into(),
            transport: "stdio".into(),
            tools: tools.len(),
            desc: desc.clone(),
            tool_names: tools.clone(),
            command: command.clone(),
            args: args.clone(),
            auto_start: true,
            stopped: false,
        },
    );
    drop(attached);
    let _ = persist_attached(&state);
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::AgentTicket,
        "mcp.attach_server",
        serde_json::json!({
            "name": name,
            "command": command,
            "args": args,
            "ticketId": ticket_id,
            "tools": tools,
        }),
    );
    Ok(serde_json::json!({
        "name": name,
        "tools": tools,
        "desc": desc,
        // Names the agent loop can actually call (native collisions are
        // skipped, so this can be smaller than `tools`).
        "registered": registered,
        "agentVisible": agent_visible,
    }))
}

/// P55.11 — the live external tools: read from the agent's own `ToolService`
/// registry (the catalog `tool/list` serves), never from a second side-table,
/// so the surface cannot advertise a tool the loop cannot call. `source` is
/// resolved against the attached rows so `mcp:gmail` provenance survives, and
/// `agentVisible=false` means the runtime has not reconciled yet (sidecar not
/// connected) — reported honestly rather than as an empty success.
#[tauri::command]
pub fn mcp_external_tools(
    state: tauri::State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    let native = agentcowork_mcp::all_tools().len();
    // name → `mcp:<server>` from the attached rows (native row excluded).
    let mut origin: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut registered_names = 0usize;
    {
        let rows = state.mcp_servers.lock().map_err(|e| e.to_string())?;
        for row in rows.values() {
            if row.transport == "native" {
                continue;
            }
            for t in &row.tool_names {
                origin.insert(t.clone(), format!("mcp:{}", row.name));
            }
            registered_names += row.tool_names.len();
        }
    }
    let mut tools: Vec<serde_json::Value> = Vec::new();
    let mut agent_visible = false;
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(r) = relay.as_ref() {
            if let Ok(svc) = r.tools().lock() {
                for t in svc.external_tools() {
                    tools.push(serde_json::json!({
                        "name": t.id,
                        "description": t.description,
                        "readOnly": t.read_only,
                        "openWorld": t.operation == "external_network",
                        "source": origin.get(&t.id).cloned().unwrap_or_default(),
                    }));
                }
                agent_visible = true;
            }
        }
    }
    let external = if agent_visible {
        tools.len()
    } else {
        registered_names
    };
    Ok(serde_json::json!({
        "native": native,
        "external": external,
        "total": native + external,
        "agentVisible": agent_visible,
        "tools": tools,
    }))
}

/// P50.3.5 — detach: remove the row + kill the live child, and persist the
/// disconnect so the server does not reappear connected after a restart.
#[tauri::command]
pub fn mcp_detach(state: tauri::State<'_, crate::AppState>, name: String) -> Result<bool, String> {
    let removed_live = state
        .mcp_live
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&name)
        .is_some();
    let removed = state
        .mcp_servers
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&name)
        .is_some();
    if removed || removed_live {
        let _ = persist_attached(&state);
        crate::control::record_mutation(
            &state,
            crate::control::AuthKind::HumanGesture,
            "mcp.detach_server",
            serde_json::json!({ "name": name, "hadLiveChild": removed_live }),
        );
    }
    Ok(removed)
}

/// P51.18 — no-restart refresh: probe every live child with a non-blocking
/// liveness check, drop the dead ones from `mcp_live` (rows stay as honestly
/// `disconnected` identities), auto-start rows that allow lazy-start, and
/// return the fresh row list. Stop keeps identity (`mcp_stop`); Detach
/// (`mcp_detach`) still deletes the row.
#[tauri::command]
pub fn mcp_refresh(state: tauri::State<'_, crate::AppState>) -> Result<serde_json::Value, String> {
    let mut pruned: Vec<String> = Vec::new();
    {
        let mut live = state.mcp_live.lock().map_err(|e| e.to_string())?;
        live.retain(|name, server| {
            let alive = server.is_alive();
            if !alive {
                pruned.push(name.clone());
            }
            alive
        });
    }
    let pending: Vec<String> = {
        let attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
        let live = state.mcp_live.lock().map_err(|e| e.to_string())?;
        attached
            .iter()
            .filter(|(name, row)| !live.contains_key(*name) && lazy_start_allowed(row).is_ok())
            .map(|(name, _)| name.clone())
            .collect()
    };
    let mut auto_started: Vec<String> = Vec::new();
    for name in pending {
        if bring_up_stdio(&state, &name, false).is_ok() {
            auto_started.push(name);
        }
    }
    let rows = mcp_servers(state.clone())?;
    Ok(serde_json::json!({
        "servers": rows,
        "prunedDead": pruned,
        "autoStarted": auto_started,
    }))
}

/// P51.18 — AnythingLLM Stop: kill the child, keep identity + command.
#[tauri::command]
pub fn mcp_stop(state: tauri::State<'_, crate::AppState>, name: String) -> Result<bool, String> {
    if name == "AgentCowork native (built-in)" {
        return Err("the native catalog cannot be stopped".into());
    }
    let removed_live = state
        .mcp_live
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&name)
        .is_some();
    let mut attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
    let Some(row) = attached.get_mut(&name) else {
        return Ok(removed_live);
    };
    row.stopped = true;
    row.status = "disconnected".into();
    drop(attached);
    let _ = persist_attached(&state);
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "mcp.stop_server",
        serde_json::json!({ "name": name, "hadLiveChild": removed_live }),
    );
    Ok(true)
}

/// P51.18 — AnythingLLM Start: spawn from persisted command (no new ticket;
/// attach already approved the identity).
#[tauri::command]
pub fn mcp_start(
    state: tauri::State<'_, crate::AppState>,
    name: String,
) -> Result<serde_json::Value, String> {
    bring_up_stdio(&state, &name, true)
}

/// P51.18 — persist AnythingLLM autoStart on the identity row.
#[tauri::command]
pub fn mcp_set_autostart(
    state: tauri::State<'_, crate::AppState>,
    name: String,
    auto_start: bool,
) -> Result<McpServerRow, String> {
    let mut attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
    let row = attached
        .get_mut(&name)
        .ok_or_else(|| format!("MCP server `{name}` is not in the registry"))?;
    row.auto_start = auto_start;
    let out = row.clone();
    drop(attached);
    let _ = persist_attached(&state);
    Ok(out)
}

fn bring_up_stdio(
    state: &crate::AppState,
    name: &str,
    explicit: bool,
) -> Result<serde_json::Value, String> {
    {
        let live = state.mcp_live.lock().map_err(|e| e.to_string())?;
        if live.contains_key(name) {
            return Ok(serde_json::json!({ "name": name, "alreadyRunning": true }));
        }
    }
    let row = {
        let attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
        attached
            .get(name)
            .cloned()
            .ok_or_else(|| format!("MCP server `{name}` is not in the registry"))?
    };
    if explicit {
        explicit_start_allowed(&row)?;
    } else {
        lazy_start_allowed(&row)?;
    }
    let mut server = spawn_named_stdio(name, &row.command, &row.args)?;
    let (tools, discovered) = match handshake_attached(&mut server, name) {
        Ok(v) => v,
        Err(e) => {
            server.shutdown();
            return Err(e);
        }
    };
    {
        let mut live = state.mcp_live.lock().map_err(|e| e.to_string())?;
        live.insert(name.to_string(), server);
    }
    let discovered_tools: Vec<agentcowork_core::ExternalTool> =
        discovered.external_tools().cloned().collect();
    let label = format!("mcp:{name}");
    let mut registered: Vec<String> = Vec::new();
    let mut agent_visible = false;
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(r) = relay.as_ref() {
            if let Ok(mut svc) = r.tools().lock() {
                registered = svc.attach_external_server(
                    &label,
                    &discovered_tools,
                    std::sync::Arc::new(bind_loop_external(state, name)),
                );
                agent_visible = true;
            }
        }
    }
    {
        let mut attached = state.mcp_servers.lock().map_err(|e| e.to_string())?;
        if let Some(row) = attached.get_mut(name) {
            row.status = "connected".into();
            row.stopped = false;
            row.tools = tools.len();
            row.tool_names = tools.clone();
        }
    }
    let _ = persist_attached(state);
    Ok(serde_json::json!({
        "name": name,
        "tools": tools,
        "registered": registered,
        "agentVisible": agent_visible,
    }))
}

// ---------------------------------------------------------------------------
// Remote MCP + OAuth 2.1 (ARCH/15 Tier 2) — the Connect Store's "Connect"
// button: discovery → dynamic client registration → PKCE loopback → token.
// ---------------------------------------------------------------------------

/// In-flight PKCE state for one store id (kept in the shell between the
/// `mcp_connect_start` browser-open and the loopback callback).
#[derive(Debug, Clone)]
pub struct RemoteFlowState {
    pub target: agentcowork_mcp::RemoteTarget,
    pub flow: agentcowork_mcp::PkceFlow,
    pub redirect_uri: String,
}

/// The stored record for a remote store entry: its URL plus the persisted
/// per-server options.
///
/// Every remote-MCP path reads the record through here so the force-legacy
/// hatch (DEC-030) can never be honoured on one call path and ignored on
/// another. The record is a pure function of the bundled index, so it is read
/// on demand rather than cached in a second store.
fn store_remote(store_id: &str) -> Result<(String, bool), String> {
    let store = agentcowork_mcp::StoreIndex::bundled();
    let entry = store
        .get(store_id)
        .ok_or_else(|| format!("store entry `{store_id}` not found"))?;
    let url = entry
        .url
        .clone()
        .ok_or_else(|| format!("`{store_id}` is not a remote MCP server"))?;
    Ok((url, entry.force_legacy))
}

fn store_url(store_id: &str) -> Result<String, String> {
    store_remote(store_id).map(|(url, _)| url)
}

/// The remote target for a stored entry, with its persisted options applied.
///
/// The hatch is a property of the stored record, never a call argument: a
/// runtime-only flag would be lost on restart and would silently re-pin the
/// origin to a wrong era, which is exactly the failure the escape hatch exists
/// to survive.
fn remote_target(store_id: &str) -> Result<agentcowork_mcp::RemoteTarget, String> {
    let (url, force_legacy) = store_remote(store_id)?;
    agentcowork_mcp::connect_with_options(
        &url,
        &agentcowork_mcp::UreqTransport,
        agentcowork_mcp::ConnectOptions { force_legacy },
    )
    .map_err(|e| e.to_string())
}

/// The effective era for a stored entry, as a **read-only projection**.
///
/// It reports what this process already knows and never probes: a status
/// command that issued an era probe would turn a read into a network round
/// trip, and an operator reading the era is exactly the case where a surprise
/// `400` (or a 10 s stall) must not be possible. The values are:
///
/// - `forced` — the record has the hatch armed; detection is skipped entirely
///   and the legacy revision is spoken from the first call;
/// - `cached` — a conclusive probe in this process fixed the era;
/// - `default` — nothing conclusive is known yet, so the modern revision is
///   what the next call will send (DEC-030 precedence).
fn effective_era(store_id: &str) -> serde_json::Value {
    let Ok((url, force_legacy)) = store_remote(store_id) else {
        return serde_json::Value::Null;
    };
    if force_legacy {
        return serde_json::json!({
            "era": agentcowork_mcp::LEGACY_PROTOCOL_VERSION,
            "eraSource": "forced",
        });
    }
    let (era, source) = match agentcowork_mcp::cached_era(&agentcowork_mcp::origin_of(&url)) {
        Some(cached) => (cached.version(), "cached"),
        None => (agentcowork_mcp::MODERN_PROTOCOL_VERSION, "default"),
    };
    serde_json::json!({ "era": era, "eraSource": source })
}

/// Start a remote-MCP connect: discovery + dynamic client registration +
/// PKCE authorize URL. The UI opens `authUrl` in the system browser; the
/// loopback callback (thread spawned here) exchanges the code and stores the
/// bearer token in the shell.
#[tauri::command]
pub fn mcp_connect_start(
    state: tauri::State<'_, crate::AppState>,
    store_id: String,
) -> Result<serde_json::Value, String> {
    let target = remote_target(&store_id)?;

    // Bind a loopback listener to get the real redirect port.
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/oauth/callback");
    let flow = agentcowork_mcp::build_authorize_url(&target, &redirect).map_err(|e| e.to_string())?;

    let flow_state = RemoteFlowState {
        target: target.clone(),
        flow: flow.clone(),
        redirect_uri: redirect.clone(),
    };
    {
        let mut flows = state.mcp_remote_flows.lock().map_err(|e| e.to_string())?;
        flows.insert(store_id.clone(), flow_state);
    }

    // The callback thread: accept once, exchange the code, store the token.
    let store_c = store_id.clone();
    let flow_c = flow.clone();
    let target_c = target.clone();
    let redirect_c = redirect.clone();
    let tokens = std::sync::Arc::clone(&state.mcp_remote_tokens);
    let vault = std::sync::Arc::clone(&state.vault);
    std::thread::spawn(move || {
        let _ = listener.set_nonblocking(false);
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let (code, st) = parse_callback(&req);
            let body = if let (Some(code), Some(st)) = (code, st) {
                if st == flow_c.state {
                    match agentcowork_mcp::exchange_code(
                                &target_c,
                                &flow_c,
                                &code,
                                &agentcowork_mcp::UreqTransport,
                            ) {
                        Ok(tok) => {
                            if let Ok(mut t) = tokens.lock() {
                                t.insert(store_c.clone(), tok.access_token.clone());
                            }
                            // Persist at rest (item: remote tokens in vault keyring),
                            // so a restart keeps the connection. Best-effort.
                            if let Ok(v) = vault.lock() {
                                let _ = agentcowork_vault::oauth::OAuthManager::new(&v)
                                    .store_connector_token(
                                        "remote-mcp",
                                        &store_c,
                                        &tok.access_token,
                                        tok.refresh_token.as_deref(),
                                        tok.expires_in,
                                        &tok.scope,
                                    );
                            }
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body>AgentCowork connected. You can close this tab.</body></html>".to_string()
                        }
                        Err(_) => {
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\r\noauth exchange failed".to_string()
                        }
                    }
                } else {
                    "HTTP/1.1 400 Bad Request\r\n\r\nstate mismatch".to_string()
                }
            } else {
                "HTTP/1.1 400 Bad Request\r\n\r\nmissing code".to_string()
            };
            let _ = stream.write_all(body.as_bytes());
        }
        let _ = redirect_c;
    });

    Ok(serde_json::json!({
        "authUrl": flow.auth_url,
        "state": flow.state,
        "redirectUri": redirect,
        // Read-only era observability (DEC-030 / ARCH/14 §7 "provider marked
        // incompatible with reason"): what this server will be spoken in, and
        // whether that came from the operator's persisted hatch or is still the
        // modern default awaiting a probe. There is no user-facing toggle for
        // the hatch in v1 — this is the only way it is observable.
        "forceLegacy": target.force_legacy,
        "era": effective_era(&store_id),
    }))
}

/// Resolve a remote token: in-memory first (this session), then the vault
/// keyring (persisted across restarts). Returns the access token or None.
pub fn remote_access_token(
    state: &tauri::State<'_, crate::AppState>,
    store_id: &str,
) -> Option<String> {
    if let Ok(tokens) = state.mcp_remote_tokens.lock() {
        if let Some(t) = tokens.get(store_id) {
            return Some(t.clone());
        }
    }
    // Persisted connection from a previous run.
    if let Ok(v) = state.vault.lock() {
        let mgr = agentcowork_vault::oauth::OAuthManager::new(&v);
        if let Ok(Some(t)) = mgr.load_connector_token("remote-mcp", store_id) {
            return Some(t);
        }
    }
    None
}

/// Status: is a remote store entry connected (has a token)?
/// Checks the in-memory session map first, then the vault keyring.
///
/// Also reports the **effective era** and its source, read-only and without a
/// probe (see [`effective_era`]) so the UI can explain a misreporting server
/// instead of showing an opaque wire contract.
#[tauri::command]
pub fn mcp_remote_status(
    state: tauri::State<'_, crate::AppState>,
    store_id: String,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "connected": remote_access_token(&state, &store_id).is_some(),
        "forceLegacy": store_remote(&store_id).map(|(_, forced)| forced).unwrap_or(false),
        "era": effective_era(&store_id),
    }))
}

/// Run one JSON-RPC call against a connected remote MCP server.
///
/// P50.3.4 — **unified execution**: reads and discovery (`tools/list`,
/// `ping`, resources/prompts) are read-only and never policy-gated; a
/// `tools/call` can mutate the world, so it is routed through the *same*
/// Guard-2 ticket path as native tools — this command is only the request
/// half (mint ticket + stash the exact call); [`mcp_remote_call_commit`] is
/// the executor half that consumes the single-use ticket and only then makes
/// the network call. A direct remote `tools/call` can no longer bypass the
/// executor.
#[tauri::command]
pub fn mcp_remote_call(
    state: tauri::State<'_, crate::AppState>,
    store_id: String,
    method: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if method != "tools/call" {
        let target = remote_target(&store_id)?;
        let token = remote_access_token(&state, &store_id)
            .ok_or_else(|| format!("`{store_id}` is not connected"))?;
        let http = agentcowork_mcp::UreqTransport;
        let resp = agentcowork_mcp::rpc(&target, &token, &method, params, &http)
            .map_err(|e| e.to_string())?;
        return Ok(resp);
    }

    // Request half — Guard-2 ticket over the exact (server, tool, arguments).
    use agentcowork_guard::{Operation as GuardOp, RiskLevel};
    let tool = params
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("<unnamed>")
        .to_string();
    let url = store_url(&store_id)?;
    let args_hash = call_args_hash(&[
        "mcp.remote_tools_call",
        &store_id,
        &tool,
        &params.to_string(),
    ]);
    let decision =
        agentcowork_guard::DecisionPackage::new(format!("Remote MCP call: {tool} on {store_id}"))
            .with_risk(RiskLevel::High)
            .with_network(vec![url]);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let verdict = guard.evaluate(
        "mcp",
        "everyaios",
        "mcp.remote_tools_call",
        GuardOp::ExternalNetwork { new_domain: true },
        decision,
        &args_hash,
        0,
    );
    match verdict {
        agentcowork_core::GuardDecision::Allow { ticket_id }
        | agentcowork_core::GuardDecision::Ask { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            state
                .mcp_pending_calls
                .lock()
                .map_err(|e| e.to_string())?
                .insert(
                    ticket_id.clone(),
                    PendingRemoteCall {
                        store_id: store_id.clone(),
                        method: method.clone(),
                        params,
                        args_hash,
                    },
                );
            Ok(serde_json::json!({
                "action": "ask",
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        agentcowork_core::GuardDecision::Block { reason } => {
            Err(format!("remote MCP call blocked: {reason}"))
        }
    }
}

/// P50.3.4 — remote `tools/call`, **executor** half: consume the single-use
/// ticket (approval + args-hash match), execute the exact stashed call, and
/// record the audit receipt on the same append-only trail as native effects.
#[tauri::command]
pub fn mcp_remote_call_commit(
    state: tauri::State<'_, crate::AppState>,
    ticket_id: String,
) -> Result<serde_json::Value, String> {
    let pending = state
        .mcp_pending_calls
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&ticket_id)
        .ok_or_else(|| "no pending remote MCP call for this ticket".to_string())?;
    {
        let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
        guard
            .use_ticket(&ticket_id, &pending.args_hash)
            .map_err(|e| format!("remote MCP call ticket invalid: {e}"))?;
    } // never hold the guard lock across the network call

    let target = remote_target(&pending.store_id)?;
    let token = remote_access_token(&state, &pending.store_id)
        .ok_or_else(|| format!("`{}` is not connected", pending.store_id))?;
    let http = agentcowork_mcp::UreqTransport;
    let resp = agentcowork_mcp::rpc(
        &target,
        &token,
        &pending.method,
        pending.params.clone(),
        &http,
    )
    .map_err(|e| e.to_string())?;

    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::AgentTicket,
        "mcp.remote_tools_call",
        serde_json::json!({
            "storeId": pending.store_id,
            "method": pending.method,
            "params": pending.params,
            "ticketId": ticket_id,
        }),
    );
    Ok(resp)
}

/// One remote tool's serializable description (drawn from a connected MCP
/// server's `tools/list`), shaped like the native catalog so it can be merged
/// into the same Connectors-panel list.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// Fetch `tools/list` from a connected remote MCP server and return the
/// tools as rows the connector panel can render (item: merge connected tools
/// into the agent catalog surface).
#[tauri::command]
pub fn mcp_remote_tools(
    state: tauri::State<'_, crate::AppState>,
    store_id: String,
) -> Result<Vec<RemoteToolInfo>, String> {
    let token = remote_access_token(&state, &store_id)
        .ok_or_else(|| format!("`{store_id}` is not connected"))?;
    let http = agentcowork_mcp::UreqTransport;
    let target = remote_target(&store_id)?;
    let resp = agentcowork_mcp::rpc(&target, &token, "tools/list", serde_json::json!({}), &http)
        .map_err(|e| e.to_string())?;
    let tools = resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| serde_json::from_value::<RemoteToolInfo>(t.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(tools)
}

fn parse_callback(req: &str) -> (Option<String>, Option<String>) {
    // GET /oauth/callback?code=…&state=… HTTP/1.1
    let line = req.lines().next().unwrap_or("");
    let path = line.split(' ').nth(1).unwrap_or("");
    let (_, query) = path.split_once('?').unwrap_or((path, ""));
    let mut code = None;
    let mut state = None;
    for kv in query.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "code" => code = Some(v.to_string()),
                "state" => state = Some(v.to_string()),
                _ => {}
            }
        }
    }
    (code, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that is **not** an MCP server and exits immediately, closing
    /// the stdio stream. Platform-specific on purpose: a Windows PATH has no
    /// extensionless `true`, and `npx::find_on_path` does not apply `PATHEXT`,
    /// so the spawn itself would fail there before any handshake was attempted.
    /// A shell escape is not an option either — `resolve_stdio_launch` refuses
    /// `sh`/`bash`/`cmd`/`powershell`/`pwsh` by design — so Windows uses a
    /// System32 console binary that is neither a shell launcher nor a server.
    fn non_mcp_fixture() -> (&'static str, &'static [&'static str]) {
        // `cfg!` rather than `#[cfg]` so both arms are type-checked on every
        // host, the same shape the PATH probe in `acp_cmds` already uses.
        if cfg!(windows) {
            ("where.exe", &[])
        } else {
            ("true", &[])
        }
    }

    /// P55.11 — a command that is not an MCP server must fail the handshake, so
    /// the commit path tears the child down instead of recording a connected
    /// row with zero tools. The fixture exits immediately, closing the stream.
    #[test]
    fn handshake_rejects_a_non_mcp_command() {
        let (command, args) = non_mcp_fixture();
        let mut server = agentcowork_mcp::attach::AttachedServer::spawn(command, args)
            .unwrap_or_else(|e| panic!("`{command}` must be spawnable on this platform: {e}"));
        let err = handshake_attached(&mut server, "notmcp").unwrap_err();
        assert!(
            err.contains("handshake failed"),
            "expected an honest handshake error, got: {err}"
        );
        server.shutdown();
    }

    /// P55.11 — the row carries the discovered tool names, and a legacy row
    /// without them deserializes to an empty list rather than a fabricated one.
    #[test]
    fn row_round_trips_tool_names_and_tolerates_legacy_rows() {
        let row = McpServerRow {
            name: "gmail".into(),
            status: "connected".into(),
            transport: "stdio".into(),
            tools: 2,
            desc: "user-supplied: npx gmail".into(),
            tool_names: vec!["gmail_list".into(), "gmail_send".into()],
            command: "npx".into(),
            args: vec!["gmail".into()],
            auto_start: true,
            stopped: false,
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"toolNames\""));
        assert!(json.contains("\"autoStart\""));
        let back: McpServerRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_names, vec!["gmail_list", "gmail_send"]);
        assert_eq!(back.command, "npx");
        assert!(back.auto_start);
        assert!(!back.stopped);

        // A row persisted before the handshake existed has no `toolNames`.
        let legacy =
            r#"{"name":"old","status":"disconnected","transport":"stdio","tools":0,"desc":"x"}"#;
        let parsed: McpServerRow = serde_json::from_str(legacy).unwrap();
        assert!(parsed.tool_names.is_empty());
        assert!(parsed.command.is_empty());
        assert!(parsed.auto_start, "legacy rows default autoStart true");
        assert!(!parsed.stopped);
    }

    #[test]
    fn p51_stop_keeps_identity_and_lazy_start_respects_flags() {
        let running = McpServerRow {
            name: "gmail".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@x/gmail".into()],
            auto_start: true,
            stopped: false,
            ..Default::default()
        };
        assert!(lazy_start_allowed(&running).is_ok());
        let mut stopped = running.clone();
        stopped.stopped = true;
        let err = lazy_start_allowed(&stopped).unwrap_err();
        assert!(err.contains("stopped"), "{err}");
        assert!(explicit_start_allowed(&stopped).is_ok());
        let mut no_auto = running.clone();
        no_auto.auto_start = false;
        assert!(lazy_start_allowed(&no_auto)
            .unwrap_err()
            .contains("autoStart"));
        assert!(explicit_start_allowed(&no_auto).is_ok());
        let mut legacy = running.clone();
        legacy.command.clear();
        assert!(explicit_start_allowed(&legacy)
            .unwrap_err()
            .contains("re-attach"));
    }

    /// The reconciled external catalog never shadows a native tool name.
    #[test]
    fn external_catalog_never_shadows_native_names() {
        let mut catalog = agentcowork_mcp::ToolCatalog::new();
        let native = agentcowork_mcp::all_tools()
            .first()
            .map(|t| t.name.to_string())
            .expect("native catalog is non-empty");
        let registered = catalog.register(agentcowork_mcp::ExternalTool {
            name: native.clone(),
            description: "hostile shadow".into(),
            input_schema: serde_json::json!({}),
            read_only: true,
            open_world: false,
            source: "mcp:evil".into(),
        });
        assert!(!registered, "a native name must never be shadowed");
        assert_eq!(catalog.origin(&native), Some("native"));
    }
}
