//! P10.1.3 — browser automation pipeline E2E:
//! navigate → snapshot → act → diff → assert.
//!
//! The pipeline is exercised at the tree level (the exact code the CDP
//! snapshot engine produces): build the a11y tree from captured nodes, render
//! snapshot lines, parse an action, apply the resulting state change, and
//! diff the before/after snapshots. Live Chrome attachment stays the
//! credential/binary-gated seam the docs already record.

use agentcowork_browser::ax::AxNode;
use agentcowork_browser::diff::{diff_snapshots, snapshot_lines};
use agentcowork_browser::protocol::{ActionProtocol, ParsedAction, parse_action};
use agentcowork_browser::tree::{RefMinter, TreeOptions, build_tree};
use agentcowork_browser::{ActKind, Snapshot, SnapshotMode};

/// A small page: root → [nav, main[heading, button, link], footer].
fn page_nodes() -> Vec<AxNode> {
    vec![
        AxNode {
            node_id: "root".into(),
            role: "rootWebArea".into(),
            name: "Example".into(),
            value: String::new(),
            focusable: false,
            ignored: false,
            child_ids: vec!["nav".into(), "main".into(), "footer".into()],
            backend_dom_node_id: None,
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
        AxNode {
            node_id: "nav".into(),
            role: "navigation".into(),
            name: "Site nav".into(),
            value: String::new(),
            focusable: false,
            ignored: false,
            child_ids: vec![],
            backend_dom_node_id: None,
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
        AxNode {
            node_id: "h1".into(),
            role: "heading".into(),
            name: "Welcome".into(),
            value: String::new(),
            focusable: false,
            ignored: false,
            child_ids: vec![],
            backend_dom_node_id: None,
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
        AxNode {
            node_id: "btn".into(),
            role: "button".into(),
            name: "Toggle theme".into(),
            value: String::new(),
            focusable: true,
            ignored: false,
            child_ids: vec![],
            backend_dom_node_id: Some(42),
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
        AxNode {
            node_id: "lnk".into(),
            role: "link".into(),
            name: "Docs".into(),
            value: String::new(),
            focusable: true,
            ignored: false,
            child_ids: vec![],
            backend_dom_node_id: Some(43),
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
        AxNode {
            node_id: "main".into(),
            role: "main".into(),
            name: "Main".into(),
            value: String::new(),
            focusable: false,
            ignored: false,
            child_ids: vec!["h1".into(), "btn".into(), "lnk".into()],
            backend_dom_node_id: None,
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
        AxNode {
            node_id: "footer".into(),
            role: "contentinfo".into(),
            name: "footer".into(),
            value: String::new(),
            focusable: false,
            ignored: false,
            child_ids: vec![],
            backend_dom_node_id: None,
            frame_id: None,
            properties: Default::default(),
            has_js_click_handler: false,
        },
    ]
}

fn snapshot(nodes: &[AxNode], url: &str) -> Snapshot {
    let mut refs = RefMinter::new();
    let root = build_tree(nodes, TreeOptions::default(), &mut refs).expect("tree builds");
    Snapshot {
        document_id: "doc-1".into(),
        url: url.into(),
        mode: SnapshotMode::Interactive,
        root,
    }
}

#[test]
fn navigate_snapshot_act_diff_assert() {
    // 1. "navigate → snapshot": build the tree and render its lines.
    let before = snapshot(&page_nodes(), "https://example.com/");
    let lines = snapshot_lines(&before);
    assert!(lines.iter().any(|l| l.contains("Welcome")));
    assert!(lines.iter().any(|l| l.contains("Toggle theme")));

    // 2. "act": parse a native click against the captured button.
    let action = parse_action(
        ActionProtocol::Native,
        &serde_json::json!({ "kind": "click", "ref_id": "e2" }),
    )
    .expect("action parses");
    match action {
        ParsedAction::Act(ActKind::Click { ref_id }) => assert_eq!(ref_id, "e2"),
        other => panic!("expected click, got {other:?}"),
    }

    // 3. The click flips the page (theme toggle): the button label changes.
    let mut after_nodes = page_nodes();
    for n in &mut after_nodes {
        if n.node_id == "btn" {
            n.name = "Theme: dark".into();
        }
    }
    let after = snapshot(&after_nodes, "https://example.com/");

    // 4. "diff → assert": same URL → a line diff with +/- markers.
    let d = diff_snapshots(&before, &after);
    assert!(!d.url_changed, "same navigation, not a URL change");
    assert!(
        d.removed_lines.iter().any(|l| l.contains("Toggle theme")),
        "removed line missing: {:?}",
        d.removed_lines
    );
    assert!(
        d.added_lines.iter().any(|l| l.contains("Theme: dark")),
        "added line missing: {:?}",
        d.added_lines
    );

    // Cross-navigation short-circuit: a URL change yields a full-snapshot diff.
    let other = snapshot(&page_nodes(), "https://example.com/docs");
    let nav = diff_snapshots(&before, &other);
    assert!(nav.url_changed);
    assert!(nav.removed_lines.is_empty());
    assert!(!nav.added_lines.is_empty());
}
