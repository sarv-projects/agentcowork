//! The DWM compositor step of the Windows capture-readiness chain, exercised
//! with a fake probe on **every** host.
//!
//! The raw `DwmIsCompositionEnabled` call is Windows-only (see
//! `platform/win.rs`, unit-tested there behind `#[cfg(windows)]`), so what
//! these tests prove is the verdict *shape* the new step plugs into: a host
//! that reports `CompositorRunning` carries it in `verified`, and a host whose
//! composition is off surfaces the typed `NoCompositor` fault with its
//! guidance — degrading to `PrintWindow`, never a silent fall-through.

use agentcowork_computeruse::capture::{
    CaptureCheck, CaptureFault, CapturePipeline, CaptureProbe, CaptureReadiness, CaptureState,
    verify_capture,
};
use agentcowork_computeruse::types::WindowInfo;

fn window() -> WindowInfo {
    WindowInfo {
        id: 7,
        title: "Editor".into(),
        app: "notepad".into(),
        x: 0,
        y: 0,
        width: 800,
        height: 600,
        has_a11y_tree: true,
    }
}

/// A fake of the Windows host chain: session, graphics device, compositor.
struct DwmProbe {
    host: Result<Vec<CaptureCheck>, CaptureFault>,
}

impl CaptureProbe for DwmProbe {
    fn pipelines(&self) -> Vec<CapturePipeline> {
        vec![
            CapturePipeline::WindowsGraphicsCapture,
            CapturePipeline::PrintWindow,
            CapturePipeline::ScreenDc,
        ]
    }

    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault> {
        self.host.clone()
    }

    fn is_occluded(&mut self, _window: &WindowInfo) -> bool {
        false
    }

    fn target(
        &mut self,
        _window: &WindowInfo,
        pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault> {
        match pipeline {
            CapturePipeline::WindowsGraphicsCapture => Ok(vec![
                CaptureCheck::WindowHandleValid,
                CaptureCheck::NonZeroExtent,
                CaptureCheck::CaptureItem,
            ]),
            _ => Ok(vec![
                CaptureCheck::WindowHandleValid,
                CaptureCheck::NonZeroExtent,
            ]),
        }
    }
}

/// A compositor that is on travels in the verdict as the named check.
#[test]
fn a_running_compositor_is_reported_as_its_own_named_check() {
    let mut probe = DwmProbe {
        host: Ok(vec![
            CaptureCheck::SessionAvailable,
            CaptureCheck::GraphicsDevice,
            CaptureCheck::CompositorRunning,
        ]),
    };
    let readiness = verify_capture(&mut probe, &window());
    assert_eq!(readiness.state, CaptureState::Ready);
    assert_eq!(
        readiness.pipeline,
        Some(CapturePipeline::WindowsGraphicsCapture)
    );
    assert!(
        readiness
            .verified
            .contains(&CaptureCheck::CompositorRunning),
        "{readiness:?}"
    );
    assert!(readiness.fault.is_none());
}

/// Composition off is the typed `NoCompositor` fault with its guidance —
///
/// recorded, never a silent fall-through. (The verdict's `pipeline` /
/// `degraded_from` orientation on the *host-fault* path is owned by
/// `capture::verify_capture` and is not pinned here; what this step owns is
/// the fault, the failing check and the guidance.)
#[test]
fn composition_off_surfaces_the_typed_fault_with_its_guidance() {
    let mut probe = DwmProbe {
        host: Err(CaptureFault::NoCompositor {
            detail: "DwmIsCompositionEnabled reports composition is off".into(),
        }),
    };
    let readiness = verify_capture(&mut probe, &window());
    assert!(readiness.is_capturable());
    assert_ne!(readiness.state, CaptureState::Ready);
    assert_eq!(
        readiness.failed_check,
        Some(CaptureCheck::CompositorRunning)
    );
    let fault = readiness.fault.clone().expect("a recorded fault");
    assert!(matches!(fault, CaptureFault::NoCompositor { .. }));
    assert!(readiness.guidance.contains("compositor"), "{readiness:?}");
    // The verdict round-trips through the JSON the surface carries.
    let json = CaptureReadiness::unavailable(fault).to_json();
    assert_eq!(json["failedCheck"], "compositor_running");
}
