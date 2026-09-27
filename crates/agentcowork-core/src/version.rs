//! Version constants — single source of truth for the binary.

/// Semantic version, kept in sync with the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Human-readable boot banner used by `agentcowork-core --version` and tray status.
pub fn banner() -> String {
    format!("agentcowork-core {VERSION} (AgentCowork) — agentic desktop core")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_semver() {
        assert!(!VERSION.is_empty());
        assert!(
            VERSION.split('.').count() >= 2,
            "expected semver, got {VERSION}"
        );
    }

    #[test]
    fn banner_contains_version() {
        assert!(banner().contains(VERSION));
    }
}
