//! The in-memory catalog index: parsed once at startup from the vendored
//! `models.json` (doc 66 §1.3), serving lookup + provider enumeration +
//! capability queries. Immutable after load — routing reads never mutate.

use crate::model::ModelEntry;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    by_id: BTreeMap<String, ModelEntry>,
}

impl ModelCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a vendored `models.json` (an array of compiled entries).
    pub fn parse(source: &str) -> Result<Self, String> {
        let entries: Vec<ModelEntry> =
            serde_json::from_str(source).map_err(|e| format!("catalog parse: {e}"))?;
        let mut by_id = BTreeMap::new();
        for e in entries {
            if e.id.is_empty() {
                continue; // a malformed row must not hide the rest
            }
            by_id.insert(e.id.clone(), e);
        }
        Ok(Self { by_id })
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Look up an entry by its `provider/model` id.
    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.by_id.get(id)
    }

    /// Look up by canonical slug (the same string in most cases).
    pub fn by_slug(&self, slug: &str) -> Option<&ModelEntry> {
        self.by_id.values().find(|m| m.canonical_slug == slug)
    }

    /// Every model id for a provider (the picker's per-provider list).
    pub fn models_for_provider(&self, provider: &str) -> Vec<&ModelEntry> {
        self.by_id
            .values()
            .filter(|m| m.provider() == provider)
            .collect()
    }

    pub fn providers(&self) -> Vec<&str> {
        let mut set: Vec<&str> = self.by_id.values().map(|m| m.provider()).collect();
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Every entry (for routing + the cost engine).
    pub fn all(&self) -> impl Iterator<Item = &ModelEntry> {
        self.by_id.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> String {
        serde_json::json!([
            { "id": "anthropic/claude-opus-4-6", "canonical_slug": "anthropic/claude-opus-4-6", "name": "Opus", "context_length": 200000, "supported_parameters": { "tools": true }, "pricing": { "prompt": 1e-5, "completion": 1e-4 } },
            { "id": "anthropic/claude-haiku-4-5", "canonical_slug": "anthropic/claude-haiku-4-5", "name": "Haiku", "context_length": 200000, "supported_parameters": { "tools": true }, "pricing": { "prompt": 1e-6, "completion": 1e-5 } },
            { "id": "openai/gpt-5", "canonical_slug": "openai/gpt-5", "name": "GPT-5", "context_length": 128000, "supported_parameters": { "tools": true, "structured_outputs": true }, "pricing": { "prompt": 1e-5, "completion": 1e-4 } },
            { "id": "", "name": "broken" }
        ]).to_string()
    }

    #[test]
    fn parses_and_indexes() {
        let cat = ModelCatalog::parse(&sample_json()).unwrap();
        assert_eq!(cat.len(), 3); // the empty-id row is skipped
        assert!(cat.get("anthropic/claude-opus-4-6").is_some());
        assert_eq!(cat.models_for_provider("anthropic").len(), 2);
        assert_eq!(cat.providers(), vec!["anthropic", "openai"]);
        assert_eq!(cat.by_slug("openai/gpt-5").unwrap().model_name(), "gpt-5");
    }

    #[test]
    fn missing_id_returns_none() {
        let cat = ModelCatalog::parse(&sample_json()).unwrap();
        assert!(cat.get("nope/x").is_none());
    }

    #[test]
    fn malformed_source_is_an_error() {
        assert!(ModelCatalog::parse("not json").is_err());
    }
}
