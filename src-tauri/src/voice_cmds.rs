//! P50.4.3 — Tauri surface over `agentcowork_core::voice`.
//!
//! VAD math and the utterance pipeline live in the crate. This module only
//! exposes them: classify a frame, process an utterance, report whether an
//! STT engine is actually installed. `NoopStt` is the honest default — an
//! empty transcript is a gap report, never a fabricated sentence.

use agentcowork_core::voice::{NoopStt, VadDetector, VoicePipeline};
use serde_json::{json, Value};
use std::rc::Rc;

#[tauri::command]
pub fn voice_status() -> Value {
    json!({
        "vad": true,
        "sttInstalled": false,
        "ttsEngine": "none",
        "reason": "VAD is live. No on-device STT engine is installed (sherpa-onnx / whisper.cpp / Vosk); transcripts are not invented. Read-aloud uses the UI speechSynthesis path when the platform provides it."
    })
}

#[tauri::command]
pub fn voice_vad_classify(samples: Vec<i16>) -> Value {
    let vad = VadDetector::default();
    let energy = VadDetector::frame_energy(&samples);
    let state = vad.classify(energy);
    json!({ "energy": energy, "state": state })
}

#[tauri::command]
pub fn voice_process_utterance(samples: Vec<i16>, utterance_len_ms: u64, auto_send: bool) -> Value {
    let pipeline = VoicePipeline::new(VadDetector::default(), auto_send, Rc::new(NoopStt));
    let ev = pipeline.process_utterance(&samples, utterance_len_ms);
    json!({
        "text": ev.text,
        "auto_send": ev.auto_send,
        "transcribed": ev.transcribed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_never_claims_stt() {
        let v = voice_status();
        assert_eq!(v["vad"], true);
        assert_eq!(v["sttInstalled"], false);
        assert_eq!(v["transcribed"], serde_json::Value::Null);
    }

    #[test]
    fn classify_speech_vs_silence() {
        let speech = voice_vad_classify(vec![16_000; 160]);
        assert_eq!(speech["state"], "speech");
        let silence = voice_vad_classify(vec![40; 160]);
        assert_eq!(silence["state"], "silence");
    }

    #[test]
    fn noop_stt_does_not_invent_a_transcript() {
        let ev = voice_process_utterance(vec![16_000; 1600], 500, true);
        assert_eq!(ev["text"], "");
        assert_eq!(ev["transcribed"], false);
        assert_eq!(ev["auto_send"], false);
    }
}
