//! The minimal `w:t` patch renderer (P4.1 item 4 — GenOffice `text-patch.ts`
//! pattern, doc 28 §1).
//!
//! Given a paragraph block's *rendered* text and the LLM's edited text, this
//! computes the common prefix/suffix and replaces **only** the `w:t` text
//! bytes in the change region. Every other byte of the part — run
//! properties, hyperlinks, images, inline formatting, untouched runs —
//! survives byte-for-byte.
//!
//! Safety fallbacks (GenOffice's `null`-return philosophy):
//! - no `w:t` anchor in the paragraph → `NoTextAnchor`
//! - the caller's expected original doesn't match the part → `StaleEdit`
//! - the edit crosses a structural marker (`w:br`/`w:tab`: adding/removing a
//!   line break needs XML surgery, not text surgery) → `PatchAcrossMarker`

use roxmltree::Node;

use crate::xml::{self, escape_text, split_byte_for_char};

use super::blocktree::Block;

/// A renderable unit of a paragraph, in document order, with byte mapping.
enum Run {
    /// A `w:t` text node: raw byte range in the part + its decoded text.
    Text {
        start_byte: usize,
        end_byte: usize,
        raw: String,
        decoded: String,
    },
    /// A structural marker (`w:br`/`w:cr`→'\n', `w:tab`→'\t').
    Marker {
        ch: char,
        /// Byte position just before the marker's rendered char.
        before_byte: usize,
        /// Byte position just after the marker's rendered char.
        after_byte: usize,
    },
}

impl Run {
    fn decoded_len(&self) -> usize {
        match self {
            Run::Text { decoded, .. } => decoded.chars().count(),
            Run::Marker { .. } => 1,
        }
    }

    fn text(&self) -> &str {
        match self {
            Run::Text { decoded, .. } => decoded,
            Run::Marker { ch, .. } => {
                // Single-char marker: use a tiny static buffer.
                static TAB: &str = "\t";
                static NL: &str = "\n";
                static DASH: &str = "-";
                match ch {
                    '\t' => TAB,
                    '\n' => NL,
                    _ => DASH,
                }
            }
        }
    }
}

/// Apply an edit to one paragraph block.
///
/// `part_xml` is the current bytes of the part owning `block`;
/// `expected_original` is the text the caller rendered for this block (must
/// match, so a stale LLM edit can never corrupt a document).
pub fn apply_block_patch(
    part_xml: &[u8],
    block: &Block,
    expected_original: &str,
    new_text: &str,
) -> Result<Vec<u8>, crate::OfficeError> {
    let part = std::str::from_utf8(part_xml)?;
    let doc = xml::parse(part_xml)?;

    // Locate the paragraph element by its recorded byte range.
    let para = doc
        .descendants()
        .find(|n| n.range().start == block.range.start && n.range().end == block.range.end)
        .ok_or(crate::OfficeError::BlockNotFound(block.address.clone()))?;

    let runs = collect_runs(para, part);
    let rendered: String = runs.iter().map(|r| r.text()).collect();

    if rendered != expected_original {
        return Err(crate::OfficeError::StaleEdit {
            address: block.address.clone(),
        });
    }
    if new_text == rendered {
        return Ok(part_xml.to_vec());
    }

    let (cp, cs) = common_prefix_suffix(&rendered, new_text);
    let new_middle = &new_text[cp..new_text.len() - cs];

    // Any marker strictly inside the change region means the edit added or
    // removed a line break / tab → structural change, refuse (documented
    // fallback; the caller can rebuild the paragraph instead).
    let mut idx = 0usize;
    for run in &runs {
        if let Run::Marker { .. } = run {
            // A marker at or inside the change region means the edit added,
            // removed, or repositioned a line break / tab → structural
            // change, refuse (the caller can rebuild the paragraph).
            if idx >= cp && idx < rendered.len() - cs {
                return Err(crate::OfficeError::PatchAcrossMarker(block.address.clone()));
            }
        }
        idx += run.decoded_len();
    }

    let start_byte = boundary_byte(&runs, cp)?;
    let end_byte = boundary_byte(&runs, rendered.len() - cs)?;

    if start_byte > end_byte {
        return Err(crate::OfficeError::InvalidPatchRange(block.address.clone()));
    }

    let mut out = Vec::with_capacity(part.len() + new_middle.len());
    out.extend_from_slice(&part_xml[..start_byte]);
    out.extend_from_slice(escape_text(new_middle).as_bytes());
    out.extend_from_slice(&part_xml[end_byte..]);
    Ok(out)
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
                    // char k is this run's first char.
                    return Ok(*start_byte);
                }
                if k < acc + len {
                    // char k is inside this run.
                    let byte = *start_byte + split_byte_for_char(raw, k - acc);
                    return Ok(byte);
                }
                if k == acc + len {
                    // char k is the NEXT span's first char (or past the end
                    // if this run is last) — the position must skip the
                    // tags/markers between runs.
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
                if k == acc + 1 {
                    return Ok(*after_byte);
                }
                acc += 1;
            }
        }
    }
    // Boundary past the last run: end of the paragraph's last text.
    match runs.last() {
        Some(Run::Text { end_byte, .. }) => Ok(*end_byte),
        Some(Run::Marker { after_byte, .. }) => Ok(*after_byte),
        None => Err(crate::OfficeError::NoTextAnchor),
    }
}

/// Byte position where the next span's first char begins (skipping the XML
/// tags between spans). Falls back to this span's end when it is last.
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

/// A raw span in document order (before byte-position resolution).
enum RawSpan {
    Text {
        pos: usize,
        raw: String,
        decoded: String,
    },
    Marker(char),
}

/// Collect the runs of a paragraph in document order.
fn collect_runs(para: Node, part: &str) -> Vec<Run> {
    let mut raw: Vec<RawSpan> = Vec::new();
    collect_spans(para, part, &mut raw);

    // Resolve marker byte positions from the neighboring text spans.
    let mut runs: Vec<Run> = Vec::with_capacity(raw.len());
    for (i, span) in raw.iter().enumerate() {
        match span {
            RawSpan::Text { pos, raw, decoded } => runs.push(Run::Text {
                start_byte: *pos,
                end_byte: pos + raw.len(),
                raw: raw.clone(),
                decoded: decoded.clone(),
            }),
            RawSpan::Marker(ch) => {
                let before_byte = raw[..i]
                    .iter()
                    .rev()
                    .find_map(|s| match s {
                        RawSpan::Text { pos, raw, .. } => Some(pos + raw.len()),
                        RawSpan::Marker(_) => None,
                    })
                    .unwrap_or_else(|| para.range().start);
                let after_byte = raw[i + 1..]
                    .iter()
                    .find_map(|s| match s {
                        RawSpan::Text { pos, .. } => Some(*pos),
                        RawSpan::Marker(_) => None,
                    })
                    .unwrap_or_else(|| para.range().end);
                runs.push(Run::Marker {
                    ch: *ch,
                    before_byte,
                    after_byte,
                });
            }
        }
    }
    runs
}

/// Collect text spans + markers under `node` (document order).
fn collect_spans(node: Node, part: &str, out: &mut Vec<RawSpan>) {
    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        if xml::is_w(child) {
            match xml::local_name(child) {
                "t" => {
                    // Use the INNER TEXT NODE's range (not the w:t element's)
                    // so region replacements never touch the tags.
                    if let Some((range, text)) = crate::xml::inner_text(child) {
                        let raw = part[range.start..range.end].to_string();
                        out.push(RawSpan::Text {
                            pos: range.start,
                            raw,
                            decoded: text,
                        });
                    }
                }
                "br" | "cr" => out.push(RawSpan::Marker('\n')),
                "tab" => out.push(RawSpan::Marker('\t')),
                "noBreakHyphen" => out.push(RawSpan::Marker('-')),
                _ => collect_spans(child, part, out),
            }
        } else {
            collect_spans(child, part, out);
        }
    }
}
