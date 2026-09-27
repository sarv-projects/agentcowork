//! P18-5 — Context7 docs-lookup reference (doc 70 §5 — official, 🟢
//! reference, post-v1). Maps to the I11 code-intel docs-lookup tool:
//! version-specific library documentation pulled into prompts *instead of*
//! stale memorized API shape.
//!
//! The live fetch (Context7 API / local index) is a documented runtime seam —
//! this module owns the *contract*: how a library+version is addressed, how
//! a docs query is shaped, and how results are bounded so they fit a prompt.
//! Nothing here performs network I/O; `DocsFetcher` is the injectable seam.

use serde::{Deserialize, Serialize};

/// A version-specific library address (Context7-style).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRef {
    /// e.g. `sst` | `@langchain/openai` | `next` | `react`
    pub slug: String,
    /// `latest` resolves at fetch time; a pin keeps prompts stable.
    pub version: String,
}

impl LibraryRef {
    pub fn latest(slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            version: "latest".into(),
        }
    }

    pub fn pinned(slug: &str, version: &str) -> Self {
        Self {
            slug: slug.to_string(),
            version: version.to_string(),
        }
    }
}

/// What to pull out of the docs for a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocsQuery {
    /// Export signatures + option tables for a symbol.
    Symbol,
    /// The installation + first-use snippet.
    Quickstart,
    /// Config/reference pages by topic.
    Topic,
}

/// The bounded result set (a prompt can hold only so much).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocsResult {
    pub library: LibraryRef,
    /// Absolute maximum characters the caller will inject.
    pub budget_chars: usize,
    pub excerpts: Vec<DocExcerpt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocExcerpt {
    /// e.g. `pages/api-reference/send.mdx#send`.
    pub anchor: String,
    pub text: String,
}

/// The injectable fetch seam — tests inject a fake; the real implementation
/// (Context7 API or a local docs index) is runtime wiring.
pub trait DocsFetcher {
    fn fetch(
        &self,
        library: &LibraryRef,
        query: DocsQuery,
        budget_chars: usize,
    ) -> Result<DocsResult, DocsLookupError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DocsLookupError {
    #[error("unknown library `{0}`")]
    UnknownLibrary(String),
    #[error("no docs for version `{0}` of `{1}`")]
    NoVersion(String, String),
    #[error("fetcher error: {0}")]
    Fetcher(String),
}

/// Truncate excerpts to the budget (whole-excerpt policy: never split a
/// snippet mid-way — drop the rest rather than garble it).
pub fn fit_budget(result: &mut DocsResult) {
    let mut used = 0usize;
    result.excerpts.retain(|e| {
        if used + e.text.len() > result.budget_chars {
            return false;
        }
        used += e.text.len();
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFetcher;
    impl DocsFetcher for FakeFetcher {
        fn fetch(
            &self,
            library: &LibraryRef,
            _query: DocsQuery,
            budget_chars: usize,
        ) -> Result<DocsResult, DocsLookupError> {
            if library.slug == "ghost" {
                return Err(DocsLookupError::UnknownLibrary("ghost".into()));
            }
            Ok(DocsResult {
                library: library.clone(),
                budget_chars,
                excerpts: vec![
                    DocExcerpt {
                        anchor: "send".into(),
                        text: "send({ to, subject })".into(),
                    },
                    DocExcerpt {
                        anchor: "reply".into(),
                        text: "reply({ to, inReplyTo })".into(),
                    },
                ],
            })
        }
    }

    #[test]
    fn addresses_library_and_version() {
        let lib = LibraryRef::pinned("@langchain/openai", "0.3.0");
        assert_eq!(lib.slug, "@langchain/openai");
        assert_eq!(lib.version, "0.3.0");
        assert_eq!(LibraryRef::latest("next").version, "latest");
    }

    #[test]
    fn fetcher_seam_is_injectable_and_honest() {
        let f = FakeFetcher;
        assert!(matches!(
            f.fetch(&LibraryRef::latest("ghost"), DocsQuery::Symbol, 1000),
            Err(DocsLookupError::UnknownLibrary(_))
        ));
        let ok = f
            .fetch(&LibraryRef::latest("sst"), DocsQuery::Symbol, 1000)
            .unwrap();
        assert_eq!(ok.excerpts.len(), 2);
    }

    #[test]
    fn budget_never_splits_an_excerpt() {
        let mut result = DocsResult {
            library: LibraryRef::latest("sst"),
            budget_chars: 30, // fits the first excerpt (29 chars), not both
            excerpts: vec![
                DocExcerpt {
                    anchor: "a".into(),
                    text: "send({ to, subject })".into(),
                },
                DocExcerpt {
                    anchor: "b".into(),
                    text: "reply({ to, inReplyTo })".into(),
                },
            ],
        };
        fit_budget(&mut result);
        assert_eq!(result.excerpts.len(), 1);
        assert_eq!(result.excerpts[0].anchor, "a");
    }
}
