//! `providers.toml` — BYOK key pools per provider (ARCH/03 §3.2).
//!
//! Schema (written to `<data_dir>/providers.toml`, created empty on first boot
//! — the user fills keys via the P1.1 key-management UI, never the log):
//!
//! ```toml
//! [[providers]]
//! name = "openai"
//! base_url = "https://api.openai.com/v1"
//!
//! [[providers.keys]]
//! id = "prod-1"
//! value = "sk-..."
//!
//! [[providers.keys]]
//! id = "prod-2"
//! value = "sk-..."
//! ```
//!
//! A [`KeyPool`] selects keys round-robin so usage spreads across the pool;
//! rotation/exhaustion logic lands with P1.1 — the pool is the data model it
//! rotates over.
//!
//! ## The registry projection ([`provider_registration`])
//!
//! A configured endpoint becomes a **provider registry entry** in the shape the
//! provider plane declares (`ARCH/14` §5): canonical id distinct from the
//! catalog key, one or more runtime transports, exactly one declared class, a
//! health, environments, an epoch, a typed auth method, and an egress
//! declaration naming the host the floor check will be run against.
//!
//! The projection is where custody is proven rather than asserted: a
//! registration carries each key's **id** as its `secret_ref` and never the
//! value, so the entry can be serialized, logged, or exported without carrying a
//! credential. [`assert_no_credential_material`] re-checks the serialized form.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentcowork_catalog::provider_adapter::{
    AdapterClass, AuthMethod, AuthSpec, EgressDeclaration, GatewayIdentityPolicy,
    ProviderRegistration, RegistrationError, custody_findings, validate_registration,
};

/// A single API key in a provider's pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderKey {
    /// Human label, e.g. `prod-1` (rotation target in P1.1).
    pub id: String,
    /// The secret itself. Never logged; vault-encrypted at rest from P1.1.
    pub value: String,
}

/// One provider's configuration: endpoint + a pool of keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub keys: Vec<ProviderKey>,
}

/// The whole `providers.toml` file.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ProvidersFile {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

impl ProvidersFile {
    /// `providers.toml` path inside the data dir.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("providers.toml")
    }

    /// Load from `path`, creating an (empty) file if missing.
    pub fn load_from(path: &Path) -> Result<Self, ProvidersError> {
        if !path.exists() {
            let file = ProvidersFile::default();
            file.save(path)?;
            return Ok(file);
        }
        let raw = std::fs::read_to_string(path).map_err(ProvidersError::Io)?;
        toml::from_str(&raw).map_err(ProvidersError::Parse)
    }

    /// Persist to `path`, creating the parent dir if needed.
    pub fn save(&self, path: &Path) -> Result<(), ProvidersError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ProvidersError::Io)?;
        }
        let toml = toml::to_string_pretty(self).map_err(ProvidersError::Serialize)?;
        std::fs::write(path, toml).map_err(ProvidersError::Io)
    }

    /// Build a live round-robin [`KeyPool`] for the named provider.
    pub fn pool(&self, name: &str) -> Option<KeyPool> {
        self.providers
            .iter()
            .find(|p| p.name == name)
            .map(|p| KeyPool::new(p.clone()))
    }
}

/// Round-robin key selector for one provider. `&self`-safe (internal mutex).
pub struct KeyPool {
    name: String,
    base_url: Option<String>,
    inner: Mutex<VecDeque<ProviderKey>>,
}

impl KeyPool {
    pub fn new(cfg: ProviderConfig) -> Self {
        Self {
            name: cfg.name,
            base_url: cfg.base_url,
            inner: Mutex::new(cfg.keys.into()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("pool poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Select the next key, rotating the queue (round-robin across the pool).
    pub fn select(&self) -> Result<ProviderKey, ProvidersError> {
        let mut queue = self.inner.lock().expect("pool poisoned");
        let key = queue
            .pop_front()
            .ok_or_else(|| ProvidersError::NoKeys(self.name.clone()))?;
        queue.push_back(key.clone());
        Ok(key)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProvidersError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("provider '{0}' has no keys in its pool")]
    NoKeys(String),
    /// The endpoint could not be registered under the provider-plane rules.
    #[error("provider registration refused: {0}")]
    Registration(#[from] RegistrationError),
    /// Custody: a serialized registration carried credential material. This is
    /// a hard failure, never a warning to be logged and ignored.
    #[error("provider registration carries credential material at {0}: {1}")]
    CustodyViolation(String, String),
}

// ---------------------------------------------------------------------------
// The registry projection (`ARCH/14` §5)
// ---------------------------------------------------------------------------

/// The host of a base URL, for the egress declaration. Returns `None` when the
/// URL carries no authority (`""` is a refusal, not a wildcard).
///
/// Deliberately tiny rather than a URL parse: the projection needs the authority
/// for a floor check and nothing else, and a partial parse that silently
/// mis-reads a host is worse than one that declines.
pub fn base_url_host(base_url: &str) -> Option<String> {
    let rest = base_url
        .trim()
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url.trim());
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Drop any userinfo — a base URL that embeds credentials is refused by the
    // custody check below, but the host projection must not carry it either.
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    if authority.is_empty() {
        return None;
    }
    // An IPv6 authority keeps its brackets; a port is stripped from either form.
    let host = if let Some(end) = authority.strip_prefix('[') {
        match end.find(']') {
            Some(close) => format!("[{}]", &end[..close]),
            None => authority.to_string(),
        }
    } else {
        authority.split(':').next().unwrap_or(authority).to_string()
    };
    (!host.is_empty()).then_some(host)
}

/// The default transport ref for a configured endpoint. The wire protocol a
/// given base URL speaks is a property of the provider, not of the URL, so the
/// projection does not guess: one configured endpoint is one transport until a
/// caller that *knows* the gateway hosts several protocols says otherwise via
/// [`with_gateway_transports`].
fn default_transport_ref(provider: &str) -> String {
    format!("wire:{provider}")
}

/// The base URL a configured provider will actually be sent to: the explicit
/// `base_url` when set, otherwise the broker's default for that provider name.
/// The registry entry must name the host the floor check runs against, so the
/// projection resolves the same way the transport does instead of declaring a
/// host nobody dials.
pub fn effective_base_url(cfg: &ProviderConfig) -> Option<String> {
    if let Some(url) = cfg.base_url.as_ref().filter(|u| !u.trim().is_empty()) {
        return Some(url.clone());
    }
    let name = cfg.name.trim();
    agentcowork_vault::broker::DEFAULT_BASE_URLS
        .iter()
        .find(|(provider, _)| *provider == name)
        .map(|(_, url)| (*url).to_string())
}

/// Project one configured provider onto the provider registry entry shape.
///
/// `key_id` is the **handle** the vault resolves at use time; the key's value
/// never crosses into the entry. A provider with no key is refused: an
/// `api`-method entry with no reference has nothing for the credential gate to
/// resolve, so the honest answer is "not registered" rather than an entry that
/// looks usable.
pub fn provider_registration(cfg: &ProviderConfig) -> Result<ProviderRegistration, ProvidersError> {
    let name = cfg.name.trim();
    if name.is_empty() {
        return Err(RegistrationError::MissingId.into());
    }
    let base_url = effective_base_url(cfg).unwrap_or_default();
    let mut hosts = Vec::new();
    if let Some(host) = base_url_host(&base_url) {
        hosts.push(host);
    }
    let mut registration = ProviderRegistration {
        id: name.to_string(),
        catalog_ref: format!("catalog/{name}"),
        transport_refs: vec![default_transport_ref(name)],
        class: AdapterClass::Http,
        version: "1.0.0".into(),
        health: agentcowork_catalog::provider_adapter::ProviderHealth::Unknown,
        capabilities: Vec::new(),
        environments: vec!["local".into()],
        // 1 until the adapter instance reports a restart; the epoch only ever
        // moves forward, and a bump invalidates outstanding handles.
        epoch: 1,
        // The auth *method* plus the first key's id as the vault reference. No
        // value, no token, no header (CTR-013).
        auth: AuthSpec {
            method: AuthMethod::Api,
            secret_ref: cfg
                .keys
                .first()
                .map(|k| format!("vault://provider/{name}/{}", k.id))
                .unwrap_or_default(),
            prompt: String::new(),
            validation: String::new(),
        },
        egress: Some(EgressDeclaration {
            hosts,
            // Loopback is a first-class local runtime, so the platform default
            // stands unless the configured host says otherwise; private/LAN is
            // never implied by a configured endpoint.
            allow_loopback: true,
            allow_private: false,
        }),
    };
    if registration
        .egress
        .as_ref()
        .is_some_and(|e| !e.is_checkable())
    {
        registration.egress = None;
    }
    validate_registration(&registration, None)?;
    Ok(registration)
}

/// Attach the additional wire protocols a gateway-class entry hosts. Several
/// transports share one canonical id and one catalog key — never the reverse
/// (`ARCH/14` §5, A10).
pub fn with_gateway_transports(
    mut registration: ProviderRegistration,
    transports: &[&str],
) -> Result<ProviderRegistration, ProvidersError> {
    for transport in transports {
        registration = registration.with_transport(*transport);
    }
    validate_registration(&registration, None)?;
    Ok(registration)
}

/// Project every configured provider, refusing the whole file if any entry is
/// malformed. A half-registered provider set is a worse state than none.
pub fn provider_registrations(
    file: &ProvidersFile,
) -> Result<Vec<ProviderRegistration>, ProvidersError> {
    let mut out = Vec::new();
    for cfg in &file.providers {
        let registration = provider_registration(cfg)?;
        assert_no_credential_material(&registration)?;
        out.push(registration);
    }
    Ok(out)
}

/// Re-scan a serialized registration for credential material and refuse it. The
/// type cannot express a secret; this guards the field someone adds next.
pub fn assert_no_credential_material(
    registration: &ProviderRegistration,
) -> Result<(), ProvidersError> {
    let json = serde_json::to_value(registration)
        .map_err(|e| ProvidersError::CustodyViolation("<entry>".into(), e.to_string()))?;
    match custody_findings(&json).into_iter().next() {
        Some(finding) => Err(ProvidersError::CustodyViolation(
            finding.path,
            finding.detail,
        )),
        None => Ok(()),
    }
}

/// The client identity the transport actually injects, read from the one place
/// that builds it (`agentcowork_vault::broker`), so the registry-declared policy
/// and the wire behaviour cannot drift apart (`REQ-PROV-009`).
pub fn wire_client_identity(session_id: &str) -> Vec<(String, String)> {
    agentcowork_vault::broker::gateway_identity_headers(session_id, "req-conformance")
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// Do the headers the transport injects match the identity policy the provider
/// registry declares? The header *names*, the per-conversation value, and the
/// client identity itself all have to line up.
pub fn gateway_identity_conforms() -> bool {
    let policy = GatewayIdentityPolicy::default();
    let wire = wire_client_identity("conv-conformance");
    let value = |name: &str| {
        wire.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    if value(&policy.session_header).as_deref() != Some("conv-conformance") {
        return false;
    }
    if value(&policy.session_header_alt).as_deref() != Some("conv-conformance") {
        return false;
    }
    if value(&policy.request_header).is_none() || value(&policy.client_header).is_none() {
        return false;
    }
    // The identity is ours: the same string the policy renders, never an
    // impersonation and never a bare SDK name.
    value("user-agent").as_deref() == Some(policy.user_agent(env!("CARGO_PKG_VERSION")).as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[providers]]
name = "openai"
base_url = "https://api.openai.com/v1"

[[providers.keys]]
id = "prod-1"
value = "sk-one"

[[providers.keys]]
id = "prod-2"
value = "sk-two"

[[providers]]
name = "anthropic"

[[providers.keys]]
id = "claude-prod"
value = "sk-ant-three"
"#;

    #[test]
    fn parses_key_pools_per_provider() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        assert_eq!(file.providers.len(), 2);
        assert_eq!(file.providers[0].name, "openai");
        assert_eq!(file.providers[0].keys.len(), 2);
        assert_eq!(file.providers[1].name, "anthropic");
        assert_eq!(file.providers[1].base_url, None); // optional field
        assert_eq!(file.providers[1].keys[0].value, "sk-ant-three");
    }

    #[test]
    fn missing_file_is_created_empty() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-providers-{}", std::process::id()));
        let path = dir.join("providers.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let file = ProvidersFile::load_from(&path).expect("create defaults");
        assert!(file.providers.is_empty());
        assert!(path.exists());
        // Reload round-trips.
        let again = ProvidersFile::load_from(&path).expect("reload");
        assert_eq!(again, file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pool_selects_round_robin() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        let pool = file.pool("openai").expect("pool exists");
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.select().unwrap().id, "prod-1");
        assert_eq!(pool.select().unwrap().id, "prod-2");
        assert_eq!(pool.select().unwrap().id, "prod-1"); // wraps around
        assert_eq!(pool.len(), 2); // rotation never drains the pool
    }

    #[test]
    fn pool_with_single_key_always_selects() {
        // Round-robin with one key never drains the pool — it always returns
        // that key (rotation just keeps it in place).
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        let pool = file.pool("anthropic").expect("pool exists");
        for _ in 0..3 {
            assert_eq!(pool.select().unwrap().id, "claude-prod");
        }
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn empty_pool_select_errors() {
        let pool = KeyPool::new(ProviderConfig {
            name: "empty-provider".into(),
            base_url: None,
            keys: vec![],
        });
        match pool.select() {
            Err(ProvidersError::NoKeys(name)) => assert_eq!(name, "empty-provider"),
            other => panic!("expected NoKeys, got {other:?}"),
        }
    }

    #[test]
    fn unknown_provider_has_no_pool() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        assert!(file.pool("groq").is_none());
    }

    #[test]
    fn roundtrip_through_toml() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        let text = toml::to_string_pretty(&file).expect("serialize");
        let back: ProvidersFile = toml::from_str(&text).expect("reparse");
        assert_eq!(back, file);
    }

    // ---- the provider registry projection (`ARCH/14` §5) --------------------

    #[test]
    fn base_url_host_extracts_the_authority_and_refuses_the_rest() {
        assert_eq!(
            base_url_host("https://api.example.test/v1"),
            Some("api.example.test".to_string())
        );
        assert_eq!(
            base_url_host("https://api.example.test:8443/v1/chat/completions?x=1"),
            Some("api.example.test".to_string())
        );
        assert_eq!(
            base_url_host("http://127.0.0.1:1234/v1"),
            Some("127.0.0.1".to_string())
        );
        assert_eq!(
            base_url_host("http://[::1]:1234/v1"),
            Some("[::1]".to_string())
        );
        // Userinfo is dropped: the host projection never carries a credential.
        assert_eq!(
            base_url_host("https://user:pass@api.example.test/v1"),
            Some("api.example.test".to_string())
        );
        assert_eq!(base_url_host(""), None);
        assert_eq!(base_url_host("   "), None);
        assert_eq!(base_url_host("https:///v1"), None);
    }

    #[test]
    fn a_configured_endpoint_registers_in_the_declared_entry_shape() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        let entries = provider_registrations(&file).expect("registrations");
        assert_eq!(entries.len(), 2);
        let openai = entries.iter().find(|e| e.id == "openai").expect("openai");
        // Exactly one declared class, and canonical id ≠ catalog key ≠ transport.
        assert_eq!(openai.class, AdapterClass::Http);
        assert_eq!(openai.catalog_ref, "catalog/openai");
        assert_eq!(openai.transport_refs, vec!["wire:openai"]);
        assert_ne!(openai.id, openai.catalog_ref);
        assert!(!openai.serves_transport("catalog/openai"));
        assert!(openai.serves_transport("wire:openai"));
        // Health starts unknown — never "ok" by assumption.
        assert_eq!(
            openai.health,
            agentcowork_catalog::provider_adapter::ProviderHealth::Unknown
        );
        assert_eq!(openai.environments, vec!["local"]);
        assert_eq!(openai.epoch, 1);
        // The egress declaration names the host the floor check runs against.
        let egress = openai.egress.as_ref().expect("http class declares egress");
        assert_eq!(
            egress.normalized_hosts().into_iter().collect::<Vec<_>>(),
            vec!["api.openai.com".to_string()]
        );
        assert!(!egress.allow_private);
        // A provider configured without a base URL resolves the host the
        // transport will really dial, so the declaration is never a fiction.
        let anthropic = entries
            .iter()
            .find(|e| e.id == "anthropic")
            .expect("anthropic");
        let hosts = anthropic.egress.as_ref().unwrap().normalized_hosts();
        assert_eq!(
            hosts.into_iter().collect::<Vec<_>>(),
            vec!["api.anthropic.com"]
        );
    }

    #[test]
    fn a_provider_with_no_resolvable_endpoint_is_refused() {
        let cfg = ProviderConfig {
            name: "nowhere".into(),
            base_url: None,
            keys: vec![ProviderKey {
                id: "k1".into(),
                value: "sk-x".into(),
            }],
        };
        // No configured URL and no default ⇒ no host to floor-check ⇒ refused,
        // rather than an `http` registration whose egress cannot be checked.
        assert!(matches!(
            provider_registration(&cfg),
            Err(ProvidersError::Registration(
                RegistrationError::UndeclaredEgress { .. }
            ))
        ));
    }

    #[test]
    fn an_auth_method_carries_the_key_id_never_the_key_value() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        let entries = provider_registrations(&file).expect("registrations");
        let openai = entries.iter().find(|e| e.id == "openai").expect("openai");
        assert_eq!(openai.auth.method, AuthMethod::Api);
        assert_eq!(openai.auth.secret_ref, "vault://provider/openai/prod-1");
        // Custody: the whole file — ids *and* values — never reaches the entry.
        let json = serde_json::to_string(&entries).unwrap();
        for secret in ["sk-one", "sk-two", "sk-ant-three"] {
            assert!(!json.contains(secret), "{secret} leaked into the registry");
        }
        assert!(assert_no_credential_material(openai).is_ok());
    }

    #[test]
    fn a_provider_with_no_keys_still_registers_but_carries_no_reference() {
        let cfg = ProviderConfig {
            name: "fresh".into(),
            base_url: Some("https://api.example.test/v1".into()),
            keys: vec![],
        };
        // The vault reference is required for a key-auth method, so an entry
        // with no key is refused at registration rather than registered with an
        // empty handle the credential gate would later have to interpret.
        let err = provider_registration(&cfg).expect_err("no key, no reference");
        assert!(matches!(err, ProvidersError::Registration(_)), "{err:?}");
    }

    #[test]
    fn several_transports_share_one_entry_without_aliasing_the_canonical_id() {
        let file: ProvidersFile = toml::from_str(SAMPLE).expect("parse");
        let openai = provider_registration(&file.providers[0]).expect("registers");
        let gateway = with_gateway_transports(openai, &["wire:responses", "wire:messages"])
            .expect("multi-transport entry registers");
        assert_eq!(
            gateway.transport_refs,
            vec!["wire:openai", "wire:responses", "wire:messages"]
        );
        assert_eq!(gateway.id, "openai", "one canonical id, three transports");
        assert_eq!(gateway.catalog_ref, "catalog/openai");
        for transport in ["wire:openai", "wire:responses", "wire:messages"] {
            assert!(gateway.serves_transport(transport));
        }
        // A transport equal to the canonical id is refused — a caller must not
        // be able to address a provider by its transport.
        let aliased = with_gateway_transports(gateway, &["openai"]);
        assert!(aliased.is_err(), "an id aliased to a transport is refused");
    }

    #[test]
    fn the_wire_client_identity_matches_the_declared_policy() {
        assert!(
            gateway_identity_conforms(),
            "the injected headers must match GatewayIdentityPolicy"
        );
        let policy = GatewayIdentityPolicy::default();
        let wire = wire_client_identity("conv-1");
        let names: Vec<String> = wire.iter().map(|(k, _)| k.clone()).collect();
        for required in [
            policy.session_header.as_str(),
            policy.session_header_alt.as_str(),
            policy.request_header.as_str(),
            policy.client_header.as_str(),
            "user-agent",
        ] {
            assert!(
                names.iter().any(|n| n.eq_ignore_ascii_case(required)),
                "{required} is declared by the policy but absent from the wire: {names:?}"
            );
        }
        // Affinity is per conversation: a different conversation, a different
        // value, and the identity header is unchanged.
        let other = wire_client_identity("conv-2");
        let value = |h: &[(String, String)], name: &str| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(value(&wire, "x-opencode-session"), "conv-1");
        assert_eq!(value(&other, "x-opencode-session"), "conv-2");
        assert_eq!(value(&wire, "user-agent"), value(&other, "user-agent"));
    }
}
