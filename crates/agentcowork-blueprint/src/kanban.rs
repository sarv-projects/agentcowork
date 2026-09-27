//! H2 Kanban-of-agents (doc 69 §3 — `hermes kanban` steal): a local
//! multi-profile collaboration board — tasks, links, and a dispatcher that
//! assigns tasks to agents. Fleets, not one card: the planner can fan work
//! out across agents and watch each task move through the columns.
//!
//! Pure data + a deterministic dispatcher. The coordinator renders the
//! board; this module owns the state transitions and the assignment policy.

use serde::{Deserialize, Serialize};

/// The board columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Backlog,
    Ready,
    InProgress,
    Blocked,
    Done,
}

impl Column {
    /// The allowed forward transitions (no skipping Done).
    pub fn advance(self) -> Option<Column> {
        match self {
            Column::Backlog => Some(Column::Ready),
            Column::Ready => Some(Column::InProgress),
            Column::InProgress => Some(Column::Done),
            Column::Blocked => Some(Column::InProgress),
            Column::Done => None,
        }
    }
}

/// One task on the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanTask {
    pub id: String,
    pub title: String,
    /// Agent/harness id assigned to the task (the fleet member).
    pub assignee: String,
    pub column: Column,
    /// Link to a parent task (dependency / decomposition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Blocks/blocked-by edges (task ids) — the dispatcher respects these.
    #[serde(default)]
    pub blocks: Vec<String>,
}

/// The collaboration board.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KanbanBoard {
    pub tasks: Vec<KanbanTask>,
}

impl KanbanBoard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, task: KanbanTask) -> Result<(), String> {
        if self.tasks.iter().any(|t| t.id == task.id) {
            return Err(format!("task `{}` already exists", task.id));
        }
        self.tasks.push(task);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&KanbanTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Advance a task to the next column (no skipping; Done is terminal).
    pub fn advance(&mut self, id: &str) -> Result<Column, String> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| format!("task `{id}` not found"))?;
        let next = task
            .column
            .advance()
            .ok_or_else(|| format!("task `{id}` is already Done"))?;
        task.column = next;
        Ok(next)
    }

    pub fn tasks_in(&self, column: Column) -> Vec<&KanbanTask> {
        self.tasks.iter().filter(|t| t.column == column).collect()
    }

    /// Tasks whose blockers are all Done — ready to be dispatched.
    pub fn ready(&self) -> Vec<&KanbanTask> {
        self.tasks
            .iter()
            .filter(|t| t.column == Column::Ready)
            .filter(|t| {
                t.blocks
                    .iter()
                    .all(|b| self.get(b).is_some_and(|bt| bt.column == Column::Done))
            })
            .collect()
    }
}

/// The dispatcher: assigns ready tasks to fleet members, honoring the
/// per-agent concurrency cap. Deterministic — no randomness in the kernel.
#[derive(Debug, Clone)]
pub struct Dispatcher {
    /// Agent id → max concurrent InProgress tasks.
    pub capacity: Vec<(String, usize)>,
}

impl Dispatcher {
    pub fn new(capacity: Vec<(String, usize)>) -> Self {
        Self { capacity }
    }

    /// The default: everyone may run up to 2 tasks at once.
    pub fn uniform(capacity: usize) -> Self {
        Self::new(vec![("*".into(), capacity)])
    }

    fn cap_for(&self, agent: &str) -> usize {
        self.capacity
            .iter()
            .find(|(a, _)| a == agent)
            .map(|(_, c)| *c)
            .unwrap_or_else(|| {
                self.capacity
                    .iter()
                    .find(|(a, _)| a == "*")
                    .map(|(_, c)| *c)
                    .unwrap_or(1)
            })
    }

    /// Assign as many ready tasks as capacity allows. Returns the ids
    /// dispatched (moved Backlog/Ready → InProgress). Tasks whose assignee
    /// is at capacity stay Ready.
    pub fn dispatch(&mut self, board: &mut KanbanBoard) -> Vec<String> {
        let mut dispatched = Vec::new();
        let mut load: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for t in &board.tasks {
            if t.column == Column::InProgress {
                *load.entry(t.assignee.clone()).or_insert(0) += 1;
            }
        }
        let ready_ids: Vec<String> = board.ready().iter().map(|t| t.id.clone()).collect();
        for id in ready_ids {
            let Some(idx) = board.tasks.iter().position(|t| t.id == id) else {
                continue;
            };
            let assignee = board.tasks[idx].assignee.clone();
            let used = load.get(&assignee).copied().unwrap_or(0);
            if used >= self.cap_for(&assignee) {
                continue;
            }
            board.tasks[idx].column = Column::InProgress;
            *load.entry(assignee).or_insert(0) += 1;
            dispatched.push(id);
        }
        dispatched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, assignee: &str, column: Column, blocks: Vec<String>) -> KanbanTask {
        KanbanTask {
            id: id.into(),
            title: id.into(),
            assignee: assignee.into(),
            column,
            parent: None,
            blocks,
        }
    }

    #[test]
    fn advance_respects_column_order() {
        let mut b = KanbanBoard::new();
        b.add(task("t1", "a", Column::Backlog, vec![])).unwrap();
        assert_eq!(b.advance("t1").unwrap(), Column::Ready);
        assert_eq!(b.advance("t1").unwrap(), Column::InProgress);
        assert_eq!(b.advance("t1").unwrap(), Column::Done);
        assert!(b.advance("t1").is_err()); // Done is terminal
    }

    #[test]
    fn ready_respects_blockers() {
        let mut b = KanbanBoard::new();
        b.add(task("setup", "a", Column::InProgress, vec![]))
            .unwrap();
        b.add(task("build", "b", Column::Ready, vec!["setup".into()]))
            .unwrap();
        b.add(task("free", "b", Column::Ready, vec![])).unwrap();
        assert_eq!(b.ready().len(), 1);
        assert_eq!(b.ready()[0].id, "free");
    }

    #[test]
    fn dispatcher_respects_capacity() {
        let mut b = KanbanBoard::new();
        b.add(task("t1", "a", Column::Ready, vec![])).unwrap();
        b.add(task("t2", "a", Column::Ready, vec![])).unwrap();
        b.add(task("t3", "a", Column::Ready, vec![])).unwrap();
        b.add(task("t4", "b", Column::Ready, vec![])).unwrap();
        let mut d = Dispatcher::new(vec![("a".into(), 2), ("b".into(), 1)]);
        let mut dispatched = d.dispatch(&mut b);
        assert_eq!(dispatched.len(), 3);
        dispatched.sort();
        assert_eq!(dispatched, vec!["t1", "t2", "t4"]);
        assert_eq!(b.tasks_in(Column::InProgress).len(), 3);
        // t3 stays Ready: a is at capacity.
        assert_eq!(b.tasks_in(Column::Ready).len(), 1);
    }
}
