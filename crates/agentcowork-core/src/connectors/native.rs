//! P22-3 — Native connector **write template** (doc 74 §4 — `postgres-mcp-
//! hardened` 🟡 ADAPT): the mandatory posture for every Native connector
//! write path, shipped as a reusable template rather than per-connector
//! boilerplate.
//!
//! The four rails every write rides:
//! 1. **Refuse-twice** — statement validation *and* a read-only default
//!    (`SqlGuard`: write statements are refused unless the caller explicitly
//!    enables writes for the session; stacked statements are refused too).
//! 2. **`statement_timeout`** — every write session carries a hard timeout
//!    (emitted as the SQL session preamble by the runtime).
//! 3. **Column redaction + EXPLAIN cost guard** — values in sensitive
//!    columns are never echoed back; queries above a cost budget are
//!    refused before they run.
//! 4. **Hash-chained audit** — every statement is appended to an
//!    append-only chain (each entry binds the previous hash), so the write
//!    trail is tamper-evident.

use serde::{Deserialize, Serialize};

/// The SQL write-statement classifier: strips comments, then tokenizes at
/// the first keyword of each statement to decide read vs write. `;`-stacked
/// statements are refused outright (the guard never guesses).
pub fn classify_sql(sql: &str) -> SqlClass {
    let cleaned = strip_sql_comments(sql);
    let first_word = cleaned
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    match first_word.as_str() {
        "SELECT" | "SHOW" | "EXPLAIN" | "DESCRIBE" | "PRAGMA" | "VALUES" => SqlClass::Read,
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "UPSERT" => SqlClass::Write,
        "CREATE" | "DROP" | "ALTER" | "TRUNCATE" | "GRANT" | "REVOKE" | "CALL" => SqlClass::Write,
        _ => SqlClass::Unrecognized,
    }
}

fn strip_sql_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;
    let b = sql.as_bytes();
    while i < b.len() {
        if b[i] == b'-' && i + 1 < b.len() && b[i + 1] == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// `INSERT ...; DROP TABLE x` — stacked statements are never allowed.
///
/// A single **trailing** `;` is a proper statement terminator (`SELECT 1;` is
/// a legitimate single statement), so only a `;` followed by more SQL counts
/// as stacking. The string-state machine also honours SQL's `''` doubled-
/// quote escape, so a literal apostrophe inside a string (e.g. `'O''Brien'`)
/// does not confuse the boundary detection.
pub fn has_stacked_statements(sql: &str) -> bool {
    let cleaned = strip_sql_comments(sql);
    let chars: Vec<char> = cleaned.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    // a `;` outside of a string literal, followed by further SQL, is stacking
    while i < n {
        let c = chars[i];
        if c == '\'' {
            // Enter (or advance within) a string literal, honouring `''`.
            i += 1;
            let mut in_string = true;
            while in_string && i < n {
                if chars[i] == '\'' {
                    // `''` is an escaped quote *within* the string — stay in,
                    // consuming both; a bare `'` *after* a non-doubled close
                    // exits. Track the previous char to decide.
                    if i + 1 < n && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        in_string = false;
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if c == ';' {
            // Skip trailing whitespace/comments; if any token follows, this is
            // a stacked statement.
            let mut j = i + 1;
            while j < n && (chars[j].is_whitespace() || chars[j] == ';') {
                j += 1;
            }
            if j < n {
                return true; // non-whitespace follows the `;`
            }
            // else: trailing terminator — legitimate.
        }
        i += 1;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlClass {
    Read,
    Write,
    Unrecognized,
}

/// Rail 1 — refuse-twice: statement class *and* a read-only default.
#[derive(Debug, Clone)]
pub struct SqlGuard {
    /// Default `true` — writes are refused until the caller opts in.
    pub read_only_default: bool,
    /// Sessions that opt in must also carry a timeout (rail 2).
    pub statement_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SqlGuardError {
    #[error("stacked statements are refused")]
    Stacked,
    #[error("unrecognized statement (refused — the guard never guesses)")]
    Unrecognized,
    #[error("write statement refused: session is read-only by default")]
    ReadOnlyDefault,
    #[error("write session requires a statement timeout (got 0)")]
    NoTimeout,
}

impl SqlGuard {
    pub fn new() -> Self {
        Self {
            read_only_default: true,
            statement_timeout_ms: 5_000,
        }
    }

    /// Gate a statement before it runs. Returns the session preamble
    /// (the `statement_timeout` SET) for write sessions.
    pub fn check(&self, sql: &str, writes_enabled: bool) -> Result<Option<String>, SqlGuardError> {
        if has_stacked_statements(sql) {
            return Err(SqlGuardError::Stacked);
        }
        match classify_sql(sql) {
            SqlClass::Read => Ok(None),
            SqlClass::Unrecognized => Err(SqlGuardError::Unrecognized),
            SqlClass::Write => {
                if !writes_enabled {
                    return Err(SqlGuardError::ReadOnlyDefault);
                }
                if self.statement_timeout_ms == 0 {
                    return Err(SqlGuardError::NoTimeout);
                }
                Ok(Some(format!(
                    "SET statement_timeout = {};",
                    self.statement_timeout_ms
                )))
            }
        }
    }
}

impl Default for SqlGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Rail 3a — column redaction: sensitive columns are never echoed back.
#[derive(Debug, Clone, Default)]
pub struct ColumnRedaction {
    /// Lowercased column names to redact (exact match).
    pub exact: Vec<String>,
    /// Substrings that mark a column sensitive (e.g. "password", "token").
    pub patterns: Vec<String>,
}

impl ColumnRedaction {
    /// The default sensitive-column set (the template's baseline).
    pub fn defaults() -> Self {
        Self {
            exact: vec!["ssn".into(), "cvv".into(), "pin".into()],
            patterns: vec![
                "password".into(),
                "token".into(),
                "secret".into(),
                "api_key".into(),
                "card".into(),
                "iban".into(),
                "credential".into(),
            ],
        }
    }

    pub fn is_sensitive(&self, column: &str) -> bool {
        let lower = column.to_ascii_lowercase();
        if self.exact.iter().any(|e| e == &lower) {
            return true;
        }
        self.patterns.iter().any(|p| lower.contains(p))
    }

    /// Replace sensitive values with `[redacted]` in a row (column → value).
    pub fn redact_row(&self, row: &[(String, String)]) -> Vec<(String, String)> {
        row.iter()
            .map(|(c, v)| {
                if self.is_sensitive(c) {
                    (c.clone(), "[redacted]".into())
                } else {
                    (c.clone(), v.clone())
                }
            })
            .collect()
    }
}

/// Rail 3b — EXPLAIN cost guard: refuse plans above a budget. Parses the
/// `cost=lo..hi` notation in a Postgres EXPLAIN (VERBOSE) line.
#[derive(Debug, Clone, Copy)]
pub struct ExplainCostGuard {
    pub max_cost: f64,
}

impl ExplainCostGuard {
    pub fn new(max_cost: f64) -> Self {
        Self { max_cost }
    }

    /// Parse the first `cost=<lo>..<hi>` in an EXPLAIN output.
    pub fn parse_cost(explain: &str) -> Option<f64> {
        let lower = explain.to_ascii_lowercase();
        let idx = lower.find("cost=")?;
        let rest = &lower[idx + 5..];
        let lo = rest.split("..").next()?.trim();
        let hi = rest
            .split("..")
            .nth(1)?
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .next()?
            .trim();
        // the guard budgets on the upper bound (worst case)
        hi.parse::<f64>().ok().or_else(|| lo.parse::<f64>().ok())
    }

    pub fn check(&self, explain: &str) -> Result<(), CostGuardError> {
        match Self::parse_cost(explain) {
            Some(cost) if cost <= self.max_cost => Ok(()),
            Some(cost) => Err(CostGuardError::OverBudget {
                cost,
                max: self.max_cost,
            }),
            None => Err(CostGuardError::NoPlan),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum CostGuardError {
    #[error("plan cost {cost:.2} exceeds the {max:.2} budget")]
    OverBudget { cost: f64, max: f64 },
    #[error("EXPLAIN output contains no parseable cost (refused)")]
    NoPlan,
}

/// Rail 4 — hash-chained audit: append-only, tamper-evident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub action: String,
    /// The full statement text (never redacted in the audit — the chain is
    /// the honest record; redaction is a *response* concern).
    pub statement: String,
    pub ts_ms: u64,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditChain {
    pub entries: Vec<AuditEntry>,
}

impl AuditChain {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The genesis hash (no predecessor).
    pub fn genesis() -> String {
        "genesis".to_string()
    }

    pub fn push(&mut self, action: &str, statement: &str, ts_ms: u64) -> String {
        let seq = self.entries.len() as u64;
        let prev = self
            .entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(Self::genesis);
        let hash = Self::hash(&prev, action, statement, ts_ms);
        self.entries.push(AuditEntry {
            seq,
            action: action.to_string(),
            statement: statement.to_string(),
            ts_ms,
            prev_hash: prev,
            hash: hash.clone(),
        });
        hash
    }

    fn hash(prev: &str, action: &str, statement: &str, ts_ms: u64) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(prev.as_bytes());
        hasher.update(action.as_bytes());
        hasher.update(statement.as_bytes());
        hasher.update(ts_ms.to_le_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Walk the whole chain; any broken link (or re-ordered/edited entry)
    /// fails the check.
    pub fn verify(&self) -> bool {
        let mut prev = Self::genesis();
        for (i, e) in self.entries.iter().enumerate() {
            if e.seq != i as u64 {
                return false;
            }
            if e.prev_hash != prev {
                return false;
            }
            let expected = Self::hash(&e.prev_hash, &e.action, &e.statement, e.ts_ms);
            if e.hash != expected {
                return false;
            }
            prev = e.hash.clone();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reads_and_writes() {
        assert_eq!(classify_sql("SELECT * FROM users"), SqlClass::Read);
        assert_eq!(classify_sql("-- fetch\nSELECT id FROM t"), SqlClass::Read);
        assert_eq!(classify_sql("/* c */ UPDATE t SET x=1"), SqlClass::Write);
        assert_eq!(classify_sql("INSERT INTO t VALUES (1)"), SqlClass::Write);
        assert_eq!(classify_sql("DROP TABLE users"), SqlClass::Write);
        assert_eq!(classify_sql("EXPLAIN SELECT 1"), SqlClass::Read);
    }

    #[test]
    fn refuses_stacked_and_unknown() {
        assert!(has_stacked_statements("SELECT 1; DROP TABLE t"));
        assert!(!has_stacked_statements("SELECT 'a;b' AS x"));
        let guard = SqlGuard::new();
        assert!(matches!(
            guard.check("SELECT 1; DELETE FROM t", true),
            Err(SqlGuardError::Stacked)
        ));
        assert!(matches!(
            guard.check("VACUUM", true),
            Err(SqlGuardError::Unrecognized)
        ));
    }

    #[test]
    fn trailing_terminator_is_not_stacking() {
        // Bugfix 17 — `SELECT 1;` is a single, well-formed read.
        assert!(!has_stacked_statements("SELECT 1;"));
        assert!(!has_stacked_statements("SELECT 1; -- trailing comment"));
        assert!(!has_stacked_statements("UPDATE t SET x=1;"));
    }

    #[test]
    fn escaped_quotes_do_not_break_the_state_machine() {
        // Bugfix 17 — SQL `''` is an escaped literal quote inside a string.
        assert!(!has_stacked_statements("SELECT 'O''Brien' AS name"));
        assert!(!has_stacked_statements(
            "SELECT 'it''s; still one string' AS x"
        ));
        // A real stacked statement after an escaped-quote string is still caught.
        assert!(has_stacked_statements("SELECT 'x''y' AS a; DROP TABLE t"));
    }

    #[test]
    fn read_only_default_refuses_writes() {
        let guard = SqlGuard::new();
        assert!(guard.check("SELECT 1", false).is_ok());
        assert!(matches!(
            guard.check("UPDATE t SET x=1", false),
            Err(SqlGuardError::ReadOnlyDefault)
        ));
        // opted-in writes get the timeout preamble
        let preamble = guard.check("UPDATE t SET x=1", true).unwrap().unwrap();
        assert!(preamble.contains("statement_timeout = 5000"));
    }

    #[test]
    fn redacts_sensitive_columns() {
        let red = ColumnRedaction::defaults();
        assert!(red.is_sensitive("password"));
        assert!(red.is_sensitive("access_token"));
        assert!(red.is_sensitive("SSN"));
        assert!(!red.is_sensitive("name"));
        let row = vec![
            ("email".to_string(), "a@b.c".to_string()),
            ("password_hash".to_string(), "hunter2".to_string()),
        ];
        let out = red.redact_row(&row);
        assert_eq!(out[1].1, "[redacted]");
        assert_eq!(out[0].1, "a@b.c");
    }

    #[test]
    fn explain_cost_guard_budgets_on_upper_bound() {
        let guard = ExplainCostGuard::new(50.0);
        assert!(
            guard
                .check("Seq Scan on t (cost=0.00..42.50 rows=10)")
                .is_ok()
        );
        assert!(matches!(
            guard.check("Nested Loop (cost=10.00..99.00 rows=1000)"),
            Err(CostGuardError::OverBudget { .. })
        ));
        assert!(matches!(
            guard.check("no cost here"),
            Err(CostGuardError::NoPlan)
        ));
        assert_eq!(
            ExplainCostGuard::parse_cost("Seq Scan (cost=0.00..42.50)"),
            Some(42.5)
        );
    }

    #[test]
    fn audit_chain_is_tamper_evident() {
        let mut chain = AuditChain::new();
        chain.push("update", "UPDATE t SET x=1 WHERE id=5", 1000);
        chain.push("insert", "INSERT INTO log VALUES (1)", 2000);
        assert!(chain.verify());
        // tamper: edit the first statement
        chain.entries[0].statement = "UPDATE t SET x=999 WHERE id=5".into();
        assert!(!chain.verify());
    }
}
