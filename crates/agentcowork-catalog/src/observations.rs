//! A11 — durable provider **observations** (runtime truth, never identity).
//!
//! The gap this closes: `ProviderRegistry::apply_probe` existed and
//! `capabilities_verified_at` existed, but **nothing on the live path ever
//! called them**. `catalog_cmds::probe_provider` ran the real `MetadataOnly`
//! probe, handed the result to the caller, and dropped it — so the catalog's own
//! rule ("advertised ≠ verified") was library-only, routing saw
//! `verified_report: None` for every provider forever, and the UI's `verifiedAt`
//! field could only ever populate from a hand-written profile stamp.
//!
//! This module is the durable half. One file
//! (`<data_dir>/provider-observations.json`) holds the **last live probe per
//! provider**, so the truth survives a restart even though `base_registry()` is
//! rebuilt from scratch on nearly every read.
//!
//! It is deliberately an *observation* store and carries no identity: ids,
//! aliases, transports and auth shape stay owned by [`crate::provider`], and
//! replaying an observation onto a freshly built registry never rewrites them.
//!
//! ## What the stamp means (and does not mean)
//!
//! `capabilities_verified_at` records **when a live probe last observed this
//! provider**. It is *not* a claim that every advertised capability was
//! confirmed — that claim lives in `VerificationReport::hard_caps_verified`,
//! which fails closed (see [`crate::probe::verify_report`]), and routing reads
//! only `trusted_capabilities`, which returns `Verified` verdicts alone.
//!
//! Two rules are enforced here rather than left to convention:
//!
//! * **Only a successful probe verifies.** A failed or unreachable probe is
//!   still recorded — it is real error history and the UI should be able to show
//!   it — but it never stamps verification.
//! * **A reachability probe cannot confirm a capability.** The `/v1/models`
//!   probe observes that the endpoint answered and which models it served, and
//!   nothing else. Every round-trip fact stays `None`, so every advertised hard
//!   capability is `Unverified`, `trusted_capabilities` stays empty, and routing
//!   keeps refusing to rely on a capability nobody confirmed. Replaying one
//!   therefore makes a provider *observed*, never *trusted*.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::probe::ProbeResult;
use crate::provider::{ProviderRegistry, normalize};

/// Which probe produced an observation.
///
/// Recorded so a future round-trip probe (one that can actually confirm
/// `tools` / `structured_output` / `codex`) is distinguishable from the
/// reachability probe that ships today. See [`crate::probe::AdvertisedHardCaps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProbePolicy {
    /// `GET {base}/models` — reachability and the served model list only.
    /// Cannot confirm any hard capability.
    #[default]
    MetadataOnly,
    /// A probe that exercised real round-trips, so it *can* confirm hard
    /// capabilities. Not yet produced by the live path.
    RoundTrip,
}

/// One provider's last live observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderObservation {
    /// When the probe ran (epoch milliseconds, as a string so the JSON shape is
    /// stable across platforms and eras).
    pub observed_at: String,
    /// Did the endpoint answer?
    pub ok: bool,
    /// HTTP status (0 = transport failure — nothing was answered).
    pub status: u16,
    /// The probe's own diagnostic.
    pub message: String,
    /// Models the endpoint advertised (0 = unknown, never a fabricated count).
    pub model_count: usize,
    /// The URL that was actually probed.
    pub url: String,
    /// Which probe produced this observation.
    pub policy: ProbePolicy,
    /// The facts the probe could honestly observe. A `MetadataOnly` probe leaves
    /// every round-trip flag `None`, which is precisely why hard capabilities
    /// stay unverified after one.
    pub probe: ProbeResult,
}

impl Default for ProviderObservation {
    fn default() -> Self {
        Self {
            observed_at: String::new(),
            ok: false,
            status: 0,
            message: String::new(),
            model_count: 0,
            url: String::new(),
            policy: ProbePolicy::default(),
            probe: ProbeResult::unobserved(),
        }
    }
}

impl ProviderObservation {
    /// Build the observation from the probe that actually ran.
    ///
    /// Field-for-field from [`crate::fetch::EndpointProbe`], so a caller cannot
    /// transcribe a status or a count that the network never produced. The
    /// `/v1/models` probe **counts** models without enumerating their ids, so
    /// `observed_model_ids` is deliberately left empty rather than filled with
    /// ids the probe never saw.
    pub fn from_metadata_probe(
        observed_at: impl Into<String>,
        probe: &crate::fetch::EndpointProbe,
    ) -> Self {
        Self::observed(
            observed_at,
            probe.ok,
            probe.status,
            probe.message.clone(),
            probe.url.clone(),
            probe.models,
        )
    }

    /// Build one from explicit facts, for callers with their own probe adapter
    /// (an ACP/MCP handshake rather than the HTTP `/v1/models` listing).
    ///
    /// Records reachability and a model **count**; every round-trip fact stays
    /// unobserved, so this can never be mistaken for capability verification.
    pub fn observed(
        observed_at: impl Into<String>,
        ok: bool,
        status: u16,
        message: impl Into<String>,
        url: impl Into<String>,
        model_count: usize,
    ) -> Self {
        Self {
            observed_at: observed_at.into(),
            ok,
            status,
            message: message.into(),
            model_count,
            url: url.into(),
            policy: ProbePolicy::MetadataOnly,
            probe: ProbeResult::unobserved(),
        }
    }

    /// Can this observation make any hard capability trustworthy? Only a
    /// probe that exercised round-trips can; reachability alone cannot.
    pub fn can_confirm_hard_caps(&self) -> bool {
        matches!(self.policy, ProbePolicy::RoundTrip)
    }

    /// Did the endpoint answer? `false` covers both "answered with an error" and
    /// "never connected" — `status` distinguishes the two (0 = transport).
    pub fn reachable(&self) -> bool {
        self.ok
    }
}

/// The whole observation file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderObservationsFile {
    pub providers: BTreeMap<String, ProviderObservation>,
}

impl ProviderObservationsFile {
    /// The observation for a provider, by **key** — case/separator-normalized
    /// (`Aws_Bedrock` reads `aws-bedrock`) but *not* alias-resolved: this file
    /// does not own the alias table, so `claude` does not find `anthropic` here.
    /// Resolve through [`ProviderRegistry::canonical_id`] first (or store with
    /// [`ObservationStore::record_resolved`], which does it for you).
    pub fn get(&self, provider: &str) -> Option<&ProviderObservation> {
        let key = normalize(provider);
        self.providers
            .get(&key)
            .or_else(|| self.providers.get(provider))
    }

    /// How many providers carry a **successful** observation.
    pub fn verified_count(&self) -> usize {
        self.providers.values().filter(|o| o.ok).count()
    }

    /// How many recorded probes never reached the endpoint (error history).
    pub fn failed_count(&self) -> usize {
        self.providers.values().filter(|o| !o.ok).count()
    }
}

/// Durable observation store (`<data_dir>/provider-observations.json`).
///
/// Atomic writes (tmp + rename) and a tolerant read: a malformed file degrades
/// to "no observations", which is the honest state — it never becomes a
/// partially trusted set of verification stamps.
#[derive(Debug, Clone)]
pub struct ObservationStore {
    path: PathBuf,
}

impl ObservationStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The conventional location inside a data dir.
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        Self::new(dir.as_ref().join("provider-observations.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> ProviderObservationsFile {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, provider: &str) -> Option<ProviderObservation> {
        self.load().get(provider).cloned()
    }

    /// Record one observation under the given key (case/separator-normalized).
    ///
    /// Dumb on purpose: this store does not own the alias table, so it cannot
    /// know that `claude` means `anthropic`. Prefer [`Self::record_resolved`]
    /// when a registry is at hand — otherwise two spellings of one provider
    /// become two rows.
    pub fn record(&self, provider: &str, obs: ProviderObservation) -> Result<(), String> {
        let key = normalize(provider);
        let mut file = self.load();
        file.providers.insert(key, obs);
        self.save(&file)
    }

    /// Record one observation keyed by the provider's **canonical** id as the
    /// registry resolves it, so `claude` and `anthropic` cannot hold two
    /// different truths. Falls back to the normalized name for a provider the
    /// registry does not know (an observation is still worth recording).
    pub fn record_resolved(
        &self,
        registry: &ProviderRegistry,
        provider: &str,
        obs: ProviderObservation,
    ) -> Result<(), String> {
        let key = registry
            .canonical_id(provider)
            .map(str::to_string)
            .unwrap_or_else(|| normalize(provider));
        self.record(&key, obs)
    }

    pub fn save(&self, file: &ProviderObservationsFile) -> Result<(), String> {
        let body = serde_json::to_string_pretty(file)
            .map_err(|e| format!("provider observations: serialize: {e}"))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("provider observations: create {}: {e}", dir.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, body)
            .map_err(|e| format!("provider observations: write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("provider observations: rename {}: {e}", tmp.display()))
    }
}

/// Replay stored observations onto a freshly built registry.
///
/// Returns how many providers were stamped. Only `ok` observations are applied;
/// a failed probe is deliberately skipped so `capabilities_verified_at` cannot
/// come to mean "we tried" instead of "a live endpoint answered".
///
/// Unknown providers are left unused rather than invented — `apply_probe`
/// returns `None` for them, so an observation whose provider was since removed
/// from the catalog cannot resurrect a record.
pub fn apply_observations(
    registry: &mut ProviderRegistry,
    file: &ProviderObservationsFile,
) -> usize {
    let mut applied = 0;
    for (id, obs) in &file.providers {
        if !obs.ok {
            continue;
        }
        if registry
            .apply_probe(id, &obs.probe, &obs.observed_at)
            .is_some()
        {
            applied += 1;
        }
    }
    applied
}

/// Map an observation onto the routing feed's health vocabulary.
///
/// Honest about the difference between *never connected* and *answered with a
/// failure*: a 401/429/5xx means the endpoint is reachable, so calling it `Down`
/// would be false — `Down` is reserved for a transport failure (`status == 0`),
/// the only case where nothing answered at all.
pub fn health_of(obs: &ProviderObservation) -> crate::routing_feed::Health {
    use crate::routing_feed::Health;
    match (obs.ok, obs.status) {
        (true, _) => Health::Healthy,
        (false, 0) => Health::Down,
        (false, _) => Health::Degraded,
    }
}

/// Replay observation health into a routing feed.
///
/// Returns how many providers got a health value. Providers with no observation
/// are left untouched, so the feed keeps its default (`Unknown`) instead of
/// being handed a health nobody observed.
pub fn apply_observation_health(
    feed: &mut crate::routing_feed::RoutingFeed,
    file: &ProviderObservationsFile,
) -> usize {
    let mut applied = 0;
    for (id, obs) in &file.providers {
        feed.set_health(id, health_of(obs));
        applied += 1;
    }
    applied
}

/// The reachability truth for a provider, straight from the durable file.
///
/// Separate from the registry stamp on purpose: "the endpoint answered" and
/// "its capabilities are trusted" are different facts, and callers that only
/// want the former (a settings row, a health hint) should not have to reason
/// about a verification report to get it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reachability {
    pub observed_at: String,
    pub ok: bool,
    pub status: u16,
    pub model_count: usize,
}

impl From<&ProviderObservation> for Reachability {
    fn from(o: &ProviderObservation) -> Self {
        Self {
            observed_at: o.observed_at.clone(),
            ok: o.ok,
            status: o.status,
            model_count: o.model_count,
        }
    }
}

impl ProviderObservationsFile {
    /// The last recorded reachability for a provider (by key — see
    /// [`Self::get`] on alias resolution).
    pub fn reachability(&self, provider: &str) -> Option<Reachability> {
        self.get(provider).map(Reachability::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Capability, trusted_capabilities};
    use crate::provider::{DiscoverySource, ProviderRecord, base_registry};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-catalog-observations-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn ok_obs(stamp: &str) -> ProviderObservation {
        ProviderObservation::observed(
            stamp,
            true,
            200,
            "ok",
            "https://api.example.com/v1/models",
            2,
        )
    }

    #[test]
    fn round_trips_through_the_store_and_folds_the_key_shape() {
        let d = dir("roundtrip");
        let store = ObservationStore::in_dir(&d);
        assert!(store.load().providers.is_empty());

        // Case/separator folding is the store's own normalization…
        store.record("Aws_Bedrock", ok_obs("1000")).unwrap();
        let back = store.get("aws-bedrock").expect("key-normalized read");
        assert_eq!(back.observed_at, "1000");
        assert_eq!(back.model_count, 2);
        assert!(back.ok);
        assert_eq!(store.load().providers.len(), 1);

        // …re-recording replaces rather than duplicating…
        store.record("aws-bedrock", ok_obs("2000")).unwrap();
        let file = store.load();
        assert_eq!(file.providers.len(), 1);
        assert_eq!(file.get("aws-bedrock").unwrap().observed_at, "2000");
        // …and a miss is honest, not a fabricated row.
        assert!(file.get("nope").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The store does not own the alias table, so `record_resolved` borrows the
    /// registry's: `claude` and `anthropic` must not become two truths.
    #[test]
    fn record_resolved_keys_by_the_registrys_canonical_id() {
        let d = dir("resolved");
        let store = ObservationStore::in_dir(&d);
        let registry = base_registry();

        store
            .record_resolved(&registry, "claude", ok_obs("1000"))
            .unwrap();
        store
            .record_resolved(&registry, "anthropic", ok_obs("2000"))
            .unwrap();
        let file = store.load();
        assert_eq!(file.providers.len(), 1, "one provider, one truth");
        assert!(file.providers.contains_key("anthropic"));
        assert_eq!(file.get("anthropic").unwrap().observed_at, "2000");

        // And the registry-touching path is alias-aware in the other direction:
        // a legacy file keyed by the alias still applies to the canonical record.
        let mut legacy = base_registry();
        let mut old = ProviderObservationsFile::default();
        old.providers.insert("claude".into(), ok_obs("3000"));
        assert_eq!(apply_observations(&mut legacy, &old), 1);
        assert_eq!(legacy.verified_at("anthropic"), Some("3000"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_malformed_file_is_no_observations_not_a_partial_trust_set() {
        let d = dir("malformed");
        std::fs::create_dir_all(&d).unwrap();
        let store = ObservationStore::in_dir(&d);
        std::fs::write(store.path(), b"{oops").unwrap();
        let file = store.load();
        assert!(file.providers.is_empty());
        assert_eq!(file.verified_count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A failed probe is recorded (real error history) but never verifies.
    #[test]
    fn a_failed_probe_is_recorded_but_never_stamps_verification() {
        let mut registry = base_registry();
        let mut file = ProviderObservationsFile::default();
        file.providers.insert(
            "openai".into(),
            ProviderObservation {
                observed_at: "3000".into(),
                ok: false,
                status: 401,
                message: "unauthorized".into(),
                ..ProviderObservation::default()
            },
        );
        assert_eq!(apply_observations(&mut registry, &file), 0);
        assert!(registry.verified_at("openai").is_none());
        // …but the failure is still in the durable file, with its status.
        assert_eq!(file.failed_count(), 1);
        assert_eq!(file.reachability("openai").unwrap().status, 401);
    }

    /// A successful reachability probe stamps *when the endpoint answered* and
    /// records the served models, but makes no advertised capability
    /// trustworthy — the rule that keeps `verifiedAt` from becoming a
    /// capability claim.
    #[test]
    fn a_reachability_probe_stamps_observation_without_confirming_hard_caps() {
        let mut registry = base_registry();
        // Advertise `tools` so there is something a probe *could* confirm.
        registry.register(ProviderRecord {
            id: "openai".into(),
            capabilities: vec!["tools".into()],
            source: DiscoverySource::UserConfig,
            ..Default::default()
        });

        let mut file = ProviderObservationsFile::default();
        file.providers.insert("openai".into(), ok_obs("4000"));
        assert_eq!(apply_observations(&mut registry, &file), 1);

        // Observed…
        assert_eq!(registry.verified_at("openai"), Some("4000"));
        let rec = registry.resolve("openai").expect("record");
        let report = rec.verified_report.as_ref().expect("report stored");
        // …and no model ids are invented for a probe that only counted them…
        assert!(report.observed_model_ids.is_empty());
        // …but nothing advertised was confirmed, so nothing may be trusted.
        assert!(!report.is_fully_verified());
        assert!(!trusted_capabilities(report).contains(&Capability::Tools));
        assert!(!file.get("openai").unwrap().can_confirm_hard_caps());
    }

    /// A registry that advertises no hard capability must not report itself
    /// "fully verified" — that vacuous truth is how reachability used to
    /// become a capability claim in `ResourceCard::from_provider`.
    #[test]
    fn an_unobserved_report_is_never_fully_verified() {
        let mut registry = base_registry();
        let mut file = ProviderObservationsFile::default();
        file.providers.insert("groq".into(), ok_obs("5000"));
        assert_eq!(apply_observations(&mut registry, &file), 1);

        let rec = registry.resolve("groq").expect("record");
        let report = rec.verified_report.as_ref().expect("report stored");
        assert!(
            !report.is_fully_verified(),
            "a reachability probe confirmed no capability and must not claim to"
        );
        assert!(trusted_capabilities(report).is_empty());
    }

    #[test]
    fn an_unknown_provider_observation_is_left_unused_not_invented() {
        let mut registry = base_registry();
        let mut file = ProviderObservationsFile::default();
        file.providers
            .insert("definitely-not-a-provider".into(), ok_obs("6000"));
        assert_eq!(apply_observations(&mut registry, &file), 0);
        assert!(!registry.all().any(|r| r.id == "definitely-not-a-provider"));
    }

    /// Health distinguishes "never connected" from "answered with a failure".
    #[test]
    fn health_separates_unreachable_from_answered_with_failure() {
        use crate::routing_feed::Health;
        assert_eq!(health_of(&ok_obs("1")), Health::Healthy);

        // A transport failure — nothing answered, so Down is literally true.
        let unreachable =
            ProviderObservation::observed("1", false, 0, "connection refused", "u", 0);
        assert_eq!(health_of(&unreachable), Health::Down);

        // An answered rejection — reachable, so it must not be called Down.
        let rejected = ProviderObservation::observed("1", false, 401, "unauthorized", "u", 0);
        assert_eq!(health_of(&rejected), Health::Degraded);
        let throttled = ProviderObservation::observed("1", false, 429, "slow down", "u", 0);
        assert_eq!(health_of(&throttled), Health::Degraded);
    }

    #[test]
    fn observation_health_reaches_the_feed_without_inventing_health() {
        use crate::routing_feed::{Health, RoutingFeed};
        let registry = base_registry();
        let mut feed = RoutingFeed::new();
        feed.load_registry(&registry);
        feed.set_health("openai", Health::Unknown);

        let mut file = ProviderObservationsFile::default();
        file.providers.insert("openai".into(), ok_obs("1"));
        file.providers.insert(
            "groq".into(),
            ProviderObservation::observed("1", false, 0, "refused", "u", 0),
        );
        assert_eq!(apply_observation_health(&mut feed, &file), 2);
        // Observed providers get their observed health; an unobserved provider
        // keeps the caller's default rather than a guessed value.
        assert_eq!(feed.health_of("openai"), Health::Healthy);
        assert_eq!(feed.health_of("groq"), Health::Down);
        assert_eq!(feed.health_of("cerebras"), Health::Unknown);
    }

    #[test]
    fn verified_count_counts_only_successes() {
        let mut file = ProviderObservationsFile::default();
        file.providers.insert("openai".into(), ok_obs("1"));
        file.providers.insert(
            "anthropic".into(),
            ProviderObservation {
                ok: false,
                ..ProviderObservation::default()
            },
        );
        assert_eq!(file.verified_count(), 1);
        assert_eq!(file.failed_count(), 1);
    }

    /// The live constructor is field-for-field off the probe, so a status or a
    /// count cannot be transcribed from anywhere else.
    #[test]
    fn a_metadata_observation_mirrors_the_probe_it_came_from() {
        let probe = crate::fetch::EndpointProbe {
            ok: true,
            status: 200,
            message: "9 models".into(),
            models: 9,
            url: "https://api.groq.com/openai/v1/models".into(),
        };
        let obs = ProviderObservation::from_metadata_probe("7000", &probe);
        assert!(obs.ok);
        assert_eq!(obs.status, 200);
        assert_eq!(obs.model_count, 9);
        assert_eq!(obs.url, "https://api.groq.com/openai/v1/models");
        assert_eq!(obs.policy, ProbePolicy::MetadataOnly);
        // No round-trip fact is claimed, and no id is invented.
        assert_eq!(obs.probe.tool_call_ok, None);
        assert!(obs.probe.observed_model_ids.is_empty());
        assert!(!obs.can_confirm_hard_caps());
    }

    #[test]
    fn a_failed_probe_mirrors_its_status_without_claiming_reachability() {
        let probe = crate::fetch::EndpointProbe {
            ok: false,
            status: 401,
            message: "unauthorized".into(),
            models: 0,
            url: "https://api.example.com/v1/models".into(),
        };
        let obs = ProviderObservation::from_metadata_probe("8000", &probe);
        assert!(!obs.ok);
        assert_eq!(obs.status, 401);
        assert!(!obs.reachable());
    }
}
