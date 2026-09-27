//! agentcowork-core binary entrypoint.
//!
//! P0.1 scope: `agentcowork-core --version` prints the banner; a plain boot
//! loads config, opens/creates the vault, and reports readiness.
//!
//! P0.4 scope: `--coordinator-bin <path>` starts the ProcessSupervisor which
//! spawns and monitors the TS coordinator sidecar. The supervisor loop runs
//! synchronously on the main thread (or a dedicated thread in Tauri).

use std::process::ExitCode;

use agentcowork_core::{boot, coordinator_bin_from_args, start_supervisor, version};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", version::banner());
        return ExitCode::SUCCESS;
    }

    // P51.19 — acpx refuses to mutate a session unless doctor passes.
    if args.first().map(String::as_str) == Some("acpx") {
        let probe = agentcowork_core::LiveProbe::new(agentcowork_core::default_data_dir());
        let report = agentcowork_core::run_doctor(agentcowork_core::version::VERSION, &probe);
        let doctor_ok = report.exit_code() == 0;
        let home = agentcowork_core::default_data_dir().join("acpx-sessions.json");
        match agentcowork_core::acpx::run_acpx(&args[1..], &home, doctor_ok) {
            Ok(text) => {
                if !text.is_empty() {
                    println!("{text}");
                }
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("acpx: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // P46.2 — `agentcowork doctor`: per-subsystem readiness report (spec H35).
    // A support primitive: diagnose a broken component without a support
    // ticket. `--json` emits the machine-readable report; the default is a
    // human table. Exit code is 1 only when a v1-required subsystem is broken.
    if args.iter().any(|a| a == "doctor") {
        let json = args.iter().any(|a| a == "--json");
        let probe = agentcowork_core::LiveProbe::new(agentcowork_core::default_data_dir());
        let report = agentcowork_core::run_doctor(agentcowork_core::version::VERSION, &probe);
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            print!("{}", report.render_table());
        }
        return if report.exit_code() == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    // P40.1 — the headless runtime profile (spec H33): the same binary, no
    // tray/UI, for the always-on executor node. The core + B7 scheduler
    // (inside the coordinator) + browser/script/office engines all run; the
    // engines stay command-driven (constructed on first use, never at boot).
    let headless = args.iter().any(|a| a == "--headless");

    match boot(&args) {
        Ok(msg) => {
            eprintln!("{msg}");
        }
        Err(e) => {
            eprintln!("agentcowork-core: boot failed: {e}");
            return ExitCode::FAILURE;
        }
    }

    if headless {
        eprintln!(
            "[main] headless runtime profile (no tray/UI) — scheduled work runs via the coordinator's B7 scheduler"
        );
        if coordinator_bin_from_args(&args).is_none() {
            eprintln!(
                "[main] WARNING: --headless without --coordinator-bin has no scheduler; pass --coordinator-bin <bun binary> to run B7 due-work"
            );
        }
    }

    // If --coordinator-bin <path> is provided, start the supervisor.
    if let Some(bin_path) = coordinator_bin_from_args(&args) {
        eprintln!("[main] starting supervisor for: {}", bin_path.display());

        let mut supervisor = match start_supervisor(bin_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[main] failed to create supervisor: {e}");
                return ExitCode::FAILURE;
            }
        };

        eprintln!("[main] supervisor state: {}", supervisor.state);

        match supervisor.wait_or_restart() {
            Ok(()) => {
                eprintln!("[main] supervisor exited cleanly");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("[main] supervisor error: {e}");
                eprintln!("[main] final state: {}", supervisor.state);
                ExitCode::FAILURE
            }
        }
    } else {
        // No coordinator binary specified — just boot and exit (headless mode).
        ExitCode::SUCCESS
    }
}
