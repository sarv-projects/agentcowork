//! WebMCP support (E16 — doc 63 §4.2, chrome-devtools-mcp `webmcp.ts`
//! pattern): web-native MCP so browser sessions can **serve** and **consume**
//! MCP tools over HTTP. A page exposes tools (name + description + JSON
//! schema); the harness lists them (`list_webmcp_tools`) and executes them
//! (`execute_webmcp_tool`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// A WebMCP tool exposed by a page/browser session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebMcpTool {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's input.
    #[serde(default)]
    pub input_schema: Value,
}

/// The result of executing a WebMCP tool (matches `webmcp.ts` `{status,
/// output, errorText}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebMcpResult {
    /// `"ok"` or `"error"`.
    pub status: String,
    pub output: Value,
    pub error_text: Option<String>,
}

impl WebMcpResult {
    pub fn ok(output: Value) -> Self {
        Self {
            status: "ok".into(),
            output,
            error_text: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            status: "error".into(),
            output: Value::Null,
            error_text: Some(message.into()),
        }
    }
}

#[derive(Debug, Error)]
pub enum WebMcpError {
    #[error("WebMCP tool {0:?} not found")]
    UnknownTool(String),
    #[error("invalid WebMCP input JSON: {0}")]
    InvalidInputJson(String),
    /// E16 tool-cancellation: the caller aborted the invocation (-32001).
    /// Clients must **not** retry a cancelled invocation — it is a caller
    /// decision, not a failure of the tool.
    #[error("WebMCP invocation cancelled: {0}")]
    Cancelled(String),
    /// E16 tool-cancellation: the tool exceeded its deadline (-32002). This is
    /// a tool-state outcome (the tool may retry or the client may escalate),
    /// distinct from a caller abort.
    #[error("WebMCP invocation timed out: {0}")]
    TimedOut(String),
}

impl WebMcpError {
    /// The JSON-RPC error code (lightpanda `mcp/protocol.zig` taxonomy):
    /// `-32001` = Cancelled (caller aborted), `-32002` = Timeout (deadline
    /// exceeded). The distinction tells the client whether a retry is safe.
    pub fn code(&self) -> i64 {
        match self {
            WebMcpError::Cancelled(_) => -32001,
            WebMcpError::TimedOut(_) => -32002,
            WebMcpError::UnknownTool(_) => -32602,
            WebMcpError::InvalidInputJson(_) => -32602,
        }
    }

    /// Is a retry appropriate? Cancelled = no (caller aborted); everything
    /// else = the client may choose to retry.
    pub fn retryable(&self) -> bool {
        !matches!(self, WebMcpError::Cancelled(_))
    }
}

/// Executes a WebMCP tool. The page supplies this (JS-side); the harness calls
/// through it so the tool's actual behavior stays behind the registry.
pub trait WebMcpExecutor {
    fn execute(&self, tool: &WebMcpTool, input: Value) -> WebMcpResult;
}

/// The registry of WebMCP tools a session exposes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebMcpRegistry {
    tools: Vec<WebMcpTool>,
}

impl WebMcpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: WebMcpTool) {
        self.tools.push(tool);
    }

    pub fn tools(&self) -> &[WebMcpTool] {
        &self.tools
    }

    pub fn find(&self, name: &str) -> Option<&WebMcpTool> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// `list_webmcp_tools` — the read-only handshake step.
    pub fn list(&self) -> Vec<&WebMcpTool> {
        self.tools.iter().collect()
    }

    /// `execute_webmcp_tool` — parse the JSON input, find the tool, execute.
    /// `input` is the JSON-stringified parameters (per `webmcp.ts`).
    pub fn execute<E: WebMcpExecutor + ?Sized>(
        &self,
        tool_name: &str,
        input: &str,
        executor: &E,
    ) -> Result<WebMcpResult, WebMcpError> {
        let tool = self
            .find(tool_name)
            .ok_or_else(|| WebMcpError::UnknownTool(tool_name.to_string()))?;
        let parsed: Value = if input.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(input).map_err(|e| WebMcpError::InvalidInputJson(e.to_string()))?
        };
        if !parsed.is_object() {
            return Err(WebMcpError::InvalidInputJson(
                "parsed input is not an object".into(),
            ));
        }
        Ok(executor.execute(tool, parsed))
    }
}

/// The lifecycle state of one in-flight WebMCP invocation (E16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvocationState {
    Running,
    Cancelled,
    TimedOut,
    Done,
}

/// Tracks in-flight WebMCP invocations so `cancelInvocation` can abort a
/// long-running page tool (lightpanda `invokeTool`/`cancelInvocation`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InvocationTracker {
    invocations: std::collections::HashMap<String, InvocationState>,
}

impl InvocationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a running invocation.
    pub fn start(&mut self, id: &str) {
        self.invocations
            .insert(id.to_string(), InvocationState::Running);
    }

    /// `cancelInvocation` — mark an invocation cancelled. Returns true when a
    /// running invocation was cancelled (false = unknown or already settled).
    pub fn cancel(&mut self, id: &str) -> bool {
        match self.invocations.get(id) {
            Some(InvocationState::Running) => {
                self.invocations
                    .insert(id.to_string(), InvocationState::Cancelled);
                true
            }
            _ => false,
        }
    }

    /// Mark an invocation as having exceeded its deadline.
    pub fn timeout(&mut self, id: &str) -> bool {
        match self.invocations.get(id) {
            Some(InvocationState::Running) => {
                self.invocations
                    .insert(id.to_string(), InvocationState::TimedOut);
                true
            }
            _ => false,
        }
    }

    /// Mark an invocation as finished normally.
    pub fn finish(&mut self, id: &str) {
        self.invocations
            .insert(id.to_string(), InvocationState::Done);
    }

    /// The current state (None = never started).
    pub fn state(&self, id: &str) -> Option<InvocationState> {
        self.invocations.get(id).copied()
    }

    pub fn len(&self) -> usize {
        self.invocations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.invocations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoExecutor;
    impl WebMcpExecutor for EchoExecutor {
        fn execute(&self, tool: &WebMcpTool, input: Value) -> WebMcpResult {
            WebMcpResult::ok(json!({"tool": tool.name, "input": input}))
        }
    }

    fn registry() -> WebMcpRegistry {
        let mut r = WebMcpRegistry::new();
        r.register(WebMcpTool {
            name: "search".into(),
            description: "search the page".into(),
            input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        });
        r
    }

    #[test]
    fn list_returns_registered_tools() {
        let r = registry();
        assert_eq!(r.list().len(), 1);
        assert_eq!(r.list()[0].name, "search");
    }

    #[test]
    fn execute_parses_input_and_runs_executor() {
        let r = registry();
        let res = r
            .execute("search", r#"{"q": "hello"}"#, &EchoExecutor)
            .unwrap();
        assert_eq!(res.status, "ok");
        assert_eq!(res.output["tool"], "search");
        assert_eq!(res.output["input"]["q"], "hello");
    }

    #[test]
    fn execute_empty_input_is_empty_object() {
        let r = registry();
        let res = r.execute("search", "", &EchoExecutor).unwrap();
        assert_eq!(res.output["input"], json!({}));
    }

    #[test]
    fn unknown_tool_errors() {
        let r = registry();
        assert!(matches!(
            r.execute("ghost", "{}", &EchoExecutor),
            Err(WebMcpError::UnknownTool(_))
        ));
    }

    #[test]
    fn invalid_json_errors() {
        let r = registry();
        assert!(matches!(
            r.execute("search", "not json", &EchoExecutor),
            Err(WebMcpError::InvalidInputJson(_))
        ));
    }

    #[test]
    fn non_object_input_errors() {
        let r = registry();
        assert!(matches!(
            r.execute("search", "[1,2,3]", &EchoExecutor),
            Err(WebMcpError::InvalidInputJson(_))
        ));
    }

    #[test]
    fn invocation_lifecycle() {
        let mut t = InvocationTracker::new();
        t.start("i1");
        assert_eq!(t.state("i1"), Some(InvocationState::Running));
        assert!(t.cancel("i1"));
        assert_eq!(t.state("i1"), Some(InvocationState::Cancelled));
        // Cancelling twice is not an error (idempotent), but reports false.
        assert!(!t.cancel("i1"));
    }

    #[test]
    fn cancel_after_finish_is_noop() {
        let mut t = InvocationTracker::new();
        t.start("i1");
        t.finish("i1");
        assert!(!t.cancel("i1"));
        assert_eq!(t.state("i1"), Some(InvocationState::Done));
    }

    #[test]
    fn timeout_is_distinct_from_cancel() {
        let mut t = InvocationTracker::new();
        t.start("i1");
        assert!(t.timeout("i1"));
        assert_eq!(t.state("i1"), Some(InvocationState::TimedOut));
    }

    #[test]
    fn error_taxonomy_cancelled_vs_timeout() {
        let c = WebMcpError::Cancelled("user aborted".into());
        let t = WebMcpError::TimedOut("deadline".into());
        assert_eq!(c.code(), -32001);
        assert_eq!(t.code(), -32002);
        // Cancelled must NOT be retried (caller decision); timeout may be.
        assert!(!c.retryable());
        assert!(t.retryable());
    }

    #[test]
    fn tracker_serializes() {
        let mut t = InvocationTracker::new();
        t.start("a");
        let json = serde_json::to_string(&t).unwrap();
        let back: InvocationTracker = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state("a"), Some(InvocationState::Running));
    }
}
