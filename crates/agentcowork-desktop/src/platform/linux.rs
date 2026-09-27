//! X11 backend (Linux) — live-tested under Xvfb in the E2E suite.
//!
//! - **See:** `GetImage` (Z_PIXMAP) on the window drawable → PNG.
//! - **Read:** EWMH `_NET_CLIENT_LIST` + `_NET_WM_NAME` + geometry; no UIA
//!   equivalent on bare X11 (AT-SPI is a follow-on) — `read_tree` returns
//!   `None` and the vision fallback (OCR) takes over.
//! - **Act:** XTEST fake input (button / motion / key), keysym lookup via
//!   `GetKeyboardMapping`. **P57.3:** `set_input_focus` + raise happen only on
//!   the foreground path — under the Background default no window is focused or
//!   restacked. **P57.1:** launch executes the canonical path directly (no
//!   `sh -c`, no PATH string interpolation); a bare name is a documented
//!   fallback resolved by looking in `PATH` ourselves.
//! - **DPI:** `Xft.dpi` root property (default 96 → scale 1.0).

use std::process::Command;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::protocol::xproto::{
    AtomEnum, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ButtonPressEvent, ConfigureWindowAux,
    EventMask, ImageFormat, InputFocus, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, KeyButMask,
    MOTION_NOTIFY_EVENT, StackMode, Window,
};
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::DesktopError;
use crate::capture::{CaptureCheck, CaptureFault, CapturePipeline, CaptureReadiness};
use crate::geometry::{DpiScale, DpiSource};
use crate::ladder::{ClickRung, LadderTarget, RungDelivery};
use crate::launch;
use crate::policy::InteractionMode;
use crate::types::{ActKind, ReadResult, Region, SeeMethod, SeeResult, WindowInfo};

pub struct X11Backend {
    conn: RustConnection,
    root: Window,
}

fn prop_string(conn: &RustConnection, window: Window, atom: u32) -> Option<String> {
    let reply = conn
        .get_property(false, window, atom, AtomEnum::ANY, 0, 4096)
        .ok()?
        .reply()
        .ok()?;
    let bytes: Vec<u8> = reply.value8()?.collect();
    let text = String::from_utf8_lossy(&bytes).to_string();
    if text.is_empty() { None } else { Some(text) }
}

impl X11Backend {
    pub fn connect() -> Result<Self, DesktopError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|e| {
            DesktopError::Platform(format!("X11 connect failed (DISPLAY set?): {e}"))
        })?;
        let root = conn.setup().roots[screen_num].root;
        Ok(Self { conn, root })
    }

    pub fn is_available() -> bool {
        std::env::var("DISPLAY").is_ok() && x11rb::connect(None).is_ok()
    }

    fn atom(&self, name: &[u8]) -> Option<u32> {
        self.conn
            .intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    }

    fn window_app(&self, window: Window) -> String {
        if let Some(pid_atom) = self.atom(b"_NET_WM_PID") {
            if let Some(reply) = self
                .conn
                .get_property(false, window, pid_atom, AtomEnum::ANY, 0, 1)
                .ok()
                .and_then(|c| c.reply().ok())
            {
                if let Some(pid) = reply.value32().and_then(|mut v| v.next()) {
                    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
                        return comm.trim().to_string();
                    }
                }
            }
        }
        "unknown".into()
    }

    fn window_geometry(&self, window: Window) -> Option<(i32, i32, u32, u32)> {
        let geo = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let tr = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some((
            i32::from(tr.dst_x),
            i32::from(tr.dst_y),
            u32::from(geo.width),
            u32::from(geo.height),
        ))
    }

    fn window_title(&self, window: Window) -> String {
        if let Some(a) = self.atom(b"_NET_WM_NAME") {
            if let Some(t) = prop_string(&self.conn, window, a) {
                return t;
            }
        }
        if let Some(a) = self.atom(b"WM_NAME") {
            if let Some(t) = prop_string(&self.conn, window, a) {
                return t;
            }
        }
        String::new()
    }

    /// P57.4 — where the pointer actually is, in root coordinates, read without
    /// moving it. The Background click path must never change this; the live E2E
    /// test asserts that against a real X server.
    pub fn pointer_position(&self) -> Result<(i32, i32), DesktopError> {
        let reply = self
            .conn
            .query_pointer(self.root)
            .map_err(|e| DesktopError::Platform(format!("query_pointer: {e}")))?
            .reply()
            .map_err(|e| DesktopError::Platform(format!("query_pointer reply: {e}")))?;
        Ok((i32::from(reply.root_x), i32::from(reply.root_y)))
    }

    /// P57.4 — the window that currently owns the input focus, per the EWMH
    /// `_NET_ACTIVE_WINDOW` root property. `None` when the WM does not publish
    /// one, so an approved escalation's restore becomes a no-op instead of a
    /// guess (and the engine reports it honestly).
    pub fn foreground_window(&self) -> Option<u64> {
        let atom = self.atom(b"_NET_ACTIVE_WINDOW")?;
        let reply = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        let id = reply.value32()?.next()?;
        if id == 0 { None } else { Some(u64::from(id)) }
    }

    /// P57.4 — hand the foreground back after an approved escalation. Raising +
    /// focusing is intrinsically a foreground operation, so this is only reached
    /// from the restore half of an approved escalation.
    pub fn restore_foreground(&self, window_id: u64) -> Result<(), DesktopError> {
        let w = Window::from(window_id as u32);
        self.conn
            .configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))
            .map_err(|e| DesktopError::Platform(format!("restore stack: {e}")))?;
        self.conn
            .set_input_focus(InputFocus::PARENT, w, 0u32)
            .map_err(|e| DesktopError::Platform(format!("restore focus: {e}")))?;
        self.conn
            .flush()
            .map_err(|e| DesktopError::Platform(format!("flush: {e}")))
    }

    pub fn list_windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        // Prefer the EWMH client list (set by a WM); fall back to a raw
        // XQueryTree walk of mapped top-level windows when no WM is running
        // (headless Xvfb, minimal sessions).
        if let Some(ewmh) = self.list_windows_ewmh() {
            return Ok(ewmh);
        }
        self.list_windows_query_tree()
    }

    fn list_windows_ewmh(&self) -> Option<Vec<WindowInfo>> {
        let client_list = self.atom(b"_NET_CLIENT_LIST")?;
        let reply = self
            .conn
            .get_property(false, self.root, client_list, AtomEnum::ANY, 0, 4096)
            .ok()?
            .reply()
            .ok()?;
        let mut out = Vec::new();
        for w in reply.value32()? {
            let window = Window::from(w);
            if let Some(info) = self.window_info(window) {
                out.push(info);
            }
        }
        Some(out)
    }

    fn list_windows_query_tree(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        let mut out = Vec::new();
        let mut stack = vec![self.root];
        // Only direct children of the root are top-level candidates; children
        // of other windows are reparented frames we must not double-count.
        let mut seen = std::collections::HashSet::new();
        while let Some(window) = stack.pop() {
            if !seen.insert(window) {
                continue;
            }
            let Some(reply) = self
                .conn
                .query_tree(window)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                continue;
            };
            let children: Vec<Window> = reply.children;
            if window == self.root {
                // Root children: mapped + viewable → top-level window.
                for child in children {
                    let mapped = self
                        .conn
                        .get_window_attributes(child)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .map(|a| a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE)
                        .unwrap_or(false);
                    if mapped {
                        if let Some(info) = self.window_info(child) {
                            out.push(info);
                        }
                    }
                }
            } else {
                // Reparented frame — descend to find the client window.
                stack.extend(children);
            }
        }
        Ok(out)
    }

    fn window_info(&self, window: Window) -> Option<WindowInfo> {
        let title = self.window_title(window);
        let app = self.window_app(window);
        if title.is_empty() && app == "unknown" {
            return None;
        }
        let (x, y, width, height) = self.window_geometry(window)?;
        Some(WindowInfo {
            id: u64::from(window),
            title,
            app,
            x,
            y,
            width,
            height,
            has_a11y_tree: false,
        })
    }

    pub fn read(&self, window: &WindowInfo) -> Result<ReadResult, DesktopError> {
        let windows = self.list_windows()?;
        // `FIX-17` — the honest shape of "this platform exposes no structured UI":
        // an `Absent` read, which is a **positive** fact (so the vision/OCR rung
        // is the documented next step) rather than an `Unknown` that would mean
        // "I was not allowed to look". Bare X11 has no AT-SPI client in the
        // dependency set, so there is nothing to be blocked from.
        Ok(ReadResult::absent(
            window.id,
            self.dpi_scale().factor,
            windows,
        ))
    }

    /// The measured X11 scale, with its provenance.
    ///
    /// X11 has **no per-window scale**: `XTEST` fake input, `SendEvent` and
    /// `XGetImage` all address device pixels whatever the font DPI says. So the
    /// only honest sources are the root `Xft.dpi` property (a global font hint,
    /// which is why [`DpiScale::applies_to_click_coordinates`] is false for it
    /// and no click is ever divided by it) and the honest "not measured"
    /// answer when the property is absent.
    pub fn dpi_scale(&self) -> DpiScale {
        self.xft_dpi()
            .map(|dpi| DpiScale::from_dpi(dpi, DpiSource::XftProperty))
            .unwrap_or_else(DpiScale::unknown)
    }

    fn xft_dpi(&self) -> Option<u32> {
        let a = self.atom(b"Xft.dpi")?;
        let reply = self
            .conn
            .get_property(false, self.root, a, AtomEnum::ANY, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        let dpi = reply.value32().and_then(|mut v| v.next())?;
        (dpi > 0).then_some(dpi)
    }

    // ---- the click ladder --------------------------------------------

    /// Attempt one rung of the click ladder.
    ///
    /// X11 can deliver a click two ways and the ladder names both:
    /// a synthetic `ButtonPress`/`ButtonRelease` addressed to the target window
    /// ([`ClickRung::SyntheticEvent`], no pointer motion, no focus) and XTEST
    /// fake motion + button ([`ClickRung::RawInput`], moves the real pointer).
    /// There is no accessibility rung: no AT-SPI client is linked, which is
    /// also why [`Self::read`] returns no tree and the vision/OCR fallback
    /// exists. A named click has no X11 surface at all, so it reports
    /// `Unavailable` with that reason rather than pretending.
    pub fn deliver_rung(
        &self,
        rung: ClickRung,
        target: &LadderTarget,
        _mode: InteractionMode,
    ) -> RungDelivery {
        let window = Window::from(target.window.id as u32);
        // Resolve the point the rung should act on. A named act has no
        // coordinate of its own; only the OCR/vision path can produce one.
        let point = match &target.act {
            ActKind::Click { x, y } => Some((*x, *y)),
            ActKind::ClickByName { name } => {
                return RungDelivery::Unavailable(format!(
                    "X11 has no named-click surface (\"{name}\"): resolve the control through \
                     the OCR/vision fallback and act on the coordinate"
                ));
            }
            other => {
                return RungDelivery::Unavailable(format!(
                    "rung {} does not apply to {}",
                    rung.as_str(),
                    other.describe()
                ));
            }
        };
        let (x, y) = point.expect("either a coordinate or an early return");
        let Some((wx, wy, _, _)) = self.window_geometry(window) else {
            return RungDelivery::Unavailable(format!("window {} is gone", target.window.id));
        };
        match rung {
            ClickRung::SyntheticEvent => self
                .synthetic_click(window, x, y)
                .map(|()| {
                    RungDelivery::Delivered(
                        "synthetic ButtonPress/ButtonRelease addressed to the deepest child \
                             under the point (no pointer motion, no focus change)"
                            .into(),
                    )
                })
                .unwrap_or_else(|e| RungDelivery::Failed(e.to_string())),
            ClickRung::RawInput => {
                if !self.xtest_available() {
                    return RungDelivery::Failed(
                        "XTEST extension not available on this X server".into(),
                    );
                }
                let (sx, sy) = (wx + x, wy + y);
                let narrow = |e: DesktopError| RungDelivery::Failed(e.to_string());
                if let Err(e) = self.fake_motion(sx as i16, sy as i16) {
                    return narrow(e);
                }
                if let Err(e) = self.fake_button(1, true, sx as i16, sy as i16) {
                    return narrow(e);
                }
                if let Err(e) = self.fake_button(1, false, sx as i16, sy as i16) {
                    return narrow(e);
                }
                RungDelivery::Delivered(format!("XTEST click at ({sx},{sy}) — pointer moved"))
            }
            ClickRung::AccessibilityInvoke => RungDelivery::Unavailable(
                "no accessibility invoke on bare X11: no AT-SPI client is linked".into(),
            ),
        }
    }

    pub fn see(&self, window: &WindowInfo, region: Region) -> Result<SeeResult, DesktopError> {
        let win = Window::from(window.id as u32);
        let (gx, gy, gw, gh) = self
            .window_geometry(win)
            .ok_or_else(|| DesktopError::Platform(format!("window {} gone", window.id)))?;
        // `region` is window-relative; `get_image` offsets are window-relative
        // too (NOT screen coords — screen-relative would mis-capture windows
        // that are not at the origin). Clamp to the window's own bounds.
        let bounds = Region {
            x: 0,
            y: 0,
            width: gw,
            height: gh,
        };
        let r = bounds
            .intersect(&region)
            .ok_or_else(|| DesktopError::InvalidRegion("requested region outside window".into()))?;
        let x = r.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let y = r.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let width = r.width.min(u16::MAX as u32) as u16;
        let height = r.height.min(u16::MAX as u32) as u16;
        let _ = (gx, gy); // screen origin is caller knowledge, not used here
        let img = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, win, x, y, width, height, !0)
            .ok()
            .and_then(|c| c.reply().ok())
            .ok_or_else(|| DesktopError::Platform("GetImage failed".into()))?;
        let bpp = 4; // ZPixmap 24/32-depth windows → 4 bytes/pixel
        let bytes_per_row = (width as usize) * bpp;
        let data = &img.data;
        let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for row in 0..height as usize {
            let start = row * bytes_per_row;
            for col in 0..width as usize {
                let i = start + col * bpp;
                if i + 2 >= data.len() {
                    continue;
                }
                let b = data[i];
                let g = data[i + 1];
                let rp = data[i + 2];
                rgba.extend_from_slice(&[rp, g, b, 255]);
            }
        }
        let buf = image::RgbaImage::from_raw(u32::from(width), u32::from(height), rgba)
            .ok_or_else(|| DesktopError::Platform("image buffer malformed".into()))?;
        let mut png: Vec<u8> = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png);
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| DesktopError::Platform(format!("png encode: {e}")))?;
        Ok(SeeResult {
            window_id: window.id,
            png,
            width: u32::from(width),
            height: u32::from(height),
            method: SeeMethod::X11GetImage,
            region: r,
            scale: self.dpi_scale().factor,
            dpi: self.dpi_scale(),
            // The engine (`DesktopEngine::see`) applies the output budget; a
            // direct backend call has had none applied, and says so via `None`.
            budget: None,
            // `FIX-18` — X11 offers exactly one pipeline and the backend verified
            // it before calling `get_image`: the connection is live, the window
            // resolved to a drawable, and the extent is non-zero. The verdict
            // travels with the capture so a later reader can tell "captured" from
            // "captured despite a degrade".
            readiness: CaptureReadiness::ready(
                CapturePipeline::X11GetImage,
                vec![
                    CaptureCheck::PipelineSupported,
                    CaptureCheck::WindowHandleValid,
                    CaptureCheck::NonZeroExtent,
                ],
            ),
        })
    }

    // ---- Act (XTEST) ----

    fn xtest_available(&self) -> bool {
        self.conn
            .extension_information(xtest::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some()
    }

    fn fake_button(&self, button: u8, press: bool, x: i16, y: i16) -> Result<(), DesktopError> {
        let event = if press {
            BUTTON_PRESS_EVENT
        } else {
            BUTTON_RELEASE_EVENT
        };
        xtest::fake_input(&self.conn, event, button, 0, self.root, x, y, 0)
            .map_err(|e| DesktopError::Platform(format!("xtest button: {e}")))?;
        self.conn
            .flush()
            .map_err(|e| DesktopError::Platform(format!("flush: {e}")))
    }

    fn fake_motion(&self, x: i16, y: i16) -> Result<(), DesktopError> {
        xtest::fake_input(&self.conn, MOTION_NOTIFY_EVENT, 0, 0, self.root, x, y, 0)
            .map_err(|e| DesktopError::Platform(format!("xtest motion: {e}")))?;
        self.conn
            .flush()
            .map_err(|e| DesktopError::Platform(format!("flush: {e}")))
    }

    /// Resolve a keysym → (keycode, needs_shift) via GetKeyboardMapping.
    fn keysym_to_keycode(&self, keysym: u32) -> Option<(u8, bool)> {
        let setup = self.conn.setup();
        let min = setup.min_keycode;
        let count = setup.max_keycode - min;
        let reply = self
            .conn
            .get_keyboard_mapping(min, count)
            .ok()?
            .reply()
            .ok()?;
        let per = reply.keysyms_per_keycode as usize;
        let min = min as usize;
        for (i, chunk) in reply.keysyms.chunks(per).enumerate() {
            let keycode = (min + i) as u8;
            for (idx, ks) in chunk.iter().enumerate() {
                if *ks == keysym {
                    return Some((keycode, idx >= 1));
                }
            }
        }
        None
    }

    fn fake_key(&self, keysym: u32) -> Result<(), DesktopError> {
        let (keycode, needs_shift) = self
            .keysym_to_keycode(keysym)
            .ok_or_else(|| DesktopError::Platform(format!("no keycode for keysym 0x{keysym:x}")))?;
        if needs_shift {
            // Shift_L = 0xffe1
            if let Some((shift_code, _)) = self.keysym_to_keycode(0xffe1) {
                xtest::fake_input(
                    &self.conn,
                    KEY_PRESS_EVENT,
                    shift_code,
                    0,
                    self.root,
                    0,
                    0,
                    0,
                )
                .map_err(|e| DesktopError::Platform(format!("xtest shift: {e}")))?;
            }
        }
        xtest::fake_input(&self.conn, KEY_PRESS_EVENT, keycode, 0, self.root, 0, 0, 0)
            .map_err(|e| DesktopError::Platform(format!("xtest key: {e}")))?;
        xtest::fake_input(
            &self.conn,
            KEY_RELEASE_EVENT,
            keycode,
            0,
            self.root,
            0,
            0,
            0,
        )
        .map_err(|e| DesktopError::Platform(format!("xtest key: {e}")))?;
        if needs_shift {
            if let Some((shift_code, _)) = self.keysym_to_keycode(0xffe1) {
                xtest::fake_input(
                    &self.conn,
                    KEY_RELEASE_EVENT,
                    shift_code,
                    0,
                    self.root,
                    0,
                    0,
                    0,
                )
                .map_err(|e| DesktopError::Platform(format!("xtest shift: {e}")))?;
            }
        }
        self.conn
            .flush()
            .map_err(|e| DesktopError::Platform(format!("flush: {e}")))
    }

    fn named_key_to_keysym(key: &str) -> Option<u32> {
        match key.to_ascii_lowercase().as_str() {
            "enter" | "return" => Some(0xff0d),
            "tab" => Some(0xff09),
            "space" => Some(0x20),
            "escape" | "esc" => Some(0xff1b),
            "backspace" => Some(0xff08),
            "delete" => Some(0xffff),
            "left" => Some(0xff51),
            "up" => Some(0xff52),
            "right" => Some(0xff53),
            "down" => Some(0xff54),
            "home" => Some(0xff50),
            "end" => Some(0xff57),
            "pageup" => Some(0xff55),
            "pagedown" => Some(0xff56),
            "f1" => Some(0xffbe),
            "f2" => Some(0xffbf),
            "f3" => Some(0xffc0),
            "f4" => Some(0xffc1),
            "f5" => Some(0xffc2),
            "f6" => Some(0xffc3),
            "f7" => Some(0xffc4),
            "f8" => Some(0xffc5),
            "f9" => Some(0xffc6),
            "f10" => Some(0xffc7),
            "f11" => Some(0xffc8),
            "f12" => Some(0xffc9),
            _ => {
                let b = key.as_bytes();
                if b.len() == 1 {
                    Some(u32::from(b[0]))
                } else {
                    None
                }
            }
        }
    }

    fn type_char(&self, c: char) -> Result<(), DesktopError> {
        let keysym = c as u32; // ASCII/Latin-1 keysyms equal the codepoint
        self.fake_key(keysym)
    }

    /// P57.3 — the deepest mapped descendant of `window` that contains the
    /// window-relative point, honouring each level's own geometry. X11 has no
    /// "window at point" that works without the pointer, so this walks the tree
    /// by hand (bounded depth — degenerate window trees exist).
    fn deepest_child_at(&self, window: Window, x: i32, y: i32) -> Window {
        let mut current = window;
        let (mut cx, mut cy) = (x, y);
        for _ in 0..8 {
            let Some(reply) = self
                .conn
                .query_tree(current)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                return current;
            };
            let mut next: Option<(Window, i32, i32)> = None;
            // Children are in stacking order (bottom → top); the last match is
            // the visible one.
            for child in reply.children {
                let Some((gx, gy, gw, gh)) = self.window_geometry(child) else {
                    continue;
                };
                let tr = self
                    .conn
                    .translate_coordinates(current, child, cx as i16, cy as i16)
                    .ok()
                    .and_then(|c| c.reply().ok());
                let (lx, ly) = tr
                    .map(|t| (i32::from(t.dst_x), i32::from(t.dst_y)))
                    .unwrap_or((cx, cy));
                let (_, _, cw, ch) = self.window_geometry(current).unwrap_or((0, 0, 1, 1));
                let inside = (gx, gy, gw, gh) != (0, 0, 0, 0);
                if inside && lx >= 0 && ly >= 0 && lx < cw as i32 && ly < ch as i32 {
                    next = Some((child, lx, ly));
                }
            }
            match next {
                Some((child, lx, ly)) => {
                    current = child;
                    cx = lx;
                    cy = ly;
                }
                None => return current,
            }
        }
        current
    }

    /// P57.3 — a click that does not move the user's pointer: a synthetic
    /// `ButtonPress`/`ButtonRelease` pair addressed to the deepest child under
    /// the point, with `propagate` so the event reaches whichever client
    /// actually selected the button mask. XTEST is never used here.
    ///
    /// Whether an app honours a synthetic event is the app's decision (some
    /// toolkits ignore `send_event: true`), so a refusal is reported honestly
    /// rather than papered over — that is the signal to escalate to Foreground.
    fn synthetic_click(&self, window: Window, x: i32, y: i32) -> Result<(), DesktopError> {
        let target = self.deepest_child_at(window, x, y);
        let (root_x, root_y) = self
            .conn
            .translate_coordinates(window, self.root, x as i16, y as i16)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|t| (i32::from(t.dst_x), i32::from(t.dst_y)))
            .unwrap_or((x, y));
        let (ex, ey) = self
            .conn
            .translate_coordinates(window, target, x as i16, y as i16)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|t| (i32::from(t.dst_x), i32::from(t.dst_y)))
            .unwrap_or((x, y));
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
        for response_type in [BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT] {
            let event = ButtonPressEvent {
                response_type,
                detail: 1, // button 1
                sequence: 0,
                time: x11rb::CURRENT_TIME,
                root: self.root,
                event: target,
                child: x11rb::NONE,
                root_x: root_x as i16,
                root_y: root_y as i16,
                event_x: ex as i16,
                event_y: ey as i16,
                state: KeyButMask::BUTTON1,
                same_screen: true,
            };
            self.conn
                .send_event(true, target, mask, event)
                .map_err(|e| DesktopError::Platform(format!("synthetic click: {e}")))?;
        }
        self.conn
            .flush()
            .map_err(|e| DesktopError::Platform(format!("flush: {e}")))
    }

    pub fn act(
        &self,
        window: &WindowInfo,
        act: &ActKind,
        mode: InteractionMode,
    ) -> Result<(), DesktopError> {
        // P57.1 — launch never needs XTEST (or the window geometry), so it is
        // handled before the input paths: `sh` is never involved.
        if let ActKind::LaunchApp { path, app } = act {
            let target = launch::resolve_target(path.as_deref(), app, &launch::path_dirs())?;
            let mut cmd = Command::new(&target);
            // No inherited stdio and no inherited credentials (H5). The launch
            // cannot raise or focus: on X11 a GUI app presents itself through
            // its own WM hints, and this path only starts the process.
            launch::prepare_child(&mut cmd);
            cmd.spawn()
                .map_err(|e| DesktopError::Platform(format!("launch {}: {e}", target.display())))?;
            return Ok(());
        }
        if !self.xtest_available() {
            return Err(DesktopError::Platform(
                "XTEST extension not available on this X server".into(),
            ));
        }
        let (wx, wy, _, _) = self
            .window_geometry(Window::from(window.id as u32))
            .ok_or_else(|| DesktopError::Platform(format!("window {} gone", window.id)))?;
        match act {
            ActKind::Click { x, y } => {
                // P57.3 — the background path delivers a synthetic button pair
                // to the target window itself, so the user's pointer and focus
                // are untouched. XTEST (which warps the real pointer) is the
                // foreground path only.
                if mode == InteractionMode::Background {
                    return self.synthetic_click(Window::from(window.id as u32), *x, *y);
                }
                let (sx, sy) = (wx + x, wy + y);
                self.fake_motion(sx as i16, sy as i16)?;
                self.fake_button(1, true, sx as i16, sy as i16)?;
                self.fake_button(1, false, sx as i16, sy as i16)
            }
            ActKind::Scroll { x, y, delta } => {
                let (sx, sy) = (wx + x, wy + y);
                self.fake_motion(sx as i16, sy as i16)?;
                let (up, down) = (4u8, 5u8);
                let n = delta.abs().min(50);
                for _ in 0..n {
                    let btn = if *delta > 0 { up } else { down };
                    self.fake_button(btn, true, sx as i16, sy as i16)?;
                    self.fake_button(btn, false, sx as i16, sy as i16)?;
                }
                Ok(())
            }
            ActKind::Drag { from, to } => {
                let (x0, y0) = (wx + from.0, wy + from.1);
                let (x1, y1) = (wx + to.0, wy + to.1);
                self.fake_motion(x0 as i16, y0 as i16)?;
                self.fake_button(1, true, x0 as i16, y0 as i16)?;
                for i in 1..=8 {
                    let t = i as f64 / 8.0;
                    let mx = (x0 as f64 + (x1 - x0) as f64 * t) as i16;
                    let my = (y0 as f64 + (y1 - y0) as f64 * t) as i16;
                    self.fake_motion(mx, my)?;
                }
                self.fake_button(1, false, x1 as i16, y1 as i16)
            }
            ActKind::Press { key } => {
                let ks = Self::named_key_to_keysym(key)
                    .ok_or_else(|| DesktopError::Platform(format!("unknown key {key}")))?;
                self.fake_key(ks)
            }
            ActKind::Type { text } => {
                for c in text.chars() {
                    if c == '\n' {
                        self.fake_key(0xff0d)?;
                    } else {
                        self.type_char(c)?;
                    }
                }
                Ok(())
            }
            ActKind::ActivateWindow { window_id } => {
                // P57.3 — the background contract: restacking a window above
                // the others and stealing input focus IS raising it. Under the
                // Background default this refuses instead (the engine already
                // refuses before reaching here; this is the backend's own
                // floor, so a direct `act` call cannot slip past it either).
                if mode == InteractionMode::Background {
                    return Err(DesktopError::Unsupported(
                        "background contract: X11 raising (stack above + set_input_focus) is a \
                         foreground escalation — switch the interaction default to Foreground"
                            .into(),
                    ));
                }
                let w = Window::from(*window_id as u32);
                let _ = self
                    .conn
                    .configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
                self.conn
                    .set_input_focus(InputFocus::PARENT, w, 0u32)
                    .map_err(|e| DesktopError::Platform(format!("set_input_focus: {e}")))?;
                self.conn
                    .flush()
                    .map_err(|e| DesktopError::Platform(format!("flush: {e}")))
            }
            ActKind::ClickByName { name } => Err(DesktopError::Platform(format!(
                "ClickByName has no X11 surface (OCR must resolve \"{name}\" first)"
            ))),
            ActKind::SetValue { name, .. } => Err(DesktopError::Platform(format!(
                "SetValue has no X11 surface (name \"{name}\")"
            ))),
            // Handled above (before the XTEST/geometry requirements). Kept as
            // an explicit error rather than a panic so a future reorder fails
            // the action honestly instead of taking the process down.
            ActKind::LaunchApp { .. } => Err(DesktopError::Platform(
                "launch was not handled on the pre-input path".into(),
            )),
        }
    }
}

/// The X11 capture-readiness probe (`FIX-18`).
///
/// X11 offers exactly one pipeline (`XGetImage` over the window drawable), so the
/// verdict is a one-check host story plus a two-check target story: a live display
/// connection, and a window with extent. There is no WGC equivalent to verify
/// here, and the probe says so rather than pretending a fallback exists.
pub struct X11CaptureProbe {
    /// Is a live X11 backend attached? A detached host has no display to capture,
    /// and says so instead of promising `XGetImage`.
    attached: bool,
}

impl X11CaptureProbe {
    pub fn new(attached: bool) -> Self {
        Self { attached }
    }
}

impl crate::capture::CaptureProbe for X11CaptureProbe {
    fn pipelines(&self) -> Vec<CapturePipeline> {
        vec![CapturePipeline::X11GetImage]
    }

    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault> {
        match self.attached {
            true => Ok(vec![CaptureCheck::PipelineSupported]),
            false => Err(CaptureFault::NoInteractiveSession {
                detail: "no X11 backend is attached (no DISPLAY)".into(),
            }),
        }
    }

    /// X11 has no occlusion concept this backend can answer cheaply, and the only
    /// pipeline reads the window's own drawable, so this is always "not occluded"
    /// — the direction that never blocks a legitimate capture.
    fn is_occluded(&mut self, _window: &WindowInfo) -> bool {
        false
    }

    fn target(
        &mut self,
        window: &WindowInfo,
        _pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault> {
        if window.width == 0 || window.height == 0 {
            return Err(CaptureFault::ZeroExtent {
                detail: format!("the listed window is {}x{}", window.width, window.height),
            });
        }
        Ok(vec![
            CaptureCheck::WindowHandleValid,
            CaptureCheck::NonZeroExtent,
        ])
    }
}
