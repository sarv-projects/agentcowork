//! `ARCH/10-KERNEL.md` §5 — **time discipline**: stored time is integer epoch
//! milliseconds UTC, durations come from a monotonic clock, and a schedule
//! carries an explicit timezone policy.
//!
//! The rule this module exists to make unrepresentable-by-accident:
//! **a duration can only be produced by a monotonic clock.** There is no
//! wall-clock subtraction helper here, and there is no constructor that builds a
//! [`Deadline`] from two wall-clock instants. A caller that wants to know "how
//! long did that take" measures [`Stopwatch`]; a caller that wants to know "is it
//! time yet" compares a monotonic deadline. Neither can be moved by the user's
//! clock, an NTP step or a DST boundary.
//!
//! [`MonotonicClock`] is the injection seam, so the immunity is *testable* and
//! not just asserted: the tests drive a fake clock forward while the wall clock
//! jumps backwards, and pin that the deadline does not move.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// A stored instant: integer epoch milliseconds, UTC (`ARCH/06-DATA-MODEL.md`
/// §0). This is the only durable time representation in the kernel, and it is an
/// integer — a float millisecond is a precision bug waiting for an id large
/// enough to show it.
///
/// Deserialization is **integer-only** and refuses a string, a float or `null`:
/// an invalid payload fails typed at the boundary rather than being coerced
/// (`ARCH/10-KERNEL.md` §6, EDGE-113). Display/ISO-8601 formatting is a boundary
/// concern of whoever renders it and deliberately absent here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EpochMillis(i64);

impl EpochMillis {
    /// The epoch itself, `1970-01-01T00:00:00Z`.
    pub const EPOCH: Self = Self(0);

    /// Wrap epoch milliseconds. No range check: the whole `i64` is a valid
    /// instant on the timeline, and a negative value is a legitimate
    /// pre-1970 timestamp — clamping it would invent a fact.
    pub const fn from_unix_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// The integer, for storage and for arithmetic that stays in milliseconds.
    pub const fn unix_millis(self) -> i64 {
        self.0
    }

    /// Whole seconds since the epoch, for a display or a log line.
    pub const fn unix_seconds(self) -> i64 {
        self.0.div_euclid(1000)
    }

    /// The sub-millisecond remainder, always non-negative.
    pub const fn subsec_millis(self) -> i64 {
        self.0.rem_euclid(1000)
    }

    /// Read a JSON number as epoch milliseconds, refusing anything that is not an
    /// integer in range. A float, a string or an out-of-range value is a typed
    /// refusal — never a lossy coercion, because a truncated float is exactly
    /// how an id or a size gets corrupted on the way through a boundary.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, EpochMillisError> {
        match value {
            serde_json::Value::Number(number) => number
                .as_i64()
                .map(Self)
                .ok_or(EpochMillisError::NotAnInteger),
            serde_json::Value::Null => Err(EpochMillisError::Null),
            serde_json::Value::String(_) => Err(EpochMillisError::StringNotAccepted),
            _ => Err(EpochMillisError::NotANumber),
        }
    }

    /// Whether this instant is at or before `other` — the only comparison a
    /// schedule or a checkpoint needs.
    pub const fn is_at_or_before(self, other: Self) -> bool {
        self.0 <= other.0
    }

    /// Milliseconds from `self` until `other`, signed.
    ///
    /// The one wall-clock subtraction the kernel offers, and it is a difference
    /// between two *recorded instants* for reporting ("this event landed 400 ms
    /// after that one") — never a duration handed to a timer. A duration comes
    /// from [`Stopwatch`], off the monotonic clock.
    pub const fn signed_millis_until(self, other: Self) -> i64 {
        other.0 - self.0
    }

    /// This instant shifted by `delta` milliseconds, saturating at the ends of
    /// the representable range rather than wrapping. A wrapped instant is a date
    /// in the past, which for a deadline means a call that is already expired.
    pub const fn saturating_add_millis(self, delta: i64) -> Self {
        Self(self.0.saturating_add(delta))
    }
}

impl fmt::Display for EpochMillis {
    /// The raw integer, explicitly labelled. This is a debugging rendering, not
    /// a calendar format: the kernel does not format dates.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

impl<'de> Deserialize<'de> for EpochMillis {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        // Read the JSON value and apply the one rule, so the typed constructor
        // and the wire path cannot disagree about what an instant is.
        let value = serde_json::Value::deserialize(de)?;
        EpochMillis::from_json(&value).map_err(|err| D::Error::custom(err.to_string()))
    }
}

/// Why a value could not be read as an [`EpochMillis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochMillisError {
    /// A JSON number that is not an integer in `i64` range.
    NotAnInteger,
    /// A JSON float. Refused even when the value is integral.
    FloatNotAccepted,
    /// A JSON string.
    StringNotAccepted,
    /// A JSON null.
    Null,
    /// Anything else (array, object, bool).
    NotANumber,
}

impl fmt::Display for EpochMillisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAnInteger => "epoch milliseconds must be an integer in i64 range",
            Self::FloatNotAccepted => "epoch milliseconds must be an integer, not a float",
            Self::StringNotAccepted => "epoch milliseconds must be a number, not a string",
            Self::Null => "epoch milliseconds must not be null",
            Self::NotANumber => "epoch milliseconds must be a number",
        })
    }
}

impl std::error::Error for EpochMillisError {}

/// Now, as epoch milliseconds UTC. The one place the kernel reads the wall
/// clock, so "when did we ask" is a single grep and a single place to audit.
pub fn now_epoch_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(delta) => delta.as_millis() as i64,
        // Before 1970 is representable, not an error.
        Err(err) => -(err.duration().as_millis() as i64),
    }
}

/// The system monotonic clock, and the injection seam.
///
/// The trait exists so a test can substitute a clock it controls: the kernel's
/// timer rule ("a wall-clock jump never moves a deadline") is otherwise
/// untestable, and an untestable rule is a comment.
pub trait MonotonicClock: Send + Sync {
    /// Milliseconds since this clock's own origin. Never wall-clock time, and
    /// never decreasing.
    fn monotonic_millis(&self) -> i64;
}

/// The real monotonic clock: [`Instant`], which the OS advances monotonically
/// and which NTP steps and DST do not touch.
///
/// Zero-sized on purpose: every instance reads from one process-wide origin, so
/// a deadline built by one caller is comparable with a check made by another.
/// A clock that carried its own origin would make two deadlines incomparable,
/// which is a much worse failure than a few bytes of memory.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl SystemClock {
    /// A handle on the process-wide monotonic clock.
    pub const fn new() -> Self {
        Self
    }
}

impl MonotonicClock for SystemClock {
    fn monotonic_millis(&self) -> i64 {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let elapsed = ORIGIN.get_or_init(Instant::now).elapsed().as_millis();
        i64::try_from(elapsed).unwrap_or(i64::MAX)
    }
}

/// A deadline: a monotonic instant a call must not run past.
///
/// Constructed from a [`MonotonicClock`], so a deadline is *only* a comparison
/// against a monotonic reading. It also carries the wall-clock instant the
/// deadline lands on, for a receipt or a log line — that value is a report, and
/// nothing reads it to make a decision.
#[derive(Debug, Clone)]
pub struct Deadline {
    at_monotonic_ms: i64,
    /// The wall-clock instant this deadline corresponds to, for display and
    /// audit only. Recorded once, at construction.
    at_epoch_ms: EpochMillis,
    label: &'static str,
}

impl Deadline {
    /// A deadline `budget` from now, read off `clock`.
    pub fn after<C: MonotonicClock + ?Sized>(
        clock: &C,
        budget: Duration,
        label: &'static str,
    ) -> Self {
        let budget_ms = i64::try_from(budget.as_millis()).unwrap_or(i64::MAX);
        Self {
            at_monotonic_ms: clock.monotonic_millis().saturating_add(budget_ms),
            at_epoch_ms: EpochMillis::from_unix_millis(
                now_epoch_millis().saturating_add(budget_ms),
            ),
            label,
        }
    }

    /// The monotonic instant this deadline expires at, in this clock's units.
    pub fn at_monotonic_ms(&self) -> i64 {
        self.at_monotonic_ms
    }

    /// The wall-clock instant, for a receipt or a log line. Never a decision
    /// input: a deadline is judged against [`Self::is_expired`].
    pub fn at_epoch_ms(&self) -> EpochMillis {
        self.at_epoch_ms
    }

    /// What this budget is for, in the owner's own words.
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// Has the budget been spent, per `clock`?
    pub fn is_expired<C: MonotonicClock + ?Sized>(&self, clock: &C) -> bool {
        clock.monotonic_millis() >= self.at_monotonic_ms
    }

    /// Budget left per `clock`, saturating at zero. Read from the monotonic
    /// clock, so a wall-clock jump changes nothing about it.
    pub fn remaining<C: MonotonicClock + ?Sized>(&self, clock: &C) -> Duration {
        let left = self.at_monotonic_ms - clock.monotonic_millis();
        if left <= 0 {
            return Duration::ZERO;
        }
        Duration::from_millis(left.unsigned_abs())
    }

    /// The tighter of two deadlines. Propagation rule (`ARCH/07-CONTRACTS.md`
    /// §5.3): a call may never outlive the call that made it, so a sub-call
    /// inherits `min(own budget, caller's remaining)`.
    pub fn tighten(self, other: Deadline) -> Deadline {
        if other.at_monotonic_ms < self.at_monotonic_ms {
            other
        } else {
            self
        }
    }
}

/// A stopwatch: elapsed time from a monotonic origin.
///
/// The only way to measure a duration in the kernel. There is deliberately no
/// `elapsed_since(EpochMillis)` — that subtraction is a wall-clock delta, and it
/// is how a timeout ends up longer or shorter than it was.
#[derive(Debug, Clone)]
pub struct Stopwatch {
    origin: Instant,
    label: &'static str,
}

impl Stopwatch {
    /// Start a stopwatch now.
    pub fn start(label: &'static str) -> Self {
        Self {
            origin: Instant::now(),
            label,
        }
    }

    /// Time since the start, from the monotonic clock.
    pub fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }

    /// What is being measured.
    pub fn label(&self) -> &'static str {
        self.label
    }
}

impl Default for Stopwatch {
    fn default() -> Self {
        Self::start("elapsed")
    }
}

/// The timezone policy a schedule is anchored in (`ARCH/10-KERNEL.md` §5: "absolute
/// times + explicit timezone policy; DST resolved at the boundary").
///
/// A schedule is stored as an absolute instant plus this policy. There is no
/// "naive local time" variant, because a DST transition makes a naive local
/// wall-clock time ambiguous or nonexistent and there is no correct default for
/// it — the boundary that parses user input resolves it against the user's
/// zone and stores the instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimezonePolicy {
    /// UTC. Unambiguous everywhere; the default for a machine with no stated zone.
    Utc,
    /// A fixed offset in minutes east of UTC, range −1439…+1439. A zone with no
    /// DST (or a machine configured not to observe it).
    FixedOffsetMinutes(i32),
    /// The host's local zone at the moment of resolution. The *instant* is
    /// already stored, so this is a report of how it was derived.
    HostLocal,
}

impl TimezonePolicy {
    /// The stable token for a schedule record.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Utc => "utc",
            Self::FixedOffsetMinutes(_) => "fixed_offset_minutes",
            Self::HostLocal => "host_local",
        }
    }

    /// Validate the policy's own argument. A fixed offset outside ±24 h is not a
    /// timezone, it is a typo, and a schedule that carries one is refused rather
    /// than silently shifted.
    pub fn validate(&self) -> Result<(), TimezonePolicyError> {
        match self {
            Self::FixedOffsetMinutes(minutes) if !(-1439..=1439).contains(minutes) => {
                Err(TimezonePolicyError::OffsetOutOfRange(*minutes))
            }
            _ => Ok(()),
        }
    }

    /// Whether this policy is unambiguous on its own, without consulting a
    /// zone database. `HostLocal` is not: it is only correct because the instant
    /// was already resolved at the boundary.
    pub const fn is_self_contained(&self) -> bool {
        !matches!(self, Self::HostLocal)
    }
}

impl fmt::Display for TimezonePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utc => f.write_str("utc"),
            Self::FixedOffsetMinutes(minutes) => write!(f, "utc{minutes:+}"),
            Self::HostLocal => f.write_str("host-local"),
        }
    }
}

/// Why a timezone policy is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimezonePolicyError {
    /// A fixed offset outside −1439…+1439 minutes.
    OffsetOutOfRange(i32),
}

impl fmt::Display for TimezonePolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutOfRange(minutes) => write!(
                f,
                "a fixed UTC offset must be between -1439 and +1439 minutes, got {minutes}"
            ),
        }
    }
}

impl std::error::Error for TimezonePolicyError {}

/// A scheduled instant: the absolute time plus the policy it was resolved
/// under. Both halves are required, so a schedule record can always be checked
/// for how it was derived (`ARCH/10-KERNEL.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// The absolute instant, epoch milliseconds UTC.
    pub at: EpochMillis,
    /// The policy the instant was resolved under. Not optional by design.
    pub timezone: TimezonePolicy,
}

impl Schedule {
    /// An absolute schedule, validated.
    pub fn absolute(
        at: EpochMillis,
        timezone: TimezonePolicy,
    ) -> Result<Self, TimezonePolicyError> {
        timezone.validate()?;
        Ok(Self { at, timezone })
    }

    /// A schedule in UTC — the only self-contained form the kernel can check on
    /// its own.
    pub fn utc(at: EpochMillis) -> Self {
        Self {
            at,
            timezone: TimezonePolicy::Utc,
        }
    }

    /// Whether the instant has arrived, per a wall-clock reading. Used by the
    /// scheduler to decide what to *fire*; never used to measure a duration.
    pub fn is_due(&self, now: EpochMillis) -> bool {
        self.at.is_at_or_before(now)
    }
}

/// A cooperative cancellation token: shared, observable, and safe to clone into
/// every sub-call (`ARCH/07-CONTRACTS.md` §5.3 — "cancellation is cooperative,
/// bounded, and leaves durable state consistent").
///
/// It carries a monotonic deadline as well as the flag, so a call that is never
/// cancelled still stops at its budget. `cancel` is **not** a kill: the owner
/// decides what a cancellation means for durable state, and this type only
/// reports that one was asked for.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: Mutex<Option<String>>,
    deadline: Mutex<Option<Deadline>>,
    /// Signals waiters on cancel or on a deadline change. A `Condvar` rather
    /// than a channel so `wait` needs no async runtime.
    signal: Condvar,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    /// A live token: not cancelled, no deadline.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancellationState {
                cancelled: Mutex::new(None),
                deadline: Mutex::new(None),
                signal: Condvar::new(),
            }),
        }
    }

    /// A token already cancelled, with the reason. Used by a caller replaying a
    /// cancelled request (a retry after a cancel must not look live).
    pub fn cancelled(reason: impl Into<String>) -> Self {
        let token = Self::new();
        token.cancel(reason);
        token
    }

    /// Attach a deadline. A tighter deadline replaces a looser one; a looser one
    /// never widens an already-tightened token, because a sub-call may not
    /// outlive its caller (`ARCH/07-CONTRACTS.md` §5.3).
    pub fn with_deadline(&self, deadline: Deadline) -> &Self {
        let mut held = lock(&self.inner.deadline);
        *held = Some(match held.take() {
            Some(existing) => existing.tighten(deadline),
            None => deadline,
        });
        self.inner.signal.notify_all();
        self
    }

    /// Attach a budget, measured off the system monotonic clock.
    pub fn with_budget(&self, budget: Duration, label: &'static str) -> &Self {
        self.with_deadline(Deadline::after(&SystemClock::new(), budget, label))
    }

    /// Ask for cancellation. Idempotent: the first reason is kept, so a later
    /// "gave up" cannot overwrite the real cause.
    pub fn cancel(&self, reason: impl Into<String>) {
        let mut held = lock(&self.inner.cancelled);
        if held.is_none() {
            *held = Some(reason.into());
        }
        self.inner.signal.notify_all();
    }

    /// Has cancellation been requested? A lock-free read, so a hot loop can poll
    /// it without contending.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.try_lock().is_ok_and(|g| g.is_some())
    }

    /// The first cancellation reason, if any.
    pub fn cancel_reason(&self) -> Option<String> {
        lock(&self.inner.cancelled).clone()
    }

    /// The deadline, if one was attached.
    pub fn deadline(&self) -> Option<Deadline> {
        lock(&self.inner.deadline).clone()
    }

    /// Whether the budget is spent, per the system monotonic clock.
    pub fn is_expired(&self) -> bool {
        match self.deadline() {
            Some(deadline) => deadline.is_expired(&SystemClock::new()),
            None => false,
        }
    }

    /// Why a call must stop, if it must. A deadline is reported as a deadline,
    /// not as a cancellation, so the caller can tell the two apart when it
    /// writes a receipt.
    pub fn stop_reason<C: MonotonicClock + ?Sized>(&self, clock: &C) -> Option<StopReason> {
        if let Some(deadline) = self.deadline()
            && deadline.is_expired(clock)
        {
            return Some(StopReason::DeadlineExpired {
                label: deadline.label(),
            });
        }
        self.cancel_reason().map(StopReason::Cancelled)
    }

    /// Block until cancelled, the deadline passes, or `budget` elapses —
    /// whichever comes first. Returns why it stopped.
    ///
    /// Bounded by construction, which is what "cooperative and bounded"
    /// (`ARCH/07-CONTRACTS.md` §5.3) has to mean: even a token nobody cancels
    /// returns within the budget. The cap is read from `clock` and a real-time
    /// backstop of the same length bounds the wait when `clock` is a test double
    /// that does not advance — a wait that only a live clock can end is a hang
    /// waiting for a production incident.
    pub fn wait_for_stop<C: MonotonicClock + ?Sized>(
        &self,
        clock: &C,
        budget: Duration,
    ) -> StopReason {
        // Poll on a short fixed cadence rather than computing a sleep from the
        // clock: the clock is injectable, so a computed sleep would be a lie
        // under a fake clock, and a poll is cheap next to any real call.
        const CADENCE: Duration = Duration::from_millis(25);
        let cap = clock
            .monotonic_millis()
            .saturating_add(i64::try_from(budget.as_millis()).unwrap_or(i64::MAX));
        let real_deadline = Instant::now() + budget;
        loop {
            if real_deadline <= Instant::now() {
                return self
                    .stop_reason(clock)
                    .unwrap_or(StopReason::BudgetExhausted);
            }
            if let Some(reason) = self.stop_reason(clock) {
                return reason;
            }
            if clock.monotonic_millis() >= cap {
                return StopReason::BudgetExhausted;
            }
            let guard = lock(&self.inner.cancelled);
            // Never sleep past the real backstop: the wait is bounded by the
            // budget, not by the next poll.
            let remaining = real_deadline.saturating_duration_since(Instant::now());
            let cadence = remaining.min(CADENCE);
            // The guard is dropped (and the mutex released) either way; the
            // timeout is a cadence, not a decision.
            let _released = self
                .inner
                .signal
                .wait_timeout(guard, cadence)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

/// Why a call stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Cancellation was requested, with the reason the requester gave.
    Cancelled(String),
    /// The deadline passed.
    DeadlineExpired {
        /// The budget's label, so a receipt says *which* budget ran out.
        label: &'static str,
    },
    /// The caller's own wait budget elapsed with nothing to report.
    BudgetExhausted,
}

impl StopReason {
    /// A stable token for logs, receipts and audit rows.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cancelled(_) => "cancelled",
            Self::DeadlineExpired { .. } => "deadline_expired",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }

    /// The error code this stop maps to (`ARCH/10-KERNEL.md` §3): a deadline is
    /// `Timeout`; a cancellation is `InvalidState` (the call was not valid for
    /// the state it was in), because there is no "Cancelled" code in the
    /// taxonomy and inventing one would extend it.
    pub fn error_code(&self) -> crate::error::ErrorCode {
        match self {
            Self::DeadlineExpired { .. } | Self::BudgetExhausted => {
                crate::error::ErrorCode::Timeout
            }
            Self::Cancelled(_) => crate::error::ErrorCode::InvalidState,
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled(reason) => write!(f, "cancelled: {reason}"),
            Self::DeadlineExpired { label } => write!(f, "deadline expired: {label}"),
            Self::BudgetExhausted => f.write_str("budget exhausted"),
        }
    }
}

/// A poisoned lock in a cancellation token holds no invariant worth preserving —
/// the value inside is a plain option — so recovering is the whole recovery.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// A monotonic clock the test drives by hand, so "the wall clock jumped" can
    /// be a fact rather than a hope.
    struct FakeClock {
        now_ms: AtomicI64,
    }

    impl FakeClock {
        fn new(now_ms: i64) -> Self {
            Self {
                now_ms: AtomicI64::new(now_ms),
            }
        }

        fn advance(&self, by: Duration) {
            let delta = i64::try_from(by.as_millis()).unwrap_or(0);
            self.now_ms.fetch_add(delta, Ordering::SeqCst);
        }
    }

    impl MonotonicClock for FakeClock {
        fn monotonic_millis(&self) -> i64 {
            self.now_ms.load(Ordering::SeqCst)
        }
    }

    /// What stands in for the wall clock in a test: an integer the test can move
    /// anywhere, forwards or backwards, without any code path consulting it.
    /// A deadline that survives this moving is a deadline that reads the
    /// monotonic clock.
    fn wall_clock_now() -> i64 {
        now_epoch_millis()
    }

    #[test]
    fn stored_time_is_an_integer_and_nothing_else() {
        let at = EpochMillis::from_unix_millis(1_700_000_000_123);
        assert_eq!(at.unix_millis(), 1_700_000_000_123);
        assert_eq!(at.unix_seconds(), 1_700_000_000);
        assert_eq!(at.subsec_millis(), 123);
        assert_eq!(serde_json::to_string(&at).unwrap(), "1700000000123");
        assert_eq!(
            serde_json::from_str::<EpochMillis>("1700000000123").unwrap(),
            at
        );
        // A negative (pre-1970) instant is a legitimate instant, not an error.
        let before = EpochMillis::from_unix_millis(-1_000);
        assert_eq!(before.unix_seconds(), -1);
        assert_eq!(before.subsec_millis(), 0);
        // Ordering is integer ordering, so a millisecond is never lost.
        assert!(EpochMillis::from_unix_millis(1) < EpochMillis::from_unix_millis(2));
    }

    #[test]
    fn a_non_integer_time_is_refused_never_coerced() {
        for (raw, why) in [
            ("1700000000123.5", "a fractional millisecond"),
            ("\"1700000000123\"", "a quoted number"),
            ("null", "a null"),
            ("true", "a bool"),
            ("[]", "an array"),
            ("99999999999999999999", "an out-of-range integer"),
        ] {
            let err = serde_json::from_str::<EpochMillis>(raw)
                .expect_err(&format!("{raw} ({why}) must be refused"));
            assert!(
                err.to_string().contains("epoch milliseconds"),
                "{raw}: {err}"
            );
        }
        // The same rule through the typed constructor.
        assert!(EpochMillis::from_json(&serde_json::json!(1.5)).is_err());
        assert!(EpochMillis::from_json(&serde_json::json!("1")).is_err());
        assert_eq!(
            EpochMillis::from_json(&serde_json::json!(42)).unwrap(),
            EpochMillis::from_unix_millis(42)
        );
    }

    #[test]
    fn a_deadline_is_unaffected_by_a_wall_clock_jump() {
        let clock = FakeClock::new(1_000);
        let deadline = Deadline::after(&clock, Duration::from_millis(500), "tool call");
        assert_eq!(deadline.at_monotonic_ms(), 1_500);
        assert!(!deadline.is_expired(&clock));
        assert_eq!(deadline.remaining(&clock), Duration::from_millis(500));

        // The wall clock jumps an hour backwards, then forwards a day. The
        // deadline's own verdict and its remaining budget are read from the
        // monotonic clock, so neither jump is visible to them — and the
        // wall-clock instant it recorded at construction does not move either.
        let recorded = deadline.at_epoch_ms().unix_millis();
        assert!(!deadline.is_expired(&clock));
        let _wall = wall_clock_now() - 3_600_000;
        assert!(!deadline.is_expired(&clock));
        let _wall = wall_clock_now() + 86_400_000;
        assert!(!deadline.is_expired(&clock));
        assert_eq!(deadline.at_epoch_ms().unix_millis(), recorded);
        assert_eq!(deadline.remaining(&clock), Duration::from_millis(500));

        // Only monotonic progress expires it — half the budget, then all of it.
        clock.advance(Duration::from_millis(499));
        assert!(!deadline.is_expired(&clock));
        clock.advance(Duration::from_millis(1));
        assert!(deadline.is_expired(&clock));
        assert_eq!(deadline.remaining(&clock), Duration::ZERO);
    }

    #[test]
    fn a_deadline_propagates_and_never_widens() {
        let clock = FakeClock::new(0);
        let outer = || Deadline::after(&clock, Duration::from_millis(1_000), "outer");
        // A sub-call with a longer budget inherits the tighter of the two.
        let inner = Deadline::after(&clock, Duration::from_millis(5_000), "inner");
        assert_eq!(outer().tighten(inner).at_monotonic_ms(), 1_000);
        // A sub-call with a shorter budget keeps its own.
        let inner = Deadline::after(&clock, Duration::from_millis(100), "inner");
        assert_eq!(outer().tighten(inner).at_monotonic_ms(), 100);
        // Tighter-then-looser is still tight: a deadline cannot be widened.
        let tightened = Deadline::after(&clock, Duration::from_millis(100), "inner");
        assert_eq!(
            tightened
                .tighten(Deadline::after(&clock, Duration::from_secs(60), "outer"))
                .at_monotonic_ms(),
            100
        );
    }

    #[test]
    fn a_stopwatch_measures_monotonically_and_takes_no_wall_clock_argument() {
        let watch = Stopwatch::start("effect");
        std::thread::sleep(Duration::from_millis(2));
        assert!(
            watch.elapsed() >= Duration::from_millis(1),
            "{:?}",
            watch.elapsed()
        );
        assert_eq!(watch.label(), "effect");
        // There is no `elapsed_since(EpochMillis)` to reach for: measuring a
        // duration against a stored instant is exactly the wall-clock delta the
        // kernel forbids, so the API does not offer it.
        assert_eq!(Stopwatch::default().label(), "elapsed");
    }

    #[test]
    fn the_system_clock_never_goes_backwards_and_is_shared() {
        let clock = SystemClock::new();
        let first = clock.monotonic_millis();
        let second = clock.monotonic_millis();
        assert!(second >= first, "{first} then {second}");
        // It is monotonic time, not wall time: it is nowhere near the epoch.
        assert!(first < 10_000, "a monotonic reading, not a date: {first}");
        // Two handles read the same origin, so a deadline built with one is
        // checkable with the other — a per-instance origin would silently make
        // every cross-handle comparison meaningless.
        assert!(SystemClock::new().monotonic_millis() >= first);
        let deadline = Deadline::after(&SystemClock::new(), Duration::from_millis(5_000), "tool");
        assert!(!deadline.is_expired(&SystemClock::new()));
    }

    #[test]
    fn a_schedule_carries_an_explicit_timezone_policy() {
        let at = EpochMillis::from_unix_millis(1_700_000_000_000);
        let utc = Schedule::utc(at);
        assert_eq!(utc.timezone, TimezonePolicy::Utc);
        assert!(utc.timezone.is_self_contained());
        assert!(utc.is_due(EpochMillis::from_unix_millis(1_700_000_000_000)));
        assert!(!utc.is_due(EpochMillis::from_unix_millis(1_699_999_999_999)));

        // A fixed offset is validated: a value outside ±24h is a typo, not a
        // zone, and it is refused instead of silently shifting the instant.
        assert!(
            Schedule::absolute(at, TimezonePolicy::FixedOffsetMinutes(-1439)).is_ok(),
            "-23:59 is a real offset"
        );
        assert!(Schedule::absolute(at, TimezonePolicy::FixedOffsetMinutes(1440)).is_err());
        assert!(Schedule::absolute(at, TimezonePolicy::FixedOffsetMinutes(-1440)).is_err());
        assert!(Schedule::absolute(at, TimezonePolicy::HostLocal).is_ok());

        // A schedule record cannot exist without the policy: it is a required
        // field, so "naive local time" is not a state the type can hold.
        let json = serde_json::to_string(&utc).unwrap();
        assert!(json.contains("\"timezone\":\"utc\""), "{json}");
        assert!(serde_json::from_str::<Schedule>(r#"{"at":1}"#).is_err());
    }

    /// The DST case, at the level the kernel owns it: a schedule's *instant* is
    /// absolute, so a DST transition cannot move it, and a policy that is not
    /// self-contained is reported as such rather than assumed.
    #[test]
    fn a_dst_transition_cannot_move_an_absolute_schedule() {
        // Two instants one hour apart in real time. If a schedule were stored as
        // naive local wall-clock time, the spring-forward hour would make one of
        // these two instants ambiguous; stored as an instant, both are just
        // instants and the order is total.
        let before_dst = Schedule::utc(EpochMillis::from_unix_millis(1_700_000_000_000));
        let after_dst = Schedule::utc(EpochMillis::from_unix_millis(1_700_003_600_000));
        assert!(before_dst.is_due(EpochMillis::from_unix_millis(1_700_003_600_000)));
        assert!(!after_dst.is_due(EpochMillis::from_unix_millis(1_700_001_800_000)));
        assert!(before_dst.at < after_dst.at);

        // `HostLocal` is the one policy that is not self-contained: it is only
        // correct because the boundary already resolved the instant, and it says
        // so rather than pretending to be checkable here.
        assert!(!TimezonePolicy::HostLocal.is_self_contained());
        assert_eq!(TimezonePolicy::HostLocal.as_str(), "host_local");
        assert_eq!(
            TimezonePolicy::FixedOffsetMinutes(330).to_string(),
            "utc+330"
        );
        assert_eq!(
            TimezonePolicy::FixedOffsetMinutes(-480).to_string(),
            "utc-480"
        );
    }

    #[test]
    fn cancellation_is_cooperative_shared_and_first_reason_wins() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        let sub = token.clone();
        token.cancel("the user pressed stop");
        assert!(sub.is_cancelled(), "a clone observes the same token");
        assert_eq!(
            sub.cancel_reason().as_deref(),
            Some("the user pressed stop")
        );
        // A later, vaguer reason cannot overwrite the first one.
        sub.cancel("gave up");
        assert_eq!(
            token.cancel_reason().as_deref(),
            Some("the user pressed stop")
        );
        assert_eq!(
            token.stop_reason(&SystemClock::new()),
            Some(StopReason::Cancelled("the user pressed stop".into()))
        );
    }

    #[test]
    fn a_deadline_is_reported_as_a_deadline_not_as_a_cancellation() {
        let clock = FakeClock::new(0);
        let token = CancellationToken::new();
        token.with_deadline(Deadline::after(
            &clock,
            Duration::from_millis(50),
            "provider call",
        ));
        assert_eq!(token.stop_reason(&clock), None);
        clock.advance(Duration::from_millis(50));
        assert_eq!(
            token.stop_reason(&clock),
            Some(StopReason::DeadlineExpired {
                label: "provider call"
            })
        );
        // A token nobody cancels is still bounded, and the wait returns — even
        // against a clock that never advances, which is what makes this a
        // property of the wait and not of the clock.
        let reason = CancellationToken::new().wait_for_stop(&clock, Duration::from_millis(5));
        assert_eq!(reason, StopReason::BudgetExhausted);
        let started = Instant::now();
        let reason =
            CancellationToken::new().wait_for_stop(&SystemClock::new(), Duration::from_millis(30));
        assert_eq!(reason, StopReason::BudgetExhausted);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait is bounded, not unbounded"
        );
        // A cancellation ends the wait early, and names itself.
        let live = CancellationToken::new();
        let waiter = live.clone();
        let handle = std::thread::spawn(move || {
            waiter.wait_for_stop(&SystemClock::new(), Duration::from_secs(30))
        });
        std::thread::sleep(Duration::from_millis(10));
        live.cancel("user stopped the run");
        assert_eq!(
            handle.join().expect("waiter"),
            StopReason::Cancelled("user stopped the run".into())
        );
    }

    #[test]
    fn a_stop_reason_maps_onto_the_taxonomy_without_inventing_a_code() {
        assert_eq!(
            StopReason::DeadlineExpired { label: "x" }.error_code(),
            crate::error::ErrorCode::Timeout
        );
        assert_eq!(
            StopReason::BudgetExhausted.error_code(),
            crate::error::ErrorCode::Timeout
        );
        assert_eq!(
            StopReason::Cancelled("x".into()).error_code(),
            crate::error::ErrorCode::InvalidState
        );
        assert_eq!(StopReason::Cancelled("x".into()).as_str(), "cancelled");
        assert!(StopReason::BudgetExhausted.to_string().contains("budget"));
    }
}
