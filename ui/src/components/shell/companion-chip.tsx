'use client'

/**
 * P30.16 — companion layer (skales Desktop-Buddy/Iris/pixel-pets pattern,
 * doc 83 §1, product-surface only — skales is BSL, no code steal).
 *
 * Honest scope: the coordinator owns the `companionFrameFor`/`moodForState`
 * seam (`packages/coordinator/src/companion.ts`); this chip is the minimal
 * live consumer — persona name + tagline with a session-state mood dot.
 * It is deliberately NOT a pixel-pet overlay (post-v1, high-effort
 * differentiator for the 6-to-60+ audience — doc 83 verdict). The seam is
 * now product-visible instead of dead code.
 */

import * as React from 'react'
import { Sparkles, Moon, Pause, Play } from 'lucide-react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useAppStore } from '@/lib/store'
import { AGENTS } from '@/lib/agents'
import { cn } from '@/lib/utils'

export type CompanionMood = 'neutral' | 'cheerful' | 'focused' | 'resting'

const MOOD_STYLE: Record<CompanionMood, { dot: string; label: string }> = {
  cheerful: { dot: 'bg-emerald-400', label: 'cheerful' },
  focused: { dot: 'bg-brand', label: 'focused' },
  resting: { dot: 'bg-sky-400', label: 'resting' },
  neutral: { dot: 'bg-zinc-400', label: 'neutral' },
}

function moodForState(paused: boolean, running: boolean): CompanionMood {
  if (running) return paused ? 'resting' : 'focused'
  return 'neutral'
}

export function CompanionChip() {
  const sessions = useAppStore((s) => s.sessions)
  const activeId = useAppStore((s) => s.activeSessionId)
  const paused = useAppStore((s) => s.pausedSessions[activeId])
  const active = sessions.find((x) => x.id === activeId)
  const running = active?.status === 'running'

  const agentName =
    AGENTS.find((a) => a.id === active?.agent)?.name ?? 'AgentCowork'
  const mood = moodForState(!!paused, !!running)
  const { dot, label } = MOOD_STYLE[mood]

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={() => useAppStore.getState().setCenterScreen('chat')}
          className="flex items-center gap-1.5 rounded-md border border-border/60 bg-background/40 px-2 py-0.5 text-[10px] text-muted-foreground hover:bg-accent/40"
          aria-label={`Companion: ${agentName} — ${label}. Open chat.`}
        >
          <Sparkles className="h-3 w-3 text-violet-400" />
          <span className="max-w-[140px] truncate font-medium text-foreground/80">
            {agentName}
          </span>
          <span className="flex items-center gap-1 text-[9px]">
            {running ? (
              paused ? (
                <Pause className="h-2.5 w-2.5 text-sky-400" />
              ) : (
                <Play className="h-2.5 w-2.5 text-emerald-400" />
              )
            ) : (
              <Moon className="h-2.5 w-2.5 text-zinc-500" />
            )}
            <span className={cn('h-1.5 w-1.5 rounded-full', dot)} />
          </span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="text-[10px]">
        {running
          ? paused
            ? `${agentName} is resting — paused mid-task, take over anytime`
            : `${agentName} is focused — working on this chat`
          : `${agentName} is ready — idle, waiting for your next task`}
      </TooltipContent>
    </Tooltip>
  )
}
