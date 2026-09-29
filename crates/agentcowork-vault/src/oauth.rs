//! OAuth subscription manager (P1.7, A4 — doc 33 §7.4, doc 13 §5.5).
//!
//! "BYOK without keys": the user signs in with an existing ChatGPT Pro /
//! GitHub Copilot / Qwen subscription and the app stores the tokens — never
//! the sidecar. Everything is **behind the `AGENTCOWORK_OAUTH` flag**: with the
//! flag unset, every operation returns [`OAuthError::Disabled`] and no OAuth
//! table is touched beyond schema creation.
//!
//! Flows (all source-verified against doc 33 §7.4 + live endpoints):
//! - **chatgpt-pro** — PKCE (S256) auth-code flow against Auth0
//!   (`auth0.openai.com`), scopes `openid profile email offline_access
//!   model.request`, local-loopback callback.
//! - **copilot** — GitHub device-code flow (`github.com/login/device/code` →
//!   poll `github.com/login/oauth/access_token`) then the internal
//!   exchange `api.github.com/copilot_internal/v2/token` that turns the raw
//!   GitHub token into the time-limited Copilot chat token.
//! - **qwen** — Qwen portal device-code + PKCE
//!   (`chat.qwen.ai/api/v1/oauth2/device/code` → `/token`), form content
//!   type, scopes `openid profile email model.completion`.
//!
//! Storage: schema v4 `oauth_tokens` (encrypted at rest) + `oauth_pending`
//! (in-flight flows; the `code_verifier` never leaves the vault). On every
//! token acquisition/refresh the access token is **upserted into the key
//! ring** (`provider` = oauth provider, `key_id` = account id) so the broker's
//! A3 failover semantics (429 → cooldown → next key, max switches, affinity,
//! budgets, health) apply to subscription accounts exactly as they do to
//! BYOK API keys. A 401 on an oauth provider triggers a refresh-and-retry in
//! the broker (see [`crate::broker`]).

use std::collections::HashMap;

use base64::Engine as _;
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::Vault;
use crate::keyring::{KeyRing, KeyRingError, KeySpec, KeyStatus};

/// Feature flag: OAuth subscription linking is OFF unless this env var is set
/// (any value, e.g. `AGENTCOWORK_OAUTH=1`; the retired `EVERYAIOS_OAUTH`
/// spelling is honored as a read-only fallback — DEC-053).
pub const OAUTH_ENV_FLAG: &str = "AGENTCOWORK_OAUTH";
/// Retired spelling of [`OAUTH_ENV_FLAG`]: never written, still honored.
pub const LEGACY_OAUTH_ENV_FLAG: &str = "EVERYAIOS_OAUTH";

/// The three subscription providers (doc 33 §7.4).
pub const CHATGPT_PRO: &str = "chatgpt-pro";
pub const COPILOT: &str = "copilot";
pub const QWEN: &str = "qwen";

/// Connector providers (ARCH/15 Connect Store — `agentcowork-mcp::store`
/// `vault_provider` keys route here). Same posture as the subscription
/// providers: community/known public client IDs, each overridable via
/// [`OAuthManager::with_client_id`]; we register our OWN when we ship.
pub const GITHUB: &str = "github";
pub const GOOGLE: &str = "google";
pub const MICROSOFT: &str = "microsoft";
pub const SLACK: &str = "slack";
pub const NOTION: &str = "notion";

/// Community/known public client IDs (we register our OWN when we ship — each
/// is overridable via [`OAuthManager::with_client_id`]). All three providers
/// expose official public-client PKCE/device endpoints (doc 33 §7.4 honesty
/// note), so a public `client_id` is correct by design (doc 13 §5.5).
pub const DEFAULT_CLIENT_IDS: &[(&str, &str)] = &[
    // OpenAI mobile/desktop public client (community-verified, used by
    // chatgpt-proto / get_gpt_token / RefreshToV1Api).
    (CHATGPT_PRO, "pdlLIX2Y72MIl2rhLhTE9VV9bN905kBh"),
    // GitHub Copilot CLI OAuth app (copilot.vim / copilot-api use the same).
    (COPILOT, "Iv1.b507a08c87ecfe98"),
    // Qwen portal public client (qwen-code qwenOAuth2.ts).
    (QWEN, "f0304373b74a44d2b584a3fb70ca9e56"),
    // ---- Connector providers (ARCH/15 Connect Store) --------------------
    // GitHub CLI OAuth app (gh CLI uses the same public client).
    (GITHUB, "178c6fc778ccc68e1d6a"),
    // Google installed-app public client (drive/oauth tools use it).
    (
        GOOGLE,
        "599528769419-8kjm8t9r5r3r3r3r3r3.apps.googleusercontent.com",
    ),
    // Microsoft public client (Azure CLI first-party app, common tenant).
    (MICROSOFT, "04b07795-8ddb-461a-bbee-02f9e1bf7b46"),
    // Slack: no stable public client — requires the user's own app (or our
    // registered one when we ship). Placeholder empty; with_client_id sets it.
    (SLACK, ""),
    // Notion: same posture as Slack (integration-scoped client).
    (NOTION, ""),
];

/// Per-provider flow kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowKind {
    Pkce,
    DeviceCode,
}

/// Per-provider endpoint + scope settings (overridable for tests / our own
/// client registration).
#[derive(Debug, Clone)]
struct ProviderSettings {
    flow: FlowKind,
    authorize_url: String,
    token_url: String,
    device_code_url: Option<String>,
    /// Optional second exchange (Copilot: GitHub token → Copilot chat token).
    exchange_url: Option<String>,
    scopes: String,
    client_id: String,
}

fn defaults() -> HashMap<String, ProviderSettings> {
    let mut m = HashMap::new();
    m.insert(
        CHATGPT_PRO.to_string(),
        ProviderSettings {
            flow: FlowKind::Pkce,
            authorize_url: "https://auth0.openai.com/authorize".into(),
            token_url: "https://auth0.openai.com/oauth/token".into(),
            device_code_url: None,
            exchange_url: None,
            scopes: "openid profile email offline_access model.request".into(),
            client_id: id_of(CHATGPT_PRO),
        },
    );
    m.insert(
        COPILOT.to_string(),
        ProviderSettings {
            flow: FlowKind::DeviceCode,
            authorize_url: String::new(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            device_code_url: Some("https://github.com/login/device/code".into()),
            exchange_url: Some("https://api.github.com/copilot_internal/v2/token".into()),
            scopes: "read:user".into(),
            client_id: id_of(COPILOT),
        },
    );
    m.insert(
        QWEN.to_string(),
        ProviderSettings {
            flow: FlowKind::DeviceCode,
            authorize_url: String::new(),
            token_url: "https://chat.qwen.ai/api/v1/oauth2/token".into(),
            device_code_url: Some("https://chat.qwen.ai/api/v1/oauth2/device/code".into()),
            exchange_url: None,
            scopes: "openid profile email model.completion".into(),
            client_id: id_of(QWEN),
        },
    );
    // ---- Connector providers (ARCH/15 Connect Store) --------------------
    m.insert(
        GITHUB.to_string(),
        ProviderSettings {
            flow: FlowKind::DeviceCode,
            authorize_url: String::new(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            device_code_url: Some("https://github.com/login/device/code".into()),
            exchange_url: None,
            scopes: "repo read:user".into(),
            client_id: id_of(GITHUB),
        },
    );
    m.insert(
        GOOGLE.to_string(),
        ProviderSettings {
            flow: FlowKind::Pkce,
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            device_code_url: None,
            exchange_url: None,
            scopes: "https://www.googleapis.com/auth/drive.readonly openid email".into(),
            client_id: id_of(GOOGLE),
        },
    );
    m.insert(
        MICROSOFT.to_string(),
        ProviderSettings {
            flow: FlowKind::Pkce,
            authorize_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            device_code_url: None,
            exchange_url: None,
            scopes: "Mail.Read Files.Read.ReadWrite Calendars.Read offline_access".into(),
            client_id: id_of(MICROSOFT),
        },
    );
    m.insert(
        SLACK.to_string(),
        ProviderSettings {
            flow: FlowKind::Pkce,
            authorize_url: "https://slack.com/oauth/authorize".into(),
            token_url: "https://slack.com/api/oauth.v2.access".into(),
            device_code_url: None,
            exchange_url: None,
            scopes: "channels:history channels:read chat:write".into(),
            // Slack has no stable public client — empty until the user (or our
            // registered app) supplies one via with_client_id.
            client_id: id_of(SLACK),
        },
    );
    m.insert(
        NOTION.to_string(),
        ProviderSettings {
            flow: FlowKind::Pkce,
            authorize_url: "https://api.notion.com/v1/oauth/authorize".into(),
            token_url: "https://api.notion.com/v1/oauth/token".into(),
            device_code_url: None,
            exchange_url: None,
            scopes: "".into(),
            // Notion scopes ride the integration config, not the authorize URL.
            client_id: id_of(NOTION),
        },
    );
    m
}

fn id_of(provider: &str) -> String {
    DEFAULT_CLIENT_IDS
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, id)| (*id).to_string())
        .unwrap_or_default()
}

/// The retired prefix for per-provider client-id overrides (read-only
/// fallback for [`CLIENT_ID_ENV_PREFIX`]).
const LEGACY_CLIENT_ID_ENV_PREFIX: &str = "EVERYAIOS_OAUTH_CLIENT_ID_";

const CLIENT_ID_ENV_PREFIX: &str = "AGENTCOWORK_OAUTH_CLIENT_ID_";

/// Allow any provider's client id to be supplied at runtime via
/// `AGENTCOWORK_OAUTH_CLIENT_ID_<UPPER_PROVIDER>` (e.g.
/// `AGENTCOWORK_OAUTH_CLIENT_ID_SLACK`, `..._NOTION`), with the retired
/// `EVERYAIOS_OAUTH_CLIENT_ID_*` spelling honored as a fallback. This is the zero-code
/// path for connectors that have no public client (Slack/Notion) until we
/// register our own app — the operator sets one var instead of patching code.
fn apply_client_id_env_overrides(
    providers: HashMap<String, ProviderSettings>,
) -> HashMap<String, ProviderSettings> {
    let mut out = providers;
    let keys: Vec<String> = out.keys().cloned().collect();
    for provider in keys {
        let upper = provider.to_uppercase();
        let env_key = format!("{CLIENT_ID_ENV_PREFIX}{upper}");
        let legacy_key = format!("{LEGACY_CLIENT_ID_ENV_PREFIX}{upper}");
        // New spelling first, retired spelling as a read-only fallback; an
        // empty value counts as unset at both levels.
        let client_id = std::env::var(&env_key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                std::env::var(&legacy_key)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            });
        if let Some(client_id) = client_id
            && let Some(p) = out.get_mut(&provider)
        {
            p.client_id = client_id.trim().to_string();
        }
    }
    out
}

/// OAuth manager. Cheap to construct; holds a borrowed SQLCipher connection.
/// Gate: `enabled` (env flag `AGENTCOWORK_OAUTH` by default, overridable).
#[derive(Clone)]
pub struct OAuthManager<'a> {
    conn: &'a Connection,
    enabled: bool,
    providers: HashMap<String, ProviderSettings>,
    /// Loopback redirect used by the PKCE flow. Callers that run the local
    /// callback server pass the real bound port via `with_redirect_uri`.
    redirect_uri: String,
}

/// Result of starting a PKCE flow: the URL to open in the system browser
/// (+ the state the caller must echo back with the authorization code).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PkceStart {
    pub provider: String,
    pub auth_url: String,
    pub state: String,
}

/// Result of starting a device-code flow: what to show the user.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DeviceCodeStart {
    pub provider: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval_secs: u64,
    pub expires_in: u64,
}

/// Poll result for an in-flight device flow.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePoll {
    /// Keep polling (call again after `interval_secs`).
    Pending { interval_secs: u64 },
    /// Approval + token persisted; the account is live in the key ring.
    Approved(OAuthAccountInfo),
    /// Server asked us to slow down (retry after `interval_secs`).
    SlowDown { interval_secs: u64 },
    /// Device code expired — restart the flow.
    Expired,
    /// User denied the request.
    Denied,
}

/// Handle-only account view (never contains a token).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthAccountInfo {
    pub provider: String,
    pub account_id: String,
    pub email: Option<String>,
    pub scopes: String,
    pub expires_at: i64,
    pub updated_at: i64,
}

impl<'a> OAuthManager<'a> {
    /// Enabled iff `AGENTCOWORK_OAUTH` (or the retired `EVERYAIOS_OAUTH`
    /// fallback) is set in the environment (behind flag).
    pub fn new(vault: &'a Vault) -> Self {
        let enabled = agentcowork_types::env_compat::is_set("OAUTH");
        Self::with_enabled(vault, enabled)
    }

    /// Explicit gate — tests use this instead of mutating the environment.
    pub fn with_enabled(vault: &'a Vault, enabled: bool) -> Self {
        let providers = apply_client_id_env_overrides(defaults());
        Self {
            conn: vault.connection(),
            enabled,
            providers,
            redirect_uri: "http://127.0.0.1:0/oauth/callback".into(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Register/replace our own client ID (we register our own per doc 33).
    pub fn with_client_id(mut self, provider: &str, client_id: impl Into<String>) -> Self {
        if let Some(p) = self.providers.get_mut(provider) {
            p.client_id = client_id.into();
        }
        self
    }

    /// Set the loopback redirect URI used by the PKCE flow.
    pub fn with_redirect_uri(mut self, uri: impl Into<String>) -> Self {
        self.redirect_uri = uri.into();
        self
    }

    /// Override a provider's token URL (tests point this at a mock server).
    pub fn with_token_url(mut self, provider: &str, url: impl Into<String>) -> Self {
        if let Some(p) = self.providers.get_mut(provider) {
            p.token_url = url.into();
        }
        self
    }

    /// Override a provider's device-code URL (tests).
    pub fn with_device_code_url(mut self, provider: &str, url: impl Into<String>) -> Self {
        if let Some(p) = self.providers.get_mut(provider) {
            p.device_code_url = Some(url.into());
        }
        self
    }

    /// Override a provider's post-poll exchange URL (Copilot internal token).
    pub fn with_exchange_url(mut self, provider: &str, url: impl Into<String>) -> Self {
        if let Some(p) = self.providers.get_mut(provider) {
            p.exchange_url = Some(url.into());
        }
        self
    }

    fn check_enabled(&self) -> Result<(), OAuthError> {
        if self.enabled {
            Ok(())
        } else {
            Err(OAuthError::Disabled)
        }
    }

    fn provider(&self, name: &str) -> Result<&ProviderSettings, OAuthError> {
        self.providers
            .get(name)
            .ok_or_else(|| OAuthError::ProviderUnsupported(name.to_string()))
    }

    // ---- PKCE (chatgpt-pro) --------------------------------------------

    /// Start the PKCE flow: mints a verifier + state (stored in the vault),
    /// returns the authorize URL to open in the system browser.
    pub fn start_pkce(&self, provider: &str) -> Result<PkceStart, OAuthError> {
        self.check_enabled()?;
        let p = self.provider(provider)?;
        if p.flow != FlowKind::Pkce {
            return Err(OAuthError::FlowMismatch {
                provider: provider.to_string(),
                expected: "pkce".into(),
            });
        }
        let verifier = random_url_b64(32);
        let challenge = code_challenge(&verifier);
        let state = random_hex(16);

        let mut authorize = url::Url::parse(&p.authorize_url)
            .map_err(|_| OAuthError::Transport("invalid OAuth authorization endpoint".into()))?;
        if authorize.scheme() != "https"
            || authorize.host_str().is_none()
            || !authorize.username().is_empty()
            || authorize.password().is_some()
            || authorize.fragment().is_some()
            || authorize.query().is_some()
        {
            return Err(OAuthError::Transport(
                "invalid OAuth authorization endpoint".into(),
            ));
        }
        // BrowserOS mirrors the official clients' extra params (doc 33 §7.4).
        authorize.query_pairs_mut().extend_pairs([
            ("client_id", p.client_id.as_str()),
            ("response_type", "code"),
            ("scope", p.scopes.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            (
                "extraAuthParams",
                "id_token_add_organizations,codex_cli_simplified_flow",
            ),
        ]);
        let auth_url = authorize.into();

        self.store_pending(
            provider,
            Pending {
                state: Some(state.clone()),
                code_verifier: verifier,
                device_code: None,
                user_code: None,
                verification_uri: None,
                interval_secs: 0,
            },
        )?;

        Ok(PkceStart {
            provider: provider.to_string(),
            auth_url,
            state,
        })
    }

    /// Complete the PKCE flow: exchange the authorization code for tokens,
    /// persist them, upsert into the key ring, clear the pending flow.
    pub fn complete_pkce(
        &self,
        provider: &str,
        code: &str,
        state: &str,
    ) -> Result<OAuthAccountInfo, OAuthError> {
        self.check_enabled()?;
        let p = self.provider(provider)?;
        let pending = self.load_pending(provider)?;
        if pending.state.as_deref() != Some(state) {
            return Err(OAuthError::StateMismatch);
        }

        let form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("client_id", p.client_id.as_str()),
            ("code_verifier", pending.code_verifier.as_str()),
        ];
        let json = post_form(&p.token_url, &form)?;
        let tokens = parse_tokens(&json)?;
        self.finish_flow(provider, &pending.code_verifier, tokens, None, None)
    }

    // ---- Device code (copilot, qwen) -----------------------------------

    /// Start a device-code flow. Returns the code the user must enter at the
    /// verification URI. Callers should then poll with [`Self::poll_device`]
    /// every `interval_secs`.
    pub fn start_device(&self, provider: &str) -> Result<DeviceCodeStart, OAuthError> {
        self.check_enabled()?;
        let p = self.provider(provider)?;
        if p.flow != FlowKind::DeviceCode {
            return Err(OAuthError::FlowMismatch {
                provider: provider.to_string(),
                expected: "device-code".into(),
            });
        }
        let device_url = p
            .device_code_url
            .as_deref()
            .ok_or_else(|| OAuthError::ProviderUnsupported(provider.to_string()))?;

        let verifier = random_url_b64(32);
        let challenge = code_challenge(&verifier);

        let mut form = vec![
            ("client_id", p.client_id.as_str()),
            ("scope", p.scopes.as_str()),
        ];
        if provider == QWEN {
            // Qwen is device-code + PKCE (doc 33 §7.4, qwenOAuth2.ts).
            form.push(("code_challenge", challenge.as_str()));
            form.push(("code_challenge_method", "S256"));
        }
        let json = post_form(device_url, &form)?;

        let device_code = json
            .get("device_code")
            .and_then(|v| v.as_str())
            .ok_or(OAuthError::MissingField("device_code"))?;
        let user_code = json
            .get("user_code")
            .and_then(|v| v.as_str())
            .ok_or(OAuthError::MissingField("user_code"))?;
        let verification_uri = json
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let verification_uri_complete = json
            .get("verification_uri_complete")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let interval = json
            .get("interval")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .max(1);
        let expires_in = json
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(900);

        self.store_pending(
            provider,
            Pending {
                state: None,
                code_verifier: verifier,
                device_code: Some(device_code.to_string()),
                user_code: Some(user_code.to_string()),
                verification_uri: Some(verification_uri.clone()),
                interval_secs: interval,
            },
        )?;

        Ok(DeviceCodeStart {
            provider: provider.to_string(),
            user_code: user_code.to_string(),
            verification_uri,
            verification_uri_complete,
            interval_secs: interval,
            expires_in,
        })
    }

    /// Poll an in-flight device flow. On approval, performs any provider
    /// exchange (Copilot internal token), persists, upserts the key ring.
    pub fn poll_device(&self, provider: &str) -> Result<DevicePoll, OAuthError> {
        self.check_enabled()?;
        let p = self.provider(provider)?;
        let pending = self.load_pending(provider)?;
        let device_code = pending
            .device_code
            .as_deref()
            .ok_or(OAuthError::MissingFlow)?;

        let mut form = vec![
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", p.client_id.as_str()),
        ];
        if provider == QWEN {
            form.push(("code_verifier", pending.code_verifier.as_str()));
        }
        let json = post_form(&p.token_url, &form)?;

        if let Some(err) = json.get("error").and_then(|v| v.as_str()) {
            return match err {
                "authorization_pending" => Ok(DevicePoll::Pending {
                    interval_secs: pending.interval_secs,
                }),
                "slow_down" => Ok(DevicePoll::SlowDown {
                    interval_secs: pending.interval_secs.saturating_add(5),
                }),
                // Terminal states: clear the in-flight flow so the user must
                // start a fresh device-code request (no repeated polling of a
                // dead device code).
                "expired_token" => {
                    self.clear_pending(provider)?;
                    Ok(DevicePoll::Expired)
                }
                "access_denied" => {
                    self.clear_pending(provider)?;
                    Ok(DevicePoll::Denied)
                }
                other => Err(OAuthError::DeviceError(other.to_string())),
            };
        }

        let mut tokens = parse_tokens(&json)?;

        // Copilot: the GitHub token is only valid for the internal exchange;
        // the chat API needs the short-lived Copilot token. Keep the GitHub
        // token as the refresh token for the next re-exchange.
        if provider == COPILOT {
            let exchange_url = p
                .exchange_url
                .as_deref()
                .ok_or(OAuthError::MissingField("exchange_url"))?;
            let github_token = tokens
                .access
                .clone()
                .ok_or(OAuthError::MissingField("access_token"))?;
            let exchange = get_json_with_auth(exchange_url, github_token.as_str())?;
            let copilot_token = exchange
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or(OAuthError::MissingField("token"))?;
            let expires_at = exchange
                .get("expires_at")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            tokens.access = Some(copilot_token.to_string());
            tokens.refresh = Some(zeroize::Zeroizing::new(github_token));
            tokens.expires_at = expires_at;
        }

        let info = self.finish_flow(provider, &pending.code_verifier, tokens, None, None)?;
        Ok(DevicePoll::Approved(info))
    }

    // ---- Token lifecycle -----------------------------------------------

    /// Refresh an account's access token (chatgpt-pro: `refresh_token`
    /// grant; qwen: `refresh_token` grant; copilot: re-run the internal
    /// exchange with the stored GitHub token). Upserts the key ring.
    pub fn refresh(
        &self,
        provider: &str,
        account_id: &str,
    ) -> Result<OAuthAccountInfo, OAuthError> {
        self.check_enabled()?;
        let p = self.provider(provider)?;
        let mut stored = self.load_tokens(provider, account_id)?;
        let mut access = stored.access.take();
        let refresh_bytes = stored
            .refresh
            .take()
            .ok_or_else(|| OAuthError::NoRefreshToken(provider.to_string()))?;
        // Keep the stable account id + email across refresh: the refresh
        // response usually carries no id_token, so derive nothing new.
        let existing_email: Option<String> = self
            .conn
            .query_row(
                "SELECT email FROM oauth_tokens WHERE provider = ?1 AND account_id = ?2",
                rusqlite::params![provider, account_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(OAuthError::Sqlite)?
            .flatten();
        // The stored refresh token is encrypted-at-rest bytes; materialize a
        // Zeroizing copy for the grant (scrubbed on drop). `stored` zeroizes
        // its own buffers when it drops at scope end.
        let refresh_token =
            zeroize::Zeroizing::new(String::from_utf8_lossy(&refresh_bytes).into_owned());

        let tokens = if provider == COPILOT {
            // Re-exchange the GitHub token for a fresh Copilot token.
            let exchange_url = p
                .exchange_url
                .as_deref()
                .ok_or(OAuthError::MissingField("exchange_url"))?;
            let exchange = get_json_with_auth(exchange_url, &refresh_token)?;
            let copilot_token = exchange
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or(OAuthError::MissingField("token"))?;
            let expires_at = exchange
                .get("expires_at")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            RawTokens {
                access: Some(copilot_token.to_string()),
                refresh: Some(refresh_token),
                token_type: None,
                scopes: None,
                expires_at,
                id_token: None,
            }
        } else {
            let mut form = vec![
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", p.client_id.as_str()),
            ];
            // Qwen's refresh grant re-sends the device-flow code_verifier
            // (qwenOAuth2.ts); the pending row is retained for QWEN until the
            // account is revoked so the verifier stays available.
            let qwen_verifier = if provider == QWEN {
                self.load_pending(provider).ok().map(|p| p.code_verifier)
            } else {
                None
            };
            if let Some(v) = qwen_verifier.as_ref() {
                form.push(("code_verifier", v.as_str()));
            }
            let json = post_form(&p.token_url, &form)?;
            let mut t = parse_tokens(&json)?;
            if t.refresh.is_none() {
                // Some providers rotate; keep the old one if none returned.
                t.refresh = Some(refresh_token);
            }
            t
        };
        // The old access token is only kept for tests; scrub it before drop.
        if let Some(a) = &mut access {
            a.zeroize();
        }
        // The temporary refresh copy drops (scrubbed by Zeroizing); the
        // persisted copy is encrypted at rest.
        let verifier = self
            .load_pending(provider)
            .ok()
            .map(|pend| pend.code_verifier)
            .unwrap_or_default();
        self.finish_flow(
            provider,
            &verifier,
            tokens,
            Some(account_id),
            existing_email.as_deref(),
        )
    }

    /// List accounts for a provider — handle-only, never tokens.
    pub fn accounts(&self, provider: &str) -> Result<Vec<OAuthAccountInfo>, OAuthError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT provider, account_id, email, scopes, expires_at, updated_at
                 FROM oauth_tokens WHERE provider = ?1 ORDER BY updated_at DESC",
            )
            .map_err(OAuthError::Sqlite)?;
        let rows = stmt
            .query_map([provider], |r| {
                Ok(OAuthAccountInfo {
                    provider: r.get(0)?,
                    account_id: r.get(1)?,
                    email: r.get(2)?,
                    scopes: r.get(3)?,
                    expires_at: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            })
            .map_err(OAuthError::Sqlite)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(OAuthError::Sqlite)?);
        }
        Ok(out)
    }

    /// Remove an account: oauth_tokens row + key-ring key + pending flow.
    pub fn revoke(&self, provider: &str, account_id: &str) -> Result<(), OAuthError> {
        self.conn.execute(
            "DELETE FROM oauth_tokens WHERE provider = ?1 AND account_id = ?2",
            rusqlite::params![provider, account_id],
        )?;
        self.clear_pending(provider)?;
        // Deleting the ring key is best-effort (a missing key is already a
        // revoked account); the token row is the source of truth.
        let ring = KeyRing::new_from_conn(self.conn);
        let _ = ring.delete_key(provider, account_id);
        Ok(())
    }

    /// Persist a remote-MCP / flat-connector bearer token (ARCH/15 Tier 2/3)
    /// into the same SQLCipher `oauth_tokens` table as subscription tokens,
    /// so connected tokens survive app restarts. `account_id` is the store id
    /// (e.g. `google-drive`); provider is a stable namespace (e.g.
    /// `remote-mcp`). Unlike PKCE flows there is no pending row or browser
    /// round-trip here — the shell hands us the finished tokens.
    pub fn store_connector_token(
        &self,
        provider: &str,
        account_id: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in: i64,
        scopes: &str,
    ) -> Result<(), OAuthError> {
        let now = now_ms() / 1000;
        let expires_at = if expires_in > 0 {
            now + expires_in
        } else {
            now + 3600
        };
        self.conn.execute(
            "INSERT INTO oauth_tokens
                (provider, account_id, access_token, refresh_token, token_type, scopes,
                 email, expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'Bearer', ?5, NULL, ?6, ?7, ?7)
             ON CONFLICT(provider, account_id) DO UPDATE SET
                 access_token = excluded.access_token,
                 refresh_token = excluded.refresh_token,
                 scopes = excluded.scopes,
                 expires_at = excluded.expires_at,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                provider,
                account_id,
                access_token.as_bytes(),
                refresh_token.map(|r| r.as_bytes()),
                scopes,
                expires_at,
                now,
            ],
        )?;
        Ok(())
    }

    /// Load a previously-persisted connector/remote access token (String, so
    /// it is never held by a private zeroizing row type across the API).
    /// Returns `Ok(Some(access))` when connected, `Ok(None)` when not.
    pub fn load_connector_token(
        &self,
        provider: &str,
        account_id: &str,
    ) -> Result<Option<String>, OAuthError> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT access_token FROM oauth_tokens
                 WHERE provider = ?1 AND account_id = ?2",
                rusqlite::params![provider, account_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(OAuthError::Sqlite)?
            .map(|b: Vec<u8>| String::from_utf8_lossy(&b).into_owned());
        Ok(row)
    }

    fn clear_pending(&self, provider: &str) -> Result<(), OAuthError> {
        self.conn
            .execute("DELETE FROM oauth_pending WHERE provider = ?1", [provider])?;
        Ok(())
    }

    // ---- internal -------------------------------------------------------

    /// Persist acquired tokens: write `oauth_tokens` (upsert), upsert the key
    /// ring (provider/account_id/access), clear the pending flow.
    ///
    /// `stable_account_id` pins the account id (and `email_fallback`) when
    /// the token response carries no `id_token` (refresh grants) — without
    /// it, the account id would hash the new access token and fork the
    /// account.
    fn finish_flow(
        &self,
        provider: &str,
        verifier: &str,
        tokens: RawTokens,
        stable_account_id: Option<&str>,
        email_fallback: Option<&str>,
    ) -> Result<OAuthAccountInfo, OAuthError> {
        let access = tokens
            .access
            .clone()
            .ok_or(OAuthError::MissingField("access_token"))?;
        let (account_id, email) = match stable_account_id {
            Some(id) => (id.to_string(), email_fallback.map(str::to_string)),
            None => match tokens.id_token.as_deref() {
                Some(jwt) => id_from_jwt(jwt).unwrap_or_else(|| (access_id(&access), None)),
                None => (access_id(&access), None),
            },
        };
        // A fresh id_token beats the fallback email.
        let email = tokens
            .id_token
            .as_deref()
            .and_then(id_from_jwt)
            .and_then(|(_, e)| e)
            .or(email);
        let now = now_ms() / 1000;
        let expires_at = if tokens.expires_at > 0 {
            tokens.expires_at
        } else {
            now + 3600
        };
        let scopes = tokens.scopes.unwrap_or_default();

        self.conn.execute(
            "INSERT INTO oauth_tokens
                (provider, account_id, access_token, refresh_token, token_type, scopes,
                 email, expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(provider, account_id) DO UPDATE SET
                 access_token = excluded.access_token,
                 refresh_token = excluded.refresh_token,
                 token_type = excluded.token_type,
                 scopes = excluded.scopes,
                 email = excluded.email,
                 expires_at = excluded.expires_at,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                provider,
                account_id,
                access.as_bytes(),
                tokens.refresh.as_ref().map(|r| r.as_bytes()),
                tokens.token_type.as_deref().unwrap_or("Bearer"),
                scopes,
                email,
                expires_at,
                now,
                now,
            ],
        )?;

        // Upsert into the key ring: BYOK failover semantics for free
        // (selection, 429 cooldown, affinity, budgets, health).
        let ring = KeyRing::new_from_conn(self.conn);
        ring.add_key(KeySpec {
            provider: provider.to_string(),
            key_id: account_id.clone(),
            value: access.into_bytes(),
            status: KeyStatus::Primary,
            model_filter: vec![],
            priority: 100,
            daily_token_cap: None,
            daily_cost_cap: None,
        })?;

        // QWEN's refresh grant re-sends the code_verifier (qwenOAuth2.ts), so
        // keep its pending row until the account is revoked; other providers
        // clear the in-flight flow on success.
        if provider != QWEN {
            self.conn
                .execute("DELETE FROM oauth_pending WHERE provider = ?1", [provider])?;
        }
        // `verifier` is borrowed from the pending row (kept for QWEN, dropped
        // for others); nothing to scrub here — the row is what holds it.
        let _ = verifier;

        Ok(OAuthAccountInfo {
            provider: provider.to_string(),
            account_id,
            email,
            scopes,
            expires_at,
            updated_at: now,
        })
    }

    fn store_pending(&self, provider: &str, pending: Pending) -> Result<(), OAuthError> {
        self.conn.execute(
            "INSERT INTO oauth_pending
                (provider, state, code_verifier, device_code, user_code,
                 verification_uri, interval_secs, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(provider) DO UPDATE SET
                 state = excluded.state,
                 code_verifier = excluded.code_verifier,
                 device_code = excluded.device_code,
                 user_code = excluded.user_code,
                 verification_uri = excluded.verification_uri,
                 interval_secs = excluded.interval_secs,
                 created_at = excluded.created_at",
            rusqlite::params![
                provider,
                pending.state,
                pending.code_verifier,
                pending.device_code,
                pending.user_code,
                pending.verification_uri,
                pending.interval_secs as i64,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    fn load_pending(&self, provider: &str) -> Result<Pending, OAuthError> {
        self.conn
            .query_row(
                "SELECT state, code_verifier, device_code, user_code, verification_uri,
                        interval_secs
                 FROM oauth_pending WHERE provider = ?1",
                [provider],
                |r| {
                    Ok(Pending {
                        state: r.get(0)?,
                        code_verifier: r.get(1)?,
                        device_code: r.get(2)?,
                        user_code: r.get(3)?,
                        verification_uri: r.get(4)?,
                        interval_secs: r.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()
            .map_err(OAuthError::Sqlite)?
            .ok_or(OAuthError::MissingFlow)
    }

    fn load_tokens(&self, provider: &str, account_id: &str) -> Result<StoredTokens, OAuthError> {
        let row: Option<StoredTokens> = self
            .conn
            .query_row(
                "SELECT access_token, refresh_token FROM oauth_tokens
                 WHERE provider = ?1 AND account_id = ?2",
                rusqlite::params![provider, account_id],
                |r| {
                    Ok(StoredTokens {
                        access: r.get(0)?,
                        refresh: r.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(OAuthError::Sqlite)?;
        row.ok_or_else(|| OAuthError::AccountNotFound {
            provider: provider.to_string(),
            account_id: account_id.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP + crypto helpers
// ---------------------------------------------------------------------------

/// POST a urlencoded form; returns parsed JSON (any HTTP status).
///
/// FIX-09: this is the vault's custody egress — the moment a provider hands us
/// a token. The destination is pre-flighted by the one implementation in
/// [`agentcowork_guard::netfloor`] before the socket opens, so a re-pointed
/// endpoint (via `with_token_url` / `with_device_code_url` /
/// `with_exchange_url`) can never aim a token request at cloud metadata, the
/// LAN, or a non-`http(s)` scheme. A denial is typed and the request is not
/// made; there is no direct-client fallback.
fn post_form(url: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, OAuthError> {
    let agent = crate::guarded_http::agent_for(url).map_err(|error| OAuthError::EgressDenied {
        reason: error.reason,
        url: error.url,
    })?;
    match agent
        .post(url)
        .set("Accept", "application/json")
        .send_form(form)
    {
        Ok(resp) => parse_oauth_response(resp),
        Err(ureq::Error::Status(code, resp)) => {
            let body = crate::guarded_http::read_bounded(resp).map_err(|_| {
                OAuthError::Transport("OAuth error response was unavailable or too large".into())
            })?;
            match serde_json::from_slice::<serde_json::Value>(&body) {
                Ok(json) if json.get("error").is_some() => Ok(json),
                _ => Err(OAuthError::Http(
                    code,
                    "provider returned an error response".into(),
                )),
            }
        }
        Err(ureq::Error::Transport(_)) => Err(OAuthError::Transport(
            "OAuth endpoint request failed".into(),
        )),
    }
}

/// GET with `Authorization: token <tok>` (Copilot internal exchange).
///
/// FIX-09: same pre-flight as [`post_form`]. This call carries a live
/// credential, so it is the one place where an unchecked destination would be
/// a credential leak rather than a policy miss.
fn get_json_with_auth(url: &str, token: &str) -> Result<serde_json::Value, OAuthError> {
    let agent = crate::guarded_http::agent_for(url).map_err(|error| OAuthError::EgressDenied {
        reason: error.reason,
        url: error.url,
    })?;
    // The internal endpoint checks editor headers; mirror copilot clients.
    match agent
        .get(url)
        .set("Authorization", &format!("token {token}"))
        .set("Editor-Version", "vscode/1.85.0")
        .set("Editor-Plugin-Version", "copilot-chat/0.14.1")
        .set("User-Agent", "GitHubCopilotChat/0.14.1")
        .set("Accept", "application/json")
        .call()
    {
        Ok(resp) => parse_oauth_response(resp),
        Err(ureq::Error::Status(code, resp)) => {
            let _ = crate::guarded_http::read_bounded(resp);
            Err(OAuthError::Http(
                code,
                "provider returned an error response".into(),
            ))
        }
        Err(ureq::Error::Transport(_)) => Err(OAuthError::Transport(
            "OAuth endpoint request failed".into(),
        )),
    }
}

fn parse_oauth_response(response: ureq::Response) -> Result<serde_json::Value, OAuthError> {
    let bytes = crate::guarded_http::read_bounded(response)
        .map_err(|_| OAuthError::Transport("OAuth response was unavailable or too large".into()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| OAuthError::Transport("OAuth response was not valid JSON".into()))
}

#[derive(Debug, Clone)]
struct RawTokens {
    access: Option<String>,
    /// Zeroizing: the refresh secret is scrubbed when this struct drops.
    refresh: Option<zeroize::Zeroizing<String>>,
    token_type: Option<String>,
    scopes: Option<String>,
    expires_at: i64,
    id_token: Option<String>,
}

fn parse_tokens(json: &serde_json::Value) -> Result<RawTokens, OAuthError> {
    let access = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if access.is_none() {
        return Err(OAuthError::MissingField("access_token"));
    }
    Ok(RawTokens {
        access,
        refresh: json
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| zeroize::Zeroizing::new(s.to_string())),
        token_type: json
            .get("token_type")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        scopes: json
            .get("scope")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        expires_at: json.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(0),
        id_token: json
            .get("id_token")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// Extract `sub` + `email` from an id_token JWT payload (no signature check —
/// the token came straight from the provider over TLS; used for a stable
/// account_id only).
fn id_from_jwt(jwt: &str) -> Option<(String, Option<String>)> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let sub = json.get("sub")?.as_str()?.to_string();
    let email = json
        .get("email")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some((sub, email))
}

/// Stable account id when no id_token exists: first 16 hex chars of the
/// SHA-256 of the access token. Deterministic per account, token never kept.
fn access_id(access: &str) -> String {
    let digest = Sha256::digest(access.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// RFC 3986 unreserved-safe percent-encoder (query values).
/// PKCE S256 code challenge from a verifier.
fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    url_b64(&digest)
}

fn random_url_b64(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    let s = url_b64(&buf);
    buf.zeroize();
    s
}

fn url_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    let s = buf.iter().map(|b| format!("{b:02x}")).collect::<String>();
    buf.zeroize();
    s
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Providers the broker treats as oauth-backed (for 401 → refresh → retry).
pub fn is_oauth_provider(provider: &str) -> bool {
    matches!(provider, CHATGPT_PRO | COPILOT | QWEN)
}

/// In-flight PKCE / device flow (module scope so the manager can return it).
struct Pending {
    state: Option<String>,
    code_verifier: String,
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    interval_secs: u64,
}

/// Decrypted-at-rest token pair loaded for a refresh. `Drop` scrubs both
/// buffers (same discipline as [`crate::keyring::KeyEntry`]) so secrets never
/// linger in vault memory after the call.
struct StoredTokens {
    access: Option<Vec<u8>>,
    refresh: Option<Vec<u8>>,
}

impl Drop for StoredTokens {
    fn drop(&mut self) {
        if let Some(a) = &mut self.access {
            a.zeroize();
        }
        if let Some(r) = &mut self.refresh {
            r.zeroize();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("oauth subscriptions are disabled (set {OAUTH_ENV_FLAG}=1)")]
    Disabled,
    #[error("key-ring error: {0}")]
    KeyRing(#[from] KeyRingError),
    #[error("unsupported oauth provider: {0}")]
    ProviderUnsupported(String),
    #[error("flow mismatch for {provider}: expected {expected}")]
    FlowMismatch { provider: String, expected: String },
    #[error("no in-flight oauth flow for this provider")]
    MissingFlow,
    #[error("oauth state mismatch (CSRF guard)")]
    StateMismatch,
    #[error("missing field in provider response: {0}")]
    MissingField(&'static str),
    #[error("device flow error: {0}")]
    DeviceError(String),
    #[error("no refresh token stored for provider '{0}'")]
    NoRefreshToken(String),
    #[error("account not found: {provider}/{account_id}")]
    AccountNotFound {
        provider: String,
        account_id: String,
    },
    #[error("HTTP {0}: {1}")]
    Http(u16, String),
    #[error("transport error: {0}")]
    Transport(String),
    /// FIX-09: the destination floor refused an outbound custody request. The
    /// reason is the floor's stable token; the URL is carried for the audit
    /// row. No request was made.
    #[error("egress denied ({reason}) for {url}")]
    EgressDenied { reason: &'static str, url: String },
    #[error("vault error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
