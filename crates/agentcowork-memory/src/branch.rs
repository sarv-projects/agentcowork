//! P36 (C2/C7) — branch/lineage memory. A session fork is not one global
//! chronological log: forking a session creates a **lineage branch** whose
//! retrieval sees the fork point plus its own turns — never the sibling
//! branch's future. Merging records ancestry so recall can walk a branch
//! history.

use serde::{Deserialize, Serialize};

/// One node in the session lineage tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageNode {
    pub session_id: String,
    /// The session this one forked from (`None` = root session).
    pub parent: Option<String>,
    /// The turn id at which the fork happened (the last shared turn).
    pub fork_point_turn: Option<String>,
    pub forked_at_ms: Option<u64>,
    /// Sibling sessions forked from the same parent (for branch listing).
    pub siblings: Vec<String>,
    /// Closed (merged back or abandoned) branches stop being recalled.
    pub closed: bool,
}

/// The lineage store. Pure in-memory; the coordinator persists it with the
/// session list.
#[derive(Debug, Clone, Default)]
pub struct Lineage {
    nodes: Vec<LineageNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LineageError {
    #[error("fork: parent session {0} does not exist")]
    UnknownParent(String),
    #[error("fork: session {0} already exists")]
    DuplicateSession(String),
    #[error("branch {0} is closed")]
    Closed(String),
}

impl Lineage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the root session.
    pub fn register_root(&mut self, session_id: &str) -> Result<(), LineageError> {
        if self.nodes.iter().any(|n| n.session_id == session_id) {
            return Err(LineageError::DuplicateSession(session_id.into()));
        }
        self.nodes.push(LineageNode {
            session_id: session_id.to_string(),
            parent: None,
            fork_point_turn: None,
            forked_at_ms: None,
            siblings: Vec::new(),
            closed: false,
        });
        Ok(())
    }

    /// Fork `child` from `parent` at the given turn. The parent gains a
    /// sibling-recorded child; the child starts empty (future-only), exactly
    /// like a git branch.
    pub fn fork(
        &mut self,
        parent: &str,
        child: &str,
        fork_point_turn: &str,
        at_ms: u64,
    ) -> Result<(), LineageError> {
        if self.nodes.iter().any(|n| n.session_id == child) {
            return Err(LineageError::DuplicateSession(child.into()));
        }
        let parent_node = self
            .nodes
            .iter_mut()
            .find(|n| n.session_id == parent)
            .ok_or_else(|| LineageError::UnknownParent(parent.to_string()))?;
        if parent_node.closed {
            return Err(LineageError::Closed(parent.to_string()));
        }
        parent_node.siblings.push(child.to_string());
        self.nodes.push(LineageNode {
            session_id: child.to_string(),
            parent: Some(parent.to_string()),
            fork_point_turn: Some(fork_point_turn.to_string()),
            forked_at_ms: Some(at_ms),
            siblings: Vec::new(),
            closed: false,
        });
        Ok(())
    }

    pub fn close(&mut self, session_id: &str) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.session_id == session_id) {
            n.closed = true;
        }
    }

    /// The node (None when unknown).
    pub fn get(&self, session_id: &str) -> Option<&LineageNode> {
        self.nodes.iter().find(|n| n.session_id == session_id)
    }

    /// The full ancestry chain root → … → session (oldest first).
    pub fn ancestry(&self, session_id: &str) -> Vec<&LineageNode> {
        let mut chain = Vec::new();
        let mut cur = self.get(session_id);
        while let Some(node) = cur {
            chain.push(node);
            cur = node.parent.as_deref().and_then(|p| self.get(p));
        }
        chain.reverse();
        chain
    }

    /// The complete branch universe this session may recall: its ancestry
    /// plus only its own fork path (never siblings' futures).
    pub fn recall_branch(&self, session_id: &str) -> Vec<&LineageNode> {
        self.ancestry(session_id)
    }

    /// Sibling sessions (the other futures a fork left behind — for the
    /// branch-switcher UI).
    pub fn siblings(&self, session_id: &str) -> Vec<&LineageNode> {
        let Some(node) = self.get(session_id) else {
            return Vec::new();
        };
        let Some(parent) = node.parent.as_deref() else {
            return Vec::new();
        };
        self.nodes
            .iter()
            .filter(|n| n.parent.as_deref() == Some(parent) && n.session_id != session_id)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_creates_branch_with_point() {
        let mut l = Lineage::new();
        l.register_root("a").unwrap();
        l.fork("a", "b", "t10", 1000).unwrap();
        let b = l.get("b").unwrap();
        assert_eq!(b.parent.as_deref(), Some("a"));
        assert_eq!(b.fork_point_turn.as_deref(), Some("t10"));
    }

    #[test]
    fn ancestry_walks_to_root() {
        let mut l = Lineage::new();
        l.register_root("a").unwrap();
        l.fork("a", "b", "t10", 1).unwrap();
        l.fork("b", "c", "t20", 2).unwrap();
        let chain = l.ancestry("c");
        let ids: Vec<&str> = chain.iter().map(|n| n.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn recall_branch_never_includes_sibling_future() {
        let mut l = Lineage::new();
        l.register_root("a").unwrap();
        l.fork("a", "b", "t10", 1).unwrap();
        l.fork("a", "c", "t10", 2).unwrap(); // sibling branch
        // c's recall branch is a → c; b's future is never visible to c.
        let recall: Vec<&str> = l
            .recall_branch("c")
            .iter()
            .map(|n| n.session_id.as_str())
            .collect();
        assert_eq!(recall, vec!["a", "c"]);
        assert!(!recall.contains(&"b"));
    }

    #[test]
    fn siblings_list_only_forked_neighbors() {
        let mut l = Lineage::new();
        l.register_root("a").unwrap();
        l.fork("a", "b", "t10", 1).unwrap();
        l.fork("a", "c", "t10", 2).unwrap();
        let sibs: Vec<&str> = l
            .siblings("b")
            .iter()
            .map(|n| n.session_id.as_str())
            .collect();
        assert_eq!(sibs, vec!["c"]);
        assert!(l.siblings("a").is_empty()); // root has no parent
    }

    #[test]
    fn duplicate_and_unknown_parent_errors() {
        let mut l = Lineage::new();
        l.register_root("a").unwrap();
        assert!(matches!(
            l.register_root("a"),
            Err(LineageError::DuplicateSession(_))
        ));
        assert!(matches!(
            l.fork("nope", "b", "t1", 0),
            Err(LineageError::UnknownParent(_))
        ));
        l.fork("a", "b", "t1", 0).unwrap();
        assert!(matches!(
            l.fork("a", "b", "t2", 0),
            Err(LineageError::DuplicateSession(_))
        ));
    }

    #[test]
    fn closed_branch_rejects_forks() {
        let mut l = Lineage::new();
        l.register_root("a").unwrap();
        l.close("a");
        assert!(matches!(
            l.fork("a", "b", "t1", 0),
            Err(LineageError::Closed(_))
        ));
    }
}
