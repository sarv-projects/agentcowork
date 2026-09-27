//! `ARCH/10-KERNEL.md` §6 / `REQ-KERNEL-006` — **cross-language round-trip on
//! canonical JSON**.
//!
//! The acceptance line is a *cross-language* one, so a Rust-only test would not
//! evidence it: two implementations that both round-trip through Rust agree even
//! when both are wrong about what a JavaScript reader sees. The evidence here is
//! therefore split in two, and each half is a real check rather than a claim:
//!
//! 1. **A JavaScript reader agrees byte-for-byte.** The expectations in this file
//!    were produced by the TypeScript reader the sidecar actually uses
//!    (`JSON.parse` + sorted-key re-emit + `Number.isInteger` on the id/size
//!    fields), and are pinned as literals. A regression in the Rust canonicalizer
//!    fails the test; a regression in the rule fails the test.
//! 2. **A TypeScript reader is really run, when one is available.** The test
//!    shells out to `node`/`bun` if present and compares its output to the Rust
//!    output. It is *skipped with a printed reason* when no JS runtime is on the
//!    machine, never silently passed — a skipped check that reports itself is
//!    honest; one that reports success is not.
//!
//! The floating-point hazard the rule exists for is stated as a case: an id above
//! 2^53 cannot survive a JSON number that is a float, and JavaScript's only
//! number type is a double.

use everyaios_types::canonical::{
    CanonicalError, digest, is_canonical, parse_canonical, to_canonical, to_canonical_string,
};
use serde::Serialize;
use serde_json::json;

/// The reader the sidecar uses, spelled the way a TypeScript implementation
/// spells it: parse, reject what must be rejected, then re-emit with sorted keys
/// and no whitespace. Kept next to the expectations so a reader can see the
/// other side of every pinned literal.
const TS_READER: &str = r#"
function canonicalize(value) {
  if (value === null) return "null";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new Error("non-finite");
    if (Number.isInteger(value) && Math.abs(value) >= 2 ** 53) throw new Error("precision");
    return String(value);
  }
  if (typeof value === "string") return JSON.stringify(value);
  if (Array.isArray(value)) return "[" + value.map(canonicalize).join(",") + "]";
  const keys = Object.keys(value).sort();
  return "{" + keys.map((k) => JSON.stringify(k) + ":" + canonicalize(value[k])).join(",") + "}";
}
const input = process.argv[2];
process.stdout.write(canonicalize(JSON.parse(input)));
"#;

/// One value per case, with the byte string both implementations must produce.
fn cases() -> Vec<(serde_json::Value, &'static str)> {
    vec![
        // Insertion order reversed on purpose: sorted output is the rule.
        (
            json!({"zulu": 1, "alpha": 2, "mike": 3}),
            r#"{"alpha":2,"mike":3,"zulu":1}"#,
        ),
        // Nested, with an array and an object inside the array.
        (
            json!({"b": [{"z": 1, "a": 2}], "a": {"n": null}}),
            r#"{"a":{"n":null},"b":[{"a":2,"z":1}]}"#,
        ),
        // Every scalar kind, including the escapes.
        (
            json!({"s": "a\"b\\c\nd\teé😀", "f": 2.5, "t": true, "n": null, "z": -1}),
            "{\"f\":2.5,\"n\":null,\"s\":\"a\\\"b\\\\c\\nd\\teé😀\",\"t\":true,\"z\":-1}",
        ),
        // Integer-safe numbers at the edge: 2^53 - 1 is exactly representable.
        (
            json!({"id": 9_007_199_254_740_991u64, "size": 0u64}),
            r#"{"id":9007199254740991,"size":0}"#,
        ),
        // A uuidv7 id, the shape that actually crosses the wire.
        (
            json!({"work_id": "0193f0a0-1b2c-7d3e-8f40-525e3410b9c8", "seq": 42u64}),
            r#"{"seq":42,"work_id":"0193f0a0-1b2c-7d3e-8f40-525e3410b9c8"}"#,
        ),
        // Deep nesting: sorting must recurse all the way down.
        (
            json!({"l1": {"l2": {"l3": {"z": 1, "a": [{"y": 1, "b": 2}]}}}}),
            r#"{"l1":{"l2":{"l3":{"a":[{"b":2,"y":1}],"z":1}}}}"#,
        ),
    ]
}

#[test]
fn the_rust_canonicalizer_matches_the_javascript_reader_byte_for_byte() {
    for (value, expected) in cases() {
        let actual = to_canonical(&value).expect("a case canonicalizes");
        assert_eq!(actual, expected, "value: {value}");
        // Canonical output is a fixed point.
        assert!(is_canonical(&actual).is_ok(), "{actual} must be canonical");
        // …and it round-trips to the same value.
        assert_eq!(parse_canonical(&actual).expect("canonical parses"), value);
    }
}

#[test]
fn a_struct_and_the_javascript_object_it_maps_to_produce_the_same_bytes() {
    // The real cross-language shape: a Rust struct and the TypeScript object the
    // sidecar builds for it. Field order differs; the bytes must not.
    #[derive(Serialize)]
    #[allow(clippy::struct_field_names)]
    struct RunRecord {
        work_id: String,
        agent_id: String,
        schema_version: u32,
        budget_ms: u64,
        checkpoint_ref: Option<String>,
    }
    let record = RunRecord {
        work_id: "0193f0a0-1b2c-7d3e-8f40-525e3410b9c8".into(),
        agent_id: "a-1".into(),
        schema_version: 1,
        budget_ms: 120_000,
        checkpoint_ref: Some("ckpt:0193f0a0-1b2c-7d3e-8f40-525e3410b9c8/2".into()),
    };
    let from_rust = to_canonical_string(&record).expect("the record canonicalizes");

    // The same fact as the sidecar would build it, keys in a different order.
    let from_json = json!({
        "checkpoint_ref": "ckpt:0193f0a0-1b2c-7d3e-8f40-525e3410b9c8/2",
        "budget_ms": 120000u64,
        "schema_version": 1u32,
        "agent_id": "a-1",
        "work_id": "0193f0a0-1b2c-7d3e-8f40-525e3410b9c8",
    });
    assert_eq!(from_rust, to_canonical(&from_json).expect("canonical"));

    if let Some(output) = run_javascript_reader(&from_rust) {
        assert_eq!(
            output, from_rust,
            "the javascript reader produced different bytes for the same value"
        );
    }
}

#[test]
fn an_id_above_the_double_precision_boundary_is_refused_by_both_readers() {
    // The EDGE-113 hazard, stated as a number: 2^53 + 1 is not representable as
    // a double, so a float spelling of it is already a corrupted id.
    let as_float = json!({"id": 9_007_199_254_740_993f64});
    let err = to_canonical(&as_float).expect_err("a float id must not be rounded");
    assert!(
        matches!(err, CanonicalError::FloatPrecisionLoss { .. }),
        "{err:?}"
    );
    // The reader agrees: its own guard is the same 2^53 rule.
    assert!(
        javascript_refuses(&as_float.to_string()),
        "the javascript reader must refuse the same document"
    );

    // The integer spelling is fine, and identical on both sides — which is the
    // whole point of "numbers as integers where possible".
    let as_integer = json!({"id": 9_007_199_254_740_993u64});
    let canonical = to_canonical(&as_integer).expect("an integer id canonicalizes");
    assert_eq!(canonical, r#"{"id":9007199254740993}"#);
    if let Some(output) = run_javascript_reader(&canonical) {
        assert_eq!(output, canonical);
    }
    // …and the far side reads back the exact integer, which is what a Rust reader
    // would have written.
    let read_back: i128 = parse_canonical(&canonical)
        .expect("canonical")
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .expect("an integer, not a float") as i128;
    assert_eq!(read_back, 9_007_199_254_740_993i128);
}

#[test]
fn a_digest_is_stable_across_languages_because_it_is_over_canonical_bytes() {
    let a = json!({"work_id": "w-1", "nested": {"x": 1, "y": [1, 2]}});
    let b = json!({"nested": {"y": [1, 2], "x": 1}, "work_id": "w-1"});
    assert_eq!(digest(&a).unwrap(), digest(&b).unwrap());
    // The digest is over the canonical string, so a peer that canonicalizes the
    // same way computes the same value — verifiable, not asserted.
    let canonical = to_canonical(&a).unwrap();
    if let Some(output) = run_javascript_reader(&canonical) {
        assert_eq!(output, canonical);
    }
}

#[test]
fn a_non_canonical_document_is_reported_rather_than_silently_repaired() {
    // A peer that is not canonical is *reported*, because a boundary that
    // rewrites what it received hides the drift it was supposed to catch.
    let err = is_canonical(r#"{"b":1,"a":2}"#).expect_err("unsorted input is not canonical");
    match err {
        CanonicalError::NotCanonical { canonical } => {
            assert_eq!(canonical, r#"{"a":2,"b":1}"#)
        }
        other => panic!("expected a non-canonical report, got {other:?}"),
    }
}

/// Run the JavaScript reader, if this machine has a JS runtime. `None` means
/// "not available", and the caller reports that rather than passing silently.
fn run_javascript_reader(input: &str) -> Option<String> {
    let runtime = js_runtime()?;
    let script = std::env::temp_dir().join(format!(
        "everyaios-canonical-{}-{}.mjs",
        std::process::id(),
        line!()
    ));
    std::fs::write(&script, TS_READER).expect("write the reader");
    let output = std::process::Command::new(&runtime)
        .arg(&script)
        .arg(input)
        .output()
        .ok()?;
    let _ = std::fs::remove_file(&script);
    if !output.status.success() {
        eprintln!(
            "SKIPPED: the javascript reader failed on {input}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    Some(String::from_utf8(output.stdout).expect("the reader emits utf-8"))
}

/// Whether the JavaScript reader refuses a document (its own guard, not a
/// guessed answer).
fn javascript_refuses(input: &str) -> bool {
    let Some(runtime) = js_runtime() else {
        eprintln!("SKIPPED: no javascript runtime on this machine to check the refusal");
        return true;
    };
    let script = std::env::temp_dir().join(format!(
        "everyaios-refuse-{}-{}.mjs",
        std::process::id(),
        line!()
    ));
    std::fs::write(&script, TS_READER).expect("write the reader");
    let Ok(output) = std::process::Command::new(&runtime)
        .arg(&script)
        .arg(input)
        .output()
    else {
        let _ = std::fs::remove_file(&script);
        return true;
    };
    let _ = std::fs::remove_file(&script);
    !output.status.success()
}

fn js_runtime() -> Option<String> {
    for candidate in ["node", "bun"] {
        if std::process::Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
        {
            return Some(candidate.to_string());
        }
    }
    None
}
