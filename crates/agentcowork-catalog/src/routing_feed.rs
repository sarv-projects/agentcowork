//! P44.8 — Routing feed: live provider health + **verified** capabilities feed
//! the route decision (A7), so intent-first routing uses observed truth, not
//! advertised metadata. A registry/discovery change bumps a generation stamp
//! that invalidates the feed's cache and forces a re-rank.
//!
//! The distinction from `routing::RouteFilters` (model-level hard filters):
//! this operates at the **provider** level — given a set of candidate
//! providers, their verified-capability reports (P44.4) and their live health
//! observations, it produces a ranked `RouteDecision` naming which providers
//! may serve a request and why the others were excluded.
//!
//! Fail-closed: a provider that advertised a required hard capability the
//! probe never confirmed is **excluded** for that requirement (it is not
//! merely down-ranked). A dead/unhealthy provider scores 0 and is excluded
//! when any healthy candidate remains.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::probe::{Capability, trusted_capabilities};
use crate::provider::{Auth, ProviderRecord, ProviderRegistry, normalize};

/// Live health for one provider (fed by A7 observations / probe pings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// Reachable + recent success.
    Healthy,
    /// Reachable but degraded (recent 429/latency spikes).
    Degraded,
    /// Recent hard failure / unreachable.
    Down,
    /// No observation yet — unknown (treated as usable but unranked-boosted).
    Unknown,
}

// Deliberate: the default is `Unknown`, not the first variant (`Healthy`),
// so `#[derive(Default)]` would change semantics if the enum is reordered.
#[allow(clippy::derivable_impls)]
impl Default for Health {
    fn default() -> Self {
        Health::Unknown
    }
}

impl Health {
    /// The health contribution to the consensus score (0..1). `Down` = 0 so a
    /// dead provider is excluded when any healthy candidate remains.
    fn score(&self) -> f64 {
        match self {
            Health::Healthy => 1.0,
            Health::Degraded => 0.4,
            Health::Unknown => 0.6,
            Health::Down => 0.0,
        }
    }
}

/// What a route request requires (the provider-level hard requirements). This
/// is the coarse gate before the model-level `RouteFilters`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRequirements {
    /// The request needs verified tool-calling.
    pub requires_tools: bool,
    /// The request needs verified structured output.
    pub requires_structured_output: bool,
    /// The request needs a codex/responses transport.
    pub requires_codex: bool,
}

impl RouteRequirements {
    fn required_caps(&self) -> Vec<Capability> {
        let mut v = Vec::new();
        if self.requires_tools {
            v.push(Capability::Tools);
        }
        if self.requires_structured_output {
            v.push(Capability::StructuredOutput);
        }
        if self.requires_codex {
            v.push(Capability::CodexResponses);
        }
        v
    }
}

/// One ranked candidate in the decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedProvider {
    pub id: String,
    pub score: f64,
    /// The verified capabilities routing may rely on.
    pub verified_capabilities: Vec<String>,
    pub health: Health,
}

/// One excluded candidate + the honest reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedProvider {
    pub id: String,
    pub reason: String,
}

/// The route decision the router consumes: the ranked usable providers +
/// the excluded ones with reasons (the honesty surface).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDecision {
    pub ranked: Vec<RankedProvider>,
    pub excluded: Vec<ExcludedProvider>,
    /// The feed generation this decision was computed against (cache key).
    pub generation: u64,
}

impl RouteDecision {
    /// The top provider id, if any candidate survived.
    pub fn top(&self) -> Option<&str> {
        self.ranked.first().map(|r| r.id.as_str())
    }
}

/// The routing feed: holds the provider registry snapshot + live health, and
/// re-ranks on demand. `generation` invalidates any cached decision: bump it
/// whenever the registry or health map changes.
#[derive(Debug, Default)]
pub struct RoutingFeed {
    providers: Vec<ProviderRecord>,
    health: HashMap<String, Health>,
    /// Provider ids (or aliases) the vault currently holds a credential for
    /// (P50.3.6). Providers whose `Auth` requires a vault key are excluded
    /// from the decision when absent — the feed never ranks a provider the
    /// user has not keyed as if it were usable.
    credentialed: HashSet<String>,
    generation: u64,
    /// Cached last decision keyed by (requirements-hash, generation).
    cache: HashMap<(u64, u64), RouteDecision>,
}

impl RoutingFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load providers from a registry, bumping the generation (invalidates the
    /// cache — a registry change forces a re-rank, per P44.8).
    pub fn load_registry(&mut self, reg: &ProviderRegistry) {
        self.providers = reg.all().cloned().collect();
        self.bump();
    }

    /// Record/refresh a provider's live health (bumps the generation).
    pub fn set_health(&mut self, provider_id: &str, health: Health) {
        self.health.insert(provider_id.to_string(), health);
        self.bump();
    }

    /// The health currently recorded for a provider.
    ///
    /// Defaults to `Unknown` when nothing has been observed — never a guessed
    /// value. Keyed by provider id, so callers replaying observations should
    /// key them by canonical id (`claude` and `anthropic` are the same provider
    /// and must not hold two health values).
    pub fn health_of(&self, provider_id: &str) -> Health {
        self.health.get(provider_id).copied().unwrap_or_default()
    }

    /// Replace the vault-credential set (P50.3.6). Ids (or their aliases)
    /// present here may rank; a keyed provider absent here is excluded with
    /// an honest "add a key" reason. Bumps the generation like any health
    /// change, so cached decisions never outlive a key change.
    pub fn set_credentialed(&mut self, ids: impl IntoIterator<Item = String>) {
        self.credentialed = ids.into_iter().map(|s| normalize(&s)).collect();
        self.bump();
    }

    /// True when `id` (or one of its aliases) is in the credential set.
    /// P50.3.6 — both sides are canonicalized via `normalize()` so vault
    /// casings (`OpenAI`, `nvidia_nim`) match registry ids/aliases
    /// (`openai`, `nvidia-nim`); without this an unkeyed provider could rank
    /// (or a keyed one could be wrongly excluded) on spelling alone.
    fn is_credentialed(&self, p: &ProviderRecord) -> bool {
        if self
            .credentialed
            .iter()
            .any(|c| normalize(c) == normalize(&p.id))
        {
            return true;
        }
        p.aliases.iter().any(|a| {
            self.credentialed
                .iter()
                .any(|c| normalize(c) == normalize(a))
        })
    }

    /// Whether the provider needs a vault-held API key at all (P50.3.6).
    /// Keyless/local runtimes and OS-credential sources are never gated by
    /// the vault key ring.
    fn requires_vault_key(auth: Auth) -> bool {
        matches!(auth, Auth::ApiKey | Auth::ApiKeyEnv)
    }

    /// The current feed generation (cache key + decision provenance).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cache.clear(); // any change invalidates every cached decision
    }

    /// Rank the providers for a request. Uses the cache when the generation is
    /// unchanged; recomputes otherwise. Fail-closed on unverified hard caps
    /// and on dead providers.
    pub fn decide(&mut self, req: &RouteRequirements) -> RouteDecision {
        let key = (req_hash(req), self.generation);
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        let decision = self.compute(req);
        self.cache.insert(key, decision.clone());
        decision
    }

    fn compute(&self, req: &RouteRequirements) -> RouteDecision {
        let required = req.required_caps();
        let mut ranked = Vec::new();
        let mut excluded = Vec::new();

        // First pass: split into usable vs excluded, scoring the usable.
        for p in &self.providers {
            let health = self.health.get(&p.id).copied().unwrap_or_default();
            let verified: Vec<Capability> = p
                .verified_report
                .as_ref()
                .map(trusted_capabilities)
                .unwrap_or_default();

            // P50.3.6 — credential gate: a provider that needs a vault-held
            // API key but has none must not rank. This is the consumer-truth
            // fix: the catalog alone no longer decides routability.
            if Self::requires_vault_key(p.auth) && !self.is_credentialed(p) {
                excluded.push(ExcludedProvider {
                    id: p.id.clone(),
                    reason: "no vault credential — add a provider key in Settings → Keys".into(),
                });
                continue;
            }

            // Fail-closed: every required hard cap must be verified.
            let missing: Vec<Capability> = required
                .iter()
                .copied()
                .filter(|c| !verified.contains(c))
                .collect();
            if !missing.is_empty() {
                excluded.push(ExcludedProvider {
                    id: p.id.clone(),
                    reason: format!(
                        "unverified required capability: {}",
                        missing
                            .iter()
                            .map(|c| format!("{c:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
                continue;
            }
            if health == Health::Down {
                excluded.push(ExcludedProvider {
                    id: p.id.clone(),
                    reason: "provider health is Down".into(),
                });
                continue;
            }
            // Consensus score: health-weighted, +bonus per verified cap
            // (rewards a probe-confirmed provider over a bare-metadata one).
            let cap_bonus = 0.05 * verified.len() as f64;
            let score = (health.score() + cap_bonus).clamp(0.0, 1.0);
            ranked.push(RankedProvider {
                id: p.id.clone(),
                score,
                verified_capabilities: verified.iter().map(|c| format!("{c:?}")).collect(),
                health,
            });
        }

        // If everything healthy got excluded only because health was Down but
        // NOTHING is usable, surface the down ones as a fallback (honest — the
        // router can decide to retry a Down provider when there is no other).
        if ranked.is_empty() && !excluded.is_empty() {
            // Re-admit Down-only exclusions (not capability failures) at score 0.
            for p in &self.providers {
                let health = self.health.get(&p.id).copied().unwrap_or_default();
                let verified: Vec<Capability> = p
                    .verified_report
                    .as_ref()
                    .map(trusted_capabilities)
                    .unwrap_or_default();
                let missing = required.iter().any(|c| !verified.contains(c));
                // The credential gate holds in the fallback too: a dead
                // provider still needs a key to be a retry candidate.
                if health == Health::Down
                    && !missing
                    && (self.is_credentialed(p) || !Self::requires_vault_key(p.auth))
                {
                    ranked.push(RankedProvider {
                        id: p.id.clone(),
                        score: 0.0,
                        verified_capabilities: verified.iter().map(|c| format!("{c:?}")).collect(),
                        health,
                    });
                }
            }
        }

        // Rank: highest score first; stable by id for determinism.
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });

        RouteDecision {
            ranked,
            excluded,
            generation: self.generation,
        }
    }
}

/// A tiny stable hash of the requirements (cache key component).
fn req_hash(req: &RouteRequirements) -> u64 {
    (req.requires_tools as u64)
        | ((req.requires_structured_output as u64) << 1)
        | ((req.requires_codex as u64) << 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{AdvertisedHardCaps, ProbeResult, verify_report};

    fn provider(id: &str, tools_verified: bool) -> ProviderRecord {
        let mut rec = ProviderRecord {
            id: id.to_string(),
            ..Default::default()
        };
        // Build a verified report: advertised tools, probe confirms iff verified.
        let advert = AdvertisedHardCaps {
            tools: true,
            structured_output: false,
            codex_responses: false,
        };
        let observed = ProbeResult {
            tool_call_ok: Some(tools_verified),
            ..Default::default()
        };
        rec.verified_report = Some(verify_report(&advert, &observed));
        rec
    }

    fn registry(recs: Vec<ProviderRecord>) -> ProviderRegistry {
        let mut reg = ProviderRegistry::default();
        for r in recs {
            reg.register(r);
        }
        reg
    }

    /// Pre-P50.3.6 tests exercise health/capability behavior, so credential
    /// every provider (default `Auth::ApiKeyEnv` is now gate-gated).
    fn credential_all(feed: &mut RoutingFeed, ids: &[&str]) {
        feed.set_credentialed(ids.iter().map(|s| s.to_string()));
    }

    #[test]
    fn healthy_verified_provider_ranks_top() {
        let reg = registry(vec![provider("a", true), provider("b", true)]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        credential_all(&mut feed, &["a", "b"]);
        feed.set_health("a", Health::Healthy);
        feed.set_health("b", Health::Degraded);
        let d = feed.decide(&RouteRequirements {
            requires_tools: true,
            ..Default::default()
        });
        assert_eq!(d.top(), Some("a")); // healthy beats degraded
        assert_eq!(d.ranked.len(), 2);
    }

    #[test]
    fn unverified_required_capability_excludes_fail_closed() {
        // 'b' advertised tools but the probe failed → excluded for a tools req.
        let reg = registry(vec![provider("a", true), provider("b", false)]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        credential_all(&mut feed, &["a", "b"]);
        feed.set_health("a", Health::Healthy);
        feed.set_health("b", Health::Healthy);
        let d = feed.decide(&RouteRequirements {
            requires_tools: true,
            ..Default::default()
        });
        assert_eq!(d.ranked.len(), 1);
        assert_eq!(d.top(), Some("a"));
        assert!(
            d.excluded
                .iter()
                .any(|e| e.id == "b" && e.reason.contains("unverified"))
        );
    }

    #[test]
    fn down_provider_excluded_when_healthy_exists() {
        let reg = registry(vec![provider("a", true), provider("b", true)]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        credential_all(&mut feed, &["a", "b"]);
        feed.set_health("a", Health::Down);
        feed.set_health("b", Health::Healthy);
        let d = feed.decide(&RouteRequirements {
            requires_tools: true,
            ..Default::default()
        });
        assert_eq!(d.top(), Some("b"));
        assert!(
            d.excluded
                .iter()
                .any(|e| e.id == "a" && e.reason.contains("Down"))
        );
    }

    #[test]
    fn down_provider_readmitted_as_fallback_when_nothing_else() {
        let reg = registry(vec![provider("a", true)]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        credential_all(&mut feed, &["a"]);
        feed.set_health("a", Health::Down);
        let d = feed.decide(&RouteRequirements {
            requires_tools: true,
            ..Default::default()
        });
        // Only candidate is Down but capability-verified → readmitted at score 0.
        assert_eq!(d.top(), Some("a"));
        assert_eq!(d.ranked[0].score, 0.0);
    }

    #[test]
    fn registry_change_invalidates_cache_and_reranks() {
        let reg1 = registry(vec![provider("a", true)]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg1);
        credential_all(&mut feed, &["a"]);
        feed.set_health("a", Health::Healthy);
        let g1 = feed.generation();
        let d1 = feed.decide(&RouteRequirements {
            requires_tools: true,
            ..Default::default()
        });
        assert_eq!(d1.generation, g1);
        assert_eq!(d1.ranked.len(), 1);

        // New registry with an added provider bumps the generation → re-rank.
        let reg2 = registry(vec![provider("a", true), provider("c", true)]);
        feed.load_registry(&reg2);
        credential_all(&mut feed, &["a", "c"]);
        feed.set_health("c", Health::Healthy);
        let g2 = feed.generation();
        assert_ne!(g1, g2);
        let d2 = feed.decide(&RouteRequirements {
            requires_tools: true,
            ..Default::default()
        });
        assert_eq!(d2.generation, g2);
        assert_eq!(d2.ranked.len(), 2); // 'c' now visible → cache did not stick
    }

    #[test]
    fn no_requirements_admits_all_non_down() {
        let reg = registry(vec![provider("a", false), provider("b", true)]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        credential_all(&mut feed, &["a", "b"]);
        feed.set_health("a", Health::Healthy);
        feed.set_health("b", Health::Unknown);
        // No hard requirement → even the tools-unverified provider is usable.
        let d = feed.decide(&RouteRequirements::default());
        assert_eq!(d.ranked.len(), 2);
    }

    // ---- P50.3.6 — vault-credential gating ------------------------------

    fn keyed(id: &str) -> ProviderRecord {
        // Default auth is ApiKeyEnv (needs a vault key).
        let mut rec = provider(id, true);
        rec.auth = Auth::ApiKeyEnv;
        rec
    }

    fn keyless(id: &str) -> ProviderRecord {
        let mut rec = provider(id, true);
        rec.auth = Auth::Keyless;
        rec
    }

    #[test]
    fn unkeyed_api_provider_is_excluded_with_add_key_reason() {
        let reg = registry(vec![keyed("a")]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        feed.set_health("a", Health::Healthy);
        // No credential set at all — 'a' requires a vault key.
        let d = feed.decide(&RouteRequirements::default());
        assert!(d.ranked.is_empty(), "unkeyed provider must never rank");
        let e = d
            .excluded
            .iter()
            .find(|e| e.id == "a")
            .expect("excluded entry");
        assert!(
            e.reason.contains("vault credential"),
            "reason: {}",
            e.reason
        );
        assert!(e.reason.contains("add a provider key"));
    }

    #[test]
    fn credentialed_api_provider_ranks() {
        let reg = registry(vec![keyed("a")]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        feed.set_credentialed(["a".to_string()]);
        feed.set_health("a", Health::Healthy);
        let d = feed.decide(&RouteRequirements::default());
        assert_eq!(d.top(), Some("a"));
        assert!(d.excluded.is_empty());
    }

    #[test]
    fn keyless_provider_is_never_gated_by_vault() {
        let reg = registry(vec![keyless("lmstudio")]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        // No credential set — keyless must still rank.
        feed.set_health("lmstudio", Health::Healthy);
        let d = feed.decide(&RouteRequirements::default());
        assert_eq!(d.top(), Some("lmstudio"));
        assert!(d.excluded.is_empty());
    }

    #[test]
    fn alias_in_credential_set_credentials_the_provider() {
        let mut rec = keyed("actual");
        rec.aliases = vec!["legacy-name".to_string()];
        let reg = registry(vec![rec]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        // The vault holds the key under the alias, not the canonical id.
        feed.set_credentialed(["legacy-name".to_string()]);
        feed.set_health("actual", Health::Healthy);
        let d = feed.decide(&RouteRequirements::default());
        assert_eq!(d.top(), Some("actual"));
    }

    #[test]
    fn unkeyed_provider_is_not_readmitted_by_down_fallback() {
        let reg = registry(vec![keyed("a")]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        feed.set_health("a", Health::Down);
        // Everything is excluded AND unkeyed — no fallback re-admission.
        let d = feed.decide(&RouteRequirements::default());
        assert!(d.ranked.is_empty());
        assert!(
            d.excluded
                .iter()
                .any(|e| e.id == "a" && e.reason.contains("vault credential"))
        );
    }

    #[test]
    fn key_change_bumps_generation_and_invalidates_cache() {
        let reg = registry(vec![keyed("a")]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        feed.set_health("a", Health::Healthy);
        feed.set_credentialed(Vec::<String>::new());
        let g1 = feed.generation();
        let d1 = feed.decide(&RouteRequirements::default());
        assert!(d1.ranked.is_empty());

        // User adds a key → credential set changes → stale cached decision dies.
        feed.set_credentialed(["a".to_string()]);
        let g2 = feed.generation();
        assert_ne!(g1, g2);
        let d2 = feed.decide(&RouteRequirements::default());
        assert_eq!(d2.top(), Some("a"));
    }

    #[test]
    fn credential_matching_is_case_and_separator_insensitive() {
        let reg = registry(vec![keyed("openai")]);
        let mut feed = RoutingFeed::new();
        feed.load_registry(&reg);
        // Vault casing/separators must not decide routability.
        feed.set_credentialed(["OpenAI".to_string()]);
        feed.set_health("openai", Health::Healthy);
        let d = feed.decide(&RouteRequirements::default());
        assert_eq!(d.top(), Some("openai"));
    }
}
