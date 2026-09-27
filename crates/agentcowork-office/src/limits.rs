//! Bounded-memory patch policy (ARCH/04 §4.6 — the honest version of the
//! "single-pass, <15MB on a 500-page document" claim).
//!
//! # What this module does and does not promise
//!
//! The engine is **DOM + byte-range** by design: each XML part is parsed
//! whole by `roxmltree` and patched by byte range into the original source
//! (`crate::xml`). It does **not** stream, and nothing here claims it does.
//! What it *can* promise is that it never attempts a load it cannot bound:
//! every mutating entry point checks the container size, the entry count, each
//! decompressed part, and the running total of loaded parts, and **refuses
//! with a named reason** instead of allocating without limit.
//!
//! # Why these numbers
//!
//! - The container is held twice (the verbatim `raw` copy for `raw_entry`
//!   plus the `zip` reader's own buffer), so the archive ceiling is the
//!   dominant resident cost: 64 MiB ≈ 128 MiB resident.
//! - `roxmltree` retains the source text *and* a node tree, so a part's DOM
//!   is a small multiple of its byte size. An 8 MiB part therefore bounds the
//!   DOM at a low-hundreds of MiB worst case, which is a defensible ceiling
//!   for a desktop process.
//! - A 500-page Word document is typically a few hundred KB to a few MB of
//!   `word/document.xml`, so the defaults cover the case ARCH/04 §4.6
//!   originally named with room to spare. The refusal path exists for
//!   pathological inputs (10^5-page bodies, media-heavy decks). Media-heavy
//!   decks are expected to hit the **archive** ceiling first, and the
//!   intended way back under it is `crate::media_gc`.
//!
//! Every ceiling is configurable, per process, through
//! [`PatchLimits::from_env`].

use crate::docx::OfficeError;

/// Environment variable overriding the per-part decompressed ceiling.
pub const ENV_MAX_PART_BYTES: &str = "AGENTCOWORK_OFFICE_MAX_PART_BYTES";
/// Environment variable overriding the whole-container ceiling.
pub const ENV_MAX_ARCHIVE_BYTES: &str = "AGENTCOWORK_OFFICE_MAX_ARCHIVE_BYTES";
/// Environment variable overriding the running total of loaded parts.
pub const ENV_MAX_TOTAL_PART_BYTES: &str = "AGENTCOWORK_OFFICE_MAX_TOTAL_PART_BYTES";
/// Environment variable overriding the maximum entry count.
pub const ENV_MAX_PARTS: &str = "AGENTCOWORK_OFFICE_MAX_PARTS";
/// Retired spellings of the ceilings above: still honored as a read-only
/// fallback, never written (DEC-053).
pub const LEGACY_ENV_MAX_PART_BYTES: &str = "EVERYAIOS_OFFICE_MAX_PART_BYTES";
/// Retired spelling of [`ENV_MAX_ARCHIVE_BYTES`]: still honored.
pub const LEGACY_ENV_MAX_ARCHIVE_BYTES: &str = "EVERYAIOS_OFFICE_MAX_ARCHIVE_BYTES";
/// Retired spelling of [`ENV_MAX_TOTAL_PART_BYTES`]: still honored.
pub const LEGACY_ENV_MAX_TOTAL_PART_BYTES: &str = "EVERYAIOS_OFFICE_MAX_TOTAL_PART_BYTES";
/// Retired spelling of [`ENV_MAX_PARTS`]: still honored.
pub const LEGACY_ENV_MAX_PARTS: &str = "EVERYAIOS_OFFICE_MAX_PARTS";

/// Default per-part decompressed ceiling: 8 MiB.
pub const DEFAULT_MAX_PART_BYTES: u64 = 8 * 1024 * 1024;
/// Default whole-container ceiling: 64 MiB.
pub const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
/// Default running total of loaded parts: 48 MiB.
pub const DEFAULT_MAX_TOTAL_PART_BYTES: u64 = 48 * 1024 * 1024;
/// Default entry-count ceiling: 4096 parts.
pub const DEFAULT_MAX_PARTS: usize = 4096;

/// Which ceiling a refusal came from (machine-readable, not prose).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// One decompressed XML/media part.
    PartBytes,
    /// The whole container file.
    ArchiveBytes,
    /// The sum of every part loaded for one patch.
    TotalPartBytes,
    /// The number of entries in the container.
    PartCount,
}

impl LimitKind {
    /// Stable identifier for audit/receipt payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            LimitKind::PartBytes => "part_size",
            LimitKind::ArchiveBytes => "archive_size",
            LimitKind::TotalPartBytes => "total_part_size",
            LimitKind::PartCount => "part_count",
        }
    }
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LimitKind::PartBytes => "per-part",
            LimitKind::ArchiveBytes => "archive",
            LimitKind::TotalPartBytes => "total-loaded-parts",
            LimitKind::PartCount => "entry-count",
        })
    }
}

/// The explicit size policy a patch runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchLimits {
    /// Max decompressed size of any single part the engine will load.
    pub max_part_bytes: u64,
    /// Max size of the whole container the engine will open.
    pub max_archive_bytes: u64,
    /// Max total decompressed bytes across every part loaded for one patch.
    pub max_total_part_bytes: u64,
    /// Max number of entries in the container.
    pub max_parts: usize,
}

impl Default for PatchLimits {
    /// The process policy: [`PatchLimits::default_policy`] with any
    /// `AGENTCOWORK_OFFICE_*` environment override applied (retired
    /// `EVERYAIOS_OFFICE_*` spellings honored as a fallback). Unset variables
    /// (the normal case, including in tests) give the documented constants.
    fn default() -> Self {
        Self::from_env()
    }
}

impl PatchLimits {
    /// The fixed documented policy, ignoring the environment. Tests and
    /// reproducible assertions use this; deployment uses [`Default`].
    pub fn default_policy() -> Self {
        Self {
            max_part_bytes: DEFAULT_MAX_PART_BYTES,
            max_archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
            max_total_part_bytes: DEFAULT_MAX_TOTAL_PART_BYTES,
            max_parts: DEFAULT_MAX_PARTS,
        }
    }

    /// The documented policy with `AGENTCOWORK_OFFICE_*` overrides applied
    /// (the retired `EVERYAIOS_OFFICE_*` spellings stay a read-only fallback).
    /// Unparsable or zero values are ignored (a bad env var must never
    /// silently disable the ceiling).
    pub fn from_env() -> Self {
        Self {
            max_part_bytes: env_limit_compat(
                "OFFICE_MAX_PART_BYTES",
                LEGACY_ENV_MAX_PART_BYTES,
                DEFAULT_MAX_PART_BYTES,
            ),
            max_archive_bytes: env_limit_compat(
                "OFFICE_MAX_ARCHIVE_BYTES",
                LEGACY_ENV_MAX_ARCHIVE_BYTES,
                DEFAULT_MAX_ARCHIVE_BYTES,
            ),
            max_total_part_bytes: env_limit_compat(
                "OFFICE_MAX_TOTAL_PART_BYTES",
                LEGACY_ENV_MAX_TOTAL_PART_BYTES,
                DEFAULT_MAX_TOTAL_PART_BYTES,
            ),
            max_parts: env_limit_compat(
                "OFFICE_MAX_PARTS",
                LEGACY_ENV_MAX_PARTS,
                DEFAULT_MAX_PARTS as u64,
            ) as usize,
        }
    }

    /// A policy with no ceiling at all. Only for callers that have already
    /// bounded the input by other means (and for tests that need to build an
    /// oversized fixture without tripping the guard).
    pub fn unbounded() -> Self {
        Self {
            max_part_bytes: u64::MAX,
            max_archive_bytes: u64::MAX,
            max_total_part_bytes: u64::MAX,
            max_parts: usize::MAX,
        }
    }

    /// Refuse a container larger than [`Self::max_archive_bytes`].
    pub fn check_archive(&self, bytes: u64) -> Result<(), OfficeError> {
        if bytes > self.max_archive_bytes {
            return Err(OfficeError::TooLarge {
                kind: LimitKind::ArchiveBytes,
                subject: "OOXML container".to_string(),
                actual: bytes,
                limit: self.max_archive_bytes,
            });
        }
        Ok(())
    }

    /// Refuse a package with more entries than [`Self::max_parts`].
    pub fn check_part_count(&self, count: usize) -> Result<(), OfficeError> {
        if count > self.max_parts {
            return Err(OfficeError::TooManyParts {
                count,
                limit: self.max_parts,
            });
        }
        Ok(())
    }

    /// Refuse a single decompressed part larger than [`Self::max_part_bytes`].
    pub fn check_part(&self, part: &str, bytes: u64) -> Result<(), OfficeError> {
        if bytes > self.max_part_bytes {
            return Err(OfficeError::TooLarge {
                kind: LimitKind::PartBytes,
                subject: format!("part {part}"),
                actual: bytes,
                limit: self.max_part_bytes,
            });
        }
        Ok(())
    }
}

/// The running total of parts loaded for one patch. Charges each part against
/// [`PatchLimits::max_total_part_bytes`] as it is read, so a package that is
/// under the per-part ceiling but collectively unbounded still refuses.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadBudget {
    used: u64,
}

impl LoadBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes charged so far.
    pub fn used(&self) -> u64 {
        self.used
    }

    /// Check `part` against the per-part ceiling, then charge it against the
    /// running total. Both refusals are named; neither mutates the budget.
    pub fn charge(
        &mut self,
        limits: &PatchLimits,
        part: &str,
        bytes: u64,
    ) -> Result<(), OfficeError> {
        limits.check_part(part, bytes)?;
        let next = self.used.saturating_add(bytes);
        if next > limits.max_total_part_bytes {
            return Err(OfficeError::TooLarge {
                kind: LimitKind::TotalPartBytes,
                subject: "parts loaded for this patch".to_string(),
                actual: next,
                limit: limits.max_total_part_bytes,
            });
        }
        self.used = next;
        Ok(())
    }
}

/// Read one ceiling override with the DEC-053 fallback: the new
/// `AGENTCOWORK_<REST>` spelling first, then the retired `EVERYAIOS_*` full
/// name. A missing, malformed, or zero value falls back to the documented
/// constant.
fn env_limit_compat(rest: &str, legacy_key: &str, default: u64) -> u64 {
    agentcowork_types::env_compat::get(rest)
        .or_else(|| std::env::var(legacy_key).ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-key reader the compat wrapper is built on (test-only now
    /// that `from_env` goes through [`env_limit_compat`]).
    fn env_limit(key: &str, default: u64) -> u64 {
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(default)
    }

    #[test]
    fn default_policy_matches_the_documented_constants() {
        let p = PatchLimits::default_policy();
        assert_eq!(p.max_part_bytes, 8 * 1024 * 1024);
        assert_eq!(p.max_archive_bytes, 64 * 1024 * 1024);
        assert_eq!(p.max_total_part_bytes, 48 * 1024 * 1024);
        assert_eq!(p.max_parts, 4096);
        // The default ceiling covers the case ARCH/04 §4.6 named.
        assert!(p.max_part_bytes > 3 * 1024 * 1024);
    }

    #[test]
    fn archive_over_ceiling_refuses_with_a_named_reason() {
        let p = PatchLimits::default_policy();
        assert!(p.check_archive(DEFAULT_MAX_ARCHIVE_BYTES).is_ok());
        let err = p.check_archive(DEFAULT_MAX_ARCHIVE_BYTES + 1).unwrap_err();
        match err {
            OfficeError::TooLarge {
                kind,
                subject,
                actual,
                limit,
            } => {
                assert_eq!(kind, LimitKind::ArchiveBytes);
                assert_eq!(kind.as_str(), "archive_size");
                assert!(subject.contains("container"));
                assert_eq!(actual, DEFAULT_MAX_ARCHIVE_BYTES + 1);
                assert_eq!(limit, DEFAULT_MAX_ARCHIVE_BYTES);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn part_over_ceiling_refuses_naming_the_part() {
        let p = PatchLimits::default_policy();
        assert!(p.check_part("word/document.xml", 1_000).is_ok());
        let err = p.check_part("word/document.xml", u64::MAX).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("word/document.xml"), "{msg}");
        assert!(msg.contains("per-part"), "{msg}");
    }

    #[test]
    fn part_count_ceiling_is_separate_from_bytes() {
        let p = PatchLimits::default_policy();
        assert!(p.check_part_count(10).is_ok());
        let err = p.check_part_count(usize::MAX).unwrap_err();
        assert!(matches!(
            err,
            OfficeError::TooManyParts { limit, .. } if limit == DEFAULT_MAX_PARTS
        ));
    }

    #[test]
    fn total_budget_charges_across_parts() {
        let limits = PatchLimits {
            max_part_bytes: 1000,
            max_archive_bytes: u64::MAX,
            max_total_part_bytes: 2500,
            max_parts: usize::MAX,
        };
        let mut budget = LoadBudget::new();
        budget.charge(&limits, "word/document.xml", 1000).unwrap();
        budget.charge(&limits, "word/header1.xml", 1000).unwrap();
        assert_eq!(budget.used(), 2000);
        // Each part is under the per-part ceiling, but the total is not.
        let err = budget
            .charge(&limits, "word/header2.xml", 1000)
            .unwrap_err();
        assert!(matches!(
            err,
            OfficeError::TooLarge {
                kind: LimitKind::TotalPartBytes,
                ..
            }
        ));
        // A refused charge does not consume budget.
        assert_eq!(budget.used(), 2000);
    }

    #[test]
    fn unbounded_never_refuses() {
        let u = PatchLimits::unbounded();
        assert!(u.check_archive(u64::MAX).is_ok());
        assert!(u.check_part("x", u64::MAX).is_ok());
        assert!(u.check_part_count(usize::MAX).is_ok());
    }

    #[test]
    fn env_override_parsing_falls_back_to_the_documented_default() {
        // The observable contract of the override reader: an absent variable
        // yields the documented constant it was asked to fall back to, so a
        // deployment that sets nothing gets the policy in this file and a
        // malformed value can never silently disable the ceiling.
        assert_eq!(env_limit("AGENTCOWORK_OFFICE_TEST_UNSET", 42), 42);
        assert_eq!(
            env_limit(ENV_MAX_PART_BYTES, DEFAULT_MAX_PART_BYTES),
            DEFAULT_MAX_PART_BYTES
        );
        assert_eq!(
            env_limit(ENV_MAX_ARCHIVE_BYTES, DEFAULT_MAX_ARCHIVE_BYTES),
            DEFAULT_MAX_ARCHIVE_BYTES
        );
        // `from_env` composes the four readers over the documented policy, and
        // `Default` is exactly `from_env`, so the engine's default policy and
        // the deployment override are the same code path.
        assert_eq!(PatchLimits::default(), PatchLimits::from_env());
        for v in [
            PatchLimits::from_env().max_part_bytes,
            PatchLimits::from_env().max_archive_bytes,
            PatchLimits::from_env().max_total_part_bytes,
        ] {
            assert!(v > 0);
        }
        assert!(PatchLimits::from_env().max_parts > 0);
    }
}
