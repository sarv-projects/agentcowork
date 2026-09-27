#!/usr/bin/env node
// AgentCowork architecture-invariant gate (P69.E).
//
// The repo has already paid for these regressions once (P69.C/P69.D). Each
// check below asserts a CORE invariant structurally, so a future edit cannot
// quietly re-introduce a second authority, a second schema, or a credential
// path outside the Rust vault. Cheap (no build), so it runs on every PR.
//
// Invariants enforced here:
//   CRED-1  no TypeScript module seals/unseals provider keys
//   CRED-2  no TS package takes a dependency on the crypto pack for custody
//   CRED-3  no TS module reads a provider secret out of `process.env`
//   AUTH-1  one auth-mode wire vocabulary (no `local_cli` / `*_cli` spellings)
//   AUTH-2  `AuthMode` is declared exactly once — in `agentcowork-types`
//   AUTH-3  the UI derives its auth type from the canonical union (no rewrite)
//   SCHEMA-1 the canonical schema objects exist in `agentcowork-types`
//   DECIDE-1 permission classification is not exported from `core-tools`
//   DECIDE-2 the ACP permission path never hardcodes `Approval::allow()`
//   E3-ACP-V2 an ACP v2 connection is refused, never silently downgraded
//   LAYER-1 `core-engine` is policies/helpers only — no transport, no egress
//   LAYER-2 `agentcowork-eval` stays outside the runtime (no production dep)
//   TS-DUP   TypeScript never re-declares a canonical record/id (P69.D15/D25)
//   RUST-DUP Rust declares each canonical primitive once (P69.B2)
//   LAYER-3  the TS search cascade is not wired into the turn loop (P69.D9)
//   LAYER-4  the coordinator orchestrates only — no privileged IO (P69.D22)
//   E3-CONNECTOR  Graph writes pass the read-first, single-use approval seam
//   E3-MCP        MCP transport auth + host ticketed tool/remote-call seams
//   E3-ACP        ACP permission decisions stay on the host Guard gate
//   E3-UI         Work commands reach the WorkGateway through the Tauri layer
//   E4-JOURNAL    one durable Work journal; no second events.jsonl owner
//   E4-SPAWN      scheduler/blueprint ownership cannot spawn execution
//   E4-WORK-CREATION  Work creation/delegation only enters through WorkGateway
//   PURITY-1 `agentcowork-ipc` is transport only (D26)
//   PURITY-3 `agentcowork-catalog` is metadata only — no vault/guard (D28)
//   PURITY-4 CDP is a backend under BrowserService, not a kernel dependency (D31)
//
// Usage: node scripts/check-arch-invariants.mjs

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

const SKIP_DIRS = new Set([
  "node_modules",
  "dist",
  "target",
  ".git",
  ".venv",
  ".code-intelligence",
  "build",
]);

/** Recursively collect files under `dir` whose extension is in `exts`. */
function walk(dir, exts, out = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (SKIP_DIRS.has(name)) continue;
    const full = join(dir, name);
    const st = statSync(full);
    if (st.isDirectory()) {
      walk(full, exts, out);
    } else if (exts.has(extname(name))) {
      out.push(full);
    }
  }
  return out;
}

const rel = (p) => relative(ROOT, p).split("\\").join("/");
const read = (p) => readFileSync(join(ROOT, p), "utf8");

const failures = [];
const fail = (id, file, detail) => failures.push({ id, file, detail });

/** For Rust files: everything before the in-file test module. */
function productionPart(src) {
  const idx = src.indexOf("#[cfg(test)]");
  return idx === -1 ? src : src.slice(0, idx);
}

/**
 * Fail when `pattern` matches `src` outside of a line comment.
 * Keeps prose ("no `getApiKey`") from tripping identifier checks.
 */
function matchesInCode(src, pattern) {
  const hits = [];
  for (const [i, line] of src.split("\n").entries()) {
    const code = line.replace(/^\s*(\/\/|\*|\/\*).*$/, "");
    if (pattern.test(code)) hits.push({ line: i + 1, text: line.trim() });
  }
  return hits;
}

/**
 * Remove comments while preserving line structure.  The E3/E4 gates are
 * deliberately structural: a prose mention of `events.jsonl`, `spawn`, or a
 * command name must not count as code, while a URL inside a string must not
 * be mistaken for a line comment.  This small lexer handles the Rust/TS
 * string forms used by the checked production surfaces without adding a
 * parser dependency.
 */
function stripComments(src) {
  const out = [];
  let state = "code";
  let quote = "";
  let blockDepth = 0;

  for (let i = 0; i < src.length; i += 1) {
    const c = src[i];
    const next = src[i + 1] ?? "";

    if (state === "line") {
      out.push(c === "\n" ? "\n" : " ");
      if (c === "\n") state = "code";
      continue;
    }

    if (state === "block") {
      if (c === "/" && next === "*") {
        out.push(" ", " ");
        blockDepth += 1;
        i += 1;
      } else if (c === "*" && next === "/") {
        out.push(" ", " ");
        blockDepth -= 1;
        i += 1;
        if (blockDepth === 0) state = "code";
      } else {
        out.push(c === "\n" ? "\n" : " ");
      }
      continue;
    }

    if (state === "string") {
      out.push(c);
      if (c === "\\" && i + 1 < src.length) {
        out.push(src[i + 1]);
        i += 1;
      } else if (c === quote) {
        state = "code";
      }
      continue;
    }

    // Rust raw strings (r"..." / r#"..."#) are copied verbatim so comment-like
    // bytes inside them cannot alter the source map used for diagnostics.
    if (c === "r") {
      const raw = /^r(#*)"/.exec(src.slice(i, i + 32));
      if (raw) {
        const terminator = `"${raw[1]}`;
        const end = src.indexOf(terminator, i + raw[0].length);
        const stop = end === -1 ? src.length : end + terminator.length;
        for (let j = i; j < stop; j += 1) out.push(src[j]);
        i = stop - 1;
        continue;
      }
    }

    if (c === "/" && next === "/") {
      out.push(" ", " ");
      i += 1;
      state = "line";
      continue;
    }
    if (c === "/" && next === "*") {
      out.push(" ", " ");
      blockDepth = 1;
      i += 1;
      state = "block";
      continue;
    }

    if (c === '"' || c === "'" || c === "`") {
      // Rust lifetimes (`'a`) are code, not the start of a character literal.
      if (c === "'" && /[A-Za-z_]/.test(next) && src[i + 2] !== "'") {
        out.push(c);
        continue;
      }
      quote = c;
      state = "string";
      out.push(c);
      continue;
    }

    out.push(c);
  }

  return out.join("");
}

/** Production Rust/TS source with comments and in-file test modules removed. */
function productionCode(src) {
  const code = stripComments(src);
  const testModule = /^\s*#\[cfg\(test\)\]/m.exec(code);
  return testModule ? code.slice(0, testModule.index) : code;
}

function lineAt(src, index) {
  return src.slice(0, Math.max(0, index)).split("\n").length;
}

function firstMatch(src, pattern) {
  const re = pattern instanceof RegExp ? pattern : new RegExp(pattern);
  re.lastIndex = 0;
  const m = re.exec(src);
  return m ? { index: m.index, text: m[0] } : null;
}

function requirePattern(id, file, code, pattern, detail) {
  const hit = firstMatch(code, pattern);
  if (!hit) {
    fail(id, file, `${detail} at line 1 (production source; expected structure not found)`);
    return null;
  }
  return hit;
}

/** Return the matching closing delimiter while ignoring quoted strings. */
function closingDelimiter(src, openIndex, open, close) {
  let depth = 0;
  let quote = "";
  for (let i = openIndex; i < src.length; i += 1) {
    const c = src[i];
    if (quote) {
      if (c === "\\") {
        i += 1;
      } else if (c === quote) {
        quote = "";
      }
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      // As in stripComments, do not treat a Rust lifetime as a quote.
      if (c === "'" && /[A-Za-z_]/.test(src[i + 1] ?? "") && src[i + 2] !== "'") continue;
      quote = c;
      continue;
    }
    if (c === open) depth += 1;
    else if (c === close && --depth === 0) return i;
  }
  return -1;
}

/** Find one Rust function and its balanced body in comment-stripped code. */
function findRustFn(src, name, fromIndex = 0) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`\\bfn\\s+${escaped}\\s*(?:<[^>{}]*>\\s*)?\\(`, "g");
  re.lastIndex = fromIndex;
  const match = re.exec(src);
  if (!match) return null;
  const paren = src.indexOf("(", match.index);
  const parenEnd = closingDelimiter(src, paren, "(", ")");
  if (parenEnd === -1) return null;
  const brace = src.indexOf("{", parenEnd);
  if (brace === -1) return null;
  const braceEnd = closingDelimiter(src, brace, "{", "}");
  if (braceEnd === -1) return null;
  return {
    start: match.index,
    bodyStart: brace + 1,
    bodyEnd: braceEnd,
    text: src.slice(brace + 1, braceEnd),
  };
}

function findRustFnContaining(src, name, needle, fromIndex = 0) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`\\bfn\\s+${escaped}\\s*(?:<[^>{}]*>\\s*)?\\(`, "g");
  re.lastIndex = fromIndex;
  let match;
  while ((match = re.exec(src)) !== null) {
    const fn = findRustFn(src, name, match.index);
    if (fn && fn.text.includes(needle)) return fn;
  }
  return null;
}

function productionRustFiles(roots) {
  const files = [];
  for (const root of roots) {
    for (const file of walk(join(ROOT, root), new Set([".rs"]))) {
      const name = rel(file);
      if (
        name.includes("/tests/") ||
        name.includes("/test/") ||
        name.includes("/fixtures/") ||
        /(?:^|[._\/-])(?:mock|fixture|acceptance|test)[^/]*\.rs$/i.test(name) ||
        /(?:\.test|_tests?)\.rs$/.test(name)
      ) {
        continue;
      }
      files.push(file);
    }
  }
  return files;
}

// --- CRED-1 / CRED-2: TS credential custody -------------------------------
const TS = new Set([".ts", ".tsx"]);
const PACKAGE_DIRS = join(ROOT, "packages");

for (const file of walk(PACKAGE_DIRS, TS)) {
  if (file.includes(`${join("packages", "")}core-tools`) && file.includes("node_modules")) continue;
  const src = readFileSync(file, "utf8");
  for (const [id, pattern] of [
    ["CRED-1", /\b(?:seal|unseal)ApiKey\s*\(/],
    ["CRED-1", /\bencryptSecret\s*\(\s*(?:apiKey|plaintextKey)/],
    ["CRED-1", /\bgetApiKey\s*\(/],
  ]) {
    const hits = matchesInCode(src, pattern);
    if (hits.length) fail(id, rel(file), `${pattern} at line ${hits[0].line}: ${hits[0].text}`);
  }
}

for (const file of walk(PACKAGE_DIRS, new Set([".json"]))) {
  const name = rel(file);
  if (!name.endsWith("package.json")) continue;
  if (name.startsWith("packages/core-security/")) continue;
  const pkg = JSON.parse(readFileSync(file, "utf8"));
  const deps = { ...(pkg.dependencies ?? {}), ...(pkg.devDependencies ?? {}) };
  if (name.startsWith("packages/core-providers/") && deps["@agentcowork/core-security"]) {
    fail(
      "CRED-2",
      name,
      "core-providers must not depend on core-security: custody belongs to the Rust vault (P69.C4/D4)",
    );
  }
}

// --- CRED-3: no secret read from the environment --------------------------
const SECRET_ENV = /process\.env(?:\.|\[['"]?)([A-Z0-9_]*(?:API_?KEY|TOKEN|SECRET|PASSWORD)[A-Z0-9_]*)/i;
for (const root of ["packages", "ui/src"]) {
  for (const file of walk(join(ROOT, root), TS)) {
    const name = rel(file);
    if (name.includes("__tests__")) continue;
    const src = readFileSync(file, "utf8");
    for (const hit of matchesInCode(src, SECRET_ENV)) {
      fail(
        "CRED-3",
        name,
        `secret read from the environment at line ${hit.line}: ${hit.text} — resolve it from agentcowork-vault through the host instead`,
      );
    }
  }
}

// --- AUTH-1 / AUTH-2: one auth vocabulary ---------------------------------
const SOURCE_EXTS = new Set([".ts", ".tsx", ".rs", ".mjs", ".js", ".json", ".yaml", ".yml"]);
const SCAN_ROOTS = ["crates", "packages", "src-tauri", "ui/src", "scripts"];

for (const root of SCAN_ROOTS) {
  for (const file of walk(join(ROOT, root), SOURCE_EXTS)) {
    const src = readFileSync(file, "utf8");
    if (file.endsWith("check-arch-invariants.mjs")) continue;
    // `local_cli` remains a legitimate *readiness* state (DESKTOP-APP-SPEC.md
    // §Settings), so only flag it where the line also speaks about auth — the
    // shape a hand-maintained auth union takes (`authMode: 'local_cli'`).
    for (const hit of matchesInCode(src, /local_cli/) ) {
      if (!/auth/i.test(hit.text)) continue;
      fail("AUTH-1", rel(file), `non-canonical auth spelling at line ${hit.line}: ${hit.text}`);
    }
  }
}

{
  const authModeDecls = [];
  for (const file of walk(join(ROOT, "crates"), new Set([".rs"]))) {
    const src = productionPart(readFileSync(file, "utf8"));
    if (/\benum\s+AuthMode\b/.test(src)) authModeDecls.push(rel(file));
  }
  const expected = "crates/agentcowork-types/src/lib.rs";
  if (authModeDecls.length !== 1 || authModeDecls[0] !== expected) {
    fail(
      "AUTH-2",
      expected,
      `AuthMode must be declared once in ${expected}; found: ${authModeDecls.join(", ") || "none"}`,
    );
  }
}

// --- AUTH-3: the UI derives its auth type --------------------------------
{
  const acpPath = "ui/src/lib/acp.ts";
  const settingsPath = "ui/src/lib/settings.ts";
  if (!/export type AuthMode\b/.test(read(acpPath))) {
    fail("AUTH-3", acpPath, "the canonical UI AuthMode union must be declared here (P69.C11)");
  }
  const settingsSrc = read(settingsPath);
  if (!/AgentAuthMode\s*=\s*AuthMode\b/.test(settingsSrc)) {
    fail("AUTH-3", settingsPath, "AgentAuthMode must alias the canonical union, not re-declare it");
  }
  for (const file of walk(join(ROOT, "ui", "src"), TS)) {
    const name = rel(file);
    if (name === acpPath) continue;
    const src = readFileSync(file, "utf8");
    for (const [i, line] of src.split("\n").entries()) {
      if (/'subscription'/.test(line) && /'api_key'/.test(line) && /\|/.test(line)) {
        fail(
          "AUTH-3",
          name,
          `hand-maintained auth union at line ${i + 1} (derive it from AuthMode instead): ${line.trim()}`,
        );
      }
    }
  }
}

// --- SCHEMA-1: canonical schema objects -----------------------------------
{
  const typesPath = "crates/agentcowork-types/src/lib.rs";
  const src = read(typesPath);
  const required = [
    "CANONICAL_SCHEMA_VERSION",
    "pub enum AuthMode",
    "pub struct AgentDefinition",
    "pub struct AgentBinding",
    "pub struct EffectRequest",
    "pub struct ContextSnapshot",
    "pub struct ContextPassport",
    "pub struct EventEnvelope",
  ];
  for (const symbol of required) {
    if (!src.includes(symbol)) {
      fail("SCHEMA-1", typesPath, `canonical schema symbol missing: ${symbol} (P69.D25)`);
    }
  }
}

// --- DECIDE-1: one authorization decider ----------------------------------
{
  const toolsSrc = join(ROOT, "packages", "core-tools", "src");
  for (const file of walk(toolsSrc, TS)) {
    const src = readFileSync(file, "utf8");
    for (const [name, pattern] of [
      ["evaluatePermissionGate", /\bevaluatePermissionGate\b/],
      ["TrustLadder", /\bTrustLadder\b/],
      ["maxRiskForScore", /\bmaxRiskForScore\b/],
    ]) {
      const hits = matchesInCode(src, pattern);
      if (hits.length) {
        fail(
          "DECIDE-1",
          rel(file),
          `${name} must live in @agentcowork/core-engine/src/policy (advisory), not core-tools; line ${hits[0].line}`,
        );
      }
    }
  }
}

// --- DECIDE-2: ACP permission path never hardcodes an allow ---------------
{
  const acpSrc = join(ROOT, "crates", "agentcowork-acp", "src");
  for (const file of walk(acpSrc, new Set([".rs"]))) {
    const src = productionPart(readFileSync(file, "utf8"));
    for (const pattern of [/Approval::allow\s*\(/, /Approval::Allow\b/]) {
      const hits = matchesInCode(src, pattern);
      if (hits.length) {
        fail(
          "DECIDE-2",
          rel(file),
          `ACP permission must be decided by the host gate, never hardcoded; line ${hits[0].line}: ${hits[0].text}`,
        );
      }
    }
  }
}

// --- E3-ACP-V2: a v2 connection is refused, never silently downgraded ------
// ADR-0007 §4 / TODO P71.11. The client hard-sends `protocolVersion: 1`; the
// only thing standing between a v2 agent and a silent v1 downgrade is the
// comparison against the agent's advertised version. ACP v2 removes the client
// filesystem/terminal surface, so a downgraded connection would let us claim a
// mediated fs/terminal path the agent cannot serve. Nothing in CI would notice
// if that check were deleted, so it is asserted here.
{
  const clientPath = join(ROOT, "crates", "agentcowork-acp", "src", "client.rs");
  const src = readFileSync(clientPath, "utf8");
  const checks = [
    { pattern: /if\s+result\.protocol_version\s*!=\s*PROTOCOL_VERSION/, why: "the advertised-version comparison" },
    { pattern: /AcpError::ProtocolMismatch\s*\(\s*result\.protocol_version\s*\)/, why: "the ProtocolMismatch refusal" },
  ];
  for (const { pattern, why } of checks) {
    if (!pattern.test(src)) {
      fail(
        "E3-ACP-V2",
        rel(clientPath),
        `ACP v2 must be refused, never downgraded (ADR-0007 §4, TODO P71.11): ${why} is missing — the client would accept a v2 agent as v1`,
      );
    }
  }
}

// --- LAYER-1: no second turn runtime in the TS workspace ------------------
// P71.2c (ADR-0005 §2) — the built-in engine is deferred to post-v1, so the
// strongest form of P69.D8's invariant is now the literal one: `packages/core-engine`
// must **not exist**. It held the ConversationEngine, its stages and the
// advisory policy classifiers; the engine and the coordinator loop moved to
// `ARCH/archive/core-engine/` and `ARCH/archive/coordinator-loop/`, which are
// outside the workspace and outside every tsconfig/build. A resurrected copy in
// `packages/` (or a package that re-declares the engine class) is exactly the
// competing-runtime regression this guards.
{
  const engineSrc = join(ROOT, "packages", "core-engine", "src");
  if (existsSync(engineSrc)) {
    fail(
      "LAYER-1",
      "packages/core-engine/src",
      "the built-in engine package is back in the workspace — it is deferred to post-v1 (ADR-0005 §2, P71.2c) and lives in ARCH/archive/core-engine/",
    );
  }
  const engineClass = /\bclass\s+ConversationEngine\b/;
  const loopEntry = /\brunChatStream\b\s*\(/;
  for (const root of ["packages", "ui/src", "src-tauri/src"]) {
    for (const file of walk(join(ROOT, root), TS)) {
      const src = readFileSync(file, "utf8");
      for (const [name, pattern] of [
        ["ConversationEngine", engineClass],
        ["runChatStream", loopEntry],
      ]) {
        const hits = matchesInCode(src, pattern);
        if (hits.length) {
          fail(
            "LAYER-1",
            rel(file),
            `${name} re-declared outside ARCH/archive at line ${hits[0].line} — agentcowork has no turn loop (ADR-0005 §2)`,
          );
        }
      }
    }
  }
}

// --- LAYER-2: evaluation stays outside the runtime ------------------------
// P69.D32 — `agentcowork-eval` is a harness, not a runtime dependency. Kernel
// crates verify through the contract (`agentcowork_blueprint::verify`); only the
// shell may link the harness itself, for its eval/debug surface.
{
  const offenders = [];
  for (const file of walk(join(ROOT, "crates"), new Set([".toml"]))) {
    const name = rel(file);
    if (name === "crates/agentcowork-eval/Cargo.toml") continue;
    if (!/agentcowork-eval\s*=/.test(readFileSync(file, "utf8"))) continue;
    offenders.push(name);
  }
  if (offenders.length) {
    fail(
      "LAYER-2",
      offenders[0],
      `production crates must not depend on the eval harness (P69.D32): ${offenders.join(", ")}`,
    );
  }
}

// --- PURITY: crate ownership boundaries -----------------------------------
/** Read a crate manifest's `[dependencies]` section only (dev-deps allowed). */
function productionDeps(crateName) {
  const src = read(`crates/${crateName}/Cargo.toml`);
  const lines = src.split("\n");
  const start = lines.findIndex((l) => l.trim() === "[dependencies]");
  if (start === -1) return [];
  const deps = [];
  for (let i = start + 1; i < lines.length; i += 1) {
    const line = lines[i];
    if (/^\[/.test(line.trim())) break;
    const m = /^([A-Za-z0-9_-]+)\s*=/.exec(line.trim());
    if (m) deps.push(m[1]);
  }
  return deps;
}

{
  // D26 — serialization, framing, streaming, lifecycle; no business logic.
  const ipcDeps = productionDeps("agentcowork-ipc").filter((d) => d.startsWith("agentcowork-"));
  if (ipcDeps.length) {
    fail("PURITY-1", "crates/agentcowork-ipc/Cargo.toml", `transport crate must not depend on ${ipcDeps.join(", ")}`);
  }

  // D27 (PURITY-2) retired 2026-09-23 with the `agentcowork-engine` deletion (P72):
  // the pure policy crate had zero dependents, so there is no purity left to gate.

  // D28 — provider/model metadata only; credentials and custody are elsewhere.
  const catalogDeps = productionDeps("agentcowork-catalog");
  for (const banned of ["agentcowork-vault", "agentcowork-guard"]) {
    if (catalogDeps.includes(banned)) {
      fail("PURITY-3", "crates/agentcowork-catalog/Cargo.toml", `catalog is metadata — ${banned} must not be a dependency`);
    }
  }

  // D31 — CDP is a backend under BrowserService.
  for (const crate of ["agentcowork-core", "agentcowork-acp", "agentcowork-office", "agentcowork-desktop", "agentcowork-script", "agentcowork-codeintel", "agentcowork-memory", "agentcowork-storage", "agentcowork-mcp", "agentcowork-catalog", "agentcowork-blueprint", "agentcowork-browser", "agentcowork-search", "agentcowork-agents"]) {
    const deps = productionDeps(crate);
    if (crate === "agentcowork-browser") continue;
    if (deps.includes("agentcowork-cdp")) {
      fail(
        "PURITY-4",
        `crates/${crate}/Cargo.toml`,
        "cdp is a backend under BrowserService (agentcowork-browser); kernel crates must not depend on it",
      );
    }
  }
}

// --- LAYER-3: one search implementation (the kernel's) --------------------
// P69.D9 — `agentcowork-search` (Rust) owns search; the TypeScript package is a
// projection/facade. Re-wiring the TS cascade into the turn loop would be a
// second implementation again, which is exactly what D9 removes.
{
  const cascadeSymbols = [
    'buildDefaultCascade',
    'WebSearchCascade',
    'runResearch',
    'buildCascadeProviders',
    'fetchAndRerankSearchResults',
  ];
  for (const root of ["packages/coordinator/src"]) {
    for (const file of walk(join(ROOT, root), TS)) {
      const src = readFileSync(file, "utf8");
      for (const symbol of cascadeSymbols) {
        const hits = matchesInCode(src, new RegExp(`\\b${symbol}\\b`));
        if (hits.length) {
          fail(
            "LAYER-3",
            rel(file),
            `${symbol} wired into the turn loop at line ${hits[0].line} — search is kernel-owned (P69.D9)`,
          );
        }
      }
    }
  }
}

// --- LAYER-4: the coordinator owns orchestration only (P69.D22) -----------
// The turn loop loads state, builds context, selects a route, projects tools,
// delegates, observes, verifies, recovers and finishes — through the host
// channel. Filesystem/shell/browser execution, credentials, authorization,
// sandbox enforcement and durable persistence all belong to the Rust core,
// which is why the coordinator's production files must not import an IO
// builtin or call an fs/process function at all. A coordinator that can write
// a file or spawn a process is a second execution path around Guard.
{
  const bannedImports =
    /from\s+["']node:(?:fs|fs\/promises|child_process|net|http|https|dgram|tls|worker_threads)["']/;
  const bannedCalls =
    /(?<![.\w])(?:writeFile|writeFileSync|readFile|readFileSync|appendFile|createWriteStream|mkdir|mkdirSync|unlink|unlinkSync|rm|rmSync|spawn|spawnSync|execSync|execFileSync|exec|execFile|fork)\s*\(/;
  for (const file of walk(join(ROOT, "packages/coordinator/src"), TS)) {
    if (file.endsWith(".test.ts")) continue;
    const src = readFileSync(file, "utf8");
    const imports = matchesInCode(src, bannedImports);
    if (imports.length) {
      fail(
        "LAYER-4",
        rel(file),
        `privileged IO import at line ${imports[0].line}: ${imports[0].text} — the coordinator orchestrates through the host channel (P69.D22)`,
      );
      continue;
    }
    const calls = matchesInCode(src, bannedCalls);
    if (calls.length) {
      fail(
        "LAYER-4",
        rel(file),
        `privileged IO call at line ${calls[0].line}: ${calls[0].text} — effects belong to the Rust core`,
      );
    }
  }
}

// --- TS-DUP: TypeScript never re-declares the canonical schema -------------
// P69.D15/D25 — `agentcowork-types` (Rust) owns the canonical records and id
// newtypes; a TS file may *project* them (a projection is named for its job —
// `AgentDirectoryEntry`, `AgentProfile`, `AgentPersonaOverlay`) but must never
// declare a second `AgentDefinition` or a second `WorkId`. Three different
// `AgentDefinition`s used to exist in TS alongside the real one.
{
  const patterns = [
    [/(?:export\s+)?interface\s+AgentDefinition\b/, 'AgentDefinition (Rust owns the agent record — project it, e.g. AgentDirectoryEntry)'],
    [/(?:export\s+)?type\s+AgentDefinition\b/, 'AgentDefinition (Rust owns the agent record)'],
    [/(?:export\s+)?(?:interface|type)\s+(?:WorkId|SessionId|EventId|EffectId|StepId|TicketId|AgentBindingId)\b/, 'canonical id newtype (agentcowork-types owns these)'],
  ];
  for (const root of ["packages", "ui/src"]) {
    for (const file of walk(join(ROOT, root), TS)) {
      const name = rel(file);
      if (name.includes("/dist/") || name.includes("node_modules")) continue;
      const src = readFileSync(file, "utf8");
      for (const [pattern, what] of patterns) {
        for (const hit of matchesInCode(src, pattern)) {
          fail("TS-DUP", name, `re-declares ${what} at line ${hit.line}: ${hit.text}`);
        }
      }
    }
  }
}

// --- RUST-DUP: one declaration per canonical primitive ---------------------
// The TS-DUP failure mode on the Rust side: `agentcowork-blueprint`'s plugin
// manifest used to declare a second `AgentBinding` (a `bind: Vec<String>`
// manifest declaration) beside the canonical durable primitive. A local shape
// is fine — shadowing a canonical name is not; name it for its job.
{
  const canonical = {
    AgentBinding: "crates/agentcowork-types/src/lib.rs",
  };
  for (const file of walk(join(ROOT, "crates"), new Set([".rs"]))) {
    const name = rel(file);
    const src = productionPart(readFileSync(file, "utf8"));
    for (const [symbol, owner] of Object.entries(canonical)) {
      const pattern = new RegExp(`\\b(?:pub\\s+)?(?:struct|enum)\\s+${symbol}\\b`);
      if (pattern.test(src) && name !== owner) {
        fail(
          "RUST-DUP",
          name,
          `declares a second ${symbol}; the canonical primitive lives in ${owner} — name the local shape for its job`,
        );
      }
    }
  }
}

// --- P69.E3: every effect passes the owning Guard seam ---------------------
// These are bounded structural checks, not claims that a regex can prove a
// runtime policy.  Each one follows the existing production owner named by the
// TODO marker and checks the ordering that makes the seam meaningful.

{
  // E3-CONNECTOR — the Graph write methods must approve the exact action
  // before the transport POST.  The shared read-first module owns the ticket
  // binding and single-use replay refusal.
  const graphPath = "crates/agentcowork-core/src/connectors/graph.rs";
  const readFirstPath = "crates/agentcowork-core/src/connectors/read_first.rs";
  const graph = productionCode(read(graphPath));
  const readFirst = productionCode(read(readFirstPath));

  for (const [pattern, detail] of [
    [/\bReadFirstPolicy\b/, "GraphConnector must use the read-first approval policy"],
    [/\bSendApproval\b/, "Graph writes must carry the Guard-2-shaped SendApproval"],
    [/\bapprove_before_send\s*\(/, "Graph writes must call ReadFirstPolicy::approve_before_send"],
  ]) {
    requirePattern("E3-CONNECTOR", graphPath, graph, pattern, detail);
  }
  for (const [pattern, detail] of [
    [/\bpub\s+struct\s+SendApproval\b/, "the shared SendApproval record is missing"],
    [/\bticket_id\s*:/, "the approval ticket id binding is missing"],
    [/\bbound_args_hash\b/, "the approval/payload hash binding is missing"],
    [/\ba\s*\.\s*bound_args_hash\s*!=\s*action\s*\.\s*args_hash\b/, "the approval hash is not compared with the exact action payload"],
    [/\bused_tickets\s*\.\s*insert\s*\(\s*a\s*\.\s*ticket_id\s*\.\s*clone\s*\(\s*\)\s*\)/, "ticket consumption is not recorded in the single-use ledger"],
    [/\bAlreadyUsed\b/, "ticket replay must have an explicit refusal"],
  ]) {
    requirePattern("E3-CONNECTOR", readFirstPath, readFirst, pattern, detail);
  }

  for (const method of ["send_mail", "create_calendar_event"]) {
    const fn = findRustFn(graph, method);
    if (!fn) {
      fail("E3-CONNECTOR", graphPath, `required write method ${method} is missing from production source`);
      continue;
    }
    const approval = firstMatch(fn.text, /\bpolicy\s*\.\s*approve_before_send\s*\(/);
    const transport = firstMatch(fn.text, /\bpost_json_with_refresh\s*\(/);
    if (!approval) {
      fail(
        "E3-CONNECTOR",
        graphPath,
        `${method} has no ReadFirstPolicy approval gate before its write (method starts at line ${lineAt(graph, fn.start)})`,
      );
    }
    if (!transport) {
      fail(
        "E3-CONNECTOR",
        graphPath,
        `${method} no longer has the expected transport write call (method starts at line ${lineAt(graph, fn.start)})`,
      );
    }
    if (approval && transport && approval.index > transport.index) {
      fail(
        "E3-CONNECTOR",
        graphPath,
        `${method} posts at line ${lineAt(graph, fn.bodyStart + transport.index)} after its approval gate`,
      );
    }
  }

  // The managed gws adapter is the other concrete connector write surface in
  // this module.  It uses a boolean approval seam rather than SendApproval,
  // but it must still refuse an unapproved write before constructing Command.
  const gwsPath = "crates/agentcowork-core/src/connectors/gws.rs";
  const gws = productionCode(read(gwsPath));
  const gwsCommand = findRustFn(gws, "command");
  if (!gwsCommand) {
    fail("E3-CONNECTOR", gwsPath, "managed connector command gate is missing at line 1");
  } else {
    const writeGate = firstMatch(gwsCommand.text, /GwsAction\s*::\s*Write\s*&&\s*!\s*approved/);
    const command = firstMatch(gwsCommand.text, /std::process\s*::\s*Command\s*::\s*new/);
    if (!writeGate) {
      fail("E3-CONNECTOR", gwsPath, "gws writes no longer refuse an absent approval before command construction");
    }
    if (command && writeGate && writeGate.index > command.index) {
      fail("E3-CONNECTOR", gwsPath, "gws command construction precedes its write approval gate");
    }
  }
}

{
  // E3-MCP — protocol transport authenticates first and delegates tool calls
  // to the host seam; the shell's remote/attached executors consume a ticket
  // before a process or network effect.
  const serverPath = "crates/agentcowork-mcp/src/server.rs";
  const server = productionCode(read(serverPath));
  const handle = findRustFn(server, "handle_json");
  const serve = findRustFn(server, "serve_http_connection");
  // The host seam is `handler.call_outcome` inside the tools/call arm.
  // HTTP auth lives on `HttpPolicy::validate` (bearer, then origin_is_local
  // via origin_matches) before `serve_http_connection` dispatches the body.
  // `thread::spawn` is the lease worker, not an effect; process spawn is
  // still forbidden in this file.
  if (!handle) {
    fail("E3-MCP", serverPath, "McpServer::handle_json is missing from production source (line 1)");
  }
  const toolArm = firstMatch(server, /"tools\/call"/);
  const hostCall = firstMatch(server, /\bself\s*\.\s*handler\s*\.\s*call_outcome\s*\(/);
  if (!toolArm || !hostCall || hostCall.index < toolArm.index) {
    fail("E3-MCP", serverPath, "tools/call must dispatch through the host ToolCallHandler seam");
  }
  if (!serve) {
    fail("E3-MCP", serverPath, "McpServer::serve_http_connection is missing (line 1)");
  } else if (!firstMatch(serve.text, /\bserve_http_connection_with_dispatch\s*\(/)) {
    fail("E3-MCP", serverPath, "HTTP MCP must enter the shared dispatch path");
  }
  const validate = findRustFn(server, "validate");
  if (!validate) {
    fail("E3-MCP", serverPath, "HttpPolicy::validate is missing");
  } else {
    const bearer = firstMatch(validate.text, /\bbearer_matches\s*\(/);
    const origin = firstMatch(validate.text, /\borigin_matches\s*\(/);
    if (!bearer || !origin || bearer.index > origin.index) {
      fail("E3-MCP", serverPath, "HTTP MCP dispatch must check bearer authorization, then loopback origin, before the body is accepted");
    }
    requirePattern("E3-MCP", serverPath, validate.text, /\bauthorization\b/, "MCP Authorization header handling is missing");
  }
  requirePattern("E3-MCP", serverPath, server, /\borigin_is_local\s*\(/, "MCP HTTP origin must be checked with origin_is_local before dispatch");
  requirePattern("E3-MCP", serverPath, server, /\bbearer_token\b/, "MCP bearer token state is missing");
  for (const pattern of [/\bstd::process\s*::/, /\bCommand\s*::\s*new\s*\(/]) {
    const hit = firstMatch(server, pattern);
    if (hit) {
      fail("E3-MCP", serverPath, `MCP protocol transport must not spawn/execute effects directly (line ${lineAt(server, hit.index)})`);
    }
  }

  const mcpCmdsPath = "src-tauri/src/mcp_cmds.rs";
  const mcpCmds = productionCode(read(mcpCmdsPath));
  const loopCall = findRustFnContaining(mcpCmds, "call", "server.call_tool(");
  if (!loopCall || !/ExternalToolBackend/.test(mcpCmds)) {
    fail("E3-MCP", mcpCmdsPath, "attached MCP tools must implement the kernel ExternalToolBackend seam");
  }
  requirePattern("E3-MCP", mcpCmdsPath, mcpCmds, /\battach_external_server\s*\(/, "attached MCP tools are no longer reconciled into the kernel ToolService registry");
  const remoteRequest = findRustFn(mcpCmds, "mcp_remote_call");
  const remoteCommit = findRustFn(mcpCmds, "mcp_remote_call_commit");
  const attachCommit = findRustFn(mcpCmds, "mcp_attach_commit");
  if (!remoteRequest) {
    fail("E3-MCP", mcpCmdsPath, "remote MCP request half is missing (line 1)");
  } else {
    const mutationBranch = firstMatch(remoteRequest.text, /method\s*!=\s*["']tools\/call["']/);
    const evaluate = firstMatch(remoteRequest.text, /\bguard\s*\.\s*evaluate\s*\(/);
    const stash = firstMatch(remoteRequest.text, /\bmcp_pending_calls\b/);
    if (!mutationBranch || !evaluate || !stash) {
      fail("E3-MCP", mcpCmdsPath, "remote tools/call must evaluate Guard and stash the exact pending call");
    } else if (mutationBranch.index > evaluate.index) {
      fail("E3-MCP", mcpCmdsPath, "remote tools/call Guard evaluation occurs before the mutation-only branch");
    }
  }
  if (!remoteCommit) {
    fail("E3-MCP", mcpCmdsPath, "remote MCP commit/executor half is missing (line 1)");
  } else {
    const useTicket = firstMatch(remoteCommit.text, /\buse_ticket\s*\(/);
    const network = firstMatch(remoteCommit.text, /\brpc\s*\(/);
    if (!useTicket || !network) {
      fail("E3-MCP", mcpCmdsPath, "remote MCP commit must consume a ticket and then execute the call");
    } else if (useTicket.index > network.index) {
      fail("E3-MCP", mcpCmdsPath, "remote MCP network execution precedes ticket consumption");
    }
  }
  if (!attachCommit) {
    fail("E3-MCP", mcpCmdsPath, "MCP attach commit/executor half is missing (line 1)");
  } else {
    const useTicket = firstMatch(attachCommit.text, /\buse_ticket\s*\(/);
    const spawn = firstMatch(attachCommit.text, /\bspawn_named_stdio\s*\(/);
    if (!useTicket || !spawn) {
      fail("E3-MCP", mcpCmdsPath, "MCP attach commit must consume a ticket before spawning the child");
    } else if (useTicket.index > spawn.index) {
      fail("E3-MCP", mcpCmdsPath, "MCP child spawn precedes ticket consumption");
    }
  }
}

{
  // E3-ACP — the library keeps the fail-closed host gate, and the shell maps
  // the ACP tool kind into a Guard decision before any permission reply.
  const acpLibPath = "crates/agentcowork-acp/src/chief.rs";
  const acpLib = productionCode(read(acpLibPath));
  const acpImpl = acpLib.indexOf("impl ChiefAdapter for AcpChief");
  const acpRequest = acpImpl === -1
    ? null
    : findRustFnContaining(acpLib, "request_permission", "self.gate.decide", acpImpl);
  for (const [pattern, detail] of [
    [/\bpub\s+trait\s+PermissionGate\b/, "ACP host PermissionGate trait is missing"],
    [/\bstruct\s+DenyAllGate\b/, "ACP fail-closed DenyAllGate is missing"],
    [/\bgate\s*:\s*Arc\s*<\s*dyn\s+PermissionGate\s*>/, "AcpChief no longer owns the host gate"],
  ]) {
    requirePattern("E3-ACP", acpLibPath, acpLib, pattern, detail);
  }
  if (!acpRequest) {
    fail("E3-ACP", acpLibPath, "AcpChief::request_permission does not call the host PermissionGate (line 1)");
  }

  const acpShellPath = "src-tauri/src/acp_cmds.rs";
  const acpShell = productionCode(read(acpShellPath));
  const mapTool = findRustFn(acpShell, "map_tool_call");
  const prompt = findRustFn(acpShell, "acp_prompt");
  if (!mapTool) {
    fail("E3-ACP", acpShellPath, "ACP tool-to-Guard risk mapper is missing (line 1)");
  } else {
    for (const [pattern, detail] of [
      [/\bToolKind::Delete\b/, "delete tool kind is not mapped"],
      [/\bToolKind::Execute\b/, "execute tool kind is not mapped"],
      [/\bRiskLevel::High\b/, "ACP high-risk tool mapping is missing"],
      [/\bOperation::GenericWrite\b/, "ACP write operation mapping is missing"],
    ]) {
      requirePattern("E3-ACP", acpShellPath, mapTool.text, pattern, detail);
    }
  }
  if (!prompt) {
    fail("E3-ACP", acpShellPath, "acp_prompt host permission closure is missing (line 1)");
  } else {
    const hostGuard = firstMatch(prompt.text, /\bstate\s*\.\s*guard_service\b/);
    const map = firstMatch(prompt.text, /\bmap_tool_call\s*\(/);
    const decision = firstMatch(prompt.text, /\bDecisionPackage\s*::\s*new\s*\(/);
    const evaluate = firstMatch(prompt.text, /\bg\s*\.\s*evaluate\s*\(/);
    const consume = firstMatch(prompt.text, /\bg\s*\.\s*use_ticket\s*\(/);
    const ask = firstMatch(prompt.text, /\bg\s*\.\s*watch_ticket\s*\(/);
    const allow = firstMatch(prompt.text, /\bPermissionDecision\s*::\s*allow\s*\(/);
    if (!hostGuard || !map || !decision || !evaluate || !consume || !ask) {
      fail("E3-ACP", acpShellPath, "ACP permission path must map the tool, build DecisionPackage, evaluate Guard, wait for Ask, and consume the ticket");
    } else if (!(map.index < decision.index && decision.index < evaluate.index && evaluate.index < consume.index)) {
      fail("E3-ACP", acpShellPath, "ACP permission path does not preserve map → DecisionPackage → evaluate → use_ticket order");
    } else if (allow && allow.index < consume.index) {
      fail("E3-ACP", acpShellPath, "ACP permission path returns allow before consuming the Guard ticket");
    }
  }
}

{
  // E3-UI — every command in work_cmds.rs must acquire the one gateway and
  // call a gateway method.  Direct filesystem/store mutation in this module
  // would create a second writer beside WorkGateway::append.
  const workPath = "src-tauri/src/work_cmds.rs";
  const work = productionCode(read(workPath));
  const gatewayFn = findRustFn(work, "gateway");
  if (!gatewayFn) {
    fail("E3-UI", workPath, "work_cmds gateway accessor is missing");
  } else {
    requirePattern("E3-UI", workPath, work, /\bWorkGateway\b/, "work command accessor no longer names WorkGateway");
    requirePattern("E3-UI", workPath, gatewayFn.text, /\.\s*work_gateway\s*\(\s*\)/, "work command accessor no longer obtains relay.work_gateway()");
  }

  const commandRe = /#\[tauri::command\][\s\S]*?\bpub\s+fn\s+(\w+)\s*\(/g;
  for (const match of work.matchAll(commandRe)) {
    const name = match[1];
    const fn = findRustFn(work, name, match.index);
    if (!fn) {
      fail("E3-UI", workPath, `cannot parse production body for Tauri command ${name} near line ${lineAt(work, match.index)}`);
      continue;
    }
    if (!/\bgateway\s*\(\s*&\s*state\s*\)/.test(fn.text)) {
      fail("E3-UI", workPath, `${name} does not acquire the shared WorkGateway (command starts at line ${lineAt(work, fn.start)})`);
    }
    if (!/\b(?:g|gateway)\s*\.\s*[A-Za-z_][A-Za-z0-9_]*\s*\(/.test(fn.text)) {
      fail("E3-UI", workPath, `${name} does not call a WorkGateway method (command starts at line ${lineAt(work, fn.start)})`);
    }
    const directWrite = firstMatch(
      fn.text,
      /\b(?:std::fs::write|fs::write|File::create|OpenOptions::new)\s*\(/,
    );
    if (directWrite) {
      fail("E3-UI", workPath, `${name} writes a store/file directly at line ${lineAt(work, fn.bodyStart + directWrite.index)}; route through WorkGateway`);
    }
  }
  for (const [command, method] of [
    ["work_create", "create_work"],
    ["work_agent_spawn", "spawn_subagent"],
    ["work_review_resolve", "resolve_review_with"],
    ["work_steer", "queue_steering"],
  ]) {
    const fn = findRustFn(work, command);
    if (!fn || !new RegExp(`\\b(?:g|gateway)\\s*\\.\\s*${method}\\s*\\(`).test(fn.text)) {
      fail("E3-UI", workPath, `${command} must route its mutation through WorkGateway::${method}`);
    }
  }
}

// --- P69.E4: one journal, no owner-owned execution, one Work creation API ----
{
  const ownerPath = "crates/agentcowork-core/src/work_gateway.rs";
  const schemaPath = "crates/agentcowork-core/src/store_schema.rs";
  const owner = productionCode(read(ownerPath));
  const schema = productionCode(read(schemaPath));

  requirePattern("E4-JOURNAL", ownerPath, owner, /default_data_dir\s*\(\s*\)\s*\.join\s*\(\s*["']work["']\s*\)\s*\.join\s*\(\s*["']events\.jsonl["']\s*\)/, "WorkGateway::open_default no longer names work/events.jsonl");
  requirePattern(
    "E4-JOURNAL",
    schemaPath,
    schema,
    /name\s*:\s*["']work_journal["'][\s\S]{0,260}?path\s*:\s*["']work\/events\.jsonl["'][\s\S]{0,180}?policy\s*:\s*StorePolicy::Manifest/,
    "the durable store registry no longer registers work_journal at work/events.jsonl",
  );

  const journalOwners = new Set([ownerPath, schemaPath]);
  for (const file of productionRustFiles(["crates", "src-tauri/src"])) {
    const name = rel(file);
    const code = productionCode(readFileSync(file, "utf8"));
    for (const match of code.matchAll(/events\.jsonl/g)) {
      if (!journalOwners.has(name)) {
        fail("E4-JOURNAL", name, `second events.jsonl path at line ${lineAt(code, match.index)}; WorkGateway/store_schema are the only owners`);
      }
    }
  }

  const append = findRustFn(owner, "append");
  if (!append) {
    fail("E4-JOURNAL", ownerPath, "WorkGateway::append is missing (line 1)");
  } else {
    const write = firstMatch(append.text, /\bwriteln!\s*\(/);
    const flush = firstMatch(append.text, /\bfile\s*\.\s*flush\s*\(/);
    const publish = firstMatch(append.text, /\bsubscribers[\s\S]*?\.send\s*\(/);
    if (!write || !flush || !publish) {
      fail("E4-JOURNAL", ownerPath, "WorkGateway::append must flush the journal before publishing to subscribers");
    } else if (write.index > flush.index || flush.index > publish.index) {
      fail("E4-JOURNAL", ownerPath, "WorkGateway::append publishes an event before durable journal flush");
    }
  }
  const open = findRustFn(owner, "open");
  if (!open || !firstMatch(open.text, /\bapply_replayed_event\s*\(/)) {
    fail("E4-JOURNAL", ownerPath, "WorkGateway::open must rebuild state through apply_replayed_event");
  }
  requirePattern("E4-JOURNAL", ownerPath, owner, /\bAgentBindingCreated\b/, "journal replay no longer carries durable agent-binding events");
}

{
  const schedulerPath = "crates/agentcowork-core/src/scheduler_service.rs";
  const scheduler = productionCode(read(schedulerPath));
  const processPatterns = [
    [/\bstd::process\s*::/, "std::process reference"],
    [/\bCommand\s*::\s*new\s*\(/, "Command::new process spawn"],
    [/\b(?:std::thread|tokio)\s*::\s*(?:spawn|Builder::new)\s*\(/, "thread/task spawn"],
    [/\bspawn\s*\(/, "direct spawn call"],
  ];
  for (const [pattern, label] of processPatterns) {
    const hit = firstMatch(scheduler, pattern);
    if (hit) fail("E4-SPAWN", schedulerPath, `${label} in scheduler trigger plane at line ${lineAt(scheduler, hit.index)}`);
  }

  // These are the declarative/runtime-shaped blueprint owners named by the
  // existing D12 contract.  Worktree/verification process code is deliberately
  // outside this scope: it is an evidence/tool backend, not subagent runtime
  // ownership.
  const runtimeFiles = [
    "crates/agentcowork-blueprint/src/subagent.rs",
    "crates/agentcowork-blueprint/src/jobs.rs",
    "crates/agentcowork-blueprint/src/kanban.rs",
    "crates/agentcowork-blueprint/src/swarm.rs",
    "crates/agentcowork-blueprint/src/workflow.rs",
    "crates/agentcowork-blueprint/src/loop_pattern.rs",
    "crates/agentcowork-blueprint/src/marketplace.rs",
  ];
  for (const file of runtimeFiles) {
    const code = productionCode(read(file));
    for (const [pattern, label] of processPatterns) {
      const hit = firstMatch(code, pattern);
      if (hit) fail("E4-SPAWN", file, `${label} in declarative blueprint owner at line ${lineAt(code, hit.index)}`);
    }
    const runtimeDecl = firstMatch(code, /\b(?:SubAgentRuntime|SwarmSession|JobRuntime|WorkflowRuntime|MultiRunRuntime)\b/);
    if (runtimeDecl) {
      fail("E4-SPAWN", file, `runtime owner declaration ${runtimeDecl.text} at line ${lineAt(code, runtimeDecl.index)}; WorkGateway/ExecutionKernel own runtime state`);
    }
  }
}

{
  const ownerPath = "crates/agentcowork-core/src/work_gateway.rs";
  const owner = productionCode(read(ownerPath));
  const allowedCreators = new Set([
    ownerPath,
    "crates/agentcowork-core/src/chat.rs",
    "src-tauri/src/scheduler_fire.rs",
    "src-tauri/src/work_cmds.rs",
    // ACP turns call WorkGateway::create_work_in_session. They do not append
    // a WorkCreated event themselves.
    "src-tauri/src/acp_cmds.rs",
  ]);
  const creationPattern = /\.\s*(create_work(?:_in_session)?|create_child_work|delegate_child_work)\s*\(/g;
  for (const file of productionRustFiles(["crates", "src-tauri/src"])) {
    const name = rel(file);
    const code = productionCode(readFileSync(file, "utf8"));
    creationPattern.lastIndex = 0;
    for (const match of code.matchAll(creationPattern)) {
      if (!allowedCreators.has(name)) {
        fail(
          "E4-WORK-CREATION",
          name,
          `${match[1]} at line ${lineAt(code, match.index)} creates Work outside the WorkGateway owner/callers`,
        );
      }
    }
    if (name !== ownerPath) {
      const addressCtor = /\bWorkAddress\s*::\s*new\s*\(/g;
      for (const match of code.matchAll(addressCtor)) {
        fail("E4-WORK-CREATION", name, `direct WorkAddress::new at line ${lineAt(code, match.index)}; use a WorkGateway creation API`);
      }
      const createdEvent = /\b(?:DomainEvent\s*::\s*WorkCreated|WorkEvent\s*::\s*Domain\s*\(\s*DomainEvent\s*::\s*WorkCreated)/g;
      for (const match of code.matchAll(createdEvent)) {
        const before = code.slice(Math.max(0, match.index - 500), match.index);
        const ahead = code.slice(match.index, match.index + 240);
        // Replay and idempotency checks read WorkCreated. Only a constructed
        // event (an append of a new value) is a second writer.
        const reading = /matches!\s*\(/.test(before) || ahead.includes("=>");
        if (reading) continue;
        fail("E4-WORK-CREATION", name, `direct WorkCreated event at line ${lineAt(code, match.index)}; append it only through WorkGateway`);
      }
    }
  }
  const delegate = findRustFn(owner, "delegate_child_work");
  if (!delegate || !/\.\s*create_child_work\s*\(/.test(delegate.text)) {
    fail("E4-WORK-CREATION", ownerPath, "WorkGateway::delegate_child_work must enter through create_child_work");
  }
  for (const [file, pattern, detail] of [
    [ownerPath, /pub\s+fn\s+create_work\s*\(/, "WorkGateway::create_work API is missing"],
    [ownerPath, /pub\s+fn\s+create_work_in_session\s*\(/, "WorkGateway::create_work_in_session API is missing"],
    [ownerPath, /pub\s+fn\s+create_child_work\s*\(/, "WorkGateway::create_child_work API is missing"],
    ["crates/agentcowork-core/src/chat.rs", /\.\s*delegate_child_work\s*\(/, "chat delegation no longer enters through WorkGateway"],
    ["src-tauri/src/scheduler_fire.rs", /\.\s*create_work_in_session\s*\(/, "scheduler firing no longer creates Work through WorkGateway"],
    ["src-tauri/src/work_cmds.rs", /\.\s*create_work\s*\(/, "the Tauri Work command no longer creates Work through WorkGateway"],
  ]) {
    requirePattern("E4-WORK-CREATION", file, productionCode(read(file)), pattern, detail);
  }
}

// --- P69.E5: the UI is not a second Work writer -------------------------
{
  const uiRoot = join(ROOT, "ui", "src");
  for (const file of walk(uiRoot, TS)) {
    const src = readFileSync(file, "utf8");
    if (/create_work_in_session|DomainEvent::WorkCreated|WorkEvent::Domain/.test(src)) {
      fail("E5-UI", rel(file), "UI source constructs Work; call a Work Gateway command instead");
    }
  }
}

// --- P69.F1: freeze canonical type names ---------------------------------
{
  const frozen = [
    "pub enum AuthMode",
    "pub struct AgentBinding",
    "pub struct EffectRequest",
    "pub struct ContextPassport",
    "pub struct EventEnvelope",
    "pub struct AuthorizationTicket",
  ];
  const allowed = new Map([
    ["pub enum AuthMode", new Set(["crates/agentcowork-types/src/lib.rs"])],
    ["pub struct AgentBinding", new Set(["crates/agentcowork-types/src/lib.rs"])],
    ["pub struct EffectRequest", new Set(["crates/agentcowork-types/src/lib.rs"])],
    ["pub struct EventEnvelope", new Set(["crates/agentcowork-types/src/lib.rs"])],
    [
      "pub struct ContextPassport",
      new Set([
        "crates/agentcowork-types/src/lib.rs",
        "crates/agentcowork-memory/src/passport.rs",
      ]),
    ],
    [
      "pub struct AuthorizationTicket",
      new Set([
        "crates/agentcowork-guard/src/ticket.rs",
        "crates/agentcowork-types/src/lib.rs",
      ]),
    ],
  ]);
  for (const file of productionRustFiles(["crates", "src-tauri/src"])) {
    const name = rel(file);
    const src = readFileSync(file, "utf8");
    for (const symbol of frozen) {
      if (!src.includes(symbol)) continue;
      const owners = allowed.get(symbol);
      if (!owners.has(name)) {
        fail("F1-FREEZE", name, `${symbol} is declared outside the frozen set`);
      }
    }
  }
}

// --- P69.C5 / C6: stale authorization slogans and over-broad audit claims ---
{
  const files = readdirSync(join(ROOT, "ARCH")).filter((name) => name.endsWith(".md"));
  for (const name of files) {
    const lines = read(`ARCH/${name}`).split("\n");
    lines.forEach((line, index) => {
      const lower = line.toLowerCase();
      const ticketSlogan = lower.includes("every mutation is ticketed") || lower.includes("everything is ticketed");
      const correction = /stale|false|obsolete|superseded|never|not the invariant|must not be used|was always wrong/.test(lower);
      if (ticketSlogan && !correction) {
        fail("C5-WORDING", `ARCH/${name}`, `line ${index + 1} states the retired ticket slogan as current`);
      }
      if (lower.includes("every action in the app is audited") || lower.includes("every external agent action is audited")) {
        fail("C6-OBSERVABILITY", `ARCH/${name}`, `line ${index + 1} claims an audit trail the host does not mediate`);
      }
    });
  }
}

// --- report ---------------------------------------------------------------
if (failures.length) {
  console.error(`architecture-invariant gate: ${failures.length} violation(s)\n`);
  for (const f of failures) console.error(`  [${f.id}] ${f.file}\n        ${f.detail}`);
  console.error(
    "\nThese invariants are the structural form of ARCH/CORE.md §2 (sidecar proposes,\n" +
      "Rust disposes) and §6 (keys live only in the vault). Fix the code, not the gate.",
  );
  process.exit(1);
}

console.log(
  "architecture-invariant gate: OK (CRED-1/2/3, AUTH-1/2/3, SCHEMA-1, DECIDE-1/2, LAYER-1/2/3/4, E3-CONNECTOR/MCP/ACP/UI, E3-ACP-V2, E4-JOURNAL/SPAWN/WORK-CREATION, PURITY-1/3/4, TS-DUP, RUST-DUP)",
);
