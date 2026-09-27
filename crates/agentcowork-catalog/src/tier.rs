//! Two-tier lab/provider schema (doc 66 §1.1): a canonical **lab model** (the
//! model family's true capabilities) + per-host **override-only** provider
//! entries. The blocker rule: *if the provider did not create the model, its
//! entry MUST use `base_model` and stays override-only* — cost/limits may be
//! overridden, canonical capability facts never duplicated.

use crate::model::ModelEntry;
use serde::{Deserialize, Serialize};

/// A provider override row — the *only* fields a non-creating provider may
/// set beyond `base_model`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderOverride {
    /// `provider/model` id of the override row.
    pub id: String,
    /// The canonical lab model this inherits from.
    pub base_model: String,
    /// Override-only cost (the provider's real price).
    pub pricing: crate::model::Pricing,
    /// Override-only limits.
    pub top_provider: crate::model::TopProvider,
    /// Optional status flag (deprecated/sunset) — still not a capability.
    #[serde(default)]
    pub deprecated: bool,
}

/// The resolved view of an entry after inheritance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedModel {
    /// The canonical lab model facts (capabilities + context).
    pub base: ModelEntry,
    /// The provider override that customized it (if any).
    pub override_: Option<ProviderOverride>,
}

impl ResolvedModel {
    /// The effective pricing: the override's when present, else the base's.
    pub fn pricing(&self) -> crate::model::Pricing {
        self.override_
            .as_ref()
            .map(|o| o.pricing)
            .unwrap_or(self.base.pricing)
    }

    /// The effective limits.
    pub fn top_provider(&self) -> crate::model::TopProvider {
        self.override_
            .as_ref()
            .map(|o| o.top_provider)
            .unwrap_or(self.base.top_provider)
    }

    /// Capability facts always come from the canonical base — an override
    /// can never claim a capability the lab model lacks.
    pub fn supports_tools(&self) -> bool {
        self.base.supports_tools()
    }
}

/// Validate the two-tier invariant over a catalog's entries: any entry that
/// did not create its model must declare `base_model` (the blocker rule),
/// and a `base_model` must resolve to an existing lab entry. Returns the
/// violations, never panics.
pub fn validate_tiers(entries: &[ModelEntry], lab_ids: &[&str]) -> Vec<String> {
    let mut violations = Vec::new();
    for e in entries {
        if e.base_model.is_none() && !lab_ids.contains(&e.id.as_str()) {
            violations.push(format!(
                "`{}` declares no base_model and is not a lab model (blocker rule)",
                e.id
            ));
        }
        if let Some(base) = &e.base_model {
            if !lab_ids.contains(&base.as_str()) {
                violations.push(format!("`{}` base_model `{base}` does not resolve", e.id));
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lab() -> ModelEntry {
        serde_json::from_value(serde_json::json!({
            "id": "anthropic/claude-opus-4-6",
            "name": "Opus",
            "context_length": 200000,
            "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
            "pricing": { "prompt": 0.000015, "completion": 0.000075 },
            "top_provider": { "context_length": 200000, "max_completion_tokens": 32000 },
            "supported_parameters": { "tools": true }
        }))
        .unwrap()
    }

    #[test]
    fn resolve_inherits_and_overrides() {
        let base = lab();
        let over = ProviderOverride {
            id: "bedrock/claude-opus-4-6".into(),
            base_model: "anthropic/claude-opus-4-6".into(),
            pricing: crate::model::Pricing {
                prompt: 0.00002,
                completion: 0.0001,
                ..Default::default()
            },
            top_provider: crate::model::TopProvider {
                max_completion_tokens: 16000,
                ..Default::default()
            },
            deprecated: false,
        };
        let r = ResolvedModel {
            base,
            override_: Some(over),
        };
        assert_eq!(r.pricing().prompt, 0.00002); // override wins
        assert_eq!(r.top_provider().max_completion_tokens, 16000);
        assert!(r.supports_tools()); // capability from the base
    }

    #[test]
    fn blocker_rule_validation() {
        let lab_entry = lab();
        let missing = ModelEntry {
            id: "bedrock/claude-opus-4-6".into(), // a non-creating provider with no base_model
            base_model: None,
            ..lab()
        };
        let dangling = ModelEntry {
            base_model: Some("ghost/x".into()),
            ..lab()
        };
        let ok = ModelEntry {
            base_model: Some("anthropic/claude-opus-4-6".into()),
            ..lab()
        };
        let v = validate_tiers(
            &[lab_entry, missing, dangling, ok],
            &["anthropic/claude-opus-4-6"],
        );
        assert!(
            v.iter()
                .any(|s| s.contains("bedrock/claude-opus-4-6") && s.contains("no base_model"))
        );
        assert!(v.iter().any(|s| s.contains("ghost/x")));
    }
}
