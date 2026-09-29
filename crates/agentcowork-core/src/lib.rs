//! agentcowork-core — the AgentCowork orchestrator binary.
//!
//! Boots headless (no Tauri) or as the Tauri backend (task P0.2), loads
//! config, initializes the vault and guard, and supervises the TS coordinator
//! sidecar via [`ProcessSupervisor`].
//!
//! Phase P0.1–P0.2 scope: binary boots, `--version` prints, config loads,
//! vault opens/creates, and the same [`boot`] path is exposed to the Tauri
//! command layer.
//!
//! Phase P0.4: ProcessSupervisor spawns the coordinator, monitors exit codes,
//! applies exponential backoff, and trips a circuit breaker on repeated crashes.

use std::path::PathBuf;

// P69.D19/D20 — the kernel boundary ledger: what stays in the execution
// kernel and what is a service renting it. Read before adding a module here.
#[path = "kernel_budget.rs"]
pub mod kernel_budget;

pub mod acpx;
pub mod adapter;
pub mod agui;
pub mod ai_marker;
pub mod automation_runtime;
pub mod blueprint;
pub mod capability_manifest;
pub mod challenge;
pub mod chat;
pub mod combos;
pub mod config;
pub mod connector_approvals;
pub mod connector_hub;
pub mod connectors;
pub mod cua;
pub mod data_home;
pub mod decline;
pub mod diagnose;
pub mod distill;
pub mod dns_cache;
pub mod doctor;
pub mod email;
pub mod eval_service;
pub mod execution;
pub mod export;
pub mod file_undo;
pub mod forge;
pub mod git_commit;
pub mod git_queue;
pub mod governor;
pub mod guard_service;
pub mod hooks;
pub mod hwfit;
pub mod inventory;
pub mod local;
pub mod watcher_glue;
pub use connectors::{
    GraphConnector, ReadFirstPolicy, SCOPE_MANIFEST, SendAction, SendApproval, WorkspaceConnector,
    attach_scopes,
};
pub mod memory_service;
pub mod messaging;
pub mod migrate;
pub mod migration;
pub mod models;
pub mod multirun;
pub mod openai_server;
pub mod orphan;
pub mod pairing;
pub mod plan_service;
// P55.8 — the search-engine configuration owner (local-first, opt-in public).
pub mod provider_ref;
pub mod providers;
pub mod reader;
pub mod remote_attach;
pub mod report;
pub mod research;
pub mod resources;
pub mod routing;
pub mod rss_measure;
pub mod scheduler_service;
pub mod search_config;
pub mod self_audit;
pub mod shell_integration;
pub mod sidecar_link;
// P64.11/P69.G5 — the content-addressed tool-output spool. Kernel-owned: the
// blob write, the compact reference, `retrieve_original`, and the retention
// policy. The renderer card is a view over this, never a second copy.
pub mod spool;
// P70.A8 — durable-store schema stamps + forward-only migration.
pub mod store_schema;
pub mod supervisor;
pub mod sync;
pub mod sync_transport;
pub mod task_ledger;
pub mod telemetry;
pub mod terminal;
pub mod tools;
pub mod tracing;
// P69.G2 — the turn-atomic file snapshot store (`RECOVERY.md` §10). The
// pre-image bytes live here and nowhere else; the Work journal carries only
// digests and store references.
pub mod turn_snapshot;
pub mod vault_key;
pub mod version;
pub mod voice;
pub mod widgets;
pub mod work_gateway;
pub mod workbench;
pub mod worker_pool;
pub mod worktree_cap;
pub mod worktrees;
pub mod wsl;

pub use git_queue::{GitOperationQueue, GitQueueError};
pub use governor::{
    ConcurrencyGovernor, DEFAULT_DENY_TASK_TOOLS, DELEGATE_BLOCKED_TOOLS, FleetTaskKind,
    FleetTaskStatus, GovernorConfig, P64_MAX_CONCURRENT, P64_MAX_DEPTH, P64_MAX_TOTAL,
    SubagentTask, check_subagent_admission, effective_subagent_tools,
};
pub use worktrees::{
    BLACKBOARD_FINDINGS, BLACKBOARD_PLAN, BLACKBOARD_RECEIPTS, MAX_RECEIPT_BYTES, WorktreeError,
    WorktreeLease, WorktreeManager, validate_task_id,
};

pub use adapter::{Stage0Adapter, exact_command_consent, is_install_script};
pub use automation_runtime::{
    AutomationError, AutomationProvenance, CompiledCapabilityRequest, CompiledStep, WorkSpec,
    compile_work, validate_step,
};
pub use blueprint::{AgentBlueprint, BlueprintError, load_all as load_blueprints, load_blueprint};
pub use capability_manifest::{CapabilityManifest, generate_manifest};
pub use challenge::{
    ByoProvider, ByoSolverError, ChallengeHandler, ChallengeKind, ChallengeResolution,
    GroundingChoice, GroundingOption, HumanChallenge, SolverHttp, UreqHttp, VisualGroundingRequest,
    create_task, parse_grounding_choice, poll_task, solve_captcha,
};
pub use chat::{ChatRelay, ChatRelayError, ChatWireEvent};
// P64.11/P69.G5 — the content-addressed tool-output spool. Re-exported so the
// shell names the kernel's threshold and policy instead of repeating them.
pub use agentcowork_mcp::ExternalTool;
pub use config::SubagentPolicy;
pub use config::{Config, ConfigError};
pub use cua::{
    CLOSE_READ_MAX, CUA_REQUIRES_VISION, ComputerUseDag, CuaNode, CuaNodeStatus, CuaSkillDraft,
    DelegationOutcome, DelegationRole, EvidenceKind, FAILED_RECLAIM_AFTER, FivePartBrief,
    HarnessModelCase, IDENTICAL_FAIL_HALT, ManagerReplanReason, ManagerReplanResult,
    MechanicalEvidence, MechanicalVerdict, ModelTier, PRIMARY_SPEND_WARN, PerceptionLayers,
    PrimarySpend, RUNTIME_PLANES, RuntimeBinding, RuntimePlane, SCOUT_ALLOWED_TOOLS, SceneGraph,
    VisionGateError, WorkSurface, append_replan_log, apply_delegation_act, apply_fabric,
    apply_five_part_brief, apply_manager_replan, apply_mechanical_verify, apply_node_stop,
    bind_runtime, classify_harness_model_case, cua_skill_from_verified, cua_skill_to_blueprint,
    delegation_step, fabric_is_perception, fabric_letter, filter_tools_for_role, fuse_perception,
    load_dag, mechanical_verify, node_contract_legal, parse_remaining_nodes, persist_cua_skill,
    persist_dag, pick_combo, refuse_cli_named_subagent, remaining_payload_skips_guard,
    route_work_surface, screen_text_is_untrusted, split_primary_spend, stop_is_blocked,
    verifier_accepts_worker_claim, vision_gate,
};
pub use doctor::{Check, DoctorProbe, DoctorReport, LiveProbe, Status as DoctorStatus, run_doctor};
pub use eval_service::EvalService;
pub use spool::{
    REDACTION, SPOOL_MAX_TOTAL_BYTES, SPOOL_RETENTION_DAYS, Spool, SpoolError, SpoolRef,
    SpoolSlice, SpoolStats, SpoolText, TOOL_OUTPUT_SERIALIZE_CAP, content_hash, estimate_tokens,
    is_valid_hash, retention_policy_note, should_spool,
};
// P71.4 — the usage ledger's observation model, re-exported so the shell can
// name a usage source without depending on `agentcowork-memory` directly.
pub use agentcowork_memory::{UsageObservations, UsageSource};
// P69.D21 — the `Execution` ↔ `Work` back-compat alias is gone (P47.4 renamed
// the struct; the alias was kept "until safe" and nothing outside this crate
// ever referenced it). One term per concept: the durable unit is `Work`.
// `ExecutionKernel`/`ExecutionPhase`/`ExecutionTrigger` are the kernel's own
// machinery names, not a second word for the Work record.
pub use execution::{
    ExecutionKernel, ExecutionPhase, ExecutionTrigger, ForkLineage, P64_MAX_OUTPUT_BYTES,
    P64_MAX_SUBAGENT_DEPTH, PendingApproval, PreflightDecision, ProjectedMessage,
    RepairClassification, RepairPlanItem, RuntimeManifest, ShadowCandidateFile, ShadowCheckOutput,
    StepCheckpointMeta, SubagentProvision, Work, auto_checkpoint_kernel, check_restore_fence,
    commit_workspace_snapshot, decide_shadow_preflight, parse_shadow_candidate,
    plan_subagent_worktree, run_shadow_command, should_restore_without_replay,
    spawn_shadow_command_tracked, truncate_to_50k,
};
pub use export::{
    ExportMessage, MemoryMirror, ObsidianNote, WipeScope, render_json_export,
    render_markdown_export, wipe_facts, wipe_messages,
};
pub use file_undo::restore_file_to_bytes;
pub use guard_service::{
    AskReason, BlockExplanation, GuardDecision, GuardLifecycle, GuardService, PendingGuardCard,
};
pub use hwfit::{
    GpuClass, HardwareProfile, LocalModelCandidate, ModelFit, detect as detect_hardware, recommend,
    score_model,
};
pub use local::{LocalConfig, LocalError, LocalManager, LocalModelInfo};
pub use memory_service::{FactStatus, MemoryService, StoredFact};
pub use messaging::{MessageReminder, ReminderQueue};
pub use openai_server::{
    ChatCompletionRequest, ChatMessage, CompletionBackend, CompletionResult, ModelLister, ModelRow,
    OpenAiServer, StreamPiece, ToolCallFunction, ToolCallOut,
};
pub use plan_service::PlanService;
pub use provider_ref::{
    AuthClass, IngestReport, ProviderEntry, classify_category, ingest_provider_reference,
    parse_provider_reference,
};
pub use providers::{KeyPool, ProviderConfig, ProviderKey, ProvidersError, ProvidersFile};
pub use reader::{
    ReaderChunk, ReaderDocument, ReaderError, ReaderFormat, ReaderHit, ReaderIndex, extract_text,
};
pub use rss_measure::{RssSnapshot, measure_self, measure_tree, snapshot};
pub use scheduler_service::{CronCheck, SchedulerService};
pub use sidecar_link::{Inbound, LinkError, SidecarLink, WriterHandle};
pub use supervisor::{ProcessSupervisor, SupervisorError, SupervisorState};
pub use sync::{
    AeadBox, ChaChaBox, ConflictPolicy, KeyExchange, KeyPair, ResolvedDiff, SYNC_MAGIC,
    SYNC_VERSION, SharedSession, SyncConflict, SyncDiff, SyncEnvelope, SyncError, SyncHello,
    SyncItem, SyncScope, SyncSession, SyncSet, SyncTransport, export_bundle, import_bundle, open,
    reconcile, resolve_conflicts, seal,
};
pub use task_ledger::{
    DEFAULT_LOST_GRACE_MS, DeliveryState, FileStore, InMemoryStore, RETENTION_MS, TaskKind,
    TaskLedger, TaskRecord, TaskStatus, TaskStore,
};
pub use telemetry::{Telemetry, TelemetryEventKind, TelemetryMode, TelemetrySample};
pub use tools::{
    BrowserBackend, EDIT_TOOL_ID, EditShapeSource, EditStrategy, ExternalToolBackend,
    FACADE_ROUTES, FacadeRoute, LexicalShapeSource, P64_MAX_EDIT_BYTES, RegisteredTool,
    TerminalExecutor, TerminalRun, ToolFamily, ToolRegistry, ToolService, apply_edit_ladder,
    apply_exact_once, apply_fuzzy_edit, apply_structured_edit, canonical_args_hash,
    count_occurrences, find_facade, is_facade,
};
pub use vault_key::{
    ResolvedVaultKey, VaultKeyError, VaultKeyOrigin, gate_mode, keyfile_path,
    needs_passphrase_gate, resolve_vault_key, setup_vault_passphrase, unlock_vault_passphrase,
};
pub use widgets::{
    LookupWidget, MathWidget, StockQuote, StockWidget, WeatherSnapshot, WeatherWidget, WidgetCard,
    WidgetError,
};
pub use work_gateway::{
    AgentLifetime, AgentSession, AttachmentRef, AuthSource, AutonomyLevel, BrokerRequest,
    CapabilityBroker, CapabilityCandidate, CapabilityGrant, CapabilityResolution,
    ClientCapabilities, ClientSession, ContextReleasePolicy, DomainEvent, EphemeralCredential,
    ExecutionNode, GatewayCapabilityBroker, OperationalEvent, PresenceEvent, PtySession,
    ReviewItem, RunAuthority, RuntimeEvent, RuntimeManifest as GatewayRuntimeManifest,
    SteeringInstruction, TrustedGestureAttestation, WorkAddress, WorkEvent, WorkEventEnvelope,
    WorkGateway, WorkGatewaySnapshot, WorkPresence, WorkPresenceState, WorktreeBinding,
};
pub use workbench::{
    AcquiredLease, LeaseError, LeaseHandle, ProjectionInput, RebuildReport, ReconcileOutcome,
    TakeoverOutcome, WorkRunLeaseCoordinator,
};
pub use wsl::{
    ExecEnvironment, WSL_LEGACY_PREFIX, WSL_UNC_PREFIX, WslFrame, WslPath, WslRunner,
    detect_environment, detect_environment_from_env, translate_linux_to_windows,
    translate_windows_drive_to_linux, translate_windows_to_linux,
};

/// Default data directory (DEC-053 precedence, via the one shared resolver):
/// `AGENTCOWORK_HOME` → `~/.agentcowork` if it exists → legacy
/// `EVERYAIOS_HOME` → `~/.everyaios` if it exists → else `~/.agentcowork`
/// (the new default). See [`agentcowork_types::env_compat`].
pub fn default_data_dir() -> PathBuf {
    agentcowork_types::env_compat::data_home()
}

/// Headless boot: load config, open/create the vault, report readiness.
///
/// Shared by the binary (`main.rs`) and the Tauri backend (task P0.2) so the
/// two entry points cannot drift.
///
/// Accepts optional `--coordinator-bin <path>` which is consumed by main.rs to
/// start the ProcessSupervisor after boot completes.
pub fn boot(args: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    // DEC-053 — the one-time data-home migration, owned solely by Core at
    // startup. Runs before the config load (the config lives inside the data
    // home): a legacy home with no new home yet is moved once; every other
    // run is a no-op. No lower-level crate migrates anything.
    let home_report = data_home::ensure_data_home();

    let cfg = Config::load()?;

    // `--vault <path>` lets tests point the vault elsewhere.
    let vault_path = args
        .windows(2)
        .find(|w| w[0] == "--vault")
        .map(|w| std::path::PathBuf::from(&w[1]))
        .unwrap_or(cfg.vault_path.clone());

    let resolved = resolve_vault_key(&cfg.data_dir)?;
    let vault = agentcowork_vault::Vault::open(&vault_path, &resolved.key)?;
    let status = vault.status();

    // P70.A8 — stamp the durable stores on the way up. This is where a
    // forward-only policy becomes true for the stores that cannot stamp
    // themselves: a store recorded at a newer schema refuses the boot with a
    // named store instead of being read by a build that cannot understand it.
    let stores = store_schema::ensure_all(&cfg.data_dir)?;

    // P64.11 — the tool-output spool's retention pass runs at boot. Writing is
    // the other trigger (`Spool::write` prunes first), but a machine that only
    // *reads* an install for a week still reclaims, so the spool cannot outlive
    // its policy simply because nobody spooled anything today.
    let spool_stats = spool::Spool::new(&cfg.data_dir).prune_at_boot(now_ms());

    // DEC-053 visibility: a honored legacy spelling must be operator-visible,
    // not silent — the migration outcome and any legacy env fallbacks ride
    // along in the ready line.
    let mut compat_bits = vec![home_report.summary()];
    if let Some(note) = home_report.note.as_deref() {
        compat_bits.push(note.to_string());
    }
    compat_bits.extend(data_home::legacy_env_notes());
    let compat = compat_bits.join("; ");

    Ok(format!(
        "agentcowork-core {} ready — data_dir={} vault={} ({}), retention_days={}, {}, {}, compat=[{}]",
        version::VERSION,
        cfg.data_dir.display(),
        vault_path.display(),
        status,
        cfg.retention_days,
        store_schema::summary(&stores),
        spool_summary(&spool_stats),
        compat,
    ))
}

/// One line for the boot report: how much the tool-output spool is holding, so
/// a spool that is quietly growing is visible at startup rather than inferred
/// later.
pub fn spool_summary(stats: &spool::SpoolStats) -> String {
    format!(
        "spool={} blob(s) {}/{} bytes",
        stats.blob_count, stats.total_bytes, stats.max_total_bytes
    )
}

/// Milliseconds since the Unix epoch, the one clock the spool policy uses.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Extract the `--coordinator-bin <path>` argument from args, if present.
pub fn coordinator_bin_from_args(args: &[String]) -> Option<PathBuf> {
    args.windows(2)
        .find(|w| w[0] == "--coordinator-bin")
        .map(|w| PathBuf::from(&w[1]))
}

/// Convenience: create and return a ProcessSupervisor ready to run.
///
/// Does NOT start the supervisor loop — caller should invoke
/// [`ProcessSupervisor::wait_or_restart`] on a dedicated thread.
pub fn start_supervisor(binary_path: PathBuf) -> Result<ProcessSupervisor, SupervisorError> {
    Ok(ProcessSupervisor::new(binary_path))
}

/// Like [`start_supervisor`], but also returns the link-handoff receiver the
/// shell drains to build a `SidecarLink` on every (re)spawn.
pub fn start_supervisor_with_link(
    binary_path: PathBuf,
) -> (
    ProcessSupervisor,
    std::sync::mpsc::Receiver<(std::process::ChildStdin, std::process::ChildStdout)>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    (ProcessSupervisor::new_with_link(binary_path, Some(tx)), rx)
}

/// Resolve the SQLCipher key (env → keyfile → first-boot generated).
/// Never returns the old hardcoded placeholder.
pub fn default_vault_key() -> String {
    resolve_vault_key(&default_data_dir())
        .map(|r| r.key)
        .unwrap_or_else(|_| {
            // Last resort for in-memory test vaults that have no data dir.
            // Still not the well-known placeholder.
            use rand::RngCore;
            let mut b = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut b);
            b.iter().map(|x| format!("{x:02x}")).collect()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_reports_ready() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-core-boot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::set_var("AGENTCOWORK_HOME", &dir);
            std::env::set_var("AGENTCOWORK_VAULT_KEY", "test-key");
        }

        let vault = dir.join("vault.db");
        let out = boot(&["--vault".into(), vault.to_string_lossy().into()]).expect("boot ok");
        assert!(out.contains("ready"), "expected ready line, got: {out}");
        assert!(out.contains("retention_days=7"));

        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::remove_var("AGENTCOWORK_HOME");
            std::env::remove_var("AGENTCOWORK_VAULT_KEY");
        }
    }

    /// P39.5 — lazy-load enforcement (R6 fix #2): cold boot must not
    /// initialize the heavy subsystems (office / IronCalc, LSP / codeintel,
    /// graph store). The core boot surface is config + vault only; the Tauri
    /// shell additionally holds browser/shell/MCP/ACP handles behind
    /// `Option` (constructed on first command use). This test locks the boot
    /// surface so a future eager-init regression is caught at the contract
    /// level.
    #[test]
    fn boot_does_not_initialize_heavy_subsystems() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-core-lazy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::set_var("AGENTCOWORK_HOME", &dir);
            std::env::set_var("AGENTCOWORK_VAULT_KEY", "test-key");
        }

        let vault = dir.join("vault.db");
        let out = boot(&["--vault".into(), vault.to_string_lossy().into()]).expect("boot ok");
        // The ready line names only the light surface.
        assert!(out.contains("ready"), "expected ready line, got: {out}");
        assert!(
            out.contains("data_dir="),
            "boot report must name the data dir"
        );
        assert!(out.contains("vault="), "boot report must name the vault");
        // The ready line may list durable stores such as `cua_graph`; that is
        // not construction of the optional graph service. Check only the
        // boot-surface fields before the store inventory.
        let boot_surface = out.split(", ").take(3).collect::<Vec<_>>().join(", ");
        for heavy in ["office", "ironcalc", "lsp", "codeintel", "graph", "monaco"] {
            assert!(
                !boot_surface.to_ascii_lowercase().contains(heavy),
                "boot report must not mention {heavy}: {out}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::remove_var("AGENTCOWORK_HOME");
            std::env::remove_var("AGENTCOWORK_VAULT_KEY");
        }
    }
}
