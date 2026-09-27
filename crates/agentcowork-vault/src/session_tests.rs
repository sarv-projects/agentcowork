//! P2.7 Session Vault tests — the 11-item TODO contract, one test per
//! requirement (capture/persist/restore, multi-account, trust-ladder gating,
//! injection/revoke, rotation, expiry, usage audit, round-trip).

use super::*;

fn vault() -> Vault {
    Vault::open_in_memory("session-test-key").expect("open in-memory vault")
}

fn cookie(name: &str, value: &str) -> Cookie {
    Cookie {
        name: name.to_string(),
        value: value.as_bytes().to_vec(),
        domain: "example.com".to_string(),
        path: "/".to_string(),
        expires: None,
        http_only: true,
        secure: true,
        same_site: "Lax".to_string(),
    }
}

fn input() -> CaptureInput {
    CaptureInput {
        cookies: vec![cookie("session", "secret-token-123"), cookie("uid", "42")],
        storage: vec![
            StorageItem {
                kind: StorageKind::Local,
                key: "theme".into(),
                value: b"dark".to_vec(),
            },
            StorageItem {
                kind: StorageKind::IndexedDb,
                key: "db/drafts/1".into(),
                value: b"{\"body\":\"hi\"}".to_vec(),
            },
        ],
        headers: vec![AuthHeader {
            name: "X-Api-Token".into(),
            value: b"hdr-secret".to_vec(),
        }],
        persist: true,
        trust_level: TrustLevel::ReadOnly,
        ttl_secs: None,
    }
}

#[test]
fn capture_roundtrip_persists_full_context() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv.capture("example.com", "work", input()).unwrap();

    // Re-open the same DB (persist survives) — restore path.
    let rec = sv.get_session(&id).unwrap();
    assert_eq!(rec.site, "example.com");
    assert_eq!(rec.account, "work");
    assert!(rec.persist);
    assert_eq!(rec.status, "active");

    // Grant + inject returns the exact sealed context.
    sv.grant(&id, "agent-1", TrustLevel::ReadOnly).unwrap();
    let ctx = sv.inject(&id, "agent-1", TrustLevel::ReadOnly).unwrap();
    assert_eq!(ctx.cookies.len(), 2);
    assert_eq!(ctx.cookies[0].value, b"secret-token-123");
    assert_eq!(ctx.storage.len(), 2);
    assert_eq!(ctx.headers.len(), 1);
}

#[test]
fn agent_never_sees_raw_cookie_values() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv.capture("mail.example.com", "personal", input()).unwrap();

    // The agent-facing record serializes with NO value fields.
    let rec = sv.get_session(&id).unwrap();
    let json = serde_json::to_string(&rec).unwrap();
    assert!(!json.contains("secret-token-123"), "leaked cookie: {json}");
    assert!(!json.contains("hdr-secret"), "leaked header: {json}");

    // Listing is metadata-only too.
    let list = sv.list_sessions().unwrap();
    let all_json = serde_json::to_string(&list).unwrap();
    assert!(!all_json.contains("secret-token-123"));
}

#[test]
fn trust_ladder_gates_injection() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv.capture("pay.example.com", "work", input()).unwrap();

    // No grant → denied.
    assert!(matches!(
        sv.inject(&id, "agent-1", TrustLevel::ReadOnly),
        Err(SessionError::AccessDenied { .. })
    ));

    // Read-only grant → read-only inject OK, but drive-autonomous escalation
    // is denied (grant must be >= requested).
    sv.grant(&id, "agent-1", TrustLevel::ReadOnly).unwrap();
    assert!(sv.authorize(&id, "agent-1", TrustLevel::ReadOnly).unwrap());
    assert!(
        !sv.authorize(&id, "agent-1", TrustLevel::DriveAutonomous)
            .unwrap()
    );
    assert!(sv.inject(&id, "agent-1", TrustLevel::ReadOnly).is_ok());
    assert!(matches!(
        sv.inject(&id, "agent-1", TrustLevel::DriveAutonomous),
        Err(SessionError::AccessDenied { .. })
    ));

    // Upgrading the grant unlocks the higher level.
    sv.grant(&id, "agent-1", TrustLevel::DriveAutonomous)
        .unwrap();
    assert!(
        sv.inject(&id, "agent-1", TrustLevel::DriveAutonomous)
            .is_ok()
    );
}

#[test]
fn multiple_accounts_per_site_are_separate() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let a = sv.capture("example.com", "personal", input()).unwrap();
    let b = sv.capture("example.com", "work", input()).unwrap();
    let c = sv.capture("example.com", "test", input()).unwrap();

    assert_ne!(a, b);
    assert_ne!(b, c);
    let list = sv.list_sessions().unwrap();
    assert_eq!(list.len(), 3);
    // Distinct account tags on the same site.
    assert_eq!(list.iter().filter(|s| s.site == "example.com").count(), 3);
}

#[test]
fn rotation_picks_next_authorized_account() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let a = sv.capture("mail.example.com", "work", input()).unwrap();
    let b = sv.capture("mail.example.com", "personal", input()).unwrap();

    sv.grant(&a, "agent-1", TrustLevel::ReadOnly).unwrap();
    sv.grant(&b, "agent-1", TrustLevel::ReadOnly).unwrap();

    // From work → personal; from personal → work (round-robin).
    assert_eq!(
        sv.rotate_account("mail.example.com", "agent-1", &a, TrustLevel::ReadOnly)
            .unwrap(),
        Some(b.clone())
    );
    assert_eq!(
        sv.rotate_account("mail.example.com", "agent-1", &b, TrustLevel::ReadOnly)
            .unwrap(),
        Some(a.clone())
    );

    // An account the agent is NOT granted is skipped.
    let c = sv
        .capture("mail.example.com", "untrusted", input())
        .unwrap();
    sv.grant(&c, "other-agent", TrustLevel::ReadOnly).unwrap();
    assert_eq!(
        sv.rotate_account("mail.example.com", "agent-1", &a, TrustLevel::ReadOnly)
            .unwrap(),
        Some(b.clone()) // c is not granted to agent-1
    );
}

#[test]
fn expiry_tracking_marks_lapsed_sessions() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv
        .capture(
            "example.com",
            "work",
            CaptureInput {
                ttl_secs: Some(0),
                ..input()
            },
        )
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(5));
    let expired = sv.expired_sessions().unwrap();
    assert!(expired.iter().any(|s| s.id == id));

    sv.mark_expired(&id).unwrap();
    assert_eq!(sv.get_session(&id).unwrap().status, "expired");

    // Expired sessions cannot be injected, even with a grant.
    sv.grant(&id, "agent-1", TrustLevel::ReadOnly).unwrap();
    assert!(matches!(
        sv.inject(&id, "agent-1", TrustLevel::ReadOnly),
        Err(SessionError::Inactive(..))
    ));
}

#[test]
fn usage_audit_records_capture_inject_and_deny() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv.capture("example.com", "work", input()).unwrap();

    // Denied injection is audited.
    let _ = sv.inject(&id, "agent-1", TrustLevel::ReadOnly);
    // Granted injection is audited.
    sv.grant(&id, "agent-1", TrustLevel::ReadOnly).unwrap();
    sv.inject(&id, "agent-1", TrustLevel::ReadOnly).unwrap();

    let rows = sv.usage_rows("example.com").unwrap();
    let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
    assert!(actions.contains(&"capture"));
    assert!(actions.contains(&"deny"));
    assert!(actions.contains(&"inject"));
    // The deny row records the requesting agent.
    assert!(
        rows.iter()
            .any(|r| r.action == "deny" && r.agent_session == "agent-1")
    );
}

#[test]
fn revoke_locks_the_session() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv.capture("example.com", "work", input()).unwrap();
    sv.grant(&id, "agent-1", TrustLevel::DriveAutonomous)
        .unwrap();
    assert!(
        sv.inject(&id, "agent-1", TrustLevel::DriveAutonomous)
            .is_ok()
    );

    sv.revoke_agent(&id, "agent-1").unwrap();
    assert!(matches!(
        sv.inject(&id, "agent-1", TrustLevel::DriveAutonomous),
        Err(SessionError::AccessDenied { .. })
    ));
}

#[test]
fn recapture_replaces_not_duplicates() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id1 = sv.capture("example.com", "work", input()).unwrap();
    let mut new_input = input();
    new_input.cookies = vec![cookie("session", "rotated-token")];
    let id2 = sv.capture("example.com", "work", new_input).unwrap();

    assert_eq!(id1, id2, "same (site, account) must map to one session");
    assert_eq!(sv.list_sessions().unwrap().len(), 1);

    sv.grant(&id1, "agent-1", TrustLevel::ReadOnly).unwrap();
    let ctx = sv.inject(&id1, "agent-1", TrustLevel::ReadOnly).unwrap();
    assert_eq!(ctx.cookies.len(), 1);
    assert_eq!(ctx.cookies[0].value, b"rotated-token");
}

#[test]
fn delete_session_wipes_context() {
    let v = vault();
    let sv = SessionVault::new(&v);
    let id = sv.capture("example.com", "work", input()).unwrap();
    sv.delete_session(&id).unwrap();
    assert!(matches!(
        sv.get_session(&id),
        Err(SessionError::NotFound(_))
    ));
    assert!(sv.list_sessions().unwrap().is_empty());
}

#[test]
fn unknown_handle_is_not_found() {
    let v = vault();
    let sv = SessionVault::new(&v);
    assert!(matches!(
        sv.get_session("sv_nope"),
        Err(SessionError::NotFound(_))
    ));
    assert!(matches!(
        sv.inject("sv_nope", "agent-1", TrustLevel::ReadOnly),
        Err(SessionError::NotFound(_))
    ));
}
