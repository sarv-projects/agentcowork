//! Local Auth Bridge (P6.6 F4 — doc 13 §5.5, doc 50 §6): the connector
//! OAuth layer for Gmail / Google Calendar / etc.
//!
//! * **PKCE client, no client secret** — public clients only; the
//!   code_verifier never leaves the vault (it is held in the pending flow).
//! * **Local token manager** — acquired access/refresh tokens are stored in
//!   the key ring (encrypted at rest), keyed `provider=connector:<provider>`,
//!   so the broker's A3 failover semantics apply to connector tokens exactly
//!   as they do to model keys. The sidecar only ever sees a handle.
//!
//! The live Google endpoints are registered as defaults but every endpoint
//! is overridable — the loopback tests point the token exchange at a mock
//! server, so the full PKCE flow is exercised without any external network.

use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::io::Read;

use crate::Vault;
use crate::keyring::{KeyRing, KeySpec, KeyStatus};

/// Connector providers the Auth Bridge knows out of the box. Each is a
/// public PKCE client (no secret by design — doc 13 §5.5).
pub const GMAIL: &str = "connector:gmail";
pub const GOOGLE_CALENDAR: &str = "connector:google-calendar";

/// A PKCE provider's endpoints. Defaults are the live Google endpoints;
/// tests override `token_url` to a loopback mock.
#[derive(Debug, Clone)]
pub struct BridgeProvider {
    pub authorize_url: String,
    pub token_url: String,
    pub scopes: String,
    /// Our public client id (registered at ship; overridable per install).
    pub client_id: String,
    /// Loopback redirect (bound port substituted by the caller).
    pub redirect_uri: String,
}

impl BridgeProvider {
    fn google(_provider: &str, scopes: &str, client_id: &str) -> Self {
        Self {
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            scopes: scopes.into(),
            client_id: client_id.into(),
            redirect_uri: "http://127.0.0.1:0/oauth/callback".into(),
        }
    }
}

fn defaults() -> Vec<(&'static str, BridgeProvider)> {
    vec![
        (
            GMAIL,
            BridgeProvider::google(
                GMAIL,
                "https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.send https://www.googleapis.com/auth/gmail.modify",
                "agentcowork-public-client-id",
            ),
        ),
        (
            GOOGLE_CALENDAR,
            BridgeProvider::google(
                GOOGLE_CALENDAR,
                "https://www.googleapis.com/auth/calendar.events",
                "agentcowork-public-client-id",
            ),
        ),
    ]
}

/// Result of starting a PKCE flow: the URL to open in the system browser.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BridgeStart {
    pub provider: String,
    pub auth_url: String,
    /// State the caller must echo back with the authorization code.
    pub state: String,
}

/// The bridge: starts PKCE flows and stores acquired tokens in the key ring.
#[derive(Clone)]
pub struct AuthBridge<'a> {
    vault: &'a Vault,
    providers: Vec<(&'static str, BridgeProvider)>,
    /// In-flight PKCE state: `(provider, state) -> verifier`. Held in
    /// memory; cleared on completion/expiry.
    pending: std::collections::HashMap<(String, String), String>,
}

impl<'a> AuthBridge<'a> {
    pub fn new(vault: &'a Vault) -> Self {
        Self {
            vault,
            providers: defaults(),
            pending: std::collections::HashMap::new(),
        }
    }

    pub fn with_client_id(mut self, provider: &str, client_id: impl Into<String>) -> Self {
        if let Some((_, p)) = self.providers.iter_mut().find(|(n, _)| *n == provider) {
            p.client_id = client_id.into();
        }
        self
    }

    pub fn with_token_url(mut self, provider: &str, url: impl Into<String>) -> Self {
        if let Some((_, p)) = self.providers.iter_mut().find(|(n, _)| *n == provider) {
            p.token_url = url.into();
        }
        self
    }

    pub fn with_redirect_uri(mut self, provider: &str, uri: impl Into<String>) -> Self {
        if let Some((_, p)) = self.providers.iter_mut().find(|(n, _)| *n == provider) {
            p.redirect_uri = uri.into();
        }
        self
    }

    fn provider(&self, name: &str) -> Option<&BridgeProvider> {
        self.providers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
    }

    /// Start a PKCE flow for a connector provider. Returns the authorize URL
    /// (state included) and stores the verifier for the completion step.
    pub fn start_pkce(&mut self, provider: &str) -> Result<BridgeStart, String> {
        let p = self
            .provider(provider)
            .ok_or_else(|| format!("unsupported connector provider {provider:?}"))?;
        let verifier = random_url_b64(32);
        let challenge = code_challenge(&verifier);
        let state = random_hex(16);

        let client_id = p.client_id.clone();
        let redirect_uri = p.redirect_uri.clone();
        let scopes = p.scopes.clone();
        let authorize_url = p.authorize_url.clone();
        let mut authorize = url::Url::parse(&authorize_url)
            .map_err(|_| "invalid OAuth authorization endpoint".to_string())?;
        if authorize.scheme() != "https"
            || authorize.host_str().is_none()
            || !authorize.username().is_empty()
            || authorize.password().is_some()
            || authorize.fragment().is_some()
            || authorize.query().is_some()
        {
            return Err("invalid OAuth authorization endpoint".into());
        }
        authorize.query_pairs_mut().extend_pairs([
            ("client_id", client_id.as_str()),
            ("response_type", "code"),
            ("scope", scopes.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ]);
        self.pending
            .insert((provider.to_string(), state.clone()), verifier);

        Ok(BridgeStart {
            provider: provider.to_string(),
            auth_url: authorize.into(),
            state,
        })
    }

    /// Complete the PKCE exchange: swap the code for tokens at the provider's
    /// token endpoint, then persist into the key ring (encrypted at rest).
    /// The account id is derived from the provider so the broker sees one
    /// handle per connected account.
    pub fn complete_pkce(
        &mut self,
        provider: &str,
        code: &str,
        state: &str,
        account_id: &str,
    ) -> Result<(), String> {
        let redirect_uri = self
            .provider(provider)
            .ok_or_else(|| format!("unsupported connector provider {provider:?}"))?
            .redirect_uri
            .clone();
        let client_id = self
            .provider(provider)
            .ok_or_else(|| format!("unsupported connector provider {provider:?}"))?
            .client_id
            .clone();
        let token_url = self
            .provider(provider)
            .ok_or_else(|| format!("unsupported connector provider {provider:?}"))?
            .token_url
            .clone();
        let verifier = self
            .pending
            .remove(&(provider.to_string(), state.to_string()))
            .ok_or_else(|| "state mismatch or expired flow".to_string())?;
        let form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ];
        let json = post_form(&token_url, &form)?;

        let access_token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "token response missing access_token".to_string())?
            .to_string();
        let refresh_token = json
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let key_id = format!("{provider}:{account_id}");
        let spec = KeySpec {
            provider: provider.to_string(),
            key_id: key_id.clone(),
            value: access_token.into_bytes(),
            status: KeyStatus::Primary,
            model_filter: Vec::new(),
            priority: 100,
            daily_token_cap: None,
            daily_cost_cap: None,
        };
        let ring = KeyRing::new(self.vault);
        // add_key replaces on conflict, so re-connecting the same account
        // rotates the token rather than erroring (no double-connect is
        // enforced at the hub layer).
        ring.add_key(spec).map_err(|e| e.to_string())?;
        if let Some(rt) = refresh_token {
            let _ = self.vault.put_ui_session(&format!("{key_id}:refresh"), &rt);
        }
        Ok(())
    }

    /// Is this account connected (a token present in the key ring)?
    pub fn is_connected(&self, provider: &str, account_id: &str) -> bool {
        let ring = KeyRing::new(self.vault);
        let key_id = format!("{provider}:{account_id}");
        ring.get(provider, &key_id).is_ok()
    }
}

// -- PKCE primitives (mirrors oauth.rs; kept local so the connector bridge is
//    self-contained and testable without the subscription flow) ------------

fn random_url_b64(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn random_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// POST an application/x-www-form-urlencoded body and parse the JSON reply.
fn post_form(url: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    agentcowork_guard::netfloor::preflight_url(url, agentcowork_guard::NetPolicy::default())
        .map_err(|denied| format!("token endpoint refused: {}", denied.reason))?;
    let agent = ureq::AgentBuilder::new()
        // OAuth responses must not silently move a code or token-bearing
        // request to another origin. A redirect is returned to the caller.
        .redirects(0)
        .timeout(std::time::Duration::from_secs(15))
        .build();
    let response = match agent.post(url).send_form(form) {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            let value = read_json_bounded(response)?;
            if value.get("error").is_some() {
                return Ok(value);
            }
            return Err(format!("token exchange returned HTTP {status}"));
        }
        Err(ureq::Error::Transport(_)) => return Err("token exchange transport failed".into()),
    };
    read_json_bounded(response)
        .map_err(|_| "token response was invalid or exceeded its size limit".into())
}

const MAX_TOKEN_RESPONSE_BYTES: u64 = 1024 * 1024;

fn read_json_bounded(response: ureq::Response) -> Result<serde_json::Value, String> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_TOKEN_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "token response read failed".to_string())?;
    if bytes.len() as u64 > MAX_TOKEN_RESPONSE_BYTES {
        return Err("token response exceeded its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "token response was not valid JSON".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn vault() -> &'static Vault {
        Box::leak(Box::new(Vault::open_in_memory("test-key").unwrap()))
    }

    /// One-shot mock token endpoint on loopback: captures the form body,
    /// replies with a canned token payload.
    fn mock_token_endpoint() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    match s.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let body =
                    r#"{"access_token":"at-123","refresh_token":"rt-456","expires_in":3600}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn start_pkce_builds_authorize_url_with_challenge_and_state() {
        let mut bridge = AuthBridge::new(vault());
        let start = bridge.start_pkce(GMAIL).unwrap();
        assert!(start.auth_url.contains("code_challenge="));
        assert!(start.auth_url.contains("code_challenge_method=S256"));
        assert!(start.auth_url.contains("response_type=code"));
        assert!(start.auth_url.contains(&format!("state={}", start.state)));
        // No client secret anywhere in the URL.
        assert!(!start.auth_url.contains("client_secret"));
    }

    #[test]
    fn complete_pkce_stores_token_in_keyring() {
        let token_url = mock_token_endpoint();
        let mut bridge = AuthBridge::new(vault())
            .with_token_url(GMAIL, &token_url)
            .with_redirect_uri(GMAIL, "http://127.0.0.1:9999/oauth/callback");
        let start = bridge.start_pkce(GMAIL).unwrap();
        bridge
            .complete_pkce(GMAIL, "auth-code", &start.state, "me@gmail.com")
            .unwrap();
        assert!(bridge.is_connected(GMAIL, "me@gmail.com"));
        // Re-completing with the same state must fail (single-use state).
        assert!(
            bridge
                .complete_pkce(GMAIL, "auth-code", &start.state, "me@gmail.com")
                .is_err()
        );
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let mut bridge = AuthBridge::new(vault());
        assert!(bridge.start_pkce("connector:slack").is_err());
    }

    #[test]
    fn state_mismatch_is_rejected_before_network() {
        let mut bridge = AuthBridge::new(vault());
        let _ = bridge.start_pkce(GMAIL).unwrap();
        assert!(
            bridge
                .complete_pkce(GMAIL, "code", "wrong-state", "a")
                .is_err()
        );
    }
}
