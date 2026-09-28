//! Stage 0 — guard-gated tool executor (S0.1).
//!
//! Sidecar proposes (`tool/exec` pre-flight + `tool/commit`); Rust disposes:
//! Guard-1 scan → `GuardService::evaluate` (ticket) → `use_ticket` → dispatch
//! → Merkle audit row. Catalog ids come from `agentcowork-mcp` (42 tools) plus
//! `script.run`, `file_ops.*`, and `search.query`.
//!
//! Browser CDP / G8 search / office mutation engines are **dispatched**
//! through the same ticket path. Search uses `agentcowork-search::G8Cascade`.
//! Office mutations run `agentcowork-office` against a path-floored file.
//! Browser tools need an attached [`BrowserBackend`] (CDP session).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agentcowork_audit::{AuditEvent, merkle::MerkleChain};
use agentcowork_guard::CapabilityBroker;
use agentcowork_guard::deflection::{DEFLECTION_AUDIT_KIND, DeflectionNudge, deflect_shell_bias};
use agentcowork_guard::{
    ConnectivityMode, DecisionPackage, EgressEngine, EgressVerdict, NetPolicy, Operation,
    ResourceBinding, RiskLevel, RiskTier, bind_exec_bytes, bind_path, bind_url, open_parent_dir,
    pathfloor::{FloorVerdict, enforce_floor},
    reverify_exec, reverify_path, reverify_url, scan_all, urlfloor,
};
use agentcowork_mcp::{ArgDef, ArgKind, ExternalTool, ToolDef, ToolKind, all_tools};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::guard_service::{GuardDecision, GuardService};

const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_SCAN_SAMPLE: usize = 50;
/// P2.3 — hard cap for `download_file` (64 MiB); an oversized response is
/// refused, not buffered past the cap.
const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

/// P2.3 (E2) — the browser engine seam behind the three file-op tools
/// (`save_pdf_enhanced` / `save_screenshot_enhanced` / `download_file`). The
/// executor owns no browser; a host that has an attached CDP session injects
/// this so those tools route real pixels/PDF to disk. When absent the tools
/// fail honestly ("browser session not attached"). `download_file` is a pure
/// HTTP fetch and does NOT need this backend.
pub trait BrowserBackend: Send + Sync {
    /// `Page.printToPDF` → write the PDF under `dir`, return the absolute path.
    fn save_pdf_enhanced(&self, dir: &Path) -> Result<String, String>;
    /// `Page.captureScreenshot` → write a JPEG under `dir`, return the path.
    fn save_screenshot_enhanced(&self, dir: &Path, quality: u8) -> Result<String, String>;
    /// A11y snapshot text of the current page.
    fn snapshot(&self) -> Result<String, String> {
        Err("browser session not attached".into())
    }
    /// Navigate the attached page.
    fn navigate(&self, _url: &str) -> Result<String, String> {
        Err("browser session not attached".into())
    }
    /// Click / type against an a11y ref (`[ref=eN]`).
    fn act(
        &self,
        _kind: &str,
        _selector: Option<&str>,
        _text: Option<&str>,
    ) -> Result<String, String> {
        Err("browser session not attached".into())
    }
}

/// Family a registered tool belongs to (dispatch key).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolFamily {
    Browser,
    Storage,
    Script,
    FileOps,
    Search,
    Office,
    /// P48.3 — desktop computer-use (E9) as a loop tool. Reached through the
    /// same ticketed executor as every other surface; an unattached backend
    /// fails honestly ("desktop session not attached").
    Desktop,
    /// P48.3 — external MCP tools attached via `attach_external` (user-supplied
    /// stdio/HTTP server). Dispatched through the executor like registry tools.
    External,
    /// P48.3 — connector writes (email/calendar). These ride the
    /// `ConnectorToolBackend` engine seam with approval + audit.
    Connector,
    /// P64.9 — shared-plane task façades (`office.*` / `browser.*` /
    /// `computer_use.*` / `workspace.*` / `artifact.*` / `work.*`). Thin
    /// aliases over the same Rust methods — same Guard-2 ticket, same Merkle
    /// audit row, never a parallel path.
    Facade,
}

/// P48.3 — the desktop computer-use engine seam behind the `desktop.*` tools.
/// A host that has a live `DesktopEngine` (agentcowork-computeruse) injects this
/// so the agent path reaches native windows through the ticketed executor. When
/// absent the tools fail honestly ("desktop session not attached").
pub trait DesktopBackend: Send + Sync {
    /// List native windows (read-only; e-stop-guarded, not a mutation).
    fn list_windows(&self) -> Result<Value, String>;
    /// A11y/OCR read of a window → serialized snapshot.
    fn read(&self, window_id: u64) -> Result<Value, String> {
        let _ = window_id;
        Err("desktop session not attached".into())
    }
    /// Act against a stable window id / a11y ref (observe → act → re-observe).
    fn act(
        &self,
        kind: &str,
        window_id: Option<u64>,
        target: Option<&str>,
        text: Option<&str>,
    ) -> Result<Value, String>;
}

/// P48.3 — the connector-write engine seam behind the `connector.*` tools
/// (email/calendar). Backed in the host by a connector adapter (P42 crates);
/// absent → honest "connector not attached" failure. Writes are gated by the
/// normal ticket + audit path. (P71.3c: the automation layer compiles Work
/// and instantiates the capability request — it never executes the write.)
pub trait ConnectorToolBackend: Send + Sync {
    fn email(&self, to: Vec<String>, subject: &str, body: &str) -> Result<Value, String>;
    fn calendar(&self, title: &str, when: &str) -> Result<Value, String>;
}

/// P48.3 — an attached external MCP server's live tool dispatcher. Backed in
/// the host by the attach machinery (`agentcowork-mcp::AttachedServer`). When
/// absent, external tools fail honestly ("external tool session not attached").
pub trait ExternalToolBackend: Send + Sync {
    fn call(&self, tool_id: &str, args: &Value) -> Result<Value, String>;
}

/// P71.1 — the delegation seam behind the `delegate.*` façades.
///
/// Delegation is a **platform feature**, not a built-in engine's private
/// ability: the primary agent chooses, AgentCowork validates. The host wires
/// this to the kernel's `subagent/spawn` handler and the Work Gateway, so a
/// delegated task is a **child Work** with a bound agent (I8) — the same path
/// the coordinator's own delegation uses. Absent ⇒ `delegate.*` fails
/// honestly ("delegation seam not attached"), never a faked spawn.
pub trait DelegationToolBackend: Send + Sync {
    /// Spawn one child Work from a `SubAgentSpec`-shaped request. The caller
    /// names the delegating Work (`workId`); admission limits, parent link,
    /// Run and ephemeral AgentSession are minted by the kernel.
    fn spawn(&self, params: &Value) -> Result<Value, String>;
    /// Read one delegated child's state (by `workId` + `taskId`, or by
    /// `childWorkId`). Returns the child's Work address + presence.
    fn status(&self, params: &Value) -> Result<Value, String>;
    /// Close one delegated child as cancelled (never as a fabricated success).
    fn cancel(&self, params: &Value) -> Result<Value, String>;
}

/// P71.3f — the one readiness source behind `agent/readiness`.
///
/// The shell owns the runtime facts (install records, PATH/App-Paths
/// discovery, live ACP handshakes, the agent's own auth state), so it mounts
/// this source; the kernel reads it for the delegation gate, the trigger
/// plane's doctor and the picker's façade. An **unmounted** source is never
/// treated as ready: readers get [`agentcowork_types::AgentReadiness::Unknown`],
/// which fails every gate that requires `Ready`.
///
/// Implementations return the fact they hold. A state that cannot be
/// determined is `Unknown` — never a guessed `Ready`.
pub trait AgentReadinessSource: Send + Sync {
    fn readiness(&self, agent_id: &str) -> agentcowork_types::AgentReadiness;
}

/// The mounted readiness source (P71.3f). `None` ⇒ nothing probed, so every
/// read is `Unknown` rather than a fabricated state.
pub type SharedAgentReadiness = Arc<Mutex<Option<Arc<dyn AgentReadinessSource>>>>;

/// Read one agent's readiness from a shared source (the one accessor both the
/// relay RPC and the delegation seam use).
pub fn read_agent_readiness(
    source: &SharedAgentReadiness,
    agent_id: &str,
) -> agentcowork_types::AgentReadiness {
    let mounted = source.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match mounted {
        Some(src) => src.readiness(agent_id),
        None => agentcowork_types::AgentReadiness::Unknown,
    }
}

/// P68.9 — the shell-execution seam behind the `script.run` tool.
///
/// The agent's command execution must land in the **one PTY plane** — the same
/// host human tabs use, on the automation profile, carrying
/// [`TerminalOrigin::Agent`](crate::terminal::TerminalOrigin) provenance — so the
/// run is audited (`terminal.agent_run`), appears in the Shell view as a
/// labelled read-only tab, and the user can watch the agent work instead of
/// trusting an invisible pipe. The host installs this at boot
/// (`src-tauri`); when it is absent `script.run` fails **honestly** rather than
/// silently substituting a different executor.
///
/// Implementations must never create a `Human`-origin session: neither the
/// renderer nor the model may launder authority by asking the agent path for a
/// human terminal.
pub trait TerminalExecutor: Send + Sync {
    fn run(
        &self,
        command: &str,
        label: &str,
        origin: crate::terminal::TerminalOrigin,
    ) -> Result<TerminalRun, String>;
}

/// One agent/task shell command's outcome, normalized for the model.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRun {
    /// The PTY the command ran in — the Shell-view tab the user watched.
    pub pty_id: String,
    pub profile_id: String,
    /// The command line the shell reported.
    pub command: String,
    pub cwd: String,
    /// Shell-reported exit code. `None` means the shell never reported
    /// completion (integration off, no integration script for this shell, or
    /// the wait timed out). That is an **absence of evidence**, never an
    /// implied success.
    pub exit_code: Option<i32>,
    pub output: String,
    /// True only when the record came from nonce-attributed shell reporting.
    pub trusted: bool,
}

/// One catalog entry — the single source of truth for `tool/list`, guard
/// pre-flight, and the coordinator's function-calling defs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredTool {
    pub id: String,
    pub family: ToolFamily,
    pub description: String,
    pub read_only: bool,
    pub operation: String,
    pub risk: String,
    pub risk_tier: String,
    pub args_schema: Value,
}

/// Canonical JSON (sorted object keys) hashed with SHA-256. Coordinator
/// `canonicalArgsHash` must produce the same hex.
///
/// Numbers are canonicalized to a runtime-independent token
/// (`n:<f64-bits-hex>`) so Rust (`serde_json`) and TS (`JSON.stringify`)
/// agree regardless of integer-vs-float formatting (`5` vs `5.0`), exponent
/// style (`1e+21` vs `1e21`), or precision beyond 2^53. JavaScript has a
/// single IEEE-754 `number` type, so hashing by the f64 bit pattern is the
/// one representation both runtimes can produce identically.
pub fn canonical_args_hash(args: &Value) -> String {
    let canon = canonicalize(args);
    let bytes = serde_json::to_vec(&canon).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Canonicalize a JSON number to a runtime-independent string token.
/// `NaN`/±∞ are not representable in JSON (serde emits `null`); we mirror
/// that by tokenizing them to a stable sentinel so both sides still agree.
fn canonical_number_token(n: &serde_json::Number) -> String {
    let f = n.as_f64().unwrap_or(f64::NAN);
    // Normalize -0.0 to 0.0 (JS `Object.is(-0, 0)` is false but JSON/`===`
    // treat them equal; both runtimes hash them the same via +0.0).
    let f = if f == 0.0 { 0.0 } else { f };
    format!("n:{:016x}", f.to_bits())
}

fn canonicalize(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                out.insert(k.clone(), canonicalize(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        // Replace numbers with a bit-pattern token string so cross-runtime
        // serialization can never diverge on number formatting.
        Value::Number(n) => Value::String(canonical_number_token(n)),
        other => other.clone(),
    }
}

/// The live catalog (42 MCP tools + extras).
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
    /// alias → primary id
    aliases: BTreeMap<String, String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut tools = Vec::new();
        let mut aliases = BTreeMap::new();

        for def in all_tools() {
            let family = if agentcowork_mcp::find_storage_tool(def.name).is_some() {
                ToolFamily::Storage
            } else {
                ToolFamily::Browser
            };
            let id = def.name.to_string();
            let prefix = match family {
                ToolFamily::Storage => "storage",
                _ => "browser",
            };
            aliases.insert(format!("{prefix}.{id}"), id.clone());
            tools.push(from_mcp(def, family));
        }

        for extra in extra_tools() {
            tools.push(extra);
        }
        // P64.5 + P64.9 — the unified edit tool and the task façades ride the
        // same registry (same Guard-2 + audit path as every native tool).
        for facade in facade_tools() {
            tools.push(facade);
        }
        aliases.insert("script.run".into(), "script.run".into());
        aliases.insert("search.query".into(), "search.query".into());
        for op in ["read", "write", "delete", "list", "edit"] {
            aliases.insert(format!("file_ops.{op}"), format!("file_ops.{op}"));
        }
        for id in [
            "office.docx_open",
            "office.docx_patch",
            "office.xlsx_open",
            "office.xlsx_edit",
            "office.pptx_open",
            "office.pptx_patch",
            "office.pdf_open",
            "office.pdf_form_fill",
            "office.pdf_redact",
            "office.pdf_pages",
            "desktop.windows",
            "desktop.read",
            "desktop.act",
            "connector.email_send",
            "connector.calendar_create",
        ] {
            aliases.insert(id.into(), id.into());
        }
        // P64.9 — façade ids are first-class catalog ids (flat dot hierarchy).
        for r in FACADE_ROUTES {
            aliases.insert(r.facade.into(), r.facade.into());
        }

        Self { tools, aliases }
    }

    /// P48.3 — reconcile an attached external MCP server's tools into the
    /// catalog as `External`-family entries (server label as provenance).
    /// Already-registered ids (native precedence) are skipped. `label` is the
    /// server provenance (e.g. `mcp:gmail`) recorded on each entry.
    pub fn register_external(&mut self, label: &str, tools: &[ExternalTool]) -> Vec<String> {
        let mut names = Vec::new();
        for t in tools {
            if self.get(&t.name).is_some() {
                continue; // native precedence — never shadow a built-in
            }
            // P51.29 OpenWorker: third-party MCP is EXTERNAL always.
            // A read-named MCP tool is a stranger's claim — never WRITE_LOCAL.
            let _ = agentcowork_guard::floors::mcp_floor_risk();
            let (operation, risk) = ("external_network", "high");
            let mut entry = RegisteredTool {
                id: t.name.clone(),
                family: ToolFamily::External,
                description: format!("{} ({label})", t.description),
                read_only: t.read_only,
                operation: operation.to_string(),
                risk: risk.to_string(),
                risk_tier: String::new(),
                args_schema: t.input_schema.clone(),
            };
            entry = stamp_tier(entry);
            self.tools.push(entry);
            self.aliases.insert(t.name.clone(), t.name.clone());
            names.push(t.name.clone());
        }
        names
    }

    pub fn list(&self) -> &[RegisteredTool] {
        &self.tools
    }

    pub fn get(&self, id: &str) -> Option<&RegisteredTool> {
        let primary = self.aliases.get(id).map(|s| s.as_str()).unwrap_or(id);
        self.tools.iter().find(|t| t.id == primary)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn from_mcp(def: &ToolDef, family: ToolFamily) -> RegisteredTool {
    let (operation, risk) = classify(def);
    stamp_tier(RegisteredTool {
        id: def.name.to_string(),
        family,
        description: def.description.to_string(),
        read_only: def.read_only,
        operation: operation.to_string(),
        risk: risk.to_string(),
        risk_tier: String::new(),
        args_schema: schema_of(def.args),
    })
}

fn stamp_tier(mut t: RegisteredTool) -> RegisteredTool {
    let rl = match t.risk.as_str() {
        "critical" => RiskLevel::Critical,
        "high" => RiskLevel::High,
        "medium" => RiskLevel::Medium,
        _ => RiskLevel::Low,
    };
    t.risk_tier = RiskTier::from_risk_and_op(rl, &t.operation, t.read_only)
        .as_str()
        .to_string();
    t
}

fn extra_tools() -> Vec<RegisteredTool> {
    vec![
        // P54.5 — the id is historical; its meaning is not. `dispatch_script`
        // runs `code` on the **one PTY plane** (automation profile, Agent
        // provenance), so the description and the arg description must say
        // shell, not JavaScript: this string is what the model reads before it
        // decides what to send, and a model told "JavaScript source" sends
        // `const x = 1` to a shell. The rquickjs sandbox is still the engine
        // behind `forge.run_js` and the automation runtime's `run_code` steps.
        RegisteredTool {
            id: "script.run".into(),
            family: ToolFamily::Script,
            description:
                "Run a shell command line in the terminal plane and return its output and exit code"
                    .into(),
            read_only: false,
            operation: "terminal_shell".into(),
            risk: "high".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Shell command line to run (e.g. `cargo test -p agentcowork-core`)"
                    }
                },
                "required": ["code"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            id: "file_ops.read".into(),
            family: ToolFamily::FileOps,
            description: "Read a UTF-8 file inside the workspace floor".into(),
            read_only: true,
            operation: "write".into(),
            risk: "low".into(),
            risk_tier: String::new(),
            args_schema: path_schema("File path to read", false),
        },
        RegisteredTool {
            id: "file_ops.list".into(),
            family: ToolFamily::FileOps,
            description: "List a directory inside the workspace floor".into(),
            read_only: true,
            operation: "write".into(),
            risk: "low".into(),
            risk_tier: String::new(),
            args_schema: path_schema("Directory path", false),
        },
        RegisteredTool {
            id: "file_ops.write".into(),
            family: ToolFamily::FileOps,
            description: "Write a UTF-8 file inside the workspace floor (atomic rename)".into(),
            read_only: false,
            operation: "write".into(),
            risk: "medium".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            id: "file_ops.delete".into(),
            family: ToolFamily::FileOps,
            description: "Delete a file inside the workspace floor".into(),
            read_only: false,
            operation: "delete".into(),
            risk: "high".into(),
            risk_tier: String::new(),
            args_schema: path_schema("File path to delete", false),
        },
        // P64.5 — unified edit ladder (exact → structured → fuzzy, fail closed).
        // Guard-2 ticketed + audit-receipted like every other mutation.
        RegisteredTool {
            id: "file_ops.edit".into(),
            family: ToolFamily::FileOps,
            description: "Surgically replace one exact occurrence (exact → structured → fuzzy ladder; refuses on 0 or 2+ matches)".into(),
            read_only: false,
            operation: "write".into(),
            risk: "medium".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string", "description": "Exact text to replace (must occur once)" },
                    "new": { "type": "string", "description": "Replacement text" }
                },
                "required": ["path", "old", "new"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            id: "search.query".into(),
            family: ToolFamily::Search,
            description: "Web search via the G8 cascade (cache → SearXNG → DDG fallback)".into(),
            read_only: true,
            operation: "external_network".into(),
            risk: "medium".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        office_tool(
            "office.docx_open",
            "Open a .docx and return plain text + block addresses",
            true,
            "low",
            path_schema("Path to a .docx", false),
        ),
        office_tool(
            "office.docx_patch",
            "Surgically patch one docx block (byte-preserving w:t write)",
            false,
            "medium",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "address": { "type": "string", "description": "Block address (e.g. p1)" },
                    "text": { "type": "string" }
                },
                "required": ["path", "address", "text"],
                "additionalProperties": false
            }),
        ),
        office_tool(
            "office.xlsx_open",
            "Open a .xlsx (windowed calamine read of the first sheet)",
            true,
            "low",
            path_schema("Path to a .xlsx", false),
        ),
        office_tool(
            "office.xlsx_edit",
            "Set one spreadsheet cell through IronCalc + surgical part-patch",
            false,
            "medium",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "sheet": { "type": "string" },
                    "address": { "type": "string" },
                    "value": { "type": "string" }
                },
                "required": ["path", "address", "value"],
                "additionalProperties": false
            }),
        ),
        office_tool(
            "office.pptx_open",
            "Open a .pptx and return the deck outline + per-slide text",
            true,
            "low",
            path_schema("Path to a .pptx", false),
        ),
        office_tool(
            "office.pptx_patch",
            "Patch shape text on one slide (byte-preserving a:t write)",
            false,
            "medium",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "part": { "type": "string" },
                    "shape": { "type": "number" },
                    "text": { "type": "string" }
                },
                "required": ["path", "text"],
                "additionalProperties": false
            }),
        ),
        office_tool(
            "office.pdf_open",
            "Inspect a PDF (page count + extracted text)",
            true,
            "low",
            path_schema("Path to a .pdf", false),
        ),
        office_tool(
            "office.pdf_form_fill",
            "Fill AcroForm fields on a PDF",
            false,
            "medium",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "fields": { "type": "object" }
                },
                "required": ["path", "fields"],
                "additionalProperties": false
            }),
        ),
        office_tool(
            "office.pdf_redact",
            "Mark a PDF rectangle for redaction",
            false,
            "high",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "page": { "type": "number" },
                    "x1": { "type": "number" },
                    "y1": { "type": "number" },
                    "x2": { "type": "number" },
                    "y2": { "type": "number" }
                },
                "required": ["path", "page"],
                "additionalProperties": false
            }),
        ),
        office_tool(
            "office.pdf_pages",
            "PDF page ops: split / merge / rotate / reorder / delete / extract",
            false,
            "medium",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "op": { "type": "string" },
                    "pages": { "type": "array", "items": { "type": "number" } },
                    "delta": { "type": "number" },
                    "other": { "type": "string" },
                    "out": { "type": "string" }
                },
                "required": ["path", "op"],
                "additionalProperties": false
            }),
        ),
        // P48.3 — desktop computer-use as a loop tool (E9 agent path).
        RegisteredTool {
            id: "desktop.windows".into(),
            family: ToolFamily::Desktop,
            description: "List native desktop windows (apps + titles + bounds)".into(),
            read_only: true,
            operation: "write".into(),
            risk: "low".into(),
            risk_tier: String::new(),
            args_schema: json!({ "type": "object", "properties": {}, "required": [] }),
        },
        RegisteredTool {
            id: "desktop.read".into(),
            family: ToolFamily::Desktop,
            description: "Read a window's a11y/OCR tree by stable window id".into(),
            read_only: true,
            operation: "write".into(),
            risk: "low".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "windowId": { "type": "number" }
                },
                "required": ["windowId"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            id: "desktop.act".into(),
            family: ToolFamily::Desktop,
            description: "Act on a native window by stable ref (click/type/scroll/launch)".into(),
            read_only: false,
            operation: "web_action".into(),
            risk: "high".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "windowId": { "type": "number" },
                    "target": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["kind"],
                "additionalProperties": false
            }),
        },
        // P48.3 — connector writes (email/calendar) via the automation engine.
        RegisteredTool {
            id: "connector.email_send".into(),
            family: ToolFamily::Connector,
            description: "Send an email through the connected mail provider".into(),
            read_only: false,
            operation: "web_action".into(),
            risk: "high".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["to", "subject"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            id: "connector.calendar_create".into(),
            family: ToolFamily::Connector,
            description: "Create a calendar event on the connected calendar provider".into(),
            read_only: false,
            operation: "web_action".into(),
            risk: "high".into(),
            risk_tier: String::new(),
            args_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "when": { "type": "string" }
                },
                "required": ["title", "when"],
                "additionalProperties": false
            }),
        },
    ]
    .into_iter()
    .map(stamp_tier)
    .collect()
}

fn path_schema(desc: &str, extra: bool) -> Value {
    json!({
        "type": "object",
        "properties": { "path": { "type": "string", "description": desc } },
        "required": ["path"],
        "additionalProperties": extra
    })
}

fn office_tool(id: &str, desc: &str, read_only: bool, risk: &str, schema: Value) -> RegisteredTool {
    RegisteredTool {
        id: id.into(),
        family: ToolFamily::Office,
        description: desc.into(),
        read_only,
        operation: "write".into(),
        risk: risk.into(),
        risk_tier: String::new(),
        args_schema: schema,
    }
}

fn schema_of(args: &[ArgDef]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for a in args {
        let ty = match a.kind {
            ArgKind::String => "string",
            ArgKind::Number => "number",
            ArgKind::Bool => "boolean",
            ArgKind::StringArray => "array",
            ArgKind::Object => "object",
        };
        let mut spec = json!({ "type": ty, "description": a.description });
        if a.kind == ArgKind::StringArray {
            spec["items"] = json!({ "type": "string" });
        }
        properties.insert(a.name.to_string(), spec);
        if a.required {
            required.push(Value::String(a.name.to_string()));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": true
    })
}

fn classify(def: &ToolDef) -> (&'static str, &'static str) {
    if def.kind == ToolKind::Delete {
        return ("delete", "high");
    }
    if def.kind == ToolKind::Execute || def.name == "run" || def.name == "evaluate" {
        return ("terminal_shell", "high");
    }
    if matches!(def.name, "act" | "upload" | "download" | "download_file") {
        return ("web_action", "high");
    }
    if def.open_world || def.kind == ToolKind::Fetch {
        return ("external_network", "medium");
    }
    if def.read_only {
        return ("write", "low");
    }
    ("write", "medium")
}

fn risk_of(s: &str) -> RiskLevel {
    match s {
        "critical" => RiskLevel::Critical,
        "high" => RiskLevel::High,
        "medium" => RiskLevel::Medium,
        _ => RiskLevel::Low,
    }
}

fn operation_of(name: &str, args: &Value) -> Result<Operation, String> {
    Ok(match name {
        "delete" => Operation::DeleteFiles,
        "multi_file_edit" => Operation::MultiFileEdit {
            files: args.get("files").and_then(Value::as_u64).unwrap_or(1) as usize,
        },
        "external_network" => Operation::ExternalNetwork {
            new_domain: args
                .get("newDomain")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        },
        "terminal_shell" => Operation::TerminalShell {
            destructive: args
                .get("destructive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "web_action" => Operation::WebAction,
        "write" => Operation::GenericWrite,
        other => return Err(format!("unknown operation: {other}")),
    })
}

/// The executor: one registry, one guard, one audit chain, one workspace floor.
pub struct ToolService {
    registry: ToolRegistry,
    guard: Arc<Mutex<GuardService>>,
    audit: MerkleChain,
    workspace: PathBuf,
    /// Live parent-dir fds keyed by ticket id (S0.6 TOCTOU).
    parent_fds: BTreeMap<String, fs::File>,
    /// H3: consumed idempotency keys for non-idempotent effects.
    used_idempotency: HashSet<String>,
    /// H3 data-release engine (shared with ChatRelay when wired).
    egress: Arc<Mutex<EgressEngine>>,
    /// File-level undo (Codex /rewind, Claude checkpoint): last write/delete.
    undo: Vec<FileUndo>,
    /// P2.3 — optional browser engine (PDF/screenshot file-op tools).
    browser: Option<Arc<dyn BrowserBackend>>,
    /// P48.3 — optional desktop computer-use engine (`desktop.*` tools, E9).
    desktop: Option<Arc<dyn DesktopBackend>>,
    /// P48.3 — optional connector-write engine (`connector.*` tools).
    connector: Option<Arc<dyn ConnectorToolBackend>>,
    /// P68.9 — the one PTY plane, as the `script.run` executor. Absent until
    /// the host attaches it at boot; absent ⇒ `script.run` fails honestly.
    terminal: Option<Arc<dyn TerminalExecutor>>,
    /// P49.7 — optional opaque capability broker for connector authorization.
    capabilities: Option<Arc<Mutex<agentcowork_guard::LocalCapabilityBroker>>>,
    /// P48.3 — attached external MCP servers (user-supplied tools).
    external: Vec<ExternalAttachment>,
    /// P71.1 — optional delegation seam (`delegate.*` façades). Absent until
    /// the host attaches the Work Gateway bridge; absent ⇒ honest failure.
    delegation: Option<Arc<dyn DelegationToolBackend>>,
    /// G8 cascade (cache → SearXNG → DDG).
    search: agentcowork_search::G8Cascade,
    search_transport: Arc<dyn agentcowork_search::SearchTransport>,
    /// P64.11/P69.G5 — the content-addressed tool-output spool. `None` until
    /// the host attaches one; an over-cap result then passes through whole
    /// (a larger context, never a lost result). Attached by the boot path so
    /// the *decision to compact* is kernel-side, never the renderer's.
    spool: Option<Arc<crate::spool::Spool>>,
    /// FIX-02 / `TASK-TRUST-001` — the tool path's admission control. The
    /// same limiter shape as the shell's IPC gate: bounded, per caller and per
    /// method, failing closed with the canonical taxonomy.
    rate_limiter: Arc<agentcowork_guard::RateLimiter>,
    /// FIX-08 — the per-effect receipts this executor emitted, keyed by
    /// receipt id. Bounded: the oldest index entry is dropped once the cap is
    /// reached (the receipt itself is immutable and was already returned in the
    /// commit response, and the append-only `tool.exec` row remains the durable
    /// record of the same fact).
    receipts: BTreeMap<String, agentcowork_audit::EffectReceipt>,
    /// Insertion order for the bounded index above, so eviction drops the
    /// genuinely oldest receipt rather than whichever id sorts first.
    receipt_order: std::collections::VecDeque<String>,
    /// How many receipts the in-memory index retains.
    receipt_index_cap: usize,
    /// `TASK-CAP-002` — the provider epoch of the in-process native adapter.
    ///
    /// A capability handle is minted against this value; an epoch bump (a
    /// provider restart) invalidates every outstanding handle, and the caller
    /// must re-resolve rather than retry against a runtime that moved
    /// (`ARCH/13` §4 · `ARCH/14` §1 rule 4 · `REQ-CAP-002`).
    provider_epoch: u64,
}

/// P48.3 — one attached external MCP server: its backend dispatcher plus the
/// tool ids it registered into the catalog (each carrying the server label).
pub struct ExternalAttachment {
    pub label: String,
    pub tools: Vec<String>,
    pub backend: Arc<dyn ExternalToolBackend>,
}

#[derive(Debug, Clone)]
pub struct FileUndo {
    pub session_id: String,
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
}

/// `TASK-ART-003` — what one committed result's delivery decided
/// (`ARCH/29-ARTIFACTS.md` §7.1).
///
/// The three states are exhaustive and each is explicit: delivered whole with no
/// artifact, delivered as a bounded preview plus the reference a real artifact
/// write returned, or delivered whole with the *absence* of that artifact
/// stated. `artifact_ref` is populated in the second state only, because it is
/// populated only after the write that produced it succeeded.
struct ResultDelivery {
    /// The value the caller receives.
    value: Value,
    /// The reference the gateway write returned, formatted from its own content
    /// address. `None` for an inlined result and for a write that did not
    /// succeed — an unwritten artifact is never advertised.
    artifact_ref: Option<String>,
    /// Why the artifact is missing, when it is: the bound was crossed and the
    /// write did not happen. Its presence is what makes the receipt a gap.
    artifact_write_failed: Option<String>,
}

impl ResultDelivery {
    /// Under the inline bound: the value goes out whole, nothing was written,
    /// and nothing is claimed about an artifact.
    fn inlined(value: Value) -> Self {
        Self {
            value,
            artifact_ref: None,
            artifact_write_failed: None,
        }
    }

    /// Over the bound, but the write did not succeed. The value is delivered
    /// whole — a larger context, which is recoverable; a silently dropped
    /// payload, which is not — and both the result and the receipt say the
    /// full value is not reachable by reference.
    fn artifact_write_failed(value: Value, total_bytes: usize, reason: &str) -> Self {
        let mut map = match value {
            Value::Object(map) => map,
            other => {
                let mut map = serde_json::Map::new();
                map.insert("value".into(), other);
                map
            }
        };
        map.insert("truncated".into(), json!(false));
        map.insert("totalBytes".into(), json!(total_bytes));
        map.insert(
            "artifactWrite".into(),
            json!({"ok": false, "error": reason}),
        );
        Self {
            value: Value::Object(map),
            artifact_ref: None,
            artifact_write_failed: Some(reason.to_string()),
        }
    }
}

/// Why a committed effect's receipt records a gap rather than a clean success
/// (`ARCH/29-ARTIFACTS.md` §3, the EV1 honesty flag).
///
/// The receipt is the honesty surface, so every way a claim can be incomplete
/// is named here rather than inferred at the point of use. `None` is the only
/// variant that is not a gap.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReceiptGap {
    /// The effect was observed and its result is reachable.
    None,
    /// A mutating effect did not report success, so its state is unknown.
    EffectUncertain,
    /// The dispatch reported failure.
    EffectFailed,
    /// The result was over the inline bound and the artifact that would have
    /// carried it was never written, so the full value is not reachable by
    /// reference (`ARCH/29-ARTIFACTS.md` §7.1, §8).
    ArtifactNotWritten(String),
}

impl ReceiptGap {
    /// Classify one committed effect. Order matters: a failed effect is
    /// reported as a failure whether or not it was mutating, and an unwritten
    /// artifact is only judged once the effect itself reported success.
    fn of(ok: bool, read_only: bool, artifact_write_failed: Option<&str>) -> Self {
        if !ok && !read_only {
            Self::EffectUncertain
        } else if !ok {
            Self::EffectFailed
        } else {
            match artifact_write_failed {
                Some(reason) => Self::ArtifactNotWritten(reason.to_string()),
                None => Self::None,
            }
        }
    }

    /// `None` when there is no gap; otherwise the reason the receipt carries.
    fn uncertainty(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::EffectUncertain => {
                Some("effect state uncertain — the dispatch did not report success".into())
            }
            Self::EffectFailed => Some("effect dispatch reported failure".into()),
            Self::ArtifactNotWritten(reason) => Some(format!(
                "tool result exceeded the inline bound and the artifact was not written ({reason}) — the full value was delivered inline, not by reference"
            )),
        }
    }
}

impl ToolService {
    /// The workspace root every tool path is floored against. Exposed so the
    /// P64.3 repo-map façade maps the *same* tree the edit tools operate on,
    /// rather than inventing a root of its own.
    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    pub fn new(guard: Arc<Mutex<GuardService>>, workspace: PathBuf) -> Self {
        Self::new_with_egress(
            guard,
            workspace,
            Arc::new(Mutex::new(EgressEngine::new(ConnectivityMode::ThirdParty))),
        )
    }

    pub fn new_with_egress(
        guard: Arc<Mutex<GuardService>>,
        workspace: PathBuf,
        egress: Arc<Mutex<EgressEngine>>,
    ) -> Self {
        let _ = fs::create_dir_all(&workspace);
        Self {
            registry: ToolRegistry::new(),
            guard,
            audit: MerkleChain::new(),
            workspace,
            parent_fds: BTreeMap::new(),
            used_idempotency: HashSet::new(),
            egress,
            undo: Vec::new(),
            browser: None,
            desktop: None,
            connector: None,
            terminal: None,
            capabilities: None,
            external: Vec::new(),
            delegation: None,
            // P55.8 — local-first (your own SearXNG, then the DDG fallback),
            // plus any public instances the user explicitly opted into in
            // Settings → Search.
            search: agentcowork_search::G8Cascade::new(
                std::time::Duration::from_secs(300),
                crate::search_config::search_endpoints_from_config(),
                3,
                std::time::Duration::from_secs(60),
            ),
            search_transport: Arc::new(UreqSearchTransport),
            spool: None,
            // `TASK-TRUST-011` — the admission shape comes from the kernel
            // configuration's `controlPlaneRateLimit` entry, so the tool gate
            // and the shell's IPC gate are built from the same resolved value
            // (the shell gate uses `RateLimitConfig::default()`, and this
            // entry's defaults *are* that value).
            rate_limiter: Arc::new(agentcowork_guard::RateLimiter::new(
                crate::config::default_rate_limit_config(),
            )),
            receipts: BTreeMap::new(),
            receipt_order: std::collections::VecDeque::new(),
            receipt_index_cap: RECEIPT_INDEX_CAP,
            provider_epoch: 1,
        }
    }

    /// `TASK-CAP-002` — the live provider epoch. A handle carries the epoch it
    /// was minted at; comparing the two is the stale-handle check.
    pub fn provider_epoch(&self) -> u64 {
        self.provider_epoch
    }

    /// `TASK-CAP-002` — record a provider restart: the epoch bumps and every
    /// outstanding capability handle is stale by definition. There is no
    /// "refresh the handle" path — the caller re-resolves (`ARCH/13` §9 "stale
    /// handle").
    pub fn restart_provider(&mut self) -> u64 {
        self.provider_epoch = self.provider_epoch.saturating_add(1);
        self.provider_epoch
    }

    /// P64.11/P69.G5 — attach the kernel spool. Once attached, every committed
    /// tool result over [`crate::spool::TOOL_OUTPUT_SERIALIZE_CAP`] is written
    /// to the content-addressed spool and replaced in the agent-facing response
    /// by a compact reference. The host attaches this at boot; a `ToolService`
    /// without one still works and simply does not compact.
    pub fn attach_spool(&mut self, spool: Arc<crate::spool::Spool>) {
        self.spool = Some(spool);
    }

    /// `TASK-TRUST-011` — install the admission shape the kernel configuration
    /// resolved.
    ///
    /// The host is the only component that knows the runtime layers (workspace,
    /// agent profile, session, run), so it resolves
    /// [`crate::config::Config::resolve_rate_limit`] and hands the result here.
    /// The type is the same [`agentcowork_guard::RateLimitConfig`] the shell's IPC
    /// gate is built from, so neither gate can be given a shape the other does
    /// not understand.
    pub fn with_rate_limit_config(
        mut self,
        rate_limit: agentcowork_guard::RateLimitConfig,
    ) -> Self {
        self.rate_limiter = Arc::new(agentcowork_guard::RateLimiter::new(rate_limit));
        self
    }

    /// The admission shape in force, for diagnostics and for a caller that must
    /// report what the gate is actually enforcing.
    pub fn rate_limit_config(&self) -> agentcowork_guard::RateLimitConfig {
        *self.rate_limiter.config()
    }

    /// The attached spool, if any. Exposed so the shell's `retrieve_original`
    /// command reads through the same instance the executor writes to.
    pub fn spool(&self) -> Option<&Arc<crate::spool::Spool>> {
        self.spool.as_ref()
    }

    /// P2.3 — attach a browser engine so `save_pdf_enhanced`/
    /// `save_screenshot_enhanced` route real captures to disk.
    pub fn with_browser(mut self, browser: Arc<dyn BrowserBackend>) -> Self {
        self.browser = Some(browser);
        self
    }

    /// Attach (or replace) the live CDP backend after `browser_start`.
    pub fn attach_browser(&mut self, browser: Arc<dyn BrowserBackend>) {
        self.browser = Some(browser);
    }

    /// P48.3 — attach (or replace) the live desktop engine so the `desktop.*`
    /// tools reach native windows through the ticketed executor. This flips the
    /// E9 agent-path cell (desktop becomes a loop tool).
    pub fn attach_desktop(&mut self, desktop: Arc<dyn DesktopBackend>) {
        self.desktop = Some(desktop);
    }

    /// P48.3 — attach (or replace) the connector engine backing `connector.*`
    /// email/calendar writes.
    pub fn attach_connector(&mut self, connector: Arc<dyn ConnectorToolBackend>) {
        self.connector = Some(connector);
    }

    /// P68.9 — attach the one PTY plane as the `script.run` executor. The host
    /// calls this at boot; until it does, `script.run` fails honestly rather
    /// than running the command somewhere the user cannot see.
    pub fn attach_terminal(&mut self, terminal: Arc<dyn TerminalExecutor>) {
        self.terminal = Some(terminal);
    }

    /// Attach the relay-owned capability broker. The broker validates opaque
    /// grants; connector backends still own all credential resolution.
    pub fn attach_capability_broker(
        &mut self,
        capabilities: Arc<Mutex<agentcowork_guard::LocalCapabilityBroker>>,
    ) {
        self.capabilities = Some(capabilities);
    }

    /// P48.3 — attach an external MCP server whose tools were reconciled into
    /// the registry under `label`. The server's tools (already present as
    /// `External`-family catalog entries) dispatch to `backend`.
    pub fn attach_external(
        &mut self,
        label: &str,
        tools: Vec<String>,
        backend: Arc<dyn ExternalToolBackend>,
    ) {
        self.external.retain(|e| e.label != label);
        self.external.push(ExternalAttachment {
            label: label.to_string(),
            tools,
            backend,
        });
    }

    /// P71.1 — attach the delegation seam so the `delegate.*` façades reach
    /// the kernel's child-Work machinery. The host calls this once the Work
    /// Gateway exists; until then delegation fails honestly.
    pub fn attach_delegation(&mut self, delegation: Arc<dyn DelegationToolBackend>) {
        self.delegation = Some(delegation);
    }

    /// P55.11 — the whole attach reconcile in one step: register the
    /// discovered tools as `External`-family entries (native precedence), then
    /// bind their dispatcher so `tool/exec` can actually call them. Returns the
    /// ids that were registered (an id already owned by a native tool is
    /// skipped and deliberately not reported as callable).
    pub fn attach_external_server(
        &mut self,
        label: &str,
        tools: &[ExternalTool],
        backend: Arc<dyn ExternalToolBackend>,
    ) -> Vec<String> {
        let names = self.registry.register_external(label, tools);
        self.attach_external(label, names.clone(), backend);
        // Attaching (or re-attaching) an external provider is that provider
        // starting, so the epoch advances: a handle minted before the attach is
        // stale and its holder must re-resolve. The in-process adapter is
        // unaffected — this is one epoch for the *catalog's* provider view.
        self.restart_provider();
        names
    }

    /// P55.11 — the external tools currently callable through this service.
    /// This is the live registry (the same catalog `tool/list` serves), not a
    /// separate side-table, so what the UI reports is what the agent can call.
    pub fn external_tools(&self) -> Vec<&RegisteredTool> {
        self.registry
            .list()
            .iter()
            .filter(|t| t.family == ToolFamily::External)
            .collect()
    }

    /// Inject a search transport (tests; production uses [`UreqSearchTransport`]).
    pub fn with_search_transport(
        mut self,
        t: Arc<dyn agentcowork_search::SearchTransport>,
    ) -> Self {
        self.search_transport = t;
        self
    }

    /// P55.8 — the SearXNG endpoints the G8 cascade will try, in order.
    pub fn search_endpoints(&self) -> Vec<String> {
        self.search.endpoints().to_vec()
    }

    /// P55.8 — replace the cascade endpoint list. Called with the resolved
    /// config (local-first, plus any public instances the user opted into); the
    /// cascade keeps its TTL/threshold/cooldown and re-routes without a restart.
    pub fn set_search_endpoints(&mut self, endpoints: Vec<String>) {
        self.search.set_endpoints(endpoints);
    }

    fn snapshot_file(&mut self, session_id: &str, path: &Path) {
        let before = fs::read(path).ok();
        self.undo.push(FileUndo {
            session_id: session_id.to_string(),
            path: path.to_path_buf(),
            before,
        });
    }

    /// Restore the last file mutation for this session (or any if empty).
    pub fn revert_last(&mut self, session_id: &str) -> Result<String, String> {
        let idx = self
            .undo
            .iter()
            .rposition(|e| session_id.is_empty() || e.session_id == session_id)
            .ok_or_else(|| "nothing to undo".to_string())?;
        let e = &self.undo[idx];
        let safe_path = self.floor_path(&e.path.to_string_lossy())?;
        crate::file_undo::restore_file_to_bytes(&safe_path, e.before.as_deref())
            .map_err(|err| err.to_string())?;
        let restored = e.path.display().to_string();
        self.undo.remove(idx);
        Ok(restored)
    }

    pub fn set_connectivity(&self, mode: ConnectivityMode) {
        self.egress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_mode(mode);
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// `TASK-CAP-002` — the resolver half of CTR-009 over the live catalog.
    ///
    /// Returns a handle for the head of the ranked chain, or the typed reason
    /// there is none. The caller never learns which provider was chosen unless it
    /// asks for the chain: `include_chain` false returns the handle (or the
    /// refusal) alone, so provider identity stays below the capability layer
    /// (INV-15).
    pub fn resolve_capability(
        &self,
        capability_id: &str,
        constraints: &agentcowork_guard::capability_broker::ResolutionConstraints,
        include_chain: bool,
    ) -> Value {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let resolution = crate::capability_manifest::resolve_capability_at(
            &self.registry,
            capability_id,
            constraints,
            now_ms,
            // The runtime's own epoch, so a handle and the stale-handle check in
            // `tool/exec` can never be derived from two different numbers.
            self.provider_epoch,
        );
        let mut out = json!({
            "capabilityId": capability_id,
            "resolved": resolution.is_resolved(),
        });
        if let Some(handle) = &resolution.handle {
            out["handle"] = serde_json::to_value(handle).unwrap_or(Value::Null);
            out["expiresAtMs"] = json!(handle.expires_at_ms);
        }
        if include_chain {
            out["chain"] = serde_json::to_value(&resolution.chain).unwrap_or_else(|_| json!([]));
            out["excluded"] =
                serde_json::to_value(&resolution.excluded).unwrap_or_else(|_| json!([]));
            out["audit"] = serde_json::to_value(&resolution.audit).unwrap_or_else(|_| json!([]));
        }
        if let Some(unresolved) = &resolution.unresolved {
            // A blocked chain is guidance with a next action, not a dead end.
            let error = unresolved.as_error(capability_id);
            out["status"] = json!(match unresolved {
                agentcowork_guard::capability_broker::Unresolved::MissingRequirement {
                    kind,
                    ..
                } if kind == "connection" || kind == "environment" => "requires_user_action",
                _ => "guidance",
            });
            out["error"] = json!({ "code": error.code(), "message": error.to_string() });
            out["nextAction"] =
                serde_json::to_value(unresolved_next_action(capability_id, unresolved))
                    .unwrap_or(Value::Null);
        }
        out
    }

    pub fn audit_len(&self) -> usize {
        self.audit.len()
    }

    pub fn handle(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        // FIX-02 / `TASK-TRUST-001` — control-plane admission control. The
        // tool surface is a control plane like the shell's IPC: a caller that
        // floods `tool/list` or `tool/exec` would otherwise saturate the Guard
        // policy engine, the ticket store and the Merkle chain. The refusal is
        // the canonical `Unavailable` code with a `retry_after_ms` backoff and
        // it happens before any work, so a throttled call never mints a ticket.
        // Fail-closed: there is no fallback path around this check.
        // The method parameters are supplied by the peer/agent and are not an
        // authenticated identity. In particular, rotating `sessionId` or
        // `agentId` must not mint a fresh per-caller bucket. A ToolService is
        // the host-owned admission boundary, so all calls through this
        // instance share its stable principal; caller/session fields remain
        // available below for routing and provenance only.
        if let Err(denied) = self.rate_limiter.check("tool-service", method) {
            return Err(format!(
                "{} ({}, retry after {}ms)",
                denied,
                denied.scope.as_str(),
                denied.retry_after_ms
            ));
        }
        match method {
            "tool/list" => {
                let plane = params.get("plane").and_then(Value::as_str).unwrap_or("all");
                // `TASK-CAP-003` — the model-facing plane is the **budgeted
                // capability subset**, never the raw catalog. It is opt-in by
                // name because the raw plane is still what the UI browser and
                // the guard pre-flight read (`ARCH/13` §6: catalog mode is known
                // to the UI, not to the model).
                if plane == "capabilities" {
                    return self.capability_subset(params);
                }
                let listed: Vec<&RegisteredTool> = match plane {
                    "shared" => self
                        .registry
                        .list()
                        .iter()
                        .filter(|t| t.family == ToolFamily::Facade)
                        .collect(),
                    _ => self.registry.list().iter().collect(),
                };
                Ok(json!({
                    "tools": listed,
                    "count": listed.len(),
                    "plane": plane,
                }))
            }
            // `TASK-CAP-002` — the capability-plane entry point. Resolution
            // never executes: `invoke` is the *same* `tool/exec` under a
            // capability id, so a resolved capability cannot reach a dispatch
            // path that a raw id could not.
            "capability/resolve" => {
                let capability_id = str_param(params, "capabilityId")
                    .ok_or("capability/resolve requires capabilityId")?;
                let constraints = resolution_constraints(params);
                Ok(self.resolve_capability(capability_id, &constraints, true))
            }
            "tool/exec" => self.exec(params),
            "tool/commit" => self.commit(params),
            // FIX-08 — evidence replay: read back the receipt a committed
            // effect emitted. It reconstructs inputs and states what was
            // authorized; it never re-executes the effect (`ARCH/29` §3).
            "tool/receipt" => {
                let id = str_param(params, "receiptId").ok_or("tool/receipt requires receiptId")?;
                match self.receipts.get(id) {
                    Some(r) => Ok(json!({ "ok": true, "receipt": r })),
                    None => Err(format!("no receipt for {id}")),
                }
            }
            _ => Err(format!("method not found: {method}")),
        }
    }

    /// `TASK-CAP-003` — the budgeted, loading-mode-honouring capability subset
    /// for one turn. The response is bounded by construction, and the raw
    /// catalog is never included, so a caller cannot obtain a flat dump from
    /// this plane.
    fn capability_subset(&self, params: &Value) -> Result<Value, String> {
        let scope = crate::capability_manifest::ActivationScope::new(
            str_param(params, "agentId").unwrap_or("agent"),
            str_param(params, "sessionId").unwrap_or("default"),
            str_param(params, "runId").unwrap_or("run"),
        );
        let budget = match params.get("budget") {
            Some(b) => crate::capability_manifest::ActivationBudget {
                max_bytes: b.get("maxBytes").and_then(Value::as_u64).unwrap_or(
                    crate::capability_manifest::DEFAULT_ACTIVATION_BUDGET.max_bytes as u64,
                ) as usize,
                max_entries: b.get("maxEntries").and_then(Value::as_u64).unwrap_or(
                    crate::capability_manifest::DEFAULT_ACTIVATION_BUDGET.max_entries as u64,
                ) as usize,
            },
            None => crate::capability_manifest::DEFAULT_ACTIVATION_BUDGET,
        };
        let requested: Vec<String> = params
            .get("requested")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let requested_refs: Vec<&str> = requested.iter().map(String::as_str).collect();
        let subset =
            crate::capability_manifest::activate(&self.registry, &scope, budget, &requested_refs);
        if !subset.within_budget() {
            // Unreachable by construction; asserted rather than assumed, because
            // the whole point of the layer is that a dump cannot cross it.
            return Err("capability subset exceeded its budget".to_string());
        }
        serde_json::to_value(&subset).map_err(|e| e.to_string())
    }

    fn exec(&mut self, params: &Value) -> Result<Value, String> {
        let tool_id = str_param(params, "toolId").ok_or("tool/exec requires toolId")?;

        // `TASK-CAP-002` — the stale-handle check, before anything else happens.
        // A caller that resolved a capability and carries its handle proves
        // which provider epoch it believes it holds; if the runtime has moved
        // since, the call is refused and re-resolution is required. A caller
        // with no handle is unaffected — the raw plane never promised one.
        if let Some(minted) = params.get("providerEpoch").and_then(Value::as_u64)
            && minted != self.provider_epoch
        {
            return Ok(json!({
                "action": "re-resolve",
                "reason": format!(
                    "capability handle is stale: minted at provider epoch {minted}, runtime is at {} — re-resolve instead of retrying",
                    self.provider_epoch
                ),
                "providerEpoch": self.provider_epoch,
                "retryable": false,
            }));
        }

        let spec = self
            .registry
            .get(tool_id)
            .ok_or_else(|| format!("unknown tool: {tool_id}"))?
            .clone();
        let args = params.get("args").cloned().unwrap_or(json!({}));
        let session = str_param(params, "sessionId").unwrap_or("default");
        let agent = str_param(params, "agentId").unwrap_or("agent");

        let hash = canonical_args_hash(&args);
        if let Some(client) = str_param(params, "argsHash") {
            if client != hash {
                return Ok(json!({
                    "action": "block",
                    "reason": "args-hash drift between coordinator and Rust",
                }));
            }
        }

        let hits = prescan(&spec, &args);
        if !hits.is_empty() {
            return Ok(json!({
                "action": "block",
                "reason": format!("Guard-1 blocked: {}", hits.join("; ")),
            }));
        }

        // P69.G2 — Guard-1 shell-bias deflection, on the one pre-exec shell
        // path. A command that drives Office, the browser, or the desktop from
        // the shell is a *refusal*, not a warning: it returns before the
        // ticket is minted, so no ticket, caller-supplied `argsHash`, or
        // pre-existing approval can authorize it, and it is never downgraded
        // for a read-only tool or for an external agent's mutating call. The
        // refusal is recorded on the same Merkle chain (same `guard.blocked`
        // kind) as every other Guard-1 denial — not a second audit path — and
        // the response names the shared façade so the model can pivot without
        // a human. Order is deliberate: the destructive-command blocklist above
        // keeps first refusal, so its verdict is unchanged.
        let deflections = deflect_shell(&args);
        if !deflections.is_empty() {
            let reason = Self::deflection_reason(&deflections);
            let seq =
                self.record_deflection_refusal(&spec.id, &hash, &session, &agent, &deflections);
            return Ok(json!({
                "action": "block",
                "refused": true,
                "reason": reason,
                "guard": "deflection",
                "suggestedFacade": deflections[0].facade,
                "suggestedTool": deflections[0].suggested_tool,
                "deflection": {
                    "targets": deflections.iter().map(|d| d.target).collect::<Vec<_>>(),
                    "matched": deflections.iter().map(|d| d.matched.clone()).collect::<Vec<_>>(),
                    "suggestedTools": deflections[0].target.suggested_tools(),
                },
                "auditSeq": seq,
            }));
        }

        let root = self.workspace.to_string_lossy().to_string();
        {
            let mut eg = self.egress.lock().unwrap_or_else(|e| e.into_inner());
            let urls = collect_urls(&args);
            if urls.is_empty() && matches!(spec.family, ToolFamily::Search | ToolFamily::Browser) {
                let plan = eg.plan(
                    &format!("capability:{}", spec.id),
                    "network",
                    None,
                    spec.id.as_str(),
                    &[&root],
                );
                if plan.verdict == EgressVerdict::Deny {
                    return Ok(json!({
                        "action": "block",
                        "reason": format!("egress denied: {} ({})", spec.id, plan.destination),
                        "egress": plan,
                    }));
                }
            }
            for u in urls {
                // P62.1 — these URLs come from the model's own context (page
                // text, search results, tool metadata), i.e. untrusted content.
                // The strict policy therefore applies: loopback and the LAN are
                // refused. A URL the *user* supplied (the download path) keeps
                // the loopback allowance, so local dev servers still work.
                if !urlfloor::check_url_strict(&u, &[&root]).is_allowed() {
                    let reason =
                        urlfloor::block_reason(&u, NetPolicy::strict()).unwrap_or("gr blocked");
                    return Ok(json!({
                        "action": "block",
                        "reason": format!("url floor refused ({reason}): {u}"),
                    }));
                }
                let plan = eg.plan_with_policy(
                    &u,
                    "network",
                    None,
                    spec.id.as_str(),
                    &[&root],
                    NetPolicy::strict(),
                );
                if plan.verdict == EgressVerdict::Deny {
                    return Ok(json!({
                        "action": "block",
                        "reason": format!("egress denied: {u}"),
                        "egress": plan,
                    }));
                }
            }
        }

        let op = operation_of(&spec.operation, &args)?;
        let mut decision = DecisionPackage::new(format!("{} {}", spec.id, hash));
        decision.risk = risk_of(&spec.risk);
        decision.affected_paths = collect_paths(&args);
        // P69.G5 — `retrieve_original` names a content address, not a path. The
        // blob path it resolves to is what the guard must judge, so it is bound
        // here and flows into `capture_bindings` → `reverify_preconditions`
        // like any other read. A hash that does not resolve contributes no
        // path — and the dispatch refuses it — so an unresolvable address can
        // never be authorized into reading something.
        if spec.id == "retrieve_original" {
            let hash_arg = args.get("hash").and_then(Value::as_str).unwrap_or("");
            if let Some(spool) = self.spool.as_ref()
                && let Ok(path) = spool.resolve(hash_arg)
            {
                decision
                    .affected_paths
                    .push(path.to_string_lossy().to_string());
            }
        }
        decision.script_lines = collect_shell(&args);
        decision.network_destinations = collect_urls(&args);

        let out = if let Some(tid) = str_param(params, "ticketId") {
            // Sidecar already ran `guard/evaluate` (H4 guard.ts). Reuse that
            // ticket; still capture bindings below.
            let _ = op;
            let _ = decision;
            GuardDecision::Allow {
                ticket_id: tid.to_string(),
            }
        } else {
            let mut g = self.guard.lock().unwrap_or_else(|e| e.into_inner());
            let mut decision_out = g.evaluate(session, agent, &spec.id, op, decision, &hash, 0);
            // Reads still mint a ticket (ticket-every-effect) but auto-Allow:
            // default policy asks on GenericWrite, which would card every
            // `file_ops.read`. The executor is Rust, not the sidecar.
            // Reads still mint a ticket (ticket-every-effect) but auto-Allow
            // for *native* read tools. P51.29: third-party MCP (External
            // family) never auto-allows — OpenWorker MCP-EXTERNAL floor.
            if spec.read_only && spec.family != ToolFamily::External {
                if let GuardDecision::Ask { ticket_id } = &decision_out {
                    let id = ticket_id.clone();
                    let _ = g.approve(&id);
                    decision_out = GuardDecision::Allow { ticket_id: id };
                }
            }
            decision_out
        };

        let ticket_id = match &out {
            GuardDecision::Allow { ticket_id } | GuardDecision::Ask { ticket_id } => {
                Some(ticket_id.clone())
            }
            GuardDecision::Block { .. } => None,
        };
        if let Some(tid) = &ticket_id {
            let bindings = self.capture_bindings(&args);
            for b in &bindings {
                if let ResourceBinding::File(f) = b {
                    if let Some(fd) = open_parent_dir(&f.canonical) {
                        self.parent_fds.insert(tid.clone(), fd);
                    }
                }
            }
            let exec_id = str_param(params, "executionId").unwrap_or("");
            let mut g = self.guard.lock().unwrap_or_else(|e| e.into_inner());
            let _ = g.set_ticket_bindings(tid, bindings);
            if !exec_id.is_empty() {
                let _ = g.set_ticket_execution(tid, exec_id);
            }
        }

        Ok(match out {
            GuardDecision::Allow { ticket_id } => json!({
                "action": "allow",
                "ticketId": ticket_id,
                "argsHash": hash,
                "readOnly": spec.read_only,
            }),
            GuardDecision::Ask { ticket_id } => json!({
                "action": "ask",
                "ticketId": ticket_id,
                "argsHash": hash,
                "readOnly": spec.read_only,
            }),
            GuardDecision::Block { reason } => json!({
                "action": "block",
                "reason": reason,
            }),
        })
    }

    /// P69.G2 — the refusal line an agent receives for a deflected shell
    /// command. Every hit is named (never just the first) so a command that
    /// spans two bypass shapes still tells the model what to stop doing, and
    /// each hit carries the concrete tool to pivot onto.
    fn deflection_reason(nudges: &[DeflectionNudge]) -> String {
        let details = nudges
            .iter()
            .map(|n| n.message.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        format!("Guard-1 refused this command (shell-bias deflection). {details}")
    }

    /// P69.G2 — the Merkle payload for a refused deflected shell command. A
    /// refusal is evidence, so it names the tool, the args hash, the
    /// session/agent that proposed it, and every needle that fired together
    /// with the façade to use instead. Appended to the *same* chain and under
    /// the *same* `guard.blocked` kind as every other Guard-1 denial.
    fn deflection_audit_payload(
        tool_id: &str,
        args_hash: &str,
        session_id: &str,
        agent_id: &str,
        nudges: &[DeflectionNudge],
    ) -> Value {
        json!({
            "toolId": tool_id,
            "argsHash": args_hash,
            "sessionId": session_id,
            "agentId": agent_id,
            "guard": "deflection",
            "ok": false,
            "state": "refused",
            "outcome": "refused",
            "deflections": nudges
                .iter()
                .map(|n| json!({
                    "target": n.target,
                    "matched": n.matched,
                    "facade": n.facade,
                    "suggestedTool": n.suggested_tool,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// P69.G2 — append the refusal to the audit chain and return its seq.
    fn record_deflection_refusal(
        &mut self,
        tool_id: &str,
        args_hash: &str,
        session_id: &str,
        agent_id: &str,
        nudges: &[DeflectionNudge],
    ) -> u64 {
        let seq = (self.audit.len() as u64) + 1;
        let payload =
            Self::deflection_audit_payload(tool_id, args_hash, session_id, agent_id, nudges);
        self.audit.push(AuditEvent {
            seq,
            ts_ms: now_ms(),
            kind: DEFLECTION_AUDIT_KIND.to_string(),
            payload,
            trace_id: String::new(),
            span_id: String::new(),
        });
        seq
    }

    /// P64.5 — build the Merkle `tool.exec` audit payload for a committed
    /// call. When the dispatch result names the ladder rung that spliced the
    /// file (`strategy: exact|structured|fuzzy` from `file_ops.edit`), the
    /// rung is copied onto the durable row so the timeline shows *how* the
    /// edit landed; results without a rung audit exactly as before.
    fn tool_exec_audit_payload(
        tool_id: &str,
        args_hash: &str,
        ticket_id: &str,
        result_hash: &str,
        ok: bool,
        duration_ms: u64,
        idempotency_key: &str,
        uncertain: bool,
        result: &Value,
    ) -> Value {
        let mut payload = json!({
            "toolId": tool_id,
            "argsHash": args_hash,
            "ticketId": ticket_id,
            "resultHash": result_hash,
            "ok": ok,
            "durationMs": duration_ms,
            "idempotencyKey": idempotency_key,
            "state": if uncertain { "uncertain" } else if ok { "ok" } else { "failed" },
        });
        if let (Some(map), Some(strategy)) = (
            payload.as_object_mut(),
            result.get("strategy").and_then(Value::as_str),
        ) {
            map.insert("strategy".into(), json!(strategy));
        }
        payload
    }

    fn commit(&mut self, params: &Value) -> Result<Value, String> {
        let started = Instant::now();
        let tool_id = str_param(params, "toolId").ok_or("tool/commit requires toolId")?;
        let ticket_id = str_param(params, "ticketId").ok_or("tool/commit requires ticketId")?;
        let spec = self
            .registry
            .get(tool_id)
            .ok_or_else(|| format!("unknown tool: {tool_id}"))?
            .clone();
        let args = params.get("args").cloned().unwrap_or(json!({}));
        let hash = canonical_args_hash(&args);
        if let Some(client) = str_param(params, "argsHash") {
            if client != hash {
                return Err("args-hash drift".into());
            }
        }

        // Consumption is always server-owned. In particular, the legacy
        // caller-supplied `ticketConsumed` flag is not protocol authority and
        // is deliberately never read. Bind consumption to the canonical tool,
        // operation, and exact args so another ticket cannot authorize this
        // dispatch.
        let operation = operation_of(&spec.operation, &args)?;
        let context_hash =
            agentcowork_guard::ticket::ticket_context_hash(&spec.id, operation.name(), &hash);
        {
            let mut g = self.guard.lock().unwrap_or_else(|e| e.into_inner());
            g.use_ticket(ticket_id, &context_hash)
                .map_err(|e| format!("ticket refused: {e}"))?;
        }

        if matches!(spec.family, ToolFamily::Connector) {
            let grant_id = str_param(params, "capabilityGrantId")
                .ok_or("connector tool requires capabilityGrantId")?;
            let run_id = str_param(params, "runId")
                .or_else(|| str_param(params, "executionId"))
                .ok_or("connector tool requires runId")?;
            let request = agentcowork_guard::CapabilityRequest {
                run_id: run_id.to_string(),
                capability: format!("connector:{}", spec.id),
                operation: spec.operation.clone(),
            };
            let broker = self
                .capabilities
                .as_ref()
                .ok_or("capability broker not attached")?;
            broker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .invoke(grant_id, &request)
                .map_err(|e| format!("capability grant refused: {e}"))?;
        }

        self.reverify_preconditions(ticket_id, &args)?;
        self.parent_fds.remove(ticket_id);

        let idem = format!("{tool_id}:{hash}");
        if !spec.read_only && self.used_idempotency.contains(&idem) {
            return Err("idempotent replay refused — reconcile before retry".into());
        }

        let result = self.dispatch(&spec, &args);
        let duration_ms = started.elapsed().as_millis() as u64;
        let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok && !spec.read_only {
            self.used_idempotency.insert(idem.clone());
        }
        // P64.11/P69.G5 — content-addressed spooling, applied *after* the
        // dispatch and *before* the response leaves the kernel. The audit
        // `resultHash` is taken from the full result first, so the durable
        // trail still attests to what the tool actually returned even though
        // the agent sees a reference. The reference keeps `ok`, so the
        // idempotency and uncertainty rules above are unchanged.
        let result_hash = canonical_args_hash(&result);
        // TASK-ART-003 — the bounded preview, the artifact-gateway write, and
        // the reference it earns, in that order (`ARCH/29-ARTIFACTS.md` §7.1).
        // This is the result path, so it is also the artifact-creation point the
        // protocol crate's own module doc names as the writer.
        let delivery = self.deliver_result(&result);
        let result = delivery.value;
        let uncertain = !ok && !spec.read_only;

        let payload = Self::tool_exec_audit_payload(
            &spec.id,
            &hash,
            ticket_id,
            &result_hash,
            ok,
            duration_ms,
            &idem,
            uncertain,
            &result,
        );
        let seq = (self.audit.len() as u64) + 1;
        let event = AuditEvent {
            seq,
            ts_ms: now_ms(),
            kind: "tool.exec".into(),
            payload,
            trace_id: String::new(),
            span_id: String::new(),
        };
        self.audit.push(event);

        // FIX-08 — the effect's receipt, emitted inside the governed path
        // (`ARCH/29-ARTIFACTS.md` §3, INV-07, REQ-ART-003/012).
        //
        // `ARCH/29` §1 is explicit that the security audit chain is *not* the
        // receipt: receipts are product evidence, audit is the security record.
        // The `tool.exec` row above is the audit view; this is the receipt
        // view of the same fact, and the two share the id so a reader can join
        // them. The receipt is mandatory and it references the ticket that
        // authorized the effect (`REQ-PROD-001`: every receipt cites a ticket).
        //
        // Fail-closed (REQ-ART-012): the receipt is built *before* the response
        // is produced, so a mutating effect can never be reported as completed
        // without one. `has_gap` is the honesty flag — a failed or uncertain
        // mutating effect is recorded as a gap, never as a clean success, and
        // so is an artifact that could not be written (TASK-ART-003).
        let gap = ReceiptGap::of(
            ok,
            spec.read_only,
            delivery.artifact_write_failed.as_deref(),
        );
        let receipt = self.record_effect_receipt(
            seq,
            &spec,
            ticket_id,
            &hash,
            &result_hash,
            &gap,
            &operation,
            delivery.artifact_ref.as_deref(),
        );
        let receipt_value =
            serde_json::to_value(&receipt).map_err(|e| format!("receipt encode: {e}"))?;

        let mut out = result;
        if let Value::Object(map) = &mut out {
            map.insert("durationMs".into(), json!(duration_ms));
            map.insert("auditSeq".into(), json!(seq));
            map.insert("ticketId".into(), json!(ticket_id));
            map.insert("idempotencyKey".into(), json!(idem));
            map.insert("receiptId".into(), json!(receipt.effect_id));
            map.insert("receipt".into(), receipt_value);
            if uncertain {
                map.insert("state".into(), json!("uncertain"));
            }
        }
        Ok(out)
    }

    /// Build and index the per-effect receipt for one committed effect.
    ///
    /// The id is derived from the audit sequence, so the receipt and its
    /// `tool.exec` row are the same fact under two views (`ARCH/29` §3: "three
    /// views of one fact, never duplicated state"). The index is bounded; an
    /// overflow drops the *oldest index entry* only — the receipt itself is
    /// immutable, was returned in its commit response, and remains derivable
    /// from the append-only audit row.
    ///
    /// `artifact_ref` is the reference the artifact write *returned*, never one
    /// this function could have derived: the caller sets it only after the write
    /// succeeded, so a receipt can never cite an artifact that does not exist.
    #[allow(clippy::too_many_arguments)]
    fn record_effect_receipt(
        &mut self,
        seq: u64,
        spec: &RegisteredTool,
        ticket_id: &str,
        args_hash: &str,
        result_hash: &str,
        gap: &ReceiptGap,
        operation: &Operation,
        artifact_ref: Option<&str>,
    ) -> agentcowork_audit::EffectReceipt {
        let receipt_id = format!("rcpt:{seq}");
        let mut receipt = agentcowork_audit::EffectReceipt::new(
            receipt_id.clone(),
            spec.id.clone(),
            ticket_id,
            args_hash,
            operation.name(),
            // The authorized operation is the one the ticket was minted for;
            // the executor binds the ticket to `spec.id` + operation + args
            // hash, so a mismatch here would mean a bypass and must be visible.
            operation.name(),
        )
        .with_refs(args_hash.to_string(), result_hash.to_string());
        if let Some(artifact_ref) = artifact_ref {
            // TASK-ART-003 — the receipt cites the artifact that holds the full
            // result, so evidence replay has somewhere to read the value from.
            receipt = receipt.with_resource(artifact_ref);
        }
        if let Some(reason) = gap.uncertainty() {
            // EV1 honesty: an effect that could not be fully observed — because
            // the dispatch failed or was uncertain, or because the artifact that
            // would have carried its result was never written — is recorded as a
            // gap, with the reason, so a reader never assumes it landed exactly
            // as claimed.
            receipt.has_gap = true;
            receipt.uncertainty = Some(reason);
        }
        while self.receipts.len() >= self.receipt_index_cap {
            match self.receipt_order.pop_front() {
                Some(oldest) => {
                    self.receipts.remove(&oldest);
                }
                None => break,
            }
        }
        self.receipt_order.push_back(receipt_id.clone());
        self.receipts.insert(receipt_id, receipt.clone());
        receipt
    }

    /// TASK-ART-003 / `ARCH/29-ARTIFACTS.md` §7.1 — the delivery step for one
    /// committed result: shape the bounded preview, write the full bytes through
    /// the artifact gateway, and only then attach the reference.
    ///
    /// The order **is** the invariant: a reference is formatted from the content
    /// address the write returned, so a value that could not be written never
    /// advertises one. A reference set before its artifact exists points at a
    /// version nobody can open, which is a worse failure than an oversized
    /// result.
    ///
    /// The honesty rules that come with it:
    ///
    /// * **Under the bound** the value is inlined whole and no artifact is
    ///   written — nothing to reach for, so nothing is written.
    /// * **Over the bound** the delivered value carries `truncated: true`, the
    ///   truthful total size, the bound it was cut to, and the reference. It
    ///   carries *no* inline payload: while a value is truncated
    ///   [`agentcowork_mcp::BoundedPreview::inline`] is `None`, and this path
    ///   honours that rather than handing a partial value over as the result.
    /// * **A write that did not succeed** delivers the value whole (a larger
    ///   context, never a wrong answer) and says so in the result *and* on the
    ///   receipt as a gap — an unwritten artifact is never a clean success.
    fn deliver_result(&self, result: &Value) -> ResultDelivery {
        let preview = agentcowork_mcp::bounded_preview(result, crate::spool::INLINE_BUDGET_BYTES);
        if !preview.truncated {
            return ResultDelivery::inlined(result.clone());
        }
        // The reference the write earns. The preview helper never invents one
        // (it has no store, no workspace and no work-item identity), so this is
        // the only place a reference can come from.
        let total_bytes = preview.total_bytes;
        let preview_bytes = preview.preview_bytes;
        if self.spool.is_none() {
            return ResultDelivery::artifact_write_failed(
                result.clone(),
                total_bytes,
                "no tool-output artifact gateway is attached on this host",
            );
        }
        // 1) write the full bytes through the gateway …
        let (projected, reference) = self.compact_result(result.clone());
        // … and 2) only now, holding the address that write returned, set the
        // reference. A `None` here means the write did not happen: the bound was
        // crossed (the preview says so) and no blob was produced.
        let Some(reference) = reference else {
            return ResultDelivery::artifact_write_failed(
                result.clone(),
                total_bytes,
                "the artifact gateway refused the write",
            );
        };
        let preview = preview.with_artifact_ref(agentcowork_mcp::artifact_ref(&reference.hash));
        let mut compact = match projected {
            Value::Object(map) => map,
            // The gateway's projection is an object; a non-object is carried
            // under its own key rather than dropped.
            other => {
                let mut map = serde_json::Map::new();
                map.insert("value".into(), other);
                map
            }
        };
        // The reference the write earned — the only source of one.
        let written_ref = preview
            .artifact_ref
            .expect("the reference was attached to the preview above");
        compact.insert("truncated".into(), json!(true));
        // The true full size, never the cut length: a consumer must be able to
        // report the real magnitude instead of guessing from the reference.
        compact.insert("totalBytes".into(), json!(total_bytes));
        compact.insert("previewBytes".into(), json!(preview_bytes));
        compact.insert("artifactRef".into(), json!(written_ref));
        ResultDelivery {
            value: Value::Object(compact),
            artifact_ref: Some(written_ref),
            artifact_write_failed: None,
        }
    }

    /// P64.11/P69.G5 — the write itself: hand one result to the content-addressed
    /// spool and take back the compact reference it replaced.
    ///
    /// The returned pair is `(delivered, reference)`. `reference` is `None` both
    /// when the result was under the cap (nothing to write) and when a write was
    /// attempted and did not succeed — the caller distinguishes the two by the
    /// inline bound, which is the same line as the cap
    /// ([`crate::spool::INLINE_BUDGET_BYTES`]).
    fn compact_result(&self, result: Value) -> (Value, Option<crate::spool::SpoolRef>) {
        match self.spool.as_ref() {
            Some(spool) => spool.project_result(&result, now_ms()),
            None => (result, None),
        }
    }

    fn dispatch(&mut self, spec: &RegisteredTool, args: &Value) -> Value {
        match spec.family {
            ToolFamily::FileOps => self.dispatch_file_ops(&spec.id, args),
            ToolFamily::Storage => self.dispatch_storage(&spec.id, args),
            ToolFamily::Script => self.dispatch_script(&spec.id, args),
            ToolFamily::Search => self.dispatch_search(args),
            ToolFamily::Browser => self.dispatch_browser(&spec.id, args),
            ToolFamily::Office => self.dispatch_office(&spec.id, args),
            ToolFamily::Desktop => self.dispatch_desktop(&spec.id, args),
            ToolFamily::External => self.dispatch_external(&spec.id, args),
            ToolFamily::Connector => self.dispatch_connector(&spec.id, args),
            // P64.9 — façades fan out to the SAME methods (one engine, two
            // façades). No parallel path: same Guard-2 ticket, same audit row.
            ToolFamily::Facade => self.dispatch_facade(&spec.id, args),
        }
    }

    /// P64.9 — dispatch a task façade to the same underlying method the
    /// native tool uses. Routing is by façade id + `path` extension / args
    /// shape; unknown shapes fail honestly (never a faked success).
    fn dispatch_facade(&mut self, facade: &str, args: &Value) -> Value {
        let Some(route) = find_facade(facade) else {
            return json!({"ok": false, "error": format!("unknown façade: {facade}")});
        };
        match facade {
            "office.open" | "office.inspect" | "office.verify" => {
                let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                let lower = path.to_ascii_lowercase();
                let native = if lower.ends_with(".docx") {
                    "office.docx_open"
                } else if lower.ends_with(".xlsx") {
                    "office.xlsx_open"
                } else if lower.ends_with(".pptx") {
                    "office.pptx_open"
                } else if lower.ends_with(".pdf") || route.targets.contains(&"office.pdf_pages") {
                    // Both conditions select the same native target, so they are
                    // one branch: a .pdf path, or a façade whose fan-out can
                    // read pages. Splitting them was a no-op `else if`.
                    "office.pdf_open"
                } else {
                    route.targets.first().copied().unwrap_or("office.docx_open")
                };
                self.dispatch_office(native, args)
            }
            "office.edit" | "office.calculate" => {
                let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                let lower = path.to_ascii_lowercase();
                let native = if lower.ends_with(".xlsx") {
                    "office.xlsx_edit"
                } else if lower.ends_with(".pptx") {
                    "office.pptx_patch"
                } else {
                    "office.docx_patch"
                };
                // Façade callers pass `{path, text}` or `{path, address, text}`;
                // the native patch path is the same method.
                self.dispatch_office(native, args)
            }
            "office.render" => self.dispatch_office("office.pdf_form_fill", args),
            "browser.research" => self.dispatch_search(args),
            "browser.extract" => {
                // Read-shaped extract: prefer the honest browser read path.
                let out = self.dispatch_browser("read", args);
                if out.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                    out
                } else {
                    self.dispatch_search(args)
                }
            }
            "browser.operate" => {
                if args.get("url").and_then(Value::as_str).is_some() {
                    self.dispatch_browser("navigate", args)
                } else if args.get("ref").or_else(|| args.get("ref_id")).is_some()
                    || args.get("kind").and_then(Value::as_str).is_some()
                {
                    self.dispatch_browser("act", args)
                } else {
                    self.dispatch_browser("snapshot", args)
                }
            }
            "computer_use.see" => {
                if args.get("windowId").and_then(Value::as_u64).is_some() {
                    self.dispatch_desktop("desktop.read", args)
                } else {
                    self.dispatch_desktop("desktop.windows", args)
                }
            }
            "computer_use.act" => self.dispatch_desktop("desktop.act", args),
            "workspace.map" => {
                if args.get("query").and_then(Value::as_str).is_some() {
                    self.dispatch_storage("filename_search", args)
                } else {
                    self.dispatch_storage("disk_scan", args)
                }
            }
            "artifact.store" | "work.create" => {
                // `{path, content}` workspace write — same method as file_ops.write.
                let mapped = if args.get("content").is_some() {
                    json!({"path": args.get("path").cloned().unwrap_or(Value::Null), "content": args.get("content").cloned().unwrap_or(Value::Null)})
                } else {
                    args.clone()
                };
                self.dispatch_file_ops("file_ops.write", &mapped)
            }
            "artifact.retrieve" | "work.status" => self.dispatch_file_ops("file_ops.read", args),
            // P71.1 — delegation is a kernel seam, not a catalog fan-out.
            "delegate.spawn" | "delegate.status" | "delegate.cancel" => {
                self.dispatch_delegate(facade, args)
            }
            // P69.G5 — the spool drilldown. The content address was resolved
            // and bound to the ticket in `exec` (so the guard the ticket was
            // minted against names the exact blob path this read will touch,
            // TOCTOU re-verification included). Here the address is re-resolved
            // through the same guarded `Spool::retrieve`; a hash that stopped
            // resolving refuses rather than falling back to anything.
            "retrieve_original" => {
                let Some(spool) = self.spool.clone() else {
                    return json!({
                        "ok": false,
                        "error": "tool-output spool not attached on this host",
                    });
                };
                let hash = args.get("hash").and_then(Value::as_str).unwrap_or("");
                let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(0);
                match spool.retrieve(hash, offset, limit) {
                    Ok(slice) => json!({
                        "ok": true,
                        "hash": slice.hash,
                        "offset": slice.offset,
                        "limit": slice.limit,
                        "totalLines": slice.total_lines,
                        "returnedLines": slice.returned_lines,
                        "nextOffset": slice.next_offset,
                        "eof": slice.eof,
                        "byteCapped": slice.byte_capped,
                        "text": slice.text,
                    }),
                    Err(err) => json!({"ok": false, "error": err.to_string()}),
                }
            }
            _ => json!({"ok": false, "error": format!("façade has no route yet: {facade}")}),
        }
    }

    /// P71.1 — delegation façades. Delegation mints child Work through the Work
    /// Gateway; the seam is attached by the host. When absent the façade fails
    /// honestly — it never fabricates a spawn or a status.
    fn dispatch_delegate(&self, facade: &str, args: &Value) -> Value {
        let Some(backend) = self.delegation.as_ref() else {
            return json!({
                "ok": false,
                "error": "delegation seam not attached — no Work Gateway bridge on this host"
            });
        };
        let result = match facade {
            "delegate.spawn" => backend.spawn(args),
            "delegate.status" => backend.status(args),
            "delegate.cancel" => backend.cancel(args),
            _ => {
                return json!({"ok": false, "error": format!("unknown delegation façade: {facade}")});
            }
        };
        match result {
            Ok(mut value) => {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("ok".to_string(), json!(true));
                }
                value
            }
            Err(e) => json!({"ok": false, "error": e}),
        }
    }

    /// P48.3 — desktop computer-use as a loop tool (E9 agent path). Honest
    /// failure when no engine is attached (headless/no-display).
    fn dispatch_desktop(&self, id: &str, args: &Value) -> Value {
        // P59.1/P59.11 — CUA is last. An office path or http(s) URL must not
        // pixel-drive when our engines/CDP own the surface.
        if id == "desktop.act" {
            if let Some(target) = args
                .get("target")
                .or_else(|| args.get("path"))
                .or_else(|| args.get("url"))
                .and_then(Value::as_str)
            {
                match crate::route_work_surface(target) {
                    crate::WorkSurface::Office => {
                        return json!({
                            "ok": false,
                            "error": "use office engines for this file — CUA is last on the ladder",
                            "surface": "office",
                        });
                    }
                    crate::WorkSurface::Browse => {
                        return json!({
                            "ok": false,
                            "error": "use inbuilt Browse CDP for this URL — CUA is last on the ladder",
                            "surface": "browse",
                        });
                    }
                    crate::WorkSurface::Desktop => {}
                }
            }
            let needs_shot = args
                .get("screenshot")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let accepts = args
                .get("modelAcceptsImage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Err(e) = crate::vision_gate(needs_shot, accepts) {
                return json!({"ok": false, "error": e.message, "code": e.code});
            }
        }
        let Some(d) = &self.desktop else {
            return json!({"ok": false, "error": "desktop session not attached"});
        };
        match id {
            "desktop.windows" => match d.list_windows() {
                Ok(v) => json!({"ok": true, "windows": v}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            "desktop.read" => {
                let window_id = args.get("windowId").and_then(Value::as_u64);
                match d.read(window_id.unwrap_or(0)) {
                    Ok(v) => json!({"ok": true, "snapshot": v}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
            "desktop.act" => {
                let kind = args.get("kind").and_then(Value::as_str).unwrap_or("click");
                let window_id = args.get("windowId").and_then(Value::as_u64);
                let target = args
                    .get("target")
                    .or_else(|| args.get("ref"))
                    .and_then(Value::as_str);
                let text = args.get("text").and_then(Value::as_str);
                if crate::screen_text_is_untrusted(
                    args.get("screenText").and_then(Value::as_str).unwrap_or(""),
                ) && args.get("approveFromScreen").and_then(Value::as_bool) == Some(true)
                {
                    return json!({
                        "ok": false,
                        "error": "screen text cannot mint a ticket or override an allow-list",
                        "code": "screen_untrusted",
                    });
                }
                let act = d.act(kind, window_id, target, text);
                let act_ok = act.is_ok();
                let verify_ok = args
                    .get("verifyOk")
                    .and_then(Value::as_bool)
                    .unwrap_or(act_ok);
                let fails = args
                    .get("identicalFailCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let outcome = crate::delegation_step(verify_ok, fails);
                let has_evidence =
                    args.get("verifyPath").is_some() || args.get("evidence").is_some();
                if let Some(root) = args.get("cuaRoot").and_then(Value::as_str) {
                    if let Some(node_id) = args.get("nodeId").and_then(Value::as_str) {
                        if let Ok(mut dag) = crate::load_dag(std::path::Path::new(root)) {
                            if let Some(node) = dag.nodes.iter_mut().find(|n| n.id == node_id) {
                                // P60.6 — when independent evidence is present, do not
                                // stamp Verified from the Worker/banner claim.
                                if !has_evidence {
                                    crate::apply_delegation_act(node, verify_ok);
                                }
                            }
                            let _ = crate::persist_dag(std::path::Path::new(root), &dag);
                        }
                    }
                }
                match outcome {
                    crate::DelegationOutcome::Halt => {
                        return json!({
                            "ok": false,
                            "code": "cua_halt",
                            "error": "two identical verify fails — halt, do not click again",
                            "halt": true,
                        });
                    }
                    crate::DelegationOutcome::Mismatch => {
                        return json!({
                            "ok": false,
                            "code": "cua_mismatch",
                            "error": "postcondition did not hold — observe again, do not spam clicks",
                            "halt": false,
                        });
                    }
                    crate::DelegationOutcome::Verified => {}
                }
                match act {
                    Ok(v) => json!({"ok": true, "result": v, "verified": true}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
            _ => json!({"ok": false, "error": format!("unknown desktop tool: {id}")}),
        }
    }

    /// P48.3 — external MCP tools route to the attached server's backend.
    fn dispatch_external(&self, id: &str, args: &Value) -> Value {
        for e in &self.external {
            if e.tools.iter().any(|t| t == id) {
                return match e.backend.call(id, args) {
                    Ok(v) => json!({"ok": true, "result": v}),
                    Err(err) => json!({"ok": false, "error": err}),
                };
            }
        }
        json!({"ok": false, "error": "external tool session not attached"})
    }

    /// P48.3 — connector writes (email/calendar) through the automation
    /// runtime's engine seam; gated by ticket + audited on the Merkle chain.
    fn dispatch_connector(&self, id: &str, args: &Value) -> Value {
        let Some(c) = &self.connector else {
            return json!({"ok": false, "error": "connector not attached"});
        };
        match id {
            "connector.email_send" => {
                let to: Vec<String> = args
                    .get("to")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                let subject = args.get("subject").and_then(Value::as_str).unwrap_or("");
                let body = args.get("body").and_then(Value::as_str).unwrap_or("");
                match c.email(to, subject, body) {
                    Ok(v) => json!({"ok": true, "result": v}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
            "connector.calendar_create" => {
                let title = args.get("title").and_then(Value::as_str).unwrap_or("");
                let when = args.get("when").and_then(Value::as_str).unwrap_or("");
                match c.calendar(title, when) {
                    Ok(v) => json!({"ok": true, "result": v}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
            _ => json!({"ok": false, "error": format!("unknown connector tool: {id}")}),
        }
    }

    fn dispatch_file_ops(&mut self, id: &str, args: &Value) -> Value {
        let path = match args.get("path").and_then(Value::as_str) {
            Some(p) => p,
            None => return json!({"ok": false, "error": "path required"}),
        };
        let abs = self.floor_path(path);
        let abs = match abs {
            Ok(p) => p,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        match id {
            "file_ops.read" => match fs::read(&abs) {
                Ok(bytes) if bytes.len() > MAX_FILE_BYTES => json!({
                    "ok": true,
                    "truncated": true,
                    "content": String::from_utf8_lossy(&bytes[..MAX_FILE_BYTES]).to_string()
                }),
                Ok(bytes) => json!({
                    "ok": true,
                    "content": String::from_utf8_lossy(&bytes).to_string()
                }),
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            },
            "file_ops.list" => match fs::read_dir(&abs) {
                Ok(rd) => {
                    let names: Vec<String> = rd
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect();
                    json!({"ok": true, "entries": names})
                }
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            },
            "file_ops.write" => {
                let content = args.get("content").and_then(Value::as_str).unwrap_or("");
                if let Some(parent) = abs.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                self.snapshot_file("", &abs);
                let tmp = abs.with_extension("tmp-agentcowork");
                match fs::write(&tmp, content).and_then(|_| fs::rename(&tmp, &abs)) {
                    Ok(()) => json!({"ok": true, "path": abs.display().to_string()}),
                    Err(e) => {
                        let _ = fs::remove_file(&tmp);
                        json!({"ok": false, "error": e.to_string()})
                    }
                }
            }
            "file_ops.delete" => {
                self.snapshot_file("", &abs);
                match fs::remove_file(&abs) {
                    Ok(()) => json!({"ok": true}),
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            // P64.5 — unified edit ladder over the same floored + snapshotted
            // + atomic-write path as `file_ops.write`. Guard-2 ticket +
            // Merkle audit are enforced by `commit` (this tool is mutating,
            // `read_only: false`); the ladder itself fails closed on 0/2+.
            "file_ops.edit" => self.dispatch_edit(&abs, args),
            other => json!({"ok": false, "error": format!("unknown file_ops id: {other}")}),
        }
    }

    /// P64.5/P64.6 — run the edit ladder against a floored absolute path,
    /// with the risk-gated shadow preflight (SPEC I15) before commit.
    ///
    /// The splice's own risk is derived first ([`derive_edit_risk`]: a
    /// declaration-count or bracket-balance delta ⇒ structural; an explicit
    /// caller flag can only raise the gate via `derived || explicit`, never
    /// silence it). When [`crate::execution::decide_shadow_preflight`] fires,
    /// the computed post-state is staged into an isolated shadow tree and the
    /// project's own declared typecheck runs *before* the atomic rename: a
    /// failing verdict refuses the write (`refused: true`) and nothing lands.
    /// An unrunnable preflight (no discoverable check, unstageable candidate)
    /// is no-evidence, never a block — the write proceeds marked
    /// `verified: false`, and no receipt is left for rollback to trust.
    fn dispatch_edit(&mut self, abs: &Path, args: &Value) -> Value {
        let old = match args.get("old").and_then(Value::as_str) {
            Some(o) => o,
            None => return json!({"ok": false, "error": "`old` required"}),
        };
        let new = match args.get("new").and_then(Value::as_str) {
            Some(n) => n,
            None => return json!({"ok": false, "error": "`new` required"}),
        };
        let content = match fs::read_to_string(abs) {
            Ok(c) => c,
            Err(e) => return json!({"ok": false, "error": e.to_string()}),
        };
        if content.len() > P64_MAX_EDIT_BYTES * 8 {
            return json!({"ok": false, "error": format!("file over the {} byte edit cap; use bounded-window reads", P64_MAX_EDIT_BYTES * 8)});
        }
        let shape = LexicalShapeSource;
        let (updated, strategy) = match apply_edit_ladder(&content, old, new, &shape) {
            Ok(ok) => ok,
            Err(e) => return json!({"ok": false, "error": e.to_string(), "refused": true}),
        };
        // P64.6 — the gate sees the computed post-state before anything lands.
        let structural = derive_edit_risk(&content, &updated).structural
            || args
                .get("structural")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let destructive = args
            .get("destructive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let decision = crate::execution::decide_shadow_preflight(1, structural, destructive);
        let preflight = self.preflight_candidate(abs, &updated, &decision);
        if preflight.blocks() {
            return json!({
                "ok": false,
                "refused": true,
                "error": preflight.refusal(),
                "strategy": strategy.as_str(),
                "structural": structural,
                "preflight": preflight.report(),
            });
        }
        self.snapshot_file("", abs);
        let tmp = abs.with_extension("tmp-agentcowork");
        match fs::write(&tmp, &updated).and_then(|_| fs::rename(&tmp, abs)) {
            Ok(()) => json!({
                "ok": true,
                "path": abs.display().to_string(),
                "strategy": strategy.as_str(),
                "structural": structural,
                "preflight": preflight.report(),
            }),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                json!({"ok": false, "error": e.to_string()})
            }
        }
    }

    /// P64.6 — stage one edit candidate into an isolated shadow tree and run
    /// the workspace's declared typecheck there. The live root is never
    /// touched; cleanup always runs and a cleanup failure is reported in the
    /// output, never a verdict upgrade.
    fn preflight_candidate(
        &self,
        abs: &Path,
        updated: &str,
        decision: &crate::execution::PreflightDecision,
    ) -> EditPreflight {
        if !decision.needs_preflight {
            return EditPreflight::Skipped(decision.reason);
        }
        let checks = crate::execution::discover_shadow_checks(&self.workspace);
        if checks.is_empty() {
            return EditPreflight::Unverifiable(format!(
                "{} — but no typecheck command was discovered in {}",
                decision.reason,
                self.workspace.display()
            ));
        }
        let rel = match abs.strip_prefix(&self.workspace) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => {
                return EditPreflight::Unverifiable(format!(
                    "{} — but the edited file is outside the workspace floor",
                    decision.reason
                ));
            }
        };
        if rel.contains("..") {
            return EditPreflight::Unverifiable(format!(
                "{} — but the staged path escapes the shadow tree",
                decision.reason
            ));
        }
        let candidate = vec![crate::execution::ShadowCandidateFile::new(
            rel,
            updated.to_string(),
        )];
        let (shadow_root, cleanup) =
            match crate::execution::stage_shadow_tree(&self.workspace, &candidate) {
                Ok(staged) => staged,
                Err(err) => {
                    return EditPreflight::Unverifiable(format!(
                        "{} — but the shadow tree could not be staged: {err}",
                        decision.reason
                    ));
                }
            };
        let results = crate::execution::run_shadow_checks(&shadow_root, &checks);
        let passed = results.len() == checks.len() && results.iter().all(|r| r.success);
        let mut output = String::new();
        for (check, result) in checks.iter().zip(results.iter()) {
            output.push_str(&format!("$ {}\n{}\n", check.label, result.preview));
        }
        if let Err(err) = cleanup.cleanup() {
            output.push_str(&format!("\n(shadow cleanup: {err})"));
        }
        let (preview, _, _) = crate::execution::truncate_to_50k(&output);
        if passed {
            EditPreflight::Passed(decision.reason)
        } else {
            EditPreflight::Failed(decision.reason, preview)
        }
    }

    fn dispatch_storage(&self, id: &str, args: &Value) -> Value {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        let root = if path.is_empty() {
            self.workspace.clone()
        } else {
            match self.floor_path(path) {
                Ok(p) => p,
                Err(e) => return json!({"ok": false, "error": e}),
            }
        };
        let opts = agentcowork_storage::ScanOptions {
            threads: 1,
            follow_symlinks: false,
            same_filesystem: true,
            min_file_size: 0,
            skip_hidden: true,
        };
        let records = match agentcowork_storage::scan(&root, &opts) {
            Ok(r) => r,
            Err(e) => return json!({"ok": false, "error": e.to_string()}),
        };
        match id {
            "disk_scan" => {
                let sample: Vec<String> = records
                    .iter()
                    .take(MAX_SCAN_SAMPLE)
                    .map(|r| r.path.display().to_string())
                    .collect();
                json!({"ok": true, "files": records.len(), "sample": sample})
            }
            "disk_large_files" => {
                let arena = agentcowork_storage::build_arena(records, &root);
                let top_n = args.get("top_n").and_then(Value::as_u64).unwrap_or(10) as usize;
                let now = now_ms() / 1000;
                let files = agentcowork_storage::find_large_files(
                    &arena,
                    &agentcowork_storage::FinderOptions {
                        top_n,
                        ..Default::default()
                    },
                    agentcowork_storage::SortBy::SizeDesc,
                    now,
                );
                let listed: Vec<Value> = files
                    .into_iter()
                    .map(|n| json!({"name": n.name, "size": n.size}))
                    .collect();
                json!({"ok": true, "files": listed})
            }
            "disk_duplicates" => {
                let cands: Vec<agentcowork_storage::DupCandidate> = records
                    .into_iter()
                    .map(|r| agentcowork_storage::DupCandidate {
                        path: r.path,
                        size: r.size,
                        dev: r.dev,
                        ino: r.ino,
                        nlink: r.nlink,
                    })
                    .collect();
                match agentcowork_storage::find_duplicates(
                    &cands,
                    &agentcowork_storage::DedupOptions::default(),
                ) {
                    Ok(groups) => json!({"ok": true, "groups": groups.len()}),
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            "disk_cleanup" => {
                json!({
                    "ok": true,
                    "proposals": [],
                    "note": "cleanup is proposal-only; never deletes"
                })
            }
            "filename_search" => {
                let q = args.get("query").and_then(Value::as_str).unwrap_or("");
                let hits: Vec<String> = records
                    .iter()
                    .filter(|r| {
                        r.path
                            .file_name()
                            .map(|n| n.to_string_lossy().contains(q))
                            .unwrap_or(false)
                    })
                    .take(MAX_SCAN_SAMPLE)
                    .map(|r| r.path.display().to_string())
                    .collect();
                json!({"ok": true, "hits": hits})
            }
            other => json!({"ok": false, "error": format!("unknown storage id: {other}")}),
        }
    }

    /// P68.9 — the agent's shell.
    ///
    /// `script.run` executes its `code` on the **one PTY plane**: the same host
    /// a human tab uses, on the automation profile, with `TerminalOrigin::Agent`
    /// provenance. The run is audited as `terminal.agent_run` and renders as a
    /// labelled read-only tab, which is the whole point — "watch the agent work"
    /// has to be a property of the product, not a claim.
    ///
    /// The `{ code }` payload is therefore a **shell command line**, not
    /// JavaScript. The rquickjs `agentcowork-script` sandbox is unchanged and
    /// still the engine for `forge.run_js` and the automation runtime's
    /// `run_code` steps — those are internal deterministic workflows, not agent
    /// shell calls.
    ///
    /// No terminal plane attached ⇒ honest failure. A host with no PTY host has
    /// no shell, and quietly evaluating the string as JavaScript under a tool id
    /// that now means *shell* would be exactly the kind of silent substitution
    /// that makes "what did the agent run?" unanswerable.
    fn dispatch_script(&self, id: &str, args: &Value) -> Value {
        let code = match args.get("code").and_then(Value::as_str) {
            Some(c) if !c.trim().is_empty() => c,
            _ => return json!({"ok": false, "error": "code required"}),
        };
        let Some(exec) = &self.terminal else {
            return json!({
                "ok": false,
                "error": "terminal plane not attached — script.run has no shell executor on this host",
            });
        };
        match exec.run(code, id, crate::terminal::TerminalOrigin::Agent) {
            Ok(run) => json!({
                // Only a shell-reported exit 0 is success. An unverified run is
                // not a failure claim — it is reported as absent evidence via
                // `exitCode: null` + `trusted: false`, and the model sees both.
                "ok": run.exit_code == Some(0),
                "ptyId": run.pty_id,
                "profile": run.profile_id,
                "command": run.command,
                "cwd": run.cwd,
                "exitCode": run.exit_code,
                "output": run.output,
                "trusted": run.trusted,
            }),
            Err(e) => json!({"ok": false, "error": e}),
        }
    }

    fn capture_bindings(&self, args: &Value) -> Vec<ResourceBinding> {
        let root = self.workspace.to_string_lossy().to_string();
        let roots: Vec<&str> = vec![&root];
        let mut out = Vec::new();
        for p in collect_paths(args) {
            let joined = if Path::new(&p).is_absolute() {
                p.clone()
            } else {
                self.workspace.join(&p).to_string_lossy().to_string()
            };
            if let Ok(b) = bind_path(&joined, &roots) {
                out.push(ResourceBinding::File(b));
            }
        }
        for u in collect_urls(args) {
            if let Ok(b) = bind_url(&u, &roots) {
                out.push(ResourceBinding::Net(b));
            }
        }
        for s in collect_shell(args) {
            out.push(ResourceBinding::Exec(bind_exec_bytes(s.as_bytes())));
        }
        out
    }

    fn reverify_preconditions(&self, ticket_id: &str, args: &Value) -> Result<(), String> {
        let root = self.workspace.to_string_lossy().to_string();
        let roots: Vec<&str> = vec![&root];
        let bindings = {
            let g = self.guard.lock().unwrap_or_else(|e| e.into_inner());
            g.ticket_bindings(ticket_id)
        };
        for b in &bindings {
            match b {
                ResourceBinding::File(f) => {
                    reverify_path(f, &roots).map_err(|e| format!("TOCTOU: {e}"))?;
                }
                ResourceBinding::Net(n) => {
                    reverify_url(n, &roots).map_err(|e| format!("TOCTOU: {e}"))?;
                }
                ResourceBinding::Exec(x) => {
                    let bytes = collect_shell(args).join("\n");
                    reverify_exec(x, bytes.as_bytes()).map_err(|e| format!("TOCTOU: {e}"))?;
                }
            }
        }
        Ok(())
    }

    fn floor_path(&self, path: &str) -> Result<PathBuf, String> {
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.workspace.join(path)
        };
        let s = joined.to_string_lossy();
        let root = self.workspace.to_string_lossy();
        match enforce_floor(&s, &[root.as_ref()]) {
            FloorVerdict::Allowed => Ok(joined),
            other => Err(format!("path floor refused: {other:?}")),
        }
    }

    /// P2.3 — floor a *directory* (for PDF/screenshot downloads), creating it
    /// if absent so the browser backend has a writable target.
    fn floor_dir(&self, dir: &str) -> Result<PathBuf, String> {
        let abs = self.floor_path(dir)?;
        if let Err(e) = fs::create_dir_all(&abs) {
            return Err(format!("create download dir failed: {e}"));
        }
        Ok(abs)
    }

    /// P2.3 (E2) — real `download_file`: HTTP GET the already floor/egress-
    /// checked URL, refuse past [`MAX_DOWNLOAD_BYTES`], and write the bytes
    /// atomically inside the workspace floor. No browser needed.
    fn dispatch_download_file(&self, args: &Value) -> Value {
        let url = match args.get("url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() => u,
            _ => return json!({"ok": false, "error": "url required"}),
        };
        // FIX-09: the pre-flight is here, at the socket, not only in
        // `tool/exec`. `dispatch` is also reachable from any in-process caller
        // of `ToolService`, so the destination floor is enforced immediately
        // before the request — one egress path, no reliance on a caller having
        // gone through the executor. The URL is user-supplied (a download the
        // user asked for), so the desktop default policy applies: loopback and
        // public hosts pass, the LAN and link-local/metadata floor does not.
        if let Err(denied) =
            agentcowork_guard::netfloor::preflight_url(url, agentcowork_guard::NetPolicy::default())
        {
            return json!({"ok": false, "error": denied.to_string()});
        }
        let dir = args
            .get("dir")
            .and_then(Value::as_str)
            .unwrap_or("downloads");
        let abs_dir = match self.floor_dir(dir) {
            Ok(p) => p,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        let name = filename_from_url(url);
        let mut bytes = Vec::new();
        match ureq::get(url).call() {
            Ok(resp) => {
                use std::io::Read;
                let reader = resp.into_reader();
                let mut limited = reader.take(MAX_DOWNLOAD_BYTES as u64 + 1);
                if limited.read_to_end(&mut bytes).is_err() {
                    return json!({"ok": false, "error": "read failed"});
                }
                if bytes.len() > MAX_DOWNLOAD_BYTES {
                    return json!({"ok": false, "error": "download exceeds 64 MiB cap"});
                }
            }
            Err(e) => return json!({"ok": false, "error": e.to_string()}),
        }
        let target = abs_dir.join(&name);
        let tmp = target.with_extension("tmp-agentcowork");
        match fs::write(&tmp, &bytes).and_then(|_| fs::rename(&tmp, &target)) {
            Ok(()) => {
                json!({"ok": true, "path": target.display().to_string(), "bytes": bytes.len()})
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                json!({"ok": false, "error": e.to_string()})
            }
        }
    }

    fn dispatch_search(&self, args: &Value) -> Value {
        let query = match args.get("query").and_then(Value::as_str) {
            Some(q) if !q.is_empty() => q,
            _ => return json!({"ok": false, "error": "query required"}),
        };
        match self.search.query(self.search_transport.as_ref(), query) {
            Ok(hits) => json!({
                "ok": true,
                "query": query,
                "count": hits.len(),
                "results": hits,
            }),
            Err(e) => json!({"ok": false, "error": e}),
        }
    }

    fn dispatch_browser(&self, id: &str, args: &Value) -> Value {
        match id {
            "download_file" => self.dispatch_download_file(args),
            "save_pdf_enhanced" | "save_screenshot_enhanced" => {
                let dir = args
                    .get("dir")
                    .and_then(Value::as_str)
                    .unwrap_or("downloads");
                let abs = match self.floor_dir(dir) {
                    Ok(p) => p,
                    Err(e) => return json!({"ok": false, "error": e}),
                };
                match &self.browser {
                    Some(b) => {
                        let res = if id == "save_pdf_enhanced" {
                            b.save_pdf_enhanced(&abs)
                        } else {
                            let q = args.get("quality").and_then(Value::as_u64).unwrap_or(80) as u8;
                            b.save_screenshot_enhanced(&abs, q)
                        };
                        match res {
                            Ok(path) => json!({"ok": true, "path": path}),
                            Err(e) => json!({"ok": false, "error": e}),
                        }
                    }
                    None => json!({"ok": false, "error": "browser session not attached"}),
                }
            }
            "navigate" | "browser.navigate" => {
                let url = match args.get("url").and_then(Value::as_str) {
                    Some(u) => u,
                    None => return json!({"ok": false, "error": "url required"}),
                };
                match &self.browser {
                    Some(b) => match b.navigate(url) {
                        Ok(u) => json!({"ok": true, "url": u}),
                        Err(e) => json!({"ok": false, "error": e}),
                    },
                    None => json!({"ok": false, "error": "browser session not attached"}),
                }
            }
            "snapshot" | "enhanced_snapshot" => match &self.browser {
                Some(b) => match b.snapshot() {
                    Ok(text) => json!({"ok": true, "text": text}),
                    Err(e) => json!({"ok": false, "error": e}),
                },
                None => json!({"ok": false, "error": "browser session not attached"}),
            },
            "act" => {
                let kind = args.get("kind").and_then(Value::as_str).unwrap_or("click");
                let selector = args
                    .get("ref")
                    .or_else(|| args.get("ref_id"))
                    .and_then(Value::as_str);
                let text = args.get("text").and_then(Value::as_str);
                match &self.browser {
                    Some(b) => match b.act(kind, selector, text) {
                        Ok(msg) => json!({"ok": true, "result": msg}),
                        Err(e) => json!({"ok": false, "error": e}),
                    },
                    None => json!({"ok": false, "error": "browser session not attached"}),
                }
            }
            _ => json!({"ok": false, "error": "browser session not attached"}),
        }
    }

    fn dispatch_office(&mut self, id: &str, args: &Value) -> Value {
        let path = match args.get("path").and_then(Value::as_str) {
            Some(p) => p,
            None => return json!({"ok": false, "error": "path required"}),
        };
        let abs = match self.floor_path(path) {
            Ok(p) => p,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        match id {
            "office.docx_open" => match fs::read(&abs) {
                Ok(bytes) => match agentcowork_office::DocxEngine::open(bytes) {
                    Ok(engine) => {
                        let blocks: Vec<Value> = engine
                            .blocks()
                            .iter()
                            .map(|b| {
                                json!({
                                    "address": b.address,
                                    "kind": format!("{:?}", b.kind),
                                    "part": b.part,
                                })
                            })
                            .collect();
                        json!({
                            "ok": true,
                            "path": abs.display().to_string(),
                            "text": engine.render_text(),
                            "blocks": blocks,
                        })
                    }
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                },
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            },
            "office.docx_patch" => {
                let address = args.get("address").and_then(Value::as_str).unwrap_or("");
                let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                self.snapshot_file("", &abs);
                match fs::read(&abs) {
                    Ok(bytes) => match agentcowork_office::DocxEngine::open(bytes) {
                        Ok(mut engine) => match engine
                            .patch_block(address, text)
                            .and_then(|_| engine.save())
                        {
                            Ok(out) => match atomic_office_write(&abs, &out) {
                                Ok(()) => {
                                    json!({"ok": true, "path": abs.display().to_string(), "address": address})
                                }
                                Err(e) => json!({"ok": false, "error": e}),
                            },
                            Err(e) => json!({"ok": false, "error": e.to_string(), "refused": true}),
                        },
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            "office.xlsx_open" => match agentcowork_office::xlsx::read::open(&abs) {
                Ok(meta) => json!({"ok": true, "path": meta.path, "sheets": meta.sheets}),
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            },
            "office.xlsx_edit" => {
                use agentcowork_office::xlsx::address::parse_ref;
                use agentcowork_office::xlsx::dsl::{
                    Operation as XlsxOp, Scalar, WorkbookCommandBatch,
                };
                use agentcowork_office::xlsx::patch::apply_batch;
                let address = args.get("address").and_then(Value::as_str).unwrap_or("");
                let value = args.get("value").and_then(Value::as_str).unwrap_or("");
                let sheet = args.get("sheet").and_then(Value::as_str).unwrap_or("");
                let cell = match parse_ref(address) {
                    Ok((_, c)) => c,
                    Err(e) => return json!({"ok": false, "error": e.to_string()}),
                };
                self.snapshot_file("", &abs);
                match fs::read(&abs) {
                    Ok(bytes) => {
                        let mut batch = WorkbookCommandBatch::new(0, format!("Set {address}"));
                        let scalar = if let Ok(n) = value.parse::<f64>() {
                            Scalar::Number(n)
                        } else {
                            Scalar::Text(value.to_string())
                        };
                        batch.operations.push(XlsxOp::SetCell {
                            address: cell,
                            value: scalar,
                        });
                        let sheet_name = if sheet.is_empty() {
                            agentcowork_office::xlsx::read::open(&abs)
                                .ok()
                                .and_then(|m| m.sheets.first().map(|s| s.name.clone()))
                                .unwrap_or_else(|| "Sheet1".into())
                        } else {
                            sheet.to_string()
                        };
                        match apply_batch(&bytes, &batch, &sheet_name) {
                            Ok(outcome) => match atomic_office_write(&abs, &outcome.bytes) {
                                Ok(()) => {
                                    json!({"ok": true, "path": abs.display().to_string(), "address": address})
                                }
                                Err(e) => json!({"ok": false, "error": e}),
                            },
                            Err(e) => json!({"ok": false, "error": e.to_string()}),
                        }
                    }
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            "office.pptx_open" => match fs::read(&abs) {
                Ok(bytes) => match agentcowork_office::PptxEngine::open(bytes) {
                    Ok(mut engine) => match engine.render_deck() {
                        Ok(deck) => {
                            json!({"ok": true, "path": abs.display().to_string(), "deck": deck})
                        }
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                },
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            },
            "office.pptx_patch" => {
                let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                let part = args.get("part").and_then(Value::as_str);
                let shape = args.get("shape").and_then(Value::as_u64).unwrap_or(0) as usize;
                self.snapshot_file("", &abs);
                match fs::read(&abs) {
                    Ok(bytes) => match agentcowork_office::PptxEngine::open(bytes) {
                        Ok(mut engine) => {
                            let part_name = match part {
                                Some(p) => p.to_string(),
                                None => match engine.slides().first() {
                                    Some(s) => s.part.clone(),
                                    None => return json!({"ok": false, "error": "no slides"}),
                                },
                            };
                            let shape_addr = format!("shape{shape}");
                            match engine.patch_shape_text(&part_name, &shape_addr, text) {
                                Ok(()) => match engine.save() {
                                    Ok(out) => match atomic_office_write(&abs, &out) {
                                        Ok(()) => {
                                            json!({"ok": true, "path": abs.display().to_string()})
                                        }
                                        Err(e) => json!({"ok": false, "error": e}),
                                    },
                                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                                },
                                Err(e) => {
                                    json!({"ok": false, "error": e.to_string(), "refused": true})
                                }
                            }
                        }
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            "office.pdf_open" => match fs::read(&abs) {
                Ok(bytes) => match agentcowork_office::inspect(&bytes) {
                    Ok(info) => json!({"ok": true, "pages": info.pages, "texts": info.texts}),
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                },
                Err(e) => json!({"ok": false, "error": e.to_string()}),
            },
            "office.pdf_form_fill" => {
                let fields_obj = args.get("fields").and_then(Value::as_object);
                let fields: Vec<(String, String)> = fields_obj
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                self.snapshot_file("", &abs);
                match fs::read(&abs) {
                    Ok(bytes) => match agentcowork_office::pdf::form::form_fill(&bytes, &fields) {
                        Ok(out) => match atomic_office_write(&abs, &out) {
                            Ok(()) => json!({"ok": true, "path": abs.display().to_string()}),
                            Err(e) => json!({"ok": false, "error": e}),
                        },
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            "office.pdf_redact" => {
                let page = args.get("page").and_then(Value::as_u64).unwrap_or(1) as u32;
                let x1 = args.get("x1").and_then(Value::as_f64).unwrap_or(0.0);
                let y1 = args.get("y1").and_then(Value::as_f64).unwrap_or(0.0);
                let x2 = args.get("x2").and_then(Value::as_f64).unwrap_or(0.0);
                let y2 = args.get("y2").and_then(Value::as_f64).unwrap_or(0.0);
                self.snapshot_file("", &abs);
                match fs::read(&abs) {
                    Ok(bytes) => match agentcowork_office::pdf::redact::redact(
                        &bytes,
                        &[(page, [x1 as f32, y1 as f32, x2 as f32, y2 as f32])],
                    ) {
                        Ok(out) => match atomic_office_write(&abs, &out) {
                            Ok(()) => json!({"ok": true, "path": abs.display().to_string()}),
                            Err(e) => json!({"ok": false, "error": e}),
                        },
                        Err(e) => json!({"ok": false, "error": e.to_string()}),
                    },
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            "office.pdf_pages" => {
                let op = args.get("op").and_then(Value::as_str).unwrap_or("");
                let pages: Vec<u32> = args
                    .get("pages")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_u64().map(|n| n as u32))
                            .collect()
                    })
                    .unwrap_or_default();
                self.snapshot_file("", &abs);
                match fs::read(&abs) {
                    Ok(bytes) => {
                        let result = match op {
                            "split" if pages.len() >= 2 => {
                                agentcowork_office::split_pdf(&bytes, pages[0]..=pages[1])
                            }
                            "extract" => agentcowork_office::extract_pages(&bytes, &pages),
                            "reorder" => agentcowork_office::reorder_pages(&bytes, &pages),
                            "delete" => agentcowork_office::delete_pages(&bytes, &pages),
                            "rotate" => {
                                let delta = args.get("delta").and_then(Value::as_i64).unwrap_or(90);
                                let sel = if pages.is_empty() {
                                    None
                                } else {
                                    Some(pages.as_slice())
                                };
                                agentcowork_office::rotate_pages(&bytes, delta, sel)
                            }
                            "merge" => {
                                let other = args.get("other").and_then(Value::as_str).unwrap_or("");
                                let other_abs = match self.floor_path(other) {
                                    Ok(p) => p,
                                    Err(e) => return json!({"ok": false, "error": e}),
                                };
                                match fs::read(&other_abs) {
                                    Ok(b2) => agentcowork_office::merge_pdfs(&[bytes.clone(), b2]),
                                    Err(e) => return json!({"ok": false, "error": e.to_string()}),
                                }
                            }
                            _ => {
                                return json!({"ok": false, "error": format!("unknown pdf page op: {op}")});
                            }
                        };
                        match result {
                            Ok(out) => {
                                let dest = args
                                    .get("out")
                                    .and_then(Value::as_str)
                                    .map(PathBuf::from)
                                    .unwrap_or(abs.clone());
                                let dest = if dest.is_absolute() {
                                    dest
                                } else {
                                    match self.floor_path(&dest.to_string_lossy()) {
                                        Ok(p) => p,
                                        Err(e) => return json!({"ok": false, "error": e}),
                                    }
                                };
                                match atomic_office_write(&dest, &out) {
                                    Ok(()) => {
                                        json!({"ok": true, "path": dest.display().to_string()})
                                    }
                                    Err(e) => json!({"ok": false, "error": e}),
                                }
                            }
                            Err(e) => json!({"ok": false, "error": e.to_string()}),
                        }
                    }
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                }
            }
            _ => json!({"ok": false, "error": format!("unknown office tool: {id}")}),
        }
    }
}

fn atomic_office_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("path has no parent")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("path has no file name")?;
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Live HTTP seam for G8: SearXNG JSON at `{endpoint}/search?format=json`, DDG HTML fallback.
///
/// FIX-09: both methods pre-flight the destination through
/// [`agentcowork_guard::netfloor::preflight_url`] immediately before the socket.
/// The search *endpoint* is user-configured (local-first SearXNG, then the
/// public instances the user opted into) so the desktop default policy applies;
/// the page `fetch` carries URLs that came out of search results — untrusted
/// content — so it is judged strictly (no loopback, no LAN). There is no
/// fallback to a direct client when the floor denies: the tier simply fails.
struct UreqSearchTransport;

impl agentcowork_search::SearchTransport for UreqSearchTransport {
    fn search(
        &self,
        endpoint: &str,
        query: &str,
    ) -> Result<Vec<agentcowork_search::SearchResult>, String> {
        let q = urlencoding::encode(query);
        if endpoint == "ddg" {
            let url = format!("https://html.duckduckgo.com/html/?q={q}");
            agentcowork_guard::netfloor::preflight_url(
                &url,
                agentcowork_guard::NetPolicy::default(),
            )
            .map_err(|e| e.to_string())?;
            let body = ureq::get(&url)
                .timeout(std::time::Duration::from_secs(8))
                .call()
                .map_err(|e| e.to_string())?
                .into_string()
                .map_err(|e| e.to_string())?;
            return Ok(parse_ddg_html(&body));
        }
        let base = endpoint.trim_end_matches('/');
        let url = format!("{base}/search?q={q}&format=json");
        agentcowork_guard::netfloor::preflight_url(&url, agentcowork_guard::NetPolicy::default())
            .map_err(|e| e.to_string())?;
        let body = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(8))
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?;
        parse_searx_json(&body)
    }

    fn fetch(&self, _tier: &str, url: &str) -> Result<String, String> {
        // Untrusted destination: the result list named this URL, so the strict
        // policy applies (loopback and LAN refused, the metadata floor always).
        agentcowork_guard::netfloor::preflight_url(url, agentcowork_guard::NetPolicy::strict())
            .map_err(|e| e.to_string())?;
        ureq::get(url)
            .timeout(std::time::Duration::from_secs(8))
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())
    }
}

fn parse_searx_json(body: &str) -> Result<Vec<agentcowork_search::SearchResult>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(results
        .iter()
        .filter_map(|r| {
            Some(agentcowork_search::SearchResult {
                url: r.get("url")?.as_str()?.to_string(),
                title: r.get("title")?.as_str().unwrap_or("").to_string(),
                snippet: r.get("content")?.as_str().unwrap_or("").to_string(),
                source: "searxng".into(),
            })
        })
        .take(8)
        .collect())
}

fn parse_ddg_html(body: &str) -> Vec<agentcowork_search::SearchResult> {
    let mut out = Vec::new();
    for chunk in body.split("result__a") {
        let Some(href) = chunk
            .split("href=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
        else {
            continue;
        };
        let title = chunk
            .split('>')
            .nth(1)
            .and_then(|s| s.split('<').next())
            .unwrap_or("")
            .to_string();
        if href.starts_with("http") {
            out.push(agentcowork_search::SearchResult {
                url: href.to_string(),
                title,
                snippet: String::new(),
                source: "ddg".into(),
            });
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// P2.3 — derive a safe file name from a URL (last path segment). A URL with
/// no path (or a trailing slash) yields the host, which is not a file name —
/// fall back to `download.bin` in that case.
fn filename_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    // No path at all (e.g. `https://a.com`) or a directory URL → fallback.
    let after_scheme = path.find("://").map(|i| &path[i + 3..]).unwrap_or(path);
    if !after_scheme.contains('/')
        || path.trim_end_matches('/').ends_with('/')
        || path.ends_with('/')
    {
        return "download.bin".to_string();
    }
    let name = path.rsplit('/').next().unwrap_or("download.bin");
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "download.bin".to_string()
    } else {
        cleaned
    }
}

fn prescan(spec: &RegisteredTool, args: &Value) -> Vec<String> {
    let shell = collect_shell(args).join("\n");
    let paths: Vec<String> = collect_paths(args);
    let urls: Vec<String> = collect_urls(args);
    let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    let url_refs: Vec<&str> = urls.iter().map(|s| s.as_str()).collect();
    let _ = spec;
    scan_all(&shell, &path_refs, &url_refs)
        .into_iter()
        .map(|h| format!("{:?} {}", h.target, h.text))
        .collect()
}

/// P69.G2 — the shell-bias deflection verdicts for the shell-shaped args, in
/// argument order. Same argument keys the blocklist pre-scan collects, so the
/// two controls see exactly the same text; only the shell is inspected (a path
/// or URL that merely names a library is not a bypass — see the documented
/// limits in [`agentcowork_guard::deflection`]).
fn deflect_shell(args: &Value) -> Vec<DeflectionNudge> {
    collect_shell(args)
        .iter()
        .filter_map(|cmd| deflect_shell_bias(cmd))
        .collect()
}

fn collect_paths(args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = args.get("path").and_then(Value::as_str) {
        out.push(p.to_string());
    }
    if let Some(arr) = args.get("files").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = v.as_str() {
                out.push(s.to_string());
            }
        }
    }
    out
}

fn collect_urls(args: &Value) -> Vec<String> {
    args.get("url")
        .and_then(Value::as_str)
        .map(|s| vec![s.to_string()])
        .unwrap_or_default()
}

fn collect_shell(args: &Value) -> Vec<String> {
    ["code", "command", "shell", "expression"]
        .iter()
        .filter_map(|k| args.get(*k).and_then(Value::as_str).map(|s| s.to_string()))
        .collect()
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

// ---------------------------------------------------------------------------
// `TASK-CAP-002` — capability-plane helpers
// ---------------------------------------------------------------------------

/// Read the resolution constraints out of request params. The policy snapshot is
/// named, never supplied: the snapshot id is recorded in the handle so a
/// mid-session policy change re-validates at ticket time
/// (`ARCH/13` §4).
fn resolution_constraints(
    params: &Value,
) -> agentcowork_guard::capability_broker::ResolutionConstraints {
    let mut constraints = agentcowork_guard::capability_broker::ResolutionConstraints::new(
        str_param(params, "environmentId").unwrap_or("local"),
        str_param(params, "policySnapshot").unwrap_or("policy-unpinned"),
    );
    if let Some(cost) = params.get("maxCostClass").and_then(Value::as_str) {
        constraints.max_cost_class = match cost {
            "free" => Some(agentcowork_guard::capability_broker::CostClass::Free),
            "low" => Some(agentcowork_guard::capability_broker::CostClass::Low),
            "standard" => Some(agentcowork_guard::capability_broker::CostClass::Standard),
            "high" => Some(agentcowork_guard::capability_broker::CostClass::High),
            _ => None,
        };
    }
    if let Some(latency) = params.get("maxLatencyClass").and_then(Value::as_str) {
        constraints.max_latency_class = match latency {
            "local" => Some(agentcowork_guard::capability_broker::LatencyClass::Local),
            "fast" => Some(agentcowork_guard::capability_broker::LatencyClass::Fast),
            "standard" => Some(agentcowork_guard::capability_broker::LatencyClass::Standard),
            "slow" => Some(agentcowork_guard::capability_broker::LatencyClass::Slow),
            _ => None,
        };
    }
    constraints
}

/// The concrete next action a blocked resolution names. A blocked chain is a
/// result with a next step, never a dead end (`ARCH/13` §3 · `REQ-CAP-005`).
fn unresolved_next_action(
    capability_id: &str,
    unresolved: &agentcowork_guard::capability_broker::Unresolved,
) -> agentcowork_guard::capability_contract::NextAction {
    use agentcowork_guard::capability_broker::Unresolved as U;
    use agentcowork_guard::capability_contract::{NextAction, NextActionKind};
    match unresolved {
        U::NotRegistered { .. } => NextAction::new(
            NextActionKind::Install,
            capability_id,
            format!("`{capability_id}` is not a registered capability; nothing can invoke it"),
            "a registered capability with a descriptor and a verification hook",
        ),
        U::Deprecated { .. } => NextAction::new(
            NextActionKind::Configure,
            capability_id,
            format!("`{capability_id}` is past its deprecation window and no longer resolves"),
            "a capability inside its deprecation window",
        ),
        U::MissingRequirement {
            missing,
            kind,
            rationale,
            ..
        } => {
            let (kind, instruction) = match kind.as_str() {
                "connection" => (
                    NextActionKind::Connect,
                    format!("connect `{missing}` — {rationale}"),
                ),
                "environment" => (
                    NextActionKind::Install,
                    format!("provide `{missing}` — {rationale}"),
                ),
                _ => (
                    NextActionKind::Configure,
                    format!("enable `{missing}` — {rationale}"),
                ),
            };
            NextAction::new(kind, missing.clone(), instruction, capability_id)
        }
        U::NoServableProvider { excluded, .. } => NextAction::new(
            NextActionKind::Configure,
            capability_id,
            format!(
                "no provider can serve `{capability_id}` ({}) — repair or attach one, then retry",
                excluded
                    .iter()
                    .map(|e| format!("{}: {}", e.provider_id, e.reason))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            format!("`{capability_id}` becomes invokable"),
        ),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// P64.5 — Unified native edit ladder (SPEC I14, ARCH/17 §17.10)
// ---------------------------------------------------------------------------
//
// One ladder, three rungs — exact single-occurrence → structured/AST →
// order-tolerant fuzzy fallback — all through the Guard-2 ticket +
// verified-commit path (`tool/exec` → `tool/commit` + Merkle `tool.exec`
// row). Ambiguity fails closed (0 or 2+ matches refuse, never guess).
//
// The structured rung is text-splice + reparse-shape: splice the replacement,
// then re-extract the symbol shape on both sides. Tree-sitter precision plugs
// in as another [`EditShapeSource`] without touching the ladder — the same
// pattern as `agentcowork-codeintel::repomap::TagSource`. No new dependencies:
// the bundled [`LexicalShapeSource`] is dependency-free.

/// P64.5 — edit payload ceiling (50 KB cap, ARCH/17 edge case 8).
pub const P64_MAX_EDIT_BYTES: usize = 50 * 1024;

/// FIX-08 — how many per-effect receipts the executor keeps addressable by id.
///
/// The receipts themselves are immutable and were returned in their commit
/// response; this is only the lookup index, and it is bounded so a long session
/// cannot grow it without limit. The append-only `tool.exec` audit row remains
/// the durable record of the same fact.
pub const RECEIPT_INDEX_CAP: usize = 1024;

/// P64.5 — the unified edit tool id (registered in `extra_tools` below).
pub const EDIT_TOOL_ID: &str = "file_ops.edit";

/// P64.5 — which ladder rung produced the edit (recorded on the Work receipt
/// via `execution/record_edit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EditStrategy {
    Exact,
    Structured,
    Fuzzy,
}

impl EditStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            EditStrategy::Exact => "exact",
            EditStrategy::Structured => "structured",
            EditStrategy::Fuzzy => "fuzzy",
        }
    }
}

/// P64.5 — typed edit failure (surfaced as `{ok:false, error, refused:true}`
/// so the model asks for more context instead of guessing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// No occurrence of `old` found — refuse, ask for more context.
    NotFound,
    /// 2+ occurrences — ambiguous, refuse rather than guess.
    Ambiguous { count: usize },
    /// Empty `old` string (would match everywhere).
    EmptyOld,
    /// Payload over the 50 KB cap.
    PayloadTooLarge { bytes: usize, cap: usize },
    /// Structured rung: the splice changed the symbol shape unexpectedly.
    ShapeChanged { before: usize, after: usize },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NotFound => {
                write!(
                    f,
                    "edit refused: no occurrence found (0 matches); provide more context"
                )
            }
            EditError::Ambiguous { count } => write!(
                f,
                "edit refused: ambiguous match ({count} occurrences); provide more context for a single occurrence"
            ),
            EditError::EmptyOld => write!(f, "edit refused: `old` must not be empty"),
            EditError::PayloadTooLarge { bytes, cap } => {
                write!(
                    f,
                    "edit refused: payload {bytes} bytes over the {cap} byte cap"
                )
            }
            EditError::ShapeChanged { before, after } => write!(
                f,
                "edit refused: structured reparse changed symbol shape ({before} → {after}); refusing rather than corrupting"
            ),
        }
    }
}

/// P64.5 — a shape probe for the structured rung. Mirrors the
/// `agentcowork-codeintel::repomap::TagSource::extract` signature
/// (`content → symbols`) so a tree-sitter source can be injected without a
/// new dependency; the ladder only compares the *shape* (symbol multiset),
/// never the tree itself.
pub trait EditShapeSource {
    fn symbols(&self, content: &str) -> Vec<String>;
}

/// P64.5 — dependency-free lexical shape source (line-prefix heuristic:
/// `fn`/`struct`/`enum`/`const`/`static`/`class`/`def`). Tree-sitter plugs in
/// as another `EditShapeSource`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LexicalShapeSource;

impl EditShapeSource for LexicalShapeSource {
    fn symbols(&self, content: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in content.lines() {
            let t = line.trim_start();
            // Strip common visibility/decoration prefixes. Each binding must
            // shadow the previous one so the next strip sees the *stripped*
            // text: the previous chain read `.strip_prefix("pub ")
            // .unwrap_or(t).strip_prefix("async ").unwrap_or(t)`, and that
            // second `unwrap_or(t)` bound the pre-strip `t` — so a `pub fn`
            // (not `pub async`) was restored *with* its `pub ` prefix and then
            // matched no keyword, hiding every pub-decorated declaration from
            // the probe.
            let t = t.strip_prefix("pub ").unwrap_or(t);
            let t = t.strip_prefix("async ").unwrap_or(t);
            for kw in [
                "fn ", "struct ", "enum ", "const ", "static ", "class ", "def ",
            ] {
                if let Some(rest) = t.strip_prefix(kw) {
                    let sym: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !sym.is_empty() {
                        out.push(format!("{kw}{sym}"));
                    }
                    break;
                }
            }
        }
        out.sort();
        out
    }
}

/// P64.5 — count exact occurrences of `needle` in `content`.
pub fn count_occurrences(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    content.match_indices(needle).count()
}

/// P64.5 — rung 1: exact single-occurrence splice. Fails closed on 0 or 2+
/// matches (the Claude Code invariant).
pub fn apply_exact_once(
    content: &str,
    old: &str,
    new: &str,
) -> Result<(String, EditStrategy), EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyOld);
    }
    if old.len() + new.len() > P64_MAX_EDIT_BYTES * 4 {
        // Guard against pathological multi-MB splices before touching memory.
        let bytes = old.len() + new.len();
        if bytes > P64_MAX_EDIT_BYTES * 4 {
            return Err(EditError::PayloadTooLarge {
                bytes,
                cap: P64_MAX_EDIT_BYTES * 4,
            });
        }
    }
    match count_occurrences(content, old) {
        0 => Err(EditError::NotFound),
        1 => Ok((content.replacen(old, new, 1), EditStrategy::Exact)),
        n => Err(EditError::Ambiguous { count: n }),
    }
}

/// P64.5 — strip every whitespace character, keeping a byte index map back into
/// the original so a token match can be converted into a raw splice range.
fn whitespace_free(s: &str) -> (String, Vec<usize>) {
    let mut text = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len());
    for (i, ch) in s.char_indices() {
        if ch.is_whitespace() {
            continue;
        }
        text.push(ch);
        map.push(i);
    }
    (text, map)
}

/// P64.5 — rung 2: structured edit = token-exact splice + reparse shape.
///
/// `old` is located by *token* equality: whitespace is insignificant (so a
/// re-indented, re-wrapped, or reformatted target still matches exactly), while
/// every other character must match byte for byte. That makes this rung
/// strictly stricter than the fuzzy rung — it refuses any window whose tokens
/// are not exactly `old`'s — and strictly more tolerant than the exact rung,
/// which is what makes it a real rung rather than a second exact check.
///
/// The raw byte range the token match maps to is spliced, then the shape check
/// verifies the symbol multiset did not change by more than ±1 (one renamed
/// declaration is legitimate because `old` held the name; two or more is a
/// corruption signal). A tree-sitter `EditShapeSource` can replace the lexical
/// probe without touching this function.
pub fn apply_structured_edit(
    content: &str,
    old: &str,
    new: &str,
    shape: &dyn EditShapeSource,
) -> Result<(String, EditStrategy), EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyOld);
    }
    let (c_text, c_map) = whitespace_free(content);
    let (t_text, _) = whitespace_free(old);
    if t_text.is_empty() {
        return Err(EditError::EmptyOld);
    }
    let byte_pos = match count_occurrences(&c_text, &t_text) {
        0 => return Err(EditError::NotFound),
        1 => c_text.find(&t_text).unwrap_or(0),
        n => return Err(EditError::Ambiguous { count: n }),
    };
    // Map the token range back to raw byte offsets. `c_map` is indexed by char
    // position while `byte_pos`/`t_text` are byte lengths, so convert first.
    let start_idx = c_text[..byte_pos].chars().count();
    let t_len = t_text.chars().count();
    let first_byte = c_map[start_idx];
    let last_byte = c_map[start_idx + t_len - 1] + 1;
    let mut spliced = String::with_capacity(content.len() + new.len());
    spliced.push_str(&content[..first_byte]);
    spliced.push_str(new);
    spliced.push_str(&content[last_byte..]);
    let before = shape.symbols(content).len();
    let after = shape.symbols(&spliced).len();
    if before.abs_diff(after) > 1 {
        return Err(EditError::ShapeChanged { before, after });
    }
    Ok((spliced, EditStrategy::Structured))
}

/// Normalize one line for fuzzy comparison: whitespace-insensitive (all
/// whitespace stripped) so `fn alpha() {` matches `fn  alpha( )  {`. This is
/// deliberately the last ladder rung — exact already failed, so the fallback
/// is maximally tolerant while still failing closed on 0 or 2+ windows.
fn norm_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// P64.5 — rung 3: order-tolerant fuzzy multi-hunk fallback (shadow-VCS pattern).
/// Matches `old`'s non-empty normalized lines as an ordered subsequence of
/// the content's normalized lines (allowing reordered/extra lines between
/// hunks would risk corruption, so order is preserved but gaps are allowed).
/// Fails closed on 0 or 2+ candidate windows.
pub fn apply_fuzzy_edit(
    content: &str,
    old: &str,
    new: &str,
) -> Result<(String, EditStrategy), EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyOld);
    }
    let want: Vec<String> = old
        .lines()
        .map(norm_line)
        .filter(|l| !l.is_empty())
        .collect();
    if want.is_empty() {
        return Err(EditError::EmptyOld);
    }
    let have: Vec<String> = content.lines().map(norm_line).collect();
    // Find candidate start lines matching the first wanted line.
    let mut starts = Vec::new();
    for (i, line) in have.iter().enumerate() {
        if *line == want[0] {
            // Check the remaining wanted lines appear in order after i.
            let mut j = i;
            let mut ok = true;
            for w in want.iter().skip(1) {
                let mut found = false;
                j += 1;
                while j < have.len() {
                    if have[j] == *w {
                        found = true;
                        break;
                    }
                    j += 1;
                }
                if !found {
                    ok = false;
                    break;
                }
            }
            if ok {
                starts.push(i);
            }
        }
    }
    match starts.len() {
        0 => Err(EditError::NotFound),
        1 => {
            // Splice the raw line range [start, end-of-last-match] with `new`.
            let start = starts[0];
            // Re-locate the end line index for the splice.
            let mut j = start;
            for w in want.iter().skip(1) {
                j += 1;
                while j < have.len() && have[j] != *w {
                    j += 1;
                }
            }
            let raw: Vec<&str> = content.lines().collect();
            let mut out = Vec::with_capacity(raw.len() + 1);
            for (i, line) in raw.iter().enumerate() {
                if i == start {
                    out.push(new.to_string());
                }
                if !(i >= start && i <= j) {
                    out.push(line.to_string());
                }
            }
            // Trailing-newline fidelity: preserve the original ending.
            let mut joined = out.join("\n");
            if content.ends_with('\n') && !joined.ends_with('\n') {
                joined.push('\n');
            }
            Ok((joined, EditStrategy::Fuzzy))
        }
        n => Err(EditError::Ambiguous { count: n }),
    }
}

/// P64.5 — the unified ladder: exact → structured → fuzzy. The first rung
/// that succeeds wins; ambiguity/not-found falls through to the next rung,
/// and if all three refuse, the *first* refusal is returned (exact's verdict
/// is the most actionable). Empty-shape sources are never consulted on the
/// fuzzy path.
pub fn apply_edit_ladder(
    content: &str,
    old: &str,
    new: &str,
    shape: &dyn EditShapeSource,
) -> Result<(String, EditStrategy), EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyOld);
    }
    if content.len() > P64_MAX_EDIT_BYTES * 8 || old.len() > P64_MAX_EDIT_BYTES {
        return Err(EditError::PayloadTooLarge {
            bytes: content.len().max(old.len()),
            cap: P64_MAX_EDIT_BYTES,
        });
    }
    let first_err = match apply_exact_once(content, old, new) {
        Ok(ok) => return Ok(ok),
        Err(e) => e,
    };
    // Rung 2 tolerates whitespace-only differences while requiring every other
    // character to match exactly — exactly the "trivial shape noise" that made
    // rung 1 miss. Rung 3 is the most tolerant (gaps and reordering allowed).
    // Strictest-first means the recorded strategy is the least tolerant rung
    // that is still able to apply the splice.
    if let Ok(ok) = apply_structured_edit(content, old, new, shape) {
        return Ok(ok);
    }
    if let Ok(ok) = apply_fuzzy_edit(content, old, new) {
        return Ok(ok);
    }
    Err(first_err)
}

/// P64.6 — the risk an in-place splice derives by itself (SPEC I15).
///
/// `structural` fires when the declaration count (the [`LexicalShapeSource`]
/// symbol multiset) changes or the bracket balance shifts across the splice —
/// exactly the edits whose breakage a typecheck can catch. `destructive` is
/// never derived from an in-place splice: deletion-shaped edits arrive
/// through `file_ops.delete`, and an explicit caller flag can only raise the
/// gate (`derived || explicit`), never silence a derived risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditRisk {
    pub structural: bool,
}

pub fn derive_edit_risk(before: &str, after: &str) -> EditRisk {
    let shape = LexicalShapeSource;
    let structural = shape.symbols(before).len() != shape.symbols(after).len()
        || bracket_balance(before) != bracket_balance(after);
    EditRisk { structural }
}

/// P64.6 — net bracket balance per kind (`{}`, `()`, `[]`). A splice that
/// opens or closes a scope changes the vector; pure renames leave it alone.
fn bracket_balance(content: &str) -> (i64, i64, i64) {
    let mut brace = 0i64;
    let mut paren = 0i64;
    let mut bracket = 0i64;
    for c in content.chars() {
        match c {
            '{' => brace += 1,
            '}' => brace -= 1,
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
    }
    (brace, paren, bracket)
}

/// P64.6 — the pre-commit shadow verdict for one staged edit candidate.
///
/// Only [`EditPreflight::Failed`] blocks the write. Every other outcome is
/// either the gate not firing ([`EditPreflight::Skipped`]) or an unrunnable
/// preflight with no evidence either way ([`EditPreflight::Unverifiable`]) —
/// both proceed, honestly marked, and neither leaves a receipt behind.
enum EditPreflight {
    Skipped(&'static str),
    Unverifiable(String),
    Passed(&'static str),
    Failed(&'static str, String),
}

impl EditPreflight {
    fn blocks(&self) -> bool {
        matches!(self, EditPreflight::Failed(..))
    }

    fn refusal(&self) -> String {
        match self {
            EditPreflight::Failed(reason, output) => {
                format!("shadow preflight failed ({reason}) — refusing before commit:\n{output}")
            }
            _ => "shadow preflight did not block".to_string(),
        }
    }

    fn report(&self) -> Value {
        match self {
            EditPreflight::Skipped(reason) => json!({
                "needsPreflight": false,
                "verified": false,
                "reason": reason,
            }),
            EditPreflight::Unverifiable(reason) => json!({
                "needsPreflight": true,
                "verified": false,
                "reason": reason,
            }),
            EditPreflight::Passed(reason) => json!({
                "needsPreflight": true,
                "verified": true,
                "passed": true,
                "reason": reason,
            }),
            EditPreflight::Failed(reason, output) => json!({
                "needsPreflight": true,
                "verified": true,
                "passed": false,
                "reason": reason,
                "output": output,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// P64.9 — Shared-plane task façades (SPEC F16, ARCH/17 §17.5)
// ---------------------------------------------------------------------------
//
// External agents never receive 51 raw primitives — they receive task-shaped
// façades over the SAME Rust methods (one engine, two façades). Each façade
// carries flat dot-hierarchy ids, `readOnly` + destructive hints, and fans
// out to canonical tool ids. Guard-2 + Merkle audit are unchanged: façades
// are `ToolFamily::Facade` registry entries dispatched through the same
// `exec` → `commit` path.

/// P64.9 — one task façade: a flat dot-hierarchy id over canonical tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacadeRoute {
    /// Flat unique id, e.g. `office.edit`.
    pub facade: &'static str,
    /// Human description (mirrored in `agentcowork-mcp::SHARED_FACADES`).
    pub description: &'static str,
    /// Whether the façade never mutates (maps to `RegisteredTool.read_only`).
    pub read_only: bool,
    /// Whether the façade can destroy data (surfaced as the destructive hint).
    pub destructive: bool,
    /// Canonical tool ids this façade fans out to (must exist in the registry).
    /// Empty for kernel-routed façades (`kernel_route`), which reach the
    /// delegation seam instead of catalog tools (P71.1).
    pub targets: &'static [&'static str],
    /// P71.1 — true when the façade is served by a kernel seam rather than a
    /// catalog fan-out. Only `delegate.*` uses this: delegation mints a child
    /// Work through the Work Gateway, and a fabricated catalog target would be
    /// the parallel path I4 forbids.
    pub kernel_route: bool,
}

/// P64.9 — the shared-plane façade table (ARCH/17 §17.5). Office 6 ·
/// browser 3 · computer-use 2 · workspace 1 · artifact 2 · work 2 = 16
/// catalog-routed surfaces, plus the 3 kernel-routed delegation façades
/// (`delegate.*`, P71.1) that reach the Work Gateway child-Work seam instead
/// of the 51-tool catalog.
pub const FACADE_ROUTES: &[FacadeRoute] = &[
    FacadeRoute {
        facade: "office.open",
        description: "Open a document for reading (docx/xlsx/pptx/pdf)",
        read_only: true,
        destructive: false,
        targets: &[
            "office.docx_open",
            "office.xlsx_open",
            "office.pptx_open",
            "office.pdf_open",
        ],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "office.inspect",
        description: "Inspect document structure (outline, sheets, pages)",
        read_only: true,
        destructive: false,
        targets: &[
            "office.docx_open",
            "office.xlsx_open",
            "office.pdf_open",
            "office.pdf_pages",
        ],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "office.edit",
        description: "Edit one document block or cell (surgical patch)",
        read_only: false,
        destructive: false,
        targets: &["office.docx_patch", "office.xlsx_edit", "office.pptx_patch"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "office.calculate",
        description: "Recalculate a spreadsheet through the formula engine",
        read_only: false,
        destructive: false,
        targets: &["office.xlsx_edit", "office.xlsx_open"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "office.render",
        description: "Render or page a document (form fill, page ops)",
        read_only: false,
        destructive: false,
        targets: &["office.pdf_form_fill", "office.pdf_pages"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "office.verify",
        description: "Verify document conformance (open + inspect)",
        read_only: true,
        destructive: false,
        targets: &["office.docx_open", "office.pdf_open"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "browser.research",
        description: "Research the web (search + read + deep research)",
        read_only: true,
        destructive: false,
        targets: &["search.query", "read", "grep"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "browser.operate",
        description: "Operate the browser (navigate + snapshot + act + wait)",
        read_only: false,
        destructive: false,
        targets: &["navigate", "snapshot", "act", "wait"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "browser.extract",
        description: "Extract page content (read + grep + pdf + screenshot)",
        read_only: true,
        destructive: false,
        targets: &["read", "grep", "pdf", "screenshot"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "computer_use.see",
        description: "Observe the desktop (list windows + read a11y tree)",
        read_only: true,
        destructive: false,
        targets: &["desktop.windows", "desktop.read"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "computer_use.act",
        description: "Act on the desktop (click/type/scroll/launch)",
        read_only: false,
        destructive: true,
        targets: &["desktop.act"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "workspace.map",
        description: "Map the workspace (scan + filename search)",
        read_only: true,
        destructive: false,
        targets: &["disk_scan", "filename_search", "file_ops.list"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "artifact.store",
        description: "Store an artifact (write a workspace file)",
        read_only: false,
        destructive: false,
        targets: &["file_ops.write"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "artifact.retrieve",
        description: "Retrieve an artifact (read a workspace file)",
        read_only: true,
        destructive: false,
        targets: &["file_ops.read"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "work.create",
        description: "Create durable work (plan + checkpoint a task)",
        read_only: false,
        destructive: false,
        targets: &["file_ops.write"],
        kernel_route: false,
    },
    FacadeRoute {
        facade: "work.status",
        description: "Read durable work status (list + read state)",
        read_only: true,
        destructive: false,
        targets: &["file_ops.read", "file_ops.list"],
        kernel_route: false,
    },
    // P71.1 — the delegation family: the façade that makes delegation a
    // platform feature instead of the built-in engine's private ability.
    // Kernel-routed (`kernel_route`) to the Work Gateway child-Work seam;
    // no catalog fan-out exists or should exist.
    FacadeRoute {
        facade: "delegate.spawn",
        description: "Delegate a task to a child Work (spawn a subagent)",
        read_only: false,
        destructive: false,
        targets: &[],
        kernel_route: true,
    },
    FacadeRoute {
        facade: "delegate.status",
        description: "Read one delegated child's state (child Work + presence)",
        read_only: true,
        destructive: false,
        targets: &[],
        kernel_route: true,
    },
    FacadeRoute {
        facade: "delegate.cancel",
        description: "Cancel one delegated child (close its child Work)",
        read_only: false,
        destructive: false,
        targets: &[],
        kernel_route: true,
    },
    // P69.G5 — the spool drilldown. Kernel-routed to the content-addressed
    // spool, not a catalog fan-out: there is no file to read, only a content
    // address to resolve inside the spool root. `read_only` is true, so it
    // rides the same native-read auto-allow that `file_ops.read` gets — the
    // floor, the ticket, and the audit row are the executor's, unchanged.
    FacadeRoute {
        facade: "retrieve_original",
        description: "Read a line range of a spooled tool output by its content address (hash)",
        read_only: true,
        destructive: false,
        targets: &[],
        kernel_route: true,
    },
];

/// P64.9 — look up a façade by id.
pub fn find_facade(facade: &str) -> Option<&'static FacadeRoute> {
    FACADE_ROUTES.iter().find(|r| r.facade == facade)
}

/// P64.9 — true when `id` is a façade (not a native tool).
pub fn is_facade(id: &str) -> bool {
    find_facade(id).is_some()
}

/// P64.9 — build the façade registry entries (same shape as native tools so
/// `tool/list` serves one catalog; Guard-2 operation/risk mirror the most
/// permissive fan-out target).
fn facade_tools() -> Vec<RegisteredTool> {
    FACADE_ROUTES
        .iter()
        .map(|r| {
            let (operation, risk) = if r.destructive {
                ("delete", "high")
            } else if r.read_only {
                ("write", "low")
            } else if r.facade.starts_with("browser.") || r.facade.starts_with("computer_use.") {
                ("web_action", "high")
            } else {
                ("write", "medium")
            };
            let route = if r.kernel_route {
                "kernel: delegation (child Work)".to_string()
            } else {
                r.targets.join(", ")
            };
            stamp_tier(RegisteredTool {
                id: r.facade.to_string(),
                family: ToolFamily::Facade,
                description: format!("{} (façade → {route})", r.description),
                read_only: r.read_only,
                operation: operation.to_string(),
                risk: risk.to_string(),
                risk_tier: String::new(),
                args_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "query": { "type": "string" },
                        "url": { "type": "string" },
                        "text": { "type": "string" }
                    },
                    "additionalProperties": true
                }),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn svc(dir: &Path) -> ToolService {
        ToolService::new(Arc::new(Mutex::new(GuardService::new())), dir.to_path_buf())
    }

    /// A `ToolService` for the **adversarial corpus** loops.
    ///
    /// Those loops drive the executor hundreds of times inside a single
    /// millisecond — deliberately a far larger burst than any human or agent
    /// turn — so admission control would refuse them for the right reason and
    /// hide the floor verdict the test is actually asserting. Rate limiting has
    /// its own test (`the_tool_path_rate_limits_and_fails_closed`).
    fn svc_for_corpus(dir: &Path) -> ToolService {
        let mut s = svc(dir);
        s.rate_limiter = Arc::new(agentcowork_guard::RateLimiter::new(
            agentcowork_guard::RateLimitConfig {
                global: agentcowork_guard::Limit::new(1_000_000, 0.0),
                per_caller_command: agentcowork_guard::Limit::new(1_000_000, 0.0),
                ttl_ms: 60_000,
                max_entries: 64,
            },
        ));
        s
    }

    #[derive(Default)]
    struct CountingTerminal {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl TerminalExecutor for CountingTerminal {
        fn run(
            &self,
            command: &str,
            label: &str,
            _origin: crate::terminal::TerminalOrigin,
        ) -> Result<TerminalRun, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(TerminalRun {
                pty_id: "test-pty".into(),
                profile_id: "test-profile".into(),
                command: command.to_string(),
                cwd: ".".into(),
                exit_code: Some(0),
                output: label.to_string(),
                trusted: true,
            })
        }
    }

    fn attach_counting_terminal(service: &mut ToolService, terminal: &Arc<CountingTerminal>) {
        let backend = Arc::clone(terminal);
        service.attach_terminal(backend);
    }

    fn approved_preflight(
        service: &mut ToolService,
        guard: &Arc<Mutex<GuardService>>,
        tool_id: &str,
        args: Value,
    ) -> Value {
        let pre = service
            .handle(
                "tool/exec",
                &json!({
                    "toolId": tool_id,
                    "sessionId": "s",
                    "agentId": "a",
                    "args": args
                }),
            )
            .unwrap();
        if pre["action"] == "ask" {
            let ticket_id = pre["ticketId"].as_str().unwrap();
            assert!(guard.lock().unwrap().approve(ticket_id));
        }
        pre
    }

    #[test]
    fn filename_from_url_sanitizes() {
        assert_eq!(
            filename_from_url("https://a.com/x/file.pdf?token=1"),
            "file.pdf"
        );
        assert_eq!(filename_from_url("https://a.com/"), "download.bin");
        assert_eq!(
            filename_from_url("https://a.com/a/b/../etc/passwd"),
            "passwd"
        );
        assert_eq!(filename_from_url("https://a.com/na<>me.txt"), "na__me.txt");
    }

    #[test]
    fn download_file_writes_into_workspace_floor() {
        // Spin a local HTTP server that serves a small payload, then exercise
        // `download_file` end-to-end (floor + atomic write + byte count).
        let dir = tempfile();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut s = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                use std::io::{Read, Write};
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let body = b"hello download";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.write_all(body);
            }
        });
        let url = format!("http://{addr}/data/payload.bin");
        let s = svc(&dir);
        let out = s.dispatch_download_file(&json!({"url": url, "dir": "downloads"}));
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["bytes"], 14);
        let path = out["path"].as_str().unwrap();
        assert!(path.ends_with("payload.bin"));
        assert_eq!(fs::read_to_string(path).unwrap(), "hello download");
    }

    /// FIX-09 — the download path is floored at the socket, not only in
    /// `tool/exec`. `dispatch` is reachable from any in-process caller, so the
    /// pre-flight cannot rely on the executor having run first.
    #[test]
    fn download_file_refuses_a_floored_destination_before_any_request() {
        let dir = tempfile();
        let s = svc(&dir);
        for url in [
            "http://169.254.169.254/latest/meta-data/iam/",
            "http://192.168.1.1/admin",
            "file:///etc/passwd",
            "not a url",
        ] {
            let out = s.dispatch_download_file(&json!({"url": url, "dir": "downloads"}));
            assert_eq!(out["ok"], false, "{url} must be refused: {out}");
            let err = out["error"].as_str().unwrap_or_default();
            assert!(
                err.contains("egress denied") || err.contains("path floor"),
                "{url} → {err}"
            );
        }
        // Nothing was written: the refusal happens before the transfer.
        let downloads = dir.join("downloads");
        let written = fs::read_dir(&downloads).map(|d| d.count()).unwrap_or(0);
        assert_eq!(written, 0, "a refused download must leave no file");
    }

    /// FIX-08 — every committed mutating effect emits a receipt that cites the
    /// ticket that authorized it (REQ-PROD-001 / REQ-ART-003), and the receipt
    /// is resolvable afterwards (evidence replay, never re-execution).
    #[test]
    fn a_committed_effect_emits_a_receipt_that_cites_its_ticket() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir);
        let args = json!({"path": "a.txt", "content": "receipted"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.write", args.clone());
        let body = json!({
            "toolId": "file_ops.write",
            "ticketId": pre["ticketId"],
            "argsHash": pre["argsHash"],
            "args": args
        });
        let out = s.handle("tool/commit", &body).expect("commit must run");
        assert_eq!(out["ok"], true, "{out}");
        let ticket_id = pre["ticketId"].as_str().unwrap();

        let receipt_id = out["receiptId"]
            .as_str()
            .expect("REQ-ART-003: a mutating effect must carry a receipt");
        // The nested receipt body is `agentcowork_audit::EffectReceipt`'s own
        // serde shape (snake_case, the same one `worktrees::append_receipt`
        // persists); `receiptId` above is the camelCase handle the boundary
        // uses for the same record.
        let receipt = &out["receipt"];
        assert_eq!(receipt["effect_id"], receipt_id);
        assert_eq!(receipt["ticket_id"], ticket_id);
        assert_eq!(receipt["tool_id"], "file_ops.write");
        assert_eq!(receipt["has_gap"], false);
        // The receipt is addressable afterwards and reports the same fact.
        let replay = s
            .handle("tool/receipt", &json!({"receiptId": receipt_id}))
            .expect("receipt must be replayable");
        assert_eq!(replay["receipt"]["ticket_id"], ticket_id);
        // An unknown id is an honest error, never an invented receipt.
        assert!(
            s.handle("tool/receipt", &json!({"receiptId": "rcpt:nope"}))
                .is_err()
        );
    }

    #[test]
    fn file_write_commit_refuses_target_mutation_since_preflight() {
        let dir = tempfile();
        let path = dir.join("a.txt");
        fs::write(&path, b"before").unwrap();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut service = ToolService::new(Arc::clone(&guard), dir.clone());
        let args = json!({"path": "a.txt", "content": "approved content"});
        let pre = approved_preflight(&mut service, &guard, "file_ops.write", args.clone());

        fs::write(&path, b"external change since approval").unwrap();
        let result = service.handle(
            "tool/commit",
            &json!({
                "toolId": "file_ops.write",
                "ticketId": pre["ticketId"],
                "argsHash": pre["argsHash"],
                "args": args,
            }),
        );

        assert!(result.unwrap_err().contains("TOCTOU"));
        assert_eq!(fs::read(&path).unwrap(), b"external change since approval");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_write_commit_refuses_target_appearing_since_preflight() {
        let dir = tempfile();
        let path = dir.join("new.txt");
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut service = ToolService::new(Arc::clone(&guard), dir.clone());
        let args = json!({"path": "new.txt", "content": "approved content"});
        let pre = approved_preflight(&mut service, &guard, "file_ops.write", args.clone());

        fs::write(&path, b"created after approval").unwrap();
        let result = service.handle(
            "tool/commit",
            &json!({
                "toolId": "file_ops.write",
                "ticketId": pre["ticketId"],
                "argsHash": pre["argsHash"],
                "args": args,
            }),
        );

        assert!(result.unwrap_err().contains("TOCTOU"));
        assert_eq!(fs::read(&path).unwrap(), b"created after approval");
        let _ = fs::remove_dir_all(&dir);
    }

    /// REQ-ART-012 — an effect that could not be fully observed is recorded as
    /// a gap with a reason, never as a clean success.
    #[test]
    fn an_unobserved_mutating_effect_is_a_receipt_gap() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir);
        // A delete of a file that is not there: the dispatch fails, the ticket
        // was already spent, and the effect's state is genuinely uncertain.
        let args = json!({"path": "missing.txt"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.delete", args.clone());
        let body = json!({
            "toolId": "file_ops.delete",
            "ticketId": pre["ticketId"],
            "argsHash": pre["argsHash"],
            "args": args
        });
        let out = s.handle("tool/commit", &body).expect("commit must answer");
        assert_eq!(out["ok"], false, "{out}");
        assert_eq!(out["state"], "uncertain");
        // The receipt is still emitted (an effect cannot complete without one)
        // and it is honest about the gap.
        let receipt = &out["receipt"];
        assert_eq!(receipt["has_gap"], true);
        assert!(
            receipt["uncertainty"]
                .as_str()
                .unwrap()
                .contains("uncertain"),
            "{receipt}"
        );
    }

    // =========================================================================
    // TASK-ART-003 — the bounded preview, the artifact-gateway write, and the
    // reference it earns (`ARCH/29-ARTIFACTS.md` §7.1).
    // =========================================================================

    /// A `ToolService` whose result path has an artifact gateway attached. The
    /// spool is the one managed, content-addressed store the kernel writes
    /// through; the tests below never create a second one.
    fn svc_with_spool(
        dir: &Path,
    ) -> (
        ToolService,
        Arc<Mutex<GuardService>>,
        Arc<crate::spool::Spool>,
    ) {
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir.to_path_buf());
        let spool = Arc::new(crate::spool::Spool::new(dir));
        s.attach_spool(Arc::clone(&spool));
        (s, guard, spool)
    }

    /// Blobs currently on disk in a spool, so a test can assert a write happened
    /// (or did not) rather than inferring it from a field.
    fn spool_blobs(spool: &crate::spool::Spool) -> Vec<String> {
        let root = spool.root();
        let mut names: Vec<String> = fs::read_dir(root)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "blob"))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// A result under the inline bound is inlined whole and **no** artifact is
    /// written: nothing was truncated, so there is nothing to reach for and
    /// nothing may be advertised.
    #[test]
    fn a_result_under_the_inline_bound_is_inlined_and_writes_no_artifact() {
        let dir = tempfile();
        let (mut s, guard, spool) = svc_with_spool(&dir);
        let args = json!({"path": "small.txt", "content": "under the bound"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.write", args.clone());
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.write",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .expect("commit must run");
        assert_eq!(out["ok"], true, "{out}");
        // The value itself is still there, uncut.
        assert!(out["path"].as_str().is_some(), "{out}");
        // No artifact was written, so no reference is advertised.
        assert!(out.get("artifactRef").is_none(), "{out}");
        assert!(out.get("tool_output_ref").is_none(), "{out}");
        assert!(out.get("artifactWrite").is_none(), "{out}");
        assert_eq!(out["receipt"]["has_gap"], false, "{out}");
        assert!(out["receipt"]["resource"].is_null(), "{out}");
        assert!(
            spool_blobs(&spool).is_empty(),
            "an inlined result must not write a blob"
        );
    }

    /// Over the bound: the full bytes go through the artifact gateway **first**,
    /// and only then is the reference attached — so the reference resolves, the
    /// receipt cites it, and the truncation is visible rather than passed off as
    /// a whole result.
    #[test]
    fn a_result_over_the_inline_bound_writes_the_artifact_then_attaches_the_reference() {
        let dir = tempfile();
        let body = "y".repeat(crate::spool::INLINE_BUDGET_BYTES * 3);
        fs::write(dir.join("big.txt"), &body).unwrap();
        let (mut s, guard, spool) = svc_with_spool(&dir);
        let args = json!({"path": "big.txt"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.read", args.clone());
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.read",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .expect("commit must run");
        assert_eq!(out["ok"], true, "{out}");

        // 1) the full bytes are on disk, under their own content address;
        let full = json!({"ok": true, "content": body}).to_string();
        let hash = crate::spool::content_hash(full.as_bytes());
        let blobs = spool_blobs(&spool);
        assert_eq!(blobs, vec![format!("{hash}.blob")], "the write happened");
        let on_disk = fs::read_to_string(spool.root().join(format!("{hash}.blob"))).unwrap();
        assert_eq!(
            on_disk.len(),
            full.len(),
            "the artifact holds every byte the tool returned"
        );

        // 2) the reference is the one the write returned …
        let reference = format!("{}://artifact/{hash}", agentcowork_mcp::ARTIFACT_URI_SCHEME);
        assert_eq!(out["artifactRef"], json!(reference), "{out}");
        // … and the drilldown the reference implies is offered with it.
        assert_eq!(out["spooled"], true, "{out}");
        assert_eq!(out["tool_output_ref"]["hash"], json!(hash), "{out}");

        // 3) the truncation is visible and the accounting is truthful: the full
        //    size, never the cut length.
        assert_eq!(out["truncated"], true, "{out}");
        assert_eq!(out["totalBytes"], json!(full.len()), "{out}");
        assert_eq!(
            out["previewBytes"],
            json!(crate::spool::INLINE_BUDGET_BYTES),
            "{out}"
        );
        // 4) a truncation is never handed over as if it were the value: no
        //    payload field, and the honest gap marker is absent.
        assert!(out.get("content").is_none(), "{out}");
        assert!(out.get("artifactWrite").is_none(), "{out}");

        // 5) the receipt cites the artifact that holds the full result.
        let receipt = &out["receipt"];
        assert_eq!(receipt["resource"], json!(reference), "{receipt}");
        assert_eq!(receipt["has_gap"], false, "{receipt}");
    }

    /// A write that does not succeed yields an uncertainty receipt, not a
    /// success: the value is still delivered whole (a larger context, never a
    /// wrong answer), but nothing advertises an artifact that does not exist and
    /// the receipt says the effect could not be fully observed.
    #[test]
    fn a_failed_artifact_write_is_a_receipt_gap_not_a_clean_success() {
        let dir = tempfile();
        // A regular file where the spool root belongs: the gateway write cannot
        // create its directory, so it cannot write. The write is attempted and
        // refused — this is the real failure path, not a stubbed one.
        fs::write(dir.join(crate::spool::SPOOL_DIR_NAME), b"not a directory").unwrap();
        let body = "z".repeat(crate::spool::INLINE_BUDGET_BYTES * 3);
        fs::write(dir.join("big.txt"), &body).unwrap();
        let (mut s, guard, spool) = svc_with_spool(&dir);
        let args = json!({"path": "big.txt"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.read", args.clone());
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.read",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .expect("commit must answer");
        // The dispatch itself succeeded, so `ok` is true …
        assert_eq!(out["ok"], true, "{out}");
        // … the value is delivered whole, because dropping it would be a lie …
        assert_eq!(out["content"], json!(body), "{out}");
        // … nothing advertises an artifact …
        assert!(out.get("artifactRef").is_none(), "{out}");
        assert!(out.get("tool_output_ref").is_none(), "{out}");
        // … the missing write is stated, and stated as a failure …
        assert_eq!(out["artifactWrite"]["ok"], false, "{out}");
        assert!(out["artifactWrite"]["error"].as_str().is_some(), "{out}");
        // … the accounting stays truthful about what the real size was …
        assert_eq!(
            out["totalBytes"],
            json!(json!({"ok": true, "content": body}).to_string().len()),
            "{out}"
        );
        // … and the receipt is a gap, not a clean success.
        let receipt = &out["receipt"];
        assert_eq!(receipt["has_gap"], true, "{receipt}");
        assert!(
            receipt["uncertainty"]
                .as_str()
                .unwrap()
                .contains("artifact was not written"),
            "{receipt}"
        );
        assert!(receipt["resource"].is_null(), "{receipt}");
        assert!(spool_blobs(&spool).is_empty(), "no blob exists");
    }

    /// The same seam with no gateway attached at all: still no invented
    /// reference, still a gap. A result that cannot be reached by reference is
    /// an honest gap, never a silent partial.
    #[test]
    fn an_unattached_gateway_reaches_no_reference_and_is_a_gap() {
        let dir = tempfile();
        let body = "w".repeat(crate::spool::INLINE_BUDGET_BYTES * 3);
        fs::write(dir.join("big.txt"), &body).unwrap();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir);
        let args = json!({"path": "big.txt"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.read", args.clone());
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.read",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .expect("commit must answer");
        assert_eq!(out["content"], json!(body), "{out}");
        assert!(out.get("artifactRef").is_none(), "{out}");
        assert_eq!(out["artifactWrite"]["ok"], false, "{out}");
        assert_eq!(out["receipt"]["has_gap"], true, "{out}");
    }

    /// FIX-02 / `TASK-TRUST-001` — the tool path refuses a flood with the
    /// canonical taxonomy instead of minting tickets forever.
    #[test]
    fn the_tool_path_rate_limits_and_fails_closed() {
        let dir = tempfile();
        let mut s = svc(&dir);
        // A deliberately tiny budget so the assertion is deterministic.
        s.rate_limiter = Arc::new(agentcowork_guard::RateLimiter::new(
            agentcowork_guard::RateLimitConfig {
                global: agentcowork_guard::Limit::new(1_000, 0.0),
                per_caller_command: agentcowork_guard::Limit::new(2, 0.0),
                ttl_ms: 60_000,
                max_entries: 16,
            },
        ));
        let params = json!({"sessionId": "s1", "plane": "shared"});
        assert!(s.handle("tool/list", &params).is_ok());
        assert!(s.handle("tool/list", &params).is_ok());
        let err = s
            .handle(
                "tool/list",
                &json!({"sessionId": "rotated-session", "agentId": "rotated-agent"}),
            )
            .expect_err("the third call must be refused");
        assert!(err.contains("rate limit exceeded"), "{err}");
        assert!(
            err.contains("Unavailable") || err.contains("retry after"),
            "{err}"
        );
        // Claimed session/agent identities cannot evade this service's bucket.
        assert!(
            s.handle("tool/list", &json!({"sessionId": "s2", "agentId": "other"}))
                .expect_err("rotating caller claims must not create a fresh bucket")
                .contains("rate limit exceeded")
        );
    }

    /// `TASK-TRUST-011` — the kernel tool gate and the shell's IPC gate read the
    /// same resolved value, so neither can be a different limit from the other.
    ///
    /// The shape assertion is the direct one: the tool gate's limiter is built
    /// from the `controlPlaneRateLimit` entry, and the shell gate builds
    /// `RateLimiter::with_defaults()` — so the two are equal by construction. The
    /// behavioural assertion then proves the equality is load-bearing: both gates
    /// refuse on the same call, for the same key, with the same shape of error.
    #[test]
    fn both_admission_gates_enforce_the_same_resolved_limit() {
        use crate::config::{Config, ConfigLayer, RateLimitOverrides};

        let dir = tempfile();
        let mut kernel_gate = svc(&dir);
        // What the shell's `control_plane_limiter` builds: `with_defaults()`.
        let shell_gate = agentcowork_guard::RateLimiter::with_defaults();
        assert_eq!(
            kernel_gate.rate_limit_config(),
            *shell_gate.config(),
            "the kernel gate must be the shell gate's shape"
        );
        assert_eq!(
            kernel_gate.rate_limit_config(),
            Config::default()
                .resolve_rate_limit(&std::collections::BTreeMap::new())
                .rate_limit_config(),
            "both gates read the resolved configuration entry"
        );

        // A run override is a real layer: the host resolves it and hands the
        // kernel gate the result, and the gate enforces *that*.
        let mut layers = std::collections::BTreeMap::new();
        layers.insert(
            ConfigLayer::Run,
            RateLimitOverrides {
                caller_command_burst: Some(2),
                caller_command_per_second: Some(0.0),
                ..RateLimitOverrides::default()
            },
        );
        let resolved = Config::default().resolve_rate_limit(&layers);
        assert_eq!(
            resolved.source_of("callerCommandBurst"),
            Some(ConfigLayer::Run)
        );
        kernel_gate = kernel_gate.with_rate_limit_config(resolved.rate_limit_config());
        assert_eq!(
            kernel_gate.rate_limit_config().per_caller_command.burst,
            2.0
        );

        // Both gates, same key, same stamp: each admits exactly its own burst
        // and refuses the call after it. The stamp is fixed, so no refill can
        // blur the boundary.
        let admitted = |gate: &dyn Fn(&str, &str, u64) -> bool, calls: u32| -> u32 {
            (0..calls)
                .take_while(|_| gate("ui", "tool/list", 1_000))
                .count() as u32
        };
        let kernel_cfg = kernel_gate.rate_limit_config();
        let shell_cfg = *shell_gate.config();
        let probe = kernel_cfg
            .per_caller_command
            .burst
            .max(shell_cfg.per_caller_command.burst) as u32
            + 2;
        let kernel_admitted = admitted(
            &|caller, method, now| {
                kernel_gate
                    .rate_limiter
                    .check_at(caller, method, now)
                    .is_ok()
            },
            probe,
        );
        let shell_admitted = admitted(
            &|caller, method, now| shell_gate.check_at(caller, method, now).is_ok(),
            probe,
        );
        // The kernel gate is enforcing the run override it was handed …
        assert_eq!(kernel_admitted, 2, "the run override is what is enforced");
        // … and the shell gate is enforcing the shipped burst.
        assert_eq!(
            shell_admitted, 120,
            "the shipped per-key burst is unchanged"
        );
    }

    struct FakeBrowser;
    impl BrowserBackend for FakeBrowser {
        fn save_pdf_enhanced(&self, dir: &Path) -> Result<String, String> {
            fs::write(dir.join("page.pdf"), b"%PDF fake").map_err(|e| e.to_string())?;
            Ok(dir.join("page.pdf").display().to_string())
        }
        fn save_screenshot_enhanced(&self, dir: &Path, _quality: u8) -> Result<String, String> {
            fs::write(dir.join("shot.jpg"), b"jpeg fake").map_err(|e| e.to_string())?;
            Ok(dir.join("shot.jpg").display().to_string())
        }
    }

    #[test]
    fn save_pdf_enhanced_routes_through_browser_backend() {
        let dir = tempfile();
        let mut s = ToolService::new(Arc::new(Mutex::new(GuardService::new())), dir.clone())
            .with_browser(Arc::new(FakeBrowser));
        // Without a backend the tool fails honestly.
        let mut no_browser = svc(&dir);
        let honest = no_browser.dispatch(
            &no_browser
                .registry
                .get("save_pdf_enhanced")
                .unwrap()
                .clone(),
            &json!({"dir": "downloads"}),
        );
        assert_eq!(honest["ok"], false);
        assert!(honest["error"].as_str().unwrap().contains("not attached"));

        let out = s.dispatch(
            &s.registry.get("save_pdf_enhanced").unwrap().clone(),
            &json!({"dir": "downloads"}),
        );
        assert_eq!(out["ok"], true, "{out}");
        assert!(out["path"].as_str().unwrap().ends_with("page.pdf"));
        assert!(dir.join("downloads/page.pdf").exists());
    }

    #[test]
    fn registry_covers_catalog_plus_extras() {
        let r = ToolRegistry::new();
        assert!(r.list().len() >= 42 + 6, "got {}", r.list().len());
        assert!(r.get("file_ops.read").is_some());
        assert!(r.get("browser.navigate").is_some());
        assert!(r.get("navigate").is_some());
        assert!(r.get("disk_scan").is_some());
        assert!(r.get("script.run").is_some());
        assert!(r.get("search.query").is_some());
    }

    #[test]
    fn canonical_hash_is_key_order_stable() {
        let a = json!({"b": 1, "a": 2});
        let b = json!({"a": 2, "b": 1});
        assert_eq!(canonical_args_hash(&a), canonical_args_hash(&b));
        let c = json!({"a": 2, "b": 3});
        assert_ne!(canonical_args_hash(&a), canonical_args_hash(&c));
    }

    #[test]
    fn canonical_hash_number_forms_are_equivalent() {
        // Integer and its float twin hash identically (JS has one number type).
        assert_eq!(
            canonical_args_hash(&json!({"n": 5})),
            canonical_args_hash(&json!({"n": 5.0}))
        );
        // Distinct numbers hash differently.
        assert_ne!(
            canonical_args_hash(&json!({"n": 5})),
            canonical_args_hash(&json!({"n": 6}))
        );
        // Large integers beyond 2^53 and exponent-y floats still hash stably.
        let _ = canonical_args_hash(&json!({"big": 1e21, "coord": 12.5, "z": 0}));
    }

    /// Cross-runtime vector: these exact hex hashes must equal the coordinator
    /// `canonicalArgsHash` output for the same inputs (see tools.test.ts
    /// `canonicalArgsHash cross-runtime vector`). If either side changes the
    /// canonicalization, both this test and the TS test must be updated in
    /// lockstep — that is the guard against silent drift.
    #[test]
    fn canonical_hash_cross_runtime_vector() {
        // maxResults int, a float coord, a big int, unicode, nested + array.
        let v = json!({
            "maxResults": 50,
            "coord": 12.5,
            "big": 9007199254740993u64, // 2^53 + 1
            "label": "café \u{1f600}",
            "nested": {"z": 1, "a": [1, 2.0, 3]}
        });
        // Printed so the TS side can assert the same constant.
        let h = canonical_args_hash(&v);
        assert_eq!(h.len(), 64);
        // This exact hex MUST equal the coordinator `canonicalArgsHash` output
        // (verified against tools.test.ts). Changing canonicalization on
        // either side breaks this constant — update both in lockstep.
        assert_eq!(
            h,
            "694541888ef627ef4ed5dedf8efa323fe9f0dd32e699debbb0a155ffbe02eeac"
        );
        // The value is stable across runs (determinism).
        assert_eq!(h, canonical_args_hash(&v));
    }

    #[test]
    fn exec_unknown_tool_errors() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let err = s
            .handle("tool/exec", &json!({"toolId": "nope", "args": {}}))
            .unwrap_err();
        assert!(err.contains("unknown tool"));
    }

    #[test]
    fn read_tool_allow_then_commit() {
        let dir = tempfile();
        fs::write(dir.join("hello.txt"), "hi").unwrap();
        let mut s = svc(&dir);
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.read",
                    "sessionId": "s1",
                    "agentId": "a1",
                    "args": {"path": "hello.txt"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "allow");
        let ticket = pre["ticketId"].as_str().unwrap();
        let hash = pre["argsHash"].as_str().unwrap();
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.read",
                    "ticketId": ticket,
                    "argsHash": hash,
                    "args": {"path": "hello.txt"}
                }),
            )
            .unwrap();
        assert_eq!(out["ok"], true);
        assert_eq!(out["content"], "hi");
        assert_eq!(out["auditSeq"], 1);
        assert_eq!(s.audit_len(), 1);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir.clone());
        let args = json!({"path": "w.txt", "content": "payload"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.write", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        assert_eq!(pre["action"], "ask", "default write policy is always_ask");
        let tid = pre["ticketId"].as_str().unwrap().to_string();
        assert!(guard.lock().unwrap().approve(&tid));
        let commit = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.write",
                    "ticketId": tid,
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .unwrap();
        assert_eq!(commit["ok"], true);
        let text = fs::read_to_string(dir.join("w.txt")).unwrap();
        assert_eq!(text, "payload");
    }

    #[test]
    fn args_mismatch_refuses_commit() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.list",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"path": "."}
                }),
            )
            .unwrap();
        let err = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.list",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": {"path": "other"}
                }),
            )
            .unwrap_err();
        assert!(
            err.contains("ticket refused") || err.contains("mismatch") || err.contains("drift"),
            "{err}"
        );
    }

    #[test]
    fn pending_ticket_cannot_commit() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        // Force ask: delete is always_ask in default policy.
        let mut s = ToolService::new(Arc::clone(&guard), dir.clone());
        fs::write(dir.join("x.txt"), "x").unwrap();
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.delete",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"path": "x.txt"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "ask");
        let err = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.delete",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": {"path": "x.txt"}
                }),
            )
            .unwrap_err();
        assert!(err.to_lowercase().contains("not approved") || err.contains("ticket refused"));
        assert!(
            dir.join("x.txt").exists(),
            "must not delete without approve"
        );
    }

    #[test]
    fn approve_then_commit_deletes() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir.clone());
        fs::write(dir.join("x.txt"), "x").unwrap();
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.delete",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"path": "x.txt"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "ask");
        let tid = pre["ticketId"].as_str().unwrap().to_string();
        {
            let mut g = guard.lock().unwrap();
            assert!(g.approve(&tid));
        }
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.delete",
                    "ticketId": tid,
                    "argsHash": pre["argsHash"],
                    "args": {"path": "x.txt"}
                }),
            )
            .unwrap();
        assert_eq!(out["ok"], true);
        assert!(!dir.join("x.txt").exists());
    }

    #[test]
    fn estop_blocks_exec_and_commit() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir.clone());
        guard
            .lock()
            .unwrap()
            .handle("guard/estop", &json!({}))
            .unwrap();
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.write",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"path": "z.txt", "content": "no"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "block");
    }

    #[test]
    fn path_escape_refused() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.read",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"path": "../../etc/passwd"}
                }),
            )
            .unwrap();
        // pre-flight still allows (read) — floor is at commit
        if pre["action"] == "allow" {
            let out = s
                .handle(
                    "tool/commit",
                    &json!({
                        "toolId": "file_ops.read",
                        "ticketId": pre["ticketId"],
                        "argsHash": pre["argsHash"],
                        "args": {"path": "../../etc/passwd"}
                    }),
                )
                .unwrap();
            assert_eq!(out["ok"], false);
            let err = out["error"].as_str().unwrap_or("");
            assert!(err.contains("floor") || err.contains("refused"), "{err}");
        }
    }

    #[test]
    fn guard1_blocks_destructive_shell() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "script.run",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"code": "rm -rf /"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "block");
    }

    // -----------------------------------------------------------------
    // P69.G2 — Guard-1 shell-bias deflection on the live pre-exec path.
    // -----------------------------------------------------------------

    /// Pre-flight a `script.run` through the one pre-exec shell path.
    fn script_preflight(service: &mut ToolService, code: &str) -> Value {
        service
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "script.run",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"code": code}
                }),
            )
            .unwrap()
    }

    #[test]
    fn p69_g2_deflected_shell_is_refused_with_the_facade_to_use() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let out = script_preflight(
            &mut s,
            "python -c 'import openpyxl; openpyxl.load_workbook(\"b.xlsx\").save(\"b.xlsx\")'",
        );
        assert_eq!(out["action"], "block", "{out}");
        assert_eq!(out["refused"], true, "{out}");
        assert_eq!(out["guard"], "deflection", "{out}");
        assert_eq!(out["suggestedFacade"], "office", "{out}");
        assert_eq!(out["suggestedTool"], "office.edit", "{out}");
        assert_eq!(out["deflection"]["targets"], json!(["office"]), "{out}");
        assert_eq!(out["deflection"]["matched"], json!(["openpyxl"]), "{out}");
        let reason = out["reason"].as_str().unwrap();
        // The refusal is a first-class result the model can act on: it names
        // what was refused and the capability to call instead.
        assert!(reason.contains("openpyxl"), "{reason}");
        assert!(reason.contains("office.edit"), "{reason}");
        assert!(reason.contains("cannot be run"), "{reason}");
        // No ticket is minted, so nothing can consume one to authorize the call.
        assert!(out["ticketId"].is_null(), "{out}");
    }

    #[test]
    fn p69_g2_deflection_targets_browser_and_desktop_facades() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let b = script_preflight(&mut s, "npx playwright test --reporter=line");
        assert_eq!(b["action"], "block", "{b}");
        assert_eq!(b["suggestedFacade"], "browser", "{b}");
        assert_eq!(b["suggestedTool"], "browser.operate", "{b}");

        let d = script_preflight(&mut s, "xdotool mousemove 10 20 click 1");
        assert_eq!(d["action"], "block", "{d}");
        assert_eq!(d["suggestedFacade"], "computer_use", "{d}");
        assert_eq!(d["suggestedTool"], "computer_use.see", "{d}");
    }

    #[test]
    fn p69_g2_near_miss_identifier_is_not_deflected() {
        let dir = tempfile();
        let mut s = svc(&dir);
        for code in [
            "cat report.pptx",
            "grep -rn selenite .",
            "cat my_python_docx_wrapper.py",
            "ls playwright-report/index.html",
        ] {
            let out = script_preflight(&mut s, code);
            assert_ne!(
                out["guard"], "deflection",
                "must not be a deflection: {code} -> {out}"
            );
            assert!(out["action"] == "allow" || out["action"] == "ask", "{out}");
        }
    }

    #[test]
    fn p69_g2_a_clean_command_is_untouched_and_leaves_no_refusal_row() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let out = script_preflight(&mut s, "printf 'hello\\n'");
        assert_ne!(out["action"], "block", "{out}");
        assert!(out["action"] == "allow" || out["action"] == "ask", "{out}");
        assert!(!out["refused"].is_boolean() || out["refused"] == false);
        // Nothing was refused, so the audit chain is still empty.
        assert_eq!(s.audit_len(), 0);
    }

    #[test]
    fn p69_g2_deflection_refusal_is_recorded_on_the_audit_chain() {
        let dir = tempfile();
        let mut s = svc(&dir);
        assert_eq!(s.audit_len(), 0, "chain starts empty");
        let out = script_preflight(&mut s, "python -c 'import pyautogui; pyautogui.click()'");
        assert_eq!(out["action"], "block", "{out}");
        // The refusal appended exactly one durable row, on the same chain the
        // committed `tool.exec` rows use, and it is tamper-evident.
        assert_eq!(s.audit_len(), 1);
        assert!(s.audit.verify().is_none());
        assert_eq!(out["auditSeq"], 1);

        let payload = ToolService::deflection_audit_payload(
            "script.run",
            "h",
            "s",
            "a",
            &[agentcowork_guard::deflection::deflect_shell_bias("pyautogui").unwrap()],
        );
        assert_eq!(payload["toolId"], "script.run");
        assert_eq!(payload["guard"], "deflection");
        assert_eq!(payload["state"], "refused");
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["deflections"][0]["target"], "desktop");
        assert_eq!(payload["deflections"][0]["facade"], "computer_use");
        assert_eq!(
            payload["deflections"][0]["suggestedTool"],
            "computer_use.see"
        );
        assert_eq!(payload["deflections"][0]["matched"], "pyautogui");
        // The kind is the guard's own denial kind, not a parallel trail.
        assert_eq!(
            agentcowork_guard::deflection::DEFLECTION_AUDIT_KIND,
            "guard.blocked"
        );
    }

    #[test]
    fn p69_g2_deflection_is_not_downgradable_by_a_ticket_or_args_hash() {
        let dir = tempfile();
        let mut s = svc(&dir);
        // A caller-supplied `argsHash` (the coordinator's pre-flight claim) and
        // a pre-existing `ticketId` must not buy authorization for a refused
        // command: the deflection returns before the ticket is consulted.
        let out = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "script.run",
                    "sessionId": "s",
                    "agentId": "external",
                    "argsHash": "deadbeef",
                    "ticketId": "tkt:forged",
                    "args": {"code": "python -c 'import xlsxwriter'"}
                }),
            )
            .unwrap();
        assert_eq!(out["action"], "block", "{out}");
        assert_eq!(out["refused"], true, "{out}");
        assert_eq!(out["suggestedTool"], "office.edit", "{out}");
        assert_eq!(s.audit_len(), 1);
    }

    /// **Documented blind spot (P69.G2).** The deflection reads command text,
    /// so a module name assembled at runtime is invisible. Recorded here as
    /// well as in the guard crate so the limitation is pinned where the
    /// pre-exec decision is made. Closing it needs an AST pass over the
    /// command (`ARCH/RECOVERY.md` §13 step 1) — explicitly out of scope.
    #[test]
    fn p69_g2_dynamically_constructed_name_is_not_deflected() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let out = script_preflight(&mut s, "python -c 'import open\" + \"pyxl'");
        assert_ne!(out["action"], "block", "{out}");
        assert!(out["action"] == "allow" || out["action"] == "ask", "{out}");
        assert_ne!(out["guard"], "deflection");
        assert_eq!(s.audit_len(), 0, "no refusal row for a miss");
    }

    #[test]
    fn p69_g2_deflection_does_not_change_the_blocklist_verdict() {
        let dir = tempfile();
        let mut s = svc(&dir);
        // A destructive command keeps the blocklist's refusal wording and is
        // not re-labelled as a deflection.
        let out = script_preflight(&mut s, "rm -rf /");
        assert_eq!(out["action"], "block", "{out}");
        assert!(
            out["reason"].as_str().unwrap().contains("Guard-1 blocked"),
            "{out}"
        );
        assert!(out["refused"].is_null(), "{out}");
        assert_eq!(s.audit_len(), 0, "the blocklist path is not re-audited");
    }

    #[test]
    fn single_use_ticket() {
        let dir = tempfile();
        fs::write(dir.join("a.txt"), "a").unwrap();
        let mut s = svc(&dir);
        let args = json!({"path": "a.txt"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.read", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        let body = json!({
            "toolId": "file_ops.read",
            "ticketId": pre["ticketId"],
            "argsHash": pre["argsHash"],
            "args": args
        });
        let first = s.handle("tool/commit", &body).unwrap();
        assert_eq!(first["ok"], true);
        let second = s.handle("tool/commit", &body);
        assert!(second.is_err());
    }

    #[test]
    fn forged_ticket_consumed_claim_never_skips_guard() {
        let dir = tempfile();
        let terminal = Arc::new(CountingTerminal::default());
        let mut s = svc(&dir);
        attach_counting_terminal(&mut s, &terminal);
        let err = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "script.run",
                    "ticketId": "forged",
                    "ticketConsumed": true,
                    "args": {"code": "printf safe"}
                }),
            )
            .unwrap_err();
        assert!(err.contains("ticket refused"), "{err}");
        assert_eq!(terminal.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn missing_and_unknown_tickets_never_dispatch() {
        let dir = tempfile();
        let terminal = Arc::new(CountingTerminal::default());
        let mut s = svc(&dir);
        attach_counting_terminal(&mut s, &terminal);
        let missing = s
            .handle(
                "tool/commit",
                &json!({"toolId": "script.run", "args": {"code": "printf missing"}}),
            )
            .unwrap_err();
        assert!(missing.contains("ticketId"), "{missing}");
        let unknown = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "script.run",
                    "ticketId": "unknown",
                    "args": {"code": "printf unknown"}
                }),
            )
            .unwrap_err();
        assert!(unknown.contains("unknown ticket"), "{unknown}");
        assert_eq!(terminal.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn replayed_terminal_ticket_dispatches_only_once() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let terminal = Arc::new(CountingTerminal::default());
        let mut s = ToolService::new(Arc::clone(&guard), dir);
        attach_counting_terminal(&mut s, &terminal);
        let args = json!({"code": "printf once"});
        let pre = approved_preflight(&mut s, &guard, "script.run", args.clone());
        let body = json!({
            "toolId": "script.run",
            "ticketId": pre["ticketId"],
            "argsHash": pre["argsHash"],
            "args": args
        });
        assert_eq!(s.handle("tool/commit", &body).unwrap()["ok"], true);
        let replay = s.handle("tool/commit", &body).unwrap_err();
        assert!(replay.contains("already used"), "{replay}");
        assert_eq!(terminal.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn changed_args_cannot_consume_original_ticket_or_dispatch() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let terminal = Arc::new(CountingTerminal::default());
        let mut s = ToolService::new(Arc::clone(&guard), dir);
        attach_counting_terminal(&mut s, &terminal);
        let original = json!({"code": "printf original"});
        let changed = json!({"code": "printf changed"});
        let pre = approved_preflight(&mut s, &guard, "script.run", original);
        let err = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "script.run",
                    "ticketId": pre["ticketId"],
                    "argsHash": canonical_args_hash(&changed),
                    "args": changed
                }),
            )
            .unwrap_err();
        assert!(err.contains("args hash mismatch"), "{err}");
        assert_eq!(terminal.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn ticket_for_another_tool_cannot_dispatch() {
        #[derive(Default)]
        struct CountingExternal {
            calls: std::sync::atomic::AtomicUsize,
        }
        impl ExternalToolBackend for CountingExternal {
            fn call(&self, _tool_id: &str, _args: &Value) -> Result<Value, String> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({"called": true}))
            }
        }

        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir);
        let tools = ["custom.write", "custom.other"]
            .into_iter()
            .map(|name| ExternalTool {
                name: name.into(),
                description: "counted external write".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                read_only: false,
                open_world: true,
                source: "mcp:test".into(),
            })
            .collect::<Vec<_>>();
        let backend = Arc::new(CountingExternal::default());
        let owned = Arc::clone(&backend);
        s.attach_external_server("mcp:test", &tools, owned);
        let args = json!({"value": 1});
        let pre = approved_preflight(&mut s, &guard, "custom.write", args.clone());
        let err = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "custom.other",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .unwrap_err();
        assert!(err.contains("args hash mismatch"), "{err}");
        assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn toctou_inode_swap_refused_at_commit() {
        let dir = tempfile();
        fs::write(dir.join("a.txt"), "orig").unwrap();
        let mut s = svc(&dir);
        let args = json!({"path": "a.txt"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.read", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        assert_eq!(pre["action"], "allow");
        fs::write(dir.join("b.txt"), "swapped-inode").unwrap();
        fs::remove_file(dir.join("a.txt")).unwrap();
        fs::rename(dir.join("b.txt"), dir.join("a.txt")).unwrap();
        let err = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.read",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .unwrap_err();
        assert!(
            err.contains("TOCTOU") || err.contains("inode") || err.contains("drift"),
            "{err}"
        );
    }

    #[test]
    fn redteam_corpus_blocked_through_executor() {
        let dir = tempfile();
        let mut s = svc_for_corpus(&dir);
        for probe in agentcowork_guard::redteam::RED_TEAM_CORPUS {
            let pre = s
                .handle(
                    "tool/exec",
                    &json!({
                        "toolId": "script.run",
                        "sessionId": "s",
                        "agentId": "a",
                        "args": {"code": probe.payload}
                    }),
                )
                .unwrap();
            assert_eq!(
                pre["action"], "block",
                "probe {} escaped: {}",
                probe.name, probe.payload
            );
        }
    }

    #[test]
    fn pathfloor_fuzz_through_executor() {
        let dir = tempfile();
        let mut s = svc_for_corpus(&dir);
        let root = dir.to_string_lossy().to_string();
        for p in agentcowork_guard::pathfloor::adversarial_paths() {
            let pre = s
                .handle(
                    "tool/exec",
                    &json!({
                        "toolId": "file_ops.read",
                        "sessionId": "s",
                        "agentId": "a",
                        "args": {"path": p}
                    }),
                )
                .unwrap();
            if pre["action"] == "block" {
                continue;
            }
            let out = s.handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.read",
                    "ticketId": pre["ticketId"],
                    "argsHash": pre["argsHash"],
                    "args": {"path": p}
                }),
            );
            if let Ok(v) = out {
                if v["ok"] == true {
                    assert!(
                        agentcowork_guard::pathfloor::is_inside_root(&p, &[&root])
                            || dir.join(&p).starts_with(&dir),
                        "path floor allowed escape: {p}"
                    );
                }
            }
        }
    }

    #[test]
    fn urlfloor_fuzz_through_executor() {
        let dir = tempfile();
        let mut s = svc_for_corpus(&dir);
        for u in agentcowork_guard::urlfloor::adversarial_urls() {
            let pre = s
                .handle(
                    "tool/exec",
                    &json!({
                        "toolId": "file_ops.read",
                        "sessionId": "s",
                        "agentId": "a",
                        "args": {"path": "x.txt", "url": u}
                    }),
                )
                .unwrap();
            assert_eq!(pre["action"], "block", "url floor missed {u}");
        }
    }

    #[test]
    fn e2e_ask_approve_commit_writes_audit_and_result() {
        let dir = tempfile();
        fs::write(dir.join("doomed.txt"), "x").unwrap();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir.clone());
        let args = json!({"path": "doomed.txt"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.delete",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": args
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "ask");
        let tid = pre["ticketId"].as_str().unwrap().to_string();
        {
            let mut g = guard.lock().unwrap();
            assert!(g.approve(&tid));
        }
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.delete",
                    "ticketId": tid,
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .unwrap();
        assert_eq!(out["ok"], true);
        assert!(out["auditSeq"].as_u64().unwrap() >= 1);
        assert!(!dir.join("doomed.txt").exists());
        assert_eq!(s.audit_len(), 1);
    }

    #[test]
    fn risk_tiers_on_catalog() {
        let r = ToolRegistry::new();
        assert_eq!(r.get("file_ops.read").unwrap().risk_tier, "R0");
        assert_eq!(r.get("file_ops.delete").unwrap().risk_tier, "R3");
        assert_eq!(r.get("search.query").unwrap().risk_tier, "R2");
        assert_eq!(r.get("script.run").unwrap().risk_tier, "R3");
    }

    #[test]
    fn tool_list_is_deterministic() {
        let a = ToolRegistry::new();
        let b = ToolRegistry::new();
        let ids_a: Vec<_> = a.list().iter().map(|t| t.id.clone()).collect();
        let ids_b: Vec<_> = b.list().iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn mutating_idempotent_replay_is_refused() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let args = json!({"path": "w.txt", "content": "once"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.write", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        let tid = pre["ticketId"].as_str().unwrap().to_string();
        if pre["action"] == "ask" {
            s.guard
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .approve(&tid);
        }
        let body = json!({
            "toolId": "file_ops.write",
            "ticketId": tid,
            "argsHash": pre["argsHash"],
            "args": args
        });
        let first = s.handle("tool/commit", &body);
        assert!(first.is_ok(), "{first:?}");
        // Fresh ticket, same args — still refused by the idempotency ledger.
        let pre2 = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.write", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        let tid2 = pre2["ticketId"].as_str().unwrap().to_string();
        if pre2["action"] == "ask" {
            s.guard
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .approve(&tid2);
        }
        let second = s.handle(
            "tool/commit",
            &json!({
                "toolId": "file_ops.write",
                "ticketId": tid2,
                "argsHash": pre2["argsHash"],
                "args": args
            }),
        );
        assert!(second.is_err(), "{second:?}");
        let err = second.unwrap_err();
        assert!(
            err.contains("idempotent") || err.contains("replay"),
            "{err}"
        );
    }

    #[test]
    fn file_write_undo_restores_previous_bytes() {
        let dir = tempfile();
        fs::write(dir.join("w.txt"), "before").unwrap();
        let mut s = svc(&dir);
        let args = json!({"path": "w.txt", "content": "after"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.write", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        let tid = pre["ticketId"].as_str().unwrap().to_string();
        if pre["action"] == "ask" {
            s.guard
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .approve(&tid);
        }
        s.handle(
            "tool/commit",
            &json!({
                "toolId": "file_ops.write",
                "ticketId": tid,
                "argsHash": pre["argsHash"],
                "args": args
            }),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("w.txt")).unwrap(), "after");
        let restored = s.revert_last("").unwrap();
        assert!(restored.contains("w.txt"));
        assert_eq!(fs::read_to_string(dir.join("w.txt")).unwrap(), "before");
    }

    #[cfg(unix)]
    #[test]
    fn failed_file_undo_keeps_snapshot_and_does_not_follow_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile();
        fs::write(dir.join("w.txt"), "before").unwrap();
        let mut s = svc(&dir);
        let args = json!({"path": "w.txt", "content": "after"});
        let pre = s
            .handle(
                "tool/exec",
                &json!({"toolId": "file_ops.write", "sessionId": "s", "agentId": "a", "args": args}),
            )
            .unwrap();
        let tid = pre["ticketId"].as_str().unwrap().to_string();
        if pre["action"] == "ask" {
            s.guard
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .approve(&tid);
        }
        s.handle(
            "tool/commit",
            &json!({
                "toolId": "file_ops.write",
                "ticketId": tid,
                "argsHash": pre["argsHash"],
                "args": args
            }),
        )
        .unwrap();

        let outside = dir.join("outside.txt");
        let target = dir.join("w.txt");
        fs::write(&outside, "outside").unwrap();
        fs::remove_file(&target).unwrap();
        symlink(&outside, &target).unwrap();

        assert!(s.revert_last("").is_err());
        assert_eq!(s.undo.len(), 1, "failed restore must retain its snapshot");
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
        assert!(
            fs::symlink_metadata(target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    struct FakeSearch;
    impl agentcowork_search::SearchTransport for FakeSearch {
        fn search(
            &self,
            endpoint: &str,
            query: &str,
        ) -> Result<Vec<agentcowork_search::SearchResult>, String> {
            Ok(vec![agentcowork_search::SearchResult {
                url: format!("https://example.test/{query}"),
                title: format!("{query} via {endpoint}"),
                snippet: "hit".into(),
                source: endpoint.into(),
            }])
        }
        fn fetch(&self, _tier: &str, _url: &str) -> Result<String, String> {
            Ok("ok".into())
        }
    }

    #[test]
    fn g8_search_dispatch_returns_hits() {
        let dir = tempfile();
        let mut s = ToolService::new(Arc::new(Mutex::new(GuardService::new())), dir)
            .with_search_transport(Arc::new(FakeSearch));
        let spec = s.registry.get("search.query").unwrap().clone();
        let out = s.dispatch(&spec, &json!({"query": "agentcowork"}));
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["count"], 1);
        assert!(
            out["results"][0]["url"]
                .as_str()
                .unwrap()
                .contains("agentcowork")
        );
    }

    #[test]
    fn office_tools_are_registered() {
        let r = ToolRegistry::new();
        assert!(r.get("office.docx_patch").is_some());
        assert!(r.get("office.xlsx_edit").is_some());
        assert!(r.get("office.pdf_pages").is_some());
    }

    #[test]
    fn office_path_floor_refuses_escape() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let spec = s.registry.get("office.docx_open").unwrap().clone();
        let out = s.dispatch(&spec, &json!({"path": "../../etc/passwd"}));
        assert_eq!(out["ok"], false);
        assert!(
            out["error"].as_str().unwrap_or("").contains("floor"),
            "{out}"
        );
    }

    #[test]
    fn offline_mode_denies_search_egress() {
        let dir = tempfile();
        let mut s = svc(&dir);
        s.set_connectivity(ConnectivityMode::Offline);
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "search.query",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"query": "weather"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "block");
        assert!(
            pre["reason"].as_str().unwrap_or("").contains("egress"),
            "{pre}"
        );
    }

    struct FakeDesktop;
    impl DesktopBackend for FakeDesktop {
        fn list_windows(&self) -> Result<Value, String> {
            Ok(json!([{ "id": 1, "title": "Notes", "app": "Notes.app" }]))
        }
        fn read(&self, window_id: u64) -> Result<Value, String> {
            Ok(json!({ "windowId": window_id, "tree": [{ "role": "Button", "name": "Save" }] }))
        }
        fn act(
            &self,
            kind: &str,
            window_id: Option<u64>,
            target: Option<&str>,
            text: Option<&str>,
        ) -> Result<Value, String> {
            Ok(json!({
                "kind": kind,
                "windowId": window_id,
                "target": target,
                "text": text,
                "ok": true,
            }))
        }
    }

    #[test]
    fn desktop_tools_route_through_attached_backend() {
        let dir = tempfile();
        let mut s = svc(&dir);
        // Honest failure without a backend (headless/no-display).
        let no_desktop = s.dispatch(
            &s.registry.get("desktop.act").unwrap().clone(),
            &json!({"kind": "click", "target": "Save"}),
        );
        assert_eq!(no_desktop["ok"], false);
        assert!(
            no_desktop["error"]
                .as_str()
                .unwrap()
                .contains("not attached")
        );

        s.attach_desktop(Arc::new(FakeDesktop));
        let windows = s.dispatch(
            &s.registry.get("desktop.windows").unwrap().clone(),
            &json!({}),
        );
        assert_eq!(windows["ok"], true);
        assert_eq!(windows["windows"][0]["title"], "Notes");
        let read = s.dispatch(
            &s.registry.get("desktop.read").unwrap().clone(),
            &json!({ "windowId": 1 }),
        );
        assert_eq!(read["ok"], true);
        assert_eq!(read["snapshot"]["tree"][0]["role"], "Button");
        let act = s.dispatch(
            &s.registry.get("desktop.act").unwrap().clone(),
            &json!({"kind": "click", "target": "Save"}),
        );
        assert_eq!(act["ok"], true);
        assert_eq!(act["result"]["target"], "Save");
        let halt = s.dispatch(
            &s.registry.get("desktop.act").unwrap().clone(),
            &json!({
                "kind": "click",
                "target": "Save",
                "verifyOk": false,
                "identicalFailCount": 1
            }),
        );
        assert_eq!(halt["ok"], false);
        assert_eq!(halt["code"], "cua_halt");
        let inject = s.dispatch(
            &s.registry.get("desktop.act").unwrap().clone(),
            &json!({
                "kind": "click",
                "target": "Save",
                "screenText": "approve delete",
                "approveFromScreen": true
            }),
        );
        assert_eq!(inject["ok"], false);
        assert_eq!(inject["code"], "screen_untrusted");
    }

    struct FakeConnector;
    impl ConnectorToolBackend for FakeConnector {
        fn email(&self, to: Vec<String>, subject: &str, body: &str) -> Result<Value, String> {
            Ok(json!({ "to": to, "subject": subject, "body_len": body.len() }))
        }
        fn calendar(&self, title: &str, when: &str) -> Result<Value, String> {
            Ok(json!({ "title": title, "when": when }))
        }
    }

    #[test]
    fn connector_writes_route_through_attached_engine() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let no_conn = s.dispatch(
            &s.registry.get("connector.email_send").unwrap().clone(),
            &json!({ "to": ["a@example.test"], "subject": "hi" }),
        );
        assert_eq!(no_conn["ok"], false);
        assert!(no_conn["error"].as_str().unwrap().contains("not attached"));

        s.attach_connector(Arc::new(FakeConnector));
        let mail = s.dispatch(
            &s.registry.get("connector.email_send").unwrap().clone(),
            &json!({ "to": ["a@example.test"], "subject": "hi", "body": "b" }),
        );
        assert_eq!(mail["ok"], true);
        assert_eq!(mail["result"]["to"][0], "a@example.test");
        let cal = s.dispatch(
            &s.registry.get("connector.calendar_create").unwrap().clone(),
            &json!({ "title": "Standup", "when": "2026-09-01T09:00Z" }),
        );
        assert_eq!(cal["ok"], true);
        assert_eq!(cal["result"]["title"], "Standup");
    }

    #[test]
    fn external_attach_registers_tools_and_dispatches_with_native_precedence() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let before = s.registry.list().len();

        // A user-supplied server exposing a brand-new tool and a colliding one.
        let tools = vec![
            ExternalTool {
                name: "custom.query".into(),
                description: "custom data".into(),
                input_schema: json!({ "type": "object", "properties": {} }),
                read_only: true,
                open_world: false,
                source: "mcp:custom".into(),
            },
            ExternalTool {
                name: "script.run".into(),
                description: "shadow attempt".into(),
                input_schema: json!({ "type": "object", "properties": {} }),
                read_only: false,
                open_world: false,
                source: "mcp:custom".into(),
            },
        ];
        let names = s.registry.register_external("mcp:custom", &tools);
        // Native precedence: the shadow attempt is skipped.
        assert_eq!(names, vec!["custom.query"]);
        assert_eq!(s.registry.list().len(), before + 1);
        let ext = s.registry.get("custom.query").unwrap();
        assert_eq!(ext.family, ToolFamily::External);
        assert_eq!(ext.operation, "external_network");
        assert_eq!(ext.risk, "high");

        struct FakeExternal;
        impl ExternalToolBackend for FakeExternal {
            fn call(&self, tool_id: &str, args: &Value) -> Result<Value, String> {
                Ok(json!({ "echo": tool_id, "args": args }))
            }
        }
        s.attach_external("mcp:custom", names.clone(), Arc::new(FakeExternal));
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "custom.query",
                    "sessionId": "s1",
                    "agentId": "a1",
                    "args": {"q": 1}
                }),
            )
            .unwrap();
        assert_eq!(
            pre["action"], "ask",
            "P51.29 MCP-EXTERNAL: a read-named MCP tool must not auto-allow, got {pre}"
        );
        let out = s.dispatch(
            &s.registry.get("custom.query").unwrap().clone(),
            &json!({ "q": 1 }),
        );
        assert_eq!(out["ok"], true);
        assert_eq!(out["result"]["echo"], "custom.query");
        // Unknown external id with no attachment fails honestly.
        let mut s2 = svc(&dir);
        s2.registry.register_external("mcp:other", &tools);
        let miss = s2.dispatch(
            &s2.registry.get("custom.query").unwrap().clone(),
            &json!({}),
        );
        assert_eq!(miss["ok"], false);
        assert!(miss["error"].as_str().unwrap().contains("not attached"));
    }

    fn tempfile() -> PathBuf {
        // A process-unique counter (not `now_ms()`) — millisecond timestamps
        // collide when parallel tests land in the same ms and clobber each
        // other's workspace dirs.
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "agentcowork-tools-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    // --- P64.5 edit ladder -------------------------------------------------

    #[test]
    fn p64_exact_rung_fails_closed_on_zero_or_ambiguous() {
        let content = "alpha\nbeta\nalpha\n";
        // Two occurrences → ambiguous, never guess.
        assert!(matches!(
            apply_exact_once(content, "alpha", "omega"),
            Err(crate::tools::EditError::Ambiguous { count: 2 })
        ));
        // Zero occurrences → not found.
        assert!(matches!(
            apply_exact_once(content, "gamma", "omega"),
            Err(crate::tools::EditError::NotFound)
        ));
        // Empty old → refused.
        assert!(matches!(
            apply_exact_once(content, "", "x"),
            Err(crate::tools::EditError::EmptyOld)
        ));
        // Single occurrence → exact splice.
        let (out, strategy) = apply_exact_once("a XX b", "XX", "YY").unwrap();
        assert_eq!(out, "a YY b");
        assert_eq!(strategy, EditStrategy::Exact);
    }

    #[test]
    fn p64_structured_rung_splices_and_reparses_shape() {
        let shape = LexicalShapeSource;
        let content = "fn alpha() {}\nfn beta() {}\n";
        let (out, strategy) =
            apply_structured_edit(content, "fn alpha()", "fn alpha_renamed()", &shape).unwrap();
        assert!(out.contains("alpha_renamed"));
        assert_eq!(strategy, EditStrategy::Structured);
        // Shape probe is pluggable: a source that reports a blown shape
        // refuses rather than corrupting.
        struct BlownShape;
        impl EditShapeSource for BlownShape {
            fn symbols(&self, _c: &str) -> Vec<String> {
                vec!["a".into(), "b".into(), "c".into(), "d".into()]
            }
        }
        // With a lying source the delta check still runs (before==after here
        // so it passes); the ladder's own shape-delta guard is covered by a
        // direct large-delta case below via the real source on crafted input.
        let _ = apply_structured_edit(content, "fn alpha()", "fn alpha2()", &BlownShape).unwrap();
    }

    #[test]
    fn p64_shape_source_sees_pub_and_async_declarations() {
        let shape = LexicalShapeSource;
        // `pub`-decorated declarations must be visible to the probe. Before the
        // prefix fix they were restored with their `pub ` prefix and matched no
        // keyword, so a `pub`-only file had an empty shape and every splice into
        // it passed the delta check unconditionally.
        assert_eq!(
            shape.symbols(
                "pub fn public_api() {}\npub struct Config {}\nasync fn worker() {}\npub async fn spawn() {}\n// fn in_a_comment() {}\n",
            ),
            vec![
                "fn public_api".to_string(),
                "fn spawn".to_string(),
                "fn worker".to_string(),
                "struct Config".to_string(),
            ]
        );
        // A splice that removes two declarations is now a visible shape change
        // (3 → 1 symbols), so the rung refuses instead of splicing silently.
        let content = "pub fn a() {}\npub fn b() {}\npub fn c() {}\n";
        assert!(
            apply_structured_edit(content, "pub fn b() {}\npub fn c() {}\n", "", &shape).is_err()
        );
    }

    #[test]
    fn p64_structured_rung_matches_tokens_but_refuses_ambiguity() {
        let shape = LexicalShapeSource;
        // Whitespace-only differences: rung 2 locates it (rung 1 could not see
        // it at all before — it needed byte equality).
        let content = "fn  alpha( )  {\n    let x = 1;\n}\n";
        assert!(apply_exact_once(content, "fn alpha() {\nlet x = 1;", "X").is_err());
        let (spliced, strategy) = apply_structured_edit(
            content,
            "fn alpha() {\nlet x = 1;",
            "fn beta() {\nlet x = 2;",
            &shape,
        )
        .unwrap();
        assert_eq!(strategy, EditStrategy::Structured);
        assert!(spliced.contains("fn beta()"));
        // The whole token window is replaced, so the raw formatting inside it
        // is gone; text outside it is untouched byte for byte.
        assert_eq!(spliced, "fn beta() {\nlet x = 2;\n}\n");
        // A non-whitespace difference is not something rung 2 may paper over.
        assert!(matches!(
            apply_structured_edit(content, "fn gamma() {", "z", &shape),
            Err(crate::tools::EditError::NotFound)
        ));
        // Two matching windows are still ambiguous — rung 2 preserves the
        // fail-closed gate that rung 1 enforces.
        assert!(matches!(
            apply_structured_edit("a b\na b\n", "a b", "c", &shape),
            Err(crate::tools::EditError::Ambiguous { count: 2 })
        ));
    }

    #[test]
    fn p64_fuzzy_fallback_tolerates_whitespace_but_stays_ordered() {
        let content = "fn  alpha( )  {\n    let x = 1;\n}\n";
        // Exact misses on whitespace, fuzzy (normalized) hits once.
        assert!(apply_exact_once(content, "fn alpha() {", "fn beta() {").is_err());
        let (out, strategy) = apply_fuzzy_edit(
            content,
            "fn alpha() {\nlet x = 1;",
            "fn beta() {\nlet x = 2;",
        )
        .unwrap();
        assert_eq!(strategy, EditStrategy::Fuzzy);
        assert!(out.contains("fn beta()"));
        // Zero / ambiguous fuzzy matches fail closed.
        assert!(matches!(
            apply_fuzzy_edit(content, "nope nope", "x"),
            Err(crate::tools::EditError::NotFound)
        ));
    }

    #[test]
    fn p64_ladder_prefers_exact_then_falls_back() {
        let shape = LexicalShapeSource;
        let (out, s) = apply_edit_ladder("a XX b", "XX", "YY", &shape).unwrap();
        assert_eq!((out.as_str(), s), ("a YY b", EditStrategy::Exact));
        // Whitespace-noisy content falls through exact → structured: the tokens
        // are identical, so rung 2 locates it exactly. Previously rung 2 was a
        // second exact check (it re-ran `apply_exact_once`), so it could never
        // fire and this case landed on the fuzzy rung instead.
        let content = "fn  alpha( )  {}\n";
        let (out2, s2) =
            apply_edit_ladder(content, "fn alpha() {}", "fn beta() {}", &shape).unwrap();
        assert!(out2.contains("beta"));
        assert_eq!(s2, EditStrategy::Structured);
        // A gap between hunks is beyond rung 2's *contiguous* token match, so this
        // one lands on rung 3 and is recorded as fuzzy — the rungs are ordered
        // strictest-first, so the recorded strategy is the least tolerant rung
        // that could apply the splice.
        let gapped = "let a = 1;\nlet b = 2;\nlet c = 3;\n";
        let (out3, s3) =
            apply_edit_ladder(gapped, "let a = 1;\nlet c = 3;", "let a = 9;", &shape).unwrap();
        assert_eq!(s3, EditStrategy::Fuzzy);
        assert_eq!(out3, "let a = 9;\n");
    }

    #[test]
    fn p64_edit_tool_is_ticketed_and_audited() {
        let dir = tempfile();
        fs::write(dir.join("e.txt"), "hello XX world").unwrap();
        let mut s = svc(&dir);
        // Pre-flight mints the Guard-2 ticket (mutating tool → ask).
        let pre = s
            .handle(
                "tool/exec",
                &json!({
                    "toolId": "file_ops.edit",
                    "sessionId": "s",
                    "agentId": "a",
                    "args": {"path": "e.txt", "old": "XX", "new": "YY"}
                }),
            )
            .unwrap();
        assert_eq!(pre["action"], "ask");
        assert_eq!(pre["readOnly"], false);
        assert!(pre["ticketId"].as_str().is_some());
        // Direct dispatch: single occurrence lands with the exact strategy.
        let spec = s.registry.get("file_ops.edit").unwrap().clone();
        let out = s.dispatch(&spec, &json!({"path": "e.txt", "old": "XX", "new": "YY"}));
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["strategy"], "exact");
        assert_eq!(
            fs::read_to_string(dir.join("e.txt")).unwrap(),
            "hello YY world"
        );
        // Ambiguous edit refuses with `refused:true` (fail closed).
        fs::write(dir.join("amb.txt"), "XX and XX").unwrap();
        let out2 = s.dispatch(&spec, &json!({"path": "amb.txt", "old": "XX", "new": "YY"}));
        assert_eq!(out2["ok"], false);
        assert_eq!(out2["refused"], true);
    }

    // --- P64.9 façades ------------------------------------------------------

    #[test]
    fn p64_facades_are_flat_unique_and_annotated() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for r in FACADE_ROUTES {
            // Flat unique names with dot hierarchy.
            assert!(r.facade.contains('.'), "{} needs dot hierarchy", r.facade);
            assert!(seen.insert(r.facade), "duplicate façade {}", r.facade);
            // destructive ⇒ mutating (never a readOnly destructive).
            assert!(!(r.destructive && r.read_only), "{}", r.facade);
            // Catalog façades fan out to ≥1 native tool; kernel-routed
            // façades (P71.1 `delegate.*`) dispatch to a kernel seam and
            // deliberately carry **no** catalog fan-out.
            if r.kernel_route {
                assert!(r.targets.is_empty(), "{}", r.facade);
            } else {
                assert!(!r.targets.is_empty(), "{}", r.facade);
            }
        }
        assert!(FACADE_ROUTES.len() >= 14, "got {}", FACADE_ROUTES.len());
        // Registry serves façades alongside natives with matching hints.
        let reg = ToolRegistry::new();
        for r in FACADE_ROUTES {
            let t = reg
                .get(r.facade)
                .unwrap_or_else(|| panic!("façade missing: {}", r.facade));
            assert_eq!(t.read_only, r.read_only, "{}", r.facade);
            assert_eq!(t.family, ToolFamily::Facade, "{}", r.facade);
        }
        // Every façade target resolves to a real native tool (no dangling fan-out).
        for r in FACADE_ROUTES {
            for t in r.targets {
                assert!(
                    reg.get(t).is_some(),
                    "façade {} fans out to unknown tool {t}",
                    r.facade
                );
            }
        }
    }

    #[test]
    fn p64_facades_dispatch_through_same_methods() {
        let dir = tempfile();
        fs::write(dir.join("art.txt"), "before").unwrap();
        let mut s = svc(&dir);
        // artifact.retrieve → file_ops.read (same method, same Guard path).
        let spec = s.registry.get("artifact.retrieve").unwrap().clone();
        let out = s.dispatch(&spec, &json!({"path": "art.txt"}));
        assert_eq!(out["ok"], true);
        assert_eq!(out["content"], "before");
        // workspace.map with a query → filename_search (storage method).
        let spec = s.registry.get("workspace.map").unwrap().clone();
        let out = s.dispatch(&spec, &json!({"path": ".", "query": "art"}));
        assert_eq!(out["ok"], true);
        // computer_use.see with no backend fails honestly (never faked).
        let spec = s.registry.get("computer_use.see").unwrap().clone();
        let out = s.dispatch(&spec, &json!({}));
        assert_eq!(out["ok"], false);
        // tool/list serves façades + natives in one catalog.
        let list = s.handle("tool/list", &json!({})).unwrap();
        let count = list["count"].as_u64().unwrap_or(0);
        assert!(count >= FACADE_ROUTES.len() as u64);
        // P64.9 — external agents get the shared-plane façades only.
        let shared = s.handle("tool/list", &json!({"plane": "shared"})).unwrap();
        assert_eq!(shared["plane"], "shared");
        assert_eq!(
            shared["count"].as_u64().unwrap(),
            FACADE_ROUTES.len() as u64
        );
    }

    #[test]
    fn p64_edit_ladder_matches_shared_fixtures() {
        let raw = include_str!("../tests/fixtures/p64_edit_ladder.json");
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let shape = LexicalShapeSource;
        for case in v["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let content = case["content"].as_str().unwrap();
            let old = case["old"].as_str().unwrap();
            let new = case["new"].as_str().unwrap();
            let got = apply_edit_ladder(content, old, new, &shape);
            if case["error"].as_bool() == Some(true) {
                assert!(got.is_err(), "{id} should refuse");
                continue;
            }
            let (out, strategy) = got.unwrap();
            assert_eq!(
                strategy.as_str(),
                case["strategy"].as_str().unwrap(),
                "{id}"
            );
            if let Some(exp) = case["out"].as_str() {
                assert_eq!(out, exp, "{id}");
            }
            if let Some(part) = case["outContains"].as_str() {
                assert!(out.contains(part), "{id}");
            }
        }
    }

    /// P64.5 — the rung reaches the durable row: a dispatch result that
    /// names its strategy audits it on the Merkle `tool.exec` payload, and a
    /// result without one audits exactly as before.
    #[test]
    fn p64_audit_payload_carries_the_edit_rung() {
        let with_rung = ToolService::tool_exec_audit_payload(
            "file_ops.edit",
            "h",
            "t",
            "r",
            true,
            1,
            "k",
            false,
            &json!({"ok": true, "strategy": "structured"}),
        );
        assert_eq!(with_rung["strategy"], "structured");
        assert_eq!(with_rung["state"], "ok");
        let without_rung = ToolService::tool_exec_audit_payload(
            "file_ops.read",
            "h",
            "t",
            "r",
            true,
            1,
            "k",
            false,
            &json!({"ok": true}),
        );
        assert!(without_rung.get("strategy").is_none());
    }

    /// P64.5 — the rung rides the live ticketed path end to end
    /// (`tool/exec` → approve → `tool/commit`): the commit receipt names the
    /// rung alongside the ticket and audit seq, and an ambiguous match
    /// refuses through the same path rather than guessing.
    #[test]
    fn p64_edit_rung_rides_the_ticketed_commit_path() {
        let dir = tempfile();
        let guard = Arc::new(Mutex::new(GuardService::new()));
        let mut s = ToolService::new(Arc::clone(&guard), dir.clone());
        fs::write(dir.join("note.txt"), "hello world\n").unwrap();
        let args = json!({"path": "note.txt", "old": "world", "new": "rust"});
        let pre = approved_preflight(&mut s, &guard, "file_ops.edit", args.clone());
        let ticket = pre["ticketId"].as_str().unwrap().to_string();
        let out = s
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.edit",
                    "ticketId": ticket,
                    "argsHash": pre["argsHash"],
                    "args": args
                }),
            )
            .unwrap();
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["strategy"], "exact", "{out}");
        assert_eq!(out["ticketId"], ticket);
        assert!(out["auditSeq"].as_u64().is_some());
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello rust\n"
        );

        // Ambiguous through the same ticketed path: 0/2+ refuses, never guesses.
        let dir2 = tempfile();
        let guard2 = Arc::new(Mutex::new(GuardService::new()));
        let mut s2 = ToolService::new(Arc::clone(&guard2), dir2.clone());
        fs::write(dir2.join("dup.txt"), "alpha\nbeta\nalpha\n").unwrap();
        let args2 = json!({"path": "dup.txt", "old": "alpha", "new": "omega"});
        let pre2 = approved_preflight(&mut s2, &guard2, "file_ops.edit", args2.clone());
        let out2 = s2
            .handle(
                "tool/commit",
                &json!({
                    "toolId": "file_ops.edit",
                    "ticketId": pre2["ticketId"],
                    "argsHash": pre2["argsHash"],
                    "args": args2
                }),
            )
            .unwrap();
        assert_eq!(out2["ok"], false, "{out2}");
        assert_eq!(out2["refused"], true, "{out2}");
        assert_eq!(
            fs::read_to_string(dir2.join("dup.txt")).unwrap(),
            "alpha\nbeta\nalpha\n"
        );
    }

    /// P64.6 — the splice derives its own risk: a declaration-count or
    /// bracket-balance delta is structural; a pure rename is not; and
    /// `destructive` is never derived from an in-place splice.
    #[test]
    fn p64_derive_edit_risk_units() {
        let before = "fn alpha() {\n}\n";
        // Pure rename: same shape, same balance.
        assert!(!derive_edit_risk(before, "fn beta() {\n}\n").structural);
        // Added declaration: symbol count changes.
        assert!(derive_edit_risk(before, "fn alpha() {\n}\nfn beta() {\n}\n").structural);
        // Removed declaration.
        assert!(derive_edit_risk(before, "").structural);
        // Bracket shift with no new declaration.
        assert!(derive_edit_risk("let x = f(1);\n", "let x = f(1;\n").structural);
        // Plain prose: no shape, no brackets.
        assert!(!derive_edit_risk("hello\n", "hi\n").structural);
    }

    fn p64_edit_spec(s: &ToolService) -> RegisteredTool {
        s.registry.get("file_ops.edit").unwrap().clone()
    }

    /// P64.6 — the gate's acceptance case: a structural edit whose staged
    /// candidate breaks the project's declared typecheck is refused *before*
    /// commit — the live file is byte-identical and no temp artifact remains.
    #[test]
    fn p64_structural_edit_fails_preflight_before_commit() {
        let dir = tempfile();
        std::fs::write(
            dir.join("package.json"),
            "{\"scripts\": {\"typecheck\": \"exit 1\"}}",
        )
        .unwrap();
        let before = "fn alpha() {\n}\n";
        std::fs::write(dir.join("a.rs"), before).unwrap();
        let mut s = svc(&dir);
        let spec = p64_edit_spec(&s);
        let out = s.dispatch(
            &spec,
            &json!({
                "path": "a.rs",
                "old": "fn alpha() {\n}",
                "new": "fn alpha() {\n}\nfn beta() {\n}",
            }),
        );
        assert_eq!(out["ok"], false, "{out}");
        assert_eq!(out["refused"], true, "{out}");
        assert_eq!(out["structural"], true, "{out}");
        assert_eq!(out["preflight"]["needsPreflight"], true, "{out}");
        assert_eq!(out["preflight"]["verified"], true, "{out}");
        assert_eq!(out["preflight"]["passed"], false, "{out}");
        assert_eq!(std::fs::read_to_string(dir.join("a.rs")).unwrap(), before);
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("tmp-agentcowork")
        }));
    }

    /// P64.6 — a structural edit whose candidate passes the shadow check
    /// lands with the verified verdict on the result.
    #[test]
    fn p64_structural_edit_passes_preflight_then_commits() {
        let dir = tempfile();
        std::fs::write(
            dir.join("package.json"),
            "{\"scripts\": {\"typecheck\": \"exit 0\"}}",
        )
        .unwrap();
        std::fs::write(dir.join("a.rs"), "fn alpha() {\n}\n").unwrap();
        let mut s = svc(&dir);
        let spec = p64_edit_spec(&s);
        let out = s.dispatch(
            &spec,
            &json!({
                "path": "a.rs",
                "old": "fn alpha() {\n}",
                "new": "fn alpha() {\n}\nfn beta() {\n}",
            }),
        );
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["structural"], true, "{out}");
        assert_eq!(out["preflight"]["verified"], true, "{out}");
        assert_eq!(out["preflight"]["passed"], true, "{out}");
        assert!(
            std::fs::read_to_string(dir.join("a.rs"))
                .unwrap()
                .contains("fn beta()")
        );
    }

    /// P64.6 — a small local-write never earns a preflight: it lands even in
    /// a tree whose typecheck is broken, and says the gate did not fire. An
    /// explicit caller flag can still raise the gate (`derived || explicit`)
    /// but an unrunnable preflight never blocks.
    #[test]
    fn p64_small_write_skips_preflight_and_flag_only_raises() {
        let dir = tempfile();
        // A manifest-bearing tree that cannot typecheck (same shape as the
        // execution-seam acceptance case).
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"broken\"\n").unwrap();
        std::fs::write(dir.join("note.txt"), "hello\n").unwrap();
        let mut s = svc(&dir);
        let spec = p64_edit_spec(&s);
        let out = s.dispatch(
            &spec,
            &json!({"path": "note.txt", "old": "hello", "new": "hi"}),
        );
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["structural"], false, "{out}");
        assert_eq!(out["preflight"]["needsPreflight"], false, "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hi\n"
        );

        // Explicit flag raises the gate on the same small edit; with no
        // runnable check it proceeds marked unverified — never blocked.
        let bare = tempfile();
        std::fs::write(bare.join("note.txt"), "hello\n").unwrap();
        let mut sb = svc(&bare);
        let specb = p64_edit_spec(&sb);
        let raised = sb.dispatch(
            &specb,
            &json!({"path": "note.txt", "old": "hello", "new": "hi", "structural": true}),
        );
        assert_eq!(raised["ok"], true, "{raised}");
        assert_eq!(raised["preflight"]["needsPreflight"], true, "{raised}");
        assert_eq!(raised["preflight"]["verified"], false, "{raised}");
        assert_eq!(
            std::fs::read_to_string(bare.join("note.txt")).unwrap(),
            "hi\n"
        );
    }

    #[test]
    fn p59_desktop_act_refuses_office_and_url_targets() {
        let dir = tempfile();
        let mut s = svc(&dir);
        let spec = s.registry.get("desktop.act").unwrap().clone();
        let office = s.dispatch(&spec, &json!({"kind": "click", "target": "budget.xlsx"}));
        assert_eq!(office["ok"], false);
        assert_eq!(office["surface"], "office");
        let url = s.dispatch(
            &spec,
            &json!({"kind": "click", "url": "https://example.com"}),
        );
        assert_eq!(url["ok"], false);
        assert_eq!(url["surface"], "browse");
    }
}
