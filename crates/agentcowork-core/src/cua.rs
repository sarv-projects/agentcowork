//! P59 — two-surface computer-use routing + Worker inner loop.
//!
//! Fetched (not recalled):
//! - Agent-S `predict(instruction, observation) → next action` (one act, not
//!   click-until-max_steps) — https://github.com/simular-ai/Agent-S
//! - ARCH/17 + spec E: Office/fs/shell first, inbuilt Browse CDP second,
//!   real-OS CUA last. "Their Chrome.exe" is CUA, not `browser_start`.
//!
//! This module is the **Rust orchestrator** (P59.12): the planner LLM may
//! write remaining DAG nodes, but ready-frontier / halt / identical-fail
//! live here. No second engine — tools still dispatch through `ToolService`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a target must run. Preference ladder is code, not a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkSurface {
    Office,
    Browse,
    Desktop,
}

/// P59.1 / P59.11 — route a target string to the cheapest correct surface.
///
/// - Office file extensions our engines open → Office
/// - `http(s):` URL that belongs in our CDP child → Browse
/// - everything else (HWND, path-launch, "their Chrome.exe") → Desktop
pub fn route_work_surface(target: &str) -> WorkSurface {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    if looks_like_office(&lower) {
        return WorkSurface::Office;
    }
    if looks_like_url(&lower) {
        return WorkSurface::Browse;
    }
    WorkSurface::Desktop
}

fn looks_like_office(lower: &str) -> bool {
    const EXTS: &[&str] = &[
        ".docx", ".xlsx", ".pptx", ".pdf", ".doc", ".xls", ".ppt", ".odt", ".ods", ".odp",
    ];
    let path = lower.split(['?', '#']).next().unwrap_or(lower);
    EXTS.iter().any(|e| path.ends_with(e))
}

fn looks_like_url(lower: &str) -> bool {
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file://")
}

/// P59.2 — screenshot-to-model is refused unless the model accepts images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionGateError {
    pub code: &'static str,
    pub message: String,
}

pub const CUA_REQUIRES_VISION: &str = "cua_requires_vision";

/// `needs_screenshot` is true when the a11y tree is empty and pixels are the
/// only observation. Never attach a screenshot to a text-only body.
pub fn vision_gate(
    needs_screenshot: bool,
    model_accepts_image: bool,
) -> Result<(), VisionGateError> {
    if needs_screenshot && !model_accepts_image {
        return Err(VisionGateError {
            code: CUA_REQUIRES_VISION,
            message: "Computer use needs a vision model — pick one with image input (models.dev `images?` or a local VL).".into(),
        });
    }
    Ok(())
}

/// P59.5 — one DAG node. Done iff the verifier holds, never because the
/// model said so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuaNode {
    pub id: String,
    pub name: String,
    pub info: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub status: CuaNodeStatus,
    #[serde(default)]
    pub last_action: Option<String>,
    #[serde(default)]
    pub screenshot_ref: Option<String>,
    #[serde(default)]
    pub identical_fail_count: u32,
    /// P60.7 — FAILED budget (BLOCKED does not increment this).
    #[serde(default)]
    pub fail_count: u32,
    /// P59.13 — Done iff these hold. An empty list is illegal (split or escalate).
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub postconditions: Vec<String>,
    #[serde(default)]
    pub timeout_s: u32,
    #[serde(default)]
    pub retry: u32,
    /// P60.5 — five-part brief stored on the node, not implied from the transcript.
    #[serde(default)]
    pub brief: Option<FivePartBrief>,
    /// P60.10 — fabric letter: a=Office, b=Browse, c=Desktop CUA.
    #[serde(default)]
    pub fabric: Option<String>,
}

impl Default for CuaNode {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            info: String::new(),
            depends_on: Vec::new(),
            status: CuaNodeStatus::Pending,
            last_action: None,
            screenshot_ref: None,
            identical_fail_count: 0,
            fail_count: 0,
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            timeout_s: 0,
            retry: 0,
            brief: None,
            fabric: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CuaNodeStatus {
    #[default]
    Pending,
    Ready,
    Running,
    Verified,
    Halted,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseDag {
    pub run_id: String,
    pub work_id: String,
    pub nodes: Vec<CuaNode>,
    pub replan_seq: u32,
}

impl ComputerUseDag {
    /// Ready-frontier: nodes whose deps are Verified and that are still Pending.
    pub fn ready_frontier(&self) -> Vec<&CuaNode> {
        self.nodes
            .iter()
            .filter(|n| n.status == CuaNodeStatus::Pending)
            .filter(|n| {
                n.depends_on.iter().all(|d| {
                    self.nodes
                        .iter()
                        .any(|x| x.id == *d && x.status == CuaNodeStatus::Verified)
                })
            })
            .collect()
    }

    /// P59.7 — replan **remaining** nodes only. Verified nodes stay.
    pub fn replan_remaining(&mut self, remaining: Vec<CuaNode>) {
        self.nodes.retain(|n| n.status == CuaNodeStatus::Verified);
        self.nodes.extend(remaining);
        self.replan_seq = self.replan_seq.saturating_add(1);
    }

    /// P59.8 — the user may edit remaining steps only (not Verified).
    pub fn apply_remaining_edit(
        &mut self,
        node_id: &str,
        name: Option<String>,
        info: Option<String>,
    ) -> Result<(), String> {
        let node = self
            .nodes
            .iter_mut()
            .find(|n| n.id == node_id)
            .ok_or_else(|| format!("unknown CUA node {node_id}"))?;
        if node.status == CuaNodeStatus::Verified {
            return Err("verified nodes cannot be edited — replan remaining only".into());
        }
        if let Some(n) = name {
            if n.trim().is_empty() {
                return Err("remaining node name must not be empty".into());
            }
            node.name = n;
        }
        if let Some(i) = info {
            node.info = i;
        }
        self.replan_seq = self.replan_seq.saturating_add(1);
        Ok(())
    }
}

/// Persist the DAG next to the Work (survive restart).
pub fn persist_dag(dir: &Path, dag: &ComputerUseDag) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("dependency_graph.json");
    let tmp = dir.join("dependency_graph.json.tmp");
    let bytes = serde_json::to_vec_pretty(dag).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn load_dag(dir: &Path) -> Result<ComputerUseDag, String> {
    let path = dir.join("dependency_graph.json");
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// P59.6 / P59.14 — one Worker step: observe → **one** act → verify.
/// Two identical fails → Halt (not another click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationOutcome {
    Verified,
    Mismatch,
    Halt,
}

pub const IDENTICAL_FAIL_HALT: u32 = 2;

pub fn delegation_step(verify_ok: bool, identical_fail_count: u32) -> DelegationOutcome {
    if verify_ok {
        return DelegationOutcome::Verified;
    }
    if identical_fail_count.saturating_add(1) >= IDENTICAL_FAIL_HALT {
        return DelegationOutcome::Halt;
    }
    DelegationOutcome::Mismatch
}

/// P59.6 / P59.14 — apply one Worker outcome onto the node. Halt is a status,
/// never a success. OpenAdapt: do not summarize halt as VERIFIED.
pub fn apply_delegation_act(node: &mut CuaNode, verify_ok: bool) -> DelegationOutcome {
    let out = delegation_step(verify_ok, node.identical_fail_count);
    match out {
        DelegationOutcome::Verified => {
            node.status = CuaNodeStatus::Verified;
            node.identical_fail_count = 0;
        }
        DelegationOutcome::Mismatch => {
            node.identical_fail_count = node.identical_fail_count.saturating_add(1);
            node.status = CuaNodeStatus::Running;
        }
        DelegationOutcome::Halt => {
            node.identical_fail_count = node.identical_fail_count.saturating_add(1);
            node.status = CuaNodeStatus::Halted;
        }
    }
    out
}

/// P59.13 — unverifiable nodes are illegal (no empty postconditions).
pub fn node_contract_legal(node: &CuaNode) -> bool {
    !node.postconditions.is_empty()
}

/// P59.7 — why the Manager rewrote remaining nodes.
///
/// Fetched Agent-S `Manager.get_action_queue` (`gui_agents/s2/agents/manager.py`
/// @ 73ea172): on `failed_subtask` generate a **new plan for the remainder**;
/// on completion revise remaining given completed + remaining lists; then
/// `_generate_dag`. MACU: mutate remaining only; verified stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagerReplanReason {
    Halt,
    Completion,
    Planner,
}

impl ManagerReplanReason {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "halt" | "identical-fail-halt" | "failure" | "failed" => Self::Halt,
            "completion" | "complete" | "done" => Self::Completion,
            _ => Self::Planner,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Halt => "halt",
            Self::Completion => "completion",
            Self::Planner => "planner",
        }
    }
}

/// A remaining-node JSON object that carries a ticket, skipGuard, or
/// `approved: true` is trying to mint Guard-2 from the plan. Illegal.
/// Planner writes remaining JSON; clicks still ticket through `tool/exec`.
pub fn remaining_payload_skips_guard(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if obj.get("ticketId").is_some() || obj.get("ticket_id").is_some() {
        return true;
    }
    if obj.get("skipGuard").and_then(|v| v.as_bool()) == Some(true)
        || obj.get("skip_guard").and_then(|v| v.as_bool()) == Some(true)
        || obj.get("approved").and_then(|v| v.as_bool()) == Some(true)
    {
        return true;
    }
    false
}

/// Parse Manager remaining-node JSON. Accepts a node array, `{nodes:[…]}`,
/// or `{remaining:[…]}`. Does not call an LLM — the planner payload is injected.
pub fn parse_remaining_nodes(value: &serde_json::Value) -> Result<Vec<CuaNode>, String> {
    let arr = if let Some(a) = value.as_array() {
        a
    } else if let Some(a) = value.get("nodes").and_then(|n| n.as_array()) {
        a
    } else if let Some(a) = value.get("remaining").and_then(|n| n.as_array()) {
        a
    } else {
        return Err("manager remaining plan must be a JSON array of nodes".into());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        if remaining_payload_skips_guard(item) {
            return Err(
                "planned click cannot skip Guard-2 — remaining nodes are proposals, not tickets"
                    .into(),
            );
        }
        let node: CuaNode =
            serde_json::from_value(item.clone()).map_err(|e| format!("remaining node: {e}"))?;
        validate_remaining_node(&node)?;
        out.push(node);
    }
    Ok(out)
}

fn validate_remaining_node(node: &CuaNode) -> Result<(), String> {
    if node.id.trim().is_empty() {
        return Err("remaining node id must not be empty".into());
    }
    if !node_contract_legal(node) {
        return Err(format!(
            "unverifiable remaining node {} — postconditions required",
            node.id
        ));
    }
    if node.status == CuaNodeStatus::Verified {
        return Err(format!(
            "remaining node {} cannot be Verified — Manager rewrites remaining only",
            node.id
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerReplanResult {
    pub replan_seq: u32,
    pub verified_kept: usize,
    pub remaining: usize,
}

/// Apply a Manager remaining-node rewrite. Verified nodes stay; everything
/// else is replaced. Empty remaining is allowed (all work verified).
pub fn apply_manager_replan(
    dag: &mut ComputerUseDag,
    remaining: Vec<CuaNode>,
) -> Result<ManagerReplanResult, String> {
    for n in &remaining {
        validate_remaining_node(n)?;
    }
    let verified_kept = dag
        .nodes
        .iter()
        .filter(|n| n.status == CuaNodeStatus::Verified)
        .count();
    dag.replan_remaining(remaining);
    Ok(ManagerReplanResult {
        replan_seq: dag.replan_seq,
        verified_kept,
        remaining: dag.nodes.len().saturating_sub(verified_kept),
    })
}

/// P59.16 — a reusable CUA skill draft. Not an orchestrator: the DAG runtime
/// stays in Rust. Fetched Agent Skills spec (`name`+`description` required;
/// https://agentskills.io/specification) + spec E.6: SKILL.md + postconditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuaSkillDraft {
    pub name: String,
    pub description: String,
    pub postconditions: Vec<String>,
    pub node_ids: Vec<String>,
    pub body: String,
}

/// Promote a **fully verified** DAG into a SKILL.md draft. Halted/pending
/// traces refuse — one-off clicks are not the library until verify holds.
pub fn cua_skill_from_verified(
    dag: &ComputerUseDag,
    name: &str,
    description: &str,
) -> Result<CuaSkillDraft, String> {
    let slug = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() || slug.len() > 64 {
        return Err("CUA skill name must be 1–64 [a-z0-9-] characters".into());
    }
    if description.trim().is_empty() {
        return Err("CUA skill description is required (Agent Skills spec)".into());
    }
    if dag.nodes.is_empty() {
        return Err("no verified trace to promote".into());
    }
    if dag
        .nodes
        .iter()
        .any(|n| n.status != CuaNodeStatus::Verified)
    {
        return Err("one-off traces cannot become a skill until every node is verified".into());
    }
    for n in &dag.nodes {
        if !node_contract_legal(n) {
            return Err(format!(
                "unverifiable node {} — postconditions required",
                n.id
            ));
        }
    }
    let postconditions: Vec<String> = dag
        .nodes
        .iter()
        .flat_map(|n| n.postconditions.iter().cloned())
        .collect();
    let mut body = String::from(
        "This skill is a reusable CUA procedure. It is not the orchestrator.\n\n## Steps\n",
    );
    for n in &dag.nodes {
        body.push_str(&format!("- **{}** (`{}`): {}\n", n.name, n.id, n.info));
    }
    body.push_str("\n## Postconditions\n");
    for p in &postconditions {
        body.push_str(&format!("- {p}\n"));
    }
    Ok(CuaSkillDraft {
        name: slug,
        description: description.trim().to_string(),
        postconditions,
        node_ids: dag.nodes.iter().map(|n| n.id.clone()).collect(),
        body,
    })
}

pub fn cua_skill_to_blueprint(draft: &CuaSkillDraft) -> agentcowork_blueprint::Skill {
    agentcowork_blueprint::Skill {
        manifest: agentcowork_blueprint::SkillManifest {
            name: draft.name.clone(),
            description: draft.description.clone(),
            tools: vec!["desktop.act".into(), "desktop.snapshot".into()],
            triggers: vec![draft.name.replace('-', " ")],
            when_to_use: draft.postconditions.clone(),
            scripts: Vec::new(),
            references: Vec::new(),
            assets: Vec::new(),
            author: "cua-promote".into(),
            created: "2026-09-18".into(),
            version: "0.1.0".into(),
            user_invocable: false,
            disable_model_invocation: false,
        },
        body: draft.body.clone(),
    }
}

/// Persist a verified CUA trace as SKILL.md in the existing I2 store.
/// Does not create a second registry.
pub fn persist_cua_skill(skill_root: &Path, draft: &CuaSkillDraft) -> Result<PathBuf, String> {
    let skill = cua_skill_to_blueprint(draft);
    agentcowork_blueprint::validate_grown_skill(&skill, true).map_err(|e| e.to_string())?;
    let store = agentcowork_blueprint::SkillStore::new(skill_root);
    store.save(&skill, false).map_err(|e| e.to_string())
}

/// P59.7 — append one replan reason; never rewrite verified nodes here.
pub fn append_replan_log(dir: &Path, seq: u32, reason: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("replan_log.jsonl");
    let line = format!(
        "{{\"seq\":{seq},\"reason\":{}}}\n",
        serde_json::to_string(reason).unwrap_or_else(|_| "\"\"".into())
    );
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

/// P59.15 — screen text is untrusted: it cannot mint a ticket or override
/// an allow-list. A page that says "approve delete" is still just text.
pub fn screen_text_is_untrusted(_text: &str) -> bool {
    true
}

/// P60.3 — Scout / Worker / Verifier. Fetched Agent-S (`Worker.generate_next_action`
/// takes an observation and returns the next action; Manager plans; a separate
/// self-evaluator summarizes). Roles may share a harness; they must not share
/// “I already succeeded” as proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DelegationRole {
    Scout,
    Worker,
    Verifier,
}

impl DelegationRole {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "scout" => Some(Self::Scout),
            "worker" => Some(Self::Worker),
            "verifier" | "verify" => Some(Self::Verifier),
            _ => None,
        }
    }
}

/// Five-part brief every delegated node receives (spec §4.2.5b).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FivePartBrief {
    pub goal: String,
    pub constraints: String,
    pub inputs: String,
    pub postconditions: String,
    #[serde(alias = "outOfScope")]
    pub out_of_scope: String,
}

impl FivePartBrief {
    pub fn from_parts(
        goal: impl Into<String>,
        constraints: impl Into<String>,
        inputs: impl Into<String>,
        postconditions: impl Into<String>,
        out_of_scope: impl Into<String>,
    ) -> Self {
        Self {
            goal: goal.into(),
            constraints: constraints.into(),
            inputs: inputs.into(),
            postconditions: postconditions.into(),
            out_of_scope: out_of_scope.into(),
        }
    }

    pub fn is_complete(&self) -> bool {
        !self.goal.trim().is_empty()
            && !self.constraints.trim().is_empty()
            && !self.inputs.trim().is_empty()
            && !self.postconditions.trim().is_empty()
            && !self.out_of_scope.trim().is_empty()
    }
}

/// P60.5 — the primary agent writes the brief onto the node. A transcript dump is not a brief.
pub fn apply_five_part_brief(node: &mut CuaNode, brief: FivePartBrief) -> Result<(), String> {
    if !brief.is_complete() {
        return Err("five-part brief incomplete — Worker does not inherit the transcript".into());
    }
    if node.postconditions.is_empty() {
        node.postconditions = vec![brief.postconditions.clone()];
    }
    node.brief = Some(brief);
    Ok(())
}

/// Scout is structurally read-only: search / read / list / snapshot. Writes
/// and `desktop.act` are never granted even if the parent listed them.
pub const SCOUT_ALLOWED_TOOLS: &[&str] = &[
    "file_ops.read",
    "file_ops.list",
    "search.query",
    "grep",
    "codeintel.repomap",
    "browser.snapshot",
    "desktop.snapshot",
];

pub fn filter_tools_for_role(role: DelegationRole, tools: &[String]) -> Vec<String> {
    match role {
        DelegationRole::Worker => tools.to_vec(),
        DelegationRole::Scout | DelegationRole::Verifier => tools
            .iter()
            .filter(|t| SCOUT_ALLOWED_TOOLS.contains(&t.as_str()))
            .cloned()
            .collect(),
    }
}

/// Verifier never treats the Worker's "I succeeded" claim as proof.
/// Only a mechanical check (`mechanical_ok`) can confirm.
pub fn verifier_accepts_worker_claim(worker_claimed_success: bool, mechanical_ok: bool) -> bool {
    let _ = worker_claimed_success;
    mechanical_ok
}

/// P60.6 — mechanical verify first (spec §4.2.5b).
/// Fetched OpenAdapt: VERIFIED only if an independent system-of-record read
/// agrees. A success banner (`--break-it`) is not evidence. Disk is truth;
/// the Worker report is not. Re-run is O(1) (file/command result). No
/// verifier → sample, never auto-verify. Close-read only for small outputs.
pub const CLOSE_READ_MAX: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    #[default]
    None,
    FileExists,
    FileContains,
    FileEquals,
    CommandExit,
    CloseRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MechanicalEvidence {
    #[serde(default)]
    pub kind: EvidenceKind,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub expect: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MechanicalVerdict {
    /// Independent check passed. The only production success.
    Verified,
    /// Disk/command disagrees with the Worker (OpenAdapt `--break-it`).
    Refuted,
    /// No verifier configured — sample; never treat as Verified.
    Sampled,
}

/// Ignore `worker_claimed`. Read disk / injected command result.
pub fn mechanical_verify(
    worker_claimed_success: bool,
    evidence: &MechanicalEvidence,
) -> MechanicalVerdict {
    let _ = worker_claimed_success;
    match evidence.kind {
        EvidenceKind::None => MechanicalVerdict::Sampled,
        EvidenceKind::FileExists => match evidence.path.as_deref() {
            Some(p) if Path::new(p).is_file() => MechanicalVerdict::Verified,
            Some(_) => MechanicalVerdict::Refuted,
            None => MechanicalVerdict::Sampled,
        },
        EvidenceKind::FileContains => {
            let (Some(p), Some(exp)) = (evidence.path.as_deref(), evidence.expect.as_deref())
            else {
                return MechanicalVerdict::Sampled;
            };
            match std::fs::read_to_string(p) {
                Ok(s) if s.contains(exp) => MechanicalVerdict::Verified,
                Ok(_) | Err(_) => MechanicalVerdict::Refuted,
            }
        }
        EvidenceKind::FileEquals => {
            let (Some(p), Some(exp)) = (evidence.path.as_deref(), evidence.expect.as_deref())
            else {
                return MechanicalVerdict::Sampled;
            };
            match std::fs::read(p) {
                Ok(b) if b == exp.as_bytes() => MechanicalVerdict::Verified,
                Ok(_) | Err(_) => MechanicalVerdict::Refuted,
            }
        }
        EvidenceKind::CommandExit => match evidence.exit_code {
            Some(0) => MechanicalVerdict::Verified,
            Some(_) => MechanicalVerdict::Refuted,
            None => MechanicalVerdict::Sampled,
        },
        EvidenceKind::CloseRead => {
            let Some(text) = evidence.text.as_deref() else {
                return MechanicalVerdict::Sampled;
            };
            if text.len() > CLOSE_READ_MAX {
                return MechanicalVerdict::Sampled;
            }
            let Some(exp) = evidence.expect.as_deref() else {
                return MechanicalVerdict::Sampled;
            };
            if text.contains(exp) {
                MechanicalVerdict::Verified
            } else {
                MechanicalVerdict::Refuted
            }
        }
    }
}

/// Apply the independent verdict onto the node. Sampled does not count as
/// success and does not burn the identical-fail budget.
pub fn apply_mechanical_verify(
    node: &mut CuaNode,
    worker_claimed_success: bool,
    evidence: &MechanicalEvidence,
) -> MechanicalVerdict {
    let verdict = mechanical_verify(worker_claimed_success, evidence);
    match verdict {
        MechanicalVerdict::Verified => {
            let _ = apply_delegation_act(node, true);
        }
        MechanicalVerdict::Refuted => {
            let _ = apply_delegation_act(node, false);
        }
        MechanicalVerdict::Sampled => {
            // Disk did not speak. Do not mark Verified. Do not increment fails.
        }
    }
    verdict
}

/// P60.7 — BLOCKED ≠ FAILED. Missing permission/info does not burn the
/// fail budget. FAILED ×3 → the primary agent reclaims the node.
pub const FAILED_RECLAIM_AFTER: u32 = 3;

pub fn stop_is_blocked(reason: &str) -> bool {
    matches!(
        reason.trim().to_ascii_lowercase().as_str(),
        "permission" | "missing-info" | "missing_info" | "blocked"
    )
}

/// Returns whether the primary agent must reclaim (do the work or replan).
pub fn apply_node_stop(node: &mut CuaNode, reason: &str) -> bool {
    if stop_is_blocked(reason) {
        node.status = CuaNodeStatus::Blocked;
        return false;
    }
    node.fail_count = node.fail_count.saturating_add(1);
    if node.fail_count >= FAILED_RECLAIM_AFTER {
        node.status = CuaNodeStatus::Halted;
        true
    } else {
        node.status = CuaNodeStatus::Running;
        false
    }
}

/// P60.1 — five planes that must stay distinct (spec §4.2.5b).
/// Governance is code, not a prompt. The primary agent is not the workhorse.
/// Orchestrator is the Rust DAG. Subagent is a role, not a CLI name.
/// Harness and model swap independently. No “claude-subagent” type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePlane {
    Governance,
    Primary,
    Orchestrator,
    Subagent,
    HarnessModel,
}

pub const RUNTIME_PLANES: [RuntimePlane; 5] = [
    RuntimePlane::Governance,
    RuntimePlane::Primary,
    RuntimePlane::Orchestrator,
    RuntimePlane::Subagent,
    RuntimePlane::HarnessModel,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBinding {
    pub harness: String,
    pub model: String,
    #[serde(default)]
    pub role: Option<DelegationRole>,
    #[serde(default)]
    pub chief: Option<String>,
}

pub fn refuse_cli_named_subagent(label: &str) -> Result<(), String> {
    let l = label.trim().to_ascii_lowercase();
    let named = ["claude", "codex", "opencode", "gemini", "cursor"]
        .iter()
        .any(|p| l.contains(p));
    if named && l.contains("subagent") {
        return Err(
            "no CLI-named subagent type — harness, model, and role are independent planes".into(),
        );
    }
    Ok(())
}

pub fn bind_runtime(
    harness: &str,
    model: &str,
    role: Option<DelegationRole>,
    chief: Option<String>,
) -> Result<RuntimeBinding, String> {
    refuse_cli_named_subagent(harness)?;
    refuse_cli_named_subagent(model)?;
    if harness.trim().is_empty() {
        return Err("harness required (independent of model)".into());
    }
    if model.trim().is_empty() {
        return Err("model required (independent of harness)".into());
    }
    Ok(RuntimeBinding {
        harness: harness.trim().to_string(),
        model: model.trim().to_string(),
        role,
        chief,
    })
}

/// P60.2 — vision-first perception fusion. Fetched Agent-S Worker
/// `generate_next_action`: `assign_screenshot(obs)` then
/// `image_content=obs["screenshot"]`. A11y/DOM/OCR/API augment; if the tree
/// is missing or lying, vision still works. Structure cannot replace pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PerceptionLayers {
    pub screenshot_ref: Option<String>,
    pub a11y: Option<String>,
    pub dom: Option<String>,
    pub ocr: Option<String>,
    pub api: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneGraph {
    pub vision_first: bool,
    pub structure_augments: bool,
    pub structure_lying: bool,
    pub usable: bool,
}

pub fn fuse_perception(layers: &PerceptionLayers) -> Result<SceneGraph, String> {
    let vision = layers
        .screenshot_ref
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !vision {
        return Err(
            "CUA perception requires a screenshot — structure cannot replace vision".into(),
        );
    }
    let nonempty = |o: &Option<String>| o.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    let lying = layers.a11y.as_deref() == Some("lying")
        || layers.dom.as_deref() == Some("lying")
        || layers.a11y.as_deref() == Some("false");
    let structure =
        (nonempty(&layers.a11y) || nonempty(&layers.dom) || nonempty(&layers.ocr)) && !lying;
    Ok(SceneGraph {
        vision_first: true,
        structure_augments: structure,
        structure_lying: lying,
        usable: true,
    })
}

/// P60.4 — cheapest *reliable* combo. Not “all workers cheap.”
/// Vision needs VL/frontier. Verify fail escalates one tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelTier {
    Cheap,
    Mid,
    Frontier,
    Local,
    Vl,
}

impl ModelTier {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mid" => Self::Mid,
            "frontier" => Self::Frontier,
            "local" => Self::Local,
            "vl" | "vision" => Self::Vl,
            _ => Self::Cheap,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cheap => "cheap",
            Self::Mid => "mid",
            Self::Frontier => "frontier",
            Self::Local => "local",
            Self::Vl => "vl",
        }
    }
}

pub fn pick_combo(needs_vision: bool, verify_failed: bool, current: ModelTier) -> ModelTier {
    let mut t = current;
    if needs_vision && t != ModelTier::Vl && t != ModelTier::Frontier {
        t = ModelTier::Vl;
    }
    if verify_failed {
        t = match t {
            ModelTier::Cheap | ModelTier::Local => ModelTier::Mid,
            ModelTier::Mid | ModelTier::Vl => ModelTier::Frontier,
            ModelTier::Frontier => ModelTier::Frontier,
        };
    }
    t
}

/// P60.8 — Case A/B/C. Occupancy stays the primary agent (F8); workers may differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessModelCase {
    /// Different harness and different model from the primary agent.
    A,
    /// Same harness and same model as the primary agent.
    B,
    /// Same harness, different model.
    C,
}

pub fn classify_harness_model_case(
    primary_harness: &str,
    primary_model: &str,
    worker_harness: &str,
    worker_model: &str,
) -> HarnessModelCase {
    let same_h = primary_harness
        .trim()
        .eq_ignore_ascii_case(worker_harness.trim());
    let same_m = primary_model
        .trim()
        .eq_ignore_ascii_case(worker_model.trim());
    match (same_h, same_m) {
        (false, _) => HarnessModelCase::A,
        (true, true) => HarnessModelCase::B,
        (true, false) => HarnessModelCase::C,
    }
}

/// P60.9 — primary-agent share of run spend. Target < 20%; warn, do not abort.
pub const PRIMARY_SPEND_WARN: f64 = 0.20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimarySpend {
    pub primary_tokens: u64,
    pub worker_tokens: u64,
    pub share: f64,
    pub warn_not_delegating: bool,
}

pub fn split_primary_spend(primary_tokens: u64, worker_tokens: u64) -> PrimarySpend {
    let total = primary_tokens.saturating_add(worker_tokens);
    let share = if total == 0 {
        0.0
    } else {
        primary_tokens as f64 / total as f64
    };
    PrimarySpend {
        primary_tokens,
        worker_tokens,
        share,
        warn_not_delegating: total > 0 && share > PRIMARY_SPEND_WARN,
    }
}

/// P60.10 — Office/Browse are cheap execution; Desktop CUA is vision-first
/// perception. Letters a/b/c match spec §4.2.5b.
pub fn fabric_letter(surface: WorkSurface) -> &'static str {
    match surface {
        WorkSurface::Office => "a",
        WorkSurface::Browse => "b",
        WorkSurface::Desktop => "c",
    }
}

pub fn fabric_is_perception(surface: WorkSurface) -> bool {
    surface == WorkSurface::Desktop
}

pub fn apply_fabric(node: &mut CuaNode, surface: WorkSurface) {
    node.fabric = Some(fabric_letter(surface).into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p59_ladder_office_before_browse_before_desktop() {
        assert_eq!(route_work_surface("report.xlsx"), WorkSurface::Office);
        assert_eq!(route_work_surface("/tmp/a.DOCX"), WorkSurface::Office);
        assert_eq!(
            route_work_surface("https://example.com/app"),
            WorkSurface::Browse
        );
        assert_eq!(
            route_work_surface("C:\\\\Program Files\\\\Chrome.exe"),
            WorkSurface::Desktop
        );
        assert_eq!(route_work_surface("QuickBooks"), WorkSurface::Desktop);
    }

    #[test]
    fn p59_vision_gate_refuses_text_only_models() {
        assert!(vision_gate(true, false).unwrap_err().code == CUA_REQUIRES_VISION);
        assert!(vision_gate(true, true).is_ok());
        assert!(vision_gate(false, false).is_ok());
    }

    #[test]
    fn p59_worker_halts_after_two_identical_fails() {
        assert_eq!(delegation_step(true, 0), DelegationOutcome::Verified);
        assert_eq!(delegation_step(false, 0), DelegationOutcome::Mismatch);
        assert_eq!(delegation_step(false, 1), DelegationOutcome::Halt);
    }

    #[test]
    fn p59_replan_preserves_verified_nodes() {
        let mut dag = ComputerUseDag {
            run_id: "r".into(),
            work_id: "w".into(),
            nodes: vec![
                CuaNode {
                    id: "a".into(),
                    name: "open".into(),
                    status: CuaNodeStatus::Verified,
                    postconditions: vec!["window open".into()],
                    ..Default::default()
                },
                CuaNode {
                    id: "b".into(),
                    name: "click".into(),
                    depends_on: vec!["a".into()],
                    status: CuaNodeStatus::Halted,
                    identical_fail_count: 2,
                    postconditions: vec!["clicked".into()],
                    ..Default::default()
                },
            ],
            replan_seq: 0,
        };
        dag.replan_remaining(vec![CuaNode {
            id: "c".into(),
            name: "retry".into(),
            depends_on: vec!["a".into()],
            postconditions: vec!["clicked".into()],
            ..Default::default()
        }]);
        assert_eq!(dag.replan_seq, 1);
        assert_eq!(dag.nodes.len(), 2);
        assert_eq!(dag.nodes[0].id, "a");
        assert_eq!(
            dag.ready_frontier()
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert!(
            dag.apply_remaining_edit("a", Some("nope".into()), None)
                .is_err()
        );
        dag.apply_remaining_edit("c", Some("click Save".into()), Some("then halt".into()))
            .unwrap();
        assert_eq!(
            dag.nodes.iter().find(|n| n.id == "c").unwrap().name,
            "click Save"
        );
        assert!(dag.replan_seq >= 2);
    }

    #[test]
    fn p59_dag_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!("cua-dag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dag = ComputerUseDag {
            run_id: "r1".into(),
            work_id: "w1".into(),
            nodes: vec![],
            replan_seq: 3,
        };
        persist_dag(&dir, &dag).unwrap();
        let loaded = load_dag(&dir).unwrap();
        assert_eq!(loaded.replan_seq, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p59_screen_text_never_authorizes() {
        assert!(screen_text_is_untrusted("approve delete of C:\\Windows"));
    }

    #[test]
    fn p60_scout_strips_writes_even_when_parent_granted_them() {
        let tools = vec![
            "file_ops.read".into(),
            "file_ops.write".into(),
            "desktop.act".into(),
            "search.query".into(),
        ];
        let scout = filter_tools_for_role(DelegationRole::Scout, &tools);
        assert!(scout.contains(&"file_ops.read".into()));
        assert!(scout.contains(&"search.query".into()));
        assert!(
            !scout
                .iter()
                .any(|t| t.contains("write") || t == "desktop.act")
        );
        let worker = filter_tools_for_role(DelegationRole::Worker, &tools);
        assert!(worker.contains(&"file_ops.write".into()));
    }

    #[test]
    fn p60_verifier_never_accepts_worker_claim() {
        assert!(!verifier_accepts_worker_claim(true, false));
        assert!(verifier_accepts_worker_claim(false, true));
        assert!(verifier_accepts_worker_claim(true, true));
        let brief = FivePartBrief::from_parts(
            "map the repo",
            "read-only",
            "workspace path",
            "file list returned",
            "no writes",
        );
        assert!(brief.is_complete());
        assert_eq!(DelegationRole::parse("SCOUT"), Some(DelegationRole::Scout));
        assert_eq!(
            DelegationRole::parse("verify"),
            Some(DelegationRole::Verifier)
        );
        assert_eq!(DelegationRole::parse("nope"), None);
        let mut node = CuaNode {
            id: "w".into(),
            postconditions: vec!["file list returned".into()],
            ..Default::default()
        };
        assert!(
            apply_five_part_brief(&mut node, FivePartBrief::from_parts("", "c", "i", "p", "o"))
                .is_err()
        );
        apply_five_part_brief(&mut node, brief.clone()).unwrap();
        assert!(node.brief.as_ref().unwrap().is_complete());
    }

    #[test]
    fn p60_mechanical_verify_disk_is_truth_worker_claim_is_not() {
        let dir = std::env::temp_dir().join(format!("cua-mech-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("record.txt");
        std::fs::write(&path, "saved:yes").unwrap();
        let ok = MechanicalEvidence {
            kind: EvidenceKind::FileContains,
            path: Some(path.display().to_string()),
            expect: Some("saved:yes".into()),
            ..Default::default()
        };
        // OpenAdapt --break-it: Worker/banner claims success, disk agrees here.
        assert_eq!(mechanical_verify(false, &ok), MechanicalVerdict::Verified);
        let missing = MechanicalEvidence {
            kind: EvidenceKind::FileContains,
            path: Some(path.display().to_string()),
            expect: Some("committed".into()),
            ..Default::default()
        };
        // Banner/worker claim true; independent read refutes.
        assert_eq!(
            mechanical_verify(true, &missing),
            MechanicalVerdict::Refuted
        );
        assert_eq!(
            mechanical_verify(true, &MechanicalEvidence::default()),
            MechanicalVerdict::Sampled
        );
        let huge = "x".repeat(CLOSE_READ_MAX + 1);
        assert_eq!(
            mechanical_verify(
                true,
                &MechanicalEvidence {
                    kind: EvidenceKind::CloseRead,
                    text: Some(huge),
                    expect: Some("x".into()),
                    ..Default::default()
                }
            ),
            MechanicalVerdict::Sampled
        );
        let mut node = CuaNode {
            id: "n".into(),
            postconditions: vec!["saved:yes".into()],
            ..Default::default()
        };
        assert_eq!(
            apply_mechanical_verify(&mut node, true, &missing),
            MechanicalVerdict::Refuted
        );
        assert_ne!(node.status, CuaNodeStatus::Verified);
        assert_eq!(
            apply_mechanical_verify(&mut node, false, &ok),
            MechanicalVerdict::Verified
        );
        assert_eq!(node.status, CuaNodeStatus::Verified);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p59_apply_worker_act_halts_and_never_marks_halt_verified() {
        let mut n = CuaNode {
            id: "n".into(),
            postconditions: vec!["field equals saved".into()],
            ..Default::default()
        };
        assert!(node_contract_legal(&n));
        assert_eq!(
            apply_delegation_act(&mut n, false),
            DelegationOutcome::Mismatch
        );
        assert_eq!(n.status, CuaNodeStatus::Running);
        assert_eq!(apply_delegation_act(&mut n, false), DelegationOutcome::Halt);
        assert_eq!(n.status, CuaNodeStatus::Halted);
        assert_ne!(n.status, CuaNodeStatus::Verified);
        let empty = CuaNode::default();
        assert!(!node_contract_legal(&empty));
    }

    #[test]
    fn p59_replan_log_appends() {
        let dir = std::env::temp_dir().join(format!("cua-replan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        append_replan_log(&dir, 1, "double-fail").unwrap();
        append_replan_log(&dir, 2, "user edit").unwrap();
        let text = std::fs::read_to_string(dir.join("replan_log.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("double-fail"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p59_manager_rewrite_keeps_verified_and_refuses_illegal_remaining() {
        let mut dag = ComputerUseDag {
            run_id: "r".into(),
            work_id: "w".into(),
            nodes: vec![
                CuaNode {
                    id: "open".into(),
                    name: "open app".into(),
                    status: CuaNodeStatus::Verified,
                    postconditions: vec!["window visible".into()],
                    ..Default::default()
                },
                CuaNode {
                    id: "click".into(),
                    name: "click save".into(),
                    depends_on: vec!["open".into()],
                    status: CuaNodeStatus::Halted,
                    identical_fail_count: 2,
                    postconditions: vec!["saved".into()],
                    ..Default::default()
                },
            ],
            replan_seq: 0,
        };
        let remaining = parse_remaining_nodes(&serde_json::json!([
            {
                "id": "alt",
                "name": "use File > Save",
                "info": "menu instead of toolbar",
                "depends_on": ["open"],
                "postconditions": ["saved"]
            }
        ]))
        .unwrap();
        let out = apply_manager_replan(&mut dag, remaining).unwrap();
        assert_eq!(out.verified_kept, 1);
        assert_eq!(out.remaining, 1);
        assert_eq!(dag.replan_seq, 1);
        assert_eq!(dag.nodes[0].id, "open");
        assert_eq!(dag.nodes[0].status, CuaNodeStatus::Verified);
        assert_eq!(dag.nodes[1].id, "alt");
        assert_eq!(dag.nodes[1].status, CuaNodeStatus::Pending);
        assert!(
            parse_remaining_nodes(&serde_json::json!([{
                "id": "bad",
                "name": "click",
                "info": "",
                "postconditions": []
            }]))
            .is_err()
        );
        assert!(
            parse_remaining_nodes(&serde_json::json!([{
                "id": "skip",
                "name": "click Save",
                "info": "approve",
                "postconditions": ["saved"],
                "ticketId": "t-forged"
            }]))
            .unwrap_err()
            .contains("Guard-2")
        );
        assert!(remaining_payload_skips_guard(&serde_json::json!({
            "id": "c",
            "skipGuard": true,
            "postconditions": ["saved"]
        })));
        assert!(!remaining_payload_skips_guard(&serde_json::json!({
            "id": "c",
            "name": "click",
            "postconditions": ["saved"]
        })));
        assert_eq!(
            ManagerReplanReason::parse("identical-fail-halt"),
            ManagerReplanReason::Halt
        );
        assert_eq!(
            ManagerReplanReason::parse("completion"),
            ManagerReplanReason::Completion
        );
    }

    #[test]
    fn p59_cua_skill_promotes_only_fully_verified_traces() {
        let mut dag = ComputerUseDag {
            run_id: "r".into(),
            work_id: "w".into(),
            nodes: vec![CuaNode {
                id: "login".into(),
                name: "login to X".into(),
                info: "type user then submit".into(),
                status: CuaNodeStatus::Halted,
                postconditions: vec!["inbox visible".into()],
                ..Default::default()
            }],
            replan_seq: 0,
        };
        assert!(cua_skill_from_verified(&dag, "login-to-x", "Login to X").is_err());
        dag.nodes[0].status = CuaNodeStatus::Verified;
        let draft =
            cua_skill_from_verified(&dag, "Login to X", "Login to X when the user asks").unwrap();
        assert_eq!(draft.name, "login-to-x");
        assert!(draft.body.contains("## Postconditions"));
        assert!(draft.postconditions.contains(&"inbox visible".into()));
        let skill = cua_skill_to_blueprint(&draft);
        assert_eq!(skill.manifest.name, "login-to-x");
        assert!(skill.body.contains("inbox visible"));
        let dir = std::env::temp_dir().join(format!("cua-skill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = persist_cua_skill(&dir, &draft).unwrap();
        assert!(path.ends_with("SKILL.md"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("name: login-to-x"));
        assert!(text.contains("inbox visible"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p60_blocked_does_not_burn_fail_budget_failed_three_reclaims() {
        let mut n = CuaNode {
            id: "n".into(),
            postconditions: vec!["x".into()],
            ..Default::default()
        };
        assert!(!apply_node_stop(&mut n, "permission"));
        assert_eq!(n.status, CuaNodeStatus::Blocked);
        assert_eq!(n.fail_count, 0);
        assert!(!apply_node_stop(&mut n, "missing-info"));
        assert_eq!(n.fail_count, 0);
        assert!(!apply_node_stop(&mut n, "crash"));
        assert!(!apply_node_stop(&mut n, "crash"));
        assert_eq!(n.status, CuaNodeStatus::Running);
        assert_eq!(n.fail_count, 2);
        assert!(apply_node_stop(&mut n, "crash"));
        assert_eq!(n.status, CuaNodeStatus::Halted);
        assert_eq!(n.fail_count, 3);
    }

    #[test]
    fn p60_five_planes_refuse_cli_named_subagent_and_bind_independently() {
        assert_eq!(RUNTIME_PLANES.len(), 5);
        assert!(refuse_cli_named_subagent("claude-subagent").is_err());
        assert!(refuse_cli_named_subagent("codex-subagent").is_err());
        let b = bind_runtime(
            "inbuilt",
            "local-vl",
            Some(DelegationRole::Worker),
            Some("inbuilt".into()),
        )
        .unwrap();
        assert_eq!(b.harness, "inbuilt");
        assert_eq!(b.model, "local-vl");
        assert_eq!(b.role, Some(DelegationRole::Worker));
        assert!(bind_runtime("claude-subagent", "opus", None, None).is_err());
        let a = bind_runtime("inbuilt", "cheap", Some(DelegationRole::Scout), None).unwrap();
        let c = bind_runtime("acp:other", "frontier", Some(DelegationRole::Worker), None).unwrap();
        assert_ne!(a.harness, c.harness);
        assert_ne!(a.model, c.model);
    }

    #[test]
    fn p60_perception_fusion_vision_first_structure_augments() {
        let missing = PerceptionLayers {
            a11y: Some("button Save".into()),
            ..Default::default()
        };
        assert!(fuse_perception(&missing).is_err());
        let vision_only = PerceptionLayers {
            screenshot_ref: Some("shot:1".into()),
            ..Default::default()
        };
        let g = fuse_perception(&vision_only).unwrap();
        assert!(g.vision_first);
        assert!(g.usable);
        assert!(!g.structure_augments);
        let lying = PerceptionLayers {
            screenshot_ref: Some("shot:1".into()),
            a11y: Some("lying".into()),
            ..Default::default()
        };
        let g2 = fuse_perception(&lying).unwrap();
        assert!(g2.usable);
        assert!(g2.structure_lying);
        assert!(!g2.structure_augments);
        let fused = PerceptionLayers {
            screenshot_ref: Some("shot:1".into()),
            a11y: Some("role=button name=Save".into()),
            ocr: Some("Save".into()),
            ..Default::default()
        };
        let g3 = fuse_perception(&fused).unwrap();
        assert!(g3.structure_augments);
    }

    #[test]
    fn p60_pick_combo_not_all_workers_cheap_escalates_on_fail() {
        assert_eq!(pick_combo(false, false, ModelTier::Cheap), ModelTier::Cheap);
        assert_eq!(pick_combo(true, false, ModelTier::Cheap), ModelTier::Vl);
        assert_eq!(pick_combo(false, true, ModelTier::Cheap), ModelTier::Mid);
        assert_eq!(
            pick_combo(true, true, ModelTier::Cheap),
            ModelTier::Frontier
        );
        assert_eq!(
            pick_combo(false, true, ModelTier::Frontier),
            ModelTier::Frontier
        );
    }

    #[test]
    fn p60_harness_model_cases_abc() {
        assert_eq!(
            classify_harness_model_case("inbuilt", "llama", "acp:other", "opus"),
            HarnessModelCase::A
        );
        assert_eq!(
            classify_harness_model_case("inbuilt", "llama", "inbuilt", "llama"),
            HarnessModelCase::B
        );
        assert_eq!(
            classify_harness_model_case("inbuilt", "llama", "inbuilt", "vl"),
            HarnessModelCase::C
        );
    }

    #[test]
    fn p60_chief_spend_warns_above_twenty_percent() {
        let ok = split_primary_spend(10, 90);
        assert!((ok.share - 0.10).abs() < 1e-9);
        assert!(!ok.warn_not_delegating);
        let warn = split_primary_spend(30, 70);
        assert!(warn.warn_not_delegating);
        assert!(!split_primary_spend(0, 0).warn_not_delegating);
    }

    #[test]
    fn p60_fabric_office_browse_execution_desktop_perception() {
        assert_eq!(fabric_letter(WorkSurface::Office), "a");
        assert_eq!(fabric_letter(WorkSurface::Browse), "b");
        assert_eq!(fabric_letter(WorkSurface::Desktop), "c");
        assert!(!fabric_is_perception(WorkSurface::Office));
        assert!(fabric_is_perception(WorkSurface::Desktop));
        let mut n = CuaNode {
            id: "n".into(),
            postconditions: vec!["x".into()],
            ..Default::default()
        };
        apply_fabric(&mut n, WorkSurface::Desktop);
        assert_eq!(n.fabric.as_deref(), Some("c"));
    }
}
