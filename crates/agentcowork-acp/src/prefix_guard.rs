//! P69.E9 — prefix-stability guard [I16] (`ARCH/CONTEXT.md` §4).
//!
//! The contract is one line: *the provider-visible prefix must be append-stable
//! across turns of the same Work, and any mutation of it is permitted only
//! when it is **intentional and observable*** (a declared cache-boundary
//! event). Before this module that invariant was prose with zero mechanical
//! enforcement — the exact failure the row exists for.
//!
//! What the stable prefix **is** here: `ARCH/CONTEXT.md` §4's diagram puts
//! system identity, stable schemas and tool definitions in the STABLE PREFIX
//! and **relevant memory in the DYNAMIC TAIL**. The live projection of the
//! stable prefix is therefore the shell-owned sections of the chief prompt —
//! the governance badge, the tool-affinity steering block and the installed
//! delegation mix (`build_chief_prompt_with_steering`). The warm-memory set
//! (`core_facts`) is deliberately **excluded**: it changes whenever the memory
//! system learns or consolidates, which is the expected dynamic-tail behaviour,
//! not a cache-boundary event. Fingerprinting it would make the guard fire on
//! every ordinary memory write — a guard that cries wolf gets deleted.
//!
//! Two pieces:
//!
//! * [`fingerprint_stable_prefix`] — a deterministic 64-bit FNV-1a over the
//!   stable-prefix inputs in assembly order. FNV-1a is hand-rolled so the
//!   hash is stable across process restarts and releases (a `DefaultHasher`
//!   seed is not), which is what makes the per-session observability log
//!   comparable over time.
//! * [`PrefixGuard`] — the per-handle state machine. Each turn calls
//!   [`PrefixGuard::observe`] with the fresh fingerprint and whether the
//!   change was *declared* (e.g. the compact-before-swap handoff bundle, which
//!   §4 explicitly permits: compaction ⇒ fresh baseline). The result is one
//!   of four events; [`PrefixEvent::UndeclaredMutation`] is the loud warning.
//!
//! The caller (the shell's turn path) writes the event into the per-session
//! tool log, making every prefix mutation observable — the "observable" half
//! of the invariant — while the declaration flag keeps intentional events
//! (compaction) distinct from churn. The guard warns; it never fails a turn:
//! the row's own wording is "fail **or** warn loudly", and refusing a turn of
//! an external agent over a prefix change would break the session for a
//! metrics problem.

/// Fingerprint inputs of the chief prompt's stable prefix, in assembly order.
fn fnv1a(parts: &[(&str, &str)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (tag, body) in parts {
        for b in tag.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        // Tag/body separator and part terminator are hashed too, so
        // ("a", "bc") and ("ab", "c") can never collide by concatenation.
        h ^= 0xff;
        h = h.wrapping_mul(0x100_0000_01b3);
        for b in body.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h ^= 0x0a;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Fingerprint the shell-owned stable prefix of the chief prompt, in the exact
/// order [`crate::chief::build_chief_prompt_with_steering`] assembles it. An
/// empty optional block hashes its *absence* (a distinct value from any
/// present body), so the delegation mix appearing or disappearing mid-Work is
/// a detected event, not silence.
pub fn fingerprint_stable_prefix(
    governance_badge: &str,
    affinity: Option<&str>,
    delegation_mix: Option<&str>,
) -> u64 {
    fnv1a(&[
        ("governance", governance_badge),
        (
            "affinity",
            match affinity {
                Some(a) if !a.trim().is_empty() => a,
                _ => "",
            },
        ),
        (
            "delegation_mix",
            match delegation_mix {
                Some(m) if !m.trim().is_empty() => m,
                _ => "",
            },
        ),
    ])
}

/// One observation of a turn's prefix state relative to the previous turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixEvent {
    /// First turn of this Work — nothing to compare against yet.
    FirstTurn,
    /// The fingerprint is unchanged; the prefix was append-stable.
    Stable,
    /// The fingerprint changed and the caller declared the change (a
    /// permitted cache-boundary event — e.g. a compaction handoff).
    DeclaredMutation,
    /// The fingerprint changed with **no declared event**. This is the I16
    /// violation: churn the caller did not intend. The shell logs it loudly.
    UndeclaredMutation,
}

impl PrefixEvent {
    /// The wire/log spelling (the per-session tool log is JSON).
    pub fn as_str(self) -> &'static str {
        match self {
            PrefixEvent::FirstTurn => "first_turn",
            PrefixEvent::Stable => "stable",
            PrefixEvent::DeclaredMutation => "declared_mutation",
            PrefixEvent::UndeclaredMutation => "undeclared_mutation",
        }
    }
}

/// Per-handle prefix state. Held inside the shell's session map; one guard
/// per launched agent session (the Work scope for interactive chats).
#[derive(Debug, Default)]
pub struct PrefixGuard {
    last: Option<u64>,
    turns: u64,
}

impl PrefixGuard {
    /// A fresh guard for a newly launched session.
    pub fn new() -> Self {
        Self {
            last: None,
            turns: 0,
        }
    }

    /// Observations so far (diagnostics; 0 until the first turn completes).
    pub fn turns(&self) -> u64 {
        self.turns
    }

    /// Record one turn's fingerprint. `declared` marks an intentional
    /// cache-boundary event the caller made (CONTEXT.md §4: permitted, but
    /// intentional and observable).
    pub fn observe(&mut self, fingerprint: u64, declared: bool) -> PrefixEvent {
        self.turns += 1;
        let event = match self.last {
            None => PrefixEvent::FirstTurn,
            Some(prev) if prev == fingerprint => PrefixEvent::Stable,
            Some(_) if declared => PrefixEvent::DeclaredMutation,
            Some(_) => PrefixEvent::UndeclaredMutation,
        };
        self.last = Some(fingerprint);
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_turn_is_first_turn() {
        let mut g = PrefixGuard::new();
        assert_eq!(g.observe(42, false), PrefixEvent::FirstTurn);
    }

    #[test]
    fn identical_prefix_is_stable() {
        let mut g = PrefixGuard::new();
        g.observe(42, false);
        assert_eq!(g.observe(42, false), PrefixEvent::Stable);
        assert_eq!(g.turns(), 2);
    }

    #[test]
    fn undeclared_change_is_the_violation() {
        let mut g = PrefixGuard::new();
        g.observe(42, false);
        assert_eq!(g.observe(43, false), PrefixEvent::UndeclaredMutation);
    }

    #[test]
    fn declared_change_is_permitted_and_observable() {
        let mut g = PrefixGuard::new();
        g.observe(42, false);
        assert_eq!(g.observe(43, true), PrefixEvent::DeclaredMutation);
    }

    #[test]
    fn fingerprint_is_deterministic_and_order_sensitive() {
        assert_eq!(
            fingerprint_stable_prefix("governed", None, None),
            fingerprint_stable_prefix("governed", None, None),
            "same inputs must hash identically across calls and restarts"
        );
        assert_ne!(
            fingerprint_stable_prefix("governed", None, None),
            fingerprint_stable_prefix("self-contained", None, None),
            "a governance change must be visible"
        );
        assert_ne!(
            fingerprint_stable_prefix("g", None, Some("- claude: coder")),
            fingerprint_stable_prefix("g", None, Some("- codex: coder")),
            "delegation-mix churn must be visible"
        );
    }

    #[test]
    fn absent_and_empty_blocks_hash_as_absent() {
        assert_eq!(
            fingerprint_stable_prefix("g", None, None),
            fingerprint_stable_prefix("g", Some(""), None),
        );
        assert_ne!(
            fingerprint_stable_prefix("g", None, None),
            fingerprint_stable_prefix("g", None, Some("- claude: coder")),
            "the delegation mix appearing must be a detected event"
        );
    }

    #[test]
    fn tag_body_boundaries_cannot_collide_by_concatenation() {
        // ("ab", "c") vs ("a", "bc") — same flattened bytes, different parts.
        assert_ne!(
            fingerprint_stable_prefix("ab", None, Some("c")),
            fingerprint_stable_prefix("a", None, Some("bc")),
        );
    }

    #[test]
    fn event_strings_are_stable_wire_vocabulary() {
        // The per-session log is a durable artifact: these strings must never
        // be reworded casually.
        assert_eq!(PrefixEvent::FirstTurn.as_str(), "first_turn");
        assert_eq!(PrefixEvent::Stable.as_str(), "stable");
        assert_eq!(PrefixEvent::DeclaredMutation.as_str(), "declared_mutation");
        assert_eq!(
            PrefixEvent::UndeclaredMutation.as_str(),
            "undeclared_mutation"
        );
    }
}
