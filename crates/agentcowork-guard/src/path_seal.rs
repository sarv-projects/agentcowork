//! P7.8 — capability-string + path-seal doctrine (doc 64 S2 — serenity
//! `WebContent/main.cpp` pledge/unveil). For script-eval and connector
//! workers: the process starts **closed** (no paths, minimal capabilities),
//! the host **unveils** an allowlist of canonicalized prefixes, and then
//! **seals** — after seal, no runtime path or capability can be added. A
//! sealed worker cannot expand its own reach, which is the whole point.
//!
//! This is the policy layer; OS-level enforcement (pledge/unveil, Landlock)
//! sits under the J21 policy layer and is applied via the sandbox profile.
//! The seal state machine and canonicalization here are pure Rust.

use crate::pathfloor::canonicalize_no_follow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A closed→unveiled→sealed capability set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SealState {
    /// Start closed: nothing is reachable.
    Closed,
    /// Unveiling: the host may add canonicalized prefixes/capabilities.
    Unveiling,
    /// Sealed: no further additions; reads only.
    Sealed,
}

/// The capability-string + path-seal set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSeal {
    pub state: SealState,
    /// Canonicalized path prefixes (unveil-equivalent allowlist).
    pub paths: BTreeSet<String>,
    /// Capability strings (e.g. `fs.read`, `net.connect`, `exec`).
    pub capabilities: BTreeSet<String>,
}

/// Why a seal operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    #[error("seal is {state:?} — no additions after sealing")]
    Sealed { state: SealState },
    #[error("path `{0}` canonicalizes outside the worker's root (escape refused)")]
    OutsideRoot(String),
    #[error("unknown capability string `{0}` (must be `<class>:<detail>` from the declared set)")]
    UnknownCapability(String),
    #[error("cannot seal before unveiling (use `begin_unveil` first)")]
    NotUnveiling,
    #[error("cannot unveil while closed (use `begin_unveil` first)")]
    StillClosed,
}

impl PathSeal {
    /// Start fully closed.
    pub fn closed() -> Self {
        Self {
            state: SealState::Closed,
            paths: BTreeSet::new(),
            capabilities: BTreeSet::new(),
        }
    }

    /// Open the unveil window (pledge-equivalent: capabilities are declared
    /// up front, before any worker code runs).
    pub fn begin_unveil(mut self) -> Self {
        self.state = SealState::Unveiling;
        self
    }

    /// Unveil a path prefix — canonicalized, and refused if it escapes the
    /// worker root (the path-floor invariant).
    pub fn unveil_path(&mut self, root: &str, path: &str) -> Result<(), SealError> {
        match self.state {
            SealState::Sealed => {
                return Err(SealError::Sealed {
                    state: SealState::Sealed,
                });
            }
            SealState::Closed => return Err(SealError::StillClosed),
            SealState::Unveiling => {}
        }
        let canonical = canonicalize_no_follow(path);
        // The unveiled prefix must stay inside the worker root.
        let root_canon = canonicalize_no_follow(root);
        let inside = canonical == root_canon
            || canonical.starts_with(&format!("{}/", root_canon.trim_end_matches('/')));
        if !inside {
            return Err(SealError::OutsideRoot(path.into()));
        }
        self.paths.insert(canonical);
        Ok(())
    }

    /// Unveil a capability string from the declared set (pledge-equivalent —
    /// only strings the host declared may be unveiled).
    pub fn unveil_capability(
        &mut self,
        declared: &[&str],
        capability: &str,
    ) -> Result<(), SealError> {
        match self.state {
            SealState::Sealed => {
                return Err(SealError::Sealed {
                    state: SealState::Sealed,
                });
            }
            SealState::Closed => return Err(SealError::StillClosed),
            SealState::Unveiling => {}
        }
        if !declared.contains(&capability) {
            return Err(SealError::UnknownCapability(capability.into()));
        }
        self.capabilities.insert(capability.into());
        Ok(())
    }

    /// Seal: after this, nothing can be added. Reads still work.
    pub fn seal(&mut self) -> Result<(), SealError> {
        if self.state != SealState::Unveiling {
            return Err(SealError::NotUnveiling);
        }
        self.state = SealState::Sealed;
        Ok(())
    }

    /// Post-seal queries.
    pub fn can_read(&self, path: &str) -> bool {
        let canonical = canonicalize_no_follow(path);
        self.paths.iter().any(|p| {
            canonical == *p || canonical.starts_with(&format!("{}/", p.trim_end_matches('/')))
        })
    }

    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn is_sealed(&self) -> bool {
        self.state == SealState::Sealed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_closed_unveil_then_seal() {
        let declared = ["fs.read", "fs.write", "net.connect"];
        let mut s = PathSeal::closed().begin_unveil();
        // Closed → nothing readable before unveiling.
        assert!(!s.can_read("/tmp/x"));
        s.unveil_path("/tmp/worker-root", "/tmp/worker-root/scratch")
            .unwrap();
        s.unveil_capability(&declared, "fs.write").unwrap();
        s.seal().unwrap();
        assert!(s.is_sealed());
        assert!(s.can_read("/tmp/worker-root/scratch/a/b.txt"));
        assert!(s.has_capability("fs.write"));
        // Sealed: no additions.
        assert!(matches!(
            s.unveil_path("/tmp/worker-root", "/tmp/worker-root/more"),
            Err(SealError::Sealed { .. })
        ));
        assert!(matches!(
            s.unveil_capability(&declared, "net.connect"),
            Err(SealError::Sealed { .. })
        ));
    }

    #[test]
    fn escape_refused_at_unveil_time() {
        let declared = ["fs.read"];
        let mut s = PathSeal::closed().begin_unveil();
        // ../ escape above the worker root.
        assert!(matches!(
            s.unveil_path("/tmp/worker-root", "/tmp/other"),
            Err(SealError::OutsideRoot(_))
        ));
        // Capability not in the declared set.
        assert!(matches!(
            s.unveil_capability(&declared, "shell.exec"),
            Err(SealError::UnknownCapability(_))
        ));
    }

    #[test]
    fn cannot_seal_without_unveiling() {
        let mut s = PathSeal::closed();
        assert!(matches!(s.seal(), Err(SealError::NotUnveiling)));
        let mut s = PathSeal::closed();
        assert!(matches!(
            s.unveil_path("/tmp/r", "/tmp/r/x"),
            Err(SealError::StillClosed)
        ));
    }
}
