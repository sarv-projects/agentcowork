//! Taste profile (C9, Algorithm #31 — Command Code taste-1 pattern, doc 37:
//! proprietary, **pattern only**). Auto-learned coding preferences as
//! confidence-scored symbolic rules; injected as a stable-prefix prior at
//! generation; exported/imported as shareable markdown.

use std::path::Path;

use crate::MemoryError;

#[derive(Debug, Clone, PartialEq)]
pub struct TasteRule {
    pub category: String,
    pub key: String,
    pub value: String,
    /// 0..1 confidence.
    pub confidence: f64,
    pub evidence: u32,
}

impl TasteRule {
    fn clamp_confidence(c: f64) -> f64 {
        c.clamp(0.0, 1.0)
    }

    pub fn observe_accept(&mut self) {
        self.confidence = Self::clamp_confidence(self.confidence + (1.0 - self.confidence) * 0.2);
        self.evidence = self.evidence.saturating_add(1);
    }

    pub fn observe_reject(&mut self) {
        self.confidence = Self::clamp_confidence(self.confidence * 0.8);
        self.evidence = self.evidence.saturating_add(1);
    }

    pub fn observe_edit(&mut self, new_value: String) {
        self.value = new_value;
        self.confidence = Self::clamp_confidence(self.confidence + (1.0 - self.confidence) * 0.3);
        self.evidence = self.evidence.saturating_add(1);
    }
}

#[derive(Debug, Clone, Default)]
pub struct TasteStore {
    rules: Vec<TasteRule>,
}

impl TasteStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rules(&self) -> &[TasteRule] {
        &self.rules
    }

    fn index_of(&self, category: &str, key: &str) -> Option<usize> {
        self.rules
            .iter()
            .position(|r| r.category == category && r.key == key)
    }

    /// Insert or update a rule's value (keeps existing confidence/evidence).
    pub fn upsert(&mut self, category: &str, key: &str, value: &str) {
        match self.index_of(category, key) {
            Some(i) => self.rules[i].value = value.to_string(),
            None => self.rules.push(TasteRule {
                category: category.to_string(),
                key: key.to_string(),
                value: value.to_string(),
                confidence: 0.5,
                evidence: 1,
            }),
        }
    }

    pub fn observe_accept(&mut self, category: &str, key: &str) {
        if let Some(i) = self.index_of(category, key) {
            self.rules[i].observe_accept();
        }
    }

    pub fn observe_reject(&mut self, category: &str, key: &str) {
        if let Some(i) = self.index_of(category, key) {
            self.rules[i].observe_reject();
        }
    }

    pub fn observe_edit(&mut self, category: &str, key: &str, new_value: &str) {
        match self.index_of(category, key) {
            Some(i) => self.rules[i].observe_edit(new_value.to_string()),
            None => self.upsert(category, key, new_value),
        }
    }

    /// The stable-prefix symbolic prior injected at generation time:
    /// highest-confidence rules first.
    pub fn inject_stable_prefix(&self) -> String {
        let mut rules = self.rules.clone();
        rules.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut out = String::from("# Taste Profile (auto-learned)\n");
        for r in &rules {
            out.push_str(&format!(
                "- [{}] `{}` = \"{}\" (confidence {:.2})\n",
                r.category, r.key, r.value, r.confidence
            ));
        }
        out
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Taste Profile\n");
        let mut cats: Vec<&String> = self.rules.iter().map(|r| &r.category).collect();
        cats.sort();
        cats.dedup();
        for cat in cats {
            out.push_str(&format!("\n## {cat}\n"));
            let mut rules: Vec<&TasteRule> =
                self.rules.iter().filter(|r| r.category == *cat).collect();
            rules.sort_by(|a, b| a.key.cmp(&b.key));
            for r in rules {
                out.push_str(&format!(
                    "- `{}` = \"{}\" (confidence {:.2}, evidence {})\n",
                    r.key, r.value, r.confidence, r.evidence
                ));
            }
        }
        out
    }

    pub fn from_markdown(md: &str) -> Self {
        let mut store = Self::new();
        let mut category = String::from("general");
        for line in md.lines() {
            let t = line.trim();
            if t.starts_with("## ") {
                category = t.strip_prefix("## ").unwrap_or(t).trim().to_string();
            } else if t.starts_with("- `") {
                // - `key` = "value" (confidence 0.80, evidence 5)
                if let Some(rest) = t.strip_prefix("- `") {
                    if let Some(kend) = rest.find("` = ") {
                        let key = &rest[..kend];
                        let after = &rest[kend + 4..];
                        if let Some(vstart) = after.find('"') {
                            let after_quote = &after[vstart + 1..];
                            if let Some(vend) = after_quote.find('"') {
                                let value = &after_quote[..vend];
                                store.upsert(&category, key, value);
                                // Parse optional confidence/evidence.
                                if let Some(paren) = after_quote[vend..].find("(confidence ") {
                                    let s = &after_quote[vend + paren + "(confidence ".len()..];
                                    let num = s.split([',', ')']).next().unwrap_or("").trim();
                                    if let Ok(c) = num.parse::<f64>() {
                                        if let Some(i) = store.index_of(&category, key) {
                                            store.rules[i].confidence = c;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        store
    }

    /// Persist to `<dir>/profile.md` (global: `~/.everyaios/taste/`; per-repo:
    /// `.agentcowork-taste/` — same format, different directory).
    pub fn save(&self, dir: &Path) -> Result<(), MemoryError> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("profile.md"), self.to_markdown())?;
        Ok(())
    }

    pub fn load(dir: &Path) -> Result<Self, MemoryError> {
        let md = std::fs::read_to_string(dir.join("profile.md"))?;
        Ok(Self::from_markdown(&md))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learn_accept_edit_reject() {
        let mut store = TasteStore::new();
        store.upsert("style", "naming", "snake_case");
        store.observe_accept("style", "naming");
        store.observe_accept("style", "naming");
        let c = store.rules()[0].confidence;
        assert!(c > 0.5 && c <= 1.0);

        store.observe_edit("style", "naming", "camelCase");
        assert_eq!(store.rules()[0].value, "camelCase");
        assert!(store.rules()[0].confidence >= c);

        let before = store.rules()[0].confidence;
        store.observe_reject("style", "naming");
        assert!(store.rules()[0].confidence < before);
    }

    #[test]
    fn stable_prefix_orders_by_confidence() {
        let mut store = TasteStore::new();
        store.upsert("style", "a", "x");
        store.upsert("style", "b", "y");
        for _ in 0..10 {
            store.observe_accept("style", "b");
        }
        let prefix = store.inject_stable_prefix();
        let pos_b = prefix.find("`b`").unwrap();
        let pos_a = prefix.find("`a`").unwrap();
        assert!(pos_b < pos_a);
        assert!(prefix.starts_with("# Taste Profile"));
    }

    #[test]
    fn markdown_round_trip() {
        let mut store = TasteStore::new();
        store.upsert("style", "naming", "snake_case");
        store.observe_accept("style", "naming");
        store.observe_accept("style", "naming");
        store.upsert("framework", "react", "server components");
        let md = store.to_markdown();
        let back = TasteStore::from_markdown(&md);
        assert_eq!(back.rules().len(), 2);
        assert_eq!(
            back.index_of("style", "naming")
                .map(|i| &back.rules()[i].value),
            Some(&"snake_case".to_string())
        );
        // Confidence survives the round trip (within formatting precision).
        let orig = store.rules()[store.index_of("style", "naming").unwrap()].confidence;
        let backc = back.rules()[back.index_of("style", "naming").unwrap()].confidence;
        assert!((orig - backc).abs() < 0.01);
    }

    #[test]
    fn save_load_to_directory() {
        let dir = std::env::temp_dir().join(format!("agentcowork-taste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = TasteStore::new();
        store.upsert("style", "k", "v");
        store.save(&dir).unwrap();
        let back = TasteStore::load(&dir).unwrap();
        assert_eq!(back.rules().len(), 1);
        assert_eq!(back.rules()[0].key, "k");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
