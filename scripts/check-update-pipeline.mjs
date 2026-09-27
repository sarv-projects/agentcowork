#!/usr/bin/env node
// P70.C7 — update-pipeline gate.
//
// The updater story spans four files that the compiler cannot relate:
//   * src-tauri/src/updater_cmds.rs  — channels, endpoints, event name
//   * src-tauri/tauri.conf.json      — plugin config, pubkey, endpoints
//   * docs/updating.md               — the published contract
//   * crates/agentcowork-core/src/store_schema.rs — update_channel.json stamp
//
// This gate fails the build when any of those drift apart, so the published
// `docs/updating.md` cannot silently stop describing the shipping binary.

import { readFileSync, existsSync } from 'node:fs';

const read = (p) => readFileSync(p, 'utf8');

const problems = [];
const fail = (msg) => problems.push(msg);

// ---------------------------------------------------------------- updater_cmds.rs
const cmds = read('src-tauri/src/updater_cmds.rs');

// The channel vocabulary is exactly two values, in code.
const chans = [...cmds.matchAll(/"(\w+)"/g)].map((m) => m[1]);
for (const c of ['stable', 'beta']) {
  if (!chans.includes(c)) fail(`updater_cmds.rs lost channel "${c}"`);
}

// The persisted channel file and its atomic-write contract.
if (!cmds.includes('update_channel.json')) fail('updater_cmds.rs no longer persists update_channel.json');
if (!cmds.includes('fs::rename')) fail('updater_cmds.rs channel write is no longer atomic (temp file + rename)');

// Channel-aware endpoint selection: the hosted base must exist, and the
// stable channel (the catch-all arm) must include the static GitHub
// latest.json fallback while beta must NOT (C2).
const hostedBase = 'releases.everyaios.dev';
if (!cmds.includes(hostedBase)) fail(`updater_cmds.rs lost the hosted base ${hostedBase}`);
const stableArm = /_\s*=>\s*(?:vec!)?\[[\s\S]{0,600}?\]/.exec(cmds);
if (!stableArm || !stableArm[0].includes('GITHUB_FALLBACK_ENDPOINT'))
  fail('stable channel (catch-all arm) no longer includes the GitHub fallback');
const betaArm = /"beta"\s*=>\s*vec!\[[\s\S]{0,600}?\]/.exec(cmds);
if (betaArm && betaArm[0].includes('GITHUB_FALLBACK'))
  fail('beta channel must not consult the static GitHub latest.json fallback');

// Boot/periodic auto-check (C1) and progress events (C1/C4).
if (!cmds.includes('spawn_periodic_check')) fail('boot + periodic auto-check (spawn_periodic_check) is gone');
if (!cmds.includes('updater-status')) fail('the updater-status event name changed; docs/updating.md §4 and the UI listen for it');

// ---------------------------------------------------------------- tauri.conf.json
const conf = JSON.parse(read('src-tauri/tauri.conf.json'));
const endpoints = conf?.plugins?.updater?.endpoints ?? [];
if (!endpoints.some((e) => e.includes(hostedBase)))
  fail(`tauri.conf.json updater endpoints lost the hosted base ${hostedBase}`);
if (!endpoints.some((e) => e.includes('github.com')))
  fail('tauri.conf.json updater endpoints lost the GitHub fallback');
if (!conf?.plugins?.updater?.pubkey) fail('tauri.conf.json lost plugins.updater.pubkey (the trust anchor)');
if (!conf?.plugins?.updater?.windows?.installMode)
  fail('tauri.conf.json lost plugins.updater.windows.installMode');
if (conf?.plugins?.updater?.windows?.installMode !== 'passive')
  fail(`updater installMode drifted: expected "passive", got "${conf?.plugins?.updater?.windows?.installMode}"`);

// The conf's first endpoint must match the code's channel scheme prefix so a
// future editor cannot change one side only. (Code builds
// `{base}/{channel}/{{target}}/{{arch}}/{{current_version}}`; conf shows the
// default channel without the segment.)
const confHosted = endpoints.find((e) => e.includes(hostedBase));
if (confHosted && !/releases\.everyaios\.dev\/\{\{target\}\}\/\{\{arch\}\}\/\{\{current_version\}\}/.test(confHosted))
  fail(`tauri.conf.json hosted endpoint shape drifted: "${confHosted}"`);

// ---------------------------------------------------------------- docs/updating.md
const doc = read('docs/updating.md');
for (const section of [
  '## 1. Manifest + hosting (C1)',
  '## 2. Channels (C2)',
  '## 3. Staged rollout + kill switch (C3)',
  '## 4. Update UX (C4)',
  '## 5. Upgrade data migration (C5)',
  '## 6. Downgrade + minimum-version policy (C6)',
  '## 7. Rollback drill (C7)',
]) {
  if (!doc.includes(section)) fail(`docs/updating.md lost "${section}"`);
}
if (!doc.includes(hostedBase)) fail('docs/updating.md no longer names the hosted manifest base');
if (!doc.includes('update_channel.json')) fail('docs/updating.md no longer names the channel store');
if (!doc.includes('TAURI_SIGNING_PRIVATE_KEY')) fail('docs/updating.md no longer documents the signing secret requirement');

// ---------------------------------------------------------------- store registry
const schema = read('crates/agentcowork-core/src/store_schema.rs');
if (!/name:\s*"update_channel"/.test(schema))
  fail('store_schema.rs no longer stamps update_channel.json (P70.C2 persistence must be a registered durable store)');

// ---------------------------------------------------------------- SUPPORT-MATRIX §4
const matrix = read('SUPPORT-MATRIX.md');
if (!matrix.includes('## 4. Data and upgrade policy'))
  fail('SUPPORT-MATRIX.md lost §4 (Data and upgrade policy) — the C6 minimum-version statement lives there');
if (!matrix.includes('P70.C6'))
  fail('SUPPORT-MATRIX.md §4 no longer references P70.C6');

// ---------------------------------------------------------------- result
if (problems.length) {
  console.error('check-update-pipeline: FAILED');
  for (const p of problems) console.error(`  ✗ ${p}`);
  process.exit(1);
}
console.log('check-update-pipeline: ok (C1–C7 surfaces agree)');
