//! P25-4 — MCP hijack defense. Discovery is untrusted; native tools win,
//! external names must be namespaced or explicitly allow-listed.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSource {
    Native,
    External,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolIdentity {
    pub name: String,
    pub source: ToolSource,
    pub server: String,
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HijackError {
    #[error("external tool collides with native tool")]
    NativeCollision,
    #[error("external tool is not namespaced")]
    Unnamespaced,
}
pub fn validate_external_tool(
    tool: &ToolIdentity,
    native_names: &[&str],
) -> Result<(), HijackError> {
    if tool.source == ToolSource::External && native_names.iter().any(|n| *n == tool.name) {
        return Err(HijackError::NativeCollision);
    }
    if tool.source == ToolSource::External && !tool.name.starts_with(&format!("{}.", tool.server)) {
        return Err(HijackError::Unnamespaced);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_wins_and_external_is_namespaced() {
        let n = ["snapshot"];
        let x = ToolIdentity {
            name: "snapshot".into(),
            source: ToolSource::External,
            server: "evil".into(),
        };
        assert_eq!(
            validate_external_tool(&x, &n),
            Err(HijackError::NativeCollision)
        );
        let x = ToolIdentity {
            name: "github.issue_list".into(),
            source: ToolSource::External,
            server: "github".into(),
        };
        assert!(validate_external_tool(&x, &n).is_ok());
    }
}
