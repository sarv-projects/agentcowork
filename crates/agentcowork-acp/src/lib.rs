//! agentcowork-acp — the ACP (Agent Client Protocol) harness bridge (P6.8 /
//! F12 / J17; doc 45 §1, doc 57 §2).
//!
//! - [`frame`] — newline-delimited JSON-RPC framing (the ACP stdio wire).
//! - [`messages`] — ACP v1 message types (initialize, session/new, prompt,
//!   stop reasons, tool kinds, session/update, request_permission).
//! - [`permission_bridge`] — FIX-03: the `session/request_permission` bridge
//!   (once/always/reject onto offered options, bound single-use ticket, bounded
//!   redacted diff preview, fail-closed).
//! - [`client`] — the [`AcpSession`] client lifecycle (spawn → initialize →
//!   session/new → prompt → permission → cancel) with a testable transport.
//! - [`registry`] — the agent **launch registry** (the `ollama launch`
//!   pattern): manifests with auth-mode badges + distribution types + how to
//!   drive each agent, with our own inbuilt engine as the default.
//! - [`registry_index`] — F8: the official ACP registry schema (`registry.json`)
//!   parse + platform resolution + install plans + merge + allow-list policy.
//! - [`registry_client`] — F8: fetch + cache the official registry (pluggable
//!   HTTP transport).
//! - [`installer`] — F8: the install executor (download → sha256 → extract +
//!   install-state persistence).
//! - [`prefix_guard`] — P69.E9: the I16 prefix-stability guard (fingerprint +
//!   per-Work state machine over cache-boundary events).

pub mod a2a;
pub mod agent_backend;
pub mod chief;
pub mod client;
pub mod frame;
pub mod harness_config;
pub mod installer;
pub mod messages;
pub mod permission_bridge;
pub mod prefix_guard;
pub mod registry;
pub mod registry_client;
pub mod registry_index;

pub use a2a::{A2aError, AgentCard, AgentCardVerifier, AgentSkill, CardTrust, SignedAgentCard};
pub use agent_backend::{
    AgentBackendSpec, BackendChannel, BackendError, BaseUrlError, ProviderBinding, backend_spec,
    builtin_backend_specs, injected_names, plan_env, redact_base_url, unexpressed,
    validate_base_url, validate_model_id,
};
pub use chief::{
    AcpChief, Approval, COWORK_AFFINITY_STEERING, ChiefAdapter, ChiefCapabilities, ChiefError,
    ChiefEvent, DelegateChief, EventStream, GovernedSession, PermissionRequest, SessionHandle,
    SessionOptions, SessionState, UserMessage, build_chief_prompt,
    build_chief_prompt_with_steering, governance_mode,
};
pub use client::{
    AcpCancelHandle, AcpCancelSender, AcpError, AcpSession, AcpShutdownSender, AcpTransport,
    ChildEnvName, ProcessTransport, ProcessTransportError, PromptOutcome, SessionLoadOutcome,
};
pub use frame::{
    MAX_ACP_FRAME_BYTES, decode_messages, encode_message, finish_decode, try_encode_message,
};
pub use harness_config::{
    ClaudeCodeConfig, CodexConfig, HarnessConfigError, HarnessConfigWriter, OpenCodeConfig,
    ProviderConfig, builtin_writers,
};
pub use installer::{InstallError, InstallOutcome, Installer, OwnershipMarker};
pub use messages::{
    AgentCapabilities, AgentInfo, AuthMethod, AuthMethodType, AuthenticateParams,
    AuthenticateResult, AvailableCommand, ClientCapabilities, ClientInfo, ConfigOption,
    ConfigOptionCapabilities, ConfigOptionValue, ContentBlock, EmbeddedResource, EnvVariable,
    FsCapabilities, HttpHeader, HttpMcpServer, InitializeParams, InitializeResult, Location,
    McpHttpLeaseBinding, McpServer, McpServerError, McpServerValidationError, PROTOCOL_VERSION,
    PermissionDecision, PermissionOption, PermissionOptionKind, PermissionOutcome,
    PermissionRequestParams, PermissionResult, Position, PromptCapabilities, PromptContent,
    PromptUsage, SessionCapabilities, SessionLoadParams, SessionLoadResult, SessionNewParams,
    SessionNewResult, SessionPromptParams, SessionPromptResult, SessionUpdate,
    SetConfigOptionParams, SetConfigOptionResult, StopReason, TextRange, ToolCall, ToolKind,
    ToolStatus,
};
pub use messages::{
    MAX_ACP_SESSION_ID_BYTES, MAX_MCP_SERVER_ARG_BYTES, MAX_MCP_SERVER_ARGS,
    MAX_MCP_SERVER_COMMAND_BYTES, MAX_MCP_SERVER_ENV_ENTRIES, MAX_MCP_SERVER_ENV_VALUE_BYTES,
    MAX_MCP_SERVER_HEADER_NAME_BYTES, MAX_MCP_SERVER_HEADER_VALUE_BYTES, MAX_MCP_SERVER_HEADERS,
    MAX_MCP_SERVER_NAME_BYTES, MAX_MCP_SERVER_URL_BYTES, MAX_MCP_SERVERS_PER_REQUEST,
    MAX_SESSION_ID_BYTES, SseMcpServer, StdioMcpServer,
};
pub use permission_bridge::{
    AcpApprovalChoice, BridgeAnswer, BridgeError, PermissionBridge, PermissionPreview, REDACTION,
    TicketBinding, TicketFacts, TrustOutcome, redact_secret_shapes, require_once_ticket,
};
pub use prefix_guard::{PrefixEvent, PrefixGuard, fingerprint_stable_prefix};
pub use registry::{
    AuthMode, Distribution, HarnessManifest, HarnessProtocol, LaunchPlan, LaunchRegistry,
};
pub use registry_client::{FetchError, RegistryClient};
pub use registry_index::{
    BinaryTarget, InstallKind, InstallSpec, Platform, PolicyVerdict, RegistryAgent,
    RegistryDistribution, RegistryIndex, RegistryPolicy,
};
