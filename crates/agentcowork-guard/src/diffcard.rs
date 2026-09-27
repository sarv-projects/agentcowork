//! P7.5 — the **native OS diff card** (J3/H8, doc 52 §2 decision packages).
//! The approval *presentation* is a native OS surface (notification/dialog),
//! not webview JS: this module renders the full decision package + ticket
//! into a complete, human-readable card and binds the approve/reject actions
//! to the ticket's nonce — the same nonce the Tauri `guard_respond` command
//! requires, so a synthesized webview request can never approve a card.
//!
//! The card is platform-agnostic text (the OS dialog/notification renders
//! it); the same payload also drives the webview card so both surfaces show
//! exactly the same facts.

use crate::decision::DecisionPackage;
use crate::ticket::AuthorizationTicket;
use serde::{Deserialize, Serialize};

/// The native card's two actions. Approve requires the ticket nonce — the
/// host never approves from the card itself; it forwards the nonce to the
/// ticket store (the webview and the native dialog are equally incapable of
/// minting a valid nonce).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardAction {
    Approve,
    Reject,
}

/// The complete native OS diff card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCard {
    pub ticket_id: String,
    pub operation: String,
    pub tool_id: String,
    pub goal: String,
    pub risk: String,
    pub affected_paths: Vec<String>,
    pub proposed_diff: String,
    pub script_lines: Vec<String>,
    pub execution_target: String,
    pub env_vars: Vec<String>,
    pub network_destinations: Vec<String>,
    pub web_action: Option<String>,
    pub nonce: String,
    pub expires_at_ms: u64,
}

impl NativeCard {
    /// Build a native card from a pending ticket + its decision package.
    pub fn from_ticket(ticket: &AuthorizationTicket, decision: &DecisionPackage) -> NativeCard {
        NativeCard {
            ticket_id: ticket.ticket_id.clone(),
            operation: ticket.operation.clone(),
            tool_id: ticket.tool_id.clone(),
            goal: decision.goal.clone(),
            risk: format!("{:?}", ticket.risk),
            affected_paths: decision.affected_paths.clone(),
            proposed_diff: decision.proposed_diff.clone(),
            script_lines: decision.script_lines.clone(),
            execution_target: decision.execution_target.clone(),
            env_vars: decision.env_vars.clone(),
            network_destinations: decision.network_destinations.clone(),
            web_action: decision.web_action.map(|w| w.as_str().to_string()),
            nonce: ticket.approval_nonce.clone(),
            expires_at_ms: ticket.expires_at_ms,
        }
    }

    /// The approve/reject payload the host forwards to `guard_respond` —
    /// bound to this card's ticket id + nonce. The webview path already
    /// validates the nonce; the native path uses the identical shape.
    pub fn respond(&self, action: CardAction) -> CardResponse {
        CardResponse {
            ticket_id: self.ticket_id.clone(),
            nonce: self.nonce.clone(),
            action,
        }
    }
}

/// The nonce-bound response a native dialog produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardResponse {
    pub ticket_id: String,
    pub nonce: String,
    pub action: CardAction,
}

/// Render the complete human-readable card — every fact the user needs to
/// judge the action, exactly once. This is the text the native OS dialog /
/// notification shows (and the webview card mirrors field-for-field).
pub fn render_native_card(card: &NativeCard) -> String {
    let mut out = String::new();
    out.push_str(&format!("=== Guard-2 approval: {} ===\n", card.operation));
    out.push_str(&format!(
        "ticket: {} · tool: {} · risk: {}\n",
        card.ticket_id, card.tool_id, card.risk
    ));
    if !card.goal.is_empty() {
        out.push_str(&format!("goal: {}\n", card.goal));
    }
    if !card.affected_paths.is_empty() {
        out.push_str("paths:\n");
        for p in &card.affected_paths {
            out.push_str(&format!("  - {p}\n"));
        }
    }
    if !card.proposed_diff.is_empty() {
        out.push_str("diff:\n");
        for line in card.proposed_diff.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    if !card.script_lines.is_empty() {
        out.push_str("script:\n");
        for line in &card.script_lines {
            out.push_str(&format!("  $ {line}\n"));
        }
    }
    if !card.execution_target.is_empty() {
        out.push_str(&format!("execution target: {}\n", card.execution_target));
    }
    if !card.env_vars.is_empty() {
        out.push_str(&format!("env vars: {}\n", card.env_vars.join(", ")));
    }
    if !card.network_destinations.is_empty() {
        out.push_str(&format!(
            "data leaving device: {}\n",
            card.network_destinations.join(", ")
        ));
    }
    if let Some(wa) = &card.web_action {
        out.push_str(&format!(
            "WEB ACTION: {wa} — confirm explicitly before running\n"
        ));
    }
    out.push_str(&format!(
        "nonce: {} · expires: {}\n",
        card.nonce, card.expires_at_ms
    ));
    out.push_str("actions: [approve] [reject]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{DecisionPackage, WebActionKind};
    use crate::ticket::{ApprovalSource, AuthorizationTicket, RiskLevel};

    fn ticket(nonce: &str) -> AuthorizationTicket {
        AuthorizationTicket {
            ticket_id: "t-1".into(),
            agent_id: "primary".into(),
            session_id: "s-1".into(),
            tool_id: "office.convert".into(),
            operation: "convert".into(),
            args_hash: "abc".into(),
            paths: vec!["/tmp/office/report.docx".into()],
            risk: RiskLevel::High,
            expires_at_ms: 1_800_000_000_000,
            single_use: true,
            approval_source: ApprovalSource::Human,
            approval_nonce: nonce.into(),
            audit_seq: 0,
            state: crate::ticket::TicketState::Pending,
            bindings: vec![],
            execution_id: String::new(),
            action_id: String::new(),
            idempotency_key: String::new(),
        }
    }

    fn decision() -> DecisionPackage {
        DecisionPackage {
            goal: "convert report.docx → pdf".into(),
            proposed_diff: "- report.docx\n+ report.pdf".into(),
            risk: RiskLevel::High,
            affected_paths: vec!["/tmp/office/report.docx".into()],
            script_lines: vec!["office convert report.docx report.pdf".into()],
            execution_target: "office-tools".into(),
            env_vars: vec!["OFFICE_TMP=/tmp/office".into()],
            network_destinations: vec![],
            web_action: Some(WebActionKind::SensitiveSubmit),
            confidence: Some(0.9),
        }
    }

    #[test]
    fn card_carries_every_decision_fact() {
        let card = NativeCard::from_ticket(&ticket("nonce-1"), &decision());
        assert_eq!(card.ticket_id, "t-1");
        assert_eq!(card.nonce, "nonce-1");
        assert_eq!(card.web_action.as_deref(), Some("sensitive_submit"));
        assert!(
            card.affected_paths
                .contains(&"/tmp/office/report.docx".into())
        );
        let text = render_native_card(&card);
        // Every fact appears exactly once in the rendered card.
        assert!(text.contains("goal: convert report.docx → pdf"));
        assert!(text.contains("$ office convert report.docx report.pdf"));
        assert!(text.contains("WEB ACTION: sensitive_submit"));
        assert!(text.contains("nonce: nonce-1"));
    }

    #[test]
    fn response_is_nonce_bound() {
        let card = NativeCard::from_ticket(&ticket("nonce-42"), &decision());
        let r = card.respond(CardAction::Approve);
        assert_eq!(r.ticket_id, "t-1");
        assert_eq!(r.nonce, "nonce-42");
        assert_eq!(r.action, CardAction::Approve);
    }
}
