//! agentcowork-desktop — E9 desktop computer-use (required, not a cut).
//!
//! ChatGPT Desktop + Claude Computer Use parity over *native* windows:
//!
//! 1. **See** — per-window capture + region zoom (X11 GetImage live; Win
//!    PrintWindow → screen-DC; macOS `screencapture`). Windows.Graphics.Capture
//!    (occluded capture) is the documented follow-on seam.
//! 2. **Read** — window/app list + UIA tree with indexes + click-by-name
//!    (Windows); EWMH list (X11); macOS window list. Empty tree → OCR.
//! 3. **Act** — UIA Invoke/SetValue first, then SendInput (Windows); XTEST
//!    (X11); System Events (macOS). Observe → **one** action → re-observe.
//! 4. **Vision fallback** — tesseract word boxes + click-coords math.
//! 5. **Verify** — assert/retry/locator cascade, halt-over-guess.
//! 6. **Layer-1 first** — API > CDP browser > desktop GUI routing.
//! 7. **Guard-2** — app allow-list, confirm taxonomy, hard denies, kill
//!    switch, rate limit, safe zones, Merkle audit.
//! 8. **macOS twin** — same surface via Screen Recording + Accessibility.
//!
//! The engine is a library; the desktop host wires the [`policy::PermissionGate`]
//! to `agentcowork-guard::TicketStore` and the [`policy::AuditSink`] to
//! `agentcowork-audit::AuditWriter` (Merkle chain), exactly like every other
//! effect in the product.

pub mod apps;
pub mod capture;
pub mod geometry;
pub mod ladder;
pub mod launch;
pub mod ocr;
pub mod platform;
pub mod policy;
pub mod readiness;
pub mod router;
pub mod types;
pub mod uia;
pub mod verify;

use std::sync::Arc;

use thiserror::Error;

pub use apps::{AppSource, InstalledApp, annotate_inventory, installed_apps, search_apps};
pub use capture::{
    CaptureCheck, CaptureDegrade, CaptureFault, CapturePipeline, CaptureProbe, CaptureReadiness,
    CaptureState, NoCaptureProbe, SharedHostProbe, verify_capture,
};
pub use geometry::{
    DpiScale, DpiSource, IMAGE_FACTOR, OutputBudget, SEE_MAX_BYTES, SEE_MAX_DIMENSION_PX,
    SeeBudget, enforce_output_budget,
};
pub use ladder::{
    ClickLadderDriver, ClickProfile, ClickRung, LadderTarget, LadderVerdict, RungAttempt,
    RungAttemptOutcome, RungDelivery, RungGate, walk_ladder,
};
pub use launch::{is_secret_env_name, prepare_child, resolve_target};
pub use ocr::{OcrEngine, VisionHit, locate_phrase};
pub use policy::{
    ActProvenance, AppPolicy, AuditSink, ConfirmClass, DesktopGuard, GateDecision, InteractionMode,
    PermissionGate,
};
pub use readiness::{Readiness, ReadinessState, derive as derive_readiness};
pub use router::{Layer, RouteDecision, route};
pub use types::{
    ActKind, ActOutcome, Capabilities, EscalationRequest, ForegroundSnapshot, ReadNode, ReadResult,
    Region, SeeMethod, SeeResult, VerifyOutcome, WindowInfo,
};
pub use uia::{
    Coverage, ElementHandle, ElementQuery, NameMatch, ObservationHandle, ObservedElement,
    Resolution, SnapshotEpoch, StructuredRead, UiaFault, UiaNode, UiaProvider, UiaRead,
    UiaReadStatus, resolve, resolve_in,
};
pub use verify::{Locator, Observer, ReadConfidence, Verifier};

/// Milliseconds since the Unix epoch (observation freshness, capture receipts,
/// `ForegroundSnapshot` timestamps).
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Every failure mode of the desktop engine.
#[derive(Debug, Error)]
pub enum DesktopError {
    #[error("platform error: {0}")]
    Platform(String),
    #[error("invalid region: {0}")]
    InvalidRegion(String),
    #[error("guard-2: {0}")]
    Guard(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The see/screenshot surface refused to return the capture because it
    /// could not be brought under the named output budget. Fail-closed: the
    /// caller gets the real numbers and names region zoom, never an
    /// over-budget payload with a quiet nod.
    #[error("output budget: {0}")]
    OutputBudget(String),
    /// `FIX-18` — the capture path was **verified before use** and is not ready
    /// for this target, so no capture was attempted. Typed on purpose: the old
    /// code answered with a bare `Option::None` and then `"all capture methods
    /// failed"`, which is indistinguishable from a platform bug. The guidance is
    /// the sentence a card shows.
    #[error("capture not ready ({state}): {guidance}")]
    CaptureNotReady {
        /// `degraded` or `unavailable`.
        state: &'static str,
        /// The actionable sentence from the readiness verdict.
        guidance: String,
        /// Which readiness check failed, named.
        failed_check: &'static str,
    },
}

pub type Result<T> = std::result::Result<T, DesktopError>;

/// The full E9 engine: platform backend + Guard-2 + verifier + OCR.
pub struct DesktopEngine {
    backend: platform::PlatformBackend,
    guard: Arc<DesktopGuard>,
    verifier: Verifier,
    ocr: Arc<dyn OcrEngine>,
}

impl DesktopEngine {
    /// Construct the engine for the current platform with the given Guard-2
    /// policy/gate/audit wiring.
    pub fn new(
        policy: AppPolicy,
        gate: Box<dyn PermissionGate>,
        sink: Box<dyn policy::AuditSink>,
    ) -> Result<Self> {
        let backend = platform::PlatformBackend::current()?;
        let ocr: Arc<dyn OcrEngine> = if ocr::TesseractCli::default().available() {
            Arc::new(ocr::TesseractCli::default())
        } else {
            Arc::new(ocr::NoOcr)
        };
        Ok(Self {
            backend,
            guard: Arc::new(DesktopGuard::new(policy, gate, sink)),
            verifier: Verifier::default(),
            ocr,
        })
    }

    /// Construct with a pre-built guard (tests inject their own gate/sink).
    pub fn with_guard(
        backend: platform::PlatformBackend,
        guard: DesktopGuard,
        ocr: Arc<dyn OcrEngine>,
    ) -> Self {
        Self {
            backend,
            guard: Arc::new(guard),
            verifier: Verifier::default(),
            ocr,
        }
    }

    pub fn guard(&self) -> &DesktopGuard {
        &self.guard
    }

    /// P57.8 — the policy the Guard-2 preflight is currently enforcing.
    pub fn policy(&self) -> AppPolicy {
        self.guard.policy()
    }

    /// P57.8 — apply a policy change to the **live** engine (Settings →
    /// Computer use writes an allow-list row or the interaction default and the
    /// next action is gated on it; no restart, no re-attach).
    pub fn set_policy(&self, policy: AppPolicy) {
        self.guard.set_policy(policy);
    }

    /// Honest capability surface for this platform (UI shows this).
    pub fn capabilities(&self) -> Capabilities {
        self.backend.capabilities()
    }

    /// The emergency kill switch (STOP button / estop).
    pub fn emergency_stop(&self) {
        self.guard.kill.stop();
    }

    pub fn resume(&self) {
        self.guard.kill.resume();
    }

    // ---- See ----

    /// Capture a window (or a sub-region — Claude-class region zoom).
    pub fn see(&self, window: &WindowInfo) -> Result<SeeResult> {
        self.see_region(
            window,
            Region::full(window.width.max(1), window.height.max(1)),
        )
    }

    /// Capture a region, applying the **named output budget** before returning.
    ///
    /// The budget lives here, not in the platform backends, for two reasons: it
    /// is one implementation every platform gets (so the `<= 1280 px` /
    /// `<= 900 KiB` claim is a fact rather than a per-platform intention), and
    /// it is one place the result can **say** what it did
    /// ([`SeeResult::budget`]). A capture over the dimension ceiling is
    /// resampled onto the patch grid and reported as clamped; one still over the
    /// byte ceiling after that is **refused** with
    /// [`DesktopError::OutputBudget`] — the remedy is a smaller region, not a
    /// silent downscale.
    pub fn see_region(&self, window: &WindowInfo, region: Region) -> Result<SeeResult> {
        self.guard.kill.check().map_err(DesktopError::Guard)?;
        let mut raw = self.backend.see(window, region)?;
        // The DPI provenance is the platform's, measured; the budget is ours.
        raw.dpi = self.backend.dpi_scale(window);
        raw.scale = raw.dpi.factor;
        let budget = OutputBudget::shipped();
        let (png, width, height, report) = geometry::enforce_output_budget(
            std::mem::take(&mut raw.png),
            raw.width,
            raw.height,
            &budget,
        )?;
        raw.png = png;
        // A clamp changes the image the caller receives, so the reported
        // dimensions must be the image's real dimensions and the region must be
        // re-derived against them — otherwise a caller reading `region` would
        // address the pre-clamp geometry.
        raw.width = width;
        raw.height = height;
        raw.region = if report.output_width == report.captured_width
            && report.output_height == report.captured_height
        {
            raw.region
        } else {
            Region::full(width, height)
        };
        raw.budget = Some(report);
        Ok(raw)
    }

    // ---- Read ----

    pub fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        self.guard.kill.check().map_err(DesktopError::Guard)?;
        self.backend.list_windows()
    }

    /// Read a window: a11y tree when the platform exposes one, else None
    /// (the caller then uses the OCR vision fallback).
    pub fn read(&self, window: &WindowInfo) -> Result<ReadResult> {
        self.guard.kill.check().map_err(DesktopError::Guard)?;
        let mut read = self.backend.read(window)?;
        // Never trust a hardcoded 1.0: the platform's measured scale wins, and
        // its provenance travels in `dpi_scale`'s sibling `DpiScale` on `see`.
        read.dpi_scale = self.backend.dpi_scale(window).factor;
        Ok(read)
    }

    // ---- Vision fallback (OCR) ----

    pub fn ocr_window(&self, window: &WindowInfo) -> Result<Vec<types::OcrWord>> {
        let see = self.see(window)?;
        Ok(self.ocr.ocr(&see.png))
    }

    /// Resolve a phrase to a click point via OCR (vision fallback).
    pub fn resolve_by_ocr(&self, window: &WindowInfo, phrase: &str) -> Result<ocr::VisionHit> {
        let words = self.ocr_window(window)?;
        Ok(ocr::locate_phrase(&words, phrase))
    }

    // ---- Act (dual-guarded) ----

    /// Run one action through the full Guard-2 gate; on `Allow` it executes.
    /// Returns the gate decision + the execution outcome.
    /// The **human-gesture** act path — the user's own UI action.
    ///
    /// An agent-initiated act must call [`DesktopEngine::act_with`] with
    /// [`ActProvenance::Agent`] so its audit row is not filed as a human gesture.
    pub fn act(&self, window: &WindowInfo, act: &ActKind, key: Option<&str>) -> Result<ActOutcome> {
        self.act_with(window, act, key, ActProvenance::HumanGesture)
    }

    /// [`DesktopEngine::act`] with explicit provenance.
    ///
    /// The gate logic, background contract, launch validation, rate limit and
    /// kill switch are identical for every caller — provenance changes only the
    /// authority class recorded on the audit row.
    ///
    /// A coordinate or named click is **ladder-owned**: after the ordinary
    /// Guard-2 preflight it walks [`ClickRung`] in the platform's declared
    /// fidelity order, and the rung that ran is reported in
    /// [`ActOutcome::click`]. Reaching the pointer-moving rung needs a
    /// Guard-2 cursor-takeover decision; without one the walk stops and the
    /// outcome carries the refusal — it never falls through to raw input.
    pub fn act_with(
        &self,
        window: &WindowInfo,
        act: &ActKind,
        key: Option<&str>,
        provenance: ActProvenance,
    ) -> Result<ActOutcome> {
        let mode = self.guard.policy().interaction_mode;
        // P57.1 — a launch must name one exact, absolute program before any
        // policy or platform work: a relative path would resolve against this
        // process's cwd, so the executed file could differ from the allow-list
        // entry the gate matched.
        if let Some(reason) = crate::launch::validate(act) {
            return Ok(ActOutcome::err(
                act.clone(),
                format!("launch refused: {reason}"),
            ));
        }
        // P57.3 — the background contract is enforced before the platform call:
        // under the Background default the driver never raises another window,
        // because that is a foreground escalation (P57.4), not a default.
        if matches!(act, ActKind::ActivateWindow { .. })
            && !self.guard.policy().allows_raising_windows()
        {
            return Ok(ActOutcome::err(
                act.clone(),
                "background contract: raising another window needs the Foreground \
                 interaction default (Settings → Computer use)",
            ));
        }
        // P57.2 — the Guard-2 subject is the program being launched, never the
        // window that happens to be focused: allow-listing an app has to gate
        // that app.
        let subject = act.launch_target().unwrap_or(window.app.as_str());
        let decision = self
            .guard
            .preflight_with(subject, act, key, provenance)
            .map_err(DesktopError::Guard)?;
        if decision != GateDecision::Allow {
            return Ok(ActOutcome::err(
                act.clone(),
                format!("gate decision: {}", decision.as_str()),
            ));
        }
        if Self::is_ladder_act(act) {
            return self.act_ladder(window, act, provenance);
        }
        self.backend.act(window, act, mode)?;
        Ok(ActOutcome::ok(act.clone()))
    }

    /// Does this act go through the click ladder?
    ///
    /// Coordinate and named clicks: the two forms the ladder's rungs can
    /// deliver. `SetValue` also falls back to raw input inside the Windows
    /// backend, and that fallback is **not** yet ladder-gated — a known,
    /// reported gap rather than a silent one.
    fn is_ladder_act(act: &ActKind) -> bool {
        matches!(act, ActKind::Click { .. } | ActKind::ClickByName { .. })
    }

    /// Walk the click ladder for one act. See [`ladder`] for the contract.
    fn act_ladder(
        &self,
        window: &WindowInfo,
        act: &ActKind,
        provenance: ActProvenance,
    ) -> Result<ActOutcome> {
        let driver = PlatformLadderDriver::new(&self.backend, &self.guard, provenance);
        let target = LadderTarget::new(window.clone(), act.clone());
        let verdict = walk_ladder(&driver, &target);
        // The verdict is audited whatever it was: which rung ran (or why none
        // did) and whether the walk escalated, on the same Merkle chain as the
        // act itself.
        self.guard
            .audit_ladder(&window.app, act, &verdict, provenance);
        match &verdict {
            LadderVerdict::Delivered { .. } => Ok(ActOutcome {
                kind: act.clone(),
                ok: true,
                verification: None,
                click: Some(verdict),
                error: None,
            }),
            LadderVerdict::NeedsAuthorization { reason, .. }
            | LadderVerdict::Refused { reason, .. }
            // `FIX-17` — an unknown region is a refusal too, and it carries the
            // same actionable sentence: no lower rung ran, and nothing was
            // synthesized into a region nobody could see.
            | LadderVerdict::Unknown { reason, .. } => {
                // A refusal must name what the user could do, and the
                // foreground escalation request is exactly that (P57.4).
                let escalation = self.escalation_for(window, act);
                let suffix = match &escalation {
                    Some(req) => format!(" — {}", req.reason),
                    None => String::new(),
                };
                Ok(ActOutcome::refused(
                    act.clone(),
                    format!("{reason}{suffix}"),
                    verdict,
                ))
            }
            LadderVerdict::Exhausted { .. } | LadderVerdict::Misconfigured { .. } => {
                Ok(ActOutcome::refused(
                    act.clone(),
                    verdict.describe(),
                    verdict,
                ))
            }
        }
    }

    // ---- P57.4 — foreground escalation (never silent) ----

    /// Decide whether an act cannot be delivered under the current interaction
    /// default. Pure: no policy change, no side effect — the UI renders this as
    /// the Guard-2 escalation card.
    pub fn escalation_for(&self, window: &WindowInfo, act: &ActKind) -> Option<EscalationRequest> {
        if self.guard.policy().allows_raising_windows() {
            return None; // already Foreground: no escalation needed
        }
        let caps = self.backend.capabilities();
        let needs_foreground = match act {
            // Raising another window IS the foreground change.
            ActKind::ActivateWindow { .. } => true,
            // A coordinate click is background-capable only where the platform
            // has a non-moving path (Windows UIA/PostMessage, X11 synthetic).
            ActKind::Click { .. } => !caps.background_input,
            // Scroll and drag are pointer motion by definition.
            ActKind::Scroll { .. } | ActKind::Drag { .. } => true,
            _ => false,
        };
        if !needs_foreground {
            return None;
        }
        Some(EscalationRequest {
            reason: format!(
                "background input refused; escalate to foreground to {}",
                act.describe()
            ),
            blocked_act: act.clone(),
            requires_gesture: true,
            target: window.app.clone(),
        })
    }

    /// Read the window that currently owns the foreground, so an approved
    /// escalation can give it back. `window_id: None` is honest — the platform
    /// could not tell, and restore is then a no-op rather than a guess.
    pub fn foreground_snapshot(&self) -> ForegroundSnapshot {
        ForegroundSnapshot {
            window_id: self.backend.foreground_window(),
            captured_at_ms: now_ms(),
        }
    }

    /// Give the foreground back after an approved escalation.
    pub fn restore_foreground(&self, snapshot: &ForegroundSnapshot) -> Result<()> {
        match snapshot.window_id {
            Some(id) => self.backend.restore_foreground(id),
            None => Ok(()),
        }
    }

    /// Run an act that needs a foreground escalation — **only** with an explicit
    /// human gesture. Without one this returns a refused outcome carrying the
    /// reason (the UI shows the card and nothing moves). With one, the previous
    /// foreground and interaction default are snapshotted, the policy is switched
    /// to Foreground for this single act, and both are restored afterwards — so a
    /// foreground escalation is reversible and auditable, not a focus steal.
    ///
    /// For a coordinate click this is the *only* route to the pointer-moving rung:
    /// the gesture is what makes `Foreground` true for the one act, and the
    /// ladder's Guard-2 cursor-takeover decision is asked again inside it.
    pub fn act_escalating(
        &self,
        window: &WindowInfo,
        act: &ActKind,
        key: Option<&str>,
        gesture_approved: bool,
    ) -> Result<ActOutcome> {
        let Some(request) = self.escalation_for(window, act) else {
            return self.act(window, act, key);
        };
        if !gesture_approved {
            return Ok(ActOutcome::err(
                act.clone(),
                format!(
                    "foreground escalation requires a human gesture: {}",
                    request.reason
                ),
            ));
        }
        let previous_policy = self.guard.policy();
        let snapshot = self.foreground_snapshot();
        let mut foreground_policy = previous_policy.clone();
        foreground_policy.interaction_mode = InteractionMode::Foreground;
        self.guard.set_policy(foreground_policy);
        let outcome = self.act(window, act, key);
        // Put the interaction default back no matter how the act ended, then
        // hand the foreground back to whoever had it.
        self.guard.set_policy(previous_policy);
        let restore = self.restore_foreground(&snapshot);
        let mut outcome = outcome?;
        if let Err(e) = restore {
            // The act may have succeeded; a failed restore is still a real
            // failure the caller must not read as clean.
            outcome.error = Some(match outcome.error {
                Some(existing) => format!("{existing}; foreground restore failed: {e}"),
                None => format!("foreground restore failed: {e}"),
            });
            outcome.ok = false;
        }
        Ok(outcome)
    }

    /// Observe → one action → re-observe with a verify cascade. Halts
    /// (never guesses) when the locator is not satisfied.
    pub fn act_with_verify(
        &self,
        window: &WindowInfo,
        act: &ActKind,
        locator: &Locator,
        key: Option<&str>,
    ) -> Result<ActOutcome> {
        let mut outcome = self.act_with(window, act, key, ActProvenance::HumanGesture)?;
        if outcome.ok {
            let obs = EngineObserver {
                engine: self,
                window_id: window.id,
            };
            let verdict = self.verifier.verify(window.id, locator, &obs);
            let halted = matches!(&verdict, VerifyOutcome::Halt { .. });
            outcome.verification = Some(verdict);
            if halted {
                outcome.ok = false;
                outcome.error = Some(format!("verify halt: {:?}", outcome.verification));
            }
        }
        Ok(outcome)
    }

    /// Vision-driven click: OCR the window, locate the phrase, click the point
    /// (all through Guard-2). NotFound → honest halt, never a guess.
    ///
    /// The OCR hit is in the **returned image's** coordinate space, so it is
    /// mapped back to a window coordinate through the capture's output scale
    /// before the click is attempted. Without that, a clamped (downscaled)
    /// capture would aim the click at a fraction of the intended point — a real
    /// bug introduced by having an honest budget, and therefore handled here
    /// rather than left to the caller.
    ///
    /// `REQ-CUA-001` / `REQ-CUA-008` — the vision rung is **last**: a capture is
    /// refused when a conclusive structured read already answers the question
    /// ("a screenshot is never taken for something an API or a tree can answer"),
    /// and refused when the read could not see the region at all (`REQ-CUA-006`:
    /// no input is synthesized into an unknown region). An `absent` read — the
    /// honest "this app has no structural UI" — is exactly the case vision is for.
    pub fn vision_click(&self, window: &WindowInfo, phrase: &str) -> Result<ActOutcome> {
        if let Err(reason) = self.vision_rung_advisable(window) {
            return Ok(ActOutcome::err(
                ActKind::ClickByName {
                    name: phrase.into(),
                },
                reason,
            ));
        }
        let see = self.see(window)?;
        let scale = (
            see.budget.as_ref().map(|b| b.output_scale_x).unwrap_or(1.0),
            see.budget.as_ref().map(|b| b.output_scale_y).unwrap_or(1.0),
        );
        let words = self.ocr.ocr(&see.png);
        match ocr::locate_phrase(&words, phrase) {
            ocr::VisionHit::Point { x, y } | ocr::VisionHit::RegionCenter { x, y, .. } => {
                let (wx, wy) = (divide(x, scale.0), divide(y, scale.1));
                self.act(window, &ActKind::Click { x: wx, y: wy }, None)
            }
            ocr::VisionHit::NotFound => Ok(ActOutcome::err(
                ActKind::ClickByName {
                    name: phrase.into(),
                },
                format!("phrase {phrase:?} not found in OCR — halting, not guessing"),
            )),
        }
    }

    /// May the vision rung run for this window at all?
    ///
    /// The one place the structured-first rule is decided, so a caller that wants
    /// to capture for a vision model asks here first and gets a reason when the
    /// answer is no:
    ///
    /// - a **conclusive** read that found actionable controls → no. The tree
    ///   answers it, and capturing would be a screenshot taken for something
    ///   structure already knows (`REQ-CUA-001`).
    /// - a read that **could not look** (elevated, dead handle) → no, and no input
    ///   is synthesized into the unknown region (`REQ-CUA-006`).
    /// - `absent` (no structural UI), or a read with nothing actionable → yes.
    ///   That is the documented vision-rung case (`ARCH/24` §8: "Tree empty /
    ///   semantics poor → OCR patch → vision rung").
    ///
    /// `Err` is the honest refusal, carrying the sentence a card shows.
    pub fn vision_rung_advisable(&self, window: &WindowInfo) -> std::result::Result<(), String> {
        let read = self
            .read(window)
            .map_err(|e| format!("the structured read failed before the vision rung: {e}"))?;
        vision_rung_verdict(&read)
    }
}

/// The structured-first decision, as a pure function of one read.
///
/// Split out from [`DesktopEngine::vision_rung_advisable`] so the rule is testable
/// against every read status without a platform backend — this is the
/// `REQ-CUA-001` / `REQ-CUA-008` discipline, and a unit test on a live desktop
/// would not cover the states that matter.
pub fn vision_rung_verdict(read: &ReadResult) -> std::result::Result<(), String> {
    if read.status.as_str() == "unknown" {
        return Err(format!(
            "the vision rung refused for this window: {}",
            read.guidance
                .clone()
                .unwrap_or_else(|| "this process could not read it".into())
        ));
    }
    let actionable = read
        .tree
        .as_ref()
        .map(|t| t.flatten().iter().any(|n| n.actionable))
        .unwrap_or(false);
    if read.may_infer_absence() && actionable {
        return Err(
            "the structured rung answers this window: its accessibility tree exposes actionable \
             controls, and a screenshot is never taken for what a tree can answer (REQ-CUA-001) — \
             act on a named control instead, or narrow the target to a \
             canvas/custom-rendered region"
                .into(),
        );
    }
    Ok(())
}

/// Divide a coordinate by an output scale, defaulting to identity for a
/// non-positive or absent scale.
fn divide(value: i32, scale: f64) -> i32 {
    if scale > 0.0 && (scale - 1.0).abs() > f64::EPSILON {
        (f64::from(value) / scale).round() as i32
    } else {
        value
    }
}

/// The engine's ladder driver: the platform backend attempts a rung, and the
/// **guard** is the only authority.
///
/// Two properties make this the real boundary and not a wrapper:
///
/// - `attempt` never authorizes anything. The only gate on a gated rung is
///   [`ClickLadderDriver::authorize`], and that routes to
///   [`DesktopGuard::authorize_rung`] — the same kill switch, rate limit,
///   policy evaluation, `PermissionGate` (→ Guard ticket store) and audit sink
///   every other desktop effect rides. There is no second path.
/// - the interaction-mode floor is checked **here**, once, so a platform
///   backend added later cannot forget it. The backends keep their own floor as
///   defence in depth; this is the single place that decides.
struct PlatformLadderDriver<'a> {
    backend: &'a platform::PlatformBackend,
    guard: &'a DesktopGuard,
    profile: ClickProfile,
    mode: InteractionMode,
    provenance: ActProvenance,
}

impl<'a> PlatformLadderDriver<'a> {
    fn new(
        backend: &'a platform::PlatformBackend,
        guard: &'a DesktopGuard,
        provenance: ActProvenance,
    ) -> Self {
        let profile = backend.click_profile();
        let mode = guard.policy().interaction_mode;
        Self {
            backend,
            guard,
            profile,
            mode,
            provenance,
        }
    }
}

impl ClickLadderDriver for PlatformLadderDriver<'_> {
    fn profile(&self) -> &ClickProfile {
        &self.profile
    }

    fn attempt(&self, rung: ClickRung, target: &LadderTarget) -> RungDelivery {
        if rung.requires_foreground() && self.mode == InteractionMode::Background {
            // P57.3/P57.4 — the interaction default governs the *posture*; the
            // guard decision governs the *act*. Both must hold for a rung that
            // moves the pointer, and the mode is a setting, not a per-act
            // approval — so the ladder stops here regardless of what the
            // authority said.
            return RungDelivery::Blocked(format!(
                "background contract: rung {} ({}) needs the Foreground interaction default \
                 (Settings → Computer use) — use a named-element action, or approve the \
                 foreground escalation",
                rung.as_str(),
                rung.describe()
            ));
        }
        self.backend.deliver_rung(rung, target, self.mode)
    }

    fn authorize(&self, rung: ClickRung, target: &LadderTarget) -> std::result::Result<(), String> {
        // The rung's authorization rides the **same** provenance the act
        // declared, so a human-gesture cursor takeover is never filed as an
        // agent action on the Merkle chain (or the reverse).
        self.guard
            .authorize_rung(&target.window.app, &target.act, None, self.provenance, rung)
    }
}

/// Bridges the verify cascade to the live engine (re-read + re-OCR).
struct EngineObserver<'a> {
    engine: &'a DesktopEngine,
    window_id: u64,
}

impl<'a> Observer for EngineObserver<'a> {
    fn read_tree(&self, _window_id: u64) -> Option<ReadNode> {
        let window = WindowInfo {
            id: self.window_id,
            title: String::new(),
            app: String::new(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            has_a11y_tree: false,
        };
        self.engine.read(&window).ok()?.tree
    }

    /// `FIX-17` — the engine's read knows whether it covered the target, so the
    /// cascade gets the real confidence instead of guessing it from a tree's
    /// emptiness. This is what stops an elevation-blocked read from being
    /// verified as "the dialog is gone".
    fn read_confidence(&self, _window_id: u64) -> verify::ReadConfidence {
        let window = WindowInfo {
            id: self.window_id,
            title: String::new(),
            app: String::new(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            has_a11y_tree: false,
        };
        match self.engine.read(&window) {
            Ok(read) => verify::ReadConfidence::from_status(&read.status),
            // The read itself failed: nothing was observed at all.
            Err(_) => verify::ReadConfidence::Unknown,
        }
    }

    fn ocr(&self, _window_id: u64) -> Vec<types::OcrWord> {
        let window = WindowInfo {
            id: self.window_id,
            title: String::new(),
            app: String::new(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            has_a11y_tree: false,
        };
        self.engine.ocr_window(&window).unwrap_or_default()
    }
}

/// Re-export the OcrEngine trait methods for `locate_phrase` callers.
pub mod prelude {
    pub use crate::ocr::{OcrEngine, VisionHit, locate_phrase};
    pub use crate::policy::GateDecision;
}

#[cfg(test)]
mod p57_escalation_tests {
    use super::*;
    use crate::policy::{DenyAllGate, NoopSink};
    use crate::uia::UiaReadStatus;

    fn engine(policy: AppPolicy) -> DesktopEngine {
        DesktopEngine::with_guard(
            platform::PlatformBackend::Unsupported,
            DesktopGuard::new(policy, Box::new(DenyAllGate), Box::new(NoopSink)),
            Arc::new(ocr::NoOcr),
        )
    }

    fn window() -> WindowInfo {
        WindowInfo {
            id: 7,
            title: "Editor".into(),
            app: "editor".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            has_a11y_tree: false,
        }
    }

    #[test]
    fn background_act_escalates_and_never_moves_without_a_gesture() {
        let engine = engine(AppPolicy::default()); // Background default
        let request = engine
            .escalation_for(&window(), &ActKind::ActivateWindow { window_id: 7 })
            .expect("activate needs foreground");
        assert!(request.requires_gesture);
        assert!(request.reason.contains("escalate to foreground"));
        assert_eq!(request.target, "editor");
        // Without a gesture the act is refused with the reason, and the
        // interaction default is untouched (nothing silently flipped).
        let outcome = engine
            .act_escalating(
                &window(),
                &ActKind::ActivateWindow { window_id: 7 },
                None,
                false,
            )
            .unwrap();
        assert!(!outcome.ok);
        assert!(outcome.error.unwrap().contains("requires a human gesture"));
        assert_eq!(
            engine.policy().interaction_mode,
            InteractionMode::Background
        );
    }

    #[test]
    fn foreground_default_needs_no_escalation() {
        let engine = engine(AppPolicy {
            interaction_mode: InteractionMode::Foreground,
            ..AppPolicy::default()
        });
        assert!(
            engine
                .escalation_for(&window(), &ActKind::ActivateWindow { window_id: 7 })
                .is_none()
        );
    }

    #[test]
    fn scroll_and_drag_are_pointer_motion_by_definition() {
        let engine = engine(AppPolicy::default());
        assert!(
            engine
                .escalation_for(
                    &window(),
                    &ActKind::Scroll {
                        x: 1,
                        y: 2,
                        delta: -1
                    }
                )
                .is_some()
        );
        assert!(
            engine
                .escalation_for(
                    &window(),
                    &ActKind::Drag {
                        from: (0, 0),
                        to: (5, 5),
                    }
                )
                .is_some()
        );
        // Typing into a background window is not a foreground act.
        assert!(
            engine
                .escalation_for(&window(), &ActKind::Type { text: "hi".into() })
                .is_none()
        );
    }

    #[test]
    fn snapshot_without_a_platform_id_restores_as_a_noop() {
        let engine = engine(AppPolicy::default());
        let snapshot = engine.foreground_snapshot();
        // `PlatformBackend::Unsupported` cannot name a foreground window.
        assert_eq!(snapshot.window_id, None);
        // A no-op restore is a success, not a fabricated one.
        assert!(engine.restore_foreground(&snapshot).is_ok());
    }

    /// `REQ-CUA-001` / `REQ-CUA-008` — the structured-first decision, pinned per
    /// read status. This is the discipline the ladder's capture path turns on, and
    /// it is pure so every state is covered without a desktop.
    #[test]
    fn the_vision_rung_runs_last_and_refuses_when_structure_answers() {
        let w = window();
        let action = |path: &str| ReadNode {
            index_path: path.into(),
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
        let tree = |children: Vec<ReadNode>| ReadNode {
            index_path: "1".into(),
            role: "Pane".into(),
            name: "Editor".into(),
            automation_id: None,
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            actionable: false,
            children,
        };

        // 1. A conclusive read with an actionable control: structure answers it.
        let conclusive = ReadResult {
            tree: Some(tree(vec![action("1.1")])),
            status: UiaReadStatus::Complete,
            ..ReadResult::absent(w.id, 1.0, vec![])
        };
        let err = vision_rung_verdict(&conclusive).unwrap_err();
        assert!(err.contains("REQ-CUA-001"), "{err}");

        // 2. A read that could not look: refused, and the reason is the read's.
        let blocked = ReadResult {
            status: UiaReadStatus::Unknown {
                detail: "elevated".into(),
            },
            guidance: Some("no input is synthesized into it".into()),
            ..ReadResult::absent(w.id, 1.0, vec![])
        };
        let err = vision_rung_verdict(&blocked).unwrap_err();
        assert!(err.contains("no input is synthesized"), "{err}");

        // 3. An absent read — no structural UI: this is the vision-rung case.
        assert!(vision_rung_verdict(&ReadResult::absent(w.id, 1.0, vec![])).is_ok());

        // 4. A complete read with nothing actionable (a canvas, custom rendering):
        //    vision is allowed, because structure does not answer.
        let canvas = ReadResult {
            tree: Some(tree(vec![ReadNode {
                index_path: "1.1".into(),
                role: "Image".into(),
                name: String::new(),
                automation_id: None,
                x: 0,
                y: 0,
                width: 10,
                height: 10,
                actionable: false,
                children: vec![],
            }])),
            status: UiaReadStatus::Complete,
            ..ReadResult::absent(w.id, 1.0, vec![])
        };
        assert!(vision_rung_verdict(&canvas).is_ok());

        // 5. A partial read with an actionable control: still refused, because a
        //    conclusive answer is not required to notice that structure answered.
        let partial = ReadResult {
            tree: Some(tree(vec![action("1.1")])),
            status: UiaReadStatus::Partial {
                detail: "stopped at a bound".into(),
            },
            ..ReadResult::absent(w.id, 1.0, vec![])
        };
        assert!(vision_rung_verdict(&partial).is_ok());
    }

    /// With no platform backend at all, the rung is refused with a typed reason
    /// rather than attempting a capture — and nothing is synthesized.
    #[test]
    fn an_unattached_host_refuses_the_vision_rung_with_a_reason() {
        let engine = engine(AppPolicy::default());
        let err = engine
            .vision_rung_advisable(&window())
            .expect_err("an unattached host cannot capture");
        assert!(err.contains("structured read failed"), "{err}");
        let outcome = engine
            .vision_click(&window(), "Save")
            .expect("the refusal is an outcome, not a panic");
        assert!(!outcome.ok);
        assert!(outcome.click.is_none());
    }
}
