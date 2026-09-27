//! P51.31 — remote-attach planner: OAuth-portal vs LAN user/pass targets.
//!
//! Security posture: user/pass credentials are only ever planned for
//! loopback/LAN-private hosts; anything else is rejected. Non-loopback `http`
//! without TLS is rejected behind the auth gate. OAuth-portal targets plan an
//! OAuth flow. Vault tokens never leave the vault — only opaque handles do
//! ([`handle_only`]).

use serde::{Deserialize, Serialize};

/// How the remote target authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachProvider {
    OAuthPortal,
    UserPassLan,
}

/// A remote attach candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTarget {
    pub url: String,
    pub provider: AttachProvider,
}

impl RemoteTarget {
    pub fn new(url: impl Into<String>, provider: AttachProvider) -> Self {
        Self {
            url: url.into(),
            provider,
        }
    }
}

/// The attach plan for a [`RemoteTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachPlan {
    OAuth { auth_url_hint: String },
    UserPass { host: String },
    Rejected { reason: String },
}

/// Plan the attach for `target`.
///
/// - [`AttachProvider::OAuthPortal`] → [`AttachPlan::OAuth`] (preferred
///   default; the hint points back at the target URL).
/// - [`AttachProvider::UserPassLan`] → [`AttachPlan::UserPass`] only when the
///   host is loopback/LAN-private (`127.*`, `10.*`, `192.168.*`,
///   `172.16-31.*`, `localhost`, `*.local`, tailscale `*.ts.net`); anything
///   else is [`AttachPlan::Rejected`].
/// - Non-loopback `http` without TLS is [`AttachPlan::Rejected`] with a
///   `require-auth-gate` reason, even for LAN hosts.
pub fn plan(target: &RemoteTarget) -> AttachPlan {
    match target.provider {
        AttachProvider::OAuthPortal => AttachPlan::OAuth {
            auth_url_hint: format!("{}#oauth", target.url),
        },
        AttachProvider::UserPassLan => {
            let (scheme, host) = split_url(&target.url);
            if host.is_empty() {
                return AttachPlan::Rejected {
                    reason: "userpass rejected: empty host".to_string(),
                };
            }
            if !is_lan_host(&host) {
                return AttachPlan::Rejected {
                    reason: format!("userpass rejected: {host} is not loopback/LAN-private"),
                };
            }
            if scheme == "http" && !is_loopback(&host) {
                return AttachPlan::Rejected {
                    reason: "require-auth-gate: non-loopback http without TLS".to_string(),
                };
            }
            AttachPlan::UserPass { host }
        }
    }
}

/// Opaque vault-handle reference. Tokens never leave the vault — only this
/// handle crosses process boundaries; resolution happens inside the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultHandleRef {
    pub handle: String,
}

/// Wrap an opaque vault handle. The secret itself stays in the vault; the
/// caller only ever holds this handle.
pub fn handle_only(handle: &str) -> VaultHandleRef {
    VaultHandleRef {
        handle: handle.to_string(),
    }
}

fn split_url(url: &str) -> (String, String) {
    let (scheme, rest) = match url.find("://") {
        Some(i) => (url[..i].to_ascii_lowercase(), url[i + 3..].to_string()),
        None => (String::new(), url.to_string()),
    };
    // Authority is up to the first / ? #.
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_string();
    // Strip userinfo.
    let hostport = authority.rsplit('@').next().unwrap_or_default();
    // Strip brackets (IPv6) then port.
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default().to_string()
    } else if hostport.chars().filter(|c| *c == ':').count() > 1 {
        // Bare IPv6 literal without brackets — keep whole.
        hostport.to_string()
    } else {
        hostport.split(':').next().unwrap_or_default().to_string()
    };
    (scheme, host.to_ascii_lowercase())
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn is_lan_host(host: &str) -> bool {
    if is_loopback(host) {
        return true;
    }
    if host.starts_with("10.") || host.starts_with("192.168.") {
        return true;
    }
    if let Some(rest) = host.strip_prefix("172.") {
        if let Some(second) = rest.split('.').next().and_then(|s| s.parse::<u8>().ok()) {
            if (16..=31).contains(&second) {
                return true;
            }
        }
    }
    if host.ends_with(".local") || host == "local" {
        return true;
    }
    // Tailscale tailnet names.
    if host.ends_with(".ts.net") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str, provider: AttachProvider) -> RemoteTarget {
        RemoteTarget::new(url, provider)
    }

    #[test]
    fn userpass_rejected_on_public_ip() {
        let t = target("https://203.0.113.7/vault", AttachProvider::UserPassLan);
        assert!(
            matches!(plan(&t), AttachPlan::Rejected { .. }),
            "public IP userpass must be rejected"
        );
        let t = target("https://example.com/app", AttachProvider::UserPassLan);
        assert!(matches!(plan(&t), AttachPlan::Rejected { .. }));
    }

    #[test]
    fn non_loopback_http_rejected() {
        // LAN-private but plain http off-loopback still needs the auth gate.
        let t = target("http://192.168.1.10/app", AttachProvider::UserPassLan);
        match plan(&t) {
            AttachPlan::Rejected { reason } => assert!(
                reason.contains("require-auth-gate"),
                "reason must name require-auth-gate, got: {reason}"
            ),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn oauth_portal_preferred_default() {
        let t = target("https://example.com/app", AttachProvider::OAuthPortal);
        match plan(&t) {
            AttachPlan::OAuth { auth_url_hint } => {
                assert!(auth_url_hint.contains("example.com"));
            }
            other => panic!("expected OAuth plan, got {other:?}"),
        }
    }

    #[test]
    fn loopback_userpass_ok() {
        let t = target("http://127.0.0.1:8080/app", AttachProvider::UserPassLan);
        match plan(&t) {
            AttachPlan::UserPass { host } => assert_eq!(host, "127.0.0.1"),
            other => panic!("expected UserPass plan, got {other:?}"),
        }
        // Tokens never leave the vault — only the handle does.
        let h = handle_only("vault:tok-abc");
        assert_eq!(h.handle, "vault:tok-abc");
    }
}
