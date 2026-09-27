#!/usr/bin/env node
// AgentCowork version-lockstep gate (P70.A7) — run in CI and pre-commit.
//
// **One authoritative application version, consumed by every surface that shows
// or publishes it.** The authority is `src-tauri/tauri.conf.json`'s `version`,
// because that is the string Tauri writes into the installer metadata *and* into
// the updater manifest — so making it the source is exactly what stops the
// shell-chrome badge from disagreeing with what was actually installed.
//
// Why this exists: the About badge was injected from `ui/package.json`, which
// read `2.0.0` while the installer/updater said `0.1.0` — two surfaces stating
// different versions of the same product. This is the same hand-kept lockstep
// the repo already uses for the capability index (`check-doc-sync.mjs` fails the
// build until `capabilities.yaml` == `ARCH/09` == spec §0). The entry that
// drifts is the reminder; there is no generator to hide it.
//
// Required to agree with the authority (a mismatch fails the build):
//   src-tauri/Cargo.toml        — the binary the installer packages
//   crates/Cargo.toml           — `[workspace.package] version`
//   package.json                — the workspace manifest
//   packages/coordinator/…      — its `package.json` + the `VERSION` it advertises
//                                 to the shell in the IPC handshake (`serverVersion`)
//   ui/vite.config.ts           — must inject the authority, never `ui/package.json`
//
// Reported but **not** fatal: sibling workspace packages. A package may
// legitimately carry its own version; it is simply not an application-version
// surface. (Not covered here: `ui/src/lib/version.ts`'s `ARCH_VERSION`, which is
// the *architecture/spec* version and is owned by `check-doc-sync.mjs`.)
//
// Usage: node scripts/check-versions.mjs

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(ROOT, p), "utf8");

const AUTHORITY = "src-tauri/tauri.conf.json";

const failures = [];
const advisories = [];

/** `version` from a JSON file, or `null` when absent/unreadable. */
function jsonVersion(path) {
  if (!existsSync(join(ROOT, path))) {
    failures.push(`${path} is missing — it is a required version consumer.`);
    return null;
  }
  try {
    return JSON.parse(read(path)).version ?? null;
  } catch (err) {
    failures.push(`${path} is not valid JSON: ${err.message}`);
    return null;
  }
}

/**
 * `version` from a Cargo manifest. When `section` is given, only that section
 * counts — a `[workspace.package]` version is a different declaration from a
 * `[package]` one and must not be confused with a dependency's `version = "1"`.
 */
function cargoVersion(path, section) {
  if (!existsSync(join(ROOT, path))) {
    failures.push(`${path} is missing — it is a required version consumer.`);
    return null;
  }
  let current = "";
  for (const raw of read(path).split("\n")) {
    const line = raw.replace(/#.*$/, "").trimEnd();
    const sec = /^\[([^\]]+)\]$/.exec(line.trim());
    if (sec) {
      current = sec[1];
      continue;
    }
    if (section && current !== section) continue;
    const m = /^version\s*=\s*"([^"]+)"/.exec(line.trim());
    if (m) return m[1];
  }
  failures.push(
    `${path} declares no version${section ? ` under [${section}]` : ""} — it is a required version consumer.`,
  );
  return null;
}

function require_agrees(label, found, authority) {
  if (found === null) return;
  if (found !== authority) {
    failures.push(`${label} declares ${found} but the authority (${AUTHORITY}) is ${authority}.`);
  }
}

const authority = jsonVersion(AUTHORITY);

if (authority) {
  // --- the app, the kernel, the workspace ---------------------------------
  require_agrees("src-tauri/Cargo.toml [package]", cargoVersion("src-tauri/Cargo.toml", "package"), authority);
  require_agrees(
    "crates/Cargo.toml [workspace.package]",
    cargoVersion("crates/Cargo.toml", "workspace.package"),
    authority,
  );
  require_agrees("package.json", jsonVersion("package.json"), authority);

  // --- the sidecar advertises its version to the shell ---------------------
  require_agrees(
    "packages/coordinator/package.json",
    jsonVersion("packages/coordinator/package.json"),
    authority,
  );
  const handshakePath = "packages/coordinator/src/index.ts";
  if (existsSync(join(ROOT, handshakePath))) {
    const m = /export const VERSION\s*=\s*"([^"]+)"/.exec(read(handshakePath));
    if (!m) {
      failures.push(
        `${handshakePath} no longer declares \`export const VERSION = "…"\` — the handshake's serverVersion has no source.`,
      );
    } else if (m[1] !== authority) {
      failures.push(
        `${handshakePath} advertises VERSION ${m[1]} but the authority (${AUTHORITY}) is ${authority} — the shell would see a sidecar version that does not exist.`,
      );
    }
  } else {
    failures.push(`${handshakePath} is missing — the sidecar handshake cannot be checked.`);
  }

  // --- the badge must read the authority, not a package-local version ------
  const vitePath = "ui/vite.config.ts";
  if (!existsSync(join(ROOT, vitePath))) {
    failures.push(`${vitePath} is missing — the shell-chrome badge has no version source.`);
  } else {
    const vite = read(vitePath);
    if (!vite.includes("tauri.conf.json")) {
      failures.push(
        `${vitePath} does not read ${AUTHORITY} — the About badge must be injected from the authority, not a package-local version.`,
      );
    }
    if (/from\s+"\.\/package\.json"/.test(vite)) {
      failures.push(
        `${vitePath} still injects \`ui/package.json\`'s version — that is the surface that drifted (2.0.0 vs 0.1.0); inject ${AUTHORITY} instead.`,
      );
    }
  }

  // --- advisory: sibling workspace packages --------------------------------
  const packagesDir = join(ROOT, "packages");
  if (existsSync(packagesDir)) {
    for (const entry of readdirSync(packagesDir, { withFileTypes: true })) {
      if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
      if (entry.name === "coordinator") continue; // required above
      const rel = `packages/${entry.name}/package.json`;
      const v = jsonVersion(rel);
      if (v && v !== authority) advisories.push(`${rel} is ${v}`);
    }
  }
}

if (failures.length) {
  console.error("❌ version-lockstep check FAILED:");
  for (const f of failures) console.error(`   - ${f}`);
  console.error(`\n   Authority: ${AUTHORITY} \`version\` — bump it first, then the consumers.`);
  process.exit(1);
}

console.log(
  `✅ version-lockstep: ${authority} (authority ${AUTHORITY}) — app, kernel, workspace, sidecar handshake` +
    ` and the shell-chrome badge all agree` +
    (advisories.length ? `; ${advisories.length} sibling package(s) carry their own version: ${advisories.join(", ")}` : ""),
);
