'use client'

import * as React from 'react'
import {
  Activity,
  Command,
  ChevronDown,
  CircleDot,
  Cog,
  Download,
  HelpCircle,
  Plug,
  Plus,
  Search,
  ShieldCheck,
  Sparkles,
  Brain,
  BarChart3,
  Clock,
  Pin,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeft,
  PanelRight,
  Sun,
  Moon,
  Settings,
} from 'lucide-react'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { Badge } from '@/components/ui/badge'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { useAppStore } from '@/lib/store'
import { useTheme } from '@/components/theme-provider'
import { cn } from '@/lib/utils'
import { useRuntimeState } from '@/lib/runtime'
import { inTauri } from '@/lib/tauri'
import { ARCH_VERSION } from '@/lib/version'
import { finiteCount, tokenThousandsLabel } from '@/lib/display-number'
import { NotificationsPopover } from './notifications-popover'

/** Native window controls (Tauri shell only — static dots in preview). */
function WindowControls() {
  const [shell, setShell] = React.useState(false)
  React.useEffect(() => {
    setShell(inTauri())
  }, [])
  if (!shell) {
    return (
      <div className="flex items-center gap-1.5" aria-hidden>
        <span className="h-3 w-3 rounded-full bg-red-500/90" />
        <span className="h-3 w-3 rounded-full bg-warning/90" />
        <span className="h-3 w-3 rounded-full bg-emerald-500/90" />
      </div>
    )
  }
  const act = (fn: 'minimize' | 'toggleMaximize' | 'close') => () =>
    void (async () => {
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window')
        const w = getCurrentWindow()
        if (fn === 'minimize') await w.minimize()
        else if (fn === 'close') await w.close()
        else await w.toggleMaximize()
      } catch {
        /* window API unavailable — controls stay inert */
      }
    })()
  const btn = 'no-drag h-3 w-3 rounded-full transition-transform hover:scale-110'
  return (
    <div className="flex items-center gap-1.5">
      <button type="button" aria-label="Minimize window" title="Minimize" onClick={act('minimize')} className={`${btn} bg-red-500/90`} />
      <button type="button" aria-label="Maximize window" title="Maximize" onClick={act('toggleMaximize')} className={`${btn} bg-warning/90`} />
      <button type="button" aria-label="Close window" title="Close" onClick={act('close')} className={`${btn} bg-emerald-500/90`} />
    </div>
  )
}

const statusColor: Record<string, string> = {
  idle: 'bg-zinc-500',
  running: 'bg-blue-500',
  'action-required': 'bg-brand',
  completed: 'bg-emerald-500',
  failed: 'bg-red-500',
  paused: 'bg-zinc-400',
  scheduled: 'bg-violet-500',
}

const statusLabel: Record<string, string> = {
  idle: 'Idle',
  running: 'Running',
  'action-required': 'Action required',
  completed: 'Completed',
  failed: 'Failed',
  paused: 'Paused',
  scheduled: 'Scheduled',
}

export function TitleBar() {
  const sessions = useAppStore((s) => s.sessions)
  const activeId = useAppStore((s) => s.activeSessionId)
  const active = sessions.find((s) => s.id === activeId)
  const toggleSidebar = useAppStore((s) => s.toggleSidebar)
  const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed)
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen)
  const powerMode = useAppStore((s) => s.powerMode)
  const togglePowerMode = useAppStore((s) => s.togglePowerMode)
  const liveBudget = useAppStore((s) => s.liveBudget)
  const taskFolder = useAppStore((s) => s.taskFolder)
  const setCenterScreen = useAppStore((s) => s.setCenterScreen)
  const setSettingsSection = useAppStore((s) => s.setSettingsSection)
  const runtime = useRuntimeState()
  const live = runtime.status === 'live'
  const { theme, toggle } = useTheme()
  const spent = finiteCount(liveBudget?.spent ?? active?.spent)
  const cap = finiteCount(liveBudget?.cap, 5)
  const tokens = finiteCount(liveBudget?.tokens ?? active?.tokens)

  return (
    <header className="drag-region h-9 shrink-0 border-b border-border bg-sidebar flex items-center px-2 gap-2 no-select">
      {/* Left cluster — traffic lights (Tauri shell) + app identity */}
      <div className="flex items-center gap-2 px-2">
        <WindowControls />
      </div>

      <div className="flex items-center gap-1.5 pl-2 pr-2 border-l border-border/60">
        <div className="grid h-5 w-5 place-items-center rounded-md bg-brand/15 ring-1 ring-brand/30">
          <Sparkles className="h-3 w-3 text-brand" />
        </div>
        <span className="text-xs font-semibold tracking-tight">
          AgentCowork
        </span>
        <Badge variant="secondary" className="h-4 text-[10px] px-1 py-0 font-mono">
          {ARCH_VERSION}
        </Badge>
      </div>

      {/* Workspace + session title */}
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <button
          type="button"
          onClick={() => setPaletteOpen(true)}
          className="no-drag hover:bg-accent rounded-md px-2 py-0.5 flex items-center gap-1 hover:text-foreground transition-colors"
          aria-label="Switch chat (opens the command palette)"
          title="Switch chat — opens the command palette"
        >
          <span className="font-medium text-foreground">agentcowork</span>
          <span className="text-muted-foreground/60">/</span>
          <span>{taskFolder ? taskFolder.split(/[\\/]/).pop() : 'work'}</span>
          <ChevronDown className="h-3 w-3 opacity-60" />
        </button>
        {active && (
          <>
            <span className="text-muted-foreground/40">·</span>
            <span className="font-medium text-foreground truncate max-w-[280px]">
              {active.title}
            </span>
            <span
              className={cn(
                'inline-block h-1.5 w-1.5 rounded-full',
                statusColor[active.status]
              )}
            />
            <span className="text-muted-foreground/80 text-[11px]">
              {statusLabel[active.status]}
            </span>
          </>
        )}
      </div>

      {/* Center — command palette launcher */}
      <div className="flex-1 flex items-center justify-center">
        <button
          onClick={() => setPaletteOpen(true)}
          className="no-drag group flex items-center gap-2 h-6 min-w-[280px] w-[40%] max-w-[420px] rounded-md border border-border bg-background/40 hover:bg-accent/40 hover:border-border/80 transition-colors px-2 text-[11px] text-muted-foreground"
        >
          <Search className="h-3 w-3 opacity-60" />
          <span className="flex-1 text-left">
            Search chats, files, commands…
          </span>
          <kbd className="flex items-center gap-0.5 text-[10px] text-muted-foreground/60 font-mono">
            <Command className="h-2.5 w-2.5" />K
          </kbd>
          <span className="text-muted-foreground/30 text-[9px] font-mono ml-1">⌘/ help</span>
        </button>
      </div>

      {/* Right cluster — guard + budget + theme + toggles */}
      <div className="flex items-center gap-1.5 pr-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={togglePowerMode}
              aria-pressed={powerMode}
              aria-label={powerMode ? 'Switch to casual mode' : 'Switch to power mode'}
              className={cn(
                'no-drag flex h-6 items-center gap-1 rounded-md border px-2 font-mono text-[10.5px] transition-colors',
                powerMode
                  ? 'border-brand/40 bg-brand/10 text-brand hover:bg-brand/20'
                  : 'border-border bg-background/40 text-muted-foreground hover:bg-accent hover:text-foreground',
              )}
            >
              <PanelRight className="h-3 w-3" />
              {powerMode ? 'Power' : 'Casual'}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {powerMode ? 'Hide cockpit views (⌘.)' : 'Show cockpit views and advanced controls (⌘.)'}
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={() => useAppStore.getState().setCenterScreen('guard')}
              className={cn(
                'no-drag flex h-6 items-center gap-1 rounded-md border px-2 font-mono text-[10.5px]',
                live
                  ? 'border-emerald-500/30 bg-emerald-500/10 hover:bg-emerald-500/20'
                  : 'border-border bg-background/40 hover:bg-accent',
              )}
            >
              <ShieldCheck className={cn('h-3 w-3', live ? 'text-emerald-400' : 'text-muted-foreground')} />
              {/* P32.12 — casual mode says what this is, not what it is called
                  internally. Power mode keeps the system vocabulary. */}
              <span className={live ? 'text-emerald-300' : 'text-muted-foreground'}>
                {powerMode
                  ? `Guard · ${live ? 'Standard' : 'unknown'}`
                  : `Safety · ${live ? 'on' : 'unknown'}`}
              </span>
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {powerMode
              ? 'Guard control center — approvals, tickets, policies. Not a sidebar destination.'
              : 'Your safety settings — what I am allowed to do, and what I have done.'}
          </TooltipContent>
        </Tooltip>

        {powerMode && (
          <>
            <div className="no-drag flex items-center gap-1 px-2 h-6 rounded-md border border-brand/30 bg-brand/10 text-[10.5px] font-mono">
              <span className="text-brand">${spent.toFixed(2)}</span>
              <span className="text-muted-foreground/60">/</span>
              <span className="text-muted-foreground">${cap.toFixed(2)}</span>
            </div>

            <div className="no-drag flex items-center gap-1 px-2 h-6 rounded-md border border-border bg-background/40 text-[10.5px] font-mono">
              <Activity className="h-3 w-3 text-blue-400" />
              <span className="text-muted-foreground">{tokenThousandsLabel(tokens)}</span>
            </div>
          </>
        )}

        <div className="w-px h-5 bg-border/60 mx-1" />

        <Tooltip>
          <TooltipTrigger asChild>
            {/* Icon-only control: the glyph alone gives no accessible name, so
                the button names the *result* (what the click will do) rather
                than its current state, and the icons stay out of the
                accessibility tree. */}
            <button
              type="button"
              onClick={toggle}
              aria-label={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
              title="Toggle theme"
              className="no-drag grid h-7 w-7 place-items-center rounded-md hover:bg-accent transition-colors"
            >
              {theme === 'dark' ? (
                <Sun aria-hidden className="h-3.5 w-3.5" />
              ) : (
                <Moon aria-hidden className="h-3.5 w-3.5" />
              )}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
          </TooltipContent>
        </Tooltip>

        <NotificationsPopover />

        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={toggleSidebar}
              aria-label={sidebarCollapsed ? 'Show sidebar' : 'Hide sidebar'}
              title="Toggle sidebar (Cmd+B)"
              className="no-drag grid h-7 w-7 place-items-center rounded-md hover:bg-accent transition-colors"
            >
              {sidebarCollapsed ? (
                <PanelLeft aria-hidden className="h-3.5 w-3.5" />
              ) : (
                <PanelLeftClose aria-hidden className="h-3.5 w-3.5" />
              )}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Toggle sidebar (Cmd+B)</TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={() => {
                setCenterScreen('settings')
                setSettingsSection('general')
              }}
              className="no-drag rounded-full hover:ring-2 hover:ring-brand/40 transition-shadow"
              aria-label="Open settings"
              title="Settings"
            >
              <Avatar className="h-6 w-6 ring-1 ring-border">
                <AvatarFallback className="bg-zinc-700 text-[10px]">⋯</AvatarFallback>
              </Avatar>
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Settings</TooltipContent>
        </Tooltip>
      </div>
    </header>
  )
}
