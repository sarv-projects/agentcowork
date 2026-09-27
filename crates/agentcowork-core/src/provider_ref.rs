//! A6 catalog long-tail (doc 58/59 — OmniRoute `PROVIDER_REFERENCE.md`
//! pattern): the ingestion core for the 339-provider reference catalog. The
//! file itself (MIT, generated) is dropped in at install/build time; this
//! module owns the **parser + allow-list policy** — import only the API-key,
//! local, and keyless (no-auth) classes, and treat the web-cookie and OAuth-CLI
//! classes as the doc-57 reject list (subscription-session harvest), not the
//! import list.

use serde::{Deserialize, Serialize};

/// The auth class of a provider — the axis our allow-list keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthClass {
    /// Documented API-key auth (the BYOK long tail).
    ApiKey,
    /// Local runtime (Ollama / llama.cpp / MLX / …).
    Local,
    /// Keyless / no-auth (public OpenAI-compatible endpoints).
    Keyless,
    /// OAuth CLI (codex, cursor, …) — doc-57 reject class.
    OAuth,
    /// Web-cookie (chatgpt-web, claude-web, …) — doc-57 reject class.
    Cookie,
    /// Search / audio / upstream-proxy / cloud-agent / system / unknown.
    Other,
}

impl AuthClass {
    /// Is this class in the A6 import allow-list?
    pub fn allowed(self) -> bool {
        matches!(
            self,
            AuthClass::ApiKey | AuthClass::Local | AuthClass::Keyless
        )
    }

    /// Is this class in the doc-57 reject list?
    pub fn rejected(self) -> bool {
        matches!(self, AuthClass::OAuth | AuthClass::Cookie)
    }
}

/// One parsed provider entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// Provider id (first table column).
    pub id: String,
    /// Raw category text (for the audit trail).
    pub category: String,
    pub auth_class: AuthClass,
    /// Tool-calling mode: native / emulated / none (empty when unknown).
    pub tool_calling: String,
}

/// The ingestion result: allowed imports + a per-class reject count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestReport {
    pub allowed: Vec<ProviderEntry>,
    pub rejected_cookie: usize,
    pub rejected_oauth: usize,
    pub rejected_other: usize,
}

impl IngestReport {
    pub fn total_seen(&self) -> usize {
        self.allowed.len() + self.rejected_cookie + self.rejected_oauth + self.rejected_other
    }
}

/// Map a raw category string (case-insensitive) to an [`AuthClass`].
pub fn classify_category(category: &str) -> AuthClass {
    let c = category.to_lowercase();
    if c.contains("oauth") {
        AuthClass::OAuth
    } else if c.contains("cookie") {
        AuthClass::Cookie
    } else if c.contains("local") {
        AuthClass::Local
    } else if c.contains("api") && c.contains("key") {
        AuthClass::ApiKey
    } else if c.contains("no-auth")
        || c.contains("no auth")
        || c.contains("keyless")
        || c.contains("none")
        || c.contains("free")
    {
        AuthClass::Keyless
    } else {
        AuthClass::Other
    }
}

/// Parse a `PROVIDER_REFERENCE.md`-style markdown table into entries. Tolerant:
/// skips non-table lines, reads column headers from the first table row, and
/// degrades gracefully when an expected column is missing.
pub fn parse_provider_reference(md: &str) -> Vec<ProviderEntry> {
    let mut lines = md.lines().filter_map(|l| {
        let t = l.trim();
        t.strip_prefix('|')
            .and_then(|s| s.strip_suffix('|'))
            .map(|s| s.trim().to_string())
    });

    // Header row → column names (lowercased).
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    // Skip the `---` separator row if present.
    let header_cells: Vec<String> = split_row(&header);
    let col_index = |name: &str| {
        header_cells
            .iter()
            .position(|h| h.to_lowercase().contains(name))
    };

    let id_col = 0usize;
    let category_col = col_index("categor")
        .or_else(|| col_index("type"))
        .or_else(|| col_index("class"))
        .unwrap_or(1);
    let tool_col = col_index("tool").or_else(|| col_index("function"));

    let mut out = Vec::new();
    for line in lines {
        let cells = split_row(&line);
        if cells.is_empty() {
            continue;
        }
        let id = cells
            .get(id_col)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        // Skip the markdown table separator row (`---` / `:--:`).
        if id.is_empty()
            || id
                .chars()
                .all(|c| c == '-' || c == ':' || c.is_whitespace())
        {
            continue;
        }
        let category = cells
            .get(category_col)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let tool_calling = tool_col
            .and_then(|i| cells.get(i))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        out.push(ProviderEntry {
            id,
            category: category.clone(),
            auth_class: classify_category(&category),
            tool_calling,
        });
    }
    out
}

/// Ingest: parse + split into the allow-list and the per-class reject counts.
pub fn ingest_provider_reference(md: &str) -> IngestReport {
    let entries = parse_provider_reference(md);
    let mut report = IngestReport {
        allowed: Vec::new(),
        rejected_cookie: 0,
        rejected_oauth: 0,
        rejected_other: 0,
    };
    for e in entries {
        match e.auth_class {
            AuthClass::Cookie => report.rejected_cookie += 1,
            AuthClass::OAuth => report.rejected_oauth += 1,
            AuthClass::Other => report.rejected_other += 1,
            _ => report.allowed.push(e),
        }
    }
    report
}

fn split_row(row: &str) -> Vec<String> {
    row.split('|').map(|s| s.trim().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "# Provider Reference\n\n\
        | Provider | Category | Tool Calling | Free Tier |\n\
        |----------|----------|--------------|-----------|\n\
        | openai | API key | native | no |\n\
        | ollama | Local | native | yes |\n\
        | deepseek | API key | native | no |\n\
        | chatgpt-web | Web cookie | none | yes |\n\
        | claude-web | Web cookie | none | yes |\n\
        | codex | OAuth | native | no |\n\
        | cursor | OAuth | native | no |\n\
        | free-llm | No auth | none | yes |\n";

    #[test]
    fn classifies_categories() {
        assert_eq!(classify_category("API key"), AuthClass::ApiKey);
        assert_eq!(classify_category("Local"), AuthClass::Local);
        assert_eq!(classify_category("Web cookie"), AuthClass::Cookie);
        assert_eq!(classify_category("OAuth"), AuthClass::OAuth);
        assert_eq!(classify_category("No auth"), AuthClass::Keyless);
        assert_eq!(classify_category("Search"), AuthClass::Other);
    }

    #[test]
    fn parses_table_rows() {
        let entries = parse_provider_reference(FIXTURE);
        assert_eq!(entries.len(), 8);
        assert_eq!(entries[0].id, "openai");
        assert_eq!(entries[0].auth_class, AuthClass::ApiKey);
        assert_eq!(entries[0].tool_calling, "native");
    }

    #[test]
    fn ingest_applies_allowlist() {
        let report = ingest_provider_reference(FIXTURE);
        // Allowed: openai, ollama, deepseek, free-llm (API-key + local + keyless).
        assert_eq!(report.allowed.len(), 4);
        let ids: Vec<&str> = report.allowed.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"ollama"));
        assert!(ids.contains(&"deepseek"));
        assert!(ids.contains(&"free-llm"));
        // Rejected: 2 cookie + 2 OAuth.
        assert_eq!(report.rejected_cookie, 2);
        assert_eq!(report.rejected_oauth, 2);
        assert_eq!(report.rejected_other, 0);
        assert_eq!(report.total_seen(), 8);
    }

    #[test]
    fn cookie_and_oauth_are_rejected_classes() {
        assert!(AuthClass::Cookie.rejected());
        assert!(AuthClass::OAuth.rejected());
        assert!(!AuthClass::Cookie.allowed());
        assert!(!AuthClass::OAuth.allowed());
        assert!(AuthClass::ApiKey.allowed());
        assert!(AuthClass::Local.allowed());
        assert!(AuthClass::Keyless.allowed());
    }

    #[test]
    fn report_serializes() {
        let report = ingest_provider_reference(FIXTURE);
        let json = serde_json::to_string(&report).unwrap();
        let back: IngestReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.allowed.len(), 4);
    }

    #[test]
    fn malformed_input_degrades_gracefully() {
        assert!(parse_provider_reference("no table here").is_empty());
        assert_eq!(ingest_provider_reference("").total_seen(), 0);
    }
}
