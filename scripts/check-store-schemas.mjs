#!/usr/bin/env node
// P70.A8 — durable-store schema stamps, statically enforced.
//
// The Rust registry (`crates/agentcowork-core/src/store_schema.rs`) is what runs
// at boot; this gate keeps it true. It fails when:
//
//   1. a `join("….db" | "….jsonl" | "….ndjson" | "….sqlite" | "….json")`
//      persistence path appears in shipping Rust code without being classified —
//      either as a **durable store** in the registry, or explicitly as a
//      **derived** artifact (a rebuildable cache / observability output) or
//      **config** (user configuration, not state). The point is that a new
//      durable file cannot appear without someone answering "does an upgrade
//      have to preserve this?";
//   2. a registered store is missing from the published table in
//      `PACKAGING.md` §6, or the two disagree on a version;
//   3. the registry's `vault` row disagrees with `agentcowork-vault`'s own
//      `SCHEMA_VERSION` constant (the authority for that store), or the vault
//      crate has lost its refuse-a-newer-schema path;
//   4. the registry's boot wiring is gone (the module is not exported, or
//      `boot()` no longer stamps the stores).
//
// Read-only; no arguments.

import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, basename } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const failures = [];
const fail = (msg) => failures.push(msg);

const REGISTRY = 'crates/agentcowork-core/src/store_schema.rs';
const VAULT = 'crates/agentcowork-vault/src/lib.rs';

// Derived artifacts: rebuildable from a source of truth, so losing one costs a
// rebuild rather than data. Each entry states the source it derives from.
const DERIVED = new Map([
  ['index.sqlite', 'audit session-replay index — rebuilt from audit.ndjson'],
  ['fts.sqlite', 'filename/full-text search index — rebuilt by re-indexing'],
  ['idx.sqlite', 'code-intelligence index — rebuilt by re-indexing'],
  ['.agentcowork-fclones.sqlite', 'dedup hash cache — recomputed from the filesystem'],
  ['registry.json', 'available-agent registry snapshot — refetched from the registry'],
  ['registry.meta.json', 'registry snapshot metadata — refetched with the snapshot'],
  ['tool_log.jsonl', 'per-session ACP tool log — diagnostic output, never read back into context'],
  ['traces.ndjson', 'trace export — observability output'],
  ['meta.json', 'seek-index metadata — rebuilt with the index'],
  ['index.json', 'model index — rebuilt from the models directory'],
  ['OWNERSHIP.json', 'agent-install ownership manifest — rewritten by the installer'],
  ['.installed.json', 'skill-store install marker — rewritten on install'],
  ['repo_cache.db', 'code-intelligence repository cache — see the registry (Derived)'],
  ['plans.db', 'blueprint plan cache — see the registry (Derived)'],
]);

// User configuration, not state: nothing to migrate, nothing to preserve
// across an upgrade beyond leaving it alone.
const CONFIG = new Set([
  'acpx.json',
  'agent_backend.json',
  'browser_config.json',
  'default_model.json',
  'desktop.json',
  'mcp_servers.json',
  'providers.json',
  'search.json',
  'settings.json',
  'sync-state.json',
  'workspace_trust.json',
  'risk_overrides.json',
  '.lsp.json',
  '.mcp.json',
  '.step.json',
  'validator.json',
]);

// ---------------------------------------------------------------------------
// 1. The registry.
// ---------------------------------------------------------------------------
if (!existsSync(join(root, REGISTRY))) {
  console.error(`store-schemas: FAIL — ${REGISTRY} is missing (P70.A8 has no registry)`);
  process.exit(1);
}
const registrySrc = readFileSync(join(root, REGISTRY), 'utf8');

/** Parse `StoreSpec { name, path, version, policy }` rows out of the registry. */
const rows = [];
for (const block of registrySrc.split('StoreSpec {').slice(1)) {
  const body = block.split('},')[0];
  const name = /name:\s*"([^"]+)"/.exec(body)?.[1];
  const path = /path:\s*"([^"]+)"/.exec(body)?.[1];
  const version = Number(/version:\s*(\d+)/.exec(body)?.[1]);
  const policy = /policy:\s*StorePolicy::(\w+)/.exec(body)?.[1];
  if (name && path && Number.isFinite(version) && policy) rows.push({ name, path, version, policy });
}
if (rows.length === 0) {
  fail(`${REGISTRY}: no StoreSpec rows parsed — the registry shape changed and this gate is blind`);
}
// Match on the file name a row names: strip a trailing parenthetical, drop
// wildcard directory segments, take the last segment.
const registeredPaths = new Set(
  rows.map((r) => {
    const segments = r.path
      .split(' ')[0]
      .split('/')
      .filter((s) => s && !s.includes('*'));
    return segments[segments.length - 1];
  }),
);

// ---------------------------------------------------------------------------
// 2. Every persistence path in shipping Rust code is classified.
// ---------------------------------------------------------------------------
function rustFiles(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'tests') continue; // integration tests are not shipped paths
      rustFiles(p, out);
    } else if (entry.name.endsWith('.rs')) out.push(p);
  }
  return out;
}

const crateDir = join(root, 'crates');
const sources = [
  ...rustFiles(join(root, 'src-tauri/src')),
  ...readdirSync(crateDir, { withFileTypes: true })
    .filter((e) => e.isDirectory() && e.name.startsWith('agentcowork-'))
    .flatMap((e) => rustFiles(join(crateDir, e.name, 'src'))),
];

/** Drop a file's bottom `mod tests` block (repo convention: tests go last). */
function shippingCode(src) {
  const m = /^\s*(#\[cfg\(test\)\]\s*)?mod tests\b/m.exec(src);
  return m ? src.slice(0, m.index) : src;
}

const discovered = new Map(); // file name -> first site
for (const file of sources) {
  const rel = file.slice(root.length + 1);
  const lines = shippingCode(readFileSync(file, 'utf8')).split('\n');
  lines.forEach((line, i) => {
    if (!/join\(/.test(line)) return;
    for (const m of line.matchAll(/"([A-Za-z0-9_./-]+\.(?:db|sqlite|ndjson|jsonl|json))"/g)) {
      const literal = m[1];
      if (literal === 'package.json') continue; // not ours — read, never written
      if (!discovered.has(literal)) discovered.set(literal, `${rel}:${i + 1}`);
    }
  });
}

for (const [literal, site] of [...discovered].sort()) {
  const name = basename(literal);
  if (registeredPaths.has(name)) continue;
  if (DERIVED.has(name)) continue;
  if (CONFIG.has(name)) continue;
  fail(
    `unclassified persistence path \`${literal}\` (first seen at ${site}) — add it to the registry in ` +
      `${REGISTRY} as a durable store (with its schema version and what an upgrade must preserve), or ` +
      'classify it as derived/config in this gate with a one-line reason',
  );
}

// ---------------------------------------------------------------------------
// 3. The vault's own constant is the authority for that store.
// ---------------------------------------------------------------------------
if (!existsSync(join(root, VAULT))) {
  fail(`${VAULT} is missing — the vault's schema version cannot be cross-checked`);
} else {
  const vaultSrc = readFileSync(join(root, VAULT), 'utf8');
  const constant = Number(/const SCHEMA_VERSION:\s*\w+\s*=\s*(\d+)/.exec(vaultSrc)?.[1]);
  const row = rows.find((r) => r.name === 'vault');
  if (!Number.isFinite(constant)) {
    fail(`${VAULT}: no \`const SCHEMA_VERSION\` found — the registry cannot be cross-checked`);
  } else if (!row) {
    fail(`${REGISTRY}: the vault has no registry row`);
  } else if (row.version !== constant) {
    fail(
      `${REGISTRY}: the vault row claims v${row.version} but agentcowork-vault's SCHEMA_VERSION is ` +
        `v${constant} — the two must move together`,
    );
  }
  if (!/NewerSchema/.test(vaultSrc)) {
    fail(`${VAULT}: the refuse-a-newer-schema path (\`NewerSchema\`) is gone — forward-only is no longer enforced for the vault`);
  }
  const calendar = rows.find((r) => r.name === 'calendar');
  if (!calendar) fail(`${REGISTRY}: the calendar tables have no registry row (they live in the vault database)`);
  else if (calendar.version !== constant) {
    fail(`${REGISTRY}: the calendar row claims v${calendar.version}, but the calendar tables live in vault.db (v${constant})`);
  }
}

// ---------------------------------------------------------------------------
// 4. Boot wiring + the published table.
// ---------------------------------------------------------------------------
const coreLib = join(root, 'crates/agentcowork-core/src/lib.rs');
const coreSrc = existsSync(coreLib) ? readFileSync(coreLib, 'utf8') : '';
if (!/pub mod store_schema;/.test(coreSrc)) {
  fail('crates/agentcowork-core/src/lib.rs: the store_schema module is not exported');
}
if (!/store_schema::ensure_all\(/.test(coreSrc)) {
  fail(
    'crates/agentcowork-core/src/lib.rs: `boot()` no longer calls `store_schema::ensure_all(..)` — the stamps ' +
      'would never be written or checked on a real install',
  );
}

const packagingPath = join(root, 'PACKAGING.md');
if (!existsSync(packagingPath)) {
  fail('PACKAGING.md is missing — the store table has no published form');
} else {
  const doc = readFileSync(packagingPath, 'utf8');
  const section = doc.split('## 6. Durable stores')[1]?.split('\n## ')[0] ?? '';
  if (!section) {
    fail('PACKAGING.md: the "Durable stores" section (§6) is missing');
  } else {
    for (const row of rows) {
      const line = section
        .split('\n')
        .find((l) => l.trim().startsWith('|') && l.includes(`\`${row.name}\``));
      if (!line) {
        fail(`PACKAGING.md §6 is missing a row for the registered store \`${row.name}\``);
        continue;
      }
      if (!new RegExp(`\\bv${row.version}\\b`).test(line)) {
        fail(`PACKAGING.md §6: the \`${row.name}\` row does not state v${row.version}`);
      }
    }
  }
}

if (failures.length > 0) {
  console.error('store-schemas: FAIL');
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
const durable = rows.filter((r) => r.policy !== 'Derived').length;
console.log(
  `store-schemas: PASS — ${rows.length} registered store(s) (${durable} stamped, ${
    rows.length - durable
  } derived), ${discovered.size} persistence path(s) classified, vault at v${
    rows.find((r) => r.name === 'vault')?.version ?? '?'
  }`,
);
