//! DEC-053 — the one-time data-home migration, owned solely by Core.
//!
//! Each test redirects `HOME` at a scratch dir and restores the process
//! environment after, serialized through one mutex (Rust runs tests in one
//! process with shared env). These run in their own test binary, isolated
//! from the lib tests' env use.

use agentcowork_core::data_home::{MigrationOutcome, ensure_data_home, migrate_dir};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static ENV: Mutex<()> = Mutex::new(());

struct EnvGuard {
    home: Option<std::ffi::OsString>,
    new_home: Option<std::ffi::OsString>,
    legacy_home: Option<std::ffi::OsString>,
}

impl EnvGuard {
    /// Hold the env lock, point `HOME` at a fresh `scratch` dir and clear
    /// both home overrides. Restores everything on drop.
    fn scratch(tag: &str) -> (MutexGuard<'static, ()>, PathBuf, EnvGuard) {
        let g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = std::env::temp_dir().join(format!(
            "agentcowork-data-home-{tag}-{}-{}",
            std::process::id(),
            tag.len()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let guard = EnvGuard {
            home: std::env::var_os("HOME"),
            new_home: std::env::var_os("AGENTCOWORK_HOME"),
            legacy_home: std::env::var_os("EVERYAIOS_HOME"),
        };
        unsafe {
            std::env::set_var("HOME", &scratch);
            std::env::remove_var("AGENTCOWORK_HOME");
            std::env::remove_var("EVERYAIOS_HOME");
        }
        (g, scratch, guard)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.home.take() {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            match self.new_home.take() {
                Some(v) => std::env::set_var("AGENTCOWORK_HOME", v),
                None => std::env::remove_var("AGENTCOWORK_HOME"),
            }
            match self.legacy_home.take() {
                Some(v) => std::env::set_var("EVERYAIOS_HOME", v),
                None => std::env::remove_var("EVERYAIOS_HOME"),
            }
        }
    }
}

fn write(dir: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
}

/// 1. new-path-wins: an explicit `AGENTCOWORK_HOME` beats a legacy
///    `EVERYAIOS_HOME`, with no migration attempted.
#[test]
fn new_path_wins_over_legacy() {
    let (_g, _scratch, _env) = EnvGuard::scratch("new-wins");
    let new = std::env::temp_dir().join("agentcowork-data-home-new-target");
    let legacy = std::env::temp_dir().join("agentcowork-data-home-legacy-target");
    unsafe {
        std::env::set_var("AGENTCOWORK_HOME", &new);
        std::env::set_var("EVERYAIOS_HOME", &legacy);
    }
    let report = ensure_data_home();
    assert_eq!(report.resolved, new);
    assert!(!report.migrated);
    assert!(report.note.is_none());
    unsafe {
        std::env::remove_var("AGENTCOWORK_HOME");
        std::env::remove_var("EVERYAIOS_HOME");
    }
}

/// 2. legacy-fallback: with nothing new anywhere, a legacy override is
///    honored as-is (nothing exists to migrate).
#[test]
fn legacy_fallback_when_nothing_new() {
    let (_g, scratch, _env) = EnvGuard::scratch("legacy-fallback");
    let legacy = scratch.join("pointed-legacy");
    unsafe {
        std::env::set_var("EVERYAIOS_HOME", &legacy);
    }
    let report = ensure_data_home();
    assert_eq!(report.resolved, legacy);
    assert!(!report.migrated);
    unsafe {
        std::env::remove_var("EVERYAIOS_HOME");
    }
}

/// 3+5. A legacy `~/.everyaios` migrates once (directory *and* the config
/// filename inside it), and a second run is a no-op, not an error.
#[test]
fn legacy_dir_migrates_once_and_second_run_is_noop() {
    let (_g, scratch, _env) = EnvGuard::scratch("migrate-once");
    let legacy = scratch.join(".everyaios");
    write(&legacy, "sentinel.txt", "user data");
    write(
        &legacy,
        "everyaios.toml",
        "data_dir = \".\"\nretention_days = 9\n",
    );

    let first = ensure_data_home();
    assert!(first.migrated, "first run moves the legacy home");
    assert_eq!(first.resolved, scratch.join(".agentcowork"));
    assert_eq!(first.migrated_from, Some(legacy.clone()));
    assert!(
        !legacy.exists(),
        "the legacy directory is gone after rename"
    );
    assert_eq!(
        std::fs::read_to_string(first.resolved.join("sentinel.txt")).unwrap(),
        "user data"
    );
    // The config file moves with the home, into the new spelling.
    assert!(first.resolved.join("agentcowork.toml").is_file());
    assert!(!first.resolved.join("everyaios.toml").exists());
    assert!(
        std::fs::read_to_string(first.resolved.join("agentcowork.toml"))
            .unwrap()
            .contains("retention_days = 9")
    );

    let second = ensure_data_home();
    assert!(!second.migrated, "second run is a no-op");
    assert_eq!(second.resolved, first.resolved);
    assert!(second.note.is_none());
    assert_eq!(
        std::fs::read_to_string(second.resolved.join("sentinel.txt")).unwrap(),
        "user data"
    );
}

/// 4. destination-exists-skip: when both homes exist nothing is merged or
///    overwritten — both sides stay intact and the run says so.
#[test]
fn destination_exists_skips_without_merge() {
    let (_g, scratch, _env) = EnvGuard::scratch("dest-exists");
    let new = scratch.join(".agentcowork");
    let legacy = scratch.join(".everyaios");
    write(&new, "new-side.txt", "new");
    write(&legacy, "legacy-side.txt", "legacy");

    let report = ensure_data_home();
    assert!(!report.migrated);
    assert_eq!(report.resolved, new, "the existing new home wins");
    let note = report.note.expect("the skip must be logged");
    assert!(note.contains("untouched"), "{note}");
    // Never merged, never overwritten: both sentinels survive.
    assert_eq!(
        std::fs::read_to_string(new.join("new-side.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(legacy.join("legacy-side.txt")).unwrap(),
        "legacy"
    );
    assert!(!new.join("legacy-side.txt").exists());
}

/// 5b. migration-failure-degrades: when the move cannot happen (read-only
/// profile dir), the legacy path stays in service and the run reports the
/// failure instead of losing data.
#[test]
fn migration_failure_degrades_to_legacy() {
    use std::os::unix::fs::PermissionsExt;
    let (_g, scratch, _env) = EnvGuard::scratch("degraded");
    let legacy = scratch.join(".everyaios");
    write(&legacy, "sentinel.txt", "user data");

    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o555)).unwrap();
    let report = ensure_data_home();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!report.migrated);
    assert_eq!(report.resolved, legacy, "keeps using the legacy path");
    let note = report.note.expect("the failure must be logged");
    assert!(note.contains("failed"), "{note}");
    assert_eq!(
        std::fs::read_to_string(legacy.join("sentinel.txt")).unwrap(),
        "user data",
        "no data moved"
    );
    assert!(
        !scratch.join(".agentcowork").exists(),
        "no half-migrated destination left behind"
    );
}

/// `migrate_dir` never merges into an existing destination.
#[test]
fn migrate_dir_destination_exists_is_not_an_error() {
    let (_g, scratch, _env) = EnvGuard::scratch("migrate-dir");
    let legacy = scratch.join("legacy");
    let dest = scratch.join("dest");
    write(&legacy, "a.txt", "a");
    write(&dest, "b.txt", "b");

    assert_eq!(
        migrate_dir(&legacy, &dest),
        MigrationOutcome::DestinationExists
    );
    assert!(legacy.join("a.txt").is_file());
    assert!(dest.join("b.txt").is_file());
    assert!(!dest.join("a.txt").exists());
}

/// `migrate_dir` on a failed copy degrades: legacy untouched, no partial
/// destination left behind.
#[test]
fn migrate_dir_failure_leaves_no_partial_destination() {
    use std::os::unix::fs::PermissionsExt;
    let (_g, scratch, _env) = EnvGuard::scratch("migrate-dir-fail");
    let legacy = scratch.join("legacy");
    write(&legacy, "a.txt", "a");
    let ro = scratch.join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();

    let outcome = migrate_dir(&legacy, &ro.join("dest"));
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(outcome, MigrationOutcome::Degraded);
    assert!(legacy.join("a.txt").is_file(), "legacy untouched");
    assert!(!ro.join("dest").exists(), "no partial destination");
}
