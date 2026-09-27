//! H30 voice-memo → structured report (doc 68 §3): the end-to-end
//! "reports from messy inputs" workflow — a voice memo (transcribed via
//! H15 STT, or pasted text) becomes a polished document (Word block-patch
//! D1 / markdown / email F14). The I/O rides H15/H28 (STT/TTS); this module
//! owns the *job* that composes them: transcript → structured sections →
//! per-format document shell.
//!
//! Deterministic: the same transcript + style yields the same report.

use serde::{Deserialize, Serialize};

/// The input memo — audio ref (H15 STT) or pasted text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoInput {
    /// The audio handle when the memo came from voice ("" for text).
    #[serde(default)]
    pub audio_ref: String,
    /// The transcript (from STT, or the pasted text itself).
    pub transcript: String,
}

/// The report format (rides the D1 docx engine / F14 email).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportFormat {
    Word,
    Markdown,
    Email,
}

/// One structured section extracted from the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSection {
    pub heading: String,
    pub body: String,
}

/// The compiled report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredReport {
    pub title: String,
    pub format: ReportFormat,
    pub sections: Vec<ReportSection>,
    /// Whether the memo actually had content (honest gap report).
    pub had_content: bool,
}

/// The pipeline: transcript → sections → document shell. Deterministic
/// sectioning — each paragraph becomes a section under a derived heading
/// (the first non-empty line of the paragraph, title-cased).
pub fn compile_report(memo: &MemoInput, format: ReportFormat) -> StructuredReport {
    let paragraphs: Vec<&str> = memo
        .transcript
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if paragraphs.is_empty() {
        return StructuredReport {
            title: "Voice memo".into(),
            format,
            sections: Vec::new(),
            had_content: false,
        };
    }
    let title = derive_title(paragraphs[0]);
    let mut sections = Vec::new();
    for (i, p) in paragraphs.iter().enumerate() {
        if i == 0 {
            continue; // the first paragraph becomes the title
        }
        sections.push(ReportSection {
            heading: derive_heading(p, i),
            body: p.to_string(),
        });
    }
    StructuredReport {
        title,
        format,
        sections,
        had_content: true,
    }
}

/// Render the report in its format's shell (markdown/email are text;
/// Word rides the D1 block-patch engine — the shell here is the markdown
/// source the D1 path converts).
pub fn render(report: &StructuredReport) -> String {
    let mut out = match report.format {
        ReportFormat::Markdown | ReportFormat::Word => format!("# {}\n\n", report.title),
        ReportFormat::Email => format!("Subject: {}\n\n", report.title),
    };
    for s in &report.sections {
        out.push_str(&format!("## {}\n{}\n\n", s.heading, s.body));
    }
    out
}

fn derive_title(first: &str) -> String {
    let first_line = first.lines().next().unwrap_or(first).trim();
    let t = first_line.trim_matches(|c| c == '.' || c == '!' || c == '?');
    title_case(t)
}

fn derive_heading(paragraph: &str, idx: usize) -> String {
    let first_line = paragraph.lines().next().unwrap_or(paragraph).trim();
    let words: Vec<&str> = first_line.split_whitespace().take(6).collect();
    if words.is_empty() {
        return format!("Section {}", idx + 1);
    }
    title_case(&words.join(" "))
}

fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c.is_whitespace() {
            out.push(' ');
            upper = true;
        } else if upper && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
            upper = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_memo_reports_gap() {
        let r = compile_report(
            &MemoInput {
                audio_ref: "a.wav".into(),
                transcript: "".into(),
            },
            ReportFormat::Markdown,
        );
        assert!(!r.had_content);
        assert!(r.sections.is_empty());
    }

    #[test]
    fn transcript_becomes_structured_report() {
        let memo = MemoInput {
            audio_ref: "memo1.wav".into(),
            transcript:
                "Weekly status update.\n\nShipped the parser fix.\n\nNext: wire the connector."
                    .into(),
        };
        let r = compile_report(&memo, ReportFormat::Markdown);
        assert!(r.had_content);
        assert_eq!(r.title, "Weekly Status Update");
        assert_eq!(r.sections.len(), 2);
        let md = render(&r);
        assert!(md.contains("## Shipped The Parser Fix"));
        assert!(md.contains("## Next: Wire The Connector"));
    }

    #[test]
    fn email_format_uses_subject() {
        let memo = MemoInput {
            audio_ref: String::new(),
            transcript: "Quick note.\n\nAll good.".into(),
        };
        let r = compile_report(&memo, ReportFormat::Email);
        assert!(render(&r).starts_with("Subject: Quick Note"));
    }

    #[test]
    fn deterministic() {
        let memo = MemoInput {
            audio_ref: "x".into(),
            transcript: "One.\n\nTwo.".into(),
        };
        let a = compile_report(&memo, ReportFormat::Markdown);
        let b = compile_report(&memo, ReportFormat::Markdown);
        assert_eq!(a, b);
    }
}
