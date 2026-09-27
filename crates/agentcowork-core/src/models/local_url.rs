//! P27 — `local://` model URLs + resolver (exact, doc 79).
//!
//! Minted forms (all **derived** from the registry + installed runtimes,
//! never a hardcoded catalog):
//! - `local://hf/{publisher}/{model}:{quant}` — from [`ModelRegistry`]
//! - `local://ollama/{model}:{tag}` — installed ollama runtimes
//! - `local://llamafile/{name}` — installed llamafile runtimes
//!
//! The broker resolves an id → runtime/endpoint via [`LocalUrlResolver`].

use std::fmt;
use std::str::FromStr;

use super::store::ModelRegistry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalUrl {
    /// `local://hf/{publisher}/{model}:{quant}`
    Hf {
        publisher: String,
        model: String,
        quant: String,
    },
    /// `local://ollama/{model}:{tag}`
    Ollama { model: String, tag: String },
    /// `local://llamafile/{name}`
    Llamafile { name: String },
}

impl LocalUrl {
    pub fn mint_hf(publisher: &str, model: &str, quant: &str) -> Self {
        LocalUrl::Hf {
            publisher: publisher.to_string(),
            model: model.to_string(),
            quant: quant.to_string(),
        }
    }

    pub fn mint_ollama(model: &str, tag: &str) -> Self {
        LocalUrl::Ollama {
            model: model.to_string(),
            tag: tag.to_string(),
        }
    }

    pub fn mint_llamafile(name: &str) -> Self {
        LocalUrl::Llamafile {
            name: name.to_string(),
        }
    }
}

impl fmt::Display for LocalUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocalUrl::Hf {
                publisher,
                model,
                quant,
            } => write!(f, "local://hf/{publisher}/{model}:{quant}"),
            LocalUrl::Ollama { model, tag } => write!(f, "local://ollama/{model}:{tag}"),
            LocalUrl::Llamafile { name } => write!(f, "local://llamafile/{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalUrlError {
    NotLocal,
    UnknownScheme(String),
    Malformed(String),
}

impl FromStr for LocalUrl {
    type Err = LocalUrlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s.strip_prefix("local://").ok_or(LocalUrlError::NotLocal)?;
        let (scheme, body) = rest.split_once('/').unwrap_or((rest, ""));
        match scheme {
            "hf" => {
                let (publisher, model_part) = body
                    .split_once('/')
                    .ok_or_else(|| LocalUrlError::Malformed(s.to_string()))?;
                let (model, quant) = model_part
                    .split_once(':')
                    .ok_or_else(|| LocalUrlError::Malformed(s.to_string()))?;
                if publisher.is_empty() || model.is_empty() || quant.is_empty() {
                    return Err(LocalUrlError::Malformed(s.to_string()));
                }
                Ok(LocalUrl::Hf {
                    publisher: publisher.to_string(),
                    model: model.to_string(),
                    quant: quant.to_string(),
                })
            }
            "ollama" => {
                let (model, tag) = body
                    .split_once(':')
                    .ok_or_else(|| LocalUrlError::Malformed(s.to_string()))?;
                Ok(LocalUrl::Ollama {
                    model: model.to_string(),
                    tag: tag.to_string(),
                })
            }
            "llamafile" => {
                if body.is_empty() {
                    return Err(LocalUrlError::Malformed(s.to_string()));
                }
                Ok(LocalUrl::Llamafile {
                    name: body.to_string(),
                })
            }
            other => Err(LocalUrlError::UnknownScheme(other.to_string())),
        }
    }
}

/// A resolved runtime endpoint for a `local://` URL.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEndpoint {
    pub provider: String, // "ollama" | "llamafile"
    pub base_url: String,
    pub model: String,
}

/// Resolves `local://` URLs from the registry + installed runtimes.
pub struct LocalUrlResolver {
    registry: ModelRegistry,
    /// e.g. `http://127.0.0.1:11434`
    ollama_base: String,
    /// e.g. `http://127.0.0.1:11435`
    llamafile_base: String,
    /// Installed ollama models (name:tag → true).
    installed_ollama: std::collections::HashSet<String>,
    /// Installed llamafile names.
    installed_llamafile: std::collections::HashSet<String>,
}

impl LocalUrlResolver {
    pub fn new(
        registry: ModelRegistry,
        ollama_base: String,
        llamafile_base: String,
        installed_ollama: Vec<(String, String)>,
        installed_llamafile: Vec<String>,
    ) -> Self {
        Self {
            registry,
            ollama_base,
            llamafile_base,
            installed_ollama: installed_ollama
                .into_iter()
                .map(|(m, t)| format!("{m}:{t}"))
                .collect(),
            installed_llamafile: installed_llamafile.into_iter().collect(),
        }
    }

    /// Resolve a URL to a runtime endpoint. Hf URLs require the file to be in
    /// the registry (derived, not assumed); Ollama/Llamafile require the
    /// runtime to be installed. Fail-closed otherwise.
    pub fn resolve(&self, url: &LocalUrl) -> Result<ResolvedEndpoint, LocalUrlError> {
        match url {
            LocalUrl::Hf {
                publisher,
                model,
                quant,
            } => {
                let id = format!("{publisher}/{model}:{quant}");
                self.registry
                    .get(&id)
                    .map(|_| ResolvedEndpoint {
                        provider: "llamafile".into(),
                        base_url: self.llamafile_base.clone(),
                        model: id.clone(),
                    })
                    .ok_or_else(|| LocalUrlError::Malformed(format!("not in registry: {id}")))
            }
            LocalUrl::Ollama { model, tag } => {
                let key = format!("{model}:{tag}");
                if !self.installed_ollama.contains(&key) {
                    return Err(LocalUrlError::Malformed(format!(
                        "ollama model not installed: {key}"
                    )));
                }
                Ok(ResolvedEndpoint {
                    provider: "ollama".into(),
                    base_url: self.ollama_base.clone(),
                    model: key,
                })
            }
            LocalUrl::Llamafile { name } => {
                if !self.installed_llamafile.contains(name) {
                    return Err(LocalUrlError::Malformed(format!(
                        "llamafile not installed: {name}"
                    )));
                }
                Ok(ResolvedEndpoint {
                    provider: "llamafile".into(),
                    base_url: self.llamafile_base.clone(),
                    model: name.clone(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_all_three_schemes() {
        for s in [
            "local://hf/microsoft/phi-4:q4_k_m",
            "local://ollama/qwen2.5:0.5b",
            "local://llamafile/llama3-8b",
        ] {
            let url: LocalUrl = s.parse().unwrap();
            assert_eq!(url.to_string(), s);
        }
    }

    #[test]
    fn parse_rejects_malformed_and_unknown() {
        assert!(matches!(
            "https://example.com/x".parse::<LocalUrl>(),
            Err(LocalUrlError::NotLocal)
        ));
        assert!(matches!(
            "local://foo/x".parse::<LocalUrl>(),
            Err(LocalUrlError::UnknownScheme(_))
        ));
        assert!(matches!(
            "local://hf/microsoft/phi-4".parse::<LocalUrl>(),
            Err(LocalUrlError::Malformed(_))
        ));
    }

    #[test]
    fn resolver_derives_from_registry_and_runtimes() {
        let base = std::env::temp_dir().join(format!("eaios-lu-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut reg = ModelRegistry::new(base.clone());
        reg.add(crate::models::store::ModelEntry {
            id: "microsoft/phi-4:q4_k_m".into(),
            path: "/tmp/phi.gguf".into(),
            sha256: "00".repeat(32),
            size: 1,
            ctx: 16384,
            quant: "q4_k_m".into(),
            source: "hf".into(),
        });
        let res = LocalUrlResolver::new(
            reg,
            "http://127.0.0.1:11434".into(),
            "http://127.0.0.1:11435".into(),
            vec![("qwen2.5".into(), "0.5b".into())],
            vec!["llama3-8b".into()],
        );

        let hf = res
            .resolve(&"local://hf/microsoft/phi-4:q4_k_m".parse().unwrap())
            .unwrap();
        assert_eq!(hf.provider, "llamafile");
        assert_eq!(hf.model, "microsoft/phi-4:q4_k_m");

        let ol = res
            .resolve(&"local://ollama/qwen2.5:0.5b".parse().unwrap())
            .unwrap();
        assert_eq!(ol.provider, "ollama");
        assert_eq!(ol.base_url, "http://127.0.0.1:11434");

        let lf = res
            .resolve(&"local://llamafile/llama3-8b".parse().unwrap())
            .unwrap();
        assert_eq!(lf.provider, "llamafile");

        // Fail-closed: not installed / not in registry.
        assert!(
            res.resolve(&"local://ollama/not-installed:1".parse().unwrap())
                .is_err()
        );
        assert!(
            res.resolve(&"local://hf/other/model:q8".parse().unwrap())
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
