//! The read-only source seam — the **Core context infrastructure** half of the
//! two-layer split (`ARCH/16-CONTEXT.md` §1.1, `REQ-CTX-001/003/004/010`,
//! `REQ-MEM-003/022`).
//!
//! ```text
//! context.search    (query, scopes) → ContextCandidate[]   refs + bounded snippets
//! context.snapshot  (scope)        → ContextSnapshot       pinned + structural state
//! context.get       (id)           → ContextItem          resolve a reference
//! context.checkpoint(scope)        → ContextCheckpoint     deterministic work state
//! context.projection(target, policy) → ScopedSlice         filtered, never raw substrate
//! ```
//!
//! This module owns **what context exists**. It never assembles a prompt, never
//! decides what the model sees, and never writes to a source. It takes
//! `&ReadOnlySource` — the type system is the enforcement: there is no `&mut`
//! path to a source from here (INV-08), so "assembly wrote back helpful state"
//! is not a code path that exists.

use crate::injection::Block;
use crate::recall::Provenance;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A stable reference. Context items are reference-first: an item names its
/// content and the client resolves it on demand (`REQ-CTX-004`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentRef {
    /// The source that owns the content (`repo` · `file` · `memory` · `artifact`
    /// · `world` · `event` · `tool` · `session`).
    pub source: String,
    /// The source's own stable id.
    pub id: String,
    /// A content version, so a changed source is re-read rather than served
    /// from a stale copy.
    pub version: Option<String>,
}

impl ContentRef {
    pub fn new(source: &str, id: &str) -> Self {
        Self {
            source: source.to_string(),
            id: id.to_string(),
            version: None,
        }
    }

    pub fn versioned(source: &str, id: &str, version: &str) -> Self {
        Self {
            source: source.to_string(),
            id: id.to_string(),
            version: Some(version.to_string()),
        }
    }

    pub fn key(&self) -> String {
        match &self.version {
            Some(v) => format!("{}:{}@{}", self.source, self.id, v),
            None => format!("{}:{}", self.source, self.id),
        }
    }
}

/// Where a context item lives (`ARCH/16-CONTEXT.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ContextScope {
    Root,
    Child,
    Task,
    Step,
    Artifact,
    Workspace,
    Project,
    User,
}

/// The sensitivity of a context item — the same canonical vocabulary as memory
/// (`public | personal | confidential`), never a second scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemSensitivity {
    Public,
    Personal,
    Confidential,
}

impl ItemSensitivity {
    pub fn within(self, ceiling: ItemSensitivity) -> bool {
        self <= ceiling
    }

    /// Parse the canonical vocabulary; `normal`/`sensitive` are risk classes and
    /// are rejected here, exactly as on the memory path.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "public" => Some(Self::Public),
            "personal" => Some(Self::Personal),
            "confidential" => Some(Self::Confidential),
            _ => None,
        }
    }
}

/// One context item (`DM-017`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextItem {
    pub id: String,
    pub source: String,
    /// `repo_map` · `file_excerpt` · `tool_result` · `memory_item` ·
    /// `artifact_ref` · `world_state` · `rules` · `checkpoint` · `test_failure`.
    pub item_type: String,
    /// A reference, not an inline copy, whenever possible.
    pub content_ref: ContentRef,
    pub token_cost: u32,
    pub scope: ContextScope,
    pub pinned: bool,
    /// **Reconstructable = may be dropped and rebuilt deterministically.**
    /// A non-reconstructable item is never pruned blindly (`REQ-CTX-006`).
    pub reconstructable: bool,
    pub compressible: bool,
    pub sensitivity: ItemSensitivity,
    /// Ranking inputs, as measured at selection time.
    pub relevance: f64,
    pub freshness: f64,
    pub priority: u32,
    /// A bounded preview, for the `context.search` surface only. A resolved
    /// item is fetched with `get`, so a search result never carries the body.
    pub bounded_snippet: Option<String>,
}

impl ContextItem {
    /// Whether this item may be pruned to fit the budget.
    pub fn is_prunable(&self) -> bool {
        self.reconstructable
    }
}

/// One hit from `context.search`: refs plus a bounded snippet. It carries
/// **no** injection policy and does not re-rank memory results — the injection
/// path is `memory.recall` and each path has exactly one scoring owner
/// (`ARCH/16-CONTEXT.md` §6, C-09).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextCandidate {
    pub content_ref: ContentRef,
    pub scope: ContextScope,
    pub sensitivity: ItemSensitivity,
    pub token_cost: u32,
    pub bounded_snippet: String,
    /// The scoring owner, recorded so a caller can see who ranked this.
    pub scored_by: String,
}

/// A snapshot: pinned + structural state at a point in time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub scope: ContextScope,
    pub taken_at: i64,
    pub items: Vec<ContextItem>,
    /// The recent checkpoint id, so a caller can see how stale the snapshot is.
    pub checkpoint_ref: Option<String>,
}

/// A durable work-state reconstruction point (`DM-006`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextCheckpointRef {
    pub id: String,
    pub version: u32,
    pub reconstructable: bool,
    pub content_ref: ContentRef,
}

/// A read-only source. The `&self` receiver is the enforcement: a source
/// reachable from context assembly can be read and not written (INV-08).
pub trait ReadOnlySource {
    /// The source's stable name.
    fn name(&self) -> &str;
    /// A bounded search: refs + a bounded snippet, never a full body.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<ContextCandidate>, SourceError>;
    /// The pinned + structural state for a scope.
    fn snapshot(&self, scope: ContextScope) -> Result<ContextSnapshot, SourceError>;
    /// Resolve one reference to its item.
    fn get(&self, r: &ContentRef) -> Result<Option<ContextItem>, SourceError>;
    /// The scope keys this source can expose.
    fn scopes(&self) -> Vec<ContextScope>;
}

/// Source failures. They are **typed** so a caller can degrade or surface
/// rather than guess.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("source {0} is unavailable")]
    Unavailable(String),
    #[error("reference not found: {0}")]
    NotFound(String),
    #[error("denied: {0}")]
    Denied(String),
    #[error("malformed query: {0}")]
    MalformedQuery(String),
}

/// A projection policy — deny-by-default for anything not explicitly in scope
/// (`ARCH/16-CONTEXT.md` §1.3, `REQ-CTX-010`, DEC-009, INV-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionPolicy {
    /// The target of the projection (an external agent, a subagent, a lens).
    pub target: String,
    /// The only scopes this projection may include. Anything else is denied.
    pub allowed_scopes: BTreeSet<ContextScope>,
    pub sensitivity_ceiling: ItemSensitivity,
    /// The one project identity in scope. A slice never spans projects.
    pub project_identity: Option<String>,
    /// A recorded loadout is the only way `confidential` is visible.
    pub confidential_loadout: bool,
    /// Whether memory's filtered recall is included at all (v1: recall only).
    pub include_memory_recall: bool,
    /// Whether the transcript may be included. A child gets a bounded snapshot,
    /// never the parent's transcript (`ARCH/16-CONTEXT.md` §8).
    pub allow_transcript: bool,
}

impl ProjectionPolicy {
    /// The v1 default for an external agent: project + own task/step + user,
    /// no org, no other projects, no `confidential` without a loadout.
    pub fn external_agent_default(target: &str, project_identity: &str) -> Self {
        Self {
            target: target.to_string(),
            allowed_scopes: [
                ContextScope::Project,
                ContextScope::Workspace,
                ContextScope::Task,
                ContextScope::Step,
                ContextScope::Artifact,
                ContextScope::User,
            ]
            .into_iter()
            .collect(),
            sensitivity_ceiling: ItemSensitivity::Personal,
            project_identity: Some(project_identity.to_string()),
            confidential_loadout: false,
            include_memory_recall: true,
            allow_transcript: false,
        }
    }

    /// A recorded loadout raises the ceiling to `confidential`. It is explicit
    /// and it is the only way up.
    pub fn with_confidential_loadout(mut self, granted: bool) -> Self {
        self.confidential_loadout = granted;
        if granted {
            self.sensitivity_ceiling = ItemSensitivity::Confidential;
        }
        self
    }

    /// The effective ceiling, after the loadout.
    pub fn effective_ceiling(&self) -> ItemSensitivity {
        if self.confidential_loadout {
            ItemSensitivity::Confidential
        } else {
            self.sensitivity_ceiling
        }
    }
}

/// The verdict on one item under a projection policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Admission {
    Admit,
    /// Out of scope. Deny-by-default: the caller never learns why, only that
    /// it is not there.
    DeniedScope,
    /// Above the sensitivity ceiling.
    DeniedSensitivity,
    /// `confidential` in a projection that does not carry a loadout.
    DeniedNoLoadout,
    /// The transcript, which a child agent never receives.
    DeniedTranscript,
    /// A scope the policy does not name.
    DeniedNotInScope,
}

impl Admission {
    pub fn is_admitted(self) -> bool {
        matches!(self, Admission::Admit)
    }
}

/// A scoped slice — the only shape an external consumer ever sees
/// (`ARCH/16-CONTEXT.md` §1.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopedSlice {
    pub target: String,
    pub project_identity: Option<String>,
    /// References, never the substrate. A slice is a list of refs plus the
    /// bounded previews needed to be useful.
    pub items: Vec<ContextCandidate>,
    pub memory_recall: Option<Block>,
    pub denied: usize,
}

/// Project one source's snapshot through a policy. Deny-by-default: an item
/// outside `allowed_scopes` is dropped without disclosure.
pub fn project_snapshot(
    policy: &ProjectionPolicy,
    items: &[ContextItem],
    memory_recall: Option<Block>,
) -> ScopedSlice {
    let ceiling = policy.effective_ceiling();
    let mut admitted = Vec::new();
    let mut denied = 0usize;
    for item in items {
        match admit(policy, item, ceiling) {
            Admission::Admit => admitted.push(ContextCandidate {
                content_ref: item.content_ref.clone(),
                scope: item.scope,
                sensitivity: item.sensitivity,
                token_cost: item.token_cost,
                bounded_snippet: item.bounded_snippet.clone().unwrap_or_default(),
                scored_by: item.source.clone(),
            }),
            _ => denied += 1,
        }
    }
    ScopedSlice {
        target: policy.target.clone(),
        project_identity: policy.project_identity.clone(),
        items: admitted,
        memory_recall: if policy.include_memory_recall {
            memory_recall
        } else {
            None
        },
        denied,
    }
}

/// The single admission predicate. Every projection path funnels through it, so
/// there is one place to reason about a leak.
pub fn admit(policy: &ProjectionPolicy, item: &ContextItem, ceiling: ItemSensitivity) -> Admission {
    if item.item_type == "transcript" && !policy.allow_transcript {
        return Admission::DeniedTranscript;
    }
    if !policy.allowed_scopes.contains(&item.scope) {
        return Admission::DeniedNotInScope;
    }
    if item.sensitivity == ItemSensitivity::Confidential && !policy.confidential_loadout {
        return Admission::DeniedNoLoadout;
    }
    if !item.sensitivity.within(ceiling) {
        return Admission::DeniedSensitivity;
    }
    Admission::Admit
}

/// The Core context infrastructure surface. Five operations, none of which
/// assembles a prompt or decides what the model sees.
pub struct ContextService<'a> {
    sources: Vec<&'a dyn ReadOnlySource>,
    /// Bounded-snippet length for `search` results.
    pub snippet_tokens: u32,
}

impl<'a> ContextService<'a> {
    pub fn new(sources: Vec<&'a dyn ReadOnlySource>) -> Self {
        Self {
            sources,
            snippet_tokens: 64,
        }
    }

    /// `context.search(query, scopes) → ContextCandidate[]` — refs plus bounded
    /// snippets, across the fronted sources.
    pub fn search(
        &self,
        query: &str,
        scopes: &[ContextScope],
    ) -> Result<Vec<ContextCandidate>, SourceError> {
        if query.trim().is_empty() {
            return Err(SourceError::MalformedQuery("empty query".into()));
        }
        let mut out = Vec::new();
        for s in self.sources.iter() {
            if !s.scopes().iter().any(|sc| scopes.contains(sc)) {
                continue;
            }
            let mut hits = s.search(query, 20)?;
            for h in hits.iter_mut() {
                // Bound the snippet here, at the search surface: a search result
                // is a ref plus a preview, never a body.
                h.bounded_snippet = bound_text(&h.bounded_snippet, self.snippet_tokens);
            }
            out.extend(hits);
        }
        Ok(out)
    }

    /// `context.snapshot(scope) → ContextSnapshot`.
    pub fn snapshot(&self, scope: ContextScope) -> Result<ContextSnapshot, SourceError> {
        let mut items = Vec::new();
        let mut taken_at = 0i64;
        for s in self.sources.iter() {
            if !s.scopes().contains(&scope) {
                continue;
            }
            let snap = s.snapshot(scope)?;
            taken_at = taken_at.max(snap.taken_at);
            items.extend(snap.items);
        }
        Ok(ContextSnapshot {
            scope,
            taken_at,
            items,
            checkpoint_ref: None,
        })
    }

    /// `context.get(id) → ContextItem` — resolve a reference to content on
    /// demand.
    pub fn get(&self, r: &ContentRef) -> Result<Option<ContextItem>, SourceError> {
        for s in self.sources.iter() {
            if s.name() != r.source {
                continue;
            }
            return s.get(r);
        }
        Ok(None)
    }

    /// `context.projection(target, policy) → ScopedSlice`.
    pub fn projection(
        &self,
        policy: &ProjectionPolicy,
        scope: ContextScope,
        memory_recall: Option<Block>,
    ) -> Result<ScopedSlice, SourceError> {
        if policy.allowed_scopes.is_empty() {
            return Ok(ScopedSlice {
                target: policy.target.clone(),
                project_identity: policy.project_identity.clone(),
                items: Vec::new(),
                memory_recall: None,
                denied: 0,
            });
        }
        if !policy.allowed_scopes.contains(&scope) {
            // Deny-by-default: an out-of-scope request is refused, and the
            // caller is told nothing about what exists.
            return Err(SourceError::Denied(format!(
                "{}: scope {scope:?} is not in the projection",
                policy.target
            )));
        }
        let snap = self.snapshot(scope)?;
        Ok(project_snapshot(policy, &snap.items, memory_recall))
    }

    /// The source names this service fronts, for the interop census.
    pub fn source_names(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.name()).collect()
    }
}

/// Bound a snippet to a token budget, marking the cut so a reader can see the
/// preview is partial.
pub fn bound_text(text: &str, tokens: u32) -> String {
    let max_chars = tokens as usize * 4;
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}…")
}

/// Render a recall response's provenance as context candidates, for a
/// projection. Provenance travels with the ref; a missing source is annotated
/// rather than dereferenced.
pub fn recall_provenance_to_candidates(
    provenance: &[Provenance],
    source_name: &str,
) -> Vec<ContextCandidate> {
    provenance
        .iter()
        .map(|p| ContextCandidate {
            content_ref: ContentRef::new(source_name, &p.id),
            scope: match p.scope.scope {
                crate::scope::Scope::Session => ContextScope::Task,
                crate::scope::Scope::Task => ContextScope::Task,
                crate::scope::Scope::Project => ContextScope::Project,
                crate::scope::Scope::User => ContextScope::User,
                crate::scope::Scope::Org => ContextScope::Root,
            },
            sensitivity: match p.sensitivity {
                crate::scope::Sensitivity::Public => ItemSensitivity::Public,
                crate::scope::Sensitivity::Personal => ItemSensitivity::Personal,
                crate::scope::Sensitivity::Confidential => ItemSensitivity::Confidential,
            },
            token_cost: 0,
            bounded_snippet: if p.source_unavailable {
                "source unavailable".to_string()
            } else {
                String::new()
            },
            scored_by: "memory.recall".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        items: Vec<ContextItem>,
        available: bool,
    }

    impl ReadOnlySource for Fixture {
        fn name(&self) -> &str {
            "fixture"
        }
        fn search(&self, query: &str, limit: usize) -> Result<Vec<ContextCandidate>, SourceError> {
            if !self.available {
                return Err(SourceError::Unavailable("fixture".into()));
            }
            Ok(self
                .items
                .iter()
                .filter(|i| i.content_ref.id.contains(query))
                .take(limit)
                .map(|i| ContextCandidate {
                    content_ref: i.content_ref.clone(),
                    scope: i.scope,
                    sensitivity: i.sensitivity,
                    token_cost: i.token_cost,
                    bounded_snippet: i
                        .bounded_snippet
                        .clone()
                        .unwrap_or_else(|| "x".repeat(1000)),
                    scored_by: "fixture.search".into(),
                })
                .collect())
        }
        fn snapshot(&self, scope: ContextScope) -> Result<ContextSnapshot, SourceError> {
            if !self.available {
                return Err(SourceError::Unavailable("fixture".into()));
            }
            Ok(ContextSnapshot {
                scope,
                taken_at: 1_000,
                items: self
                    .items
                    .iter()
                    .filter(|i| i.scope == scope)
                    .cloned()
                    .collect(),
                checkpoint_ref: Some("ckpt:1".into()),
            })
        }
        fn get(&self, r: &ContentRef) -> Result<Option<ContextItem>, SourceError> {
            Ok(self.items.iter().find(|i| i.content_ref == *r).cloned())
        }
        fn scopes(&self) -> Vec<ContextScope> {
            vec![ContextScope::Project, ContextScope::Task]
        }
    }

    fn item(id: &str, scope: ContextScope, sens: ItemSensitivity) -> ContextItem {
        ContextItem {
            id: id.into(),
            source: "fixture".into(),
            item_type: "memory_item".into(),
            content_ref: ContentRef::new("fixture", id),
            token_cost: 10,
            scope,
            pinned: false,
            reconstructable: true,
            compressible: true,
            sensitivity: sens,
            relevance: 1.0,
            freshness: 1.0,
            priority: 0,
            bounded_snippet: Some(id.to_string()),
        }
    }

    fn fixture() -> Fixture {
        Fixture {
            items: vec![
                item(
                    "proj-fact",
                    ContextScope::Project,
                    ItemSensitivity::Personal,
                ),
                item(
                    "proj-secret",
                    ContextScope::Project,
                    ItemSensitivity::Confidential,
                ),
                item("task-fact", ContextScope::Task, ItemSensitivity::Personal),
                item("root-fact", ContextScope::Root, ItemSensitivity::Public),
            ],
            available: true,
        }
    }

    #[test]
    fn search_returns_bounded_snippets_not_bodies() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        let hits = svc.search("proj", &[ContextScope::Project]).unwrap();
        assert!(!hits.is_empty());
        for h in &hits {
            assert!(h.bounded_snippet.chars().count() <= 64 * 4 + 1);
        }
    }

    #[test]
    fn an_empty_query_is_a_typed_error_not_a_wildcard_search() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        assert!(matches!(
            svc.search("   ", &[ContextScope::Project]),
            Err(SourceError::MalformedQuery(_))
        ));
    }

    #[test]
    fn get_resolves_a_reference_and_a_wrong_source_returns_none() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        // A ref naming the right source resolves.
        assert!(
            svc.get(&ContentRef::new("fixture", "task-fact"))
                .unwrap()
                .is_some()
        );
        // A ref naming an unknown source resolves to nothing rather than
        // reaching into another source.
        assert!(
            svc.get(&ContentRef::new("other", "task-fact"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_projection_is_deny_by_default_for_out_of_scope_requests() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        let policy = ProjectionPolicy::external_agent_default("agent:ag", "p1");
        // A root-scoped request is refused with a typed error.
        let err = svc
            .projection(&policy, ContextScope::Root, None)
            .unwrap_err();
        assert!(matches!(err, SourceError::Denied(_)));
    }

    #[test]
    fn a_projection_contains_no_org_no_other_project_and_no_confidential() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        let policy = ProjectionPolicy::external_agent_default("agent:ag", "p1");
        let slice = svc
            .projection(&policy, ContextScope::Project, None)
            .unwrap();
        let ids: Vec<&str> = slice
            .items
            .iter()
            .map(|i| i.content_ref.id.as_str())
            .collect();
        assert!(ids.contains(&"proj-fact"));
        assert!(
            !ids.contains(&"proj-secret"),
            "confidential needs a loadout"
        );
        assert!(!ids.contains(&"root-fact"));
        // Only the confidential project item was in this snapshot and it was
        // denied; nothing else leaked.
        assert_eq!(slice.denied, 1);
    }

    #[test]
    fn a_recorded_loadout_is_the_only_way_confidential_enters_a_projection() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        let policy = ProjectionPolicy::external_agent_default("agent:ag", "p1")
            .with_confidential_loadout(true);
        let slice = svc
            .projection(&policy, ContextScope::Project, None)
            .unwrap();
        let ids: Vec<&str> = slice
            .items
            .iter()
            .map(|i| i.content_ref.id.as_str())
            .collect();
        assert!(ids.contains(&"proj-secret"));
    }

    #[test]
    fn a_child_never_receives_the_parent_transcript() {
        let policy = ProjectionPolicy::external_agent_default("child:1", "p1");
        let mut t = item("transcript", ContextScope::Task, ItemSensitivity::Personal);
        t.item_type = "transcript".into();
        assert_eq!(
            admit(&policy, &t, policy.effective_ceiling()),
            Admission::DeniedTranscript
        );
    }

    #[test]
    fn memory_recall_appears_only_when_the_policy_allows_it() {
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        let block = Block {
            name: "memory_relevant".into(),
            text: "- [user|user|personal] a fact".into(),
            tokens: 8,
            items: vec![],
            dropped: vec![],
            ceiling: 256,
        };
        let with = ProjectionPolicy::external_agent_default("agent:ag", "p1");
        let slice = svc
            .projection(&with, ContextScope::Project, Some(block.clone()))
            .unwrap();
        assert!(slice.memory_recall.is_some());
        let without = ProjectionPolicy {
            include_memory_recall: false,
            ..ProjectionPolicy::external_agent_default("agent:ag", "p1")
        };
        let slice = svc
            .projection(&without, ContextScope::Project, Some(block))
            .unwrap();
        assert!(slice.memory_recall.is_none());
    }

    #[test]
    fn the_service_exposes_no_policy_about_what_the_model_sees() {
        // The Core surface is search/snapshot/get/checkpoint/projection. There is
        // deliberately no `assemble`, `select`, `prune`, `compact`, `pin` or
        // `exclude` here: those belong to the agent's control layer
        // (`REQ-CTX-003`, DEC-007). The test pins that the only mutating-looking
        // verb a caller can reach is a projection request.
        let f = fixture();
        let svc = ContextService::new(vec![&f]);
        assert_eq!(svc.source_names(), vec!["fixture"]);
        // The five documented operations, and nothing else that assembles.
        assert!(
            svc.search("proj", &[ContextScope::Project]).is_ok(),
            "context.search"
        );
        assert!(
            svc.snapshot(ContextScope::Project).is_ok(),
            "context.snapshot"
        );
        assert!(
            svc.get(&ContentRef::new("fixture", "proj-fact")).is_ok(),
            "context.get"
        );
        // Two calls with the same input produce the same slice: a projection is
        // a pure function of the source state and the policy.
        let policy = ProjectionPolicy::external_agent_default("agent:ag", "p1");
        let a = svc
            .projection(&policy, ContextScope::Project, None)
            .unwrap();
        let b = svc
            .projection(&policy, ContextScope::Project, None)
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn sensitivity_vocabulary_is_canonical_and_shared_with_memory() {
        assert_eq!(
            ItemSensitivity::parse("normal"),
            None,
            "a risk class is not a sensitivity class"
        );
        assert_eq!(
            ItemSensitivity::parse("confidential"),
            Some(ItemSensitivity::Confidential)
        );
    }

    #[test]
    fn an_unavailable_source_is_a_typed_error_the_caller_can_degrade_on() {
        let f = Fixture {
            items: vec![],
            available: false,
        };
        let svc = ContextService::new(vec![&f]);
        assert!(matches!(
            svc.snapshot(ContextScope::Project),
            Err(SourceError::Unavailable(_))
        ));
    }

    #[test]
    fn bounding_marks_the_cut() {
        assert_eq!(bound_text("hello", 64), "hello");
        let long = "x".repeat(1_000);
        let b = bound_text(&long, 4);
        assert!(b.ends_with('…'));
        assert_eq!(b.chars().count(), 17);
    }
}
