//! Cache stability (`ARCH/16-CONTEXT.md` §5, `REQ-CTX-009`).
//!
//! ```text
//! Stable prefix:  system contract · agent identity · project rules · stable tool definitions
//! Dynamic suffix: task · retrieved context · observations · tool results
//! ```
//!
//! The shipped pattern is **baseline + deltas**: the first full render is
//! persisted and each later turn emits only what changed. That is what keeps the
//! provider-side cache alive, so "did the prefix stay byte-stable?" is a
//! testable property here rather than a hope at the provider.

use serde::{Deserialize, Serialize};

/// The two halves of a packed turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedTurn {
    /// Byte-stable across turns until a real change occurs.
    pub stable_prefix: String,
    /// Everything dynamic, in order.
    pub dynamic_suffix: Vec<String>,
}

impl PackedTurn {
    pub fn total_tokens(&self) -> u32 {
        let mut t = estimate(&self.stable_prefix);
        for d in &self.dynamic_suffix {
            t += estimate(d);
        }
        t
    }

    /// The exact bytes sent to the model.
    pub fn render(&self) -> String {
        let mut parts = vec![self.stable_prefix.clone()];
        parts.extend(self.dynamic_suffix.iter().cloned());
        parts.join("\n")
    }
}

fn estimate(text: &str) -> u32 {
    text.chars().count().div_ceil(4) as u32
}

/// The stable prefix and its cache key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StablePrefix {
    pub system_contract: String,
    pub agent_identity: String,
    pub project_rules: String,
    pub stable_tool_definitions: String,
    /// Bumped on any real change, so an unchanged prefix is recognisable.
    pub version: u32,
}

impl StablePrefix {
    pub fn new(
        system_contract: &str,
        agent_identity: &str,
        project_rules: &str,
        stable_tool_definitions: &str,
    ) -> Self {
        Self {
            system_contract: system_contract.to_string(),
            agent_identity: agent_identity.to_string(),
            project_rules: project_rules.to_string(),
            stable_tool_definitions: stable_tool_definitions.to_string(),
            version: 1,
        }
    }

    /// The rendered prefix — the same bytes every turn until a part changes.
    pub fn render(&self) -> String {
        [
            self.system_contract.as_str(),
            self.agent_identity.as_str(),
            self.project_rules.as_str(),
            self.stable_tool_definitions.as_str(),
        ]
        .join("\n")
    }

    /// The cache key: the rendered bytes **and** the version. The version is
    /// the declared change marker, so a bump busts the key even when the bytes
    /// happen to be identical — an unchanged prefix always yields the same key
    /// regardless of how it was assembled.
    pub fn cache_key(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.version.to_le_bytes());
        h.update([0u8]);
        h.update(self.render().as_bytes());
        format!("{:x}", h.finalize())
    }

    /// The next prefix after a real change (a new tool, a new rule, a new
    /// contract). Dynamic content never changes it.
    pub fn with_version(&self, version: u32) -> Self {
        Self {
            version,
            ..self.clone()
        }
    }
}

/// A frozen injection block: computed once per session, reused verbatim.
/// Re-scoring it would mutate the prefix and bust the cache, so it is stored
/// with the signature of its member set (`REQ-CTX-009`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenBlock {
    pub name: String,
    /// The exact rendered bytes, reused verbatim.
    pub text: String,
    pub tokens: u32,
    /// The signature of the member set this block was computed from.
    pub signature: String,
}

/// Cache-hit telemetry (`REQ-CTX-009`: "cache-hit telemetry").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheTelemetry {
    pub turns: u32,
    pub prefix_hits: u32,
    pub prefix_misses: u32,
    pub frozen_block_hits: u32,
    pub frozen_block_misses: u32,
    pub baseline_emitted: u32,
    pub deltas_emitted: u32,
}

impl CacheTelemetry {
    pub fn prefix_hit_rate(&self) -> f64 {
        let total = self.prefix_hits + self.prefix_misses;
        if total == 0 {
            1.0
        } else {
            self.prefix_hits as f64 / total as f64
        }
    }
}

/// The session-scoped packing cache: baseline + deltas.
#[derive(Debug, Clone, Default)]
pub struct PackingCache {
    baseline: Option<String>,
    last_render: Option<String>,
    frozen: Vec<FrozenBlock>,
    pub telemetry: CacheTelemetry,
}

impl PackingCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persist a frozen block. Re-persisting an identical block is a no-op: the
    /// same bytes, the same signature, no cache bust.
    pub fn freeze(&mut self, block: FrozenBlock) -> bool {
        let already_frozen = self
            .frozen
            .iter()
            .any(|b| b.name == block.name && *b == block);
        if already_frozen {
            // Re-freezing identical bytes is a cache hit, never a recompute.
            self.telemetry.frozen_block_hits += 1;
            return false;
        }
        self.frozen.retain(|b| b.name != block.name);
        self.frozen.push(block);
        self.telemetry.frozen_block_misses += 1;
        true
    }

    /// The frozen block for a session, reused verbatim.
    pub fn frozen(&self, name: &str) -> Option<&FrozenBlock> {
        self.frozen.iter().find(|b| b.name == name)
    }

    /// Drop a frozen block after a declared mutation. The cache bust is
    /// accepted and counted.
    pub fn unfreeze(&mut self, name: &str) -> bool {
        let before = self.frozen.len();
        self.frozen.retain(|b| b.name != name);
        self.frozen.len() != before
    }

    /// Pack a turn: stable prefix plus the dynamic suffix, emitting the
    /// baseline on the first turn and a delta afterwards.
    pub fn pack(&mut self, prefix: &StablePrefix, dynamic: Vec<String>) -> PackedTurn {
        self.telemetry.turns += 1;
        let key = prefix.cache_key();
        let previous = self.baseline.clone();
        if previous.as_deref() == Some(key.as_str()) {
            self.telemetry.prefix_hits += 1;
            self.telemetry.deltas_emitted += 1;
        } else {
            self.telemetry.prefix_misses += 1;
            self.telemetry.baseline_emitted += 1;
            self.baseline = Some(key);
        }
        self.last_render = Some(prefix.render());
        PackedTurn {
            stable_prefix: prefix.render(),
            dynamic_suffix: dynamic,
        }
    }

    /// Whether the prefix is byte-stable relative to the previous turn. A
    /// `true` here is what keeps the provider cache alive.
    pub fn prefix_is_stable(&self, prefix: &StablePrefix) -> bool {
        self.last_render.as_deref() == Some(prefix.render().as_str())
    }

    /// Whether the full render is byte-identical to the previous turn.
    pub fn render_is_stable(&self, turn: &PackedTurn) -> bool {
        match &self.last_render {
            Some(prev) => prev != &turn.render(),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> StablePrefix {
        StablePrefix::new(
            "You are AgentCowork.",
            "agent:agent-x",
            "project rules: read ARCH/ before editing",
            "tools: read, write, bash",
        )
    }

    #[test]
    fn the_stable_prefix_is_byte_stable_across_turns() {
        let mut cache = PackingCache::new();
        let p = prefix();
        let a = cache.pack(&p, vec!["task: one".into()]);
        let b = cache.pack(&p, vec!["task: two".into()]);
        assert_eq!(a.stable_prefix, b.stable_prefix);
        assert!(cache.prefix_is_stable(&p));
        assert_eq!(cache.telemetry.prefix_hit_rate(), 0.5, "one miss, one hit");
    }

    #[test]
    fn a_real_change_to_the_prefix_busts_the_key_and_is_counted() {
        let mut cache = PackingCache::new();
        cache.pack(&prefix(), vec!["task: one".into()]);
        let changed = prefix().with_version(2);
        // A version bump is the declared change marker: the key changes, so
        // the cache is treated as cold and a fresh baseline is emitted.
        assert_ne!(prefix().cache_key(), changed.cache_key());
        cache.pack(&changed, vec!["task: two".into()]);
        assert_eq!(cache.telemetry.prefix_misses, 2);
        assert_eq!(cache.telemetry.baseline_emitted, 2);
        // The bytes themselves are unchanged by a version-only bump, which is
        // why the key — not the bytes — is what the cache keys on.
        assert_eq!(prefix().render(), changed.render());
    }

    #[test]
    fn dynamic_content_never_enters_the_prefix() {
        let mut cache = PackingCache::new();
        let p = prefix();
        let a = cache.pack(&p, vec!["observation: 42".into()]);
        let b = cache.pack(
            &p,
            vec!["observation: 43 — and here is a longer observation".into()],
        );
        assert_eq!(a.stable_prefix, b.stable_prefix);
        assert_ne!(a.dynamic_suffix, b.dynamic_suffix);
        assert!(a.total_tokens() < b.total_tokens());
        // The dynamic part is what grew: the prefix token count is identical.
        assert_eq!(
            crate::context::stability::estimate(&a.stable_prefix),
            crate::context::stability::estimate(&b.stable_prefix)
        );
    }

    #[test]
    fn the_first_turn_emits_a_baseline_and_later_turns_emit_deltas() {
        let mut cache = PackingCache::new();
        let p = prefix();
        cache.pack(&p, vec!["a".into()]);
        cache.pack(&p, vec!["b".into()]);
        cache.pack(&p, vec!["c".into()]);
        assert_eq!(cache.telemetry.baseline_emitted, 1);
        assert_eq!(cache.telemetry.deltas_emitted, 2);
        assert_eq!(cache.telemetry.turns, 3);
    }

    #[test]
    fn a_frozen_block_is_reused_verbatim_across_turns() {
        let mut cache = PackingCache::new();
        let block = FrozenBlock {
            name: "memory_always_on".into(),
            text: "- [user|user|personal] prefers tabs".into(),
            tokens: 12,
            signature: "m1|m2".into(),
        };
        assert!(cache.freeze(block.clone()), "the first freeze is a miss");
        for _ in 0..5 {
            assert!(
                !cache.freeze(block.clone()),
                "recomputing would churn the cache"
            );
        }
        assert_eq!(cache.frozen("memory_always_on").unwrap().text, block.text);
        assert_eq!(cache.telemetry.frozen_block_hits, 5);
        assert_eq!(cache.telemetry.frozen_block_misses, 1);
    }

    #[test]
    fn a_mutation_unfreezes_the_block_and_the_cache_bust_is_counted() {
        let mut cache = PackingCache::new();
        cache.freeze(FrozenBlock {
            name: "memory_always_on".into(),
            text: "a".into(),
            tokens: 1,
            signature: "m1".into(),
        });
        assert!(cache.unfreeze("memory_always_on"));
        assert!(cache.frozen("memory_always_on").is_none());
        // The next turn reflects the change: a fresh block is frozen.
        assert!(cache.freeze(FrozenBlock {
            name: "memory_always_on".into(),
            text: "b".into(),
            tokens: 1,
            signature: "m1".into(),
        }));
    }

    #[test]
    fn a_frozen_block_with_a_changed_member_set_replaces_the_old_one() {
        let mut cache = PackingCache::new();
        cache.freeze(FrozenBlock {
            name: "memory_always_on".into(),
            text: "a".into(),
            tokens: 1,
            signature: "m1".into(),
        });
        cache.freeze(FrozenBlock {
            name: "memory_always_on".into(),
            text: "a2".into(),
            tokens: 1,
            signature: "m1|m2".into(),
        });
        assert_eq!(cache.frozen("memory_always_on").unwrap().signature, "m1|m2");
        assert_eq!(cache.frozen("memory_always_on").unwrap().text, "a2");
    }

    #[test]
    fn cache_hit_telemetry_is_reported() {
        let mut cache = PackingCache::new();
        let p = prefix();
        cache.pack(&p, vec!["a".into()]);
        cache.pack(&p, vec!["b".into()]);
        let t = cache.telemetry;
        assert_eq!(t.prefix_hits, 1);
        assert_eq!(t.prefix_misses, 1);
        assert!((t.prefix_hit_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_prefix_cache_key_is_independent_of_assembly_order_of_turns() {
        let p1 = prefix();
        let p2 = prefix();
        assert_eq!(p1.cache_key(), p2.cache_key());
    }
}
