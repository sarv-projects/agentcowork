//! P57.6 — Windows Graphics Capture (WGC) occluded-window capture.
//!
//! PrintWindow (`PW_RENDERFULLCONTENT`) redraws a window into a DC and the
//! screen-DC BitBlt can only ever return what is actually on screen, so an
//! occluded (or parked-behind-another-window) target comes back as the occluder
//! or as black. Windows.Graphics.Capture instead composites the window's own
//! surface, so occlusion does not matter.
//!
//! This module is the whole WGC path:
//!   1. a BGRA-capable D3D11 device, wrapped as a WinRT `IDirect3DDevice`;
//!   2. `GraphicsCaptureItem::TryCreateFromWindowId` for the target HWND;
//!   3. a free-threaded `Direct3D11CaptureFramePool` + capture session;
//!   4. the first frame's `IDirect3DSurface` → `ID3D11Texture2D`;
//!   5. copy to a CPU-readable staging texture, map, BGRA→RGBA, encode PNG.
//!
//! Everything is **fail-closed**: any missing step returns `None` and the
//! caller falls back to PrintWindow → screen-DC. `available()` is a real probe
//! (WinRT support + a live D3D11 device), not a platform guess, so
//! `Capabilities::see_occluded_wgc` never promises a capture the host would
//! return black for.
//!
//! Verification: the module is cross-compile-checked for
//! `x86_64-pc-windows-msvc` in CI (it cannot run on this host), so the code is
//! proven to build against the real `windows` crate surface but runtime
//! evidence is pending a Windows runner.

#![cfg(windows)]

use std::sync::OnceLock;

use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::{IDirect3DDevice, IDirect3DSurface};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::UI::WindowId;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::core::Interface;

/// Frames to try before giving up: WGC delivers asynchronously, so the first
/// `TryGetNextFrame` right after `StartCapture` can legitimately be empty.
const FRAME_ATTEMPTS: u32 = 40;
const FRAME_POLL_MS: u64 = 25;

/// Is WGC usable on this host right now? WinRT support plus a real BGRA-capable
/// D3D11 device. Cached: the probe creates a device, which is not free.
///
/// Retained as a `bool` for the capability surface; the *reasons* live in
/// [`session_supported`] / [`device_available`] / [`item_available`], which
/// `FIX-18` needs so a refusal can name what failed (see `WinCaptureProbe` in
/// [`crate::platform::win`]).
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| session_supported() && device_available())
}

/// Is `GraphicsCaptureSession` supported on this host?
///
/// The WinRT session check on its own. Separate from [`device_available`] so the
/// readiness verdict can say *which* of the two failed — "graphics capture is not
/// supported here" and "no BGRA-capable device" need different remedies.
pub fn session_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| GraphicsCaptureSession::IsSupported().unwrap_or(false))
}

/// Can a BGRA-capable D3D11 device be created right now?
///
/// A device that cannot be created is a device graphics capture cannot use
/// either (headless GPU, a session with no adapter, WARP disabled).
pub fn device_available() -> bool {
    static DEVICE: OnceLock<bool> = OnceLock::new();
    *DEVICE.get_or_init(|| unsafe { create_device().is_ok() })
}

/// Can a capture item be created for this window? (`FIX-18`'s per-target check.)
///
/// This is the check the v0 code did not have: `GraphicsCaptureItem::
/// TryCreateFromWindowId` failing is the difference between "graphics capture is
/// unavailable on this host" and "graphics capture cannot see *this* window", and
/// only the first is worth falling back from silently.
///
/// `Err(detail)` names the HRESULT so a receipt can say what the compositor
/// refused.
pub fn item_available(hwnd: HWND) -> Result<(), String> {
    GraphicsCaptureItem::TryCreateFromWindowId(WindowId {
        Value: hwnd.0 as u64,
    })
    .map(|item| {
        // The item is dropped here on purpose: creating it *is* the check, and
        // holding one per probe would leak a compositor reference per readiness
        // call.
        drop(item);
    })
    .map_err(|e| format!("GraphicsCaptureItem::TryCreateFromWindowId failed: {e}"))
}

/// Capture `hwnd` through WGC.
///
/// `FIX-18`: the return type is a typed `Result`, not `Option`. `None` used to
/// mean six different things at once (no session, no device, no item, no frame,
/// a bad map, an encode failure), and every one of them silently degraded to
/// PrintWindow. The verdict is now computed by `WinCaptureProbe` *before* this is
/// called, so a failure here is a real failure rather than a fallback trigger.
///
/// The capture size is the item's own (WGC renders the window's client surface,
/// which is authoritative), so the caller uses those dimensions rather than the
/// window rect.
pub fn capture(hwnd: HWND) -> Result<(Vec<u8>, u32, u32), String> {
    unsafe { capture_inner(hwnd) }
}

unsafe fn capture_inner(hwnd: HWND) -> Result<(Vec<u8>, u32, u32), String> {
    // SAFETY: The caller has validated the HWND; WinRT/D3D objects are owned
    // for this call and all mapped texture memory is unmapped before return.
    unsafe {
        let (device, context, rt_device) =
            create_device().map_err(|e| format!("D3D11 device: {e}"))?;
        let item = GraphicsCaptureItem::TryCreateFromWindowId(WindowId {
            Value: hwnd.0 as u64,
        })
        .map_err(|e| format!("GraphicsCaptureItem: {e}"))?;
        let size = item.Size().map_err(|e| format!("capture item size: {e}"))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(format!(
                "the capture item reports no extent ({}x{})",
                size.Width, size.Height
            ));
        }
        let width = size.Width as u32;
        let height = size.Height as u32;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &rt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            SizeInt32 {
                Width: size.Width,
                Height: size.Height,
            },
        )
        .map_err(|e| format!("Direct3D11CaptureFramePool: {e}"))?;
        let session: GraphicsCaptureSession = pool
            .CreateCaptureSession(&item)
            .map_err(|e| format!("CreateCaptureSession: {e}"))?;
        // A capture is for content, not for the user's cursor.
        let _ = session.SetIsCursorCaptureEnabled(false);
        session
            .StartCapture()
            .map_err(|e| format!("StartCapture: {e}"))?;

        let mut frame = None;
        for _ in 0..FRAME_ATTEMPTS {
            if let Ok(f) = pool.TryGetNextFrame() {
                frame = Some(f);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(FRAME_POLL_MS));
        }
        let Some(frame) = frame else {
            return Err(format!(
                "no frame arrived within {} attempts \u{d7} {}ms",
                FRAME_ATTEMPTS, FRAME_POLL_MS
            ));
        };
        let surface: IDirect3DSurface =
            frame.Surface().map_err(|e| format!("frame surface: {e}"))?;
        let _ = frame.Close();
        let _ = session.Close();
        let _ = pool.Close();

        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .map_err(|e| format!("surface \u{2192} DXGI access: {e}"))?;
        let texture: ID3D11Texture2D = access
            .GetInterface()
            .map_err(|e| format!("DXGI access \u{2192} ID3D11Texture2D: {e}"))?;
        let png = copy_and_encode(&device, &context, &texture, width, height)
            .ok_or_else(|| "the captured surface could not be read back or encoded".to_string())?;
        Ok((png, width, height))
    }
}

/// A BGRA-capable D3D11 device + immediate context, wrapped as the WinRT
/// `IDirect3DDevice` the frame pool needs.
unsafe fn create_device()
-> windows::core::Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
    // SAFETY: D3D11CreateDevice initializes the optional COM interfaces below;
    // each returned interface is checked before it is used.
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut D3D_FEATURE_LEVEL::default()),
            Some(&mut context),
        )?;
        let device = device.ok_or_else(windows::core::Error::from_win32)?;
        let context = context.ok_or_else(windows::core::Error::from_win32)?;
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast()?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?;
        let rt_device: IDirect3DDevice = inspectable.cast()?;
        Ok((device, context, rt_device))
    }
}

/// Copy the GPU texture into a CPU-readable staging texture, map it, and encode
/// the BGRA rows as PNG (RGBA).
unsafe fn copy_and_encode(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    // SAFETY: `src` is a live texture from the captured frame; `mapped` is
    // checked for a non-null pointer and adequate row pitch before reading.
    // Each row/pixel offset is bounded by the validated dimensions and pitch.
    unsafe {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        src.GetDesc(&mut src_desc);

        let mut staging_desc = src_desc;
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.BindFlags = 0;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        staging_desc.MiscFlags = 0;
        staging_desc.MipLevels = 1;
        staging_desc.ArraySize = 1;
        staging_desc.SampleDesc.Count = 1;
        staging_desc.SampleDesc.Quality = 0;

        let mut staging: Option<ID3D11Texture2D> = None;
        device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .ok()?;
        let staging = staging?;
        let dst_res: ID3D11Resource = staging.cast().ok()?;
        let src_res: ID3D11Resource = src.cast().ok()?;
        context.CopyResource(&dst_res, &src_res);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&dst_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .ok()?;

        let row_pitch = mapped.RowPitch as usize;
        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
        if mapped.pData.is_null() || row_pitch < (width as usize) * 4 {
            context.Unmap(&dst_res, 0);
            return None;
        }
        for y in 0..height as usize {
            let src_row = (mapped.pData as *const u8).add(y * row_pitch);
            for x in 0..width as usize {
                let p = src_row.add(x * 4);
                // BGRA (little-endian) → RGBA
                let b = *p;
                let g = *p.add(1);
                let r = *p.add(2);
                let a = *p.add(3);
                let o = (y * (width as usize) + x) * 4;
                rgba[o] = r;
                rgba[o + 1] = g;
                rgba[o + 2] = b;
                rgba[o + 3] = a;
            }
        }
        context.Unmap(&dst_res, 0);

        let img = image::RgbaImage::from_raw(width, height, rgba)?;
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .ok()?;
        Some(out.into_inner())
    }
}
