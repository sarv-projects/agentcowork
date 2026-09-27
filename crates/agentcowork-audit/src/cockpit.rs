//! P3.2 — Cockpit / Ambient Flight Deck live state (H2, doc 33 §9.5).
//!
//! The in-memory dashboard state the shell + UI render: "running now" agent
//! cards (status, model, token counters, elapsed, action trail), the
//! slide-over panel data, MCQ interrupt cards for circuit-break, and the
//! quiet-mode single-sentence status line for the tray.
//!
//! Pure logic, no I/O: the coordinator/sidecar feeds it (via the control
//! channel seam), the UI polls it, and every rule here is unit-tested.

use serde::{Deserialize, Serialize};

/// How many trailing actions each agent card keeps in its trail.
const ACTION_TRAIL_CAP: usize = 12;

/// Agent status shown on the card + in the quiet line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Actively executing (the LIVE chip).
    Running,
    /// Paused on a circuit-break MCQ interrupt.
    Waiting,
    /// Finished cleanly.
    Done,
    /// Finished with an error.
    Failed,
    /// Not currently active.
    Idle,
}

/// One entry in an agent's action trail ("Updating report…", "browser.act"…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveAction {
    /// UNIX timestamp (ms).
    pub ts_ms: u64,
    /// Tool/verb name (e.g. `browser.act`, `office.patch`, `memory.recall`).
    pub tool: String,
    /// Human-readable summary — the quiet-line sentence fragment.
    pub summary: String,
}

/// Per-agent token counters (the slide-over token counters).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounters {
    pub tokens_in: u64,
    pub tokens_out: u64,
}

impl TokenCounters {
    pub fn total(self) -> u64 {
        self.tokens_in + self.tokens_out
    }
}

/// One "running now" agent card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCard {
    pub agent_id: String,
    pub label: String,
    pub model: String,
    pub provider: String,
    pub status: AgentStatus,
    pub tokens: TokenCounters,
    /// UNIX timestamp (ms) the agent started.
    pub started_ms: u64,
    /// UNIX timestamp (ms) of the most recent action.
    pub last_action_ms: u64,
    pub actions: Vec<LiveAction>,
}

impl AgentCard {
    pub fn new(
        agent_id: impl Into<String>,
        label: impl Into<String>,
        model: impl Into<String>,
        provider: impl Into<String>,
        started_ms: u64,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            label: label.into(),
            model: model.into(),
            provider: provider.into(),
            status: AgentStatus::Idle,
            tokens: TokenCounters::default(),
            started_ms,
            last_action_ms: started_ms,
            actions: Vec::new(),
        }
    }

    /// Append a live action, cap the trail, and mark the agent Running.
    pub fn record_action(
        &mut self,
        ts_ms: u64,
        tool: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.actions.push(LiveAction {
            ts_ms,
            tool: tool.into(),
            summary: summary.into(),
        });
        if self.actions.len() > ACTION_TRAIL_CAP {
            self.actions.remove(0);
        }
        self.status = AgentStatus::Running;
        self.last_action_ms = ts_ms;
    }

    pub fn add_tokens(&mut self, tokens_in: u64, tokens_out: u64) {
        self.tokens.tokens_in += tokens_in;
        self.tokens.tokens_out += tokens_out;
    }

    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.started_ms)
    }

    /// The most recent action summary (for the quiet tray line), or the
    /// label when the trail is empty.
    pub fn latest_summary(&self) -> &str {
        self.actions
            .last()
            .map(|a| a.summary.as_str())
            .unwrap_or(&self.label)
    }
}

/// A circuit-break MCQ interrupt card: the agent hit a risky/uncertain point
/// and offers the user 4 actionable choices (doc 33 §9.5: skip/retry/
/// escalate/manual).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptCard {
    pub id: String,
    pub agent_id: String,
    pub prompt: String,
    /// Exactly the actionable options (skip / retry / escalate / manual…).
    pub options: Vec<String>,
    /// Index into `options` once the user responded; `None` while open.
    pub responded: Option<usize>,
    /// UNIX timestamp (ms) the card was presented.
    pub created_ms: u64,
}

/// The full cockpit snapshot the UI renders (serde-serializable for the
/// `cockpit_snapshot` Tauri command).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CockpitState {
    pub agents: Vec<AgentCard>,
    pub interrupts: Vec<InterruptCard>,
    /// Quiet mode: the cockpit collapses to a single-sentence tray status.
    pub quiet: bool,
}

impl CockpitState {
    /// Insert or update an agent card (upsert by agent_id).
    pub fn upsert_agent(&mut self, card: AgentCard) {
        if let Some(existing) = self.agents.iter_mut().find(|a| a.agent_id == card.agent_id) {
            *existing = card;
        } else {
            self.agents.push(card);
        }
    }

    /// Record a live action on an agent (creating a Running card if absent —
    /// the coordinator's first tool call brings the card to life).
    pub fn agent_action(
        &mut self,
        ts_ms: u64,
        agent_id: impl Into<String>,
        tool: impl Into<String>,
        summary: impl Into<String>,
    ) {
        let agent_id = agent_id.into();
        if let Some(agent) = self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            agent.record_action(ts_ms, tool, summary);
        } else {
            let mut card = AgentCard::new(agent_id.clone(), agent_id, "", "", ts_ms);
            card.record_action(ts_ms, tool, summary);
            self.agents.push(card);
        }
    }

    pub fn agent_tokens(&mut self, agent_id: &str, tokens_in: u64, tokens_out: u64) -> bool {
        match self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            Some(a) => {
                a.add_tokens(tokens_in, tokens_out);
                true
            }
            None => false,
        }
    }

    /// STOP: mark the agent Done (killed). The kill itself is the control
    /// channel's `agent/stop`; this mirrors it into the cockpit state.
    pub fn stop(&mut self, agent_id: &str) -> bool {
        match self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            Some(a) => {
                a.status = AgentStatus::Done;
                true
            }
            None => false,
        }
    }

    /// UNDO: request revert of the last action. The revert itself is the
    /// control channel's `agent/undo`; this records the request + marks the
    /// card as Waiting on the revert so the user sees feedback.
    pub fn undo(&mut self, agent_id: &str) -> bool {
        match self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            Some(a) => {
                a.status = AgentStatus::Waiting;
                a.actions.push(LiveAction {
                    ts_ms: now_ms(),
                    tool: "agent.undo".into(),
                    summary: "reverting last action…".into(),
                });
                true
            }
            None => false,
        }
    }

    /// Aggregate token counters across all cards (slide-over totals).
    pub fn token_totals(&self) -> TokenCounters {
        self.agents
            .iter()
            .fold(TokenCounters::default(), |acc, a| TokenCounters {
                tokens_in: acc.tokens_in + a.tokens.tokens_in,
                tokens_out: acc.tokens_out + a.tokens.tokens_out,
            })
    }

    /// The single-sentence quiet-mode status line (tray tooltip), e.g.
    /// `AgentCowork: Updating report…` or `AgentCowork: idle`.
    pub fn quiet_status(&self, now_ms: u64) -> String {
        let mut latest: Option<(&AgentCard, u64)> = None;
        for a in &self.agents {
            let t = match a.status {
                AgentStatus::Running | AgentStatus::Waiting => a.last_action_ms,
                _ => continue,
            };
            if latest.map(|(_, lt)| t > lt).unwrap_or(true) {
                latest = Some((a, t));
            }
        }
        match latest {
            Some((a, _)) => format!(
                "AgentCowork: {} — {}s",
                a.latest_summary(),
                a.elapsed_ms(now_ms) / 1000
            ),
            None => "AgentCowork: idle".to_string(),
        }
    }

    /// Present a circuit-break MCQ interrupt card (4 actionable options).
    pub fn present_interrupt(
        &mut self,
        id: impl Into<String>,
        agent_id: impl Into<String>,
        prompt: impl Into<String>,
        options: Vec<String>,
        created_ms: u64,
    ) {
        debug_assert_eq!(
            options.len(),
            4,
            "MCQ interrupt cards take exactly 4 options"
        );
        let agent_id = agent_id.into();
        if let Some(agent) = self.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            agent.status = AgentStatus::Waiting;
        }
        self.interrupts.push(InterruptCard {
            id: id.into(),
            agent_id,
            prompt: prompt.into(),
            options,
            responded: None,
            created_ms,
        });
    }

    /// The user picked one of the 4 options. Returns the chosen text (for the
    /// control-channel `agent/interrupt-response` call) or `None` when the
    /// card is unknown/already answered.
    pub fn respond_interrupt(&mut self, id: &str, choice: usize) -> Option<String> {
        let card = self
            .interrupts
            .iter_mut()
            .find(|c| c.id == id && c.responded.is_none())?;
        if choice >= card.options.len() {
            return None;
        }
        card.responded = Some(choice);
        let chosen = card.options[choice].clone();
        // Back to Running — the loop continues with the chosen path.
        if let Some(agent) = self.agents.iter_mut().find(|a| a.agent_id == card.agent_id) {
            agent.status = AgentStatus::Running;
        }
        Some(chosen)
    }

    /// Open (unanswered) interrupt cards only.
    pub fn open_interrupts(&self) -> Vec<&InterruptCard> {
        self.interrupts
            .iter()
            .filter(|c| c.responded.is_none())
            .collect()
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_agent() -> CockpitState {
        let mut s = CockpitState::default();
        s.upsert_agent(AgentCard::new(
            "a1",
            "Researcher",
            "claude",
            "anthropic",
            1000,
        ));
        s
    }

    #[test]
    fn action_appends_trail_and_marks_running() {
        let mut s = state_with_agent();
        s.agent_action(2000, "a1", "browser.act", "Opening search results");
        let a = &s.agents[0];
        assert_eq!(a.status, AgentStatus::Running);
        assert_eq!(a.actions.len(), 1);
        assert_eq!(a.actions[0].tool, "browser.act");
        assert_eq!(a.last_action_ms, 2000);
    }

    #[test]
    fn trail_is_capped() {
        let mut s = state_with_agent();
        for i in 0..20 {
            s.agent_action(1000 + i, "a1", "tool", format!("step {i}"));
        }
        assert_eq!(s.agents[0].actions.len(), ACTION_TRAIL_CAP);
        assert_eq!(s.agents[0].actions[0].summary, "step 8"); // 20 - 12 = 8 dropped
        assert_eq!(s.agents[0].actions[11].summary, "step 19");
    }

    #[test]
    fn first_action_creates_card() {
        let mut s = CockpitState::default();
        s.agent_action(500, "spawned", "browser.act", "first move");
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].agent_id, "spawned");
        assert_eq!(s.agents[0].status, AgentStatus::Running);
    }

    #[test]
    fn token_counters_accumulate_and_total() {
        let mut s = state_with_agent();
        assert!(s.agent_tokens("a1", 100, 20));
        assert!(s.agent_tokens("a1", 50, 10));
        assert!(!s.agent_tokens("nope", 1, 1));
        let t = s.token_totals();
        assert_eq!((t.tokens_in, t.tokens_out, t.total()), (150, 30, 180));
    }

    #[test]
    fn quiet_status_single_sentence() {
        let mut s = state_with_agent();
        assert_eq!(s.quiet_status(2000), "AgentCowork: idle");
        s.agent_action(1500, "a1", "office.patch", "Updating report");
        assert_eq!(s.quiet_status(3000), "AgentCowork: Updating report — 2s");
        // Idle agents don't produce a status.
        s.agents[0].status = AgentStatus::Done;
        assert_eq!(s.quiet_status(3000), "AgentCowork: idle");
    }

    #[test]
    fn stop_and_undo_update_status() {
        let mut s = state_with_agent();
        s.agent_action(2000, "a1", "browser.act", "filling form");
        assert!(s.stop("a1"));
        assert_eq!(s.agents[0].status, AgentStatus::Done);
        assert!(!s.stop("missing"));

        s.agent_action(2500, "a1", "browser.act", "filling form again");
        assert!(s.undo("a1"));
        assert_eq!(s.agents[0].status, AgentStatus::Waiting);
        assert_eq!(s.agents[0].actions.last().unwrap().tool, "agent.undo");
    }

    #[test]
    fn interrupt_lifecycle_present_respond() {
        let mut s = state_with_agent();
        s.agent_action(2000, "a1", "browser.act", "about to send email");
        s.present_interrupt(
            "i1",
            "a1",
            "Send this email to the client?",
            vec![
                "Skip".into(),
                "Retry".into(),
                "Escalate".into(),
                "Do it manually".into(),
            ],
            2500,
        );
        // Presenting pauses the agent.
        assert_eq!(s.agents[0].status, AgentStatus::Waiting);
        assert_eq!(s.open_interrupts().len(), 1);

        // Bad choice rejected.
        assert!(s.respond_interrupt("i1", 9).is_none());
        // Good choice resumes + returns the chosen text.
        let chosen = s.respond_interrupt("i1", 1).unwrap();
        assert_eq!(chosen, "Retry");
        assert_eq!(s.agents[0].status, AgentStatus::Running);
        assert!(s.open_interrupts().is_empty());
        // Double-respond is a no-op.
        assert!(s.respond_interrupt("i1", 0).is_none());
    }

    #[test]
    fn upsert_replaces_and_preserves_identity() {
        let mut s = state_with_agent();
        s.agent_action(2000, "a1", "browser.act", "work");
        s.upsert_agent(AgentCard::new("a1", "Renamed", "gpt", "openai", 900));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].label, "Renamed");
        // A fresh upsert resets the trail (coordinator re-registers the agent).
        assert!(s.agents[0].actions.is_empty());
    }
}
