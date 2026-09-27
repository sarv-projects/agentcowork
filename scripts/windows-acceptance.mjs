#!/usr/bin/env node
/**
 * P68.7 and P66.6–P66.9.
 *
 * Probes the binaries those rows name. A present binary is recorded as
 * evidence of presence only. This script always exits 2: presence is not
 * a Windows acceptance pass.
 */
import { spawnSync } from 'node:child_process'
import { writeFileSync } from 'node:fs'

function probe(cmd, args) {
  const result = spawnSync(cmd, args, { encoding: 'utf8', timeout: 8000 })
  return {
    cmd: [cmd, ...args].join(' '),
    status: result.status,
    error: result.error ? String(result.error.message) : null,
    stdout: (result.stdout || '').slice(0, 400),
    stderr: (result.stderr || '').slice(0, 200),
  }
}

const host = process.platform
const probes = {
  powershell: probe(host === 'win32' ? 'powershell.exe' : 'pwsh', ['-NoProfile', '-Command', '$PSVersionTable.PSVersion.ToString()']),
  soffice: probe('soffice', ['--version']),
  chrome: probe(host === 'win32' ? 'chrome' : 'google-chrome', ['--version']),
}
const report = {
  host,
  status: 'blocked',
  rows: {
    'P68.7': host === 'win32' && probes.powershell.status === 0 ? 'powershell-present' : 'blocked',
    'P66.6': probes.soffice.status === 0 ? 'soffice-present' : 'blocked',
    'P66.7': probes.chrome.status === 0 ? 'chrome-present' : 'blocked',
    'P66.8': 'blocked',
    'P66.9': 'blocked',
  },
  probes,
  reason:
    'Presence of a binary is not acceptance. ConPTY resize, an Office round-trip, UI Automation, restart hydration, and a clean install still need a recorded run on a Windows machine.',
}
const out = process.env.AGENTCOWORK_WINDOWS_ACCEPTANCE_REPORT
if (out) writeFileSync(out, JSON.stringify(report, null, 2))
console.log(JSON.stringify(report, null, 2))
process.exit(2)
