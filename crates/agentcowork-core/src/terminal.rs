//! H36 (P54) — integrated terminal profiles + PTY backends (VS Code model).
//!
//! Three pieces, one owner (`agentcowork-core` — no second registry):
//!
//! 1. **Profile registry** (P54.1) — `TerminalConfig` in `agentcowork.toml`:
//!    `terminal.profiles.<platform>` (name → profile, `null` deletes a
//!    detected one), `terminal.defaultProfile.<platform>`,
//!    `terminal.automationProfile.<platform>`, `terminal.useWslProfiles`,
//!    `terminal.unsafeConfirmed.<platform>`.
//!
//! 2. **Detection** (P54.1) — VS Code's exact algorithm, ported from
//!    `src/vs/platform/terminal/node/terminalProfiles.ts` +
//!    `src/vs/base/node/powershell.ts` (MIT): Sysnative/System32 switch,
//!    pwsh highest-version-dir + MSIX + scoop enumeration, Git Bash via
//!    `git.exe` on PATH + ProgramFiles fallbacks + scoop, unsafe Cygwin/
//!    MSYS2/Cmder `requiresPath`, WSL `wsl.exe -l -q` (utf16le, build
//!    ≥ 19041, `docker-desktop*` skipped), unix `$SHELL` then `/etc/shells`
//!    with basename + `(n)` dedupe, PATH fallback with `isFromPath`,
//!    config override (null deletes / object requires path or source).
//!
//! 3. **Backend + PTY host** (P54.2/.3) — `TerminalBackend` { Local, Wsl,
//!    Remote } with mandatory `abi_version` (host vN serves 1..=N), and a
//!    real PTY over `portable-pty` (unix pty / Windows ConPTY — **not**
//!    piped stdio; TUI apps must work). Resize / kill / reap. Human typing
//!    is `AuthKind::human_gesture` at the shell layer; agent `script.run` /
//!    ACP `terminal/create` stay ticketed above this host.
//!
//! Detection is pure where possible (fs + env injected), so the tests run
//! the real algorithms without spawning shells.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

// P68.8/P68.9 — re-exported: `PtyHost` hands these back (`tracker()`) and the
// `script.run` executor reads a session's records, so a consumer of the PTY
// plane gets the record types from the plane itself rather than having to reach
// into the shell-integration module.
pub use crate::shell_integration::{CommandRecord, CommandTracker};
use crate::shell_integration::{
    INJECTION_ENV, IntegrationEvent, IntegrationParser, IntegrationShell, NONCE_ENV, Segment,
};

// ---------------------------------------------------------------------------
// P54.1 — TerminalConfig (agentcowork.toml `terminal.*`)
// ---------------------------------------------------------------------------

/// Profile source contract (VS Code `source:`): "detect the install", never a
/// baked path. Windows-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProfileSource {
    PowerShell,
    GitBash,
}

/// One path entry of a `path:` fallback array; `unsafe` paths are detected but
/// not offered until the user confirms (VS Code `ITerminalUnsafePath`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsafePath {
    pub path: String,
    #[serde(rename = "isUnsafe", default)]
    pub is_unsafe: bool,
}

/// `TerminalProfile.path` accepts VS Code's shapes: a bare string, an unsafe
/// object, or an array of either. An empty string contributes nothing, so
/// `path = ""` reads as the "delete this detected profile" marker (toml has
/// no `null`).
mod path_serde {
    use super::UnsafePath;
    use serde::de::Deserializer;
    use serde::ser::Serializer;
    use serde::{Deserialize, Serialize};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Bare(String),
        Unsafe {
            path: String,
            #[serde(rename = "isUnsafe", default)]
            is_unsafe: bool,
        },
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape {
        Many(Vec<Entry>),
        One(Entry),
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<UnsafePath>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = match Shape::deserialize(deserializer)? {
            Shape::One(e) => vec![e],
            Shape::Many(v) => v,
        };
        Ok(entries
            .into_iter()
            .filter_map(|e| match e {
                Entry::Bare(path) => (!path.is_empty()).then_some(UnsafePath {
                    path,
                    is_unsafe: false,
                }),
                Entry::Unsafe { path, is_unsafe } => {
                    (!path.is_empty()).then_some(UnsafePath { path, is_unsafe })
                }
            })
            .collect())
    }

    #[derive(Serialize)]
    #[serde(untagged)]
    enum OutEntry<'a> {
        Bare(&'a str),
        Unsafe {
            path: &'a str,
            #[serde(rename = "isUnsafe")]
            is_unsafe: bool,
        },
    }

    pub fn serialize<S>(paths: &[UnsafePath], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let out: Vec<OutEntry<'_>> = paths
            .iter()
            .map(|p| {
                if p.is_unsafe {
                    OutEntry::Unsafe {
                        path: &p.path,
                        is_unsafe: true,
                    }
                } else {
                    OutEntry::Bare(&p.path)
                }
            })
            .collect();
        out.serialize(serializer)
    }
}

/// A user/detected terminal profile (VS Code `ITerminalExecutable` shape).
/// Stored in `agentcowork.toml` under `terminal.profiles.<platform>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProfile {
    /// Executable path (string / fallback array) — or empty when `source` is set.
    #[serde(default, with = "path_serde", skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<UnsafePath>,
    /// Windows-only `source` contract (`PowerShell` | `GitBash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProfileSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    /// Working directory; empty = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Which backend runs this profile. Default `Local`.
    #[serde(default)]
    pub backend: TerminalBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Detected-but-unconfirmed (world-writable install dir).
    #[serde(
        rename = "isUnsafe",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub is_unsafe: bool,
    /// Path resolved from `$PATH` (VS Code `isFromPath`).
    #[serde(
        rename = "isFromPath",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub is_from_path: bool,
}

impl Default for TerminalProfile {
    fn default() -> Self {
        Self {
            path: Vec::new(),
            source: None,
            args: Vec::new(),
            env: None,
            cwd: None,
            icon: None,
            color: None,
            backend: TerminalBackend::Local,
            hidden: None,
            is_unsafe: false,
            is_from_path: false,
        }
    }
}

/// H36 backends (spec §4.5). `abi_version` is mandatory; host vN serves
/// 1..=N. UI talks only `pty_id` + `profile_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TerminalBackend {
    #[default]
    Local,
    Wsl,
    /// H33 v1 attach: PTY on a user-owned ExecutionNode over P8.9.
    /// Not yet wired — the enum slot exists so profiles can be authored;
    /// spawning fails closed with `remote_unavailable`.
    Remote,
}

impl TerminalBackend {
    /// H36 `abi_version` — host vN serves 1..=N (clients advertise N; older
    /// clients on a newer host are fine, newer clients refuse).
    pub const HOST_ABI_VERSION: u32 = 1;

    pub fn abi_version(&self) -> u32 {
        Self::HOST_ABI_VERSION
    }
}

/// `terminal.*` section of `agentcowork.toml` (P54.1) — camelCase keys match
/// the spec exactly: `terminal.profiles.<platform>`,
/// `terminal.defaultProfile.<platform>`, `terminal.automationProfile.<platform>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TerminalConfig {
    /// name → profile per platform. Delete marker = empty `path` + no
    /// `source` (toml has no null; VS Code's `null` delete → empty profile).
    pub profiles: PlatformMap<ProfileMap>,
    #[serde(rename = "defaultProfile")]
    pub default_profile: PlatformMap<String>,
    /// P54.5 — POSIX-friendly shell for tasks / agent `script.run`. Distinct
    /// from the interactive default (never the user's heavy PowerShell
    /// profile on every agent command).
    #[serde(rename = "automationProfile")]
    pub automation_profile: PlatformMap<String>,
    /// WSL distro profiles on/off (default on, VS Code `useWslProfiles`).
    #[serde(rename = "useWslProfiles", default = "default_true")]
    pub use_wsl_profiles: bool,
    /// P54.6 — unsafe install dirs the user has confirmed this platform.
    #[serde(rename = "unsafeConfirmed")]
    pub unsafe_confirmed: PlatformMap<Vec<String>>,
    /// Unconfirmed unsafe profiles are still returned by detection (flagged),
    /// but never offered in the `+` dropdown until confirmed.
    #[serde(rename = "hiddenUnsafe", default)]
    pub hidden_unsafe: bool,
    /// P67 — inject the OSC 633 shell-integration script at spawn. On by
    /// default; turning it off gives a plain terminal with no structured
    /// facts (no cwd, no exit codes, no command history for the agent).
    #[serde(rename = "shellIntegration", default = "default_true")]
    pub shell_integration: bool,
    /// P68.8 — retained output bytes per PTY session (the replay ring).
    /// `None` = [`DEFAULT_SCROLLBACK_BYTES`]. Normalized by
    /// [`TerminalConfig::scrollback_bytes`], which clamps it so a config value
    /// can neither starve replay nor grow unbounded.
    #[serde(
        rename = "scrollbackBytes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub scrollback_bytes: Option<usize>,
}

// ---------------------------------------------------------------------------
// P68.8 — output ring buffer + replay after reattach
// ---------------------------------------------------------------------------

/// Retained output per session when `terminal.scrollbackBytes` is unset.
/// 256 KiB is ~4× a full 80×24 screen of dense TUI output and ~2.5k lines of
/// plain text — enough that a view closed for a moment replays seamlessly,
/// small enough that a dozen live sessions stay trivial in memory.
pub const DEFAULT_SCROLLBACK_BYTES: usize = 256 * 1024;
/// Floor: below this the ring is a decoration (a single `ls -l` would evict it).
/// `0` is deliberately *not* reachable through config — it is only reachable by
/// constructing [`TerminalRing`] directly, where it means "retention disabled".
pub const MIN_SCROLLBACK_BYTES: usize = 8 * 1024;
/// Ceiling: 16 MiB per session bounds the worst case (a `yes` loop) so a
/// runaway program cannot make the host the memory fault line.
pub const MAX_SCROLLBACK_BYTES: usize = 16 * 1024 * 1024;

impl TerminalConfig {
    /// P68.8 — the effective replay-ring capacity, clamped to
    /// [`MIN_SCROLLBACK_BYTES`]..=[`MAX_SCROLLBACK_BYTES`].
    pub fn scrollback_bytes(&self) -> usize {
        self.scrollback_bytes
            .unwrap_or(DEFAULT_SCROLLBACK_BYTES)
            .clamp(MIN_SCROLLBACK_BYTES, MAX_SCROLLBACK_BYTES)
    }
}

/// A bounded, byte-oriented ring over a session's rendered output.
///
/// The cursor is a **byte sequence** (`seq`), not an index, so a consumer that
/// already rendered some of the stream can ask for only what it has not seen.
/// When a cursor falls outside the retained window the difference is reported
/// as a gap ([`TerminalReplay::dropped`]) rather than silently stitched — the
/// same honesty rule the shell-reporting path follows for untrusted records.
#[derive(Debug, Clone)]
pub struct TerminalRing {
    buf: VecDeque<u8>,
    /// Maximum retained bytes; `0` disables retention (cursor still advances).
    cap: usize,
    /// Sequence of the oldest retained byte — i.e. bytes discarded so far.
    start: u64,
    /// Total bytes ever pushed (`seq()`).
    total: u64,
}

impl TerminalRing {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            cap,
            start: 0,
            total: 0,
        }
    }

    /// Append rendered output; returns the new cursor. Oldest bytes are evicted
    /// until the ring is within capacity.
    pub fn push(&mut self, bytes: &[u8]) -> u64 {
        let n = bytes.len() as u64;
        if self.cap == 0 || bytes.is_empty() {
            self.total += n;
            self.start = self.total;
            return self.total;
        }
        self.buf.extend(bytes.iter().copied());
        self.total += n;
        if self.buf.len() > self.cap {
            let excess = self.buf.len() - self.cap;
            self.buf.drain(..excess);
        }
        self.start = self.total - self.buf.len() as u64;
        self.total
    }

    /// The newest cursor (pass back as `fromSeq` on the next replay).
    pub fn seq(&self) -> u64 {
        self.total
    }

    /// Bytes dropped so far (== the sequence of the oldest retained byte).
    pub fn discarded(&self) -> u64 {
        self.start
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Everything retained, plus the current cursor (gap is always zero here).
    pub fn snapshot(&self) -> (u64, Vec<u8>) {
        (self.total, self.buf.iter().copied().collect())
    }

    /// Bytes written strictly after `from`, plus the new cursor and how many
    /// bytes lie before the retained window (`dropped > 0` ⇒ the caller's
    /// cursor was already evicted and the replay is a truncated view).
    pub fn since(&self, from: u64) -> (u64, Vec<u8>, u64) {
        if from >= self.total {
            return (self.total, Vec::new(), 0);
        }
        let dropped = self.start.saturating_sub(from);
        let offset = from.saturating_sub(self.start) as usize;
        let bytes: Vec<u8> = self.buf.iter().skip(offset).copied().collect();
        (self.total, bytes, dropped)
    }
}

/// One replay response: the bytes to render, the cursor to resume from, and an
/// explicit statement of any gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReplay {
    /// Newest retained cursor — pass back as `fromSeq` to fetch only updates.
    pub seq: u64,
    /// Output to render (empty is honest: "nothing retained since your cursor").
    pub bytes: Vec<u8>,
    /// Bytes the caller missed because its cursor was evicted. Nonzero means
    /// the replay is a truncated view and must be labelled as such.
    pub dropped: u64,
    /// The ring's capacity, so the UI can state its retention window honestly.
    pub capacity: usize,
}

pub type ProfileMap = HashMap<String, TerminalProfile>;

/// The platform a config/detection run targets. Injected so the Windows
/// detection port (VS Code `terminalProfiles.ts`) is testable off-Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Platform {
    Windows,
    #[default]
    Linux,
    Macos,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

/// `{ windows, linux, macos }` map with permissive platform keys in toml
/// (`windows` / `linux` / `macos`; missing = empty/default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlatformMap<T> {
    #[serde(
        rename = "windows",
        alias = "win32",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub windows: Option<T>,
    #[serde(rename = "linux", default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<T>,
    #[serde(
        rename = "macos",
        alias = "osx",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub macos: Option<T>,
}

fn default_true() -> bool {
    true
}

impl<T: Default> PlatformMap<T> {
    pub fn for_platform(&self, platform: Platform) -> Option<&T> {
        match platform {
            Platform::Windows => self.windows.as_ref(),
            Platform::Macos => self.macos.as_ref(),
            Platform::Linux => self.linux.as_ref(),
        }
    }

    pub fn for_current_platform(&self) -> Option<&T> {
        self.for_platform(Platform::current())
    }

    pub fn platform_mut_for(&mut self, platform: Platform) -> &mut T {
        let slot = match platform {
            Platform::Windows => &mut self.windows,
            Platform::Macos => &mut self.macos,
            Platform::Linux => &mut self.linux,
        };
        slot.get_or_insert_with(Default::default)
    }

    pub fn platform_mut(&mut self) -> &mut T {
        self.platform_mut_for(Platform::current())
    }
}

impl TerminalConfig {
    pub fn profiles_for(&self, platform: Platform) -> Option<&ProfileMap> {
        self.profiles.for_platform(platform)
    }

    pub fn platform_profiles(&self) -> Option<&ProfileMap> {
        self.profiles.for_current_platform()
    }

    pub fn default_profile_name_for(&self, platform: Platform) -> Option<&str> {
        self.default_profile
            .for_platform(platform)
            .map(|s| s.as_str())
    }

    pub fn default_profile_name(&self) -> Option<&str> {
        self.default_profile_name_for(Platform::current())
    }

    /// P54.5 — automation profile name; falls back to the interactive default.
    pub fn automation_profile_name_for(&self, platform: Platform) -> Option<&str> {
        self.automation_profile
            .for_platform(platform)
            .map(|s| s.as_str())
            .or_else(|| self.default_profile_name_for(platform))
    }

    pub fn automation_profile_name(&self) -> Option<&str> {
        self.automation_profile_name_for(Platform::current())
    }

    /// P54.6 — confirmed-unsafe profile names for this platform.
    pub fn unsafe_confirmed_names_for(&self, platform: Platform) -> &[String] {
        self.unsafe_confirmed
            .for_platform(platform)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn unsafe_confirmed_names(&self) -> &[String] {
        self.unsafe_confirmed_names_for(Platform::current())
    }
}

// ---------------------------------------------------------------------------
// Detection (VS Code terminalProfiles.ts port)
// ---------------------------------------------------------------------------

/// Injected environment for pure testing (VS Code `shellEnv` + `fsProvider`).
#[derive(Debug, Clone, Default)]
pub struct DetectEnv {
    /// Windows env: `windir`, `ProgramFiles`, `ProgramFiles(X86)`,
    /// `ProgramW6432`, `LocalAppData`, `HOMEDRIVE`, `UserProfile`,
    /// `PROCESSOR_ARCHITEW6432`, `PROCESSOR_ARCHITECTURE`, `CMDER_ROOT`.
    pub env: HashMap<String, String>,
    /// Unix env: `SHELL`, `PATH`.
    pub env_unix: HashMap<String, String>,
    /// Contents of `/etc/shells` (unix; `None` = file missing).
    pub etc_shells: Option<String>,
    /// Directories that exist (validated paths check membership).
    pub exists_dirs: Vec<String>,
    /// Files that exist.
    pub exists_files: Vec<String>,
    /// `PATH` search results: command name → resolved absolute path.
    pub on_path: HashMap<String, String>,
    /// `PowerShell/<ver>` dirs under Program Files (Windows pwsh detection).
    pub pwsh_install_dirs: Vec<String>,
    /// MSIX subdirs under LocalAppData/WindowsApps (pwsh Store detection).
    pub msix_app_dirs: Vec<String>,
    /// WSL distro list (`wsl.exe -l -q` output as decoded lines).
    pub wsl_distros: Vec<String>,
    /// Windows build number (`None` on unix; < 19041 disables WSL discovery).
    pub windows_build: Option<u32>,
    /// When true (real host only), existence checks fall back to the actual
    /// filesystem. Injected test envs keep this false so detection stays
    /// hermetic and a coincidental host file can never pass a test.
    pub probe_fs: bool,
    /// Platform whose detection algorithm runs (`Windows` | `Linux` | `Macos`).
    pub platform: Platform,
}

impl DetectEnv {
    fn env_any(&self, key: &str) -> Option<&String> {
        self.env
            .get(key)
            .or_else(|| self.env_unix.get(key))
            .filter(|v| !v.is_empty())
    }

    fn file_exists(&self, p: &str) -> bool {
        if self.exists_files.iter().any(|f| f == p) {
            return true;
        }
        self.probe_fs && Path::new(p).is_file()
    }

    fn dir_exists(&self, p: &str) -> bool {
        if self.exists_dirs.iter().any(|d| d == p) {
            return true;
        }
        self.probe_fs && Path::new(p).is_dir()
    }

    /// VS Code `findExecutable`: absolute path → existence check; bare name
    /// → scan `PATH` entries.
    fn find_executable(&self, command: &str) -> Option<String> {
        if Path::new(command).is_absolute() {
            return if self.file_exists(command) {
                Some(command.to_string())
            } else {
                None
            };
        }
        if let Some(hit) = self.on_path.get(command) {
            return Some(hit.clone());
        }
        // Windows file names are case-insensitive; the probe stores the
        // lowercase alias so `git.exe` resolves from a `Git.EXE` entry.
        if self.probe_fs {
            if let Some(hit) = self.on_path.get(&command.to_ascii_lowercase()) {
                return Some(hit.clone());
            }
        }
        None
    }
}

fn real_env() -> DetectEnv {
    let mut env = HashMap::new();
    for k in [
        "windir",
        "ProgramFiles",
        "ProgramFiles(X86)",
        "ProgramW6432",
        "LocalAppData",
        "HOMEDRIVE",
        "UserProfile",
        "PROCESSOR_ARCHITEW6432",
        "PROCESSOR_ARCHITECTURE",
        "CMDER_ROOT",
    ] {
        if let Ok(v) = std::env::var(k) {
            env.insert(k.to_string(), v);
        }
    }
    let mut env_unix = HashMap::new();
    for k in ["SHELL", "PATH"] {
        if let Ok(v) = std::env::var(k) {
            env_unix.insert(k.to_string(), v);
        }
    }

    // --- filesystem probes (VS Code does these through its fsProvider) ---
    let etc_shells = std::fs::read_to_string("/etc/shells").ok();

    // PATH resolution: command name → first absolute hit, in PATH order.
    let mut on_path = HashMap::new();
    if let Some(path) = env_unix.get("PATH") {
        let sep = if cfg!(target_os = "windows") {
            ';'
        } else {
            ':'
        };
        for dir in path.split(sep).filter(|d| !d.is_empty()) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let abs = entry.path().to_string_lossy().to_string();
                if cfg!(target_os = "windows") {
                    on_path
                        .entry(name.to_ascii_lowercase())
                        .or_insert_with(|| abs.clone());
                }
                on_path.entry(name).or_insert(abs);
            }
        }
    }

    // PowerShell install dirs + MSIX app dirs (Windows only).
    let mut pwsh_install_dirs = Vec::new();
    let mut msix_app_dirs = Vec::new();
    if cfg!(target_os = "windows") {
        for pf in ["ProgramFiles", "ProgramW6432", "ProgramFiles(X86)"] {
            if let Some(pf_dir) = env.get(pf) {
                if let Ok(entries) = std::fs::read_dir(format!(r"{pf_dir}\PowerShell")) {
                    for entry in entries.flatten() {
                        if let Ok(name) = entry.file_name().into_string() {
                            pwsh_install_dirs.push(name);
                        }
                    }
                }
            }
        }
        if let Some(lad) = env.get("LocalAppData") {
            if let Ok(entries) = std::fs::read_dir(format!(r"{lad}\Microsoft\WindowsApps")) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.starts_with("Microsoft.PowerShell_") {
                            msix_app_dirs.push(name);
                        }
                    }
                }
            }
        }
    }

    // WSL distros (`wsl.exe -l -q`), Windows only.
    let wsl_distros = if cfg!(target_os = "windows") {
        let windir = env
            .get("windir")
            .cloned()
            .unwrap_or_else(|| r"C:\Windows".into());
        let is32_on_64 = env.contains_key("PROCESSOR_ARCHITEW6432");
        let sysdir = if is32_on_64 {
            format!(r"{windir}\Sysnative")
        } else {
            format!(r"{windir}\System32")
        };
        real_wsl_distros(Path::new(&sysdir))
    } else {
        Vec::new()
    };

    DetectEnv {
        env,
        env_unix,
        etc_shells,
        on_path,
        pwsh_install_dirs,
        msix_app_dirs,
        wsl_distros,
        probe_fs: true,
        ..DetectEnv::default()
    }
}

/// Read real Windows build number from `HKLM ...\CurrentBuildNumber` via
/// reg.exe (VS Code `getWindowsBuildNumberAsync`); None off-Windows/failure.
#[cfg(target_os = "windows")]
fn real_windows_build() -> Option<u32> {
    let out = std::process::Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\Software\Microsoft\Windows NT\CurrentVersion",
            "/v",
            "CurrentBuildNumber",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .last()
        .and_then(|t| t.parse::<u32>().ok())
}

#[cfg(not(target_os = "windows"))]
fn real_windows_build() -> Option<u32> {
    None
}

/// Real WSL distro list: `wsl.exe -l -q`, output is utf16le by default
/// (VS Code forces the encoding; we decode both).
fn real_wsl_distros(system32: &Path) -> Vec<String> {
    let wsl = system32.join("wsl.exe");
    let out = match std::process::Command::new(&wsl)
        .args(["-l", "-q"])
        .env("WSL_UTF8", "0")
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    let text = if out.len() >= 2 && out[1] == 0 {
        // utf16le
        let units: Vec<u16> = out
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&out).to_string()
    };
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// A fully-validated detected/configured profile (VS Code `ITerminalProfile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedProfile {
    #[serde(rename = "profileName")]
    pub profile_name: String,
    pub path: String,
    /// The resolved path lives in a world-writable install dir; P54.6 blocks
    /// offering it until the user confirms the profile by name.
    #[serde(rename = "isUnsafePath", default)]
    pub is_unsafe_path: bool,
    #[serde(rename = "isFromPath", default)]
    pub is_from_path: bool,
    #[serde(rename = "isAutoDetected", default)]
    pub is_auto_detected: bool,
    #[serde(rename = "isDefault", default)]
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub backend: TerminalBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProfileSource>,
    /// WSL distro name (Wsl backend profiles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl_distro: Option<String>,
    /// Optional working directory override for spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl DetectedProfile {
    /// P54.6 — unsafe install dirs are hidden until the user confirms them
    /// (`terminal.unsafeConfirmed`) unless the user opted to show all.
    pub fn offered(&self, confirmed: &[String], hide_unsafe: bool) -> bool {
        if !self.is_unsafe_path {
            return true;
        }
        if hide_unsafe {
            return confirmed.iter().any(|c| c == &self.profile_name);
        }
        true
    }
}

/// Profile candidate pre-validation (name + unresolved paths).
struct Candidate {
    name: String,
    profile: TerminalProfile,
    is_auto_detected: bool,
    icon: Option<String>,
}

/// Detect available profiles for the current platform, VS Code-style:
/// auto-detected first, config profiles override by name (null deletes),
/// validated paths in fallback order, PATH resolution last.
pub fn detect_available_profiles(cfg: &TerminalConfig) -> Vec<DetectedProfile> {
    detect_available_profiles_in(cfg, &real_env(), real_windows_build())
}

/// Testable core: same algorithm, injected env.
pub fn detect_available_profiles_in(
    cfg: &TerminalConfig,
    env: &DetectEnv,
    windows_build: Option<u32>,
) -> Vec<DetectedProfile> {
    let mut candidates: Vec<Candidate> = Vec::new();
    if env.platform == Platform::Windows {
        detect_windows_candidates(env, windows_build, &mut candidates);
        if cfg.use_wsl_profiles {
            collect_wsl_candidates(env, windows_build, &mut candidates);
        }
    } else {
        detect_unix_candidates(env, &mut candidates);
    }

    // applyConfigProfilesToMap — config overrides by name; delete marker
    // (empty path + no source) removes a detected profile.
    let mut by_name: HashMap<String, (TerminalProfile, bool)> = candidates
        .into_iter()
        .map(|c| {
            let mut p = c.profile;
            if p.icon.is_none() {
                p.icon = c.icon.clone();
            }
            (c.name, (p, c.is_auto_detected))
        })
        .collect();
    if let Some(configured) = cfg.profiles_for(env.platform) {
        for (name, value) in configured {
            if value.path.is_empty() && value.source.is_none() {
                // Delete marker (toml has no null; VS Code `null` delete).
                by_name.remove(name);
            } else {
                by_name.insert(name.clone(), (value.clone(), false));
            }
        }
    }

    let default_name = cfg
        .default_profile_name_for(env.platform)
        .map(|s| s.to_string());
    let mut out = Vec::new();
    for (name, (profile, auto)) in &by_name {
        if let Some(mut d) = validate_profile_paths(name, profile, env) {
            d.is_auto_detected = *auto;
            d.is_default = default_name.as_deref() == Some(name.as_str());
            out.push(d);
        }
    }
    out.sort_by(|a, b| a.profile_name.cmp(&b.profile_name));
    out
}

fn push_candidate(
    candidates: &mut Vec<Candidate>,
    name: &str,
    profile: TerminalProfile,
    auto: bool,
    icon: Option<&str>,
) {
    candidates.push(Candidate {
        name: name.to_string(),
        profile,
        is_auto_detected: auto,
        icon: icon.map(|s| s.to_string()),
    });
}

fn unsafe_path(p: String) -> UnsafePath {
    UnsafePath {
        path: p,
        is_unsafe: true,
    }
}

/// Windows auto-detected candidates — VS Code `detectAvailableWindowsProfiles`.
fn detect_windows_candidates(
    env: &DetectEnv,
    _build: Option<u32>,
    candidates: &mut Vec<Candidate>,
) {
    // Sysnative when a 32-bit process runs on 64-bit Windows (PSReadline).
    let is32_on_64 = env.env_any("PROCESSOR_ARCHITEW6432").is_some();
    let windir = env
        .env_any("windir")
        .cloned()
        .unwrap_or_else(|| r"C:\Windows".into());
    let system32 = if is32_on_64 {
        format!(r"{windir}\Sysnative")
    } else {
        format!(r"{windir}\System32")
    };

    // PowerShell (source: PowerShell) — enumerate all installs.
    let pwsh = TerminalProfile {
        source: Some(ProfileSource::PowerShell),
        ..TerminalProfile::default()
    };
    push_candidate(
        candidates,
        "PowerShell",
        pwsh,
        true,
        Some("terminal-powershell"),
    );

    // Windows PowerShell (opt-in semantics preserved: only validated when the
    // exe exists — 32-bit System32 only offers if the file is really there).
    let wps = TerminalProfile {
        path: vec![format!(r"{system32}\WindowsPowerShell\v1.0\powershell.exe").map_string()],
        ..TerminalProfile::default()
    };
    push_candidate(
        candidates,
        "Windows PowerShell",
        wps,
        true,
        Some("terminal-powershell"),
    );

    // Git Bash (source: GitBash).
    let gb = TerminalProfile {
        source: Some(ProfileSource::GitBash),
        args: vec!["--login".into(), "-i".into()],
        ..TerminalProfile::default()
    };
    push_candidate(candidates, "Git Bash", gb, true, Some("terminal-gitbash"));

    // Command Prompt.
    let cmd = TerminalProfile {
        path: vec![format!(r"{system32}\cmd.exe").map_string()],
        ..TerminalProfile::default()
    };
    push_candidate(
        candidates,
        "Command Prompt",
        cmd,
        true,
        Some("terminal-cmd"),
    );

    // Cygwin — unsafe until confirmed.
    let homedrive = env
        .env_any("HOMEDRIVE")
        .cloned()
        .unwrap_or_else(|| "C:".into());
    let cygwin = TerminalProfile {
        path: vec![
            unsafe_path(format!(r"{homedrive}\cygwin64\bin\bash.exe")),
            unsafe_path(format!(r"{homedrive}\cygwin\bin\bash.exe")),
        ],
        args: vec!["--login".into()],
        is_unsafe: true,
        ..TerminalProfile::default()
    };
    push_candidate(candidates, "Cygwin", cygwin, true, Some("terminal-linux"));

    // bash (MSYS2) — unsafe until confirmed; CHERE_INVOKING keeps cwd.
    let mut msys_env = HashMap::new();
    msys_env.insert("CHERE_INVOKING".to_string(), "1".to_string());
    let msys = TerminalProfile {
        path: vec![unsafe_path(format!(r"{homedrive}\msys64\usr\bin\bash.exe"))],
        args: vec!["--login".into(), "-i".into()],
        env: Some(msys_env),
        is_unsafe: true,
        icon: Some("terminal-bash".into()),
        ..TerminalProfile::default()
    };
    push_candidate(
        candidates,
        "bash (MSYS2)",
        msys,
        true,
        Some("terminal-bash"),
    );

    // Cmder — safe iff CMDER_ROOT provided the init script.
    let cmder_root = env
        .env_any("CMDER_ROOT")
        .cloned()
        .unwrap_or_else(|| format!(r"{homedrive}\cmder"));
    let cmder_init = format!(r"{cmder_root}\vendor\bin\vscode_init.cmd");
    let cmder = TerminalProfile {
        path: vec![format!(r"{system32}\cmd.exe").map_string()],
        args: vec!["/K".into(), cmder_init.clone()],
        // Safe only when derived from CMDER_ROOT (VS Code requiresPath).
        is_unsafe: !env.env_any("CMDER_ROOT").is_some(),
        ..TerminalProfile::default()
    };
    // Only offer Cmder when the init script exists (VS Code requiresPath).
    if env.file_exists(&cmder_init) {
        push_candidate(candidates, "Cmder", cmder, true, None);
    }
}

/// WSL distro profiles — VS Code `getWslProfiles` (build ≥ 19041, docker
/// desktop distros skipped, `wsl.exe -d <name>` = F10 path translation).
fn collect_wsl_candidates(env: &DetectEnv, build: Option<u32>, candidates: &mut Vec<Candidate>) {
    let allow = match build {
        Some(b) => b >= 19041,
        None => env.platform == Platform::Windows,
    };
    if !allow {
        return;
    }
    let windir = env
        .env_any("windir")
        .cloned()
        .unwrap_or_else(|| r"C:\Windows".into());
    let is32_on_64 = env.env_any("PROCESSOR_ARCHITEW6432").is_some();
    let wsl_exe = if is32_on_64 {
        format!(r"{windir}\Sysnative\wsl.exe")
    } else {
        format!(r"{windir}\System32\wsl.exe")
    };
    for distro in &env.wsl_distros {
        if distro.starts_with("docker-desktop") {
            continue;
        }
        let profile = TerminalProfile {
            path: vec![wsl_exe.clone().map_string()],
            args: vec!["-d".into(), distro.clone()],
            backend: TerminalBackend::Wsl,
            ..TerminalProfile::default()
        };
        let name = format!("{distro} (WSL)");
        let icon = if distro.contains("Ubuntu") {
            "terminal-ubuntu"
        } else if distro.contains("Debian") {
            "terminal-debian"
        } else {
            "terminal-linux"
        };
        push_candidate(candidates, &name, profile, false, Some(icon));
    }
}

/// Unix detection — `$SHELL` first, then `/etc/shells` with basename + `(n)`
/// dedupe (VS Code `detectAvailableUnixProfiles`).
fn detect_unix_candidates(env: &DetectEnv, candidates: &mut Vec<Candidate>) {
    let mut seen: HashMap<String, usize> = HashMap::new();

    let mut add_shell = |path: String, candidates: &mut Vec<Candidate>| {
        let base = Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        let name = if *count > 1 {
            format!("{base} ({count})")
        } else {
            base
        };
        let profile = TerminalProfile {
            path: vec![path.clone().map_string()],
            ..TerminalProfile::default()
        };
        push_candidate(candidates, &name, profile, true, None);
    };

    // $SHELL first (unix default per spec: macOS/Linux = $SHELL).
    if let Some(shell) = env.env_unix.get("SHELL") {
        if !shell.is_empty() {
            add_shell(shell.clone(), candidates);
        }
    }
    // /etc/shells (exists check like VS Code).
    if let Some(contents) = &env.etc_shells {
        for line in contents.lines() {
            let line = match line.find('#') {
                Some(i) => &line[..i],
                None => line,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            add_shell(line.to_string(), candidates);
        }
    }
}

/// Git Bash source paths — VS Code `getGitBashPaths` (git.exe on PATH first,
/// then ProgramFiles roots, then scoop).
fn git_bash_paths(env: &DetectEnv) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    if let Some(git_exe) = env.find_executable("git.exe") {
        // `<installdir>/cmd/git.exe` → installdir
        if let Some(dir) = parent_dir(&git_exe) {
            if let Some(install) = parent_dir(dir) {
                dirs.push(install.to_string());
            }
        }
    }
    for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(X86)"] {
        if let Some(v) = env.env_any(key) {
            dirs.push(v.clone());
        }
    }
    if let Some(v) = env.env_any("LocalAppData") {
        dirs.push(format!(r"{v}\Program"));
    }
    let mut paths = Vec::new();
    for d in dirs {
        paths.push(format!(r"{d}\Git\bin\bash.exe"));
        paths.push(format!(r"{d}\Git\usr\bin\bash.exe"));
        paths.push(format!(r"{d}\usr\bin\bash.exe")); // Git SDK layout
    }
    if let Some(up) = env.env_any("UserProfile") {
        paths.push(format!(r"{up}\scoop\apps\git\current\bin\bash.exe"));
        paths.push(format!(
            r"{up}\scoop\apps\git-with-openssh\current\bin\bash.exe"
        ));
    }
    paths
}

/// PowerShell source paths — VS Code `getPowershellPaths`:
/// highest-version `<ProgramFiles>\PowerShell\<n>\pwsh.exe`, then MSIX
/// `Microsoft.PowerShell_*` under LocalAppData WindowsApps, then scoop.
fn powershell_paths(env: &DetectEnv) -> Vec<String> {
    let mut paths = Vec::new();
    // Program Files installs (highest version wins first).
    for pf in ["ProgramFiles", "ProgramW6432", "ProgramFiles(X86)"] {
        let Some(pf_dir) = env.env_any(pf) else {
            continue;
        };
        let base = format!(r"{pf_dir}\PowerShell");
        if !env.dir_exists(&base) {
            continue;
        }
        let mut best: Option<(i64, String)> = None;
        for dir in &env.pwsh_install_dirs {
            // dir is the relative version dir, e.g. "7" or "7-preview".
            let (ver_num, preview) = match dir.split_once('-') {
                Some((n, "preview")) => (n.parse::<i64>().unwrap_or(-1), true),
                _ => (dir.parse::<i64>().unwrap_or(-1), false),
            };
            if ver_num < 0 {
                continue;
            }
            let _ = preview; // stable enumeration: integer dirs only in this pass
            let exe = format!(r"{base}\{dir}\pwsh.exe");
            if !env.file_exists(&exe) {
                continue;
            }
            let better = match &best {
                Some((v, _)) => ver_num > *v,
                None => true,
            };
            if better {
                best = Some((ver_num, exe));
            }
        }
        if let Some((_, exe)) = best {
            paths.push(exe);
        }
    }
    // MSIX (Store) installs.
    if let Some(lad) = env.env_any("LocalAppData") {
        let apps = format!(r"{lad}\Microsoft\WindowsApps");
        if env.dir_exists(&apps) {
            for sub in &env.msix_app_dirs {
                if sub.starts_with("Microsoft.PowerShell_") {
                    paths.push(format!(r"{apps}\{sub}\pwsh.exe"));
                }
            }
        }
    }
    // scoop pwsh.
    if let Some(up) = env.env_any("UserProfile") {
        paths.push(format!(r"{up}\scoop\apps\pwsh\current\pwsh.exe"));
        paths.push(format!(r"{up}\scoop\apps\pwsh-preview\current\pwsh.exe"));
    }
    paths
}

/// Parent directory in either separator convention (`/` or `\`), so the
/// Windows algorithm stays host-independent.
fn parent_dir(p: &str) -> Option<&str> {
    let idx = p.rfind(['/', '\\'])?;
    Some(&p[..idx])
}

trait MapString {
    fn map_string(self) -> UnsafePath;
}
impl MapString for String {
    fn map_string(self) -> UnsafePath {
        UnsafePath {
            path: self,
            is_unsafe: false,
        }
    }
}

/// VS Code `initializeWindowsProfiles` — resolve source-contract profiles.
fn resolve_source(source: ProfileSource, env: &DetectEnv) -> (Vec<String>, Vec<String>) {
    match source {
        ProfileSource::GitBash => (git_bash_paths(env), vec!["--login".into(), "-i".into()]),
        ProfileSource::PowerShell => (powershell_paths(env), Vec::new()),
    }
}

/// VS Code `validateProfilePaths` — walk the fallback array; bare names
/// resolve on `$PATH` (`isFromPath`); unsafe entries flag the profile.
fn validate_profile_paths(
    name: &str,
    profile: &TerminalProfile,
    env: &DetectEnv,
) -> Option<DetectedProfile> {
    // (path, is_unsafe): config arrays carry `isUnsafe` per entry; paths that
    // come from a `source` contract are first-party installs (safe).
    let (paths, default_args): (Vec<(String, bool)>, Vec<String>) = match profile.source {
        Some(src) => {
            let (p, a) = resolve_source(src, env);
            (p.into_iter().map(|p| (p, false)).collect(), a)
        }
        None => (
            profile
                .path
                .iter()
                .map(|e| (e.path.clone(), e.is_unsafe))
                .collect(),
            Vec::new(),
        ),
    };
    let args = if profile.args.is_empty() {
        default_args
    } else {
        profile.args.clone()
    };

    for (p, is_unsafe) in paths {
        if p.is_empty() {
            continue;
        }
        // Bare name → `$PATH` lookup; path with a separator → filesystem
        // existence. Separator detection is convention-based (`/` or `\`),
        // not `Path`, so Windows paths behave the same off-Windows.
        let (resolved, is_from_path) = if !p.contains('/') && !p.contains('\\') {
            match env.find_executable(&p) {
                Some(resolved) => (resolved, true),
                None => continue,
            }
        } else if env.file_exists(&p) {
            (p, false)
        } else {
            continue;
        };
        // WSL profiles carry their distro in `-d <distro>`; surface it so the
        // UI can label/relaunch a distro without re-parsing args.
        let wsl_distro = if profile.backend == TerminalBackend::Wsl {
            args.windows(2).find(|w| w[0] == "-d").map(|w| w[1].clone())
        } else {
            None
        };
        return Some(DetectedProfile {
            profile_name: name.to_string(),
            path: resolved,
            is_unsafe_path: is_unsafe,
            is_from_path,
            // Overwritten by the caller: config overrides are not auto-detected.
            is_auto_detected: true,
            is_default: false,
            args,
            env: profile.env.clone(),
            icon: profile.icon.clone(),
            backend: profile.backend,
            source: profile.source,
            wsl_distro,
            cwd: profile.cwd.clone(),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// P54.2/.3 — PTY host (portable-pty; unix pty / Windows ConPTY)
// ---------------------------------------------------------------------------

/// Errors surfaced to the UI (typed, actionable — spec §12 style).
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("pty not found: {0}")]
    PtyNotFound(String),
    #[error("pty already exited: {0}")]
    PtyExited(String),
    #[error("remote backend not available in this build (H33 attach pending)")]
    RemoteUnavailable,
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("io: {0}")]
    Io(String),
}

/// Who owns a PTY session. This is the whole point of the unified plane: a
/// human tab is authorized by the user's own gesture, while an agent/task tab
/// is ticket-authorized and rendered read-only, so the user can *watch* the
/// agent work in the same terminal view instead of in an invisible pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TerminalOrigin {
    #[default]
    Human,
    /// AgentCowork Native `script.run` (the agent's own shell).
    Agent,
    /// A durable task / automation run.
    Task,
}

impl TerminalOrigin {
    pub fn is_human(self) -> bool {
        matches!(self, TerminalOrigin::Human)
    }
}

/// Options for one spawn. `integration` is explicit rather than read from
/// config here, so the automation path can force it off for a non-interactive
/// command runner that has no prompt to hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnOpts {
    pub origin: TerminalOrigin,
    pub integration: bool,
}

impl Default for SpawnOpts {
    fn default() -> Self {
        Self {
            origin: TerminalOrigin::Human,
            integration: true,
        }
    }
}

/// A frame from a PTY reader thread. `Data` carries only bytes that should be
/// rendered — shell-integration sequences are already removed.
#[derive(Debug, Clone)]
pub enum PtyFrame {
    /// Bytes for the renderer.
    Data(Vec<u8>),
    /// A command the shell reported finishing (with exit code + output).
    Command(Box<CommandRecord>),
    /// The shell's working directory changed.
    Cwd(String),
    /// The child exited.
    Exit(Option<u32>),
}

/// A live PTY session (H36 `PtySession` shape): process survives client
/// disconnect; the UI attaches by `pty_id`. The reaper thread owns the child
/// (wait/reap); the session keeps a killer clone so `kill` works while the
/// reaper blocks in `wait`.
pub struct PtySession {
    pub pty_id: String,
    pub profile_id: String,
    pub backend: TerminalBackend,
    /// P67 — human / agent / task provenance.
    pub origin: TerminalOrigin,
    /// Display label for non-human tabs (`script.run`, task name).
    pub label: Option<String>,
    /// Which shell integration script was injected (and its quality), if any.
    pub integration: Option<IntegrationShell>,
    /// Scratch dir holding the generated integration script (removed on kill).
    pub scratch: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
    pub pid: Option<u32>,
    /// Structured facts the shell reported — the source for the terminal's
    /// cwd, exit decorations, recent-command picker and chat context.
    pub tracker: Arc<Mutex<CommandTracker>>,
    /// P68.8 — retained rendered output for replay after a view reattach.
    /// Written by the reader thread; read by [`PtyHost::replay`].
    pub ring: Arc<Mutex<TerminalRing>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    exited: Arc<AtomicU64>, // 0 = running; otherwise exit-code bits
}

impl PtySession {
    pub fn write(&self, data: &[u8]) -> Result<(), TerminalError> {
        let mut w = self
            .writer
            .lock()
            .map_err(|e| TerminalError::Io(e.to_string()))?;
        w.write_all(data)
            .and_then(|_| w.flush())
            .map_err(|e| TerminalError::Io(e.to_string()))
    }

    pub fn kill(&self) {
        // A session that already exited fails here; that is not an error
        // (kill is best-effort teardown, reaping is the reaper thread's job).
        let _ = self.killer.clone_killer().kill();
    }
}

/// One PTY master handle kept alive for resize/kill after spawn.
struct PtyEntry {
    session: Arc<PtySession>,
    master: Box<dyn portable_pty::MasterPty + Send>,
}

/// The PTY host — owns live sessions keyed by `pty_id`. Dropped sessions are
/// killed + reaped (drop of `PtyEntry` kills the child via ChildKiller).
pub struct PtyHost {
    ptys: Mutex<HashMap<String, PtyEntry>>,
    /// `native_pty_system()` yields `Box<dyn PtySystem + Send>` (not `Sync`),
    /// so the host must hold it behind a lock to stay `Sync` and live in
    /// Tauri's shared state. Only `openpty` touches it — a brief lock.
    pty_system: Mutex<Box<dyn portable_pty::PtySystem + Send>>,
    counter: AtomicU64,
    /// P68.8 — replay-ring capacity (bytes) for sessions spawned from now on,
    /// set once at boot from `terminal.scrollbackBytes`. Read at spawn so a
    /// live session's scrollback never silently shrinks under the user.
    scrollback_bytes: Arc<AtomicUsize>,
}

impl Default for PtyHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyHost {
    pub fn new() -> Self {
        Self {
            ptys: Mutex::new(HashMap::new()),
            pty_system: Mutex::new(portable_pty::native_pty_system()),
            counter: AtomicU64::new(1),
            scrollback_bytes: Arc::new(AtomicUsize::new(DEFAULT_SCROLLBACK_BYTES)),
        }
    }

    /// P68.8 — set the replay-ring capacity (bytes) for future sessions,
    /// clamped to the supported range. Called once at boot from
    /// `terminal.scrollbackBytes`.
    pub fn set_scrollback_bytes(&self, bytes: usize) {
        self.scrollback_bytes.store(
            bytes.clamp(MIN_SCROLLBACK_BYTES, MAX_SCROLLBACK_BYTES),
            Ordering::Relaxed,
        );
    }

    /// The capacity future sessions will be spawned with.
    pub fn scrollback_bytes(&self) -> usize {
        self.scrollback_bytes.load(Ordering::Relaxed)
    }

    fn next_pty_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("pty-{n}")
    }

    /// Spawn a profile into a real PTY. Returns `(pty_id, PtyOutput)`; the
    /// reader thread started by [`PtyOutput::stream`] delivers [`PtyFrame`]s.
    pub fn spawn_profile(
        &self,
        profile: &DetectedProfile,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<(String, PtyOutput), TerminalError> {
        self.spawn_with(profile, cwd, rows, cols, SpawnOpts::default())
    }

    /// P67 — the full spawn path, with provenance + shell integration.
    pub fn spawn_with(
        &self,
        profile: &DetectedProfile,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
        opts: SpawnOpts,
    ) -> Result<(String, PtyOutput), TerminalError> {
        if profile.backend == TerminalBackend::Remote {
            // H33 v1 attach — fail closed, never silently local.
            return Err(TerminalError::RemoteUnavailable);
        }
        let pty_id = self.next_pty_id();
        let integration = if opts.integration {
            setup_integration(&pty_id, profile)
        } else {
            None
        };
        let tracker = Arc::new(Mutex::new(CommandTracker::new()));

        let pair = {
            let sys = self
                .pty_system
                .lock()
                .map_err(|e| TerminalError::Io(e.to_string()))?;
            sys.openpty(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::Spawn(e.to_string()))?
        };

        let mut cmd = portable_pty::CommandBuilder::new(&profile.path);
        cmd.args(&profile.args);
        if let Some(env) = &profile.env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        if let Some(int) = &integration {
            // Injection is additive: the profile's own args/env stay intact and
            // the generated wrapper sources the user's own rc first, so
            // aliases, prompt and PATH behave exactly as in a plain terminal.
            cmd.args(&int.extra_args);
            cmd.env(NONCE_ENV, &int.nonce);
            cmd.env(INJECTION_ENV, "1");
            for (k, v) in &int.extra_env {
                cmd.env(k, v);
            }
            cmd.env("TERM_PROGRAM", "agentcowork");
        }
        if profile.backend == TerminalBackend::Wsl {
            // F10 owns path translation; the profile runs `wsl.exe -d <d>`,
            // which starts in the distro's home — keep Windows cwd mapping
            // out of this layer (spec: F10 consumes path translation).
            cmd.env("WSL_UTF8", "1");
        }
        if let Some(cwd) = cwd.or(profile.cwd.as_deref()) {
            if !cwd.is_empty() {
                cmd.cwd(cwd);
            }
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| TerminalError::Spawn(e.to_string()))?;
        let pid = child.process_id();

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| TerminalError::Io(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| TerminalError::Io(e.to_string()))?;

        let exited = Arc::new(AtomicU64::new(0));
        let killer: Box<dyn portable_pty::ChildKiller + Send + Sync> = child.clone_killer();

        // Reaper thread: wait() reaps the zombie; exit code published to
        // `exited` so status polls never report a stale "running".
        let exited_for_reaper = exited.clone();
        let mut child_box: Box<dyn portable_pty::Child + Send + Sync> = child;
        std::thread::Builder::new()
            .name(format!("pty-reap-{pty_id}"))
            .spawn(move || match child_box.wait() {
                Ok(status) => {
                    let code = portable_pty::ExitStatus::exit_code(&status) as u64;
                    exited_for_reaper.store(code, Ordering::Release);
                }
                Err(_) => exited_for_reaper.store(u64::MAX, Ordering::Release),
            })
            .map_err(|e| TerminalError::Io(e.to_string()))?;

        // P68.8 — this session's replay ring. Capacity is snapshotted here so a
        // later config change cannot retroactively resize a live scrollback.
        let ring = Arc::new(Mutex::new(TerminalRing::new(self.scrollback_bytes())));
        let session = Arc::new(PtySession {
            pty_id: pty_id.clone(),
            profile_id: profile.profile_name.clone(),
            backend: profile.backend,
            origin: opts.origin,
            label: None,
            integration: integration.as_ref().map(|i| i.shell),
            scratch: integration.as_ref().map(|i| i.dir.clone()),
            rows,
            cols,
            pid,
            tracker: tracker.clone(),
            ring: ring.clone(),
            writer: Mutex::new(writer),
            killer,
            exited: exited.clone(),
        });

        let parser = integration
            .as_ref()
            .map(|i| IntegrationParser::new(true, Some(i.nonce.clone())));
        let output = PtyOutput::new(&pty_id, reader, exited.clone(), parser, tracker, ring);

        self.ptys
            .lock()
            .map_err(|e| TerminalError::Io(e.to_string()))?
            .insert(
                pty_id.clone(),
                PtyEntry {
                    session,
                    master: pair.master,
                },
            );
        Ok((pty_id, output))
    }

    /// Write raw bytes to the pty master (human keystrokes or pastes).
    pub fn write(&self, pty_id: &str, data: &[u8]) -> Result<(), TerminalError> {
        let session = {
            let map = self
                .ptys
                .lock()
                .map_err(|e| TerminalError::Io(e.to_string()))?;
            map.get(pty_id)
                .map(|e| e.session.clone())
                .ok_or_else(|| TerminalError::PtyNotFound(pty_id.to_string()))?
        };
        session.write(data)
    }

    /// Resize the pty pair.
    pub fn resize(&self, pty_id: &str, rows: u16, cols: u16) -> Result<(), TerminalError> {
        let map = self
            .ptys
            .lock()
            .map_err(|e| TerminalError::Io(e.to_string()))?;
        let entry = map
            .get(pty_id)
            .ok_or_else(|| TerminalError::PtyNotFound(pty_id.to_string()))?;
        entry
            .master
            .resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::Io(e.to_string()))
    }

    /// Exit state: `None` = running, `Some(code)` = reaped exit code.
    pub fn exit_code(&self, pty_id: &str) -> Result<Option<u32>, TerminalError> {
        let session = {
            let map = self
                .ptys
                .lock()
                .map_err(|e| TerminalError::Io(e.to_string()))?;
            map.get(pty_id)
                .map(|e| e.session.clone())
                .ok_or_else(|| TerminalError::PtyNotFound(pty_id.to_string()))?
        };
        let v = session.exited.load(Ordering::Acquire);
        if v == 0 {
            Ok(None)
        } else if v == u64::MAX {
            Ok(Some(u32::MAX))
        } else {
            Ok(Some((v & 0xFFFF_FFFFu64) as u32))
        }
    }

    /// Kill + reap + remove. Dropping the master closes the pty; the child
    /// gets SIGHUP / ConPTY close.
    pub fn kill(&self, pty_id: &str) -> Result<bool, TerminalError> {
        let removed = self
            .ptys
            .lock()
            .map_err(|e| TerminalError::Io(e.to_string()))?
            .remove(pty_id);
        match removed {
            Some(entry) => {
                // Killer fires; the reaper thread's wait() returns and reaps.
                // Dropping the master closes the pty (SIGHUP / ConPTY close).
                entry.session.kill();
                // Best-effort scratch cleanup: the generated scripts are not
                // secrets (the nonce travels in the env, never the file), but
                // a per-session dir should not outlive its session.
                if let Some(dir) = &entry.session.scratch {
                    let _ = std::fs::remove_dir_all(dir);
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Structured facts (cwd, command records) for a live session. `None` when
    /// the pty is unknown — never an empty tracker that reads as "nothing ran".
    pub fn tracker(&self, pty_id: &str) -> Option<Arc<Mutex<CommandTracker>>> {
        let map = self.ptys.lock().ok()?;
        map.get(pty_id).map(|e| e.session.tracker.clone())
    }

    /// P68.8 — replay a session's retained output.
    ///
    /// `from_seq = None` returns everything still retained (a view reattaching
    /// with nothing rendered); `Some(n)` returns only what the caller has not
    /// seen. A cursor that has already been evicted yields a nonzero
    /// `dropped`, so the renderer can say the view is truncated instead of
    /// presenting a seamless scrollback that lies about the gap.
    ///
    /// Reads a live session's ring, so it works after the original reader
    /// callback has been replaced by a reattached view — and equally when no
    /// listener was ever attached.
    pub fn replay(
        &self,
        pty_id: &str,
        from_seq: Option<u64>,
    ) -> Result<TerminalReplay, TerminalError> {
        let ring = {
            let map = self
                .ptys
                .lock()
                .map_err(|e| TerminalError::Io(e.to_string()))?;
            map.get(pty_id)
                .map(|e| e.session.ring.clone())
                .ok_or_else(|| TerminalError::PtyNotFound(pty_id.to_string()))?
        };
        let ring = ring.lock().map_err(|e| TerminalError::Io(e.to_string()))?;
        let (seq, bytes, dropped) = match from_seq {
            None => {
                let (seq, bytes) = ring.snapshot();
                (seq, bytes, 0)
            }
            Some(from) => ring.since(from),
        };
        Ok(TerminalReplay {
            seq,
            bytes,
            dropped,
            capacity: ring.capacity(),
        })
    }

    /// Full session view for the status chip + tab strip.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        match self.ptys.lock() {
            Ok(map) => map
                .values()
                .map(|e| SessionInfo {
                    pty_id: e.session.pty_id.clone(),
                    profile_id: e.session.profile_id.clone(),
                    backend: e.session.backend,
                    origin: e.session.origin,
                    label: e.session.label.clone(),
                    integration: e.session.integration.map(|s| s.quality()),
                    pid: e.session.pid,
                    cwd: e
                        .session
                        .tracker
                        .lock()
                        .ok()
                        .map(|t| t.cwd().to_string())
                        .unwrap_or_default(),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Kill everything (app shutdown / session switch teardown).
    pub fn kill_all(&self) {
        let ids: Vec<String> = match self.ptys.lock() {
            Ok(map) => map.keys().cloned().collect(),
            Err(_) => return,
        };
        for id in ids {
            let _ = self.kill(&id);
        }
    }

    /// Live pty rows for the status chip.
    pub fn status(&self) -> Vec<(String, String, TerminalBackend)> {
        match self.ptys.lock() {
            Ok(map) => map
                .values()
                .map(|e| {
                    (
                        e.session.pty_id.clone(),
                        e.session.profile_id.clone(),
                        e.session.backend,
                    )
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Handle to a PTY output stream. The reader thread removes shell-integration
/// sequences, keeps the [`CommandTracker`] current, and emits [`PtyFrame`]s.
/// It never interprets VT — xterm.js owns that.
pub struct PtyOutput {
    pub pty_id: String,
    reader: Box<dyn Read + Send>,
    exited: Arc<AtomicU64>,
    parser: Option<IntegrationParser>,
    tracker: Arc<Mutex<CommandTracker>>,
    /// P68.8 — the session's replay ring; every rendered byte is retained here
    /// before it is handed to the (possibly absent) listener.
    ring: Arc<Mutex<TerminalRing>>,
}

impl PtyOutput {
    fn new(
        pty_id: &str,
        reader: Box<dyn Read + Send>,
        exited: Arc<AtomicU64>,
        parser: Option<IntegrationParser>,
        tracker: Arc<Mutex<CommandTracker>>,
        ring: Arc<Mutex<TerminalRing>>,
    ) -> Self {
        Self {
            pty_id: pty_id.to_string(),
            reader,
            exited,
            parser,
            tracker,
            ring,
        }
    }

    /// Start streaming. `on_frame` receives render bytes, command records,
    /// cwd changes and finally the exit frame, in arrival order.
    pub fn stream<F>(self, mut on_frame: F) -> std::io::Result<()>
    where
        F: FnMut(PtyFrame) + Send + 'static,
    {
        let pty_id = self.pty_id.clone();
        let exited = self.exited.clone();
        let mut reader = self.reader;
        let mut parser = self.parser;
        let tracker = self.tracker.clone();
        let ring = self.ring.clone();
        std::thread::Builder::new()
            .name(format!("pty-read-{pty_id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) => break, // eof: pty closed
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    let chunk = &buf[..n];
                    match parser.as_mut() {
                        None => {
                            // Retain before handing off: a consumer that never
                            // attaches (a closed view) must not cost the user
                            // the output.
                            if let Ok(mut r) = ring.lock() {
                                r.push(chunk);
                            }
                            on_frame(PtyFrame::Data(chunk.to_vec()))
                        }
                        Some(p) => {
                            for seg in p.feed_segments(chunk) {
                                match seg {
                                    Segment::Bytes(b) => {
                                        // Bytes land in the tracker *before*
                                        // they are rendered, and in the same
                                        // order the shell emitted them, so a
                                        // completed command carries exactly its
                                        // own output.
                                        if let Ok(mut t) = tracker.lock() {
                                            t.push_output(&b);
                                        }
                                        if let Ok(mut r) = ring.lock() {
                                            r.push(&b);
                                        }
                                        on_frame(PtyFrame::Data(b));
                                    }
                                    Segment::Event(ev) => {
                                        let mut closed: Option<CommandRecord> = None;
                                        let mut cwd: Option<String> = None;
                                        if let Ok(mut t) = tracker.lock() {
                                            t.apply(&ev);
                                            match &ev {
                                                IntegrationEvent::CommandFinished(_) => {
                                                    closed = t.last().cloned();
                                                }
                                                IntegrationEvent::Cwd(p) => cwd = Some(p.clone()),
                                                _ => {}
                                            }
                                        }
                                        if let Some(rec) = closed {
                                            on_frame(PtyFrame::Command(Box::new(rec)));
                                        }
                                        if let Some(p) = cwd {
                                            on_frame(PtyFrame::Cwd(p));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // A truncated sequence must not be swallowed silently.
                if let Some(p) = parser.as_mut() {
                    let tail = p.flush();
                    if !tail.is_empty() {
                        if let Ok(mut r) = ring.lock() {
                            r.push(&tail);
                        }
                        on_frame(PtyFrame::Data(tail));
                    }
                }
                let code = exited.load(Ordering::Acquire);
                let code = if code == 0 || code == u64::MAX {
                    None
                } else {
                    Some((code & 0xFFFF_FFFFu64) as u32)
                };
                on_frame(PtyFrame::Exit(code));
            })?;
        Ok(())
    }
}

/// What the Shell view needs to render a session row: identity, provenance,
/// backend, and the shell's live cwd (when integration is active).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub pty_id: String,
    pub profile_id: String,
    pub backend: TerminalBackend,
    pub origin: TerminalOrigin,
    /// `script.run` / task label for non-human tabs.
    pub label: Option<String>,
    /// `"Rich"` when a shell-integration script is active, else `None`.
    pub integration: Option<&'static str>,
    pub pid: Option<u32>,
    pub cwd: String,
}

// ---------------------------------------------------------------------------
// P54.5 — plane read models (one engine, two façades)
// ---------------------------------------------------------------------------

/// One shell-reported command, as both façades serve it.
///
/// `trusted: false` means the line could not be attributed to the session's own
/// nonce — an *indirect* injection (tool/web output printing a forged prompt)
/// must never enter the agent's context as fact, so the flag travels with the
/// row rather than being decided by each caller.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCommandView {
    pub command: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub output: String,
    pub trusted: bool,
    pub failed: bool,
}

/// One session row for the status surface (Shell tab strip + relay read).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionView {
    pub pty_id: String,
    pub profile_id: String,
    pub backend: TerminalBackend,
    /// `human` is a user tab; `agent`/`task` are read-only.
    pub origin: TerminalOrigin,
    /// `script.run` / task label for non-human tabs.
    pub label: Option<String>,
    /// `"Rich"` when a shell-integration script is active, else `None`.
    pub integration: Option<&'static str>,
    pub cwd: String,
    pub pid: Option<u32>,
    /// `exit_code.is_none()` — a session that already exited is not running.
    pub running: bool,
    pub exit_code: Option<u32>,
}

/// What the one PTY plane currently looks like.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPlaneStatus {
    /// `false` when this host has no PTY host at all. A caller must render that
    /// as "no shell on this host", never as "zero sessions".
    pub attached: bool,
    pub count: usize,
    pub ptys: Vec<TerminalSessionView>,
}

/// The honest status for a host with no PTY host: `attached: false`, not an
/// empty session list that reads as "a shell with nothing running".
pub fn detached_plane_status() -> TerminalPlaneStatus {
    TerminalPlaneStatus {
        attached: false,
        count: 0,
        ptys: Vec::new(),
    }
}

/// A **read-only** view of the one PTY plane.
///
/// Deliberately cannot run anything. The privileged shell path is the ticketed
/// `script.run` tool (`tool/exec` → `tool/commit` → Guard-2 →
/// [`TerminalExecutor`](crate::tools::TerminalExecutor)), so an observer can
/// never become a second, unticketed executor for a privileged effect. The
/// relay's `terminal/*` arm and the Tauri read commands both stand on this,
/// which is what keeps the agent-facing and UI-facing views of one session
/// identical instead of two hand-rolled projections that drift.
pub trait TerminalPlaneObserver: Send + Sync {
    /// Live sessions, with provenance and the shell's own cwd.
    ///
    /// Named `plane_status` rather than `status` because `PtyHost` already has
    /// an inherent `status()` with a different (tuple) shape, and an inherent
    /// method silently wins method resolution over a trait method — which would
    /// have made this call return the wrong type rather than fail to compile.
    fn plane_status(&self) -> TerminalPlaneStatus;
    /// One session row, if it exists.
    fn session(&self, pty_id: &str) -> Option<TerminalSessionView>;
    /// The shell-reported records for one session (oldest first, bounded).
    fn commands(&self, pty_id: &str, limit: usize) -> Result<Vec<TerminalCommandView>, String>;
    /// The model-ready last-command block. `None` when the shell has not
    /// reported a trusted command yet — never a fabricated empty block.
    fn last_command(&self, pty_id: &str, max_chars: usize) -> Result<Option<String>, String>;
    /// Recent command history as a compact block.
    fn history(
        &self,
        pty_id: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<Option<String>, String>;
}

impl TerminalPlaneObserver for PtyHost {
    fn plane_status(&self) -> TerminalPlaneStatus {
        let ptys: Vec<TerminalSessionView> = self
            .sessions()
            .into_iter()
            .map(|s| {
                let exit_code = self.exit_code(&s.pty_id).ok().flatten();
                TerminalSessionView {
                    pty_id: s.pty_id,
                    profile_id: s.profile_id,
                    backend: s.backend,
                    origin: s.origin,
                    label: s.label,
                    integration: s.integration,
                    cwd: s.cwd,
                    pid: s.pid,
                    running: exit_code.is_none(),
                    exit_code,
                }
            })
            .collect();
        TerminalPlaneStatus {
            attached: true,
            count: ptys.len(),
            ptys,
        }
    }

    fn session(&self, pty_id: &str) -> Option<TerminalSessionView> {
        self.plane_status()
            .ptys
            .into_iter()
            .find(|s| s.pty_id == pty_id)
    }

    fn commands(&self, pty_id: &str, limit: usize) -> Result<Vec<TerminalCommandView>, String> {
        let tracker = self
            .tracker(pty_id)
            .ok_or_else(|| format!("no such pty: {pty_id}"))?;
        let t = tracker.lock().map_err(|e| e.to_string())?;
        Ok(t.recent(limit.clamp(1, MAX_REPLAY_COMMANDS))
            .into_iter()
            .map(|r| TerminalCommandView {
                command: r.command.clone(),
                cwd: r.cwd.clone(),
                exit_code: r.exit_code,
                output: r.output.clone(),
                trusted: r.trusted,
                failed: r.failed(),
            })
            .collect())
    }

    fn last_command(&self, pty_id: &str, max_chars: usize) -> Result<Option<String>, String> {
        let tracker = self
            .tracker(pty_id)
            .ok_or_else(|| format!("no such pty: {pty_id}"))?;
        let t = tracker.lock().map_err(|e| e.to_string())?;
        Ok(t.context_block(max_chars.clamp(1, MAX_CONTEXT_CHARS)))
    }

    fn history(
        &self,
        pty_id: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<Option<String>, String> {
        let tracker = self
            .tracker(pty_id)
            .ok_or_else(|| format!("no such pty: {pty_id}"))?;
        let t = tracker.lock().map_err(|e| e.to_string())?;
        Ok(t.history_block(
            limit.clamp(1, MAX_REPLAY_COMMANDS),
            max_chars.clamp(1, MAX_CONTEXT_CHARS),
        ))
    }
}

/// Upper bound on the session reads, so a caller cannot ask a façade for an
/// unbounded slice of the tracker (the ring is bounded; the *response* is too).
const MAX_REPLAY_COMMANDS: usize = 200;
/// Upper bound on a rendered context block. Mirrors the Tauri commands' clamp.
const MAX_CONTEXT_CHARS: usize = 64_000;

// ---------------------------------------------------------------------------
// P67 — shell-integration injection
// ---------------------------------------------------------------------------

/// Everything a spawn needs to make the shell talk OSC 633.
struct IntegrationSetup {
    shell: IntegrationShell,
    nonce: String,
    dir: PathBuf,
    extra_args: Vec<String>,
    extra_env: Vec<(String, String)>,
}

/// A per-session nonce. This is **not a secret**: any program the user runs
/// can read its own environment. What it prevents is the real attack — an
/// *indirect* injection where tool/web output prints a forged command line,
/// which must never enter the agent's context as fact. Same posture as VS
/// Code's post-2023 nonce.
fn new_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut x = nanos.rotate_left(17).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pid << 64)
        ^ 0xDEAD_BEEF_CAFE_F00D_u128;
    let mut out = String::with_capacity(32);
    for _ in 0..32 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push(char::from_digit(((x >> 60) & 0xF) as u32, 16).unwrap());
    }
    out
}

/// Write the integration script (+ a wrapper that preserves the user's own
/// rc file) and work out the args/env the shell needs.
///
/// Returns `None` for shells we have no script for — an honest "no structured
/// facts" rather than a silently broken shell.
fn setup_integration(pty_id: &str, profile: &DetectedProfile) -> Option<IntegrationSetup> {
    let shell = IntegrationShell::for_path(&profile.path)?;
    let dir = std::env::temp_dir().join(format!(
        "agentcowork-terminal-{}-{pty_id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let script = dir.join(shell.file_name());
    std::fs::write(&script, shell.script()).ok()?;
    let script_path = script.to_string_lossy().to_string();

    let mut extra_args: Vec<String> = Vec::new();
    let mut extra_env: Vec<(String, String)> = Vec::new();

    let arg_present = |a: &str| profile.args.iter().any(|v| v == a);
    let env_present = |k: &str| {
        profile
            .env
            .as_ref()
            .map(|e| e.contains_key(k))
            .unwrap_or(false)
    };

    match shell {
        IntegrationShell::Bash => {
            // Measured on bash 5, not assumed:
            //
            // * `--rcfile <file>` (and its alias `--init-file`) work for an
            //   interactive bash, and the file may source `~/.bashrc`.
            // * `-i` **combined with either long option aborts bash** with
            //   `--: invalid option`. So a profile that already passes `-i` is
            //   left alone rather than turned into a shell that cannot start.
            // * The user's `~/.bashrc` can take seconds to source
            //   (bash-completion, lesspipe). Nothing here may assume a fixed
            //   startup delay — the reader's first integration frame is the
            //   only honest readiness signal.
            if arg_present("--rcfile") || arg_present("--init-file") || arg_present("-i") {
                return None;
            }
            let wrapper = dir.join("wrapper.bash");
            // `--rcfile` *replaces* ~/.bashrc, so the wrapper restores it
            // first — aliases, prompt and PATH stay exactly as the user's.
            let body = format!(
                "# AgentCowork terminal wrapper — sources the user's rc, then integration.\n\
                 if [ -f \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\n\
                 . \"{script_path}\"\n"
            );
            std::fs::write(&wrapper, body).ok()?;
            extra_args.push("--rcfile".into());
            extra_args.push(wrapper.to_string_lossy().to_string());
        }
        IntegrationShell::Zsh => {
            if env_present("ZDOTDIR") {
                return None;
            }
            // zsh reads `$ZDOTDIR/.zshrc`; ours sources the real one first.
            let zshrc = dir.join(".zshrc");
            let body = format!(
                "# AgentCowork terminal wrapper — sources the user's rc, then integration.\n\
                 if [ -f \"$HOME/.zshrc\" ]; then . \"$HOME/.zshrc\"; fi\n\
                 . \"{script_path}\"\n"
            );
            std::fs::write(&zshrc, body).ok()?;
            extra_env.push(("ZDOTDIR".into(), dir.to_string_lossy().to_string()));
        }
        IntegrationShell::Fish => {
            if arg_present("--init-command") || arg_present("-C") {
                return None;
            }
            extra_args.push("--init-command".into());
            extra_args.push(format!("source \"{script_path}\""));
        }
        IntegrationShell::Pwsh => {
            if arg_present("-Command") {
                return None;
            }
            // pwsh loads its own profile first; `-Command` runs after, so the
            // user's $PROFILE is preserved.
            extra_args.push("-NoExit".into());
            extra_args.push("-Command".into());
            extra_args.push(format!(". \"{script_path}\""));
        }
    }

    Some(IntegrationSetup {
        shell,
        nonce: new_nonce(),
        dir,
        extra_args,
        extra_env,
    })
}

impl PtyHost {
    /// P54.5 — resolve the automation profile (config name → detected
    /// profile → exe + args). POSIX-friendly by config intent; consumers
    /// (tasks / agent `script.run`) adopt this instead of hardcoding sh/cmd.
    pub fn resolve_automation_command(cfg: &TerminalConfig) -> Option<(String, Vec<String>)> {
        let name = cfg.automation_profile_name()?;
        let found = detect_available_profiles(cfg);
        let p = match found.iter().find(|d| d.profile_name == name) {
            Some(p) => p.clone(),
            None => found.into_iter().next()?,
        };
        Some((p.path, p.args))
    }
}

// ---------------------------------------------------------------------------
// Tests — pure detection + a real PTY round-trip
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Test setup builds a `*::default()` fixture and then sets the two or three
    // fields the case actually cares about; spelling each as a struct literal
    // with `..Default::default()` would be longer and harder to read than the
    // assignments.
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    fn win_env() -> DetectEnv {
        let mut env = HashMap::new();
        env.insert("windir".to_string(), r"C:\Windows".to_string());
        env.insert("ProgramFiles".to_string(), r"C:\Program Files".to_string());
        env.insert("HOMEDRIVE".to_string(), "C:".to_string());
        env.insert("UserProfile".to_string(), r"C:\Users\u".to_string());
        DetectEnv {
            env,
            platform: Platform::Windows,
            ..DetectEnv::default()
        }
    }

    /// Does `hay` contain `needle`? (test helper; no `windows(0)` panic)
    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    // --- P68.8 replay ring ---------------------------------------------------

    #[test]
    fn ring_keeps_the_newest_bytes_and_advances_the_cursor() {
        let mut ring = TerminalRing::new(8);
        assert_eq!(ring.push(b"abc"), 3);
        assert_eq!(ring.push(b"defgh"), 8);
        let (seq, bytes) = ring.snapshot();
        assert_eq!(seq, 8);
        assert_eq!(bytes, b"abcdefgh".to_vec());
        // Overflow: oldest bytes are evicted, the cursor keeps counting.
        assert_eq!(ring.push(b"XY"), 10);
        let (seq, bytes) = ring.snapshot();
        assert_eq!(seq, 10);
        assert_eq!(bytes, b"cdefghXY".to_vec());
        assert_eq!(ring.discarded(), 2);
        // A cursor still inside the window gets only the unseen suffix.
        let (seq, bytes, dropped) = ring.since(8);
        assert_eq!(seq, 10);
        assert_eq!(bytes, b"XY".to_vec());
        assert_eq!(dropped, 0);
        // A cursor at the head gets nothing, and that is not a gap.
        let (_, bytes, dropped) = ring.since(10);
        assert!(bytes.is_empty());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn replay_reports_an_evicted_cursor_as_a_gap() {
        let mut ring = TerminalRing::new(16);
        ring.push(&[b'a'; 32]);
        let (seq, bytes, dropped) = ring.since(0);
        assert_eq!(seq, 32);
        assert_eq!(bytes.len(), 16);
        assert_eq!(
            dropped, 16,
            "an evicted cursor is a reported gap, never silence"
        );
    }

    #[test]
    fn ring_capacity_zero_disables_retention_but_keeps_the_cursor() {
        let mut ring = TerminalRing::new(0);
        assert_eq!(ring.push(b"abc"), 3);
        assert!(ring.snapshot().1.is_empty());
        assert_eq!(ring.discarded(), 3);
    }

    #[test]
    fn replay_on_an_unknown_pty_fails_closed() {
        let host = PtyHost::new();
        assert!(host.replay("pty-nope", None).is_err());
    }

    #[test]
    fn scrollback_capacity_is_clamped_to_the_supported_range() {
        let host = PtyHost::new();
        assert_eq!(host.scrollback_bytes(), DEFAULT_SCROLLBACK_BYTES);
        host.set_scrollback_bytes(0);
        assert_eq!(host.scrollback_bytes(), MIN_SCROLLBACK_BYTES);
        host.set_scrollback_bytes(usize::MAX);
        assert_eq!(host.scrollback_bytes(), MAX_SCROLLBACK_BYTES);
    }

    /// The P68.8 claim itself, end to end: bytes produced while **no view is
    /// listening** are still recoverable. Drain the reader with a no-op
    /// consumer (exactly what a closed view looks like), run a command, and
    /// assert `replay()` returns its output.
    ///
    /// Skipped (not failed) when there is no bash on this host.
    #[test]
    fn ring_replays_output_produced_with_no_listener_attached() {
        let bash = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .iter()
            .find(|p| Path::new(p).exists());
        let Some(bash) = bash else {
            eprintln!("skipping: no bash on this host");
            return;
        };
        let profile = DetectedProfile {
            profile_name: "bash".into(),
            path: (*bash).into(),
            is_unsafe_path: false,
            is_from_path: false,
            is_auto_detected: true,
            is_default: true,
            args: Vec::new(),
            env: None,
            icon: None,
            backend: TerminalBackend::Local,
            source: None,
            wsl_distro: None,
            cwd: None,
        };
        let host = PtyHost::new();
        let (pty_id, output) = host
            .spawn_with(
                &profile,
                None,
                24,
                80,
                SpawnOpts {
                    origin: TerminalOrigin::Agent,
                    integration: false,
                },
            )
            .expect("spawn into a real pty");

        // A view with no listener: the reader still drains the PTY (or the
        // child would block on a full buffer) but nothing is rendered.
        output.stream(|_frame| {}).expect("stream");
        host.write(&pty_id, b"echo agentcowork-ring-ok\r")
            .expect("write to pty master");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = false;
        while std::time::Instant::now() < deadline {
            if let Ok(r) = host.replay(&pty_id, None) {
                if r.seq > 0 && contains(&r.bytes, b"agentcowork-ring-ok") {
                    assert_eq!(r.dropped, 0, "a full snapshot has no gap");
                    assert!(r.capacity >= MIN_SCROLLBACK_BYTES);
                    seen = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            seen,
            "replay() must return output produced with no listener attached"
        );
        host.kill(&pty_id).expect("kill");
    }

    #[test]
    fn default_config_roundtrips_terminal_section() {
        let cfg = TerminalConfig {
            use_wsl_profiles: true,
            ..Default::default()
        };
        let text = toml::to_string(&cfg).unwrap();
        let back: TerminalConfig = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn config_authoring_shapes_delete_marker_and_platform_keys() {
        // `TerminalConfig` *is* the `[terminal]` section, so its keys sit at
        // the root of the parsed document.
        let toml_text = r#"
[profiles.linux]
"bash (custom)" = { path = "/opt/bash", args = ["--login"] }
"zsh" = { path = "" } # delete marker (empty path + no source)
"cygwin style" = { path = { path = "C:\\cygwin64\\bin\\bash.exe", isUnsafe = true } }
"multi" = { path = ["/usr/bin/a", { path = "/usr/bin/b", isUnsafe = true }] }

[defaultProfile]
linux = "bash (custom)"

[automationProfile]
linux = "sh"
"#;
        let cfg: TerminalConfig = toml::from_str(toml_text).unwrap();
        let pm = cfg.profiles_for(Platform::Linux).unwrap();

        // Bare string path + args.
        assert_eq!(pm["bash (custom)"].path[0].path, "/opt/bash");
        assert_eq!(pm["bash (custom)"].args, vec!["--login"]);
        // Empty string reads as the delete marker, not a one-entry path.
        assert!(pm["zsh"].path.is_empty());
        // Unsafe entry object shape.
        assert!(pm["cygwin style"].path[0].is_unsafe);
        // Mixed array keeps order and per-entry flags.
        assert_eq!(pm["multi"].path.len(), 2);
        assert!(!pm["multi"].path[0].is_unsafe);
        assert!(pm["multi"].path[1].is_unsafe);
        // defaultProfile / automationProfile are plain strings per platform.
        assert_eq!(
            cfg.default_profile_name_for(Platform::Linux),
            Some("bash (custom)")
        );
        assert_eq!(cfg.automation_profile_name_for(Platform::Linux), Some("sh"));

        // Round-trip: writing the config back must stay parseable.
        let back: TerminalConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn empty_path_config_deletes_detected_profile_via_toml() {
        let mut env = DetectEnv::default();
        env.etc_shells = Some("/bin/bash\n/usr/bin/zsh\n".into());
        env.exists_files = vec!["/bin/bash".into(), "/usr/bin/zsh".into()];
        let cfg: TerminalConfig = toml::from_str(
            r#"
[profiles.linux]
zsh = { path = "" }
"#,
        )
        .unwrap();
        let names: Vec<String> = detect_available_profiles_in(&cfg, &env, None)
            .into_iter()
            .map(|d| d.profile_name)
            .collect();
        assert!(names.contains(&"bash".to_string()));
        assert!(
            !names.contains(&"zsh".to_string()),
            "toml delete marker removed zsh: {names:?}"
        );
    }

    #[test]
    fn automation_profile_falls_back_to_default() {
        let cfg = TerminalConfig {
            automation_profile: PlatformMap {
                windows: None,
                linux: Some("sh".to_string()),
                macos: None,
            },
            default_profile: PlatformMap {
                windows: None,
                linux: Some("bash".to_string()),
                macos: None,
            },
            ..Default::default()
        };
        assert_eq!(cfg.automation_profile_name_for(Platform::Linux), Some("sh"));
        let cfg2 = TerminalConfig {
            default_profile: cfg.default_profile.clone(),
            ..Default::default()
        };
        assert_eq!(
            cfg2.automation_profile_name_for(Platform::Linux),
            Some("bash")
        );
    }

    #[test]
    fn abi_version_contract() {
        // Host vN serves 1..=N — clients advertise N; a client claiming 2
        // against host 1 must refuse. Backend exposes the host ABI.
        assert_eq!(TerminalBackend::HOST_ABI_VERSION, 1);
        assert_eq!(TerminalBackend::Local.abi_version(), 1);
    }

    #[test]
    fn unix_detection_uses_shell_then_etc_shells_with_dedupe() {
        let mut env = DetectEnv::default();
        env.env_unix
            .insert("SHELL".to_string(), "/bin/bash".to_string());
        env.etc_shells = Some("/bin/sh\n/bin/bash\n/usr/bin/zsh\n/bin/bash\n#comment\n".into());
        env.exists_files = vec!["/bin/bash".into(), "/bin/sh".into(), "/usr/bin/zsh".into()];
        let cfg = TerminalConfig::default();
        let found = detect_available_profiles_in(&cfg, &env, None);
        let names: Vec<&str> = found.iter().map(|d| d.profile_name.as_str()).collect();
        // $SHELL == /bin/bash == first /etc/shells bash → single "bash", dup gets (2)
        assert!(names.contains(&"bash"), "names={names:?}");
        assert!(names.contains(&"sh"));
        assert!(names.contains(&"zsh"));
        assert!(names.contains(&"bash (2)"), "deduped duplicate: {names:?}");
    }

    #[test]
    fn unix_detection_marks_missing_shells_absent() {
        let mut env = DetectEnv::default();
        env.etc_shells = Some("/bin/bash\n/usr/bin/nonexistent-shell\n".into());
        env.exists_files = vec!["/bin/bash".into()];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let names: Vec<&str> = found.iter().map(|d| d.profile_name.as_str()).collect();
        assert_eq!(names, vec!["bash"]);
    }

    #[test]
    fn config_override_deletes_detected_profile() {
        let mut env = DetectEnv::default();
        env.etc_shells = Some("/bin/bash\n/usr/bin/fish\n".into());
        env.exists_files = vec!["/bin/bash".into(), "/usr/bin/fish".into()];
        let cfg = TerminalConfig {
            profiles: PlatformMap {
                linux: Some(HashMap::from([(
                    "fish".to_string(),
                    TerminalProfile {
                        path: vec![],
                        ..TerminalProfile::default()
                    },
                )])),
                windows: None,
                macos: None,
            },
            ..Default::default()
        };
        let found = detect_available_profiles_in(&cfg, &env, None);
        let names: Vec<&str> = found.iter().map(|d| d.profile_name.as_str()).collect();
        assert!(
            !names.contains(&"fish"),
            "null-delete removed fish: {names:?}"
        );
        assert!(names.contains(&"bash"));
    }

    #[test]
    fn config_override_replaces_detected_profile_by_name() {
        let mut env = DetectEnv::default();
        env.etc_shells = Some("/bin/bash\n".into());
        env.exists_files = vec!["/bin/bash".into(), "/opt/mybash".into()];
        let cfg = TerminalConfig {
            profiles: PlatformMap {
                linux: Some(HashMap::from([(
                    "bash".to_string(),
                    TerminalProfile {
                        path: vec!["/opt/mybash".to_string().map_string()],
                        ..TerminalProfile::default()
                    },
                )])),
                windows: None,
                macos: None,
            },
            ..Default::default()
        };
        let found = detect_available_profiles_in(&cfg, &env, None);
        let bash = found.iter().find(|d| d.profile_name == "bash").unwrap();
        assert_eq!(bash.path, "/opt/mybash");
        assert!(!bash.is_from_path);
    }

    #[test]
    fn bare_name_resolves_on_path_and_flags_is_from_path() {
        let mut env = DetectEnv::default();
        env.etc_shells = Some("/bin/fishish\n".into()); // not existing file
        env.on_path
            .insert("fish".to_string(), "/usr/local/bin/fish".to_string());
        let cfg = TerminalConfig {
            profiles: PlatformMap {
                linux: Some(HashMap::from([(
                    "fish".to_string(),
                    TerminalProfile {
                        path: vec!["fish".to_string().map_string()],
                        ..TerminalProfile::default()
                    },
                )])),
                windows: None,
                macos: None,
            },
            ..Default::default()
        };
        let found = detect_available_profiles_in(&cfg, &env, None);
        let fish = found.iter().find(|d| d.profile_name == "fish").unwrap();
        assert_eq!(fish.path, "/usr/local/bin/fish");
        assert!(fish.is_from_path);
    }

    #[test]
    fn wsl_profiles_skipped_below_build_19041_and_docker_desktop_filtered() {
        let mut env = win_env();
        env.wsl_distros = vec![
            "Ubuntu-22.04".into(),
            "docker-desktop".into(),
            "docker-desktop-data".into(),
            "Debian".into(),
        ];
        env.exists_files = vec![r"C:\Windows\System32\wsl.exe".into()];
        let cfg = TerminalConfig {
            use_wsl_profiles: true,
            ..Default::default()
        };
        // Build 19040: WSL discovery off (WSL2 `-d` flag needs ≥ 19041).
        let found = detect_available_profiles_in(&cfg, &env, Some(19040));
        assert!(found.iter().all(|d| d.backend != TerminalBackend::Wsl));
        // Build 19041: docker-desktop* filtered, real distros present.
        let found = detect_available_profiles_in(&cfg, &env, Some(19041));
        let wsl: Vec<&str> = found
            .iter()
            .filter(|d| d.backend == TerminalBackend::Wsl)
            .map(|d| d.profile_name.as_str())
            .collect();
        assert!(wsl.contains(&"Ubuntu-22.04 (WSL)"));
        assert!(wsl.contains(&"Debian (WSL)"));
        assert!(!wsl.iter().any(|n| n.contains("docker")));
    }

    #[test]
    fn cygwin_msys2_marked_unsafe_until_confirmed() {
        let mut env = win_env();
        env.exists_files = vec![
            r"C:\cygwin64\bin\bash.exe".into(),
            r"C:\msys64\usr\bin\bash.exe".into(),
        ];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let cyg = found.iter().find(|d| d.profile_name == "Cygwin").unwrap();
        assert!(cyg.is_unsafe_path);
        let msys = found
            .iter()
            .find(|d| d.profile_name == "bash (MSYS2)")
            .unwrap();
        assert!(msys.is_unsafe_path);
        // P54.6 — confirmed list unblocks offering.
        assert!(!cyg.offered(&[], true));
        assert!(cyg.offered(&["Cygwin".to_string()], true));
        // Default (no hide flag) still returns them flagged for the UI.
        assert!(cyg.offered(&[], false));
    }

    #[test]
    fn git_bash_source_finds_via_git_exe_on_path() {
        let mut env = win_env();
        env.on_path.insert(
            "git.exe".to_string(),
            r"C:\Program Files\Git\cmd\git.exe".to_string(),
        );
        env.exists_files = vec![r"C:\Program Files\Git\bin\bash.exe".into()];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let gb = found.iter().find(|d| d.profile_name == "Git Bash").unwrap();
        assert_eq!(gb.path, r"C:\Program Files\Git\bin\bash.exe");
        assert_eq!(gb.args, vec!["--login", "-i"]);
    }

    #[test]
    fn pwsh_highest_version_dir_wins() {
        let mut env = win_env();
        env.exists_dirs = vec![r"C:\Program Files\PowerShell".into()];
        env.pwsh_install_dirs = vec!["6".into(), "7".into(), "not-a-version".into()];
        env.exists_files = vec![
            r"C:\Program Files\PowerShell\6\pwsh.exe".into(),
            r"C:\Program Files\PowerShell\7\pwsh.exe".into(),
        ];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let ps = found
            .iter()
            .find(|d| d.profile_name == "PowerShell")
            .unwrap();
        assert_eq!(ps.path, r"C:\Program Files\PowerShell\7\pwsh.exe");
    }

    #[test]
    fn pwsh_msix_and_scoop_fallbacks() {
        let mut env = win_env();
        env.env.insert(
            "LocalAppData".to_string(),
            r"C:\Users\u\AppData\Local".to_string(),
        );
        env.exists_dirs = vec![r"C:\Users\u\AppData\Local\Microsoft\WindowsApps".into()];
        env.msix_app_dirs = vec![
            "Microsoft.PowerShell_8wekyb3d8bbwe".into(),
            "Other.App_xyz".into(),
        ];
        env.exists_files = vec![
            r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\Microsoft.PowerShell_8wekyb3d8bbwe\pwsh.exe".into(),
        ];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let ps = found
            .iter()
            .find(|d| d.profile_name == "PowerShell")
            .unwrap();
        assert!(ps.path.contains(r"WindowsApps\Microsoft.PowerShell_"));
    }

    #[test]
    fn sysnative_switch_for_32bit_process_on_64bit_windows() {
        let mut env = win_env();
        env.env
            .insert("PROCESSOR_ARCHITEW6432".to_string(), "AMD64".to_string());
        env.exists_files = vec![r"C:\Windows\Sysnative\cmd.exe".into()];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let cmd = found
            .iter()
            .find(|d| d.profile_name == "Command Prompt")
            .unwrap();
        assert_eq!(cmd.path, r"C:\Windows\Sysnative\cmd.exe");
    }

    #[test]
    fn cmder_requires_cmdroot_init_script() {
        let mut env = win_env();
        // No CMDER_ROOT → default path, no init file → not offered.
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        assert!(found.iter().all(|d| d.profile_name != "Cmder"));
        // With CMDER_ROOT + init file → offered and safe.
        env.env
            .insert("CMDER_ROOT".to_string(), r"C:\tools\cmder".to_string());
        env.exists_files
            .push(r"C:\tools\cmder\vendor\bin\vscode_init.cmd".into());
        env.exists_files.push(r"C:\Windows\System32\cmd.exe".into());
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let cmder = found.iter().find(|d| d.profile_name == "Cmder").unwrap();
        assert!(!cmder.is_unsafe_path);
        assert_eq!(cmder.path, r"C:\Windows\System32\cmd.exe");
    }

    #[test]
    fn windows_detection_finds_cmd_and_windows_powershell() {
        let env = win_env();
        let mut env = env;
        env.exists_files = vec![
            r"C:\Windows\System32\cmd.exe".into(),
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
        ];
        let found = detect_available_profiles_in(&TerminalConfig::default(), &env, None);
        let names: Vec<&str> = found.iter().map(|d| d.profile_name.as_str()).collect();
        assert!(names.contains(&"Command Prompt"));
        assert!(names.contains(&"Windows PowerShell"));
    }

    // --- P54.5 plane read models (real PTY) ---

    /// The observer over a **real** `PtyHost`, not a stub: the arm's wire shape
    /// is asserted in `chat::tests`, so what matters here is that the read
    /// models describe a live session truthfully — including the degenerate
    /// cases that must stay distinguishable.
    #[test]
    fn plane_observer_reads_a_real_session() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let profile = DetectedProfile {
            profile_name: "observer-shell".into(),
            path: shell,
            is_unsafe_path: false,
            is_from_path: false,
            is_auto_detected: true,
            is_default: false,
            args: Vec::new(),
            env: Some(HashMap::from([("PS1".to_string(), "$ ".to_string())])),
            icon: None,
            backend: TerminalBackend::Local,
            source: None,
            wsl_distro: None,
            cwd: None,
        };
        let host = PtyHost::new();
        // Integration off: this test is about the read model, and it must hold
        // for a session whose shell cannot report anything structurally.
        let (pty_id, output) = host
            .spawn_with(
                &profile,
                None,
                24,
                80,
                SpawnOpts {
                    origin: TerminalOrigin::Agent,
                    integration: false,
                },
            )
            .expect("spawn into real pty");
        output.stream(|_| {}).expect("reader drains");

        let status = host.plane_status();
        assert!(status.attached, "a host with a pty is attached");
        assert_eq!(status.count, 1);
        let row = &status.ptys[0];
        assert_eq!(row.pty_id, pty_id);
        assert_eq!(row.profile_id, "observer-shell");
        // Provenance must survive the read: this is what makes an agent tab
        // renderable as read-only rather than as the user's own shell.
        assert_eq!(row.origin, TerminalOrigin::Agent);
        assert!(row.running, "a freshly spawned session is running");
        assert_eq!(row.integration, None, "integration was forced off");

        assert!(
            host.session(&pty_id).is_some(),
            "the session is addressable by id"
        );
        assert!(host.session("pty-nope").is_none());

        // The one case that must never be confused with the other: a session
        // with integration off reports *no* command records, and the last
        // command is an absence of evidence (`None`) — not an empty string a
        // caller could render as "the command succeeded quietly".
        let records = host.commands(&pty_id, 50).expect("commands read");
        assert!(records.is_empty(), "no integration ⇒ no records");
        assert_eq!(host.last_command(&pty_id, 6000).expect("read"), None);
        assert_eq!(host.history(&pty_id, 10, 4000).expect("read"), None);

        // An unknown session is a caller bug, not an empty result.
        assert!(host.commands("pty-nope", 50).is_err());
        assert!(host.last_command("pty-nope", 6000).is_err());

        host.kill_all();
    }

    /// A host with no sessions still reports `attached: true` with zero rows.
    /// The *detached* posture is a property of the host, not of an empty list —
    /// collapsing the two is exactly the misreading `attached` exists to stop.
    #[test]
    fn plane_observer_distinguishes_empty_from_detached() {
        let host = PtyHost::new();
        let status = host.plane_status();
        assert!(status.attached);
        assert_eq!(status.count, 0);
        assert!(status.ptys.is_empty());

        let detached = detached_plane_status();
        assert!(!detached.attached);
        assert_eq!(detached.count, 0);
    }

    // --- real PTY round-trip (P54.3 evidence) ---

    #[test]
    fn pty_roundtrip_echo_and_resize_and_exit() {
        // Use the detected default shell; fall back to sh on unix.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let profile = DetectedProfile {
            profile_name: "test-shell".into(),
            path: shell,
            is_unsafe_path: false,
            is_from_path: false,
            is_auto_detected: true,
            is_default: false,
            args: Vec::new(),
            env: Some(HashMap::from([
                ("PS1".to_string(), "$ ".to_string()),
                ("TERM".to_string(), "xterm-256color".to_string()),
            ])),
            icon: None,
            backend: TerminalBackend::Local,
            source: None,
            wsl_distro: None,
            cwd: None,
        };
        let host = PtyHost::new();
        // Integration off: this test is about raw PTY duplex semantics, and a
        // piped non-interactive shell has no prompt to hook.
        let (pty_id, output) = host
            .spawn_with(
                &profile,
                None,
                24,
                80,
                SpawnOpts {
                    origin: TerminalOrigin::Human,
                    integration: false,
                },
            )
            .expect("spawn into real pty");
        assert!(pty_id.starts_with("pty-"));

        // Resize must not error.
        host.resize(&pty_id, 30, 100).expect("resize");

        // Write `echo agentcowork-pty-ok` + newline; expect the echo back in
        // the raw stream (PTY echo) — proves duplex + real pty semantics.
        host.write(&pty_id, b"echo agentcowork-pty-ok\r")
            .expect("write to pty master");

        let mut all: Vec<u8> = Vec::new();
        let collected = std::sync::Arc::new(Mutex::new(all.clone()));
        let collected2 = collected.clone();
        output
            .stream(move |frame| {
                if let PtyFrame::Data(chunk) = frame {
                    let mut c = collected2.lock().unwrap();
                    c.extend_from_slice(&chunk);
                }
            })
            .expect("stream");

        // Read up to ~3s for the echo.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut saw = false;
        while std::time::Instant::now() < deadline {
            {
                let c = collected.lock().unwrap();
                if contains(&c, b"agentcowork-pty-ok") {
                    saw = true;
                }
            }
            if saw {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        all = collected.lock().unwrap().clone();
        assert!(
            saw,
            "expected echo in raw pty stream, got: {:?}",
            String::from_utf8_lossy(&all)
        );

        // Exit + reap.
        host.kill(&pty_id).expect("kill");
        std::thread::sleep(std::time::Duration::from_millis(200));
        // After kill the pty is removed.
        assert!(host.exit_code(&pty_id).is_err());
    }

    /// P67 — the real end-to-end proof that the injected bash script speaks the
    /// protocol we parse: spawn an interactive bash through the PTY host with
    /// integration on, run a command that fails, and assert the host captured
    /// the command line, its cwd, its exit code and its output.
    ///
    /// Skipped (not failed) when there is no bash on this host — the parser
    /// itself is covered by `shell_integration` unit tests either way.
    #[test]
    fn integration_captures_command_cwd_exit_and_output_from_real_bash() {
        let bash = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .iter()
            .find(|p| Path::new(p).exists());
        let Some(bash) = bash else {
            eprintln!("no bash on this host — skipping live shell-integration test");
            return;
        };
        let profile = DetectedProfile {
            profile_name: "integration-bash".into(),
            path: (*bash).to_string(),
            is_unsafe_path: false,
            is_from_path: false,
            is_auto_detected: true,
            is_default: false,
            args: vec![],
            env: Some(HashMap::from([
                ("PS1".to_string(), "EAIOS$ ".to_string()),
                ("TERM".to_string(), "xterm-256color".to_string()),
                // A scratch history file: the shell needs history to report
                // the accepted line, and a test must not append to the
                // developer's real ~/.bash_history.
                (
                    "HISTFILE".to_string(),
                    std::env::temp_dir()
                        .join("agentcowork-integration-test-history")
                        .to_string_lossy()
                        .to_string(),
                ),
            ])),
            icon: None,
            backend: TerminalBackend::Local,
            source: None,
            wsl_distro: None,
            cwd: Some("/tmp".to_string()),
        };
        let host = PtyHost::new();
        let (pty_id, output) = host
            .spawn_with(
                &profile,
                Some("/tmp"),
                24,
                90,
                SpawnOpts {
                    origin: TerminalOrigin::Agent,
                    integration: true,
                },
            )
            .expect("spawn integrated bash");

        let frames = std::sync::Arc::new(Mutex::new(Vec::<PtyFrame>::new()));
        let sink = frames.clone();
        output
            .stream(move |f| {
                sink.lock().unwrap().push(f);
            })
            .expect("stream");

        // Wait for readiness, not for a wall-clock guess: the integration
        // script emits its first `Cwd` only after the user's own rc file has
        // finished sourcing. That is genuinely slow on real machines — 11s on
        // the box this was developed on (bash-completion + lesspipe + tool
        // PATH shims) — so the budget is generous on purpose. A fixed sleep
        // here is exactly the flake this test exists to catch, and it is also
        // why the Shell view must render a "starting…" state rather than
        // assuming a prompt appears promptly.
        let ready = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut live = false;
        while std::time::Instant::now() < ready {
            if frames
                .lock()
                .unwrap()
                .iter()
                .any(|f| matches!(f, PtyFrame::Cwd(_)))
            {
                live = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            live,
            "shell integration never armed — no cwd frame within 20s"
        );
        // Give the prompt a moment to finish drawing, then run a command.
        std::thread::sleep(std::time::Duration::from_millis(300));
        host.write(&pty_id, b"echo agentcowork-int-ok; false\r")
            .expect("write");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut record: Option<CommandRecord> = None;
        while std::time::Instant::now() < deadline {
            {
                let f = frames.lock().unwrap();
                for frame in f.iter() {
                    if let PtyFrame::Command(rec) = frame {
                        if rec.command.contains("agentcowork-int-ok") {
                            record = Some((**rec).clone());
                        }
                    }
                }
            }
            if record.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let cmd = record.unwrap_or_else(|| {
            let f = frames.lock().unwrap();
            let rendered: String = f
                .iter()
                .filter_map(|fr| match fr {
                    PtyFrame::Data(b) => Some(String::from_utf8_lossy(b).to_string()),
                    _ => None,
                })
                .collect();
            let kinds: Vec<String> = f
                .iter()
                .map(|fr| match fr {
                    PtyFrame::Data(_) => "data".to_string(),
                    PtyFrame::Command(c) => format!("command({:?})", c.command),
                    PtyFrame::Cwd(p) => format!("cwd({p})"),
                    PtyFrame::Exit(c) => format!("exit({c:?})"),
                })
                .collect();
            let live = host
                .sessions()
                .iter()
                .map(|s| format!("{} origin={:?} integration={:?}", s.pty_id, s.origin, s.integration))
                .collect::<Vec<_>>()
                .join(", ");
            panic!(
                "no command captured.\n  rendered: {rendered:?}\n  frames: {kinds:?}\n  sessions: [{live}]"
            )
        });
        assert!(
            cmd.command.contains("echo agentcowork-int-ok"),
            "command line was {:?}",
            cmd.command
        );
        assert!(
            cmd.trusted,
            "the nonce-bearing command line must be trusted"
        );
        assert_eq!(cmd.exit_code, Some(1), "exit code of `false`");
        assert_eq!(cmd.cwd, "/tmp", "cwd reported by the shell");
        assert!(
            cmd.output.contains("agentcowork-int-ok"),
            "output was {:?}",
            cmd.output
        );

        // The tracker stored on the session must agree with the frames.
        let tracker = host.tracker(&pty_id).expect("tracker for live pty");
        let last = tracker.lock().unwrap().last().cloned();
        assert_eq!(last.map(|r| r.command), Some(cmd.command.clone()));

        // Provenance is recorded on the session.
        let sessions = host.sessions();
        let s = sessions.iter().find(|s| s.pty_id == pty_id).unwrap();
        assert_eq!(s.origin, TerminalOrigin::Agent);
        assert_eq!(s.integration, Some("Rich"));

        host.kill(&pty_id).expect("kill");
    }

    /// Unambiguous host check: `cat` has no shell semantics, so the only way
    /// its output can appear is if the child is really running with the pty
    /// slave as stdio. A shell round-trip cannot prove this — a tty echoes
    /// input even when nothing is reading it.
    #[test]
    fn pty_child_actually_runs_with_the_slave_as_stdio() {
        let cat = ["/bin/cat", "/usr/bin/cat"]
            .iter()
            .find(|p| Path::new(p).exists());
        let Some(cat) = cat else {
            eprintln!("no cat on this host");
            return;
        };
        let profile = DetectedProfile {
            profile_name: "cat".into(),
            path: (*cat).to_string(),
            is_unsafe_path: false,
            is_from_path: false,
            is_auto_detected: true,
            is_default: false,
            args: vec![],
            env: None,
            icon: None,
            backend: TerminalBackend::Local,
            source: None,
            wsl_distro: None,
            cwd: None,
        };
        let host = PtyHost::new();
        let (pty_id, output) = host
            .spawn_with(
                &profile,
                None,
                24,
                80,
                SpawnOpts {
                    origin: TerminalOrigin::Human,
                    integration: false,
                },
            )
            .expect("spawn cat");

        let seen = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = seen.clone();
        output
            .stream(move |f| {
                if let PtyFrame::Data(b) = f {
                    sink.lock().unwrap().extend_from_slice(&b);
                }
            })
            .expect("stream");

        std::thread::sleep(std::time::Duration::from_millis(250));
        host.write(&pty_id, b"pty-child-alive-42\n").expect("write");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut ok = false;
        while std::time::Instant::now() < deadline {
            if contains(&seen.lock().unwrap(), b"pty-child-alive-42") {
                ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            ok,
            "the child never echoed our line back — its stdio is not the pty \
             slave. saw: {:?}",
            String::from_utf8_lossy(&seen.lock().unwrap())
        );
        host.kill(&pty_id).expect("kill");
    }

    #[test]
    fn setup_integration_declines_shells_it_cannot_integrate() {
        let base = DetectedProfile {
            profile_name: "x".into(),
            path: "/bin/sh".into(),
            is_unsafe_path: false,
            is_from_path: false,
            is_auto_detected: true,
            is_default: false,
            args: vec![],
            env: None,
            icon: None,
            backend: TerminalBackend::Local,
            source: None,
            wsl_distro: None,
            cwd: None,
        };
        assert!(setup_integration("pty-test", &base).is_none(), "dash/sh");
        let bash = DetectedProfile {
            path: "/bin/bash".into(),
            ..base.clone()
        };
        let setup = setup_integration("pty-test-bash", &bash).expect("bash integrates");
        assert_eq!(setup.shell, IntegrationShell::Bash);
        assert_eq!(setup.nonce.len(), 32);
        assert!(
            setup.extra_args.contains(&"--rcfile".to_string()),
            "bash must be integrated with --rcfile (--init-file does not work)"
        );
        // The generated wrapper must preserve the user's own rc.
        let wrapper = setup.dir.join("wrapper.bash");
        let body = std::fs::read_to_string(&wrapper).expect("wrapper written");
        assert!(body.contains(".bashrc"), "wrapper must source the user rc");
        assert!(
            body.contains("integration.bash"),
            "wrapper must source ours"
        );
        let _ = std::fs::remove_dir_all(&setup.dir);

        // Anything we cannot inject cleanly is declined, never half-applied.
        for args in [
            vec!["--init-file".into(), "/tmp/mine".into()],
            vec!["--rcfile".into(), "/tmp/mine".into()],
            vec!["-i".into()],
        ] {
            let custom = DetectedProfile {
                path: "/bin/bash".into(),
                args,
                ..base.clone()
            };
            assert!(setup_integration("pty-test-custom", &custom).is_none());
        }

        // Nonces must not repeat across sessions.
        let a = setup_integration("pty-nonce-a", &bash).unwrap();
        let b = setup_integration("pty-nonce-b", &bash).unwrap();
        assert_ne!(a.nonce, b.nonce);
        let _ = std::fs::remove_dir_all(&a.dir);
        let _ = std::fs::remove_dir_all(&b.dir);

        // zsh goes through ZDOTDIR, not args.
        let zsh = DetectedProfile {
            path: "/usr/bin/zsh".into(),
            ..base.clone()
        };
        if let Some(s) = setup_integration("pty-test-zsh", &zsh) {
            assert!(s.extra_env.iter().any(|(k, _)| k == "ZDOTDIR"));
            let _ = std::fs::remove_dir_all(&s.dir);
        }
    }
}
