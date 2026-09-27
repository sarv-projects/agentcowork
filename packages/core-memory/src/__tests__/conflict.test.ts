import { describe, expect, it } from 'vitest';
import { MemoryConflictResolver } from '../conflict.js';
import type { FactCandidate, MemoryFact } from '@agentcowork/core-domain';

function hashEmbed(text: string): Float32Array {
  const vector = new Float32Array(16);
  let hash = 0;
  for (let i = 0; i < text.length; i++) {
    hash = (hash * 31 + text.charCodeAt(i)) | 0;
    const slot = Math.abs(hash) % vector.length;
    vector[slot] = (vector[slot] ?? 0) + 1;
  }
  let norm = 0;
  for (let i = 0; i < vector.length; i++) {
    norm += vector[i]! * vector[i]!;
  }
  if (norm > 0) {
    const scale = 1 / Math.sqrt(norm);
    for (let i = 0; i < vector.length; i++) {
      vector[i] = vector[i]! * scale;
    }
  }
  return vector;
}

const baseFact = (content: string): MemoryFact => ({
  id: 1,
  content,
  category: 'personal',
  tags: [],
  source: 'test',
  isActive: true,
  decayScore: 1,
  accessCount: 0,
  confidence: 0.9,
  pinned: false,
  storedAt: new Date().toISOString(),
  updatedAt: new Date().toISOString(),
  status: 'approved',
});

describe('MemoryConflictResolver', () => {
  it('flags duplicate facts with identical content', async () => {
    const resolver = new MemoryConflictResolver(async (text) => hashEmbed(text));
    const candidate: FactCandidate = {
      content: 'User lives in Mumbai',
      category: 'personal',
      source: 'chat',
      confidence: 'high',
    };
    const result = await resolver.detect(candidate, [baseFact('User lives in Mumbai')]);
    expect(result.type).toBe('duplicate');
  });

  it('treats unrelated facts as new', async () => {
    const resolver = new MemoryConflictResolver(async (text) => hashEmbed(text));
    const candidate: FactCandidate = {
      content: 'Favorite color is blue',
      category: 'personal',
      source: 'chat',
      confidence: 'low',
    };
    const result = await resolver.detect(candidate, [baseFact('User works at Acme Corp')]);
    expect(result.type).toBe('new');
  });
});