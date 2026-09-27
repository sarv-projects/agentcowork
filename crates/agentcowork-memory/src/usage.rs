//! Usage accounting (P8 — the per-key cost display, per-session breakdown,
//! and cache-hit-rate items). A [`UsageLedger`] records token usage per
//! provider key and per session, tracks prompt-cache hit/miss events, and
//! answers the display queries the UI needs — all deterministic and
//! serializable.
//!
//! **P71.4 — every number is an observation, with its source attached.** With
//! the built-in engine deferred and the provider broker deleted (`ADR-0005`),
//! there is no gateway of ours in the path to measure a turn. Usage therefore
//! arrives as a *report*: from the agent (its own prompt-turn usage), from an
//! ACP event, from a provider's report where it exposes one, or from an
//! AgentCowork capability call. [`UsageLedger::record_observed`] stamps which of
//! those it was; [`UsageLedger::record_unreported`] exists for the opposite
//! case, because "the agent reported nothing" and "the agent reported zero"
//! are different facts and the second must never be invented from the first
//! (`ARCH/ROUTING.md` §5, **I15**).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a usage figure came from (`ARCH/ROUTING.md` §5).
///
/// Closed on purpose: a new producer must be a deliberate, named addition
/// rather than an anonymous write into the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The bound external agent reported its own turn usage.
    AgentReport,
    /// An ACP event carried the figure (works even for an agent that only
    /// announces usage mid-turn).
    AcpEvent,
    /// A provider exposed the usage for a call AgentCowork made directly.
    ProviderReport,
    /// An AgentCowork capability call measured its own work (search, office,
    /// browser, delegation).
    CapabilityCall,
}

impl UsageSource {
    /// The wire spelling (snake_case, matching the serde rename).
    pub fn as_str(self) -> &'static str {
        match self {
            UsageSource::AgentReport => "agent_report",
            UsageSource::AcpEvent => "acp_event",
            UsageSource::ProviderReport => "provider_report",
            UsageSource::CapabilityCall => "capability_call",
        }
    }
}

/// One recorded usage event.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageRecord {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cached_tokens: u64,
    /// P71.4 — prompt-cache **writes**, which are billed separately from a
    /// read and therefore never folded into `cached_tokens`.
    pub cached_write_tokens: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    /// P71.4 — the cost the *producer* reported for these tokens. This is an
    /// observation; it stays separate from [`UsageRecord::est_cost_usd`], which
    /// is **our estimate** from configured prices. The two must never be added
    /// together or presented as one number (I15).
    pub reported_cost_usd: f64,
}

impl UsageRecord {
    pub fn total_tokens(self) -> u64 {
        self.tokens_in + self.tokens_out
    }

    /// Cache-aware split: (read, write). Reported verbatim, never inferred.
    pub fn cache_split(self) -> (u64, u64) {
        (self.cached_tokens, self.cached_write_tokens)
    }

    /// Cache-hit rate 0..=1 (1 = every request hit the prompt cache).
    pub fn cache_hit_rate(self) -> f64 {
        let calls = self.cache_hits + self.cache_misses;
        if calls == 0 {
            0.0
        } else {
            self.cache_hits as f64 / calls as f64
        }
    }

    /// Estimate cost in USD (a rough per-Mtok rate; the broker's real prices
    /// plug in via `set_price`). 0 when no prices are configured.
    pub fn est_cost_usd(self, input_per_mtok: f64, output_per_mtok: f64) -> f64 {
        let input_tokens = self.tokens_in.saturating_sub(self.cached_tokens) as f64;
        input_tokens / 1e6 * input_per_mtok + self.tokens_out as f64 / 1e6 * output_per_mtok
    }
}

impl std::ops::Add for UsageRecord {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            tokens_in: self.tokens_in + rhs.tokens_in,
            tokens_out: self.tokens_out + rhs.tokens_out,
            cached_tokens: self.cached_tokens + rhs.cached_tokens,
            cached_write_tokens: self.cached_write_tokens + rhs.cached_write_tokens,
            cache_hits: self.cache_hits + rhs.cache_hits,
            cache_misses: self.cache_misses + rhs.cache_misses,
            reported_cost_usd: self.reported_cost_usd + rhs.reported_cost_usd,
        }
    }
}

/// P71.4 — the provenance read model `usage_snapshot` publishes: tokens per
/// observation source, plus the owners whose turns produced **no** report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageObservations {
    /// Source spelling (`agent_report` · `acp_event` · `provider_report` ·
    /// `capability_call`) → the tokens that source reported.
    pub by_source: BTreeMap<String, UsageRecord>,
    /// Owner id (agent, else session) → count of turns with no usage report.
    pub unreported: BTreeMap<String, u64>,
}

/// One agent's summary row (P17 per-agent session metrics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionMetrics {
    pub agent: String,
    /// Sessions started for this agent/harness.
    pub sessions: u64,
    pub usage: UsageRecord,
}

impl AgentSessionMetrics {
    /// Estimated USD for this harness — callers supply the provider price
    /// (the ledger's key pricing doesn't map 1:1 to agents).
    pub fn est_cost_usd(&self, input_per_mtok: f64, output_per_mtok: f64) -> f64 {
        self.usage.est_cost_usd(input_per_mtok, output_per_mtok)
    }
}

/// The usage ledger.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageLedger {
    /// Provider key id → usage.
    by_key: BTreeMap<String, UsageRecord>,
    /// Session id → usage.
    by_session: BTreeMap<String, UsageRecord>,
    /// Agent/harness id → usage (P17 per-agent session metrics).
    by_agent: BTreeMap<String, UsageRecord>,
    /// Agent/harness id → session count (sessions-per-agent).
    sessions_by_agent: BTreeMap<String, u64>,
    /// Provider key → price per Mtok (input, output). Empty = no pricing.
    prices: BTreeMap<String, (f64, f64)>,
    /// P71.4 — tokens per observation source (the fleet-wide provenance split).
    by_source: BTreeMap<UsageSource, UsageRecord>,
    /// P71.4 — the run's primary agent (set by the turn path, never guessed).
    primary_agent: Option<String>,
    /// P71.4 — the last source that reported for each owner, so a per-row
    /// surface can say *who* said it. Absent = nothing has been reported yet.
    source_by_key: BTreeMap<String, UsageSource>,
    source_by_session: BTreeMap<String, UsageSource>,
    source_by_agent: BTreeMap<String, UsageSource>,
    /// P71.4 — turns that completed **without** a usage report, per owner. This
    /// is the honesty counter: it is what lets a surface say "this agent
    /// reported no usage" instead of showing a measured zero (**I15**).
    unreported_turns: BTreeMap<String, u64>,
    /// Active session (what `record_observed` attributes to when a session is set).
    active_session: Option<String>,
    /// The key the active session is billed to.
    active_key: Option<String>,
    /// The active agent/harness (what `record_observed` attributes to, when set).
    active_agent: Option<String>,
}

impl UsageLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the price (USD per Mtok) for a provider key. Input + output.
    pub fn set_price(&mut self, key: &str, input_per_mtok: f64, output_per_mtok: f64) {
        self.prices
            .insert(key.to_string(), (input_per_mtok, output_per_mtok));
    }

    /// Bind the active session (subsequent `record` calls attribute to it).
    pub fn set_active(&mut self, session_id: &str, key: &str) {
        self.active_session = Some(session_id.to_string());
        self.active_key = Some(key.to_string());
    }

    pub fn clear_active(&mut self) {
        self.active_session = None;
        self.active_key = None;
    }

    /// Begin a session for an agent/harness (P17): increments the agent's
    /// session count and makes subsequent `record` calls attribute to it.
    pub fn begin_session(&mut self, agent_id: &str) {
        *self
            .sessions_by_agent
            .entry(agent_id.to_string())
            .or_insert(0) += 1;
        self.active_agent = Some(agent_id.to_string());
    }

    pub fn clear_agent(&mut self) {
        self.active_agent = None;
    }

    /// P71.4 — record one **observed** usage report: tokens, the cache
    /// read/write split, and any cost the producer reported, together with
    /// *where the figure came from*.
    ///
    /// There is no door that writes usage without a source: an unlabelled
    /// number in this ledger would be exactly the fabricated precision
    /// `ARCH/ROUTING.md` §5 forbids, so provenance is a parameter rather than
    /// an option.
    #[allow(clippy::too_many_arguments)]
    pub fn record_observed(
        &mut self,
        source: UsageSource,
        tokens_in: u64,
        tokens_out: u64,
        cache_hit: bool,
        cached_read_tokens: u64,
        cached_write_tokens: u64,
        reported_cost_usd: f64,
    ) {
        let rec = UsageRecord {
            tokens_in,
            tokens_out,
            cached_tokens: cached_read_tokens,
            cached_write_tokens,
            cache_hits: u64::from(cache_hit),
            cache_misses: u64::from(!cache_hit),
            reported_cost_usd,
        };
        let key = self.active_key.clone();
        let session = self.active_session.clone();
        let agent = self.active_agent.clone();
        if let Some(key) = key {
            let entry = self.by_key.entry(key.clone()).or_default();
            *entry = *entry + rec;
            self.source_by_key.insert(key, source);
        }
        if let Some(session) = session {
            let entry = self.by_session.entry(session.clone()).or_default();
            *entry = *entry + rec;
            self.source_by_session.insert(session, source);
        }
        if let Some(agent) = agent {
            let entry = self.by_agent.entry(agent.clone()).or_default();
            *entry = *entry + rec;
            self.source_by_agent.insert(agent, source);
        }
        // A report landed for this turn, so the active owner is no longer in
        // the "finished with nothing reported" set. Agent first: an agent
        // report is about the agent, and clearing the session's counter on
        // someone else's report would hide a real gap.
        if let Some(agent) = self.active_agent.clone() {
            self.unreported_turns.remove(&agent);
        } else if let Some(session) = self.active_session.clone() {
            self.unreported_turns.remove(&session);
        }
        let fleet = self.by_source.entry(source).or_default();
        *fleet = *fleet + rec;
    }

    /// P71.4 — a turn finished and nothing was reported for it. Recorded
    /// explicitly so a surface can distinguish "no usage reported" from
    /// "zero tokens used" (**I15**). Never adds tokens.
    pub fn record_unreported(&mut self, owner: &str) {
        if owner.is_empty() {
            return;
        }
        *self.unreported_turns.entry(owner.to_string()).or_insert(0) += 1;
    }

    /// P71.4 — the provenance read model: tokens per source, and which owners
    /// have turns with no report at all.
    pub fn observations(&self) -> UsageObservations {
        UsageObservations {
            by_source: self
                .by_source
                .iter()
                .map(|(s, r)| (s.as_str().to_string(), *r))
                .collect(),
            unreported: self
                .unreported_turns
                .iter()
                .map(|(owner, turns)| (owner.clone(), *turns))
                .collect(),
        }
    }

    /// P71.4 — attribute the run's primary agent. Only the turn path knows
    /// which agent is primary; the ledger never guesses it from a key name.
    pub fn set_primary_agent(&mut self, agent_id: &str) {
        self.primary_agent = Some(agent_id.to_string());
    }

    pub fn clear_primary_agent(&mut self) {
        self.primary_agent = None;
    }

    /// P71.4 — `(primary_tokens, worker_tokens)` from the **agent** dimension.
    ///
    /// `None` when no primary agent is attributed: a split invented from key
    /// names would be exactly the fabricated precision this row removes, so
    /// the caller renders "not attributed" instead of a share (**I15**).
    pub fn primary_worker_split(&self) -> Option<(u64, u64)> {
        let primary = self.primary_agent.as_ref()?;
        let primary_tokens = self
            .by_agent
            .get(primary)
            .copied()
            .unwrap_or_default()
            .total_tokens();
        let all: u64 = self
            .by_agent
            .values()
            .map(|r| r.total_tokens())
            .fold(0u64, |a, b| a.saturating_add(b));
        Some((primary_tokens, all.saturating_sub(primary_tokens)))
    }

    /// The source that reported for an owner (key / session / agent), if any.
    pub fn source_for_key(&self, key: &str) -> Option<UsageSource> {
        self.source_by_key.get(key).copied()
    }

    pub fn source_for_session(&self, session: &str) -> Option<UsageSource> {
        self.source_by_session.get(session).copied()
    }

    pub fn source_for_agent(&self, agent: &str) -> Option<UsageSource> {
        self.source_by_agent.get(agent).copied()
    }

    /// Usage for one provider key (the per-key cost display).
    pub fn key_usage(&self, key: &str) -> Option<UsageRecord> {
        self.by_key.get(key).copied()
    }

    /// Usage for one session (the per-session breakdown).
    pub fn session_usage(&self, session_id: &str) -> Option<UsageRecord> {
        self.by_session.get(session_id).copied()
    }

    /// Every key with usage (for the per-key table).
    pub fn keys(&self) -> Vec<(String, UsageRecord)> {
        self.by_key.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    /// Every session with usage.
    pub fn sessions(&self) -> Vec<(String, UsageRecord)> {
        self.by_session
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Per-agent/harness summary (P17): sessions + tokens + est. cost per
    /// harness, for the Spend/analytics surface. Cost uses the *active key's*
    /// price when the agent billed to a key, else 0.
    pub fn agent_metrics(&self) -> Vec<AgentSessionMetrics> {
        let mut out: Vec<AgentSessionMetrics> = self
            .by_agent
            .iter()
            .map(|(agent, usage)| AgentSessionMetrics {
                agent: agent.clone(),
                sessions: self.sessions_by_agent.get(agent).copied().unwrap_or(0),
                usage: *usage,
            })
            .collect();
        // Agents with sessions but no recorded usage still appear.
        for (agent, sessions) in &self.sessions_by_agent {
            if !out.iter().any(|m| &m.agent == agent) {
                out.push(AgentSessionMetrics {
                    agent: agent.clone(),
                    sessions: *sessions,
                    usage: UsageRecord::default(),
                });
            }
        }
        out.sort_by_key(|m| std::cmp::Reverse(m.usage.total_tokens()));
        out
    }

    /// Totals across all keys.
    pub fn total(&self) -> UsageRecord {
        self.by_key
            .values()
            .copied()
            .fold(UsageRecord::default(), |a, r| a + r)
    }

    /// Per-key cost using the configured prices (0 when a key has no price).
    pub fn key_cost_usd(&self, key: &str) -> Option<f64> {
        let usage = self.by_key.get(key)?;
        let (in_p, out_p) = self.prices.get(key)?;
        Some(usage.est_cost_usd(*in_p, *out_p))
    }

    /// The cache-hit rate across all recorded calls (per-provider display).
    pub fn cache_hit_rate(&self) -> f64 {
        self.total().cache_hit_rate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_breaks_down_by_key_and_session() {
        let mut l = UsageLedger::new();
        l.set_active("s1", "anthropic");
        l.record_observed(UsageSource::AgentReport, 1_000, 200, true, 800, 0, 0.0); // cache hit
        l.set_active("s2", "openai");
        l.record_observed(UsageSource::AgentReport, 500, 100, false, 0, 0, 0.0);
        l.clear_active();

        let anthropic = l.key_usage("anthropic").unwrap();
        assert_eq!(anthropic.tokens_in, 1_000);
        let s1 = l.session_usage("s1").unwrap();
        assert_eq!(s1.tokens_out, 200);
        assert_eq!(l.keys().len(), 2);
        assert_eq!(l.sessions().len(), 2);
        assert_eq!(l.total().total_tokens(), 1_800);
    }

    #[test]
    fn cache_hit_rate_tracks() {
        let mut l = UsageLedger::new();
        l.set_active("s1", "k");
        l.record_observed(UsageSource::AgentReport, 100, 10, true, 80, 0, 0.0);
        l.record_observed(UsageSource::AgentReport, 100, 10, true, 80, 0, 0.0);
        l.record_observed(UsageSource::AgentReport, 100, 10, false, 0, 0, 0.0);
        l.clear_active();
        let r = l.key_usage("k").unwrap();
        assert_eq!(r.cache_hits, 2);
        assert_eq!(r.cache_misses, 1);
        assert!((r.cache_hit_rate() - 2.0 / 3.0).abs() < 1e-9);
        assert!((l.cache_hit_rate() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn cost_uses_configured_prices() {
        let mut l = UsageLedger::new();
        l.set_active("s1", "deepseek");
        l.record_observed(
            UsageSource::AgentReport,
            1_000_000,
            1_000_000,
            false,
            0,
            0,
            0.0,
        ); // 1M in + 1M out
        l.clear_active();
        assert_eq!(l.key_cost_usd("deepseek"), None); // no price set
        l.set_price("deepseek", 0.27, 1.10);
        let cost = l.key_cost_usd("deepseek").unwrap();
        assert!((cost - (0.27 + 1.10)).abs() < 1e-9);
    }

    #[test]
    fn cached_tokens_are_not_billed_as_input() {
        let mut l = UsageLedger::new();
        l.set_price("k", 3.0, 15.0);
        l.set_active("s1", "k");
        l.record_observed(
            UsageSource::AgentReport,
            1_000_000,
            100_000,
            true,
            800_000,
            0,
            0.0,
        ); // 800k cached
        l.clear_active();
        let cost = l.key_cost_usd("k").unwrap();
        // Only 200k uncached input tokens billed.
        assert!((cost - (0.2 * 3.0 + 0.1 * 15.0)).abs() < 1e-9);
    }

    #[test]
    fn per_agent_session_metrics() {
        let mut l = UsageLedger::new();
        l.begin_session("claude");
        l.set_active("s1", "anthropic");
        l.record_observed(UsageSource::AgentReport, 1_000, 200, true, 800, 0, 0.0);
        l.clear_agent();
        l.begin_session("claude");
        l.record_observed(UsageSource::AgentReport, 500, 100, false, 0, 0, 0.0);
        l.clear_agent();
        l.begin_session("opencode");
        l.set_active("s2", "openai");
        l.record_observed(UsageSource::AgentReport, 100, 50, false, 0, 0, 0.0);
        l.clear_agent();
        l.clear_active();

        let metrics = l.agent_metrics();
        assert_eq!(metrics.len(), 2);
        let claude = metrics.iter().find(|m| m.agent == "claude").unwrap();
        assert_eq!(claude.sessions, 2);
        assert_eq!(claude.usage.total_tokens(), 1_800);
        let opencode = metrics.iter().find(|m| m.agent == "opencode").unwrap();
        assert_eq!(opencode.sessions, 1);
        // Tokens-per-harness, sorted desc.
        assert_eq!(metrics[0].agent, "claude");
    }

    #[test]
    fn p71_4_provenance_is_recorded_per_source_and_owner() {
        let mut l = UsageLedger::new();
        l.set_active("s1", "agent-acme");
        l.begin_session("acme");
        l.record_observed(UsageSource::AgentReport, 900, 100, false, 0, 0, 0.07);
        l.clear_agent();
        l.clear_active();

        let obs = l.observations();
        assert_eq!(obs.by_source.len(), 1);
        let row = obs.by_source.get("agent_report").expect("source row");
        assert_eq!(row.total_tokens(), 1_000);
        assert!((row.reported_cost_usd - 0.07).abs() < 1e-9);
        assert_eq!(l.source_for_agent("acme"), Some(UsageSource::AgentReport));
        assert!(obs.unreported.is_empty());
    }

    #[test]
    fn p71_4_unreported_turns_are_not_zero_tokens() {
        let mut l = UsageLedger::new();
        l.record_unreported("acme");
        l.record_unreported("acme");
        let obs = l.observations();
        assert_eq!(obs.unreported.get("acme"), Some(&2));
        // Nothing was recorded as tokens: an unreported turn is absent, not 0.
        assert_eq!(l.total().total_tokens(), 0);
        assert!(obs.by_source.is_empty());

        // A later report from the same owner clears the gap counter.
        l.begin_session("acme");
        l.record_observed(UsageSource::AgentReport, 10, 5, false, 0, 0, 0.0);
        l.clear_agent();
        assert!(l.observations().unreported.is_empty());
    }

    #[test]
    fn p71_4_cache_write_is_never_folded_into_read() {
        let mut l = UsageLedger::new();
        l.set_active("s1", "k");
        l.record_observed(UsageSource::AgentReport, 1_000, 100, true, 800, 250, 0.0);
        l.clear_active();
        let r = l.key_usage("k").unwrap();
        assert_eq!(r.cache_split(), (800, 250));
    }

    #[test]
    fn p71_4_split_is_absent_without_an_attributed_primary() {
        let mut l = UsageLedger::new();
        l.begin_session("acme");
        l.record_observed(UsageSource::AgentReport, 500, 100, false, 0, 0, 0.0);
        l.clear_agent();
        assert_eq!(l.primary_worker_split(), None);

        l.set_primary_agent("acme");
        assert_eq!(l.primary_worker_split(), Some((600, 0)));
    }

    #[test]
    fn ledger_serializes() {
        let mut l = UsageLedger::new();
        l.set_active("s1", "k");
        l.record_observed(UsageSource::AgentReport, 100, 10, true, 50, 0, 0.0);
        l.clear_active();
        let json = serde_json::to_string(&l).unwrap();
        let back: UsageLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total().total_tokens(), 110);
    }
}
