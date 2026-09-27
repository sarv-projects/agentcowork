//! P51.16 — tool allow-list approval policy.
//!
//! A deterministic allow-list over tool calls: each rule pairs a
//! [`ToolPattern`] (tool name + optional arg glob) with an [`Approval`].
//! Matching is case-insensitive with `*` / `?` wildcards. Rules are ordered
//! and last-match-wins, except an explicit [`Approval::Deny`] always wins
//! over [`Approval::Allow`] (deny-wins, like the capability granter).
//! Unknown tools default to [`Approval::Ask`] (fail-closed).

/// What the policy decides for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Run without a human ticket (still subject to Guard-1).
    Allow,
    /// Mint a Guard-2 ticket and wait for approval.
    Ask,
    /// Refuse outright.
    Deny,
}

/// A tool-name pattern plus an optional argument glob.
///
/// Both halves are case-insensitive; `*` matches any character sequence
/// (including separators) and `?` matches exactly one character. When
/// `args_glob` is `None`, only the tool name must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPattern {
    pub tool: String,
    pub args_glob: Option<String>,
}

impl ToolPattern {
    pub fn new(tool: &str, args_glob: Option<&str>) -> Self {
        Self {
            tool: tool.to_string(),
            args_glob: args_glob.map(|s| s.to_string()),
        }
    }

    /// Does this pattern match a `(tool, args)` call?
    pub fn matches(&self, tool: &str, args: &str) -> bool {
        if !glob_match(&self.tool, tool) {
            return false;
        }
        match &self.args_glob {
            None => true,
            Some(g) => glob_match(g, args),
        }
    }
}

/// Case-insensitive glob match: `*` matches any (possibly empty) sequence,
/// `?` matches exactly one character.
fn glob_match(pattern: &str, value: &str) -> bool {
    fn go(p: &[u8], v: &[u8]) -> bool {
        if p.is_empty() {
            return v.is_empty();
        }
        match p[0] {
            b'*' => {
                // Collapse runs of `*` into one.
                let mut i = 1;
                while i < p.len() && p[i] == b'*' {
                    i += 1;
                }
                for k in 0..=v.len() {
                    if go(&p[i..], &v[k..]) {
                        return true;
                    }
                }
                false
            }
            b'?' => !v.is_empty() && go(&p[1..], &v[1..]),
            c => !v.is_empty() && v[0] == c && go(&p[1..], &v[1..]),
        }
    }
    go(
        pattern.to_lowercase().as_bytes(),
        value.to_lowercase().as_bytes(),
    )
}

/// An ordered allow-list: `rules` are evaluated in order, last match wins,
/// except any matching [`Approval::Deny`] beats [`Approval::Allow`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovalPolicy {
    pub rules: Vec<(ToolPattern, Approval)>,
}

impl ApprovalPolicy {
    pub fn new(rules: Vec<(ToolPattern, Approval)>) -> Self {
        Self { rules }
    }

    /// Evaluate one tool call. Defaults to [`Approval::Ask`] when nothing
    /// matches; an explicit deny always wins over an allow.
    pub fn evaluate(&self, tool: &str, args: &str) -> Approval {
        let mut last: Option<Approval> = None;
        let mut denied = false;
        for (pat, approval) in &self.rules {
            if pat.matches(tool, args) {
                if *approval == Approval::Deny {
                    denied = true;
                }
                last = Some(*approval);
            }
        }
        if denied {
            return Approval::Deny;
        }
        last.unwrap_or(Approval::Ask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_allowlist_exact_match_allows() {
        let policy = ApprovalPolicy::new(vec![(ToolPattern::new("bash", None), Approval::Allow)]);
        assert_eq!(policy.evaluate("bash", "ls -la"), Approval::Allow);
        // Case-insensitive tool match.
        assert_eq!(policy.evaluate("BASH", "ls"), Approval::Allow);
    }

    #[test]
    fn deny_beats_allow_last_match() {
        let policy = ApprovalPolicy::new(vec![
            (ToolPattern::new("bash", None), Approval::Allow),
            (ToolPattern::new("bash", Some("rm -rf *")), Approval::Deny),
        ]);
        // The deny rule is last and matches: deny wins.
        assert_eq!(policy.evaluate("bash", "rm -rf /"), Approval::Deny);
        // Non-matching args still hit the earlier allow.
        assert_eq!(policy.evaluate("bash", "ls -la"), Approval::Allow);
        // Deny always wins even when an allow matches later.
        let inverted = ApprovalPolicy::new(vec![
            (ToolPattern::new("bash", Some("rm -rf *")), Approval::Deny),
            (ToolPattern::new("bash", None), Approval::Allow),
        ]);
        assert_eq!(inverted.evaluate("bash", "rm -rf /"), Approval::Deny);
    }

    #[test]
    fn unknown_tool_defaults_ask() {
        let policy = ApprovalPolicy::new(vec![(ToolPattern::new("bash", None), Approval::Allow)]);
        assert_eq!(policy.evaluate("unknown_tool", "anything"), Approval::Ask);
        assert_eq!(
            ApprovalPolicy::default().evaluate("bash", "ls"),
            Approval::Ask
        );
    }
}
