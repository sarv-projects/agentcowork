//! Guard-bound HTTP transport for vault-owned requests.
//!
//! The vault owns credentials, so its outbound OAuth transport must not follow
//! a redirect or allow the HTTP library to perform a second DNS lookup after
//! the destination has been checked. This module pins the checked address set
//! into a one-request `ureq` agent. Each agent is scoped to one checked origin
//! and disables redirects so credentials cannot be forwarded to another host.
//! This does not yet provide asynchronous cancellation or a deadline for the
//! blocking system resolver.

use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

/// Upper bound for OAuth token/error response bodies.
pub(crate) const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
/// Upper bound for a non-streaming provider completion body.
pub(crate) const MAX_PROVIDER_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// Build a one-request agent whose resolver returns only addresses checked by
/// the canonical Guard destination floor. Automatic redirects are disabled,
/// so that the pinned target cannot silently change origins.
pub(crate) fn agent_for(url: &str) -> Result<ureq::Agent, EgressSetupError> {
    let parsed = parse_endpoint(url)?;
    if parsed.scheme() == "http"
        && !matches!(
            parsed
                .host()
                .map(|host| agentcowork_guard::netfloor::classify_url_host(&host)),
            Some(agentcowork_guard::NetClass::Loopback)
        )
    {
        return Err(EgressSetupError {
            url: safe_origin(&parsed),
            reason: "cleartext_oauth_endpoint",
        });
    }
    build_agent(
        url,
        agentcowork_guard::NetPolicy::default(),
        Some(Duration::from_secs(15)),
    )
}

/// Build a checked provider agent using the endpoint's declared egress policy.
/// `overall_timeout` is used for bounded metadata probes. Streaming calls use
/// per-connect/read/write timeouts so a healthy long generation can continue.
pub(crate) fn provider_agent_for(
    url: &str,
    policy: agentcowork_guard::NetPolicy,
    overall_timeout: Option<Duration>,
) -> Result<ureq::Agent, EgressSetupError> {
    build_agent(url, policy, overall_timeout)
}

fn parse_endpoint(url: &str) -> Result<url::Url, EgressSetupError> {
    let parsed = url::Url::parse(url).map_err(|_| EgressSetupError {
        url: "<invalid-url>".into(),
        reason: "malformed",
    })?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(EgressSetupError {
            url: safe_origin(&parsed),
            reason: "invalid_endpoint",
        });
    }
    Ok(parsed)
}

fn build_agent(
    url: &str,
    policy: agentcowork_guard::NetPolicy,
    overall_timeout: Option<Duration>,
) -> Result<ureq::Agent, EgressSetupError> {
    let parsed = parse_endpoint(url)?;

    let binding =
        agentcowork_guard::toctou::bind_url_with_policy(url, &[], policy).map_err(|_| {
            EgressSetupError {
                url: safe_origin(&parsed),
                reason: "destination_or_resolution_denied",
            }
        })?;
    if binding.resolved_ips.is_empty() {
        return Err(EgressSetupError {
            url: safe_origin(&parsed),
            reason: "empty_resolution",
        });
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| EgressSetupError {
            url: safe_origin(&parsed),
            reason: "missing_port",
        })?;
    let mut pinned = Vec::with_capacity(binding.resolved_ips.len());
    for address in binding.resolved_ips {
        let ip = IpAddr::from_str(&address).map_err(|_| EgressSetupError {
            url: safe_origin(&parsed),
            reason: "invalid_resolution",
        })?;
        pinned.push(SocketAddr::new(ip, port));
    }

    let mut builder = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(30))
        .resolver(move |_netloc: &str| Ok(pinned.clone()));
    if let Some(timeout) = overall_timeout {
        builder = builder.timeout(timeout);
    }
    Ok(builder.build())
}

fn safe_origin(url: &url::Url) -> String {
    url.origin().ascii_serialization()
}

/// Read a response without allocating beyond the configured limit.
pub(crate) fn read_bounded(response: ureq::Response) -> Result<Vec<u8>, ReadResponseError> {
    read_bounded_to(response, MAX_RESPONSE_BYTES)
}

/// Read a provider response with an explicit allocation ceiling.
pub(crate) fn read_bounded_to(
    response: ureq::Response,
    limit: u64,
) -> Result<Vec<u8>, ReadResponseError> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ReadResponseError::Io)?;
    if bytes.len() as u64 > limit {
        return Err(ReadResponseError::TooLarge);
    }
    Ok(bytes)
}

/// Opaque endpoint error safe to return to UI/log surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EgressSetupError {
    pub(crate) url: String,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadResponseError {
    Io,
    TooLarge,
}

impl From<ReadResponseError> for io::Error {
    fn from(value: ReadResponseError) -> Self {
        match value {
            ReadResponseError::Io => io::Error::other("response read failed"),
            ReadResponseError::TooLarge => io::Error::other("response exceeded size limit"),
        }
    }
}
