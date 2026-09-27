//! P8.6 WSL/POSIX Bridge (F10 — doc 03 §5, v2.0 §P5).
//!
//! Lets the app run and manage Linux tooling from a Windows host (and from
//! inside WSL) through four pure, fully-testable pieces plus explicit runtime
//! seams:
//!
//! - [`WslPath`] + [`translate_windows_to_linux`] / [`translate_linux_to_windows`]
//!   / [`translate_windows_drive_to_linux`] — `\\wsl.localhost\` path translation.
//! - [`ExecEnvironment`] + [`detect_environment`] — native Linux vs WSL1 vs
//!   WSL2 vs Windows detection.
//! - [`WslRunner`] — a `wsl.exe`-style command wrapper (argv construction is
//!   pure and tested; execution goes through the injected [`CommandRunner`]
//!   seam so the guard still sees the full command).
//! - [`WslFrame`] — length-prefixed loopback IPC framing (protocol tested;
//!   the socket/stdio transport is a runtime seam, documented below).

use crate::forge::{CommandOutput, CommandRunner};

/// A translated path pair. `distro == None` means the default WSL distro.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WslPath {
    pub distro: Option<String>,
    /// Linux form (forward slashes, leading `/`).
    pub linux: String,
    /// Windows UNC form (`\\wsl.localhost\<distro>\...`).
    pub windows_unc: String,
}

/// Current WSL UNC prefix (`\\wsl.localhost\`).
pub const WSL_UNC_PREFIX: &str = "\\\\wsl.localhost\\";
/// Legacy WSL prefix (`\\wsl$\`) — accepted on input, normalized on output.
pub const WSL_LEGACY_PREFIX: &str = "\\\\wsl$\\";

/// Translate a Windows UNC path into a Linux path.
///
/// Accepts both `\\wsl.localhost\<distro>\...` and legacy `\\wsl$\<distro>\...`.
/// Returns `None` when the input is not a WSL path.
pub fn translate_windows_to_linux(input: &str) -> Option<WslPath> {
    let trimmed = input.trim().trim_start_matches('"').trim_end_matches('"');
    let rest = trimmed
        .strip_prefix(WSL_UNC_PREFIX)
        .or_else(|| trimmed.strip_prefix(WSL_LEGACY_PREFIX))?;
    if rest.is_empty() {
        return None;
    }
    let (distro, linux_rel) = match rest.split_once('\\') {
        Some((d, r)) if !r.is_empty() => (Some(d.to_string()), r),
        _ => (None, rest),
    };
    let linux = format!("/{}", linux_rel.replace('\\', "/"));
    let windows_unc = format!("{WSL_UNC_PREFIX}{rest}");
    Some(WslPath {
        distro,
        linux,
        windows_unc,
    })
}

/// Translate a Linux path back into the Windows UNC form.
pub fn translate_linux_to_windows(linux_path: &str, distro: Option<&str>) -> WslPath {
    let linux = linux_path.trim().trim_start_matches('/').to_string();
    let windows_unc = match distro {
        Some(d) => format!("{WSL_UNC_PREFIX}{d}\\{}", linux.replace('/', "\\")),
        None => format!("{WSL_UNC_PREFIX}{}", linux.replace('/', "\\")),
    };
    WslPath {
        distro: distro.map(|s| s.to_string()),
        linux: format!("/{linux}"),
        windows_unc,
    }
}

/// Translate a Windows drive path (`C:\foo\bar`) into the WSL `/mnt/<drive>`
/// form. Returns `None` when the input is not a drive path.
pub fn translate_windows_drive_to_linux(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let rest = trimmed[2..].trim_start_matches(['\\', '/']);
    Some(format!("/mnt/{drive}/{}", rest.replace('\\', "/")))
}

/// Where the current process is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecEnvironment {
    /// Native Linux (no WSL).
    NativeLinux,
    /// WSL1.
    Wsl1,
    /// WSL2.
    Wsl2,
    /// Native Windows (cmd.exe / PowerShell host, no WSL).
    Windows,
    /// Unknown / not determinable from the given inputs.
    Unknown,
}

impl ExecEnvironment {
    pub fn is_linux(&self) -> bool {
        matches!(
            self,
            ExecEnvironment::NativeLinux | ExecEnvironment::Wsl1 | ExecEnvironment::Wsl2
        )
    }

    pub fn is_wsl(&self) -> bool {
        matches!(self, ExecEnvironment::Wsl1 | ExecEnvironment::Wsl2)
    }
}

/// Detect the execution environment from pure inputs (testable). Callers wire
/// real values from `std::env`; see [`detect_environment_from_env`].
pub fn detect_environment(
    wsl_distro_name: Option<&str>,
    os_env: Option<&str>,
    proc_version: Option<&str>,
) -> ExecEnvironment {
    let version = proc_version.unwrap_or("");
    let microsoft = version.contains("microsoft");
    // WSL2 sets WSL_DISTRO_NAME; WSL1 does not but sets WSL_INTEROP and its
    // /proc/version still says "microsoft".
    if wsl_distro_name.is_some() {
        if microsoft && version.contains("WSL2") {
            return ExecEnvironment::Wsl2;
        }
        if microsoft && version.contains("WSL1") {
            return ExecEnvironment::Wsl1;
        }
        // WSL_DISTRO_NAME present → WSL2 is the modern default.
        return ExecEnvironment::Wsl2;
    }
    if microsoft {
        return ExecEnvironment::Wsl1;
    }
    if os_env
        .map(|o| o.eq_ignore_ascii_case("Windows_NT"))
        .unwrap_or(false)
    {
        return ExecEnvironment::Windows;
    }
    if cfg!(target_os = "linux") {
        return ExecEnvironment::NativeLinux;
    }
    ExecEnvironment::Unknown
}

/// Detect the environment from the live process. Pure reads of `std::env` +
/// `/proc/version` (best-effort); the actual detection logic lives in
/// [`detect_environment`].
pub fn detect_environment_from_env() -> ExecEnvironment {
    let wsl_distro = std::env::var("WSL_DISTRO_NAME").ok();
    let os_env = std::env::var("OS").ok();
    let proc_version = std::fs::read_to_string("/proc/version")
        .ok()
        .map(|s| s.trim().to_string());
    detect_environment(
        wsl_distro.as_deref(),
        os_env.as_deref(),
        proc_version.as_deref(),
    )
}

/// A `wsl.exe`-style command wrapper. `build_argv` is pure and tested; actual
/// execution goes through the injected [`CommandRunner`] seam so the guard
/// still observes the full command (never a bypass).
#[derive(Debug, Clone)]
pub struct WslRunner {
    /// Binary name: `wsl.exe` on a Windows host, `wsl` inside WSL.
    pub wsl_binary: String,
    /// Target distro (`None` = default distro).
    pub distro: Option<String>,
    /// `true` = Windows host (`wsl.exe -d <d> -e <cmd>`); `false` = inside WSL
    /// (`wsl -d <d> -- <cmd>`).
    pub use_wsl_exe: bool,
}

impl WslRunner {
    /// Default runner for the current platform.
    pub fn default_runner() -> Self {
        let use_wsl_exe = cfg!(target_os = "windows");
        Self {
            wsl_binary: if use_wsl_exe {
                "wsl.exe".to_string()
            } else {
                "wsl".to_string()
            },
            distro: None,
            use_wsl_exe,
        }
    }

    pub fn with_distro(mut self, distro: &str) -> Self {
        self.distro = Some(distro.to_string());
        self
    }

    /// Build the full argv for running `command` inside the distro.
    pub fn build_argv(&self, command: &str, args: &[&str]) -> Vec<String> {
        let mut argv = vec![self.wsl_binary.clone()];
        if let Some(d) = &self.distro {
            argv.push("-d".to_string());
            argv.push(d.clone());
        }
        if self.use_wsl_exe {
            argv.push("-e".to_string());
        } else {
            argv.push("--".to_string());
        }
        argv.push(command.to_string());
        argv.extend(args.iter().map(|s| s.to_string()));
        argv
    }

    /// Run a command inside the distro through the injected runner. The argv
    /// is built by [`build_argv`] (pure); the runner is the execution seam.
    pub fn run_through<R: CommandRunner>(
        &self,
        runner: &R,
        command: &str,
        args: &[&str],
        cwd: &str,
    ) -> Result<CommandOutput, String> {
        let argv = self.build_argv(command, args);
        let bin = argv[0].clone();
        let rest: Vec<&str> = argv[1..].iter().map(|s| s.as_str()).collect();
        runner.run(&bin, &rest, cwd)
    }
}

/// One length-prefixed frame for loopback IPC between a Windows host and WSL.
///
/// Wire format: `[4-byte big-endian length][UTF-8 JSON payload]`. The protocol
/// is pure and tested here; the actual transport (Unix socket / TCP loopback /
/// stdio) is a runtime seam the coordinator wires.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WslFrame {
    pub id: u64,
    pub method: String,
    #[serde(default = "serde_json::Value::default")]
    pub params: serde_json::Value,
}

impl WslFrame {
    pub fn new(id: u64, method: &str, params: serde_json::Value) -> Self {
        Self {
            id,
            method: method.to_string(),
            params,
        }
    }

    /// Encode to the wire format.
    pub fn encode(&self) -> Vec<u8> {
        let payload = serde_json::to_vec(self).expect("frame is serializable");
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Decode all complete frames from a buffer, returning the frames and the
    /// leftover bytes (so a frame split across two reads is handled — the
    /// caller keeps the remainder and prepends it to the next read).
    pub fn decode_all(buf: &[u8]) -> (Vec<WslFrame>, &[u8]) {
        let mut frames = Vec::new();
        let mut offset = 0;
        while buf.len() - offset >= 4 {
            let len = u32::from_be_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]) as usize;
            if buf.len() - offset - 4 < len {
                break; // incomplete frame
            }
            let payload = &buf[offset + 4..offset + 4 + len];
            if let Ok(frame) = serde_json::from_slice(payload) {
                frames.push(frame);
            }
            offset += 4 + len;
        }
        (frames, &buf[offset..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_unc_to_linux_with_distro() {
        let p = translate_windows_to_linux(r"\\wsl.localhost\Ubuntu\home\user\x.txt").unwrap();
        assert_eq!(p.distro.as_deref(), Some("Ubuntu"));
        assert_eq!(p.linux, "/home/user/x.txt");
        assert_eq!(p.windows_unc, r"\\wsl.localhost\Ubuntu\home\user\x.txt");
    }

    #[test]
    fn translate_legacy_prefix_and_default_distro() {
        let p = translate_windows_to_linux(r"\\wsl$\Ubuntu-22.04\etc\hosts").unwrap();
        assert_eq!(p.distro.as_deref(), Some("Ubuntu-22.04"));
        assert_eq!(p.linux, "/etc/hosts");
        // Legacy prefix normalizes to the current UNC form.
        assert_eq!(p.windows_unc, r"\\wsl.localhost\Ubuntu-22.04\etc\hosts");
        // First segment after the prefix is always the distro name (WSL UNC
        // convention): `\\wsl.localhost\home\user` = distro `home`, path `/user`.
        let d = translate_windows_to_linux(r"\\wsl.localhost\home\user").unwrap();
        assert_eq!(d.distro.as_deref(), Some("home"));
        assert_eq!(d.linux, "/user");
    }

    #[test]
    fn translate_rejects_non_wsl_paths() {
        assert!(translate_windows_to_linux(r"C:\foo").is_none());
        assert!(translate_windows_to_linux(r"\\server\share\foo").is_none());
        assert!(translate_windows_to_linux("").is_none());
        assert!(translate_windows_to_linux(r"\\wsl.localhost\").is_none());
    }

    #[test]
    fn translate_linux_to_windows_roundtrip() {
        let win = translate_linux_to_windows("/home/user/x.txt", Some("Ubuntu"));
        assert_eq!(win.windows_unc, r"\\wsl.localhost\Ubuntu\home\user\x.txt");
        let back = translate_windows_to_linux(&win.windows_unc).unwrap();
        assert_eq!(back.linux, "/home/user/x.txt");
        assert_eq!(back.distro.as_deref(), Some("Ubuntu"));
    }

    #[test]
    fn translate_drive_to_mnt() {
        assert_eq!(
            translate_windows_drive_to_linux(r"C:\Users\me\file.txt").as_deref(),
            Some("/mnt/c/Users/me/file.txt")
        );
        assert_eq!(
            translate_windows_drive_to_linux(r"D:/data").as_deref(),
            Some("/mnt/d/data")
        );
        assert_eq!(translate_windows_drive_to_linux("not-a-drive"), None);
        assert_eq!(translate_windows_drive_to_linux(r"1:\x"), None);
    }

    #[test]
    fn detect_wsl2_and_wsl1_and_native() {
        assert_eq!(
            detect_environment(
                Some("Ubuntu"),
                None,
                Some("Linux version 5.15 ... microsoft ... WSL2")
            ),
            ExecEnvironment::Wsl2
        );
        assert_eq!(
            detect_environment(None, None, Some("Linux version 4.4 ... microsoft")),
            ExecEnvironment::Wsl1
        );
        assert_eq!(
            detect_environment(None, Some("Windows_NT"), None),
            ExecEnvironment::Windows
        );
        assert_eq!(
            detect_environment(None, None, None),
            ExecEnvironment::NativeLinux
        );
        assert_eq!(
            detect_environment(Some("Ubuntu"), None, None),
            ExecEnvironment::Wsl2,
            "WSL_DISTRO_NAME → WSL2 default"
        );
    }

    #[test]
    fn wsl_runner_builds_windows_argv() {
        let r = WslRunner {
            wsl_binary: "wsl.exe".to_string(),
            distro: Some("Ubuntu".to_string()),
            use_wsl_exe: true,
        };
        let argv = r.build_argv("bash", &["-lc", "ls -la"]);
        assert_eq!(
            argv,
            vec!["wsl.exe", "-d", "Ubuntu", "-e", "bash", "-lc", "ls -la"]
        );
    }

    #[test]
    fn wsl_runner_builds_inside_wsl_argv() {
        let r = WslRunner {
            wsl_binary: "wsl".to_string(),
            distro: None,
            use_wsl_exe: false,
        };
        let argv = r.build_argv("python3", &["-V"]);
        assert_eq!(argv, vec!["wsl", "--", "python3", "-V"]);
    }

    #[test]
    fn wsl_runner_run_through_injected_runner() {
        // A scripted runner that records the argv it was given.
        struct RecordingRunner(std::sync::Mutex<Vec<String>>);
        impl CommandRunner for RecordingRunner {
            fn run(
                &self,
                command: &str,
                args: &[&str],
                _cwd: &str,
            ) -> Result<CommandOutput, String> {
                let mut v = vec![command.to_string()];
                v.extend(args.iter().map(|s| s.to_string()));
                self.0.lock().unwrap().extend(v);
                Ok(CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                })
            }
        }
        let rec = RecordingRunner(std::sync::Mutex::new(Vec::new()));
        let r = WslRunner {
            wsl_binary: "wsl.exe".to_string(),
            distro: Some("Ubuntu".to_string()),
            use_wsl_exe: true,
        };
        r.run_through(&rec, "git", &["status"], "/tmp").unwrap();
        let got = rec.0.lock().unwrap().clone();
        assert_eq!(got, vec!["wsl.exe", "-d", "Ubuntu", "-e", "git", "status"]);
    }

    #[test]
    fn frame_roundtrip_and_partial_decode() {
        let f1 = WslFrame::new(1, "fs.read", serde_json::json!({ "path": "/etc/hosts" }));
        let f2 = WslFrame::new(2, "fs.write", serde_json::json!({ "ok": true }));
        let mut wire = f1.encode();
        wire.extend_from_slice(&f2.encode());

        // Split the wire mid-frame: first read gets f1 + the first 2 bytes of f2.
        let (frames, rest) = WslFrame::decode_all(&wire[..wire.len() - 4]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id, 1);
        assert_eq!(frames[0].method, "fs.read");
        // Leftover + remaining bytes complete f2.
        let mut remainder = rest.to_vec();
        remainder.extend_from_slice(&wire[wire.len() - 4..]);
        let (frames2, rest2) = WslFrame::decode_all(&remainder);
        assert_eq!(frames2.len(), 1);
        assert_eq!(frames2[0].id, 2);
        assert_eq!(frames2[0].method, "fs.write");
        assert!(rest2.is_empty());
    }
}
