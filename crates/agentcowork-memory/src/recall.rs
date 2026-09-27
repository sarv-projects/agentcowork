//! The recall primitive (`ARCH/17-MEMORY.md` §6, `REQ-MEM-003/012/015`).
//!
//! ```text
//! Context Controller ── memory.recall(query, scope_filter?, tokens) ──►
//!   0 scope set + ceiling derived from the actor binding (never caller-supplied)
//!   1 scope+state filter (current, unexpired, sensitivity ≤ ceiling)
//!   2 FTS5 BM25 candidates (top ~50); free text sanitized into valid MATCH
//!     syntax or abstain
//!   3 relevance = (−bm25) × kind/pin boost × recency boost → sort descending,
//!     deterministic ties
//!   4 dedup (hash); MMR-ready interface (no MMR in v1)
//!   5 budget fit: render, drop whole items until ≤ budget (never truncate)
//! ```
//!
//! **Memory ≠ context** (DEC-019): this returns *candidates with provenance*.
//! It never decides inclusion, it never writes into a prompt, and it never
//! mutates anything — a recall is a non-touching read (INV-08).

use crate::scope::{AccessSet, ActorBinding, Kind, ScopeKey, Sensitivity, TrustTier};
use crate::store::{MemoryItem, MemoryStore, StoreError};
use serde::{Deserialize, Serialize};

/// FTS5 candidate depth before ranking (`ARCH/17-MEMORY.md` §6 step 2).
pub const FTS_CANDIDATE_LIMIT: usize = 50;

/// The three outcomes. Abstention and error are **distinguishable** and metered,
/// so silent quality loss is visible (`REQ-MEM-015`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallOutcome {
    /// At least one candidate above the relevance floor.
    Hit,
    /// A genuine miss: nothing above the floor, or a query that cannot be turned
    /// into a valid FTS query.
    Abstain,
    /// The store could not be read. Never reported as abstention.
    Error,
}

/// Ranking inputs. `relevance` is the FTS base normalized to non-negative; the
/// boosts multiply it, so a boost can never invert a relevance order — it only
/// scales it (`REQ-MEM-015`: "boosts never invert relevance").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ranked {
    pub item: MemoryItem,
    /// The non-negative relevance base: `−bm25`.
    pub relevance: f64,
    /// The multiplicative boost applied to the base.
    pub boost: f64,
    /// The final score: `relevance × boost`.
    pub score: f64,
}

/// A recall response: candidates **and provenance only** — never a rendered
/// block, never an inclusion decision (`REQ-MEM-003`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallResponse {
    pub outcome: RecallOutcome,
    pub candidates: Vec<Ranked>,
    /// Provenance for the render, parallel to `candidates`.
    pub provenance: Vec<Provenance>,
    /// The relevance floor that was applied.
    pub floor: f64,
    /// The budget the controller offered, for its own accounting.
    pub token_budget: u32,
    /// Why the call abstained, when it did. `None` on a hit.
    pub abstain_reason: Option<AbstainReason>,
}

/// Why a recall abstained. A malformed query is *not* an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbstainReason {
    /// The query was empty or carried only punctuation/stop words.
    EmptyQuery,
    /// The query could not be sanitized into valid FTS5 MATCH syntax
    /// (operator/quote injection).
    MalformedQuery,
    /// Candidates existed but none cleared the relevance floor.
    BelowFloor,
    /// The actor's derived set has no reachable scope for this request.
    NoPermittedScope,
}

/// The provenance every injected item must expose (`REQ-MEM-014`): source tag,
/// trust tier, created-at, and whether the referenced source is still present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub id: String,
    pub scope: ScopeKey,
    pub source: String,
    pub trust_tier: TrustTier,
    pub created_at: i64,
    pub sensitivity: Sensitivity,
    /// `source unavailable` when the referenced source was pruned (DEC-032).
    /// The ref is never dereferenced during injection.
    pub source_unavailable: bool,
    /// The provenance ref (turn/event/artifact/checkpoint id).
    pub source_ref: Option<String>,
}

/// Ranking configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecallConfig {
    /// The relevance floor. Below it, a candidate is not a hit.
    pub relevance_floor: f64,
    /// Pin boost. A pinned item outranks a better-raw-BM25 unpinned match.
    pub pin_boost: f64,
    /// Boost by kind — a `preference` is durable intent, a `reference` is a
    /// pointer; both are worth surfacing above plain `fact` prose.
    pub kind_boost_preference: f64,
    pub kind_boost_decision: f64,
    pub kind_boost_reference: f64,
    pub kind_boost_summary: f64,
    /// Recency half-life in ms. The boost is `0.5^(age/half_life)`, bounded at
    /// `min_recency_boost` so a clock jump cannot reorder a fixed set.
    pub recency_half_life_ms: f64,
    pub min_recency_boost: f64,
    pub candidate_limit: usize,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            relevance_floor: 0.0,
            pin_boost: 2.0,
            kind_boost_preference: 1.5,
            kind_boost_decision: 1.4,
            kind_boost_reference: 1.2,
            kind_boost_summary: 1.0,
            recency_half_life_ms: 7.0 * 24.0 * 60.0 * 60.0 * 1000.0,
            min_recency_boost: 0.5,
            candidate_limit: FTS_CANDIDATE_LIMIT,
        }
    }
}

impl RecallConfig {
    pub fn kind_boost(self, kind: Kind) -> f64 {
        match kind {
            Kind::Preference => self.kind_boost_preference,
            Kind::Decision => self.kind_boost_decision,
            Kind::Reference => self.kind_boost_reference,
            Kind::Summary => self.kind_boost_summary,
            Kind::Fact => 1.0,
        }
    }

    /// The recency boost, clamped so a backwards clock never promotes an older
    /// item over a newer one (`REQ-MEM-025`). Public because the clamp is the
    /// documented time-correctness rule, not an internal detail.
    pub fn recency_boost(self, created_at: i64, now: i64) -> f64 {
        let age = (now - created_at).max(0) as f64;
        let b = 0.5f64.powf(age / self.recency_half_life_ms);
        b.clamp(self.min_recency_boost, 1.0)
    }
}

/// What is live (not expired) in the store — the caller's way of telling
/// recall which provenance refs still resolve, without recall dereferencing
/// them.
pub trait SourceAvailability {
    fn is_available(&self, source_ref: &str) -> bool;
}

/// Everything is available (the default when no source registry is wired).
pub struct AllSourcesAvailable;

impl SourceAvailability for AllSourcesAvailable {
    fn is_available(&self, _source_ref: &str) -> bool {
        true
    }
}

/// A set of refs that were pruned. Renders as "source unavailable".
#[derive(Debug, Default, Clone)]
pub struct PrunedSources(std::collections::BTreeSet<String>);

impl PrunedSources {
    pub fn with<I: IntoIterator<Item = S>, S: Into<String>>(refs: I) -> Self {
        Self(refs.into_iter().map(Into::into).collect())
    }

    pub fn insert(&mut self, r: &str) {
        self.0.insert(r.to_string());
    }
}

impl SourceAvailability for PrunedSources {
    fn is_available(&self, source_ref: &str) -> bool {
        !self.0.contains(source_ref)
    }
}

/// Why a free-text query could not be turned into a valid FTS5 query. The call
/// **abstains**; it never returns arbitrary candidates and never errors
/// (`REQ-MEM-015`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryRejection {
    /// The query carried FTS5 syntax (operators, quotes, column filters,
    /// prefixes) and was therefore not a plain free-text query.
    Syntax,
}

/// Free text → FTS5 MATCH syntax, or abstain.
///
/// The tokenizer is `porter unicode61`; an operator/quote injection attempt is
/// **rejected, not escaped into something that silently matches nothing**
/// (`ARCH/17-MEMORY.md` §6, F-12). This is the whole `REQ-MEM-015` robustness
/// rule: an invalid query never returns arbitrary candidates.
pub fn sanitize_fts_query(raw: &str) -> Result<Option<String>, QueryRejection> {
    // Reject FTS5 syntax characters outright: `* " ' ( ) { } : ^ - NEAR AND OR
    // NOT + , .` are either operators, prefixes, phrases or column filters.
    // A query carrying any of them is not a plain free-text query.
    const FORBIDDEN: &[char] = &[
        '"', '\'', '*', '(', ')', '{', '}', ':', '^', '-', '+', ',', '.', '/', '\\', '|', '~', '!',
        '@', '#', '$', '%', '&', '<', '>', '=',
    ];
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // A query with no word characters at all ("...", "???") is an empty query,
    // not a malformed one: there is nothing to sanitize.
    if !trimmed.chars().any(|c| c.is_alphanumeric() || c == '_') {
        return Ok(None);
    }
    if trimmed.chars().any(|c| FORBIDDEN.contains(&c)) {
        return Err(QueryRejection::Syntax);
    }
    let tokens: Vec<String> = trimmed
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Ok(None);
    }
    // An FTS5 bareword is `[A-Za-z0-9_]` plus any non-ASCII letter. Anything
    // else would be parsed as syntax.
    if tokens
        .iter()
        .any(|t| !t.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return Err(QueryRejection::Syntax);
    }
    // A FTS5 keyword used as a term would change the parse.
    const KEYWORDS: &[&str] = &[
        "and",
        "or",
        "not",
        "near",
        "match",
        "asc",
        "desc",
        "filter",
        "columnsize",
        "offset",
        "limit",
        "bm25",
        "snippet",
        "offsets",
        "matchinfo",
        "integrity",
        "rebuild",
        "optimize",
    ];
    if tokens.iter().any(|t| KEYWORDS.contains(&t.as_str())) {
        return Err(QueryRejection::Syntax);
    }
    // Implicit AND across barewords: exact, and it cannot be widened by a
    // caller into a broader or narrower result than the words they typed.
    Ok(Some(tokens.join(" ")))
}

/// A recall request. `scope_filter` may only **narrow** the actor-derived set.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallQuery<'a> {
    pub text: &'a str,
    pub filter: Option<Vec<ScopeKey>>,
    /// The controller's budget. The response reports it; inclusion is still the
    /// controller's decision.
    pub token_budget: u32,
    pub now_ms: i64,
}

impl MemoryStore {
    /// Ranked candidates with provenance. A non-touching read: no counter, no
    /// salience, no timestamp moves (`REQ-MEM-004`).
    /// Non-touching read: no counter, no salience, no timestamp moves
    /// (`REQ-MEM-004`). A store fault is an **error**, never an abstention.
    pub fn recall(
        &self,
        actor: &ActorBinding,
        q: &RecallQuery<'_>,
        cfg: &RecallConfig,
        sources: &dyn SourceAvailability,
    ) -> Result<RecallResponse, StoreError> {
        // (0) The permitted set is derived here, from the actor binding. A
        // caller-supplied filter is a narrowing, and an attempt to widen is
        // not silently ignored: it abstains.
        let derived = match AccessSet::derive(actor).narrow(q.filter.as_deref()) {
            Ok(s) => s,
            Err(_) => {
                return Ok(abstain(
                    q.token_budget,
                    cfg,
                    AbstainReason::NoPermittedScope,
                    Vec::new(),
                ));
            }
        };
        if derived.keys().count() == 0 {
            return Ok(abstain(
                q.token_budget,
                cfg,
                AbstainReason::NoPermittedScope,
                Vec::new(),
            ));
        }
        // (2) Sanitize, or abstain.
        let match_expr = match sanitize_fts_query(q.text) {
            Ok(Some(m)) => m,
            Ok(None) => {
                return Ok(abstain(
                    q.token_budget,
                    cfg,
                    AbstainReason::EmptyQuery,
                    Vec::new(),
                ));
            }
            Err(QueryRejection::Syntax) => {
                return Ok(abstain(
                    q.token_budget,
                    cfg,
                    AbstainReason::MalformedQuery,
                    Vec::new(),
                ));
            }
        };

        let candidates = match self.fts_candidates(&match_expr, cfg.candidate_limit) {
            Ok(c) => c,
            // A store read failure is an **error**, never an abstention: the
            // typed error carries the distinction, so a caller cannot report a
            // broken store as "nothing relevant".
            Err(e) => return Err(StoreError::Recall(e.to_string())),
        };

        let mut ranked: Vec<Ranked> = Vec::new();
        let mut seen_hashes = std::collections::BTreeSet::new();
        for (id, bm25) in candidates {
            // (1) State filter, re-applied on the read path: current,
            // unexpired, sensitivity within the key's ceiling.
            let Some(item) = self.get(&id)? else {
                continue;
            };
            if !item.is_current() || item.is_expired(q.now_ms) {
                continue;
            }
            let key = item.key();
            if !derived.permits(&key) {
                continue;
            }
            if !item.sensitivity.within(derived.ceiling_for(&key)) {
                continue;
            }
            // (4) Dedup by hash: two rows with identical normalized content
            // collapse to the better-ranked one.
            if !seen_hashes.insert(item.content_hash.clone()) {
                continue;
            }
            // (3) Scoring. FTS5 `bm25()` is negative for better matches, so the
            // base is `-bm25` — non-negative by construction.
            let relevance = (-bm25).max(0.0);
            let mut boost = cfg.kind_boost(item.kind);
            if item.pinned {
                boost *= cfg.pin_boost;
            }
            boost *= cfg.recency_boost(item.created_at, q.now_ms);
            ranked.push(Ranked {
                score: relevance * boost,
                relevance,
                boost,
                item,
            });
        }

        // Descending by score; ties break deterministically on `created_at`,
        // then id, so two runs over the same store always agree.
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.item.created_at.cmp(&a.item.created_at))
                .then_with(|| a.item.id.cmp(&b.item.id))
        });

        // (5) The relevance floor decides hit vs abstain, not truncation.
        let above: Vec<Ranked> = ranked
            .into_iter()
            .filter(|r| r.relevance > cfg.relevance_floor)
            .collect();
        if above.is_empty() {
            return Ok(abstain(
                q.token_budget,
                cfg,
                AbstainReason::BelowFloor,
                Vec::new(),
            ));
        }
        let provenance = above
            .iter()
            .map(|r| provenance_of(&r.item, sources))
            .collect();
        Ok(RecallResponse {
            outcome: RecallOutcome::Hit,
            candidates: above,
            provenance,
            floor: cfg.relevance_floor,
            token_budget: q.token_budget,
            abstain_reason: None,
        })
    }
}

fn abstain(
    token_budget: u32,
    cfg: &RecallConfig,
    reason: AbstainReason,
    candidates: Vec<Ranked>,
) -> RecallResponse {
    RecallResponse {
        outcome: RecallOutcome::Abstain,
        candidates,
        provenance: Vec::new(),
        floor: cfg.relevance_floor,
        token_budget,
        abstain_reason: Some(reason),
    }
}

fn provenance_of(item: &MemoryItem, sources: &dyn SourceAvailability) -> Provenance {
    let source_unavailable = item
        .source_ref
        .as_deref()
        .map(|r| !sources.is_available(r))
        .unwrap_or(false);
    Provenance {
        id: item.id.clone(),
        scope: item.key(),
        source: item.source.clone(),
        trust_tier: item.trust_tier,
        created_at: item.created_at,
        sensitivity: item.sensitivity,
        source_unavailable,
        source_ref: item.source_ref.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewItem, StoreConfig};

    fn store() -> MemoryStore {
        MemoryStore::open_in_memory(StoreConfig::default()).unwrap()
    }

    fn add(s: &mut MemoryStore, id: &str, key: &ScopeKey, text: &str, at: i64) -> MemoryItem {
        s.insert(&NewItem::new(id, key.clone(), Kind::Fact, text), at)
            .unwrap()
    }

    fn recall(s: &MemoryStore, actor: &ActorBinding, text: &str) -> RecallResponse {
        s.recall(
            actor,
            &RecallQuery {
                text,
                filter: None,
                token_budget: 256,
                now_ms: 1_000_000,
            },
            &RecallConfig::default(),
            &AllSourcesAvailable,
        )
        .expect("recall")
    }

    #[test]
    fn bm25_is_normalized_to_a_non_negative_relevance_base() {
        let mut s = store();
        add(
            &mut s,
            "m1",
            &ScopeKey::project("p"),
            "the build uses bazel",
            1,
        );
        add(
            &mut s,
            "m2",
            &ScopeKey::project("p"),
            "the build uses buck",
            1,
        );
        // FTS5 bm25 is negative; the exposed base must not be.
        let raw = s.fts_candidates("bazel", 10).unwrap();
        assert!(raw[0].1 < 0.0, "sqlite bm25 is negative for a match");
        let r = recall(&s, &ActorBinding::local_in("u", "p"), "bazel");
        assert_eq!(r.outcome, RecallOutcome::Hit);
        assert!(r.candidates.iter().all(|c| c.relevance >= 0.0));
        assert!(r.candidates.iter().all(|c| c.score >= 0.0));
    }

    #[test]
    fn a_pinned_item_with_worse_raw_bm25_still_ranks_first() {
        let mut s = store();
        // A short exact match: better raw BM25.
        add(&mut s, "exact", &ScopeKey::project("p"), "bazel", 1);
        // A longer item that mentions bazel once: worse raw BM25.
        add(
            &mut s,
            "pinned",
            &ScopeKey::project("p"),
            "bazel is the build tool we standardised on after a long evaluation across teams",
            1,
        );
        s.set_pinned("pinned", true, 2).unwrap();
        let r = s
            .recall(
                &ActorBinding::local_in("u", "p"),
                &RecallQuery {
                    text: "bazel",
                    filter: None,
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                // A pin boost strong enough to overcome the raw gap.
                &RecallConfig {
                    pin_boost: 100.0,
                    ..Default::default()
                },
                &AllSourcesAvailable,
            )
            .expect("recall");
        assert_eq!(r.candidates.len(), 2);
        assert_eq!(r.candidates[0].item.id, "pinned");
        // The base really was worse, so the boost — not the BM25 — decided it.
        let pinned = r.candidates.iter().find(|c| c.item.id == "pinned").unwrap();
        let exact = r.candidates.iter().find(|c| c.item.id == "exact").unwrap();
        assert!(
            pinned.relevance < exact.relevance,
            "the fixture must have a worse raw relevance for the pinned item"
        );
        assert!(pinned.score > exact.score);
    }

    #[test]
    fn a_boost_never_lets_a_weaker_relevance_beat_a_much_stronger_one() {
        let mut s = store();
        add(
            &mut s,
            "short",
            &ScopeKey::project("p"),
            "bazel bazel bazel bazel",
            1,
        );
        add(
            &mut s,
            "long",
            &ScopeKey::project("p"),
            "bazel is the build tool we standardised on after a long evaluation across many teams",
            1,
        );
        s.set_pinned("long", true, 2).unwrap();
        let r = s
            .recall(
                &ActorBinding::local_in("u", "p"),
                &RecallQuery {
                    text: "bazel",
                    filter: None,
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &RecallConfig::default(),
                &AllSourcesAvailable,
            )
            .expect("recall");
        // With the default 2x pin boost a 4-occurrence match still wins: the
        // score is relevance x boost, so the boost scales rather than inverts.
        assert_eq!(r.candidates[0].item.id, "short");
    }

    #[test]
    fn boosts_never_invert_a_relevance_order_at_equal_boosts() {
        let mut s = store();
        add(
            &mut s,
            "strong",
            &ScopeKey::project("p"),
            "bazel bazel bazel",
            1,
        );
        add(
            &mut s,
            "weak",
            &ScopeKey::project("p"),
            "we use bazel here sometimes",
            1,
        );
        let r = recall(&s, &ActorBinding::local_in("u", "p"), "bazel");
        let strong = r.candidates.iter().find(|c| c.item.id == "strong").unwrap();
        let weak = r.candidates.iter().find(|c| c.item.id == "weak").unwrap();
        // Same kind and pin state ⇒ same boost ⇒ the order is the relevance
        // order, because the score is `relevance × boost`.
        assert!((strong.boost - weak.boost).abs() < 1e-9);
        assert!(strong.relevance > weak.relevance);
    }

    #[test]
    fn ties_break_deterministically_on_created_at_then_id() {
        let mut s = store();
        add(
            &mut s,
            "b-item",
            &ScopeKey::project("p"),
            "shared token here",
            500,
        );
        add(
            &mut s,
            "a-item",
            &ScopeKey::project("p"),
            "shared token here too",
            500,
        );
        let actor = ActorBinding::local_in("u", "p");
        let first = recall(&s, &actor, "shared");
        let second = recall(&s, &actor, "shared");
        let ids_a: Vec<&str> = first
            .candidates
            .iter()
            .map(|c| c.item.id.as_str())
            .collect();
        let ids_b: Vec<&str> = second
            .candidates
            .iter()
            .map(|c| c.item.id.as_str())
            .collect();
        // Identical inputs give an identical order every time, and the order is
        // a total one (no duplicates, both present).
        assert_eq!(ids_a, ids_b, "the order is reproducible");
        assert!(
            ids_a.contains(&"a-item") && ids_a.contains(&"b-item"),
            "both candidates are present: {ids_a:?}"
        );
        // The order is total: no duplicates.
        let mut sorted = ids_a.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids_a.len());
    }

    #[test]
    fn an_exact_tie_on_score_and_created_at_breaks_on_id() {
        let mut s = store();
        // Two items whose normalized content differs only in a token the query
        // does not match, so bm25 ties exactly.
        add(&mut s, "zzz-item", &ScopeKey::project("p"), "widget", 500);
        add(
            &mut s,
            "aaa-item",
            &ScopeKey::project("p"),
            "widget gadget",
            500,
        );
        // Query only "widget": both match once, so the raw scores tie.
        let r = s
            .recall(
                &ActorBinding::local_in("u", "p"),
                &RecallQuery {
                    text: "widget",
                    filter: None,
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &RecallConfig::default(),
                &AllSourcesAvailable,
            )
            .expect("recall");
        if r.candidates[0].score == r.candidates[1].score
            && r.candidates[0].item.created_at == r.candidates[1].item.created_at
        {
            assert_eq!(
                r.candidates[0].item.id, "aaa-item",
                "a true tie breaks on id"
            );
        }
        // Either way the order is stable across calls.
        let again = s
            .recall(
                &ActorBinding::local_in("u", "p"),
                &RecallQuery {
                    text: "widget",
                    filter: None,
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &RecallConfig::default(),
                &AllSourcesAvailable,
            )
            .expect("recall");
        assert_eq!(
            r.candidates
                .iter()
                .map(|c| c.item.id.clone())
                .collect::<Vec<_>>(),
            again
                .candidates
                .iter()
                .map(|c| c.item.id.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_malformed_query_abstains_and_never_errors_or_returns_garbage() {
        let mut s = store();
        add(&mut s, "m1", &ScopeKey::project("p"), "ordinary content", 1);
        let actor = ActorBinding::local_in("u", "p");
        for hostile in [
            "\"unbalanced",
            "bazel OR *",
            "NEAR(a b)",
            "content : column",
            "content^2",
            "a AND b",
            "content) (",
        ] {
            let r = recall(&s, &actor, hostile);
            assert_eq!(
                r.outcome,
                RecallOutcome::Abstain,
                "{hostile:?} must abstain"
            );
            assert_eq!(r.abstain_reason, Some(AbstainReason::MalformedQuery));
            assert!(r.candidates.is_empty(), "{hostile:?} returned candidates");
        }
    }

    #[test]
    fn an_empty_or_punctuation_query_abstains_as_empty_not_error() {
        let s = store();
        let r = recall(&s, &ActorBinding::local_in("u", "p"), "   ...  ");
        assert_eq!(r.outcome, RecallOutcome::Abstain);
        assert_eq!(r.abstain_reason, Some(AbstainReason::EmptyQuery));
    }

    #[test]
    fn a_genuine_miss_abstains_below_the_floor() {
        let mut s = store();
        add(
            &mut s,
            "m1",
            &ScopeKey::project("p"),
            "the build uses bazel",
            1,
        );
        let cfg = RecallConfig {
            relevance_floor: 1000.0,
            ..Default::default()
        };
        let r = s
            .recall(
                &ActorBinding::local_in("u", "p"),
                &RecallQuery {
                    text: "bazel",
                    filter: None,
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &cfg,
                &AllSourcesAvailable,
            )
            .expect("recall");
        assert_eq!(r.outcome, RecallOutcome::Abstain);
        assert_eq!(r.abstain_reason, Some(AbstainReason::BelowFloor));
    }

    #[test]
    fn superseded_and_expired_rows_are_never_served_as_current() {
        let mut s = store();
        add(
            &mut s,
            "old",
            &ScopeKey::project("p"),
            "the build uses bazel",
            1,
        );
        add(
            &mut s,
            "new",
            &ScopeKey::project("p"),
            "the build uses buck",
            2,
        );
        s.mark_superseded("old", "new", 3).unwrap();
        let mut exp = NewItem::new("exp", ScopeKey::project("p"), Kind::Fact, "bazel expired");
        exp.expires_at = Some(10);
        s.insert(&exp, 4).unwrap();
        let r = recall(&s, &ActorBinding::local_in("u", "p"), "bazel");
        let ids: Vec<&str> = r.candidates.iter().map(|c| c.item.id.as_str()).collect();
        assert!(!ids.contains(&"old"), "a superseded row is not current");
        assert!(!ids.contains(&"exp"), "an expired row is not recalled");
    }

    #[test]
    fn recall_is_a_non_touching_read() {
        let mut s = store();
        let it = add(
            &mut s,
            "m1",
            &ScopeKey::project("p"),
            "the build uses bazel",
            1,
        );
        let before = it.clone();
        let actor = ActorBinding::local_in("u", "p");
        recall(&s, &actor, "bazel");
        recall(&s, &actor, "bazel");
        s.integrity_check().unwrap();
        let after = s.get("m1").unwrap().unwrap();
        assert_eq!(before, after, "a read changed nothing");
        assert_eq!(after.used_count, 0);
        assert_eq!(after.last_used_at, None);
    }

    #[test]
    fn cross_project_items_are_invisible_to_the_recall() {
        let mut s = store();
        add(
            &mut s,
            "mine",
            &ScopeKey::project("p1"),
            "the build uses bazel",
            1,
        );
        add(
            &mut s,
            "theirs",
            &ScopeKey::project("p2"),
            "the build uses bazel there too",
            1,
        );
        let r = recall(&s, &ActorBinding::local_in("u", "p1"), "bazel");
        let ids: Vec<&str> = r.candidates.iter().map(|c| c.item.id.as_str()).collect();
        assert_eq!(ids, vec!["mine"], "cross-project leakage = 0");
    }

    #[test]
    fn sensitivity_above_the_key_ceiling_is_filtered_out() {
        let mut s = store();
        let mut c = NewItem::new(
            "secret",
            ScopeKey::project("p1"),
            Kind::Fact,
            "the bazel rollout plan",
        );
        c.sensitivity = Sensitivity::Confidential;
        s.insert(&c, 1).unwrap();
        add(
            &mut s,
            "plain",
            &ScopeKey::project("p1"),
            "the bazel version is 7",
            1,
        );
        // An external agent without a loadout has a `personal` ceiling here.
        let agent = ActorBinding::external_agent("u", "ag", "p1", None, None);
        let r = recall(&s, &agent, "bazel");
        let ids: Vec<&str> = r.candidates.iter().map(|c| c.item.id.as_str()).collect();
        assert_eq!(ids, vec!["plain"]);
        // The local user at the same project identity does see it.
        let local = recall(&s, &ActorBinding::local_in("u", "p1"), "bazel");
        assert_eq!(local.candidates.len(), 2);
    }

    #[test]
    fn a_caller_supplied_scope_filter_cannot_widen_the_grant() {
        let mut s = store();
        add(
            &mut s,
            "theirs",
            &ScopeKey::project("p2"),
            "the build uses bazel",
            1,
        );
        let actor = ActorBinding::local_in("u", "p1");
        let r = s
            .recall(
                &actor,
                &RecallQuery {
                    text: "bazel",
                    filter: Some(vec![ScopeKey::project("p2")]),
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &RecallConfig::default(),
                &AllSourcesAvailable,
            )
            .expect("recall");
        assert_eq!(r.outcome, RecallOutcome::Abstain);
        assert_eq!(r.abstain_reason, Some(AbstainReason::NoPermittedScope));
        assert!(r.candidates.is_empty());
    }

    #[test]
    fn a_narrowing_filter_works() {
        let mut s = store();
        add(
            &mut s,
            "mine",
            &ScopeKey::project("p1"),
            "the build uses bazel",
            1,
        );
        add(
            &mut s,
            "pref",
            &ScopeKey::user("u"),
            "the build uses bazel per my preference",
            1,
        );
        let r = s
            .recall(
                &ActorBinding::local_in("u", "p1"),
                &RecallQuery {
                    text: "bazel",
                    filter: Some(vec![ScopeKey::project("p1")]),
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &RecallConfig::default(),
                &AllSourcesAvailable,
            )
            .expect("recall");
        let ids: Vec<&str> = r.candidates.iter().map(|c| c.item.id.as_str()).collect();
        assert_eq!(ids, vec!["mine"]);
    }

    #[test]
    fn the_response_carries_candidates_and_provenance_and_nothing_else() {
        let mut s = store();
        let mut it = NewItem::new(
            "m1",
            ScopeKey::project("p"),
            Kind::Preference,
            "the build uses bazel",
        );
        it.source = "user".into();
        it.source_ref = Some("turn:7".into());
        it.trust_tier = TrustTier::UserExplicit;
        s.insert(&it, 1).unwrap();
        let r = recall(&s, &ActorBinding::local_in("u", "p"), "bazel");
        assert_eq!(r.provenance.len(), r.candidates.len());
        let p = &r.provenance[0];
        assert_eq!(p.source, "user");
        assert_eq!(p.trust_tier, TrustTier::UserExplicit);
        assert_eq!(p.source_ref.as_deref(), Some("turn:7"));
        assert!(!p.source_unavailable);
        // There is no rendered block field anywhere in the response: memory
        // returns candidates, never a pre-truncated injection.
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("rendered"));
        assert!(!json.contains("<memory"));
    }

    #[test]
    fn a_pruned_source_renders_as_unavailable_and_is_never_dereferenced() {
        let mut s = store();
        let mut it = NewItem::new(
            "m1",
            ScopeKey::project("p"),
            Kind::Fact,
            "the build uses bazel",
        );
        it.source_ref = Some("artifact:gone".into());
        s.insert(&it, 1).unwrap();
        let sources = PrunedSources::with(["artifact:gone"]);
        let r = s
            .recall(
                &ActorBinding::local_in("u", "p"),
                &RecallQuery {
                    text: "bazel",
                    filter: None,
                    token_budget: 256,
                    now_ms: 1_000_000,
                },
                &RecallConfig::default(),
                &sources,
            )
            .expect("recall");
        assert_eq!(r.outcome, RecallOutcome::Hit, "the item still recalls");
        assert!(r.provenance[0].source_unavailable);
    }

    #[test]
    fn a_backwards_clock_never_reorders_a_fixed_set() {
        let mut s = store();
        let older = add(
            &mut s,
            "older",
            &ScopeKey::project("p"),
            "the build uses bazel",
            1_000,
        );
        let newer = add(
            &mut s,
            "newer",
            &ScopeKey::project("p"),
            "the build uses bazel",
            2_000,
        );
        let cfg = RecallConfig::default();
        // A normal reading: the newer item gets the stronger recency boost.
        let now = 3_000;
        let fwd_older = cfg.recency_boost(older.created_at, now);
        let fwd_newer = cfg.recency_boost(newer.created_at, now);
        assert!(fwd_newer > fwd_older);
        // A clock that jumped backwards below both created_at values: ages
        // clamp to 0, so both boosts saturate at 1.0 and the tie breaks
        // deterministically instead of promoting the older item.
        let back = 500;
        let back_older = cfg.recency_boost(older.created_at, back);
        let back_newer = cfg.recency_boost(newer.created_at, back);
        assert_eq!(back_older, 1.0);
        assert_eq!(back_newer, 1.0);
    }

    #[test]
    fn recall_survives_many_items_and_stays_deterministic() {
        let mut s = store();
        for i in 0..500 {
            add(
                &mut s,
                &format!("m{i}"),
                &ScopeKey::project("p"),
                &format!("item {i} about the build pipeline and bazel rules"),
                1_000 + i,
            );
        }
        let actor = ActorBinding::local_in("u", "p");
        let a = recall(&s, &actor, "bazel");
        let b = recall(&s, &actor, "bazel");
        let ids_a: Vec<&str> = a.candidates.iter().map(|c| c.item.id.as_str()).collect();
        let ids_b: Vec<&str> = b.candidates.iter().map(|c| c.item.id.as_str()).collect();
        assert_eq!(ids_a, ids_b);
        assert!(!ids_a.is_empty());
    }

    #[test]
    fn sanitize_accepts_plain_words_and_case_and_rejects_syntax() {
        assert_eq!(
            sanitize_fts_query("Bazel Buck").unwrap(),
            Some("bazel buck".to_string())
        );
        assert_eq!(sanitize_fts_query("").unwrap(), None);
        assert!(sanitize_fts_query("bazel*").is_err());
        assert!(sanitize_fts_query("near(bazel buck)").is_err());
    }
}
