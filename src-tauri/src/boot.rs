//! Boot-time "small boot" glue (Fix 1c).
//!
//! Self-contained side systems the Tauri shell brings up alongside the window
//! and the coordinator sidecar. Kept out of `lib.rs` (which now owns only the
//! app assembly, state wiring and command registration) so adding or tuning a
//! side system does not require reading the whole command surface.
//!
//! Deliberately *not* moved here: the coordinator-relay wiring
//! (`connect_chat_relay`, `locate_coordinator_bin`, `pre_spawn_coordinator`,
//! `serve_unix_control_channel`) — it is one cohesive cluster feeding the
//! chat/AG-UI/task command surface and shares its constants, so it lives with
//! the commands in `lib.rs`.

use tauri::Manager;

/// P2.11 (E16) — spawn the WebMCP HTTP server on a loopback port so browser
/// sessions can serve MCP tools (`tools/list` + `tools/call`) to any local
/// HTTP client. The registry mirrors the 37-tool browser catalog; tool calls
/// fail honestly until a live browser session is attached (the executor is a
/// "not attached" stub — the engine itself lives in `agentcowork-browser`).
pub fn spawn_webmcp_server() {
    use agentcowork_browser::webmcp::{WebMcpExecutor, WebMcpRegistry, WebMcpResult, WebMcpTool};
    use agentcowork_mcp::ArgKind;
    use serde_json::json;

    let mut registry = WebMcpRegistry::new();
    for def in agentcowork_mcp::BROWSER_TOOLS {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for a in def.args {
            let ty = match a.kind {
                ArgKind::String => "string",
                ArgKind::Number => "number",
                ArgKind::Bool => "boolean",
                ArgKind::StringArray => "array",
                ArgKind::Object => "object",
            };
            let mut spec = json!({ "type": ty, "description": a.description });
            if a.kind == ArgKind::StringArray {
                spec["items"] = json!({ "type": "string" });
            }
            properties.insert(a.name.to_string(), spec);
            if a.required {
                required.push(serde_json::Value::String(a.name.to_string()));
            }
        }
        registry.register(WebMcpTool {
            name: def.name.to_string(),
            description: def.description.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": required,
            }),
        });
    }

    struct NotAttached;
    impl WebMcpExecutor for NotAttached {
        fn execute(&self, tool: &WebMcpTool, _input: serde_json::Value) -> WebMcpResult {
            WebMcpResult::err(format!(
                "browser session not attached — {} is catalog-only until a CDP session is wired",
                tool.name
            ))
        }
    }

    match agentcowork_browser::webmcp_http::McpHttpServer::serve(
        "127.0.0.1:0",
        registry,
        std::sync::Arc::new(NotAttached),
    ) {
        Ok(server) => match server.local_addr() {
            Ok(addr) => eprintln!(
                "agentcowork-desktop: WebMCP HTTP listening on http://{addr}/mcp (token {})",
                server.token()
            ),
            Err(e) => eprintln!("agentcowork-desktop: WebMCP addr lookup failed: {e}"),
        },
        Err(e) => eprintln!("agentcowork-desktop: WebMCP server spawn failed (continuing): {e}"),
    }
}

/// System tray (P0.2 task 17 / H11): status icon + Show/Run-automations/Quit
/// menu. Scheduled tasks execute headless via the coordinator's own due-loop;
/// the tray item just forces a manual tick (works with the window hidden).
pub fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "Show AgentCowork", true, None::<&str>)?;
    let run = MenuItem::with_id(
        app,
        "run-automations",
        "Run automations now",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &run, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray");
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let _tray = builder
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
            // H11: force a due-check + execution pass headless (no window).
            // P71.2c — the host fires: each due job becomes Work + Run and
            // runs through its session's bound agent over ACP
            // (`ARCH/AUTOMATION.md` §9 — agent execution is never the
            // scheduler's). Fire-and-forget: failures are filed as run-level
            // incidents by the firing path itself.
            "run-automations" => {
                let _ = crate::scheduler_fire::fire_due(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}
