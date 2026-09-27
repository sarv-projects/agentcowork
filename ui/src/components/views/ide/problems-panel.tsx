'use client'

import { useEffect, useState } from 'react'
import { AlertCircle, AlertTriangle, Info } from 'lucide-react'
import { cn } from '@/lib/utils'
import { lspDiagnostics, type LspProblem } from '@/lib/lsp'
import { SkeletonBlock } from '@/components/ui/loading-state'

/**
 * Problems panel — real LSP diagnostics (lsp_cmds → agentcowork-codeintel
 * LspRunner) for the active editor file. The runner spawns the configured
 * server (rust-analyzer / typescript-language-server / pyright), opens the
 * file and returns publishDiagnostics. Honest ceiling: per-file collect
 * (no long-lived session); server must be installed on the machine.
 */
export function ProblemsPanel({
  file,
  onJump,
}: {
  file: { path: string; name: string; content: string } | null
  onJump: (line: number, col: number) => void
}) {
  const [rows, setRows] = useState<LspProblem[]>([])
  const [loading, setLoading] = useState(false)
  const [note, setNote] = useState<string | null>(null)

  useEffect(() => {
    if (!file) {
      setRows([])
      setNote(null)
      return
    }
    let active = true
    setLoading(true)
    setNote(null)
    // Workspace root: file's dir (git root lookup happens on the Rust side
    // when we wire the full session; for now use the file's parent).
    const root = file.path.includes('/') ? file.path.slice(0, file.path.lastIndexOf('/')) : '.'
    void lspDiagnostics(root, file.path, file.name, file.content).then((res) => {
      if (!active) return
      if (res.error) {
        setRows([])
        setNote(res.error)
      } else {
        setRows(res.rows)
      }
      setLoading(false)
    })
    return () => {
      active = false
    }
  }, [file?.path, file?.content])

  const errors = rows.filter((r) => r.severity === 1).length
  const warnings = rows.filter((r) => r.severity === 2).length

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-3 border-b border-border px-3 font-mono text-[10px] text-muted-foreground">
        <span className="font-semibold uppercase tracking-wider">Problems</span>
        <span className="text-rose-400">{errors} errors</span>
        <span className="text-warning">{warnings} warnings</span>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-1.5">
        {loading && <div className="p-2"><SkeletonBlock lines={3} /></div>}
        {note && <div className="p-2 text-[10px] text-muted-foreground">{note}</div>}
        {!loading && !note && rows.length === 0 && (
          <div className="p-2 text-[11px] text-muted-foreground">
            {file ? 'No problems detected.' : 'Open a file to lint it.'}
          </div>
        )}
        {rows.map((r, i) => (
          <button
            key={i}
            onClick={() => onJump(r.line, r.col)}
            className="flex w-full items-start gap-2 rounded px-1.5 py-0.5 text-left text-[11px] hover:bg-accent/40"
          >
            {r.severity === 1 ? (
              <AlertCircle className="mt-0.5 h-3 w-3 shrink-0 text-rose-400" />
            ) : r.severity === 2 ? (
              <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-warning" />
            ) : (
              <Info className="mt-0.5 h-3 w-3 shrink-0 text-sky-400" />
            )}
            <span className="min-w-0 flex-1">
              <span className="text-foreground">{r.message}</span>
              <span className={cn('ml-2 font-mono text-[9px] text-muted-foreground')}>
                {r.path.split('/').pop()} · Ln {r.line + 1}, Col {r.col + 1}
              </span>
            </span>
          </button>
        ))}
      </div>
    </div>
  )
}
