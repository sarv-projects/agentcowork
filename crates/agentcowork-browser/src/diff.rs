//! Snapshot line-diff (P2.2, E3) — `+n/-n` gutter markers and the URL-change
//! short-circuit (ARCH/08 §8.3, doc 33 §5.2): when the URL changed, return a
//! diff flagged `url_changed` so the caller renders the full new snapshot
//! instead of a garbage cross-navigation diff.

use crate::{Snapshot, SnapshotDiff};
use similar::{ChangeTag, TextDiff};

/// Render a snapshot to its indented lines for diffing.
pub fn snapshot_lines(snapshot: &Snapshot) -> Vec<String> {
    snapshot.root.render().lines().map(str::to_string).collect()
}

/// Diff two snapshots of the same page.
///
/// - Same URL → a line diff with `+n` / `-n` markers (context radius 3).
/// - Different URL → `url_changed: true`; the added set carries the full new
///   snapshot lines and removed is empty (caller should use the new snapshot).
pub fn diff_snapshots(base: &Snapshot, current: &Snapshot) -> SnapshotDiff {
    if base.url != current.url {
        return SnapshotDiff {
            base_document_id: base.document_id.clone(),
            base_url: base.url.clone(),
            added_lines: snapshot_lines(current),
            removed_lines: Vec::new(),
            url_changed: true,
        };
    }
    let old_lines = snapshot_lines(base);
    let new_lines = snapshot_lines(current);
    let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
    let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
    let diff = TextDiff::from_slices(&old_refs, &new_refs);
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => {
                removed.push(format!("- {}", change.value().trim_end()));
            }
            ChangeTag::Insert => {
                added.push(format!("+ {}", change.value().trim_end()));
            }
            ChangeTag::Equal => {}
        }
    }
    SnapshotDiff {
        base_document_id: base.document_id.clone(),
        base_url: base.url.clone(),
        added_lines: added,
        removed_lines: removed,
        url_changed: false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{A11yNode, SnapshotMode};

    fn tree_with_button(name: &str) -> A11yNode {
        let mut root = A11yNode::new("WebArea", "Page");
        root.push(A11yNode::new("heading", "Welcome"));
        root.push(
            A11yNode::new("button", name)
                .with_ref("e1")
                .with_actionable(),
        );
        root
    }

    fn snap(doc: &str, url: &str, tree: A11yNode) -> Snapshot {
        Snapshot {
            document_id: doc.into(),
            url: url.into(),
            mode: SnapshotMode::Interactive,
            root: tree,
        }
    }

    #[test]
    fn line_diff_marks_added_removed() {
        let base = snap("d1", "https://a.example", tree_with_button("Old"));
        let current = snap("d1", "https://a.example", tree_with_button("New"));
        let d = diff_snapshots(&base, &current);
        assert!(!d.url_changed);
        assert!(
            d.removed_lines.iter().any(|l| l.contains("Old")),
            "removed: {:?}",
            d.removed_lines
        );
        assert!(
            d.added_lines.iter().any(|l| l.contains("New")),
            "added: {:?}",
            d.added_lines
        );
    }

    #[test]
    fn unchanged_snapshots_produce_empty_diff() {
        let a = snap("d1", "https://a.example", tree_with_button("Go"));
        let d = diff_snapshots(&a, &a);
        assert!(!d.url_changed);
        assert!(d.added_lines.is_empty());
        assert!(d.removed_lines.is_empty());
    }

    #[test]
    fn url_change_short_circuits_to_full_snapshot() {
        let base = snap("d1", "https://a.example", tree_with_button("Go"));
        let navigated = snap("d2", "https://b.example", tree_with_button("New page"));
        let d = diff_snapshots(&base, &navigated);
        assert!(d.url_changed);
        // Full new content in added, nothing removed.
        assert!(!d.added_lines.is_empty());
        assert!(d.added_lines.iter().any(|l| l.contains("New page")));
        assert!(d.removed_lines.is_empty());
        assert_eq!(d.base_url, "https://a.example");
    }
}
