export const MERMAID_MAX_SOURCE_CHARS = 50_000
export const MERMAID_MAX_EDGES = 500

export type MermaidSourceCheck =
  | { ok: true; source: string }
  | { ok: false; reason: string }

export function svgViewBoxSize(svg: string): { width: number; height: number } | null {
  const root = svg.match(/<svg\b[^>]*>/i)?.[0]
  const viewBox = root?.match(/\bviewBox\s*=\s*['"]\s*([\d.e+-]+)[ ,]+([\d.e+-]+)[ ,]+([\d.e+-]+)[ ,]+([\d.e+-]+)\s*['"]/i)
  if (!viewBox) return null
  const width = Number(viewBox[3])
  const height = Number(viewBox[4])
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return null
  return { width, height }
}

/** Validate user/model-provided Mermaid before passing it to the renderer. */
export function checkMermaidSource(source: string): MermaidSourceCheck {
  const normalized = source.replace(/^\uFEFF/, '').trim()
  if (!normalized) return { ok: false, reason: 'The diagram is empty.' }
  if (normalized.length > MERMAID_MAX_SOURCE_CHARS) {
    return { ok: false, reason: `This diagram is over the ${MERMAID_MAX_SOURCE_CHARS.toLocaleString()} character limit.` }
  }
  // Mermaid directives can override renderer configuration. This surface keeps
  // security and theme options controlled by the host, so directives are refused.
  if (/%%\s*\{/i.test(normalized)) {
    return { ok: false, reason: 'This diagram uses a configuration directive that is not allowed here.' }
  }

  const edges = normalized.match(/(?:<)?(?:o|x)?[-.=]{2,}(?:o|x)?(?:>)?/g)?.length ?? 0
  if (edges > MERMAID_MAX_EDGES) {
    return { ok: false, reason: `This diagram has more than ${MERMAID_MAX_EDGES} connections.` }
  }
  return { ok: true, source: normalized }
}
