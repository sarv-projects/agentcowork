import { describe, expect, it } from 'vitest';
import { SHIPPED_AGENTS } from '@agentcowork/core-agents';
import { AGENT_CATALOG, AGENT_ORDER, agentSystemBlock, getAgentById, UI_EXTENSIONS } from '../chat/agents.js';
import { assembleChatPrompt } from '../chat/system-prompt.js';

describe('agents', () => {
  it('has general research writer planner reader', () => {
    const ids = AGENT_CATALOG.map((a) => a.id);
    expect(ids).toEqual(expect.arrayContaining(['general', 'research', 'writer', 'planner', 'reader']));
  });

  it('stays in sync with core-agents SHIPPED_AGENTS', () => {
    const shippedIds = SHIPPED_AGENTS.map((a) => a.id).sort();
    const uiIds = Object.keys(UI_EXTENSIONS).sort();
    expect(uiIds).toEqual(shippedIds);
    const catalogIds = AGENT_CATALOG.map((a) => a.id).sort();
    expect(catalogIds).toEqual(shippedIds);
  });

  it('defines an explicit UI order for every shipped agent', () => {
    const shippedIds = SHIPPED_AGENTS.map((a) => a.id).sort();
    const orderIds = [...AGENT_ORDER].sort();
    expect(orderIds).toEqual(shippedIds);
  });

  it('research prefers web', () => {
    expect(getAgentById('research').preferWeb).toBe(true);
  });

  it('assembleChatPrompt includes agent overlay', () => {
    const prompt = assembleChatPrompt({ agentId: 'writer', personaId: 'terse' });
    expect(prompt).toContain('Writer');
    expect(prompt).toContain('CACHE BOUNDARY');
  });

  it('agentSystemBlock names agent', () => {
    expect(agentSystemBlock('planner')).toContain('Planner');
  });
});
