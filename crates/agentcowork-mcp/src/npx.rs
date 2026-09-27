//! P51.17 — OpenCowork npx resolution: system-over-bundled, trusted-list
//! package specs, bundled-node PATH enrichment.
//!
//! Fetched OpenCoworkAI/open-cowork (prefer system npx; bundled Node on PATH
//! so shims find `node`). Arbitrary `npx -c` / path-traversal package names
//! are refused — the spawn layer never rewrites a hostile name.

use std::path::{Path, PathBuf};

/// Where the launcher binary came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpxSource {
    System,
    Bundled,
}

/// A spawn-ready command line after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLaunch {
    pub command: String,
    pub args: Vec<String>,
    /// PATH to set on the child (`bundled` prepended when present).
    pub path: String,
    pub source: NpxSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NpxError {
    UntrustedPackage(String),
    MissingLauncher(String),
    ShellEscape(String),
}

impl std::fmt::Display for NpxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NpxError::UntrustedPackage(p) => {
                write!(f, "npx package `{p}` is not on the trusted list")
            }
            NpxError::MissingLauncher(c) => write!(f, "launcher `{c}` not found on PATH"),
            NpxError::ShellEscape(c) => {
                write!(f, "refusing shell-form MCP launch `{c}`")
            }
        }
    }
}

impl std::error::Error for NpxError {}

/// Package specs npx/uvx may fetch. Scoped name, optional `@version` / `@tag`.
/// No paths, no `..`, no shell metacharacters.
pub fn trusted_npx_package(spec: &str) -> bool {
    let s = spec.trim();
    if s.is_empty() || s.len() > 214 {
        return false;
    }
    // `@scope/pkg` has exactly one slash; path traversal and Windows
    // separators stay refused.
    if s.starts_with('.')
        || s.starts_with('-')
        || s.starts_with('_')
        || s.contains("..")
        || s.contains('\\')
    {
        return false;
    }
    if !s.starts_with('@') && s.contains('/') {
        return false;
    }
    if s.chars()
        .any(|c| matches!(c, ';' | '|' | '&' | '`' | '$' | '\n' | ' '))
    {
        return false;
    }
    let body = s.strip_prefix('@').unwrap_or(s);
    let (name, ver) = match body.split_once('@') {
        Some((n, v)) => (n, Some(v)),
        None => (body, None),
    };
    if name.is_empty() {
        return false;
    }
    let name_ok = if s.starts_with('@') {
        // @scope/pkg
        let mut parts = name.split('/');
        let scope = parts.next().unwrap_or("");
        let pkg = parts.next().unwrap_or("");
        parts.next().is_none() && slug(scope) && slug(pkg)
    } else {
        slug(name)
    };
    let ver_ok = ver.map(slug).unwrap_or(true);
    name_ok && ver_ok
}

fn slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn is_npx_family(command: &str) -> bool {
    matches!(
        command.rsplit(['/', '\\']).next().unwrap_or(command),
        "npx" | "npx.cmd" | "npx.exe" | "uvx" | "uvx.exe"
    )
}

fn is_shell_launcher(command: &str) -> bool {
    matches!(
        command.rsplit(['/', '\\']).next().unwrap_or(command),
        "sh" | "bash" | "zsh" | "dash" | "cmd" | "cmd.exe" | "powershell" | "pwsh"
    )
}

/// First non-flag argument is the package. `-y`/`--yes`/`--offline` allowed.
pub fn npx_package_from_args(args: &[&str]) -> Result<String, NpxError> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "-y" || a == "--yes" || a == "--offline" || a == "--no-install" {
            i += 1;
            continue;
        }
        if a == "-c" || a == "--call" || a == "-p" || a == "--package" || a.starts_with('-') {
            return Err(NpxError::UntrustedPackage(a.to_string()));
        }
        if !trusted_npx_package(a) {
            return Err(NpxError::UntrustedPackage(a.to_string()));
        }
        return Ok(a.to_string());
    }
    Err(NpxError::UntrustedPackage("(missing package)".into()))
}

fn find_on_path(name: &str, path: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn enrich_path(system_path: &str, bundled_node: Option<&Path>) -> String {
    match bundled_node {
        Some(dir) if dir.is_dir() => {
            if std::env::split_paths(system_path).any(|p| p == dir) {
                return system_path.to_string();
            }
            let mut parts = vec![dir.to_path_buf()];
            parts.extend(std::env::split_paths(system_path));
            std::env::join_paths(parts)
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| system_path.to_string())
        }
        _ => system_path.to_string(),
    }
}

/// Resolve `npx`/`uvx` (or a plain binary): system PATH wins, then bundled
/// node dir (`AGENTCOWORK_BUNDLED_NODE`, retired `EVERYAIOS_BUNDLED_NODE`
/// honored). Package args are trusted-list gated.
pub fn resolve_stdio_launch_with(
    command: &str,
    args: &[&str],
    system_path: &str,
    bundled_node: Option<&Path>,
) -> Result<ResolvedLaunch, NpxError> {
    if is_shell_launcher(command) {
        return Err(NpxError::ShellEscape(command.to_string()));
    }
    if is_npx_family(command) {
        let _pkg = npx_package_from_args(args)?;
    }
    let basename = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let path = enrich_path(system_path, bundled_node);
    if let Some(sys) = find_on_path(basename, system_path) {
        return Ok(ResolvedLaunch {
            command: sys.to_string_lossy().into_owned(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            path,
            source: NpxSource::System,
        });
    }
    if let Some(dir) = bundled_node {
        let bundled = dir.join(basename);
        if bundled.is_file() {
            return Ok(ResolvedLaunch {
                command: bundled.to_string_lossy().into_owned(),
                args: args.iter().map(|s| (*s).to_string()).collect(),
                path,
                source: NpxSource::Bundled,
            });
        }
    }
    // Absolute / relative command the user already approved — keep as-is.
    if command.contains('/') || command.contains('\\') {
        return Ok(ResolvedLaunch {
            command: command.to_string(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            path,
            source: NpxSource::System,
        });
    }
    Err(NpxError::MissingLauncher(command.to_string()))
}

pub fn bundled_node_dir() -> Option<PathBuf> {
    agentcowork_types::env_compat::get_os("BUNDLED_NODE").map(PathBuf::from)
}

pub fn resolve_stdio_launch(command: &str, args: &[&str]) -> Result<ResolvedLaunch, NpxError> {
    let path = std::env::var("PATH").unwrap_or_default();
    resolve_stdio_launch_with(command, args, &path, bundled_node_dir().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn trusted_packages_accept_scoped_and_pinned() {
        assert!(trusted_npx_package("@modelcontextprotocol/server-github"));
        assert!(trusted_npx_package(
            "@modelcontextprotocol/server-github@1.2.3"
        ));
        assert!(trusted_npx_package("prettier@3.0.0"));
        assert!(!trusted_npx_package("../evil"));
        assert!(!trusted_npx_package("foo;rm"));
        assert!(!trusted_npx_package("-c"));
        assert!(!trusted_npx_package(""));
    }

    #[test]
    fn npx_args_refuse_call_and_missing_package() {
        assert!(npx_package_from_args(&["-y", "@org/pkg"]).is_ok());
        assert!(npx_package_from_args(&["-c", "rm -rf /"]).is_err());
        assert!(npx_package_from_args(&["--call", "x"]).is_err());
        assert!(npx_package_from_args(&[]).is_err());
    }

    #[test]
    fn system_npx_wins_over_bundled() {
        let tmp = std::env::temp_dir().join(format!("agentcowork-npx-{}", std::process::id()));
        let sys = tmp.join("sys");
        let bun = tmp.join("bun");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&sys).unwrap();
        fs::create_dir_all(&bun).unwrap();
        let sys_npx = sys.join("npx");
        let bun_npx = bun.join("npx");
        fs::write(&sys_npx, b"sys").unwrap();
        fs::write(&bun_npx, b"bun").unwrap();
        let got = resolve_stdio_launch_with(
            "npx",
            &["-y", "@org/mcp"],
            sys.to_str().unwrap(),
            Some(&bun),
        )
        .unwrap();
        assert_eq!(got.source, NpxSource::System);
        assert_eq!(got.command, sys_npx.to_string_lossy());
        assert!(got.path.contains(&bun.to_string_lossy().to_string()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn bundled_used_when_system_missing() {
        let tmp = std::env::temp_dir().join(format!("agentcowork-npx-b-{}", std::process::id()));
        let bun = tmp.join("bun");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&bun).unwrap();
        let bun_npx = bun.join("npx");
        fs::write(&bun_npx, b"bun").unwrap();
        let got =
            resolve_stdio_launch_with("npx", &["@org/mcp"], "/no/such/bin", Some(&bun)).unwrap();
        assert_eq!(got.source, NpxSource::Bundled);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn shell_launchers_are_refused() {
        assert!(matches!(
            resolve_stdio_launch_with("bash", &["-c", "x"], "/bin", None),
            Err(NpxError::ShellEscape(_))
        ));
    }
}
