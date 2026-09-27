//! P57.8 — H4 driver readiness as **derived state**, never a binary claim.
//!
//! The spec's rule: Computer Use reports ● Ready · ⚠ Permission required ·
//! ⚠ Driver missing · ⚠ App unsupported · ✕ Blocked by policy, and the honest
//! failure is a sentence ("I can see the desktop but this app's accessibility
//! tree is unavailable"), not a green dot that means nothing. This module is
//! that derivation, pure over the facts the host already has: the backend's
//! capability surface, the policy, the kill switch, and the attach error when
//! there is one.
//!
//! It is deliberately *not* a probe: nothing here touches the desktop, so the
//! Settings chip cannot itself steal focus or permission-prompt.

use crate::policy::{AppPolicy, InteractionMode};
use crate::types::Capabilities;

/// The five states from the spec (H4), in the order they are derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessState {
    /// The driver can see, read and act on this host.
    Ready,
    /// The driver is present but cannot act without a grant (macOS TCC
    /// Accessibility / Screen Recording, Windows UIAccess, Linux AT-SPI/XTEST).
    PermissionRequired,
    /// No usable backend on this host (headless, Session 0, no display).
    DriverMissing,
    /// The backend works, but this host exposes no way to read an app
    /// (no accessibility tree and no OCR fallback).
    AppUnsupported,
    /// Policy refuses: emergency stop engaged, or strict mode with nothing
    /// allow-listed (so every app is denied).
    PolicyBlocked,
}

impl ReadinessState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReadinessState::Ready => "ready",
            ReadinessState::PermissionRequired => "permission_required",
            ReadinessState::DriverMissing => "driver_missing",
            ReadinessState::AppUnsupported => "app_unsupported",
            ReadinessState::PolicyBlocked => "policy_blocked",
        }
    }

    /// The one-glyph badge the UI shows (never a bare colour: the label is
    /// always rendered next to it).
    pub fn glyph(&self) -> &'static str {
        match self {
            ReadinessState::Ready => "\u{25cf}",
            ReadinessState::PermissionRequired => "\u{26a0}",
            ReadinessState::DriverMissing => "\u{26a0}",
            ReadinessState::AppUnsupported => "\u{26a0}",
            ReadinessState::PolicyBlocked => "\u{2715}",
        }
    }
}

/// The derived readiness + the sentence that explains it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Readiness {
    pub state: ReadinessState,
    pub detail: String,
    /// True only for `Ready` — callers can gate on a single boolean without
    /// re-deriving, but the sentence travels with it so nothing can paint a
    /// green state without a reason.
    pub usable: bool,
}

/// Derive readiness from the host's facts.
///
/// Precedence (first match wins), documented because the chip's honesty
/// depends on it:
/// 1. no capabilities at all → **Driver missing** (the attach error is shown),
/// 2. emergency stop engaged → **Policy blocked**,
/// 3. strict mode with an empty allow-list (names *and* paths) → **Policy
///    blocked** — nothing could ever be allowed,
/// 4. no way to act (no accessibility invoke and no input synthesis) →
///    **Permission required**,
/// 5. no way to read (no accessibility tree and no OCR) → **App unsupported**,
/// 6. otherwise → **Ready**, with the background-mode limitation appended to the
///    sentence when the host has no non-moving click path (P57.4) — so the
///    first failed coordinate click is never the user's introduction to it.
pub fn derive(
    caps: Option<&Capabilities>,
    policy: &AppPolicy,
    kill_stopped: bool,
    attach_error: Option<&str>,
) -> Readiness {
    let Some(caps) = caps else {
        return Readiness {
            state: ReadinessState::DriverMissing,
            detail: attach_error
                .filter(|e| !e.is_empty())
                .unwrap_or("the desktop driver is not attached on this host")
                .to_string(),
            usable: false,
        };
    };
    // P57.5 — Session 0 (Windows services) has no interactive desktop to see or
    // drive; saying "driver missing" is the truth, not an empty window list.
    if !caps.interactive_desktop {
        return Readiness {
            state: ReadinessState::DriverMissing,
            detail: "this process is in a non-interactive session (Session 0) — there is no \
                     desktop to see or drive"
                .into(),
            usable: false,
        };
    }
    if kill_stopped {
        return Readiness {
            state: ReadinessState::PolicyBlocked,
            detail: "emergency stop engaged — every desktop op fails closed until it is resumed"
                .into(),
            usable: false,
        };
    }
    if policy.strict && policy.allow_list.is_empty() && policy.allow_paths.is_empty() {
        return Readiness {
            state: ReadinessState::PolicyBlocked,
            detail: "strict mode with an empty allow-list blocks every app — allow-list one first"
                .into(),
            usable: false,
        };
    }
    if !caps.send_input && !caps.invoke_set_value {
        return Readiness {
            state: ReadinessState::PermissionRequired,
            detail: "the driver cannot act on this host — grant input/accessibility permission"
                .into(),
            usable: false,
        };
    }
    // P57.5 — macOS TCC (Screen Recording / Accessibility). Naming the exact
    // consent is the only way the sentence is actionable; the state is derived
    // from the OS, never assumed.
    let mut missing_consent: Vec<&str> = Vec::new();
    if !caps.screen_recording_granted {
        missing_consent.push("Screen Recording");
    }
    if !caps.accessibility_granted {
        missing_consent.push("Accessibility");
    }
    if !missing_consent.is_empty() {
        return Readiness {
            state: ReadinessState::PermissionRequired,
            detail: format!(
                "macOS consent missing: {} — grant it in System Settings → Privacy & Security",
                missing_consent.join(" + ")
            ),
            usable: false,
        };
    }
    if !caps.uia_tree && !caps.ocr {
        return Readiness {
            state: ReadinessState::AppUnsupported,
            detail: "no accessibility tree and no OCR fallback — app contents would read empty"
                .into(),
            usable: false,
        };
    }
    let mut parts: Vec<&str> = Vec::new();
    if caps.uia_tree {
        parts.push("accessibility tree");
    }
    if caps.send_input {
        parts.push("input synthesis");
    }
    if caps.invoke_set_value {
        parts.push("invoke/set-value");
    }
    if caps.ocr {
        parts.push("OCR fallback");
    }
    if caps.window_list {
        parts.push("window listing");
    }
    let mut detail = format!("{} available on this host", parts.join(" + "));
    // P57.4 — the honest caveat for the default that is actually selected: in
    // Background mode a coordinate click needs a non-moving path; when the host
    // has none, say so up front instead of letting the first click fail.
    if policy.interaction_mode == InteractionMode::Background
        && !caps.background_input
        && !caps.uia_tree
    {
        detail.push_str(
            "; background mode cannot deliver a coordinate click on this host \
             (no non-moving click path) — named/AX clicks work, or switch the \
             interaction default to Foreground",
        );
    }
    Readiness {
        state: ReadinessState::Ready,
        detail,
        usable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SeeMethod;

    fn caps(uia: bool, send: bool, invoke: bool, ocr: bool) -> Capabilities {
        Capabilities {
            see: SeeMethod::X11GetImage,
            see_occluded: false,
            uia_tree: uia,
            invoke_set_value: invoke,
            send_input: send,
            // Models the Linux host these tests describe: XTEST for the
            // Foreground path plus a synthetic-click path for the default.
            background_input: true,
            foreground_restore: true,
            a11y_action: false,
            see_occluded_wgc: false,
            interactive_desktop: true,
            screen_recording_granted: true,
            accessibility_granted: true,
            ocr,
            window_list: true,
            launch_app: true,
            // `FIX-18` — the capture chip is a separate fact from the read
            // surface; these tests are about the read surface.
            capture_readiness: crate::capture::CaptureReadiness::ready(
                crate::capture::CapturePipeline::X11GetImage,
                vec![crate::capture::CaptureCheck::PipelineSupported],
            )
            .into(),
        }
    }

    /// P57.4 — the Background default on a host with no non-moving click path is
    /// reported in the sentence, not hidden until the first click fails.
    #[test]
    fn background_default_without_a_non_moving_click_path_says_so() {
        let c = Capabilities {
            see: SeeMethod::MacScreenCapture,
            see_occluded: false,
            uia_tree: false,
            invoke_set_value: false,
            send_input: true,
            background_input: false,
            // macOS-shaped: no restorable foreground id, no AX-by-point action.
            foreground_restore: false,
            a11y_action: false,
            see_occluded_wgc: false,
            interactive_desktop: true,
            screen_recording_granted: true,
            accessibility_granted: true,
            ocr: true,
            window_list: true,
            launch_app: true,
            // `FIX-18` — a screen-only pipeline: ready when the session is, and
            // the verdict would refuse an occluded target rather than capture the
            // occluder.
            capture_readiness: crate::capture::CaptureReadiness::ready(
                crate::capture::CapturePipeline::MacScreenCapture,
                vec![crate::capture::CaptureCheck::PipelineSupported],
            )
            .into(),
        };
        let background = derive(Some(&c), &AppPolicy::default(), false, None);
        assert_eq!(background.state, ReadinessState::Ready);
        assert!(background.detail.contains("background mode cannot deliver"));

        // The same host on the Foreground default has no such limitation.
        let foreground = AppPolicy {
            interaction_mode: InteractionMode::Foreground,
            ..AppPolicy::default()
        };
        let r = derive(Some(&c), &foreground, false, None);
        assert!(!r.detail.contains("background mode cannot deliver"));
    }

    /// A Linux host with a synthetic-click path keeps its clean sentence.
    #[test]
    fn background_input_keeps_the_sentence_clean() {
        let r = derive(
            Some(&caps(false, true, false, true)),
            &AppPolicy::default(),
            false,
            None,
        );
        assert_eq!(r.state, ReadinessState::Ready);
        assert!(!r.detail.contains("cannot deliver"));
    }

    #[test]
    fn missing_driver_reports_the_attach_error_verbatim() {
        let r = derive(
            None,
            &AppPolicy::default(),
            false,
            Some("no display on this host"),
        );
        assert_eq!(r.state, ReadinessState::DriverMissing);
        assert!(r.detail.contains("no display"));
        assert!(!r.usable);
        // No error recorded still yields a sentence, not an empty chip.
        let r2 = derive(None, &AppPolicy::default(), false, None);
        assert_eq!(r2.state, ReadinessState::DriverMissing);
        assert!(!r2.detail.is_empty());
    }

    #[test]
    fn kill_switch_and_empty_strict_policy_are_policy_blocked() {
        let c = caps(true, true, false, false);
        let killed = derive(Some(&c), &AppPolicy::default(), true, None);
        assert_eq!(killed.state, ReadinessState::PolicyBlocked);
        assert!(killed.detail.contains("emergency stop"));

        let strict = AppPolicy {
            strict: true,
            ..AppPolicy::default()
        };
        assert_eq!(
            derive(Some(&c), &strict, false, None).state,
            ReadinessState::PolicyBlocked
        );
        // One allow-listed path is enough for strict mode to be usable.
        let mut listed = strict.clone();
        listed.allow_paths.push("/usr/bin/gedit".into());
        assert!(derive(Some(&c), &listed, false, None).usable);
    }

    #[test]
    fn no_act_path_is_permission_required() {
        let r = derive(
            Some(&caps(true, false, false, true)),
            &AppPolicy::default(),
            false,
            None,
        );
        assert_eq!(r.state, ReadinessState::PermissionRequired);
        assert!(!r.usable);
    }

    #[test]
    fn no_read_path_is_app_unsupported() {
        let r = derive(
            Some(&caps(false, true, false, false)),
            &AppPolicy::default(),
            false,
            None,
        );
        assert_eq!(r.state, ReadinessState::AppUnsupported);
        assert!(r.detail.contains("accessibility tree"));
    }

    #[test]
    fn ready_names_what_is_actually_available() {
        let r = derive(
            Some(&caps(true, true, true, true)),
            &AppPolicy::default(),
            false,
            None,
        );
        assert_eq!(r.state, ReadinessState::Ready);
        assert!(r.usable);
        assert_eq!(r.state.glyph(), "\u{25cf}");
        assert!(r.detail.contains("accessibility tree"));
        assert!(r.detail.contains("OCR fallback"));
        // A capability surface with no OCR still reads Ready when the tree and
        // an act path exist — the detail lists only what is there.
        let r2 = derive(
            Some(&caps(true, true, false, false)),
            &AppPolicy::default(),
            false,
            None,
        );
        assert!(r2.usable);
        assert!(!r2.detail.contains("OCR"));
    }
}
