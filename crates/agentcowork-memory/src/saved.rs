//! P5 saved-vs-discovered metric (doc 65 §10 — claude-mem steal): memory
//! injection measured, not assumed.
//!
//! Every memory observation carries a measured `token_cost` — the tokens the
//! injected memory replaced (what the model would otherwise have spent
//! rediscovering the fact). The context builder records each injection into
//! the [`SavedVsDiscovered`] ledger, which also counts the tokens actually
//! spent *discovering* (retrieval, compaction, indexing). The net is a real
//! number: memories earn their place only when `tokens_saved > tokens_spent`.

use serde::{Deserialize, Serialize};

/// Where a memory observation came from — the discovery-cost bucket it
/// charges against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    /// Injected from the reinforced review queue (cheap — already known).
    Reinforced,
    /// Retrieved fresh (charged retrieval tokens).
    Retrieved,
    /// Crystallized from a completed task (charged the run's remaining cost).
    Crystallized,
}

/// One memory observation with its measured token cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryObservation {
    /// The memory id (or a stable handle).
    pub memory_id: String,
    /// Measured tokens the injected memory replaced (est. via the fusion
    /// `approx_tokens` convention — deterministic, not a model guess).
    pub token_cost: u64,
    pub source: ObservationSource,
}

impl MemoryObservation {
    pub fn new(memory_id: impl Into<String>, token_cost: u64, source: ObservationSource) -> Self {
        Self {
            memory_id: memory_id.into(),
            token_cost,
            source,
        }
    }
}

/// The cumulative saved-vs-discovered ledger. The context builder updates it
/// per turn: injections credit `tokens_saved`; retrieval/indexing costs
/// debit `tokens_spent_discovering`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedVsDiscovered {
    pub tokens_saved: u64,
    pub tokens_spent_discovering: u64,
    /// Observations recorded (the audit trail, capped at a sane bound).
    #[serde(default)]
    pub observations: Vec<MemoryObservation>,
}

impl SavedVsDiscovered {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one injection: credit its measured token cost.
    pub fn record_injection(&mut self, obs: MemoryObservation) {
        self.tokens_saved += obs.token_cost;
        if self.observations.len() < 10_000 {
            self.observations.push(obs);
        }
    }

    /// Record a discovery cost (retrieval, compaction, indexing tokens).
    pub fn record_discovery(&mut self, tokens: u64) {
        self.tokens_spent_discovering += tokens;
    }

    /// Net savings (negative = discovery is costing more than it saves).
    pub fn net_savings(&self) -> i64 {
        self.tokens_saved as i64 - self.tokens_spent_discovering as i64
    }

    /// The saved-vs-discovered ratio — 1.0 means savings exactly pay for
    /// discovery; >1 the memories earn their place. 0 when nothing spent.
    pub fn ratio(&self) -> f64 {
        if self.tokens_spent_discovering == 0 {
            return 0.0;
        }
        self.tokens_saved as f64 / self.tokens_spent_discovering as f64
    }

    /// The context-builder summary line (the measured claim, not an
    /// assumption): what was saved, what discovery cost, and the ratio.
    pub fn render(&self) -> String {
        format!(
            "memory savings: {saved} tokens saved vs {spent} spent discovering (ratio {ratio:.2}, net {net:+})",
            saved = self.tokens_saved,
            spent = self.tokens_spent_discovering,
            ratio = self.ratio(),
            net = self.net_savings(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injections_credit_savings() {
        let mut l = SavedVsDiscovered::new();
        l.record_injection(MemoryObservation::new(
            "m1",
            120,
            ObservationSource::Reinforced,
        ));
        l.record_injection(MemoryObservation::new(
            "m2",
            80,
            ObservationSource::Retrieved,
        ));
        assert_eq!(l.tokens_saved, 200);
        assert_eq!(l.net_savings(), 200);
    }

    #[test]
    fn discovery_costs_debit() {
        let mut l = SavedVsDiscovered::new();
        l.record_injection(MemoryObservation::new(
            "m1",
            200,
            ObservationSource::Retrieved,
        ));
        l.record_discovery(150);
        assert_eq!(l.net_savings(), 50);
        assert!((l.ratio() - 4.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn losing_ledger_is_honest() {
        let mut l = SavedVsDiscovered::new();
        l.record_injection(MemoryObservation::new(
            "m1",
            10,
            ObservationSource::Crystallized,
        ));
        l.record_discovery(500);
        assert!(l.net_savings() < 0);
        assert!(l.render().contains("net -490"));
    }

    #[test]
    fn render_is_deterministic() {
        let mut l = SavedVsDiscovered::new();
        l.record_injection(MemoryObservation::new(
            "m1",
            100,
            ObservationSource::Reinforced,
        ));
        l.record_discovery(50);
        assert_eq!(l.render(), l.render());
    }
}
