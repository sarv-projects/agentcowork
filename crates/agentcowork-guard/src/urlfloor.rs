//! P7.4 — URL floors. `file://` URLs are only allowed inside the granted
//! roots; other schemes are gated per-policy (http/https allowed by default,
//! everything else requires an explicit allow). Deterministic: the same URL +
//! roots always yields the same verdict.
//!
//! **P62.1:** an `http(s)` URL is additionally checked against the network
//! destination floor ([`crate::netfloor`]) — cloud-metadata/link-local,
//! multicast, unspecified and reserved space are always refused, and private
//! / CGNAT / ULA / discovery names are refused unless the policy opts in.
//! The check is pure and synchronous (no DNS); see [`NetPolicy`].

use crate::netfloor::{self, NetPolicy};

/// Schemes that are always allowed (network reads with no local side effect).
const ALWAYS_ALLOWED: &[&str] = &["http", "https"];

/// Schemes that are always refused (local side effects / opaque).
const NEVER_ALLOWED: &[&str] = &[
    "file",
    "javascript",
    "data",
    "about",
    "vbscript",
    "smb",
    "nfs",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlVerdict {
    Allowed,
    /// Scheme not in the allow set.
    SchemeBlocked,
    /// `file://` outside the granted roots.
    OutsideRoots,
    /// Malformed URL / no scheme.
    Malformed,
    /// P62.1 — a network destination the [`NetPolicy`] refuses (loopback,
    /// private, link-local/metadata, CGNAT, local-only name, or a hard-floor
    /// class that no policy can allow).
    PrivateDestination,
}

impl UrlVerdict {
    pub fn is_allowed(self) -> bool {
        self == UrlVerdict::Allowed
    }
}

/// Decide whether `url` may be fetched given the granted `roots`
/// (absolute canonical paths). `file://` is only allowed when the decoded
/// path is inside one of the roots. Uses [`NetPolicy::default()`] for the
/// network destination floor.
pub fn check_url(url: &str, roots: &[&str]) -> UrlVerdict {
    check_url_with_policy(url, roots, NetPolicy::default())
}

/// As [`check_url`], but refusing loopback too — for destinations that came
/// from untrusted content (page text, search results, remote tool metadata).
pub fn check_url_strict(url: &str, roots: &[&str]) -> UrlVerdict {
    check_url_with_policy(url, roots, NetPolicy::strict())
}

/// The full check: scheme floor + `file://` root containment + the network
/// destination floor under `policy`. Pure and synchronous.
pub fn check_url_with_policy(url: &str, roots: &[&str], policy: NetPolicy) -> UrlVerdict {
    let Ok(parsed) = url::Url::parse(url) else {
        return UrlVerdict::Malformed;
    };
    let scheme = parsed.scheme();
    if NEVER_ALLOWED.contains(&scheme) {
        if scheme == "file" {
            let Ok(path) = parsed.to_file_path() else {
                return UrlVerdict::Malformed;
            };
            let path_str = path.to_string_lossy();
            let canonical = crate::pathfloor::canonicalize_no_follow(&path_str);
            let inside = roots.iter().any(|r| {
                let root = crate::pathfloor::canonicalize_no_follow(r);
                canonical == root
                    || canonical.starts_with(&format!("{}/", root.trim_end_matches('/')))
            });
            return if inside {
                UrlVerdict::Allowed
            } else {
                UrlVerdict::OutsideRoots
            };
        }
        return UrlVerdict::SchemeBlocked;
    }
    if ALWAYS_ALLOWED.contains(&scheme) {
        // The parsed host is already normalized by the URL parser, so the
        // decimal / octal / hex and IPv4-mapped bypass forms are classified
        // correctly without any string tricks of our own.
        let Some(host) = parsed.host() else {
            return UrlVerdict::Malformed;
        };
        return if policy.allows(netfloor::classify_url_host(&host)) {
            UrlVerdict::Allowed
        } else {
            UrlVerdict::PrivateDestination
        };
    }
    UrlVerdict::SchemeBlocked
}

/// Is this URL fetchable under the given roots?
pub fn is_allowed(url: &str, roots: &[&str]) -> bool {
    check_url(url, roots).is_allowed()
}

/// Why a destination was refused, as a stable token for the audit row / card
/// (`None` when it is allowed). Never includes the raw host.
pub fn block_reason(url: &str, policy: NetPolicy) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host()?;
    let class = netfloor::classify_url_host(&host);
    if policy.allows(class) {
        None
    } else {
        Some(netfloor::class_reason(class))
    }
}

/// Adversarial URL corpus for the S0.7 fuzz gate (scheme smuggling,
/// file-exfil, javascript, data, UNC).
pub fn adversarial_urls() -> Vec<&'static str> {
    vec![
        "javascript:alert(1)",
        "data:text/html,pwn",
        "file:///etc/passwd",
        "file:///etc/shadow",
        "file://../../etc/passwd",
        "about:blank",
        "vbscript:msgbox",
        "smb://evil/share",
        "nfs://evil/export",
        "file:///workspace/../../etc/passwd",
        "javascript://https://example.com/%0aalert(1)",
        "data:application/javascript,fetch('http://evil')",
        "file:///C:/Windows/System32/config/SAM",
        "file:////etc/passwd",
        // P62.1 — SSRF destination corpus (cloud metadata, RFC1918, loopback,
        // CGNAT, ULA, local-only names, and the numeric bypass forms).
        "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
        "http://169.254.170.2/v2/credentials",
        "http://192.168.1.1/admin",
        "http://10.0.0.1/api",
        "http://172.16.0.5/",
        "http://127.0.0.1:6379/",
        "http://localhost:2375/containers/json",
        "http://[::1]:9200/_cat/indices",
        "http://[fd00::1]/",
        "http://[fe80::1%25eth0]/",
        "http://100.64.0.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://0177.0.0.1/",
        "http://metadata.google.internal/computeMetadata/v1/",
        "http://0.0.0.0/",
        "http://nas.local/",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_always_allowed() {
        assert_eq!(check_url("https://example.com/x", &[]), UrlVerdict::Allowed);
        assert_eq!(check_url("http://example.com", &[]), UrlVerdict::Allowed);
    }

    #[test]
    fn file_inside_roots_allowed() {
        assert_eq!(
            check_url("file:///workspace/a.txt", &["/workspace"]),
            UrlVerdict::Allowed
        );
        assert_eq!(
            check_url("file:///workspace/sub/b.txt", &["/workspace"]),
            UrlVerdict::Allowed
        );
    }

    #[test]
    fn file_outside_roots_blocked() {
        assert_eq!(
            check_url("file:///etc/passwd", &["/workspace"]),
            UrlVerdict::OutsideRoots
        );
        assert_eq!(
            check_url("file:///workspace2/x", &["/workspace"]),
            UrlVerdict::OutsideRoots
        );
    }

    #[test]
    fn dangerous_schemes_blocked() {
        assert_eq!(
            check_url("javascript:alert(1)", &[]),
            UrlVerdict::SchemeBlocked
        );
        assert_eq!(
            check_url("data:text/html,<script>", &[]),
            UrlVerdict::SchemeBlocked
        );
        assert_eq!(
            check_url("smb://host/share", &[]),
            UrlVerdict::SchemeBlocked
        );
    }

    #[test]
    fn malformed_blocked() {
        assert_eq!(check_url("not a url", &[]), UrlVerdict::Malformed);
        assert_eq!(check_url("", &[]), UrlVerdict::Malformed);
    }

    #[test]
    fn cloud_metadata_blocked_under_every_policy() {
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.170.2/v2/credentials",
        ] {
            for policy in [
                NetPolicy::strict(),
                NetPolicy::default(),
                NetPolicy::local(),
            ] {
                assert_eq!(
                    check_url_with_policy(url, &[], policy),
                    UrlVerdict::PrivateDestination,
                    "{url}"
                );
            }
        }
    }

    #[test]
    fn lan_is_opt_in_and_loopback_is_the_desktop_default() {
        // A LAN address is refused by default and under strict, allowed after
        // the explicit local-network opt-in.
        assert_eq!(
            check_url("http://192.168.1.1/admin", &[]),
            UrlVerdict::PrivateDestination
        );
        assert_eq!(
            check_url_with_policy("http://192.168.1.1/admin", &[], NetPolicy::local()),
            UrlVerdict::Allowed
        );
        // Loopback is reachable on a local desktop (local dev servers, Ollama,
        // CDP) but refused for untrusted destinations.
        assert_eq!(
            check_url("http://127.0.0.1:11434/v1", &[]),
            UrlVerdict::Allowed
        );
        assert_eq!(
            check_url_strict("http://127.0.0.1:11434/v1", &[]),
            UrlVerdict::PrivateDestination
        );
    }

    #[test]
    fn numeric_bypass_forms_blocked() {
        for url in [
            "http://2130706433/",
            "http://0x7f000001/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:192.168.1.1]/",
        ] {
            assert_eq!(
                check_url_with_policy(url, &[], NetPolicy::strict()),
                UrlVerdict::PrivateDestination,
                "{url}"
            );
        }
    }

    #[test]
    fn public_http_still_allowed() {
        assert_eq!(
            check_url("https://api.openai.com/v1/models", &[]),
            UrlVerdict::Allowed
        );
        assert!(block_reason("https://example.com", NetPolicy::strict()).is_none());
    }

    #[test]
    fn every_adversarial_url_is_refused_under_the_strict_policy() {
        for url in adversarial_urls() {
            assert!(
                !check_url_strict(url, &["/workspace"]).is_allowed(),
                "{url} must not pass the strict floor"
            );
        }
    }

    /// The default policy deliberately allows loopback (documented decision —
    /// local dev servers, local model runtimes, CDP). Pinned here so the
    /// choice is explicit rather than accidental, and so a future change to it
    /// fails loudly.
    #[test]
    fn loopback_is_allowed_by_default_but_gated_for_untrusted_content() {
        assert_eq!(
            check_url("http://127.0.0.1:6379/", &[]),
            UrlVerdict::Allowed
        );
        assert_eq!(
            check_url_strict("http://127.0.0.1:6379/", &[]),
            UrlVerdict::PrivateDestination
        );
        // A LAN address is never in the default allow set, so the SSRF prizes
        // outside the machine stay closed without an opt-in.
        assert_eq!(
            check_url("http://192.168.1.1/", &[]),
            UrlVerdict::PrivateDestination
        );
    }
}
