//! PPTX slide text model + patch (P4.3 item 1).
//!
//! A slide's text lives in `<p:sp>` shapes → `<p:txBody>` → `<a:p>` paragraphs
//! → `<a:r>` runs → `<a:t>` text. Bullets are `<a:buChar>` / `<a:buAutoNum>`
//! in `<a:pPr>`. The LLM edits the rendered plain text; `patch_shape_text`
//! maps the change back to `<a:t>` byte surgery (GenOffice `w:t` pattern,
//! doc 28 §1 — untouched runs/formatting/geometry survive byte-for-byte).
//!
//! Bullets, `<a:br>` line breaks and paragraph boundaries render as
//! **read-only markers** — they appear in the rendered text but cannot be
//! edited away without XML surgery (editing across one is refused, the same
//! `PatchAcrossMarker` rule as docx).

use std::collections::HashMap;
use std::ops::Range;

use roxmltree::Node;

use crate::xml::{self, escape_text, split_byte_for_char};

use super::parts::{A_NS, P_NS};

fn is_p(node: Node) -> bool {
    node.tag_name().namespace() == Some(P_NS)
}

fn is_a(node: Node) -> bool {
    node.tag_name().namespace() == Some(A_NS)
}

/// An addressable text shape on a slide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// 1-based ordinal among the slide's `<p:sp>` shapes.
    pub ordinal: usize,
    /// Stable address (`shape1`, `shape2`, …).
    pub address: String,
    /// `<p:cNvPr id="…">`.
    pub id: Option<String>,
    /// `<p:cNvPr name="…">`.
    pub name: Option<String>,
    /// `<p:ph type="…">` placeholder type (title/body/…), if any.
    pub ph_type: Option<String>,
    /// Byte range of the `<p:sp>` element in its part.
    pub range: Range<usize>,
    /// The shape's rendered plain text (the LLM's edit surface).
    pub text: String,
}

/// Extract every text shape (`<p:sp>`) from a slide part, in document order.
pub fn shapes(xml: &[u8]) -> Result<Vec<Shape>, crate::OfficeError> {
    let part = std::str::from_utf8(xml)?;
    let doc = xml::parse(xml)?;
    let mut out = Vec::new();
    for node in doc
        .descendants()
        .filter(|n| n.is_element() && is_p(*n) && xml::local_name(*n) == "sp")
    {
        let ordinal = out.len() + 1;
        let (id, name) = c_nv_pr(node);
        let ph_type = placeholder_type(node);
        let text = render_shape(node, part);
        out.push(Shape {
            ordinal,
            address: format!("shape{ordinal}"),
            id,
            name,
            ph_type,
            range: node.range(),
            text,
        });
    }
    Ok(out)
}

/// The `<p:cNvPr id name>` of a shape.
fn c_nv_pr(shape: Node) -> (Option<String>, Option<String>) {
    let c_nv = shape
        .descendants()
        .find(|n| n.is_element() && is_p(*n) && xml::local_name(*n) == "cNvPr");
    match c_nv {
        Some(n) => (
            n.attribute("id").map(|s| s.to_string()),
            n.attribute("name").map(|s| s.to_string()),
        ),
        None => (None, None),
    }
}

/// The `<p:ph type>` placeholder type of a shape.
fn placeholder_type(shape: Node) -> Option<String> {
    shape
        .descendants()
        .find(|n| n.is_element() && is_p(*n) && xml::local_name(*n) == "ph")
        .and_then(|n| n.attribute("type"))
        .map(|s| s.to_string())
}

/// Render a shape's plain text (bullets + runs, paragraphs `\n`-joined).
fn render_shape(shape: Node, part: &str) -> String {
    collect_runs(shape, part).iter().map(|r| r.text()).collect()
}

/// Apply an edit to one shape's text. The change region is mapped back to
/// `<a:t>` byte surgery; bullets/line-breaks/paragraph-boundaries are
/// non-editable (editing across one is refused).
pub fn patch_shape_text(
    xml: &[u8],
    ordinal: usize,
    new_text: &str,
) -> Result<Vec<u8>, crate::OfficeError> {
    let part = std::str::from_utf8(xml)?;
    let doc = xml::parse(xml)?;
    let shape = doc
        .descendants()
        .filter(|n| n.is_element() && is_p(*n) && xml::local_name(*n) == "sp")
        .nth(ordinal - 1)
        .ok_or_else(|| crate::OfficeError::BlockNotFound(format!("shape{ordinal}")))?;

    let runs = collect_runs(shape, part);
    if runs.is_empty() {
        return Err(crate::OfficeError::NoTextAnchor);
    }
    let rendered: String = runs.iter().map(|r| r.text()).collect();
    if rendered == new_text {
        return Ok(xml.to_vec());
    }

    // `common_prefix_suffix` returns CHAR counts; convert to byte offsets for
    // slicing (multi-byte glyphs like `•` make the two differ).
    let rendered_chars = rendered.chars().count();
    let (cp, cs) = common_prefix_suffix(&rendered, new_text);
    let cp_byte = char_to_byte(new_text, cp);
    let new_end_byte = char_to_byte(new_text, new_text.chars().count() - cs);
    let new_middle = &new_text[cp_byte..new_end_byte];
    let region_end = rendered_chars - cs;

    // Any marker overlapping the change region means the edit added/removed a
    // bullet, line break or paragraph boundary → structural change, refuse.
    let mut idx = 0usize;
    for run in &runs {
        let len = run.decoded_len();
        if let Run::Marker { .. } = run {
            if idx + len > cp && idx < region_end {
                return Err(crate::OfficeError::PatchAcrossMarker(format!(
                    "shape{ordinal}"
                )));
            }
        }
        idx += len;
    }

    let start_byte = boundary_byte(&runs, cp)?;
    let end_byte = boundary_byte(&runs, region_end)?;
    if start_byte > end_byte {
        return Err(crate::OfficeError::InvalidPatchRange(format!(
            "shape{ordinal}"
        )));
    }

    let mut out = Vec::with_capacity(xml.len() + new_middle.len());
    out.extend_from_slice(&xml[..start_byte]);
    out.extend_from_slice(escape_text(new_middle).as_bytes());
    out.extend_from_slice(&xml[end_byte..]);
    Ok(out)
}

/// A renderable unit of a shape's text, in document order, with byte mapping.
enum Run {
    /// An `<a:t>` text node: raw byte range + decoded text.
    Text {
        start_byte: usize,
        end_byte: usize,
        raw: String,
        decoded: String,
    },
    /// A non-editable marker (bullet, `<a:br>`, `<a:tab>`, paragraph boundary).
    Marker {
        text: String,
        before_byte: usize,
        after_byte: usize,
    },
}

impl Run {
    fn decoded_len(&self) -> usize {
        match self {
            Run::Text { decoded, .. } => decoded.chars().count(),
            Run::Marker { text, .. } => text.chars().count(),
        }
    }

    fn text(&self) -> &str {
        match self {
            Run::Text { decoded, .. } => decoded,
            Run::Marker { text, .. } => text,
        }
    }
}

/// A raw span before byte-position resolution.
enum RawSpan {
    Text {
        pos: usize,
        raw: String,
        decoded: String,
    },
    /// Element-backed marker (`<a:br>`, `<a:tab>`, `<a:buChar>`, `<a:buAutoNum>`).
    ElementMarker { text: String, range: Range<usize> },
    /// Implied `\n` between two `<a:p>` paragraphs (no element of its own).
    ParagraphBoundary,
}

/// Collect the runs of a shape's text body in document order.
fn collect_runs(shape: Node, part: &str) -> Vec<Run> {
    // `<p:txBody>` is a presentationml element (its children are drawingml).
    let tx_body = shape
        .descendants()
        .find(|n| n.is_element() && is_p(*n) && xml::local_name(*n) == "txBody");
    let Some(tx_body) = tx_body else {
        return Vec::new();
    };

    let mut raw: Vec<RawSpan> = Vec::new();
    let mut counters: HashMap<usize, usize> = HashMap::new();
    let paragraphs: Vec<Node> = tx_body
        .children()
        .filter(|n| n.is_element() && is_a(*n) && xml::local_name(*n) == "p")
        .collect();

    for (i, p) in paragraphs.iter().enumerate() {
        if i > 0 {
            raw.push(RawSpan::ParagraphBoundary);
        }
        if let Some(ppr) = p
            .children()
            .find(|n| n.is_element() && is_a(*n) && xml::local_name(*n) == "pPr")
        {
            if let Some(marker) = bullet_marker(ppr, &mut counters) {
                raw.push(marker);
            }
        }
        collect_para_spans(*p, part, &mut raw);
    }

    resolve_runs(&raw, tx_body.range())
}

/// The bullet marker for a paragraph's `<a:pPr>`, if it declares one.
fn bullet_marker(ppr: Node, counters: &mut HashMap<usize, usize>) -> Option<RawSpan> {
    let level = ppr
        .attribute("lvl")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    for child in ppr.children().filter(|n| n.is_element() && is_a(*n)) {
        match xml::local_name(child) {
            "buChar" => {
                if let Some(c) = child.attribute("char") {
                    return Some(RawSpan::ElementMarker {
                        text: format!("{c} "),
                        range: child.range(),
                    });
                }
            }
            "buAutoNum" => {
                let n = counters.entry(level).or_insert(0);
                *n += 1;
                return Some(RawSpan::ElementMarker {
                    text: format!("{n}. "),
                    range: child.range(),
                });
            }
            _ => {}
        }
    }
    None
}

/// Collect text spans + markers under a paragraph (document order).
fn collect_para_spans(node: Node, part: &str, out: &mut Vec<RawSpan>) {
    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        if is_a(child) {
            match xml::local_name(child) {
                "t" => {
                    // The INNER text node's range (never the <a:t> tags).
                    if let Some((range, text)) = crate::xml::inner_text(child) {
                        let raw = part[range.start..range.end].to_string();
                        out.push(RawSpan::Text {
                            pos: range.start,
                            raw,
                            decoded: text,
                        });
                    }
                }
                "br" => out.push(RawSpan::ElementMarker {
                    text: "\n".to_string(),
                    range: child.range(),
                }),
                "tab" => out.push(RawSpan::ElementMarker {
                    text: "\t".to_string(),
                    range: child.range(),
                }),
                _ => collect_para_spans(child, part, out),
            }
        } else {
            // Defensive recursion (a:r/a:fld are `a:`; other namespaces may nest).
            collect_para_spans(child, part, out);
        }
    }
}

/// Resolve raw spans into byte-mapped runs.
fn resolve_runs(raw: &[RawSpan], fallback: Range<usize>) -> Vec<Run> {
    let mut runs = Vec::with_capacity(raw.len());
    for (i, span) in raw.iter().enumerate() {
        match span {
            RawSpan::Text { pos, raw, decoded } => runs.push(Run::Text {
                start_byte: *pos,
                end_byte: pos + raw.len(),
                raw: raw.clone(),
                decoded: decoded.clone(),
            }),
            RawSpan::ElementMarker { text, range } => runs.push(Run::Marker {
                text: text.clone(),
                before_byte: range.start,
                after_byte: range.end,
            }),
            RawSpan::ParagraphBoundary => {
                let before_byte = raw[..i]
                    .iter()
                    .rev()
                    .find_map(|s| match s {
                        RawSpan::Text { pos, raw, .. } => Some(pos + raw.len()),
                        RawSpan::ElementMarker { range, .. } => Some(range.end),
                        RawSpan::ParagraphBoundary => None,
                    })
                    .unwrap_or(fallback.start);
                let after_byte = raw[i + 1..]
                    .iter()
                    .find_map(|s| match s {
                        RawSpan::Text { pos, .. } => Some(*pos),
                        RawSpan::ElementMarker { range, .. } => Some(range.start),
                        RawSpan::ParagraphBoundary => None,
                    })
                    .unwrap_or(fallback.end);
                runs.push(Run::Marker {
                    text: "\n".to_string(),
                    before_byte,
                    after_byte,
                });
            }
        }
    }
    runs
}

/// Byte position of rendered char `k` — the start byte of that char in the
/// part (or the position after the last char when `k == total chars`).
fn boundary_byte(runs: &[Run], k: usize) -> Result<usize, crate::OfficeError> {
    let mut acc = 0usize;
    for (i, run) in runs.iter().enumerate() {
        let len = run.decoded_len();
        match run {
            Run::Text {
                start_byte, raw, ..
            } => {
                if k == acc {
                    return Ok(*start_byte);
                }
                if k < acc + len {
                    return Ok(*start_byte + split_byte_for_char(raw, k - acc));
                }
                if k == acc + len {
                    return Ok(next_span_start(runs, i));
                }
                acc += len;
            }
            Run::Marker {
                before_byte,
                after_byte,
                ..
            } => {
                if k == acc {
                    return Ok(*before_byte);
                }
                if k == acc + len {
                    return Ok(*after_byte);
                }
                acc += len;
            }
        }
    }
    match runs.last() {
        Some(Run::Text { end_byte, .. }) => Ok(*end_byte),
        Some(Run::Marker { after_byte, .. }) => Ok(*after_byte),
        None => Err(crate::OfficeError::NoTextAnchor),
    }
}

/// Byte position where the next span's first char begins (skipping tags
/// between spans). Falls back to this span's end when it is last.
fn next_span_start(runs: &[Run], i: usize) -> usize {
    if let Some(next) = runs.get(i + 1) {
        return match next {
            Run::Text { start_byte, .. } => *start_byte,
            Run::Marker { before_byte, .. } => *before_byte,
        };
    }
    match &runs[i] {
        Run::Text { end_byte, .. } => *end_byte,
        Run::Marker { after_byte, .. } => *after_byte,
    }
}

/// Common prefix + common suffix lengths (chars) of two strings.
fn common_prefix_suffix(a: &str, b: &str) -> (usize, usize) {
    let cp = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let cs = a
        .chars()
        .rev()
        .zip(b.chars().rev())
        .take(a.chars().count().saturating_sub(cp))
        .take_while(|(x, y)| x == y)
        .count();
    (cp, cs)
}

/// Byte offset of character index `char_idx` (or `s.len()` past the end).
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}
