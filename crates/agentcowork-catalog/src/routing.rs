//! A7 — the routing filter matrix (doc 66 §1.3): `supported_parameters` +
//! `architecture` modalities + `context_length`/`max_completion_tokens` are
//! the **hard-requirement filters** for route selection. A candidate that
//! fails any required filter is out — the router never soft-passes a
//! capability the model lacks.

use crate::model::ModelEntry;

/// The hard requirements a route candidate must satisfy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteFilters {
    /// The model must support tools.
    pub requires_tools: bool,
    /// The model must support structured outputs.
    pub requires_structured_outputs: bool,
    /// The model must support reasoning.
    pub requires_reasoning: bool,
    /// Minimum context length (tokens).
    pub min_context: u64,
    /// Minimum max-completion-tokens.
    pub min_max_completion: u64,
    /// Input modalities the request needs (e.g. `image`).
    pub input_modalities: Vec<String>,
    /// Output modalities the request needs.
    pub output_modalities: Vec<String>,
}

impl RouteFilters {
    /// Does this candidate satisfy every hard requirement?
    pub fn matches(&self, m: &ModelEntry) -> bool {
        if self.requires_tools && !m.supports_tools() {
            return false;
        }
        if self.requires_structured_outputs && !m.supports_structured_outputs() {
            return false;
        }
        if self.requires_reasoning && !m.supports_reasoning() {
            return false;
        }
        if m.effective_context() < self.min_context {
            return false;
        }
        if self.min_max_completion > 0 && m.max_completion_tokens() < self.min_max_completion {
            return false;
        }
        for mod_ in &self.input_modalities {
            if !m.accepts_input_modality(mod_) {
                return false;
            }
        }
        for mod_ in &self.output_modalities {
            if !m.produces_output_modality(mod_) {
                return false;
            }
        }
        true
    }

    /// The surviving candidates (the router's hard-filter pass).
    pub fn filter<'a>(
        &self,
        candidates: impl Iterator<Item = &'a ModelEntry>,
    ) -> Vec<&'a ModelEntry> {
        candidates.filter(|m| self.matches(m)).collect()
    }
}

/// The failure reasons for one candidate (the router's honesty surface —
/// why each model was excluded).
pub fn rejection_reasons(filters: &RouteFilters, m: &ModelEntry) -> Vec<String> {
    let mut reasons = Vec::new();
    if filters.requires_tools && !m.supports_tools() {
        reasons.push("no tools".into());
    }
    if filters.requires_structured_outputs && !m.supports_structured_outputs() {
        reasons.push("no structured outputs".into());
    }
    if filters.requires_reasoning && !m.supports_reasoning() {
        reasons.push("no reasoning".into());
    }
    if m.effective_context() < filters.min_context {
        reasons.push(format!(
            "context {} < {min}",
            m.effective_context(),
            min = filters.min_context
        ));
    }
    for mod_ in &filters.input_modalities {
        if !m.accepts_input_modality(mod_) {
            reasons.push(format!("no input modality `{mod_}`"));
        }
    }
    for mod_ in &filters.output_modalities {
        if !m.produces_output_modality(mod_) {
            reasons.push(format!("no output modality `{mod_}`"));
        }
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        tools: bool,
        structured: bool,
        reasoning: bool,
        context: u64,
        max_out: u64,
        input: &[&str],
    ) -> ModelEntry {
        serde_json::from_value(serde_json::json!({
            "id": "p/m",
            "context_length": context,
            "architecture": { "input_modalities": input, "output_modalities": ["text"] },
            "top_provider": { "context_length": context, "max_completion_tokens": max_out },
            "supported_parameters": { "tools": tools, "structured_outputs": structured, "reasoning": reasoning }
        })).unwrap()
    }

    #[test]
    fn hard_filters_exclude_cleanly() {
        let capable = entry(true, true, true, 200_000, 32_000, &["text", "image"]);
        let no_tools = entry(false, true, true, 200_000, 32_000, &["text"]);
        let small_ctx = entry(true, true, true, 16_000, 4_000, &["text"]);

        let f = RouteFilters {
            requires_tools: true,
            requires_structured_outputs: true,
            requires_reasoning: true,
            min_context: 64_000,
            min_max_completion: 8_000,
            input_modalities: vec!["image".into()],
            output_modalities: vec!["text".into()],
        };
        assert!(f.matches(&capable));
        assert!(!f.matches(&no_tools));
        assert!(!f.matches(&small_ctx));
        assert_eq!(
            f.filter(vec![&capable, &no_tools, &small_ctx].into_iter())
                .len(),
            1
        );
        assert!(rejection_reasons(&f, &no_tools).contains(&"no tools".to_string()));
    }
}
