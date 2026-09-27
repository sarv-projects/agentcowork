//! H4/R7 — vault key derivation: env · keyfile · passphrase+Argon2id.
//! Never falls back to a hardcoded SQLCipher passphrase.

use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

use agentcowork_types::env_compat;

const KEYFILE_NAME: &str = "vault.key";
const KDF_M_KIB: u32 = 19_456;
const KDF_T: u32 = 2;
const KDF_P: u32 = 1;
const KEY_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultKeyOrigin {
    Env,
    Keyfile,
    Passphrase,
    Generated,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedVaultKey {
    pub key: String,
    pub origin: VaultKeyOrigin,
    pub path: Option<String>,
}

#[derive(Debug, Error)]
pub enum VaultKeyError {
    #[error(
        "vault locked: set AGENTCOWORK_VAULT_KEY, AGENTCOWORK_VAULT_PASSPHRASE, or call vault_setup"
    )]
    NeedsSetup,
    #[error("vault keyfile unreadable: {0}")]
    Io(#[from] std::io::Error),
    #[error("vault key derivation failed: {0}")]
    Kdf(String),
    #[error("passphrase does not match the stored verifier")]
    BadPassphrase,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyFile {
    kdf: String,
    salt_hex: String,
    m_kib: u32,
    t_cost: u32,
    p_cost: u32,
    /// SHA-256 of the SQLCipher key (hex) — never the key itself.
    verifier: String,
    /// Raw SQLCipher key hex — Generated origin only (CI / `ALLOW_GENERATED_KEY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_hex: Option<String>,
    /// Generated key XOR-wrapped with Argon2(passphrase) so an existing
    /// vault.db keeps opening after the user sets a passphrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrapped_key_hex: Option<String>,
}

pub fn keyfile_path(data_dir: &Path) -> PathBuf {
    if let Some(p) = env_compat::get("VAULT_KEYFILE") {
        return PathBuf::from(p);
    }
    data_dir.join(KEYFILE_NAME)
}

/// Resolve the SQLCipher key. Order: env → passphrase+keyfile → raw keyfile.
/// First boot without a passphrase does **not** mint a silent key (1Password /
/// Bitwarden / Signal rule) unless `AGENTCOWORK_ALLOW_GENERATED_KEY=1` (CI).
/// Every `AGENTCOWORK_*` name below honors its retired `EVERYAIOS_*` spelling
/// as a read-only fallback (DEC-053).
pub fn resolve_vault_key(data_dir: &Path) -> Result<ResolvedVaultKey, VaultKeyError> {
    if let Some(k) = env_compat::get("VAULT_KEY")
        && !k.is_empty()
    {
        return Ok(ResolvedVaultKey {
            key: k,
            origin: VaultKeyOrigin::Env,
            path: None,
        });
    }
    let path = keyfile_path(data_dir);
    if path.is_file() {
        return open_keyfile(&path);
    }
    if env_compat::get("ALLOW_GENERATED_KEY").as_deref() == Some("1") {
        return generate_keyfile(data_dir);
    }
    Err(VaultKeyError::NeedsSetup)
}

/// True when the UI must block until the user sets or unlocks a passphrase.
pub fn needs_passphrase_gate(data_dir: &Path) -> bool {
    if env_compat::get("VAULT_KEY")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    match resolve_vault_key(data_dir) {
        Ok(r) => r.origin == VaultKeyOrigin::Generated,
        Err(VaultKeyError::NeedsSetup) => true,
        Err(_) => true,
    }
}

/// `"setup"` (no keyfile) · `"unlock"` (passphrase keyfile) · `"wrap"` (legacy generated).
pub fn gate_mode(data_dir: &Path) -> &'static str {
    if env_compat::get("VAULT_KEY")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        return "open";
    }
    let path = keyfile_path(data_dir);
    if !path.is_file() {
        return "setup";
    }
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    if let Ok(rec) = serde_json::from_str::<KeyFile>(raw.trim()) {
        if rec.key_hex.is_some() {
            return "wrap";
        }
        if env_compat::is_set("VAULT_PASSPHRASE") {
            return "open";
        }
        return "unlock";
    }
    "setup"
}

/// First-boot / UI gate: wrap a user passphrase with Argon2id and persist
/// salt+verifier (the derived key is the SQLCipher secret).
pub fn setup_vault_passphrase(
    data_dir: &Path,
    passphrase: &str,
) -> Result<ResolvedVaultKey, VaultKeyError> {
    if passphrase.len() < 8 {
        return Err(VaultKeyError::Kdf(
            "passphrase must be at least 8 characters".into(),
        ));
    }
    let _ = std::fs::create_dir_all(data_dir);
    let path = keyfile_path(data_dir);
    // Wrapping a previously generated keyfile keeps the SQLCipher key so an
    // already-created vault.db still opens.
    let existing = if path.is_file() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<KeyFile>(raw.trim()).ok())
            .and_then(|r| r.key_hex)
    } else {
        None
    };
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let wrapping = derive(passphrase, &salt)?;
    let (key, wrapped_key_hex) = if let Some(k) = existing {
        (
            k.clone(),
            Some(xor_hex(&k, &wrapping).map_err(VaultKeyError::Kdf)?),
        )
    } else {
        (wrapping.clone(), None)
    };
    let rec = KeyFile {
        kdf: "argon2id".into(),
        salt_hex: hex(&salt),
        m_kib: KDF_M_KIB,
        t_cost: KDF_T,
        p_cost: KDF_P,
        verifier: sha256_hex(key.as_bytes()),
        key_hex: None,
        wrapped_key_hex,
    };
    write_keyfile(&path, &rec)?;
    Ok(ResolvedVaultKey {
        key,
        origin: VaultKeyOrigin::Passphrase,
        path: Some(path.display().to_string()),
    })
}

// Thread-local carry for the unlock passphrase — never copied into the
// process environment (bugfix 16), so it can't leak into `/proc/<pid>/environ`
// or be inherited by child processes. Cleared immediately after the KDF.
thread_local! {
    static UNLOCK_PASSPHRASE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn with_unlock_passphrase<T>(passphrase: Option<String>, f: impl FnOnce() -> T) -> T {
    UNLOCK_PASSPHRASE.with(|cell| *cell.borrow_mut() = passphrase);
    let out = f();
    UNLOCK_PASSPHRASE.with(|cell| *cell.borrow_mut() = None);
    out
}

/// Unlock an existing passphrase keyfile (subsequent launches). The passphrase
/// is carried thread-locally for the KDF and cleared afterwards — it is never
/// placed in the process environment.
pub fn unlock_vault_passphrase(
    data_dir: &Path,
    passphrase: &str,
) -> Result<ResolvedVaultKey, VaultKeyError> {
    with_unlock_passphrase(Some(passphrase.to_string()), || resolve_vault_key(data_dir))
}

fn generate_keyfile(data_dir: &Path) -> Result<ResolvedVaultKey, VaultKeyError> {
    let _ = std::fs::create_dir_all(data_dir);
    let mut raw = [0u8; KEY_LEN];
    rand::thread_rng().fill_bytes(&mut raw);
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = hex(&raw);
    let rec = KeyFile {
        kdf: "argon2id".into(),
        salt_hex: hex(&salt),
        m_kib: KDF_M_KIB,
        t_cost: KDF_T,
        p_cost: KDF_P,
        verifier: sha256_hex(key.as_bytes()),
        key_hex: Some(key.clone()),
        wrapped_key_hex: None,
    };
    let path = keyfile_path(data_dir);
    write_keyfile(&path, &rec)?;
    Ok(ResolvedVaultKey {
        key,
        origin: VaultKeyOrigin::Generated,
        path: Some(path.display().to_string()),
    })
}

fn open_keyfile(path: &Path) -> Result<ResolvedVaultKey, VaultKeyError> {
    let raw = std::fs::read_to_string(path)?;
    let rec: KeyFile = serde_json::from_str(raw.trim())
        .map_err(|e| VaultKeyError::Kdf(format!("malformed keyfile: {e}")))?;
    if let Some(k) = rec.key_hex.clone() {
        if sha256_hex(k.as_bytes()) != rec.verifier {
            return Err(VaultKeyError::BadPassphrase);
        }
        return Ok(ResolvedVaultKey {
            key: k,
            origin: VaultKeyOrigin::Generated,
            path: Some(path.display().to_string()),
        });
    }
    // Prefer the thread-local unlock passphrase (bugfix 16 — never leaked into
    // the environment); env fallback remains for env-driven/test flows.
    let pass = UNLOCK_PASSPHRASE
        .with(|c| c.borrow().clone())
        .or_else(|| env_compat::get("VAULT_PASSPHRASE"))
        .ok_or(VaultKeyError::NeedsSetup)?;
    let salt = unhex(&rec.salt_hex).map_err(VaultKeyError::Kdf)?;
    let wrapping = derive(&pass, &salt)?;
    let key = if let Some(w) = rec.wrapped_key_hex.as_deref() {
        xor_hex(w, &wrapping).map_err(VaultKeyError::Kdf)?
    } else {
        wrapping
    };
    if sha256_hex(key.as_bytes()) != rec.verifier {
        return Err(VaultKeyError::BadPassphrase);
    }
    Ok(ResolvedVaultKey {
        key,
        origin: VaultKeyOrigin::Passphrase,
        path: Some(path.display().to_string()),
    })
}

fn xor_hex(a_hex: &str, b_hex: &str) -> Result<String, String> {
    let a = unhex(a_hex)?;
    let b = unhex(b_hex)?;
    if a.len() != b.len() {
        return Err("wrap length mismatch".into());
    }
    Ok(hex(&a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| x ^ y)
        .collect::<Vec<_>>()))
}

fn derive(passphrase: &str, salt: &[u8]) -> Result<String, VaultKeyError> {
    let params = Params::new(KDF_M_KIB, KDF_T, KDF_P, Some(KEY_LEN))
        .map_err(|e| VaultKeyError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| VaultKeyError::Kdf(e.to_string()))?;
    Ok(hex(&out))
}

fn write_keyfile(path: &Path, rec: &KeyFile) -> Result<(), VaultKeyError> {
    let json = serde_json::to_string_pretty(rec).map_err(|e| VaultKeyError::Kdf(e.to_string()))?;
    std::fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV: Mutex<()> = Mutex::new(());

    fn isolate() -> std::sync::MutexGuard<'static, ()> {
        let g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `ENV` is held for the whole test, serializing the environment
        // mutations performed by this test module. Both spellings are cleared
        // so a legacy value cannot leak into a new-spelling test or vice versa.
        unsafe {
            for rest in [
                "VAULT_KEY",
                "VAULT_PASSPHRASE",
                "VAULT_KEYFILE",
                "ALLOW_GENERATED_KEY",
            ] {
                std::env::remove_var(env_compat::new_name(rest));
                std::env::remove_var(env_compat::legacy_name(rest));
            }
        }
        g
    }

    #[test]
    fn env_wins_and_never_uses_placeholder() {
        let _g = isolate();
        // SAFETY: `isolate` holds `ENV` for this test, so this environment
        // mutation is serialized with the other vault-key tests.
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_KEY", "from-env");
        }
        let r = resolve_vault_key(Path::new("/tmp")).unwrap();
        assert_eq!(r.origin, VaultKeyOrigin::Env);
        assert_eq!(r.key, "from-env");
        assert_ne!(r.key, "agentcowork-core-dev-key-do-not-use");
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_VAULT_KEY");
        }
    }

    #[test]
    fn legacy_env_spelling_is_honored_with_new_winning() {
        // DEC-053: the retired `EVERYAIOS_*` spelling stays a read-only
        // fallback; the new spelling wins when both are set.
        let _g = isolate();
        unsafe {
            std::env::set_var("EVERYAIOS_VAULT_KEY", "legacy-env");
        }
        let r = resolve_vault_key(Path::new("/tmp")).unwrap();
        assert_eq!(r.origin, VaultKeyOrigin::Env);
        assert_eq!(r.key, "legacy-env");
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_KEY", "new-env");
        }
        let r = resolve_vault_key(Path::new("/tmp")).unwrap();
        assert_eq!(r.key, "new-env");
        unsafe {
            std::env::remove_var("EVERYAIOS_VAULT_KEY");
            std::env::remove_var("AGENTCOWORK_VAULT_KEY");
        }
    }

    #[test]
    fn passphrase_roundtrip() {
        let _g = isolate();
        let dir = std::env::temp_dir().join(format!("vault-kdf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = setup_vault_passphrase(&dir, "correct-horse").unwrap();
        assert_eq!(first.origin, VaultKeyOrigin::Passphrase);
        // SAFETY: `isolate` holds `ENV` for this test, so these environment
        // mutations are serialized with the other vault-key tests.
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_PASSPHRASE", "correct-horse");
            std::env::set_var("AGENTCOWORK_VAULT_KEYFILE", dir.join(KEYFILE_NAME));
        }
        let again = resolve_vault_key(&dir).unwrap();
        assert_eq!(again.key, first.key);
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_PASSPHRASE", "wrong-pass-xx");
        }
        assert!(matches!(
            resolve_vault_key(&dir),
            Err(VaultKeyError::BadPassphrase)
        ));
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_VAULT_PASSPHRASE");
            std::env::remove_var("AGENTCOWORK_VAULT_KEYFILE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrap_generated_then_unlock_keeps_sqlcipher_key() {
        let _g = isolate();
        // SAFETY: `isolate` holds `ENV` for this test, so these environment
        // mutations are serialized with the other vault-key tests.
        unsafe {
            std::env::set_var("AGENTCOWORK_ALLOW_GENERATED_KEY", "1");
        }
        let dir = std::env::temp_dir().join(format!("vault-wrap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_KEYFILE", dir.join(KEYFILE_NAME));
        }
        let generated = resolve_vault_key(&dir).unwrap();
        assert_eq!(generated.origin, VaultKeyOrigin::Generated);
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_ALLOW_GENERATED_KEY");
        }
        let wrapped = setup_vault_passphrase(&dir, "correct-horse").unwrap();
        assert_eq!(wrapped.key, generated.key);
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_VAULT_PASSPHRASE");
        }
        let unlocked = unlock_vault_passphrase(&dir, "correct-horse").unwrap();
        assert_eq!(unlocked.key, generated.key);
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_VAULT_PASSPHRASE");
            std::env::remove_var("AGENTCOWORK_VAULT_KEYFILE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_boot_without_passphrase_is_needs_setup() {
        let _g = isolate();
        let dir = std::env::temp_dir().join(format!("vault-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: `isolate` holds `ENV` for this test, so this environment
        // mutation is serialized with the other vault-key tests.
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_KEYFILE", dir.join(KEYFILE_NAME));
        }
        assert!(matches!(
            resolve_vault_key(&dir),
            Err(VaultKeyError::NeedsSetup)
        ));
        assert!(needs_passphrase_gate(&dir));
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_VAULT_KEYFILE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_key_only_when_explicitly_allowed() {
        let _g = isolate();
        // SAFETY: `isolate` holds `ENV` for this test, so this environment
        // mutation is serialized with the other vault-key tests.
        unsafe {
            std::env::set_var("AGENTCOWORK_ALLOW_GENERATED_KEY", "1");
        }
        let dir = std::env::temp_dir().join(format!("vault-gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::set_var("AGENTCOWORK_VAULT_KEYFILE", dir.join(KEYFILE_NAME));
        }
        let r = resolve_vault_key(&dir).unwrap();
        assert_eq!(r.origin, VaultKeyOrigin::Generated);
        assert_ne!(r.key, "agentcowork-core-dev-key-do-not-use");
        // SAFETY: `isolate` still holds `ENV` for this test.
        unsafe {
            std::env::remove_var("AGENTCOWORK_ALLOW_GENERATED_KEY");
            std::env::remove_var("AGENTCOWORK_VAULT_KEYFILE");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
