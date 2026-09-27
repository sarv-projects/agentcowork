//! Accessibility-tree node model (P2.2, E3) — the wire-to-domain layer.
//!
//! Parses `Accessibility.getFullAXTree` nodes (doc 55 agent-browser
//! snapshot.rs semantics): role taxonomy (`INTERACTIVE_ROLES` /
//! `CONTENT_ROLES` / `STRUCTURAL_ROLES`), zero-width-char filtering
//! (`\u{FEFF}`, `\u{200B}` …), focusable detection, frame-id capture (for
//! iframe stitching).

use serde_json::Value;
use std::collections::HashMap;

/// Roles the agent can act on — these get `[ref=eN]` handles.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "menuitem",
    "option",
    "slider",
    "switch",
    "tab",
    "treeitem",
    "spinbutton",
    "colorwell",
    "menuitemcheckbox",
    "menuitemradio",
];

/// Roles that carry content worth showing in interactive mode (headings +
/// images/tables are the notable ones; doc 33 §5.2 keeps "actionables +
/// headings only").
pub const CONTENT_ROLES: &[&str] = &[
    "heading",
    "img",
    "image",
    "table",
    "list",
    "listitem",
    "paragraph",
    "link",
    "math",
    "meter",
    "progressbar",
    "radio",
    "separator",
    "alert",
];

/// Structural/container roles — pruned in interactive mode unless they have
/// kept descendants.
pub const STRUCTURAL_ROLES: &[&str] = &[
    "generic",
    "group",
    "row",
    "cell",
    "columnheader",
    "rowheader",
    "statictext",
    "WebArea",
    "RootWebArea",
    "Iframe",
    "iframe",
    "document",
    "application",
    "article",
    "main",
    "navigation",
    "region",
    "section",
    "complementary",
    "contentinfo",
    "banner",
    "form",
    "toolbar",
    "menu",
    "menubar",
    "tablist",
    "tree",
    "grid",
    "dialog",
    "window",
    "text",
];

/// Zero-width / invisible characters stripped from accessible names.
const ZERO_WIDTH: &[char] = &[
    '\u{FEFF}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{00AD}',
];

/// Is this role directly actionable (interactive)?
pub fn is_interactive(role: &str) -> bool {
    INTERACTIVE_ROLES.contains(&role)
}

/// Is this role a content-carrying one kept even in interactive mode?
pub fn is_content(role: &str) -> bool {
    CONTENT_ROLES.contains(&role)
}

/// Is this role pure structure (prunable in interactive mode)?
pub fn is_structural(role: &str) -> bool {
    STRUCTURAL_ROLES.contains(&role)
}

/// Heading check (kept in interactive mode per doc 33 §5.2).
pub fn is_heading(role: &str) -> bool {
    role == "heading"
}

/// Strip zero-width and soft-hyphen characters from a name (doc 55).
pub fn strip_zero_width(name: &str) -> String {
    if name.chars().any(|c| ZERO_WIDTH.contains(&c)) {
        name.chars().filter(|c| !ZERO_WIDTH.contains(c)).collect()
    } else {
        name.to_string()
    }
}

/// One `Accessibility.getFullAXTree` node, domain-shaped.
#[derive(Debug, Clone, PartialEq)]
pub struct AxNode {
    pub node_id: String,
    pub role: String,
    pub name: String,
    pub value: String,
    pub focusable: bool,
    pub ignored: bool,
    pub child_ids: Vec<String>,
    pub backend_dom_node_id: Option<i64>,
    pub frame_id: Option<String>,
    /// Extra boolean/string properties of interest (pressed, expanded, checked…).
    pub properties: HashMap<String, String>,
    /// E16 slim-mode event-listener detection (lightpanda `interactive.zig`
    /// pattern): an element clickable only via JavaScript (SPA divs with an
    /// `onclick`/event listener, no native role). Populated by a separate
    /// `DOMDebugger.getEventListeners` pass — not part of the AX tree.
    pub has_js_click_handler: bool,
}

impl AxNode {
    /// Mark this node as JS-clickable (the capture layer calls this after a
    /// `DOMDebugger.getEventListeners` pass).
    pub fn with_js_click_handler(mut self) -> Self {
        self.has_js_click_handler = true;
        self
    }

    /// True when the node is clickable only via JS (no native/ARIA role).
    pub fn is_js_clickable(&self) -> bool {
        self.has_js_click_handler
    }
}

impl AxNode {
    /// Parse from the CDP wire node. Tolerant: unknown shapes degrade to
    /// empty fields instead of failing the whole capture.
    pub fn from_json(v: &Value) -> AxNode {
        let role = role_of(v);
        AxNode {
            node_id: v
                .get("nodeId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            role,
            name: strip_zero_width(
                v.get("name")
                    .and_then(|n| n.get("value"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            value: v
                .get("value")
                .and_then(|n| n.get("value"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            focusable: bool_property(v, "focusable"),
            ignored: v.get("ignored").and_then(Value::as_bool).unwrap_or(false),
            child_ids: v
                .get("childIds")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            backend_dom_node_id: v.get("backendDOMNodeId").and_then(Value::as_i64),
            frame_id: v.get("frameId").and_then(Value::as_str).map(str::to_string),
            // The AX tree does not carry event listeners; the capture layer
            // sets this via `with_js_click_handler` after a DOMDebugger pass.
            has_js_click_handler: false,
            properties: property_map(v),
        }
    }

    /// Parse a whole `nodes` array from `Accessibility.getFullAXTree`.
    pub fn parse_many(v: &Value) -> Vec<AxNode> {
        match v.get("nodes").and_then(Value::as_array) {
            Some(nodes) => nodes.iter().map(AxNode::from_json).collect(),
            None => Vec::new(),
        }
    }

    /// Index nodes by node_id.
    pub fn index(nodes: &[AxNode]) -> HashMap<String, AxNode> {
        nodes
            .iter()
            .map(|n| (n.node_id.clone(), n.clone()))
            .collect()
    }
}

fn role_of(v: &Value) -> String {
    v.get("role")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn bool_property(v: &Value, key: &str) -> bool {
    v.get("properties")
        .and_then(Value::as_array)
        .and_then(|props| {
            props
                .iter()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(key))
        })
        .and_then(|p| p.pointer("/value/value"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn property_map(v: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(props) = v.get("properties").and_then(Value::as_array) {
        for p in props {
            let Some(name) = p.get("name").and_then(Value::as_str) else {
                continue;
            };
            if let Some(val) = p.get("value").and_then(|vv| vv.get("value")) {
                let s = match val {
                    Value::Bool(b) => b.to_string(),
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                out.insert(name.to_string(), s);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cdp_node() {
        let v = json!({
            "nodeId": "1",
            "ignored": false,
            "role": { "type": "role", "value": "button" },
            "name": { "type": "computedString", "value": "Submit\u{200B}" },
            "value": { "type": "computedString", "value": "" },
            "properties": [
                { "name": "focusable", "value": { "type": "boolean", "value": true } },
                { "name": "pressed", "value": { "type": "boolean", "value": false } }
            ],
            "childIds": ["2", "3"],
            "backendDOMNodeId": 12,
            "frameId": "FRAME-A"
        });
        let n = AxNode::from_json(&v);
        assert_eq!(n.node_id, "1");
        assert_eq!(n.role, "button");
        assert_eq!(n.name, "Submit"); // zero-width stripped
        assert!(n.focusable);
        assert_eq!(n.child_ids, vec!["2", "3"]);
        assert_eq!(n.backend_dom_node_id, Some(12));
        assert_eq!(n.frame_id.as_deref(), Some("FRAME-A"));
        assert_eq!(
            n.properties.get("pressed").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn tolerant_of_missing_fields() {
        let n = AxNode::from_json(&json!({ "nodeId": "x" }));
        assert_eq!(n.role, "");
        assert_eq!(n.name, "");
        assert!(!n.focusable);
        assert!(n.child_ids.is_empty());
    }

    #[test]
    fn zero_width_variants_stripped() {
        for ch in [
            '\u{FEFF}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{00AD}',
        ] {
            let name = format!("a{ch}b");
            assert_eq!(strip_zero_width(&name), "ab", "char U+{:04X}", ch as u32);
        }
    }

    #[test]
    fn taxonomy_classifies_roles() {
        assert!(is_interactive("button"));
        assert!(is_interactive("textbox"));
        assert!(is_interactive("menuitem"));
        assert!(!is_interactive("heading"));
        assert!(is_heading("heading"));
        assert!(is_content("img"));
        assert!(is_structural("generic"));
        assert!(is_structural("WebArea"));
    }

    #[test]
    fn parse_many_and_index() {
        let v = json!({
            "nodes": [
                { "nodeId": "1", "role": { "value": "WebArea" }, "name": { "value": "Doc" } },
                { "nodeId": "2", "role": { "value": "button" }, "name": { "value": "Go" } }
            ]
        });
        let nodes = AxNode::parse_many(&v);
        assert_eq!(nodes.len(), 2);
        let idx = AxNode::index(&nodes);
        assert!(idx.contains_key("1"));
        assert!(idx.contains_key("2"));
    }
}
