//! Client-side semantic compression for MCP tool results: a **bounded preview**
//! plus the artifact-ref seam, so a large result is referenced rather than
//! inlined raw.
//!
//! This is the *result* side of the same principle as `ARCH/13-CAPABILITY.md`
//! §6's loading modes: the model must never receive a raw dump it cannot use.
//! Definitions are budgeted per activation (`eager` / `catalog` / `on-demand`);
//! results are budgeted per delivery.
//!
//! ## The no-silent-loss rule
//!
//! Truncation here is always **visible and accounted**, never silent:
//!
//! - a preview that was cut reports `truncated: true` and the *full* byte
//!   count, so a consumer can never mistake it for the whole value;
//! - [`BoundedPreview::inline`] returns `None` for a truncated value, so the
//!   type system refuses the one use that would be a silent loss — passing a
//!   partial value off as the result;
//! - the helper **never invents an artifact reference**. A ref is only as good
//!   as the artifact behind it, and writing one is a local persistent mutation
//!   owned elsewhere (`29`/DEC-032). The seam is a field the caller fills after
//!   its own write succeeds, so an unwritten artifact can never be advertised.
//!
//! ## Integration seam (who writes the artifact)
//!
//! The MCP crate is a protocol layer: it reads a reply and shapes it, and it
//! has no workspace, no store, and no identity for a work item. The component
//! that must own the write is the **work/capability result path** — the same
//! place a receipt is emitted (`ARCH/29-ARTIFACTS.md` §3) and where
//! `31`-level artifact creation already belongs. That component calls
//! [`bounded_preview`], writes the full bytes through the artifact gateway, and
//! then sets [`BoundedPreview::with_artifact_ref`] with the ref it received.
//! Recording that here rather than guessing it: inventing a writer inside the
//! protocol crate would be a second artifact store (DEC-002, DEC-005).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The URI scheme for an artifact reference (DEC-032 working token; the final
/// scheme renames with the brand, `ARCH/29` §5 / OQ-ART-02).
pub const ARTIFACT_URI_SCHEME: &str = "eaios";

/// The default preview budget for one tool result.
///
/// A budget, not a cap on the real result: the full value is never destroyed by
/// this helper, only its *inline* form is bounded. The figure is the same
/// order as the tool-output budgets the verified references use
/// (50 KiB / 2 000 lines), chosen conservatively because this crate cannot see
/// what the receiving context has left (`16` §3 owns that).
pub const DEFAULT_PREVIEW_BYTES: usize = 8 * 1024;

/// A bounded view of one tool result.
///
/// The three states are exhaustive and each is explicit: inlined whole,
/// truncated-and-needs-a-ref, or empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundedPreview {
    /// The bounded text, at most `preview_bytes` long.
    pub preview: String,
    /// The budget this preview was cut to.
    pub preview_bytes: usize,
    /// The **full** serialized size of the value, so a consumer can report the
    /// real magnitude instead of guessing from the preview.
    pub total_bytes: usize,
    /// Whether anything was dropped from the inline form.
    pub truncated: bool,
    /// The artifact reference for the full value, once a writer has produced
    /// one. `None` while `truncated` is `true` means "the full value is not
    /// reachable yet" — never "the full value is this preview".
    pub artifact_ref: Option<String>,
}

impl BoundedPreview {
    /// The text that is safe to inline as *the* result, or `None` when the
    /// value was cut.
    ///
    /// Returning `None` rather than a partial string is the no-silent-loss rule
    /// made structural: a caller that ignores truncation gets a compile error
    /// at the call site rather than a plausible-looking wrong answer in a
    /// model context.
    pub fn inline(&self) -> Option<&str> {
        if self.truncated {
            None
        } else {
            Some(&self.preview)
        }
    }

    /// Does this result need an artifact before the full value is reachable?
    pub fn requires_artifact_ref(&self) -> bool {
        self.truncated && self.artifact_ref.is_none()
    }

    /// Attach the reference an artifact write produced.
    ///
    /// The ref is set even when the value was **not** truncated when the caller
    /// wrote it anyway (an artifact may be a deliberate work product); the
    /// helper never removes one.
    pub fn with_artifact_ref(mut self, artifact_ref: impl Into<String>) -> Self {
        self.artifact_ref = Some(artifact_ref.into());
        self
    }
}

/// Bound one JSON tool result for inline delivery.
///
/// `max_bytes` is a budget in **bytes of the compact serialization**, which is
/// what actually crosses the context boundary; the preview is then cut on a
/// UTF-8 character boundary so a cut preview is always valid text.
pub fn bounded_preview(value: &Value, max_bytes: usize) -> BoundedPreview {
    let serialized = value.to_string();
    let total_bytes = serialized.len();
    if total_bytes <= max_bytes {
        return BoundedPreview {
            preview: serialized,
            preview_bytes: max_bytes,
            total_bytes,
            truncated: false,
            artifact_ref: None,
        };
    }
    // Never split a multi-byte character: a partial code point is not text and
    // would fail to re-encode downstream.
    let mut end = max_bytes;
    while end > 0 && !serialized.is_char_boundary(end) {
        end -= 1;
    }
    BoundedPreview {
        preview: serialized[..end].to_string(),
        preview_bytes: end,
        total_bytes,
        truncated: true,
        artifact_ref: None,
    }
}

/// The bounded preview of one tool result, at [`DEFAULT_PREVIEW_BYTES`].
pub fn default_preview(value: &Value) -> BoundedPreview {
    bounded_preview(value, DEFAULT_PREVIEW_BYTES)
}

/// The artifact reference for an artifact id (DEC-032's working scheme token).
///
/// This only *formats* a reference the writer already owns. It performs no
/// existence check, so it must never be called before the write succeeds.
pub fn artifact_ref(artifact_id: &str) -> String {
    format!("{ARTIFACT_URI_SCHEME}://artifact/{artifact_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_result_is_inlined_whole() {
        let value = serde_json::json!({"ok": true, "items": [1, 2, 3]});
        let preview = bounded_preview(&value, 1024);
        assert!(!preview.truncated);
        assert_eq!(preview.total_bytes, value.to_string().len());
        assert_eq!(preview.inline(), Some(value.to_string().as_str()));
        assert!(!preview.requires_artifact_ref());
    }

    #[test]
    fn a_large_result_is_truncated_and_refuses_to_be_inlined() {
        let value = serde_json::json!({"rows": "x".repeat(50_000)});
        let preview = bounded_preview(&value, 1_000);
        assert!(preview.truncated);
        assert!(preview.preview.len() <= 1_000);
        // The full magnitude is reported, never just the preview's size.
        assert_eq!(preview.total_bytes, value.to_string().len());
        assert!(preview.total_bytes > 10_000);
        // The no-silent-loss rule: no partial value is offered as the result.
        assert_eq!(preview.inline(), None);
        assert!(preview.requires_artifact_ref());
        assert!(preview.artifact_ref.is_none());
    }

    #[test]
    fn truncation_lands_on_a_character_boundary() {
        // Each entry is 4 bytes of a multi-byte character, so a naive byte cut
        // at 5 would split one.
        let value = serde_json::json!(["€€€€€"]);
        for budget in 0..24 {
            let preview = bounded_preview(&value, budget);
            assert!(
                preview.preview.is_char_boundary(preview.preview.len()),
                "budget {budget} produced a split character"
            );
            // Whatever the budget, the accounting stays truthful.
            assert!(preview.preview.len() <= budget || budget == 0);
            if !preview.truncated {
                assert_eq!(preview.total_bytes, value.to_string().len());
            }
        }
    }

    #[test]
    fn a_written_artifact_ref_makes_a_truncated_result_reachable() {
        let value = serde_json::json!({"rows": "x".repeat(50_000)});
        let preview = default_preview(&value).with_artifact_ref(artifact_ref("art-7"));
        assert!(preview.truncated);
        assert_eq!(
            preview.artifact_ref.as_deref(),
            Some("eaios://artifact/art-7")
        );
        // Still not inlinable: the ref points at the full value, it does not
        // turn a preview into the value.
        assert_eq!(preview.inline(), None);
        assert!(!preview.requires_artifact_ref());
    }

    #[test]
    fn the_helper_never_invents_a_ref() {
        let value = serde_json::json!({"rows": "x".repeat(50_000)});
        let preview = default_preview(&value);
        assert!(preview.artifact_ref.is_none());
    }

    #[test]
    fn the_default_budget_bounds_a_gigantic_result() {
        let value = serde_json::json!({"blob": "y".repeat(2_000_000)});
        let preview = default_preview(&value);
        assert_eq!(preview.preview_bytes, DEFAULT_PREVIEW_BYTES);
        assert!(preview.preview.len() <= DEFAULT_PREVIEW_BYTES);
        assert_eq!(preview.total_bytes, value.to_string().len());
    }
}
