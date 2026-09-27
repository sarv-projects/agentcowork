//! Tab ownership (E6 — ARCH/08 §8.4, doc 33 §6, ARCH/09 E6).
//!
//! Every tab has an owner: `mine | user | other-agent`. User tabs are never
//! touched unless the user asks; agent tabs are grouped per agent session;
//! closing an agent session closes its tab group; every claim/denial is
//! recorded in the audit log (`browser.tab_claim` rows — the `tab_claims`
//! table, BrowserOS model).
//!
//! The registry is a pure in-memory policy store: it decides, the caller
//! executes. `TabRegistry::sync_targets` reconciles against CDP
//! (`Target.getTargets`) so user-opened tabs and agent-opened tabs are
//! attributed without any application state on the browser side.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agentcowork_cdp::{TargetInfo, TargetType};
use serde::{Deserialize, Serialize};

/// Who owns a tab (ARCH/08 §8.4). Kebab-case to match the script sandbox's
/// `PageOwnership` (agentcowork-script) — the two map 1:1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TabOwner {
    Mine,
    User,
    OtherAgent,
}

impl TabOwner {
    pub fn label(self) -> &'static str {
        match self {
            TabOwner::Mine => "mine",
            TabOwner::User => "user",
            TabOwner::OtherAgent => "other-agent",
        }
    }
}

/// One tab's live ownership record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabRecord {
    /// CDP target id (`Target.getTargets` / `/json/list`).
    pub tab_id: String,
    pub url: String,
    pub title: String,
    pub owner: TabOwner,
    /// Present iff `owner == Mine` — the agent session that claimed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    /// Tab-group id (per agent session — ARCH/08 §8.4 "grouped per agent").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// When the tab was first seen (UNIX ms).
    pub first_seen_ms: u64,
    /// When it was claimed by an agent (UNIX ms); 0 = never.
    #[serde(default)]
    pub claimed_at_ms: u64,
}

/// The audit payload for `browser.tab_claim` events (the `tab_claims` table).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabClaim {
    pub tab_id: String,
    pub owner: TabOwner,
    /// `claim` | `release` | `denied` | `group_closed`
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Why an ownership decision refused a request.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OwnershipError {
    #[error("tab {0} is not tracked (not seen via CDP)")]
    UnknownTab(String),
    #[error("tab {0} belongs to the user — never touched unless asked")]
    UserTab(String),
    #[error("tab {0} belongs to another agent")]
    OtherAgent(String),
    #[error("tab {0} is owned by agent session {1}, not {2}")]
    WrongSession(String, String, String),
}

/// The tab-ownership policy store. Thread-safe; shared across the browser
/// layer, the script sandbox's host, and the coordinator.
pub struct TabRegistry {
    tabs: Mutex<HashMap<String, TabRecord>>,
    /// agent_session -> group_id (one group per agent session).
    groups: Mutex<HashMap<String, String>>,
    /// Optional audit sink — every claim/denial lands here as a
    /// `browser.tab_claim` event (the `tab_claims` table).
    audit: Option<Arc<Mutex<agentcowork_audit::AuditWriter>>>,
}

impl Default for TabRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TabRegistry {
    pub fn new() -> Self {
        Self {
            tabs: Mutex::new(HashMap::new()),
            groups: Mutex::new(HashMap::new()),
            audit: None,
        }
    }

    /// Attach an audit writer; `browser.tab_claim` events are written from
    /// here on. Writer errors are swallowed (audit must never break the
    /// browser path — the writer's own contract).
    pub fn with_audit(&mut self, writer: Arc<Mutex<agentcowork_audit::AuditWriter>>) -> &mut Self {
        self.audit = Some(writer);
        self
    }

    /// Reconcile against a CDP target list (`Target.getTargets` / json list).
    /// - New page/tab targets → owned by the **user** (they were opened by
    ///   the user or an untracked surface). Agents claim via `claim`.
    /// - Targets that vanished → their records are dropped (tab closed).
    /// - Existing records keep their owner/group across refreshes.
    pub fn sync_targets(&self, targets: &[TargetInfo]) {
        let mut tabs = self.tabs.lock().unwrap();
        let now = now_ms();

        let mut seen: std::collections::HashSet<String> = Default::default();
        for t in targets {
            // Only page-like targets are tabs.
            if t.target_type != TargetType::Page && t.target_type != TargetType::Tab {
                continue;
            }
            seen.insert(t.target_id.clone());
            tabs.entry(t.target_id.clone())
                .or_insert_with(|| TabRecord {
                    tab_id: t.target_id.clone(),
                    url: t.url.clone(),
                    title: t.title.clone(),
                    owner: TabOwner::User,
                    agent_session: None,
                    group_id: None,
                    first_seen_ms: now,
                    claimed_at_ms: 0,
                });
        }
        tabs.retain(|id, _| seen.contains(id));
    }

    /// Agent claims a tab it opened (or was handed). Assigns the agent's
    /// group (creating one per session on first claim — ARCH/08 §8.4
    /// "agent tabs grouped per agent"). Audits `browser.tab_claim`.
    pub fn claim(&self, tab_id: &str, agent_session: &str) -> Result<(), OwnershipError> {
        let mut tabs = self.tabs.lock().unwrap();
        let rec = tabs
            .get_mut(tab_id)
            .ok_or_else(|| OwnershipError::UnknownTab(tab_id.to_string()))?;

        let now = now_ms();
        let group_id = {
            let mut groups = self.groups.lock().unwrap();
            groups
                .entry(agent_session.to_string())
                .or_insert_with(|| format!("agent-{agent_session}"))
                .clone()
        };
        rec.owner = TabOwner::Mine;
        rec.agent_session = Some(agent_session.to_string());
        rec.group_id = Some(group_id.clone());
        rec.claimed_at_ms = now;

        self.audit_claim(TabClaim {
            tab_id: tab_id.to_string(),
            owner: TabOwner::Mine,
            action: "claim".into(),
            agent_session: Some(agent_session.to_string()),
            group_id: Some(group_id),
            reason: None,
        });
        Ok(())
    }

    /// Agent closes one of its own tabs: the record is dropped (the tab is
    /// being closed). Audits a `release`.
    pub fn release(&self, tab_id: &str, agent_session: &str) -> Result<(), OwnershipError> {
        let mut tabs = self.tabs.lock().unwrap();
        let rec = tabs
            .get(tab_id)
            .ok_or_else(|| OwnershipError::UnknownTab(tab_id.to_string()))?;
        if rec.owner != TabOwner::Mine {
            return Err(match rec.owner {
                TabOwner::User => OwnershipError::UserTab(tab_id.to_string()),
                _ => OwnershipError::OtherAgent(tab_id.to_string()),
            });
        }
        if rec.agent_session.as_deref() != Some(agent_session) {
            return Err(OwnershipError::WrongSession(
                tab_id.to_string(),
                rec.agent_session.clone().unwrap_or_default(),
                agent_session.to_string(),
            ));
        }
        tabs.remove(tab_id);
        drop(tabs);

        self.audit_claim(TabClaim {
            tab_id: tab_id.to_string(),
            owner: TabOwner::Mine,
            action: "release".into(),
            agent_session: Some(agent_session.to_string()),
            group_id: None,
            reason: Some("agent closed its tab".into()),
        });
        Ok(())
    }

    /// May `agent_session` close `tab_id`? The refusal is audited too —
    /// denied attempts are part of the trail.
    pub fn can_close(&self, tab_id: &str, agent_session: &str) -> Result<(), OwnershipError> {
        let tabs = self.tabs.lock().unwrap();
        let rec = tabs
            .get(tab_id)
            .ok_or_else(|| OwnershipError::UnknownTab(tab_id.to_string()))?;
        match rec.owner {
            TabOwner::Mine => {
                if rec.agent_session.as_deref() == Some(agent_session) {
                    Ok(())
                } else {
                    let e = OwnershipError::WrongSession(
                        tab_id.to_string(),
                        rec.agent_session.clone().unwrap_or_default(),
                        agent_session.to_string(),
                    );
                    self.audit_claim(TabClaim {
                        tab_id: tab_id.to_string(),
                        owner: rec.owner,
                        action: "denied".into(),
                        agent_session: Some(agent_session.to_string()),
                        group_id: rec.group_id.clone(),
                        reason: Some(e.to_string()),
                    });
                    Err(e)
                }
            }
            TabOwner::User => {
                let e = OwnershipError::UserTab(tab_id.to_string());
                self.audit_claim(TabClaim {
                    tab_id: tab_id.to_string(),
                    owner: rec.owner,
                    action: "denied".into(),
                    agent_session: Some(agent_session.to_string()),
                    group_id: rec.group_id.clone(),
                    reason: Some(e.to_string()),
                });
                Err(e)
            }
            TabOwner::OtherAgent => {
                let e = OwnershipError::OtherAgent(tab_id.to_string());
                self.audit_claim(TabClaim {
                    tab_id: tab_id.to_string(),
                    owner: rec.owner,
                    action: "denied".into(),
                    agent_session: Some(agent_session.to_string()),
                    group_id: rec.group_id.clone(),
                    reason: Some(e.to_string()),
                });
                Err(e)
            }
        }
    }

    /// Closing an agent session closes its tab group: returns every tab id
    /// in that session's group (caller closes them via CDP). Audits each.
    pub fn close_agent_group(&self, agent_session: &str) -> Vec<String> {
        let tabs = self.tabs.lock().unwrap();
        let ids: Vec<String> = tabs
            .iter()
            .filter(|(_, r)| {
                r.owner == TabOwner::Mine && r.agent_session.as_deref() == Some(agent_session)
            })
            .map(|(id, r)| {
                self.audit_claim(TabClaim {
                    tab_id: id.clone(),
                    owner: TabOwner::Mine,
                    action: "group_closed".into(),
                    agent_session: Some(agent_session.to_string()),
                    group_id: r.group_id.clone(),
                    reason: Some("agent session closed".into()),
                });
                id.clone()
            })
            .collect();
        drop(tabs);
        let mut tabs = self.tabs.lock().unwrap();
        for id in &ids {
            tabs.remove(id);
        }
        ids
    }

    /// Owner of a tab (defaults to `User` for untracked tabs — the
    /// fail-closed direction: never assume an agent owns a tab it didn't
    /// claim).
    pub fn owner_of(&self, tab_id: &str) -> TabOwner {
        self.tabs
            .lock()
            .unwrap()
            .get(tab_id)
            .map(|r| r.owner)
            .unwrap_or(TabOwner::User)
    }

    /// Live records — the source for `pages.list()` ownership labels.
    pub fn records(&self) -> Vec<TabRecord> {
        self.tabs
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>()
    }

    /// The group id assigned to an agent session (if any).
    pub fn group_of(&self, agent_session: &str) -> Option<String> {
        self.groups.lock().unwrap().get(agent_session).cloned()
    }

    fn audit_claim(&self, claim: TabClaim) {
        if let Some(w) = &self.audit {
            if let Ok(mut w) = w.lock() {
                let _ = w.write(
                    "browser.tab_claim",
                    serde_json::to_value(claim).unwrap_or_default(),
                );
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, url: &str, typ: TargetType) -> TargetInfo {
        TargetInfo {
            target_id: id.into(),
            target_type: typ,
            title: String::new(),
            url: url.into(),
            ws_url: String::new(),
            frame_id: None,
        }
    }

    #[test]
    fn sync_marks_new_tabs_as_user() {
        let r = TabRegistry::new();
        r.sync_targets(&[
            target("t1", "https://user.example/", TargetType::Page),
            target("w1", "about:blank", TargetType::Worker), // ignored
        ]);
        assert_eq!(r.owner_of("t1"), TabOwner::User);
        assert_eq!(r.owner_of("w1"), TabOwner::User); // untracked → fail-closed User
        assert_eq!(r.records().len(), 1);
    }

    #[test]
    fn claim_assigns_mine_and_group() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t1", "https://agent.example/", TargetType::Page)]);
        r.claim("t1", "sess-1").unwrap();
        assert_eq!(r.owner_of("t1"), TabOwner::Mine);
        assert_eq!(r.group_of("sess-1").as_deref(), Some("agent-sess-1"));
        let rec = r.records().into_iter().find(|x| x.tab_id == "t1").unwrap();
        assert_eq!(rec.agent_session.as_deref(), Some("sess-1"));
        assert_eq!(rec.group_id.as_deref(), Some("agent-sess-1"));
        assert!(rec.claimed_at_ms > 0);
    }

    #[test]
    fn sync_preserves_existing_claims() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t1", "https://a/", TargetType::Page)]);
        r.claim("t1", "sess-1").unwrap();
        // Refresh with same target still present — claim survives.
        r.sync_targets(&[target("t1", "https://a/", TargetType::Page)]);
        assert_eq!(r.owner_of("t1"), TabOwner::Mine);
    }

    #[test]
    fn sync_drops_closed_tabs() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t1", "https://a/", TargetType::Page)]);
        r.sync_targets(&[]);
        assert_eq!(r.records().len(), 0);
    }

    #[test]
    fn agent_cannot_close_a_user_tab() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t-user", "https://user.example/", TargetType::Page)]);
        let err = r.can_close("t-user", "sess-1").unwrap_err();
        assert_eq!(err, OwnershipError::UserTab("t-user".into()));
    }

    #[test]
    fn agent_cannot_close_another_agents_tab() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t1", "https://a/", TargetType::Page)]);
        r.claim("t1", "sess-A").unwrap();
        let err = r.can_close("t1", "sess-B").unwrap_err();
        assert_eq!(
            err,
            OwnershipError::WrongSession("t1".into(), "sess-A".into(), "sess-B".into())
        );
    }

    #[test]
    fn agent_can_close_own_tab() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t1", "https://a/", TargetType::Page)]);
        r.claim("t1", "sess-1").unwrap();
        assert!(r.can_close("t1", "sess-1").is_ok());
    }

    #[test]
    fn release_removes_own_tab() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t1", "https://a/", TargetType::Page)]);
        r.claim("t1", "sess-1").unwrap();
        r.release("t1", "sess-1").unwrap();
        assert_eq!(r.records().len(), 0);
        assert_eq!(r.owner_of("t1"), TabOwner::User); // fail-closed default
    }

    #[test]
    fn release_of_user_tab_fails() {
        let r = TabRegistry::new();
        r.sync_targets(&[target("t-user", "https://u/", TargetType::Page)]);
        let err = r.release("t-user", "sess-1").unwrap_err();
        assert_eq!(err, OwnershipError::UserTab("t-user".into()));
    }

    #[test]
    fn close_agent_group_returns_only_that_sessions_tabs() {
        let r = TabRegistry::new();
        r.sync_targets(&[
            target("t1", "https://a/", TargetType::Page),
            target("t2", "https://b/", TargetType::Page),
            target("t-user", "https://u/", TargetType::Page),
        ]);
        r.claim("t1", "sess-A").unwrap();
        r.claim("t2", "sess-B").unwrap();
        let closed = r.close_agent_group("sess-A");
        assert_eq!(closed, vec!["t1".to_string()]);
        // sess-B's tab and the user tab survive.
        assert_eq!(r.owner_of("t2"), TabOwner::Mine);
        assert_eq!(r.owner_of("t-user"), TabOwner::User);
        assert_eq!(r.owner_of("t1"), TabOwner::User); // gone → fail-closed User
    }

    #[test]
    fn denied_attempts_are_audited() {
        let dir = std::env::temp_dir().join(format!("agentcowork-own-test-{}", std::process::id()));
        let path = dir.join("audit.ndjson");
        let _ = std::fs::remove_dir_all(&dir);
        let w = agentcowork_audit::AuditWriter::open(&path).unwrap();
        let mut r = TabRegistry::new();
        r.with_audit(Arc::new(Mutex::new(w)));
        r.sync_targets(&[target("t-user", "https://u/", TargetType::Page)]);
        let _ = r.can_close("t-user", "sess-1");
        let _ = r.claim("t-user", "sess-1"); // claim also works on user tabs (hand-off)

        let log = std::fs::read_to_string(&path).unwrap();
        assert!(
            log.contains("\"action\":\"denied\""),
            "denial must be audited: {log}"
        );
        assert!(
            log.contains("\"action\":\"claim\""),
            "claim must be audited: {log}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
