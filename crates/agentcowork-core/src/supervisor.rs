//! ProcessSupervisor — synchronous child-process supervisor for the TS coordinator sidecar.
//!
//! Spawns the coordinator binary, monitors its exit codes, applies exponential
//! backoff on crashes, and implements a circuit breaker (5 crashes in 10 min →
//! open). A watchdog (J10) kills + restarts on connect/idle timeouts; the idle
//! clock is re-armed **per byte of stream** by dedicated stdout/stderr reader
//! threads, and the sidecar sends a periodic `heartbeat` notification so a
//! healthy-but-idle process is never falsely killed.
//!
//! Designed to run on a dedicated std::thread, NOT the async runtime.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Supervisor states reflecting the lifecycle of the managed child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorState {
    Starting,
    Running,
    Restarting,
    CircuitOpen,
    Stopped,
}

impl std::fmt::Display for SupervisorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "Starting"),
            Self::Running => write!(f, "Running"),
            Self::Restarting => write!(f, "Restarting"),
            Self::CircuitOpen => write!(f, "CircuitOpen"),
            Self::Stopped => write!(f, "Stopped"),
        }
    }
}

/// Watchdog decision (J10) — the *reason* a child would be considered hung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogStatus {
    /// Child is healthy — no action.
    Healthy,
    /// Child produced no output within [`CONNECT_TIMEOUT`] of spawn.
    ConnectTimeout,
    /// No stream activity for longer than [`IDLE_TIMEOUT`] while running.
    IdleTimeout,
}

/// Errors produced by the supervisor.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("failed to spawn coordinator: {0}")]
    SpawnFailed(#[from] io::Error),

    #[error("circuit breaker open — too many crashes in 10 minutes")]
    CircuitOpen,

    #[error("watchdog timeout — child unresponsive")]
    WatchdogTimeout,

    #[error("supervisor killed")]
    Killed,
}

/// Circuit breaker window: crashes older than this are forgotten.
const CIRCUIT_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Maximum crashes within the window before tripping the breaker.
const CIRCUIT_THRESHOLD: usize = 5;

/// Connect timeout: time allowed for the child to emit its first byte after spawn.
/// The sidecar writes a `session/ready` notification on boot, so this is normally
/// satisfied within milliseconds.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Idle timeout: maximum silence (no stdout/stderr bytes) before the watchdog
/// kills + restarts. The sidecar emits a `heartbeat` notification every 10s,
/// so a healthy-but-idle process never trips this.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval for try_wait loop.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum backoff delay in seconds.
const MAX_BACKOFF_SECS: u64 = 60;

/// The ProcessSupervisor manages a single child process (the TS coordinator).
///
/// It is fully synchronous — designed to run on its own `std::thread`.
pub struct ProcessSupervisor {
    /// Handle to the running child process (None when stopped).
    pub child: Option<Child>,
    /// Path to the coordinator binary to spawn.
    pub binary_path: PathBuf,
    /// Current lifecycle state.
    pub state: SupervisorState,
    /// Timestamps of recent crashes for the circuit breaker.
    pub crash_history: VecDeque<Instant>,
    /// Number of consecutive restarts (for exponential backoff).
    pub restart_count: u32,
    /// When the current child was spawned (for connect watchdog).
    pub started_at: Option<Instant>,
    /// UNIX millis of the last byte observed on the child's stdout/stderr.
    /// Shared with the reader threads; `0` = no activity yet.
    pub last_activity_ms: Arc<AtomicU64>,
    /// stdout/stderr reader threads for the current child (exit on pipe EOF).
    pub readers: Vec<std::thread::JoinHandle<()>>,
    /// Windows: Job Object handle with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    /// Kept open for the app's lifetime so the OS kills the child on parent death.
    #[cfg(target_os = "windows")]
    pub job_handle: Option<isize>,
    /// New-link handoff: each successful spawn sends the fresh child's stdin
    /// (write) and stdout (read) ends here so the shell can build a
    /// `SidecarLink`. `None` for the headless coordinator binary (which has no
    /// shell to wire a link to and pumps stdout for the watchdog instead).
    pub link_tx: Option<Sender<(ChildStdin, ChildStdout)>>,
    /// P45.8 — set by the shell when the next turn needs the sidecar.
    /// The supervisor parks on idle-exit (code 75) until this flips.
    pub resume: Arc<AtomicBool>,
    /// True while the child is down after an idle exit.
    pub parked: Arc<AtomicBool>,
}

impl ProcessSupervisor {
    /// Create a new supervisor for the given binary.
    pub fn new(binary_path: PathBuf) -> Self {
        Self::new_with_link(binary_path, None)
    }

    /// Create a supervisor that hands each spawned child's stdio pipes to a
    /// `SidecarLink` via `link_tx` (the Tauri shell path).
    pub fn new_with_link(
        binary_path: PathBuf,
        link_tx: Option<Sender<(ChildStdin, ChildStdout)>>,
    ) -> Self {
        Self {
            child: None,
            binary_path,
            state: SupervisorState::Stopped,
            crash_history: VecDeque::new(),
            restart_count: 0,
            started_at: None,
            last_activity_ms: Arc::new(AtomicU64::new(0)),
            readers: Vec::new(),
            link_tx,
            resume: Arc::new(AtomicBool::new(false)),
            parked: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "windows")]
            job_handle: None,
        }
    }

    /// Spawn the coordinator binary as a child process.
    ///
    /// Sets `BUN_JSC_heapSize` (512 MB), applies platform-specific
    /// orphan-prevention (`pre_exec` on Linux/macOS; a Job Object on Windows,
    /// created *before* the process spawns), and starts stdout/stderr reader
    /// threads that re-arm the watchdog's activity clock on every byte.
    pub fn spawn(&mut self) -> Result<(), SupervisorError> {
        self.state = SupervisorState::Starting;
        // Join reader threads from any previous child BEFORE resetting the
        // activity clock below. A stale thread still draining the dying pipe
        // could otherwise stamp a fresh timestamp and falsely arm the new
        // child's connect timeout. Safe: every path into `spawn()` has a dead
        // or absent previous child, so its pipes are EOF'd and the threads
        // return promptly.
        for handle in self.readers.drain(..) {
            let _ = handle.join();
        }

        // NOTE: do NOT set `BUN_JSC_heapSize` here — Bun ≥1.3 rejects it as an
        // invalid JSC environment variable and the embedded runtime exits(1)
        // before running any app code (verified 2026-08-17 on Bun 1.3.14).
        // Heap pressure is handled by the coordinator's own heap monitor
        // (packages/coordinator/src/heap.ts — self-restart at 80%, J13) plus
        // the clean-rotation path below (exit code 0).
        let mut cmd = Command::new(&self.binary_path);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The coordinator exits 75 after 60s with no turn. This process
            // treats that as a park, not a crash.
            // `EVERYAIOS_SIDECAR_IDLE_EXIT` keeps its legacy spelling on
            // purpose: it is a cross-plane token handed to the TS coordinator
            // child, and the TS side of this rename belongs to the parallel
            // lane — one end must not be renamed without the other (DEC-053
            // Step 4: wire tokens stay).
            .env("EVERYAIOS_SIDECAR_IDLE_EXIT", "1");
        self.parked.store(false, Ordering::Release);

        // Platform-specific pre_exec for orphan prevention.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    // PR_SET_PDEATHSIG: when parent dies, deliver SIGTERM to this child.
                    let ret = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    if ret != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        #[cfg(target_os = "macos")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    // Create a new process group so we can signal the whole group.
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        // Windows: create the Job Object BEFORE the process spawns so there is
        // no window in which the child runs outside the job.
        #[cfg(target_os = "windows")]
        let job = {
            let job = crate::orphan::windows::create_job_object()?;
            self.job_handle = Some(job);
            job
        };

        let mut child = cmd.spawn().map_err(SupervisorError::SpawnFailed)?;

        #[cfg(target_os = "windows")]
        {
            // Assign the fresh child to the job by PID (belt + suspenders vs
            // the pre_exec path Unix platforms use).
            let pid = child.id();
            if let Err(e) = crate::orphan::windows::assign_to_job(job, pid) {
                // Nested-job environments (app launched from inside another
                // Job) return ERROR_ACCESS_DENIED here. The Job Object is the
                // *extra* orphan layer — degrade to a warning rather than
                // failing the spawn; stdin-EOF + ppid polling still backstop.
                eprintln!("[supervisor] warning: could not assign child {pid} to Job Object: {e}");
            }
        }

        // Watchdog activity clock — reset to "no activity yet". The reader
        // threads set it to `now` on the first byte, which is what promotes
        // Starting → Running and arms the idle clock. (Setting it to `now`
        // here would make the connect timeout dead code — `watchdog_status`
        // treats `0` as "never produced a byte".)
        self.last_activity_ms.store(0, Ordering::Relaxed);

        // stderr reader thread: every byte re-arms the idle watchdog and the
        // lines are forwarded to the app's stderr (prefixed) so sidecar
        // failures are visible in the terminal instead of being silently
        // swallowed — the 2026-08-17 "child crashed with code 1" loop was
        // invisible precisely because stderr was discarded.
        let last_stderr = Arc::clone(&self.last_activity_ms);
        if let Some(err) = child.stderr.take() {
            self.readers.push(std::thread::spawn(move || {
                let _ = pump_visible(err, &last_stderr);
            }));
        }

        // stdout carries the framed JSON-RPC stream (`session/ready`, the
        // periodic `session/heartbeat`, responses). When a link is wired, hand
        // stdin+stdout to the shell so it can build a `SidecarLink` (whose
        // reader re-arms the watchdog clock on every decoded frame). Headless,
        // pump stdout for the watchdog clock instead.
        if let Some(tx) = &self.link_tx {
            if let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) {
                if tx.send((stdin, stdout)).is_err() {
                    eprintln!("[supervisor] link handoff failed — shell receiver gone");
                }
            }
        } else if let Some(out) = child.stdout.take() {
            let last_stdout = Arc::clone(&self.last_activity_ms);
            self.readers.push(std::thread::spawn(move || {
                let _ = pump(out, &last_stdout);
            }));
        }

        self.child = Some(child);
        // State is promoted to `Running` by the watchdog on the first byte
        // (see `check_watchdog`), which makes the connect timeout meaningful.
        self.started_at = Some(Instant::now());

        Ok(())
    }

    /// Restart with exponential backoff: delay = min(2^restart_count, 60) seconds.
    pub fn restart_with_backoff(&mut self) -> Result<(), SupervisorError> {
        self.state = SupervisorState::Restarting;

        // `checked_shl` (saturating_shl is unstable): shift overflow → u64::MAX,
        // which min() with MAX_BACKOFF_SECS clamps to the 60s cap anyway.
        let delay_secs = std::cmp::min(
            1u64.checked_shl(self.restart_count).unwrap_or(u64::MAX),
            MAX_BACKOFF_SECS,
        );
        eprintln!(
            "[supervisor] restart_with_backoff: sleeping {}s (attempt {})",
            delay_secs, self.restart_count
        );
        std::thread::sleep(Duration::from_secs(delay_secs));

        self.restart_count += 1;
        // `spawn()` sets `Starting`; the watchdog promotes to `Running` on the
        // first byte — do NOT set `Running` here or the connect timeout would
        // never apply to restarted children.
        self.spawn()?;
        Ok(())
    }

    /// Check the circuit breaker. Prunes old entries, trips if threshold exceeded.
    pub fn check_circuit_breaker(&mut self) -> Result<(), SupervisorError> {
        let cutoff = Instant::now() - CIRCUIT_WINDOW;
        // Remove entries older than the window.
        while let Some(front) = self.crash_history.front() {
            if *front < cutoff {
                self.crash_history.pop_front();
            } else {
                break;
            }
        }

        if self.crash_history.len() >= CIRCUIT_THRESHOLD {
            self.state = SupervisorState::CircuitOpen;
            return Err(SupervisorError::CircuitOpen);
        }
        Ok(())
    }

    /// Record a crash and check the circuit breaker.
    pub fn record_crash(&mut self) -> Result<(), SupervisorError> {
        self.crash_history.push_back(Instant::now());
        self.check_circuit_breaker()
    }

    /// The watchdog decision — pure, no side effects (unit-testable).
    ///
    /// - `Starting` + no byte within [`CONNECT_TIMEOUT`] → `ConnectTimeout`.
    /// - `Running` + silence longer than [`IDLE_TIMEOUT`] → `IdleTimeout`.
    /// - Unknown activity (`last_activity_ms == 0`) is never treated as idle.
    pub fn watchdog_status(&self) -> WatchdogStatus {
        let last = self.last_activity_ms.load(Ordering::Relaxed);
        match self.state {
            SupervisorState::Starting => {
                if let Some(started) = self.started_at {
                    if last == 0 && started.elapsed() > CONNECT_TIMEOUT {
                        return WatchdogStatus::ConnectTimeout;
                    }
                }
            }
            SupervisorState::Running
                if last != 0 && now_ms().saturating_sub(last) > IDLE_TIMEOUT.as_millis() as u64 =>
            {
                return WatchdogStatus::IdleTimeout;
            }
            _ => {}
        }
        WatchdogStatus::Healthy
    }

    /// Watchdog (J10): on connect/idle timeout, kill + restart.
    ///
    /// Also promotes `Starting → Running` once the first byte arrives. Returns
    /// `Ok(())` after a violation-triggered restart; `Err` only when the
    /// restart itself fails (e.g. the binary went missing).
    pub fn check_watchdog(&mut self) -> Result<(), SupervisorError> {
        match self.watchdog_status() {
            WatchdogStatus::Healthy => {
                // First byte seen while Starting → connected.
                if self.state == SupervisorState::Starting
                    && self.last_activity_ms.load(Ordering::Relaxed) != 0
                {
                    self.state = SupervisorState::Running;
                }
                Ok(())
            }
            status => {
                eprintln!("[supervisor] watchdog: {status:?} — killing child");
                self.kill();
                // A hung child is a crash-like condition — count it toward the
                // circuit breaker so a perpetually-broken binary eventually
                // opens instead of restart-looping forever (backoff caps at 60s).
                self.record_crash()?;
                self.restart_with_backoff()?;
                Ok(())
            }
        }
    }

    /// Kill the child process (if running).
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.state = SupervisorState::Stopped;
    }

    /// Main supervisor loop: spawn, poll, restart on crash.
    ///
    /// The watchdog is checked on every iteration (connect/idle timeouts →
    /// kill + restart). Returns only when the circuit breaker trips or an
    /// unrecoverable error occurs. This method blocks the calling thread
    /// (run it on a dedicated thread).
    pub fn wait_or_restart(&mut self) -> Result<(), SupervisorError> {
        self.spawn()?;
        eprintln!("[supervisor] state: {}", self.state);

        loop {
            // Check circuit breaker first.
            if self.state == SupervisorState::CircuitOpen {
                return Err(SupervisorError::CircuitOpen);
            }

            // Watchdog: kill + restart on connect/idle timeout. Only returns
            // Err if the restart itself failed.
            self.check_watchdog()?;

            // Poll the child.
            let exited = if let Some(ref mut child) = self.child {
                match child.try_wait() {
                    Ok(Some(status)) => Some(status),
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!("[supervisor] try_wait error: {e}");
                        None
                    }
                }
            } else {
                // No child — should not happen in normal flow.
                eprintln!("[supervisor] no child process — attempting respawn");
                self.spawn()?;
                None
            };

            if let Some(status) = exited {
                let code = status.code().unwrap_or(-1);
                self.child = None;

                match code {
                    0 => {
                        // Clean rotation (heap timer or graceful shutdown).
                        eprintln!("[supervisor] child exited cleanly (code 0) — rotating");
                        self.restart_count = 0;
                        self.spawn()?;
                        eprintln!("[supervisor] state: {}", self.state);
                    }
                    75 => {
                        // P45.8 — idle exit. Stay in this loop, do not count a
                        // crash, and spawn again when the shell sets `resume`.
                        eprintln!(
                            "[supervisor] child idle-exited (code 75) — parked until the next turn"
                        );
                        self.state = SupervisorState::Stopped;
                        self.parked.store(true, Ordering::Release);
                        loop {
                            if self.resume.swap(false, Ordering::AcqRel) {
                                self.parked.store(false, Ordering::Release);
                                self.restart_count = 0;
                                self.spawn()?;
                                eprintln!("[supervisor] state: {}", self.state);
                                break;
                            }
                            std::thread::sleep(POLL_INTERVAL);
                        }
                    }
                    71 => {
                        // Heap pressure exit (EX_OSERR).
                        eprintln!("[supervisor] child exited with code 71 (heap pressure)");
                        if self.record_crash().is_err() {
                            return Err(SupervisorError::CircuitOpen);
                        }
                        self.restart_with_backoff()?;
                        eprintln!("[supervisor] state: {}", self.state);
                    }
                    _ => {
                        // Unexpected crash.
                        eprintln!("[supervisor] child crashed with code {code}");
                        if self.record_crash().is_err() {
                            return Err(SupervisorError::CircuitOpen);
                        }
                        self.restart_with_backoff()?;
                        eprintln!("[supervisor] state: {}", self.state);
                    }
                }
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// Read a child pipe until EOF/error, re-arming the watchdog activity clock
/// on every chunk that contains at least one byte.
fn pump<R: Read>(mut reader: R, last_activity_ms: &AtomicU64) -> io::Result<()> {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()), // EOF — child closed the pipe
            Ok(_) => {
                last_activity_ms.store(now_ms(), Ordering::Relaxed);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Like [`pump`], but also forwards each chunk to the app's stderr (prefixed
/// `[sidecar]`) so the coordinator's own diagnostics stay visible in the
/// terminal. Keeps the watchdog clock armed exactly like [`pump`].
fn pump_visible<R: Read>(mut reader: R, last_activity_ms: &AtomicU64) -> io::Result<()> {
    let mut buf = [0u8; 4096];
    let mut pending = Vec::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                if !pending.is_empty() {
                    eprintln!("[sidecar] {}", String::from_utf8_lossy(&pending).trim_end());
                }
                return Ok(()); // EOF — child closed the pipe
            }
            Ok(n) => {
                last_activity_ms.store(now_ms(), Ordering::Relaxed);
                let mut chunk = &buf[..n];
                while let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
                    pending.extend_from_slice(&chunk[..pos]);
                    eprintln!("[sidecar] {}", String::from_utf8_lossy(&pending).trim_end());
                    pending.clear();
                    chunk = &chunk[pos + 1..];
                }
                pending.extend_from_slice(chunk);
            }
            Err(e) => return Err(e),
        }
    }
}

/// UNIX millisecond timestamp.
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_new_defaults() {
        let s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        assert_eq!(s.state, SupervisorState::Stopped);
        assert_eq!(s.restart_count, 0);
        assert!(s.child.is_none());
        assert!(s.crash_history.is_empty());
        assert_eq!(s.last_activity_ms.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn circuit_breaker_trips_after_threshold() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        for _ in 0..5 {
            s.crash_history.push_back(Instant::now());
        }
        let result = s.check_circuit_breaker();
        assert!(result.is_err());
        assert_eq!(s.state, SupervisorState::CircuitOpen);
    }

    #[test]
    fn circuit_breaker_prunes_old_entries() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        // Push entries that are "old" (we can't easily fake Instant, so just test
        // that fewer than 5 recent entries doesn't trip).
        for _ in 0..4 {
            s.crash_history.push_back(Instant::now());
        }
        let result = s.check_circuit_breaker();
        assert!(result.is_ok());
        assert_ne!(s.state, SupervisorState::CircuitOpen);
    }

    #[test]
    fn spawn_fails_with_nonexistent_binary() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/nonexistent/binary/path"));
        let result = s.spawn();
        assert!(result.is_err());
        match result.unwrap_err() {
            SupervisorError::SpawnFailed(_) => {}
            other => panic!("expected SpawnFailed, got: {other:?}"),
        }
    }

    #[test]
    fn kill_when_no_child_is_noop() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        s.kill(); // Should not panic.
        assert_eq!(s.state, SupervisorState::Stopped);
    }

    #[test]
    fn supervisor_state_display() {
        assert_eq!(format!("{}", SupervisorState::Starting), "Starting");
        assert_eq!(format!("{}", SupervisorState::Running), "Running");
        assert_eq!(format!("{}", SupervisorState::CircuitOpen), "CircuitOpen");
    }

    #[test]
    fn record_crash_adds_to_history() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        let _ = s.record_crash();
        assert_eq!(s.crash_history.len(), 1);
    }

    #[test]
    fn watchdog_healthy_with_recent_activity() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        s.state = SupervisorState::Running;
        s.last_activity_ms.store(now_ms(), Ordering::Relaxed);
        assert_eq!(s.watchdog_status(), WatchdogStatus::Healthy);
    }

    #[test]
    fn watchdog_detects_idle_timeout() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        s.state = SupervisorState::Running;
        s.last_activity_ms
            .store(now_ms().saturating_sub(60_000), Ordering::Relaxed);
        assert_eq!(s.watchdog_status(), WatchdogStatus::IdleTimeout);
    }

    #[test]
    fn watchdog_detects_connect_timeout() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        s.state = SupervisorState::Starting;
        s.started_at = Some(Instant::now() - Duration::from_secs(10));
        // No activity yet (last_activity_ms == 0).
        assert_eq!(s.watchdog_status(), WatchdogStatus::ConnectTimeout);
    }

    #[test]
    fn watchdog_ignores_idle_when_no_activity_recorded() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        s.state = SupervisorState::Running;
        // `last == 0` means "no data yet" — must not trip the idle watchdog.
        assert_eq!(s.watchdog_status(), WatchdogStatus::Healthy);
    }

    #[test]
    fn watchdog_promotes_to_running_on_first_byte() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/tmp/fake-bin"));
        s.state = SupervisorState::Starting;
        s.started_at = Some(Instant::now());
        s.last_activity_ms.store(now_ms(), Ordering::Relaxed);
        let result = s.check_watchdog();
        assert!(result.is_ok());
        assert_eq!(s.state, SupervisorState::Running);
    }

    #[test]
    fn watchdog_timeout_triggers_kill_and_restart() {
        let mut s = ProcessSupervisor::new(PathBuf::from("/nonexistent/bin"));
        s.state = SupervisorState::Running;
        s.last_activity_ms
            .store(now_ms().saturating_sub(60_000), Ordering::Relaxed);
        // Restart attempt fails (missing binary) → check_watchdog surfaces it.
        let result = s.check_watchdog();
        assert!(result.is_err());
    }
}
