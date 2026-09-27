'use client'

import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { FileText, ChevronLeft, ChevronRight, ZoomIn, ZoomOut, MessageSquare, ScanSearch } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { inTauri } from '@/lib/tauri'
import { useAppStore } from '@/lib/store'
import { OfficeOpenBar } from './office-open-bar'
import OfficeFileSwitcher from './office-file-switcher'
import ChatOverlay from './chat-overlay'
import { pdfOpen, pdfBytes, pdfPageOp, officeOpenExternal, isOfficeFloorError, type PdfPayload } from '@/lib/office'

const PdfCanvas = lazy(() => import('./pdf-canvas'))
// P2.12 — pdf.js text-layer find: the lazy canvas exposes `findText` when a
// live document is mounted; the input falls back to the extracted-text array
// when it is not (preview / text-extraction fallback mode).
type PdfFindHandle = { findText: (query: string) => Promise<number | null> }

const FORM_FIELDS = [
  { label: 'Party A:', value: 'Acme Holdings, Inc.', top: 18 },
  { label: 'Party B:', value: 'AgentCowork, LLC', top: 26 },
  { label: 'Effective Date:', value: '2026-10-01', top: 34 },
  { label: 'Contract Value:', value: '$ 1,800,000.00 USD', top: 42, highlight: true },
]

// P4.7 — demo document text for the chat overlay when no PDF is open.
const DEMO_DOC_TEXT = [
  'Master Services Agreement between Acme Holdings, Inc. and AgentCowork, LLC, effective 2026-10-01.',
  '4.2 Payment Schedule: invoices are issued monthly and due net-thirty (30) days from issuance.',
  '4.3 Late Payment: any payment not received within fifteen (15) days of the due date accrues interest at 1.5% per month.',
  'Contract value is $1,800,000.00 USD, payable in twelve installments.',
].join('\n')

export default function OfficePdfView() {
  const [page, setPage] = useState(1)
  const [zoom, setZoom] = useState(100)
  const [payload, setPayload] = useState<PdfPayload | null>(null)
  const [dataUrl, setDataUrl] = useState<string | null>(null)
  const [pixelsUnavailable, setPixelsUnavailable] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // P50.3.7 — same attachment wiring as Word (store owns path/history).
  const officePath = useAppStore((s) => s.officePaths['office-pdf'])
  const setScopedView = useAppStore((s) => s.setScopedView)
  const setScopedDoc = useAppStore((s) => s.setScopedDoc)
  const [overlayOpen, setOverlayOpen] = useState(false)
  const [lastAttempted, setLastAttempted] = useState<string | null>(null)
  const pdfRef = useRef<PdfFindHandle>(null)
  const running = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId)?.status === 'running')
  const paused = useAppStore((s) => s.pausedSessions[s.activeSessionId])
  // P1.9 — read-only while the agent is running (same lock as Word/Excel).
  const locked = running && !paused
  // P4.7 — document text injected as the chat-overlay's `<user_document>`.
  const docContext = payload ? payload.texts.join('\n') : inTauri() ? '' : DEMO_DOC_TEXT
  // P33 scoped-PDF fix — keep the store's scoped document in sync so the
  // main composer's sendUserMessage can ground answers without the overlay.
  const docTitle = payload?.path ?? 'contract.pdf'

  const open = async (path: string) => {
    try {
      setError(null)
      setPixelsUnavailable(false)
      setPayload(await pdfOpen(path))
      useAppStore.getState().openOfficeDoc(path)
      setPage(1)
      // Real pixels when the shell can hand us the raw bytes (pdf.js render);
      // fall back to the text-extraction cards if that fails.
      try {
        setDataUrl(await pdfBytes(path))
      } catch {
        setDataUrl(null)
        setPixelsUnavailable(true)
      }
    } catch (err) {
      setLastAttempted(path)
      setError(err instanceof Error ? err.message : 'Failed to open PDF')
    }
  }

  // P50.3.7 — open the store-owned path (artifact / folder / tab-switch).
  useEffect(() => {
    if (officePath && officePath !== payload?.path) void open(officePath)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [officePath])

  // Mutating page ops honor the agent-run lock at the call site too (the
  // buttons are disabled, but the helpers must also refuse — never rely on
  // button-only enforcement for an engine mutation).
  const runPageOp = async (op: string, payloadJson: string) => {
    if (!payload || locked) return
    try {
      await pdfPageOp(payload.path, op, { other: payloadJson })
      await open(payload.path)
    } catch (e) {
      setError(String(e))
    }
  }
  const [annotText, setAnnotText] = useState('')
  const [redactRect, setRedactRect] = useState('60,60,540,90')
  const [formFields, setFormFields] = useState('')
  const annotatePage = () => {
    void runPageOp('annotate', JSON.stringify({ page, rect: [60, 60, 540, 90], text: annotText }))
  }
  const redactPage = () => {
    const nums = redactRect.split(',').map(Number)
    if (nums.length !== 4 || nums.some((n) => Number.isNaN(n))) {
      setError('Redact needs exactly four numbers: x1,y1,x2,y2')
      return
    }
    void runPageOp('redact', JSON.stringify([{ page, rect: nums }]))
  }
  const fillForm = () => {
    if (!formFields.trim()) {
      setError('Form fields need field=value pairs')
      return
    }
    const fields = formFields.split(',').map((kv) => {
      const [field, ...rest] = kv.split('=')
      return { field: field?.trim() ?? '', value: rest.join('=').trim() }
    })
    if (fields.some((f) => !f.field)) {
      setError('Form fields need field=value pairs')
      return
    }
    void runPageOp('form_fill', JSON.stringify(fields))
  }

  return (
    <div className="relative flex h-full w-full flex-col bg-zinc-900">
      <header className="flex items-center justify-between border-b border-border px-3 py-2">
        <div className="flex items-center gap-2">
          <FileText className="h-4 w-4 text-red-400" />
          <span className="max-w-[240px] truncate font-mono text-xs font-medium text-foreground">
            {payload?.path ?? (inTauri() ? 'No PDF open' : 'contract.pdf')}
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
              demo
            </Badge>
          )}
        </div>
        <Badge variant="secondary" className="text-[10px]">
          lopdf
        </Badge>
        {/* P4.7 — chat overlay: scope + inject this PDF's text as context */}
        <Button
          size="sm"
          variant={overlayOpen ? 'default' : 'outline'}
          onClick={() => {
            setOverlayOpen((v) => !v)
            if (!overlayOpen) {
              setScopedView('office-pdf')
              setScopedDoc({ title: docTitle, content: docContext })
            } else {
              setScopedView(undefined)
              setScopedDoc(undefined)
            }
          }}
          className={cn(
            'h-6 gap-1 px-2 text-[10px]',
            overlayOpen && 'bg-brand text-white hover:bg-brand-hover'
          )}
          title="Ask the chat about this PDF — the document text is injected as context"
        >
          <ScanSearch className="h-3 w-3" />
          {overlayOpen ? 'Close chat' : 'Ask about this PDF'}
        </Button>
      </header>

      <OfficeOpenBar onOpen={open} livePath={payload?.path} />
      <OfficeFileSwitcher view="office-pdf" current={payload?.path} onOpen={open} />
      {payload && (
        <div className="flex flex-wrap items-center gap-1 border-b border-border px-3 py-1">
          <input
            placeholder="Find in PDF…"
            className="h-6 flex-1 rounded border border-border bg-zinc-950 px-2 font-mono text-[10px]"
            onChange={(e) => {
              const q = e.target.value.toLowerCase()
              if (!q) return
              // Real pdf.js text-layer search when the canvas is live;
              // extracted-text array otherwise (honest fallback).
              if (pdfRef.current) {
                void pdfRef.current.findText(q).then((p) => {
                  if (p) setPage(p)
                })
              } else {
                const idx = payload.texts.findIndex((t) => t.toLowerCase().includes(q))
                if (idx >= 0) setPage(idx + 1)
              }
            }}
          />
          <Button size="sm" variant="outline" className="h-6 text-[10px]"
            disabled={locked} title={locked ? 'Read-only while the agent is running' : 'Rotate 90°'}
            onClick={() => pdfPageOp(payload.path, 'rotate', { delta: 90 }).then(() => open(payload.path)).catch((e) => setError(String(e)))}>
            Rotate 90°
          </Button>
          {/* P2.12 — surgical content ops on the same engine the agent uses.
              Inline inputs (no prompt dialogs): blank annotation = highlight. */}
          <input
            value={annotText}
            onChange={(e) => setAnnotText(e.target.value)}
            placeholder="Annotation text…"
            disabled={locked}
            className="h-6 w-32 rounded border border-border bg-zinc-950 px-1.5 font-mono text-[10px] disabled:opacity-40"
          />
          <Button size="sm" variant="outline" className="h-6 text-[10px]" disabled={locked} onClick={annotatePage} title="Add a highlight/sticky-note annotation on the current page">
            Annotate
          </Button>
          <input
            value={redactRect}
            onChange={(e) => setRedactRect(e.target.value)}
            placeholder="x1,y1,x2,y2"
            disabled={locked}
            className="h-6 w-28 rounded border border-border bg-zinc-950 px-1.5 font-mono text-[10px] disabled:opacity-40"
          />
          <Button size="sm" variant="outline" className="h-6 text-[10px]" disabled={locked} onClick={redactPage} title="Redact a rect on the current page">
            Redact
          </Button>
          <input
            value={formFields}
            onChange={(e) => setFormFields(e.target.value)}
            placeholder="field=value,…"
            disabled={locked}
            className="h-6 w-32 rounded border border-border bg-zinc-950 px-1.5 font-mono text-[10px] disabled:opacity-40"
          />
          <Button size="sm" variant="outline" className="h-6 text-[10px]" disabled={locked} onClick={fillForm} title="Fill AcroForm fields">
            Fill form
          </Button>
          <Button size="sm" variant="outline" className="h-6 text-[10px]"
            onClick={() => officeOpenExternal(payload.path).catch((e) => setError(String(e)))}>
            Open in LibreOffice
          </Button>
        </div>
      )}

      {locked && (
        <div className="border-b border-warning/30 bg-warning/10 px-3 py-1 font-mono text-[10px] text-warning">
          Read-only while the agent is running — pause to take over
        </div>
      )}

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

      <div className="flex min-h-0 flex-1">
      {/* P2.12 — thumbnail rail (page cards from the extracted text) */}
      {payload && (
        <ScrollArea className="scroll-thin w-28 shrink-0 border-r border-border bg-zinc-950/40">
          <div className="space-y-2 p-2">
            {Array.from({ length: payload.pages }, (_, i) => i + 1).map((n) => (
              <button
                key={n}
                onClick={() => setPage(n)}
                className={cn(
                  'block w-full rounded border p-1 text-left transition-colors',
                  page === n
                    ? 'border-brand/60 bg-brand/10'
                    : 'border-border bg-zinc-950/40 hover:border-muted-foreground',
                )}
              >
                <div className="truncate text-[9px] font-medium text-foreground">
                  {n}
                </div>
                <div className="mt-0.5 line-clamp-3 text-[8px] leading-tight text-muted-foreground">
                  {payload.texts[n - 1] ?? '—'}
                </div>
              </button>
            ))}
          </div>
        </ScrollArea>
      )}
      <ScrollArea className="scroll-thin min-h-0 flex-1 bg-zinc-950/40">
        <div className="flex justify-center p-4">
          {payload ? (
            dataUrl ? (
              <Suspense
                fallback={
                  <div className="rounded-sm bg-[#fbfbf9] p-6 font-mono text-[11px] text-zinc-500">
                    Rendering page…
                  </div>
                }
              >
                <PdfCanvas ref={pdfRef} dataUrl={dataUrl} page={page} scale={zoom / 100} />
              </Suspense>
            ) : (
              <div className="w-full max-w-3xl space-y-2">
                {pixelsUnavailable && (
                  <div className="rounded-md border border-warning/30 bg-warning/10 px-3 py-1.5 font-mono text-[10px] text-warning">
                    Pixel render unavailable — showing extracted text instead.
                  </div>
                )}
                {payload.texts[page - 1] != null ? (
                  <div className="rounded-sm bg-[#fbfbf9] p-6 font-mono text-[11px] leading-relaxed text-zinc-800 shadow-xl">
                    {payload.texts[page - 1]}
                  </div>
                ) : (
                  <div className="rounded-sm bg-[#fbfbf9] p-6 font-mono text-[11px] text-zinc-500 shadow-xl">
                    This page has no extractable text.
                  </div>
                )}
              </div>
            )
          ) : inTauri() ? (
          <div className="flex min-h-[420px] w-full max-w-3xl items-center justify-center rounded-lg border border-dashed border-border bg-background/30 p-8 text-center text-xs text-muted-foreground">
            Open a real PDF to view pages, extract text, and use document actions.
          </div>
          ) : (
          <div
            className="relative rounded-sm bg-[#fbfbf9] shadow-xl"
            style={{ width: `${(612 * zoom) / 100}px`, height: `${(792 * zoom) / 100}px` }}
          >
            <div className="absolute inset-0 p-8 text-zinc-800">
              <div className="mb-1 text-[9px] uppercase tracking-widest text-zinc-400">
                Master Services Agreement
              </div>
              <h1 className="mb-3 text-lg font-bold text-zinc-900">
                Section 4 — Compensation &amp; Payment Terms
              </h1>

              <div className="space-y-1.5 text-[11px] leading-relaxed">
                <p>
                  This Master Services Agreement (&quot;Agreement&quot;) is entered into
                  between the parties identified below, effective as of the Effective Date set
                  forth herein.
                </p>

                {FORM_FIELDS.map((f) => (
                  <div
                    key={f.label}
                    className={cn(
                      'relative rounded border px-2 py-1',
                      f.highlight
                        ? 'border-brand bg-warning/40'
                        : 'border-warning bg-warning/60'
                    )}
                    style={{ marginTop: '8px' }}
                  >
                    <div className="flex items-baseline gap-2">
                      <span className="font-semibold text-zinc-700">{f.label}</span>
                      <span className="flex-1 border-b border-dotted border-zinc-400 font-mono">
                        {f.value}
                        {f.highlight && (
                          <span className="caret-blink ml-0.5 inline-block h-3 w-0.5 bg-brand align-middle" />
                        )}
                      </span>
                    </div>
                    <span className="absolute -right-5 top-0 text-[8px] text-zinc-500">
                      <MessageSquare className="h-3 w-3" />
                    </span>
                  </div>
                ))}

                <p className="pt-2">
                  4.2 <span className="font-semibold">Payment Schedule.</span> Invoices shall
                  be issued on a monthly basis and are due net-thirty (30) days from the date
                  of issuance.
                </p>
                <p>
                  4.3 <span className="font-semibold">Late Payment.</span> Any payment not
                  received within fifteen (15) days of the due date shall accrue interest at
                  1.5% per month.
                </p>
              </div>

              <div className="absolute bottom-8 left-8 right-8 flex justify-between text-[9px] text-zinc-400">
                <span>Acme Holdings, Inc.</span>
                <span>AgentCowork, LLC</span>
              </div>
              <div className="absolute bottom-2 left-0 right-0 text-center text-[8px] text-zinc-400">
                Page {page} of 8 · CONFIDENTIAL
              </div>
            </div>

            <div className="absolute right-2 top-2 rounded bg-brand/90 px-1.5 py-0.5 font-mono text-[8px] text-black">
              ANNOTATION · §4.1
            </div>
          </div>
          )}
        </div>
      </ScrollArea>
      </div>

      <footer className="flex items-center justify-between border-t border-border bg-zinc-900/60 px-3 py-1.5 font-mono text-[10px] text-muted-foreground">
        <div className="flex items-center gap-2">
          <span>Page</span>
          <button
            onClick={() => setPage(Math.max(1, page - 1))}
            className="rounded p-0.5 hover:bg-accent hover:text-foreground"
          >
            <ChevronLeft className="h-3 w-3" />
          </button>
          <span className="text-foreground">
            {page} / {payload ? payload.pages : inTauri() ? '—' : 8}
          </span>
          <button
            onClick={() => setPage(Math.min(payload ? payload.pages : 8, page + 1))}
            className="rounded p-0.5 hover:bg-accent hover:text-foreground"
          >
            <ChevronRight className="h-3 w-3" />
          </button>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setZoom(Math.max(50, zoom - 10))}
            className="rounded p-0.5 hover:bg-accent hover:text-foreground"
          >
            <ZoomOut className="h-3 w-3" />
          </button>
          <span className="text-foreground">Zoom: {zoom}%</span>
          <button
            onClick={() => setZoom(Math.min(200, zoom + 10))}
            className="rounded p-0.5 hover:bg-accent hover:text-foreground"
          >
            <ZoomIn className="h-3 w-3" />
          </button>
        </div>
      </footer>

      {overlayOpen && (
        <ChatOverlay
          title={payload?.path ?? 'contract.pdf'}
          context={docContext}
          onClose={() => setOverlayOpen(false)}
        />
      )}
    </div>
  )
}
