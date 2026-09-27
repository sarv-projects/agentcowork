//! P7.9 — Warm worker pool (doc 64 §5.5 — chromium `zygote_linux.cc`
//! fork → sandbox-flags-over-fd → child applies seccomp before exec). The
//! pool pre-spawns N sandboxed workers and keeps them warm; a job is
//! assigned to an already-sandboxed worker instead of paying a cold spawn
//! (which is where a zygote saves most of its cost). Complements J13
//! (warm pool, doc 43).
//!
//! Each worker is a child process launched with its sandbox profile passed
//! as flags (`--profile <name> --scratch <path>` over the inherited stdio
//! handle); the worker applies the profile at startup (the P7.8 `apply`
//! seam) before serving requests. The pool tracks liveness, hands out
//! workers on demand, and refills/resizes on exit.

use std::io::{BufRead, BufReader, Write}; // BufRead for read_line
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A sandboxed worker child.
pub struct SandboxedWorker {
    pub id: usize,
    pub profile: String,
    pub scratch: Option<String>,
    pub child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Currently assigned to a job?
    pub busy: bool,
}

impl SandboxedWorker {
    /// Spawn one warm worker. The profile flags are passed over the child's
    /// inherited stdio — the child applies its sandbox before serving.
    pub fn spawn(
        binary: &str,
        id: usize,
        profile: &str,
        scratch: Option<&str>,
    ) -> std::io::Result<SandboxedWorker> {
        let mut cmd = Command::new(binary);
        cmd.arg("--profile")
            .arg(profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(s) = scratch {
            cmd.arg("--scratch").arg(s);
        }
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("worker stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("worker stdout unavailable"))?;
        let mut w = SandboxedWorker {
            id,
            profile: profile.to_string(),
            scratch: scratch.map(|s| s.to_string()),
            child,
            stdin,
            stdout: BufReader::new(stdout),
            busy: false,
        };
        // The worker signals readiness after applying its sandbox profile —
        // a warm worker is one that already passed the apply step.
        let mut ready = String::new();
        let n = w.stdout.read_line(&mut ready)?;
        if n == 0 {
            return Err(std::io::Error::other(format!(
                "worker {id} exited before reporting ready"
            )));
        }
        let ready = ready.trim().to_string();
        if !ready.starts_with("ready ") {
            return Err(std::io::Error::other(format!(
                "worker {id} did not report ready (got `{ready}`)"
            )));
        }
        Ok(w)
    }

    /// Run a job on this worker: send the request, read the ack line.
    pub fn run(&mut self, request: &str) -> std::io::Result<String> {
        writeln!(self.stdin, "{request}")?;
        self.stdin.flush()?;
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                format!("worker {} died mid-job", self.id),
            ));
        }
        Ok(line.trim().to_string())
    }

    /// Ask the worker to exit (graceful).
    pub fn shutdown(&mut self) {
        let _ = writeln!(self.stdin, "bye");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }

    /// Is the child still alive?
    pub fn is_alive(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

/// The warm worker pool: pre-spawned sandboxed workers, assigned on demand.
pub struct WorkerPool {
    binary: String,
    pub workers: Vec<SandboxedWorker>,
    next_id: usize,
    next_assign: usize,
}

impl WorkerPool {
    /// Pre-spawn `size` warm workers under the given profile.
    pub fn spawn(
        binary: &str,
        profile: &str,
        scratch: Option<&str>,
        size: usize,
    ) -> std::io::Result<WorkerPool> {
        let mut workers = Vec::with_capacity(size);
        for i in 0..size {
            workers.push(SandboxedWorker::spawn(binary, i, profile, scratch)?);
        }
        Ok(WorkerPool {
            binary: binary.to_string(),
            workers,
            next_id: size,
            next_assign: 0,
        })
    }

    pub fn size(&self) -> usize {
        self.workers.len()
    }

    /// Assign the next free worker to a job (round-robin). Spawns a
    /// replacement if a worker died since last check. `None` = all busy.
    pub fn acquire(
        &mut self,
        profile: &str,
        scratch: Option<&str>,
    ) -> std::io::Result<Option<&mut SandboxedWorker>> {
        // Reap dead workers and refill the slot lazily.
        let mut i = 0;
        while i < self.workers.len() {
            if !self.workers[i].is_alive() {
                let id = self.next_id;
                self.next_id += 1;
                let replacement = SandboxedWorker::spawn(&self.binary, id, profile, scratch)?;
                self.workers[i] = replacement;
            }
            i += 1;
        }
        for _ in 0..self.workers.len() {
            let idx = self.next_assign % self.workers.len();
            self.next_assign += 1;
            if !self.workers[idx].busy {
                self.workers[idx].busy = true;
                return Ok(Some(&mut self.workers[idx]));
            }
        }
        Ok(None)
    }

    /// Release a worker back to the pool.
    pub fn release(&mut self, id: usize) {
        if let Some(w) = self.workers.iter_mut().find(|w| w.id == id) {
            w.busy = false;
        }
    }

    /// Resize the pool (spawn `additional` more warm workers).
    pub fn grow(
        &mut self,
        profile: &str,
        scratch: Option<&str>,
        additional: usize,
    ) -> std::io::Result<()> {
        for _ in 0..additional {
            let id = self.next_id;
            self.next_id += 1;
            self.workers
                .push(SandboxedWorker::spawn(&self.binary, id, profile, scratch)?);
        }
        Ok(())
    }

    /// Shut every worker down.
    pub fn shutdown(&mut self) {
        for w in &mut self.workers {
            w.shutdown();
        }
    }
}
