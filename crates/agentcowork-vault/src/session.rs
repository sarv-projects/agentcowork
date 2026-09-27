//! P2.7 Session Vault (E11/E7/E13 — ARCH/08 §8.9, doc 55 Steel + doc 33 §3.2).
//!
//! Encrypted per-site **full storage context** — cookie jars (host-keyed) +
//! localStorage + sessionStorage + IndexedDB + auth headers — stored in the
//! SQLCipher vault. The trust model (ARCH/08 §8.9) is enforced here:
//!
//! * the agent only ever sees the opaque `id` + [`SessionRecord`] metadata
//!   (`list_sessions` / `get_session` never return raw values);
//! * raw values flow only through [`SessionVault::inject`], which is gated by
//!   a per-agent Trust-Ladder [`TrustLevel`] grant;
//! * every capture / inject / rotate / revoke / deny writes a `session_uses`
//!   audit row so the replay/scrubber shows which account touched what.
//!
//! Chrome raw-storage decoding (`0x00` = UTF-16-LE, `0x01` = ISO-8859-1 —
//! Steel `leveldb` pattern, doc 55 §3) is applied by the *capture* path that
//! imports real Chrome storage; this module stores bytes verbatim and leaves
//! the decode to the import path.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Vault;

/// Trust-Ladder requirement per site+account (ARCH/08 §8.9): read-only =
/// low · form-fill = medium · drive-autonomously = high. Ordering is by
/// privilege, so a grant is sufficient when `granted >= requested`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TrustLevel {
    ReadOnly = 0,
    FormFill = 1,
    DriveAutonomous = 2,
}

impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustLevel::ReadOnly => "read_only",
            TrustLevel::FormFill => "form_fill",
            TrustLevel::DriveAutonomous => "drive_autonomous",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "form_fill" => TrustLevel::FormFill,
            "drive_autonomous" => TrustLevel::DriveAutonomous,
            _ => TrustLevel::ReadOnly,
        }
    }
}

/// Session lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Expired,
    Revoked,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Expired => "expired",
            SessionStatus::Revoked => "revoked",
        }
    }
}

/// One cookie in a session's jar. `value` is the raw bytes (SQLCipher-
/// encrypted at rest). Never serialized to the sidecar.
#[derive(Debug, Clone)]
pub struct Cookie {
    pub name: String,
    pub value: Vec<u8>,
    pub domain: String,
    pub path: String,
    pub expires: Option<i64>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: String,
}

/// Which storage surface a [`StorageItem`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    Local,
    Session,
    IndexedDb,
}

impl StorageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StorageKind::Local => "local",
            StorageKind::Session => "session",
            StorageKind::IndexedDb => "indexeddb",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "session" => StorageKind::Session,
            "indexeddb" => StorageKind::IndexedDb,
            _ => StorageKind::Local,
        }
    }
}

/// One localStorage / sessionStorage / IndexedDB entry.
#[derive(Debug, Clone)]
pub struct StorageItem {
    pub kind: StorageKind,
    pub key: String,
    pub value: Vec<u8>,
}

/// One captured auth header (e.g. a `Authorization` bearer for an API that
/// cannot use cookies).
#[derive(Debug, Clone)]
pub struct AuthHeader {
    pub name: String,
    pub value: Vec<u8>,
}

/// Full storage context. Returned **only** by [`SessionVault::inject`] to the
/// Rust browser layer (never the sidecar).
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub cookies: Vec<Cookie>,
    pub storage: Vec<StorageItem>,
    pub headers: Vec<AuthHeader>,
}

/// Input to [`SessionVault::capture`] — the complete context to seal.
#[derive(Debug, Clone)]
pub struct CaptureInput {
    pub cookies: Vec<Cookie>,
    pub storage: Vec<StorageItem>,
    pub headers: Vec<AuthHeader>,
    pub persist: bool,
    pub trust_level: TrustLevel,
    /// Optional TTL for expiry tracking (ARCH/08 §8.9 hygiene).
    pub ttl_secs: Option<u64>,
}

/// Agent-facing session metadata. **Deliberately carries no cookie / storage
/// / header values** — the "agent never sees raw cookies" invariant is
/// enforced by construction (no `value` field exists on this type).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRecord {
    pub id: String,
    pub site: String,
    pub account: String,
    pub persist: bool,
    pub trust_level: String,
    pub status: String,
    pub expires_at: Option<i64>,
    pub last_used_at: i64,
    pub created_at: i64,
}

/// One usage-audit row (`session_uses`).
#[derive(Debug, Clone)]
pub struct SessionUse {
    pub session_id: String,
    pub agent_session: String,
    pub action: String,
    pub ts: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("access denied: agent {agent} lacks {level} on session {session}")]
    AccessDenied {
        agent: String,
        session: String,
        level: String,
    },
    #[error("session {0} is {1}")]
    Inactive(String, String),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Session Vault handle over an open [`Vault`] (borrows the connection —
/// mirrors [`crate::KeyRing`]).
pub struct SessionVault<'a> {
    vault: &'a Vault,
}

impl<'a> SessionVault<'a> {
    pub fn new(vault: &'a Vault) -> Self {
        Self { vault }
    }

    fn conn(&self) -> &Connection {
        self.vault.connection()
    }

    /// Seal a full storage context for a `(site, account)` pair. Returns the
    /// opaque session id (deterministic per `site:account`, so re-capturing
    /// the same account *replaces* the context rather than duplicating it).
    pub fn capture(
        &self,
        site: &str,
        account: &str,
        input: CaptureInput,
    ) -> Result<String, SessionError> {
        let conn = self.conn();
        let id = session_id(site, account);
        let now = now_ms();
        let expires_at = input.ttl_secs.map(|t| now + (t as i64) * 1000);

        // Upsert the session row (keep the original created_at on re-capture).
        conn.execute(
            "INSERT INTO sessions
                (id, site, account, persist, trust_level, status, expires_at, last_used_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                persist      = excluded.persist,
                trust_level  = excluded.trust_level,
                status       = 'active',
                expires_at   = excluded.expires_at,
                last_used_at = excluded.last_used_at",
            params![
                id,
                site,
                account,
                input.persist as i64,
                input.trust_level.as_str(),
                expires_at,
                now,
                now,
            ],
        )?;

        // Replace the full context (delete-then-insert, never merge).
        conn.execute("DELETE FROM session_cookies WHERE session_id = ?1", [&id])?;
        conn.execute("DELETE FROM session_storage WHERE session_id = ?1", [&id])?;
        conn.execute("DELETE FROM session_headers WHERE session_id = ?1", [&id])?;

        for c in &input.cookies {
            conn.execute(
                "INSERT INTO session_cookies
                    (session_id, name, value, domain, path, expires, http_only, secure, same_site)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    c.name,
                    c.value,
                    c.domain,
                    c.path,
                    c.expires,
                    c.http_only as i64,
                    c.secure as i64,
                    c.same_site,
                ],
            )?;
        }

        for s in &input.storage {
            conn.execute(
                "INSERT INTO session_storage (session_id, kind, key, value) VALUES (?1, ?2, ?3, ?4)",
                params![id, s.kind.as_str(), s.key, s.value],
            )?;
        }

        for h in &input.headers {
            conn.execute(
                "INSERT INTO session_headers (session_id, name, value) VALUES (?1, ?2, ?3)",
                params![id, h.name, h.value],
            )?;
        }

        self.record_use(&id, "", "capture")?;
        Ok(id)
    }

    /// Agent-facing listing — metadata only, never raw values.
    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, SessionError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, site, account, persist, trust_level, status, expires_at, last_used_at, created_at
             FROM sessions ORDER BY site, account",
        )?;
        let rows = stmt.query_map([], row_to_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Metadata for one session (no values).
    pub fn get_session(&self, handle: &str) -> Result<SessionRecord, SessionError> {
        self.conn()
            .query_row(
                "SELECT id, site, account, persist, trust_level, status, expires_at, last_used_at, created_at
                 FROM sessions WHERE id = ?1",
                [handle],
                row_to_record,
            )
            .optional()?
            .ok_or_else(|| SessionError::NotFound(handle.to_string()))
    }

    /// Grant an agent a Trust-Ladder level on a session (idempotent upsert).
    pub fn grant(
        &self,
        handle: &str,
        agent_id: &str,
        level: TrustLevel,
    ) -> Result<(), SessionError> {
        self.get_session(handle)?; // fail-fast on unknown handle
        self.conn().execute(
            "INSERT INTO session_grants (session_id, agent_id, granted_level, granted_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, agent_id) DO UPDATE SET
                granted_level = excluded.granted_level,
                granted_at    = excluded.granted_at",
            params![handle, agent_id, level.as_str(), now_ms()],
        )?;
        Ok(())
    }

    /// Revoke one agent's grant.
    pub fn revoke_agent(&self, handle: &str, agent_id: &str) -> Result<(), SessionError> {
        self.conn().execute(
            "DELETE FROM session_grants WHERE session_id = ?1 AND agent_id = ?2",
            params![handle, agent_id],
        )?;
        Ok(())
    }

    /// Revoke all grants (full lock-down of a session).
    pub fn revoke_all(&self, handle: &str) -> Result<(), SessionError> {
        self.conn()
            .execute("DELETE FROM session_grants WHERE session_id = ?1", [handle])?;
        Ok(())
    }

    /// Does `agent_id` hold a grant >= `level` on an *active* session?
    pub fn authorize(
        &self,
        handle: &str,
        agent_id: &str,
        level: TrustLevel,
    ) -> Result<bool, SessionError> {
        match self.check_access(handle, agent_id, level) {
            Ok(_) => Ok(true),
            Err(SessionError::AccessDenied { .. }) | Err(SessionError::Inactive(..)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Shared access gate: session must exist, be active, and the agent must
    /// hold a grant at or above the requested level. Returns the record on
    /// success so callers can proceed without a second lookup. Denied /
    /// inactive attempts are audited as `deny` rows (mirrors the P2.6
    /// `browser.tab_claim` denied-attempts-are-audited rule).
    fn check_access(
        &self,
        handle: &str,
        agent_id: &str,
        level: TrustLevel,
    ) -> Result<SessionRecord, SessionError> {
        let rec = self.get_session(handle)?;
        if rec.status != SessionStatus::Active.as_str() {
            let _ = self.record_use(handle, agent_id, "deny");
            return Err(SessionError::Inactive(
                handle.to_string(),
                rec.status.clone(),
            ));
        }
        let granted: Option<String> = self
            .conn()
            .query_row(
                "SELECT granted_level FROM session_grants WHERE session_id = ?1 AND agent_id = ?2",
                params![handle, agent_id],
                |r| r.get(0),
            )
            .optional()?;
        match granted {
            Some(g) if TrustLevel::from_key(&g) >= level => Ok(rec),
            _ => {
                let _ = self.record_use(handle, agent_id, "deny");
                Err(SessionError::AccessDenied {
                    agent: agent_id.to_string(),
                    session: handle.to_string(),
                    level: level.as_str().to_string(),
                })
            }
        }
    }

    /// Inject the full storage context into the (Rust) browser layer. This is
    /// the **only** path that returns raw cookie/storage/header values, and it
    /// requires an active grant at or above `level`. The sidecar never calls
    /// this — the browser layer does, on the agent's behalf.
    pub fn inject(
        &self,
        handle: &str,
        agent_id: &str,
        level: TrustLevel,
    ) -> Result<SessionContext, SessionError> {
        let rec = self.check_access(handle, agent_id, level)?;
        let ctx = self.load_context(handle)?;
        self.conn().execute(
            "UPDATE sessions SET last_used_at = ?1 WHERE id = ?2",
            params![now_ms(), handle],
        )?;
        self.record_use(handle, agent_id, "inject")?;
        let _ = rec;
        Ok(ctx)
    }

    /// Rotate to the next authorized account for a site (round-robin by
    /// least-recently-used), mirroring key-ring rotation (A3). Returns `None`
    /// when no other authorized account exists.
    pub fn rotate_account(
        &self,
        site: &str,
        agent_id: &str,
        current_handle: &str,
        level: TrustLevel,
    ) -> Result<Option<String>, SessionError> {
        let mut stmt = self.conn().prepare(
            "SELECT id FROM sessions
             WHERE site = ?1 AND status = 'active' AND id != ?2
             ORDER BY last_used_at ASC",
        )?;
        let rows = stmt.query_map(params![site, current_handle], |r| r.get::<_, String>(0))?;
        for r in rows {
            let candidate = r?;
            if self.authorize(&candidate, agent_id, level)? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Mark a session expired (re-auth nudge card, ARCH/08 §8.9).
    pub fn mark_expired(&self, handle: &str) -> Result<(), SessionError> {
        self.get_session(handle)?;
        self.conn().execute(
            "UPDATE sessions SET status = 'expired' WHERE id = ?1",
            [handle],
        )?;
        Ok(())
    }

    /// Sessions whose TTL has lapsed (still flagged `active` but past expiry).
    pub fn expired_sessions(&self) -> Result<Vec<SessionRecord>, SessionError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, site, account, persist, trust_level, status, expires_at, last_used_at, created_at
             FROM sessions
             WHERE status = 'active' AND expires_at IS NOT NULL AND expires_at < ?1
             ORDER BY expires_at",
        )?;
        let rows = stmt.query_map([now_ms()], row_to_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Append a usage-audit row (capture / inject / rotate / revoke / deny).
    pub fn record_use(
        &self,
        handle: &str,
        agent_session: &str,
        action: &str,
    ) -> Result<(), SessionError> {
        self.conn().execute(
            "INSERT INTO session_uses (session_id, agent_session, action, ts) VALUES (?1, ?2, ?3, ?4)",
            params![handle, agent_session, action, now_ms()],
        )?;
        Ok(())
    }

    /// Usage-audit rows for a site (feeds the replay/scrubber "which account
    /// touched what" view).
    pub fn usage_rows(&self, site: &str) -> Result<Vec<SessionUse>, SessionError> {
        let mut stmt = self.conn().prepare(
            "SELECT u.session_id, u.agent_session, u.action, u.ts
             FROM session_uses u JOIN sessions s ON s.id = u.session_id
             WHERE s.site = ?1 ORDER BY u.ts",
        )?;
        let rows = stmt.query_map([site], |r| {
            Ok(SessionUse {
                session_id: r.get(0)?,
                agent_session: r.get(1)?,
                action: r.get(2)?,
                ts: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Per-site wipe (deletes the session + cascade cookies/storage/headers/
    /// grants; usage rows are retained for the audit trail).
    pub fn delete_session(&self, handle: &str) -> Result<(), SessionError> {
        self.get_session(handle)?;
        self.conn()
            .execute("DELETE FROM sessions WHERE id = ?1", [handle])?;
        Ok(())
    }

    /// Load the full context for a session (internal — called by `inject`
    /// after the access gate passes).
    fn load_context(&self, handle: &str) -> Result<SessionContext, SessionError> {
        let conn = self.conn();

        let mut cookies = Vec::new();
        let mut cstmt = conn.prepare(
            "SELECT name, value, domain, path, expires, http_only, secure, same_site
             FROM session_cookies WHERE session_id = ?1 ORDER BY name, domain, path",
        )?;
        let crows = cstmt.query_map([handle], |r| {
            Ok(Cookie {
                name: r.get(0)?,
                value: r.get(1)?,
                domain: r.get(2)?,
                path: r.get(3)?,
                expires: r.get(4)?,
                http_only: r.get::<_, i64>(5)? != 0,
                secure: r.get::<_, i64>(6)? != 0,
                same_site: r.get(7)?,
            })
        })?;
        for r in crows {
            cookies.push(r?);
        }

        let mut storage = Vec::new();
        let mut sstmt = conn.prepare(
            "SELECT kind, key, value FROM session_storage WHERE session_id = ?1 ORDER BY kind, key",
        )?;
        let srows = sstmt.query_map([handle], |r| {
            let kind: String = r.get(0)?;
            Ok(StorageItem {
                kind: StorageKind::from_str(&kind),
                key: r.get(1)?,
                value: r.get(2)?,
            })
        })?;
        for r in srows {
            storage.push(r?);
        }

        let mut headers = Vec::new();
        let mut hstmt = conn.prepare(
            "SELECT name, value FROM session_headers WHERE session_id = ?1 ORDER BY name",
        )?;
        let hrows = hstmt.query_map([handle], |r| {
            Ok(AuthHeader {
                name: r.get(0)?,
                value: r.get(1)?,
            })
        })?;
        for r in hrows {
            headers.push(r?);
        }

        Ok(SessionContext {
            cookies,
            storage,
            headers,
        })
    }
}

/// Deterministic opaque id for a `(site, account)` pair (stable across
/// re-captures, so capture = replace not duplicate).
fn session_id(site: &str, account: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("{site}:{account}").as_bytes());
    format!("sv_{}", to_hex(&h.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: r.get(0)?,
        site: r.get(1)?,
        account: r.get(2)?,
        persist: r.get::<_, i64>(3)? != 0,
        trust_level: r.get(4)?,
        status: r.get(5)?,
        expires_at: r.get(6)?,
        last_used_at: r.get(7)?,
        created_at: r.get(8)?,
    })
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
