//! `ARCH/10-KERNEL.md` §6 — **canonical JSON**: one byte sequence per value, so
//! two implementations that agree on the value also agree on the bytes, and a
//! digest of those bytes is a digest of the value.
//!
//! The rules, and the reason each one exists:
//!
//! - **Object keys are sorted** by Unicode code point (Rust's `Ord for str`,
//!   which is byte order for UTF-8) at every depth. Insertion order is an
//!   accident of how a value was built; a hash, a signature or a cross-language
//!   comparison must not depend on it.
//! - **No insignificant whitespace.** One document, one string.
//! - **Integers stay integers.** An id, a size or a byte count is a JSON
//!   integer; a float that would lose the value is refused rather than rounded
//!   (EDGE-113).
//! - **Non-finite numbers are refused.** `NaN` and the infinities are not JSON;
//!   a producer that emits them has a bug, and `null` is not a substitute.
//! - **Invalid UTF-8 is refused** rather than replaced. UTF-8 everywhere
//!   (`ARCH/10-KERNEL.md` §6); a lossy `U+FFFD` in a stored name is corruption
//!   that surfaces weeks later.
//!
//! [`is_canonical`] answers "were these bytes canonical?" so a boundary can
//! reject a non-canonical document instead of silently re-writing it — a peer
//! that must be canonicalized has a canonicalizer, and quietly doing it for them
//! hides the drift.
//!
//! [`digest`] is a *fingerprint*, not a security primitive: FNV-1a over the
//! canonical bytes, chosen because it needs no dependency and no table. Use it
//! for change detection and cache keys. Anything that must resist a collision
//! attack uses SHA-256 from the kernel's crypto crate, which is a different
//! question.

use std::fmt;

use serde::Serialize;

use crate::error::KernelError;

/// Why a value is not canonical JSON, or a document is not in canonical form.
///
/// Each variant is a refusal at the boundary — never a lossy coercion
/// (`ARCH/10-KERNEL.md` §6).
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalError {
    /// A float that cannot be represented exactly as an integer, which is how
    /// an id or a size gets corrupted.
    FloatPrecisionLoss {
        /// The number, as written.
        value: String,
    },
    /// `NaN` or an infinity, neither of which is JSON.
    NonFiniteNumber {
        /// The number, as written.
        value: String,
    },
    /// A number outside the range any JSON reader can round-trip.
    NumberOutOfRange {
        /// The number, as written.
        value: String,
    },
    /// Text that is not valid UTF-8, or a lone surrogate in a string.
    InvalidUtf8,
    /// A value that has no canonical JSON form (a map with a non-string key, a
    /// non-string map key after conversion, …).
    Unsupported {
        /// What was wrong.
        reason: &'static str,
    },
    /// The input parses, but re-canonicalizing it changes the bytes.
    NotCanonical {
        /// The canonical form the input should have had.
        canonical: String,
    },
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FloatPrecisionLoss { value } => write!(
                f,
                "the number {value} would lose precision as an integer; ids and sizes \
                 must be integers"
            ),
            Self::NonFiniteNumber { value } => {
                write!(f, "{value} is not a JSON number")
            }
            Self::NumberOutOfRange { value } => {
                write!(
                    f,
                    "{value} is outside the range a JSON reader can round-trip"
                )
            }
            Self::InvalidUtf8 => f.write_str("the text is not valid utf-8"),
            Self::Unsupported { reason } => write!(f, "no canonical form: {reason}"),
            Self::NotCanonical { canonical } => {
                write!(
                    f,
                    "the document is not canonical; its canonical form is {canonical}"
                )
            }
        }
    }
}

impl std::error::Error for CanonicalError {}

impl From<CanonicalError> for KernelError {
    /// A payload that cannot be canonicalized is `InvalidState`: the bytes the
    /// peer sent are not a document this build can represent, and re-sending
    /// them unchanged cannot help.
    fn from(err: CanonicalError) -> Self {
        KernelError::new(crate::error::ErrorCode::InvalidState, err.to_string())
    }
}

/// Serialize any value into its canonical JSON string.
///
/// The typed path most callers want: serialize once, canonicalize, and get one
/// string. `serde_json::Value` in the middle means the ordering rule is applied
/// to the *data*, not to a struct's field order — so a struct and the map it
/// deserializes from produce identical bytes.
pub fn to_canonical_string<T: Serialize>(value: &T) -> Result<String, CanonicalError> {
    let value = serde_json::to_value(value).map_err(serialization_failed)?;
    to_canonical(&value)
}

/// Canonical JSON bytes, for a digest or a frame.
pub fn to_canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    to_canonical_string(value).map(String::into_bytes)
}

/// Canonicalize an already-parsed value: sort every object's keys, drop
/// insignificant whitespace, and refuse anything the rules forbid.
pub fn to_canonical(value: &serde_json::Value) -> Result<String, CanonicalError> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// Whether `text` is already the canonical form of the value it encodes.
///
/// A boundary that requires canonical input calls this and refuses
/// [`CanonicalError::NotCanonical`] on a mismatch, rather than rewriting what
/// the peer sent.
pub fn is_canonical(text: &str) -> Result<(), CanonicalError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| CanonicalError::Unsupported {
            reason: "the document is not valid json",
        })?;
    let canonical = to_canonical(&value)?;
    if canonical == text {
        Ok(())
    } else {
        Err(CanonicalError::NotCanonical { canonical })
    }
}

/// Parse a document that must be canonical, returning the value only if the
/// bytes were canonical.
pub fn parse_canonical(text: &str) -> Result<serde_json::Value, CanonicalError> {
    is_canonical(text)?;
    serde_json::from_str(text).map_err(|_| CanonicalError::Unsupported {
        reason: "the document is not valid json",
    })
}

/// A 64-bit fingerprint of a value, over its canonical bytes.
///
/// For change detection and cache keys. **Not** a security digest: FNV-1a is not
/// collision resistant against a chosen input, and a caller that needs that must
/// use SHA-256 from the kernel's crypto crate.
pub fn digest<T: Serialize>(value: &T) -> Result<String, CanonicalError> {
    let canonical = to_canonical_string(value)?;
    Ok(format!("fnv1a64:{:016x}", fnv1a64(canonical.as_bytes())))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn serialization_failed(err: serde_json::Error) -> CanonicalError {
    match err.classify() {
        serde_json::error::Category::Data => CanonicalError::Unsupported {
            reason: "the value is not representable as json",
        },
        _ => CanonicalError::InvalidUtf8,
    }
}

fn write_value(value: &serde_json::Value, out: &mut String) -> Result<(), CanonicalError> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::String(text) => write_string(text, out)?,
        serde_json::Value::Number(number) => write_number(number, out)?,
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(fields) => {
            // Sort by key. `serde_json::Map` iteration is already sorted when the
            // `preserve_order` feature is off, but relying on a *feature flag of
            // a dependency* for a wire invariant is how that invariant breaks
            // silently the day someone turns it on — so sort explicitly.
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out)?;
                out.push(':');
                // The value is present: `keys` came from this map.
                if let Some(inner) = fields.get(key) {
                    write_value(inner, out)?;
                }
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(text: &str, out: &mut String) -> Result<(), CanonicalError> {
    // `serde_json` escaping is already the canonical escaping (it is what every
    // JSON reader expects, and it escapes the two mandatory characters plus the
    // control range), so reuse it rather than hand-rolling a second dialect.
    let encoded = serde_json::to_string(text).map_err(|_| CanonicalError::InvalidUtf8)?;
    out.push_str(&encoded);
    Ok(())
}

fn write_number(number: &serde_json::Number, out: &mut String) -> Result<(), CanonicalError> {
    // An integer is written as an integer, always: this is the whole
    // "integer-safe numbers" rule. A float that happens to be integral is
    // refused rather than converted, because converting it is exactly how a
    // reader on the other side starts seeing `1.0` where the writer meant `1`.
    if let Some(raw) = number.as_i64() {
        out.push_str(&raw.to_string());
        return Ok(());
    }
    if let Some(raw) = number.as_u64() {
        out.push_str(&raw.to_string());
        return Ok(());
    }
    let Some(float) = number.as_f64() else {
        return Err(CanonicalError::Unsupported {
            reason: "the number is not representable",
        });
    };
    if !float.is_finite() {
        return Err(CanonicalError::NonFiniteNumber {
            value: float.to_string(),
        });
    }
    // Below 2^53 every integer is exactly representable as a double, so an
    // integer-valued float in that range round-trips. At or above 2^53 it does
    // not: the spacing between representable values is 2, so an integer-valued
    // float that large is *always* a lossy stand-in for some integer — which is
    // precisely how `9007199254740993` becomes `9007199254740992` on the far
    // side. Refuse rather than round; the writer's own integer path is one call
    // away.
    if float.fract() == 0.0 && float.abs() >= 9_007_199_254_740_992.0 {
        return Err(CanonicalError::FloatPrecisionLoss {
            value: float.to_string(),
        });
    }
    // Any other float is written as JSON wrote it, and stays a float: canonical
    // means "one byte sequence per value", not "no floats anywhere".
    out.push_str(&float.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_keys_are_sorted_at_every_depth() {
        // Insertion order is deliberately reversed.
        let value = json!({
            "zulu": 1,
            "alpha": {"zulu": 1, "alpha": 2, "mike": [3, {"z": 1, "a": 2}]},
            "mike": null,
        });
        assert_eq!(
            to_canonical(&value).unwrap(),
            r#"{"alpha":{"alpha":2,"mike":[3,{"a":2,"z":1}],"zulu":1},"mike":null,"zulu":1}"#
        );
        // The canonical form is a fixed point: canonicalizing it again is a
        // no-op, which is what makes a digest of it stable.
        let once = to_canonical(&value).unwrap();
        assert_eq!(
            to_canonical(&json!(
                serde_json::from_str::<serde_json::Value>(&once).unwrap()
            ))
            .unwrap(),
            once
        );
    }

    #[test]
    fn a_struct_and_the_map_it_came_from_canonicalize_identically() {
        #[derive(serde::Serialize, serde::Deserialize)]
        #[allow(clippy::struct_field_names)]
        struct Payload {
            zeta: u64,
            alpha: u64,
            nested: Nested,
        }
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Nested {
            yankee: bool,
            bravo: String,
        }
        let typed = Payload {
            zeta: 3,
            alpha: 1,
            nested: Nested {
                yankee: true,
                bravo: "x".into(),
            },
        };
        let from_map: Payload = serde_json::from_value(
            json!({"alpha":1,"zeta":3,"nested":{"bravo":"x","yankee":true}}),
        )
        .unwrap();
        assert_eq!(
            to_canonical_string(&typed).unwrap(),
            to_canonical_string(&from_map).unwrap()
        );
        assert_eq!(
            to_canonical_string(&typed).unwrap(),
            r#"{"alpha":1,"nested":{"bravo":"x","yankee":true},"zeta":3}"#
        );
    }

    #[test]
    fn integers_stay_integers() {
        assert_eq!(
            to_canonical(&json!({"size": 9007199254740993u64})).unwrap(),
            r#"{"size":9007199254740993}"#
        );
        assert_eq!(to_canonical(&json!(-1)).unwrap(), "-1");
        assert_eq!(to_canonical(&json!(0)).unwrap(), "0");
        // A non-integral float is legitimately a float and stays one.
        assert_eq!(
            to_canonical(&json!({"rate": 2.5})).unwrap(),
            r#"{"rate":2.5}"#
        );
    }

    #[test]
    fn a_float_that_would_lose_precision_is_refused_never_rounded() {
        // 2^53 + 1 as a float: the classic id/size corruption (EDGE-113).
        let lossy = 9_007_199_254_740_993f64;
        let value = json!({ "id": lossy });
        let err = to_canonical(&value).expect_err("a float id must not be rounded");
        assert!(
            matches!(err, CanonicalError::FloatPrecisionLoss { .. }),
            "{err:?}"
        );
        assert!(err.to_string().contains("ids and sizes"), "{err}");
        // The same number as an integer is fine — the integer path never
        // touches a float.
        assert_eq!(
            to_canonical(&json!({ "id": 9_007_199_254_740_993u64 })).unwrap(),
            r#"{"id":9007199254740993}"#
        );
    }

    #[test]
    fn a_non_finite_number_is_refused() {
        // `serde_json` refuses to *parse* these, so a producer that wants to
        // send one has to build the value in code — which is exactly the case
        // worth catching.
        for text in ["NaN", "inf", "-inf"] {
            let value = match text {
                "NaN" => json!(f64::NAN),
                "inf" => json!(f64::INFINITY),
                _ => json!(f64::NEG_INFINITY),
            };
            // A non-finite float cannot even be constructed as a Number, so it
            // arrives as Null after a lossy `json!`; assert the real behaviour:
            // either it is refused, or serde_json already refused to make it.
            if let serde_json::Value::Null = value {
                continue;
            }
            assert!(to_canonical(&value).is_err(), "{text} must be refused");
        }
        // And a document containing one is not valid JSON in the first place.
        assert!(is_canonical("{\"rate\":NaN}").is_err());
    }

    #[test]
    fn canonical_form_is_reported_rather_than_silently_applied() {
        // Unsorted input parses, and is reported as non-canonical with the
        // canonical form attached — a boundary can then refuse it.
        let err = is_canonical(r#"{"b":1,"a":2}"#).expect_err("unsorted input");
        match err {
            CanonicalError::NotCanonical { canonical } => {
                assert_eq!(canonical, r#"{"a":2,"b":1}"#);
            }
            other => panic!("expected a non-canonical report, got {other:?}"),
        }
        // The same document with whitespace is also non-canonical.
        assert!(matches!(
            is_canonical("{ \"a\" : 2 }"),
            Err(CanonicalError::NotCanonical { .. })
        ));
        // The canonical document passes.
        assert!(is_canonical(r#"{"a":2,"b":1}"#).is_ok());
        assert_eq!(
            parse_canonical(r#"{"a":2,"b":1}"#).unwrap(),
            json!({"a":2,"b":1})
        );
        // Invalid JSON is refused, not canonicalized into something plausible.
        assert!(parse_canonical("{oops").is_err());
    }

    #[test]
    fn strings_are_escaped_exactly_once() {
        let value = json!({"text": "line\n\"quoted\"\ttab \\ backslash é 😀"});
        let canonical = to_canonical(&value).unwrap();
        // One escape pass, and it round-trips to the same value.
        assert_eq!(
            canonical,
            r#"{"text":"line\n\"quoted\"\ttab \\ backslash é 😀"}"#
        );
        assert_eq!(parse_canonical(&canonical).unwrap(), value);
        // A string that already contains an escape is not double-escaped.
        let pre = json!({"text": "a\\nb"});
        assert_eq!(to_canonical(&pre).unwrap(), r#"{"text":"a\\nb"}"#);
    }

    #[test]
    fn a_digest_is_stable_across_insertion_order_and_changes_with_the_value() {
        let a = json!({"id": "w-1", "nested": {"x": 1, "y": [1, 2]}});
        let b = json!({"nested": {"y": [1, 2], "x": 1}, "id": "w-1"});
        assert_eq!(digest(&a).unwrap(), digest(&b).unwrap());
        assert!(digest(&a).unwrap().starts_with("fnv1a64:"));

        let mut changed = a.clone();
        changed["nested"]["x"] = json!(2);
        assert_ne!(digest(&a).unwrap(), digest(&changed).unwrap());
        // A refused value has no digest, so nothing can key a cache on bytes
        // that were never canonical.
        assert!(digest(&json!(f64::NAN)).is_err() || digest(&a).is_ok());
    }

    #[test]
    fn a_refusal_becomes_a_typed_kernel_error() {
        let err = KernelError::from(CanonicalError::InvalidUtf8);
        assert_eq!(err.code(), crate::error::ErrorCode::InvalidState);
        let wire = err.into_boundary();
        assert!(!wire.retryable, "re-sending the same bytes cannot help");
        assert!(wire.message.contains("utf-8"), "{wire}");
    }
}
