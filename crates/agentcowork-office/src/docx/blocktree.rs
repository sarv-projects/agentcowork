//! DOCX block tree (P4.1 items 2 + 3).
//!
//! The document is decomposed into **anchored blocks** — each block carries
//! the byte range it occupies in its part, so the patch engine can touch
//! exactly the bytes it needs (GenOffice `docxIndex` anchors, doc 28 §1).
//!
//! Addresses are stable, LLM-facing identifiers:
//! - `p3` — 3rd paragraph of the body
//! - `t1` — 1st table (rows `t1:r1`, cells `t1:r1c2`, cell paragraphs `t1:r1c2:p1`)
//! - `sec1` — 1st section properties block (renders empty; patching unsupported)
//! - `hdr1:p2` — 2nd paragraph of the 1st header part; `ftr1:p1` similarly
//!
//! Headers/footers are **separate blocks in separate parts** (P4.1 item 6) —
//! they render and patch through the same path as body paragraphs.

use roxmltree::Node;

use crate::xml;

/// Max nesting depth for tables inside table cells (safety guard).
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Table,
    Row,
    Cell,
    Section,
    HeaderFooter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Stable address (see module doc).
    pub address: String,
    pub kind: BlockKind,
    /// The part that owns this block (e.g. `word/document.xml`).
    pub part: String,
    /// Byte range of the element in `part`'s XML.
    pub range: std::ops::Range<usize>,
}

/// The parsed block tree of one document (body + headers/footers).
pub struct BlockTree {
    pub blocks: Vec<Block>,
    /// The plain-text render in document order (the LLM's editing surface).
    pub render: String,
}

/// Build the block tree for a body part + its header/footer parts.
pub fn build_blocks(
    body_xml: &[u8],
    body_part: &str,
    headers: &[(String, Vec<u8>)],
) -> Result<BlockTree, crate::OfficeError> {
    let mut tree = BlockTree {
        blocks: Vec::new(),
        render: String::new(),
    };

    // Body: <w:document><w:body> children.
    let doc = xml::parse(body_xml)?;
    if let Some(body) = doc
        .descendants()
        .find(|n| n.is_element() && xml::is_w(*n) && xml::local_name(*n) == "body")
    {
        walk_container(body, body_part, "", &mut tree, 0);
        tree.render.push_str(&render_container(body));
    }

    // Headers/footers: each part's root is <w:hdr>/<w:ftr>.
    for (name, bytes) in headers {
        let part_doc = xml::parse(bytes)?;
        let prefix = header_prefix(name);
        let root = part_doc.root_element();
        walk_container(root, name, &prefix, &mut tree, 0);
        tree.render.push_str(&render_container(root));
    }

    Ok(tree)
}

/// Determine the address prefix for a header/footer part (`hdr1:`, `ftr1:`).
fn header_prefix(part: &str) -> String {
    let name = part.rsplit('/').next().unwrap_or(part);
    if name.starts_with("header") {
        let n: String = name.chars().skip_while(|c| !c.is_ascii_digit()).collect();
        format!("hdr{}:", n.trim_end_matches(".xml"))
    } else if name.starts_with("footer") {
        let n: String = name.chars().skip_while(|c| !c.is_ascii_digit()).collect();
        format!("ftr{}:", n.trim_end_matches(".xml"))
    } else {
        format!("{name}:")
    }
}

/// Walk a container element (w:body / w:hdr / table cell) collecting blocks.
fn walk_container(container: Node, part: &str, prefix: &str, tree: &mut BlockTree, depth: usize) {
    let mut p = 0usize;
    let mut t = 0usize;
    let mut sec = 0usize;
    for child in container.children() {
        if !child.is_element() || !xml::is_w(child) {
            continue;
        }
        match xml::local_name(child) {
            "p" => {
                p += 1;
                tree.push_block(Block {
                    address: format!("{prefix}p{p}"),
                    kind: BlockKind::Paragraph,
                    part: part.to_string(),
                    range: child.range(),
                });
            }
            "tbl" => {
                t += 1;
                let addr = format!("{prefix}t{t}");
                tree.push_block(Block {
                    address: addr.clone(),
                    kind: BlockKind::Table,
                    part: part.to_string(),
                    range: child.range(),
                });
                walk_table(child, part, &addr, tree, depth + 1);
            }
            "sectPr" => {
                sec += 1;
                tree.push_block(Block {
                    address: format!("{prefix}sec{sec}"),
                    kind: BlockKind::Section,
                    part: part.to_string(),
                    range: child.range(),
                });
            }
            _ => {}
        }
    }
}

/// Render a container's plain text in document order (paragraphs, tables,
/// sections) — the LLM's editing surface.
fn render_container(container: Node) -> String {
    let mut out = String::new();
    for child in container.children() {
        if !child.is_element() || !xml::is_w(child) {
            continue;
        }
        match xml::local_name(child) {
            "p" => {
                out.push_str(&render_paragraph(child));
                out.push('\n');
            }
            "tbl" => {
                out.push_str(&render_table(child));
                out.push('\n');
            }
            _ => {}
        }
    }
    out
}

/// Walk a table: rows, cells, and cell paragraphs (nested tables allowed).
fn walk_table(table: Node, part: &str, prefix: &str, tree: &mut BlockTree, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut r = 0usize;
    for tr in table.children().filter(|n| n.is_element() && xml::is_w(*n)) {
        if xml::local_name(tr) != "tr" {
            continue;
        }
        r += 1;
        let r_addr = format!("{prefix}:r{r}");
        tree.push_block(Block {
            address: r_addr.clone(),
            kind: BlockKind::Row,
            part: part.to_string(),
            range: tr.range(),
        });
        let mut c = 0usize;
        for tc in tr.children().filter(|n| n.is_element() && xml::is_w(*n)) {
            if xml::local_name(tc) != "tc" {
                continue;
            }
            c += 1;
            let c_addr = format!("{r_addr}c{c}");
            tree.push_block(Block {
                address: c_addr.clone(),
                kind: BlockKind::Cell,
                part: part.to_string(),
                range: tc.range(),
            });
            // Nested table inside the cell.
            for nested in tc.children().filter(|n| n.is_element() && xml::is_w(*n)) {
                if xml::local_name(nested) == "tbl" {
                    let t_addr = format!("{c_addr}:t1");
                    tree.push_block(Block {
                        address: t_addr.clone(),
                        kind: BlockKind::Table,
                        part: part.to_string(),
                        range: nested.range(),
                    });
                    walk_table(nested, part, &t_addr, tree, depth + 1);
                }
            }
            // Cell paragraphs (excluding ones inside nested tables — already walked).
            walk_container(tc, part, &format!("{c_addr}:"), tree, depth);
        }
    }
}

/// Render one paragraph's plain text: `w:t` text + `w:br`→`\n`, `w:tab`→`\t`,
/// `w:cr`→`\n`, `w:noBreakHyphen`→`-` (document order, any run nesting).
pub fn render_paragraph(p: Node) -> String {
    let mut out = String::new();
    collect_text(p, &mut out);
    out
}

fn collect_text(node: Node, out: &mut String) {
    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        if xml::is_w(child) {
            match xml::local_name(child) {
                "t" => {
                    if let Some(text) = child.text() {
                        out.push_str(text);
                    }
                }
                "br" | "cr" => out.push('\n'),
                "tab" => out.push('\t'),
                "noBreakHyphen" => out.push('-'),
                _ => collect_text(child, out),
            }
        } else {
            // Non-w elements (e.g. w14 line breaks) — recurse defensively.
            collect_text(child, out);
        }
    }
}

/// Render a table's plain text: rows joined by `\n`, cells by ` | `.
fn render_table(table: Node) -> String {
    let mut out = String::new();
    let mut first_row = true;
    for tr in table.children().filter(|n| n.is_element() && xml::is_w(*n)) {
        if xml::local_name(tr) != "tr" {
            continue;
        }
        if !first_row {
            out.push('\n');
        }
        first_row = false;
        let mut first_cell = true;
        for tc in tr.children().filter(|n| n.is_element() && xml::is_w(*n)) {
            if xml::local_name(tc) != "tc" {
                continue;
            }
            if !first_cell {
                out.push_str(" | ");
            }
            first_cell = false;
            // Cell paragraphs (skip nested tables' content — rendered via their own block).
            for p in tc
                .descendants()
                .filter(|n| n.is_element() && xml::is_w(*n) && xml::local_name(*n) == "p")
            {
                out.push_str(&render_paragraph(p));
            }
        }
    }
    out
}

impl BlockTree {
    fn push_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// Look up a block by address.
    pub fn find(&self, address: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.address == address)
    }

    /// All paragraph blocks of a given part (patch targets).
    pub fn paragraphs_in(&self, part: &str) -> Vec<&Block> {
        self.blocks
            .iter()
            .filter(move |b| b.part == part && b.kind == BlockKind::Paragraph)
            .collect()
    }
}

/// Build a block tree from a bare document part (no headers) — used by the
/// engine for header-less parts and by tests.
pub fn blocks_of_part(xml_bytes: &[u8], part: &str) -> Result<BlockTree, crate::OfficeError> {
    build_blocks(xml_bytes, part, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_get_sequential_addresses() {
        let tree = blocks_of_part(crate::zip::tests::DOCUMENT_XML, "word/document.xml").unwrap();
        let paras: Vec<&Block> = tree.paragraphs_in("word/document.xml");
        assert_eq!(paras.len(), 4); // p1, p2, + both table cell paragraphs
        assert_eq!(paras[0].address, "p1");
        assert_eq!(paras[1].address, "p2");
        assert_eq!(paras[2].address, "t1:r1c1:p1");
        assert_eq!(paras[3].address, "t1:r1c2:p1");
        assert_eq!(paras[2].kind, BlockKind::Paragraph);
    }

    #[test]
    fn table_rows_and_cells_are_blocks() {
        let tree = blocks_of_part(crate::zip::tests::DOCUMENT_XML, "word/document.xml").unwrap();
        let rows: Vec<&Block> = tree
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Row)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, "t1:r1");
        let cells: Vec<&Block> = tree
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Cell)
            .collect();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].address, "t1:r1c1");
        assert_eq!(cells[1].address, "t1:r1c2");
    }

    #[test]
    fn section_is_a_block() {
        let tree = blocks_of_part(crate::zip::tests::DOCUMENT_XML, "word/document.xml").unwrap();
        let sec: Vec<&Block> = tree
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Section)
            .collect();
        assert_eq!(sec.len(), 1);
        assert_eq!(sec[0].address, "sec1");
    }

    #[test]
    fn render_is_document_order_plain_text() {
        let tree = blocks_of_part(crate::zip::tests::DOCUMENT_XML, "word/document.xml").unwrap();
        assert_eq!(
            tree.render,
            "Hello, world!\nLine one\nline two\ncell A1 | cell B1\n"
        );
    }

    #[test]
    fn headers_are_separate_blocks_in_separate_parts() {
        let header_xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>Page header</w:t></w:r></w:p>
</w:hdr>"#;
        let tree = build_blocks(
            crate::zip::tests::DOCUMENT_XML,
            "word/document.xml",
            &[("word/header1.xml".to_string(), header_xml.to_vec())],
        )
        .unwrap();
        let hdr: Vec<&Block> = tree
            .blocks
            .iter()
            .filter(|b| b.part == "word/header1.xml")
            .collect();
        assert_eq!(hdr.len(), 1);
        assert_eq!(hdr[0].address, "hdr1:p1");
        assert!(tree.render.contains("Page header"));
    }

    #[test]
    fn find_by_address() {
        let tree = blocks_of_part(crate::zip::tests::DOCUMENT_XML, "word/document.xml").unwrap();
        assert!(tree.find("p1").is_some());
        assert!(tree.find("p2").is_some());
        assert!(tree.find("nope").is_none());
    }
}
