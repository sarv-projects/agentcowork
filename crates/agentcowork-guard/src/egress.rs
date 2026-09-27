//! H3 — Data Egress Engine. Before any external call, produce a data-release
//! plan (what / where / why / which model / authorization → ALLOW / REDACT /
//! DENY). Unifies URL floors + connectivity modes.

use crate::netfloor::{self, NetPolicy};
use crate::urlfloor::{UrlVerdict, check_url_with_policy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityMode {
    /// No network, no registries.
    Offline,
    /// Local models / tools only.
    Local,
    /// Direct provider calls with user keys.
    Byok,
    /// MCP / ACP / browser / registry / search.
    #[default]
    ThirdParty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressVerdict {
    Allow,
    Redact,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressPlan {
    pub destination: String,
    pub kind: String,
    pub model: Option<String>,
    pub reason: String,
    pub verdict: EgressVerdict,
}

#[derive(Debug, Clone, Default)]
pub struct EgressEngine {
    pub mode: ConnectivityMode,
    /// Destination policy for agent-chosen `http(s)` URLs (P62.1). Loopback is
    /// allowed by default on a local desktop; LAN/private needs the explicit
    /// opt-in. Metadata/link-local is refused under every policy.
    pub policy: NetPolicy,
    /// Destinations the *user* configured (provider base URLs, a local model
    /// runtime, a paired node). These bypass the private/LAN gate because the
    /// user typed them — the gate exists for destinations the agent chose.
    granted_hosts: Vec<String>,
    inventory: Vec<EgressPlan>,
}

impl EgressEngine {
    pub fn new(mode: ConnectivityMode) -> Self {
        Self {
            mode,
            policy: NetPolicy::default(),
            granted_hosts: Vec::new(),
            inventory: Vec::new(),
        }
    }

    /// Grant a user-configured host (`localhost`, `192.168.1.50`, `nas.local`).
    /// Comparison is exact and ASCII-insensitive on the host, port stripped.
    pub fn grant_host(&mut self, host: &str) {
        let h = host.trim().to_ascii_lowercase();
        if !h.is_empty() && !self.granted_hosts.contains(&h) {
            self.granted_hosts.push(h);
        }
    }

    /// Grant the host of a full `http(s)` URL — the form the shell holds for a
    /// user-chosen endpoint (a provider `base_url`, a local runtime, a NAS).
    ///
    /// This is the P62.4 seam: the destination floor exists to stop
    /// *agent-chosen* destinations (metadata, link-local, an address the model
    /// read out of a page). A base URL only exists because the user or the
    /// shipped catalog put it there, so it is authorized by construction. Host
    /// parsing stays here so there is exactly one implementation.
    pub fn grant_url(&mut self, url: &str) {
        if let Some(host) = host_of(url) {
            self.grant_host(host);
        }
    }

    /// Builder form of [`Self::grant_host`].
    pub fn with_granted_hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for h in hosts {
            self.grant_host(h.as_ref());
        }
        self
    }

    /// Set the destination policy (e.g. [`NetPolicy::strict`] for a managed or
    /// paranoid profile, [`NetPolicy::local`] to permit a LAN node).
    pub fn with_policy(mut self, policy: NetPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn granted_hosts(&self) -> &[String] {
        &self.granted_hosts
    }

    fn is_granted(&self, destination: &str) -> bool {
        let Some(host) = host_of(destination) else {
            return false;
        };
        let h = host.to_ascii_lowercase();
        self.granted_hosts.contains(&h)
    }

    pub fn plan(
        &mut self,
        destination: &str,
        kind: &str,
        model: Option<&str>,
        reason: &str,
        roots: &[&str],
    ) -> EgressPlan {
        self.plan_with_policy(destination, kind, model, reason, roots, self.policy)
    }

    /// Plan a destination under an explicit policy — used by the agent tool
    /// path, where the URL was chosen from untrusted content and therefore gets
    /// [`NetPolicy::strict`] (no loopback, no LAN) regardless of the engine's
    /// own policy.
    #[allow(clippy::too_many_arguments)]
    pub fn plan_with_policy(
        &mut self,
        destination: &str,
        kind: &str,
        model: Option<&str>,
        reason: &str,
        roots: &[&str],
        policy: NetPolicy,
    ) -> EgressPlan {
        let verdict = self.verdict_for_with_policy(destination, kind, roots, policy);
        let plan = EgressPlan {
            destination: destination.to_string(),
            kind: kind.to_string(),
            model: model.map(str::to_string),
            reason: reason.to_string(),
            verdict,
        };
        self.inventory.push(plan.clone());
        plan
    }

    fn verdict_for_with_policy(
        &self,
        destination: &str,
        kind: &str,
        roots: &[&str],
        policy: NetPolicy,
    ) -> EgressVerdict {
        // 1) The hard floor comes first and outranks everything, including a
        //    user grant: link-local (cloud metadata), unspecified, multicast
        //    and reserved space have no legitimate desktop use and cannot be
        //    opted into. `NetPolicy::local()` allows loopback + private, so
        //    anything it still refuses is by definition always-refused.
        if let Some(class) = destination_class(destination)
            && !NetPolicy::local().allows(class)
        {
            return EgressVerdict::Deny;
        }
        // 2) The user's own configured endpoints (provider base URL, local
        //    runtime, paired node) pass without the private/LAN gate — the
        //    gate exists for destinations the agent chose itself.
        if self.is_granted(destination) {
            return EgressVerdict::Allow;
        }
        match self.mode {
            ConnectivityMode::Offline => {
                if kind == "network" || destination.starts_with("http") {
                    EgressVerdict::Deny
                } else {
                    EgressVerdict::Allow
                }
            }
            ConnectivityMode::Local => {
                if destination.starts_with("http://127.0.0.1")
                    || destination.starts_with("http://localhost")
                    || kind == "local"
                {
                    EgressVerdict::Allow
                } else if destination.starts_with("http") {
                    EgressVerdict::Deny
                } else {
                    EgressVerdict::Allow
                }
            }
            ConnectivityMode::Byok => {
                if destination.starts_with("http") {
                    match check_url_with_policy(destination, roots, policy) {
                        UrlVerdict::Allowed => EgressVerdict::Allow,
                        _ => EgressVerdict::Deny,
                    }
                } else {
                    EgressVerdict::Allow
                }
            }
            ConnectivityMode::ThirdParty => {
                match check_url_with_policy(destination, roots, policy) {
                    UrlVerdict::Allowed => EgressVerdict::Allow,
                    // Every refusal is a deny: the plan carries the destination
                    // and the card explains it via `urlfloor::block_reason`.
                    UrlVerdict::SchemeBlocked
                    | UrlVerdict::Malformed
                    | UrlVerdict::OutsideRoots
                    | UrlVerdict::PrivateDestination => EgressVerdict::Deny,
                }
            }
        }
    }

    pub fn inventory(&self) -> &[EgressPlan] {
        &self.inventory
    }

    pub fn set_mode(&mut self, mode: ConnectivityMode) {
        self.mode = mode;
    }
}

/// The host of an `http(s)` destination, port stripped.
fn host_of(destination: &str) -> Option<&str> {
    let rest = destination
        .strip_prefix("http://")
        .or_else(|| destination.strip_prefix("https://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // Drop `userinfo@` and the port (handle a bracketed IPv6 literal).
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(stripped) = authority.strip_prefix('[') {
        return stripped
            .split(']')
            .next()
            .map(|h| h.trim_start_matches('['));
    }
    Some(authority.split(':').next().unwrap_or(authority))
}

/// Classify an `http(s)` destination's host, when it has one.
fn destination_class(destination: &str) -> Option<netfloor::NetClass> {
    let host = host_of(destination)?;
    if host.is_empty() {
        return None;
    }
    Some(netfloor::classify_host(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_denies_network() {
        let mut e = EgressEngine::new(ConnectivityMode::Offline);
        let p = e.plan(
            "https://api.openai.com",
            "network",
            Some("gpt"),
            "chat",
            &[],
        );
        assert_eq!(p.verdict, EgressVerdict::Deny);
    }

    #[test]
    fn local_allows_loopback_only() {
        let mut e = EgressEngine::new(ConnectivityMode::Local);
        assert_eq!(
            e.plan("http://127.0.0.1:11434", "network", None, "ollama", &[])
                .verdict,
            EgressVerdict::Allow
        );
        assert_eq!(
            e.plan("https://api.openai.com", "network", None, "chat", &[])
                .verdict,
            EgressVerdict::Deny
        );
    }

    #[test]
    fn third_party_uses_url_floor() {
        let mut e = EgressEngine::new(ConnectivityMode::ThirdParty);
        assert_eq!(
            e.plan(
                "javascript:alert(1)",
                "network",
                None,
                "xss",
                &["/workspace"]
            )
            .verdict,
            EgressVerdict::Deny
        );
        assert_eq!(
            e.plan("https://example.com", "network", None, "fetch", &[])
                .verdict,
            EgressVerdict::Allow
        );
    }

    #[test]
    fn cloud_metadata_denied_in_every_mode() {
        for mode in [
            ConnectivityMode::ThirdParty,
            ConnectivityMode::Byok,
            ConnectivityMode::Local,
        ] {
            let mut e = EgressEngine::new(mode).with_policy(NetPolicy::local());
            assert_eq!(
                e.plan(
                    "http://169.254.169.254/latest/meta-data/",
                    "network",
                    None,
                    "ssrf",
                    &[]
                )
                .verdict,
                EgressVerdict::Deny,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn lan_denied_by_default_allowed_with_local_policy() {
        let mut e = EgressEngine::new(ConnectivityMode::ThirdParty);
        assert_eq!(
            e.plan("http://192.168.1.50:8080/x", "network", None, "node", &[])
                .verdict,
            EgressVerdict::Deny
        );
        let mut local =
            EgressEngine::new(ConnectivityMode::ThirdParty).with_policy(NetPolicy::local());
        assert_eq!(
            local
                .plan("http://192.168.1.50:8080/x", "network", None, "node", &[])
                .verdict,
            EgressVerdict::Allow
        );
    }

    #[test]
    fn user_granted_hosts_bypass_the_private_gate() {
        let mut e = EgressEngine::new(ConnectivityMode::ThirdParty).with_granted_hosts([
            "192.168.1.50",
            "localhost",
            "ollama.lan",
        ]);
        assert_eq!(e.granted_hosts().len(), 3);
        assert_eq!(
            e.plan("http://192.168.1.50:11434/v1", "network", None, "user", &[])
                .verdict,
            EgressVerdict::Allow
        );
        assert_eq!(
            e.plan("http://127.0.0.1:1234/v1", "network", None, "user", &[])
                .verdict,
            EgressVerdict::Allow
        );
        // A granted host never extends to the hard floor: the metadata
        // endpoint is refused even when someone tries to grant it.
        let mut g = EgressEngine::new(ConnectivityMode::ThirdParty);
        g.grant_host("169.254.169.254");
        assert_eq!(
            g.plan("http://169.254.169.254/", "network", None, "ssrf", &[])
                .verdict,
            EgressVerdict::Deny
        );
    }

    /// P62.4 — the shell grants the hosts of the endpoints the user configured
    /// (provider `base_url`, local runtime, NAS). `grant_url` is the one seam
    /// for that, so the host parsing cannot drift from [`host_of`].
    #[test]
    fn grant_url_registers_the_host_of_a_configured_endpoint() {
        let mut e = EgressEngine::new(ConnectivityMode::ThirdParty);
        // A self-hosted / NAS endpoint the user typed in Settings.
        e.grant_url("http://192.168.1.50:8000/v1");
        assert_eq!(e.granted_hosts(), ["192.168.1.50"]);
        assert_eq!(
            e.plan(
                "http://192.168.1.50:8000/v1/chat/completions",
                "network",
                None,
                "user",
                &[]
            )
            .verdict,
            EgressVerdict::Allow
        );
        // A loopback runtime (Ollama / LM Studio) likewise.
        e.grant_url("http://127.0.0.1:11434/v1");
        assert!(e.granted_hosts().contains(&"127.0.0.1".to_string()));
        // A non-http string grants nothing rather than inventing a host.
        e.grant_url("capability:search");
        assert_eq!(e.granted_hosts().len(), 2);
        // And the hard floor still outranks a granted URL.
        e.grant_url("http://169.254.169.254/latest/meta-data/");
        assert_eq!(
            e.plan(
                "http://169.254.169.254/latest/meta-data/",
                "network",
                None,
                "ssrf",
                &[]
            )
            .verdict,
            EgressVerdict::Deny
        );
    }

    #[test]
    fn host_of_strips_userinfo_port_and_ipv6_brackets() {
        assert_eq!(host_of("https://example.com/a?b=c"), Some("example.com"));
        assert_eq!(host_of("http://user:pw@host.tld:8080/x"), Some("host.tld"));
        assert_eq!(host_of("http://[::1]:9200/_cat"), Some("::1"));
        assert_eq!(host_of("not-a-url"), None);
    }
}
