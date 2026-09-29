'use client'

import { useEffect, useId, useState } from 'react'
import { AlertTriangle, Check, Copy, Download, RotateCw } from 'lucide-react'
import { checkMermaidSource, svgViewBoxSize } from './mermaid-policy'

const RENDER_TIMEOUT_MS = 5_000

interface RenderedDiagram {
  url: string
  svg: string
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.toLowerCase().includes('timeout')) {
    return 'Rendering took too long. The source is still available below.'
  }
  return 'This diagram could not be rendered. The source is still available below.'
}

export function MermaidDiagram({ source }: { source: string }) {
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, '')
  const [attempt, setAttempt] = useState(0)
  const [rendered, setRendered] = useState<RenderedDiagram | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [exportMessage, setExportMessage] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const check = checkMermaidSource(source)

  useEffect(() => {
    let active = true
    let objectUrl: string | undefined
    let timeout: number | undefined
    setRendered(null)
    setError(check.ok ? null : check.reason)
    if (!check.ok) return () => { active = false }

    const render = async () => {
      try {
        const mermaid = (await import('mermaid')).default
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: 'strict',
          suppressErrorRendering: true,
          theme: 'neutral',
          flowchart: { htmlLabels: false },
          sequence: { useMaxWidth: true },
        })
        let result: Awaited<ReturnType<typeof mermaid.render>>
        try {
          result = await Promise.race([
            mermaid.render(`diagram-${id}-${attempt}`, check.source),
            new Promise<never>((_, reject) => {
              timeout = window.setTimeout(() => reject(new Error('timeout')), RENDER_TIMEOUT_MS)
            }),
          ])
        } finally {
          if (timeout !== undefined) window.clearTimeout(timeout)
        }
        // Render as an image, never privileged inline SVG. Refuse resource and
        // HTML-bearing output even though Mermaid is in strict mode.
        if (/<(?:script|foreignObject|image|iframe|audio|video|a)\b|\son\w+\s*=|(?:href|src)\s*=|url\s*\(\s*['"]?(?!#)/i.test(result.svg)) {
          throw new Error('unsafe SVG output')
        }
        const blob = new Blob([result.svg], { type: 'image/svg+xml' })
        objectUrl = URL.createObjectURL(blob)
        if (active) setRendered({ url: objectUrl, svg: result.svg })
        else URL.revokeObjectURL(objectUrl)
      } catch (renderError) {
        if (active) setError(errorMessage(renderError))
      }
    }
    void render()

    return () => {
      active = false
      if (timeout !== undefined) window.clearTimeout(timeout)
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [attempt, check.ok, check.ok ? check.source : check.reason, id])

  const copySource = async () => {
    try {
      await navigator.clipboard?.writeText(source)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1_500)
    } catch {
      setError('Clipboard access is unavailable. You can select and copy the source below.')
    }
  }

  const downloadSvg = () => {
    if (!rendered) return
    const url = URL.createObjectURL(new Blob([rendered.svg], { type: 'image/svg+xml' }))
    const link = document.createElement('a')
    link.href = url
    link.download = 'diagram.svg'
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1_000)
  }

  const downloadPng = async () => {
    if (!rendered) return
    setExportMessage(null)
    const size = svgViewBoxSize(rendered.svg)
    if (!size) {
      setExportMessage('PNG export is unavailable for this diagram. SVG download remains available.')
      return
    }
    try {
      const scale = Math.min(2, 4096 / Math.max(size.width, size.height))
      const canvas = document.createElement('canvas')
      canvas.width = Math.max(1, Math.round(size.width * scale))
      canvas.height = Math.max(1, Math.round(size.height * scale))
      const context = canvas.getContext('2d')
      if (!context) throw new Error('Canvas is unavailable')
      const bitmap = await createImageBitmap(new Blob([rendered.svg], { type: 'image/svg+xml' }))
      try {
        context.drawImage(bitmap, 0, 0, canvas.width, canvas.height)
      } finally {
        bitmap.close()
      }
      const png = await new Promise<Blob>((resolve, reject) => {
        canvas.toBlob((blob) => blob ? resolve(blob) : reject(new Error('PNG conversion failed')), 'image/png')
      })
      const url = URL.createObjectURL(png)
      const link = document.createElement('a')
      link.href = url
      link.download = 'diagram.png'
      link.click()
      window.setTimeout(() => URL.revokeObjectURL(url), 1_000)
      setExportMessage('PNG downloaded.')
    } catch {
      setExportMessage('PNG export is unavailable here. SVG download remains available.')
    }
  }

  return (
    <figure className="my-3 overflow-hidden rounded-xl border border-border bg-background/70" aria-label="Diagram">
      <div className="flex min-h-36 items-center justify-center overflow-auto p-4 sm:min-h-48">
        {rendered ? (
          <img src={rendered.url} alt="Rendered diagram" className="h-auto max-h-[32rem] max-w-full object-contain" />
        ) : error ? (
          <div role="status" className="flex max-w-md flex-col items-center gap-2 text-center text-xs text-muted-foreground">
            <AlertTriangle aria-hidden className="h-4 w-4 text-amber-500" />
            <span>{error}</span>
            {check.ok && (
              <button type="button" onClick={() => setAttempt((value) => value + 1)} className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-border px-3 text-foreground hover:bg-accent">
                <RotateCw aria-hidden className="h-3.5 w-3.5" /> Try again
              </button>
            )}
          </div>
        ) : (
          <div role="status" className="text-xs text-muted-foreground">Rendering diagram…</div>
        )}
      </div>
      <figcaption className="flex items-center justify-between gap-2 border-t border-border px-3 py-2">
        <details className="min-w-0 flex-1">
          <summary className="cursor-pointer text-[11px] text-muted-foreground hover:text-foreground">Diagram source</summary>
          <pre className="mt-2 max-h-64 overflow-auto rounded-lg bg-muted/50 p-3 font-mono text-[11px] leading-relaxed text-foreground">{source}</pre>
        </details>
        <div className="flex shrink-0 items-center gap-1">
          {rendered && (
            <>
              <button type="button" onClick={downloadSvg} className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-2 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground" aria-label="Download diagram as SVG">
                <Download aria-hidden className="h-3.5 w-3.5" /> SVG
              </button>
              <button type="button" onClick={() => void downloadPng()} className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-2 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground" aria-label="Download diagram as PNG">
                <Download aria-hidden className="h-3.5 w-3.5" /> PNG
              </button>
            </>
          )}
          <button type="button" onClick={() => void copySource()} className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-2 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground" aria-label="Copy diagram source">
            {copied ? <Check aria-hidden className="h-3.5 w-3.5" /> : <Copy aria-hidden className="h-3.5 w-3.5" />}
            {copied ? 'Copied' : 'Copy source'}
          </button>
        </div>
      </figcaption>
      {exportMessage && <div role="status" className="border-t border-border px-3 py-2 text-[11px] text-muted-foreground">{exportMessage}</div>}
    </figure>
  )
}
