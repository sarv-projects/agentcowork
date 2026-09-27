#!/usr/bin/env node
/**
 * P50.1.7 — clean-profile boot verification (setup/offline states, no seeds).
 *
 * Boots a REAL binary with an isolated profile (fresh HOME + AGENTCOWORK_HOME,
 * no vault key, no provider keys, no coordinator) and asserts:
 *   (a) the boot fails honestly locked/setup (never silently ready),
 *   (b) `doctor --json` reports Credentials with zero keys (count-only),
 *   (c) the data dir holds no seeded tasks/scheduler records,
 *   (d) no sidecar-liveness is ever claimed while the coordinator is absent,
 *   (e) zero demo/seed markers in any output.
 *
 * Binary selection: `argv[2]`, else AGENTCOWORK_CLEAN_BOOT_BIN, else the debug
 * `agentcowork-core` binary (built by CI before this gate). Expectations adapt
 * to the binary kind: the headless core binary never spawns a supervisor
 * without `--coordinator-bin`, so (d) is "no live-sidecar claim"; a packaged
 * Tauri shell instead must print its pre-spawn skip line.
 *
 * Exit: 0 PASS / 1 FAIL / 2 SKIP (no binary available).
 */
import { existsSync, mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve, dirname, basename } from 'node:path'
import { spawnSync, spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
// This script lives directly in scripts/ (one level below the repo root).
const REPO_ROOT = resolve(HERE, '..')
const EXE_SUFFIX = process.platform === 'win32' ? '.exe' : ''
const DEFAULT_CORE_BIN = resolve(REPO_ROOT, `crates/target/debug/agentcowork-core${EXE_SUFFIX}`)

// The binary override is read on either spelling (DEC-053 legacy fallback).
const executable = process.argv[2] || process.env.AGENTCOWORK_CLEAN_BOOT_BIN || process.env.EVERYAIOS_CLEAN_BOOT_BIN || DEFAULT_CORE_BIN
if (!existsSync(executable)) {
  console.log(`[P50.1.7] SKIP — no binary at ${executable} (pass one, set AGENTCOWORK_CLEAN_BOOT_BIN, or build the debug core binary)`)
  process.exit(2)
}
const isCoreBinary = basename(executable).startsWith('agentcowork-core')

const failures = []
function pass(label) {
  console.log(`  ok — ${label}`)
}
function fail(label) {
  console.error(`  FAIL — ${label}`)
  failures.push(label)
}

const NO_SEED = /mockSessions|DEMO_TASKS|demoCockpit|seeded/i

// Isolated profile. AGENTCOWORK_HOME is the real isolation switch (Rust
// default_data_dir, with EVERYAIOS_HOME as the legacy fallback the current
// binary still reads — both are pinned to the profile); HOME/USERPROFILE
// cover the fallback chain. The vault key is explicitly absent so the boot
// must report locked/setup.
const profile = mkdtempSync(join(tmpdir(), 'agentcowork-clean-profile-'))
const dataDir = join(profile, '.agentcowork')
const env = {
  ...process.env,
  HOME: profile,
  USERPROFILE: profile,
  AGENTCOWORK_HOME: dataDir,
  EVERYAIOS_HOME: dataDir,
  AGENTCOWORK_DATA_DIR: dataDir,
  EVERYAIOS_DATA_DIR: dataDir,
}
// Scrub key material on BOTH spellings: a clean profile must resolve to
// NeedsSetup even on runners that export the legacy names, and the future
// binary must not inherit the new ones either.
delete env.AGENTCOWORK_VAULT_KEY
delete env.EVERYAIOS_VAULT_KEY
// Never inherit key material or key-generation permission: a clean profile
// must resolve to NeedsSetup even on CI runners that export these.
delete env.AGENTCOWORK_ALLOW_GENERATED_KEY
delete env.EVERYAIOS_ALLOW_GENERATED_KEY
delete env.AGENTCOWORK_VAULT_KEYFILE
delete env.EVERYAIOS_VAULT_KEYFILE
delete env.AGENTCOWORK_VAULT_PASSPHRASE
delete env.EVERYAIOS_VAULT_PASSPHRASE
delete env.AGENTCOWORK_E2E_BASE_URL
delete env.EVERYAIOS_E2E_BASE_URL

function run(args, timeoutMs = 20_000) {
  const r = spawnSync(executable, args, { env, encoding: 'utf8', timeout: timeoutMs })
  return { code: r.status, out: `${r.stdout ?? ''}\n${r.stderr ?? ''}`, signal: r.signal }
}

try {
  // ---- (a) clean boot fails honestly locked/setup, never silently ready ---
  {
    const { code, out } = run([])
    if (isCoreBinary) {
      // resolve_vault_key yields NeedsSetup with no key material anywhere;
      // boot() surfaces it as a failure, never a silent ready.
      const honest = /setup|locked|needs-setup|vault key|passphrase/i.test(out)
      if (code !== 0 && honest) pass(`clean boot fails honestly locked/setup (code ${code})`);
      else fail(`clean boot must fail locked/setup, got code ${code}: ${out.trim().slice(0, 120)}`)
      if (/ready — .*vault=.*\(ok\)/i.test(out)) fail('clean boot claimed a ready vault with no key');
      else pass('no ready-vault claim without a key')
    } else {
      // Packaged Tauri shell: 10s observation window, then kill.
      const child = spawn(executable, [], { env, stdio: ['ignore', 'pipe', 'pipe'] })
      let output = ''
      child.stdout.on('data', (c) => { output += c })
      child.stderr.on('data', (c) => { output += c })
      await new Promise((r) => setTimeout(r, 10_000))
      child.kill()
      const skipped = /coordinator binary not found|pre-spawn skipped/i.test(output)
      const setup = /vault|setup|locked|persistence/i.test(output)
      if (skipped && setup) pass('packaged shell: pre-spawn skipped + setup state observed');
      else fail(`packaged shell boot: pre-spawn-skip=${skipped} setup=${setup}`)
      if (/mockSessions|demo|seeded/i.test(output)) fail('seeded marker in shell output');
      else pass('no seeded marker in shell output')
    }
  }

  if (isCoreBinary) {
    // ---- (b) doctor reports zero keys, count-only ---------------------------
    {
      const { code, out } = run(['doctor', '--json'])
      let doc = null
      try {
        doc = JSON.parse(out.trim())
      } catch { doc = null }
      if (doc === null) fail('doctor --json emitted no parseable report');
      else {
        const creds = doc?.checks?.find((c) => c.name === 'Credentials')
        if (creds && /no provider keys/i.test(creds?.detail ?? '')) pass('doctor Credentials: zero keys, count-only');
        else fail(`doctor Credentials must report zero keys: ${JSON.stringify(creds)?.slice(0, 120)}`)
        // A locked vault is a v1-required break: exit 1 is the honest code.
        if (code === 1) pass('doctor exits 1 for the locked vault');
        else fail(`doctor exit must be 1 for a locked vault, got ${code}`)
      }
    }

    // ---- (c) no seeded tasks/scheduler records -------------------------------
    {
      const readRecords = (name) => {
        const p = join(dataDir, name)
        if (!existsSync(p)) return { absent: true, count: 0 }
        try {
          const v = JSON.parse(readFileSync(p, 'utf8'))
          const arr = Array.isArray(v) ? v : v.records ?? v.jobs ?? []
          return { absent: false, count: Array.isArray(arr) ? arr.length : 0 }
        } catch {
          return { absent: false, count: -1 }
        }
      }
      for (const name of ['tasks.json', 'scheduler.json']) {
        const r = readRecords(name)
        if (r.count === 0) pass(`${name}: ${r.absent ? 'absent' : 'empty'} — nothing seeded`);
        else fail(`${name} holds ${r.count} record(s) after a clean boot`)
      }
    }

    // ---- (d) no sidecar-liveness claim without a coordinator ------------------
    {
      const { out } = run([])
      if (/sidecar (connected|live|ready)|coordinator (connected|running)/i.test(out)) {
        fail('sidecar liveness claimed with no coordinator binary')
      } else pass('no sidecar-liveness claim without a coordinator')
    }

    // ---- (e) zero demo/seed markers across all output --------------------------
    {
      const { out } = run(['doctor'])
      if (NO_SEED.test(out)) fail('demo/seed marker in core output');
      else pass('zero demo/seed markers in core output')
    }
  }
} finally {
  rmSync(profile, { recursive: true, force: true })
}

if (failures.length > 0) {
  console.error(`[P50.1.7] FAIL — ${failures.length} assertion(s) failed`)
  process.exit(1)
}
console.log('[P50.1.7] PASS — clean-profile boot (honest locked/setup, sidecar-absent, zero seeds)')
