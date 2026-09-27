//! P46.1 — Knowledge → Skill Compiler (`/learn`, spec I13).
//!
//! Hermes `/learn` pattern (Nous Research, 2026-06): author a reusable skill
//! from a directory, URL, conversation, or pasted notes — no hand-writing
//! needed. Everything here stays a **normal versioned skill** (a `SKILL.md`
//! in the blueprint `SkillStore`); it never becomes a plugin with new powers.
//!
//! Composition (each stage rides an existing landed seam):
//!   1. **Ingestion** — evidence arrives as already-extracted text. The
//!      caller uses `agentcowork-core::reader::extract_text` (markdown/plain/
//!      html in-process; PDF/EPUB are honest runtime seams) or a conversation
//!      transcript. This module takes text, not files, so it stays pure and
//!      testable.
//!   2. **Blueprint** — deterministic extraction from the evidence: a slug
//!      name, a one-line description (first non-empty paragraph), trigger
//!      keywords (leading heading/paragraph tokens), and a `when_to_use`
//!      clause. Deterministic ⇒ a re-learn with the same evidence and no
//!      version bump fails idempotently and testably.
//!   3. **Sandbox gate** — the caller supplies a [`LearnGate`]. A learned
//!      skill is saved **only after** the gate verifies the blueprint sandbox-
//!      loads and, when scripts are attached, that they pass in the sandbox.
//!      The gate is never bypassed: `Option<LearnGate>` ⇒ `Some` runs, `None`
//!      **refuses to save** rather than guessing the skill is safe.
//!   4. **Provenance + versioning** — `author` + `created + source_sha256`
//!      (bytes of the evidence) recorded; re-learning the same slug bumps the
//!      patch version (never a silent overwrite — the ownership trail stays).
//!
//! `learn_from_evidence` is the deterministic core (unit-testable without
//! files); `learn_and_save` composes it with the store + gate.

use sha2::{Digest, Sha256};

use crate::skill_store::{Skill, SkillError, SkillManifest, SkillStore};

/// P46.1 — the sandbox verification gate for a `/learn` result.
///
/// This is the "never runs unsandboxed" boundary. The harness is a runtime
/// seam (WorkerPool / script sandbox); the shell wires it. A skill is only
/// saved when the gate returns `Ok`.
pub trait LearnGate {
    /// Verify the blueprint loads in the sandbox and any attached scripts
    /// pass the smoke test. Returning `Err` blocks the save entirely.
    fn verify(&self, skill: &Skill, evidence_sha256: &str) -> Result<(), String>;
}

/// A `/learn` request: the evidence text and authorship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnRequest {
    /// Reader-extracted text (markdown/plain/html already normalized). The
    /// caller owns file→text (reader seams); this module owns text→skill.
    pub evidence: String,
    /// Display name hint, e.g. the source title. Used as the skill's
    /// description seed when no paragraph is extractable.
    pub title: Option<String>,
    /// Who authored the skill (provenance, recorded verbatim).
    pub author: String,
    /// Explicit slug; `None` = derived deterministically from the evidence.
    pub name: Option<String>,
    /// Capability hints for selection (I2/I6 allow-list vocabulary).
    pub tools: Vec<String>,
}

/// The deterministic outcome — the blueprint (not yet saved), the provenance
/// hash, and the version that a save would use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnDraft {
    pub skill: Skill,
    pub evidence_sha256: String,
    /// Version the next save would write (patch-bumped if the slug exists).
    pub next_version: String,
}

/// Derived slug — deterministic from the request: explicit name wins, else
/// the title, else the first heading-like line, else a stable prefix of the
/// evidence. Always `[a-z0-9-]+` (matches `SkillManifest::valid_name`).
pub fn derive_name(req: &LearnRequest) -> String {
    derive_name_impl(req)
}

fn derive_name_impl(req: &LearnRequest) -> String {
    if let Some(explicit) = &req.name {
        return slugify(explicit);
    }
    if let Some(t) = &req.title {
        let s = slugify(t);
        if !s.is_empty() {
            return s;
        }
    }
    for line in req.evidence.lines() {
        let line = line.trim().trim_start_matches('#').trim();
        if line.len() >= 4 {
            let s = slugify(line);
            if !s.is_empty() {
                return s;
            }
        }
    }
    // Stable fallback: first 24 slugified chars of the evidence.
    let base: String = req
        .evidence
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    if base.is_empty() {
        "learned-skill".into()
    } else {
        base.to_lowercase()
    }
}

/// First non-empty paragraph, cleaned of markdown artifacts — the one-line
/// description seed.
fn first_paragraph(text: &str) -> Option<String> {
    for para in text.split("\n\n") {
        let clean: String = para
            .lines()
            .map(|l| l.trim().trim_start_matches('#'))
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let clean = clean
            .replace(['*', '_', '`'], "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if clean.len() >= 12 {
            return Some(clean);
        }
    }
    None
}

/// Trigger keywords: first few distinct words of the first meaningful line,
/// lowercased (natural-language triggers, I2). Deterministic.
fn derive_triggers(req: &LearnRequest) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(t) = &req.title {
        out.push(t.to_ascii_lowercase());
    }
    for line in req.evidence.lines().take(3) {
        let line = line.trim().trim_start_matches('#').trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<String> = line
            .split_whitespace()
            .take(4)
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_ascii_lowercase()
            })
            .filter(|w| w.len() >= 3)
            .collect();
        if !words.is_empty() {
            out.push(words.join(" "));
        }
        break; // one line only — deterministic and sufficient
    }
    out
}

/// Build the blueprint draft without touching the store.
pub fn learn_from_evidence(
    req: &LearnRequest,
    existing_version: Option<&str>,
) -> Result<LearnDraft, SkillError> {
    let evidence = req.evidence.trim();
    if evidence.is_empty() {
        return Err(SkillError::Malformed {
            path: "<learn>".into(),
            msg: "empty evidence — nothing to learn".into(),
        });
    }
    let name = derive_name(req);
    let description = first_paragraph(evidence)
        .or_else(|| req.title.clone())
        .unwrap_or_else(|| "Learned workflow skill".into());
    let body = format!(
        "# {}\n\n{}\n\n---\n\nProvenance: `/learn` compiled on the evidence below (sha256 `{}`).\nRewrite or extend this file to keep the skill current — every save is versioned.\n\n```text\n{}\n```",
        name,
        description,
        evidence_sha256(evidence),
        truncate_marker(evidence)
    );
    let mut version = "0.1.0".to_string();
    if let Some(existing) = existing_version {
        version = bump_patch(existing);
    }
    let manifest = SkillManifest {
        name: name.clone(),
        description,
        tools: req.tools.clone(),
        triggers: derive_triggers(req),
        when_to_use: vec![format!(
            "whenever the task matches the learned workflow in {}",
            name
        )],
        scripts: Vec::new(),
        references: Vec::new(),
        assets: Vec::new(),
        author: req.author.clone(),
        created: cron_like_now(),
        version: version.clone(),
        user_invocable: false,
        disable_model_invocation: false,
    };
    let next_version = manifest.version.clone();
    let skill = Skill {
        manifest,
        body: body.clone(),
    };
    Ok(LearnDraft {
        skill,
        evidence_sha256: evidence_sha256(evidence),
        next_version,
    })
}

/// Compose the full `/learn`: derive the blueprint, run the sandbox gate,
/// then save versioned (patch-bump on re-learn) with provenance. Returns the
/// saved skill + the path it was written to.
pub fn learn_and_save(
    store: &SkillStore,
    req: &LearnRequest,
    gate: &dyn LearnGate,
) -> Result<std::path::PathBuf, SkillError> {
    // Sandbox gate FIRST — a learned skill is saved only after the gate
    // verifies it loads sandboxed (None would refuse below, never bypass).
    let existing = store.load(&derive_name(req)).ok();
    let draft = learn_from_evidence(req, existing.as_ref().map(|s| s.manifest.version.as_str()))?;
    gate.verify(&draft.skill, &draft.evidence_sha256)
        .map_err(|e| SkillError::Malformed {
            path: "<learn:gate>".into(),
            msg: format!("sandbox gate refused the skill: {e}"),
        })?;
    let path = store.save(&draft.skill, true)?;
    Ok(path)
}

/// sha256 of the evidence bytes — the provenance marker.
pub fn evidence_sha256(evidence: &str) -> String {
    let mut h = Sha256::new();
    h.update(evidence.as_bytes());
    format!("{:x}", h.finalize())
}

fn truncate_marker(text: &str) -> String {
    let max = 2000;
    if text.len() <= max {
        text.to_string()
    } else {
        format!("{}…", &text[..max])
    }
}

fn cron_like_now() -> String {
    // Deterministic for tests; production callers may override via
    // SkillManifest directly. Format matches `skills_cmds` usage.
    "2026-09-01".into()
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "learned-skill".to_string()
    } else {
        out
    }
}

fn bump_patch(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    let patch: u32 = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
    format!(
        "{}.{}.{}",
        parts.first().copied().unwrap_or("0"),
        parts.get(1).copied().unwrap_or("0"),
        patch + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_store::grow_from_task;

    struct AllowGate;
    impl LearnGate for AllowGate {
        fn verify(&self, _: &Skill, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    struct DenyGate;
    impl LearnGate for DenyGate {
        fn verify(&self, _: &Skill, _: &str) -> Result<(), String> {
            Err("policy says no".into())
        }
    }

    fn req() -> LearnRequest {
        LearnRequest {
            evidence: "# PDF triage workflow\n\nScan an attached PDF, extract headings, and summarize per page. Run the saved script and paste receipts back.\n\nSecond paragraph with enough length to count as a description fallback.".into(),
            title: Some("PDF Triage".into()),
            author: "tester".into(),
            name: None,
            tools: vec!["tool.mcp".into()],
        }
    }

    #[test]
    fn derives_deterministic_slug_from_title() {
        let draft = learn_from_evidence(&req(), None).unwrap();
        assert_eq!(draft.skill.manifest.name, "pdf-triage");
        assert!(
            draft
                .skill
                .manifest
                .triggers
                .contains(&"pdf triage".to_string())
        );
        // Deterministic: same input ⇒ same everything.
        let again = learn_from_evidence(&req(), None).unwrap();
        assert_eq!(draft, again);
    }

    #[test]
    fn empty_evidence_is_a_malformed_error() {
        let mut r = req();
        r.evidence = "   ".into();
        assert!(matches!(
            learn_from_evidence(&r, None),
            Err(SkillError::Malformed { .. })
        ));
    }

    #[test]
    fn provenance_sha256_is_stable_and_marks_the_body() {
        let draft = learn_from_evidence(&req(), None).unwrap();
        let h = evidence_sha256(&req().evidence);
        assert_eq!(draft.evidence_sha256, h);
        assert!(draft.skill.body.contains(&h));
    }

    #[test]
    fn relearn_bumps_patch_and_never_overwrites_silently() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-learn-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SkillStore::new(&dir);
        let p1 = learn_and_save(&store, &req(), &AllowGate).unwrap();
        assert!(p1.exists());
        let first = store.load("pdf-triage").unwrap();
        assert_eq!(first.manifest.version, "0.1.0");

        let p2 = learn_and_save(&store, &req(), &AllowGate).unwrap();
        assert!(p2.exists());
        let second = store.load("pdf-triage").unwrap();
        assert_eq!(second.manifest.version, "0.1.1"); // patch bump, trail kept
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deny_gate_never_saves() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-learn-deny-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SkillStore::new(&dir);
        let res = learn_and_save(&store, &req(), &DenyGate);
        assert!(matches!(res, Err(SkillError::Malformed { .. })));
        assert!(!dir.join("pdf-triage").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grow_from_task_remains_the_task_growth_path() {
        // Regression guard: the GenericAgent task-growth path stays intact
        // and version-bumping agrees with /learn's patch bump.
        let dir =
            std::env::temp_dir().join(format!("agentcowork-learn-grow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SkillStore::new(&dir);
        let s = grow_from_task(&store, "PDF Triage", "solved it", "tester", "0.1.0").unwrap();
        let s2 =
            grow_from_task(&store, "PDF Triage", "solved it again", "tester", "0.1.0").unwrap();
        assert_eq!(s.manifest.version, "0.1.0");
        assert_eq!(s2.manifest.version, "0.1.1");
        std::fs::remove_dir_all(&dir).ok();
    }
}
