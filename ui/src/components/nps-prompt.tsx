'use client'

// P11.6.2 — non-intrusive NPS/satisfaction prompt.
//
// Shows a small dismissible card after the user has been around for 7 days
// (tracked in localStorage), at most once per 90-day window. Scores + optional
// comment are stored locally; they are not sent anywhere. The timing rules
// live in `lib/nps.ts` (pure, unit-tested) — this component is just the card.

import { useState } from 'react'
import { X } from 'lucide-react'
import {
  localStorageNpsStorage,
  npsRecordScore,
  npsShouldPrompt,
} from '@/lib/nps'

export default function NpsPrompt() {
  const [visible, setVisible] = useState<boolean>(() => npsShouldPrompt(Date.now(), localStorageNpsStorage))
  const [score, setScore] = useState<number | null>(null)
  const [comment, setComment] = useState('')

  if (!visible) return null

  const submit = (finalScore: number) => {
    npsRecordScore(finalScore, comment.trim(), Date.now(), localStorageNpsStorage)
    setVisible(false)
    setScore(finalScore)
  }

  return (
    <div
      data-testid="nps-prompt"
      className="fixed bottom-16 right-4 z-50 w-72 rounded-lg border border-border bg-card p-4 shadow-xl enter-approval"
    >
      <button
        aria-label="Dismiss NPS prompt"
        className="absolute right-2 top-2 flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
        onClick={() => setVisible(false)}
      >
        <X className="h-3 w-3" />
      </button>
      <p className="text-xs font-medium text-foreground">How likely are you to recommend AgentCowork to a friend or colleague?</p>
      {score === null ? (
        <div className="mt-3 flex justify-between gap-1">
          {[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((n) => (
            <button
              key={n}
              onClick={() => void submit(n)}
              aria-label={`Score ${n} of 10`}
              className="flex size-5 items-center justify-center rounded text-[10px] text-muted-foreground transition-colors hover:bg-brand/20 hover:text-brand"
            >
              {n}
            </button>
          ))}
        </div>
      ) : (
        <div className="mt-3 space-y-2">
          <input
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            placeholder="Anything you'd improve? (optional, stored locally)"
            className="w-full rounded border border-border bg-background/40 px-2 py-1 text-[11px] text-foreground placeholder:text-muted-foreground/50 focus:border-brand/50 focus:outline-none"
          />
          <button
            className="w-full rounded bg-brand py-1 text-[11px] font-medium text-black hover:bg-brand"
            onClick={() => submit(score)}
          >
            Save
          </button>
        </div>
      )}
      <p className="mt-2 text-[9px] text-muted-foreground">Stored locally only. Thanks for making AgentCowork better.</p>
    </div>
  )
}
