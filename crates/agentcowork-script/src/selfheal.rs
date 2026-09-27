//! P24-2 — governed self-healing helpers (browser-harness pattern).
//! Helpers are stored as data and only returned to the caller for explicit
//! execution through the existing sandbox; this module never evaluates code.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Helper {
    pub name: String,
    pub source: String,
    pub reason: String,
    pub version: u32,
}
#[derive(Debug, Clone, Default)]
pub struct HelperStore {
    items: Vec<Helper>,
    max: usize,
}
impl HelperStore {
    pub fn new(max: usize) -> Self {
        Self {
            items: Vec::new(),
            max: max.max(1),
        }
    }
    pub fn propose(&mut self, name: &str, source: &str, reason: &str) -> Result<Helper, String> {
        if name.trim().is_empty() || source.trim().is_empty() {
            return Err("helper name/source required".into());
        }
        if let Some(h) = self.items.iter_mut().find(|h| h.name == name) {
            h.source = source.into();
            h.reason = reason.into();
            h.version += 1;
            return Ok(h.clone());
        }
        if self.items.len() >= self.max {
            return Err("helper store limit reached".into());
        }
        let h = Helper {
            name: name.into(),
            source: source.into(),
            reason: reason.into(),
            version: 1,
        };
        self.items.push(h.clone());
        Ok(h)
    }
    pub fn get(&self, name: &str) -> Option<&Helper> {
        self.items.iter().find(|h| h.name == name)
    }
    pub fn list(&self) -> &[Helper] {
        &self.items
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_are_versioned_and_bounded() {
        let mut s = HelperStore::new(1);
        assert_eq!(s.propose("x", "return 1", "unknown").unwrap().version, 1);
        assert_eq!(s.propose("x", "return 2", "repair").unwrap().version, 2);
        assert!(s.propose("y", "return 3", "overflow").is_err());
    }
}
