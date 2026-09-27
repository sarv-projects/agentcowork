//! Compaction pipeline (Algorithm #21 — Reasonix ratio knobs + BrowserOS
//! callSummarizer + opencode compaction + Hermes tool-result persistence).
//!
//! Stale tool results are snipped to head/tail anchors; the context is
//! soft-compacted (notice) at 0.5 and force-summarized at 0.9; splits never
//! land mid-turn; `prefix_dirty` tracks cache-break events; the OpenCode
//! PRUNE_PROTECT budget erases surplus tool output.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionConfig {
    /// Stale tool result → head/tail anchor when stale fraction ≥ this.
    pub snip_ratio: f64,
    /// Notice-only threshold (soft compact).
    pub soft_ratio: f64,
    /// Force-summarize threshold.
    pub force_ratio: f64,
    /// OpenCode PRUNE_PROTECT: cap on retained tool-output tokens.
    pub prune_protect_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        CompactionConfig {
            snip_ratio: 0.6,
            soft_ratio: 0.5,
            force_ratio: 0.9,
            prune_protect_tokens: 40_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    None,
    /// A stale tool result crossed the snip ratio.
    Snip,
    /// Context crossed the soft ratio — notice only.
    SoftCompact,
    /// Context crossed the force ratio — summarize older turns.
    ForceCompact,
}

/// Decide the context-level action from tokens used vs the budget.
pub fn decide_context_action(used: usize, max: usize, cfg: &CompactionConfig) -> ContextAction {
    if max == 0 {
        return ContextAction::None;
    }
    let r = used as f64 / max as f64;
    if r >= cfg.force_ratio {
        ContextAction::ForceCompact
    } else if r >= cfg.soft_ratio {
        ContextAction::SoftCompact
    } else {
        ContextAction::None
    }
}

/// Should a stale tool result be snipped to a head/tail anchor?
pub fn should_snip(stale_ratio: f64, cfg: &CompactionConfig) -> bool {
    stale_ratio >= cfg.snip_ratio
}

/// Head/tail anchor: keep the first `head` and last `tail` chars, ellipsis
/// in the middle.
pub fn snip_anchor(text: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= head + tail {
        return text.to_string();
    }
    let mut out: String = chars[..head].iter().collect();
    out.push_str("\n…[snip]…\n");
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Find a split point (in turns) so the leading turns fit within
/// `target_tokens` and the boundary never lands mid-turn. Returns the number
/// of leading turns to keep.
pub fn find_safe_split(turn_tokens: &[usize], target_tokens: usize) -> usize {
    let mut acc = 0usize;
    let mut keep = 0usize;
    for &t in turn_tokens {
        if acc + t > target_tokens {
            break;
        }
        acc += t;
        keep += 1;
    }
    keep
}

/// Sliding window: keep the most recent `recent_tokens` of turns; the rest
/// are candidates for summarization.
pub fn sliding_window(turn_tokens: &[usize], recent_tokens: usize) -> (usize, usize) {
    // (summarize_end, keep_start) indices into turn_tokens
    let mut keep = 0usize;
    let mut acc = 0usize;
    for (i, &t) in turn_tokens.iter().enumerate().rev() {
        if acc + t > recent_tokens {
            break;
        }
        acc += t;
        keep = i;
    }
    (keep, turn_tokens.len())
}

/// Summarize-or-passthrough (BrowserOS callSummarizer): if the summarizer
/// returns `None` (timeout / abort), fail-open with the original text.
pub fn summarize_or_passthrough<F>(text: &str, summarize: F) -> String
where
    F: FnOnce(&str) -> Option<String>,
{
    summarize(text).unwrap_or_else(|| text.to_string())
}

/// Cache-break event kinds that set `prefix_dirty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheBreak {
    KeyRotation,
    ProviderSwitch,
    ModelSwitch,
    SystemPromptEdit,
}

/// Tracks whether the prompt-cache prefix is dirty (must be re-cached).
#[derive(Debug, Clone, Copy, Default)]
pub struct PrefixCache {
    pub dirty: bool,
}

impl PrefixCache {
    pub fn mark_dirty(&mut self, _reason: CacheBreak) {
        self.dirty = true;
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }
}

/// Hermes 3-layer tool-result persistence decision: small results stay
/// inline; large results are persisted to a path and only a preview enters
/// context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistDecision {
    Inline,
    PreviewAndPath,
}

/// `inline_cap_tokens` is the inline threshold; above it we persist.
pub fn persist_decision(tokens: usize, inline_cap_tokens: usize) -> PersistDecision {
    if tokens <= inline_cap_tokens {
        PersistDecision::Inline
    } else {
        PersistDecision::PreviewAndPath
    }
}

/// OpenCode PRUNE_PROTECT: retain tool outputs up to the protect budget,
/// erasing the surplus (oldest first). `outputs` = (id, tokens).
pub fn prune_protect(outputs: &[(String, usize)], budget: usize) -> Vec<String> {
    let mut used = 0usize;
    let mut kept = Vec::new();
    for (id, tokens) in outputs.iter().rev() {
        if used + tokens > budget {
            break;
        }
        used += tokens;
        kept.push(id.clone());
    }
    kept.reverse();
    kept
}

// ---------------------------------------------------------------------------
// Turn-loop coordinator wiring (P5.8 — the coordinator's hook into the
// compaction lifecycle)
// ---------------------------------------------------------------------------

/// The coordinator-side compaction state: observes per-turn token counts,
/// decides the context action, and drives [`run_compaction_lifecycle`] when a
/// compact is due — emitting the `PreCompact/Compacted/PostCompact` events
/// the turn loop turns into `ContextCompaction` turn items.
#[derive(Debug, Clone)]
pub struct CompactionCoordinator {
    pub config: CompactionConfig,
    /// The context window budget the loop is operating under.
    pub max_tokens: usize,
    /// Token count of each completed turn (the accumulation is what triggers).
    turn_tokens: Vec<usize>,
    /// Sum of all turn tokens since the last compact.
    total_tokens: usize,
    /// Lifecycle events emitted since the last drain (turn items).
    events: Vec<CompactionEvent>,
}

impl CompactionCoordinator {
    pub fn new(config: CompactionConfig, max_tokens: usize) -> Self {
        Self {
            config,
            max_tokens,
            turn_tokens: Vec::new(),
            total_tokens: 0,
            events: Vec::new(),
        }
    }

    /// Record one completed turn's token count and return the context action
    /// the loop should take now (soft notice vs force compact).
    pub fn push_turn(&mut self, tokens: usize) -> ContextAction {
        self.turn_tokens.push(tokens);
        self.total_tokens += tokens;
        decide_context_action(self.total_tokens, self.max_tokens, &self.config)
    }

    /// Tokens accumulated since the last compact.
    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }

    /// Drive the compact through its lifecycle when the loop decides one is
    /// due. Runs [`run_compaction_lifecycle`], records its events as turn
    /// items, and resets the accumulation. Returns `None` when no compact is
    /// due (the loop keeps the context untouched).
    pub fn maybe_compact(
        &mut self,
        text: &str,
        summarizers: &[&Summarizer],
    ) -> Option<(String, FallbackStep)> {
        let action = decide_context_action(self.total_tokens, self.max_tokens, &self.config);
        if !matches!(
            action,
            ContextAction::SoftCompact | ContextAction::ForceCompact
        ) {
            return None;
        }
        let from_tokens = self.total_tokens;
        let (out, step) =
            run_compaction_lifecycle(text, from_tokens, self.max_tokens, summarizers, |e| {
                self.events.push(e)
            });
        let to_tokens = out.chars().count() / 4;
        self.total_tokens = to_tokens;
        self.turn_tokens.clear();
        Some((out, step))
    }

    /// A soft-compact notice is warranted (crossed `soft_ratio`, not yet
    /// `force_ratio`) — the loop emits a user-visible "context is getting
    /// long" hint.
    pub fn should_notice(&self) -> bool {
        matches!(
            decide_context_action(self.total_tokens, self.max_tokens, &self.config),
            ContextAction::SoftCompact
        )
    }

    /// Drain the lifecycle events recorded since the last call (the turn
    /// items the loop persists/emits).
    pub fn drain_events(&mut self) -> Vec<CompactionEvent> {
        std::mem::take(&mut self.events)
    }
}

// ---------------------------------------------------------------------------
// Compaction-as-lifecycle (doc 63 §4.5 — codex `hook_runtime.rs` pattern)
// ---------------------------------------------------------------------------

/// A summarizer callback: returns `None` on timeout/abort (fail-open).
pub type Summarizer = dyn Fn(&str) -> Option<String>;

/// Which step of the fallback chain produced the compacted text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackStep {
    /// The primary summarizer succeeded.
    PrimarySummarizer,
    /// The `n`-th fallback model (0-based index into the chain) succeeded.
    FallbackModel(usize),
    /// Every summarizer failed — last-resort truncation with a marker.
    TruncateWithMarker,
}

/// Events emitted across a compaction lifecycle (codex PreCompactHook →
/// compact → PostCompactHook; the `Compacted` event is what becomes the
/// `ContextCompaction` turn item in the coordinator loop).
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionEvent {
    /// PreCompactHook ran — notify/capture (memory, metrics, cache) before
    /// the compact. Carries the pre-compact token count.
    PreCompact { from_tokens: usize },
    /// The compact completed.
    Compacted {
        from_tokens: usize,
        to_tokens: usize,
        step: FallbackStep,
    },
    /// PostCompactHook ran.
    PostCompact { to_tokens: usize },
}

/// Last-resort truncation: keep the head and tail, mark the cut so the
/// model (and the user) can see context was dropped — never silent loss.
pub fn truncate_with_marker(text: &str, max_tokens: usize) -> String {
    let approx = text.chars().count() / 4; // ~4 chars/token heuristic
    if approx <= max_tokens {
        return text.to_string();
    }
    let head_chars = max_tokens * 3; // ~3/4 head, 1/4 tail
    let tail_chars = max_tokens;
    let chars: Vec<char> = text.chars().collect();
    let head: String = chars.iter().take(head_chars).collect();
    let tail: String = chars
        .iter()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "{head}\n\n…[context truncated: {max_tokens} token budget — earlier turns summarized away]…\n\n{tail}"
    )
}

/// Compact `text` to fit `max_tokens` using a fallback chain. `summarizers`
/// is tried in order (primary first); each returns `None` on timeout/abort.
/// If every summarizer fails, the text is truncated with a marker. Returns
/// the compacted text + which step produced it.
pub fn compact_with_fallback(
    text: &str,
    max_tokens: usize,
    summarizers: &[&Summarizer],
) -> (String, FallbackStep) {
    let approx = text.chars().count() / 4;
    if approx <= max_tokens {
        return (text.to_string(), FallbackStep::PrimarySummarizer);
    }
    for (i, summarize) in summarizers.iter().enumerate() {
        if let Some(out) = summarize(text) {
            let step = if i == 0 {
                FallbackStep::PrimarySummarizer
            } else {
                FallbackStep::FallbackModel(i - 1)
            };
            return (out, step);
        }
    }
    (
        truncate_with_marker(text, max_tokens),
        FallbackStep::TruncateWithMarker,
    )
}

/// Drive a compaction through its lifecycle, emitting an event at each phase
/// (`on_event` is the coordinator's hook — it can persist turn items, update
/// metrics, etc.). Returns the compacted text + fallback step.
pub fn run_compaction_lifecycle<F>(
    text: &str,
    from_tokens: usize,
    max_tokens: usize,
    summarizers: &[&Summarizer],
    mut on_event: F,
) -> (String, FallbackStep)
where
    F: FnMut(CompactionEvent),
{
    on_event(CompactionEvent::PreCompact { from_tokens });
    let (out, step) = compact_with_fallback(text, max_tokens, summarizers);
    let to_tokens = out.chars().count() / 4;
    on_event(CompactionEvent::Compacted {
        from_tokens,
        to_tokens,
        step,
    });
    on_event(CompactionEvent::PostCompact { to_tokens });
    (out, step)
}

// ---------------------------------------------------------------------------
// Budget formula + tail selection + overflow replay (P51.33 + P52.18-Rust-half)
// ---------------------------------------------------------------------------

/// Reserve kept free so a compact has headroom to emit without re-overflowing.
pub const COMPACTION_BUFFER: usize = 20_000;

/// Minimum retained size the prune step never compacts below.
pub const PRUNE_MINIMUM: usize = 20_000;

/// Number of recent full turns always preserved through a compact.
pub const TAIL_TURNS: usize = 2;

/// Input-budget formula: `usable = input_limit - reserved`, with
/// `preserve_recent ≈ 25% of usable` clamped to 2K–15K tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetFormula {
    pub input_limit: usize,
    pub reserved: usize,
}

impl BudgetFormula {
    pub fn usable(&self) -> usize {
        self.input_limit.saturating_sub(self.reserved)
    }

    pub fn preserve_recent_tokens(&self) -> usize {
        (self.usable() / 4).clamp(2000, 15000)
    }
}

/// Stats for a binary/partial-turn split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialTurn {
    pub kept_chars: usize,
    pub dropped_chars: usize,
}

/// Binary/partial-turn split keeping the TAIL within `budget_chars`
/// (complement to the linear whole-turn-prefix [`find_safe_split`]).
/// Returns `(dropped head, stats, kept tail)`.
pub fn split_turn(text: &str, budget_chars: usize) -> (String, PartialTurn, String) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= budget_chars {
        return (
            String::new(),
            PartialTurn {
                kept_chars: chars.len(),
                dropped_chars: 0,
            },
            text.to_string(),
        );
    }
    if budget_chars == 0 {
        return (
            text.to_string(),
            PartialTurn {
                kept_chars: 0,
                dropped_chars: chars.len(),
            },
            String::new(),
        );
    }
    let split_at = chars.len() - budget_chars;
    let dropped: String = chars[..split_at].iter().collect();
    let kept: String = chars[split_at..].iter().collect();
    (
        dropped,
        PartialTurn {
            kept_chars: budget_chars,
            dropped_chars: split_at,
        },
        kept,
    )
}

/// Keep the recent tail within `preserve_budget` (chars): at most
/// [`TAIL_TURNS`] full turns plus one partial boundary turn (via
/// [`split_turn`]). Returns `(dropped prefix turns, kept suffix turns)` where
/// the first kept entry may be a partial tail fragment.
pub fn select_tail(turns: &[String], preserve_budget: usize) -> (Vec<String>, Vec<String>) {
    if turns.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if preserve_budget == 0 {
        return (turns.to_vec(), Vec::new());
    }
    let mut kept_rev: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut full_kept = 0usize;
    // `kept_start` = index of the first turn contributing to `kept`
    // (full or partial). Everything before it is dropped.
    let mut idx = turns.len();
    while idx > 0 && full_kept < TAIL_TURNS {
        let len = turns[idx - 1].chars().count();
        if used + len > preserve_budget {
            break;
        }
        kept_rev.push(turns[idx - 1].clone());
        used += len;
        full_kept += 1;
        idx -= 1;
    }
    let mut kept_start = idx;
    if idx > 0 && used < preserve_budget {
        let remaining = preserve_budget - used;
        let boundary = &turns[idx - 1];
        let (_head, _info, tail) = split_turn(boundary, remaining);
        if !tail.is_empty() {
            kept_rev.push(tail);
            kept_start = idx - 1;
        }
    }
    kept_rev.reverse();
    let dropped = turns[..kept_start].to_vec();
    (dropped, kept_rev)
}

/// Synthetic marker re-injected when an overflow was replayed through
/// prune → compact so the model sees the cut instead of silent loss.
pub const CONTINUE_MARKER: &str = "[continued — earlier context compacted…]";

/// Return the synthetic continue marker when an overflow was replayed through
/// prune → compact (`was_overflow`) and bytes were actually pruned;
/// otherwise `None` (emitted at most once per replay).
pub fn overflow_replay(was_overflow: bool, pruned_bytes: usize) -> Option<String> {
    if was_overflow && pruned_bytes > 0 {
        Some(CONTINUE_MARKER.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_actions_by_ratio() {
        let cfg = CompactionConfig::default();
        assert_eq!(decide_context_action(40, 100, &cfg), ContextAction::None);
        assert_eq!(
            decide_context_action(50, 100, &cfg),
            ContextAction::SoftCompact
        );
        assert_eq!(
            decide_context_action(90, 100, &cfg),
            ContextAction::ForceCompact
        );
        assert_eq!(decide_context_action(10, 0, &cfg), ContextAction::None);
    }

    #[test]
    fn coordinator_triggers_compact_at_ratio_without_breaking_loop() {
        // P5.7: the per-turn loop — push turns until the force ratio, compact,
        // and drain the lifecycle events (PreCompact → Compacted → PostCompact)
        // in order. The loop never sees a broken/partial state.
        let mut c = CompactionCoordinator::new(CompactionConfig::default(), 100);

        // Below soft ratio: no action.
        assert_eq!(c.push_turn(20), ContextAction::None);
        assert_eq!(c.push_turn(20), ContextAction::None); // 40/100

        // Cross soft ratio (0.5): notice.
        assert_eq!(c.push_turn(15), ContextAction::SoftCompact); // 55/100
        assert!(c.should_notice());

        // Cross force ratio (0.9): compact.
        assert_eq!(c.push_turn(40), ContextAction::ForceCompact); // 95/100

        let text = "x".repeat(800); // ~200 tokens — over the 100 budget
        let summarizer: &Summarizer = &|_| Some("summarized".to_string());
        let (out, step) = c.maybe_compact(&text, &[summarizer]).unwrap();
        assert_eq!(step, FallbackStep::PrimarySummarizer);
        assert_eq!(out, "summarized");
        assert!(c.total_tokens() < 95, "compact must shrink the context");

        // Lifecycle events emitted in order.
        let events = c.drain_events();
        assert!(matches!(events[0], CompactionEvent::PreCompact { .. }));
        assert!(matches!(events[1], CompactionEvent::Compacted { .. }));
        assert!(matches!(events[2], CompactionEvent::PostCompact { .. }));

        // After a compact, pushing more turns restarts accumulation cleanly.
        assert_eq!(c.push_turn(10), ContextAction::None);
    }

    #[test]
    fn coordinator_falls_back_to_truncate_when_summarizer_fails() {
        let mut c = CompactionCoordinator::new(CompactionConfig::default(), 100);
        c.push_turn(95); // force
        let failing: &Summarizer = &|_| None;
        let text = "y".repeat(800); // ~200 tokens — over the 100 budget
        let (out, step) = c.maybe_compact(&text, &[failing]).unwrap();
        assert_eq!(step, FallbackStep::TruncateWithMarker);
        assert!(out.contains("y"));
        assert!(
            out.contains("[context truncated"),
            "marker must be present: {out}"
        );
        assert!(
            out.chars().count() < 800,
            "truncated shorter than the input"
        );
    }

    #[test]
    fn snip_anchor_keeps_head_tail() {
        let text = "A".repeat(100);
        let s = snip_anchor(&text, 10, 10);
        assert!(s.starts_with("AAAAAAAAAA"));
        assert!(s.ends_with("AAAAAAAAAA"));
        assert!(s.contains("[snip]"));
        // Short text is returned verbatim.
        assert_eq!(snip_anchor("hi", 10, 10), "hi");
    }

    #[test]
    fn safe_split_never_mid_turn() {
        let turns = [100, 100, 100, 100];
        assert_eq!(find_safe_split(&turns, 250), 2); // 200 fits, 300 doesn't
        assert_eq!(find_safe_split(&turns, 0), 0);
    }

    #[test]
    fn sliding_window_keeps_recent() {
        let turns = [50, 50, 50, 50];
        // recent 100 tokens → last two turns (indices 2..4)
        let (summarize_end, _) = sliding_window(&turns, 100);
        assert_eq!(summarize_end, 2);
    }

    #[test]
    fn summarize_fails_open() {
        assert_eq!(summarize_or_passthrough("abc", |_| None), "abc");
        assert_eq!(
            summarize_or_passthrough("abc", |t| Some(format!("<{t}>"))),
            "<abc>"
        );
    }

    #[test]
    fn prefix_dirty_flag() {
        let mut p = PrefixCache::default();
        assert!(!p.dirty);
        p.mark_dirty(CacheBreak::KeyRotation);
        assert!(p.dirty);
        p.mark_clean();
        assert!(!p.dirty);
    }

    #[test]
    fn persist_decision_threshold() {
        assert_eq!(persist_decision(100, 200), PersistDecision::Inline);
        assert_eq!(persist_decision(201, 200), PersistDecision::PreviewAndPath);
    }

    #[test]
    fn prune_protect_erases_surplus() {
        let outputs = vec![
            ("a".to_string(), 10_000),
            ("b".to_string(), 20_000),
            ("c".to_string(), 15_000),
        ];
        // budget 30k → keeps c (15k) + b (20k) = 35k > 30k, so only c fits
        // then b would push over → keep c only? iterate rev: c(15k) ok, b(+20k)=35k>30k stop.
        let kept = prune_protect(&outputs, 30_000);
        assert_eq!(kept, vec!["c".to_string()]);

        // 35k keeps c (15k) + b (20k) = 35k, then a (+10k = 45k) > 35k → erased.
        let kept2 = prune_protect(&outputs, 35_000);
        assert_eq!(kept2, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn fallback_chain_uses_first_success() {
        let primary: &dyn Fn(&str) -> Option<String> = &|_| None; // fails
        let fb: &dyn Fn(&str) -> Option<String> = &|_| Some("summary".into());
        let (out, step) = compact_with_fallback(&"x".repeat(10_000), 100, &[primary, fb]);
        assert_eq!(out, "summary");
        assert_eq!(step, FallbackStep::FallbackModel(0));
    }

    #[test]
    fn fallback_chain_truncates_when_all_fail() {
        let fail: &dyn Fn(&str) -> Option<String> = &|_| None;
        let (out, step) = compact_with_fallback(&"y".repeat(10_000), 50, &[fail, fail]);
        assert_eq!(step, FallbackStep::TruncateWithMarker);
        assert!(out.contains("context truncated"), "{out}");
    }

    #[test]
    fn small_text_bypasses_compaction() {
        let fail: &dyn Fn(&str) -> Option<String> = &|_| None;
        let (out, step) = compact_with_fallback("short", 100, &[fail]);
        assert_eq!(out, "short");
        assert_eq!(step, FallbackStep::PrimarySummarizer);
    }

    #[test]
    fn lifecycle_emits_pre_compact_post() {
        let ok: &dyn Fn(&str) -> Option<String> = &|_| Some("compacted".into());
        let mut events = Vec::new();
        let (out, step) =
            run_compaction_lifecycle(&"z".repeat(10_000), 2500, 100, &[ok], |e| events.push(e));
        assert_eq!(out, "compacted");
        assert_eq!(step, FallbackStep::PrimarySummarizer);
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            CompactionEvent::PreCompact { from_tokens: 2500 }
        ));
        assert!(matches!(
            events[1],
            CompactionEvent::Compacted {
                step: FallbackStep::PrimarySummarizer,
                ..
            }
        ));
        assert!(matches!(events[2], CompactionEvent::PostCompact { .. }));
    }

    #[test]
    fn coordinator_accumulates_and_soft_notices() {
        let cfg = CompactionConfig::default(); // soft 0.5, force 0.9
        let mut c = CompactionCoordinator::new(cfg, 100);
        assert_eq!(c.push_turn(40), ContextAction::None); // 40/100
        assert_eq!(c.push_turn(10), ContextAction::SoftCompact); // 50/100
        assert!(c.should_notice());
        assert_eq!(c.total_tokens(), 50);
    }

    #[test]
    fn coordinator_force_compacts_and_resets() {
        let ok: &dyn Fn(&str) -> Option<String> = &|_| Some("compacted".into());
        let mut c = CompactionCoordinator::new(CompactionConfig::default(), 100);
        c.push_turn(40);
        c.push_turn(40);
        assert_eq!(c.push_turn(15), ContextAction::ForceCompact); // 95/100
        let (out, step) = c.maybe_compact(&"x".repeat(10_000), &[ok]).unwrap();
        assert_eq!(out, "compacted");
        assert_eq!(step, FallbackStep::PrimarySummarizer);
        // Lifecycle events became turn items.
        let events = c.drain_events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            CompactionEvent::PreCompact { from_tokens: 95 }
        ));
        // Accumulation reset; total now reflects the compacted size.
        assert!(c.total_tokens() < 95);
        assert!(!c.should_notice());
    }

    #[test]
    fn coordinator_no_compact_when_not_due() {
        let fail: &dyn Fn(&str) -> Option<String> = &|_| None;
        let mut c = CompactionCoordinator::new(CompactionConfig::default(), 100);
        c.push_turn(10);
        assert!(c.maybe_compact("short", &[fail]).is_none());
        assert!(c.drain_events().is_empty());
    }

    #[test]
    fn coordinator_compaction_reuses_the_fallback_chain() {
        let fail: &dyn Fn(&str) -> Option<String> = &|_| None;
        let mut c = CompactionCoordinator::new(CompactionConfig::default(), 100);
        c.push_turn(60);
        c.push_turn(40); // 100 ≥ 90 → force
        let (out, step) = c.maybe_compact(&"y".repeat(10_000), &[fail]).unwrap();
        assert_eq!(step, FallbackStep::TruncateWithMarker);
        assert!(out.contains("context truncated"));
    }

    #[test]
    fn budget_is_25pct_clamped() {
        // 100K usable → 25K capped to 15K.
        let b = BudgetFormula {
            input_limit: 120_000,
            reserved: 20_000,
        };
        assert_eq!(b.usable(), 100_000);
        assert_eq!(b.preserve_recent_tokens(), 15_000);
        // 32K usable → 8K (within bounds).
        let b2 = BudgetFormula {
            input_limit: 32_000,
            reserved: 0,
        };
        assert_eq!(b2.usable(), 32_000);
        assert_eq!(b2.preserve_recent_tokens(), 8_000);
        // 4K usable → 1K floored to 2K.
        let b3 = BudgetFormula {
            input_limit: 4_000,
            reserved: 0,
        };
        assert_eq!(b3.usable(), 4_000);
        assert_eq!(b3.preserve_recent_tokens(), 2_000);
    }

    #[test]
    fn split_turn_keeps_partial_tail() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let (dropped, info, kept) = split_turn(text, 10);
        assert_eq!(kept, "qrstuvwxyz");
        assert_eq!(dropped, "abcdefghijklmnop");
        assert_eq!(info.kept_chars, 10);
        assert_eq!(info.dropped_chars, 16);
        assert_eq!(dropped.len() + kept.len(), text.len());
        // Fits fully → nothing dropped.
        let (d2, info2, k2) = split_turn("hi", 10);
        assert_eq!(k2, "hi");
        assert!(d2.is_empty());
        assert_eq!(info2.dropped_chars, 0);
    }

    #[test]
    fn select_tail_keeps_two_full_turns_plus_partial() {
        let turns = vec![
            "a".repeat(100),
            "b".repeat(100),
            "c".repeat(100),
            "d".repeat(100),
        ];
        // Budget 250: two full recent turns (200) + 50-char partial of boundary.
        let (dropped, kept) = select_tail(&turns, 250);
        assert_eq!(kept.len(), 3, "2 full + 1 partial: {kept:?}");
        assert_eq!(kept[1], "c".repeat(100));
        assert_eq!(kept[2], "d".repeat(100));
        assert_eq!(kept[0].chars().count(), 50);
        assert!(kept[0].chars().all(|c| c == 'b'));
        assert_eq!(dropped, vec!["a".repeat(100)]);
        // Exactly TAIL_TURNS full turns preserved, never more full turns.
        assert_eq!(TAIL_TURNS, 2);
    }

    #[test]
    fn overflow_replays_marker_once() {
        assert_eq!(overflow_replay(false, 100), None);
        assert_eq!(overflow_replay(true, 0), None);
        let once = overflow_replay(true, 128);
        assert_eq!(once.as_deref(), Some(CONTINUE_MARKER));
    }

    #[test]
    fn continue_marker_present() {
        assert!(CONTINUE_MARKER.contains("continued"));
        assert!(CONTINUE_MARKER.contains("compacted"));
    }
}
