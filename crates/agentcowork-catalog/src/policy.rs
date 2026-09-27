//! P51.3 — provider use-policy + model effort variants.
//!
//! The picker-level gate above routing: a [`UsePolicy`] narrows which
//! canonical providers may serve a request (empty `allow` = everything is
//! allowed), and [`ModelVariant`]/[`ModelEffort`] is the Low/Medium/High
//! effort cycler the UI steps through per model. Pure + testable: no
//! registry access here — callers resolve ids via [`crate::provider`].

use serde::{Deserialize, Serialize};

use crate::provider::normalize;

/// Which canonical providers may serve a request (P51.3).
///
/// `allow` holds canonical provider ids; comparison is case-normalized via
/// [`normalize`] (the P44.2 `ALIASES` discipline: `OpenAI`, `openai`,
/// `openai_api`-style spellings all match `openai`). An empty `allow` means
/// "no restriction" — everything is allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsePolicy {
    #[serde(default)]
    pub allow: Vec<String>,
}

impl UsePolicy {
    pub fn new(allow: Vec<String>) -> Self {
        Self { allow }
    }

    /// True when `id` may serve under this policy. Empty `allow` permits all.
    pub fn allowed(&self, id: &str) -> bool {
        if self.allow.is_empty() {
            return true;
        }
        let norm = normalize(id);
        self.allow.iter().any(|a| normalize(a) == norm)
    }
}

/// The effort tier of one model variant (the UI's Low/Medium/High stepper).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelEffort {
    #[default]
    Low,
    Medium,
    High,
}

/// One model + its selected effort tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelVariant {
    pub id: String,
    pub effort: ModelEffort,
}

impl ModelVariant {
    pub fn new(id: String, effort: ModelEffort) -> Self {
        Self { id, effort }
    }
}

/// Step the effort cycler once: Low → Medium → High → Low.
pub fn cycle_variant(current: &ModelEffort) -> ModelEffort {
    match current {
        ModelEffort::Low => ModelEffort::Medium,
        ModelEffort::Medium => ModelEffort::High,
        ModelEffort::High => ModelEffort::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_policy_excludes_denied() {
        let p = UsePolicy::new(vec!["openai".to_string()]);
        assert!(p.allowed("openai"));
        // Case-normalized like ALIASES (P44.2).
        assert!(p.allowed("OpenAI"));
        assert!(!p.allowed("anthropic"));
        assert!(!p.allowed("openai-api"));
    }

    #[test]
    fn empty_allow_permits_all() {
        let p = UsePolicy::default();
        assert!(p.allowed("openai"));
        assert!(p.allowed("anthropic"));
        assert!(p.allowed("anything-at-all"));
    }

    #[test]
    fn variant_cycles_low_med_high() {
        assert_eq!(cycle_variant(&ModelEffort::Low), ModelEffort::Medium);
        assert_eq!(cycle_variant(&ModelEffort::Medium), ModelEffort::High);
        assert_eq!(cycle_variant(&ModelEffort::High), ModelEffort::Low);
        // A full turn returns to the start.
        let mut e = ModelEffort::Low;
        for _ in 0..3 {
            e = cycle_variant(&e);
        }
        assert_eq!(e, ModelEffort::Low);
    }
}
