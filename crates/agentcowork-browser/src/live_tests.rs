//! Live integration tests (P2.1/P2.2) — `#[ignore]`d by default; run with
//! `AGENTCOWORK_LIVE_TEST=1` when Chrome is available.
//!
//! Exercises the real path end to end: `spawn_browser` →
//! `DevToolsActivePort` → `connect_to_browser` → attach → a11y snapshot →
//! iframe stitching.

use crate::{SnapshotEngine, SnapshotMode};
use agentcowork_cdp::TargetType;

fn live_enabled() -> bool {
    agentcowork_types::env_compat::get("LIVE_TEST").as_deref() == Some("1")
}

/// Spawn headless Chrome and return a connected client + the first page
/// target's session.
fn spawn_and_connect(
    tag: &str,
) -> (
    agentcowork_cdp::BrowserChild,
    agentcowork_cdp::CdpClient,
    agentcowork_cdp::Session,
) {
    let opts = agentcowork_cdp::LaunchOptions {
        headless: true,
        // Unique per (pid, tag) so parallel runs never contend on the same
        // Chrome profile lock.
        user_data_dir: std::env::temp_dir().join(format!(
            "agentcowork-live-profile-{}-{tag}",
            std::process::id()
        )),
        ..Default::default()
    };
    let child = agentcowork_cdp::spawn_browser(&opts)
        .expect("spawn headless Chrome (is google-chrome installed?)");
    let endpoint = child.endpoint().clone();
    let client = agentcowork_cdp::connect_to_browser(&endpoint).expect("connect to browser");
    let targets = client.list_targets().expect("list targets");
    let page = targets
        .iter()
        .find(|t| t.target_type == TargetType::Page)
        .cloned()
        .unwrap_or_else(|| {
            client
                .call(
                    "Target.createTarget",
                    serde_json::json!({ "url": "about:blank" }),
                )
                .expect("create target");
            client
                .list_targets()
                .expect("list targets after create")
                .into_iter()
                .find(|t| t.target_type == TargetType::Page)
                .expect("page target")
        });
    let session = client.attach(&page.target_id).expect("attach to page");
    (child, client, session)
}

#[test]
#[ignore]
fn live_spawn_connect_attach_snapshot() {
    if !live_enabled() {
        return;
    }
    let (child, client, session) = spawn_and_connect("snapshot");

    // Navigate to a simple page and wait for it to settle.
    client
        .call_session(
            &session.session_id,
            "Page.navigate",
            serde_json::json!({ "url": "data:text/html,<html><body><h1>Hello</h1><button id=b>Go</button></body></html>" }),
        )
        .expect("navigate");
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let engine = SnapshotEngine::default().with_mode(SnapshotMode::Interactive);
    let snap = engine
        .capture(&client, Some(&session.session_id), "live-doc")
        .expect("capture snapshot");
    let rendered = snap.root.render();
    eprintln!("=== LIVE SNAPSHOT ===\n{rendered}\n=== END ===");
    assert!(
        rendered.contains("button Go [ref=e"),
        "expected an actionable button with a ref, got:\n{rendered}"
    );
    assert!(
        rendered.contains("heading"),
        "expected a heading:\n{rendered}"
    );
    eprintln!("LIVE PASS: real Chrome a11y snapshot with stable refs");

    drop(child);
}

#[test]
#[ignore]
fn live_iframe_stitching() {
    if !live_enabled() {
        return;
    }
    let (child, client, session) = spawn_and_connect("iframe");

    // A page with a srcdoc iframe — same-process, no separate target; the
    // engine must stitch it via Accessibility.getFullAXTree({frameId}).
    let html = "<html><body><h1>Outer</h1><iframe srcdoc=\"<button id=inner>Inside</button>\"></iframe></body></html>";
    client
        .call_session(
            &session.session_id,
            "Page.navigate",
            serde_json::json!({ "url": format!("data:text/html,{html}") }),
        )
        .expect("navigate");
    std::thread::sleep(std::time::Duration::from_millis(2000));

    let engine = SnapshotEngine::default().with_mode(SnapshotMode::Interactive);
    let snap = engine
        .capture(&client, Some(&session.session_id), "live-doc-iframe")
        .expect("capture snapshot");
    let rendered = snap.root.render();
    eprintln!("=== LIVE IFRAME SNAPSHOT ===\n{rendered}\n=== END ===");
    assert!(
        rendered.contains("heading Outer"),
        "expected outer heading:\n{rendered}"
    );
    assert!(
        rendered.contains("button Inside"),
        "expected stitched iframe content:\n{rendered}"
    );
    eprintln!("LIVE PASS: iframe content stitched inline");

    drop(child);
}

// ---------------------------------------------------------------------------
// P2.3 — action engine live test: navigate → snapshot → act → read
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_act_loop_navigate_click_read() {
    if !live_enabled() {
        return;
    }
    let (child, client, session) = spawn_and_connect("actions");

    // A page with a button that toggles a paragraph.
    let html = "<html><body>\
        <h1>P2.3 Test</h1>\
        <button id=b onclick=\"document.getElementById('out').textContent='clicked!'\">Go</button>\
        <p id=out>initial</p>\
        </body></html>";
    client
        .call_session(
            &session.session_id,
            "Page.navigate",
            serde_json::json!({ "url": format!("data:text/html,{html}") }),
        )
        .expect("navigate");
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let actions = crate::BrowserActions::new(&client, Some(&session.session_id));

    // snapshot → find the button ref.
    let snap = actions.snapshot("live-act").expect("snapshot");
    let rendered = snap.root.render();
    assert!(
        rendered.contains("button Go [ref=e1"),
        "expected button ref:\n{rendered}"
    );
    eprintln!("=== LIVE ACT SNAPSHOT ===\n{rendered}\n=== END ===");

    // act: click the button (ref → geometry → Input.dispatchMouseEvent).
    let res = actions
        .act(crate::ActKind::Click {
            ref_id: "e1".into(),
        })
        .expect("act click");
    assert_eq!(res.kind, "click");
    assert!(res.diff.is_some(), "act must return a post-settle diff");

    // verify the click landed via read (DOM walker sees the new text).
    let read = actions.read(crate::ReadMode::Raw).expect("read");
    assert!(
        read.text.contains("clicked!"),
        "click should have updated the DOM:\n{}",
        read.text
    );

    // navigate: back/forward/reload round-trip.
    actions
        .navigate(crate::NavigateAction::Reload)
        .expect("reload");
    std::thread::sleep(std::time::Duration::from_millis(1000));

    eprintln!("LIVE PASS: act loop (navigate → snapshot → click → read → reload)");
    drop(child);
}

/// P2.4 — tiered engine stack live: static tier handles a plain page, and a
/// NeedsJs intent escalates to the installed light engine (Lightpanda). With
/// the light engine pointed at a missing binary, escalation lands on real
/// headless Chrome (the TODO's "escalates to Chrome" check, adapted: Obscura
/// isn't installed, so we exercise the gap path with it).
#[test]
#[ignore]
fn live_tiered_stack_escalation() {
    if !live_enabled() {
        return;
    }
    use crate::tiers::{EngineConfig, EngineTier, FetchIntent, LightEngine, TieredEngine};

    let engine = TieredEngine::new(EngineConfig::default());

    // Static intent: tier 0 handles a plain page with no browser process.
    let r0 = engine
        .fetch("https://example.com/", FetchIntent::Static)
        .expect("static fetch");
    assert_eq!(r0.tier, EngineTier::Static);
    assert!(
        r0.markdown.contains("Example Domain"),
        "static tier should read example.com:\n{}",
        r0.markdown
    );

    // NeedsJs intent: escalates to the light engine (Lightpanda, installed).
    // Environment-honest: Lightpanda is optional — where it is missing the
    // designed gap path escalates to real headless Chrome (unit-proven in
    // tiers.rs), and the live assertion must accept that instead of failing
    // an env without the optional engine.
    let lightpanda_present = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .any(|d| d.join("lightpanda").is_file() || d.join("lightpanda.exe").is_file())
        })
        .unwrap_or(false);
    let r1 = engine
        .fetch("https://example.com/", FetchIntent::NeedsJs)
        .expect("light fetch");
    if lightpanda_present {
        assert!(
            matches!(r1.tier, EngineTier::Lightpanda),
            "expected Lightpanda tier, got {:?}",
            r1.tier
        );
    } else {
        eprintln!("LIVE NOTE: lightpanda not installed — asserting the Chrome gap path");
        assert_eq!(r1.tier, EngineTier::Chrome);
    }
    assert!(
        r1.markdown.contains("Example Domain"),
        "light tier should render example.com:\n{}",
        r1.markdown
    );

    // Light-engine capability gap (Obscura not installed) → escalates to
    // real headless Chrome (tier 2).
    let engine2 = TieredEngine::new(EngineConfig {
        light_engine: LightEngine::Obscura,
        obscura_bin: Some("/nonexistent/obscura".into()),
        ..Default::default()
    });
    let r2 = engine2
        .fetch("https://example.com/", FetchIntent::NeedsJs)
        .expect("escalate to chrome");
    assert_eq!(r2.tier, EngineTier::Chrome);
    assert!(
        r2.markdown.contains("Example Domain"),
        "chrome tier should render example.com:\n{}",
        r2.markdown
    );

    eprintln!("LIVE PASS: tiered stack — static → light → chrome escalation");
}

/// E15 Electron E2E — attach to a live Electron debug port, snapshot →
/// click-by-ref → read. Gate: `AGENTCOWORK_LIVE_TEST=1` + `AGENTCOWORK_ELECTRON_PORT`.
/// A local Electron app is required (e.g. launch VS Code with
/// `--remote-debugging-port=9223`); the test skips silently otherwise.
#[test]
#[ignore]
fn live_electron_attach_snapshot_click_read() {
    if !live_enabled() {
        return;
    }
    let Some(port) =
        agentcowork_types::env_compat::get("ELECTRON_PORT").and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skip: AGENTCOWORK_ELECTRON_PORT not set");
        return;
    };

    // `attach` already runs the Electron probe (Browser: Electron/…) and
    // errors on any non-Electron target, so a successful attach is the check.
    let handle = crate::ElectronHandle::attach(port).expect("attach to Electron debug port");
    assert!(
        !handle.app.browser_ws_url.is_empty(),
        "Electron ws url present"
    );

    // snapshot → a tree with at least a root WebArea + some nodes.
    let snap = handle
        .snapshot("electron-e2e", SnapshotMode::Interactive)
        .expect("snapshot Electron window");
    let rendered = snap.root.render();
    eprintln!("=== ELECTRON SNAPSHOT ===\n{rendered}\n=== END ===");
    assert!(!rendered.trim().is_empty(), "snapshot must not be empty");

    // click the first actionable ref (if any) — proves the ref→geometry→click
    // path works against a real Electron target.
    let first_ref = crate::first_actionable_ref(&snap.root);
    if let Some(r) = first_ref {
        handle.click_ref(&r).expect("click by ref");
        eprintln!("ELECTRON PASS: clicked ref {r}");
    } else {
        eprintln!("ELECTRON PASS: no actionable ref in this window (attach + snapshot only)");
    }

    // read → the window's visible text comes back through Runtime.evaluate.
    let text = handle.read().expect("read Electron window");
    eprintln!("=== ELECTRON READ ({} chars) ===\n{}", text.len(), text);

    // screenshot → non-empty base64 PNG proves the render path works.
    let png = handle.screenshot().expect("screenshot Electron window");
    assert!(!png.is_empty(), "screenshot must return a PNG payload");
    eprintln!("ELECTRON PASS: attach → snapshot → click → read → screenshot");
}

#[test]
#[ignore]
fn live_session_inheritance_pulls_cookies_from_chrome_debug_port() {
    if !live_enabled() {
        return;
    }
    // E13: the "user's" Chrome already runs with a discoverable debug port.
    // Spawn it with `--remote-debugging-port=0`, then discover the real port
    // the way the inheritance path does (DevToolsActivePort → probe).
    let user_data_dir =
        std::env::temp_dir().join(format!("agentcowork-live-inherit-{}", std::process::id()));
    let opts = agentcowork_cdp::LaunchOptions {
        headless: true,
        user_data_dir: user_data_dir.clone(),
        ..Default::default()
    };
    let child = agentcowork_cdp::spawn_browser(&opts).expect("spawn Chrome");

    let endpoint = agentcowork_cdp::read_devtools_active_port(&user_data_dir)
        .expect("read DevToolsActivePort");
    let port = url::Url::parse(&endpoint.browser_ws_url)
        .expect("parse ws url")
        .port()
        .expect("ws url has a port");

    // Simulate the user's existing session: set a cookie on connection #1.
    let client = agentcowork_cdp::connect_to_browser(child.endpoint()).expect("connect");
    let page = client
        .list_targets()
        .expect("targets")
        .into_iter()
        .find(|t| t.target_type == TargetType::Page)
        .expect("a page target");
    let session = client.attach(&page.target_id).expect("attach");
    client
        .call_session(
            &session.session_id,
            "Network.setCookies",
            serde_json::json!({ "cookies": [{
                "name": "inherit-test",
                "value": "yes",
                "domain": "example.com",
                "path": "/",
                "httpOnly": false,
                "secure": false,
                "sameSite": "Lax"
            }] }),
        )
        .expect("set cookie");

    // E13: inherit via the discovered debug port on a fresh connection.
    let buckets = crate::session::inherit_cookies_from_chrome(port, Some(&user_data_dir), true)
        .expect("inherit cookies");
    let found = buckets.iter().any(|(site, cookies)| {
        site == "example.com" && cookies.iter().any(|c| c.name == "inherit-test")
    });
    assert!(
        found,
        "inherited cookies must include the cookie set on the user's session"
    );
    eprintln!("LIVE PASS: session inheritance via debug port (E13)");

    drop(child);
}
