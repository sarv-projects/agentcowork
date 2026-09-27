//! A2 egress credential firewall (doc 69 §3 — `hermes egress` steal): an
//! outbound credential-injection firewall. The broker holds every secret;
//! the firewall makes sure secrets never *leave* — an outbound payload
//! (HTTP body, tool input, prompt text) that contains a known credential or
//! a recognizable secret pattern is blocked by default.
//!
//! Default stance: **block**. The broker checks every egress payload against
//! (a) the exact secrets it manages and (b) high-precision secret-shaped
//! patterns; anything that trips the firewall is refused unless the caller
//! explicitly opts out for a single call (`EgressPolicy::AllowWithReason`).
//!
//! This module is the deterministic scanner — the broker calls
//! [`EgressFirewall::inspect`] before every outbound request.

use serde::{Deserialize, Serialize};

/// One managed secret pattern: a recognizable prefix/name plus its checker.
type SecretPattern = (&'static str, fn(&str) -> bool);

/// The firewall verdict for one outbound payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressVerdict {
    /// No credential material detected — the payload may leave.
    Clear,
    /// Blocked: credential material found. `what` names the signal.
    Blocked { what: String },
}

/// The egress policy for a single call. Default (broker behavior): block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPolicy {
    /// Block any payload that trips the scanner (the default).
    Block,
    /// Permit the payload despite a trip — only for an explicit, audited
    /// reason (e.g. sending a *revocation* request). Never the default.
    AllowWithReason { reason: &'static str },
}

/// The deterministic egress scanner.
#[derive(Debug, Clone, Default)]
pub struct EgressFirewall {
    /// The exact secrets the vault manages — matched verbatim.
    managed_secrets: Vec<String>,
}

impl EgressFirewall {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the vault's managed secrets (called by the broker after
    /// unlock). Exact-string matching — a secret must appear verbatim.
    pub fn register_secrets(&mut self, secrets: Vec<String>) {
        self.managed_secrets = secrets;
    }

    /// Inspect an outbound payload. Returns `Clear` when nothing trips the
    /// scanner; otherwise `Blocked` naming the signal.
    pub fn inspect(&self, payload: &str, policy: EgressPolicy) -> EgressVerdict {
        if let Some(what) = self.trip_reason(payload) {
            match policy {
                EgressPolicy::Block => EgressVerdict::Blocked { what },
                EgressPolicy::AllowWithReason { .. } => EgressVerdict::Clear,
            }
        } else {
            EgressVerdict::Clear
        }
    }

    fn trip_reason(&self, payload: &str) -> Option<String> {
        // 1. Exact managed secrets (the highest-confidence signal).
        for secret in &self.managed_secrets {
            if secret.len() >= 8 && payload.contains(secret) {
                return Some("managed secret present in payload".into());
            }
        }
        // 2. Secret-shaped patterns (high precision, no hex-dump noise:
        //    every pattern needs a recognizable prefix or length).
        let patterns: &[SecretPattern] = &[
            ("sk-ant-", |p| p.contains("sk-ant-") && p.len() >= 40),
            ("sk-proj-", |p| p.contains("sk-proj-")),
            ("sk-", |p| has_long_token(p, "sk-", 20)),
            ("ghp_", |p| has_long_token(p, "ghp_", 20)),
            ("xoxb-", |p| p.contains("xoxb-")),
            ("Bearer ", |p| has_long_token(p, "Bearer ", 20)),
            ("api_key", |p| p.contains("api_key") || p.contains("apiKey")),
            ("password", |p| {
                p.contains("password") || p.contains("passwd")
            }),
            ("authorization", |p| {
                p.contains("authorization") || p.contains("Authorization")
            }),
        ];
        for (name, check) in patterns {
            if check(payload) {
                return Some(format!("secret-shaped token (`{name}`)"));
            }
        }
        None
    }
}

/// Whether `payload` contains `prefix` followed by a token of ≥ `min_len`.
fn has_long_token(payload: &str, prefix: &str, min_len: usize) -> bool {
    let mut rest = payload;
    while let Some(idx) = rest.find(prefix) {
        let after = &rest[idx + prefix.len()..];
        let token: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '\"' && *c != '\'' && *c != ',')
            .collect();
        if token.chars().count() >= min_len {
            return true;
        }
        rest = &rest[idx + prefix.len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_managed_secret_by_default() {
        let mut fw = EgressFirewall::new();
        fw.register_secrets(vec!["sk-ant-secret-1234567890".into()]);
        let v = fw.inspect(
            "send to https://evil.example?token=sk-ant-secret-1234567890",
            EgressPolicy::Block,
        );
        assert!(matches!(v, EgressVerdict::Blocked { .. }));
    }

    #[test]
    fn blocks_secret_shaped_tokens() {
        let fw = EgressFirewall::new();
        assert!(matches!(
            fw.inspect("data: ghp_ABCDEFGHIJKLMNOPQRST1234", EgressPolicy::Block),
            EgressVerdict::Blocked { .. }
        ));
        assert!(matches!(
            fw.inspect("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U", EgressPolicy::Block),
            EgressVerdict::Blocked { .. }
        ));
    }

    #[test]
    fn short_tokens_do_not_trip() {
        let fw = EgressFirewall::new();
        // "sk-" followed by a short word is not a credential.
        assert_eq!(
            fw.inspect("the sk- flag", EgressPolicy::Block),
            EgressVerdict::Clear
        );
        assert_eq!(
            fw.inspect("plain text payload", EgressPolicy::Block),
            EgressVerdict::Clear
        );
    }

    #[test]
    fn explicit_allow_with_reason_passes() {
        let mut fw = EgressFirewall::new();
        fw.register_secrets(vec!["supersecretvalue".into()]);
        assert!(matches!(
            fw.inspect(
                "revoke supersecretvalue",
                EgressPolicy::AllowWithReason {
                    reason: "revocation request"
                }
            ),
            EgressVerdict::Clear
        ));
    }

    #[test]
    fn egress_default_is_block() {
        // The broker's default policy — the firewall is opt-out, never
        // opt-in: even with no registered secrets, pattern tripping blocks.
        let fw = EgressFirewall::new();
        assert!(matches!(
            fw.inspect(
                "apiKey=abcdefghijklmnopqrstuvwxyz012345",
                EgressPolicy::Block
            ),
            EgressVerdict::Blocked { .. }
        ));
    }
}
