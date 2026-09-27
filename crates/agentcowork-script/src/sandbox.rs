//! The rquickjs-backed sandbox (P2.5).
//!
//! Every `eval` runs in a **fresh QuickJS runtime on a dedicated thread**:
//! a runaway script can never poison a shared engine, no JS state leaks
//! between runs, and the tokio runtime can never collide with the caller's
//! (the `run` tool may be invoked from inside another runtime).
//!
//! JSON bridging: rquickjs 0.12 has no built-in serde_json conversion, so the
//! boundary is strings — `__primitive` takes `JSON.stringify`d args and
//! returns a JSON string; the script's return value is captured through the
//! global `JSON.stringify` and parsed in Rust. All of it runs inside the
//! sandbox context, so the model-facing result is still plain JSON.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rquickjs::context::EvalOptions;
use rquickjs::function::Async;
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Function, Object, Promise, Value};

use crate::{BrowserHost, DataHost, PrimitiveCall, SandboxError, SandboxLimits, ScriptSandbox};

/// The sandbox implementation. `host` is shared so the real browser
/// (agentcowork-browser via agentcowork-mcp) can back the SDK later; the mock
/// in tests exercises the full contract. `data_host` (optional) backs the
/// `data.query` ref-handle surface (P5.8).
pub struct Sandbox {
    limits: SandboxLimits,
    host: Arc<dyn BrowserHost>,
    data_host: Option<Arc<dyn DataHost>>,
}

impl Sandbox {
    pub fn new(limits: SandboxLimits, host: Arc<dyn BrowserHost>) -> Self {
        Self {
            limits,
            host,
            data_host: None,
        }
    }

    /// Construct with a `data` SDK host (P5.8): scripts can call
    /// `data.query(handle, term)` to pull matching lines from a ref handle.
    pub fn with_data(
        limits: SandboxLimits,
        host: Arc<dyn BrowserHost>,
        data_host: Arc<dyn DataHost>,
    ) -> Self {
        Self {
            limits,
            host,
            data_host: Some(data_host),
        }
    }
}

impl ScriptSandbox for Sandbox {
    fn eval(&self, code: &str) -> Result<String, SandboxError> {
        let limits = self.limits;
        let host = Arc::clone(&self.host);
        let data_host = self.data_host.clone();
        let code = code.to_string();
        std::thread::Builder::new()
            .name("agentcowork-script".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| SandboxError::Runtime(e.to_string()))?;
                rt.block_on(run_script(host, data_host, limits, &code))
            })
            .map_err(|e| SandboxError::Runtime(e.to_string()))?
            .join()
            .map_err(|_| SandboxError::Runtime("eval thread panicked".into()))?
    }

    fn limits(&self) -> SandboxLimits {
        self.limits
    }
}

async fn run_script(
    host: Arc<dyn BrowserHost>,
    data_host: Option<Arc<dyn DataHost>>,
    limits: SandboxLimits,
    code: &str,
) -> Result<String, SandboxError> {
    // The interrupt handler flips this when the deadline passes; it
    // distinguishes a timeout from any other exception without trusting
    // exception text.
    let timed_out = Arc::new(AtomicBool::new(false));
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let logs_truncated = Arc::new(AtomicBool::new(false));

    let rt = AsyncRuntime::new().map_err(|e| SandboxError::Runtime(e.to_string()))?;
    // Memory + stack limits must be set before the context is created.
    rt.set_memory_limit(limits.max_heap_bytes as usize).await;
    rt.set_max_stack_size(limits.max_stack_bytes as usize).await;

    let deadline = Instant::now() + Duration::from_secs(limits.timeout_secs);
    let flag = Arc::clone(&timed_out);
    rt.set_interrupt_handler(Some(Box::new(move || {
        if Instant::now() >= deadline {
            flag.store(true, Ordering::SeqCst);
            true // raise an uncatchable exception → control returns to us
        } else {
            false
        }
    })))
    .await;

    let ctx = AsyncContext::full(&rt)
        .await
        .map_err(|e| SandboxError::Runtime(e.to_string()))?;

    let result = run_in_ctx(
        EvalCtx {
            ctx: &ctx,
            host: &host,
            data_host: data_host.as_ref(),
            logs: &logs,
            logs_truncated: &logs_truncated,
            timed_out: &timed_out,
        },
        limits,
        code,
    )
    .await;

    // Teardown discipline: the context must be released and the runtime
    // garbage-collected *before* the runtime is freed, or QuickJS's
    // `JS_FreeRuntime` assertion (`gc_obj_list` empty) aborts the process —
    // this fires on the timeout path (in-flight future dropped mid-poll),
    // the OOM path, and even plain runs with captured globals.
    drop(ctx);
    let _ = rt.run_gc().await;
    result
}

/// The borrowed runtime pieces shared across one eval (bundled so the
/// function stays under the argument-count lint).
struct EvalCtx<'a> {
    ctx: &'a AsyncContext,
    host: &'a Arc<dyn BrowserHost>,
    data_host: Option<&'a Arc<dyn DataHost>>,
    logs: &'a Arc<Mutex<Vec<String>>>,
    logs_truncated: &'a Arc<AtomicBool>,
    timed_out: &'a Arc<AtomicBool>,
}

/// The actual eval, plus teardown that always runs GC first.
async fn run_in_ctx(
    ec: EvalCtx<'_>,
    limits: SandboxLimits,
    code: &str,
) -> Result<String, SandboxError> {
    // Belt + suspenders: the interrupt handler catches JS-side loops; the
    // tokio timeout catches a script awaiting a Rust future that hangs.
    let grace = Duration::from_secs(limits.timeout_secs.saturating_add(5));
    let run = ec.ctx.async_with(async |ctx| {
        install_sdk(
            ctx.clone(),
            ec.host,
            ec.data_host,
            ec.logs,
            ec.logs_truncated,
            limits.max_log_lines,
        )?;
        let promise: Promise = match ctx.eval_with_options(code, {
            let mut o = EvalOptions::default();
            o.global = true;
            o.promise = true; // top-level `await` accepted; resolves to {value}
            o.backtrace_barrier = true;
            o
        }) {
            Ok(p) => p,
            Err(e) => return Err(map_eval_error(e, &ctx)),
        };
        let resolved: Value = match promise.into_future().await {
            Ok(v) => v,
            Err(e) => return Err(map_eval_error(e, &ctx)),
        };

        // `promise:true` resolves with `{value: <script return value>}`.
        let value = match resolved.as_object() {
            Some(obj) => obj.get::<_, Value>("value").unwrap_or(resolved),
            None => resolved,
        };

        // Capture the script's return value as JSON via the global
        // JSON.stringify (works for any JSON-serializable value).
        let json: serde_json::Value = if value.is_undefined() || value.is_null() {
            serde_json::Value::Null
        } else {
            let json_obj: Object = ctx
                .globals()
                .get("JSON")
                .map_err(|e| SandboxError::Js(e.to_string()))?;
            let stringify: Function = json_obj
                .get("stringify")
                .map_err(|e| SandboxError::Js(e.to_string()))?;
            let json_str: String = stringify
                .call((value,))
                .map_err(|e| SandboxError::Js(format!("result not JSON-serializable: {e}")))?;
            serde_json::from_str(&json_str)
                .map_err(|e| SandboxError::Js(format!("result not JSON: {e}")))?
        };

        let out = serde_json::to_string(&json).map_err(|e| SandboxError::Runtime(e.to_string()))?;
        if out.len() as u64 > limits.max_return_bytes {
            return Err(SandboxError::ReturnTooLarge(
                out.len(),
                limits.max_return_bytes,
            ));
        }

        let logs_out = ec.logs.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let final_json = serde_json::json!({
            "result": json,
            "logs": logs_out,
            "logs_truncated": ec.logs_truncated.load(Ordering::SeqCst),
        });
        let final_out =
            serde_json::to_string(&final_json).map_err(|e| SandboxError::Runtime(e.to_string()))?;
        Ok::<_, SandboxError>(final_out)
    });

    match tokio::time::timeout(grace, run).await {
        Err(_) => Err(SandboxError::Timeout(limits.timeout_secs)),
        Ok(Err(e)) => {
            if ec.timed_out.load(Ordering::SeqCst) {
                Err(SandboxError::Timeout(limits.timeout_secs))
            } else {
                Err(e)
            }
        }
        Ok(Ok(s)) => Ok(s),
    }
}

/// Install the `browser` SDK + bounded `console` into the fresh context.
///
/// The InnerCallHook lives in the single Rust→JS channel (`__primitive`):
/// (a) authorize → (b) exec → (c) record (denied attempts included) →
/// (d) page-creation claim. No script path can reach a browser action that
/// does not pass through it.
fn install_sdk<'js>(
    ctx: Ctx<'js>,
    host: &Arc<dyn BrowserHost>,
    data_host: Option<&Arc<dyn DataHost>>,
    logs: &Arc<Mutex<Vec<String>>>,
    logs_truncated: &Arc<AtomicBool>,
    max_log_lines: u64,
) -> Result<(), SandboxError> {
    let h = Arc::clone(host);
    // NB: the closure captures NO `Ctx` — a cloned `Ctx` inside a `Function`
    // leaks the `js_context` object and aborts `JS_FreeRuntime` (rquickjs#370),
    // and a `Ctx` parameter would pin the returned future's lifetime. Errors
    // reject the promise with a message-carrying `rquickjs::Error` instead.
    let primitive = Function::new(
        ctx.clone(),
        // The outer closure is `Fn`: it clones the host handle per call, so
        // the same JS function can be invoked many times.
        Async(move |name: String, args_json: String| {
            let h = Arc::clone(&h);
            async move {
                let args: serde_json::Value =
                    serde_json::from_str(&args_json).unwrap_or(serde_json::Value::Null);
                let page_id = args
                    .get("pageId")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string());
                let call = PrimitiveCall {
                    name,
                    page_id,
                    args,
                };

                // (a) authorize — ownership + permissions, decided by the host.
                if let Err(e) = h.authorize(&call) {
                    let msg = e.to_string();
                    // Still recorded: a denied attempt is part of the trail.
                    let _ = h.record(&call, false, &msg);
                    return Err(PrimErr(format!("denied: {msg}")));
                }
                // (b) execute.
                let outcome = h.exec(&call);
                let (ok, err) = match &outcome {
                    Ok(_) => (true, String::new()),
                    Err(e) => (false, e.to_string()),
                };
                // (c) record — every attempt, successful or not.
                let _ = h.record(&call, ok, &err);
                match outcome {
                    Ok(v) => {
                        // (d) page-creation claim (grouped like `tabs new`).
                        if call.name == "pages.newPage" {
                            if let Some(pid) = v.get("id").and_then(|x| x.as_str()) {
                                let _ = h.on_page_created(pid, &call);
                            }
                        }
                        serde_json::to_string(&v).map_err(|e| PrimErr(e.to_string()))
                    }
                    Err(e) => Err(PrimErr(format!("{e}"))),
                }
            }
        }),
    );
    ctx.globals()
        .set("__primitive", primitive)
        .map_err(|e| SandboxError::Runtime(e.to_string()))?;

    // Bounded console — 1K log lines per run, then dropped (flagged).
    let l = Arc::clone(logs);
    let t = Arc::clone(logs_truncated);
    let log_fn = Function::new(ctx.clone(), move |line: String| {
        let mut sink = l.lock().unwrap_or_else(|p| p.into_inner());
        if (sink.len() as u64) < max_log_lines {
            sink.push(line);
        } else {
            t.store(true, Ordering::SeqCst);
        }
    });
    ctx.globals()
        .set("__console_log", log_fn)
        .map_err(|e| SandboxError::Runtime(e.to_string()))?;

    // P5.8 `data.query` — a second channel backed by the DataHost (optional).
    // `max_hits` is clamped so a script cannot request an unbounded dump.
    if let Some(dh) = data_host {
        let d = Arc::clone(dh);
        let data_primitive = Function::new(
            ctx.clone(),
            Async(move |handle: String, term: String, max_hits: u32| {
                let d = Arc::clone(&d);
                async move {
                    let hits = max_hits.clamp(1, 1000) as usize;
                    match d.query(&handle, &term, hits) {
                        Ok(v) => serde_json::to_string(&v).map_err(|e| PrimErr(e.to_string())),
                        Err(e) => Err(PrimErr(format!("{e}"))),
                    }
                }
            }),
        );
        ctx.globals()
            .set("__data_primitive", data_primitive)
            .map_err(|e| SandboxError::Runtime(e.to_string()))?;
    }

    ctx.eval::<(), _>(SDK_PRELUDE)
        .map_err(|e| SandboxError::Js(format!("sdk install failed: {e}")))?;
    if data_host.is_some() {
        ctx.eval::<(), _>(DATA_PRELUDE)
            .map_err(|e| SandboxError::Js(format!("data sdk install failed: {e}")))?;
    }
    Ok(())
}

/// A primitive rejection carrying a model-friendly message. `From` into
/// `rquickjs::Error` makes the promise reject with that message as a JS
/// exception (`String(e)` in the script sees the real reason).
struct PrimErr(String);

impl From<PrimErr> for rquickjs::Error {
    fn from(e: PrimErr) -> rquickjs::Error {
        rquickjs::Error::new_from_js_message("browser", "primitive", e.0)
    }
}

/// Map an rquickjs error to a model-friendly `SandboxError`. For JS
/// exceptions the pending exception value is pulled (string or `.message`)
/// so the model sees the real reason, not "exception".
fn map_eval_error(e: rquickjs::Error, ctx: &Ctx<'_>) -> SandboxError {
    match e {
        rquickjs::Error::Allocation => SandboxError::Limit("out of memory".into()),
        rquickjs::Error::Exception => {
            let ex = ctx.catch();
            let msg = ex
                .as_string()
                .and_then(|s| s.to_string().ok())
                .or_else(|| {
                    ex.as_object()
                        .and_then(|o| o.get::<_, String>("message").ok())
                })
                .unwrap_or_else(|| "javascript exception".to_string());
            // QuickJS surfaces heap exhaustion as an internal error whose
            // message is "out of memory" — map it to the resource-limit
            // variant so the model sees Limit, not a generic JS error.
            if msg.contains("out of memory") {
                SandboxError::Limit(msg)
            } else {
                SandboxError::Js(msg)
            }
        }
        other => SandboxError::Js(other.to_string()),
    }
}

/// The `browser` SDK prelude — mirrors ARCH/08 §8.4 exactly. Every method
/// funnels through `__primitive` (the InnerCallHook channel) and returns a
/// Promise of the parsed JSON result.
/// The `data` SDK prelude (P5.8) — installed only when a [`DataHost`] is
/// present. `data.query(handle, term)` pulls matching lines from a ref handle
/// without ever serializing the full payload into the sandbox.
const DATA_PRELUDE: &str = r#"
(function () {
  "use strict";
  globalThis.data = {
    query: function (handle, term, maxHits) {
      return __data_primitive(handle, term, maxHits || 20).then(JSON.parse);
    }
  };
})();
"#;

const SDK_PRELUDE: &str = r#"
(function () {
  "use strict";
  function __log() {
    __console_log(Array.prototype.map.call(arguments, String).join(" "));
  }
  globalThis.console = {
    log: __log,
    info: __log,
    debug: __log,
    warn: function () { __log.apply(null, ["warn:"].concat(Array.prototype.slice.call(arguments))); },
    error: function () { __log.apply(null, ["error:"].concat(Array.prototype.slice.call(arguments))); }
  };
  function call(name, args) {
    return __primitive(name, JSON.stringify(args === undefined ? {} : args)).then(JSON.parse);
  }
  globalThis.browser = {
    pages: {
      newPage: function (url) { return call("pages.newPage", { url: url }); },
      close: function (id) { return call("pages.close", { pageId: id }); },
      list: function () { return call("pages.list", {}); },
      getInfo: function (id) { return call("pages.getInfo", { pageId: id }); }
    },
    observe: function (pageId) {
      return {
        snapshot: function () { return call("observe.snapshot", { pageId: pageId }); },
        diff: function () { return call("observe.diff", { pageId: pageId }); },
        resolveRef: function (ref) { return call("observe.resolveRef", { pageId: pageId, ref: ref }); }
      };
    },
    input: function (pageId) {
      return {
        click: function (ref) { return call("input.click", { pageId: pageId, ref: ref }); },
        fill: function (ref, text) { return call("input.fill", { pageId: pageId, ref: ref, text: text }); },
        type: function (ref, text) { return call("input.type", { pageId: pageId, ref: ref, text: text }); },
        press: function (key) { return call("input.press", { pageId: pageId, key: key }); },
        hover: function (ref) { return call("input.hover", { pageId: pageId, ref: ref }); },
        select: function (ref, value) { return call("input.select", { pageId: pageId, ref: ref, value: value }); },
        scroll: function (ref, dx, dy) { return call("input.scroll", { pageId: pageId, ref: ref, dx: dx, dy: dy }); }
      };
    },
    nav: function (pageId) {
      return {
        goto: function (url) { return call("nav.goto", { pageId: pageId, url: url }); },
        back: function () { return call("nav.back", { pageId: pageId }); },
        forward: function () { return call("nav.forward", { pageId: pageId }); },
        reload: function () { return call("nav.reload", { pageId: pageId }); }
      };
    },
    read: function (pageId, opts) { return call("read", Object.assign({ pageId: pageId }, opts || {})); },
    grep: function (pageId, pattern) { return call("grep", { pageId: pageId, pattern: pattern }); },
    wait: function (pageId, opts) { return call("wait", Object.assign({ pageId: pageId }, opts || {})); },
    screenshot: function (pageId) { return call("screenshot", { pageId: pageId }); },
    evaluate: function (pageId, expr) { return call("evaluate", { pageId: pageId, expr: expr }); },
    pdf: function (pageId) { return call("pdf", { pageId: pageId }); },
    download: function (pageId, url) { return call("download", { pageId: pageId, url: url }); },
    upload: function (pageId, path) { return call("upload", { pageId: pageId, path: path }); },
    tabGroups: function () { return call("tabGroups", {}); },
    windows: function () { return call("windows", {}); },
    cdp: function (method, params, sessionId) { return call("cdp", { method: method, params: params || {}, sessionId: sessionId || null }); }
  };
})();
"#;
