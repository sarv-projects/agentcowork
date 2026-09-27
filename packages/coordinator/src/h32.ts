/**
 * H32 — agent-scoped model surface (doc 68 §3), restated for v1.
 *
 * The original policy had two cases: the built-in engine (`everyaios-native`)
 * could carry a model selection because it owned model routing, while ACP agents
 * owned theirs and never received a per-agent model over the transport.
 *
 * `P71.2c` removed the built-in engine and `P71.2d` removed AgentCowork's own
 * inference path, so **the second case is the only case**: every agent owns its
 * model, and no request the sidecar builds may carry a per-agent model pin.
 * The functions keep their names because they are the policy seam a future
 * caller would use — they now answer the same thing for every input, which is
 * the point (there is no runtime to special-case).
 */

/** Ids that name no runtime: blank, or a retired built-in spelling. Kept as one
 * list so the next retirement extends the predicate instead of silently passing
 * through it — `everyaios-native` was exactly that omission. */
// DEC-053: the legacy spellings stay so already-retired agents keep being recognized, never resurrected.
const RETIRED_AGENT_IDS = new Set(["", "inbuilt", "everyaios", "everyaios-native"]);

/** Every runtime is an ACP agent now. Kept as a predicate so callers read the
 * intent, and so an empty/retired id is still distinguishable from an agent. */
export function isAcpAgent(agentId: string): boolean {
  return !RETIRED_AGENT_IDS.has(agentId.trim());
}

/**
 * The send policy: nothing is forwarded. A model selection made anywhere in the
 * UI is display-only, because the agent resolves its own model — carrying a pin
 * into the transport would name a model that agent never receives.
 */
export function shouldForwardModel(_agentId: string): boolean {
  return false;
}

/** Sanitize a request envelope for an agent-bound send: always strip `model`. */
export function sanitizeRequest<T extends { model?: string }>(req: T, _agentId: string): T {
  const { model: _dropped, ...rest } = req;
  return rest as T;
}

/** The picker column policy: the column is visible, never a selectable pin —
 * it renders as "managed by <agent>". */
export function modelColumnState(agentId: string): { visible: true; selectable: false; hint: string } {
  return { visible: true, selectable: false, hint: `model managed by ${agentId}` };
}
