'use client'

// The one run surface: everything you need to inspect a run end to end, in a
// single right-rail lens —
//
//   identity → context & usage → ordered trace steps → files the run produced
//   → working folder → MCP servers
//
// This is an **aggregation projection**, not a replacement. Progress, Trajectory,
// Diff, Artifact, Tool output and Audit stay exactly where they were as
// drill-down lenses, and the links at the foot of this panel route into them
// through the store's own `addView` — the same entry point the "+ Add view"
// menu and the rail use. Nothing here owns run state: the Work journal, the
// usage ledger, the agent directory and the filesystem remain the owners
// (`ARCH/UI.md` §1–§3).
//
// Disclosure state is persisted (one preference), keyboard operable, and
// reduced-motion safe. Section bodies are conditional renders, so the only
// layout change is one the user asked for.

import { useMemo } from 'react'
import {
  ChevronDown,
  Database,
  FileStack,
  FolderOpen,
  GitCompare,
  Layers,
  ListChecks,
  Plug,
  ShieldCheck,
  Sparkles,
  SquareActivity,
  ScanSearch,
} from 'lucide-react'
import { useAppStore } from '@/lib/store'
import { usePref } from '@/lib/ui-prefs'
import { agentCardFromEvents } from '@/lib/agent-card'
import { RunHeader } from '@/components/views/run-header'
import { RunSection } from '@/components/views/run-section'
import { RunUsageSection } from '@/components/views/run-usage'
import { RunTrace } from '@/components/views/run-trace'
import { RunArtifacts, collectProducedFiles } from '@/components/views/run-artifacts'
import { RunFolderFiles, RunMcpServers } from '@/components/views/run-inventory'
import { RunOutcomeStrip } from '@/components/views/run-outcome'
import { buildRunTrace, countRunSteps } from '@/components/views/run-projection'
import { LiveDeskSummary } from '@/components/views/live-desk-summary'
import { hasPendingUserAction, projectLiveDeskUpdates } from '@/components/views/live-desk-activity'
import { cn } from '@/lib/utils'

type SectionId = 'outcome' | 'usage' | 'trace' | 'files' | 'folder' | 'mcp'

const ALL_SECTIONS: SectionId[] = ['outcome', 'usage', 'trace', 'files', 'folder', 'mcp']

export default function RunView() {
  const activeSessionId = useAppStore((s) => s.activeSessionId)
  const chat = useAppStore((s) => s.sessions.find((x) => x.id === s.activeSessionId))
  const taskFolder = useAppStore((s) => s.taskFolder)
  const workEvents = useAppStore((s) => s.workEvents)
  const addView = useAppStore((s) => s.addView)
  const spooledOutput = useAppStore((s) => s.spooledOutput)

  const [collapsed, setCollapsed] = usePref<SectionId[]>('runCollapsedSections', [])
  const card = useMemo(() => agentCardFromEvents(workEvents), [workEvents])
  const updates = useMemo(() => projectLiveDeskUpdates(workEvents), [workEvents])
  const hasPendingAction = useMemo(() => hasPendingUserAction(workEvents), [workEvents])
  const live = chat?.status === 'running'
  const workingDir = chat?.folder ?? taskFolder ?? null
  const steps = useMemo(
    () => buildRunTrace({ journal: workEvents, card, live }),
    [workEvents, card, live],
  )
  const counts = countRunSteps(steps)
  const artifacts = useMemo(
    () => (chat?.messages ?? []).flatMap((m) => m.artifacts ?? []),
    [chat],
  )
  const assistantArtifacts = useMemo(
    () => (chat?.messages ?? []).filter((message) => message.role === 'assistant').flatMap((message) => message.artifacts ?? []),
    [chat],
  )
  // The collapsed-section count is the de-duplicated reported-file count, the
  // same list the open section renders — never a raw sum that double counts.
  const reportedFiles = useMemo(
    () => collectProducedFiles({ artifacts, touchedFiles: card.files, workingDir }).length,
    [artifacts, card.files, workingDir],
  )
  const problems = counts.failed + counts.uncertain + (card.awaitingInput ? 1 : 0)
  const outcomeMeta = `${card.files.length} file${card.files.length === 1 ? '' : 's'} · ${
    card.tests.length
  } test${card.tests.length === 1 ? '' : 's'} · ${card.conflicts.length} conflict${
    card.conflicts.length === 1 ? '' : 's'
  }`

  const list = Array.isArray(collapsed) ? collapsed : []
  const isOpen = (id: SectionId) => !list.includes(id)
  const toggle = (id: SectionId) =>
    setCollapsed(list.includes(id) ? list.filter((x) => x !== id) : [...list, id])
  const allCollapsed = ALL_SECTIONS.every((id) => list.includes(id))
  /** `true` collapses every section, `false` opens every section. */
  const setAll = (nextCollapsed: boolean) =>
    setCollapsed(nextCollapsed ? [...ALL_SECTIONS] : [])

  return (
    <div
      data-testid="run-view"
      className="flex h-full min-h-0 w-full flex-col overflow-hidden bg-background"
    >
      <div className="min-h-0 flex-1 overflow-y-auto scroll-thin">
        <div className="space-y-3 p-3">
          <LiveDeskSummary
            key={`${activeSessionId}:${workEvents[0]?.workId ?? ''}`}
            title={chat?.title}
            sessionStatus={chat?.status}
            card={card}
            updates={updates}
            needsAttention={hasPendingAction}
            artifacts={assistantArtifacts}
          />

          <details className="group rounded-xl border border-border bg-card/50">
            <summary className="flex min-h-10 cursor-pointer list-none items-center justify-between gap-2 rounded-xl px-3 text-xs font-medium text-muted-foreground outline-none transition-colors hover:bg-accent/50 hover:text-foreground focus-visible:ring-2 focus-visible:ring-brand/60 [&::-webkit-details-marker]:hidden">
              <span>Activity details</span>
              <ChevronDown aria-hidden className="h-3.5 w-3.5 transition-transform group-open:rotate-180" />
            </summary>
            <div className="border-t border-border/70 px-1 pb-1">
              <RunHeader />
              <RunSection
                title="Outcome"
                icon={ListChecks}
                tone={!isOpen('outcome') && (card.conflicts.length > 0 || card.tests.some((t) => !t.passed)) ? 'alert' : 'default'}
                meta={isOpen('outcome') ? undefined : outcomeMeta}
                open={isOpen('outcome')}
                onToggle={() => toggle('outcome')}
              >
                <RunOutcomeStrip card={card} />
              </RunSection>

              <RunSection
                title="Context & usage"
                icon={Database}
                open={isOpen('usage')}
                onToggle={() => toggle('usage')}
              >
                <RunUsageSection chatId={activeSessionId || null} />
              </RunSection>

              <RunSection
                title="Steps"
                icon={SquareActivity}
                // The alert tint only ever appears beside the written counts in the
                // meta line, so it reinforces a word and never replaces one. Open,
                // the rows themselves carry the status in words.
                tone={!isOpen('trace') && problems > 0 ? 'alert' : 'default'}
                meta={isOpen('trace') ? undefined : traceMeta(counts, steps.length)}
                open={isOpen('trace')}
                onToggle={() => toggle('trace')}
              >
                <RunTrace steps={steps} />
              </RunSection>

              <RunSection
                title="Files"
                icon={FileStack}
                meta={isOpen('files') ? undefined : `${reportedFiles} reported`}
                open={isOpen('files')}
                onToggle={() => toggle('files')}
              >
                <RunArtifacts
                  workingDir={workingDir}
                  artifacts={artifacts}
                  touchedFiles={card.files}
                />
              </RunSection>

              <RunSection
                title="Working folder"
                icon={FolderOpen}
                meta={isOpen('folder') ? undefined : workingDir ?? 'none attached'}
                open={isOpen('folder')}
                onToggle={() => toggle('folder')}
              >
                <RunFolderFiles workingDir={workingDir} />
              </RunSection>

              <RunSection
                title="MCP servers"
                icon={Plug}
                open={isOpen('mcp')}
                onToggle={() => toggle('mcp')}
              >
                <RunMcpServers />
              </RunSection>

              {/* Existing technical drill-downs stay reachable, but no longer
                  crowd the everyday summary. */}
              <footer className="border-t border-border bg-card/40 px-3 py-2">
                <div className="mb-1.5 flex items-center justify-between gap-2">
                  <span className="flex items-center gap-1.5 font-mono text-[9px] uppercase tracking-wider text-muted-foreground/70">
                    <Layers aria-hidden className="h-3 w-3" />
                    More details
                  </span>
                  <button
                    type="button"
                    onClick={() => setAll(!allCollapsed)}
                    aria-pressed={allCollapsed}
                    className="inline-flex h-5 items-center gap-1 rounded-md border border-border px-1.5 font-mono text-[9px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
                  >
                    <ChevronDown
                      aria-hidden
                      className={cn('h-2.5 w-2.5 transition-transform', allCollapsed && '-rotate-90')}
                    />
                    {allCollapsed ? 'Expand all' : 'Collapse all'}
                  </button>
                </div>
                <div className="flex flex-wrap gap-1">
                  <DrillButton label="Progress" icon={SquareActivity} onClick={() => addView('progress')} />
                  <DrillButton label="Trace" icon={ScanSearch} onClick={() => addView('trajectory')} />
                  <DrillButton label="Changes" icon={GitCompare} onClick={() => addView('diff')} />
                  <DrillButton label="Audit" icon={ShieldCheck} onClick={() => addView('audit')} />
                  {spooledOutput ? (
                    <DrillButton label="Tool output" icon={Sparkles} onClick={() => addView('tool-output')} />
                  ) : null}
                </div>
              </footer>
            </div>
          </details>
        </div>
      </div>
    </div>
  )
}

function DrillButton({
  label,
  icon: Icon,
  onClick,
}: {
  label: string
  icon: React.ElementType
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="inline-flex h-6 items-center gap-1 rounded-md border border-border px-1.5 font-mono text-[9px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
    >
      <Icon aria-hidden className="h-2.5 w-2.5" />
      {label}
    </button>
  )
}

function traceMeta(counts: Record<string, number>, total: number): string {
  if (total === 0) return 'empty'
  const parts: string[] = [`${total} steps`]
  if (counts.running) parts.push(`${counts.running} running`)
  if (counts.waiting) parts.push(`${counts.waiting} waiting`)
  if (counts.failed) parts.push(`${counts.failed} failed`)
  if (counts.uncertain) parts.push(`${counts.uncertain} unknown`)
  return parts.join(' · ')
}
