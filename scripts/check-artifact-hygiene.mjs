#!/usr/bin/env node
// P70.B7 — secret-leak gate on PRODUCED artifacts.
//
// The source tree is checked by the invariant gates; this script exists
// because a leak can also happen at *packaging* time: an installer that
// sweeps a directory can pick up a `.env`, a dev SQLite database, a signing
// key, or a build log that never appears in git. It scans the bundle
// directories the tauri-action produced (installer + updater artifacts +
// manifest) and fails the release on any hit.
//
// Usage: node scripts/check-artifact-hygiene.mjs --paths "<dir>[,<dir>...]"
//   A `**` inside a path segment matches any depth (e.g.
//   `src-tauri/target/**/bundle`); plain directories are scanned as-is.
//   Missing paths / empty globs fail loudly rather than passing silently,
//   so a path typo cannot turn the gate into a no-op.

import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs';
import { join, extname, basename, sep } from 'node:path';

const BINARY_EXTS = new Set([
  '.exe', '.msi', '.dll', '.pdb', '.ico', '.png', '.jpg', '.woff', '.woff2',
  '.ttf', '.zip', '.gz', '.zst', '.sig', '.node',
]);

// Filenames that must never ship inside a bundle, whatever the extension.
const FORBIDDEN_NAMES = [
  /^\.env($|\..+)$/i, // .env, .env.local, .env.production …
  /\.pem$/i,
  /\.key$/i,
  /id_rsa|id_ed25519|id_ecdsa/i,
  /tauri_signing_private/i,
  /\.sqlite3?($|[-.])/i,
  /\.db$/i,
  /\.vault$/i,
  /\.log$/i,
];

// Content patterns scanned inside TEXT artifacts (NSIS/WiX scripts, the
// updater manifest, licence files, any straggler text that got swept).
const CONTENT_PATTERNS = [
  { re: /-----BEGIN (RSA |EC |OPENSSH |PGP |)PRIVATE KEY-----/, why: 'embedded private key block' },
  { re: /sk-[A-Za-z0-9_-]{20,}/, why: 'OpenAI-style API key literal' },
  { re: /ghp_[A-Za-z0-9]{20,}/, why: 'GitHub token literal' },
  { re: /AKIA[0-9A-Z]{16}/, why: 'AWS access key id literal' },
  { re: /TAURI_SIGNING_PRIVATE_KEY\s*=/, why: 'updater private key assignment' },
  // Both spellings: the product reads AGENTCOWORK_VAULT_KEY with an
  // EVERYAIOS_VAULT_KEY legacy fallback (DEC-053) — an artifact leaking
  // either assignment is a finding.
  { re: /(?:AGENTCOWORK|EVERYAIOS)_VAULT_KEY\s*=\s*\S+/, why: 'vault key assignment' },
];

function walk(dir, out) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return; // unreadable dir — recorded by the caller's existence check
  }
  for (const entry of entries) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) walk(p, out);
    else out.push(p);
  }
}

// Expand `a/**/b` to every existing `a/…/b` directory; anything else passes
// through unchanged. Returns { paths, existed } — existed=false fails the run.
function expandGlob(raw) {
  const star = raw.indexOf('**');
  if (star === -1) return { paths: [raw], existed: existsSync(raw) };

  const anchorRaw = raw.slice(0, star);
  // Anchor = the existing directory above the `**` (drop the partial segment).
  const anchor = anchorRaw.endsWith(sep) || anchorRaw.endsWith('/')
    ? anchorRaw.slice(0, -1)
    : anchorRaw.slice(0, anchorRaw.lastIndexOf(sep) > 0 ? anchorRaw.lastIndexOf(sep) : anchorRaw.length);
  const suffix = raw
    .slice(star)
    .replace(/^\*\*\//, '')
    .replace(/^\*\*/, '')
    .replace(/^\//, '');
  if (!existsSync(anchor)) return { paths: [], existed: false };

  const all = [];
  walk(anchor, all);
  const dirs = new Set(all.map((f) => join(f, '..')));
  if (suffix === '') return { paths: [anchor], existed: true };

  const hits = [...dirs].filter((d) => d.split(sep).join('/').endsWith(suffix.split(sep).join('/')));
  return { paths: hits, existed: hits.length > 0 };
}

function scanFile(path) {
  const hits = [];
  const name = basename(path);

  for (const bad of FORBIDDEN_NAMES) {
    if (bad.test(name)) hits.push(`forbidden filename pattern: ${name}`);
  }
  if (BINARY_EXTS.has(extname(path).toLowerCase())) return hits;

  // Size guard: a mis-swept directory could include a huge blob; cap the
  // content scan at 8 MiB per file.
  let st;
  try { st = statSync(path); } catch { return hits; }
  if (st.size > 8 * 1024 * 1024) {
    hits.push(`oversize text artifact (${(st.size / 1048576).toFixed(1)} MiB) — likely a mis-sweep`);
    return hits;
  }

  let text;
  try { text = readFileSync(path, 'utf8'); } catch { return hits; }
  for (const { re, why } of CONTENT_PATTERNS) {
    if (re.test(text)) hits.push(`${why}: ${name}`);
  }
  return hits;
}

const argIdx = process.argv.indexOf('--paths');
const raw = argIdx >= 0 ? process.argv[argIdx + 1] : process.argv[2];
if (!raw) {
  console.error('usage: check-artifact-hygiene.mjs --paths "<dir-or-glob>[,<dir-or-glob>...]"');
  process.exit(2);
}

const roots = raw.split(',').map((s) => s.trim()).filter(Boolean);
if (roots.length === 0) {
  console.error('no paths given — refusing to pass a gate that scanned nothing');
  process.exit(2);
}

const files = [];
for (const root of roots) {
  const { paths, existed } = expandGlob(root);
  if (!existed) {
    console.error(`artifact-hygiene: path matched nothing: ${root}`);
    process.exit(2);
  }
  for (const dir of paths) {
    const st = statSync(dir);
    if (st.isFile()) files.push(dir);
    else walk(dir, files);
  }
}

const failures = [];
for (const f of files) {
  failures.push(...scanFile(f).map((h) => `${f}: ${h}`));
}

console.log(`artifact-hygiene: scanned ${files.length} file(s) across ${roots.length} path(s)`);
if (failures.length > 0) {
  console.error('artifact-hygiene: FAIL — leaks found in produced artifacts:');
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log('artifact-hygiene: PASS — no key material, env files, dev databases or logs in the bundle');
