//! A6 — the canonical `ModelEntry`, mirroring the models.dev compiled shape
//! (doc 66 §1.3): one row per provider/model with the capability + pricing
//! facts the router and cost engine need. Parsed once at startup from the
//! vendored `models.json` into the in-memory index.

use serde::{Deserialize, Serialize};

/// Per-token pricing. All fields are per-token USD strings (models.dev
/// shape); the cost engine parses them with [`crate::pricing`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Pricing {
    #[serde(default)]
    pub prompt: f64,
    #[serde(default)]
    pub completion: f64,
    #[serde(default)]
    pub web_search: f64,
    #[serde(default)]
    pub input_cache_read: f64,
    #[serde(default)]
    pub input_cache_write: f64,
}

/// Architecture facts (doc 66 §1.3 `architecture`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Architecture {
    #[serde(default)]
    pub modality: String,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    #[serde(default)]
    pub tokenizer: String,
    #[serde(default)]
    pub instruct_type: String,
}

/// The capability proxy (doc 66 §1.3 `supported_parameters`) — the A7
/// routing filter matrix inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedParameters {
    #[serde(default)]
    pub include_reasoning: bool,
    #[serde(default)]
    pub max_tokens: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub response_format: bool,
    #[serde(default)]
    pub stop: bool,
    #[serde(default)]
    pub structured_outputs: bool,
    #[serde(default)]
    pub tool_choice: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub verbosity: bool,
}

/// The top provider's limits (doc 66 §1.3 `top_provider`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopProvider {
    #[serde(default)]
    pub context_length: u64,
    #[serde(default)]
    pub max_completion_tokens: u64,
    #[serde(default)]
    pub is_moderated: bool,
}

/// One catalog entry (the compiled shape).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    /// `provider/model` (the unique id).
    pub id: String,
    #[serde(default)]
    pub canonical_slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub context_length: u64,
    #[serde(default)]
    pub architecture: Architecture,
    #[serde(default)]
    pub pricing: Pricing,
    #[serde(default)]
    pub top_provider: TopProvider,
    #[serde(default)]
    pub supported_parameters: SupportedParameters,
    #[serde(default)]
    pub default_parameters: serde_json::Value,
    /// The two-tier schema: `base_model` when the provider did NOT create
    /// the model (override-only inheritance, doc 66 §1.1 blocker rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_model: Option<String>,
    #[serde(default)]
    pub knowledge_cutoff: String,
}

impl ModelEntry {
    pub fn provider(&self) -> &str {
        self.id.split('/').next().unwrap_or("")
    }

    pub fn model_name(&self) -> &str {
        self.id.split('/').nth(1).unwrap_or("")
    }

    /// The effective context length (entry, falling back to top_provider).
    pub fn effective_context(&self) -> u64 {
        if self.context_length > 0 {
            self.context_length
        } else {
            self.top_provider.context_length
        }
    }

    /// The effective max completion tokens.
    pub fn max_completion_tokens(&self) -> u64 {
        self.top_provider.max_completion_tokens
    }

    /// Whether the model supports tools (A7 filter).
    pub fn supports_tools(&self) -> bool {
        self.supported_parameters.tools
    }

    /// Whether the model supports structured outputs (A7 filter).
    pub fn supports_structured_outputs(&self) -> bool {
        self.supported_parameters.structured_outputs
    }

    /// Whether the model supports reasoning (A7 filter).
    pub fn supports_reasoning(&self) -> bool {
        self.supported_parameters.reasoning
    }

    /// Whether the model can take the given input modality (A7 filter).
    pub fn accepts_input_modality(&self, modality: &str) -> bool {
        self.architecture
            .input_modalities
            .iter()
            .any(|m| m.eq_ignore_ascii_case(modality))
    }

    /// Whether the model can produce the given output modality.
    pub fn produces_output_modality(&self, modality: &str) -> bool {
        self.architecture
            .output_modalities
            .iter()
            .any(|m| m.eq_ignore_ascii_case(modality))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ModelEntry {
        serde_json::from_value(serde_json::json!({
            "id": "anthropic/claude-opus-4-6",
            "canonical_slug": "anthropic/claude-opus-4-6",
            "name": "Claude Opus 4.6",
            "context_length": 200000,
            "architecture": {
                "modality": "text+image->text",
                "input_modalities": ["text", "image"],
                "output_modalities": ["text"],
                "tokenizer": "Claude",
                "instruct_type": "claude-4"
            },
            "pricing": { "prompt": 0.000015, "completion": 0.000075, "input_cache_read": 0.0000015, "input_cache_write": 0.00003 },
            "top_provider": { "context_length": 200000, "max_completion_tokens": 32000, "is_moderated": false },
            "supported_parameters": { "tools": true, "structured_outputs": true, "reasoning": true, "tool_choice": true, "response_format": true },
            "knowledge_cutoff": "2026-06"
        })).unwrap()
    }

    #[test]
    fn parses_compiled_shape() {
        let m = sample();
        assert_eq!(m.provider(), "anthropic");
        assert_eq!(m.model_name(), "claude-opus-4-6");
        assert_eq!(m.effective_context(), 200_000);
        assert_eq!(m.max_completion_tokens(), 32_000);
        assert!(m.supports_tools());
        assert!(m.supports_structured_outputs());
        assert!(m.supports_reasoning());
        assert!(m.accepts_input_modality("image"));
        assert!(m.produces_output_modality("text"));
        assert_eq!(m.pricing.input_cache_read, 0.0000015);
    }

    #[test]
    fn falls_back_to_top_provider_context() {
        let mut m = sample();
        m.context_length = 0;
        assert_eq!(m.effective_context(), 200_000);
    }
}
