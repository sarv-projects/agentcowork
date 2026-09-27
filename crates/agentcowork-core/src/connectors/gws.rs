//! P26-2 — Google Workspace `gws` managed-child adapter.
//!
//! The official `googleworkspace/cli` remains an optional user-installed
//! binary. This module owns argv construction and read-first policy; process
//! execution, OAuth, and vault handles remain injected runtime seams.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GwsAction {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GwsRequest {
    pub service: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub action: GwsAction,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GwsError {
    #[error("gws writes require Guard-2 approval")]
    ApprovalRequired,
    #[error("invalid gws service or method")]
    InvalidRoute,
    #[error("gws binary is not installed")]
    BinaryUnavailable,
}

#[derive(Debug, Clone, Default)]
pub struct GwsConnector {
    pub binary: Option<std::path::PathBuf>,
}
impl GwsConnector {
    pub fn new(binary: Option<std::path::PathBuf>) -> Self {
        Self { binary }
    }
    pub fn command(
        &self,
        r: &GwsRequest,
        approved: bool,
    ) -> Result<std::process::Command, GwsError> {
        if r.service.trim().is_empty() || r.method.trim().is_empty() {
            return Err(GwsError::InvalidRoute);
        }
        if r.action == GwsAction::Write && !approved {
            return Err(GwsError::ApprovalRequired);
        }
        let bin = self.binary.clone().ok_or(GwsError::BinaryUnavailable)?;
        let mut c = std::process::Command::new(bin);
        c.args([&r.service, &r.method, "--format", "json"]);
        if !r.params.is_null() {
            c.arg("--params").arg(r.params.to_string());
        }
        Ok(c)
    }
    pub fn read_request(service: &str, method: &str, params: serde_json::Value) -> GwsRequest {
        GwsRequest {
            service: service.into(),
            method: method.into(),
            params,
            action: GwsAction::Read,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_is_constructed_without_approval() {
        let c = GwsConnector::new(Some("gws".into()));
        let r =
            GwsConnector::read_request("drive", "files.list", serde_json::json!({"pageSize":10}));
        assert!(c.command(&r, false).is_ok());
    }
    #[test]
    fn writes_fail_closed() {
        let c = GwsConnector::new(Some("gws".into()));
        let r = GwsRequest {
            service: "gmail".into(),
            method: "users.messages.send".into(),
            params: serde_json::Value::Null,
            action: GwsAction::Write,
        };
        assert!(matches!(
            c.command(&r, false),
            Err(GwsError::ApprovalRequired)
        ));
        assert!(c.command(&r, true).is_ok());
    }
}
