//! Core E9 types — the union contract shared by every platform backend.
//!
//! One surface, three backends: X11 (Linux, live-tested), Win32 UIA +
//! SendInput + PrintWindow/screen-DC (Windows, cross-compiled), macOS AX +
//! ScreenCapture (subprocess). Everything here is platform-neutral.

use serde::{Deserialize, Serialize};

use crate::capture::{CaptureDegrade, CaptureReadiness};
use crate::geometry::{DpiScale, SeeBudget};
use crate::ladder::LadderVerdict;
use crate::uia::{FreshnessAnomaly, SnapshotEpoch, UiaReadStatus};

/// A desktop window as seen by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    /// Stable id for this window on this platform (HWND / window-id / CGWindowID).
    pub id: u64,
    /// Human title / name.
    pub title: String,
    /// Owning application / process name.
    pub app: String,
    /// Position + size in *physical* pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// UI-Automation / a11y tree availability for this window (Read uses it).
    pub has_a11y_tree: bool,
}

/// How a window capture was produced (honesty: never claim a method we didn't use).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SeeMethod {
    /// Windows.Graphics.Capture per-HWND — captures occluded windows.
    /// WGC is a WinRT interop seam on this build (see `capabilities()`).
    WindowsGraphicsCapture,
    /// PrintWindow with PW_RENDERFULLCONTENT (Windows).
    PrintWindow,
    /// BitBlt from the window's screen DC (Windows popups/fallback).
    ScreenDc,
    /// XGetImage over the X11 window (Linux).
    X11GetImage,
    /// `screencapture -l <windowid>` (macOS).
    MacScreenCapture,
    /// No capture backend available — honest failure.
    #[default]
    Unsupported,
}

/// The result of `see()` — a window image + provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeeResult {
    pub window_id: u64,
    /// PNG-encoded window pixels (physical resolution).
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub method: SeeMethod,
    /// Region this capture covers within the window (full window for `see()`,
    /// a sub-rect for region zoom).
    pub region: Region,
    /// Native scale factor of the capture path (1.0 on an unscaled display).
    ///
    /// This is the *platform's* factor, not a correction: the coordinates a
    /// caller supplies and the pixels in `png` are both physical, so nothing
    /// here rescales them. [`Self::dpi`] is the measured value with its
    /// provenance, and it is what says whether a raw-input rung has to convert.
    pub scale: f64,
    /// The measured DPI factor **with its source**, so a `1.0` is
    /// distinguishable from "not measured".
    pub dpi: DpiScale,
    /// The output budget the engine actually applied — including whether it
    /// clamped. `None` only for a raw platform capture that did not go through
    /// [`crate::geometry::enforce_output_budget`] (a direct backend call); an
    /// engine-produced capture always has one.
    pub budget: Option<SeeBudget>,
    /// `FIX-18` — the readiness verdict this capture was made **under**, verified
    /// before the capture was attempted. Non-optional on purpose: a capture with no
    /// readiness record is exactly the `FIX-18` defect, so the type does not allow
    /// one to be built silently.
    pub readiness: CaptureReadiness,
}

impl SeeResult {
    /// Map a point in **this returned image** back to a **window-relative**
    /// point, undoing any clamp. A model that predicts a coordinate on a
    /// downscaled capture is aiming at the same control as on a full-size one,
    /// so a click must not use the raw image coordinate.
    pub fn image_point_to_window(&self, x: i32, y: i32) -> (i32, i32) {
        match &self.budget {
            Some(b) => b.image_point_to_window(x, y),
            // No budget report: the image is the capture, so the two spaces
            // are the same. An honest identity, not a guess.
            None => (x, y),
        }
    }

    /// One honest sentence for an audit row: what was captured, what came back,
    /// and whether the budget changed it.
    pub fn describe(&self) -> String {
        let dpi = self.dpi.describe();
        match &self.budget {
            Some(b) => format!(
                "{} · {} · {} · {}",
                format!("{:?}", self.method),
                dpi,
                b.describe(),
                if b.disposition.clamped() {
                    "clamped"
                } else {
                    "unchanged"
                }
            ),
            None => format!("{:?} · {dpi} · no output budget applied", self.method),
        }
    }

    /// `FIX-18` — the capture's readiness/degrade record, for an audit row and a
    /// receipt. It names the pipeline that ran, the one that was skipped and why,
    /// and whether the vision rung asked for the capture at all.
    pub fn degrade(&self, vision_rung: bool) -> CaptureDegrade {
        CaptureDegrade {
            readiness: self.readiness.clone(),
            method: self.method,
            skipped: self.readiness.degraded_from,
            reason: self.readiness.fault.as_ref().map(|f| f.guidance()),
            vision_rung,
        }
    }
}

/// A rectangular region in window/physical coordinates.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub fn full(w: u32, h: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && py >= self.y
            && px < self.x + self.width as i32
            && py < self.y + self.height as i32
    }

    /// True if this region covers the entire given size (origin 0,0).
    pub fn is_full(&self, w: u32, h: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width == w && self.height == h
    }

    /// Clamp this region to a window of the given size (never overflows).
    pub fn clamp_to(&self, w: u32, h: u32) -> Region {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (x0 + self.width as i32).min(w as i32);
        let y1 = (y0 + self.height as i32).min(h as i32);
        Region {
            x: x0,
            y: y0,
            width: (x1 - x0).max(0) as u32,
            height: (y1 - y0).max(0) as u32,
        }
    }

    /// Clamp an inner region to this region (never overflows).
    pub fn intersect(&self, other: &Region) -> Option<Region> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.width as i32).min(other.x + other.width as i32);
        let y1 = (self.y + self.height as i32).min(other.y + other.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Region {
            x: x0,
            y: y0,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        })
    }

    /// Center point (the canonical click target).
    pub fn center(&self) -> (i32, i32) {
        (
            self.x + (self.width as i32) / 2,
            self.y + (self.height as i32) / 2,
        )
    }
}

/// One node of the a11y/UI-Automation tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadNode {
    /// Index path like `1.3.2` (ChatGPT `sky` style click-by-name/index).
    pub index_path: String,
    /// Control-type name: "Button", "Edit", "ListItem", "Text"…
    pub role: String,
    pub name: String,
    /// `AutomationId` / native id — a **hint, never a key**.
    ///
    /// `FIX-17`: MS documents `AutomationId` as optional, unique only among
    /// siblings, and *not stable across builds* (`ARCH/21` §3, DM-026; the v0
    /// comment here called it a "stable locator", which it is not). It is carried
    /// for diagnosis and may only narrow an already-unique candidate set — see
    /// [`crate::uia::resolve`]. Element identity is
    /// `(runtime_id | role+name+automationId+bounds)` scoped to one observation
    /// epoch; a tree is never cached as identity.
    pub automation_id: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Whether this node can be invoked (UIA InvokePattern) / has a value settable.
    pub actionable: bool,
    pub children: Vec<ReadNode>,
}

impl ReadNode {
    /// Depth-first flatten in document order (indexes precomputed).
    pub fn flatten(&self) -> Vec<&ReadNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.flatten());
        }
        out
    }

    pub fn find_by_name(&self, name: &str) -> Option<&ReadNode> {
        self.flatten().into_iter().find(|n| {
            n.name
                .to_ascii_lowercase()
                .contains(&name.to_ascii_lowercase())
        })
    }

    /// Click target for a named node (used by act-by-name).
    pub fn center(&self) -> (i32, i32) {
        Region {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
        .center()
    }
}

/// The result of `read()` — either an a11y tree or an honest absence.
///
/// `FIX-17`: the tree alone could not say *which* kind of absence it was. A
/// window with no accessibility tree (the honest `None` that starts the vision
/// rung) and a window this process is not allowed to read (elevation/UIAccess)
/// used to look identical. So the read now carries its **status**, its **epoch**
/// and its **guidance**: absence may only be inferred from a read that covered
/// the target (`REQ-CUA-003`, `REQ-CUA-006`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResult {
    pub window_id: u64,
    /// None when the platform/a11y surface is genuinely absent → vision-fallback
    /// path. Read `status` for whether that absence may be trusted.
    pub tree: Option<ReadNode>,
    /// The effective DPI scale for this window (for coordinate math).
    ///
    /// Measured per platform, not hardcoded: Windows per-monitor-v2
    /// (`GetDpiForWindow`), macOS backing scale, X11 `Xft.dpi`. A `1.0` is a
    /// real value on an unscaled display *and* the honest answer where the
    /// platform could not be asked — see [`crate::geometry::DpiSource`] to tell
    /// the two apart.
    pub dpi_scale: f64,
    /// The window list snapshot used (apps + windows).
    pub windows: Vec<WindowInfo>,
    /// The snapshot generation this observation belongs to. An element handle
    /// from any other generation is stale (`REQ-CUA-003`).
    pub epoch: SnapshotEpoch,
    /// When the observation was taken.
    pub observed_at_ms: u64,
    /// `complete` (fully walked) · `partial` (a bound, the budget, a lazy or
    /// elevated region) · `absent` (no structural UI — the vision rung) ·
    /// `unknown` (this process could not look; no absence may be inferred).
    pub status: UiaReadStatus,
    /// The actionable sentence when the read is not clean: elevation guidance,
    /// the re-read instruction, or the browser-rung (CDP) pointer.
    pub guidance: Option<String>,
    /// Freshness anomalies recorded during the read — a gap forces a bounded
    /// rescan and is never silent (`ARCH/21` §4).
    pub anomalies: Vec<FreshnessAnomaly>,
}

impl ReadResult {
    /// Build the neutral result for a platform with no structured UI (X11 bare,
    /// macOS without an AX layer): an `absent` read, which is a **positive** fact
    /// and therefore permits the vision-rung fallback.
    pub fn absent(window_id: u64, dpi_scale: f64, windows: Vec<WindowInfo>) -> Self {
        Self {
            window_id,
            tree: None,
            dpi_scale,
            windows,
            epoch: SnapshotEpoch(0),
            observed_at_ms: crate::now_ms(),
            status: UiaReadStatus::Absent {
                detail: "this platform exposes no accessibility tree on this build — the OCR / \
                         vision rung is the documented fallback"
                    .into(),
            },
            guidance: Some(
                "this platform exposes no accessibility tree on this build — use the OCR / vision \
                 rung (a capture of the window) for what is on screen"
                    .into(),
            ),
            anomalies: Vec::new(),
        }
    }

    /// May absence be inferred from this read? Only a read that covered the whole
    /// target may say "not present" (`REQ-CUA-006`).
    pub fn may_infer_absence(&self) -> bool {
        self.status.may_infer_absence()
    }

    /// The status's own sentence, for a receipt or a guidance card.
    pub fn status_detail(&self) -> String {
        match &self.status {
            UiaReadStatus::Complete => "the whole readable target was walked".into(),
            UiaReadStatus::Partial { detail }
            | UiaReadStatus::Absent { detail }
            | UiaReadStatus::Unknown { detail } => detail.clone(),
        }
    }

    /// The compact JSON shape the Tauri surface and the agent tool serve.
    ///
    /// Kept beside the type so the fields a caller reads cannot drift from the
    /// type, and mirrored (not re-implemented) by `desktop_cmds::snapshot_json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "detail": self.status_detail(),
            "mayInferAbsence": self.may_infer_absence(),
            "epoch": self.epoch.0,
            "observedAtMs": self.observed_at_ms,
            "hasTree": self.tree.is_some(),
            "guidance": self.guidance,
            "anomalies": self
                .anomalies
                .iter()
                .map(|a| serde_json::json!({
                    "kind": a.kind,
                    "scope": a.scope,
                    "detail": a.detail,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

/// A text word + its bounding box (OCR vision fallback).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrWord {
    pub text: String,
    /// Confidence 0..=100 (tesseract TSV `conf`).
    pub confidence: f64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl OcrWord {
    pub fn region(&self) -> Region {
        Region {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }

    pub fn center(&self) -> (i32, i32) {
        self.region().center()
    }
}

/// The action vocabulary — deliberately small and human-reviewable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActKind {
    Click {
        x: i32,
        y: i32,
    },
    /// UIA InvokePattern on a named element (Windows); xdotool-name on X11.
    ClickByName {
        name: String,
    },
    Type {
        text: String,
    },
    /// UIA ValuePattern::SetValue (Windows) / osascript set value (mac).
    SetValue {
        name: String,
        value: String,
    },
    Press {
        key: String,
    },
    Scroll {
        x: i32,
        y: i32,
        delta: i32,
    },
    Drag {
        from: (i32, i32),
        to: (i32, i32),
    },
    /// Launch a program (P57.1). `path` is the **canonical filesystem path** to
    /// the executable — Windows `.exe`, macOS `.app` bundle, Linux binary — and
    /// is what the allow-list matched and what the platforms execute. `app` is
    /// the name-only fallback the spec allows *after* path resolution (resolved
    /// through `PATH`), used only when no path is known.
    LaunchApp {
        path: Option<String>,
        app: String,
    },
    ActivateWindow {
        window_id: u64,
    },
}

impl ActKind {
    /// P57.1 — launch by canonical path (the preferred form).
    pub fn launch_path(path: impl Into<String>) -> Self {
        ActKind::LaunchApp {
            path: Some(path.into()),
            app: String::new(),
        }
    }

    /// P57.1 — launch by name; only valid when no path is known (the platform
    /// resolves it through `PATH`, and the policy treats the name as the
    /// subject).
    pub fn launch_by_name(app: impl Into<String>) -> Self {
        ActKind::LaunchApp {
            path: None,
            app: app.into(),
        }
    }

    /// P57.1/P57.2 — the program this act launches (the path when known, else
    /// the name). This is the Guard-2 **subject** for a launch: allow-listing an
    /// app has to gate the thing being launched, not whichever window happens
    /// to be focused.
    pub fn launch_target(&self) -> Option<&str> {
        match self {
            ActKind::LaunchApp { path, app } => Some(
                path.as_deref()
                    .filter(|p| !p.is_empty())
                    .unwrap_or(app.as_str()),
            ),
            _ => None,
        }
    }

    /// A human-readable one-liner for the Guard-2 card / audit line.
    pub fn describe(&self) -> String {
        match self {
            ActKind::Click { x, y } => format!("click at ({x},{y})"),
            ActKind::ClickByName { name } => format!("click \"{name}\""),
            ActKind::Type { text } => format!("type {} char(s)", text.chars().count()),
            ActKind::SetValue { name, .. } => format!("set value of \"{name}\""),
            ActKind::Press { key } => format!("press {key}"),
            ActKind::Scroll { x, y, delta } => format!("scroll at ({x},{y}) by {delta}"),
            ActKind::Drag { from, to } => format!("drag {from:?} → {to:?}"),
            ActKind::LaunchApp { .. } => {
                format!("launch {}", self.launch_target().unwrap_or("<nothing>"))
            }
            ActKind::ActivateWindow { window_id } => format!("activate window {window_id}"),
        }
    }
}

/// Outcome of one `act()` step (observe → one action → re-observe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActOutcome {
    pub kind: ActKind,
    pub ok: bool,
    /// Post-action re-observe (tree diff / OCR text) when a verifier ran.
    pub verification: Option<VerifyOutcome>,
    /// **Which rung of the click ladder actually ran, and what was tried before
    /// it.** `Some` for every coordinate/name click (and for the refusal when a
    /// rung needed authority that did not arrive), so a fall-through is visible
    /// in the result instead of having to be inferred from behaviour.
    pub click: Option<LadderVerdict>,
    pub error: Option<String>,
}

impl ActOutcome {
    /// A completed act.
    pub fn ok(kind: ActKind) -> Self {
        Self {
            kind,
            ok: true,
            verification: None,
            click: None,
            error: None,
        }
    }

    /// A refused/declined act, with the reason the caller must see.
    pub fn err(kind: ActKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            ok: false,
            verification: None,
            click: None,
            error: Some(reason.into()),
        }
    }

    /// A refused act that carries a click-ladder verdict.
    pub fn refused(kind: ActKind, reason: impl Into<String>, click: LadderVerdict) -> Self {
        Self {
            kind,
            ok: false,
            verification: None,
            click: Some(click),
            error: Some(reason.into()),
        }
    }

    /// The rung that delivered this act, when the act went through the ladder.
    pub fn rung(&self) -> Option<crate::ladder::ClickRung> {
        self.click.as_ref().and_then(|v| v.rung())
    }
}

/// Verify cascade outcome — halt-over-guess is the contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// The expected state was observed after the action.
    Confirmed,
    /// Retried and eventually satisfied.
    ConfirmedAfterRetry { attempts: u32 },
    /// Max retries exhausted without confirmation — we HALT, never guess.
    Halt { attempts: u32, reason: String },
}

/// What the platform reports it can actually do (honest capability surface).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capabilities {
    pub see: SeeMethod,
    pub see_occluded: bool,
    pub uia_tree: bool,
    pub invoke_set_value: bool,
    /// Global input synthesis (SendInput / XTEST / CGEvent). These **move the
    /// real pointer or keyboard focus**, which is why `background_input` is a
    /// separate fact rather than an implication of this one.
    pub send_input: bool,
    /// P57.3 — can a coordinate click be delivered to the target **without**
    /// moving the user's pointer? Windows: UIA `InvokePattern` at the hit-test
    /// point, else `PostMessage` to the target HWND. Linux/X11: a synthetic
    /// `ButtonPress`/`ButtonRelease` sent to the deepest child under the point
    /// (the server moves nothing; whether the app honours a synthetic event is
    /// the app's business — Tk/Gtk walk their own event queues). macOS: no —
    /// System Events clicks are real pointer events, so background coordinate
    /// clicks refuse and the caller escalates or uses a named AX click.
    pub background_input: bool,
    /// P57.4 — the platform can report and restore the foreground window, which
    /// is what makes an approved foreground escalation reversible instead of a
    /// focus steal.
    pub foreground_restore: bool,
    /// P57.7 — a real accessibility action (AT-SPI `Action.Invoke` on Linux /
    /// AX `AXPress` by point on macOS) exists *without* synthesising a pointer
    /// event. False means the named-element path is UI Automation (Windows) or
    /// unavailable, and label it honestly rather than implying parity.
    pub a11y_action: bool,
    /// P57.6 — occluded-window capture (Windows Graphics Capture). False until a
    /// backend actually implements it, so the UI never promises a screenshot the
    /// platform would return black for.
    pub see_occluded_wgc: bool,
    /// P57.5 — Windows Session 0: the process is in the services session, so
    /// there is no interactive desktop to see or drive. The engine refuses and
    /// says so instead of reporting an empty window list as success.
    pub interactive_desktop: bool,
    /// P57.5 — macOS Screen Recording consent (TCC). False = capture will return
    /// a desktop-picture placeholder, not the app.
    pub screen_recording_granted: bool,
    /// P57.5 — macOS Accessibility consent (TCC). False = System Events driving
    /// (click/keystroke) is refused by the OS.
    pub accessibility_granted: bool,
    pub ocr: bool,
    pub window_list: bool,
    pub launch_app: bool,
    /// `FIX-18` — the **host-scoped** capture-readiness summary, so the surface
    /// can say "graphics capture unavailable — PrintWindow fallback" *before* a
    /// capture is attempted. Per-target readiness is verified on each capture and
    /// travels on [`SeeResult::readiness`]; this field is the chip.
    pub capture_readiness: CaptureReadinessSummary,
}

/// The compact form of a [`CaptureReadiness`] carried on the capability surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CaptureReadinessSummary {
    /// `ready` · `degraded` · `unavailable`.
    pub state: String,
    /// The pipeline that will run when one can.
    pub pipeline: Option<String>,
    /// One actionable sentence — never empty, so no green dot over a dead path.
    pub detail: String,
}

impl From<CaptureReadiness> for CaptureReadinessSummary {
    fn from(readiness: CaptureReadiness) -> Self {
        Self::from_readiness(&readiness)
    }
}

impl CaptureReadinessSummary {
    /// Build the summary from a full readiness verdict.
    pub fn from_readiness(readiness: &CaptureReadiness) -> Self {
        Self {
            state: readiness.state.as_str().to_string(),
            pipeline: readiness.pipeline.map(|p| p.as_str().to_string()),
            detail: readiness.guidance.clone(),
        }
    }

    /// The state as a typed value, for a caller that wants the enum.
    pub fn state(&self) -> crate::capture::CaptureState {
        match self.state.as_str() {
            "ready" => crate::capture::CaptureState::Ready,
            "degraded" => crate::capture::CaptureState::Degraded,
            _ => crate::capture::CaptureState::Unavailable,
        }
    }
}

/// P57.4 — why a Background act cannot be delivered, and what escalating to
/// Foreground would cost. The engine never escalates silently: it returns this,
/// the UI renders the Guard-2 card, and only an explicit human gesture flips the
/// interaction mode for that one act (then the previous foreground is restored).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EscalationRequest {
    /// Plain-language reason shown on the card ("background input refused;\n    /// escalate to foreground?").
    pub reason: String,
    /// The act that could not be delivered under the current default.
    pub blocked_act: ActKind,
    /// Always true for P57.4: a foreground escalation is a real foreground
    /// change, so it needs a human gesture and cannot be auto-approved.
    pub requires_gesture: bool,
    /// What would be foregrounded (the target window/app).
    pub target: String,
}

/// P57.4 — the window that owned the foreground before an approved escalation,
/// so it can be given back afterwards. A missing id is honest: the platform
/// could not tell, and restore is then a no-op rather than a guess.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ForegroundSnapshot {
    pub window_id: Option<u64>,
    pub captured_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_center_and_containment() {
        let r = Region {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert_eq!(r.center(), (60, 45));
        assert!(r.contains(10, 20));
        assert!(!r.contains(110, 20));
    }

    #[test]
    fn region_intersect_clamps() {
        let a = Region::full(100, 100);
        let b = Region {
            x: 50,
            y: 50,
            width: 200,
            height: 200,
        };
        let i = a.intersect(&b).unwrap();
        assert_eq!((i.x, i.y, i.width, i.height), (50, 50, 50, 50));
        // Disjoint → None
        let c = Region {
            x: 500,
            y: 500,
            width: 10,
            height: 10,
        };
        assert!(a.intersect(&c).is_none());
    }

    #[test]
    fn read_node_flatten_and_find() {
        let mut child = ReadNode {
            index_path: "1.1".into(),
            role: "Button".into(),
            name: "Save".into(),
            automation_id: None,
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            actionable: true,
            children: vec![],
        };
        child.x = 100;
        child.y = 200;
        let root = ReadNode {
            index_path: "1".into(),
            role: "Pane".into(),
            name: "window".into(),
            automation_id: None,
            x: 0,
            y: 0,
            width: 300,
            height: 300,
            actionable: false,
            children: vec![child],
        };
        assert_eq!(root.flatten().len(), 2);
        let save = root.find_by_name("save").expect("find by name");
        assert_eq!(save.center(), (105, 205));
    }

    #[test]
    fn act_kind_describes_human_readably() {
        let d = ActKind::Type {
            text: "hello".into(),
        }
        .describe();
        assert!(d.contains("type 5 char(s)"), "{d}");
        let d2 = ActKind::Click { x: 1, y: 2 }.describe();
        assert!(d2.contains("click at (1,2)"), "{d2}");
    }

    fn see_result_with_budget(budget: SeeBudget) -> SeeResult {
        SeeResult {
            window_id: 7,
            png: vec![1, 2, 3],
            width: budget.output_width,
            height: budget.output_height,
            method: SeeMethod::PrintWindow,
            region: Region::full(budget.output_width, budget.output_height),
            scale: 1.0,
            dpi: DpiScale::from_dpi(120, crate::geometry::DpiSource::PerMonitorV2),
            budget: Some(budget),
            // `FIX-18` — a capture cannot be built without a readiness verdict.
            readiness: crate::capture::CaptureReadiness::ready(
                crate::capture::CapturePipeline::PrintWindow,
                vec![crate::capture::CaptureCheck::PipelineSupported],
            ),
        }
    }

    fn sample_budget(scale: f64) -> SeeBudget {
        SeeBudget {
            captured_width: 2400,
            captured_height: 1200,
            output_width: 1232,
            output_height: 616,
            output_scale_x: scale,
            output_scale_y: scale,
            disposition: crate::geometry::BudgetDisposition::Clamped {
                dimension: true,
                aligned: true,
            },
            alignment: 28,
            max_dimension_px: 1280,
            max_bytes: 900 * 1024,
            bytes: 1234,
        }
    }

    /// A clamped capture must not hand a raw image coordinate to a click: the
    /// mapping back to window space is the honest inverse of the clamp.
    #[test]
    fn a_clamped_capture_maps_image_points_back_to_window_space() {
        let see = see_result_with_budget(sample_budget(1232.0 / 2400.0));
        // The centre of the returned image is the centre of the capture.
        let (wx, wy) = see.image_point_to_window(616, 308);
        assert!((wx - 1200).abs() <= 1, "got {wx}");
        assert!((wy - 600).abs() <= 1, "got {wy}");
        // And the description says it was clamped, with the measured DPI.
        let d = see.describe();
        assert!(d.contains("clamped"), "{d}");
        assert!(d.contains("per_monitor_v2 factor 1.25"), "{d}");
    }

    /// A raw backend capture with no budget report is an honest identity, not a
    /// silent lie: the description says the budget was never applied.
    #[test]
    fn an_unbudgeted_capture_is_labelled_not_assumed_fine() {
        let mut see = see_result_with_budget(sample_budget(1.0));
        see.budget = None;
        assert_eq!(see.image_point_to_window(5, 6), (5, 6));
        assert!(see.describe().contains("no output budget applied"));
    }

    #[test]
    fn act_outcome_carries_the_rung_that_ran() {
        use crate::ladder::ClickRung;
        let ok = ActOutcome::ok(ActKind::Click { x: 1, y: 1 });
        assert_eq!(ok.rung(), None);
        assert!(ok.click.is_none());

        let refused = ActOutcome::refused(
            ActKind::Click { x: 1, y: 1 },
            "gate decision: deny",
            LadderVerdict::NeedsAuthorization {
                rung: ClickRung::RawInput,
                reason: "gate decision: deny".into(),
                attempts: vec![],
            },
        );
        assert!(!refused.ok);
        // Nothing ran, so no rung is reported as having delivered it.
        assert_eq!(refused.rung(), None);
        assert!(refused.error.unwrap().contains("deny"));
    }
}

/// The wire-shape contract between this crate and the Tauri surface.
///
/// `FIX-17` / `FIX-18` added fields to [`ReadResult`], [`SeeResult`] and
/// [`Capabilities`], and the host reads them in `src-tauri/src/desktop_cmds.rs`.
/// Those reads are pinned here — with the same field names and the same method
/// calls — so a rename here cannot silently blank the UI's status chip or the
/// agent's snapshot. The host cannot be compiled in every CI lane (it needs the
/// Tauri toolchain), so the contract is asserted on this side of the boundary.
#[cfg(test)]
mod host_contract_tests {
    use super::*;

    fn window() -> WindowInfo {
        WindowInfo {
            id: 11,
            title: "Editor".into(),
            app: "notepad".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 80,
            has_a11y_tree: true,
        }
    }

    /// What `desktop_cmds::desktop_read` and `::snapshot_json` read off a read.
    #[test]
    fn a_read_exposes_the_fields_the_host_surface_reads() {
        let read = ReadResult::absent(11, 1.0, vec![window()]);
        assert_eq!(read.window_id, 11);
        assert!(read.tree.is_none());
        assert_eq!(read.dpi_scale, 1.0);
        assert_eq!(read.windows.len(), 1);
        assert_eq!(read.status.as_str(), "absent");
        assert!(read.may_infer_absence());
        // Epoch 0 on a platform with no structured read: there is no generation to
        // stamp, and saying so is honest (a structured read always stamps > 0).
        assert_eq!(read.epoch, SnapshotEpoch(0));
        assert!(read.observed_at_ms > 0);
        assert!(!read.status_detail().is_empty());
        assert!(read.guidance.is_some());
        assert!(read.anomalies.is_empty());

        // The exact JSON keys the host serializes.
        let json = read.to_json();
        for key in [
            "status",
            "detail",
            "mayInferAbsence",
            "epoch",
            "observedAtMs",
            "hasTree",
            "guidance",
            "anomalies",
        ] {
            assert!(json.get(key).is_some(), "missing host key {key}");
        }

        // And a read that could not look refuses absence.
        let blocked = ReadResult {
            status: UiaReadStatus::Unknown {
                detail: "elevated".into(),
            },
            tree: None,
            guidance: Some("no input is synthesized into it".into()),
            epoch: SnapshotEpoch(4),
            anomalies: vec![FreshnessAnomaly {
                kind: "elevation_restricted".into(),
                scope: "window:11".into(),
                detail: "elevated".into(),
                observed_at_ms: 1,
            }],
            ..read
        };
        assert!(!blocked.may_infer_absence());
        let json = blocked.to_json();
        assert_eq!(json["status"], "unknown");
        assert_eq!(json["mayInferAbsence"], false);
        assert_eq!(json["epoch"], 4);
        assert_eq!(json["anomalies"][0]["kind"], "elevation_restricted");
    }

    /// What `desktop_cmds::desktop_see` reads off a capture, and what
    /// `desktop_status` reads off the capability surface.
    #[test]
    fn a_capture_and_the_capability_surface_expose_what_the_host_reads() {
        let see = SeeResult {
            window_id: 11,
            png: vec![0x89, b'P', b'N', b'G'],
            width: 800,
            height: 600,
            method: SeeMethod::PrintWindow,
            region: Region::full(800, 600),
            scale: 1.0,
            dpi: DpiScale::unknown(),
            budget: None,
            readiness: crate::capture::CaptureReadiness::ready(
                crate::capture::CapturePipeline::PrintWindow,
                vec![crate::capture::CaptureCheck::PipelineSupported],
            ),
        };
        // `desktop_see` serializes: method, readiness, degraded, budget, describe.
        assert_eq!(format!("{:?}", see.method), "PrintWindow");
        assert!(!see.readiness.is_degraded());
        let json = see.readiness.to_json();
        assert_eq!(json["state"], "ready");
        assert_eq!(json["pipeline"], "print_window");
        assert!(!see.describe().is_empty());
        // The degrade record the vision-rung discipline rides.
        let degrade = see.degrade(true);
        assert!(!degrade.is_degraded());
        assert!(!degrade.describe().is_empty());

        // `desktop_status` serializes the capture chip.
        let caps = Capabilities {
            capture_readiness: see.readiness.clone().into(),
            ..Capabilities::default()
        };
        assert_eq!(caps.capture_readiness.state, "ready");
        assert_eq!(
            caps.capture_readiness.pipeline.as_deref(),
            Some("print_window")
        );
        assert!(!caps.capture_readiness.detail.is_empty());
        assert_eq!(
            caps.capture_readiness.state(),
            crate::capture::CaptureState::Ready
        );
        let mut degraded = see.readiness.clone();
        degraded.state = crate::capture::CaptureState::Degraded;
        let summary: CaptureReadinessSummary = degraded.into();
        assert_eq!(summary.state(), crate::capture::CaptureState::Degraded);
    }
}
