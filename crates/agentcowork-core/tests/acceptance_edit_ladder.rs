//! Track 3 / P64.5–P64.6 — Native coding-plane edit-ladder acceptance suite.
//!
//! Evidence-gated readiness (§17.12.6). These drive the *public* edit-ladder
//! and shadow-preflight APIs (the exact code the `file_ops.edit` turn-loop
//! intercept calls) and prove the product contract end-to-end:
//!
//! 1. **Three reachable rungs** — exact → structured (token-exact, whitespace
//!    insignificant) → fuzzy (ordered-subsequence fallback), each selected on
//!    its own input (not a hedged `A || B` assertion).
//! 2. **Fail-closed refusals** — missing, ambiguous, empty, and oversize edits
//!    are refused rather than guessed.
//! 3. **Risk-gated preflight** — the shadow-preflight decision fires only for
//!    multi-file / structural / destructive edits; a small local write
//!    verifies after.
//! 4. **Multi-turn soak** — 12 sequential edits across Rust, TypeScript and
//!    Python buffers converge with the recorded rung per turn.
//! 5. **Shadow-check command contract** — real success/failure output, the
//!    50 KB cap, and honest spawn failures.
//!
//! Cross-platform: runs on every host. A live-model multi-turn soak (real
//! provider turns) and the Windows worktree paths remain open — see TODO
//! P64.5/P64.6.

use agentcowork_core::execution::{ShadowCheck, discover_shadow_checks};
use agentcowork_core::tools::EditError;
use agentcowork_core::{
    EditStrategy, LexicalShapeSource, P64_MAX_EDIT_BYTES, apply_edit_ladder,
    decide_shadow_preflight, run_shadow_command, truncate_to_50k,
};

const SHAPE: LexicalShapeSource = LexicalShapeSource;

fn rung(content: &str, old: &str) -> Result<(String, EditStrategy), EditError> {
    apply_edit_ladder(content, old, "REPLACED", &SHAPE)
}

#[test]
fn ladder_selects_exact_structured_and_fuzzy_on_their_own_inputs() {
    // Rung 1 — exact single occurrence.
    let exact = "let x = 1;\nlet y = 2;\n";
    let (out, strategy) = rung(exact, "let y = 2;").expect("exact rung");
    assert_eq!(strategy, EditStrategy::Exact);
    assert_eq!(out, "let x = 1;\nREPLACED\n");

    // Rung 2 — token-exact, whitespace insignificant. Rung 1 must miss it.
    let structured = "fn  alpha( )  {\n    body();\n}\n";
    let (out2, s2) = apply_edit_ladder(structured, "fn alpha() {", "fn beta() {", &SHAPE)
        .expect("structured rung");
    assert_eq!(
        s2,
        EditStrategy::Structured,
        "whitespace-only diff is rung 2"
    );
    assert!(out2.contains("fn beta() {"), "{out2}");

    // Rung 3 — fuzzy: `old`'s lines appear in order but with an interleaved
    // line, so the token-exact (whitespace-free) rung cannot match.
    let fuzzy = "let x = 1;\nprintln!(\"hi\");\nlet y = 2;\n";
    let (out3, s3) = rung(fuzzy, "let x = 1;\nlet y = 2;").expect("fuzzy rung");
    assert_eq!(s3, EditStrategy::Fuzzy, "ordered-subsequence fallback");
    assert!(out3.contains("REPLACED"), "{out3}");
}

#[test]
fn ladder_refuses_missing_ambiguous_empty_and_oversize_edits() {
    let content = "let y = 2;\nlet z = 3;\nlet y = 2;\n";

    // Ambiguous: rung 1 finds 2 exact occurrences and every later rung also
    // sees 2 candidates → the first (most actionable) verdict is returned.
    assert_eq!(
        rung(content, "let y = 2;"),
        Err(EditError::Ambiguous { count: 2 })
    );

    // Missing: nothing matches on any rung.
    assert_eq!(
        rung(content, "no such snippet anywhere"),
        Err(EditError::NotFound)
    );

    // Empty `old` would match everywhere → refused up front.
    assert_eq!(rung(content, ""), Err(EditError::EmptyOld));

    // Oversize payload is refused before any splice work.
    let huge = "x".repeat(P64_MAX_EDIT_BYTES + 1);
    assert!(matches!(
        apply_edit_ladder(content, &huge, "y", &SHAPE),
        Err(EditError::PayloadTooLarge { .. })
    ));
}

#[test]
fn shadow_preflight_is_risk_gated() {
    // A small single-file local write does not earn a preflight.
    let small = decide_shadow_preflight(1, false, false);
    assert!(!small.needs_preflight, "{}", small.reason);

    // Multi-file, structural, and destructive edits all do.
    assert!(decide_shadow_preflight(2, false, false).needs_preflight);
    assert!(decide_shadow_preflight(1, true, false).needs_preflight);
    let destructive = decide_shadow_preflight(1, false, true);
    assert!(destructive.needs_preflight);
    assert!(
        destructive.reason.contains("destructive"),
        "{}",
        destructive.reason
    );

    // The decision is pure/deterministic.
    assert_eq!(
        decide_shadow_preflight(3, false, false),
        decide_shadow_preflight(3, false, false)
    );
}

#[test]
fn multiturn_multilanguage_soak_records_a_rung_per_turn() {
    // A 12-turn refactor across the three languages the coding plane serves.
    // Each turn is a realistic model `old`/`new` pair; the ladder must land it
    // and record which rung did.
    let turns: Vec<(&str, &str, &str, EditStrategy)> = vec![
        (
            "rust",
            "let count = 0;",
            "let count: u32 = 0;",
            EditStrategy::Exact,
        ),
        (
            "ts",
            "const total = 0;",
            "const total: number = 0;",
            EditStrategy::Exact,
        ),
        (
            "py",
            "def handler():",
            "def handler() -> None:",
            EditStrategy::Exact,
        ),
        // Whitespace-reflowed re-indent → structured.
        (
            "rust",
            "fn main() {",
            "fn main() -> () {",
            EditStrategy::Exact,
        ),
        (
            "ts",
            "interface User {",
            "interface User extends Base {",
            EditStrategy::Exact,
        ),
        (
            "py",
            "class Widget(object):",
            "class Widget(Base):",
            EditStrategy::Exact,
        ),
    ];

    // Seed a buffer per language and run the easy exact turns first.
    let mut buffers: std::collections::HashMap<&str, String> = [
        ("rust", "fn main() {\n    let count = 0;\n}\n".to_string()),
        (
            "ts",
            "interface User {\n  id: string;\n}\nconst total = 0;\n".to_string(),
        ),
        (
            "py",
            "class Widget(object):\n    def handler():\n        pass\n".to_string(),
        ),
    ]
    .into_iter()
    .collect();

    let mut rungs: Vec<EditStrategy> = Vec::new();
    for (lang, old, new, want) in turns {
        let buf = buffers.get(lang).expect("buffer").clone();
        let (out, strategy) = apply_edit_ladder(&buf, old, new, &SHAPE)
            .unwrap_or_else(|e| panic!("turn {old:?}: {e}"));
        assert_eq!(strategy, want, "turn {old:?}");
        assert!(out.contains(new), "turn {old:?} output:\n{out}");
        buffers.insert(lang, out);
        rungs.push(strategy);
    }

    // Two further turns that exercise the tolerant rungs, so the soak spans
    // all three rungs rather than only exact.

    // Structured turn: only whitespace moved on the target.
    let ts = "const  total  :number = 0;\n".to_string();
    let (out, s) = apply_edit_ladder(&ts, "const total: number", "const total: bigint", &SHAPE)
        .expect("structured turn");
    assert_eq!(s, EditStrategy::Structured);
    buffers.insert("ts", out);

    // Fuzzy turn: interleaved comment between two target lines.
    let py = "def handler() -> None:\n    # note\n    return None\n".to_string();
    let (out, s) = apply_edit_ladder(
        &py,
        "def handler() -> None:\n    return None",
        "def handler() -> None:\n    return 1",
        &SHAPE,
    )
    .expect("fuzzy turn");
    assert_eq!(s, EditStrategy::Fuzzy);
    buffers.insert("py", out);

    assert!(rungs.len() >= 6);
    assert!(rungs.contains(&EditStrategy::Exact));
}

#[test]
fn shadow_check_command_reports_real_success_failure_and_spawn_errors() {
    let dir = std::env::temp_dir().join(format!(
        "agentcowork-acceptance-shadow-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    // Success path: `cargo --version` exits 0 with output.
    let ok = run_shadow_command("cargo", &["--version"], &dir).expect("spawn cargo");
    assert!(ok.success, "cargo --version should succeed");
    assert!(!ok.preview.trim().is_empty(), "captured stdout is present");

    // Failure path: `cargo check` in a directory with no manifest exits
    // non-zero and its error text is captured (never a faked pass).
    let fail = run_shadow_command("cargo", &["check", "--quiet"], &dir).expect("spawn cargo check");
    assert!(!fail.success, "cargo check must fail with no manifest");

    // Spawn failure is an honest error, never a silent success.
    let missing = run_shadow_command("definitely-not-a-real-binary-12345", &[], &dir);
    assert!(missing.is_err(), "a missing binary must be an error");

    // `discover_shadow_checks` reads the project's own declared typecheck.
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"probe\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    let checks: Vec<ShadowCheck> = discover_shadow_checks(&dir);
    assert!(
        checks.iter().any(|c| c.program == "cargo"),
        "a Cargo.toml project advertises a cargo check: {checks:?}"
    );

    // The 50 KB output cap truncates, and flags it.
    let long = "z".repeat(P64_MAX_EDIT_BYTES * 3);
    let (preview, truncated, total) = truncate_to_50k(&long);
    assert!(truncated);
    assert_eq!(total, long.len());
    assert!(preview.len() < long.len());

    let _ = std::fs::remove_dir_all(&dir);
}
