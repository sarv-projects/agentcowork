//! DEC-053 — the one-time data-home migration, owned solely by Core.
//!
//! On startup (see [`crate::boot`]) Core resolves the data home through
//! [`agentcowork_types::env_compat`] and, when the resolved home is the
//! **legacy** path (`EVERYAIOS_HOME` / `~/.everyaios`) while the new path
//! (`~/.agentcowork`) does not exist yet, moves the directory once:
//! `rename` first, copy-then-remove as a cross-device fallback. The
//! `everyaios.toml` inside moves with the directory and is renamed to
//! `agentcowork.toml` when no new-spelling file exists yet.
//!
//! The contract, so there is exactly one migrator and no race:
//!
//! - **idempotent**: a second run is a no-op, never an error;
//! - **never destructive**: a destination that already exists is left
//!   untouched (no merge, no overwrite) and the run degrades to it with a
//!   note;
//! - **failure degrades**: a failed move keeps serving the legacy path and
//!   says so — data loss is never the fallback;
//! - **no other crate migrates anything**: lower-level crates call the shared
//!   resolver and get the path; they never move files.

use agentcowork_types::env_compat;
use std::path::{Path, PathBuf};

/// What one [`ensure_data_home`] run did. Rendered into the boot report so
/// the operator sees every migration, skip and honored legacy value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataHomeReport {
    /// The data home the process must use from here on.
    pub resolved: PathBuf,
    /// True when this run moved the legacy directory to the new path.
    pub migrated: bool,
    /// The legacy directory the move came from (when [`Self::migrated`]).
    pub migrated_from: Option<PathBuf>,
    /// Operator-visible note: skips, degradations and honored legacy values.
    /// `None` on a clean new-path run.
    pub note: Option<String>,
}

impl DataHomeReport {
    /// One line for the boot report.
    pub fn summary(&self) -> String {
        if self.migrated {
            format!(
                "data-home migrated {} → {}",
                self.migrated_from
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "?".into()),
                self.resolved.display(),
            )
        } else {
            format!("data-home {}", self.resolved.display())
        }
    }
}

/// The outcome of moving one directory (no resolution, no env).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The legacy directory now lives at the destination.
    Migrated,
    /// The destination already existed: left untouched, never merged.
    DestinationExists,
    /// Not a directory (or nothing there): nothing to move.
    NothingToMove,
    /// The move failed: the legacy path is untouched and stays in service.
    Degraded,
}

/// Move `legacy` to `dest` once: `rename` first, copy-then-remove as the
/// cross-device fallback. Never merges into an existing destination and
/// never leaves a half-copied destination behind on failure.
pub fn migrate_dir(legacy: &Path, dest: &Path) -> MigrationOutcome {
    if dest.exists() {
        return MigrationOutcome::DestinationExists;
    }
    if !legacy.is_dir() {
        return MigrationOutcome::NothingToMove;
    }
    if legacy == dest {
        return MigrationOutcome::NothingToMove;
    }
    if fs_rename(legacy, dest).is_ok() {
        migrate_config_filename(dest);
        return MigrationOutcome::Migrated;
    }
    // Cross-device (or otherwise un-renamable): copy, then remove the source
    // only after the full copy verified. A failed copy removes the partial
    // destination so a later run retries instead of seeing a half-migrated
    // directory and wrongly concluding the move already happened.
    match copy_dir_all(legacy, dest) {
        Ok(()) => {
            migrate_config_filename(dest);
            // The destination is now complete. Removing the source is
            // best-effort: a failed removal leaves a leftover copy behind
            // but never loses data.
            let _ = std::fs::remove_dir_all(legacy);
            MigrationOutcome::Migrated
        }
        Err(_) => {
            let _ = std::fs::remove_dir_all(dest);
            MigrationOutcome::Degraded
        }
    }
}

/// `rename`, creating the destination parent first (a fresh profile has no
/// `~/.agentcowork` parent chain issues, but a redirected `HOME` might).
fn fs_rename(legacy: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(legacy, dest)
}

/// Inside a just-moved home, rename the legacy `everyaios.toml` to
/// `agentcowork.toml` when the new spelling does not exist yet. Best-effort:
/// a failure here is non-fatal because the config loader reads the legacy
/// filename as a fallback.
fn migrate_config_filename(new_home: &Path) {
    let legacy = new_home.join(env_compat::LEGACY_CONFIG_FILENAME);
    let current = new_home.join(env_compat::CONFIG_FILENAME);
    if legacy.is_file() && !current.exists() {
        let _ = std::fs::rename(&legacy, &current);
    }
}

/// Recursively copy `src` to `dst` (which must not exist). Returns `Err` on
/// the first failure; the caller removes the partial destination.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        let src_path = entry.path();
        if entry.file_type()?.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Resolve the data home and, when it is the legacy path with no new path
/// yet, migrate once. The single entry point Core calls at startup; no other
/// crate calls [`migrate_dir`].
pub fn ensure_data_home() -> DataHomeReport {
    let (resolved, provenance) = env_compat::data_home_with_provenance();
    // A legacy directory that still sits next to the resolved home is never
    // merged or removed — but it must be visible, or its data is silently
    // orphaned. Explicit `AGENTCOWORK_HOME` is the operator's own choice and
    // needs no note; every implicit resolution says what it left behind.
    let legacy_alongside = |resolved: &Path| -> Option<String> {
        let legacy = env_compat::home_base().join(env_compat::LEGACY_DIR_NAME);
        if legacy.exists() && legacy != *resolved {
            Some(format!(
                "legacy {} still exists alongside {}; left untouched (no merge)",
                legacy.display(),
                resolved.display(),
            ))
        } else {
            None
        }
    };
    match provenance {
        env_compat::DataHomeProvenance::NewEnv => DataHomeReport {
            resolved,
            migrated: false,
            migrated_from: None,
            note: None,
        },
        env_compat::DataHomeProvenance::NewDirExists
        | env_compat::DataHomeProvenance::NewDefault => {
            let note = legacy_alongside(&resolved);
            DataHomeReport {
                resolved,
                migrated: false,
                migrated_from: None,
                note,
            }
        }
        env_compat::DataHomeProvenance::LegacyEnv
        | env_compat::DataHomeProvenance::LegacyDirExists => {
            let legacy = resolved.clone();
            if !legacy.exists() {
                // An override pointing at nothing yet (or a removed legacy
                // dir): there is nothing to move; downstream creates it.
                return DataHomeReport {
                    resolved,
                    migrated: false,
                    migrated_from: None,
                    note: None,
                };
            }
            let dest = env_compat::home_base().join(env_compat::NEW_DIR_NAME);
            match migrate_dir(&legacy, &dest) {
                MigrationOutcome::Migrated => DataHomeReport {
                    resolved: dest,
                    migrated: true,
                    migrated_from: Some(legacy),
                    note: Some(format!(
                        "migrated legacy data home to {}",
                        env_compat::NEW_DIR_NAME
                    )),
                },
                MigrationOutcome::DestinationExists => DataHomeReport {
                    resolved: resolved.clone(),
                    migrated: false,
                    migrated_from: None,
                    note: Some(format!(
                        "both {} and the legacy home exist; using {} and leaving the legacy directory untouched (no merge)",
                        env_compat::NEW_DIR_NAME,
                        resolved.display(),
                    )),
                },
                MigrationOutcome::NothingToMove => DataHomeReport {
                    resolved,
                    migrated: false,
                    migrated_from: None,
                    note: None,
                },
                MigrationOutcome::Degraded => DataHomeReport {
                    resolved,
                    migrated: false,
                    migrated_from: None,
                    note: Some(
                        "data-home migration failed; continuing on the legacy path (no data moved)"
                            .to_string(),
                    ),
                },
            }
        }
    }
}

/// Operator-visible notes for every legacy spelling currently honored by
/// this process (DEC-053: a honored legacy value must be visible, not
/// silent). Folded into the boot report; an empty vec is the clean state.
pub fn legacy_env_notes() -> Vec<String> {
    const WATCHED: &[&str] = &[
        "HOME",
        "VAULT_KEY",
        "VAULT_PASSPHRASE",
        "VAULT_KEYFILE",
        "ALLOW_GENERATED_KEY",
        "COORDINATOR_BIN",
        "LLAMAFILE",
        "SKILLS_DIR",
    ];
    WATCHED
        .iter()
        .filter_map(|rest| env_compat::legacy_fallback_note(rest))
        .collect()
}
