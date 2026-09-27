//! P7.3 — CapabilityGranter (I6, doc 44 §5 modularity; Zed `allowed_*` +
//! Hermes allow-list pattern). The security half of the plugin ABI.
//!
//! A plugin *declares* capabilities in its manifest; the host *grants* a
//! fixed capability set and a fixed set of trusted agents. The granter is
//! the intersection: **manifest allow-list ∧ host grant**, with explicit
//! denies always winning and fail-closed trust flags (Hermes `allowed_*`
//! pattern — nothing is allowed unless it was both declared and granted).
//!
//! This module is deliberately dependency-free (no agentcowork-* imports):
//! it is a pure security primitive. [`GrantRequest`] is the plain shape a
//! plugin manifest converts into; [`CapabilityGranter::grant`] returns the
//! exact [`GrantedCapabilities`] the host may hand to a plugin's facades.

use serde::{Deserialize, Serialize};

/// Fail-closed trust flags (Hermes `allowed_*` pattern). Every flag defaults
/// to `false`: a plugin that never declares `network = true` can never
/// receive a network capability, no matter what its allow-list says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustFlags {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub files_write: bool,
    #[serde(default)]
    pub approval_required: bool,
    #[serde(default)]
    pub sandboxed: bool,
}

/// What the plugin asks for. Produced from a plugin manifest; consumed by
/// the granter. Capability strings look like `fs.read:/tmp/office/**` —
/// `<class>:<detail>` where the detail supports `*`/`**` wildcards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRequest {
    pub name: String,
    /// Agents this plugin may bind to. **Empty means bound to nothing** —
    /// capabilities are never global (explicit agent-binding).
    pub agent_bindings: Vec<String>,
    pub trust: TrustFlags,
    pub capabilities_allow: Vec<String>,
    /// Explicit denies always win, even over a host grant.
    pub capabilities_deny: Vec<String>,
}

/// What the host is willing to grant: trusted agent names and the full
/// capability set the host can hand out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostGrant {
    pub trusted_agents: Vec<String>,
    pub capabilities: Vec<String>,
}

/// The exact grant: the one agent this plugin is bound to and the exact
/// capability set it may exercise. The host hands this to the plugin's
/// facades — a facade refuses anything outside it.
///
/// `capabilities` are the surviving allow patterns; `denied` carries the
/// manifest's explicit denies so concrete capability checks can exclude the
/// denied subtrees (allow-list minus deny-list refinement — a deny does not
/// invalidate a broader allow, it narrows it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedCapabilities {
    pub plugin: String,
    pub agent: String,
    pub capabilities: Vec<String>,
    pub denied: Vec<String>,
}

/// Why a grant was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    #[error("plugin `{plugin}` binds no agents — capabilities are never global")]
    NoAgentBinding { plugin: String },
    #[error("plugin `{plugin}` binds agents {bindings:?} but the host trusts none of them")]
    NotTrusted {
        plugin: String,
        bindings: Vec<String>,
    },
    #[error(
        "plugin `{plugin}` declares capability `{cap}` but its trust flags are fail-closed (missing {flag})"
    )]
    TrustFlagClosed {
        plugin: String,
        cap: String,
        flag: &'static str,
    },
    #[error(
        "plugin `{plugin}` declares `{cap}` but is not sandboxed — dangerous powers require the sandbox"
    )]
    UnsandboxedPower { plugin: String, cap: String },
    #[error(
        "plugin `{plugin}` declares `{cap}` but does not require approval — dangerous powers require human approval"
    )]
    ApprovalNotRequired { plugin: String, cap: String },
    #[error("capability `{cap}` denied by explicit deny-list for plugin `{plugin}`")]
    ExplicitlyDenied { plugin: String, cap: String },
    #[error(
        "capability `{cap}` not granted by host (manifest allow-list ∧ host grant is empty) for plugin `{plugin}`"
    )]
    NotHostGranted { plugin: String, cap: String },
}

impl GrantRequest {
    /// The capability class — the part of `<class>:<detail>` before the
    /// first `:`. Bare strings (no `:`) have no class and are invalid.
    pub fn class(cap: &str) -> Option<&str> {
        cap.split_once(':').map(|(c, _)| c)
    }

    /// Which trust flag gates this class, if any.
    pub fn required_trust_flag(class: &str) -> Option<&'static str> {
        match class {
            "network" => Some("network"),
            "shell" => Some("shell"),
            "fs.write" => Some("files_write"),
            _ => None,
        }
    }

    /// Is this class a "dangerous power" that requires the sandbox and
    /// human approval (fail-closed unless both are declared)?
    pub fn is_dangerous(class: &str) -> bool {
        matches!(class, "network" | "shell" | "fs.write")
    }
}

/// Zed-style wildcard matcher over capability strings:
/// - `*` matches any characters **except `/`** (one path/segment level),
/// - `**` matches any characters **including `/`** (any depth),
/// - `?` matches exactly one character.
///
/// `fs.read:**` matches `fs.read:/tmp/a/b.txt`; `fs.read:*` matches
/// `fs.read:/tmp` but not `fs.read:/tmp/a`. Plain equality also matches.
pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    fn go(p: &[u8], v: &[u8]) -> bool {
        if p.is_empty() {
            return v.is_empty();
        }
        match p[0] {
            b'*' => {
                let mut i = 1;
                while i < p.len() && p[i] == b'*' {
                    i += 1;
                }
                let double = i >= 2; // `**` may consume `/`; `*` may not
                for k in 0..=v.len() {
                    if k > 0 && !double && v[k - 1] == b'/' {
                        break;
                    }
                    if go(&p[i..], &v[k..]) {
                        return true;
                    }
                }
                false
            }
            b'?' => !v.is_empty() && go(&p[1..], &v[1..]),
            c => !v.is_empty() && v[0] == c && go(&p[1..], &v[1..]),
        }
    }
    go(pattern.as_bytes(), value.as_bytes())
}

/// The P7.3 security gate: manifest allow-list ∧ host grant.
#[derive(Debug, Clone, Default)]
pub struct CapabilityGranter {
    host: HostGrant,
}

impl CapabilityGranter {
    pub fn new(host: HostGrant) -> Self {
        Self { host }
    }

    pub fn host(&self) -> &HostGrant {
        &self.host
    }

    /// Grant or refuse. Refusals are explicit [`GrantError`]s — a plugin
    /// with a single unlisted capability is refused *entirely* (fail-closed:
    /// partial grants are not a thing; the host either trusts the bundle or
    /// it does not).
    pub fn grant(&self, req: &GrantRequest) -> Result<GrantedCapabilities, GrantError> {
        // 1. Explicit agent binding — never global.
        if req.agent_bindings.is_empty() {
            return Err(GrantError::NoAgentBinding {
                plugin: req.name.clone(),
            });
        }
        let agent = req
            .agent_bindings
            .iter()
            .find(|a| self.host.trusted_agents.contains(a))
            .ok_or_else(|| GrantError::NotTrusted {
                plugin: req.name.clone(),
                bindings: req.agent_bindings.clone(),
            })?;

        // 2. Every declared capability must survive the intersection.
        let mut granted = Vec::new();
        for cap in &req.capabilities_allow {
            // Explicit deny: if a deny pattern fully covers this allow
            // pattern (every value the allow admits is denied), the bundle
            // is contradictory — refuse. A deny that only *narrows* a
            // broader allow is carried into the grant instead.
            if req.capabilities_deny.iter().any(|d| wildcard_match(d, cap)) {
                return Err(GrantError::ExplicitlyDenied {
                    plugin: req.name.clone(),
                    cap: cap.clone(),
                });
            }
            // Host must actually grant it (wildcard-aware).
            if !self
                .host
                .capabilities
                .iter()
                .any(|h| wildcard_match(h, cap) || wildcard_match(cap, h))
            {
                return Err(GrantError::NotHostGranted {
                    plugin: req.name.clone(),
                    cap: cap.clone(),
                });
            }
            // Fail-closed trust flags (Hermes allowed_* pattern).
            let class = GrantRequest::class(cap).unwrap_or(cap.as_str());
            if let Some(flag) = GrantRequest::required_trust_flag(class) {
                let declared = match flag {
                    "network" => req.trust.network,
                    "shell" => req.trust.shell,
                    "files_write" => req.trust.files_write,
                    _ => false,
                };
                if !declared {
                    return Err(GrantError::TrustFlagClosed {
                        plugin: req.name.clone(),
                        cap: cap.clone(),
                        flag,
                    });
                }
            }
            // Dangerous powers require the sandbox and human approval.
            if GrantRequest::is_dangerous(class) {
                if !req.trust.sandboxed {
                    return Err(GrantError::UnsandboxedPower {
                        plugin: req.name.clone(),
                        cap: cap.clone(),
                    });
                }
                if !req.trust.approval_required {
                    return Err(GrantError::ApprovalNotRequired {
                        plugin: req.name.clone(),
                        cap: cap.clone(),
                    });
                }
            }
            granted.push(cap.clone());
        }

        Ok(GrantedCapabilities {
            plugin: req.name.clone(),
            agent: agent.clone(),
            capabilities: granted,
            denied: req.capabilities_deny.clone(),
        })
    }

    /// Convenience: does a granted set contain a capability matching `need`
    /// (wildcard-aware) **and** not excluded by an explicit deny? Used by
    /// facades before every operation — allow-list minus deny-list.
    pub fn granted_has(granted: &GrantedCapabilities, need: &str) -> bool {
        let allowed = granted
            .capabilities
            .iter()
            .any(|g| wildcard_match(g, need) || wildcard_match(need, g));
        if !allowed {
            return false;
        }
        !granted.denied.iter().any(|d| wildcard_match(d, need))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostGrant {
        HostGrant {
            trusted_agents: vec!["primary".into(), "office-worker".into()],
            capabilities: vec![
                "fs.read:**".into(),
                "fs.write:/tmp/office/**".into(),
                "network:https".into(),
                "llm.call".into(),
                "approval.request".into(),
                "session.read".into(),
            ],
        }
    }

    fn safe_req() -> GrantRequest {
        GrantRequest {
            name: "office-tools".into(),
            agent_bindings: vec!["office-worker".into()],
            trust: TrustFlags {
                files_write: true,
                approval_required: true,
                sandboxed: true,
                ..Default::default()
            },
            capabilities_allow: vec![
                "fs.read:/tmp/office/**".into(),
                "fs.write:/tmp/office/**".into(),
            ],
            capabilities_deny: vec![],
        }
    }

    #[test]
    fn wildcard_star_is_segment_scoped() {
        // `*` matches within a segment (no `/`); `**` crosses segments.
        assert!(wildcard_match("network:*", "network:https"));
        assert!(!wildcard_match("network:*", "network:https/x"));
        assert!(wildcard_match("network:**", "network:https/x/y"));
        assert!(wildcard_match("fs.read:**", "fs.read:/tmp/a/b.txt"));
        assert!(wildcard_match("fs.read:**", "fs.read:/tmp"));
        assert!(wildcard_match(
            "fs.write:/tmp/office/*",
            "fs.write:/tmp/office/report.pdf"
        ));
        assert!(!wildcard_match(
            "fs.write:/tmp/office/*",
            "fs.write:/tmp/office/x/y.pdf"
        ));
        assert!(wildcard_match(
            "fs.write:/tmp/office/**",
            "fs.write:/tmp/office/x/y.pdf"
        ));
        assert!(wildcard_match("llm.call", "llm.call"));
        assert!(!wildcard_match("llm.call", "llm.call:gpt"));
        assert!(wildcard_match("?", "a"));
        assert!(!wildcard_match("?", "ab"));
    }

    #[test]
    fn grants_intersection_of_allow_and_host() {
        let g = CapabilityGranter::new(host());
        let granted = g.grant(&safe_req()).unwrap();
        assert_eq!(granted.agent, "office-worker");
        assert_eq!(
            granted.capabilities,
            vec![
                "fs.read:/tmp/office/**".to_string(),
                "fs.write:/tmp/office/**".to_string(),
            ]
        );
    }

    #[test]
    fn empty_agent_binding_is_never_global() {
        let mut req = safe_req();
        req.agent_bindings = vec![];
        let err = CapabilityGranter::new(host()).grant(&req).unwrap_err();
        assert!(matches!(err, GrantError::NoAgentBinding { .. }));
    }

    #[test]
    fn untrusted_agent_is_refused() {
        let mut req = safe_req();
        req.agent_bindings = vec!["stranger".into()];
        let err = CapabilityGranter::new(host()).grant(&req).unwrap_err();
        assert!(matches!(err, GrantError::NotTrusted { .. }));
    }

    #[test]
    fn unlisted_capability_is_refused() {
        let mut req = safe_req();
        req.capabilities_allow.push("shell.exec:any".into());
        req.trust.shell = true;
        let err = CapabilityGranter::new(host()).grant(&req).unwrap_err();
        assert!(matches!(err, GrantError::NotHostGranted { .. }));
    }

    #[test]
    fn trust_flag_closed_blocks_declared_capability() {
        let mut req = safe_req();
        req.capabilities_allow.push("network:https".into());
        // trust.network stays false — fail-closed.
        let err = CapabilityGranter::new(host()).grant(&req).unwrap_err();
        assert!(matches!(
            err,
            GrantError::TrustFlagClosed {
                flag: "network",
                ..
            }
        ));
    }

    #[test]
    fn dangerous_power_requires_sandbox_and_approval() {
        let mut req = safe_req();
        req.capabilities_allow.push("network:https".into());
        req.trust.network = true;
        req.trust.sandboxed = false;
        assert!(matches!(
            CapabilityGranter::new(host()).grant(&req).unwrap_err(),
            GrantError::UnsandboxedPower { .. }
        ));
        req.trust.sandboxed = true;
        req.trust.approval_required = false;
        assert!(matches!(
            CapabilityGranter::new(host()).grant(&req).unwrap_err(),
            GrantError::ApprovalNotRequired { .. }
        ));
    }

    #[test]
    fn deny_narrows_a_broad_allow_instead_of_invalidating_it() {
        let mut req = safe_req();
        req.capabilities_deny
            .push("fs.write:/tmp/office/secret/**".into());
        let granted = CapabilityGranter::new(host()).grant(&req).unwrap();
        // Concrete checks: secret subtree excluded, rest of the allow kept.
        assert!(CapabilityGranter::granted_has(
            &granted,
            "fs.write:/tmp/office/report.pdf"
        ));
        assert!(!CapabilityGranter::granted_has(
            &granted,
            "fs.write:/tmp/office/secret/report.pdf"
        ));
    }

    #[test]
    fn explicit_deny_wins_over_host_grant() {
        let mut req = safe_req();
        req.capabilities_deny
            .push("fs.write:/tmp/office/secret/**".into());
        req.capabilities_allow
            .push("fs.write:/tmp/office/secret/report.pdf".into());
        let err = CapabilityGranter::new(host()).grant(&req).unwrap_err();
        assert!(matches!(err, GrantError::ExplicitlyDenied { .. }));
    }

    #[test]
    fn granted_has_is_wildcard_aware() {
        let granted = CapabilityGranter::new(host()).grant(&safe_req()).unwrap();
        assert!(CapabilityGranter::granted_has(
            &granted,
            "fs.read:/tmp/office/x/y.pdf"
        ));
        assert!(!CapabilityGranter::granted_has(
            &granted,
            "fs.read:/home/secret"
        ));
        assert!(!CapabilityGranter::granted_has(&granted, "llm.call"));
    }
}
