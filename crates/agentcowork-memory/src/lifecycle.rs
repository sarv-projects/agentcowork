//! Memory lifecycle: retention, growth bounds, expiry and the sweeper
//! (`ARCH/17-MEMORY.md` §7, `REQ-MEM-005/010/016/017`).
//!
//! Every removal here is **audited individually and leaves the store
//! consistent** — the FTS row goes through the delete trigger, so a sweeper can
//! never leave an orphan. A pinned item is never auto-removed: the pin is a
//! user statement, and a bound never overrides it (the item is *reported*
//! instead, so an over-cap store is visible rather than silently trimmed).

use crate::scope::{Kind, ScopeKey};
use crate::store::{ForgetOutcome, MemoryStore, NewItem, StoreConfig, StoreError, WipeOutcome};
use serde::{Deserialize, Serialize};

/// The retention-class anchors that can end a session's memory lifetime
/// (`ARCH/11-WORK.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionAnchor {
    Archived,
    /// Only where the retention class allows it.
    Hibernated,
    /// The fallback when no anchor event was recorded.
    LastActive,
}

impl SessionAnchor {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionAnchor::Archived => "archived",
            SessionAnchor::Hibernated => "hibernated",
            SessionAnchor::LastActive => "last_active",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "archived" => Some(Self::Archived),
            "hibernated" => Some(Self::Hibernated),
            "last_active" => Some(Self::LastActive),
            _ => None,
        }
    }
}

/// What a sweep did. Every removed id is named, so retention is auditable
/// without an item body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepReport {
    /// Expired by the persisted anchor.
    pub expired: Vec<String>,
    /// Evicted for a per-scope item cap.
    pub evicted: Vec<String>,
    /// Pinned items that exceeded the cap and were **kept** (and reported).
    pub pinned_retained: Vec<String>,
    /// Sessions whose anchor was refreshed.
    pub anchored: Vec<String>,
}

impl SweepReport {
    pub fn is_empty(&self) -> bool {
        self.expired.is_empty()
            && self.evicted.is_empty()
            && self.pinned_retained.is_empty()
            && self.anchored.is_empty()
    }
}

/// A forget result plus the audit surface: a forget/wipe never returns an item
/// body, only ids, counts and the suppression digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetAudit {
    pub id: String,
    pub content_hash: String,
    pub removed_rows: usize,
    pub erased: bool,
    pub cascade_collapsed: bool,
}

impl From<(ForgetOutcome, bool)> for ForgetAudit {
    fn from((o, collapsed): (ForgetOutcome, bool)) -> Self {
        Self {
            id: o.id,
            content_hash: o.content_hash,
            removed_rows: o.removed_rows,
            erased: o.erased,
            cascade_collapsed: collapsed,
        }
    }
}

impl MemoryStore {
    /// Forget one item, permanently. Hard delete + suppression + audit, with the
    /// chain collapse: forgetting a head removes the rows it superseded, so
    /// nothing is resurrected and no `superseded_by` is left dangling.
    pub fn forget_with_audit(&mut self, id: &str, now: i64) -> Result<ForgetAudit, StoreError> {
        // The chain-collapse flag is decided before the delete: a head with
        // predecessors cascades, a head with none removes only itself.
        let had_chain: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_items WHERE superseded_by = ?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )?;
        let out = self.forget(id, now)?;
        Ok(ForgetAudit::from((out, had_chain)))
    }

    /// Wipe a scope. The scope's items and superseded rows go; **suppressions
    /// are retained**, because they are not scope data. The declared
    /// consequence: content forgotten anywhere stays un-extractable everywhere,
    /// including inside the wiped scope.
    pub fn wipe_with_audit(&mut self, key: &ScopeKey, now: i64) -> Result<ForgetAudit, StoreError> {
        let out: WipeOutcome = self.wipe_scope(key, now)?;
        Ok(ForgetAudit {
            id: key.as_key(),
            content_hash: String::new(),
            removed_rows: out.removed_rows,
            erased: out.erased,
            cascade_collapsed: false,
        })
    }

    /// The TTL expiry instant for a session scope, derived from the **persisted
    /// anchor**. Returns `None` while the session is still live.
    pub fn session_expiry(
        &self,
        session_id: &str,
        cfg: &StoreConfig,
    ) -> Result<Option<(i64, SessionAnchor)>, StoreError> {
        let Some((anchor, at)) = self.session_anchor(session_id)? else {
            return Ok(None);
        };
        let a = SessionAnchor::parse(&anchor).unwrap_or(SessionAnchor::LastActive);
        Ok(Some((at + cfg.session_ttl_ms, a)))
    }

    /// Anchor a session: record the anchor event's persisted timestamp. Once
    /// anchored, the item rows carry an `expires_at` computed from it — the TTL
    /// is then evaluated against a stored value, not a live wall-clock delta.
    pub fn anchor_session(
        &mut self,
        session_id: &str,
        anchor: SessionAnchor,
        anchored_at: i64,
        cfg: &StoreConfig,
    ) -> Result<usize, StoreError> {
        self.set_session_anchor(session_id, anchor.as_str(), anchored_at)?;
        let expiry = anchored_at + cfg.session_ttl_ms;
        let n = self.conn().execute(
            "UPDATE memory_items SET expires_at = ?2, updated_at = ?3
             WHERE scope = 'session' AND scope_ref = ?1 AND expires_at IS NULL",
            rusqlite::params![session_id, expiry, anchored_at],
        )?;
        Ok(n)
    }

    /// The retention sweep: expire, then evict to the per-scope caps.
    ///
    /// Order matters and is fixed: expiry first (a row past its anchor is not
    /// "the oldest", it is out of lifetime), then eviction oldest-first. A
    /// pinned item is skipped and reported.
    pub fn sweep(&mut self, now: i64, cfg: &StoreConfig) -> Result<SweepReport, StoreError> {
        let mut report = SweepReport {
            expired: Vec::new(),
            evicted: Vec::new(),
            pinned_retained: Vec::new(),
            anchored: Vec::new(),
        };

        // (1) Expiry. The predicate is `expires_at <= now` — evaluated against
        // the stored instant, so a backwards clock cannot resurrect an expired
        // row (it can only delay the sweep) nor expire a fresh one early.
        let expired: Vec<String> = {
            let mut stmt = self.conn().prepare(
                "SELECT id FROM memory_items WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            )?;
            let rows = stmt.query_map([now], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for id in &expired {
            // A forget-style delete (no suppression): the item aged out, and a
            // TTL expiry is not a user forget, so nothing is suppressed.
            self.conn()
                .execute("DELETE FROM memory_items WHERE id = ?1", [id])?;
            report.expired.push(id.clone());
        }

        // (2) Growth bounds, per scope, oldest first, unpinned only.
        for (scope, cap) in &cfg.scope_item_caps {
            let key_prefix = scope.as_str();
            let mut rows: Vec<(String, i64, bool)> = {
                let mut stmt = self.conn().prepare(
                    "SELECT id, created_at, pinned FROM memory_items
                     WHERE scope = ?1 ORDER BY created_at ASC, id ASC",
                )?;
                let it = stmt.query_map([key_prefix], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                })?;
                it.collect::<Result<Vec<_>, _>>()?
            };
            if rows.len() <= *cap {
                continue;
            }
            let over = rows.len() - *cap;
            // Take the oldest, skipping pinned rows.
            let mut taken = 0usize;
            let mut remove: Vec<String> = Vec::new();
            for (id, _, pinned) in rows.iter() {
                if taken >= over {
                    break;
                }
                if *pinned {
                    report.pinned_retained.push(id.clone());
                    continue;
                }
                remove.push(id.clone());
                taken += 1;
            }
            for id in &remove {
                self.conn()
                    .execute("DELETE FROM memory_items WHERE id = ?1", [id])?;
                report.evicted.push(id.clone());
            }
            rows.clear();
        }
        Ok(report)
    }

    /// Enforce the per-scope byte bound on top of the item-count cap. The
    /// declared byte bound for a scope is `cap × max_item_bytes`; when a scope
    /// exceeds it, the oldest unpinned rows are evicted, and when the remaining
    /// rows are **all pinned**, nothing is removed and the overage is reported.
    pub fn enforce_byte_bound(
        &mut self,
        key: &ScopeKey,
        cfg: &StoreConfig,
    ) -> Result<(usize, usize), StoreError> {
        let cap = cfg.cap_for(key.scope).unwrap_or(0);
        if cap == 0 {
            return Ok((0, 0));
        }
        let bound = (cap * cfg.max_item_bytes) as i64;
        let rows: Vec<(String, i64, bool)> = {
            let mut stmt = self.conn().prepare(
                "SELECT id, byte_size, pinned FROM memory_items
                 WHERE scope = ?1 AND scope_ref IS ?2
                 ORDER BY created_at ASC, id ASC",
            )?;
            let it = stmt.query_map(rusqlite::params![key.scope.as_str(), key.scope_ref], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            it.collect::<Result<Vec<_>, _>>()?
        };
        let total: i64 = rows.iter().map(|r| r.1).sum();
        if total <= bound {
            return Ok((0, 0));
        }
        let mut over = total - bound;
        let mut removed = 0usize;
        for (id, bytes, pinned) in &rows {
            if over <= 0 {
                break;
            }
            if *pinned {
                continue;
            }
            self.conn()
                .execute("DELETE FROM memory_items WHERE id = ?1", [id])?;
            over -= *bytes;
            removed += 1;
        }
        let remaining_over = over.max(0) as usize;
        Ok((removed, remaining_over))
    }

    /// The pinned members of a scope, for the always-on block. A non-touching
    /// read.
    pub fn pinned_in(&self, key: &ScopeKey, now: i64) -> Result<Vec<NewItem>, StoreError> {
        Ok(self
            .current_in(key, now)?
            .into_iter()
            .filter(|i| i.pinned)
            .map(|i| {
                let key = i.key();
                NewItem {
                    id: i.id,
                    key,
                    kind: i.kind,
                    content: i.content,
                    dedup_key: i.dedup_key,
                    sensitivity: i.sensitivity,
                    trust_tier: i.trust_tier,
                    source: i.source,
                    source_ref: i.source_ref,
                    confidence: i.confidence,
                    pinned: i.pinned,
                    expires_at: i.expires_at,
                }
            })
            .collect())
    }

    /// The declared per-scope bounds, for the drift simulation and the UI.
    pub fn bounds_report(&self, cfg: &StoreConfig) -> Result<Vec<ScopeBound>, StoreError> {
        let mut out = Vec::new();
        for (scope, cap) in &cfg.scope_item_caps {
            let live: i64 = self.conn().query_row(
                "SELECT COUNT(*) FROM memory_items WHERE scope = ?1",
                [scope.as_str()],
                |r| r.get(0),
            )?;
            let bytes: i64 = self.conn().query_row(
                "SELECT COALESCE(SUM(byte_size), 0) FROM memory_items WHERE scope = ?1",
                [scope.as_str()],
                |r| r.get(0),
            )?;
            let pinned: i64 = self.conn().query_row(
                "SELECT COUNT(*) FROM memory_items WHERE scope = ?1 AND pinned = 1",
                [scope.as_str()],
                |r| r.get(0),
            )?;
            out.push(ScopeBound {
                scope: *scope,
                max_items: *cap,
                live_items: live as usize,
                live_bytes: bytes as usize,
                byte_bound: cap * cfg.max_item_bytes,
                pinned_items: pinned as usize,
            });
        }
        Ok(out)
    }

    /// The declared prune precedence: summaries prune by count/age per scope
    /// like anything else, **except pinned summaries, which follow the pin
    /// rule** and are never auto-pruned.
    pub fn is_auto_prunable(&self, item: &crate::store::MemoryItem) -> bool {
        !item.pinned
    }

    /// A summary item is a *reference* to a checkpoint, never work state
    /// (DEC-041). This validates the shape at write time: a summary that
    /// claims to be a checkpoint is refused.
    pub fn validate_summary_is_reference(
        kind: Kind,
        source_ref: Option<&str>,
    ) -> Result<(), StoreError> {
        if kind == Kind::Summary {
            match source_ref {
                Some(r) if r.starts_with("checkpoint:") || r.starts_with("session:") => Ok(()),
                _ => Err(StoreError::Rejected(
                    "a summary item must carry a checkpoint:/session: source_ref and is never work state"
                        .into(),
                )),
            }
        } else {
            Ok(())
        }
    }
}

/// A declared scope bound, as the drift simulation reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeBound {
    pub scope: crate::scope::Scope,
    pub max_items: usize,
    pub live_items: usize,
    pub live_bytes: usize,
    pub byte_bound: usize,
    pub pinned_items: usize,
}

impl ScopeBound {
    /// Declared growth vs actual growth. A `None` here would be "unbounded
    /// growth with no declared bound", which the spec treats as a defect.
    pub fn is_within_bound(&self) -> bool {
        self.live_items <= self.max_items && self.live_bytes <= self.byte_bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::Scope;
    use crate::store::MemoryStore;

    fn store() -> MemoryStore {
        MemoryStore::open_in_memory(StoreConfig::default()).unwrap()
    }

    fn add(s: &mut MemoryStore, id: &str, key: &ScopeKey, text: &str, at: i64) {
        s.insert(&NewItem::new(id, key.clone(), Kind::Fact, text), at)
            .unwrap();
    }

    #[test]
    fn anchoring_a_session_sets_a_persisted_expiry_not_a_live_delta() {
        let mut s = store();
        let cfg = StoreConfig::default();
        s.insert(
            &NewItem::new(
                "m1",
                ScopeKey::session("s1"),
                Kind::Summary,
                "what happened",
            ),
            1_000,
        )
        .unwrap();
        let anchored_at = 5_000_000i64;
        let n = s
            .anchor_session("s1", SessionAnchor::Archived, anchored_at, &cfg)
            .unwrap();
        assert_eq!(n, 1);
        let (expiry, anchor) = s.session_expiry("s1", &cfg).unwrap().unwrap();
        assert_eq!(anchor, SessionAnchor::Archived);
        assert_eq!(expiry, anchored_at + cfg.session_ttl_ms);
        // A live session has no expiry at all.
        s.insert(
            &NewItem::new("m2", ScopeKey::session("s2"), Kind::Summary, "live"),
            1_000,
        )
        .unwrap();
        assert!(s.session_expiry("s2", &cfg).unwrap().is_none());
    }

    #[test]
    fn the_sweeper_expires_by_the_anchor_and_leaves_no_fts_orphans() {
        let mut s = store();
        let cfg = StoreConfig::default();
        s.insert(
            &NewItem::new(
                "old",
                ScopeKey::session("s1"),
                Kind::Summary,
                "bazel was the build tool",
            ),
            1_000,
        )
        .unwrap();
        s.anchor_session("s1", SessionAnchor::Archived, 1_000, &cfg)
            .unwrap();
        // Far future: expired.
        let far = 1_000 + cfg.session_ttl_ms + 1;
        let r = s.sweep(far, &cfg).unwrap();
        assert_eq!(r.expired, vec!["old".to_string()]);
        assert!(
            s.current_in(&ScopeKey::session("s1"), far)
                .unwrap()
                .is_empty()
        );
        assert!(s.integrity_check().unwrap().in_sync, "no FTS orphans");
        assert!(s.fts_candidates("bazel", 10).unwrap().is_empty());
    }

    #[test]
    fn a_backwards_clock_never_resurrects_an_expired_item() {
        let mut s = store();
        let cfg = StoreConfig::default();
        s.insert(
            &NewItem::new(
                "old",
                ScopeKey::session("s1"),
                Kind::Summary,
                "expired item",
            ),
            1_000,
        )
        .unwrap();
        s.anchor_session("s1", SessionAnchor::Archived, 1_000, &cfg)
            .unwrap();
        let after = 1_000 + cfg.session_ttl_ms + 1;
        s.sweep(after, &cfg).unwrap();
        // A clock that jumps back before the anchor: the row is already gone and
        // the read path still excludes it, because `expires_at` is stored.
        assert!(s.get("old").unwrap().is_none());
        assert!(
            s.current_in(&ScopeKey::session("s1"), 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_sweeper_evicts_the_oldest_unpinned_items_first() {
        let mut s = store();
        let cfg = StoreConfig {
            scope_item_caps: vec![(Scope::Project, 3)],
            ..Default::default()
        };
        for i in 0..5 {
            add(
                &mut s,
                &format!("m{i}"),
                &ScopeKey::project("p"),
                &format!("fact {i}"),
                i as i64,
            );
        }
        let r = s.sweep(100, &cfg).unwrap();
        assert_eq!(r.evicted, vec!["m0".to_string(), "m1".to_string()]);
        let live: Vec<String> = s
            .current_in(&ScopeKey::project("p"), 100)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(live, vec!["m2", "m3", "m4"]);
    }

    #[test]
    fn a_pinned_item_survives_pruning_and_is_reported_instead() {
        let mut s = store();
        let cfg = StoreConfig {
            scope_item_caps: vec![(Scope::Project, 2)],
            ..Default::default()
        };
        add(&mut s, "old", &ScopeKey::project("p"), "old fact", 1);
        add(&mut s, "mid", &ScopeKey::project("p"), "mid fact", 2);
        add(&mut s, "new", &ScopeKey::project("p"), "new fact", 3);
        s.set_pinned("old", true, 4).unwrap();
        let r = s.sweep(100, &cfg).unwrap();
        // The cap needs two evictions; the oldest is pinned, so it is retained
        // and the *next* oldest goes instead.
        assert_eq!(r.evicted, vec!["mid".to_string()]);
        assert!(r.pinned_retained.contains(&"old".to_string()));
        assert!(
            s.get("old").unwrap().is_some(),
            "a pinned item is never auto-pruned"
        );
    }

    #[test]
    fn a_fully_pinned_scope_keeps_everything_and_reports_the_overage() {
        let mut s = store();
        let cfg = StoreConfig {
            scope_item_caps: vec![(Scope::User, 1)],
            ..Default::default()
        };
        add(&mut s, "a", &ScopeKey::user("u"), "pinned one", 1);
        add(&mut s, "b", &ScopeKey::user("u"), "pinned two", 2);
        s.set_pinned("a", true, 3).unwrap();
        s.set_pinned("b", true, 4).unwrap();
        let r = s.sweep(100, &cfg).unwrap();
        assert!(r.evicted.is_empty());
        assert_eq!(r.pinned_retained.len(), 2);
        assert!(s.get("a").unwrap().is_some() && s.get("b").unwrap().is_some());
    }

    #[test]
    fn the_byte_bound_is_declared_and_enforced() {
        let mut s = store();
        let mut cfg = StoreConfig {
            scope_item_caps: vec![(Scope::Project, 2)],
            ..Default::default()
        };
        cfg.max_item_bytes = 100;
        add(&mut s, "a", &ScopeKey::project("p"), &"x".repeat(90), 1);
        add(&mut s, "b", &ScopeKey::project("p"), &"y".repeat(90), 2);
        let bounds = s.bounds_report(&cfg).unwrap();
        let proj = bounds.iter().find(|b| b.scope == Scope::Project).unwrap();
        assert_eq!(proj.byte_bound, 200);
        assert_eq!(proj.live_bytes, 180);
        assert!(proj.is_within_bound());
        add(&mut s, "c", &ScopeKey::project("p"), &"z".repeat(90), 3);
        // 270 bytes over a 200-byte bound: evicting the oldest 90-byte row
        // brings the scope inside the bound, and the reported remainder is 0
        // because the bound is now satisfied.
        let (removed, over) = s.enforce_byte_bound(&ScopeKey::project("p"), &cfg).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(over, 0);
        assert!(s.integrity_check().unwrap().in_sync);
        let bounds = s.bounds_report(&cfg).unwrap();
        let proj = bounds.iter().find(|b| b.scope == Scope::Project).unwrap();
        assert!(proj.is_within_bound());
    }

    #[test]
    fn a_summary_must_reference_a_checkpoint_and_is_never_work_state() {
        assert!(
            MemoryStore::validate_summary_is_reference(Kind::Summary, Some("checkpoint:ckpt:1"))
                .is_ok()
        );
        assert!(
            MemoryStore::validate_summary_is_reference(Kind::Summary, Some("session:s1")).is_ok()
        );
        assert!(MemoryStore::validate_summary_is_reference(Kind::Summary, Some("turn:9")).is_err());
        assert!(MemoryStore::validate_summary_is_reference(Kind::Summary, None).is_err());
        assert!(MemoryStore::validate_summary_is_reference(Kind::Fact, None).is_ok());
    }

    #[test]
    fn forgetting_a_head_collapses_the_chain_it_headed() {
        let mut s = store();
        add(&mut s, "gen1", &ScopeKey::project("p"), "generation one", 1);
        add(&mut s, "gen2", &ScopeKey::project("p"), "generation two", 2);
        s.mark_superseded("gen1", "gen2", 3).unwrap();
        let audit = s.forget_with_audit("gen2", 4).unwrap();
        assert!(audit.cascade_collapsed);
        assert_eq!(audit.removed_rows, 2);
        assert!(s.get("gen1").unwrap().is_none());
        assert!(s.integrity_check().unwrap().in_sync);
    }

    #[test]
    fn forgetting_a_leaf_never_fails_and_leaves_no_dangling_pointer() {
        let mut s = store();
        add(&mut s, "gen1", &ScopeKey::project("p"), "generation one", 1);
        add(&mut s, "gen2", &ScopeKey::project("p"), "generation two", 2);
        s.mark_superseded("gen1", "gen2", 3).unwrap();
        // Forget the *older* row: the head stays current, no error, no dangling
        // pointer (the pointer lived on the deleted row).
        let audit = s.forget_with_audit("gen1", 4).unwrap();
        assert!(!audit.cascade_collapsed);
        assert_eq!(audit.removed_rows, 1);
        let head = s.get("gen2").unwrap().unwrap();
        assert!(head.superseded_by.is_none());
        assert!(s.integrity_check().unwrap().in_sync);
    }

    #[test]
    fn the_pointer_integrity_check_passes_after_every_mutation() {
        let mut s = store();
        add(&mut s, "a", &ScopeKey::project("p"), "a", 1);
        add(&mut s, "b", &ScopeKey::project("p"), "b", 2);
        s.mark_superseded("a", "b", 3).unwrap();
        s.forget_with_audit("b", 4).unwrap();
        let dangling: i64 = s
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM memory_items m WHERE m.superseded_by IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM memory_items t WHERE t.id = m.superseded_by)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0);
    }

    #[test]
    fn a_scope_wipe_retains_suppressions_and_is_reported() {
        let mut s = store();
        add(&mut s, "a", &ScopeKey::project("p"), "alpha", 1);
        s.forget_with_audit("a", 2).unwrap();
        add(&mut s, "b", &ScopeKey::project("p"), "beta", 3);
        let before = s.suppression_count().unwrap();
        let audit = s.wipe_with_audit(&ScopeKey::project("p"), 4).unwrap();
        assert_eq!(audit.removed_rows, 1);
        assert_eq!(s.suppression_count().unwrap(), before);
        assert!(s.integrity_check().unwrap().in_sync);
    }

    #[test]
    fn pinned_members_are_listed_for_the_always_on_block() {
        let mut s = store();
        add(&mut s, "a", &ScopeKey::user("u"), "prefers tabs", 1);
        add(&mut s, "b", &ScopeKey::user("u"), "unpinned fact", 1);
        s.set_pinned("a", true, 2).unwrap();
        let pinned = s.pinned_in(&ScopeKey::user("u"), 10).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].id, "a");
    }

    #[test]
    fn the_bounds_report_declares_a_bound_for_every_writable_scope() {
        let s = store();
        let cfg = StoreConfig::default();
        let report = s.bounds_report(&cfg).unwrap();
        assert_eq!(
            report.len(),
            4,
            "session/task/project/user — org is not writable"
        );
        assert!(report.iter().all(|b| b.max_items > 0 && b.byte_bound > 0));
        assert!(report.iter().all(ScopeBound::is_within_bound));
    }
}
