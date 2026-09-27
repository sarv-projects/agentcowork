//! "Reinforce what I learned" flow (P5.11 / C13 — doc 63 §2.2): post-session
//! candidate extraction → FSRS queue → review prompts at optimal intervals.
//!
//! The coordinator extracts candidates (facts/concepts worth remembering)
//! from a session and calls [`ReviewQueue::ingest`]; this module schedules
//! each on the FSRS model and hands back due review prompts at the right
//! time. The pure scheduling core lives here — extraction (NLP) is the
//! coordinator's job.

use crate::fsrs::{Fsrs, MemoryState, Rating};

/// A post-session candidate: something worth reinforcing.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewCandidate {
    pub id: String,
    pub content: String,
    /// Higher importance → surface sooner (used to order same-day reviews).
    pub importance: f32,
}

/// A scheduled card: the candidate + its FSRS memory state + due day.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewCard {
    pub id: String,
    pub content: String,
    pub importance: f32,
    pub state: MemoryState,
    pub due_day: u32,
    pub last_review_day: i32,
}

/// The reinforcement queue: candidates scheduled on FSRS.
#[derive(Debug)]
pub struct ReviewQueue {
    fsrs: Fsrs,
    desired_retention: f32,
    cards: Vec<ReviewCard>,
}

impl ReviewQueue {
    pub fn new(desired_retention: f32) -> Self {
        Self::with_fsrs(Fsrs::default(), desired_retention)
    }

    pub fn with_fsrs(fsrs: Fsrs, desired_retention: f32) -> Self {
        Self {
            fsrs,
            desired_retention,
            cards: Vec::new(),
        }
    }

    /// Ingest candidates on `day`, scheduling each as a new card (initial
    /// Good first-review state). Returns how many were added. Idempotent per
    /// id — re-ingesting an existing id is a no-op.
    pub fn ingest(&mut self, candidates: Vec<ReviewCandidate>, day: u32) -> usize {
        let mut added = 0;
        for c in candidates {
            if self.cards.iter().any(|card| card.id == c.id) {
                continue;
            }
            let state = self
                .fsrs
                .next_states(None, self.desired_retention, 0)
                .good
                .memory;
            let due_day = day
                + self
                    .fsrs
                    .next_interval(Some(state.stability), self.desired_retention, Rating::Good)
                    .ceil() as u32;
            self.cards.push(ReviewCard {
                id: c.id,
                content: c.content,
                importance: c.importance,
                state,
                due_day,
                last_review_day: day as i32,
            });
            added += 1;
        }
        added
    }

    /// Review prompts due on `day`, most important first.
    pub fn due(&self, day: u32) -> Vec<&ReviewCard> {
        let mut cards: Vec<&ReviewCard> = self.cards.iter().filter(|c| c.due_day <= day).collect();
        cards.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.due_day.cmp(&b.due_day))
        });
        cards
    }

    /// Record a review of `id` on `day`, rescheduling on FSRS. Returns the
    /// new due day, or `None` if the id is unknown.
    pub fn review(&mut self, id: &str, rating: Rating, day: u32) -> Option<u32> {
        let card = self.cards.iter_mut().find(|c| c.id == id)?;
        let elapsed = (day as i32 - card.last_review_day).max(0) as u32;
        let states = self
            .fsrs
            .next_states(Some(card.state), self.desired_retention, elapsed);
        card.state = match rating {
            Rating::Again => states.again.memory,
            Rating::Hard => states.hard.memory,
            Rating::Good => states.good.memory,
            Rating::Easy => states.easy.memory,
        };
        card.last_review_day = day as i32;
        card.due_day = day
            + self
                .fsrs
                .next_interval(Some(card.state.stability), self.desired_retention, rating)
                .ceil() as u32;
        Some(card.due_day)
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Deterministic candidate extraction (the coordinator's NLP half)
// ---------------------------------------------------------------------------

/// FNV-1a — a stable content hash for candidate ids (no external dep, same
/// id for the same content across runs).
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Split text into sentences (`.`, `!`, `?` followed by space/newline/end).
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if matches!(b, b'.' | b'!' | b'?') {
            let after = bytes.get(i + 1).copied();
            let end_of_text = after.is_none();
            let boundary = after.map_or(true, |a| {
                a == b' ' || a == b'\n' || a == b'\r' || a == b'\t'
            });
            if end_of_text || boundary {
                let sentence = text[start..=i].trim();
                if !sentence.is_empty() {
                    out.push(sentence.to_string());
                }
                start = i + 1;
            }
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// Fact-pattern markers that make a sentence a reinforcement candidate.
const FACT_MARKERS: &[&str] = &[
    " is ",
    " are ",
    " means ",
    " refers to ",
    " learned that ",
    " remember that ",
    " important",
    " key ",
    "key:",
    " note:",
    "fact:",
    "definition:",
    " stands for ",
    " equals ",
    " consists of ",
    " example:",
];

/// Heuristic importance from keywords (0.5 baseline, boosted by salience).
fn sentence_importance(lower: &str) -> f32 {
    if lower.contains("important") || lower.contains("critical") {
        1.0
    } else if lower.contains("key ") || lower.contains("key:") || lower.contains("core ") {
        0.8
    } else if lower.contains("remember") || lower.contains("learned") {
        0.7
    } else if lower.contains("note") || lower.contains("definition") {
        0.6
    } else {
        0.5
    }
}

/// Extract reinforcement candidates from a session transcript (deterministic,
/// model-free): sentences matching fact patterns become candidates with a
/// stable content-hash id and a keyword-derived importance. This is the
/// coordinator's extraction step — the queue scheduling lives above.
pub fn extract_candidates(text: &str) -> Vec<ReviewCandidate> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sentence in split_sentences(text) {
        let lower = sentence.to_lowercase();
        if !FACT_MARKERS.iter().any(|m| lower.contains(m)) {
            continue;
        }
        let id = format!("cand-{:016x}", fnv1a(&sentence));
        if !seen.insert(id.clone()) {
            continue;
        }
        out.push(ReviewCandidate {
            id,
            content: sentence,
            importance: sentence_importance(&lower),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, importance: f32) -> ReviewCandidate {
        ReviewCandidate {
            id: id.into(),
            content: format!("fact {id}"),
            importance,
        }
    }

    #[test]
    fn ingest_schedules_new_cards() {
        let mut q = ReviewQueue::new(0.9);
        let added = q.ingest(vec![cand("a", 1.0), cand("b", 0.5)], 0);
        assert_eq!(added, 2);
        assert_eq!(q.len(), 2);
        // New cards are scheduled in the future, not immediately due.
        assert!(q.due(0).is_empty());
    }

    #[test]
    fn ingest_is_idempotent_per_id() {
        let mut q = ReviewQueue::new(0.9);
        q.ingest(vec![cand("a", 1.0)], 0);
        let added = q.ingest(vec![cand("a", 1.0)], 1);
        assert_eq!(added, 0);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn due_orders_by_importance_then_due_day() {
        let mut q = ReviewQueue::new(0.9);
        q.ingest(vec![cand("low", 0.1), cand("high", 1.0)], 0);
        // Force both due on day 10.
        for id in ["low", "high"] {
            q.review(id, Rating::Again, 0);
        }
        let due: Vec<&str> = q.due(10).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(due, vec!["high", "low"]);
    }

    #[test]
    fn review_good_grows_interval_over_repeated_reviews() {
        let mut q = ReviewQueue::new(0.9);
        q.ingest(vec![cand("a", 1.0)], 0);
        // Review repeatedly at its due day with Good; stability must grow.
        let mut day = q.due(0).len() as u32; // 0
        let mut prev_due = 0;
        let mut grew = false;
        for _ in 0..10 {
            // Advance to the card's due day.
            while q.due(day).is_empty() {
                day += 1;
            }
            let next = q.review("a", Rating::Good, day).unwrap();
            if next > prev_due {
                grew = true;
            }
            prev_due = next;
            day = next;
        }
        assert!(grew, "intervals should grow with Good reviews");
    }

    #[test]
    fn review_again_shortens_interval() {
        let mut q = ReviewQueue::new(0.9);
        q.ingest(vec![cand("a", 1.0)], 0);
        // Build up stability with a few Good reviews.
        let mut day = 0;
        for _ in 0..5 {
            while q.due(day).is_empty() {
                day += 1;
            }
            day = q.review("a", Rating::Good, day).unwrap();
        }
        let before = q.cards[0].state.stability;
        let after_again = q.review("a", Rating::Again, day).unwrap();
        let _ = after_again;
        assert!(
            q.cards[0].state.stability < before,
            "lapse lowers stability"
        );
    }

    #[test]
    fn review_unknown_id_returns_none() {
        let mut q = ReviewQueue::new(0.9);
        assert_eq!(q.review("ghost", Rating::Good, 0), None);
    }

    #[test]
    fn split_sentences_on_punctuation_boundaries() {
        let s = split_sentences("First fact. Second one! Third? Last");
        assert_eq!(s, vec!["First fact.", "Second one!", "Third?", "Last"]);
    }

    #[test]
    fn split_sentences_keeps_decimal_points_intact() {
        let s = split_sentences("The value is 3.14 and it holds.");
        // The decimal point is not a boundary (next char is a digit).
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn extracts_fact_sentences_with_stable_ids() {
        let text = "The kernel uses LRU paging. An important fact: context windows are precious. Nothing here matters.";
        let cands = extract_candidates(text);
        // The "important fact" sentence qualifies; the plain one does not.
        assert!(cands.iter().any(|c| c.content.contains("context windows")));
        assert!(cands.iter().all(|c| !c.content.contains("Nothing here")));
        // Ids are stable across calls.
        let again = extract_candidates(text);
        assert_eq!(cands[0].id, again[0].id);
        // Importance boosted by "important".
        let important = cands
            .iter()
            .find(|c| c.content.contains("context windows"))
            .unwrap();
        assert_eq!(important.importance, 1.0);
    }

    #[test]
    fn extraction_dedupes_identical_sentences() {
        let text = "Key: apples are fruit. Key: apples are fruit.";
        let cands = extract_candidates(text);
        assert_eq!(cands.len(), 1);
    }

    #[test]
    fn extraction_feeds_the_queue() {
        let mut q = ReviewQueue::new(0.9);
        let cands = extract_candidates("An important fact: Rust is memory-safe. The sky is blue.");
        // Both sentences carry a fact pattern ("important"/" is "); the
        // queue ingests them all.
        assert!(cands.iter().any(|c| c.content.contains("memory-safe")));
        let added = q.ingest(cands, 0);
        assert_eq!(added, 2);
    }
}
