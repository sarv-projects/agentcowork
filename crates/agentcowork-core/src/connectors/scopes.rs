//! P42.3 — ToS/scope review + honesty surface.
//!
//! The documented OAuth scopes for the P42 connectors, **read-only-first**:
//! every service is attached with the narrowest scope that supports its
//! declared read surface, and the send/mutate scope is only ever requested
//! when the user explicitly enables outbound for that connector (never by
//! default). No compliance/enterprise claims — these are the exact scope
//! strings the Auth Bridge requests, reviewed 2026-08-25 against the official
//! Google + Microsoft OAuth scope registries.
//!
//! The Connectors panel renders these rows verbatim (the honesty surface):
//! a connector row shows its scopes and its read/write posture before the
//! user attaches it.

/// One documented scope entry.
#[derive(Debug, Clone)]
pub struct ScopeEntry {
    /// The exact OAuth scope string.
    pub scope: &'static str,
    /// What the app reads/writes with it.
    pub purpose: &'static str,
    /// `read` | `write` — write scopes are opt-in per connector.
    pub direction: &'static str,
    /// `optional` = only requested when the user enables outbound.
    pub required: bool,
}

/// Google Workspace scopes (P42.2 — Gmail/Drive/Docs/Sheets).
pub const GOOGLE_WORKSPACE_SCOPES: &[ScopeEntry] = &[
    ScopeEntry {
        scope: "https://www.googleapis.com/auth/gmail.readonly",
        purpose: "Read Gmail messages/labels for triage + search",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "https://www.googleapis.com/auth/gmail.send",
        purpose: "Send mail as the user (opt-in: outbound must be enabled)",
        direction: "write",
        required: false,
    },
    ScopeEntry {
        scope: "https://www.googleapis.com/auth/drive.readonly",
        purpose: "List/read Drive files (metadata + export links)",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "https://www.googleapis.com/auth/documents.readonly",
        purpose: "Read Google Docs content",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "https://www.googleapis.com/auth/spreadsheets.readonly",
        purpose: "Read Google Sheets cell values",
        direction: "read",
        required: true,
    },
];

/// Microsoft Graph scopes (P42.1 — mail/calendar/OneDrive/Teams).
pub const MICROSOFT_GRAPH_SCOPES: &[ScopeEntry] = &[
    ScopeEntry {
        scope: "Mail.Read",
        purpose: "Read mailbox messages",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "Mail.Send",
        purpose: "Send mail (opt-in: outbound must be enabled)",
        direction: "write",
        required: false,
    },
    ScopeEntry {
        scope: "Calendars.Read",
        purpose: "Read calendar events + availability",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "Files.Read",
        purpose: "Read OneDrive/SharePoint files (metadata + download)",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "Chat.Read",
        purpose: "Read Teams chat messages",
        direction: "read",
        required: true,
    },
    ScopeEntry {
        scope: "offline_access",
        purpose: "Refresh tokens while the user is away",
        direction: "read",
        required: true,
    },
];

/// The full manifest the UI renders (connector id → scopes + posture).
#[derive(Debug, Clone)]
pub struct ConnectorScopeManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub scopes: &'static [ScopeEntry],
    /// Read-only-first posture line (never claims write parity).
    pub posture: &'static str,
}

pub const SCOPE_MANIFEST: &[ConnectorScopeManifest] = &[
    ConnectorScopeManifest {
        id: "google-workspace",
        name: "Google Workspace (Gmail · Drive · Docs · Sheets)",
        scopes: GOOGLE_WORKSPACE_SCOPES,
        posture: "read-only by default; outbound send is opt-in and Guard-2-gated",
    },
    ConnectorScopeManifest {
        id: "microsoft-graph",
        name: "Microsoft 365 / Graph (Mail · Calendar · OneDrive · Teams)",
        scopes: MICROSOFT_GRAPH_SCOPES,
        posture: "read-only by default; outbound send is opt-in and Guard-2-gated",
    },
];

/// The read scopes a connector requests on attach (never includes the opt-in
/// write scopes unless `with_outbound` is true).
pub fn attach_scopes(manifest: &ConnectorScopeManifest, with_outbound: bool) -> Vec<String> {
    manifest
        .scopes
        .iter()
        .filter(|s| s.required || (with_outbound && s.direction == "write"))
        .map(|s| s.scope.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_attach_never_requests_write_scopes() {
        for m in SCOPE_MANIFEST {
            let scopes = attach_scopes(m, false);
            for s in &scopes {
                let entry = m.scopes.iter().find(|e| e.scope == s).unwrap();
                assert_eq!(entry.direction, "read", "{} leaked a write scope", m.id);
            }
        }
    }

    #[test]
    fn opt_in_outbound_adds_exactly_the_write_scopes() {
        let ws = SCOPE_MANIFEST
            .iter()
            .find(|m| m.id == "google-workspace")
            .unwrap();
        let read = attach_scopes(ws, false);
        let full = attach_scopes(ws, true);
        assert!(read.len() < full.len());
        let extra: Vec<String> = full.iter().filter(|s| !read.contains(s)).cloned().collect();
        assert_eq!(
            extra.len(),
            1,
            "gmail.send is the only opt-in write: {extra:?}"
        );
        assert!(extra[0].contains("gmail.send"));
    }

    #[test]
    fn manifest_has_no_enterprise_overclaims() {
        for m in SCOPE_MANIFEST {
            let lower = format!("{} {}", m.name, m.posture).to_ascii_lowercase();
            assert!(
                !lower.contains("compliance") && !lower.contains("enterprise"),
                "no compliance/enterprise claims allowed: {}",
                m.name
            );
        }
    }
}
