//! `ARCH/10-KERNEL.md` §6 — **durable store conventions**: one writer per store,
//! forward-only idempotent migrations, and a gate that keeps a half-migrated
//! store from serving traffic.
//!
//! `INV-06` says each durable store has exactly one writer, and §6 says
//! migrations run **before** the feature that needs them and **fail closed**.
//! Both are runtime rules about a SQLite file, so they are stated here as data
//! and types the owning store applies — this crate opens no connection (it is
//! deliberately IO-free), which is exactly why a store owner cannot skip the
//! gate: the gate is the only thing that says a store is ready to serve.
//!
//! # The three rules
//!
//! 1. **One writer per store.** [`StoreRegistry::claim_writer`] hands out one
//!    writer lease per [`StoreKind`] per process. A second claim is refused
//!    with the incumbent's identity, so a second writer is a startup error
//!    naming its cause rather than a corrupted file found later. The lease is a
//!    process-lifetime claim: a store that outlives the process is reopened by
//!    the next process, which is not a second writer.
//! 2. **Forward-only, idempotent migrations.** [`Migration`] is a numbered step
//!    with an explicit SQL body; [`MigrationPlan`] is the ordered ladder. A
//!    migration is `Idempotent` (re-running it is a no-op) or `OneShot` (it
//!    refuses to re-run). There is no `down`, and [`MigrationPlan::plan`]
//!    refuses a store that is *ahead* of the binary — a rollback is not a
//!    migration, it is a restore.
//! 3. **A half-migrated store never serves.** [`StoreGate::open`] returns
//!    [`GateDecision::Blocked`] with the exact migration that failed, so the
//!    caller reports the migration and the error and disables the dependent
//!    feature instead of reading a partial schema.
//!
//! # Cross-module access
//!
//! [`StoreKind`] carries the single owner module. A reader that is not the owner
//! does not get a connection: it asks the owning service for a projection
//! (`INV-11`). That is a review rule rather than something this crate can
//! enforce at runtime, and it is stated here so the registry is the place a
//! reviewer looks.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The durable stores of the system, each with exactly one owning module
/// (`INV-06`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    /// Work items, runs, steps, checkpoints (`11-WORK`).
    Work,
    /// The single append-only event log (`30-EVENTS`).
    Event,
    /// Memory layers and their derived indexes (`17-MEMORY`).
    Memory,
    /// Artifacts, versions, receipts (`29-ARTIFACTS`).
    Artifact,
    /// World-object graph state (`21-WORLD-MODEL`).
    World,
    /// The encrypted credential store (`12-TRUST`). Listed because it is a
    /// durable store with an owner; its *contents* are never a projection
    /// (`INV-02`), so no other module may read it at all.
    Vault,
    /// The configuration snapshot a run is pinned to (`10-KERNEL`, §4). Small,
    /// versioned, and read-only once written.
    Config,
}

impl StoreKind {
    /// Every store kind, in the registry's own order.
    pub const ALL: [StoreKind; 7] = [
        StoreKind::Work,
        StoreKind::Event,
        StoreKind::Memory,
        StoreKind::Artifact,
        StoreKind::World,
        StoreKind::Vault,
        StoreKind::Config,
    ];

    /// The `ARCH/NN` document number of the owning module.
    pub const fn owner_doc(self) -> &'static str {
        match self {
            Self::Work => "11",
            Self::Event => "30",
            Self::Memory => "17",
            Self::Artifact => "29",
            Self::World => "21",
            Self::Vault => "12",
            Self::Config => "10",
        }
    }

    /// The owning module's name, for a startup error that names its cause.
    pub const fn owner(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Event => "events",
            Self::Memory => "memory",
            Self::Artifact => "artifacts",
            Self::World => "world",
            Self::Vault => "trust",
            Self::Config => "kernel",
        }
    }

    /// Whether a non-owner module may read projections from this store.
    /// `false` for the vault: its contents are not a projection at all
    /// (`INV-02`), so "read a projection" is not an offer anyone can make.
    pub const fn serves_projections_to_non_owners(self) -> bool {
        !matches!(self, Self::Vault)
    }

    /// The stable token for a log line, a receipt or an audit row.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Event => "event",
            Self::Memory => "memory",
            Self::Artifact => "artifact",
            Self::World => "world",
            Self::Vault => "vault",
            Self::Config => "config",
        }
    }
}

impl fmt::Display for StoreKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} store (owner: {})", self.as_str(), self.owner())
    }
}

/// Why a writer claim was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterAlreadyClaimed {
    /// The store that is already claimed.
    pub store: StoreKind,
    /// Who holds the claim.
    pub holder: String,
}

impl fmt::Display for WriterAlreadyClaimed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the {} already has a writer: `{}`. {} is its only owner (INV-06) — a second \
             writer means a second owner, which is an architecture violation, not a retry.",
            self.store,
            self.holder,
            self.store.owner()
        )
    }
}

impl std::error::Error for WriterAlreadyClaimed {}

/// The process-wide writer registry: one lease per [`StoreKind`].
///
/// Cheap to construct, meaningful to share. A store owner claims its own kind
/// once at startup and holds the [`WriterLease`] for the process lifetime; the
/// lease's `Drop` releases it, which is what makes a test able to claim, drop
/// and re-claim.
#[derive(Debug, Default)]
pub struct StoreRegistry {
    claimed: Mutex<BTreeMap<StoreKind, String>>,
}

impl StoreRegistry {
    /// An empty registry: nothing is claimed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim the single writer for `store` on behalf of `holder`.
    ///
    /// `holder` names the component taking the lease, so the refusal names the
    /// incumbent rather than just saying "taken".
    pub fn claim_writer(
        &self,
        store: StoreKind,
        holder: impl Into<String>,
    ) -> Result<WriterLease<'_>, WriterAlreadyClaimed> {
        let holder = holder.into();
        let mut claimed = self.lock();
        if let Some(incumbent) = claimed.get(&store) {
            return Err(WriterAlreadyClaimed {
                store,
                holder: incumbent.clone(),
            });
        }
        claimed.insert(store, holder);
        Ok(WriterLease {
            store,
            registry: self,
        })
    }

    /// Who holds `store`'s writer lease, if anyone.
    pub fn writer_of(&self, store: StoreKind) -> Option<String> {
        self.lock().get(&store).cloned()
    }

    /// Whether any lease is outstanding — a shutdown assertion for a process
    /// that expects to be the only writer.
    pub fn is_quiet(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<StoreKind, String>> {
        self.claimed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The exclusive writer lease for one store. Holding it is the proof that this
/// process is the store's only writer; dropping it releases the claim.
#[derive(Debug)]
pub struct WriterLease<'a> {
    store: StoreKind,
    registry: &'a StoreRegistry,
}

impl WriterLease<'_> {
    /// The store this lease is for.
    pub fn store(&self) -> StoreKind {
        self.store
    }
}

impl Drop for WriterLease<'_> {
    fn drop(&mut self) {
        self.registry.lock().remove(&self.store);
    }
}

/// Whether re-running a migration is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationKind {
    /// Re-running is a no-op: `CREATE TABLE IF NOT EXISTS`, an upsert, an
    /// `ALTER TABLE ADD COLUMN` guarded by a column probe. **The default, and
    /// the only kind a new migration may use** — a migration that is not
    /// idempotent cannot be recovered by re-running it, which is the whole
    /// recovery path after a crash mid-migration.
    Idempotent,
    /// Data-destructive and not re-runnable (a backfill that consumes its own
    /// source). Requires the caller to have recorded that it already ran, and
    /// is the reason the gate below reports the exact migration on failure.
    OneShot,
}

/// One numbered migration step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// Position in the ladder, starting at 1. Contiguous and gap-free: a gap is
    /// a missing migration, and a duplicate is a fork.
    pub version: u32,
    /// What the step does, for a report.
    pub name: &'static str,
    /// Whether re-running is safe.
    pub kind: MigrationKind,
    /// The statements to run, in order, inside one transaction.
    pub sql: &'static [&'static str],
}

impl Migration {
    /// An idempotent migration.
    pub const fn idempotent(
        version: u32,
        name: &'static str,
        sql: &'static [&'static str],
    ) -> Self {
        Self {
            version,
            name,
            kind: MigrationKind::Idempotent,
            sql,
        }
    }

    /// A one-shot migration.
    pub const fn one_shot(version: u32, name: &'static str, sql: &'static [&'static str]) -> Self {
        Self {
            version,
            name,
            kind: MigrationKind::OneShot,
            sql,
        }
    }
}

/// Why a store cannot serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    /// The ladder has two migrations at one version, or a gap. Either means the
    /// ladder is not a sequence, and running it would apply an unknown order.
    LadderNotContiguous {
        /// The first offending version.
        version: u32,
        /// What is wrong with it.
        detail: &'static str,
    },
    /// The store's recorded version is **ahead** of this binary's ladder: the
    /// file was written by a newer build. Serving it would mean guessing what a
    /// newer schema means, and "fixing" it by migrating down is not a rollback
    /// path — it is a restore.
    StoreAheadOfBinary {
        /// What the store says.
        store_version: u32,
        /// What this binary knows.
        binary_version: u32,
    },
    /// A migration failed. The store is now half-migrated and must not serve.
    Failed {
        /// The exact migration that failed.
        version: u32,
        /// Its name.
        name: &'static str,
        /// The driver's error text.
        detail: String,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LadderNotContiguous { version, detail } => {
                write!(f, "the migration ladder is broken at v{version}: {detail}")
            }
            Self::StoreAheadOfBinary {
                store_version,
                binary_version,
            } => write!(
                f,
                "the store is at schema v{store_version} but this build only knows v{binary_version}: \
                 it was written by a newer build. Downgrading is not a migration — restore a backup \
                 or run the newer build. Serving it now would read a schema this build does not understand."
            ),
            Self::Failed {
                version,
                name,
                detail,
            } => write!(
                f,
                "migration v{version} (`{name}`) failed: {detail}. The store is half-migrated and \
                 must not serve traffic; re-run the migration (it is idempotent) or restore."
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

/// The ordered ladder of migrations for one store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    store: StoreKind,
    steps: Vec<Migration>,
}

impl MigrationPlan {
    /// A ladder, checked for contiguity and uniqueness. An empty ladder is
    /// refused: a store with no migrations declared is a store nobody
    /// versioned, which is how a half-migrated schema becomes undetectable.
    pub fn new(store: StoreKind, steps: Vec<Migration>) -> Result<Self, MigrationError> {
        if steps.is_empty() {
            return Err(MigrationError::LadderNotContiguous {
                version: 0,
                detail: "a store must declare at least one migration",
            });
        }
        for (index, step) in steps.iter().enumerate() {
            let expected = index as u32 + 1;
            if step.version != expected {
                return Err(MigrationError::LadderNotContiguous {
                    version: step.version,
                    detail: "versions must be contiguous and start at 1",
                });
            }
        }
        Ok(Self { store, steps })
    }

    /// Which store this ladder belongs to.
    pub fn store(&self) -> StoreKind {
        self.store
    }

    /// The version a fully-migrated store is at.
    pub fn target_version(&self) -> u32 {
        self.steps.len() as u32
    }

    /// The whole ladder, for a report or a schema dump.
    pub fn steps(&self) -> &[Migration] {
        &self.steps
    }

    /// What has to happen to bring a store at `store_version` up to date.
    ///
    /// `Ok(&[])` means the store is current and may serve. An empty plan is the
    /// *only* green light in this type: a store that needs work gets a plan, and
    /// a caller that skips it is skipping the gate.
    pub fn plan(&self, store_version: u32) -> Result<Vec<&Migration>, MigrationError> {
        if store_version > self.target_version() {
            return Err(MigrationError::StoreAheadOfBinary {
                store_version,
                binary_version: self.target_version(),
            });
        }
        Ok(self.steps[store_version as usize..].iter().collect())
    }
}

/// What the gate decided about a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// The store is at the target version and may serve.
    Ready,
    /// Migrations are pending. The dependent feature must not serve until they
    /// are applied — this is the "run before the feature that needs them" rule
    /// stated as a value the caller has to handle.
    NeedsMigration {
        /// The pending migrations, in order.
        pending: Vec<u32>,
    },
    /// A migration failed. The store is half-migrated: it must not serve, and
    /// the report names the exact migration.
    Blocked {
        /// The migration that failed.
        version: u32,
        /// Its name.
        name: &'static str,
        /// The driver's error text.
        detail: String,
    },
}

impl GateDecision {
    /// Whether the store may serve traffic.
    pub fn may_serve(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// The refusal a caller should surface, when it may not serve.
    pub fn refusal(&self) -> Option<MigrationError> {
        match self {
            Self::Ready => None,
            Self::NeedsMigration { pending } => Some(MigrationError::LadderNotContiguous {
                version: pending.first().copied().unwrap_or(0),
                detail: "migrations are pending",
            }),
            Self::Blocked {
                version,
                name,
                detail,
            } => Some(MigrationError::Failed {
                version: *version,
                name,
                detail: detail.clone(),
            }),
        }
    }
}

/// The gate a store owner consults before serving: has the schema reached the
/// target, and did the last attempt fail?
#[derive(Debug, Default)]
pub struct StoreGate {
    state: std::sync::Mutex<GateState>,
}

#[derive(Debug, Default, Clone)]
struct GateState {
    /// The version the store is believed to be at. `0` = empty.
    applied: u32,
    /// The last failed migration, if any.
    failed: Option<(u32, &'static str, String)>,
}

impl StoreGate {
    /// A gate for a store at version 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// What this store may do right now.
    ///
    /// A recorded failure dominates everything: a half-migrated store is
    /// `Blocked` even if the pending list is empty, because "the schema version
    /// says it is fine" and "the migration actually failed" are different facts
    /// and only the second one is true.
    pub fn decide(&self, plan: &MigrationPlan) -> GateDecision {
        let state = self.lock();
        if let Some((version, name, detail)) = &state.failed {
            return GateDecision::Blocked {
                version: *version,
                name,
                detail: detail.clone(),
            };
        }
        match plan.plan(state.applied) {
            Ok(pending) if pending.is_empty() => GateDecision::Ready,
            Ok(pending) => GateDecision::NeedsMigration {
                pending: pending.iter().map(|m| m.version).collect(),
            },
            // A store ahead of the binary is not a half-migrated store; it is a
            // store this build must not touch. Report it as blocked with the
            // exact versions so the operator sees which build wrote the file.
            Err(MigrationError::StoreAheadOfBinary {
                store_version,
                binary_version,
            }) => GateDecision::Blocked {
                version: store_version,
                name: "schema-version-ahead-of-binary",
                detail: format!("this build only knows v{binary_version}"),
            },
            Err(other) => GateDecision::Blocked {
                version: 0,
                name: "migration-ladder-invalid",
                detail: other.to_string(),
            },
        }
    }

    /// Record a migration as applied. Only valid when the version is the next
    /// one: skipping a version means a migration was skipped, and accepting that
    /// would let a store claim a version it never reached.
    pub fn record_applied(&self, version: u32) -> Result<(), MigrationError> {
        let mut state = self.lock();
        let expected = state.applied + 1;
        if version != expected {
            return Err(MigrationError::LadderNotContiguous {
                version,
                detail: "migrations must be recorded in order, one at a time",
            });
        }
        state.applied = version;
        Ok(())
    }

    /// Record a failure. The store is now half-migrated and every subsequent
    /// [`Self::decide`] refuses to serve until the failure is cleared.
    pub fn record_failure(&self, migration: &Migration, detail: impl Into<String>) {
        let mut state = self.lock();
        state.failed = Some((migration.version, migration.name, detail.into()));
    }

    /// Clear a recorded failure, after the migration has been re-run
    /// successfully. The caller records the applied version too, in order.
    pub fn clear_failure(&self) {
        self.lock().failed = None;
    }

    /// The version this store is believed to be at.
    pub fn applied_version(&self) -> u32 {
        self.lock().applied
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Whether a content reference or an inline copy is the right call
/// (`ARCH/10-KERNEL.md` §6: "prefer references; inline only when bounded and
/// reconstructable").
///
/// This is a decision helper, not an enforcement mechanism: the kernel cannot see
/// a caller's payload. It exists so the rule is a named, testable predicate
/// rather than a sentence in a doc.
pub fn prefer_content_ref(inline_bytes: usize, reconstructable: bool) -> bool {
    // Bounded *and* reconstructable is the spec's exception; either alone is not
    // enough, because a small unreconstructable blob is still a copy of
    // something only this process has.
    !(inline_bytes <= INLINE_CONTENT_LIMIT && reconstructable)
}

/// The ceiling for inline content: 256 KiB. Above this a payload is a reference,
/// because an inline copy of a large body is both a store-duplication bug and a
/// memory-pressure bug.
pub const INLINE_CONTENT_LIMIT: usize = 256 * 1024;

/// UTF-8 everywhere, sizes in bytes (`ARCH/10-KERNEL.md` §6). The one place a
/// byte length is taken, so "is this text" is one answer rather than one per
/// call site.
pub fn utf8_byte_len(text: &str) -> usize {
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> MigrationPlan {
        MigrationPlan::new(
            StoreKind::Memory,
            vec![
                Migration::idempotent(
                    1,
                    "create_layers",
                    &["CREATE TABLE IF NOT EXISTS layers (id TEXT)"],
                ),
                Migration::idempotent(
                    2,
                    "add_ttl",
                    &["ALTER TABLE layers ADD COLUMN ttl_ms INTEGER"],
                ),
                Migration::idempotent(
                    3,
                    "create_fts",
                    &["CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(id)"],
                ),
            ],
        )
        .expect("a contiguous ladder is accepted")
    }

    #[test]
    fn every_store_has_exactly_one_named_owner() {
        assert_eq!(StoreKind::ALL.len(), 7);
        for store in StoreKind::ALL {
            assert!(!store.owner().is_empty(), "{store}");
            assert!(
                store.owner_doc().chars().all(|c| c.is_ascii_digit()),
                "{store} names a module doc"
            );
        }
        // The vault serves no projections at all (INV-02), and the work store
        // does — the distinction is the rule, not a coincidence.
        assert!(!StoreKind::Vault.serves_projections_to_non_owners());
        assert!(StoreKind::Work.serves_projections_to_non_owners());
        assert_eq!(StoreKind::Work.to_string(), "work store (owner: work)");
    }

    #[test]
    fn a_second_writer_is_refused_and_names_the_incumbent() {
        let registry = StoreRegistry::new();
        let lease = registry
            .claim_writer(StoreKind::Work, "work-service")
            .expect("the first claim wins");
        assert_eq!(
            registry.writer_of(StoreKind::Work).as_deref(),
            Some("work-service")
        );
        assert!(!registry.is_quiet());

        // A different store is unaffected: one writer *per store*, not one
        // writer in the process.
        let other = registry
            .claim_writer(StoreKind::Event, "event-service")
            .expect("a different store has its own single writer");

        let err = registry
            .claim_writer(StoreKind::Work, "rogue-replica")
            .expect_err("a second writer for the same store is refused");
        assert_eq!(err.store, StoreKind::Work);
        assert_eq!(err.holder, "work-service");
        let shown = err.to_string();
        assert!(
            shown.contains("work-service"),
            "names the incumbent: {shown}"
        );
        assert!(shown.contains("INV-06"), "{shown}");

        // Dropping the lease releases the claim, which is what lets a
        // single-writer test run more than once.
        drop(lease);
        drop(other);
        assert!(registry.is_quiet());
        assert!(
            registry
                .claim_writer(StoreKind::Work, "work-service")
                .is_ok()
        );
    }

    #[test]
    fn concurrent_claims_hand_out_exactly_one_lease() {
        let registry = std::sync::Arc::new(StoreRegistry::new());
        // A barrier makes the eight claims genuinely simultaneous, and each
        // winner holds its lease until every thread has been served — a lease
        // dropped on the spot would let the next thread win in turn and prove
        // nothing about exclusivity.
        let start = std::sync::Arc::new(std::sync::Barrier::new(9));
        let held = std::sync::Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for index in 0..8 {
                let registry = std::sync::Arc::clone(&registry);
                let start = std::sync::Arc::clone(&start);
                let held = std::sync::Arc::clone(&held);
                handles.push(scope.spawn(move || {
                    start.wait();
                    let lease = registry
                        .claim_writer(StoreKind::Artifact, format!("writer-{index}"))
                        .ok();
                    // Nobody releases the winner's lease until every other
                    // thread has been served.
                    held.wait();
                    lease.map(|lease| lease.store())
                }));
            }
            start.wait();
            let winners: Vec<StoreKind> = handles
                .into_iter()
                .filter_map(|h| h.join().expect("thread"))
                .collect();
            assert_eq!(
                winners,
                vec![StoreKind::Artifact],
                "8 simultaneous claims on one store, exactly one writer"
            );
        });
    }

    #[test]
    fn a_ladder_must_be_contiguous_from_one() {
        assert!(
            MigrationPlan::new(StoreKind::Work, vec![]).is_err(),
            "empty ladder"
        );
        // A gap.
        let err = MigrationPlan::new(
            StoreKind::Work,
            vec![
                Migration::idempotent(1, "a", &["SELECT 1"]),
                Migration::idempotent(3, "c", &["SELECT 1"]),
            ],
        )
        .expect_err("a gap is a missing migration");
        assert!(
            matches!(err, MigrationError::LadderNotContiguous { .. }),
            "{err:?}"
        );
        // Starting at zero.
        assert!(
            MigrationPlan::new(
                StoreKind::Work,
                vec![Migration::idempotent(0, "a", &["SELECT 1"])]
            )
            .is_err()
        );
        // A duplicate version is a fork, and is refused the same way.
        assert!(
            MigrationPlan::new(
                StoreKind::Work,
                vec![
                    Migration::idempotent(1, "a", &["SELECT 1"]),
                    Migration::idempotent(1, "b", &["SELECT 1"]),
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn a_plan_is_the_exact_suffix_of_pending_migrations() {
        let plan = plan();
        assert_eq!(plan.target_version(), 3);
        assert_eq!(plan.store(), StoreKind::Memory);
        assert_eq!(plan.plan(0).unwrap().len(), 3);
        assert_eq!(plan.plan(2).unwrap().len(), 1);
        assert_eq!(plan.plan(2).unwrap()[0].name, "create_fts");
        assert!(
            plan.plan(3).unwrap().is_empty(),
            "a current store has nothing to do"
        );
    }

    #[test]
    fn a_store_from_a_newer_build_is_refused_not_downgraded() {
        let plan = plan();
        let err = plan.plan(4).expect_err("v4 > this build's v3");
        assert_eq!(
            err,
            MigrationError::StoreAheadOfBinary {
                store_version: 4,
                binary_version: 3
            }
        );
        let shown = err.to_string();
        assert!(shown.contains("newer build"), "{shown}");
        assert!(
            shown.contains("restore a backup"),
            "names the real path: {shown}"
        );
    }

    #[test]
    fn a_half_migrated_store_cannot_serve() {
        let plan = plan();
        let gate = StoreGate::new();

        // Fresh store: everything pending, nothing may serve yet.
        assert_eq!(
            gate.decide(&plan),
            GateDecision::NeedsMigration {
                pending: vec![1, 2, 3]
            }
        );
        assert!(!gate.decide(&plan).may_serve());

        gate.record_applied(1).unwrap();
        gate.record_applied(2).unwrap();
        assert_eq!(gate.applied_version(), 2);
        assert_eq!(
            gate.decide(&plan),
            GateDecision::NeedsMigration { pending: vec![3] }
        );

        // The third migration fails. The store is now half-migrated.
        let failing = plan.steps()[2].clone();
        gate.record_failure(&failing, "disk full");
        let decision = gate.decide(&plan);
        assert!(!decision.may_serve());
        match &decision {
            GateDecision::Blocked {
                version,
                name,
                detail,
            } => {
                assert_eq!(*version, 3, "the report names the exact migration");
                assert_eq!(*name, "create_fts");
                assert_eq!(detail, "disk full");
            }
            other => panic!("expected blocked, got {other:?}"),
        }
        let refusal = decision.refusal().expect("a blocked store has a refusal");
        assert!(
            refusal.to_string().contains("v3 (`create_fts`)"),
            "{refusal}"
        );
        assert!(
            refusal.to_string().contains("must not serve traffic"),
            "{refusal}"
        );

        // …and it stays blocked after the plan would otherwise be satisfied. A
        // failure dominates the version counter: "the version says it is fine"
        // and "the migration failed" are different facts.
        gate.record_applied(3).expect("in order");
        assert!(
            !gate.decide(&plan).may_serve(),
            "a recorded failure still blocks"
        );

        // Recovery: re-run (the migration is idempotent), clear, serve.
        gate.clear_failure();
        assert!(gate.decide(&plan).may_serve());
        assert!(gate.decide(&plan).refusal().is_none());
    }

    #[test]
    fn migrations_recorded_out_of_order_are_refused() {
        let gate = StoreGate::new();
        assert!(gate.record_applied(2).is_err(), "cannot skip v1");
        assert!(gate.record_applied(1).is_ok());
        assert!(gate.record_applied(1).is_err(), "cannot apply v1 twice");
        assert_eq!(gate.applied_version(), 1);
    }

    #[test]
    fn a_store_ahead_of_the_binary_is_blocked_with_both_versions() {
        let gate = StoreGate::new();
        gate.record_applied(1).unwrap();
        gate.record_applied(2).unwrap();
        gate.record_applied(3).unwrap();
        let decision = gate.decide(&plan());
        assert!(decision.may_serve());
        // A build that only knows v2 meets a v3 store: blocked, with both
        // versions named, never a silent downgrade.
        let short = MigrationPlan::new(
            StoreKind::Memory,
            vec![
                Migration::idempotent(1, "create_layers", &["SELECT 1"]),
                Migration::idempotent(2, "add_ttl", &["SELECT 1"]),
            ],
        )
        .unwrap();
        match gate.decide(&short) {
            GateDecision::Blocked {
                version, detail, ..
            } => {
                assert_eq!(version, 3);
                assert!(detail.contains("v2"), "{detail}");
            }
            other => panic!("expected blocked, got {other:?}"),
        }
    }

    #[test]
    fn content_is_referenced_unless_it_is_small_and_reconstructable() {
        // Small + reconstructable: inlining is the spec's stated exception.
        assert!(!prefer_content_ref(1_024, true));
        // Small but the only copy: still a reference.
        assert!(prefer_content_ref(1_024, false));
        // Large even when reconstructable: a reference.
        assert!(prefer_content_ref(INLINE_CONTENT_LIMIT + 1, true));
        // The boundary is inclusive, so the rule is a stated constant rather
        // than an off-by-one that drifts.
        assert!(!prefer_content_ref(INLINE_CONTENT_LIMIT, true));
        assert_eq!(utf8_byte_len("héllo"), 6, "sizes are bytes, not chars");
    }
}
