//! DEC-053 — one `AGENTCOWORK_*` / legacy `EVERYAIOS_*` compatibility rule.
//!
//! Every environment lookup and every data-home / config-file resolution in
//! the workspace funnels through this module, so the precedence rule exists
//! exactly once:
//!
//! - **`AGENTCOWORK_<REST>` first, then the legacy `EVERYAIOS_<REST>` as a
//!   read-only fallback.** Writes and newly-spawned children use the new name
//!   only; a honored legacy value is reported through [`provenance`] /
//!   [`used_legacy`] so the caller can surface it to the operator.
//! - Data home: `AGENTCOWORK_HOME` → `~/.agentcowork` if it exists → legacy
//!   `EVERYAIOS_HOME` → `~/.everyaios` if it exists → else `~/.agentcowork`
//!   (the new default).
//! - Config file: `agentcowork.toml` if it exists, else the legacy
//!   `everyaios.toml` if it exists, else `agentcowork.toml` (fresh default).
//!
//! Pure env/path computation: the only filesystem contact is an existence
//! check, so this module keeps this crate's no-IO character. In particular
//! **nothing here migrates anything** — the one-time data-home migration is
//! owned solely by Core at startup (DEC-053: no lower-level crate may move
//! files, so there is no migration race and no second migrator). A
//! lower-level crate that needs the resolved home calls [`data_home`] and
//! gets the path; it never moves files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The current environment-variable prefix (DEC-053).
pub const NEW_PREFIX: &str = "AGENTCOWORK_";
/// The retired prefix, honored read-only as a fallback (DEC-053).
pub const LEGACY_PREFIX: &str = "EVERYAIOS_";

/// The current data-home override variable.
pub const NEW_HOME_VAR: &str = "AGENTCOWORK_HOME";
/// The retired data-home override variable (read-only fallback).
pub const LEGACY_HOME_VAR: &str = "EVERYAIOS_HOME";

/// The current data-home directory name under the user's profile.
pub const NEW_DIR_NAME: &str = ".agentcowork";
/// The retired data-home directory name (accepted, migrated once by Core).
pub const LEGACY_DIR_NAME: &str = ".everyaios";

/// The current on-disk config filename.
pub const CONFIG_FILENAME: &str = "agentcowork.toml";
/// The retired on-disk config filename (read-fallback, migrated with the
/// data home by Core).
pub const LEGACY_CONFIG_FILENAME: &str = "everyaios.toml";

/// The new spelling of a variable: `AGENTCOWORK_<REST>`.
pub fn new_name(rest: &str) -> String {
    format!("{NEW_PREFIX}{rest}")
}

/// The retired spelling of a variable: `EVERYAIOS_<REST>` (read-only
/// fallback; never written by current code).
pub fn legacy_name(rest: &str) -> String {
    format!("{LEGACY_PREFIX}{rest}")
}

/// Where a variable's value came from: the new name, the legacy fallback, or
/// nowhere. A [`Provenance::Legacy`] answer is the caller's cue to surface
/// the honored legacy value to the operator (Core folds these into its boot
/// report; sees [`used_legacy`] and [`legacy_fallback_note`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Set under the current `AGENTCOWORK_<REST>` name.
    New,
    /// Set only under the retired `EVERYAIOS_<REST>` name (read-only fallback).
    Legacy,
    /// Set under neither name (or set-but-empty under both).
    Unset,
}

/// Which spelling of `AGENTCOWORK_<REST>` / `EVERYAIOS_<REST>` is in force.
/// An empty value counts as unset and falls through to the next source — an
/// explicitly-emptied variable must not shadow a real legacy value (nor a
/// real new value shadowed by nothing).
pub fn provenance(rest: &str) -> Provenance {
    if std::env::var_os(new_name(rest)).is_some_and(|v| !v.is_empty()) {
        return Provenance::New;
    }
    if std::env::var_os(legacy_name(rest)).is_some_and(|v| !v.is_empty()) {
        return Provenance::Legacy;
    }
    Provenance::Unset
}

/// Read `AGENTCOWORK_<REST>`, falling back to the legacy `EVERYAIOS_<REST>`.
/// Empty counts as unset at both levels. Returns `None` when neither is set.
pub fn get(rest: &str) -> Option<String> {
    match provenance(rest) {
        Provenance::New => std::env::var(new_name(rest)).ok(),
        Provenance::Legacy => std::env::var(legacy_name(rest)).ok(),
        Provenance::Unset => None,
    }
}

/// [`get`] for values that are not valid Unicode (e.g. `*_HOME` overrides).
pub fn get_os(rest: &str) -> Option<OsString> {
    match provenance(rest) {
        Provenance::New => std::env::var_os(new_name(rest)),
        Provenance::Legacy => std::env::var_os(legacy_name(rest)),
        Provenance::Unset => None,
    }
}

/// Whether `AGENTCOWORK_<REST>` or (as a fallback) `EVERYAIOS_<REST>` is set
/// to a non-empty value. For live-test / feature gates.
pub fn is_set(rest: &str) -> bool {
    provenance(rest) != Provenance::Unset
}

/// True when the value in force for `rest` comes from the retired spelling.
/// The caller logs this once per variable so a honored legacy value is
/// visible to the operator rather than silent.
pub fn used_legacy(rest: &str) -> bool {
    provenance(rest) == Provenance::Legacy
}

/// The operator-visible note for a honored legacy value, or `None` when the
/// new spelling (or nothing) is in force.
pub fn legacy_fallback_note(rest: &str) -> Option<String> {
    if used_legacy(rest) {
        Some(format!(
            "{} is unset; honoring legacy {} (set {} to silence this)",
            new_name(rest),
            legacy_name(rest),
            new_name(rest),
        ))
    } else {
        None
    }
}

/// True for environment-variable names that are host-control material: ours
/// under either the current or the retired prefix. Child-environment filters
/// (ACP transport, sandbox spawn, desktop launch scrub) block both spellings
/// — a renamed credential must not pass a filter that only knew the old name.
pub fn is_host_control_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with(NEW_PREFIX) || upper.starts_with(LEGACY_PREFIX)
}

/// The user's profile base: `HOME`, then Windows `USERPROFILE`, then
/// `HOMEDRIVE`+`HOMEPATH` (Windows has no `HOME`); last resort is the OS temp
/// dir — never the cwd, or vault/config would land in `./.agentcowork` under
/// whatever directory the process happened to start in.
pub fn home_base() -> PathBuf {
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .or_else(|_| {
            let d = std::env::var("HOMEDRIVE").unwrap_or_default();
            let p = std::env::var("HOMEPATH").unwrap_or_default();
            if d.is_empty() || p.is_empty() {
                Err(std::env::VarError::NotPresent)
            } else {
                Ok(format!("{d}{p}"))
            }
        })
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    PathBuf::from(base)
}

/// Where the resolved data home came from (DEC-053 precedence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataHomeProvenance {
    /// `AGENTCOWORK_HOME` is set (non-empty): explicit operator choice, wins.
    NewEnv,
    /// `~/.agentcowork` exists: the migrated (or already-new) install.
    NewDirExists,
    /// `EVERYAIOS_HOME` is set (non-empty): legacy override, honored until
    /// the operator moves to `AGENTCOWORK_HOME`. Core migrates the directory
    /// this points at when the new path does not exist yet.
    LegacyEnv,
    /// `~/.everyaios` exists: a pre-rename install. Core migrates it once.
    LegacyDirExists,
    /// Fresh machine: neither spelling nor directory exists; the new default.
    NewDefault,
}

/// The resolved data home plus where it came from.
///
/// Precedence: `AGENTCOWORK_HOME` → `~/.agentcowork` if it exists → legacy
/// `EVERYAIOS_HOME` → `~/.everyaios` if it exists → else `~/.agentcowork`
/// (the new default). The only filesystem contact is the two existence
/// checks; nothing is created or moved here.
pub fn data_home_with_provenance() -> (PathBuf, DataHomeProvenance) {
    // NOTE: the order below is the precedence — an existing `~/.agentcowork`
    // beats an explicit legacy override, so a migrated install is never
    // pulled back onto the legacy path by a stale `EVERYAIOS_HOME`.
    if std::env::var_os(NEW_HOME_VAR).is_some_and(|v| !v.is_empty()) {
        return (
            PathBuf::from(std::env::var_os(NEW_HOME_VAR).expect("non-empty above")),
            DataHomeProvenance::NewEnv,
        );
    }
    let base = home_base();
    if base.join(NEW_DIR_NAME).exists() {
        return (base.join(NEW_DIR_NAME), DataHomeProvenance::NewDirExists);
    }
    if let Some(home) = std::env::var_os(LEGACY_HOME_VAR)
        && !home.is_empty()
    {
        return (PathBuf::from(home), DataHomeProvenance::LegacyEnv);
    }
    if base.join(LEGACY_DIR_NAME).exists() {
        return (
            base.join(LEGACY_DIR_NAME),
            DataHomeProvenance::LegacyDirExists,
        );
    }
    (base.join(NEW_DIR_NAME), DataHomeProvenance::NewDefault)
}

/// The resolved data home (DEC-053 precedence). See
/// [`data_home_with_provenance`] for the provenance breakdown.
pub fn data_home() -> PathBuf {
    data_home_with_provenance().0
}

/// The config file inside `data_dir`: `agentcowork.toml` when present, else
/// the legacy `everyaios.toml` when present (an existing install keeps its
/// config), else `agentcowork.toml` as the fresh default. Pure path
/// computation; the caller decides whether to create what is missing.
pub fn config_path_for(data_dir: &Path) -> PathBuf {
    let current = data_dir.join(CONFIG_FILENAME);
    if current.exists() {
        return current;
    }
    let legacy = data_dir.join(LEGACY_CONFIG_FILENAME);
    if legacy.exists() {
        return legacy;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV: Mutex<()> = Mutex::new(());

    /// Serialize the env-mutating tests in this module (each uses a unique
    /// `REST`, but `HOME`-family tests share process-global state).
    fn isolate() -> std::sync::MutexGuard<'static, ()> {
        ENV.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Remove both spellings of `rest`; caller must hold [`isolate`].
    /// SAFETY: the `ENV` lock serializes every mutation in this module.
    unsafe fn clear(rest: &str) {
        unsafe {
            std::env::remove_var(new_name(rest));
            std::env::remove_var(legacy_name(rest));
        }
    }

    #[test]
    fn new_name_wins_over_legacy() {
        let _g = isolate();
        let rest = "ENV_COMPAT_PROBE_NEW_WINS";
        unsafe {
            std::env::set_var(new_name(rest), "new-value");
            std::env::set_var(legacy_name(rest), "legacy-value");
            assert_eq!(provenance(rest), Provenance::New);
            assert_eq!(get(rest), Some("new-value".to_string()));
            assert!(!used_legacy(rest));
            assert!(legacy_fallback_note(rest).is_none());
            clear(rest);
        }
    }

    #[test]
    fn legacy_fallback_when_new_unset() {
        let _g = isolate();
        let rest = "ENV_COMPAT_PROBE_LEGACY_FALLBACK";
        unsafe {
            clear(rest);
            assert_eq!(provenance(rest), Provenance::Unset);
            assert_eq!(get(rest), None);
            std::env::set_var(legacy_name(rest), "legacy-value");
            assert_eq!(provenance(rest), Provenance::Legacy);
            assert_eq!(get(rest), Some("legacy-value".to_string()));
            assert!(used_legacy(rest));
            let note = legacy_fallback_note(rest).expect("a note for a honored legacy value");
            assert!(note.contains(&legacy_name(rest)), "{note}");
            assert!(note.contains(&new_name(rest)), "{note}");
            clear(rest);
        }
    }

    #[test]
    fn empty_new_falls_back_to_legacy() {
        let _g = isolate();
        let rest = "ENV_COMPAT_PROBE_EMPTY_FALLS_BACK";
        unsafe {
            std::env::set_var(new_name(rest), "");
            std::env::set_var(legacy_name(rest), "legacy-value");
            // An emptied new spelling must not shadow a real legacy value.
            assert_eq!(get(rest), Some("legacy-value".to_string()));
            clear(rest);
            std::env::set_var(new_name(rest), "");
            assert_eq!(get(rest), None);
            clear(rest);
        }
    }

    #[test]
    fn host_control_prefix_covers_both_spellings() {
        assert!(is_host_control_name("AGENTCOWORK_VAULT_KEY"));
        assert!(is_host_control_name("EVERYAIOS_VAULT_KEY"));
        assert!(is_host_control_name("everyaios_home"));
        assert!(!is_host_control_name("ANTHROPIC_API_KEY"));
        assert!(!is_host_control_name("PATH"));
    }

    /// Run `f` with `HOME` pointed at a fresh scratch dir and both home
    /// overrides cleared; restores everything after. Caller holds [`isolate`].
    fn with_scratch_home(tag: &str, f: impl FnOnce(&Path)) {
        let scratch = std::env::temp_dir().join(format!(
            "agentcowork-env-compat-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_new = std::env::var_os(NEW_HOME_VAR);
        let prev_legacy = std::env::var_os(LEGACY_HOME_VAR);
        unsafe {
            std::env::set_var("HOME", &scratch);
            std::env::remove_var(NEW_HOME_VAR);
            std::env::remove_var(LEGACY_HOME_VAR);
            f(&scratch);
            match prev_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            match prev_new {
                Some(v) => std::env::set_var(NEW_HOME_VAR, v),
                None => std::env::remove_var(NEW_HOME_VAR),
            }
            match prev_legacy {
                Some(v) => std::env::set_var(LEGACY_HOME_VAR, v),
                None => std::env::remove_var(LEGACY_HOME_VAR),
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn data_home_new_env_wins() {
        let _g = isolate();
        with_scratch_home("new-env", |scratch| {
            let explicit = scratch.join("explicit-home");
            unsafe {
                std::env::set_var(NEW_HOME_VAR, &explicit);
                std::env::set_var(LEGACY_HOME_VAR, scratch.join("legacy-home"));
            }
            let (resolved, prov) = data_home_with_provenance();
            assert_eq!(resolved, explicit);
            assert_eq!(prov, DataHomeProvenance::NewEnv);
        });
    }

    #[test]
    fn data_home_existing_new_dir_beats_legacy_env() {
        let _g = isolate();
        with_scratch_home("new-dir", |scratch| {
            std::fs::create_dir_all(scratch.join(NEW_DIR_NAME)).unwrap();
            unsafe {
                std::env::set_var(LEGACY_HOME_VAR, scratch.join("legacy-home"));
            }
            let (resolved, prov) = data_home_with_provenance();
            assert_eq!(resolved, scratch.join(NEW_DIR_NAME));
            assert_eq!(prov, DataHomeProvenance::NewDirExists);
        });
    }

    #[test]
    fn data_home_legacy_env_honored() {
        let _g = isolate();
        with_scratch_home("legacy-env", |scratch| {
            let legacy = scratch.join("legacy-home");
            unsafe {
                std::env::set_var(LEGACY_HOME_VAR, &legacy);
            }
            let (resolved, prov) = data_home_with_provenance();
            assert_eq!(resolved, legacy);
            assert_eq!(prov, DataHomeProvenance::LegacyEnv);
        });
    }

    #[test]
    fn data_home_legacy_dir_accepted_then_new_default() {
        let _g = isolate();
        with_scratch_home("legacy-dir", |scratch| {
            std::fs::create_dir_all(scratch.join(LEGACY_DIR_NAME)).unwrap();
            let (resolved, prov) = data_home_with_provenance();
            assert_eq!(resolved, scratch.join(LEGACY_DIR_NAME));
            assert_eq!(prov, DataHomeProvenance::LegacyDirExists);

            let _ = std::fs::remove_dir_all(scratch.join(LEGACY_DIR_NAME));
            let (resolved, prov) = data_home_with_provenance();
            assert_eq!(resolved, scratch.join(NEW_DIR_NAME));
            assert_eq!(prov, DataHomeProvenance::NewDefault);
        });
    }

    #[test]
    fn config_path_prefers_new_keeps_legacy() {
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-env-compat-config-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Fresh: the new filename is the default.
        assert_eq!(config_path_for(&dir), dir.join(CONFIG_FILENAME));

        // Existing install: the legacy file keeps working.
        std::fs::write(dir.join(LEGACY_CONFIG_FILENAME), "retention_days = 7\n").unwrap();
        assert_eq!(config_path_for(&dir), dir.join(LEGACY_CONFIG_FILENAME));

        // Migrated install: the new file wins once it exists.
        std::fs::write(dir.join(CONFIG_FILENAME), "retention_days = 9\n").unwrap();
        assert_eq!(config_path_for(&dir), dir.join(CONFIG_FILENAME));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
