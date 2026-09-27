//! P63.11 — the loopback MCP lease passed into ACP `session/new`.
//!
//! The lease exposes the shared façades. Tool calls go to the kernel
//! `ToolService` when the chat relay has published one. The lease does not
//! authorize anything by itself.

use std::sync::{Arc, Mutex};

use agentcowork_acp::McpServer;
use agentcowork_core::ToolService;
use agentcowork_mcp::{McpHttpLease, ToolCallHandler};
use serde_json::{json, Value};

/// Tools the Channel B handler may call. Empty until the relay is live.
#[derive(Clone, Default)]
pub struct SharedTools(pub Arc<Mutex<Option<Arc<Mutex<ToolService>>>>>);

struct FacadeHandler(SharedTools);

impl ToolCallHandler for FacadeHandler {
    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let tools = self
            .0
             .0
            .lock()
            .map_err(|err| err.to_string())?
            .clone()
            .ok_or_else(|| "channel B tool service is not attached".to_string())?;
        let mut tools = tools.lock().map_err(|err| err.to_string())?;
        tools.handle(
            "tool/exec",
            &json!({
                "toolId": name,
                "args": arguments,
                "sessionId": "channel-b",
                "agentId": "external",
            }),
        )
    }
}

/// A process-lived loopback server plus the ACP descriptor built from it.
pub struct ChannelBSlot {
    lease: McpHttpLease<FacadeHandler>,
    server: McpServer,
}

impl ChannelBSlot {
    pub fn start(tools: SharedTools) -> Result<Self, String> {
        let lease = McpHttpLease::start(FacadeHandler(tools))
            .map_err(|err| format!("channel B lease failed: {err}"))?;
        let server = McpServer::http_with_lease_values("agentcowork", lease.url(), lease.token())
            .map_err(|err| format!("channel B descriptor failed: {err}"))?;
        Ok(Self { lease, server })
    }

    pub fn url(&self) -> &str {
        self.lease.url()
    }

    pub fn servers(&self) -> Vec<McpServer> {
        vec![self.server.clone()]
    }
}

/// Publish the relay's tool service into the handler shared with the lease.
pub fn publish_tools(tools: &SharedTools, service: Arc<Mutex<ToolService>>) {
    if let Ok(mut guard) = tools.0.lock() {
        *guard = Some(service);
    }
}

/// Ensure one lease exists and return the server list for `session/new`.
pub fn ensure_servers(
    slot: &Mutex<Option<ChannelBSlot>>,
    tools: &SharedTools,
) -> Result<Vec<McpServer>, String> {
    let mut guard = slot.lock().map_err(|err| err.to_string())?;
    if guard.is_none() {
        *guard = Some(ChannelBSlot::start(tools.clone())?);
    }
    Ok(guard.as_ref().expect("just inserted").servers())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_started_lease_is_loopback_and_not_an_empty_server_list() {
        let slot = ChannelBSlot::start(SharedTools::default()).unwrap();
        assert!(slot.url().starts_with("http://127.0.0.1:"));
        let servers = slot.servers();
        assert_eq!(servers.len(), 1);
        assert!(servers[0].is_http());
    }
}
