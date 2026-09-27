//! G9 read-cleaner (doc 64 §4 — brave adblock-rust pattern): a pre-`read` /
//! `snapshot` / markdown-export transform that strips ads, trackers, and
//! consent-walls before page text reaches the model (ads are wasted tokens).
//!
//! A self-contained Adblock-Plus-style filter engine over the subset EasyList
//! actually uses most: `||domain^` domain-anchored block rules, `||domain/path^`
//! path-anchored rules, `@@` exception rules, `/regex/` rules, and `##selector`
//! / `#@#selector` cosmetic (element-hiding) rules. The full brave `adblock`
//! crate (v0.13.0 MIT, compiled-engine cache, CSP directives, request-type /
//! third-party options) is the documented swap-in — this is the deterministic
//! core with no dependency weight.

/// Block vs. exception (allowlist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Block,
    Exception,
}

/// The match pattern of one network filter rule.
#[derive(Debug, Clone)]
enum Pattern {
    /// `||example.com^` — anchored to the host (or a subdomain of it).
    DomainAnchor(String),
    /// `||example.com/path^` — host anchor + URL path prefix.
    DomainPath(String, String),
    /// A bare substring matched against the full URL.
    Substring(String),
    /// `/regex/` — a regular expression against the URL.
    Regex(String),
}

impl Pattern {
    fn matches(&self, url: &str, host: &str, path: &str) -> bool {
        match self {
            Pattern::DomainAnchor(d) => host == d || host.ends_with(&format!(".{d}")),
            Pattern::DomainPath(d, p) => {
                (host == d || host.ends_with(&format!(".{d}"))) && path.starts_with(p.as_str())
            }
            Pattern::Substring(s) => url.contains(s.as_str()),
            Pattern::Regex(r) => regex::Regex::new(r)
                .map(|re| re.is_match(url))
                .unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone)]
struct FilterRule {
    kind: RuleKind,
    pattern: Pattern,
}

/// A cosmetic (element-hiding) rule: optional domain restriction + selector.
#[derive(Debug, Clone)]
struct CosmeticRule {
    /// Restricting domains (empty = all domains).
    domains: Vec<String>,
    selector: String,
    exception: bool,
}

/// A parsed filter list (EasyList-lite subset).
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    rules: Vec<FilterRule>,
    cosmetic: Vec<CosmeticRule>,
}

impl FilterSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one filter line (ABP syntax subset). Unknown/malformed lines are
    /// ignored (never fail the whole list).
    pub fn add_rule(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
            return;
        }
        if let Some(cosmetic) = parse_cosmetic(line) {
            self.cosmetic.push(cosmetic);
            return;
        }
        if let Some(rule) = parse_network(line) {
            self.rules.push(rule);
        }
    }

    /// Add a whole filter list (one rule per line).
    pub fn add_filter_list(&mut self, text: &str) {
        for line in text.lines() {
            self.add_rule(line);
        }
    }

    /// Is `url` blocked? Exceptions (allowlist) win over block rules.
    pub fn is_blocked(&self, url: &str) -> bool {
        let (host, path) = split_url(url);
        for r in &self.rules {
            if r.kind == RuleKind::Exception && r.pattern.matches(url, &host, &path) {
                return false;
            }
        }
        self.rules
            .iter()
            .any(|r| r.kind == RuleKind::Block && r.pattern.matches(url, &host, &path))
    }

    /// Element-hiding selectors active for `url` (for DOM cleanup).
    pub fn cosmetic_selectors(&self, url: &str) -> Vec<String> {
        let (host, _) = split_url(url);
        let mut out = Vec::new();
        for c in &self.cosmetic {
            if c.exception {
                continue;
            }
            if c.domains.is_empty() || c.domains.iter().any(|d| host_matches(&host, d)) {
                out.push(c.selector.clone());
            }
        }
        out
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn cosmetic_count(&self) -> usize {
        self.cosmetic.len()
    }
}

/// The result of cleaning a page's markdown.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanedText {
    pub text: String,
    pub removed_links: usize,
    pub removed_lines: usize,
}

/// Consent/ad-spam line markers (case-insensitive) stripped from markdown.
const SPAM_MARKERS: &[&str] = &[
    "sponsored",
    "advertisement",
    "cookie consent",
    "accept all cookies",
    "we value your privacy",
    "sign up for our newsletter",
    "adblock",
    "promoted",
];

/// A minimal bundled filter list (the deterministic default when no user list
/// is loaded). Covers the highest-volume trackers; the full brave `adblock`
/// crate + EasyList are the documented swap-in (doc 64 §4).
const DEFAULT_FILTER_LIST: &str = "\
||doubleclick.net^\n\
||google-analytics.com^\n\
||googletagmanager.com^\n\
||googlesyndication.com^\n\
||facebook.com/tr^\n\
||amazon-adsystem.com^\n\
||scorecardresearch.com^\n\
||taboola.com^\n\
||outbrain.com^\n\
||quantserve.com^\n";

/// Build the deterministic default [`FilterSet`] (the G9 read-cleaner's
/// fallback when no user-supplied EasyList is loaded).
pub fn default_filter_set() -> FilterSet {
    let mut f = FilterSet::new();
    f.add_filter_list(DEFAULT_FILTER_LIST);
    f
}

/// Clean markdown by: (1) dropping `[text](url)` / `![alt](url)` links whose
/// URL is blocked, (2) dropping lines that are pure consent/ad spam (including
/// lines that reduce to nothing after their ads are stripped). Returns the
/// cleaned text + what was removed.
pub fn clean_markdown(filters: &FilterSet, page_url: &str, markdown: &str) -> CleanedText {
    let link_re = regex::Regex::new(r"!?\[[^\]]*\]\(([^)\s]+)\)").unwrap();
    let mut removed_links = 0usize;
    let mut removed_lines = 0usize;
    let mut kept: Vec<String> = Vec::new();

    for line in markdown.lines() {
        let stripped = link_re.replace_all(line, |caps: &regex::Captures| {
            let target = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if filters.is_blocked(target) {
                removed_links += 1;
                String::new()
            } else {
                caps.get(0).unwrap().as_str().to_string()
            }
        });
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            // A non-empty source line that reduced to nothing was all ads;
            // a genuinely blank source line is preserved (paragraph breaks).
            if !line.trim().is_empty() {
                removed_lines += 1;
                continue;
            }
            kept.push(String::new());
            continue;
        }
        let lower = trimmed.to_lowercase();
        let is_spam = SPAM_MARKERS.iter().any(|m| lower.contains(m))
            // A bare blocked URL on its own line.
            || (trimmed.starts_with("http") && filters.is_blocked(trimmed))
            // A blocked-host image embed that the link regex left behind.
            || (trimmed.starts_with('!') && filters.is_blocked(page_url));
        if is_spam {
            removed_lines += 1;
            continue;
        }
        kept.push(stripped.to_string());
    }

    CleanedText {
        text: kept.join("\n"),
        removed_links,
        removed_lines,
    }
}

/// Split a URL into `(host, path)` (both empty when unparseable).
fn split_url(url: &str) -> (String, String) {
    match url::Url::parse(url) {
        Ok(u) => (
            u.host_str().unwrap_or_default().to_string(),
            u.path().to_string(),
        ),
        Err(_) => (String::new(), String::new()),
    }
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// Parse a network (URL) filter line → a [`FilterRule`], or None.
fn parse_network(line: &str) -> Option<FilterRule> {
    let mut s = line.to_string();
    let kind = if let Some(rest) = s.strip_prefix("@@") {
        s = rest.to_string();
        RuleKind::Exception
    } else {
        RuleKind::Block
    };
    // Strip options (everything from the first `$`, e.g. `$third-party`).
    if let Some(dollar) = s.find('$') {
        s.truncate(dollar);
    }
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let pattern = if let Some(body) = s.strip_prefix("||") {
        // Domain anchor with an optional path: `||host/path^`.
        let domain: String = body
            .chars()
            .take_while(|c| *c != '/' && *c != '^')
            .collect();
        if domain.is_empty() {
            return None;
        }
        let rest = &body[domain.len()..];
        let path: String = rest
            .trim_start_matches('/')
            .chars()
            .take_while(|c| *c != '^')
            .collect();
        if path.is_empty() {
            Pattern::DomainAnchor(domain)
        } else {
            Pattern::DomainPath(domain, format!("/{path}"))
        }
    } else if s.starts_with('/') && s.ends_with('/') && s.len() > 2 {
        let inner = &s[1..s.len() - 1];
        Pattern::Regex(inner.to_string())
    } else {
        Pattern::Substring(s.to_string())
    };
    Some(FilterRule { kind, pattern })
}

/// Parse an element-hiding (cosmetic) rule → [`CosmeticRule`], or None.
fn parse_cosmetic(line: &str) -> Option<CosmeticRule> {
    for marker in ["#@#", "#?#", "##"] {
        if let Some(pos) = line.find(marker) {
            let (domain_part, selector) = line.split_at(pos);
            let selector = &selector[marker.len()..];
            if selector.is_empty() {
                return None;
            }
            let domains: Vec<String> = domain_part
                .split(',')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .collect();
            return Some(CosmeticRule {
                domains,
                selector: selector.to_string(),
                exception: marker == "#@#",
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn easy_list_lite() -> FilterSet {
        let mut f = FilterSet::new();
        f.add_filter_list(
            "! EasyList lite\n\
             [Adblock Plus 2.0]\n\
             ||doubleclick.net^\n\
             ||google-analytics.com^\n\
             ||example-ads.com^\n\
             ||tracker.io/pixel^\n\
             @@||example-ads.com/allow-this^\n\
             /adserver\\.[a-z]+/\n\
             example.com##.ad-banner\n\
             example.com,other.com##.sponsored\n\
             #@#.keep-me\n",
        );
        f
    }

    #[test]
    fn blocks_domain_anchored_ads() {
        let f = easy_list_lite();
        assert!(f.is_blocked("https://doubleclick.net/pixel?id=1"));
        assert!(f.is_blocked("https://sub.doubleclick.net/route"));
        assert!(f.is_blocked("https://www.google-analytics.com/collect"));
    }

    #[test]
    fn domain_anchor_does_not_overblock() {
        let f = easy_list_lite();
        assert!(!f.is_blocked("https://notdoubleclick.net/"));
        assert!(!f.is_blocked("https://example.com/legit"));
    }

    #[test]
    fn path_anchor_matches_only_the_path() {
        let f = easy_list_lite();
        // `||tracker.io/pixel^` blocks the pixel path, not the whole domain.
        assert!(f.is_blocked("https://tracker.io/pixel"));
        assert!(f.is_blocked("https://tracker.io/pixel/extra"));
        assert!(!f.is_blocked("https://tracker.io/legit-page"));
    }

    #[test]
    fn exceptions_win() {
        let f = easy_list_lite();
        assert!(f.is_blocked("https://example-ads.com/banner"));
        assert!(!f.is_blocked("https://example-ads.com/allow-this"));
    }

    #[test]
    fn regex_rules_match() {
        let f = easy_list_lite();
        assert!(f.is_blocked("https://cdn.adserver.xyz/impress"));
        assert!(!f.is_blocked("https://example.com/page"));
    }

    #[test]
    fn cosmetic_selectors_respect_domain() {
        let f = easy_list_lite();
        let sel = f.cosmetic_selectors("https://example.com/article");
        assert!(sel.contains(&".ad-banner".to_string()));
        assert!(sel.contains(&".sponsored".to_string()));
        let sel_other = f.cosmetic_selectors("https://unrelated.com/");
        assert!(!sel_other.contains(&".sponsored".to_string()));
        assert!(!sel.contains(&".keep-me".to_string()));
    }

    #[test]
    fn clean_markdown_strips_ads_and_consent() {
        let f = easy_list_lite();
        let md = "# Hello\n\
                  Read [our story](https://example.com/story) here.\n\
                  [Ad](https://doubleclick.net/click)\n\
                  ![tracker](https://tracker.io/pixel)\n\
                  We value your privacy. Accept all cookies.\n\
                  Normal content line.\n";
        let cleaned = clean_markdown(&f, "https://example.com", md);
        assert!(
            cleaned
                .text
                .contains("Read [our story](https://example.com/story)")
        );
        assert!(!cleaned.text.contains("doubleclick.net"));
        assert!(!cleaned.text.contains("tracker.io"));
        assert!(!cleaned.text.contains("Accept all cookies"));
        assert!(cleaned.text.contains("Normal content line"));
        assert_eq!(cleaned.removed_links, 2);
        // Ad-only line + image line + consent line = 3 removed lines.
        assert_eq!(cleaned.removed_lines, 3);
    }

    #[test]
    fn empty_or_comment_lines_ignored() {
        let mut f = FilterSet::new();
        f.add_rule("");
        f.add_rule("! a comment");
        f.add_rule("[Adblock Plus 2.0]");
        assert_eq!(f.rule_count(), 0);
    }
}
