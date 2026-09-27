//! Vision fallback (JeenyJAI / AtomicBot pattern): when the a11y tree is
//! empty (canvas / games / custom controls), OCR the window into word boxes
//! and click by screenshot coordinates.
//!
//! Backend: `tesseract` CLI with TSV output (`--psm 11` sparse text), which
//! emits per-word bounding boxes + confidence. The seam keeps the engine
//! replaceable (rust ocrs / Windows OCR / on-device models).

use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::types::{OcrWord, Region};

/// Anything that turns image bytes into word boxes.
pub trait OcrEngine: Send + Sync {
    fn ocr(&self, image_png: &[u8]) -> Vec<OcrWord>;
}

/// Honest no-OCR engine — fails closed with an empty result.
pub struct NoOcr;
impl OcrEngine for NoOcr {
    fn ocr(&self, _image_png: &[u8]) -> Vec<OcrWord> {
        vec![]
    }
}

/// Tesseract via the CLI: `tesseract stdin stdout --psm 11 tsv`.
/// The TSV output has columns: level page block par line word left top width
/// height conf text — we keep `word` rows with confidence >= `min_conf`.
pub struct TesseractCli {
    pub min_conf: f64,
    pub binary: String,
}

impl Default for TesseractCli {
    fn default() -> Self {
        Self {
            min_conf: 40.0,
            binary: "tesseract".into(),
        }
    }
}

impl TesseractCli {
    pub fn available(&self) -> bool {
        Command::new(&self.binary)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn parse_tsv(tsv: &str) -> Vec<OcrWord> {
        let mut words = Vec::new();
        let mut lines = tsv.lines();
        let _header = lines.next(); // "level page block par line word left top width height conf text"
        for line in lines {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 12 {
                continue;
            }
            if cols[0] != "5" {
                continue; // word rows only
            }
            let Ok(left) = cols[6].parse::<i32>() else {
                continue;
            };
            let Ok(top) = cols[7].parse::<i32>() else {
                continue;
            };
            let Ok(width) = cols[8].parse::<u32>() else {
                continue;
            };
            let Ok(height) = cols[9].parse::<u32>() else {
                continue;
            };
            let Ok(conf) = cols[10].parse::<f64>() else {
                continue;
            };
            let text = cols[11].trim().to_string();
            if text.is_empty() {
                continue;
            }
            words.push(OcrWord {
                text,
                confidence: conf,
                x: left,
                y: top,
                width,
                height,
            });
        }
        words
    }
}

impl OcrEngine for TesseractCli {
    fn ocr(&self, image_png: &[u8]) -> Vec<OcrWord> {
        let Ok(mut child) = Command::new(&self.binary)
            .args(["stdin", "stdout", "--psm", "11", "tsv"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return vec![];
        };
        if child
            .stdin
            .as_mut()
            .map(|s| s.write_all(image_png))
            .is_none()
        {
            return vec![];
        }
        drop(child.stdin.take());
        let Ok(out) = child.wait_with_output() else {
            return vec![];
        };
        if !out.status.success() {
            return vec![];
        }
        let tsv = String::from_utf8_lossy(&out.stdout);
        Self::parse_tsv(&tsv)
            .into_iter()
            .filter(|w| w.confidence >= self.min_conf)
            .collect()
    }
}

/// The vision-fallback locator math: given OCR words, find the click point
/// for a target phrase (exact word or phrase).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VisionHit {
    /// Click at this (window) point.
    Point { x: i32, y: i32 },
    /// The phrase spans multiple words; use the union region center.
    RegionCenter {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    /// Not found — the caller must NOT guess (halt).
    NotFound,
}

/// Find a target phrase in OCR words (greedy: prefer an exact single word,
/// else a contiguous run of words whose texts concatenate to the phrase).
pub fn locate_phrase(words: &[OcrWord], phrase: &str) -> VisionHit {
    let needle = phrase.trim();
    if needle.is_empty() {
        return VisionHit::NotFound;
    }
    let lower = needle.to_ascii_lowercase();
    // Exact single-word match (case-insensitive).
    if let Some(w) = words.iter().find(|w| w.text.to_ascii_lowercase() == lower) {
        let (cx, cy) = w.center();
        return VisionHit::Point { x: cx, y: cy };
    }
    // Contains match on a single word (e.g. "Submit" vs "Submit now" word).
    if let Some(w) = words
        .iter()
        .find(|w| w.text.to_ascii_lowercase().contains(&lower))
    {
        let (cx, cy) = w.center();
        return VisionHit::Point { x: cx, y: cy };
    }
    // Phrase across a contiguous run of words (same line, close together).
    let mut sorted: Vec<&OcrWord> = words.iter().collect();
    sorted.sort_by_key(|w| (w.y / 20, w.x)); // row bucket, then x
    for i in 0..sorted.len() {
        let mut joined = String::new();
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = 0i32;
        let mut max_y = 0i32;
        for w in &sorted[i..] {
            if joined.len() > lower.len() + 8 {
                break;
            }
            if !joined.is_empty() {
                joined.push(' ');
            }
            joined.push_str(&w.text.to_ascii_lowercase());
            min_x = min_x.min(w.x);
            min_y = min_y.min(w.y);
            max_x = max_x.max(w.x + w.width as i32);
            max_y = max_y.max(w.y + w.height as i32);
            if joined == lower {
                return VisionHit::RegionCenter {
                    x: min_x,
                    y: min_y,
                    width: (max_x - min_x) as u32,
                    height: (max_y - min_y) as u32,
                };
            }
        }
    }
    VisionHit::NotFound
}

/// Confidence-weighted union of a region — used for "click the button that
/// says X" when X spans multiple OCR words.
pub fn union_region(words: &[OcrWord]) -> Option<Region> {
    let mut it = words.iter();
    let first = it.next()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.width as i32;
    let mut max_y = first.y + first.height as i32;
    for w in it {
        min_x = min_x.min(w.x);
        min_y = min_y.min(w.y);
        max_x = max_x.max(w.x + w.width as i32);
        max_y = max_y.max(w.y + w.height as i32);
    }
    Some(Region {
        x: min_x,
        y: min_y,
        width: (max_x - min_x) as u32,
        height: (max_y - min_y) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, x: i32, y: i32) -> OcrWord {
        OcrWord {
            text: text.into(),
            confidence: 90.0,
            x,
            y,
            width: 40,
            height: 12,
        }
    }

    #[test]
    fn tsv_parser_reads_word_rows_only() {
        let tsv = "level\tpage\tblock\tpar\tline\tword\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t100\t100\t-1\t\n\
                   5\t1\t0\t0\t0\t0\t10\t20\t30\t10\t92\tHello\n\
                   5\t1\t0\t0\t0\t1\t50\t20\t30\t10\t-1\tignored\n";
        let words = TesseractCli::parse_tsv(tsv);
        // Both word rows parse (confidence filtering happens in OcrEngine::ocr).
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[1].text, "ignored");
        assert_eq!(
            (words[0].x, words[0].y, words[0].width, words[0].height),
            (10, 20, 30, 10)
        );
    }

    #[test]
    fn single_word_click_point() {
        let words = vec![w("Submit", 100, 100)];
        assert_eq!(
            locate_phrase(&words, "submit"),
            VisionHit::Point { x: 120, y: 106 }
        );
    }

    #[test]
    fn phrase_across_contiguous_words() {
        let words = vec![w("Sign", 10, 10), w("in", 52, 10)];
        assert!(matches!(
            locate_phrase(&words, "sign in"),
            VisionHit::RegionCenter { .. }
        ));
    }

    #[test]
    fn missing_phrase_is_not_found() {
        assert_eq!(locate_phrase(&[], "nothing here"), VisionHit::NotFound);
        let words = vec![w("Yes", 0, 0)];
        assert_eq!(locate_phrase(&words, "no"), VisionHit::NotFound);
    }

    #[test]
    fn union_region_spans_all_words() {
        let words = vec![w("a", 0, 0), w("b", 100, 0)];
        let r = union_region(&words).unwrap();
        assert_eq!(r.width, 140);
    }
}
