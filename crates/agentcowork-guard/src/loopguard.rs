//! P7.7 — loop guard. A SHA-256 circuit breaker that detects when the agent
//! is repeating the same (tool, args-hash) pair without progress. The
//! breaker tracks a rolling window of step hashes; when the same hash
//! repeats past a threshold, the coordinator is told to stop and escalate
//! (or the executor refuses the repeated action).

use sha2::{Digest, Sha256};

/// SHA-256 of a step's identity (tool + canonical args).
pub fn step_hash(tool: &str, args: &str) -> String {
    let mut h = Sha256::new();
    h.update(tool.as_bytes());
    h.update([0u8]);
    h.update(args.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Rolling-window loop detector. `max_repeats` repeats of the same step
/// hash within `window` steps trips the breaker.
#[derive(Debug, Clone)]
pub struct LoopGuard {
    window: usize,
    max_repeats: usize,
    recent: Vec<String>,
}

impl LoopGuard {
    pub fn new(window: usize, max_repeats: usize) -> Self {
        Self {
            window,
            recent: Vec::new(),
            max_repeats,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(8, 3)
    }

    /// Record one step; returns true if the breaker trips.
    pub fn record(&mut self, hash: &str) -> bool {
        self.recent.push(hash.to_string());
        if self.recent.len() > self.window {
            self.recent.remove(0);
        }
        let count = self.recent.iter().filter(|h| *h == hash).count();
        count >= self.max_repeats
    }

    /// Number of steps currently in the window.
    pub fn steps(&self) -> usize {
        self.recent.len()
    }

    /// Reset (after escalation / user intervention).
    pub fn reset(&mut self) {
        self.recent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable() {
        assert_eq!(
            step_hash("shell.exec", "ls -la"),
            step_hash("shell.exec", "ls -la")
        );
        assert_ne!(
            step_hash("shell.exec", "ls -la"),
            step_hash("shell.exec", "ls -l")
        );
    }

    #[test]
    fn trips_on_repeat() {
        let mut g = LoopGuard::new(8, 3);
        let h = step_hash("browser.click", "x=1");
        assert!(!g.record(&h));
        assert!(!g.record(&h));
        assert!(g.record(&h), "third repeat should trip");
    }

    #[test]
    fn does_not_trip_on_progress() {
        let mut g = LoopGuard::new(8, 3);
        for i in 0..20 {
            let h = step_hash("tool", &format!("step{i}"));
            assert!(!g.record(&h), "progress should never trip");
        }
    }

    #[test]
    fn window_expires_old_repeats() {
        let mut g = LoopGuard::new(3, 3);
        let h = step_hash("a", "b");
        g.record(&h);
        g.record(&h);
        // two different steps push the repeats out of a window of 3
        g.record(&step_hash("x", "1"));
        g.record(&step_hash("y", "2"));
        assert!(!g.record(&h), "repeat aged out of window");
    }

    #[test]
    fn reset_clears() {
        let mut g = LoopGuard::new(8, 3);
        let h = step_hash("a", "b");
        g.record(&h);
        g.record(&h);
        g.reset();
        assert_eq!(g.steps(), 0);
        assert!(!g.record(&h));
    }
}
