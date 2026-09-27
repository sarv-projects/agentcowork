#![cfg(target_os = "linux")]

//! Live Linux E2E — E9 desktop computer-use under Xvfb.
//!
//! Requires: `AGENTCOWORK_LIVE_TEST=1`, an X server on DISPLAY, `python3` with
//! tkinter (for the fixture app), `Xvfb` + `xdpyinfo`. Spawns a small Xvfb
//! on a free display if none is running.

use agentcowork_computeruse::ocr::{OcrEngine, TesseractCli, VisionHit, locate_phrase};
use agentcowork_computeruse::platform::linux::X11Backend;
use agentcowork_computeruse::policy::InteractionMode;
use agentcowork_computeruse::types::{ActKind, Region, WindowInfo};
use agentcowork_computeruse::verify::{Locator, Verifier};
use std::sync::Mutex;

/// Serializes process-environment mutations in this live-test binary.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Find a free display number by probing /tmp/.X11-unix.
fn free_display() -> String {
    for n in 90..=110u32 {
        let lock = std::path::Path::new("/tmp").join(format!(".X{n}-lock"));
        if !lock.exists() {
            return format!(":{n}");
        }
    }
    ":99".into()
}

fn display_live(display: &str) -> bool {
    std::process::Command::new("xdpyinfo")
        .arg("-display")
        .arg(display)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The live fixture app, checked into the repo next to this test. Falls back to
/// a copy in the temp dir for anyone who prefers to keep it there.
fn fixture_app() -> std::path::PathBuf {
    let in_repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e9_app.py");
    if in_repo.exists() {
        return in_repo;
    }
    std::env::temp_dir().join("e9_app.py")
}

fn ensure_xvfb() -> Option<std::process::Child> {
    // If DISPLAY is already live, don't spawn our own.
    if display_live(&std::env::var("DISPLAY").unwrap_or_default()) {
        return None;
    }
    let display = free_display();
    // SAFETY: Each live test holds `ENV_LOCK` for its full execution, so this
    // process-global DISPLAY mutation is serialized with the other live tests.
    unsafe {
        std::env::set_var("DISPLAY", &display);
    }
    let mut child = std::process::Command::new("Xvfb")
        .args([&display, "-screen", "0", "1280x800x24", "-nolisten", "tcp"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Xvfb spawn failed — is Xvfb installed?");
    for _ in 0..50 {
        if display_live(&display) {
            return Some(child);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // The Xvfb child never became ready — reap it before bailing.
    let _ = child.wait();
    panic!("Xvfb on {display} never became ready");
}

#[test]
#[ignore = "live E2E — needs AGENTCOWORK_LIVE_TEST=1 + an X server + python3/tkinter"]
fn live_x11_list_capture_ocr_act_verify() {
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if agentcowork_types::env_compat::get("LIVE_TEST").as_deref() != Some("1") {
        eprintln!("skipping: set AGENTCOWORK_LIVE_TEST=1");
        return;
    }
    let _xvfb = ensure_xvfb();
    let tesseract = TesseractCli::default();
    assert!(tesseract.available(), "tesseract binary missing");

    // 1. Open a window we control: a tiny tkinter app with a "GO" button
    //    that flips a label to "CLICKED" — OCR + XTEST + verify end-to-end.
    let app = fixture_app();
    assert!(app.exists(), "fixture app missing at {}", app.display());
    let mut tk = std::process::Command::new("python3")
        .arg(&app)
        .env("DISPLAY", std::env::var("DISPLAY").unwrap())
        .spawn()
        .expect("python3 spawn failed");
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // 2. List windows — the tk window must be visible.
    let backend = X11Backend::connect().expect("X11 connect");
    let windows = backend.list_windows().expect("list_windows");
    let target = windows
        .iter()
        .find(|w| w.title.contains("agentcowork-e2e"))
        .expect("tk window not listed");
    assert!(target.width > 0 && target.height > 0, "window has size");

    // 3. Capture the window — PNG with plausible dimensions.
    let see = backend
        .see(target, Region::full(target.width, target.height))
        .expect("see");
    assert!(!see.png.is_empty(), "capture is empty");
    assert_eq!(
        &see.png[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "PNG header"
    );

    // 4. OCR the capture — the fixture text must be readable.
    let words = tesseract.ocr(&see.png);
    let joined: String = words
        .iter()
        .map(|w| w.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("E9") || joined.contains("OK") || joined.contains("hello"),
        "OCR did not read the fixture text, got: {joined:?}"
    );

    // 5. Locate the GO button via vision fallback (window coords), then
    //    translate to screen coords and click it with XTEST.
    let hit = locate_phrase(&words, "GO");
    let (cx, cy) = match hit {
        VisionHit::Point { x, y } => (x, y),
        VisionHit::RegionCenter {
            x,
            y,
            width,
            height,
        } => (x + width as i32 / 2, y + height as i32 / 2),
        VisionHit::NotFound => panic!("GO button not found via OCR (words: {joined:?})"),
    };
    // 5a. P57.3 — the background contract refuses the raise on this backend,
    //     so the live test is explicit about asking for the foreground path.
    let background_refusal = backend
        .act(
            target,
            &ActKind::ActivateWindow {
                window_id: target.id,
            },
            InteractionMode::Background,
        )
        .expect_err("background must refuse to raise a window");
    assert!(
        background_refusal
            .to_string()
            .contains("background contract"),
        "unexpected refusal: {background_refusal}"
    );
    backend
        .act(
            target,
            &ActKind::ActivateWindow {
                window_id: target.id,
            },
            InteractionMode::Foreground,
        )
        .expect("activate");
    std::thread::sleep(std::time::Duration::from_millis(300));
    // ActKind::Click takes window-relative coords (the backend adds the
    // window's screen origin itself). OCR words are window-relative, so
    // pass them straight through.
    backend
        .act(
            target,
            &ActKind::Click { x: cx, y: cy },
            InteractionMode::Foreground,
        )
        .expect("click");
    std::thread::sleep(std::time::Duration::from_millis(800));

    // 6. Re-capture + re-OCR — the label must now read CLICKED.
    let see2 = backend
        .see(target, Region::full(target.width, target.height))
        .expect("see2");
    let words2 = tesseract.ocr(&see2.png);
    let joined2: String = words2
        .iter()
        .map(|w| w.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined2.contains("CLICKED"),
        "click did not flip the label, OCR after: {joined2:?}"
    );

    // 7. Verify cascade — the CLICKED text locator is satisfied against OCR.
    let ok_locator = Locator::OcrText {
        text: "CLICKED".into(),
        region: Region::full(target.width, target.height),
    };
    assert!(
        Verifier::satisfied(&ok_locator, None, &words2),
        "verify locator not satisfied (OCR {joined2:?})"
    );

    // `Child` neither kills nor reaps on drop: the fixture app is killed here
    // and then `wait()`ed, or it lingers as a zombie for the rest of the run.
    let _ = tk.kill();
    let _ = tk.wait();
}

/// P57.4 — the Background coordinate click must **not** move the user's pointer.
///
/// This is the contract the whole background/default split exists for, and it is
/// only provable against a real X server: read the pointer, deliver the click,
/// read it again. The click itself is a synthetic `ButtonPress`/`ButtonRelease`
/// addressed to the deepest child under the point. Whether the app *honours* a
/// synthetic event is the app's decision (Tk inspects `send_event`), so the
/// reaction is reported rather than asserted — the pointer invariance is the
/// hard assertion, and the honest-escalation path is asserted too: the call may
/// never silently fail.
#[test]
#[ignore = "live E2E — needs AGENTCOWORK_LIVE_TEST=1 + an X server + python3/tkinter"]
fn live_background_click_leaves_the_pointer_alone() {
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if agentcowork_types::env_compat::get("LIVE_TEST").as_deref() != Some("1") {
        eprintln!("skipping: set AGENTCOWORK_LIVE_TEST=1");
        return;
    }
    let _xvfb = ensure_xvfb();
    let tesseract = TesseractCli::default();
    assert!(tesseract.available(), "tesseract binary missing");

    let app = fixture_app();
    assert!(app.exists(), "fixture app missing at {}", app.display());
    let mut tk = std::process::Command::new("python3")
        .arg(&app)
        .env("DISPLAY", std::env::var("DISPLAY").unwrap())
        .spawn()
        .expect("python3 spawn failed");
    std::thread::sleep(std::time::Duration::from_millis(2000));

    let backend = X11Backend::connect().expect("X11 connect");
    let windows = backend.list_windows().expect("list_windows");
    let target = windows
        .iter()
        .find(|w| w.title.contains("agentcowork-e2e"))
        .expect("tk window not listed");

    // Move the pointer deliberately far from the button, so "unchanged" is a
    // meaningful observation rather than a coincidence.
    let start = (target.x + 5, target.y + 5);
    std::process::Command::new("xdotool")
        .args(["mousemove", &start.0.to_string(), &start.1.to_string()])
        .status()
        .expect("xdotool mousemove (test-only helper)");
    std::thread::sleep(std::time::Duration::from_millis(150));
    let before = backend.pointer_position().expect("pointer before");

    let see = backend
        .see(target, Region::full(target.width, target.height))
        .expect("see");
    let words = tesseract.ocr(&see.png);
    let hit = locate_phrase(&words, "GO");
    let (cx, cy) = match hit {
        VisionHit::Point { x, y } => (x, y),
        VisionHit::RegionCenter {
            x,
            y,
            width,
            height,
        } => (x + width as i32 / 2, y + height as i32 / 2),
        VisionHit::NotFound => panic!("GO button not found via OCR"),
    };

    backend
        .act(
            target,
            &ActKind::Click { x: cx, y: cy },
            InteractionMode::Background,
        )
        .expect("background click must be delivered, not silently skipped");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let after = backend.pointer_position().expect("pointer after");
    assert_eq!(
        before, after,
        "the Background click moved the real pointer: {before:?} -> {after:?}"
    );

    // Report the app's reaction (not an assertion — synthetic-event honouring is
    // the toolkit's choice); a flip means the event landed for real.
    let see2 = backend
        .see(target, Region::full(target.width, target.height))
        .expect("see2");
    let joined2: String = tesseract
        .ocr(&see2.png)
        .iter()
        .map(|w| w.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "background synthetic click: pointer held at {before:?}; fixture OCR after = {joined2:?}"
    );

    // `Child` neither kills nor reaps on drop: the fixture app is killed here
    // and then `wait()`ed, or it lingers as a zombie for the rest of the run.
    let _ = tk.kill();
    let _ = tk.wait();
}

/// P57.1 — a real path-based launch through the X11 backend: the file itself is
/// executed (no shell interpolation), the child does not inherit this process's
/// credentials (H5), and the session environment survives.
#[cfg(unix)]
#[test]
#[ignore = "live — spawns a real program through the X11 backend"]
fn live_path_launch_executes_the_file_without_a_shell_or_secrets() {
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if agentcowork_types::env_compat::get("LIVE_TEST").as_deref() != Some("1") {
        eprintln!("skipping: set AGENTCOWORK_LIVE_TEST=1");
        return;
    }
    // The X11 backend owns the launch path, so it needs a live display even
    // though the launch itself never talks to X. Run this file with
    // `--test-threads=1` when both live tests run together (each ensures its
    // own display and `DISPLAY` is process-global).
    let _xvfb = ensure_xvfb();
    let backend = X11Backend::connect().expect("X11 connect");
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("e9-launch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let argv_out = dir.join("argv.txt");
    let env_out = dir.join("env.txt");
    let script = dir.join("probe.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' \"$0\" > \"{}\"\nenv > \"{}\"\n",
            argv_out.display(),
            env_out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    // A secret in this process the child must not see, and the hand-off paths
    // it must see (neither matches the scrub's credential patterns).
    // SAFETY: `ENV_LOCK` is held for the full test, serializing these
    // process-global environment mutations with the other live tests.
    unsafe {
        std::env::set_var("AGENTCOWORK_LAUNCH_TEST_API_KEY", "must-not-leak");
        std::env::set_var("E9_LAUNCH_ARGV", &argv_out);
        std::env::set_var("E9_LAUNCH_ENV", &env_out);
    }

    let window = WindowInfo {
        id: 0,
        title: String::new(),
        app: String::new(),
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        has_a11y_tree: false,
    };
    backend
        .act(
            &window,
            &ActKind::launch_path(script.to_string_lossy()),
            InteractionMode::Background,
        )
        .expect("launch");
    for _ in 0..50 {
        if argv_out.exists() && env_out.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // `$0` is the canonical path: the exact file was executed, and nothing
    // about a shell command string was involved.
    let argv = std::fs::read_to_string(&argv_out).expect("the launched file ran");
    let canonical = std::fs::canonicalize(&script).unwrap();
    assert_eq!(argv, canonical.to_string_lossy().to_string());

    let child_env = std::fs::read_to_string(&env_out).unwrap();
    assert!(
        !child_env.contains("AGENTCOWORK_LAUNCH_TEST_API_KEY"),
        "the child inherited a credential"
    );
    assert!(child_env.contains("PATH="), "PATH must reach the child");
    // The hand-off vars are not credentials, so they must survive — the scrub is
    // name-filtered, not a blanket clear.
    assert!(child_env.contains("E9_LAUNCH_ARGV="));

    // SAFETY: `ENV_LOCK` is still held for the full test, so cleanup cannot
    // race another live test's environment access.
    unsafe {
        std::env::remove_var("AGENTCOWORK_LAUNCH_TEST_API_KEY");
        std::env::remove_var("E9_LAUNCH_ARGV");
        std::env::remove_var("E9_LAUNCH_ENV");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
