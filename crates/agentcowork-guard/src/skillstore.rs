//! P9.7 — Skills/plugins store index signing (marketplace canonicalizer).
//!
//! The Connect Store (`agentcowork-mcp::store`) is the *connector* surface.
//! This is the *skills/plugins* store companion: a curated **registry index**
//! of skills with per-entry capability demands, signed as one Ed25519 list.
//!
//! Why a single signed index (instead of per-skill manifests only): the
//! coordinator has to know what is *in* the store before any skill is
//! installed — that list itself is trust-bearing. A tampered index could
//! advertise a skill that demands `shell.exec` while the card claims
//! "reads your notes". So the whole index is covered by one signature against
//! a pinned public key; the runtime installs nothing from an index that does
//! not verify.
//!
//! Flow (mirrors ARCH/15 tier-3 "skills = MCP + SKILL.md"):
//!   verify_skill_index(index, key)
//!     → signed index validates (tamper + spoof-proof)
//!     → per-skill capability allowlist check (Guard-2 consent surface)
//!     → runtime installs only listed skills under Guard-2 consent.
//!
//! Per-skill content integrity is the job of `manifest` (P7.7); this module
//! is the *catalog* that points to it.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One skill row in the store index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillRow {
    /// Stable slug, e.g. `docx-assistant`.
    pub id: String,
    /// Human name, e.g. "DOCX Assistant".
    pub name: String,
    /// Version (semver-ish).
    pub version: String,
    /// Short description shown in the store card.
    pub description: String,
    /// Capability demands — must be a subset of the runtime allowlist or the
    /// whole index is rejected. This is what Guard-2 renders as consent.
    pub permissions: Vec<String>,
    /// Optional manifest id this row points at (P7.7) in the skills bundle.
    #[serde(default)]
    pub manifest: String,
}

impl SkillRow {
    /// Canonical serialization — byte-exact so the signature is reproducible.
    pub fn canonical(&self) -> String {
        serde_json::to_string(self).expect("skill row serializes")
    }
}

/// A signed store index: the exact ordered list + one Ed25519 signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedSkillIndex {
    /// Canonical JSON array of [`SkillRow`].
    pub body: String,
    /// base64(Ed25519 signature over `body`).
    pub signature_b64: String,
}

/// The capability allowlist the runtime ships (least privilege).
///
/// This is deliberately separate from any one skill — it is the *floor*:
/// top-level capabilities no skill may exceed without a new signed release.
/// Add capabilities here only after a security review.
pub const RUNTIME_CAPABILITY_ALLOWLIST: &[&str] = &[
    "fs.read",
    "fs.write",       // gated by an explicit write approval (Guard-2 risk card)
    "tool.mcp",       // call {stdio,http} MCP tools
    "tool.connector", // call a connected connector tool
];

/// Construct a signed index from rows.
pub fn sign_skill_index(rows: &[SkillRow], signing_key: &SigningKey) -> SignedSkillIndex {
    let body = serde_json::to_string(rows).expect("index serializes");
    let sig = signing_key.sign(body.as_bytes());
    SignedSkillIndex {
        body,
        signature_b64: B64.encode(sig.to_bytes()),
    }
}

/// Verify a signed index against a trusted key and return the canonical rows.
pub fn verify_skill_index(
    signed: &SignedSkillIndex,
    public_key_b64: &str,
) -> Result<Vec<SkillRow>, SkillStoreError> {
    let pk_bytes = B64
        .decode(public_key_b64)
        .map_err(|_| SkillStoreError::BadKey)?;
    let pk = VerifyingKey::from_bytes(
        pk_bytes
            .as_slice()
            .try_into()
            .map_err(|_| SkillStoreError::BadKey)?,
    )?;
    let sig_bytes = B64
        .decode(&signed.signature_b64)
        .map_err(|_| SkillStoreError::BadSignature)?;
    let sig = Signature::from_bytes(
        sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| SkillStoreError::BadSignature)?,
    );
    pk.verify(signed.body.as_bytes(), &sig)?;
    serde_json::from_str::<Vec<SkillRow>>(&signed.body).map_err(|_| SkillStoreError::MalformedBody)
}

/// Reject an index with duplicate ids or rows that demand capabilities
/// outside the runtime allowlist (structural + capability validation).
pub fn validate_skill_index(rows: &[SkillRow], allowlist: &[&str]) -> Result<(), SkillStoreError> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        if row.id.trim().is_empty() {
            return Err(SkillStoreError::EmptyId);
        }
        if !seen.insert(row.id.clone()) {
            return Err(SkillStoreError::DuplicateId(row.id.clone()));
        }
        for p in &row.permissions {
            if !allowlist.contains(&p.as_str()) {
                return Err(SkillStoreError::UnlistedCapability(
                    row.name.clone(),
                    p.clone(),
                ));
            }
        }
    }
    Ok(())
}

/// Convenience: verify + validate in one call.
pub fn verify_and_validate(
    signed: &SignedSkillIndex,
    public_key_b64: &str,
    allowlist: &[&str],
) -> Result<Vec<SkillRow>, SkillStoreError> {
    let rows = verify_skill_index(signed, public_key_b64)?;
    validate_skill_index(&rows, allowlist)?;
    Ok(rows)
}

/// Look up a skill by slug (post-verify).
pub fn get_skill<'a>(rows: &'a [SkillRow], id: &str) -> Option<&'a SkillRow> {
    rows.iter().find(|r| r.id == id)
}

/// Index by id — convenient for install gating.
pub fn by_id(rows: &[SkillRow]) -> BTreeMap<String, &SkillRow> {
    rows.iter().map(|r| (r.id.clone(), r)).collect()
}

/// A key pair bundle helper for tests / tooling (the store operator holds
/// the signing key; the app ships only the verifying public key).
pub struct StoreKeys {
    pub signing: SigningKey,
    pub public_key_b64: String,
}

/// Generate a fresh key pair (store-operator tooling / tests).
pub fn generate_store_keys() -> StoreKeys {
    use rand::rngs::OsRng;
    let signing = SigningKey::generate(&mut OsRng);
    let public_key_b64 = B64.encode(signing.verifying_key().to_bytes());
    StoreKeys {
        signing,
        public_key_b64,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillStoreError {
    #[error("bad store public key")]
    BadKey,
    #[error("bad signature encoding")]
    BadSignature,
    #[error("index signature verification failed")]
    InvalidSignature(#[from] ed25519_dalek::SignatureError),
    #[error("index body is not a valid JSON array")]
    MalformedBody,
    #[error("skill row has an empty id")]
    EmptyId,
    #[error("duplicate skill id `{0}`")]
    DuplicateId(String),
    #[error("skill `{0}` demands unlisted capability `{1}`")]
    UnlistedCapability(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> StoreKeys {
        generate_store_keys()
    }

    fn rows() -> Vec<SkillRow> {
        vec![
            SkillRow {
                id: "docx-assistant".into(),
                name: "DOCX Assistant".into(),
                version: "1.2.0".into(),
                description: "Draft and format .docx documents.".into(),
                permissions: vec!["fs.write".into(), "tool.mcp".into()],
                manifest: "docx-assistant".into(),
            },
            SkillRow {
                id: "note-taker".into(),
                name: "Note Taker".into(),
                version: "0.9.0".into(),
                description: "Read your notes and surface relevant ones.".into(),
                permissions: vec!["fs.read".into()],
                manifest: String::new(),
            },
        ]
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let k = keys();
        let signed = sign_skill_index(&rows(), &k.signing);
        let verified =
            verify_and_validate(&signed, &k.public_key_b64, RUNTIME_CAPABILITY_ALLOWLIST)
                .expect("index verifies + validates");
        assert_eq!(verified.len(), 2);
        assert_eq!(
            get_skill(&verified, "note-taker").unwrap().name,
            "Note Taker"
        );
    }

    #[test]
    fn tampered_index_rejected() {
        let k = keys();
        let signed = sign_skill_index(&rows(), &k.signing);
        // Tamper the body (add a dangerous capability) keeping the signature.
        let mut tampered: Vec<SkillRow> = serde_json::from_str(&signed.body).unwrap();
        tampered[0].permissions.push("shell.exec".to_string());
        let new_signed = SignedSkillIndex {
            body: serde_json::to_string(&tampered).unwrap(),
            signature_b64: signed.signature_b64.clone(),
        };
        assert!(verify_skill_index(&new_signed, &k.public_key_b64).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        let sk1 = generate_store_keys();
        let sk2 = generate_store_keys();
        let signed = sign_skill_index(&rows(), &sk1.signing);
        assert!(verify_skill_index(&signed, &sk2.public_key_b64).is_err());
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut r = rows();
        r.push(r[0].clone());
        let err = validate_skill_index(&r, RUNTIME_CAPABILITY_ALLOWLIST).unwrap_err();
        assert!(matches!(err, SkillStoreError::DuplicateId(_)));
    }

    #[test]
    fn unlisted_capability_rejected() {
        let mut r = rows();
        r[0].permissions.push("network.exfil".to_string());
        let err = validate_skill_index(&r, RUNTIME_CAPABILITY_ALLOWLIST).unwrap_err();
        assert!(matches!(err, SkillStoreError::UnlistedCapability(_, _)));
    }

    #[test]
    fn capability_floor_respected() {
        // fs.write is an explicit, allowlisted capability → must pass.
        let r = rows();
        assert!(validate_skill_index(&r, RUNTIME_CAPABILITY_ALLOWLIST).is_ok());
        // by_id maps correctly post-verify.
        let k = keys();
        let signed = sign_skill_index(&r, &k.signing);
        let verified = verify_skill_index(&signed, &k.public_key_b64).unwrap();
        let map = by_id(&verified);
        assert!(map.contains_key("docx-assistant"));
        assert_eq!(map.len(), 2);
    }
}
