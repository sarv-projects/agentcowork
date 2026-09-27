#!/usr/bin/env node
// P69.E10 — user-facing vocabulary drift gate (ARCH/SESSION.md §2).
//
// The contract is one sentence: **"Nothing user-visible may say 'Session'."**
// User-facing words are `Chat` / `Space` / `Project`; `Session` is internal/API
// vocabulary (kernel, IPC, storage) and must never leak into what the user can
// read or hear.
//
// What this gate scans (ui/src, tests excluded):
//   * string and template literals, and JSX text between tags;
//   * i18n message catalogs.
//
// What it deliberately does NOT flag (stated, so drift is visible):
//   * identifiers, type names, and import paths (`session_id`, `sessionList`);
//   * test files (`.test.ts/tsx`) — internal vocabulary there is the point;
//   * comments (a comment explaining the vocabulary rule may quote the word);
//   * protocol-qualified terms: `CDP session`, `ACP session` — those name
//     *external protocol* objects (Chrome DevTools Protocol / Agent Client
//     Protocol), not an AgentCowork Chat;
//   * PTY terminal-line copy referring to the shell's stream session;
//   * `nativeCall('<op>')` / `operation: '<op>'` IPC labels — internal/API
//     vocabulary by the same §2 table;
//   * agent/third-party *names* that contain the word (e.g. agent ids in demo
//     analytics data) and enum-like tokens (`tone: 'session'`).
//
// Exit 0 = compliant, 1 = drift found.

import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const ROOT = new URL('..', import.meta.url).pathname
const UI = join(ROOT, 'ui', 'src')

// ---------------------------------------------------------------------------
// Allowlist — file-level and line-level, each with a reason. Every entry here
// is a statement that the occurrence is internal/API or protocol vocabulary,
// per ARCH/SESSION.md §2. Grow this only with the same rigour.
// ---------------------------------------------------------------------------

const FILE_ALLOW = [
  // Test files: internal vocabulary is the subject under test.
  /\.test\.(ts|tsx)$/,
]

const LINE_ALLOW = [
  // IPC operation labels (nativeCall('session save')) — internal/API vocabulary.
  { re: /nativeCall\(\s*['"`]/, reason: 'IPC op label' },
  { re: /operation:\s*['"`]/, reason: 'bridge operation label' },
  // Import paths / module specifiers.
  { re: /(?:^|\s)from\s+['"`]/, reason: 'import path' },
  { re: /import\(\s*['"`]/, reason: 'dynamic import path' },
  // Protocol-qualified: names an external protocol object, not a Chat.
  { re: /\b(CDP|ACP|PTY|MCP)\s+[Ss]ession/, reason: 'protocol-qualified' },
  { re: /\b[Ss]ession\s+(attached|detached|identity|recording|recorder|config)\b/, reason: 'protocol-qualified' },
  // Enum-ish tokens and identifiers appearing inside literals.
  { re: /tone:\s*['"`]session['"`]/, reason: 'enum token' },
  { re: /\bid:\s*['"`][^'"`]*session/, reason: 'identifier literal' },
  { re: /['"`][a-z0-9-]*session[a-z0-9-]*['"`]/, reason: 'identifier literal' },
  // Names containing the word (third-party agent names, csv headers…).
  { re: /sessions\s*:/, reason: 'data field name' },
  // Dotted event namespaces (`agentcowork.session-recording`) — wire identifiers.
  { re: /agentcowork\.[a-z.\-]+/, reason: 'event namespace' },
  // Agent-facing prompt bundles — the §2 internal/API vocabulary is correct
  // there ("the runtime says 'resume session'"); the user never reads these.
  { re: /Chief handoff — continuing work in session/, reason: 'agent-facing prompt' },
  // Terminal escape sequences carrying PTY stream copy (shell session).
  { re: /\\x1b\[/, reason: 'PTY stream copy' },
  // Archive-of-record files that quote citation shapes as examples.
  { re: /^TODO\.md$/, file: true, reason: 'dated record' },
]

const isLineAllowed = (relPath, line) =>
  LINE_ALLOW.some(({ re }) => re.test(line)) ||
  FILE_ALLOW.some((re) => re.test(relPath))

// ---------------------------------------------------------------------------
// Scanners
// ---------------------------------------------------------------------------

const collectFiles = (dir, acc = []) => {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) collectFiles(p, acc)
    else if (/\.(ts|tsx)$/.test(name)) acc.push(p)
  }
  return acc
}

/** Strip comments across the whole source (block comments may span lines),
 * then strip ${…} template expressions per line (they interpolate runtime
 * values — identifiers/data, not presentation copy). */
const stripComments = (source) => {
  let t = source.replace(/\/\*[\s\S]*?\*\//g, ' ')
  t = t.replace(/(^|\n)\s*\/\/[^\n]*/g, '$1')
  return t
}

const stripExpressions = (line) => line.replace(/\$\{[^}]*\}/g, ' ')

const SESSION_RE = /\b[Ss]essions?\b/

function scanFile(path) {
  const findings = []
  const rel = relative(ROOT, path)
  const source = stripComments(readFileSync(path, 'utf8'))
  const lines = source.split('\n')
  for (let i = 0; i < lines.length; i++) {
    const line = stripExpressions(lines[i])
    if (!SESSION_RE.test(line)) continue
    const relLine = `${rel}:${i + 1}`
    if (isLineAllowed(rel, line)) continue

    // 1. String / template literals.
    for (const m of line.matchAll(/(['"`])((?:\\.|(?!\1)[^\\])*)\1/g)) {
      const v = m[2]
      if (SESSION_RE.test(v)) {
        findings.push({ where: relLine, kind: 'string literal', text: v.slice(0, 120) })
        break
      }
    }
    if (findings.length && findings[findings.length - 1].where === relLine) continue

    // 2. JSX text between tags (no braces inside → static copy).
    const jsx = line.match(/>([^<>{}]+)</g)
    if (jsx) {
      for (const seg of jsx) {
        if (SESSION_RE.test(seg)) {
          findings.push({ where: relLine, kind: 'JSX text', text: seg.slice(1, -1).trim().slice(0, 120) })
          break
        }
      }
    }
  }
  return findings
}

// ---------------------------------------------------------------------------
// i18n catalogs: every value is user-visible by definition.
// ---------------------------------------------------------------------------

function scanI18n() {
  const findings = []
  for (const name of readdirSync(UI)) {
    if (!/^i18n/.test(name)) continue
    const path = join(UI, name)
    const rel = relative(ROOT, path)
    const lines = readFileSync(path, 'utf8').split('\n')
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]
      const m = line.match(/^\s*'[^']+'\s*:\s*['"`](.*)['"`],\s*$/)
      if (m && SESSION_RE.test(m[1])) {
        findings.push({ where: `${rel}:${i + 1}`, kind: 'i18n message', text: m[1].slice(0, 120) })
      }
    }
  }
  return findings
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

const findings = []
for (const f of collectFiles(UI)) findings.push(...scanFile(f))
findings.push(...scanI18n())

if (process.argv.includes('--json')) {
  console.log(JSON.stringify({ ok: findings.length === 0, findings }, null, 2))
} else if (findings.length === 0) {
  console.log('P69.E10 vocabulary gate: PASS — no user-facing "Session" in ui/src')
} else {
  console.error(`P69.E10 vocabulary gate: FAIL — ${findings.length} user-visible "Session" occurrence(s):`)
  for (const f of findings) console.error(`  ✗ ${f.where} (${f.kind}): ${f.text}`)
  console.error('\nContract: ARCH/SESSION.md §2 — user-facing words are Chat / Space / Project.')
  console.error('If an occurrence is genuinely internal/API or protocol vocabulary, extend')
  console.error('LINE_ALLOW with a stated reason (same rigour as check-doc-refs).')
  process.exit(1)
}
