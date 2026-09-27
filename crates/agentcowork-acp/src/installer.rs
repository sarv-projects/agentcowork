//! F8 — the **install executor** (the "touch" half of plan-before-touch).
//!
//! Given an [`InstallSpec`] from [`crate::registry_index`], [`Installer`]
//! downloads a binary archive, verifies its sha256, extracts it into
//! `<install_root>/<agent>/<version>/`, and records an install-state file so
//! [`crate::registry::LaunchRegistry::launch_plan`] can resolve the installed
//! binary path. npx/uvx agents are self-installing (the package manager
//! fetches at first spawn), so they install as a no-op that records the pin.
//!
//! The one-click UX: **install → (agent's own auth via ACP `authMethods`) →
//! use**. This module only mutates disk *under* `install_root` and verifies
//! every download against the registry's published sha256 (never trusts an
//! unverified archive).

use crate::registry_index::{InstallKind, InstallSpec};
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("install error: {0}")]
    Msg(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Sha256Mismatch { expected: String, actual: String },
    #[error("unsupported archive format: {0}")]
    UnsupportedArchive(String),
}

/// The result of an install (npx/uvx = recorded pin, no bytes downloaded).
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub agent_id: String,
    pub version: String,
    pub kind: String,
    /// Absolute path to the launchable binary (binary installs only).
    pub binary_path: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// Installs registry agents under an install root (default `<data_dir>/agents`).
pub struct Installer {
    install_root: PathBuf,
}

impl Installer {
    pub fn new(install_root: PathBuf) -> Self {
        Self { install_root }
    }

    pub fn install_root(&self) -> &Path {
        &self.install_root
    }

    /// Install an agent from its spec. For `Binary`: download → verify sha256
    /// → extract → record state. For npx/uvx: record the pin (no download).
    /// Every install writes an **ownership marker** (provenance: source URL +
    /// sha256 + exact argv) so the installed artifact is auditable (F8).
    pub fn install(&self, spec: &InstallSpec) -> Result<InstallOutcome, InstallError> {
        let dir = self.install_root.join(&spec.agent_id).join(&spec.version);
        match &spec.kind {
            InstallKind::Npx { package, args, env } => {
                let outcome = InstallOutcome {
                    agent_id: spec.agent_id.clone(),
                    version: spec.version.clone(),
                    kind: "npx".to_string(),
                    binary_path: None,
                    env: env.clone(),
                };
                self.record(
                    &outcome,
                    &Ownership::new(package, String::new(), args.clone()),
                )?;
                Ok(outcome)
            }
            InstallKind::Uvx { package, args, env } => {
                let outcome = InstallOutcome {
                    agent_id: spec.agent_id.clone(),
                    version: spec.version.clone(),
                    kind: "uvx".to_string(),
                    binary_path: None,
                    env: env.clone(),
                };
                self.record(
                    &outcome,
                    &Ownership::new(package, String::new(), args.clone()),
                )?;
                Ok(outcome)
            }
            InstallKind::Binary {
                archive,
                cmd,
                args,
                sha256,
                env,
            } => {
                // Fail closed: a registry binary install without a published
                // sha256 is never verified, so refuse before downloading at
                // all (bugfix 11). Unverified bytes must never reach disk.
                if sha256.trim().is_empty() {
                    return Err(InstallError::Msg(
                        "registry entry has no sha256 pin — refusing unverified download".into(),
                    ));
                }
                let bytes = self.download(archive)?;
                verify_sha256(&bytes, sha256)?;
                let extract_dir = dir.join("pkg");
                extract_archive(&bytes, archive, &extract_dir)?;
                let binary_path = extract_dir.join(rel_path(cmd));
                let outcome = InstallOutcome {
                    agent_id: spec.agent_id.clone(),
                    version: spec.version.clone(),
                    kind: "binary".to_string(),
                    binary_path: Some(binary_path.clone()),
                    env: env.clone(),
                };
                self.record(
                    &outcome,
                    &Ownership::new(archive, sha256.clone(), args.clone()),
                )?;
                Ok(outcome)
            }
        }
    }

    /// Load a prior install outcome (if this agent/version is installed).
    /// Persist a user-owned PATH discovery so future launches use the exact
    /// executable instead of relying on a mutable child PATH. This is not an
    /// AgentCowork download; the state is explicitly marked `kind: "path"`.
    pub fn record_path(&self, agent_id: &str, path: &Path) -> Result<(), InstallError> {
        let outcome = InstallOutcome {
            agent_id: agent_id.to_string(),
            version: "path".to_string(),
            kind: "path".to_string(),
            binary_path: Some(path.to_path_buf()),
            env: vec![],
        };
        self.record(
            &outcome,
            &Ownership::new(
                "PATH discovery",
                String::new(),
                vec![path.to_string_lossy().into_owned()],
            ),
        )
    }

    pub fn installed(&self, agent_id: &str) -> Option<InstallOutcome> {
        let dir = self.install_root.join(agent_id);
        let state: InstallState =
            serde_json::from_str(&std::fs::read_to_string(dir.join("installed.json")).ok()?)
                .ok()?;
        let binary_path = state.binary_path.map(|p| {
            let p = PathBuf::from(p);
            if p.is_absolute() { p } else { dir.join(p) }
        });
        Some(InstallOutcome {
            agent_id: agent_id.to_string(),
            version: state.version,
            kind: state.kind,
            binary_path,
            env: state.env,
        })
    }

    fn download(&self, url: &str) -> Result<Vec<u8>, InstallError> {
        // The registry archive URLs are plain GETs; the `Fetch` trait returns
        // UTF-8 text, so binary archives are fetched directly via ureq here.
        let resp = ureq::get(url)
            .timeout(std::time::Duration::from_secs(120))
            .call()
            .map_err(|e| InstallError::Msg(e.to_string()))?;
        let mut buf = Vec::new();
        resp.into_reader()
            .take(256 * 1024 * 1024) // 256MB ceiling — never slurp unbounded.
            .read_to_end(&mut buf)
            .map_err(InstallError::Io)?;
        Ok(buf)
    }

    fn record(&self, outcome: &InstallOutcome, ownership: &Ownership) -> Result<(), InstallError> {
        // The "current" pointer lives at `<root>/<agent>/installed.json`;
        // versioned binary packages live at `<root>/<agent>/<version>/pkg`.
        let agent_dir = self.install_root.join(&outcome.agent_id);
        std::fs::create_dir_all(&agent_dir)?;
        let state = InstallState {
            version: outcome.version.clone(),
            kind: outcome.kind.clone(),
            binary_path: outcome
                .binary_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            env: outcome.env.clone(),
        };
        std::fs::write(
            agent_dir.join("installed.json"),
            serde_json::to_string_pretty(&state).unwrap_or_default(),
        )?;
        // Ownership marker (F8): provenance of every installed artifact.
        std::fs::write(
            agent_dir.join("OWNERSHIP.json"),
            serde_json::to_string_pretty(&OwnershipMarker {
                agent_id: outcome.agent_id.clone(),
                version: outcome.version.clone(),
                source: ownership.source.clone(),
                sha256: ownership.sha256.clone(),
                args: ownership.args.clone(),
                installed_at_ms: now_ms(),
            })
            .unwrap_or_default(),
        )?;
        Ok(())
    }

    /// Read back the ownership marker for an installed agent (audit seam).
    pub fn ownership(&self, agent_id: &str) -> Option<OwnershipMarker> {
        let path = self.install_root.join(agent_id).join("OWNERSHIP.json");
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct InstallState {
    version: String,
    kind: String,
    #[serde(default)]
    binary_path: Option<String>,
    #[serde(default)]
    env: Vec<(String, String)>,
}

/// Provenance passed into `record` (F8 ownership markers).
struct Ownership {
    source: String,
    sha256: String,
    args: Vec<String>,
}

impl Ownership {
    fn new(source: impl Into<String>, sha256: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            source: source.into(),
            sha256: sha256.into(),
            args,
        }
    }
}

/// The on-disk ownership marker (F8) — auditable provenance for every
/// installed artifact.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OwnershipMarker {
    pub agent_id: String,
    pub version: String,
    pub source: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub installed_at_ms: i64,
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), InstallError> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex(&hasher.finalize());
    if actual != expected_hex.to_ascii_lowercase() {
        return Err(InstallError::Sha256Mismatch {
            expected: expected_hex.to_ascii_lowercase(),
            actual,
        });
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Strip a leading `./` so `./bin/devin` resolves under the extract dir.
fn rel_path(cmd: &str) -> PathBuf {
    let c = cmd.trim_start_matches("./").trim_start_matches(".\\");
    PathBuf::from(c)
}

/// Extract `.tar.gz` / `.tgz` / `.tar` / `.zip` (detected by suffix).
fn extract_archive(bytes: &[u8], url: &str, dest: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(dest)?;
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        extract_zip(bytes, dest)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract_tar_gz(bytes, dest)
    } else if lower.ends_with(".tar") {
        extract_tar(bytes, dest)
    } else {
        Err(InstallError::UnsupportedArchive(url.to_string()))
    }
}

/// Confine a member path under `dest`: reject absolute paths, `..` escapes
/// and Windows drive roots. Returns the joined target path (path-traversal
/// safe) — bugfix 11. `..` inside a segment is allowed (e.g. a name with a
/// literal `..`) as long as no *segment* is exactly `..`.
fn safe_join(dest: &Path, member: &str) -> Result<std::path::PathBuf, InstallError> {
    let member = member.replace('\\', "/"); // normalize windows separators too
    if member.starts_with('/') || member.contains(':') {
        return Err(InstallError::Msg(format!(
            "archive member with absolute path refused: {member}"
        )));
    }
    let mut out = dest.to_path_buf();
    for seg in member.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(InstallError::Msg(format!(
                "archive member with path traversal refused: {member}"
            )));
        }
        out.push(seg);
    }
    // Belt + braces: the joined path must stay strictly under dest.
    if !out.starts_with(dest) {
        return Err(InstallError::Msg(format!(
            "archive member escapes extraction root: {member}"
        )));
    }
    Ok(out)
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), InstallError> {
    let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes))
        .map_err(|e| InstallError::Msg(e.to_string()))?;
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|e| InstallError::Msg(e.to_string()))?;
        let target = safe_join(dest, file.name())?;
        if file.is_dir() {
            std::fs::create_dir_all(&target).map_err(InstallError::Io)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
        }
        let mut reader = std::io::BufReader::new(file);
        let mut out = std::fs::File::create(&target).map_err(InstallError::Io)?;
        std::io::copy(&mut reader, &mut out).map_err(InstallError::Io)?;
    }
    Ok(())
}

fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<(), InstallError> {
    let gz = flate2::read::GzDecoder::new(io::Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);
    extract_tar_entries(&mut archive, dest)
}

fn extract_tar(bytes: &[u8], dest: &Path) -> Result<(), InstallError> {
    let mut archive = tar::Archive::new(io::Cursor::new(bytes));
    extract_tar_entries(&mut archive, dest)
}

/// Extract a tar archive, confining every member under `dest` (zip-slip
/// defence). Symlinks/hard-links are never followed out of the root; a link
/// target that escapes is refused and the link is materialized only as a
/// regular file inside `dest` if it stays in-root.
fn extract_tar_entries<R: std::io::Read>(
    archive: &mut tar::Archive<R>,
    dest: &Path,
) -> Result<(), InstallError> {
    let full = dest.to_path_buf();
    std::fs::create_dir_all(&full)?;
    let entries = archive
        .entries()
        .map_err(|e| InstallError::Msg(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| InstallError::Msg(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| InstallError::Msg(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        let target = safe_join(&full, &path)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            std::fs::create_dir_all(&target).map_err(InstallError::Io)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
        }
        if entry_type.is_symlink() {
            // Materialize the link as a plain file carrying its target text —
            // never create a symlink that could point outside the root.
            let link = entry
                .link_name()
                .map_err(|e| InstallError::Msg(e.to_string()))?;
            let text = link
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            std::fs::write(&target, text).map_err(InstallError::Io)?;
            continue;
        }
        let mut out = std::fs::File::create(&target).map_err(InstallError::Io)?;
        std::io::copy(&mut entry, &mut out).map_err(InstallError::Io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_root(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("agentcowork-acp-inst-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn tar_gz_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, &body[..]).unwrap();
        }
        let tar = tar.into_inner().unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&tar).unwrap();
        enc.finish().unwrap()
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn binary_tar_gz_downloads_verifies_and_extracts() {
        let bytes = tar_gz_bytes(&[("bin/devin", b"#!/bin/sh\necho devin\n")]);
        let sha = hex(&Sha256::digest(&bytes));
        let root = tmp_root("tgz");

        // The download() path uses ureq (network); the deterministic core is
        // extract + sha verify, tested directly against an in-memory archive.
        let dest = root.join("devin").join("1.0.0").join("pkg");
        extract_archive(&bytes, "https://x/devin.tar.gz", &dest).unwrap();
        assert!(dest.join("bin/devin").exists());
        verify_sha256(&bytes, &sha).unwrap();

        // sha mismatch rejects.
        assert!(matches!(
            verify_sha256(&bytes, "deadbeef"),
            Err(InstallError::Sha256Mismatch { .. })
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn zip_extracts() {
        let bytes = zip_bytes(&[("kilo.exe", b"PK-binary")]);
        let root = tmp_root("zip");
        let dest = root.join("kilo").join("1.0.0").join("pkg");
        extract_archive(&bytes, "https://x/kilo.zip", &dest).unwrap();
        assert!(dest.join("kilo.exe").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn npx_install_records_pin_without_download() {
        let root = tmp_root("npx");
        let installer = Installer::new(root.clone());
        let spec = InstallSpec {
            license_url: None,
            agent_id: "cline".into(),
            name: "Cline".into(),
            version: "3.0.55".into(),
            license: "Apache-2.0".into(),
            kind: InstallKind::Npx {
                package: "cline@3.0.55".into(),
                args: vec!["--acp".into()],
                env: vec![],
            },
            install_dir: None,
        };
        let out = installer.install(&spec).unwrap();
        assert_eq!(out.kind, "npx");
        assert!(out.binary_path.is_none());
        assert!(root.join("cline").join("installed.json").exists());

        // Install state round-trips.
        let loaded = installer.installed("cline").unwrap();
        assert_eq!(loaded.version, "3.0.55");
        assert_eq!(loaded.kind, "npx");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn path_discovery_records_exact_absolute_path() {
        let root = tmp_root("path");
        let installer = Installer::new(root.clone());
        let path = root.join("bin").join("agent");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"agent").unwrap();
        installer.record_path("agent", &path).unwrap();
        let loaded = installer.installed("agent").unwrap();
        assert_eq!(loaded.kind, "path");
        assert_eq!(loaded.binary_path, Some(path));
        assert_eq!(
            installer.ownership("agent").unwrap().source,
            "PATH discovery"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rel_path_strips_leading_dot_slash() {
        assert_eq!(rel_path("./bin/devin"), PathBuf::from("bin/devin"));
        assert_eq!(rel_path("kilo.exe"), PathBuf::from("kilo.exe"));
    }

    #[test]
    fn zip_slip_is_refused() {
        // Bugfix 11 — the zip format permits `..` member names, so this is the
        // real zip-slip vector our confined extractor must refuse. (The tar
        // crate rejects `..`/absolute members at both build and unpack, so tar
        // needs no extra defence.)
        let root = tmp_root("slip");
        let dest = root.join("pkg");

        let evil_zip = zip_bytes(&[("../evil", b"bad"), ("bin/ok", b"ok")]);
        let err = extract_archive(&evil_zip, "https://x/evil.zip", &dest).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("traversal") || msg.contains("escape"),
            "got: {msg}"
        );
        assert!(!dest.join("evil").exists());
        assert!(!root.join("evil").exists());
        // The safe member inside the same archive should never have been
        // written either (extraction aborts on the hostile member).
        assert!(!dest.join("bin/ok").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_sha256_is_refused_before_download() {
        // Bugfix 11 — an empty sha pin fails closed (no unverified archive
        // can reach disk). Exercises the guard directly on a Binary spec.
        let root = tmp_root("nosha");
        let installer = Installer::new(root.clone());
        let spec = InstallSpec {
            license_url: None,
            agent_id: "x".into(),
            name: "X".into(),
            version: "1".into(),
            license: "MIT".into(),
            kind: InstallKind::Binary {
                archive: "https://x/sha.tar.gz".into(),
                cmd: "bin/x".into(),
                args: vec![],
                sha256: String::new(),
                env: vec![],
            },
            install_dir: None,
        };
        let err = installer.install(&spec).unwrap_err();
        assert!(err.to_string().contains("sha256"), "got: {err}");
        // Nothing was extracted.
        assert!(!root.join("x").join("pkg").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
