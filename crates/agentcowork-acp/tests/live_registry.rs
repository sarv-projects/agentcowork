//! P60.14 — live ACP registry refresh E2E (the dynamic-catalog honest ceiling).
//!
//! `#[ignore]` so the default suite stays offline; run explicitly:
//!
//! ```text
//! cargo test -p agentcowork-acp --test live_registry -- --ignored --nocapture
//! ```
//!
//! It proves the exact composition the desktop shell resolves against
//! (`acp_cmds::launch_registry()` = `LaunchRegistry::builtin()` +
//! `RegistryClient::load_cached()` + `RegistryIndex::merge_into`) against the
//! **real CDN**, using the production `ureq` transport:
//!
//! 1. a fresh cache dir has no catalog (the shell must degrade to builtin);
//! 2. `refresh()` fetches `registry.json` and parses it;
//! 3. the cache it writes is what `load_cached()` reads back (offline path =
//!    the same catalog);
//! 4. `merge_into` upserts every registry agent into the curated seed without
//!    dropping the inbuilt row — so a refreshed catalog really does drive
//!    discovery/launch data.
//!
//! It deliberately does **not** claim occupancy: a merged row is a *catalog*
//! fact. Whether a binary exists on this machine stays `agent_installed`
//! (install record or PATH) in the shell.

use agentcowork_acp::{HarnessProtocol, LaunchRegistry, Platform, RegistryClient};

/// Mirrors `registry_index::canonical_id` (private): the merge resolves a
/// registry id to the launch-registry id through this mapping, so the test has
/// to apply the same one to look the row up afterwards.
fn canonical_id(id: &str) -> String {
    match id {
        "github-copilot-cli" => "copilot".to_string(),
        "grok-build" => "grok".to_string(),
        "glm-acp-agent" => "glm-agent".to_string(),
        "factory-droid" => "factory-droid".to_string(),
        other => other.trim_end_matches("-acp").to_string(),
    }
}

#[test]
#[ignore = "network: fetches the live ACP registry CDN"]
fn live_registry_refresh_parses_caches_and_merges_into_launch_registry() {
    let dir =
        std::env::temp_dir().join(format!("agentcowork-live-registry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let client = RegistryClient::new(dir.clone());

    // 1. Before any fetch there is no catalog — the shell must fall back to the
    //    curated seed rather than inventing rows.
    assert!(
        client.load_cached().is_none(),
        "a fresh cache dir must not report a cached catalog"
    );

    // 2. The production transport fetches the real CDN and parses it.
    let snap = client
        .refresh()
        .expect("live registry refresh (is the network available?)");
    assert!(
        !snap.index.agents.is_empty(),
        "the live registry must declare agents"
    );
    assert!(
        !snap.index.version.is_empty(),
        "the registry pins a version"
    );
    eprintln!(
        "live registry: version={} agents={}",
        snap.index.version,
        snap.index.agents.len()
    );

    // 3. The cache round-trips: the offline path sees the same catalog.
    let cached = client.load_cached().expect("refresh wrote the cache");
    assert_eq!(
        cached.index.agents.len(),
        snap.index.agents.len(),
        "cached agent count must match the fetched snapshot"
    );
    assert_eq!(cached.index.version, snap.index.version);
    assert!(cached.from_cache);
    assert!(cached.fetched_at_ms > 0, "the fetch time is recorded");

    // 4. Merge exactly as the shell does.
    let mut reg = LaunchRegistry::builtin();
    let seed = reg.clone();
    let seed_len = reg.agents.len();
    snap.index.merge_into(&mut reg, Platform::current());
    assert!(
        reg.agents.len() >= seed_len,
        "merge never drops curated rows (seed {seed_len} → {})",
        reg.agents.len()
    );
    assert!(
        reg.get("everyaios").is_some(),
        "the inbuilt AgentCowork row survives the registry merge"
    );
    let acp_rows = reg
        .agents
        .iter()
        .filter(|m| m.protocol == HarnessProtocol::Acp)
        .count();
    assert!(
        acp_rows >= snap.index.agents.len(),
        "every registry agent must have an ACP launch row ({acp_rows} rows, {} registry agents)",
        snap.index.agents.len()
    );

    // ...and it *drives* the row rather than being ignored: at least one curated
    // entry changes (version pin / distribution / description) or is added.
    let changed: Vec<&str> = snap
        .index
        .agents
        .iter()
        .filter(|a| {
            let id = canonical_id(&a.id);
            match (seed.get(&id), reg.get(&id)) {
                (None, Some(_)) => true, // brand-new catalog row
                (Some(before), Some(after)) => format!("{before:?}") != format!("{after:?}"),
                _ => false,
            }
        })
        .map(|a| a.id.as_str())
        .collect();
    eprintln!(
        "merged: {} rows ({} seed), {acp_rows} ACP rows; {} registry rows changed or added",
        reg.agents.len(),
        seed_len,
        changed.len()
    );
    eprintln!("changed/added: {changed:?}");
    assert!(
        !changed.is_empty(),
        "a live refresh must change or add at least one launch row — otherwise the registry is not driving launch"
    );
    assert!(
        snap.index
            .agents
            .iter()
            .any(|a| a.id.contains("claude") || a.id.contains("opencode")),
        "expected a familiar ecosystem agent in the live registry"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
