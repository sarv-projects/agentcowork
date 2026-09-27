//! P7.7 — Ed25519-signed extension manifests (OpenFang pattern). An
//! extension bundle ships a manifest (name, version, tool list, permission
//! demands) plus a signature. The runtime only loads manifests whose
//! signature verifies against a trusted key; a bad bundle is rejected
//! outright — a signed manifest is the *only* way a tool gains capability.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// A signed extension manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    /// The manifest body (JSON, canonical).
    pub body: String,
    /// base64(Ed25519 signature over `body`).
    pub signature_b64: String,
}

/// The manifest body the signature covers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestBody {
    pub name: String,
    pub version: String,
    /// Tool ids this extension registers.
    pub tools: Vec<String>,
    /// Permission demands (e.g. `["shell.exec", "fs.write", "network"]`).
    pub permissions: Vec<String>,
    /// Optional canonical source URL (shown to the user, never trusted).
    #[serde(default)]
    pub source: String,
}

impl ManifestBody {
    /// Canonical serialization — the exact bytes the signature must cover.
    pub fn canonical(&self) -> String {
        serde_json::to_string(self).expect("manifest serializes")
    }
}

/// Verify a signed manifest against a trusted public key (base64).
pub fn verify_manifest(
    signed: &SignedManifest,
    public_key_b64: &str,
) -> Result<ManifestBody, ManifestError> {
    let pk_bytes = B64
        .decode(public_key_b64)
        .map_err(|_| ManifestError::BadKey)?;
    let pk = VerifyingKey::from_bytes(
        pk_bytes
            .as_slice()
            .try_into()
            .map_err(|_| ManifestError::BadKey)?,
    )?;
    let sig_bytes = B64
        .decode(&signed.signature_b64)
        .map_err(|_| ManifestError::BadSignature)?;
    let sig = Signature::from_bytes(
        sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| ManifestError::BadSignature)?,
    );
    pk.verify(signed.body.as_bytes(), &sig)?;
    let body: ManifestBody =
        serde_json::from_str(&signed.body).map_err(|_| ManifestError::MalformedBody)?;
    Ok(body)
}

/// Sign a manifest body (used by the extension author / test fixtures).
pub fn sign_manifest(body: &ManifestBody, signing_key: &SigningKey) -> SignedManifest {
    let canonical = body.canonical();
    let sig = signing_key.sign(canonical.as_bytes());
    SignedManifest {
        body: canonical,
        signature_b64: B64.encode(sig.to_bytes()),
    }
}

/// Verify + reject bundles that demand capabilities they shouldn't.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("bad public key")]
    BadKey,
    #[error("bad signature encoding")]
    BadSignature,
    #[error("signature verification failed")]
    InvalidSignature(#[from] ed25519_dalek::SignatureError),
    #[error("manifest body is not valid JSON")]
    MalformedBody,
}

/// Check a verified manifest against a capability allowlist. A manifest
/// demanding a capability not in the allowlist is rejected.
pub fn check_capabilities(body: &ManifestBody, allowed: &[&str]) -> Result<(), String> {
    for p in &body.permissions {
        if !allowed.contains(&p.as_str()) {
            return Err(format!(
                "manifest '{}' demands unlisted capability '{}'",
                body.name, p
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn keypair() -> (SigningKey, String) {
        let sk = SigningKey::generate(&mut OsRng);
        let pk_b64 = B64.encode(sk.verifying_key().to_bytes());
        (sk, pk_b64)
    }

    fn body() -> ManifestBody {
        ManifestBody {
            name: "demo-extension".to_string(),
            version: "1.0.0".to_string(),
            tools: vec!["demo.ping".to_string()],
            permissions: vec!["fs.read".to_string()],
            source: String::new(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let (sk, pk) = keypair();
        let signed = sign_manifest(&body(), &sk);
        let verified = verify_manifest(&signed, &pk).expect("verifies");
        assert_eq!(verified.name, "demo-extension");
    }

    #[test]
    fn tampered_body_rejected() {
        let (sk, pk) = keypair();
        let mut signed = sign_manifest(&body(), &sk);
        // Tamper with the body (keep signature) → must fail.
        let mut tampered: ManifestBody = serde_json::from_str(&signed.body).unwrap();
        tampered.permissions.push("shell.exec".to_string());
        signed.body = tampered.canonical();
        assert!(verify_manifest(&signed, &pk).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        let (sk, _pk) = keypair();
        let (_, other_pk) = keypair();
        let signed = sign_manifest(&body(), &sk);
        assert!(verify_manifest(&signed, &other_pk).is_err());
    }

    #[test]
    fn unlisted_capability_rejected() {
        let body = ManifestBody {
            permissions: vec!["shell.exec".to_string()],
            ..body()
        };
        let err = check_capabilities(&body, &["fs.read"]).unwrap_err();
        assert!(err.contains("unlisted capability"));
    }

    #[test]
    fn listed_capability_accepted() {
        assert!(check_capabilities(&body(), &["fs.read"]).is_ok());
    }
}
