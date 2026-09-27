//! P31.10 — the local agent registry under `<data_dir>/agents/`:
//! list / load / save / duplicate / disable / export. Sharing = exporting
//! the `agent.toml` (the future marketplace rides K6 supply chain, P28 —
//! not here).

use crate::bundle::AgentBundle;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("duplicate agent id: {0}")]
    Duplicate(String),
    #[error("invalid agent id: {0}")]
    InvalidId(String),
    #[error("io: {0}")]
    Io(String),
}

/// One registry row (no full bundle — the store stays light).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMeta {
    pub id: String,
    pub name: String,
    pub emoji: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub description: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The registry. `<data_dir>/agents/<id>/agent.toml` per agent — one
/// directory per agent so scoped assets (skills, helpers) can live beside it.
#[derive(Debug, Clone)]
pub struct AgentRegistry {
    root: PathBuf,
}

impl AgentRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_home() -> PathBuf {
        // DEC-053: the one shared data-home rule (new path first, legacy
        // honored as a fallback). This crate resolves; it never migrates —
        // Core owns the one-time migration at startup.
        agentcowork_types::env_compat::data_home().join("agents")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn agent_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn bundle_path(&self, id: &str) -> PathBuf {
        self.agent_dir(id).join("agent.toml")
    }

    pub fn save(&self, bundle: &AgentBundle) -> Result<(), RegistryError> {
        let id = slug(&bundle.name);
        if !valid_id(&id) {
            return Err(RegistryError::InvalidId(id));
        }
        let dir = self.agent_dir(&id);
        fs::create_dir_all(&dir).map_err(|e| RegistryError::Io(e.to_string()))?;
        let toml = bundle.to_toml().map_err(RegistryError::Io)?;
        fs::write(self.bundle_path(&id), toml).map_err(|e| RegistryError::Io(e.to_string()))
    }

    pub fn load(&self, id: &str) -> Result<AgentBundle, RegistryError> {
        let path = self.bundle_path(id);
        let raw = fs::read_to_string(&path).map_err(|_| RegistryError::NotFound(id.to_string()))?;
        AgentBundle::from_toml(&raw).map_err(RegistryError::Io)
    }

    pub fn meta(&self, id: &str) -> Result<AgentMeta, RegistryError> {
        let b = self.load(id)?;
        Ok(AgentMeta {
            id: id.to_string(),
            name: b.name,
            emoji: b.emoji,
            engine: crate::bundle::EngineBinding::label(b.engine.as_ref()),
            disabled: disabled_for(&self.agent_dir(id)),
            description: b.description,
        })
    }

    pub fn list(&self) -> Vec<AgentMeta> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("agent.toml").exists() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if let Ok(m) = self.meta(name) {
                        out.push(m);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Duplicate a bundle under a new id (the wizard's "make a copy").
    pub fn duplicate(&self, id: &str, new_name: &str) -> Result<String, RegistryError> {
        let mut b = self.load(id)?;
        b.name = new_name.to_string();
        let new_id = slug(&b.name);
        if self.load(&new_id).is_ok() {
            return Err(RegistryError::Duplicate(new_id));
        }
        self.save(&b)?;
        Ok(new_id)
    }

    pub fn set_disabled(&self, id: &str, disabled: bool) -> Result<(), RegistryError> {
        let path = self.agent_dir(id).join(".disabled");
        if disabled {
            fs::write(&path, "disabled").map_err(|e| RegistryError::Io(e.to_string()))
        } else {
            let _ = fs::remove_file(&path);
            Ok(())
        }
    }

    /// Export the `agent.toml` bytes (sharing = the file, future K6).
    pub fn export(&self, id: &str) -> Result<String, RegistryError> {
        fs::read_to_string(self.bundle_path(id))
            .map_err(|_| RegistryError::NotFound(id.to_string()))
    }

    pub fn removes(&self, id: &str) -> Result<(), RegistryError> {
        if !self.agent_dir(id).exists() {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        fs::remove_dir_all(self.agent_dir(id)).map_err(|e| RegistryError::Io(e.to_string()))
    }
}

fn disabled_for(dir: &Path) -> bool {
    dir.join(".disabled").exists()
}

/// Deterministic id from a display name.
pub fn slug(name: &str) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = base.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("agentcowork-agents-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn save_list_load_round_trip() {
        let root = tmp("list");
        let reg = AgentRegistry::new(root.clone());
        let mut b = AgentBundle::new("Budget Analyst");
        b.description = "sums sheets".into();
        reg.save(&b).unwrap();
        let metas = reg.list();
        assert_eq!(metas.len(), 1);
        let back = reg.load("budget-analyst").unwrap();
        assert_eq!(back.name, "Budget Analyst");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_creates_new_agent() {
        let root = tmp("dup");
        let reg = AgentRegistry::new(root.clone());
        reg.save(&AgentBundle::new("Coder")).unwrap();
        let new_id = reg.duplicate("coder", "Coder v2").unwrap();
        assert_eq!(new_id, "coder-v2");
        assert_eq!(reg.list().len(), 2);
        assert!(matches!(
            reg.duplicate("coder", "Coder v2"),
            Err(RegistryError::Duplicate(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn disable_flag_survives() {
        let root = tmp("disable");
        let reg = AgentRegistry::new(root.clone());
        reg.save(&AgentBundle::new("Quiet")).unwrap();
        assert!(!reg.meta("quiet").unwrap().disabled);
        reg.set_disabled("quiet", true).unwrap();
        assert!(reg.meta("quiet").unwrap().disabled);
        reg.set_disabled("quiet", false).unwrap();
        assert!(!reg.meta("quiet").unwrap().disabled);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn export_is_the_toml() {
        let root = tmp("export");
        let reg = AgentRegistry::new(root.clone());
        reg.save(&AgentBundle::new("Export Me")).unwrap();
        let toml = reg.export("export-me").unwrap();
        assert!(toml.contains("export-me") || toml.contains("Export Me"));
        let _ = fs::remove_dir_all(&root);
    }
}
