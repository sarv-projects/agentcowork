//! Snapshot capture engine (P2.2, E3) — drives a CDP client session to
//! produce an `A11yNode` tree with iframes stitched inline (doc 33 §5.2:
//! multi-frame capture, max frame depth 5, refs scoped to (document_id, url)).

use crate::ax::AxNode;
use crate::tree::{RefMinter, TreeOptions, build_tree};
use crate::{A11yNode, Snapshot, SnapshotMode};
use agentcowork_cdp::{CdpClient, CdpError, Session, TargetInfo};
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;

/// Max nested-frame depth (doc 33 §5.2 `MAX_FRAME_DEPTH 5`).
pub const MAX_FRAME_DEPTH: usize = 5;

/// The subset of the CDP client the snapshot engine needs — kept as a trait
/// so tests can drive the engine with a scripted mock instead of a real
/// browser.
pub trait CdpSession {
    /// Browser-level call (no session).
    fn call(&self, method: &str, params: Value) -> Result<Value, CdpError>;
    /// Session-scoped call (a tab or frame).
    fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError>;
    /// Attach a session to a target (used for child frames).
    fn attach(&self, target_id: &str) -> Result<Session, CdpError>;
    /// Drain queued protocol events (used for Target.attachedToTarget).
    fn drain_events(&self) -> Vec<agentcowork_cdp::CdpEvent>;
}

impl CdpSession for CdpClient {
    fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        CdpClient::call(self, method, params)
    }
    fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        CdpClient::call_session(self, session_id, method, params)
    }
    fn attach(&self, target_id: &str) -> Result<Session, CdpError> {
        CdpClient::attach(self, target_id)
    }
    fn drain_events(&self) -> Vec<agentcowork_cdp::CdpEvent> {
        CdpClient::drain_events(self)
    }
}

/// Snapshot capture engine.
pub struct SnapshotEngine {
    pub options: TreeOptions,
    pub max_frame_depth: usize,
}

impl Default for SnapshotEngine {
    fn default() -> Self {
        Self {
            options: TreeOptions::default(),
            max_frame_depth: MAX_FRAME_DEPTH,
        }
    }
}

impl SnapshotEngine {
    pub fn with_mode(mut self, mode: SnapshotMode) -> Self {
        self.options = self.options.apply_mode(mode);
        self
    }

    /// Capture a full snapshot of one document (root frame + stitched
    /// iframes). `session_id` is the attached session of the top frame.
    pub fn capture<C: CdpSession>(
        &self,
        client: &C,
        session_id: Option<&str>,
        document_id: &str,
    ) -> Result<Snapshot, CdpError> {
        let mut refs = RefMinter::new();
        let root = self.capture_frame(client, session_id, None, 0, &mut refs)?;
        let url = self.current_url(client, session_id);
        Ok(Snapshot {
            document_id: document_id.to_string(),
            url,
            mode: self.options.mode,
            root,
        })
    }

    /// Capture one frame's tree and stitch its child frames inline.
    ///
    /// `explicit_frame_id` requests a specific (child) frame's tree via
    /// `Accessibility.getFullAXTree({frameId})` — the documented mechanism
    /// for same-process iframes, which never appear as attachable targets.
    fn capture_frame<C: CdpSession>(
        &self,
        client: &C,
        session_id: Option<&str>,
        explicit_frame_id: Option<&str>,
        depth: usize,
        refs: &mut RefMinter,
    ) -> Result<A11yNode, CdpError> {
        let params = match explicit_frame_id {
            Some(fid) => json!({ "frameId": fid }),
            None => json!({}),
        };
        let raw = match session_id {
            Some(sid) => client.call_session(sid, "Accessibility.getFullAXTree", params)?,
            None => client.call("Accessibility.getFullAXTree", params)?,
        };
        let mut nodes = AxNode::parse_many(&raw);
        // The AX iframe node often lacks `frameId`; resolve it via
        // DOM.describeNode (the owner-frame id) so stitching can attach.
        resolve_iframe_frame_ids(client, session_id, &mut nodes);
        let mut tree = build_tree(&nodes, self.options, refs)
            .unwrap_or_else(|| A11yNode::new("WebArea", "(empty document)"));

        if depth >= self.max_frame_depth {
            return Ok(tree);
        }

        // Stitch child frames: iframe placeholder nodes carry their frame id.
        let iframe_frame_ids = collect_iframe_frame_ids(&tree);
        if iframe_frame_ids.is_empty() {
            return Ok(tree);
        }
        // Primary mechanism: Accessibility.getFullAXTree({frameId}) — works
        // for same-process iframes (no target to attach to). Fallback: an
        // attached session for that frame (OOPIFs / auto-attach).
        let frame_sessions = self.collect_frame_sessions(client, session_id);
        for frame_id in iframe_frame_ids {
            let primary = self.capture_frame(client, session_id, Some(&frame_id), depth + 1, refs);
            // Chrome answers an unknown frameId with Ok + empty nodes — treat
            // an empty tree as "no such frame" so the attached-session
            // fallback still runs.
            let primary_ok = matches!(
                &primary,
                Ok(child) if !child.children.is_empty() || !child.name.is_empty()
            );
            let child_tree = if primary_ok {
                primary
            } else {
                frame_sessions
                    .get(&frame_id)
                    .cloned()
                    .and_then(|sid| {
                        self.capture_frame(client, Some(&sid), None, depth + 1, refs)
                            .ok()
                    })
                    .ok_or(CdpError::Protocol {
                        code: -1,
                        message: format!("cannot capture frame {frame_id}"),
                    })
            };
            match child_tree {
                Ok(child) => {
                    splice_iframe_children(&mut tree, &frame_id, &child);
                }
                Err(_) => continue, // keep the placeholder line
            }
        }
        Ok(tree)
    }

    /// Resolve `frameId -> sessionId` for child frames of this session.
    ///
    /// `Target.setAutoAttach(flatten)` on the owning session makes Chrome
    /// attach a session to every child frame (same-process included) and emit
    /// `Target.attachedToTarget` events; OOPIFs also show up via
    /// `Target.getTargets`. Events are drained right after attach, so freshly
    /// created frame sessions are captured without waiting.
    fn collect_frame_sessions<C: CdpSession>(
        &self,
        client: &C,
        session_id: Option<&str>,
    ) -> HashMap<String, String> {
        let mut out = HashMap::new();
        // 1. Auto-attach to child frames (works for same-process iframes).
        let attach = match session_id {
            Some(sid) => client.call_session(
                sid,
                "Target.setAutoAttach",
                json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
            ),
            None => client.call(
                "Target.setAutoAttach",
                json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
            ),
        };
        if attach.is_ok() {
            // Chrome emits Target.attachedToTarget asynchronously — poll the
            // event queue briefly to catch existing child frames.
            for _ in 0..10 {
                let mut found = false;
                for ev in client.drain_events() {
                    if ev.method != "Target.attachedToTarget" {
                        continue;
                    }
                    let Some(child_sid) = ev.params.get("sessionId").and_then(Value::as_str) else {
                        continue;
                    };
                    if let Some(fid) = ev
                        .params
                        .pointer("/targetInfo/frameId")
                        .and_then(Value::as_str)
                    {
                        out.insert(fid.to_string(), child_sid.to_string());
                        found = true;
                    }
                }
                if found {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        // 2. OOPIF targets.
        if let Ok(v) = client.call("Target.getTargets", Value::Null) {
            if let Ok(targets) = serde_json::from_value::<Vec<TargetInfo>>(
                v.get("targetInfos").cloned().unwrap_or(Value::Null),
            ) {
                for t in targets {
                    if let Some(fid) = &t.frame_id {
                        if let Ok(sid) = client.attach(&t.target_id) {
                            out.insert(fid.clone(), sid.session_id);
                        }
                    }
                }
            }
        }
        out
    }

    /// The top frame's URL via `Page.getFrameTree` (best-effort; empty on
    /// failure).
    fn current_url<C: CdpSession>(&self, client: &C, session_id: Option<&str>) -> String {
        let res = match session_id {
            Some(sid) => client.call_session(sid, "Page.getFrameTree", Value::Null),
            None => client.call("Page.getFrameTree", Value::Null),
        };
        match res {
            Ok(v) => v
                .pointer("/frameTree/frame/url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            Err(_) => String::new(),
        }
    }
}

/// Fill missing `frame_id` on iframe AX nodes using `DOM.describeNode`
/// (`backendNodeId` → `frameId` of the owner frame). Best-effort: failures
/// leave the node without a frame id (placeholder stays unstitched).
fn resolve_iframe_frame_ids<C: CdpSession>(
    client: &C,
    session_id: Option<&str>,
    nodes: &mut [AxNode],
) {
    for node in nodes.iter_mut() {
        if !(node.role == "Iframe" || node.role == "iframe") || node.frame_id.is_some() {
            continue;
        }
        let Some(backend_id) = node.backend_dom_node_id else {
            continue;
        };
        let res = match session_id {
            Some(sid) => client.call_session(
                sid,
                "DOM.describeNode",
                json!({ "backendNodeId": backend_id, "depth": 0 }),
            ),
            None => client.call(
                "DOM.describeNode",
                json!({ "backendNodeId": backend_id, "depth": 0 }),
            ),
        };
        if let Ok(v) = res {
            if let Some(fid) = v.pointer("/node/frameId").and_then(Value::as_str) {
                node.frame_id = Some(fid.to_string());
            }
        }
    }
}

/// Collect the frame ids of inline iframe placeholder nodes (role Iframe).
fn collect_iframe_frame_ids(tree: &A11yNode) -> Vec<String> {
    let mut out = Vec::new();
    collect_iframe_frame_ids_into(tree, &mut out);
    out
}

fn collect_iframe_frame_ids_into(node: &A11yNode, out: &mut Vec<String>) {
    if node.role.eq_ignore_ascii_case("iframe") {
        if let Some(fid) = &node.frame_id {
            out.push(fid.clone());
        }
    }
    for c in &node.children {
        collect_iframe_frame_ids_into(c, out);
    }
}

/// Replace the iframe placeholder's children with the captured child frame's
/// content (the child's WebArea root line is skipped — content splices
/// directly under the placeholder, matching "iframes stitched inline at their
/// placeholder line").
fn splice_iframe_children(tree: &mut A11yNode, frame_id: &str, child_tree: &A11yNode) -> bool {
    splice_into(tree, frame_id, child_tree)
}

fn splice_into(node: &mut A11yNode, frame_id: &str, child_tree: &A11yNode) -> bool {
    if node.role.eq_ignore_ascii_case("iframe") && node.frame_id.as_deref() == Some(frame_id) {
        // Skip only the child frame's document root (WebArea) — its content
        // splices directly under the placeholder. If the interactive-mode
        // collapse already replaced the root with actionable content, keep
        // that node as a child.
        if child_tree.role.eq_ignore_ascii_case("webarea")
            || child_tree.role.eq_ignore_ascii_case("rootwebarea")
        {
            node.children = child_tree.children.clone();
        } else {
            node.children = vec![child_tree.clone()];
        }
        return true;
    }
    for c in &mut node.children {
        if splice_into(c, frame_id, child_tree) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Scripted fake CDP session.
    #[derive(Clone)]
    struct FakeSession {
        /// target_id -> session_id for attach (iframe targets).
        frames: HashMap<String, String>,
        /// session_id -> AX nodes JSON for that frame.
        trees: HashMap<String, Value>,
        /// target infos incl. iframe targets.
        targets: Vec<TargetInfo>,
        /// top-frame url.
        url: String,
    }

    impl FakeSession {
        fn with_frame(mut self, frame_id: &str, session_id: &str, nodes: Value) -> Self {
            self.frames.insert(frame_id.into(), session_id.into());
            self.trees.insert(session_id.into(), nodes);
            self
        }

        fn with_url(mut self, url: &str) -> Self {
            self.url = url.into();
            self
        }
    }

    impl Default for FakeSession {
        fn default() -> Self {
            Self {
                frames: HashMap::new(),
                trees: HashMap::new(),
                targets: Vec::new(),
                url: "https://example.com".into(),
            }
        }
    }

    fn ax_node(id: &str, role: &str, name: &str, children: &[&str], frame: Option<&str>) -> Value {
        let mut v = serde_json::json!({
            "nodeId": id,
            "role": { "value": role },
            "name": { "value": name },
            "childIds": children,
        });
        if let Some(f) = frame {
            v["frameId"] = serde_json::json!(f);
        }
        v
    }

    impl CdpSession for FakeSession {
        fn call(&self, method: &str, _params: Value) -> Result<Value, CdpError> {
            match method {
                "Target.getTargets" => Ok(serde_json::json!({ "targetInfos": self.targets })),
                _ => Err(CdpError::Protocol {
                    code: -1,
                    message: format!("unexpected browser call {method}"),
                }),
            }
        }

        fn call_session(
            &self,
            session_id: &str,
            method: &str,
            params: Value,
        ) -> Result<Value, CdpError> {
            match method {
                "Accessibility.getFullAXTree" => {
                    // A frameId param requests that frame's tree directly.
                    if let Some(fid) = params.get("frameId").and_then(Value::as_str) {
                        if let Some(sid) = self.frames.get(fid) {
                            return Ok(self
                                .trees
                                .get(sid)
                                .cloned()
                                .unwrap_or(serde_json::json!({ "nodes": [] })));
                        }
                    }
                    Ok(self
                        .trees
                        .get(session_id)
                        .cloned()
                        .unwrap_or(serde_json::json!({ "nodes": [] })))
                }
                "Page.getFrameTree" => Ok(serde_json::json!({
                    "frameTree": { "frame": { "url": self.url } }
                })),
                _ => Err(CdpError::Protocol {
                    code: -1,
                    message: format!("unexpected session call {method}"),
                }),
            }
        }

        fn attach(&self, target_id: &str) -> Result<Session, CdpError> {
            match self.frames.get(target_id) {
                Some(sid) => Ok(Session {
                    session_id: sid.clone(),
                    target_id: target_id.into(),
                }),
                None => Err(CdpError::Protocol {
                    code: -1,
                    message: "no such frame target".into(),
                }),
            }
        }

        fn drain_events(&self) -> Vec<agentcowork_cdp::CdpEvent> {
            Vec::new()
        }
    }

    #[test]
    fn captures_simple_document_with_url() {
        let session = FakeSession::default().with_frame(
            "ROOT",
            "root-sess",
            serde_json::json!({
                "nodes": [
                    ax_node("1", "WebArea", "Page", &["2", "3"], None),
                    ax_node("2", "heading", "Hello", &[], None),
                    ax_node("3", "button", "Go", &[], None),
                ]
            }),
        );
        let engine = SnapshotEngine::default();
        let snap = engine
            .capture(&session, Some("root-sess"), "doc-1")
            .unwrap();
        assert_eq!(snap.url, "https://example.com");
        let rendered = snap.root.render();
        assert!(rendered.contains("heading Hello"), "{rendered}");
        assert!(rendered.contains("button Go [ref=e1]"), "{rendered}");
    }

    #[test]
    fn stitches_iframe_inline() {
        // Root frame has an Iframe placeholder with a frameId.
        let root = serde_json::json!({
            "nodes": [
                ax_node("1", "WebArea", "Root", &["2"], None),
                ax_node("2", "Iframe", "child frame", &[], Some("FRAME-CHILD")),
            ]
        });
        let child = serde_json::json!({
            "nodes": [
                ax_node("c1", "WebArea", "Child", &["c2"], None),
                ax_node("c2", "button", "In frame", &[], None),
            ]
        });
        let session = FakeSession::default()
            .with_frame("ROOT", "root-sess", root.clone())
            .with_frame("FRAME-CHILD", "child-sess", child.clone())
            .with_url("https://example.com");
        // Wire up the iframe target so the engine can attach to it.
        let session = FakeSession {
            targets: vec![TargetInfo {
                target_id: "FRAME-CHILD".into(),
                target_type: agentcowork_cdp::TargetType::Iframe,
                title: "child".into(),
                url: "https://child.example".into(),
                ws_url: "ws://127.0.0.1:0/x".into(),
                frame_id: Some("FRAME-CHILD".into()),
            }],
            ..session
        };

        let engine = SnapshotEngine::default();
        let snap = engine
            .capture(&session, Some("root-sess"), "doc-1")
            .unwrap();
        let rendered = snap.root.render();
        // The iframe placeholder line remains, with the child's button inline.
        assert!(rendered.contains("Iframe child frame"), "{rendered}");
        assert!(rendered.contains("button In frame"), "{rendered}");
    }

    #[test]
    fn unresolvable_iframe_keeps_placeholder() {
        let root = serde_json::json!({
            "nodes": [
                ax_node("1", "WebArea", "Root", &["2"], None),
                ax_node("2", "Iframe", "orphan", &[], Some("FRAME-GONE")),
            ]
        });
        // No target for FRAME-GONE → placeholder stays, no crash.
        let session = FakeSession::default()
            .with_frame("ROOT", "root-sess", root.clone())
            .with_url("https://example.com");
        let engine = SnapshotEngine::default();
        let snap = engine
            .capture(&session, Some("root-sess"), "doc-1")
            .unwrap();
        assert!(snap.root.render().contains("Iframe orphan"));
    }

    #[test]
    fn frame_depth_limit_stops_stitching() {
        let engine = SnapshotEngine {
            max_frame_depth: 0,
            ..Default::default()
        };
        let session = FakeSession::default();
        let snap = engine
            .capture(&session, Some("root-sess"), "doc-1")
            .unwrap();
        assert_eq!(snap.root.role, "WebArea");
    }

    #[test]
    fn splice_and_collect_helpers() {
        let mut tree = A11yNode::new("WebArea", "Root");
        tree.push(A11yNode::new("Iframe", "f").with_frame_id("FRAME-X"));
        assert_eq!(collect_iframe_frame_ids(&tree), vec!["FRAME-X".to_string()]);
        let mut child = A11yNode::new("WebArea", "Child");
        child.push(A11yNode::new("button", "Inside"));
        assert!(splice_iframe_children(&mut tree, "FRAME-X", &child));
        assert_eq!(tree.children[0].children.len(), 1);
    }
}
