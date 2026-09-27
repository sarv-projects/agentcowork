'use client'

import { useEffect, useMemo, useState } from 'react'
import { CheckCircle2, Circle, Folder, Search, Sparkles } from 'lucide-react'
import ChatComposer from '@/components/chat/chat-composer'
import { fuzzyRank } from '@/lib/fuzzy'
import { useAppStore, type Session } from '@/lib/store'
import { cn } from '@/lib/utils'
import { useChatColumnClass } from '@/lib/layout'
import {
  FIRST_TASKS,
  localStorageFirstRunStorage,
  markFirstSeen,
  shouldNudgeFirstTask,
} from '@/lib/first-run'

function greeting() {
  const h = new Date().getHours()
  if (h < 12) return 'Good morning'
  if (h < 18) return 'Good afternoon'
  return 'Good evening'
}

function statusLine(s: Session) {
  if (s.status === 'action-required') return 'Waiting for approval'
  if (s.status === 'running') return 'Running'
  if (s.status === 'completed') return 'Completed'
  if (s.status === 'scheduled') return s.preview || 'Scheduled'
  if (s.status === 'paused') return 'Paused'
  if (s.status === 'failed') return 'Failed'
  if (s.status === 'cancelled') return 'Cancelled'
  if (s.status === 'budget_exceeded') return 'Budget limit reached'
  return s.preview
}

export default function HomeLaunchpad() {
  const sessions = useAppStore((s) => s.sessions)
  const setActiveSession = useAppStore((s) => s.setActiveSession)
  const setComposerValue = useAppStore((s) => s.setComposerValue)
  const continueWork = sessions.slice(0, 4)
  const col = useChatColumnClass()

  // P32.10 / WP2 — kill the blank canvas. With no work yet, the starters are
  // shown as scoped task cards (what will happen, in plain words); once the
  // user has work, they collapse back to compact pills.
  const noWorkYet = continueWork.length === 0
  const [nudge, setNudge] = useState(false)

  // The 24-hour nudge fires at most once, and only for someone who has never
  // started a task. Evaluated once on mount (the delay dwarfs any load time).
  useEffect(() => {
    const store = localStorageFirstRunStorage
    markFirstSeen(store)
    if (shouldNudgeFirstTask(store, Date.now(), useAppStore.getState().sessions.length > 0)) {
      setNudge(true)
    }
  }, [])

  const startTask = (prompt: string) => setComposerValue(prompt)

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center px-6">
        <div className="mb-1 text-xs text-muted-foreground">{greeting()}.</div>
        <h1 className="mb-4 text-lg font-semibold tracking-tight">What would you like to get done?</h1>
        <div className={col}>
          <ChatComposer centered />
        </div>
        {noWorkYet ? (
          <div className={cn(col, 'mt-4')}>
            <div className="mb-2 text-center text-[11px] text-muted-foreground">
              Start here — pick one and I&apos;ll take it from there.
            </div>
            <div className="grid gap-2 text-left sm:grid-cols-2">
              {FIRST_TASKS.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  onClick={() => startTask(t.prompt)}
                  className="rounded-lg border border-border bg-card/40 p-3 transition-colors hover:border-brand/40 hover:bg-accent/40"
                >
                  <span className="text-base leading-none">{t.emoji}</span>
                  <span className="mt-1.5 block text-[12px] font-medium text-foreground">
                    {t.label}
                  </span>
                  <span className="mt-0.5 block text-[10px] leading-snug text-muted-foreground">
                    {t.detail}
                  </span>
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div className={cn(col, 'mt-3 flex flex-wrap justify-center gap-1.5')}>
            {FIRST_TASKS.map((t) => (
              <button
                key={t.id}
                type="button"
                onClick={() => startTask(t.prompt)}
                className="rounded-full border border-border bg-card/40 px-2.5 py-1 text-[11px] text-muted-foreground hover:border-brand/40 hover:text-foreground"
              >
                {t.emoji} {t.label}
              </button>
            ))}
          </div>
        )}

        {/* P32.10 — one-shot nudge after a day with nothing started. */}
        {nudge && (
          <div className={cn(col, 'mt-4')}>
            <div className="flex items-start gap-2 rounded-lg border border-brand/40 bg-brand/5 px-3 py-2">
              <Sparkles className="mt-0.5 h-3.5 w-3.5 shrink-0 text-brand" />
              <div className="min-w-0 flex-1">
                <p className="text-[12px] text-foreground">
                  Nothing has run yet — want me to start with this?
                </p>
                <p className="mt-0.5 text-[10px] leading-snug text-muted-foreground">
                  {FIRST_TASKS[0]!.detail}
                </p>
              </div>
              <button
                type="button"
                onClick={() => {
                  startTask(FIRST_TASKS[0]!.prompt)
                  setNudge(false)
                }}
                className="shrink-0 rounded-md bg-brand px-2 py-1 text-[10px] font-medium text-black hover:bg-brand"
              >
                Start
              </button>
              <button
                type="button"
                onClick={() => setNudge(false)}
                title="Not now"
                className="shrink-0 rounded p-1 text-muted-foreground hover:text-foreground"
              >
                ✕
              </button>
            </div>
          </div>
        )}

        {continueWork.length > 0 && (
          <div className={cn(col, 'mt-6')}>
            <div className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/70">
              Continue working
            </div>
            <ul className="space-y-1">
              {continueWork.map((s) => (
                <li key={s.id}>
                  <button
                    type="button"
                    onClick={() => setActiveSession(s.id)}
                    className="flex w-full items-start gap-2.5 rounded-md border border-transparent px-2 py-2 text-left hover:border-border hover:bg-accent/40"
                  >
                    {s.status === 'completed' ? (
                      <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 text-emerald-400" />
                    ) : (
                      <Circle
                        className={cn(
                          'mt-0.5 h-3.5 w-3.5',
                          s.status === 'action-required' && 'text-brand',
                          s.status === 'running' && 'text-blue-400',
                          s.status === 'scheduled' && 'text-violet-400',
                          s.status === 'cancelled' && 'text-zinc-400',
                          s.status === 'budget_exceeded' && 'text-warning',
                        )}
                      />
                    )}
                    <span className="min-w-0">
                      <span className="block text-[13px] text-foreground">{s.title}</span>
                      <span className="block text-[11px] text-muted-foreground">{statusLine(s)}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  )
}

export function ActivityPanel() {
  const sessions = useAppStore((s) => s.sessions)
  const setActiveSession = useAppStore((s) => s.setActiveSession)
  const live = sessions.filter((s) => s.status === 'running' || s.status === 'action-required' || s.status === 'scheduled')
  const done = sessions.filter((s) => s.status === 'completed' || s.status === 'failed' || s.status === 'cancelled' || s.status === 'budget_exceeded' || s.status === 'paused')
  return (
    <div className="flex h-full w-full flex-col">
      <header className="border-b border-border px-4 py-3">
        <h2 className="text-sm font-semibold">Activity</h2>
        <p className="text-[11px] text-muted-foreground">Everything currently happening or recently completed.</p>
      </header>
      <div className="scroll-thin min-h-0 flex-1 overflow-y-auto p-4">
        <Section title="Now" items={live} onPick={setActiveSession} />
        <Section title="Recently finished" items={done} onPick={setActiveSession} />
      </div>
    </div>
  )
}

function Section({ title, items, onPick }: { title: string; items: Session[]; onPick: (id: string) => void }) {
  return (
    <section className="mb-6">
      <div className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/70">{title}</div>
      {items.length === 0 ? (
        <p className="text-[11px] text-muted-foreground">Nothing here.</p>
      ) : (
        <ul className="space-y-1">
          {items.map((s) => (
            <li key={s.id}>
              <button
                type="button"
                onClick={() => onPick(s.id)}
                className="flex w-full items-start gap-2 rounded-md border border-border/50 bg-background/30 px-3 py-2 text-left hover:border-brand/30"
              >
                <Sparkles className="mt-0.5 h-3.5 w-3.5 text-brand" />
                <span>
                  <span className="block text-[13px]">{s.title}</span>
                  <span className="block text-[11px] text-muted-foreground">{statusLine(s)}</span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

export function ProjectsPanel() {
  const sessions = useAppStore((s) => s.sessions)
  const setActiveSession = useAppStore((s) => s.setActiveSession)
  // P52.15 (UI slice) — fuzzy search across project paths and their work
  // items; folders sort pinned-first, then by most-recent work. Archive/
  // History + trash delete remain gated on the session-trash command.
  const [query, setQuery] = useState('')
  const folders = useMemo(() => {
    const all = Array.from(new Set(sessions.map((s) => s.folder).filter(Boolean))) as string[]
    const withRecency = all.map((f) => {
      const inFolder = sessions.filter((s) => s.folder === f)
      const newest = inFolder.reduce(
        (acc, s) => (s.updatedAt > acc ? s.updatedAt : acc),
        inFolder[0]?.updatedAt ?? '',
      )
      return { f, inFolder, newest }
    })
    withRecency.sort((a, b) => b.newest.localeCompare(a.newest))
    if (!query.trim()) return withRecency
    const q = query.trim()
    return fuzzyRank(
      q,
      withRecency,
      (x) => `${x.f} ${x.inFolder.map((s) => s.title).join(' ')}`,
    )
  }, [sessions, query])
  return (
    <div className="flex h-full w-full flex-col">
      <header className="border-b border-border px-4 py-3">
        <h2 className="text-sm font-semibold">Projects</h2>
        <p className="text-[11px] text-muted-foreground">Persistent bodies of work — folders AgentCowork has used.</p>
      </header>
      <div className="border-b border-border px-4 pb-2">
        <div className="flex items-center gap-2 rounded-lg border border-border/60 bg-card/40 px-2.5 py-1.5">
          <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search projects and work items…"
            className="h-6 flex-1 bg-transparent text-[12px] text-foreground placeholder:text-muted-foreground/60 focus:outline-none"
          />
          {query && (
            <button
              type="button"
              onClick={() => setQuery('')}
              className="text-[10px] text-muted-foreground hover:text-foreground"
            >
              clear
            </button>
          )}
        </div>
      </div>
      <div className="scroll-thin min-h-0 flex-1 overflow-y-auto p-4">
        {folders.length === 0 ? (
          <p className="text-[11px] text-muted-foreground">
            {query.trim() ? `No projects match “${query}”.` : 'No projects yet. Start work from Home.'}
          </p>
        ) : (
          <ul className="space-y-1">
            {folders.map(({ f, inFolder }) => (
              <li key={f}>
                <button
                  type="button"
                  onClick={() => inFolder[0] && setActiveSession(inFolder[0].id)}
                  className="flex w-full items-center gap-2 rounded-md border border-border/50 bg-background/30 px-3 py-2 text-left hover:border-brand/30"
                >
                  <Folder className="h-4 w-4 shrink-0 text-brand" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-mono text-[12px]">{f}</span>
                    <span className="text-[10px] text-muted-foreground">{inFolder.length} work item{inFolder.length === 1 ? '' : 's'}</span>
                  </span>
                  <span className="shrink-0 text-[9px] text-muted-foreground/50">
                    {inFolder.slice(0, 2).map((s) => s.title).join(' · ').slice(0, 40)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  )
}
