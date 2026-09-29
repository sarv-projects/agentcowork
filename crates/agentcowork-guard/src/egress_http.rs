//! Guard-bound, bounded HTTP requests for Core-mediated network operations.
//!
//! `netfloor` classifies destinations and `toctou` binds resolved addresses.
//! This adapter carries that binding into the socket resolver, refuses origin
//! changes and redirects, and bounds response allocation. It deliberately does
//! not own credentials, retry policy, or long-lived streaming cancellation.

use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use crate::{NetPolicy, toctou::bind_url_with_policy};

/// A one-origin HTTP client pinned to a Guard-approved resolution.
pub struct GuardedHttpClient {
    agent: ureq::Agent,
    origin: String,
}

/// Bounded HTTP response, including non-2xx response bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedHttpResponse {
    /// The HTTP status returned by the endpoint.
    pub status: u16,
    /// Response body, bounded by the request's byte limit.
    pub body: Vec<u8>,
}

/// Safe transport failure. It intentionally excludes full URLs and raw client
/// errors, which may contain sensitive endpoint data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardedHttpError {
    /// URL is malformed, unsupported, contains userinfo, or changes origin.
    #[error("invalid or cross-origin HTTP endpoint")]
    InvalidEndpoint,
    /// Guard's destination policy refused the endpoint or its resolution.
    #[error("Guard refused the HTTP destination")]
    EgressDenied,
    /// The request failed or could not be read completely.
    #[error("HTTP transport failed")]
    Transport,
    /// The response exceeded the caller's declared maximum.
    #[error("HTTP response exceeded the configured byte limit")]
    BodyTooLarge,
}

impl GuardedHttpClient {
    /// Resolve/check `endpoint` and pin the approved addresses for all
    /// same-origin requests made through this client.
    pub fn new(
        endpoint: &str,
        policy: NetPolicy,
        timeout: Duration,
    ) -> Result<Self, GuardedHttpError> {
        let parsed = parse_url(endpoint)?;
        let binding = bind_url_with_policy(endpoint, &[], policy)
            .map_err(|_| GuardedHttpError::EgressDenied)?;
        if binding.resolved_ips.is_empty() {
            return Err(GuardedHttpError::EgressDenied);
        }
        let port = parsed
            .port_or_known_default()
            .ok_or(GuardedHttpError::InvalidEndpoint)?;
        let mut pinned = Vec::with_capacity(binding.resolved_ips.len());
        for address in binding.resolved_ips {
            let ip = IpAddr::from_str(&address).map_err(|_| GuardedHttpError::EgressDenied)?;
            pinned.push(SocketAddr::new(ip, port));
        }

        let origin = parsed.origin().ascii_serialization();
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(timeout)
            .timeout_connect(Duration::from_secs(5))
            .resolver(move |_netloc: &str| Ok(pinned.clone()))
            .build();
        Ok(Self { agent, origin })
    }

    /// Issue one request to this client's origin and bound response allocation.
    /// Callers must apply credential-specific TLS rules before supplying
    /// sensitive headers; this API enforces network destination policy only.
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        max_response_bytes: usize,
    ) -> Result<GuardedHttpResponse, GuardedHttpError> {
        let parsed = parse_url(url)?;
        if parsed.origin().ascii_serialization() != self.origin {
            return Err(GuardedHttpError::InvalidEndpoint);
        }
        let method = match method {
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" => method,
            _ => return Err(GuardedHttpError::InvalidEndpoint),
        };
        let mut request = self.agent.request(method, url);
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("proxy-authorization")
            {
                return Err(GuardedHttpError::InvalidEndpoint);
            }
            request = request.set(name, value);
        }

        let result = match body {
            Some(bytes) => request.send_bytes(bytes),
            None => request.call(),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(_)) => return Err(GuardedHttpError::Transport),
        };
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(max_response_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| GuardedHttpError::Transport)?;
        if bytes.len() > max_response_bytes {
            return Err(GuardedHttpError::BodyTooLarge);
        }
        Ok(GuardedHttpResponse {
            status,
            body: bytes,
        })
    }
}

fn parse_url(url: &str) -> Result<url::Url, GuardedHttpError> {
    let parsed = url::Url::parse(url).map_err(|_| GuardedHttpError::InvalidEndpoint)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(GuardedHttpError::InvalidEndpoint);
    }
    Ok(parsed)
}
