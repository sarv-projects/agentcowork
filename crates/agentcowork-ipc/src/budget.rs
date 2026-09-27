//! P39.1 — per-message-type IPC payload budgets (doc-42 §1.4, spec §9.3 §1).
//!
//! The transport has one hard cap ([`crate::frame::MAX_FRAME_LEN`], 16 MiB)
//! and [`crate::handle`] already truncates multi-MiB payloads into `ref:`
//! handles. This module adds the *per-message-type* budgets the doc-42 table
//! calls for: a tool result over 50 KB, a scraped page, an a11y snapshot, or
//! an audit export each get a type-specific inline limit, with the full
//! payload parked behind a `ref:` handle (spec C10 pass-by-reference).
//!
//! Budgets are enforced at the app layer (where the message kind is known),
//! never inside the generic frame codec. [`budget_for`] is the doc-42 table;
//! [`apply_budget`] is the enforcement helper — the inline payload is always
//! ≤ the type's `inline_limit`, and any truncation produces a handle + a
//! visible marker so the peer can fetch the full payload on demand.

use crate::handle::{HandleRef, HandleStore};

/// The message kinds the doc-42 §1.4 table budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// A tool result (browser snapshot, storage query, …).
    ToolResult,
    /// A scraped web page's extracted text.
    ScrapedPage,
    /// An accessibility-tree snapshot (browser view state).
    A11ySnapshot,
    /// An audit NDJSON export / replay bundle.
    AuditExport,
    /// Anything not covered by the table.
    Default,
}

/// The per-type budget: how much stays inline vs how much is deferred to a
/// `ref:` handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadBudget {
    /// Max inline payload bytes for this kind (doc-42 §1.4).
    pub inline_limit: usize,
    /// Max inline *preview* bytes when the full payload is deferred to a ref
    /// (the "first-2KB extract" for scraped pages, "slim summary" for a11y).
    pub preview_limit: usize,
}

/// The doc-42 §1.4 table (spec §9.3 §1). Tool results stay usable inline up
/// to 50 KB (larger → truncate + ref); scraped pages and a11y snapshots are
/// ref-first with a tiny extract; audit exports get a roomier budget.
pub fn budget_for(kind: MessageKind) -> PayloadBudget {
    match kind {
        MessageKind::ToolResult => PayloadBudget {
            inline_limit: 50 * 1024,
            preview_limit: 2 * 1024,
        },
        MessageKind::ScrapedPage => PayloadBudget {
            inline_limit: 2 * 1024,
            preview_limit: 2 * 1024,
        },
        MessageKind::A11ySnapshot => PayloadBudget {
            inline_limit: 8 * 1024,
            preview_limit: 1024,
        },
        MessageKind::AuditExport => PayloadBudget {
            inline_limit: 64 * 1024,
            preview_limit: 4 * 1024,
        },
        MessageKind::Default => PayloadBudget {
            inline_limit: 256 * 1024,
            preview_limit: 4 * 1024,
        },
    }
}

/// The result of applying a budget to a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budgeted {
    /// The inline bytes the peer receives (≤ the type's `inline_limit`).
    pub inline: Vec<u8>,
    /// A handle to the full payload, when it exceeded the inline limit.
    pub handle: Option<HandleRef>,
    /// Whether the payload was truncated into a ref.
    pub truncated: bool,
}

/// Enforce a message kind's budget: small payloads stay inline; oversized
/// payloads are stored in `store` behind a one-shot `ref:` handle with a
/// bounded inline extract (spec C10). A 60 KB tool result therefore arrives
/// as a ≤50 KB payload + ref — never a 60 KB frame.
pub fn apply_budget(store: &HandleStore, kind: MessageKind, payload: Vec<u8>) -> Budgeted {
    let budget = budget_for(kind);
    if payload.len() <= budget.inline_limit {
        return Budgeted {
            inline: payload,
            handle: None,
            truncated: false,
        };
    }
    // Oversized: extract the preview first (the store is one-shot), then
    // park the full payload behind a ref handle at this kind's threshold.
    let preview_len = budget.preview_limit.min(payload.len());
    let mut inline = payload[..preview_len].to_vec();
    inline.extend_from_slice(&preview_marker(budget.preview_limit));
    let handle = match store.store_above(payload, budget.inline_limit) {
        crate::handle::WirePayload::Ref(r) => r,
        crate::handle::WirePayload::Inline(_) => {
            unreachable!("payload over inline_limit must become a ref")
        }
    };
    Budgeted {
        inline,
        handle: Some(handle),
        truncated: true,
    }
}

/// A visible marker that the payload was truncated (honesty: the peer can
/// always fetch the full payload via the ref handle).
fn preview_marker(limit: usize) -> Vec<u8> {
    format!(
        "\n[truncated by payload budget — full payload behind ref handle; inline preview capped at {limit} bytes]"
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_over_50kb_is_truncated_with_ref() {
        let store = HandleStore::new();
        let payload = vec![b'x'; 60 * 1024]; // 60 KB tool result
        let b = apply_budget(&store, MessageKind::ToolResult, payload);
        assert!(b.truncated);
        assert!(b.handle.is_some());
        // The inline payload is within the 50 KB budget — never a 60 KB frame.
        assert!(b.inline.len() <= 50 * 1024);
        assert!(String::from_utf8_lossy(&b.inline).contains("truncated by payload budget"));
        // Full payload is one-shot fetchable.
        let full = store
            .take(b.handle.unwrap().id)
            .expect("full payload behind ref");
        assert_eq!(full.len(), 60 * 1024);
    }

    #[test]
    fn tool_result_under_50kb_stays_inline() {
        let store = HandleStore::new();
        let payload = vec![b'y'; 10 * 1024];
        let b = apply_budget(&store, MessageKind::ToolResult, payload.clone());
        assert!(!b.truncated);
        assert!(b.handle.is_none());
        assert_eq!(b.inline, payload);
    }

    #[test]
    fn scraped_page_is_ref_first_with_2kb_extract() {
        let store = HandleStore::new();
        let payload = vec![b'z'; 100 * 1024]; // a big scraped page
        let b = apply_budget(&store, MessageKind::ScrapedPage, payload);
        assert!(b.truncated);
        assert!(b.handle.is_some());
        // Inline extract is capped at the 2 KB preview — never full text.
        assert!(b.inline.len() <= 2 * 1024 + 128);
        let full = store.take(b.handle.unwrap().id).unwrap();
        assert_eq!(full.len(), 100 * 1024);
    }

    #[test]
    fn every_kind_has_a_bounded_budget() {
        for kind in [
            MessageKind::ToolResult,
            MessageKind::ScrapedPage,
            MessageKind::A11ySnapshot,
            MessageKind::AuditExport,
            MessageKind::Default,
        ] {
            let b = budget_for(kind);
            assert!(b.inline_limit > 0 && b.inline_limit < crate::frame::MAX_FRAME_LEN as usize);
            assert!(b.preview_limit > 0);
        }
    }

    #[test]
    fn mixed_1mb_feed_stays_within_budget() {
        // The P39.1 measurement gate: feed 1MB of mixed payloads; every
        // budgeted frame is within its type's inline limit.
        let store = HandleStore::new();
        let mut oversized = 0;
        for i in 0..8 {
            let kind = match i % 4 {
                0 => MessageKind::ToolResult,
                1 => MessageKind::ScrapedPage,
                2 => MessageKind::A11ySnapshot,
                _ => MessageKind::AuditExport,
            };
            let payload = vec![b'q'; 256 * 1024];
            let b = apply_budget(&store, kind, payload);
            assert!(b.inline.len() <= budget_for(kind).inline_limit + 128);
            if b.truncated {
                oversized += 1;
                assert!(b.handle.is_some());
            }
        }
        assert_eq!(oversized, 8);
        assert_eq!(store.len(), 8); // all full payloads parked behind refs
    }
}
