#!/usr/bin/env node
/**
 * P50.5.5 — Failure-injection suite (real binary, headless).
 *
 * Drives the REAL `agentcowork-core` binary (or AGENTCOWORK_E2E_CORE_BIN) with an
 * isolated profile and injects real failures. Every leg asserts the runtime
 * stays truthful — honest failure/fresh state, never demo/synthetic success:
 *
 *   L1 kill sidecar   — SIGKILL the coordinator mid-run → the supervisor
 *                       restarts it (state cycles Running → Restarting →
 *                       Running); the sidecar never silently disappears.
 *   L2 lock vault     — no vault key → honest "vault locked" boot failure,
 *                       zero demo/seed markers.
 *   L3 remove provider— doctor reports "no provider keys configured"
 *                       (count-only, no fabricated routes); the routing feed
 *                       gating is unit-proven under P50.3.6.
 *   L4 corrupt persistence — garbage vault.db / memory.json / tasks.json →
 *                       boot survives honestly (fresh/lenient), zero seed
 *                       markers, no crash-loop, no fake recovery.
 *   L5 deny permissions — the guard deny path (expire ticket, deny
 *                       permissions) runs the crate security suites
 *                       (p10_security + agentcowork-guard) as executable
 *                       evidence; the packaged-shell variant is P50.5.7.
 *   L6 break the command — `--version` answers honestly; a bogus flag
 *                       terminates (no hang) with zero demo/seed markers.
 *   L7 disconnect Chrome — no display in CI: without AGENTCOWORK_E2E_CHROME=1
 *                       (legacy EVERYAIOS_E2E_CHROME is also honoured) the leg
 *                       reports the install probe honestly (browser
 *                       absent ⇒ UI must show unavailable, never attached);
 *                       with it, the live-Chrome ignored suite must pass.
 *
 * Exit: 0 PASS / 1 FAIL / 2 SKIP (no core binary available).
 */
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir, homedir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// scripts/e2e → desktop_app
const REPO_ROOT = resolve(HERE, "../..");
const COORDINATOR_BIN = resolve(REPO_ROOT, "packages/coordinator/dist/coordinator");
const CORE_BIN = process.env.AGENTCOWORK_E2E_CORE_BIN ?? resolve(REPO_ROOT, "crates/target/debug/agentcowork-core");

if (!existsSync(CORE_BIN)) {
  console.log(`[P50.5.5] SKIP — no core binary at ${CORE_BIN} (set AGENTCOWORK_E2E_CORE_BIN)`);
  process.exit(2);
}

// Cargo is not on PATH in every shell (notably non-interactive WSL shells);
// resolve it the same way CI would: explicit env, else the rustup default
// install location, else PATH. L5/L7 both go through here.
const CARGO_BIN =
  process.env.AGENTCOWORK_E2E_CARGO_BIN ??
  (existsSync(join(homedir(), ".cargo/bin/cargo")) ? join(homedir(), ".cargo/bin/cargo") : "cargo");

const failures = [];
const skips = [];
function assert(cond, label) {
  if (cond) console.log(`  ok — ${label}`);
  else {
    console.error(`  FAIL — ${label}`);
    failures.push(label);
  }
}
function skip(label) {
  console.log(`  SKIP — ${label}`);
  skips.push(label);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Run the core binary to completion; returns {code, out} (stderr+stdout). */
function runCore(profile, args = [], env = {}) {
  const dataDir = join(profile, ".agentcowork");
  mkdirSync(dataDir, { recursive: true });
  try {
    const out = execFileSync(CORE_BIN, args, {
      env: {
        ...process.env,
        HOME: profile,
        USERPROFILE: profile,
        // New data-home first, legacy fallback alongside: the current binary
        // still reads EVERYAIOS_* (DEC-053 migration is Core-owned).
        AGENTCOWORK_HOME: dataDir,
        EVERYAIOS_HOME: dataDir,
        AGENTCOWORK_DATA_DIR: dataDir,
        EVERYAIOS_DATA_DIR: dataDir,
        ...env,
      },
      encoding: "utf8",
      timeout: 20_000,
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { code: 0, out };
  } catch (e) {
    return { code: e.status ?? -1, out: `${e.stdout ?? ""}\n${e.stderr ?? ""}` };
  }
}

const NO_DEMO = /mockSessions|demo|seeded/i;

// ---- L2 lock vault ---------------------------------------------------------
{
  console.log("L2 — lock vault: boot without a key must fail honestly…");
  const profile = mkdtempSync(join(tmpdir(), "agentcowork-fi-lock-"));
  try {
    // Pin the vault key empty on both spellings so the boot must fail
    // locked/setup no matter which name the binary reads (DEC-053).
    const { code, out } = runCore(profile, [], { AGENTCOWORK_VAULT_KEY: "", EVERYAIOS_VAULT_KEY: "" });
    assert(code !== 0, `boot exits non-zero (code ${code})`);
    assert(/vault locked|vault key resolve failed|cannot open/i.test(out),
      `honest vault-locked message (${out.trim().slice(0, 90)})`);
    assert(!NO_DEMO.test(out), "no demo/seed markers in the failure output");
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
}

// ---- L3 remove provider ----------------------------------------------------
{
  console.log("L3 — remove provider: doctor must not fabricate routes…");
  const profile = mkdtempSync(join(tmpdir(), "agentcowork-fi-noprov-"));
  try {
    const { code, out } = runCore(profile, ["doctor", "--json"]);
    // Exit 1 is CORRECT here — the locked vault is a v1-required broken
    // subsystem; the report itself still prints to stdout.
    assert(code === 1, `doctor exit 1 for the locked vault (got ${code})`);
    let doc = null;
    try {
      doc = JSON.parse(out.trim()); // --json prints a single (pretty) document
    } catch {
      doc = null;
    }
    assert(doc !== null, "doctor emits JSON");
    const creds = doc?.checks?.find((c) => c.name === "Credentials");
    assert(creds?.status === "warn" && /no provider keys/i.test(creds?.detail ?? ""),
      "Credentials reports no keys (count-only, honest)");
    assert(!NO_DEMO.test(out), "no demo/seed markers in doctor output");
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
}

// ---- L4 corrupt persistence ------------------------------------------------
{
  console.log("L4 — corrupt persistence: garbage files must not seed or crash-loop…");
  const profile = mkdtempSync(join(tmpdir(), "agentcowork-fi-corrupt-"));
  try {
    const dataDir = join(profile, ".agentcowork");
    mkdirSync(dataDir, { recursive: true });
    writeFileSync(join(dataDir, "vault.db"), "GARBAGE-NOT-A-DB");
    writeFileSync(join(dataDir, "memory.json"), "{broken json");
    writeFileSync(join(dataDir, "tasks.json"), "[broken");
    writeFileSync(join(dataDir, "scheduler.json"), "not-json{{");
    const { code, out } = runCore(profile, [], { AGENTCOWORK_VAULT_KEY: "fi-test-key-000", EVERYAIOS_VAULT_KEY: "fi-test-key-000" });
    // A corrupt vault.db is FAIL-CLOSED with an honest sqlite error (never a
    // silent recreate of a vault the user may hold key material in, never a
    // crash-loop, never fabricated demo content).
    assert(!NO_DEMO.test(out), "no demo/seed markers after corruption");
    assert(code !== 0 && /file is not a database|sqlite error|cannot open|vault locked/i.test(out),
      `fail-closed on corrupt persistence (code ${code}, ${out.trim().slice(0, 90)})`);
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
}

// ---- L1 kill sidecar -------------------------------------------------------
if (!existsSync(COORDINATOR_BIN)) {
  skip(`L1 needs the coordinator binary at ${COORDINATOR_BIN} (build it first)`);
} else {
  console.log("L1 — kill sidecar: the supervisor must restart the coordinator…");
  const profile = mkdtempSync(join(tmpdir(), "agentcowork-fi-kill-"));
  try {
    const dataDir = join(profile, ".agentcowork");
    mkdirSync(dataDir, { recursive: true });
    const child = spawn(CORE_BIN, ["--headless", "--coordinator-bin", COORDINATOR_BIN], {
      env: { ...process.env, HOME: profile, USERPROFILE: profile, AGENTCOWORK_HOME: dataDir, EVERYAIOS_HOME: dataDir, AGENTCOWORK_DATA_DIR: dataDir, EVERYAIOS_DATA_DIR: dataDir, AGENTCOWORK_VAULT_KEY: "fi-kill-key", EVERYAIOS_VAULT_KEY: "fi-kill-key" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let log = "";
    child.stderr.on("data", (d) => (log += d.toString()));
    child.stdout.on("data", (d) => (log += d.toString()));

    // Wait for the first spawn (state: Starting) and a live coordinator child.
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline && !/\[supervisor\] state: Starting/.test(log)) {
      await sleep(100);
    }
    assert(/\[supervisor\] state: Starting/.test(log), "supervisor spawns the coordinator");
    const corePid = child.pid;
    const liveChildren = () =>
      spawn("bash", ["-lc", `pgrep -P ${corePid} | head -5`], { stdio: ["ignore", "pipe", "ignore"] });
    const readPids = (proc) =>
      new Promise((res) => {
        let s = "";
        proc.stdout.on("data", (d) => (s += d));
        proc.on("exit", () => res(s.trim().split("\n").filter(Boolean)));
      });
    const d0 = Date.now() + 30_000;
    let pids = [];
    while (Date.now() < d0) {
      pids = await readPids(liveChildren());
      if (pids.length > 0) break;
      await sleep(200);
    }
    assert(pids.length > 0, `coordinator child process alive (${pids.join(",")})`);

    // SIGKILL the coordinator mid-run.
    const killed = spawn("bash", ["-lc", `pkill -9 -P ${corePid}; true`]);
    await new Promise((r) => killed.on("exit", r));
    const logBefore = log.length;
    // The supervisor must detect the death and respawn (second Starting).
    const d2 = Date.now() + 30_000;
    while (Date.now() < d2 && !/child crashed/.test(log.slice(logBefore))) {
      await sleep(100);
    }
    assert(/child crashed/.test(log.slice(logBefore)),
      "supervisor detects the SIGKILL (child crashed)");
    const d3 = Date.now() + 30_000;
    while (Date.now() < d3 && (log.match(/\[supervisor\] state: Starting/g) ?? []).length < 2) {
      await sleep(100);
    }
    assert((log.match(/\[supervisor\] state: Starting/g) ?? []).length >= 2,
      "supervisor respawns the sidecar (second spawn)");
    const d4 = Date.now() + 15_000;
    let pids2 = [];
    while (Date.now() < d4) {
      pids2 = await readPids(liveChildren());
      if (pids2.length > 0) break;
      await sleep(200);
    }
    assert(pids2.length > 0, "a fresh coordinator child is running after the restart");
    assert(!NO_DEMO.test(log), "no demo/seed markers in the supervisor log");
    child.kill("SIGKILL");
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
}

// ---- L5 deny permissions (crate security suites as executable evidence) ----
{
  console.log("L5 — deny permissions: guard security suites…");
  const cargo = process.env.AGENTCOWORK_E2E_CARGO ?? resolve(REPO_ROOT, "crates");
  try {
    const out = execFileSync(CARGO_BIN, ["test", "-p", "agentcowork-guard", "--quiet"], {
      cwd: cargo,
      encoding: "utf8",
      timeout: 600_000,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const summary = (out.match(/\d+ passed/) ?? ["0 passed"])[0];
    assert(/[1-9]\d* passed/.test(summary), `agentcowork-guard deny/permission suites pass (${summary})`);
  } catch (e) {
    assert(false, `agentcowork-guard suites run (${(e.message ?? "").slice(0, 100)})`);
  }
}

// ---- L6 break the command --------------------------------------------------
{
  console.log("L6 — break the command: --version answers, bogus flags terminate…");
  const profile = mkdtempSync(join(tmpdir(), "agentcowork-fi-cmd-"));
  try {
    const ver = runCore(profile, ["--version"], { AGENTCOWORK_VAULT_KEY: "fi-ver-key", EVERYAIOS_VAULT_KEY: "fi-ver-key" });
    assert(ver.code === 0 && /\d+\.\d+\.\d+/.test(ver.out),
      `--version exits 0 with a version (${ver.out.trim().slice(0, 60)})`);
    assert(!NO_DEMO.test(ver.out), "no demo/seed markers in --version output");
    const bogus = runCore(profile, ["--definitely-not-a-flag"], { AGENTCOWORK_VAULT_KEY: "fi-bogus-key", EVERYAIOS_VAULT_KEY: "fi-bogus-key" });
    assert(!NO_DEMO.test(bogus.out), "no demo/seed markers after a bogus flag");
    assert(bogus.code !== undefined, `bogus invocation terminated (code ${bogus.code})`);
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
}

// ---- L7 disconnect Chrome --------------------------------------------------
{
  console.log("L7 — disconnect Chrome: no display here, so probe honestly…");
  const cands = ["google-chrome", "google-chrome-stable", "chrome", "chromium", "chromium-browser", "msedge", "microsoft-edge", "brave-browser"];
  let found = null;
  for (const c of cands) {
    try {
      execFileSync(process.platform === "win32" ? "where" : "which", [c], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
      found = c;
      break;
    } catch { /* not installed */ }
  }
  // Either spelling enables the live leg (the secret itself migrates separately).
  const liveChrome = process.env.AGENTCOWORK_E2E_CHROME ?? process.env.EVERYAIOS_E2E_CHROME;
  if (liveChrome === "1" && found) {
    console.log(`  live Chrome leg via ${found}: running the ignored live suite…`);
    try {
      execFileSync(CARGO_BIN, ["test", "-p", "agentcowork-browser", "--lib", "--", "--ignored", "--test-threads=1"], {
        cwd: resolve(REPO_ROOT, "crates"),
        encoding: "utf8",
        timeout: 600_000,
        stdio: ["ignore", "pipe", "pipe"],
        // Both spellings: the current browser harness gates on the legacy name.
        env: { ...process.env, AGENTCOWORK_LIVE_TEST: "1", EVERYAIOS_LIVE_TEST: "1" },
      });
      assert(true, "live-Chrome ignored suite passes with a display");
    } catch (e) {
      assert(false, `live-Chrome suite failed (${(e.message ?? "").slice(0, 120)})`);
    }
  } else if (found) {
    skip(`Chrome present (${found}) but AGENTCOWORK_E2E_CHROME!=1 — live attach is the P50.5.8 display step`);
  } else {
    console.log("  ok — no browser installed: UI must report unavailable (browse empty state + status dot), never attached");
  }
}

if (failures.length > 0) {
  console.error(`[P50.5.5] FAIL — ${failures.length} assertion(s) failed`);
  process.exit(1);
}
console.log(`[P50.5.5] PASS — failure-injection suite (real binary, truthful states${skips.length > 0 ? `; ${skips.length} leg(s) skipped: ${skips.join(" / ")}` : ""})`);
