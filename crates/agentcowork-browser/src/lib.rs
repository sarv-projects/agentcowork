//! agentcowork-browser — accessibility-tree snapshot engine + action layer
//! (ARCH/08, E3; doc 63 §2.1 E15–E17).
//!
//! - P2.2: CDP Accessibility-domain capture (`capture::SnapshotEngine`),
//!   interactive-mode pruning (~90% token cut, E16 slim), stable refs
//!   `[ref=eN]` scoped to (document_id, url) (`tree`), iframe stitching
//!   inline and URL-change short-circuit (`diff`).
//! - P2.3: the action engine (`actions`) — click/fill/read/hover/etc over the
//!   CDP stack; `protocol` lowers provider action protocols (native / Anthropic
//!   CUA / OpenAI CUA / UI-TARS) to canonical parsed actions (E17).
//! - E15: `electron` — attach to a running Electron app (VS Code/Slack/…)
//!   by debug port, connect to its first page target, snapshot/click/fill/read/
//!   screenshot (agent-browser pattern).
//! - WebMCP (E16): `webmcp` handshake + `webmcp_http` — an HTTP transport
//!   (std `TcpListener`) serving `tools/list` + `tools/call` over JSON-RPC.
//! - Session/vault/replay glue: `session`, `ownership` (tab ownership),
//!   `replay` (injected recorder), `read` (page text), `humanize`, `tiers`.

pub mod acquisition;
pub mod actions;
pub mod ax;
pub mod capture;
pub mod content;
pub mod demo;
pub mod diagnostics;
pub mod diff;
pub mod electron;
pub mod har;
pub mod humanize;
pub mod locator;
pub mod ownership;
pub mod protocol;
pub mod read;
pub mod replay;
pub mod resources;
pub mod selector;
pub mod session;
pub mod tiers;
pub mod tree;
pub mod webmcp;
pub mod webmcp_http;

#[cfg(test)]
mod live_tests;

pub use actions::{
    ActKind, ActResult, AnnotatedScreenshot, BrowserActions, EnhancedSnapshot, FieldValue,
    NavigateAction, Point, ReadMode, ScreenshotLabel, ScrollDirection, TextResult, WaitFor,
    WaitOutcome, find_ref,
};
pub use ax::{AxNode, INTERACTIVE_ROLES};
pub use capture::{CdpSession, MAX_FRAME_DEPTH, SnapshotEngine};
pub use content::{CleanedText, FilterSet, RuleKind, clean_markdown};
pub use demo::{
    DemoAnchor, DemoInput, DemoRecording, DemoStep, OutcomeEvidence, anchor_from_node,
    outcome_holds,
};
pub use diff::{diff_snapshots, snapshot_lines};
pub use electron::ElectronHandle;
pub use locator::{
    A11yIssue, A11ySeverity, BatchParseError, Located, RefRegistry, SemanticQuery, a11y_audit,
    find_first, find_semantic, first_actionable_ref, parse_batch,
};
pub use ownership::{OwnershipError, TabClaim, TabOwner, TabRecord, TabRegistry};
pub use protocol::{ActionParseError, ActionProtocol, ParsedAction, parse_action};
pub use read::{ReadOptions, ReadSource, read_http};
pub use resources::{Domain, ResourceDropPolicy};
pub use selector::{CssOrXPath, SelectorResolver, SemanticTarget};
pub use session::{
    SessionBridgeError, cookie_from_cdp, cookie_to_cdp, get_cookies, group_cookies_by_site,
    inherit_cookies_from_chrome, inject_session, seal_session, set_cookies,
};
pub use tiers::{
    EngineConfig, EngineError, EngineResult, EngineTier, FetchIntent, LightEngine, TieredEngine,
};
pub use tree::{RefMinter, TreeOptions, build_tree};
pub use webmcp::{
    InvocationState, InvocationTracker, WebMcpError, WebMcpExecutor, WebMcpRegistry, WebMcpResult,
    WebMcpTool,
};
pub use webmcp_http::{
    HttpParseError, MAX_BODY_BYTES, MCP_PATH, McpHttpServer, bearer_token, fresh_token,
    handle_mcp_request, parse_http_request,
};

use serde::{Deserialize, Serialize};

/// Snapshot verbosity (E3): `interactive` = actionables + headings only
/// (token-lean); `full` = complete tree with depth caps 1..=100;
/// `slim` (E16, doc 63 §4.2 — chrome-devtools-mcp `SlimMcpResponse`) =
/// interactive pruning + long-text collapse + a shallower depth cap, for the
/// tightest token budget on every browser turn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotMode {
    Interactive,
    Full,
    Slim,
}

/// One node of the accessibility tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct A11yNode {
    pub role: String,
    pub name: String,
    /// Stable ref `eN`, scoped to (document_id, url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    /// True for interactive elements (buttons, links, inputs).
    #[serde(default)]
    pub actionable: bool,
    /// Present on iframe placeholder nodes — the child frame's id, used by
    /// the snapshot engine to stitch child-frame trees inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    /// The backing DOM node id (CDP `backendDOMNodeId`). Lets `act` resolve
    /// a `[ref=eN]` to geometry (`DOM.getBoxModel`) for click/type/hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_dom_node_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<A11yNode>,
}

impl A11yNode {
    pub fn new(role: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            name: name.into(),
            ref_id: None,
            actionable: false,
            frame_id: None,
            backend_dom_node_id: None,
            children: Vec::new(),
        }
    }

    pub fn with_ref(mut self, ref_id: impl Into<String>) -> Self {
        self.ref_id = Some(ref_id.into());
        self
    }

    pub fn with_actionable(mut self) -> Self {
        self.actionable = true;
        self
    }

    pub fn with_frame_id(mut self, frame_id: impl Into<String>) -> Self {
        self.frame_id = Some(frame_id.into());
        self
    }

    pub fn with_backend_dom_node_id(mut self, id: i64) -> Self {
        self.backend_dom_node_id = Some(id);
        self
    }

    pub fn push(&mut self, child: A11yNode) {
        self.children.push(child);
    }

    /// Indented tree text — the exact token-lean form fed to the model.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out, 0);
        out
    }

    fn render_into(&self, out: &mut String, depth: usize) {
        let indent = "  ".repeat(depth);
        let ref_part = self
            .ref_id
            .as_ref()
            .map(|r| format!(" [ref={r}]"))
            .unwrap_or_default();
        out.push_str(&format!(
            "{indent}{} {}{}\n",
            self.role, self.name, ref_part
        ));
        for c in &self.children {
            c.render_into(out, depth + 1);
        }
    }
}

/// A captured snapshot of one document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub document_id: String,
    pub url: String,
    pub mode: SnapshotMode,
    pub root: A11yNode,
}

/// Line-diff between two snapshots (`+n/-n` markers, E3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotDiff {
    pub base_document_id: String,
    pub base_url: String,
    pub added_lines: Vec<String>,
    pub removed_lines: Vec<String>,
    /// True when a navigation happened (URL change short-circuit → caller
    /// should return a full new snapshot instead of a diff).
    pub url_changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> A11yNode {
        let mut root = A11yNode::new("dialog", "Sign in");
        let mut form = A11yNode::new("form", "");
        let email = A11yNode::new("textbox", "Email")
            .with_ref("e1")
            .with_actionable();
        let submit = A11yNode::new("button", "Continue")
            .with_ref("e2")
            .with_actionable();
        form.push(email);
        form.push(submit);
        root.push(form);
        root
    }

    #[test]
    fn tree_renders_indented_with_refs() {
        let text = sample_tree().render();
        assert!(text.contains("dialog Sign in"));
        assert!(text.contains("  form "));
        assert!(text.contains("    textbox Email [ref=e1]"));
        assert!(text.contains("    button Continue [ref=e2]"));
    }

    #[test]
    fn snapshot_modes_roundtrip() {
        let s = Snapshot {
            document_id: "doc-1".into(),
            url: "https://example.com".into(),
            mode: SnapshotMode::Interactive,
            root: sample_tree(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.mode, SnapshotMode::Interactive);
    }

    #[test]
    fn diff_marks_url_change() {
        let d = SnapshotDiff {
            base_document_id: "doc-1".into(),
            base_url: "https://a.example".into(),
            added_lines: vec!["+ h1 Welcome".into()],
            removed_lines: vec!["- button Old [ref=e9]".into()],
            url_changed: true,
        };
        assert!(d.url_changed);
        assert!(d.added_lines[0].starts_with('+'));
    }
}
