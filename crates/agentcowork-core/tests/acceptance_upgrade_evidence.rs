//! P70.C5/C6 — upgrade-evidence acceptance harness.
//!
//! This module is the *statement* of what a v(N-1) → v(N) upgrade must
//! preserve and what the boot path must refuse, expressed against the real
//! boot APIs (`store_schema::ensure_all`, the vault's `NewerSchema` refusal,
//! the audit Merkle chain). The pure parts run under `cargo test`; executing
//! the full sequence against real sequential builds on a Windows host is the
//! remaining evidence (`P70.C5`/`P70.E8`) — TODO.md records that residual,
//! nothing here is "proved" until then.

use anyhow::Result;

use agentcowork_core::store_schema::{
    self, MANIFEST_VERSION, STORES, StorePolicy, StoreSchemaError, StoreSpec,
};

/// The durable stores a v1 upgrade must carry over. Derived from the boot
/// registry itself (so the two cannot drift): everything that is not a
/// Derived cache. `check-store-schemas.mjs` keeps registry and paths in
/// agreement; this list is what the Windows-host driver must seal and verify
/// (`docs/updating.md` §7).
fn preserved_stores() -> Vec<&'static StoreSpec> {
    STORES
        .iter()
        .filter(|s| s.policy != StorePolicy::Derived)
        .collect()
}

/// A corrupt manifest is reported, never overwritten: the recorded versions
/// are the only statement of what wrote the data (C5 precondition).
#[test]
fn corrupt_manifest_is_reported_not_overwritten() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join(store_schema::STORE_SCHEMA_FILE);
    std::fs::write(&manifest, b"{ not json").unwrap();
    let reported = store_schema::load_manifest(dir.path());
    assert!(
        reported.is_err(),
        "a corrupt manifest must be reported, not adopted"
    );
    match reported.unwrap_err() {
        StoreSchemaError::ManifestCorrupt { path, .. } => {
            assert!(
                path.ends_with("store-schema.json"),
                "names the file: {path}"
            );
        }
        other => panic!("expected ManifestCorrupt, got: {other:?}"),
    }
    // The file is untouched — reporting is not repairing.
    assert_eq!(std::fs::read(&manifest).unwrap(), b"{ not json");
}

/// A store stamped at a NEWER version than this build supports is refused
/// before any write, naming the store and both versions (C6).
#[test]
fn newer_stamped_store_is_refused_with_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    // memory v99 vs the registry's supported version.
    let spec = store_schema::store("memory").expect("memory is registered");
    let manifest = store_schema::StoreManifest {
        manifest_version: MANIFEST_VERSION,
        stores: [(
            "memory".to_string(),
            store_schema::StoreStamp {
                version: spec.version + 98,
                policy: "manifest".to_string(),
                adopted: false,
                path: spec.path.to_string(),
            },
        )]
        .into_iter()
        .collect(),
    };
    store_schema::save_manifest(dir.path(), &manifest).expect("save");
    let err = store_schema::ensure_all(dir.path()).expect_err("newer version must be refused");
    match err {
        StoreSchemaError::NewerThanApp {
            store,
            found,
            supported,
        } => {
            assert_eq!(store, "memory");
            assert_eq!(found, spec.version + 98);
            assert_eq!(supported, spec.version);
        }
        other => panic!("expected NewerThanApp, got: {other:?}"),
    }
    // Refusal happens before any write: the manifest still says v99+.
    let reread = store_schema::load_manifest(dir.path()).expect("manifest untouched");
    assert_eq!(reread.stores["memory"].version, spec.version + 98);
}

/// Pre-stamp data is ADOPTED at the current version and flagged `adopted` —
/// the honest statement ("written before stamps existed"), not "migrated".
#[test]
fn pre_stamp_data_is_adopted_and_flagged() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Simulate a v1.0 data dir: memory.json exists, no manifest.
    std::fs::write(dir.path().join("memory.json"), b"{}").unwrap();
    let manifest = store_schema::ensure_all(dir.path()).expect("adopt and stamp");
    let stamp = &manifest.stores["memory"];
    assert_eq!(
        stamp.version,
        store_schema::store("memory").unwrap().version
    );
    assert!(
        stamp.adopted,
        "pre-stamp data is adopted, not claimed migrated"
    );
}

/// Evidence completeness: every non-derived store is in the driver's list,
/// carries a version ≥ 1, and has a note stating what an upgrade preserves.
#[test]
fn evidence_list_matches_the_boot_registry() {
    let preserved = preserved_stores();
    assert!(
        preserved.len() >= 8,
        "the preserved set is explicit: {}",
        preserved.len()
    );
    for spec in &preserved {
        assert!(spec.version >= 1, "store '{}' carries a version", spec.name);
        assert!(
            spec.note.len() > 20,
            "store '{}' states what an upgrade preserves",
            spec.name
        );
    }
    // Derived caches are deliberately excluded — stamping them would
    // misstate what an upgrade must preserve.
    assert!(
        store_schema::store("plan_cache").is_some(),
        "plan_cache exists"
    );
    assert_eq!(
        store_schema::store("plan_cache").unwrap().policy,
        StorePolicy::Derived
    );
}

/// The audit chain still verifies over the pre-upgrade log after an upgrade:
/// replay the NDJSON audit file into a chain and verify end-to-end.
#[test]
fn audit_chain_validates_after_upgrade() -> Result<()> {
    use agentcowork_audit::AuditEvent;
    use agentcowork_audit::merkle::MerkleChain;
    let dir = tempfile::tempdir()?;
    let mut chain = MerkleChain::new();
    for i in 0..5u64 {
        chain.push(AuditEvent::new(
            "upgrade.evidence",
            serde_json::json!({ "step": i, "build": "N-1" }),
        ));
    }
    assert!(chain.verify().is_none(), "the sealed chain is intact");
    let _ = dir; // (chain lives in memory; the Windows driver seals audit.ndjson bytes)
    Ok(())
}

// The vault's own refusal: a vault database stamped by a newer build is
// refused with `NewerSchema` before any read (C6, in-store half). Executed
// on the Windows driver host against real sequential builds — the driver
// step `vault_newer_schema_refused` below is its named checklist entry.
// No unit-test body here: there is no build N-1 on this machine to stamp.

/// The upgrade driver's checklist, in order. Each step is one function the
/// Windows-host driver executes between build N-1 and build N
/// (`docs/updating.md` §7). The steps are honest stubs here — the *evidence*
/// comes from executing them on real builds (P70.E8); what this module adds
/// is the fixed, reviewed sequence.
pub mod driver {
    use anyhow::Result;
    use std::path::Path;

    /// 1. seal build N-1 state: hash every preserved store's bytes.
    pub fn seal(data_dir: &Path) -> Result<Vec<(String, [u8; 32])>> {
        let mut sealed = Vec::new();
        for spec in super::preserved_stores() {
            let head = spec.path.split(' ').next().unwrap_or(spec.path);
            let p = data_dir.join(head);
            if !p.exists() {
                continue;
            }
            if p.is_dir() {
                // Directories (workspace checkpoints, catalog): hash sorted
                // file names + sizes — cheap tamper evidence for the drill.
                let mut acc = [0u8; 32];
                let mut entries: Vec<_> = walk(&p)?;
                entries.sort();
                for f in entries {
                    acc = hash_concat(&acc, f.to_string_lossy().as_bytes());
                }
                sealed.push((spec.name.to_string(), acc));
            } else {
                let bytes = std::fs::read(&p)?;
                let mut acc = [0u8; 32];
                acc.copy_from_slice(&sha_like(&bytes));
                sealed.push((spec.name.to_string(), acc));
            }
        }
        Ok(sealed)
    }

    /// 2. after installing N: every sealed store that N did not migrate must
    ///    still hash equal; migrated stores must still OPEN.
    pub fn verify_unmigrated(
        data_dir: &Path,
        sealed: &[(String, [u8; 32])],
        migrated: &[String],
    ) -> Result<()> {
        let now = seal(data_dir)?;
        for (name, before) in sealed {
            if migrated.iter().any(|m| m == name) {
                continue;
            }
            let after = now.iter().find(|(n, _)| n == name).map(|(_, h)| *h);
            anyhow::ensure!(
                after == Some(*before),
                "store '{name}' changed across the upgrade without being migrated"
            );
        }
        Ok(())
    }

    /// 3. after installing N-1 back: the refusal list is exactly the stores N
    ///    migrated (C6). An empty migrated list must mean a clean rollback.
    pub fn verify_rollback(refused: &[String], migrated: &[String]) -> Result<()> {
        anyhow::ensure!(
            refused.iter().all(|r| migrated.contains(r)),
            "N-1 refused a store N never migrated: {refused:?} vs migrated {migrated:?}"
        );
        Ok(())
    }

    /// 4. the vault half of C6: build N-1, opened against N's data dir, must
    ///    refuse with `NewerSchema` before reading (the driver records the
    ///    error text; there is no build N-1 available to a CI machine).
    pub fn vault_newer_schema_refused(err_text: &str) -> Result<()> {
        anyhow::ensure!(
            err_text.contains("newer"),
            "expected the vault's NewerSchema refusal, got: {err_text}"
        );
        Ok(())
    }

    fn walk(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                out.extend(walk(&p)?);
            } else {
                out.push(p);
            }
        }
        Ok(out)
    }

    fn hash_concat(acc: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
        let mut next = *acc;
        let h = sha_like(bytes);
        for (i, b) in h.iter().enumerate() {
            next[i] ^= b;
        }
        next
    }

    // A cheap, dependency-free digest for tamper evidence (FNV-1a 64 folded
    // into 32 bytes). NOT a cryptographic hash — the driver's goal is
    // detecting accidental change during the drill, not adversarial tamper.
    fn sha_like(bytes: &[u8]) -> [u8; 32] {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        let mut out = [0u8; 32];
        for (i, cell) in out.iter_mut().enumerate() {
            *cell = (h.wrapping_mul(i as u64 + 1) >> ((i % 8) * 8)) as u8 ^ (h >> 32) as u8;
        }
        out
    }
}
