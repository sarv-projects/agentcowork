//! P36 (E14/I2) — learned browser helpers as **persistent skills**.
//!
//! A helper is a small, named, versioned script an agent learned once and
//! keeps under its `agent-workspace/helpers/` directory — it survives runs
//! (P24 browser-harness pattern), unlike a one-shot script. Helpers are
//! stored as JSON beside the skill registry; `helpers/` is workspace-scoped
//! and the grant check stays at the caller (a helper is data, not a right).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HelperKind {
    /// Browser automation recipe (observe → act → verify sequence).
    BrowserAct,
    /// Content extraction/transform recipe.
    Extract,
    /// QA probe (headless check).
    Probe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnedHelper {
    pub name: String,
    pub kind: HelperKind,
    pub description: String,
    /// Version string the agent controls; `install` bumps it on overwrite.
    pub version: String,
    /// The executable source (JS prelude-compatible, E4 surface).
    pub source: String,
    pub installed_at_ms: u64,
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub last_run_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HelperError {
    #[error("helper already installed: {0}")]
    Exists(String),
    #[error("helper not found: {0}")]
    NotFound(String),
    #[error("invalid helper name: {0}")]
    InvalidName(String),
    #[error("io: {0}")]
    Io(String),
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The persistent helper store under `agent-workspace/helpers/`.
#[derive(Debug, Clone)]
pub struct WorkspaceHelpers {
    root: PathBuf,
}

impl WorkspaceHelpers {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.json"))
    }

    pub fn install(&mut self, helper: LearnedHelper, overwrite: bool) -> Result<(), HelperError> {
        if !valid_name(&helper.name) {
            return Err(HelperError::InvalidName(helper.name));
        }
        let path = self.path_for(&helper.name);
        if path.exists() && !overwrite {
            return Err(HelperError::Exists(helper.name));
        }
        fs::create_dir_all(&self.root).map_err(|e| HelperError::Io(e.to_string()))?;
        let json =
            serde_json::to_string_pretty(&helper).map_err(|e| HelperError::Io(e.to_string()))?;
        fs::write(&path, json).map_err(|e| HelperError::Io(e.to_string()))
    }

    pub fn load(&self, name: &str) -> Result<LearnedHelper, HelperError> {
        let path = self.path_for(name);
        let raw = fs::read_to_string(&path).map_err(|_| HelperError::NotFound(name.to_string()))?;
        serde_json::from_str(&raw).map_err(|e| HelperError::Io(e.to_string()))
    }

    pub fn list(&self) -> Vec<LearnedHelper> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<LearnedHelper> = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(raw) = fs::read_to_string(&p) {
                    if let Ok(h) = serde_json::from_str(&raw) {
                        out.push(h);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn remove(&self, name: &str) -> Result<(), HelperError> {
        let path = self.path_for(name);
        if !path.exists() {
            return Err(HelperError::NotFound(name.to_string()));
        }
        fs::remove_file(&path).map_err(|e| HelperError::Io(e.to_string()))
    }

    /// Mark a run: bumps `runs` and `last_run_at_ms` — the evidence that a
    /// learned helper survives runs and is actually used.
    pub fn record_run(&mut self, name: &str, at_ms: u64) -> Result<LearnedHelper, HelperError> {
        let mut h = self.load(name)?;
        h.runs += 1;
        h.last_run_at_ms = Some(at_ms);
        self.install(h.clone(), true)?;
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-helpers-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn helper(name: &str) -> LearnedHelper {
        LearnedHelper {
            name: name.into(),
            kind: HelperKind::BrowserAct,
            description: "d".into(),
            version: "1.0.0".into(),
            source: "observe(pageId).snapshot()".into(),
            installed_at_ms: 1,
            runs: 0,
            last_run_at_ms: None,
        }
    }

    #[test]
    fn install_load_survives_reinstantiation() {
        let root = tmp("persist");
        let mut store = WorkspaceHelpers::new(root.clone());
        store.install(helper("login-flow"), false).unwrap();
        // New store instance over the same root (i.e. a later run) still sees it.
        let store2 = WorkspaceHelpers::new(root.clone());
        let h = store2.load("login-flow").unwrap();
        assert_eq!(h.version, "1.0.0");
        assert_eq!(store2.list().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn overwrite_bumps_version() {
        let root = tmp("overwrite");
        let mut store = WorkspaceHelpers::new(root.clone());
        store.install(helper("a"), false).unwrap();
        assert!(matches!(
            store.install(helper("a"), false),
            Err(HelperError::Exists(_))
        ));
        let mut h = helper("a");
        h.version = "1.1.0".into();
        store.install(h, true).unwrap();
        assert_eq!(store.load("a").unwrap().version, "1.1.0");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn runs_accumulate_across_sessions() {
        let root = tmp("runs");
        let mut store = WorkspaceHelpers::new(root.clone());
        store.install(helper("p"), false).unwrap();
        store.record_run("p", 100).unwrap();
        // New session, same helper dir.
        let mut store2 = WorkspaceHelpers::new(root.clone());
        store2.record_run("p", 200).unwrap();
        assert_eq!(store2.load("p").unwrap().runs, 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_or_missing_errors() {
        let root = tmp("errors");
        let store = WorkspaceHelpers::new(root.clone());
        assert!(matches!(store.load("nope"), Err(HelperError::NotFound(_))));
        assert!(matches!(
            store.remove("nope"),
            Err(HelperError::NotFound(_))
        ));
        let mut store = WorkspaceHelpers::new(root);
        assert!(matches!(
            store.install(helper("bad/name"), false),
            Err(HelperError::InvalidName(_))
        ));
    }
}
