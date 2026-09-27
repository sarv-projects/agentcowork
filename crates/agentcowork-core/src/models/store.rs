//! P27 — local model store + registry (exact, doc 79).
//!
//! Layout: `<data_dir>/models/hf/{publisher}/{model}/{quant}-{sha8}.gguf` with
//! a single `index.json` registry (`id`, `path`, `sha256`, `size`, `ctx`,
//! `quant`, `source`). The registry is the derived catalog for `local://`
//! URLs and the picker — never a hardcoded model list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One installed model file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    /// Canonical id: `{publisher}/{model}:{quant}` (the `local://` key).
    pub id: String,
    /// Absolute path to the GGUF/MLX file.
    pub path: String,
    /// Hex sha256 (verified at download; re-verifiable at load).
    pub sha256: String,
    /// Size in bytes on disk.
    pub size: u64,
    /// Context window (0 = unknown).
    pub ctx: u32,
    /// Quant id (e.g. `q4_k_m`), or `mlx`/`unknown`.
    pub quant: String,
    /// Source: `hf`, `ollama-create`, `local`.
    pub source: String,
}

/// The on-disk registry: `index.json` beside the hf model tree.
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    base_dir: PathBuf,
    entries: BTreeMap<String, ModelEntry>,
}

/// Canonical path for a download (doc 79 layout).
pub fn entry_path(base: &Path, publisher: &str, model: &str, quant: &str, sha8: &str) -> PathBuf {
    base.join("hf")
        .join(publisher)
        .join(model)
        .join(format!("{quant}-{sha8}.gguf"))
}

impl ModelRegistry {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            entries: BTreeMap::new(),
        }
    }

    pub fn index_path(&self) -> PathBuf {
        self.base_dir.join("hf").join("index.json")
    }

    /// Load the registry from `index.json` (missing file = empty registry).
    pub fn load(base_dir: PathBuf) -> Self {
        let mut reg = Self::new(base_dir.clone());
        if let Ok(bytes) = std::fs::read(reg.index_path()) {
            if let Ok(list) = serde_json::from_slice::<Vec<ModelEntry>>(&bytes) {
                reg.entries = list.into_iter().map(|e| (e.id.clone(), e)).collect();
            }
        }
        let _ = base_dir;
        reg
    }

    pub fn add(&mut self, entry: ModelEntry) {
        self.entries.insert(entry.id.clone(), entry);
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.entries.get(id)
    }

    pub fn list(&self) -> Vec<&ModelEntry> {
        self.entries.values().collect()
    }

    pub fn remove(&mut self, id: &str) -> Option<ModelEntry> {
        self.entries.remove(id)
    }

    /// Persist `index.json` (atomic temp+rename, same discipline as
    /// `agentcowork-office::atomic`).
    pub fn save(&self) -> std::io::Result<()> {
        let idx = self.index_path();
        let dir = idx.parent().unwrap_or(&self.base_dir);
        std::fs::create_dir_all(dir)?;
        let bytes = serde_json::to_vec_pretty(&self.entries.values().collect::<Vec<_>>())
            .map_err(std::io::Error::other)?;
        let tmp = self.index_path().with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, self.index_path())?;
        Ok(())
    }

    /// Total bytes on disk for "My models" (derived, not cached).
    pub fn total_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        // Per-test subdir: tests run in parallel and a shared dir races on
        // remove/create (spurious `NotFound`). Unique per test, so parallel
        // runs are deterministic.
        let d =
            std::env::temp_dir().join(format!("eaios-models-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn entry_path_follows_doc79_layout() {
        let base = Path::new("/data/models");
        let p = entry_path(base, "microsoft", "phi-4", "q4_k_m", "a1b2c3d4");
        assert_eq!(
            p,
            Path::new("/data/models/hf/microsoft/phi-4/q4_k_m-a1b2c3d4.gguf")
        );
    }

    #[test]
    fn registry_round_trips_through_index_json() {
        let base = tmp("roundtrip");
        let mut reg = ModelRegistry::new(base.clone());
        reg.add(ModelEntry {
            id: "microsoft/phi-4:q4_k_m".into(),
            path: entry_path(&base, "microsoft", "phi-4", "q4_k_m", "a1b2c3d4")
                .to_string_lossy()
                .into_owned(),
            sha256: "ab".repeat(32),
            size: 2_400_000_000,
            ctx: 16384,
            quant: "q4_k_m".into(),
            source: "hf".into(),
        });
        reg.save().unwrap();

        let mut loaded = ModelRegistry::load(base.clone());
        assert_eq!(loaded.list().len(), 1);
        let e = loaded.get("microsoft/phi-4:q4_k_m").unwrap();
        assert_eq!(e.ctx, 16384);
        assert_eq!(e.quant, "q4_k_m");
        assert_eq!(loaded.total_bytes(), 2_400_000_000);

        loaded.remove("microsoft/phi-4:q4_k_m");
        assert!(loaded.get("microsoft/phi-4:q4_k_m").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_index_json_is_empty_registry() {
        let base = tmp("empty");
        let reg = ModelRegistry::load(base.clone());
        assert!(reg.list().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
