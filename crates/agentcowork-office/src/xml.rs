//! XML helpers for surgical OOXML editing.
//!
//! The patch engine works with **byte ranges** into the original part XML
//! (roxmltree `Node::range()`), so untouched bytes are never re-serialized.
//! Because a text node's raw source span may contain entity references
//! (`&amp;`) or multi-byte UTF-8, mapping a *decoded* character index to a
//! *raw byte* index requires scanning the source and counting entities as
//! one character — `split_byte_for_char` does exactly that.

use roxmltree::{Document, Node};

/// WordprocessingML namespace (the `w:` prefix).
pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// Parse a part's bytes into a roxmltree document (borrowed from the input).
pub fn parse<'i>(bytes: &'i [u8]) -> Result<Document<'i>, OfficeXmlError> {
    let text = std::str::from_utf8(bytes).map_err(|_| OfficeXmlError::NotUtf8)?;
    Document::parse(text).map_err(OfficeXmlError::Parse)
}

#[derive(Debug, thiserror::Error)]
pub enum OfficeXmlError {
    #[error("part is not utf-8")]
    NotUtf8,
    #[error("xml parse error: {0}")]
    Parse(roxmltree::Error),
}

/// Does this element belong to the WordprocessingML namespace?
pub fn is_w(node: Node) -> bool {
    node.tag_name().namespace() == Some(W)
}

/// Local name of a node (e.g. `p`, `t`, `tbl`), namespace-agnostic.
pub fn local_name<'i>(node: Node<'_, 'i>) -> &'i str {
    node.tag_name().name()
}

/// The text node directly inside `element`, if any (element → text, no
/// nested elements). Returns the raw byte range + decoded text.
pub fn inner_text(element: Node) -> Option<(std::ops::Range<usize>, String)> {
    for child in element.children() {
        if child.is_text() {
            let range = child.range();
            let text = child.text().unwrap_or("").to_string();
            return Some((range, text));
        }
    }
    None
}

/// Map a *decoded* character index within a raw text span to the byte index
/// in the source. Entities (`&amp;`, `&#38;`, `&#x26;`) count as one
/// decoded character; multi-byte UTF-8 counts by its decoded length.
///
/// `raw` must be the exact source substring of a text node.
pub fn split_byte_for_char(raw: &str, decoded_char_idx: usize) -> usize {
    let mut chars = 0usize;
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && chars < decoded_char_idx {
        if bytes[i] == b'&' {
            // Entity: scan to ';' — counts as one decoded char.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b';' {
                j += 1;
            }
            i = if j < bytes.len() { j + 1 } else { i + 1 };
        } else {
            // One UTF-8 code point.
            let len = utf8_len(bytes[i]);
            i += len;
        }
        chars += 1;
    }
    i
}

/// Decoded character length of a raw text span (entities = 1 char).
pub fn decoded_len(raw: &str) -> usize {
    let bytes = raw.as_bytes();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b';' {
                j += 1;
            }
            i = if j < bytes.len() { j + 1 } else { i + 1 };
        } else {
            i += utf8_len(bytes[i]);
        }
        n += 1;
    }
    n
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Escape text for inclusion inside an XML text node (`<w:t>` content).
/// Escapes `&`, `<`, `>`; strips characters not allowed in XML 1.0.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if is_xml_char(c) => out.push(c),
            _ => {} // strip control chars invalid in XML 1.0
        }
    }
    out
}

fn is_xml_char(c: char) -> bool {
    matches!(c as u32,
        0x9 | 0xA | 0xD
        | 0x20..=0xD7FF
        | 0xE000..=0xFFFD
        | 0x10000..=0x10FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_maps_decoded_to_byte_across_entities() {
        let raw = "a&amp;b"; // decoded: "a&b" (3 chars), raw: 7 bytes
        assert_eq!(split_byte_for_char(raw, 0), 0);
        assert_eq!(split_byte_for_char(raw, 1), 1); // 'a'
        assert_eq!(split_byte_for_char(raw, 2), 6); // after &amp; → 'b'
        assert_eq!(split_byte_for_char(raw, 3), 7); // end
        assert_eq!(decoded_len(raw), 3);
    }

    #[test]
    fn split_handles_multibyte_utf8() {
        let raw = "aé";
        assert_eq!(split_byte_for_char(raw, 1), 1);
        assert_eq!(split_byte_for_char(raw, 2), 3);
        assert_eq!(decoded_len(raw), 2);
    }

    #[test]
    fn numeric_entities_count_as_one() {
        let raw = "&#38;&#x26;"; // 5-byte + 6-byte entities
        assert_eq!(decoded_len(raw), 2);
        assert_eq!(split_byte_for_char(raw, 0), 0);
        assert_eq!(split_byte_for_char(raw, 1), 5); // after the first entity
        assert_eq!(split_byte_for_char(raw, 2), 11); // after both
    }

    #[test]
    fn escape_handles_specials_and_strips_control() {
        assert_eq!(escape_text("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(escape_text("tab\u{1}here"), "tabhere"); // \u{1} stripped
    }

    #[test]
    fn inner_text_finds_text_node_range() {
        let xml = "<w:t xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">Hello</w:t>";
        let doc = parse(xml.as_bytes()).unwrap();
        let root = doc.root_element();
        let (range, text) = inner_text(root).unwrap();
        assert_eq!(text, "Hello");
        let expected_start = xml.find(">Hello").unwrap() + 1;
        assert_eq!(range.start, expected_start); // after the opening tag
        assert_eq!(range.end, expected_start + "Hello".len());
    }
}
