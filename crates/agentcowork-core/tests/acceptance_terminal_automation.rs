//! Track 5 / P54.5 — Automation-profile acceptance suite (non-Windows half).
//!
//! Evidence-gated readiness (§17.12.6). The agent's shell (`script.run`) and
//! durable tasks run on the **automation profile**, not the user's interactive
//! shell — `src-tauri`'s `TerminalPlaneExecutor` calls
//! `resolve_automation_profile(cfg)` for both `terminal_run` and `script.run`,
//! so one resolution decides the shell for the renderer and the model. This
//! suite pins that resolution contract directly:
//!
//! 1. **Selection** — the automation profile wins when set; otherwise the
//!    interactive default is used; both are per-platform.
//! 2. **Resolution** — [`PtyHost::resolve_automation_command`] yields a real
//!    shell command, and an unknown/unavailable name falls back to a detected
//!    profile rather than failing the agent's command.
//!
//! **Windows/ConPTY is deliberately out of scope here.** The Windows
//! acceptance run (spawn `pwsh.exe`/`cmd.exe` via ConPTY, verify OSC 633
//! shell integration + `terminal_resize`) is **not implemented/verified** —
//! see TODO P68.7 / P54.5. This suite only proves the resolution half that
//! runs cross-platform.

use agentcowork_core::terminal::{Platform, PtyHost, TerminalConfig};

fn cfg_with(automation: Option<&str>, default: Option<&str>) -> TerminalConfig {
    let mut cfg = TerminalConfig::default();
    if let Some(a) = automation {
        *cfg.automation_profile.platform_mut() = a.to_string();
    }
    if let Some(d) = default {
        *cfg.default_profile.platform_mut() = d.to_string();
    }
    cfg
}

#[test]
fn automation_profile_name_prefers_automation_then_default() {
    // Nothing configured → no automation shell (the host then refuses
    // honestly rather than guessing).
    let empty = cfg_with(None, None);
    assert_eq!(empty.automation_profile_name(), None);

    // Automation profile wins over the interactive default.
    let both = cfg_with(Some("automation-sh"), Some("interactive-bash"));
    assert_eq!(both.automation_profile_name(), Some("automation-sh"));

    // Automation unset → the interactive default is adopted.
    let default_only = cfg_with(None, Some("interactive-bash"));
    assert_eq!(
        default_only.automation_profile_name(),
        Some("interactive-bash")
    );
}

#[test]
fn automation_profile_is_selected_per_platform() {
    let mut cfg = TerminalConfig::default();
    *cfg.automation_profile.platform_mut_for(Platform::Windows) = "pwsh".into();
    *cfg.automation_profile.platform_mut_for(Platform::Linux) = "sh".into();
    *cfg.automation_profile.platform_mut_for(Platform::Macos) = "zsh".into();

    assert_eq!(
        cfg.automation_profile_name_for(Platform::Windows),
        Some("pwsh")
    );
    assert_eq!(cfg.automation_profile_name_for(Platform::Linux), Some("sh"));
    assert_eq!(
        cfg.automation_profile_name_for(Platform::Macos),
        Some("zsh")
    );
}

#[test]
fn resolver_yields_a_real_shell_command() {
    // A configured automation profile resolves to an actual spawnable
    // command (path + args), which is what `script.run` executes on.
    let cfg = cfg_with(Some("sh"), Some("sh"));
    let (program, _args) = PtyHost::resolve_automation_command(&cfg)
        .expect("the host must resolve an automation shell");
    assert!(!program.trim().is_empty(), "a resolved program is required");
}

#[test]
fn resolver_falls_back_to_a_detected_profile_for_an_unknown_name() {
    // A configured-but-missing name must not make the agent's shell
    // unspawnable: the resolver falls back to a detected profile.
    let cfg = cfg_with(Some("definitely-not-a-shell-xyz"), None);
    let resolved = PtyHost::resolve_automation_command(&cfg);
    assert!(
        resolved.is_some(),
        "an unknown automation name falls back to a detected profile, never None"
    );
}
