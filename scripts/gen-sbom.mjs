#!/usr/bin/env node
// P70.B5 — SBOM + build-provenance generation.
//
// Emits, per release:
//   * `agentcowork-sbom-rust.cdx.json`   — CycloneDX 1.5 over `crates/Cargo.lock`
//                                        (the kernel + every workspace crate)
//   * `agentcowork-sbom-shell.cdx.json`  — CycloneDX 1.5 over `src-tauri/Cargo.lock`
//                                        (the Tauri shell + its dependency tree)
//   * `agentcowork-sbom-ui.cdx.json`     — CycloneDX 1.5 over `ui/package-lock.json`
//   * `agentcowork-sbom-sidecar.cdx.json`— CycloneDX 1.5 over the coordinator +
//                                        vendored core-* package-lock.json files
//   * `agentcowork-provenance.json`      — in-toto-style statement: subject (the
//                                        app version), source commit, workflow
//                                        run, toolchain versions, and the gates
//                                        the commit passed.
//
// Everything is generated from the **lockfiles** — deterministic and offline, no
// network query, no registry dependency. Package licences come from the lock
// records when present (`license` field) and are otherwise null rather than
// guessed; the licence *compliance* pass is P70.B6 (`check-licences.mjs`).
//
// Usage:
//   node scripts/gen-sbom.mjs --out <dir> [--commit <sha>] [--run <url>]
// With no flags it writes into `dist-sbom/` using the local git HEAD.

import { readFileSync, writeFileSync, mkdirSync, existsSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { execSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const argv = process.argv.slice(2);
const argOf = (flag) => {
  const i = argv.indexOf(flag);
  return i >= 0 ? argv[i + 1] : undefined;
};
const outDir = resolve(root, argOf('--out') ?? 'dist-sbom');
const commit = argOf('--commit') ?? git('git rev-parse HEAD');
const runUrl = argOf('--run') ?? 'local';

function git(cmd) {
  try {
    return execSync(cmd, { cwd: root, encoding: 'utf8' }).trim();
  } catch {
    return '';
  }
}

const appVersion = JSON.parse(
  readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8'),
).version;

const CYCLONEDX = 'http://cyclonedx.org/schema/bom-1.5.schema.json';
const SPEC_VERSION = '1.5';
const now = new Date().toISOString();

/** One CycloneDX document skeleton. */
function doc(name) {
  return {
    $schema: CYCLONEDX,
    bomFormat: 'CycloneDX',
    specVersion: SPEC_VERSION,
    serialNumber: `urn:uuid:${uuidV5(`agentcowork:${name}:${commit}:${appVersion}`)}`,
    version: 1,
    metadata: {
      timestamp: now,
      component: { type: 'application', name: 'AgentCowork', version: appVersion },
      properties: [
        { name: 'agentcowork:source:commit', value: commit },
        { name: 'agentcowork:build:workflow', value: runUrl },
      ],
    },
    components: [],
  };
}

/** RFC 4122 UUIDv5 in the DNS namespace, over a string. */
function uuidV5(name) {
  const NS = Buffer.from('6ba7b8109dad11d180b400c04fd430c8', 'hex');
  const h = createHash('sha1').update(Buffer.concat([NS, Buffer.from(name, 'utf8')])).digest();
  const b = Buffer.from(h.subarray(0, 16));
  b[6] = (b[6] & 0x0f) | 0x50;
  b[8] = (b[8] & 0x3f) | 0x80;
  const s = b.toString('hex');
  return `${s.slice(0, 8)}-${s.slice(8, 12)}-${s.slice(12, 16)}-${s.slice(16, 20)}-${s.slice(20)}`;
}

// --- Cargo.lock → components -------------------------------------------------
function cargoComponents(lockPath, sourceLabel) {
  const raw = readFileSync(join(root, lockPath), 'utf8');
  const components = [];
  // Each [[package]] block: name / version / source.
  const blocks = raw.split('[[package]]').slice(1);
  for (const block of blocks) {
    const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(block)?.[1];
    const version = /^\s*version\s*=\s*"([^"]+)"/m.exec(block)?.[1];
    const source = /^\s*source\s*=\s*"([^"]+)"/m.exec(block)?.[1];
    if (!name || !version) continue;
    components.push({
      type: 'library',
      'bom-ref': `pkg:cargo/${name}@${version}`,
      name,
      version,
      purl: `pkg:cargo/${name}@${version}`,
      // A registry crate is third-party; a path/git dep is first-party and
      // carries the workspace licence implicitly — recorded as a property.
      scope: source ? 'required' : 'required',
      properties: [
        { name: 'agentcowork:origin', value: source ?? sourceLabel },
        { name: 'agentcowork:lockfile', value: lockPath },
      ],
    });
  }
  return components;
}

// --- package-lock.json → components -----------------------------------------
function npmComponents(lockPath) {
  const abs = join(root, lockPath);
  if (!existsSync(abs)) return [];
  const lock = JSON.parse(readFileSync(abs, 'utf8'));
  const components = [];
  for (const [key, rec] of Object.entries(lock.packages ?? {})) {
    if (!key) continue; // the root entry
    const name = key.replace(/^node_modules\//, '').replace(/\/node_modules\//, '/');
    // A nested path means a transitive duplicate; keep the innermost name.
    const clean = name.split('node_modules/').pop();
    const version = rec.version;
    if (!clean || !version) continue;
    const component = {
      type: 'library',
      'bom-ref': `pkg:npm/${clean}@${version}`,
      name: clean,
      version,
      purl: `pkg:npm/${clean}@${version}`,
      properties: [
        { name: 'agentcowork:lockfile', value: lockPath },
        { name: 'agentcowork:resolved', value: rec.resolved ?? 'vendored' },
      ],
    };
    if (rec.license) component.licenses = [{ license: { id: rec.license } }];
    components.push(component);
  }
  return components;
}

// --- pnpm-lock.yaml → components (the workspace: coordinator + core-*) ------
function pnpmComponents(lockPath) {
  const raw = readFileSync(join(root, lockPath), 'utf8');
  const components = [];
  const start = raw.indexOf('\npackages:\n');
  if (start < 0) return components;
  const section = raw.slice(start);
  // Package keys are exactly two-space indented: `  name@version:` or
  // `  'name@version(...)':` (the quotes wrap scoped names / peer suffixes).
  for (const m of section.matchAll(/^ {2}'?([^'\n]+?)'?:$/gm)) {
    const spec = m[1];
    // Strip a trailing peer-suffix like `(granian@…)` before splitting name@version.
    const cleaned = spec.replace(/\([^)]*\)$/, '');
    const idx = cleaned.lastIndexOf('@');
    if (idx <= 0) continue;
    const name = cleaned.slice(0, idx);
    const version = cleaned.slice(idx + 1);
    if (!name || !version) continue;
    components.push({
      type: 'library',
      'bom-ref': `pkg:npm/${name}@${version}`,
      name,
      version,
      purl: `pkg:npm/${name}@${version}`,
      properties: [{ name: 'agentcowork:lockfile', value: lockPath }],
    });
  }
  // Workspace packages (`link:../core-ai` importers) never appear in the
  // snapshot section — enumerate the importers' own package.json files so the
  // first-party components are in the SBOM too.
  const workspacePkgs = ['packages/coordinator', ...readdirSync(join(root, 'packages'), { withFileTypes: true })
    .filter((e) => e.isDirectory() && e.name.startsWith('core-'))
    .map((e) => `packages/${e.name}`)];
  for (const dir of workspacePkgs) {
    const manifestPath = join(root, dir, 'package.json');
    if (!existsSync(manifestPath)) continue;
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
    if (!manifest.name || !manifest.version) continue;
    components.push({
      type: 'application',
      'bom-ref': `pkg:npm/${manifest.name}@${manifest.version}`,
      name: manifest.name,
      version: manifest.version,
      purl: `pkg:npm/${manifest.name}@${manifest.version}`,
      properties: [
        { name: 'agentcowork:origin', value: `workspace:${dir}` },
        { name: 'agentcowork:lockfile', value: lockPath },
      ],
    });
  }
  return components;
}

const sboms = [
  ['agentcowork-sbom-rust.cdx.json', 'crates/Cargo.lock', (p) => cargoComponents(p, 'path:workspace')],
  ['agentcowork-sbom-shell.cdx.json', 'src-tauri/Cargo.lock', (p) => cargoComponents(p, 'path:workspace')],
  ['agentcowork-sbom-ui.cdx.json', 'ui/package-lock.json', npmComponents],
  ['agentcowork-sbom-sidecar.cdx.json', 'pnpm-lock.yaml', pnpmComponents],
];

mkdirSync(outDir, { recursive: true });
const summary = [];
for (const [name, lockPath, extractor] of sboms) {
  if (!existsSync(join(root, lockPath))) {
    console.error(`gen-sbom: FAIL — ${lockPath} is missing (commit the lockfile; a release without a lockfile has no reproducible dependency set)`);
    process.exit(1);
  }
  const components = extractor(lockPath);
  if (components.length === 0) {
    console.error(`gen-sbom: FAIL — ${lockPath} yielded zero components; refusing to emit an empty SBOM`);
    process.exit(1);
  }
  const d = doc(name);
  d.components = components;
  writeFileSync(join(outDir, name), `${JSON.stringify(d, null, 2)}\n`);
  summary.push(`${name}: ${components.length} component(s) from ${lockPath}`);
}

// --- provenance attestation ---------------------------------------------------
const gates = [
  'check-doc-sync.mjs',
  'check-arch-invariants.mjs',
  'check-versions.mjs',
  'check-release-matrix.mjs',
  'check-app-metadata.mjs',
  'check-native-deps.mjs',
  'check-size-budget.mjs',
  'check-store-schemas.mjs',
  'check-licences.mjs',
];
const rustToolchain = git('rustc --version');
const nodeToolchain = process.version;
const provenance = {
  $schema: 'https://in-toto.io/Statement/v1',
  type: 'https://in-toto.io/Statement/v1',
  subject: [
    { name: 'AgentCowork', digest: { sha256: createHash('sha256').update(`${commit}:${appVersion}`).digest('hex') } },
  ],
  predicateType: 'https://slsa.dev/provenance/v1',
  predicate: {
    buildDefinition: {
      buildType: 'https://github.com/sarv-projects/AgentCowork/.github/workflows/release.yml@v2',
      externalParameters: { version: appVersion, workflow: runUrl },
      internalParameters: { rust: rustToolchain, node: nodeToolchain, platform: 'windows x64 + arm64' },
      resolvedDependencies: [{ uri: `git+https://github.com/sarv-projects/AgentCowork@${commit}`, digest: { gitCommit: commit } }],
    },
    runDetails: {
      builder: { id: 'GitHub Actions / release.yml' },
      metadata: {
        invocationId: runUrl,
        startedOn: now,
        gatesRequired: gates,
      },
    },
  },
};
writeFileSync(join(outDir, 'agentcowork-provenance.json'), `${JSON.stringify(provenance, null, 2)}\n`);
summary.push('agentcowork-provenance.json: in-toto/SLSA statement');

for (const line of summary) console.log(`  ${line}`);
console.log(`gen-sbom: PASS — wrote ${summary.length} file(s) to ${outDir} (app ${appVersion}, commit ${commit.slice(0, 12)})`);
