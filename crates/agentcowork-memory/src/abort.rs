//! P36 (C3) — abortable retrieval: cancel an in-flight fuse when the owning
//! turn dies.
//!
//! Retrieval runs can outlive their turn (multi-signal fusion, RAG reads).
//! This module gives every stage a cooperative cancel point: a
//! [`CancellationToken`] the owning turn flips on teardown, and an abortable
//! fuse that checks it between signals and between documents.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancel flag. Cheap to construct, cheap to check.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// The seam any cancel source can implement (tests can fake one without
/// touching the token).
pub trait AbortHandle: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

impl AbortHandle for CancellationToken {
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

/// A no-op handle: always proceeds (default path).
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverAbort;

impl AbortHandle for NeverAbort {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortError {
    /// The owning turn was torn down mid-retrieval.
    Cancelled,
}

/// Abortable weighted RRF fuse: checks the handle before adding each signal.
/// If any signal's hits are missing due to cancellation, the whole fuse
/// aborts with [`AbortError::Cancelled`] — no partially-fused answer is
/// ever handed to a dead turn.
pub fn fuse_abortable(
    signals: &[crate::fusion::Signal],
    k: f64,
    abort: Option<&dyn AbortHandle>,
) -> Result<Vec<(String, f64)>, AbortError> {
    if abort.is_some_and(|a| a.is_cancelled()) {
        return Err(AbortError::Cancelled);
    }
    let mut partial: Vec<(String, f64)> = Vec::new();
    for sig in signals {
        if abort.is_some_and(|a| a.is_cancelled()) {
            return Err(AbortError::Cancelled);
        }
        for (rank, (id, _score)) in sig.hits.iter().enumerate() {
            if abort.is_some_and(|a| a.is_cancelled()) {
                return Err(AbortError::Cancelled);
            }
            let contrib = sig.weight / (k + rank as f64);
            if let Some((_, s)) = partial.iter_mut().find(|(i, _)| i == id) {
                *s += contrib;
            } else {
                partial.push((id.clone(), contrib));
            }
        }
    }
    partial.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(partial)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> CancellationToken {
        CancellationToken::new()
    }

    #[test]
    fn no_abort_fuses_normally() {
        let a = token();
        let h1: [(String, f64); 2] = [("x".into(), 1.0), ("y".into(), 0.5)];
        let h2: [(String, f64); 1] = [("y".into(), 1.0)];
        let signals = vec![
            crate::fusion::Signal {
                weight: 1.0,
                hits: &h1,
            },
            crate::fusion::Signal {
                weight: 0.5,
                hits: &h2,
            },
        ];
        let fused = fuse_abortable(&signals, 60.0, Some(&a)).unwrap();
        assert_eq!(fused.len(), 2);
        // y appears in both signals → fused score 1.5/k beats x's 1.0/k.
        assert_eq!(fused[0].0, "y");
    }

    #[test]
    fn cancelled_before_start_errors() {
        let token = token();
        token.cancel();
        let h: [(String, f64); 1] = [("x".into(), 1.0)];
        let signals = vec![crate::fusion::Signal {
            weight: 1.0,
            hits: &h,
        }];
        let out = fuse_abortable(&signals, 60.0, Some(&token));
        assert_eq!(out, Err(AbortError::Cancelled));
    }

    #[test]
    fn cancel_mid_fuse_stops_cleanly() {
        // A token cancelled between two fuse calls aborts the second before
        // any hit is added — the half-fused first pass is never surfaced.
        let token = token();
        let h1: [(String, f64); 1] = [("x".into(), 1.0)];
        let h2: [(String, f64); 1] = [("y".into(), 1.0)];
        let signals = vec![
            crate::fusion::Signal {
                weight: 1.0,
                hits: &h1,
            },
            crate::fusion::Signal {
                weight: 1.0,
                hits: &h2,
            },
        ];
        let per_signal = fuse_abortable(&signals[..1], 60.0, Some(&token)).unwrap();
        assert_eq!(per_signal.len(), 1);
        token.cancel();
        let out = fuse_abortable(&signals, 60.0, Some(&token));
        assert_eq!(out, Err(AbortError::Cancelled));
    }

    #[test]
    fn never_abort_default() {
        let h: [(String, f64); 1] = [("x".into(), 1.0)];
        let signals = vec![crate::fusion::Signal {
            weight: 1.0,
            hits: &h,
        }];
        let fused = fuse_abortable(&signals, 60.0, Some(&NeverAbort)).unwrap();
        assert_eq!(fused.len(), 1);
    }
}
