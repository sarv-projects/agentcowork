//! P2.5 tests: limits, browser SDK, InnerCallHook, ownership, audit trail.

use std::path::Path;
use std::sync::{Arc, Mutex};

use super::*;

// ---------------------------------------------------------------------------
// MockBrowser — a test `BrowserHost` with an in-memory page registry, an
// optional NDJSON audit trail, and a call log.
// ---------------------------------------------------------------------------

struct MockBrowser {
    pages: Mutex<Vec<PageInfo>>,
    claims: Mutex<Vec<String>>,
    calls: Mutex<Vec<String>>,
    audit: Mutex<Option<agentcowork_audit::AuditWriter>>,
}

impl MockBrowser {
    fn new() -> Self {
        Self {
            pages: Mutex::new(vec![
                PageInfo {
                    id: "p-user".into(),
                    url: "https://user.example/".into(),
                    title: "user's page".into(),
                    ownership: PageOwnership::User,
                },
                PageInfo {
                    id: "p-other".into(),
                    url: "https://other.example/".into(),
                    title: "other agent's page".into(),
                    ownership: PageOwnership::OtherAgent,
                },
            ]),
            claims: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            audit: Mutex::new(None),
        }
    }

    fn with_audit(path: &Path) -> Self {
        let w = agentcowork_audit::AuditWriter::open(path).unwrap();
        Self {
            audit: Mutex::new(Some(w)),
            ..Self::new()
        }
    }
}

impl BrowserHost for MockBrowser {
    fn authorize(&self, call: &PrimitiveCall) -> Result<(), SandboxError> {
        // Scripts may only act on pages they own (or just created); every
        // page-creation is always allowed.
        if let Some(pid) = &call.page_id {
            if call.name != "pages.newPage" {
                let owned = self
                    .pages
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|p| p.id == *pid && matches!(p.ownership, PageOwnership::Mine));
                let claimed = self.claims.lock().unwrap().contains(pid);
                if !owned && !claimed {
                    return Err(SandboxError::Primitive(
                        call.name.clone(),
                        format!("page {pid} is not owned by this script"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn record(&self, call: &PrimitiveCall, ok: bool, error: &str) -> Result<(), SandboxError> {
        self.calls.lock().unwrap().push(call.name.clone());
        if let Some(w) = self.audit.lock().unwrap().as_mut() {
            w.write(
                "script.primitive",
                serde_json::json!({
                    "primitive": call.name,
                    "page_id": call.page_id,
                    "ok": ok,
                    "error": if error.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(error.to_string())
                    },
                }),
            )
            .map_err(|e| SandboxError::Runtime(e.to_string()))?;
        }
        Ok(())
    }

    fn on_page_created(
        &self,
        page_id: &str,
        _created_from: &PrimitiveCall,
    ) -> Result<(), SandboxError> {
        self.claims.lock().unwrap().push(page_id.to_string());
        if let Some(w) = self.audit.lock().unwrap().as_mut() {
            w.write(
                "script.page_created",
                serde_json::json!({ "page_id": page_id }),
            )
            .map_err(|e| SandboxError::Runtime(e.to_string()))?;
        }
        Ok(())
    }

    fn pages(&self) -> Vec<PageInfo> {
        self.pages.lock().unwrap().clone()
    }

    fn exec(&self, call: &PrimitiveCall) -> Result<serde_json::Value, SandboxError> {
        match call.name.as_str() {
            "pages.newPage" => {
                let id = format!("p-{}", self.claims.lock().unwrap().len() + 100);
                let url = call
                    .args
                    .get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("about:blank")
                    .to_string();
                let mut pages = self.pages.lock().unwrap();
                let title = String::new();
                pages.push(PageInfo {
                    id: id.clone(),
                    url,
                    title,
                    ownership: PageOwnership::Mine,
                });
                Ok(serde_json::json!({ "id": id, "url": call.args.get("url") }))
            }
            "pages.close" => {
                let pid = call.page_id.clone().unwrap_or_default();
                let mut pages = self.pages.lock().unwrap();
                let before = pages.len();
                pages.retain(|p| p.id != pid);
                if pages.len() == before {
                    return Err(SandboxError::Primitive(
                        call.name.clone(),
                        format!("page {pid} not found"),
                    ));
                }
                Ok(serde_json::json!({ "closed": pid }))
            }
            "pages.list" => {
                serde_json::to_value(self.pages()).map_err(|e| SandboxError::Runtime(e.to_string()))
            }
            "pages.getInfo" => {
                let pid = call.page_id.clone().unwrap_or_default();
                let p = self
                    .pages
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|p| p.id == pid)
                    .cloned()
                    .ok_or_else(|| {
                        SandboxError::Primitive(call.name.clone(), format!("page {pid} not found"))
                    })?;
                serde_json::to_value(p).map_err(|e| SandboxError::Runtime(e.to_string()))
            }
            "nav.goto" => Ok(serde_json::json!({ "ok": true, "url": call.args.get("url") })),
            "read" => Ok(
                serde_json::json!({ "text": "# Mock page\nsome content", "url": "https://example.com" }),
            ),
            "grep" => Ok(serde_json::json!({ "matches": 2 })),
            "wait" => Ok(serde_json::json!({ "ok": true })),
            "evaluate" => Ok(serde_json::json!({ "result": 42 })),
            "screenshot" => Ok(serde_json::json!({ "ok": true, "path": "/tmp/mock.png" })),
            "cdp" => Ok(serde_json::json!({ "ok": true, "method": call.args.get("method") })),
            other => Err(SandboxError::Primitive(
                other.to_string(),
                "unsupported in mock host".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers + tests
// ---------------------------------------------------------------------------

fn sandbox_with(limits: SandboxLimits) -> (Sandbox, Arc<MockBrowser>) {
    let host = Arc::new(MockBrowser::new());
    let sb = Sandbox::new(limits, Arc::clone(&host) as Arc<dyn BrowserHost>);
    (sb, host)
}

fn default_sandbox() -> (Sandbox, Arc<MockBrowser>) {
    sandbox_with(SandboxLimits::default())
}

fn eval_json(sb: &Sandbox, code: &str) -> serde_json::Value {
    serde_json::from_str(&sb.eval(code).unwrap()).unwrap()
}

#[test]
fn default_limits_match_spec() {
    let l = SandboxLimits::default();
    assert_eq!(l.max_heap_bytes, 64 * 1024 * 1024);
    assert_eq!(l.max_stack_bytes, 512 * 1024);
    assert_eq!(l.timeout_secs, 30);
    assert_eq!(l.max_log_lines, 1024);
    assert_eq!(l.max_return_bytes, 2 * 1024 * 1024);
}

#[test]
fn error_messages_are_model_friendly() {
    let e = SandboxError::Timeout(30);
    assert!(e.to_string().contains("timeout"));
    let e2 = SandboxError::ReturnTooLarge(3_000_000, 2 * 1024 * 1024);
    assert!(e2.to_string().contains("large"));
}

#[test]
fn basic_expression_evaluates() {
    let (sb, _) = default_sandbox();
    let out = eval_json(&sb, "1 + 1");
    assert_eq!(out["result"], 2);
    assert_eq!(out["logs_truncated"], false);
}

#[test]
fn top_level_await_and_sdk_primitives_work() {
    let (sb, host) = default_sandbox();
    let code = r#"
        const p = await browser.pages.newPage("https://example.com");
        await browser.nav(p.id).goto("https://example.com");
        const s = await browser.read(p.id);
        const g = await browser.grep(p.id, "content");
        await browser.pages.close(p.id);
        ({ pages: (await browser.pages.list()).length, readChars: s.text.length, matches: g.matches });
    "#;
    let out = eval_json(&sb, code);
    assert_eq!(out["result"]["matches"], 2);
    assert_eq!(out["result"]["pages"], 2); // p-user + p-other remain (script closed only its own)
    // every primitive was recorded, in order — nothing bypassed the hook
    let calls = host.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            "pages.newPage",
            "nav.goto",
            "read",
            "grep",
            "pages.close",
            "pages.list"
        ]
    );
    // the created page was claimed
    assert_eq!(host.claims.lock().unwrap().len(), 1);
}

#[test]
fn pages_list_returns_ownership_labels() {
    let (sb, host) = default_sandbox();
    let out = eval_json(
        &sb,
        r#"
            await browser.pages.newPage("https://mine.example/");
            const l = await browser.pages.list();
            l.map(p => ({ id: p.id, ownership: p.ownership })).sort((a, b) => a.id.localeCompare(b.id));
        "#,
    );
    let list = out["result"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.contains(&serde_json::json!({"id": "p-user", "ownership": "user"})));
    assert!(list.contains(&serde_json::json!({"id": "p-other", "ownership": "other-agent"})));
    // the page this script created is labeled "mine"
    let created = host.claims.lock().unwrap().first().unwrap().clone();
    assert!(list.contains(&serde_json::json!({"id": created, "ownership": "mine"})));
}

#[test]
fn foreign_tabs_are_blocked_and_still_audited() {
    let (sb, host) = default_sandbox();
    let out = eval_json(
        &sb,
        r#"
            try {
                await browser.pages.close("p-user");
                "no-error";
            } catch (e) {
                String(e);
            }
        "#,
    );
    let msg = out["result"].as_str().unwrap();
    assert!(msg.contains("not owned"), "got: {msg}");
    // the denied attempt was still recorded (trail cannot be bypassed)
    let calls = host.calls.lock().unwrap().clone();
    assert_eq!(calls, vec!["pages.close"]);
}

#[test]
fn timeout_interrupts_runaway_loop() {
    let limits = SandboxLimits {
        timeout_secs: 1,
        ..Default::default()
    };
    let (sb, _) = sandbox_with(limits);
    let e = sb.eval("while (true) {}").unwrap_err();
    assert!(matches!(e, SandboxError::Timeout(1)));
}

#[test]
fn timeout_covers_hung_promise() {
    let limits = SandboxLimits {
        timeout_secs: 1,
        ..Default::default()
    };
    let (sb, _) = sandbox_with(limits);
    let e = sb.eval("await new Promise(() => {}); 42").unwrap_err();
    assert!(matches!(e, SandboxError::Timeout(1)));
}

#[test]
fn memory_limit_is_enforced() {
    // ~128MB of live data against a 4MB heap: the engine MUST be stopped by
    // the memory limit (GC cannot free the array entries). QuickJS surfaces
    // heap exhaustion as `Limit` or as a caught JS exception (its internal
    // error message is not always extractable) — both prove confinement;
    // the only way this script errors at all is the heap limit.
    let limits = SandboxLimits {
        max_heap_bytes: 4 * 1024 * 1024,
        ..Default::default()
    };
    let (sb, _) = sandbox_with(limits);
    let code = "let a = []; for (let i = 0; i < 2_000_000; i++) a.push('x'.repeat(64)); a.length;";
    let e = sb.eval(code).unwrap_err();
    assert!(
        matches!(&e, SandboxError::Limit(_) | SandboxError::Js(_)),
        "got: {e:?}"
    );
}

#[test]
fn stack_limit_catches_recursion() {
    let (sb, _) = default_sandbox();
    let e = sb.eval("function f() { return f(); } f();").unwrap_err();
    assert!(
        matches!(e, SandboxError::Js(_) | SandboxError::Limit(_)),
        "got: {e:?}"
    );
}

#[test]
fn return_size_is_capped() {
    let limits = SandboxLimits {
        max_return_bytes: 100,
        ..Default::default()
    };
    let (sb, _) = sandbox_with(limits);
    let e = sb.eval("'x'.repeat(1000)").unwrap_err();
    assert!(
        matches!(e, SandboxError::ReturnTooLarge(_, _)),
        "got: {e:?}"
    );
}

#[test]
fn log_lines_are_capped() {
    let limits = SandboxLimits {
        max_log_lines: 10,
        ..Default::default()
    };
    let (sb, _) = sandbox_with(limits);
    let out = eval_json(
        &sb,
        "for (let i = 0; i < 100; i++) console.log('line ' + i); 'done';",
    );
    assert_eq!(out["result"], "done");
    assert_eq!(out["logs"].as_array().unwrap().len(), 10);
    assert_eq!(out["logs_truncated"], true);
}

#[test]
fn js_errors_surface_cleanly() {
    let (sb, _) = default_sandbox();
    let e = sb.eval("throw new Error('boom');").unwrap_err();
    assert!(
        matches!(&e, SandboxError::Js(m) if m.contains("boom")),
        "got: {e:?}"
    );
}

#[test]
fn multi_step_script_every_primitive_has_audit_row() {
    // P2.5 exit criterion: run a multi-step script → verify every primitive
    // has an audit row, and the page-creation was claimed.
    let dir = std::env::temp_dir().join(format!("agentcowork-script-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audit.ndjson");

    let host = Arc::new(MockBrowser::with_audit(&path));
    let sb = Sandbox::new(
        SandboxLimits::default(),
        Arc::clone(&host) as Arc<dyn BrowserHost>,
    );
    let code = r#"
        const p = await browser.pages.newPage("https://example.com");
        await browser.nav(p.id).goto("https://example.com");
        await browser.read(p.id);
        await browser.grep(p.id, "content");
        await browser.pages.close(p.id);
        "ok";
    "#;
    assert!(sb.eval(code).unwrap().contains("\"result\":\"ok\""));

    let mut rows = Vec::new();
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        let ev: agentcowork_audit::AuditEvent = serde_json::from_str(line).unwrap();
        rows.push(ev);
    }
    let prims: Vec<String> = rows
        .iter()
        .filter(|e| e.kind == "script.primitive")
        .map(|e| e.payload["primitive"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        prims,
        vec!["pages.newPage", "nav.goto", "read", "grep", "pages.close"]
    );
    // every primitive row is a success; the created page was claimed
    assert!(
        rows.iter()
            .filter(|e| e.kind == "script.primitive")
            .all(|e| e.payload["ok"] == serde_json::Value::Bool(true))
    );
    let claims: Vec<String> = rows
        .iter()
        .filter(|e| e.kind == "script.page_created")
        .map(|e| e.payload["page_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(claims.len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// P5.8 `data.query` — ref-handle querying over the DataHost seam.
// ---------------------------------------------------------------------------

struct MockData {
    lines: Mutex<Vec<String>>,
}

impl DataHost for MockData {
    fn query(
        &self,
        _handle: &str,
        term: &str,
        max_hits: usize,
    ) -> Result<serde_json::Value, SandboxError> {
        let hits: Vec<String> = self
            .lines
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains(term))
            .take(max_hits)
            .cloned()
            .collect();
        Ok(serde_json::json!({ "hits": hits, "total": hits.len() }))
    }
}

#[test]
fn data_query_returns_matching_lines_only() {
    let host = Arc::new(MockBrowser::new());
    let data = Arc::new(MockData {
        lines: Mutex::new(vec![
            "alpha budget line".into(),
            "beta marketing line".into(),
            "gamma budget line".into(),
        ]),
    });
    let sb = Sandbox::with_data(
        SandboxLimits::default(),
        Arc::clone(&host) as Arc<dyn BrowserHost>,
        data,
    );
    let out = eval_json(
        &sb,
        r#"const r = await data.query("ref1", "budget", 10); r.hits;"#,
    );
    let hits = out["result"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.as_str().unwrap().contains("budget")));
}

#[test]
fn data_sdk_absent_when_no_host() {
    // Without a DataHost, `data` is undefined → a clean Js error, never a
    // crash or a silent success.
    let (sb, _) = default_sandbox();
    let e = sb.eval("data.query('x','y')").unwrap_err();
    assert!(matches!(e, SandboxError::Js(_)), "got: {e:?}");
}
