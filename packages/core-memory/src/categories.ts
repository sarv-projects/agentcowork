import type { IntentCategory, MemoryCategory } from '@agentcowork/core-domain';
import { MEMORY_CATEGORIES } from '@agentcowork/core-domain';

const QUERY_CATEGORY_HINTS: Array<{ pattern: RegExp; categories: MemoryCategory[] }> = [
  { pattern: /\b(book|chapter|character|author|novel|read)\b/i, categories: ['books'] },
  { pattern: /\b(bill|payment|tax|invest|stock|mutual fund|emi|salary)\b/i, categories: ['finance'] },
  { pattern: /\b(doctor|medicine|prescription|health|symptom)\b/i, categories: ['health'] },
  { pattern: /\b(meeting|client|project|deadline|office|work)\b/i, categories: ['work', 'projects'] },
  { pattern: /\b(family|prefer|habit|goal|name is|live in)\b/i, categories: ['personal'] },
];

/** Heuristic category inference from query text (SLM can refine later). */
export function inferMemoryCategoriesFromQuery(query: string): MemoryCategory[] {
  const matched = new Set<MemoryCategory>();
  for (const hint of QUERY_CATEGORY_HINTS) {
    if (hint.pattern.test(query)) {
      for (const cat of hint.categories) {
        matched.add(cat);
      }
    }
  }
  if (matched.size === 0) {
    return ['personal'];
  }
  return [...matched];
}

/** Map router intent → memory categories for scoped recall. */
export function memoryCategoriesForIntent(
  category: IntentCategory,
  query: string,
  options: { openDocumentId?: string } = {},
): MemoryCategory[] {
  if (options.openDocumentId) {
    return ['books'];
  }

  switch (category) {
    case 'needs-files':
      return ['books', 'work', 'projects'];
    case 'needs-connector':
      return inferMemoryCategoriesFromQuery(query);
    case 'conversational':
      return inferMemoryCategoriesFromQuery(query);
    default:
      return ['personal'];
  }
}

export function allMemoryCategories(): MemoryCategory[] {
  return [...MEMORY_CATEGORIES];
}