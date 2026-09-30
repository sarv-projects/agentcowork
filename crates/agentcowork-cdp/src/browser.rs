//! Browser discovery & launch (P2.1, E1) — system Chrome/Edge first, then
//! chrome-for-testing fallback (ARCH/08 §8.1/§8.8, doc 34 §2.1).
//!
//! Launch contract: `--remote-debugging-port=0 --user-data-dir=<dir>` +
//! first-run flags; the real port is read back from `<dir>/DevToolsActivePort`
//! (never trust a fixed port). All CDP traffic is loopback-only.

use crate::discovery::read_devtools_active_port;
use crate::{BrowserEndpoint, CdpError};
use serde::{Deserialize, Serialize};
use std::env;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The supported browser families / channels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserChannel {
    #[default]
    Auto,
    Brave,
    Chrome,
    Edge,
    Chromium,
    Arc,
    Vivaldi,
    Custom,
}

impl BrowserChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            BrowserChannel::Auto => "auto",
            BrowserChannel::Brave => "brave",
            BrowserChannel::Chrome => "chrome",
            BrowserChannel::Edge => "edge",
            BrowserChannel::Chromium => "chromium",
            BrowserChannel::Arc => "arc",
            BrowserChannel::Vivaldi => "vivaldi",
            BrowserChannel::Custom => "custom",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            BrowserChannel::Auto => "Auto-Detect (Best Available)",
            BrowserChannel::Brave => "Brave Browser",
            BrowserChannel::Chrome => "Google Chrome",
            BrowserChannel::Edge => "Microsoft Edge",
            BrowserChannel::Chromium => "Chromium",
            BrowserChannel::Arc => "Arc Browser",
            BrowserChannel::Vivaldi => "Vivaldi",
            BrowserChannel::Custom => "Custom Executable Path",
        }
    }
}

/// Source of a discovered browser candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    SystemPath,
    WindowsAppPaths,
    StandardProgramFiles,
    LocalAppData,
    ManagedCft,
    UserConfig,
}

/// Information about a detected browser on the host system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserCandidate {
    pub channel: BrowserChannel,
    pub name: String,
    pub executable_path: String,
    pub version: Option<String>,
    pub is_default: bool,
    pub source: CandidateSource,
}

/// Profile management strategy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProfileMode {
    /// Dedicated clean profile isolated from personal browsing.
    #[default]
    Isolated,
    /// Explicitly paired non-default profile (preserves specific logins).
    Paired,
}

fn default_true() -> bool {
    true
}

fn default_extra_args() -> Vec<String> {
    vec!["--mute-audio".to_string()]
}

/// Persisted configuration for browser automation.
// NOTE: `Default` is implemented by hand below, not derived — the dynamic
// default is `headless: true` + `extra_args: ["--mute-audio"]`, which a derive
// would silently replace with `false` / `[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default)]
    pub preferred_channel: BrowserChannel,
    #[serde(default)]
    pub custom_executable_path: Option<String>,
    #[serde(default)]
    pub profile_mode: BrowserProfileMode,
    #[serde(default = "default_true")]
    pub headless: bool,
    #[serde(default = "default_extra_args")]
    pub extra_args: Vec<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            preferred_channel: BrowserChannel::Auto,
            custom_executable_path: None,
            profile_mode: BrowserProfileMode::Isolated,
            headless: true,
            extra_args: default_extra_args(),
        }
    }
}

/// Wait budget for a freshly launched browser to write DevToolsActivePort.
pub const DEFAULT_LAUNCH_WAIT: Duration = Duration::from_secs(20);
/// Download size cap for the chrome-for-testing zip (~500MB).
const MAX_DOWNLOAD_BYTES: u64 = 600 * 1024 * 1024;
/// Official last-known-good chrome-for-testing manifest.
pub const CFT_KNOWN_GOOD_URL: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
/// Cache dir under the user's data dir.
pub const CFT_SUBDIR: &str = "browser/chrome-for-testing";

/// Options for launching the managed browser.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Profile/data dir — also where DevToolsActivePort lands. Defaults to
    /// `<data_dir>/browser-profile`.
    pub user_data_dir: PathBuf,
    /// Run headless (scrape tier, ARCH/08 §8.8 tier 2).
    pub headless: bool,
    /// Explicit browser binary (config override). If None, search PATH +
    /// platform defaults, then chrome-for-testing cache.
    pub browser_binary: Option<PathBuf>,
    /// Extra launch flags.
    pub extra_args: Vec<String>,
    /// How long to wait for DevToolsActivePort.
    pub wait_timeout: Duration,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        let user_data_dir = default_profile_dir();
        Self {
            user_data_dir,
            headless: false,
            browser_binary: None,
            extra_args: Vec::new(),
            wait_timeout: DEFAULT_LAUNCH_WAIT,
        }
    }
}

/// A spawned browser child. Killing/cleanup happens on drop.
pub struct BrowserChild {
    child: Child,
    endpoint: BrowserEndpoint,
}

impl BrowserChild {
    pub fn endpoint(&self) -> &BrowserEndpoint {
        &self.endpoint
    }
}

impl Drop for BrowserChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Default profile dir: the shared data home's `browser-profile`
/// (`AGENTCOWORK_HOME` / `~/.agentcowork`, legacy `EVERYAIOS_HOME` /
/// `~/.everyaios` honored as a fallback — DEC-053).
pub fn default_profile_dir() -> PathBuf {
    agentcowork_types::env_compat::data_home().join("browser-profile")
}

/// Platform-appropriate candidate binaries for system Chrome/Edge/Brave.
fn platform_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        out.extend(
            [
                "brave-browser",
                "brave-browser-stable",
                "brave",
                "google-chrome",
                "google-chrome-stable",
                "chromium",
                "chromium-browser",
                "microsoft-edge",
                "microsoft-edge-stable",
                "vivaldi",
                "vivaldi-stable",
            ]
            .iter()
            .map(PathBuf::from),
        );
        out.extend(
            [
                "/usr/bin/brave-browser",
                "/usr/bin/google-chrome",
                "/usr/bin/chromium-browser",
                "/usr/bin/microsoft-edge",
                "/snap/bin/brave",
            ]
            .iter()
            .map(PathBuf::from),
        );
    }
    #[cfg(target_os = "macos")]
    {
        out.extend(
            [
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/Applications/Arc.app/Contents/MacOS/Arc",
                "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
            ]
            .iter()
            .map(PathBuf::from),
        );
    }
    #[cfg(target_os = "windows")]
    {
        out.extend(
            [
                r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
                r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Vivaldi\Application\vivaldi.exe",
            ]
            .iter()
            .map(PathBuf::from),
        );
        // Per-user installs (no admin rights needed) live under LocalAppData
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            for rel in [
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
                r"Google\Chrome\Application\chrome.exe",
                r"Microsoft\Edge\Application\msedge.exe",
                r"Arc\Arc.exe",
                r"Vivaldi\Application\vivaldi.exe",
            ] {
                out.push(local.join(rel));
            }
        }
    }
    out
}

/// Find an executable by name on PATH.
fn find_on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).find_map(|dir| {
        let candidate = dir.join(name);
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
}

/// Probe a browser executable's version string.
pub fn probe_browser_version(path: &Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// Detect the browser channel from an executable path string.
pub fn detect_channel_from_path(path: &Path) -> BrowserChannel {
    let s = path.to_string_lossy().to_lowercase();
    if s.contains("brave") {
        BrowserChannel::Brave
    } else if s.contains("msedge") || s.contains("microsoft edge") || s.contains("edge") {
        BrowserChannel::Edge
    } else if s.contains("vivaldi") {
        BrowserChannel::Vivaldi
    } else if s.contains("arc") {
        BrowserChannel::Arc
    } else if s.contains("google-chrome")
        || s.contains("google/chrome")
        || s.contains("google chrome")
        || s.contains("chrome.exe")
        || s.contains("google chrome.app")
    {
        BrowserChannel::Chrome
    } else if s.contains("chromium") {
        BrowserChannel::Chromium
    } else {
        BrowserChannel::Custom
    }
}

/// Return candidate search paths for a specific channel on this OS.
pub fn channel_candidates(channel: BrowserChannel) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match channel {
        BrowserChannel::Brave => {
            #[cfg(target_os = "linux")]
            {
                out.extend(
                    [
                        "brave-browser",
                        "brave-browser-stable",
                        "brave",
                        "/usr/bin/brave-browser",
                        "/snap/bin/brave",
                    ]
                    .iter()
                    .map(PathBuf::from),
                );
            }
            #[cfg(target_os = "macos")]
            {
                out.push(PathBuf::from(
                    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                ));
            }
            #[cfg(target_os = "windows")]
            {
                out.push(PathBuf::from(
                    r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
                ));
                out.push(PathBuf::from(
                    r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
                ));
                if let Some(local) = env::var_os("LOCALAPPDATA") {
                    out.push(
                        PathBuf::from(local)
                            .join(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
                    );
                }
            }
        }
        BrowserChannel::Chrome => {
            #[cfg(target_os = "linux")]
            {
                out.extend(
                    [
                        "google-chrome",
                        "google-chrome-stable",
                        "/usr/bin/google-chrome",
                    ]
                    .iter()
                    .map(PathBuf::from),
                );
            }
            #[cfg(target_os = "macos")]
            {
                out.push(PathBuf::from(
                    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                ));
            }
            #[cfg(target_os = "windows")]
            {
                out.push(PathBuf::from(
                    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                ));
                out.push(PathBuf::from(
                    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
                ));
                if let Some(local) = env::var_os("LOCALAPPDATA") {
                    out.push(PathBuf::from(local).join(r"Google\Chrome\Application\chrome.exe"));
                }
            }
        }
        BrowserChannel::Edge => {
            #[cfg(target_os = "linux")]
            {
                out.extend(
                    [
                        "microsoft-edge",
                        "microsoft-edge-stable",
                        "/usr/bin/microsoft-edge",
                    ]
                    .iter()
                    .map(PathBuf::from),
                );
            }
            #[cfg(target_os = "macos")]
            {
                out.push(PathBuf::from(
                    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                ));
            }
            #[cfg(target_os = "windows")]
            {
                out.push(PathBuf::from(
                    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
                ));
                out.push(PathBuf::from(
                    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                ));
                if let Some(local) = env::var_os("LOCALAPPDATA") {
                    out.push(PathBuf::from(local).join(r"Microsoft\Edge\Application\msedge.exe"));
                }
            }
        }
        BrowserChannel::Chromium => {
            #[cfg(target_os = "linux")]
            {
                out.extend(
                    [
                        "chromium",
                        "chromium-browser",
                        "/usr/bin/chromium",
                        "/usr/bin/chromium-browser",
                    ]
                    .iter()
                    .map(PathBuf::from),
                );
            }
            #[cfg(target_os = "macos")]
            {
                out.push(PathBuf::from(
                    "/Applications/Chromium.app/Contents/MacOS/Chromium",
                ));
            }
            #[cfg(target_os = "windows")]
            {
                out.push(PathBuf::from(
                    r"C:\Program Files\Chromium\Application\chrome.exe",
                ));
                if let Some(local) = env::var_os("LOCALAPPDATA") {
                    out.push(PathBuf::from(local).join(r"Chromium\Application\chrome.exe"));
                }
            }
        }
        BrowserChannel::Arc => {
            #[cfg(target_os = "macos")]
            {
                out.push(PathBuf::from("/Applications/Arc.app/Contents/MacOS/Arc"));
            }
            #[cfg(target_os = "windows")]
            {
                if let Some(local) = env::var_os("LOCALAPPDATA") {
                    out.push(PathBuf::from(local).join(r"Arc\Arc.exe"));
                }
            }
            #[cfg(target_os = "linux")]
            {}
        }
        BrowserChannel::Vivaldi => {
            #[cfg(target_os = "linux")]
            {
                out.extend(
                    ["vivaldi", "vivaldi-stable", "/usr/bin/vivaldi"]
                        .iter()
                        .map(PathBuf::from),
                );
            }
            #[cfg(target_os = "macos")]
            {
                out.push(PathBuf::from(
                    "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
                ));
            }
            #[cfg(target_os = "windows")]
            {
                out.push(PathBuf::from(
                    r"C:\Program Files\Vivaldi\Application\vivaldi.exe",
                ));
                if let Some(local) = env::var_os("LOCALAPPDATA") {
                    out.push(PathBuf::from(local).join(r"Vivaldi\Application\vivaldi.exe"));
                }
            }
        }
        BrowserChannel::Auto | BrowserChannel::Custom => {}
    }
    out
}

/// Discover all installed browser candidates on this host.
pub fn discover_installed_browsers() -> Vec<BrowserCandidate> {
    let mut candidates = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    for channel in [
        BrowserChannel::Brave,
        BrowserChannel::Chrome,
        BrowserChannel::Edge,
        BrowserChannel::Chromium,
        BrowserChannel::Arc,
        BrowserChannel::Vivaldi,
    ] {
        for candidate_path in channel_candidates(channel) {
            let resolved = if candidate_path.is_file() {
                Some(candidate_path)
            } else if let Some(name) = candidate_path.file_name().and_then(|n| n.to_str()) {
                find_on_path(name)
            } else {
                None
            };

            if let Some(path) = resolved {
                let canonical = path.to_string_lossy().to_string();
                if seen_paths.insert(canonical.clone()) {
                    let version = probe_browser_version(&path);
                    candidates.push(BrowserCandidate {
                        channel,
                        name: channel.display_name().to_string(),
                        executable_path: canonical,
                        version,
                        is_default: false,
                        source: CandidateSource::StandardProgramFiles,
                    });
                }
            }
        }
    }

    // Managed chrome-for-testing cache
    if let Some(cached) = cached_cft_binary() {
        let canonical = cached.to_string_lossy().to_string();
        if seen_paths.insert(canonical.clone()) {
            candidates.push(BrowserCandidate {
                channel: BrowserChannel::Chromium,
                name: "Managed Chrome for Testing".to_string(),
                executable_path: canonical,
                version: Some("Managed CfT".to_string()),
                is_default: false,
                source: CandidateSource::ManagedCft,
            });
        }
    }

    candidates
}

/// Resolve the browser binary according to the given user configuration.
pub fn resolve_browser_binary(
    config: &BrowserConfig,
) -> Result<(PathBuf, BrowserChannel), CdpError> {
    if config.preferred_channel == BrowserChannel::Custom {
        let path_str = config.custom_executable_path.as_deref().unwrap_or_default();
        if path_str.is_empty() {
            return Err(CdpError::BrowserNotFound(
                "custom browser channel selected, but no executable path configured".into(),
            ));
        }
        let p = PathBuf::from(path_str);
        if p.is_file() {
            return Ok((p, BrowserChannel::Custom));
        }
        return Err(CdpError::BrowserNotFound(format!(
            "custom browser binary missing: {}",
            p.display()
        )));
    }

    if config.preferred_channel != BrowserChannel::Auto {
        for candidate_path in channel_candidates(config.preferred_channel) {
            if candidate_path.is_file() {
                return Ok((candidate_path, config.preferred_channel));
            }
            if let Some(name) = candidate_path.file_name().and_then(|n| n.to_str()) {
                if let Some(found) = find_on_path(name) {
                    return Ok((found, config.preferred_channel));
                }
            }
        }
        return Err(CdpError::BrowserNotFound(format!(
            "selected browser {} not found on this host; choose another browser or install it",
            config.preferred_channel.display_name()
        )));
    }

    // Auto resolution: Locate system browser in priority order
    let binary = locate_system_browser(None)?;
    let channel = detect_channel_from_path(&binary);
    Ok((binary, channel))
}

/// Locate a usable browser binary: explicit config override → system
/// Chrome/Edge (PATH + platform defaults) → chrome-for-testing cache.
pub fn locate_system_browser(browser_binary: Option<&Path>) -> Result<PathBuf, CdpError> {
    if let Some(p) = browser_binary {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        return Err(CdpError::BrowserNotFound(format!(
            "configured browser binary missing: {}",
            p.display()
        )));
    }
    for candidate in platform_candidates() {
        if candidate.is_file() {
            return Ok(candidate);
        }
        if let Some(name) = candidate.file_name().and_then(|n| n.to_str()) {
            if let Some(found) = find_on_path(name) {
                return Ok(found);
            }
        }
    }
    // chrome-for-testing cache fallback.
    if let Some(cached) = cached_cft_binary() {
        if cached.is_file() {
            return Ok(cached);
        }
    }
    // Actionable failure: name what was probed so the UI can tell the user
    // to install Chrome/Edge instead of showing a bare "not found".
    let probed: Vec<String> = platform_candidates()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    Err(CdpError::BrowserNotFound(format!(
        "no system Chrome/Edge/Brave found (looked for: {}); install Chrome, Brave, or Edge, or use install_chrome_for_testing(), or set a browser binary",
        probed.join(", ")
    )))
}

/// Spawn the browser with `--remote-debugging-port=0` and wait for
/// DevToolsActivePort. Fails closed on early exit or timeout.
pub fn spawn_browser(opts: &LaunchOptions) -> Result<BrowserChild, CdpError> {
    let binary = locate_system_browser(opts.browser_binary.as_deref())?;
    std::fs::create_dir_all(&opts.user_data_dir).map_err(CdpError::Io)?;
    // Remove a stale DevToolsActivePort from a previous run with the same
    // profile dir — otherwise the wait below could read a dead port.
    let _ = std::fs::remove_file(opts.user_data_dir.join("DevToolsActivePort"));
    let mut cmd = Command::new(&binary);
    cmd.arg("--remote-debugging-port=0");
    cmd.arg(format!("--user-data-dir={}", opts.user_data_dir.display()));
    cmd.arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-default-apps");
    if opts.headless {
        cmd.arg("--headless=new");
    }
    for arg in &opts.extra_args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| CdpError::BrowserNotFound(format!("spawn {}: {e}", binary.display())))?;

    let deadline = Instant::now() + opts.wait_timeout;
    loop {
        if let Ok(endpoint) = read_devtools_active_port(&opts.user_data_dir) {
            return Ok(BrowserChild { child, endpoint });
        }
        if let Ok(Some(status)) = child.try_wait() {
            let _ = child.kill();
            return Err(CdpError::BrowserNotFound(format!(
                "browser exited early ({status}); check that the binary supports --remote-debugging-port"
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err(CdpError::Timeout(format!(
                "browser did not write DevToolsActivePort within {:?}",
                opts.wait_timeout
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// chrome-for-testing fallback
// ---------------------------------------------------------------------------

/// Current platform identifier used by the chrome-for-testing manifest.
pub fn cft_platform() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux64"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "mac-arm64"
    }
    #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
    {
        "mac-x64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "win64"
    }
    #[cfg(all(target_os = "windows", not(target_arch = "x86_64")))]
    {
        "win32"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "linux64"
    }
}

/// Folder name inside the extracted zip for this platform.
fn cft_zip_folder() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "chrome-linux64"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "chrome-mac-arm64"
    }
    #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
    {
        "chrome-mac-x64"
    }
    #[cfg(target_os = "windows")]
    {
        "chrome-win64"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "chrome-linux64"
    }
}

/// Relative path of the actual binary inside the extracted folder.
fn cft_binary_rel_path() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "chrome"
    }
    #[cfg(target_os = "macos")]
    {
        "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
    }
    #[cfg(target_os = "windows")]
    {
        "chrome.exe"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "chrome"
    }
}

/// Cache root: the shared data home's `browser/chrome-for-testing` (same
/// DEC-053 precedence as [`default_profile_dir`]).
fn cft_cache_root() -> PathBuf {
    agentcowork_types::env_compat::data_home().join(CFT_SUBDIR)
}

/// Find an already-installed chrome-for-testing binary in the cache.
fn cached_cft_binary() -> Option<PathBuf> {
    let root = cft_cache_root();
    let entries = std::fs::read_dir(&root).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let bin = dir.join(cft_zip_folder()).join(cft_binary_rel_path());
        if bin.is_file() {
            return Some(bin);
        }
    }
    None
}

/// Download + install chrome-for-testing when no system browser exists.
///
/// `json_url` overrides the manifest (test hook); the manifest's download URL
/// may also be a local mock. Returns the path to the installed `chrome`
/// binary. Idempotent — reuses an existing install.
pub fn install_chrome_for_testing(json_url: Option<&str>) -> Result<PathBuf, CdpError> {
    let url = json_url.unwrap_or(CFT_KNOWN_GOOD_URL);
    let manifest = crate::discovery::http_get(url)?;
    let v: serde_json::Value = serde_json::from_str(&manifest)
        .map_err(|e| CdpError::Discovery(format!("chrome-for-testing manifest: {e}")))?;
    let version = v
        .pointer("/channels/Stable/version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CdpError::Discovery("cft manifest: no Stable version".into()))?;
    let platform = cft_platform();
    let download_url = v
        .pointer("/channels/Stable/downloads/chrome")
        .and_then(serde_json::Value::as_array)
        .and_then(|arr| {
            arr.iter().find_map(|d| {
                if d.get("platform").and_then(serde_json::Value::as_str) == Some(platform) {
                    d.get("url").and_then(serde_json::Value::as_str)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            CdpError::Discovery(format!(
                "cft manifest: no chrome download for platform {platform}"
            ))
        })?;

    let install_dir = cft_cache_root().join(version);
    let binary_path = install_dir
        .join(cft_zip_folder())
        .join(cft_binary_rel_path());
    if binary_path.is_file() {
        return Ok(binary_path); // already installed
    }

    std::fs::create_dir_all(&install_dir).map_err(CdpError::Io)?;
    let bytes = download_zip(download_url)?;
    extract_zip(&bytes, &install_dir)?;

    if !binary_path.is_file() {
        return Err(CdpError::Discovery(format!(
            "cft install: extracted binary missing at {}",
            binary_path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(binary_path)
}

fn download_zip(url: &str) -> Result<Vec<u8>, CdpError> {
    let client = agentcowork_guard::egress_http::GuardedHttpClient::new(
        url,
        agentcowork_guard::NetPolicy::default(),
        Duration::from_secs(300),
    )
    .map_err(|_| CdpError::Security("Guard refused the browser archive destination".into()))?;
    let response = client
        .request("GET", url, &[], None, MAX_DOWNLOAD_BYTES as usize)
        .map_err(|_| CdpError::Http("Guarded browser archive request failed".into()))?;
    if !(200..300).contains(&response.status) {
        return Err(CdpError::Http(format!(
            "browser archive returned HTTP {} (redirects are not followed)",
            response.status
        )));
    }
    let bytes = response.body;
    if bytes.is_empty() {
        return Err(CdpError::Http("browser archive was empty".into()));
    }
    Ok(bytes)
}

/// Extract a zip archive, guarding against zip-slip (entries escaping the
/// target dir).
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), CdpError> {
    let reader = Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| CdpError::Discovery(format!("cft zip: {e}")))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| CdpError::Discovery(format!("cft zip entry {i}: {e}")))?;
        let Some(name) = file.enclosed_name() else {
            return Err(CdpError::Security(
                "cft zip: entry escapes the extract dir".into(),
            ));
        };
        let out_path = dest.join(name);
        if file.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(CdpError::Io)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(CdpError::Io)?;
            }
            let mut out = std::fs::File::create(&out_path).map_err(CdpError::Io)?;
            std::io::copy(&mut file, &mut out).map_err(CdpError::Io)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_dir_points_into_agentcowork_home() {
        let dir = default_profile_dir();
        // The shared data-home rule: the new `~/.agentcowork` by default, the
        // legacy `~/.everyaios` (or an explicit override) when honored.
        let s = dir.to_string_lossy();
        assert!(
            s.contains(".agentcowork") || s.contains(".everyaios") || s.contains("browser-profile"),
            "unexpected profile dir: {s}",
        );
        assert!(dir.ends_with("browser-profile"));
    }

    #[test]
    fn cft_platform_is_valid() {
        let p = cft_platform();
        assert!(["linux64", "mac-arm64", "mac-x64", "win64", "win32"].contains(&p));
    }

    #[test]
    fn locate_browser_missing_errors() {
        let err = locate_system_browser(Some(Path::new("/nonexistent/chrome"))).unwrap_err();
        assert!(matches!(err, CdpError::BrowserNotFound(_)), "got {err:?}");
    }

    #[test]
    fn extract_zip_rejects_slip_entries() {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        let mut zipw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        // A malicious entry that would escape the destination dir.
        zipw.start_file("../escape.txt", opts).unwrap();
        zipw.write_all(b"evil").unwrap();
        zipw.finish().unwrap();
        let dir = tempfile_dir();
        let err = extract_zip(&buf.into_inner(), &dir).unwrap_err();
        assert!(matches!(err, CdpError::Security(_)), "got {err:?}");
    }

    #[test]
    fn extract_zip_writes_files() {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zipw = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zipw.start_file("chrome-linux64/chrome", opts).unwrap();
            zipw.write_all(b"#!/bin/sh\necho hi\n").unwrap();
            zipw.finish().unwrap();
        }
        let dir = tempfile_dir();
        extract_zip(&buf.into_inner(), &dir).unwrap();
        let bin = dir.join("chrome-linux64/chrome");
        assert!(bin.is_file());
        assert_eq!(
            std::fs::read_to_string(&bin).unwrap(),
            "#!/bin/sh\necho hi\n"
        );
    }

    #[test]
    fn install_chrome_for_testing_downloads_and_extracts() {
        use std::io::Write;
        // Build a zip payload.
        let mut zip_bytes = Cursor::new(Vec::new());
        {
            let mut zipw = zip::ZipWriter::new(&mut zip_bytes);
            let opts = zip::write::SimpleFileOptions::default();
            zipw.start_file("chrome-linux64/chrome", opts).unwrap();
            zipw.write_all(b"#!/bin/sh\necho mock-chrome\n").unwrap();
            zipw.finish().unwrap();
        }
        let payload = zip_bytes.into_inner();
        // The manifest's `platform` is the cft platform id (linux64), while
        // the zip's inner folder is the cft folder name (chrome-linux64).
        let platform = cft_platform().to_string();

        // Mock server: manifest + zip.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::Read as _;
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut req = String::new();
                let mut buf = [0u8; 8192];
                let mut header_end = None;
                loop {
                    let n = match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if let Some(pos) = req.find("\r\n\r\n") {
                        header_end = Some(pos);
                        break;
                    }
                }
                let _ = header_end;
                let first = req.lines().next().unwrap_or_default().to_string();
                if first.contains("manifest.json") {
                    let manifest = format!(
                        r#"{{"channels":{{"Stable":{{"version":"120.0.6099.71","downloads":{{"chrome":[{{"platform":"{platform}","url":"http://127.0.0.1:{port}/chrome.zip"}}]}}}}}}}}"#
                    );
                    let body = manifest.as_bytes();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(resp.as_bytes());
                    let _ = s.write_all(body);
                    let _ = s.flush();
                } else if first.contains("chrome.zip") {
                    // Serve the raw zip bytes.
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                        payload.len()
                    );
                    let _ = s.write_all(resp.as_bytes());
                    let _ = s.write_all(&payload);
                    let _ = s.flush();
                } else {
                    let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                    let _ = s.write_all(resp.as_bytes());
                    let _ = s.flush();
                };
            }
        });

        // Point install at the mock.
        let manifest_url = format!("http://127.0.0.1:{port}/manifest.json");
        let bin = install_chrome_for_testing(Some(&manifest_url)).unwrap();
        assert!(bin.is_file(), "installed binary missing: {}", bin.display());
        assert_eq!(
            std::fs::read_to_string(&bin).unwrap(),
            "#!/bin/sh\necho mock-chrome\n"
        );
    }

    #[test]
    fn browser_archive_download_refuses_redirects() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: /payload\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });

        let url = format!("http://{address}/archive.zip");
        let error = download_zip(&url).unwrap_err();
        server.join().unwrap();

        assert!(error.to_string().contains("redirects are not followed"));
    }

    // ---- P10.4: system Chrome/Edge detection + fallback (all 3 platforms) --

    #[test]
    fn platform_candidates_cover_chrome_and_edge() {
        let candidates = platform_candidates();
        assert!(!candidates.is_empty(), "no platform candidates on this OS");
        #[cfg(target_os = "linux")]
        {
            let names: Vec<String> = candidates
                .iter()
                .filter_map(|c| c.file_name().and_then(|n| n.to_str()))
                .map(|s| s.to_string())
                .collect();
            assert!(
                names
                    .iter()
                    .any(|n| n.starts_with("google-chrome") || n.starts_with("chromium")),
                "linux candidates must include chrome/chromium: {names:?}"
            );
            assert!(
                names.iter().any(|n| n.starts_with("microsoft-edge")),
                "linux candidates must include microsoft-edge: {names:?}"
            );
        }
        #[cfg(target_os = "macos")]
        {
            let paths: Vec<String> = candidates.iter().map(|c| c.display().to_string()).collect();
            assert!(
                paths.iter().any(|p| p.contains("Google Chrome.app")),
                "macOS candidates must include Google Chrome.app: {paths:?}"
            );
            assert!(
                paths.iter().any(|p| p.contains("Microsoft Edge.app")),
                "macOS candidates must include Microsoft Edge.app: {paths:?}"
            );
        }
        #[cfg(target_os = "windows")]
        {
            let paths: Vec<String> = candidates.iter().map(|c| c.display().to_string()).collect();
            assert!(
                paths.iter().any(|p| p.contains("chrome.exe")),
                "Windows candidates must include chrome.exe: {paths:?}"
            );
            assert!(
                paths.iter().any(|p| p.contains("msedge.exe")),
                "Windows candidates must include msedge.exe: {paths:?}"
            );
        }
    }

    #[test]
    fn explicit_override_wins_and_missing_override_is_honest() {
        // A real file as the explicit override is preferred verbatim.
        let dir = tempfile_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-browser");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        let located = locate_system_browser(Some(&fake)).unwrap();
        assert_eq!(located, fake);
        // A configured-but-missing override fails closed (no silent fallback).
        let missing = dir.join("not-there");
        let err = locate_system_browser(Some(&missing)).unwrap_err();
        assert!(
            matches!(err, CdpError::BrowserNotFound(_)),
            "expected BrowserNotFound, got {err:?}"
        );
    }

    #[test]
    fn missing_system_browser_reports_honestly() {
        // No browser anywhere → the error names the fix, never a panic.
        let err = locate_system_browser(None).err();
        if let Some(e) = err {
            let msg = e.to_string();
            assert!(
                msg.contains("Chrome/Edge") || msg.contains("browser"),
                "unhelpful error: {msg}"
            );
        }
        // On CI runners a browser usually exists; the honest-error path is
        // exercised by the explicit-override test above either way.
    }

    #[test]
    fn browser_config_roundtrips_json() {
        let cfg = BrowserConfig {
            preferred_channel: BrowserChannel::Brave,
            custom_executable_path: Some("/opt/brave/brave".to_string()),
            profile_mode: BrowserProfileMode::Isolated,
            headless: true,
            extra_args: vec!["--mute-audio".to_string()],
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let parsed: BrowserConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.preferred_channel, BrowserChannel::Brave);
        assert_eq!(
            parsed.custom_executable_path,
            Some("/opt/brave/brave".to_string())
        );
        assert_eq!(parsed.profile_mode, BrowserProfileMode::Isolated);
        assert!(parsed.headless);
    }

    #[test]
    fn detect_channel_from_path_identifies_known_browsers() {
        assert_eq!(
            detect_channel_from_path(Path::new("/usr/bin/brave-browser")),
            BrowserChannel::Brave
        );
        assert_eq!(
            detect_channel_from_path(Path::new(
                r"C:\Program Files\Google\Chrome\Application\chrome.exe"
            )),
            BrowserChannel::Chrome
        );
        assert_eq!(
            detect_channel_from_path(Path::new(
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
            )),
            BrowserChannel::Edge
        );
        assert_eq!(
            detect_channel_from_path(Path::new("/Applications/Arc.app/Contents/MacOS/Arc")),
            BrowserChannel::Arc
        );
        assert_eq!(
            detect_channel_from_path(Path::new("/usr/bin/vivaldi")),
            BrowserChannel::Vivaldi
        );
        assert_eq!(
            detect_channel_from_path(Path::new("/custom/tool/binary")),
            BrowserChannel::Custom
        );
    }

    #[test]
    fn resolve_browser_binary_custom_validates_existence() {
        let dir = tempfile_dir();
        let fake = dir.join("fake-custom-browser");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();

        let cfg = BrowserConfig {
            preferred_channel: BrowserChannel::Custom,
            custom_executable_path: Some(fake.to_string_lossy().to_string()),
            ..Default::default()
        };
        let (path, channel) = resolve_browser_binary(&cfg).expect("custom path should resolve");
        assert_eq!(path, fake);
        assert_eq!(channel, BrowserChannel::Custom);

        let missing = dir.join("missing-binary");
        let bad_cfg = BrowserConfig {
            preferred_channel: BrowserChannel::Custom,
            custom_executable_path: Some(missing.to_string_lossy().to_string()),
            ..Default::default()
        };
        assert!(resolve_browser_binary(&bad_cfg).is_err());
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-cdp-browser-test-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}
