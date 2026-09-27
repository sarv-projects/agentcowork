/** P6.7 — external MCP search consumer.
 *
 * The APP package already contains the MCP SDK client. Desktop keeps it lazy:
 * no network connection or optional SDK import is opened at boot, and every
 * endpoint is supplied by the user/configuration. Credentials and egress
 * policy remain host-owned.
 */
import type { McpSearchClient } from "@agentcowork/core-search";

const clients = new Map<string, McpSearchClient>();

async function clientFor(endpoint: string): Promise<McpSearchClient> {
  let client = clients.get(endpoint);
  if (!client) {
    const { McpSearchClient: Client } = await import("@agentcowork/core-search");
    client = new Client(endpoint);
    clients.set(endpoint, client);
  }
  return client;
}

export async function searchExternalMcp(
  endpoint: string,
  query: string,
  toolName = "search",
): Promise<unknown[]> {
  if (!/^https:\/\//i.test(endpoint) && !/^http:\/\/127\.0\.0\.1(?::|\/)/i.test(endpoint) && !/^http:\/\/localhost(?::|\/)/i.test(endpoint)) {
    throw new Error("external MCP search endpoint must be HTTPS or loopback HTTP");
  }
  return (await clientFor(endpoint)).search(query, toolName);
}

export async function disconnectExternalMcp(endpoint: string): Promise<void> {
  const client = clients.get(endpoint);
  if (client) {
    await client.disconnect();
    clients.delete(endpoint);
  }
}
