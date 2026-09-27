//! P26-3 — the Jobs vertical (doc 78 §3 — AIHawk pattern, but composed of
//! our own engines: career-ops rubric scoring + the Office engine for the
//! tailored CV/cover letter + **approve-before-send on every submission**).
//!
//! This is the pattern adoption the research doc prescribes: AIHawk's
//! loop (scan → tailor → apply) mapped onto what we already built — no
//! Python harness, no stealth automation. Every submission is a Guard-2
//! ticket bound to the exact posting URL + tailored payload. The engine
//! itself never submits: it produces the [`ApplicationPackage`] and
//! requires the caller's approval decision (from the card flow).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPosting {
    pub id: String,
    pub url: String,
    pub title: String,
    pub company: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RubricGrade {
    A,
    B,
    C,
    D,
    E,
    F,
}
pub const PASS_BAR: f32 = 3.5;
impl RubricGrade {
    pub fn score(self) -> f32 {
        match self {
            Self::A => 5.0,
            Self::B => 4.0,
            Self::C => 3.0,
            Self::D => 2.0,
            Self::E => 1.0,
            Self::F => 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RubricJudge {
    pub signals: Vec<(String, f32)>,
}
impl Default for RubricJudge {
    fn default() -> Self {
        Self {
            signals: vec![
                ("typescript".into(), 1.0),
                ("rust".into(), 1.0),
                ("react".into(), 0.8),
                ("python".into(), 0.8),
                ("sql".into(), 0.6),
                ("aws".into(), 0.6),
                ("docker".into(), 0.5),
                ("kubernetes".into(), 0.5),
                ("5+ years".into(), 0.4),
                ("remote".into(), 0.3),
            ],
        }
    }
}
impl RubricJudge {
    pub fn score(&self, body: &str) -> (RubricGrade, f32, Vec<String>) {
        let lower = body.to_lowercase();
        let mut hit = Vec::new();
        let mut score = 0.0;
        for (signal, weight) in &self.signals {
            if lower.contains(signal) {
                score += weight;
                hit.push(signal.clone());
            }
        }
        let grade = match score {
            x if x >= 4.0 => RubricGrade::A,
            x if x >= 3.0 => RubricGrade::B,
            x if x >= 2.0 => RubricGrade::C,
            x if x >= 1.0 => RubricGrade::D,
            x if x > 0.0 => RubricGrade::E,
            _ => RubricGrade::F,
        };
        (grade, score, hit)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationPackage {
    pub posting: JobPosting,
    pub score: RubricGrade,
    pub cv_bytes: Vec<u8>,
    pub cover_letter_bytes: Vec<u8>,
    pub payload_hash: String,
    pub approved: bool,
}
impl ApplicationPackage {
    pub fn signature(&self) -> String {
        signature_for(
            &self.posting,
            self.score,
            &self.cv_bytes,
            &self.cover_letter_bytes,
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct SubmissionLatch {
    approved_hash: Option<String>,
    used: bool,
}
impl SubmissionLatch {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn approve(&mut self, p: &ApplicationPackage) {
        self.approved_hash = Some(simple_hash(&p.signature()));
        self.used = false;
    }
    pub fn present(&mut self, p: &ApplicationPackage) -> Result<(), JobsError> {
        match &self.approved_hash {
            Some(h) if h == &simple_hash(&p.signature()) && !self.used => {
                self.used = true;
                Ok(())
            }
            Some(_) => Err(JobsError::PayloadMismatch),
            None => Err(JobsError::NotApproved),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct JobsPipeline {
    latch: SubmissionLatch,
    processed: usize,
}
impl JobsPipeline {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn processed(&self) -> usize {
        self.processed
    }
    pub fn scan_and_tailor(
        &self,
        judge: &RubricJudge,
        posting: &JobPosting,
        tailor_into: impl Fn(&JobPosting) -> (Vec<u8>, Vec<u8>),
    ) -> (
        Option<ApplicationPackage>,
        Option<(JobPosting, RubricGrade, f32)>,
    ) {
        let (grade, score, _) = judge.score(&posting.body);
        if score < PASS_BAR {
            return (None, Some((posting.clone(), grade, score)));
        }
        let (cv, letter) = tailor_into(posting);
        let hash = simple_hash(&signature_for(posting, grade, &cv, &letter));
        (
            Some(ApplicationPackage {
                posting: posting.clone(),
                score: grade,
                cv_bytes: cv,
                cover_letter_bytes: letter,
                payload_hash: hash,
                approved: false,
            }),
            None,
        )
    }
    pub fn approve(&mut self, p: &ApplicationPackage) {
        self.latch.approve(p)
    }
    pub fn approve_and_submit(&mut self, p: &ApplicationPackage) -> Result<(), JobsError> {
        let r = self.latch.present(p);
        if r.is_ok() {
            self.processed += 1;
        }
        r
    }
    pub fn submit_if_approved(&mut self, p: &ApplicationPackage) -> Result<(), JobsError> {
        let r = self.latch.present(p);
        if r.is_ok() {
            self.processed += 1;
        }
        r
    }
}
fn signature_for(p: &JobPosting, g: RubricGrade, cv: &[u8], letter: &[u8]) -> String {
    fn digest(b: &[u8]) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut h);
        format!("{:016x}", h.finish())
    }
    format!("{}:{:?}:{}:{}", p.url, g, digest(cv), digest(letter))
}
fn simple_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobsError {
    NotApproved,
    PayloadMismatch,
}
impl std::fmt::Display for JobsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotApproved => write!(f, "submission not approved"),
            Self::PayloadMismatch => write!(f, "payload changed after approval"),
        }
    }
}
impl std::error::Error for JobsError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn posting(id: &str, body: &str) -> JobPosting {
        JobPosting {
            id: id.into(),
            url: format!("https://ex/{id}"),
            title: "t".into(),
            company: "c".into(),
            body: body.into(),
        }
    }
    #[test]
    fn rubric_scores_below_bar_stay_unsent() {
        let j = RubricJudge::default();
        let p = posting("1", "hello world");
        let (q, b) = JobsPipeline::new().scan_and_tailor(&j, &p, |_| (vec![], vec![]));
        assert!(q.is_none());
        assert!(b.is_some());
    }
    #[test]
    fn high_score_tailors_and_needs_approval() {
        let j = RubricJudge::default();
        let p = posting(
            "2",
            "Rust + TypeScript + AWS + Docker + Kubernetes + 5+ years, remote",
        );
        let mut x = JobsPipeline::new();
        let (q, b) = x.scan_and_tailor(&j, &p, |_| (b"CV".to_vec(), b"LETTER".to_vec()));
        assert!(b.is_none());
        let q = q.unwrap();
        assert!(q.score.score() >= PASS_BAR);
        assert_eq!(x.approve_and_submit(&q), Err(JobsError::NotApproved));
    }
    #[test]
    fn approved_exactly_once_per_package() {
        let j = RubricJudge::default();
        let p = posting("3", "Rust + React + SQL + Docker + Kubernetes + remote");
        let mut x = JobsPipeline::new();
        let (q, _) = x.scan_and_tailor(&j, &p, |_| (b"CV".to_vec(), b"L".to_vec()));
        let q = q.unwrap();
        x.approve(&q);
        assert!(x.approve_and_submit(&q).is_ok());
        assert_eq!(x.approve_and_submit(&q), Err(JobsError::PayloadMismatch));
        assert_eq!(x.processed(), 1);
    }
    #[test]
    fn changed_payload_after_approval_is_refused() {
        let j = RubricJudge::default();
        let p = posting("4", "Rust + Python + AWS + Docker + Kubernetes + remote");
        let mut x = JobsPipeline::new();
        let (q, _) = x.scan_and_tailor(&j, &p, |_| (b"CV".to_vec(), b"L".to_vec()));
        let mut q = q.unwrap();
        x.approve(&q);
        q.cv_bytes = b"DIFFERENT".to_vec();
        assert_eq!(x.submit_if_approved(&q), Err(JobsError::PayloadMismatch));
    }
}
