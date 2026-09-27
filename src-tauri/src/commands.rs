//! Tauri command surface (Fix 1d).
//!
//! The single list of every `#[tauri::command]` registered on the shell's IPC
//! handler. `lib.rs` now only calls `commands::handler()` here instead of
//! owning a 180-line inline list. Each family module (`*_cmds.rs`) contributes
//! `X_cmds::…` paths; the remaining lib.rs-level commands (`chat_stream`,
//! `session_*`, …) are referenced as `crate::…`.
//!
//! NOTE on why this is ONE generate_handler (not many): `Builder::invoke_handler`
//! REPLACES the handler (`self.invoke_handler = Box::new(...)`); calling it more
//! than once would silently keep only the last list. So we compose a single
//! `generate_handler!` here. `tests/registration_sync.rs` guarantees this list
//! stays complete as commands are added.

use crate::acp_cmds;
use crate::agent_backend_cmds;
use crate::agent_cmds;
use crate::artifact_cmds;
use crate::browser_cmds;
use crate::calendar_cmds;
use crate::catalog_cmds;
use crate::cockpit_cmds;
use crate::codeintel_cmds;
use crate::desktop_cmds;
use crate::diagnostics_cmds;
use crate::discovery_cmds;
use crate::doctor_cmds;
use crate::feedback_cmds;
use crate::fs_cmds;
use crate::git_cmds;
use crate::guard_cmds;
use crate::local_cmds;
use crate::lsp_cmds;
use crate::maintenance_cmds;
use crate::mcp_cmds;
use crate::memory_cmds;
use crate::oauth_cmds;
use crate::office_cmds;
use crate::openai_cmds;
use crate::replay_cmds;
use crate::runtime_cmds;
use crate::scheduler_cmds;
use crate::search_cmds;
use crate::settings_cmds;

use crate::skills_cmds;
use crate::storage_cmds;
use crate::sync_cmds;
use crate::tasks_cmds;
use crate::terminal_cmds;
use crate::trajectory_cmds;
use crate::updater_cmds;
use crate::vault_cmds;
use crate::voice_cmds;
use crate::work_cmds;

use crate::xlsx_cmds;

/// Build the full IPC handler for the shell. The `generate_handler!` macro
/// expands to a `move |invoke| { match … }` closure implementing
/// `Fn(Invoke) -> bool`, which is exactly what `Builder::invoke_handler` wants.
pub fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        crate::runtime_status,
        crate::sidecar_probe,
        crate::version,
        catalog_cmds::catalog_sync_plan,
        catalog_cmds::catalog_sync_refresh,
        catalog_cmds::catalog_status,
        catalog_cmds::catalog_refresh,
        catalog_cmds::catalog_set_interval,
        catalog_cmds::catalog_clear,
        catalog_cmds::catalog_providers,
        catalog_cmds::catalog_provider_models,
        catalog_cmds::provider_probe,
        catalog_cmds::provider_profiles_list,
        catalog_cmds::provider_profile_upsert,
        catalog_cmds::provider_profile_remove,
        catalog_cmds::provider_nim_profile,
        crate::core_boot_report,
        crate::scan_text,
        crate::probe_vault,
        crate::vault_key_status,
        crate::vault_setup,
        crate::vault_unlock,
        crate::session_list,
        crate::session_put,
        crate::session_delete,
        calendar_cmds::calendar_list,
        calendar_cmds::calendar_put,
        calendar_cmds::calendar_delete,
        calendar_cmds::calendar_event_list,
        calendar_cmds::calendar_event_put,
        calendar_cmds::calendar_event_delete,
        oauth_cmds::oauth_status,
        oauth_cmds::oauth_accounts,
        oauth_cmds::oauth_start_pkce,
        oauth_cmds::oauth_start_device,
        oauth_cmds::oauth_poll_device,
        oauth_cmds::oauth_revoke,
        local_cmds::local_models,
        local_cmds::local_ensure,
        local_cmds::local_hardware,
        // P71.2c — `chat_stream`, `chat_cancel`, `chat_tool_retry`,
        // `plan_execute` and `plan_respond` are deleted with the built-in
        // engine (ADR-0005 §2). A turn runs on the bound external agent's
        // channel: `acp_launch` → `acp_prompt` → `acp_cancel` (or
        // `acp_shutdown`). The events they fed are gone with them.
        crate::agui_send,
        crate::agui_listen,
        crate::usage_snapshot,
        crate::session_totals,
        // P55.8 — the SearXNG endpoint config + searx.space instance feed.
        search_cmds::search_config,
        search_cmds::search_instances,
        search_cmds::search_instances_apply,
        replay_cmds::replay_sessions,
        replay_cmds::replay_timeline,
        replay_cmds::replay_screenshot,
        replay_cmds::watch_events,
        replay_cmds::agent_stop,
        trajectory_cmds::trajectory_sessions,
        trajectory_cmds::trajectory_snapshot,
        guard_cmds::guard_tickets,
        feedback_cmds::feedback_submit,
        guard_cmds::guard_respond,
        guard_cmds::guard_open_window,
        guard_cmds::guard_receipts,
        guard_cmds::guard_policy,
        guard_cmds::guard_extend_ttl,
        guard_cmds::guard_explain_block,
        guard_cmds::guard_set_policy_rules,
        guard_cmds::guard_autonomy,
        guard_cmds::guard_set_autonomy,
        guard_cmds::guard_estop,
        guard_cmds::guard_activity,
        guard_cmds::guard_permissions_matrix,
        guard_cmds::guard_combos,
        guard_cmds::guard_apply_combo,
        cockpit_cmds::cockpit_snapshot,
        cockpit_cmds::cockpit_activity,
        cockpit_cmds::cockpit_tokens,
        cockpit_cmds::cockpit_quiet,
        cockpit_cmds::agent_undo,
        cockpit_cmds::interrupt_respond,
        cockpit_cmds::cockpit_upsert_agent,
        xlsx_cmds::xlsx_open,
        xlsx_cmds::xlsx_recalc,
        xlsx_cmds::xlsx_edit_request,
        xlsx_cmds::xlsx_edit_commit,
        xlsx_cmds::xlsx_batch_request,
        xlsx_cmds::xlsx_batch_commit,
        xlsx_cmds::xlsx_pivot,
        mcp_cmds::mcp_catalog,
        mcp_cmds::mcp_servers,
        mcp_cmds::mcp_attach_request,
        mcp_cmds::mcp_attach_commit,
        mcp_cmds::mcp_external_tools,
        mcp_cmds::mcp_detach,
        // P51.18: no-restart refresh (prune dead children + relist).
        mcp_cmds::mcp_refresh,
        mcp_cmds::mcp_start,
        mcp_cmds::mcp_stop,
        mcp_cmds::mcp_set_autostart,
        mcp_cmds::store_catalog,
        mcp_cmds::mcp_connect_start,
        mcp_cmds::mcp_remote_status,
        mcp_cmds::mcp_remote_call,
        mcp_cmds::mcp_remote_call_commit,
        mcp_cmds::mcp_remote_tools,
        skills_cmds::skills_catalog,
        skills_cmds::skills_install,
        skills_cmds::skills_learn,
        skills_cmds::skills_uninstall,
        office_cmds::docx_open,
        office_cmds::docx_patch,
        office_cmds::docx_tracks,
        office_cmds::pptx_open,
        office_cmds::pptx_notes,
        office_cmds::pdf_open,
        office_cmds::pdf_bytes,
        office_cmds::pdf_page_op,
        office_cmds::office_open_external,
        vault_cmds::vault_keys_list,
        vault_cmds::vault_key_add,
        vault_cmds::vault_key_remove,
        // P51.1: in-place key rotation.
        vault_cmds::vault_key_rotate,
        acp_cmds::chief_default_get,
        acp_cmds::chief_default_set,
        agent_cmds::agent_registry_list,
        agent_cmds::agent_registry_save,
        agent_cmds::agent_registry_get,
        agent_cmds::agent_registry_remove,
        agent_cmds::agent_registry_duplicate,
        agent_cmds::agent_registry_set_disabled,
        // P69.D1 — the single agent directory (inbuilt + ACP registry + local
        // bundles composed server-side; the UI reads this as a façade).
        agent_cmds::agent_directory_list,
        acp_cmds::acp_agents,
        acp_cmds::acp_launch,
        // P63 — per-agent model-backend configuration (install-tab card).
        agent_backend_cmds::agent_backend_get,
        agent_backend_cmds::agent_backend_providers,
        agent_backend_cmds::agent_backend_set,
        agent_backend_cmds::agent_backend_clear,
        agent_backend_cmds::agent_backend_probe,
        acp_cmds::acp_prompt,
        acp_cmds::acp_session_commands,
        acp_cmds::acp_session_config_options,
        acp_cmds::acp_session_set_config_option,
        acp_cmds::acp_config_options,
        acp_cmds::acp_set_session_config_option,
        // P71.12 — the `session/load` provider-resume seam. v1 refuses it with a
        // typed, plain-language reason (ADR-0007 §3/§4) instead of a fabricated
        // resume; see `acp_cmds::acp_session_load`.
        acp_cmds::acp_session_load,
        acp_cmds::acp_tool_log,
        // P53.6 — Settings → Subagents rows + when-to-use note edits.
        acp_cmds::chief_subagents,
        acp_cmds::chief_subagent_set_policy,
        acp_cmds::chief_subagent_set_note,
        acp_cmds::chief_subagent_set_enabled,
        acp_cmds::chief_subagent_mix,
        acp_cmds::acp_cancel,
        acp_cmds::acp_shutdown,
        acp_cmds::acp_sessions,
        acp_cmds::acp_registry_refresh,
        acp_cmds::acp_registry_status,
        acp_cmds::acp_registry_install_plan,
        acp_cmds::acp_install_status,
        acp_cmds::acp_install_request,
        acp_cmds::acp_install_commit,
        acp_cmds::acp_install_await,
        acp_cmds::acp_install,
        acp_cmds::acp_agent_import,
        acp_cmds::acp_agent_verify,
        acp_cmds::acp_authenticate,
        // Maintenance: audit retention sweep (ledger-growth fault line).
        maintenance_cmds::audit_compact,
        // P6.4 (B7): scheduled tasks.
        scheduler_cmds::scheduler_list,
        scheduler_cmds::scheduler_create,
        scheduler_cmds::scheduler_delete,
        scheduler_cmds::scheduler_enable,
        scheduler_cmds::scheduler_pause,
        scheduler_cmds::scheduler_pause_session,
        scheduler_cmds::scheduler_resume,
        scheduler_cmds::scheduler_run_now,
        scheduler_cmds::scheduler_runs,
        scheduler_cmds::scheduler_duplicate,
        scheduler_cmds::scheduler_export,
        scheduler_cmds::scheduler_battery,
        scheduler_cmds::scheduler_fire_event,
        scheduler_cmds::scheduler_fire_webhook,
        scheduler_cmds::scheduler_nudges,
        scheduler_cmds::scheduler_nudge,
        // P51.32: notepad, incidents, doctor (runs history is the Event Log's).
        scheduler_cmds::scheduler_notepad_get,
        scheduler_cmds::scheduler_notepad_append,
        scheduler_cmds::scheduler_incidents,
        scheduler_cmds::scheduler_incident_ack,
        scheduler_cmds::scheduler_doctor,
        tasks_cmds::tasks_list,
        tasks_cmds::tasks_show,
        tasks_cmds::tasks_cancel,
        tasks_cmds::tasks_retry,
        tasks_cmds::tasks_enqueue,
        tasks_cmds::tasks_start,
        tasks_cmds::tasks_complete,
        tasks_cmds::tasks_sweep,
        storage_cmds::storage_health,
        storage_cmds::storage_scan,
        storage_cmds::storage_large_files,
        storage_cmds::storage_duplicates,
        storage_cmds::storage_cleanup_proposals,
        storage_cmds::storage_battery,
        // P8.9 sync: encrypted bundle export/import + live TCP transport
        // (direct ip:port — LAN + Tailscale; explicit trigger, default 47615).
        sync_cmds::sync_export_bundle,
        sync_cmds::sync_import_bundle,
        sync_cmds::sync_keypair_generate,
        sync_cmds::sync_public_key,
        sync_cmds::sync_serve_start,
        sync_cmds::sync_serve_stop,
        sync_cmds::sync_serve_status,
        sync_cmds::sync_peer_sync,
        sync_cmds::node_attach,
        sync_cmds::sync_fingerprint,
        // P8.8: auto-updater check + install/relaunch.
        // P70.C2–C4: channel selection, background download, restart.
        updater_cmds::updater_check,
        updater_cmds::updater_install,
        updater_cmds::updater_channel_get,
        updater_cmds::updater_channel_set,
        updater_cmds::updater_download,
        updater_cmds::updater_restart,
        // P11.5.3: real FS / shell / CDP-browser / memory views.
        fs_cmds::fs_home,
        fs_cmds::fs_list_dir,
        fs_cmds::fs_read_file,
        fs_cmds::fs_write_file,
        fs_cmds::fs_write_ticket,
        fs_cmds::fs_write_commit,
        fs_cmds::fs_undo_list,
        fs_cmds::fs_undo_restore,
        fs_cmds::fs_undo_snapshot,
        // H36 (P54/P67) — the ONE terminal plane: profile registry + real PTY
        // host, shared by human tabs, the agent's `script.run`, and durable
        // tasks. `shell_cmds`'s piped `sh -i`/`cmd` path was retired here.
        terminal_cmds::terminal_profiles,
        terminal_cmds::terminal_set_default,
        terminal_cmds::terminal_set_automation,
        terminal_cmds::terminal_confirm_unsafe,
        terminal_cmds::terminal_get_shell_integration,
        terminal_cmds::terminal_set_shell_integration,
        terminal_cmds::terminal_spawn,
        terminal_cmds::terminal_run,
        // P68.8 — replay retained output into a view that reattached.
        terminal_cmds::terminal_replay,
        terminal_cmds::terminal_write,
        terminal_cmds::terminal_resize,
        terminal_cmds::terminal_kill,
        terminal_cmds::terminal_status,
        terminal_cmds::terminal_commands,
        terminal_cmds::terminal_last_command_context,
        terminal_cmds::terminal_history_context,
        browser_cmds::browser_list_installed,
        browser_cmds::browser_get_config,
        browser_cmds::browser_set_config,
        browser_cmds::browser_start,
        browser_cmds::browser_navigate,
        browser_cmds::browser_snapshot,
        browser_cmds::browser_read,
        // P55.7 — tiered read (static → light engine → Chrome) with the tier
        // that actually served it reported back.
        browser_cmds::browser_read_url,
        browser_cmds::browser_click,
        browser_cmds::browser_type,
        browser_cmds::browser_stop,
        browser_cmds::browser_status,
        memory_cmds::memory_request,
        memory_cmds::memory_read,
        // P11.5.3 IDE: git SCM + LSP diagnostics.
        git_cmds::git_status,
        git_cmds::git_log,
        git_cmds::git_diff,
        git_cmds::git_stage_all,
        git_cmds::git_commit,
        git_cmds::git_root,
        git_cmds::git_worktree_add,
        git_cmds::git_worktree_list,
        git_cmds::git_worktree_merge,
        git_cmds::git_worktree_revert,
        lsp_cmds::lsp_diagnostics,
        // P11.5.9: repo-map / file-outline / MODEL_ALIASES / ai! markers.
        codeintel_cmds::repomap_build,
        codeintel_cmds::file_outline,
        codeintel_cmds::model_aliases_resolve,
        codeintel_cmds::ai_markers_scan,
        // P48.3 (E9): desktop computer-use through the effect funnel.
        desktop_cmds::desktop_status,
        desktop_cmds::desktop_attach,
        // P57.8 — Settings → Computer use: policy + allow-list + inventory.
        desktop_cmds::desktop_policy_get,
        desktop_cmds::desktop_apps,
        desktop_cmds::desktop_policy_allow_path,
        desktop_cmds::desktop_policy_remove_path,
        desktop_cmds::desktop_policy_set_interaction,
        desktop_cmds::desktop_windows,
        desktop_cmds::desktop_read,
        desktop_cmds::desktop_see,
        desktop_cmds::desktop_act,
        // P57.4: foreground-escalation read + gesture-approved act.
        desktop_cmds::desktop_escalation,
        desktop_cmds::desktop_act_escalating,
        desktop_cmds::desktop_stop,
        desktop_cmds::cua_dag_get,
        desktop_cmds::cua_dag_edit_remaining,
        work_cmds::work_list,
        work_cmds::work_snapshot,
        work_cmds::work_events,
        work_cmds::work_presence,
        work_cmds::work_reviews,
        // P49.10-12: session-runtime lifecycle (PtySession/WorktreeBinding/AgentSession).
        work_cmds::work_pty_spawn,
        work_cmds::work_pty_resize,
        work_cmds::work_pty_signal,
        work_cmds::work_pty_close,
        work_cmds::work_pty_snapshot,
        work_cmds::work_worktree_create,
        work_cmds::work_worktree_attach,
        work_cmds::work_worktree_op,
        work_cmds::work_agent_spawn,
        work_cmds::work_agent_op,
        work_cmds::work_agent_sessions,
        work_cmds::work_children,
        // P49.1/.3/.4/.7/.8/.9/.13/.14/.15/.17: the V1-local Work Gateway wiring.
        work_cmds::work_create,
        work_cmds::work_get,
        work_cmds::work_archive,
        work_cmds::work_locator,
        work_cmds::work_nodes,
        work_cmds::work_node_register,
        work_cmds::work_node_bind,
        work_cmds::work_node_unbind,
        work_cmds::work_authority_acquire,
        work_cmds::work_authority_renew,
        work_cmds::work_authority_release,
        work_cmds::work_clients,
        work_cmds::work_client_connect,
        work_cmds::work_client_detach,
        work_cmds::work_capabilities,
        work_cmds::work_capability_grant,
        work_cmds::work_capability_resolve,
        work_cmds::work_review_resolve,
        work_cmds::work_steer,
        work_cmds::work_steer_interrupt,
        work_cmds::work_manifest_create,
        work_cmds::work_manifest_get,
        work_cmds::work_manifest_restore,
        work_cmds::work_attachment_add,
        work_cmds::work_attachment_list,
        work_cmds::work_attachment_resolve,
        // P15-H29: local artifact preview server (loopback, path-floored).
        artifact_cmds::artifact_serve,
        artifact_cmds::artifact_stop,
        // P46.2: agentcowork doctor — per-subsystem readiness report.
        doctor_cmds::doctor_report,
        // P70.D4/D7/D8 — diagnostics: sandbox honesty, support bundle,
        // remove-all-data (typed-confirmed in the UI).
        diagnostics_cmds::diagnostics_sandbox_posture,
        diagnostics_cmds::diagnostics_support_bundle,
        diagnostics_cmds::data_remove_all,
        // P9.5: local OpenAI-compatible server (loopback + bearer token).
        openai_cmds::openai_server_start,
        openai_cmds::openai_server_stop,
        openai_cmds::openai_server_status,
        // P44.7/44.8: discovery surface + routing feed.
        discovery_cmds::discovery_inventory,
        discovery_cmds::routing_feed_decide,
        // P51.1: live provider health ping.
        discovery_cmds::provider_health_probe,
        // P50.4.2: local model downloads (HF GGUF/MLX) + registry + serve.
        crate::model_cmds::model_download_start,
        crate::model_cmds::model_downloads,
        crate::model_cmds::model_download_cancel,
        crate::model_cmds::model_registry_list,
        crate::model_cmds::model_registry_remove,
        crate::model_cmds::model_recommend_quant,
        crate::model_cmds::model_serve,
        crate::model_cmds::model_serve_stop,
        crate::model_cmds::model_serve_list,
        runtime_cmds::runtime_inventory_list,
        runtime_cmds::runtime_models,
        runtime_cmds::runtime_start,
        runtime_cmds::runtime_stop,
        // P52.1/P52.2/P52.5: fit estimate, gallery parse, best-variant pick.
        crate::model_cmds::model_estimate_fit,
        crate::model_cmds::model_gallery_parse,
        crate::model_cmds::model_best_pick,
        // P65 — Settings Control Center (ARCH/17 §17.12): the read-models for
        // providers / agents / connections / schedules / extensions, plus the
        // single mutation funnel (`settings_default_model_set`,
        // `settings_schedule_set_enabled`). Same registration surface as every
        // other family — no second handler, no second registry.
        settings_cmds::settings_providers_list,
        settings_cmds::settings_default_model_get,
        settings_cmds::settings_default_model_set,
        settings_cmds::settings_agents_list,
        settings_cmds::settings_agent_get,
        settings_cmds::settings_agent_loadout,
        settings_cmds::settings_connections_list,
        settings_cmds::settings_schedules_list,
        settings_cmds::settings_schedule_get,
        settings_cmds::settings_schedule_set_enabled,
        settings_cmds::settings_extensions_list,
        // P50.4.3 — crate VAD / utterance pipeline (honest NoopStt).
        voice_cmds::voice_status,
        voice_cmds::voice_vad_classify,
        voice_cmds::voice_process_utterance,
    ]
}
