//! Platform backends for E9.
//!
//! One union contract (`see`/`read`/`act`), three backends:
//! - [`linux`] — X11 (EWMH + XGetImage + XTEST), live-tested under Xvfb.
//! - [`win`] — UIA tree + Invoke/SetValue + SendInput + PrintWindow/DC,
//!   cross-compile-checked for x86_64-pc-windows-msvc.
//! - [`macos`] — `screencapture` + `osascript` System Events (zero deps).

#[cfg(target_os = "linux")]
pub mod linux;
// The macOS backend is dependency-free `std::process` plus this crate's own
// types, so it also compiles in test builds on every host. That is deliberate:
// a darwin-only `cfg` means a typo there is invisible until someone builds on a
// Mac (this module shipped two real compile errors exactly that way — a missing
// `GenericImageView` import and a `&str` handed to a `String` variant). `cargo
// test` now proves all three backends still compile.
#[cfg(any(target_os = "macos", test))]
pub mod macos;
#[cfg(windows)]
pub mod win;
// P57.6 — Windows.Graphics.Capture (occluded-window capture). Windows-only and
// runtime-gated: it cannot be exercised on this host, so it is kept in its own
// module with a real `available()` probe rather than a compile-time promise.
#[cfg(windows)]
pub mod wgc;

use crate::DesktopError;
use crate::capture::{CaptureProbe, CaptureReadiness};
use crate::geometry::DpiScale;
use crate::ladder::{ClickProfile, ClickRung, LadderTarget, RungDelivery};
use crate::policy::InteractionMode;
use crate::types::{
    ActKind, Capabilities, CaptureReadinessSummary, ReadResult, Region, SeeMethod, SeeResult,
    WindowInfo,
};

/// The click ladder each platform really has, declared as data.
///
/// The three profiles are deliberately different, and the difference is the
/// honest part: macOS has no message-level primitive and no accessibility
/// invoke by point in this dependency set, and X11 has no accessibility client
/// at all. A platform that cannot do a rung says so instead of the ladder
/// silently starting lower. Declared once, per backend, so the capability
/// surface, the audit row and the engine's walk cannot disagree.
pub fn linux_click_profile() -> ClickProfile {
    ClickProfile::new(
        "linux",
        vec![ClickRung::SyntheticEvent, ClickRung::RawInput],
        vec![
            "no accessibility invoke on bare X11: no AT-SPI client is linked, so the ladder \
             starts at the synthetic-event rung and the a11y tree is empty by design"
                .into(),
        ],
    )
    .expect("the linux click profile is a fixed, ordered literal")
}

/// A live desktop backend for the current platform.
/// `X11` carries a live connection (large variant) — boxed is overkill since
/// the enum is never stored in arrays or copied.
#[allow(clippy::large_enum_variant)]
pub enum PlatformBackend {
    #[cfg(target_os = "linux")]
    X11(crate::platform::linux::X11Backend),
    #[cfg(windows)]
    Win,
    #[cfg(target_os = "macos")]
    Mac,
    Unsupported,
}

impl PlatformBackend {
    /// Try to construct the current platform's backend.
    pub fn current() -> Result<Self, DesktopError> {
        #[cfg(target_os = "linux")]
        {
            if crate::platform::linux::X11Backend::is_available() {
                Ok(PlatformBackend::X11(
                    crate::platform::linux::X11Backend::connect()?,
                ))
            } else {
                Err(DesktopError::Unsupported(
                    "no X11 DISPLAY available — E9 needs a desktop session".into(),
                ))
            }
        }
        #[cfg(windows)]
        {
            Ok(PlatformBackend::Win)
        }
        #[cfg(target_os = "macos")]
        {
            Ok(PlatformBackend::Mac)
        }
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            Err(DesktopError::Unsupported(
                "E9 is supported on Linux (X11), Windows and macOS".into(),
            ))
        }
    }

    pub fn list_windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.list_windows(),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::WinBackend::list_windows(),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::MacBackend::list_windows(),
            PlatformBackend::Unsupported => Err(DesktopError::Unsupported("no backend".into())),
        }
    }

    pub fn read(&self, window: &WindowInfo) -> Result<ReadResult, DesktopError> {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.read(window),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::WinUia::init()?.read(window),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::MacBackend::read(window),
            PlatformBackend::Unsupported => Err(DesktopError::Unsupported("no backend".into())),
        }
    }

    pub fn see(&self, window: &WindowInfo, region: Region) -> Result<SeeResult, DesktopError> {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.see(window, region),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::WinBackend::see(window, region),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::MacBackend::see(window),
            PlatformBackend::Unsupported => Err(DesktopError::Unsupported("no backend".into())),
        }
    }

    /// P57.3 — `mode` is the policy's interaction default. Every backend must
    /// honour it: under `Background` nothing may be raised, focused or activated
    /// (`SetForegroundWindow`, EWMH stacking + `set_input_focus`, or macOS
    /// `activate`), and a launch must not steal focus
    /// (`SW_SHOWNOACTIVATE` / `open -g`).
    pub fn act(
        &self,
        window: &WindowInfo,
        act: &ActKind,
        mode: InteractionMode,
    ) -> Result<(), DesktopError> {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.act(window, act, mode),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::act(window, act, None, mode),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::MacBackend::act(window, act, mode),
            PlatformBackend::Unsupported => Err(DesktopError::Unsupported("no backend".into())),
        }
    }

    /// P57.4 — the window that currently owns the foreground. `None` means the
    /// platform cannot say (or its ids are not restorable); the caller then
    /// skips restore rather than guessing.
    pub fn foreground_window(&self) -> Option<u64> {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.foreground_window(),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::WinBackend::foreground_window(),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::MacBackend::foreground_window(),
            PlatformBackend::Unsupported => None,
        }
    }

    /// P57.4 — hand the foreground back after an approved escalation.
    pub fn restore_foreground(&self, window_id: u64) -> Result<(), DesktopError> {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.restore_foreground(window_id),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::WinBackend::restore_foreground(window_id),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => {
                crate::platform::macos::MacBackend::restore_foreground(window_id)
            }
            PlatformBackend::Unsupported => Err(DesktopError::Unsupported("no backend".into())),
        }
    }

    /// The click ladder this platform can actually walk, in fidelity order.
    pub fn click_profile(&self) -> ClickProfile {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(_) => linux_click_profile(),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::win_click_profile(),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::mac_click_profile(),
            PlatformBackend::Unsupported => ClickProfile {
                platform: "unsupported",
                rungs: Vec::new(),
                limits: vec!["no platform backend is attached, so there is no ladder".into()],
            },
        }
    }

    /// Attempt one rung of the click ladder.
    ///
    /// Every backend keeps its own interaction-mode floor here as well (the
    /// engine checks first, so a new backend cannot forget the rule): a rung
    /// that moves the pointer or needs the foreground must refuse under the
    /// Background default regardless of what the authority decided, because the
    /// Guard authorizes the *act* and the interaction default governs the
    /// *posture*.
    pub fn deliver_rung(
        &self,
        rung: ClickRung,
        target: &LadderTarget,
        mode: InteractionMode,
    ) -> RungDelivery {
        if rung.requires_foreground() && mode == InteractionMode::Background {
            return RungDelivery::Blocked(format!(
                "background contract: rung {} ({}) — switch the interaction default to \
                 Foreground (Settings → Computer use) or use a named-element action",
                rung.as_str(),
                rung.describe()
            ));
        }
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.deliver_rung(rung, target, mode),
            #[cfg(windows)]
            PlatformBackend::Win => {
                crate::platform::win::WinBackend::deliver_rung(rung, target, mode)
            }
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => {
                crate::platform::macos::MacBackend::deliver_rung(rung, target, mode)
            }
            PlatformBackend::Unsupported => {
                RungDelivery::Unavailable("no platform backend is attached".into())
            }
        }
    }

    /// The measured DPI scale for a window, with its provenance.
    ///
    /// Never a bare `f64`: a `1.0` on an unscaled display and a `1.0` because
    /// the platform could not be asked are different facts, and
    /// [`DpiScale::source`] is what tells them apart.
    pub fn dpi_scale(&self, _window: &WindowInfo) -> DpiScale {
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(b) => b.dpi_scale(),
            #[cfg(windows)]
            PlatformBackend::Win => crate::platform::win::WinBackend::dpi_scale(window),
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => crate::platform::macos::MacBackend::dpi_scale(),
            PlatformBackend::Unsupported => DpiScale::unknown(),
        }
    }

    /// `FIX-18` — verify the capture path for a window **before** capturing.
    ///
    /// The host-scoped half of the verdict, which is what a capability chip can
    /// show: "graphics capture unavailable — PrintWindow fallback". The
    /// per-target half (a live handle, a creatable capture item) needs a window
    /// and is verified per capture; its verdict travels on
    /// [`SeeResult::readiness`].
    pub fn capture_readiness(&self, window: &WindowInfo) -> CaptureReadiness {
        let mut probe = self.capture_probe(window);
        crate::capture::verify_capture(&mut *probe, window)
    }

    /// The platform's capture probe — the injectable boundary `FIX-18` verifies
    /// through.
    fn capture_probe(&self, _window: &WindowInfo) -> Box<dyn CaptureProbe> {
        #[cfg(target_os = "linux")]
        {
            Box::new(crate::platform::linux::X11CaptureProbe::new(matches!(
                self,
                PlatformBackend::X11(_)
            )))
        }
        #[cfg(windows)]
        {
            Box::new(crate::platform::win::WinCaptureProbe::new(
                crate::platform::win::hwnd_of(window),
            ))
        }
        #[cfg(all(target_os = "macos", not(windows)))]
        {
            Box::new(crate::platform::macos::MacCaptureProbe)
        }
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            Box::new(NoCaptureProbe)
        }
    }

    /// Honest per-platform capability surface.
    pub fn capabilities(&self) -> Capabilities {
        let ocr_available = crate::ocr::TesseractCli::default().available();
        // `FIX-18` — the readiness chip is a real probe, not a promise: an
        // unattached backend reports `unavailable` with a sentence rather than a
        // green dot over a dead path.
        let capture = self.capture_readiness(&WindowInfo {
            id: 0,
            title: String::new(),
            app: String::new(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            has_a11y_tree: false,
        });
        match self {
            #[cfg(target_os = "linux")]
            PlatformBackend::X11(_) => Capabilities {
                see: SeeMethod::X11GetImage,
                see_occluded: false,
                uia_tree: false,
                invoke_set_value: false,
                send_input: true,
                // Synthetic ButtonPress/ButtonRelease to the deepest child
                // under the point: the server moves nothing.
                background_input: true,
                // P57.4 — `_NET_ACTIVE_WINDOW` is restorable (real X window ids).
                foreground_restore: true,
                // P57.7 — no AT-SPI client in the dependency set yet, so there is
                // no accessibility action path here; X11 clicks are X-level.
                a11y_action: false,
                see_occluded_wgc: false,
                // An X11 connection means an interactive session.
                interactive_desktop: true,
                // TCC is a macOS mechanism; nothing gates capture/driving here.
                screen_recording_granted: true,
                accessibility_granted: true,
                ocr: ocr_available,
                window_list: true,
                launch_app: true,
                capture_readiness: CaptureReadinessSummary::from_readiness(&capture),
            },
            #[cfg(windows)]
            PlatformBackend::Win => Capabilities {
                see: if crate::platform::wgc::available() {
                    SeeMethod::WindowsGraphicsCapture
                } else {
                    SeeMethod::PrintWindow
                },
                // `PW_RENDERFULLCONTENT` renders the window's own content, so
                // occlusion does not decide the result — and WGC (where present)
                // is compositor-native on top of that.
                see_occluded: true,
                uia_tree: true,
                invoke_set_value: true,
                send_input: true,
                // UIA InvokePattern at the hit-test point, else PostMessage to
                // the target HWND.
                background_input: true,
                // P57.4 — `GetForegroundWindow`/`SetForegroundWindow` on real HWNDs.
                foreground_restore: true,
                // P57.7 — UIA `InvokePattern` IS an accessibility action.
                a11y_action: true,
                // P57.6 — a real probe: WinRT support + a live BGRA-capable
                // D3D11 device, so this only reads true where WGC can actually
                // return pixels (never a platform guess).
                see_occluded_wgc: crate::platform::wgc::available(),
                // P57.5 — Session 0 (services) has no interactive desktop.
                interactive_desktop: crate::platform::win::WinBackend::interactive_desktop(),
                screen_recording_granted: true,
                accessibility_granted: true,
                ocr: ocr_available,
                window_list: true,
                launch_app: true,
                // `FIX-18` — the probe, not the `see_occluded_wgc` bool: the
                // verdict names the pipeline and the reason it degraded.
                capture_readiness: CaptureReadinessSummary::from_readiness(&capture),
            },
            #[cfg(target_os = "macos")]
            PlatformBackend::Mac => Capabilities {
                see: SeeMethod::MacScreenCapture,
                see_occluded: false,
                uia_tree: false,
                invoke_set_value: false,
                send_input: true,
                // System Events `click at` is a real pointer event, so a
                // background coordinate click cannot be delivered here.
                background_input: false,
                // P57.4 — this backend assigns synthetic window ids for listing,
                // so a real foreground window cannot be named back. Restore is
                // therefore honestly unsupported here.
                foreground_restore: false,
                // P57.7 — `AXUIElementCopyElementAtPosition` + `kAXPressAction`
                // needs an ApplicationServices FFI layer that is not in yet.
                a11y_action: false,
                see_occluded_wgc: false,
                interactive_desktop: true,
                // P57.5 — TCC state, probed rather than assumed.
                screen_recording_granted:
                    crate::platform::macos::MacBackend::screen_recording_granted(),
                accessibility_granted: crate::platform::macos::MacBackend::accessibility_granted(),
                ocr: ocr_available,
                window_list: true,
                launch_app: true,
                capture_readiness: CaptureReadinessSummary::from_readiness(&capture),
            },
            PlatformBackend::Unsupported => Capabilities::default(),
        }
    }
}
