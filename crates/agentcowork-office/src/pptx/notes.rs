//! Presenter mode + `SPEAKER_NOTES` contract (doc 63 §4.9 — guizang
//! `presenter-mode.md` pattern). Extracts speaker notes from a notes slide
//! (`ppt/notesSlides/notesSlideN.xml`) and assembles the `data-slide-id`-keyed
//! `SPEAKER_NOTES` array the presenter view consumes.

use roxmltree::Document;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// DrawingML namespace (`a:t` text runs).
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// One `SPEAKER_NOTES` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerNotesEntry {
    /// `data-slide-id` (the slide's rel id or numeric id).
    pub slide_id: String,
    pub title: String,
    pub section: Option<String>,
    pub minutes: Option<u32>,
    pub purpose: Option<String>,
    /// The talk/notes text.
    pub talk: String,
    pub timing: Option<String>,
    pub transition: Option<String>,
}

#[derive(Debug, Error)]
pub enum NotesError {
    #[error("xml parse error: {0}")]
    Parse(#[from] roxmltree::Error),
}

/// Extract the speaker-notes text from a notes-slide part (concatenated `a:t`
/// runs). Empty when the slide has no notes.
pub fn extract_notes_text(notes_slide_xml: &str) -> Result<String, NotesError> {
    let doc = Document::parse(notes_slide_xml)?;
    let mut parts: Vec<&str> = Vec::new();
    for node in doc.descendants() {
        if node.is_element()
            && node.tag_name().namespace() == Some(A)
            && node.tag_name().name() == "t"
        {
            if let Some(t) = node.text() {
                parts.push(t);
            }
        }
    }
    Ok(parts.join(""))
}

// ---------------------------------------------------------------------------
// Presenter-mode validation + rehearsal timing (the "validate-notes↔slides
// sync script" half — doc 63 §4.9)
// ---------------------------------------------------------------------------

/// Validate that the speaker-notes array stays in sync with the deck's slide
/// ids: every slide must have a notes entry and every notes entry must map to
/// a real slide. Returns the list of problems (empty = synced).
pub fn validate_slides_notes_sync(
    slide_ids: &[String],
    notes: &[SpeakerNotesEntry],
) -> Vec<String> {
    let mut problems = Vec::new();
    let notes_ids: std::collections::HashSet<&str> =
        notes.iter().map(|n| n.slide_id.as_str()).collect();
    for id in slide_ids {
        if !notes_ids.contains(id.as_str()) {
            problems.push(format!("slide {id} has no speaker notes entry"));
        }
    }
    for entry in notes {
        if !slide_ids.iter().any(|s| s == &entry.slide_id) {
            problems.push(format!(
                "notes entry for {slide_id} does not match any slide",
                slide_id = entry.slide_id
            ));
        }
        if let Some(minutes) = entry.minutes {
            if entry.talk.is_empty() && minutes > 0 {
                problems.push(format!(
                    "notes entry for {slide_id} has minutes but no talk text",
                    slide_id = entry.slide_id
                ));
            }
        }
    }
    problems
}

/// A rehearsal timing plan: per-slide minutes + the deck total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RehearsalTiming {
    /// Per-slide estimated minutes (parallel to the notes entries).
    pub per_slide_minutes: Vec<u32>,
    pub total_minutes: u32,
    pub total_seconds: u32,
}

/// Estimate rehearsal time from the notes: `words_per_minute` speaking rate
/// (default ~150 wpm) over the `talk` text, floored at 1 minute per slide with
/// notes. Deterministic — the rehearsal view's auto-advance clock.
pub fn plan_rehearsal(notes: &[SpeakerNotesEntry], words_per_minute: u32) -> RehearsalTiming {
    let wpm = words_per_minute.max(1);
    let mut per_slide_minutes = Vec::with_capacity(notes.len());
    let mut total_minutes = 0u32;
    for entry in notes {
        let words = entry.talk.split_whitespace().count() as u32;
        let minutes = if words == 0 {
            0
        } else {
            words.div_ceil(wpm).max(1)
        };
        per_slide_minutes.push(minutes);
        total_minutes += minutes;
    }
    let total_seconds = total_minutes * 60;
    RehearsalTiming {
        per_slide_minutes,
        total_minutes,
        total_seconds,
    }
}

/// Assemble the `SPEAKER_NOTES` array from (slide_id, title, notes_text)
/// triples. `talk` carries the notes; the richer fields are filled by the
/// caller's analysis (section/purpose/timing are optional).
pub fn build_speaker_notes(slides: &[(String, String, String)]) -> Vec<SpeakerNotesEntry> {
    slides
        .iter()
        .map(|(id, title, notes)| SpeakerNotesEntry {
            slide_id: id.clone(),
            title: title.clone(),
            section: None,
            minutes: None,
            purpose: None,
            talk: notes.clone(),
            timing: None,
            transition: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_notes_text() {
        let xml = r#"<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Mention the Q3 budget</a:t></a:r><a:r><a:t> update.</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#;
        let text = extract_notes_text(xml).unwrap();
        assert_eq!(text, "Mention the Q3 budget update.");
    }

    #[test]
    fn empty_notes_returns_empty_string() {
        let xml = r#"<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld/></p:notes>"#;
        assert_eq!(extract_notes_text(xml).unwrap(), "");
    }

    #[test]
    fn builds_speaker_notes_array() {
        let slides = vec![
            ("sld1".into(), "Intro".into(), "Welcome".into()),
            ("sld2".into(), "Budget".into(), "Show the chart".into()),
        ];
        let notes = build_speaker_notes(&slides);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].slide_id, "sld1");
        assert_eq!(notes[0].talk, "Welcome");
        assert_eq!(notes[1].title, "Budget");
    }

    #[test]
    fn speaker_notes_serializes() {
        let notes = build_speaker_notes(&[("s1".into(), "T".into(), "N".into())]);
        let json = serde_json::to_string(&notes).unwrap();
        assert!(json.contains("slide_id"));
    }

    #[test]
    fn sync_validation_flags_missing_and_orphan_entries() {
        // Missing: s2 has no notes. Orphan: the "s9" notes entry matches no
        // slide.
        let slides = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        let mut notes = build_speaker_notes(&[
            ("s1".into(), "Intro".into(), "Welcome".into()),
            ("s3".into(), "Close".into(), "Thanks".into()),
        ]);
        notes.push(SpeakerNotesEntry {
            slide_id: "s9".into(),
            title: "Orphan".into(),
            section: None,
            minutes: None,
            purpose: None,
            talk: "?".into(),
            timing: None,
            transition: None,
        });
        let problems = validate_slides_notes_sync(&slides, &notes);
        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("s2 has no speaker notes"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("s9") && p.contains("does not match"))
        );
    }

    #[test]
    fn sync_validation_passes_when_aligned() {
        let slides = vec!["s1".to_string()];
        let notes = build_speaker_notes(&[("s1".into(), "T".into(), "N".into())]);
        assert!(validate_slides_notes_sync(&slides, &notes).is_empty());
    }

    #[test]
    fn sync_validation_flags_notes_without_talk() {
        let slides = vec!["s1".to_string()];
        let mut notes = build_speaker_notes(&[("s1".into(), "T".into(), "".into())]);
        notes[0].minutes = Some(2);
        let problems = validate_slides_notes_sync(&slides, &notes);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("minutes but no talk text"))
        );
    }

    #[test]
    fn rehearsal_timing_from_word_count() {
        let notes = build_speaker_notes(&[
            ("s1".into(), "Intro".into(), "Welcome to the review.".into()),
            (
                "s2".into(),
                "Deep".into(),
                "This section has exactly ten words here for timing.".into(),
            ),
        ]);
        let plan = plan_rehearsal(&notes, 10); // 10 wpm for easy math
        assert_eq!(plan.per_slide_minutes, vec![1, 1]); // 5 words→1, 10 words→1
        assert_eq!(plan.total_minutes, 2);
        assert_eq!(plan.total_seconds, 120);
    }

    #[test]
    fn rehearsal_timing_scales_with_rate() {
        let notes = build_speaker_notes(&[("s1".into(), "T".into(), "a b c d e".into())]);
        let fast = plan_rehearsal(&notes, 5); // 5 words → 1 minute
        let slow = plan_rehearsal(&notes, 1); // 5 words → 5 minutes
        assert_eq!(fast.per_slide_minutes, vec![1]);
        assert_eq!(slow.per_slide_minutes, vec![5]);
    }

    #[test]
    fn empty_notes_plan_is_zero() {
        let plan = plan_rehearsal(&[], 150);
        assert_eq!(plan.total_minutes, 0);
        assert_eq!(plan.total_seconds, 0);
    }
}
