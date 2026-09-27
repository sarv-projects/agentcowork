//! P71.6b — **prove the engine is optional.**
//!
//! `ADR/0005` §2's rule is that every v1 feature works with **no built-in
//! binding present**. The built-in engine was deleted rather than bypassed
//! (`P71.2c`), so this suite exists to prove the deletion is real, complete,
//! and that the turn path still works without it:
//!
//! 1. **A full turn runs on an external agent alone** — the public ACP client
//!    drives a real spawned agent process through `initialize` →
//!    `session/new` → `session/prompt` and the turn completes. Nothing in the
//!    path is AgentCowork's own inference.
//! 2. **The agent's usage report survives the turn** (`P71.4`) — the figure the
//!    agent sends is carried on `PromptOutcome` verbatim, and a turn that
//!    reports nothing is recorded as *unreported* rather than as zero tokens.
//! 3. **The built-in engine cannot come back silently** — the protocol enums
//!    are matched exhaustively, so adding an `Inbuilt`/`ModelBackend` variant
//!    fails to compile here; and the archived loop plus the deleted TypeScript
//!    engine are asserted absent on disk.
//!
//! Cross-platform: the fixture is a plain Rust binary; nothing here needs a
//! credential, a provider, or a network.

use agentcowork_acp::{
    AcpSession, ClientInfo, HarnessProtocol, PROTOCOL_VERSION, PermissionDecision, ProcessTransport,
};
use agentcowork_memory::{UsageLedger, UsageSource};
// The canonical agent protocol lives in the schema crate (P69.D25).
use agentcowork_types::AgentProtocol;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "agentcowork".into(),
        title: "AgentCowork".into(),
        version: "0.1.0".into(),
    }
}

/// Resolve a compiled test-fixture binary via cargo's `CARGO_BIN_EXE_*` env.
/// Cargo's naming has drifted across versions, so probe the common spellings.
fn fixture_bin(name: &str) -> String {
    let underscored = name.replace('-', "_");
    let candidates = [
        format!("CARGO_BIN_EXE_{name}"),
        format!("CARGO_BIN_EXE_{}", underscored),
        format!("CARGO_BIN_EXE_{}", underscored.to_uppercase()),
        format!("CARGO_BIN_EXE_{}", name.to_uppercase()),
    ];
    for key in &candidates {
        if let Ok(v) = std::env::var(key) {
            return v;
        }
    }
    panic!("no CARGO_BIN_EXE_* env for `{name}`");
}

/// The repository root, from this crate's manifest dir.
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// (1) + (2) — one real turn, driven by an external agent, with no built-in
/// engine anywhere in the path.
#[test]
fn a_turn_completes_on_an_external_agent_alone() {
    let bin = fixture_bin("mock_acp_agent");
    let transport = ProcessTransport::spawn(&bin, &[], &[]).expect("spawn mock agent");
    let mut session = AcpSession::new(transport);

    let init = session.initialize(client_info()).expect("initialize");
    assert_eq!(init.protocol_version, PROTOCOL_VERSION);
    assert!(session.is_alive());

    let _ = session.session_new("/tmp", vec![]).expect("session/new");

    let outcome = session
        .prompt("engine optional acceptance", |_| {
            PermissionDecision::allow()
        })
        .expect("prompt turn");
    assert!(
        outcome.updates.iter().any(|u| u
            .content
            .iter()
            .any(|c| c.text.contains("engine optional acceptance"))),
        "the agent must have answered the turn: {:?}",
        outcome.updates
    );

    // (2) The agent's own usage is an observation the client carries through
    // untouched — AgentCowork never measured this turn, the agent reported it.
    let usage = outcome.usage.expect("the agent reported usage");
    assert!(usage.reported(), "a reporting agent must read as reported");
    assert_eq!(usage.input_tokens, 1_200);
    assert_eq!(usage.output_tokens, 180);
    // The cache split stays split: a write is billed apart from a read.
    assert_eq!(usage.cached_read_tokens, 900);
    assert_eq!(usage.cached_write_tokens, 40);
    assert_eq!(usage.cost_usd, Some(0.0125));

    // …and that report is exactly what the ledger records, with its source.
    let mut ledger = UsageLedger::new();
    ledger.set_active("chat-1", "mock-acp");
    ledger.begin_session("mock-acp");
    ledger.record_observed(
        UsageSource::AgentReport,
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_read_tokens > 0,
        usage.cached_read_tokens,
        usage.cached_write_tokens,
        usage.cost_usd.unwrap_or(0.0),
    );
    ledger.clear_agent();
    ledger.clear_active();
    assert_eq!(ledger.total().total_tokens(), 1_380);
    assert_eq!(ledger.total().cache_split(), (900, 40));
    assert_eq!(
        ledger.source_for_agent("mock-acp"),
        Some(UsageSource::AgentReport)
    );

    session.shutdown();
    assert!(!session.is_alive());
}

/// (2 cont.) — an agent that reports **nothing** must produce an unreported
/// turn, never a zero measurement (`ARCH/ROUTING.md` §5, `I15`).
#[test]
fn an_agent_that_reports_nothing_is_unreported_not_zero() {
    let bin = fixture_bin("mock_acp_agent");
    let transport = ProcessTransport::spawn(&bin, &[], &[]).expect("spawn mock agent");
    let mut session = AcpSession::new(transport);
    session.initialize(client_info()).expect("initialize");
    let _ = session.session_new("/tmp", vec![]).expect("session/new");

    let outcome = session
        .prompt("no-usage turn", |_| PermissionDecision::allow())
        .expect("prompt turn");
    assert!(
        outcome.usage.is_none(),
        "this agent sent no usage block: {:?}",
        outcome.usage
    );

    let mut ledger = UsageLedger::new();
    ledger.record_unreported("mock-acp");
    let obs = ledger.observations();
    assert_eq!(obs.unreported.get("mock-acp"), Some(&1));
    // Absent, not zero: nothing was added to the token totals.
    assert_eq!(ledger.total().total_tokens(), 0);
    assert!(obs.by_source.is_empty());

    session.shutdown();
}

/// (3) — the protocol vocabulary has no built-in variant. Both matches are
/// exhaustive, so reintroducing `Inbuilt` / `ModelBackend` breaks the build
/// here rather than silently re-entering the product.
#[test]
fn no_built_in_protocol_variant_exists() {
    fn agent_protocol_spelling(p: AgentProtocol) -> &'static str {
        match p {
            AgentProtocol::Acp => "acp",
            AgentProtocol::ModelOnly => "model_only",
        }
    }
    fn harness_protocol_spelling(p: HarnessProtocol) -> &'static str {
        match p {
            HarnessProtocol::Acp => "acp",
        }
    }

    assert_eq!(agent_protocol_spelling(AgentProtocol::Acp), "acp");
    assert_eq!(
        agent_protocol_spelling(AgentProtocol::ModelOnly),
        "model_only"
    );
    assert_eq!(harness_protocol_spelling(HarnessProtocol::Acp), "acp");

    // The retired spellings are readable off the wire for compatibility, but
    // they name no runnable variant.
    assert!(serde_json::from_str::<AgentProtocol>("\"inbuilt\"").is_err());
    assert!(serde_json::from_str::<HarnessProtocol>("\"model_backend\"").is_err());
}

/// (3 cont.) — the deletion is on disk, not just in the type system: no
/// compilable copy of the native loop survives anywhere in the live tree, and
/// the sidecar reasoning package is gone. The archived copy itself lives under
/// the git-ignored `ARCHIVE/v0/` tree, so its presence is a local fact rather
/// than a CI-verifiable one; what must hold everywhere is that no copy remains
/// where a compiler could pick it up.
#[test]
fn the_built_in_engine_is_archived_or_deleted() {
    fn find_named(dir: &std::path::Path, name: &str, depth: usize) -> bool {
        if depth == 0 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if matches!(
                    path.file_name().and_then(|n| n.to_str()),
                    Some("target") | Some("node_modules") | Some("ARCHIVE") | Some(".git")
                ) {
                    continue;
                }
                if find_named(&path, name, depth - 1) {
                    return true;
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return true;
            }
        }
        false
    }

    let root = repo_root();

    assert!(
        !find_named(&root, "native_loop.rs", 8),
        "native_loop.rs must not exist anywhere in the live tree (crates/, packages/, src-tauri/)"
    );

    assert!(
        !root.join("packages/core-engine/src").exists(),
        "the sidecar reasoning package must be gone (P71.2c)"
    );
    assert!(
        !root.join("packages/coordinator/src/chat.ts").exists(),
        "the sidecar turn loop must be gone (P71.2c)"
    );
    // The retired coordinator loop stays recoverable only from the local,
    // git-ignored archive, so the live tree must not carry it in any form.
    assert!(
        !root.join("ARCH/archive/coordinator-loop").exists()
            && !root
                .join("packages/coordinator/src/coordinator-loop")
                .exists(),
        "the retired coordinator loop must not exist in the live tree"
    );
}
