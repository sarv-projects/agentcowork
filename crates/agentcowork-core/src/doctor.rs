//! P46.2 — `agentcowork doctor`: per-subsystem readiness report (spec H35).
//!
//! A support/diagnostic primitive promoted to V1: a broken component must be
//! *diagnosable* without a support ticket. The report derives from the same
//! readiness signals as E9 H4 (the boot/attach checks each subsystem already
//! exposes) — it does not start heavy subsystems, and it never reads or prints
//! secret material (credentials are reported as present/absent by key name,
//! never by value).
//!
//! Design (mirrors the OpenCode/Claude-Code `doctor` pattern, adapted):
//!   - deterministic, pure-where-possible checks, each returning a [`Check`]
//!     with a tri-state [`Status`] (`Ok` / `Warn` / `Fail`) + a plain-language
//!     detail + an optional remediation hint;
//!   - the full [`DoctorReport`] rolls the checks up into one overall status
//!     and renders as a human table or as machine JSON (`--json`);
//!   - the checks that touch the environment (disk, local runtimes, CDP) are
//!     injected via a [`DoctorProbe`] seam so the whole report is testable
//!     without a live machine (the same seam discipline as the rest of core).
//!
//! The full admin CLI (`providers`/`models`/`agents`/…) stays post-v1; this is
//! only the `doctor` subcommand.

use std::path::{Path, PathBuf};

use agentcowork_types::env_compat;
use serde::{Deserialize, Serialize};

/// Tri-state readiness for one subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ready.
    Ok,
    /// Degraded / optional / needs attention but not fatal.
    Warn,
    /// Broken / a v1-required subsystem is unavailable.
    Fail,
}

impl Status {
    /// The glyph used in the human table (matches the spec's ✓ / ⚠ / ✕).
    pub fn glyph(&self) -> &'static str {
        match self {
            Status::Ok => "✓",
            Status::Warn => "⚠",
            Status::Fail => "✕",
        }
    }

    /// Fold two statuses, keeping the worse. `Fail` > `Warn` > `Ok`.
    fn worse(self, other: Status) -> Status {
        use Status::*;
        match (self, other) {
            (Fail, _) | (_, Fail) => Fail,
            (Warn, _) | (_, Warn) => Warn,
            _ => Ok,
        }
    }
}

/// One subsystem readiness line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Short subsystem name (e.g. `Core`, `Vault`, `Chrome/CDP`).
    pub name: String,
    pub status: Status,
    /// Plain-language one-liner (never contains secret values).
    pub detail: String,
    /// Optional remediation hint when `status != Ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    pub fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Ok,
            detail: detail.into(),
            hint: None,
        }
    }
    pub fn warn(name: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Warn,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }
    pub fn fail(name: &str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Fail,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }
}

/// The full readiness report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub version: String,
    pub checks: Vec<Check>,
    /// Overall = the worst individual status.
    pub overall: Status,
}

impl DoctorReport {
    fn from_checks(version: String, checks: Vec<Check>) -> Self {
        let overall = checks.iter().fold(Status::Ok, |acc, c| acc.worse(c.status));
        Self {
            version,
            checks,
            overall,
        }
    }

    /// Human table (the default `agentcowork doctor` output).
    pub fn render_table(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("AgentCowork doctor — {}\n", self.version));
        out.push_str("─────────────────────────────────────────\n");
        // Align the subsystem column.
        let width = self
            .checks
            .iter()
            .map(|c| c.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        for c in &self.checks {
            out.push_str(&format!(
                "{} {:width$}  {}\n",
                c.status.glyph(),
                c.name,
                c.detail,
                width = width
            ));
            if let Some(hint) = &c.hint {
                out.push_str(&format!("  {:width$}   ↳ {hint}\n", "", width = width));
            }
        }
        out.push_str("─────────────────────────────────────────\n");
        let summary = match self.overall {
            Status::Ok => "All subsystems ready.",
            Status::Warn => "Ready with warnings — see ⚠ lines above.",
            Status::Fail => "One or more required subsystems are broken — see ✕ lines above.",
        };
        out.push_str(&format!("{} {summary}\n", self.overall.glyph()));
        out
    }

    /// The process exit code convention: `Ok`/`Warn` = 0, `Fail` = 1.
    pub fn exit_code(&self) -> i32 {
        match self.overall {
            Status::Ok | Status::Warn => 0,
            Status::Fail => 1,
        }
    }
}

/// The environment seam every check that touches the machine goes through, so
/// the report is fully testable without a live system. Live wiring implements
/// this against the real filesystem / process table / CDP probe; tests inject
/// a deterministic fake.
pub trait DoctorProbe {
    /// Does the vault DB file exist and open (key resolvable)? Returns the
    /// resolved origin string (`env`/`keyfile`/`generated`) or an error.
    fn vault_status(&self) -> Result<String, String>;
    /// Free / total bytes on the data-dir mount.
    fn disk_free(&self) -> Result<(u64, u64), String>;
    /// Is a local model runtime reachable? `(ollama_up, llamafile_available)`.
    fn local_runtimes(&self) -> (bool, bool);
    /// Is a Chrome/Chromium binary discoverable for CDP pairing?
    fn chrome_available(&self) -> bool;
    /// Number of BYOK credentials in the key ring (by key-name count only —
    /// never the values).
    fn credential_count(&self) -> usize;
    /// Number of installed/attachable MCP servers.
    fn mcp_server_count(&self) -> usize;
    /// Does the audit/database directory exist and is it writable?
    fn database_writable(&self) -> Result<PathBuf, String>;
    /// P70.D3 — is the packaged coordinator sidecar binary present (the
    /// bundled resource, `AGENTCOWORK_COORDINATOR_BIN`, or a dev build)?
    /// `None` = the probe cannot know (e.g. the headless CLI has no
    /// resource dir) — the check is then skipped, not failed.
    fn sidecar_binary(&self) -> Option<Result<PathBuf, String>> {
        None
    }
    /// P64.11 — the tool-output spool's live occupancy, so a spool that is
    /// quietly growing is *visible* rather than inferred. `None` = the probe
    /// cannot see a data dir (skipped, not failed).
    fn spool_usage(&self) -> Option<Result<SpoolUsage, String>> {
        None
    }
    /// P70.D3 — the host OS the report is running on ("windows", "linux",
    /// "macos", …). Doctor flags hosts outside the published matrix.
    fn platform(&self) -> String {
        std::env::consts::OS.to_string()
    }
}

/// Disk-free warning floor (matches storage `health` threshold intent).
pub const DISK_WARN_PCT: f64 = 90.0;
/// Disk-free hard-fail floor (almost full — writes will start failing).
pub const DISK_FAIL_PCT: f64 = 98.0;

/// The fraction of the spool's byte ceiling above which doctor warns. Chosen
/// well below 1.0 so the warning lands while there is still room to react —
/// the policy itself is a hard bound, not an alert.
pub const SPOOL_WARN_FRACTION: f64 = 0.80;

/// P64.11 — the observable shape of the tool-output spool, as the doctor
/// reports it. Counts and bytes only: never content, never a preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolUsage {
    pub blob_count: u64,
    pub total_bytes: u64,
    pub max_total_bytes: u64,
    pub retention_days: u32,
    pub used_fraction: f64,
    /// Blobs + bytes the most recent retention pass reclaimed.
    pub pruned_blobs: u64,
    pub pruned_bytes: u64,
}

/// Render one spool line for the doctor table. Over the warn fraction is a
/// `Warn`; the policy is still enforcing the bound, so this is "look at it",
/// not "it is broken".
pub fn spool_check(usage: &SpoolUsage) -> Check {
    let detail = format!(
        "{} blob(s), {} of {} bytes ({:.1}% of the ceiling); kept {} day(s), reclaimed {} blob(s) / {} bytes on the last pass",
        usage.blob_count,
        usage.total_bytes,
        usage.max_total_bytes,
        usage.used_fraction * 100.0,
        usage.retention_days,
        usage.pruned_blobs,
        usage.pruned_bytes,
    );
    if usage.used_fraction >= SPOOL_WARN_FRACTION {
        Check::warn(
            "Tool output spool",
            detail,
            "an over-cap tool result is written to this cache so it does not crowd the model \
             out; the retention policy reclaims the oldest first, or clear the spool directory",
        )
    } else {
        Check::ok("Tool output spool", detail)
    }
}

/// Build the full report from a probe. Pure given the probe — this is the
/// unit-tested core; the CLI/Tauri wiring only supplies a live probe.
pub fn run_doctor(version: &str, probe: &dyn DoctorProbe) -> DoctorReport {
    let mut checks = Vec::new();

    // Core — the binary itself booted far enough to run this, so Core is Ok by
    // construction (the report exists).
    checks.push(Check::ok("Core", format!("orchestrator {version} booted")));

    // Vault.
    match probe.vault_status() {
        Ok(origin) => checks.push(Check::ok("Vault", format!("SQLCipher open (key: {origin})"))),
        Err(e) => checks.push(Check::fail(
            "Vault",
            format!("cannot open: {e}"),
            "run `agentcowork` once to initialize, or set AGENTCOWORK_VAULT_KEY / unlock the passphrase",
        )),
    }

    // Database / audit dir writable.
    match probe.database_writable() {
        Ok(p) => checks.push(Check::ok(
            "Database",
            format!("writable at {}", p.display()),
        )),
        Err(e) => checks.push(Check::fail(
            "Database",
            format!("not writable: {e}"),
            "check permissions on ~/.agentcowork (or AGENTCOWORK_HOME)",
        )),
    }

    // Disk.
    match probe.disk_free() {
        Ok((free, total)) if total > 0 => {
            let used_pct = 100.0 * (total.saturating_sub(free)) as f64 / total as f64;
            if used_pct >= DISK_FAIL_PCT {
                checks.push(Check::fail(
                    "Disk",
                    format!("{used_pct:.0}% full — writes may fail"),
                    "free space on the data-dir volume before running heavy work",
                ));
            } else if used_pct >= DISK_WARN_PCT {
                checks.push(Check::warn(
                    "Disk",
                    format!("{used_pct:.0}% full"),
                    "consider a storage cleanup pass",
                ));
            } else {
                checks.push(Check::ok("Disk", format!("{used_pct:.0}% used")));
            }
        }
        Ok(_) => checks.push(Check::warn("Disk", "total size unknown", "not fatal")),
        Err(e) => checks.push(Check::warn(
            "Disk",
            format!("probe failed: {e}"),
            "not fatal",
        )),
    }

    // Chrome / CDP pairing (optional — browser work degrades to honest-fail).
    if probe.chrome_available() {
        checks.push(Check::ok("Chrome/CDP", "a Chromium binary is discoverable"));
    } else {
        checks.push(Check::warn(
            "Chrome/CDP",
            "no Chromium binary found",
            "install Chrome/Chromium/Edge for browser + computer-use work (optional)",
        ));
    }

    // Local runtimes (optional — BYOK cloud still works without them).
    let (ollama, llamafile) = probe.local_runtimes();
    match (ollama, llamafile) {
        (true, _) => checks.push(Check::ok("Local runtimes", "Ollama reachable")),
        (false, true) => checks.push(Check::ok(
            "Local runtimes",
            "llamafile binary available",
        )),
        (false, false) => checks.push(Check::warn(
            "Local runtimes",
            "no Ollama / llamafile detected",
            "install Ollama or drop a llamafile to run models locally (optional — BYOK cloud works without it)",
        )),
    }

    // Credentials (BYOK) — count only, never values.
    let creds = probe.credential_count();
    if creds > 0 {
        checks.push(Check::ok(
            "Credentials",
            format!("{creds} BYOK key(s) in the vault"),
        ));
    } else {
        checks.push(Check::warn(
            "Credentials",
            "no provider keys configured",
            "add a key in Settings (or point at a local runtime) before running model work",
        ));
    }

    // MCP.
    let mcp = probe.mcp_server_count();
    checks.push(Check::ok(
        "MCP",
        format!("{mcp} server(s) installed; 42-tool inbuilt catalog always available"),
    ));

    // P70.D3 — the coordinator sidecar binary (the live agent-work path).
    // The desktop shell's chat/delegation surfaces are dead without it, so a
    // missing bundle is a FAIL with the remedy named, not a soft warn.
    match probe.sidecar_binary() {
        Some(Ok(p)) => checks.push(Check::ok(
            "Sidecar",
            format!("coordinator binary present ({})", p.display()),
        )),
        Some(Err(e)) => checks.push(Check::fail(
            "Sidecar",
            format!("coordinator binary not found: {e}"),
            if probe.platform() == "windows" {
                "the install is broken or incomplete — reinstall AgentCowork"
            } else {
                "build it with `pnpm --filter @agentcowork/coordinator build` (or set AGENTCOWORK_COORDINATOR_BIN)"
            },
        )),
        // Headless CLI has no resource dir to probe — skip, don't guess.
        None => {}
    }

    // P70.D5 — the host platform, checked against the published matrix. Out
    // of scope is stated where the user is, never silently assumed away.
    let platform = probe.platform();
    match platform.as_str() {
        "windows" => checks.push(Check::ok(
            "Platform",
            "Windows — a shipped v1 platform (SUPPORT-MATRIX.md)".to_string(),
        )),
        "linux" => checks.push(Check::warn(
            "Platform",
            "native Linux desktop is out of v1 scope",
            "WSL2 is supported as an AGENT host; the cockpit ships for Windows (SUPPORT-MATRIX.md §1)",
        )),
        "macos" => checks.push(Check::warn(
            "Platform",
            "macOS is out of v1 scope",
            "no macOS artifact is published; the local harness still builds here (SUPPORT-MATRIX.md §1)",
        )),
        other => checks.push(Check::warn(
            "Platform",
            format!("unknown host OS: {other}"),
            "not in the published support matrix (SUPPORT-MATRIX.md §1)",
        )),
    }

    // P54.8 — detection notes for the Windows shells the profile registry
    // already knows. This is doctor text, not a second terminal product.
    // Status stays Ok: the note is documentation, and a missing shell is not
    // a broken subsystem.
    checks.push(Check::ok(
        "Windows shells",
        "Cmder, Cygwin, and MSYS2 are detected at their default install paths and withheld until the user confirms (those paths are world-writable). Git Bash is detected from git.exe on PATH, Program Files, and scoop.",
    ));
    checks.push(Check {
        name: "Git Bash history".into(),
        status: Status::Ok,
        detail: "Git Bash does not flush history until the shell exits. Set PROMPT_COMMAND='history -a' so the terminal history picker sees commands as they run."
            .into(),
        hint: Some(
            "Detection lives in the terminal profile registry. This line does not install or wrap those shells."
                .into(),
        ),
    });

    // Browser engine crate is compiled in (always true in a real build); this
    // line reports the *engine* readiness separate from a live CDP session.
    checks.push(Check::ok(
        "Browser",
        "engine compiled in (a11y-tree + CDP); live session attaches on first use",
    ));

    // P64.11 — the tool-output spool's real occupancy. A spool with no
    // retention pass, or one quietly eating a disk, is exactly the failure a
    // user cannot see from the outside.
    match probe.spool_usage() {
        Some(Ok(usage)) => checks.push(spool_check(&usage)),
        Some(Err(e)) => checks.push(Check::warn(
            "Tool output spool",
            format!("occupancy could not be read: {e}"),
            "the retention policy could not confirm what the spool is holding; \
             check that the data directory is readable",
        )),
        None => checks.push(Check::ok(
            "Tool output spool",
            "no data directory on this probe — occupancy not sampled",
        )),
    }

    DoctorReport::from_checks(version.to_string(), checks)
}

/// A live [`DoctorProbe`] over the real machine + config. Kept dependency-light
/// (no heavy subsystem construction) so `doctor` stays fast and side-effect
/// free.
pub struct LiveProbe {
    pub data_dir: PathBuf,
}

impl LiveProbe {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

impl DoctorProbe for LiveProbe {
    fn vault_status(&self) -> Result<String, String> {
        let resolved = crate::resolve_vault_key(&self.data_dir).map_err(|e| e.to_string())?;
        let path = self.data_dir.join("vault.db");
        let vault =
            agentcowork_vault::Vault::open(&path, &resolved.key).map_err(|e| e.to_string())?;
        let _ = vault.status();
        Ok(format!("{:?}", resolved.origin).to_lowercase())
    }

    fn disk_free(&self) -> Result<(u64, u64), String> {
        // Reuse the storage crate's drive-stats probe (read-only, no scan).
        match agentcowork_storage::drive_stats(&self.data_dir) {
            Ok(s) => Ok((s.available, s.total)),
            Err(e) => Err(e.to_string()),
        }
    }

    fn local_runtimes(&self) -> (bool, bool) {
        let cfg = crate::Config::load().unwrap_or_default();
        let mgr = crate::LocalManager::from_config(&cfg);
        let ollama = mgr.ollama_running();
        let llamafile =
            mgr.find_llamafile(&self.data_dir).is_some() || env_compat::is_set("LLAMAFILE");
        (ollama, llamafile)
    }

    fn chrome_available(&self) -> bool {
        // Dependency-light probe (doctor must stay fast + not pull the CDP
        // stack): check PATH names + the well-known install locations across
        // platforms. This mirrors agentcowork-cdp's locate order without linking
        // the websocket/tokio CDP crate into the doctor path.
        const NAMES: &[&str] = &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "chrome",
            "msedge",
            "microsoft-edge",
            "brave-browser",
        ];
        if let Ok(path) = std::env::var("PATH") {
            let sep = if cfg!(windows) { ';' } else { ':' };
            for dir in path.split(sep) {
                for name in NAMES {
                    let mut p = std::path::PathBuf::from(dir);
                    p.push(name);
                    if p.is_file() {
                        return true;
                    }
                    if cfg!(windows) {
                        p.set_extension("exe");
                        if p.is_file() {
                            return true;
                        }
                    }
                }
            }
        }
        const WELL_KNOWN: &[&str] = &[
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/brave-browser",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
            "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
        ];
        WELL_KNOWN.iter().any(|p| Path::new(p).is_file())
    }

    fn credential_count(&self) -> usize {
        // Open the vault read-only and count key-ring entries across the known
        // provider registry (by key id only — never the secret values).
        let Ok(resolved) = crate::resolve_vault_key(&self.data_dir) else {
            return 0;
        };
        let path = self.data_dir.join("vault.db");
        let Ok(vault) = agentcowork_vault::Vault::open(&path, &resolved.key) else {
            return 0;
        };
        agentcowork_catalog::base_registry()
            .all()
            .filter_map(|p| vault.list_keys(&p.id).ok())
            .map(|ids| ids.len())
            .sum()
    }

    fn mcp_server_count(&self) -> usize {
        // Installed servers live under <data_dir>/mcp; count directory entries.
        let dir = self.data_dir.join("mcp");
        std::fs::read_dir(&dir)
            .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    }

    fn database_writable(&self) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        // Probe writability with a temp file that we immediately remove.
        let probe = self.data_dir.join(".doctor-write-probe");
        std::fs::write(&probe, b"ok").map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&probe);
        Ok(self.data_dir.clone())
    }

    /// P70.D3 — the sidecar as far as the headless probe can see: the
    /// `AGENTCOWORK_COORDINATOR_BIN` override, then the workspace dev build
    /// paths relative to the current directory. The *packaged resource dir*
    /// needs the running app handle, so the desktop shell layers its own
    /// `DoctorProbe` on top of this one for the full picture; from the CLI
    /// this is the honest subset.
    fn sidecar_binary(&self) -> Option<Result<PathBuf, String>> {
        if let Some(p) = env_compat::get("COORDINATOR_BIN") {
            let p = PathBuf::from(p);
            return Some(if p.is_file() {
                Ok(p)
            } else {
                Err(format!(
                    "AGENTCOWORK_COORDINATOR_BIN points at a missing file: {}",
                    p.display()
                ))
            });
        }
        let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
        let cwd = std::env::current_dir().ok()?;
        for rel in [
            "packages/coordinator/dist/coordinator",
            "../packages/coordinator/dist/coordinator",
        ] {
            let mut p = cwd.join(rel);
            p.set_extension(exe_suffix.trim_start_matches('.'));
            if p.is_file() {
                return Some(Ok(p));
            }
            // Also try the unsuffixed name (the compile output on unix).
            let bare = cwd.join(rel);
            if bare.is_file() {
                return Some(Ok(bare));
            }
        }
        Some(Err(
            "not in AGENTCOWORK_COORDINATOR_BIN or the workspace build output".to_string(),
        ))
    }

    /// P64.11 — read the spool's real occupancy. The directory is only walked,
    /// never written: doctor stays a read-only, side-effect-free report.
    fn spool_usage(&self) -> Option<Result<SpoolUsage, String>> {
        let stats = crate::spool::Spool::new(&self.data_dir).stats();
        Some(Ok(SpoolUsage {
            blob_count: stats.blob_count,
            total_bytes: stats.total_bytes,
            max_total_bytes: stats.max_total_bytes,
            retention_days: stats.retention_days,
            used_fraction: stats.used_fraction,
            pruned_blobs: stats.pruned_blobs,
            pruned_bytes: stats.pruned_bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProbe {
        vault: Result<String, String>,
        disk: Result<(u64, u64), String>,
        ollama: bool,
        llamafile: bool,
        chrome: bool,
        creds: usize,
        mcp: usize,
        db: Result<PathBuf, String>,
        /// `None` = the probe cannot know (check skipped, like the CLI).
        sidecar: Option<Result<PathBuf, String>>,
        platform: String,
    }

    impl Default for FakeProbe {
        fn default() -> Self {
            Self {
                vault: Ok("env".into()),
                disk: Ok((50, 100)),
                ollama: true,
                llamafile: false,
                chrome: true,
                creds: 2,
                mcp: 1,
                db: Ok(PathBuf::from("/tmp/x")),
                sidecar: Some(Ok(PathBuf::from("/opt/agentcowork/bin/coordinator"))),
                platform: "windows".into(),
            }
        }
    }

    impl DoctorProbe for FakeProbe {
        fn vault_status(&self) -> Result<String, String> {
            self.vault.clone()
        }
        fn disk_free(&self) -> Result<(u64, u64), String> {
            self.disk.clone()
        }
        fn local_runtimes(&self) -> (bool, bool) {
            (self.ollama, self.llamafile)
        }
        fn chrome_available(&self) -> bool {
            self.chrome
        }
        fn credential_count(&self) -> usize {
            self.creds
        }
        fn mcp_server_count(&self) -> usize {
            self.mcp
        }
        fn database_writable(&self) -> Result<PathBuf, String> {
            self.db.clone()
        }
        fn sidecar_binary(&self) -> Option<Result<PathBuf, String>> {
            self.sidecar.clone()
        }
        fn platform(&self) -> String {
            self.platform.clone()
        }
    }

    #[test]
    fn all_ok_when_everything_healthy() {
        let r = run_doctor("v-test", &FakeProbe::default());
        assert_eq!(r.overall, Status::Ok);
        assert_eq!(r.exit_code(), 0);
        // Core + Vault + Database + Disk + Chrome + Local + Credentials + MCP
        // + Sidecar + Platform + Windows shells + Git Bash history + Browser
        assert_eq!(r.checks.len(), 13);
        let shells = r
            .checks
            .iter()
            .find(|c| c.name == "Windows shells")
            .unwrap();
        assert!(shells.detail.contains("Cmder"));
        assert!(shells.detail.contains("Cygwin"));
        assert!(shells.detail.contains("MSYS2"));
        assert!(shells.detail.contains("Git Bash"));
        let history = r
            .checks
            .iter()
            .find(|c| c.name == "Git Bash history")
            .unwrap();
        assert!(history.detail.contains("PROMPT_COMMAND='history -a'"));
        assert!(r.checks.iter().all(|c| c.status == Status::Ok));
    }

    /// P70.D3 — a missing sidecar binary is a FAIL naming the remedy, and on
    /// Windows the remedy is "reinstall" (a broken install), not a build tip.
    #[test]
    fn missing_sidecar_fails_with_the_reinstall_remedy_on_windows() {
        let probe = FakeProbe {
            sidecar: Some(Err("bundle resource absent".into())),
            ..Default::default()
        };
        let r = run_doctor("v", &probe);
        let sidecar = r.checks.iter().find(|c| c.name == "Sidecar").unwrap();
        assert_eq!(sidecar.status, Status::Fail);
        assert!(sidecar.hint.as_deref().unwrap().contains("reinstall"));
        assert_eq!(r.overall, Status::Fail);
    }

    /// P70.D5 — an out-of-scope host is a stated WARN with the matrix
    /// pointer, never a silent pass and never a fabricated FAIL.
    #[test]
    fn out_of_scope_platform_warns_with_the_matrix_pointer() {
        for os in ["linux", "macos", "freebsd"] {
            let probe = FakeProbe {
                platform: os.into(),
                ..Default::default()
            };
            let r = run_doctor("v", &probe);
            let platform = r.checks.iter().find(|c| c.name == "Platform").unwrap();
            assert_eq!(platform.status, Status::Warn, "os={os}");
            assert!(
                platform
                    .hint
                    .as_deref()
                    .unwrap()
                    .contains("SUPPORT-MATRIX.md")
            );
            assert_eq!(r.overall, Status::Warn);
        }
    }

    #[test]
    fn vault_failure_makes_overall_fail() {
        let probe = FakeProbe {
            vault: Err("locked".into()),
            ..Default::default()
        };
        let r = run_doctor("v", &probe);
        assert_eq!(r.overall, Status::Fail);
        assert_eq!(r.exit_code(), 1);
        let vault = r.checks.iter().find(|c| c.name == "Vault").unwrap();
        assert_eq!(vault.status, Status::Fail);
        assert!(vault.hint.is_some());
    }

    #[test]
    fn no_credentials_and_no_local_is_warn_not_fail() {
        let probe = FakeProbe {
            creds: 0,
            ollama: false,
            llamafile: false,
            ..Default::default()
        };
        let r = run_doctor("v", &probe);
        // Warnings, but nothing v1-required is broken.
        assert_eq!(r.overall, Status::Warn);
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn disk_thresholds_map_to_status() {
        // 95% used → warn.
        let warn = run_doctor(
            "v",
            &FakeProbe {
                disk: Ok((5, 100)),
                ..Default::default()
            },
        );
        assert_eq!(
            warn.checks
                .iter()
                .find(|c| c.name == "Disk")
                .unwrap()
                .status,
            Status::Warn
        );
        // 99% used → fail.
        let fail = run_doctor(
            "v",
            &FakeProbe {
                disk: Ok((1, 100)),
                ..Default::default()
            },
        );
        assert_eq!(fail.overall, Status::Fail);
    }

    #[test]
    fn render_table_and_json_never_leak_secret_values() {
        // credential_count is a count; the report must never contain a value.
        let r = run_doctor(
            "v",
            &FakeProbe {
                creds: 3,
                ..Default::default()
            },
        );
        let table = r.render_table();
        let json = serde_json::to_string(&r).unwrap();
        assert!(table.contains("3 BYOK key"));
        assert!(json.contains("\"credentials\"") || json.contains("Credentials"));
        // A count, not a value — no "sk-" style token can appear.
        assert!(!table.contains("sk-"));
        assert!(!json.contains("sk-"));
    }

    #[test]
    fn overall_is_worst_of_all_checks() {
        let probe = FakeProbe {
            chrome: false,          // warn
            vault: Err("x".into()), // fail
            ..Default::default()
        };
        let r = run_doctor("v", &probe);
        assert_eq!(r.overall, Status::Fail); // fail dominates warn
    }
}
