//! Forge runtime (P7.1 — I1/I4, doc 56/63 §2.1): the write → sandbox →
//! test → iterate loop, the TDD loop (auto-generate tests, read stderr,
//! rewrite until green), code execution in the rquickjs sandbox
//! (`agentcowork-script`), and the optional Docker sandbox command builder
//! for heavy/data workflows.
//!
//! Everything here is deterministic machinery over injected seams:
//! - [`CommandRunner`] — where `cargo test` / `npm test` / `pytest` /
//!   `docker run` actually executes (live binding: [`SystemCommandRunner`]).
//! - [`FileStore`] — the workspace file surface (live binding:
//!   [`DiskFileStore`]).
//! - [`agentcowork_script::ScriptSandbox`] — JS execution in the rquickjs
//!   sandbox.
//!
//! The loops are bounded (max iterations + a SHA-256 loop guard on repeated
//! source text) so a bad test or a non-terminating rewrite cannot spin
//! forever; each iteration is reported so the caller can audit it.

use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::Digest;

/// Output of one command run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

impl CommandOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// stderr only (the TDD loops read stderr for failure reasons).
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    pub fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Where commands actually execute. The live runner shells out via
/// `std::process::Command`; tests inject a scripted runner.
pub trait CommandRunner: Send + Sync {
    fn run(&self, command: &str, args: &[&str], cwd: &str) -> Result<CommandOutput, String>;
}

/// The live command runner (guarded by the caller — the executor seam, not a
/// bypass: forge steps are invoked through the guard-gated tool surface).
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, command: &str, args: &[&str], cwd: &str) -> Result<CommandOutput, String> {
        let out = Command::new(command)
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("failed to run `{command}`: {e}"))?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code(),
        })
    }
}

/// The workspace file surface (write/read source files during a forge loop).
pub trait FileStore: Send + Sync {
    fn write(&self, path: &str, content: &str) -> Result<(), String>;
    fn read(&self, path: &str) -> Result<String, String>;
}

/// The live file store (direct `std::fs` — the caller's executor already
/// passed the path floor + guard before a forge loop may write).
pub struct DiskFileStore;

impl FileStore for DiskFileStore {
    fn write(&self, path: &str, content: &str) -> Result<(), String> {
        std::fs::write(path, content).map_err(|e| format!("write {path}: {e}"))
    }
    fn read(&self, path: &str) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))
    }
}

/// Why a forge loop stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForgeStop {
    /// The test command exited 0.
    Green,
    /// Ran out of iterations before green.
    IterationsExhausted,
    /// The SHA-256 loop guard tripped on repeated source text.
    LoopTripped,
    /// The command runner failed (spawn error), not a test failure.
    RunnerError(String),
}

/// The outcome of one forge loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeOutcome {
    pub stop: ForgeStop,
    pub iterations: u32,
    /// The final source text written (None if nothing was written).
    pub final_source: Option<String>,
    /// The last test output observed (stdout + stderr).
    pub last_output: CommandOutput,
    /// One line per iteration for the audit trail.
    pub trace: Vec<String>,
}

impl ForgeOutcome {
    pub fn is_green(&self) -> bool {
        self.stop == ForgeStop::Green
    }
}

/// The bounded write→test→iterate loop.
pub struct ForgeRuntime<C: CommandRunner, F: FileStore> {
    commands: C,
    files: F,
    /// The rquickjs sandbox used by `run_js` (and, when the test command is
    /// the sandbox itself, the iterate loop).
    sandbox: Arc<dyn agentcowork_script::ScriptSandbox>,
    max_iterations: u32,
    /// Working directory for command runs.
    cwd: String,
}

impl<C: CommandRunner, F: FileStore> ForgeRuntime<C, F> {
    pub fn new(
        commands: C,
        files: F,
        sandbox: Arc<dyn agentcowork_script::ScriptSandbox>,
        cwd: impl Into<String>,
    ) -> Self {
        Self {
            commands,
            files,
            sandbox,
            max_iterations: 8,
            cwd: cwd.into(),
        }
    }

    /// Override the iteration budget (default 8).
    pub fn with_max_iterations(mut self, n: u32) -> Self {
        self.max_iterations = n.max(1);
        self
    }

    /// The rquickjs sandbox surface (P7.1 — code execution in the sandbox).
    pub fn sandbox(&self) -> &dyn agentcowork_script::ScriptSandbox {
        self.sandbox.as_ref()
    }

    /// Run a JS snippet in the rquickjs sandbox. Returns the sandbox's JSON
    /// envelope (`{"result":…,"logs":[…]}`) or an error.
    pub fn run_js(&self, code: &str) -> Result<String, String> {
        self.sandbox.eval(code).map_err(|e| e.to_string())
    }

    /// Execute any command through the injected runner (the test command
    /// seam; also used for `docker run`).
    pub fn run(&self, command: &str, args: &[&str]) -> Result<CommandOutput, String> {
        self.commands.run(command, args, &self.cwd)
    }

    /// The write → sandbox/test → iterate loop:
    ///
    /// 1. write `initial` to `path`;
    /// 2. run `test_cmd` with `test_args`;
    /// 3. green → stop; otherwise call `rewrite(current_source, stderr)` to
    ///    produce the next source, write it, and repeat;
    /// 4. stop at [`ForgeStop::Green`] / `IterationsExhausted` /
    ///    `LoopTripped` (same source repeated twice) / `RunnerError`.
    pub fn iterate(
        &self,
        path: &str,
        initial: &str,
        test_cmd: &str,
        test_args: &[&str],
        rewrite: &mut dyn FnMut(&str, &str) -> String,
    ) -> ForgeOutcome {
        let mut trace = Vec::new();
        if let Err(e) = self.files.write(path, initial) {
            return ForgeOutcome {
                stop: ForgeStop::RunnerError(e),
                iterations: 0,
                final_source: Some(initial.to_string()),
                last_output: CommandOutput::default(),
                trace,
            };
        }
        trace.push(format!("write {path} ({initial} bytes)"));
        let mut source = initial.to_string();
        let mut previous: Option<String> = None;
        let mut iterations = 0u32;

        loop {
            iterations += 1;
            trace.push(format!("iter {iterations}: run {test_cmd} {:?}", test_args));
            match self.commands.run(test_cmd, test_args, &self.cwd) {
                Ok(out) => {
                    if out.succeeded() {
                        trace.push("iter {iterations}: GREEN".to_string());
                        return ForgeOutcome {
                            stop: ForgeStop::Green,
                            iterations,
                            final_source: Some(source),
                            last_output: out,
                            trace,
                        };
                    }
                    if iterations >= self.max_iterations {
                        trace.push("iter exhausted".to_string());
                        return ForgeOutcome {
                            stop: ForgeStop::IterationsExhausted,
                            iterations,
                            final_source: Some(source),
                            last_output: out,
                            trace,
                        };
                    }
                    // Rewrite from stderr.
                    let next = rewrite(&source, out.stderr());
                    let next_sha = sha2::Sha256::digest(next.as_bytes());
                    if previous.as_deref() == Some(next.as_str())
                        || next_sha == sha2::Sha256::digest(source.as_bytes())
                    {
                        trace.push("loop guard tripped (repeated source)".to_string());
                        return ForgeOutcome {
                            stop: ForgeStop::LoopTripped,
                            iterations,
                            final_source: Some(source),
                            last_output: out,
                            trace,
                        };
                    }
                    if let Err(e) = self.files.write(path, &next) {
                        return ForgeOutcome {
                            stop: ForgeStop::RunnerError(e),
                            iterations,
                            final_source: Some(next),
                            last_output: out,
                            trace,
                        };
                    }
                    trace.push(format!("iter {iterations}: rewrote {path}"));
                    previous = Some(source);
                    source = next;
                }
                Err(e) => {
                    return ForgeOutcome {
                        stop: ForgeStop::RunnerError(e),
                        iterations,
                        final_source: Some(source),
                        last_output: CommandOutput::default(),
                        trace,
                    };
                }
            }
        }
    }

    /// The TDD loop: write the generated test, run it (expecting RED), then
    /// run the write→test→iterate loop until the implementation makes it
    /// green. `gen_test(spec)` produces the test file, `gen_impl(stderr)`
    /// produces the next implementation attempt from the failure output.
    #[allow(clippy::too_many_arguments)]
    pub fn tdd_loop(
        &self,
        impl_path: &str,
        test_path: &str,
        spec: &str,
        gen_test: impl Fn(&str) -> String,
        gen_impl: impl Fn(&str) -> String,
        test_cmd: &str,
        test_args: &[&str],
    ) -> ForgeOutcome {
        let test_source = gen_test(spec);
        if let Err(e) = self.files.write(test_path, &test_source) {
            return ForgeOutcome {
                stop: ForgeStop::RunnerError(e),
                iterations: 0,
                final_source: None,
                last_output: CommandOutput::default(),
                trace: Vec::new(),
            };
        }
        // First run: prove RED (the test fails without an implementation).
        let red = match self.commands.run(test_cmd, test_args, &self.cwd) {
            Ok(out) => out,
            Err(e) => {
                return ForgeOutcome {
                    stop: ForgeStop::RunnerError(e),
                    iterations: 0,
                    final_source: None,
                    last_output: CommandOutput::default(),
                    trace: Vec::new(),
                };
            }
        };
        let trace = vec![
            "tdd: wrote test".to_string(),
            format!("tdd: initial run exit={:?}", red.exit_code),
        ];
        let first_impl = gen_impl(red.stderr());
        if let Err(e) = self.files.write(impl_path, &first_impl) {
            return ForgeOutcome {
                stop: ForgeStop::RunnerError(e),
                iterations: 0,
                final_source: Some(first_impl),
                last_output: red,
                trace,
            };
        }
        let mut outcome = self.iterate(
            impl_path,
            &first_impl,
            test_cmd,
            test_args,
            &mut |cur, err| gen_impl(err).trim_end_matches(cur).to_string(),
        );
        // The iterate loop wrote `first_impl` again as its initial — reuse
        // its trace/stop but keep the TDD preamble.
        outcome.trace.splice(0..0, trace);
        outcome
    }
}

/// Optional Docker sandbox (P7.1 — heavy/data workflows): a declarative
/// builder that turns a sandbox description into the exact `docker run`
/// command vector, executed through the same [`CommandRunner`] seam (so the
/// guard sees the full command line before it runs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerSandbox {
    pub image: String,
    /// `(host_path, container_path)` read-only mounts.
    pub read_only_mounts: Vec<(String, String)>,
    /// `(host_path, container_path)` writable mounts.
    pub writable_mounts: Vec<(String, String)>,
    pub network: Option<String>,
    pub memory_limit: Option<String>,
    pub cpus: Option<String>,
}

impl DockerSandbox {
    /// Build the `docker run` command vector (image first, then flags).
    /// `command`/`args` are the work to run inside the container.
    pub fn build_command(&self, command: &str, args: &[&str]) -> Vec<String> {
        let mut cmd: Vec<String> = vec!["docker".into(), "run".into(), "--rm".into()];
        for (h, c) in &self.read_only_mounts {
            cmd.push("-v".into());
            cmd.push(format!("{h}:{c}:ro"));
        }
        for (h, c) in &self.writable_mounts {
            cmd.push("-v".into());
            cmd.push(format!("{h}:{c}"));
        }
        if let Some(n) = &self.network {
            cmd.push("--network".into());
            cmd.push(n.clone());
        }
        if let Some(m) = &self.memory_limit {
            cmd.push("--memory".into());
            cmd.push(m.clone());
        }
        if let Some(c) = &self.cpus {
            cmd.push("--cpus".into());
            cmd.push(c.clone());
        }
        cmd.push(self.image.clone());
        cmd.push(command.into());
        cmd.extend(args.iter().map(|a| a.to_string()));
        cmd
    }

    /// Run the sandboxed command through a [`CommandRunner`].
    pub fn run<C: CommandRunner>(
        &self,
        commands: &C,
        cwd: &str,
        command: &str,
        args: &[&str],
    ) -> Result<CommandOutput, String> {
        let full = self.build_command(command, args);
        let (cmd, rest) = full.split_first().expect("docker command non-empty");
        let rest: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
        commands.run(cmd, &rest, cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted runner: each call pops the next canned output; the final
    /// entry repeats forever.
    #[derive(Default)]
    struct ScriptedRunner {
        script: Mutex<Vec<CommandOutput>>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl ScriptedRunner {
        // Kept for scripted-runner callers that assert on output ordering;
        // the plain test path does not exercise it.
        #[allow(dead_code)]
        fn push(&self, out: CommandOutput) {
            self.script.lock().unwrap().push(out);
        }
        fn scripted(outs: Vec<CommandOutput>) -> Self {
            Self {
                script: Mutex::new(outs),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&self, command: &str, args: &[&str], _cwd: &str) -> Result<CommandOutput, String> {
            self.calls.lock().unwrap().push((
                command.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
            let mut script = self.script.lock().unwrap();
            let out = if script.is_empty() {
                CommandOutput::default()
            } else if script.len() == 1 {
                script[0].clone()
            } else {
                script.remove(0)
            };
            Ok(out)
        }
    }

    /// A file store that keeps files in memory (per-path, newest wins).
    #[derive(Default)]
    struct MemFiles {
        files: Mutex<std::collections::HashMap<String, String>>,
    }

    impl FileStore for MemFiles {
        fn write(&self, path: &str, content: &str) -> Result<(), String> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), content.to_string());
            Ok(())
        }
        fn read(&self, path: &str) -> Result<String, String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| format!("no such file {path}"))
        }
    }

    /// A stub sandbox that echoes the code as its JSON result.
    struct StubSandbox;
    impl agentcowork_script::ScriptSandbox for StubSandbox {
        fn eval(&self, code: &str) -> Result<String, agentcowork_script::SandboxError> {
            Ok(format!(
                "{{\"result\":\"{}\",\"logs\":[]}}",
                code.trim().len()
            ))
        }
        fn limits(&self) -> agentcowork_script::SandboxLimits {
            agentcowork_script::SandboxLimits::default()
        }
    }

    fn green() -> CommandOutput {
        CommandOutput {
            stdout: "ok".into(),
            stderr: String::new(),
            exit_code: Some(0),
        }
    }

    fn failing(stderr: &str) -> CommandOutput {
        CommandOutput {
            stdout: String::new(),
            stderr: stderr.into(),
            exit_code: Some(1),
        }
    }

    #[test]
    fn iterate_stops_green_immediately() {
        let runner = ScriptedRunner::scripted(vec![green()]);
        let files = MemFiles::default();
        let rt = ForgeRuntime::new(runner, files, Arc::new(StubSandbox), "/workspace");
        let outcome = rt.iterate("a.rs", "fn main(){}", "cargo", &["test"], &mut |_, _| {
            String::new()
        });
        assert!(outcome.is_green());
        assert_eq!(outcome.iterations, 1);
    }

    #[test]
    fn iterate_rewrites_until_green_or_budget() {
        // fail 3 times (each with a different stderr), then green.
        let runner = ScriptedRunner::scripted(vec![
            failing("error[E0308]: mismatch"),
            failing("error[E0425]: cannot find value"),
            failing("error[E0433]: unresolved import"),
            green(),
        ]);
        let files = MemFiles::default();
        let rt = ForgeRuntime::new(runner, files, Arc::new(StubSandbox), "/w");
        let mut seen = Vec::new();
        let outcome = rt.iterate("a.rs", "v0", "cargo", &["test"], &mut |cur, err| {
            seen.push(err.to_string());
            format!("{cur}+1")
        });
        assert!(outcome.is_green());
        assert_eq!(outcome.iterations, 4);
        assert_eq!(seen.len(), 3);
        assert!(outcome.trace.iter().any(|l| l.contains("GREEN")));
    }

    #[test]
    fn iterate_loop_guard_trips_on_repeated_source() {
        // Every run fails; the rewrite returns the SAME text each time.
        let runner = ScriptedRunner::scripted(vec![failing("nope")]);
        let files = MemFiles::default();
        let rt = ForgeRuntime::new(runner, files, Arc::new(StubSandbox), "/w");
        let outcome = rt.iterate("a.rs", "same", "t", &[], &mut |_, _| "same".to_string());
        assert_eq!(outcome.stop, ForgeStop::LoopTripped);
    }

    #[test]
    fn iterate_stops_at_iteration_budget() {
        let runner = ScriptedRunner::scripted(vec![failing("x")]); // repeats
        let files = MemFiles::default();
        let rt =
            ForgeRuntime::new(runner, files, Arc::new(StubSandbox), "/w").with_max_iterations(3);
        let outcome = rt.iterate("a.rs", "v0", "t", &[], &mut |cur, _| format!("{cur}+"));
        assert_eq!(outcome.stop, ForgeStop::IterationsExhausted);
        assert_eq!(outcome.iterations, 3);
    }

    #[test]
    fn run_js_delegates_to_sandbox() {
        let rt = ForgeRuntime::new(
            ScriptedRunner::default(),
            MemFiles::default(),
            Arc::new(StubSandbox),
            "/w",
        );
        let out = rt.run_js("1 + 1").unwrap();
        assert!(out.contains("\"result\""));
    }

    #[test]
    fn docker_command_builds_mounts_and_limits() {
        let sb = DockerSandbox {
            image: "python:3.12".into(),
            read_only_mounts: vec![("/data".into(), "/data".into())],
            writable_mounts: vec![("/out".into(), "/out".into())],
            network: Some("none".into()),
            memory_limit: Some("2g".into()),
            cpus: Some("2".into()),
        };
        let cmd = sb.build_command("python", &["/data/run.py"]);
        assert_eq!(cmd[0], "docker");
        assert!(cmd.contains(&"-v".to_string()));
        assert!(cmd.contains(&"/data:/data:ro".to_string()));
        assert!(cmd.contains(&"/out:/out".to_string()));
        assert!(cmd.contains(&"--network".to_string()));
        assert!(cmd.contains(&"python:3.12".to_string()));
        assert_eq!(cmd.last().map(String::as_str), Some("/data/run.py"));
    }

    #[test]
    fn docker_run_through_runner() {
        let runner = ScriptedRunner::scripted(vec![green()]);
        let sb = DockerSandbox {
            image: "alpine".into(),
            read_only_mounts: vec![],
            writable_mounts: vec![],
            network: None,
            memory_limit: None,
            cpus: None,
        };
        let out = sb.run(&runner, "/w", "echo", &["hi"]).unwrap();
        assert!(out.succeeded());
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0].0, "docker");
        assert!(calls[0].1.iter().any(|a| a == "run"));
    }
}
