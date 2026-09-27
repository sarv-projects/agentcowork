//! P57.1 — path-based launch, and the environment a launched program gets.
//!
//! Two rules, one module, shared by all three backends so they cannot drift:
//!
//! 1. **Launch by canonical path, never by a shell string.** `LaunchApp` carries
//!    the executable the user allow-listed; a bare name is only a *fallback*
//!    after path resolution (the spec's rule), and it is resolved by looking in
//!    `PATH` — never by handing a string to `sh -c`. Nothing here interpolates
//!    user input into a shell.
//! 2. **Child-env sanitization (H5).** A launched program must not inherit the
//!    model credentials or vault material this process holds. The scrub is
//!    name-based and deliberately narrow: `PATH`, `HOME`, `DISPLAY`,
//!    `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`, `XDG_*`, `LANG` and the
//!    rest of the session environment must reach the app or GUI programs break.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::DesktopError;
use crate::types::ActKind;

/// P57.1 — reject a launch we cannot attribute to one exact program, **before**
/// any policy or platform work. A relative path is refused outright: it would
/// resolve against this process's working directory, so the executed file could
/// differ from the one the allow-list matched.
pub fn validate(act: &ActKind) -> Option<String> {
    let ActKind::LaunchApp { path, app } = act else {
        return None;
    };
    if let Some(p) = path {
        if p.trim().is_empty() {
            return Some("launch path is empty".into());
        }
        if !Path::new(p).is_absolute() {
            return Some(format!(
                "launch path must be absolute (got {p:?}) — a relative path could resolve to a different program"
            ));
        }
        return None;
    }
    if app.trim().is_empty() {
        return Some("launch needs a path or an app name".into());
    }
    None
}

/// Resolve what to execute.
///
/// * A path is canonicalized (so the executed file is the one the allow-list
///   stored) and must exist.
/// * A bare name is searched across `path_dirs` — the documented name-only
///   fallback. Not found ⇒ an honest error telling the caller to pass a path.
pub fn resolve_target(
    path: Option<&str>,
    app: &str,
    path_dirs: &[PathBuf],
) -> Result<PathBuf, DesktopError> {
    if let Some(p) = path {
        let p = Path::new(p);
        return std::fs::canonicalize(p)
            .map_err(|e| DesktopError::Platform(format!("cannot resolve {}: {e}", p.display())));
    }
    let name = app.trim();
    if name.is_empty() {
        return Err(DesktopError::Platform(
            "launch needs a path or an app name".into(),
        ));
    }
    for dir in path_dirs {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return std::fs::canonicalize(&candidate).or(Ok(candidate));
        }
    }
    Err(DesktopError::Platform(format!(
        "{name} is not on PATH — pass the program's full path"
    )))
}

/// The `PATH` an app name is resolved against (injected in tests).
pub fn path_dirs() -> Vec<PathBuf> {
    std::env::var("PATH")
        .map(|v| {
            v.split(if cfg!(windows) { ';' } else { ':' })
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Environment variable names a launched program never inherits: provider keys,
/// vault material, tokens and passwords, plus this app's own session secrets.
///
/// `SSH_AUTH_SOCK` and other session plumbing are intentionally **not** matched:
/// they are how the user's desktop works, not AgentCowork credentials.
pub fn is_secret_env_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    const PATTERNS: &[&str] = &[
        "API_KEY",
        "APIKEY",
        "API-TOKEN",
        "ACCESS_TOKEN",
        "AUTH_TOKEN",
        "BEARER",
        "CLIENT_SECRET",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "PASSWORD",
        "PASSWD",
        "SECRET",
        "SESSION_KEY",
        "_TOKEN",
        "TOKEN_",
    ];
    if PATTERNS.iter().any(|p| n.contains(p)) {
        return true;
    }
    // The app's own keys (`AGENTCOWORK_VAULT_KEY`, `AGENTCOWORK_HOME`, and the
    // retired `EVERYAIOS_*` spellings) are the product's session material, not
    // the launched program's business. The one exception is the
    // live-test/data-dir override, which some launched tools legitimately
    // read. Both eras stay scrubbed/blocked so a renamed credential never
    // passes a filter that only knew the old name (DEC-053).
    agentcowork_types::env_compat::is_host_control_name(&n)
        && n != "AGENTCOWORK_HOME"
        && n != "EVERYAIOS_HOME"
}

/// Filter a list of variable names down to the ones that must be scrubbed.
/// Pure, so the policy is testable without touching this process's environment.
pub fn secret_env_names_from<I: IntoIterator<Item = String>>(names: I) -> Vec<String> {
    names
        .into_iter()
        .filter(|n| is_secret_env_name(n))
        .collect()
}

/// Strip AgentCowork's credentials from a child command's environment.
pub fn scrub_child_env(cmd: &mut Command) {
    for name in secret_env_names_from(std::env::vars().map(|(k, _)| k)) {
        cmd.env_remove(name);
    }
}

/// P57.1 — prepare a launched program: no inherited stdio (a GUI app must not
/// hold the shell's pipes open) and no inherited credentials.
pub fn prepare_child(cmd: &mut Command) {
    use std::process::Stdio;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    scrub_child_env(cmd);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ea-launch-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn program(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        p
    }

    #[test]
    fn a_relative_path_is_refused_before_any_execution() {
        let relative = ActKind::launch_path("bin/foo");
        let err = validate(&relative).unwrap();
        assert!(err.contains("absolute"), "got: {err}");
        assert!(validate(&ActKind::launch_path("/usr/bin/foo")).is_none());
        assert!(validate(&ActKind::launch_by_name("firefox")).is_none());
        let empty = ActKind::LaunchApp {
            path: Some("  ".into()),
            app: String::new(),
        };
        assert!(validate(&empty).unwrap().contains("empty"));
        let nothing = ActKind::LaunchApp {
            path: None,
            app: "   ".into(),
        };
        assert!(validate(&nothing).unwrap().contains("path or an app name"));
        // Non-launch acts are not this function's business.
        assert!(validate(&ActKind::Click { x: 1, y: 2 }).is_none());
    }

    #[test]
    fn a_path_resolves_to_its_canonical_file() {
        let dir = tmp("resolve");
        let exe = program(&dir, "myeditor");
        let resolved = resolve_target(Some(exe.to_str().unwrap()), "", &[]).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&exe).unwrap());
        // A missing path is an error, not a silent name fallback.
        let err = resolve_target(Some("/nope/definitely-not-here"), "", &[]).unwrap_err();
        assert!(err.to_string().contains("cannot resolve"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bare_name_is_a_path_lookup_fallback_and_says_so_when_absent() {
        let dir = tmp("name");
        let exe = program(&dir, "myeditor");
        let resolved = resolve_target(None, "myeditor", std::slice::from_ref(&dir)).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&exe).unwrap());
        let err = resolve_target(None, "no-such-program", std::slice::from_ref(&dir)).unwrap_err();
        assert!(err.to_string().contains("not on PATH"), "got: {err}");
        assert!(resolve_target(None, "   ", &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_env_names_cover_credentials_and_never_the_session() {
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "DB_PASSWORD",
            "MY_CLIENT_SECRET",
            "TLS_PRIVATE_KEY",
            "AGENTCOWORK_VAULT_KEY",
            "AGENTCOWORK_VAULT_PASSPHRASE",
            "AGENTCOWORK_API_KEY",
            "AGENTCOWORK_ACP_TOKEN",
            // The retired spellings stay scrubbed too (DEC-053).
            "EVERYAIOS_VAULT_KEY",
            "EVERYAIOS_VAULT_PASSPHRASE",
            "EVERYAIOS_API_KEY",
            "EVERYAIOS_ACP_TOKEN",
        ] {
            assert!(is_secret_env_name(name), "{name} must be scrubbed");
        }
        for name in [
            "PATH",
            "HOME",
            "USER",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_DATA_DIRS",
            "LANG",
            "TMPDIR",
            "AGENTCOWORK_HOME",
            "EVERYAIOS_HOME",
            // Session plumbing, not an AgentCowork credential.
            "SSH_AUTH_SOCK",
            "PWD",
        ] {
            assert!(!is_secret_env_name(name), "{name} must reach the app");
        }
    }

    #[test]
    fn the_scrub_is_name_filtered_not_blanket() {
        let names = [
            "PATH",
            "DISPLAY",
            "ANTHROPIC_API_KEY",
            "AGENTCOWORK_VAULT_KEY",
            "AGENTCOWORK_HOME",
            "EVERYAIOS_VAULT_KEY",
            "EVERYAIOS_HOME",
            "GH_TOKEN",
        ]
        .map(String::from)
        .to_vec();
        let scrubbed = secret_env_names_from(names);
        assert_eq!(
            scrubbed,
            vec![
                "ANTHROPIC_API_KEY".to_string(),
                "AGENTCOWORK_VAULT_KEY".to_string(),
                "EVERYAIOS_VAULT_KEY".to_string(),
                "GH_TOKEN".to_string()
            ]
        );
        assert!(secret_env_names_from(Vec::new()).is_empty());
    }

    /// The environment mechanism end-to-end: a child launched through
    /// `scrub_child_env` sees the session environment and none of the credential
    /// names this process holds. (`Command` exposes no env getter, so the child
    /// reports it.)
    #[cfg(unix)]
    #[test]
    fn a_scrubbed_child_keeps_the_session_env_and_loses_the_credentials() {
        let mut cmd = Command::new("/usr/bin/env");
        scrub_child_env(&mut cmd);
        let out = cmd.output().expect("env must run");
        assert!(out.status.success());
        let child: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split('=').next().map(|s| s.to_string()))
            .collect();
        assert!(!child.is_empty());
        assert!(child.iter().any(|n| n == "PATH"), "PATH must survive");
        assert!(child.iter().any(|n| n == "HOME"), "HOME must survive");
        assert!(
            !child.iter().any(|n| is_secret_env_name(n)),
            "no credential name may reach the child: {:?}",
            child
                .iter()
                .filter(|n| is_secret_env_name(n))
                .collect::<Vec<_>>()
        );
    }

    /// P57.1 — a launched program does not hold this process's stdio open (an
    /// inherited pipe is how a GUI app keeps a shell wrapped around itself).
    #[cfg(unix)]
    #[test]
    fn prepare_child_detaches_stdio() {
        let mut prepared = Command::new("/bin/sh");
        prepared.arg("-c").arg("echo leaked");
        prepare_child(&mut prepared);
        let out = prepared.output().expect("sh must run");
        assert!(out.status.success(), "the program still runs");
        assert!(out.stdout.is_empty(), "stdout must not be captured/held");
        assert!(out.stderr.is_empty());
    }
}
