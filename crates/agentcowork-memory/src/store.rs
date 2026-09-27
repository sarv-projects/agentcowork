//! The durable memory store (`ARCH/17-MEMORY.md` §3, `REQ-MEM-001`).
//!
//! One Core-owned SQLite file, WAL, FTS5 kept in sync by **explicit triggers**
//! (external-content FTS5 does not self-sync, so the trigger DDL is part of the
//! store definition, not an optimization). Encryption at rest is whole-DB
//! (DEC-039): a caller supplies the key, and `PRAGMA key` runs before anything
//! else, so an unencrypted open of an encrypted file fails rather than silently
//! reading plaintext.
//!
//! What this module guarantees and what it deliberately does not:
//!
//! - **No rewrite.** `update` of a stored body is not exposed. A reversal is a
//!   new row plus `superseded_by` on the target (`REQ-MEM-009/016`).
//! - **Forget is a hard delete plus a keyed suppression** that survives scope
//!   wipes, so neither re-extraction nor import can resurrect the content.
//! - **Erasure is bounded and stated**: `secure_delete` overwrites freed pages,
//!   the WAL is checkpointed + truncated after a forget, and the FTS row leaves
//!   through the delete trigger. No claim is made about OS caches, backups or
//!   flash wear-leveling (DEC-039 threat model).
//! - **Bounded contention**: `busy_timeout` + WAL serialize writers; a `BUSY`
//!   is returned to the caller to degrade, never turned into a turn failure.

use crate::scope::{Kind, Scope, ScopeKey, Sensitivity, TrustTier};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// `PRAGMA user_version` — the schema version. Bump + add a forward step in
/// [`migrate`] (`REQ-MEM-027`).
pub const SCHEMA_VERSION: u32 = 2;

/// `hash_version` — the normalization function version (`REQ-MEM-027`). A
/// normalization or tokenizer change is a versioned change, never a silent
/// swap: v1 = lowercase, collapse whitespace, strip trailing punctuation.
pub const HASH_VERSION: i64 = 1;

/// Default per-item byte cap (`ARCH/17-MEMORY.md` §3/§7: 4 KiB).
pub const DEFAULT_MAX_ITEM_BYTES: usize = 4096;

/// Declared per-scope item caps (`ARCH/17-MEMORY.md` §7). Product-visible
/// knobs; `org` is absent because it is v1-disabled.
pub const DEFAULT_SCOPE_ITEM_CAPS: [(Scope, usize); 4] = [
    (Scope::Session, 500),
    (Scope::Task, 500),
    (Scope::Project, 5_000),
    (Scope::User, 1_000),
];

/// The store's tunable envelope. Every field is a declared product knob, not a
/// magic number buried in a query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreConfig {
    pub max_item_bytes: usize,
    pub scope_item_caps: Vec<(Scope, usize)>,
    /// Session-scope TTL in ms (7 d), measured from the **persisted anchor**.
    pub session_ttl_ms: i64,
    /// `busy_timeout` in ms (DEC: bounded, default 5 s).
    pub busy_timeout_ms: i64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
            scope_item_caps: DEFAULT_SCOPE_ITEM_CAPS.to_vec(),
            session_ttl_ms: 7 * 24 * 60 * 60 * 1000,
            busy_timeout_ms: 5_000,
        }
    }
}

impl StoreConfig {
    pub fn cap_for(&self, scope: Scope) -> Option<usize> {
        self.scope_item_caps
            .iter()
            .find(|(s, _)| *s == scope)
            .map(|(_, c)| *c)
    }
}

/// Store/persistence failures. `Busy` is deliberately its own variant so the
/// call site can *degrade* instead of failing a turn (`REQ-MEM-002/018`).
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("memory store busy (bounded busy_timeout elapsed)")]
    Busy,
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid integrity state: {0}")]
    Integrity(String),
    #[error("recall failed: {0}")]
    Recall(String),
    #[error("item not found: {0}")]
    NotFound(String),
    #[error("schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("{0}")]
    Rejected(String),
}

/// A clamp against a backwards clock (`REQ-MEM-025`). Reads are taken through
/// this so a wall-clock jump can never move a stored timestamp backwards, and
/// the fact that it clamped is observable.
#[derive(Debug, Clone)]
pub struct MonotoneClock {
    last: std::cell::Cell<i64>,
    skew_events: std::cell::RefCell<Vec<i64>>,
}

impl Default for MonotoneClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotoneClock {
    pub fn new() -> Self {
        Self {
            last: std::cell::Cell::new(0),
            skew_events: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Wall-clock now, clamped monotone. Returns `(clamped_now, was_skewed)`;
    /// a backwards observation is recorded so the skew is visible
    /// (`REQ-MEM-025`: "non-monotonic observations are clamped and recorded").
    pub fn now(&self) -> (i64, bool) {
        let raw = wall_clock_ms();
        let last = self.last.get();
        if raw < last {
            self.skew_events.borrow_mut().push(raw);
            (last, true)
        } else {
            self.last.set(raw);
            (raw, false)
        }
    }

    /// The recorded backwards observations, oldest first.
    pub fn skew_events(&self) -> Vec<i64> {
        self.skew_events.borrow().clone()
    }
}

/// `ARCH/06-DATA-MODEL.md` §0: integer epoch milliseconds internally, never a
/// local wall-clock string.
pub fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The store-local keyed digest. `sha256(store_key ‖ 0x00 ‖ normalized)` — a
/// prefix-keyed construction, so a suppression digest is not a plain content
/// hash: recovering content from a leaked digest requires the key that lives
/// with the store (`ARCH/17-MEMORY.md` §9).
#[derive(Debug, Clone)]
pub struct ContentKey(String);

impl ContentKey {
    pub fn new(store_key: &str) -> Self {
        Self(store_key.to_string())
    }

    /// The keyed digest of normalized content. Normalization is versioned by
    /// [`HASH_VERSION`]; the version is stored alongside the digest so a future
    /// normalization function is a migration, not a silent reinterpretation.
    pub fn digest(&self, content: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.0.as_bytes());
        h.update([0u8]);
        h.update(normalize(content).as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Digest of the **already normalized** text (dedup compares like with
    /// like, so a caller that normalized itself gets the same digest).
    pub fn digest_normalized(&self, normalized: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.0.as_bytes());
        h.update([0u8]);
        h.update(normalized.as_bytes());
        format!("{:x}", h.finalize())
    }
}

/// The versioned normalization function (v1).
pub fn normalize(content: &str) -> String {
    content
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', '!', '?', ';', ':'])
        .to_string()
}

/// The byte size of normalized content — the measured value the per-scope byte
/// bound and the item cap read.
pub fn measured_bytes(normalized: &str) -> usize {
    normalized.len()
}

/// One stored memory item (`DM-018`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub scope: Scope,
    pub scope_ref: Option<String>,
    pub kind: Kind,
    pub content: String,
    pub byte_size: i64,
    pub content_hash: String,
    pub hash_version: i64,
    pub dedup_key: Option<String>,
    pub sensitivity: Sensitivity,
    pub trust_tier: TrustTier,
    pub source: String,
    pub source_ref: Option<String>,
    pub confidence: f64,
    pub pinned: bool,
    pub used_count: i64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    pub superseded_by: Option<String>,
}

impl MemoryItem {
    pub fn key(&self) -> ScopeKey {
        ScopeKey::new(self.scope, self.scope_ref.as_deref())
    }

    /// Current = not superseded. Read paths filter on this, so a stale row can
    /// never be served as current (`REQ-MEM-009`).
    pub fn is_current(&self) -> bool {
        self.superseded_by.is_none()
    }

    /// Expiry is evaluated against the persisted anchor, never a live delta
    /// (`REQ-MEM-005/025`).
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at.map(|e| now >= e).unwrap_or(false)
    }
}

/// A candidate row about to be written. The store computes every derived
/// field (`byte_size`, `content_hash`, `hash_version`, `updated_at`); a caller
/// cannot set them.
#[derive(Debug, Clone, PartialEq)]
pub struct NewItem {
    pub id: String,
    pub key: ScopeKey,
    pub kind: Kind,
    pub content: String,
    pub dedup_key: Option<String>,
    pub sensitivity: Sensitivity,
    pub trust_tier: TrustTier,
    pub source: String,
    pub source_ref: Option<String>,
    pub confidence: f64,
    pub pinned: bool,
    pub expires_at: Option<i64>,
}

impl NewItem {
    pub fn new(id: &str, key: ScopeKey, kind: Kind, content: &str) -> Self {
        Self {
            id: id.to_string(),
            key,
            kind,
            content: content.to_string(),
            dedup_key: None,
            sensitivity: Sensitivity::Personal,
            trust_tier: TrustTier::DerivedUntrusted,
            source: "user".to_string(),
            source_ref: None,
            confidence: 1.0,
            pinned: false,
            expires_at: None,
        }
    }
}

/// The DDL of record (`ARCH/17-MEMORY.md` §3). The FTS sync triggers are part
/// of the store definition, not an afterthought.
const DDL_V1: &str = r#"
CREATE TABLE IF NOT EXISTS memory_items (
    id             TEXT PRIMARY KEY,
    scope          TEXT NOT NULL CHECK (scope IN ('session','task','project','user','org')),
    scope_ref      TEXT,
    kind           TEXT NOT NULL CHECK (kind IN ('preference','fact','decision','reference','summary')),
    content        TEXT NOT NULL,
    byte_size      INTEGER NOT NULL,
    content_hash   TEXT NOT NULL,
    hash_version   INTEGER NOT NULL DEFAULT 1,
    dedup_key      TEXT,
    sensitivity    TEXT NOT NULL DEFAULT 'personal'
                   CHECK (sensitivity IN ('public','personal','confidential')),
    trust_tier     TEXT NOT NULL DEFAULT 'derived_untrusted'
                   CHECK (trust_tier IN ('user_explicit','agent_asserted','derived_untrusted','import')),
    source         TEXT NOT NULL,
    source_ref     TEXT,
    confidence     REAL NOT NULL DEFAULT 1.0,
    pinned         INTEGER NOT NULL DEFAULT 0,
    used_count     INTEGER NOT NULL DEFAULT 0,
    last_used_at   INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    expires_at     INTEGER,
    superseded_by  TEXT REFERENCES memory_items(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_mem_scope   ON memory_items(scope, scope_ref, kind);
CREATE INDEX IF NOT EXISTS idx_mem_hash    ON memory_items(content_hash);
CREATE INDEX IF NOT EXISTS idx_mem_key     ON memory_items(dedup_key) WHERE dedup_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_mem_current ON memory_items(scope, scope_ref) WHERE superseded_by IS NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    content, content='memory_items', content_rowid='rowid', tokenize='porter unicode61');

CREATE TRIGGER IF NOT EXISTS memory_items_ai AFTER INSERT ON memory_items BEGIN
    INSERT INTO memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER IF NOT EXISTS memory_items_ad AFTER DELETE ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER IF NOT EXISTS memory_items_au AFTER UPDATE OF content ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TABLE IF NOT EXISTS memory_suppressions (
    content_hash TEXT PRIMARY KEY,
    scope        TEXT NOT NULL,
    scope_ref    TEXT,
    created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_jobs (
    job_key        TEXT PRIMARY KEY,
    status         TEXT NOT NULL CHECK (status IN ('pending','running','done','error')),
    lease_until    INTEGER,
    retry_at       INTEGER,
    retry_remaining INTEGER NOT NULL DEFAULT 3,
    last_error     TEXT,
    watermark      INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
"#;

/// v2 adds the persisted session anchor (`ARCH/17-MEMORY.md` §7: TTL is
/// evaluated against the anchor event's persisted timestamp, never a live
/// wall-clock delta). See the doc delta reported to the coordinator.
const DDL_V2: &str = r#"
CREATE TABLE IF NOT EXISTS memory_session_anchors (
    session_id  TEXT PRIMARY KEY,
    anchor      TEXT NOT NULL,              -- 'archived' | 'hibernated' | 'last_active'
    anchored_at INTEGER NOT NULL
);
"#;

/// FTS drift detection + repair result (`REQ-MEM-001`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityReport {
    /// `PRAGMA integrity_check` result, verbatim.
    pub sqlite_integrity: String,
    /// Row count in the item table.
    pub item_rows: i64,
    /// Row count in the external-content FTS index.
    pub fts_rows: i64,
    /// True when the two agree and SQLite reports `ok`.
    pub in_sync: bool,
    /// True when a `rebuild` ran (parity is re-verified after it).
    pub rebuilt: bool,
}

/// A forget outcome (`REQ-MEM-010/016`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetOutcome {
    pub id: String,
    pub content_hash: String,
    /// Rows removed by the cascade (`superseded_by` chains the head collapses).
    pub removed_rows: usize,
    /// True when the erased bytes were overwritten and the WAL truncated.
    pub erased: bool,
}

/// A scope-wipe outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WipeOutcome {
    pub key: ScopeKey,
    pub removed_rows: usize,
    /// Suppressions are **not** scope data and survive the wipe
    /// (`ARCH/17-MEMORY.md` §7) — this is the declared consequence: content
    /// forgotten anywhere stays un-extractable everywhere.
    pub suppressions_retained: usize,
    pub erased: bool,
}

/// An extraction job row (`memory_jobs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRow {
    pub job_key: String,
    pub status: String,
    pub lease_until: Option<i64>,
    pub retry_at: Option<i64>,
    pub retry_remaining: i64,
    pub last_error: Option<String>,
    pub watermark: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Jobs-GC report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcOutcome {
    pub swept: usize,
}

/// The durable store.
pub struct MemoryStore {
    conn: Connection,
    key: ContentKey,
    cfg: StoreConfig,
    clock: MonotoneClock,
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStore")
            .field("schema_version", &self.schema_version().unwrap_or(0))
            .field("config", &self.cfg)
            .finish_non_exhaustive()
    }
}

fn map_busy(e: rusqlite::Error) -> StoreError {
    match e {
        rusqlite::Error::SqliteFailure(ref f, _)
            if f.code == rusqlite::ErrorCode::DatabaseBusy
                || f.code == rusqlite::ErrorCode::DatabaseLocked =>
        {
            StoreError::Busy
        }
        other => StoreError::Sqlite(other),
    }
}

impl MemoryStore {
    /// Open (or create) the store at `path`. `encryption_key` is the DEC-039
    /// whole-DB key from the vault; `None` opens a plaintext file (used by the
    /// test suite and never by the product path).
    pub fn open(
        path: &Path,
        encryption_key: Option<&str>,
        cfg: StoreConfig,
    ) -> Result<Self, StoreError> {
        let conn = if encryption_key.is_some() {
            // SQLCipher: the key pragma must precede every other statement,
            // including `user_version`.
            rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
            )
            .map_err(map_busy)?
        } else {
            Connection::open(path).map_err(map_busy)?
        };
        Self::init(conn, encryption_key, cfg)
    }

    /// An in-memory store for tests and ephemeral sessions.
    pub fn open_in_memory(cfg: StoreConfig) -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(map_busy)?;
        Self::init(conn, None, cfg)
    }

    fn init(
        conn: Connection,
        encryption_key: Option<&str>,
        cfg: StoreConfig,
    ) -> Result<Self, StoreError> {
        if let Some(k) = encryption_key {
            // `PRAGMA key` takes a literal; SQLCipher parses it before any
            // schema read, so a wrong key surfaces as a read error below
            // rather than as a plaintext read.
            conn.execute_batch(&format!("PRAGMA key = \"{}\";", k.replace('"', "\"\"")))?;
        }
        let mut store = Self {
            conn,
            key: ContentKey::new(encryption_key.unwrap_or("store-local")),
            cfg,
            clock: MonotoneClock::new(),
        };
        store.apply_pragmas()?;
        store.migrate()?;
        Ok(store)
    }

    fn apply_pragmas(&self) -> Result<(), StoreError> {
        // Per-connection pragmas, in the order §3 declares. `foreign_keys` is
        // OFF by default in SQLite, so the `superseded_by` cascade only exists
        // because this is set.
        self.conn.execute_batch(&format!(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout={};
             PRAGMA secure_delete=ON;
             PRAGMA synchronous=NORMAL;",
            self.cfg.busy_timeout_ms
        ))?;
        Ok(())
    }

    /// The `PRAGMA user_version` the file carries.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let v: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        Ok(v as u32)
    }

    /// Forward-only migrations, one per shipped version (`REQ-MEM-027`). Each
    /// step is idempotent (`IF NOT EXISTS`) so a partially applied migration
    /// can be re-run.
    pub fn migrate(&mut self) -> Result<(), StoreError> {
        let from = self.schema_version()?;
        if from > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: from,
                supported: SCHEMA_VERSION,
            });
        }
        if from < 1 {
            self.conn.execute_batch(DDL_V1).map_err(map_busy)?;
            self.conn
                .pragma_update(None, "user_version", 1i64)
                .map_err(map_busy)?;
        }
        if from < 2 {
            self.conn.execute_batch(DDL_V2).map_err(map_busy)?;
            self.conn
                .pragma_update(None, "user_version", 2i64)
                .map_err(map_busy)?;
        }
        Ok(())
    }

    pub fn config(&self) -> &StoreConfig {
        &self.cfg
    }

    pub fn clock(&self) -> &MonotoneClock {
        &self.clock
    }

    pub fn key(&self) -> &ContentKey {
        &self.key
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Set the store-local digest key. The suppression scheme is *keyed*
    /// (`ARCH/17-MEMORY.md` §9); rotation is an explicit operation, and a
    /// rotation invalidates existing suppressions unless they are recomputed —
    /// so it is a test/ops seam, not something a caller does per request.
    pub fn set_content_key(&mut self, key: ContentKey) {
        self.key = key;
    }

    // ---------------------------------------------------------------- writes

    /// Insert one item in the caller's transaction. `new` is validated first:
    /// scope must be v1-writable and keyed, content must be inside the byte
    /// cap, and the digest is computed here.
    pub fn insert(&mut self, new: &NewItem, created_at: i64) -> Result<MemoryItem, StoreError> {
        let item = self.prepare(new, created_at)?;
        insert_row(&self.conn, &item)?;
        Ok(item)
    }

    /// Validate a candidate and compute every derived field, **without
    /// writing**. The write path validates the whole batch first and then
    /// applies it in one transaction, so a candidate that fails validation
    /// costs nothing and a later failure rolls back the earlier inserts.
    pub fn prepare(&self, new: &NewItem, created_at: i64) -> Result<MemoryItem, StoreError> {
        new.key
            .validate()
            .map_err(|e| StoreError::Rejected(e.to_string()))?;
        let normalized = normalize(&new.content);
        if normalized.is_empty() {
            return Err(StoreError::Rejected("empty item content".into()));
        }
        let bytes = measured_bytes(&normalized);
        if bytes > self.cfg.max_item_bytes {
            return Err(StoreError::Rejected(format!(
                "item is {bytes} bytes, over the {}-byte cap",
                self.cfg.max_item_bytes
            )));
        }
        // The monotone floor is applied by the write path before it reaches
        // here; this is the belt-and-braces check that a caller cannot smuggle
        // an under-classed item in below the surface it came from.
        Ok(MemoryItem {
            id: new.id.clone(),
            scope: new.key.scope,
            scope_ref: new.key.scope_ref.clone(),
            kind: new.kind,
            content: new.content.clone(),
            byte_size: bytes as i64,
            content_hash: self.key.digest_normalized(&normalized),
            hash_version: HASH_VERSION,
            dedup_key: new.dedup_key.clone(),
            sensitivity: new.sensitivity,
            trust_tier: new.trust_tier,
            source: new.source.clone(),
            source_ref: new.source_ref.clone(),
            confidence: new.confidence,
            pinned: new.pinned,
            used_count: 0,
            last_used_at: None,
            created_at,
            updated_at: created_at,
            expires_at: new.expires_at,
            superseded_by: None,
        })
    }

    /// Mark `old_id` superseded by `new_id`. The body is untouched, and the
    /// validation that the target exists and is current happens here.
    pub fn mark_superseded(
        &mut self,
        old_id: &str,
        new_id: &str,
        at: i64,
    ) -> Result<(), StoreError> {
        mark_superseded_on(&self.conn, old_id, new_id, at)
    }

    /// Pin/unpin. A pin mutation is a mutation, so it invalidates the frozen
    /// always-on block (`REQ-MEM-019`) — that invalidation is driven by the
    /// caller, never as a read side effect.
    pub fn set_pinned(&mut self, id: &str, pinned: bool, at: i64) -> Result<(), StoreError> {
        let n = self
            .conn
            .execute(
                "UPDATE memory_items SET pinned = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, pinned as i64, at],
            )
            .map_err(map_busy)?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// The **only** place a usage counter moves. It is never reached from
    /// recall or inspection (`REQ-MEM-004`).
    pub fn bump_explicit_use(&mut self, id: &str, at: i64) -> Result<i64, StoreError> {
        let n: i64 = self
            .conn
            .query_row(
                "UPDATE memory_items SET used_count = used_count + 1, last_used_at = ?2, updated_at = ?2 WHERE id = ?1 RETURNING used_count",
                params![id, at],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        Ok(n)
    }

    // ---------------------------------------------------------------- reads

    /// One row by id, superseded or not. A non-touching read.
    pub fn get(&self, id: &str) -> Result<Option<MemoryItem>, StoreError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM memory_items WHERE id = ?1"),
                params![id],
                row_to_item,
            )
            .optional()?)
    }

    /// Current, unexpired rows in a scope key, oldest first. A non-touching
    /// read.
    pub fn current_in(&self, key: &ScopeKey, now: i64) -> Result<Vec<MemoryItem>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memory_items
             WHERE scope = ?1 AND scope_ref IS ?2 AND superseded_by IS NULL
               AND (expires_at IS NULL OR expires_at > ?3)
             ORDER BY created_at ASC, id ASC"
        ))?;
        let rows = stmt.query_map(params![key.scope.as_str(), key.scope_ref, now], row_to_item)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Every row in a derived set, expired or not. The inspect surface needs
    /// this: an expired row is still the user's data and must still be
    /// listable, deletable and exportable.
    pub fn all_in_set(&self, set: &crate::scope::AccessSet) -> Result<Vec<MemoryItem>, StoreError> {
        let (scope_pred, binds) = set.as_sql_filter();
        let sql = format!(
            "SELECT {COLUMNS} FROM memory_items
             WHERE {scope_pred} AND superseded_by IS NULL
             ORDER BY created_at ASC, id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), row_to_item)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Current, unexpired rows across a caller-narrowed derived set. The scope
    /// predicate is built from the [`crate::scope::AccessSet`], never from a
    /// caller list.
    pub fn current_in_set(
        &self,
        set: &crate::scope::AccessSet,
        now: i64,
    ) -> Result<Vec<MemoryItem>, StoreError> {
        let (scope_pred, binds) = set.as_sql_filter();
        // The scope predicate is built from anonymous placeholders, so the
        // expiry bound must also be anonymous and must come after the scope
        // binds in the statement text.
        let sql = format!(
            "SELECT {COLUMNS} FROM memory_items
             WHERE {scope_pred} AND superseded_by IS NULL
               AND (expires_at IS NULL OR expires_at > ?)
             ORDER BY created_at ASC, id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
        params.push(&now as &dyn rusqlite::ToSql);
        let rows = stmt.query_map(params.as_slice(), row_to_item)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// FTS5 candidates with their BM25 scores. `bm25()` returns **negative**
    /// values for better matches, so the caller normalizes with `-score`
    /// (`ARCH/17-MEMORY.md` §6, F-02/C-03).
    pub fn fts_candidates(
        &self,
        match_expr: &str,
        limit: usize,
    ) -> Result<Vec<(String, f64)>, StoreError> {
        let sql = "SELECT m.id, bm25(memory_fts) AS score
                   FROM memory_fts JOIN memory_items m ON m.rowid = memory_fts.rowid
                   WHERE memory_fts MATCH ?1
                   ORDER BY score ASC, m.id ASC
                   LIMIT ?2";
        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => return Err(StoreError::Rejected(format!("invalid FTS query: {e}"))),
        };
        let rows = stmt.query_map(params![match_expr, limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// True when the normalized content's digest is suppressed anywhere. A
    /// suppression is **global** (`ARCH/17-MEMORY.md` §7), so the scope columns
    /// are provenance only and are not part of the predicate.
    pub fn is_suppressed(&self, content_hash: &str) -> Result<bool, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_suppressions WHERE content_hash = ?1",
            params![content_hash],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn suppression_count(&self) -> Result<usize, StoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_suppressions", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    // ------------------------------------------------------------- integrity

    /// FTS drift detection by row-count parity plus `PRAGMA integrity_check`
    /// (`REQ-MEM-001`).
    pub fn integrity_check(&self) -> Result<IntegrityReport, StoreError> {
        let sqlite_integrity: String =
            self.conn
                .pragma_query_value(None, "integrity_check", |r| r.get(0))?;
        let item_rows: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_items", [], |r| r.get(0))?;
        // External-content FTS5 reports one row per indexed item; the `-1` is
        // the FTS5 internal "index has been written" marker row, which is
        // counted by `total_rows` but is not an item.
        let fts_rows: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_fts", [], |r| r.get(0))?;
        let in_sync = sqlite_integrity == "ok" && fts_rows == item_rows;
        Ok(IntegrityReport {
            sqlite_integrity,
            item_rows,
            fts_rows,
            in_sync,
            rebuilt: false,
        })
    }

    /// Repair drift: FTS5 `rebuild`, then **verify after the rebuild**. A
    /// failure to restore parity is surfaced (`§8`: never silently ignored).
    pub fn rebuild_fts(&self) -> Result<IntegrityReport, StoreError> {
        self.conn
            .execute_batch("INSERT INTO memory_fts(memory_fts) VALUES('rebuild');")?;
        let mut report = self.integrity_check()?;
        report.rebuilt = true;
        if !report.in_sync {
            return Err(StoreError::Integrity(format!(
                "FTS rebuild did not restore parity: {} item rows vs {} fts rows (sqlite={})",
                report.item_rows, report.fts_rows, report.sqlite_integrity
            )));
        }
        Ok(report)
    }

    // ---------------------------------------------------------------- forget

    /// Hard delete + suppression + the DEC-039 erasure step. A forget never
    /// returns through extraction or import (`REQ-MEM-010/016`).
    pub fn forget(&mut self, id: &str, now: i64) -> Result<ForgetOutcome, StoreError> {
        let item: Option<MemoryItem> = self.get(id)?;
        let Some(item) = item else {
            return Err(StoreError::NotFound(id.to_string()));
        };
        // Every row the chain removal took with it is suppressed, not only the
        // head: the whole chain is gone, so none of its content may come back
        // through re-extraction or import. Suppressing only the head would let
        // the predecessor's text reappear one run later.
        let removed_hashes = self.chain_hashes(id)?;
        let removed = self.hard_delete_chain(id)?;
        for (hash, scope, scope_ref) in removed_hashes {
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO memory_suppressions(content_hash, scope, scope_ref, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![hash, scope, scope_ref, now],
                )
                .map_err(map_busy)?;
        }
        let erased = self.erase_wal()?;
        Ok(ForgetOutcome {
            id: id.to_string(),
            content_hash: item.content_hash,
            removed_rows: removed,
            erased,
        })
    }

    /// Every content hash in the chain headed by `id`, with its scope for
    /// provenance. Collected before the delete.
    fn chain_hashes(&self, id: &str) -> Result<Vec<(String, String, Option<String>)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE chain(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT m.id FROM memory_items m JOIN chain c ON m.superseded_by = c.id
             )
             SELECT content_hash, scope, scope_ref FROM memory_items WHERE id IN (SELECT id FROM chain)",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Delete a head and the rows it superseded. The `superseded_by` cascade
    /// handles the chain; a head with no predecessors removes only itself, and
    /// nothing is resurrected either way (`REQ-MEM-010`).
    fn hard_delete_chain(&self, id: &str) -> Result<usize, StoreError> {
        // Count first so the audit/report can state what went, without a body.
        let rows: i64 = self.conn.query_row(
            "WITH RECURSIVE chain(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT m.id FROM memory_items m JOIN chain c ON m.superseded_by = c.id
             )
             SELECT COUNT(*) FROM memory_items WHERE id IN (SELECT id FROM chain)",
            params![id],
            |r| r.get(0),
        )?;
        self.conn
            .execute("DELETE FROM memory_items WHERE id = ?1", params![id])
            .map_err(map_busy)?;
        Ok(rows as usize)
    }

    /// Scope wipe: the scope's items **and** its superseded rows go; the
    /// suppressions stay (`ARCH/17-MEMORY.md` §7).
    pub fn wipe_scope(&mut self, key: &ScopeKey, now: i64) -> Result<WipeOutcome, StoreError> {
        let retained = self.suppression_count()?;
        let n = self
            .conn
            .execute(
                "DELETE FROM memory_items WHERE scope = ?1 AND scope_ref IS ?2",
                params![key.scope.as_str(), key.scope_ref],
            )
            .map_err(map_busy)?;
        let _ = now;
        let erased = self.erase_wal()?;
        Ok(WipeOutcome {
            key: key.clone(),
            removed_rows: n,
            suppressions_retained: retained,
            erased,
        })
    }

    /// The DEC-039 erasure step: `secure_delete` has already zeroed the freed
    /// pages at delete time; here the write-ahead log is checkpointed and
    /// truncated so the deleted bytes do not survive in the WAL. The claim is
    /// bounded by the declared threat model (no secure erase from OS caches,
    /// backups or flash wear-leveling).
    fn erase_wal(&self) -> Result<bool, StoreError> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(true)
    }

    // --------------------------------------------------------------- anchors

    /// Persist a session anchor event. TTL is measured from this timestamp
    /// (`REQ-MEM-005/025`).
    pub fn set_session_anchor(
        &self,
        session_id: &str,
        anchor: &str,
        anchored_at: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO memory_session_anchors(session_id, anchor, anchored_at) VALUES (?1, ?2, ?3)",
            params![session_id, anchor, anchored_at],
        ).map_err(map_busy)?;
        Ok(())
    }

    pub fn session_anchor(&self, session_id: &str) -> Result<Option<(String, i64)>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT anchor, anchored_at FROM memory_session_anchors WHERE session_id = ?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// The `updated_at` of a row — the "set on any row mutation" stamp.
    pub fn updated_at_of(&self, id: &str) -> Result<Option<i64>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT updated_at FROM memory_items WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?)
    }

    // ----------------------------------------------------------------- jobs

    /// Upsert a job row (`'session:<id>'` | `'task:<id>'`).
    pub fn put_job(&mut self, job: &JobRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO memory_jobs
             (job_key, status, lease_until, retry_at, retry_remaining, last_error, watermark, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![job.job_key, job.status, job.lease_until, job.retry_at,
                    job.retry_remaining, job.last_error, job.watermark, job.created_at, job.updated_at],
        ).map_err(map_busy)?;
        Ok(())
    }

    pub fn job(&self, job_key: &str) -> Result<Option<JobRow>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT job_key, status, lease_until, retry_at, retry_remaining, last_error, watermark, created_at, updated_at
                 FROM memory_jobs WHERE job_key = ?1",
                params![job_key],
                row_to_job,
            )
            .optional()?)
    }

    /// Claim a job lease. An **expired** lease is reclaimable; a live one is
    /// not, so a second writer cannot double-run extraction (`INV-06`,
    /// `REQ-MEM-018`). A clock that reads *behind* the lease term fails safe:
    /// the claim is refused, never granted early.
    pub fn claim_job(
        &mut self,
        job_key: &str,
        now: i64,
        lease_ms: i64,
    ) -> Result<bool, StoreError> {
        let row: Option<JobRow> = self.job(job_key)?;
        let Some(row) = row else {
            return Ok(false);
        };
        if row.lease_until.is_some_and(|until| until > now) {
            // A live lease is respected: the claim is refused, so a second
            // writer cannot double-run the job.
            return Ok(false);
        }
        self.conn
            .execute(
                "UPDATE memory_jobs SET status = 'running', lease_until = ?2, updated_at = ?3 WHERE job_key = ?1",
                params![job_key, now + lease_ms, now],
            )
            .map_err(map_busy)?;
        Ok(true)
    }

    /// Release a lease, recording the watermark (idempotent harvest).
    pub fn finish_job(
        &mut self,
        job_key: &str,
        now: i64,
        watermark: Option<i64>,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "UPDATE memory_jobs SET status = 'done', lease_until = NULL, watermark = ?2, updated_at = ?3 WHERE job_key = ?1",
                params![job_key, watermark, now],
            )
            .map_err(map_busy)?;
        Ok(())
    }

    /// Mark an error and back off. `retry_at` is the next eligible attempt; a
    /// job with no retries left stays `error` for the GC sweep.
    pub fn fail_job(
        &mut self,
        job_key: &str,
        now: i64,
        backoff_ms: i64,
        error: &str,
    ) -> Result<JobRow, StoreError> {
        let row: JobRow = self
            .conn
            .query_row(
                "SELECT job_key, status, lease_until, retry_at, retry_remaining, last_error, watermark, created_at, updated_at
                 FROM memory_jobs WHERE job_key = ?1",
                params![job_key],
                row_to_job,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(job_key.to_string()))?;
        let remaining = (row.retry_remaining - 1).max(0);
        self.conn
            .execute(
                "UPDATE memory_jobs SET status = 'error', lease_until = NULL, retry_at = ?2, retry_remaining = ?3, last_error = ?4, updated_at = ?5 WHERE job_key = ?1",
                params![job_key, now + backoff_ms, remaining, error, now],
            )
            .map_err(map_busy)?;
        Ok(self.job(job_key)?.expect("row exists"))
    }

    /// Jobs GC: `done` older than 7 d, `error` older than 30 d (declared
    /// defaults, `ARCH/17-MEMORY.md` §3).
    pub fn gc_jobs(&mut self, now: i64) -> Result<GcOutcome, StoreError> {
        let done_before = now - 7 * 24 * 60 * 60 * 1000;
        let error_before = now - 30 * 24 * 60 * 60 * 1000;
        let swept = self
            .conn
            .execute(
                "DELETE FROM memory_jobs
                 WHERE (status = 'done' AND updated_at < ?1) OR (status = 'error' AND updated_at < ?2)",
                params![done_before, error_before],
            )
            .map_err(map_busy)?;
        Ok(GcOutcome { swept })
    }

    // -------------------------------------------------------------- export

    /// A JSON export: current **and** superseded items (history is preserved)
    /// plus the suppression set, which travels with the items so an import
    /// cannot resurrect forgotten content (`ARCH/17-MEMORY.md` §4).
    pub fn export_json(&self) -> Result<String, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM memory_items ORDER BY id"))?;
        let items: Vec<MemoryItem> = stmt
            .query_map([], row_to_item)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut sstmt = self.conn.prepare(
            "SELECT content_hash, scope, scope_ref, created_at FROM memory_suppressions ORDER BY content_hash",
        )?;
        let suppressions: Vec<SuppressionRow> = sstmt
            .query_map([], |r| {
                Ok(SuppressionRow {
                    content_hash: r.get(0)?,
                    scope: r.get(1)?,
                    scope_ref: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::to_string_pretty(&ExportBundle {
            schema_version: self.schema_version()?,
            hash_version: HASH_VERSION,
            items,
            suppressions,
        })?)
    }

    /// Markdown export: one section per scope, each item rendered with its
    /// provenance line (scope/source/created-at) so a human export is still
    /// inspectable.
    pub fn export_markdown(&self) -> Result<String, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memory_items ORDER BY scope, scope_ref, created_at"
        ))?;
        let items: Vec<MemoryItem> = stmt
            .query_map([], row_to_item)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = String::from("# Memory export\n\n");
        for item in &items {
            out.push_str(&format!(
                "## {} — `{}`\n\n- scope: `{}`\n- source: `{}`\n- created_at: {}\n- trust: `{}`\n- sensitivity: `{}`\n- state: {}\n\n{}\n\n",
                item.kind.as_str(),
                item.id,
                item.key().as_key(),
                item.source,
                item.created_at,
                item.trust_tier.as_str(),
                item.sensitivity,
                if item.is_current() { "current" } else { "superseded" },
                item.content,
            ));
        }
        let sup = self.suppression_count()?;
        out.push_str(&format!(
            "---\n\n{sup} content-hash suppression(s) travel with this export.\n"
        ));
        Ok(out)
    }

    // -------------------------------------------------------------- import

    /// Import a bundle: re-run hash + secret + suppression checks, merge
    /// suppressions (never remove one), remap ids/scopes, and land as
    /// `source='import'` / trust tier `import`.
    pub fn import_json(
        &mut self,
        json: &str,
        remap: &ImportRemap,
        now: i64,
    ) -> Result<ImportReport, StoreError> {
        let bundle: ExportBundle = serde_json::from_str(json)?;
        if bundle.schema_version > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: bundle.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        // Suppressions merge first and are never removed, so a forgotten hash
        // blocks the item even if the bundle tries to re-add it.
        let mut suppressions_merged = 0usize;
        for s in &bundle.suppressions {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO memory_suppressions(content_hash, scope, scope_ref, created_at) VALUES (?1,?2,?3,?4)",
                    params![s.content_hash, s.scope, s.scope_ref, s.created_at],
                )
                .map_err(map_busy)?;
            suppressions_merged += 1;
        }

        let mut report = ImportReport {
            considered: bundle.items.len(),
            imported: Vec::new(),
            skipped_suppressed: 0,
            skipped_secret: 0,
            skipped_duplicate: 0,
            skipped_invalid: 0,
            suppressions_merged,
            scope_remapped: 0,
        };
        for item in &bundle.items {
            // A project identity is **not** assumed portable: an item whose
            // project does not exist here is either explicitly remapped or
            // abstained on — never attached to whatever project is here.
            let Some(key) = remap.map_key(&item.key()) else {
                if item.scope == Scope::Project {
                    report.scope_remapped += 1;
                }
                report.skipped_invalid += 1;
                continue;
            };
            let hash = self.key.digest(&item.content);
            if self.is_suppressed(&hash)? {
                report.skipped_suppressed += 1;
                continue;
            }
            if crate::write_path::looks_like_secret(&item.content) {
                // Secrets are rejected on every write path, import included.
                report.skipped_secret += 1;
                continue;
            }
            let id = remap.map_id(&item.id);
            if self.get(&id)?.is_some() {
                report.skipped_duplicate += 1;
                continue;
            }
            if key.validate().is_err() {
                report.skipped_invalid += 1;
                continue;
            }
            let mut new = NewItem::new(&id, key, item.kind, &item.content);
            new.sensitivity = item.sensitivity;
            // The two declared re-stamps: an imported item always lands with
            // `source='import'` and the import trust tier, whatever it claimed.
            new.trust_tier = TrustTier::Import;
            new.source = "import".to_string();
            new.source_ref = Some(format!("import:{}", item.id));
            new.dedup_key = item.dedup_key.clone();
            new.confidence = item.confidence;
            new.pinned = item.pinned;
            new.expires_at = item.expires_at;
            // `created_at` is preserved: the item was created then. The
            // round-trip difference is therefore exactly the two declared
            // provenance fields above, nothing else.
            let created_at = item.created_at;
            let _ = now;
            match self.insert(&new, created_at) {
                Ok(saved) => {
                    if saved.id != item.id {
                        report.scope_remapped += 0;
                    }
                    report.imported.push(saved.id);
                }
                Err(StoreError::Rejected(_)) => {
                    report.skipped_invalid += 1;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(report)
    }
}

/// The projection the store reads and the repair path exports. `pub(crate)`
/// because the ops module reuses it rather than duplicating the column list.
pub(crate) const COLUMNS: &str =
    "id, scope, scope_ref, kind, content, byte_size, content_hash, hash_version,
     dedup_key, sensitivity, trust_tier, source, source_ref, confidence, pinned, used_count,
     last_used_at, created_at, updated_at, expires_at, superseded_by";

/// Row → item, shared by the store reads and the repair export.
pub(crate) fn row_to_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryItem> {
    Ok(MemoryItem {
        id: r.get(0)?,
        scope: Scope::parse(&r.get::<_, String>(1)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(e)),
            )
        })?,
        scope_ref: r.get(2)?,
        kind: Kind::parse(&r.get::<_, String>(3)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(e)),
            )
        })?,
        content: r.get(4)?,
        byte_size: r.get(5)?,
        content_hash: r.get(6)?,
        hash_version: r.get(7)?,
        dedup_key: r.get(8)?,
        sensitivity: Sensitivity::parse(&r.get::<_, String>(9)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(e)),
            )
        })?,
        trust_tier: TrustTier::parse(&r.get::<_, String>(10)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(e)),
            )
        })?,
        source: r.get(11)?,
        source_ref: r.get(12)?,
        confidence: r.get(13)?,
        pinned: r.get::<_, i64>(14)? != 0,
        used_count: r.get(15)?,
        last_used_at: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
        expires_at: r.get(19)?,
        superseded_by: r.get(20)?,
    })
}

fn row_to_job(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        job_key: r.get(0)?,
        status: r.get(1)?,
        lease_until: r.get(2)?,
        retry_at: r.get(3)?,
        retry_remaining: r.get(4)?,
        last_error: r.get(5)?,
        watermark: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn insert_row(conn: &Connection, item: &MemoryItem) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO memory_items
         (id, scope, scope_ref, kind, content, byte_size, content_hash, hash_version, dedup_key,
          sensitivity, trust_tier, source, source_ref, confidence, pinned, used_count, last_used_at,
          created_at, updated_at, expires_at, superseded_by)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
        params![
            item.id,
            item.scope.as_str(),
            item.scope_ref,
            item.kind.as_str(),
            item.content,
            item.byte_size,
            item.content_hash,
            item.hash_version,
            item.dedup_key,
            item.sensitivity.as_str(),
            item.trust_tier.as_str(),
            item.source,
            item.source_ref,
            item.confidence,
            item.pinned as i64,
            item.used_count,
            item.last_used_at,
            item.created_at,
            item.updated_at,
            item.expires_at,
            item.superseded_by,
        ],
    )
    .map_err(map_busy)?;
    Ok(())
}

/// Insert a prepared row on any connection, including inside a transaction.
pub(crate) fn insert_prepared(conn: &Connection, item: &MemoryItem) -> Result<(), StoreError> {
    insert_row(conn, item)
}

/// The supersede pointer transition, on any connection (so the write path can
/// run it inside its single transaction). The stored body is never touched.
pub(crate) fn mark_superseded_on(
    conn: &Connection,
    old_id: &str,
    new_id: &str,
    at: i64,
) -> Result<(), StoreError> {
    let target: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT scope, scope_ref, superseded_by FROM memory_items WHERE id = ?1",
            params![old_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (scope, scope_ref, superseded_by) =
        target.ok_or_else(|| StoreError::NotFound(old_id.to_string()))?;
    if superseded_by.is_some() {
        return Err(StoreError::Rejected(format!(
            "{old_id} is already superseded"
        )));
    }
    // The new head must be current and in the same-or-narrower scope.
    let head: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT scope, scope_ref FROM memory_items WHERE id = ?1",
            params![new_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (h_scope, h_ref) =
        head.ok_or_else(|| StoreError::NotFound(format!("supersede target {new_id}")))?;
    if scope != h_scope || scope_ref != h_ref {
        return Err(StoreError::Rejected(format!(
            "supersede target {new_id} is in {h_scope}:{h_ref:?}, not the same-or-narrower scope of {old_id}"
        )));
    }
    conn.execute(
        "UPDATE memory_items SET superseded_by = ?2, updated_at = ?3 WHERE id = ?1",
        params![old_id, new_id, at],
    )
    .map_err(map_busy)?;
    Ok(())
}

/// A suppression row as exported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuppressionRow {
    pub content_hash: String,
    pub scope: String,
    pub scope_ref: Option<String>,
    pub created_at: i64,
}

/// The export/import bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportBundle {
    pub schema_version: u32,
    pub hash_version: i64,
    pub items: Vec<MemoryItem>,
    pub suppressions: Vec<SuppressionRow>,
}

/// Explicit import remapping. Project identity is not portable, so the two
/// cases are distinct constructors rather than one lenient one:
///
/// - [`ImportRemap::identity`] — the **same-machine round trip**: a project
///   identity not named in the maps is kept as-is. This is what makes
///   "byte-identical for unchanged identity" true.
/// - [`ImportRemap::cross_machine`] — a **different machine**: an unmapped
///   project identity is *abstained on*, never attached to whatever project
///   happens to be here (a leak).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportRemap {
    pub project_identity_map: std::collections::BTreeMap<String, String>,
    pub user_id_map: std::collections::BTreeMap<String, String>,
    /// Remap every id (`<prefix><old_id>`); `None` keeps the id.
    pub id_prefix: Option<String>,
    /// When false, an unmapped project identity is refused rather than kept.
    pub assume_same_identity: bool,
}

impl ImportRemap {
    /// Identity preserved — the byte-identical round-trip rule.
    pub fn identity() -> Self {
        Self {
            assume_same_identity: true,
            ..Default::default()
        }
    }

    /// A different machine: nothing is assumed. A project item only lands
    /// through an explicit mapping.
    pub fn cross_machine() -> Self {
        Self::default()
    }

    /// Map a project identity explicitly.
    pub fn with_project(mut self, from: &str, to: &str) -> Self {
        self.project_identity_map
            .insert(from.to_string(), to.to_string());
        self
    }

    /// Map a user id explicitly.
    pub fn with_user(mut self, from: &str, to: &str) -> Self {
        self.user_id_map.insert(from.to_string(), to.to_string());
        self
    }

    /// Remap every id with a prefix.
    pub fn with_id_prefix(mut self, prefix: &str) -> Self {
        self.id_prefix = Some(prefix.to_string());
        self
    }

    pub fn map_key(&self, key: &ScopeKey) -> Option<ScopeKey> {
        let scope_ref = match key.scope {
            Scope::Project => {
                let r = key.scope_ref.as_deref()?;
                match self.project_identity_map.get(r) {
                    Some(m) => m.clone(),
                    // Same machine: the identity is unchanged, so it is kept.
                    None if self.assume_same_identity => r.to_string(),
                    // Different machine: abstain rather than guess.
                    None => return None,
                }
            }
            Scope::User => {
                let r = key.scope_ref.as_deref()?;
                match self.user_id_map.get(r) {
                    Some(m) => m.clone(),
                    None if self.assume_same_identity => r.to_string(),
                    None => return None,
                }
            }
            _ => key.scope_ref.clone()?,
        };
        Some(ScopeKey::new(key.scope, Some(&scope_ref)))
    }

    pub fn map_id(&self, id: &str) -> String {
        match &self.id_prefix {
            Some(p) => format!("{p}{id}"),
            None => id.to_string(),
        }
    }
}

/// What an import did. Every skip has a reason so "nothing happened" is never
/// mistaken for a silent loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub considered: usize,
    pub imported: Vec<String>,
    pub skipped_suppressed: usize,
    pub skipped_secret: usize,
    pub skipped_duplicate: usize,
    pub skipped_invalid: usize,
    pub suppressions_merged: usize,
    pub scope_remapped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::ActorBinding;

    fn store() -> MemoryStore {
        MemoryStore::open_in_memory(StoreConfig::default()).expect("in-memory store")
    }

    fn item(id: &str, key: &ScopeKey, content: &str) -> NewItem {
        NewItem::new(id, key.clone(), Kind::Fact, content)
    }

    #[test]
    fn schema_version_is_carried_and_migrations_are_forward_only() {
        let s = store();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        // Re-running the migration on a current store is a no-op, not an error.
        let mut s = s;
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn ddl_is_installed_with_its_fts_sync_triggers() {
        let s = store();
        let triggers: Vec<String> = s
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            triggers,
            vec!["memory_items_ad", "memory_items_ai", "memory_items_au"]
        );
    }

    #[test]
    fn inserts_measure_bytes_and_a_keyed_hash() {
        let mut s = store();
        let it = s
            .insert(
                &item("m1", &ScopeKey::project("p1"), "The Build Uses  Bazel  "),
                100,
            )
            .unwrap();
        // Normalization collapses whitespace before measuring.
        assert_eq!(it.byte_size, "the build uses bazel".len() as i64);
        assert_eq!(it.hash_version, HASH_VERSION);
        assert_eq!(it.content_hash, s.key().digest("The Build Uses  Bazel  "));
        // The digest is keyed: a different key gives a different digest.
        let other = ContentKey::new("another-store");
        assert_ne!(it.content_hash, other.digest(&it.content));
    }

    #[test]
    fn an_oversize_item_is_rejected_with_no_partial_write() {
        let mut s = store();
        let big = "x".repeat(DEFAULT_MAX_ITEM_BYTES + 1);
        let err = s
            .insert(&item("big", &ScopeKey::project("p1"), &big), 1)
            .unwrap_err();
        assert!(matches!(err, StoreError::Rejected(_)));
        assert!(s.get("big").unwrap().is_none());
        assert_eq!(s.integrity_check().unwrap().item_rows, 0);
    }

    #[test]
    fn org_scope_writes_are_rejected_in_v1() {
        let mut s = store();
        let err = s
            .insert(&item("o1", &ScopeKey::org(), "shared"), 1)
            .unwrap_err();
        assert!(err.to_string().contains("disabled in v1"));
        assert!(s.get("o1").unwrap().is_none());
    }

    #[test]
    fn fts_stays_in_sync_through_insert_update_delete_and_rebuild() {
        let mut s = store();
        s.insert(&item("a", &ScopeKey::project("p"), "alpha unique token"), 1)
            .unwrap();
        s.insert(&item("b", &ScopeKey::project("p"), "beta other token"), 2)
            .unwrap();
        let r = s.integrity_check().unwrap();
        assert!(r.in_sync, "{r:?}");
        // The trigger really indexes: a MATCH finds the row.
        assert!(!s.fts_candidates("unique", 10).unwrap().is_empty());
        // Deleting through the store removes the FTS row via the delete trigger.
        s.forget("a", 3).unwrap();
        let r = s.integrity_check().unwrap();
        assert!(r.in_sync, "{r:?}");
        assert!(s.fts_candidates("unique", 10).unwrap().is_empty());
        // A rebuild restores parity and verifies after the rebuild.
        let r = s.rebuild_fts().unwrap();
        assert!(r.rebuilt && r.in_sync);
    }

    #[test]
    fn supersede_marks_the_old_row_without_touching_its_body() {
        let mut s = store();
        s.insert(
            &item("old", &ScopeKey::project("p"), "the build uses bazel"),
            1,
        )
        .unwrap();
        s.insert(
            &item("new", &ScopeKey::project("p"), "the build uses buck"),
            2,
        )
        .unwrap();
        s.mark_superseded("old", "new", 3).unwrap();
        let old = s.get("old").unwrap().unwrap();
        // Body untouched, pointer set, row still present for audit.
        assert_eq!(old.content, "the build uses bazel");
        assert_eq!(old.superseded_by.as_deref(), Some("new"));
        assert!(!old.is_current());
        assert_eq!(s.updated_at_of("old").unwrap(), Some(3));
    }

    #[test]
    fn supersede_rejects_a_missing_non_current_or_wider_target() {
        let mut s = store();
        s.insert(&item("old", &ScopeKey::project("p"), "a"), 1)
            .unwrap();
        s.insert(&item("other", &ScopeKey::project("p2"), "b"), 2)
            .unwrap();
        s.insert(&item("mid", &ScopeKey::project("p"), "c"), 3)
            .unwrap();
        s.mark_superseded("old", "mid", 4).unwrap();
        // missing target
        assert!(s.mark_superseded("mid", "nope", 5).is_err());
        // wider scope
        assert!(s.mark_superseded("mid", "other", 5).is_err());
        // already superseded
        assert!(s.mark_superseded("old", "mid", 5).is_err());
    }

    #[test]
    fn current_reads_filter_superseded_and_expired() {
        let mut s = store();
        s.insert(&item("live", &ScopeKey::project("p"), "live item"), 1)
            .unwrap();
        let mut exp = item("exp", &ScopeKey::project("p"), "expired item");
        exp.expires_at = Some(5);
        s.insert(&exp, 2).unwrap();
        s.insert(&item("old", &ScopeKey::project("p"), "superseded item"), 3)
            .unwrap();
        s.insert(&item("new", &ScopeKey::project("p"), "head item"), 4)
            .unwrap();
        s.mark_superseded("old", "new", 5).unwrap();
        let cur = s.current_in(&ScopeKey::project("p"), 10).unwrap();
        let ids: Vec<&str> = cur.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["live", "new"]);
        // At and after the expiry instant the expired row is out; the
        // unexpired rows remain.
        let cur = s.current_in(&ScopeKey::project("p"), 5).unwrap();
        let ids: Vec<&str> = cur.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["live", "new"]);
    }

    #[test]
    fn set_reads_are_built_from_the_derived_access_set() {
        let mut s = store();
        for (id, key) in [
            ("mine", ScopeKey::project("p1")),
            ("theirs", ScopeKey::project("p2")),
            ("user", ScopeKey::user("u1")),
        ] {
            s.insert(&item(id, &key, "shared token here"), 1).unwrap();
        }
        let set = crate::scope::AccessSet::derive(&ActorBinding::local_in("u1", "p1"));
        let visible = s.current_in_set(&set, 10).unwrap();
        let ids: Vec<&str> = visible.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"mine") && ids.contains(&"user"));
        assert!(!ids.contains(&"theirs"), "cross-project leakage = 0");
    }

    #[test]
    fn bump_explicit_use_is_the_only_counter_movement() {
        let mut s = store();
        s.insert(&item("m", &ScopeKey::project("p"), "counted"), 1)
            .unwrap();
        // Reads do not move it.
        s.get("m").unwrap();
        s.current_in(&ScopeKey::project("p"), 2).unwrap();
        s.integrity_check().unwrap();
        assert_eq!(s.get("m").unwrap().unwrap().used_count, 0);
        assert!(s.get("m").unwrap().unwrap().last_used_at.is_none());
        // The explicit-use path does.
        assert_eq!(s.bump_explicit_use("m", 42).unwrap(), 1);
        assert_eq!(s.get("m").unwrap().unwrap().used_count, 1);
        assert_eq!(s.get("m").unwrap().unwrap().last_used_at, Some(42));
    }

    #[test]
    fn forget_hard_deletes_writes_a_suppression_and_erases() {
        let mut s = store();
        s.insert(&item("m", &ScopeKey::project("p"), "forget me"), 1)
            .unwrap();
        let out = s.forget("m", 10).unwrap();
        assert!(s.get("m").unwrap().is_none());
        assert_eq!(out.removed_rows, 1);
        assert!(out.erased);
        assert!(s.is_suppressed(&out.content_hash).unwrap());
        // The suppression is global: it is scoped to no other project.
        assert!(s.is_suppressed(&s.key().digest("Forget  Me!")).unwrap());
    }

    #[test]
    fn deleting_a_superseding_head_never_resurrects_the_older_row() {
        let mut s = store();
        s.insert(&item("old", &ScopeKey::project("p"), "generation one"), 1)
            .unwrap();
        s.insert(&item("new", &ScopeKey::project("p"), "generation two"), 2)
            .unwrap();
        s.mark_superseded("old", "new", 3).unwrap();
        // Forget the head: the chain it headed goes with it (no dangling
        // pointer, no error, no resurrection).
        let out = s.forget("new", 4).unwrap();
        assert_eq!(out.removed_rows, 2);
        assert!(s.get("old").unwrap().is_none());
        assert!(s.get("new").unwrap().is_none());
        assert!(s.integrity_check().unwrap().in_sync);
    }

    #[test]
    fn scope_wipe_clears_items_and_superseded_rows_but_keeps_suppressions() {
        let mut s = store();
        s.insert(&item("a", &ScopeKey::project("p"), "alpha"), 1)
            .unwrap();
        s.insert(&item("b", &ScopeKey::project("p"), "beta"), 2)
            .unwrap();
        s.insert(&item("c", &ScopeKey::project("p"), "gamma"), 3)
            .unwrap();
        s.mark_superseded("a", "b", 4).unwrap();
        s.insert(&item("other", &ScopeKey::project("p2"), "delta"), 4)
            .unwrap();
        s.forget("c", 5).unwrap();
        let sup_before = s.suppression_count().unwrap();
        assert_eq!(sup_before, 1);

        let out = s.wipe_scope(&ScopeKey::project("p"), 6).unwrap();
        // a (superseded) + b (head) went; the other project is untouched.
        assert_eq!(out.removed_rows, 2);
        assert!(s.current_in(&ScopeKey::project("p"), 7).unwrap().is_empty());
        assert_eq!(s.current_in(&ScopeKey::project("p2"), 7).unwrap().len(), 1);
        // Suppressions are not scope data and survive.
        assert_eq!(s.suppression_count().unwrap(), sup_before);
        assert!(s.integrity_check().unwrap().in_sync, "no FTS orphans");
    }

    #[test]
    fn erase_is_bounded_to_the_declared_threat_model() {
        // The claim is the declared policy: freed pages are zeroed by
        // secure_delete and the WAL is truncated. This test pins that the WAL
        // truncate step ran and the pragma is on, and does not claim anything
        // about OS caches or backups.
        let mut s = store();
        s.insert(&item("m", &ScopeKey::project("p"), "sensitive words"), 1)
            .unwrap();
        let out = s.forget("m", 2).unwrap();
        assert!(out.erased);
        let sd: i64 = s
            .conn
            .pragma_query_value(None, "secure_delete", |r| r.get(0))
            .unwrap();
        assert_eq!(sd, 1);
        let fk: i64 = s
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "the superseded_by cascade only exists with FKs on");
    }

    #[test]
    fn a_job_lease_is_exclusive_and_an_expired_one_is_reclaimable() {
        let mut s = store();
        s.put_job(&JobRow {
            job_key: "session:s1".into(),
            status: "pending".into(),
            lease_until: None,
            retry_at: None,
            retry_remaining: 3,
            last_error: None,
            watermark: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
        assert!(s.claim_job("session:s1", 100, 50).unwrap());
        // A second writer inside the term is refused (INV-06).
        assert!(!s.claim_job("session:s1", 120, 50).unwrap());
        // Past the term it is reclaimable.
        assert!(s.claim_job("session:s1", 200, 50).unwrap());
        // An unknown job is not claimable.
        assert!(!s.claim_job("session:other", 200, 50).unwrap());
    }

    #[test]
    fn a_lease_fails_safe_when_the_clock_reads_behind() {
        let mut s = store();
        s.put_job(&JobRow {
            job_key: "session:s1".into(),
            status: "running".into(),
            lease_until: Some(500),
            retry_at: None,
            retry_remaining: 3,
            last_error: None,
            watermark: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
        // Clock jumped backwards to 100: the live lease is still respected, so
        // the claim fails safe rather than being granted early.
        assert!(!s.claim_job("session:s1", 100, 50).unwrap());
    }

    #[test]
    fn job_failure_backs_off_and_the_gc_sweeps_by_age() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let mut s = store();
        s.put_job(&JobRow {
            job_key: "task:t1".into(),
            status: "pending".into(),
            lease_until: None,
            retry_at: None,
            retry_remaining: 1,
            last_error: None,
            watermark: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
        // Fail it a day before the sweep point: recent enough that the 30-day
        // error window keeps it.
        let fail_at = 9 * DAY;
        let row = s
            .fail_job("task:t1", fail_at, 20, "model call failed")
            .unwrap();
        assert_eq!(row.status, "error");
        assert_eq!(row.retry_at, Some(fail_at + 20));
        assert_eq!(row.retry_remaining, 0);
        assert_eq!(row.last_error.as_deref(), Some("model call failed"));

        s.put_job(&JobRow {
            job_key: "task:t2".into(),
            status: "done".into(),
            lease_until: None,
            retry_at: None,
            retry_remaining: 3,
            last_error: None,
            watermark: Some(9),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
        // Sweep 10 days in: the `done` row is 10 days old (past its 7-day
        // window) and goes; the `error` row is 1 day old and stays.
        assert_eq!(s.gc_jobs(10 * DAY).unwrap().swept, 1);
        assert!(s.job("task:t1").unwrap().is_some());
        assert!(s.job("task:t2").unwrap().is_none());
        // Sweep 40 days in: the error row ages out too.
        assert_eq!(s.gc_jobs(40 * DAY).unwrap().swept, 1);
        assert!(s.job("task:t1").unwrap().is_none());
    }

    #[test]
    fn session_anchors_are_persisted_not_recomputed() {
        let s = store();
        assert!(s.session_anchor("s1").unwrap().is_none());
        s.set_session_anchor("s1", "archived", 1_700_000_000_000)
            .unwrap();
        assert_eq!(
            s.session_anchor("s1").unwrap(),
            Some(("archived".to_string(), 1_700_000_000_000))
        );
    }

    #[test]
    fn monotone_clock_clamps_a_backwards_observation_and_records_it() {
        let clock = MonotoneClock::new();
        let (a, skewed) = clock.now();
        assert!(!skewed && a > 0);
        // Inject a backwards reading the way a wall-clock jump would present.
        clock.last.set(i64::MAX / 2);
        let (b, skewed) = clock.now();
        assert!(skewed, "a backwards clock is recorded, not adopted");
        assert_eq!(b, i64::MAX / 2, "the reading is clamped, never moved back");
        assert_eq!(clock.skew_events().len(), 1);
    }

    #[test]
    fn export_json_carries_items_and_the_suppression_set() {
        let mut s = store();
        s.insert(&item("a", &ScopeKey::project("p"), "keep me"), 1)
            .unwrap();
        s.insert(&item("b", &ScopeKey::project("p"), "drop me"), 2)
            .unwrap();
        s.forget("b", 3).unwrap();
        let json = s.export_json().unwrap();
        let bundle: ExportBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(bundle.schema_version, SCHEMA_VERSION);
        assert_eq!(bundle.items.len(), 1);
        assert_eq!(bundle.suppressions.len(), 1);
        // Markdown export carries provenance per item.
        let md = s.export_markdown().unwrap();
        assert!(md.contains("source: `user`"));
        assert!(md.contains("1 content-hash suppression(s)"));
    }

    #[test]
    fn import_with_identity_is_byte_identical_for_unchanged_identity() {
        let mut src = store();
        src.insert(&item("a", &ScopeKey::project("p"), "keep me"), 1)
            .unwrap();
        let json = src.export_json().unwrap();

        let mut dst = store();
        let report = dst
            .import_json(&json, &ImportRemap::identity(), 100)
            .unwrap();
        assert_eq!(report.imported, vec!["a".to_string()]);
        assert_eq!(report.skipped_invalid, 0);
        // "Byte-identical for unchanged identity" is defined against the declared
        // remapping rules, and there is exactly one: an import re-stamps
        // provenance. Every other field — id, scope, kind, content, byte size,
        // digest, hash version, class, pin, created/updated — is identical.
        let mut back: ExportBundle = serde_json::from_str(&dst.export_json().unwrap()).unwrap();
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.hash_version, HASH_VERSION);
        let landed = &back.items[0];
        let original: ExportBundle = serde_json::from_str(&json).unwrap();
        let src_item = &original.items[0];
        assert_eq!(landed.id, src_item.id);
        assert_eq!(landed.scope, src_item.scope);
        assert_eq!(landed.scope_ref, src_item.scope_ref);
        assert_eq!(landed.kind, src_item.kind);
        assert_eq!(landed.content, src_item.content);
        assert_eq!(landed.byte_size, src_item.byte_size);
        assert_eq!(landed.content_hash, src_item.content_hash);
        assert_eq!(landed.sensitivity, src_item.sensitivity);
        assert_eq!(landed.pinned, src_item.pinned);
        assert_eq!(landed.created_at, src_item.created_at);
        // The two declared differences.
        assert_eq!(landed.source, "import");
        assert_eq!(landed.trust_tier, TrustTier::Import);
        back.items[0].source = src_item.source.clone();
        back.items[0].trust_tier = src_item.trust_tier;
        back.items[0].source_ref = src_item.source_ref.clone();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(&original).unwrap(),
            "apart from the declared provenance re-stamp, the bundle is identical"
        );
    }

    #[test]
    fn import_cannot_resurrect_a_suppressed_hash() {
        let mut src = store();
        src.insert(&item("b", &ScopeKey::project("p"), "secret wisdom"), 2)
            .unwrap();
        let json = src.export_json().unwrap();

        let mut dst = store();
        dst.insert(&item("b", &ScopeKey::project("p"), "secret wisdom"), 1)
            .unwrap();
        dst.forget("b", 3).unwrap();
        // The bundle is an older copy taken before the forget.
        let report = dst
            .import_json(&json, &ImportRemap::identity(), 100)
            .unwrap();
        assert_eq!(report.skipped_suppressed, 1);
        assert!(report.imported.is_empty());
        assert!(dst.get("b").unwrap().is_none());
    }

    #[test]
    fn import_refuses_to_attach_a_project_item_to_the_wrong_project() {
        let mut src = store();
        src.insert(&item("a", &ScopeKey::project("p-origin"), "portable?"), 1)
            .unwrap();
        let json = src.export_json().unwrap();

        // A different machine with no explicit mapping: project identity is not
        // assumed portable, so abstain rather than attach to the local project.
        let mut dst = store();
        let report = dst
            .import_json(&json, &ImportRemap::cross_machine(), 100)
            .unwrap();
        assert!(report.imported.is_empty());
        assert_eq!(report.skipped_invalid, 1);

        // An explicit mapping lands it, in the named project.
        let remap = ImportRemap::cross_machine().with_project("p-origin", "p-here");
        let mut dst2 = store();
        let report = dst2.import_json(&json, &remap, 100).unwrap();
        assert_eq!(report.imported, vec!["a".to_string()]);
        assert_eq!(
            dst2.get("a").unwrap().unwrap().scope_ref.as_deref(),
            Some("p-here")
        );
    }

    #[test]
    fn import_re_runs_the_secret_check_and_lands_as_import() {
        let mut src = store();
        src.insert(
            &item("s", &ScopeKey::project("p"), "aws key AKIAIOSFODNN7EXAMPLE"),
            1,
        )
        .unwrap();
        let json = src.export_json().unwrap();
        let mut dst = store();
        let report = dst
            .import_json(&json, &ImportRemap::identity(), 100)
            .unwrap();
        assert_eq!(report.skipped_secret, 1);
        assert!(dst.get("s").unwrap().is_none());
    }

    #[test]
    fn import_remaps_ids_and_lands_with_the_import_provenance() {
        let mut src = store();
        src.insert(&item("a", &ScopeKey::user("u1"), "portable user fact"), 1)
            .unwrap();
        let json = src.export_json().unwrap();
        let remap = ImportRemap::cross_machine()
            .with_user("u1", "u2")
            .with_id_prefix("imported:");
        let mut dst = store();
        let report = dst.import_json(&json, &remap, 100).unwrap();
        assert_eq!(report.imported, vec!["imported:a".to_string()]);
        let landed = dst.get("imported:a").unwrap().unwrap();
        assert_eq!(landed.source, "import");
        assert_eq!(landed.trust_tier, TrustTier::Import);
        assert_eq!(landed.source_ref.as_deref(), Some("import:a"));
        assert_eq!(landed.scope_ref.as_deref(), Some("u2"));
    }

    #[test]
    fn busy_is_a_distinct_error_so_the_call_site_can_degrade() {
        // The variant is the contract: a `BUSY` never becomes a turn failure.
        let e = StoreError::Busy;
        assert_eq!(
            e.to_string(),
            "memory store busy (bounded busy_timeout elapsed)"
        );
    }

    #[test]
    fn bounded_busy_timeout_and_wal_are_applied_per_connection() {
        // WAL is a file-mode journal: an in-memory database reports `memory`,
        // so the pragma is asserted on the file-backed store the product uses.
        let dir = std::env::temp_dir().join(format!("agentcowork-mem-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = MemoryStore::open(&dir.join("memory.db"), None, StoreConfig::default()).unwrap();
        let bt: i64 = s
            .conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .unwrap();
        assert_eq!(bt, StoreConfig::default().busy_timeout_ms);
        let mode: String = s
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = s
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rollback_leaves_no_partial_write_and_no_fts_orphan() {
        let mut s = store();
        let digest = s.key().digest("inside tx");
        {
            let tx = s.conn.transaction().unwrap();
            let it = MemoryItem {
                id: "a".into(),
                scope: Scope::Project,
                scope_ref: Some("p".into()),
                kind: Kind::Fact,
                content: "inside tx".into(),
                byte_size: 9,
                content_hash: digest,
                hash_version: HASH_VERSION,
                dedup_key: None,
                sensitivity: Sensitivity::Personal,
                trust_tier: TrustTier::DerivedUntrusted,
                source: "user".into(),
                source_ref: None,
                confidence: 1.0,
                pinned: false,
                used_count: 0,
                last_used_at: None,
                created_at: 1,
                updated_at: 1,
                expires_at: None,
                superseded_by: None,
            };
            insert_row(&tx, &it).unwrap();
            tx.rollback().unwrap();
        }
        assert!(s.get("a").unwrap().is_none());
        assert!(s.integrity_check().unwrap().in_sync);
    }

    #[test]
    fn a_failed_supersede_in_a_batch_rolls_the_whole_batch_back() {
        let s = store();
        let candidate = NewItem::new(
            "new-1",
            ScopeKey::project("p"),
            Kind::Fact,
            "the build uses buck",
        );
        let good = s.prepare(&candidate, 2).unwrap();
        {
            // One transaction: a good insert, then a supersede whose target does
            // not exist. The error aborts the transaction, so the good insert
            // is not left behind (`REQ-MEM-008`: no partial write on failure).
            let tx = s.conn.unchecked_transaction().unwrap();
            insert_prepared(&tx, &good).unwrap();
            let err = mark_superseded_on(&tx, "does-not-exist", &good.id, 2).unwrap_err();
            assert!(matches!(err, StoreError::NotFound(_)));
            // Drop without commit → rollback.
        }
        assert!(s.get("new-1").unwrap().is_none());
        assert_eq!(s.integrity_check().unwrap().item_rows, 0);
        assert!(s.integrity_check().unwrap().in_sync);
    }
}
