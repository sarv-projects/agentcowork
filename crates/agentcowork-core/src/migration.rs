//! P29 — Native Sidecar Migration ledger (external review 2026-08-17; spec
//! §9.1 R6, ARCH/01 §1.3). The target: the ~48K-line TS engine becomes a
//! native Rust sidecar in three tiers (~93MB → ~15MB). This module is the
//! **ledger + exit criterion** — each tier's status, its TS home, and the
//! Rust counterpart that replaces it (most already exist and are tested in
//! this workspace; the tiers that "keep TS" are decisions, recorded as such).
//!
//! The Tier-1 rationale correction is recorded here too: keys never reach
//! the sidecar *already* (the credential broker is Rust); the real value is
//! eliminating IPC hops + dropping the V8 execution surface + memory.

use serde::{Deserialize, Serialize};

/// The migration tier status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierStatus {
    /// The Rust replacement exists and is tested; TS call-site rewiring is
    /// the remaining work.
    RustSideLanded,
    /// The seam (in-process loop / direct guard / streaming) is landed and
    /// tested; the TS collapse onto it is the remaining work.
    SeamLanded,
    /// Decision recorded: this tier keeps TS by design.
    KeepsTs,
    /// Not started.
    Open,
    /// The replacement was built and then archived with its consumer
    /// (`ADR-0005`): it is not live in v1, and the tier keeps its history
    /// instead of pretending the collapse is still on the roadmap.
    Archived,
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationTier {
    pub id: String,
    pub name: String,
    /// The TS home this tier replaces (or keeps).
    pub ts_home: String,
    /// The Rust crate/module that replaces it.
    pub rust_home: String,
    pub status: TierStatus,
    pub note: String,
}

/// The ledger (all 11 tiers).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationLedger {
    pub tiers: Vec<MigrationTier>,
}

impl MigrationLedger {
    pub fn current() -> Self {
        Self {
            tiers: vec![
                MigrationTier {
                    id: "1a".into(),
                    name: "collapse IPC".into(),
                    ts_home: "frame.ts/message.ts/index.ts (99+76+364)".into(),
                    rust_home: "ARCH/archive/native_loop.rs (`NativeLoop`, mpsc in-process)".into(),
                    status: TierStatus::Archived,
                    note: "landed 2026-08-24 as the built-in engine's in-process loop, then archived 2026-09-21 with the engine it existed to serve (P71.2e, ADR-0005 §4); not compiled, nothing may depend on it".into(),
                },
                MigrationTier {
                    id: "1b".into(),
                    name: "guard.ts → native".into(),
                    ts_home: "guard.ts (108)".into(),
                    rust_home: "ARCH/archive/native_loop.rs (`DirectGuard`, agentcowork-guard TicketStore in-process)".into(),
                    status: TierStatus::Archived,
                    note: "tickets minted+consumed in-process with zero IPC hop; archived with the built-in engine, and `DirectGuard` may be re-homed to agentcowork-guard when the governed binding returns — it must never carry an agent identity".into(),
                },
                MigrationTier {
                    id: "1c".into(),
                    name: "Rust owns streaming".into(),
                    ts_home: "core-providers (15/3.3K)".into(),
                    rust_home: "agentcowork-vault::broker (SSE ChatStreamEvent; heap.ts/orphan.ts eliminated via OS primitives — orphan.rs job objects / pdeathsig)".into(),
                    status: TierStatus::SeamLanded,
                    note: "broker already holds keys + streams SSE; failover loop + orphan prevention are OS-level".into(),
                },
                MigrationTier {
                    id: "2a".into(),
                    name: "core-memory → pure-Rust math".into(),
                    ts_home: "core-memory (24/4.7K)".into(),
                    rust_home: "agentcowork-memory (actr, graph, fsrs, fusion, planner)".into(),
                    status: TierStatus::RustSideLanded,
                    note: "ACT-R / spreading activation / FSRS / fusion exist + tested; TS call-sites rewire onto them".into(),
                },
                MigrationTier {
                    id: "2b".into(),
                    name: "core-search → Rust".into(),
                    ts_home: "core-search (45/3.9K)".into(),
                    rust_home: "agentcowork-search (G8Cascade, DeepResearch) + agentcowork-memory::bm25".into(),
                    status: TierStatus::RustSideLanded,
                    note: "fetch cascade + BM25 rerank exist; tantivy local index is the follow-on".into(),
                },
                MigrationTier {
                    id: "2c".into(),
                    name: "core-files → consolidate".into(),
                    ts_home: "core-files (88/14.5K)".into(),
                    rust_home: "agentcowork-office (calamine/IronCalc/lopdf) + agentcowork-storage".into(),
                    status: TierStatus::RustSideLanded,
                    note: "largest LOC win; text-extraction/chunking/diffing glue stays Rust (docx-rs, tree-sitter)".into(),
                },
                MigrationTier {
                    id: "2d".into(),
                    name: "core-automations/core-engine → Rust".into(),
                    ts_home: "core-automations/core-engine (42/6.8K + 16/2K)".into(),
                    rust_home: "agentcowork-blueprint (blueprint DAG, iteration budgets, change_set) + agentcowork-core::automation_runtime".into(),
                    status: TierStatus::RustSideLanded,
                    note: "state machines / DAG execution / circuit breakers / deterministic cancellation exist".into(),
                },
                MigrationTier {
                    id: "3a".into(),
                    name: "prompt/router/catalog → config/templates".into(),
                    ts_home: "prompt.ts/router.ts/catalog.ts (190+182+157)".into(),
                    rust_home: "agentcowork-agents (templates, persona) + TOML config".into(),
                    status: TierStatus::KeepsTs,
                    note: "fast-iterating glue — Minijinja/Tera + Serde TOML is the option, keep TS acceptable".into(),
                },
                MigrationTier {
                    id: "3b".into(),
                    name: "core-ai/core-agents — keep TS/QuickJS".into(),
                    ts_home: "core-ai (40/4.7K) + core-agents (4/300)".into(),
                    rust_home: "rquickjs sandbox".into(),
                    status: TierStatus::KeepsTs,
                    note: "prompt tuning, blueprint loops, experimental subagent personas stay in the sandbox".into(),
                },
                MigrationTier {
                    id: "3c".into(),
                    name: "core-connectors — keep TS/QuickJS".into(),
                    ts_home: "core-connectors (38/7.1K)".into(),
                    rust_home: "rquickjs sandbox (MCP-servers + native connector decision)".into(),
                    status: TierStatus::KeepsTs,
                    note: "fast-changing third-party schemas (Google/Slack/Composio)".into(),
                },
                MigrationTier {
                    id: "exit".into(),
                    name: "exit criterion".into(),
                    ts_home: "—".into(),
                    rust_home: "this module".into(),
                    status: TierStatus::Open,
                    note: "full test parity + combined warm RSS <120MB + zero plain-text key in non-Rust memory + no capability regression".into(),
                },
            ],
        }
    }

    pub fn tier(&self, id: &str) -> Option<&MigrationTier> {
        self.tiers.iter().find(|t| t.id == id)
    }
}

/// The exit-criterion check (deterministic; the numbers come from the P8
/// RSS publisher + the test harness).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ExitCriterion {
    /// Full test parity (workspace + coordinator + UI tsc green).
    pub test_parity: bool,
    /// Combined warm RSS in MB (target <120MB).
    pub warm_rss_mb: f64,
    /// Zero plain-text key in non-Rust memory (already asserted by the
    /// sealed-channel test).
    pub no_plaintext_key: bool,
    /// No capability regression (the capability matrix re-run).
    pub no_capability_regression: bool,
}

impl ExitCriterion {
    pub const TARGET_WARM_RSS_MB: f64 = 120.0;

    pub fn met(&self) -> bool {
        self.test_parity
            && self.warm_rss_mb < Self::TARGET_WARM_RSS_MB
            && self.no_plaintext_key
            && self.no_capability_regression
    }

    pub fn render(&self) -> String {
        format!(
            "test parity: {} · warm RSS {:.0}MB (target <{:.0}) · keys: {} · capability regression: {} → {}",
            self.test_parity,
            self.warm_rss_mb,
            Self::TARGET_WARM_RSS_MB,
            if self.no_plaintext_key {
                "clean"
            } else {
                "LEAK"
            },
            if self.no_capability_regression {
                "none"
            } else {
                "YES"
            },
            if self.met() { "EXIT MET" } else { "not met" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_has_all_11_tiers() {
        let l = MigrationLedger::current();
        assert_eq!(l.tiers.len(), 11);
        for id in [
            "1a", "1b", "1c", "2a", "2b", "2c", "2d", "3a", "3b", "3c", "exit",
        ] {
            assert!(l.tier(id).is_some(), "missing tier {id}");
        }
    }

    #[test]
    fn exit_criterion_is_strict() {
        let partial = ExitCriterion {
            test_parity: true,
            warm_rss_mb: 200.0,
            no_plaintext_key: true,
            no_capability_regression: true,
        };
        assert!(!partial.met());
        let met = ExitCriterion {
            test_parity: true,
            warm_rss_mb: 95.0,
            no_plaintext_key: true,
            no_capability_regression: true,
        };
        assert!(met.met());
        assert!(met.render().contains("EXIT MET"));
    }
}
