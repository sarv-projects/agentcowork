//! P68 Adversarial Failure Refinement — Action avoidance rules to prevent repetitive error loops
//!
//! Inspired by SEAgent/EvoCUA failure imitation and hermes-agent learning:
//! When an agent action (CUA coordinate click, shell command, tool argument)
//! fails or triggers an error, an `AvoidRule` is recorded with contextual tags
//! and root cause.
//!
//! On future turns, `should_avoid()` checks candidate actions against active
//! avoidance rules to break infinite failure loops.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvoidRule {
    pub id: String,
    pub action_pattern: String,
    pub context_pattern: String,
    pub root_cause: String,
    pub fail_count: u32,
    pub created_at: i64,
}

pub struct AvoidanceStore {
    rules: Vec<AvoidRule>,
    max_rules: usize,
}

impl Default for AvoidanceStore {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            max_rules: 100,
        }
    }
}

impl AvoidanceStore {
    pub fn new(max_rules: usize) -> Self {
        Self {
            rules: Vec::new(),
            max_rules,
        }
    }

    /// Record a failed action into the avoidance store.
    pub fn record_failure(&mut self, action: &str, context: &str, root_cause: &str, now_ms: i64) {
        if let Some(existing) = self
            .rules
            .iter_mut()
            .find(|r| r.action_pattern == action && r.context_pattern == context)
        {
            existing.fail_count += 1;
            existing.root_cause = root_cause.to_string();
            return;
        }

        if self.rules.len() >= self.max_rules {
            // Drop oldest rule with lowest fail count
            self.rules.sort_by_key(|r| r.fail_count);
            self.rules.remove(0);
        }

        self.rules.push(AvoidRule {
            id: format!("avoid-{}", self.rules.len() + 1),
            action_pattern: action.to_string(),
            context_pattern: context.to_string(),
            root_cause: root_cause.to_string(),
            fail_count: 1,
            created_at: now_ms,
        });
    }

    /// Check if a candidate action matches any active avoidance rule in the current context.
    pub fn should_avoid(
        &self,
        candidate_action: &str,
        current_context: &str,
    ) -> Option<&AvoidRule> {
        self.rules.iter().find(|r| {
            (candidate_action.contains(&r.action_pattern)
                || r.action_pattern.contains(candidate_action))
                && (current_context.contains(&r.context_pattern)
                    || r.context_pattern.contains(current_context))
        })
    }

    pub fn rules(&self) -> &[AvoidRule] {
        &self.rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_avoidance_store_records_and_matches() {
        let mut store = AvoidanceStore::new(10);
        store.record_failure(
            "click_element_before_load",
            "page_state_loading",
            "Element not yet attached to DOM",
            1000,
        );

        let hit = store.should_avoid("click_element_before_load", "page_state_loading");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().root_cause, "Element not yet attached to DOM");

        let miss = store.should_avoid("click_element_before_load", "page_state_ready");
        assert!(miss.is_none());
    }
}
