//! PDF **redaction by removal** (FIX-15, `ARCH/22` §2/§3/§5, REQ-OFFICE-006,
//! REQ-OFFICE-007, REQ-OFFICE-008 — the v0 P0 carried forward).
//!
//! # Why this module exists
//!
//! The previous implementation of [`redact`] appended `/Redact` **annotations**
//! over the given rectangles. A mark is not a redaction: the text underneath
//! stays in the content stream, stays extractable, and copies out with the
//! file. `ARCH/22` §2 is explicit — "Redact must **remove** content, not
//! annotate" — and REQ-OFFICE-008 requires a post-op text-extraction check
//! that *proves* the removal. This module removes the drawing operators.
//!
//! # What "removal" means here
//!
//! Redaction happens at the **content-stream** level:
//!
//! - **Text** (`Tj` / `TJ` / `'` / `"`): the pass runs the PDF graphics and
//!   text state machines (`q`/`Q`/`cm`/`BT`/`ET`/`Tf`/`Td`/`TD`/`Tm`/`T*`/…),
//!   computes each glyph run's box in page user space from the font's own
//!   widths, and **drops the showing operator** when the box intersects a
//!   redaction rectangle. The glyphs are gone from the file; they are not
//!   covered.
//! - **Form XObjects** (`Do` on a `/Subtype /Form`): recursed into with the
//!   accumulated CTM, bounded by `max_form_depth`. A form whose `/BBox` lies
//!   wholly inside a rectangle is removed wholesale; a partially covered form
//!   is recursed into rather than dropped.
//! - **Image XObjects and inline images**: removed only when the placed image
//!   is *wholly* inside the redaction area. A **partial** cover cannot be
//!   removed without visible collateral damage, so it is an explicit
//!   [`Unremovable`] finding — never a silent pass-through.
//!
//! # Fail-closed by default
//!
//! Anything intersecting that this engine cannot remove is a finding, not a
//! shrug. [`UnremovablePolicy::Refuse`] (the default) turns any finding into a
//! typed [`PdfError::Unremovable`], so a caller cannot accidentally commit a
//! "redaction" that left content behind.
//! [`UnremovablePolicy::Report`] keeps the removals that succeeded and returns
//! the findings in the [`RedactReport`] — a deliberate opt-in, never the
//! default. Encrypted documents are refused outright: a partial pass over
//! streams the engine could not decrypt is worse than no pass.
//!
//! # Declared re-serialization (REQ-OFFICE-006)
//!
//! Removing operators means re-encoding the content stream, so
//! [`Residual::ReserializedContentStream`] is **always** in the report. Other
//! structures (fonts, images, metadata, untouched pages) are left alone, but a
//! redacted page's content stream is re-encoded rather than byte-preserved.
//! That is disclosed, not silent.
//!
//! # Known residuals
//!
//! [`Residual`] enumerates what this engine does **not** reach: Type3 glyph
//! procedures (a finding when a run is removed), annotation appearance
//! streams and `/Contents` (an intersecting annotation is a finding), content
//! unreachable from the page's content streams, and embedded font glyph tables
//! (glyph outlines for a removed run may survive unreferenced, so they are not
//! extractable but a forensic reader could find them). This is the honest
//! boundary of a `lopdf`-based engine, and it is reported rather than implied.

use std::collections::{BTreeMap, HashMap, HashSet};

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, dictionary};

use super::PdfError;

/// One redaction rectangle: a 1-based page number plus a rectangle in that
/// page's user space (`[x1, y1, x2, y2]`, origin bottom-left).
pub type RedactRect = (u32, [f32; 4]);

/// A redaction request: the rectangles plus the strings whose absence must be
/// proven after the pass (the REQ-OFFICE-008 verification hook).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RedactRequest {
    /// The rectangles to clear.
    pub rects: Vec<RedactRect>,
    /// Text that must be **absent** from the redacted pages afterwards. A
    /// surviving occurrence is a typed [`PdfError::RemovalUnproven`], never a
    /// warning — this is the check that separates redaction from annotation.
    pub verify_absent: Vec<String>,
}

impl RedactRequest {
    /// Rectangles only.
    pub fn new(rects: Vec<RedactRect>) -> Self {
        Self {
            rects,
            verify_absent: Vec::new(),
        }
    }

    /// Add post-op verification targets.
    pub fn with_verify_absent(mut self, targets: impl IntoIterator<Item = String>) -> Self {
        self.verify_absent.extend(targets);
        self
    }
}

/// What to do about content that intersects a rectangle but cannot be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnremovablePolicy {
    /// Refuse the whole operation with a typed error. **Default** — a
    /// redaction that left content behind is a security failure, not a
    /// cosmetic one.
    #[default]
    Refuse,
    /// Commit the removals that succeeded and return the findings in the
    /// report. An explicit opt-in by a caller that has accepted the residual.
    Report,
}

/// Why a piece of intersecting content could not be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnremovableKind {
    /// A raster image that is only partly covered (removing it entirely would
    /// destroy unrelated pixels; removing part of it needs image surgery this
    /// engine does not do).
    PartialImage,
    /// An inline image (`BI`…`EI`) that is only partly covered.
    PartialInlineImage,
    /// A form XObject nested deeper than [`RedactOptions::max_form_depth`], or
    /// reached through a cyclic reference.
    FormDepth,
    /// Text drawn in a Type3 font: its glyph procedures can paint outside the
    /// declared width box, so the run's real extent is not knowable here.
    Type3Glyphs,
    /// An annotation whose rectangle intersects (its `/Contents` and `/AP` are
    /// content this engine does not remove; deleting a `/Widget` would break
    /// the form).
    Annotation,
    /// The page's content stream could not be decoded, so nothing on it was
    /// examined.
    UndecodableContent,
}

impl UnremovableKind {
    /// Stable identifier for receipts and audit payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            UnremovableKind::PartialImage => "partial-image",
            UnremovableKind::PartialInlineImage => "partial-inline-image",
            UnremovableKind::FormDepth => "form-depth",
            UnremovableKind::Type3Glyphs => "type3-glyphs",
            UnremovableKind::Annotation => "annotation",
            UnremovableKind::UndecodableContent => "undecodable-content",
        }
    }
}

/// A finding: content that intersects a rectangle and was not removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Unremovable {
    /// The 1-based page.
    pub page: u32,
    /// Why it could not be removed.
    pub kind: UnremovableKind,
    /// A human-readable detail (the XObject name, the field name, the depth).
    pub detail: String,
}

impl Unremovable {
    /// A one-line description for an error message or a receipt.
    pub fn describe(&self) -> String {
        format!(
            "page {}: {} could not be removed ({})",
            self.page,
            self.kind.as_str(),
            self.detail
        )
    }
}

/// One removal the pass actually performed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Removal {
    /// The 1-based page.
    pub page: u32,
    /// `text` · `form-xobject` · `image-xobject` · `inline-image`.
    pub kind: &'static str,
    /// The literal text that was removed (empty for non-text removals).
    pub text: String,
    /// The XObject resource name for a non-text removal.
    pub name: String,
    /// The removed region, in page user space.
    pub rect: [f32; 4],
}

/// One removal as it appears in a serialized [`RedactReport`]: the kind is an
/// owned `String` so the report round-trips through `serde` (a borrowed
/// `&'static str` would pin the deserializer's lifetime).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemovalWire {
    page: u32,
    kind: String,
    text: String,
    name: String,
    rect: [f32; 4],
}

impl From<&Removal> for RemovalWire {
    fn from(r: &Removal) -> Self {
        Self {
            page: r.page,
            kind: r.kind.to_string(),
            text: r.text.clone(),
            name: r.name.clone(),
            rect: r.rect,
        }
    }
}

/// What the engine could not reach, stated rather than hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Residual {
    /// The redacted pages' content streams were re-encoded (operator removal
    /// is not byte-preserving). Always present — REQ-OFFICE-006 requires a
    /// re-serialization to be disclosed.
    ReserializedContentStream,
    /// Content unreachable from the page's content streams (optional-content
    /// groups that are not selected, XObjects nothing draws).
    UnreachableContent,
    /// Embedded font programs are not subsetted: a removed run's glyph outlines
    /// may survive unreferenced in the font file. They are not extractable as
    /// text, but a forensic reader could recover the shapes.
    FontGlyphsNotSubetted,
}

/// The result of a redaction pass.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RedactReport {
    /// The redacted document bytes.
    pub bytes: Vec<u8>,
    /// Every removal performed, in pass order (owned `kind`, so the report
    /// round-trips through `serde`).
    pub removals: Vec<RemovalWire>,
    /// Content that intersected a rectangle and was **not** removed.
    pub unremovable: Vec<Unremovable>,
    /// The engine's declared reach.
    pub residuals: Vec<Residual>,
    /// Post-op text extraction per redacted page (the REQ-OFFICE-008 evidence:
    /// what text survived).
    pub surviving_text: BTreeMap<u32, String>,
    /// Pages whose rectangles matched no content at all. Reported so an
    /// all-empty redaction is visible rather than a silent no-op.
    pub no_intersection: Vec<u32>,
}

impl RedactReport {
    /// Total text characters removed.
    pub fn removed_chars(&self) -> usize {
        self.removals
            .iter()
            .filter(|r| r.kind == "text")
            .map(|r| r.text.chars().count())
            .sum()
    }

    /// Whether any removal happened at all.
    pub fn removed_anything(&self) -> bool {
        !self.removals.is_empty()
    }
}

/// Tuning for the removal pass.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RedactOptions {
    /// How deep to recurse into form XObjects. A form nested deeper is a
    /// [`UnremovableKind::FormDepth`] finding, not a silent skip.
    pub max_form_depth: usize,
    /// What to do about findings.
    pub unremovable: UnremovablePolicy,
    /// Glyph box height above the baseline, in em. Deliberately generous:
    /// over-removing a sliver of neighbouring text is a cosmetic loss,
    /// under-removing is a disclosure.
    pub glyph_ascent_em: f32,
    /// Glyph box depth below the baseline, in em.
    pub glyph_descent_em: f32,
}

impl Default for RedactOptions {
    fn default() -> Self {
        Self {
            max_form_depth: 8,
            unremovable: UnremovablePolicy::Refuse,
            glyph_ascent_em: 1.0,
            glyph_descent_em: 0.25,
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// A 2-D affine transform `[a b c d e f]`, PDF order.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Matrix {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Matrix {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn translation(tx: f32, ty: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }

    /// Read a matrix out of `cm` / `Tm` operands.
    fn from_operands(ops: &[Object]) -> Option<Self> {
        if ops.len() < 6 {
            return None;
        }
        Some(Self {
            a: as_f32(&ops[0])?,
            b: as_f32(&ops[1])?,
            c: as_f32(&ops[2])?,
            d: as_f32(&ops[3])?,
            e: as_f32(&ops[4])?,
            f: as_f32(&ops[5])?,
        })
    }

    /// Read a matrix out of a dictionary entry such as a form `/Matrix` or a
    /// page's `/Matrix` (identity when absent).
    fn from_dict(d: &Dictionary, key: &[u8]) -> Self {
        dict_array(d, key)
            .and_then(|a| Matrix::from_operands(a))
            .unwrap_or(Matrix::IDENTITY)
    }

    /// `self × outer` — the PDF concatenation order, so `inner.then(outer)`
    /// means "apply `inner` first, in `outer`'s space".
    fn then(self, outer: Matrix) -> Matrix {
        Matrix {
            a: self.a * outer.a + self.b * outer.c,
            b: self.a * outer.b + self.b * outer.d,
            c: self.c * outer.a + self.d * outer.c,
            d: self.c * outer.b + self.d * outer.d,
            e: self.e * outer.a + self.f * outer.c + outer.e,
            f: self.e * outer.b + self.f * outer.d + outer.f,
        }
    }

    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

/// An axis-aligned box in page user space.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Box {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl Box {
    /// The bounding box of a rect's transformed corners.
    fn from_rect(rect: [f32; 4], m: Matrix) -> Self {
        let pts = [
            m.apply(rect[0], rect[1]),
            m.apply(rect[2], rect[1]),
            m.apply(rect[0], rect[3]),
            m.apply(rect[2], rect[3]),
        ];
        Self::from_points(&pts)
    }

    /// The bounding box of a set of points.
    fn from_points(pts: &[(f32, f32)]) -> Self {
        let (mut x1, mut x2) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut y1, mut y2) = (f32::INFINITY, f32::NEG_INFINITY);
        for (x, y) in pts.iter().copied() {
            x1 = x1.min(x);
            x2 = x2.max(x);
            y1 = y1.min(y);
            y2 = y2.max(y);
        }
        Self { x1, y1, x2, y2 }
    }

    /// Touching counts as intersecting: a glyph flush with the rect edge is
    /// inside the redaction area.
    fn intersects(self, other: Self) -> bool {
        self.x1 <= other.x2 && other.x1 <= self.x2 && self.y1 <= other.y2 && other.y1 <= self.y2
    }

    fn contains_point(self, x: f32, y: f32) -> bool {
        x >= self.x1 && x <= self.x2 && y >= self.y1 && y <= self.y2
    }
}

/// Whether `inner` lies wholly inside the union of `outer` (conservative
/// sampling: the four corners, the four edge midpoints and the centre must all
/// be inside). Reporting a partial cover as a finding is the safe direction —
/// it produces an [`Unremovable`] instead of a silent partial removal.
fn wholly_covered(inner: Box, outer: &[Box]) -> bool {
    let mid_x = (inner.x1 + inner.x2) / 2.0;
    let mid_y = (inner.y1 + inner.y2) / 2.0;
    let samples = [
        (inner.x1, inner.y1),
        (inner.x2, inner.y1),
        (inner.x1, inner.y2),
        (inner.x2, inner.y2),
        (mid_x, inner.y1),
        (mid_x, inner.y2),
        (inner.x1, mid_y),
        (inner.x2, mid_y),
        (mid_x, mid_y),
    ];
    samples
        .iter()
        .all(|&(x, y)| outer.iter().any(|o| o.contains_point(x, y)))
}

/// The first matching rect's overlap with `box_`, in page user space — the
/// region the report names as removed.
fn covering_rect(rects: &[Box], box_: Box) -> [f32; 4] {
    for r in rects {
        if box_.intersects(*r) {
            return [
                r.x1.max(box_.x1),
                r.y1.max(box_.y1),
                r.x2.min(box_.x2),
                r.y2.min(box_.y2),
            ];
        }
    }
    [box_.x1, box_.y1, box_.x2, box_.y2]
}

/// The numeric value of a PDF object (`Integer` or `Real`).
fn as_f32(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(f) => Some(*f),
        _ => None,
    }
}

/// A dictionary entry (`Dictionary::get` is `Result`-shaped in `lopdf`).
fn dict_obj<'a>(d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok()
}

/// A dictionary entry's array value.
fn dict_array<'a>(d: &'a Dictionary, key: &[u8]) -> Option<&'a Vec<Object>> {
    dict_obj(d, key)?.as_array().ok()
}

/// A dictionary entry's numeric value.
fn dict_f32(d: &Dictionary, key: &[u8]) -> Option<f32> {
    dict_obj(d, key).and_then(as_f32)
}

/// A dictionary entry's name value.
fn dict_name<'a>(d: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    dict_obj(d, key)?.as_name().ok()
}

fn set_f32(slot: &mut f32, ops: &[Object]) {
    if let Some(v) = ops.first().and_then(as_f32) {
        *slot = v;
    }
}

fn normalize_rect(rect: [f32; 4]) -> [f32; 4] {
    let (x1, x2) = if rect[0] <= rect[2] {
        (rect[0], rect[2])
    } else {
        (rect[2], rect[0])
    };
    let (y1, y2) = if rect[1] <= rect[3] {
        (rect[1], rect[3])
    } else {
        (rect[3], rect[1])
    };
    [x1, y1, x2, y2]
}

// ---------------------------------------------------------------------------
// Font metrics
// ---------------------------------------------------------------------------

/// Fallback glyph width (1/1000 em) for a simple font that declares none.
/// Deliberately wide: redaction over-approximates rather than under-removes.
const DEFAULT_WIDTH: f32 = 600.0;
/// Fallback CID width (1/1000 em) for a composite font with no `/DW`.
const DEFAULT_CID_WIDTH: f32 = 1000.0;

/// Horizontal glyph metrics for one font resource.
#[derive(Debug, Clone, Default)]
struct FontMetrics {
    /// Glyph widths in 1/1000 em, keyed by character (simple) or CID (Type0).
    widths: HashMap<u16, f32>,
    /// Fallback width for an unmapped code.
    missing: f32,
    /// Codes are two bytes (Type0 / Identity-H).
    composite: bool,
    /// Type3 fonts paint through procedures: the run's real extent is not
    /// derivable from widths, so a removed run in one is a finding.
    type3: bool,
}

impl FontMetrics {
    /// Width of one code in 1/1000 em.
    fn width(&self, code: u16) -> f32 {
        self.widths.get(&code).copied().unwrap_or(self.missing)
    }
}

/// Build the metrics for a dereferenced font dictionary.
fn font_metrics(doc: &Document, font: &Dictionary) -> FontMetrics {
    let subtype = dict_name(font, b"Subtype");
    let mut m = FontMetrics {
        widths: HashMap::new(),
        missing: DEFAULT_WIDTH,
        composite: false,
        type3: subtype == Some(&b"Type3"[..]),
    };
    if subtype == Some(&b"Type0"[..]) {
        m.composite = true;
        m.missing = DEFAULT_CID_WIDTH;
        for id in dict_refs(dict_obj(font, b"DescendantFonts")) {
            let Ok(d) = doc.get_dictionary(id) else {
                continue;
            };
            if let Some(dw) = dict_f32(d, b"DW") {
                m.missing = dw;
            }
            if let Some(w) = dict_array(d, b"W") {
                parse_cid_widths(w, &mut m.widths);
            }
        }
        return m;
    }
    // Simple font: /FirstChar + /Widths, else the descriptor's /MissingWidth.
    let first = dict_f32(font, b"FirstChar").unwrap_or(0.0).max(0.0) as u32;
    if let Some(widths) = dict_array(font, b"Widths") {
        for (i, w) in widths.iter().enumerate() {
            if let Some(v) = as_f32(w) {
                let code = first + i as u32;
                if code <= u16::MAX as u32 {
                    m.widths.insert(code as u16, v);
                }
            }
        }
    }
    if let Some(id) = dict_obj(font, b"FontDescriptor").and_then(|o| o.as_reference().ok())
        && let Ok(d) = doc.get_dictionary(id)
        && let Some(mw) = dict_f32(d, b"MissingWidth")
    {
        m.missing = mw;
    }
    m
}

/// The object ids of a dictionary/array entry that may be inline or indirect.
fn dict_refs(obj: Option<&Object>) -> Vec<ObjectId> {
    match obj {
        Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
        Some(Object::Reference(id)) => vec![*id],
        _ => Vec::new(),
    }
}

/// Parse a CID `/W` array: `[ c [w …] | cfirst clast w … ]`.
fn parse_cid_widths(arr: &[Object], out: &mut HashMap<u16, f32>) {
    let mut i = 0usize;
    while i < arr.len() {
        let Some(c) = as_f32(&arr[i]).map(|v| v as i64) else {
            i += 1;
            continue;
        };
        match &arr[i + 1..] {
            [Object::Array(ws), ..] => {
                for (k, w) in ws.iter().enumerate() {
                    if let Some(v) = as_f32(w) {
                        out.insert(clamp_code(c + k as i64), v);
                    }
                }
                i += 2;
            }
            [second, third, ..] => {
                let (Some(c2), Some(w)) = (as_f32(second).map(|v| v as i64), as_f32(third)) else {
                    i += 1;
                    continue;
                };
                // A run longer than the CID space is a malformed array; stop
                // rather than allocating from it.
                if c2 >= c && (c2 - c) < 65_536 {
                    for code in c..=c2 {
                        out.insert(clamp_code(code), w);
                    }
                }
                i += 3;
            }
            _ => i += 1,
        }
    }
}

fn clamp_code(code: i64) -> u16 {
    code.clamp(0, u16::MAX as i64) as u16
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// The `/Font` and `/XObject` sub-dictionaries available to one content stream.
#[derive(Debug, Default, Clone)]
struct Resources {
    fonts: BTreeMap<Vec<u8>, ObjectId>,
    xobjects: BTreeMap<Vec<u8>, ObjectId>,
}

impl Resources {
    /// Read the resource maps out of a resources dictionary.
    fn read(doc: &Document, res: Option<&Dictionary>) -> Self {
        let mut out = Resources::default();
        let Some(res) = res else {
            return out;
        };
        for key in [b"Font".as_slice(), b"XObject".as_slice()] {
            let entries = match dict_obj(res, key) {
                Some(Object::Reference(id)) => doc.get_dictionary(*id).ok(),
                Some(Object::Dictionary(d)) => Some(d),
                _ => None,
            };
            let Some(entries) = entries else { continue };
            for (name, value) in entries.iter() {
                if let Ok(id) = value.as_reference() {
                    if key == b"Font" {
                        out.fonts.insert(name.clone(), id);
                    } else {
                        out.xobjects.insert(name.clone(), id);
                    }
                }
            }
        }
        out
    }

    /// A form's own resources, falling back to the page's (a form that omits
    /// `/Resources` inherits the page's per the spec).
    fn for_form(doc: &Document, page: &Resources, form_id: ObjectId) -> Resources {
        let Ok(dict) = doc.get_dictionary(form_id) else {
            return page.clone();
        };
        let Some(res) = dict_obj(dict, b"Resources") else {
            // A form that omits /Resources inherits the page's.
            return page.clone();
        };
        match res {
            Object::Reference(id) => doc
                .get_dictionary(*id)
                .map(|d| Resources::read(doc, Some(d)))
                .unwrap_or_else(|_| page.clone()),
            other => match other.as_dict() {
                Ok(d) => {
                    // A form's own maps shadow the page's.
                    let mut merged = page.clone();
                    let own = Resources::read(doc, Some(d));
                    for (k, v) in own.fonts {
                        merged.fonts.insert(k, v);
                    }
                    for (k, v) in own.xobjects {
                        merged.xobjects.insert(k, v);
                    }
                    merged
                }
                Err(_) => page.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Graphics / text state
// ---------------------------------------------------------------------------

/// The graphics state plus the text state, as the content-stream walker needs
/// it. The graphics half is saved and restored by `q`/`Q`.
#[derive(Debug, Clone)]
struct Gs {
    ctm: Matrix,
    font: Option<Vec<u8>>,
    size: f32,
    char_spacing: f32,
    word_spacing: f32,
    h_scale: f32,
    leading: f32,
    rise: f32,
    tm: Matrix,
    tlm: Matrix,
}

impl Default for Gs {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
        }
    }
}

/// The page-space box of a text run, the literal text it draws, and the text
/// matrix position after the run.
struct RunMeasurement {
    text: String,
    box_: Box,
    tm_after: Matrix,
    type3: bool,
}

/// Measure a text-showing operation (`Tj` / `TJ` / `'` / `"`).
fn measure_run(
    gs: &Gs,
    metrics: Option<&FontMetrics>,
    op: &Operation,
    opts: &RedactOptions,
) -> RunMeasurement {
    let mut tm = gs.tm;
    let mut pts: Vec<(f32, f32)> = Vec::new();
    let mut text = String::new();
    let tfs = gs.size;

    for item in show_items(op, metrics.map(|m| m.composite).unwrap_or(false)) {
        match item {
            ShowItem::Kern(adjust) => {
                // A `TJ` number shifts the run left by `n/1000 * size * h_scale`.
                let shift = -adjust / 1000.0 * tfs * gs.h_scale;
                tm = Matrix::translation(shift, 0.0).then(tm);
            }
            ShowItem::Glyph { code, bytes } => {
                let (w, is_space) = match metrics {
                    Some(m) => (m.width(code), !m.composite && code == 32),
                    None => (DEFAULT_WIDTH, code == 32),
                };
                let char_em = gs.char_spacing / tfs.max(f32::MIN_POSITIVE);
                let word_em = if is_space {
                    gs.word_spacing / tfs.max(f32::MIN_POSITIVE)
                } else {
                    0.0
                };
                // Glyph advance in em (before the horizontal scale, which the
                // text matrix applies).
                let adv_em = w / 1000.0 + char_em + word_em;
                // `trm` maps em-space text coordinates to page user space: the
                // glyph box is measured in em and scaled exactly once.
                let trm = Matrix {
                    a: tfs * gs.h_scale,
                    b: 0.0,
                    c: 0.0,
                    d: tfs,
                    e: 0.0,
                    f: gs.rise,
                }
                .then(tm)
                .then(gs.ctm);
                let below = trm.apply(0.0, -opts.glyph_descent_em);
                let right = trm.apply(adv_em, opts.glyph_ascent_em);
                pts.extend([below, right, (below.0, right.1), (right.0, below.1)]);
                // The literal glyph, decoded as Latin-1 for the report. A
                // composite (two-byte) code is not text, so it is not reported
                // as a character.
                if bytes.len() == 1 {
                    text.push(bytes[0] as char);
                }
                tm = Matrix::translation(adv_em * gs.h_scale * tfs, 0.0).then(tm);
            }
        }
    }

    RunMeasurement {
        text,
        box_: Box::from_points(&pts),
        tm_after: tm,
        type3: metrics.is_some_and(|m| m.type3),
    }
}

enum ShowItem {
    Glyph { code: u16, bytes: Vec<u8> },
    Kern(f32),
}

/// Flatten a show operand into glyphs and kerning adjustments, in drawing
/// order. `Tj`/`'` carry one string; `"` is `[aw ac string]`; `TJ` is an array
/// interleaving strings and kerning numbers.
fn show_items(op: &Operation, composite: bool) -> Vec<ShowItem> {
    let operand = match op.operator.as_str() {
        // `"` sets word and character spacing first, so the string is last.
        "\"" => op.operands.last(),
        _ => op.operands.first(),
    };
    let Some(operand) = operand else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match operand {
        Object::String(bytes, _) => push_glyphs(&mut out, bytes, composite),
        Object::Array(items) => {
            for item in items {
                match item {
                    Object::String(bytes, _) => push_glyphs(&mut out, bytes, composite),
                    other => {
                        if let Some(n) = as_f32(other) {
                            out.push(ShowItem::Kern(n));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn push_glyphs(out: &mut Vec<ShowItem>, bytes: &[u8], composite: bool) {
    if composite {
        for pair in bytes.chunks(2) {
            let code = if pair.len() == 2 {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from(pair[0])
            };
            out.push(ShowItem::Glyph {
                code,
                bytes: pair.to_vec(),
            });
        }
    } else {
        for b in bytes {
            out.push(ShowItem::Glyph {
                code: u16::from(*b),
                bytes: vec![*b],
            });
        }
    }
}

// ---------------------------------------------------------------------------
// The removal pass
// ---------------------------------------------------------------------------

/// Where an XObject is invoked, and what is in force at that point.
/// Where a text-showing operator sits, and what is in force at that point.
struct ShowSite<'a> {
    /// The `Tj` / `TJ` / `'` / `"` operator.
    op: &'a Operation,
    /// The text state, advanced past the run either way.
    gs: &'a mut Gs,
    /// The resources in force (for the font's widths).
    resources: &'a Resources,
    /// The rectangles in play on this page.
    rects: &'a [Box],
    /// The 1-based page.
    page: u32,
    /// The output stream a surviving operator is appended to.
    out: &'a mut Vec<Operation>,
    /// Set when the operator was removed.
    changed: &'a mut bool,
}

/// Where an XObject is invoked, and what is in force at that point.
struct XObjectSite<'a> {
    /// The `Do` operator.
    op: &'a Operation,
    /// The output stream the operator is appended to when it survives.
    out: &'a mut Vec<Operation>,
    /// The graphics/text state at the invocation.
    gs: &'a Gs,
    /// The resources in force (the page's, or a form's for a nested call).
    resources: &'a Resources,
    /// The rectangles still in play.
    rects: &'a [Box],
    /// The current form-nesting depth.
    depth: usize,
}

/// The mutable accumulator for one redaction pass.
struct Pass<'a> {
    doc: &'a mut Document,
    opts: RedactOptions,
    removals: Vec<Removal>,
    findings: Vec<Unremovable>,
    /// Form XObjects currently being walked, so a cyclic reference terminates.
    in_progress: HashSet<ObjectId>,
}

impl<'a> Pass<'a> {
    fn new(doc: &'a mut Document, opts: RedactOptions) -> Self {
        Self {
            doc,
            opts,
            removals: Vec::new(),
            findings: Vec::new(),
            in_progress: HashSet::new(),
        }
    }

    /// Walk one content stream, removing what intersects `rects`. Returns
    /// whether the stream changed.
    fn walk(
        &mut self,
        content: &mut Content<Vec<Operation>>,
        page: u32,
        ctm: Matrix,
        resources: &Resources,
        rects: &[Box],
        depth: usize,
    ) -> bool {
        let mut gs = Gs {
            ctm,
            ..Gs::default()
        };
        let mut stack: Vec<Gs> = Vec::new();
        let mut out: Vec<Operation> = Vec::with_capacity(content.operations.len());
        let mut changed = false;

        for op in content.operations.drain(..) {
            match op.operator.as_str() {
                "q" => {
                    stack.push(gs.clone());
                    out.push(op);
                }
                "Q" => {
                    if let Some(prev) = stack.pop() {
                        gs = prev;
                    }
                    out.push(op);
                }
                "cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        gs.ctm = m.then(gs.ctm);
                    }
                    out.push(op);
                }
                "BT" => {
                    gs.tm = Matrix::IDENTITY;
                    gs.tlm = Matrix::IDENTITY;
                    out.push(op);
                }
                "Tf" => {
                    if let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) {
                        gs.font = Some(name.to_vec());
                    }
                    if let Some(size) = op.operands.get(1).and_then(as_f32) {
                        gs.size = size;
                    }
                    out.push(op);
                }
                "Tc" => set_f32(&mut gs.char_spacing, &op.operands),
                "Tw" => set_f32(&mut gs.word_spacing, &op.operands),
                "Tz" => {
                    if let Some(v) = op.operands.first().and_then(as_f32) {
                        gs.h_scale = v / 100.0;
                    }
                }
                "TL" => set_f32(&mut gs.leading, &op.operands),
                "Ts" => set_f32(&mut gs.rise, &op.operands),
                "Td" => {
                    let tx = op.operands.first().and_then(as_f32).unwrap_or(0.0);
                    let ty = op.operands.get(1).and_then(as_f32).unwrap_or(0.0);
                    gs.tlm = Matrix::translation(tx, ty).then(gs.tlm);
                    gs.tm = gs.tlm;
                    out.push(op);
                }
                "TD" => {
                    let tx = op.operands.first().and_then(as_f32).unwrap_or(0.0);
                    let ty = op.operands.get(1).and_then(as_f32).unwrap_or(0.0);
                    gs.leading = -ty;
                    gs.tlm = Matrix::translation(tx, ty).then(gs.tlm);
                    gs.tm = gs.tlm;
                    out.push(op);
                }
                "Tm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        gs.tm = m;
                        gs.tlm = m;
                    }
                    out.push(op);
                }
                "T*" => {
                    gs.tlm = Matrix::translation(0.0, -gs.leading).then(gs.tlm);
                    gs.tm = gs.tlm;
                    out.push(op);
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    // `'` and `"` both move to the next line before showing.
                    if op.operator == "'" || op.operator == "\"" {
                        gs.tlm = Matrix::translation(0.0, -gs.leading).then(gs.tlm);
                        gs.tm = gs.tlm;
                    }
                    // `"` sets the word and character spacing first.
                    if op.operator == "\"" {
                        if let Some(v) = op.operands.first().and_then(as_f32) {
                            gs.word_spacing = v;
                        }
                        if let Some(v) = op.operands.get(1).and_then(as_f32) {
                            gs.char_spacing = v;
                        }
                    }
                    let mut site = ShowSite {
                        op: &op,
                        gs: &mut gs,
                        resources,
                        rects,
                        page,
                        out: &mut out,
                        changed: &mut changed,
                    };
                    if self.show_or_keep(&mut site) {
                        continue;
                    }
                }
                "Do" => {
                    let name = op
                        .operands
                        .first()
                        .and_then(|o| o.as_name().ok())
                        .map(|n| n.to_vec());
                    match name
                        .as_ref()
                        .and_then(|n| resources.xobjects.get(n))
                        .copied()
                    {
                        None => out.push(op),
                        Some(xobj_id) => {
                            let mut site = XObjectSite {
                                op: &op,
                                out: &mut out,
                                gs: &gs,
                                resources,
                                rects,
                                depth,
                            };
                            if self.xobject(page, xobj_id, &mut site) {
                                changed = true;
                            }
                        }
                    }
                }
                "BI" => {
                    // An inline image is placed on the unit square by the
                    // current CTM.
                    let box_ = Box::from_rect([0.0, 0.0, 1.0, 1.0], gs.ctm);
                    if rects.iter().any(|r| box_.intersects(*r)) {
                        if wholly_covered(box_, rects) {
                            self.removals.push(Removal {
                                page,
                                kind: "inline-image",
                                text: String::new(),
                                name: String::new(),
                                rect: covering_rect(rects, box_),
                            });
                            changed = true;
                            continue;
                        }
                        self.findings.push(Unremovable {
                            page,
                            kind: UnremovableKind::PartialInlineImage,
                            detail: "inline image is only partly covered".to_string(),
                        });
                    }
                    out.push(op);
                }
                _ => out.push(op),
            }
        }
        content.operations = out;
        changed
    }

    /// Measure a show operator; drop it when it intersects a rectangle, else
    /// keep it. Always advances the text matrix so later runs keep position.
    /// Returns `true` when the operator was removed.
    fn show_or_keep(&mut self, site: &mut ShowSite<'_>) -> bool {
        let ShowSite {
            op,
            gs,
            resources,
            rects,
            page,
            out,
            changed,
        } = site;
        let (op, resources, rects, page) = (*op, *resources, *rects, *page);
        let (gs, out, changed) = (&mut **gs, &mut **out, &mut **changed);
        let metrics = self.metrics(gs, resources);
        let m = measure_run(gs, metrics.as_ref(), op, &self.opts);
        let hit = rects.iter().any(|r| m.box_.intersects(*r));
        if hit {
            if m.type3 {
                self.findings.push(Unremovable {
                    page,
                    kind: UnremovableKind::Type3Glyphs,
                    detail: format!(
                        "text {:?} is drawn in a Type3 font whose glyph procedures may paint outside the width box",
                        m.text
                    ),
                });
            }
            self.removals.push(Removal {
                page,
                kind: "text",
                text: m.text.clone(),
                name: String::new(),
                rect: covering_rect(rects, m.box_),
            });
            *changed = true;
        } else {
            out.push(op.clone());
        }
        gs.tm = m.tm_after;
        hit
    }

    /// Handle an XObject invocation. Returns whether anything was removed.
    fn xobject(&mut self, page: u32, xobj_id: ObjectId, site: &mut XObjectSite<'_>) -> bool {
        let XObjectSite {
            op,
            out,
            gs,
            resources,
            rects,
            depth,
        } = site;
        let (op, gs, resources, rects, depth) = (*op, *gs, *resources, *rects, *depth);
        let out = &mut **out;
        let label = op
            .operands
            .first()
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_else(|| format!("object {xobj_id:?}"));

        let (subtype, bbox, form_matrix, content) = {
            let Ok(Object::Stream(s)) = self.doc.get_object(xobj_id) else {
                // A broken reference: leave the operator alone, and say so.
                self.findings.push(Unremovable {
                    page,
                    kind: UnremovableKind::UndecodableContent,
                    detail: format!("XObject /{label} could not be resolved"),
                });
                out.push(op.clone());
                return false;
            };
            let subtype = dict_name(&s.dict, b"Subtype")
                .map(|n| n.to_vec())
                .unwrap_or_default();
            let bbox = array_rect(dict_obj(&s.dict, b"BBox")).unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let form_matrix = Matrix::from_dict(&s.dict, b"Matrix");
            let bytes = s
                .decompressed_content()
                .unwrap_or_else(|_| s.content.clone());
            (subtype, bbox, form_matrix, bytes)
        };

        if subtype == b"Image" {
            // An image is placed on the unit square by the current CTM.
            let box_ = Box::from_rect([0.0, 0.0, 1.0, 1.0], gs.ctm);
            if !rects.iter().any(|r| box_.intersects(*r)) {
                out.push(op.clone());
                return false;
            }
            if wholly_covered(box_, rects) {
                self.removals.push(Removal {
                    page,
                    kind: "image-xobject",
                    text: String::new(),
                    name: label,
                    rect: covering_rect(rects, box_),
                });
                return true;
            }
            // A partial cover would need image surgery: a finding, and under
            // the default policy a refusal.
            self.findings.push(Unremovable {
                page,
                kind: UnremovableKind::PartialImage,
                detail: format!("image /{label} is only partly covered"),
            });
            out.push(op.clone());
            return false;
        }

        // Form XObject.
        let form_ctm = form_matrix.then(gs.ctm);
        let form_box = Box::from_rect(bbox, form_ctm);
        if !rects.iter().any(|r| form_box.intersects(*r)) {
            out.push(op.clone());
            return false;
        }
        if wholly_covered(form_box, rects) {
            self.removals.push(Removal {
                page,
                kind: "form-xobject",
                text: String::new(),
                name: label,
                rect: covering_rect(rects, form_box),
            });
            return true;
        }
        if depth >= self.opts.max_form_depth {
            self.findings.push(Unremovable {
                page,
                kind: UnremovableKind::FormDepth,
                detail: format!(
                    "form /{label} nests deeper than the {} level limit",
                    self.opts.max_form_depth
                ),
            });
            out.push(op.clone());
            return false;
        }
        if !self.in_progress.insert(xobj_id) {
            self.findings.push(Unremovable {
                page,
                kind: UnremovableKind::FormDepth,
                detail: format!("form /{label} is reached through a cyclic reference"),
            });
            out.push(op.clone());
            return false;
        }

        let mut inner = match Content::decode(&content) {
            Ok(c) => c,
            Err(_) => {
                self.in_progress.remove(&xobj_id);
                self.findings.push(Unremovable {
                    page,
                    kind: UnremovableKind::UndecodableContent,
                    detail: format!("form /{label} content stream could not be decoded"),
                });
                out.push(op.clone());
                return false;
            }
        };
        let inner_res = Resources::for_form(self.doc, resources, xobj_id);
        let child_rects: Vec<Box> = rects
            .iter()
            .copied()
            .filter(|r| form_box.intersects(*r))
            .collect();
        let changed = self.walk(
            &mut inner,
            page,
            gs.ctm,
            &inner_res,
            &child_rects,
            depth + 1,
        );
        self.in_progress.remove(&xobj_id);
        if changed {
            let Ok(encoded) = inner.encode() else {
                out.push(op.clone());
                return false;
            };
            self.doc.change_content_stream(xobj_id, encoded);
        }
        // The invocation always survives; only the form's content shrank, so
        // the rest of the form still draws.
        out.push(op.clone());
        changed
    }

    /// The metrics for the currently selected font, when it is resolvable.
    fn metrics(&self, gs: &Gs, resources: &Resources) -> Option<FontMetrics> {
        let name = gs.font.as_ref()?;
        let id = resources.fonts.get(name)?;
        let font = self.doc.get_dictionary(*id).ok()?;
        Some(font_metrics(self.doc, font))
    }
}

/// A four-number rectangle from an object, if it holds one.
fn array_rect(obj: Option<&Object>) -> Option<[f32; 4]> {
    let arr = obj?.as_array().ok()?;
    if arr.len() < 4 {
        return None;
    }
    Some([
        as_f32(&arr[0])?,
        as_f32(&arr[1])?,
        as_f32(&arr[2])?,
        as_f32(&arr[3])?,
    ])
}

/// Whether `haystack` contains `needle` (a plain byte-subsequence search).
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Drop content-stream objects that no page references any more.
///
/// `change_page_content` allocates a fresh object when a page had several
/// streams; the superseded objects stay in the table and would be written to
/// the saved file, carrying the redacted text with them. Only ids that no
/// page's `/Contents` resolves to any more are removed, so a stream shared
/// between pages survives.
fn drop_unreferenced_content_streams(
    doc: &mut Document,
    candidates: &[ObjectId],
) -> Result<(), PdfError> {
    if candidates.is_empty() {
        return Ok(());
    }
    let mut live: std::collections::HashSet<ObjectId> = std::collections::HashSet::new();
    for page_id in doc.page_iter() {
        for id in doc.get_page_contents(page_id) {
            live.insert(id);
        }
    }
    for id in candidates {
        if !live.contains(id) {
            doc.objects.remove(id);
        }
    }
    Ok(())
}

/// A page's content, **with a newline between content streams**.///
/// `lopdf::Document::get_page_content` concatenates the page's streams with no
/// separator, so a stream ending in `ET` and the next starting with `BT`
/// become the single bogus operator `ETBT` — the whole tail of the page then
/// fails to parse and its content escapes both the redaction and the
/// verification oracle. Re-joining with an explicit separator is what makes
/// multi-stream pages (the common shape for an incrementally-updated document)
/// honest.
fn page_content(doc: &Document, page_id: ObjectId) -> Result<Vec<u8>, PdfError> {
    let mut out: Vec<u8> = Vec::new();
    for id in doc.get_page_contents(page_id) {
        let Ok(Object::Stream(s)) = doc.get_object(id) else {
            continue;
        };
        if !out.is_empty() {
            out.push(b'\n');
        }
        // `decompressed_content` errors on a stream with no `/Filter`; the raw
        // bytes are already plain in that case.
        match s.decompressed_content() {
            Ok(bytes) => out.extend_from_slice(&bytes),
            Err(_) => out.extend_from_slice(&s.content),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Redact `bytes`: remove the content that draws into `rects`.
///
/// Under [`UnremovablePolicy::Refuse`] (the default) any intersecting content
/// that cannot be removed aborts the whole operation with
/// [`PdfError::Unremovable`] and the caller keeps its original bytes — there is
/// no partial commit.
pub fn redact(bytes: &[u8], rects: &[RedactRect]) -> Result<Vec<u8>, PdfError> {
    let request = RedactRequest::new(rects.to_vec());
    Ok(redact_checked(bytes, &request, &RedactOptions::default())?.bytes)
}

/// Redact with an explicit request and policy, returning the full report.
pub fn redact_checked(
    bytes: &[u8],
    request: &RedactRequest,
    opts: &RedactOptions,
) -> Result<RedactReport, PdfError> {
    if request.rects.is_empty() {
        return Err(PdfError::NothingToRedact);
    }
    let mut doc = Document::load_mem(bytes)?;
    if doc.is_encrypted() {
        // Content removal needs decrypted streams. Redacting only what happened
        // to be readable would be a silent partial pass — refuse instead.
        return Err(PdfError::Encrypted);
    }

    let page_ids: Vec<ObjectId> = doc.page_iter().collect();
    let mut by_page: BTreeMap<u32, Vec<[f32; 4]>> = BTreeMap::new();
    for (page, rect) in &request.rects {
        if *page == 0 || *page as usize > page_ids.len() {
            return Err(PdfError::PageNotFound(*page));
        }
        if !(rect[0].is_finite()
            && rect[1].is_finite()
            && rect[2].is_finite()
            && rect[3].is_finite())
        {
            return Err(PdfError::InvalidRect(*page, *rect));
        }
        by_page
            .entry(*page)
            .or_default()
            .push(normalize_rect(*rect));
    }

    let mut removals: Vec<Removal> = Vec::new();
    let mut findings: Vec<Unremovable> = Vec::new();
    let mut touched: Vec<u32> = Vec::new();
    let mut no_intersection: Vec<u32> = Vec::new();

    for (page_number, rects) in &by_page {
        let page_id = page_ids[(*page_number as usize) - 1];
        // The page's base CTM (its `/Matrix`, identity when absent). The
        // caller's rectangles already account for `/Rotate`, which is
        // deliberately not applied here.
        let base = Matrix::from_dict(doc.get_dictionary(page_id)?, b"Matrix");
        let rect_boxes: Vec<Box> = rects.iter().map(|r| Box::from_rect(*r, base)).collect();

        let content_bytes = page_content(&doc, page_id)?;
        let mut content = Content::decode(&content_bytes)
            .map_err(|_| PdfError::UndecodableContent { page: *page_number })?;
        let resources = {
            // `/Resources` may be inline on the page or an indirect reference
            // (and a page may inherit it from an ancestor `/Pages` node), so
            // both sources are merged — an earlier binding wins, as the page
            // tree's shadowing rules require.
            let (res_dict, res_ids) = doc.get_page_resources(page_id)?;
            let mut merged = Resources::read(&doc, res_dict);
            for id in res_ids {
                if let Ok(d) = doc.get_dictionary(id) {
                    let own = Resources::read(&doc, Some(d));
                    for (k, v) in own.fonts {
                        merged.fonts.entry(k).or_insert(v);
                    }
                    for (k, v) in own.xobjects {
                        merged.xobjects.entry(k).or_insert(v);
                    }
                }
            }
            merged
        };

        let (page_removals, page_findings, changed) = {
            let mut pass = Pass::new(&mut doc, *opts);
            let changed = pass.walk(&mut content, *page_number, base, &resources, &rect_boxes, 0);
            (pass.removals, pass.findings, changed)
        };
        removals.extend(page_removals);
        findings.extend(page_findings);
        if changed {
            let encoded = content.encode()?;
            // The re-encoded stream replaces the old one. When the page had
            // several content streams, `change_page_content` allocates a new
            // object and repoints `/Contents` — the old objects would linger
            // unreferenced and **still contain the redacted text in the saved
            // file**, so they are dropped here. A redaction that leaves the
            // literal in the bytes is not a redaction.
            let before: Vec<ObjectId> = doc.get_page_contents(page_id);
            doc.change_page_content(page_id, encoded)?;
            drop_unreferenced_content_streams(&mut doc, &before)?;
            touched.push(*page_number);
        } else {
            no_intersection.push(*page_number);
        }
    }

    // An annotation inside a redaction area carries content (`/Contents`,
    // `/AP`) this engine does not remove: a finding, never a silent pass.
    findings.extend(annotation_findings(&doc, &by_page, &page_ids)?);
    // After a real removal the marks are stale bookkeeping — drop them so no
    // reader believes the page still needs a burn-in pass.
    drop_redaction_marks(&mut doc, &by_page, &page_ids)?;

    if !findings.is_empty() && matches!(opts.unremovable, UnremovablePolicy::Refuse) {
        // No partial commit: the caller's original bytes are untouched.
        return Err(PdfError::Unremovable(findings));
    }

    let mut out = Vec::new();
    doc.save_to(&mut out)?;

    // REQ-OFFICE-008 — the post-op extraction check. The text that survived
    // each redacted page is the evidence; a target that survived is a typed
    // failure, not a warning.
    let check = Document::load_mem(&out)?;
    let mut surviving_text: BTreeMap<u32, String> = BTreeMap::new();
    for page in &touched {
        surviving_text.insert(*page, check.extract_text(&[*page]).unwrap_or_default());
    }
    if !request.verify_absent.is_empty() {
        let mut hits: Vec<(String, u32)> = Vec::new();
        for target in &request.verify_absent {
            for (page, text) in &surviving_text {
                if text.contains(target.as_str()) {
                    hits.push((target.clone(), *page));
                }
            }
            // A second, stronger oracle: the literal must not survive anywhere
            // in the saved bytes, not merely in an extractable stream. A
            // target that is gone from the extracted text but still present in
            // the file (an orphaned object, say) is still a disclosure. A
            // hex- or UTF-16-encoded target never matches this scan, so a
            // failure here is a sound positive, never a false alarm.
            if contains_bytes(&out, target.as_bytes()) && !hits.iter().any(|(t, _)| t == target) {
                hits.push((target.clone(), 0));
            }
        }
        if !hits.is_empty() {
            return Err(PdfError::RemovalUnproven(hits));
        }
    }

    Ok(RedactReport {
        bytes: out,
        removals: removals.iter().map(RemovalWire::from).collect(),
        unremovable: findings,
        residuals: vec![
            // Always disclosed: operator removal re-encodes the stream.
            Residual::ReserializedContentStream,
            Residual::UnreachableContent,
            Residual::FontGlyphsNotSubetted,
        ],
        surviving_text,
        no_intersection,
    })
}

/// The annotations on the redacted pages whose rectangles intersect a
/// redaction area (excluding our own `/Redact` marks, which are bookkeeping).
fn annotation_findings(
    doc: &Document,
    by_page: &BTreeMap<u32, Vec<[f32; 4]>>,
    page_ids: &[ObjectId],
) -> Result<Vec<Unremovable>, PdfError> {
    let mut findings = Vec::new();
    for (page_number, rects) in by_page {
        let page_id = page_ids[(*page_number as usize) - 1];
        let base = Matrix::from_dict(doc.get_dictionary(page_id)?, b"Matrix");
        let boxes: Vec<Box> = rects.iter().map(|r| Box::from_rect(*r, base)).collect();
        for annot in doc.get_page_annotations(page_id).unwrap_or_default() {
            let subtype = dict_name(annot, b"Subtype")
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_else(|| "Annot".to_string());
            if subtype == "Redact" {
                continue;
            }
            let Some(rect) = array_rect(dict_obj(annot, b"Rect")) else {
                continue;
            };
            if !boxes
                .iter()
                .any(|r| Box::from_rect(rect, base).intersects(*r))
            {
                continue;
            }
            let field = dict_obj(annot, b"T")
                .and_then(|o| o.as_str().ok())
                .map(|s| format!(" field /T {}", String::from_utf8_lossy(s)))
                .unwrap_or_default();
            findings.push(Unremovable {
                page: *page_number,
                kind: UnremovableKind::Annotation,
                detail: format!("annotation /{subtype}{field} intersects the redaction area"),
            });
        }
    }
    Ok(findings)
}

/// Remove this engine's own `/Redact` marks from the pages it just cleared.
fn drop_redaction_marks(
    doc: &mut Document,
    by_page: &BTreeMap<u32, Vec<[f32; 4]>>,
    page_ids: &[ObjectId],
) -> Result<(), PdfError> {
    for page_number in by_page.keys() {
        let page_id = page_ids[(*page_number as usize) - 1];
        let annots = doc.get_page_annotations(page_id).unwrap_or_default();
        let has_mark = annots
            .iter()
            .any(|a| dict_name(a, b"Subtype") == Some(&b"Redact"[..]));
        if !has_mark {
            continue;
        }
        // Resolve each surviving annotation back to its indirect object so the
        // page's `/Annots` array keeps referring to the original dictionaries.
        let mut keep: Vec<Object> = Vec::with_capacity(annots.len());
        let dict = doc.get_dictionary(page_id)?;
        for (key, value) in dict.iter() {
            if key.as_slice() != b"Annots" {
                continue;
            }
            let ids: Vec<ObjectId> = match value {
                Object::Array(a) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
                Object::Reference(id) => match doc.get_object(*id) {
                    Ok(Object::Array(a)) => {
                        a.iter().filter_map(|o| o.as_reference().ok()).collect()
                    }
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            for id in ids {
                let is_mark = doc
                    .get_dictionary(id)
                    .ok()
                    .and_then(|d| dict_name(d, b"Subtype"))
                    .map(|n| n == b"Redact")
                    .unwrap_or(false);
                if !is_mark {
                    keep.push(Object::Reference(id));
                }
            }
        }
        let page_dict = doc.get_dictionary_mut(page_id)?;
        if keep.is_empty() {
            page_dict.remove(b"Annots");
        } else {
            page_dict.set("Annots", keep);
        }
    }
    Ok(())
}

/// Mark-for-redact: append `/Redact` annotations over the given rectangles.
///
/// This is the **marking** half of the two-step convention, exposed
/// explicitly for workflows that review marks before burning them in. It is
/// *not* redaction — the content stays in the file, which is exactly why
/// [`redact`] is the operation `office_cmds::pdf_page_op("redact")` runs.
pub fn mark_for_redaction(bytes: &[u8], rects: &[RedactRect]) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(bytes)?;
    let pages: Vec<ObjectId> = doc.page_iter().collect();

    for (page, [x1, y1, x2, y2]) in rects {
        let page_id = pages
            .get((*page as usize).saturating_sub(1))
            .copied()
            .ok_or(PdfError::PageNotFound(*page))?;

        let annot = Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Redact",
            "Rect" => vec![
                Object::Real(*x1),
                Object::Real(*y1),
                Object::Real(*x2),
                Object::Real(*y2),
            ],
            "F" => 4,
        });
        let annot_id = doc.add_object(annot);

        // Determine where the page's `/Annots` array lives (avoid a double
        // mutable borrow of the document).
        let loc = {
            let page_dict = doc.get_dictionary(page_id)?;
            match page_dict.get(b"Annots") {
                Ok(Object::Array(_)) => AnnotsLoc::Inline(page_id),
                Ok(Object::Reference(id)) => AnnotsLoc::Array(*id),
                _ => AnnotsLoc::Missing(page_id),
            }
        };
        match loc {
            AnnotsLoc::Inline(page_id) => {
                let d = doc.get_dictionary_mut(page_id)?;
                if let Ok(Object::Array(arr)) = d.get_mut(b"Annots") {
                    arr.push(Object::Reference(annot_id));
                }
            }
            AnnotsLoc::Array(arr_id) => {
                let obj = doc.get_object_mut(arr_id)?;
                if let Object::Array(arr) = obj {
                    arr.push(Object::Reference(annot_id));
                }
            }
            AnnotsLoc::Missing(page_id) => {
                doc.get_dictionary_mut(page_id)?
                    .set("Annots", vec![Object::Reference(annot_id)]);
            }
        }
    }

    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

enum AnnotsLoc {
    Inline(ObjectId),
    Array(ObjectId),
    Missing(ObjectId),
}

/// Extract one page's text — the post-op proof's oracle.
pub fn extract_page_text(bytes: &[u8], page: u32) -> Result<String, PdfError> {
    let doc = Document::load_mem(bytes)?;
    Ok(doc.extract_text(&[page]).unwrap_or_default())
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
