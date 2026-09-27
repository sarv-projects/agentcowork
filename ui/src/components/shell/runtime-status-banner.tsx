'use client'

import { AlertTriangle, CheckCircle2, CircleDot, Loader2, LockKeyhole, ServerOff } from 'lucide-react'
import { useRuntimeState } from '@/lib/runtime'
import { cn } from '@/lib/utils'

const META = {
  preview: {
    label: 'Development Preview — no real files, accounts, providers, or tasks',
    className: 'border-warning/40 bg-warning/10 text-warning',
    icon: AlertTriangle,
  },
  booting: {
    label: 'Starting AgentCowork…',
    className: 'border-blue-500/30 bg-blue-500/10 text-blue-200',
    icon: Loader2,
  },
  'vault-setup': {
    label: 'Vault setup required',
    className: 'border-warning/40 bg-warning/10 text-warning',
    icon: LockKeyhole,
  },
  'vault-locked': {
    label: 'Vault locked',
    className: 'border-warning/40 bg-warning/10 text-warning',
    icon: LockKeyhole,
  },
  'sidecar-offline': {
    label: 'Coordinator offline — live agent work is unavailable',
    className: 'border-red-500/40 bg-red-500/10 text-red-200',
    icon: ServerOff,
  },
  live: {
    label: 'Live runtime',
    className: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-200',
    icon: CheckCircle2,
  },
  degraded: {
    label: 'Runtime degraded — some capabilities are unavailable',
    className: 'border-red-500/40 bg-red-500/10 text-red-200',
    icon: CircleDot,
  },
} as const

export function RuntimeStatusBanner() {
  const runtime = useRuntimeState()
  const meta = META[runtime.status]
  const Icon = meta.icon
  const persistent = runtime.status !== 'live'

  if (!persistent) return null

  return (
    <div
      role="status"
      aria-live="polite"
      className={cn('flex shrink-0 items-center gap-2 border-b px-3 py-1.5 text-[11px]', meta.className)}
      title={runtime.detail}
    >
      <Icon className={cn('h-3.5 w-3.5 shrink-0', runtime.status === 'booting' && 'animate-spin')} />
      <span className="font-medium">{meta.label}</span>
      {runtime.detail && runtime.status !== 'preview' && (
        <span className="truncate opacity-75">· {runtime.detail}</span>
      )}
    </div>
  )
}
