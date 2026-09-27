//! P7.4 — the full destructive-pattern blocklist (doc 06, doc 03 §8, doc 26
//! red-team corpus). Every pattern is a conservative regex: blocking on a
//! match is mandatory, so patterns err toward matching (the cost of a false
//! positive is an approval card, not data loss).

/// Categories the blocklist patterns belong to (for the decision package:
/// Guard-2 cards show *why* something was blocked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlocklistCategory {
    /// Recursive delete / home wipe / root wipe.
    DestructiveDelete,
    /// mkfs, dd, format — raw device writes.
    RawDeviceWrite,
    /// drop database / truncate / delete without where.
    DatabaseDestruction,
    /// Fork bombs and CPU/VM exhaustion.
    ForkBomb,
    /// Key/secret exfiltration (ssh keys, .env, wallet files, token files).
    KeyExfiltration,
    /// `.git` destruction (clean -f, rm .git, filter-branch).
    GitDestruction,
    /// Home-wipe / shell-profile clobber.
    HomeWipe,
    /// chmod 777-style permission loosening.
    PermissionLoosening,
}

/// (pattern, category) pairs. Keep patterns regex-safe (compiled into a
/// `RegexSet` by the caller).
pub const BLOCKLIST: &[(&str, BlocklistCategory)] = &[
    // --- DestructiveDelete ---
    (
        r"(?i)\brm\s+(-[a-z]*[rR][a-z]*\s*)+/?\s*(/|\*|~|\.)",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+/(\s|$)",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+~(\s|$)",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+\*(\s|$)",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\brmdir\s+/(\s|$)",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+\.",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\bshred\s+(-[a-z]*[uU][a-z]*\s*)?(-[a-z]*[zZ][a-z]*\s*)?/?\s*(/|\*|~)",
        BlocklistCategory::DestructiveDelete,
    ),
    (
        r"(?i)\btruncate\s+-s\s+0\s+",
        BlocklistCategory::DestructiveDelete,
    ),
    // --- RawDeviceWrite ---
    (r"(?i)\bmkfs(?:\.|\s)", BlocklistCategory::RawDeviceWrite),
    (r"(?i)\bmkfs\.\w+", BlocklistCategory::RawDeviceWrite),
    (r"(?i)\bdd\s+if=", BlocklistCategory::RawDeviceWrite),
    (r"(?i)\bdd\s+of=/dev/", BlocklistCategory::RawDeviceWrite),
    (r"(?i)\bformat\s+[a-z]:", BlocklistCategory::RawDeviceWrite),
    (r"(?i)\bmkfs\s+/dev/", BlocklistCategory::RawDeviceWrite),
    (r"(?i)\bfdisk\s+/dev/", BlocklistCategory::RawDeviceWrite),
    (
        r"(?i)\bfdisk\s+-?\w*\s*$",
        BlocklistCategory::RawDeviceWrite,
    ),
    (
        r"(?i)\bparted\s+/dev/.*\bmkpart\b",
        BlocklistCategory::RawDeviceWrite,
    ),
    // --- DatabaseDestruction ---
    (
        r"(?i)\bdrop\s+database\b",
        BlocklistCategory::DatabaseDestruction,
    ),
    (
        r"(?i)\bdrop\s+table\b",
        BlocklistCategory::DatabaseDestruction,
    ),
    (
        r"(?i)\btruncate\s+table\b",
        BlocklistCategory::DatabaseDestruction,
    ),
    (
        r"(?i)\bdelete\s+from\s+\w+\s*(;|$)",
        BlocklistCategory::DatabaseDestruction,
    ),
    (
        r"(?i)\bdelete\s+from\s+\w+\s+where\s+1\s*=\s*1",
        BlocklistCategory::DatabaseDestruction,
    ),
    // --- ForkBomb ---
    (
        r"(?:^|[^[:alnum:]_]):\(\)\s*\{\s*:\|:&\s*\};:",
        BlocklistCategory::ForkBomb,
    ),
    (
        r"(?i)\b:\(\)\s*\{\s*:\|:&\s*\}",
        BlocklistCategory::ForkBomb,
    ),
    (
        r"(?i)\bwhile\s+true\s*;\s*do\s+(.*)\s*&\s*done\b",
        BlocklistCategory::ForkBomb,
    ),
    (
        r"(?i)\bfor\s+.*\s+in\s+.*;\s*do\s+.*&\s*done\b",
        BlocklistCategory::ForkBomb,
    ),
    // --- KeyExfiltration ---
    (
        r"(?i)(curl|wget|nc|ncat|socat)\s+.*(\b-h\b|--header|\.ssh/|id_rsa|\.env|\.pem|credentials|secret|token|wallet|\.key)",
        BlocklistCategory::KeyExfiltration,
    ),
    (
        r"(?i)\bcat\s+.*(id_rsa|\.pem|\.env|credentials|secret|wallet)\b",
        BlocklistCategory::KeyExfiltration,
    ),
    (
        r"(?i)\bscp\s+.*(id_rsa|\.pem|\.env|credentials|secret|wallet)\b",
        BlocklistCategory::KeyExfiltration,
    ),
    (
        r"(?i)\bbase64\s+-d?\s+.*(id_rsa|\.pem|\.env|credentials)",
        BlocklistCategory::KeyExfiltration,
    ),
    (
        r"(?i)\bcurl\s+.*\b-F\s+.*@\b",
        BlocklistCategory::KeyExfiltration,
    ),
    (
        r"(?i)\bopenssl\s+.*\b(genrsa|genpkey)\b",
        BlocklistCategory::KeyExfiltration,
    ),
    (r"(?i)\bssh-keyscan\b", BlocklistCategory::KeyExfiltration),
    // --- GitDestruction ---
    (
        r"(?i)\bgit\s+clean\s+-[a-z]*[fF]",
        BlocklistCategory::GitDestruction,
    ),
    (
        r"(?i)\bgit\s+filter-branch\s+--force",
        BlocklistCategory::GitDestruction,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+\.git",
        BlocklistCategory::GitDestruction,
    ),
    (
        r"(?i)\bfind\s+\.\s+.*\b-delete\b",
        BlocklistCategory::GitDestruction,
    ),
    (
        r"(?i)\bgit\s+update-ref\s+-d\s+refs/heads/",
        BlocklistCategory::GitDestruction,
    ),
    // --- HomeWipe ---
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+~/\s*$",
        BlocklistCategory::HomeWipe,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+\$HOME\b",
        BlocklistCategory::HomeWipe,
    ),
    (
        r"(?i)\brm\s+-[a-z]*[rR][a-z]*\s+\$\{HOME\}\b",
        BlocklistCategory::HomeWipe,
    ),
    (
        r"(?i)>\s*~/\.[a-z]+(rc|_history|profile|bashrc|zshrc)",
        BlocklistCategory::HomeWipe,
    ),
    (
        r"(?i)\brm\s+(-[a-z]*\s+)?~?/?\.bashrc",
        BlocklistCategory::HomeWipe,
    ),
    (
        r"(?i)\brm\s+(-[a-z]*\s+)?~?/?\.zshrc",
        BlocklistCategory::HomeWipe,
    ),
    (
        r"(?i)\brm\s+(-[a-z]*\s+)?~?/?\.profile",
        BlocklistCategory::HomeWipe,
    ),
    // --- PermissionLoosening ---
    (
        r"(?i)\bchmod\s+[0-7]{4}\b",
        BlocklistCategory::PermissionLoosening,
    ),
    (
        r"(?i)\bchmod\s+-[a-z]*[Rr][a-z]*\s+777",
        BlocklistCategory::PermissionLoosening,
    ),
    (r"(?i)\bchmod\s+777", BlocklistCategory::PermissionLoosening),
    (
        r"(?i)\bchown\s+-[a-z]*[Rr][a-z]*\s+",
        BlocklistCategory::PermissionLoosening,
    ),
    (r"(?i)\bsudo\s+rm\b", BlocklistCategory::DestructiveDelete),
    (r"(?i)\bsudo\s+dd\b", BlocklistCategory::RawDeviceWrite),
];

/// The pattern strings alone, in the same order (for `RegexSet`).
pub fn blocklist_for() -> Vec<String> {
    BLOCKLIST.iter().map(|(p, _)| (*p).to_string()).collect()
}

/// Category of the pattern at index `i` (as returned by `Guard::scan`).
pub fn category_of(index: usize) -> Option<BlocklistCategory> {
    BLOCKLIST.get(index).map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_has_all_categories() {
        let mut cats = std::collections::HashSet::new();
        for (_, c) in BLOCKLIST {
            cats.insert(*c);
        }
        assert!(cats.contains(&BlocklistCategory::DestructiveDelete));
        assert!(cats.contains(&BlocklistCategory::RawDeviceWrite));
        assert!(cats.contains(&BlocklistCategory::DatabaseDestruction));
        assert!(cats.contains(&BlocklistCategory::ForkBomb));
        assert!(cats.contains(&BlocklistCategory::KeyExfiltration));
        assert!(cats.contains(&BlocklistCategory::GitDestruction));
        assert!(cats.contains(&BlocklistCategory::HomeWipe));
        assert!(cats.contains(&BlocklistCategory::PermissionLoosening));
    }
}
