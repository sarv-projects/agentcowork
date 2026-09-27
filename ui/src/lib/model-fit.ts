/**
 * P52.1/P52.5 — pure fit + variant-pick helpers over the live native seams
 * (`model_estimate_fit`, `model_best_pick`). Keeping the derivation logic
 * here (and unit-tested) means the traffic-light chips and the one-click
 * auto-pick always describe the same quant/ctx vocabulary the Rust side
 * uses — no client-side copy of the *decision*, only of the *input build*.
 *
 * The decision stays native: estimate tiers come from
 * `agentcowork_core::models::fit` (60% / 85% of budget) and the variant pick
 * from `agentcowork_core::models::best`. This module only shapes their inputs
 * and turns their outputs into UI semantics.
 */

import { quantFromPath, type HardwareProfile } from "./local-models"
import type { HwClass, VariantCandidate } from "./models-download"

/** GGUF Q4_K_M — the size/quality default (mirrors fit::DEFAULT_QUANT). */
export const DEFAULT_QUANT = "Q4_K_M"

/** Context the fit pre-check estimates at — the serving default `num_ctx`
 * (16,384; see `agentcowork-core` `default_num_ctx`). */
export const FIT_CTX = 16384
export const FIT_CTX_LABEL = "16K"

/** Tier vocabulary returned by `model_estimate_fit`. */
export type FitTier = "fits" | "may_be_slow" | "wont_fit"

export interface FitEstimate {
  tier: FitTier
  fileGb: number
  kvGb: number
  totalGb: number
  ramGb: number
  vramGb: number
  defaultQuant: string
}

export interface TierTone {
  key: "ok" | "warn" | "bad"
  label: string
  /** One-line human explanation shown next to the light. */
  hint: string
}

/** Turn a native tier into traffic-light semantics. Unknown tiers fail
 * closed to the worst tone — never silently green. */
export function tierTone(tier: FitTier | string | null | undefined): TierTone {
  switch (tier) {
    case "fits":
      return {
        key: "ok",
        label: "fits",
        hint: "File + KV within 60% of RAM — comfortable headroom.",
      }
    case "may_be_slow":
      return {
        key: "warn",
        label: "may be slow",
        hint: "File + KV within 85% of RAM — expect pressure on long contexts.",
      }
    default:
      return {
        key: "bad",
        label: "won't fit",
        hint: "Beyond 85% of RAM — pick a smaller quant or shorter context.",
      }
  }
}

/** The host accelerator class reported to the native picker. A reported GPU
 * means the runner can offload, so we ask for GPU-targeted builds first
 * (the native fallback still prefers a portable Q4_K_M build over failing). */
export function hostHwClass(hw: HardwareProfile | null | undefined): HwClass {
  const gpu = (hw?.gpu ?? "").trim().toLowerCase()
  if (!gpu || gpu === "—" || gpu === "unknown") return "cpu"
  return "gpu"
}

/** Class of a single weight file from its path markers. Plain GGUF builds
 * are portable (CPU); only explicit neural markers opt a build into `npu`. */
export function fileHwClass(path: string): HwClass {
  const lower = path.toLowerCase()
  return /npu|ane|neural|mlx|metal/i.test(lower) ? "npu" : "cpu"
}

/** Build the candidate list the native best-variant picker consumes, from
 * the repo's `.gguf` files. Non-GGUF files are excluded (this picker is for
 * the llama.cpp-family runtime the downloader serves). */
export function buildCandidates(
  repo: string,
  files: ReadonlyArray<{ path: string }>,
): VariantCandidate[] {
  const out: VariantCandidate[] = []
  for (const f of files) {
    if (!f.path.toLowerCase().endsWith(".gguf")) continue
    out.push({
      repo,
      file: f.path,
      hw: fileHwClass(f.path),
      quant: quantFromPath(f.path),
    })
  }
  return out
}

/** GiB for `estimateFit(fileGb)`: bytes ÷ 2^30 (matches the native registry
 * size convention — `size` is bytes, divided by 2^30 Rust-side). */
export function bytesToGib(bytes: number): number {
  if (!bytes || bytes <= 0) return 0
  return bytes / 2 ** 30
}
