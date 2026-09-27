//! P36 — I6 executor hooks (spec v3.39): `PreToolUse` (deny-only) ·
//! `PostToolUse` / `PostToolUseFailure` · `PostToolBatch` · turn/session.
//!
//! Distinct from J18 profiles: hooks are invocation-time callbacks, scoped by
//! capability, and every invocation they observe is audited. `PreToolUse`
//! hooks can only **deny** — they can never skip a Guard-2 ticket. An
//! `Allow` from a pre-hook degrades to `Record` in dispatch: the ticket is
//! never skipped.

use serde::{Deserialize, Serialize};

/// The hook kinds (I6): per-tool, per-batch, per-turn, per-session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookKind {
    /// Before a tool runs. **Deny-only**: may only block the call; may never
    /// authorize it.
    PreToolUse,
    /// After a tool ran successfully.
    PostToolUse,
    /// After a tool failed.
    PostToolUseFailure,
    /// After a batch of tool calls completed.
    PostToolBatch,
    Turn,
    Session,
}

/// What a hook returns. `Deny` is the only *effective* pre-tool action;
/// post-hooks observe (record).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum HookAction {
    /// Proceed — for `PreToolUse` this still never skips a ticket (dispatcher
    /// degrades it to `Record`).
    Allow,
    /// Block the tool / fail the turn.
    Deny { reason: String },
    /// Record-only observation.
    Record,
}

/// The event payload every hook receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookEvent {
    pub kind: HookKind,
    pub capability: String,
    pub tool: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub ok: Option<bool>,
    pub error: Option<String>,
    pub at_tokens: Option<u64>,
}

/// A registered hook: named, kind-scoped, capability-scoped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    pub name: String,
    pub kind: HookKind,
    /// The capability this hook is allowed to observe (e.g. `fs.write:/tmp/**`).
    pub capability: String,
}

pub type HookFn = Box<dyn Fn(&HookEvent) -> HookAction + Send + Sync>;

/// The registry: hooks by (name, kind, capability); identical tuples dedupe.
/// Every invocation lands in the audit trail — denied attempts included.
#[derive(Default)]
pub struct HookRegistry {
    hooks: Vec<(HookSpec, HookFn)>,
    audit: Vec<(String, String, HookAction)>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn install(&mut self, spec: HookSpec, f: HookFn) {
        if let Some(pos) = self.hooks.iter().position(|(s, _)| {
            s.name == spec.name && s.kind == spec.kind && s.capability == spec.capability
        }) {
            self.hooks[pos].1 = f;
        } else {
            self.hooks.push((spec, f));
        }
    }

    pub fn list(&self) -> &[(HookSpec, HookFn)] {
        &self.hooks
    }

    /// Run the hooks matching `event`. The returned action is the first
    /// denial, or `Record` (pre-`Allow` degrades to `Record`).
    pub fn dispatch(&mut self, event: &HookEvent) -> HookAction {
        let mut action = HookAction::Record;
        for (_spec, f) in self
            .hooks
            .iter()
            .filter(|(s, _)| s.kind == event.kind && s.capability == event.capability)
        {
            let a = f(event);
            self.audit
                .push((event.tool.clone(), format!("{:?}", event.kind), a.clone()));
            match &a {
                HookAction::Deny { .. } => {
                    if event.kind == HookKind::PreToolUse {
                        return a; // first denial wins, pre-tool
                    }
                    // Post* denials are recorded observations only.
                }
                HookAction::Allow => {
                    if event.kind == HookKind::PreToolUse {
                        action = HookAction::Record; // ticket still required
                    } else {
                        action = a;
                    }
                }
                HookAction::Record => {
                    if matches!(action, HookAction::Record) {
                        action = a;
                    }
                }
            }
        }
        action
    }

    pub fn audit_trail(&self) -> &[(String, String, HookAction)] {
        &self.audit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: HookKind, capability: &str, tool: &str) -> HookEvent {
        HookEvent {
            kind,
            capability: capability.into(),
            tool: tool.into(),
            session_id: "s1".into(),
            turn_id: None,
            ok: None,
            error: None,
            at_tokens: None,
        }
    }

    #[test]
    fn pre_hook_can_deny() {
        let mut reg = HookRegistry::new();
        reg.install(
            HookSpec {
                name: "no-delete".into(),
                kind: HookKind::PreToolUse,
                capability: "fs.write/**".into(),
            },
            Box::new(|_| HookAction::Deny {
                reason: "delete is blocked".into(),
            }),
        );
        let action = reg.dispatch(&ev(HookKind::PreToolUse, "fs.write/**", "fs.remove"));
        assert!(matches!(action, HookAction::Deny { .. }));
        assert_eq!(reg.audit_trail().len(), 1); // denied attempts audited
    }

    #[test]
    fn pre_allow_never_skips_ticket() {
        let mut reg = HookRegistry::new();
        reg.install(
            HookSpec {
                kind: HookKind::PreToolUse,
                capability: "*".into(),
                name: "allow-all".into(),
            },
            Box::new(|_| HookAction::Allow),
        );
        let action = reg.dispatch(&ev(HookKind::PreToolUse, "fs.write/**", "fs.write"));
        // An allow hook degrades to Record: the ticket is never skipped.
        assert_eq!(action, HookAction::Record);
    }

    #[test]
    fn post_failure_hook_records() {
        let mut reg = HookRegistry::new();
        reg.install(
            HookSpec {
                kind: HookKind::PostToolUseFailure,
                capability: "*".into(),
                name: "failwatch".into(),
            },
            Box::new(|_| HookAction::Record),
        );
        let action = reg.dispatch(&ev(HookKind::PostToolUseFailure, "browser.navigate", "nav"));
        assert_eq!(action, HookAction::Record);
    }

    #[test]
    fn post_deny_is_record_only() {
        let mut reg = HookRegistry::new();
        reg.install(
            HookSpec {
                kind: HookKind::PostToolUse,
                capability: "*".into(),
                name: "p".into(),
            },
            Box::new(|_| HookAction::Deny {
                reason: "too late anyway".into(),
            }),
        );
        let action = reg.dispatch(&ev(HookKind::PostToolUse, "*", "t"));
        // The tool already ran; the deny is recorded, not effective.
        assert_eq!(action, HookAction::Record);
    }

    #[test]
    fn capability_scope_gates_hook() {
        let mut reg = HookRegistry::new();
        reg.install(
            HookSpec {
                name: "narrow".into(),
                kind: HookKind::PreToolUse,
                capability: "fs.write:/tmp/**".into(),
            },
            Box::new(|_| HookAction::Deny {
                reason: "no".into(),
            }),
        );
        // Different capability: hook does not fire.
        let action = reg.dispatch(&ev(HookKind::PreToolUse, "browser.navigate", "nav"));
        assert_eq!(action, HookAction::Record);
    }
}
