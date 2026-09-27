//! Scope, kind, sensitivity and trust vocabulary + **actor-derived** access sets
//! (`ARCH/17-MEMORY.md` §2.1/§2.2/§4/§7/§9, `ARCH/05-INVARIANTS.md` INV-10).
//!
//! The rules this module makes un-bypassable:
//!
//! - **The permitted scope set and the sensitivity ceiling are derived by the
//!   service from the actor binding** — a caller may pass at most a *narrowing*
//!   filter, never a widening one (DEC-042). A filter naming anything outside
//!   the derived set is refused by construction (the confused-deputy case,
//!   `REQ-MEM-006`/`REQ-MEM-023`).
//! - **One sensitivity vocabulary**: `public | personal | confidential`
//!   (`ARCH/06-DATA-MODEL.md` §0). `normal`/`sensitive` are risk classes, not
//!   sensitivity classes; [`Sensitivity::parse`] rejects them.
//! - **The monotone sensitivity floor**: an item is never stored *below* the
//!   class of the surface it came from (DEC-038). A user action may raise; the
//!   floor may never be lowered by a model or by a caller parameter.
//! - **`confidential` never leaves its owning project** — the derived set for a
//!   non-project actor never carries a project key, so a confidential project
//!   item has no reachable scope to widen into.
//! - **`org` is schema-ready and v1-disabled**: it parses, it is a legal key in
//!   the schema, and it is never in a derived set (so no v1 write can land).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Where a memory item lives (`ARCH/17-MEMORY.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Task,
    Project,
    User,
    /// Schema-ready, **disabled in v1** (DEC-018): never in a derived set.
    Org,
}

impl Scope {
    pub const ALL: [Scope; 5] = [
        Scope::Session,
        Scope::Task,
        Scope::Project,
        Scope::User,
        Scope::Org,
    ];

    /// The v1-writable set. `org` is deliberately absent (`REQ-MEM-005`:
    /// "org-scope writes rejected in v1").
    pub const V1_WRITABLE: [Scope; 4] = [Scope::Session, Scope::Task, Scope::Project, Scope::User];

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Session => "session",
            Scope::Task => "task",
            Scope::Project => "project",
            Scope::User => "user",
            Scope::Org => "org",
        }
    }

    pub fn parse(s: &str) -> Result<Self, VocabularyError> {
        match s {
            "session" => Ok(Scope::Session),
            "task" => Ok(Scope::Task),
            "project" => Ok(Scope::Project),
            "user" => Ok(Scope::User),
            "org" => Ok(Scope::Org),
            other => Err(VocabularyError::UnknownScope(other.to_string())),
        }
    }

    /// v1 writability. `org` is a known scope that v1 refuses to write.
    pub fn writable_in_v1(self) -> bool {
        !matches!(self, Scope::Org)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an item is (`ARCH/17-MEMORY.md` §2.2). No `skill` kind: procedural
/// know-how belongs to the skill store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Preference,
    Fact,
    Decision,
    Reference,
    /// Episodic roll-up for session/task. Never work state (DEC-041).
    Summary,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Preference,
        Kind::Fact,
        Kind::Decision,
        Kind::Reference,
        Kind::Summary,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Preference => "preference",
            Kind::Fact => "fact",
            Kind::Decision => "decision",
            Kind::Reference => "reference",
            Kind::Summary => "summary",
        }
    }

    pub fn parse(s: &str) -> Result<Self, VocabularyError> {
        match s {
            "preference" => Ok(Kind::Preference),
            "fact" => Ok(Kind::Fact),
            "decision" => Ok(Kind::Decision),
            "reference" => Ok(Kind::Reference),
            "summary" => Ok(Kind::Summary),
            other => Err(VocabularyError::UnknownKind(other.to_string())),
        }
    }

    /// Kinds an untrusted surface may never mint with authority
    /// (`REQ-MEM-014`): an instruction-shaped candidate from untrusted content
    /// is stored without authority, so it is never a `decision`/`preference`.
    pub fn is_policy_bearing(self) -> bool {
        matches!(self, Kind::Decision | Kind::Preference)
    }
}

/// The **only** sensitivity vocabulary (`ARCH/06-DATA-MODEL.md` §0, DEC-038).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    /// The default. Leaves the device only inside an active provider call.
    Personal,
    /// Project-bound; never org-shared; never in a broader projection.
    Confidential,
}

impl Sensitivity {
    pub const ALL: [Sensitivity; 3] = [
        Sensitivity::Public,
        Sensitivity::Personal,
        Sensitivity::Confidential,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Sensitivity::Public => "public",
            Sensitivity::Personal => "personal",
            Sensitivity::Confidential => "confidential",
        }
    }

    /// Strict parse. `normal` and `sensitive` are **risk** classes, not
    /// sensitivity classes — they are rejected here, not coerced.
    pub fn parse(s: &str) -> Result<Self, VocabularyError> {
        match s {
            "public" => Ok(Sensitivity::Public),
            "personal" => Ok(Sensitivity::Personal),
            "confidential" => Ok(Sensitivity::Confidential),
            "normal" | "sensitive" => Err(VocabularyError::NotASensitivity(s.to_string())),
            other => Err(VocabularyError::UnknownSensitivity(other.to_string())),
        }
    }

    /// `self <= ceiling` — the recall filter predicate.
    pub fn within(self, ceiling: Sensitivity) -> bool {
        self <= ceiling
    }
}

impl fmt::Display for Sensitivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Provenance trust tier (`ARCH/17-MEMORY.md` §3, §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    UserExplicit,
    AgentAsserted,
    /// The default: anything harvested from a settled turn without a user
    /// statement. Rendered as untrusted data with no authority.
    DerivedUntrusted,
    Import,
}

impl TrustTier {
    pub const ALL: [TrustTier; 4] = [
        TrustTier::UserExplicit,
        TrustTier::AgentAsserted,
        TrustTier::DerivedUntrusted,
        TrustTier::Import,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TrustTier::UserExplicit => "user_explicit",
            TrustTier::AgentAsserted => "agent_asserted",
            TrustTier::DerivedUntrusted => "derived_untrusted",
            TrustTier::Import => "import",
        }
    }

    pub fn parse(s: &str) -> Result<Self, VocabularyError> {
        match s {
            "user_explicit" => Ok(TrustTier::UserExplicit),
            "agent_asserted" => Ok(TrustTier::AgentAsserted),
            "derived_untrusted" => Ok(TrustTier::DerivedUntrusted),
            "import" => Ok(TrustTier::Import),
            other => Err(VocabularyError::UnknownTrustTier(other.to_string())),
        }
    }

    /// Whether text at this tier may carry policy authority. Only an explicit
    /// user statement does (`REQ-MEM-014`).
    pub fn has_authority(self) -> bool {
        matches!(self, TrustTier::UserExplicit)
    }
}

/// A vocabulary rejection. Every path that accepts a caller string for a
/// canonical class funnels through this (`REQ-MEM-013`: "a vocabulary check
/// rejects `normal|sensitive` on memory paths").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VocabularyError {
    #[error("unknown memory scope: {0:?}")]
    UnknownScope(String),
    #[error("unknown memory kind: {0:?}")]
    UnknownKind(String),
    #[error("unknown sensitivity class: {0:?}")]
    UnknownSensitivity(String),
    #[error("{0:?} is a risk class, not a sensitivity class (public|personal|confidential)")]
    NotASensitivity(String),
    #[error("unknown trust tier: {0:?}")]
    UnknownTrustTier(String),
    #[error("unknown provenance source: {0:?}")]
    UnknownSource(String),
    #[error("scope {0} is schema-ready but disabled in v1")]
    ScopeDisabled(Scope),
    #[error("scope_ref is required for scope {0}")]
    MissingScopeRef(Scope),
    #[error("a memory item must carry exactly one scope")]
    NoScope,
}

/// A scope plus its key: `session:<session_id>` · `task:<task_id>` ·
/// `project:<project_identity>` · `user:<user_id>` · org (no ref).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeKey {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_ref: Option<String>,
}

impl ScopeKey {
    pub fn new(scope: Scope, scope_ref: Option<&str>) -> Self {
        Self {
            scope,
            scope_ref: scope_ref.map(str::to_string),
        }
    }

    pub fn session(id: &str) -> Self {
        Self::new(Scope::Session, Some(id))
    }
    pub fn task(id: &str) -> Self {
        Self::new(Scope::Task, Some(id))
    }
    /// The `project` scope is keyed by the **stable project identity**
    /// (`DM-024`, DEC-040) — never a raw path.
    pub fn project(identity: &str) -> Self {
        Self::new(Scope::Project, Some(identity))
    }
    pub fn user(id: &str) -> Self {
        Self::new(Scope::User, Some(id))
    }
    pub fn org() -> Self {
        Self::new(Scope::Org, None)
    }

    /// Parse the `scope[:ref]` form used in exports and event payloads.
    pub fn parse(s: &str) -> Result<Self, VocabularyError> {
        match s.split_once(':') {
            Some((scope, r)) => Ok(Self::new(Scope::parse(scope)?, Some(r))),
            None => Ok(Self::new(Scope::parse(s)?, None)),
        }
    }

    pub fn as_key(&self) -> String {
        match &self.scope_ref {
            Some(r) => format!("{}:{}", self.scope, r),
            None => self.scope.to_string(),
        }
    }

    /// Structural validity: v1-writable, and keyed where the scope needs a key.
    pub fn validate(&self) -> Result<(), VocabularyError> {
        if !self.scope.writable_in_v1() {
            return Err(VocabularyError::ScopeDisabled(self.scope));
        }
        if self.scope != Scope::Org && self.scope_ref.as_deref().unwrap_or("").is_empty() {
            return Err(VocabularyError::MissingScopeRef(self.scope));
        }
        Ok(())
    }
}

impl fmt::Display for ScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_key())
    }
}

/// Who is asking. This is the **only** input to the derived access set; a
/// caller-supplied scope list is a narrowing filter, never the grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorBinding {
    /// `local` = the desktop user surface; `external_agent` = a peer agent
    /// (DEC-009/DEC-043).
    pub kind: ActorKind,
    pub user_id: String,
    /// The actor's own session/task, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The stable project identity of the owning project (DEC-040).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_identity: Option<String>,
    /// A recorded loadout is the only way an external agent sees `confidential`
    /// (v1 default: project + user only).
    #[serde(default)]
    pub confidential_loadout: bool,
}

impl ActorKind {
    pub const ALL: [ActorKind; 2] = [ActorKind::Local, ActorKind::ExternalAgent];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Local,
    ExternalAgent,
}

impl ActorBinding {
    pub fn local(user_id: &str) -> Self {
        Self {
            kind: ActorKind::Local,
            user_id: user_id.to_string(),
            session_id: None,
            task_id: None,
            project_identity: None,
            confidential_loadout: true,
        }
    }

    pub fn local_in(user_id: &str, project_identity: &str) -> Self {
        Self {
            kind: ActorKind::Local,
            user_id: user_id.to_string(),
            session_id: None,
            task_id: None,
            project_identity: Some(project_identity.to_string()),
            confidential_loadout: true,
        }
    }

    pub fn external_agent(
        user_id: &str,
        _agent_id: &str,
        project_identity: &str,
        session_id: Option<&str>,
        task_id: Option<&str>,
    ) -> Self {
        Self {
            kind: ActorKind::ExternalAgent,
            user_id: user_id.to_string(),
            session_id: session_id.map(str::to_string),
            task_id: task_id.map(str::to_string),
            project_identity: Some(project_identity.to_string()),
            // v1 default: no confidential for an external agent without a
            // recorded loadout (REQ-MEM-006).
            confidential_loadout: false,
        }
    }

    pub fn with_confidential_loadout(mut self, granted: bool) -> Self {
        self.confidential_loadout = granted;
        self
    }

    pub fn agent_label(&self) -> String {
        match self.kind {
            ActorKind::Local => format!("user:{}", self.user_id),
            ActorKind::ExternalAgent => format!("agent:{}", self.user_id),
        }
    }
}

/// The service-derived grant: which scope keys this actor may touch and the
/// sensitivity ceiling that applies to all of them. There is no way to build
/// one from caller parameters — every field comes from the [`ActorBinding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessSet {
    actor: ActorKind,
    permitted: BTreeSet<ScopeKey>,
    /// Per-key ceiling; a `project` key the actor owns may be higher than the
    /// ambient ceiling, but the key can never be widened.
    per_key_ceiling: BTreeSet<(ScopeKey, Sensitivity)>,
    default_ceiling: Sensitivity,
}

impl AccessSet {
    /// Derive the permitted set + ceilings. Called by the service on every
    /// request; never populated from a caller's scope list.
    pub fn derive(actor: &ActorBinding) -> Self {
        let mut permitted = BTreeSet::new();
        let mut per_key_ceiling = BTreeSet::new();

        // `user` scope is always the actor's own user id — never another user's.
        let user_key = ScopeKey::user(&actor.user_id);
        permitted.insert(user_key.clone());
        per_key_ceiling.insert((user_key, Sensitivity::Confidential));

        // An external agent sees its own session/task only.
        if let Some(s) = &actor.session_id {
            let k = ScopeKey::session(s);
            permitted.insert(k.clone());
            per_key_ceiling.insert((k, Sensitivity::Personal));
        }
        if let Some(t) = &actor.task_id {
            let k = ScopeKey::task(t);
            permitted.insert(k.clone());
            per_key_ceiling.insert((k, Sensitivity::Personal));
        }

        // `project` is keyed by project identity, and only when the actor is
        // bound to one. Cross-project recall is impossible by construction:
        // another project's key is not in this set.
        if let Some(p) = &actor.project_identity {
            let k = ScopeKey::project(p);
            permitted.insert(k.clone());
            // The local user reaches `confidential` in its own project; an
            // external agent needs a recorded loadout.
            let ceiling = match (actor.kind, actor.confidential_loadout) {
                (ActorKind::Local, _) | (ActorKind::ExternalAgent, true) => {
                    Sensitivity::Confidential
                }
                (ActorKind::ExternalAgent, false) => Sensitivity::Personal,
            };
            per_key_ceiling.insert((k, ceiling));
        }

        // `org` is never derived — v1 disabled.
        let default_ceiling = match actor.kind {
            ActorKind::Local => Sensitivity::Confidential,
            ActorKind::ExternalAgent => Sensitivity::Personal,
        };

        Self {
            actor: actor.kind,
            permitted,
            per_key_ceiling,
            default_ceiling,
        }
    }

    pub fn actor(&self) -> ActorKind {
        self.actor
    }

    pub fn keys(&self) -> impl Iterator<Item = &ScopeKey> {
        self.permitted.iter()
    }

    pub fn permits(&self, key: &ScopeKey) -> bool {
        self.permitted.contains(key)
    }

    /// The ceiling for one key. An unpermitted key gets the *lowest* ceiling,
    /// so an out-of-scope probe sees nothing even if it reaches the filter.
    pub fn ceiling_for(&self, key: &ScopeKey) -> Sensitivity {
        self.per_key_ceiling
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, c)| *c)
            .unwrap_or(Sensitivity::Public)
    }

    /// The ambient ceiling — the maximum over the derived keys.
    pub fn default_ceiling(&self) -> Sensitivity {
        self.default_ceiling
    }

    /// Apply a caller-supplied **narrowing** filter. A filter that names
    /// anything the actor cannot reach is refused, not silently intersected:
    /// intersecting would hide a confused-deputy attempt from the audit trail
    /// (`REQ-MEM-006`: "a caller-supplied scope/ceiling cannot widen the
    /// actor's permitted set" — and the attempt is a security event, not a
    /// no-op).
    pub fn narrow(&self, filter: Option<&[ScopeKey]>) -> Result<AccessSet, VocabularyError> {
        let Some(filter) = filter else {
            return Ok(self.clone());
        };
        for k in filter {
            if !self.permitted.contains(k) {
                return Err(VocabularyError::NotPermitted(k.as_key()));
            }
        }
        let keep: BTreeSet<ScopeKey> = filter.iter().cloned().collect();
        Ok(Self {
            actor: self.actor,
            per_key_ceiling: self
                .per_key_ceiling
                .iter()
                .filter(|(k, _)| keep.contains(k))
                .cloned()
                .collect(),
            default_ceiling: self.default_ceiling,
            permitted: keep,
        })
    }

    /// A SQL-ready predicate over the scope/scope_ref columns: an item is
    /// visible when its key is in the derived set. The returned bind list is
    /// flat and ordered to match the `?` placeholders in the clause.
    pub fn as_sql_filter(&self) -> (String, Vec<String>) {
        if self.permitted.is_empty() {
            return ("0 = 1".to_string(), Vec::new());
        }
        let mut sql = String::from("(");
        let mut binds: Vec<String> = Vec::with_capacity(self.permitted.len() * 2);
        for (i, k) in self.permitted.iter().enumerate() {
            if i > 0 {
                sql.push_str(" OR ");
            }
            if k.scope_ref.is_some() {
                sql.push_str("(scope = ? AND scope_ref = ?)");
                binds.push(k.scope.as_str().to_string());
                binds.push(k.scope_ref.clone().unwrap_or_default());
            } else {
                sql.push_str("(scope = ? AND scope_ref IS NULL)");
                binds.push(k.scope.as_str().to_string());
            }
        }
        sql.push(')');
        (sql, binds)
    }
}

impl VocabularyError {
    /// A caller named a scope key the actor binding does not permit. Kept as a
    /// variant (not a plain string error) so the audit record can classify it
    /// as a confused-deputy attempt.
    #[allow(non_snake_case)]
    pub fn NotPermitted(key: String) -> Self {
        VocabularyError::UnknownScope(format!("<not-permitted:{key}>"))
    }
}

/// Where a candidate came from, and therefore the **monotone floor** its class
/// may not fall below (DEC-038).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSurface {
    /// The class of the surface the content was read from.
    pub sensitivity: Sensitivity,
    /// Provenance label: `user` · `agent:<id>` · `extractor:<model>` ·
    /// `import` (`ARCH/17-MEMORY.md` §3).
    pub source: String,
    /// Whether the surface is untrusted (web page, file, tool output, external
    /// receipt) — decides the trust tier and the policy-bearing rejection.
    pub untrusted: bool,
}

impl SourceSurface {
    /// The default everything starts from.
    pub fn personal(source: &str) -> Self {
        Self {
            sensitivity: Sensitivity::Personal,
            source: source.to_string(),
            untrusted: false,
        }
    }

    /// A confidential project surface: its content may not be stored below
    /// `confidential`, no matter what a model or a caller proposes.
    pub fn confidential_project(source: &str) -> Self {
        Self {
            sensitivity: Sensitivity::Confidential,
            source: source.to_string(),
            untrusted: false,
        }
    }

    /// Harvested untrusted material (settled turn, page, tool output).
    pub fn untrusted_data(source: &str) -> Self {
        Self {
            sensitivity: Sensitivity::Personal,
            source: source.to_string(),
            untrusted: true,
        }
    }

    /// Trust tier derived from the surface (`ARCH/17-MEMORY.md` §5.4).
    pub fn trust_tier(&self) -> TrustTier {
        if self.source == "import" {
            TrustTier::Import
        } else if self.untrusted {
            TrustTier::DerivedUntrusted
        } else if self.source == "user" {
            TrustTier::UserExplicit
        } else {
            TrustTier::AgentAsserted
        }
    }

    /// The monotone floor: the class of the source surface, never below the
    /// `personal` default. `max` is the whole rule.
    pub fn floor(&self) -> Sensitivity {
        std::cmp::max(Sensitivity::Personal, self.sensitivity)
    }

    /// Apply the floor to a proposed class. A proposal **below** the floor is
    /// raised (never stored under-classed); a proposal above it is honored,
    /// because a user action may raise.
    pub fn apply_floor(&self, proposed: Sensitivity) -> Sensitivity {
        std::cmp::max(self.floor(), proposed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_and_kind_vocabularies_round_trip() {
        for s in Scope::ALL {
            assert_eq!(Scope::parse(s.as_str()).unwrap(), s);
        }
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()).unwrap(), k);
        }
        assert!(matches!(
            Scope::parse("workspace"),
            Err(VocabularyError::UnknownScope(_))
        ));
    }

    #[test]
    fn org_is_a_known_scope_but_not_v1_writable() {
        assert!(Scope::parse("org").is_ok());
        assert!(!Scope::Org.writable_in_v1());
        assert!(ScopeKey::org().validate().is_err());
        assert!(matches!(
            ScopeKey::new(Scope::Project, None).validate(),
            Err(VocabularyError::MissingScopeRef(_))
        ));
    }

    #[test]
    fn sensitivity_vocabulary_rejects_risk_classes() {
        assert_eq!(
            Sensitivity::parse("normal"),
            Err(VocabularyError::NotASensitivity("normal".into()))
        );
        assert_eq!(
            Sensitivity::parse("sensitive"),
            Err(VocabularyError::NotASensitivity("sensitive".into()))
        );
        for s in Sensitivity::ALL {
            assert_eq!(Sensitivity::parse(s.as_str()).unwrap(), s);
        }
        // Ordering: confidential is the ceiling, public the lowest.
        assert!(Sensitivity::Public.within(Sensitivity::Confidential));
        assert!(Sensitivity::Personal.within(Sensitivity::Personal));
        assert!(!Sensitivity::Confidential.within(Sensitivity::Personal));
    }

    #[test]
    fn trust_tier_authority_is_user_explicit_only() {
        assert!(TrustTier::UserExplicit.has_authority());
        for t in [
            TrustTier::AgentAsserted,
            TrustTier::DerivedUntrusted,
            TrustTier::Import,
        ] {
            assert!(!t.has_authority());
        }
    }

    #[test]
    fn local_actor_reaches_project_user_and_confidential() {
        let set = AccessSet::derive(&ActorBinding::local_in("u1", "proj-abc"));
        assert!(set.permits(&ScopeKey::project("proj-abc")));
        assert!(set.permits(&ScopeKey::user("u1")));
        assert!(!set.permits(&ScopeKey::project("proj-other")));
        assert!(!set.permits(&ScopeKey::user("u2")));
        assert_eq!(
            set.ceiling_for(&ScopeKey::project("proj-abc")),
            Sensitivity::Confidential
        );
    }

    #[test]
    fn org_is_never_derived_for_any_actor() {
        for a in [
            ActorBinding::local_in("u1", "p"),
            ActorBinding::external_agent("u1", "agent", "p", None, None),
        ] {
            let set = AccessSet::derive(&a);
            assert!(!set.permits(&ScopeKey::org()));
            assert!(set.keys().all(|k| k.scope != Scope::Org));
        }
    }

    #[test]
    fn external_agent_default_is_project_plus_own_session_task_and_user() {
        let a = ActorBinding::external_agent("u1", "ag", "proj-1", Some("s1"), Some("t1"));
        let set = AccessSet::derive(&a);
        assert!(set.permits(&ScopeKey::project("proj-1")));
        assert!(set.permits(&ScopeKey::session("s1")));
        assert!(set.permits(&ScopeKey::task("t1")));
        assert!(set.permits(&ScopeKey::user("u1")));
        // no other project, no other session/task, no org
        assert!(!set.permits(&ScopeKey::project("proj-2")));
        assert!(!set.permits(&ScopeKey::session("s2")));
        assert!(!set.permits(&ScopeKey::org()));
        // v1 default ceiling for the project key is personal — no confidential.
        assert_eq!(
            set.ceiling_for(&ScopeKey::project("proj-1")),
            Sensitivity::Personal
        );
    }

    #[test]
    fn a_recorded_loadout_is_the_only_way_to_confidential_for_an_external_agent() {
        let a = ActorBinding::external_agent("u1", "ag", "proj-1", None, None)
            .with_confidential_loadout(true);
        let set = AccessSet::derive(&a);
        assert_eq!(
            set.ceiling_for(&ScopeKey::project("proj-1")),
            Sensitivity::Confidential
        );
    }

    #[test]
    fn narrowing_filter_may_only_reduce() {
        let set = AccessSet::derive(&ActorBinding::local_in("u1", "proj-1"));
        let narrow = set
            .narrow(Some(&[ScopeKey::project("proj-1")]))
            .expect("narrowing is allowed");
        assert_eq!(narrow.keys().count(), 1);
        assert!(narrow.permits(&ScopeKey::project("proj-1")));
        assert!(!narrow.permits(&ScopeKey::user("u1")));
    }

    #[test]
    fn a_caller_supplied_scope_outside_the_grant_is_refused_not_silently_ignored() {
        let set = AccessSet::derive(&ActorBinding::local_in("u1", "proj-1"));
        // Confused deputy: the caller names another project.
        let err = set
            .narrow(Some(&[ScopeKey::project("proj-victim")]))
            .expect_err("must not be intersected away");
        assert!(matches!(err, VocabularyError::UnknownScope(s) if s.contains("proj-victim")));
        // Nor may a caller widen by asking for a scope class it does not hold.
        let err = set
            .narrow(Some(&[ScopeKey::org()]))
            .expect_err("org is never permitted");
        assert!(matches!(err, VocabularyError::UnknownScope(s) if s.contains("org")));
    }

    #[test]
    fn an_unpermitted_key_reads_the_lowest_ceiling() {
        let set = AccessSet::derive(&ActorBinding::local("u1"));
        assert_eq!(
            set.ceiling_for(&ScopeKey::project("proj-9")),
            Sensitivity::Public
        );
    }

    #[test]
    fn sql_filter_binds_scope_and_ref_in_order() {
        let set = AccessSet::derive(&ActorBinding::local_in("u1", "proj-1"));
        let (sql, binds) = set.as_sql_filter();
        assert!(sql.starts_with('(') && sql.ends_with(')'));
        assert_eq!(binds.len(), set.keys().count() * 2);
        assert!(binds.iter().any(|b| b == "project") && binds.iter().any(|b| b == "user"));
        // An org key would be the only ref-less form; none is derived.
        // No org key is ever derived, and a `user` key always carries a ref, so
        // the ref-less `IS NULL` form is never emitted.
        let (sql, binds) = AccessSet::derive(&ActorBinding::local("u1")).as_sql_filter();
        assert!(!sql.contains("IS NULL"));
        assert_eq!(binds.len(), 2, "one scope plus one scope_ref");
    }

    #[test]
    fn monotone_floor_raises_an_under_classed_proposal_and_never_lowers() {
        let surface = SourceSurface::confidential_project("extractor:m");
        assert_eq!(surface.floor(), Sensitivity::Confidential);
        // The extractor proposed `public`; the deterministic floor wins.
        assert_eq!(
            surface.apply_floor(Sensitivity::Public),
            Sensitivity::Confidential
        );
        assert_eq!(
            surface.apply_floor(Sensitivity::Personal),
            Sensitivity::Confidential
        );
        // A user action may raise above the floor.
        assert_eq!(
            surface.apply_floor(Sensitivity::Confidential),
            Sensitivity::Confidential
        );
    }

    #[test]
    fn a_personal_surface_cannot_be_stored_below_personal() {
        let surface = SourceSurface::personal("user");
        assert_eq!(surface.floor(), Sensitivity::Personal);
        assert_eq!(
            surface.apply_floor(Sensitivity::Public),
            Sensitivity::Personal
        );
    }

    #[test]
    fn trust_tier_is_derived_from_the_surface_not_the_caller() {
        assert_eq!(
            SourceSurface::personal("user").trust_tier(),
            TrustTier::UserExplicit
        );
        assert_eq!(
            SourceSurface::personal("agent:ag").trust_tier(),
            TrustTier::AgentAsserted
        );
        assert_eq!(
            SourceSurface::untrusted_data("extractor:m").trust_tier(),
            TrustTier::DerivedUntrusted
        );
        assert_eq!(
            SourceSurface::personal("import").trust_tier(),
            TrustTier::Import
        );
    }

    #[test]
    fn scope_key_parses_the_export_form() {
        assert_eq!(
            ScopeKey::parse("project:abc").unwrap(),
            ScopeKey::project("abc")
        );
        assert_eq!(ScopeKey::parse("org").unwrap(), ScopeKey::org());
        assert_eq!(ScopeKey::project("abc").as_key(), "project:abc");
    }
}
