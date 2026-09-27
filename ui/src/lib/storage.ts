// P4.8 (D9–D12, G7) — storage intelligence bridge. Mirrors the Rust
// `storage_*` Tauri commands (agentcowork-storage crate). In a plain-browser
// preview (no shell) the callers fall back to demo data so the page is
// explorable.

import { inTauri, invoke } from "./tauri";
import { nativeCall } from './runtime';

export interface StorageHealth {
  mount: string;
  totalBytes: number;
  availableBytes: number;
  usedBytes: number;
  usedPct: number;
  thresholdPct: number;
  overThreshold: boolean;
  battery: boolean;
}

export interface TreemapRect {
  id: number;
  name: string;
  path: string;
  size: number;
  isDir: boolean;
  w: number;
  h: number;
  color: [number, number, number];
}

export interface StorageScan {
  deferred: boolean;
  reason?: string;
  files: number;
  root?: string;
  treemap: TreemapRect[];
}

export interface LargeFile {
  name: string;
  path: string;
  size: number;
  isDir: boolean;
}

export interface DupGroup {
  size: number;
  wastedBytes: number;
  copies: number;
  files: string[];
}

/** D9–D12 — a Guard-2 cleanup proposal (decision-package shape). The storage
 * engine only ever PROPOSES; deletion runs through a Guard-2 ticket. */
export interface CleanupProposal {
  goal?: string;
  summary?: string;
  risk?: string;
  paths?: string[];
  bytes?: number;
  [k: string]: unknown;
}

/** D12 — free-space health (never battery-gated). */
export async function storageHealth(path?: string): Promise<StorageHealth> {
  if (!inTauri()) return demoHealth();
  return nativeCall('storage health', () => invoke<StorageHealth>("storage_health", path ? { path } : undefined));
}

/** D9/D10 — scan + treemap (battery-gated on the Rust side). */
export async function storageScan(path?: string): Promise<StorageScan> {
  if (!inTauri()) return demoScan();
  return nativeCall('storage scan', () => invoke<StorageScan>("storage_scan", path ? { path } : undefined));
}

/** D11 — largest files (battery-gated). */
export async function storageLargeFiles(path?: string): Promise<LargeFile[]> {
  if (!inTauri()) return demoLargeFiles();
  const r = await nativeCall('storage large files', () => invoke<{ deferred: boolean; files: LargeFile[] }>(
    "storage_large_files",
    path ? { path } : undefined,
  ));
  return r.deferred ? [] : r.files;
}

/** D10 — duplicate groups (battery-gated). */
export async function storageDuplicates(path?: string): Promise<DupGroup[]> {
  if (!inTauri()) return demoDupGroups();
  const r = await nativeCall('storage duplicates', () => invoke<{ deferred: boolean; groups: DupGroup[] }>(
    "storage_duplicates",
    path ? { path } : undefined,
  ));
  return r.deferred ? [] : r.groups;
}

/** D9–D12 — large-file cleanup proposals (Guard-2 decision packages; battery-gated). */
export async function storageCleanupProposals(
  path?: string,
  topN = 10,
): Promise<CleanupProposal[]> {
  if (!inTauri()) return demoCleanupProposals();
  const args: Record<string, unknown> = { topN };
  if (path) args.path = path;
  const r = await nativeCall('storage cleanup proposals', () => invoke<{ deferred: boolean; proposals: CleanupProposal[] }>(
    "storage_cleanup_proposals",
    args,
  ));
  return r.deferred ? [] : r.proposals;
}

/** J16 — tell the Rust side whether the device is on battery (heavy scans defer). */
export async function storageBattery(on: boolean): Promise<void> {
  if (!inTauri()) return;
  await nativeCall('storage battery', () => invoke("storage_battery", { on }));
}

function bytes(v: number): string {
  if (v <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let n = v;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i += 1;
  }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

export { bytes };

// ---- demo data (preview mode) ------------------------------------------

function demoHealth(): StorageHealth {
  return {
    mount: "/",
    totalBytes: 100 * 1024 ** 3,
    availableBytes: 35.8 * 1024 ** 3,
    usedBytes: 64.2 * 1024 ** 3,
    usedPct: 64.2,
    thresholdPct: 90,
    overThreshold: false,
    battery: false,
  };
}

function demoScan(): StorageScan {
  return {
    deferred: false,
    files: 118_402,
    root: "~",
    treemap: [
      { id: 1, name: "raw-events.csv", path: "data/raw-events.csv", size: 18 * 1024 ** 2, isDir: false, w: 0.5, h: 0.5, color: [56, 189, 248] },
      { id: 2, name: "pitch.pptx", path: "pitch.pptx", size: 8.4 * 1024 ** 2, isDir: false, w: 0.4, h: 0.4, color: [249, 115, 22] },
      { id: 3, name: "Q3-Financials.xlsx", path: "Q3-Financials.xlsx", size: 2.1 * 1024 ** 2, isDir: false, w: 0.3, h: 0.3, color: [16, 185, 129] },
      { id: 4, name: "logo.png", path: "assets/logo.png", size: 4.2 * 1024 ** 2, isDir: false, w: 0.3, h: 0.3, color: [168, 85, 247] },
      { id: 5, name: "exec-summary.docx", path: "exec-summary.docx", size: 412 * 1024, isDir: false, w: 0.2, h: 0.2, color: [59, 130, 246] },
    ],
  };
}

function demoLargeFiles(): LargeFile[] {
  return [
    { name: "raw-events.csv", path: "data/raw-events.csv", size: 18 * 1024 ** 2, isDir: false },
    { name: "pitch.pptx", path: "pitch.pptx", size: 8.4 * 1024 ** 2, isDir: false },
    { name: "logo.png", path: "assets/logo.png", size: 4.2 * 1024 ** 2, isDir: false },
  ];
}

function demoDupGroups(): DupGroup[] {
  return [
    { size: 4000, wastedBytes: 12 * 1024, copies: 3, files: ["src/pipeline.ts", "out/pipeline.ts", "data/pipeline.ts"] },
    { size: 4.2 * 1024 ** 2, wastedBytes: 4.2 * 1024 ** 2, copies: 2, files: ["assets/logo.png", "public/logo.png"] },
  ];
}

function demoCleanupProposals(): CleanupProposal[] {
  return [
    { goal: "Move raw-events.csv to review", summary: "18 MB large file, unopened 90d", risk: "medium", paths: ["data/raw-events.csv"], bytes: 18 * 1024 ** 2 },
    { goal: "Remove duplicate logo.png", summary: "2 copies, keep newest", risk: "low", paths: ["public/logo.png"], bytes: 4.2 * 1024 ** 2 },
  ];
}
