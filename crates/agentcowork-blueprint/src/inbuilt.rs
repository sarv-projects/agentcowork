//! P23-2 / P24-1 / P25-3 / P26-3 — the **inbuilt first-party skill packs**
//! (doc 75 §4 / doc 76 / doc 77 / doc 78).
//!
//! `SKILL.md` wrappers over our native engines plus the bundled general set
//! (document-creation, skill-creator, ui-ux-pro-max design-intelligence,
//! design-system, engineering exit-criteria, graphify, jobs). These live in
//! `<data_dir>/skills` (read-only, no install step) and follow the I2
//! `SKILL.md` anatomy exactly (frontmatter + instruction body).
//!
//! [`seed_inbuilt`] writes them into a [`crate::skill_store::SkillStore`]
//! root idempotently (never overwrites a user skill of the same name — the
//! user's copy wins). [`INBUILT_SKILL_NAMES`] is the always-true census.
//!
//! **License discipline:** the exit-criteria set, the design systems, the
//! doc-skills, and the graphwork/jobs verticals are *pattern* adoptions —
//! we write our own instruction bodies over our own engines, never copied
//! text (docs 71–78).

use crate::skill_store::{Skill, SkillManifest, SkillScript};
use std::path::Path;

/// The authoritative inbuilt skill set.
pub const INBUILT_SKILL_NAMES: &[&str] = &[
    "office-documents",
    "browser-automation",
    "storage-intelligence",
    "code-intelligence",
    "document-creation",
    "skill-creator",
    "ui-ux-pro-max",
    "design-system",
    "engineering-exit-criteria",
    "graphwork",
    "jobs",
];

/// Seed the inbuilt skill packs into a `SkillStore` root. Idempotent:
/// existing files are never overwritten (user copies win).
pub fn seed_inbuilt(root: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut written = Vec::new();
    for (name, skill) in all_inbuilt() {
        let dir = root.join(&name);
        if dir.join("SKILL.md").exists() {
            continue;
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("SKILL.md"), skill.to_skill_md()).map_err(|e| e.to_string())?;
        written.push(name);
    }
    Ok(written)
}

/// All inbuilt skills as `(name, Skill)` — the manifests are bundled here,
/// and the bodies are our own instruction text over our own engines (no
/// copied text from the reference repos).
pub fn all_inbuilt() -> Vec<(String, Skill)> {
    vec![
        make(
            "office-documents",
            "Drive the Office engines (docx/xlsx/pptx/pdf) via the surgical patcher: open → edit → byte-preserving save.",
            &[
                "office",
                "docx",
                "xlsx",
                "pptx",
                "pdf",
                "word",
                "excel",
                "powerpoint",
            ],
            &["edit a document", "spreadsheet", "slide deck", "pdf form"],
            "## Goal\nMake surgical, byte-preserving edits to Office files on the local machine — never a lossy re-serialize.\n\n## Rules\n- Open via the office bridge: docx_open / xlsx_open / pptx_open / pdf_open. Text render first, then patch.\n- Edits go through the block-patch engine (addresses like p1, t1:r1c2, t1:r1c2:p1). Minimal prefix/suffix rewrite.\n- Formulas are never invented: write the formula, recalc with IronCalc, take the engine-computed value.\n- Save via the atomic writer (temp → fsync → rename).\n- Capture a pre-edit snapshot before any destructive edit — one-click undo stays available.\n\n## Exit criteria\n- The patched file opens clean in LibreOffice (conformance oracle ran in CI).\n- Untouched parts byte-identical (zip-level diff shows only the intended part changed).",
        ),
        make(
            "browser-automation",
            "Drive the browser via the a11y snapshot + CDP stack: snapshot → act → diff, ownership + Guard-2 gated.",
            &["browser", "web", "snapshot", "click", "automation"],
            &[
                "open a webpage",
                "click something",
                "fill a form",
                "scrape a site",
            ],
            "## Goal\nDo browser work through the accessibility snapshot + CDP layers — token-lean, ownership-checked, Guard-2-gated.\n\n## Rules\n- Take an accessibility snapshot first; act on [ref=eN] anchors. Prefer interactive mode for token economy.\n- Never teleport with click_at: resolve the ref to geometry, use humanized paths when enabled.\n- Dialogs: accept/dismiss via the page-dialog primitive; never fudge a bypass.\n- If a page hasn't rendered yet, wait before re-snapping — never guess the DOM.\n- State-changing acts are Guard-2 tickets; a denial is a signal to re-read the policy.\n\n## Exit criteria\n- Every act is followed by a verified snapshot diff (not the model's claim).\n- Halt instead of guessing when the post-condition can't be verified.",
        ),
        make(
            "storage-intelligence",
            "Filesystem intelligence: walk, snapshot, treemap, dedupe find, FTS5 filename search, journal change drifts.",
            &["storage", "files", "disk", "find", "duplicate", "treemap"],
            &["find a file", "duplicates", "disk usage", "big files"],
            "## Goal\nAnswer file/disk questions fast with the storage engine — not by brute-force scans in scripts.\n\n## Rules\n- Use the storage bridge (health/treemap/dedup/finder/search); it's a parallel work-stealing walker + FTS5.\n- Duplicates are 7-stage verified (size → xxHash → BLAKE3 → hardlink) — never claim from size alone.\n- Cleanup is proposal-only: create a Guard-2 decision card. The crate never deletes.\n- Snapshot (zstd) before any batch op; restore is one call.\n\n## Exit criteria\n- Exact units (MiB/GiB) where possible; destructive proposals are cards, not silent actions.",
        ),
        make(
            "code-intelligence",
            "Code intelligence: LSP diagnostics, symbol jumps, repo-map/PageRank, graph queries, docs lookup.",
            &["code", "lsp", "diagnostics", "symbol", "refactor"],
            &[
                "find symbol",
                "fix diagnostics",
                "understand a codebase",
                "refactor",
            ],
            "## Goal\nAnswer code questions with the code-intel engines (LSP runner, SCIP, repo map, graph) — not grepping the whole tree.\n\n## Rules\n- Use an LSP connection (didOpen → diagnostics) for type-level questions; SCIP for symbol graphs.\n- Query the persistent graph before falling back to tree-sitter maps.\n- Keep diffs minimal: the ponytail doctrine — the best code is the code you never wrote.\n\n## Exit criteria\n- After a change: diagnostics clean and the diff is the fewest lines that satisfy the ticket.",
        ),
        make(
            "document-creation",
            "Turn a brief into a polished artifact: Word via block-patch, deck via the author path, PDF via re-author.",
            &["document", "write", "report", "deck", "brief"],
            &[
                "make me a document",
                "write a report",
                "make a deck",
                "format this",
            ],
            "## Goal\nProduce polished, byte-preserving documents from a brief or notes.\n\n## Rules\n- Reason first, native shapes second: new decks use the author path (title + bullets + transitions), existing files get surgical patches.\n- Cite precisely: exact figures from this run's receipt, never invented.\n- Surface the artifact after it exists (Office view, PDF render, or artifact card).\n\n## Exit criteria\n- The document opens clean (LibreOffice oracle) and reads like a human wrote it.\n- 0 hallucinated numbers.",
        ),
        make(
            "skill-creator",
            "Author new SKILL.md packs in the I2 anatomy: name/description/tools/triggers + lazy scripts + exit criteria.",
            &["skill", "create", "SKILL.md"],
            &["create a skill", "new skill", "teach the agent"],
            "## Goal\nAuthor reusable SKILL.md packs in the native anatomy.\n\n## Rules\n- Frontmatter: name (kebab), description, optional tools/triggers/when_to_use/scripts/references.\n- Body: when → what → exit criteria (evidence).\n- Scripts are lazy — run on demand, never at load.\n- Store at <data_dir>/skills/<name>/SKILL.md; every solved task becomes a versioned skill.",
        ),
        make(
            "ui-ux-pro-max",
            "Design-intelligence knowledge pack: the style database as a runnable skill — applied to agent-built interfaces.",
            &["design", "ui", "ux", "palette", "typography", "layout"],
            &["make this look good", "design", "ui pattern"],
            "## Referenced doctrine\n- Consistency beats novelty: prefer existing UI tokens over new ones.\n- Fitts's law: target ≥ 24×24, spacing ≥ 4px grid.\n- Contrast first, color second: WCAG 4.5:1 text / 3:1 graphic.\n- One CTA per view.\n- Feedback: every control responds (hover/active/focus-visible).\n- Empty states carry the next action, not a broken icon.\n\nProduce the design first, then cite which rule you applied.",
        ),
        make(
            "design-system",
            "Repo-level DESIGN.md brand system: tokens as a skill the agent loads before any UI work.",
            &["brand", "design-system", "tokens", "DESIGN.md"],
            &["brand colors", "design system", "styleguide"],
            "Generate and use a DESIGN.md at the repo/app root — the single source for brand decisions.\n\n## Tokens\n- color / typography / spacing / radius\n- Components (name + states)\n- Do / Don't\n\n## Rules\n- Read DESIGN.md before any UI edit; if missing, offer to create it (from existing UI) before changing pixels.\n- Composable design-skills: small reusable fragments (palette, typography, layout) instead of a monolith.",
        ),
        make(
            "engineering-exit-criteria",
            "Production engineering checklists with exit criteria (spec → plan → build → test → ship) — verified, not 'done on vibes'.",
            &["engineering", "quality", "exit criteria", "review", "ship"],
            &["production ready", "do it properly", "no shortcuts"],
            "## The six checklists\n- spec: SMART goal; acceptance tests written before code.\n- plan: design is the fewest moving parts that satisfy the spec.\n- build: minimal change; no dead code; tests accompany code.\n- test: every exit criterion has a passing test; edge cases named.\n- ship: real runtime evidence (build output, test run, usage data).\n\nEach phase ends with an explicit checklist a reader can verify.",
        ),
        make(
            "graphwork",
            "The graphify-style knowledge-graph skill: build and query a graph over code + docs + configs.",
            &["graph", "knowledge-graph", "codebase", "query"],
            &[
                "map this repo",
                "knowledge graph",
                "understand the codebase",
            ],
            "## How to use\n1. Walk the repo: code files + docs + SQL schemas + configs.\n2. Parse per-language with tree-sitter; emit entities + edges.\n3. Persist in the graph store.\n4. Answer with graph traversal first; fall back to BM25/full-text second.\n\n## Exit criteria\n- A structural query (module contains, who calls) resolves from the graph — no grep.",
        ),
        make(
            "jobs",
            "The Jobs vertical: scan portals → rubric-score → tailor CV/cover letter via the docx engine → auto-apply with Guard-2 approval.",
            &["jobs", "apply", "career", "cv", "cover letter"],
            &["apply to this job", "tailor my CV", "job search"],
            "## Queue\n- Scan portal listings; assign A–F rubric scores and only proceed ≥ 3.5.\n- Tailor the CV + cover letter with the office engine (docx), targeting rubric hits.\n- Every submission is a Guard-2 ticket — never silent mass-apply; the human confirms.\n- Evidence: the exact posting URL + score + targeted changes sit in the audit.",
        ),
    ]
}

/// Build one `(name, Skill)` from the bundled fields.
fn make(
    name: &str,
    description: &str,
    tools: &[&str],
    triggers: &[&str],
    body: &str,
) -> (String, Skill) {
    let manifest = SkillManifest {
        name: name.into(),
        description: description.into(),
        tools: tools.iter().map(|s| s.to_string()).collect(),
        triggers: triggers.iter().map(|s| s.to_string()).collect(),
        when_to_use: Vec::new(),
        scripts: vec![SkillScript {
            name: "help".into(),
            command: "cat SKILL.md".into(),
            lazy: true,
        }],
        references: Vec::new(),
        assets: Vec::new(),
        author: "everyaios".into(),
        created: "2026-08-24".into(),
        version: "1.0.0".into(),
        user_invocable: false,
        disable_model_invocation: false,
    };
    (
        name.into(),
        Skill {
            manifest,
            body: body.into(),
        },
    )
}

/// The doc-75 license-boundary record (source-available, never copied).
pub const DOC75_REFERENCE_NOTE: &str = "anthropics/skills document-skills are source-available \
(non-open) — reference-only patterns for our surgical engines; we never copy their text. \
Bundled SKILL.md pack = folder now with frontmatter + body.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_inbuilt_are_named_and_parse() {
        let skills = all_inbuilt();
        assert_eq!(skills.len(), INBUILT_SKILL_NAMES.len());
        for (name, skill) in &skills {
            assert_eq!(skill.manifest.name, *name);
            assert_eq!(skill.manifest.author, "everyaios");
            assert!(!skill.body.is_empty());
            let md = skill.to_skill_md();
            let parsed = Skill::from_skill_md(&md, "inbuilt").expect("roundtrip parse");
            assert_eq!(parsed.manifest.name, *name);
            assert_eq!(parsed.body, skill.body);
        }
    }

    #[test]
    fn seed_is_idempotent_and_user_copy_wins() {
        let root = std::env::temp_dir().join(format!("inbuilt-test-{}", std::process::id()));
        let first = seed_inbuilt(&root).unwrap();
        assert_eq!(first.len(), INBUILT_SKILL_NAMES.len());
        let second = seed_inbuilt(&root).unwrap();
        assert!(second.is_empty()); // already present
        // user copy wins
        let dir = root.join("office-documents");
        std::fs::write(dir.join("SKILL.md"), "user's own").unwrap();
        let third = seed_inbuilt(&root).unwrap();
        assert!(!third.contains(&"office-documents".to_string()));
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "user's own"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn covers_every_named_surface() {
        let names: Vec<String> = all_inbuilt().iter().map(|(n, _)| n.clone()).collect();
        for expected in [
            "office-documents",
            "browser-automation",
            "storage-intelligence",
            "code-intelligence",
            "document-creation",
            "skill-creator",
            "ui-ux-pro-max",
            "design-system",
            "engineering-exit-criteria",
            "graphwork",
            "jobs",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }
}
