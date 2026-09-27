#!/usr/bin/env node
// AgentCowork — CODEBASE-MAP.md generator.
//
// Regenerates ONLY the region between the two markers in CODEBASE-MAP.md
// (everything under `BEGIN-GENERATED-INVENTORY`). The narrative in sections
// 1-8 is hand-authored and is never touched.
//
// What it produces, mechanically and exhaustively over every git-tracked file:
//   Section 9   Rust workspace, per crate, per file: lines, module doc, public
//               items, every function (public AND private) with its line
//               number, test count, and a cross-file reference count.
//   Section 10  TS/TSX, per area, per file: lines, exports, EVERY named
//               function (parsed with the TypeScript compiler — declarations,
//               class/object methods, constructors, accessors and named fn
//               expressions), the count of anonymous fn expressions, test
//               count, and an import-graph wiring verdict.
//   Section 11  Wiring & status matrix (orphans / test-only / ghosts).
//   Section 12  Dependencies (Cargo per crate, package.json per package).
//   Section 13  Non-source inventory: every remaining tracked file — Cargo
//               manifests, CI workflows, the deploy/ node pack, tsconfigs,
//               capability manifests, lockfiles, scripts, icons, and the
//               committed .turbo build logs (flagged as such).
//   Section 14  Documentation index: every tracked .md with title, line count
//               and opening sentence.
//   Section 15  Appendices: Tauri command registry + UI call sites, coordinator
//               IPC method registry, a full directory/file index, and a
//               coverage audit that fails the run if any tracked file is
//               unaccounted for.
//
// The command-registry matcher is ported from scripts/ipc-parity.mjs (the
// project's own balance-aware invoke() extractor) rather than approximated;
// a naive generic pattern silently drops real call sites.
//
// Completeness is enforced, not asserted: every renderer registers the paths it
// gives a `####` entry to in ACCOUNTED, and main() diffs that set against
// `git ls-files` at the end. A non-empty diff exits 1 after writing, so a
// silently-dropped file class can never reappear.
//
// Usage: node scripts/gen-codebase-map.mjs [--check]
//   --check   exit 1 if the generated section is stale, without rewriting.
//
// 2026-09-26: the map is archived at
// ARCHIVE/v0/repo-cleanup-2026-09-26/CODEBASE-MAP.md; `--check` is no longer
// wired into CI. Restore the map to the repo root before running this
// generator (it edits CODEBASE-MAP.md in place).

import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { readFileSync, writeFileSync, existsSync, statSync } from "node:fs";
import { dirname, join, extname, basename } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const MAP_PATH = join(ROOT, "CODEBASE-MAP.md");
/** This generator's own output — excluded from every self-referential stat. */
const SELF_MAP = "CODEBASE-MAP.md";
const BEGIN = "<!-- BEGIN-GENERATED-INVENTORY -->";
const END = "<!-- END-GENERATED-INVENTORY -->";

const CHECK = process.argv.includes("--check");

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/** Every git-tracked file, repo-relative, POSIX separators, sorted. */
function trackedFiles() {
  const raw = execFileSync("git", ["ls-files", "-z"], { cwd: ROOT, maxBuffer: 1 << 28 });
  return raw
    .toString("utf8")
    .split("\0")
    .filter(Boolean)
    .sort();
}

const FILES = trackedFiles();
const textCache = new Map();

function read(rel) {
  if (textCache.has(rel)) return textCache.get(rel);
  let out = "";
  try {
    out = readFileSync(join(ROOT, rel), "utf8");
  } catch {
    out = "";
  }
  textCache.set(rel, out);
  return out;
}

const isText = (rel) =>
  /\.(rs|ts|tsx|js|mjs|cjs|json|toml|yaml|yml|md|css|html|sh|py|log|txt|lock|service|plist)$/.test(
    rel,
  ) ||
  /(^|\/)(LICENSE|LICENSE-APACHE|LICENSE-MIT|Dockerfile|Makefile|\.gitignore|\.pre-commit-config\.yaml)$/.test(rel);

const countLines = (s) => (s === "" ? 0 : s.split("\n").length);
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

// ---------------------------------------------------------------------------
// Common text helpers
// ---------------------------------------------------------------------------

/** Leading `//!` block joined and truncated at the first sentence. */
function moduleDoc(lines) {
  const buf = [];
  for (const line of lines) {
    const t = line.trim();
    if (t.startsWith("#![")) continue;
    if (t.startsWith("//!")) buf.push(t.replace(/^\/\/!\s?/, ""));
    else if (buf.length) break;
    else if (t === "" || t.startsWith("//")) continue;
    else break;
  }
  return sentence(buf.join(" "));
}

/** Contiguous `///` block immediately above a declaration. */
function itemDoc(lines, idx) {
  const buf = [];
  for (let i = idx - 1; i >= 0; i--) {
    const t = lines[i].trim();
    if (t.startsWith("///")) buf.unshift(t.replace(/^\/\/\/\s?/, ""));
    else if (t === "") continue;
    else break;
  }
  return sentence(buf.join(" "));
}

function sentence(s) {
  const one = s.replace(/\s+/g, " ").trim();
  if (!one) return "";
  const cut = one.search(/\.(\s|$)/);
  const out = cut > 40 ? one.slice(0, cut + 1) : one;
  return out.length > 220 ? out.slice(0, 217) + "..." : out;
}

// ---------------------------------------------------------------------------
// Rust analysis
// ---------------------------------------------------------------------------

const rustCache = new Map();

const RUST_ITEM = /^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?(fn|struct|enum|trait|const|static|type)\s+([A-Za-z_][A-Za-z0-9_]*)/;
const RUST_FN = /^\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/;
const RUST_TEST = /^\s*#\[(?:tokio::)?test\]/;

function analyzeRust(rel) {
  if (rustCache.has(rel)) return rustCache.get(rel);
  const lines = read(rel).split("\n");
  const publics = [];
  const fns = [];
  let tests = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (RUST_TEST.test(line)) tests++;
    const fn = RUST_FN.exec(line);
    if (fn) fns.push([fn[1], i + 1]);
    const item = RUST_ITEM.exec(line);
    if (item) {
      publics.push({ kind: item[1], name: item[2], line: i + 1, doc: itemDoc(lines, i) });
    }
  }
  const res = { lines: lines.length, doc: moduleDoc(lines), publics, fns, tests };
  rustCache.set(rel, res);
  return res;
}

// ---------------------------------------------------------------------------
// TS / TSX analysis
// ---------------------------------------------------------------------------

const tsCache = new Map();

const TS_EXPORT_DECL = /^export\s+(?:default\s+)?(?:declare\s+)?(?:async\s+)?(function|const|let|var|class|interface|type|enum)\s+([A-Za-z_$][\w$]*)/;
// A line regex cannot see class methods, object-literal methods, constructors,
// accessors or named function expressions: none of them start with `function`.
// The TypeScript compiler is already a workspace devDependency, so parse with
// it when it resolves; otherwise fall back to the regex walker and SAY SO in
// the rendered section. A degraded map is acceptable; a silently partial one
// is not.
let ts = null;
try {
  const req = createRequire(import.meta.url);
  ts = req(req.resolve("typescript", { paths: [join(ROOT, "ui"), ROOT] }));
} catch {
  ts = null;
}
const TS_MODE = ts ? "ast" : "regex";

const TS_EXPORT_BRACE = /^export\s+(?:type\s+)?\{([^}]*)\}/;
const TS_FN = /^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/;
const TS_FN_CONST = /^\s*(?:export\s+)?const\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:function|\()/;
const TS_TEST_CASE = /^\s*(?:it|test)(?:\.\w+)?\s*\(/;
const TS_TEST_DESC = /^\s*describe\s*\(/;

/** Declaration name, tolerant of string / numeric / computed / private names. */
function tsNodeName(node, sf) {
  const n = node.name;
  if (!n) return null;
  if (ts.isPrivateIdentifier(n)) return `#${n.text}`;
  if (ts.isIdentifier(n) || ts.isStringLiteral(n) || ts.isNumericLiteral(n)) return n.text;
  const txt = n.getText(sf).replace(/\s+/g, " ");
  return txt.length > 40 ? txt.slice(0, 37) + "..." : txt;
}

/** For an anonymous fn expression, the name it is bound to (else null). */
function tsBindingName(node) {
  const p = node.parent;
  if (!p) return null;
  if (ts.isVariableDeclaration(p)) return ts.isIdentifier(p.name) ? p.name.text : null;
  if (ts.isPropertyAssignment(p) || ts.isPropertyDeclaration(p)) return tsNodeName(p, node.getSourceFile());
  if (ts.isBinaryExpression(p) && p.operatorToken.kind === ts.SyntaxKind.EqualsToken && ts.isIdentifier(p.left))
    return p.left.text;
  if (ts.isExportAssignment(p)) return "default";
  return null;
}

/** Every function-like node with its binding name; anonymous ones counted. */
function tsAstWalk(rel, src) {
  const sf = ts.createSourceFile(
    rel,
    src,
    ts.ScriptTarget.Latest,
    true,
    rel.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  const fns = [];
  let anon = 0;
  const at = (node) => sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
  const isDefault = (node) =>
    ts.canHaveModifiers(node) &&
    (ts.getModifiers(node) ?? []).some((m) => m.kind === ts.SyntaxKind.DefaultKeyword);
  const visit = (node) => {
    if (ts.isFunctionDeclaration(node)) {
      fns.push([node.name?.text ?? (isDefault(node) ? "default" : "<function>"), at(node)]);
    } else if (ts.isMethodDeclaration(node)) {
      fns.push([tsNodeName(node, sf) ?? "<method>", at(node)]);
    } else if (ts.isConstructorDeclaration(node)) {
      fns.push(["constructor", at(node)]);
    } else if (ts.isGetAccessorDeclaration(node)) {
      fns.push([`get ${tsNodeName(node, sf) ?? ""}`.trim(), at(node)]);
    } else if (ts.isSetAccessorDeclaration(node)) {
      fns.push([`set ${tsNodeName(node, sf) ?? ""}`.trim(), at(node)]);
    } else if (ts.isFunctionExpression(node) || ts.isArrowFunction(node)) {
      const bound = tsBindingName(node);
      if (bound) fns.push([bound, at(node)]);
      else anon++;
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  fns.sort((a, b) => a[1] - b[1]);
  return { fns, anon };
}

function analyzeTs(rel) {
  if (tsCache.has(rel)) return tsCache.get(rel);
  const src = read(rel);
  const lines = src.split("\n");
  const exports = [];
  let tests = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (TS_TEST_CASE.test(line) || TS_TEST_DESC.test(line)) tests++;
    const ed = TS_EXPORT_DECL.exec(line);
    if (ed) exports.push(`${ed[1]} ${ed[2]}`);
    const eb = TS_EXPORT_BRACE.exec(line);
    if (eb) {
      for (const part of eb[1].split(",")) {
        const name = part.trim().split(/\s+as\s+/).pop()?.trim();
        if (name) exports.push(`re-export ${name}`);
      }
    }
  }

  let fns = [];
  let anon = 0;
  if (ts) {
    ({ fns, anon } = tsAstWalk(rel, src));
  } else {
    for (let i = 0; i < lines.length; i++) {
      const fn = TS_FN.exec(lines[i]);
      if (fn) fns.push([fn[1], i + 1]);
      else {
        const fc = TS_FN_CONST.exec(lines[i]);
        if (fc) fns.push([fc[1], i + 1]);
      }
    }
  }

  const res = { lines: lines.length, doc: moduleDoc(lines), exports, fns, anon, tests, mode: TS_MODE };
  tsCache.set(rel, res);
  return res;
}

// ---------------------------------------------------------------------------
// Rust wiring (reference counting)
// ---------------------------------------------------------------------------

const RUST_FILES = FILES.filter((f) => f.endsWith(".rs"));
const GENERIC_MODULE_NAMES = new Set([
  "lib", "main", "mod", "types", "type", "config", "state", "util",
  "utils", "error", "errors", "helpers", "helper", "test", "tests", "mods", "prelude",
]);

/**
 * For each Rust file, how many OTHER files reference `<stem>::`.
 *
 * Implemented as one alternation scan per file (not one regex per file-pair),
 * so it is O(files x bytes) rather than O(files^2 x bytes). Two modules with
 * the same stem in different crates collapse into one bucket — a known and
 * accepted approximation.
 */
function rustRefCounts() {
  const owner = new Map();
  for (const f of RUST_FILES) {
    const stem = basename(f, ".rs");
    if (GENERIC_MODULE_NAMES.has(stem)) continue;
    if (!owner.has(stem)) owner.set(stem, f);
  }
  const alt = [...owner.keys()].sort((a, b) => b.length - a.length).map(escapeRe).join("|");
  const scan = new RegExp(`\\b(${alt})::`, "g");
  const tally = new Map();
  for (const f of RUST_FILES) {
    const src = read(f);
    if (!src) continue;
    scan.lastIndex = 0;
    let m;
    while ((m = scan.exec(src))) {
      const hit = owner.get(m[1]);
      if (hit === f) continue;
      tally.set(hit, (tally.get(hit) ?? 0) + 1);
    }
  }
  const out = new Map();
  for (const f of RUST_FILES) {
    const stem = basename(f, ".rs");
    out.set(
      f,
      GENERIC_MODULE_NAMES.has(stem)
        ? { refs: -1, ambiguous: true }
        : { refs: tally.get(f) ?? 0, ambiguous: false },
    );
  }
  return out;
}

// ---------------------------------------------------------------------------
// TS wiring (import-graph reachability)
// ---------------------------------------------------------------------------

const TS_FILES = FILES.filter((f) => /\.(ts|tsx)$/.test(f));
const TS_ENTRIES = ["ui/src/main.tsx", "ui/src/guard-main.ts", "packages/coordinator/src/index.ts"];

const isTestFile = (f) => /\.(test|spec)\.(ts|tsx)$/.test(f) || /(^|\/)__tests__\//.test(f);

function importSpecs(src) {
  const specs = [];
  const res = [
    /from\s+["']([^"']+)["']/g,
    /import\s+["']([^"']+)["']/g,
    /require\(\s*["']([^"']+)["']\s*\)/g,
  ];
  for (const re of res) {
    let m;
    while ((m = re.exec(src))) specs.push(m[1]);
  }
  return specs;
}

function resolveSpec(fromFile, spec) {
  const tryExts = ["", ".ts", ".tsx", ".js", ".jsx", "/index.ts", "/index.tsx", "/index.js"];
  const candidates = [];
  if (spec.startsWith("@/")) candidates.push(join("ui/src", spec.slice(2)));
  else if (spec.startsWith(".")) candidates.push(join(dirname(fromFile), spec));
  else if (spec.startsWith("@agentcowork/")) candidates.push(join("packages", spec.split("/")[1], "src/index"));
  else return null;

  for (const c of candidates) {
    for (const ext of tryExts) {
      const p = (c + ext).replace(/\/{2,}/g, "/");
      if (TS_FILES.includes(p)) return p;
    }
  }
  return null;
}

function tsReachability() {
  const graph = new Map();
  for (const f of TS_FILES) {
    graph.set(f, importSpecs(read(f)).map((s) => resolveSpec(f, s)).filter(Boolean));
  }
  const bfs = (entries, allowTests) => {
    const seen = new Set(entries.filter((e) => TS_FILES.includes(e)));
    const queue = [...seen];
    while (queue.length) {
      const cur = queue.shift();
      for (const next of graph.get(cur) ?? []) {
        if (seen.has(next)) continue;
        if (!allowTests && isTestFile(next)) continue;
        seen.add(next);
        queue.push(next);
      }
    }
    return seen;
  };
  const prod = bfs(TS_ENTRIES, false);
  const withTests = bfs(TS_ENTRIES, true);

  const importedBy = new Map();
  const importersOf = new Map();
  for (const [from, tos] of graph) {
    for (const t of tos) {
      importedBy.set(t, (importedBy.get(t) ?? 0) + 1);
      if (!importersOf.has(t)) importersOf.set(t, new Set());
      importersOf.get(t).add(from);
    }
  }

  /**
   * Reachability alone cannot spot a test-only module: nothing imports a test
   * file, so a test file is unreachable from the entries and every file it
   * imports then looks unreachable too. The real signal is the importer set.
   */
  const verdict = (f) => {
    if (isTestFile(f)) return "TEST";
    if (TS_ENTRIES.includes(f)) return "ENTRY";
    if (prod.has(f)) return "REACHABLE";
    const importers = [...(importersOf.get(f) ?? [])];
    if (importers.length === 0) return "NOT IMPORTED";
    if (importers.some(isTestFile)) return "TEST-ONLY";
    return "UNREACHED";
  };

  return { graph, prod, withTests, importedBy, importersOf, verdict };
}

// ---------------------------------------------------------------------------
// Tauri command registry
// ---------------------------------------------------------------------------

/**
 * Balance-aware `invoke("name")` extraction, ported from scripts/ipc-parity.mjs.
 * The generic argument list must be skipped by BRACKET BALANCE, not by a
 * character class: `invoke<Record<string, InstallState>>("x")` and
 * `invoke<{ s?: Array<import('./store').Session> }>('y')` both defeat `<[^>(]*>`
 * and were reported as ghosts while the UI called them on every load.
 */
function invokeNames(src) {
  const found = [];
  const re = /\binvoke\b/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    let i = m.index + m[0].length;
    if (src[i] === "<") {
      let depth = 0;
      while (i < src.length) {
        if (src[i] === "<") depth += 1;
        else if (src[i] === ">") {
          depth -= 1;
          if (depth === 0) {
            i += 1;
            break;
          }
        }
        i += 1;
      }
    }
    while (i < src.length && /\s/.test(src[i])) i += 1;
    if (src[i] !== "(") continue;
    i += 1;
    while (i < src.length && /\s/.test(src[i])) i += 1;
    const quote = src[i];
    if (quote !== '"' && quote !== "'") continue;
    let j = i + 1;
    let name = "";
    while (j < src.length && src[j] !== quote) {
      name += src[j];
      j += 1;
    }
    if (/^[a-z0-9_]+$/.test(name)) found.push(name);
  }
  return found;
}

/** Direct `invoke("x")` sites, plus the weaker "name appears as a literal" signal. */
function uiRefIndex() {
  const direct = new Map();
  const referenced = new Map();
  const bump = (map, k) => map.set(k, (map.get(k) ?? 0) + 1);
  for (const f of FILES.filter((x) => /^(ui|packages)\/.*\.(ts|tsx)$/.test(x))) {
    const src = read(f);
    for (const n of invokeNames(src)) bump(direct, n);
    for (const m of src.matchAll(/["']([a-z][a-z0-9_]{2,})["']/g)) bump(referenced, m[1]);
  }
  return { direct, referenced };
}

/**
 * Rust-side mentions of a command name, excluding its own definition and the
 * registration list. Answers "is anything on the Rust plane calling this?" for
 * commands the UI never names literally.
 */
function rustMentions(names) {
  const out = new Map();
  if (!names.length) return out;
  const alt = names.map(escapeRe).sort((a, b) => b.length - a.length).join("|");
  const anyRe = new RegExp(`\\b(${alt})\\b`, "g");
  const defRe = new RegExp(`fn\\s+(${alt})\\s*[<(]`, "g");
  for (const f of RUST_FILES) {
    if (f.endsWith("commands.rs")) continue;
    const src = read(f);
    if (!src) continue;
    anyRe.lastIndex = 0;
    let m;
    while ((m = anyRe.exec(src))) out.set(m[1], (out.get(m[1]) ?? 0) + 1);
    defRe.lastIndex = 0;
    let d;
    while ((d = defRe.exec(src))) out.set(d[1], (out.get(d[1]) ?? 0) - 1);
  }
  return out;
}

function tauriCommands() {
  const rel = "src-tauri/src/commands.rs";
  if (!existsSync(join(ROOT, rel))) return [];
  const body = read(rel).slice(read(rel).indexOf("generate_handler!["));
  const names = [];
  const seen = new Set();
  for (const line of body.split("\n")) {
    const seg = line.trim().replace(/,$/, "");
    if (!seg || seg.startsWith("//")) continue;
    // Accept an optional `crate::` prefix: `crate::model_cmds::model_serve` is a
    // registration like any other, and requiring exactly two segments silently
    // dropped the ten `model_*` commands the shell registers that way.
    const m = /^(?:crate::)?([a-z_][\w]*)::([a-z_][\w]*)$/.exec(seg);
    if (!m || seen.has(m[2])) continue;
    seen.add(m[2]);
    names.push({ name: m[2], module: m[1] });
  }
  const { direct, referenced } = uiRefIndex();
  const ghosts = names.filter((n) => !direct.has(n.name)).map((n) => n.name);
  const rust = rustMentions(ghosts);
  return names.map((n) => ({
    ...n,
    calls: direct.get(n.name) ?? 0,
    refs: referenced.get(n.name) ?? 0,
    rust: rust.get(n.name) ?? 0,
  }));
}

function coordinatorMethods() {
  const rel = "packages/coordinator/src/index.ts";
  if (!existsSync(join(ROOT, rel))) return [];
  const names = new Set();
  let m;
  const re = /case\s+"([a-z][\w/]*)"/g;
  const src = read(rel);
  while ((m = re.exec(src))) names.add(m[1]);
  return [...names].sort();
}

// ---------------------------------------------------------------------------
// Dependencies
// ---------------------------------------------------------------------------

function cargoDeps() {
  const out = [];
  for (const f of FILES.filter((x) => /^crates\/[^/]+\/Cargo\.toml$/.test(x))) {
    const src = read(f);
    const section = (name) => {
      const i = src.indexOf(`[${name}]`);
      if (i < 0) return [];
      const chunk = src.slice(i + name.length + 2);
      const stop = chunk.search(/^\[/m);
      const body = stop >= 0 ? chunk.slice(0, stop) : chunk;
      return body
        .split("\n")
        .map((l) => l.trim())
        .filter((l) => l && !l.startsWith("#"))
        .filter((l) => !l.startsWith("["))
        .map((l) => l.split("=")[0].trim().replace(/^"|"$/g, ""));
    };
    const deps = section("dependencies");
    out.push({
      crate: f.split("/")[1],
      deps,
      devDeps: section("dev-dependencies"),
      internal: deps.filter((d) => d.startsWith("agentcowork-")),
    });
  }
  return out.sort((a, b) => a.crate.localeCompare(b.crate));
}

function npmDeps() {
  const out = [];
  for (const f of FILES.filter((x) => x.endsWith("package.json"))) {
    let json;
    try {
      json = JSON.parse(read(f));
    } catch {
      continue;
    }
    out.push({
      path: f,
      name: json.name ?? "(unnamed)",
      scripts: Object.keys(json.scripts ?? {}),
      deps: Object.keys(json.dependencies ?? {}),
      devDeps: Object.keys(json.devDependencies ?? {}),
    });
  }
  return out.sort((a, b) => a.path.localeCompare(b.path));
}

// ---------------------------------------------------------------------------
// Coverage accounting
// ---------------------------------------------------------------------------

/**
 * Every tracked file that gets its own `####` entry, whichever section emits
 * it. main() diffs this against git ls-files; the map is "complete" only when
 * the diff is empty. Sections register here instead of claiming coverage in
 * prose, because prose claims are how the first draft of this map reported 100%
 * while 276 files had no entry at all.
 */
const ACCOUNTED = new Map();
/** Register a file's `####` entry, and which section emitted it. */
const acct = (rel, bucket) => ACCOUNTED.set(rel, bucket);

const bytesOf = (rel) => {
  try {
    return statSync(join(ROOT, rel)).size;
  } catch {
    return 0;
  }
};

const humanBytes = (n) =>
  n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`;

/** First `# ` heading, or the first non-empty line. */
function mdTitle(rel) {
  for (const line of read(rel).split("\n")) {
    const t = line.trim();
    if (!t) continue;
    if (t.startsWith("#")) return t.replace(/^#+\s*/, "").slice(0, 120);
    return t.slice(0, 120);
  }
  return "(empty)";
}

/** First sentence of body prose (skipping headings, quotes, tables, fences). */
/**
 * A relative link copied out of `dir/doc.md` and pasted into this map (which
 * always lives at the repo root) would point at the wrong place —
 * `[`CORE.md`](CORE.md)` inside `ARCH/01-SYSTEM-ARCHITECTURE.md` must resolve
 * as `ARCH/CORE.md` where it is quoted. Rewriting here is what keeps the
 * map's own links unbroken (`scripts/check-doc-refs.mjs` runs the check).
 */
function rootifyLinks(text, rel) {
  const dir = rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "";
  if (!dir) return text;
  return text.replace(/\]\(([^)\s]+)\)/g, (m, href) => {
    if (/^(https?:|mailto:|#|\/|[A-Za-z]:)/.test(href)) return m;
    return `](${dir}/${href})`;
  });
}

function mdLead(rel) {
  const lines = read(rel).split("\n");
  let i = 0;
  while (i < lines.length && (!lines[i].trim() || lines[i].trim().startsWith("#"))) i += 1;
  const buf = [];
  for (; i < lines.length && buf.join(" ").length < 260; i += 1) {
    const t = lines[i].trim();
    if (!t || t.startsWith("#") || t.startsWith("|") || t.startsWith("```")) {
      if (buf.length) break;
      continue;
    }
    buf.push(t.replace(/^>+\s*/, ""));
  }
  const text = buf.join(" ").replace(/\s+/g, " ").trim();
  const cut = /^(.{40,240}?[.!?])\s/.exec(text);
  return rootifyLinks((cut ? cut[1] : text.slice(0, 200)).trim(), rel);
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const fmtFns = (fns, limit = Infinity) =>
  fns.slice(0, limit).map(([n, l]) => `${n}:${l}`).join(", ") +
  (fns.length > limit ? `, ... (+${fns.length - limit} more)` : "");

const groupOf = (f) => (f.startsWith("crates/") ? f.split("/").slice(0, 2).join("/") : "src-tauri");

function renderRustSection(refs) {
  const lines = [];
  lines.push("## 9. Rust workspace — per-crate, per-file LLD");
  lines.push("");
  lines.push("**Every tracked `.rs` file is enumerated below** (504 of them — sources, integration tests, fixtures).");
  lines.push("`doc` is the file's own `//!` module doc, truncated at the first sentence. `public:` lists every `pub`");
  lines.push("item as `name:line`. `fns:` lists **every** `fn` — public *and* private — as `name:line`; that is the");
  lines.push("complete function accounting for the file. `refs` is the cross-file `stem::` reference count [D];");
  lines.push("`ambiguous` means the stem is too generic to measure (§11 caveats).");
  lines.push("");
  lines.push("### 9.0 Rust crate summary");
  lines.push("");
  lines.push("| Location | Files | Lines | Functions | `#[test]` fns |");
  lines.push("| --- | ---: | ---: | ---: | ---: |");
  const crates = [...new Set(RUST_FILES.map(groupOf))].sort();
  const summary = crates.map((crate) => {
    const files = RUST_FILES.filter((f) => groupOf(f) === crate);
    const acc = files.reduce(
      (a, f) => {
        const t = analyzeRust(f);
        return { lines: a.lines + t.lines, fns: a.fns + t.fns.length, tests: a.tests + t.tests };
      },
      { lines: 0, fns: 0, tests: 0 },
    );
    return { crate, files: files.length, ...acc };
  });
  for (const s of summary) {
    lines.push(`| \`${s.crate}/\` | ${s.files} | ${s.lines.toLocaleString()} | ${s.fns} | ${s.tests} |`);
  }
  const tot = summary.reduce(
    (a, s) => ({
      files: a.files + s.files,
      lines: a.lines + s.lines,
      fns: a.fns + s.fns,
      tests: a.tests + s.tests,
    }),
    { files: 0, lines: 0, fns: 0, tests: 0 },
  );
  lines.push(`| **TOTAL** | **${tot.files}** | **${tot.lines.toLocaleString()}** | **${tot.fns}** | **${tot.tests}** |`);
  lines.push("");

  crates.forEach((crate, i) => {
    const files = RUST_FILES.filter((f) => groupOf(f) === crate).sort();
    if (!files.length) return;
    lines.push(`### 9.${i + 1} \`${crate}/\``);
    lines.push("");
    for (const f of files) {
      const a = analyzeRust(f);
      const ref = refs.get(f);
      const refTxt = ref.ambiguous ? "ambiguous" : String(ref.refs);
      acct(f, "rust");
      lines.push(`#### \`${f}\` — ${a.lines.toLocaleString()} lines · ${a.tests} tests · refs ${refTxt}`);
      if (a.doc) {
        lines.push("");
        lines.push(`> ${a.doc}`);
        lines.push("");
      }
      if (a.publics.length) {
        const groups = {};
        for (const p of a.publics) (groups[p.kind] ??= []).push(p);
        lines.push(
          Object.keys(groups)
            .sort()
            .map(
              (kind) =>
                `**\`${kind}\`** (${groups[kind].length}): ` +
                groups[kind].map((p) => `\`${p.name}\`:${p.line}`).join(", "),
            )
            .join(" · "),
        );
      }
      if (a.fns.length) lines.push(`- \`fns\` (${a.fns.length}): ${fmtFns(a.fns)}`);
      lines.push("");
    }
  });
  return lines.join("\n");
}

function renderTsSection(wiring) {
  const lines = [];
  lines.push("## 10. TypeScript / TSX — per-area, per-file LLD");
  lines.push("");
  lines.push(`**Every tracked \`.ts\`/\`.tsx\` file is enumerated below** (${TS_FILES.length} of them). \`exports:\` lists every exported`);
  lines.push("declaration and re-export. `fns:` lists **every named function** — declarations, class and object-literal");
  lines.push("methods, constructors, get/set accessors, and function/arrow expressions bound to a name — as `name:line`,");
  lines.push(
    TS_MODE === "ast"
      ? "parsed with the TypeScript compiler. `anon:` counts the"
      : "parsed with a **line regex** — `typescript` did not resolve, so class/object methods,"
  );
  if (TS_MODE !== "ast") {
    lines.push("constructors, accessors and named fn expressions are UNDERCOUNTED here. `anon:` counts the");
  }
  lines.push("function/arrow expressions with no binding name (inline callbacks); `fns + anon` is the file's full function");
  lines.push("population. `wiring:` is the §11 verdict.");
  lines.push("");
  const areas = new Map();
  for (const f of TS_FILES) {
    const parts = f.split("/");
    const area = parts[0] === "ui" ? (parts[1] === "src" ? "ui/src" : "ui") : parts.slice(0, 2).join("/");
    if (!areas.has(area)) areas.set(area, []);
    areas.get(area).push(f);
  }
  const areaNames = [...areas.keys()].sort();
  lines.push("### 10.0 TS/TSX area summary");
  lines.push("");
  lines.push("| Area | Files | Lines | Fns | Anon | Exports | Tests | Prod-reachable |");
  lines.push("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
  for (const area of areaNames) {
    const files = areas.get(area);
    const st = files.reduce(
      (a, f) => {
        const t = analyzeTs(f);
        return {
          lines: a.lines + t.lines,
          fns: a.fns + t.fns.length,
          anon: a.anon + t.anon,
          exports: a.exports + t.exports.length,
          tests: a.tests + t.tests,
        };
      },
      { lines: 0, fns: 0, anon: 0, exports: 0, tests: 0 },
    );
    lines.push(
      `| \`${area}\` | ${files.length} | ${st.lines.toLocaleString()} | ${st.fns} | ${st.anon} | ${st.exports} | ${st.tests} | ${files.filter((f) => wiring.prod.has(f)).length} |`,
    );
  }
  lines.push("");
  areaNames.forEach((area, i) => {
    lines.push(`### 10.${i + 1} \`${area}\``);
    lines.push("");
    for (const f of areas.get(area).sort()) {
      const t = analyzeTs(f);
      const v = wiring.verdict(f);
      const imports = wiring.importedBy.get(f) ?? 0;
      acct(f, "ts");
      lines.push(`#### \`${f}\` — ${t.lines.toLocaleString()} lines · ${t.tests} tests · ${v} · imported by ${imports}`);
      if (t.doc) {
        lines.push(`> ${t.doc}`);
        lines.push("");
      }
      if (t.exports.length) lines.push(`- \`exports\` (${t.exports.length}): ${t.exports.join(", ")}`);
      if (t.fns.length) lines.push(`- \`fns\` (${t.fns.length}): ${fmtFns(t.fns)}`);
      if (t.anon) lines.push(`- \`anon\` (${t.anon}): function/arrow expressions with no binding name`);
      lines.push("");
    }
  });
  return lines.join("\n");
}

function renderWiringSection(refs, wiring) {
  const lines = [];
  lines.push("## 11. Wiring & status matrix [D]");
  lines.push("");
  lines.push("### 11.1 Caveats — read these before trusting any table below");
  lines.push("");
  lines.push("1. Reference counting proves **reachability by name**, not invocation on a live runtime path. A module");
  lines.push("   referenced only by its own crate's `pub use` re-export block can still be dead at runtime.");
  lines.push("2. Generic stems (`lib`, `types`, `config`, `state`, `error`, `helpers`, `prelude`, ...) match everywhere");
  lines.push("   and are reported as `ambiguous` rather than counted.");
  lines.push("3. Two modules with the same stem in different crates collapse into one reference bucket.");
  lines.push("4. TS reachability treats only statically visible imports (`from`, bare `import`, `require`) as edges.");
  lines.push("   Dynamic `import()` and string-keyed dispatch are missed.");
  lines.push("5. Test files and Rust `tests/` fixtures are included in these tables on purpose (they are files too),");
  lines.push("   which is why a mock server or an acceptance suite can appear as an \"orphan\".");
  lines.push("");
  lines.push("### 11.2 Rust files with zero cross-file references");
  lines.push("");
  const orphans = RUST_FILES.filter((f) => {
    const r = refs.get(f);
    return r && !r.ambiguous && r.refs === 0;
  });
  const isFixture = (f) =>
    /(^|\/)tests\//.test(f) || /(mock|probe|acceptance_|live_|p10_|p50_|_tests\.rs$|fixture)/.test(f);
  const srcOrphans = orphans.filter((f) => !isFixture(f));
  const fixtureOrphans = orphans.filter(isFixture);
  lines.push(
    `${orphans.length} of ${RUST_FILES.length} files have no ` + "`stem::`" + ` reference anywhere else.`,
  );
  lines.push(`**${srcOrphans.length} are non-test source files** (the real candidates); the other ${fixtureOrphans.length} are`);
  lines.push("integration tests, fixtures, and mock servers, which are never referenced by name.");
  lines.push("");
  const byCrate = new Map();
  for (const f of srcOrphans) {
    const c = f.startsWith("crates/") ? f.split("/").slice(0, 3).join("/") : "src-tauri";
    if (!byCrate.has(c)) byCrate.set(c, []);
    byCrate.get(c).push(basename(f, ".rs"));
  }
  lines.push("**Non-test source orphans, by directory:**");
  lines.push("");
  if (!byCrate.size) lines.push("_None._");
  for (const [c, fs] of [...byCrate].sort()) {
    lines.push(`- \`${c}\` (${fs.length}): ${fs.sort().join(", ")}`);
  }
  lines.push("");
  lines.push("<details><summary>All orphan candidates including tests/fixtures</summary>");
  lines.push("");
  const allByCrate = new Map();
  for (const f of orphans) {
    const c = f.startsWith("crates/") ? f.split("/").slice(0, 3).join("/") : "src-tauri";
    if (!allByCrate.has(c)) allByCrate.set(c, []);
    allByCrate.get(c).push(basename(f, ".rs"));
  }
  for (const [c, fs] of [...allByCrate].sort()) {
    lines.push(`- \`${c}\` (${fs.length}): ${fs.sort().join(", ")}`);
  }
  lines.push("");
  lines.push("</details>");
  lines.push("");
  lines.push("### 11.3 Rust files with the lowest non-zero reference counts");
  lines.push("");
  lines.push("The tail of this list is where \"referenced once, by its own re-export\" hides.");
  lines.push("");
  lines.push("| File | refs |");
  lines.push("| --- | ---: |");
  for (const r of RUST_FILES.map((f) => ({ f, ...refs.get(f) }))
    .filter((r) => !r.ambiguous && r.refs > 0)
    .sort((a, b) => a.refs - b.refs)
    .slice(0, 50)) {
    lines.push(`| \`${r.f}\` | ${r.refs} |`);
  }
  lines.push("");
  lines.push("### 11.4 TS/TSX files never imported by anything");
  lines.push("");
  const never = TS_FILES.filter((f) => wiring.verdict(f) === "NOT IMPORTED" && !isTestFile(f));
  lines.push(`${never.length} files (excluding test files).`);
  lines.push("");
  for (const f of never) lines.push(`- \`${f}\``);
  lines.push("");
  lines.push("### 11.5 TS/TSX files imported ONLY by tests (test-only modules)");
  lines.push("");
  const testOnly = TS_FILES.filter((f) => wiring.verdict(f) === "TEST-ONLY");
  lines.push(`${testOnly.length} files.`);
  lines.push("");
  for (const f of testOnly) lines.push(`- \`${f}\``);
  lines.push("");
  lines.push("### 11.6 Tauri commands with no static `invoke()` call site");
  lines.push("");
  const cmds = tauriCommands();
  const unreached = cmds.filter((c) => c.calls === 0);
  const indirect = unreached.filter((c) => c.refs > 0 || c.rust > 0);
  const cold = unreached.filter((c) => c.refs === 0 && c.rust === 0);
  lines.push(`${cmds.length} commands registered. ${cmds.length - unreached.length} have a direct \`invoke("name")\` site.`);
  lines.push(`Of the ${unreached.length} without one:`);
  lines.push("");
  lines.push(
    `- **${indirect.length} are only *indirectly* referenced** — the name appears as a string literal somewhere in the`,
  );
  lines.push("  UI/coordinator, or Rust mentions it. These are probably fine (the UI may pass a variable, as");
  lines.push("  `vault-gate.tsx` does with `invoke(cmd)`).");
  lines.push(`- **${cold.length} are cold** — the name appears nowhere outside its own definition and registration.`);
  lines.push("");
  lines.push("A high `Rust mentions` count on a generic word (`version`, `status`, `tasks`) is noise — the name");
  lines.push("matches unrelated identifiers. Counts above 50 are flagged `(noisy)` and carry no signal.");
  lines.push("");
  lines.push("| Command | Module family | literal refs | Rust mentions |");
  lines.push("| --- | --- | ---: | ---: |");
  for (const c of unreached.sort((a, b) => a.refs + a.rust - (b.refs + b.rust))) {
    const rust = c.rust > 50 ? `${c.rust} (noisy)` : `${c.rust}`;
    lines.push(`| \`${c.name}\` | \`${c.module}\` | ${c.refs} | ${rust} |`);
  }
  lines.push("");
  return lines.join("\n");
}

function renderDepsSection() {
  const lines = [];
  lines.push("## 12. Dependencies");
  lines.push("");
  lines.push("### 12.1 Rust — per-crate declarations");
  lines.push("");
  for (const c of cargoDeps()) {
    const external = c.deps.filter((d) => !d.startsWith("agentcowork-"));
    lines.push(`#### \`${c.crate}\``);
    lines.push("");
    lines.push(`- **external (${external.length}):** ${external.join(", ") || "_none_"}`);
    lines.push(`- **internal (${c.internal.length}):** ${c.internal.join(", ") || "_none_"}`);
    if (c.devDeps.length) lines.push(`- **dev-dependencies:** ${c.devDeps.join(", ")}`);
    lines.push("");
  }
  lines.push("### 12.2 npm — per manifest");
  lines.push("");
  for (const p of npmDeps()) {
    acct(p.path, "npm");
    lines.push(`#### \`${p.path}\` — \`${p.name}\``);
    lines.push("");
    if (p.scripts.length) lines.push(`- **scripts:** ${p.scripts.join(", ")}`);
    lines.push(`- **dependencies:** ${p.deps.join(", ") || "_none_"}`);
    lines.push(`- **devDependencies:** ${p.devDeps.join(", ") || "_none_"}`);
    lines.push("");
  }
  return lines.join("\n");
}

function renderAppendix(unaccounted) {
  const lines = [];
  lines.push("## 15. Appendices");
  lines.push("");
  lines.push("### 15.1 Tauri command registry (registered ↔ UI call sites)");
  lines.push("");
  const cmds = tauriCommands();
  lines.push(`**${cmds.length} commands** registered in the single \`generate_handler!\` in \`src-tauri/src/commands.rs\`.`);
  lines.push("");
  lines.push("| # | Command | Module family | direct `invoke()` sites | literal refs | Rust mentions |");
  lines.push("| ---: | --- | --- | ---: | ---: | ---: |");
  cmds.forEach((c, i) =>
    lines.push(
      `| ${i + 1} | \`${c.name}\` | \`${c.module}\` | ${c.calls} | ${c.refs} | ${c.rust > 50 ? `${c.rust} (noisy)` : c.rust} |`,
    ),
  );
  lines.push("");
  lines.push("### 15.2 Coordinator IPC method registry (`handleRequest`)");
  lines.push("");
  const methods = coordinatorMethods();
  lines.push(`${methods.length} methods:`);
  lines.push("");
  for (const m of methods) lines.push(`- \`${m}\``);
  lines.push("");
  lines.push("### 15.3 Full file index (every git-tracked path, grouped by directory)");
  lines.push("");
  const dirs = new Map();
  for (const f of FILES) {
    const d = dirname(f);
    if (!dirs.has(d)) dirs.set(d, []);
    dirs.get(d).push(basename(f));
  }
  for (const d of [...dirs.keys()].sort()) {
    lines.push(`- \`${d}/\` — ${dirs.get(d).sort().join(", ")}`);
  }
  lines.push("");
  lines.push("### 15.4 Census");
  lines.push("");
  const ext = new Map();
  for (const f of FILES) {
    const e = extname(f) || "(no extension)";
    ext.set(e, (ext.get(e) ?? 0) + 1);
  }
  lines.push("| Extension | Files |");
  lines.push("| --- | ---: |");
  for (const [e, n] of [...ext].sort((a, b) => b[1] - a[1])) lines.push(`| \`${e}\` | ${n} |`);
  lines.push(`| **TOTAL tracked** | **${FILES.length}** |`);
  lines.push("");
  // The map itself is excluded: its own line count changes on every write, so
  // including it would make `--check` fail forever (see renderDocsSection).
  const textFiles = FILES.filter((f) => isText(f) && f !== SELF_MAP);
  const totalLines = textFiles.reduce((a, f) => a + countLines(read(f)), 0);
  lines.push(
    `Lines counted across the ${textFiles.length} tracked text files at generation time: **${totalLines.toLocaleString()}** (this map excluded — self-referential).`,
  );
  lines.push("");
  lines.push("### 15.5 Coverage audit — is any tracked file unaccounted for?");
  lines.push("");
  lines.push("Each section registers the files it gives an entry to; this table is a diff against `git ls-files`.");
  lines.push("");
  const buckets = new Map();
  for (const b of ACCOUNTED.values()) buckets.set(b, (buckets.get(b) ?? 0) + 1);
  const bucketLabel = {
    rust: "§9 Rust — per-file `####`",
    ts: "§10 TS/TSX — per-file `####`",
    npm: "§12.2 npm manifests",
    assets: "§13 non-source inventory",
    docs: "§14 documentation index",
  };
  lines.push("| Section | Files accounted for |");
  lines.push("| --- | ---: |");
  for (const k of ["rust", "ts", "npm", "assets", "docs"]) {
    lines.push(`| ${bucketLabel[k]} | ${buckets.get(k) ?? 0} |`);
  }
  lines.push(`| **TOTAL** | **${ACCOUNTED.size} / ${FILES.length}** |`);
  lines.push("");
  if (!unaccounted.length) {
    lines.push(
      "**100% of tracked files have an entry, and that is mechanically enforced:** the generator exits non-zero if this",
    );
    lines.push("list is ever non-empty. Note what this does *not* claim — an entry is accounting, not explanation. §9/§10",
    );
    lines.push("describe structure (items, functions, wiring); §13/§14 describe purpose in one line.");
  } else {
    lines.push(`⚠️ **${unaccounted.length} tracked files have no entry — the generator is incomplete:**`);
    lines.push("");
    for (const f of unaccounted) lines.push(`- \`${f}\``);
  }
  lines.push("");
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Section 13 — non-source inventory
// ---------------------------------------------------------------------------

/** Coarse bucket per file. First match wins; every bucket is rendered. */
function assetGroup(f) {
  if (/(^|\/)Cargo\.toml$/.test(f)) return "cargo";
  if (/^\.github\/workflows\//.test(f) || f === ".pre-commit-config.yaml") return "ci";
  if (/^deploy\//.test(f)) return "deploy";
  if (/\.turbo\/[^/]*\.log$/.test(f) || /(^|\/)[0-9a-f]{8}-[0-9a-f-]{27}\.png$/i.test(f)) return "artifacts";
  if (/(^|\/)(Cargo\.lock|bun\.lock|package-lock\.json|pnpm-lock\.yaml)$/.test(f)) return "locks";
  if (
    /(^|\/)(tauri\.conf\.json|capabilities\.yaml|tsconfig\.json|postcss\.config\.js|globals\.css|index\.html|guard\.html)$/.test(
      f,
    ) ||
    /(^|\/)(capabilities|gen\/schemas)\//.test(f)
  )
    return "shell";
  if (/\.(mjs|js|py|sh)$/.test(f) || /^scripts\//.test(f)) return "scripts";
  return "other";
}

/** Body of a TOML `[section]`, up to the next top-level section. */
function tomlSection(t, name) {
  const i = t.indexOf(`\n[${name}]`);
  if (i < 0) return "";
  const j = t.indexOf("\n[", i + 1);
  return t.slice(i, j < 0 ? t.length : j);
}

/**
 * A file's stated purpose: the first prose line of its header comment.
 * Handles JSDoc comment blocks (marker on line 1, prose on line 2), skips
 * shebangs, and rejects Usage and dollar-prefixed invocation lines, which
 * describe how to call a script rather than what it does.
 */
function purposeOf(t) {
  const lines = t.split("\n");
  const i = lines[0]?.startsWith("#!") ? 1 : 0;
  const inlineDoc = /^\s*\/\*\*?\s*(.+?)\s*\*\/\s*$/.exec(lines[i] ?? "");
  if (inlineDoc) return inlineDoc[1].slice(0, 200);
  if (/^\s*\/\*/.test(lines[i] ?? "")) {
    for (let k = i + 1; k < Math.min(lines.length, i + 40); k += 1) {
      if (/\*\//.test(lines[k])) break;
      const m = /^\s*\*?\s*(\S.*?)\s*$/.exec(lines[k].replace(/\/\*\*?/, ""));
      const text = m?.[1];
      if (text && !/^\*+$/.test(text) && !/^(Usage|usage):/.test(text)) return text.slice(0, 200);
    }
  }
  const c = lines.find((l) => /^\s*(\/\/|#)(?!!)\s*\S/.test(l));
  if (!c) return "";
  const txt = c.replace(/^\s*(\/\/|#)\s*/, "").trim();
  if (/^(Usage|usage):|→|^\$ /.test(txt)) return "";
  return txt.slice(0, 200);
}

const firstComment = purposeOf;

const countMatches = (t, re) => (t.match(re) ?? []).length;

/**
 * Body of a top-level YAML key ("on", "jobs", ...), stopped at the next
 * column-0 key. Slicing by naming the next key instead would swallow the "env"
 * block sitting between "on" and "jobs" — which is exactly how CI config keys
 * came to be reported as workflow triggers in an earlier revision.
 */
function topBlock(t, key) {
  const lines = t.split("\n");
  const start = lines.findIndex((l) => new RegExp(`^${key}:`).test(l));
  if (start < 0) return "";
  const out = [lines[start]];
  for (let i = start + 1; i < lines.length; i += 1) {
    if (/^[A-Za-z_][\w-]*:/.test(lines[i])) break;
    out.push(lines[i]);
  }
  return out.join("\n");
}

/**
 * `[workspace.package]` from crates/Cargo.toml, so member manifests that say
 * `edition.workspace = true` can be reported with the value they actually
 * inherit instead of a bare dash (which reads as "absent" — it is not).
 */
let WS_PKG = null;
function wsPkg() {
  if (!WS_PKG) {
    const ws = FILES.includes("crates/Cargo.toml") ? tomlSection(read("crates/Cargo.toml"), "workspace.package") : "";
    WS_PKG = {
      version: (/^\s*version\s*=\s*"([^"]+)"/m.exec(ws) ?? [])[1],
      edition: (/^\s*edition\s*=\s*"([^"]+)"/m.exec(ws) ?? [])[1],
      rustVersion: (/^\s*rust-version\s*=\s*"([^"]+)"/m.exec(ws) ?? [])[1],
      license: (/^\s*license\s*=\s*"([^"]+)"/m.exec(ws) ?? [])[1],
    };
  }
  return WS_PKG;
}

/** `x = "v"` locally, or `x.workspace = true` resolved against the workspace. */
function inherit(t, key, wsValue) {
  const own = new RegExp(`^\\s*${key}\\s*=\\s*"([^"]+)"`, "m").exec(t);
  if (own) return own[1];
  if (new RegExp(`\\b${key}\\.workspace\\s*=\\s*true`).test(t)) return wsValue ? `${wsValue} (inherited)` : "workspace";
  return "—";
}

/** Facts for one non-source file. Never returns empty: bytes are the floor. */
function describeAsset(f) {
  const t = isText(f) ? read(f) : "";
  const b = [];
  const base = basename(f);
  const cm = (re) => countMatches(t, re);
  const g = (re) => (re.exec(t) ?? [])[1];

  if (/Cargo\.toml$/.test(f)) {
    if (/^\s*\[workspace\]/m.test(t)) {
      const members = (t.match(/^\s{4}"([^"]+)",?\s*$/gm) ?? []).map((s) => s.trim().replace(/[",]/g, ""));
      const wp = wsPkg();
      b.push(
        `workspace manifest — ${members.length} member crates · resolver \`${g(/^\s*resolver\s*=\s*"([^"]+)"/m) ?? "—"}\` · ${cm(/^\[patch\./gm)} patch sections`,
      );
      b.push(`members: \`${members.join("`, `")}\``);
      b.push(
        `\`[workspace.package]\` (what members inherit) — version \`${wp.version ?? "—"}\` · edition \`${wp.edition ?? "—"}\` · rust-version \`${wp.rustVersion ?? "—"}\` · license \`${wp.license ?? "—"}\``,
      );
      b.push(
        `\`[workspace.dependencies]\` rows: ${countMatches(tomlSection(t, "workspace.dependencies"), /^\s*[a-zA-Z0-9_-]+\s*=/gm)}`,
      );
    } else {
      const deps = tomlSection(t, "dependencies");
      const internal = [...new Set(deps.match(/agentcowork-[a-z]+/g) ?? [])];
      const name = g(/^\s*name\s*=\s*"([^"]+)"/m) ?? "?";
      b.push(
        `package \`${name}\` · edition \`${inherit(t, "edition", wsPkg().edition)}\` · rust-version \`${inherit(t, "rust-version", wsPkg().rustVersion)}\``,
      );
      const desc = g(/^\s*description\s*=\s*"([^"]{0,200})"/m);
      if (desc) b.push(`purpose: ${desc}`);
      b.push(
        `deps: ${countMatches(deps, /^\s*[a-zA-Z0-9_-]+\s*=/gm)} · internal: ${internal.join(", ") || "_none_"} · dev-deps: ${countMatches(tomlSection(t, "dev-dependencies"), /^\s*[a-zA-Z0-9_-]+\s*=/gm)} · features: ${/^\[features\]/m.test(t) ? "yes" : "no"} · bin targets: ${cm(/^\[\[bin\]\]/gm)}`,
      );
    }
  } else if (/^\.github\/workflows\//.test(f)) {
    const onBody = topBlock(t, "on");
    const triggers =
      [...new Set((onBody.match(/^ {2}([a-zA-Z_]+):/gm) ?? []).map((s) => s.trim().replace(":", "")))].join("`, `") ||
      (g(/^on:\s*\[([^\]]+)\]/m) ?? "push");
    const jobs = (topBlock(t, "jobs").match(/^ {2}([a-zA-Z0-9_-]+):\s*$/gm) ?? []).map((s) =>
      s.trim().replace(":", ""),
    );
    const runs = [...new Set((t.match(/runs-on:\s*([^\n]+)/g) ?? []).map((s) => s.split(":")[1].trim()))];
    // Any path token, not just `node <path>`: `bun run scripts/x`, `./scripts/x`,
    // and quoted paths all appear in these workflows, and a narrower pattern
    // silently reported wired scripts as unwired.
    const scripts = [
      ...new Set((t.match(/["'(\s]((?:scripts|ui)\/[\w./-]+\.(?:mjs|py|sh))/g) ?? []).map((s) => basename(s.slice(1)))),
    ];
    b.push(`**${g(/^name:\s*(.+)$/m) ?? base}** · triggers: \`${triggers}\` · jobs (${jobs.length}): \`${jobs.join("`, `")}\``);
    b.push(`steps: ${cm(/^\s*- (?:uses|run):/gm)} · runners: \`${runs.join("`, `")}\``);
    if (scripts.length) b.push(`invokes: ${[...new Set(scripts)].map((s) => `\`${s}\``).join(", ")}`);
  } else if (/^\.pre-commit-config\.yaml$/.test(f)) {
    // [ \t] rather than \s: \s also matches a newline, so the match could start
    // on the previous line and capture a hook name with a literal newline in it.
    const hooks = (t.match(/^[ \t]*- id:[ \t]*(\S+)/gm) ?? []).map((s) =>
      s.replace(/^[ \t]*- id:[ \t]*/, ""),
    );
    b.push(`hooks (${hooks.length}): \`${hooks.join("`, `")}\``);
    b.push(`first comment: ${firstComment(t)}`);
  } else if (/^deploy\/Dockerfile$/.test(f)) {
    const stages = [...t.matchAll(/^FROM\s+(\S+)(?:\s+AS\s+(\S+))?/gm)].map((m) => `${m[1]}${m[2] ? ` (${m[2]})` : ""}`);
    b.push(`${stages.length} build stages: ${stages.map((s) => `\`${s}\``).join(" → ")}`);
    b.push(`\`RUN\` steps: ${cm(/^RUN /gm)} · base image is a slim Debian runtime; the coordinator is Bun-compiled, the core is \`cargo build --release -p agentcowork-core\``);
    b.push(`runtime user/permissions hardening: ${/^USER /m.test(t) ? "explicit \`USER\`" : "none — container runs as root unless overridden"}`);
  } else if (/^deploy\/docker-compose\.yml$/.test(f)) {
    const svcIdx = t.indexOf("\nservices:");
    const svcs = svcIdx < 0 ? [] : (t.slice(svcIdx).match(/^ {2}([a-zA-Z0-9_-]+):/gm) ?? []).map((s) => s.trim().replace(":", ""));
    b.push(`services (${svcs.length}): \`${svcs.join("`, `")}\` · published ports: ${cm(/^\s+- "\d+:\d+\/tcp"/gm)}`);
    b.push(`vault key is fail-fast (\`\${AGENTCOWORK_VAULT_KEY:?…}\`), so the stack refuses to start without it — never hardcoded`);
  } else if (/^deploy\/fly\.toml$/.test(f)) {
    b.push(`app \`${g(/^app\s*=\s*"([^"]+)"/m) ?? "?"}\` · internal_port \`${g(/internal_port\s*=\s*(\d+)/m) ?? "?"}\` · health check \`${g(/^\s*type\s*=\s*"([^"]+)"/m) ?? "?"}\``);
  } else if (/\.service$/.test(f)) {
    b.push(`systemd unit \`${base}\` · \`ExecStart=${g(/^ExecStart=(.+)$/m) ?? "?"}\``);
    b.push(`User \`${g(/^User=(.+)$/m) ?? "(root)"}\` · restart \`${g(/^Restart=(.+)$/m) ?? "—"}\` · EnvironmentFile \`${g(/^EnvironmentFile=-?(.+)$/m) ?? "none"}\``);
  } else if (/\.plist$/.test(f)) {
    const args = [...t.matchAll(/<string>(--?[a-z-]+|\/[\w./-]+)<\/string>/g)].map((m) => m[1]);
    b.push(`launchd agent \`${g(/<key>Label<\/key>\s*<string>([^<]+)<\/string>/m) ?? "?"}\` · args: \`${args.join(" ")}\``);
    b.push(`RunAtLoad \`${/RunAtLoad/.test(t) ? "yes" : "no"}\` · KeepAlive \`${/KeepAlive/.test(t) ? "yes" : "no"}\``);
  } else if (/tsconfig\.json$/.test(f)) {
    const co = (/"compilerOptions"\s*:\s*\{([\s\S]*?)\n\s{2,4}\}/.exec(t) ?? [])[1] ?? "";
    const opts = (co.match(/^\s*"([^"]+)"\s*:/gm) ?? []).map((s) => s.trim().replace(/[":]/g, ""));
    b.push(`compilerOptions (${opts.length}): \`${opts.slice(0, 10).join("`, `")}\`${opts.length > 10 ? " …" : ""}`);
    b.push(`include \`${(/"include"\s*:\s*\[([^\]]*)\]/.exec(t) ?? [])[1]?.trim() || "—"}\` · references: ${cm(/"path"\s*:/gm)}`);
  } else if (/tauri\.conf\.json$/.test(f)) {
    b.push(`product \`${g(/"productName"\s*:\s*"([^"]+)"/) ?? "?"}\` v${g(/"version"\s*:\s*"([^"]+)"/) ?? "?"} · identifier \`${g(/"identifier"\s*:\s*"([^"]+)"/) ?? "?"}\``);
    b.push(`windows: ${cm(/"title"\s*:/g)} · bundle targets: ${(/"targets"\s*:\s*"([^"]+)"/.exec(t) ?? [])[1] ?? "—"} · CSP set: ${/"csp"/.test(t) ? "yes" : "no"}`);
  } else if (/(^|\/)capabilities\/[^/]+\.json$/.test(f)) {
    b.push(`Tauri v2 capability window · permission entries: ${cm(/"[a-z][a-z0-9:-]+"/gm)}`);
    b.push(`files/\`$schema\` declared: ${cm(/\$schema/gm)} · platforms: ${[...new Set(t.match(/"(linux|macOS|windows)"/g) ?? [])].join(", ") || "all"}`);
  } else if (/capabilities\.yaml$/.test(f)) {
    b.push(`capability ledger (root) — top-level sections: ${cm(/^[a-z][a-zA-Z_-]*:/gm)} · total lines ${countLines(t)}`);
    b.push(`§7 item 3: this file is the canonical example of doc-vs-code capability drift — treat its claims as [C]`);
  } else if (/gen\/schemas\/[^/]+\.json$/.test(f)) {
    b.push(`**generated by Tauri CLI** (checked in, not hand-written) — top-level keys: ${cm(/^ {2}"[^"]+":/gm)}`);
  } else if (/index\.html$/.test(f)) {
    b.push(`Vite entry HTML · title \`${g(/<title>([^<]*)<\/title>/) ?? "?"}\` · scripts: ${cm(/<script/gm)} · root div \`${g(/id="([^"]+)"/) ?? "?"}\``);
  } else if (/guard\.html$/.test(f)) {
    b.push(`standalone Guard approval webview page (no framework) — inline JS blocks: ${cm(/<script/gm)} · nonce/CSP mentions: ${cm(/nonce|Content-Security-Policy/gi)}`);
  } else if (/globals\.css$/.test(f)) {
    b.push(`Tailwind v4 entry — \`@import\`: ${cm(/@import/gm)} · \`@theme\` blocks: ${cm(/@theme/gm)} · custom properties: ${cm(/^\s*--[\w-]+:/gm)} · selectors: ${cm(/^[.#][\w-]+/gm)}`);
  } else if (/postcss\.config\.js$/.test(f)) {
    b.push(`PostCSS plugins: ${[...new Set(t.match(/require\(['"]([^'"]+)|from ['"]([^'"]+)/g) ?? [])].join(", ") || g(/plugins[\s\S]{0,120}/) || "(see file)"}`);
  } else if (/Cargo\.lock$/.test(f)) {
    b.push(`Cargo resolution — \`[[package]]\` entries: ${cm(/^\[\[package\]\]/gm)} · lines ${countLines(t)}`);
  } else if (/package-lock\.json$/.test(f)) {
    let n = 0;
    try {
      n = Object.keys(JSON.parse(t).packages ?? {}).length;
    } catch {
      n = cm(/"resolved"\s*:/g);
    }
    b.push(`npm resolution — ${n} resolved package entries`);
  } else if (/pnpm-lock\.yaml$/.test(f)) {
    b.push(`pnpm resolution — lockfileVersion \`${g(/^lockfileVersion:\s*'?([\w.]+)'?/m) ?? "?"}\` · package rows: ${cm(/^\s{2,4}['"]?[\w@][\w@/.-]*['"]?:/gm)}`);
  } else if (/bun\.lock$/.test(f)) {
    b.push(`bun text-format lockfile — \`"resolved"\` rows: ${cm(/"resolved"/g)} · lines ${countLines(t)}`);
  } else if (/\.turbo\/[^/]*\.log$/.test(f)) {
    const errored = /(^|\n)\s*(error|Error:|ERROR|FAIL|✖)/.test(t);
    b.push(`**committed build artifact** — turbo's per-package task log; contains ${errored ? "error text" : "no error text"} · lines ${countLines(t)}`);
  } else if (/\.(mjs|js|sh)$/.test(f) || (/\.py$/.test(f) && !/fixtures\//.test(f))) {
    const role = firstComment(t) || `(${base})`;
    b.push(`purpose: ${role}`);
    b.push(scriptWiring(f));
  } else if (/^LICENSE/.test(base)) {
    b.push(`license text — \`${(t.split("\n")[0] ?? "").trim().slice(0, 120)}\``);
  } else if (/\.gitignore$/.test(f)) {
    b.push(`ignore rules: ${cm(/^[^#\s][^\n]*$/gm)} · negations: ${cm(/^!/gm)}`);
  } else if (/\.(png|ico|icns)$/.test(f)) {
    const refs = textBlob().split(base).length - 1;
    b.push(
      refs
        ? `binary image asset — ${humanBytes(bytesOf(f))} · named by ${refs} tracked text file${refs === 1 ? "" : "s"}`
        : `binary image asset — ${humanBytes(bytesOf(f))} · **named nowhere in the tree** (no config, doc, or code path refers to it)`,
    );
  } else if (/\.py$/.test(f)) {
    b.push(`test fixture — ${firstComment(t)}`);
  } else if (/\.json$/.test(f)) {
    let keys = 0;
    try {
      const j = JSON.parse(t);
      keys = Array.isArray(j) ? j.length : Object.keys(j).length;
    } catch {
      keys = cm(/^\s*"/gm);
    }
    b.push(`data file — ${Array.isArray((() => { try { return JSON.parse(t); } catch { return null; } })()) ? "array of " + keys + " entries" : keys + " top-level keys"}`);
  } else {
    b.push(`\`${base}\` — ${humanBytes(bytesOf(f))}${t ? `, ${countLines(t)} lines` : ""}`);
  }
  return b;
}

/**
 * Is a script actually invoked? CI yaml, package.json scripts, and sibling
 * scripts are searched for the basename. This is the [D]-grade wiring verdict
 * for tooling, the same question §11 asks of modules.
 */
/**
 * Concatenated text of every tracked text file, for `named by N files` checks.
 * This map is excluded: it enumerates every path, so counting it would make
 * "named by 1 file" the answer for every file in the repo — the metric would
 * always be >= 1 and therefore carry no signal at all.
 */
let TEXT_BLOB = null;
const textBlob = () =>
  (TEXT_BLOB ??= FILES.filter((f) => isText(f) && f !== "CODEBASE-MAP.md")
    .map(read)
    .join("\n"));

let SCRIPT_WIRING_BLOBS = null;
function scriptWiring(f) {
  if (!SCRIPT_WIRING_BLOBS) {
    SCRIPT_WIRING_BLOBS = {
      ci: FILES.filter((x) => /^\.github\/workflows\//.test(x)).map(read).join("\n"),
      pkg: FILES.filter((x) => x.endsWith("package.json")).map(read).join("\n"),
      scripts: FILES.filter((x) => /\.(mjs|js|py|sh)$/.test(x)).map(read).join("\n"),
    };
  }
  const base = basename(f);
  const ci = SCRIPT_WIRING_BLOBS.ci.split(base).length - 1;
  const pkg = SCRIPT_WIRING_BLOBS.pkg.split(base).length - 1;
  const sib = Math.max(0, (SCRIPT_WIRING_BLOBS.scripts.split(base).length - 1) - (read(f).split(base).length - 1));
  const bits = [];
  if (ci) bits.push(`CI ×${ci}`);
  if (pkg) bits.push(`package.json ×${pkg}`);
  if (sib) bits.push(`other scripts ×${sib}`);
  return bits.length ? `wired: ${bits.join(" · ")}` : "**no reference found** — not named by any workflow, package script, or sibling script";
}

function renderAssetSection() {
  const groups = new Map();
  for (const f of FILES) {
    if (ACCOUNTED.has(f) || f.endsWith(".md")) continue;
    const g = assetGroup(f);
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(f);
  }
  const spec = [
    ["cargo", "13.1 Rust manifests", "Every `Cargo.toml`. Dependency lists are expanded in §12.1; this is each manifest's own facts, so no manifest file is without an entry."],
    ["ci", "13.2 CI, hooks & gates", "The GitHub workflows and the pre-commit hook config, with the scripts each one invokes."],
    ["deploy", "13.3 The deployment plane (`deploy/`)", "Narrated in **§1.9** — the BYO-host pack that runs the same core binary headless on hardware the user owns. Five files here; the sixth, `BYO-HOST.md`, is a document and is indexed in §14."],
    ["shell", "13.4 App & build config", "Tauri shell config, capability manifests, generated ACL schemas, TypeScript configs, and the HTML/CSS entry points."],
    ["locks", "13.5 Lockfiles", "Committed dependency resolutions."],
    ["artifacts", "13.6 Committed build artifacts ⚠", "**These are build output that git is tracking.** Turbo writes one `.turbo/*.log` per package per task and 37 are tracked, with no `.turbo` rule in `.gitignore`; the root PNG here is *deliberately* un-ignored by `.gitignore:38` yet named by nothing else in the repo, and the sibling rule beside it points at a file that no longer exists. Recorded rather than silently skipped — evidence in §7 items 11–12."],
    ["scripts", "13.7 Scripts & tooling", "The CI, e2e, and perf tooling, each with a wiring verdict: is this script named by a workflow, a `package.json` script, or another script?"],
    ["other", "13.8 Binary & misc assets", "Icons, licences, ignore rules, and fixture data."],
    ["unclassified", "13.9 Unclassified", "Rendered only if a file matched no rule above, so a new file type can never be dropped silently."],
  ];
  const lines = [];
  lines.push("## 13. Non-source inventory — every remaining tracked file");
  lines.push("");
  const total = [...groups.values()].reduce((a, v) => a + v.length, 0);
  lines.push(
    `The ${total} tracked files that are not Rust, TypeScript, or an npm manifest. Every one has a \`####\``,
  );
  lines.push("entry below — nothing is summarised away at this level. Line counts are omitted for binaries.");
  lines.push(
    "The `wired:` verdicts in §13.7 are name searches over CI YAML, `package.json` scripts, and sibling scripts — the",
  );
  lines.push("same heuristic class as §11, so a script invoked through a variable or a wrapper reads as unwired.");
  lines.push("");
  lines.push("| Group | Files |");
  lines.push("| --- | ---: |");
  for (const [key, title] of spec) {
    const fs = groups.get(key) ?? [];
    if (fs.length) lines.push(`| §${title.split(" ")[0]} | ${fs.length} |`);
  }
  lines.push(`| **TOTAL** | **${total}** |`);
  lines.push("");
  for (const [key, title, blurb] of spec) {
    const fs = (groups.get(key) ?? []).sort();
    lines.push(`### ${title}`);
    lines.push("");
    if (blurb) lines.push(blurb.startsWith("*") ? blurb : `*${blurb}*`);
    lines.push("");
    if (!fs.length) {
      lines.push("_None._");
      lines.push("");
      continue;
    }
    for (const f of fs) {
      acct(f, "assets");
      lines.push(
        `#### \`${f}\` — ${humanBytes(bytesOf(f))}${
          isText(f) ? ` · ${countLines(read(f)).toLocaleString()} lines` : " · binary"
        }`,
      );
      for (const bullet of describeAsset(f)) lines.push(`- ${bullet}`);
      lines.push("");
    }
  }
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Section 14 — documentation index
// ---------------------------------------------------------------------------

function renderDocsSection() {
  const docs = FILES.filter((f) => f.endsWith(".md"));
  // Self-reference guard: this map documents itself, and its own byte/line
  // counts change each time it is written — so the census and the per-file
  // header would differ from the previous run forever and the `--check` gate
  // (run in CI) could never pass. The entry stays (coverage is complete) but
  // its numbers are omitted and excluded from the aggregate rows.
  const statsDocs = docs.filter((f) => f !== SELF_MAP);
  const familyOf = (f) => {
    if (f.startsWith("ARCH/")) return "ARCH — the design set (+ DIAGRAMS & ADR)";
    if (f.startsWith("RESEARCH/desktop_app/")) return "RESEARCH/desktop_app — the prior-art & competitor corpus";
    if (f.startsWith("RESEARCH/")) return `RESEARCH/${f.split("/")[1]} — other research`;
    if (f.startsWith("ui/")) return "ui/ — UI design docs";
    if (f.startsWith("deploy/")) return "deploy/ — deployment docs";
    if (f.includes("/")) return `${dirname(f)}/`;
    return "root — specs, handover, and this map";
  };
  const fams = new Map();
  for (const f of docs) {
    const fam = familyOf(f);
    if (!fams.has(fam)) fams.set(fam, []);
    fams.get(fam).push(f);
  }
  const lines = [];
  lines.push("## 14. Documentation index — every tracked `.md`");
  lines.push("");
  lines.push(
    `**All ${docs.length} Markdown files** carry an entry: title, size, and opening sentence. This closes the gap where the`,
  );
  lines.push(
    "first draft said the corpus was \"listed with their headings\" but was in fact only listed by name. These are the repo's",
  );
  lines.push(
    "claims *about itself*; `scripts/check-doc-sync.mjs` gates index/count drift in them, not capability drift — tag them [C] per §0.",
  );
  lines.push("");
  lines.push("### 14.0 Documentation census");
  lines.push("");
  lines.push("| Family | Files | Lines |");
  lines.push("| --- | ---: | ---: |");
  const famNames = [...fams.keys()].sort();
  for (const fam of famNames) {
    const fs = fams.get(fam).filter((f) => f !== SELF_MAP);
    if (fs.length === 0) {
      // The self entry's family — counted as 0 lines so the totals stay stable.
      lines.push(`| ${fam} | ${fams.get(fam).length} | — |`);
      continue;
    }
    lines.push(`| ${fam} | ${fs.length} | ${fs.reduce((a, f) => a + countLines(read(f)), 0).toLocaleString()} |`);
  }
  lines.push(
    `| **TOTAL** | **${docs.length}** | **${statsDocs.reduce((a, f) => a + countLines(read(f)), 0).toLocaleString()}** |`,
  );
  lines.push("");
  famNames.forEach((fam, i) => {
    lines.push(`### 14.${i + 1} ${fam}`);
    lines.push("");
    for (const f of fams.get(fam).sort()) {
      acct(f, "docs");
      if (f === SELF_MAP) {
        lines.push(
          `#### \`${f}\` — the map itself (size and line count omitted: self-referential, they change on every write)`,
        );
        lines.push(`> ${mdTitle(f)}`);
        lines.push("");
        continue;
      }
      lines.push(`#### \`${f}\` — ${countLines(read(f)).toLocaleString()} lines · ${humanBytes(bytesOf(f))}`);
      lines.push(`> ${mdTitle(f)}`);
      const lead = mdLead(f);
      if (lead && lead !== mdTitle(f)) lines.push("");
      if (lead && lead !== mdTitle(f)) lines.push(`- opening: ${lead}`);
      lines.push("");
    }
  });
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function main() {
  const refs = rustRefCounts();
  const wiring = tsReachability();
  // Order matters: each renderer registers what it covers in ACCOUNTED, and
  // §13 claims only what §9/§10/§12 have not already claimed.
  const rustSection = renderRustSection(refs);
  const tsSection = renderTsSection(wiring);
  const wiringSection = renderWiringSection(refs, wiring);
  const depsSection = renderDepsSection();
  const assetSection = renderAssetSection();
  const docsSection = renderDocsSection();
  const unaccounted = FILES.filter((f) => !ACCOUNTED.has(f));
  const appendixSection = renderAppendix(unaccounted);
  const generated = [
    rustSection,
    tsSection,
    wiringSection,
    depsSection,
    assetSection,
    docsSection,
    appendixSection,
  ].join("\n\n---\n\n");

  if (!existsSync(MAP_PATH)) {
    console.error(`CODEBASE-MAP.md not found at ${MAP_PATH}`);
    process.exit(1);
  }
  const doc = readFileSync(MAP_PATH, "utf8");
  const start = doc.indexOf(BEGIN);
  const end = doc.indexOf(END);
  if (start < 0 || end < 0 || end < start) {
    console.error("markers not found in CODEBASE-MAP.md");
    process.exit(1);
  }
  const next = doc.slice(0, start + BEGIN.length) + "\n\n" + generated + "\n" + doc.slice(end);

  if (CHECK) {
    const same = next === doc;
    console.log(same ? "CODEBASE-MAP.md inventory is up to date" : "CODEBASE-MAP.md inventory is STALE");
    process.exit(same ? 0 : 1);
  }
  writeFileSync(MAP_PATH, next);
  console.log(
    `wrote CODEBASE-MAP.md — ${FILES.length} tracked files, ${RUST_FILES.length} rust, ` +
      `${TS_FILES.length} ts/tsx, ${tauriCommands().length} tauri commands, ${next.split("\n").length} lines`,
  );
  console.log(`coverage: ${ACCOUNTED.size}/${FILES.length} tracked files have an entry`);
  if (unaccounted.length) {
    console.error(`\nUNACCOUNTED FILES (${unaccounted.length}) — add a bucket in §13 or §14 for these:`);
    for (const f of unaccounted) console.error(`  ${f}`);
    process.exit(1);
  }
}

main();
