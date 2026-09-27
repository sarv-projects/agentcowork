//! FIX-03 / `TASK-CHAN-001` — the ACP **permission bridge**: the Guard-2 seam
//! for `session/request_permission`.
//!
//! The bridge is a **projection, never a decider**. Trust answers one approval
//! primitive (`DEC-021`, `CTR-012`) and Guard mints the bound ticket (`CTR-011`,
//! `DM-009`); this module only maps that answer onto an option the agent
//! actually offered, and refuses when it cannot (`ARCH/12-TRUST.md` §11
//! "fail closed", `REQ-TRUST-009`).
//!
//! Three rules make the mapping safe (`ARCH/42-EVIDENCE-MAP.md` §4 FIX-03):
//!
//! 1. **once / always / reject — never a client-side default.** `once` is a
//!    single-use ticket bound to this request; `always` *additionally* requires
//!    a durable declarative rule the user made explicitly, and is otherwise
//!    narrowed back to `once` (never granted as a client-side default); `reject`
//!    is a first-class answer (`ARCH/12-TRUST.md` §5 vocabulary mapping).
//! 2. **Never synthesize an option id.** An id the agent never offered is a
//!    typed error, not an invented string.
//! 3. **Show what is being approved.** [`PermissionPreview`] builds a bounded,
//!    secret-redacted [`DecisionPackage`] so the approval card carries the diff
//!    rather than a title alone.
//!
//! Redaction note: [`redact_secret_shapes`] mirrors the corpus and algorithm of
//! `agentcowork-core::spool::redact_secrets` (that crate cannot depend on this
//! one). It is a bounded, shape-based defence for surfaces that *quote* a tool's
//! arguments — the vault (INV-02) is the control that keeps a key from ever
//! reaching a tool argument. The intended end state is one implementation;
//! re-homing the corpus is tracked with the owner of `agentcowork-core`.

use crate::messages::{
    PermissionDecision, PermissionOptionKind, PermissionRequestParams, ToolKind,
};
use agentcowork_guard::RiskLevel;
use agentcowork_guard::ticket::AuthorizationTicket;
use serde::{Deserialize, Serialize};

/// The answer a human (or policy) reached on the owning channel, in the three
/// shapes an ACP permission surface can express (`ARCH/12-TRUST.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpApprovalChoice {
    /// Approve this one call. Realized by a single-use, bound ticket.
    Once,
    /// Approve and remember. Requires a persisted policy change by the user.
    Always,
    /// Do not run it. The only answer that needs no ticket.
    Reject,
}

impl AcpApprovalChoice {
    /// The ACP option kinds that can realize this choice, most specific first.
    fn accepted_kinds(self) -> &'static [PermissionOptionKind] {
        match self {
            // `always` is never reachable by accident: the bridge narrows it to
            // `Once` unless Trust recorded a policy change (see
            // `PermissionBridge::answer`).
            AcpApprovalChoice::Once => &[PermissionOptionKind::AllowOnce],
            AcpApprovalChoice::Always => &[
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::AllowOnce,
            ],
            AcpApprovalChoice::Reject => &[
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ],
        }
    }

    /// The vocabulary the approval card uses (`ARCH/12-TRUST.md` §5).
    pub fn card_vocabulary(self) -> &'static str {
        match self {
            AcpApprovalChoice::Once => "once",
            AcpApprovalChoice::Always => "always",
            AcpApprovalChoice::Reject => "deny",
        }
    }
}

/// The identity a ticket must be bound to for it to back an ACP allow
/// (`DM-009`: `capability_id`, `provider_id`, `environment_id`, args, uses).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketBinding {
    pub agent_id: String,
    pub session_id: String,
    /// The args fingerprint the ticket was minted for; the executor must
    /// present the identical hash.
    pub args_hash: String,
}

/// The ticket facts the bridge validates before it will express an allow.
///
/// A host that holds the real artifact converts it with
/// [`TicketFacts::from_ticket`]; a host that only has Guard's verdict reports
/// the same facts after `use_ticket` accepted the ticket. The bridge never
/// mints and never spends — Trust does both.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketFacts {
    pub ticket_id: String,
    pub agent_id: String,
    pub session_id: String,
    /// The args fingerprint the ticket was minted for.
    pub args_hash: String,
    /// Declared at issue (`DM-009`): a `once` answer needs a single-use ticket.
    pub single_use: bool,
    /// Guard accepted this ticket for this request: approved, unexpired, and
    /// (for an executing effect) spent. `false` means no artifact authorizes
    /// this allow.
    pub validated_by_guard: bool,
}

impl TicketFacts {
    /// Read the facts off a real ticket. `validated_by_guard` reflects the
    /// ticket's own state (`Approved` and unexpired right now); the argument
    /// binding is checked by [`require_once_ticket`].
    pub fn from_ticket(ticket: &AuthorizationTicket) -> Self {
        Self {
            ticket_id: ticket.ticket_id.clone(),
            agent_id: ticket.agent_id.clone(),
            session_id: ticket.session_id.clone(),
            args_hash: ticket.args_hash.clone(),
            single_use: ticket.single_use,
            validated_by_guard: ticket.is_valid(),
        }
    }
}

/// What the one Trust path decided for one ACP permission request.
///
/// The ACP layer never constructs a verdict of its own: the host fills this
/// from Guard's `ALLOW | ASK | DENY` plus the human's answer on the owning
/// channel (`ARCH/32-CHANNELS.md` §7).
#[derive(Debug, Clone)]
pub struct TrustOutcome {
    /// The choice the human made (`Reject` for a Guard `DENY`).
    pub choice: AcpApprovalChoice,
    /// The ticket Guard minted for this request, if one exists.
    pub ticket: Option<TicketFacts>,
    /// Trust wrote a durable declarative rule for this operation — the `always`
    /// half of the approval primitive. False for a one-off approval, and false
    /// whenever the user did not ask for persistence.
    pub policy_recorded: bool,
    /// Audit note (the decision, not a secret).
    pub reason: String,
}

impl TrustOutcome {
    /// A `once` answer backed by `ticket`.
    pub fn once(ticket: TicketFacts, reason: impl Into<String>) -> Self {
        Self {
            choice: AcpApprovalChoice::Once,
            ticket: Some(ticket),
            policy_recorded: false,
            reason: reason.into(),
        }
    }

    /// An `always` answer. `policy_recorded` is the host's assertion that the
    /// user's policy change was actually persisted by Trust.
    pub fn always(ticket: TicketFacts, policy_recorded: bool, reason: impl Into<String>) -> Self {
        Self {
            choice: AcpApprovalChoice::Always,
            ticket: Some(ticket),
            policy_recorded,
            reason: reason.into(),
        }
    }

    /// A `reject` answer (human rejection or Guard `DENY`). No ticket: a
    /// refusal is not an authorization artifact.
    pub fn reject(reason: impl Into<String>) -> Self {
        Self {
            choice: AcpApprovalChoice::Reject,
            ticket: None,
            policy_recorded: false,
            reason: reason.into(),
        }
    }
}

/// Why the bridge refused to answer. Every variant is fail-closed: the caller
/// must deny, never allow.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BridgeError {
    /// An allow carried no ticket — nothing backs it.
    #[error("no authorization ticket backs the ACP allow for {0}")]
    MissingTicket(String),
    /// Expired, revoked, already used, or never approved.
    #[error("the ticket is not live (expired, revoked, spent or unapproved)")]
    TicketNotLive,
    /// `once` must be spendable exactly once (`DM-009`, `ARCH/12-TRUST.md` §4).
    #[error("the ACP allow requires a single-use ticket")]
    TicketNotSingleUse,
    /// The ticket belongs to another agent, session, or argument set.
    #[error("ticket binding mismatch: {0}")]
    BindingMismatch(String),
    /// The agent offered no option that can express the decision.
    #[error("the agent offered no `{0}` option — refusing rather than inventing one")]
    NoOfferedOption(&'static str),
    /// An explicit option id was not among the offered options.
    #[error("offered option id {0} is not in the permission request")]
    UnknownOptionId(String),
}

/// The wire answer plus the evidence for the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeAnswer {
    /// Ready for the `session/request_permission` reply.
    pub decision: PermissionDecision,
    /// The choice actually expressed on the wire.
    pub choice: AcpApprovalChoice,
    /// The offered option id that was selected.
    pub option_id: String,
    /// The ticket backing the allow (`None` for a rejection).
    pub ticket_id: Option<String>,
    /// `true` when an `always` request was answered as `once` because no durable
    /// policy change was recorded — the effect is allowed, the persistent grant
    /// is not.
    pub narrowed: bool,
    /// Audit note.
    pub reason: String,
}

/// The ACP permission bridge. Stateless by design: it holds no authority, so
/// there is nothing to configure and nothing to keep in sync.
#[derive(Debug, Clone, Copy, Default)]
pub struct PermissionBridge;

impl PermissionBridge {
    /// Create the bridge.
    pub const fn new() -> Self {
        Self
    }

    /// Map one Trust outcome onto one offered ACP option.
    ///
    /// Order is deliberate: the wire option is resolved **first** (an
    /// unexpressible decision is refused before any authority is considered),
    /// then the ticket is validated, and only then is the answer returned.
    pub fn answer(
        &self,
        request: &PermissionRequestParams,
        binding: &TicketBinding,
        outcome: &TrustOutcome,
    ) -> Result<BridgeAnswer, BridgeError> {
        // `always` is an explicit user policy change, never a client default.
        // Without one, the effect may still run — but only once, and the
        // narrowing is reported so the host can surface it.
        let (choice, narrowed) = match outcome.choice {
            AcpApprovalChoice::Always if !outcome.policy_recorded => {
                (AcpApprovalChoice::Once, true)
            }
            other => (other, false),
        };

        if choice == AcpApprovalChoice::Reject {
            let option_id = select_option(request, AcpApprovalChoice::Reject)?.ok_or({
                // No reject option at all: we cannot express a refusal, so the
                // caller fails the request closed rather than guessing.
                BridgeError::NoOfferedOption("reject")
            })?;
            return Ok(BridgeAnswer {
                decision: PermissionDecision::Deny {
                    option_id: Some(option_id.clone()),
                },
                choice,
                option_id,
                ticket_id: None,
                narrowed,
                reason: outcome.reason.clone(),
            });
        }

        let option_id = select_option(request, choice)?
            .ok_or(BridgeError::NoOfferedOption(choice.card_vocabulary()))?;
        let ticket = outcome
            .ticket
            .as_ref()
            .ok_or_else(|| BridgeError::MissingTicket(request.tool_call.tool_call_id.clone()))?;
        require_once_ticket(ticket, binding)?;

        Ok(BridgeAnswer {
            decision: PermissionDecision::Allow {
                option_id: Some(option_id.clone()),
            },
            choice,
            option_id,
            ticket_id: Some(ticket.ticket_id.clone()),
            narrowed,
            reason: outcome.reason.clone(),
        })
    }

    /// Resolve a host decision into the option id the wire reply carries, or
    /// fail closed.
    ///
    /// This is the single strict resolver for `session/request_permission`
    /// replies (`AcpSession::prompt` delegates to it):
    ///
    /// - an **unpinned allow is a `once`** — the resolver never picks an
    ///   `allow_always` the user never chose;
    /// - an **explicit id must be one the agent offered** — a value the agent
    ///   never minted is refused, not echoed;
    /// - an **unanswerable decision is an error**, never a synthesized id.
    pub fn resolve(
        &self,
        request: &PermissionRequestParams,
        decision: &PermissionDecision,
    ) -> Result<String, BridgeError> {
        match decision {
            PermissionDecision::Allow {
                option_id: Some(id),
            } => offered_id(request, id),
            PermissionDecision::Deny {
                option_id: Some(id),
            } => offered_id(request, id),
            PermissionDecision::Allow { option_id: None } => {
                select_option(request, AcpApprovalChoice::Once)?
                    .ok_or(BridgeError::NoOfferedOption("allow_once"))
            }
            PermissionDecision::Deny { option_id: None } => {
                select_option(request, AcpApprovalChoice::Reject)?
                    .ok_or(BridgeError::NoOfferedOption("reject"))
            }
        }
    }
}

/// The offered option with this id, or a typed refusal.
fn offered_id(request: &PermissionRequestParams, id: &str) -> Result<String, BridgeError> {
    request
        .options
        .iter()
        .find(|option| option.option_id == id)
        .map(|option| option.option_id.clone())
        .ok_or_else(|| BridgeError::UnknownOptionId(id.to_string()))
}

/// The offered option that realizes `choice`, if the agent offered one.
///
/// A choice the agent never offered has no answer: the caller fails closed
/// rather than sending an option id the agent never minted.
fn select_option(
    request: &PermissionRequestParams,
    choice: AcpApprovalChoice,
) -> Result<Option<String>, BridgeError> {
    // Priority is by *kind*, not by the order the agent listed them in: an
    // `always` answer prefers `allow_always` over `allow_once` and vice versa,
    // so list order can never change what the wire expresses.
    for kind in choice.accepted_kinds() {
        if let Some(option) = request.options.iter().find(|o| o.kind == *kind) {
            return Ok(Some(option.option_id.clone()));
        }
    }
    Ok(None)
}

/// Refuse an allow that is not backed by a live, single-use, correctly bound
/// ticket (`DM-009`, `ARCH/12-TRUST.md` §4).
///
/// This is a *precondition check* on the artifact Guard already issued, not a
/// second decider: approval, validity, argument match and the single-use spend
/// are enforced by Trust (`GuardService::use_ticket`). The bridge only refuses
/// to answer `allow` for facts that could not authorize an effect.
pub fn require_once_ticket(
    ticket: &TicketFacts,
    binding: &TicketBinding,
) -> Result<(), BridgeError> {
    if ticket.ticket_id.trim().is_empty() {
        return Err(BridgeError::MissingTicket("<empty>".to_string()));
    }
    if !ticket.validated_by_guard {
        return Err(BridgeError::TicketNotLive);
    }
    if !ticket.single_use {
        return Err(BridgeError::TicketNotSingleUse);
    }
    if ticket.agent_id != binding.agent_id {
        return Err(BridgeError::BindingMismatch(format!(
            "ticket agent {:?} is not the requesting agent {:?}",
            ticket.agent_id, binding.agent_id
        )));
    }
    if ticket.session_id != binding.session_id {
        return Err(BridgeError::BindingMismatch(format!(
            "ticket session {:?} is not the requesting session {:?}",
            ticket.session_id, binding.session_id
        )));
    }
    if ticket.args_hash != binding.args_hash {
        return Err(BridgeError::BindingMismatch(
            "ticket arguments do not match this request".to_string(),
        ));
    }
    Ok(())
}

// ===========================================================================
// The approval preview — bounded, redacted, "show exactly what"
// ===========================================================================

/// Preview bounds. An agent's payload sizes the card, never the reverse.
pub const PREVIEW_MAX_DIFF_CHARS: usize = 2_000;
pub const PREVIEW_MAX_TITLE_CHARS: usize = 200;
pub const PREVIEW_MAX_PATHS: usize = 32;
pub const PREVIEW_MAX_PATH_CHARS: usize = 512;
pub const PREVIEW_MAX_SCRIPT_LINES: usize = 20;
pub const PREVIEW_MAX_LINE_CHARS: usize = 400;
pub const PREVIEW_MAX_DESTINATIONS: usize = 16;

/// What the approver is being asked to allow, in bounded and redacted form.
///
/// This is the ACP-side projection of `agentcowork_guard::decision::DecisionPackage`
/// (the Guard-2 card payload). It is built from the agent's own request, so it
/// can be rendered **before** the decision is made — an approval the human
/// cannot inspect is not informed consent (`ARCH/12-TRUST.md` §2, `DEC-028`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPreview {
    pub tool_call_id: String,
    /// The agent's tool-call title (what it says it is doing).
    pub title: String,
    /// The ACP tool kind (`edit` / `execute` / `delete` / …).
    pub kind: String,
    /// The canonical Guard operation name this request maps to.
    pub operation: String,
    pub risk: RiskLevel,
    /// Files the call touches, from the ACP `locations`.
    pub paths: Vec<String>,
    /// The proposed change, rendered from the tool's own input.
    pub diff: String,
    /// The exact command lines, for shell-class calls.
    pub script_lines: Vec<String>,
    /// What would run (interpreter / program).
    pub execution_target: String,
    /// Hosts the call would contact.
    pub network_destinations: Vec<String>,
    /// `true` when a bound elided content.
    pub truncated: bool,
    /// `true` when credential-shaped spans were masked.
    pub redacted: bool,
}

impl PermissionPreview {
    /// Build the preview for one permission request.
    ///
    /// `operation` and `risk` come from the host's ACP→Guard mapping (one
    /// mapping, `acp_cmds::map_tool_call`); everything else is derived from the
    /// request itself.
    pub fn build(request: &PermissionRequestParams, operation: &str, risk: RiskLevel) -> Self {
        let tool_call = &request.tool_call;
        let kind = tool_call
            .kind
            .map(|kind| kind.as_str())
            .unwrap_or_else(|| ToolKind::Other.as_str());

        let mut truncated = false;
        let mut redacted = false;

        let (title, t_redacted) =
            cap_text(&tool_call.title, PREVIEW_MAX_TITLE_CHARS, &mut truncated);
        redacted |= t_redacted;

        let mut paths = Vec::new();
        for location in &tool_call.locations {
            if paths.len() >= PREVIEW_MAX_PATHS {
                truncated = true;
                break;
            }
            let (uri, uri_redacted) =
                cap_text(&location.uri, PREVIEW_MAX_PATH_CHARS, &mut truncated);
            redacted |= uri_redacted;
            paths.push(uri);
        }

        let (diff, script_lines, execution_target, d_redacted, s_redacted) = render_arguments(
            &tool_call.raw_input,
            &tool_call.content,
            kind,
            &mut truncated,
        );
        redacted |= d_redacted | s_redacted;

        let (network_destinations, n_redacted) = collect_destinations(&diff, &script_lines);
        redacted |= n_redacted;

        Self {
            tool_call_id: tool_call.tool_call_id.clone(),
            title,
            kind: kind.to_string(),
            operation: operation.to_string(),
            risk,
            paths,
            diff,
            script_lines,
            execution_target,
            network_destinations,
            truncated,
            redacted,
        }
    }
}

/// Keys whose values are an edit's "before" and "after" text.
const OLD_TEXT_KEYS: &[&str] = &[
    "oldText",
    "old_text",
    "old_string",
    "oldString",
    "before",
    "original",
    "old_content",
];
const NEW_TEXT_KEYS: &[&str] = &[
    "newText",
    "new_text",
    "new_string",
    "newString",
    "after",
    "replacement",
    "new_content",
    "content",
];
const PATCH_KEYS: &[&str] = &["patch", "diff", "unified_diff", "unifiedDiff"];
const COMMAND_KEYS: &[&str] = &["command", "cmd", "script", "shell_command", "commandLine"];

/// Render the tool's own input into a diff the approver can read.
///
/// Deliberately structural rather than clever: an edit-shaped call renders
/// `-`/`+` lines from the before/after values, a patch-shaped call renders the
/// patch, and anything else renders its arguments as `key = value`. Sorted keys
/// keep the rendering deterministic.
#[allow(clippy::type_complexity)]
fn render_arguments(
    raw_input: &Option<serde_json::Value>,
    content: &[crate::messages::ContentBlock],
    kind: &str,
    truncated: &mut bool,
) -> (String, Vec<String>, String, bool, bool) {
    let mut redacted = false;
    let mut lines: Vec<String> = Vec::new();
    let mut script_lines: Vec<String> = Vec::new();
    let mut execution_target = String::new();

    if let Some(serde_json::Value::Object(map)) = raw_input {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for key in keys {
            let value = &map[key];
            if PATCH_KEYS.contains(&key.as_str()) {
                push_text_lines(&mut lines, as_text(value), "-", truncated);
                continue;
            }
            if OLD_TEXT_KEYS.contains(&key.as_str()) {
                push_text_lines(&mut lines, as_text(value), "-", truncated);
                continue;
            }
            if NEW_TEXT_KEYS.contains(&key.as_str()) {
                push_text_lines(&mut lines, as_text(value), "+", truncated);
                continue;
            }
            if COMMAND_KEYS.contains(&key.as_str()) {
                let command = as_text(value);
                for line in command.lines().take(PREVIEW_MAX_SCRIPT_LINES) {
                    script_lines.push(cap_line_tracked(line, truncated));
                }
                if command.lines().count() > PREVIEW_MAX_SCRIPT_LINES {
                    *truncated = true;
                }
                if execution_target.is_empty() {
                    execution_target = cap_line_tracked(
                        command.split_whitespace().next().unwrap_or_default(),
                        truncated,
                    );
                }
                continue;
            }
            // Any other argument: show it, bounded. A `url` argument is a
            // destination and is rendered verbatim (the destination collector
            // reads it back out).
            match value {
                serde_json::Value::String(text) => {
                    if text.contains('\n') {
                        push_text_lines(&mut lines, text.clone(), "+", truncated);
                    } else {
                        lines.push(format!("{key} = {}", cap_line_tracked(text, truncated)));
                    }
                }
                other => {
                    let rendered = serde_json::to_string(other).unwrap_or_default();
                    lines.push(format!(
                        "{key} = {}",
                        cap_line_tracked(&rendered, truncated)
                    ));
                }
            }
        }
    }

    // The ACP `content` blocks carry the call's own payload for edit-shaped
    // calls; use them when the arguments carried no renderable text.
    if lines.is_empty() {
        for block in content {
            if block.text.trim().is_empty() {
                continue;
            }
            push_text_lines(&mut lines, block.text.clone(), "+", truncated);
        }
    }

    // A shell-class call with no explicit command still deserves its script.
    if script_lines.is_empty() && kind == ToolKind::Execute.as_str() {
        for line in lines.iter().take(PREVIEW_MAX_SCRIPT_LINES) {
            script_lines.push(cap_line(line.trim_start_matches(['-', '+', ' '])));
        }
        if execution_target.is_empty() {
            execution_target = cap_line(
                script_lines
                    .first()
                    .and_then(|l| l.split_whitespace().next())
                    .unwrap_or_default(),
            );
        }
    }

    let (diff, diff_redacted) = join_bounded(lines, PREVIEW_MAX_DIFF_CHARS, truncated);
    redacted |= diff_redacted;
    let mut scripts_redacted = false;
    for line in script_lines.iter_mut() {
        let (value, was_redacted) = redact_secret_shapes(line);
        scripts_redacted |= was_redacted;
        *line = value;
    }
    (
        diff,
        script_lines,
        execution_target,
        redacted,
        scripts_redacted,
    )
}

/// A JSON value as text (scalars verbatim, structures compactly serialized).
fn as_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn push_text_lines(lines: &mut Vec<String>, text: String, marker: &str, truncated: &mut bool) {
    let count = text.lines().count();
    for line in text.lines().take(PREVIEW_MAX_SCRIPT_LINES * 5) {
        lines.push(format!("{marker} {}", cap_line_tracked(line, truncated)));
    }
    if count > PREVIEW_MAX_SCRIPT_LINES * 5 {
        *truncated = true;
    }
}

fn join_bounded(lines: Vec<String>, max_chars: usize, truncated: &mut bool) -> (String, bool) {
    let mut out = String::new();
    let mut redacted = false;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if out.len() + line.len() > max_chars {
            *truncated = true;
            break;
        }
        let (value, was_redacted) = redact_secret_shapes(line);
        redacted |= was_redacted;
        out.push_str(&value);
    }
    (out, redacted)
}

fn cap_line(line: &str) -> String {
    if line.chars().count() <= PREVIEW_MAX_LINE_CHARS {
        return line.to_string();
    }
    line.chars()
        .take(PREVIEW_MAX_LINE_CHARS)
        .chain(std::iter::once('…'))
        .collect()
}

/// [`cap_line`] that records that it elided something, so the preview can say
/// "this was truncated" instead of quietly showing a partial line.
fn cap_line_tracked(line: &str, truncated: &mut bool) -> String {
    if line.chars().count() > PREVIEW_MAX_LINE_CHARS {
        *truncated = true;
    }
    cap_line(line)
}

fn cap_text(text: &str, max_chars: usize, truncated: &mut bool) -> (String, bool) {
    let mut value = String::new();
    let mut redacted = false;
    for ch in text.chars() {
        if value.len() >= max_chars {
            *truncated = true;
            break;
        }
        let mut buf = [0u8; 4];
        value.push_str(ch.encode_utf8(&mut buf));
    }
    if value.len() < text.len() {
        *truncated = true;
    }
    let (value, was_redacted) = redact_secret_shapes(&value);
    redacted |= was_redacted;
    (value, redacted)
}

/// Collect the hosts a call would contact, from the rendered text.
fn collect_destinations(diff: &str, script_lines: &[String]) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut redacted = false;
    let haystack = format!("{diff}\n{}", script_lines.join("\n"));
    let (masked, was_redacted) = redact_secret_shapes(&haystack);
    redacted |= was_redacted;
    for token in masked.split(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        let host = url_host(token);
        let Some(host) = host else { continue };
        if !out.contains(&host) {
            out.push(host);
        }
        if out.len() >= PREVIEW_MAX_DESTINATIONS {
            break;
        }
    }
    (out, redacted)
}

/// The host of an `http(s)/ws(s)/file` token, or `None` when it is not a URL.
fn url_host(token: &str) -> Option<String> {
    let token = token.trim_matches(|c: char| c == '(' || c == ')' || c == ',');
    for scheme in ["https://", "http://", "wss://", "ws://", "file://"] {
        if let Some(rest) = token.strip_prefix(scheme) {
            let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
            if host.is_empty() {
                return None;
            }
            return Some(cap_line(&format!("{scheme}{host}")));
        }
    }
    None
}

// ===========================================================================
// Secret redaction (shape-based, bounded corpus)
// ===========================================================================

/// What a redacted span is replaced with.
pub const REDACTION: &str = "[redacted]";

/// Credential-shaped token prefixes. A fixed, bounded corpus of *token shapes*:
/// this is a defence-in-depth control for surfaces that quote a tool's
/// arguments (a card, a copied log), not a DLP product.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "r8_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xapp-",
    "npm_",
    "hf_",
    "dop_v1_",
    "AKIA",
    "ASIA",
    "AIza",
    "ya29.",
];

/// Assignment keys whose *value* is a secret (matched case-insensitively).
const SECRET_ASSIGNMENT_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "x-api-key",
    "access_token",
    "auth_token",
    "token",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "secret_key",
    "private_key",
    "authorization",
];

/// Replace credential-shaped spans with [`REDACTION`], returning the masked
/// text and whether anything was masked.
///
/// The key name of an assignment stays legible — the point of a preview is
/// "this run set `ANTHROPIC_API_KEY`" without carrying the key.
pub fn redact_secret_shapes(text: &str) -> (String, bool) {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    let mut redacted = false;
    while index < bytes.len() {
        if let Some(end) = pem_block(bytes, index) {
            out.push_str(REDACTION);
            index = end;
            redacted = true;
            continue;
        }
        if let Some(end) = prefixed_token(bytes, index) {
            out.push_str(REDACTION);
            index = end;
            redacted = true;
            continue;
        }
        if let Some(end) = jwt(bytes, index) {
            out.push_str(REDACTION);
            index = end;
            redacted = true;
            continue;
        }
        if let Some((value_start, value_end)) = assigned_secret(bytes, index) {
            out.push_str(&text[index..value_start]);
            out.push_str(REDACTION);
            index = value_end;
            redacted = true;
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    if redacted {
        (out, true)
    } else {
        (text.to_string(), false)
    }
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b'+' | b'=')
}

fn pem_block(bytes: &[u8], index: usize) -> Option<usize> {
    if !bytes[index..].starts_with(b"-----BEGIN") {
        return None;
    }
    let end = bytes[index..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| index + p)
        .unwrap_or(bytes.len());
    let line = String::from_utf8_lossy(&bytes[index..end]);
    line.contains("PRIVATE KEY").then_some(end)
}

fn prefixed_token(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    for prefix in SECRET_PREFIXES {
        let head = prefix.as_bytes();
        if !bytes[index..].starts_with(head) {
            continue;
        }
        let mut end = index + head.len();
        while end < bytes.len() && is_token_byte(bytes[end]) {
            end += 1;
        }
        // A prefix with almost nothing after it is a word, not a key.
        if end - (index + head.len()) >= 8 {
            return Some(end);
        }
    }
    None
}

fn jwt(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    if !bytes[index..].starts_with(b"eyJ") {
        return None;
    }
    let mut cursor = index;
    for segment in 0..3 {
        let start = cursor;
        while cursor < bytes.len() && is_base64url(bytes[cursor]) {
            cursor += 1;
        }
        if cursor - start < 8 {
            return None;
        }
        if segment < 2 {
            if cursor >= bytes.len() || bytes[cursor] != b'.' {
                return None;
            }
            cursor += 1;
        }
    }
    Some(cursor)
}

fn is_base64url(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

/// Detect `key = value` / `key: value` / `-H "authorization: value"` shapes and
/// return the value span, so only the secret is masked.
fn assigned_secret(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let key_start = line_start(bytes, index);
    // Walk the candidate key (word characters plus `-`).
    let mut cursor = index;
    while cursor < bytes.len() && (is_word_byte(bytes[cursor]) || bytes[cursor] == b'-') {
        cursor += 1;
    }
    if cursor == index || cursor - index > 48 {
        return None;
    }
    let key = String::from_utf8_lossy(&bytes[index..cursor]).to_ascii_lowercase();
    if !SECRET_ASSIGNMENT_KEYS.contains(&key.trim_start_matches('-').trim_end_matches('-')) {
        return None;
    }
    let mut scan = cursor;
    while scan < bytes.len() && (bytes[scan] == b' ' || bytes[scan] == b'\t') {
        scan += 1;
    }
    if scan >= bytes.len() || (bytes[scan] != b'=' && bytes[scan] != b':') {
        return None;
    }
    scan += 1;
    while scan < bytes.len() && (bytes[scan] == b' ' || bytes[scan] == b'\t') {
        scan += 1;
    }
    if scan >= bytes.len() {
        return None;
    }
    let quote = matches!(bytes[scan], b'"' | b'\'');
    if quote {
        scan += 1;
    }
    let value_start = scan;
    let mut value_end = scan;
    while value_end < bytes.len() {
        let byte = bytes[value_end];
        if quote {
            if byte == bytes[scan - 1] {
                break;
            }
        } else if byte.is_ascii_whitespace() || matches!(byte, b',' | b';' | b'}' | b']') {
            break;
        }
        value_end += 1;
    }
    if value_end == value_start {
        return None;
    }
    let _ = key_start;
    Some((value_start, value_end))
}

fn line_start(bytes: &[u8], index: usize) -> usize {
    let mut start = index;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{ContentBlock, Location, PermissionOption, ToolCall};
    use agentcowork_guard::ticket::{ApprovalSource, TicketState};

    fn option(id: &str, kind: PermissionOptionKind) -> PermissionOption {
        PermissionOption {
            option_id: id.to_string(),
            kind,
            label: id.to_string(),
        }
    }

    fn request(
        options: Vec<PermissionOption>,
        raw_input: Option<serde_json::Value>,
    ) -> PermissionRequestParams {
        PermissionRequestParams {
            session_id: "s1".into(),
            tool_call: ToolCall {
                tool_call_id: "tc1".into(),
                title: "Edit a.rs".into(),
                kind: Some(ToolKind::Edit),
                content: Vec::new(),
                locations: vec![Location {
                    r#type: "file".into(),
                    uri: "file:///w/a.rs".into(),
                    range: None,
                }],
                raw_input,
            },
            options,
        }
    }

    fn ticket() -> AuthorizationTicket {
        AuthorizationTicket {
            ticket_id: "tkt:1".into(),
            agent_id: "claude-code".into(),
            session_id: "s1".into(),
            tool_id: "acp.tc1".into(),
            operation: "write".into(),
            args_hash: "args-1".into(),
            paths: vec!["/w/a.rs".into()],
            expires_at_ms: 0,
            single_use: true,
            approval_source: ApprovalSource::Human,
            approval_nonce: "nonce".into(),
            risk: RiskLevel::Medium,
            audit_seq: 0,
            state: TicketState::Approved,
            bindings: Vec::new(),
            execution_id: String::new(),
            action_id: String::new(),
            idempotency_key: String::new(),
        }
    }

    /// The facts of [`ticket`] as a host that holds the real artifact reports
    /// them.
    fn facts() -> TicketFacts {
        TicketFacts::from_ticket(&ticket())
    }

    fn binding() -> TicketBinding {
        TicketBinding {
            agent_id: "claude-code".into(),
            session_id: "s1".into(),
            args_hash: "args-1".into(),
        }
    }

    // ---- once / always / reject ------------------------------------------

    #[test]
    fn once_selects_the_offered_allow_once_option_and_returns_its_ticket() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![
                option("allow-once", PermissionOptionKind::AllowOnce),
                option("allow-always", PermissionOptionKind::AllowAlways),
            ],
            None,
        );
        let answer = bridge
            .answer(
                &req,
                &binding(),
                &TrustOutcome::once(facts(), "human approved"),
            )
            .expect("answered");
        assert_eq!(answer.option_id, "allow-once");
        assert_eq!(answer.choice, AcpApprovalChoice::Once);
        assert!(!answer.narrowed);
        assert_eq!(answer.ticket_id.as_deref(), Some("tkt:1"));
        assert_eq!(
            answer.decision,
            PermissionDecision::Allow {
                option_id: Some("allow-once".into())
            }
        );
    }

    #[test]
    fn always_is_refused_without_a_recorded_policy_change_and_narrows_to_once() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![
                option("allow-once", PermissionOptionKind::AllowOnce),
                option("allow-always", PermissionOptionKind::AllowAlways),
            ],
            None,
        );
        let answer = bridge
            .answer(
                &req,
                &binding(),
                &TrustOutcome::always(facts(), false, "user pressed always"),
            )
            .expect("answered");
        // The persistent grant is never granted client-side; the effect still
        // runs exactly once.
        assert_eq!(answer.option_id, "allow-once");
        assert_eq!(answer.choice, AcpApprovalChoice::Once);
        assert!(
            answer.narrowed,
            "the narrowing must be reported to the host"
        );
    }

    #[test]
    fn always_is_granted_only_when_trust_recorded_the_policy_change() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![
                option("allow-once", PermissionOptionKind::AllowOnce),
                option("allow-always", PermissionOptionKind::AllowAlways),
            ],
            None,
        );
        let answer = bridge
            .answer(
                &req,
                &binding(),
                &TrustOutcome::always(facts(), true, "rule persisted"),
            )
            .expect("answered");
        assert_eq!(answer.option_id, "allow-always");
        assert_eq!(answer.choice, AcpApprovalChoice::Always);
        assert!(!answer.narrowed);
    }

    #[test]
    fn reject_uses_the_offered_reject_option_and_needs_no_ticket() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![
                option("allow-once", PermissionOptionKind::AllowOnce),
                option("reject-once", PermissionOptionKind::RejectOnce),
            ],
            None,
        );
        let answer = bridge
            .answer(&req, &binding(), &TrustOutcome::reject("human rejected"))
            .expect("answered");
        assert_eq!(answer.option_id, "reject-once");
        assert_eq!(
            answer.decision,
            PermissionDecision::Deny {
                option_id: Some("reject-once".into())
            }
        );
        assert!(answer.ticket_id.is_none());
    }

    // ---- fail closed -------------------------------------------------------

    #[test]
    fn a_bridge_error_never_yields_an_allow() {
        let bridge = PermissionBridge::new();
        // No reject option offered: a refusal cannot be expressed, so the
        // bridge errors instead of inventing an id.
        let req = request(
            vec![option("allow-always", PermissionOptionKind::AllowAlways)],
            None,
        );
        let error = bridge
            .answer(&req, &binding(), &TrustOutcome::reject("denied"))
            .expect_err("must fail closed");
        assert_eq!(error, BridgeError::NoOfferedOption("reject"));
    }

    #[test]
    fn an_allow_without_a_ticket_is_refused() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        let mut outcome = TrustOutcome::reject("policy allow");
        outcome.choice = AcpApprovalChoice::Once;
        let error = bridge
            .answer(&req, &binding(), &outcome)
            .expect_err("must fail closed");
        assert_eq!(error, BridgeError::MissingTicket("tc1".into()));
    }

    #[test]
    fn a_multi_use_ticket_cannot_back_a_once_answer() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        let mut ticket = facts();
        ticket.single_use = false;
        let error = bridge
            .answer(
                &req,
                &binding(),
                &TrustOutcome::once(ticket, "policy auto-allow"),
            )
            .expect_err("must fail closed");
        assert_eq!(error, BridgeError::TicketNotSingleUse);
    }

    #[test]
    fn a_spent_or_unapproved_ticket_cannot_back_an_allow() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        for state in [
            TicketState::Used,
            TicketState::Pending,
            TicketState::Revoked,
            TicketState::Expired,
            TicketState::Rejected,
        ] {
            let mut spent = ticket();
            spent.state = state;
            let error = bridge
                .answer(
                    &req,
                    &binding(),
                    &TrustOutcome::once(TicketFacts::from_ticket(&spent), "stale"),
                )
                .expect_err("must fail closed");
            assert_eq!(error, BridgeError::TicketNotLive, "state {state:?}");
        }
        // A host that reports the facts without Guard's validation is refused
        // too: the bridge never trusts an unvalidated claim.
        let mut claimed = facts();
        claimed.validated_by_guard = false;
        assert_eq!(
            bridge
                .answer(
                    &req,
                    &binding(),
                    &TrustOutcome::once(claimed, "unvalidated")
                )
                .expect_err("must fail closed"),
            BridgeError::TicketNotLive
        );
    }

    #[test]
    fn a_ticket_bound_to_another_agent_session_or_args_is_refused() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        for wrong in [
            TicketBinding {
                agent_id: "other-agent".into(),
                session_id: "s1".into(),
                args_hash: "args-1".into(),
            },
            TicketBinding {
                agent_id: "claude-code".into(),
                session_id: "other-session".into(),
                args_hash: "args-1".into(),
            },
            TicketBinding {
                agent_id: "claude-code".into(),
                session_id: "s1".into(),
                args_hash: "args-2".into(),
            },
        ] {
            let error = bridge
                .answer(
                    &req,
                    &wrong,
                    &TrustOutcome::once(facts(), "bound elsewhere"),
                )
                .expect_err("must fail closed");
            assert!(matches!(error, BridgeError::BindingMismatch(_)), "{error}");
        }
    }

    #[test]
    fn a_ticket_from_another_call_cannot_be_replayed_for_this_one() {
        // The binding facts of a *different* request: same agent/session, but a
        // different args fingerprint. A replayed ticket is refused (and Trust's
        // `use_ticket` is what actually spends it).
        let bridge = PermissionBridge::new();
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        let mut other = facts();
        other.ticket_id = "tkt:other".into();
        other.args_hash = "args-2".into();
        let error = bridge
            .answer(
                &req,
                &binding(),
                &TrustOutcome::once(other, "replay attempt"),
            )
            .expect_err("must fail closed");
        assert!(matches!(error, BridgeError::BindingMismatch(_)));
    }

    // ---- the preview -------------------------------------------------------

    #[test]
    fn preview_renders_an_edit_diff_the_approver_can_read() {
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            Some(serde_json::json!({
                "path": "/w/a.rs",
                "old_string": "let x = 1;",
                "new_string": "let x = 2;"
            })),
        );
        let preview = PermissionPreview::build(&req, "write", RiskLevel::Medium);
        assert!(preview.diff.contains("- let x = 1;"));
        assert!(preview.diff.contains("+ let x = 2;"));
        assert!(preview.diff.contains("path = /w/a.rs"));
        assert_eq!(preview.paths, vec!["file:///w/a.rs".to_string()]);
        assert_eq!(preview.operation, "write");
        assert_eq!(preview.risk, RiskLevel::Medium);
        assert_eq!(preview.tool_call_id, "tc1");
        assert_eq!(preview.kind, "edit");
        assert!(!preview.redacted);
        assert!(!preview.truncated);
    }

    #[test]
    fn preview_extracts_the_command_and_its_destinations() {
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            Some(serde_json::json!({
                "command": "curl -sS https://api.example.com/v1/models",
                "cwd": "/w"
            })),
        );
        let preview = PermissionPreview::build(&req, "exec", RiskLevel::High);
        assert_eq!(
            preview.script_lines,
            vec!["curl -sS https://api.example.com/v1/models".to_string()]
        );
        assert_eq!(preview.execution_target, "curl");
        assert_eq!(
            preview.network_destinations,
            vec!["https://api.example.com".to_string()]
        );
    }

    #[test]
    fn preview_is_bounded_however_large_the_payload() {
        let huge = "x".repeat(PREVIEW_MAX_DIFF_CHARS * 3);
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            Some(serde_json::json!({ "content": huge })),
        );
        let preview = PermissionPreview::build(&req, "write", RiskLevel::Low);
        assert!(
            preview.diff.len() <= PREVIEW_MAX_DIFF_CHARS,
            "diff was {} chars",
            preview.diff.len()
        );
        assert!(preview.truncated);
    }

    #[test]
    fn preview_caps_the_path_list() {
        let mut req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        req.tool_call.locations = (0..(PREVIEW_MAX_PATHS + 10))
            .map(|i| Location {
                r#type: "file".into(),
                uri: format!("file:///w/f{i}"),
                range: None,
            })
            .collect();
        let preview = PermissionPreview::build(&req, "write", RiskLevel::Low);
        assert_eq!(preview.paths.len(), PREVIEW_MAX_PATHS);
        assert!(preview.truncated);
    }

    #[test]
    fn preview_redacts_credential_shaped_spans() {
        let req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            Some(serde_json::json!({
                "command": "curl -H 'authorization: Bearer sk-abcdef1234567890' https://api.example.com"
            })),
        );
        let preview = PermissionPreview::build(&req, "exec", RiskLevel::High);
        let rendered = format!("{}\n{}", preview.diff, preview.script_lines.join("\n"));
        assert!(
            !rendered.contains("sk-abcdef1234567890"),
            "the key leaked into the preview: {rendered}"
        );
        assert!(preview.redacted);
        // The key name stays legible — the human still sees what was set.
        assert!(
            rendered.to_lowercase().contains("authorization"),
            "{rendered}"
        );
        // The destination is still reported (egress is the point of the card).
        assert_eq!(
            preview.network_destinations,
            vec!["https://api.example.com".to_string()]
        );
    }

    #[test]
    fn redaction_covers_prefixes_jwts_pem_and_assignments() {
        let cases = [
            "export OPENAI_API_KEY=sk-proj-abcdefgh12345678",
            "token: ghp_abcdefgh1234567890abcdefgh",
            "password = 'hunter2hunter2'",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEow==",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        ];
        for case in cases {
            let (out, redacted) = redact_secret_shapes(case);
            assert!(redacted, "not redacted: {case}");
            assert!(out.contains(REDACTION), "no marker in {out}");
        }
        // Ordinary text is untouched.
        let ordinary = "cargo test -p agentcowork-acp --lib";
        let (out, redacted) = redact_secret_shapes(ordinary);
        assert!(!redacted);
        assert_eq!(out, ordinary);
    }

    #[test]
    fn content_blocks_are_used_when_the_arguments_carry_no_text() {
        let mut req = request(
            vec![option("allow-once", PermissionOptionKind::AllowOnce)],
            None,
        );
        req.tool_call.content = vec![ContentBlock {
            r#type: "content".into(),
            text: "+ replaced line".into(),
        }];
        let preview = PermissionPreview::build(&req, "write", RiskLevel::Low);
        assert!(preview.diff.contains("+ replaced line"), "{}", preview.diff);
    }

    // ---- wire resolution (the strict resolver) -----------------------------

    #[test]
    fn resolve_never_upgrades_an_unpinned_allow_to_allow_always() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![
                option("allow-once", PermissionOptionKind::AllowOnce),
                option("allow-always", PermissionOptionKind::AllowAlways),
            ],
            None,
        );
        assert_eq!(
            bridge
                .resolve(&req, &PermissionDecision::allow())
                .expect("resolved"),
            "allow-once"
        );
    }

    #[test]
    fn resolve_accepts_an_offered_id_and_refuses_a_foreign_one() {
        let bridge = PermissionBridge::new();
        let req = request(
            vec![
                option("allow-once", PermissionOptionKind::AllowOnce),
                option("reject-always", PermissionOptionKind::RejectAlways),
            ],
            None,
        );
        assert_eq!(
            bridge
                .resolve(
                    &req,
                    &PermissionDecision::Deny {
                        option_id: Some("reject-always".into())
                    }
                )
                .expect("resolved"),
            "reject-always"
        );
        let error = bridge
            .resolve(
                &req,
                &PermissionDecision::Allow {
                    option_id: Some("approve-everything".into()),
                },
            )
            .expect_err("must fail closed");
        assert_eq!(
            error,
            BridgeError::UnknownOptionId("approve-everything".into())
        );
    }

    #[test]
    fn resolve_errors_when_the_agent_offered_nothing_usable() {
        let bridge = PermissionBridge::new();
        let req = request(vec![], None);
        assert_eq!(
            bridge.resolve(&req, &PermissionDecision::allow()),
            Err(BridgeError::NoOfferedOption("allow_once"))
        );
        assert_eq!(
            bridge.resolve(&req, &PermissionDecision::deny()),
            Err(BridgeError::NoOfferedOption("reject"))
        );
    }
}
