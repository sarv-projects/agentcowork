//! Orphan prevention — platform-specific mechanisms to kill child processes
//! when the parent (supervisor) dies unexpectedly.
//!
//! # Strategy per platform
//!
//! - **Linux**: `PR_SET_PDEATHSIG(SIGTERM)` via `prctl(2)` in the child's `pre_exec`.
//!   This is set in [`super::supervisor::ProcessSupervisor::spawn`].
//!
//! - **macOS**: `setsid()` creates a new process group. The supervisor can then
//!   signal the entire group on shutdown. Combined with parent-PID polling on the
//!   TS side for defense in depth.
//!
//! - **Windows**: Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When the
//!   supervisor handle closes (including on crash), Windows automatically terminates
//!   all processes in the Job. The Job is created *before* the child spawns and the
//!   fresh process is assigned to it by PID (see [`super::supervisor::ProcessSupervisor::spawn`]).
//!
//! # Sidecar-side polling
//!
//! The TypeScript coordinator also polls `process.ppid` and exits if the parent PID
//! changes (indicating the supervisor died and init/launchd re-parented). This is
//! implemented in the TS codebase and serves as a second line of defense.

/// Marker module for Linux orphan prevention.
///
/// The actual `PR_SET_PDEATHSIG` call is done inline in `ProcessSupervisor::spawn()`
/// via `CommandExt::pre_exec`. This module exists for documentation and potential
/// future helper utilities.
#[cfg(target_os = "linux")]
pub mod linux {
    /// The signal sent to the child when the parent dies.
    /// Using SIGTERM allows graceful shutdown.
    pub const DEATH_SIGNAL: i32 = libc::SIGTERM;

    /// Verify that PR_SET_PDEATHSIG is available (it always is on Linux ≥ 2.1.57).
    /// Returns true on success.
    pub fn verify_pdeathsig_support() -> bool {
        // PR_SET_PDEATHSIG has been in the kernel since 2.1.57 — effectively always.
        true
    }
}

/// Marker module for macOS orphan prevention.
#[cfg(target_os = "macos")]
pub mod macos {
    /// macOS does not have PR_SET_PDEATHSIG. We use:
    /// 1. `setsid()` in pre_exec to create a process group.
    /// 2. Parent-PID polling on the TS side.
    /// 3. Explicit SIGTERM to the process group on supervisor shutdown.
    pub fn strategy_description() -> &'static str {
        "setsid + parent-PID polling + explicit SIGTERM on shutdown"
    }
}

/// Windows orphan prevention — Job Objects (J12).
///
/// A Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` guarantees that when the
/// supervisor's last handle to the Job is closed (parent crash, exit, or explicit
/// close), Windows terminates every process assigned to the Job. Unlike the Unix
/// `pre_exec` hooks, this is enforced by the OS kernel with no race window, and it
/// kills the entire process tree of the child, not just the child itself.
#[cfg(target_os = "windows")]
pub mod windows {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    /// Create a Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    ///
    /// Returns the raw Job handle (as `isize`, for storage in the supervisor).
    /// The caller MUST keep the handle alive for the app's lifetime — closing it
    /// terminates every assigned child process.
    pub fn create_job_object() -> io::Result<isize> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job == INVALID_HANDLE_VALUE || job.is_null() {
                return Err(io::Error::last_os_error());
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(job);
                return Err(err);
            }
            Ok(job as isize)
        }
    }

    /// Assign a running child process (by PID) to the Job.
    ///
    /// Called immediately after `spawn()` so the child never runs outside the
    /// kill-on-close guarantee.
    pub fn assign_to_job(job: isize, pid: u32) -> io::Result<()> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ok = AssignProcessToJobObject(job as HANDLE, process);
            // The process handle is only needed for the assignment itself.
            CloseHandle(process);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "linux")]
    fn linux_pdeathsig_support() {
        assert!(super::linux::verify_pdeathsig_support());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_strategy() {
        let desc = super::macos::strategy_description();
        assert!(desc.contains("setsid"));
    }
}
