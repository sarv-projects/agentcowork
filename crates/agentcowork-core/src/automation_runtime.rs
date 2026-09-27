//! The pure Work factory for automations (P71.3c — `ARCH/AUTOMATION.md` §5).
//!
//! An automation revision is immutable data.  This module validates that
//! data, derives its canonical identity, and returns a `WorkSpec`; it never
//! creates Work, executes an effect, retries an effect, or owns an event log.
//! The host owns the durable Work/Run admission boundary.

use agentcowork_blueprint::{Automation, AutomationStep};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Provenance stamped on every Work produced from an automation revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationProvenance {
    pub automation_id: String,
    /// Canonical `g<generation>:r<revision>:<sha256>` identity.
    pub revision_id: String,
    /// The durable automation generation which owned this revision.
    #[serde(default)]
    pub automation_generation: u64,
    /// The one trigger admission which produced this Work.
    pub trigger_occurrence_id: String,
}

/// One compiled step.  Classification is data, not an execution decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledStep {
    pub index: usize,
    pub kind: String,
    pub deterministic: bool,
    pub capability_request: Option<CompiledCapabilityRequest>,
}

/// A capability request carried by the Work factory.  The factory does not
/// resolve or grant it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledCapabilityRequest {
    pub capability_id: String,
    pub step_index: usize,
    pub reason: String,
}

/// The complete, pure artifact handed to the Work owner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSpec {
    pub provenance: AutomationProvenance,
    pub objective: String,
    pub steps: Vec<CompiledStep>,
    pub capability_requests: Vec<CompiledCapabilityRequest>,
    pub agent_required: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutomationError {
    #[error("automation has no steps")]
    Empty,
    #[error("run_code language `{0}` is not supported by the script engine")]
    UnsupportedLanguage(String),
    #[error("automation revision id is required and must be immutable")]
    MissingRevision,
    #[error("automation revision id is invalid: {0}")]
    InvalidRevision(String),
    #[error("automation revision does not match its immutable snapshot")]
    RevisionMismatch,
    #[error("automation names no trigger occurrence; compile requires one firing")]
    MissingOccurrence,
}

/// The parsed identity carried by a canonical revision id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionIdentity {
    pub generation: u64,
    pub revision: u64,
    pub digest: String,
}

/// Validate a step shape before a Work artifact is produced.
pub fn validate_step(step: &AutomationStep) -> Result<(), AutomationError> {
    match step {
        AutomationStep::RunCode { language, .. } => {
            let language = language.to_ascii_lowercase();
            if !matches!(language.as_str(), "js" | "javascript" | "ts" | "typescript") {
                return Err(AutomationError::UnsupportedLanguage(language));
            }
            Ok(())
        }
        AutomationStep::OnlineSearch { .. }
        | AutomationStep::Email { .. }
        | AutomationStep::Calendar { .. } => Ok(()),
    }
}

fn capability_of(index: usize, step: &AutomationStep) -> CompiledCapabilityRequest {
    let (capability_id, reason) = match step {
        AutomationStep::RunCode { .. } => (
            "script.eval",
            "sandboxed code execution for automation step",
        ),
        AutomationStep::OnlineSearch { .. } => {
            ("net.search", "web search cascade for automation step")
        }
        AutomationStep::Email { .. } => ("connector.email", "outbound email write"),
        AutomationStep::Calendar { .. } => ("connector.calendar", "calendar write"),
    };
    CompiledCapabilityRequest {
        capability_id: capability_id.into(),
        step_index: index,
        reason: reason.into(),
    }
}

/// Canonical JSON used for revision and delivery digests.  Object keys are
/// sorted recursively so equal values have one byte representation.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(values) => {
            let values: Vec<String> = values.iter().map(canonical_json).collect();
            format!("[{}]", values.join(","))
        }
        Value::Object(values) => {
            let mut keys: Vec<&String> = values.keys().collect();
            keys.sort();
            let values: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(&values[key])
                    )
                })
                .collect();
            format!("{{{}}}", values.join(","))
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const SNAPSHOT_MARKER: &str = "::snapshot:";

/// Strip the scheduler's immutable snapshot envelope before exposing the
/// automation identity in Work provenance.  The envelope is hashed as part of
/// the revision definition, so policy/session metadata cannot be edited without
/// changing the revision id.
pub fn canonical_automation_id(automation_id: &str) -> &str {
    automation_id
        .split_once(SNAPSHOT_MARKER)
        .map(|(base, _)| base)
        .unwrap_or(automation_id)
}

/// Return the content digest for the immutable automation definition and its
/// generation.  The generation is part of the digest, not merely a display
/// counter, so deleting and recreating an id cannot inherit an old revision.
///
/// NOTE (DEC-053 Step 4): `"everyaios.automation.revision.v1"` is a persisted
/// wire/schema token parsed by the sidecar — its spelling stays so existing
/// definitions keep verifying.
pub fn revision_content_digest(automation: &Automation, generation: u64) -> String {
    let envelope = serde_json::json!({
        "schema": "everyaios.automation.revision.v1",
        "generation": generation,
        "automation": automation,
    });
    sha256_hex(canonical_json(&envelope).as_bytes())
}

/// Build the canonical revision identity for a definition and generation.
pub fn content_addressed_revision_id_for_generation(
    automation: &Automation,
    generation: u64,
    revision: u64,
) -> String {
    format!(
        "g{generation}:r{revision}:{}",
        revision_content_digest(automation, generation)
    )
}

/// Backwards-compatible convenience for callers which have no persisted
/// generation yet.  Such callers use generation one explicitly.
pub fn content_addressed_revision_id(automation: &Automation, revision: u64) -> String {
    content_addressed_revision_id_for_generation(automation, 1, revision)
}

/// Alias with the generation in the name for host adapters.
pub fn content_addressed_revision_id_with_generation(
    automation: &Automation,
    generation: u64,
    revision: u64,
) -> String {
    content_addressed_revision_id_for_generation(automation, generation, revision)
}

/// Parse and validate the shape of a canonical revision identity.
pub fn parse_revision_id(revision_id: &str) -> Result<RevisionIdentity, AutomationError> {
    let parts: Vec<&str> = revision_id.trim().split(':').collect();
    if parts.len() != 3 {
        return Err(AutomationError::InvalidRevision(revision_id.to_string()));
    }
    let generation_text = parts[0]
        .strip_prefix('g')
        .ok_or_else(|| AutomationError::InvalidRevision(revision_id.to_string()))?;
    let generation = generation_text
        .parse::<u64>()
        .ok()
        .filter(|value| value.to_string() == generation_text)
        .ok_or_else(|| AutomationError::InvalidRevision(revision_id.to_string()))?;
    let revision_text = parts[1]
        .strip_prefix('r')
        .ok_or_else(|| AutomationError::InvalidRevision(revision_id.to_string()))?;
    let revision = revision_text
        .parse::<u64>()
        .ok()
        .filter(|value| value.to_string() == revision_text)
        .ok_or_else(|| AutomationError::InvalidRevision(revision_id.to_string()))?;
    let digest = parts[2];
    if generation == 0
        || revision == 0
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AutomationError::InvalidRevision(revision_id.to_string()));
    }
    Ok(RevisionIdentity {
        generation,
        revision,
        digest: digest.to_string(),
    })
}

/// Validate that an id names the exact immutable definition supplied to the
/// factory.  This is deliberately not a string-shape check.
pub fn validate_revision_id(
    automation: &Automation,
    revision_id: &str,
) -> Result<RevisionIdentity, AutomationError> {
    let identity = parse_revision_id(revision_id)?;
    if identity.digest != revision_content_digest(automation, identity.generation) {
        return Err(AutomationError::RevisionMismatch);
    }
    Ok(identity)
}

/// Compile one immutable automation revision into a Work specification.
/// This function is pure: it has no persistence, Work, effect, or retry path.
pub fn compile_work(
    automation: &Automation,
    revision_id: &str,
    trigger_occurrence_id: &str,
) -> Result<WorkSpec, AutomationError> {
    if automation.steps.is_empty() {
        return Err(AutomationError::Empty);
    }
    if canonical_automation_id(&automation.id).trim().is_empty() {
        return Err(AutomationError::InvalidRevision(
            "automation has no id".into(),
        ));
    }
    let revision_id = revision_id.trim();
    if revision_id.is_empty() {
        return Err(AutomationError::MissingRevision);
    }
    let identity = validate_revision_id(automation, revision_id)?;
    let trigger_occurrence_id = trigger_occurrence_id.trim();
    if trigger_occurrence_id.is_empty() {
        return Err(AutomationError::MissingOccurrence);
    }
    for step in &automation.steps {
        validate_step(step)?;
    }

    let steps: Vec<CompiledStep> = automation
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| CompiledStep {
            index,
            kind: kind_of(step).into(),
            deterministic: !is_agent_backed(step),
            capability_request: Some(capability_of(index, step)),
        })
        .collect();
    let capability_requests = steps
        .iter()
        .filter_map(|step| step.capability_request.clone())
        .collect();

    Ok(WorkSpec {
        provenance: AutomationProvenance {
            automation_id: canonical_automation_id(&automation.id).to_string(),
            revision_id: revision_id.to_string(),
            automation_generation: identity.generation,
            trigger_occurrence_id: trigger_occurrence_id.to_string(),
        },
        objective: objective_of(automation),
        agent_required: steps.iter().any(|step| !step.deterministic),
        steps,
        capability_requests,
    })
}

fn is_agent_backed(_step: &AutomationStep) -> bool {
    false
}

fn objective_of(automation: &Automation) -> String {
    format!(
        "automation:{}:{}",
        canonical_automation_id(&automation.id),
        automation.name
    )
}

fn kind_of(step: &AutomationStep) -> &'static str {
    match step {
        AutomationStep::RunCode { .. } => "run_code",
        AutomationStep::OnlineSearch { .. } => "online_search",
        AutomationStep::Email { .. } => "email",
        AutomationStep::Calendar { .. } => "calendar",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_blueprint::{AutomationStep as Step, Trigger};

    fn automation() -> Automation {
        Automation::new("a1", "Morning brief", Trigger::Manual)
            .step(Step::OnlineSearch {
                query: "latest AI news".into(),
            })
            .step(Step::RunCode {
                language: "js".into(),
                code: "return 42".into(),
            })
    }

    #[test]
    fn content_addressed_revision_is_stable_and_generation_sensitive() {
        let first = automation();
        let same = automation();
        assert_eq!(
            content_addressed_revision_id(&first, 4),
            content_addressed_revision_id(&same, 4)
        );
        assert_ne!(
            content_addressed_revision_id_for_generation(&first, 1, 4),
            content_addressed_revision_id_for_generation(&first, 2, 4)
        );
    }

    #[test]
    fn compile_validates_digest_and_carries_generation() {
        let definition = automation();
        let id = content_addressed_revision_id(&definition, 4);
        let spec = compile_work(&definition, &id, "occ-revision").unwrap();
        assert_eq!(spec.provenance.automation_generation, 1);
        let mut changed = definition.clone();
        changed.name = "edited".into();
        assert_eq!(
            compile_work(&changed, &id, "occ-revision"),
            Err(AutomationError::RevisionMismatch)
        );
    }

    #[test]
    fn blank_revision_refuses_before_work_compilation() {
        assert_eq!(
            compile_work(&automation(), "  ", "occ-revision"),
            Err(AutomationError::MissingRevision)
        );
    }

    #[test]
    fn compiles_a_work_spec_with_provenance() {
        let definition = automation();
        let id = content_addressed_revision_id(&definition, 4);
        let spec = compile_work(&definition, &id, "occ-1").unwrap();
        assert_eq!(spec.provenance.automation_id, "a1");
        assert_eq!(spec.provenance.revision_id, id);
        assert_eq!(spec.provenance.automation_generation, 1);
        assert_eq!(spec.provenance.trigger_occurrence_id, "occ-1");
        assert_eq!(spec.objective, "automation:a1:Morning brief");
        assert_eq!(spec.steps.len(), 2);
        assert!(spec.steps.iter().all(|step| step.deterministic));
        assert!(!spec.agent_required);
    }

    #[test]
    fn capability_requests_are_instantiated_per_step() {
        let definition = automation();
        let id = content_addressed_revision_id(&definition, 4);
        let spec = compile_work(&definition, &id, "occ-1").unwrap();
        let ids: Vec<&str> = spec
            .capability_requests
            .iter()
            .map(|request| request.capability_id.as_str())
            .collect();
        assert_eq!(ids, vec!["net.search", "script.eval"]);
        assert_eq!(spec.capability_requests[0].step_index, 0);
    }

    #[test]
    fn email_and_calendar_request_connector_capabilities() {
        let definition = Automation::new("a2", "Send report", Trigger::Manual)
            .step(Step::Email {
                to: vec!["bob@x.test".into()],
                subject: "s".into(),
                body: "b".into(),
            })
            .step(Step::Calendar {
                title: "review".into(),
                when: "2026-09-21T10:00:00Z".into(),
            });
        let id = content_addressed_revision_id(&definition, 1);
        let spec = compile_work(&definition, &id, "occ-2").unwrap();
        let ids: Vec<&str> = spec
            .capability_requests
            .iter()
            .map(|request| request.capability_id.as_str())
            .collect();
        assert_eq!(ids, vec!["connector.email", "connector.calendar"]);
    }

    #[test]
    fn invalid_language_refuses_at_compile_time() {
        let definition = Automation::new("a3", "Calc", Trigger::Manual).step(Step::RunCode {
            language: "python".into(),
            code: "x".into(),
        });
        let id = content_addressed_revision_id(&definition, 1);
        assert_eq!(
            compile_work(&definition, &id, "occ-3"),
            Err(AutomationError::UnsupportedLanguage("python".into()))
        );
    }

    #[test]
    fn empty_definition_refuses() {
        let definition = Automation::new("a4", "Empty", Trigger::Manual);
        let id = content_addressed_revision_id(&definition, 1);
        assert_eq!(
            compile_work(&definition, &id, "occ-4"),
            Err(AutomationError::Empty)
        );
    }

    #[test]
    fn blank_occurrence_refuses() {
        let definition = automation();
        let id = content_addressed_revision_id(&definition, 1);
        assert_eq!(
            compile_work(&definition, &id, "  "),
            Err(AutomationError::MissingOccurrence)
        );
    }
}
