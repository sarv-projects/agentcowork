'use client'

import { useEffect, useState } from 'react'
import { FileText } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { OfficeOpenBar } from './office-open-bar'
import OfficeFileSwitcher from './office-file-switcher'
import ChatOverlay from './chat-overlay'
import { docxOpen, docxPatch, docxTracks, officeOpenExternal, isOfficeFloorError, demoDocx, type DocxPayload } from '@/lib/office'
import { useAppStore } from '@/lib/store'
import { inTauri } from '@/lib/tauri'

export default function OfficeDocxView() {
  const [payload, setPayload] = useState<DocxPayload | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [trackChanges, setTrackChanges] = useState(true)
  const [selected, setSelected] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  const [tracks, setTracks] = useState<Array<{ kind: string; author: string; text: string }>>([])
  const [lastAttempted, setLastAttempted] = useState<string | null>(null)
  const [overlayOpen, setOverlayOpen] = useState(false)
  const officePath = useAppStore((s) => s.officePaths['office-docx'])
  const running = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.status === 'running')
  const paused = useAppStore((s) => s.pausedSessions[s.activeSessionId])
  const locked = running && !paused

  // P3.15 — when the surgical engine refuses a file, keep the attempted path
  // so the error banner can offer the honest LibreOffice fallback.
  const fail = (err: unknown, path: string) => {
    setLastAttempted(path)
    setError(err instanceof Error ? err.message : 'Failed to open document')
  }

  const open = async (path: string) => {
    try {
      setError(null)
      setPayload(await docxOpen(path))
      useAppStore.getState().openOfficeDoc(path)
      try {
        const t = await docxTracks(path)
        setTracks(t.changes)
      } catch {
        setTracks([])
      }
    } catch (err) {
      fail(err, path)
    }
  }

  useEffect(() => {
    if (officePath && officePath !== payload?.path) void open(officePath)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [officePath])

  const title = payload?.path ?? 'exec-summary.docx'
  const paragraphs = payload
    ? payload.text.split('\n').filter((l) => l.trim().length > 0)
    : null

  return (
    <div className="flex h-full w-full flex-col bg-zinc-900">
      <header className="flex items-center justify-between border-b border-border px-3 py-2">
        <div className="flex items-center gap-2">
          <FileText className="h-4 w-4 text-blue-400" />
          <span className="max-w-[240px] truncate font-mono text-xs font-medium text-foreground">
            {payload?.path ?? (inTauri() ? 'No document open' : 'exec-summary.docx')}
          </span>
          {payload ? (
            <Badge variant="outline" className="text-[10px] text-emerald-300">
              engine read
            </Badge>
          ) : (
            <Badge
              variant="outline"
              className="gap-1 border-brand/40 bg-brand/10 text-[10px] text-brand"
            >
              <span className="live-dot h-1.5 w-1.5 rounded-full bg-brand" />
              {inTauri() ? 'no file open' : 'preview'}
            </Badge>
          )}
        </div>
        {/* P11.2 — track-changes toggle: highlights the agent's edits like
            Word track changes (modified blocks get a border + badge). */}
        <button
          onClick={() => setTrackChanges(!trackChanges)}
          aria-pressed={trackChanges}
          className={`rounded border px-2 py-0.5 text-[10px] transition-colors ${
            trackChanges
              ? 'border-brand/40 bg-brand/10 text-brand'
              : 'border-border text-muted-foreground'
          }`}
        >
          {trackChanges ? 'Track changes on' : 'Track changes off'}
        </button>
        <Badge variant="secondary" className="text-[10px]">
          block-patch
        </Badge>
      </header>

      <OfficeOpenBar onOpen={open} livePath={payload?.path} />
      <OfficeFileSwitcher view="office-docx" current={payload?.path} onOpen={open} />
      {locked && (
        <div className="border-b border-warning/30 bg-warning/10 px-3 py-1 font-mono text-[10px] text-warning">
          Read-only while the agent is running — takeover (pause) to edit
        </div>
      )}
      <div className="flex gap-1 border-b border-border px-3 py-1">
        <Button size="sm" variant="outline" className="h-6 text-[10px]" disabled={!payload || locked}
          onClick={() => payload && officeOpenExternal(payload.path).catch((e) => setError(String(e)))}>
          Open in LibreOffice
        </Button>
        <Button size="sm" variant="outline" className="h-6 text-[10px]" disabled={!payload}
          onClick={() => setOverlayOpen(true)}
          title="Ask about this document (scoped chat)">
          Ask about doc
        </Button>
        {selected && (
          <Button size="sm" className="h-6 text-[10px]" disabled={locked}
            onClick={() => payload && selected && docxPatch(payload.path, selected, draft).then(() => open(payload.path)).catch((e) => setError(String(e)))}>
            Patch {selected}
          </Button>
        )}
      </div>

      {error && (
        <div className="flex flex-wrap items-center gap-2 border-b border-red-500/30 bg-red-500/10 px-3 py-1.5 font-mono text-[10px] text-red-400">
          <span>⚠ {error}</span>
          {lastAttempted && !isOfficeFloorError(error) && (
            <button
              className="rounded border border-red-500/40 bg-red-500/15 px-1.5 py-0.5 text-[9px] text-red-300 hover:bg-red-500/25"
              onClick={() =>
                officeOpenExternal(lastAttempted).catch((e) => setError(String(e)))
              }
            >
              Engine refused — open in LibreOffice instead
            </button>
          )}
        </div>
      )}

      <ScrollArea className="scroll-thin min-h-0 flex-1">
        {payload ? (
          <div className="mx-auto max-w-3xl bg-[#1c1d20] p-8 sm:p-12">
            <article className="prose-invert space-y-4">
              {paragraphs!.map((p, i) => (
                <p key={i} className="text-sm leading-relaxed text-foreground/90">
                  {p}
                </p>
              ))}
            </article>
            <div className="mt-6 border-t border-border pt-3 font-mono text-[10px] text-muted-foreground">
              {payload.blocks.length} block(s) · surgical OOXML
            </div>
            {payload.blocks.length === 0 && (
              <p className="mt-2 font-mono text-[10px] text-muted-foreground/70">
                No editable blocks in this document — open it in LibreOffice for full fidelity.
              </p>
            )}
            <ul className="mt-2 space-y-1 font-mono text-[10px]">
              {payload.blocks.map((b) => (
                <li key={b.address}>
                  <button
                    className={`w-full truncate text-left ${selected === b.address ? 'text-brand' : 'text-muted-foreground'}`}
                    disabled={locked}
                    onClick={() => {
                      setSelected(b.address)
                      // P50.3.7 — the engine exposes no per-block text read,
                      // so never seed another block's text here: patching
                      // with the wrong seed would corrupt the document.
                      // Start empty; the address labels the target.
                      setDraft('')
                    }}
                  >
                    {b.address} · {b.kind}
                  </button>
                </li>
              ))}
            </ul>
            {selected && (
              <textarea
                className="mt-2 h-20 w-full rounded border border-border bg-zinc-950 p-2 text-xs"
                disabled={locked}
                value={draft}
                placeholder={`Replacement text for ${selected} — Saving patches this block only`}
                onChange={(e) => setDraft(e.target.value)}
              />
            )}
            {trackChanges && tracks.length > 0 && (
              <div className="mt-3 space-y-1 text-[10px] text-brand">
                {tracks.map((t, i) => (
                  <div key={i}>{t.kind} · {t.author}: {t.text}</div>
                ))}
              </div>
            )}
          </div>
        ) : inTauri() ? (
        <div className="mx-auto max-w-3xl rounded-lg border border-dashed border-border bg-background/30 p-10 text-center text-xs text-muted-foreground">
          Open a real DOCX file to view and edit its OOXML blocks.
        </div>
        ) : (
        <div className="mx-auto max-w-3xl bg-[#1c1d20] p-8 sm:p-12">
          <article className="prose-invert space-y-4">
            <h1 className="text-2xl font-bold tracking-tight text-foreground">
              Q3 2026 Executive Summary
            </h1>
            <div className="font-mono text-[10px] text-muted-foreground">
              Prepared by AgentCowork Agent · 2026-09-30
            </div>

            <h2 className="pt-2 text-lg font-semibold text-brand">1. Overview</h2>
            <p className="text-sm leading-relaxed text-foreground/90">
              The third quarter of 2026 marked a strong inflection for the business, with
              revenue acceleration driven by enterprise expansion across the EMEA and APAC
              regions. Customer retention remained above industry benchmarks and product
              velocity increased materially.
            </p>

            <h2 className="pt-2 text-lg font-semibold text-brand">
              2. Financial Highlights
            </h2>
            <ul className="space-y-1.5 text-sm text-foreground/90">
              <li className="flex gap-2">
                <span className="text-brand">▸</span>
                <span>Revenue reached $1.8M, up 20% quarter-over-quarter.</span>
              </li>
              <li className="flex gap-2">
                <span className="text-brand">▸</span>
                <span>Gross margin improved to 66% (from 61% in Q2).</span>
              </li>
              <li className="flex gap-2">
                <span className="text-brand">▸</span>
                <span>Enterprise deal count grew 35% YoY.</span>
              </li>
            </ul>

            <h2 className="pt-2 text-lg font-semibold text-brand">
              3. Key Drivers · §3.2
            </h2>
            <div
              className={
                trackChanges
                  ? 'rounded border-l-2 border-brand bg-brand/5 px-3 py-2'
                  : 'rounded px-3 py-2'
              }
            >
              <div className="mb-1 flex items-center gap-2 font-mono text-[10px] uppercase tracking-wide text-brand">
                <span>{trackChanges ? 'Agent editing · typing' : 'Inserted text'}</span>
                {trackChanges && (
                  <Badge variant="outline" className="border-brand/40 bg-brand/10 text-[9px] text-brand">
                    modified
                  </Badge>
                )}
              </div>
              <p className="overflow-hidden whitespace-nowrap text-sm leading-relaxed text-foreground">
                <span
                  className="inline-block align-bottom"
                  style={{
                    animation: 'type-in 4s steps(60) forwards',
                  }}
                >
                  Revenue grew 20% QoQ, reaching $1.8M driven by enterprise deals.
                </span>
                <span className="caret-blink ml-0.5 inline-block h-4 w-0.5 bg-brand align-middle" />
              </p>
            </div>

            <h2 className="pt-2 text-lg font-semibold text-brand">
              4. Outlook · Q4 2026
            </h2>
            <p className="text-sm leading-relaxed text-muted-foreground">
              <span className="text-foreground/70">Lorem ipsum dolor sit amet</span> — pipeline
              expansion and partner co-sell motion expected to sustain trajectory. ▮
            </p>
          </article>
        </div>
        )}
      </ScrollArea>

      <footer className="flex items-center justify-between border-t border-border bg-zinc-900/60 px-3 py-1.5 font-mono text-[10px] text-muted-foreground">
        <span>{payload ? `Blocks: ${payload.blocks.length}` : inTauri() ? 'Blocks: —' : 'Page 1/3'}</span>
        <span>{payload ? `Words: ${payload.text.split(/\s+/).length}` : inTauri() ? 'Words: —' : 'Words: 847'}</span>
        <Badge
          variant="outline"
          className="gap-1 border-brand/40 text-[9px] text-brand"
        >
          <span className="h-1.5 w-1.5 rounded-full bg-brand" />
          {payload ? 'Modified' : inTauri() ? 'No document' : 'Preview'}
        </Badge>
      </footer>

      {overlayOpen && payload && (
        <ChatOverlay
          title={payload.path}
          context={payload.text}
          onClose={() => setOverlayOpen(false)}
        />
      )}
    </div>
  )
}
