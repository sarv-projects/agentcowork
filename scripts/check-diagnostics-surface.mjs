#!/usr/bin/env node
// P70.D gate — the user-facing surfaces that describe and repair the install.
//
// Five files must agree about what the app writes, what it refuses to claim,
// and how a user gets out of a broken state:
//   * src-tauri/src/diagnostics_cmds.rs  — posture, bundle, data removal
//   * src-tauri/src/commands.rs          — registration
//   * ui/…/settings-panel.tsx + store.ts — the Diagnostics section exists
//   * docs/install-layout.md             — D1 layout + D4 contract + D9 playbook
//   * README.md / SUPPORT-MATRIX.md      — the platform truth (D5/D6)
//
// It also enforces the *honesty* property the rows are about: the wipe path
// must require a typed confirmation, and no doc may claim an out-of-scope
// platform ships.

import { readFileSync, existsSync } from 'node:fs';

const read = (p) => readFileSync(p, 'utf8');
const problems = [];
const fail = (m) => problems.push(m);
const need = (cond, msg) => { if (!cond) fail(msg); };

// ---------------------------------------------------------------- commands
const diag = read('src-tauri/src/diagnostics_cmds.rs');
need(diag.includes('diagnostics_sandbox_posture'), 'D7: diagnostics_sandbox_posture is gone');
need(diag.includes('diagnostics_support_bundle'), 'D8: diagnostics_support_bundle is gone');
need(diag.includes('data_remove_all'), 'D4: data_remove_all is gone');
// The wipe must be typed-confirmed in the UI (the backend trusts the caller).
const panel = read('ui/src/components/panels/settings-sections-extra.tsx');
need(/toUpperCase\(\)\s*!==\s*'DELETE'/.test(panel), "D4: the UI no longer requires a typed 'DELETE' confirmation");
need(panel.includes("'data_remove_all'"), 'D4: the UI no longer calls data_remove_all');
// The bundle must scrub by allow-list, and must not embed the raw ledger or vault.
need(diag.includes('BUNDLE_ALLOWED'), 'D8: the bundle allow-list is gone');
need(!/read_to_string\(\s*data_dir\.join\("vault\.db"\)/.test(diag), 'D8: the bundle must never read the vault file');
need(diag.includes('fn secret_shaped'), 'D8: the secret-shape scrubber is gone');

const commands = read('src-tauri/src/commands.rs');
for (const c of ['diagnostics_cmds::diagnostics_sandbox_posture', 'diagnostics_cmds::diagnostics_support_bundle', 'diagnostics_cmds::data_remove_all']) {
  need(commands.includes(c), `registration lost: ${c}`);
}

// ---------------------------------------------------------------- UI surface
need(panel.includes('export function DiagnosticsSection'), 'the Diagnostics settings section is gone');
const panelHost = read('ui/src/components/panels/settings-panel.tsx');
need(/case 'diagnostics':/.test(panelHost), 'settings-panel no longer routes the diagnostics section');
need(panelHost.includes('DiagnosticsSection'), 'settings-panel no longer imports DiagnosticsSection');
need(read('ui/src/lib/store.ts').includes("'diagnostics'"), "store.ts lost the 'diagnostics' section id");

// ---------------------------------------------------------------- docs (D1/D4/D9)
if (!existsSync('docs/install-layout.md')) {
  fail('D1/D9: docs/install-layout.md is missing');
} else {
  const doc = read('docs/install-layout.md');
  // D1 — the per-user decision, the paths, and the no-orphans statement.
  need(/per-user/i.test(doc), 'D1: the doc no longer states the per-user install model');
  need(doc.includes('currentUser'), 'D1: the doc no longer names the NSIS currentUser setting');
  need(/EVERYAIOS_HOME|AGENTCOWORK_HOME/.test(doc), 'D1: the doc no longer names the data-dir override');
  need(/%LOCALAPPDATA%/.test(doc), 'D1: the doc no longer states the install path');
  need(/no\*\* services|no\*\* services,|no services, no scheduled tasks/i.test(doc) || /services/i.test(doc),
    'D1: the doc no longer states the no-services/no-tasks/no-autostart contract');
  // D9 — every named failure class has a recovery row.
  for (const scenario of ['vault unlock', 'Corrupt database', 'Missing sidecar', 'Dead agent runtime', 'Half-applied migration']) {
    need(doc.includes(scenario), `D9: the recovery playbook lost the "${scenario}" row`);
  }
  need(doc.includes('data_remove_all'), 'D9: the doc no longer names the remove-all-data command');
}

// ---------------------------------------------------------------- platform truth (D5/D6)
const readme = read('README.md');
need(/badge\/platforms-Windows/.test(readme), 'D5/D6: the README platform badge no longer names Windows');
need(!/badge\/platforms-Windows%2011%20%7C%20macOS%20%7C%20Linux/.test(readme),
  'D5/D6: the README badge claims macOS/Linux ship again — they are out of v1 scope');
need(/out of v1 scope/i.test(readme), 'D6: the README no longer states that macOS / native Linux desktop are out of v1 scope');
need(/not yet qualified|P70\.E8/i.test(readme), 'D6: the README no longer states the Windows-not-yet-qualified position');

const matrix = read('SUPPORT-MATRIX.md');
need(matrix.includes('## 1.'), 'SUPPORT-MATRIX §1 is gone');
need(/P70\.D6/.test(matrix) || /Windows-first|out of v1/i.test(matrix), 'SUPPORT-MATRIX no longer states the platform scope');

// ---------------------------------------------------------------- result
if (problems.length) {
  console.error('check-diagnostics-surface: FAILED');
  for (const p of problems) console.error(`  ✗ ${p}`);
  process.exit(1);
}
console.log('check-diagnostics-surface: ok (D1/D4/D5/D6/D7/D8/D9 surfaces agree)');
