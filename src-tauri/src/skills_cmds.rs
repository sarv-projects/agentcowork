//! P9.7 — skills-store install flow (ARCH/15 tier 3, "skills = MCP + SKILL.md").
//!
//! Closes the loop on `agentcowork-guard::skillstore` (the single Ed25519-signed
//! skills index) + `agentcowork-blueprint::SkillStore` (the on-disk `SKILL.md`
//! registry at `<skill-store home>`):
//!
//!   1. `skills_catalog()` — verify the bundled signed index against the pinned
//!      public key; return the rows for the UI (with per-skill capability
//!      demands = the Guard-2 consent surface) alongside what's already installed
//!      on disk.
//!   2. `skills_install(id)` — re-verify + structurally validate the index (a
//!      tampered index is rejected — no install happens), then write a `SKILL.md`
//!      into the blueprint `SkillStore`. The caller (UI) renders the consent
//!      card from `permissions` before invoking; the shell gates the write.
//!   3. `skills_uninstall(id)` — remove the skill directory, confined to the
//!      store root and audited on success and on refusal.
//!
//! The app ships only the verifying public key + signed index (never the store
//! operator's signing key) — the asymmetry from `skillstore`'s trust model.

use serde::Serialize;
use tauri::State;

use crate::AppState;

/// Pinned verifying public key for the bundled skills store index (the store
/// operator's public half; the private signing key never ships in the app).
/// Regenerated with `cargo run -p agentcowork-guard --example gen_skillstore_seed`.
pub const STORE_PUBLIC_KEY_B64: &str = "lvI3luTatntgPJAIeBRIFHJsYv3CQRUCMZg97OYZrT0=";

/// The bundled signed index body (canonical JSON array of `SkillRow`).
pub const STORE_INDEX_BODY: &str = r#"[{"id":"docx-assistant","name":"DOCX Assistant","version":"1.2.0","description":"Draft and format .docx documents from plain instructions.","permissions":["fs.write","tool.mcp"],"manifest":"docx-assistant"},{"id":"note-taker","name":"Note Taker","version":"0.9.0","description":"Read your notes and surface the ones relevant to the current task.","permissions":["fs.read"],"manifest":"note-taker"},{"id":"doc-scanner","name":"Document Scanner","version":"0.4.1","description":"Scan a folder for documents and summarize contents.","permissions":["fs.read","tool.mcp"],"manifest":"doc-scanner"},{"id":"email-drafter","name":"Email Drafter","version":"1.0.0","description":"Draft replies in your tone from an inbox thread.","permissions":["fs.read","tool.connector"],"manifest":"email-drafter"}]"#;

/// The matching Ed25519 signature (base64) over `STORE_INDEX_BODY`.
pub const STORE_INDEX_SIGNATURE_B64: &str =
    "9qWu8UH8qeraWOutHfGpbYfU1LaD3x5F4mMeIIqOCm82Gi+vYW5+HhUo0QQYgeTcC0aaUJaSKVpzEXzGoX0dAA==";

/// The capability floor a skill may not exceed (mirrors
/// `agentcowork_guard::skillstore::RUNTIME_CAPABILITY_ALLOWLIST`).
pub const RUNTIME_CAPABILITY_ALLOWLIST: &[&str] =
    &["fs.read", "fs.write", "tool.mcp", "tool.connector"];

/// Resolve the skill-store root. Prefer `AGENTCOWORK_SKILLS_DIR` (retired
/// `EVERYAIOS_SKILLS_DIR` honored); fall back to the blueprint default home.
fn skills_root() -> std::path::PathBuf {
    agentcowork_types::env_compat::get_os("SKILLS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(agentcowork_blueprint::SkillStore::default_home)
}

/// One skill row rendered to the UI (with install state).
#[derive(Debug, Clone, Serialize)]
pub struct SkillRowView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub scopes_plain: Vec<String>,
    pub installed: bool,
    /// `Some(true)` = installed skill's on-disk bytes differ from its
    /// install-time sha-256 pin (mutated / upgraded out-of-band). `None` =
    /// not installed or no pin (user-authored skill).
    pub tampered: Option<bool>,
    /// P51.28 — slash/picker (Crush user-invocable or Zed disable-model).
    pub user_invocable: bool,
    /// P51.28 — model must not auto-select.
    pub disable_model_invocation: bool,
}

/// Verify the bundled signed index against the pinned key and return the rows.
fn verify_bundled() -> Result<Vec<agentcowork_guard::skillstore::SkillRow>, String> {
    let signed = agentcowork_guard::skillstore::SignedSkillIndex {
        body: STORE_INDEX_BODY.to_string(),
        signature_b64: STORE_INDEX_SIGNATURE_B64.to_string(),
    };
    agentcowork_guard::skillstore::verify_and_validate(
        &signed,
        STORE_PUBLIC_KEY_B64,
        RUNTIME_CAPABILITY_ALLOWLIST,
    )
    .map_err(|e| e.to_string())
}

/// P46.1 — the `/learn` sandbox gate as a structural allow-list check.
///
/// A learned skill may carry **no new powers** (spec I13: "output = a normal
/// versioned skill, not a plugin with new powers"): every requested tool must
/// be in the runtime capability allow-list, and `/learn` never attaches
/// scripts (the blueprint is instruction+references; execution stays in the
/// existing sandboxed tool paths). This check runs BEFORE any save — the
/// `agentcowork-blueprint::learn::LearnGate` boundary refuses the write.
struct LearnStructuralGate;

impl agentcowork_blueprint::LearnGate for LearnStructuralGate {
    fn verify(
        &self,
        skill: &agentcowork_blueprint::Skill,
        _evidence_sha256: &str,
    ) -> Result<(), String> {
        // No new powers: tools ⊆ runtime allow-list.
        for t in &skill.manifest.tools {
            if !RUNTIME_CAPABILITY_ALLOWLIST.contains(&t.as_str()) {
                return Err(format!(
                    "skill requests capability `{t}` outside the runtime allow-list — /learn cannot grant new powers"
                ));
            }
        }
        // No scripts: a learned skill is instructions+references only.
        if !skill.manifest.scripts.is_empty() {
            return Err(
                "/learn never attaches scripts — the blueprint is instructions + references".into(),
            );
        }
        Ok(())
    }
}

/// P46.1 — `/learn`: compile evidence (already reader-extracted text: a
/// URL/PDF/repo/conversation/folder the user points at) into a versioned
/// `SKILL.md` in the local skill registry. Sandbox-gated (structural
/// allow-list — no new powers), provenance-sha256-marked, patch-versioned on
/// re-learn. The UI renders the resulting skill in the registry.
#[tauri::command]
pub fn skills_learn(
    evidence: String,
    title: Option<String>,
    name: Option<String>,
    author: Option<String>,
    tools: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    if evidence.trim().is_empty() {
        return Err("nothing to learn — evidence is empty".into());
    }
    let store = agentcowork_blueprint::SkillStore::new(skills_root());
    let req = agentcowork_blueprint::LearnRequest {
        evidence,
        title,
        author: author.unwrap_or_else(|| "agentcowork-user".into()),
        name,
        tools: tools.unwrap_or_default(),
    };
    let gate = LearnStructuralGate;
    let path =
        agentcowork_blueprint::learn_and_save(&store, &req, &gate).map_err(|e| e.to_string())?;
    // Reload the saved skill (learn_and_save wrote it under its derived name).
    let id = agentcowork_blueprint::derive_name(&req);
    let skill = store.load(&id).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "id": id,
        "name": id,
        "version": skill.manifest.version,
        "description": skill.manifest.description,
        "path": path.display().to_string(),
        "author": skill.manifest.author,
        "learned": true,
    }))
}

/// Read the set of installed skill names from the on-disk registry (skipping
/// malformed entries — same policy as `SkillStore::scan`).
fn installed_names() -> std::collections::BTreeSet<String> {
    let store = agentcowork_blueprint::SkillStore::new(skills_root());
    match store.scan() {
        Ok(skills) => skills.into_iter().map(|s| s.manifest.name).collect(),
        Err(_) => std::collections::BTreeSet::new(),
    }
}

/// doc-75 sha-pin integrity check: has the installed skill's on-disk bytes
/// drifted from its install-time pin? `None` = not present on disk / no pin.
fn skill_tampered(store: &agentcowork_blueprint::SkillStore, id: &str) -> Option<bool> {
    let path = store.root().join(id).join("SKILL.md");
    let bytes = std::fs::read(&path).ok()?;
    store.is_tampered(id, &bytes)
}

/// P9.7 — the skills-store listing: verified index rows + install state.
#[tauri::command]
pub fn skills_catalog(
    #[allow(unused)] state: State<'_, AppState>,
) -> Result<Vec<SkillRowView>, String> {
    let rows = verify_bundled()?;
    let installed = installed_names();
    let store = agentcowork_blueprint::SkillStore::new(skills_root());
    Ok(rows
        .into_iter()
        .map(|r| {
            let on_disk = store.load(&r.id).ok();
            SkillRowView {
                id: r.id.clone(),
                name: r.name.clone(),
                version: r.version.clone(),
                description: r.description.clone(),
                permissions: r.permissions.clone(),
                scopes_plain: r
                    .permissions
                    .iter()
                    .map(|p| plain_language_scope(p).to_string())
                    .collect(),
                installed: installed.contains(&r.id),
                tampered: skill_tampered(&store, &r.id),
                user_invocable: on_disk
                    .as_ref()
                    .map(|s| agentcowork_blueprint::may_user_slash_invoke(&s.manifest))
                    .unwrap_or(false),
                disable_model_invocation: on_disk
                    .as_ref()
                    .map(|s| s.manifest.disable_model_invocation)
                    .unwrap_or(false),
            }
        })
        .collect())
}

/// P9.7 — install a listed skill. The signed index is re-verified and
/// structurally validated here (a tampered index installs nothing); then a
/// `SKILL.md` is written into the blueprint `SkillStore`. The UI renders the
/// Guard-2 consent card from `permissions` *before* calling this.
#[tauri::command]
pub fn skills_install(id: String) -> Result<serde_json::Value, String> {
    let rows = verify_bundled()?;
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("skill `{id}` is not in the verified store"))?;

    // Build a blueprint Skill (manifest + body) from the verified row. The
    // registry keys skills by their slug `id` (SkillStore::save validates
    // `[a-z0-9-]+`), so the manifest name is the row id, not the display name.
    let skill = agentcowork_blueprint::Skill {
        manifest: agentcowork_blueprint::SkillManifest {
            name: row.id.clone(),
            description: row.description.clone(),
            tools: row.permissions.clone(),
            triggers: vec![row.name.clone()],
            when_to_use: Vec::new(),
            scripts: Vec::new(),
            references: Vec::new(),
            assets: Vec::new(),
            author: "agentcowork-store".into(),
            created: chrono_like_now(),
            version: row.version.clone(),
            user_invocable: false,
            disable_model_invocation: false,
        },
        body: format!(
            "# {}\n\n{}\n\nStore-sourced skill (verified against the pinned store key). Capabilities: {}.",
            row.name,
            row.description,
            row.permissions.join(", ")
        ),
    };

    let store = agentcowork_blueprint::SkillStore::new(skills_root());
    let path = store.save(&skill, true).map_err(|e| e.to_string())?;
    // doc-75 sha-pinned marketplace model: pin the skill to the exact bytes we
    // wrote, so any later on-disk mutation is detected (never silently trusted).
    store.pin(
        row.id.as_str(),
        "agentcowork-store",
        row.version.as_str(),
        skill.to_skill_md().as_bytes(),
    );
    Ok(serde_json::json!({
        "id": row.id,
        "name": row.name,
        "installed": true,
        "path": path.display().to_string(),
    }))
}

/// P9.7 — uninstall a skill (removes its directory from the on-disk registry).
///
/// `name` arrives from the renderer, so it is untrusted input. It is *not* a
/// path: [`SkillStore::delete`] accepts a registered package id only, measures
/// it against the canonical store root (pathfloor), and removes the tree with
/// the bounded, link-aware walk. A `..` walk, an absolute path, a symlinked
/// package dir, an unregistered directory, or a link inside the package that
/// leaves the root is a typed refusal that deletes nothing. The pin is dropped
/// by the same call (uninstall, not a data wipe — the Library and its receipts
/// are preserved; `REQ-SKILL-010`).
#[tauri::command]
pub fn skills_uninstall(
    #[allow(unused)] state: State<'_, AppState>,
    name: String,
) -> Result<serde_json::Value, String> {
    let store = agentcowork_blueprint::SkillStore::new(skills_root());
    // Every delete outcome is a first-class audit entry (`ARCH/12-TRUST.md`
    // §9, INV-24: denials and delete are logged, never swallowed).
    match store.delete(&name) {
        Ok(()) => {
            let seq = crate::control::record_mutation(
                &state,
                crate::control::AuthKind::HumanGesture,
                "skills.uninstall",
                serde_json::json!({ "id": name, "result": "removed" }),
            );
            Ok(serde_json::json!({ "id": name, "installed": false, "auditSeq": seq }))
        }
        Err(e) => {
            crate::control::record_mutation(
                &state,
                crate::control::AuthKind::HumanGesture,
                "skills.uninstall_refused",
                serde_json::json!({
                    "id": name,
                    "result": "refused",
                    "reason": e.to_string(),
                }),
            );
            Err(e.to_string())
        }
    }
}

/// Human-readable consent summary for the Guard-2 card (plain-language scopes).
pub fn plain_language_scope(permission: &str) -> &'static str {
    match permission {
        "fs.read" => "Read your files",
        "fs.write" => "Write to your files (each write is approved)",
        "tool.mcp" => "Call local + remote MCP tools",
        "tool.connector" => "Call your connected connectors (Gmail, Drive, Notion…)",
        _ => "Request an OS capability",
    }
}

fn chrono_like_now() -> String {
    // A dependency-light timestamp (the shell already carries `time`, but a
    // stable YYYY-MM-DD is enough for the ownership marker).
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days→civil date (Howard Hinnant's algorithm). Not `time`-crate-dependent.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded seed must verify against the pinned key — if the operator
    /// regenerates the index (gen_skillstore_seed) without updating the key,
    /// this fails loudly instead of shipping a broken store.
    #[test]
    fn bundled_seed_verifies() {
        let rows = verify_bundled().expect("bundled seed verifies vs pinned key");
        assert_eq!(rows.len(), 4);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"docx-assistant"));
        assert!(ids.contains(&"email-drafter"));
        for r in &rows {
            for p in &r.permissions {
                assert!(plain_language_scope(p) != "Request an OS capability");
            }
        }
    }

    /// FIX-05 — the uninstall path is the one place a renderer-supplied string
    /// reaches a recursive delete, so the shell asserts the refusals against
    /// the same store construction `skills_uninstall` uses (`skills_root()` +
    /// `SkillStore::new`). The command itself is a thin audited wrapper around
    /// [`agentcowork_blueprint::SkillStore::delete`].
    #[test]
    fn uninstall_refuses_paths_and_only_removes_a_registered_skill() {
        let base =
            std::env::temp_dir().join(format!("agentcowork-skills-cmds-{}", std::process::id()));
        let root = base.join("skills");
        let outside = base.join("outside");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), "keep").unwrap();

        let skill = agentcowork_blueprint::Skill {
            manifest: agentcowork_blueprint::SkillManifest {
                name: "note-taker".into(),
                description: "A skill that lives in the store".into(),
                author: "tester".into(),
                created: "2026-09-26".into(),
                version: "0.1.0".into(),
                ..Default::default()
            },
            body: "body".into(),
        };
        let store = agentcowork_blueprint::SkillStore::new(&root);
        store.save(&skill, false).unwrap();

        // Traversal and absolute paths are refused, and the decoy survives.
        for id in [
            "..".to_string(),
            "../outside".to_string(),
            outside.display().to_string(),
        ] {
            let err = store.delete(&id).expect_err("must be refused");
            assert!(
                err.to_string().contains("refused") || err.to_string().contains("invalid"),
                "`{id}` → {err}"
            );
        }
        assert!(outside.join("precious.txt").exists());
        assert!(root.join("note-taker/SKILL.md").exists());

        // The registered id still uninstalls.
        store.delete("note-taker").unwrap();
        assert!(!root.join("note-taker").exists());
        assert!(outside.join("precious.txt").exists());
    }

    #[test]
    fn tampered_seed_rejected() {
        // A signature that no longer matches the body must reject.
        let bad = agentcowork_guard::skillstore::SignedSkillIndex {
            body: r#"[{"id":"evi","name":"Evi","version":"1.0.0","description":"x","permissions":["shell.exec"],"manifest":""}]"#
                .to_string(),
            signature_b64: STORE_INDEX_SIGNATURE_B64.to_string(),
        };
        assert!(
            agentcowork_guard::skillstore::verify_skill_index(&bad, STORE_PUBLIC_KEY_B64).is_err()
        );
    }
}
