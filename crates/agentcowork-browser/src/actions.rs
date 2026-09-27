//! P2.3 — browser action engine (E2). One struct, `BrowserActions`, that
//! turns the 34-tool catalog (ARCH/08 §8.2) into CDP calls: navigation,
//! input dispatch (`act`), snapshot/diff, reading (DOM walker → markdown),
//! grep/wait/evaluate, screenshot/PDF export, tab/window/history management,
//! download/upload routing, and enhanced snapshots with paint-order
//! filtering. Everything goes through the `CdpSession` trait so tests drive
//! the engine with a scripted mock (no real browser needed).
//!
//! Geometry model: a snapshot node carries `backend_dom_node_id`; `act`
//! resolves a `[ref=eN]` to its backing DOM node, queries `DOM.getBoxModel`
//! for the content quad, and dispatches `Input.*` events at the center.

use crate::capture::CdpSession;
use crate::humanize::{BehaviorProfile, XorShift, mouse_path, typing_delays};
use crate::{A11yNode, Snapshot, SnapshotDiff, SnapshotEngine, SnapshotMode, diff_snapshots};
use agentcowork_cdp::CdpError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Settle wait after an `act` before capturing the post-action diff.
pub const ACT_SETTLE_MS: u64 = 500;

/// A resolved click/type target: geometry (CSS viewport px) + backend node id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// One `act` primitive (BrowserOS semantics, doc 33 §6.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActKind {
    Click {
        ref_id: String,
    },
    ClickAt {
        x: f64,
        y: f64,
    },
    Type {
        ref_id: String,
        text: String,
    },
    TypeAt {
        x: f64,
        y: f64,
        text: String,
    },
    /// Fill a whole form in one call — `fields: [{ref_id, value}]`.
    Fill {
        fields: Vec<FieldValue>,
    },
    Press {
        key: String,
    },
    Hover {
        ref_id: String,
    },
    HoverAt {
        x: f64,
        y: f64,
    },
    Focus {
        ref_id: String,
    },
    Check {
        ref_id: String,
    },
    Uncheck {
        ref_id: String,
    },
    Select {
        ref_id: String,
        value: String,
    },
    Scroll {
        direction: ScrollDirection,
    },
    Drag {
        from_ref: String,
        to_ref: String,
    },
    DragAt {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    },
    DialogAccept,
    DialogDismiss,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldValue {
    pub ref_id: String,
    pub value: String,
}

/// Result of one `act` — always carries the post-settle diff (the snapshot→
/// act→diff loop needs no follow-up snapshot call).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActResult {
    pub kind: String,
    pub diff: Option<SnapshotDiff>,
    pub note: Option<String>,
}

/// Text-selection result (read/grep).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextResult {
    pub text: String,
    pub truncated: bool,
    /// Present when the output was too large and routed to a file.
    pub saved_to: Option<String>,
}

/// read() modes (doc 55 read.rs: `--filter`/`--outline`/`--raw`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    /// Full cleaned markdown (default).
    Full,
    /// Headings + links outline only.
    Outline,
    /// Raw visible text (no markdown syntax).
    Raw,
}

/// The action engine. Owns no browser; drives one through `CdpSession`.
pub struct BrowserActions<'a, C: CdpSession> {
    pub client: &'a C,
    pub session_id: Option<&'a str>,
    pub snapshot_engine: SnapshotEngine,
    /// P2.9 — per-site behavioral realism (Bézier mouse, typing cadence).
    /// Off by default; `act` humanizes only when the profile says the site
    /// is enabled (ARCH/08 §8.10).
    behavior: BehaviorProfile,
    /// Deterministic when the profile carries a seed (tests), else time-seeded.
    rng: std::sync::Mutex<XorShift>,
    /// Last known cursor position — the start of the next Bézier path.
    mouse_pos: std::sync::Mutex<Point>,
    /// P46.4 — ref-generation registry: enforces act → invalidate →
    /// re-observe (E9 H2). A state-changing action advances the generation,
    /// so refs from the pre-mutation snapshot are stale until a re-observe.
    refs: std::sync::Mutex<crate::locator::RefRegistry>,
}

impl<'a, C: CdpSession> BrowserActions<'a, C> {
    pub fn new(client: &'a C, session_id: Option<&'a str>) -> Self {
        let behavior = BehaviorProfile::default();
        Self {
            client,
            session_id,
            snapshot_engine: SnapshotEngine::default(),
            rng: std::sync::Mutex::new(Self::make_rng(&behavior)),
            mouse_pos: std::sync::Mutex::new(Point { x: 0.0, y: 0.0 }),
            behavior,
            refs: std::sync::Mutex::new(crate::locator::RefRegistry::new()),
        }
    }

    fn make_rng(behavior: &BehaviorProfile) -> XorShift {
        match behavior.seed {
            Some(s) => XorShift::new(s),
            None => {
                let t = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0x1234_5678_9ABC_DEF0);
                XorShift::new(t)
            }
        }
    }

    pub fn with_mode(mut self, mode: SnapshotMode) -> Self {
        self.snapshot_engine = self.snapshot_engine.with_mode(mode);
        self
    }

    /// P2.9 — enable per-site behavioral realism on this engine.
    pub fn with_behavior(mut self, behavior: BehaviorProfile) -> Self {
        self.rng = std::sync::Mutex::new(Self::make_rng(&behavior));
        self.behavior = behavior;
        self
    }

    // ------------------------------------------------------------------
    // navigation
    // ------------------------------------------------------------------

    /// `navigate` — goto URL, back, forward, reload.
    pub fn navigate(&self, action: NavigateAction) -> Result<Snapshot, CdpError> {
        match action {
            NavigateAction::Goto { url } => {
                self.client
                    .call_session(self.sid()?, "Page.navigate", json!({ "url": url }))?;
            }
            NavigateAction::Back => {
                // navigateToHistoryEntry needs the entry's `id`, not the index.
                let (entries, idx) = self.history()?;
                if let Some(entry) = entries.get(idx.saturating_sub(1) as usize) {
                    self.client.call_session(
                        self.sid()?,
                        "Page.navigateToHistoryEntry",
                        json!({ "entryId": entry }),
                    )?;
                }
            }
            NavigateAction::Forward => {
                let (entries, idx) = self.history()?;
                if let Some(entry) = entries.get((idx + 1) as usize) {
                    self.client.call_session(
                        self.sid()?,
                        "Page.navigateToHistoryEntry",
                        json!({ "entryId": entry }),
                    )?;
                }
            }
            NavigateAction::Reload => {
                self.client.call_session(
                    self.sid()?,
                    "Page.reload",
                    json!({ "ignoreCache": false }),
                )?;
            }
        }
        self.settle(600);
        self.snapshot("after-navigate")
    }

    /// `wait` — poll for text/selector, or sleep ms.
    pub fn wait(&self, wait_for: WaitFor, timeout_ms: u64) -> Result<WaitOutcome, CdpError> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let done = match &wait_for {
                WaitFor::Ms(ms) => {
                    std::thread::sleep(Duration::from_millis(*ms));
                    true
                }
                WaitFor::Text(t) => {
                    let txt = self.visible_text()?;
                    txt.contains(t)
                }
                WaitFor::Selector(sel) => {
                    let found = self
                        .client
                        .call_session(
                            self.sid()?,
                            "Runtime.evaluate",
                            json!({
                                "expression": format!("!!document.querySelector({sel:?})"),
                                "returnByValue": true,
                            }),
                        )?
                        .pointer("/result/value")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    found
                }
            };
            if done {
                return Ok(WaitOutcome::Satisfied);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(WaitOutcome::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `evaluate` — CDP Runtime.evaluate with return value.
    pub fn evaluate(&self, expression: &str) -> Result<Value, CdpError> {
        let out = self.client.call_session(
            self.sid()?,
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )?;
        Ok(out)
    }

    // ------------------------------------------------------------------
    // act
    // ------------------------------------------------------------------

    /// `act` — one input primitive; always returns the post-settle diff.
    pub fn act(&self, act: ActKind) -> Result<ActResult, CdpError> {
        // P46.4 — the pre/post snapshots are raw (diff-only): they must not
        // re-observe refs, or the invalidation between acts would be undone.
        let pre = self.capture_raw("pre-act")?;
        let kind = act_kind_name(&act);
        // P2.9 — per-site behavioral realism: humanize only when the profile
        // says the current page's host is enabled.
        let humanized = self.behavior.site_enabled(&pre.url);
        // P46.4 — refuse a ref the registry knows is from a pre-mutation
        // snapshot (E9 H2): the caller must re-observe before acting again.
        self.reject_stale_refs(&act)?;
        self.dispatch(&act, humanized)?;
        self.settle(ACT_SETTLE_MS);
        // P46.4 — the action changed state: prior refs are now stale until
        // the next snapshot re-observes them.
        self.refs.lock().unwrap().invalidate();
        let post = self.capture_raw("post-act")?;
        let diff = diff_snapshots(&pre, &post);
        Ok(ActResult {
            kind: kind.to_string(),
            diff: Some(diff),
            note: humanized.then(|| "humanized (P2.9)".to_string()),
        })
    }

    /// P46.4 — check every ref used by `act` against the registry. A ref
    /// observed in an older (pre-mutation) generation is refused: acting on
    /// it would target stale geometry. Unknown refs pass (one-shot use).
    fn reject_stale_refs(&self, act: &ActKind) -> Result<(), CdpError> {
        let reg = self.refs.lock().unwrap();
        let check = |ref_id: &str| -> Result<(), CdpError> {
            if reg.is_stale(ref_id) {
                return Err(CdpError::Protocol {
                    code: -1,
                    message: format!(
                        "ref {ref_id} is stale (pre-mutation snapshot) — re-observe before acting"
                    ),
                });
            }
            Ok(())
        };
        match act {
            ActKind::Click { ref_id }
            | ActKind::Type { ref_id, .. }
            | ActKind::Hover { ref_id }
            | ActKind::Focus { ref_id }
            | ActKind::Check { ref_id }
            | ActKind::Uncheck { ref_id }
            | ActKind::Select { ref_id, .. } => check(ref_id),
            ActKind::Fill { fields } => {
                for f in fields {
                    check(&f.ref_id)?;
                }
                Ok(())
            }
            ActKind::Drag { from_ref, to_ref } => {
                check(from_ref)?;
                check(to_ref)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn dispatch(&self, act: &ActKind, humanized: bool) -> Result<(), CdpError> {
        match act {
            ActKind::Click { ref_id } => {
                let p = self.ref_point(ref_id)?;
                self.click_at(&p, humanized)
            }
            ActKind::ClickAt { x, y } => self.click_at(&Point { x: *x, y: *y }, humanized),
            ActKind::Type { ref_id, text } => {
                let p = self.ref_point(ref_id)?;
                self.focus_at(&p, humanized)?;
                self.insert_text(text, humanized)
            }
            ActKind::TypeAt { x, y, text } => {
                let p = Point { x: *x, y: *y };
                self.focus_at(&p, humanized)?;
                self.insert_text(text, humanized)
            }
            ActKind::Fill { fields } => {
                for f in fields {
                    let p = self.ref_point(&f.ref_id)?;
                    self.focus_at(&p, humanized)?;
                    self.insert_text(&f.value, humanized)?;
                }
                Ok(())
            }
            ActKind::Press { key } => self.press_key(key),
            ActKind::Hover { ref_id } => {
                let p = self.ref_point(ref_id)?;
                self.mouse_move_to(&p, humanized, true)
            }
            ActKind::HoverAt { x, y } => {
                self.mouse_move_to(&Point { x: *x, y: *y }, humanized, true)
            }
            ActKind::Focus { ref_id } => {
                let p = self.ref_point(ref_id)?;
                self.focus_at(&p, humanized)
            }
            ActKind::Check { ref_id } => self.set_checked(ref_id, true),
            ActKind::Uncheck { ref_id } => self.set_checked(ref_id, false),
            ActKind::Select { ref_id, value } => self.select_value(ref_id, value),
            ActKind::Scroll { direction } => self.scroll(*direction),
            ActKind::Drag { from_ref, to_ref } => {
                let from = self.ref_point(from_ref)?;
                let to = self.ref_point(to_ref)?;
                self.drag(&from, &to, humanized)
            }
            ActKind::DragAt {
                from_x,
                from_y,
                to_x,
                to_y,
            } => self.drag(
                &Point {
                    x: *from_x,
                    y: *from_y,
                },
                &Point { x: *to_x, y: *to_y },
                humanized,
            ),
            ActKind::DialogAccept => self.dialog(true),
            ActKind::DialogDismiss => self.dialog(false),
        }
    }

    fn click_at(&self, p: &Point, humanized: bool) -> Result<(), CdpError> {
        if humanized {
            // Move along a Bézier curve to a jittered natural target, then
            // press/release there (the cursor never teleports).
            self.mouse_move_to(p, true, false)?;
            let target = *self.mouse_pos.lock().unwrap();
            self.mouse("mousePressed", &target, Some("left"))?;
            self.mouse("mouseReleased", &target, Some("left"))
        } else {
            self.mouse("mousePressed", p, Some("left"))?;
            self.mouse("mouseReleased", p, Some("left"))
        }
    }

    /// Move the cursor to `p` — as one `mouseMoved` when not humanized, or
    /// along a Bézier `mouse_path` (with per-step cadence) when humanized.
    /// `exact_end`: hover targets land precisely (jitter is an interior
    /// path property); click targets keep the jittered natural endpoint.
    /// Updates the tracked cursor position either way.
    fn mouse_move_to(&self, p: &Point, humanized: bool, exact_end: bool) -> Result<(), CdpError> {
        let from = *self.mouse_pos.lock().unwrap();
        if humanized {
            let path = {
                let mut rng = self.rng.lock().unwrap();
                mouse_path(&mut rng, &self.behavior.mouse, &from, p)
            };
            let (lo, hi) = self.behavior.mouse.move_delay_ms;
            for step in &path {
                self.mouse("mouseMoved", step, None)?;
                *self.mouse_pos.lock().unwrap() = *step;
                let delay = {
                    let mut rng = self.rng.lock().unwrap();
                    rng.range(lo as f64, hi as f64) as u64
                };
                thread::sleep(Duration::from_millis(delay));
            }
            if exact_end && path.last().map(|l| l != p).unwrap_or(true) {
                self.mouse("mouseMoved", p, None)?;
                *self.mouse_pos.lock().unwrap() = *p;
            }
            Ok(())
        } else {
            self.mouse("mouseMoved", p, None)?;
            *self.mouse_pos.lock().unwrap() = *p;
            Ok(())
        }
    }

    fn mouse(&self, typ: &str, p: &Point, button: Option<&str>) -> Result<(), CdpError> {
        let mut params = json!({
            "type": typ,
            "x": p.x,
            "y": p.y,
            "clickCount": 1,
        });
        if let Some(b) = button {
            params["button"] = json!(b);
            params["buttons"] = json!(1);
        }
        self.client
            .call_session(self.sid()?, "Input.dispatchMouseEvent", params)?;
        Ok(())
    }

    fn focus_at(&self, p: &Point, humanized: bool) -> Result<(), CdpError> {
        // Click to focus the element under the point, then Ctrl+A so a
        // subsequent insertText replaces (not appends to) existing content.
        self.click_at(p, humanized)?;
        self.press_raw("Control", "ControlLeft", 17)?;
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({ "type": "keyDown", "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "nativeVirtualKeyCode": 65, "modifiers": 2 }),
        )?;
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({ "type": "keyUp", "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "nativeVirtualKeyCode": 65, "modifiers": 2 }),
        )?;
        self.press_raw("Control", "ControlLeft", 17)
    }

    /// A bare modifier press/release (used to build Ctrl+A etc.).
    fn press_raw(&self, key: &str, code: &str, vk: i32) -> Result<(), CdpError> {
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({ "type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk }),
        )?;
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk }),
        )?;
        Ok(())
    }

    fn insert_text(&self, text: &str, humanized: bool) -> Result<(), CdpError> {
        if !humanized {
            self.client
                .call_session(self.sid()?, "Input.insertText", json!({ "text": text }))?;
            return Ok(());
        }
        // P2.9 — per-key typing cadence: one keyDown/keyUp per char with
        // natural per-char delays and word-boundary pauses.
        let delays = {
            let mut rng = self.rng.lock().unwrap();
            typing_delays(&mut rng, &self.behavior.typing, text)
        };
        for (i, ch) in text.chars().enumerate() {
            self.dispatch_char(ch)?;
            if let Some(d) = delays.get(i) {
                thread::sleep(*d);
            }
        }
        Ok(())
    }

    /// One printable character as keyDown/keyUp (with `text`), CDP-style.
    fn dispatch_char(&self, ch: char) -> Result<(), CdpError> {
        let (key, code, vk): (String, String, i32) = match ch {
            '\t' => ("Tab".into(), "Tab".into(), 9),
            '\n' => ("Enter".into(), "Enter".into(), 13),
            ' ' => (" ".into(), "Space".into(), 32),
            c if c.is_ascii_alphabetic() => {
                let u = c.to_ascii_uppercase();
                (c.to_string(), format!("Key{u}"), u as i32)
            }
            c if c.is_ascii_digit() => (c.to_string(), format!("Digit{c}"), c as i32),
            c if c.is_ascii() => (c.to_string(), String::new(), c as i32),
            c => (c.to_string(), String::new(), 0),
        };
        let mut down = json!({
            "type": "keyDown",
            "key": key,
            "code": code,
            "windowsVirtualKeyCode": vk,
            "nativeVirtualKeyCode": vk,
        });
        if ch != '\t' && ch != '\n' {
            down["text"] = json!(ch.to_string());
        }
        self.client
            .call_session(self.sid()?, "Input.dispatchKeyEvent", down)?;
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
            }),
        )?;
        Ok(())
    }

    fn press_key(&self, key: &str) -> Result<(), CdpError> {
        let normalized = match key {
            "Enter" | "enter" => "Enter",
            "Tab" | "tab" => "Tab",
            "Escape" | "esc" => "Escape",
            "Backspace" | "backspace" => "Backspace",
            "ArrowUp" => "ArrowUp",
            "ArrowDown" => "ArrowDown",
            "ArrowLeft" => "ArrowLeft",
            "ArrowRight" => "ArrowRight",
            k => k,
        };
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({ "type": "keyDown", "key": normalized }),
        )?;
        self.client.call_session(
            self.sid()?,
            "Input.dispatchKeyEvent",
            json!({ "type": "keyUp", "key": normalized }),
        )?;
        Ok(())
    }

    fn set_checked(&self, ref_id: &str, checked: bool) -> Result<(), CdpError> {
        let backend = self.ref_backend(ref_id)?;
        let object_id = self.backend_to_object_id(backend)?;
        self.client.call_session(
            self.sid()?,
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function(v) { if (this) { this.checked = arguments[0]; this.dispatchEvent(new Event('change', {bubbles:true})); } return !!this; }",
                "arguments": [json!(checked)],
                "returnByValue": true,
            }),
        )?;
        Ok(())
    }

    fn select_value(&self, ref_id: &str, value: &str) -> Result<(), CdpError> {
        let backend = self.ref_backend(ref_id)?;
        let object_id = self.backend_to_object_id(backend)?;
        // Native <select>: set value + dispatch change.
        self.client.call_session(
            self.sid()?,
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function(v) { if (this && this.tagName === 'SELECT') { this.value = arguments[0]; this.dispatchEvent(new Event('change', {bubbles:true})); return true; } return false; }",
                "arguments": [json!(value)],
                "returnByValue": true,
            }),
        )?;
        Ok(())
    }

    fn scroll(&self, dir: ScrollDirection) -> Result<(), CdpError> {
        let (dx, dy) = match dir {
            ScrollDirection::Up => (0.0, -400.0),
            ScrollDirection::Down => (0.0, 400.0),
            ScrollDirection::Left => (-400.0, 0.0),
            ScrollDirection::Right => (400.0, 0.0),
        };
        self.client.call_session(
            self.sid()?,
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseWheel", "x": 640.0, "y": 400.0, "deltaX": dx, "deltaY": dy }),
        )?;
        Ok(())
    }

    fn drag(&self, from: &Point, to: &Point, humanized: bool) -> Result<(), CdpError> {
        self.mouse("mousePressed", from, Some("left"))?;
        // Intermediate moves: linear when plain, Bézier when humanized. The
        // release target stays exact either way (a drop must land precisely).
        let path: Vec<Point> = if humanized {
            let mut rng = self.rng.lock().unwrap();
            mouse_path(&mut rng, &self.behavior.mouse, from, to)
        } else {
            (1..=8)
                .map(|i| {
                    let t = i as f64 / 8.0;
                    Point {
                        x: from.x + (to.x - from.x) * t,
                        y: from.y + (to.y - from.y) * t,
                    }
                })
                .collect()
        };
        for p in &path {
            self.mouse("mouseMoved", p, None)?;
        }
        *self.mouse_pos.lock().unwrap() = *to;
        self.mouse("mouseReleased", to, Some("left"))
    }

    fn dialog(&self, accept: bool) -> Result<(), CdpError> {
        self.client.call_session(
            self.sid()?,
            "Page.handleJavaScriptDialog",
            json!({ "accept": accept }),
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // geometry + refs
    // ------------------------------------------------------------------

    /// Resolve a `[ref=eN]` to its center point (CSS viewport px).
    fn resolve_ref(&self, ref_id: &str) -> Result<Point, CdpError> {
        let snap = self.snapshot("ref-resolve")?;
        let node = find_ref(&snap.root, ref_id).ok_or_else(|| CdpError::Protocol {
            code: -1,
            message: format!("ref {ref_id} not found in current snapshot"),
        })?;
        let backend = node.backend_dom_node_id.ok_or_else(|| CdpError::Protocol {
            code: -1,
            message: format!("ref {ref_id} has no backing DOM node"),
        })?;
        self.box_center(&backend)
    }

    fn ref_point(&self, ref_id: &str) -> Result<Point, CdpError> {
        self.resolve_ref(ref_id)
    }

    /// Resolve a backend node to a Runtime object id (for callFunctionOn).
    fn backend_to_object_id(&self, backend: i64) -> Result<String, CdpError> {
        self.ensure_dom_enabled()?;
        let out = self.client.call_session(
            self.sid()?,
            "DOM.resolveNode",
            json!({ "backendNodeId": backend }),
        )?;
        out.pointer("/result/object/objectId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "DOM.resolveNode returned no objectId".into(),
            })
    }

    /// Center of the element's content quad (CSS viewport px).
    fn box_center(&self, backend: &i64) -> Result<Point, CdpError> {
        self.ensure_dom_enabled()?;
        let out = self.client.call_session(
            self.sid()?,
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend }),
        )?;
        let quad = out
            .pointer("/model/content")
            .and_then(Value::as_array)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "DOM.getBoxModel returned no content quad".into(),
            })?;
        if quad.len() < 8 {
            return Err(CdpError::Protocol {
                code: -1,
                message: "DOM.getBoxModel quad too short".into(),
            });
        }
        let xs: Vec<f64> = quad.iter().step_by(2).filter_map(Value::as_f64).collect();
        let ys: Vec<f64> = quad
            .iter()
            .skip(1)
            .step_by(2)
            .filter_map(Value::as_f64)
            .collect();
        if xs.is_empty() || ys.is_empty() {
            return Err(CdpError::Protocol {
                code: -1,
                message: "DOM.getBoxModel quad unparseable".into(),
            });
        }
        let cx = xs.iter().sum::<f64>() / xs.len() as f64;
        let cy = ys.iter().sum::<f64>() / ys.len() as f64;
        Ok(Point { x: cx, y: cy })
    }

    // ------------------------------------------------------------------
    // snapshot / diff / enhanced snapshot
    // ------------------------------------------------------------------

    pub fn snapshot(&self, document_id: &str) -> Result<Snapshot, CdpError> {
        let snap = self
            .snapshot_engine
            .capture(self.client, self.session_id, document_id)?;
        // P46.4 — a fresh snapshot re-stamps its refs as current (the
        // re-observe half of act → invalidate → re-observe).
        self.refs.lock().unwrap().observe(&snap.root);
        Ok(snap)
    }

    /// P46.4 — a *non-observing* capture for `act`'s internal pre/post
    /// snapshots. These are for diffing only; they must not re-stamp refs,
    /// or the invalidation between acts would be undone by the very action
    /// that caused it. Only the caller's explicit [`Self::snapshot`] is a
    /// re-observe.
    fn capture_raw(&self, document_id: &str) -> Result<Snapshot, CdpError> {
        self.snapshot_engine
            .capture(self.client, self.session_id, document_id)
    }

    pub fn diff(&self, base: &Snapshot, current: &Snapshot) -> SnapshotDiff {
        diff_snapshots(base, current)
    }

    /// `enhanced_snapshot` — snapshot + paint-order filtering: a node whose
    /// center point is covered by a different element is flagged `occluded`.
    pub fn enhanced_snapshot(&self, document_id: &str) -> Result<EnhancedSnapshot, CdpError> {
        let snap = self.snapshot(document_id)?;
        let occluded = self.paint_order_filter(&snap.root)?;
        Ok(EnhancedSnapshot {
            snapshot: snap,
            occluded,
        })
    }

    fn paint_order_filter(&self, root: &A11yNode) -> Result<Vec<String>, CdpError> {
        let mut occluded = Vec::new();
        collect_actionable(root, &mut |node| {
            if let Some(backend) = node.backend_dom_node_id {
                if let Ok(p) = self.box_center(&backend) {
                    // elementFromPoint tells us what actually paints at (x,y).
                    let expr = format!(
                        "(function(){{ const el = document.elementFromPoint({}, {}); \
                         return el ? el.textContent.slice(0,60) : null; }})()",
                        p.x, p.y
                    );
                    let sid = self.session_id.unwrap_or("");
                    if !sid.is_empty() {
                        if let Ok(out) = self.client.call_session(
                            sid,
                            "Runtime.evaluate",
                            json!({ "expression": expr, "returnByValue": true }),
                        ) {
                            let hit = out.pointer("/result/value").and_then(Value::as_str);
                            let own = node.name.as_str();
                            // If the topmost element's text doesn't match, the node
                            // is painted over by something else.
                            // Heuristic: only flag occlusion when the node has a
                            // distinctive-enough name (>= 3 chars) — short/shared
                            // names ("Go", "OK") cause false positives.
                            let covered = match hit {
                                Some(h) => {
                                    own.len() >= 3
                                        && !h.is_empty()
                                        && !own.is_empty()
                                        && !h.contains(own)
                                }
                                None => false,
                            };
                            if covered {
                                if let Some(r) = &node.ref_id {
                                    occluded.push(r.clone());
                                }
                            }
                        }
                    }
                }
            }
        });
        Ok(occluded)
    }

    // ------------------------------------------------------------------
    // read / grep
    // ------------------------------------------------------------------

    /// `read` — page → clean markdown via the DOM walker.
    pub fn read(&self, mode: ReadMode) -> Result<TextResult, CdpError> {
        let script = match mode {
            ReadMode::Full => DOM_WALKER_MARKDOWN,
            ReadMode::Outline => DOM_WALKER_OUTLINE,
            ReadMode::Raw => DOM_WALKER_RAW,
        };
        let out = self.client.call_session(
            self.sid()?,
            "Runtime.evaluate",
            json!({ "expression": script, "returnByValue": true }),
        )?;
        let text = out
            .pointer("/result/value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // G9 read-cleaner (P2.11): strip ad/tracker links + consent walls.
        let cleaned = crate::content::clean_markdown(
            &crate::content::default_filter_set(),
            &self.current_url(),
            &text,
        );
        Ok(TextResult {
            truncated: false,
            saved_to: None,
            text: cleaned.text,
        })
    }

    /// `grep` — line matches in the page's visible text.
    pub fn grep(&self, pattern: &str) -> Result<TextResult, CdpError> {
        let raw = self.visible_text()?;
        let mut out = String::new();
        let re = regex::Regex::new(pattern).map_err(|e| CdpError::Protocol {
            code: -1,
            message: format!("bad grep pattern: {e}"),
        })?;
        for (i, line) in raw.lines().enumerate() {
            if re.is_match(line) {
                out.push_str(&format!("{}: {}\n", i + 1, line.trim()));
            }
        }
        Ok(TextResult {
            text: out,
            truncated: false,
            saved_to: None,
        })
    }

    fn visible_text(&self) -> Result<String, CdpError> {
        let out = self.client.call_session(
            self.sid()?,
            "Runtime.evaluate",
            json!({
                "expression": "document.body ? document.body.innerText : ''",
                "returnByValue": true,
            }),
        )?;
        Ok(out
            .pointer("/result/value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    // ------------------------------------------------------------------
    // screenshot / pdf
    // ------------------------------------------------------------------

    /// `screenshot` — JPEG capture (base64). Caller routes to a file.
    pub fn screenshot_jpeg(&self, quality: u8) -> Result<String, CdpError> {
        let out = self.client.call_session(
            self.sid()?,
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": quality }),
        )?;
        // CDP returns `{data}` as the method result (already JSON-RPC-
        // unwrapped by the transport) — read `/data`, not `/result/data`.
        out.get("data")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "Page.captureScreenshot returned no data".into(),
            })
    }

    /// `annotated_screenshot` (post-v1 tool, doc 55): a JPEG capture plus the
    /// numbered-label overlay data (`ref ↔ accessible name ↔ viewport center`)
    /// the UI draws on top. Maps `[ref=eN]` ids to pixel positions so a user
    /// (or model) can see exactly what each ref points at — the deterministic
    /// half of annotated screenshots; the actual pixel overlay is drawn by the
    /// frontend, which keeps the image library out of the browser crate.
    pub fn annotated_screenshot(
        &self,
        document_id: &str,
        quality: u8,
    ) -> Result<AnnotatedScreenshot, CdpError> {
        let snap = self.snapshot(document_id)?;
        let mut labels = Vec::new();
        collect_actionable(&snap.root, &mut |node| {
            let (Some(ref_id), Some(backend)) = (node.ref_id.clone(), node.backend_dom_node_id)
            else {
                return;
            };
            // Geometry can be missing for off-screen/display:none nodes — skip
            // the label rather than failing the whole capture.
            if let Ok(center) = self.box_center(&backend) {
                labels.push(ScreenshotLabel {
                    ref_id,
                    label: node.name.clone(),
                    x: center.x,
                    y: center.y,
                });
            }
        });
        let screenshot = self.screenshot_jpeg(quality)?;
        Ok(AnnotatedScreenshot { screenshot, labels })
    }

    /// `pdf` — Page.printToPDF (base64).
    pub fn pdf_base64(&self) -> Result<String, CdpError> {
        let out = self.client.call_session(
            self.sid()?,
            "Page.printToPDF",
            json!({ "printBackground": true }),
        )?;
        out.get("data")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "Page.printToPDF returned no data".into(),
            })
    }

    // ------------------------------------------------------------------
    // tabs / windows / history
    // ------------------------------------------------------------------

    pub fn tabs(&self) -> Result<Value, CdpError> {
        self.client.call("Target.getTargets", json!({}))
    }

    pub fn windows(&self) -> Result<Value, CdpError> {
        // Group targets by browserContextId (each context = one window).
        // Targets without a context belong to the default window — counted
        // as one "default" window so we never undercount.
        let targets = self.client.call("Target.getTargets", json!({}))?;
        let list = targets
            .pointer("/result/targetInfos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut windows = list
            .iter()
            .filter_map(|t| t.get("browserContextId").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        let has_default = list.iter().any(|t| t.get("browserContextId").is_none());
        if has_default {
            windows.insert("default");
        }
        Ok(json!({ "windows": windows.len(), "contextIds": windows }))
    }

    pub fn create_window(&self, hidden: bool) -> Result<Value, CdpError> {
        // Create a dedicated browser context so close_window can dispose it
        // (the returned context id owns every target created in it).
        let ctx = self.client.call("Target.createBrowserContext", json!({}))?;
        let context_id = ctx
            .pointer("/browserContextId")
            .and_then(Value::as_str)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: "Target.createBrowserContext returned no browserContextId".into(),
            })?
            .to_string();
        let created = self.client.call(
            "Target.createTarget",
            json!({ "url": "about:blank", "newWindow": true, "background": hidden, "browserContextId": context_id }),
        )?;
        // Tag the result with the context id for the caller (close_window).
        let mut created = created;
        if let Some(obj) = created.as_object_mut() {
            obj.insert("browserContextId".into(), json!(context_id));
        }
        Ok(created)
    }

    pub fn close_window(&self, context_id: &str) -> Result<(), CdpError> {
        self.client.call(
            "Target.disposeBrowserContext",
            json!({ "browserContextId": context_id }),
        )?;
        Ok(())
    }

    /// `close_tab` — close one tab by CDP target id (`Target.closeTarget`).
    /// Ownership is the caller's job (TabRegistry::can_close — E6); this is
    /// the raw CDP primitive behind it.
    pub fn close_tab(&self, tab_id: &str) -> Result<(), CdpError> {
        self.client
            .call("Target.closeTarget", json!({ "targetId": tab_id }))?;
        Ok(())
    }

    pub fn history(&self) -> Result<(Vec<u64>, u64), CdpError> {
        let out = self
            .client
            .call_session(self.sid()?, "Page.getNavigationHistory", json!({}))?;
        let idx = out
            .pointer("/result/currentIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let entries = out
            .pointer("/result/entries")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.get("id").and_then(Value::as_u64))
                    .collect()
            })
            .unwrap_or_default();
        Ok((entries, idx))
    }

    /// E16 — `DOMDebugger.getEventListeners` pass (doc 63 §4.2): does the DOM
    /// node behind `backend_node_id` have a JavaScript `click` listener? SPA
    /// divs with `onClick` have no ARIA role, so this is how slim snapshots
    /// decide they are still actionable.
    pub fn js_click_handler(&self, backend_node_id: u64) -> Result<bool, CdpError> {
        let resolved = self.client.call_session(
            self.sid()?,
            "DOM.resolveNode",
            json!({ "backendNodeId": backend_node_id }),
        )?;
        let object_id = match resolved.pointer("/object/objectId").and_then(Value::as_str) {
            Some(id) => id,
            None => return Ok(false),
        };
        let listeners = self.client.call_session(
            self.sid()?,
            "DOMDebugger.getEventListeners",
            json!({ "objectId": object_id }),
        )?;
        Ok(listeners
            .pointer("/listeners")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .any(|l| l.get("type").and_then(Value::as_str) == Some("click"))
            })
            .unwrap_or(false))
    }

    /// The current page URL (best-effort, for the G9 read-cleaner).
    fn current_url(&self) -> String {
        let sid = match self.sid() {
            Ok(s) => s,
            Err(_) => return String::new(),
        };
        self.client
            .call_session(sid, "Page.getNavigationHistory", json!({}))
            .ok()
            .and_then(|out| {
                let idx = out
                    .pointer("/result/currentIndex")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                out.pointer("/result/entries")
                    .and_then(Value::as_array)
                    .and_then(|a| a.get(idx))
                    .and_then(|e| e.get("url").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // download / upload
    // ------------------------------------------------------------------

    /// `download` — set the browser download behavior to a temp dir.
    pub fn set_download_path(&self, dir: &str) -> Result<(), CdpError> {
        self.client.call(
            "Browser.setDownloadBehavior",
            json!({ "behavior": "allow", "downloadPath": dir, "eventsEnabled": true }),
        )?;
        Ok(())
    }

    /// `upload` — DOM.setFileInputFiles on a `<input type=file>` by ref.
    pub fn upload_files(&self, ref_id: &str, paths: &[String]) -> Result<(), CdpError> {
        let backend = self.ref_backend(ref_id)?;
        self.client.call_session(
            self.sid()?,
            "DOM.setFileInputFiles",
            json!({ "backendNodeId": backend, "files": paths }),
        )?;
        Ok(())
    }

    fn ref_backend(&self, ref_id: &str) -> Result<i64, CdpError> {
        let snap = self.snapshot("ref-backend")?;
        find_ref(&snap.root, ref_id)
            .and_then(|n| n.backend_dom_node_id)
            .ok_or_else(|| CdpError::Protocol {
                code: -1,
                message: format!("ref {ref_id} has no backing DOM node"),
            })
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    fn settle(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// The attached session id — page-level actions need one.
    fn sid(&self) -> Result<&str, CdpError> {
        self.session_id.ok_or_else(|| CdpError::Protocol {
            code: -1,
            message: "action requires an attached session".into(),
        })
    }

    /// `DOM.enable` is required before DOM-domain calls in modern Chrome.
    /// Idempotent — the extra call is harmless on re-entry.
    fn ensure_dom_enabled(&self) -> Result<(), CdpError> {
        self.client
            .call_session(self.sid()?, "DOM.enable", json!({}))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// navigation / wait enums (kept outside the impl for the tool layer)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum NavigateAction {
    Goto { url: String },
    Back,
    Forward,
    Reload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "for", rename_all = "snake_case")]
pub enum WaitFor {
    Text(String),
    Selector(String),
    Ms(u64),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitOutcome {
    Satisfied,
    TimedOut,
}

/// `enhanced_snapshot` result: snapshot + refs occluded by paint order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhancedSnapshot {
    pub snapshot: Snapshot,
    pub occluded: Vec<String>,
}

/// One numbered label for an annotated screenshot (ref ↔ name ↔ center).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScreenshotLabel {
    pub ref_id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
}

/// `annotated_screenshot` result: JPEG base64 + label overlay data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnnotatedScreenshot {
    pub screenshot: String,
    pub labels: Vec<ScreenshotLabel>,
}

fn act_kind_name(act: &ActKind) -> &'static str {
    match act {
        ActKind::Click { .. } => "click",
        ActKind::ClickAt { .. } => "click_at",
        ActKind::Type { .. } => "type",
        ActKind::TypeAt { .. } => "type_at",
        ActKind::Fill { .. } => "fill",
        ActKind::Press { .. } => "press",
        ActKind::Hover { .. } => "hover",
        ActKind::HoverAt { .. } => "hover_at",
        ActKind::Focus { .. } => "focus",
        ActKind::Check { .. } => "check",
        ActKind::Uncheck { .. } => "uncheck",
        ActKind::Select { .. } => "select",
        ActKind::Scroll { .. } => "scroll",
        ActKind::Drag { .. } => "drag",
        ActKind::DragAt { .. } => "drag_at",
        ActKind::DialogAccept => "dialog_accept",
        ActKind::DialogDismiss => "dialog_dismiss",
    }
}

/// Find a node by its `[ref=eN]` id (breadth-first).
pub fn find_ref<'n>(root: &'n A11yNode, ref_id: &str) -> Option<&'n A11yNode> {
    if root.ref_id.as_deref() == Some(ref_id) {
        return Some(root);
    }
    for c in &root.children {
        if let Some(found) = find_ref(c, ref_id) {
            return Some(found);
        }
    }
    None
}

fn collect_actionable<'n>(node: &'n A11yNode, f: &mut impl FnMut(&'n A11yNode)) {
    if node.actionable {
        f(node);
    }
    for c in &node.children {
        collect_actionable(c, f);
    }
}

// ---------------------------------------------------------------------------
// DOM walkers (Runtime.evaluate scripts) — our in-process `content-markdown`
// equivalent (ARCH/08 §8.2 read/grep; doc 55 read.rs semantics).
// ---------------------------------------------------------------------------

pub(crate) const DOM_WALKER_MARKDOWN: &str = r#"
(function () {
  const out = [];
  function esc(t) { return String(t ?? '').replace(/[\\`*_{}\[\]<>#+.!|]/g, '\\$&'); }
  function walk(el, depth) {
    if (!el || depth > 24) return;
    const tag = el.tagName ? el.tagName.toLowerCase() : '';
    const role = el.getAttribute && el.getAttribute('role');
    if (tag === 'script' || tag === 'style' || tag === 'noscript' || tag === 'template' || tag === 'svg') return;
    if (role === 'presentation') { for (const c of el.children) walk(c, depth); return; }
    const text = (el.innerText || el.textContent || '').trim();
    if (/^h[1-6]$/.test(tag)) { out.push('#'.repeat(+tag[1]) + ' ' + esc(text)); return; }
    if (tag === 'a') { out.push('[' + esc(text) + '](' + (el.href || '') + ')'); return; }
    if (tag === 'li') { out.push('  '.repeat(Math.max(0, depth - 1)) + '- ' + esc(text)); return; }
    if (tag === 'p') { out.push(esc(text)); return; }
    if (tag === 'pre' || tag === 'code') { out.push('```\n' + text + '\n```'); return; }
    if (tag === 'img') { out.push('![' + (el.alt || '') + '](' + (el.src || '') + ')'); return; }
    if (tag === 'table') {
      for (const row of el.querySelectorAll('tr')) {
        const cells = [...row.querySelectorAll('th,td')].map(c => esc(c.innerText.trim())).join(' | ');
        if (cells) out.push('| ' + cells + ' |');
      }
      return;
    }
    for (const c of el.children) walk(c, depth + 1);
  }
  walk(document.body, 0);
  return out.filter(Boolean).join('\n');
})()
"#;

pub(crate) const DOM_WALKER_OUTLINE: &str = r#"
(function () {
  const out = [];
  for (const h of document.querySelectorAll('h1,h2,h3,h4,h5,h6')) {
    out.push('#'.repeat(+h.tagName[1]) + ' ' + h.innerText.trim());
  }
  for (const a of document.querySelectorAll('a[href]')) {
    const t = (a.innerText || '').trim();
    if (t) out.push('- [' + t + '](' + a.href + ')');
  }
  return out.join('\n');
})()
"#;

pub(crate) const DOM_WALKER_RAW: &str = r#"
(function () { return document.body ? document.body.innerText : ''; })()
"#;

// ---------------------------------------------------------------------------
// tests — mock CdpSession
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CdpSession;
    use agentcowork_cdp::Session;
    use std::collections::HashMap;

    /// Scripted mock: answers known CDP calls with canned results.
    #[derive(Default)]
    struct MockSession {
        calls: std::sync::Mutex<Vec<(Option<String>, String, Value)>>,
        responses: HashMap<&'static str, Value>,
        ax: Value,
        url: String,
    }

    impl MockSession {
        fn with_ax(mut self, ax: Value) -> Self {
            self.ax = ax;
            self
        }
        fn with_url(mut self, url: &str) -> Self {
            self.url = url.into();
            self
        }
        fn calls(&self) -> Vec<(Option<String>, String)> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(s, m, _)| (s.clone(), m.clone()))
                .collect()
        }
        fn responded_methods(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(_, m, _)| m.clone())
                .collect()
        }
    }

    fn ax_node(id: i64, role: &str, name: &str) -> Value {
        json!({
            "nodeId": id.to_string(),
            "role": { "value": role },
            "name": { "value": name },
            "backendDOMNodeId": id + 100,
            "childIds": [],
            "properties": [],
        })
    }

    impl CdpSession for MockSession {
        fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
            self.calls
                .lock()
                .unwrap()
                .push((None, method.to_string(), params));
            Ok(self.responses.get(method).cloned().unwrap_or(json!({})))
        }
        fn call_session(
            &self,
            session_id: &str,
            method: &str,
            params: Value,
        ) -> Result<Value, CdpError> {
            self.calls.lock().unwrap().push((
                Some(session_id.to_string()),
                method.to_string(),
                params.clone(),
            ));
            match method {
                "Page.getFrameTree" => Ok(json!({
                    "frameTree": { "frame": { "url": self.url } }
                })),
                "Accessibility.getFullAXTree" => {
                    let nodes = self.ax.get("nodes").cloned().unwrap_or(self.ax.clone());
                    Ok(json!({ "nodes": nodes }))
                }
                "Runtime.evaluate" => Ok(json!({
                    "result": { "type": "string", "value": "# Mock Page\n\nbody content" }
                })),
                "DOM.getBoxModel" => Ok(json!({
                    "model": { "content": [0, 0, 100, 0, 100, 50, 0, 50] }
                })),
                "DOM.requestNode" => Ok(json!({ "result": { "nodeId": 7 } })),
                "DOM.resolveNode" => Ok(json!({ "object": { "objectId": "obj-1" } })),
                "DOMDebugger.getEventListeners" => Ok(json!({
                    "listeners": [{ "type": "click", "useCapture": false }]
                })),
                "Page.captureScreenshot" => Ok(json!({ "data": "fakebase64png" })),
                _ => Ok(json!({})),
            }
        }
        fn attach(&self, _target_id: &str) -> Result<Session, CdpError> {
            Err(CdpError::Protocol {
                code: -1,
                message: "no attach in mock".into(),
            })
        }
        fn drain_events(&self) -> Vec<agentcowork_cdp::CdpEvent> {
            Vec::new()
        }
    }

    fn mock() -> MockSession {
        MockSession::default().with_ax(json!({
            "nodes": [
                {
                    "nodeId": "1",
                    "role": { "value": "WebArea" },
                    "name": { "value": "" },
                    "backendDOMNodeId": 101,
                    "childIds": ["2"],
                    "properties": [],
                },
                ax_node(2, "button", "Go"),
            ]
        }))
    }

    #[test]
    fn navigate_goto_calls_page_navigate() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        a.navigate(NavigateAction::Goto {
            url: "https://example.com".into(),
        })
        .unwrap();
        let methods = m.responded_methods();
        assert!(methods.contains(&"Page.navigate".to_string()));
    }

    #[test]
    fn snapshot_captures_and_mints_refs() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let snap = a.snapshot("doc-1").unwrap();
        let rendered = snap.root.render();
        assert!(rendered.contains("button Go [ref=e1]"));
        // backendDOMNodeId must thread through for act geometry.
        let node = find_ref(&snap.root, "e1").unwrap();
        assert_eq!(node.backend_dom_node_id, Some(102));
    }

    #[test]
    fn act_click_resolves_ref_and_dispatches_mouse() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let res = a
            .act(ActKind::Click {
                ref_id: "e1".into(),
            })
            .unwrap();
        assert_eq!(res.kind, "click");
        let methods = m.responded_methods();
        assert!(methods.contains(&"DOM.getBoxModel".to_string()));
        assert!(methods.contains(&"Input.dispatchMouseEvent".to_string()));
    }

    #[test]
    fn act_click_at_dispatches_at_point() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        a.act(ActKind::ClickAt { x: 50.0, y: 25.0 }).unwrap();
        let calls = m.calls();
        let mouse = calls.iter().find(|(_, m)| m == "Input.dispatchMouseEvent");
        assert!(
            mouse.is_some(),
            "expected Input.dispatchMouseEvent, got {:?}",
            calls
        );
    }

    #[test]
    fn act_type_inserts_text() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        a.act(ActKind::Type {
            ref_id: "e1".into(),
            text: "hi".into(),
        })
        .unwrap();
        assert!(
            m.responded_methods()
                .contains(&"Input.insertText".to_string())
        );
    }

    #[test]
    fn act_fill_writes_all_fields_in_one_call() {
        let m = mock().with_ax(json!({
            "nodes": [
                {
                    "nodeId": "1",
                    "role": { "value": "WebArea" },
                    "name": { "value": "" },
                    "backendDOMNodeId": 101,
                    "childIds": ["2", "3"],
                    "properties": [],
                },
                ax_node(2, "textbox", "Email"),
                ax_node(3, "textbox", "Name"),
            ]
        }));
        let a = BrowserActions::new(&m, Some("sess-1"));
        a.act(ActKind::Fill {
            fields: vec![
                FieldValue {
                    ref_id: "e1".into(),
                    value: "a".into(),
                },
                FieldValue {
                    ref_id: "e2".into(),
                    value: "b".into(),
                },
            ],
        })
        .unwrap();
        let inserts = m
            .responded_methods()
            .iter()
            .filter(|m| m.as_str() == "Input.insertText")
            .count();
        assert_eq!(inserts, 2);
    }

    #[test]
    fn act_returns_post_settle_diff() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let res = a
            .act(ActKind::Click {
                ref_id: "e1".into(),
            })
            .unwrap();
        let d = res.diff.expect("act must return a diff");
        assert!(!d.url_changed);
    }

    #[test]
    fn navigate_back_uses_history_entry() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let _ = a.navigate(NavigateAction::Back);
        // History index 0 → no navigation (guarded).
        assert!(
            m.responded_methods()
                .contains(&"Page.getNavigationHistory".to_string())
        );
    }

    #[test]
    fn read_runs_dom_walker() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let r = a.read(ReadMode::Full).unwrap();
        assert!(r.text.contains("# Mock Page"));
        let outline = a.read(ReadMode::Outline).unwrap();
        assert_eq!(outline.text, "# Mock Page\n\nbody content");
    }

    #[test]
    fn js_click_handler_detects_click_listener() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        assert!(a.js_click_handler(101).unwrap());
        assert!(
            m.responded_methods()
                .contains(&"DOMDebugger.getEventListeners".to_string())
        );
        assert!(
            m.responded_methods()
                .contains(&"DOM.resolveNode".to_string())
        );
    }

    #[test]
    fn enhanced_snapshot_flags_occluded() {
        let m = mock().with_ax(json!({
            "nodes": [
                {
                    "nodeId": "1",
                    "role": { "value": "WebArea" },
                    "name": { "value": "" },
                    "backendDOMNodeId": 101,
                    "childIds": ["2"],
                    "properties": [],
                },
                ax_node(2, "button", "ProceedAnyway"),
            ]
        }));
        let a = BrowserActions::new(&m, Some("sess-1"));
        let es = a.enhanced_snapshot("doc-1").unwrap();
        assert_eq!(es.snapshot.mode, SnapshotMode::Interactive);
        // elementFromPoint returns "# Mock Page" ≠ "ProceedAnyway" (>= 3
        // chars) → e1 flagged occluded.
        assert!(es.occluded.contains(&"e1".to_string()));
    }

    #[test]
    fn enhanced_snapshot_skips_short_names() {
        // Short/shared names ("Go") must NOT be flagged — avoids false
        // positives from the elementFromPoint heuristic.
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let es = a.enhanced_snapshot("doc-1").unwrap();
        assert!(
            es.occluded.is_empty(),
            "short names must not be flagged: {:?}",
            es.occluded
        );
    }

    #[test]
    fn annotated_screenshot_returns_labels_and_image() {
        let m = mock(); // button "Go" @ backend 102, content quad center (50,25)
        let a = BrowserActions::new(&m, Some("sess-1"));
        let ann = a.annotated_screenshot("doc-1", 70).unwrap();
        assert_eq!(ann.screenshot, "fakebase64png");
        assert_eq!(ann.labels.len(), 1);
        let l = &ann.labels[0];
        assert_eq!(l.ref_id, "e1");
        assert_eq!(l.label, "Go");
        assert_eq!(l.x, 50.0);
        assert_eq!(l.y, 25.0);
    }

    #[test]
    fn tabs_lists_targets() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let _ = a.tabs().unwrap();
        assert!(
            m.responded_methods()
                .contains(&"Target.getTargets".to_string())
        );
    }

    #[test]
    fn close_tab_calls_target_close_target() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        a.close_tab("tab-42").unwrap();
        assert!(
            m.responded_methods()
                .contains(&"Target.closeTarget".to_string())
        );
        assert!(m.calls().iter().any(|(_, mth)| mth == "Target.closeTarget"));
    }

    #[test]
    fn find_ref_handles_missing() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let snap = a.snapshot("doc-1").unwrap();
        assert!(find_ref(&snap.root, "nope").is_none());
        assert!(find_ref(&snap.root, "e1").is_some());
    }

    // P46.4 — the act path refuses a stale ref: snapshot → act (invalidate)
    // → act again on the same ref must fail with the re-observe error.
    #[test]
    fn act_refuses_stale_ref_after_mutation() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        // Snapshot 1 registers e1 as current.
        a.snapshot("doc-1").unwrap();
        // First act consumes it and invalidates the generation.
        a.act(ActKind::Click {
            ref_id: "e1".into(),
        })
        .unwrap();
        // Second act on the same ref: the registry knows e1 is stale.
        let err = a
            .act(ActKind::Click {
                ref_id: "e1".into(),
            })
            .unwrap_err();
        assert!(err.to_string().contains("stale"), "got: {err}");
        // Re-observe (fresh snapshot) re-stamps e1 → act works again.
        a.snapshot("doc-2").unwrap();
        a.act(ActKind::Click {
            ref_id: "e1".into(),
        })
        .unwrap();
    }

    #[test]
    fn wait_for_ms_returns_satisfied() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let out = a.wait(WaitFor::Ms(1), 100).unwrap();
        assert_eq!(out, WaitOutcome::Satisfied);
    }

    #[test]
    fn wait_for_selector_polls() {
        let m = mock();
        let a = BrowserActions::new(&m, Some("sess-1"));
        let out = a.wait(WaitFor::Selector("#btn".into()), 50).unwrap();
        assert_eq!(out, WaitOutcome::TimedOut);
    }

    // -------------------------------------------------------------------
    // P2.9 — behavioral realism (Bézier mouse + typing cadence, per-site)
    // -------------------------------------------------------------------

    /// Fast, deterministic, humanized profile restricted to example.com.
    fn human_profile() -> crate::humanize::BehaviorProfile {
        let mut b = crate::humanize::BehaviorProfile::human()
            .for_sites(&["example.com"])
            .seeded(42);
        b.typing.cpm = 6000.0; // ~10ms/char — keeps tests quick
        b.typing.word_pause_ms = 0;
        b.mouse.move_delay_ms = (0, 1);
        b
    }

    fn mouse_event_types(m: &MockSession) -> Vec<String> {
        m.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, mth, _)| mth == "Input.dispatchMouseEvent")
            .filter_map(|(_, _, p)| p.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect()
    }

    #[test]
    fn humanized_click_dispatches_bezier_moves() {
        let m = mock().with_url("https://example.com/");
        let a = BrowserActions::new(&m, Some("sess-1")).with_behavior(human_profile());
        let res = a
            .act(ActKind::Click {
                ref_id: "e1".into(),
            })
            .unwrap();
        assert!(res.note.as_deref() == Some("humanized (P2.9)"));
        let types = mouse_event_types(&m);
        // Bézier path: several mouseMoved steps, then press + release.
        let moves = types.iter().filter(|t| t.as_str() == "mouseMoved").count();
        assert!(
            moves >= 2,
            "expected a Bézier path, got {moves} moves: {types:?}"
        );
        assert!(types.ends_with(&["mousePressed".into(), "mouseReleased".into()]));
    }

    #[test]
    fn plain_click_has_no_mouse_moves() {
        let m = mock().with_url("https://example.com/");
        let a = BrowserActions::new(&m, Some("sess-1")); // behavior off by default
        a.act(ActKind::Click {
            ref_id: "e1".into(),
        })
        .unwrap();
        assert!(
            mouse_event_types(&m).iter().all(|t| t != "mouseMoved"),
            "plain click must not emit mouseMoved"
        );
    }

    #[test]
    fn humanized_typing_dispatches_per_key_events() {
        let m = mock().with_url("https://example.com/");
        let a = BrowserActions::new(&m, Some("sess-1")).with_behavior(human_profile());
        a.act(ActKind::Type {
            ref_id: "e1".into(),
            text: "hi".into(),
        })
        .unwrap();
        let calls = m.calls.lock().unwrap();
        let texts: Vec<String> = calls
            .iter()
            .filter(|(_, mth, _)| mth == "Input.dispatchKeyEvent")
            .filter_map(|(_, _, p)| p.get("text").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert_eq!(texts, vec!["h".to_string(), "i".to_string()]);
        assert!(
            !calls.iter().any(|(_, mth, _)| mth == "Input.insertText"),
            "humanized typing must not use one-shot insertText"
        );
    }

    #[test]
    fn behavior_is_site_gated() {
        // Profile restricted to other.com → example.com stays plain.
        let m = mock().with_url("https://example.com/");
        let mut b = human_profile().for_sites(&["other.com"]);
        b.mouse.click_jitter_px = 0.0;
        let a = BrowserActions::new(&m, Some("sess-1")).with_behavior(b);
        let res = a
            .act(ActKind::Click {
                ref_id: "e1".into(),
            })
            .unwrap();
        assert!(
            res.note.is_none(),
            "site must gate humanization: {:?}",
            res.note
        );
        assert!(
            mouse_event_types(&m).iter().all(|t| t != "mouseMoved"),
            "other.com profile must not humanize example.com"
        );
    }

    #[test]
    fn humanized_drag_releases_at_exact_target() {
        let m = mock().with_url("https://example.com/");
        let a = BrowserActions::new(&m, Some("sess-1")).with_behavior(human_profile());
        a.act(ActKind::DragAt {
            from_x: 0.0,
            from_y: 0.0,
            to_x: 200.0,
            to_y: 120.0,
        })
        .unwrap();
        let calls = m.calls.lock().unwrap();
        let released = calls
            .iter()
            .filter(|(_, mth, _)| mth == "Input.dispatchMouseEvent")
            .find(|(_, _, p)| p.get("type").and_then(|t| t.as_str()) == Some("mouseReleased"))
            .map(|(_, _, p)| p.clone())
            .unwrap();
        assert_eq!(released["x"], json!(200.0));
        assert_eq!(released["y"], json!(120.0));
    }
}
