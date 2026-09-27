//! P5 learning-journey timeline (doc 69 §3 — `hermes journey` steal): a
//! chronological timeline of what the agent learned — skills grown from
//! tasks, memories added, reinforcements scheduled and reviewed. This is the
//! visualization data for the learning-journey surface (validates the
//! reinforce-queue story: skills accumulate, reviews land at the right
//! intervals, nothing is assumed).

use crate::reinforce::ReviewCard;
use serde::{Deserialize, Serialize};

/// One event on the journey timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JourneyEvent {
    /// Monotonic day counter (the same `day` the reinforce queue uses).
    pub day: u32,
    pub kind: JourneyKind,
    /// The subject: skill name, memory id, or review card id.
    pub subject: String,
    /// Optional detail (version, importance, due day).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The event taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JourneyKind {
    /// A skill was grown from a solved task (versioned).
    SkillLearned,
    /// A memory was added to the store.
    MemoryAdded,
    /// A reinforcement card was scheduled.
    ReviewScheduled,
    /// A reinforcement card was reviewed (with its new due day).
    ReviewCompleted,
}

/// The journey: an append-only event log + a rendered timeline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Journey {
    pub events: Vec<JourneyEvent>,
}

impl Journey {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, ev: JourneyEvent) {
        self.events.push(ev);
    }

    /// Record a skill growth (the blueprint `grow_from_task` seam).
    pub fn skill_learned(&mut self, day: u32, skill_name: &str, version: &str) {
        self.record(JourneyEvent {
            day,
            kind: JourneyKind::SkillLearned,
            subject: skill_name.into(),
            detail: Some(format!("v{version}")),
        });
    }

    /// Record a memory addition.
    pub fn memory_added(&mut self, day: u32, memory_id: &str) {
        self.record(JourneyEvent {
            day,
            kind: JourneyKind::MemoryAdded,
            subject: memory_id.into(),
            detail: None,
        });
    }

    /// Record a scheduled reinforcement card.
    pub fn review_scheduled(&mut self, day: u32, card: &ReviewCard) {
        self.record(JourneyEvent {
            day,
            kind: JourneyKind::ReviewScheduled,
            subject: card.id.clone(),
            detail: Some(format!("due day {}", card.due_day)),
        });
    }

    /// Record a completed review (with the new due day from the FSRS state).
    pub fn review_completed(&mut self, day: u32, card_id: &str, next_due: u32) {
        self.record(JourneyEvent {
            day,
            kind: JourneyKind::ReviewCompleted,
            subject: card_id.into(),
            detail: Some(format!("next due day {next_due}")),
        });
    }

    /// Events on `day`, in recording order.
    pub fn on(&self, day: u32) -> Vec<&JourneyEvent> {
        self.events.iter().filter(|e| e.day == day).collect()
    }

    /// Count of each kind (the summary numbers the timeline header shows).
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut s = 0;
        let mut m = 0;
        let mut rs = 0;
        let mut rc = 0;
        for e in &self.events {
            match e.kind {
                JourneyKind::SkillLearned => s += 1,
                JourneyKind::MemoryAdded => m += 1,
                JourneyKind::ReviewScheduled => rs += 1,
                JourneyKind::ReviewCompleted => rc += 1,
            }
        }
        (s, m, rs, rc)
    }

    /// The rendered timeline (the journey surface's text form).
    pub fn render(&self) -> String {
        let (s, m, rs, rc) = self.counts();
        let mut out = format!(
            "# Learning journey — {s} skills, {m} memories, {rs} scheduled, {rc} reviewed\n\n"
        );
        for e in &self.events {
            let label = match e.kind {
                JourneyKind::SkillLearned => "skill",
                JourneyKind::MemoryAdded => "memory",
                JourneyKind::ReviewScheduled => "scheduled",
                JourneyKind::ReviewCompleted => "reviewed",
            };
            let detail = e.detail.as_deref().unwrap_or("");
            out.push_str(&format!(
                "day {:>4}  {:<9} {} {}\n",
                e.day, label, e.subject, detail
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsrs::MemoryState;
    use crate::reinforce::ReviewCard;

    fn card(id: &str, due: u32) -> ReviewCard {
        ReviewCard {
            id: id.into(),
            content: "x".into(),
            importance: 1.0,
            state: MemoryState::default(),
            due_day: due,
            last_review_day: -1,
        }
    }

    #[test]
    fn timeline_records_and_counts() {
        let mut j = Journey::new();
        j.skill_learned(1, "refactor-helper", "1.0.0");
        j.memory_added(1, "m1");
        j.review_scheduled(1, &card("c1", 4));
        j.review_completed(4, "c1", 9);
        assert_eq!(j.on(1).len(), 3);
        let (s, m, rs, rc) = j.counts();
        assert_eq!((s, m, rs, rc), (1, 1, 1, 1));
        let rendered = j.render();
        assert!(rendered.contains("1 skills, 1 memories"));
        assert!(rendered.contains("day    4  reviewed  c1 next due day 9"));
    }

    #[test]
    fn render_is_deterministic() {
        let mut j = Journey::new();
        j.skill_learned(1, "s", "1");
        assert_eq!(j.render(), j.render());
    }
}
