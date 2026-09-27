'use client'

// P11.5.10 — Kanban view for parallel sub-agents, with git-worktree isolation
// per branch (Codex-app pattern / doc 47). The board renders blueprint tasks
// (P6.1) as cards across status columns; each card carries its agent, branch
// (a separate `git worktree`), and verify gate. In preview mode the board
// renders a small demo so the surface stays explorable.

import { useEffect, useMemo, useState } from 'react'
import { GitBranch, ShieldCheck } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/lib/store'
import { inTauri } from '@/lib/tauri'

export type KanbanStatus = 'pending' | 'in_progress' | 'review' | 'done' | 'blocked'

export interface KanbanCard {
  id: string
  title: string
  status: KanbanStatus
  agent: string
  model: string
  /** The branch this sub-agent owns (a separate git worktree). */
  branch: string
  /** Verify-gated: the deterministic check that must pass before done. */
  verify: string
  progress?: number
  /** Per-agent iteration budget consumed / total (B6 Hermes 500/50). */
  iterations?: { used: number; total: number }
}

const COLUMNS: { status: KanbanStatus; label: string }[] = [
  { status: 'pending', label: 'Pending' },
  { status: 'in_progress', label: 'In progress' },
  { status: 'review', label: 'Review' },
  { status: 'done', label: 'Done' },
  { status: 'blocked', label: 'Blocked' },
]

const DEMO: KanbanCard[] = [
  { id: 't1', title: 'Add auth to API', status: 'in_progress', agent: 'coder-1', model: 'gpt-5-codex', branch: 'worktree/agent-1', verify: 'tests/auth.test.ts passes', progress: 62, iterations: { used: 31, total: 50 } },
  { id: 't2', title: 'Migrate search index', status: 'pending', agent: 'coder-2', model: 'claude-sonnet-4', branch: 'worktree/agent-2', verify: 'cargo test -p agentcowork-memory', iterations: { used: 0, total: 50 } },
  { id: 't3', title: 'Design review of planner', status: 'review', agent: 'reviewer', model: 'claude-opus', branch: 'worktree/reviewer', verify: 'oracle gate passed', progress: 90, iterations: { used: 12, total: 500 } },
  { id: 't4', title: 'Docs: architecture chapter', status: 'done', agent: 'writer-1', model: 'gpt-5', branch: 'worktree/agent-3', verify: 'markdown links resolve', iterations: { used: 8, total: 50 } },
]

/** Sort a card list for the column: by progress desc, then id. */
export function sortKanban(cards: KanbanCard[]): KanbanCard[] {
  return [...cards].sort((a, b) => {
    if ((a.progress ?? 0) !== (b.progress ?? 0)) return (b.progress ?? 0) - (a.progress ?? 0)
    return a.id.localeCompare(b.id)
  })
}

export default function KanbanView() {
  const [cards, setCards] = useState<KanbanCard[]>(() => (inTauri() ? [] : DEMO))
  const blueprint = useAppStore((s) => s.pendingPlan)

  // P11.5.10 — when a live plan exists (P6.3 plan executor), derive cards from
  // its tasks; otherwise the demo board renders so the view is explorable.
  useEffect(() => {
    if (!blueprint?.tasks?.length) {
      if (inTauri()) setCards([])
      return
    }
    const live: KanbanCard[] = blueprint.tasks.map((t, i) => ({
      id: t.id ?? `task-${i}`,
      title: t.goal ?? `Task ${i + 1}`,
      status: 'pending',
      agent: `agent-${i + 1}`,
      model: 'auto (router)',
      branch: `worktree/agent-${i + 1}`,
      verify: 'acceptance checks',
    }))
    setCards(live)
  }, [blueprint])

  const grouped = useMemo(() => {
    const map = new Map<KanbanStatus, KanbanCard[]>()
    for (const c of COLUMNS) map.set(c.status, [])
    for (const c of sortKanban(cards)) {
      map.get(c.status)?.push(c)
    }
    return map
  }, [cards])

  return (
    <div className="scroll-thin h-full overflow-x-auto p-3">
      <div className="mb-2 flex items-center gap-2">
        <GitBranch className="h-3.5 w-3.5 text-brand" />
        <span className="text-xs font-medium text-foreground">Parallel sub-agents</span>
        <Badge variant="secondary" className="text-[9px]">worktree-per-branch</Badge>
        <span className="ml-auto font-mono text-[10px] text-muted-foreground">{cards.length} tasks</span>
      </div>
      {inTauri() && cards.length === 0 ? (
        <div className="flex h-[calc(100%-28px)] items-center justify-center rounded-lg border border-dashed border-border px-6 text-center text-xs text-muted-foreground">
          No parallel work is active. Approved plan tasks will appear here when the live work gateway reports them.
        </div>
      ) : (
      <div className="flex h-[calc(100%-28px)] min-w-max gap-2.5">
        {COLUMNS.map((col) => {
          const colCards = grouped.get(col.status) ?? []
          return (
            <div key={col.status} className="flex w-56 shrink-0 flex-col rounded-lg border border-border/60 bg-background/30">
              <div className="flex items-center justify-between border-b border-border/50 px-2.5 py-1.5">
                <span className="text-[10px] font-medium text-foreground">{col.label}</span>
                <Badge variant="secondary" className="px-1.5 text-[9px]">{colCards.length}</Badge>
              </div>
              <div className="scroll-thin min-h-0 flex-1 space-y-1.5 overflow-y-auto p-2">
                {colCards.map((c) => (
                  <div key={c.id} className="rounded-md border border-border/60 bg-card/70 p-2">
                    <div className="text-[11px] font-medium leading-snug text-foreground">{c.title}</div>
                    <div className="mt-1 flex items-center gap-1 font-mono text-[9px] text-muted-foreground">
                      <GitBranch className="h-2.5 w-2.5" />
                      {c.branch}
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-1">
                      <Badge variant="secondary" className="px-1 text-[8px]">{c.agent}</Badge>
                      <Badge variant="secondary" className="px-1 text-[8px]">{c.model}</Badge>
                      {c.iterations && (
                        <Badge variant="secondary" className="px-1 text-[8px]">
                          {c.iterations.used}/{c.iterations.total} iters
                        </Badge>
                      )}
                    </div>
                    {c.progress !== undefined && (
                      <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-border/60">
                        <div
                          className={cn(
                            'h-full rounded-full',
                            c.status === 'done' ? 'bg-emerald-500' : 'bg-brand',
                          )}
                          style={{ width: `${c.progress}%` }}
                        />
                      </div>
                    )}
                    <div className="mt-1.5 flex items-start gap-1 text-[9px] text-muted-foreground/80">
                      <ShieldCheck className="mt-px size-2.5 shrink-0 text-emerald-400/70" />
                      <span className="line-clamp-1">{c.verify}</span>
                    </div>
                  </div>
                ))}
                {colCards.length === 0 && (
                  <div className="rounded border border-dashed border-border/40 px-2 py-3 text-center font-mono text-[9px] text-muted-foreground/50">
                    empty
                  </div>
                )}
              </div>
            </div>
          )
        })}
      </div>
      )}
    </div>
  )
}
