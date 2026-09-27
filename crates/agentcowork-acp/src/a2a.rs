//! A2A secondary interface (P6.8/J21).
//!
//! ACP remains the local harness protocol. This module only models the
//! remote-agent discovery boundary: an Agent Card, explicit capabilities,
//! signed-card metadata, and a verifier supplied by the host trust store. It
//! deliberately does not treat a discovered card as trusted and does not
//! implement remote task execution.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A discoverable remote agent capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// The identity and advertised surface of a remote A2A agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub id: String,
    pub name: String,
    pub version: String,
    pub endpoint: String,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
    /// Issuer/key id used to verify the signed card.
    pub issuer: String,
    pub key_id: String,
}

/// Signed envelope as received from a remote discovery endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedAgentCard {
    pub card: AgentCard,
    pub algorithm: String,
    pub signature: String,
}

/// Trust is explicit and never inferred from discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTrust {
    Verified,
    Unverified,
    Rejected,
}

/// Host-owned verification interface. A production implementation should
/// resolve `issuer`/`key_id` from a pinned trust store or authenticated key
/// discovery, then verify the canonical card bytes with the advertised
/// algorithm (for example JWS/Ed25519). No token is passed to the agent.
pub trait AgentCardVerifier {
    fn verify(&self, signed: &SignedAgentCard) -> Result<(), String>;
}

/// True when `endpoint` is `https` or loopback `http`, refusing insecure
/// remote `http`. The loopback check parses the host and requires a literal
/// loopback address — lookalikes like `http://localhost.evil.com` or
/// `http://127.0.0.1.nip.io` do **not** pass (bugfix 10).
pub fn is_secure_endpoint(endpoint: &str) -> Result<(), A2aError> {
    let url = url::Url::parse(endpoint).map_err(|_| A2aError::InsecureEndpoint)?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url
                .host_str()
                .unwrap_or("")
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_ascii_lowercase();
            let loopback = host == "127.0.0.1" || host == "localhost" || host == "::1";
            if !loopback {
                Err(A2aError::InsecureEndpoint)
            } else {
                Ok(())
            }
        }
        _ => Err(A2aError::InsecureEndpoint),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum A2aError {
    #[error("A2A card is missing a required identity field")]
    InvalidIdentity,
    #[error("A2A card endpoint must use https or loopback http")]
    InsecureEndpoint,
    #[error("A2A card signature is not verified: {0}")]
    Unverified(String),
}

impl SignedAgentCard {
    /// Validate the card shape and endpoint before any cryptographic work.
    pub fn validate(&self) -> Result<(), A2aError> {
        if self.card.id.trim().is_empty()
            || self.card.name.trim().is_empty()
            || self.card.version.trim().is_empty()
            || self.card.endpoint.trim().is_empty()
            || self.card.issuer.trim().is_empty()
            || self.card.key_id.trim().is_empty()
            || self.algorithm.trim().is_empty()
            || self.signature.trim().is_empty()
        {
            return Err(A2aError::InvalidIdentity);
        }
        is_secure_endpoint(&self.card.endpoint)?;
        Ok(())
    }

    /// Fail closed until the host verifier accepts the exact card.
    pub fn verify<V: AgentCardVerifier>(&self, verifier: &V) -> Result<CardTrust, A2aError> {
        self.validate()?;
        verifier
            .verify(self)
            .map(|_| CardTrust::Verified)
            .map_err(A2aError::Unverified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(endpoint: &str) -> SignedAgentCard {
        SignedAgentCard {
            card: AgentCard {
                id: "agent-1".into(),
                name: "Remote".into(),
                version: "1".into(),
                endpoint: endpoint.into(),
                skills: vec![],
                issuer: "issuer".into(),
                key_id: "key-1".into(),
            },
            algorithm: "Ed25519".into(),
            signature: "sig".into(),
        }
    }

    struct Accept;
    impl AgentCardVerifier for Accept {
        fn verify(&self, _signed: &SignedAgentCard) -> Result<(), String> {
            Ok(())
        }
    }

    struct Reject;
    impl AgentCardVerifier for Reject {
        fn verify(&self, _signed: &SignedAgentCard) -> Result<(), String> {
            Err("issuer not pinned".into())
        }
    }

    #[test]
    fn verified_card_requires_host_verifier() {
        assert_eq!(
            card("https://agent.example").verify(&Accept).unwrap(),
            CardTrust::Verified
        );
        assert_eq!(
            card("https://agent.example").verify(&Reject),
            Err(A2aError::Unverified("issuer not pinned".into()))
        );
    }

    #[test]
    fn insecure_remote_http_is_rejected() {
        assert_eq!(
            card("http://agent.example").validate(),
            Err(A2aError::InsecureEndpoint)
        );
        assert!(card("http://127.0.0.1:9000/a2a").validate().is_ok());
    }

    #[test]
    fn malformed_card_fails_closed() {
        let mut c = card("https://agent.example");
        c.signature.clear();
        assert_eq!(c.validate(), Err(A2aError::InvalidIdentity));
    }

    #[test]
    fn loopback_lookalikes_do_not_pass() {
        // Bugfix 10 — a prefix check would let these through; a parsed-host
        // check rejects them (they are external hosts that happen to start
        // with the loopback text or resolve to it via a wildcard).
        assert!(card("http://localhost.evil.com").validate().is_err());
        assert!(card("http://127.0.0.1.nip.io").validate().is_err());
        assert!(card("http://127.0.0.1.evil.io").validate().is_err());
        assert!(card("http://127.0.0.1.0.1").validate().is_err());
        // Genuine literal loopback still passes.
        assert!(card("http://127.0.0.1:9000/a2a").validate().is_ok());
        assert!(card("http://localhost:8080").validate().is_ok());
        assert!(card("http://[::1]:7000/a2a").validate().is_ok());
    }
}
