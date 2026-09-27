//! P51.30 — protected own-settings paths and recursive-rm criticality.
//!
//! The agent's own settings must never be removed by a recursive delete:
//! [`is_protected`] recognizes them and [`rm_critical`] treats an `rm`-like
//! recursive command aimed at `/`, `~`, `.`, a workspace-root marker,
//! `.git/`, or a protected path as critical (fail-closed).

/// Own-settings file/dir names. Any canonical path containing one of these
/// is protected.
pub static PROTECTED_PREFIXES: &[&str] = &[
    "config.toml",
    "risk_overrides.json",
    "workspace_trust.json",
    "permissions.toml",
    // Both data-home eras (DEC-053): the current `~/.agentcowork` and the
    // retired `~/.everyaios` a pre-rename install still uses.
    ".agentcowork/",
    ".everyaios/",
    // P62.3 — other agents' *configuration* surfaces. CVE-2025-53773 was a
    // prompt injection that made an agent rewrite its own config to enable
    // auto-approve and then execute; the equivalent for us is an agent
    // editing another harness's settings (or the ones our ClientCompatibility
    // ring will project into). Only the settings are protected — a project's
    // instruction files (`CLAUDE.md`, `AGENTS.md`) stay normal workspace files
    // the user and agent may legitimately edit.
    ".claude/",
    ".codex/",
    ".gemini/",
    ".cursor/",
    ".windsurf/",
    ".cline/",
    ".roo/",
    ".continue/",
    ".openclaw/",
    ".aider/",
    ".mcp.json",
    // Secret and ambient credential files (CVE-2026-47211 protection).
    ".env",
    ".env.local",
    ".env.production",
    ".env.staging",
    ".ssh/",
    ".aws/",
    "id_rsa",
    "id_ed25519",
    "service_account.json",
];

/// Is this canonical path one of our own settings files/dirs?
/// Substring match (fail-closed): absolute project paths like
/// `/home/u/.agentcowork/permissions.toml` (or the retired
/// `/home/u/.everyaios/permissions.toml`) still trip.
pub fn is_protected(canonical_path: &str) -> bool {
    let normalized = canonical_path.replace('\\', "/");
    PROTECTED_PREFIXES.iter().any(|p| normalized.contains(p))
}

/// Workspace-root markers: any of these as an `rm` target is critical.
fn is_workspace_root_marker(target: &str) -> bool {
    matches!(
        target.to_lowercase().as_str(),
        "$workspace"
            | "${workspace}"
            | "$workspace_root"
            | "${workspace_root}"
            | "<workspace-root>"
            | "<workspace_root>"
            | "<workspace>"
            | "/workspace"
            | "workspace"
    )
}

fn is_rm_like_recursive(command: &str) -> bool {
    let lower = command.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let has_rm = tokens.iter().any(|t| {
        let bare = t.trim_matches(|c| c == '\'' || c == '"' || c == ';' || c == ',');
        bare == "rm" || bare == "rmdir" || bare.ends_with("/rm") || bare.ends_with("/rmdir")
    });
    if !has_rm {
        return false;
    }
    tokens.iter().any(|t| {
        if *t == "--recursive" || t.starts_with("--recursive=") {
            return true;
        }
        if t.starts_with('-') && !t.starts_with("--") && t.len() > 1 {
            return t[1..].chars().any(|c| c == 'r' || c == 'R');
        }
        false
    })
}

fn is_critical_target(target: &str) -> bool {
    let trimmed = target.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    if trimmed.is_empty() {
        return false;
    }
    let normalized = trimmed.replace('\\', "/");
    // `/`, `~`, `.` families.
    if normalized == "/" || normalized == "/*" {
        return true;
    }
    if matches!(normalized.as_str(), "~" | "~/" | "$HOME" | "${HOME}") {
        return true;
    }
    if normalized == "." || normalized == "./" {
        return true;
    }
    if is_workspace_root_marker(&normalized) {
        return true;
    }
    // `.git/` destruction.
    if normalized.contains(".git") {
        return true;
    }
    // Own settings.
    if is_protected(&normalized) {
        return true;
    }
    false
}

/// True when `command` is an `rm`-like recursive invocation AND any entry
/// of `targets` is `/`, `~`, `.`, a workspace-root marker, `.git/`, or a
/// protected settings path.
pub fn rm_critical(command: &str, targets: &[&str]) -> bool {
    if !is_rm_like_recursive(command) {
        return false;
    }
    targets.iter().any(|t| is_critical_target(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rm_rf_root_is_critical() {
        assert!(rm_critical("rm -rf", &["/"]));
        assert!(rm_critical("sudo rm -rf /", &["/"]));
        assert!(!rm_critical("rm /tmp/foo", &["/tmp/foo"]));
    }

    #[test]
    fn rm_protected_settings_is_critical() {
        assert!(rm_critical(
            "rm -rf",
            &["/home/user/.everyaios/permissions.toml"]
        ));
        assert!(rm_critical("rm -rf /tmp/x", &[".github/workflows/../.."]));
        // Direct protected target (config file) is critical under rm -rf.
        assert!(rm_critical("rm -rf", &["config.toml"]));
    }

    #[test]
    fn plain_ls_not_critical() {
        assert!(!rm_critical("ls -la", &["/tmp"]));
        // Even `ls /` is not an rm-critical action.
        assert!(!rm_critical("ls", &["/"]));
    }

    #[test]
    fn is_protected_matches_settings_files() {
        assert!(is_protected("/home/user/.agentcowork/config.toml"));
        // The retired data-home spelling stays protected too (DEC-053).
        assert!(is_protected("/home/user/.everyaios/config.toml"));
        assert!(is_protected("permissions.toml"));
        assert!(is_protected("/proj/.everyaios/risk_overrides.json"));
        assert!(is_protected("/proj/.agentcowork/risk_overrides.json"));
        assert!(is_protected("workspace_trust.json"));
        assert!(!is_protected("/tmp/foo.txt"));
        assert!(!is_protected("/workspace/src/main.rs"));
    }

    /// P62.3 — an agent may not silently rewrite another harness's settings
    /// (the CVE-2025-53773 class), nor our own projected config.
    #[test]
    fn other_harness_settings_are_protected() {
        for p in [
            "/home/u/.claude/settings.local.json",
            "/home/u/.claude/settings.json",
            "/proj/.codex/config.toml",
            "/proj/.gemini/settings.json",
            "/proj/.cursor/mcp.json",
            "/proj/.openclaw/config.json",
            "/proj/.mcp.json",
        ] {
            assert!(is_protected(p), "{p} must be protected");
            assert!(rm_critical("rm -rf", &[p]), "{p} must be rm-critical");
        }
    }

    /// Secret and ambient credential files must be protected from destructive deletion.
    #[test]
    fn secret_and_credential_files_are_protected() {
        for p in [
            "/proj/.env",
            "/proj/.env.local",
            "/proj/.env.production",
            "/home/u/.ssh/id_rsa",
            "/home/u/.ssh/id_ed25519",
            "/home/u/.aws/credentials",
            "/proj/service_account.json",
        ] {
            assert!(is_protected(p), "{p} must be protected");
            assert!(rm_critical("rm -rf", &[p]), "{p} must be rm-critical");
        }
    }

    /// Instruction files stay ordinary workspace files: the agent and the user
    /// legitimately edit them, so protecting them would break normal work.
    #[test]
    fn instruction_files_are_not_over_protected() {
        for p in [
            "/proj/CLAUDE.md",
            "/proj/AGENTS.md",
            "/proj/.github/workflows/ci.yml",
            "/proj/claude.md",
        ] {
            assert!(!is_protected(p), "{p} should stay editable");
        }
    }
}
