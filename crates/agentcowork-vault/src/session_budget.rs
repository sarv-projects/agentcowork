//! Per-session $ budget (P1.3, J11) — the "hard $ budget per session" kill
//! switch. Default **$2.00/agent, configurable**; when a session's spent cost
//! reaches the limit the broker refuses further calls for that session and the
//! UI surfaces "stopped: $X limit".
//!
//! The tracker is in-memory (the durable record is the `token_usage` ledger —
//! [`crate::Vault::session_spend`]); the broker holds one and checks it as a
//! single choke point before every call, so a misbehaving sidecar cannot
//! bypass the budget by holding its own key.

use std::collections::HashMap;
use std::sync::Mutex;

/// J11 default: hard $ budget per session.
pub const DEFAULT_SESSION_BUDGET_USD: f64 = 2.00;

/// Per-session spend tracker (thread-safe; cheap).
#[derive(Debug)]
pub struct SessionBudget {
    limit: f64,
    spent: Mutex<HashMap<String, f64>>,
}

impl SessionBudget {
    pub fn new(limit: f64) -> Self {
        Self {
            limit,
            spent: Mutex::new(HashMap::new()),
        }
    }

    /// J11 default: $2.00/session.
    pub fn default_budget() -> Self {
        Self::new(DEFAULT_SESSION_BUDGET_USD)
    }

    /// The configured limit ($).
    pub fn limit(&self) -> f64 {
        self.limit
    }

    /// Total spent so far for `session`.
    pub fn spent(&self, session: &str) -> f64 {
        self.spent
            .lock()
            .expect("session budget poisoned")
            .get(session)
            .copied()
            .unwrap_or(0.0)
    }

    /// Remaining budget ($) for `session` (0 once at/over the limit).
    pub fn remaining(&self, session: &str) -> f64 {
        (self.limit - self.spent(session)).max(0.0)
    }

    /// May the session issue another call? `false` once spent ≥ limit — the
    /// kill-on-exceed boundary.
    pub fn can_issue(&self, session: &str) -> bool {
        self.spent(session) < self.limit
    }

    /// Accumulate a completed call's cost. Returns the new total spent.
    /// The session is now "dead" for further calls when the total ≥ limit.
    pub fn settle(&self, session: &str, cost: f64) -> f64 {
        let mut spent = self.spent.lock().expect("session budget poisoned");
        let total = spent.get(session).copied().unwrap_or(0.0) + cost;
        spent.insert(session.to_string(), total);
        total
    }

    /// How far over the limit the session is (0 when under).
    pub fn over_by(&self, session: &str) -> f64 {
        (self.spent(session) - self.limit).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_two_dollars() {
        assert_eq!(SessionBudget::default_budget().limit(), 2.00);
    }

    #[test]
    fn can_issue_until_limit_reached() {
        let b = SessionBudget::new(1.00);
        assert!(b.can_issue("s1"));
        b.settle("s1", 0.60);
        assert!(b.can_issue("s1"));
        assert_eq!(b.remaining("s1"), 0.40);
        b.settle("s1", 0.40); // exactly at limit
        assert!(!b.can_issue("s1"), "session at limit must be refused");
        assert_eq!(b.remaining("s1"), 0.0);
    }

    #[test]
    fn sessions_are_isolated() {
        let b = SessionBudget::new(1.00);
        b.settle("s1", 0.99);
        // Just under the limit: still allowed.
        assert!(b.can_issue("s1"));
        b.settle("s1", 0.02);
        // Over the limit: refused — but s2 is untouched.
        assert!(!b.can_issue("s1"));
        assert!(b.can_issue("s2"));
        assert_eq!(b.spent("s2"), 0.0);
    }

    #[test]
    fn over_budget_reported() {
        let b = SessionBudget::new(1.00);
        b.settle("s1", 1.25);
        assert_eq!(b.over_by("s1"), 0.25);
        assert!((b.remaining("s1") - 0.0).abs() < 1e-9);
    }
}
