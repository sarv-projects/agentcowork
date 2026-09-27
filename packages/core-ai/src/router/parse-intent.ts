import type { IntentCategory, IntentClassification } from '@agentcowork/core-domain';
import { classifyDepth } from './heuristic-classifier.js';

const VALID_CATEGORIES: IntentCategory[] = [
  'conversational',
  'needs-files',
  'needs-web',
  'needs-automation',
  'needs-connector',
  'needs-docs',
  'out-of-scope',
];

export function parseIntentCategory(raw: string): IntentCategory | null {
  const normalized = raw.trim().toLowerCase().replace(/\s+/g, '-');
  const direct = VALID_CATEGORIES.find((category) => normalized === category || normalized.includes(category));
  if (direct) {
    return direct;
  }

  if (normalized.includes('web') || normalized.includes('search')) {
    return 'needs-web';
  }
  if (normalized.includes('file') || normalized.includes('document')) {
    return 'needs-files';
  }
  // 'task' keyword maps to needs-automation (legacy intent name)
  if (normalized.includes('task') || normalized.includes('remind')) {
    return 'needs-automation';
  }
  if (normalized.includes('connector') || normalized.includes('email')) {
    return 'needs-connector';
  }
  if (normalized.includes('out') || normalized.includes('scope')) {
    return 'out-of-scope';
  }
  if (normalized.includes('convers')) {
    return 'conversational';
  }
  return null;
}

export function classificationFromLabel(
  raw: string,
  fallback: IntentCategory = 'conversational',
): IntentClassification {
  const category = parseIntentCategory(raw) ?? fallback;
  return {
    category,
    confidence: category === fallback ? 0.55 : 0.75,
    depth: classifyDepth(raw),
  };
}