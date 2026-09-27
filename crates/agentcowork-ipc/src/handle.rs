//! Oversized-payload truncation → `ref:` handles (spec C10 pass-by-reference).
//!
//! Frames are capped at [`crate::frame::MAX_FRAME_LEN`] (16 MiB), but even
//! that is wasteful for big results (a multi-MB audit NDJSON export, a browser
//! snapshot, a vault dump). The app layer stores the full payload in a
//! [`HandleStore`] and sends `{"ref": "handle:<id>"}` instead; the peer
//! fetches it once with the `refs/get` method. Handles are **one-shot**
//! (take-once) so a fetched payload is freed immediately.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Payloads larger than this are truncated into `ref:` handles at the app
/// layer. 1 MiB keeps the transport snappy while staying far under the 16 MiB
/// frame cap.
pub const TRUNCATION_THRESHOLD: usize = 1024 * 1024;

/// A one-shot reference to a stored payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleRef {
    pub id: u64,
}

impl HandleRef {
    /// Wire form: `ref:handle:<id>` (spec C10).
    pub fn wire(&self) -> String {
        format!("ref:handle:{}", self.id)
    }

    /// Parse `ref:handle:<id>` back into a handle.
    pub fn parse(s: &str) -> Option<Self> {
        s.strip_prefix("ref:handle:")
            .and_then(|id| id.parse().ok())
            .map(|id| HandleRef { id })
    }
}

/// What a peer receives for a payload: inline bytes, or a handle to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WirePayload {
    Inline(Vec<u8>),
    Ref(HandleRef),
}

/// Thread-safe store of large payloads behind `ref:` handles.
#[derive(Default)]
pub struct HandleStore {
    inner: Mutex<HashMap<u64, Vec<u8>>>,
    next: AtomicU64,
}

impl HandleStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// Store a payload; small ones stay inline, large ones become a handle.
    pub fn store(&self, payload: Vec<u8>) -> WirePayload {
        self.store_above(payload, TRUNCATION_THRESHOLD)
    }

    /// Store a payload with a caller-chosen threshold (P39.1 per-message-type
    /// budgets need 2–50 KB thresholds, far below the 1 MiB default).
    pub fn store_above(&self, payload: Vec<u8>, threshold: usize) -> WirePayload {
        if payload.len() <= threshold {
            return WirePayload::Inline(payload);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .lock()
            .expect("handle store poisoned")
            .insert(id, payload);
        WirePayload::Ref(HandleRef { id })
    }

    /// Take (one-shot) a stored payload; `None` on unknown/already-taken id.
    pub fn take(&self, id: u64) -> Option<Vec<u8>> {
        self.inner
            .lock()
            .expect("handle store poisoned")
            .remove(&id)
    }

    /// Number of payloads currently held behind handles.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("handle store poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_payload_stays_inline() {
        let store = HandleStore::new();
        let payload = vec![b'x'; 100];
        match store.store(payload.clone()) {
            WirePayload::Inline(bytes) => assert_eq!(bytes, payload),
            WirePayload::Ref(_) => panic!("small payload must stay inline"),
        }
        assert!(store.is_empty());
    }

    #[test]
    fn oversized_payload_becomes_ref_with_wire_form() {
        let store = HandleStore::new();
        let payload = vec![b'x'; TRUNCATION_THRESHOLD + 1];
        match store.store(payload.clone()) {
            WirePayload::Ref(r) => {
                let wire = r.wire();
                assert!(wire.starts_with("ref:handle:"));
                assert_eq!(HandleRef::parse(&wire), Some(r));
                // One-shot: take returns the bytes, then the store is empty.
                assert_eq!(store.take(r.id), Some(payload));
                assert_eq!(store.take(r.id), None);
                assert!(store.is_empty());
            }
            WirePayload::Inline(_) => panic!("oversized payload must become a ref"),
        }
    }

    #[test]
    fn boundary_at_threshold_stays_inline() {
        let store = HandleStore::new();
        let payload = vec![b'x'; TRUNCATION_THRESHOLD];
        assert!(matches!(store.store(payload), WirePayload::Inline(_)));
    }

    #[test]
    fn store_above_honors_custom_threshold() {
        let store = HandleStore::new();
        // 60 KB is well under the 1 MiB default threshold but over a 50 KB
        // per-type budget → must become a ref (P39.1).
        let payload = vec![b'x'; 60 * 1024];
        let r = store.store_above(payload, 50 * 1024);
        assert!(
            matches!(r, WirePayload::Ref(_)),
            "60KB must be a ref at a 50KB budget"
        );
        let small = vec![b'x'; 40 * 1024];
        assert!(matches!(
            store.store_above(small, 50 * 1024),
            WirePayload::Inline(_)
        ));
    }

    #[test]
    fn unknown_handle_take_returns_none() {
        let store = HandleStore::new();
        assert_eq!(store.take(999), None);
    }

    #[test]
    fn ids_are_monotonic() {
        let store = HandleStore::new();
        let big = vec![b'x'; TRUNCATION_THRESHOLD + 1];
        let a = store.store(big.clone());
        let b = store.store(big);
        if let (WirePayload::Ref(x), WirePayload::Ref(y)) = (a, b) {
            assert!(y.id > x.id);
        } else {
            panic!("expected refs");
        }
    }
}
