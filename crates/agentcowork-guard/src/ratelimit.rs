//! Control-plane rate limiting (FIX-02 / `TASK-TRUST-001`).
//!
//! Trust infrastructure must fail closed (`ARCH/12-TRUST.md` §11,
//! `REQ-TRUST-009`): when a dependency is unavailable or a caller exceeds its
//! share, the effect is **denied with a typed error** — never allowed by
//! fallback. Before this module the control plane (`nativeCall`/Tauri commands
//! and the kernel tool path) had no admission control at all, so a single
//! caller could saturate the IPC surface, the Guard policy engine, or the
//! ticket store.
//!
//! **Design constraints (all hard):**
//!
//! * **Bounded memory.** A token bucket per `(caller, command)` key in a map
//!   capped at [`RateLimitConfig::max_entries`]. When the cap is reached the
//!   least-recently-used bucket is evicted — the map can never grow without
//!   bound, and eviction can only *relax* a limit, never grant an allowance
//!   that did not exist.
//! * **Bounded time.** Idle buckets expire after [`RateLimitConfig::ttl_ms`]
//!   and are dropped on the next admission check, so a churning key space
//!   cannot pin memory either.
//! * **No panic, no poisoning, fail closed.** A poisoned lock is treated as a
//!   denial. There is no `unwrap` on a lock, no allocation that can abort the
//!   caller, and no path that returns "allow" without a token.
//! * **Monotonic clock.** Durations come from a caller-supplied epoch-ms
//!   (`Instant`-backed in the shell, `now_ms()` in the kernel), never from a
//!   wall-clock delta, per `ARCH/10-KERNEL.md` §5.
//!
//! **Error code.** A refusal maps onto the canonical taxonomy's
//! [`Unavailable`](`ARCH/10-KERNEL.md` §3) code: the callee is temporarily
//! unable to serve this request and a bounded backoff may succeed. The
//! taxonomy is canonical and "extensions require a `DEC`", so this module
//! deliberately does **not** introduce a `RateLimited` code; it only adds the
//! stable `rate_limited` reason token plus `retry_after_ms`, which is what a
//! caller needs to back off correctly.
//!
//! Two tiers compose, so one noisy command cannot starve the rest of the
//! surface and one noisy caller cannot spend another's budget:
//!
//! * **global** — one bucket for the whole control plane;
//! * **per key** — one bucket per `(caller, command)` pair.
//!
//! Both must have a token for the call to proceed.

use std::collections::HashMap;
use std::sync::Mutex;

/// Which tier refused the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitScope {
    /// The process-wide control-plane bucket.
    Global,
    /// The `(caller, command)` bucket.
    CallerCommand,
}

impl std::fmt::Display for RateLimitScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl RateLimitScope {
    /// Stable token for the audit row / structured error.
    pub const fn as_str(self) -> &'static str {
        match self {
            RateLimitScope::Global => "global",
            RateLimitScope::CallerCommand => "caller_command",
        }
    }
}

/// One token bucket: a burst allowance plus a steady refill rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    /// Maximum tokens the bucket can hold (the burst allowance).
    pub burst: f64,
    /// Tokens added per second.
    pub refill_per_sec: f64,
}

impl Limit {
    /// A bucket of `burst` tokens refilling at `refill_per_sec`.
    pub const fn new(burst: u32, refill_per_sec: f64) -> Self {
        Self {
            burst: burst as f64,
            refill_per_sec,
        }
    }
}

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last_refill_ms: u64,
    last_seen_ms: u64,
}

impl Bucket {
    /// Add the tokens earned since `last_refill_ms`, saturating at the
    /// bucket's burst ceiling — idle time is never banked.
    fn refill(&mut self, elapsed_ms: u64, limit: &Limit) {
        if elapsed_ms == 0 || limit.refill_per_sec <= 0.0 {
            return;
        }
        let added = (elapsed_ms as f64 / 1000.0) * limit.refill_per_sec;
        self.tokens = (self.tokens + added).min(limit.burst);
    }

    /// Milliseconds until this bucket holds one whole token again (at least 1
    /// so a zero-rate bucket still reports a positive, honest wait).
    fn retry_after_ms(&self, limit: &Limit) -> u64 {
        if limit.refill_per_sec <= 0.0 {
            return u64::MAX;
        }
        let missing = (1.0 - self.tokens).max(0.0);
        if missing <= f64::EPSILON {
            return 0;
        }
        ((missing / limit.refill_per_sec) * 1000.0).ceil() as u64
    }
}

/// The limiter's shape. Both tiers and both bounds are explicit so a caller
/// can size them to its own surface without editing this module.
///
/// The shape is a **product knob, not a spec constant** (`TASK-TRUST-011`): the
/// kernel configuration owns it as the `controlPlaneRateLimit` entry, whose
/// *defaults* are [`RateLimitConfig::default`] below. This module stays the one
/// place the shipped numbers are written down, and the entry reads them from
/// here rather than repeating them, so the shell's IPC gate (which builds
/// [`RateLimiter::with_defaults`]) and the kernel tool gate (which builds the
/// limiter from the resolved entry) cannot be given different limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitConfig {
    /// Process-wide ceiling (across every caller and command).
    pub global: Limit,
    /// Per `(caller, command)` ceiling.
    pub per_caller_command: Limit,
    /// Idle-bucket TTL. A bucket unseen for this long is dropped.
    pub ttl_ms: u64,
    /// Hard cap on tracked buckets. The map never exceeds this.
    pub max_entries: usize,
}

impl Default for RateLimitConfig {
    /// The desktop default: generous enough that a normal turn is never
    /// throttled, tight enough that a runaway loop is refused in seconds.
    ///
    /// **Why these numbers.** A busy turn on this surface is *bursty by
    /// nature*: a chat turn streams chunks, a tool loop calls the executor many
    /// times in a row, and the UI polls status. A per-key burst in the tens
    /// would throttle legitimate work, so the burst is sized in the hundreds and
    /// the *steady rate* is the real control: 20/s per `(caller, command)` and
    /// 100/s process-wide means a runaway loop is refused after roughly a
    /// second of hammering, while no human interaction pattern reaches it. The
    /// bound that actually protects the process is the map cap
    /// ([`RateLimitConfig::max_entries`]) plus the TTL, not the token count.
    ///
    /// **Where they live now.** These are the *defaults* of the kernel
    /// configuration's `controlPlaneRateLimit` entry, and this is the one place
    /// they are written down. The entry layers a user/workspace/agent/session/run
    /// override on top of them; a host that resolves that entry hands the result
    /// to both admission gates, so the two read one value. Changing a number
    /// here changes the shipped default for every gate at once — which is the
    /// point.
    fn default() -> Self {
        Self {
            global: Limit::new(600, 100.0),
            per_caller_command: Limit::new(120, 20.0),
            ttl_ms: 60_000,
            max_entries: 4096,
        }
    }
}

impl RateLimitConfig {
    /// A config sized for tests / small surfaces.
    pub fn tight() -> Self {
        Self {
            global: Limit::new(10, 0.0),
            per_caller_command: Limit::new(3, 0.0),
            ttl_ms: 1_000,
            max_entries: 8,
        }
    }
}

/// A refusal. Typed, actionable, and free of any user or request content
/// (`ARCH/10-KERNEL.md` §3: no secrets, no internals across a boundary).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("rate limit exceeded ({scope}) — retry after {retry_after_ms}ms")]
pub struct RateLimitError {
    /// Which tier ran out.
    pub scope: RateLimitScope,
    /// Canonical taxonomy code: `Unavailable` (retryable, back off).
    pub code: &'static str,
    /// Stable reason token for the audit row.
    pub reason: &'static str,
    /// How long until the refused tier has a token again.
    pub retry_after_ms: u64,
}

impl RateLimitError {
    /// `ARCH/10-KERNEL.md` §3 — `Unavailable` is "provider down or degraded",
    /// the only canonical code whose contract (retry with backoff) matches a
    /// throttled call. A dedicated code would need a `DEC`.
    pub const CODE: &'static str = "Unavailable";

    /// Retryable: a bounded backoff can succeed.
    pub const RETRYABLE: bool = true;

    /// [`Self::CODE`].
    pub const fn code(&self) -> &'static str {
        Self::CODE
    }

    /// [`Self::RETRYABLE`].
    pub const fn retryable(&self) -> bool {
        Self::RETRYABLE
    }
}

#[derive(Debug)]
struct State {
    buckets: HashMap<String, Bucket>,
    global: Bucket,
}

/// A bounded, per-caller/per-command token-bucket limiter.
///
/// Cheap to share: one instance behind an `Arc` guards the whole control
/// plane. Every method is non-panicking; a poisoned lock is a denial.
#[derive(Debug)]
pub struct RateLimiter {
    cfg: RateLimitConfig,
    state: Mutex<State>,
}

impl RateLimiter {
    /// Build a limiter with an explicit shape.
    pub fn new(cfg: RateLimitConfig) -> Self {
        let global = Bucket {
            tokens: cfg.global.burst,
            last_refill_ms: 0,
            last_seen_ms: 0,
        };
        Self {
            cfg,
            state: Mutex::new(State {
                buckets: HashMap::new(),
                global,
            }),
        }
    }

    /// Build a limiter with [`RateLimitConfig::default`] — the shipped values,
    /// which are also the `controlPlaneRateLimit` entry's defaults
    /// (`TASK-TRUST-011`).
    ///
    /// A gate that should honour a *resolved* entry (one that a user, workspace,
    /// agent, session or run overrode) must be built with
    /// [`RateLimiter::new`] and that entry's value instead; this constructor is
    /// the un-overridden default, not a second source of numbers.
    pub fn with_defaults() -> Self {
        Self::new(RateLimitConfig::default())
    }

    /// The configuration in force (for diagnostics and tests).
    pub fn config(&self) -> &RateLimitConfig {
        &self.cfg
    }

    /// How many buckets are currently tracked. Never exceeds
    /// [`RateLimitConfig::max_entries`] — the memory bound, asserted rather
    /// than assumed.
    pub fn tracked_keys(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .buckets
            .len()
    }

    /// Admit one call from `caller` for `command`, stamped at `now_ms`.
    ///
    /// The global tier is checked first (cheapest, and the one that stops a
    /// whole-surface flood), then the `(caller, command)` tier. A refusal on
    /// either tier spends nothing on the other.
    pub fn check_at(&self, caller: &str, command: &str, now_ms: u64) -> Result<(), RateLimitError> {
        let mut state = match self.state.lock() {
            Ok(s) => s,
            // Fail closed: a poisoned lock must never become an allow.
            Err(_) => {
                return Err(RateLimitError {
                    scope: RateLimitScope::Global,
                    code: RateLimitError::CODE,
                    reason: "rate_limiter_unavailable",
                    retry_after_ms: 1_000,
                });
            }
        };

        evict_expired(&mut state.buckets, now_ms, self.cfg.ttl_ms);

        // 1) Global tier.
        let mut global = std::mem::replace(
            &mut state.global,
            Bucket {
                tokens: 0.0,
                last_refill_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
        global.refill(
            now_ms.saturating_sub(global.last_refill_ms),
            &self.cfg.global,
        );
        global.last_refill_ms = now_ms;
        global.last_seen_ms = now_ms;
        let global_ok = global.tokens >= 1.0;
        if global_ok {
            global.tokens -= 1.0;
        }
        let global_retry = global.retry_after_ms(&self.cfg.global);
        state.global = global;
        if !global_ok {
            return Err(RateLimitError {
                scope: RateLimitScope::Global,
                code: RateLimitError::CODE,
                reason: "rate_limited",
                retry_after_ms: global_retry,
            });
        }

        // 2) Per `(caller, command)` tier.
        let key = bucket_key(caller, command);
        if !state.buckets.contains_key(&key) && state.buckets.len() >= self.cfg.max_entries {
            evict_lru(&mut state.buckets);
        }
        let entry = state.buckets.entry(key).or_insert_with(|| Bucket {
            tokens: self.cfg.per_caller_command.burst,
            last_refill_ms: now_ms,
            last_seen_ms: now_ms,
        });
        entry.refill(
            now_ms.saturating_sub(entry.last_refill_ms),
            &self.cfg.per_caller_command,
        );
        entry.last_refill_ms = now_ms;
        entry.last_seen_ms = now_ms;
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            return Ok(());
        }
        let retry_after_ms = entry.retry_after_ms(&self.cfg.per_caller_command);
        // The global token was already spent; give it back so a per-key
        // refusal does not also burn the process-wide allowance.
        state.global.tokens = (state.global.tokens + 1.0).min(self.cfg.global.burst);
        Err(RateLimitError {
            scope: RateLimitScope::CallerCommand,
            code: RateLimitError::CODE,
            reason: "rate_limited",
            retry_after_ms,
        })
    }

    /// [`Self::check_at`] stamped with the current epoch-ms clock.
    pub fn check(&self, caller: &str, command: &str) -> Result<(), RateLimitError> {
        self.check_at(caller, command, now_ms())
    }

    /// Drop every tracked bucket and reset the global tier to its full burst.
    /// Explicit, audited lifecycle (there is no implicit reset on failure).
    pub fn reset(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.buckets.clear();
            s.global.tokens = self.cfg.global.burst;
        }
    }
}

fn bucket_key(caller: &str, command: &str) -> String {
    // One separator that cannot appear in a Tauri command name or a session
    // id, so `("a:b", "c")` and `("a", "b:c")` cannot collide.
    format!("{caller}\u{1f}{command}")
}

fn evict_expired(buckets: &mut HashMap<String, Bucket>, now_ms: u64, ttl_ms: u64) {
    if ttl_ms == 0 {
        return;
    }
    buckets.retain(|_, b| now_ms.saturating_sub(b.last_seen_ms) < ttl_ms);
}

fn evict_lru(buckets: &mut HashMap<String, Bucket>) {
    let oldest = buckets
        .iter()
        .min_by(|a, b| {
            a.1.last_seen_ms.cmp(&b.1.last_seen_ms).then_with(|| {
                a.1.tokens
                    .partial_cmp(&b.1.tokens)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .map(|(k, _)| k.clone());
    if let Some(k) = oldest {
        buckets.remove(&k);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> RateLimiter {
        RateLimiter::new(RateLimitConfig {
            global: Limit::new(100, 0.0),
            per_caller_command: Limit::new(3, 0.0),
            ttl_ms: 60_000,
            max_entries: 64,
        })
    }

    #[test]
    fn admits_up_to_the_burst_then_refuses_with_a_typed_error() {
        let rl = limiter();
        for i in 0..3 {
            assert!(rl.check_at("ui", "fs_read_file", 1_000).is_ok(), "call {i}");
        }
        let err = rl
            .check_at("ui", "fs_read_file", 1_000)
            .expect_err("fourth call exceeds the burst of 3");
        assert_eq!(err.scope, RateLimitScope::CallerCommand);
        assert_eq!(err.code(), "Unavailable");
        assert!(err.retryable());
        assert_eq!(err.reason, "rate_limited");
    }

    /// The per-key budget is per command: a noisy command must not throttle a
    /// different one on the same caller.
    #[test]
    fn the_budget_is_per_caller_and_per_command() {
        let rl = limiter();
        for _ in 0..3 {
            assert!(rl.check_at("ui", "fs_read_file", 1_000).is_ok());
        }
        assert!(rl.check_at("ui", "fs_read_file", 1_000).is_err());
        // A different command on the same caller still has its own budget.
        assert!(rl.check_at("ui", "terminal_status", 1_000).is_ok());
        // A different caller has its own budget too.
        assert!(rl.check_at("sidecar", "fs_read_file", 1_000).is_ok());
    }

    /// One caller's flood must not spend the whole process-wide allowance.
    #[test]
    fn a_noisy_caller_cannot_starve_the_control_plane() {
        let rl = limiter();
        for _ in 0..3 {
            let _ = rl.check_at("noisy", "terminal_run", 1_000);
        }
        // The per-key refusal returns the global token it borrowed.
        for _ in 0..3 {
            let _ = rl.check_at("noisy", "terminal_run", 1_000);
        }
        assert!(rl.check_at("quiet", "fs_read_file", 1_000).is_ok());
    }

    #[test]
    fn the_global_tier_stops_a_whole_surface_flood() {
        let rl = RateLimiter::new(RateLimitConfig {
            global: Limit::new(4, 0.0),
            per_caller_command: Limit::new(100, 0.0),
            ttl_ms: 60_000,
            max_entries: 64,
        });
        for _ in 0..4 {
            assert!(rl.check_at("ui", "fs_read_file", 1_000).is_ok());
        }
        let err = rl.check_at("ui", "fs_list_dir", 1_000).unwrap_err();
        assert_eq!(err.scope, RateLimitScope::Global);
        assert_eq!(err.code(), "Unavailable");
    }

    #[test]
    fn the_bucket_refills_over_time() {
        let rl = RateLimiter::new(RateLimitConfig {
            global: Limit::new(100, 0.0),
            per_caller_command: Limit::new(1, 10.0),
            ttl_ms: 60_000,
            max_entries: 64,
        });
        assert!(rl.check_at("ui", "chat_stream", 1_000).is_ok());
        assert!(rl.check_at("ui", "chat_stream", 1_000).is_err());
        // 100 ms at 10/s = exactly one token.
        assert!(rl.check_at("ui", "chat_stream", 1_100).is_ok());
        assert!(rl.check_at("ui", "chat_stream", 1_100).is_err());
        // The refill saturates at the burst ceiling — idle time is not banked.
        assert!(rl.check_at("ui", "chat_stream", 9_000).is_ok());
        assert!(rl.check_at("ui", "chat_stream", 9_000).is_err());
    }

    /// A zero-refill bucket reports a positive wait rather than a divide by
    /// zero or a silent 0 ms busy-loop hint.
    #[test]
    fn a_zero_rate_bucket_reports_an_honest_wait() {
        let rl = limiter();
        for _ in 0..3 {
            let _ = rl.check_at("ui", "fs_read_file", 1_000);
        }
        let err = rl.check_at("ui", "fs_read_file", 1_000).unwrap_err();
        assert_eq!(err.retry_after_ms, u64::MAX);
    }

    /// The memory bound: a churning key space can never grow the map past the
    /// configured cap, and eviction drops the least-recently-used bucket.
    #[test]
    fn the_key_space_is_bounded_and_evicts_the_least_recently_used() {
        let rl = RateLimiter::new(RateLimitConfig {
            global: Limit::new(1_000_000, 0.0),
            per_caller_command: Limit::new(1_000, 0.0),
            ttl_ms: 600_000,
            max_entries: 4,
        });
        // `keep` is touched repeatedly so it is never the LRU victim.
        for i in 0..64u64 {
            let _ = rl.check_at("keep", "cmd", 1_000 + i);
            let _ = rl.check_at(&format!("c{i}"), "cmd", 1_000 + i);
            assert!(rl.tracked_keys() <= 4, "grew to {}", rl.tracked_keys());
        }
        assert_eq!(rl.tracked_keys(), 4);
        let state = rl.state.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            state.buckets.contains_key(&bucket_key("keep", "cmd")),
            "the hot key was evicted as LRU"
        );
        drop(state);
        // The hot key kept its budget across the churn.
        assert!(rl.check_at("keep", "cmd", 2_000).is_ok());
    }

    /// Idle buckets are dropped by the TTL, so a churn that stops leaves no
    /// residue.
    #[test]
    fn idle_buckets_expire() {
        let rl = RateLimiter::new(RateLimitConfig {
            global: Limit::new(1_000_000, 0.0),
            per_caller_command: Limit::new(10, 0.0),
            ttl_ms: 1_000,
            max_entries: 64,
        });
        for i in 0..10 {
            let _ = rl.check_at(&format!("c{i}"), "cmd", 1_000);
        }
        assert_eq!(rl.tracked_keys(), 10);
        let _ = rl.check_at("later", "cmd", 10_000);
        assert_eq!(rl.tracked_keys(), 1);
    }

    #[test]
    fn a_backwards_clock_cannot_grant_free_capacity() {
        let rl = RateLimiter::new(RateLimitConfig {
            global: Limit::new(100, 0.0),
            per_caller_command: Limit::new(2, 5.0),
            ttl_ms: 60_000,
            max_entries: 64,
        });
        assert!(rl.check_at("ui", "cmd", 5_000).is_ok());
        assert!(rl.check_at("ui", "cmd", 5_000).is_ok());
        assert!(rl.check_at("ui", "cmd", 5_000).is_err());
        // A stamp that moves backwards refills nothing (saturating sub).
        assert!(rl.check_at("ui", "cmd", 1_000).is_err());
    }

    #[test]
    fn reset_clears_the_budget_explicitly() {
        let rl = limiter();
        for _ in 0..3 {
            let _ = rl.check_at("ui", "fs_read_file", 1_000);
        }
        assert!(rl.check_at("ui", "fs_read_file", 1_000).is_err());
        rl.reset();
        assert!(rl.check_at("ui", "fs_read_file", 1_000).is_ok());
    }

    /// The default desktop posture must not throttle an ordinary turn: a full
    /// burst of control-plane calls is admitted, and a pause refills.
    #[test]
    fn the_default_config_admits_an_ordinary_turn() {
        let cfg = RateLimitConfig::default();
        let rl = RateLimiter::with_defaults();
        // A burst (chat chunks, file reads, a status poll) passes untouched.
        for i in 0..cfg.per_caller_command.burst as u64 {
            assert!(
                rl.check_at("ui", "chat_stream", 1_000 + i * 10).is_ok(),
                "call {i} must pass"
            );
        }
        // A pause long enough to refill the bucket admits the next burst.
        let refill_ms =
            (cfg.per_caller_command.burst as f64 / cfg.per_caller_command.refill_per_sec * 1000.0)
                .ceil() as u64;
        let base = 1_000 + cfg.per_caller_command.burst as u64 * 10 + refill_ms;
        for i in 0..cfg.per_caller_command.burst as u64 {
            assert!(
                rl.check_at("ui", "chat_stream", base + i * 10).is_ok(),
                "refilled call {i} must pass"
            );
        }
    }

    /// The real control is the steady rate: a runaway loop hammering one
    /// command is refused once the burst is spent, and the refusal reports a
    /// concrete backoff rather than a vague limit.
    #[test]
    fn the_default_config_stops_a_runaway_loop() {
        let cfg = RateLimitConfig::default();
        let rl = RateLimiter::with_defaults();
        let burst = cfg.per_caller_command.burst as u64;
        for _ in 0..burst {
            assert!(rl.check_at("ui", "fs_read_file", 1_000).is_ok());
        }
        // Immediately after the burst is spent, the call is refused.
        let err = rl
            .check_at("ui", "fs_read_file", 1_000)
            .expect_err("a runaway loop must be stopped");
        assert_eq!(err.code(), "Unavailable");
        // …and the backoff is a real, positive number of milliseconds.
        assert!(
            err.retry_after_ms > 0 && err.retry_after_ms < 60_000,
            "{err}"
        );
    }
}
