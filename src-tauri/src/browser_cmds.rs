//! P11.5.3 — browse view over a real CDP session. `browser_start` spawns a
//! headless Chrome (chrome-for-testing fallback), connects through
//! `agentcowork-cdp`, attaches the first page target, and holds the session in
//! `AppState.browser`. The UI drives it with `browser_navigate` /
//! `browser_snapshot` / `browser_read` / `browser_click` / `browser_type` and
//! tears it down with `browser_stop`. Every call is the real engine — the
//! same code the P2.1–P2.3 LIVE tests drive against real Chrome.
//!
//! Honest ceilings: the session is a fresh isolated headless profile (not the
//! user's default Chrome profile — no session inheritance here, that's the
//! E13 seam); `browser_snapshot` returns the a11y tree text, not a rendered
//! page bitmap (screenshots are a catalog tool, wired separately).
//!
//! P55.7 — the interactive session is tier 2 by definition (scripting a page
//! needs a full engine), and `browser_start` now says so instead of leaving the
//! tier implicit. Read-only fetches do not need it: [`browser_read_url`] runs
//! the E10 tiered stack (static → Lightpanda/Obscura → Chrome) and reports the
//! tier that actually served the read, so the app never implies a light engine
//! is doing work it is not doing — and uses one for real when it is present.

use tauri::State;

use crate::AppState;

/// Wait briefly for the page target a just-created `about:blank` produces.
///
/// Chrome does not always publish the new target on the very next
/// `Target.list` call, so a single immediate read is not enough. This used to
/// be an `.expect("page target after create")`, which killed the whole desktop
/// app when Chrome was merely slow — the opposite of what an optional browse
/// view should be able to do. Polls for up to 3s, then returns an honest
/// error.
pub(crate) fn wait_for_page_target(
    client: &agentcowork_cdp::CdpClient,
) -> Result<agentcowork_cdp::TargetInfo, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last_err: Option<String> = None;
    loop {
        match client.list_targets() {
            Ok(ts) => {
                if let Some(t) = ts
                    .into_iter()
                    .find(|t| t.target_type == agentcowork_cdp::TargetType::Page)
                {
                    return Ok(t);
                }
            }
            Err(e) => last_err = Some(format!("list targets: {e}")),
        }
        if std::time::Instant::now() >= deadline {
            return Err(last_err.unwrap_or_else(|| {
                "no page target appeared after Target.createTarget (3s)".to_string()
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The live browser session held in `AppState.browser`.
pub struct LiveBrowser {
    /// Owns the browser child — `BrowserChild::drop` kills the process when
    /// the session is cleared (browser_stop / app teardown). Never read
    /// directly; its Drop is the whole point.
    #[allow(dead_code)]
    child: agentcowork_cdp::BrowserChild,
    client: std::sync::Arc<agentcowork_browser::tiers::CdpNetworkGuard>,
    session_id: String,
    url: String,
    channel: agentcowork_cdp::BrowserChannel,
    browser_name: String,
    browser_version: Option<String>,
}

/// Shared CDP backend injected into the agent `ToolService` so browser.*
/// tools on the loop hit the same session as the browse view.
struct LoopBrowser {
    client: std::sync::Arc<agentcowork_browser::tiers::CdpNetworkGuard>,
    session_id: String,
}

fn browser_config_path() -> std::path::PathBuf {
    agentcowork_core::default_data_dir().join("browser_config.json")
}

pub fn load_browser_config() -> agentcowork_cdp::BrowserConfig {
    let path = browser_config_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<agentcowork_cdp::BrowserConfig>(&data) {
            return cfg;
        }
    }
    agentcowork_cdp::BrowserConfig::default()
}

pub fn save_browser_config(cfg: &agentcowork_cdp::BrowserConfig) -> Result<(), String> {
    let path = browser_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(())
}

/// Discover all supported installed browsers on the host system.
#[tauri::command]
pub fn browser_list_installed() -> Result<Vec<agentcowork_cdp::BrowserCandidate>, String> {
    Ok(agentcowork_cdp::discover_installed_browsers())
}

/// Get the current user browser configuration.
#[tauri::command]
pub fn browser_get_config() -> Result<agentcowork_cdp::BrowserConfig, String> {
    Ok(load_browser_config())
}

/// Update the user browser configuration.
#[tauri::command]
pub fn browser_set_config(
    config: agentcowork_cdp::BrowserConfig,
) -> Result<agentcowork_cdp::BrowserConfig, String> {
    save_browser_config(&config)?;
    Ok(config)
}

impl agentcowork_core::BrowserBackend for LoopBrowser {
    fn save_pdf_enhanced(&self, dir: &std::path::Path) -> Result<String, String> {
        let path = dir.join("page.pdf");
        let res = self
            .client
            .call_session(
                &self.session_id,
                "Page.printToPDF",
                serde_json::json!({ "printBackground": true }),
            )
            .map_err(|e| e.to_string())?;
        let b64 = res
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or("printToPDF missing data")?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        ensure_network_guard_clean(&self.client)?;
        Ok(path.display().to_string())
    }

    fn save_screenshot_enhanced(
        &self,
        dir: &std::path::Path,
        quality: u8,
    ) -> Result<String, String> {
        let path = dir.join("shot.jpg");
        let res = self
            .client
            .call_session(
                &self.session_id,
                "Page.captureScreenshot",
                serde_json::json!({ "format": "jpeg", "quality": quality }),
            )
            .map_err(|e| e.to_string())?;
        let b64 = res
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or("captureScreenshot missing data")?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        ensure_network_guard_clean(&self.client)?;
        Ok(path.display().to_string())
    }

    fn snapshot(&self) -> Result<String, String> {
        let actions = agentcowork_browser::BrowserActions::new(&*self.client, Some(&self.session_id));
        let snap = actions.snapshot("loop").map_err(|e| e.to_string())?;
        ensure_network_guard_clean(&self.client)?;
        Ok(snap.root.render())
    }

    fn navigate(&self, url: &str) -> Result<String, String> {
        self.client
            .navigate(
                &self.session_id,
                url,
                std::time::Duration::from_millis(800),
                true,
            )
            .map_err(|error| format!("browser navigate: {error}"))
    }

    fn act(
        &self,
        kind: &str,
        selector: Option<&str>,
        text: Option<&str>,
    ) -> Result<String, String> {
        let actions = agentcowork_browser::BrowserActions::new(&*self.client, Some(&self.session_id));
        let kind = kind.to_lowercase();
        match kind.as_str() {
            "click" => {
                let r = selector.ok_or("ref required")?;
                actions
                    .act(agentcowork_browser::ActKind::Click {
                        ref_id: r.to_string(),
                    })
                    .map_err(|e| e.to_string())?;
            }
            "type" => {
                let r = selector.ok_or("ref required")?;
                actions
                    .act(agentcowork_browser::ActKind::Type {
                        ref_id: r.to_string(),
                        text: text.unwrap_or("").to_string(),
                    })
                    .map_err(|e| e.to_string())?;
            }
            other => return Err(format!("unsupported act kind: {other}")),
        }
        ensure_network_guard_clean(&self.client)?;
        Ok(kind)
    }
}

fn lock_browser<'a>(
    state: &'a State<'_, AppState>,
) -> Result<std::sync::MutexGuard<'a, Option<LiveBrowser>>, String> {
    state.browser.lock().map_err(|e| e.to_string())
}

fn actions(
    b: &LiveBrowser,
) -> agentcowork_browser::BrowserActions<'_, agentcowork_browser::tiers::CdpNetworkGuard> {
    agentcowork_browser::BrowserActions::new(&*b.client, Some(&b.session_id))
}

fn ensure_network_guard_clean(
    client: &agentcowork_browser::tiers::CdpNetworkGuard,
) -> Result<(), String> {
    client.pump_network_guard();
    if let Some(error) = client.take_network_error() {
        return Err(format!("browser network guard: {error}"));
    }
    Ok(())
}

/// Spawn + connect a browser session (idempotent — returns current status if
/// already attached). Uses the configured browser binary and isolates profiles per channel.
#[tauri::command]
pub fn browser_start(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    {
        let guard = lock_browser(&state)?;
        if let Some(b) = guard.as_ref() {
            ensure_network_guard_clean(&b.client)?;
            return Ok(serde_json::json!({
                "attached": true,
                "url": b.url,
                "fresh": false,
                "channel": b.channel,
                "name": b.browser_name,
                "version": b.browser_version,
            }));
        }
    }

    let config = load_browser_config();
    let (resolved_bin, channel) =
        agentcowork_cdp::resolve_browser_binary(&config).map_err(|e| e.to_string())?;
    let browser_version = agentcowork_cdp::probe_browser_version(&resolved_bin);
    let browser_name = channel.display_name().to_string();

    let channel_dir_name = match channel {
        agentcowork_cdp::BrowserChannel::Brave => "brave",
        agentcowork_cdp::BrowserChannel::Chrome => "chrome",
        agentcowork_cdp::BrowserChannel::Edge => "edge",
        agentcowork_cdp::BrowserChannel::Chromium => "chromium",
        agentcowork_cdp::BrowserChannel::Arc => "arc",
        agentcowork_cdp::BrowserChannel::Vivaldi => "vivaldi",
        agentcowork_cdp::BrowserChannel::Custom => "custom",
        agentcowork_cdp::BrowserChannel::Auto => "auto",
    };

    let profile = agentcowork_core::default_data_dir()
        .join("browser-profiles")
        .join(channel_dir_name);
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;

    let mut extra_args = vec!["--mute-audio".to_string()];
    for arg in &config.extra_args {
        if !extra_args.contains(arg) {
            extra_args.push(arg.clone());
        }
    }

    let opts = agentcowork_cdp::LaunchOptions {
        user_data_dir: profile,
        headless: config.headless,
        browser_binary: Some(resolved_bin),
        extra_args,
        wait_timeout: std::time::Duration::from_secs(30),
    };
    let child = agentcowork_cdp::spawn_browser(&opts).map_err(|e| format!("spawn browser: {e}"))?;
    let endpoint = child.endpoint().clone();
    let client =
        agentcowork_cdp::connect_to_browser(&endpoint).map_err(|e| format!("connect: {e}"))?;
    let targets = client
        .list_targets()
        .map_err(|e| format!("list targets: {e}"))?;
    let page = match targets
        .iter()
        .find(|t| t.target_type == agentcowork_cdp::TargetType::Page)
        .cloned()
    {
        Some(page) => page,
        None => {
            client
                .call(
                    "Target.createTarget",
                    serde_json::json!({ "url": "about:blank" }),
                )
                .map_err(|e| format!("create target: {e}"))?;
            wait_for_page_target(&client)?
        }
    };
    let session = client
        .attach(&page.target_id)
        .map_err(|e| format!("attach: {e}"))?;

    let client = std::sync::Arc::new(client);
    let client = std::sync::Arc::new(
        agentcowork_browser::tiers::CdpNetworkGuard::enable(
            std::sync::Arc::clone(&client),
            &session.session_id,
            agentcowork_guard::netfloor::NetPolicy::strict(),
            Vec::new(),
        )
        .map_err(|error| format!("enable browser network guard: {error}"))?,
    );
    let live = LiveBrowser {
        child,
        client: std::sync::Arc::clone(&client),
        session_id: session.session_id.clone(),
        url: "about:blank".to_string(),
        channel,
        browser_name: browser_name.clone(),
        browser_version: browser_version.clone(),
    };
    {
        let mut guard = lock_browser(&state)?;
        *guard = Some(live);
    }
    if let Ok(relay) = state.chat_relay.lock() {
        if let Some(r) = relay.as_ref() {
            r.attach_browser(std::sync::Arc::new(LoopBrowser {
                client,
                session_id: session.session_id,
            }));
        }
    }
    Ok(serde_json::json!({
        "attached": true,
        "url": "about:blank",
        "fresh": true,
        "channel": channel,
        "name": browser_name,
        "version": browser_version,
    }))
}

/// Navigate the attached page to a URL and wait for the load to settle.
#[tauri::command]
pub fn browser_navigate(
    state: State<'_, AppState>,
    url: String,
) -> Result<serde_json::Value, String> {
    let mut guard = lock_browser(&state)?;
    let b = guard
        .as_mut()
        .ok_or("browser not attached — start it first")?;
    let final_url = b
        .client
        .navigate(
            &b.session_id,
            &url,
            std::time::Duration::from_millis(1500),
            true,
        )
        .map_err(|error| format!("browser navigate: {error}"))?;
    b.url = final_url.clone();
    // P48.3 — human-initiated UI action: authorized by the user's own gesture
    // (navigate is side-effecting if the destination page mutates on load),
    // audited on the same Merkle chain as every other effect.
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "browser.navigate",
        serde_json::json!({ "url": final_url }),
    );
    Ok(serde_json::json!({ "url": final_url }))
}

/// Accessibility snapshot of the current page (the P2.2 tree text).
#[tauri::command]
pub fn browser_snapshot(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let guard = lock_browser(&state)?;
    let b = guard
        .as_ref()
        .ok_or("browser not attached — start it first")?;
    let snap = actions(b)
        .snapshot("browse")
        .map_err(|e| format!("snapshot: {e}"))?;
    ensure_network_guard_clean(&b.client)?;
    Ok(serde_json::json!({
        "url": b.url,
        "documentId": snap.document_id,
        "text": snap.root.render(),
    }))
}

/// Clean markdown read of the current page (P2.3 read tool, Full mode).
#[tauri::command]
pub fn browser_read(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let guard = lock_browser(&state)?;
    let b = guard
        .as_ref()
        .ok_or("browser not attached — start it first")?;
    let out = actions(b)
        .read(agentcowork_browser::ReadMode::Full)
        .map_err(|e| format!("read: {e}"))?;
    ensure_network_guard_clean(&b.client)?;
    Ok(serde_json::json!({ "url": b.url, "text": out.text }))
}

/// Click an a11y ref from the snapshot (`[ref=eN]`).
#[tauri::command]
pub fn browser_click(
    state: State<'_, AppState>,
    ref_id: String,
) -> Result<serde_json::Value, String> {
    let guard = lock_browser(&state)?;
    let b = guard
        .as_ref()
        .ok_or("browser not attached — start it first")?;
    let res = actions(b)
        .act(agentcowork_browser::ActKind::Click {
            ref_id: ref_id.clone(),
        })
        .map_err(|e| format!("click {ref_id}: {e}"))?;
    ensure_network_guard_clean(&b.client)?;
    let (added, removed) = match res.diff.as_ref() {
        Some(d) => (d.added_lines.clone(), d.removed_lines.clone()),
        None => (Vec::new(), Vec::new()),
    };
    // P48.3 — human-initiated UI click: a click can submit a form / trigger a
    // side effect on the page, so it is gesture-authorized + audited.
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "browser.click",
        serde_json::json!({ "refId": ref_id }),
    );
    Ok(serde_json::json!({
        "ok": true,
        "refId": ref_id,
        "added": added,
        "removed": removed,
    }))
}

/// Type text into a focused field (uses the ref's geometry when provided,
/// else the focused element).
#[tauri::command]
pub fn browser_type(
    state: State<'_, AppState>,
    ref_id: Option<String>,
    text: String,
) -> Result<serde_json::Value, String> {
    let guard = lock_browser(&state)?;
    let b = guard
        .as_ref()
        .ok_or("browser not attached — start it first")?;
    let act = match ref_id.clone() {
        Some(id) => agentcowork_browser::ActKind::Type {
            ref_id: id.clone(),
            text: text.clone(),
        },
        None => agentcowork_browser::ActKind::TypeAt {
            x: 0.0,
            y: 0.0,
            text: text.clone(),
        },
    };
    let res = actions(b).act(act).map_err(|e| format!("type: {e}"))?;
    ensure_network_guard_clean(&b.client)?;
    let added = match res.diff.as_ref() {
        Some(d) => d.added_lines.clone(),
        None => Vec::new(),
    };
    // P48.3 — typing on a live page can mutate state / submit forms; gesture
    // -authorized + audited on the same chain as every other effect.
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "browser.type",
        serde_json::json!({ "refId": ref_id, "chars": text.chars().count() }),
    );
    Ok(serde_json::json!({ "ok": true, "added": added }))
}

/// P55.7 — read a URL through the E10 tiered engine stack, without a live CDP
/// session: tier 0 static extraction, then the configured light engine
/// (Lightpanda / Obscura) when the page needs JS, then Chrome. The response
/// names the tier and read source that actually produced the text, so the
/// surface can label it honestly instead of assuming a light engine ran.
///
/// The stack's own containments apply on every tier (SSRF/private-network and
/// `file://` blocked by default, optional `allowed_domains`), and a policy
/// rejection is **not** escalated — a heavier engine would hit the same wall.
#[tauri::command]
pub async fn browser_read_url(
    url: String,
    needs_js: Option<bool>,
) -> Result<serde_json::Value, String> {
    let intent = if needs_js.unwrap_or(false) {
        agentcowork_browser::FetchIntent::NeedsJs
    } else {
        agentcowork_browser::FetchIntent::Static
    };
    let engine = agentcowork_browser::TieredEngine::new(agentcowork_browser::EngineConfig::default());
    let url_for_log = url.clone();
    // Blocking HTTP/CDP work off the async runtime (this is a plain command
    // thread, but the tiered stack spawns a child browser on escalation).
    let result = tauri::async_runtime::spawn_blocking(move || engine.fetch(&url, intent))
        .await
        .map_err(|e| format!("tiered read join: {e}"))?;
    match result {
        Ok(out) => Ok(serde_json::json!({
            "url": url_for_log,
            "tier": out.tier,
            "source": out.source,
            "truncated": out.truncated,
            "text": out.markdown,
        })),
        Err(e) => Err(format!("tiered read failed: {e}")),
    }
}

/// Tear down the browser session (kills the Chrome child).
#[tauri::command]
pub fn browser_stop(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let mut guard = lock_browser(&state)?;
    let was_attached = guard.is_some();
    *guard = None; // Drop kills + waits the child (BrowserChild::drop)
    Ok(serde_json::json!({ "stopped": was_attached }))
}

#[cfg(test)]
mod tiered_read_tests {
    //! P55.7 — the tiered read's policy floors, asserted on the product path.
    //! Both refusals happen **before** any engine starts, so these run offline
    //! and with no display: a private-network target must never be escalated to
    //! a heavier engine, and `file://` is refused outright.

    fn read(url: &str) -> Result<serde_json::Value, String> {
        tauri::async_runtime::block_on(super::browser_read_url(url.to_string(), None))
    }

    #[test]
    fn private_network_targets_are_refused_not_escalated() {
        let err = read("http://127.0.0.1:8080/").expect_err("loopback must be refused");
        assert!(err.contains("SSRF"), "expected the SSRF floor, got: {err}");
    }

    #[test]
    fn file_urls_are_refused() {
        let err = read("file:///etc/passwd").expect_err("file:// must be refused");
        assert!(
            err.contains("file://") || err.contains("file:"),
            "expected the file:// floor, got: {err}"
        );
    }
}

/// Status probe for the rail live-dot.
#[tauri::command]
pub fn browser_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let guard = lock_browser(&state)?;
    match guard.as_ref() {
        // P55.7 — `engine` is a fact, not a default: the attached session is
        // always the full engine, and saying so stops any surface implying the
        // tier-1 light engine is serving interaction.
        Some(b) => {
            ensure_network_guard_clean(&b.client)?;
            Ok(serde_json::json!({
                "attached": true,
                "url": b.url,
                "engine": "chrome",
                "channel": b.channel,
                "name": b.browser_name,
                "version": b.browser_version,
            }))
        }
        None => Ok(serde_json::json!({ "attached": false })),
    }
}
