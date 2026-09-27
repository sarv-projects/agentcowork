//! Cache-aware usage + cost ledger (P1.3, A9) — the token/cost accounting pillar.
//!
//! Everything the broker learns about a completed call lands in ONE append-only
//! table (`token_usage`, ARCH/05 §5.6) shared by per-key budgets (ARCH/03),
//! session efficiency projections, and the UI's live token/cost stream.
//!
//! ## AI SDK v6 cached-input normalization (the opencode gotcha)
//!
//! The AI SDK v6 normalizes `inputTokens` to **include** cached input. Costing
//! that input again at the full input rate double-bills it, so cached input is
//! subtracted back out before cost (ARCH/05 §5.6, doc 38). Providers report
//! cache tokens under different names:
//!
//! - OpenAI-compatible: `usage.prompt_tokens_details.cached_tokens`
//! - Anthropic: `usage.cache_read_input_tokens` / `usage.cache_creation_input_tokens`
//!
//! [`Usage::from_any`] reads every known shape so the broker never cares which
//! provider answered; [`Usage::billable_input`] applies the normalization.

use serde::{Deserialize, Serialize};

/// Token counts for one provider call, cache-aware (the pi `EMPTY_USAGE`
/// mirror, doc 05/38 — `{input, output, cache:{read, write}}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Input tokens **as reported by the provider** (may include cached input).
    pub prompt: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// Total reported tokens (mirrors provider `total_tokens` semantics —
    /// input as reported, i.e. cached input included).
    pub fn total(self) -> u64 {
        self.prompt.saturating_add(self.output)
    }

    /// AI SDK v6 cache normalization: billable input = reported input minus
    /// cached-read tokens (never underflows). This is what cost is computed on.
    pub fn billable_input(self) -> u64 {
        self.prompt.saturating_sub(self.cache_read)
    }

    /// Parse a provider usage object. Handles OpenAI-compatible and Anthropic
    /// field names, plus the `total_tokens`-only fallback. Returns `None` when
    /// the value carries no token fields (e.g. a non-usage chunk).
    pub fn from_any(value: &serde_json::Value) -> Option<Usage> {
        let prompt = get_u64(value, &["prompt_tokens", "input_tokens"]);
        let output = get_u64(value, &["completion_tokens", "output_tokens"]);
        let cache_read = value
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|t| t.as_u64())
            .or_else(|| Some(get_u64(value, &["cache_read_input_tokens"])))
            .unwrap_or(0);
        let cache_write = get_u64(value, &["cache_creation_input_tokens"]);

        // `total_tokens`-only fallback (some OpenAI-compatible endpoints only
        // echo a total): count it as input so budgets never under-report.
        let (prompt, output) = if prompt == 0 && output == 0 {
            let total = get_u64(value, &["total_tokens"]);
            (total, 0)
        } else {
            (prompt, output)
        };

        if prompt == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
            None
        } else {
            Some(Usage {
                prompt,
                output,
                cache_read,
                cache_write,
            })
        }
    }

    /// Merge a stream-observed usage into an accumulator (max per field — an
    /// OpenAI stream's final chunk carries the full usage once; Anthropic
    /// splits input/cache-write into `message_start` and output into
    /// `message_delta`, so later chunks must not zero earlier ones).
    pub fn merge_max(&mut self, other: Usage) {
        self.prompt = self.prompt.max(other.prompt);
        self.output = self.output.max(other.output);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.cache_write = self.cache_write.max(other.cache_write);
    }
}

fn get_u64(value: &serde_json::Value, keys: &[&str]) -> u64 {
    for k in keys {
        if let Some(v) = value.get(k).and_then(|t| t.as_u64()) {
            return v;
        }
    }
    0
}

/// Per-1M-token pricing for one provider (A9 — cache-aware $ per call).
///
/// `input_per_m` prices **billable** input (after the cached-input
/// subtraction); `cache_read_per_m`/`cache_write_per_m` price the cache
/// economy separately. Defaults are approximate public list prices (Aug 2026)
/// and are overridable per provider via [`crate::broker::Broker::with_pricing`]
/// (the "core-providers live-pricing" fetch is a later enhancement — J11).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input_per_m: f64,
    pub output_per_m: f64,
    pub cache_read_per_m: f64,
    pub cache_write_per_m: f64,
}

impl Pricing {
    /// Generic OpenAI-compatible default (used for unknown providers so cost
    /// is never silently zero).
    pub const fn default_pricing() -> Self {
        Self {
            input_per_m: 2.50,
            output_per_m: 10.00,
            cache_read_per_m: 1.25,
            cache_write_per_m: 2.50,
        }
    }

    /// $ cost of one call, honoring the cached-input normalization: only
    /// `billable_input` is charged at the input rate.
    pub fn cost_of(self, usage: Usage) -> f64 {
        usage.billable_input() as f64 / 1e6 * self.input_per_m
            + usage.output as f64 / 1e6 * self.output_per_m
            + usage.cache_read as f64 / 1e6 * self.cache_read_per_m
            + usage.cache_write as f64 / 1e6 * self.cache_write_per_m
    }
}

impl Default for Pricing {
    fn default() -> Self {
        Self::default_pricing()
    }
}

/// Default list pricing for the brokers's known providers ($ per 1M tokens).
/// Approximate public rates (Aug 2026); overridable — never authoritative.
pub fn default_pricing(provider: &str) -> Option<Pricing> {
    match provider {
        // gpt-4o class
        "openai" => Some(Pricing {
            input_per_m: 2.50,
            output_per_m: 10.00,
            cache_read_per_m: 1.25,
            cache_write_per_m: 2.50,
        }),
        // claude-3.5-sonnet class (cache read is the big discount; write = input)
        "anthropic" => Some(Pricing {
            input_per_m: 3.00,
            output_per_m: 15.00,
            cache_read_per_m: 0.30,
            cache_write_per_m: 3.75,
        }),
        // deepseek-chat (cache hit price is the headline number)
        "deepseek" => Some(Pricing {
            input_per_m: 0.27,
            output_per_m: 1.10,
            cache_read_per_m: 0.07,
            cache_write_per_m: 0.27,
        }),
        // groq llama-3.3-70b class (no server-side cache pricing)
        "groq" => Some(Pricing {
            input_per_m: 0.59,
            output_per_m: 0.79,
            cache_read_per_m: 0.0,
            cache_write_per_m: 0.59,
        }),
        // nvidia NIM default
        "nvidia" => Some(Pricing {
            input_per_m: 0.50,
            output_per_m: 0.50,
            cache_read_per_m: 0.0,
            cache_write_per_m: 0.50,
        }),
        _ => None,
    }
}

/// One append-only `token_usage` ledger row (ARCH/05 §5.6, ARCH/03 §3.4).
///
/// P51.12 cost-carrier scope: `task_id`/`run_id`/`work_id` bind the row to the
/// detached-work join (`Vault::usage_for_task` → `Vault::task_cost` →
/// `TaskRecord::attach_cost`). Unscoped rows (broker turns with no task
/// context) store `""` — the pre-P51.12 migration backfills `""` so old rows
/// stay valid and read back with empty scope.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub session: String,
    pub provider: String,
    pub model: String,
    /// The key's label (not the opaque handle — the ledger is for analytics,
    /// and the handle adds no information).
    pub key_id: String,
    pub usage: Usage,
    pub cost: f64,
    /// Optional tool id for tool calls (nullable in the schema).
    pub tool: Option<String>,
    /// Detached-work task this spend belongs to (`""` = unscoped).
    pub task_id: String,
    /// One execution attempt of the task (`""` = unscoped).
    pub run_id: String,
    /// Work-gateway work item (`""` = unscoped).
    pub work_id: String,
}

/// One per-session aggregate row for the analytics cost table (P5.9) — the
/// `token_usage` ledger grouped by session, ordered by cost descending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTotal {
    pub session: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
}

/// One raw `token_usage` ledger row (newest-first) — the **durable
/// observation feed** for the coordinator's RouteDecision scorer (ARCH/05
/// seam: `token_usage` → `ProviderObservation`, restart/offline survival).
/// Latency is not recorded by the broker, so the coordinator hydrates
/// latency from the live ring where present and treats ledger-only rows as
/// latency-unknown (0). `cost` is total $ for the call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentUsage {
    pub ts_ms: u64,
    pub provider: String,
    pub model: String,
    pub in_tokens: u64,
    pub out_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_cached_input_is_normalized_out_of_cost() {
        let usage = Usage::from_any(&serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "prompt_tokens_details": { "cached_tokens": 80 },
        }))
        .unwrap();
        assert_eq!(usage.prompt, 100);
        assert_eq!(usage.output, 50);
        assert_eq!(usage.cache_read, 80);
        // The opencode gotcha: billable input excludes the cached 80.
        assert_eq!(usage.billable_input(), 20);
        // Input charged on 20, not 100 — otherwise cached input is double-billed.
        let p = Pricing {
            input_per_m: 1.0,
            output_per_m: 1.0,
            cache_read_per_m: 0.1,
            cache_write_per_m: 1.0,
        };
        let cost = p.cost_of(usage);
        let expected = 20e-6 + 50e-6 + 8e-6;
        assert!((cost - expected).abs() < 1e-12, "cost {cost} != {expected}");
    }

    #[test]
    fn anthropic_cache_fields_parsed() {
        let usage = Usage::from_any(&serde_json::json!({
            "input_tokens": 200,
            "output_tokens": 30,
            "cache_creation_input_tokens": 150,
            "cache_read_input_tokens": 40,
        }))
        .unwrap();
        assert_eq!(usage.prompt, 200);
        assert_eq!(usage.output, 30);
        assert_eq!(usage.cache_write, 150);
        assert_eq!(usage.cache_read, 40);
    }

    #[test]
    fn total_tokens_only_fallback_counts_as_input() {
        let usage = Usage::from_any(&serde_json::json!({ "total_tokens": 37 })).unwrap();
        assert_eq!(usage.prompt, 37);
        assert_eq!(usage.total(), 37);
        assert_eq!(usage.billable_input(), 37);
    }

    #[test]
    fn non_usage_value_returns_none() {
        assert!(Usage::from_any(&serde_json::json!({ "choices": [] })).is_none());
    }

    #[test]
    fn merge_max_keeps_late_partial_usage() {
        // Anthropic streaming: message_start (input+cache_write) then
        // message_delta (output) — merging must preserve all three.
        let start = Usage::from_any(&serde_json::json!({
            "input_tokens": 300, "cache_creation_input_tokens": 250
        }))
        .unwrap();
        let delta = Usage::from_any(&serde_json::json!({ "output_tokens": 90 })).unwrap();
        let mut acc = Usage::default();
        acc.merge_max(start);
        acc.merge_max(delta);
        assert_eq!(acc.prompt, 300);
        assert_eq!(acc.output, 90);
        assert_eq!(acc.cache_write, 250);
    }

    #[test]
    fn default_pricing_known_providers() {
        assert!(default_pricing("openai").is_some());
        assert!(default_pricing("anthropic").is_some());
        assert!(default_pricing("deepseek").is_some());
        assert!(default_pricing("unknown").is_none());
    }

    #[test]
    fn cache_read_is_cheaper_than_input() {
        let p = default_pricing("anthropic").unwrap();
        assert!(p.cache_read_per_m < p.input_per_m);
        assert!(p.cache_write_per_m >= p.input_per_m);
    }
}
