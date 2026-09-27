//! P52.7 — MLX sidecar: Apple-Silicon model serving via `mlx_lm.server`.
//!
//! On Apple Silicon the MLX sidecar (unified-memory Metal inference) beats
//! the portable GGUF path, so [`prefer_mlx`] routes there. [`MlxServer`] is
//! the sidecar launch spec (exact argv, test-asserted like
//! [`crate::models::gguf_args`]), and [`mlx_quant_id`] maps a Hugging Face
//! id to the `mlx-community/<name>-4bit` convention.

use serde::{Deserialize, Serialize};

/// True when the MLX sidecar is the preferred backend — i.e. on Apple
/// Silicon (the caller passes the platform detection, e.g. from
/// [`crate::hwfit`] `GpuClass::AppleSilicon`, so this stays pure).
pub fn prefer_mlx(is_apple_silicon: bool) -> bool {
    is_apple_silicon
}

/// The MLX sidecar launch spec: `mlx_lm.server --model <m> --port <p>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MlxServer {
    /// Model id to serve (an [`mlx_quant_id`]-style id).
    pub model: String,
    pub port: u16,
}

impl MlxServer {
    pub fn new(model: String, port: u16) -> Self {
        Self { model, port }
    }

    /// The exact sidecar argv (no shell — spawned directly like the
    /// llamafile path in [`crate::models::ModelsRuntime::serve_gguf`]).
    pub fn argv(&self) -> Vec<String> {
        vec![
            "mlx_lm.server".to_string(),
            "--model".to_string(),
            self.model.clone(),
            "--port".to_string(),
            self.port.to_string(),
        ]
    }
}

/// Map a Hugging Face id to the `mlx-community/<name>-4bit` convention:
/// takes the last path component (`org/name` → `name`), appends `-4bit`
/// unless already suffixed (case-insensitive).
pub fn mlx_quant_id(hf_id: &str) -> String {
    let name = hf_id.rsplit('/').next().unwrap_or(hf_id).trim();
    let name = if name.is_empty() { hf_id.trim() } else { name };
    if name.to_ascii_lowercase().ends_with("-4bit") {
        format!("mlx-community/{name}")
    } else {
        format!("mlx-community/{name}-4bit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_silicon_prefers_mlx_sidecar() {
        assert!(prefer_mlx(true));
        assert!(!prefer_mlx(false));
    }

    #[test]
    fn argv_shape() {
        let s = MlxServer::new("mlx-community/Llama-3-8B-4bit".to_string(), 11436);
        assert_eq!(
            s.argv(),
            vec![
                "mlx_lm.server".to_string(),
                "--model".to_string(),
                "mlx-community/Llama-3-8B-4bit".to_string(),
                "--port".to_string(),
                "11436".to_string(),
            ]
        );
    }

    #[test]
    fn quant_id_follows_mlx_community_convention() {
        assert_eq!(
            mlx_quant_id("meta-llama/Llama-3-8B"),
            "mlx-community/Llama-3-8B-4bit"
        );
        // Already-suffixed ids are not doubled.
        assert_eq!(
            mlx_quant_id("mlx-community/Llama-3-8B-4bit"),
            "mlx-community/Llama-3-8B-4bit"
        );
    }
}
