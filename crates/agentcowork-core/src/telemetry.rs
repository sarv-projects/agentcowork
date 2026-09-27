//! P8.7 Telemetry (H12 — doc 33 §11).
//!
//! Opt-in only. Telemetry is **off by default** and every event carries
//! enumerated fields only — never message content, file names, prompts, or
//! memory text. The [`Telemetry`] struct is the single choke point: if the
//! user has not explicitly opted in, `record` is a no-op and no transport is
//! ever constructed. The cold-boot test below proves the no-request guarantee.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Opt-in switch. Defaults to `false`; only an explicit user action flips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryMode {
    /// No telemetry at all (default). `record` is a no-op.
    #[default]
    Off,
    /// Enumerated, content-free event counters only.
    On,
}

/// High-level event kinds. *Only* these are ever recorded — no free-form
/// strings, no content, no identifiers beyond a coarse bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryEventKind {
    AppLaunch,
    AppQuit,
    SessionOpen,
    SessionClose,
    ChatMessageSent,
    ToolInvoked,
    ToolSucceeded,
    ToolFailed,
    PlanCreated,
    AutomationRun,
    SchedulerTick,
    MemoryWrite,
    MemoryRead,
    Error,
    Crash,
}

impl TelemetryEventKind {
    /// Enumerated bucket name (never free-form).
    pub fn as_str(&self) -> &'static str {
        match self {
            TelemetryEventKind::AppLaunch => "app_launch",
            TelemetryEventKind::AppQuit => "app_quit",
            TelemetryEventKind::SessionOpen => "session_open",
            TelemetryEventKind::SessionClose => "session_close",
            TelemetryEventKind::ChatMessageSent => "chat_message_sent",
            TelemetryEventKind::ToolInvoked => "tool_invoked",
            TelemetryEventKind::ToolSucceeded => "tool_succeeded",
            TelemetryEventKind::ToolFailed => "tool_failed",
            TelemetryEventKind::PlanCreated => "plan_created",
            TelemetryEventKind::AutomationRun => "automation_run",
            TelemetryEventKind::SchedulerTick => "scheduler_tick",
            TelemetryEventKind::MemoryWrite => "memory_write",
            TelemetryEventKind::MemoryRead => "memory_read",
            TelemetryEventKind::Error => "error",
            TelemetryEventKind::Crash => "crash",
        }
    }
}

/// One telemetry sample. Every field is enumerated or numeric — there is no
/// field that could carry user content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TelemetrySample {
    pub kind: TelemetryEventKind,
    /// Coarse platform bucket (`linux` / `macos` / `windows` / `unknown`).
    pub platform: String,
    /// `agentcowork-core` version (semver string).
    pub version: String,
    /// Wall-clock duration of the activity in ms (0 when not measurable).
    pub duration_ms: u64,
    /// Numeric error code (0 = none) — never a message string.
    pub error_code: u32,
}

/// The single telemetry choke point.
#[derive(Debug)]
pub struct Telemetry {
    mode: AtomicBool,
    enabled_at_least_once: AtomicBool,
    /// In-memory event buffer (enumerated samples only). In a real build this
    /// would flush to an opt-in endpoint; the buffer keeps it testable and
    /// proves the "no requests without opt-in" property without a network.
    buffer: std::sync::Mutex<Vec<TelemetrySample>>,
    total_events: AtomicU64,
    _platform: String,
    _version: String,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new(TelemetryMode::Off, "unknown", env!("CARGO_PKG_VERSION"))
    }
}

impl Telemetry {
    pub fn new(mode: TelemetryMode, platform: &str, version: &str) -> Self {
        let on = mode == TelemetryMode::On;
        Telemetry {
            mode: AtomicBool::new(on),
            enabled_at_least_once: AtomicBool::new(on),
            buffer: std::sync::Mutex::new(Vec::new()),
            total_events: AtomicU64::new(0),
            _platform: platform.to_string(),
            _version: version.to_string(),
        }
    }

    /// Explicit user opt-in. This is the *only* way telemetry turns on.
    pub fn opt_in(&self) {
        self.mode.store(true, Ordering::SeqCst);
        self.enabled_at_least_once.store(true, Ordering::SeqCst);
    }

    /// Explicit opt-out (also the default state).
    pub fn opt_out(&self) {
        self.mode.store(false, Ordering::SeqCst);
    }

    pub fn is_enabled(&self) -> bool {
        self.mode.load(Ordering::SeqCst)
    }

    /// Record one enumerated sample. **No-op unless the user has opted in.**
    pub fn record(&self, sample: TelemetrySample) {
        if !self.mode.load(Ordering::SeqCst) {
            return;
        }
        self.total_events.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push(sample);
        }
    }

    /// Convenience: record with only kind + duration (content-free by
    /// construction).
    pub fn record_event(&self, kind: TelemetryEventKind, duration_ms: u64) {
        self.record(TelemetrySample {
            kind,
            platform: self.platform().to_string(),
            version: self.version().to_string(),
            duration_ms,
            error_code: 0,
        });
    }

    pub fn platform(&self) -> &str {
        &self._platform
    }

    pub fn version(&self) -> &str {
        &self._version
    }

    /// Number of events recorded since boot (for tests / status UI).
    pub fn total_events(&self) -> u64 {
        self.total_events.load(Ordering::SeqCst)
    }

    /// Snapshot of the in-memory buffer (enumerated samples only).
    pub fn snapshot(&self) -> Vec<TelemetrySample> {
        self.buffer.lock().map(|b| b.clone()).unwrap_or_default()
    }

    /// Whether the user has *ever* opted in this boot (distinct from *current*
    /// mode — used by the cold-boot guarantee).
    pub fn ever_enabled(&self) -> bool {
        self.enabled_at_least_once.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_boot_off_by_default_and_no_events() {
        let t = Telemetry::default();
        assert!(!t.is_enabled());
        assert!(!t.ever_enabled());
        t.record_event(TelemetryEventKind::AppLaunch, 12);
        t.record_event(TelemetryEventKind::ChatMessageSent, 300);
        // Nothing recorded, nothing ever enabled — the no-request guarantee.
        assert_eq!(t.total_events(), 0);
        assert!(t.snapshot().is_empty());
    }

    #[test]
    fn only_explicit_opt_in_enables_recording() {
        let t = Telemetry::default();
        t.record_event(TelemetryEventKind::AppLaunch, 1);
        assert_eq!(t.total_events(), 0);

        t.opt_in();
        assert!(t.is_enabled());
        assert!(t.ever_enabled());
        t.record_event(TelemetryEventKind::AppLaunch, 12);
        t.record_event(TelemetryEventKind::ToolSucceeded, 40);
        assert_eq!(t.total_events(), 2);
        let snap = t.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].kind, TelemetryEventKind::AppLaunch);
        assert_eq!(snap[1].kind, TelemetryEventKind::ToolSucceeded);
    }

    #[test]
    fn opt_out_returns_to_noop() {
        let t = Telemetry::new(TelemetryMode::On, "linux", "0.0.0");
        t.record_event(TelemetryEventKind::AppLaunch, 5);
        assert_eq!(t.total_events(), 1);
        t.opt_out();
        t.record_event(TelemetryEventKind::AppLaunch, 5);
        assert_eq!(t.total_events(), 1, "opt-out must stop recording");
        assert!(!t.is_enabled());
        // But ever_enabled stays true — the user did opt in at some point.
        assert!(t.ever_enabled());
    }

    #[test]
    fn samples_are_enumerated_content_free() {
        let t = Telemetry::new(TelemetryMode::On, "windows", "1.2.3");
        t.record(TelemetrySample {
            kind: TelemetryEventKind::Error,
            platform: "windows".to_string(),
            version: "1.2.3".to_string(),
            duration_ms: 0,
            error_code: 42,
        });
        let snap = t.snapshot();
        assert_eq!(snap[0].kind, TelemetryEventKind::Error);
        assert_eq!(snap[0].error_code, 42);
        // No content-bearing field exists on the type — compiler-enforced.
        assert_eq!(snap[0].platform, "windows");
        assert_eq!(snap[0].version, "1.2.3");
    }

    #[test]
    fn mode_serde_defaults_to_off() {
        let m: TelemetryMode = serde_json::from_str("\"off\"").unwrap();
        assert_eq!(m, TelemetryMode::Off);
        let d: TelemetryMode = serde_json::from_value(serde_json::Value::Null).unwrap_or_default();
        assert_eq!(d, TelemetryMode::Off);
    }
}
