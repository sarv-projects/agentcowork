//! FIX-18 / `TASK-CUA-002` — **capture readiness**: verify the capture path is
//! actually usable *before* attempting a capture, and return a typed, actionable
//! result when it is not.
//!
//! `ARCH/42-EVIDENCE-MAP.md` §4 `FIX-18` re-verified against this tree: the
//! Windows capture path had **one** fact about itself — a `OnceLock<bool>` from
//! `GraphicsCaptureSession::IsSupported()` plus a D3D11 device creation — and
//! every other failure was a bare `Option::None` that meant "use the fallback
//! chain" (`platform/win.rs` `see()`) or a string error
//! (`"all capture methods failed"`). So a target WGC could never capture came
//! back as a successful `PrintWindow` capture with no record of why, and
//! `Capabilities::see_occluded_wgc` (a *host* probe) was the only readiness fact
//! the surface had.
//!
//! What is missing, and what this module adds, per `ARCH/24-COMPUTER-USE.md` §7
//! ("Vision model unavailable when scheduled → degrade with a recorded gap") and
//! §4 (capture discipline), and `REQ-CUA-008` / `REQ-WORLD-010`:
//!
//! - **per-target** readiness (the host is capable; *this* window may not be),
//! - the **supported pipeline** named, plus the fallback that will be used,
//! - a **typed fault** per failed check, each with its own guidance sentence,
//! - a **recorded degrade**: which pipeline ran, which was skipped and why, so
//!   the rung discipline is visible instead of inferred.
//!
//! The verification is a pure function over an injected [`CaptureProbe`], so the
//! readiness rules are tested on every host with fakes. The real WGC probe lives
//! in [`crate::platform::wgc`] (Windows-only, WinRT/D3D11) and the per-platform
//! composition lives in [`crate::platform`]; what stays unverified on this host is
//! the runtime behaviour of the Windows paths.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::now_ms;
use crate::types::{SeeMethod, WindowInfo};

/// The capture pipeline a platform can offer, in fidelity order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePipeline {
    /// Windows.Graphics.Capture — composites the window's own surface, so
    /// occlusion does not matter.
    WindowsGraphicsCapture,
    /// `PrintWindow` with `PW_RENDERFULLCONTENT` — the window renders its own
    /// content, so occlusion does not decide the result either.
    PrintWindow,
    /// `BitBlt` from the window's screen DC — returns what is on screen, so an
    /// occluded target comes back as the occluder.
    ScreenDc,
    /// `XGetImage` over the X11 window.
    X11GetImage,
    /// `screencapture -l <windowid>`.
    MacScreenCapture,
}

impl CapturePipeline {
    pub fn as_str(&self) -> &'static str {
        match self {
            CapturePipeline::WindowsGraphicsCapture => "windows_graphics_capture",
            CapturePipeline::PrintWindow => "print_window",
            CapturePipeline::ScreenDc => "screen_dc",
            CapturePipeline::X11GetImage => "x11_get_image",
            CapturePipeline::MacScreenCapture => "mac_screen_capture",
        }
    }

    /// The [`SeeMethod`] this pipeline reports on a capture, so the two cannot
    /// disagree about what ran.
    pub fn see_method(&self) -> SeeMethod {
        match self {
            CapturePipeline::WindowsGraphicsCapture => SeeMethod::WindowsGraphicsCapture,
            CapturePipeline::PrintWindow => SeeMethod::PrintWindow,
            CapturePipeline::ScreenDc => SeeMethod::ScreenDc,
            CapturePipeline::X11GetImage => SeeMethod::X11GetImage,
            CapturePipeline::MacScreenCapture => SeeMethod::MacScreenCapture,
        }
    }

    /// Does this pipeline return the window's own content regardless of what is
    /// drawn on top of it?
    ///
    /// This is the property the ladder's rung discipline turns on: a screen-DC
    /// capture of an occluded window is a capture **of the occluder**, so a vision
    /// model would then be reasoning about the wrong application
    /// (`ARCH/24` §4).
    pub fn is_occlusion_independent(&self) -> bool {
        matches!(
            self,
            CapturePipeline::WindowsGraphicsCapture
                | CapturePipeline::PrintWindow
                | CapturePipeline::X11GetImage
                | CapturePipeline::MacScreenCapture
        )
    }

    /// Is this the compositor-native pipeline?
    pub fn is_compositor_native(&self) -> bool {
        matches!(self, CapturePipeline::WindowsGraphicsCapture)
    }

    /// One honest sentence for a receipt.
    pub fn describe(&self) -> String {
        match self {
            CapturePipeline::WindowsGraphicsCapture => {
                "Windows.Graphics.Capture (compositor-native, occluded-window safe)".into()
            }
            CapturePipeline::PrintWindow => {
                "PrintWindow/PW_RENDERFULLCONTENT (the window renders its own content)".into()
            }
            CapturePipeline::ScreenDc => {
                "screen-DC BitBlt (returns what is on screen — an occluded target captures its \
                 occluder)"
                    .into()
            }
            CapturePipeline::X11GetImage => "XGetImage over the X11 window".into(),
            CapturePipeline::MacScreenCapture => "screencapture -l <windowid>".into(),
        }
    }
}

/// A single readiness check's verdict, with the reason it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCheck {
    /// WinRT / session availability (Windows.Graphics.Capture support).
    SessionAvailable,
    /// A BGRA-capable graphics device the capture pipeline can use.
    GraphicsDevice,
    /// The OS compositor (DWM) is running, so a windowed capture is meaningful.
    CompositorRunning,
    /// The window handle still names a live window.
    WindowHandleValid,
    /// The window has non-zero extent, so a capture has pixels to return.
    NonZeroExtent,
    /// The capture item for this target could be created.
    CaptureItem,
    /// The capture pipeline this platform offers is supported on this host.
    PipelineSupported,
}

impl CaptureCheck {
    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureCheck::SessionAvailable => "session_available",
            CaptureCheck::GraphicsDevice => "graphics_device",
            CaptureCheck::CompositorRunning => "compositor_running",
            CaptureCheck::WindowHandleValid => "window_handle_valid",
            CaptureCheck::NonZeroExtent => "non_zero_extent",
            CaptureCheck::CaptureItem => "capture_item",
            CaptureCheck::PipelineSupported => "pipeline_supported",
        }
    }

    /// What this check failing means for the caller: "the pipeline cannot run at
    /// all here" versus "this particular target cannot be captured".
    pub fn is_target_scoped(&self) -> bool {
        matches!(
            self,
            CaptureCheck::WindowHandleValid
                | CaptureCheck::NonZeroExtent
                | CaptureCheck::CaptureItem
        )
    }

    /// A one-line name for a receipt: what was verified.
    pub fn describe(&self) -> &'static str {
        match self {
            CaptureCheck::SessionAvailable => "graphics-capture session support",
            CaptureCheck::GraphicsDevice => "a BGRA-capable graphics device",
            CaptureCheck::CompositorRunning => "a running desktop compositor",
            CaptureCheck::WindowHandleValid => "a live window handle",
            CaptureCheck::NonZeroExtent => "a window with non-zero extent",
            CaptureCheck::CaptureItem => "a capture item for the target",
            CaptureCheck::PipelineSupported => "a supported capture pipeline",
        }
    }
}

/// Why one readiness check failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum CaptureFault {
    /// No interactive session (Session 0, a service, a locked desktop): there is
    /// no desktop to capture.
    NoInteractiveSession { detail: String },
    /// Windows.Graphics.Capture is not supported on this host.
    SessionUnsupported { detail: String },
    /// No BGRA-capable graphics device (headless GPU, a WARP-less session, a
    /// remote-desktop session without a compositor).
    NoGraphicsDevice { detail: String },
    /// The desktop compositor is not running.
    NoCompositor { detail: String },
    /// The platform offers no capture pipeline at all.
    PipelineUnsupported { detail: String },
    /// The window handle does not name a live window (destroyed, or reused since
    /// it was listed).
    InvalidWindowHandle { detail: String },
    /// The window has no extent, so a capture would be empty.
    ZeroExtent { detail: String },
    /// The capture item for the target could not be created (minimized to a
    /// shell icon, a protected surface, a window the compositor refuses).
    CaptureItemUnavailable { detail: String },
    /// Screen-recording / capture consent is not granted.
    ConsentMissing { detail: String },
    /// The window is occluded and the only available pipeline captures the screen
    /// rather than the window — the vision rung would then be reasoning about the
    /// occluder.
    OccludedAndScreenOnly { detail: String },
    /// The probe itself failed.
    ProbeFailed { detail: String },
}

impl CaptureFault {
    /// Which readiness check this fault belongs to.
    pub fn check(&self) -> CaptureCheck {
        match self {
            CaptureFault::NoInteractiveSession { .. } => CaptureCheck::SessionAvailable,
            CaptureFault::SessionUnsupported { .. } => CaptureCheck::SessionAvailable,
            CaptureFault::NoGraphicsDevice { .. } => CaptureCheck::GraphicsDevice,
            CaptureFault::NoCompositor { .. } => CaptureCheck::CompositorRunning,
            CaptureFault::PipelineUnsupported { .. } => CaptureCheck::PipelineSupported,
            CaptureFault::InvalidWindowHandle { .. } => CaptureCheck::WindowHandleValid,
            CaptureFault::ZeroExtent { .. } => CaptureCheck::NonZeroExtent,
            CaptureFault::CaptureItemUnavailable { .. } => CaptureCheck::CaptureItem,
            CaptureFault::ConsentMissing { .. } => CaptureCheck::SessionAvailable,
            CaptureFault::OccludedAndScreenOnly { .. } => CaptureCheck::CaptureItem,
            CaptureFault::ProbeFailed { .. } => CaptureCheck::PipelineSupported,
        }
    }

    /// Can a different pipeline deliver the same capture for this target?
    ///
    /// `false` means no fallback exists, so the honest answer is `Unavailable`
    /// rather than "degrade quietly".
    pub fn has_fallback(&self) -> bool {
        !matches!(
            self,
            CaptureFault::NoInteractiveSession { .. }
                | CaptureFault::InvalidWindowHandle { .. }
                | CaptureFault::ZeroExtent { .. }
                | CaptureFault::PipelineUnsupported { .. }
        )
    }

    /// One actionable sentence: what is wrong, and what the user can do. Never a
    /// bare error code (`REQ-CUA-002`: "unavailable rungs yield guidance, never
    /// silent failure").
    pub fn guidance(&self) -> String {
        match self {
            CaptureFault::NoInteractiveSession { .. } => {
                "this process has no interactive desktop session (Session 0 / a service), so there \
                 is nothing to capture — run the app in the user's session"
                    .into()
            }
            CaptureFault::SessionUnsupported { .. } => {
                "Windows.Graphics.Capture is not supported on this host — capture falls back to \
                 PrintWindow, which cannot see an occluded window"
                    .into()
            }
            CaptureFault::NoGraphicsDevice { .. } => {
                "no BGRA-capable graphics device is available for graphics capture — capture falls \
                 back to PrintWindow"
                    .into()
            }
            CaptureFault::NoCompositor { .. } => {
                "the desktop compositor is not running, so a window capture cannot be composited"
                    .into()
            }
            CaptureFault::PipelineUnsupported { .. } => {
                "this platform offers no window-capture pipeline on this build".into()
            }
            CaptureFault::InvalidWindowHandle { .. } => {
                "this window handle does not name a live window any more (it was closed, or the \
                 handle was reused) — re-list the windows"
                    .into()
            }
            CaptureFault::ZeroExtent { .. } => {
                "this window has no visible extent, so a capture would be empty — restore it first"
                    .into()
            }
            CaptureFault::CaptureItemUnavailable { .. } => {
                "no capture item could be created for this window (it may be minimized to a shell \
                 icon, protected, or refused by the compositor) — capture falls back to PrintWindow"
                    .into()
            }
            CaptureFault::ConsentMissing { .. } => {
                "screen-capture consent is not granted, so no pixels can be read — grant Screen \
                 Recording (macOS) or the equivalent consent before capturing"
                    .into()
            }
            CaptureFault::OccludedAndScreenOnly { .. } => {
                "this window is occluded and the only available pipeline captures the screen, so the \
                 result would be the occluding window — restore the target, or capture through a \
                 pipeline that composites the window itself"
                    .into()
            }
            CaptureFault::ProbeFailed { detail } => format!(
                "the capture readiness probe failed ({detail}); the capture was not attempted"
            ),
        }
    }
}

/// The readiness verdict for one capture attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    /// The named pipeline is verified usable for this target.
    Ready,
    /// A lower-fidelity pipeline will be used; the reason is recorded.
    Degraded,
    /// No pipeline can deliver this target's pixels.
    Unavailable,
}

impl CaptureState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureState::Ready => "ready",
            CaptureState::Degraded => "degraded",
            CaptureState::Unavailable => "unavailable",
        }
    }
}

/// Everything the surface needs to say about a capture before it is attempted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureReadiness {
    pub state: CaptureState,
    /// The pipeline that **will** run.
    pub pipeline: Option<CapturePipeline>,
    /// The pipeline that was verified and is being skipped, when degraded.
    pub degraded_from: Option<CapturePipeline>,
    /// The fault that caused the degrade or the refusal, verbatim.
    pub fault: Option<CaptureFault>,
    /// Which readiness check failed (named, for a receipt).
    pub failed_check: Option<CaptureCheck>,
    /// One actionable sentence. Never empty.
    pub guidance: String,
    /// The checks that passed, in order — evidence, not a claim.
    pub verified: Vec<CaptureCheck>,
    /// When the verdict was produced.
    pub verified_at_ms: u64,
}

impl CaptureReadiness {
    /// A verdict that verified the named pipeline for this target.
    pub fn ready(pipeline: CapturePipeline, verified: Vec<CaptureCheck>) -> Self {
        Self {
            state: CaptureState::Ready,
            pipeline: Some(pipeline),
            degraded_from: None,
            fault: None,
            failed_check: None,
            guidance: format!(
                "capture is ready: {} (verified: {})",
                pipeline.describe(),
                verified
                    .iter()
                    .map(|c| c.describe())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            verified,
            verified_at_ms: now_ms(),
        }
    }

    /// A verdict that will not capture anything.
    pub fn unavailable(fault: CaptureFault) -> Self {
        Self {
            state: CaptureState::Unavailable,
            pipeline: None,
            degraded_from: None,
            failed_check: Some(fault.check()),
            guidance: fault.guidance(),
            fault: Some(fault),
            verified: Vec::new(),
            verified_at_ms: now_ms(),
        }
    }

    /// Is a capture allowed to be attempted?
    pub fn is_capturable(&self) -> bool {
        !matches!(self.state, CaptureState::Unavailable)
    }

    /// Did the rung discipline change because of this verdict? (Vision may only be
    /// used for canvas/poor semantics/verification/a structured miss, so a
    /// degraded capture is itself a reason to prefer the structured rung.)
    pub fn is_degraded(&self) -> bool {
        matches!(self.state, CaptureState::Degraded)
    }

    /// The compact JSON form the Tauri surface and the audit chain carry.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state.as_str(),
            "pipeline": self.pipeline.map(|p| p.as_str()),
            "degradedFrom": self.degraded_from.map(|p| p.as_str()),
            "failedCheck": self.failed_check.map(|c| c.as_str()),
            "fault": self.fault.as_ref().map(|f| format!("{:?}", f)),
            "guidance": self.guidance,
            "verified": self
                .verified
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>(),
            "verifiedAtMs": self.verified_at_ms,
        })
    }
}

/// What a platform can say about a capture, asked one fact at a time.
///
/// Implemented per platform (the WGC/D3D11 probe on Windows, the display on X11,
/// TCC on macOS) and faked in tests, which is what makes the readiness rules
/// testable off Windows.
pub trait CaptureProbe: Send {
    /// The pipelines this platform offers, in fidelity order. Empty means none.
    fn pipelines(&self) -> Vec<CapturePipeline>;

    /// Host-scoped checks, in order. Return the first fault.
    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault>;

    /// Whether this target is occluded (a screen-DC capture would then be a
    /// capture of the occluder).
    fn is_occluded(&mut self, window: &WindowInfo) -> bool;

    /// Target-scoped checks for the first named pipeline, in order. Return the
    /// first fault.
    fn target(
        &mut self,
        window: &WindowInfo,
        pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault>;
}

/// Verify the capture path for one window, before any capture is attempted.
///
/// The order is the point: host checks first (a host that cannot capture at all
/// should not be probed per window), then the highest-fidelity pipeline's
/// target checks, then the degrade/fallback decision. A pipeline is only skipped
/// for a **recorded** fault — that is what makes "guidance instead of a failed or
/// empty capture" (`FIX-18`) true rather than aspirational.
pub fn verify_capture<P: CaptureProbe + ?Sized>(
    probe: &mut P,
    window: &WindowInfo,
) -> CaptureReadiness {
    let pipelines = probe.pipelines();
    if pipelines.is_empty() {
        return CaptureReadiness::unavailable(CaptureFault::PipelineUnsupported {
            detail: "the platform declares no capture pipeline".into(),
        });
    }
    let mut verified: Vec<CaptureCheck> = Vec::new();
    let host = match probe.host() {
        Ok(checks) => {
            verified.extend(checks);
            Ok(())
        }
        Err(fault) => Err(fault),
    };
    if let Err(fault) = host {
        // A host that cannot capture has no fallback either: report it once, and
        // do not pretend a lower pipeline exists.
        let has_fallback = pipelines.len() > 1
            && fault.has_fallback()
            && !matches!(fault, CaptureFault::NoInteractiveSession { .. });
        return if has_fallback {
            degraded_from(pipelines[0], fault, verified, Some(pipelines[1]))
        } else {
            CaptureReadiness::unavailable(fault)
        };
    }

    let mut skip: Option<(CapturePipeline, CaptureFault)> = None;
    for pipeline in &pipelines {
        match probe.target(window, *pipeline) {
            Ok(checks) => {
                verified.extend(checks);
                return match skip {
                    // A degrade that was recorded, not a silent fall-through.
                    Some((from, fault)) => degraded_from(*pipeline, fault, verified, Some(from)),
                    None => CaptureReadiness {
                        state: CaptureState::Ready,
                        pipeline: Some(*pipeline),
                        degraded_from: None,
                        fault: None,
                        failed_check: None,
                        guidance: format!(
                            "capture is ready: {} (verified: {})",
                            pipeline.describe(),
                            verified
                                .iter()
                                .map(|c| c.describe())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        verified,
                        verified_at_ms: now_ms(),
                    },
                };
            }
            Err(fault) => {
                // An occlusion fault on a screen-only pipeline is refused rather
                // than degraded: the bytes would be another application's window.
                if *pipeline == CapturePipeline::ScreenDc && probe.is_occluded(window) {
                    return CaptureReadiness::unavailable(CaptureFault::OccludedAndScreenOnly {
                        detail: fault.guidance(),
                    });
                }
                skip = Some((*pipeline, fault));
            }
        }
    }
    let Some((_from, fault)) = skip else {
        return CaptureReadiness::unavailable(CaptureFault::ProbeFailed {
            detail: "no pipeline was probed".into(),
        });
    };
    // Every pipeline refused this target: nothing can capture it.
    CaptureReadiness::unavailable(fault)
}

fn degraded_from(
    pipeline: CapturePipeline,
    fault: CaptureFault,
    verified: Vec<CaptureCheck>,
    degraded_from: Option<CapturePipeline>,
) -> CaptureReadiness {
    let state = if pipeline.is_occlusion_independent() {
        CaptureState::Degraded
    } else {
        // A screen-only pipeline cannot honestly be "degraded into": it captures
        // whatever is on screen. Refuse instead of returning another app's pixels.
        CaptureState::Unavailable
    };
    let failed_check = fault.check();
    let guidance = format!(
        "{} — falling back to {} ({}); {}",
        fault.guidance(),
        pipeline.describe(),
        pipeline.as_str(),
        state_label(state)
    );
    CaptureReadiness {
        state,
        pipeline: match state {
            CaptureState::Unavailable => None,
            _ => Some(pipeline),
        },
        degraded_from,
        failed_check: Some(failed_check),
        fault: Some(fault),
        guidance,
        verified,
        verified_at_ms: now_ms(),
    }
}

fn state_label(state: CaptureState) -> &'static str {
    match state {
        CaptureState::Ready => "capture is verified ready",
        CaptureState::Degraded => "the capture is recorded as degraded",
        CaptureState::Unavailable => "no capture will be attempted",
    }
}

// ---------------------------------------------------------------------------
// The degrade record that travels with a capture
// ---------------------------------------------------------------------------

/// What a capture did, and what it cost.
///
/// `ARCH/24` §4/§7 make this a discipline rather than an implementation detail:
/// screenshots enter context only when the vision rung ran, the capture is
/// size-capped before send, and a degrade is recorded rather than hidden
/// (`REQ-CUA-008`, `REQ-CUA-007`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureDegrade {
    /// The readiness verdict this capture was made under.
    pub readiness: CaptureReadiness,
    /// The pipeline that actually produced the bytes.
    pub method: SeeMethod,
    /// The pipeline that was preferred and skipped, when the verdict degraded.
    pub skipped: Option<CapturePipeline>,
    /// The fault that caused the skip.
    pub reason: Option<String>,
    /// Was the capture requested by the vision rung? (`ARCH/24` §7: screenshots
    /// enter context only via the vision rung.)
    pub vision_rung: bool,
}

impl CaptureDegrade {
    /// Did the capture come from a lower-fidelity path than the host prefers?
    pub fn is_degraded(&self) -> bool {
        self.readiness.is_degraded() || self.skipped.is_some()
    }

    /// One line for an audit row / receipt.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("{:?}", self.method)];
        if let Some(skipped) = self.skipped {
            parts.push(format!("degraded from {}", skipped.as_str()));
        }
        if let Some(reason) = &self.reason {
            parts.push(reason.clone());
        }
        if !self.vision_rung {
            parts.push(
                "captured outside the vision rung — a capture must not enter context unless the \
                 vision rung ran"
                    .to_string(),
            );
        }
        parts.join(" · ")
    }
}

// ---------------------------------------------------------------------------
// Cached host probes
// ---------------------------------------------------------------------------

/// A host-scoped probe result, cached because creating a graphics device is not
/// free (`platform/wgc.rs` already cached its `bool`; the *reason* is cached
/// with it now, which is the `FIX-18` difference).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProbe {
    result: Result<Vec<CaptureCheck>, CaptureFault>,
    /// When the cached verdict was taken. `pub` so a test can age it without
    /// sleeping for the TTL.
    pub at: Instant,
}

/// How long a host probe stays valid before it is re-run.
pub const HOST_PROBE_TTL: Duration = Duration::from_secs(60);

impl HostProbe {
    pub fn new(result: Result<Vec<CaptureCheck>, CaptureFault>) -> Self {
        Self {
            result,
            at: Instant::now(),
        }
    }

    /// The cached verdict, re-running the probe when the TTL has passed.
    pub fn get_or_probe<F>(&mut self, probe: F) -> &Result<Vec<CaptureCheck>, CaptureFault>
    where
        F: FnOnce() -> Result<Vec<CaptureCheck>, CaptureFault>,
    {
        if self.at.elapsed() > HOST_PROBE_TTL {
            self.result = probe();
            self.at = Instant::now();
        }
        &self.result
    }

    /// The age of the cached verdict.
    pub fn age(&self) -> Duration {
        self.at.elapsed()
    }
}

/// A shared, cached host probe, so repeated readiness checks (the status chip, a
/// capture, the agent path) agree and pay the probe cost once.
#[derive(Debug, Default)]
pub struct SharedHostProbe {
    cell: Mutex<Option<HostProbe>>,
}

impl SharedHostProbe {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get-or-probe the host verdict.
    pub fn get_or_probe<F>(&self, probe: F) -> Result<Vec<CaptureCheck>, CaptureFault>
    where
        F: FnOnce() -> Result<Vec<CaptureCheck>, CaptureFault>,
    {
        let mut guard = self.cell.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(HostProbe::new(probe()));
            return guard.as_ref().map(|p| p.result.clone()).unwrap_or_else(|| {
                Err(CaptureFault::ProbeFailed {
                    detail: "probe".into(),
                })
            });
        }
        guard
            .as_mut()
            .expect("just installed")
            .get_or_probe(probe)
            .clone()
    }

    /// Drop the cached verdict, forcing the next check to probe again.
    pub fn invalidate(&self) {
        let mut guard = self.cell.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
}

/// A capture probe that always refuses, used where no platform backend is
/// attached — so an unattached host gets typed guidance rather than a `None`.
#[derive(Debug, Default)]
pub struct NoCaptureProbe;

impl CaptureProbe for NoCaptureProbe {
    fn pipelines(&self) -> Vec<CapturePipeline> {
        Vec::new()
    }
    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault> {
        Err(CaptureFault::NoInteractiveSession {
            detail: "no platform backend is attached".into(),
        })
    }
    fn is_occluded(&mut self, _window: &WindowInfo) -> bool {
        false
    }
    fn target(
        &mut self,
        _window: &WindowInfo,
        _pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault> {
        Err(CaptureFault::PipelineUnsupported {
            detail: "no platform backend is attached".into(),
        })
    }
}

/// A shared probe handle, so a host-scoped verdict is computed once.
pub type SharedProbe = Arc<SharedHostProbe>;
