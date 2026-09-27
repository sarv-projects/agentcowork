'use client'

import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { CornerDownRight, Send, X } from 'lucide-react'
import { useAppStore } from '@/lib/store'
import { sendUserMessage } from '@/lib/bridge'
import { Button } from '@/components/ui/button'

/**
 * P30.12 — AIPointer quick-ask overlay (skales pattern, doc 83 §1, built
 * lean): a hotkey-anchored (⌥Space) translucent ask-box over any app. The
 * ask routes through the normal chat surface (`sendUserMessage`); screen
 * capture / clipboard reuse is the ADD-1 capture seam the box can call later.
 * Lean by design — no new engines.
 */
export function AiPointer() {
  const open = useAppStore((s) => s.aiPointerOpen)
  const setOpen = useAppStore((s) => s.setAiPointerOpen)
  const [text, setText] = useState('')
  const inputRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (open) {
      inputRef.current?.focus()
    } else {
      setText('')
    }
  }, [open])

  // ⌥Space / Escape are owned by the global shortcuts handler
  // (`keyboard-shortcuts.tsx`), which is the single listener for this box.
  // (A duplicate local listener used to double-toggle the open state.)

  const [sending, setSending] = useState(false)
  const send = async () => {
    const t = text.trim()
    if (!t || sending) return
    setSending(true)
    try {
      setOpen(false)
      await sendUserMessage(t)
    } catch (e) {
      useAppStore.getState().notify(
        e instanceof Error ? e.message : 'Quick ask failed',
        'error',
      )
    } finally {
      setSending(false)
    }
  }

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0, y: 10, scale: 0.98 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 10, scale: 0.98 }}
          transition={{ duration: 0.16, ease: [0.4, 0, 0.2, 1] }}
          className="fixed bottom-16 left-1/2 z-50 w-[440px] max-w-[calc(100vw-2rem)] -translate-x-1/2"
        >
          <div className="overflow-hidden rounded-xl border border-border/80 bg-zinc-950/85 shadow-2xl backdrop-blur-xl">
            <div className="flex items-center gap-2 border-b border-border/60 px-3 py-1.5">
              <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/70">
                Ask AgentCowork anywhere
              </span>
              <span className="ml-auto font-mono text-[9px] text-muted-foreground/50">⌥Space</span>
              <button
                onClick={() => setOpen(false)}
                className="grid h-5 w-5 place-items-center rounded text-muted-foreground/60 hover:bg-accent hover:text-foreground"
                aria-label="Close quick ask"
              >
                <X className="h-3 w-3" />
              </button>
            </div>
            <textarea
              ref={inputRef}
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  void send()
                }
              }}
              placeholder="Ask anything — it lands in the chat and runs with the agent…"
              rows={3}
              className="w-full resize-none bg-transparent px-3 py-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/50"
            />
            <div className="flex items-center justify-between border-t border-border/60 px-3 py-1.5">
              <span className="flex items-center gap-1 font-mono text-[9px] text-muted-foreground/50">
                <CornerDownRight className="h-2.5 w-2.5" /> Enter to ask · ⇧Enter newline
              </span>
              <Button size="sm" className="h-6 gap-1 px-2 text-[10px]" disabled={sending || !text.trim()} onClick={() => void send()}>
                <Send className="h-3 w-3" /> {sending ? 'Asking…' : 'Ask'}
              </Button>
            </div>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
