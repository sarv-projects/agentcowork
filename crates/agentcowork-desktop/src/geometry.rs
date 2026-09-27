//! Coordinate normalization, patch-aligned quantization, and the **honest
//! output budget** for everything the desktop engine hands a model.
//!
//! Three separate problems live here, and they are kept apart on purpose:
//!
//! 1. **DPI normalization** ([`DpiScale`]) — a coordinate the caller supplies is
//!    in *window-relative physical pixels*; a coordinate a platform's own API
//!    takes is in whatever space that API uses (Windows screen coords are
//!    physical once the process is per-monitor-v2 aware, macOS System Events
//!    takes **points**, X11 takes device pixels). A hardcoded `1.0` is wrong on
//!    every scaled display, so the factor is measured per platform and its
//!    **provenance is carried with it** — [`DpiSource::Unknown`] is a real,
//!    reportable answer, not a disguised `1.0`.
//!
//! 2. **Patch-aligned quantization** ([`IMAGE_FACTOR`]) — what the model is
//!    *shown* is snapped to the vision encoder's patch grid, so a control's
//!    box does not jump between turns. See [`IMAGE_FACTOR`] for the rationale
//!    and [`SeeBudget`] for what is actually reported.
//!
//! 3. **The output budget** ([`OutputBudget`]) — a real ceiling with a real
//!    refusal, not a silent downscale. A capture is clamped to
//!    [`SEE_MAX_DIMENSION_PX`] and re-encoded, the result **says** it clamped
//!    ([`BudgetDisposition`]), and a payload that is still over
//!    [`SEE_MAX_BYTES`] after the clamp is **refused** rather than shipped.

use serde::{Deserialize, Serialize};

use crate::DesktopError;

/// **Patch-alignment factor, in pixels.**
///
/// 28 px is the token cell of the vision encoders this surface feeds: a 14 px
/// ViT patch (`SigLIP`/`SmolVLM` 384, `InternVL`) merged 2×2, which is what
/// `Qwen2-VL`-family grounding models use. Snapping the **returned image
/// edges** and every **reported bounding box** to that grid keeps a control in
/// the same sub-cell from one turn to the next: without it, a window that
/// changes size by one pixel re-derives the whole resize grid and every
/// coordinate the model predicted last turn becomes off by a fraction of a
/// token.
///
/// This is applied to what the model is *shown*, never to a delivered click
/// point — see [`quantize_box`] for why.
pub const IMAGE_FACTOR: u32 = 28;

/// **Longest edge, in pixels, of an image returned to the model.** Named ceiling
/// for the see/screenshot surface.
///
/// 1280 px bounds a capture at 1.64 Mpx, which is inside the resample budget of
/// the common vision encoders (1.0–2.2 Mpx) with headroom, and keeps a frame
/// inside one tool-result payload. A larger capture spends the same context on
/// pixels that resolve no additional control: a desktop button is tens of
/// pixels across at 1.0 scale, and ~50 px at 2.0 — still comfortably resolvable
/// at 1280 on the long edge.
pub const SEE_MAX_DIMENSION_PX: u32 = 1280;

/// **Largest encoded payload, in bytes, an image returned to the model may
/// have.** Named ceiling for the see/screenshot surface.
///
/// The architecture states "≤ 900 KB"; this is the KiB reading of that figure
/// (921 600 bytes) and the reading is stated here rather than left implicit.
/// It bounds the base64/IPC form of the payload to ~1.2 MB, which is one
/// tool-result budget's worth. A capture that cannot be brought under both
/// this and [`SEE_MAX_DIMENSION_PX`] is **refused** — see
/// [`enforce_output_budget`].
pub const SEE_MAX_BYTES: usize = 900 * 1024;

/// Where a [`DpiScale`] factor came from. Carried so `1.0` is never
/// indistinguishable from "not measured".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DpiSource {
    /// Windows per-monitor-v2: `GetDpiForWindow` on the target HWND, with the
    /// process set per-monitor-v2 aware so Win32 stops virtualising
    /// coordinates.
    PerMonitorV2,
    /// macOS main-display backing scale (`CGMainDisplayPixelsWide` over
    /// `CGMainDisplayPointsWide`).
    BackingStore,
    /// X11 `Xft.dpi` on the root window. This is a **global font hint**, not a
    /// per-window scale — see [`DpiScale::applies_to_click_coordinates`].
    XftProperty,
    /// The platform could not be asked. The factor is 1.0 and this says so, so
    /// no caller can mistake a default for a measurement.
    #[default]
    Unknown,
}

impl DpiSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            DpiSource::PerMonitorV2 => "per_monitor_v2",
            DpiSource::BackingStore => "backing_store",
            DpiSource::XftProperty => "xft_dpi",
            DpiSource::Unknown => "unknown",
        }
    }
}

/// A measured (or honestly-unmeasured) DPI scale factor.
///
/// The factor is always in the physical-pixels-per-logical-unit direction: a
/// 1.25 display gives `1.25`, a Retina 2.0 display gives `2.0`. It is clamped
/// to a physically plausible `[0.5, 8.0]` band so a corrupt platform answer
/// cannot turn a click into a wild point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DpiScale {
    pub factor: f64,
    pub source: DpiSource,
}

/// The DPI base every platform's scale is expressed against (96 DPI = 1.0).
pub const DPI_BASE: f64 = 96.0;

/// Lowest factor accepted from a platform query (a 0.5× display is unusual but
/// real; below this a query is corrupt).
const DPI_MIN_FACTOR: f64 = 0.5;
/// Highest factor accepted (8× is far past any shipping display; beyond this a
/// query is corrupt and would misplace a click by an order of magnitude).
const DPI_MAX_FACTOR: f64 = 8.0;

impl Default for DpiScale {
    fn default() -> Self {
        Self::unknown()
    }
}

impl DpiScale {
    /// The honest "not measured" answer: factor 1.0, `source: Unknown`.
    pub fn unknown() -> Self {
        Self {
            factor: 1.0,
            source: DpiSource::Unknown,
        }
    }

    /// Build from a raw platform DPI value (e.g. `GetDpiForWindow`'s 120).
    /// A zero/implausible value degrades to [`DpiScale::unknown`] rather than
    /// being trusted.
    pub fn from_dpi(dpi: u32, source: DpiSource) -> Self {
        if dpi == 0 {
            return Self::unknown();
        }
        Self::from_factor(f64::from(dpi) / DPI_BASE, source)
    }

    /// Build from a raw scale factor, sanitising the answer.
    pub fn from_factor(factor: f64, source: DpiSource) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return Self::unknown();
        }
        let clamped = factor.clamp(DPI_MIN_FACTOR, DPI_MAX_FACTOR);
        if (clamped - factor).abs() > f64::EPSILON {
            // An out-of-band answer is not a measurement we can act on.
            return Self::unknown();
        }
        Self {
            factor: clamped,
            source,
        }
    }

    /// Was this actually measured, or is it the honest default?
    pub fn is_measured(&self) -> bool {
        self.source != DpiSource::Unknown
    }

    /// Is this the identity (a 1.0 display, or unmeasured)?
    pub fn is_identity(&self) -> bool {
        (self.factor - 1.0).abs() < f64::EPSILON
    }

    /// Logical units → physical pixels, with the rounding rule
    /// `LogicalToPhysicalPointForPerMonitorDPI` uses (round-half-away-from-zero
    /// on `.5`).
    pub fn to_physical(&self, logical: f64) -> f64 {
        (logical * self.factor).round()
    }

    /// Physical pixels → logical units.
    pub fn to_logical(&self, physical: f64) -> f64 {
        physical / self.factor
    }

    /// Logical → physical for an integer point.
    pub fn point_to_physical(&self, x: i32, y: i32) -> (i32, i32) {
        (
            self.to_physical(f64::from(x)) as i32,
            self.to_physical(f64::from(y)) as i32,
        )
    }

    /// Does this factor legitimately belong in a **click coordinate**?
    ///
    /// `PerMonitorV2` and `BackingStore` do: both describe the coordinate space
    /// the platform's own input API takes. `XftProperty` does **not** — X11
    /// input (`XTEST`, `SendEvent`) and `XGetImage` all address *device*
    /// pixels regardless of `Xft.dpi`, so dividing a click by `Xft.dpi/96`
    /// would misplace it. This is the difference between an honest report and
    /// a wrong click, so it is a method rather than a comment.
    pub fn applies_to_click_coordinates(&self) -> bool {
        matches!(
            self.source,
            DpiSource::PerMonitorV2 | DpiSource::BackingStore
        )
    }

    /// The one-line form an audit row or a screenshot projection carries.
    pub fn describe(&self) -> String {
        if self.is_identity() {
            if self.is_measured() {
                return format!(
                    "{} factor 1.0 (no scaling on this display)",
                    self.source.as_str()
                );
            }
            return "dpi not measured (factor 1.0)".into();
        }
        format!("{} factor {:.2}", self.source.as_str(), self.factor)
    }
}

/// Floor a value to the patch grid, never below 1 px.
pub fn align_to_patch(value: u32) -> u32 {
    let aligned = (value / IMAGE_FACTOR) * IMAGE_FACTOR;
    aligned.max(1)
}

/// Snap a bounding box onto the patch grid, in the coordinate space it is
/// expressed in.
///
/// **Not** applied to a delivered click point, deliberately. Snapping a point
/// moves it by up to half a patch (14 px), which is larger than a small
/// control — it would turn a correct prediction into a miss. Patch alignment is
/// for what the model is *shown* (image edges + the boxes it reads), so the
/// predicted point lands in a stable sub-cell instead.
pub fn quantize_box(region: crate::types::Region) -> crate::types::Region {
    let f = IMAGE_FACTOR as i32;
    let x = (region.x / f) * f;
    let y = (region.y / f) * f;
    let x1 = ((region.x + region.width as i32) / f) * f;
    let y1 = ((region.y + region.height as i32) / f) * f;
    let width = (x1 - x).max(f);
    let height = (y1 - y).max(f);
    crate::types::Region {
        x,
        y,
        width: width.max(0) as u32,
        height: height.max(0) as u32,
    }
}

/// What the engine did to a capture to bring it inside the budget.
///
/// A distinct `AsCaptured` is the point: the UI and the audit can tell "passed
/// through untouched" from "I was transformed", and neither can be confused
/// with a silent lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "fields")]
pub enum BudgetDisposition {
    /// Returned byte-for-byte: already inside both ceilings and patch-aligned.
    AsCaptured,
    /// Re-encoded. `dimension` = the long edge exceeded
    /// [`SEE_MAX_DIMENSION_PX`]; `aligned` = the returned edges were moved onto
    /// the patch grid. Both may be true.
    Clamped { dimension: bool, aligned: bool },
}

impl BudgetDisposition {
    /// Did the engine transform the capture? (The result must be able to say.)
    pub fn clamped(&self) -> bool {
        matches!(self, BudgetDisposition::Clamped { .. })
    }
}

/// The budget actually applied to one capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeeBudget {
    /// What the platform captured, before any clamp.
    pub captured_width: u32,
    pub captured_height: u32,
    /// What is being returned.
    pub output_width: u32,
    pub output_height: u32,
    /// output / captured, per axis. A coordinate in the returned image maps
    /// back to a window coordinate by dividing.
    pub output_scale_x: f64,
    pub output_scale_y: f64,
    /// Whether the capture was transformed to fit the budget.
    pub disposition: BudgetDisposition,
    /// The patch-alignment factor the returned edges are on.
    pub alignment: u32,
    /// The named ceilings this was measured against.
    pub max_dimension_px: u32,
    pub max_bytes: usize,
    /// The returned payload size.
    pub bytes: usize,
}

impl SeeBudget {
    /// Map a point in the **returned image** back to a **window-relative**
    /// point. This is the honest inverse of a clamp: a model that predicts
    /// (700, 400) on a downscaled capture is aiming at the same control as
    /// (700, 400) on a full-size one.
    pub fn image_point_to_window(&self, x: i32, y: i32) -> (i32, i32) {
        let sx = if self.output_scale_x > 0.0 {
            self.output_scale_x
        } else {
            1.0
        };
        let sy = if self.output_scale_y > 0.0 {
            self.output_scale_y
        } else {
            1.0
        };
        (
            (f64::from(x) / sx).round() as i32,
            (f64::from(y) / sy).round() as i32,
        )
    }

    /// A one-line human summary for an audit row or a screenshot caption.
    pub fn describe(&self) -> String {
        let base = format!(
            "{}x{} → {}x{} px, {} bytes (ceiling {} px / {} bytes, patch {})",
            self.captured_width,
            self.captured_height,
            self.output_width,
            self.output_height,
            self.bytes,
            self.max_dimension_px,
            self.max_bytes,
            self.alignment,
        );
        match self.disposition {
            BudgetDisposition::AsCaptured => format!("{base} — unchanged"),
            BudgetDisposition::Clamped { dimension, aligned } => {
                let mut why: Vec<&str> = Vec::new();
                if dimension {
                    why.push("over the max dimension");
                }
                if aligned {
                    why.push("re-aligned to the patch grid");
                }
                format!("{base} — clamped ({})", why.join(" + "))
            }
        }
    }
}

/// The named, stated ceiling for the see/screenshot surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputBudget {
    pub max_dimension_px: u32,
    pub max_bytes: usize,
    pub alignment: u32,
}

impl Default for OutputBudget {
    fn default() -> Self {
        Self {
            max_dimension_px: SEE_MAX_DIMENSION_PX,
            max_bytes: SEE_MAX_BYTES,
            alignment: IMAGE_FACTOR,
        }
    }
}

impl OutputBudget {
    /// The shipped ceiling.
    pub fn shipped() -> Self {
        Self::default()
    }
}

/// Bring one captured image inside `budget`.
///
/// The rule, in order:
/// 1. If the long edge is over the ceiling, scale it down **and** land the
///    result on the patch grid (so a model reading the image and a model
///    reading the previous turn's image share a grid).
/// 2. If the long edge is inside the ceiling but the edges are off the patch
///    grid *and* the image is at least one patch across, re-align (shrink only —
///    never up-scale a small capture).
/// 3. If the resulting PNG is still over `max_bytes`, **refuse**: the error
///    carries the real numbers and names region zoom as the remedy. Refusing is
///    the fail-closed choice; a silently truncated or oversized payload is the
///    dishonesty this ceiling exists to prevent.
pub fn enforce_output_budget(
    png: Vec<u8>,
    width: u32,
    height: u32,
    budget: &OutputBudget,
) -> Result<(Vec<u8>, u32, u32, SeeBudget), DesktopError> {
    let captured_width = width;
    let captured_height = height;

    let long = width.max(height);
    let mut target = (width, height);
    let mut dimension_clamped = false;
    if long > budget.max_dimension_px && long > 0 {
        let scale = f64::from(budget.max_dimension_px) / f64::from(long);
        let scaled_w = ((f64::from(width) * scale).floor() as u32).max(1);
        let scaled_h = ((f64::from(height) * scale).floor() as u32).max(1);
        target = (scaled_w, scaled_h);
        dimension_clamped = true;
    }

    // Patch alignment. Only ever shrinks, and only when the image is at least
    // one patch across on both axes (below that, alignment is vacuous and a
    // "snap" would just destroy the capture).
    let alignable = target.0 >= budget.alignment && target.1 >= budget.alignment;
    if alignable {
        let aligned_w = align_to_patch(target.0);
        let aligned_h = align_to_patch(target.1);
        if aligned_w != target.0 || aligned_h != target.1 {
            target = (aligned_w, aligned_h);
        }
    }
    // A dimension clamp can land off-grid when the scaled edge is under one
    // patch; re-align and re-clamp so the ceiling is never exceeded.
    if dimension_clamped || target.0 > budget.max_dimension_px || target.1 > budget.max_dimension_px
    {
        target.0 = target.0.min(budget.max_dimension_px).max(1);
        target.1 = target.1.min(budget.max_dimension_px).max(1);
    }
    let aligned = target != (width, height);
    let needs_resample = dimension_clamped || aligned;

    let (out_png, out_w, out_h) = if needs_resample {
        let img = image::load_from_memory(&png)
            .map_err(|e| DesktopError::Platform(format!("decode capture for budget: {e}")))?;
        // The clamped size is computed from the *captured* dimensions; resize
        // from the decoded image directly so the two can never disagree.
        let resized = image::imageops::resize(
            &img,
            target.0,
            target.1,
            image::imageops::FilterType::Triangle,
        );
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut out);
        resized
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| DesktopError::Platform(format!("re-encode clamped capture: {e}")))?;
        (out, target.0, target.1)
    } else {
        (png, width, height)
    };

    if out_png.len() > budget.max_bytes {
        // Fail closed. The caller can narrow the region; it must not be handed
        // an over-budget payload and told nothing.
        return Err(DesktopError::OutputBudget(format!(
            "see/screenshot refused: {} bytes exceeds the {} byte ceiling (and the \
             {}x{} px capture is already at or below the {} px dimension ceiling, so \
             resizing cannot help) — capture a smaller region (see/region zoom)",
            out_png.len(),
            budget.max_bytes,
            width,
            height,
            budget.max_dimension_px,
        )));
    }

    let disposition = if needs_resample {
        BudgetDisposition::Clamped {
            dimension: dimension_clamped,
            aligned,
        }
    } else {
        BudgetDisposition::AsCaptured
    };
    let report = SeeBudget {
        captured_width,
        captured_height,
        output_width: out_w,
        output_height: out_h,
        output_scale_x: if width > 0 {
            f64::from(out_w) / f64::from(width)
        } else {
            1.0
        },
        output_scale_y: if height > 0 {
            f64::from(out_h) / f64::from(height)
        } else {
            1.0
        },
        disposition,
        alignment: budget.alignment,
        max_dimension_px: budget.max_dimension_px,
        max_bytes: budget.max_bytes,
        bytes: out_png.len(),
    };
    Ok((out_png, out_w, out_h, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Region;

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([40, 90, 160, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    // ---- DPI ----------------------------------------------------------

    #[test]
    fn unknown_dpi_is_one_and_says_so() {
        let d = DpiScale::unknown();
        assert_eq!(d.factor, 1.0);
        assert!(!d.is_measured());
        assert!(d.is_identity());
        assert!(d.describe().contains("not measured"));
    }

    #[test]
    fn dpi_conversion_rounds_and_round_trips() {
        for factor in [1.25_f64, 1.5, 2.0, 1.75] {
            let d = DpiScale::from_factor(factor, DpiSource::PerMonitorV2);
            assert!(d.is_measured());
            assert!((d.factor - factor).abs() < 1e-9);
            // 1.25 * 20 = 25.0 exactly; 1.5 * 5 = 7.5 → 8 (half away from zero).
            let (px, py) = d.point_to_physical(20, 5);
            assert_eq!(px, (20.0 * factor).round() as i32);
            assert_eq!(py, (5.0 * factor).round() as i32);
            let logical = d.to_logical(f64::from(px));
            assert!((logical - 20.0).abs() < 1e-6, "round trip: {logical}");
        }
        assert_eq!(
            DpiScale::from_factor(1.5, DpiSource::BackingStore)
                .point_to_physical(0, 5)
                .1,
            8
        );
    }

    #[test]
    fn dpi_from_raw_values_sanitises_a_corrupt_answer() {
        // 96 DPI is 1.0; 120 DPI is 1.25.
        assert_eq!(DpiScale::from_dpi(96, DpiSource::PerMonitorV2).factor, 1.0);
        assert_eq!(
            DpiScale::from_dpi(120, DpiSource::PerMonitorV2).factor,
            1.25
        );
        assert_eq!(DpiScale::from_dpi(192, DpiSource::BackingStore).factor, 2.0);
        // A zero or absurd answer is not trusted: it degrades to the honest
        // unknown rather than misplacing a click.
        for bad in [0_u32, 1, 10_000] {
            let d = DpiScale::from_dpi(bad, DpiSource::PerMonitorV2);
            assert_eq!(d.source, DpiSource::Unknown, "dpi {bad}");
            assert_eq!(d.factor, 1.0);
        }
        for bad in [0.0_f64, -1.0, f64::NAN, f64::INFINITY, 100.0] {
            let d = DpiScale::from_factor(bad, DpiSource::BackingStore);
            assert_eq!(d.source, DpiSource::Unknown, "factor {bad}");
        }
    }

    /// The one that matters on X11: `Xft.dpi` is a font hint, not an input
    /// coordinate space. Applying it to a click would misplace the click.
    #[test]
    fn only_real_input_spaces_may_scale_a_click_coordinate() {
        assert!(DpiScale::from_dpi(120, DpiSource::PerMonitorV2).applies_to_click_coordinates());
        assert!(DpiScale::from_dpi(192, DpiSource::BackingStore).applies_to_click_coordinates());
        assert!(!DpiScale::from_dpi(120, DpiSource::XftProperty).applies_to_click_coordinates());
        assert!(!DpiScale::unknown().applies_to_click_coordinates());
        // 1.0 sources agree, so a per-monitor-v2 aware process on an unscaled
        // display still reports itself as a click-space authority.
        assert!(DpiScale::from_dpi(96, DpiSource::PerMonitorV2).applies_to_click_coordinates());
    }

    // ---- patch alignment ----------------------------------------------

    #[test]
    fn patch_alignment_floors_to_the_grid_and_never_reaches_zero() {
        // 1260 = 45 * 28 is the largest multiple of the patch factor below the
        // 1280 px ceiling — the named ceiling is deliberately *not* on the grid,
        // so an aligned clamp lands just under it rather than over it.
        assert_eq!(align_to_patch(1260), 1260);
        assert_eq!(align_to_patch(1290), 1288);
        assert_eq!(align_to_patch(1280), 1260, "1280 is not a multiple of 28");
        assert_eq!(align_to_patch(30), 28);
        assert_eq!(align_to_patch(27), 1);
        assert_eq!(align_to_patch(0), 1);
        assert!(
            IMAGE_FACTOR == 28,
            "the named factor is the encoder patch cell"
        );
    }

    #[test]
    fn quantize_box_snaps_onto_the_grid_and_never_empties() {
        let q = quantize_box(Region {
            x: 10,
            y: 33,
            width: 20,
            height: 5,
        });
        assert_eq!(q.x, 0);
        assert_eq!(q.y, 28);
        // Never degenerate: a box that lands inside one cell grows to the cell
        // rather than collapsing to nothing.
        assert!(q.width >= IMAGE_FACTOR);
        assert!(q.height >= IMAGE_FACTOR);
        // Already-aligned input is untouched.
        let aligned = quantize_box(Region {
            x: 56,
            y: 56,
            width: 56,
            height: 56,
        });
        assert_eq!((aligned.x, aligned.y), (56, 56));
        assert_eq!((aligned.width, aligned.height), (56, 56));
    }

    // ---- the budget ---------------------------------------------------

    #[test]
    fn a_capture_inside_the_budget_and_on_the_grid_passes_through_untouched() {
        let png = png_of(560, 280);
        let (out, w, h, report) =
            enforce_output_budget(png.clone(), 560, 280, &OutputBudget::shipped()).unwrap();
        assert_eq!(out, png, "byte-for-byte passthrough");
        assert_eq!((w, h), (560, 280));
        assert_eq!(report.disposition, BudgetDisposition::AsCaptured);
        assert!(!report.disposition.clamped());
        assert_eq!(report.bytes, png.len());
        assert_eq!(report.alignment, IMAGE_FACTOR);
        assert!(report.describe().contains("unchanged"));
    }

    #[test]
    fn an_oversized_capture_is_clamped_to_the_named_ceiling_and_says_so() {
        // 3000x1500 is over the 1280 px ceiling.
        let (out, w, h, report) =
            enforce_output_budget(png_of(3000, 1500), 3000, 1500, &OutputBudget::shipped())
                .unwrap();
        assert!(
            w <= SEE_MAX_DIMENSION_PX && h <= SEE_MAX_DIMENSION_PX,
            "{w}x{h}"
        );
        assert!(!out.is_empty());
        assert!(report.disposition.clamped());
        assert_eq!(
            report.disposition,
            BudgetDisposition::Clamped {
                dimension: true,
                aligned: true
            }
        );
        // Both axes land on the patch grid, and the reported scale is the
        // honest inverse of the clamp that actually happened.
        assert_eq!(w % IMAGE_FACTOR, 0, "width {w} must land on the patch grid");
        assert_eq!(
            h % IMAGE_FACTOR,
            0,
            "height {h} must land on the patch grid"
        );
        // The clamp is declared, not silent.
        assert!(report.describe().contains("clamped"));
        assert!(report.describe().contains("over the max dimension"));
        assert!((report.output_scale_x - f64::from(w) / 3000.0).abs() < 1e-9);
        // An image-space point maps back to the *same fraction* of the original
        // capture, so a model that predicted (w/2, h/2) aims at the centre of
        // the window and not at a scaled-down corner.
        let (wx, wy) = report.image_point_to_window((w / 2) as i32, (h / 2) as i32);
        assert!((wx - 1500).abs() <= 1, "got {wx}, expected ~1500");
        assert!((wy - 750).abs() <= 1, "got {wy}, expected ~750");
    }

    #[test]
    fn an_off_grid_capture_inside_the_ceiling_is_re_aligned_not_up_scaled() {
        // 1000x500 is under 1280 but off the 28 px grid.
        let (out, w, h, report) =
            enforce_output_budget(png_of(1000, 500), 1000, 500, &OutputBudget::shipped()).unwrap();
        assert!(!out.is_empty());
        assert_eq!(w % IMAGE_FACTOR, 0, "width {w} must land on the patch grid");
        assert_eq!(
            h % IMAGE_FACTOR,
            0,
            "height {h} must land on the patch grid"
        );
        assert!(w <= 1000 && h <= 500, "alignment must never up-scale");
        assert_eq!(
            report.disposition,
            BudgetDisposition::Clamped {
                dimension: false,
                aligned: true
            }
        );
    }

    #[test]
    fn a_capture_smaller_than_one_patch_is_left_alone() {
        // 20x12 is under one patch cell; snapping it would destroy it.
        let (out, w, h, report) =
            enforce_output_budget(png_of(20, 12), 20, 12, &OutputBudget::shipped()).unwrap();
        assert_eq!((w, h), (20, 12));
        assert_eq!(report.disposition, BudgetDisposition::AsCaptured);
        assert!(!out.is_empty());
    }

    #[test]
    fn a_payload_that_cannot_be_brought_under_the_byte_ceiling_is_refused() {
        // A byte ceiling smaller than any re-encode forces the fail-closed
        // branch: the error must carry the real numbers and the remedy, and
        // nothing over-budget may be returned.
        let tiny = OutputBudget {
            max_dimension_px: SEE_MAX_DIMENSION_PX,
            max_bytes: 8,
            alignment: IMAGE_FACTOR,
        };
        let err = enforce_output_budget(png_of(560, 280), 560, 280, &tiny).unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, DesktopError::OutputBudget(_)), "{err:?}");
        assert!(msg.contains("refused"), "{msg}");
        assert!(msg.contains("region"), "the remedy must be named: {msg}");
    }

    #[test]
    fn the_named_ceilings_are_what_the_contract_states() {
        let b = OutputBudget::shipped();
        assert_eq!(b.max_dimension_px, 1280);
        assert_eq!(b.max_bytes, 900 * 1024);
        assert_eq!(b.alignment, 28);
    }
}
