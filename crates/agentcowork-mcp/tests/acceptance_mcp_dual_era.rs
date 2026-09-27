//! W0 `TASK-PROV-003` (FIX-12 + FIX-13) — the dual-era MCP contract, proved
//! end to end on loopback with no external network.
//!
//! Two halves of one contract:
//!
//! - the **façade** serves the modern revision's mandatory `server/discover`
//!   and validates the `Mcp-Method` / `Mcp-Name` envelope headers
//!   (DEC-030, ARCH/14 §4, REQ-PROV-005);
//! - the **client** speaks that same envelope, probes the modern era first,
//!   falls back to the legacy `2025-11-25` contract, caches the verdict per
//!   origin, and honours the force-legacy escape hatch (REQ-PROV-004).
//!
//! The last two tests drive the real client against the real façade, so a
//! disagreement between the header names the client emits and the ones the
//! façade validates fails here rather than in production.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use agentcowork_mcp::{
    AuthServerMetadata, ClientRegistration, ConnectOptions, DISCOVER_METHOD, EraSource, EraVerdict,
    HttpTransport, LEGACY_PROTOCOL_VERSION, METHOD_HEADER, McpEra, McpResponse, McpServer,
    NAME_HEADER, PROTOCOL_VERSION_HEADER, RemoteError, RemoteTarget, SUPPORTED_PROTOCOL_VERSION,
    StoreEntry, StoreIndex, StoreKind, ToolCallHandler, cached_era, classify_era,
    connect_with_options, modern_headers, negotiate_era_detailed, origin_of, rpc,
};
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(3);

struct Echo;

impl ToolCallHandler for Echo {
    fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({"tool": name, "arguments": arguments}))
    }
}

// ---------------------------------------------------------------------------
// raw loopback HTTP
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Raw {
    status: u16,
    headers: String,
    body: String,
}

impl Raw {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("expected JSON body, got {:?}: {error}", self.body))
    }
}

/// Post one request with an explicit header set, so a test can omit or corrupt
/// any single envelope field.
fn post_raw(addr: SocketAddr, token: &str, headers: &[(&str, &str)], body: &str) -> Raw {
    let port = addr.port();
    let mut head = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let mut stream = TcpStream::connect(addr).expect("connect to loopback lease");
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    stream.write_all(head.as_bytes()).expect("write request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let (head, body) = raw.split_once("\r\n\r\n").expect("response framing");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("HTTP status");
    Raw {
        status,
        headers: head.to_string(),
        body: body.to_string(),
    }
}

// ---------------------------------------------------------------------------
// FIX-13 — the façade
// ---------------------------------------------------------------------------

#[test]
fn acceptance_server_discover_is_served_on_the_modern_lease() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": DISCOVER_METHOD, "params": {}
    })
    .to_string();
    let response = post_raw(
        lease.local_addr(),
        lease.token(),
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, DISCOVER_METHOD),
        ],
        &body,
    );

    assert_eq!(response.status, 200, "body: {}", response.body);
    let value = response.json();
    assert_eq!(value["result"]["protocolVersion"], "2026-07-28");
    assert!(
        response
            .headers
            .to_ascii_lowercase()
            .contains("mcp-protocol-version: 2026-07-28")
    );
    // The legacy revision is advertised as still supported.
    let supported: Vec<&str> = value["result"]["supportedProtocolVersions"]
        .as_array()
        .expect("supportedProtocolVersions")
        .iter()
        .map(|version| version.as_str().expect("revision string"))
        .collect();
    assert!(supported.contains(&"2026-07-28"));
    assert!(supported.contains(&LEGACY_PROTOCOL_VERSION));
    // Discovery exposes task-shaped capability ids, never the native catalog.
    let names: Vec<&str> = value["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    assert!(names.contains(&"office.edit"));
    assert!(!names.contains(&"snapshot"));
    lease.close();
}

#[test]
fn acceptance_the_mcp_method_and_mcp_name_envelope_headers_are_validated() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let addr = lease.local_addr();
    let token = lease.token().to_string();
    let list = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
    })
    .to_string();
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "office.edit", "arguments": {"blockId": "b-1"}}
    })
    .to_string();

    // A missing Mcp-Method is refused: the façade will not guess the method.
    let no_method = post_raw(
        addr,
        &token,
        &[(PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION)],
        &list,
    );
    assert_eq!(no_method.status, 400);
    assert!(
        no_method.json()["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Mcp-Method"))
    );

    // Mcp-Method that disagrees with the JSON-RPC method is refused.
    let mismatched = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/call"),
        ],
        &list,
    );
    assert_eq!(mismatched.status, 400);

    // tools/call without Mcp-Name is refused.
    let no_name = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/call"),
        ],
        &call,
    );
    assert_eq!(no_name.status, 400);
    assert!(
        no_name.json()["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Mcp-Name"))
    );

    // Mcp-Name that disagrees with params.name is refused.
    let wrong_name = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "browser.extract"),
        ],
        &call,
    );
    assert_eq!(wrong_name.status, 400);

    // Mcp-Name is only meaningful for tools/call.
    let stray_name = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/list"),
            (NAME_HEADER, "office.edit"),
        ],
        &list,
    );
    assert_eq!(stray_name.status, 400);

    // A legacy revision is refused on the modern lease: sessions are a
    // non-goal, so only the modern contract is negotiable here.
    let legacy = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, LEGACY_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/list"),
        ],
        &list,
    );
    assert_eq!(legacy.status, 400);

    // The fully valid envelope is accepted.
    let accepted = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "office.edit"),
        ],
        &call,
    );
    assert_eq!(accepted.status, 200, "body: {}", accepted.body);
    assert_eq!(
        accepted.json()["result"]["structuredContent"]["tool"],
        "office.edit"
    );
    lease.close();
}

#[test]
fn acceptance_unknown_method_and_undeclared_tool_are_typed_refusals() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let addr = lease.local_addr();
    let token = lease.token().to_string();

    let unknown_method = serde_json::json!({
        "jsonrpc": "2.0", "id": 4, "method": "resources/list", "params": {}
    })
    .to_string();
    let refused = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "resources/list"),
        ],
        &unknown_method,
    );
    // A refusal is a JSON-RPC answer, not a transport failure or a silent no-op.
    assert_eq!(refused.status, 200);
    let value = refused.json();
    assert_eq!(value["error"]["code"], -32601);
    assert!(value["error"]["data"]["guidance"].is_string());

    // A guessed native primitive is refused with guidance, not exposed.
    let guessed = serde_json::json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "snapshot", "arguments": {}}
    })
    .to_string();
    let refused = post_raw(
        addr,
        &token,
        &[
            (PROTOCOL_VERSION_HEADER, SUPPORTED_PROTOCOL_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "snapshot"),
        ],
        &guessed,
    );
    let value = refused.json();
    assert_eq!(value["error"]["code"], -32602);
    assert!(
        value["error"]["data"]["guidance"]
            .as_str()
            .is_some_and(|guidance| guidance.contains("tools/list"))
    );
    // Nothing in the refusal may hand back the internal native table.
    for leaked in [
        "disk_scan",
        "deep_research",
        "office_edit",
        "filename_search",
    ] {
        assert!(
            !refused.body.contains(leaked),
            "the refusal leaked an internal name: {leaked}"
        );
    }
    lease.close();
}

// ---------------------------------------------------------------------------
// FIX-12 — the client, driving the real façade
// ---------------------------------------------------------------------------

/// A JSON-RPC transport over the loopback lease, so the client half runs
/// against the real server half instead of a mock.
struct LeaseHttp {
    addr: SocketAddr,
    token: String,
}

impl HttpTransport for LeaseHttp {
    fn get_json(&self, _url: &str) -> Result<Value, RemoteError> {
        Err(RemoteError::Msg(
            "this transport speaks JSON-RPC only".into(),
        ))
    }

    fn post_form(&self, _url: &str, _form: &[(&str, &str)]) -> Result<Value, RemoteError> {
        Err(RemoteError::Msg(
            "this transport speaks JSON-RPC only".into(),
        ))
    }

    fn post_json(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _body: &Value,
    ) -> Result<Value, RemoteError> {
        Err(RemoteError::Msg(
            "the era path must use post_json_rpc".into(),
        ))
    }

    fn post_json_rpc(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<McpResponse, RemoteError> {
        let port = self.addr.port();
        let mut head = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n",
            token = self.token
        );
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        let payload = body.to_string();
        head.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ));
        let mut stream = TcpStream::connect(self.addr)
            .map_err(|error| RemoteError::Transport(error.to_string()))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|error| RemoteError::Transport(error.to_string()))?;
        stream
            .set_write_timeout(Some(TIMEOUT))
            .map_err(|error| RemoteError::Transport(error.to_string()))?;
        stream
            .write_all(head.as_bytes())
            .map_err(|error| RemoteError::Transport(error.to_string()))?;
        let mut raw = String::new();
        stream
            .read_to_string(&mut raw)
            .map_err(|error| RemoteError::Transport(error.to_string()))?;
        let (head, payload) = raw
            .split_once("\r\n\r\n")
            .ok_or_else(|| RemoteError::Msg("unframed response".to_string()))?;
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse().ok())
            .ok_or_else(|| RemoteError::Msg("no HTTP status".to_string()))?;
        Ok(McpResponse {
            status,
            body: serde_json::from_str(payload).unwrap_or(Value::Null),
        })
    }
}

fn lease_target(lease: &agentcowork_mcp::McpHttpLease<Echo>) -> RemoteTarget {
    RemoteTarget {
        url: lease.url().to_string(),
        auth: AuthServerMetadata {
            issuer: "http://127.0.0.1".into(),
            authorization_endpoint: String::new(),
            token_endpoint: String::new(),
            registration_endpoint: String::new(),
            scopes_supported: vec![],
            response_types_supported: vec![],
        },
        client: ClientRegistration {
            client_id: "acceptance".into(),
            client_secret: String::new(),
            token_endpoint_auth_method: "none".into(),
        },
        force_legacy: false,
    }
}

#[test]
fn acceptance_the_client_era_probe_speaks_the_envelope_the_facade_validates() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let target = lease_target(&lease);
    let http = LeaseHttp {
        addr: lease.local_addr(),
        token: lease.token().to_string(),
    };

    let listed = rpc(
        &target,
        lease.token(),
        "tools/list",
        serde_json::json!({}),
        &http,
    )
    .expect("a modern client must be able to list on the modern façade");
    assert!(listed["result"]["tools"].is_array());

    // The era was detected once and cached for this origin.
    assert_eq!(cached_era(&origin_of(&target.url)), Some(McpEra::Modern));

    // A call envelope built by the same header helper the façade validates.
    let called = rpc(
        &target,
        lease.token(),
        "tools/call",
        serde_json::json!({"name": "office.edit", "arguments": {"blockId": "b-1"}}),
        &http,
    )
    .expect("a modern tools/call must reach the façade");
    assert_eq!(called["result"]["structuredContent"]["tool"], "office.edit");
    lease.close();
}

#[test]
fn acceptance_force_legacy_speaks_the_legacy_contract_and_the_modern_lease_refuses_it() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let target = lease_target(&lease).with_force_legacy(true);
    let http = LeaseHttp {
        addr: lease.local_addr(),
        token: lease.token().to_string(),
    };

    // The hatch is a client-side decision: a legacy request carries no
    // protocol-version header, so a modern-only lease must refuse it. That
    // refusal is typed, not a silent no-op, and no tool ran.
    let error = rpc(
        &target,
        lease.token(),
        "tools/list",
        serde_json::json!({}),
        &http,
    )
    .expect_err("a header-less legacy request must not pass a strict lease");
    assert!(
        matches!(error, RemoteError::Rpc { status: 400, .. }),
        "expected a typed 400, got {error}"
    );
    lease.close();
}

#[test]
fn acceptance_the_stored_force_legacy_flag_is_reachable_and_skips_detection() {
    // The hatch used to be a crate-level flag no user could reach, which is dead
    // code plus a false sense of coverage. It is now persisted on the stored
    // server record, so this proves both halves: the record round-trips, and a
    // target built from it skips detection with no wire traffic at all.
    let mut entry = StoreEntry {
        id: "operator-forced".into(),
        kind: StoreKind::RemoteMcp,
        name: "Operator-forced server".into(),
        description: "fixture".into(),
        url: Some("https://forced.example.com/mcp".into()),
        force_legacy: true,
        flow: agentcowork_mcp::ConnectFlow::Pkce,
        vault_provider: "fixture".into(),
        consent: agentcowork_mcp::ConnectConsent {
            scopes_plain: vec!["Read nothing; this entry proves the hatch".into()],
            can_mutate: false,
            indexes_into_memory: false,
        },
        tool_hint: 0,
    };
    let store = StoreIndex::with([entry.clone()]);
    assert!(
        store.get("operator-forced").expect("entry").force_legacy,
        "the hatch must survive the store round trip"
    );
    // The field defaults to absent-safe, so an older record cannot arm it.
    entry.force_legacy = false;
    let defaults: StoreEntry =
        serde_json::from_str(r#"{"id":"x","kind":"remote-mcp","name":"x","description":"x","flow":"pkce","consent":{"scopesPlain":["x"]}}"#)
            .expect("a record written before the field existed");
    assert!(
        !defaults.force_legacy,
        "a record without the field must default to the modern era"
    );

    let target = RemoteTarget {
        url: entry.url.clone().expect("url"),
        auth: AuthServerMetadata {
            issuer: "https://auth.example.com".into(),
            authorization_endpoint: String::new(),
            token_endpoint: String::new(),
            registration_endpoint: String::new(),
            scopes_supported: vec![],
            response_types_supported: vec![],
        },
        client: ClientRegistration {
            client_id: "acceptance".into(),
            client_secret: String::new(),
            token_endpoint_auth_method: "none".into(),
        },
        // The stored flag reaches the target — this is the whole point.
        force_legacy: true,
    };
    // A transport that would fail the test if it were touched.
    let http = LeaseHttp {
        addr: "127.0.0.1:1".parse().expect("loopback"),
        token: "unused".into(),
    };
    let negotiation = negotiate_era_detailed(&target, None, &http);
    assert_eq!(negotiation.era, McpEra::Legacy);
    assert_eq!(negotiation.source, EraSource::Forced);
    assert_eq!(negotiation.version(), LEGACY_PROTOCOL_VERSION);
    assert!(
        cached_era(&origin_of(&target.url)).is_none(),
        "a force-legacy server must not poison the shared origin cache"
    );
}

#[test]
fn acceptance_connect_with_options_is_the_persisted_hatch_seam() {
    // `connect()` used to hardcode `force_legacy: false`, so no stored value
    // could ever reach a target. The options struct is the seam the shell reads
    // the record through, and it must not disturb the OAuth half.
    let http = DiscoveryOnly;
    let plain = agentcowork_mcp::connect("https://connect.example.com/mcp", &http)
        .expect("the plain handshake still works");
    assert!(!plain.force_legacy);
    let forced = connect_with_options(
        "https://connect.example.com/mcp",
        &http,
        ConnectOptions { force_legacy: true },
    )
    .expect("the forced handshake still works");
    assert!(forced.force_legacy);
    assert_eq!(forced.url, plain.url);
    assert_eq!(forced.client.client_id, plain.client.client_id);
}

/// Serves only the two well-known documents `connect` reads, so the test never
/// touches the network.
#[derive(Default)]
struct DiscoveryOnly;

impl HttpTransport for DiscoveryOnly {
    fn get_json(&self, url: &str) -> Result<Value, RemoteError> {
        if url.ends_with("/.well-known/oauth-protected-resource") {
            return Ok(serde_json::json!({
                "resource": "https://connect.example.com",
                "authorization_servers": ["https://auth.example.com"]
            }));
        }
        if url.ends_with("/.well-known/oauth-authorization-server") {
            return Ok(serde_json::json!({
                "issuer": "https://auth.example.com",
                "authorization_endpoint": "https://auth.example.com/authorize",
                "token_endpoint": "https://auth.example.com/token",
                "registration_endpoint": "https://auth.example.com/register"
            }));
        }
        Err(RemoteError::Msg(format!("unexpected GET {url}")))
    }

    fn post_form(&self, url: &str, _form: &[(&str, &str)]) -> Result<Value, RemoteError> {
        Err(RemoteError::Msg(format!("unexpected form POST {url}")))
    }

    fn post_json(
        &self,
        url: &str,
        _bearer: Option<&str>,
        _body: &Value,
    ) -> Result<Value, RemoteError> {
        if url.ends_with("/register") {
            return Ok(serde_json::json!({"client_id": "dyn-client"}));
        }
        Err(RemoteError::Msg(format!("unexpected POST {url}")))
    }
}

#[test]
fn acceptance_modern_era_precedence_and_legacy_fallback_are_separable() {
    // The modern era carries the protocol revision, the method, and — for a
    // call — the name.
    let call_headers = modern_headers("tools/call", Some("office.edit"));
    assert!(
        call_headers
            .iter()
            .any(|(name, value)| name == PROTOCOL_VERSION_HEADER && value == "2026-07-28")
    );
    assert!(
        call_headers
            .iter()
            .any(|(name, value)| name == METHOD_HEADER && value == "tools/call")
    );
    assert!(
        call_headers
            .iter()
            .any(|(name, value)| name == NAME_HEADER && value == "office.edit")
    );
    // A non-call method carries no name.
    assert!(
        !modern_headers("tools/list", Some("office.edit"))
            .iter()
            .any(|(name, _)| name == NAME_HEADER)
    );

    assert_eq!(McpEra::Modern.version(), "2026-07-28");
    assert_eq!(McpEra::Legacy.version(), LEGACY_PROTOCOL_VERSION);

    // The classifier reads the refusal body, not just the status: an unknown
    // method means legacy, an auth or availability failure proves nothing.
    assert_eq!(
        classify_era(
            400,
            &serde_json::json!({"error": {"code": -32601, "message": "Method not found"}})
        ),
        EraVerdict::Legacy
    );
    assert_eq!(
        classify_era(401, &serde_json::json!({"error": "unauthorized"})),
        EraVerdict::Inconclusive
    );
}

// ---------------------------------------------------------------------------
// The lease revision pin and the single `initialize` exemption (DEC-030)
// ---------------------------------------------------------------------------

#[test]
fn acceptance_a_legacy_initialize_completes_on_the_strict_lease() {
    // `ARCH/14` §4 promises "stateless modern + `initialize` compatibility" and
    // REQ-PROV-005 requires it. This is the end-to-end proof that the promise
    // is reachable over the strict lease rather than only in-process.
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 11, "method": "initialize",
        "params": {
            "protocolVersion": LEGACY_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "legacy-client", "version": "1"}
        }
    })
    .to_string();
    // No `MCP-Protocol-Version` and no `Mcp-Method`: a legacy client has no
    // reason to send a header from a contract it does not speak.
    let response = post_raw(lease.local_addr(), lease.token(), &[], &body);

    assert_eq!(response.status, 200, "body: {}", response.body);
    let value = response.json();
    assert_eq!(value["result"]["protocolVersion"], LEGACY_PROTOCOL_VERSION);
    assert_eq!(value["result"]["serverInfo"]["name"], "agentcowork-mcp");
    // The compat handshake is how a client discovers the window, so the answer
    // carries it (REQ-CHAN-012).
    let supported: Vec<&str> = value["result"]["supportedProtocolVersions"]
        .as_array()
        .expect("supportedProtocolVersions")
        .iter()
        .map(|version| version.as_str().expect("revision string"))
        .collect();
    assert!(supported.contains(&SUPPORTED_PROTOCOL_VERSION));
    assert!(supported.contains(&LEGACY_PROTOCOL_VERSION));
    // The session-less guarantee is stated, not implied.
    let instructions = value["result"]["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.contains("no session is created"),
        "the compat handshake must stay stateless, got: {instructions}"
    );
    // No session id is minted: a session-bearing exemption would be a contract
    // change, not a compat detail.
    assert!(value["result"].get("sessionId").is_none());
    lease.close();
}

#[test]
fn acceptance_the_revision_pin_holds_for_every_method_except_initialize() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let addr = lease.local_addr();
    let token = lease.token().to_string();
    let list = serde_json::json!({
        "jsonrpc": "2.0", "id": 12, "method": "tools/list", "params": {}
    })
    .to_string();
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 13, "method": "tools/call",
        "params": {"name": "office.edit", "arguments": {}}
    })
    .to_string();

    // A legacy revision on a dispatching method stays refused...
    for (label, body) in [("tools/list", &list), ("tools/call", &call)] {
        let refused = post_raw(
            addr,
            &token,
            &[
                (PROTOCOL_VERSION_HEADER, LEGACY_PROTOCOL_VERSION),
                (METHOD_HEADER, label),
                (NAME_HEADER, "office.edit"),
            ],
            body,
        );
        assert_eq!(refused.status, 400, "{label} must stay pinned");
        let message = refused.json()["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // REQ-CHAN-012: the refusal names the supported window.
        assert!(
            message.contains(SUPPORTED_PROTOCOL_VERSION),
            "{label} refusal must name the pinned revision, got: {message}"
        );
        assert!(
            message.contains(LEGACY_PROTOCOL_VERSION),
            "{label} refusal must name the window, got: {message}"
        );
        assert!(
            message.contains("initialize"),
            "{label} refusal must name the one exempt method, got: {message}"
        );
        let data = refused.json()["error"]["data"]["supportedProtocolVersions"]
            .as_array()
            .expect("window in error data")
            .clone();
        assert!(
            data.iter()
                .any(|version| version == SUPPORTED_PROTOCOL_VERSION),
            "the machine-readable window must be present too"
        );
    }

    // ...and the same header-less envelope that is admitted for `initialize` is
    // refused for `tools/call`, so the exemption cannot be widened by swapping
    // the method on a request that already passed.
    let refused = post_raw(addr, &token, &[], &call);
    assert_eq!(refused.status, 400);
    lease.close();
}

#[test]
fn acceptance_a_comma_duplicated_version_header_is_normalized() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let list = serde_json::json!({
        "jsonrpc": "2.0", "id": 14, "method": "tools/list", "params": {}
    })
    .to_string();

    // A proxy that appended the same header twice is a transport artifact, not
    // a disagreement, so the modern contract still holds.
    let accepted = post_raw(
        lease.local_addr(),
        lease.token(),
        &[
            (PROTOCOL_VERSION_HEADER, "2026-07-28, 2026-07-28"),
            (METHOD_HEADER, "tools/list"),
        ],
        &list,
    );
    assert_eq!(accepted.status, 200, "body: {}", accepted.body);
    assert!(accepted.json()["result"]["tools"].is_array());

    // Conflicting revisions are a real disagreement: refused, never resolved by
    // picking one.
    let refused = post_raw(
        lease.local_addr(),
        lease.token(),
        &[
            (PROTOCOL_VERSION_HEADER, "2026-07-28, 2025-11-25"),
            (METHOD_HEADER, "tools/list"),
        ],
        &list,
    );
    assert_eq!(refused.status, 400);
    assert!(
        refused.body.contains("more than one revision"),
        "body: {}",
        refused.body
    );
    lease.close();
}

#[test]
fn acceptance_the_initialize_exemption_dispatches_nothing() {
    // The exemption's whole safety argument is that `initialize` is read-only:
    // it must not reach the handler at all. A shared-facade lease whose handler
    // counts dispatches proves the compat path grants no execution (INV-03).
    struct Counting {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl ToolCallHandler for Counting {
        fn call(&mut self, _name: &str, _args: &Value) -> Result<Value, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(serde_json::json!({}))
        }
    }
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lease = McpServer::start_http_listener(Counting {
        calls: std::sync::Arc::clone(&calls),
    })
    .expect("start lease");
    let initialize = serde_json::json!({
        "jsonrpc": "2.0", "id": 15, "method": "initialize",
        "params": {"protocolVersion": LEGACY_PROTOCOL_VERSION, "capabilities": {}}
    })
    .to_string();
    let response = post_raw(lease.local_addr(), lease.token(), &[], &initialize);
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the compatibility handshake must never dispatch a tool"
    );
    // A subsequent legacy call is still refused, so the exemption bought one
    // handshake and nothing else.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 16, "method": "tools/call",
        "params": {"name": "office.edit", "arguments": {}}
    })
    .to_string();
    let refused = post_raw(
        lease.local_addr(),
        lease.token(),
        &[(PROTOCOL_VERSION_HEADER, LEGACY_PROTOCOL_VERSION)],
        &call,
    );
    assert_eq!(refused.status, 400);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a refused call must not reach the handler either"
    );
    lease.close();
}

#[test]
fn acceptance_an_unknown_initialize_version_is_answered_not_echoed() {
    let lease = McpServer::start_http_listener(Echo).expect("start lease");
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 17, "method": "initialize",
        "params": {"protocolVersion": "9999-01-01", "capabilities": {}}
    })
    .to_string();
    let response = post_raw(lease.local_addr(), lease.token(), &[], &body);
    assert_eq!(response.status, 200, "body: {}", response.body);
    let value = response.json();
    // Answered with a revision we implement; the untrusted string is never
    // reflected back to the client.
    assert_eq!(
        value["result"]["protocolVersion"],
        SUPPORTED_PROTOCOL_VERSION
    );
    assert!(
        !response.body.contains("9999-01-01"),
        "an untrusted revision must never be echoed: {}",
        response.body
    );
    // A header naming an unimplementable revision is refused with the window
    // rather than negotiated down: we cannot answer that client truthfully.
    let refused = post_raw(
        lease.local_addr(),
        lease.token(),
        &[(PROTOCOL_VERSION_HEADER, "9999-01-01")],
        &body,
    );
    assert_eq!(refused.status, 400);
    assert!(refused.body.contains(SUPPORTED_PROTOCOL_VERSION));
    lease.close();
}
