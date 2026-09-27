// P42.3 — OAuth scope review + honesty surface. Mirrors the Rust manifest in
// `agentcowork-core::connectors::scopes` (the connector modules request exactly
// these strings). Read-only-first: the write scope is opt-in per connector and
// Guard-2-gated. No compliance/enterprise claims — the panel renders these
// rows verbatim.

export interface ScopeEntry {
  scope: string;
  purpose: string;
  direction: "read" | "write";
  required: boolean;
}

export interface ConnectorScopeManifest {
  id: string;
  name: string;
  scopes: ScopeEntry[];
  posture: string;
}

export const SCOPE_MANIFEST: ConnectorScopeManifest[] = [
  {
    id: "google-workspace",
    name: "Google Workspace (Gmail · Drive · Docs · Sheets)",
    posture: "read-only by default; outbound send is opt-in and Guard-2-gated",
    scopes: [
      {
        scope: "https://www.googleapis.com/auth/gmail.readonly",
        purpose: "Read Gmail messages/labels for triage + search",
        direction: "read",
        required: true,
      },
      {
        scope: "https://www.googleapis.com/auth/gmail.send",
        purpose: "Send mail as the user (opt-in: outbound must be enabled)",
        direction: "write",
        required: false,
      },
      {
        scope: "https://www.googleapis.com/auth/drive.readonly",
        purpose: "List/read Drive files (metadata + export links)",
        direction: "read",
        required: true,
      },
      {
        scope: "https://www.googleapis.com/auth/documents.readonly",
        purpose: "Read Google Docs content",
        direction: "read",
        required: true,
      },
      {
        scope: "https://www.googleapis.com/auth/spreadsheets.readonly",
        purpose: "Read Google Sheets cell values",
        direction: "read",
        required: true,
      },
    ],
  },
  {
    id: "microsoft-graph",
    name: "Microsoft 365 / Graph (Mail · Calendar · OneDrive · Teams)",
    posture: "read-only by default; outbound send is opt-in and Guard-2-gated",
    scopes: [
      { scope: "Mail.Read", purpose: "Read mailbox messages", direction: "read", required: true },
      {
        scope: "Mail.Send",
        purpose: "Send mail (opt-in: outbound must be enabled)",
        direction: "write",
        required: false,
      },
      {
        scope: "Calendars.Read",
        purpose: "Read calendar events + availability",
        direction: "read",
        required: true,
      },
      {
        scope: "Files.Read",
        purpose: "Read OneDrive/SharePoint files (metadata + download)",
        direction: "read",
        required: true,
      },
      {
        scope: "Chat.Read",
        purpose: "Read Teams chat messages",
        direction: "read",
        required: true,
      },
      {
        scope: "offline_access",
        purpose: "Refresh tokens while the user is away",
        direction: "read",
        required: true,
      },
    ],
  },
];

export function scopesFor(id: string): ConnectorScopeManifest | undefined {
  return SCOPE_MANIFEST.find((m) => m.id === id);
}
