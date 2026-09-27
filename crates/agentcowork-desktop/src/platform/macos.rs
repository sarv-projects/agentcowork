//! macOS backend (E9 twin) — zero-dependency subprocess surface, the same
//! shape as ChatGPT Mac Computer Use:
//!
//! - **See:** `screencapture -l <windowid> -x` (Screen Recording permission).
//! - **Read:** `osascript` System Events for the app + window list; deep AX
//!   tree traversal is a follow-on (this build reads windows + OCR).
//! - **Act:** System Events `click at`, `keystroke`, `key code`, scroll via
//!   `scroll` action (Accessibility permission).
//!   **P57.3:** `activate` (System Events / `tell application … to activate`)
//!   happens only on the foreground path; under the Background default the
//!   script targets the process by name without activating it, and a launch
//!   uses `open -g` so the app does not come to the front.
//!   **P57.4:** System Events has no message-level or `AXPress`-by-point
//!   primitive, so a coordinate click / scroll / drag *is* real pointer motion
//!   here. Under the Background default those refuse with an actionable
//!   sentence (the named-element click is the background path that works),
//!   instead of moving the user's cursor behind their back.
//!   **P57.1:** launch opens the canonical path (`.app` bundle or binary).
//!
//! Compiles on every target; live use requires macOS + the two TCC
//! permissions, surfaced honestly through `capabilities()`.

use std::process::Command;

// `DynamicImage::dimensions()` is a `GenericImageView` method, not inherent to
// the enum — without this import `see()` does not compile on macOS.
use image::GenericImageView;

use crate::DesktopError;
use crate::capture::{CaptureCheck, CaptureFault, CapturePipeline, CaptureReadiness};
use crate::geometry::DpiScale;
use crate::ladder::{ClickProfile, ClickRung, LadderTarget, RungDelivery};
use crate::launch;
use crate::policy::InteractionMode;
use crate::types::{ActKind, ReadResult, Region, SeeMethod, SeeResult, WindowInfo};

/// The macOS click ladder, as an honest statement of what System Events can
/// actually do.
///
/// Two of the three rungs do not exist here, and saying so is the point:
///
/// - **No message-level primitive exists on macOS at all.** There is no
///   `PostMessage` equivalent: an app receives real events from the window
///   server. A coordinate click therefore has no "address the event to the
///   target window" rung.
/// - **No accessibility invoke *by point*.** `AXPress` needs an
///   ApplicationServices FFI layer that is not in this build's dependency set,
///   which is exactly what `Capabilities::a11y_action == false` reports. A
///   *named* AX click (`click "Save" of window 1`) does work through System
///   Events and is used by [`MacBackend::act`] for `ClickByName` — but the
///   coordinate ladder, which is what a model drives from a screenshot, has no
///   by-point form.
///
/// So the coordinate ladder on macOS is a **single, gated** rung, and it is the
/// pointer-moving one. Under the Background default it therefore always stops:
/// there is no non-moving coordinate click to fall back to, which is precisely
/// the honest version of the §4 background contract.
pub fn mac_click_profile() -> ClickProfile {
    ClickProfile::new(
        "macos",
        vec![ClickRung::RawInput],
        vec![
            "no message-level primitive on macOS: an app only receives window-server events, \
             so there is no synthetic-event rung to fall back to"
                .into(),
            "no accessibility invoke by point: AXPress needs an ApplicationServices FFI layer \
             that is not in this build (Capabilities::a11y_action is false). A *named* AX click \
             works through System Events, but a coordinate click from a screenshot has no \
             by-point rung"
                .into(),
        ],
    )
    .expect("the macOS click profile is a fixed, ordered literal")
}

pub struct MacBackend;

impl MacBackend {
    /// Apps with visible windows (System Events).
    pub fn list_apps() -> Vec<String> {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(
                "tell application \"System Events\" to get name of every process whose background only is false",
            )
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        out.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn list_windows() -> Result<Vec<WindowInfo>, DesktopError> {
        let mut out = Vec::new();
        let mut id: u64 = 1000;
        for app in Self::list_apps() {
            // Window names for this app (empty → the app has a main window
            // without a title — still list it with an empty title).
            let script = format!(
                "tell application \"System Events\" to tell process \"{}\" to get name of every window",
                app
            );
            let windows = Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            let names: Vec<String> = if windows.trim().is_empty() {
                vec![String::new()]
            } else {
                windows
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            for title in names {
                id += 1;
                out.push(WindowInfo {
                    id,
                    title,
                    app: app.clone(),
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0, // bounds require a second AX call — follow-on
                    has_a11y_tree: false,
                });
            }
        }
        Ok(out)
    }

    pub fn see(window: &WindowInfo) -> Result<SeeResult, DesktopError> {
        // `FIX-18` — verify the capture path *before* attempting it. On macOS the
        // gate is TCC Screen Recording: without it `screencapture` writes a
        // desktop-picture placeholder and exits 0, so a readiness check is the
        // only thing that distinguishes a real capture from a picture of the
        // desktop. The verdict then travels with the capture.
        let readiness = if Self::screen_recording_granted() {
            CaptureReadiness::ready(
                CapturePipeline::MacScreenCapture,
                vec![
                    CaptureCheck::PipelineSupported,
                    CaptureCheck::SessionAvailable,
                    CaptureCheck::WindowHandleValid,
                ],
            )
        } else {
            CaptureReadiness::unavailable(CaptureFault::ConsentMissing {
                detail: "Screen Recording consent is not granted, so screencapture would return a \
                         desktop-picture placeholder rather than the app"
                    .into(),
            })
        };
        if !readiness.is_capturable() {
            return Err(DesktopError::CaptureNotReady {
                state: readiness.state.as_str(),
                guidance: readiness.guidance.clone(),
                failed_check: readiness
                    .failed_check
                    .map(|c| c.as_str())
                    .unwrap_or("session_available"),
            });
        }
        // `screencapture -l <id>` needs a CGWindowID; we carry our own id
        // space, so the caller must pass a real CGWindowID in window.id.
        let tmp = std::env::temp_dir().join(format!("agentcowork-see-{}.png", window.id));
        let status = Command::new("screencapture")
            .args([
                "-l",
                &window.id.to_string(),
                "-x",
                tmp.to_str().unwrap_or("/tmp/e9.png"),
            ])
            .status()
            .map_err(|e| DesktopError::Platform(format!("screencapture: {e}")))?;
        if !status.success() {
            return Err(DesktopError::Platform(
                "screencapture failed — Screen Recording permission?".into(),
            ));
        }
        let bytes = std::fs::read(&tmp)
            .map_err(|e| DesktopError::Platform(format!("read capture: {e}")))?;
        let _ = std::fs::remove_file(&tmp);
        let img = image::load_from_memory(&bytes)
            .map_err(|e| DesktopError::Platform(format!("decode capture: {e}")))?;
        let (width, height) = img.dimensions();
        let mut png = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| DesktopError::Platform(format!("png encode: {e}")))?;
        Ok(SeeResult {
            window_id: window.id,
            png,
            width,
            height,
            method: SeeMethod::MacScreenCapture,
            region: Region::full(width, height),
            scale: 1.0,
            dpi: Self::dpi_scale(),
            // The engine (`DesktopEngine::see`) applies the output budget; a
            // direct backend call has had none applied, and says so via `None`.
            budget: None,
            readiness,
        })
    }

    /// The main display's backing scale, with its provenance.
    ///
    /// `screencapture` returns **pixels**; System Events `click at` takes
    /// **points**. On a Retina display those differ by the backing scale, which
    /// is the concrete reason a click sent in pixel coordinates lands in the
    /// wrong place there — the platform's "scale factor is 1.0" assumption is
    /// what produced it.
    ///
    /// The value is the main display's, because that is what the OS exposes
    /// without a per-window window-server query; it is reported as
    /// [`DpiSource::BackingStore`] so a caller can see it is a display-level
    /// fact. When it cannot be read, the honest [`DpiScale::unknown`] is
    /// returned and no coordinate is converted.
    #[cfg(target_os = "macos")]
    pub fn dpi_scale() -> DpiScale {
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGMainDisplayPixelsWide() -> usize;
            fn CGMainDisplayPointsWide() -> usize;
        }
        // SAFETY: two side-effect-free CoreGraphics display-geometry queries.
        let (px, pt) = unsafe { (CGMainDisplayPixelsWide(), CGMainDisplayPointsWide()) };
        if pt == 0 {
            return DpiScale::unknown();
        }
        DpiScale::from_factor(px as f64 / pt as f64, DpiSource::BackingStore)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn dpi_scale() -> DpiScale {
        DpiScale::unknown()
    }

    /// Attempt one rung of the click ladder.
    ///
    /// Only [`ClickRung::RawInput`] exists on this platform (see
    /// [`mac_click_profile`]); the other two return `Unavailable` with the
    /// reason, so the verdict trail is self-explanatory rather than a bare
    /// "unsupported".
    ///
    /// The coordinate is converted from the **pixel** space the caller uses
    /// (the same space as the screenshot) into the **point** space System
    /// Events expects, using the measured backing scale — and only when that
    /// scale is actually measured. An unmeasured scale converts nothing, so a
    /// host that cannot be asked behaves exactly as it did before rather than
    /// guessing a factor.
    pub fn deliver_rung(
        rung: ClickRung,
        target: &LadderTarget,
        _mode: InteractionMode,
    ) -> RungDelivery {
        let (x, y) = match &target.act {
            ActKind::Click { x, y } => (*x, *y),
            other => {
                return RungDelivery::Unavailable(format!(
                    "rung {} does not apply to {}",
                    rung.as_str(),
                    other.describe()
                ));
            }
        };
        let dpi = Self::dpi_scale();
        match rung {
            ClickRung::RawInput => {
                // `click at` takes points; the incoming coordinate is pixels.
                let converts = dpi.applies_to_click_coordinates() && !dpi.is_identity();
                let (tx, ty) = if converts {
                    (
                        dpi.to_logical(f64::from(x)) as i32,
                        dpi.to_logical(f64::from(y)) as i32,
                    )
                } else {
                    (x, y)
                };
                let script =
                    format!("tell application \"System Events\" to click at {{{tx}, {ty}}}");
                let status = Command::new("osascript")
                    .arg("-e")
                    .arg(&script)
                    .status()
                    .map_err(|e| RungDelivery::Failed(format!("osascript: {e}")));
                match status {
                    Ok(s) if s.success() => {
                        let conversion = if converts {
                            format!(
                                " (pixel ({x},{y}) → point ({tx},{ty}) via {})",
                                dpi.describe()
                            )
                        } else {
                            String::new()
                        };
                        RungDelivery::Delivered(format!(
                            "System Events click at ({tx},{ty}){conversion}"
                        ))
                    }
                    Ok(_) => RungDelivery::Failed(
                        "osascript failed — Accessibility permission? (System Settings → Privacy \
                         & Security → Accessibility)"
                            .into(),
                    ),
                    Err(e) => RungDelivery::Failed(format!("osascript: {e:?}")),
                }
            }
            ClickRung::SyntheticEvent => RungDelivery::Unavailable(
                "macOS has no message-level primitive: an app only receives window-server \
                 events, so there is no synthetic-event rung"
                    .into(),
            ),
            ClickRung::AccessibilityInvoke => RungDelivery::Unavailable(
                "no accessibility invoke by point on this build: AXPress needs an \
                 ApplicationServices FFI layer that is not linked (a named AX click works)"
                    .into(),
            ),
        }
    }

    pub fn read(_window: &WindowInfo) -> Result<ReadResult, DesktopError> {
        // `FIX-17` — an `Absent` read: this build exposes no AX traversal layer,
        // which is a *positive* fact about the platform (so the OCR/vision rung is
        // the documented next step) rather than an `Unknown` that would claim
        // permission was refused.
        Ok(ReadResult::absent(
            _window.id,
            Self::dpi_scale().factor,
            Self::list_windows()?,
        ))
    }

    pub fn act(
        window: &WindowInfo,
        act: &ActKind,
        mode: InteractionMode,
    ) -> Result<(), DesktopError> {
        // P57.1 — a launch is `open [-g] <path>`: `-g` is macOS's documented
        // "do not bring the application to the foreground", i.e. exactly the
        // background contract's no-activate rule. A `.app` bundle and a plain
        // binary are both accepted by `open`; a bare name stays a name-only
        // fallback (`open -a`), which is what the spec allows after path
        // resolution fails.
        if let ActKind::LaunchApp { path, app } = act {
            let mut cmd = Command::new("open");
            if mode == InteractionMode::Background {
                cmd.arg("-g");
            }
            match path.as_deref() {
                Some(p) if !p.is_empty() => {
                    let target = launch::resolve_target(Some(p), "", &[])?;
                    cmd.arg(target);
                }
                _ => {
                    if app.trim().is_empty() {
                        return Err(DesktopError::Platform(
                            "launch needs a path or an app name".into(),
                        ));
                    }
                    cmd.arg("-a").arg(app);
                }
            }
            let status = cmd
                .status()
                .map_err(|e| DesktopError::Platform(format!("open: {e}")))?;
            if !status.success() {
                return Err(DesktopError::Platform(
                    "open failed — is the bundle path correct?".into(),
                ));
            }
            return Ok(());
        }
        // Coordinate space: System Events uses screen points; the window id we
        // carry is our own — activate by app name + click by point.
        let app = &window.app;
        // P57.4 — a point-addressed System Events action is a genuine pointer
        // event: it moves the cursor and clicks wherever the cursor lands. There
        // is no macOS equivalent of a UIA invoke-by-point or a message click, so
        // the Background contract refuses these rather than breaking its promise.
        // (`click "name" of window 1` below is AXPress — that one is fine, and
        // is the background path for a named element.)
        if mode == InteractionMode::Background {
            let pointer_moving = matches!(
                act,
                ActKind::Click { .. } | ActKind::Scroll { .. } | ActKind::Drag { .. }
            );
            if pointer_moving {
                return Err(DesktopError::Unsupported(
                    "background contract: System Events clicks/scrolls/drags move the real \
                     pointer and macOS exposes no non-moving equivalent — use a named-element \
                     click, or switch the interaction default to Foreground (Settings → \
                     Computer use)"
                        .into(),
                ));
            }
        }
        let script = match act {
            ActKind::Click { x, y } => {
                format!("tell application \"System Events\" to click at {{{x}, {y}}}")
            }
            ActKind::ClickByName { name } => format!(
                "tell application \"System Events\" to tell process \"{app}\" to click \"{name}\" of window 1"
            ),
            ActKind::Type { text } => {
                let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
                format!(
                    "tell application \"System Events\" to tell process \"{app}\" to keystroke \"{escaped}\""
                )
            }
            ActKind::Press { key } => {
                let code = match key.to_ascii_lowercase().as_str() {
                    "enter" | "return" => 36,
                    "tab" => 48,
                    "escape" | "esc" => 53,
                    "space" => 49,
                    "delete" => 51,
                    "left" => 123,
                    "up" => 126,
                    "right" => 124,
                    "down" => 125,
                    "home" => 115,
                    "end" => 119,
                    "pageup" => 116,
                    "pagedown" => 121,
                    _ => return Err(DesktopError::Platform(format!("unknown key {key}"))),
                };
                format!("tell application \"System Events\" to key code {code}")
            }
            ActKind::Scroll { x, y, delta } => format!(
                "tell application \"System Events\" to tell process \"{app}\" to scroll {delta} at {{{x}, {y}}}"
            ),
            ActKind::SetValue { name, value } => {
                let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
                format!(
                    "tell application \"System Events\" to tell process \"{app}\" to set value of \"{name}\" of window 1 to \"{escaped}\""
                )
            }
            ActKind::Drag { from, to } => format!(
                "tell application \"System Events\" to drag from {{{}, {}}} to {{{}, {}}}",
                from.0, from.1, to.0, to.1
            ),
            ActKind::ActivateWindow { .. } => {
                // P57.3 — `activate` brings the app to the front, i.e. the
                // foreground escalation. Under Background the act refuses
                // rather than pretending (the engine refuses earlier too).
                if mode == InteractionMode::Background {
                    return Err(DesktopError::Unsupported(
                        "background contract: macOS `activate` is a foreground escalation — \
                         switch the interaction default to Foreground"
                            .into(),
                    ));
                }
                format!("tell application \"{app}\" to activate")
            }
            // Handled above (before the System Events script paths).
            ActKind::LaunchApp { .. } => {
                return Err(DesktopError::Platform(
                    "launch was not handled on the pre-script path".into(),
                ));
            }
        };
        let status = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status()
            .map_err(|e| DesktopError::Platform(format!("osascript: {e}")))?;
        if !status.success() {
            return Err(DesktopError::Platform(
                "osascript failed — Accessibility permission? (System Settings → Privacy & Security \
                 → Accessibility)"
                    .into(),
            ));
        }
        Ok(())
    }

    /// P57.4 — this backend lists windows with **synthetic** ids (it walks the
    /// app list), so a real foreground window cannot be named back. Returning
    /// `None` is the honest answer: the engine then skips restore rather than
    /// restoring the wrong thing.
    pub fn foreground_window() -> Option<u64> {
        None
    }

    /// P57.4 — restore is unsupported here for the same reason; the engine
    /// surfaces this honestly instead of pretending the foreground was handed
    /// back.
    pub fn restore_foreground(_window_id: u64) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported(
            "macOS window ids are synthetic — the previous foreground cannot be restored by id"
                .into(),
        ))
    }

    /// P57.5 — Screen Recording consent (TCC). macOS returns a desktop-picture
    /// placeholder for an ungranted capture, so asking the OS is the only honest
    /// answer. On non-macOS hosts this probe is not reachable.
    #[cfg(target_os = "macos")]
    pub fn screen_recording_granted() -> bool {
        unsafe extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        // SAFETY: a side-effect-free CoreGraphics query.
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn screen_recording_granted() -> bool {
        false
    }

    /// P57.5 — Accessibility consent (TCC). A System Events read is refused with
    /// error -1743 when not granted, so the probe doubles as the check.
    pub fn accessibility_granted() -> bool {
        Command::new("osascript")
            .arg("-e")
            .arg("tell application \"System Events\" to get name of first process")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// The macOS capture-readiness probe (`FIX-18`).
///
/// One pipeline, one real gate: Screen Recording (TCC). Without it `screencapture`
/// exits 0 and writes a desktop-picture placeholder, so the readiness check is the
/// only thing that separates a real capture from a picture of the desktop — the
/// `FIX-18` case in its purest form.
pub struct MacCaptureProbe;

impl crate::capture::CaptureProbe for MacCaptureProbe {
    fn pipelines(&self) -> Vec<CapturePipeline> {
        vec![CapturePipeline::MacScreenCapture]
    }

    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault> {
        if !MacBackend::screen_recording_granted() {
            return Err(CaptureFault::ConsentMissing {
                detail: "Screen Recording consent is not granted for this process".into(),
            });
        }
        Ok(vec![
            CaptureCheck::PipelineSupported,
            CaptureCheck::SessionAvailable,
        ])
    }

    /// `screencapture -l` is per-window, so an occluded window is still captured
    /// correctly; the check exists for platforms whose only pipeline is the screen.
    fn is_occluded(&mut self, _window: &WindowInfo) -> bool {
        false
    }

    fn target(
        &mut self,
        window: &WindowInfo,
        _pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault> {
        if window.id == 0 {
            return Err(CaptureFault::InvalidWindowHandle {
                detail: "this backend assigns its own window ids; a capture needs a real \
                         CGWindowID in window.id"
                    .into(),
            });
        }
        Ok(vec![CaptureCheck::WindowHandleValid])
    }
}
