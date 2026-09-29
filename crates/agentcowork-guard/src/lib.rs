//! agentcowork-guard — Guard-1: deterministic pre-exec scanning of every
//! generated shell string, filesystem path and URL before execution
//! (ARCH/06 §6.2; doc 06, doc 03 §8).
//!
//! P7 scope:
//! - [`blocklist`] — the full destructive-pattern corpus (rm -rf variants,
//!   mkfs, dd, drop database, format, fork bombs, key exfiltration, `.git`
//!   destruction, home wipes).
//! - [`prescan`] — pre-exec scan of shell strings, filesystem paths and URLs.
//! - [`deflection`] — P62.5/P69.G2 shell-bias deflection: a shell command that
//!   drives Office, the browser, or the desktop from outside the shared plane is
//!   refused with the façade to pivot onto. Token-aware text scan, **not** AST
//!   inspection; read the module's documented limits before trusting it.
//! - [`urlfloor`] — URL floors: `file://` only inside granted roots, scheme
//!   guard.
//! - [`ticket`] — the authorization ticket contract (doc 53 §3).
//! - [`ratelimit`] — control-plane admission control: a bounded, per-caller and
//!   per-command token bucket that fails closed with the canonical taxonomy
//!   (`ARCH/12-TRUST.md` §11, `REQ-TRUST-009`).
//! - [`redteam`] — the cyber red-team corpus (doc 26) as an adversarial test
//!   suite; the 100%-blocked gate.
//! - [`injection`] — P7.6 prompt-injection defense: context scan,
//!   `<user_document>` wrapping, tool-result sanitization, estop.
//! - [`pathfloor`] — P7.7 canonicalization, symlink-safe boundaries, `..`
//!   prevention, and the path-floor fuzz gate.
//! - [`profiles`] — P7.7 profile-gated hooks (minimal/standard/strict),
//!   ECC pattern (doc 46).
//! - [`configscan`] — P7.7 AgentShield config scanning of agentcowork.toml,
//!   blueprints and MCP configs.
//! - [`loopguard`] — P7.7 SHA256 circuit breaker against infinite loops.
//! - [`manifest`] — P7.7 Ed25519-signed extension manifests (OpenFang
//!   pattern).
//!
//! Guard-2 (diff-card approval, human-in-the-loop UX) is a separate gate
//! (P7.5).

pub mod approval_policy;
pub mod autonomy;
pub mod batch;
pub mod blocklist;
pub mod capability_broker;
pub mod capability_contract;
pub mod configscan;
pub mod decision;
pub mod deflection;
pub mod diffcard;
pub mod ecc;
pub mod egress;
pub mod egress_http;
pub mod floors;
pub mod fs_broker;
pub mod granter;
pub mod injection;
pub mod loopguard;
pub mod manifest;
pub mod netfloor;
pub mod path_seal;
pub mod pathfloor;
pub mod permissions;
pub mod prescan;
pub mod profiles;
pub mod protected_paths;
pub mod ratelimit;
pub mod redteam;
pub mod release;
pub mod reviewer;
pub mod sandbox;
pub mod seccomp;
pub mod skillstore;
pub mod structural;
pub mod ticket;
pub mod toctou;
pub mod urlfloor;

pub use autonomy::{AutonomyPolicy, AutonomyVerdict, Mode, RiskClass};
pub use batch::{
    BatchAction, BatchOperation, BatchReceipt, BatchTicket, BatchTicketStore, change_set_hash,
};
pub use blocklist::{BLOCKLIST, BlocklistCategory, blocklist_for};
pub use capability_broker::{
    CapabilityBroker, CapabilityBrokerError, CapabilityGrant, CapabilityRequest,
    EphemeralCredential, LocalCapabilityBroker,
};
pub use capability_contract::CapabilityInvocation;
pub use decision::{DecisionPackage, WebActionKind};
pub use deflection::{
    DEFLECTION_AUDIT_KIND, DeflectionNudge, DeflectionTarget, deflect_shell_bias,
};
pub use diffcard::{CardAction, CardResponse, NativeCard, render_native_card};
pub use egress::{ConnectivityMode, EgressEngine, EgressPlan, EgressVerdict};
pub use fs_broker::{
    BrokerHost, BrokerOp, BrokerRequest, BrokerResponse, BrokerTransport, InProcessBroker,
};
pub use granter::{
    CapabilityGranter, GrantError, GrantRequest, GrantedCapabilities, HostGrant, TrustFlags,
    wildcard_match,
};
pub use injection::Estop;
pub use netfloor::{
    NetClass, NetFloorDenied, NetPolicy, classify_host, classify_ip, host_allowed,
    is_always_blocked, preflight_url,
};
pub use path_seal::{PathSeal, SealError, SealState};
pub use pathfloor::{
    FloorVerdict, FsOp, GrantAxis, PathGrant, canonicalize_no_follow, enforce_floor,
    is_inside_root, normalize_lexical,
};
pub use permissions::{AutonomyPreset, Operation, PermissionsPolicy, PolicyAction, Rule};
pub use prescan::{PreExecScan, ScanTarget, scan_path, scan_shell, scan_url};
pub use profiles::{GateAction, Hook, Profile};
pub use ratelimit::{Limit, RateLimitConfig, RateLimitError, RateLimitScope, RateLimiter};
pub use release::{
    EgressPolicy, EgressPolicyEngine, EnforcementZone, ReleaseDecision, ReleaseReceipt,
};
#[cfg(target_os = "linux")]
pub use sandbox::LinuxBwrapBackend;
pub use sandbox::{
    PathAccess, PathRule, SandboxBackend, SandboxBackendKind, SandboxError, SandboxProcess,
    SandboxProfile, SandboxReceipt, SandboxRole, SandboxSpec, SyscallGroup,
    enforced_backend_capabilities, linux_bwrap_available, resolve_sandbox_backend,
};
pub use seccomp::{Action, ArgFilter, SeccompError, SeccompPolicy, SyscallRule};
pub use structural::{
    SHELL_OPERATORS, StructuralVerdict, contains_shell_operator, structural_verdict,
};
pub use ticket::{
    ApprovalSource, AuthorizationTicket, GuardReceipt, ReceiptAction, RiskLevel, RiskTier,
    TicketState, TicketStore,
};
pub use toctou::{
    ExecBinding, FileBinding, NetBinding, ResourceBinding, ToctouError, bind_exec_bytes, bind_path,
    bind_url, is_blocked_ip, open_parent_dir, reverify_exec, reverify_path, reverify_url,
};

/// Scan everything pre-exec: shell string, filesystem paths, URLs.
/// Returns every blocklist pattern that matched any target.
pub fn scan_all(shell: &str, paths: &[&str], urls: &[&str]) -> Vec<PreExecScan> {
    let mut hits = Vec::new();
    let guard = prescan::guard();
    if guard.is_blocked(shell) {
        hits.push(PreExecScan::new(
            ScanTarget::Shell,
            shell.to_string(),
            guard.scan(shell),
        ));
    }
    for p in paths {
        if guard.is_blocked(p) {
            hits.push(PreExecScan::new(
                ScanTarget::Path,
                (*p).to_string(),
                guard.scan(p),
            ));
        }
    }
    for u in urls {
        if guard.is_blocked(u) {
            hits.push(PreExecScan::new(
                ScanTarget::Url,
                (*u).to_string(),
                guard.scan(u),
            ));
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_all_catches_across_targets() {
        let hits = scan_all("ls -la", &["/tmp/x"], &["file:///etc/passwd"]);
        assert!(hits.is_empty());
        let hits = scan_all("rm -rf ~", &["/etc/passwd"], &["https://ok.test"]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].target, ScanTarget::Shell);
    }
}
