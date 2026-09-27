//! P7.4 — pre-exec scan of every generated shell string, filesystem path and
//! URL against the compiled blocklist. One [`PreExecScan`] per hit with the
//! exact target text + matched pattern indices, so the decision package can
//! show *what* tripped the guard.

use crate::blocklist::{BlocklistCategory, blocklist_for, category_of};
use regex::RegexSet;
use std::sync::OnceLock;

/// A compiled guard instance reused across scans (compile once).
pub fn guard() -> &'static Guard {
    static GUARD: OnceLock<Guard> = OnceLock::new();
    GUARD.get_or_init(|| Guard::compile(&blocklist_for()).expect("blocklist must compile"))
}

/// Compile an *arbitrary* pattern set into a guard (used for the
/// injection-pattern scans in [`crate::injection`] — a different corpus from
/// the destructive-command blocklist). Cached per distinct pattern slice;
/// returns a clone (the cache owns the canonical instance).
pub fn guard_extra(patterns: &'static [&'static str]) -> Guard {
    use std::sync::OnceLock;
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<usize, Guard>>> =
        OnceLock::new();
    let ptr = patterns.as_ptr() as usize;
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .entry(ptr)
        .or_insert_with(|| {
            Guard::compile(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>())
                .expect("extra patterns must compile")
        })
        .clone()
}

/// Compiled blocklist over the full P7.4 corpus.
#[derive(Debug, Clone)]
pub struct Guard {
    set: RegexSet,
    count: usize,
}

impl Guard {
    /// Compile from explicit patterns (tests, custom corpora).
    pub fn compile(patterns: &[String]) -> Result<Self, regex::Error> {
        let set = RegexSet::new(patterns)?;
        let count = patterns.len();
        Ok(Self { set, count })
    }

    /// All indices of patterns that matched `text`.
    pub fn scan(&self, text: &str) -> Vec<usize> {
        self.set.matches(text).iter().collect()
    }

    /// Is `text` blocked by any pattern?
    pub fn is_blocked(&self, text: &str) -> bool {
        !self.scan(text).is_empty()
    }

    /// Matched indices → categories, deduplicated in order.
    pub fn categories(&self, indices: &[usize]) -> Vec<BlocklistCategory> {
        let mut out = Vec::new();
        for &i in indices {
            if let Some(c) = category_of(i)
                && !out.contains(&c)
            {
                out.push(c);
            }
        }
        out
    }

    /// Number of compiled patterns (for the audit line + tests).
    pub fn pattern_count(&self) -> usize {
        self.count
    }
}

/// What kind of target a scan hit (shown on the Guard-2 card).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanTarget {
    Shell,
    Path,
    Url,
}

impl ScanTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanTarget::Shell => "shell",
            ScanTarget::Path => "path",
            ScanTarget::Url => "url",
        }
    }
}

/// One blocklist hit during pre-exec scanning.
#[derive(Debug, Clone, PartialEq)]
pub struct PreExecScan {
    pub target: ScanTarget,
    /// The exact string that matched (for the diff card).
    pub text: String,
    /// Pattern indices that matched (into the P7.4 corpus).
    pub pattern_indices: Vec<usize>,
}

impl PreExecScan {
    pub fn new(target: ScanTarget, text: String, pattern_indices: Vec<usize>) -> Self {
        Self {
            target,
            text,
            pattern_indices,
        }
    }

    /// Categories of the matched patterns.
    pub fn categories(&self) -> Vec<BlocklistCategory> {
        guard().categories(&self.pattern_indices)
    }
}

/// Scan a single shell string. Empty = clean.
pub fn scan_shell(cmd: &str) -> Vec<usize> {
    guard().scan(cmd)
}

/// Scan a single filesystem path. Empty = clean.
pub fn scan_path(path: &str) -> Vec<usize> {
    guard().scan(path)
}

/// Scan a single URL. Empty = clean.
pub fn scan_url(url: &str) -> Vec<usize> {
    guard().scan(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_corpus_compiles() {
        assert!(guard().pattern_count() >= blocklist_len());
        assert!(guard().pattern_count() > 30, "corpus should be substantial");
    }

    fn blocklist_len() -> usize {
        crate::blocklist::BLOCKLIST.len()
    }

    #[test]
    fn blocks_destructive_shell() {
        let g = guard();
        for cmd in [
            "rm -rf /",
            "rm -rf ~",
            "rm -rf *",
            "rm -r /etc",
            "rm -fr /home/user",
            "sudo rm -rf /",
            "mkfs.ext4 /dev/sdb",
            "dd if=/dev/zero of=/dev/sda",
            "dd of=/dev/sda if=/dev/zero",
            ":(){ :|:& };:",
            "drop database prod",
            "truncate table users",
            "delete from accounts where 1=1",
            "chmod 777 /tmp/x",
            "git clean -f",
            "git clean -fd",
            "shred -uz /",
            "> ~/.bashrc",
        ] {
            assert!(g.is_blocked(cmd), "should block: {cmd}");
        }
    }

    #[test]
    fn allows_benign() {
        let g = guard();
        for cmd in [
            "ls -la",
            "cat notes.md",
            "git status",
            "git diff",
            "cargo build",
            "echo hello",
            "rm file.txt", // single non-recursive file — not blocked
        ] {
            assert!(!g.is_blocked(cmd), "should allow: {cmd}");
        }
    }

    #[test]
    fn category_lookup_works() {
        let hits = scan_shell("rm -rf ~");
        assert!(!hits.is_empty());
        let cats = guard().categories(&hits);
        assert!(
            cats.contains(&BlocklistCategory::HomeWipe)
                || cats.contains(&BlocklistCategory::DestructiveDelete)
        );
    }
}
