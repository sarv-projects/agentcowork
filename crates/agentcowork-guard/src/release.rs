//! K5 Data Release Firewall (doc 81 §3.2): the egress policy engine +
//! data-release receipts, with the **two enforcement zones** declared
//! (doc 81 §3.2 correction): the broker-mediated zone (inbuilt engine →
//! broker → provider, plus MCP/connector wrappers routed through the
//! broker) and the OS-egress zone (external ACP agents / MCP / browser with
//! their own network stacks, governed at the OS/proxy boundary).
//!
//! Every release is receipted; the acceptance test — "model routing cannot
//! bypass the broker" — is only true for the mediated zone, so the firewall
//! declares the envelope explicitly instead of pretending.

use serde::{Deserialize, Serialize};

/// The two enforcement zones (doc 81 §3.2). The product must declare which
/// zone a release path runs in — the receipt records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementZone {
    /// Inbuilt engine → broker → provider (and wrappers routed through the
    /// broker). Fully governed: the broker can scan, redact, and refuse.
    Mediated,
    /// External ACP agents / MCP servers / browser have their own network
    /// stacks — governed at the OS egress proxy, not the broker.
    OsEgress,
}

/// The release verdict for one payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseDecision {
    Allow,
    /// Allow with sensitive material redacted (the payload rewriter).
    Redact,
    Deny,
}

/// One data release — appended to the K1 receipt trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseReceipt {
    pub id: String,
    /// What left (payload description, never the payload itself).
    pub payload_desc: String,
    /// Destination (host / endpoint class).
    pub destination: String,
    pub zone: EnforcementZone,
    /// Which profile's rules applied.
    pub profile: String,
    pub decision: ReleaseDecision,
    /// The vault egress scan verdict (managed-secret / pattern hit), if any.
    pub scan_signal: Option<String>,
    pub released_at: u64,
}

/// Per-profile egress rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    pub profile: String,
    /// Zones this profile allows at all (a profile can refuse the OS-egress
    /// zone entirely).
    pub allowed_zones: Vec<EnforcementZone>,
    /// Destinations that are always denied (regardless of scan).
    pub deny_destinations: Vec<String>,
    /// Destinations that must be redacted (payload rewriter).
    pub redact_destinations: Vec<String>,
}

impl EgressPolicy {
    pub fn standard() -> Self {
        Self {
            profile: "standard".into(),
            allowed_zones: vec![EnforcementZone::Mediated, EnforcementZone::OsEgress],
            deny_destinations: vec![],
            redact_destinations: vec![],
        }
    }

    /// Strict: broker-mediated only — external agents must route through the
    /// broker or are refused at the envelope.
    pub fn strict() -> Self {
        Self {
            profile: "strict".into(),
            allowed_zones: vec![EnforcementZone::Mediated],
            deny_destinations: vec![],
            redact_destinations: vec![],
        }
    }
}

/// The egress policy engine: policy per profile + release receipts.
#[derive(Debug, Clone, Default)]
pub struct EgressPolicyEngine {
    policies: Vec<EgressPolicy>,
    receipts: Vec<ReleaseReceipt>,
}

impl EgressPolicyEngine {
    pub fn new(policies: Vec<EgressPolicy>) -> Self {
        Self {
            policies,
            receipts: Vec::new(),
        }
    }

    pub fn receipts(&self) -> &[ReleaseReceipt] {
        &self.receipts
    }

    /// Evaluate one release through a deterministic decision table
    /// (fail-closed): an unknown profile, a zone outside the profile, or a
    /// denied destination each Deny; a redact-listed destination yields
    /// Redact; anything else Allow. A `scan_signal` (managed secret /
    /// pattern) from the vault egress firewall downgrades Allow → Redact
    /// and Redact stays Redact.
    pub fn evaluate(
        &mut self,
        profile: &str,
        destination: &str,
        payload_desc: &str,
        zone: EnforcementZone,
        scan_signal: Option<String>,
        at_ms: u64,
    ) -> ReleaseDecision {
        let Some(policy) = self.policies.iter().find(|p| p.profile == profile) else {
            self.receipt(
                profile,
                destination,
                payload_desc,
                zone,
                ReleaseDecision::Deny,
                scan_signal,
                at_ms,
            );
            return ReleaseDecision::Deny;
        };
        if !policy.allowed_zones.contains(&zone) {
            self.receipt(
                profile,
                destination,
                payload_desc,
                zone,
                ReleaseDecision::Deny,
                scan_signal,
                at_ms,
            );
            return ReleaseDecision::Deny;
        }
        if policy
            .deny_destinations
            .iter()
            .any(|d| destination.contains(d))
        {
            self.receipt(
                profile,
                destination,
                payload_desc,
                zone,
                ReleaseDecision::Deny,
                scan_signal,
                at_ms,
            );
            return ReleaseDecision::Deny;
        }
        let redact = policy
            .redact_destinations
            .iter()
            .any(|d| destination.contains(d));
        let decision = if redact || scan_signal.is_some() {
            ReleaseDecision::Redact
        } else {
            ReleaseDecision::Allow
        };
        self.receipt(
            profile,
            destination,
            payload_desc,
            zone,
            decision,
            scan_signal,
            at_ms,
        );
        decision
    }

    // The decision table is intentionally explicit; each column is part of the
    // release contract and the receipt must persist them all.
    #[allow(clippy::too_many_arguments)]
    fn receipt(
        &mut self,
        profile: &str,
        destination: &str,
        payload_desc: &str,
        zone: EnforcementZone,
        decision: ReleaseDecision,
        scan_signal: Option<String>,
        at_ms: u64,
    ) {
        let id = format!("rel:{}", self.receipts.len());
        self.receipts.push(ReleaseReceipt {
            id,
            payload_desc: payload_desc.into(),
            destination: destination.into(),
            zone,
            profile: profile.into(),
            decision,
            scan_signal,
            released_at: at_ms,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_allows_mediated_clean() {
        let mut eng = EgressPolicyEngine::new(vec![EgressPolicy::standard()]);
        let d = eng.evaluate(
            "standard",
            "api.anthropic.com",
            "chat request",
            EnforcementZone::Mediated,
            None,
            1,
        );
        assert_eq!(d, ReleaseDecision::Allow);
        assert_eq!(eng.receipts().len(), 1);
    }

    #[test]
    fn strict_refuses_os_egress_zone() {
        let mut eng = EgressPolicyEngine::new(vec![EgressPolicy::strict()]);
        let d = eng.evaluate(
            "strict",
            "external-agent.example",
            "acp tool call",
            EnforcementZone::OsEgress,
            None,
            1,
        );
        assert_eq!(d, ReleaseDecision::Deny);
    }

    #[test]
    fn scan_signal_downgrades_to_redact() {
        let mut eng = EgressPolicyEngine::new(vec![EgressPolicy::standard()]);
        let d = eng.evaluate(
            "standard",
            "api.x.com",
            "payload",
            EnforcementZone::Mediated,
            Some("managed secret present".into()),
            1,
        );
        assert_eq!(d, ReleaseDecision::Redact);
    }

    #[test]
    fn unknown_profile_fails_closed() {
        let mut eng = EgressPolicyEngine::new(vec![EgressPolicy::standard()]);
        assert_eq!(
            eng.evaluate("nope", "any", "p", EnforcementZone::Mediated, None, 1),
            ReleaseDecision::Deny
        );
    }

    #[test]
    fn receipts_are_append_only() {
        let mut eng = EgressPolicyEngine::new(vec![EgressPolicy::standard()]);
        eng.evaluate("standard", "a", "p1", EnforcementZone::Mediated, None, 1);
        eng.evaluate("standard", "b", "p2", EnforcementZone::Mediated, None, 2);
        assert_eq!(eng.receipts().len(), 2);
        assert_ne!(eng.receipts()[0].id, eng.receipts()[1].id);
    }
}
