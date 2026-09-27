/**
 * P32 — the casual-vs-power UX deltas (doc 84, Wharton/Nielsen research):
 *
 * - P32.1 `toPlainStage` — consumer phrasing for the now-doing strip, with
 *   the technical stage kept for the hover/expand layer.
 * - P32.3 `preciseFigures` — numbers in artifact cards, but **only the ones the
 *   run actually reported** (competence via precision, never via invention).
 * - P32.4 `limitationFor` — honest-limitation surfacing: say plainly what
 *   can't be done + offer the nearest alternative.
 * - P32.2 `SUGGESTED_AGENT_NAMES` — the name-your-agent ownership moment.
 * - P32.6 `inheritContext` — fewest-questions folder/session pre-fill.
 */

import { getLocalItem } from './storage-compat'

// ---------------------------------------------------------------------------
// P32.1 — plain-language stage labels (now-doing strip)
// ---------------------------------------------------------------------------

export const PLAIN_STAGE_LABELS: Record<string, string> = {
  'tool:office:running': 'Updating your document…',
  'tool:shell:running': 'Running a command for you…',
  'tool:browser:running': 'Browsing the page…',
  'tool:memory:running': 'Checking what I remember…',
  'tool:search:running': 'Searching your files…',
  'tool:codeintel:running': 'Reading your code…',
  'tool:exec:running': 'Working on it…',
  'tool:todo:running': 'Setting up a checklist…',
  'planning': 'Working out a plan…',
  'extracting_memory': 'Noting what I learned…',
  'streaming_start': 'Thinking…',
  'compiling': 'Getting ready…',
  'routed': 'Choosing the best model…',
  'context:logged': 'Keeping a record of what I used…',
}

/**
 * Map a stage label to its consumer phrase.
 *
 * WP8 — "never block without a sentence": the coordinator emits prefixed
 * stages (`routed:<provider>/<model> · <reason>`, `cache:hit:semantic`,
 * `context:logged:5`, `chief:refused:<reason>`) and both running *and* settled
 * tool stages. Every one of those now reads as a sentence instead of machine
 * text. A stage nobody has taught this map is passed through unchanged rather
 * than guessed at.
 */
export function toPlainStage(stage: string): string {
  const exact = PLAIN_STAGE_LABELS[stage]
  if (exact) return exact
  // tool:<id>:running|done|failed
  const tool = stage.match(/^tool:([a-z0-9_-]+):(running|done|failed)$/)
  if (tool) {
    const what = tool[1]!.replace(/-/g, ' ')
    if (tool[2] === 'done') return `Finished with ${what}.`
    if (tool[2] === 'failed') return `That did not work with ${what} — trying another way.`
    return `Working with ${what}…`
  }
  // Prefixed stages carry their detail after the first colon; only the head
  // decides the sentence (the detail stays in the technical hover layer).
  const head = stage.split(':', 1)[0]
  switch (head) {
    case 'routed':
      return 'Choosing the best model…'
    case 'cache':
      return 'Reusing what I already worked out…'
    case 'context':
      return 'Keeping a record of what I used…'
    case 'chief':
      return 'Handing this to another agent…'
    default:
      return stage
  }
}

// ---------------------------------------------------------------------------
// P32.3 — precise figures for artifact cards (K1 receipts companion)
// ---------------------------------------------------------------------------

export interface ArtifactFigures {
  type: ArtifactType
  name: string
  preview: string
  /**
   * Figures the run actually reported for this artifact (from its K1 receipt).
   * Absent means the run reported none — which renders as **no badge**, not as
   * a plausible-looking guess.
   */
  figures?: string[]
}

type ArtifactType = 'docx' | 'xlsx' | 'pptx' | 'pdf' | 'code' | 'markdown' | 'image' | 'webapp'

/**
 * Figures for an artifact card.
 *
 * P32.3 (corrected 2026-09-13) — this function **counts nothing and invents
 * nothing**. It returns exactly what the run reported, and an empty list when
 * the run reported nothing. The card then renders no badge at all.
 *
 * The earlier revision returned hardcoded sample counts (`'42 cells updated'`,
 * `'8 slides'`, `'0 tests broken'`) while the card rendered them under a
 * tooltip reading "Exact figures from this run's receipt". That put invented
 * numbers behind a provenance claim — the exact failure mode the honesty rule
 * exists to prevent, and worse than showing nothing.
 */
export function preciseFigures(a: ArtifactFigures): string[] {
  return a.figures ?? []
}

// ---------------------------------------------------------------------------
// P32.4 — honest-limitation surfacing
// ---------------------------------------------------------------------------

export interface Limitation {
  /** Plain-language statement of what couldn't be done. */
  plain: string
  /** The nearest thing that CAN be done. */
  alternative: string
}

/** Map a raw failure string to a plain + alternative pair. */
export function limitationFor(message: string): Limitation {
  const m = message.toLowerCase()
  if (m.includes('budget') || m.includes('limit')) {
    return {
      plain: 'I stopped here — this turn hit its budget.',
      alternative: 'You can raise the per-turn budget in Settings → Intelligence, then ask again.',
    }
  }
  if (m.includes('provider') || m.includes('model') || m.includes('key')) {
    return {
      plain: "I couldn't reach the AI provider (a model or key problem).",
      alternative: 'Check your provider key in Settings → Intelligence, or switch to a local model.',
    }
  }
  if (m.includes('network') || m.includes('offline') || m.includes('timeout')) {
    return {
      plain: "I couldn't reach the network from here.",
      alternative: 'Check your connection, or I can work on local files instead.',
    }
  }
  if (m.includes('permission') || m.includes('denied') || m.includes('block')) {
    return {
      plain: "I'm not allowed to do that without your go-ahead.",
      alternative: 'Approve the request on the card, or tell me a different way to reach the goal.',
    }
  }
  return {
    plain: "I couldn't finish that the way I tried.",
    alternative: 'Tell me what happened and I can try a different approach.',
  }
}

// ---------------------------------------------------------------------------
// P32.12 — noun vocabulary map (casual mode)
// ---------------------------------------------------------------------------

/**
 * The words the system uses, and the words a non-technical person would use.
 *
 * `toPlainStage` covers *what is happening*; this covers *the things*: the
 * names that leak developer vocabulary into the casual surface. Applied at
 * casual render points (title bar, now-doing, the casual safety summary), not
 * by string-replacing prose — a term absent here keeps its own name rather
 * than being described as something it is not.
 */
export const PLAIN_NOUNS: Record<string, string> = {
  guard: 'safety check',
  'guard-1': 'automatic safety check',
  'guard-2': 'your approval',
  'trust ladder': 'safety levels',
  autonomy: 'how much I can do on my own',
  'work mode': 'what I do',
  mcp: 'connected tools',
  acp: 'external agent',
  chief: 'the agent in charge',
  sidecar: 'my helper engine',
  vault: 'secure key store',
  trajectory: 'what happened',
  sandbox: 'look only',
  ticket: 'permission',
  receipt: 'record of what changed',
  audit: 'activity record',
  'policy rule': 'rule',
  'capability scope': 'what I am allowed to touch',
  egress: 'what leaves your computer',
  'tool call': 'step',
  subagent: 'helper',
  'diff card': 'change preview',
  nonce: 'one-time code',
}

/** Plain wording for a system noun (falls back to the term itself). */
export function toPlainNoun(term: string): string {
  return PLAIN_NOUNS[term.trim().toLowerCase()] ?? term
}

// ---------------------------------------------------------------------------
// P32.9 — one plain autonomy dial (casual mode)
// ---------------------------------------------------------------------------

/**
 * The autonomy ladder in words a non-technical user would actually use.
 *
 * This is a **display layer only**: the underlying four-value
 * `PermissionMode` ('sandbox' | 'ask' | 'auto' | 'full') is untouched, so the
 * per-task permission snapshot (`taskScopeHash`), the live Rust preset sync
 * (`syncAutonomyFromRust`) and every guard decision keep working unchanged.
 * Casual mode shows these labels; power mode keeps the technical controls.
 */
export const PLAIN_AUTONOMY_LABELS: Record<string, { emoji: string; label: string; hint: string }> = {
  sandbox: {
    emoji: '👀',
    label: 'Look only',
    hint: "I can read and plan, but I won't change anything.",
  },
  ask: {
    emoji: '🙋',
    label: 'Ask me first',
    hint: 'I check with you before anything changes.',
  },
  auto: {
    emoji: '⚖️',
    label: 'Balanced',
    hint: 'I handle routine changes and ask before big ones.',
  },
  full: {
    emoji: '🚀',
    label: 'Just do it',
    hint: 'I keep going on my own — deletes, payments and secrets still ask.',
  },
}

/** Ascending freedom — the order the casual dial and the keyboard cycle use. */
export const PLAIN_AUTONOMY_ORDER = ['sandbox', 'ask', 'auto', 'full'] as const

/**
 * Plain wording for an autonomy level. An unknown value returns itself with no
 * emoji rather than being silently described as something it is not.
 */
export function toPlainAutonomy(mode: string): { emoji: string; label: string; hint: string } {
  return PLAIN_AUTONOMY_LABELS[mode] ?? { emoji: '', label: mode, hint: '' }
}

// ---------------------------------------------------------------------------
// P32.2 — name-your-agent ownership moment
// ---------------------------------------------------------------------------

/** Suggested names for the default agent (B9 wizard step 1). */
export const SUGGESTED_AGENT_NAMES = [
  'Mira',
  'Aster',
  'Nova',
  'Iris',
  'Kite',
  'Fern',
  'Sage',
  'Orion',
]

export function suggestAgentNames(count = 4): string[] {
  // Deterministic pick from the middle so the same first-run sees the same set.
  return SUGGESTED_AGENT_NAMES.slice(2, 2 + count)
}

// ---------------------------------------------------------------------------
// P32.6 — fewest-questions context inheritance
// ---------------------------------------------------------------------------

export interface InheritedContext {
  /** The folder the first ask should already point at (best-effort). */
  folder?: string
  /** A session title derived from context, when available. */
  title?: string
}

/**
 * Pre-fill the first ask's context so no setup is required: the last-used
 * folder wins; otherwise we leave the folder unset (the agent asks only when
 * it genuinely needs to — ARCH/12 §4.0 onboarding stays enforced).
 */
export function inheritContext(): InheritedContext {
  try {
    // DEC-053: legacy `everyaios.*` key honored + promoted once.
    const last = getLocalItem('agentcowork.lastFolder')
    if (last) return { folder: last }
  } catch {
    /* storage unavailable — no inheritance */
  }
  return {}
}
