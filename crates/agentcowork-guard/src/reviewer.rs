//! P51.16 — confidence-gated auto reviewer with a circuit breaker.
//!
//! Fail-closed: a missing confidence, a confidence below the floor, or an
//! open [`ReviewerBreaker`] all escalate to a human. The breaker trips after
//! `threshold` recorded rejects/errors and recloses on [`ReviewerBreaker::reset`].

/// Static reviewer tuning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReviewerConfig {
    /// Minimum model confidence for the auto path; below this → escalate.
    pub confidence_floor: f64,
    /// Run-level auto budget (0 disables auto entirely). The count itself is
    /// enforced by the caller; `auto_review` treats 0 as always-escalate so
    /// the field is never silently ignored.
    pub max_auto_per_run: u32,
}

impl ReviewerConfig {
    pub fn new(confidence_floor: f64, max_auto_per_run: u32) -> Self {
        Self {
            confidence_floor,
            max_auto_per_run,
        }
    }
}

/// What the reviewer decides for one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOutcome {
    /// Proceed on the auto path.
    AutoAllow,
    /// Escalate to a human with a reason.
    Escalate(String),
}

/// Circuit breaker over reviewer rejects/errors.
///
/// Closed (new / after [`Self::reset`]) means auto may proceed;
/// open (failures at/above `threshold`) means every review escalates.
/// `cooled_down == true` means the breaker has cooled/reset and is closed;
/// recording a reject/error re-arms it (`cooled_down = false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerBreaker {
    pub failures: u32,
    pub threshold: u32,
    pub cooled_down: bool,
}

impl ReviewerBreaker {
    pub fn new(threshold: u32) -> Self {
        Self {
            failures: 0,
            threshold,
            cooled_down: true,
        }
    }

    fn trip_if_needed(&mut self) {
        if self.threshold == 0 || self.failures >= self.threshold {
            self.cooled_down = false;
        }
    }

    /// Record a human/model rejection of an auto proposal.
    pub fn record_reject(&mut self) {
        self.failures = self.failures.saturating_add(1);
        self.cooled_down = false;
        self.trip_if_needed();
    }

    /// Record a reviewer/executor error.
    pub fn record_error(&mut self) {
        self.failures = self.failures.saturating_add(1);
        self.cooled_down = false;
        self.trip_if_needed();
    }

    /// Reclose the breaker after cooldown / human intervention.
    pub fn reset(&mut self) {
        self.failures = 0;
        self.cooled_down = true;
    }

    /// Is the breaker currently open (traffic must escalate)?
    pub fn is_open(&self) -> bool {
        if self.threshold == 0 {
            return true;
        }
        !self.cooled_down && self.failures >= self.threshold
    }

    /// Breaker check: `true` when open (caller must escalate), `false`
    /// when closed. Alias of [`Self::is_open`].
    pub fn check(&self) -> bool {
        self.is_open()
    }
}

/// Confidence-gated review. Fail-closed: `None` confidence, a confidence
/// below `cfg.confidence_floor` (or non-finite), a zero auto budget, or an
/// open breaker all return [`ReviewOutcome::Escalate`].
pub fn auto_review(
    decision_confidence: Option<f64>,
    cfg: &ReviewerConfig,
    breaker: &ReviewerBreaker,
) -> ReviewOutcome {
    if breaker.check() {
        return ReviewOutcome::Escalate("reviewer breaker open".to_string());
    }
    if cfg.max_auto_per_run == 0 {
        return ReviewOutcome::Escalate("auto budget is zero".to_string());
    }
    match decision_confidence {
        None => ReviewOutcome::Escalate("missing decision confidence".to_string()),
        Some(c) if !c.is_finite() || c < cfg.confidence_floor => ReviewOutcome::Escalate(format!(
            "confidence {c} below floor {}",
            cfg.confidence_floor
        )),
        Some(_) => ReviewOutcome::AutoAllow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ReviewerConfig {
        ReviewerConfig::new(0.8, 10)
    }

    fn closed_breaker() -> ReviewerBreaker {
        ReviewerBreaker::new(3)
    }

    #[test]
    fn reviewer_fail_closed_on_none_confidence() {
        let outcome = auto_review(None, &cfg(), &closed_breaker());
        assert!(matches!(outcome, ReviewOutcome::Escalate(_)));
    }

    #[test]
    fn reviewer_escalates_below_floor() {
        assert!(matches!(
            auto_review(Some(0.5), &cfg(), &closed_breaker()),
            ReviewOutcome::Escalate(_)
        ));
        assert_eq!(
            auto_review(Some(0.9), &cfg(), &closed_breaker()),
            ReviewOutcome::AutoAllow
        );
    }

    #[test]
    fn breaker_opens_after_threshold_parks() {
        let mut breaker = ReviewerBreaker::new(2);
        assert!(!breaker.check());
        breaker.record_reject();
        assert!(!breaker.check());
        breaker.record_error();
        assert!(breaker.check(), "breaker should be open at threshold");
        // Open breaker parks even a high-confidence decision.
        let outcome = auto_review(Some(0.99), &cfg(), &breaker);
        assert!(matches!(outcome, ReviewOutcome::Escalate(_)));
    }

    #[test]
    fn breaker_reset_recloses() {
        let mut breaker = ReviewerBreaker::new(1);
        breaker.record_reject();
        assert!(breaker.check());
        breaker.reset();
        assert!(!breaker.check(), "reset should reclose the breaker");
        assert_eq!(
            auto_review(Some(0.9), &cfg(), &breaker),
            ReviewOutcome::AutoAllow
        );
    }
}
