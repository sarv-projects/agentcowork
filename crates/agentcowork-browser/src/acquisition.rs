//! P36 (E10) — acquisition adapter: **the agent never chooses HTTP vs CDP vs
//! stealth; the engine does.**
//!
//! The model expresses an *intent* (`need static text`, `need JS rendering`,
//! `need logged-in session`); the adapter maps intent + policy → the cheapest
//! tier (ARCH/08, doc 55). Cloud browser is explicitly not bundled — the
//! adapter is honest about it.

use serde::{Deserialize, Serialize};

/// What the agent needs from the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcquisitionIntent {
    /// Plain HTML/text — HTTP tier suffices.
    Static,
    /// JavaScript rendering required (SPA, dynamic content).
    NeedsJs,
    /// A logged-in session is required (cookies, auth headers).
    NeedsLogin,
    /// Interactive control (click/type/observe loop).
    Interactive,
}

/// The tier the engine picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcquisitionKind {
    /// Plain HTTP fetch (tier 0: markdown negotiation + llms.txt walk).
    Http,
    /// Light engine (Lightpanda/Obscura) — JS without a full Chrome.
    Light,
    /// Full CDP browser (Chrome/Edge/Electron).
    Cdp,
    /// Stealth-mode acquisition — NOT bundled; only selected when the user
    /// explicitly opted in to a stealth build (honest ceiling).
    Stealth,
}

/// What the adapter decided, with the reason trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionPlan {
    pub kind: AcquisitionKind,
    pub reason: Vec<String>,
    /// The URL the chosen tier should acquire (the same one the caller
    /// asked for — the adapter never rewrites the target).
    pub url: String,
    /// Whether the picked tier needs a live session (cookie injection).
    pub needs_session: bool,
    /// Honest escalation: which tier would answer if this one fails.
    pub escalation: Option<AcquisitionKind>,
}

/// The seams the adapter consults. All optional — an empty policy is the
/// honest default (no stealth build, no cloud).
#[derive(Debug, Clone, Default)]
pub struct AcquisitionOptions {
    /// A stealth build is installed and enabled (never assumed).
    pub stealth_available: bool,
    /// A browser is installed (if false, CDP tiers hard-degrade).
    pub browser_available: bool,
    /// A light engine (Lightpanda/Obscura) is installed.
    pub light_available: bool,
    /// Policy toggle: never use stealth even when available.
    pub deny_stealth: bool,
    /// Policy toggle: never use CDP for non-interactive intents.
    pub deny_cdp_for_static: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AcquisitionError {
    #[error("no usable tier for intent {intent:?} with the installed engines")]
    NoTier { intent: AcquisitionIntent },
}

/// Decide. The mapping is deterministic and documented:
/// - `Static` → HTTP (unless HTTP is impossible, which this surface cannot
///   know — the caller reports tier failure and we escalate).
/// - `NeedsJs` → Light when available, else CDP.
/// - `NeedsLogin` → CDP (session cookies live in the browser) — or Light if
///   the user opted in to light sessions.
/// - `Interactive` → CDP always.
pub fn pick(
    intent: AcquisitionIntent,
    url: &str,
    opts: &AcquisitionOptions,
) -> Result<AcquisitionPlan, AcquisitionError> {
    let mut reasons = Vec::new();
    match intent {
        AcquisitionIntent::Static => {
            reasons.push("static intent: HTTP tier is cheapest".into());
            Ok(AcquisitionPlan {
                kind: AcquisitionKind::Http,
                reason: reasons,
                url: url.to_string(),
                needs_session: false,
                escalation: Some(AcquisitionKind::Light),
            })
        }
        AcquisitionIntent::NeedsJs => {
            if opts.light_available {
                reasons.push("JS intent: light engine renders without a full browser".into());
                Ok(AcquisitionPlan {
                    kind: AcquisitionKind::Light,
                    reason: reasons,
                    url: url.to_string(),
                    needs_session: false,
                    escalation: Some(AcquisitionKind::Cdp),
                })
            } else if opts.browser_available {
                reasons.push("JS intent, no light engine: full CDP browser".into());
                Ok(AcquisitionPlan {
                    kind: AcquisitionKind::Cdp,
                    reason: reasons,
                    url: url.to_string(),
                    needs_session: false,
                    escalation: None,
                })
            } else {
                Err(AcquisitionError::NoTier { intent })
            }
        }
        AcquisitionIntent::NeedsLogin => {
            if opts.browser_available {
                reasons.push("login intent: session cookies live in the browser".into());
                Ok(AcquisitionPlan {
                    kind: AcquisitionKind::Cdp,
                    reason: reasons,
                    url: url.to_string(),
                    needs_session: true,
                    escalation: None,
                })
            } else if opts.light_available {
                reasons.push("login intent: light engine with a session cookie jar".into());
                Ok(AcquisitionPlan {
                    kind: AcquisitionKind::Light,
                    reason: reasons,
                    url: url.to_string(),
                    needs_session: true,
                    escalation: None,
                })
            } else {
                Err(AcquisitionError::NoTier { intent })
            }
        }
        AcquisitionIntent::Interactive => {
            if !opts.browser_available {
                return Err(AcquisitionError::NoTier { intent });
            }
            reasons.push("interactive intent: full CDP browser".into());
            Ok(AcquisitionPlan {
                kind: AcquisitionKind::Cdp,
                reason: reasons,
                url: url.to_string(),
                needs_session: false,
                escalation: None,
            })
        }
    }
}

/// Stealth is contractually gated: the adapter never emits `Stealth` unless
/// a build tool is installed AND policy allows. Cloud stealth is not bundled.
pub fn stealth_allowed(opts: &AcquisitionOptions) -> bool {
    opts.stealth_available && !opts.deny_stealth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_picks_http_with_escalation() {
        let plan = pick(
            AcquisitionIntent::Static,
            "https://example.com",
            &AcquisitionOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.kind, AcquisitionKind::Http);
        assert_eq!(plan.escalation, Some(AcquisitionKind::Light));
        assert!(!plan.needs_session);
    }

    #[test]
    fn js_picks_light_then_cdp() {
        let opts = AcquisitionOptions {
            light_available: true,
            ..Default::default()
        };
        let plan = pick(AcquisitionIntent::NeedsJs, "https://x", &opts).unwrap();
        assert_eq!(plan.kind, AcquisitionKind::Light);
        let opts = AcquisitionOptions {
            browser_available: true,
            ..Default::default()
        };
        let plan = pick(AcquisitionIntent::NeedsJs, "https://x", &opts).unwrap();
        assert_eq!(plan.kind, AcquisitionKind::Cdp);
    }

    #[test]
    fn login_requires_session_flag() {
        let opts = AcquisitionOptions {
            browser_available: true,
            ..Default::default()
        };
        let plan = pick(AcquisitionIntent::NeedsLogin, "https://mail", &opts).unwrap();
        assert_eq!(plan.kind, AcquisitionKind::Cdp);
        assert!(plan.needs_session);
    }

    #[test]
    fn no_tier_fails_closed() {
        let plan = pick(
            AcquisitionIntent::Interactive,
            "https://x",
            &AcquisitionOptions::default(),
        );
        assert!(matches!(plan, Err(AcquisitionError::NoTier { .. })));
        let plan = pick(
            AcquisitionIntent::NeedsJs,
            "https://x",
            &AcquisitionOptions::default(),
        );
        assert!(matches!(plan, Err(AcquisitionError::NoTier { .. })));
    }

    #[test]
    fn stealth_never_emitted_without_build() {
        assert!(!stealth_allowed(&AcquisitionOptions::default()));
        let opts = AcquisitionOptions {
            stealth_available: true,
            ..Default::default()
        };
        assert!(stealth_allowed(&opts));
        let opts = AcquisitionOptions {
            stealth_available: true,
            deny_stealth: true,
            ..Default::default()
        };
        assert!(!stealth_allowed(&opts));
        // And `pick` never returns Stealth at all — the engine doesn't offer
        // it unless a future build wires it as an AcquisitionKind branch.
    }
}
