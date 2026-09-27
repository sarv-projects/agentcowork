import { defineConfig, normalizePath, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath, URL } from "node:url";

// Tauri expects a fixed frontend port for `devUrl` (see src-tauri/tauri.conf.json).
const host = process.env.TAURI_DEV_HOST;

// P58.2 — inject the application version at build time so the About stamp can
// never rot. A plain literal in settings-sections-extra.tsx drifted (claimed
// v0.7.2 · build 2026.01.15 while package.json says 2.0.0).
//
// P70.A7 — the source of that version is `src-tauri/tauri.conf.json`, **not**
// `ui/package.json`. That file's `version` is what Tauri writes into the
// installer metadata and the updater manifest, so injecting it here is what
// stops the badge from disagreeing with what was actually installed (it read
//
//   2.0.0 (ui/package.json)  vs  0.1.0 (installer/updater)
//
// — two surfaces stating different versions of the same product).
// `scripts/check-versions.mjs` fails the build if any consumer drifts.
import tauriConf from "../src-tauri/tauri.conf.json" with { type: "json" };

const require = createRequire(import.meta.url);
const monacoDomSanitizePath = require.resolve(
  "monaco-editor/base/browser/domSanitize.js",
);
const monacoVendoredDompurifyPath = require.resolve(
  "monaco-editor/base/browser/dompurify/dompurify.js",
);
const monacoDompurifyImport = "./dompurify/dompurify.js";
const dompurifyEntryPath = require.resolve("dompurify", {
  paths: [dirname(monacoDomSanitizePath)],
});
const dompurifyDistPath = dirname(dompurifyEntryPath);
const dompurifyPackagePath = join(dompurifyDistPath, "..", "package.json");
const dompurifyEsModulePath = join(dompurifyDistPath, "purify.es.mjs");
const monacoDompurifyReplacementId = "\0agentcowork:monaco-dompurify";

function readRequiredFile(path: string, label: string): string {
  if (!existsSync(path)) {
    throw new Error(`${label} is missing: ${path}`);
  }
  return readFileSync(path, "utf8");
}

const monacoDomSanitizeSource = readRequiredFile(
  monacoDomSanitizePath,
  "Monaco DOMSanitize module",
);
const monacoVendoredDompurifySource = readRequiredFile(
  monacoVendoredDompurifyPath,
  "Monaco vendored DOMPurify module",
);
const monacoDompurifyImportPattern =
  /\bfrom\s+['"]\.\/dompurify\/dompurify\.js['"]/;
if (!monacoDompurifyImportPattern.test(monacoDomSanitizeSource)) {
  throw new Error(
    "Monaco no longer imports ./dompurify/dompurify.js; the DOMPurify replacement guard must be updated.",
  );
}
if (!/DOMPurify\.version\s*=\s*['"]\d+\.\d+\.\d+['"]/.test(monacoVendoredDompurifySource)) {
  throw new Error(
    "Monaco's vendored DOMPurify module is not recognisable; refusing to build without an audited replacement.",
  );
}

const dompurifyPackage = JSON.parse(
  readRequiredFile(dompurifyPackagePath, "DOMPurify package manifest"),
) as { version?: unknown };
if (dompurifyPackage.version !== "3.4.16") {
  throw new Error(
    `DOMPurify 3.4.16 is required for Monaco replacement; found ${String(dompurifyPackage.version)}.`,
  );
}

const monacoDompurifyReplacementSource = readRequiredFile(
  dompurifyEsModulePath,
  "isolated DOMPurify replacement",
);
if (!/DOMPurify\.version\s*=\s*['"]3\.4\.16['"]/.test(monacoDompurifyReplacementSource)) {
  throw new Error(
    "The isolated DOMPurify replacement is not version 3.4.16; refusing to build.",
  );
}

function isMonacoDomSanitizeImporter(importer: string | undefined): boolean {
  if (!importer) return false;
  const normalizedImporter = normalizePath(importer).split("?", 1)[0];
  return (
    normalizedImporter === normalizePath(monacoDomSanitizePath) ||
    normalizedImporter.endsWith(
      "/monaco-editor/esm/vs/base/browser/domSanitize.js",
    )
  );
}

function monacoDompurifyReplacement(): Plugin {
  let expectedImportSeen = false;

  return {
    name: "agentcowork-monaco-dompurify-replacement",
    enforce: "pre",
    buildStart() {
      expectedImportSeen = false;
    },
    resolveId(source, importer) {
      const normalizedSource = source.split("?", 1)[0];
      if (
        normalizedSource === monacoDompurifyImport &&
        isMonacoDomSanitizeImporter(importer)
      ) {
        expectedImportSeen = true;
        // A virtual module keeps Monaco's sanitizer instance separate from
        // Mermaid's DOMPurify import while still using the audited package.
        return monacoDompurifyReplacementId;
      }
      return null;
    },
    load(id) {
      if (id === monacoDompurifyReplacementId) {
        return monacoDompurifyReplacementSource;
      }
      return null;
    },
    buildEnd() {
      if (!expectedImportSeen) {
        throw new Error(
          "Monaco DOMPurify replacement did not run: expected ./dompurify/dompurify.js import was not resolved.",
        );
      }
    },
  };
}

export default defineConfig({
  plugins: [monacoDompurifyReplacement(), react()],
  // P58.2/P70.A7 — __APP_VERSION__ is compile-time only (see the About section).
  define: {
    __APP_VERSION__: JSON.stringify(tauriConf.version),
  },
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // WSL: Windows browsers need 0.0.0.0, not loopback-only.
    host: host || true,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Rust changes don't need a frontend reload
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "es2021",
    sourcemap: false,
    chunkSizeWarningLimit: 900,
    rollupOptions: {
      // F1 — the dedicated Guard-2 approval window is its own tiny page
      // (guard.html + src/guard-main.ts), built as a second entry so the
      // guard webview loads a fixed local asset, never the SPA.
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        guard: fileURLToPath(new URL("./guard.html", import.meta.url)),
      },
      output: {
        manualChunks: {
          // Heavy vendors split out of the app chunk (Tauri caches them).
          // Monaco is ~5MB ESM; its own chunk keeps the app chunk small and
          // lets Tauri cache the editor separately.
          monaco: ["monaco-editor", "@monaco-editor/react"],
          charts: ["recharts"],
          motion: ["framer-motion"],
          radix: [
            "@radix-ui/react-accordion",
            "@radix-ui/react-dialog",
            "@radix-ui/react-dropdown-menu",
            "@radix-ui/react-popover",
            "@radix-ui/react-select",
            "@radix-ui/react-tabs",
            "@radix-ui/react-tooltip",
          ],
          markdown: ["react-markdown"],
        },
      },
    },
  },
});
