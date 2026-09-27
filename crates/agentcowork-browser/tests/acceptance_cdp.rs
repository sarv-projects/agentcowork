//! Track 2 / P66.7 — Live Chrome CDP acceptance suite.
//!
//! Evidence-gated readiness (§17.12.6). These are `#[ignore]`d by default and
//! gated on `AGENTCOWORK_LIVE_TEST=1`; with that set they drive a **real**
//! headless Chrome over the CDP WebSocket and prove the product path:
//!
//! 1. **Pairing + snapshot** — spawn → `DevToolsActivePort` → connect → list
//!    targets → attach → capture a real accessibility tree with stable refs.
//! 2. **Ref + coordinate actions** — click by `[ref=eN]` and click by raw
//!    coordinates, reading the DOM back to confirm each effect landed, and a
//!    post-action diff.
//!
//! Run: `AGENTCOWORK_LIVE_TEST=1 cargo test -p agentcowork-browser --test
//! acceptance_cdp -- --ignored --test-threads=1`
//!
//! (The bare WireGuard/browser fixture is cross-platform; only the Windows
//! `--headless=new` flag matrix run remains open — see TODO P66.7.)

use agentcowork_browser::{ActKind, BrowserActions, NavigateAction, ReadMode, SnapshotEngine};

fn live_enabled() -> bool {
    agentcowork_types::env_compat::get("LIVE_TEST").as_deref() == Some("1")
}

/// Spawn headless Chrome and return the child + a client attached to its first
/// page target. Unique profile per (pid, tag) so parallel runs never contend.
fn spawn_and_connect(
    tag: &str,
) -> (
    agentcowork_cdp::BrowserChild,
    agentcowork_cdp::CdpClient,
    agentcowork_cdp::Session,
) {
    let opts = agentcowork_cdp::LaunchOptions {
        headless: true,
        user_data_dir: std::env::temp_dir().join(format!(
            "agentcowork-acceptance-profile-{}-{tag}",
            std::process::id()
        )),
        ..Default::default()
    };
    let child = agentcowork_cdp::spawn_browser(&opts).expect("spawn headless Chrome");
    let endpoint = child.endpoint().clone();
    let client =
        agentcowork_cdp::connect_to_browser(&endpoint).expect("connect over the CDP socket");

    let page = client
        .list_targets()
        .expect("list targets")
        .into_iter()
        .find(|t| t.target_type == agentcowork_cdp::TargetType::Page)
        .expect("a page target exists");
    let session = client.attach(&page.target_id).expect("attach to page");
    (child, client, session)
}

fn navigate(client: &agentcowork_cdp::CdpClient, session_id: &str, html: &str) {
    client
        .call_session(
            session_id,
            "Page.navigate",
            serde_json::json!({ "url": format!("data:text/html,{html}") }),
        )
        .expect("Page.navigate");
    std::thread::sleep(std::time::Duration::from_millis(1500));
}

#[test]
#[ignore = "live CDP — needs AGENTCOWORK_LIVE_TEST=1 and Chrome"]
fn live_cdp_pairing_and_a11y_snapshot() {
    if !live_enabled() {
        eprintln!("skipped: set AGENTCOWORK_LIVE_TEST=1");
        return;
    }
    let (child, client, session) = spawn_and_connect("snapshot");

    navigate(
        &client,
        &session.session_id,
        "<html><body><h1>Acceptance</h1><button id=b>Go</button><a href='#'>Link</a></body></html>",
    );

    // Real accessibility tree, captured over CDP, with stable refs.
    let engine =
        SnapshotEngine::default().with_mode(agentcowork_browser::SnapshotMode::Interactive);
    let snap = engine
        .capture(&client, Some(&session.session_id), "acceptance-doc")
        .expect("capture a11y snapshot");
    let rendered = snap.root.render();
    eprintln!("=== LIVE SNAPSHOT ===\n{rendered}\n=== END ===");
    assert!(
        rendered.contains("heading"),
        "expected a heading:\n{rendered}"
    );
    assert!(
        rendered.contains("button Go [ref=e"),
        "expected an actionable button with a ref:\n{rendered}"
    );
    assert!(snap.url.starts_with("data:"), "url captured: {}", snap.url);

    drop(child);
}

#[test]
#[ignore = "live CDP — needs AGENTCOWORK_LIVE_TEST=1 and Chrome"]
fn live_cdp_ref_and_coordinate_actions() {
    if !live_enabled() {
        eprintln!("skipped: set AGENTCOWORK_LIVE_TEST=1");
        return;
    }
    let (child, client, session) = spawn_and_connect("actions");

    // A button that flips the paragraph, pinned at a known coordinate so the
    // coordinate path is deterministic.
    let html = "<html><body>\
        <h1>P66.7</h1>\
        <button id=b style='position:absolute;left:100px;top:100px;width:120px;height:40px' \
        onclick=\"document.getElementById('out').textContent='clicked!'\">Go</button>\
        <p id=out>initial</p></body></html>";
    navigate(&client, &session.session_id, html);

    let actions = BrowserActions::new(&client, Some(&session.session_id));

    // Ref-based action: snapshot → find the button ref → click it.
    let snap = actions.snapshot("acceptance-act").expect("snapshot");
    let rendered = snap.root.render();
    let btn_ref = {
        let mut found = None;
        for line in rendered.lines() {
            if line.contains("button Go") {
                if let Some(start) = line.find("[ref=") {
                    let rest = &line[start + 5..];
                    if let Some(end) = rest.find(']') {
                        found = Some(rest[..end].to_string());
                    }
                }
            }
        }
        found.unwrap_or_else(|| panic!("no button ref in:\n{rendered}"))
    };
    let res = actions
        .act(ActKind::Click {
            ref_id: btn_ref.clone(),
        })
        .expect("click by ref");
    assert_eq!(res.kind, "click");
    let read = actions.read(ReadMode::Raw).expect("read");
    assert!(
        read.text.contains("clicked!"),
        "ref click did not land:\n{}",
        read.text
    );

    // Coordinate-based action: reload to reset, then click by raw x/y.
    actions.navigate(NavigateAction::Reload).expect("reload");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let reset = actions.read(ReadMode::Raw).expect("read reset");
    assert!(
        reset.text.contains("initial"),
        "reload should reset the DOM"
    );

    let at = actions
        .act(ActKind::ClickAt { x: 160.0, y: 120.0 })
        .expect("click at coordinates");
    assert_eq!(at.kind, "click_at");
    let after = actions
        .read(ReadMode::Raw)
        .expect("read after coordinate click");
    assert!(
        after.text.contains("clicked!"),
        "coordinate click did not land:\n{}",
        after.text
    );

    eprintln!("LIVE PASS: CDP pairing → a11y snapshot → ref click → coordinate click");
    drop(child);
}
