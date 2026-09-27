//! P36 — `ManagedResource` (spec v3.39; ARCH/01 §resource record).
//!
//! A live-registry hit is not a capability. MCP servers, ACP agents, model
//! runners, browser children, sandboxes, and workers materialize as a durable,
//! versioned, health-aware **resource record**. The lifecycle is
//! discover → validate → install → configure → enable → start → health →
//! use → observe → update/rollback → remove. **Install ≠ enable ≠ running ≠
//! healthy** — each is a distinct field and a distinct transition.
//!
//! Deliberately **not** Office (ticketed mutation engine), **not** vault
//! providers (credential lifecycle), **not** deployments. The resource owns a
//! process; effects still require a ticket.

use serde::{Deserialize, Serialize};

/// Hook invoked when a resource transition is audited.
type AuditHook = Box<dyn Fn(&str, &str, bool) + Send + Sync>;

/// The kinds of process a [`ManagedResource`] can own. Office and vault
/// providers are excluded by construction (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    ModelRunner,
    McpServer,
    AcpAgent,
    BrowserChild,
    Sandbox,
    Worker,
}

/// Lifecycle phases. `Removed` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourcePhase {
    Discovered,
    Validated,
    Installed,
    Enabled,
    Started,
    Stopped,
    Faulted,
    Removed,
}

impl ResourcePhase {
    /// The legal transition set. Install ≠ enable ≠ running ≠ healthy:
    /// every hop is explicit, and every real effect still requires a ticket.
    pub fn can_transition(self, next: Self) -> bool {
        use ResourcePhase::*;
        matches!(
            (self, next),
            (Discovered, Validated)
                | (Validated, Installed)
                | (Installed, Enabled)
                | (Enabled, Started)
                | (Started, Started) // restart
                | (Started, Faulted)
                | (Discovered, Removed)
                | (Validated, Removed)
                | (Installed, Removed)
                | (Enabled, Removed)
                | (Started, Removed)
                | (Started, Stopped)
                | (Stopped, Started)
                | (Stopped, Enabled)
                | (Faulted, Started)
                | (Faulted, Removed)
                | (Stopped, Removed)
        )
    }
}

/// Health is runtime evidence, distinct from install/enable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceHealth {
    Unknown,
    Starting,
    Healthy,
    Degraded,
    Down,
}

/// Where the install state lives — install state, enable state, and runtime
/// health are distinct facts and are never conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallState {
    NotInstalled,
    Installing,
    Installed,
    UpdateAvailable,
    UpdateInProgress,
    RolledBack,
}

/// The durable record. `R` is the typed row (e.g. an `MCPServerRecord`,
/// a `LocalManager` entry, an ACP installer row) — this wrapper supplies the
/// uniform kernel lifecycle every managed process shares.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedResource<R> {
    pub id: String,
    pub kind: ResourceKind,
    /// Immutable config fingerprint: any config change mints a new hash and
    /// forces re-validation (install-state ≠ config-state).
    pub config_hash: String,
    pub version: String,
    pub phase: ResourcePhase,
    pub health: ResourceHealth,
    pub install_state: InstallState,
    pub enabled: bool,
    pub observed_at_ms: u64,
    pub last_error: Option<String>,
    /// Events (audit-friendly, append-only).
    pub lifecycle_log: Vec<LifecycleEvent>,
    /// The typed row carried by this record.
    pub record: R,
}

/// One append-only lifecycle event (receipts feed the J5 audit family).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub at_ms: u64,
    pub what: String,
    pub ok: bool,
    pub detail: Option<String>,
}

impl<R> ManagedResource<R> {
    pub fn new(
        id: impl Into<String>,
        kind: ResourceKind,
        config_hash: impl Into<String>,
        version: impl Into<String>,
        record: R,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            config_hash: config_hash.into(),
            version: version.into(),
            phase: ResourcePhase::Discovered,
            health: ResourceHealth::Unknown,
            install_state: InstallState::NotInstalled,
            enabled: false,
            observed_at_ms: 0,
            last_error: None,
            lifecycle_log: Vec::new(),
            record,
        }
    }

    /// Transition with an explicit event. Rejects illegal hops — the state
    /// machine is the kernel's guard against "started but never installed".
    pub fn transition(
        &mut self,
        next: ResourcePhase,
        what: &str,
        ok: bool,
        detail: Option<String>,
        at_ms: u64,
    ) -> Result<(), String> {
        if !self.phase.can_transition(next) {
            return Err(format!(
                "illegal resource transition {:?} -> {:?} for {}",
                self.phase, next, self.id
            ));
        }
        self.phase = next;
        if next == ResourcePhase::Enabled {
            self.enabled = true;
        } else if next == ResourcePhase::Removed {
            self.enabled = false;
        }
        self.observed_at_ms = at_ms;
        self.last_error = if ok { None } else { detail.clone() };
        self.lifecycle_log.push(LifecycleEvent {
            at_ms,
            what: what.to_string(),
            ok,
            detail,
        });
        Ok(())
    }

    /// Convenience step: transition with an OK event (no detail). The
    /// full [`Self::transition`] is the audit-grade path.
    pub fn step(&mut self, next: ResourcePhase, what: &str, at_ms: u64) -> Result<(), String> {
        self.transition(next, what, true, None, at_ms)
    }

    pub fn set_health(&mut self, health: ResourceHealth, at_ms: u64) {
        self.health = health;
        self.observed_at_ms = at_ms;
    }

    pub fn report_error(&mut self, what: &str, detail: impl Into<String>, at_ms: u64) {
        self.health = ResourceHealth::Down;
        self.last_error = Some(detail.into());
        self.lifecycle_log.push(LifecycleEvent {
            at_ms,
            what: what.to_string(),
            ok: false,
            detail: self.last_error.clone(),
        });
    }

    /// Install ≠ enable ≠ running ≠ healthy — a resource can be installed but
    /// disabled, running but degraded.
    pub fn running_and_healthy(&self) -> bool {
        self.enabled
            && self.phase == ResourcePhase::Started
            && self.health == ResourceHealth::Healthy
    }
}

/// The kernel registry: every managed resource in one place, with an audit
/// sink so lifecycle events land in the event log.
#[derive(Default)]
pub struct ResourceManager {
    audit: Option<AuditHook>,
    pub resources: Vec<ManagedResource<serde_json::Value>>,
}

impl ResourceManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_audit(mut self, f: impl Fn(&str, &str, bool) + Send + Sync + 'static) -> Self {
        self.audit = Some(Box::new(f));
        self
    }

    pub fn register(&mut self, res: ManagedResource<serde_json::Value>) {
        self.audit_event("resource.register", &res.id, true);
        self.resources.push(res);
    }

    pub fn get(&self, id: &str) -> Option<&ManagedResource<serde_json::Value>> {
        self.resources.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ManagedResource<serde_json::Value>> {
        self.resources.iter_mut().find(|r| r.id == id)
    }

    pub fn list(&self) -> &[ManagedResource<serde_json::Value>] {
        &self.resources
    }

    pub fn list_kind(
        &self,
        kind: ResourceKind,
    ) -> impl Iterator<Item = &ManagedResource<serde_json::Value>> {
        self.resources.iter().filter(move |r| r.kind == kind)
    }

    pub fn transition(
        &mut self,
        id: &str,
        next: ResourcePhase,
        what: &str,
        at_ms: u64,
    ) -> Result<(), String> {
        {
            let res = self
                .get_mut(id)
                .ok_or_else(|| format!("no resource {id}"))?;
            res.transition(next, what, true, None, at_ms)?;
        }
        self.audit_event(id, what, true);
        Ok(())
    }

    fn audit_event(&mut self, id: &str, what: &str, ok: bool) {
        if let Some(f) = &mut self.audit {
            f(id, what, ok);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> u64 {
        1_700_000_000_000
    }

    #[test]
    fn full_lifecycle_round_trip() {
        let mut mgr = ResourceManager::new();
        let audits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let audits_clone = audits.clone();
        mgr.audit = Some(Box::new(move |id, what, _ok| {
            audits_clone
                .lock()
                .unwrap()
                .push((id.to_string(), what.to_string()))
        }));
        let res = ManagedResource::new(
            "mcp-filesystem",
            ResourceKind::McpServer,
            "hash-abc",
            "1.2.3",
            serde_json::json!({"transport": "stdio"}),
        );
        mgr.register(res);
        for (phase, what) in [
            (ResourcePhase::Validated, "validate"),
            (ResourcePhase::Installed, "install"),
            (ResourcePhase::Enabled, "enable"),
            (ResourcePhase::Started, "start"),
        ] {
            mgr.transition("mcp-filesystem", phase, what, at()).unwrap();
        }
        let r = mgr.get("mcp-filesystem").unwrap();
        assert_eq!(r.phase, ResourcePhase::Started);
        assert!(!r.running_and_healthy());
        assert_eq!(audits.lock().unwrap().len(), 5); // register + 4 transitions all audited
    }

    #[test]
    fn install_is_not_running_is_not_healthy() {
        let mut r =
            ManagedResource::new("x", ResourceKind::Worker, "h", "1", serde_json::Value::Null);
        r.step(ResourcePhase::Validated, "v", at()).unwrap();
        r.step(ResourcePhase::Installed, "i", at()).unwrap();
        r.step(ResourcePhase::Enabled, "e", at()).unwrap();
        r.install_state = InstallState::Installed;
        r.health = ResourceHealth::Down;
        // Installed + enabled but not started and not healthy — usable check false
        assert!(!r.running_and_healthy());
        r.step(ResourcePhase::Started, "s", at()).unwrap();
        r.set_health(ResourceHealth::Healthy, at());
        assert!(r.running_and_healthy());
    }

    #[test]
    fn illegal_transition_rejected() {
        let mut r =
            ManagedResource::new("y", ResourceKind::Worker, "h", "1", serde_json::Value::Null);
        // Started before installed is illegal
        assert!(r.step(ResourcePhase::Started, "start", at()).is_err());
    }

    #[test]
    fn removed_is_terminal() {
        let mut r = ManagedResource::new(
            "z",
            ResourceKind::Sandbox,
            "h",
            "1",
            serde_json::Value::Null,
        );
        r.step(ResourcePhase::Validated, "v", at()).unwrap();
        r.step(ResourcePhase::Installed, "i", at()).unwrap();
        r.step(ResourcePhase::Enabled, "e", at()).unwrap();
        r.step(ResourcePhase::Started, "s", at()).unwrap();
        r.step(ResourcePhase::Removed, "rm", at()).unwrap();
        assert!(r.step(ResourcePhase::Started, "restart", at()).is_err());
    }

    #[test]
    fn faulted_can_restart_or_remove() {
        let mut r = ManagedResource::new(
            "f",
            ResourceKind::BrowserChild,
            "h",
            "1",
            serde_json::Value::Null,
        );
        for p in [
            ResourcePhase::Validated,
            ResourcePhase::Installed,
            ResourcePhase::Enabled,
            ResourcePhase::Started,
        ] {
            r.step(p, "step", at()).unwrap();
        }
        r.step(ResourcePhase::Faulted, "crash", at()).unwrap();
        let mut r2 = r.clone();
        assert!(r.step(ResourcePhase::Started, "restart", at()).is_ok());
        assert!(r2.step(ResourcePhase::Removed, "remove", at()).is_ok());
    }
}
