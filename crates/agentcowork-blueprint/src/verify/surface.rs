//! P48.3 — **per-surface Verify beyond EV1-at-plan**. EV1's `OutcomeCheck`
//! is filesystem-only (file exists / hash / contains). This module is the
//! per-surface generalization: a `SurfaceCheck` names the surface (file,
//! shell, git, office, browser, desktop, search, network) and the exact
//! post-effect observable that proves the effect landed — and a
//! `SurfaceContext` carries the runtime observation the attached engine
//! reports. Missing context (engine not attached) is **honestly
//! `Unverifiable`**, never a fake pass — the same honesty invariant as EV1's
//! "no checkable outcomes" rule (doc 05).
//!
//! Verdicts map onto the EV1 `CompletionStatus` taxonomy (`verified_complete`,
//! `partially_complete`, `failed_safely`, `unverifiable`, ...) so a per-effect
//! verdict can fold into a K1 `VerificationSummary` without a new vocabulary.

use serde::{Deserialize, Serialize};

/// The execution surfaces from the §4.3 executor-attachment matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    File,
    Shell,
    Git,
    Office,
    Browser,
    Desktop,
    Search,
    Network,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Shell => "shell",
            Self::Git => "git",
            Self::Office => "office",
            Self::Browser => "browser",
            Self::Desktop => "desktop",
            Self::Search => "search",
            Self::Network => "network",
        }
    }
}

/// A post-effect check on a specific surface. Each variant names the exact
/// observable that proves the effect landed; the runtime observation arrives
/// via [`SurfaceContext`] at verify time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum SurfaceCheck {
    /// The file's hash must match `expected` (post-write verification).
    FileHash {
        path: String,
        algorithm: super::manifest::HashAlgorithm,
        expected: String,
    },
    /// The file's text must contain `substring` (post-write verification).
    FileContains { path: String, substring: String },
    /// A shell command must exit with `expected` (post-run verification).
    ShellExit { expected: i32 },
    /// The working tree must be clean after the effect.
    GitClean,
    /// A specific commit must be the branch head.
    GitHead { commit: String },
    /// An office cell must hold `expected` after the write.
    OfficeCell {
        path: String,
        sheet: String,
        address: String,
        expected: String,
    },
    /// The browser must have navigated to a URL with `prefix`.
    BrowserUrl { prefix: String },
    /// The desktop element ref must be visible/stable after the action.
    DesktopElement { ref_id: String },
    /// A search cascade must have returned at least `min_hits`.
    SearchHits { min_hits: usize },
    /// A network effect must have landed with the expected status class.
    NetworkStatus { expected_class: u16 },
}

impl SurfaceCheck {
    pub fn surface(&self) -> Surface {
        match self {
            Self::FileHash { .. } | Self::FileContains { .. } => Surface::File,
            Self::ShellExit { .. } => Surface::Shell,
            Self::GitClean { .. } | Self::GitHead { .. } => Surface::Git,
            Self::OfficeCell { .. } => Surface::Office,
            Self::BrowserUrl { .. } => Surface::Browser,
            Self::DesktopElement { .. } => Surface::Desktop,
            Self::SearchHits { .. } => Surface::Search,
            Self::NetworkStatus { .. } => Surface::Network,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::FileHash {
                path, algorithm, ..
            } => {
                format!("file.hash({path}, {algorithm:?})")
            }
            Self::FileContains { path, .. } => format!("file.contains({path})"),
            Self::ShellExit { expected } => format!("shell.exit({expected})"),
            Self::GitClean => "git.clean".into(),
            Self::GitHead { commit } => format!("git.head({commit})"),
            Self::OfficeCell {
                path,
                sheet,
                address,
                ..
            } => format!("office.cell({path}, {sheet}!{address})"),
            Self::BrowserUrl { prefix } => format!("browser.url({prefix}…)"),
            Self::DesktopElement { ref_id } => format!("desktop.element({ref_id})"),
            Self::SearchHits { min_hits } => format!("search.hits(≥{min_hits})"),
            Self::NetworkStatus { expected_class } => {
                format!("network.status({expected_class}xx)")
            }
        }
    }
}

/// The runtime observation the attached engine reports after the effect. A
/// check whose context is missing is honestly `Unverifiable` — the surface
/// was not attached, so the claim cannot be proven.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SurfaceContext {
    /// File bytes observed post-effect (for hash/contains checks).
    #[serde(default)]
    pub file_bytes: Option<Vec<u8>>,
    /// Shell exit code observed post-run.
    #[serde(default)]
    pub shell_exit: Option<i32>,
    /// Git working-tree cleanliness.
    #[serde(default)]
    pub git_clean: Option<bool>,
    /// Current branch head commit.
    #[serde(default)]
    pub git_head: Option<String>,
    /// Office cell value observed post-write.
    #[serde(default)]
    pub office_cell: Option<String>,
    /// Current browser URL observed post-navigation.
    #[serde(default)]
    pub browser_url: Option<String>,
    /// Desktop element visibility/stability observed post-action.
    #[serde(default)]
    pub desktop_element_visible: Option<bool>,
    /// Search result count observed.
    #[serde(default)]
    pub search_hits: Option<usize>,
    /// Network response status class observed (e.g. 2xx → 2).
    #[serde(default)]
    pub network_status_class: Option<u16>,
}

/// The per-surface verification verdict. `Verified` is the only pass;
/// everything else is an honest non-claim with the reason attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum SurfaceVerdict {
    /// The observed context satisfies the check.
    Verified { surface: Surface, detail: String },
    /// The surface was attached but could only partially confirm (EV1
    /// `partially_complete`).
    Degraded { surface: Surface, reason: String },
    /// The surface was attached and the check ran, but the effect did not
    /// land as required (EV1 `failed_safely` / `failed_unsafely`).
    NotVerified { surface: Surface, detail: String },
    /// The surface was not attached / the observable is unavailable — the
    /// claim cannot be proven and must not be assumed (EV1 `unverifiable`).
    Unverifiable { surface: Surface, reason: String },
}

impl SurfaceVerdict {
    /// The EV1 `CompletionStatus::label()` this verdict folds into.
    pub fn status_label(&self) -> &'static str {
        match self {
            Self::Verified { .. } => "verified_complete",
            Self::Degraded { .. } => "partially_complete",
            Self::NotVerified { .. } => "failed_safely",
            Self::Unverifiable { .. } => "unverifiable",
        }
    }

    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

/// Lowercase hex encode (std hex isn't stable; tiny local helper, same as
/// the verifier module).
fn encode_lower(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(TABLE[(b >> 4) as usize] as char);
        out.push(TABLE[(b & 0xf) as usize] as char);
    }
    out
}

/// Run one per-surface check against the observed context. Missing context
/// ⇒ `Unverifiable` (never a fake pass).
pub fn verify_surface(check: &SurfaceCheck, ctx: &SurfaceContext) -> SurfaceVerdict {
    let surface = check.surface();
    match check {
        SurfaceCheck::FileHash {
            path,
            algorithm,
            expected,
        } => match &ctx.file_bytes {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: format!("{path}: file bytes not observed (surface not attached)"),
            },
            Some(bytes) => {
                use sha2::Digest;
                let hex = match algorithm {
                    super::manifest::HashAlgorithm::Sha256 => {
                        let mut h = sha2::Sha256::new();
                        h.update(bytes);
                        encode_lower(&h.finalize())
                    }
                    super::manifest::HashAlgorithm::Sha1 => {
                        let mut h = sha1::Sha1::new();
                        h.update(bytes);
                        encode_lower(&h.finalize())
                    }
                };
                if hex.eq_ignore_ascii_case(expected) {
                    SurfaceVerdict::Verified {
                        surface,
                        detail: format!("{path}: hash matches"),
                    }
                } else {
                    SurfaceVerdict::NotVerified {
                        surface,
                        detail: format!("{path}: hash {hex} != expected {expected}"),
                    }
                }
            }
        },
        SurfaceCheck::FileContains { path, substring } => match &ctx.file_bytes {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: format!("{path}: file bytes not observed (surface not attached)"),
            },
            Some(bytes) => {
                let text = String::from_utf8_lossy(bytes);
                if text.contains(substring.as_str()) {
                    SurfaceVerdict::Verified {
                        surface,
                        detail: format!("{path}: contains required text"),
                    }
                } else {
                    SurfaceVerdict::NotVerified {
                        surface,
                        detail: format!("{path}: missing required text"),
                    }
                }
            }
        },
        SurfaceCheck::ShellExit { expected } => match ctx.shell_exit {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: "shell exit code not observed (shell surface not attached)".into(),
            },
            Some(code) if code == *expected => SurfaceVerdict::Verified {
                surface,
                detail: format!("exit {code}"),
            },
            Some(code) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("exit {code} != expected {expected}"),
            },
        },
        SurfaceCheck::GitClean => match ctx.git_clean {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: "git tree state not observed (git surface not attached)".into(),
            },
            Some(true) => SurfaceVerdict::Verified {
                surface,
                detail: "working tree clean".into(),
            },
            Some(false) => SurfaceVerdict::NotVerified {
                surface,
                detail: "working tree dirty after effect".into(),
            },
        },
        SurfaceCheck::GitHead { commit } => match &ctx.git_head {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: "git head not observed (git surface not attached)".into(),
            },
            Some(head) if head == commit => SurfaceVerdict::Verified {
                surface,
                detail: format!("head at {commit}"),
            },
            Some(head) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("head {head} != expected {commit}"),
            },
        },
        SurfaceCheck::OfficeCell {
            path: _,
            sheet,
            address,
            expected,
        } => match &ctx.office_cell {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: format!(
                    "{sheet}!{address}: cell value not observed (office surface not attached)"
                ),
            },
            Some(v) if v == expected => SurfaceVerdict::Verified {
                surface,
                detail: format!("{sheet}!{address} = {v}"),
            },
            Some(v) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("{sheet}!{address} = {v:?} != expected {expected:?}"),
            },
        },
        SurfaceCheck::BrowserUrl { prefix } => match &ctx.browser_url {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: "browser URL not observed (browser surface not attached)".into(),
            },
            Some(url) if url.starts_with(prefix.as_str()) => SurfaceVerdict::Verified {
                surface,
                detail: format!("url {url} matches {prefix}…"),
            },
            Some(url) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("url {url} does not match {prefix}…"),
            },
        },
        SurfaceCheck::DesktopElement { ref_id } => match ctx.desktop_element_visible {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: format!(
                    "element {ref_id}: visibility not observed (desktop surface not attached)"
                ),
            },
            Some(true) => SurfaceVerdict::Verified {
                surface,
                detail: format!("element {ref_id} visible"),
            },
            Some(false) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("element {ref_id} not visible after action"),
            },
        },
        SurfaceCheck::SearchHits { min_hits } => match ctx.search_hits {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: "search hit count not observed (search surface not attached)".into(),
            },
            Some(hits) if hits >= *min_hits => SurfaceVerdict::Verified {
                surface,
                detail: format!("{hits} hits ≥ {min_hits}"),
            },
            Some(hits) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("{hits} hits < {min_hits}"),
            },
        },
        SurfaceCheck::NetworkStatus { expected_class } => match ctx.network_status_class {
            None => SurfaceVerdict::Unverifiable {
                surface,
                reason: "network status class not observed (network surface not attached)".into(),
            },
            Some(class) if class == *expected_class => SurfaceVerdict::Verified {
                surface,
                detail: format!("status {class}xx"),
            },
            Some(class) => SurfaceVerdict::NotVerified {
                surface,
                detail: format!("status {class}xx != expected {expected_class}xx"),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest::HashAlgorithm;
    use super::*;

    fn file_check() -> SurfaceCheck {
        SurfaceCheck::FileHash {
            path: "out.txt".into(),
            algorithm: HashAlgorithm::Sha256,
            expected: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(), // sha256("")
        }
    }

    #[test]
    fn missing_context_is_unverifiable_not_a_pass() {
        let v = verify_surface(&file_check(), &SurfaceContext::default());
        assert!(matches!(v, SurfaceVerdict::Unverifiable { .. }));
        assert_eq!(v.status_label(), "unverifiable");
        assert!(!v.is_verified());

        let v = verify_surface(
            &SurfaceCheck::ShellExit { expected: 0 },
            &SurfaceContext::default(),
        );
        assert!(matches!(v, SurfaceVerdict::Unverifiable { .. }));
    }

    #[test]
    fn file_hash_verifies_and_fails_honestly() {
        let mut ctx = SurfaceContext {
            file_bytes: Some(Vec::new()), // sha256("")
            ..Default::default()
        };
        let v = verify_surface(&file_check(), &ctx);
        assert!(matches!(v, SurfaceVerdict::Verified { .. }));
        assert_eq!(v.status_label(), "verified_complete");

        ctx.file_bytes = Some(b"tampered".to_vec());
        let v = verify_surface(&file_check(), &ctx);
        assert!(matches!(v, SurfaceVerdict::NotVerified { .. }));
    }

    #[test]
    fn shell_git_browser_office_verify() {
        // Shell exit code.
        let mut ctx = SurfaceContext {
            shell_exit: Some(0),
            ..Default::default()
        };
        assert!(verify_surface(&SurfaceCheck::ShellExit { expected: 0 }, &ctx).is_verified());
        ctx.shell_exit = Some(2);
        assert!(matches!(
            verify_surface(&SurfaceCheck::ShellExit { expected: 0 }, &ctx),
            SurfaceVerdict::NotVerified { .. }
        ));

        // Git clean.
        ctx.git_clean = Some(true);
        assert!(verify_surface(&SurfaceCheck::GitClean, &ctx).is_verified());
        ctx.git_clean = Some(false);
        assert!(matches!(
            verify_surface(&SurfaceCheck::GitClean, &ctx),
            SurfaceVerdict::NotVerified { .. }
        ));

        // Browser URL prefix.
        ctx.browser_url = Some("https://example.com/docs".into());
        assert!(
            verify_surface(
                &SurfaceCheck::BrowserUrl {
                    prefix: "https://example.com".into(),
                },
                &ctx,
            )
            .is_verified()
        );
        assert!(matches!(
            verify_surface(
                &SurfaceCheck::BrowserUrl {
                    prefix: "https://evil.example".into(),
                },
                &ctx,
            ),
            SurfaceVerdict::NotVerified { .. }
        ));

        // Office cell value.
        ctx.office_cell = Some("42".into());
        assert!(
            verify_surface(
                &SurfaceCheck::OfficeCell {
                    path: "w.xlsx".into(),
                    sheet: "Sheet1".into(),
                    address: "B4".into(),
                    expected: "42".into(),
                },
                &ctx,
            )
            .is_verified()
        );
    }

    #[test]
    fn per_surface_status_folds_into_ev1_taxonomy() {
        let ctx = SurfaceContext {
            search_hits: Some(7),
            ..Default::default()
        };
        let v = verify_surface(&SurfaceCheck::SearchHits { min_hits: 5 }, &ctx);
        assert_eq!(v.status_label(), "verified_complete");
        assert!(v.is_verified());
        let v = verify_surface(&SurfaceCheck::SearchHits { min_hits: 9 }, &ctx);
        assert_eq!(v.status_label(), "failed_safely");
    }
}
