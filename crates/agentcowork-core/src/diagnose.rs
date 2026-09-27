//! P37 live resource diagnose: the `rss_measure` snapshot → a plain-language
//! verdict + suggestion (the Resources panel's AI-diagnose line). The
//! diagnosis is rule-based and deterministic — no model needed for the
//! verdict; the coordinator can attach a model explanation on top.

use serde::{Deserialize, Serialize};

/// The diagnosis severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Normal,
    Heavy,
    Critical,
}

/// One diagnosis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDiagnosis {
    pub severity: Severity,
    /// The measured combined RSS (MB).
    pub rss_mb: u64,
    pub suggestion: String,
}

/// Deterministic thresholds (MB of combined RSS: core + sidecar + browser).
pub const HEAVY_MB: u64 = 300;
pub const CRITICAL_MB: u64 = 700;

/// Diagnose a measured snapshot. Pure — the numbers come from
/// [`crate::rss_measure`].
pub fn diagnose(rss_mb: u64) -> ResourceDiagnosis {
    if rss_mb >= CRITICAL_MB {
        ResourceDiagnosis {
            severity: Severity::Critical,
            rss_mb,
            suggestion: "Critical footprint: close idle browser tabs, drop unused worktrees, and consider a sidecar restart (P29 target ~15MB).".into(),
        }
    } else if rss_mb >= HEAVY_MB {
        ResourceDiagnosis {
            severity: Severity::Heavy,
            rss_mb,
            suggestion: "Heavy footprint: memory compaction + releasing finished sub-agent worktrees will bring this down (P39.5 lazy-load already landed).".into(),
        }
    } else {
        ResourceDiagnosis {
            severity: Severity::Normal,
            rss_mb,
            suggestion: "Footprint is healthy — no action needed.".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_map_to_verdicts() {
        assert_eq!(diagnose(120).severity, Severity::Normal);
        assert_eq!(diagnose(400).severity, Severity::Heavy);
        assert_eq!(diagnose(800).severity, Severity::Critical);
        assert!(diagnose(400).suggestion.contains("compaction"));
        assert!(diagnose(120).suggestion.contains("healthy"));
    }

    #[test]
    fn boundary_values_are_stable() {
        assert_eq!(diagnose(HEAVY_MB - 1).severity, Severity::Normal);
        assert_eq!(diagnose(HEAVY_MB).severity, Severity::Heavy);
        assert_eq!(diagnose(CRITICAL_MB).severity, Severity::Critical);
    }
}
