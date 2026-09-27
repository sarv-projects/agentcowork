//! The verify cascade (sandraschi / ChatGPT `guidance.md` pattern):
//! observe → **one** action → re-observe → assert the expected state with a
//! locator → retry with backoff → **halt-over-guess** (never continue on an
//! unverified outcome).
//!
//! `FIX-17` changed one rule here, and it was a real defect: `UiGone` was
//! **confirmed** whenever the read produced no tree. So an elevation-blocked
//! window — or a provider that never answered — was read as "the dialog closed,
//! confirmed". Absence may only be inferred from a read that covered the target,
//! so the observer now reports **how** the read went and the cascade refuses to
//! conclude anything from a read that could not look (`REQ-CUA-006`,
//! `REQ-CUA-003`).

use std::time::{Duration, Instant};

use crate::types::{OcrWord, ReadNode, VerifyOutcome};
use crate::uia::UiaReadStatus;

/// Where the expected state lives after an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    /// A named a11y node should exist (UIA tree).
    UiName(String),
    /// OCR text should appear in the given window region.
    OcrText {
        text: String,
        region: crate::types::Region,
    },
    /// A named node should be gone (e.g. the dialog closed).
    UiGone(String),
}

/// How well the observation behind a verification established what it saw.
///
/// Derived from the read's [`UiaReadStatus`]; the default is the conservative one
/// for an observer that does not report a status (so an old `Observer`
/// implementation cannot silently regain the false-confirmation behaviour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadConfidence {
    /// The read covered the target: absence is a fact.
    Conclusive,
    /// The read was partial: absence is unproven.
    Partial,
    /// The read could not look: nothing may be concluded.
    Unknown,
}

impl ReadConfidence {
    /// May the cascade conclude "the element is gone" from this read?
    pub fn may_infer_absence(self) -> bool {
        matches!(self, ReadConfidence::Conclusive)
    }

    /// From a read status.
    pub fn from_status(status: &UiaReadStatus) -> Self {
        if status.may_infer_absence() {
            ReadConfidence::Conclusive
        } else if status.is_usable() {
            ReadConfidence::Partial
        } else {
            ReadConfidence::Unknown
        }
    }
}

/// The observable world the verifier re-reads after each action.
pub trait Observer: Send + Sync {
    /// Re-read the window after the action (a11y tree or None).
    fn read_tree(&self, window_id: u64) -> Option<ReadNode>;
    /// Re-OCR the window (vision fallback).
    fn ocr(&self, window_id: u64) -> Vec<OcrWord>;
    /// How conclusive this observer's read is.
    ///
    /// The default is [`ReadConfidence::Conclusive`] when a tree came back and
    /// [`ReadConfidence::Unknown`] when none did — the same rule the read path
    /// now applies, expressed once so a caller that does not track statuses
    /// still cannot turn "no tree" into "confirmed gone".
    fn read_confidence(&self, _window_id: u64) -> ReadConfidence {
        ReadConfidence::Conclusive
    }
}

/// A null observer for tests that drive the cascade directly.
pub struct FakeObserver {
    pub tree: Option<ReadNode>,
    pub words: Vec<OcrWord>,
    /// Reported by [`Observer::read_confidence`]; defaults to the tree's own
    /// shape, so a test that leaves it alone keeps the pre-`FIX-17` behaviour for
    /// a *present* tree and the safe one for an absent tree.
    pub confidence: Option<ReadConfidence>,
}
impl Observer for FakeObserver {
    fn read_tree(&self, _window_id: u64) -> Option<ReadNode> {
        self.tree.clone()
    }
    fn ocr(&self, _window_id: u64) -> Vec<OcrWord> {
        self.words.clone()
    }
    fn read_confidence(&self, _window_id: u64) -> ReadConfidence {
        self.confidence.unwrap_or(if self.tree.is_some() {
            ReadConfidence::Conclusive
        } else {
            ReadConfidence::Unknown
        })
    }
}

pub struct Verifier {
    pub max_attempts: u32,
    pub backoff: Duration,
}

impl Default for Verifier {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff: Duration::from_millis(400),
        }
    }
}

impl Verifier {
    /// Check the locator against one observation.
    ///
    /// `confidence` gates the **absence** locators only: an observation that
    /// could not look cannot prove something is gone, and one that was partial
    /// cannot either. A positive locator (`UiName`, `OcrText`) is unaffected — a
    /// control we *did* see is evidence either way.
    pub fn satisfied_with(
        locator: &Locator,
        tree: Option<&ReadNode>,
        words: &[OcrWord],
        confidence: ReadConfidence,
    ) -> bool {
        match locator {
            Locator::UiName(name) => tree.and_then(|t| t.find_by_name(name)).is_some(),
            Locator::UiGone(name) => {
                confidence.may_infer_absence() && tree.and_then(|t| t.find_by_name(name)).is_none()
            }
            Locator::OcrText { text, region } => {
                let needle = text.to_ascii_lowercase();
                words
                    .iter()
                    .filter(|w| region.contains(w.x, w.y))
                    .any(|w| w.text.to_ascii_lowercase().contains(&needle))
            }
        }
    }

    /// [`Self::satisfied_with`] with a conclusive read — the historical
    /// behaviour, kept for callers that have established their read covered the
    /// target.
    pub fn satisfied(locator: &Locator, tree: Option<&ReadNode>, words: &[OcrWord]) -> bool {
        Self::satisfied_with(locator, tree, words, ReadConfidence::Conclusive)
    }

    /// Run the cascade: poll the observer until the locator is satisfied or
    /// attempts are exhausted → `Halt` (never guess).
    pub fn verify(
        &self,
        window_id: u64,
        locator: &Locator,
        observer: &dyn Observer,
    ) -> VerifyOutcome {
        self.cascade(window_id, locator, observer, None)
    }

    /// Time-boxed variant for tests: same cascade, but stop early if the
    /// deadline passes (returns Halt).
    pub fn verify_until(
        &self,
        window_id: u64,
        locator: &Locator,
        observer: &dyn Observer,
        deadline: Instant,
    ) -> VerifyOutcome {
        self.cascade(window_id, locator, observer, Some(deadline))
    }

    /// The one implementation both entry points share.
    fn cascade(
        &self,
        window_id: u64,
        locator: &Locator,
        observer: &dyn Observer,
        deadline: Option<Instant>,
    ) -> VerifyOutcome {
        for attempt in 1..=self.max_attempts {
            if let Some(deadline) = deadline
                && Instant::now() >= deadline
            {
                return VerifyOutcome::Halt {
                    attempts: attempt.saturating_sub(1).max(1),
                    reason: "deadline exceeded".into(),
                };
            }
            let tree = observer.read_tree(window_id);
            let words = observer.ocr(window_id);
            let confidence = observer.read_confidence(window_id);
            if Self::satisfied_with(locator, tree.as_ref(), &words, confidence) {
                return if attempt == 1 {
                    VerifyOutcome::Confirmed
                } else {
                    VerifyOutcome::ConfirmedAfterRetry { attempts: attempt }
                };
            }
            if attempt < self.max_attempts {
                std::thread::sleep(self.backoff);
            }
        }
        // `FIX-17` — an absence locator over a read that could not look halts with
        // the reason, instead of the pre-fix "confirmed" it used to return. A
        // caller reading `Confirmed` for `UiGone` on an unreadable window was
        // being told a dialog closed on the strength of a permission error.
        let confidence = observer.read_confidence(window_id);
        VerifyOutcome::Halt {
            attempts: self.max_attempts,
            reason: if !confidence.may_infer_absence() && matches!(locator, Locator::UiGone(_)) {
                format!(
                    "locator {locator:?} not satisfied, and the observation was {confidence:?} — \
                     absence cannot be inferred from a read that could not cover the target, so \
                     this is an unknown, not a confirmed absence"
                )
            } else {
                format!(
                    "locator {locator:?} not satisfied after {} attempts",
                    self.max_attempts
                )
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Region;

    fn node(name: &str) -> ReadNode {
        ReadNode {
            index_path: "1".into(),
            role: "Button".into(),
            name: name.into(),
            automation_id: None,
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            actionable: true,
            children: vec![],
        }
    }

    fn word(text: &str) -> OcrWord {
        OcrWord {
            text: text.into(),
            confidence: 90.0,
            x: 100,
            y: 100,
            width: 40,
            height: 10,
        }
    }

    #[test]
    fn confirmed_on_first_observation() {
        let v = Verifier::default();
        let obs = FakeObserver {
            tree: Some(node("Save")),
            words: vec![],
            confidence: None,
        };
        assert_eq!(
            v.verify(1, &Locator::UiName("save".into()), &obs),
            VerifyOutcome::Confirmed
        );
    }

    #[test]
    fn retries_then_halts_never_guesses() {
        let v = Verifier {
            max_attempts: 3,
            backoff: Duration::from_millis(1),
        };
        let obs = FakeObserver {
            tree: None,
            words: vec![],
            confidence: None,
        };
        match v.verify(1, &Locator::UiName("Save".into()), &obs) {
            VerifyOutcome::Halt { attempts, .. } => assert_eq!(attempts, 3),
            other => panic!("expected Halt, got {other:?}"),
        }
    }

    /// `FIX-17` — a `UiGone` locator is only *confirmed* by a read that covered
    /// the target. An unreadable window (elevation, a provider that never
    /// answered) halts with a reason instead: the pre-fix code returned
    /// `Confirmed` for any `None` tree, which read a permission error as a closed
    /// dialog.
    #[test]
    fn ui_gone_is_confirmed_by_a_complete_read_and_halted_by_an_unreadable_one() {
        let v = Verifier {
            max_attempts: 2,
            backoff: Duration::from_millis(1),
        };
        // A read that covered the target and found the dialog gone.
        let conclusive = FakeObserver {
            tree: Some(node("Editor")),
            words: vec![],
            confidence: Some(ReadConfidence::Conclusive),
        };
        assert_eq!(
            v.verify(1, &Locator::UiGone("Confirm dialog".into()), &conclusive),
            VerifyOutcome::Confirmed
        );
        // A read that could not look: the same "no dialog" is an unknown.
        for confidence in [ReadConfidence::Unknown, ReadConfidence::Partial] {
            let unreadable = FakeObserver {
                tree: None,
                words: vec![],
                confidence: Some(confidence),
            };
            match v.verify(1, &Locator::UiGone("Confirm dialog".into()), &unreadable) {
                VerifyOutcome::Halt { reason, .. } => {
                    assert!(
                        reason.contains("absence cannot be inferred"),
                        "expected the unknown-region reason, got: {reason}"
                    );
                }
                other => panic!("expected Halt for {confidence:?}, got {other:?}"),
            }
        }
    }

    /// The default confidence is derived from the tree, so an `Observer` that does
    /// not track statuses still cannot turn "no tree" into "confirmed gone".
    #[test]
    fn an_observer_without_a_status_still_cannot_confirm_an_absence_it_cannot_see() {
        let v = Verifier {
            max_attempts: 1,
            backoff: Duration::from_millis(1),
        };
        let obs = FakeObserver {
            tree: None,
            words: vec![],
            confidence: None,
        };
        assert_eq!(obs.read_confidence(1), ReadConfidence::Unknown);
        assert!(matches!(
            v.verify(1, &Locator::UiGone("Confirm dialog".into()), &obs),
            VerifyOutcome::Halt { .. }
        ));
    }

    /// A *positive* locator is unaffected by the confidence gate: a control we did
    /// see is evidence even from a partial read.
    #[test]
    fn a_positive_locator_still_confirms_from_a_partial_read() {
        let v = Verifier {
            max_attempts: 1,
            backoff: Duration::from_millis(1),
        };
        let obs = FakeObserver {
            tree: Some(node("Save")),
            words: vec![],
            confidence: Some(ReadConfidence::Partial),
        };
        assert_eq!(
            v.verify(1, &Locator::UiName("Save".into()), &obs),
            VerifyOutcome::Confirmed
        );
    }

    #[test]
    fn ocr_locator_matches_within_region() {
        let v = Verifier::default();
        let obs = FakeObserver {
            tree: None,
            words: vec![word("Submit")],
            confidence: None,
        };
        let region = Region {
            x: 0,
            y: 0,
            width: 500,
            height: 500,
        };
        assert_eq!(
            v.verify(
                1,
                &Locator::OcrText {
                    text: "submit".into(),
                    region
                },
                &obs
            ),
            VerifyOutcome::Confirmed
        );
    }

    #[test]
    fn ocr_locator_respects_region() {
        let v = Verifier::default();
        let obs = FakeObserver {
            tree: None,
            words: vec![word("Submit")],
            confidence: None,
        };
        let far = Region {
            x: 1000,
            y: 1000,
            width: 10,
            height: 10,
        };
        let out = v.verify(
            1,
            &Locator::OcrText {
                text: "submit".into(),
                region: far,
            },
            &obs,
        );
        assert!(matches!(out, VerifyOutcome::Halt { .. }));
    }

    #[test]
    fn ui_gone_confirmed_when_dialog_closed() {
        let v = Verifier::default();
        let obs = FakeObserver {
            tree: Some(node("Editor")),
            words: vec![],
            confidence: Some(ReadConfidence::Conclusive),
        };
        assert_eq!(
            v.verify(1, &Locator::UiGone("Confirm dialog".into()), &obs),
            VerifyOutcome::Confirmed
        );
    }
}
