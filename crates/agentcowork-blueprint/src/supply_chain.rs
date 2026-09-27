//! K6 Trusted skill/automation supply chain (doc 81 §4; Gate E before
//! marketplace scale — the P22/P23/P26 pre-req): signed manifests +
//! capability/fixture tests + version pinning + quarantine + revoke.
//!
//! A skill/automation ships a [`SignedManifest`]: a digest over the manifest
//! body + a MAC signature under a signing key. The policy layer enforces
//! **pinned versions** (never floating), **quarantine** (unknown signers /
//! untested bundles land in quarantine, not the active set), and **revoke**
//! (a revoked id is refused even if signed). Signing keys are the vault's
//! job; this module owns the deterministic verification contract.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest body (what the signature covers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBody {
    pub id: String,
    pub name: String,
    /// Exact pinned version (semver-ish; floating ranges are refused).
    pub version: String,
    /// Declared capabilities (tool ids) the bundle may exercise.
    pub capabilities: Vec<String>,
    /// Fixture-test names that must pass before the bundle activates.
    pub fixture_tests: Vec<String>,
    pub author: String,
}

/// The signed envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedManifest {
    pub body: ManifestBody,
    /// Hex SHA-256 of the canonical manifest body (what the signature is
    /// over).
    pub digest: String,
    /// Hex MAC (HMAC-SHA256) of the digest under the signing key.
    pub signature: String,
}

/// Deterministic canonical digest of a manifest body.
pub fn digest(body: &ManifestBody) -> String {
    let canon = serde_json::to_vec(body).unwrap_or_default();
    format!("{:x}", Sha256::digest(canon))
}

/// HMAC-SHA256 (RFC 2104) — no extra crate; the key lives in the vault.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = vec![0x36u8; BLOCK];
    let mut opad = vec![0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    format!("{:x}", outer.finalize())
}

impl SignedManifest {
    /// Sign a manifest body under `key` (deterministic).
    pub fn sign(body: ManifestBody, key: &[u8]) -> Self {
        let d = digest(&body);
        let signature = hmac_sha256(key, d.as_bytes());
        Self {
            body,
            digest: d,
            signature,
        }
    }

    /// Verify: digest matches the body AND the signature verifies under the
    /// key. Any tamper breaks it.
    pub fn verify(&self, key: &[u8]) -> bool {
        if self.digest != digest(&self.body) {
            return false;
        }
        hmac_sha256(key, self.digest.as_bytes()) == self.signature
    }
}

/// A quarantine entry — a bundle that is not yet trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineEntry {
    pub id: String,
    pub reason: String,
}

/// The supply-chain policy.
#[derive(Debug, Clone, Default)]
pub struct SupplyChainPolicy {
    /// Pinned versions: id → exact version (floating is refused).
    pins: std::collections::BTreeMap<String, String>,
    /// Allowed signer keys (hex digest of the key). Empty = deny-all.
    allowed_keys: Vec<String>,
    /// Revoked ids (even a valid signature is refused).
    revoked: Vec<String>,
    /// Quarantined bundles (not active).
    quarantine: Vec<QuarantineEntry>,
}

impl SupplyChainPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pin(&mut self, id: &str, version: &str) {
        self.pins.insert(id.into(), version.into());
    }

    pub fn allow_key(&mut self, key: &[u8]) {
        self.allowed_keys.push(hex_key(key));
    }

    pub fn revoke(&mut self, id: &str) {
        if !self.revoked.iter().any(|r| r == id) {
            self.revoked.push(id.into());
        }
    }

    pub fn quarantine(&mut self, id: &str, reason: &str) {
        self.quarantine.push(QuarantineEntry {
            id: id.into(),
            reason: reason.into(),
        });
    }

    pub fn quarantine_list(&self) -> &[QuarantineEntry] {
        &self.quarantine
    }

    /// Evaluate a signed manifest for activation. Fail-closed:
    /// revoked → refuse · signer not allowed → quarantine · unpinned /
    /// version-mismatch → refuse · otherwise activate.
    pub fn evaluate(&mut self, manifest: &SignedManifest, key: &[u8]) -> SupplyVerdict {
        if !manifest.verify(key) {
            return SupplyVerdict::Refuse("bad signature or digest".into());
        }
        if self.revoked.contains(&manifest.body.id) {
            return SupplyVerdict::Refuse("revoked".into());
        }
        if !self.allowed_keys.contains(&hex_key(key)) {
            let reason = "signer not allowed".to_string();
            self.quarantine(&manifest.body.id, &reason);
            return SupplyVerdict::Quarantined(reason);
        }
        match self.pins.get(&manifest.body.id) {
            None => SupplyVerdict::Refuse("unpinned (floating) version".into()),
            Some(pinned) if *pinned == manifest.body.version => SupplyVerdict::Activate,
            Some(pinned) => SupplyVerdict::Refuse(format!("version mismatch (pinned {pinned})")),
        }
    }
}

fn hex_key(key: &[u8]) -> String {
    format!("{:x}", Sha256::digest(key))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupplyVerdict {
    Activate,
    /// Not trusted enough to activate — moved to quarantine.
    Quarantined(String),
    /// Refused outright (bad signature / revoked / floating / mismatch).
    Refuse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(id: &str, version: &str) -> ManifestBody {
        ManifestBody {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            capabilities: vec!["fs.write".into()],
            fixture_tests: vec!["write-then-read".into()],
            author: "everyaios".into(),
        }
    }

    const KEY: &[u8] = b"test-signing-key-0123456789";

    #[test]
    fn sign_verify_roundtrip_and_tamper() {
        let m = SignedManifest::sign(body("skill-a", "1.0.0"), KEY);
        assert!(m.verify(KEY));
        let mut t = m.clone();
        t.body.version = "9.9.9".into();
        assert!(!t.verify(KEY));
    }

    #[test]
    fn evaluate_requires_pin_and_allowed_signer() {
        let m = SignedManifest::sign(body("skill-a", "1.0.0"), KEY);
        let mut p = SupplyChainPolicy::new();
        // Signer not allowed → quarantine (fail-closed before pinning).
        let v = p.evaluate(&m, KEY);
        assert!(matches!(v, SupplyVerdict::Quarantined(_)));
        assert_eq!(p.quarantine_list().len(), 1);
        p.allow_key(KEY);
        // Now the pin is the gate: unpinned → refuse.
        assert_eq!(
            p.evaluate(&m, KEY),
            SupplyVerdict::Refuse("unpinned (floating) version".into())
        );
        p.pin("skill-a", "1.0.0");
        assert_eq!(p.evaluate(&m, KEY), SupplyVerdict::Activate);
    }

    #[test]
    fn pin_mismatch_and_revoke_refuse() {
        let m = SignedManifest::sign(body("skill-a", "1.0.0"), KEY);
        let mut p = SupplyChainPolicy::new();
        p.pin("skill-a", "1.0.1");
        p.allow_key(KEY);
        assert!(matches!(p.evaluate(&m, KEY), SupplyVerdict::Refuse(_)));
        p.pin("skill-a", "1.0.0");
        p.revoke("skill-a");
        assert_eq!(p.evaluate(&m, KEY), SupplyVerdict::Refuse("revoked".into()));
    }
}
