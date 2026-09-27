//! P6.6/P6.11 — Connector transports.
//!
//! Every transport is behind an injectable [`HttpTransport`] seam (or
//! [`CdpSession`] for browser-session connectors) so the full protocol logic
//! is testable with mock data and no live accounts.
//!
//! The live transport implementations (real OAuth tokens, real CDP sessions)
//! are wired at runtime through the `ConnectorHub` engine routing — never
//! from inside these modules.

pub mod browser_session;
pub mod calendar;
pub mod gmail;
pub mod graph;
pub mod gws;
pub mod imap_smtp;
pub mod native;
pub mod read_first;
pub mod scopes;
pub mod workspace;

pub use browser_session::BrowserSessionConnector;
pub use calendar::CalendarConnector;
pub use gmail::GmailConnector;
pub use graph::{
    GraphCalendarEvent, GraphChat, GraphChatMessage, GraphConnector, GraphDriveItem,
    GraphMailMessage,
};
pub use gws::{GwsAction, GwsConnector, GwsError, GwsRequest};
pub use imap_smtp::ImapSmtpConnector;
pub use native::{
    AuditChain, AuditEntry, ColumnRedaction, CostGuardError, ExplainCostGuard, SqlClass, SqlGuard,
    SqlGuardError, classify_sql, has_stacked_statements,
};
pub use read_first::{
    ReadFirstPolicy, SendAction, SendApproval, SendBlocked, SendClass, SendKind, VaultTokenRef,
};
pub use scopes::{
    ConnectorScopeManifest, GOOGLE_WORKSPACE_SCOPES, MICROSOFT_GRAPH_SCOPES, SCOPE_MANIFEST,
    ScopeEntry, attach_scopes,
};
pub use workspace::{WorkspaceConnector, WorkspaceDoc, WorkspaceDriveFile, WorkspaceSheetValues};

/// Injectable HTTP transport seam — all connectors call the outside world
/// through this trait so the full protocol logic can be tested without
/// network access.
pub trait HttpTransport {
    /// Send a JSON POST and return the parsed response.
    fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<Vec<u8>, TransportError>;

    /// Send a GET and return the raw response bytes.
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, TransportError>;
}

/// Use-style credential access (INV-02, CTR-013, `ARCH/12-TRUST.md` §6).
///
/// FIX-01: a connector used to hold the access token as a `String` field, which
/// is a *read-value* API outside the vault — it could be cloned, serialized,
/// logged, returned from any method, or captured by a closure, and it outlived
/// the call in the connector's own state. This trait is the replacement: the
/// connector holds a [`VaultTokenRef`] (a key id + service) and the value is
/// only reachable **inside** the closure `with_token` / `refresh` hands it to,
/// for the duration of one call. Enumeration by agent code is therefore
/// impossible, the connector struct itself contains no secret, and the only
/// implementation that touches bytes lives in the vault.
pub trait TokenSource: Send + Sync {
    /// Perform `f` with the live access token for `token_ref`.
    ///
    /// `f` returns the *transport* result, so a vault failure (no token, vault
    /// unavailable) stays distinguishable from a provider failure — the two
    /// carry different taxonomy codes and must not be collapsed.
    fn with_token<T>(
        &self,
        token_ref: &VaultTokenRef,
        f: &mut dyn FnMut(&str) -> Result<T, TransportError>,
    ) -> Result<T, TransportError>;

    /// Perform `f` with a freshly refreshed access token (the vault's exchange).
    /// A connector calls this exactly once after a `401`, then retries — it
    /// never caches the refreshed value.
    fn refresh<T>(
        &self,
        token_ref: &VaultTokenRef,
        f: &mut dyn FnMut(&str) -> Result<T, TransportError>,
    ) -> Result<T, TransportError>;
}

/// Injectable CDP session seam for browser-session connectors.
pub trait CdpSession {
    /// Evaluate a JavaScript expression in the page context.
    fn evaluate(&self, expression: &str) -> Result<String, TransportError>;

    /// Navigate to a URL and wait for load.
    fn navigate(&self, url: &str) -> Result<(), TransportError>;

    /// Send a CDP command and return the JSON result.
    fn send_command(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError>;
}

/// Transport-level error.
#[derive(Debug, Clone)]
pub struct TransportError {
    pub kind: TransportErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportErrorKind {
    Network,
    Auth,
    RateLimited,
    NotFound,
    InvalidResponse,
    Timeout,
    Other,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for TransportError {}
