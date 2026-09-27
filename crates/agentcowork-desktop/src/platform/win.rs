//! Windows backend (E9) — cross-compile-checked against x86_64-pc-windows-msvc.
//!
//! - **Read:** UI Automation via a **control view** tree walker (the raw view
//!   contains control-only duplicates that inflate every count), window/app list
//!   via `EnumWindows`. `FIX-17` moved the whole collector into
//!   [`crate::uia`]: bounded (nodes · depth · text · per-call budget), run on a
//!   **worker** with incremental output so a hung provider yields a *partial*
//!   read instead of stalling the agent, `AutomationId` treated as a **hint**,
//!   elevation limits degrading to a typed `Unknown`, protected fields masked,
//!   and ambiguity **rejected** rather than guessed.
//! - **Act:** UIA Invoke/SetValue **first**; SendInput fallback for
//!   click/type/scroll/drag (winappCli / deploymenttheory order).
//!   **P57.3:** activation (`SetForegroundWindow` + `ShowWindow`) and
//!   focus-stealing launches (`SW_SHOWNOACTIVATE` vs `SW_SHOWNORMAL`) are
//!   decided by the policy's interaction default — the Background path never
//!   calls `SetForegroundWindow`.
//!   **P57.4:** a *coordinate* click under the Background default also refuses
//!   SendInput. It is delivered either as a UIA `InvokePattern` on the element
//!   at the hit-test point or as a `WM_LBUTTONDOWN`/`WM_LBUTTONUP` message to
//!   the deepest child under the point — neither warps the cursor nor takes
//!   focus. If no element is invokable and the app ignores the message, the
//!   action fails honestly instead of silently moving the user's pointer.
//!   **P57.1:** launch goes through `ShellExecuteExW` on the canonical path.
//! - **See:** capture readiness is **verified before any capture** (`FIX-18`,
//!   [`crate::capture`]): WinRT graphics-capture support, a live BGRA-capable
//!   D3D11 device, a compositor, and a live window handle with non-zero extent.
//!   Then Windows.Graphics.Capture (WGC) → PrintWindow (`PW_RENDERFULLCONTENT`)
//!   → screen-DC BitBlt, with every skip recorded as a typed degrade on the
//!   result instead of a bare `None`.
//!
//! All COM/UIA/WinRT code is behind `#[cfg(windows)]`; this module compiles but
//! is never linked on non-Windows targets. Its **runtime** behaviour is therefore
//! unverified on this host: what is verified here is that the collector's
//! identity, ambiguity, elevation, timeout and readiness *rules* hold (they live
//! in the platform-neutral [`crate::uia`] / [`crate::capture`] modules and are
//! tested with fakes), not that a real UIA provider answers the way the Windows
//! client expects.

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetCurrentObject, GetDC, GetDIBits,
    GetObjectW, GetWindowDC, HBITMAP, HDC, OBJ_BITMAP, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::core::Interface;
// `ScreenToClient` is declared in the Gdi module by the windows-rs bindings.
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_ButtonControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId, UIA_EditControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_InvokePatternId, UIA_ListItemControlTypeId,
    UIA_MenuItemControlTypeId, UIA_RadioButtonControlTypeId, UIA_TextControlTypeId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    LogicalToPhysicalPointForPerMonitorDPI, SetProcessDpiAwarenessContext,
};
// `FIX-18` — the capture-readiness probe also asks the compositor itself:
// `DwmIsCompositionEnabled` (`Win32_Graphics_Dwm`, see the manifest) reports
// whether DWM composition is on, and a disabled compositor is a typed
// [`CaptureFault::NoCompositor`] rather than a capture that comes back black.
// The graphics-capture side of the same question is covered by
// `GraphicsCaptureSession::IsSupported()` plus a live BGRA-capable D3D11 device
// (see `platform/wgc.rs`).
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_WHEEL, MapVirtualKeyA, SendInput, VIRTUAL_KEY, mouse_event,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, ChildWindowFromPointEx, EnumWindows, GetClassNameW, GetForegroundWindow,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    PostMessageW, SW_RESTORE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SetCursorPos, SetForegroundWindow,
    ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP,
};

/// `MK_LBUTTON` — the modifier key state a mouse-down message carries. The
/// constant lives behind the `Win32_System_SystemServices` feature; the value is
/// fixed by the Win32 API (1), so it is spelled once here with its name.
const MK_LBUTTON: usize = 0x0001;

use crate::DesktopError;
use crate::capture::{CaptureCheck, CaptureFault, CapturePipeline, CaptureProbe, CaptureReadiness};
use crate::geometry::{DpiScale, DpiSource};
use crate::ladder::{ClickProfile, ClickRung, LadderTarget, RungDelivery};
use crate::launch;
use crate::policy::InteractionMode;
use crate::types::{ActKind, ReadNode, ReadResult, Region, SeeMethod, SeeResult, WindowInfo};
use crate::uia::{
    Coverage, SnapshotEpoch, UiaAnswer, UiaCall, UiaFault, UiaHandle, UiaNode, UiaProvider,
};

/// The Windows click ladder, as data.
///
/// All three rungs exist here — this is the only platform where the full ladder
/// is real: UIA `InvokePattern` at the hit-test point, a `PostMessageW` message
/// click to the deepest child under the point, and `SendInput` real pointer
/// injection. `RawInput` is last and gated, as it is everywhere.
pub fn win_click_profile() -> ClickProfile {
    ClickProfile::new(
        "windows",
        vec![
            ClickRung::AccessibilityInvoke,
            ClickRung::SyntheticEvent,
            ClickRung::RawInput,
        ],
        vec![
            "the pointer-moving rung is gated: reaching it needs a Guard-2 cursor-takeover \
             decision, and the walk stops there without one"
                .into(),
        ],
    )
    .expect("the Windows click profile is a fixed, ordered literal")
}

pub struct WinBackend;

fn hwnd_of(window: &WindowInfo) -> HWND {
    HWND(window.id as usize as *mut core::ffi::c_void)
}

/// `ActKind::Click` is **window-relative** on every platform (the X11 backend
/// adds the window origin; the live E2E test asserts it). UIA hit-testing and
/// `SetCursorPos` both want screen coordinates, so translate once, here.
fn screen_point(window: &WindowInfo, x: i32, y: i32) -> (i32, i32) {
    (window.x + x, window.y + y)
}

fn control_type_name(id: i32) -> String {
    match id {
        v if v == UIA_ButtonControlTypeId.0 => "Button".into(),
        v if v == UIA_EditControlTypeId.0 => "Edit".into(),
        v if v == UIA_ComboBoxControlTypeId.0 => "ComboBox".into(),
        v if v == UIA_CheckBoxControlTypeId.0 => "CheckBox".into(),
        v if v == UIA_RadioButtonControlTypeId.0 => "RadioButton".into(),
        v if v == UIA_MenuItemControlTypeId.0 => "MenuItem".into(),
        v if v == UIA_ListItemControlTypeId.0 => "ListItem".into(),
        v if v == UIA_HyperlinkControlTypeId.0 => "Hyperlink".into(),
        v if v == UIA_TextControlTypeId.0 => "Text".into(),
        other => format!("Type{other}"),
    }
}

/// The real UI Automation client behind [`crate::uia::UiaProvider`].
///
/// Three decisions here are load-bearing for `FIX-17` and were not in the v0 code:
///
/// - **Control view, not raw view.** The raw view contains control-only and
///   content-only duplicates of the same element, so a raw walk inflates every
///   count and can make one button look like two — which is exactly how a
///   "click the Save button" query becomes ambiguous. The control view is the
///   view a control's identity is expressed in.
/// - **Properties on request.** Name/bounds/IsPassword/IsOffscreen are read in
///   [`Self::call`] when the collector asks, not cached on a struct, so a
///   protected field's text is never held anywhere but the call that masks it.
/// - **Typed failures.** `UIAccess`-style refusals come back as
///   [`UiaFault::ElevationBlocked`], not as an absent child.
///
/// Cloning is how a read reaches its worker: the COM wrappers are reference
/// counted, and every walk gets its own per-walk handle maps.
#[derive(Clone)]
pub struct WinUiaClient {
    automation: IUIAutomation,
    walker: IUIAutomationTreeWalker,
    /// The snapshot generation this client stamps on its observations. Advances
    /// on every read, because a UIA tree is lazy and changes.
    snapshot: std::sync::atomic::AtomicU64,
    /// Per-process integrity level, cached: it cannot change while the process
    /// runs, and it is what decides whether an elevated target is readable.
    integrity: IntegrityLevel,
    /// Root elements by window id for the current walk, so a handle is only ever
    /// meaningful inside the walk that produced it.
    roots: std::collections::BTreeMap<u64, IUIAutomationElement>,
    /// Children by parent handle for the current walk, populated lazily.
    children: std::collections::BTreeMap<u64, Vec<IUIAutomationElement>>,
    /// The runtime id assigned to each element of the current walk.
    runtime: std::collections::BTreeMap<u64, IUIAutomationElement>,
}

/// How elevated this process is — the fact that decides whether an elevated
/// target's UI can be read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityLevel {
    /// Medium integrity: the default for a normal desktop app.
    Medium,
    /// High integrity: the process is elevated.
    High,
    /// System integrity.
    System,
}

impl IntegrityLevel {
    /// May this level read a target at `target`?
    ///
    /// A medium-integrity client cannot read elevated UI at all without the
    /// UIAccess flag, and SYSTEM UI is unreachable even for an elevated client
    /// unless it is itself UIAccess-enabled. We are not UIAccess-enabled (that
    /// needs a signed binary in a secure location, which is a deployment decision
    /// `ARCH/21` §5 rule 4 defers), so the rule here is the strict one.
    pub fn may_read(&self, target: IntegrityLevel) -> bool {
        match (self, target) {
            (IntegrityLevel::High, _) => true,
            // SYSTEM UI needs UIAccess regardless of elevation.
            (IntegrityLevel::Medium, IntegrityLevel::High) => false,
            (IntegrityLevel::Medium, IntegrityLevel::System) => false,
            (IntegrityLevel::Medium, IntegrityLevel::Medium) => true,
            (IntegrityLevel::System, _) => true,
        }
    }
}

impl WinUiaClient {
    /// Create the client. COM is initialised MTA, which is what the collector's
    /// worker thread wants.
    pub fn init() -> Result<Self, DesktopError> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let automation = unsafe {
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_ALL)
                .map_err(|e| DesktopError::Platform(format!("UIA init: {e}")))?
        };
        // `FIX-17` — the **control** view walker. The raw view duplicates
        // elements (a control-only node plus a content-only node for the same
        // control), so a raw walk makes one button look like two and turns every
        // name-addressed click into a coin flip.
        let walker = unsafe {
            automation
                .ControlViewWalker()
                .map_err(|e| DesktopError::Platform(format!("ControlViewWalker: {e}")))?
        };
        Ok(Self {
            automation,
            walker,
            snapshot: std::sync::atomic::AtomicU64::new(0),
            integrity: Self::integrity_level(),
            roots: Default::default(),
            children: Default::default(),
            runtime: Default::default(),
        })
    }

    /// This process's integrity level.
    ///
    /// `GetTokenInformation(TokenIntegrityLevel)` + `GetSidSubAuthority` is the
    /// documented way to read it. It is read once and cached: a process's
    /// integrity level cannot be lowered without exiting, so re-reading it per
    /// call would cost a syscall for a constant.
    pub fn integrity_level() -> IntegrityLevel {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::TOKEN_QUERY;
        use windows::Win32::Security::{
            GetSidSubAuthority, GetTokenInformation, TOKEN_ELEVATION, TokenElevation,
            TokenIntegrityLevel,
        };
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        unsafe {
            let mut token = windows::Win32::Foundation::HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return IntegrityLevel::Medium;
            }
            // An elevated token is the direct, unambiguous answer; fall back to
            // the integrity SID when the elevation class is not available.
            let mut elevated = 0u32;
            let mut returned = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevated as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
                &mut returned,
            )
            .is_ok();
            if ok {
                let _ = LocalFree(Some(HLOCAL(token.0)));
                return if elevated != 0 {
                    IntegrityLevel::High
                } else {
                    IntegrityLevel::Medium
                };
            }
            let mut sid = std::ptr::null_mut();
            let mut size = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenIntegrityLevel,
                (&mut sid as *mut _).cast(),
                0,
                &mut size,
            )
            .is_ok()
                && size > 0;
            let mut level = IntegrityLevel::Medium;
            if ok {
                let mut buffer = vec![0u8; size as usize];
                let ok = GetTokenInformation(
                    token,
                    TokenIntegrityLevel,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut returned,
                )
                .is_ok();
                if ok {
                    let sid_ptr = buffer.as_mut_ptr().cast();
                    let rid = GetSidSubAuthority(sid_ptr, 4); // RID 4 = integrity
                    if !rid.is_null() {
                        level = match *rid {
                            r if r >= 0x4000 => IntegrityLevel::System,
                            r if r >= 0x2000 => IntegrityLevel::High,
                            _ => IntegrityLevel::Medium,
                        };
                    }
                }
            }
            let _ = LocalFree(Some(HLOCAL(token.0)));
            level
        }
    }

    /// This client's integrity level (cached at construction).
    pub fn integrity(&self) -> IntegrityLevel {
        self.integrity
    }

    /// A live `IUIAutomationElement` for a handle from the current walk.
    fn element(&self, handle: UiaHandle) -> Option<IUIAutomationElement> {
        self.runtime.get(&handle.0).cloned()
    }

    /// The root element for a window, read once per walk.
    fn root_for(&mut self, window: &WindowInfo) -> Option<IUIAutomationElement> {
        if let Some(r) = self.roots.get(&window.id) {
            return Some(r.clone());
        }
        let element = unsafe { self.automation.ElementFromHandle(hwnd_of(window)) }.ok()?;
        if element.as_raw().is_null() {
            return None;
        }
        self.roots.insert(window.id, element.clone());
        Some(element)
    }

    /// The children of an element, walked once per walk.
    fn children_of(&mut self, parent: UiaHandle) -> Vec<IUIAutomationElement> {
        if let Some(c) = self.children.get(&parent.0) {
            return c.clone();
        }
        let mut out: Vec<IUIAutomationElement> = Vec::new();
        if let Some(element) = self.element(parent) {
            if let Ok(mut child) = unsafe { self.walker.GetFirstChildElement(&element) } {
                let mut guard = 0;
                while !child.as_raw().is_null() && guard < crate::uia::MAX_NODES {
                    guard += 1;
                    let next = unsafe { self.walker.GetNextSiblingElement(&child) }.ok();
                    out.push(child.clone());
                    match next {
                        Ok(n) if !n.as_raw().is_null() => child = n,
                        _ => break,
                    }
                }
            }
        }
        self.children.insert(parent.0, out.clone());
        out
    }

    /// Assign (or reuse) the runtime id for an element of this walk.
    ///
    /// The handle the collector sees is a small integer we assign, not the UIA
    /// `RuntimeId` array: the array is opaque, not stable over time, and
    /// comparing it is not a supported operation. A per-walk index is honest,
    /// epoch-scoped identity, which is exactly what `DM-026` asks for.
    fn assign(&mut self, element: &IUIAutomationElement) -> UiaHandle {
        if let Some((handle, _)) = self
            .runtime
            .iter()
            .find(|(_, e)| std::ptr::eq(e.as_raw(), element.as_raw()))
        {
            return UiaHandle(*handle);
        }
        let handle = UiaHandle(self.runtime.len() as u64 + 1);
        self.runtime.insert(handle.0, element.clone());
        handle
    }

    /// Read an element's properties into the collector's node shape.
    fn node_of(&self, handle: UiaHandle, element: &IUIAutomationElement) -> UiaNode {
        let role = control_type_name(
            unsafe { element.CurrentControlType() }
                .map(|c| c.0)
                .unwrap_or(0),
        );
        // `IsPassword` is read **before** the name: a protected field's text is
        // never put into a node at all, only the mask (`REQ-CUA-009`,
        // `ARCH/21` §5.3). The collector's `sanitize` enforces the rule again, so
        // a second implementation cannot leak it.
        let is_password = unsafe { element.CurrentIsPassword() }.unwrap_or(false);
        let name = if is_password {
            crate::uia::MASKED.to_string()
        } else {
            unsafe { element.CurrentName() }
                .map(|b| b.to_string())
                .unwrap_or_default()
        };
        let automation_id = unsafe { element.CurrentAutomationId() }
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty());
        let mut rect: RECT = std::mem::zeroed();
        if let Ok(r) = unsafe { element.CurrentBoundingRectangle() } {
            rect = r;
        }
        UiaNode {
            handle,
            role,
            name,
            automation_id,
            bounds: Region {
                x: rect.left,
                y: rect.top,
                width: (rect.right - rect.left).max(0) as u32,
                height: (rect.bottom - rect.top).max(0) as u32,
            },
            is_password,
            is_offscreen: unsafe { element.CurrentIsOffscreen() }.unwrap_or(false),
            is_enabled: unsafe { element.CurrentIsEnabled() }.unwrap_or(true),
            truncated: false,
        }
    }

    /// Invoke the control at a screen point, refusing anything not owned by the
    /// target's process (the hit-test is a screen-space lookup, so an overlapping
    /// window can answer for a point inside our target).
    pub fn invoke_at_screen(
        &self,
        window: &WindowInfo,
        sx: i32,
        sy: i32,
    ) -> Result<bool, DesktopError> {
        let point = POINT { x: sx, y: sy };
        let target = hwnd_of(window);
        let target_pid = process_id_of(target)?;
        let element = match unsafe { self.automation.ElementFromPoint(point) } {
            Ok(e) if !e.as_raw().is_null() => e,
            _ => return Ok(false),
        };
        let same_process = unsafe { element.CurrentProcessId() }
            .map(|pid| pid as u32 == target_pid && target_pid != 0)
            .unwrap_or(false);
        if !same_process {
            return Ok(false);
        }
        let pattern = match unsafe { element.GetCurrentPattern(UIA_InvokePatternId) } {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };
        let invoke = match pattern.cast::<IUIAutomationInvokePattern>() {
            Ok(i) if !i.as_raw().is_null() => i,
            _ => return Ok(false),
        };
        unsafe {
            invoke
                .Invoke()
                .map_err(|e| DesktopError::Platform(format!("UIA Invoke at ({sx},{sy}): {e}")))?;
        }
        Ok(true)
    }

    /// Set an element's value through `ValuePattern` — the pattern-first rung
    /// (`REQ-CUA-005`).
    pub fn set_value_at_screen(&self, sx: i32, sy: i32, value: &str) -> Result<bool, DesktopError> {
        let element = match unsafe { self.automation.ElementFromPoint(POINT { x: sx, y: sy }) } {
            Ok(e) if !e.as_raw().is_null() => e,
            _ => return Ok(false),
        };
        let pattern = match unsafe { element.GetCurrentPattern(UIA_ValuePatternId) } {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };
        let value_pattern = match pattern.cast::<IUIAutomationValuePattern>() {
            Ok(v) if !v.as_raw().is_null() => v,
            _ => return Ok(false),
        };
        unsafe {
            value_pattern
                .SetValue(&windows::core::BSTR::from(value))
                .map_err(|e| DesktopError::Platform(format!("UIA SetValue: {e}")))?;
        }
        Ok(true)
    }
}

impl crate::uia::UiaProvider for WinUiaClient {
    fn epoch(&self) -> SnapshotEpoch {
        SnapshotEpoch(self.snapshot.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// `FIX-17` — coverage comes from the process's **integrity level** and the
    /// target's, never from the emptiness of a result.
    ///
    /// The target's level is read from the UIA element itself when it can be
    /// resolved; a window we cannot resolve at all is reported by
    /// [`Self::call`] as `TargetUnavailable`, not as an empty tree.
    fn coverage(&mut self, window: &WindowInfo) -> Coverage {
        if let Some(element) = self.root_for(window) {
            let target_pid = unsafe { element.CurrentProcessId() }
                .map(|p| p as u32)
                .unwrap_or(0);
            if target_pid != 0
                && let Some(level) = process_integrity(target_pid)
                && !self.integrity.may_read(level)
            {
                return Coverage::Restricted { elevated: true };
            }
        }
        Coverage::Complete
    }

    fn call(
        &mut self,
        _window: &WindowInfo,
        call: crate::uia::UiaCall,
    ) -> std::result::Result<crate::uia::UiaAnswer, UiaFault> {
        use crate::uia::{UiaAnswer, UiaCall};
        match call {
            UiaCall::Root => {
                let element = self.root_for(_window).ok_or(UiaFault::TargetUnavailable {
                    detail:
                        "ElementFromHandle returned no element (closed window, reused handle, or \
                              another desktop)"
                            .into(),
                })?;
                let handle = self.assign(&element);
                Ok(UiaAnswer::Element(Box::new(UiaNode {
                    handle,
                    role: "Pane".into(),
                    name: String::new(),
                    automation_id: None,
                    bounds: Region::full(0, 0),
                    is_password: false,
                    is_offscreen: false,
                    is_enabled: true,
                    truncated: false,
                })))
            }
            UiaCall::Properties(handle) => {
                let element = self.element(handle).ok_or(UiaFault::TargetUnavailable {
                    detail: format!("element {handle:?} is not part of this walk"),
                })?;
                Ok(UiaAnswer::Element(Box::new(self.node_of(handle, &element))))
            }
            UiaCall::FirstChild(handle) => {
                let children = self.children_of(handle);
                let Some(first) = children.first().cloned() else {
                    return Ok(UiaAnswer::Absent);
                };
                let h = self.assign(&first);
                Ok(UiaAnswer::Element(Box::new(UiaNode {
                    handle: h,
                    role: "Pane".into(),
                    name: String::new(),
                    automation_id: None,
                    bounds: Region::full(0, 0),
                    is_password: false,
                    is_offscreen: false,
                    is_enabled: true,
                    truncated: false,
                })))
            }
            UiaCall::NextSibling(handle) => {
                // The sibling list is materialised on the first-child call, so a
                // sibling is looked up by position in it.
                let Some((parent, index)) = self.sibling_position(handle) else {
                    return Ok(UiaAnswer::Absent);
                };
                let children = self.children_of(parent);
                let Some(next) = children.get(index + 1).cloned() else {
                    return Ok(UiaAnswer::Absent);
                };
                let h = self.assign(&next);
                Ok(UiaAnswer::Element(Box::new(UiaNode {
                    handle: h,
                    role: "Pane".into(),
                    name: String::new(),
                    automation_id: None,
                    bounds: Region::full(0, 0),
                    is_password: false,
                    is_offscreen: false,
                    is_enabled: true,
                    truncated: false,
                })))
            }
        }
    }
}

impl WinUiaClient {
    /// The (parent, index) of a handle inside its parent's materialised child
    /// list, or `None` for a root.
    fn sibling_position(&self, handle: UiaHandle) -> Option<(UiaHandle, usize)> {
        for (parent, children) in self.children.iter() {
            if let Some(index) = children
                .iter()
                .position(|c| std::ptr::eq(c.as_raw(), self.runtime.get(&handle.0)?.as_raw()))
            {
                return Some((UiaHandle(*parent), index));
            }
        }
        None
    }
}

/// The integrity level of another process, when it can be read.
///
/// A failure is `None`, which the caller treats as "cannot tell" — never as
/// "medium", so a target we cannot inspect is not asserted to be readable.
fn process_integrity(pid: u32) -> Option<IntegrityLevel> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::Authorization::TOKEN_QUERY;
    use windows::Win32::Security::{GetSidSubAuthority, GetTokenInformation, TokenIntegrityLevel};
    use windows::Win32::System::Threading::{OpenProcess, OpenProcessToken};
    unsafe {
        let process = OpenProcess(
            windows::Win32::Foundation::PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
        .ok()?;
        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            let _ = windows::Win32::Foundation::CloseHandle(process);
            return None;
        }
        let mut sid = std::ptr::null_mut();
        let mut size = 0u32;
        let first = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            (&mut sid as *mut _).cast(),
            0,
            &mut size,
        )
        .is_ok()
            && size > 0;
        let mut level = None;
        if first {
            let mut buffer = vec![0u8; size as usize];
            if GetTokenInformation(
                token,
                TokenIntegrityLevel,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
            .is_ok()
            {
                let sid_ptr = buffer.as_mut_ptr().cast();
                let rid = GetSidSubAuthority(sid_ptr, 4);
                if !rid.is_null() {
                    level = Some(match *rid {
                        r if r >= 0x4000 => IntegrityLevel::System,
                        r if r >= 0x2000 => IntegrityLevel::High,
                        _ => IntegrityLevel::Medium,
                    });
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(process);
        level
    }
}

/// The Windows façade the rest of the crate talks to.
///
/// `FIX-17` changed what this type is allowed to do:
///
/// - `read` goes through [`crate::uia::read_window`] — bounded, worker-isolated,
///   epoch-stamped, and **typed** about what it could not see. It no longer
///   returns a bare `Option<ReadNode>` where "not allowed to look" and "there is
///   nothing there" were the same answer.
/// - `click_by_name` / `set_value` resolve through [`crate::uia::resolve`] and
///   **reject an ambiguous match** instead of invoking whichever element the
///   first `contains` hit returned.
/// - a read whose status is not conclusive refuses to act, because a coordinate
///   derived from an unknown region is a blind input attempt.
pub struct WinUia {
    client: WinUiaClient,
    tracker: crate::uia::UiaEpochTracker,
}

impl WinUia {
    pub fn init() -> Result<Self, DesktopError> {
        Ok(Self {
            client: WinUiaClient::init()?,
            tracker: crate::uia::UiaEpochTracker::new(),
        })
    }

    /// The UIA client, for the point-addressed rungs (which do not walk a tree).
    pub fn client(&self) -> &WinUiaClient {
        &self.client
    }

    /// `FIX-17` — one bounded, isolated, epoch-stamped structured read.
    ///
    /// This is the only read path. There is deliberately no "quick tree" variant:
    /// an unbounded synchronous UIA walk is the failure `REQ-CUA-004` names, and
    /// a second path would be a way back to it.
    pub fn structured_read(&self, window: &WindowInfo) -> crate::uia::UiaRead {
        let cfg = crate::uia::CollectConfig::default();
        let snapshot = self
            .client
            .snapshot
            .load(std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.client
            .snapshot
            .store(snapshot, std::sync::atomic::Ordering::SeqCst);
        let read = crate::uia::read_window_with(self.client.clone(), window, &cfg, &self.tracker);
        self.client.runtime.clear();
        self.client.children.clear();
        self.client.roots.clear();
        read
    }

    /// The bounded tree of one window, for a caller that only needs the shape.
    ///
    /// Returns `None` only when the read genuinely found no structural UI; an
    /// unreadable (elevated) window returns `None` **and** an `Unknown` status in
    /// [`Self::read`], which is where the difference is observable.
    pub fn tree_for(&self, window: &WindowInfo) -> Option<ReadNode> {
        self.structured_read(window).read.tree
    }

    pub fn read(&self, window: &WindowInfo) -> Result<ReadResult, DesktopError> {
        let observed = self.structured_read(window);
        let read = &observed.read;
        Ok(ReadResult {
            window_id: window.id,
            tree: read.tree.clone(),
            dpi_scale: WinBackend::dpi_scale(window).factor,
            windows: WinBackend::list_windows()?,
            epoch: read.epoch,
            observed_at_ms: read.observed_at_ms,
            status: read.status.clone(),
            guidance: read.guidance(window),
            anomalies: read.anomalies.clone(),
        })
    }

    pub fn send_click(&self, x: i32, y: i32) -> Result<(), DesktopError> {
        unsafe {
            SetCursorPos(x, y).map_err(|e| DesktopError::Platform(format!("SetCursorPos: {e}")))?;
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }
        Ok(())
    }

    /// UIA-first click on a **named** control.
    ///
    /// `FIX-17`: the name is resolved against a fresh bounded read and an
    /// ambiguous match is **rejected** with every candidate named — never
    /// resolved to "the first one that contains the text". A read whose status
    /// does not permit inferring absence refuses outright, because there is no
    /// evidence the control is there at all.
    pub fn click_by_name(&self, window: &WindowInfo, name: &str) -> Result<(), DesktopError> {
        let point = self.resolve_point(window, &crate::uia::ElementQuery::named(name))?;
        let (cx, cy) = point;
        // `InvokePattern` first (the highest-fidelity rung), then a raw click on
        // the resolved bounds. The point comes from a UIA bounding rectangle, so
        // it is already screen-space physical pixels.
        if self.client.invoke_at_screen(window, cx, cy)? {
            return Ok(());
        }
        self.send_click(cx, cy)
    }

    /// `ValuePattern::SetValue` on a **named** control, with the same
    /// ambiguity/elevation discipline as [`Self::click_by_name`].
    pub fn set_value_by_name(
        &self,
        window: &WindowInfo,
        name: &str,
        value: &str,
    ) -> Result<(), DesktopError> {
        let (cx, cy) = self.resolve_point(window, &crate::uia::ElementQuery::named(name))?;
        if self.client.set_value_at_screen(cx, cy, value)? {
            return Ok(());
        }
        // No `ValuePattern` on the resolved control: the honest fallback is the
        // ladder's message rung, then typing — and both need a point the resolver
        // already proved unique.
        post_screen_click(window, cx, cy)?;
        send_input_type(value)
    }

    /// Resolve a name to a screen point through a fresh bounded read, or refuse
    /// with the reason.
    ///
    /// The single place `FIX-17`'s identity rules become enforcement: a
    /// non-conclusive read, an ambiguous match and an absent element each produce
    /// their own typed refusal, and none of them produces a coordinate.
    pub fn resolve_point(
        &self,
        window: &WindowInfo,
        query: &crate::uia::ElementQuery,
    ) -> Result<(i32, i32), DesktopError> {
        let observed = self.structured_read(window);
        match crate::uia::resolve_in(&observed.read, query) {
            crate::uia::Resolution::Resolved(handle) => {
                // An observation is valid for one action; this is that action.
                // The check is here so a future cached handle cannot be actuated.
                let validity = handle.validate(observed.read.current_epoch(), crate::now_ms());
                if !validity.is_current {
                    return Err(DesktopError::Platform(
                        validity
                            .reason
                            .unwrap_or_else(|| "stale observation".into()),
                    ));
                }
                Ok(handle.element.bounds.center())
            }
            crate::uia::Resolution::Ambiguous { reason, .. } => {
                Err(DesktopError::Platform(format!(
                    "ambiguous element, refusing to guess: {reason} — re-read the window and \
                     narrow the query (role, or the bounds you believe it has)"
                )))
            }
            crate::uia::Resolution::NotFound { reason, .. } => Err(DesktopError::Platform(
                format!("no element to act on: {reason}"),
            )),
        }
    }

    /// P57.4 — activate the control **at the point** via UIA, without moving the
    /// cursor. Returns `Ok(false)` when there is no invokable element there (a
    /// canvas, a game surface, an occluding window) — a normal answer that lets
    /// the caller fall back to a message click.
    pub fn invoke_at(&self, window: &WindowInfo, x: i32, y: i32) -> Result<bool, DesktopError> {
        let (sx, sy) = screen_point(window, x, y);
        self.client.invoke_at_screen(window, sx, sy)
    }

    /// [`Self::invoke_at`] at an absolute **screen** point — the form the
    /// name-addressed ladder uses, where the point comes from a UIA bounding
    /// rectangle (already screen-space, physical pixels).
    ///
    /// The same-process check is the load-bearing part: a hit-test is a
    /// screen-space lookup, so an overlapping window can answer for a point
    /// inside our target. The click is only ever activated on an element that
    /// really belongs to the process we were asked to act on.
    pub fn invoke_at_screen(
        &self,
        window: &WindowInfo,
        sx: i32,
        sy: i32,
    ) -> Result<bool, DesktopError> {
        self.client.invoke_at_screen(window, sx, sy)
    }
}

/// The Windows capture-readiness probe (`FIX-18`).
///
/// Declared in fidelity order, exactly the order `see()` will try: graphics
/// capture → `PrintWindow` → screen DC. Declaring the order *here* (and only
/// here) is what makes "the highest-fidelity usable pipeline" a fact rather than
/// a preference expressed twice.
pub struct WinCaptureProbe {
    hwnd: HWND,
    /// Screen DC available for a `ScreenDc` capture.
    screen_dc: bool,
}

impl WinCaptureProbe {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            // A screen DC is available on any interactive desktop; the *honest*
            // use of it (a screen capture of an occluded window) is the caller's
            // decision, which is why `is_occluded` is asked separately.
            screen_dc: true,
        }
    }
}

impl crate::capture::CaptureProbe for WinCaptureProbe {
    fn pipelines(&self) -> Vec<CapturePipeline> {
        vec![
            CapturePipeline::WindowsGraphicsCapture,
            CapturePipeline::PrintWindow,
            CapturePipeline::ScreenDc,
        ]
    }

    /// Host-scoped checks: an interactive session, WinRT graphics-capture
    /// support, a live BGRA-capable D3D11 device, and a running compositor.
    fn host(&mut self) -> Result<Vec<CaptureCheck>, CaptureFault> {
        if !WinBackend::interactive_desktop() {
            return Err(CaptureFault::NoInteractiveSession {
                detail: "SESSIONNAME=Services — this process is in Session 0, which has no \
                         interactive desktop"
                    .into(),
            });
        }
        let mut verified = vec![CaptureCheck::SessionAvailable];
        if !crate::platform::wgc::session_supported() {
            return Err(CaptureFault::SessionUnsupported {
                detail: "GraphicsCaptureSession::IsSupported() is false on this host (pre-1803 \
                         Windows, or the feature is disabled)"
                    .into(),
            });
        }
        verified.push(CaptureCheck::GraphicsDevice);
        if !crate::platform::wgc::device_available() {
            return Err(CaptureFault::NoGraphicsDevice {
                detail: "a BGRA-capable D3D11 device could not be created (headless GPU, or a \
                         session with no adapter)"
                    .into(),
            });
        }
        // The compositor step: without DWM composition there is no composited
        // surface for graphics capture to composite, so a disabled compositor
        // is a typed `NoCompositor` — its own check and its own fault, never a
        // boolean folded into the session/device verdict above.
        verified.push(compositor_verdict(dwm_composition())?);
        Ok(verified)
    }

    /// A conservative occlusion test: the target's own rectangle is not fully
    /// covered by the foreground window. False means "probably visible", which is
    /// the direction that never blocks a legitimate capture.
    fn is_occluded(&mut self, window: &WindowInfo) -> bool {
        let Some(foreground) = (unsafe { GetForegroundWindow() }) else {
            return false;
        };
        if foreground == self.hwnd {
            return false;
        }
        let mut target: RECT = std::mem::zeroed();
        let mut front: RECT = std::mem::zeroed();
        unsafe {
            if GetWindowRect(self.hwnd, &mut target).is_err()
                || GetWindowRect(foreground, &mut front).is_err()
            {
                return false;
            }
        }
        front.left >= target.left
            && front.top >= target.top
            && front.right <= target.right
            && front.bottom <= target.bottom
    }

    /// Per-target checks for one named pipeline.
    fn target(
        &mut self,
        _window: &WindowInfo,
        pipeline: CapturePipeline,
    ) -> Result<Vec<CaptureCheck>, CaptureFault> {
        // A live handle is a precondition for every pipeline, so it is checked
        // first and reported as such.
        if unsafe { IsWindow(self.hwnd) }.as_bool() == false {
            return Err(CaptureFault::InvalidWindowHandle {
                detail:
                    "IsWindow is false — the window was closed, or the handle was reused since \
                         it was listed"
                        .into(),
            });
        }
        let mut verified = vec![CaptureCheck::WindowHandleValid];
        let mut rect: RECT = std::mem::zeroed();
        unsafe {
            GetWindowRect(self.hwnd, &mut rect).map_err(|e| CaptureFault::ProbeFailed {
                detail: format!("GetWindowRect: {e}"),
            })?;
        }
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Err(CaptureFault::ZeroExtent {
                detail: format!(
                    "GetWindowRect returned {}x{} — the window is minimized or has no visible \
                     extent",
                    rect.right - rect.left,
                    rect.bottom - rect.top
                ),
            });
        }
        verified.push(CaptureCheck::NonZeroExtent);
        match pipeline {
            CapturePipeline::WindowsGraphicsCapture => {
                // The real per-target check: a capture item must exist for this
                // HWND. A minimized-to-shell-icon, protected or compositor-refused
                // window has none, and that is exactly the case the v0 code
                // collapsed into `None`.
                crate::platform::wgc::item_available(self.hwnd)
                    .map_err(|detail| CaptureFault::CaptureItemUnavailable { detail })?;
                verified.push(CaptureCheck::CaptureItem);
                Ok(verified)
            }
            CapturePipeline::PrintWindow => Ok(verified),
            CapturePipeline::ScreenDc => {
                if !self.screen_dc {
                    return Err(CaptureFault::ProbeFailed {
                        detail: "no screen device context is available".into(),
                    });
                }
                Ok(verified)
            }
            other => Err(CaptureFault::PipelineUnsupported {
                detail: format!("{} is not a Windows pipeline", other.as_str()),
            }),
        }
    }
}

/// The raw DWM composition flag: `DwmIsCompositionEnabled` as the OS reports
/// it.
///
/// `Ok(enabled)` is the answer; `Err` carries the HRESULT text. The call has
/// no interesting failure mode on a healthy desktop (it has reported the flag
/// since Vista), so there is no retry — the verdict below decides what a
/// failure means.
fn dwm_composition() -> Result<bool, String> {
    unsafe {
        windows::Win32::Graphics::Dwm::DwmIsCompositionEnabled()
            .map(|enabled| enabled.as_bool())
            .map_err(|e| format!("DwmIsCompositionEnabled: {e}"))
    }
}

/// The compositor step of the capture-readiness chain, as a typed verdict.
///
/// This is the `wgc.rs` split applied to DWM: the raw flag in, a named check
/// (`CompositorRunning`) or its own typed fault (`NoCompositor`) out — never a
/// boolean folded into the session/device verdict. A *failing call* is also a
/// `NoCompositor`, deliberately: composition that cannot be confirmed running
/// must not be promised, and the HRESULT in the detail keeps the two cases
/// apart on a receipt.
fn compositor_verdict(query: Result<bool, String>) -> Result<CaptureCheck, CaptureFault> {
    match query {
        Ok(true) => Ok(CaptureCheck::CompositorRunning),
        Ok(false) => Err(CaptureFault::NoCompositor {
            detail: "DwmIsCompositionEnabled reports composition is off (a Basic theme, or a \
                     session without composition) — graphics capture has no composited surface \
                     to composite"
                .into(),
        }),
        Err(call) => Err(CaptureFault::NoCompositor {
            detail: format!(
                "{call} — treating composition as off rather than attempting a capture that \
                 would come back black"
            ),
        }),
    }
}

/// P57.4 — a click delivered as a window message, which no component of Win32
/// turns into pointer motion: no `SetCursorPos`, no `SendInput`, no focus.
///
/// The target is the deepest child of the requested window under the point
/// (children are separate HWNDs and route their own input), and the search never
/// leaves that window, so an overlapping app cannot receive the click.
///
/// The resolved HWND is then **re-checked against the target's own process**
/// before the message is posted. `ChildWindowFromPointEx` only ever returns a
/// descendant of the handle it is given, so this is belt-and-braces — but the
/// rule it enforces ("a synthetic click never acts on another process's
/// window") is worth a runtime check that cannot be argued away, and a violated
/// check fails the click instead of the process.
fn post_click(window: &WindowInfo, x: i32, y: i32) -> Result<(), DesktopError> {
    let (sx, sy) = screen_point(window, x, y);
    post_click_at_screen(window, sx, sy)
}

/// [`post_click`] at an absolute screen point — the form the name-addressed
/// ladder needs, where the point comes from a UIA bounding rectangle that is
/// already screen-space.
fn post_screen_click(window: &WindowInfo, sx: i32, sy: i32) -> Result<(), DesktopError> {
    post_click_at_screen(window, sx, sy)
}

fn post_click_at_screen(window: &WindowInfo, sx: i32, sy: i32) -> Result<(), DesktopError> {
    let mut target = hwnd_of(window);
    let target_pid = process_id_of(target)?;
    let mut client = POINT { x: sx, y: sy };
    for _ in 0..8 {
        let mut rect: RECT = RECT::default();
        unsafe {
            GetWindowRect(target, &mut rect)
                .map_err(|e| DesktopError::Platform(format!("GetWindowRect: {e}")))?;
        }
        client = POINT {
            x: sx - rect.left,
            y: sy - rect.top,
        };
        let child = unsafe { ChildWindowFromPointEx(target, client, CWP_SKIPINVISIBLE) };
        if child.is_invalid() || child == target {
            break;
        }
        target = child;
    }
    if process_id_of(target)? != target_pid {
        return Err(DesktopError::Platform(format!(
            "refusing to post a click: the HWND under ({sx},{sy}) belongs to process {}, not the \
             target's process {target_pid}",
            process_id_of(target)?,
        )));
    }
    let lparam = LPARAM(((client.y as isize) << 16) | (client.x as isize & 0xffff));
    unsafe {
        PostMessageW(target, WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), lparam)
            .map_err(|e| DesktopError::Platform(format!("PostMessage WM_LBUTTONDOWN: {e}")))?;
        PostMessageW(target, WM_LBUTTONUP, WPARAM(0), lparam)
            .map_err(|e| DesktopError::Platform(format!("PostMessage WM_LBUTTONUP: {e}")))?;
    }
    Ok(())
}

/// The owning process of an HWND. `0` is returned for a handle with no owning
/// process (a desktop window / the shell), which callers treat as "unknown".
fn process_id_of(hwnd: HWND) -> Result<u32, DesktopError> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    Ok(pid)
}

impl WinBackend {
    /// P69.G4 — declare the process **per-monitor-v2 DPI aware**, once.
    ///
    /// This is the load-bearing half of the DPI work. A process that is *not*
    /// per-monitor aware has its coordinates virtualised by Win32: the OS
    /// scales `GetWindowRect`, `ElementFromPoint` and friends into the
    /// process's own logical space while `SetCursorPos` still takes real screen
    /// pixels. On a 1.25× or 1.5× display that mismatch is exactly the bug the
    /// old hardcoded `1.0` hid — the numbers looked self-consistent and the
    /// click still landed in the wrong place.
    ///
    /// With awareness set, every Win32 coordinate this module uses is physical,
    /// which is also the space a screenshot and an incoming `ActKind::Click`
    /// are in. The call is idempotent and may legitimately fail with
    /// `E_ACCESSDENIED` when the host process (e.g. the Tauri shell) already
    /// chose a context — that is not an error, it only means somebody else set
    /// it, so it is reported as `false` and the caller falls back to the
    /// measured per-window DPI rather than assuming a context it did not set.
    pub fn ensure_per_monitor_v2() -> bool {
        static SET: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SET.get_or_init(|| unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok()
        })
    }

    /// P69.G4 — the target window's real DPI, with its provenance.
    ///
    /// `GetDpiForWindow` is per-window, so a second monitor at a different scale
    /// is reported correctly. A zero answer (an invalid HWND, or a pre-10
    /// Windows 10 host) degrades to the honest unknown, never to a fabricated
    /// 96 DPI.
    pub fn dpi_scale(window: &WindowInfo) -> DpiScale {
        Self::ensure_per_monitor_v2();
        let dpi = unsafe { GetDpiForWindow(hwnd_of(window)) };
        DpiScale::from_dpi(dpi, DpiSource::PerMonitorV2)
    }

    /// P69.G4 — attempt one rung of the click ladder.
    ///
    /// Coordinate arithmetic, stated once so the three rungs cannot disagree:
    /// the caller supplies **window-relative physical pixels** (the same space as
    /// the screenshot), and a per-monitor-v2 aware process means
    /// `GetWindowRect`/`ElementFromPoint`/`SetCursorPos` are all in physical
    /// screen pixels too — so no factor is needed. If awareness could *not* be
    /// set (something else in the process owns the DPI context), the measured
    /// per-window factor is used to convert through
    /// `LogicalToPhysicalPointForPerMonitorDPI` instead, which is the honest
    /// fallback: better a documented conversion than a mis-placed click.
    pub fn deliver_rung(
        rung: ClickRung,
        target: &LadderTarget,
        mode: InteractionMode,
    ) -> RungDelivery {
        let window = &target.window;
        let (x, y) = match &target.act {
            ActKind::Click { x, y } => (*x, *y),
            ActKind::ClickByName { name } => return deliver_named(rung, name, window, mode),
            other => {
                return RungDelivery::Unavailable(format!(
                    "rung {} does not apply to {}",
                    rung.as_str(),
                    other.describe()
                ));
            }
        };
        match rung {
            ClickRung::AccessibilityInvoke => {
                let (sx, sy) = screen_point(window, x, y);
                let uia = match WinUia::init() {
                    Ok(u) => u,
                    Err(e) => {
                        return RungDelivery::Unavailable(format!(
                            "UI Automation unavailable: {e}"
                        ));
                    }
                };
                match uia.invoke_at_screen(window, sx, sy) {
                    Ok(true) => RungDelivery::Delivered(format!(
                        "UIA InvokePattern on the control at ({sx},{sy}) (same process as the target)"
                    )),
                    Ok(false) => RungDelivery::Unavailable(
                        "no invokable element owned by the target process at this point".into(),
                    ),
                    Err(e) => RungDelivery::Failed(e.to_string()),
                }
            }
            ClickRung::SyntheticEvent => match post_click(window, x, y) {
                Ok(()) => RungDelivery::Delivered(format!(
                    "WM_LBUTTONDOWN/WM_LBUTTONUP posted to the target's own HWND at ({x},{y})"
                )),
                Err(e) => RungDelivery::Failed(e.to_string()),
            },
            ClickRung::RawInput => {
                if mode == InteractionMode::Background {
                    // Defence in depth: `PlatformBackend::deliver_rung` refuses
                    // this first. Belt and braces so a future caller that
                    // reaches the backend directly cannot warp the pointer.
                    return RungDelivery::Blocked(
                        "background contract: SendInput moves the real pointer — switch the \
                         interaction default to Foreground"
                            .into(),
                    );
                }
                let (sx, sy) = physical_point(window, x, y);
                let dpi = WinBackend::dpi_scale(window);
                let fail = |e: DesktopError| RungDelivery::Failed(e.to_string());
                let uia = WinUia::init().map_err(fail)?;
                uia.send_click(sx, sy).map_err(fail)?;
                RungDelivery::Delivered(format!(
                    "SendInput at physical ({sx},{sy}) — cursor moved, target focused ({})",
                    dpi.describe()
                ))
            }
        }
    }
}

/// The named-element form of the ladder: a name resolves to a control (rung 1)
/// or to a point on that control (rungs 2 and 3). A name that does not resolve
/// is `Unavailable` at every rung, never a guess at the screen centre.
fn deliver_named(
    rung: ClickRung,
    name: &str,
    window: &WindowInfo,
    mode: InteractionMode,
) -> RungDelivery {
    if rung == ClickRung::RawInput && mode == InteractionMode::Background {
        return RungDelivery::Blocked(
            "background contract: SendInput moves the real pointer — switch the interaction \
             default to Foreground"
                .into(),
        );
    }
    let uia = match WinUia::init() {
        Ok(u) => u,
        Err(e) => return RungDelivery::Unavailable(format!("UI Automation unavailable: {e}")),
    };
    // `FIX-17` — the name is resolved against a fresh **bounded, isolated** read
    // and an ambiguous match is a hard stop, not a fall-through to a lower rung:
    // delivering the act to one of two "Save" buttons is a guess, and guessing
    // twice (once for the message rung, once for raw input) is worse. A read that
    // could not see the region returns `Unknown` so the ladder stops with
    // guidance instead of synthesising input into an unknown area.
    let observed = uia.structured_read(window);
    let query = crate::uia::ElementQuery::named(name);
    let point = match crate::uia::resolve_in(&observed.read, &query) {
        crate::uia::Resolution::Resolved(h) => h.element.bounds.center(),
        crate::uia::Resolution::Ambiguous { .. } => {
            return RungDelivery::Unknown(
                crate::uia::resolve_in(&observed.read, &query).describe(),
            );
        }
        crate::uia::Resolution::NotFound { reason, .. } => {
            return match observed.read.status {
                crate::uia::UiaReadStatus::Complete | crate::uia::UiaReadStatus::Absent { .. } => {
                    RungDelivery::Unavailable(format!("no UIA element named \"{name}\" ({reason})"))
                }
                _ => RungDelivery::Unknown(format!(
                    "no UIA element named \"{name}\" could be established: {reason}"
                )),
            };
        }
    };
    // UIA bounding rectangles are screen-space, physical pixels.
    let (cx, cy) = point;
    match rung {
        ClickRung::AccessibilityInvoke => match uia.invoke_at_screen(window, cx, cy) {
            Ok(true) => {
                RungDelivery::Delivered(format!("UIA InvokePattern on \"{name}\" at ({cx},{cy})"))
            }
            Ok(false) => RungDelivery::Unavailable(format!(
                "\"{name}\" exposes no InvokePattern, so the control itself cannot be activated"
            )),
            Err(e) => RungDelivery::Failed(e.to_string()),
        },
        ClickRung::SyntheticEvent => match post_screen_click(window, cx, cy) {
            Ok(()) => RungDelivery::Delivered(format!(
                "WM_LBUTTON pair posted to the target's own HWND for \"{name}\" at ({cx},{cy})"
            )),
            Err(e) => RungDelivery::Failed(e.to_string()),
        },
        ClickRung::RawInput => match uia.send_click(cx, cy) {
            Ok(()) => RungDelivery::Delivered(format!(
                "SendInput at physical ({cx},{cy}) for \"{name}\" — cursor moved, target focused"
            )),
            Err(e) => RungDelivery::Failed(e.to_string()),
        },
    }
}

/// Window-relative physical pixels → physical screen pixels.
///
/// Under per-monitor-v2 awareness this is a plain offset (that is the point of
/// the awareness call). When awareness could not be established, the window
/// rect is in logical units, so the per-window DPI factor is applied through
/// `LogicalToPhysicalPointForPerMonitorDPI` — and falls back to the unconverted
/// offset only if even that is refused, in which case the platform itself could
/// not confirm the space and a fabricated factor would be worse than none.
fn physical_point(window: &WindowInfo, x: i32, y: i32) -> (i32, i32) {
    let aware = WinBackend::ensure_per_monitor_v2();
    if aware {
        return screen_point(window, x, y);
    }
    let mut pt = POINT {
        x: window.x + x,
        y: window.y + y,
    };
    let ok = unsafe { LogicalToPhysicalPointForPerMonitorDPI(hwnd_of(window), &mut pt) };
    if ok.as_bool() {
        (pt.x, pt.y)
    } else {
        (window.x + x, window.y + y)
    }
}

impl WinBackend {
    pub fn list_windows() -> Result<Vec<WindowInfo>, DesktopError> {
        let mut out: Vec<WindowInfo> = Vec::new();
        unsafe {
            EnumWindows(Some(enum_proc), LPARAM(&mut out as *mut _ as isize))
                .map_err(|e| DesktopError::Platform(format!("EnumWindows: {e}")))?;
        }
        Ok(out)
    }

    /// `FIX-18` — capture with the readiness gate **in front of** the capture.
    ///
    /// The order is the fix. The v0 code asked WGC for a frame, took `None` for
    /// any reason at all, and fell through to PrintWindow → screen-DC, so a
    /// target that graphics capture can never see came back as a *successful*
    /// capture with no record of why. Now:
    ///
    /// 1. [`WinCaptureProbe`] verifies host readiness (Session 0, WinRT
    ///    graphics-capture support, a live BGRA-capable D3D11 device) and
    ///    per-target readiness (live handle, non-zero extent, a creatable capture
    ///    item) through [`crate::capture::verify_capture`];
    /// 2. the verdict names the pipeline that will run, the one that was skipped
    ///    and the fault that skipped it;
    /// 3. an `Unavailable` verdict is a **typed** [`DesktopError::CaptureNotReady`]
    ///    — no capture is attempted, and the guidance is actionable;
    /// 4. the verdict travels on the result, so a degrade is a recorded fact
    ///    rather than something a reader has to infer.
    pub fn see(window: &WindowInfo, region: Region) -> Result<SeeResult, DesktopError> {
        let hwnd = hwnd_of(window);
        let dpi = WinBackend::dpi_scale(window);
        let mut probe = WinCaptureProbe::new(hwnd);
        let readiness = crate::capture::verify_capture(&mut probe, window);
        if !readiness.is_capturable() {
            return Err(DesktopError::CaptureNotReady {
                state: readiness.state.as_str(),
                guidance: readiness.guidance.clone(),
                failed_check: readiness
                    .failed_check
                    .map(|c| c.as_str())
                    .unwrap_or("pipeline_supported"),
            });
        }
        let mut rect: RECT = std::mem::zeroed();
        unsafe {
            GetWindowRect(hwnd, &mut rect)
                .map_err(|e| DesktopError::Platform(format!("GetWindowRect: {e}")))?;
        }
        let width = (rect.right - rect.left).max(0) as u32;
        let height = (rect.bottom - rect.top).max(0) as u32;

        // WGC is compositor-native: it composites the window's own surface, so
        // occlusion is irrelevant. Its capture size is the item's, not the rect's,
        // so the dimensions come back from it.
        if readiness.pipeline == Some(CapturePipeline::WindowsGraphicsCapture)
            && let Ok((png, w, h)) = crate::platform::wgc::capture(hwnd)
        {
            let region = if region.is_full(w, h) {
                Region::full(w, h)
            } else {
                region.clamp_to(w, h)
            };
            return Ok(SeeResult {
                window_id: window.id,
                png,
                width: w,
                height: h,
                method: SeeMethod::WindowsGraphicsCapture,
                region,
                scale: dpi.factor,
                dpi,
                // The engine (`DesktopEngine::see`) applies the output budget;
                // a direct backend call has had none applied.
                budget: None,
                readiness,
            });
        }

        // Fallback: PrintWindow renders the window directly (independent of
        // screen occlusion), then BitBlt from the screen DC for popups. The
        // screen-DC leg is only taken when the readiness verdict named it — a
        // screen capture of an occluded window would be a capture of the
        // occluder, and the verdict refuses that case rather than returning
        // another application's pixels.
        let print_window = unsafe { capture_print_window(hwnd, width, height) };
        let (png, method) = match print_window {
            Some(png) => (png, SeeMethod::PrintWindow),
            None if readiness.pipeline == Some(CapturePipeline::ScreenDc) => (
                unsafe { capture_screen_dc(hwnd, width, height) }.ok_or_else(|| {
                    DesktopError::CaptureNotReady {
                        state: "unavailable",
                        guidance: format!(
                            "neither graphics capture nor PrintWindow could return pixels for this \
                             window{}",
                            readiness
                                .fault
                                .as_ref()
                                .map(|f| format!(" ({})", f.guidance()))
                                .unwrap_or_default()
                        ),
                        failed_check: "capture_item",
                    }
                })?,
                SeeMethod::ScreenDc,
            ),
            None => {
                return Err(DesktopError::CaptureNotReady {
                    state: "unavailable",
                    guidance: format!(
                        "no capture pipeline returned pixels for this window; the verified \
                         pipeline was {}",
                        readiness
                            .pipeline
                            .map(|p| p.describe())
                            .unwrap_or_else(|| "none".into())
                    ),
                    failed_check: "capture_item",
                });
            }
        };
        let region = if region.is_full(width, height) {
            Region::full(width, height)
        } else {
            region.clamp_to(width, height)
        };
        Ok(SeeResult {
            window_id: window.id,
            png,
            width,
            height,
            method,
            region,
            scale: dpi.factor,
            dpi,
            budget: None,
            readiness,
        })
    }

    /// P57.4 — the HWND that currently owns the foreground. Real HWNDs are
    /// restorable, so an approved escalation can hand the foreground back.
    pub fn foreground_window() -> Option<u64> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            None
        } else {
            Some(hwnd.0 as u64)
        }
    }

    /// P57.4 — give the foreground back after an approved escalation.
    pub fn restore_foreground(window_id: u64) -> Result<(), DesktopError> {
        let hwnd = HWND(window_id as *mut _);
        let ok = unsafe { SetForegroundWindow(hwnd) };
        if ok.as_bool() {
            Ok(())
        } else {
            // Windows refuses foreground changes from a non-foreground process;
            // say so instead of pretending the restore happened.
            Err(DesktopError::Platform(
                "SetForegroundWindow refused the restore (this process does not own the \
                 foreground)"
                    .into(),
            ))
        }
    }

    /// P57.5 — Session 0 (the services session) has no interactive desktop: a
    /// process there can neither see nor drive a user's windows. Windows exposes
    /// the session name it was started in, so this is a fact, not a guess.
    pub fn interactive_desktop() -> bool {
        !std::env::var("SESSIONNAME")
            .map(|v| v.eq_ignore_ascii_case("Services"))
            .unwrap_or(false)
    }
}

/// PrintWindow with PW_RENDERFULLCONTENT → DIB → PNG.
unsafe fn capture_print_window(hwnd: HWND, width: u32, height: u32) -> Option<Vec<u8>> {
    let dc = GetDC(hwnd);
    if dc.is_invalid() {
        return None;
    }
    let mem = CreateCompatibleDC(dc);
    let bmp = CreateCompatibleBitmap(dc, width as i32, height as i32);
    if bmp.is_invalid() {
        ReleaseDC(hwnd, dc);
        return None;
    }
    let old = SelectObject(mem, bmp);
    // PW_RENDERFULLCONTENT (0x2) asks the window to render its full content —
    // including DirectComposition / hardware-composited surfaces — which is what
    // makes this the occluded-window path rather than a plain client redraw.
    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
    let ok = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
    let mut png = None;
    if ok {
        png = dib_to_png(mem, width, height);
    }
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    ReleaseDC(hwnd, dc);
    png
}

/// Screen-DC BitBlt fallback (captures popups / non-Windows-rendered content).
unsafe fn capture_screen_dc(hwnd: HWND, width: u32, height: u32) -> Option<Vec<u8>> {
    let wdc = GetWindowDC(hwnd);
    if wdc.is_invalid() {
        return None;
    }
    let mem = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, width as i32, height as i32);
    if bmp.is_invalid() {
        ReleaseDC(hwnd, wdc);
        return None;
    }
    let old = SelectObject(mem, bmp);
    let ok = BitBlt(mem, 0, 0, width as i32, height as i32, wdc, 0, 0, SRCCOPY).is_ok();
    let mut png = None;
    if ok {
        png = dib_to_png(mem, width, height);
    }
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    ReleaseDC(hwnd, wdc);
    png
}

/// Copy the DC's bitmap into a BGRA buffer and encode PNG.
unsafe fn dib_to_png(dc: HDC, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut bm: BITMAP = BITMAP::default();
    let bmp = GetCurrentObject(dc, OBJ_BITMAP);
    GetObjectW(
        bmp,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut _),
    );
    let mut info: BITMAPINFO = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width as i32;
    info.bmiHeader.biHeight = -(height as i32); // top-down
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
    let copied = GetDIBits(
        dc,
        HBITMAP(bmp.0),
        0,
        height,
        Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
        &mut info,
        DIB_RGB_COLORS,
    );
    if copied == 0 {
        return None;
    }
    // BGRA → RGBA.
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(width, height, buf)?;
    let mut png = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let mut title = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut title);
    if len == 0 {
        return BOOL(1);
    }
    let title = String::from_utf16_lossy(&title[..len as usize]);
    let mut class = [0u16; 256];
    let clen = GetClassNameW(hwnd, &mut class);
    let class_name = if clen > 0 {
        String::from_utf16_lossy(&class[..clen as usize])
    } else {
        String::new()
    };
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let app = if pid > 0 {
        class_name
    } else {
        "unknown".into()
    };
    let mut rect: RECT = std::mem::zeroed();
    let _ = GetWindowRect(hwnd, &mut rect);
    out.push(WindowInfo {
        id: hwnd.0 as u64,
        title,
        app,
        x: rect.left,
        y: rect.top,
        width: (rect.right - rect.left).max(0) as u32,
        height: (rect.bottom - rect.top).max(0) as u32,
        has_a11y_tree: true,
    });
    BOOL(1)
}

/// SendInput text typing (per-char keydown/keyup with scan codes).
pub fn send_input_type(text: &str) -> Result<(), DesktopError> {
    for c in text.chars() {
        if c == '\n' {
            press_vk(13)?;
            continue;
        }
        if !c.is_ascii() {
            return Err(DesktopError::Platform(format!("no VK for {c:?}")));
        }
        let scan = unsafe { MapVirtualKeyA(c as u32, MAPVK_VK_TO_VSC) };
        unsafe {
            let down = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: scan as u16,
                        dwFlags: KEYEVENTF_SCANCODE,
                        ..Default::default()
                    },
                },
            };
            let up = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: scan as u16,
                        dwFlags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
                        ..Default::default()
                    },
                },
            };
            SendInput(&[down], std::mem::size_of::<INPUT>() as i32);
            SendInput(&[up], std::mem::size_of::<INPUT>() as i32);
        }
    }
    Ok(())
}

fn press_vk(vk: u16) -> Result<(), DesktopError> {
    unsafe {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: KEYEVENTF_EXTENDEDKEY,
                    ..Default::default()
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        };
        SendInput(&[down], std::mem::size_of::<INPUT>() as i32);
        SendInput(&[up], std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

/// P57.1 — launch through the shell, with the show-state the interaction
/// default requires: `SW_SHOWNOACTIVATE` on the Background path so the program
/// appears without becoming the foreground window (the Cua contract),
/// `SW_SHOWNORMAL` only when the user switched to Foreground. `lpFile` is the
/// canonical path (a bare name stays a Shell fallback, which is what the spec
/// allows for a name with no resolved path).
fn shell_launch(target: &str, mode: InteractionMode) -> Result<(), DesktopError> {
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: windows::core::PCWSTR(verb.as_ptr()),
        lpFile: windows::core::PCWSTR(file.as_ptr()),
        nShow: if mode == InteractionMode::Background {
            SW_SHOWNOACTIVATE.0 as i32
        } else {
            SW_SHOWNORMAL.0 as i32
        },
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)
            .map_err(|e| DesktopError::Platform(format!("ShellExecuteEx {target}: {e}")))?;
    }
    Ok(())
}

/// Act dispatch: UIA first (invoke/set-value), SendInput for the rest.
pub fn act(
    window: &WindowInfo,
    act: &ActKind,
    uia: Option<&WinUia>,
    mode: InteractionMode,
) -> Result<(), DesktopError> {
    // P57.1 — resolve the launch target before anything else so a bad path is
    // refused without touching the shell.
    if let ActKind::LaunchApp { path, app } = act {
        let target = launch::resolve_target(path.as_deref(), app, &launch::path_dirs())?;
        return shell_launch(&target.to_string_lossy(), mode);
    }
    match act {
        ActKind::ClickByName { name } => {
            let u = match uia {
                Some(u) => u,
                None => &WinUia::init()?,
            };
            u.click_by_name(window, name)
        }
        ActKind::SetValue { name, value } => {
            let u = match uia {
                Some(u) => u,
                None => &WinUia::init()?,
            };
            // `FIX-17` — `ValuePattern::SetValue` on a name resolved through the
            // bounded collector, with the same ambiguity/elevation discipline as a
            // click: an ambiguous name is refused, and a read that could not see
            // the region produces no coordinate at all.
            u.set_value_by_name(window, name, value)
        }
        ActKind::Click { .. } => {
            // P69.G4 — a coordinate click is now **ladder-owned**
            // (`DesktopEngine` walks `ClickRung` through `deliver_rung`),
            // so reaching this arm means a caller bypassed the engine. Refuse
            // rather than keep a second, ungated copy of the escalation: the
            // engine is the only place that can ask Guard about the
            // pointer-moving rung.
            Err(DesktopError::Unsupported(
                "coordinate clicks go through the click ladder (DesktopEngine::act_with) — \
                 this backend arm has no gate on the pointer-moving rung and is refused"
                    .into(),
            ))
        }
        ActKind::Type { text } => send_input_type(text),
        ActKind::Press { key } => {
            let vk = match key.to_ascii_lowercase().as_str() {
                "enter" | "return" => 13,
                "tab" => 9,
                "escape" | "esc" => 27,
                "backspace" => 8,
                "delete" => 46,
                "space" => 32,
                "left" => 37,
                "up" => 38,
                "right" => 39,
                "down" => 40,
                "home" => 36,
                "end" => 35,
                "pageup" => 33,
                "pagedown" => 34,
                other => {
                    let b = other.as_bytes();
                    if b.len() == 1 && b[0].is_ascii_alphabetic() {
                        b[0].to_ascii_uppercase() as u16
                    } else {
                        return Err(DesktopError::Platform(format!("unknown key {key}")));
                    }
                }
            };
            press_vk(vk)
        }
        ActKind::Scroll { x, y, delta } => unsafe {
            // P57.4 — scrolling is pointer motion + a wheel event, so it is a
            // foreground escalation under the background contract.
            if mode == InteractionMode::Background {
                return Err(DesktopError::Unsupported(
                    "background contract: scrolling moves the real pointer — switch the \
                     interaction default to Foreground"
                        .into(),
                ));
            }
            let (sx, sy) = screen_point(window, *x, *y);
            SetCursorPos(sx, sy)
                .map_err(|e| DesktopError::Platform(format!("SetCursorPos: {e}")))?;
            let amount = (*delta).clamp(-120, 120);
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, amount, 0);
            Ok(())
        },
        ActKind::Drag { from, to } => unsafe {
            // P57.4 — a drag is pointer motion by definition.
            if mode == InteractionMode::Background {
                return Err(DesktopError::Unsupported(
                    "background contract: dragging moves the real pointer — switch the \
                     interaction default to Foreground"
                        .into(),
                ));
            }
            let (fx, fy) = screen_point(window, from.0, from.1);
            let (tx, ty) = screen_point(window, to.0, to.1);
            SetCursorPos(fx, fy)
                .map_err(|e| DesktopError::Platform(format!("SetCursorPos: {e}")))?;
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            for i in 1..=8 {
                let t = i as f64 / 8.0;
                let x = (fx as f64 + (tx - fx) as f64 * t) as i32;
                let y = (fy as f64 + (ty - fy) as f64 * t) as i32;
                if SetCursorPos(x, y).is_err() {
                    break;
                }
            }
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            Ok(())
        },
        ActKind::ActivateWindow { window_id } => {
            // P57.3 — `SetForegroundWindow` + `SW_RESTORE` is exactly the raise
            // the Background contract forbids. It happens only on the
            // Foreground path (the engine refuses earlier for the policy path;
            // this is the backend's own floor).
            if mode == InteractionMode::Background {
                return Err(DesktopError::Unsupported(
                    "background contract: SetForegroundWindow/ShowWindow is a foreground \
                     escalation — switch the interaction default to Foreground"
                        .into(),
                ));
            }
            unsafe {
                let hwnd = HWND(*window_id as usize as *mut core::ffi::c_void);
                let _ = SetForegroundWindow(hwnd);
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            Ok(())
        }
        // Handled above (before the UIA/SendInput paths).
        ActKind::LaunchApp { .. } => Err(DesktopError::Platform(
            "launch was not handled on the pre-input path".into(),
        )),
    }
}

// P57.6 — Windows.Graphics.Capture (occluded capture) lives in
// [`crate::platform::wgc`]; `see()` above calls it first and falls back here.

/// The DWM compositor step, tested without a Windows host: the verdict is a
/// pure function of the injected flag, so the mapping (enabled → named check,
/// disabled/failed call → typed fault with guidance) is proven here while the
/// raw `DwmIsCompositionEnabled` call itself is Windows-only. (This module is
/// `#[cfg(windows)]`-gated, so these tests run on a Windows runner, not on
/// Linux — the Linux-runnable half is `tests/acceptance_dwm_compositor.rs`,
/// which proves the same verdict shape through `verify_capture` with a fake.)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwm_enabled_reports_the_named_compositor_check() {
        assert_eq!(
            compositor_verdict(Ok(true)),
            Ok(CaptureCheck::CompositorRunning)
        );
    }

    #[test]
    fn dwm_disabled_surfaces_the_typed_fault_with_its_guidance() {
        let fault =
            compositor_verdict(Ok(false)).expect_err("composition off must refuse the check");
        assert!(matches!(fault, CaptureFault::NoCompositor { .. }));
        assert_eq!(fault.check(), CaptureCheck::CompositorRunning);
        assert!(!fault.guidance().is_empty(), "{fault:?}");
    }

    #[test]
    fn a_failing_dwm_call_is_a_compositor_fault_not_a_probe_failure() {
        let fault =
            compositor_verdict(Err("E_FAIL".into())).expect_err("a failed call must refuse");
        assert!(matches!(fault, CaptureFault::NoCompositor { .. }));
        assert_eq!(fault.check(), CaptureCheck::CompositorRunning);
        assert!(fault.guidance().contains("compositor"), "{fault:?}");
    }
}
