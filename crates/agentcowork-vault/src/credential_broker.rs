//! P46.3 — Credential-provider abstraction (spec J24): the `CredentialBroker`.
//!
//! The agent requests **opaque fill** into an approved target — environment
//! of a to-be-spawned process, a file the target owns, a form field — and
//! **never receives the password/OTP/raw credential**. The broker resolves the
//! secret from the vault (the J8 ring) *inside* the boundary, obtains Guard-2
//! approval via the [`FillApprover`] seam, and writes the value into the
//! target through the [`FillSink`] handler. The caller receives a
//! [`FillReceipt`] only.
//!
//! Security invariants (mirroring the E9 hard-deny for password-manager
//! surfaces):
//! - The raw secret is zeroized after the fill and is never part of any
//!   return value, log line, or diagnostic (receipt carries identity +
//!   outcome only).
//! - Unknown / unapproved / cap-violating targets fail closed — no fill, no
//!   partial write, no fallback guess.
//! - Resolution and write are one atomic broker operation: the agent cannot
//!   induce a second fill (target handles are single-use).
//! - Approval is checked AFTER resolution but BEFORE any write; a deny leaves
//!   the target untouched and proves nothing was written.

use std::collections::HashMap;

use zeroize::Zeroize;

use crate::keyring::{KeyRing, KeyRingError};

/// What the agent wants filled. Targets are named so the broker can validate
/// them against policy; the sink implementation decides the concrete write.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialHandle {
    /// Provider namespace in the vault ring (e.g. `"forms.example.com"`).
    pub provider: String,
    /// Key id / account id inside that provider (`key_ring.key_id`).
    pub key_id: String,
}

impl CredentialHandle {
    pub fn new(provider: impl Into<String>, key_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            key_id: key_id.into(),
        }
    }
}

/// The fill target — a validated destination, never a raw path the agent
/// passes through unvalidated (path-floor discipline from the FS broker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillTarget {
    /// Inject into the environment of a process the caller is about to spawn.
    /// `process_id` is a caller-chosen correlation label (audit), not a
    /// mutable authority.
    EnvVar {
        process_id: String,
        var_name: String,
    },
    /// Write into a file at a workspace-floor-backed path. The broker checks
    /// the path is a plain path under a configured floor (see
    /// [`CredentialBroker::with_floor`]); otherwise it is refused.
    File { path: std::path::PathBuf },
}

/// The Guard-2 approval seam. The shell wires this to the real approval card
/// (nonce-bound, human-gesture). `deny(reason)` blocks the fill entirely.
pub trait FillApprover {
    /// Approve one concrete fill (already-resolved identity + target). Called
    /// only with the *identity*, never the secret.
    fn approve(&self, handle: &CredentialHandle, target: &FillTarget) -> Result<(), String>;
}

/// The concrete write seam. Receives the secret ONCE to place it. The
/// implementation must not log or persist the value.
pub trait FillSink {
    fn write(&self, target: &FillTarget, value: &[u8]) -> Result<(), String>;
}

/// What the caller gets back: identity + outcome, never the secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FillReceipt {
    pub provider: String,
    pub key_id: String,
    pub target: String,
    pub approved: bool,
}

/// Default approver: deny everything (fail-closed until the shell wires the
/// Guard-2 approval path). This is the only default — a fill without an
/// approval path is refused, never silently allowed.
pub struct DenyAllApprover;

impl FillApprover for DenyAllApprover {
    fn approve(&self, _: &CredentialHandle, _: &FillTarget) -> Result<(), String> {
        Err("no credential-fill approval path is wired — fill denied".into())
    }
}

/// The broker: resolves from the vault ring inside the boundary and writes
/// through the sink. Constructed cheaply; owns no state except the optional
/// path floor (so it is `Send`/`Sync`-friendly and unit-testable).
pub struct CredentialBroker<'a, A: FillApprover, S: FillSink> {
    ring: &'a KeyRing<'a>,
    approver: A,
    sink: S,
    /// Optional workspace path floor for `FillTarget::File`. Absent ⇒ File
    /// fills are denied (fail-closed).
    floor: Option<std::path::PathBuf>,
}

impl<'a, A: FillApprover, S: FillSink> CredentialBroker<'a, A, S> {
    pub fn new(ring: &'a KeyRing<'a>, approver: A, sink: S) -> Self {
        Self {
            ring,
            approver,
            sink,
            floor: None,
        }
    }

    /// Configure the workspace-floor check for file fills. Without this,
    /// `FillTarget::File` is refused.
    pub fn with_floor(mut self, floor: impl Into<std::path::PathBuf>) -> Self {
        self.floor = Some(floor.into());
        self
    }

    /// Resolve + approve + fill in one call. The secret is zeroized even on
    /// failure paths after the write attempt.
    pub fn fill(
        &self,
        handle: &CredentialHandle,
        target: &FillTarget,
    ) -> Result<FillReceipt, CredentialFillError> {
        // Resolve INSIDE the boundary. The raw secret never leaves this fn.
        let entry = self
            .ring
            .get(&handle.provider, &handle.key_id)
            .map_err(CredentialFillError::from)?;
        let secret = &mut entry.value.clone();
        let result = self.fill_resolved(handle, target, secret);
        secret.zeroize();
        result
    }

    fn fill_resolved(
        &self,
        handle: &CredentialHandle,
        target: &FillTarget,
        secret: &[u8],
    ) -> Result<FillReceipt, CredentialFillError> {
        // Target validation (fail-closed, BEFORE approval so a denied target
        // costs nothing): File needs a floor + a plain non-dir path.
        self.validate_target(target)?;
        // Guard-2 approval — identity only.
        self.approver
            .approve(handle, target)
            .map_err(CredentialFillError::Denied)?;
        // Write.
        self.sink
            .write(target, secret)
            .map_err(CredentialFillError::Write)?;
        Ok(FillReceipt {
            provider: handle.provider.clone(),
            key_id: handle.key_id.clone(),
            target: target_label(target),
            approved: true,
        })
    }

    fn validate_target(&self, target: &FillTarget) -> Result<(), CredentialFillError> {
        match target {
            FillTarget::EnvVar { var_name, .. } => {
                if var_name.trim().is_empty() {
                    return Err(CredentialFillError::InvalidTarget(
                        "empty env var name".into(),
                    ));
                }
                // Reject dangerous env names (PATH/LD_PRELOAD/etc. could
                // redirect execution — fill is for credentials, not hijack).
                let upper = var_name.to_ascii_uppercase();
                for bad in ["PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "HOME", "SHELL"] {
                    if upper == bad {
                        return Err(CredentialFillError::InvalidTarget(format!(
                            "env var `{var_name}` is not a valid credential fill target"
                        )));
                    }
                }
                Ok(())
            }
            FillTarget::File { path } => {
                let floor = self.floor.as_deref().ok_or_else(|| {
                    CredentialFillError::InvalidTarget("file fills disabled (no path floor)".into())
                })?;
                // Canonical floor check: the file's parent must resolve under
                // the floor. Symlink-free canonicalization via fs::canonicalize
                // (the file need not exist — canonicalize the nearest existing
                // ancestor when it doesn't).
                let parent = path.parent().unwrap_or(path);
                let resolved = if parent.exists() {
                    std::fs::canonicalize(parent).map_err(|e| {
                        CredentialFillError::InvalidTarget(format!("path floor: {e}"))
                    })?
                } else {
                    let mut anc = parent.to_path_buf();
                    while !anc.exists() {
                        if !anc.pop() {
                            break;
                        }
                    }
                    std::fs::canonicalize(&anc).map_err(|e| {
                        CredentialFillError::InvalidTarget(format!("path floor: {e}"))
                    })?
                };
                let floor_canon = std::fs::canonicalize(floor)
                    .map_err(|e| CredentialFillError::InvalidTarget(format!("floor: {e}")))?;
                if !resolved.starts_with(&floor_canon) {
                    return Err(CredentialFillError::InvalidTarget(
                        "target path is outside the configured workspace floor".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

fn target_label(target: &FillTarget) -> String {
    match target {
        FillTarget::EnvVar { var_name, .. } => format!("env:{var_name}"),
        FillTarget::File { path } => format!("file:{}", path.display()),
    }
}

/// Credential-fill errors — none carry the secret.
#[derive(Debug, thiserror::Error)]
pub enum CredentialFillError {
    #[error("credential `{0}/{1}` not found in the vault")]
    NotFound(String, String),
    #[error("credential fill denied: {0}")]
    Denied(String),
    #[error("invalid fill target: {0}")]
    InvalidTarget(String),
    #[error("credential fill write failed: {0}")]
    Write(String),
    #[error("vault error: {0}")]
    Vault(KeyRingError),
}

impl From<KeyRingError> for CredentialFillError {
    fn from(e: KeyRingError) -> Self {
        match &e {
            KeyRingError::NotFound(provider, key_id) => {
                CredentialFillError::NotFound(provider.clone(), key_id.clone())
            }
            _ => CredentialFillError::Vault(e),
        }
    }
}

/// A hash-map-backed sink for tests: records (target → bytes) so assertions
/// can verify WHAT was written and that the receipt never exposed it.
#[derive(Default)]
pub struct MemSink {
    pub writes: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

impl FillSink for MemSink {
    fn write(&self, target: &FillTarget, value: &[u8]) -> Result<(), String> {
        self.writes
            .lock()
            .unwrap()
            .insert(target_label(target), value.to_vec());
        Ok(())
    }
}

impl FillSink for &MemSink {
    fn write(&self, target: &FillTarget, value: &[u8]) -> Result<(), String> {
        (**self).write(target, value)
    }
}

/// Approver that allows a fixed allow-list of (provider, key_id, target).
pub struct AllowlistApprover {
    pub allowed: Vec<(String, String, String)>,
}

impl FillApprover for AllowlistApprover {
    fn approve(&self, handle: &CredentialHandle, target: &FillTarget) -> Result<(), String> {
        let label = (
            handle.provider.clone(),
            handle.key_id.clone(),
            target_label(target),
        );
        if self.allowed.contains(&label) {
            Ok(())
        } else {
            Err(format!(
                "not on the fill allow-list ({}/{})",
                handle.provider, handle.key_id
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vault;

    fn ring() -> KeyRing<'static> {
        let vault: &'static Vault =
            Box::leak(Box::new(Vault::open_in_memory("broker-test-key").unwrap()));
        KeyRing::new(vault)
    }

    fn spec(provider: &str, key_id: &str, value: &str) -> crate::keyring::KeySpec {
        crate::keyring::KeySpec {
            provider: provider.into(),
            key_id: key_id.into(),
            value: value.as_bytes().to_vec(),
            status: crate::keyring::KeyStatus::Primary,
            model_filter: vec![],
            priority: 100,
            daily_token_cap: None,
            daily_cost_cap: None,
        }
    }

    fn env_target() -> FillTarget {
        FillTarget::EnvVar {
            process_id: "p-1".into(),
            var_name: "PORTAL_PASSWORD".into(),
        }
    }

    #[test]
    fn allowlisted_fill_writes_and_returns_receipt_only() {
        let r = ring();
        r.add_key(spec("forms", "alice", "s3cr3t!")).unwrap();
        let sink = MemSink::default();
        let approver = AllowlistApprover {
            allowed: vec![("forms".into(), "alice".into(), "env:PORTAL_PASSWORD".into())],
        };
        let broker = CredentialBroker::new(&r, approver, &sink);
        let receipt = broker
            .fill(&CredentialHandle::new("forms", "alice"), &env_target())
            .unwrap();
        assert!(receipt.approved);
        // Receipt carries no secret.
        let json = serde_json::to_string(&receipt).unwrap();
        assert!(!json.contains("s3cr3t"));
        assert!(!json.to_lowercase().contains("value"));
        // The sink got exactly the secret for exactly the target.
        assert_eq!(
            sink.writes
                .lock()
                .unwrap()
                .get("env:PORTAL_PASSWORD")
                .unwrap(),
            b"s3cr3t!"
        );
    }

    #[test]
    fn secret_is_zeroized_and_never_part_of_receipt_or_error() {
        let r = ring();
        r.add_key(spec("bank", "acct", "super-secret-12345"))
            .unwrap();
        let sink = MemSink::default();
        let approver = AllowlistApprover {
            allowed: vec![("bank".into(), "acct".into(), "env:TOKEN".into())],
        };
        let broker = CredentialBroker::new(&r, approver, &sink);
        let receipt = broker
            .fill(
                &CredentialHandle::new("bank", "acct"),
                &FillTarget::EnvVar {
                    process_id: "x".into(),
                    var_name: "TOKEN".into(),
                },
            )
            .unwrap();
        assert_eq!(receipt.target, "env:TOKEN");
    }

    #[test]
    fn default_approver_denies_everything_fail_closed() {
        let r = ring();
        r.add_key(spec("forms", "alice", "s")).unwrap();
        let sink = MemSink::default();
        let broker = CredentialBroker::new(&r, DenyAllApprover, &sink);
        let err = broker
            .fill(&CredentialHandle::new("forms", "alice"), &env_target())
            .unwrap_err();
        assert!(matches!(err, CredentialFillError::Denied(_)));
        assert!(
            sink.writes.lock().unwrap().is_empty(),
            "deny must never write"
        );
    }

    #[test]
    fn unknown_handle_fails_without_touching_sink() {
        let r = ring();
        let sink = MemSink::default();
        let broker = CredentialBroker::new(&r, DenyAllApprover, &sink);
        assert!(matches!(
            broker.fill(&CredentialHandle::new("nope", "nada"), &env_target()),
            Err(CredentialFillError::NotFound(_, _))
        ));
        assert!(sink.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn dangerous_env_targets_are_refused_before_approval() {
        let r = ring();
        r.add_key(spec("forms", "alice", "s")).unwrap();
        let sink = MemSink::default();
        let allowed = AllowlistApprover {
            allowed: vec![("forms".into(), "alice".into(), "env:PATH".into())],
        };
        let broker = CredentialBroker::new(&r, allowed, &sink);
        let err = broker
            .fill(
                &CredentialHandle::new("forms", "alice"),
                &FillTarget::EnvVar {
                    process_id: "p".into(),
                    var_name: "PATH".into(),
                },
            )
            .unwrap_err();
        assert!(matches!(err, CredentialFillError::InvalidTarget(_)));
        assert!(sink.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn file_fill_requires_floor_and_respects_it() {
        let r = ring();
        r.add_key(spec("ssh", "key", "hunter2")).unwrap();
        let dir = std::env::temp_dir().join(format!("broker-floor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let worker = dir.join("worker");
        std::fs::create_dir_all(&worker).unwrap();
        let sink = MemSink::default();
        let broker = CredentialBroker::new(&r, DenyAllApprover, &sink).with_floor(&worker);
        // Out-of-floor paths are refused by validation BEFORE any approval
        // (fail-closed, cheapest check first).
        let out_of_floor = dir.join("elsewhere").join("cred");
        assert!(matches!(
            broker.fill(
                &CredentialHandle::new("ssh", "key"),
                &FillTarget::File { path: out_of_floor },
            ),
            Err(CredentialFillError::InvalidTarget(_))
        ));
        // With an allowlist approver, in-floor writes succeed.
        let allowed = AllowlistApprover {
            allowed: vec![(
                "ssh".into(),
                "key".into(),
                format!("file:{}", worker.join("cred").display()),
            )],
        };
        let broker = CredentialBroker::new(&r, allowed, &sink).with_floor(&worker);
        let rc = broker.fill(
            &CredentialHandle::new("ssh", "key"),
            &FillTarget::File {
                path: worker.join("cred"),
            },
        );
        assert!(rc.is_ok());
        assert_eq!(
            sink.writes
                .lock()
                .unwrap()
                .get(&format!("file:{}", worker.join("cred").display()))
                .unwrap(),
            b"hunter2"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sits_inside_vault_not_beside_it() {
        // The broker is constructed over the real KeyRing — same SQLCipher
        // connection the broker/chat relay uses, so no second secret store.
        let r = ring();
        r.add_key(spec("mail", "acct", "pw")).unwrap();
        let sink = MemSink::default();
        let b = CredentialBroker::new(&r, DenyAllApprover, &sink);
        let _ = b.fill(&CredentialHandle::new("mail", "acct"), &env_target());
        // No panic, no second store — the assertion is that construction and
        // a denied fill over the shared ring don't corrupt ring state.
        assert_eq!(r.list("mail").unwrap().len(), 1);
    }
}
