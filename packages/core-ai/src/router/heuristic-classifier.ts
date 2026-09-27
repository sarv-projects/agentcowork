import type { IntentCategory, IntentClassification, UserQuery } from '@agentcowork/core-domain';
import type { IntentClassifier } from './types.js';

const KEYWORD_RULES: Array<{ category: IntentCategory; patterns: RegExp[]; confidence: number }> = [
  // NOTE (C.10): local-intent rules (files / connectors / automations / docs)
  // MUST run before the web rule — "search my files" / "update my calendar"
  // were being misrouted to web because needs-web matched the search verb first.
  {
    category: 'needs-files',
    patterns: [
      /\bmy (file|files|document|documents|pdf|pdfs|lease|contract)\b/i,
      /\bin (this|the) (file|document|pdf)\b/i,
      /\bfrom (my|the) (files|documents)\b/i,
      /\bopen document\b/i,
      /\bsummarize (this|the|my) (file|document|pdf|article)\b/i,
    ],
    confidence: 0.84,
  },
  {
    category: 'needs-connector',
    patterns: [
      /\bfrom (gmail|email|calendar|slack|notion|drive)\b/i,
      /\bmy (inbox|emails|calendar|calendar events|drive|notion)\b/i,
      /\bconnected app\b/i,
    ],
    confidence: 0.78,
  },
  {
    category: 'needs-automation',
    patterns: [
      /\bremind me\b/i,
      /\bevery (morning|day|week|month)\b/i,
      /\bschedule\b/i,
      /\bset (a )?task\b/i,
      /\btodo\b/i,
    ],
    confidence: 0.8,
  },
  {
    category: 'needs-docs',
    patterns: [
      // Explicit library/API help
      /\b(use |with |in |for )?(react|next\.?js|vue|angular|svelte|node\.?js|express|fastify|nestjs|django|flask|laravel|rails|spring)\b/i,
      /\b(use |with )?(supabase|firebase|prisma|mongodb|postgres|mysql|redis|sqlite|drizzle)\b/i,
      /\b(use |with )?(tailwind|mui|shadcn|chakra|bootstrap|expo|react native|flutter)\b/i,
      /\b(use |with )?(aws|gcp|azure|cloudflare|vercel|netlify|heroku)\b/i,
      /\b(how (do|can|should) I .+ (in |with |using )?(react|next|vue|node|express|prisma|supabase|tailwind|expo|flutter))\b/i,
      // Package/library specific phrasing
      /\b(ctx7|context7|docs for|documentation for|api reference for|sdk for)\b/i,
      /\b(how does .+ work in .+)\b/i,
      /\b(error in|bug in|import from|install|setup|configure) .+\b/i,
    ],
    confidence: 0.82,
  },
  {
    category: 'needs-web',
    patterns: [
      // Explicit search verbs (not followed by a local-intent target)
      /\b(search|google|look up|find online|web search|search the web)\b(?! (my|this|the) (file|files|document|documents|pdf|inbox|emails|calendar|drive|notion))/i,
      // Time-sensitive / current-event markers
      /\b(current|latest|recent|breaking|just announced|today|yesterday|tomorrow|this week|this month|this year)\b/i,
      // Weather / markets / sports / traffic — live data
      /\b(weather|forecast|stock price|crypto|bitcoin|market|score|live score|fixture|traffic|flight status)\b/i,
      // News / events / releases
      /\b(news|headlines|election results|who won|released|launched|dropped|update|patch notes|changelog)\b/i,
      // Prices / availability that change frequently
      /\b(price of|cost of|available now|in stock|sold out|booking|reservation)\b/i,
    ],
    confidence: 0.85,
  },
  {
    category: 'out-of-scope',
    patterns: [/\bhack\b/i, /\billegal\b/i, /\bself[- ]harm\b/i],
    confidence: 0.9,
  },
];


export class HeuristicIntentClassifier implements IntentClassifier {
  async classify(query: UserQuery): Promise<IntentClassification> {
    const normalized = query.text.trim().toLowerCase();
    const depth = classifyDepth(query.text);

    if (query.scope === 'open-document' || query.attachments?.length) {
      return { category: 'needs-files', confidence: 0.88, subCategory: 'scoped-to-document', depth };
    }

    for (const rule of KEYWORD_RULES) {
      if (rule.patterns.some((pattern) => pattern.test(normalized))) {
        return { category: rule.category, confidence: rule.confidence, depth };
      }
    }

    const greeting = /^(hi|hey|hello|thanks|thank you|good (morning|evening|afternoon|night)|bye|goodbye)\b/i;
    const veryShort = /^.{0,25}$/;
    const conversationOnly = /\b(how are you|what('s| is) up|sup\b|wassup|yo\b|okay|ok|nice|cool|great|awesome|lol|haha)\b/i;
    const creativeOrPersonal = /\b(write|draft|story|poem|joke|idea|help me|advice|opinion|recommend|suggest)\b/i;

    if (
      greeting.test(normalized) ||
      conversationOnly.test(normalized) ||
      creativeOrPersonal.test(normalized) ||
      veryShort.test(normalized)
    ) {
      return { category: 'conversational', confidence: 0.92, depth };
    }

    // Default non-trivial queries to conversational, NOT needs-web.
    // The Research agent can still force web via preferWeb=true.
    // This stops "Hi" / "explain X" / "who is Y" from triggering a web search.
    return { category: 'conversational', confidence: 0.72, depth };
  }
}

/**
 * Classify response depth from the user's query.
 * Depth is independent of model routing (Fast/Smart/Vision).
 */
export function classifyDepth(query: string): 'quick' | 'standard' | 'detailed' {
  const lower = query.toLowerCase();

  // Quick: short questions, greetings, simple facts
  if (
    /^(hi|hey|hello|thanks|bye|ok|yes|no|cool|nice)\b/.test(lower) ||
    lower.length < 30 ||
    /\b(what is|who is|when did|how many|define|meaning of)\b/.test(lower)
  ) {
    return 'quick';
  }

  // Detailed: explicit depth signals
  if (
    /\b(detailed|comprehensive|in.depth|thorough|full analysis|explain in detail|deep dive|elaborate|compare.* pros and cons|step.by.step|write.* essay|research|investigate)\b/.test(
      lower,
    ) ||
    lower.length > 200
  ) {
    return 'detailed';
  }

  // Default: standard
  return 'standard';
}
