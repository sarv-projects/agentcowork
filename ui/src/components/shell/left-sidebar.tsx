'use client'

import * as React from 'react'
import {
  CheckCircle2,
  ChevronDown,
  Circle,
  Clock,
  Cog,
  FileText,
  Folder,
  HelpCircle,
  Home,
  Plus,
  Search,
  Sparkles,
  Star,
  Zap,
  AlertCircle,
  Pause,
  RefreshCw,
  X,
} from 'lucide-react'
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { useAppStore, type Session, type SessionStatus } from '@/lib/store'
import { AGENT_MAP } from '@/lib/agents'
import { acpIdFor, currentBinding } from '@/lib/acp'
import { staggerStyle } from '@/lib/stagger'
import { cn } from '@/lib/utils'

const statusMeta: Record<
  SessionStatus,
  { color: string; ring: string; Icon: typeof Circle; label: string }
> = {
  idle: { color: 'text-zinc-500', ring: 'bg-zinc-500', Icon: Circle, label: 'Idle' },
  running: { color: 'text-blue-400', ring: 'bg-blue-500', Icon: Circle, label: 'Running' },
  'action-required': { color: 'text-brand', ring: 'bg-brand', Icon: AlertCircle, label: 'Waiting for approval' },
  completed: { color: 'text-emerald-400', ring: 'bg-emerald-500', Icon: CheckCircle2, label: 'Completed' },
  failed: { color: 'text-red-400', ring: 'bg-red-500', Icon: AlertCircle, label: 'Failed' },
  cancelled: { color: 'text-zinc-400', ring: 'bg-zinc-400', Icon: Circle, label: 'Cancelled' },
  budget_exceeded: { color: 'text-warning', ring: 'bg-warning', Icon: AlertCircle, label: 'Budget reached' },
  paused: { color: 'text-zinc-400', ring: 'bg-zinc-400', Icon: Pause, label: 'Paused' },
  scheduled: { color: 'text-violet-400', ring: 'bg-violet-500', Icon: Star, label: 'Scheduled' },
  reconnecting: { color: 'text-warning', ring: 'bg-warning', Icon: RefreshCw, label: 'Reconnecting' },
}

export type AttentionBucket = 'needs-attention' | 'running' | 'recent'

export type AttentionSession = Pick<Session, 'status' | 'updatedAt' | 'title' | 'preview' | 'agent' | 'folder'>

export interface AttentionGroup<T extends AttentionSession> {
  id: AttentionBucket
  label: string
  sessions: T[]
}

/** The one ordering used by the sidebar and its DOM tests. */
export const ATTENTION_GROUPS: ReadonlyArray<{ id: AttentionBucket; label: string }> = [
  { id: 'needs-attention', label: 'Needs attention' },
  { id: 'running', label: 'Running' },
  { id: 'recent', label: 'Recent / idle' },
]

export function attentionBucketForSession(session: Pick<Session, 'status'>): AttentionBucket {
  if (session.status === 'action-required' || session.status === 'failed' || session.status === 'budget_exceeded' || session.status === 'reconnecting') {
    return 'needs-attention'
  }
  if (session.status === 'running') return 'running'
  return 'recent'
}

export function groupSessionsByAttention<T extends AttentionSession>(sessions: T[]): AttentionGroup<T>[] {
  const order = new Map(ATTENTION_GROUPS.map((group, index) => [group.id, index]))
  return ATTENTION_GROUPS.map((group) => ({
    ...group,
    sessions: sessions
      .filter((session) => attentionBucketForSession(session) === group.id)
      .sort((a, b) => (b.updatedAt ?? '').localeCompare(a.updatedAt ?? '')),
  })).sort((a, b) => (order.get(a.id) ?? 0) - (order.get(b.id) ?? 0))
}

export function sessionOwnerLabel(session: Pick<Session, 'agent'>): string {
  if (!session.agent) return 'You'
  return AGENT_MAP[session.agent]?.name ?? session.agent
}

export function sessionReasonLabel(session: Pick<Session, 'status' | 'preview'>): string {
  if (session.status === 'action-required') return 'Waiting for your approval'
  if (session.status === 'running') return 'Working now'
  if (session.status === 'failed') return 'Needs a retry'
  if (session.status === 'budget_exceeded') return 'Budget limit reached'
  if (session.status === 'reconnecting') return 'Reconnecting'
  if (session.status === 'scheduled') return 'Scheduled'
  if (session.status === 'completed') return 'Completed'
  if (session.status === 'paused') return 'Paused'
  return session.preview?.trim() || 'Ready when you are'
}

export function sessionNextAction(session: Pick<Session, 'status'>): { label: string; aria: string } {
  if (session.status === 'action-required') return { label: 'Review', aria: 'Review this item' }
  if (session.status === 'failed' || session.status === 'reconnecting') return { label: 'Retry', aria: 'Retry this item' }
  if (session.status === 'budget_exceeded') return { label: 'Review', aria: 'Review this item' }
  if (session.status === 'running') return { label: 'Watch', aria: 'Watch this item' }
  return { label: 'Open', aria: 'Open this item' }
}

function NavItem({
  icon: Icon,
  label,
  active,
  badge,
  collapsed,
  onClick,
}: {
  icon: React.ElementType
  label: string
  active?: boolean
  badge?: string
  collapsed?: boolean
  onClick?: () => void
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onClick}
          className={cn(
            'group relative flex w-full items-center gap-2 rounded-md text-[12.5px] transition-colors',
            collapsed ? 'mx-auto h-9 w-9 justify-center' : 'h-8 px-2',
            active
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground',
          )}
        >
          <Icon className={cn('h-4 w-4 shrink-0', active && 'text-brand')} />
          {!collapsed && <span className="flex-1 truncate text-left">{label}</span>}
          {!collapsed && badge && (
            <span className="font-mono text-[10px] text-muted-foreground/70">{badge}</span>
          )}
          {active && !collapsed && (
            <span className="absolute bottom-1.5 left-0 top-1.5 w-0.5 rounded-r bg-brand" />
          )}
        </button>
      </TooltipTrigger>
      <TooltipContent side="right" sideOffset={8}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

function Label({ children, collapsed }: { children: React.ReactNode; collapsed?: boolean }) {
  if (collapsed) return null
  return (
    <div className="px-2 pb-1 pt-3 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/70">
      {children}
    </div>
  )
}

function WorkRow({
  session,
  collapsed,
  active,
  depth = 0,
  onNavigate,
}: {
  session: Session
  collapsed?: boolean
  active?: boolean
  depth?: number
  onNavigate?: () => void
}) {
  const setActiveSession = useAppStore((s) => s.setActiveSession)
  const selectedAgentId = useAppStore((s) => s.selectedAgentId)
  const userDefaultChief = useAppStore((s) => s.userDefaultChief)
  const sessionChiefs = useAppStore((s) => s.sessionChiefs)
  const liveAgents = useAppStore((s) => s.liveAgents)
  const meta = statusMeta[session.status]
  const Icon = session.pinned ? Star : meta.Icon
  const action = sessionNextAction(session)
  const boundId = currentBinding(session.chiefPin)
    ?? currentBinding(sessionChiefs[session.id])
    ?? currentBinding(userDefaultChief)
    ?? currentBinding(selectedAgentId)
  const boundLive = boundId
    ? liveAgents.find((agent) => acpIdFor(agent.id) === acpIdFor(boundId))
    : undefined
  const owner = boundLive?.name
    ?? (boundId ? AGENT_MAP[acpIdFor(boundId)]?.name ?? boundId : sessionOwnerLabel(session))
  const reason = sessionReasonLabel(session)
  const project = session.folder?.split(/[\\/]/).filter(Boolean).pop()

  const open = () => {
    setActiveSession(session.id)
    onNavigate?.()
  }

  if (collapsed) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={open}
            aria-label={`${session.title} — ${reason}`}
            className={cn(
              'relative mx-auto grid h-8 w-8 place-items-center rounded-md transition-colors',
              active ? 'bg-accent' : 'hover:bg-accent/50',
            )}
          >
            <Icon className={cn('h-4 w-4', meta.color)} />
            <span className={cn('absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full ring-2 ring-sidebar', meta.ring)} />
          </button>
        </TooltipTrigger>
        <TooltipContent side="right">
          <span>{session.title}</span>
          <span className="mt-0.5 block text-[10px] text-muted-foreground">{owner} · {action.label}</span>
        </TooltipContent>
      </Tooltip>
    )
  }

  return (
    <div
      className={cn(
        'group relative flex min-w-0 items-center rounded-md transition-colors',
        active ? 'bg-accent' : 'hover:bg-accent/50',
      )}
      style={depth > 0 ? { paddingLeft: depth * 14 } : undefined}
      data-session-id={session.id}
      data-attention-group={attentionBucketForSession(session)}
    >
      <button
        type="button"
        onClick={open}
        aria-label={`${session.title} — ${reason}. Next: ${action.label}`}
        className="min-w-0 flex-1 px-2 py-1.5 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-brand/60"
      >
        <span className="flex items-start gap-2">
          {depth > 0 && (
            <span aria-hidden="true" className="mt-1.5 h-2 w-2 shrink-0 rounded-[2px] border border-muted-foreground/40" />
          )}
          <Icon className={cn('mt-0.5 h-3.5 w-3.5 shrink-0', meta.color)} />
          <span className="min-w-0 flex-1">
            <span className="flex min-w-0 items-center gap-1.5">
              <span className="min-w-0 flex-1 truncate text-[12.5px] text-foreground">{session.title}</span>
              {project && (
                <span className="max-w-[5.5rem] shrink-0 truncate font-mono text-[9px] text-muted-foreground/60" title={session.folder}>
                  {project}
                </span>
              )}
            </span>
            <span className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[10px] text-muted-foreground">
              <span className="shrink-0 font-medium text-foreground/75">{owner}</span>
              <span aria-hidden="true">·</span>
              <span className="min-w-0 truncate">{reason}</span>
            </span>
          </span>
        </span>
      </button>
      <button
        type="button"
        onClick={open}
        aria-label={`${action.aria}: ${session.title}`}
        className="mr-1 shrink-0 rounded px-1.5 py-1 text-[10px] font-medium text-muted-foreground opacity-60 transition-opacity hover:bg-brand/10 hover:text-brand focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 group-hover:opacity-100"
      >
        {action.label}
      </button>
    </div>
  )
}

/**
 * P51.13 — per-project thread rows. Groups top-level sessions by their
 * `taskFolder` (projects), most-recent-first; forks (`parentId`) are left to
 * their parent's group. Pure so the grouping contract is testable.
 */
export function groupSessionsByFolder<T extends { id: string; parentId?: string | null; taskFolder?: string; updatedAt?: string }>(
  sessions: T[],
): Array<{ folder: string; sessions: T[] }> {
  const tops = sessions.filter((s) => !s.parentId)
  const byFolder = new Map<string, typeof sessions>()
  for (const s of tops) {
    const folder = s.taskFolder?.trim() || '(no project)'
    const list = byFolder.get(folder) ?? []
    list.push(s)
    byFolder.set(folder, list)
  }
  const sortNewest = (a: { updatedAt?: string }, b: { updatedAt?: string }) =>
    (b.updatedAt ?? '').localeCompare(a.updatedAt ?? '')
  return [...byFolder.entries()]
    .map(([folder, list]) => ({ folder, sessions: [...list].sort(sortNewest) }))
    .sort((a, b) => sortNewest(a.sessions[0], b.sessions[0]))
}

export interface LeftSidebarProps {
  /** Render the navigation as an accessible overlay instead of a flex column. */
  narrow?: boolean
  /** Controlled overlay state used by the app shell. */
  drawerOpen?: boolean
  onDrawerOpenChange?: (open: boolean) => void
}

export function LeftSidebar({
  narrow = false,
  drawerOpen = false,
  onDrawerOpenChange,
}: LeftSidebarProps = {}) {
  const storeCollapsed = useAppStore((s) => s.sidebarCollapsed)
  const sessions = useAppStore((s) => s.sessions)
  const activeId = useAppStore((s) => s.activeSessionId)
  const newSession = useAppStore((s) => s.newSession)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const centerScreen = useAppStore((s) => s.centerScreen)
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen)
  const setSettingsSection = useAppStore((s) => s.setSettingsSection)
  const automations = sessions.filter((s) => s.status === 'scheduled').length
  const drawerActive = narrow && drawerOpen
  // A drawer is a full navigation surface even when the desktop icon rail is
  // collapsed. Keeping that distinction local avoids squeezing labels into a
  // 48px overlay.
  const collapsed = drawerActive ? false : storeCollapsed
  const drawerRef = React.useRef<HTMLElement | null>(null)
  const restoreFocusRef = React.useRef<HTMLElement | null>(null)

  const closeDrawer = React.useCallback(() => {
    if (drawerActive) onDrawerOpenChange?.(false)
  }, [drawerActive, onDrawerOpenChange])

  React.useEffect(() => {
    if (!drawerActive) return
    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const focusFirst = () => {
      const first = drawerRef.current?.querySelector<HTMLElement>(
        '[data-sidebar-close],button:not([disabled]),input:not([disabled]),[href],[tabindex]:not([tabindex="-1"])',
      )
      first?.focus()
    }
    focusFirst()
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        closeDrawer()
        return
      }
      if (event.key !== 'Tab' || !drawerRef.current) return
      const focusable = Array.from(
        drawerRef.current.querySelectorAll<HTMLElement>(
          'button:not([disabled]),input:not([disabled]),[href],[tabindex]:not([tabindex="-1"])',
        ),
      ).filter((element) => !element.hasAttribute('data-sidebar-backdrop'))
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    document.addEventListener('keydown', onKeyDown, true)
    return () => {
      document.removeEventListener('keydown', onKeyDown, true)
      restoreFocusRef.current?.focus()
    }
  }, [drawerActive, closeDrawer])

  const navigate = () => {
    closeDrawer()
  }

  const [query, setQuery] = React.useState('')
  const recent = React.useMemo(() => {
    const list = query
      ? sessions.filter((s) => s.title.toLowerCase().includes(query.toLowerCase()))
      : sessions
    // Never let a long idle history push an approval or running item out of
    // the first viewport. The cap is applied after attention-first ordering.
    const priority = list.filter((s) => attentionBucketForSession(s) !== 'recent')
    const background = list.filter((s) => attentionBucketForSession(s) === 'recent')
    return [...priority, ...background].slice(0, 8)
  }, [sessions, query])

  const groupedRecent = React.useMemo(
    () => recent.filter((s) => !s.parentId || !recent.some((parent) => parent.id === s.parentId)),
    [recent],
  )

  if (narrow && !drawerActive) return null

  return (
    <>
      {drawerActive && (
        <button
          type="button"
          data-sidebar-backdrop
          aria-label="Close work navigation"
          tabIndex={-1}
          onClick={closeDrawer}
          className="fixed inset-0 z-40 cursor-default bg-black/35 backdrop-blur-[1px]"
        />
      )}
      <aside
        ref={drawerRef}
        data-sidebar-mode={drawerActive ? 'overlay' : 'inline'}
        aria-label={drawerActive ? undefined : 'Work navigation'}
        role={drawerActive ? 'dialog' : undefined}
        aria-modal={drawerActive ? true : undefined}
        aria-labelledby={drawerActive ? 'work-navigation-title' : undefined}
        tabIndex={drawerActive ? -1 : undefined}
        className={cn(
          'flex min-w-0 flex-col border-r border-border bg-sidebar no-select transition-[width] duration-300 ease-[cubic-bezier(0.4,0,0.2,1)]',
          !drawerActive && 'shrink-0',
          drawerActive
            ? 'fixed inset-y-0 left-0 z-50 w-[min(20rem,calc(100vw-2rem))] shadow-2xl'
            : collapsed
              ? 'w-12'
              : 'w-60',
        )}
      >
      {drawerActive && (
        <>
          <h2 id="work-navigation-title" className="sr-only">Work navigation</h2>
          <button
            type="button"
            data-sidebar-close
            aria-label="Close work navigation"
            onClick={closeDrawer}
            className="absolute right-2 top-2 z-10 grid h-7 w-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
          >
            <X aria-hidden className="h-4 w-4" />
          </button>
        </>
      )}
      <div className={cn('border-b border-border p-2', collapsed && 'px-1')}>
        {collapsed ? (
          <div className="mx-auto grid h-8 w-8 place-items-center rounded-md bg-brand/15 ring-1 ring-brand/30">
            <Sparkles className="h-4 w-4 text-brand" />
          </div>
        ) : (
          <button
            type="button"
            onClick={() => { setCenterScreen('home'); navigate() }}
            className="flex h-8 w-full items-center gap-2 rounded-md px-2 hover:bg-accent"
            aria-label="Go home"
          >
            <div className="grid h-5 w-5 place-items-center rounded-md bg-brand/15 ring-1 ring-brand/30">
              <Sparkles className="h-3 w-3 text-brand" />
            </div>
            <span className="flex-1 text-left text-[12.5px] font-semibold">AgentCowork</span>
            <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
          </button>
        )}
      </div>

      <div className={cn('space-y-1.5 border-b border-border p-2', collapsed && 'px-1')}>
        {collapsed ? (
          <>
            <Tooltip>
              <TooltipTrigger asChild>
                <button type="button" onClick={() => setPaletteOpen(true)} className="mx-auto grid h-8 w-8 place-items-center rounded-md hover:bg-accent">
                  <Search className="h-4 w-4" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">Search (⌘K)</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  onClick={() => { newSession(); navigate() }}
                  className="mx-auto grid h-8 w-8 place-items-center rounded-md bg-brand/15 ring-1 ring-brand/30 hover:bg-brand/25"
                >
                  <Plus className="h-4 w-4 text-brand" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">New work (⌘N)</TooltipContent>
            </Tooltip>
          </>
        ) : (
          <>
            <div className="relative">
              <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') { setPaletteOpen(true); navigate() }
                }}
                placeholder="Filter work (Enter for ⌘K)"
                title="Type to filter the list below · Enter opens full search"
                className="h-7 w-full rounded-md border border-border bg-background/60 pl-7 pr-2 text-[12px] placeholder:text-muted-foreground/60 focus:outline-none focus:ring-1 focus:ring-brand/40"
              />
            </div>
            <button
              type="button"
              onClick={() => { newSession(); navigate() }}
              className="flex h-7 w-full items-center justify-center gap-1.5 rounded-md bg-brand text-[12px] text-white hover:bg-brand-hover"
            >
              <Plus className="h-3.5 w-3.5" /> New work
            </button>
          </>
        )}
      </div>

      <nav className="border-b border-border p-2">
        <Label collapsed={collapsed}>Home</Label>
        <NavItem
          icon={Home}
          label="Home"
          collapsed={collapsed}
          active={centerScreen === 'home'}
          onClick={() => { setCenterScreen('home'); navigate() }}
        />
        <NavItem
          icon={Clock}
          label="Activity"
          collapsed={collapsed}
          active={centerScreen === 'activity'}
          onClick={() => { setCenterScreen('activity'); navigate() }}
        />
        <Label collapsed={collapsed}>Work</Label>
        <NavItem
          icon={Folder}
          label="Projects"
          collapsed={collapsed}
          active={centerScreen === 'projects'}
          onClick={() => { setCenterScreen('projects'); navigate() }}
        />
        <NavItem
          icon={FileText}
          label="Files"
          collapsed={collapsed}
          active={centerScreen === 'files'}
          onClick={() => { setCenterScreen('files'); navigate() }}
        />
        <NavItem
          icon={Zap}
          label="Automations"
          collapsed={collapsed}
          badge={automations ? String(automations) : undefined}
          active={centerScreen === 'automations'}
          onClick={() => { setCenterScreen('automations'); navigate() }}
        />
      </nav>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <div className="flex items-center justify-between px-3 py-2">
          {!collapsed && (
            <span className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/70">
              Recent
            </span>
          )}
        </div>
        {/* P45.6 — content-visibility: auto skips layout/paint of off-screen
            session rows in long lists. */}
        <div className="scroll-thin min-h-0 flex-1 space-y-0.5 overflow-y-auto px-1 pb-2 [content-visibility:auto] [contain-intrinsic-size:auto_48px]">
          {/* P51.13 — per-project thread rows: top-level sessions group under
              their taskFolder; forks stay indented under their parent inside
              the parent's group. Unfoldered rows sit in an implicit group. */}
          {collapsed
            ? recent.map((s, i) => (
                <div key={s.id} className="enter-stagger" style={staggerStyle(i)}>
                  <WorkRow
                    session={s}
                    collapsed={collapsed}
                    active={activeId === s.id}
                    depth={s.parentId ? 1 : 0}
                    onNavigate={navigate}
                  />
                </div>
              ))
            : groupSessionsByAttention(groupedRecent)
                .filter((group) => group.sessions.length > 0)
                .map((group, groupIndex) => (
                  <section key={group.id} className="enter-stagger" data-sidebar-group={group.id}>
                    <div className="flex items-center gap-1.5 px-2 pb-0.5 pt-2">
                      {group.id === 'needs-attention' ? (
                        <AlertCircle className="h-3 w-3 shrink-0 text-brand" />
                      ) : group.id === 'running' ? (
                        <RefreshCw className="h-3 w-3 shrink-0 text-info" />
                      ) : (
                        <Clock className="h-3 w-3 shrink-0 text-muted-foreground/60" />
                      )}
                      <span className="truncate text-[9px] font-semibold uppercase tracking-wider text-muted-foreground/70">
                        {group.label}
                      </span>
                      <span className="ml-auto font-mono text-[9px] text-muted-foreground/50">
                        {group.sessions.length}
                      </span>
                    </div>
                    {group.sessions.map((s, j) => (
                      <div key={s.id} style={staggerStyle(groupIndex * 4 + j)}>
                        <WorkRow
                          session={s}
                          active={activeId === s.id}
                          onNavigate={navigate}
                        />
                        {recent
                          .filter((c) => c.parentId === s.id)
                          .map((c) => (
                            <WorkRow
                              key={c.id}
                              session={c}
                              active={activeId === c.id}
                              depth={1}
                              onNavigate={navigate}
                            />
                          ))}
                      </div>
                    ))}
                  </section>
                ))}
          {/* P50.2.1 — an empty vault renders an honest empty state, never a
              blank pane that reads as still loading. */}
          {recent.length === 0 && !collapsed && (
            <div className="px-3 py-6 text-center">
              <p className="text-[11px] text-muted-foreground">No work yet</p>
              <p className="mt-1 font-mono text-[10px] text-muted-foreground/60">
                New work above — your first message opens it
              </p>
            </div>
          )}
        </div>
      </div>

      <div className={cn('flex flex-col gap-0.5 border-t border-border p-2', collapsed && 'items-center')}>
        <NavItem
          icon={Cog}
          label="Settings"
          collapsed={collapsed}
          active={centerScreen === 'settings'}
          onClick={() => { setCenterScreen('settings'); navigate() }}
        />
        <NavItem
          icon={HelpCircle}
          label="Help"
          collapsed={collapsed}
          onClick={() => { window.open('https://github.com/sarv-projects/agentcowork', '_blank', 'noopener'); navigate() }}
        />
        {!collapsed && (
          <button
            type="button"
            onClick={() => {
              setSettingsSection('general')
              setCenterScreen('settings')
              navigate()
            }}
            className="mt-1 flex items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent"
            title="Local profile — opens general settings"
          >
            <span className="grid h-6 w-6 place-items-center rounded-full bg-zinc-700 text-[10px] font-medium">○</span>
            <span className="min-w-0 flex-1 truncate text-[12px]">Local profile</span>
            <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
          </button>
        )}
      </div>
      </aside>
    </>
  )
}
