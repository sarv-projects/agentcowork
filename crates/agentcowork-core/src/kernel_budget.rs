//! P69.D19/D20 — the kernel boundary ledger.
//!
//! `agentcowork-core` is the execution kernel: Work/Run/Step/Effect/Event/
//! Receipt, the ExecutionKernel + Work Gateway, the ToolRegistry + ticketed
//! executor, workspace/checkpoint/recovery, the sidecar link, and Guard
//! service wiring. Everything else is a *service renting the kernel*, and the
//! list below is the honest, reviewed inventory of which file is which.
//!
//! The D20 move (services → owning crates) is mechanical but wide: each file
//! below must leave with its tests, its `use` sites re-pointed, and no new
//! dependency direction (a service crate may depend on the kernel contract
//! types in `agentcowork-types`, never on `agentcowork-core` internals).
//!
//! Kernel (stays):
//!   execution.rs, work_gateway.rs, tools.rs (executor + registry),
//!   guard_service.rs, sidecar_link.rs, supervisor.rs, ipc framing glue,
//!   checkpoint/recovery helpers in execution.rs, worktree, boot, config.
//!
//! Services (move under P69.D20, owner in parentheses):
//!   memory_service.rs (agentcowork-memory) · reader.rs (agentcowork-search)
//!   sync.rs + sync_transport.rs (agentcowork-storage/sync owner)
//!   scheduler_service.rs (own scheduler crate → becomes Work creator only)
//!   openai_server.rs (agentcowork-catalog/model plane) · local.rs (local models)
//!   cua.rs (agentcowork-desktop policy) · widgets.rs (shell/ui plane)
//!   forge.rs, email.rs, messaging.rs (connector plane) · challenge.rs (eval)
//!   doctor.rs (support/ops plane) · shell_integration.rs (shell plane)
//!
//! This module is documentation-as-code: it exists so the boundary is
//! reviewable in one place and so a `TODO(D20)` marker has a home. Moving a
//! file updates exactly one line here.
