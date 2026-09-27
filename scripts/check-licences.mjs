#!/usr/bin/env node
// P70.B6 — third-party licence compliance.
//
// The shipped binaries are MIT OR Apache-2.0; everything they link against or
// bundle must be checked for obligations that affect distribution. This gate:
//
//   1. inventories every dependency licence from the lockfiles (Rust: the
//      crates.io index metadata; npm: the lockfile's `license` field plus a
//      package.json lookup in node_modules when present);
//   2. **fails on copyleft licences** that would impose obligations the
//      distribution model cannot carry (GPL/AGPL family, and LGPL for
//      statically-linked Rust crates) — the release cannot ship until such a
//      dependency is replaced, or a row records an explicit decision;
//   3. requires the notices file (`THIRD-PARTY-NOTICES.md`) to exist and to
//      carry the licences actually found — it is regenerated with
//      `--write-notices`, and the gate fails when it is stale;
//   4. keeps the previously flagged licence-boundary items visible (the
//      source-available document-skills boundary, LLMLingua-2 MIT).
//
// Usage:
//   node scripts/check-licences.mjs                    // gate mode
//   node scripts/check-licences.mjs --write-notices    // regenerate the notices file
//
// Exit 0 = every licence is allow-listed and the notices file is current.

import { readFileSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const NOTICES = 'THIRD-PARTY-NOTICES.md';

const argv = process.argv.slice(2);
const writeNotices = argv.includes('--write-notices');

// SPDX expressions this distribution model can carry. Everything else needs an
// explicit row in DECIDED below before the gate passes.
const ALLOWED = new Set([
  'MIT', 'MIT-0', 'Apache-2.0', 'BSD-2-Clause', 'BSD-3-Clause', 'BSD-3-Clause-Clear',
  'ISC', '0BSD', 'Zlib', 'CC0-1.0', 'Unlicense', 'WTFPL', 'BlueOak-1.0.0',
  'Python-2.0', 'MPL-2.0', 'BSL-1.0', 'MIT OR Apache-2.0', 'Apache-2.0 OR MIT',
  'MIT/Apache-2.0', 'Apache-2.0/MIT', 'MIT OR Apache-2.0 OR LGPL-2.1-or-later',
]);

// Licence fragments that are disqualifying even inside a longer expression,
// unless the exact combination is allow-listed.
const COPYLEFT = /(GPL-[123](\.\d)?|AGPL-[123](\.\d)?|SSPL|EUPL|OSL|CDDL|EPL-2?\.0|Sleepycat)/;

/**
 * An SPDX expression is acceptable when **at least one arm** (split on the
 * top-level `OR`) is allow-listed — the distributor may choose which licence
 * applies — and no arm is copyleft. `AND` (all arms apply) and parenthesised
 * expressions fall back to exact-string or DECIDED matching.
 */
function licenceAcceptable(raw) {
  let licence = normalize(raw);
  if (!licence) return false;
  if (COPYLEFT.test(licence)) return false;
  if (ALLOWED.has(licence)) return true;
  // Legacy slash spellings: `A / B`, `A/B`, `A OR B` — all mean the
  // distributor's choice of one arm.
  licence = licence.replace(/\s*\/\s*/g, ' OR ');
  if (ALLOWED.has(licence)) return true;
  // Parenthesised groups: `(A OR B) AND C` is acceptable when C is allowed and
  // at least one arm of the group is allowed (choose the group's permissive arm).
  const groupAnd = /^\(([^)]+)\)\s+AND\s+(.+)$/.exec(licence);
  if (groupAnd) {
    const [, group, tail] = groupAnd;
    if (!ALLOWED.has(tail.trim()) || COPYLEFT.test(tail)) return false;
    return group.split(/\s+OR\s+/).some((a) => ALLOWED.has(a.trim()) && !COPYLEFT.test(a));
  }
  // Top-level OR split.
  const arms = licence.split(/\s+OR\s+/);
  if (arms.length > 1) {
    return arms.every((a) => !COPYLEFT.test(a)) && arms.some((a) => ALLOWED.has(a.trim()));
  }
  return false;
}

// Decisions recorded where a dependency's licence is not in the allow-list but
// has been examined and accepted, with the reason. A dependency that shows up
// here is audited, not waved through.
const DECIDED = new Map([
  // CC-BY-4.0: caniuse-lite is browser-compatibility *data* consumed at
  // build time (browserslist), never shipped in the binary.
  ['CC-BY-4.0', 'build-time compatibility data (browserslist), not distributed'],
  // Dual/expression licences whose members are all allow-listed.
  ['(MPL-2.0 OR Apache-2.0)', 'both arms allow-listed; the user may choose the Apache arm'],
  ['MIT AND ISC', 'both arms allow-listed'],
  ['(MIT OR CC-BY-3.0)', 'MIT arm chosen'],
  // ICU4X data files are distributed under the Unicode licence alongside the
  // permissive code licence — the Unicode licence is file-based data with
  // attribution satisfied by the notices file.
  ['(MIT OR Apache-2.0) AND Unicode-DFS-2016', 'Unicode data files: attribution carried by the notices file'],
  ['Unicode-3.0', 'Unicode data licence: permissive with attribution, carried by the notices file'],
  ['(MIT OR Apache-2.0) AND Unicode-3.0', 'Unicode data files: attribution carried by the notices file'],
  ['Apache-2.0 WITH LLVM-exception', 'LLVM exception broadens the Apache-2.0 grant; not copyleft'],
  ['CDLA-Permissive-2.0', 'webpki-roots: permissive data licence (root certificates), attribution in the notices file'],
  // r-efi (UEFI target support, pulled by `getrandom` for the uefi target) is
  // tri-licensed MIT OR Apache-2.0 OR LGPL-2.1-or-later: the distributor
  // chooses the MIT or Apache arm, so no LGPL obligation arises. The crate is
  // also never linked on the Windows/Linux/macOS targets v1 ships.
  ['MIT OR Apache-2.0 OR LGPL-2.1-or-later', 'MIT/Apache arms chosen; LGPL arm never used — and the crate is UEFI-target only'],
  ['MIT OR Apache-2.0 OR BSD-1-Clause', 'permissive arms chosen (fiat-crypto)'],
  // AND-joined expressions where every arm is allow-listed: all arms apply,
  // and every arm is one we can satisfy.
  ['Apache-2.0 AND ISC', 'ring: both arms allow-listed'],
  ['BSD-3-Clause AND MIT', 'brotli: both arms allow-listed'],
  ['Apache-2.0 AND MIT', 'dpi: both arms allow-listed'],
  // model2vec-rs ships `license-file = LICENSE` (MIT text) rather than an SPDX
  // id; the file itself states MIT.
  ['license-file: MIT (model2vec-rs)', 'LICENSE file in the crate states MIT'],
  ['(Apache-2.0 WITH LLVM-exception) OR Apache-2.0 OR MIT', 'LLVM exception is more permissive, not less'],
  // Add rows here — with the reason — rather than widening ALLOWED.
]);

const failures = [];
const fail = (msg) => failures.push(msg);

/** SPDX string → one entry {id, licence, source}. */
function normalize(raw) {
  return String(raw ?? '').trim();
}

/**
 * crates.io's API licence field, with a small in-process cache. Returns null
 * on any failure — the caller then records the crate as unresolved rather
 * than inventing a licence.
 */
const cratesIoCache = new Map();
let cratesIoUnavailable = false;
async function fetchCratesIoLicence(name, version) {
  const key = `${name}@${version}`;
  if (cratesIoUnavailable) return null;
  if (cratesIoCache.has(key)) return cratesIoCache.get(key);
  let licence = null;
  // AbortSignal.timeout uses an unref'd timer on supported Node releases. Keep
  // a referenced timer for the request so an offline runner exits through the
  // normal unresolved-licence failure instead of Node's unsettled-TLA exit 13.
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10_000);
  try {
    const res = await fetch(`https://crates.io/api/v1/crates/${encodeURIComponent(name)}/${encodeURIComponent(version)}`, {
      headers: { 'user-agent': 'agentcowork-licence-gate (release compliance check)' },
      signal: controller.signal,
    });
    if (res.ok) {
      const body = await res.json();
      licence = normalize(body?.version?.license) || null;
    } else if (res.status !== 404) {
      cratesIoUnavailable = true;
    }
  } catch {
    // offline / rate-limited: stop repeated network attempts and fall through
    // to the actionable unresolved-licence result.
    cratesIoUnavailable = true;
  } finally {
    clearTimeout(timeout);
  }
  cratesIoCache.set(key, licence);
  return licence;
}

function cargoRegistryRoots() {
  const registrySrc = join(process.env.HOME ?? '', '.cargo/registry/src');
  try {
    return readdirSync(registrySrc).map((entry) => join(registrySrc, entry));
  } catch (err) {
    // A clean CI runner may not have fetched crate sources yet. Leave the local
    // source set empty so the crates.io fallback can still resolve metadata;
    // anything unresolved is inventoried and fails the gate.
    if (err?.code === 'ENOENT') return [];
    throw err;
  }
}

async function collectRust() {
  // crates.io licence metadata lives in the cargo registry cache. Where it is
  // absent (no local cache), the component is reported with licence null and
  // the notices file carries it under 'unknown' — never silently skipped.
  const out = [];
  const locks = ['crates/Cargo.lock', 'src-tauri/Cargo.lock'];
  const seen = new Set();
  const registryRoots = cargoRegistryRoots();
  for (const lock of locks) {
    const raw = readFileSync(join(root, lock), 'utf8');
    for (const block of raw.split('[[package]]').slice(1)) {
      const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(block)?.[1];
      const version = /^\s*version\s*=\s*"([^"]+)"/m.exec(block)?.[1];
      const source = /^\s*source\s*=\s*"([^"]+)"/m.exec(block)?.[1];
      if (!name || !version || !source) continue; // path deps are first-party
      const key = `${name}@${version}`;
      if (seen.has(key)) continue;
      seen.add(key);
      let licence = null;
      for (const reg of registryRoots) {
        const dir = join(reg, `${name}-${version}`);
        // Prefer Cargo.toml.orig (the pre-workspace-inheritance manifest —
        // workspace crates write `license.workspace = true`, which resolves to
        // nothing locally), then the rewritten Cargo.toml.
        const candidates = ['Cargo.toml.orig', 'Cargo.toml'];
        for (const file of candidates) {
          const m = join(dir, file);
          if (!existsSync(m)) continue;
          const txt = readFileSync(m, 'utf8');
          let mm = /^\s*license\s*=\s*"([^"]+)"/m.exec(txt);
          if (!mm) mm = /^\s*license\s*=\s*(?:'([^']+)'|<([^>]+)>)/m.exec(txt);
          if (mm) {
            const value = normalize(mm[1] ?? mm[2]);
            if (value && !/workspace\s*=\s*true/.test(value)) {
              licence = value;
              break;
            }
          }
        }
        if (licence) break;
      }  // Vendored but unreadable happens with `license.workspace = true` crates
  // whose .orig is absent — the crates.io API is the authoritative source.
  // Offline (CI air-gapped or fetch failure) → stays null and is
  // inventoried as unresolved, never guessed.
      if (!licence) {
        // crates.io's record is authoritative for any registry crate, vendored
        // locally or not (target-specific deps of other platforms are usually
        // NOT in this machine's cache).
        const api = await fetchCratesIoLicence(name, version);
        // `non-standard` is crates.io's word for a license-file-only crate —
        // not a licence; fall through to reading the vendored LICENSE file.
        if (api && api !== 'non-standard') licence = api;
      }
      // A `license-file` (no SPDX id) is only accepted when the vendored file
      // itself begins with a recognizable permissive licence text.
      if (!licence) {
        for (const reg of registryRoots) {
          const licenseFile = join(reg, `${name}-${version}`, 'LICENSE');
          if (existsSync(licenseFile)) {
            const head = readFileSync(licenseFile, 'utf8').slice(0, 400);
            if (head.includes('Permission is hereby granted, free of charge')) {
              licence = `license-file: MIT (${name})`;
              DECIDED.set(licence, `LICENSE file in the crate states MIT`);
              break;
            }
          }
        }
      }
      out.push({ name, version, licence, lock });
    }
  }
  return out;
}

function collectNpmFromLock(lockPath, out) {
  const abs = join(root, lockPath);
  if (!existsSync(abs)) return;
  const lock = JSON.parse(readFileSync(abs, 'utf8'));
  for (const [key, rec] of Object.entries(lock.packages ?? {})) {
    if (!key || !rec.version) continue;
    const name = key.replace(/^node_modules\//, '').split('node_modules/').pop();
    if (rec.license) out.push({ name, version: rec.version, licence: normalize(rec.license), lock: lockPath });
  }
}

function collectNpmWorkspace() {
  const out = [];
  const lockFiles = [];
  if (existsSync(join(root, 'ui/package-lock.json'))) lockFiles.push('ui/package-lock.json');
  collectNpmFromLock('ui/package-lock.json', out);
  // The pnpm workspace's packages declare licences in their manifests.
  const pkgDirs = join(root, 'packages');
  for (const entry of readdirSync(pkgDirs, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const manifestPath = join(pkgDirs, entry.name, 'package.json');
    const nm = join(pkgDirs, entry.name, 'node_modules');
    if (!existsSync(manifestPath)) continue;
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
    out.push({
      name: manifest.name ?? entry.name,
      version: manifest.version ?? '0',
      licence: manifest.license ?? 'SEE LICENSE IN LICENSE',
      lock: `packages/${entry.name}/package.json`,
    });
    // First-party transitive deps for this package, via node_modules manifests.
    if (existsSync(nm)) {
      for (const dep of readdirSync(nm, { withFileTypes: true })) {
        if (!dep.isDirectory()) continue;
        const scopeDir = dep.name.startsWith('@') ? dep.name : null;
        if (scopeDir) {
          for (const inner of readdirSync(join(nm, dep.name), { withFileTypes: true })) {
            const mp = join(nm, dep.name, inner.name, 'package.json');
            if (existsSync(mp)) {
              const m = JSON.parse(readFileSync(mp, 'utf8'));
              out.push({ name: m.name ?? `${dep.name}/${inner.name}`, version: m.version ?? '', licence: m.license ?? null, lock: `packages/${entry.name}/node_modules` });
            }
          }
          void scopeDir;
        } else {
          const mp = join(nm, dep.name, 'package.json');
          if (existsSync(mp)) {
            const m = JSON.parse(readFileSync(mp, 'utf8'));
            out.push({ name: m.name ?? dep.name, version: m.version ?? '', licence: m.license ?? null, lock: `packages/${entry.name}/node_modules` });
          }
        }
      }
    }
    void lockFiles;
  }
  return out;
}

// --- collect -----------------------------------------------------------------
const rust = await collectRust();
const js = collectNpmWorkspace();
const all = [...rust, ...js];

const unknown = [];
const copyleft = [];
const decided = [];
const licences = new Map(); // licence -> names
for (const dep of all) {
  const licence = dep.licence;
  if (!licence) {
    unknown.push(dep);
    continue;
  }
  if (DECIDED.has(licence)) {
    decided.push(dep);
    continue;
  }
  if (licenceAcceptable(licence)) {
    // Group under the licence string the *project* relies on, so the notices
    // file stays readable instead of listing every arm permutation.
    const key = licence.includes('OR') ? licence : licence;
    licences.set(key, [...(licences.get(key) ?? []), `${dep.name}@${dep.version}`]);
    continue;
  }
  if (COPYLEFT.test(licence)) {
    copyleft.push(dep);
    continue;
  }
  unknown.push(dep);
}

for (const dep of copyleft) {
  fail(
    `copyleft licence ${dep.licence} on ${dep.name}@${dep.version} (${dep.lock}) — a distribution-model decision ` +
      '(replace the dependency, or record an explicit row in DECIDED with the reason) is required before shipping',
  );
}

// Licences the local registry could not resolve: either the crate source is
// not vendored on this machine (target-specific deps of other platforms —
// normal) or the metadata really is absent. In both cases the licence must
// still be inventoried, so these are emitted into the notices file under
// "unresolved on this machine" and the gate keeps them visible. A REAL
// unknown-licence problem shows up as an npm dependency (whose lockfile always
// carries `license`) or as an unresolved crate with no lockfile source at all.
const unresolvedRust = unknown.filter((d) => d.lock.endsWith('Cargo.lock'));
const unknownNpm = unknown.filter((d) => !d.lock.endsWith('Cargo.lock'));
for (const dep of unknownNpm) {
  fail(
    `licence not classified: ${dep.name}@${dep.version} reports ${JSON.stringify(dep.licence)} (${dep.lock}) — ` +
      'npm lockfiles carry licence metadata; fix or pin the dependency',
  );
}

function resolveUnresolvedRust() {
    // A crate whose source IS vendored locally must have readable licence
    // metadata — if it does not, that is a gate failure, not a note. A crate
    // that is NOT vendored locally is a target-specific dependency of another
    // platform (e.g. `block2` for macOS): the crates.io API already resolved
    // its licence when it could, so what remains here was also unresolved
    // remotely — inventoried in the notices file under 'unresolved', and
    // failing the gate, because a release cannot ship with any unknown.
    const missing = [];
    const registryRootsLocal = cargoRegistryRoots();
    for (const dep of unresolvedRust) {
      const vendored = registryRootsLocal.some((reg) =>
        existsSync(join(reg, `${dep.name}-${dep.version}`, 'Cargo.toml.orig')) ||
        existsSync(join(reg, `${dep.name}-${dep.version}`, 'Cargo.toml')));
      if (vendored) {
        fail(
          `licence metadata unreadable for the locally-vendored crate ${dep.name}@${dep.version} — ` +
            'inspect its Cargo.toml manually and allow-list or record a decision',
        );
      } else {
        missing.push(dep);
      }
    }
    return missing;
  }
  const unresolved = resolveUnresolvedRust();
  if (unresolved.length > 0 && !writeNotices) {
    fail(
      `${unresolved.length} component(s) have no resolvable licence (not vendored locally and no crates.io record) — ` +
        'the release cannot carry an unknown licence; make crates.io reachable or run `cargo fetch` before this gate, then rerun',
    );
  }

// --- the notices file ---------------------------------------------------------
const noticesPath = join(root, NOTICES);
if (!existsSync(noticesPath)) {
  fail(`${NOTICES} is missing — the notices file must ship with the binaries (P70.B6)`);
} else if (!writeNotices) {
  const notices = readFileSync(noticesPath, 'utf8');
  for (const licence of licences.keys()) {
    if (!notices.includes(licence)) {
      fail(`${NOTICES} does not mention licence \`${licence}\` (found on ${licences.get(licence).slice(0, 2).join(', ')}) — regenerate with --write-notices`);
      break;
    }
  }
  for (const [needle, why] of [
    ['MIT OR Apache-2.0', 'the workspace licence claim'],
    ['LLMLingua-2', 'the flagged research-transfer licence boundary'],
  ]) {
    if (!notices.includes(needle)) fail(`${NOTICES}: ${why} is no longer stated`);
  }
}

if (writeNotices) {
  const lines = [
    '# Third-party notices',
    '',
    '> Generated by `scripts/check-licences.mjs --write-notices` from the lockfiles. Do not edit by hand;',
    '> re-run the generator. The AgentCowork sources are dual-licensed MIT OR Apache-2.0 — see `LICENSE`.',
    '',
    '## Licence boundary decisions already recorded',
    '',
    '- **LLMLingua-2** (semantic distillation research, `DESKTOP-APP-SPEC` P39) is **MIT** — its papers/models are',
    '  referenced, not bundled; the implementation here is original.',
    '- **Source-available document skills** (anthropic research doc 75) are referenced as *formats to read/write*,',
    '  not bundled code — no redistribution obligation arises.',
    '- Workspace dependencies (`path:` crates, `packages/core-*`) are first-party and carry the workspace licence.',
    '',
    '## Dependencies by licence',
    '',
  ];
  for (const [licence, names] of [...licences].sort()) {
    lines.push(`### ${licence}`, '');
    const unique = [...new Set(names)];
    lines.push(...unique.map((n) => `- ${n}`));
    lines.push('');
  }
  if (unresolved.length > 0) {
    lines.push('### Unresolved on this machine (crate source not vendored locally — target-specific deps)', '');
    lines.push(...unresolved.map((d) => `- ${d.name}@${d.version} (${d.lock})`));
    lines.push('');
  }
  if (unknownNpm.length > 0) {
    lines.push('### Unknown / unclassified (must be resolved before release)', '');
    lines.push(...unknownNpm.map((d) => `- ${d.name}@${d.version} (${d.lock})`));
    lines.push('');
  }
  writeFileSync(noticesPath, `${lines.join('\n')}\n`);
  console.log(`check-licences: wrote ${NOTICES} — ${licences.size} licence(s), ${unresolved.length} unresolved locally, ${unknownNpm.length} unclassified`);
  if (failures.length === 0) process.exit(0);
}

if (failures.length > 0) {
  console.error('check-licences: FAIL');
  for (const f of failures.slice(0, 30)) console.error(`  - ${f}`);
  if (failures.length > 30) console.error(`  … and ${failures.length - 30} more`);
  process.exit(1);
}
console.log(
  `check-licences: PASS — ${all.length} component(s): ${licences.size} licence(s) allow-listed, ` +
    `${decided.length} on recorded decisions, 0 copyleft, ${unresolved.length} unresolved-locally (inventoried), ` +
    `${unknownNpm.length} unclassified`,
);
