//! P44.4 — capability-probe verification ("catalog says ≠ runtime is").
//!
//! After discovery, probe the live endpoint and write back *verified* truth
//! so routing never trusts stale metadata. The A11 `capabilities_verified_at`
//! field is the presence marker; this module defines **what** gets verified
//! and how the observed facts compare against what the catalog advertised.
//!
//! Mirroring the catalog crate's pure + testable discipline: the network
//! probe itself is the caller's job (an injected transport seam), and this
//! module turns raw observed facts into a verdict + a verified-capability
//! record that `ProviderRegistry` stores. The same pattern is reused for
//! agents and MCP servers (advertised vs verified — J17 / F6) by constructing
//! a [`ProbeResult`] from their handshake outputs instead of a `/v1/models`
//! listing.
//!
//! **Fail-closed on hard capabilities.** A hard capability (tools, structured
//! output, codex/responses transport) that the catalog *advertised* but the
//! probe could not confirm stays `Unverified` — routing must not rely on it.
//! Soft facts (exact model list, observed context ceiling) are recorded as
//! observed without failing the provider.

use serde::{Deserialize, Serialize};

/// What the live endpoint actually reported. Built by the caller's probe
/// adapter (the injected transport seam — this module never performs I/O).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProbeResult {
    /// Model ids the endpoint actually served (`/v1/models`, handshake, .);
    /// the provider is *not* verified to serve models outside this set.
    pub observed_model_ids: Vec<String>,
    /// A tool-call round-trip actually succeeded.
    pub tool_call_ok: Option<bool>,
    /// A structured-output (JSON mode) round-trip actually succeeded.
    pub structured_output_ok: Option<bool>,
    /// Observed max context length (tokens) from the probe.
    pub observed_context_len: Option<u64>,
    /// A Responses/Codex-compatible round-trip succeeded (when the catalog
    /// advertised `CodexResponses` transport).
    pub codex_ok: Option<bool>,
}

impl ProbeResult {
    /// A truthful "no probe data" baseline (no fields set). Callers use this
    /// when a provider was discovered but the probe was skipped/errored so an
    /// honest unverified state exists instead of an implicit trust.
    pub fn unobserved() -> Self {
        Self::default()
    }
}

/// The hard capabilities the catalog may advertise and the probe must
/// confirm before routing may rely on them. Presence of a capability here
/// after verification means *observed*, not merely advertised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Tools,
    StructuredOutput,
    /// Codex/Responses transport produces structured tool results.
    CodexResponses,
}

/// One capability's verification outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Observed present and matching (or exceeding) the catalog advert.
    Verified,
    /// The catalog advertised it but the probe could not confirm it.
    /// Routing must **not** rely on it.
    Unverified,
    /// Neither advertised by the catalog nor observed — honest unknown.
    Unadvertised,
}

/// The verification comparison for one capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityVerdict {
    pub capability: Capability,
    pub verdict: Verdict,
    /// Human-readable evidence (e.g. which model round-tripped tools).
    #[serde(default)]
    pub evidence: String,
}

/// The full verification report for a provider. Stored on the
/// `ProviderRecord` (`verified_capabilities` are the trusted facts routing
/// reads) and used to stamp `capabilities_verified_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VerificationReport {
    #[serde(default)]
    pub capability_verdicts: Vec<CapabilityVerdict>,
    /// Model ids actually observed (subset of the catalog's advertised set
    /// when the catalog over-advertised).
    #[serde(default)]
    pub observed_model_ids: Vec<String>,
    /// Observed context ceiling (if any).
    #[serde(default)]
    pub observed_context_len: Option<u64>,
    /// Whether every hard capability the catalog *advertised* was confirmed
    /// **and at least one capability was actually confirmed**. `false` → the
    /// provider is `Unverified` for its advertised capabilities (or confirmed
    /// nothing at all) and routing must not rely on them.
    pub hard_caps_verified: bool,
}

impl VerificationReport {
    /// Did the probe fully confirm every hard capability the catalog claimed?
    pub fn is_fully_verified(&self) -> bool {
        self.hard_caps_verified
    }
}

/// Which hard capabilities the provider advertises to the probe (defaults to
/// the transport family + the catalog's model-level `supported_parameters`).
/// Callers pass the advertised set so the report knows what must be confirmed.
pub struct AdvertisedHardCaps {
    pub tools: bool,
    pub structured_output: bool,
    pub codex_responses: bool,
}

impl AdvertisedHardCaps {
    pub fn list(&self) -> Vec<Capability> {
        let mut out = Vec::new();
        if self.tools {
            out.push(Capability::Tools);
        }
        if self.structured_output {
            out.push(Capability::StructuredOutput);
        }
        if self.codex_responses {
            out.push(Capability::CodexResponses);
        }
        out
    }
}

/// Pure comparison: given what the catalog advertised (`advertised`) and what
/// the probe actually observed (`observed`), produce a verdict for every hard
/// capability. **Fails closed** — a hard capability the catalog claims but the
/// probe did not confirm is `Unverified`. Soft facts (model list, context
/// ceiling) are recorded without failing the provider.
pub fn verify_report(
    advertised: &AdvertisedHardCaps,
    observed: &ProbeResult,
) -> VerificationReport {
    // For each hard capability: confirmed → Verified; advertised-but-not-
    // confirmed → Unverified; neither advertised nor observed → Unadvertised.
    let verdicts = vec![
        CapabilityVerdict {
            capability: Capability::Tools,
            verdict: hard_verdict(advertised.tools, observed.tool_call_ok),
            evidence: evidence_for(Capability::Tools, advertised.tools, observed.tool_call_ok),
        },
        CapabilityVerdict {
            capability: Capability::StructuredOutput,
            verdict: hard_verdict(advertised.structured_output, observed.structured_output_ok),
            evidence: evidence_for(
                Capability::StructuredOutput,
                advertised.structured_output,
                observed.structured_output_ok,
            ),
        },
        CapabilityVerdict {
            capability: Capability::CodexResponses,
            verdict: hard_verdict(advertised.codex_responses, observed.codex_ok),
            evidence: evidence_for(
                Capability::CodexResponses,
                advertised.codex_responses,
                observed.codex_ok,
            ),
        },
    ];
    // Fully verified only when nothing advertised sits in the Unverified
    // bucket (Unadvertised and Verified are both fine) **and at least one
    // capability was actually confirmed**. Without the second half the
    // predicate is vacuously true for a provider that advertises no hard
    // capability, which silently turns "the endpoint answered" into a
    // `capabilities_verified: true` / `Healthy` card — the exact false claim
    // this module exists to prevent. Failing closed is the safe direction:
    // consumers read it as "may I claim verified capabilities?" → no.
    let confirmed = verdicts
        .iter()
        .filter(|v| v.verdict == Verdict::Verified)
        .count();
    let hard_caps_verified =
        confirmed > 0 && !verdicts.iter().any(|v| v.verdict == Verdict::Unverified);

    VerificationReport {
        capability_verdicts: verdicts,
        observed_model_ids: observed.observed_model_ids.clone(),
        observed_context_len: observed.observed_context_len,
        hard_caps_verified,
    }
}

/// The per-capability verdict: advertised must be confirmed (fail closed);
/// beyond-advert observation counts as Verified; otherwise Unadvertised.
fn hard_verdict(advertised: bool, observed: Option<bool>) -> Verdict {
    match (advertised, observed) {
        (true, Some(true)) => Verdict::Verified,
        (true, _) => Verdict::Unverified, // advertised but not confirmed
        (false, Some(true)) => Verdict::Verified, // observed beyond advert
        (false, _) => Verdict::Unadvertised,
    }
}

fn evidence_for(cap: Capability, advertised: bool, observed: Option<bool>) -> String {
    match observed {
        Some(true) => match cap {
            Capability::Tools => "tool-call round-trip succeeded".into(),
            Capability::StructuredOutput => "structured-output round-trip succeeded".into(),
            Capability::CodexResponses => "codex/responses round-trip succeeded".into(),
        },
        Some(false) => {
            if advertised {
                format!("advertised {cap:?} but probe observed a failure")
            } else {
                format!("{cap:?} probe failed (not advertised)")
            }
        }
        None => {
            if advertised {
                format!("advertised {cap:?} but probe did not confirm")
            } else {
                "not advertised, not probed".into()
            }
        }
    }
}

/// Convenience: the subset of `ProviderRecord.capabilities` routing may rely
/// on given a report (only `Verified` hard capabilities; advertised-but-
/// unverified are excluded). Unadvertised-but-observed capabilities are
/// returned as extra.
pub fn trusted_capabilities(report: &VerificationReport) -> Vec<Capability> {
    report
        .capability_verdicts
        .iter()
        .filter(|v| v.verdict == Verdict::Verified)
        .map(|v| v.capability)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertised(tools: bool, structured: bool, codex: bool) -> AdvertisedHardCaps {
        AdvertisedHardCaps {
            tools,
            structured_output: structured,
            codex_responses: codex,
        }
    }

    #[test]
    fn fully_verified_when_every_advertised_hard_cap_is_confirmed() {
        let advert = advertised(true, true, false);
        let observed = ProbeResult {
            tool_call_ok: Some(true),
            structured_output_ok: Some(true),
            ..Default::default()
        };
        let report = verify_report(&advert, &observed);
        assert!(report.is_fully_verified());
        assert_eq!(report.capability_verdicts.len(), 3); // all three hard caps get a row
        assert_eq!(
            report
                .capability_verdicts
                .iter()
                .filter(|v| v.verdict == Verdict::Verified)
                .count(),
            2
        );
        // codex was never advertised/observed → Unadvertised (honest, not trusted)
        let codex = report
            .capability_verdicts
            .iter()
            .find(|v| v.capability == Capability::CodexResponses)
            .unwrap();
        assert_eq!(codex.verdict, Verdict::Unadvertised);
        assert_eq!(trusted_capabilities(&report).len(), 2);
    }

    #[test]
    fn unverified_when_advertised_tools_probe_missing() {
        // advertised tools but the probe never confirmed → fail closed
        let advert = advertised(true, false, false);
        let observed = ProbeResult {
            tool_call_ok: Some(false),
            ..Default::default()
        };
        let report = verify_report(&advert, &observed);
        assert!(!report.is_fully_verified());
        let tools = report
            .capability_verdicts
            .iter()
            .find(|v| v.capability == Capability::Tools)
            .unwrap();
        assert_eq!(tools.verdict, Verdict::Unverified);
        assert!(!trusted_capabilities(&report).contains(&Capability::Tools));
    }

    #[test]
    fn none_probe_is_not_treated_as_verified() {
        // "no probe data" must be Unverified for a tools-advertising provider
        let advert = advertised(true, false, false);
        let observed = ProbeResult::unobserved();
        let report = verify_report(&advert, &observed);
        assert!(!report.is_fully_verified());
        assert!(!trusted_capabilities(&report).contains(&Capability::Tools));
    }

    /// A report that confirmed nothing is not "fully verified", even when the
    /// catalog advertised nothing to confirm. Vacuous truth here would let a
    /// bare reachability probe claim verified capabilities.
    #[test]
    fn a_report_that_confirmed_nothing_is_not_fully_verified() {
        for advertised in [
            advertised(false, false, false),
            advertised(true, false, false),
        ] {
            let report = verify_report(&advertised, &ProbeResult::unobserved());
            assert!(
                !report.is_fully_verified(),
                "unobserved probe must never report fully verified"
            );
            assert!(trusted_capabilities(&report).is_empty());
        }
    }

    #[test]
    fn observes_beyond_advert_as_verified_not_unadvertised() {
        let advert = advertised(false, false, false);
        let observed = ProbeResult {
            structured_output_ok: Some(true),
            ..Default::default()
        };
        let report = verify_report(&advert, &observed);
        assert!(report.is_fully_verified()); // nothing advertised → nothing to fail
        assert!(trusted_capabilities(&report).contains(&Capability::StructuredOutput));
    }

    #[test]
    fn context_ceiling_and_model_ids_recorded() {
        let advert = advertised(true, false, false);
        let observed = ProbeResult {
            observed_model_ids: vec!["pv/model-a".into(), "pv/model-b".into()],
            tool_call_ok: Some(true), // confirms the advertised hard cap
            observed_context_len: Some(32_000),
            ..Default::default()
        };
        let report = verify_report(&advert, &observed);
        assert!(report.is_fully_verified());
        assert_eq!(report.observed_model_ids.len(), 2);
        assert_eq!(report.observed_context_len, Some(32_000));
        assert!(
            report
                .capability_verdicts
                .iter()
                .any(|v| v.capability == Capability::StructuredOutput)
        );
    }
}
