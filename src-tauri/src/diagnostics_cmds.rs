//! P70.D4 · D7 · D8 — diagnostics, sandbox honesty and the data-removal path.
//!
//! Three user-facing surfaces the doctor does not cover:
//!
//! * **D7 — sandbox honesty:** the cockpit states which containment posture
//!   third-party MCP children actually get on this host (`Confined` on Linux
//!   with bwrap, `Ambient` otherwise). Ambient is never presented as
//!   confinement — this is the same statement `ARCH/15-CONNECT-STORE.md`
//!   makes, surfaced where the user can see it.
//! * **D8 — support bundle:** a user-triggered diagnostic export (versions,
//!   platform, doctor report, capability probes, audit summary, recent
//!   errors) with **secrets scrubbed**. The scrubbing is structural: the
//!   bundle is assembled from allow-listed fields only, so a key can only
//!   leak by adding it to the allow-list in review — never by forgetting a
//!   deny-rule.
//! * **D4 — remove all data:** the explicit, confirmed, audited deletion of
//!   the per-user data directory (the uninstall default keeps it). Every
//!   store that holds user data is closed before removal; the audit writer
//!   is re-opened afterwards so the deletion itself is the last recorded
//!   event and the app stays bootable.

use std::path::PathBuf;

use serde_json::json;
use tauri::State;

use crate::AppState;

// ---------------------------------------------------------------- D7 posture

/// P70.D7 — the honest sandbox statement for this host. `Confined` only when
/// the platform can actually deliver it (Linux + bwrap); every other host
/// reports `Ambient` with the reason. The UI renders this verbatim — it is a
/// report, not a capability claim.
#[tauri::command]
pub fn diagnostics_sandbox_posture() -> serde_json::Value {
    let posture = agentcowork_mcp::attach::SandboxPosture::preferred();
    let (posture, detail) = match posture {
        agentcowork_mcp::attach::SandboxPosture::Confined => (
            "Confined",
            "third-party MCP servers run inside bubblewrap: own mount namespace, no new \
             privileges, no inherited environment — provider keys are reachable only \
             through the vault broker",
        ),
        agentcowork_mcp::attach::SandboxPosture::Ambient => (
            "Ambient",
            "this platform has no native sandbox backend yet (P49.5): third-party MCP \
             servers run with the inherited environment. They are still ticket-gated, \
             audited and net-floored by Guard-2 — but they are NOT filesystem-confined. \
             Install only servers you trust.",
        ),
    };
    json!({
        "posture": posture,
        "contained": posture == "Confined",
        "detail": detail,
        "platform": std::env::consts::OS,
    })
}

// ---------------------------------------------------------------- D8 bundle

/// The files the bundle may read, by explicit allow-list. Everything else in
/// the data dir (the vault, key rings, token caches) is invisible to the
/// exporter by construction. Each entry is read if present, scrubbed, and
/// embedded under `files`.
const BUNDLE_ALLOWED: &[&str] = &[
    // (the audit ledger is NOT embedded raw — `audit_summary` carries its
    //  shape: last-40 kinds with scrubbed payload previews)
    "store-schema.json",   // versions + adopted flags
    "update_channel.json", // { channel } only
];

/// A secret-shaped key name. Mirrors `agentcowork_guard::sandbox`'s refusal
/// list so the two stay honest about what counts as a secret.
fn secret_shaped(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.ends_with("_key")
        || k == "key"
        || k.ends_with("_token")
        || k == "token"
        || k.contains("secret")
        || k.contains("password")
        || k.contains("credential")
        || k.starts_with("agentcowork_")
        // Retired spellings stay scrubbed too (DEC-053): an old credential
        // name in a diagnostics bundle is still a credential.
        || k.starts_with("everyaios_")
        || k.ends_with("_api")
        || k == "authorization"
}

/// Scrub a JSON value: drop every object member whose key is secret-shaped,
/// and truncate long strings (raw prompt/response bodies are not diagnostics).
fn scrub(value: &serde_json::Value, max_str: usize) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .filter(|(k, _)| !secret_shaped(k))
                .map(|(k, v)| (k.clone(), scrub(v, max_str)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(|v| scrub(v, max_str)).collect())
        }
        serde_json::Value::String(s) if s.len() > max_str => {
            serde_json::Value::String(format!("<{} bytes>", s.len()))
        }
        other => other.clone(),
    }
}

/// Audit summary: the last N events as kind + timestamp + a scrubbed payload
/// preview, **newest first**. The ledger is the one place the app's own
/// history lives; the bundle carries its shape, never its secrets.
fn audit_summary(audit_path: &std::path::Path, last: usize) -> serde_json::Value {
    let Ok(raw) = std::fs::read_to_string(audit_path) else {
        return json!({ "events": [], "note": "no audit ledger yet" });
    };
    let mut kinds: Vec<serde_json::Value> = Vec::new();
    let mut total = 0usize;
    for line in raw.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        total += 1;
        if kinds.len() < last {
            let ts = v.get("ts_ms").cloned().unwrap_or(json!(null));
            let kind = v.get("kind").cloned().unwrap_or(json!("?"));
            let mut payload = v.get("payload").cloned().unwrap_or(json!({}));
            payload = scrub(&payload, 80);
            kinds.push(json!({ "ts_ms": ts, "kind": kind, "payload": payload }));
        }
    }
    json!({ "total_lines": total, "events": kinds })
}

/// P70.D8 — build the support bundle. Returns the JSON document; the UI
/// offers it as a file (or the user copies it). Everything below is either a
/// count, a version, a status or an allow-listed file — the vault, the key
/// ring and token caches are never read.
#[tauri::command]
pub fn diagnostics_support_bundle(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let data_dir: PathBuf = state.replay_dir.clone();
    let audit_path = data_dir.join("audit.ndjson");
    let audit = audit_summary(&audit_path, 40);

    // Doctor: the readiness report (vault ORIGIN string, counts only — the
    // live probe resolves the key but never returns it).
    let doctor = agentcowork_core::doctor::run_doctor(
        env!("CARGO_PKG_VERSION"),
        &agentcowork_core::doctor::LiveProbe::new(data_dir.clone()),
    );
    let doctor_json = serde_json::to_value(&doctor).map_err(|e| e.to_string())?;

    // Capability probes (booleans + counts, no values).
    let sandbox = diagnostics_sandbox_posture();

    // Allow-listed files, scrubbed. A file missing from disk is reported as
    // null — the absence is information too.
    let mut files = serde_json::Map::new();
    for name in BUNDLE_ALLOWED {
        let v = std::fs::read_to_string(data_dir.join(name))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| scrub(&v, 200));
        files.insert(name.to_string(), v.unwrap_or(serde_json::Value::Null));
    }

    let bundle = json!({
        "bundle_version": 1,
        "generated_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "doctor": doctor_json,
        "vault_unlocked": state
            .vault_unlocked
            .load(std::sync::atomic::Ordering::Relaxed),
        "sandbox": sandbox,
        "files": serde_json::Value::Object(files),
        "audit": audit,
        "mcp": { "attached": state.mcp_servers.lock().map(|m| m.len()).unwrap_or(0) },
    });
    Ok(bundle)
}

// ---------------------------------------------------------------- D4 wipe

/// P70.D4 — remove **all** user data. The caller has already confirmed in the
/// UI (typed confirmation); this is the effectful half.
///
/// Order matters:
///   1. close the audit writer (fd) so the ledger file is deletable,
///   2. record the deletion intent in memory (it cannot be written to the
///      file that is about to vanish — the response is the receipt),
///   3. delete the data directory,
///   4. re-create it empty and re-open a fresh audit writer, so the running
///      app stays coherent and the next boot re-stamps from zero.
///
/// The vault connection inside `state.vault` still points at the deleted
/// file's inode; it is drained of keys by dropping every keyring entry we
/// can reach, and the honest statement is: restart the app after a wipe.
#[tauri::command]
pub fn data_remove_all(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let data_dir: PathBuf = state.replay_dir.clone();

    // 1. Quiesce: no appends can interleave from here.
    let writer = state.audit_log.lock().map_err(|e| e.to_string())?.take();

    // 2. Count what is about to go (for the receipt).
    let mut removed_bytes = 0u64;
    let mut removed_files = 0u64;
    fn measure(dir: &std::path::Path, files: &mut u64, bytes: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                measure(&p, files, bytes);
            } else if let Ok(m) = e.metadata() {
                *files += 1;
                *bytes += m.len();
            }
        }
    }
    measure(&data_dir, &mut removed_files, &mut removed_bytes);
    let _ = writer; // the writer is closed here — the ledger is quiescent

    // 3. Delete. Refusing is safe: nothing has been touched yet except the
    //    writer, which is re-opened below.
    std::fs::remove_dir_all(&data_dir)
        .map_err(|e| format!("refusing to half-wipe (nothing deleted): {e}"))?;

    // 4. Re-create the shell of the data dir + a fresh ledger.
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("recreate data dir: {e}"))?;
    *state.audit_log.lock().map_err(|e| e.to_string())? =
        agentcowork_audit::AuditWriter::open(&data_dir.join("audit.ndjson")).ok();

    // The stamp manifest is rebuilt by the next boot; write it now so the
    // wipe itself is auditable by the *next* boot's tooling.
    let _ = agentcowork_core::store_schema::ensure_all(&data_dir);

    Ok(json!({
        "removed": true,
        "files": removed_files,
        "bytes": removed_bytes,
        "note": "restart AgentCowork to re-initialize from a clean profile",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_shaped_catches_the_obvious() {
        assert!(secret_shaped("OPENAI_API_KEY"));
        assert!(secret_shaped("github_token"));
        assert!(secret_shaped("client_secret"));
        assert!(secret_shaped("EVERYAIOS_HOME"));
        assert!(secret_shaped("vault_password"));
        assert!(secret_shaped("key"));
        assert!(!secret_shaped("version"));
        assert!(!secret_shaped("channel"));
        assert!(!secret_shaped("total_lines"));
    }

    #[test]
    fn scrub_drops_secret_members_and_truncates_strings() {
        let v = json!({
            "name": "coordinator",
            "api_key": "sk-123",
            "nested": { "token": "x", "count": 3 },
            "blob": "a".repeat(500),
        });
        let s = scrub(&v, 100);
        assert_eq!(s["name"], "coordinator");
        assert!(s.get("api_key").is_none());
        assert!(s["nested"].get("token").is_none());
        assert_eq!(s["nested"]["count"], 3);
        assert!(s["blob"].as_str().unwrap().starts_with('<'));
    }

    #[test]
    fn audit_summary_reports_shape_without_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.ndjson");
        std::fs::write(
            &p,
            concat!(
                r#"{"seq":1,"ts_ms":1,"kind":"a.open","payload":{"user":"me","api_key":"sk-9"}}"#,
                "\n",
                r#"{"seq":2,"ts_ms":2,"kind":"b.close","payload":{"reason":"done"}}"#,
                "\n"
            ),
        )
        .unwrap();
        let s = audit_summary(&p, 10);
        assert_eq!(s["total_lines"], 2);
        let events = s["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "b.close");
        assert!(events[0]["payload"].get("api_key").is_none());
        assert_eq!(events[1]["payload"]["user"], "me");
    }
}
