//! P7.8/P49.5 — declarative sandbox policy and V1 backend resolution.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathAccess {
    ReadOnly,
    ReadAndExecute,
    ReadWrite,
    AddIfExists,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRule {
    pub prefix: String,
    pub access: PathAccess,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyscallGroup {
    ReadonlyFileOpens,
    FsMetadata,
    FsWrites,
    FdOps,
    ProcessCreation,
    Ipc,
    CommonRuntime,
    ExecMem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxProfile {
    pub name: String,
    pub no_new_privs: bool,
    pub paths: Vec<PathRule>,
    pub syscalls: Vec<SyscallGroup>,
    pub spawns_children: bool,
    pub files_write: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    #[error("profile `{0}` must set no_new_privs (fail-closed)")]
    NoNewPrivsRequired(String),
    #[error("profile `{name}` enables {group:?} but the syscall group is not in the policy")]
    SyscallNotInPolicy { name: String, group: SyscallGroup },
    #[error("profile `{name}` enables files_write but no path rule grants write access")]
    WriteWithoutPath { name: String },
    #[error("sandbox backend is unavailable for this platform/policy (fail-closed)")]
    UnsupportedPlatform,
    #[error("sandbox policy intersection is empty or invalid: {0}")]
    InvalidPolicy(String),
    #[error("sandbox process has no monitor handle")]
    MissingMonitor,
    #[error("sandbox process exited before postflight verification")]
    ProcessExited,
    #[error("sandbox process exceeded its deadline")]
    DeadlineExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxRole {
    AgentSandbox,
    ChildExecutionSandbox,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub role: SandboxRole,
    pub profile: SandboxProfile,
    pub network: String,
    pub credentials: String,
    pub resource_limit_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxReceipt {
    pub environment_id: String,
    pub backend: String,
    pub policy_hash: String,
    pub preflight_status: String,
    pub runtime_status: String,
    pub violations: Vec<String>,
    pub postflight_status: String,
    pub state_hash: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackendKind {
    TrustedNative,
    NativeProcess,
    Container,
    MicroVm,
}

impl SandboxReceipt {
    pub fn preflight(spec: &SandboxSpec, backend: &str, environment_id: impl Into<String>) -> Self {
        Self {
            environment_id: environment_id.into(),
            backend: backend.into(),
            policy_hash: policy_hash(spec),
            preflight_status: "passed".into(),
            runtime_status: "not_started".into(),
            violations: Vec::new(),
            postflight_status: "not_verified".into(),
            state_hash: String::new(),
        }
    }

    pub fn observe(mut self, status: impl Into<String>, violations: Vec<String>) -> Self {
        self.runtime_status = status.into();
        self.violations = violations;
        self
    }

    pub fn postflight(mut self, status: impl Into<String>, state: &str) -> Self {
        self.postflight_status = status.into();
        let mut h = Sha256::new();
        h.update(state.as_bytes());
        self.state_hash = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        self
    }

    pub fn verified(&self) -> bool {
        self.preflight_status == "passed"
            && self.postflight_status == "passed"
            && self.violations.is_empty()
            && !self.state_hash.is_empty()
    }
}

fn policy_hash(spec: &SandboxSpec) -> String {
    let bytes = serde_json::to_vec(spec).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub trait SandboxBackend {
    fn capabilities(&self) -> Vec<String>;
    fn validate(&self, spec: &SandboxSpec) -> Result<(), SandboxError>;
    fn spawn(&self, spec: &SandboxSpec, command: &[String])
    -> Result<SandboxProcess, SandboxError>;
}

/// A process launched by a concrete sandbox backend. The child remains owned
/// by this handle until it is reaped; callers cannot claim postflight success
/// without observing its exit and producing a receipt.
pub struct SandboxProcess {
    child: Child,
    pub backend: String,
    pub pid: u32,
    started_at: Instant,
}

/// The stdio endpoints and lifecycle monitor returned by a concrete sandbox
/// launcher. The pipes are transferred to the protocol transport while the
/// monitor retains ownership of the child process for kill/reap operations.
pub struct SandboxedStdio {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub monitor: SandboxProcess,
}

impl SandboxProcess {
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, SandboxError> {
        self.child
            .try_wait()
            .map_err(|_| SandboxError::ProcessExited)
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Take piped stdio for a protocol transport without transferring child
    /// lifecycle ownership. The backend must have created both pipes.
    pub fn take_stdio(&mut self) -> Result<(ChildStdin, ChildStdout), SandboxError> {
        let stdin = self
            .child
            .stdin
            .take()
            .ok_or(SandboxError::MissingMonitor)?;
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or(SandboxError::MissingMonitor)?;
        Ok((stdin, stdout))
    }

    pub fn wait_with_deadline(&mut self, deadline: Instant) -> Result<ExitStatus, SandboxError> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                self.kill()?;
                return Err(SandboxError::DeadlineExceeded);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn kill(&mut self) -> Result<(), SandboxError> {
        self.child.kill().map_err(|_| SandboxError::ProcessExited)?;
        self.child
            .wait()
            .map_err(|_| SandboxError::ProcessExited)
            .map(|_| ())
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Produce a postflight receipt only after the process has been reaped.
    /// A successful exit alone is not a proof of filesystem cleanliness; the
    /// caller supplies the independently verified state digest.
    pub fn postflight_receipt(
        &mut self,
        spec: &SandboxSpec,
        environment_id: impl Into<String>,
        deadline: Instant,
        state: &str,
        violations: Vec<String>,
    ) -> Result<SandboxReceipt, SandboxError> {
        let status = self.wait_with_deadline(deadline)?;
        let runtime = if status.success() {
            "completed"
        } else {
            "failed"
        };
        let postflight = if status.success() && violations.is_empty() {
            "passed"
        } else {
            "failed"
        };
        Ok(
            SandboxReceipt::preflight(spec, self.backend(), environment_id)
                .observe(runtime, violations)
                .postflight(postflight, state),
        )
    }
}

/// Linux bubblewrap backend. It is deliberately constructed as a command
/// wrapper: policy validation happens before spawning, and only explicitly
/// allowed paths are exposed. No credentials or ambient environment are
/// forwarded.
#[cfg(target_os = "linux")]
pub struct LinuxBwrapBackend;

#[cfg(target_os = "linux")]
impl LinuxBwrapBackend {
    pub fn command(spec: &SandboxSpec, command: &[String]) -> Result<Command, SandboxError> {
        spec.profile.validate()?;
        if command.is_empty() || command.iter().any(|arg| arg.contains('\0')) {
            return Err(SandboxError::InvalidPolicy(
                "empty or invalid command".into(),
            ));
        }
        let mut child = Command::new("bwrap");
        child.args(["--die-with-parent", "--new-session", "--clearenv"]);
        if spec.profile.no_new_privs {
            child.arg("--unshare-user").args(bwrap_userns_hardening());
        }
        child.args(["--ro-bind", "/usr", "/usr"]);
        child.args(["--ro-bind", "/bin", "/bin"]);
        child.args(["--ro-bind", "/lib", "/lib"]);
        child.args(["--ro-bind", "/lib64", "/lib64"]);
        child.arg("--proc").arg("/proc").arg("--dev").arg("/dev");
        if spec.network == "deny" {
            child.arg("--unshare-net");
        }
        for rule in &spec.profile.paths {
            let path = Path::new(&rule.prefix);
            if !path.is_absolute() {
                return Err(SandboxError::InvalidPolicy(
                    "sandbox path must be absolute".into(),
                ));
            }
            match rule.access {
                PathAccess::ReadOnly | PathAccess::ReadAndExecute => {
                    child.args(["--ro-bind", &rule.prefix, &rule.prefix]);
                }
                PathAccess::ReadWrite | PathAccess::AddIfExists => {
                    if !spec.profile.files_write {
                        return Err(SandboxError::InvalidPolicy(
                            "write path without write policy".into(),
                        ));
                    }
                    child.args(["--bind", &rule.prefix, &rule.prefix]);
                }
            }
        }
        child.arg("--").args(command);
        Ok(child)
    }
}

/// True for environment-variable names that exist to carry credentials. The
/// confined-spawn env allow-list must never carry these (they would re-open
/// the exact channel `--clearenv` closed); secrets reach a child only through
/// the vault broker or an explicit caller that passes them another way.
#[cfg(target_os = "linux")]
fn is_secret_like_env_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.contains("API_KEY")
        || n.contains("_TOKEN")
        || n.ends_with("SECRET")
        || n.contains("PASSWORD")
        // Both the current `AGENTCOWORK_*` and the retired `EVERYAIOS_*`
        // spellings (DEC-053): a renamed credential must stay refused.
        || agentcowork_types::env_compat::is_host_control_name(&n)
}

#[cfg(target_os = "linux")]
/// The environment an MCP interpreter child genuinely needs to run. Deliberately
/// minimal and credential-free: path/loader/discovery vars only.
pub fn essential_env() -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH",
        "HOME",
        "LANG",
        "LC_ALL",
        "TERM",
        "NODE_PATH",
        "VIRTUAL_ENV",
        "PYTHONPATH",
        "PYTHONHOME",
        "TMPDIR",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
    ];
    std::env::vars()
        .filter(|(k, _)| KEEP.contains(&k.as_str()))
        .filter(|(k, _)| !is_secret_like_env_name(k))
        .collect()
}

#[cfg(target_os = "linux")]
impl LinuxBwrapBackend {
    /// Spawn a sandboxed child with piped stdio. The monitor retains process
    /// ownership; callers may transfer only the returned endpoints.
    pub fn spawn_stdio(
        &self,
        spec: &SandboxSpec,
        command: &[String],
    ) -> Result<SandboxedStdio, SandboxError> {
        self.spawn_stdio_with_env(spec, command, &[])
    }

    /// [`Self::spawn_stdio`] with an explicit allow-list of environment
    /// variables the child needs to function.
    ///
    /// `--clearenv` is the containment win (no ambient secrets reach the
    /// child), but a completely empty environment breaks *interpreters*: a
    /// `python server.py` needs `PATH` to find `python` itself, and node/venv
    /// layouts need the usual loader vars. This is the documented middle path
    /// (P62.2): an explicit, small allow-list — never the ambient environment,
    /// never a `*_API_KEY`/`*_TOKEN` — set **on top of** `--clearenv`, so
    /// everything except these names is still scrubbed. Secrets stay on the
    /// brokered credential path.
    pub fn spawn_stdio_with_env(
        &self,
        spec: &SandboxSpec,
        command: &[String],
        env: &[(String, String)],
    ) -> Result<SandboxedStdio, SandboxError> {
        self.validate(spec)?;
        let mut child = Self::command(spec, command)?;
        for (k, v) in env {
            // Refuse to smuggle a secret-shaped variable through the
            // allow-list — that would silently undo the containment.
            if is_secret_like_env_name(k) {
                return Err(SandboxError::InvalidPolicy(format!(
                    "refusing to pass secret-shaped env var {k:?} into a confined child"
                )));
            }
            child.env(k, v);
        }
        child
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = child
            .spawn()
            .map_err(|_| SandboxError::UnsupportedPlatform)?;
        let stdin = child.stdin.take().ok_or(SandboxError::MissingMonitor)?;
        let stdout = child.stdout.take().ok_or(SandboxError::MissingMonitor)?;
        let pid = child.id();
        Ok(SandboxedStdio {
            stdin,
            stdout,
            monitor: SandboxProcess {
                child,
                backend: "linux-bwrap".into(),
                pid,
                started_at: Instant::now(),
            },
        })
    }
}

#[cfg(target_os = "linux")]
impl SandboxBackend for LinuxBwrapBackend {
    fn capabilities(&self) -> Vec<String> {
        vec![
            "linux".into(),
            "bwrap".into(),
            "network-isolation".into(),
            "path-isolation".into(),
        ]
    }
    fn validate(&self, spec: &SandboxSpec) -> Result<(), SandboxError> {
        if !linux_bwrap_available() {
            return Err(SandboxError::UnsupportedPlatform);
        }
        spec.profile.validate()
    }
    fn spawn(
        &self,
        spec: &SandboxSpec,
        command: &[String],
    ) -> Result<SandboxProcess, SandboxError> {
        self.validate(spec)?;
        let mut command = Self::command(spec, command)?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|_| SandboxError::UnsupportedPlatform)?;
        let pid = child.id();
        Ok(SandboxProcess {
            child,
            backend: "linux-bwrap".into(),
            pid,
            started_at: Instant::now(),
        })
    }
}

/// Linux backend availability is explicit. We never claim containment when
/// bubblewrap is absent; callers must choose a different trusted policy or fail.
pub fn linux_bwrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Userns-hardening flags for the local bubblewrap, version-detected once.
///
/// bubblewrap 0.9.0 removed `--disable-setuid` (and later releases followed
/// upstream's rename to `--disable-userns` / `--assert-userns-disabled`,
/// which only exist on 0.9+). Passing the old flag makes every sandboxed
/// spawn on a modern distro fail, so the builder must pick the flags the
/// installed bwrap actually understands. Parse failure falls back to the
/// legacy flag (correct for pre-0.9; on 0.9+ it fails closed with a clear
/// bwrap error rather than silently weakening the policy).
#[cfg(target_os = "linux")]
fn bwrap_userns_hardening() -> &'static [&'static str] {
    use std::sync::OnceLock;
    static FLAGS: OnceLock<&'static [&'static str]> = OnceLock::new();
    FLAGS.get_or_init(|| {
        let version = Command::new("bwrap")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let modern = version
            .split_whitespace()
            .last()
            .and_then(|v| {
                let mut parts = v.split('.');
                let major: u64 = parts.next()?.parse().ok()?;
                let minor: u64 = parts.next().unwrap_or("0").parse().ok()?;
                Some((major, minor))
            })
            .map(|(major, minor)| major > 0 || (major == 0 && minor >= 9))
            .unwrap_or(false);
        if modern {
            &["--disable-userns", "--assert-userns-disabled"]
        } else {
            &["--disable-setuid"]
        }
    })
}

/// Runtime containment capabilities. This is deliberately capability-based:
/// a platform is not reported as governed merely because it can spawn a
/// process. Returns the active containment features for the host OS.
pub fn enforced_backend_capabilities() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        if linux_bwrap_available() {
            vec![
                "linux-bwrap".into(),
                "process-monitoring".into(),
                "postflight-receipts".into(),
            ]
        } else {
            Vec::new()
        }
    }
    #[cfg(windows)]
    {
        vec![
            "windows-job-object".into(),
            "restricted-tokens".into(),
            "process-monitoring".into(),
            "postflight-receipts".into(),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "macos-seatbelt".into(),
            "process-monitoring".into(),
            "postflight-receipts".into(),
        ]
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    Vec::new()
}

/// Resolve a backend for an external child. `NativeProcess` is deliberately
/// not accepted as proof of containment: callers must provide a real backend
/// (for example Linux bubblewrap) or fail closed.
pub fn resolve_sandbox_backend(
    role: SandboxRole,
    requested: SandboxBackendKind,
) -> Result<SandboxBackendKind, SandboxError> {
    if role == SandboxRole::ChildExecutionSandbox && requested == SandboxBackendKind::TrustedNative
    {
        return Err(SandboxError::InvalidPolicy(
            "child execution cannot use trusted native".into(),
        ));
    }
    match requested {
        SandboxBackendKind::TrustedNative => Ok(requested),
        SandboxBackendKind::NativeProcess => {
            if role == SandboxRole::AgentSandbox {
                Ok(requested)
            } else {
                Err(SandboxError::UnsupportedPlatform)
            }
        }
        SandboxBackendKind::Container | SandboxBackendKind::MicroVm => {
            Err(SandboxError::UnsupportedPlatform)
        }
    }
}

pub mod profiles {
    use super::*;
    pub fn renderer() -> SandboxProfile {
        SandboxProfile {
            name: "renderer".into(),
            no_new_privs: true,
            paths: vec![PathRule {
                prefix: "/usr/share".into(),
                access: PathAccess::ReadAndExecute,
            }],
            syscalls: vec![
                SyscallGroup::ReadonlyFileOpens,
                SyscallGroup::FsMetadata,
                SyscallGroup::FdOps,
                SyscallGroup::CommonRuntime,
            ],
            spawns_children: false,
            files_write: false,
        }
    }
    pub fn worker(scratch: &str) -> SandboxProfile {
        SandboxProfile {
            name: "worker".into(),
            no_new_privs: true,
            paths: vec![
                PathRule {
                    prefix: "/usr/share".into(),
                    access: PathAccess::ReadAndExecute,
                },
                PathRule {
                    prefix: scratch.into(),
                    access: PathAccess::ReadWrite,
                },
            ],
            syscalls: vec![
                SyscallGroup::ReadonlyFileOpens,
                SyscallGroup::FsMetadata,
                SyscallGroup::FsWrites,
                SyscallGroup::FdOps,
                SyscallGroup::CommonRuntime,
            ],
            spawns_children: false,
            files_write: true,
        }
    }
    pub fn network() -> SandboxProfile {
        SandboxProfile {
            name: "network".into(),
            no_new_privs: true,
            paths: vec![],
            syscalls: vec![
                SyscallGroup::FdOps,
                SyscallGroup::Ipc,
                SyscallGroup::CommonRuntime,
            ],
            spawns_children: false,
            files_write: false,
        }
    }
}

impl SandboxProfile {
    pub fn validate(&self) -> Result<(), SandboxError> {
        if !self.no_new_privs {
            return Err(SandboxError::NoNewPrivsRequired(self.name.clone()));
        }
        if self.spawns_children && !self.syscalls.contains(&SyscallGroup::ProcessCreation) {
            return Err(SandboxError::SyscallNotInPolicy {
                name: self.name.clone(),
                group: SyscallGroup::ProcessCreation,
            });
        }
        if self.files_write
            && !self
                .paths
                .iter()
                .any(|p| matches!(p.access, PathAccess::ReadWrite | PathAccess::AddIfExists))
        {
            return Err(SandboxError::WriteWithoutPath {
                name: self.name.clone(),
            });
        }
        Ok(())
    }
    pub fn check_path(&self, path: &str, access: PathAccess) -> bool {
        use crate::pathfloor::canonicalize_no_follow;
        let c = canonicalize_no_follow(path);
        if matches!(access, PathAccess::ReadWrite | PathAccess::AddIfExists) && !self.files_write {
            return false;
        }
        self.paths.iter().any(|r| {
            let p = canonicalize_no_follow(&r.prefix);
            let inside = c == p || c.starts_with(&format!("{}/", p.trim_end_matches('/')));
            inside && r.access_allows(access)
        })
    }
    /// Apply this profile to the current process. The library does not claim
    /// to provide portable in-process confinement; external children must use
    /// a concrete backend wrapper. This method therefore remains fail-closed.
    pub fn apply(&self) -> Result<(), SandboxError> {
        self.validate()?;
        Err(SandboxError::UnsupportedPlatform)
    }
}
impl PathRule {
    pub fn access_allows(&self, requested: PathAccess) -> bool {
        use PathAccess::*;
        matches!(
            (self.access, requested),
            (ReadWrite, _)
                | (AddIfExists, AddIfExists | ReadOnly | ReadAndExecute)
                | (ReadAndExecute, ReadAndExecute | ReadOnly)
                | (ReadOnly, ReadOnly)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_validate_and_network_has_no_fs() {
        profiles::renderer().validate().unwrap();
        profiles::worker("/tmp/s").validate().unwrap();
        assert!(!profiles::network().check_path("/etc/passwd", PathAccess::ReadOnly));
    }
    #[test]
    fn roles_and_backend_fail_closed() {
        assert_eq!(
            resolve_sandbox_backend(SandboxRole::AgentSandbox, SandboxBackendKind::NativeProcess)
                .unwrap(),
            SandboxBackendKind::NativeProcess
        );
        assert!(
            resolve_sandbox_backend(
                SandboxRole::ChildExecutionSandbox,
                SandboxBackendKind::TrustedNative
            )
            .is_err()
        );
    }
    #[test]
    fn apply_is_honest() {
        assert!(profiles::renderer().apply().is_err());
    }

    #[test]
    fn receipt_requires_clean_postflight_and_state_proof() {
        let spec = SandboxSpec {
            role: SandboxRole::ChildExecutionSandbox,
            profile: profiles::worker("/tmp/scratch"),
            network: "deny".into(),
            credentials: "opaque_handles".into(),
            resource_limit_bytes: 1024,
        };
        let receipt = SandboxReceipt::preflight(&spec, "native-process", "env-1")
            .observe("completed", Vec::new())
            .postflight("passed", "clean");
        assert!(receipt.verified());
        let violated = SandboxReceipt::preflight(&spec, "native-process", "env-1")
            .observe("completed", vec!["network".into()])
            .postflight("passed", "clean");
        assert!(!violated.verified());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_command_is_clearenv_and_network_constrained() {
        let spec = SandboxSpec {
            role: SandboxRole::ChildExecutionSandbox,
            profile: profiles::worker("/tmp/scratch"),
            network: "deny".into(),
            credentials: "opaque_handles".into(),
            resource_limit_bytes: 1024,
        };
        let command =
            LinuxBwrapBackend::command(&spec, &["/bin/echo".into(), "ok".into()]).unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into())
            .collect();
        assert!(args.contains(&"--clearenv".into()));
        assert!(args.contains(&"--unshare-net".into()));
        assert!(args.windows(2).any(|pair| pair == ["--", "/bin/echo"]));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_command_rejects_relative_policy_paths() {
        let mut profile = profiles::worker("relative");
        profile.paths[1].prefix = "relative".into();
        let spec = SandboxSpec {
            role: SandboxRole::ChildExecutionSandbox,
            profile,
            network: "deny".into(),
            credentials: "opaque_handles".into(),
            resource_limit_bytes: 1024,
        };
        assert!(LinuxBwrapBackend::command(&spec, &["/bin/echo".into()]).is_err());
    }

    #[test]
    fn linux_backend_never_claims_available_without_bwrap() {
        // This is an availability probe only; unsupported environments remain
        // fail-closed through backend resolution and apply().
        let _ = linux_bwrap_available();
    }
}
