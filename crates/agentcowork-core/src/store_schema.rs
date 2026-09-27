//! P70.A8 — durable-store schema stamps + a forward-only migration path.
//!
//! Every durable store the app owns needs to answer two questions at open
//! time: *what schema is this data written in?* and *is a build that can read
//! it the one that is running?* The second half is the load-bearing one: a
//! downgrade — or a beta install started against release data — must be
//! **refused with a named store**, never guessed at. Opening newer data with an
//! older reader is how a product corrupts a user's history while appearing to
//! work.
//!
//! Two stamp mechanisms, because the stores differ:
//!
//! * **`InStore`** — the store stamps itself. `vault.db` carries a
//!   `schema_meta` table managed by `agentcowork-vault`, which already refuses a
//!   newer `schema_version` (`VaultError::NewerSchema`). The calendar tables
//!   live in that same encrypted database and therefore share its version.
//! * **`Manifest`** — the store is a plain file (NDJSON journal, JSON
//!   document) with no room for self-describing metadata without rewriting
//!   append-only data. Those versions are recorded in
//!   `<data_dir>/store-schema.json`, written atomically, and checked at boot.
//!   A store that already has data but no recorded version is *adopted* as
//!   version 0 and recorded at the current version — the honest statement is
//!   "written before stamps existed", not "already migrated".
//!
//! Derived stores (caches that rebuild from their source) are listed here too,
//! marked [`StorePolicy::Derived`]: they are not stamped, because losing one
//! costs a rebuild rather than data, and pretending a cache is durable would
//! misstate what an upgrade has to preserve.
//!
//! The registry is the single source of truth for which stores exist and which
//! version each is at; `scripts/check-store-schemas.mjs` fails the build when a
//! new durable file appears in the tree without a row here, or when a row's
//! version disagrees with the constant its owning crate actually uses.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The manifest file name, relative to the data directory.
pub const STORE_SCHEMA_FILE: &str = "store-schema.json";

/// The manifest's own format version. Bumped only if the manifest shape
/// changes; it is not a store version.
pub const MANIFEST_VERSION: u32 = 1;

/// How a store's schema version is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorePolicy {
    /// The store stamps itself; the owning crate refuses a newer schema.
    InStore,
    /// The version is recorded in `store-schema.json` and checked at boot.
    Manifest,
    /// A cache that rebuilds from its source; not stamped on purpose.
    Derived,
}

/// One durable store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreSpec {
    /// Stable key used in the manifest (snake_case).
    pub name: &'static str,
    /// Path relative to the data directory (`*` allowed for per-workspace data).
    pub path: &'static str,
    /// The schema version this build reads and writes.
    pub version: u32,
    pub policy: StorePolicy,
    /// What the store holds and what an upgrade must preserve — the reason the
    /// row exists, stated where the next reader of this file will see it.
    pub note: &'static str,
}

/// The registry. Adding a durable store means adding a row (and the gate in
/// `scripts/check-store-schemas.mjs` will ask for it).
pub const STORES: &[StoreSpec] = &[
    StoreSpec {
        name: "vault",
        path: "vault.db",
        version: 8,
        policy: StorePolicy::InStore,
        note: "SQLCipher vault: provider keys, key rings, token usage, the calendar tables. \
               Self-stamped in its `schema_meta` table; `agentcowork-vault` refuses a newer \
               `schema_version` (`VaultError::NewerSchema`) rather than reading it.",
    },
    StoreSpec {
        name: "calendar",
        path: "vault.db (ui_calendars, ui_calendar_events)",
        version: 8,
        policy: StorePolicy::InStore,
        note: "Calendar rows live inside the encrypted vault database and share its schema \
               version; they are named separately so an upgrade report distinguishes user \
               calendar data from credentials.",
    },
    StoreSpec {
        name: "audit",
        path: "audit.ndjson",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Append-only NDJSON audit trail (single-writer, Merkle-chained). An upgrade must \
               preserve the chain and the sequence continuity; the version is recorded in the \
               store-schema manifest because rewriting an append-only log to insert metadata \
               would invalidate every hash after the insertion point.",
    },
    StoreSpec {
        name: "memory",
        path: "memory.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Persistent memory (warm set, core facts, avoidance records). User-authored \
               content: an upgrade must not drop or duplicate it.",
    },
    StoreSpec {
        name: "work_journal",
        path: "work/events.jsonl",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Work/event journal. Replay is fail-closed (a malformed or duplicate sequence \
               refuses to open), so the stamp stays outside the log.",
    },
    StoreSpec {
        name: "checkpoints",
        path: "workspace/*/*/bp.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Blueprint checkpoints and step checkpoints per workspace. Recovery reads these to \
               classify interrupted work, so an unreadable checkpoint must be a named failure.",
    },
    StoreSpec {
        name: "scheduler",
        path: "scheduler.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "User scheduler / automations. Hand-authored state: an upgrade must preserve every \
               automation the user created.",
    },
    StoreSpec {
        name: "installed_agents",
        path: "agents/*/installed.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "The external agents the user installed or imported, plus their runtime locations. \
               Losing it would drop agents the user had configured without removing them from \
               the machine.",
    },
    StoreSpec {
        name: "tasks",
        path: "tasks.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "The user's task list (the Tasks surface). Hand-authored state: an upgrade must not \
               lose it.",
    },
    StoreSpec {
        name: "catalog_observations",
        path: "provider-observations.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Provider observations learned from real calls (latency, failures, capability \
               drift). Rebuildable in principle, but it is user-visible routing evidence, so it \
               is stamped rather than silently reset on upgrade.",
    },
    StoreSpec {
        name: "cua_graph",
        path: "dependency_graph.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Computer-use dependency graph learned from real runs; the replan path reads it.",
    },
    StoreSpec {
        name: "cua_replan_log",
        path: "replan_log.jsonl",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "Append-only replan history for computer-use runs (diagnostic evidence a user can \
               open).",
    },
    StoreSpec {
        name: "plan_cache",
        path: "plans.db",
        version: 1,
        policy: StorePolicy::Derived,
        note: "Blueprint plan cache. A miss rebuilds the plan; not stamped because losing it is \
               not data loss.",
    },
    StoreSpec {
        name: "repo_cache",
        path: "repo_cache.db",
        version: 1,
        policy: StorePolicy::Derived,
        note: "Code-intelligence repository index. Corrupt or stale data falls back to a full \
               rebuild by design.",
    },
    StoreSpec {
        name: "execution_kernel_checkpoint",
        path: "work/execution-kernel.checkpoint.json",
        version: 1,
        policy: StorePolicy::Derived,
        note: "ExecutionKernel snapshot cache. `work/events.jsonl` is the authority; this file is \
               accepted only after its identities and states validate against the replayed Work \
               events, so a stale or unreadable checkpoint costs a rebuild, never data.",
    },
    StoreSpec {
        name: "acpx_sessions",
        path: "acpx-sessions.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "P51.19 — named acpx driver sessions (cwd, queue, last outcome) so a later `acpx \
               flow`/`doctor` invocation resumes the same named session. Hand-authored state: an \
               upgrade must preserve the session names the user relies on.",
    },
    StoreSpec {
        name: "update_channel",
        path: "update_channel.json",
        version: 1,
        policy: StorePolicy::Manifest,
        note: "P70.C2 — the user's release channel (stable | beta), written atomically by \
               updater_cmds. Losing it reverts the user to the stable channel, never to a \
               broken state; stamped so an upgrade report names it.",
    },
    StoreSpec {
        name: "spool",
        path: "spool/*.blob",
        version: 1,
        policy: StorePolicy::Derived,
        note: "P64.11/P69.G5 — the content-addressed tool-output spool. Derived on purpose: \
               the file name *is* the SHA-256 of its own contents and the payload is opaque \
               bytes, so there is no inner schema to stamp and no migration an upgrade has \
               to perform. It is a bounded cache, not a record — \
               `agentcowork_core::spool::SPOOL_RETENTION_DAYS` (7) and `SPOOL_MAX_TOTAL_BYTES` \
               (512 MiB) reclaim it at boot and before every write, so an upgrade loses at \
               most a bounded window of `retrieve_original` drilldowns that the transcript's \
               own tool result already covers.",
    },
];

/// Look up a store by name.
pub fn store(name: &str) -> Option<&'static StoreSpec> {
    STORES.iter().find(|s| s.name == name)
}

/// The stores whose versions are recorded in the manifest.
pub fn manifest_stores() -> impl Iterator<Item = &'static StoreSpec> {
    STORES.iter().filter(|s| s.policy != StorePolicy::Derived)
}

/// One recorded store version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreStamp {
    /// The version the data was last written at.
    pub version: u32,
    /// How the version is carried (`in_store` / `manifest`).
    pub policy: String,
    /// True when the store existed before stamps were introduced and was
    /// adopted at its current version rather than migrated.
    #[serde(default)]
    pub adopted: bool,
    /// The recorded path, so a moved data directory is visible in the file.
    pub path: String,
}

/// The manifest as written to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreManifest {
    pub manifest_version: u32,
    pub stores: std::collections::BTreeMap<String, StoreStamp>,
}

impl Default for StoreManifest {
    fn default() -> Self {
        Self {
            manifest_version: MANIFEST_VERSION,
            stores: std::collections::BTreeMap::new(),
        }
    }
}

/// Why a store could not be opened.
// `io::Error` is neither `Eq` nor fully `PartialEq`, so this error is compared
// through `matches!` in tests rather than with `==`.
#[derive(Debug, thiserror::Error)]
pub enum StoreSchemaError {
    /// The recorded schema is newer than this build understands. Forward-only:
    /// never read it, never silently migrate it downwards.
    #[error(
        "store `{store}` holds schema v{found}, but this build supports v{supported} — \
         it was written by a newer version of AgentCowork; refusing to open it rather than \
         reading (or corrupting) data this build does not understand"
    )]
    NewerThanApp {
        store: String,
        found: u32,
        supported: u32,
    },
    /// The manifest could not be read or written.
    #[error("store schema manifest at {path} could not be {action}: {source}")]
    ManifestIo {
        path: String,
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// The manifest exists but is not valid JSON — reported rather than
    /// overwritten, because overwriting would discard the recorded versions.
    #[error("store schema manifest at {path} is not valid JSON: {detail}")]
    ManifestCorrupt { path: String, detail: String },
}

/// Read the manifest. A missing file is an empty manifest (first run);
/// unreadable or corrupt content is an error, never silently replaced.
pub fn load_manifest(data_dir: &Path) -> Result<StoreManifest, StoreSchemaError> {
    let path = data_dir.join(STORE_SCHEMA_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StoreManifest::default()),
        Err(e) => {
            return Err(StoreSchemaError::ManifestIo {
                path: path.display().to_string(),
                action: "read",
                source: e,
            });
        }
    };
    if raw.trim().is_empty() {
        return Ok(StoreManifest::default());
    }
    serde_json::from_str(&raw).map_err(|e| StoreSchemaError::ManifestCorrupt {
        path: path.display().to_string(),
        detail: e.to_string(),
    })
}

/// Write the manifest atomically (temp file + rename), so a crash mid-write
/// cannot leave a half-recorded version set behind.
pub fn save_manifest(data_dir: &Path, manifest: &StoreManifest) -> Result<(), StoreSchemaError> {
    std::fs::create_dir_all(data_dir).map_err(|e| StoreSchemaError::ManifestIo {
        path: data_dir.display().to_string(),
        action: "create",
        source: e,
    })?;
    let path = data_dir.join(STORE_SCHEMA_FILE);
    let tmp = data_dir.join(format!("{STORE_SCHEMA_FILE}.tmp"));
    let body = serde_json::to_vec_pretty(manifest).unwrap_or_default();
    std::fs::write(&tmp, &body).map_err(|e| StoreSchemaError::ManifestIo {
        path: tmp.display().to_string(),
        action: "write",
        source: e,
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| StoreSchemaError::ManifestIo {
        path: path.display().to_string(),
        action: "replace",
        source: e,
    })
}

/// Does the store already hold data? A store that has never been written is
/// recorded at the current version and marked as created, not adopted.
fn store_has_data(data_dir: &Path, spec: &StoreSpec) -> bool {
    if spec.path.contains('*') {
        // Per-workspace stores: a workspace directory with any checkpoint file.
        let workspace = data_dir.join("workspace");
        let Ok(entries) = std::fs::read_dir(&workspace) else {
            return false;
        };
        return entries.flatten().any(|e| e.path().is_dir());
    }
    let head = spec.path.split(' ').next().unwrap_or(spec.path);
    data_dir.join(head).exists()
}

/// Stamp every manifest-carrying store for this data directory, refusing to
/// proceed when one was written by a newer build.
///
/// Returns the manifest as recorded. Callers run this at boot: it is what makes
/// "forward-only" true for the stores that cannot stamp themselves.
pub fn ensure_all(data_dir: &Path) -> Result<StoreManifest, StoreSchemaError> {
    let mut manifest = load_manifest(data_dir)?;
    if manifest.manifest_version > MANIFEST_VERSION {
        return Err(StoreSchemaError::ManifestCorrupt {
            path: data_dir.join(STORE_SCHEMA_FILE).display().to_string(),
            detail: format!(
                "manifest format v{} is newer than this build's v{MANIFEST_VERSION}",
                manifest.manifest_version
            ),
        });
    }
    manifest.manifest_version = MANIFEST_VERSION;

    // Forward-only first: check every recorded store before writing anything,
    // so a refused open leaves the manifest untouched.
    for spec in manifest_stores() {
        if let Some(recorded) = manifest.stores.get(spec.name) {
            if recorded.version > spec.version {
                return Err(StoreSchemaError::NewerThanApp {
                    store: spec.name.to_string(),
                    found: recorded.version,
                    supported: spec.version,
                });
            }
        }
    }

    for spec in manifest_stores() {
        let has_data = store_has_data(data_dir, spec);
        match manifest.stores.get_mut(spec.name) {
            Some(entry) => {
                let was_unstamped = entry.adopted;
                entry.version = spec.version;
                entry.policy = policy_name(spec.policy).to_string();
                entry.path = spec.path.to_string();
                // Stays adopted until a real migration lands for this store.
                entry.adopted = was_unstamped || (has_data && entry.version == 0);
            }
            None => {
                manifest.stores.insert(
                    spec.name.to_string(),
                    StoreStamp {
                        version: spec.version,
                        policy: policy_name(spec.policy).to_string(),
                        // Data present with no record: written before stamps.
                        adopted: has_data,
                        path: spec.path.to_string(),
                    },
                );
            }
        }
    }

    save_manifest(data_dir, &manifest)?;
    Ok(manifest)
}

fn policy_name(policy: StorePolicy) -> &'static str {
    match policy {
        StorePolicy::InStore => "in_store",
        StorePolicy::Manifest => "manifest",
        StorePolicy::Derived => "derived",
    }
}

/// A one-line boot summary: which stores are at which version, so the version a
/// user's data is in is visible without opening anything.
pub fn summary(manifest: &StoreManifest) -> String {
    let mut rows: Vec<String> = manifest
        .stores
        .iter()
        .map(|(name, stamp)| {
            let suffix = if stamp.adopted { ", adopted" } else { "" };
            format!("{name} v{}{suffix}", stamp.version)
        })
        .collect();
    rows.sort();
    format!("{} store(s): {}", rows.len(), rows.join(" · "))
}

/// The data directory a manifest path belongs to, for error messages.
pub fn manifest_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STORE_SCHEMA_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ea-store-schema-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_registry_row_is_unique_and_documented() {
        let mut names: Vec<&str> = STORES.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "store names must be unique");
        for spec in STORES {
            assert!(!spec.path.is_empty(), "{} has no path", spec.name);
            assert!(!spec.note.is_empty(), "{} has no note", spec.name);
            assert!(spec.version >= 1, "{} has version 0", spec.name);
        }
    }

    #[test]
    fn a_first_run_writes_a_manifest_at_the_current_versions() {
        let dir = tmpdir("first-run");
        let manifest = ensure_all(&dir).unwrap();
        assert!(dir.join(STORE_SCHEMA_FILE).exists());
        for spec in manifest_stores() {
            let stamp = manifest.stores.get(spec.name).expect("recorded");
            assert_eq!(stamp.version, spec.version);
            assert!(
                !stamp.adopted,
                "{} had no data, so it is not adopted",
                spec.name
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pre_stamp_data_is_adopted_rather_than_claimed_as_migrated() {
        let dir = tmpdir("adopt");
        std::fs::write(dir.join("audit.ndjson"), "{\"seq\":1}\n").unwrap();
        let manifest = ensure_all(&dir).unwrap();
        let stamp = manifest.stores.get("audit").unwrap();
        assert_eq!(stamp.version, storerow("audit").version);
        assert!(
            stamp.adopted,
            "data that predates stamps must be marked adopted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_recorded_schema_is_refused_and_leaves_the_manifest_alone() {
        let dir = tmpdir("newer");
        let mut manifest = ensure_all(&dir).unwrap();
        manifest.stores.get_mut("memory").unwrap().version = 99;
        save_manifest(&dir, &manifest).unwrap();
        let before = std::fs::read_to_string(dir.join(STORE_SCHEMA_FILE)).unwrap();
        let err = ensure_all(&dir).unwrap_err();
        match err {
            StoreSchemaError::NewerThanApp {
                store,
                found,
                supported,
            } => {
                assert_eq!(store, "memory");
                assert_eq!(found, 99);
                assert_eq!(supported, storerow("memory").version);
            }
            other => panic!("expected NewerThanApp, got {other:?}"),
        }
        let after = std::fs::read_to_string(dir.join(STORE_SCHEMA_FILE)).unwrap();
        assert_eq!(
            before, after,
            "a refused open must not rewrite the manifest"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_manifest_is_reported_not_overwritten() {
        let dir = tmpdir("corrupt");
        std::fs::write(dir.join(STORE_SCHEMA_FILE), "{ not json").unwrap();
        let err = ensure_all(&dir).unwrap_err();
        assert!(matches!(err, StoreSchemaError::ManifestCorrupt { .. }));
        assert_eq!(
            std::fs::read_to_string(dir.join(STORE_SCHEMA_FILE)).unwrap(),
            "{ not json",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_summary_names_every_manifest_store() {
        let dir = tmpdir("summary");
        let manifest = ensure_all(&dir).unwrap();
        let line = summary(&manifest);
        for spec in manifest_stores() {
            assert!(line.contains(spec.name), "summary omits {}", spec.name);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn storerow(name: &str) -> &'static StoreSpec {
        store(name).expect("registry row")
    }
}
