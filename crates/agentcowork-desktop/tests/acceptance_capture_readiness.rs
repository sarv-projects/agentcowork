//! `FIX-18` / `TASK-CUA-002` — capture readiness, exercised with fake probes on
//! **every** host.
//!
//! The real WGC/D3D11 probe is `#[cfg(windows)]` and cannot run here, so what
//! these tests prove is the readiness *rule* — that a capture attempt is preceded
//! by a verdict, that a verdict is typed and actionable, and that a degrade is a
//! recorded fact rather than a silent fall-through. Whether WinRT answers as the
//! Windows client expects is what a Windows acceptance record has to show.

use std::sync::Mutex;

use agentcowork_computeruse::capture::{
    CaptureCheck, CaptureDegrade, CaptureFault, CapturePipeline, CaptureProbe, CaptureReadiness,
    CaptureState, HOST_PROBE_TTL, HostProbe, NoCaptureProbe, SharedHostProbe, verify_capture,
};
use agentcowork_computeruse::types::{SeeMethod, WindowInfo};

fn window() -> WindowInfo {
    WindowInfo {
        id: 42,
        title: "Editor".into(),
        app: "notepad".into(),
        x: 0,
        y: 0,
        width: 800,
        height: 600,
        has_a11y_tree: true,
    }
}

/// A scripted probe: per-pipeline target verdicts, a host verdict, and a call log
/// so a test can assert *what was verified before what*.
struct FakeProbe {
    pipelines: Vec<CapturePipeline>,
    host: Result<Vec<CaptureCheck>, CaptureFault>,
    /// pipeline → target verdict.
    targets: Vec<(CapturePipeline, Result<Vec<CaptureCheck>, CaptureFault>)>,
    occluded: bool,
    calls: Mutex<Vec<String>>,
}

impl FakeProbe {
    fn new(pipelines: Vec<CapturePipeline>) -> Self {
        Self {
            pipelines,
            host: Ok(vec![CaptureCheck::SessionAvailable]),
            targets: Vec::new(),
            occluded: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn host_fault(mut self, fault: CaptureFault) -> Self {
        self.host = Err(fault);
        self
    }

    fn target(
        mut self,
        pipeline: CapturePipeline,
        verdict: Result<Vec<CaptureCheck>, CaptureFault>,
    ) -> Self {
        self.targets.push((pipeline, verdict));
        self
    }

    fn occluded(mut self) -> Self {
        self.occluded = true;
        self
    }

    fn log(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

impl CaptureProbe for FakeProbe {
    fn pipelines(&self) -> Vec<CapturePipeline> {
        self.pipelines.clone()
    }

    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault> {
        self.calls
            .lock()
            .map(|mut c| {
                c.push("host".into());
            })
            .ok();
        self.host.clone()
    }

    fn is_occluded(&mut self, _window: &WindowInfo) -> bool {
        self.occluded
    }

    fn target(
        &mut self,
        _window: &WindowInfo,
        pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault> {
        self.calls
            .lock()
            .map(|mut c| {
                c.push(format!("target:{}", pipeline.as_str()));
            })
            .ok();
        self.targets
            .iter()
            .find(|(p, _)| *p == pipeline)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| Ok(vec![CaptureCheck::PipelineSupported]))
    }
}

fn ok(checks: Vec<CaptureCheck>) -> Result<Vec<CaptureCheck>, CaptureFault> {
    Ok(checks)
}

/// A host where graphics capture is unavailable falls back to `PrintWindow` and
/// says so — the decision is recorded, not silent.
#[test]
fn a_degraded_capture_names_the_skipped_pipeline_and_the_reason() {
    let mut probe = FakeProbe::new(vec![
        CapturePipeline::WindowsGraphicsCapture,
        CapturePipeline::PrintWindow,
    ])
    .target(
        CapturePipeline::WindowsGraphicsCapture,
        Err(CaptureFault::SessionUnsupported {
            detail: "GraphicsCaptureSession::IsSupported() is false".into(),
        }),
    )
    .target(
        CapturePipeline::PrintWindow,
        ok(vec![
            CaptureCheck::WindowHandleValid,
            CaptureCheck::NonZeroExtent,
        ]),
    );
    let readiness = verify_capture(&mut probe, &window());

    assert_eq!(readiness.state, CaptureState::Degraded);
    assert!(readiness.is_capturable());
    assert!(readiness.is_degraded());
    assert_eq!(readiness.pipeline, Some(CapturePipeline::PrintWindow));
    assert_eq!(
        readiness.degraded_from,
        Some(CapturePipeline::WindowsGraphicsCapture)
    );
    assert_eq!(readiness.failed_check, Some(CaptureCheck::SessionAvailable));
    let fault = readiness.fault.clone().expect("a recorded fault");
    assert!(matches!(fault, CaptureFault::SessionUnsupported { .. }));
    // The guidance is a sentence with the remedy in it, not an error code.
    assert!(
        readiness.guidance.contains("Windows.Graphics.Capture"),
        "{readiness:?}"
    );
    assert!(readiness.guidance.contains("PrintWindow"), "{readiness:?}");
    // The evidence travels: the checks that did pass are named.
    assert!(
        readiness
            .verified
            .contains(&CaptureCheck::WindowHandleValid)
    );
    assert!(readiness.verified.contains(&CaptureCheck::NonZeroExtent));
    assert!(readiness.verified_at_ms > 0);

    // And the highest-fidelity usable pipeline is the one named, verified first.
    assert_eq!(
        probe.log(),
        vec![
            "host".to_string(),
            "target:windows_graphics_capture".to_string(),
            "target:print_window".to_string(),
        ],
        "the host is checked before any per-target probe, and the best pipeline is tried first"
    );
}

/// A target no pipeline can capture is `unavailable`: no capture is attempted and
/// the refusal is typed, actionable and names the failing check.
#[test]
fn an_unavailable_target_is_refused_before_any_capture_is_attempted() {
    let mut probe = FakeProbe::new(vec![CapturePipeline::X11GetImage]).target(
        CapturePipeline::X11GetImage,
        Err(CaptureFault::ZeroExtent {
            detail: "the window is 0x0".into(),
        }),
    );
    let readiness = verify_capture(&mut probe, &window());
    assert_eq!(readiness.state, CaptureState::Unavailable);
    assert!(!readiness.is_capturable());
    assert_eq!(readiness.pipeline, None);
    assert_eq!(readiness.failed_check, Some(CaptureCheck::NonZeroExtent));
    assert!(readiness.guidance.contains("restore it"), "{readiness:?}");
    assert!(readiness.verified_at_ms > 0);
    // No fault is invented: the verdict carries the one that happened.
    assert!(matches!(
        readiness.fault,
        Some(CaptureFault::ZeroExtent { .. })
    ));
}

/// A closed window says so, with the remedy (re-list), instead of returning an
/// empty capture.
#[test]
fn a_dead_window_handle_names_the_remedy() {
    let mut probe = FakeProbe::new(vec![
        CapturePipeline::WindowsGraphicsCapture,
        CapturePipeline::PrintWindow,
    ])
    .target(
        CapturePipeline::WindowsGraphicsCapture,
        Err(CaptureFault::InvalidWindowHandle {
            detail: "IsWindow is false".into(),
        }),
    )
    .target(
        CapturePipeline::PrintWindow,
        Err(CaptureFault::InvalidWindowHandle {
            detail: "IsWindow is false".into(),
        }),
    );
    let readiness = verify_capture(&mut probe, &window());
    assert_eq!(readiness.state, CaptureState::Unavailable);
    assert!(
        readiness.guidance.contains("re-list the windows"),
        "{readiness:?}"
    );
    assert_eq!(
        readiness.failed_check,
        Some(CaptureCheck::WindowHandleValid)
    );
    assert!(
        !CaptureFault::InvalidWindowHandle {
            detail: String::new()
        }
        .has_fallback()
    );
}

/// Session 0 has no desktop to capture: no probe of a per-target pipeline even
/// runs, and the refusal names the session.
#[test]
fn a_non_interactive_session_refuses_before_probing_any_target() {
    let mut probe = FakeProbe::new(vec![
        CapturePipeline::WindowsGraphicsCapture,
        CapturePipeline::PrintWindow,
    ])
    .host_fault(CaptureFault::NoInteractiveSession {
        detail: "SESSIONNAME=Services".into(),
    });
    let readiness = verify_capture(&mut probe, &window());
    assert_eq!(readiness.state, CaptureState::Unavailable);
    assert_eq!(readiness.failed_check, Some(CaptureCheck::SessionAvailable));
    assert!(
        readiness.guidance.contains("user's session"),
        "{readiness:?}"
    );
}

/// The screen-only pipeline is the interesting case: a capture of an occluded
/// window through the screen DC would be a capture of the **occluder**, so the
/// verdict refuses rather than degrading into it.
#[test]
fn an_occluded_window_on_a_screen_only_pipeline_is_refused_not_degraded() {
    let mut probe = FakeProbe::new(vec![CapturePipeline::ScreenDc])
        .target(
            CapturePipeline::ScreenDc,
            Err(CaptureFault::CaptureItemUnavailable {
                detail: "no capture item for the target".into(),
            }),
        )
        .occluded();
    let readiness = verify_capture(&mut probe, &window());
    assert_eq!(readiness.state, CaptureState::Unavailable);
    assert_eq!(readiness.pipeline, None);
    assert!(matches!(
        readiness.fault,
        Some(CaptureFault::OccludedAndScreenOnly { .. })
    ));
    assert!(
        readiness.guidance.contains("occluding window"),
        "{readiness:?}"
    );
}

/// A platform with no pipeline at all is `unavailable` with a sentence, not a
/// green dot (an unattached host included).
#[test]
fn no_pipeline_is_unavailable_with_guidance() {
    let readiness = verify_capture(&mut NoCaptureProbe, &window());
    assert_eq!(readiness.state, CaptureState::Unavailable);
    assert!(!readiness.guidance.is_empty());
    assert!(
        readiness.guidance.contains("no window-capture pipeline"),
        "{readiness:?}"
    );

    let json = readiness.to_json();
    assert_eq!(json["state"], "unavailable");
    assert!(json["guidance"].as_str().unwrap().len() > 10);
}

/// The ready path names the pipeline and lists what was verified — evidence, not
/// a claim.
#[test]
fn a_ready_verdict_lists_the_checks_that_passed() {
    let mut probe = FakeProbe::new(vec![CapturePipeline::WindowsGraphicsCapture]).target(
        CapturePipeline::WindowsGraphicsCapture,
        ok(vec![
            CaptureCheck::WindowHandleValid,
            CaptureCheck::NonZeroExtent,
            CaptureCheck::CaptureItem,
        ]),
    );
    let readiness = verify_capture(&mut probe, &window());
    assert_eq!(readiness.state, CaptureState::Ready);
    assert!(!readiness.is_degraded());
    assert_eq!(
        readiness.pipeline,
        Some(CapturePipeline::WindowsGraphicsCapture)
    );
    assert!(readiness.fault.is_none());
    assert!(readiness.failed_check.is_none());
    assert!(
        readiness.guidance.contains("compositor-native"),
        "{readiness:?}"
    );
    for check in [
        CaptureCheck::SessionAvailable,
        CaptureCheck::WindowHandleValid,
        CaptureCheck::NonZeroExtent,
        CaptureCheck::CaptureItem,
    ] {
        assert!(readiness.verified.contains(&check), "missing {check:?}");
    }
}

/// The occlusion-independent property is what the ladder's vision discipline turns
/// on, so it is pinned per pipeline.
#[test]
fn only_the_window_compositing_pipelines_are_occlusion_independent() {
    assert!(CapturePipeline::WindowsGraphicsCapture.is_occlusion_independent());
    assert!(CapturePipeline::PrintWindow.is_occlusion_independent());
    assert!(CapturePipeline::X11GetImage.is_occlusion_independent());
    assert!(CapturePipeline::MacScreenCapture.is_occlusion_independent());
    // The screen DC is the one that returns the occluder.
    assert!(!CapturePipeline::ScreenDc.is_occlusion_independent());
    assert!(CapturePipeline::ScreenDc.describe().contains("occluder"));
    assert!(CapturePipeline::WindowsGraphicsCapture.is_compositor_native());
    assert!(!CapturePipeline::PrintWindow.is_compositor_native());
}

/// The pipeline and the `SeeMethod` a capture reports cannot drift apart.
#[test]
fn each_pipeline_reports_its_own_see_method() {
    for (pipeline, method) in [
        (
            CapturePipeline::WindowsGraphicsCapture,
            SeeMethod::WindowsGraphicsCapture,
        ),
        (CapturePipeline::PrintWindow, SeeMethod::PrintWindow),
        (CapturePipeline::ScreenDc, SeeMethod::ScreenDc),
        (CapturePipeline::X11GetImage, SeeMethod::X11GetImage),
        (
            CapturePipeline::MacScreenCapture,
            SeeMethod::MacScreenCapture,
        ),
    ] {
        assert_eq!(pipeline.see_method(), method, "{}", pipeline.as_str());
    }
}

/// The degrade record that travels with a capture names the skipped pipeline, the
/// reason, and whether the vision rung asked for it (`ARCH/24` §7: a screenshot
/// enters context only when the vision rung ran).
#[test]
fn the_degrade_record_names_the_skip_and_the_vision_rung() {
    let mut probe = FakeProbe::new(vec![
        CapturePipeline::WindowsGraphicsCapture,
        CapturePipeline::PrintWindow,
    ])
    .target(
        CapturePipeline::WindowsGraphicsCapture,
        Err(CaptureFault::NoGraphicsDevice {
            detail: "no BGRA device".into(),
        }),
    )
    .target(
        CapturePipeline::PrintWindow,
        ok(vec![CaptureCheck::NonZeroExtent]),
    );
    let readiness = verify_capture(&mut probe, &window());

    let degrade = CaptureDegrade {
        readiness: readiness.clone(),
        method: SeeMethod::PrintWindow,
        skipped: readiness.degraded_from,
        reason: readiness.fault.as_ref().map(|f| f.guidance()),
        vision_rung: true,
    };
    assert!(degrade.is_degraded());
    let line = degrade.describe();
    assert!(line.contains("PrintWindow"), "{line}");
    assert!(
        line.contains("degraded from windows_graphics_capture"),
        "{line}"
    );
    assert!(!line.contains("outside the vision rung"), "{line}");

    // A capture taken outside the vision rung says so on its own record.
    let outside = CaptureDegrade {
        vision_rung: false,
        ..degrade
    };
    assert!(
        outside.describe().contains("outside the vision rung"),
        "{}",
        outside.describe()
    );
}

/// The host probe is cached with its **reason**, and the TTL re-runs it — the
/// `FIX-18` difference from the old `OnceLock<bool>`.
#[test]
fn the_host_probe_is_cached_with_its_reason_and_expires() {
    let mut probe = HostProbe::new(Err(CaptureFault::NoCompositor {
        detail: "dwm is off".into(),
    }));
    // Served from the cache within the TTL.
    let cached = probe.get_or_probe(|| panic!("must not re-probe inside the TTL"));
    assert!(matches!(cached, Err(CaptureFault::NoCompositor { .. })));
    // Past the TTL it re-runs, so a host that changed state is re-measured.
    probe.at = std::time::Instant::now() - (HOST_PROBE_TTL + std::time::Duration::from_secs(1));
    let fresh = probe.get_or_probe(|| Ok(vec![CaptureCheck::CompositorRunning]));
    assert!(fresh.is_ok());

    // The shared probe computes once, and can be invalidated.
    let shared = SharedHostProbe::new();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    for _ in 0..3 {
        let v = shared.get_or_probe(|| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![CaptureCheck::SessionAvailable])
        });
        assert!(v.is_ok());
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    shared.invalidate();
    let _ = shared.get_or_probe(|| {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![CaptureCheck::SessionAvailable])
    });
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// A fault is either target-scoped (another pipeline may still deliver) or
/// host-scoped, and the classification decides whether a fallback exists.
#[test]
fn fault_scope_decides_whether_a_fallback_exists() {
    for target_scoped in [
        CaptureFault::InvalidWindowHandle {
            detail: String::new(),
        },
        CaptureFault::ZeroExtent {
            detail: String::new(),
        },
    ] {
        assert!(!target_scoped.has_fallback(), "{target_scoped:?}");
        assert!(
            target_scoped.check().is_target_scoped(),
            "{target_scoped:?}"
        );
    }
    // Host-scoped faults with no fallback: no pipeline of any kind can help.
    for host_scoped in [
        CaptureFault::NoInteractiveSession {
            detail: String::new(),
        },
        CaptureFault::PipelineUnsupported {
            detail: String::new(),
        },
    ] {
        assert!(!host_scoped.has_fallback(), "{host_scoped:?}");
        assert!(!host_scoped.check().is_target_scoped(), "{host_scoped:?}");
    }
    for fallbackable in [
        CaptureFault::SessionUnsupported {
            detail: String::new(),
        },
        CaptureFault::NoGraphicsDevice {
            detail: String::new(),
        },
        CaptureFault::CaptureItemUnavailable {
            detail: String::new(),
        },
        CaptureFault::ConsentMissing {
            detail: String::new(),
        },
    ] {
        assert!(fallbackable.has_fallback(), "{fallbackable:?}");
    }
    // Every fault names a check and produces a sentence.
    for fault in [
        CaptureFault::NoInteractiveSession { detail: "d".into() },
        CaptureFault::SessionUnsupported { detail: "d".into() },
        CaptureFault::NoGraphicsDevice { detail: "d".into() },
        CaptureFault::NoCompositor { detail: "d".into() },
        CaptureFault::PipelineUnsupported { detail: "d".into() },
        CaptureFault::InvalidWindowHandle { detail: "d".into() },
        CaptureFault::ZeroExtent { detail: "d".into() },
        CaptureFault::CaptureItemUnavailable { detail: "d".into() },
        CaptureFault::ConsentMissing { detail: "d".into() },
        CaptureFault::OccludedAndScreenOnly { detail: "d".into() },
        CaptureFault::ProbeFailed { detail: "d".into() },
    ] {
        assert!(!fault.guidance().is_empty(), "{fault:?}");
        assert!(fault.guidance().contains('d'), "{fault:?}");
    }
}

/// The typed error a refusal produces carries the state, the guidance and the
/// failing check — so a caller never has to parse a string to learn what happened.
#[test]
fn the_typed_error_carries_the_state_the_guidance_and_the_check() {
    let readiness = CaptureReadiness::unavailable(CaptureFault::ZeroExtent {
        detail: "0x0".into(),
    });
    let rendered = format!(
        "{}",
        agentcowork_computeruse::DesktopError::CaptureNotReady {
            state: readiness.state.as_str(),
            guidance: readiness.guidance.clone(),
            failed_check: readiness
                .failed_check
                .map(|c| c.as_str())
                .unwrap_or("unknown"),
        }
    );
    assert!(
        rendered.starts_with("capture not ready (unavailable):"),
        "{rendered}"
    );
    assert!(rendered.contains("restore it first"), "{rendered}");
}
