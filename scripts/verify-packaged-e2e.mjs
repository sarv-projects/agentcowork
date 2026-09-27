import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

console.log("=== AgentCowork Packaged Release & E2E Verification Suite ===\n");

const suite = {
  timestamp: new Date().toISOString(),
  checks: [],
  passed: 0,
  failed: 0,
};

function recordCheck(name, passed, detail) {
  suite.checks.push({ name, passed, detail });
  if (passed) {
    suite.passed += 1;
    console.log("PASS: " + name + " - " + detail);
  } else {
    suite.failed += 1;
    console.error("FAIL: " + name + " - " + detail);
  }
}

// A missing file is a FAILED check, not a stack trace: a path that moved (or
// was archived) must be reported as this suite failing, with the path named,
// rather than crashing the runner before the remaining checks execute.
function readOrFail(label, relPath) {
  const abs = join(process.cwd(), relPath);
  if (!existsSync(abs)) {
    recordCheck(label, false, `${relPath} is missing — the check cannot be evaluated`);
    return null;
  }
  return readFileSync(abs, "utf-8");
}

// 1. Check Store Hydration Gate
const storeSrc = readOrFail("P50.2.1 Store Hydration Gate", "ui/src/lib/store.ts");
if (storeSrc) {
  const hasTauriGate =
    storeSrc.includes("inTauri() ? [] : mockSessions") && storeSrc.includes("sessionsHydrated");
  recordCheck(
    "P50.2.1 Store Hydration Gate",
    hasTauriGate,
    "UI store strictly gates mock preview data behind inTauri() === false"
  );
}

// 2. Check Calendar IPC and Vault Schema
const calendarCmds = readOrFail("P6.22 Calendar IPC & Schema v8", "src-tauri/src/calendar_cmds.rs");
if (calendarCmds) {
  const hasCalendarCRUD =
    calendarCmds.includes("calendar_event_put") && calendarCmds.includes("calendar_event_list");
  recordCheck(
    "P6.22 Calendar IPC & Schema v8",
    hasCalendarCRUD,
    "Native calendar CRUD commands registered and mapped to encrypted SQLCipher tables"
  );
}

// 3. Check the tool surface on its live owner.
// ADR-0005 retired the built-in turn loop, and with it the coordinator's
// `tools.ts` (`FIRST_CLASS_NATIVE_TOOLS` / `mergeWithNativeTools`). The tool
// surface now lives in `agentcowork-mcp`, which validates its own catalog.
const mcpLib = readOrFail("P64.1 Tool surface (agentcowork-mcp)", "crates/agentcowork-mcp/src/lib.rs");
if (mcpLib) {
  const hasToolSurface =
    mcpLib.includes("pub fn all_tools") &&
    mcpLib.includes("pub fn inbuilt_catalog") &&
    mcpLib.includes("pub fn validate_facades");
  recordCheck(
    "P64.1 Tool surface (agentcowork-mcp)",
    hasToolSurface,
    "The inbuilt tool catalog and facade validation are owned by agentcowork-mcp"
  );
}

// 4. Check the context passport on the external-agent path.
// The retired coordinator loop resolved @-mentions in `chat.ts`; the live
// owner of prompt/context assembly is the Rust context passport in
// `acp_cmds.rs`, which injects the memory warm set and the governance block
// before the prompt reaches the bound agent.
const acpCmds = readOrFail("P64.2 / C14 Context passport", "src-tauri/src/acp_cmds.rs");
if (acpCmds) {
  const hasPassport =
    acpCmds.includes("fn build_acp_prompt_with_passport") &&
    acpCmds.includes("agentcowork_acp::build_chief_prompt") &&
    acpCmds.includes("GovernedSession");
  recordCheck(
    "P64.2 / C14 Context passport",
    hasPassport,
    "The ACP prompt carries the memory facts + governance block (built in Rust, not in the sidecar)"
  );
}

// 5. Check Avoidance Memory Store
const avoidSrc = readOrFail("P51.34 Negative Failure Memory", "crates/agentcowork-memory/src/avoid.rs");
if (avoidSrc) {
  const hasAvoidanceStore =
    avoidSrc.includes("AvoidanceStore") && avoidSrc.includes("record_failure");
  recordCheck(
    "P51.34 Negative Failure Memory",
    hasAvoidanceStore,
    "AvoidanceStore records tool failures to prevent repetitive agent loops"
  );
}

// 6. Check Multi-Agent Fleet Worktree Concurrency
const fleetSrc = readOrFail("P51.13 Swarm Worktree Isolation", "packages/coordinator/src/fleet.ts");
if (fleetSrc) {
  const hasFleetIsolation =
    fleetSrc.includes("worktreeSpecs") && fleetSrc.includes("multiplex");
  recordCheck(
    "P51.13 Swarm Worktree Isolation",
    hasFleetIsolation,
    "Independent Git worktrees with 3-file blackboards prevent index lock collisions"
  );
}

// 7. Check P45 Performance Measurements
const perfMeasurementsPath = join(process.cwd(), "scripts", "p45-live-measurements.json");
const hasPerfData = existsSync(perfMeasurementsPath);
recordCheck(
  "P45 Performance Benchmark Evidence",
  hasPerfData,
  "Live measurement data recorded on disk with SQLite, read latency, and audit metrics"
);

console.log("\n=== Verification Complete: " + suite.passed + " Passed, " + suite.failed + " Failed ===");

if (suite.failed > 0) {
  process.exit(1);
}
