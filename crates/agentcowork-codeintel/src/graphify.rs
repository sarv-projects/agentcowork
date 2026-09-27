//! P25-2 — Graphify-style repository knowledge projection.
//! This is intentionally a lightweight adapter over the existing RepoMap
//! tags: code, docs, SQL, config, and PDF-like files become typed nodes and
//! lexical co-occurrence edges. A future parser can replace the classifier.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KnowledgeKind {
    Code,
    Documentation,
    Sql,
    Config,
    Pdf,
    Other,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeNode {
    pub path: String,
    pub kind: KnowledgeKind,
    pub symbols: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeEdge {
    pub from: String,
    pub to: String,
    pub label: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeGraph {
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
}
impl KnowledgeGraph {
    pub fn add_file(&mut self, path: &str, symbols: Vec<String>) {
        let kind = match path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "rs" | "ts" | "tsx" | "js" | "py" => KnowledgeKind::Code,
            "md" | "txt" => KnowledgeKind::Documentation,
            "sql" => KnowledgeKind::Sql,
            "toml" | "yaml" | "yml" | "json" => KnowledgeKind::Config,
            "pdf" => KnowledgeKind::Pdf,
            _ => KnowledgeKind::Other,
        };
        self.nodes.push(KnowledgeNode {
            path: path.into(),
            kind,
            symbols,
        });
    }
    pub fn link_shared_symbols(&mut self) {
        for i in 0..self.nodes.len() {
            for j in i + 1..self.nodes.len() {
                if self.nodes[i]
                    .symbols
                    .iter()
                    .any(|s| self.nodes[j].symbols.contains(s))
                {
                    self.edges.push(KnowledgeEdge {
                        from: self.nodes[i].path.clone(),
                        to: self.nodes[j].path.clone(),
                        label: "shared-symbol".into(),
                    });
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_cross_surface_graph() {
        let mut g = KnowledgeGraph::default();
        g.add_file("src/a.rs", vec!["Vault".into()]);
        g.add_file("docs/vault.md", vec!["Vault".into()]);
        g.add_file("schema.sql", vec!["Vault".into()]);
        g.link_shared_symbols();
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.nodes[2].kind, KnowledgeKind::Sql);
    }
}
