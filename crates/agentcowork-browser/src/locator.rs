//! P2.3 — semantic locators, a11y audit, and batch command mode (doc 55
//! `find` semantics; chrome-devtools-mcp / skyvern `parse_actions.py` pattern).
//!
//! These are pure tree functions over [`A11yNode`] — no CDP round-trip. They
//! are the deterministic half of the post-v1 tools (the annotated-screenshot
//! overlay still needs live `DOM.getBoxModel` geometry, so it stays out here).

use crate::{A11yNode, ActKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// `find` semantic locators (ARIA role + accessible name/label)
// ---------------------------------------------------------------------------

/// A semantic query: match nodes by role and/or accessible name.
///
/// - `role` matches case-insensitively against the node's ARIA role.
/// - `name` matches case-insensitively as a substring of the accessible name
///   (whitespace-normalized), so `"search"` hits `"Search the site"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl SemanticQuery {
    pub fn role(role: impl Into<String>) -> Self {
        Self {
            role: Some(role.into()),
            name: None,
        }
    }

    pub fn name(name: impl Into<String>) -> Self {
        Self {
            role: None,
            name: Some(name.into()),
        }
    }

    pub fn new(role: Option<String>, name: Option<String>) -> Self {
        Self { role, name }
    }

    fn matches(&self, node: &A11yNode) -> bool {
        if let Some(role) = &self.role {
            if !node.role.eq_ignore_ascii_case(role.trim()) {
                return false;
            }
        }
        if let Some(name) = &self.name {
            let needle = normalize(name).to_lowercase();
            if needle.is_empty() {
                return true; // empty needle = role-only match
            }
            if !normalize(&node.name).to_lowercase().contains(&needle) {
                return false;
            }
        }
        true
    }
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A located node, ready to hand back to the model as a `[ref=eN]` target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Located {
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
    pub actionable: bool,
}

impl From<&A11yNode> for Located {
    fn from(n: &A11yNode) -> Self {
        Self {
            role: n.role.clone(),
            name: n.name.clone(),
            ref_id: n.ref_id.clone(),
            actionable: n.actionable,
        }
    }
}

/// Breadth-first search over the snapshot tree. Returns every node matching
/// the query (empty when nothing matches). Prefer this over `find_ref` when the
/// model does not yet know the exact `[ref=eN]` — the whole point of a locator.
pub fn find_semantic(root: &A11yNode, query: &SemanticQuery) -> Vec<Located> {
    let mut out = Vec::new();
    find_semantic_into(root, query, &mut out);
    out
}

fn find_semantic_into(node: &A11yNode, query: &SemanticQuery, out: &mut Vec<Located>) {
    if query.matches(node) {
        out.push(Located::from(node));
    }
    for c in &node.children {
        find_semantic_into(c, query, out);
    }
}

/// Convenience: `find` a single best candidate by role+name. Returns the first
/// match in breadth-first order (document order), which mirrors how a human
/// reads the tree top-to-bottom.
pub fn find_first(root: &A11yNode, query: &SemanticQuery) -> Option<Located> {
    find_semantic(root, query).into_iter().next()
}

/// The first actionable `[ref=eN]` id in document order — handy for E2E tests
/// and "act on the obvious target" fallbacks when the model gives no ref.
pub fn first_actionable_ref(root: &A11yNode) -> Option<String> {
    fn walk(node: &A11yNode) -> Option<String> {
        if node.actionable {
            if let Some(r) = &node.ref_id {
                return Some(r.clone());
            }
        }
        for c in &node.children {
            if let Some(r) = walk(c) {
                return Some(r);
            }
        }
        None
    }
    walk(root)
}

// ---------------------------------------------------------------------------
// `a11y_audit` — deterministic lint over the snapshot tree
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum A11ySeverity {
    /// Should block an automated submission attempt.
    Error,
    /// Advisory (duplicate labels, empty alt, nesting smell).
    Warning,
}

/// One accessibility issue found in the tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct A11yIssue {
    pub severity: A11ySeverity,
    pub rule: String,
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
}

impl A11yIssue {
    fn new(
        severity: A11ySeverity,
        rule: &str,
        role: &str,
        name: &str,
        ref_id: Option<&str>,
    ) -> Self {
        Self {
            severity,
            rule: rule.into(),
            role: role.into(),
            name: name.into(),
            ref_id: ref_id.map(str::to_string),
        }
    }
}

/// Audit the tree for common a11y problems (axe-core subset):
///
/// 1. **error** — actionable element with an empty accessible name (the model
///    cannot target it by label and screen-reader users cannot either).
/// 2. **error** — duplicate `[ref=eN]` ids (would make `act` ambiguous).
/// 3. **warning** — `img`/`image` with an empty name (missing alt).
/// 4. **warning** — interactive node nested inside another interactive node
///    (a11y anti-pattern; often a broken SPA click target).
pub fn a11y_audit(root: &A11yNode) -> Vec<A11yIssue> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    audit_into(root, &mut seen, &mut out, false);
    out
}

fn audit_into(
    node: &A11yNode,
    seen: &mut HashSet<String>,
    out: &mut Vec<A11yIssue>,
    in_interactive: bool,
) {
    let interactive = node.actionable || crate::ax::is_interactive(&node.role);

    // 1. actionable but nameless.
    if interactive && normalize(&node.name).is_empty() {
        out.push(A11yIssue::new(
            A11ySeverity::Error,
            "actionable_without_name",
            &node.role,
            &node.name,
            node.ref_id.as_deref(),
        ));
    }

    // 2. duplicate refs.
    if let Some(r) = &node.ref_id {
        if !seen.insert(r.clone()) {
            out.push(A11yIssue::new(
                A11ySeverity::Error,
                "duplicate_ref",
                &node.role,
                &node.name,
                Some(r),
            ));
        }
    }

    // 3. image without alt.
    if (node.role.eq_ignore_ascii_case("img") || node.role.eq_ignore_ascii_case("image"))
        && normalize(&node.name).is_empty()
    {
        out.push(A11yIssue::new(
            A11ySeverity::Warning,
            "image_without_alt",
            &node.role,
            &node.name,
            node.ref_id.as_deref(),
        ));
    }

    // 4. nested interactive.
    if in_interactive && interactive {
        out.push(A11yIssue::new(
            A11ySeverity::Warning,
            "nested_interactive",
            &node.role,
            &node.name,
            node.ref_id.as_deref(),
        ));
    }

    for c in &node.children {
        audit_into(c, seen, out, in_interactive || interactive);
    }
}

// ---------------------------------------------------------------------------
// P46.4 — explicit ref invalidation (E9 H2 / E3 hard invariant)
// ---------------------------------------------------------------------------

/// P46.4 — the ref-generation registry. Enforces the **act → invalidate →
/// re-observe** invariant at the ref level: a snapshot mints refs into a
/// generation; any state-changing action advances the generation, so refs
/// from a pre-mutation snapshot are **stale** and must not be acted on
/// without a re-observe. The registry is deliberately lenient toward refs it
/// has never seen (a one-shot `act` with a fresh ref still works) — the hard
/// rule is that a ref *known* to come from an older generation is rejected.
///
/// Pure + deterministic (no CDP): the action layer holds one registry per
/// engine, `observe`s after every snapshot, `invalidate`s after every
/// state-changing action, and refuses stale refs before resolving geometry.
#[derive(Debug, Clone, Default)]
pub struct RefRegistry {
    /// The current generation. Advances on every invalidation.
    generation: u64,
    /// ref_id → the generation it was observed in.
    observed: std::collections::HashMap<String, u64>,
}

impl RefRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register every ref in a fresh snapshot under the current generation.
    /// Call this right after a `snapshot()`. Ref ids already present from an
    /// older generation are re-stamped as current (the re-observe is what
    /// makes them fresh again).
    pub fn observe(&mut self, root: &A11yNode) {
        fn walk(node: &A11yNode, generation: u64, reg: &mut RefRegistry) {
            if let Some(r) = &node.ref_id {
                reg.observed.insert(r.clone(), generation);
            }
            for c in &node.children {
                walk(c, generation, reg);
            }
        }
        walk(root, self.generation, self);
    }

    /// Advance the generation — call after **any state-changing action**.
    /// Every ref observed before this call becomes stale.
    pub fn invalidate(&mut self) {
        self.generation += 1;
    }

    /// Is this ref known to come from an older (pre-mutation) generation?
    /// `false` for unknown refs (fresh single-shot use) and for refs observed
    /// in the current generation.
    pub fn is_stale(&self, ref_id: &str) -> bool {
        self.observed
            .get(ref_id)
            .is_some_and(|g| *g < self.generation)
    }

    /// The current generation (for diagnostics).
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

// ---------------------------------------------------------------------------
// Batch JSON command mode (post-v1: run many `act` primitives in one call)
// ---------------------------------------------------------------------------

/// Error parsing a batch of commands — carries the 0-based index that failed.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchParseError {
    pub index: usize,
    pub message: String,
}

impl std::fmt::Display for BatchParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command {}: {}", self.index, self.message)
    }
}

/// Parse a JSON array of [`ActKind`] commands (batch command mode). Fails with
/// the offending index so the caller can report which line was malformed
/// instead of rejecting the whole batch silently.
pub fn parse_batch(json: &str) -> Result<Vec<ActKind>, BatchParseError> {
    let values: Vec<Value> = serde_json::from_str(json).map_err(|e| BatchParseError {
        index: 0,
        message: format!("batch is not a JSON array: {e}"),
    })?;
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            serde_json::from_value::<ActKind>(v).map_err(|e| BatchParseError {
                index: i,
                message: e.to_string(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> A11yNode {
        let mut root = A11yNode::new("WebArea", "Sign in");
        let mut search = A11yNode::new("searchbox", "Search the site")
            .with_ref("e1")
            .with_actionable();
        search.push(
            A11yNode::new("button", "Clear search")
                .with_ref("e2")
                .with_actionable(),
        );
        let email = A11yNode::new("textbox", "Email")
            .with_ref("e3")
            .with_actionable();
        let img = A11yNode::new("img", "");
        root.push(search);
        root.push(email);
        root.push(img);
        root
    }

    #[test]
    fn find_by_role_is_case_insensitive() {
        let got = find_semantic(&tree(), &SemanticQuery::role("SearchBox"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ref_id.as_deref(), Some("e1"));
    }

    #[test]
    fn find_by_name_substring_normalizes_whitespace() {
        let got = find_semantic(&tree(), &SemanticQuery::name("search the"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].role, "searchbox");
    }

    #[test]
    fn find_first_returns_document_order() {
        let q = SemanticQuery::new(Some("button".into()), None);
        let got = find_first(&tree(), &q).unwrap();
        assert_eq!(got.name, "Clear search");
    }

    #[test]
    fn find_empty_when_no_match() {
        assert!(find_semantic(&tree(), &SemanticQuery::name("nonexistent")).is_empty());
    }

    #[test]
    fn audit_flags_nameless_actionable_and_missing_alt() {
        let issues = a11y_audit(&tree());
        // `img` with empty name → warning (not actionable, so not an error).
        assert!(
            issues
                .iter()
                .any(|i| i.rule == "image_without_alt" && i.severity == A11ySeverity::Warning)
        );
        // nested button inside searchbox → nested_interactive warning.
        assert!(issues.iter().any(|i| i.rule == "nested_interactive"));
    }

    #[test]
    fn audit_flags_duplicate_ref() {
        let mut root = A11yNode::new("WebArea", "");
        root.push(
            A11yNode::new("button", "A")
                .with_ref("e1")
                .with_actionable(),
        );
        root.push(
            A11yNode::new("button", "B")
                .with_ref("e1")
                .with_actionable(),
        );
        let issues = a11y_audit(&root);
        assert!(issues.iter().any(|i| i.rule == "duplicate_ref"));
    }

    #[test]
    fn batch_parses_commands() {
        let json = r#"[
            {"kind":"click","ref_id":"e1"},
            {"kind":"type","ref_id":"e3","text":"hi"}
        ]"#;
        let cmds = parse_batch(json).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            cmds[0],
            ActKind::Click {
                ref_id: "e1".into()
            }
        );
        assert_eq!(
            cmds[1],
            ActKind::Type {
                ref_id: "e3".into(),
                text: "hi".into()
            }
        );
    }

    #[test]
    fn batch_reports_offending_index() {
        let json = r#"[{"kind":"click","ref_id":"e1"},{"kind":"bogus"}]"#;
        let err = parse_batch(json).unwrap_err();
        assert_eq!(err.index, 1);
    }

    #[test]
    fn batch_rejects_non_array() {
        assert!(parse_batch("42").is_err());
    }

    // P46.4 — the acceptance test: a ref from a pre-mutation snapshot cannot
    // be used post-mutation without a re-observe.
    #[test]
    fn ref_from_pre_mutation_snapshot_is_stale_until_reobserve() {
        let mut reg = RefRegistry::new();

        // Observe snapshot 1 — e1/e2 are fresh.
        reg.observe(&tree());
        assert!(!reg.is_stale("e1"));
        assert!(!reg.is_stale("e2"));

        // A state-changing action invalidates the generation.
        reg.invalidate();
        assert!(
            reg.is_stale("e1"),
            "ref from pre-mutation snapshot must be stale"
        );
        assert!(reg.is_stale("e2"));

        // Acting on e1 now is refused by the caller (stale check).
        assert!(reg.is_stale("e1"));

        // A fresh snapshot (re-observe) re-stamps e1 as current.
        let mut post = tree();
        post.push(
            A11yNode::new("textbox", "New field")
                .with_ref("e9")
                .with_actionable(),
        );
        reg.observe(&post);
        assert!(!reg.is_stale("e1"), "re-observe makes the ref fresh again");
        assert!(!reg.is_stale("e9"));
    }

    #[test]
    fn unknown_refs_are_lenient_single_shot_use_works() {
        let mut reg = RefRegistry::new();
        // No observe yet — a one-shot act with a fresh ref still passes.
        assert!(!reg.is_stale("e1"));
        // After an invalidation, unknown refs remain allowed (the registry
        // never saw them; it cannot call them stale).
        reg.invalidate();
        assert!(!reg.is_stale("e1"));
    }
}
