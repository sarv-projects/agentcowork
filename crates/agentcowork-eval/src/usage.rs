//! A9 usage-parser registry + J11 efficiency metrics (doc 65 §1 — codeburn).
//!
//! Every provider reports usage differently (token buckets, cache counters,
//! tool-call accounting). [`UsageParser`] normalizes any provider's raw usage
//! payload into one canonical [`Usage`] shape — the same adapter split the
//! provider layer uses, but for cost accounting. A registry keyed by
//! provider id hands the coordinator the right parser at runtime.
//!
//! Every turn also gets a [`TurnClass`] (test/git/build/install/debug/
//! feature/refactor/brainstorm/research) so routing + eval segmentation can
//! slice the run by *kind of work*, not just by agent.
//!
//! J11 sits on top: [`EfficiencyMetrics`] aggregates a run's turns into the
//! cost-vs-quality axis the budget gate consumes (`one_shot_rate`,
//! `retries_per_edit`, `cost_per_edit`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Canonical usage — the normalized shape every provider parser must
/// produce. Mirrors the broker's token buckets plus the tool-call counter
/// (the J11 denominator for `cost_per_edit`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tool_calls: u64,
}

impl Usage {
    pub fn total_tokens(self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// One provider's raw usage payload → canonical [`Usage`]. Implementations
/// are pure (no IO): they parse whatever JSON/struct the provider emits.
pub trait UsageParser: Send + Sync {
    /// Parse a provider-specific payload into canonical [`Usage`]. Returns
    /// `None` when the payload isn't a usage payload (e.g. an event of
    /// another kind) or can't be understood.
    fn parse(&self, raw: &serde_json::Value) -> Option<Usage>;
    /// The provider id this parser serves.
    fn provider(&self) -> &str;
}

/// The tolerant default: reads common token/cache field names from JSON,
/// tolerating missing keys (Anthropic-style `input_tokens`/`output_tokens`,
/// OpenAI-style `prompt_tokens`/`completion_tokens`, cache fields under
/// either name). This is the parser used when a provider has no bespoke one.
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericUsageParser;

impl UsageParser for GenericUsageParser {
    fn provider(&self) -> &str {
        "*"
    }

    fn parse(&self, raw: &serde_json::Value) -> Option<Usage> {
        let obj = raw.as_object()?;
        let input = num(obj, &["input_tokens", "prompt_tokens", "input"])
            .or_else(|| nested(obj, "usage", &["input_tokens", "prompt_tokens"]))?;
        let output = num(obj, &["output_tokens", "completion_tokens", "output"])
            .or_else(|| nested(obj, "usage", &["output_tokens", "completion_tokens"]))
            .unwrap_or(0);
        let cache_read = num(obj, &["cache_read_input_tokens", "cached_tokens"])
            .or_else(|| nested(obj, "usage", &["cache_read_input_tokens", "cached_tokens"]))
            .unwrap_or(0);
        let cache_write = num(obj, &["cache_creation_input_tokens"])
            .or_else(|| nested(obj, "usage", &["cache_creation_input_tokens"]))
            .unwrap_or(0);
        let tool_calls = num(obj, &["tool_calls", "toolCalls"])
            .or_else(|| {
                // `toolCalls` may be an array — count the entries.
                obj.get("toolCalls")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len() as u64)
            })
            .or_else(|| {
                // Some payloads nest under `message.usage`.
                obj.get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|v| v.as_u64())
            })
            .unwrap_or(0);
        Some(Usage {
            input,
            output,
            cache_read,
            cache_write,
            tool_calls,
        })
    }
}

fn num(obj: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|k| obj.get(*k).and_then(|v| v.as_u64()))
}

fn nested(
    obj: &serde_json::Map<String, serde_json::Value>,
    sub: &str,
    keys: &[&str],
) -> Option<u64> {
    obj.get(sub)
        .and_then(|v| v.as_object())
        .and_then(|o| num(o, keys))
}

/// Registry of [`UsageParser`]s keyed by provider id. Falls back to the
/// generic parser for unknown providers (never fails a cost display).
#[derive(Default)]
pub struct UsageParserRegistry {
    parsers: BTreeMap<String, Box<dyn UsageParser>>,
}

impl UsageParserRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a parser for its provider id (replaces any existing one).
    pub fn register(&mut self, parser: Box<dyn UsageParser>) {
        self.parsers.insert(parser.provider().to_string(), parser);
    }

    /// Parse a payload for `provider`. Unknown providers use the generic
    /// parser; unparseable payloads yield `None`.
    pub fn parse(&self, provider: &str, raw: &serde_json::Value) -> Option<Usage> {
        match self.parsers.get(provider) {
            Some(p) => p.parse(raw),
            None => GenericUsageParser.parse(raw),
        }
    }

    pub fn providers(&self) -> impl Iterator<Item = &str> {
        self.parsers.keys().map(|s| s.as_str())
    }
}

/// The work-class of a turn, attached for routing + eval segmentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnClass {
    Test,
    Git,
    Build,
    Install,
    Debug,
    Feature,
    Refactor,
    Brainstorm,
    Research,
}

impl TurnClass {
    /// Deterministic keyword classifier over the turn's text (prompt +
    /// tool calls). Keyword hits are scored; the highest wins; ties fall to
    /// the first in declaration order (stable).
    pub fn classify(text: &str) -> TurnClass {
        let t = text.to_lowercase();
        let mut best = TurnClass::Feature;
        let mut best_score = 0;
        for (cls, words) in KEYWORDS {
            let score = words.iter().filter(|w| t.contains(**w)).count();
            if score > best_score {
                best = *cls;
                best_score = score;
            }
        }
        best
    }
}

impl FromStr for TurnClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "test" => Ok(TurnClass::Test),
            "git" => Ok(TurnClass::Git),
            "build" => Ok(TurnClass::Build),
            "install" => Ok(TurnClass::Install),
            "debug" => Ok(TurnClass::Debug),
            "feature" => Ok(TurnClass::Feature),
            "refactor" => Ok(TurnClass::Refactor),
            "brainstorm" => Ok(TurnClass::Brainstorm),
            "research" => Ok(TurnClass::Research),
            _ => Err(format!("unknown turn class `{s}`")),
        }
    }
}

/// Keyword sets per class, in declaration-priority order.
const KEYWORDS: &[(TurnClass, &[&str])] = &[
    (
        TurnClass::Test,
        &["test", "assert", "spec", "expect(", "pytest", "cargo test"],
    ),
    (
        TurnClass::Install,
        &["install", "npm i", "cargo add", "pip install", "setup"],
    ),
    (
        TurnClass::Git,
        &["git ", "commit", "rebase", "merge", "branch"],
    ),
    (
        TurnClass::Build,
        &[
            "build",
            "compile",
            "bundle",
            "tsc",
            "cargo build",
            "typecheck",
        ],
    ),
    (
        TurnClass::Debug,
        &["debug", "error", "panic", "crash", "traceback", "log"],
    ),
    (
        TurnClass::Refactor,
        &["refactor", "rename", "extract", "dedupe", "cleanup"],
    ),
    (
        TurnClass::Brainstorm,
        &["brainstorm", "idea", "proposal", "design", "option"],
    ),
    (
        TurnClass::Research,
        &["research", "search", "find", "investigate", "docs", "read"],
    ),
];

/// J11 efficiency metrics over an eval run (doc 65 §1). The cost-vs-quality
/// axis for the budget gate: a run that edits once and passes is cheap; one
/// that burns edits and tokens on retries is not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EfficiencyMetrics {
    /// Fraction of edits that passed on the first attempt (no retry).
    pub one_shot_rate: f64,
    /// Average edit attempts per successful edit.
    pub retries_per_edit: f64,
    /// Estimated USD per successful edit (cost / edits, at given prices).
    pub cost_per_edit: f64,
}

impl EfficiencyMetrics {
    /// Compute over a run. `turns` is every turn (usage + class + whether
    /// the edit passed on its first attempt); `edits` is the count of edits
    /// that ultimately succeeded.
    pub fn compute(
        turns: &[TurnStat],
        edits: u64,
        input_per_mtok: f64,
        output_per_mtok: f64,
    ) -> Self {
        if turns.is_empty() || edits == 0 {
            return Self::default();
        }
        let mut tokens_in = 0u64;
        let mut tokens_out = 0u64;
        let mut one_shot = 0u64;
        let mut attempts = 0u64;
        for t in turns {
            tokens_in += t.usage.input + t.usage.cache_read;
            tokens_out += t.usage.output;
            if t.kind == TurnKind::EditAttempt {
                attempts += 1;
                if t.first_attempt_passed {
                    one_shot += 1;
                }
            }
        }
        let cost =
            tokens_in as f64 / 1e6 * input_per_mtok + tokens_out as f64 / 1e6 * output_per_mtok;
        let one_shot_rate = if attempts == 0 {
            0.0
        } else {
            one_shot as f64 / attempts as f64
        };
        let retries_per_edit = if edits == 0 {
            0.0
        } else {
            attempts as f64 / edits as f64
        };
        Self {
            one_shot_rate,
            retries_per_edit,
            cost_per_edit: if edits == 0 { 0.0 } else { cost / edits as f64 },
        }
    }
}

/// One turn's contribution to the efficiency computation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TurnStat {
    pub usage: Usage,
    pub kind: TurnKind,
    /// For `EditAttempt` turns: whether the edit passed verification on the
    /// first attempt (no retry followed).
    pub first_attempt_passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    /// A normal LLM turn.
    Llm,
    /// An edit attempt (the J11 denominator input).
    EditAttempt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_parser_handles_nested_and_flat_shapes() {
        let flat = serde_json::json!({
            "input_tokens": 100, "output_tokens": 50,
            "cache_read_input_tokens": 20, "cache_creation_input_tokens": 10,
            "tool_calls": 3
        });
        let u = GenericUsageParser.parse(&flat).unwrap();
        assert_eq!(
            u,
            Usage {
                input: 100,
                output: 50,
                cache_read: 20,
                cache_write: 10,
                tool_calls: 3
            }
        );

        let nested = serde_json::json!({
            "usage": { "prompt_tokens": 7, "completion_tokens": 3, "cached_tokens": 2 },
            "toolCalls": [1, 2]
        });
        let u = GenericUsageParser.parse(&nested).unwrap();
        assert_eq!(u.input, 7);
        assert_eq!(u.output, 3);
        assert_eq!(u.cache_read, 2);
        assert_eq!(u.tool_calls, 2);
    }

    #[test]
    fn registry_falls_back_to_generic() {
        let mut reg = UsageParserRegistry::new();
        reg.register(Box::new(GenericUsageParser));
        let u = reg
            .parse(
                "anthropic",
                &serde_json::json!({ "input_tokens": 1, "output_tokens": 1 }),
            )
            .unwrap();
        assert_eq!(u.total_tokens(), 2);
    }

    #[test]
    fn turn_class_keyword_classification() {
        assert_eq!(
            TurnClass::classify("add unit tests for the parser"),
            TurnClass::Test
        );
        assert_eq!(
            TurnClass::classify("rebase the feature branch onto main"),
            TurnClass::Git
        );
        assert_eq!(
            TurnClass::classify("fix the panic in the transport"),
            TurnClass::Debug
        );
        assert_eq!(
            TurnClass::classify("brainstorm naming options for the app"),
            TurnClass::Brainstorm
        );
    }

    #[test]
    fn efficiency_metrics_compute() {
        let turns = vec![
            TurnStat {
                usage: Usage {
                    input: 100,
                    output: 20,
                    ..Default::default()
                },
                kind: TurnKind::EditAttempt,
                first_attempt_passed: true,
            },
            TurnStat {
                usage: Usage {
                    input: 40,
                    output: 10,
                    ..Default::default()
                },
                kind: TurnKind::EditAttempt,
                first_attempt_passed: false,
            },
        ];
        let m = EfficiencyMetrics::compute(&turns, 1, 3.0, 15.0);
        assert_eq!(m.one_shot_rate, 0.5);
        assert_eq!(m.retries_per_edit, 2.0);
        // 140 in-tokens @ $3/M + 30 out @ $15/M = $0.00042 + $0.00045 = $0.00087
        assert!((m.cost_per_edit - 0.00087).abs() < 1e-9);
    }
}
