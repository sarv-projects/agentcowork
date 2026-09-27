#!/usr/bin/env node
// W0 · TASK-UI-001 — UI-observable secret-corpus scan (FIX-04 · SEC-21 ·
// REQ-PROD-002 · INV-02).
//
// REQ-PROD-002: "GIVEN a provider credential is stored, WHEN any prompt,
// context, event, log, receipt or code path is produced, THEN the credential
// value never appears." Its acceptance clause is a *secret-corpus scan* over
// exactly those surfaces. This gate is that scan, over the code that produces
// them: the cockpit (which renders prompts, log/session-event projections,
// receipts, artifact metadata, tool results and error cards), the Tauri command
// surface (every return value and error string the webview can observe), the
// shell-e2e harness, and the kernel files that own the secret vocabulary and its
// deliberate fixtures.
//
// It is a *structural* scan, not a keyword grep. A keyword grep for "token" or
// "api_key" is a keyword grep: this repo's own kernel redactor
// (`crates/agentcowork-core/src/spool.rs`) is explicit that the corpus is "a
// fixed, bounded corpus of *token shapes*", and that is what is matched here:
//
//   * prefix rules   — the credential prefixes of the providers this product
//                      can hold a key for (openai/anthropic/opencode/openrouter,
//                      xai, groq, github, gitlab, slack, stripe, npm, hf,
//                      digitalocean, google, aws). Structural: authoritative.
//   * shape rules    — JWT, PEM private-key headers.
//   * heuristic rules— `secret_key: "…"`, `*_TOKEN=<blob>`, `Bearer <blob>`,
//                      `user:secret@host`, long high-entropy base64/hex blobs.
//                      Entropy- and character-class-gated plus a placeholder
//                      screen, so they stay usable rather than noisy; the
//                      report states the limits they carry.
//
// Structural rules ignore the placeholder screen on purpose: a `sk-…` literal
// is a finding even when its body is filler, because the redactor must still
// see it. What a structural hit *means* is then a review decision, recorded in
// scripts/secret-scan-allowlist.json — never a silent pass.
//
// The prefix table is cross-checked against the kernel's own `SECRET_PREFIXES`
// (see `vocabulary:` in the report): one vocabulary, and a prefix the kernel
// learns to redact cannot be missing here.
//
// No secret value is ever printed. A finding reports rule, location, length and
// a sha256 fingerprint prefix — enough to find, review and allowlist a value,
// useless to an attacker who reads CI logs.
//
// Usage:
//   node scripts/check-secret-corpus.mjs                  # scan the default corpus
//   node scripts/check-secret-corpus.mjs --self-test      # prove the detector
//   node scripts/check-secret-corpus.mjs --json           # machine-readable
//   node scripts/check-secret-corpus.mjs --paths a,b      # override the corpus
//   node scripts/check-secret-corpus.mjs --print-allowlist
//   node scripts/check-secret-corpus.mjs --min-entropy 3.7 --min-blob-length 48
//                                                          # tune the heuristics
//
// Exit: 0 PASS · 1 findings or stale expectations · 2 cannot run (bad usage,
// missing corpus root, unparsable allowlist, missing vocabulary source).

import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
/** The repository root with symlinks resolved — the containment boundary. */
const ROOT_REAL = (() => {
  try {
    return realpathSync(ROOT);
  } catch {
    return ROOT;
  }
})();
const ALLOWLIST_PATH = join(ROOT, 'scripts/secret-scan-allowlist.json');

// ---------------------------------------------------------------------------
// Corpus — the surfaces the UI can observe, grouped so the report says what was
// actually read. A root that does not exist is a hard error (exit 2): a path
// typo must never turn this gate into a no-op that passes.
// ---------------------------------------------------------------------------
const CORPUS = [
  {
    id: 'ui',
    label: 'cockpit source — prompts, log/session-event projections, receipts, artifact metadata, tool results, error surfaces, previews and test fixtures',
    roots: ['ui/src', 'ui/index.html', 'ui/guard.html'],
  },
  {
    id: 'shell',
    label: 'Tauri command surface — every return value and error string the webview can observe',
    roots: ['src-tauri/src'],
  },
  {
    id: 'gate',
    label: 'the gate itself — detector tables and the shell e2e harness (a scanner that cannot see its own rules is not a scanner)',
    roots: ['scripts/e2e', 'scripts/check-secret-corpus.mjs'],
  },
  {
    id: 'corpus',
    label: 'secret-corpus fixtures — the kernel files that own the vocabulary and the deliberate fake secrets that keep the redactor honest',
    roots: [
      'crates/agentcowork-core/src/spool.rs',
      'crates/agentcowork-vault/src/lib.rs',
      'crates/agentcowork-vault/src/egress.rs',
      'crates/agentcowork-acp/src/agent_backend.rs',
    ],
  },
];

/** Single reference for what counts as a secret shape (ARCH/12 §6 vocabulary). */
const VOCAB_SOURCE = 'crates/agentcowork-core/src/spool.rs';

const SKIP_DIRS = new Set([
  'node_modules', 'target', 'dist', '.git', 'coverage',
  '.code-intelligence', '.venv', '__pycache__',
]);

const TEXT_EXTS = new Set([
  '.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.json', '.rs', '.html', '.css',
  '.md', '.txt', '.yml', '.yaml', '.toml', '.sql', '.svg', '.xml', '.sh', '.ps1',
  '.cfg', '.ini', '.env', '.properties', '.gradle', '.py',
]);

/** Per-file size cap: a mis-swept directory must not be slurped into memory. */
const MAX_BYTES = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Rules. `prefix` is the *public* structural prefix; it is what the fingerprint
// shows, never any part of the secret value.
// ---------------------------------------------------------------------------

/** Credential prefixes of the providers this product can hold a key for. */
const PREFIX_RULES = [
  { prefix: 'sk-', re: /\bsk-[A-Za-z0-9_-]{16,}/, why: 'OpenAI / Anthropic / OpenRouter / OpenCode-style API key literal' },
  { prefix: 'sk_live_', re: /\bsk_live_[A-Za-z0-9]{16,}/, why: 'Stripe live secret key literal' },
  { prefix: 'sk_test_', re: /\bsk_test_[A-Za-z0-9]{16,}/, why: 'Stripe test secret key literal' },
  { prefix: 'r8_', re: /\br8_[A-Za-z0-9]{24,}/, why: 'r8 key literal (kernel redactor corpus)' },
  { prefix: 'xai-', re: /\bxai-[A-Za-z0-9]{24,}/, why: 'xAI API key literal' },
  { prefix: 'gsk_', re: /\bgsk_[A-Za-z0-9]{24,}/, why: 'Groq API key literal' },
  { prefix: 'ghp_', re: /\bghp_[A-Za-z0-9]{20,}/, why: 'GitHub classic personal access token literal' },
  { prefix: 'gho_', re: /\bgho_[A-Za-z0-9]{20,}/, why: 'GitHub OAuth access token literal' },
  { prefix: 'ghs_', re: /\bghs_[A-Za-z0-9]{20,}/, why: 'GitHub server-to-server token literal' },
  { prefix: 'ghu_', re: /\bghu_[A-Za-z0-9]{20,}/, why: 'GitHub user-to-server token literal' },
  { prefix: 'github_pat_', re: /\bgithub_pat_[A-Za-z0-9_]{22,}/, why: 'GitHub fine-grained personal access token literal' },
  { prefix: 'glpat-', re: /\bglpat-[A-Za-z0-9_-]{16,}/, why: 'GitLab personal access token literal' },
  { prefix: 'xoxb-', re: /\bxoxb-[A-Za-z0-9-]{10,}/, why: 'Slack bot token literal' },
  { prefix: 'xoxp-', re: /\bxoxp-[A-Za-z0-9-]{10,}/, why: 'Slack user token literal' },
  { prefix: 'xoxa-', re: /\bxoxa-[A-Za-z0-9-]{10,}/, why: 'Slack app-level token literal' },
  { prefix: 'xapp-', re: /\bxapp-[A-Za-z0-9-]{10,}/, why: 'Slack app-level token literal' },
  { prefix: 'npm_', re: /\bnpm_[A-Za-z0-9]{24,}/, why: 'npm access token literal' },
  { prefix: 'hf_', re: /\bhf_[A-Za-z0-9]{24,}/, why: 'HuggingFace access token literal' },
  { prefix: 'dop_v1_', re: /\bdop_v1_[A-Za-z0-9]{24,}/, why: 'DigitalOcean personal access token literal' },
  { prefix: 'AKIA', re: /\bAKIA[0-9A-Z]{16}/, why: 'AWS access key id literal' },
  { prefix: 'ASIA', re: /\bASIA[0-9A-Z]{16}/, why: 'AWS temporary access key id literal' },
  { prefix: 'AIza', re: /\bAIza[0-9A-Za-z_-]{30}/, why: 'Google API key literal' },
  { prefix: 'ya29.', re: /\bya29\.[0-9A-Za-z_-]{20}/, why: 'Google OAuth access token literal' },
];

/** Non-prefixed shapes. Structural: a hit is a finding, no entropy gate. */
const SHAPE_RULES = [
  { id: 'pem-private-key', re: /-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----/, why: 'PEM private-key block header' },
  { id: 'jwt', re: /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8}/, why: 'JWT-shaped token (three base64url segments)' },
];

/**
 * Heuristic rules. Gated on entropy + character classes so the gate is usable;
 * every one of them also passes the placeholder screen.
 */
const HEURISTIC_RULES = [
  { id: 'assigned-secret', re: /\b([A-Za-z0-9_.-]*(?:api[_-]?key|apikey|access[_-]?token|auth[_-]?token|refresh[_-]?token|client[_-]?secret|secret[_-]?key|private[_-]?key|secret|password|passwd|authorization|token))\s*[:=]\s*["'`]([^"'`\r\n]{12,})["'`]/i, group: 2, minEntropy: 3.0, minClasses: 3, why: 'secret-shaped name assigned a high-entropy literal' },
  { id: 'env-secret', re: /\b([A-Z0-9_]*_KEY|[A-Z0-9_]*ACCESS_TOKEN|[A-Z0-9_]*AUTH_TOKEN|[A-Z0-9_]*SECRET|[A-Z0-9_]*PASSWORD|[A-Z0-9_]*TOKEN)\s*=\s*([A-Za-z0-9+/=_-]{24,})/, group: 2, minEntropy: 3.5, minClasses: 3, why: 'secret-shaped env name assigned a high-entropy value' },
  { id: 'bearer-token', re: /\b[Bb]earer\s+([A-Za-z0-9._~+/=-]{20,})/, group: 1, minEntropy: 3.0, minClasses: 3, why: 'bearer credential in a header or config literal' },
  // A userinfo password of one lowercase word (`https://user:password@host`, the
  // shape every URL-rejection test uses) is a placeholder; a real one is not.
  { id: 'url-userinfo', re: /\b[a-z][a-z0-9+.-]*:\/\/[^\s/:@'"]{1,64}:([^\s/@"']{8,})@/, group: 1, minEntropy: 3.2, minClasses: 2, why: 'credential embedded in a URL (user:secret@host)' },
  { id: 'high-entropy-blob', re: /(?<![A-Za-z0-9+/=_-])[A-Za-z0-9+/]{40,}={0,2}(?![A-Za-z0-9+/=_-])/, minEntropy: 3.9, minClasses: 3, why: 'long high-entropy base64 blob (no known prefix)' },
  { id: 'hex-digest-blob', re: /(?<![0-9A-Za-z])[0-9a-f]{64,}(?![0-9A-Za-z])/, minEntropy: 3.5, minClasses: 2, why: 'long hex blob (no known prefix)' },
];

/**
 * Placeholder screen. A heuristic hit whose value reads like documentation is
 * not a leak; a *structural* hit never reaches this screen, so a fake key that
 * must stay recognizable to the redactor tests is still reported (and reviewed
 * through the allowlist rather than silently ignored).
 */
const PLACEHOLDER = /(?:example|placeholder|changeme|change[-_ ]?me|your[-_ ]|dummy|fake|sample|redact|notreal|not[-_]?real|todo|fixme|insert[-_ ]?here|lorem|abcdef|1234567890|foo|bar|baz)/i;

/** Template interpolation and mask characters: not literal secret material. */
const TEMPLATE_CHARS = /[$`{}<>\\*…]/;

/** Known non-secret high-entropy shapes, excluded structurally: content hashes
 *  (IPFS CIDv0) and public addresses/keys (`0x…`). None of these are
 *  credentials, and no entropy floor separates them from one. */
const CONTENT_HASH = /^(?:Qm[1-9A-HJ-NP-Za-km-z]{44}|0x[0-9a-fA-F]{40}|0x[0-9a-fA-F]{64})$/;

/** Markers that turn a following high-entropy run into non-secret payload. */
const PAYLOAD_MARKERS = ['base64,', '0x'];

/**
 * A `/`-separated run whose every part is a word (`ttft/batch/done/…`, a URL
 * path, an event-name list) is a list, not one base64 payload. Real base64 has
 * no such structure, so this is a cheap, explainable cut.
 */
function looksLikeSlashPath(value) {
  const parts = value.split('/');
  if (parts.length < 2) return false;
  return parts.every((p) => /^[A-Za-z][A-Za-z0-9]{2,}$/.test(p)) && parts.some((p) => p.length >= 4);
}

/** Everything a rule can emit, with its public prefix and reason. */
const RULES = [
  ...PREFIX_RULES.map((r) => ({ id: `prefix:${r.prefix}`, re: r.re, prefix: r.prefix, kind: 'structural', why: r.why })),
  ...SHAPE_RULES.map((r) => ({ id: r.id, re: r.re, group: r.group ?? 0, kind: 'structural', why: r.why })),
  ...HEURISTIC_RULES.map((r) => ({ id: r.id, re: r.re, group: r.group ?? 0, kind: 'heuristic', minEntropy: r.minEntropy, minClasses: r.minClasses, why: r.why })),
];

// The detector's own known-bad corpus is the one place in the tree that must
// contain complete credential-shaped literals — that is what `--self-test`
// proves them against. It is bracketed by sentinels and skipped, and the skip is
// reported on every run. The sentinel is honoured *only* in this file: no other
// file may declare itself exempt. The sentinels are assembled from parts so
// their own definitions are not themselves a match.
const SELF_CORPUS_BEGIN = ['secret-corpus', 'begin-known-bad-corpus'].join(':');
const SELF_CORPUS_END = ['secret-corpus', 'end-known-bad-corpus'].join(':');
const SELF_FILE = 'scripts/check-secret-corpus.mjs';

/**
 * Blank the detector's own sample table, preserving byte offsets so reported
 * line/column numbers stay true for everything outside it. Returns the number
 * of regions removed (0 or 1) or an error string if the sentinel is gone — a
 * silently-vanished sentinel would quietly stop exempting the corpus and turn
 * the gate into a permanent FAIL, so it is surfaced instead.
 */
function maskSelfCorpus(rel, text) {
  if (rel !== SELF_FILE) return { text, masked: 0 };
  const from = text.indexOf(`// ${SELF_CORPUS_BEGIN}`);
  const to = text.indexOf(`// ${SELF_CORPUS_END}`);
  if (from === -1) return { text, masked: 0, error: `${SELF_FILE} no longer carries the "// ${SELF_CORPUS_BEGIN}" sentinel` };
  if (to === -1 || to < from) return { text, masked: 0, error: `${SELF_FILE} carries a begin sentinel with no matching "${SELF_CORPUS_END}"` };
  const start = text.lastIndexOf('\n', from) + 1;
  const end = (text.indexOf('\n', to) + 1) || text.length;
  if (end - start < 64) return { text, masked: 0, error: `${SELF_FILE} sentinel span is implausibly small (${end - start} bytes) — the sample table moved out from between the sentinels` };
  return { text: `${text.slice(0, start)}${' '.repeat(end - start)}${text.slice(end)}`, masked: 1 };
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/** Shannon entropy in bits per character. */
function shannon(value) {
  const counts = new Map();
  for (const ch of value) counts.set(ch, (counts.get(ch) ?? 0) + 1);
  let bits = 0;
  for (const n of counts.values()) {
    const p = n / value.length;
    bits -= p * Math.log2(p);
  }
  return bits;
}

/** How many of {lower, upper, digit, symbol} a value uses. */
function charClasses(value) {
  const seen = new Set();
  for (const ch of value) {
    if (/[a-z]/.test(ch)) seen.add('lower');
    else if (/[A-Z]/.test(ch)) seen.add('upper');
    else if (/[0-9]/.test(ch)) seen.add('digit');
    else seen.add('symbol');
  }
  return seen;
}

/**
 * A finding's identity: the value's sha256, truncated. Stable when the code
 * around a fixture moves, and different when the value changes — so a reviewed
 * expectation cannot silently start covering a different secret.
 */
function fingerprint(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex').slice(0, 12);
}

/**
 * A printable finding. Contains no substring of the secret's *unpredictable*
 * part: the rule id is a public classification (it names the credential family,
 * which the kernel redactor table already publishes in-repo), and the identity
 * is the length plus a truncated sha256 — never the value.
 */
function formatFinding(finding) {
  const where = `${finding.path}:${finding.line}:${finding.col}`;
  const rules = [...finding.rules].sort().join(' + ');
  return `${where}  ${rules}  len=${finding.len}  sha256=${finding.fp}`;
}

function formatWhy(finding) {
  const rule = RULES.find((r) => finding.rules.has(r.id));
  return rule ? rule.why : 'credential-shaped literal';
}

// ---------------------------------------------------------------------------
// Walking + detection
// ---------------------------------------------------------------------------

/**
 * Collect the text files under `root`. Every entry's real path must resolve
 * inside the repository and must not have been visited already, so a symlink
 * cannot make this gate loop forever or read a file the corpus never declared.
 */
function walk(root, out, seen) {
  let real;
  try {
    real = realpathSync(root);
  } catch {
    return;
  }
  if (real !== ROOT_REAL && !real.startsWith(`${ROOT_REAL}/`)) return;
  if (seen.has(real)) return;
  seen.add(real);
  let st;
  try {
    st = statSync(root);
  } catch {
    return;
  }
  if (st.isFile()) {
    if (TEXT_EXTS.has(extname(root).toLowerCase())) out.push(root);
    return;
  }
  let entries;
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of [...entries].sort((a, b) => (a.name < b.name ? -1 : 1))) {
    if (SKIP_DIRS.has(entry.name)) continue;
    walk(join(root, entry.name), out, seen);
  }
}

/** Offsets of each line start, for one pass per file. */
function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i += 1) if (text.charCodeAt(i) === 10) starts.push(i + 1);
  return starts;
}

/** Index of the line containing `offset` (binary search over the starts). */
function lineOf(starts, offset) {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= offset) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/**
 * A heuristic candidate is a finding only when it clears entropy, character
 * classes, the template screen and the placeholder screen. Structural rules
 * skip all four: the shape itself is the evidence.
 */
function heuristicAccepts(value, rule) {
  if (TEMPLATE_CHARS.test(value)) return false;
  if (PLACEHOLDER.test(value)) return false;
  if (shannon(value) < rule.minEntropy) return false;
  if (charClasses(value).size < rule.minClasses) return false;
  return true;
}

/**
 * Detect every credential-shaped literal in one file. Findings are merged per
 * (line, value) so one secret reported by two rules is one reviewable line.
 */
function detect(text) {
  const starts = lineStarts(text);
  const found = new Map();
  for (const rule of RULES) {
    const flags = rule.re.flags.includes('g') ? rule.re.flags : `${rule.re.flags}g`;
    for (const match of text.matchAll(new RegExp(rule.re.source, flags))) {
      const value = rule.group ? match[rule.group] : match[0];
      if (!value) continue;
      if (rule.kind === 'heuristic' && !heuristicAccepts(value, rule)) continue;
      if (CONTENT_HASH.test(value)) continue;
      // A data URI's base64 payload is an image; an `0x…` run is an address;
      // a `/`-separated word list is a path, not one payload.
      if (rule.id === 'high-entropy-blob') {
        const before = text.slice(Math.max(0, match.index - 8), match.index);
        if (PAYLOAD_MARKERS.some((m) => before.endsWith(m))) continue;
        if (looksLikeSlashPath(value)) continue;
      }

      const valueAt = match.index + (rule.group ? match[0].indexOf(value) : 0);
      const li = lineOf(starts, valueAt);
      const key = `${li}:${value}`;
      const existing = found.get(key);
      if (existing) {
        existing.rules.add(rule.id);
        existing.prefix ??= rule.prefix ?? null;
        continue;
      }
      found.set(key, {
        line: li + 1,
        col: valueAt - starts[li] + 1,
        rules: new Set([rule.id]),
        prefix: rule.prefix ?? null,
        len: value.length,
        fp: fingerprint(value),
      });
    }
  }
  return [...found.values()];
}

function scanFile(abs) {
  let st;
  try {
    st = statSync(abs);
  } catch {
    return { findings: [], skipped: 0, masked: 0, error: null };
  }
  if (st.size > MAX_BYTES) return { findings: [], skipped: 1, masked: 0, error: null };
  let text;
  try {
    text = readFileSync(abs, 'utf8');
  } catch {
    return { findings: [], skipped: 1, masked: 0, error: null };
  }
  if (text.includes('\0')) return { findings: [], skipped: 1, masked: 0, error: null };
  const rel = abs.slice(ROOT.length + 1).replace(/\\/g, '/');
  const self = maskSelfCorpus(rel, text);
  return { findings: detect(self.text).map((f) => ({ ...f, path: rel })), skipped: 0, masked: self.masked, error: self.error ?? null };
}

// ---------------------------------------------------------------------------
// Vocabulary concordance — one secret vocabulary (ARCH/05 INV-02 vocabulary).
// ---------------------------------------------------------------------------

function vocabulary() {
  const src = join(ROOT, VOCAB_SOURCE);
  if (!existsSync(src)) {
    return { ok: false, reason: `vocabulary source missing: ${VOCAB_SOURCE}` };
  }
  const block = /const\s+SECRET_PREFIXES\s*:\s*&\[&str\]\s*=\s*&\[([\s\S]*?)\n\s*\];/.exec(readFileSync(src, 'utf8'));
  if (!block) {
    return { ok: false, reason: `SECRET_PREFIXES no longer parseable in ${VOCAB_SOURCE}` };
  }
  const kernel = [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  const mine = new Set(PREFIX_RULES.map((r) => r.prefix));
  const missing = kernel.filter((p) => !mine.has(p));
  return { ok: missing.length === 0, kernel, missing };
}

// ---------------------------------------------------------------------------
// Allowlist — a reviewed expectation per (path, fingerprint).
// ---------------------------------------------------------------------------

function loadAllowlist(explicitPath) {
  const p = explicitPath ? resolve(explicitPath) : ALLOWLIST_PATH;
  if (!existsSync(p)) return { path: p, entries: [] };
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(p, 'utf8'));
  } catch (e) {
    return { path: p, error: `unparsable allowlist ${p}: ${e.message}` };
  }
  if (!Array.isArray(parsed?.entries)) return { path: p, error: `allowlist ${p} has no "entries" array` };
  const entries = [];
  for (const [i, e] of parsed.entries.entries()) {
    if (typeof e?.path !== 'string' || !Array.isArray(e.fingerprints) || typeof e.reason !== 'string' || e.reason.trim() === '') {
      return { path: p, error: `allowlist entry #${i} needs { path, fingerprints[], reason }` };
    }
    entries.push(e);
  }
  return { path: p, entries };
}

function allowlistKey(path, fp) {
  return `${path}|${fp}`;
}

// ---------------------------------------------------------------------------
// Self-test — CI must be able to trust the detector, so the detector is proven
// against known-bad, known-good and documented-not-flagged samples before the
// tree is read at all.
// ---------------------------------------------------------------------------

// The known-bad table below is where the detector is proven against real shapes,
// not against prefixes. The table is bracketed by the two sentinels so the scan
// can skip exactly this span and nothing else, and every run reports that it did
// so — but that exemption is local. Upstream push protection pattern-matches
// credential shapes across every pushed blob and hard-blocks the whole push, and
// it has no knowledge of these sentinels or of `scripts/secret-scan-allowlist.json`.
// A shape written here as one contiguous literal is therefore a push blocker, so
// such values are assembled from fragments at runtime: the detector still sees a
// byte-identical sample and `--self-test` still proves a true positive, while the
// source carries no contiguous credential for a remote scanner to match.
// secret-corpus:begin-known-bad-corpus

// Join sample fragments at runtime. `secret` and `in` below are byte-identical to
// a written-out literal once this module runs; only the source bytes differ, and
// the source bytes are what a remote push-protection scan reads.
const frag = (...parts) => parts.join('');

const KNOWN_BAD = [
  { id: 'openai', expect: 'prefix:sk-', secret: frag('sk-proj-', '9T4mQ2vB8xL7nR3wZ6yH1jK5pC0dF4gA'), in: `const k = "${frag('sk-proj-', '9T4mQ2vB8xL7nR3wZ6yH1jK5pC0dF4gA')}";` },
  { id: 'anthropic', expect: 'prefix:sk-', secret: frag('sk-ant-api03-', '7Yq2Lx9RtVb4NmZ6KdWp3HcJf8SgE1Au5Oi0'), in: `provider_key: "${frag('sk-ant-api03-', '7Yq2Lx9RtVb4NmZ6KdWp3HcJf8SgE1Au5Oi0')}"` },
  { id: 'openrouter', expect: 'prefix:sk-', secret: frag('sk-or-v1-', '4d9c2a7b1e8f3c6a5b4d2e1f9c8b7a6d5e4f3c2b1a0d9e8f'), in: `{"apiKey":"${frag('sk-or-v1-', '4d9c2a7b1e8f3c6a5b4d2e1f9c8b7a6d5e4f3c2b1a0d9e8f')}"}` },
  { id: 'aws-access-key-id', expect: 'prefix:AKIA', secret: frag('AKIA', 'IOSFODNN7EXAMPLE'), in: `let id = "${frag('AKIA', 'IOSFODNN7EXAMPLE')}";` },
  { id: 'github-classic', expect: 'prefix:ghp_', secret: frag('ghp_', '16C7e42F292c6912E7710c838347Ae178B4a'), in: `token: ${frag('ghp_', '16C7e42F292c6912E7710c838347Ae178B4a')}` },
  { id: 'github-fine-grained', expect: 'prefix:github_pat_', secret: frag('github_pat_', '11ABCDEFG0aBcDeFgHiJkL_MnOpQrStUvWxYz0123456789AbCdEfGhIjKlMnOpQrSt'), in: `"${frag('github_pat_', '11ABCDEFG0aBcDeFgHiJkL_MnOpQrStUvWxYz0123456789AbCdEfGhIjKlMnOpQrSt')}"` },
  { id: 'slack', expect: 'prefix:xoxb-', secret: frag('xoxb-', '2385917462-', '2837483920-', 'Kq8vBn3mXpL5ZtR7wYc1'), in: `slackToken = "${frag('xoxb-', '2385917462-', '2837483920-', 'Kq8vBn3mXpL5ZtR7wYc1')}";` },
  { id: 'google-api-key', expect: 'prefix:AIza', secret: frag('AIza', 'SyD-9tSrke72PouQMnMX-a7eZSW0jkFMBWY'), in: `const GOOGLE_KEY = "${frag('AIza', 'SyD-9tSrke72PouQMnMX-a7eZSW0jkFMBWY')}";` },
  { id: 'google-oauth', expect: 'prefix:ya29.', secret: frag('ya29.', 'A0ARrdaM9tK4mB7yQ1cX8nP2sV6uZ3wL5hJ0eF8gK2nD7xQ'), in: `refresh: "${frag('ya29.', 'A0ARrdaM9tK4mB7yQ1cX8nP2sV6uZ3wL5hJ0eF8gK2nD7xQ')}"` },
  { id: 'jwt', expect: 'jwt', secret: frag('eyJhbGciOiJIUzI1', 'NiIsInR5cCI6IkpXVCJ9', '.', 'eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0', '.', 'dBjftJeZ4CVPmB92K27uhbUJU1p1r', '_wW1gFWFOEjXk'), in: `"${frag('eyJhbGciOiJIUzI1', 'NiIsInR5cCI6IkpXVCJ9', '.', 'eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0', '.', 'dBjftJeZ4CVPmB92K27uhbUJU1p1r', '_wW1gFWFOEjXk')}"` },
  { id: 'pem-private-key', expect: 'pem-private-key', secret: frag('-----BEGIN RSA', ' PRIVATE KEY-----'), in: `${frag('-----BEGIN RSA', ' PRIVATE KEY-----')}\nMIIEow==\n${frag('-----END RSA', ' PRIVATE KEY-----')}` },
  { id: 'bearer-header', expect: 'bearer-token', secret: frag('4Zt9xQw2', 'Lm7Nv1Kp8Rd3Sf6Hg0Jc5Yb1Ua2Xe4Ni'), in: `Authorization: 'Bearer ${frag('4Zt9xQw2', 'Lm7Nv1Kp8Rd3Sf6Hg0Jc5Yb1Ua2Xe4Ni')}'` },
  { id: 'assigned-secret', expect: 'assigned-secret', secret: frag('Tr0ub4dor', '&3xKcd!mn9Qv2wLpZ'), in: `const vaultPassword = "${frag('Tr0ub4dor', '&3xKcd!mn9Qv2wLpZ')}";` },
  { id: 'env-secret', expect: 'env-secret', secret: frag('9fKq2LmZ', '7xRw4Tb1Vc6Yn0Ps3Dg8Hj5Kl2Fq7Bx1'), in: `AGENTCOWORK_VAULT_KEY=${frag('9fKq2LmZ', '7xRw4Tb1Vc6Yn0Ps3Dg8Hj5Kl2Fq7Bx1')}` },
  { id: 'high-entropy-blob', expect: 'high-entropy-blob', secret: frag('Zm9vYmFy', 'MTIzNDU2Nzg5MEFCQ0RlRmdISUpLTE1OT1BRUlNUVVZXWFla'), in: `"${frag('Zm9vYmFy', 'MTIzNDU2Nzg5MEFCQ0RlRmdISUpLTE1OT1BRUlNUVVZXWFla')}"` },
  { id: 'hex-blob', expect: 'hex-digest-blob', secret: frag('a3f1c95e', '7b2d4806af13ce95b7d2048ea16fb3c97d5e0a4b83c1f6927de5b0a48'), in: `digest = "${frag('a3f1c95e', '7b2d4806af13ce95b7d2048ea16fb3c97d5e0a4b83c1f6927de5b0a48')}"` },
  { id: 'credentialed-url', expect: 'url-userinfo', secret: frag('Zt7Qw2Lm', '9Xk4Rb1Vc6Yn0Ps3Dg8Hj5Kl'), in: `await fetch("https://svc:${frag('hZt7Qw2', 'Lm9Xk4Rb1Vc6Yn0Ps3Dg8Hj5Kl')}@internal.example/api")` },
  // A *public* key half is not a secret, and no entropy floor can tell it from
  // a private one. The gate reports the shape; the allowlist carries the review.
  { id: 'public-key-blob', expect: 'high-entropy-blob', secret: frag('lvI3luTa', 'tntgPJAIeBRIFHJsYv3CQRUCMZg97OYZrT0='), in: `STORE_PUBLIC_KEY_B64 = "${frag('lvI3luTa', 'tntgPJAIeBRIFHJsYv3CQRUCMZg97OYZrT0=')}"` },
  // A `sk-`-prefixed literal is a structural finding even when its body is a
  // placeholder: the redactor must still see it, so the gate reports it and the
  // review (allowlist) decides. The placeholder screen is a *heuristic* gate.
  { id: 'structural-beats-placeholder', expect: 'prefix:sk-', secret: frag('sk-', 'portable-000000000000'), in: `secret: "${frag('sk-', 'portable-000000000000')}"` },
  // The value whose fingerprint the allowlist carries for the vault at-rest
  // fixture (`crates/agentcowork-vault/src/lib.rs`). Pinned below so the two
  // cannot drift apart silently.
  { id: 'allowlist-key-pin', expect: 'prefix:sk-', secret: frag('sk-', 'portable-secret-42'), in: `let secret = "${frag('sk-', 'portable-secret-42')}";` },
];
// secret-corpus:end-known-bad-corpus

const KNOWN_GOOD = [
  { id: 'camel-case-identifier', in: `mergeHydratedSessionsWithProviderObservationDuringInitialHydrationPass` },
  { id: 'long-prose', in: `hint: 'Re-check the key in Settings, it may be revoked or pasted with extra whitespace'` },
  { id: 'model-id', in: `modelId: "anthropic/claude-sonnet-4-20250514-thinking-16k"` },
  { id: 'content-type', in: `accept: 'application/json'` },
  { id: 'data-uri', in: `src = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="` },
  { id: 'placeholder-upper', in: `apiKey: "YOUR_API_KEY_HERE"` },
  { id: 'placeholder-mask', in: `api_key: "<redacted>"` },
  { id: 'env-reference', in: `const key = process.env.AGENTCOWORK_VAULT_KEY;` },
  { id: 'template-interpolation', in: 'const url = `https://api.example.com/v1?key=${apiKey}`;' },
  { id: 'git-sha', in: `const base = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";` },
  { id: 'uuid', in: `const sessionId = "9f3b7c1d-5a8e-4f2b-8c0d-9e1a3f5b7c9d";` },
  { id: 'oidc-scopes', in: `scopes: "repo read:org user:email workflow"` },
  { id: 'cidv0-content-hash', in: `const cid = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";` },
  { id: 'public-address', in: `const to = "0x71C7656EC7ab88b098defB751B7401B5f6d8976F";` },
  { id: 'hex-placeholder', in: `sha256: 0000000000000000000000000000000000000000000000000000000000000000` },
  { id: 'bare-prefix-string', in: `if (lower.starts_with("github_pat_"))` },
  { id: 'import-path', in: `import { run } from "agentcowork-acp/src/agent_backend";` },
  { id: 'css-classes', in: `className="h-6 rounded-md border-input bg-background px-2 text-xs font-medium"` },
  { id: 'token-counter', in: `tokensThisTurn={Math.round(streamStats.tokensThisTurn / 1000)}` },
  { id: 'semver', in: `version: "1.12.0-beta.3+build.20260926"` },
];

/** Deliberate non-detections, each with the reason it is not a finding. */
const KNOWN_NOTE = [
  { id: 'weak-password-fixture', in: `out = redact_secrets("password: hunter2hunter2");`, why: 'the assigned-secret rule requires 3 character classes; a two-class weak password is below the bar (see report §limitations)' },
  { id: 'low-class-api-key-fixture', in: `api_key = 'abcd1234efgh5678'`, why: 'two character classes only — the placeholder-ish shape the heuristic rules decline to claim' },
  { id: 'bare-bearer-word', in: `Authorization: "Bearer"`, why: 'no credential material after the scheme' },
];

/** Longest run of a secret's own characters that must never reach the report. */
const MIN_LEAK_RUN = 8;

function selfTest() {
  const problems = [];

  for (const sample of KNOWN_BAD) {
    const findings = detect(sample.in);
    if (findings.length === 0) {
      problems.push(`known-bad ${sample.id}: not detected`);
      continue;
    }
    const rules = [...new Set(findings.flatMap((f) => [...f.rules]))];
    if (!rules.includes(sample.expect)) {
      problems.push(`known-bad ${sample.id}: expected rule ${sample.expect}, got ${rules.sort().join(', ')}`);
      continue;
    }
    // The redaction contract, on the real formatting path: a finding's printable
    // form must not contain any 8+ character run of the secret's *unpredictable*
    // part. The structural prefix (`sk-`, `github_pat_`, …) is a public
    // classification the kernel redactor table already publishes in-repo, and
    // the rule id necessarily names it — so it is excluded from the check.
    const rendered = findings.map((f) => formatFinding({ ...f, path: `self-test/${sample.id}` })).join('\n');
    const publicPrefix = PREFIX_RULES.find((r) => sample.secret.startsWith(r.prefix))?.prefix ?? '';
    const unpredictable = publicPrefix ? sample.secret.slice(publicPrefix.length) : sample.secret;
    for (let i = 0; i + MIN_LEAK_RUN <= unpredictable.length; i += 1) {
      if (rendered.includes(unpredictable.slice(i, i + MIN_LEAK_RUN))) {
        problems.push(`known-bad ${sample.id}: the report line leaks the value`);
        break;
      }
    }
  }

  for (const sample of KNOWN_GOOD) {
    const findings = detect(sample.in);
    if (findings.length > 0) {
      problems.push(`known-good ${sample.id}: false positive via ${[...new Set(findings.flatMap((f) => [...f.rules]))].sort().join(', ')}`);
    }
  }

  for (const sample of KNOWN_NOTE) {
    const findings = detect(sample.in);
    if (findings.length > 0) {
      problems.push(`documented non-detection ${sample.id} (${sample.why}): now flagged by ${[...new Set(findings.flatMap((f) => [...f.rules]))].sort().join(', ')} — update the note or the rule`);
    }
  }

  // Same value, two rules → one reviewable finding. Reuses a table sample so
  // no credential-shaped literal lives outside the sentinel span.
  const [openai] = KNOWN_BAD;
  const merged = detect(`apiKey: "${openai.secret}"`);
  if (merged.length !== 1 || merged[0].rules.size < 2) {
    problems.push('a value matched by two rules is not merged into one finding');
  }

  // The fingerprint is the allowlist's key, so pin its exact value: this ties
  // the self-test to the key a reviewer wrote into the allowlist for the vault
  // at-rest fixture, and fails loudly if the hash prefix or the truncation ever
  // changes (every allowlist entry would then need re-deriving).
  const pin = KNOWN_BAD.find((s) => s.id === 'allowlist-key-pin');
  if (fingerprint(pin.secret) !== 'be88632f04f9') {
    problems.push(`fingerprint drift: the allowlist-key pin hashes to ${fingerprint(pin.secret)}, the allowlist assumes be88632f04f9 — re-derive every entry`);
  }

  if (problems.length > 0) {
    console.error('secret-corpus: SELF-TEST FAILED');
    for (const p of problems) console.error(`  ✗ ${p}`);
    return 1;
  }
  console.log(
    `secret-corpus: SELF-TEST PASS — ${KNOWN_BAD.length}/${KNOWN_BAD.length} known-bad detected with the expected rule, ` +
    `${KNOWN_GOOD.length}/${KNOWN_GOOD.length} known-good clean, ${KNOWN_NOTE.length} documented non-detection(s) unchanged, ` +
    'report redaction contract holds (no 8-char run of a secret reaches a finding line)',
  );
  return 0;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const opts = { selfTest: false, json: false, printAllowlist: false, paths: null, allowlist: null, minEntropy: null, minBlobLength: null };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--self-test') opts.selfTest = true;
    else if (a === '--json') opts.json = true;
    else if (a === '--print-allowlist') opts.printAllowlist = true;
    else if (a === '--help' || a === '-h') opts.help = true;
    else if (a === '--paths') opts.paths = argv[++i];
    else if (a === '--allowlist') opts.allowlist = argv[++i];
    else if (a.startsWith('--min-entropy=')) opts.minEntropy = Number(a.split('=')[1]);
    else if (a === '--min-entropy') opts.minEntropy = Number(argv[++i]);
    else if (a.startsWith('--min-blob-length=')) opts.minBlobLength = Number(a.split('=')[1]);
    else if (a === '--min-blob-length') opts.minBlobLength = Number(argv[++i]);
    else if (a === '--paths=') opts.paths = a.split('=').slice(1).join('=');
    else if (a === '--allowlist=') opts.allowlist = a.split('=').slice(1).join('=');
    else return { error: `unknown argument: ${a}` };
  }
  if (opts.minEntropy !== null && (!Number.isFinite(opts.minEntropy) || opts.minEntropy <= 0)) return { error: '--min-entropy needs a positive number' };
  if (opts.minBlobLength !== null && (!Number.isFinite(opts.minBlobLength) || opts.minBlobLength < 16)) return { error: '--min-blob-length needs a number >= 16' };
  return opts;
}

const USAGE = `usage: node scripts/check-secret-corpus.mjs [--self-test] [--json] [--paths "<dir-or-file>[,...]"]
                        [--allowlist <file>] [--min-entropy <bits>] [--min-blob-length <chars>]
                        [--print-allowlist]

UI-observable secret-corpus scan (TASK-UI-001 / FIX-04 / REQ-PROD-002 / INV-02).
  (no flag)        scan the default corpus; allowlisted findings pass, anything
                   else exits 1 with a location report
  --self-test      prove the detector on known-bad/known-good samples, then exit
  --json           machine-readable report on stdout
  --paths          scan these roots instead of the default corpus
  --allowlist      reviewed-expectation file (default scripts/secret-scan-allowlist.json)
  --print-allowlist  print ready-to-paste entries for the unallowlisted findings
  --min-entropy    raise the generic blob entropy floor (default 3.9)
  --min-blob-length  raise the generic blob length floor (default 40)

exit: 0 pass · 1 findings or stale expectations · 2 cannot run`;

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    console.log(USAGE);
    return 0;
  }
  if (opts.error) {
    console.error(`secret-corpus: ${opts.error}`);
    console.error(USAGE);
    return 2;
  }
  // Tuning is applied to the live rule table, before anything else, so
  // `--self-test` always proves the *effective* configuration rather than the
  // default one — and so a tuning flag can never be a silent no-op.
  for (const rule of RULES) {
    if (opts.minEntropy !== null && rule.id === 'high-entropy-blob') rule.minEntropy = opts.minEntropy;
    if (opts.minBlobLength !== null && rule.id === 'high-entropy-blob') {
      rule.re = new RegExp(`(?<![A-Za-z0-9+/=_-])[A-Za-z0-9+/]{${opts.minBlobLength},}={0,2}(?![A-Za-z0-9+/=_-])`);
    }
  }
  if (opts.selfTest) return selfTest();

  const vocab = vocabulary();
  if (!vocab.ok) {
    console.error(`secret-corpus: cannot run — ${vocab.reason}`);
    return 2;
  }

  const allow = loadAllowlist(opts.allowlist);
  if (allow.error) {
    console.error(`secret-corpus: cannot run — ${allow.error}`);
    return 2;
  }

  // ---- corpus ------------------------------------------------------------
  const groups = opts.paths
    ? [{ id: 'override', label: 'operator-supplied roots', roots: opts.paths.split(',').map((s) => s.trim()).filter(Boolean) }]
    : CORPUS;
  if (groups[0].roots.length === 0) {
    console.error('secret-corpus: cannot run — no corpus roots (refusing to pass a gate that scanned nothing)');
    return 2;
  }
  for (const g of groups) {
    for (const r of g.roots) {
      if (!existsSync(join(ROOT, r))) {
        console.error(`secret-corpus: cannot run — corpus root does not exist: ${r}`);
        return 2;
      }
    }
  }

  const files = new Set();
  const perGroup = [];
  for (const g of groups) {
    const owned = [];
    // A fresh visited-set per group: it stops a symlink loop inside one root,
    // and `files` collapses the overlap between groups.
    for (const r of g.roots) walk(join(ROOT, r), owned, new Set());
    for (const f of owned) files.add(f);
    perGroup.push({ id: g.id, label: g.label, roots: g.roots, count: owned.length });
  }
  if (files.size === 0) {
    console.error('secret-corpus: cannot run — the corpus resolved to 0 files (refusing to pass a gate that scanned nothing)');
    return 2;
  }

  // ---- scan --------------------------------------------------------------
  const findings = [];
  const scanErrors = [];
  let scanned = 0;
  let skipped = 0;
  let masked = 0;
  let bytes = 0;
  for (const abs of [...files].sort()) {
    const res = scanFile(abs);
    scanned += 1;
    skipped += res.skipped;
    masked += res.masked;
    if (res.error) scanErrors.push(res.error);
    try { bytes += statSync(abs).size; } catch { /* counted as scanned */ }
    findings.push(...res.findings);
  }
  if (scanErrors.length > 0) {
    for (const e of scanErrors) console.error(`secret-corpus: cannot run — ${e}`);
    return 2;
  }
  findings.sort((a, b) => a.path.localeCompare(b.path) || a.line - b.line || a.col - b.col || [...a.rules].sort()[0].localeCompare([...b.rules].sort()[0]));

  // ---- classify against the reviewed expectations -------------------------
  const allowed = new Map();
  for (const e of allow.entries) for (const fp of e.fingerprints) allowed.set(allowlistKey(e.path, fp), e);
  const unallowlisted = [];
  const allowlisted = [];
  for (const f of findings) {
    const e = allowed.get(allowlistKey(f.path, f.fp));
    if (e) allowlisted.push({ ...f, entry: e, reason: e.reason });
    else unallowlisted.push(f);
  }
  const used = new Set();
  for (const f of allowlisted) used.add(allowlistKey(f.path, f.fp));
  const stale = allow.entries.filter((e) => e.fingerprints.some((fp) => !used.has(allowlistKey(e.path, fp))));

  const ok = unallowlisted.length === 0 && stale.length === 0;
  const report = {
    gate: 'secret-corpus',
    req: 'REQ-PROD-002',
    task: 'TASK-UI-001',
    fix: 'FIX-04',
    corpus: perGroup.map((g) => ({ id: g.id, label: g.label, roots: g.roots })),
    filesScanned: scanned,
    bytesScanned: bytes,
    filesSkipped: skipped,
    selfCorpusRegionsMasked: masked,
    vocabulary: { source: VOCAB_SOURCE, kernelPrefixes: vocab.kernel.length, covered: vocab.missing.length === 0 },
    findings: { total: findings.length, allowlisted: allowlisted.length, unallowlisted: unallowlisted.length },
    unallowlisted: unallowlisted.map((f) => ({ ...f, rules: [...f.rules].sort(), why: formatWhy(f) })),
    allowlisted: allowlisted.map((f) => ({ ...f, entry: undefined, rules: [...f.rules].sort(), why: formatWhy(f) })),
    staleAllowlist: stale.map((e) => ({ path: e.path, fingerprints: e.fingerprints, reason: e.reason })),
    verdict: ok ? 'PASS' : 'FAIL',
  };

  if (opts.json) {
    console.log(JSON.stringify(report, null, 2));
    return ok ? 0 : 1;
  }

  console.log(`secret-corpus: scanned ${scanned} file(s) (${(bytes / 1024).toFixed(0)} KiB) across ${groups.length} corpus group(s)${skipped > 0 ? `, ${skipped} skipped (binary or oversize)` : ''}`);
  for (const g of perGroup) console.log(`  ${g.id.padEnd(7)} ${String(g.count).padStart(4)} file(s)  ${g.label}`);
  console.log(`  vocabulary: ${vocab.kernel.length} kernel prefix(es) in ${VOCAB_SOURCE}, all covered by this gate`);
  if (masked > 0) {
    console.log(`  exempt: ${masked} region(s) — ${SELF_FILE} between ${SELF_CORPUS_BEGIN} and ${SELF_CORPUS_END} (this detector's own known-bad corpus; every other byte of that file is still scanned)`);
  }

  if (allowlisted.length > 0) {
    console.log(`\nALLOWLISTED — ${allowlisted.length} finding(s) covered by ${allow.entries.length} reviewed expectation(s) in ${allow.path.slice(ROOT.length + 1).replace(/\\/g, '/')}`);
    // One reason per reviewed entry, its findings listed underneath: the report
    // is meant to be read as the review itself.
    const grouped = new Map();
    for (const f of allowlisted) {
      if (!grouped.has(f.entry)) grouped.set(f.entry, []);
      grouped.get(f.entry).push(formatFinding(f));
    }
    for (const [entry, lines] of grouped) {
      for (const line of lines.sort()) console.log(`  ${line}`);
      console.log(`      ↳ ${entry.path}: ${entry.reason}`);
    }
  }
  if (unallowlisted.length > 0) {
    console.error(`\nUNALLOWLISTED — ${unallowlisted.length} finding(s): a credential-shaped literal the UI can observe, with no reviewed exception`);
    for (const f of unallowlisted) {
      console.error(`  ${formatFinding(f)}`);
      console.error(`      ${formatWhy(f)}`);
    }
  }
  if (stale.length > 0) {
    console.error(`\nSTALE EXPECTATIONS — ${stale.length} allowlist entr(ies) no longer match any finding; a deleted or renamed fixture must be re-reviewed, not left behind`);
    for (const e of stale) console.error(`  ${e.path}  ${e.fingerprints.join(', ')}\n      ${e.reason}`);
  }

  if (opts.printAllowlist) {
    const stub = unallowlisted.length === 0 ? [] : unallowlisted.map((f) => ({
      path: f.path,
      fingerprints: [f.fp],
      reason: 'TODO: state why this value is not credential material (or delete it and let the gate pass clean)',
      reviewed: 'YYYY-MM-DD',
    }));
    console.log('\nallowlist stub (paste into scripts/secret-scan-allowlist.json — one entry per finding, reasons are the review):');
    console.log(JSON.stringify(stub, null, 2));
  }

  if (!ok) {
    console.error(`\nsecret-corpus: FAIL — ${unallowlisted.length} unallowlisted finding(s), ${stale.length} stale expectation(s)`);
    console.error('secret-corpus: a finding is a LOCATION plus a rule, never a value — do not paste the secret into the report.');
    return 1;
  }
  console.log(`\nsecret-corpus: PASS — ${scanned} file(s) scanned, ${findings.length} finding(s), all covered by a reviewed expectation`);
  console.log('secret-corpus: limits — structural rules (prefix / PEM / JWT) are authoritative; the heuristic rules (assigned-secret, env-secret, bearer-token,');
  console.log('               url-userinfo, high-entropy-blob, hex-digest-blob) need entropy >= 3.0-3.9 bits/char AND >= 2-3 character classes, so a');
  console.log('               low-entropy or two-class credential ("hunter2hunter2", "abcd1234efgh5678") is a documented non-detection, and a base64 payload');
  console.log('               that decodes to a slash-separated word list is excluded. Tune with --min-entropy / --min-blob-length.');
  return 0;
}

process.exit(main());
