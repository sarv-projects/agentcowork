use serde_json::Value;
use tauri::State;

use crate::AppState;

fn gateway(
    state: &AppState,
) -> Result<std::sync::Arc<std::sync::Mutex<agentcowork_core::WorkGateway>>, String> {
    crate::ensure_sidecar(state);
    let relay = state.chat_relay.lock().map_err(|e| e.to_string())?;
    relay
        .as_ref()
        .map(|r| r.work_gateway())
        .ok_or_else(|| "sidecar not connected — work gateway unavailable".to_string())
}

#[tauri::command]
pub fn work_list(state: State<'_, AppState>) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let gateway = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(gateway.list_work()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn work_snapshot(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let gateway = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(gateway.snapshot(&work_id)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn work_events(
    state: State<'_, AppState>,
    work_id: String,
    from_sequence: Option<u64>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let gateway = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(gateway.replay_from(&work_id, from_sequence.unwrap_or(0)))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn work_presence(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let gateway = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(gateway.presence(&work_id)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn work_reviews(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let gateway = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(gateway.reviews(&work_id)).map_err(|e| e.to_string())
}

// =============================================================================
// P49.10–12 — session-runtime lifecycle commands (PtySession / WorktreeBinding
// / AgentSession). The gateway owns the durable descriptors + event fan-out;
// the OS-level PTY spawn / git worktree ops are the shell's existing engines
// (shell_cmds / git_cmds) — these commands record + drive the runtime state
// machine and emit the WorkEvent stream every client replays.
// =============================================================================

/// P49.10 — register a spawned PTY (the caller supplies the OS pid).
#[tauri::command]
pub fn work_pty_spawn(
    state: State<'_, AppState>,
    work_id: String,
    pty_id: String,
    process_id: Option<u32>,
    rows: u16,
    cols: u16,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.spawn_pty(&work_id, &pty_id, process_id, rows, cols)?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.10 — resize a PTY.
#[tauri::command]
pub fn work_pty_resize(
    state: State<'_, AppState>,
    work_id: String,
    pty_id: String,
    rows: u16,
    cols: u16,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.resize_pty(&work_id, &pty_id, rows, cols)?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.10 — signal a PTY (SIGINT/SIGTERM/…). The shell delivers the OS signal.
#[tauri::command]
pub fn work_pty_signal(
    state: State<'_, AppState>,
    work_id: String,
    pty_id: String,
    signal: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.signal_pty(&work_id, &pty_id, &signal)?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.10 — close a PTY with an exit code.
#[tauri::command]
pub fn work_pty_close(
    state: State<'_, AppState>,
    work_id: String,
    pty_id: String,
    code: Option<i32>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.close_pty(&work_id, &pty_id, code)?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.10 — snapshot the retained terminal buffer for a re-attaching client.
#[tauri::command]
pub fn work_pty_snapshot(state: State<'_, AppState>, pty_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.snapshot_terminal(&pty_id)).map_err(|e| e.to_string())
}

/// P49.11 — create a worktree binding for a run.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn work_worktree_create(
    state: State<'_, AppState>,
    work_id: String,
    run_id: String,
    worktree_id: String,
    repo_root: String,
    worktree_root: String,
    base_revision: String,
    branch: String,
    isolation_mode: Option<String>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.create_worktree(
        &work_id,
        &run_id,
        &worktree_id,
        &repo_root,
        &worktree_root,
        &base_revision,
        &branch,
        &isolation_mode.unwrap_or_else(|| "worktree".to_string()),
    )?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.11 — attach a worktree to a (possibly new) run.
#[tauri::command]
pub fn work_worktree_attach(
    state: State<'_, AppState>,
    work_id: String,
    worktree_id: String,
    run_id: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.attach_worktree(&work_id, &worktree_id, &run_id)?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.11 — merge / revert / destroy a worktree (op = merge|revert|destroy).
#[tauri::command]
pub fn work_worktree_op(
    state: State<'_, AppState>,
    work_id: String,
    worktree_id: String,
    op: String,
    into: Option<String>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = match op.as_str() {
        "merge" => g.merge_worktree(
            &work_id,
            &worktree_id,
            &into.unwrap_or_else(|| "main".into()),
        )?,
        "revert" => g.revert_worktree(&work_id, &worktree_id)?,
        "destroy" => g.destroy_worktree(&work_id, &worktree_id)?,
        other => return Err(format!("unknown worktree op: {other}")),
    };
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.12 — spawn a subagent session (lifetime = ephemeral|persistent).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn work_agent_spawn(
    state: State<'_, AppState>,
    work_id: String,
    run_id: String,
    agent_session_id: String,
    agent_id: String,
    lifetime: String,
    pty_id: Option<String>,
    worktree_id: Option<String>,
) -> Result<Value, String> {
    use agentcowork_core::AgentLifetime;
    let lt = match lifetime.as_str() {
        "persistent" | "persistent_attached_session" => AgentLifetime::PersistentAttachedSession,
        _ => AgentLifetime::EphemeralChild,
    };
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = g.spawn_subagent(
        &work_id,
        &run_id,
        &agent_session_id,
        &agent_id,
        lt,
        pty_id,
        worktree_id,
    )?;
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P49.12 — agent-session op (attach|detach|steer|checkpoint|terminate).
#[tauri::command]
pub fn work_agent_op(
    state: State<'_, AppState>,
    work_id: String,
    agent_session_id: String,
    op: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let ev = match op.as_str() {
        "attach" => g.attach_agent_session(&work_id, &agent_session_id)?,
        "detach" => g.detach_agent_session(&work_id, &agent_session_id)?,
        "steer" => g.steer_agent_session(&work_id, &agent_session_id)?,
        "checkpoint" => g.checkpoint_agent_session(&work_id, &agent_session_id)?,
        "terminate" => g.terminate_agent_session(&work_id, &agent_session_id)?,
        other => return Err(format!("unknown agent-session op: {other}")),
    };
    serde_json::to_value(ev).map_err(|e| e.to_string())
}

/// P71.9d — the delegation tree below a Work: child Works with their parent
/// link, owning Session kind and bound-agent handle. Read-only; children only
/// ever come into existence through `delegate_child_work` (P69.D14).
#[tauri::command]
pub fn work_children(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.children_of(&work_id)).map_err(|e| e.to_string())
}

/// P49.12 — list agent sessions for a work.
#[tauri::command]
pub fn work_agent_sessions(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.agent_sessions_for(&work_id)).map_err(|e| e.to_string())
}

// =============================================================================
// P49.1/.3/.4/.7/.8/.9/.13/.14/.15/.17 — the V1-local Work Gateway wiring.
// Each command below is the live consumer for its queue row: the Gateway owns
// the durable state, Tauri is only the transport. Remote clients / multi-node
// failover stay post-v1 (P49.19/.20).
// =============================================================================

/// P49.1 — create a Work (canonical address, durable).
#[tauri::command]
pub fn work_create(
    state: State<'_, AppState>,
    work_id: String,
    objective: String,
    project_id: Option<String>,
    session_id: Option<String>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let address = g.create_work(work_id, project_id, session_id, objective);
    serde_json::to_value(address).map_err(|e| e.to_string())
}

/// P49.1 — read a single Work address.
#[tauri::command]
pub fn work_get(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.get_work(&work_id)).map_err(|e| e.to_string())
}

/// P49.1 — archive a Work.
#[tauri::command]
pub fn work_archive(state: State<'_, AppState>, work_id: String) -> Result<bool, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    Ok(g.archive_work(&work_id))
}

/// P49.1 — the canonical externally-addressable locator for a Work.
#[tauri::command]
pub fn work_locator(state: State<'_, AppState>, work_id: String) -> Result<String, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    g.get_work(&work_id)
        .map(|a| a.locator())
        .ok_or_else(|| format!("unknown work: {work_id}"))
}

/// P49.3 — list registered execution nodes.
#[tauri::command]
pub fn work_nodes(state: State<'_, AppState>) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.nodes()).map_err(|e| e.to_string())
}

/// P49.3 — pair + verify a node (V1: the desktop itself is node-1).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn work_node_register(
    state: State<'_, AppState>,
    node_id: String,
    owner: String,
    platform: String,
    node_kind: String,
    always_on: bool,
    capabilities: Vec<String>,
    network_policy: String,
    sandbox_class: String,
) -> Result<Value, String> {
    use agentcowork_core::ExecutionNode;
    let node = ExecutionNode {
        node_id,
        owner,
        platform,
        node_kind,
        always_on,
        capabilities,
        sandbox_class,
        network_policy,
        credential_policy: "brokered".into(),
        health: String::new(),
        last_heartbeat_ms: 0,
    };
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let paired = g.pair_node(node)?;
    let verified = g.verify_node(&paired.node_id)?;
    serde_json::to_value(verified).map_err(|e| e.to_string())
}

/// P49.3 — bind a verified node to a Work.
#[tauri::command]
pub fn work_node_bind(
    state: State<'_, AppState>,
    node_id: String,
    work_id: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.bind_node(&node_id, &work_id)?).map_err(|e| e.to_string())
}

/// P49.3 — unbind a node; the Work survives.
#[tauri::command]
pub fn work_node_unbind(state: State<'_, AppState>, node_id: String) -> Result<bool, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    Ok(g.unbind_node(&node_id))
}

/// P49.4 — acquire the run authority (lease + fencing token).
#[tauri::command]
pub fn work_authority_acquire(
    state: State<'_, AppState>,
    run_id: String,
    node_id: String,
    ttl_ms: Option<u64>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let authority = g.acquire_run_authority(&run_id, &node_id, ttl_ms.unwrap_or(60_000))?;
    serde_json::to_value(authority).map_err(|e| e.to_string())
}

/// P49.4 — renew a live lease (a stale fence is refused).
#[tauri::command]
pub fn work_authority_renew(
    state: State<'_, AppState>,
    run_id: String,
    node_id: String,
    token: u64,
    ttl_ms: Option<u64>,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let authority = g.renew_lease(&run_id, &node_id, token, ttl_ms.unwrap_or(60_000))?;
    serde_json::to_value(authority).map_err(|e| e.to_string())
}

/// P49.4 — release the run authority.
#[tauri::command]
pub fn work_authority_release(
    state: State<'_, AppState>,
    run_id: String,
    node_id: String,
    token: u64,
) -> Result<bool, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    Ok(g.release_authority(&run_id, &node_id, token))
}

/// P49.9 — list the clients bound to a Work.
#[tauri::command]
pub fn work_clients(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.clients_for(&work_id)).map_err(|e| e.to_string())
}

/// P49.9 — the V1 client handshake (authenticate → negotiate → bind).
#[tauri::command]
pub fn work_client_connect(
    state: State<'_, AppState>,
    client_id: String,
    client_type: String,
    work_id: String,
    authenticated: bool,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let client = g.connect_client(&client_id, &client_type, &work_id, authenticated)?;
    serde_json::to_value(client).map_err(|e| e.to_string())
}

/// P49.9 — detach a client binding (the binding dies; the Work does not).
#[tauri::command]
pub fn work_client_detach(state: State<'_, AppState>, client_id: String) -> Result<bool, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    Ok(g.detach_client(&client_id))
}

/// P49.7 — the brokered capability set.
#[tauri::command]
pub fn work_capabilities(state: State<'_, AppState>) -> Result<Value, String> {
    use agentcowork_core::CapabilityBroker;
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.broker().list_capabilities()).map_err(|e| e.to_string())
}

/// P49.7 — mint an opaque, run-scoped credential handle (never a secret).
#[tauri::command]
pub fn work_capability_grant(
    state: State<'_, AppState>,
    capability_id: String,
    work_id: String,
    run_id: String,
    consumer: String,
) -> Result<Value, String> {
    use agentcowork_core::{BrokerRequest, CapabilityBroker};
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    let request = BrokerRequest {
        capability_id,
        work_id,
        run_id,
        consumer,
    };
    let grant = g.broker().authorize(&request)?;
    serde_json::to_value(grant).map_err(|e| e.to_string())
}

/// P49.8 — resolve an intent to a ranked capability path.
#[tauri::command]
pub fn work_capability_resolve(
    state: State<'_, AppState>,
    intent: String,
    candidates: Value,
) -> Result<Value, String> {
    use agentcowork_core::CapabilityCandidate;
    let candidates: Vec<CapabilityCandidate> =
        serde_json::from_value(candidates).map_err(|e| e.to_string())?;
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    let resolution = g.resolve_capability(&intent, candidates);
    let mut value = serde_json::to_value(&resolution).map_err(|e| e.to_string())?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "explanation".into(),
            Value::String(resolution.explain_choice()),
        );
    }
    Ok(value)
}

/// P49.13 — resolve a review item (approved | rejected | revision_requested).
#[tauri::command]
pub fn work_review_resolve(
    state: State<'_, AppState>,
    review_id: String,
    state_name: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let event = g.resolve_review_with(&review_id, &state_name)?;
    serde_json::to_value(event).map_err(|e| e.to_string())
}

/// P49.14 — queue a steering instruction (durable policy delta).
#[tauri::command]
pub fn work_steer(
    state: State<'_, AppState>,
    work_id: String,
    client_id: String,
    instruction: String,
    scope: String,
    run_id: Option<String>,
    priority: Option<u8>,
) -> Result<Value, String> {
    use agentcowork_core::SteeringInstruction;
    let steering = SteeringInstruction {
        work_id: work_id.clone(),
        run_id,
        source_client: client_id,
        instruction,
        scope,
        priority: priority.unwrap_or(50),
        created_at_ms: 0,
    };
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let event = g.queue_steering(steering)?;
    serde_json::to_value(event).map_err(|e| e.to_string())
}

/// P49.14 — ask the run to stop at the current step boundary.
#[tauri::command]
pub fn work_steer_interrupt(
    state: State<'_, AppState>,
    work_id: String,
    client_id: String,
    reason: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let event = g.interrupt_current_step(&work_id, &client_id, &reason)?;
    serde_json::to_value(event).map_err(|e| e.to_string())
}

/// P49.15 — freeze the full per-run runtime contract.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn work_manifest_create(
    state: State<'_, AppState>,
    work_id: String,
    chief: String,
    model: String,
    capabilities: Vec<String>,
    network_policy: String,
    filesystem_policy: String,
    autonomy: String,
    node_id: String,
) -> Result<Value, String> {
    use agentcowork_core::{AutonomyLevel, GatewayRuntimeManifest};
    let mut manifest = GatewayRuntimeManifest::new(work_id.clone(), chief, model);
    manifest.capabilities = capabilities;
    manifest.network_policy = network_policy;
    manifest.filesystem_policy = filesystem_policy;
    manifest.node_id = node_id;
    manifest.autonomy = match autonomy.as_str() {
        "sandbox" => AutonomyLevel::Sandbox,
        "auto" => AutonomyLevel::Auto,
        "maximum" => AutonomyLevel::Maximum,
        _ => AutonomyLevel::Ask,
    };
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    let frozen = g.create_runtime_manifest(&work_id, manifest)?;
    serde_json::to_value(frozen).map_err(|e| e.to_string())
}

/// P49.15 — read the frozen manifest for a Work.
#[tauri::command]
pub fn work_manifest_get(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.runtime_manifest(&work_id)).map_err(|e| e.to_string())
}

/// P49.15 — restore a saved manifest intersected with the current trusted
/// policy (a saved manifest is untrusted data; nothing widens).
#[tauri::command]
pub fn work_manifest_restore(
    state: State<'_, AppState>,
    work_id: String,
    trusted_capabilities: Vec<String>,
    trusted_network: String,
    trusted_filesystem: String,
) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    let saved = g
        .runtime_manifest(&work_id)
        .cloned()
        .ok_or_else(|| format!("no runtime manifest for work: {work_id}"))?;
    let restored = g.restore_runtime_manifest(
        &saved,
        &trusted_capabilities,
        &trusted_network,
        &trusted_filesystem,
    );
    serde_json::to_value(restored).map_err(|e| e.to_string())
}

/// P49.17 — register a content-addressed attachment reference.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn work_attachment_add(
    state: State<'_, AppState>,
    attachment_id: String,
    work_id: String,
    path: String,
    media_type: String,
    source: String,
    allowed_consumers: Vec<String>,
    retention: String,
) -> Result<Value, String> {
    use agentcowork_core::AttachmentRef;
    let source_path = std::path::PathBuf::from(&path);
    let attachment = AttachmentRef {
        attachment_id,
        content_hash: String::new(),
        size: std::fs::metadata(&source_path)
            .map(|m| m.len())
            .unwrap_or(0),
        media_type,
        source,
        work_scope: work_id.clone(),
        session_scope: None,
        allowed_consumers,
        retention,
    };
    let gateway = gateway(&state)?;
    let mut g = gateway.lock().map_err(|e| e.to_string())?;
    g.create_attachment(attachment, source_path)?;
    serde_json::to_value(g.attachments_for(&work_id)).map_err(|e| e.to_string())
}

/// P49.17 — list the attachments scoped to a Work.
#[tauri::command]
pub fn work_attachment_list(state: State<'_, AppState>, work_id: String) -> Result<Value, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(g.attachments_for(&work_id)).map_err(|e| e.to_string())
}

/// P49.17 — resolve an attachment for an allowed consumer.
#[tauri::command]
pub fn work_attachment_resolve(
    state: State<'_, AppState>,
    attachment_id: String,
    consumer: String,
) -> Result<String, String> {
    let gateway = gateway(&state)?;
    let g = gateway.lock().map_err(|e| e.to_string())?;
    g.resolve_attachment(&attachment_id, &consumer)
        .map(|p| p.display().to_string())
}
