//! ACP v1 message types (doc 45 §1, agentclientprotocol.com/protocol/v1).
//!
//! Conventions (from the spec): JSON object property keys are **camelCase**;
//! discriminator string values are **snake_case**. We mirror that here with
//! `#[serde(rename_all = "camelCase")]` on structs and `rename_all =
//! "snake_case"` on the enums that serialize as strings.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// The stable wire protocol version (integer major; negotiated at
/// `initialize`). Bumped only on breaking changes — non-breaking features
/// ride the capability mechanism.
pub const PROTOCOL_VERSION: u64 = 1;

// ---------------------------------------------------------------------------
// Capabilities — all optional, default = unsupported (doc 45 §1.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientCapabilities {
    pub fs: FsCapabilities,
    pub terminal: bool,
    /// ACP session configuration support. Select options need no capability
    /// marker; the optional boolean marker advertises that this client can
    /// render and set boolean options too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionCapabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_options: Option<ConfigOptionCapabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigOptionCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boolean: Option<serde_json::Value>,
}

impl SessionCapabilities {
    pub fn config_options_with_boolean() -> Self {
        Self {
            config_options: Some(ConfigOptionCapabilities {
                boolean: Some(serde_json::json!({})),
            }),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FsCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub prompt_capabilities: PromptCapabilities,
    pub mcp_capabilities: McpCapabilities,
    pub auth: AuthCapabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpCapabilities {
    pub http: bool,
    pub sse: bool,
}

/// Agent auth capabilities. The spec's `agentCapabilities.auth.logout` is an
/// empty object `{}` when supported (and omitted/null when not) — a bool would
/// misparse. `logout` is `Some(())` when the agent supports it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthCapabilities {
    #[serde(default, deserialize_with = "deser_marker")]
    pub logout: Option<()>,
}

/// Accept `{}` (object), `true`, or a missing key as the logout marker.
fn deser_marker<'de, D>(de: D) -> Result<Option<()>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<serde_json::Value> = Option::deserialize(de)?;
    Ok(match v {
        Some(serde_json::Value::Null) | None => None,
        // `{}` or `true` ⇒ supported.
        _ => Some(()),
    })
}

impl AuthCapabilities {
    pub fn supports_logout(&self) -> bool {
        self.logout.is_some()
    }
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// `agent` (default) | `url` | `terminal` — how the client completes login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<AuthMethodType>,
    /// Terminal-type methods carry args/env for the out-of-band launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Terminal-type methods carry env for the out-of-band launch. Real agents
    /// send this as a JSON **object** (`"env": {}` on `pi-acp`), while the
    /// pair-list form appears elsewhere — accept both.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_env_pairs"
    )]
    pub env: Option<Vec<(String, String)>>,
}

/// Accept an env payload as either a list of `[key, value]` pairs or a
/// `{ key: value }` object. Absent/null stays `None`.
fn deserialize_env_pairs<'de, D>(deserializer: D) -> Result<Option<Vec<(String, String)>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Pairs(Vec<(String, String)>),
        Map(std::collections::BTreeMap<String, String>),
    }

    Ok(match Option::<Raw>::deserialize(deserializer)? {
        None => None,
        Some(Raw::Pairs(pairs)) => Some(pairs),
        Some(Raw::Map(map)) => Some(map.into_iter().collect()),
    })
}

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u64,
    pub client_capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitializeResult {
    pub protocol_version: u64,
    pub agent_capabilities: AgentCapabilities,
    pub agent_info: AgentInfo,
    #[serde(default, deserialize_with = "deserialize_auth_methods")]
    pub auth_methods: Vec<AuthMethod>,
}

/// `authMethods` is a **sequence** in most ACP agents (live-verified against
/// `pi-acp`, which sends an array), while **id → method maps** also appear in
/// the ecosystem. Both describe the same thing, so accept either: when the
/// payload is a map, the key is the method id and the name falls back to it.
///
/// This is one of several places the ACP wire format is not self-consistent
/// (with `session/update`'s `content` — a single block on chunk updates, an
/// array on tool calls — and an auth method's `env`, which arrives as an object
/// on real agents). The tolerant parse is deliberate: refusing a real agent
/// over a shape difference is worse than normalizing it.
fn deserialize_auth_methods<'de, D>(deserializer: D) -> Result<Vec<AuthMethod>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct AuthMethodBody {
        id: Option<String>,
        name: Option<String>,
        description: Option<String>,
        r#type: Option<AuthMethodType>,
        args: Option<Vec<String>>,
        env: Option<Vec<(String, String)>>,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        List(Vec<AuthMethod>),
        Map(std::collections::BTreeMap<String, AuthMethodBody>),
    }

    match Raw::deserialize(deserializer)? {
        Raw::List(list) => Ok(list),
        Raw::Map(map) => Ok(map
            .into_iter()
            .map(|(key, body)| AuthMethod {
                id: body.id.unwrap_or_else(|| key.clone()),
                name: body.name.unwrap_or(key),
                description: body.description,
                r#type: body.r#type,
                args: body.args,
                env: body.env,
            })
            .collect()),
    }
}

// ---------------------------------------------------------------------------
// session/new and session/load
// ---------------------------------------------------------------------------

/// Wire limits for an ACP `mcpServers[]` descriptor.
///
/// The limits are deliberately on the descriptor itself rather than on the
/// eventual agent process. A descriptor is part of the ACP request and must
/// be bounded before it can be copied into a child or an HTTP request.
pub const MAX_MCP_SERVER_NAME_BYTES: usize = 128;
pub const MAX_MCP_SERVER_COMMAND_BYTES: usize = 4 * 1024;
pub const MAX_MCP_SERVER_ARG_BYTES: usize = 4 * 1024;
pub const MAX_MCP_SERVER_ENV_ENTRIES: usize = 64;
pub const MAX_MCP_SERVER_ARGS: usize = 64;
pub const MAX_MCP_SERVER_ENV_VALUE_BYTES: usize = 8 * 1024;
pub const MAX_MCP_SERVER_URL_BYTES: usize = 4 * 1024;
pub const MAX_MCP_SERVERS_PER_REQUEST: usize = 32;
pub const MAX_MCP_SERVER_HEADERS: usize = 32;
pub const MAX_MCP_SERVER_HEADER_NAME_BYTES: usize = 128;
pub const MAX_MCP_SERVER_HEADER_VALUE_BYTES: usize = 8 * 1024;
/// Provider/session identifiers are opaque, but still bounded before they are
/// used as an identity key or echoed in a protocol error.
pub const MAX_ACP_SESSION_ID_BYTES: usize = 512;
/// Short alias for callers that do not need the protocol prefix.
pub const MAX_SESSION_ID_BYTES: usize = MAX_ACP_SESSION_ID_BYTES;

/// A validation failure for an ACP MCP server descriptor.
///
/// Error messages never include a URL, header value, environment value, or
/// bearer credential. They are untrusted wire data and must not become log
/// or diagnostic material.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum McpServerError {
    #[error("MCP server name must be non-empty")]
    EmptyName,
    #[error("MCP server name exceeds the {0}-byte limit")]
    NameTooLong(usize),
    #[error("MCP server name contains an invalid character")]
    InvalidName,
    #[error("MCP server URL exceeds the {0}-byte limit")]
    UrlTooLong(usize),
    #[error("MCP server URL is invalid or uses an unsupported scheme")]
    InvalidUrl,
    #[error("MCP server URL must not contain credentials or a fragment")]
    UnsafeUrl,
    #[error("MCP server transport discriminator is invalid")]
    InvalidTransport,
    #[error("MCP server has more than {0} headers")]
    TooManyHeaders(usize),
    #[error("MCP server list has more than {0} entries")]
    TooManyServers(usize),
    #[error("MCP server header name is invalid")]
    InvalidHeaderName,
    #[error("MCP server header value is invalid or too long")]
    InvalidHeaderValue,
    #[error("MCP server has more than {0} environment entries")]
    TooManyEnvEntries(usize),
    #[error("MCP server has more than {0} arguments")]
    TooManyArgs(usize),
    #[error("MCP server environment entry is invalid")]
    InvalidEnvironment,
    #[error("MCP server bearer or credential material cannot be placed in a child environment")]
    CredentialEnvironment,
    #[error("MCP server command is required for a stdio descriptor")]
    MissingCommand,
    #[error("MCP server command or argument is invalid or too long")]
    InvalidCommand,
    #[error("MCP server command must be an absolute path")]
    RelativeCommand,
    #[error("MCP server cannot mix stdio and HTTP descriptor fields")]
    MixedTransport,
    #[error("MCP bearer is only valid for an explicit loopback Channel B lease")]
    BearerNotBound,
    #[error("MCP Channel B lease URL is not a loopback MCP endpoint")]
    InvalidLeaseUrl,
    #[error("MCP Channel B lease token is invalid")]
    InvalidLeaseToken,
}

/// Descriptive alias for callers that prefer the validation-oriented name.
pub type McpServerValidationError = McpServerError;

/// One environment variable in the official ACP stdio descriptor shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
}

impl EnvVariable {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// One HTTP header in the official ACP descriptor shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

impl HttpHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// A loopback Channel B lease identity.
///
/// The lease owner creates this from the URL and token returned by the
/// supervised MCP lease. Keeping the pair together prevents a bearer obtained
/// for one endpoint from being attached to another URL. This type deliberately
/// has no remote-bearer policy: remote bearer MCP needs a separate owner and
/// policy and is outside this ACP v1 bridge.
#[derive(Clone, PartialEq, Eq)]
pub struct McpHttpLeaseBinding {
    url: String,
    token: String,
}

impl std::fmt::Debug for McpHttpLeaseBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpHttpLeaseBinding")
            .field("url", &self.url)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

impl McpHttpLeaseBinding {
    /// Construct a binding only for a loopback `/mcp` lease URL.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Result<Self, McpServerError> {
        let url = url.into();
        let token = token.into();
        validate_lease_url(&url)?;
        validate_lease_token(&token)?;
        Ok(Self { url, token })
    }

    /// Explicitly named alias for call sites that want to make the Channel B
    /// owner visible at the construction site.
    pub fn from_loopback_lease(
        url: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, McpServerError> {
        Self::new(url, token)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

/// A stdio MCP transport descriptor.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct StdioMcpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<EnvVariable>,
}

impl std::fmt::Debug for StdioMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StdioMcpServer")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args_count", &self.args.len())
            .field(
                "env_names",
                &self.env.iter().map(|entry| &entry.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// An HTTP MCP transport descriptor.
///
/// `lease` is private so a bearer-bearing descriptor cannot be detached from
/// the exact loopback URL/token pair after construction.
#[derive(Clone)]
pub struct HttpMcpServer {
    pub name: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    lease: Option<McpHttpLeaseBinding>,
}

impl std::fmt::Debug for HttpMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpMcpServer")
            .field("name", &self.name)
            .field("url", &redacted_url(&self.url))
            .field(
                "header_names",
                &self
                    .headers
                    .iter()
                    .map(|header| &header.name)
                    .collect::<Vec<_>>(),
            )
            .field("lease_bound", &self.lease.is_some())
            .finish()
    }
}

impl std::fmt::Debug for SseMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SseMcpServer")
            .field("name", &self.name)
            .field("url", &redacted_url(&self.url))
            .field(
                "header_names",
                &self
                    .headers
                    .iter()
                    .map(|header| &header.name)
                    .collect::<Vec<_>>(),
            )
            .field("lease_bound", &self.lease.is_some())
            .finish()
    }
}

impl Default for HttpMcpServer {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            headers: Vec::new(),
            lease: None,
        }
    }
}

/// An SSE MCP transport descriptor. ACP v1 negotiates it separately from
/// HTTP; this type keeps the wire union explicit even though the current
/// Channel B owner exposes HTTP.
#[derive(Clone, Default)]
pub struct SseMcpServer {
    pub name: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    lease: Option<McpHttpLeaseBinding>,
}

impl PartialEq for HttpMcpServer {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.url == other.url && self.headers == other.headers
    }
}

impl Eq for HttpMcpServer {}

impl PartialEq for SseMcpServer {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.url == other.url && self.headers == other.headers
    }
}

impl Eq for SseMcpServer {}

/// The ACP v1 MCP transport union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServer {
    Stdio(StdioMcpServer),
    Http(HttpMcpServer),
    Sse(SseMcpServer),
}

impl Default for McpServer {
    fn default() -> Self {
        Self::Stdio(StdioMcpServer::default())
    }
}

impl McpServer {
    /// Construct a stdio descriptor using the historical `KEY=VALUE` input
    /// shape. Outbound serialization always emits `EnvVariable` objects.
    pub fn stdio(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: Vec<String>,
    ) -> Self {
        let env = env
            .into_iter()
            .map(|entry| match entry.split_once('=') {
                Some((name, value)) => EnvVariable::new(name, value),
                None => EnvVariable::new("", entry),
            })
            .collect();
        Self::stdio_with_env(name, command, args, env)
    }

    /// Construct a stdio descriptor from the official typed environment list.
    pub fn stdio_with_env(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: Vec<EnvVariable>,
    ) -> Self {
        Self::Stdio(StdioMcpServer {
            name: name.into(),
            command: command.into(),
            args,
            env,
        })
    }

    /// Construct an HTTP descriptor with an empty header list.
    pub fn http(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self::Http(HttpMcpServer {
            name: name.into(),
            url: url.into(),
            headers: Vec::new(),
            lease: None,
        })
    }

    /// Construct an HTTP descriptor with legacy `(name, value)` header pairs.
    /// The outbound shape is still an array of `HttpHeader` objects. A
    /// bearer-bearing descriptor must instead use [`Self::http_with_lease`].
    pub fn http_with_headers<I, K, V>(
        name: impl Into<String>,
        url: impl Into<String>,
        headers: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let headers = headers
            .into_iter()
            .map(|(name, value)| HttpHeader::new(name, value))
            .collect();
        Self::Http(HttpMcpServer {
            name: name.into(),
            url: url.into(),
            headers,
            lease: None,
        })
    }

    /// Construct an HTTP descriptor from the official header object list.
    pub fn http_with_header_objects(
        name: impl Into<String>,
        url: impl Into<String>,
        headers: Vec<HttpHeader>,
    ) -> Self {
        Self::Http(HttpMcpServer {
            name: name.into(),
            url: url.into(),
            headers,
            lease: None,
        })
    }

    /// Bind an HTTP descriptor to the exact loopback lease URL/token.
    pub fn http_with_lease(name: impl Into<String>, lease: &McpHttpLeaseBinding) -> Self {
        Self::Http(HttpMcpServer {
            name: name.into(),
            url: lease.url().to_string(),
            headers: vec![HttpHeader::new(
                "Authorization",
                format!("Bearer {}", lease.token()),
            )],
            lease: Some(lease.clone()),
        })
    }

    /// Convenience form for a host that has just received the two values from
    /// its supervised Channel B lease. It is intentionally named as an
    /// explicit lease operation rather than a generic bearer constructor.
    pub fn http_with_lease_values(
        name: impl Into<String>,
        url: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, McpServerError> {
        let lease = McpHttpLeaseBinding::new(url, token)?;
        Ok(Self::http_with_lease(name, &lease))
    }

    /// Compatibility constructor retained for existing callers. It is safe
    /// only for the explicit loopback lease policy; remote/private bearer
    /// endpoints are rejected before a descriptor can be built.
    pub fn http_with_bearer(
        name: impl Into<String>,
        url: impl Into<String>,
        bearer: &str,
    ) -> Result<Self, McpServerError> {
        Self::http_with_lease_values(name, url, bearer)
    }

    pub fn sse(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self::Sse(SseMcpServer {
            name: name.into(),
            url: url.into(),
            headers: Vec::new(),
            lease: None,
        })
    }

    pub fn sse_with_headers<I, K, V>(
        name: impl Into<String>,
        url: impl Into<String>,
        headers: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self::Sse(SseMcpServer {
            name: name.into(),
            url: url.into(),
            headers: headers
                .into_iter()
                .map(|(name, value)| HttpHeader::new(name, value))
                .collect(),
            lease: None,
        })
    }

    pub fn is_http(&self) -> bool {
        matches!(self, Self::Http(_))
    }

    pub fn is_sse(&self) -> bool {
        matches!(self, Self::Sse(_))
    }

    pub fn is_stdio(&self) -> bool {
        matches!(self, Self::Stdio(_))
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Stdio(server) => &server.name,
            Self::Http(server) => &server.name,
            Self::Sse(server) => &server.name,
        }
    }

    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Stdio(server) => Some(&server.command),
            Self::Http(_) | Self::Sse(_) => None,
        }
    }

    pub fn args(&self) -> &[String] {
        match self {
            Self::Stdio(server) => &server.args,
            Self::Http(_) | Self::Sse(_) => &[],
        }
    }

    pub fn env(&self) -> &[EnvVariable] {
        match self {
            Self::Stdio(server) => &server.env,
            Self::Http(_) | Self::Sse(_) => &[],
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Stdio(_) => None,
            Self::Http(server) => Some(&server.url),
            Self::Sse(server) => Some(&server.url),
        }
    }

    pub fn headers(&self) -> &[HttpHeader] {
        match self {
            Self::Stdio(_) => &[],
            Self::Http(server) => &server.headers,
            Self::Sse(server) => &server.headers,
        }
    }

    /// Validate a bounded descriptor list before it crosses the ACP wire.
    pub fn validate_list(servers: &[Self]) -> Result<(), McpServerError> {
        if servers.len() > MAX_MCP_SERVERS_PER_REQUEST {
            return Err(McpServerError::TooManyServers(MAX_MCP_SERVERS_PER_REQUEST));
        }
        for server in servers {
            server.validate_outbound()?;
        }
        Ok(())
    }

    /// Validate a descriptor before it is used for an outbound ACP request.
    pub fn validate(&self) -> Result<(), McpServerError> {
        self.validate_outbound()
    }

    fn validate_outbound(&self) -> Result<(), McpServerError> {
        self.validate_shape(true)
    }

    fn validate_inbound(&self) -> Result<(), McpServerError> {
        self.validate_shape(false)
    }

    fn validate_shape(&self, require_lease_binding: bool) -> Result<(), McpServerError> {
        match self {
            Self::Stdio(server) => {
                validate_name(&server.name)?;
                if server.command.is_empty() {
                    return Err(McpServerError::MissingCommand);
                }
                validate_command(&server.command)?;
                if !is_absolute_command_path(&server.command) {
                    return Err(McpServerError::RelativeCommand);
                }
                if server.args.len() > MAX_MCP_SERVER_ARGS {
                    return Err(McpServerError::TooManyArgs(MAX_MCP_SERVER_ARGS));
                }
                for arg in &server.args {
                    if arg.len() > MAX_MCP_SERVER_ARG_BYTES || contains_control(arg) {
                        return Err(McpServerError::InvalidCommand);
                    }
                }
                validate_environment(&server.env)
            }
            Self::Http(server) => {
                validate_name(&server.name)?;
                validate_url(&server.url)?;
                validate_headers(&server.headers)?;
                validate_bearer_binding(
                    &server.url,
                    &server.headers,
                    server.lease.as_ref(),
                    require_lease_binding,
                )
            }
            Self::Sse(server) => {
                validate_name(&server.name)?;
                validate_url(&server.url)?;
                validate_headers(&server.headers)?;
                validate_bearer_binding(
                    &server.url,
                    &server.headers,
                    server.lease.as_ref(),
                    require_lease_binding,
                )
            }
        }
    }
}

impl Serialize for McpServer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::{Error as _, SerializeMap};

        self.validate_outbound().map_err(S::Error::custom)?;
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Stdio(server) => {
                // The v1 stdio form is the legacy/default arm and has no
                // discriminator; HTTP/SSE arms carry their `type` tag.
                map.serialize_entry("name", &server.name)?;
                map.serialize_entry("command", &server.command)?;
                map.serialize_entry("args", &server.args)?;
                map.serialize_entry("env", &server.env)?;
            }
            Self::Http(server) => {
                map.serialize_entry("type", "http")?;
                map.serialize_entry("name", &server.name)?;
                map.serialize_entry("url", &server.url)?;
                map.serialize_entry("headers", &server.headers)?;
            }
            Self::Sse(server) => {
                map.serialize_entry("type", "sse")?;
                map.serialize_entry("name", &server.name)?;
                map.serialize_entry("url", &server.url)?;
                map.serialize_entry("headers", &server.headers)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for McpServer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Debug, Deserialize, Default)]
        #[serde(default)]
        struct Wire {
            #[serde(rename = "type")]
            transport: Option<String>,
            name: String,
            command: Option<String>,
            args: Vec<String>,
            #[serde(deserialize_with = "deserialize_mcp_env")]
            env: Option<Vec<EnvVariable>>,
            url: Option<String>,
            #[serde(deserialize_with = "deserialize_mcp_headers")]
            headers: Option<Vec<HttpHeader>>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let has_stdio_fields =
            wire.command.is_some() || !wire.args.is_empty() || wire.env.is_some();
        let has_http_fields = wire.url.is_some() || wire.headers.is_some();
        if (wire.transport.as_deref() == Some("http") || wire.transport.as_deref() == Some("sse"))
            && has_stdio_fields
        {
            return Err(serde::de::Error::custom(McpServerError::MixedTransport));
        }
        if wire.transport.is_none() && has_stdio_fields && has_http_fields {
            return Err(serde::de::Error::custom(McpServerError::MixedTransport));
        }
        if wire.transport.as_deref() == Some("stdio") && has_http_fields {
            return Err(serde::de::Error::custom(McpServerError::MixedTransport));
        }
        let server = match wire.transport.as_deref() {
            Some("stdio") | None if wire.command.is_some() || wire.url.is_none() => {
                Self::Stdio(StdioMcpServer {
                    name: wire.name,
                    command: wire.command.unwrap_or_default(),
                    args: wire.args,
                    env: wire.env.unwrap_or_default(),
                })
            }
            Some("http") | None if wire.url.is_some() => Self::Http(HttpMcpServer {
                name: wire.name,
                url: wire.url.unwrap_or_default(),
                headers: wire.headers.unwrap_or_default(),
                lease: None,
            }),
            Some("sse") => Self::Sse(SseMcpServer {
                name: wire.name,
                url: wire.url.unwrap_or_default(),
                headers: wire.headers.unwrap_or_default(),
                lease: None,
            }),
            Some("http") | Some("stdio") | Some(_) => {
                return Err(serde::de::Error::custom(McpServerError::InvalidTransport));
            }
            None => {
                return Err(serde::de::Error::custom(McpServerError::InvalidTransport));
            }
        };
        server
            .validate_inbound()
            .map_err(serde::de::Error::custom)?;
        Ok(server)
    }
}

fn deserialize_mcp_env<'de, D>(deserializer: D) -> Result<Option<Vec<EnvVariable>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        List(Vec<EnvVariable>),
        LegacyList(Vec<String>),
        Map(BTreeMap<String, String>),
    }

    Ok(match Option::<Raw>::deserialize(deserializer)? {
        None => None,
        Some(Raw::List(values)) => Some(values),
        Some(Raw::LegacyList(values)) => Some(
            values
                .into_iter()
                .map(|entry| match entry.split_once('=') {
                    Some((name, value)) => EnvVariable::new(name, value),
                    None => EnvVariable::new("", entry),
                })
                .collect(),
        ),
        Some(Raw::Map(values)) => Some(
            values
                .into_iter()
                .map(|(name, value)| EnvVariable::new(name, value))
                .collect(),
        ),
    })
}

fn deserialize_mcp_headers<'de, D>(deserializer: D) -> Result<Option<Vec<HttpHeader>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        List(Vec<HttpHeader>),
        Map(BTreeMap<String, String>),
    }

    Ok(match Option::<Raw>::deserialize(deserializer)? {
        None => None,
        Some(Raw::List(values)) => Some(values),
        Some(Raw::Map(values)) => Some(
            values
                .into_iter()
                .map(|(name, value)| HttpHeader::new(name, value))
                .collect(),
        ),
    })
}

fn contains_control(value: &str) -> bool {
    value.chars().any(|character| character.is_control())
}

fn is_absolute_command_path(value: &str) -> bool {
    if Path::new(value).is_absolute() {
        return true;
    }
    // Validate Windows absolute forms even when the schema is being built on
    // a Unix host (the descriptor may be persisted for a Windows agent).
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/'))
        || value.starts_with('/')
        || value.starts_with('\\')
}

fn redacted_url(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return "[invalid-url]".to_string();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string()
}

fn validate_name(name: &str) -> Result<(), McpServerError> {
    if name.is_empty() {
        return Err(McpServerError::EmptyName);
    }
    if name.len() > MAX_MCP_SERVER_NAME_BYTES {
        return Err(McpServerError::NameTooLong(MAX_MCP_SERVER_NAME_BYTES));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(McpServerError::InvalidName);
    }
    Ok(())
}

fn validate_url(raw: &str) -> Result<(), McpServerError> {
    if raw.len() > MAX_MCP_SERVER_URL_BYTES {
        return Err(McpServerError::UrlTooLong(MAX_MCP_SERVER_URL_BYTES));
    }
    if contains_control(raw) {
        return Err(McpServerError::InvalidUrl);
    }
    let parsed = url::Url::parse(raw).map_err(|_| McpServerError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(McpServerError::InvalidUrl);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(McpServerError::UnsafeUrl);
    }
    if let Some(query) = parsed.query() {
        let carries_credential = query.split('&').any(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = key.to_ascii_lowercase();
            let value = value.to_ascii_lowercase();
            key.contains("token")
                || key.contains("secret")
                || key.contains("password")
                || key.contains("authorization")
                || value.starts_with("bearer%20")
                || value.starts_with("bearer+")
                || value.starts_with("bearer ")
        });
        if carries_credential {
            return Err(McpServerError::UnsafeUrl);
        }
    }
    Ok(())
}

fn validate_headers(headers: &[HttpHeader]) -> Result<(), McpServerError> {
    if headers.len() > MAX_MCP_SERVER_HEADERS {
        return Err(McpServerError::TooManyHeaders(MAX_MCP_SERVER_HEADERS));
    }
    let mut names = BTreeMap::new();
    for header in headers {
        if header.name.is_empty()
            || header.name.len() > MAX_MCP_SERVER_HEADER_NAME_BYTES
            || !header.name.bytes().all(is_http_token_byte)
        {
            return Err(McpServerError::InvalidHeaderName);
        }
        let canonical = header.name.to_ascii_lowercase();
        if names.insert(canonical, ()).is_some() {
            return Err(McpServerError::InvalidHeaderName);
        }
        if header.value.is_empty()
            || header.value.len() > MAX_MCP_SERVER_HEADER_VALUE_BYTES
            || contains_control(&header.value)
        {
            return Err(McpServerError::InvalidHeaderValue);
        }
    }
    Ok(())
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn validate_command(command: &str) -> Result<(), McpServerError> {
    if command.len() > MAX_MCP_SERVER_COMMAND_BYTES || contains_control(command) {
        return Err(McpServerError::InvalidCommand);
    }
    Ok(())
}

fn validate_environment(entries: &[EnvVariable]) -> Result<(), McpServerError> {
    if entries.len() > MAX_MCP_SERVER_ENV_ENTRIES {
        return Err(McpServerError::TooManyEnvEntries(
            MAX_MCP_SERVER_ENV_ENTRIES,
        ));
    }
    for entry in entries {
        if entry.name.is_empty()
            || entry.name.len() > 128
            || !entry
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(McpServerError::InvalidEnvironment);
        }
        if entry.value.len() > MAX_MCP_SERVER_ENV_VALUE_BYTES || contains_control(&entry.value) {
            return Err(McpServerError::InvalidEnvironment);
        }
        let upper = entry.name.to_ascii_uppercase();
        if upper == "AUTHORIZATION"
            || upper.contains("TOKEN")
            || upper.contains("SECRET")
            || upper.contains("PASSWORD")
            || upper.contains("APIKEY")
            || upper.contains("API_KEY")
            || upper.contains("CREDENTIAL")
            || upper.contains("PRIVATE_KEY")
            || entry.value.to_ascii_lowercase().starts_with("bearer ")
        {
            return Err(McpServerError::CredentialEnvironment);
        }
    }
    Ok(())
}

fn validate_lease_url(raw: &str) -> Result<(), McpServerError> {
    validate_url(raw).map_err(|_| McpServerError::InvalidLeaseUrl)?;
    let parsed = url::Url::parse(raw).map_err(|_| McpServerError::InvalidLeaseUrl)?;
    let is_loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "::1" | "[::1]"));
    if !is_loopback
        || parsed.scheme() != "http"
        || parsed.path() != "/mcp"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port().is_none()
    {
        return Err(McpServerError::InvalidLeaseUrl);
    }
    Ok(())
}

fn validate_lease_token(token: &str) -> Result<(), McpServerError> {
    if token.is_empty()
        || token.len() > MAX_MCP_SERVER_HEADER_VALUE_BYTES
        || token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(McpServerError::InvalidLeaseToken);
    }
    Ok(())
}

fn validate_bearer_binding(
    url: &str,
    headers: &[HttpHeader],
    lease: Option<&McpHttpLeaseBinding>,
    require_lease_binding: bool,
) -> Result<(), McpServerError> {
    if headers.iter().any(|header| {
        !header.name.eq_ignore_ascii_case("authorization")
            && header.value.to_ascii_lowercase().starts_with("bearer ")
    }) {
        return Err(McpServerError::BearerNotBound);
    }
    let bearer = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("authorization"));
    let Some(header) = bearer else {
        if lease.is_some() {
            return Err(McpServerError::BearerNotBound);
        }
        return Ok(());
    };
    if !require_lease_binding {
        return Ok(());
    }
    let lease = lease.ok_or(McpServerError::BearerNotBound)?;
    if url != lease.url() || header.value != format!("Bearer {}", lease.token()) {
        return Err(McpServerError::BearerNotBound);
    }
    Ok(())
}

fn deserialize_config_options<'de, D>(deserializer: D) -> Result<Vec<ConfigOption>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<ConfigOption>>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_mcp_server_list<'de, D>(deserializer: D) -> Result<Vec<McpServer>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let servers = Vec::<McpServer>::deserialize(deserializer)?;
    if servers.len() > MAX_MCP_SERVERS_PER_REQUEST {
        return Err(serde::de::Error::custom(McpServerError::TooManyServers(
            MAX_MCP_SERVERS_PER_REQUEST,
        )));
    }
    Ok(servers)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionNewParams {
    pub cwd: String,
    #[serde(deserialize_with = "deserialize_mcp_server_list")]
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionLoadParams {
    pub session_id: String,
    pub cwd: String,
    #[serde(deserialize_with = "deserialize_mcp_server_list")]
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionLoadResult {
    /// ACP v1 permits an omitted/empty load result. When present, this is an
    /// optional agent extension identifier; the requested provider id remains
    /// the fallback identity. The client validates the surrounding JSON-RPC
    /// envelope before deserializing this value; a missing `result` member is
    /// not equivalent to an explicit `result: null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional agent-owned configuration returned by a few v1 adapters.
    #[serde(default, deserialize_with = "deserialize_config_options")]
    pub config_options: Vec<ConfigOption>,
    /// A tolerant extension for adapters that inline replay notifications in
    /// the result instead of sending them as JSON-RPC notifications.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<SessionUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewResult {
    pub session_id: String,
    /// Agent-owned session configuration, including model selectors when the
    /// agent exposes them. Empty means the agent manages its own model state.
    ///
    /// Optional on the wire: the protocol says the Agent **MAY** return
    /// `configOptions`, so an agent that does not must not fail `session/new`.
    #[serde(default)]
    pub config_options: Vec<ConfigOption>,
}

/// One ACP session-level configuration selector. The agent owns this
/// vocabulary and the current value; AgentCowork must not substitute its native
/// provider/model catalog for it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub r#type: String,
    pub current_value: serde_json::Value,
    pub options: Vec<ConfigOptionValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigOptionValue {
    pub value: serde_json::Value,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetConfigOptionParams {
    pub session_id: String,
    pub config_id: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetConfigOptionResult {
    pub config_options: Vec<ConfigOption>,
}

// ---------------------------------------------------------------------------
// session/prompt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptContent {
    #[serde(rename = "text")]
    Text { text: String },
    /// A workspace file embedded as an ACP resource. Agents must advertise
    /// `promptCapabilities.embeddedContext` before the client sends this
    /// capability-gated block.
    #[serde(rename = "resource")]
    Resource { resource: EmbeddedResource },
    // image/audio/other v1 content is capability-gated; unknown variants do
    // not break a peer that sends them.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedResource {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

impl PromptContent {
    pub fn text(s: impl Into<String>) -> Self {
        PromptContent::Text { text: s.into() }
    }

    pub fn resource(
        uri: impl Into<String>,
        mime_type: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        PromptContent::Resource {
            resource: EmbeddedResource {
                uri: uri.into(),
                mime_type: mime_type.into(),
                text: text.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

/// Token usage an agent **reported** for one prompt turn (P71.4).
///
/// This is an *observation*, never a computation: every field is what the agent
/// said, and an absent field is unknown rather than zero (`ARCH/ROUTING.md` §5,
/// **I15**). `cached_read_tokens` and `cached_write_tokens` stay separate
/// because they are billed differently — collapsing them would invent
/// precision the agent did not report.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptUsage {
    /// Prompt (input) tokens the agent charged this turn.
    pub input_tokens: u64,
    /// Completion (output) tokens the agent produced.
    pub output_tokens: u64,
    /// Prompt-cache **reads** — a subset of the input the agent billed cheaper.
    pub cached_read_tokens: u64,
    /// Prompt-cache **writes** — billed separately from a read.
    pub cached_write_tokens: u64,
    /// The turn's own cost in USD when the agent prices its turns. `None` means
    /// the agent reported no cost; it is never estimated here.
    pub cost_usd: Option<f64>,
}

impl PromptUsage {
    /// Whether the agent reported any token count at all. A turn whose usage
    /// was entirely absent must not read as a measured zero.
    pub fn reported(&self) -> bool {
        self.input_tokens > 0
            || self.output_tokens > 0
            || self.cached_read_tokens > 0
            || self.cached_write_tokens > 0
            || self.cost_usd.is_some()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionPromptResult {
    pub stop_reason: StopReason,
    /// P71.4 — the agent's own usage report for this turn, when it sends one.
    /// Absent is the normal case for agents that report nothing; the ledger
    /// records that as *unreported*, never as zero tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<PromptUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Turn ended normally (the common case).
    #[default]
    EndTurn,
    MaxTokens,
    Cancelled,
    Refusal,
    NotImplemented,
    Error,
    /// Unknown/forward-compat stop reason (`_`-prefixed or new).
    #[serde(other)]
    Other,
}

impl StopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::Cancelled => "cancelled",
            StopReason::Refusal => "refusal",
            StopReason::NotImplemented => "not_implemented",
            StopReason::Error => "error",
            StopReason::Other => "other",
        }
    }
}

// ---------------------------------------------------------------------------
// Tool kinds (the shared permission taxonomy — doc 45 §1.4, F9)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    #[serde(other)]
    Other,
}

impl ToolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Delete => "delete",
            ToolKind::Move => "move",
            ToolKind::Search => "search",
            ToolKind::Execute => "execute",
            ToolKind::Think => "think",
            ToolKind::Fetch => "fetch",
            ToolKind::Other => "other",
        }
    }

    /// Map an ACP tool kind onto our Guard-2 operation class, so a tool call
    /// arriving over ACP routes into the same policy engine as native tools.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            ToolKind::Edit | ToolKind::Delete | ToolKind::Move | ToolKind::Execute
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    #[default]
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Other,
}

// ---------------------------------------------------------------------------
// session/update (agent → client notification)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentBlock {
    pub r#type: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Location {
    pub r#type: String,
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<TextRange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Position {
    pub line: u64,
    pub character: u64,
}

/// A parsed `session/update` notification. `session_update` is the
/// discriminator string; we keep the fields we act on (tool calls, plans,
/// available commands, mode changes) and ignore the rest (forward-compat).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionUpdate {
    pub session_id: String,
    /// `tool_call` | `tool_call_update` | `agent_message_chunk` |
    /// `plan` | `available_commands_update` | `mode_change` | …
    pub session_update: String,
    pub tool_call_id: String,
    pub title: String,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    /// ACP is inconsistent about this field: the **chunk** updates
    /// (`agent_message_chunk` / `agent_thought_chunk` / `user_message_chunk`)
    /// carry a *single* `ContentBlock`, while `tool_call` /
    /// `tool_call_update` carry an *array*. Accepting only the array shape
    /// silently drops every streamed token from a spec-conformed agent
    /// (opencode sends the object form), so both are normalised to a list
    /// here rather than at each consumer.
    #[serde(default, deserialize_with = "content_blocks")]
    pub content: Vec<ContentBlock>,
    pub locations: Vec<Location>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
    /// P53.1 — the agent's live slash vocabulary. Present only on
    /// `available_commands_update`; empty otherwise. Stored per ACP handle and
    /// served to the composer — never a hardcoded per-harness table.
    #[serde(default)]
    pub available_commands: Vec<AvailableCommand>,
    /// Complete agent-owned configuration after a config-option update.
    #[serde(default)]
    pub config_options: Vec<ConfigOption>,
}

/// Deserialize `SessionUpdate::content` from **either** a single
/// `ContentBlock` or an array of them (see the field docs). `null`/absent
/// yields an empty list, preserving the previous `default` behaviour.
fn content_blocks<'de, D>(de: D) -> Result<Vec<ContentBlock>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<ContentBlock>),
        One(ContentBlock),
    }
    Ok(match Option::<OneOrMany>::deserialize(de)? {
        Some(OneOrMany::Many(v)) => v,
        Some(OneOrMany::One(b)) => vec![b],
        None => Vec::new(),
    })
}

/// One live slash command advertised by the agent
/// (`available_commands_update.availableCommands[]` — ACP slash-commands
/// surface: `{name, description, input?}`, no leading `/`; the client
/// displays `/name` and submits `/name args` as `session/prompt` text).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
    /// Optional JSON-schema-ish input hint. Opaque to us (rendered as help
    /// text); never executed.
    pub input: Option<serde_json::Value>,
}

impl SessionUpdate {
    pub fn is_tool_call(&self) -> bool {
        self.session_update == "tool_call"
    }

    pub fn is_tool_call_update(&self) -> bool {
        self.session_update == "tool_call_update"
    }

    /// P53.1 — this update carries the agent's live slash vocabulary.
    pub fn is_available_commands_update(&self) -> bool {
        self.session_update == "available_commands_update"
    }

    pub fn is_config_option_update(&self) -> bool {
        self.session_update == "config_option_update"
    }
}

// ---------------------------------------------------------------------------
// session/request_permission (agent → client request) — the Guard-2 seam
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: Option<ToolKind>,
    pub content: Vec<ContentBlock>,
    pub locations: Vec<Location>,
    pub raw_input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: PermissionOptionKind,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionRequestParams {
    pub session_id: String,
    pub tool_call: ToolCall,
    pub options: Vec<PermissionOption>,
}

// ---------------------------------------------------------------------------
// Authentication (doc 45 §1.5 — `authenticate` / `logout`, auth_required)
// ---------------------------------------------------------------------------

/// The authentication method type (the `type` field on [`AuthMethod`]).
/// `Agent` is the default: the agent drives its own login flow (prints a URL,
/// opens its own browser, waits for the user). `Url` returns a URL the client
/// opens in the system browser; the client calls `authenticate` again after
/// the user completes login. `Terminal` is an out-of-band interactive launch
/// (not driven over the ACP connection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethodType {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "url")]
    Url,
    #[serde(rename = "terminal")]
    Terminal,
}

impl AuthMethodType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMethodType::Agent => "agent",
            AuthMethodType::Url => "url",
            AuthMethodType::Terminal => "terminal",
        }
    }
}

/// The `authenticate` request: pick one of the methods advertised in the
/// `initialize` response's `authMethods`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticateParams {
    pub method_id: String,
}

/// The `authenticate` response. `{}` on success for agent-type methods; a
/// `url` for url-type methods (the client opens it in the system browser, the
/// user completes login, then the client calls `authenticate` again).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthenticateResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The client's decision for a permission request. `allow` + `option_id`
/// selects one of the offered options; when `option_id` is `None` the session
/// synthesizes a default allow_once/reject_once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow { option_id: Option<String> },
    Deny { option_id: Option<String> },
}

impl PermissionDecision {
    pub fn allow() -> Self {
        PermissionDecision::Allow { option_id: None }
    }

    pub fn deny() -> Self {
        PermissionDecision::Deny { option_id: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOutcome {
    pub option_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResult {
    pub outcome: PermissionOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_result_roundtrips_camelcase() {
        let r = InitializeResult {
            protocol_version: 1,
            agent_capabilities: AgentCapabilities {
                load_session: true,
                ..Default::default()
            },
            agent_info: AgentInfo {
                name: "claude-acp".into(),
                title: "Claude Agent".into(),
                version: "0.66.0".into(),
            },
            auth_methods: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["protocolVersion"], 1);
        assert_eq!(v["agentCapabilities"]["loadSession"], true);
        assert_eq!(v["agentInfo"]["name"], "claude-acp");

        let back: InitializeResult = serde_json::from_value(v).unwrap();
        assert!(back.agent_capabilities.load_session);
    }

    #[test]
    fn auth_methods_accept_the_array_shape() {
        // Most agents: a sequence of methods, each carrying its own id.
        let v = serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": { "name": "a", "title": "A", "version": "1" },
            "authMethods": [
                { "id": "login", "name": "Log in", "type": "url" }
            ]
        });
        let r: InitializeResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.auth_methods.len(), 1);
        assert_eq!(r.auth_methods[0].id, "login");
        assert_eq!(r.auth_methods[0].r#type, Some(AuthMethodType::Url));
    }

    #[test]
    fn auth_methods_accept_the_map_shape_pi_acp_sends() {
        // pi-acp sends `authMethods` as a map of methodId → method (observed
        // live). The key becomes the id; the name falls back to the key.
        let v = serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": { "name": "pi-acp", "title": "pi", "version": "1" },
            "authMethods": {
                "terminal": { "type": "terminal", "description": "Run pi in a terminal" }
            }
        });
        let r: InitializeResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.auth_methods.len(), 1);
        assert_eq!(r.auth_methods[0].id, "terminal");
        assert_eq!(r.auth_methods[0].name, "terminal");
        assert_eq!(r.auth_methods[0].r#type, Some(AuthMethodType::Terminal));
        assert_eq!(
            r.auth_methods[0].description.as_deref(),
            Some("Run pi in a terminal")
        );
    }

    #[test]
    fn terminal_auth_method_env_parses_as_an_object_or_a_pair_list() {
        // Exactly what `pi-acp` sends (live-captured 2026-09-14): a sequence of
        // methods whose `env` is an object, not a pair list.
        let v = serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "mcpCapabilities": { "http": false, "sse": false },
                "promptCapabilities": { "audio": false, "embeddedContext": false, "image": true },
                "sessionCapabilities": { "delete": {}, "list": {} }
            },
            "agentInfo": { "name": "pi-acp", "title": "pi ACP adapter", "version": "0.0.33" },
            "authMethods": [{
                "args": ["--terminal-login"],
                "description": "Start pi in an interactive terminal to configure API keys or login",
                "env": {},
                "id": "pi_terminal_login",
                "name": "Launch pi in the terminal",
                "type": "terminal"
            }]
        });
        let r: InitializeResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.protocol_version, 1);
        assert!(r.agent_capabilities.load_session);
        assert!(r.agent_capabilities.prompt_capabilities.image);
        assert_eq!(r.agent_info.name, "pi-acp");
        assert_eq!(r.auth_methods.len(), 1);
        assert_eq!(r.auth_methods[0].id, "pi_terminal_login");
        assert_eq!(r.auth_methods[0].r#type, Some(AuthMethodType::Terminal));
        assert_eq!(r.auth_methods[0].env.as_deref(), Some(&[][..]));

        // The pair-list form still parses.
        let pairs = serde_json::json!({ "env": [ ["A", "1"], ["B", "2"] ] });
        let m: AuthMethod = serde_json::from_value(serde_json::json!({
            "id": "x", "name": "X", "env": pairs["env"].clone()
        }))
        .unwrap();
        assert_eq!(
            m.env.as_deref(),
            Some(&[("A".into(), "1".into()), ("B".into(), "2".into())][..])
        );

        // And an object map becomes pairs.
        let m2: AuthMethod = serde_json::from_value(serde_json::json!({
            "id": "y", "name": "Y", "env": { "K": "V" }
        }))
        .unwrap();
        assert_eq!(m2.env.as_deref(), Some(&[("K".into(), "V".into())][..]));
    }

    #[test]
    fn auth_methods_tolerate_being_absent_or_empty() {
        let v = serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": { "name": "a", "title": "A", "version": "1" },
            "authMethods": {}
        });
        let r: InitializeResult = serde_json::from_value(v).unwrap();
        assert!(r.auth_methods.is_empty());
    }

    #[test]
    fn tool_kind_and_stop_reason_use_snake_case() {
        assert_eq!(serde_json::to_value(ToolKind::Edit).unwrap(), "edit");
        assert_eq!(
            serde_json::to_value(StopReason::EndTurn).unwrap(),
            "end_turn"
        );
        assert_eq!(
            serde_json::from_value::<ToolKind>(serde_json::json!("delete")).unwrap(),
            ToolKind::Delete
        );
        assert!(ToolKind::Edit.is_mutation());
        assert!(!ToolKind::Read.is_mutation());
    }

    #[test]
    fn session_update_parses_tool_call_discriminator() {
        let v = serde_json::json!({
            "sessionId": "s1",
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "title": "Edit main.rs",
            "kind": "edit",
            "status": "in_progress"
        });
        let u: SessionUpdate = serde_json::from_value(v).unwrap();
        assert!(u.is_tool_call());
        assert_eq!(u.tool_call_id, "tc1");
        assert_eq!(u.kind, Some(ToolKind::Edit));
        assert_eq!(u.status, Some(ToolStatus::InProgress));
    }

    #[test]
    fn permission_request_roundtrips() {
        let p = PermissionRequestParams {
            session_id: "s1".into(),
            tool_call: ToolCall {
                tool_call_id: "tc1".into(),
                title: "Write /w/a.rs".into(),
                kind: Some(ToolKind::Edit),
                ..Default::default()
            },
            options: vec![PermissionOption {
                option_id: "allow-once".into(),
                kind: PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            }],
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["sessionId"], "s1");
        assert_eq!(v["toolCall"]["toolCallId"], "tc1");
        assert_eq!(v["options"][0]["optionId"], "allow-once");
    }

    #[test]
    fn prompt_content_text() {
        let c = PromptContent::text("hello");
        let v = serde_json::to_value(c).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
    }

    #[test]
    fn prompt_content_resource_uses_acp_embedded_shape() {
        let c = PromptContent::resource("file:///workspace/a.rs", "text/plain", "fn main() {}");
        let v = serde_json::to_value(c).unwrap();
        assert_eq!(v["type"], "resource");
        assert_eq!(v["resource"]["uri"], "file:///workspace/a.rs");
        assert_eq!(v["resource"]["mimeType"], "text/plain");
        assert_eq!(v["resource"]["text"], "fn main() {}");
    }

    #[test]
    fn available_commands_update_parses_live_slash_vocab() {
        // P53.1 — the exact ACP wire shape: `sessionUpdate:
        // "available_commands_update"` + `availableCommands[]`. `input` is
        // optional; a missing list degrades to empty, never an error.
        let v = serde_json::json!({
            "sessionId": "s1",
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                {"name": "compact", "description": "Compact the conversation"},
                {"name": "review", "description": "Review the diff",
                 "input": {"type": "object"}}
            ]
        });
        let u: SessionUpdate = serde_json::from_value(v).unwrap();
        assert!(u.is_available_commands_update());
        assert_eq!(u.available_commands.len(), 2);
        assert_eq!(u.available_commands[0].name, "compact");
        assert_eq!(
            u.available_commands[0].description,
            "Compact the conversation"
        );
        assert!(u.available_commands[0].input.is_none());
        assert!(u.available_commands[1].input.is_some());

        // A tool_call update carries no commands (empty, not absent-error).
        let t: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "sessionUpdate": "agent_message_chunk",
        }))
        .unwrap();
        assert!(!t.is_available_commands_update());
        assert!(t.available_commands.is_empty());
    }

    /// The exact shape opencode sends for a streamed token: a **single**
    /// `content` object, not an array. A `Vec`-only field rejected this with
    /// "invalid type: map, expected a sequence", which failed the whole prompt
    /// turn — the streaming path of every spec-conformed agent.
    #[test]
    fn chunk_content_may_be_a_single_object() {
        let u: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionId": "ses_1",
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": "thinking out loud" }
        }))
        .unwrap();
        assert_eq!(u.content.len(), 1);
        assert_eq!(u.content[0].r#type, "text");
        assert_eq!(u.content[0].text, "thinking out loud");

        // The array shape (tool_call / tool_call_update) still parses, and
        // order is preserved.
        let many: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionId": "ses_1",
            "sessionUpdate": "tool_call",
            "content": [
                { "type": "text", "text": "first" },
                { "type": "text", "text": "second" }
            ]
        }))
        .unwrap();
        assert_eq!(many.content.len(), 2);
        assert_eq!(many.content[0].text, "first");
        assert_eq!(many.content[1].text, "second");

        // Absent and explicit-null both stay empty (the previous default).
        for body in [
            serde_json::json!({ "sessionUpdate": "agent_message_chunk" }),
            serde_json::json!({ "sessionUpdate": "agent_message_chunk", "content": null }),
        ] {
            let u: SessionUpdate = serde_json::from_value(body).unwrap();
            assert!(u.content.is_empty());
        }
    }

    #[test]
    fn mcp_http_descriptor_uses_the_official_union_shape() {
        let lease =
            McpHttpLeaseBinding::new("http://127.0.0.1:43123/mcp", "fixture-token").unwrap();
        let server = McpServer::http_with_lease("shared", &lease);
        let value = serde_json::to_value(&server).unwrap();
        assert_eq!(value["type"], "http");
        assert_eq!(value["name"], "shared");
        assert_eq!(value["url"], "http://127.0.0.1:43123/mcp");
        assert_eq!(value["headers"][0]["name"], "Authorization");
        assert_eq!(value["headers"][0]["value"], "Bearer fixture-token");
        assert!(value.get("command").is_none());
        assert!(value.get("args").is_none());
        assert!(value.get("env").is_none());
        assert!(!format!("{server:?}").contains("fixture-token"));

        let back: McpServer = serde_json::from_value(value).unwrap();
        assert!(back.is_http());
        assert_eq!(server, back);
    }

    #[test]
    fn mcp_stdio_descriptor_uses_env_objects_and_absolute_command() {
        let server = McpServer::stdio(
            "local",
            "/usr/local/bin/agent-mcp",
            vec!["--stdio".into()],
            vec!["LANG=C".into()],
        );
        let value = serde_json::to_value(&server).unwrap();
        assert!(value.get("type").is_none());
        assert_eq!(value["command"], "/usr/local/bin/agent-mcp");
        assert_eq!(value["args"][0], "--stdio");
        assert_eq!(value["env"][0]["name"], "LANG");
        assert_eq!(value["env"][0]["value"], "C");
        assert!(value.get("url").is_none());
        assert!(value.get("headers").is_none());
        assert!(!format!("{server:?}").contains("LANG=C"));
        assert!(server.validate().is_ok());

        let relative = McpServer::stdio("local", "agent-mcp", vec![], vec![]);
        assert!(matches!(
            relative.validate(),
            Err(McpServerError::RelativeCommand)
        ));

        let malformed_env = McpServer::stdio(
            "local",
            "/usr/local/bin/agent-mcp",
            vec![],
            vec!["MISSING_SEPARATOR".into()],
        );
        assert!(matches!(
            malformed_env.validate(),
            Err(McpServerError::InvalidEnvironment)
        ));

        let bearer_env = McpServer::stdio(
            "local",
            "/usr/local/bin/agent-mcp",
            vec![],
            vec!["MCP_TOKEN=fixture-secret".into()],
        );
        assert!(matches!(
            bearer_env.validate(),
            Err(McpServerError::CredentialEnvironment)
        ));
    }

    #[test]
    fn mcp_descriptor_validation_rejects_unsafe_or_unbound_fields() {
        let cases = [
            McpServer::stdio("bad name", "/usr/bin/agent", vec![], vec![]),
            McpServer::stdio("bad", "/usr/bin/agent\nnext", vec![], vec![]),
            McpServer::http("bad", "file:///tmp/mcp"),
            McpServer::http_with_headers(
                "bad",
                "https://example.test/mcp",
                [("X-Bad", "line\nbreak")],
            ),
            McpServer::http_with_headers(
                "bad",
                "https://example.test/mcp",
                [("Authorization", "Bearer remote-secret")],
            ),
        ];
        for case in cases {
            assert!(case.validate().is_err(), "descriptor should be refused");
        }
        assert!(matches!(
            McpServer::http_with_bearer("bad", "https://example.test/mcp", "remote-secret"),
            Err(McpServerError::InvalidLeaseUrl)
        ));
        assert!(matches!(
            McpServer::http_with_bearer("bad", "http://127.0.0.1:43123/mcp", "token"),
            Ok(_)
        ));
    }

    #[test]
    fn session_load_params_use_the_documented_camel_case_shape() {
        let params = SessionLoadParams {
            session_id: "provider-1".into(),
            cwd: "/workspace".into(),
            mcp_servers: vec![McpServer::http("shared", "http://127.0.0.1:1/mcp")],
        };
        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["sessionId"], "provider-1");
        assert_eq!(value["cwd"], "/workspace");
        assert_eq!(value["mcpServers"][0]["type"], "http");
        assert_eq!(value["mcpServers"][0]["url"], "http://127.0.0.1:1/mcp");
    }

    #[test]
    fn official_mcp_descriptors_round_trip() {
        let stdio: McpServer = serde_json::from_value(serde_json::json!({
            "type": "stdio",
            "name": "local",
            "command": "/usr/local/bin/agent-mcp",
            "args": ["--stdio"],
            "env": [{ "name": "LANG", "value": "C" }]
        }))
        .unwrap();
        assert!(stdio.is_stdio());
        assert!(matches!(stdio, McpServer::Stdio(ref server)
            if server.env[0].name == "LANG" && server.env[0].value == "C"));

        let http: McpServer = serde_json::from_value(serde_json::json!({
            "type": "http",
            "name": "remote",
            "url": "https://example.test/mcp",
            "headers": [{ "name": "X-Test", "value": "yes" }]
        }))
        .unwrap();
        assert!(http.is_http());
        let value = serde_json::to_value(http).unwrap();
        assert_eq!(value["type"], "http");
        assert_eq!(value["headers"][0]["name"], "X-Test");
    }
}
