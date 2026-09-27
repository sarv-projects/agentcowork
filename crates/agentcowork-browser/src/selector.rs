//! G8 selector resolver (doc 65 §2 — Scrapling steal): turn a *semantic
//! target* ("the save button", "the search input") + a DOM snapshot into a
//! concrete [`CssOrXPath`] selector that survives minor DOM drift.
//!
//! The resolver walks the [`A11yNode`] tree (role + accessible name), finds
//! the best-scoring node, and emits a **layered** selector: a robust
//! XPath by role+name first, a CSS fallback second, plus the node's stable
//! `ref_id` when present. The model never sees raw DOM noise — it names what
//! it wants and gets back something the browser layer can execute.

use crate::A11yNode;
use serde::{Deserialize, Serialize};

/// A resolved selector: both flavors plus the stable ref, so the caller can
/// try in order (XPath → CSS → ref) and still act when the DOM shifted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssOrXPath {
    pub xpath: String,
    pub css: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
}

/// A semantic target: what the model wants, in accessible terms.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The accessible name (substring-matched, whitespace-normalized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional disambiguator for repeated nodes (nth match).
    #[serde(default)]
    pub index: usize,
}

impl SemanticTarget {
    pub fn role(role: impl Into<String>) -> Self {
        Self {
            role: Some(role.into()),
            name: None,
            index: 0,
        }
    }
    pub fn name(name: impl Into<String>) -> Self {
        Self {
            role: None,
            name: Some(name.into()),
            index: 0,
        }
    }
}

/// The deterministic resolver — pure tree walk, no IO.
#[derive(Debug, Clone, Default)]
pub struct SelectorResolver;

impl SelectorResolver {
    /// Resolve a semantic target against the snapshot's root node. Returns
    /// `None` when nothing matches (the caller can fall back to a re-snapshot
    /// rather than a blind guess).
    pub fn resolve(&self, root: &A11yNode, target: &SemanticTarget) -> Option<CssOrXPath> {
        let mut matches: Vec<&A11yNode> = Vec::new();
        collect_matches(root, target, &mut matches);
        let node = *matches.get(target.index)?;
        Some(self.selector_for(node))
    }

    /// Resolve with a *fallback name*: if the exact name misses, retry with
    /// the loosest distinctive word of the name (survives minor DOM drift —
    /// "Save changes" → "Save").
    pub fn resolve_lenient(&self, root: &A11yNode, target: &SemanticTarget) -> Option<CssOrXPath> {
        if let Some(s) = self.resolve(root, target) {
            return Some(s);
        }
        let name = target.name.as_ref()?;
        // Longest word with ≥ 3 chars, as the drift-tolerant fallback.
        let word = name
            .split_whitespace()
            .filter(|w| w.chars().count() >= 3)
            .max_by_key(|w| w.chars().count())?;
        let loose = SemanticTarget {
            name: Some(word.to_string()),
            ..target.clone()
        };
        self.resolve(root, &loose)
    }

    fn selector_for(&self, node: &A11yNode) -> CssOrXPath {
        let role = css_role(&node.role);
        let name = escape_name(&node.name);
        // XPath by role + normalized name (robust to wrapper divs).
        let xpath = if node.name.is_empty() {
            format!("//*[@role='{}']", role)
        } else {
            format!("//*[@role='{}' and normalize-space()='{}']", role, name)
        };
        // CSS fallback: role attribute selector (plus ref when available).
        let css = if node.name.is_empty() {
            format!("[role='{role}']")
        } else {
            format!("[role='{role}'][aria-label='{name}']")
        };
        CssOrXPath {
            xpath,
            css,
            ref_id: node.ref_id.clone(),
        }
    }
}

fn collect_matches<'a>(node: &'a A11yNode, target: &SemanticTarget, out: &mut Vec<&'a A11yNode>) {
    if matches(node, target) {
        out.push(node);
    }
    for child in &node.children {
        collect_matches(child, target, out);
    }
}

fn matches(node: &A11yNode, target: &SemanticTarget) -> bool {
    if let Some(role) = &target.role {
        if !node.role.eq_ignore_ascii_case(role.trim()) {
            return false;
        }
    }
    if let Some(name) = &target.name {
        let needle = normalize(name);
        if needle.is_empty() {
            return true;
        }
        if !normalize(&node.name)
            .to_lowercase()
            .contains(&needle.to_lowercase())
        {
            return false;
        }
    }
    true
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// ARIA roles map to lowercased CSS role strings; a few need aliases.
fn css_role(role: &str) -> String {
    match role.to_ascii_lowercase().as_str() {
        "textbox" => "textbox".into(),
        "button" => "button".into(),
        "link" => "link".into(),
        "checkbox" => "checkbox".into(),
        "combobox" => "combobox".into(),
        r => r.into(),
    }
}

/// Escape single quotes for the XPath literal and CSS attribute value.
fn escape_name(name: &str) -> String {
    name.replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> A11yNode {
        let mut root = A11yNode::new("document", "Page");
        let mut form = A11yNode::new("form", "");
        let mut search = A11yNode::new("textbox", "Search the site");
        search = search.with_ref("e3");
        form.push(search);
        let save = A11yNode::new("button", "Save changes").with_ref("e7");
        form.push(save);
        root.push(form);
        root
    }

    #[test]
    fn resolves_by_role_and_name() {
        let r = SelectorResolver;
        let s = r
            .resolve(&tree(), &SemanticTarget::name("Save changes"))
            .unwrap();
        assert!(s.xpath.contains("@role='button'"));
        assert!(s.xpath.contains("Save changes"));
        assert_eq!(s.ref_id.as_deref(), Some("e7"));
    }

    #[test]
    fn lenient_resolve_survives_drift() {
        let r = SelectorResolver;
        // Exact name misses ("Save now"), lenient falls back to "Save".
        let s = r
            .resolve_lenient(&tree(), &SemanticTarget::name("Save now"))
            .unwrap();
        assert!(s.xpath.contains("Save"));
    }

    #[test]
    fn role_only_and_index() {
        let r = SelectorResolver;
        let s = r
            .resolve(&tree(), &SemanticTarget::role("textbox"))
            .unwrap();
        assert!(s.css.contains("textbox"));
        let idx = SemanticTarget {
            role: Some("textbox".into()),
            name: None,
            index: 1,
        };
        assert!(r.resolve(&tree(), &idx).is_none()); // only one textbox
    }
}
