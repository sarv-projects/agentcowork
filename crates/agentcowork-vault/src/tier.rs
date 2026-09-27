//! Asymmetric model tiering (P6.10 — A7, doc 53 §5, doc 59 OmniRoute,
//! doc 62 NeMo Switchyard).
//!
//! The surgical hierarchy is *routing policy, not a mandatory pipeline*: a
//! simple edit should run on the cheapest executor tier directly; only broad
//! refactors escalate to the frontier planner. This module is the pure
//! selection logic — it never touches keys (the [`crate::keyring::KeyRing`]
//! owns secrets); the broker consumes a [`TierDecision`] and asks the ring
//! for the actual key.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A role in the tier chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierRole {
    /// The frontier model that plans / judges (expensive, accurate).
    Planner,
    /// The cheap/fast model that grinds execution (cheap MoE / local).
    Executor,
    /// A deterministic verifier (no model — EV1 checks).
    Verifier,
    /// Retrieval/RepoMap only — no generative tier at all.
    Retrieval,
    /// A known crystallized skill — zero model tokens (P6.5).
    Skill,
}

/// The task classification that drives shortest-path chain selection
/// (doc 53 §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    SimpleEdit,
    BroadRefactor,
    CodeQuestion,
    BrowserResearch,
    SpreadsheetCleanup,
    KnownSkill,
    GeneralChat,
}

/// The pre-set scoring mode packs (doc 59 §3). Each is a *profile*, not a
/// hardcoded table of floats — the floats are the OmniRoute-verified weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierMode {
    /// Latency first (ship-fast).
    Fast,
    /// Cost first (cost-saver).
    Cheap,
    /// Quality first (quality-first).
    Quality,
    /// Local/offline models only (offline-friendly).
    Offline,
}

/// The routing-strategy vocabulary (doc 59 §6). `Lkgp` / `ResetAware` /
/// `Headroom` / `CacheOptimized` are the three upgrades over the old
/// "429 → cooldown → next key" failover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    Priority,
    RoundRobin,
    LeastUsed,
    /// Sticky last-known-good-path.
    Lkgp,
    /// Prefer the key whose quota-reset window is most favorable.
    ResetAware,
    /// Most remaining quota / headroom.
    Headroom,
    /// Route to the connection holding the prompt-cache prefix.
    CacheOptimized,
}

impl RoutingStrategy {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "priority" => Self::Priority,
            "round_robin" | "round-robin" => Self::RoundRobin,
            "least_used" | "least-used" => Self::LeastUsed,
            "lkgp" => Self::Lkgp,
            "reset_aware" | "reset-aware" => Self::ResetAware,
            "headroom" => Self::Headroom,
            "cache_optimized" | "cache-optimized" => Self::CacheOptimized,
            _ => return None,
        })
    }

    pub const ALL: &'static [&'static str] = &[
        "priority",
        "round_robin",
        "least_used",
        "lkgp",
        "reset_aware",
        "headroom",
        "cache_optimized",
    ];
}

/// A resolved model tier selection for one role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierDecision {
    pub role: TierRole,
    /// The concrete model id (or `auto/<category>:<tier>` still unresolved).
    pub model: String,
    /// Category when an `auto/…` DSL was resolved.
    pub category: Option<String>,
    /// Tier when an `auto/…` DSL was resolved.
    pub tier: Option<String>,
}

/// The asymmetric tier configuration (A7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierConfig {
    /// Frontier model for planning (planner_model).
    pub planner_model: String,
    /// Cheap/local models for grinding (subagent_models) — first is default.
    pub subagent_models: Vec<String>,
    /// Per-agent (blueprint id) model override — wins over role defaults.
    pub agent_overrides: BTreeMap<String, String>,
    /// The active scoring mode pack.
    pub mode: TierMode,
    /// Default key-routing strategy per provider (empty = Priority).
    pub routing: BTreeMap<String, RoutingStrategy>,
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            planner_model: "auto/reasoning:pro".into(),
            subagent_models: vec!["nvidia/nemotron-3.5-lightning".into()],
            agent_overrides: BTreeMap::new(),
            mode: TierMode::Cheap,
            routing: BTreeMap::new(),
        }
    }
}

impl TierConfig {
    /// Resolve the model for a role, honoring per-agent overrides first.
    pub fn resolve(&self, role: TierRole, agent_id: Option<&str>) -> String {
        if let Some(agent) = agent_id {
            if let Some(m) = self.agent_overrides.get(agent) {
                return m.clone();
            }
        }
        match role {
            TierRole::Planner => self.planner_model.clone(),
            TierRole::Executor => self
                .subagent_models
                .first()
                .cloned()
                .unwrap_or_else(|| self.planner_model.clone()),
            TierRole::Verifier | TierRole::Retrieval | TierRole::Skill => {
                // No model for deterministic/retrieval/skill tiers.
                String::new()
            }
        }
    }
}

/// The shortest-path tier chain per task class (doc 53 §5.2).
pub fn shortest_path_chain(class: TaskClass) -> Vec<TierRole> {
    use TierRole::*;
    match class {
        TaskClass::SimpleEdit => vec![Planner, Executor],
        TaskClass::BroadRefactor => vec![Planner, Executor, Executor],
        TaskClass::CodeQuestion => vec![Planner, Retrieval],
        TaskClass::BrowserResearch => vec![Planner, Executor],
        TaskClass::SpreadsheetCleanup => vec![Planner, Executor],
        TaskClass::KnownSkill => vec![Skill],
        TaskClass::GeneralChat => vec![Planner],
    }
}

/// The `auto/<category>:<tier>` DSL (doc 59 §4). Returns `(category, tier)`.
pub fn parse_auto_model(model: &str) -> Option<(String, String)> {
    let rest = model.strip_prefix("auto/")?;
    let (category, tier) = rest.split_once(':')?;
    if category.is_empty() || tier.is_empty() {
        return None;
    }
    Some((category.to_string(), tier.to_string()))
}

/// Escalate-by-floor (doc 62 §2): the executor tier is the *default*; the
/// frontier planner is only pulled in when a task is classified as
/// planning/judgment-heavy. This returns the chain *without* the frontier
/// planner when `escalate` is false — the floor is the cheap tier, not the
/// frontier.
pub fn escalate_by_floor(class: TaskClass, escalate: bool) -> Vec<TierRole> {
    let mut chain = shortest_path_chain(class);
    if !escalate && chain.contains(&TierRole::Planner) {
        // Drop the frontier planner; the executor is the floor.
        chain.retain(|r| *r != TierRole::Planner);
        if chain.is_empty() {
            chain.push(TierRole::Executor);
        }
    }
    chain
}

/// The four mode packs, condensed to the 5-factor subset that carries 0.70 of
/// the full OmniRoute weight (doc 59 §2: health + quota + cost + latency +
/// taskFit). Each `(health, quota, cost_inv, latency_inv, task_fit)` tuple is
/// **renormalized to sum to 1.0**; the relative emphasis mirrors doc 59 §3 —
/// Fast weighs latency, Cheap weighs cost, Quality weighs taskFit, Offline
/// weighs quota (local models have no $ cost but scarce context/quota).
pub fn mode_weights(mode: TierMode) -> (f64, f64, f64, f64, f64) {
    match mode {
        TierMode::Fast => (0.20, 0.10, 0.05, 0.55, 0.10),
        TierMode::Cheap => (0.15, 0.10, 0.55, 0.05, 0.15),
        TierMode::Quality => (0.15, 0.05, 0.05, 0.05, 0.70),
        TierMode::Offline => (0.25, 0.45, 0.10, 0.05, 0.15),
    }
}

// ---------------------------------------------------------------------------
// NeMo Switchyard-style routing (P6.10 — doc 62)
// ---------------------------------------------------------------------------

/// The Switchyard decision (doc 62 §2): which tier runs the task. The
/// executor tier (Nemotron 3.5 Lightning) is the **default floor**; the
/// frontier planner is only pulled in for planning/judgment-heavy work. This
/// is the deterministic policy — the LangChain proof numbers (74% cheaper /
/// 7% frontier calls / 145 tasks) are a benchmark of that policy, not a
/// separate mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchyardTier {
    /// Nemotron 3.5 Lightning executor — the cheap floor.
    Executor,
    /// Frontier planner — escalated only when planning weight demands it.
    Planner,
    /// No generative tier (retrieval / known skill).
    NoModel,
}

/// Switchyard routing policy (doc 62 §2): return the tier that should run a
/// task given its class and a `planning_weight` in `[0,1]` (how much of the
/// task is decomposition/judgment vs grinding). `escalate` forces the
/// planner; the default floor is always the executor.
pub fn switchyard_route(class: TaskClass, planning_weight: f64, escalate: bool) -> SwitchyardTier {
    use TaskClass::*;
    let planning = planning_weight.clamp(0.0, 1.0);
    if escalate || planning >= 0.6 {
        return SwitchyardTier::Planner;
    }
    match class {
        // Pure knowledge/skill work never needs a generative tier.
        KnownSkill => SwitchyardTier::NoModel,
        // Code questions lean on retrieval first; the executor grinds.
        CodeQuestion if planning < 0.3 => SwitchyardTier::Executor,
        // Everything else runs on the cheap executor floor by default.
        SimpleEdit | BroadRefactor | CodeQuestion | BrowserResearch | SpreadsheetCleanup
        | GeneralChat => SwitchyardTier::Executor,
    }
}

/// The default executor model for the Switchyard floor (doc 62 §2 — the
/// Nemotron 3.5 Lightning tier).
pub const SWITCHYARD_EXECUTOR_MODEL: &str = "nvidia/nemotron-3.5-lightning";

/// Compute the weighted score of a candidate (the five factors from
/// [`mode_weights`], values in `[0,1]`), clamped and normalized. The
/// broker uses this to rank candidates; this is the pure scorer.
pub fn score(
    mode: TierMode,
    health: f64,
    quota: f64,
    cost_inv: f64,
    latency_inv: f64,
    task_fit: f64,
) -> f64 {
    let (wh, wq, wc, wl, wt) = mode_weights(mode);
    let clamp = |v: f64| v.clamp(0.0, 1.0);
    wh * clamp(health)
        + wq * clamp(quota)
        + wc * clamp(cost_inv)
        + wl * clamp(latency_inv)
        + wt * clamp(task_fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_resolves_roles() {
        let c = TierConfig::default();
        assert_eq!(c.resolve(TierRole::Planner, None), "auto/reasoning:pro");
        assert_eq!(
            c.resolve(TierRole::Executor, None),
            "nvidia/nemotron-3.5-lightning"
        );
        assert_eq!(c.resolve(TierRole::Verifier, None), "");
        assert_eq!(c.resolve(TierRole::Skill, None), "");
    }

    #[test]
    fn agent_override_wins_over_role() {
        let mut c = TierConfig::default();
        c.agent_overrides
            .insert("coder".into(), "anthropic/claude-opus".into());
        assert_eq!(
            c.resolve(TierRole::Executor, Some("coder")),
            "anthropic/claude-opus"
        );
        // A different agent falls back to the executor default.
        assert_eq!(
            c.resolve(TierRole::Executor, Some("researcher")),
            "nvidia/nemotron-3.5-lightning"
        );
    }

    #[test]
    fn shortest_path_is_minimal_per_class() {
        assert_eq!(
            shortest_path_chain(TaskClass::SimpleEdit),
            vec![TierRole::Planner, TierRole::Executor]
        );
        assert_eq!(
            shortest_path_chain(TaskClass::KnownSkill),
            vec![TierRole::Skill]
        );
        assert_eq!(
            shortest_path_chain(TaskClass::CodeQuestion),
            vec![TierRole::Planner, TierRole::Retrieval]
        );
    }

    #[test]
    fn escalate_by_floor_drops_planner() {
        // No escalation → the floor is the executor (cheap tier), never the
        // frontier planner.
        assert_eq!(
            escalate_by_floor(TaskClass::SimpleEdit, false),
            vec![TierRole::Executor]
        );
        // Escalation → full chain including the planner.
        assert_eq!(
            escalate_by_floor(TaskClass::SimpleEdit, true),
            vec![TierRole::Planner, TierRole::Executor]
        );
        // KnownSkill has no planner to drop.
        assert_eq!(
            escalate_by_floor(TaskClass::KnownSkill, false),
            vec![TierRole::Skill]
        );
    }

    #[test]
    fn auto_dsl_parses() {
        assert_eq!(
            parse_auto_model("auto/coding:fast"),
            Some(("coding".into(), "fast".into()))
        );
        assert_eq!(
            parse_auto_model("auto/reasoning:pro"),
            Some(("reasoning".into(), "pro".into()))
        );
        assert_eq!(parse_auto_model("gpt-4o"), None);
        assert_eq!(parse_auto_model("auto/coding"), None);
        assert_eq!(parse_auto_model("auto/"), None);
    }

    #[test]
    fn routing_vocabulary_roundtrips() {
        for s in RoutingStrategy::ALL {
            assert!(RoutingStrategy::parse(s).is_some(), "{s}");
        }
        assert_eq!(RoutingStrategy::parse("lkgp"), Some(RoutingStrategy::Lkgp));
        assert_eq!(RoutingStrategy::parse("bogus"), None);
    }

    #[test]
    fn mode_weights_sum_to_one() {
        for mode in [
            TierMode::Fast,
            TierMode::Cheap,
            TierMode::Quality,
            TierMode::Offline,
        ] {
            let (h, q, c, l, t) = mode_weights(mode);
            let total = h + q + c + l + t;
            assert!((total - 1.0).abs() < 1e-9, "{mode:?} sums to {total}");
        }
    }

    #[test]
    fn switchyard_floor_is_executor_by_default() {
        // Grinding work stays on the cheap executor floor (74%-cheaper
        // property of doc 62: most tasks never touch the frontier).
        assert_eq!(
            switchyard_route(TaskClass::SimpleEdit, 0.1, false),
            SwitchyardTier::Executor
        );
        assert_eq!(
            switchyard_route(TaskClass::SpreadsheetCleanup, 0.2, false),
            SwitchyardTier::Executor
        );
        assert_eq!(
            switchyard_route(TaskClass::GeneralChat, 0.0, false),
            SwitchyardTier::Executor
        );
    }

    #[test]
    fn switchyard_escalates_on_planning_weight_or_force() {
        // High planning weight → frontier planner.
        assert_eq!(
            switchyard_route(TaskClass::BroadRefactor, 0.8, false),
            SwitchyardTier::Planner
        );
        // Force escalation wins regardless of weight.
        assert_eq!(
            switchyard_route(TaskClass::SimpleEdit, 0.0, true),
            SwitchyardTier::Planner
        );
    }

    #[test]
    fn switchyard_known_skill_needs_no_model() {
        assert_eq!(
            switchyard_route(TaskClass::KnownSkill, 0.0, false),
            SwitchyardTier::NoModel
        );
        // Even a known skill escalates when forced.
        assert_eq!(
            switchyard_route(TaskClass::KnownSkill, 0.0, true),
            SwitchyardTier::Planner
        );
    }

    #[test]
    fn switchyard_executor_model_constant_matches_default_config() {
        // The default TierConfig already uses the Nemotron Lightning floor;
        // the constant must agree so the two stay in lock-step.
        assert_eq!(
            TierConfig::default().subagent_models[0],
            SWITCHYARD_EXECUTOR_MODEL
        );
    }

    #[test]
    fn scorer_is_bounded_and_orders_correctly() {
        // All factors at max → 1.0 (weights sum to 1).
        assert!((score(TierMode::Cheap, 1.0, 1.0, 1.0, 1.0, 1.0) - 1.0).abs() < 1e-9);
        // All at zero → 0.0.
        assert_eq!(score(TierMode::Cheap, 0.0, 0.0, 0.0, 0.0, 0.0), 0.0);
        // A healthy, cheap candidate beats a broken, expensive one.
        let good = score(TierMode::Cheap, 1.0, 1.0, 1.0, 1.0, 1.0);
        let bad = score(TierMode::Cheap, 0.0, 0.0, 0.0, 0.0, 1.0);
        assert!(good > bad);
    }
}
