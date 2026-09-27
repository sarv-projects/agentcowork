#!/usr/bin/env node
// P70.A3 — the per-platform native dependency audit, as a gate.
//
// The audit is only useful if it cannot rot. It states two things: (1) the
// shipping paths never require a build toolchain (Rust/Node/Bun/pnpm) on the
// user's machine, and (2) every external program the shell or the kernel can
// execute is classified in `PACKAGING.md` §2 with what happens when it is
// missing. Both are checked here: a newly spawned program fails the build
// until someone classifies it, and a toolchain name in a spawn position fails
// outright (that would make the installer unshippable).
//
// What is checked:
//   1. every program literal spawned via `Command::new("…")` in `src-tauri/src`
//      and `crates/*/src` is classified in PACKAGING.md §2 (compared by
//      basename, ignoring test modules — the repo convention puts tests in a
//      bottom `#[cfg(test)] mod tests`);
//   2. none of the toolchain names (`node`, `bun`, `cargo`, …) appears in a
//      spawn position anywhere in the shipping paths;
//   3. the coordinator is still shipped as a **standalone compiled binary**
//      (`bun build --compile`), declared as a bundle resource, and staged by the
//      release workflow — i.e. no Bun runtime is needed on the user's machine;
//   4. `PACKAGING.md` still states the classification table and the WSL
//      agent-host contract.
//
// Read-only; no arguments.

import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, basename } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const failures = [];
const fail = (msg) => failures.push(msg);

// A runtime dependency on any of these would mean the installer only works on a
// developer machine. They are build-time tools (PACKAGING.md §2).
const TOOLCHAIN = [
  'node', 'nodejs', 'npm', 'npx', 'pnpm', 'bun', 'yarn', 'deno',
  'cargo', 'rustc', 'rustup',
  'python', 'python3', 'pip', 'pip3',
  'tsc', 'vite', 'esbuild', 'bunx',
];

/** Every `.rs` file under the given roots. */
function rustFiles(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) rustFiles(p, out);
    else if (entry.name.endsWith('.rs')) out.push(p);
  }
  return out;
}

const sources = [
  ...rustFiles(join(root, 'src-tauri/src')),
  ...readdirSync(join(root, 'crates'), { withFileTypes: true })
    .filter((e) => e.isDirectory() && e.name.startsWith('agentcowork-'))
    .flatMap((e) => rustFiles(join(root, 'crates', e.name, 'src'))),
];

// --- 1 + 2: spawn sites ------------------------------------------------------
/** Drop everything from a file's `mod tests` onward (repo convention: tests last). */
function shippingCode(src) {
  const m = /^\s*(#\[cfg\(test\)\]\s*)?mod tests\b/m.exec(src);
  return m ? src.slice(0, m.index) : src;
}

const discovered = new Map(); // basename -> ["relative/path.rs:line", ...]
const toolchainHits = [];

for (const file of sources) {
  const rel = file.slice(root.length + 1);
  const lines = shippingCode(readFileSync(file, 'utf8')).split('\n');
  lines.forEach((line, i) => {
    if (!/Command::new\(/.test(line)) return;
    const literal = /Command::new\(\s*"([^"\\]+)"/.exec(line);
    if (!literal) return; // program comes from config/args — classified as dynamic
    const program = literal[1];
    const name = basename(program);
    const at = `${rel}:${i + 1}`;
    if (TOOLCHAIN.includes(name)) toolchainHits.push(`${at} spawns \`${program}\``);
    if (!discovered.has(name)) discovered.set(name, []);
    discovered.get(name).push(at);
  });
}

for (const hit of toolchainHits) {
  fail(
    `${hit} — a build toolchain in a runtime spawn position. The user's machine has no Rust/Node/Bun/pnpm; ` +
      'the shipping paths must not require one (PACKAGING.md §2).',
  );
}

// A `sh -c "… bun run …"` payload is the same defect wearing a hat.
for (const file of sources) {
  const rel = file.slice(root.length + 1);
  const lines = shippingCode(readFileSync(file, 'utf8')).split('\n');
  lines.forEach((line, i) => {
    const sh = /spawn\(\s*"sh"\s*,\s*&\[[^\]]*"([^"]*)"/.exec(line);
    if (!sh) return;
    const payload = sh[1];
    for (const tool of TOOLCHAIN) {
      if (new RegExp(`(^|\\s|/)${tool}(\\s|$|/)`).test(payload)) {
        fail(`${rel}:${i + 1}: \`sh -c\` payload invokes the build toolchain (\`${tool}\`)`);
      }
    }
  });
}

// --- classification against PACKAGING.md §2 ----------------------------------
const packagingPath = join(root, 'PACKAGING.md');
if (!existsSync(packagingPath)) {
  fail('PACKAGING.md is missing — the native-dependency audit has no published form (P70.A3)');
} else {
  const doc = readFileSync(packagingPath, 'utf8');
  const section = doc.split('## 2. Runtime external programs')[1]?.split('\n## ')[0] ?? '';
  if (!section) {
    fail('PACKAGING.md: the "Runtime external programs" section is missing');
  } else {
    // First column of each table row, tokenised on `/` and `,`, backticks stripped.
    const classified = new Set();
    for (const line of section.split('\n')) {
      if (!line.trim().startsWith('|')) continue;
      const first = line.split('|')[1] ?? '';
      for (const token of first.split(/[/,]/)) {
        const name = token.replace(/[`*]/g, '').trim();
        if (name && !/^-+$/.test(name) && name !== 'Program') classified.add(name);
      }
    }
    if (classified.size === 0) {
      fail('PACKAGING.md: the runtime-program table is empty — nothing is classified');
    }
    for (const [name, sites] of [...discovered].sort()) {
      if (!classified.has(name)) {
        fail(
          `unclassified runtime program \`${name}\` (spawned at ${sites.slice(0, 2).join(', ')}${
            sites.length > 2 ? `, +${sites.length - 2} more` : ''
          }) — add a row to PACKAGING.md §2 naming what provides it and what happens without it`,
        );
      }
    }
  }
  for (const [needle, why] of [
    ['WSL2 is audited only as a', 'the WSL-is-an-agent-host contract'],
    ['standalone', 'the standalone-sidecar claim (no Bun on the user\'s machine)'],
  ]) {
    if (!doc.includes(needle)) fail(`PACKAGING.md: ${why} is no longer stated`);
  }
}

// --- 3: the sidecar is a self-contained binary -------------------------------
try {
  const pkg = JSON.parse(readFileSync(join(root, 'packages/coordinator/package.json'), 'utf8'));
  const build = pkg.scripts?.build ?? '';
  if (!/bun build --compile/.test(build)) {
    fail(
      'packages/coordinator/package.json: the build script no longer uses `bun build --compile` — the sidecar ' +
        'would then need a Bun runtime on the user\'s machine (P70.A2/A3)',
    );
  }
  if (!/outfile\s+dist\/coordinator/.test(build)) {
    fail('packages/coordinator/package.json: the build script no longer emits `dist/coordinator` — the bundle resource path would drift');
  }
} catch (err) {
  fail(`packages/coordinator/package.json: ${err.message}`);
}

try {
  const conf = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
  const resources = JSON.stringify(conf.bundle?.resources ?? []);
  if (!resources.includes('bin/coordinator')) {
    fail('src-tauri/tauri.conf.json: `bundle.resources` no longer includes the coordinator sidecar — the packaged app would fail closed');
  }
} catch (err) {
  fail(`src-tauri/tauri.conf.json: ${err.message}`);
}

try {
  const release = readFileSync(join(root, '.github/workflows/release.yml'), 'utf8');
  const hasStage = /DST="bin\/coordinator\.exe"/.test(release);
  const hasInject = /echo "TAURI_CONFIG=\{\\"bundle\\":\{\\"resources\\"/.test(release);
  if (!hasStage || !hasInject) {
    fail(
      '.github/workflows/release.yml: the sidecar is no longer staged into the bundle as a resource ' +
        `(stage step: ${hasStage}, TAURI_CONFIG resource injection: ${hasInject})`,
    );
  }
} catch (err) {
  fail(`.github/workflows/release.yml: ${err.message}`);
}

if (failures.length > 0) {
  console.error('native-deps: FAIL');
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
const programs = [...discovered.keys()].sort();
console.log(
  `native-deps: PASS — ${programs.length} external program(s) classified, none from the build toolchain ` +
    `[${programs.join(', ')}]; the sidecar ships compiled as a bundle resource`,
);
