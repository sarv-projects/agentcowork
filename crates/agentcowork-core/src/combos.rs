//! P51.20 — skill/integration combos with per-scope grants.
//!
//! A [`Combo`] bundles skills + integrations, but every sensitive scope needs
//! its own grant: approving one scope never approves another. Install requires
//! [`all_approved`], and reads go through the approve-then-read gate
//! ([`request_read`]).

use serde::{Deserialize, Serialize};

/// One scope's grant: denied until [`approve_scope`] records a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerScopeGrant {
    pub scope: String,
    pub granted: bool,
    pub ticket_id: Option<String>,
}

impl PerScopeGrant {
    pub fn denied(scope: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            granted: false,
            ticket_id: None,
        }
    }
}

/// A skill/integration combo awaiting per-scope approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Combo {
    pub id: String,
    pub skill_ids: Vec<String>,
    pub integration_ids: Vec<String>,
    pub grants: Vec<PerScopeGrant>,
}

impl Combo {
    pub fn new(
        id: impl Into<String>,
        skill_ids: Vec<String>,
        integration_ids: Vec<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            skill_ids,
            integration_ids,
            grants: scopes.into_iter().map(PerScopeGrant::denied).collect(),
        }
    }
}

/// The approve-then-read verdict for one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadVerdict {
    Allow,
    NeedsTicket,
}

/// Grant one scope on `combo`, recording the approval `ticket_id`.
/// Scoping is exact: only the named scope flips; every other grant is
/// untouched. Approving an unknown scope records it as granted so a later
/// [`all_approved`] check sees the explicit decision.
pub fn approve_scope(combo: &mut Combo, scope: &str, ticket_id: &str) {
    match combo.grants.iter_mut().find(|g| g.scope == scope) {
        Some(g) => {
            g.granted = true;
            g.ticket_id = Some(ticket_id.to_string());
        }
        None => combo.grants.push(PerScopeGrant {
            scope: scope.to_string(),
            granted: true,
            ticket_id: Some(ticket_id.to_string()),
        }),
    }
}

/// Install gate: true only when every scope grant is approved.
pub fn all_approved(combo: &Combo) -> bool {
    combo.grants.iter().all(|g| g.granted)
}

/// A saved bundle of steps. One click runs the list only after every
/// read scope the bundle names has been approved. The click does not
/// bypass Guard: an unapproved scope returns [`ReadVerdict::NeedsTicket`]
/// and runs nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepBundle {
    pub name: String,
    pub steps: Vec<String>,
    pub read_scopes: Vec<String>,
}

/// Run a saved combo. Returns the steps in order when every read scope is
/// granted. Otherwise returns the first scope that still needs a ticket.
pub fn run_saved_combo(bundle: &StepBundle, combo: &Combo) -> Result<Vec<String>, String> {
    for scope in &bundle.read_scopes {
        if request_read(combo, scope) != ReadVerdict::Allow {
            return Err(scope.clone());
        }
    }
    Ok(bundle.steps.clone())
}

/// Approve-then-read: [`ReadVerdict::Allow`] iff `scope` is granted,
/// otherwise [`ReadVerdict::NeedsTicket`].
pub fn request_read(combo: &Combo, scope: &str) -> ReadVerdict {
    match combo.grants.iter().find(|g| g.scope == scope) {
        Some(g) if g.granted => ReadVerdict::Allow,
        _ => ReadVerdict::NeedsTicket,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo() -> Combo {
        Combo::new(
            "combo-1",
            vec!["skill-a".to_string()],
            vec!["integ-b".to_string()],
            vec!["scope:read".to_string(), "scope:write".to_string()],
        )
    }

    #[test]
    fn per_scope_grant_isolated() {
        let mut c = combo();
        approve_scope(&mut c, "scope:read", "tkt-1");
        assert_eq!(request_read(&c, "scope:read"), ReadVerdict::Allow);
        // The sibling scope is untouched by the grant.
        assert_eq!(request_read(&c, "scope:write"), ReadVerdict::NeedsTicket);
        assert!(!all_approved(&c));
    }

    #[test]
    fn combo_install_requires_all_scopes() {
        let mut c = combo();
        assert!(!all_approved(&c));
        approve_scope(&mut c, "scope:read", "tkt-1");
        assert!(!all_approved(&c), "one of two scopes is not enough");
        approve_scope(&mut c, "scope:write", "tkt-2");
        assert!(all_approved(&c));
    }

    #[test]
    fn read_denied_without_grant() {
        let c = combo();
        assert_eq!(request_read(&c, "scope:read"), ReadVerdict::NeedsTicket);
        assert_eq!(request_read(&c, "scope:unknown"), ReadVerdict::NeedsTicket);
    }

    #[test]
    fn saved_combo_runs_only_after_its_read_scopes_are_approved() {
        let mut c = combo();
        let bundle = StepBundle {
            name: "read-thread".into(),
            steps: vec!["open thread".into(), "summarize".into()],
            read_scopes: vec!["scope:read".into()],
        };
        assert!(run_saved_combo(&bundle, &c).is_err());
        approve_scope(&mut c, "scope:read", "tkt-1");
        assert_eq!(
            run_saved_combo(&bundle, &c).unwrap(),
            vec!["open thread".to_string(), "summarize".to_string()]
        );
    }
}
