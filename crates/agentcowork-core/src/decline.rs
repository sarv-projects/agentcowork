//! Decline-list guard (doc 80/81/82): the features this product **refuses to
//! build or claim** until their gates are met. Two halves:
//!
//! 1. [`DeclineList`] — structural no-goes (no gen-media front-ends, no
//!    connector-count marketing, no silent autonomy, no replacement
//!    browser/IDE, no recursive swarms). Refusing is a hard error, not a
//!    warning.
//! 2. [`ClaimGate`] — marketing claims ("teach once", "broadest control
//!    plane") gated on the architecture Gates A/B/D: the claim is only
//!    permitted when the required gates are met (doc 82 §3 — Gate A = live
//!    ticketed executor, Gate B = recovery evidence, Gate D = simulator).

use serde::{Deserialize, Serialize};

/// The architecture gates (doc 82 §3) as a state struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateState {
    /// Gate A — live ticketed executor (landed 2026-08-20).
    pub a: bool,
    /// Gate B — receipt/recovery evidence.
    pub b: bool,
    /// Gate D — simulator/fixtures.
    pub d: bool,
}

impl GateState {
    pub fn met(&self, gate: ClaimGate) -> bool {
        match gate {
            ClaimGate::A => self.a,
            ClaimGate::B => self.b,
            ClaimGate::D => self.d,
        }
    }
}

/// Which architecture gate a claim depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ClaimGate {
    A,
    B,
    D,
}

/// The structural decline list (doc 82 §1 ⛔ AVOID / 🚫 IGNORE, endorsed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclinedFeature {
    /// No generative-media front-ends (image/video gen UIs).
    GenMediaFrontend,
    /// No connector-count marketing.
    ConnectorCountMarketing,
    /// No silent autonomy — every mutation stays governed.
    SilentAutonomy,
    /// No replacement browser.
    ReplacementBrowser,
    /// No replacement IDE.
    ReplacementIde,
    /// No recursive swarms.
    RecursiveSwarms,
}

/// The decline verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclineVerdict {
    /// Allowed — the feature/claim is not on the decline list / gate met.
    Allowed,
    /// Structurally refused — never built, no gate can lift it.
    Refused,
    /// The claim is gated and the required gate is not met yet.
    Gated { gate: ClaimGate },
}

/// The guard — pure, deterministic.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeclineGuard;

impl DeclineGuard {
    /// Is this feature on the structural decline list?
    pub fn feature(&self, f: DeclinedFeature) -> DeclineVerdict {
        match f {
            DeclinedFeature::GenMediaFrontend
            | DeclinedFeature::ConnectorCountMarketing
            | DeclinedFeature::SilentAutonomy
            | DeclinedFeature::ReplacementBrowser
            | DeclinedFeature::ReplacementIde
            | DeclinedFeature::RecursiveSwarms => DeclineVerdict::Refused,
        }
    }

    /// May we make this claim given the current gate state?
    pub fn claim(&self, claim: &str, gates: GateState) -> DeclineVerdict {
        match claim {
            // "Teach once" (doc 81 §3.1 reframe) — needs the simulator.
            "teach once" | "teach_once" => {
                if gates.d {
                    DeclineVerdict::Allowed
                } else {
                    DeclineVerdict::Gated { gate: ClaimGate::D }
                }
            }
            // "Broadest control plane" — needs the ticketed executor + recovery.
            "broadest control plane" | "broadest_control_plane" => {
                if gates.a && gates.b {
                    DeclineVerdict::Allowed
                } else if !gates.a {
                    DeclineVerdict::Gated { gate: ClaimGate::A }
                } else {
                    DeclineVerdict::Gated { gate: ClaimGate::B }
                }
            }
            _ => DeclineVerdict::Allowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_declines_are_always_refused() {
        let g = DeclineGuard;
        for f in [
            DeclinedFeature::GenMediaFrontend,
            DeclinedFeature::ConnectorCountMarketing,
            DeclinedFeature::SilentAutonomy,
            DeclinedFeature::ReplacementBrowser,
            DeclinedFeature::ReplacementIde,
            DeclinedFeature::RecursiveSwarms,
        ] {
            assert_eq!(g.feature(f), DeclineVerdict::Refused);
        }
    }

    #[test]
    fn teach_once_is_gated_on_simulator() {
        let g = DeclineGuard;
        assert_eq!(
            g.claim("teach once", GateState::default()),
            DeclineVerdict::Gated { gate: ClaimGate::D }
        );
        assert_eq!(
            g.claim(
                "teach once",
                GateState {
                    d: true,
                    ..Default::default()
                }
            ),
            DeclineVerdict::Allowed
        );
    }

    #[test]
    fn control_plane_claim_needs_a_and_b() {
        let g = DeclineGuard;
        let none = GateState::default();
        assert_eq!(
            g.claim("broadest control plane", none),
            DeclineVerdict::Gated { gate: ClaimGate::A }
        );
        let a_only = GateState {
            a: true,
            ..Default::default()
        };
        assert_eq!(
            g.claim("broadest control plane", a_only),
            DeclineVerdict::Gated { gate: ClaimGate::B }
        );
        let ab = GateState {
            a: true,
            b: true,
            ..Default::default()
        };
        assert_eq!(
            g.claim("broadest control plane", ab),
            DeclineVerdict::Allowed
        );
    }

    #[test]
    fn unlisted_claims_are_allowed() {
        let g = DeclineGuard;
        assert_eq!(
            g.claim("local-first", GateState::default()),
            DeclineVerdict::Allowed
        );
    }
}
