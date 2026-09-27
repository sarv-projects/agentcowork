/**
 * P50.4.2 — Local model downloads (UI bridge over `model_cmds.rs`).
 *
 * The P27 backend was already landed in Rust (HfClient resumable Range
 * downloads + sha256 verify, ModelRegistry at `<data_dir>/models/hf`,
 * local:// resolver, ModelsRuntime). This module is the consumer wiring:
 * start/cancel/resume downloads, registry CRUD, hardware-fit quant
 * recommendation, and runtime binding — plus the live `model-download`
 * event stream (progress + terminal states).
 *
 * Truth rules: inside Tauri every call is a native call; failures propagate
 * (never converted to fake progress). Outside Tauri the panel is a
 * design-only preview: downloads are not offered (the controls render the
 * honest "requires the Tauri shell" note).
 */

import { inTauri, invoke, listen, type UnlistenFn } from "./tauri";

/** One in-flight or just-finished download (mirrors `DownloadStatus`). */
export interface ModelDownloadRow {
  id: string
  repo: string
  filename: string
  phase: "downloading" | "done" | "error" | "cancelled" | "serving" | "served"
  doneBytes: number
  totalBytes: number
  error?: string | null
  path?: string | null
  registryId?: string | null
}

/** An interrupted `.part` file found on disk (crash/cancel across restarts). */
export interface OrphanPart {
  dest: string
  rel: string
  doneBytes: number
}

/** One installed model in the canonical registry (`index.json`). */
export interface RegistryEntry {
  id: string
  path: string
  sha256: string
  size: number
  ctx: number
  quant: string
  source: string
}

export interface RegistryList {
  models: RegistryEntry[]
  totalBytes: number
  baseDir: string
}

/** Live `model-download` event payload (download + serve kinds). */
export interface ModelDownloadEvent {
  kind: "download" | "serve"
  id: string
  repo?: string
  filename?: string
  phase: string
  doneBytes?: number
  totalBytes?: number
  error?: string | null
  path?: string | null
  registryId?: string | null
  baseUrl?: string | null
}

export async function startDownload(repo: string, filename: string): Promise<{
  ok: boolean
  alreadyInstalled?: boolean
  id?: string
  resuming?: boolean
}> {
  return invoke("model_download_start", { repo, filename })
}

export async function listDownloads(): Promise<{
  active: ModelDownloadRow[]
  orphans: OrphanPart[]
}> {
  return invoke("model_downloads")
}

export async function cancelDownload(id: string): Promise<{ ok: boolean }> {
  return invoke("model_download_cancel", { id })
}

export async function registryList(): Promise<RegistryList> {
  return invoke("model_registry_list")
}

export async function removeModel(id: string): Promise<{ ok: boolean }> {
  return invoke("model_registry_remove", { id })
}

export async function recommendQuant(repo: string): Promise<{
  quant: string
  availableRamBytes: number
}> {
  return invoke("model_recommend_quant", { repo })
}

/** P52.1/P52.3 — dry-run fit estimate (file GB + ctx tokens vs live hardware).
 * Nothing is downloaded or served; returns the tier + file/KV/total split.
 * `file_gb` is in GiB (registry `size` bytes ÷ 2^30). */
export async function estimateFit(
  fileGb: number,
  ctxTokens: number,
): Promise<{
  tier: 'fits' | 'may_be_slow' | 'wont_fit'
  fileGb: number
  kvGb: number
  totalGb: number
  ramGb: number
  vramGb: number
  defaultQuant: string
}> {
  return invoke("model_estimate_fit", { fileGb, ctxTokens })
}

/** P52.4 — per-serve llama.cpp options passed to `model_serve` (real launch
 * flags; camelCase mirrors `ServeOptions` + the extra `kvCache` element
 * type). Every field optional — absent = llama.cpp default. */
export interface ServeOptions {
  gpuLayers?: number
  flashAttn?: 'on' | 'off' | 'auto'
  numCtx?: number
  noMmap?: boolean
  mlock?: boolean
  kvCache?: 'q8_0' | 'q4_0' | 'f32' | 'f16'
  /** P52.7 — runtime: 'gguf' (llamafile, default) or 'mlx' (mlx_lm.server
   * sidecar, Apple Silicon). MLX serves an HF id, not the local GGUF. */
  runtime?: 'gguf' | 'mlx'
  /** P52.7 — HF model id for the MLX sidecar (mlx-community/<name>-4bit);
   * derived from the registry id when omitted. Ignored for the GGUF runtime. */
  modelId?: string
}

export async function serveModel(id: string, options?: ServeOptions): Promise<{
  ok: boolean
  port: number
  baseUrl: string
  starting: boolean
}> {
  if (options) return invoke("model_serve", { id, serveOptions: options })
  return invoke("model_serve", { id, serveOptions: null })
}

/** P52.2 — one pinned file inside a gallery entry (mirrors
 * `agentcowork_catalog::gallery::GalleryFile`). */
export interface GalleryFile {
  path: string
  sha256: string
}

/** P52.2 — one `gallery@model` entry (mirrors `GalleryEntry`). */
export interface GalleryEntry {
  id: string
  files: GalleryFile[]
  /** Backend override (e.g. `ollama`); merges over the default runtime. */
  backend_override: string | null
  /** Preload the weights at startup when true. */
  preload: boolean
}

/** P52.2 — parsed gallery index (mirrors `GalleryIndex`). */
export interface GalleryIndex {
  version: number
  models: GalleryEntry[]
}

/** P52.2 — parse a LocalAI-style gallery `index.yaml` (no network, no
 * install; the native parse also refuses half-pinned files). */
export async function parseGalleryYaml(yaml: string): Promise<GalleryIndex> {
  return invoke("model_gallery_parse", { yaml })
}

/** P52.5 — host accelerator class for the best-variant pick (lowercase,
 * mirrors `HwClass`). */
export type HwClass = "npu" | "gpu" | "cpu"

/** P52.5 — one downloadable weight build (mirrors `VariantCandidate`). */
export interface VariantCandidate {
  repo: string
  file: string
  hw: HwClass
  quant: string
}

/** P52.5 — best-variant pick (pure native pick; download still goes through
 * `startDownload`). Returns `null` when no candidate can be chosen. */
export async function bestPick(
  hw: HwClass,
  candidates: VariantCandidate[],
): Promise<{ repo: string; file: string; hw: HwClass; quant: string } | null> {
  return invoke("model_best_pick", { hw, candidates })
}

/** Subscribe to download/serve progress events; returns an unlisten fn. */
export async function onModelDownloadEvent(
  cb: (e: ModelDownloadEvent) => void,
): Promise<UnlistenFn> {
  return listen<ModelDownloadEvent>("model-download", (event) => cb(event.payload))
}

/** True only in the Tauri shell — the downloader is a native capability. */
export function downloadsAvailable(): boolean {
  return inTauri()
}