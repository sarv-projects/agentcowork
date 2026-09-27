//! P57.8 — the installed-application inventory behind Settings → Computer use.
//!
//! The Settings surface needs two things from the OS: the apps a user could
//! allow-list, and an honest refusal set (an app the policy can *never*
//! automate is shown as blocked, never as addable). Both live here rather than
//! in the Tauri layer so the discovery rules are testable without a display.
//!
//! Platform sources, in the order the desktop actually resolves a program:
//!
//! * **Linux** — freedesktop `*.desktop` entries under `XDG_DATA_HOME`,
//!   `XDG_DATA_DIRS`, `/usr/local/share/applications`,
//!   `/usr/share/applications` (the executable in `Exec=`, minus
//!   field-code placeholders).
//! * **macOS** — `*.app` bundles in `/Applications`, `/System/Applications`,
//!   `~/Applications` (the bundle path is what `open -a` takes).
//! * **Windows** — Start Menu shortcuts (`.lnk`) under `%APPDATA%` and
//!   `%ProgramData%`; `%ProgramFiles%` executables are deliberately *not*
//!   enumerated (hundreds of non-app entries) — use **Add by path** for those.
//!
//! Nothing here launches or executes anything: it reads directory listings and
//! `.desktop` text. Every root is injected, so tests use a temp tree.

use std::path::{Path, PathBuf};

use crate::policy::AppPolicy;

/// Where an inventory row came from (shown honestly in the picker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppSource {
    /// A freedesktop `.desktop` entry (Linux).
    DesktopEntry,
    /// An application bundle directory (macOS).
    AppBundle,
    /// A Start Menu shortcut (Windows).
    StartMenu,
    /// Added by the user through **Add by path**.
    UserPath,
}

impl AppSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppSource::DesktopEntry => "desktop_entry",
            AppSource::AppBundle => "app_bundle",
            AppSource::StartMenu => "start_menu",
            AppSource::UserPath => "user_path",
        }
    }
}

/// One installed application the user could allow-list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstalledApp {
    /// Display name (`Name=` for a desktop entry, the bundle/app stem otherwise).
    pub name: String,
    /// Absolute path to the program (or bundle) — what the allow-list stores.
    pub path: String,
    pub source: AppSource,
    /// `Some(reason)` when the policy hard-denies this app: the UI must show a
    /// block, never an Add button, and the backend refuses the add anyway.
    pub hard_denied: Option<String>,
    /// True when this path is already allow-listed.
    pub allow_listed: bool,
}

/// Parse one freedesktop `.desktop` document → `(name, exec)`.
///
/// Deliberately strict, because a wrong row is worse than a missing one:
/// `Hidden=true` and non-`Application` types are skipped, `Exec` is required,
/// and field codes (`%U`, `%F`, `%i`, …) are dropped so the stored path is the
/// executable itself.
pub fn parse_desktop_entry(text: &str) -> Option<(String, String)> {
    let mut name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut hidden = false;
    let mut is_application = false;
    let mut in_group = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            // Only the primary group carries the launch definition.
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Type" => is_application = value.eq_ignore_ascii_case("Application"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            "Name" if name.is_none() => name = Some(value.to_string()),
            "Exec" if exec.is_none() => {
                let binary = value
                    .split_whitespace()
                    .find(|tok| !tok.starts_with('%'))
                    .unwrap_or_default()
                    .trim_matches('"');
                if !binary.is_empty() {
                    exec = Some(binary.to_string());
                }
            }
            _ => {}
        }
    }
    if hidden || !is_application {
        return None;
    }
    let name = name?;
    let exec = exec?;
    Some((name, exec))
}

/// Resolve an `Exec=` value to an absolute path without executing anything:
/// an absolute path is taken as-is, a bare name is looked up across the given
/// directories (the `PATH` the caller passes in).
fn resolve_exec(exec: &str, path_dirs: &[PathBuf]) -> Option<String> {
    let p = Path::new(exec);
    if p.is_absolute() {
        return Some(exec.to_string());
    }
    path_dirs
        .iter()
        .map(|d| d.join(exec))
        .find(|c| c.is_file())
        .map(|c| c.to_string_lossy().into_owned())
}

/// Collect `.desktop` entries from the given application roots (bounded walk,
/// 2 levels — freedesktop nests at most one vendor directory).
pub fn collect_desktop_entries(roots: &[PathBuf], path_dirs: &[PathBuf]) -> Vec<InstalledApp> {
    let mut out: Vec<InstalledApp> = Vec::new();
    for root in roots {
        collect_desktop_dir(root, path_dirs, 0, &mut out);
    }
    dedupe(&mut out);
    out.sort_by_key(|a| a.name.to_ascii_lowercase());
    out
}

fn collect_desktop_dir(dir: &Path, path_dirs: &[PathBuf], depth: u8, out: &mut Vec<InstalledApp>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 2 {
                collect_desktop_dir(&path, path_dirs, depth + 1, out);
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some((name, exec)) = parse_desktop_entry(&text) {
            if let Some(resolved) = resolve_exec(&exec, path_dirs) {
                out.push(InstalledApp {
                    name,
                    path: resolved,
                    source: AppSource::DesktopEntry,
                    hard_denied: None,
                    allow_listed: false,
                });
            }
        }
    }
}

/// Collect application bundles from the given directories (macOS shape; also
/// used by tests on any platform, since a bundle is just a `.app` directory).
pub fn collect_app_bundles(roots: &[PathBuf]) -> Vec<InstalledApp> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            out.push(InstalledApp {
                name,
                path: path.to_string_lossy().into_owned(),
                source: AppSource::AppBundle,
                hard_denied: None,
                allow_listed: false,
            });
        }
    }
    dedupe(&mut out);
    out.sort_by_key(|a| a.name.to_ascii_lowercase());
    out
}

/// Windows: Start Menu shortcuts. Kept separate so the Linux/macOS builds do
/// not need the extension check, and so the walk depth is explicit.
#[cfg(windows)]
pub fn collect_start_menu(roots: &[PathBuf]) -> Vec<InstalledApp> {
    fn walk(dir: &Path, depth: u8, out: &mut Vec<InstalledApp>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 3 {
                    walk(&path, depth + 1, out);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("lnk") {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            out.push(InstalledApp {
                name,
                path: path.to_string_lossy().into_owned(),
                source: AppSource::StartMenu,
                hard_denied: None,
                allow_listed: false,
            });
        }
    }
    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut out);
    }
    dedupe(&mut out);
    out.sort_by_key(|a| a.name.to_ascii_lowercase());
    out
}

/// Keep the first row per path (a `.desktop` shadowed by a later root must not
/// become two Settings rows).
fn dedupe(apps: &mut Vec<InstalledApp>) {
    let mut seen: Vec<String> = Vec::new();
    apps.retain(|a| {
        let key = a.path.to_ascii_lowercase();
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
}

#[cfg(target_os = "linux")]
fn env_dirs(var: &str) -> Vec<PathBuf> {
    std::env::var(var)
        .ok()
        .map(|v| {
            v.split(if cfg!(windows) { ';' } else { ':' })
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn home() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// The application roots for this platform, in resolution order.
pub fn platform_app_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = home() {
            roots.push(home.join(".local/share/applications"));
            roots.push(home.join(".local/share/flatpak/exports/share/applications"));
        }
        if let Some(xdg) = std::env::var("XDG_DATA_HOME").ok().map(PathBuf::from) {
            roots.push(xdg.join("applications"));
        }
        roots.extend(
            env_dirs("XDG_DATA_DIRS")
                .into_iter()
                .map(|d| d.join("applications")),
        );
        roots.push(PathBuf::from("/usr/local/share/applications"));
        roots.push(PathBuf::from("/usr/share/applications"));
    }
    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Applications"));
        roots.push(PathBuf::from("/System/Applications"));
        if let Some(home) = home() {
            roots.push(home.join("Applications"));
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            roots.push(PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs"));
        }
        if let Ok(programdata) = std::env::var("PROGRAMDATA") {
            roots.push(PathBuf::from(programdata).join("Microsoft/Windows/Start Menu/Programs"));
        }
    }
    // `XDG_DATA_DIRS` normally already lists `/usr/local/share` and
    // `/usr/share`; scanning them twice is wasted work (the row dedupe would
    // hide the duplicate, but not the cost).
    let mut seen: Vec<String> = Vec::new();
    roots.retain(|r| {
        let key = r.to_string_lossy().to_ascii_lowercase();
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
    roots
}

/// The `PATH` an `Exec=` value is resolved against.
#[cfg(target_os = "linux")]
fn exec_search_dirs() -> Vec<PathBuf> {
    env_dirs("PATH")
}

/// Enumerate installed applications for this platform, mark hard-denied rows
/// and already-allow-listed rows.
pub fn installed_apps(policy: &AppPolicy) -> Vec<InstalledApp> {
    let roots = platform_app_roots();
    let mut apps: Vec<InstalledApp> = Vec::new();
    #[cfg(target_os = "linux")]
    {
        apps.extend(collect_desktop_entries(&roots, &exec_search_dirs()));
    }
    #[cfg(target_os = "macos")]
    {
        apps.extend(collect_app_bundles(&roots));
    }
    #[cfg(windows)]
    {
        apps.extend(collect_start_menu(&roots));
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = roots;
    }
    annotate(&mut apps, policy);
    apps
}

/// One `Add by path` row. The caller has already canonicalized the path (the
/// policy owns that step); this marks the row against the current policy so a
/// freshly accepted path reads as listed and a hard-denied one as blocked.
pub fn user_path_row(path: &str, policy: &AppPolicy) -> InstalledApp {
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let mut row = InstalledApp {
        name: stem,
        path: path.to_string(),
        source: AppSource::UserPath,
        hard_denied: None,
        allow_listed: false,
    };
    annotate_one(&mut row, policy);
    row
}

/// Fill in `hard_denied` / `allow_listed` from the policy — the two facts the
/// picker must not guess. This is also safe to run over a session-cached
/// inventory after a policy write: it updates annotations without rescanning
/// the platform's application directories.
pub fn annotate_inventory(apps: &mut [InstalledApp], policy: &AppPolicy) {
    annotate(apps, policy);
}

fn annotate(apps: &mut [InstalledApp], policy: &AppPolicy) {
    for app in apps.iter_mut() {
        annotate_one(app, policy);
    }
}

fn annotate_one(app: &mut InstalledApp, policy: &AppPolicy) {
    let stem = Path::new(&app.path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    app.hard_denied =
        AppPolicy::hard_deny(&stem, None).or_else(|| AppPolicy::hard_deny(&app.name, None));
    app.allow_listed = policy
        .allow_paths
        .iter()
        .any(|p| p.eq_ignore_ascii_case(&app.path))
        || policy
            .allow_list
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&stem) || a.eq_ignore_ascii_case(&app.name));
}

/// Search the inventory: every character of the query must appear in order in
/// the name or the path (the same all-character rule Settings search uses), so
/// `gimp` finds `GNU Image Manipulation Program` by name and `firefox` finds it
/// by path. An empty query returns everything.
pub fn search_apps(apps: &[InstalledApp], query: &str) -> Vec<InstalledApp> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return apps.to_vec();
    }
    apps.iter()
        .filter(|a| {
            subsequence(&a.name.to_ascii_lowercase(), &q)
                || subsequence(&a.path.to_ascii_lowercase(), &q)
        })
        .cloned()
        .collect()
}

/// Both sides are already lowercased by the caller.
fn subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|n| chars.any(|h| h == n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ea-apps-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn desktop_entry_parses_exec_and_drops_field_codes() {
        let text =
            "[Desktop Entry]\nType=Application\nName=Text Editor\nExec=gedit %U\nIcon=gedit\n";
        let (name, exec) = parse_desktop_entry(text).unwrap();
        assert_eq!(name, "Text Editor");
        assert_eq!(exec, "gedit");
    }

    #[test]
    fn desktop_entry_skips_hidden_and_non_applications() {
        assert!(
            parse_desktop_entry("[Desktop Entry]\nType=Application\nHidden=true\nName=X\nExec=x\n")
                .is_none()
        );
        assert!(parse_desktop_entry("[Desktop Entry]\nType=Link\nName=X\nExec=x\n").is_none());
        assert!(parse_desktop_entry("[Desktop Entry]\nType=Application\nName=X\n").is_none());
        assert!(parse_desktop_entry("not a desktop file").is_none());
    }

    #[test]
    fn desktop_entry_ignores_other_groups() {
        // A `[Desktop Action …]` block's Name/Exec must not win the row.
        let text = "[Desktop Entry]\nType=Application\nName=Apps\nExec=apps %F\n\n[Desktop Action new]\nName=New Window\nExec=apps --new\n";
        let (name, exec) = parse_desktop_entry(text).unwrap();
        assert_eq!((name.as_str(), exec.as_str()), ("Apps", "apps"));
    }

    #[test]
    fn collection_resolves_a_bare_exec_against_path() {
        let root = tmp("collect");
        let bin = tmp("collect-bin");
        let exe = bin.join("myeditor");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        std::fs::write(
            root.join("myeditor.desktop"),
            b"[Desktop Entry]\nType=Application\nName=My Editor\nExec=myeditor\n",
        )
        .unwrap();
        let apps = collect_desktop_entries(std::slice::from_ref(&root), std::slice::from_ref(&bin));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].path, exe.to_string_lossy());
        // A vendor subdirectory is walked.
        let sub = root.join("vendor");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("other.desktop"),
            b"[Desktop Entry]\nType=Application\nName=Other\nExec=/usr/bin/other\n",
        )
        .unwrap();
        assert_eq!(
            collect_desktop_entries(std::slice::from_ref(&root), &[bin]).len(),
            2
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(tmp("collect-bin"));
    }

    #[test]
    fn collection_dedupes_by_path() {
        let root = tmp("dedupe");
        let a = root.join("a");
        std::fs::create_dir_all(&a).unwrap();
        for (dir, name) in [(&root, "one"), (&a, "two")] {
            std::fs::write(
                dir.join(format!("{name}.desktop")),
                format!("[Desktop Entry]\nType=Application\nName={name}\nExec=/usr/bin/shared\n"),
            )
            .unwrap();
        }
        let apps = collect_desktop_entries(std::slice::from_ref(&root), &[]);
        assert_eq!(apps.len(), 1, "one row per program, not per desktop file");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn annotation_marks_hard_denied_and_allow_listed() {
        let mut policy = AppPolicy::default();
        policy.allow_paths.push("/usr/bin/gedit".into());
        let mut apps = vec![
            InstalledApp {
                name: "Files".into(),
                path: "/usr/bin/nautilus".into(),
                source: AppSource::DesktopEntry,
                hard_denied: None,
                allow_listed: false,
            },
            InstalledApp {
                name: "Terminal".into(),
                path: "/usr/bin/gnome-terminal".into(),
                source: AppSource::DesktopEntry,
                hard_denied: None,
                allow_listed: false,
            },
            InstalledApp {
                name: "Text Editor".into(),
                path: "/usr/bin/gedit".into(),
                source: AppSource::DesktopEntry,
                hard_denied: None,
                allow_listed: false,
            },
        ];
        annotate(&mut apps, &policy);
        assert!(apps[0].hard_denied.is_none());
        assert!(!apps[0].allow_listed);
        assert!(apps[1].hard_denied.is_some(), "a terminal is never addable");
        assert!(apps[2].allow_listed, "an allow-listed path reads as listed");
    }

    #[test]
    fn cached_inventory_annotations_follow_policy_changes_without_rescanning() {
        let mut apps = vec![InstalledApp {
            name: "Text Editor".into(),
            path: "/usr/bin/gedit".into(),
            source: AppSource::DesktopEntry,
            hard_denied: None,
            allow_listed: false,
        }];

        let empty = AppPolicy::default();
        annotate_inventory(&mut apps, &empty);
        assert!(!apps[0].allow_listed);

        let mut updated = AppPolicy::default();
        updated.allow_paths.push("/usr/bin/gedit".into());
        annotate_inventory(&mut apps, &updated);
        assert!(apps[0].allow_listed);

        annotate_inventory(&mut apps, &empty);
        assert!(!apps[0].allow_listed);
    }

    #[test]
    fn search_is_all_character_over_name_and_path() {
        let apps = vec![
            InstalledApp {
                name: "GNU Image Manipulation Program".into(),
                path: "/usr/bin/gimp".into(),
                source: AppSource::DesktopEntry,
                hard_denied: None,
                allow_listed: false,
            },
            InstalledApp {
                name: "Firefox".into(),
                path: "/usr/lib/firefox/firefox".into(),
                source: AppSource::DesktopEntry,
                hard_denied: None,
                allow_listed: false,
            },
        ];
        assert_eq!(search_apps(&apps, "").len(), 2);
        assert_eq!(
            search_apps(&apps, "gimp")[0].name,
            "GNU Image Manipulation Program"
        );
        assert_eq!(search_apps(&apps, "manip").len(), 1);
        assert_eq!(search_apps(&apps, "frefox").len(), 1, "subsequence match");
        assert!(search_apps(&apps, "zzz").is_empty());
    }

    /// Live inventory against the **real** machine (network-free, but it reads
    /// the host's application directories, so it is opt-in): proves the picker
    /// is populated from the OS and not from a fixture.
    /// `cargo test -p agentcowork-computeruse live_inventory -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_inventory_reads_this_host() {
        let policy = AppPolicy::default();
        let roots = platform_app_roots();
        let apps = installed_apps(&policy);
        let denied = apps.iter().filter(|a| a.hard_denied.is_some()).count();
        println!(
            "roots={} found={} hard_denied={}",
            roots.len(),
            apps.len(),
            denied
        );
        for a in apps.iter().take(5) {
            println!("  {} → {} ({})", a.name, a.path, a.source.as_str());
        }
        for r in &roots {
            println!("  root: {}", r.display());
        }
        // A Linux host with a desktop environment has applications; a bare
        // container legitimately has none, so only the shape is asserted.
        assert!(apps.iter().all(|a| !a.path.is_empty()));
        assert!(apps.iter().all(|a| a.source != AppSource::UserPath));
        assert!(search_apps(&apps, "").len() == apps.len());
    }

    #[test]
    fn app_bundles_are_collected_by_directory() {
        let root = tmp("bundles");
        for name in ["Safari", "Notes"] {
            std::fs::create_dir_all(root.join(format!("{name}.app"))).unwrap();
        }
        let apps = collect_app_bundles(std::slice::from_ref(&root));
        assert_eq!(apps.len(), 2);
        assert!(apps.iter().all(|a| a.source == AppSource::AppBundle));
        assert!(apps.iter().any(|a| a.path.ends_with("Safari.app")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
