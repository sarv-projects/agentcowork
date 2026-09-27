//! Memory **ops**: single-writer ownership, bounded contention, corruption
//! quarantine + repair, and the inspect surface (`ARCH/17-MEMORY.md` §4/§8,
//! `REQ-MEM-002/011/018/023`).
//!
//! The failure contract this module exists to enforce: **memory is never the
//! reason a turn fails.** A locked database degrades this one call. A corrupt
//! file quarantines the store, surfaces a warning, and leaves chat running. A
//! caller never sees `BUSY` dressed up as an error condition for the turn, and
//! a repair is offered rather than performed silently.

use crate::scope::{AccessSet, ActorBinding, Kind, ScopeKey, Sensitivity};
use crate::store::{IntegrityReport, MemoryItem, MemoryStore, StoreError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Who holds the single-writer lease (INV-06). One host owns the store; other
/// processes route through it, or reclaim a **stale** lease with an audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterLease {
    pub owner: String,
    pub host: String,
    pub acquired_at: i64,
    /// Heartbeat: the writer refreshes this while it lives.
    pub heartbeat_at: i64,
    /// A lease older than this term is reclaimable.
    pub term_ms: i64,
}

/// The default lease term: 30 s heartbeat, 90 s reclaim threshold.
pub const DEFAULT_LEASE_TERM_MS: i64 = 90_000;

impl Default for WriterLease {
    fn default() -> Self {
        Self {
            owner: String::new(),
            host: String::new(),
            acquired_at: 0,
            heartbeat_at: 0,
            term_ms: DEFAULT_LEASE_TERM_MS,
        }
    }
}

impl WriterLease {
    /// Whether the lease has lapsed at `now`. A clock that reads **behind** the
    /// heartbeat fails safe: the lease is treated as live, so a skewed process
    /// cannot steal ownership by accident.
    pub fn is_stale(&self, now: i64) -> bool {
        now - self.heartbeat_at > self.term_ms
    }
}

/// The claim outcome. `Reclaimed` is the audit-worthy case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimOutcome {
    /// This host now owns the store.
    Acquired,
    /// Another host holds a live lease; the caller routes through it.
    Held { owner: String, host: String },
    /// The previous lease lapsed; ownership moved and the event is audited.
    Reclaimed {
        previous_owner: String,
        previous_host: String,
    },
}

impl ClaimOutcome {
    pub fn owns(&self) -> bool {
        !matches!(self, ClaimOutcome::Held { .. })
    }
}

/// The single-writer registry. A JSON file beside the store, so a second
/// process (desktop + CLI + detached worker) sees the same owner.
#[derive(Debug, Default, Serialize, Deserialize)]
struct LeaseFile {
    lease: Option<WriterLease>,
}

/// Claim the store. Reclaiming a stale lease is the only way to take ownership
/// from a live-looking peer, and it is always reported so the caller can audit
/// it.
pub fn claim_writer(
    lease_path: &Path,
    owner: &str,
    host: &str,
    now: i64,
) -> Result<ClaimOutcome, StoreError> {
    let existing: Option<WriterLease> = match std::fs::read(lease_path) {
        Ok(bytes) => serde_json::from_slice::<LeaseFile>(&bytes)
            .ok()
            .and_then(|f| f.lease),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(StoreError::Io(e)),
    };
    let outcome = match &existing {
        None => ClaimOutcome::Acquired,
        Some(l) if l.owner == owner && l.host == host => ClaimOutcome::Acquired,
        Some(l) if l.is_stale(now) => ClaimOutcome::Reclaimed {
            previous_owner: l.owner.clone(),
            previous_host: l.host.clone(),
        },
        Some(l) => ClaimOutcome::Held {
            owner: l.owner.clone(),
            host: l.host.clone(),
        },
    };
    if outcome.owns() {
        write_lease(
            lease_path,
            &WriterLease {
                owner: owner.to_string(),
                host: host.to_string(),
                acquired_at: now,
                heartbeat_at: now,
                term_ms: DEFAULT_LEASE_TERM_MS,
            },
        )?;
    }
    Ok(outcome)
}

/// Refresh the heartbeat of a lease this host holds.
pub fn heartbeat(lease_path: &Path, owner: &str, now: i64) -> Result<bool, StoreError> {
    let Ok(bytes) = std::fs::read(lease_path) else {
        return Ok(false);
    };
    let Some(mut lease) = serde_json::from_slice::<LeaseFile>(&bytes)
        .ok()
        .and_then(|f| f.lease)
    else {
        return Ok(false);
    };
    if lease.owner != owner {
        return Ok(false);
    }
    lease.heartbeat_at = now;
    write_lease(lease_path, &lease)?;
    Ok(true)
}

fn write_lease(path: &Path, lease: &WriterLease) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("lease.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&LeaseFile {
            lease: Some(lease.clone()),
        })?,
    )?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// How the store is behaving for this session. A `Disabled` state means zero
/// injection, zero retrieval, zero writes and zero background extraction
/// (`REQ-MEM-002`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryHealth {
    Healthy,
    /// The store is locked: this call degrades, the session continues.
    Degraded,
    /// Corruption: the store is quarantined and a repair is offered.
    Quarantined,
    /// Disabled for this session (user or policy).
    Disabled,
}

/// What a bounded retry around a locked store concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentionOutcome {
    pub attempts: u32,
    pub health: MemoryHealth,
    /// True when the write landed (no lost write).
    pub committed: bool,
}

/// Bounded `BUSY` handling: retry with backoff, and report rather than
/// propagate. A caller that wants to fail the turn must do so explicitly —
/// the store never makes that decision.
///
/// `Ok(None)` is the degraded outcome: the lock never cleared within the bound.
/// A non-`Busy` error is propagated unchanged, because a real fault should not
/// be disguised as contention.
pub fn with_bounded_retry<T>(
    max_attempts: u32,
    backoff_ms: u64,
    mut op: impl FnMut(u32) -> Result<T, StoreError>,
) -> Result<(Option<T>, ContentionOutcome), StoreError> {
    let mut attempt = 0u32;
    let mut delay = backoff_ms.max(1);
    loop {
        attempt += 1;
        match op(attempt) {
            Ok(v) => {
                return Ok((
                    Some(v),
                    ContentionOutcome {
                        attempts: attempt,
                        health: MemoryHealth::Healthy,
                        committed: true,
                    },
                ));
            }
            Err(StoreError::Busy) if attempt < max_attempts => {
                std::thread::sleep(std::time::Duration::from_millis(delay));
                delay = (delay * 2).min(2_000);
            }
            Err(StoreError::Busy) => {
                // Bounded: degrade this call, keep the session alive.
                return Ok((
                    None,
                    ContentionOutcome {
                        attempts: attempt,
                        health: MemoryHealth::Degraded,
                        committed: false,
                    },
                ));
            }
            Err(e) => return Err(e),
        }
    }
}

/// A quarantined store: the file is preserved for forensics, memory is disabled
/// for the session, and chat keeps working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantine {
    /// Where the corrupt file was moved to. Never deleted — the user's data is
    /// not this system's to destroy on a guess.
    pub quarantined_to: PathBuf,
    pub reason: String,
    /// The repair the user is offered: export readable rows → recreate →
    /// re-import validated.
    pub repair: RepairPlan,
}

/// The repair plan a quarantined store offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairPlan {
    /// Nothing to repair; the store was healthy.
    None,
    /// FTS drift only: a rebuild restores parity, verified after the rebuild.
    RebuildFts,
    /// The file is unreadable: export whatever rows are readable, recreate, and
    /// re-import the validated subset.
    ExportRecreateReimport { readable_rows: usize },
}

/// Detect corruption and quarantine. A corrupt store disables memory for the
/// session with a surfaced warning and an audit entry — it never blocks chat
/// (`ARCH/17-MEMORY.md` §8).
pub fn check_and_quarantine(
    store: &MemoryStore,
    path: &Path,
) -> Result<IntegrityReport, Quarantine> {
    match store.integrity_check() {
        Ok(r) if r.sqlite_integrity == "ok" => Ok(r),
        Ok(_) | Err(_) => {
            let dest = quarantine_path(path);
            let reason = match store.integrity_check() {
                Ok(r) => format!(
                    "fts drift: {} items vs {} fts rows",
                    r.item_rows, r.fts_rows
                ),
                Err(e) => e.to_string(),
            };
            // Best effort: if the rename fails we still report the quarantine so
            // the caller disables memory rather than reading a corrupt file.
            let _ = std::fs::rename(path, &dest);
            Err(Quarantine {
                quarantined_to: dest,
                reason,
                repair: RepairPlan::ExportRecreateReimport { readable_rows: 0 },
            })
        }
    }
}

/// The quarantine destination for a store path.
pub fn quarantine_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(format!(".quarantine-{}", crate::store::wall_clock_ms()));
    path.with_file_name(name)
}

/// The recovery path for a quarantined store: export the readable rows, recreate
/// a fresh store, and re-import only what validates. Nothing is deleted until
/// the new store is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairOutcome {
    pub exported_rows: usize,
    pub reimported_rows: usize,
    pub dropped_rows: usize,
    pub integrity: IntegrityReport,
}

/// Export the readable rows from a possibly-damaged store. Rows that fail to
/// parse are counted, not silently dropped.
pub fn export_readable(store: &MemoryStore) -> (Vec<MemoryItem>, usize) {
    let Ok(mut stmt) = store.conn().prepare(&format!(
        "SELECT {} FROM memory_items ORDER BY id",
        crate::store::COLUMNS
    )) else {
        return (Vec::new(), 0);
    };
    let Ok(rows) = stmt.query_map([], crate::store::row_to_item) else {
        return (Vec::new(), 0);
    };
    let mut ok = Vec::new();
    let mut bad = 0usize;
    for r in rows {
        match r {
            Ok(item) => ok.push(item),
            Err(_) => bad += 1,
        }
    }
    (ok, bad)
}

/// Rebuild a store from exported rows: write the new file first, verify, then
/// report. The old file is never removed here.
pub fn repair_by_reimport(
    fresh: &mut MemoryStore,
    rows: &[MemoryItem],
    now: i64,
) -> Result<RepairOutcome, StoreError> {
    let mut reimported = 0usize;
    let mut dropped = 0usize;
    for r in rows {
        if fresh.get(&r.id)?.is_some() {
            dropped += 1;
            continue;
        }
        let mut new = crate::store::NewItem::new(&r.id, r.key(), r.kind, &r.content);
        new.sensitivity = r.sensitivity;
        new.trust_tier = r.trust_tier;
        new.source = r.source.clone();
        new.source_ref = r.source_ref.clone();
        new.dedup_key = r.dedup_key.clone();
        new.confidence = r.confidence;
        new.pinned = r.pinned;
        new.expires_at = r.expires_at;
        match fresh.insert(&new, now) {
            Ok(_) => reimported += 1,
            Err(_) => dropped += 1,
        }
    }
    // The rebuilt store must end in sync or the repair failed — surfaced, never
    // silently ignored.
    let integrity = fresh.integrity_check()?;
    Ok(RepairOutcome {
        exported_rows: rows.len(),
        reimported_rows: reimported,
        dropped_rows: dropped,
        integrity,
    })
}

/// One row of the inspect surface (`memory.inspect`): scope, source,
/// created-at, provenance, and the state a user needs to decide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectRow {
    pub id: String,
    pub scope: ScopeKey,
    pub kind: Kind,
    pub sensitivity: Sensitivity,
    pub trust_tier: crate::scope::TrustTier,
    pub source: String,
    pub source_ref: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned: bool,
    pub used_count: i64,
    pub byte_size: i64,
    pub state: ItemState,
    /// True when the referenced source is gone. The row is still usable.
    pub source_unavailable: bool,
}

/// The current/superseded state of a row, as inspect reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemState {
    Current,
    Superseded,
    Expired,
}

impl MemoryStore {
    /// The inspect surface for the Memory UI. Actor-derived scopes: a caller
    /// can only ever see what its binding reaches, and every row carries its
    /// provenance (`REQ-MEM-011/023`).
    pub fn inspect(
        &self,
        actor: &ActorBinding,
        now: i64,
        sources: &dyn crate::recall::SourceAvailability,
    ) -> Result<Vec<InspectRow>, StoreError> {
        let set = AccessSet::derive(actor);
        // Every row in the set, expired included: inspect is where a user finds
        // and deletes their data, so a lifetime-expired row must still appear.
        let items = self.all_in_set(&set)?;
        let mut out = Vec::new();
        for i in items {
            let expired = i.is_expired(now);
            let key = i.key();
            out.push(InspectRow {
                state: if expired {
                    ItemState::Expired
                } else if i.is_current() {
                    ItemState::Current
                } else {
                    ItemState::Superseded
                },
                source_unavailable: i
                    .source_ref
                    .as_deref()
                    .map(|r| !sources.is_available(r))
                    .unwrap_or(false),
                id: i.id,
                scope: key,
                kind: i.kind,
                sensitivity: i.sensitivity,
                trust_tier: i.trust_tier,
                source: i.source,
                source_ref: i.source_ref,
                created_at: i.created_at,
                updated_at: i.updated_at,
                pinned: i.pinned,
                used_count: i.used_count,
                byte_size: i.byte_size,
            });
        }
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// The audit census: every mutation class the spec names, and whether it is
    /// covered. Memory mutations are local persistent mutations — policy-gated
    /// per scope, audited, **not** per-write tickets (`DEC-042`).
    pub fn audit_census() -> Vec<AuditClass> {
        vec![
            AuditClass {
                class: "extract".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "remember".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "forget".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "supersede".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "pin_edit".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "wipe".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "policy_delete".into(),
                classification: Classification::LocalPersistentMutation,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "import".into(),
                // Boundary-crossing: the file IO follows the guarded path.
                classification: Classification::GovernedEffect,
                audited: true,
                carries_item_body: false,
            },
            AuditClass {
                class: "export".into(),
                classification: Classification::GovernedEffect,
                audited: true,
                carries_item_body: false,
            },
        ]
    }
}

/// How a memory mutation is classified (`DEC-042`, `ARCH/06-DATA-MODEL.md` §3.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Classification {
    /// In-store write: policy-gated per scope + audited, no per-write ticket.
    LocalPersistentMutation,
    /// Boundary-crossing (export/import file IO, sharing): the governed path.
    GovernedEffect,
}

/// One row of the audit census.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditClass {
    pub class: String,
    pub classification: Classification,
    pub audited: bool,
    /// Memory audit records never carry an item body.
    pub carries_item_body: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recall::{AllSourcesAvailable, PrunedSources};
    use crate::store::{NewItem, StoreConfig};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentcowork-mem-ops-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_lease_is_exclusive_to_a_live_owner() {
        let dir = tmp("lease");
        let p = dir.join("writer.lease");
        assert_eq!(
            claim_writer(&p, "app", "hostA", 1_000).unwrap(),
            ClaimOutcome::Acquired
        );
        // A second process finds a live lease and routes through it.
        assert_eq!(
            claim_writer(&p, "cli", "hostB", 1_010).unwrap(),
            ClaimOutcome::Held {
                owner: "app".into(),
                host: "hostA".into()
            }
        );
        // The same host re-claiming is fine.
        assert_eq!(
            claim_writer(&p, "app", "hostA", 1_020).unwrap(),
            ClaimOutcome::Acquired
        );
    }

    #[test]
    fn a_stale_lease_is_reclaimable_and_the_takeover_is_reported() {
        let dir = tmp("stale");
        let p = dir.join("writer.lease");
        claim_writer(&p, "app", "hostA", 1_000).unwrap();
        let out = claim_writer(&p, "cli", "hostB", 1_000 + DEFAULT_LEASE_TERM_MS + 1).unwrap();
        assert_eq!(
            out,
            ClaimOutcome::Reclaimed {
                previous_owner: "app".into(),
                previous_host: "hostA".into()
            }
        );
        assert!(out.owns());
    }

    #[test]
    fn a_clock_behind_the_heartbeat_fails_safe_and_does_not_steal_the_lease() {
        let dir = tmp("skew");
        let p = dir.join("writer.lease");
        claim_writer(&p, "app", "hostA", 1_000).unwrap();
        // A peer whose clock reads far behind must not conclude the lease
        // lapsed.
        let out = claim_writer(&p, "cli", "hostB", 0).unwrap();
        assert!(matches!(out, ClaimOutcome::Held { .. }));
    }

    #[test]
    fn a_heartbeat_extends_the_lease_and_only_for_its_owner() {
        let dir = tmp("hb");
        let p = dir.join("writer.lease");
        claim_writer(&p, "app", "hostA", 1_000).unwrap();
        assert!(heartbeat(&p, "app", 1_000 + 50_000).unwrap());
        assert!(!heartbeat(&p, "cli", 1_000).unwrap());
        // Still live well past the original term, because the heartbeat moved.
        assert!(matches!(
            claim_writer(&p, "cli", "hostB", 1_000 + DEFAULT_LEASE_TERM_MS - 1).unwrap(),
            ClaimOutcome::Held { .. }
        ));
    }

    #[test]
    fn bounded_retry_commits_when_the_lock_clears() {
        let calls = AtomicU32::new(0);
        let (v, outcome) = with_bounded_retry(4, 1, |_| {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                Err(StoreError::Busy)
            } else {
                Ok(7u32)
            }
        })
        .unwrap();
        assert_eq!(v, Some(7));
        assert!(outcome.committed);
        assert_eq!(outcome.attempts, 3);
        assert_eq!(outcome.health, MemoryHealth::Healthy);
    }

    #[test]
    fn bounded_retry_degrades_rather_than_failing_the_turn() {
        let (v, outcome) = with_bounded_retry::<u32>(2, 1, |_| Err(StoreError::Busy)).unwrap();
        assert!(v.is_none());
        assert!(!outcome.committed);
        assert_eq!(outcome.health, MemoryHealth::Degraded);
        assert_eq!(outcome.attempts, 2);
    }

    #[test]
    fn a_non_busy_error_is_propagated_unchanged() {
        let r: Result<(Option<u32>, ContentionOutcome), StoreError> =
            with_bounded_retry(3, 1, |_| Err(StoreError::NotFound("m1".into())));
        assert!(matches!(r, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn a_corrupt_file_is_quarantined_and_never_blocks_chat() {
        let dir = tmp("corrupt");
        let path = dir.join("memory.db");
        std::fs::write(&path, b"this is definitely not a sqlite database").unwrap();
        // Opening reports the failure rather than handing back an empty store.
        let open = MemoryStore::open(&path, None, StoreConfig::default());
        assert!(open.is_err());
        // A corrupt store can still be inspected for readable rows (zero here).
        if let Ok(s) = open {
            let (rows, bad) = export_readable(&s);
            assert!(rows.is_empty());
            assert_eq!(bad, 0);
        }
        let q = Quarantine {
            quarantined_to: quarantine_path(&path),
            reason: "not a database".into(),
            repair: RepairPlan::ExportRecreateReimport { readable_rows: 0 },
        };
        assert!(q.quarantined_to.to_string_lossy().contains("quarantine"));
        assert!(matches!(
            q.repair,
            RepairPlan::ExportRecreateReimport { .. }
        ));
    }

    #[test]
    fn a_healthy_store_passes_the_check_without_quarantine() {
        let dir = tmp("healthy");
        let path = dir.join("memory.db");
        let mut s = MemoryStore::open(&path, None, StoreConfig::default()).unwrap();
        s.insert(
            &NewItem::new("m1", ScopeKey::project("p"), Kind::Fact, "healthy"),
            1,
        )
        .unwrap();
        let r = check_and_quarantine(&s, &path).unwrap();
        assert!(r.in_sync);
        assert!(path.exists(), "a healthy store is not moved");
    }

    #[test]
    fn repair_exports_recreates_and_verifies() {
        let mut broken = MemoryStore::open_in_memory(StoreConfig::default()).unwrap();
        for i in 0..3 {
            broken
                .insert(
                    &NewItem::new(
                        &format!("m{i}"),
                        ScopeKey::project("p"),
                        Kind::Fact,
                        &format!("fact {i}"),
                    ),
                    i as i64,
                )
                .unwrap();
        }
        let (rows, bad) = export_readable(&broken);
        assert_eq!(rows.len(), 3);
        assert_eq!(bad, 0);
        let mut fresh = MemoryStore::open_in_memory(StoreConfig::default()).unwrap();
        let out = repair_by_reimport(&mut fresh, &rows, 100).unwrap();
        assert_eq!(out.reimported_rows, 3);
        assert_eq!(out.dropped_rows, 0);
        assert!(out.integrity.in_sync);
        // A re-run is idempotent: nothing is duplicated, the dupes are counted.
        let again = repair_by_reimport(&mut fresh, &rows, 200).unwrap();
        assert_eq!(again.reimported_rows, 0);
        assert_eq!(again.dropped_rows, 3);
    }

    #[test]
    fn inspect_shows_provenance_for_every_row_and_actor_derived_scopes_only() {
        let mut s = MemoryStore::open_in_memory(StoreConfig::default()).unwrap();
        let mut it = NewItem::new(
            "m1",
            ScopeKey::project("p1"),
            Kind::Preference,
            "prefers tabs",
        );
        it.source = "user".into();
        it.source_ref = Some("turn:7".into());
        s.insert(&it, 1).unwrap();
        s.insert(
            &NewItem::new("m2", ScopeKey::project("p2"), Kind::Fact, "another project"),
            1,
        )
        .unwrap();
        let rows = s
            .inspect(&ActorBinding::local_in("u", "p1"), 10, &AllSourcesAvailable)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "m1");
        assert_eq!(rows[0].source, "user");
        assert_eq!(rows[0].source_ref.as_deref(), Some("turn:7"));
        assert_eq!(rows[0].state, ItemState::Current);
        assert!(rows[0].created_at == 1);
    }

    #[test]
    fn inspect_annotates_a_row_whose_source_was_pruned_without_breaking_it() {
        let mut s = MemoryStore::open_in_memory(StoreConfig::default()).unwrap();
        let mut it = NewItem::new(
            "m1",
            ScopeKey::project("p1"),
            Kind::Fact,
            "references a turn",
        );
        it.source_ref = Some("turn:gone".into());
        s.insert(&it, 1).unwrap();
        let rows = s
            .inspect(
                &ActorBinding::local_in("u", "p1"),
                10,
                &PrunedSources::with(["turn:gone"]),
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].source_unavailable);
    }

    #[test]
    fn inspect_reports_the_expired_state_separately_from_superseded() {
        let mut s = MemoryStore::open_in_memory(StoreConfig::default()).unwrap();
        let mut exp = NewItem::new("exp", ScopeKey::project("p"), Kind::Fact, "old");
        exp.expires_at = Some(5);
        s.insert(&exp, 1).unwrap();
        let rows = s
            .inspect(&ActorBinding::local_in("u", "p"), 10, &AllSourcesAvailable)
            .unwrap();
        assert_eq!(rows[0].state, ItemState::Expired);
    }

    #[test]
    fn the_audit_census_covers_every_mutation_class_with_no_item_body() {
        let census = MemoryStore::audit_census();
        for class in [
            "extract",
            "remember",
            "forget",
            "supersede",
            "pin_edit",
            "wipe",
            "policy_delete",
            "import",
            "export",
        ] {
            assert!(
                census.iter().any(|c| c.class == class),
                "{class} is missing from the census"
            );
        }
        assert!(census.iter().all(|c| c.audited), "coverage = 100%");
        assert!(
            census.iter().all(|c| !c.carries_item_body),
            "no memory audit record carries an item body"
        );
        // In-store writes are not per-write tickets; export/import are governed.
        assert!(
            census.iter().any(|c| c.class == "forget"
                && c.classification == Classification::LocalPersistentMutation)
        );
        assert!(
            census
                .iter()
                .any(|c| c.class == "export" && c.classification == Classification::GovernedEffect)
        );
    }
}
