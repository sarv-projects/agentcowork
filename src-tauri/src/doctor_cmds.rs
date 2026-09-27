//! P46.2 — `doctor_report` Tauri command: the UI-facing readiness report.
//!
//! Same report as the `agentcowork doctor` CLI (`agentcowork_core::run_doctor`
//! over a `LiveProbe`), returned as JSON so the Settings → Doctor panel can
//! render the per-subsystem lines. Read-only + side-effect-free (no heavy
//! subsystem construction, never prints secret values).

use agentcowork_core::{run_doctor, version, DoctorReport, LiveProbe};

/// Build the live doctor report. No `AppState` needed — the probe reads the
/// data dir + config directly (the same path the headless CLI uses), so the
/// report is identical whether invoked from the cockpit or the terminal.
#[tauri::command]
pub fn doctor_report() -> Result<DoctorReport, String> {
    let probe = LiveProbe::new(agentcowork_core::default_data_dir());
    Ok(run_doctor(version::VERSION, &probe))
}
