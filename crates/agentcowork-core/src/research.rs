//! H31 corpus-first research surface (doc 68 §2.2 — Gemini-Notebook-class):
//! pick sources (files / folders / URLs / emails) → grounded, cited answers
//! + report artifacts, reusing the C-series RAG + G2 deep research + EV1
//!   citation fidelity. The **audio-digest output** (podcast-style Audio
//!   Overview) rides H28 TTS — recorded here as the composition seam, not
//!   faked.
//!
//! This module owns the *research contract*: source selection, the
//! grounded answer with per-claim citations, and the deterministic
//! citation-fidelity check (every claim must cite a source span).

use serde::{Deserialize, Serialize};

/// A selected source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchSource {
    pub id: String,
    pub kind: String, // file | folder | url | email
    pub location: String,
}

/// One citation: the claim tied to a source span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub source_id: String,
    /// The source span the claim is grounded in (file/line, URL section).
    pub span: String,
    /// The supporting quote (verbatim).
    pub quote: String,
}

/// The grounded answer: claims + their citations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundedAnswer {
    pub question: String,
    pub claims: Vec<GroundedClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundedClaim {
    pub claim: String,
    pub citations: Vec<Citation>,
}

/// The research session state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchSession {
    pub sources: Vec<ResearchSource>,
}

impl ResearchSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_source(&mut self, source: ResearchSource) {
        self.sources.push(source);
    }

    pub fn source_ids(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.id.as_str()).collect()
    }
}

/// EV1-style citation fidelity: a claim is **grounded** when every citation
/// references a selected source AND the quote is actually present in the
/// source's content. A claim with zero citations is never counted as
/// grounded (the "right number without evidence" chain fails here too).
pub fn citation_fidelity(
    answer: &GroundedAnswer,
    sources: &[ResearchSource],
    contents: &[(String, String)],
) -> CitationScore {
    let mut grounded = 0;
    let mut total = 0;
    for claim in &answer.claims {
        total += 1;
        let ok = !claim.citations.is_empty()
            && claim.citations.iter().all(|c| {
                sources.iter().any(|s| s.id == c.source_id)
                    && contents
                        .iter()
                        .any(|(id, text)| id == &c.source_id && text.contains(&c.quote))
            });
        if ok {
            grounded += 1;
        }
    }
    CitationScore {
        grounded_claims: grounded,
        total_claims: total,
        rate: if total == 0 {
            0.0
        } else {
            grounded as f64 / total as f64
        },
    }
}

/// The fidelity score (EV1 citation-fidelity metric).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CitationScore {
    pub grounded_claims: usize,
    pub total_claims: usize,
    /// 0..=1 — the fraction of claims with verifiable citations.
    pub rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_tracks_sources() {
        let mut s = ResearchSession::new();
        s.add_source(ResearchSource {
            id: "f1".into(),
            kind: "file".into(),
            location: "notes.md".into(),
        });
        assert_eq!(s.source_ids(), vec!["f1"]);
    }

    #[test]
    fn fidelity_requires_real_citations() {
        let sources = vec![ResearchSource {
            id: "f1".into(),
            kind: "file".into(),
            location: "notes.md".into(),
        }];
        let contents = vec![("f1".to_string(), "The parser is fast.".to_string())];
        let answer = GroundedAnswer {
            question: "q".into(),
            claims: vec![
                GroundedClaim {
                    claim: "The parser is fast".into(),
                    citations: vec![Citation {
                        source_id: "f1".into(),
                        span: "notes.md:1".into(),
                        quote: "The parser is fast.".into(),
                    }],
                },
                // Uncited claim — never counts as grounded.
                GroundedClaim {
                    claim: "It also flies".into(),
                    citations: vec![],
                },
                // Quote not in the source — not grounded.
                GroundedClaim {
                    claim: "x".into(),
                    citations: vec![Citation {
                        source_id: "f1".into(),
                        span: "notes.md:2".into(),
                        quote: "no such line".into(),
                    }],
                },
            ],
        };
        let score = citation_fidelity(&answer, &sources, &contents);
        assert_eq!(score.grounded_claims, 1);
        assert_eq!(score.total_claims, 3);
        assert!((score.rate - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn citation_to_unknown_source_fails() {
        let answer = GroundedAnswer {
            question: "q".into(),
            claims: vec![GroundedClaim {
                claim: "c".into(),
                citations: vec![Citation {
                    source_id: "ghost".into(),
                    span: "x".into(),
                    quote: "y".into(),
                }],
            }],
        };
        let score = citation_fidelity(&answer, &[], &[]);
        assert_eq!(score.rate, 0.0);
    }
}
