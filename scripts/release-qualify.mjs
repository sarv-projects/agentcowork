#!/usr/bin/env node
// P70.E — release qualification harness (E1–E12).
//
// One entry point for the release-candidate pass. Each item reports exactly
// one of:
//
//   PASS      the check ran here and succeeded
//   FAIL      the check ran here and failed (the only state that exits 1)
//   RUNNABLE  the check can run here but was not executed (needs --execute,
//             or a binary/toolchain that is not present in this checkout)
//   BLOCKED   the check cannot run here at all — the blocker is named, with
//             the artefact the release driver must produce
//
// The distinction matters: "blocked" is not "passed", and this harness never
// upgrades one into the other. `--record <version>` writes the sign-off file,
// and refuses to do so unless **every** item is PASS — which is why E12 can
// only ever pass from a machine that has executed the whole list.
//
// Usage:
//   node scripts/release-qualify.mjs                     fast checks + blockers
//   node scripts/release-qualify.mjs --execute           also run the suites
//   node scripts/release-qualify.mjs --record 0.1.0      write the sign-off file
//   node scripts/release-qualify.mjs --json              machine output
//
// Exit: 0 = no FAIL (blocked items allowed), 1 = a FAIL, or --record with a
// non-PASS item.

import { readFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '..');

const argv = process.argv.slice(2);
const flag = (name) => argv.includes(name);
const opt = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const EXECUTE = flag('--execute');
const JSON_OUT = flag('--json');
const RECORD = opt('--record');

// The live switch is read on either spelling: CI sets AGENTCOWORK_LIVE_TEST;
// the pre-rename Rust harness still gates on EVERYAIOS_LIVE_TEST.
const LIVE = (process.env.AGENTCOWORK_LIVE_TEST ?? process.env.EVERYAIOS_LIVE_TEST) === '1';
const read = (p) => readFileSync(resolve(ROOT, p), 'utf8');
const has = (p) => existsSync(resolve(ROOT, p));

/** Run a command, returning {ok, out}. Never throws. */
function sh(command, cwd = ROOT) {
  const r = spawnSync(command, { shell: true, cwd, encoding: 'utf8', timeout: 30 * 60 * 1000 });
  return { ok: r.status === 0, out: `${r.stdout ?? ''}${r.stderr ?? ''}`.trim() };
}

const results = [];
const record = (id, title, status, detail) => {
  results.push({ id, title, status, detail });
};

// ---------------------------------------------------------------- E1
function e1_kernelGate() {
  const todo = read('TODO.md');
  const start = todo.indexOf('## KERNEL GATE');
  const end = todo.indexOf('\n---', start);
  const section = todo.slice(start, end > 0 ? end : undefined);
  const open = [...section.matchAll(/^- \[ \] \*\*(P[\w.]+) — ([^*]+)\*\*/gm)].map((m) => `${m[1]} ${m[2]}`);
  if (open.length) {
    record('P70.E1', 'kernel gate clear', 'FAIL', `${open.length} open: ${open.join('; ')}`);
  } else {
    const done = (section.match(/^- \[x\]/gm) ?? []).length;
    record('P70.E1', 'kernel gate clear', 'PASS', `${done} item(s) closed; doc-sync blocks new capability rows while any is open`);
  }
}

// ---------------------------------------------------------------- E2
function e2_suites() {
  const commandList = [
    ['cargo test --workspace --all-features', 'crates'],
    ['cargo clippy --workspace --all-targets --all-features -- -D warnings', 'crates'],
    ['cargo fmt --all -- --check', 'crates'],
    ['npm run type-check', 'ui'],
    ['bun test', 'ui'],
    ["pnpm --filter './packages/core-*' run build", '.'],
    ["pnpm --filter './packages/core-*' run test", '.'],
    ['bun run type-check', 'packages/coordinator'],
    ['pnpm --filter @agentcowork/coordinator test', '.'],
    ['pnpm --filter @agentcowork/coordinator build', '.'],
  ];
  if (!EXECUTE) {
    record('P70.E2', 'all suites green', 'RUNNABLE',
      `${commandList.length} commands not executed — re-run with --execute (ci.yml runs them on every PR)`);
    return;
  }
  const failures = [];
  for (const [cmd, cwd] of commandList) {
    const { ok, out } = sh(cmd, resolve(ROOT, cwd));
    if (!ok) failures.push(`${cmd}${out ? ` — ${out.split('\n').slice(-3).join(' | ')}` : ''}`);
  }
  record('P70.E2', 'all suites green', failures.length ? 'FAIL' : 'PASS',
    failures.length ? failures.join('; ') : `${commandList.length} commands passed`);
}

// ---------------------------------------------------------------- E3
function e3_docGates() {
  // 2026-09-26: `gen-codebase-map.mjs --check` retired (CODEBASE-MAP.md
  // archived; the map is no longer wired into CI). `check-doc-refs.mjs` takes
  // the documentation-integrity slot.
  const gates = ['check-doc-sync.mjs', 'check-doc-refs.mjs', 'ipc-parity.mjs'];
  const failures = [];
  for (const gate of gates) {
    const { ok, out } = sh(`node scripts/${gate}`);
    if (!ok) failures.push(`${gate}${out ? ` — ${out.split('\n')[0]}` : ''}`);
  }
  // ipc-parity exits 0 even with broken commands: assert the counts directly.
  const parity = sh('node scripts/ipc-parity.mjs');
  try {
    const parsed = JSON.parse(parity.out);
    if (parsed.counts?.broken > 0) failures.push(`ipc-parity: ${parsed.counts.broken} broken`);
    if ((parsed.unregisteredDefinitions ?? []).length > 0) failures.push('ipc-parity: unregistered definitions');
  } catch {
    failures.push('ipc-parity: output was not JSON');
  }
  record('P70.E3', 'documentation gates', failures.length ? 'FAIL' : 'PASS',
    failures.length ? failures.join('; ') : 'doc-sync + doc-refs + ipc-parity all exit 0');
}

// ---------------------------------------------------------------- E4
function e4_securityGate() {
  const wiredIn = (workflow) =>
    has(workflow) && /security-gate\.mjs/.test(read(workflow));
  const wired = wiredIn('.github/workflows/p50-gates.yml') && wiredIn('.github/workflows/release.yml');
  if (!wired) {
    record('P70.E4', 'security gate suite', 'FAIL',
      'scripts/e2e/security-gate.mjs is no longer wired into p50-gates.yml + release.yml');
    return;
  }
  if (!EXECUTE) {
    record('P70.E4', 'security gate suite', 'RUNNABLE',
      'S1–S6 wired in p50-gates (3-OS matrix) + release.yml; re-run with --execute');
    return;
  }
  const { ok, out } = sh('node scripts/e2e/security-gate.mjs');
  record('P70.E4', 'security gate suite', ok ? 'PASS' : 'FAIL', ok ? 'S1–S6 passed' : out.split('\n').slice(-3).join(' | '));
}

// ---------------------------------------------------------------- E5
function e5_liveIntegration() {
  const needs = [
    'real Chrome/Chromium for CDP legs',
    'an installed external agent for the ACP handshake/prompt/permission legs',
    'LibreOffice for the office oracle (ubuntu CI installs it; this host may not)',
    'a real vault-hydration run on each claimed platform',
  ];
  if (!LIVE) {
    record('P70.E5', 'live integration gates', 'BLOCKED',
      `set AGENTCOWORK_LIVE_TEST=1 (legacy EVERYAIOS_LIVE_TEST is also honoured) and provide: ${needs.join('; ')}`);
    return;
  }
  record('P70.E5', 'live integration gates', EXECUTE ? 'RUNNABLE' : 'RUNNABLE',
    'live switch is on; the live legs run in p50-gates + the crate `#[ignore]` live tests');
}

// ---------------------------------------------------------------- E6
function e6_liveSoak() {
  record('P70.E6', 'live-model soak', 'BLOCKED',
    'needs live provider credentials and a real repository checkout; closes P64.5/P64.6 (edit-ladder + shadow preflight) and P65.8 (external-agent probes) — no credential exists in this environment');
}

// ---------------------------------------------------------------- E7
function e7_crashSoak() {
  const wired = has('.github/workflows/p50-gates.yml') && /failure-injection\.mjs/.test(read('.github/workflows/p50-gates.yml'));
  const bin = resolve(ROOT, `crates/target/debug/agentcowork-core${process.platform === 'win32' ? '.exe' : ''}`);
  if (!wired) {
    record('P70.E7', 'crash-free session soak', 'FAIL', 'failure-injection L1–L7 is no longer wired into p50-gates.yml');
    return;
  }
  if (!existsSync(bin)) {
    record('P70.E7', 'crash-free session soak', 'BLOCKED',
      `L1–L7 are wired in p50-gates (3-OS matrix) but the driver needs the debug core binary at ${bin}`);
    return;
  }
  if (!EXECUTE) {
    record('P70.E7', 'crash-free session soak', 'RUNNABLE', 'debug core binary present; re-run with --execute');
    return;
  }
  const { ok, out } = sh(`node scripts/e2e/failure-injection.mjs`);
  record('P70.E7', 'crash-free session soak', ok ? 'PASS' : 'FAIL', ok ? 'L1–L7 passed' : out.split('\n').slice(-3).join(' | '));
}

// ---------------------------------------------------------------- E8
function e8_upgradeEvidence() {
  const driver = has('crates/agentcowork-core/tests/acceptance_upgrade_evidence.rs');
  if (!driver) {
    record('P70.E8', 'upgrade/downgrade/rollback evidence', 'FAIL', 'the upgrade-evidence harness is gone');
    return;
  }
  record('P70.E8', 'upgrade/downgrade/rollback evidence', 'BLOCKED',
    'needs two sequential Windows builds (install N-1, upgrade to N, reinstall N-1); the driver checklist and the honest failure modes are in crates/agentcowork-core/tests/acceptance_upgrade_evidence.rs + docs/updating.md §7');
}

// ---------------------------------------------------------------- E9
function e9_cleanMachine() {
  const closest = has('scripts/clean-profile-boot-check.mjs');
  record('P70.E9', 'clean-machine install test', 'BLOCKED',
    `needs a clean Windows VM/host (install → first run → provider → one real task → uninstall). Closest available evidence, wired in p50-gates on all three OSes${closest ? '' : ' (MISSING)'}: scripts/clean-profile-boot-check.mjs (P50.1.7 — isolated profile, honest locked/setup, zero seeds)`);
}

// ---------------------------------------------------------------- E10
function e10_perf() {
  const wired = has('.github/workflows/perf-regression.yml');
  if (!wired) {
    record('P70.E10', 'performance regression gate', 'FAIL', 'perf-regression.yml is gone');
    return;
  }
  const commands = [
    'cargo test --release --all-features --test p10_bench -- --test-threads=4 (crates/)',
    'node scripts/measure-perf-p45.mjs',
    'node scripts/check-size-budget.mjs --measurements <file>',
  ];
  if (!EXECUTE) {
    record('P70.E10', 'performance regression gate', 'RUNNABLE',
      `budgets in docs/packaging/budgets.json; commands: ${commands.join('; ')}`);
    return;
  }
  const failures = [];
  for (const cmd of ['cargo test --release --all-features --test p10_bench -- --test-threads=4', 'node scripts/measure-perf-p45.mjs']) {
    const cwd = cmd.startsWith('cargo') ? resolve(ROOT, 'crates') : ROOT;
    const { ok, out } = sh(cmd, cwd);
    if (!ok) failures.push(`${cmd.split(' ').slice(0, 3).join(' ')}${out ? ` — ${out.split('\n').slice(-2).join(' | ')}` : ''}`);
  }
  const budget = sh('node scripts/check-size-budget.mjs');
  if (!budget.ok) failures.push('check-size-budget (budget file invalid)');
  record('P70.E10', 'performance regression gate', failures.length ? 'FAIL' : 'PASS',
    failures.length ? failures.join('; ') : 'p10_bench + p45 measurement + budget file all pass');
}

// ---------------------------------------------------------------- E11
// Honest-capability audit. High-signal seed patterns fail; the broader
// honesty-annotation census is reported so the reviewer can see the shape.
function e11_honestyAudit() {
  const SEED = /\b(seedData|SEED_DATA|mockData|MOCK_DATA|demoData|DEMO_DATA|fixtureData|fakeData)\b/;
  const dirs = ['ui/src', 'src-tauri/src', 'crates'];
  const hits = [];
  for (const dir of dirs) {
    const { out } = sh(`grep -rnE "\\b(seedData|SEED_DATA|mockData|MOCK_DATA|demoData|DEMO_DATA|fixtureData|fakeData)\\b" ${dir} --include="*.ts" --include="*.tsx" --include="*.rs" 2>/dev/null | grep -v "\\.test\\." | grep -v "mod tests"`);
    for (const line of out.split('\n').filter(Boolean)) {
      if (SEED.test(line)) hits.push(line);
    }
  }
  // Informational: explicit honest-negative annotations (the audit's evidence
  // that unavailable surfaces are *stated*, not faked).
  const { out: negatives } = sh(
    'grep -rcE "\\b(not implemented|not built|no telemetry|not editable|switches nothing|never a hardcoded|no fixtures)\\b" ui/src --include="*.ts" --include="*.tsx" 2>/dev/null | grep -v ":0$" | wc -l'
  );
  if (hits.length) {
    record('P70.E11', 'honest capability audit', 'FAIL',
      `${hits.length} seed-shaped identifier(s) in shipping code: ${hits.slice(0, 5).join(' | ')}`);
  } else {
    record('P70.E11', 'honest capability audit', 'PASS',
      `no seed/mock/fixture data in shipping paths; ${negatives.trim() || 0} UI file(s) carry explicit honest-negative annotations (review list in the record)`);
  }
}

// ---------------------------------------------------------------- E12
function e12_signOff() {
  const version = RECORD;
  if (!version) {
    record('P70.E12', 'release-candidate sign-off', 'BLOCKED',
      'run with --record <version>: the sign-off file is only written when E1–E11 are all PASS');
    return;
  }
  const notPassed = results.filter((r) => r.status !== 'PASS');
  if (notPassed.length) {
    record('P70.E12', 'release-candidate sign-off', 'BLOCKED',
      `${notPassed.length} item(s) are not PASS (${notPassed.map((r) => `${r.id}:${r.status}`).join(', ')}) — no sign-off recorded`);
    return;
  }
  const commit = sh('git rev-parse HEAD').out.trim();
  record('P70.E12', 'release-candidate sign-off', 'PASS', `commit ${commit.slice(0, 12)} over ${results.length - 1} items`);
}

// ---------------------------------------------------------------- run
e1_kernelGate();
e2_suites();
e3_docGates();
e4_securityGate();
e5_liveIntegration();
e6_liveSoak();
e7_crashSoak();
e8_upgradeEvidence();
e9_cleanMachine();
e10_perf();
e11_honestyAudit();
e12_signOff();

const order = { PASS: 0, FAIL: 1, RUNNABLE: 2, BLOCKED: 3, SKIPPED: 4 };
const counts = results.reduce((acc, r) => ({ ...acc, [r.status]: (acc[r.status] ?? 0) + 1 }), {});
const failed = results.filter((r) => r.status === 'FAIL');
const qualified = results.every((r) => r.status === 'PASS');

if (JSON_OUT) {
  console.log(JSON.stringify({ generatedAt: new Date().toISOString(), qualified, counts, items: results }, null, 2));
} else {
  console.log('P70.E — release qualification\n');
  for (const r of results) {
    const mark = { PASS: '✓', FAIL: '✗', RUNNABLE: '▶', BLOCKED: '⛔', SKIPPED: '·' }[r.status] ?? '?';
    console.log(`${mark} ${r.id.padEnd(8)} ${r.title.padEnd(34)} ${r.status}`);
    if (r.detail) console.log(`    ${r.detail}`);
  }
  console.log(
    `\n${qualified ? 'QUALIFIED' : 'NOT QUALIFIED'} — ` +
      Object.entries(counts).sort((a, b) => order[a[0]] - order[b[0]]).map(([k, v]) => `${v} ${k}`).join(', ')
  );
}

if (RECORD) {
  // Only a fully-passing run produces a sign-off file. A partial run is
  // reported on stdout and writes nothing — a file in docs/release/ must mean
  // "this candidate passed", or the directory stops meaning anything.
  if (!qualified) {
    console.error(
      `\nno sign-off written: ${results.filter((r) => r.status !== 'PASS').length} item(s) are not PASS ` +
        `(${results.filter((r) => r.status !== 'PASS').map((r) => `${r.id}:${r.status}`).join(', ')})`
    );
  } else {
    mkdirSync(resolve(ROOT, 'docs/release'), { recursive: true });
    const file = resolve(ROOT, 'docs/release', `qualification-${RECORD}.json`);
    writeFileSync(
      file,
      JSON.stringify(
        {
          version: RECORD,
          generatedAt: new Date().toISOString(),
          commit: sh('git rev-parse HEAD').out.trim(),
          qualified: true,
          counts,
          items: results,
        },
        null,
        2
      ) + '\n'
    );
    console.log(`\nrecord: docs/release/qualification-${RECORD}.json`);
  }
}

process.exit(failed.length || (RECORD && !qualified) ? 1 : 0);
