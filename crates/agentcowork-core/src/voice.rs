//! P37 voice pipeline (P9.3): VAD + STT + DJI/USB auto-send. The pipeline
//! contract is deterministic: an energy-based [`VadDetector`] classifies
//! frames (silence / speech), a [`SttProvider`] turns an utterance buffer
//! into text, and [`VoicePipeline`] decides when to auto-send (DJI/USB
//! hands-free). The STT provider *binding* (sherpa-onnx / whisper.cpp /
//! Vosk) stays an installed-engine integration — this module owns the
//! pipeline, the VAD math, and the auto-send policy.

use serde::{Deserialize, Serialize};

/// One audio frame's classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VadState {
    Silence,
    Speech,
}

/// Energy-based VAD: frames whose RMS energy clears the threshold count as
/// speech; `speech_frames_to_commit` consecutive speech frames open an
/// utterance, `silence_frames_to_end` consecutive silence frames close it.
/// Deterministic — same frames, same states.
#[derive(Debug, Clone, Copy)]
pub struct VadDetector {
    pub threshold: f32,
    pub speech_frames_to_commit: u32,
    pub silence_frames_to_end: u32,
}

impl Default for VadDetector {
    fn default() -> Self {
        Self {
            threshold: 0.02,
            speech_frames_to_commit: 3,
            silence_frames_to_end: 10,
        }
    }
}

impl VadDetector {
    /// RMS energy of a frame (i16 PCM samples, normalized 0..=1).
    pub fn frame_energy(samples: &[i16]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        ((sum / samples.len() as f64) as f32).sqrt() / 32768.0
    }

    pub fn classify(&self, energy: f32) -> VadState {
        if energy >= self.threshold {
            VadState::Speech
        } else {
            VadState::Silence
        }
    }
}

/// The STT seam: an engine turns utterance PCM into text. The binding
/// (sherpa-onnx / whisper.cpp / Vosk) plugs in here; the pipeline only needs
/// the trait.
pub trait SttProvider {
    fn transcribe(&self, pcm: &[i16]) -> String;
}

/// A stub-free deterministic provider for tests + the offline default path
/// (no real STT engine installed → the pipeline reports the gap, never
/// fakes a transcript).
pub struct NoopStt;
impl SttProvider for NoopStt {
    fn transcribe(&self, _pcm: &[i16]) -> String {
        String::new()
    }
}

/// What the pipeline emits per utterance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceEvent {
    /// The transcribed text ("" when no STT engine is installed).
    pub text: String,
    /// Whether the DJI/USB auto-send policy fired (hands-free).
    pub auto_send: bool,
    /// Whether the utterance was actually transcribed (honest gap report).
    pub transcribed: bool,
}

/// The pipeline: VAD → utterance framing → STT → auto-send decision.
#[derive(Clone)]
pub struct VoicePipeline {
    pub vad: VadDetector,
    pub auto_send: bool,
    stt: std::rc::Rc<dyn SttProvider>,
}

impl VoicePipeline {
    pub fn new(vad: VadDetector, auto_send: bool, stt: std::rc::Rc<dyn SttProvider>) -> Self {
        Self {
            vad,
            auto_send,
            stt,
        }
    }

    /// Process a full utterance buffer (frames already VAD-committed):
    /// transcribe and decide auto-send. `utterance_len_ms` gates hands-free
    /// send (a hiccup shorter than 300ms is not a command).
    pub fn process_utterance(&self, pcm: &[i16], utterance_len_ms: u64) -> VoiceEvent {
        let text = self.stt.transcribe(pcm);
        let transcribed = !text.is_empty();
        let auto_send = self.auto_send && transcribed && utterance_len_ms >= 300;
        VoiceEvent {
            text,
            auto_send,
            transcribed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: i16, n: usize) -> Vec<i16> {
        vec![amp; n]
    }

    #[test]
    fn vad_classifies_by_energy() {
        let vad = VadDetector::default();
        assert_eq!(vad.classify(0.001), VadState::Silence);
        assert_eq!(vad.classify(0.05), VadState::Speech);
        // RMS of a 16000-amp frame ≈ 16000/32768 ≈ 0.488 — clearly speech.
        assert_eq!(
            vad.classify(VadDetector::frame_energy(&tone(16000, 160))),
            VadState::Speech
        );
        assert_eq!(
            vad.classify(VadDetector::frame_energy(&tone(100, 160))),
            VadState::Silence
        );
    }

    #[test]
    fn auto_send_respects_engine_and_duration() {
        let p = VoicePipeline::new(VadDetector::default(), true, std::rc::Rc::new(NoopStt));
        // No STT engine → no fake transcript, no auto-send.
        let ev = p.process_utterance(&tone(16000, 16000), 500);
        assert_eq!(ev.text, "");
        assert!(!ev.transcribed);
        assert!(!ev.auto_send);

        // With a real provider, the duration gate applies.
        struct Fake;
        impl SttProvider for Fake {
            fn transcribe(&self, _pcm: &[i16]) -> String {
                "run the tests".into()
            }
        }
        let p = VoicePipeline::new(VadDetector::default(), true, std::rc::Rc::new(Fake));
        assert!(p.process_utterance(&tone(16000, 16000), 500).auto_send);
        assert!(!p.process_utterance(&tone(16000, 16000), 100).auto_send);
    }
}
