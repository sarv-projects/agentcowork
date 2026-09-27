/**
 * Governed search transport (P69.C4 / P69.D4).
 *
 * The sidecar proposes, Rust disposes: a BYOK search provider must never hold a
 * credential and must never open its own egress (AGENTS.md §15, `ARCH/CORE.md`
 * §2/§6). Instead it hands the *intent* — provider id, endpoint, body without
 * credentials — to the host, which
 *
 *   1. resolves the provider secret from `agentcowork-vault` (`keyRef`, never a
 *      plaintext key crossing into JavaScript),
 *   2. validates the destination through Guard-2 `netfloor`, and
 *   3. performs the request, returning the raw response.
 *
 * A host that has not attached a transport means "no governed egress path
 * available": key-based providers degrade to *unavailable* rather than falling
 * back to an environment variable or a direct `fetch`. That fallback is exactly
 * the duplicate-authority path P69.C4 removed.
 */
export interface GovernedSearchRequest {
  /** Logical provider id the host resolves to a vault key ref, e.g. `tavily`. */
  provider: string;
  /** Absolute destination; the host validates it against netfloor. */
  url: string;
  method: 'GET' | 'POST';
  /** Request body, always free of credentials — the host injects those. */
  body?: unknown;
  /** Extra non-secret headers the provider requires (e.g. content type). */
  headers?: Record<string, string>;
  /** Advisory timeout; the host enforces its own ceiling. */
  timeoutMs?: number;
}

export interface GovernedSearchResponse {
  status: number;
  json(): Promise<unknown>;
  text(): Promise<string>;
}

export interface GovernedSearchTransport {
  request(req: GovernedSearchRequest): Promise<GovernedSearchResponse>;
}

let activeTransport: GovernedSearchTransport | null = null;

/**
 * Attach (or clear) the host transport. Called once during sidecar bootstrap
 * with the bridge that speaks the shell's governed-egress call.
 */
export function setGovernedSearchTransport(transport: GovernedSearchTransport | null): void {
  activeTransport = transport;
}

/** The attached host transport, or `null` when the host provides none. */
export function getGovernedSearchTransport(): GovernedSearchTransport | null {
  return activeTransport;
}

/** True when a governed egress path exists, i.e. a BYOK provider can run. */
export function hasGovernedSearchTransport(): boolean {
  return activeTransport !== null;
}
