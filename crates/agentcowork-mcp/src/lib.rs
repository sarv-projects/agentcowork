//! agentcowork-mcp — MCP server exposing the browser + connector tools
//! (ARCH/08 §8.2/§8.6, F6/F7).
//!
//! P2.3 scope: the **37-tool catalog** — the 17 BrowserOS-compatible core
//! tools + `enhanced_snapshot` + bookmarks×6 + tab-groups×5 + windows×5
//! (34 total per ARCH/08 §8.2) + `file_ops`×3 workspace extension (E2).
//! Each tool carries the ACP tool-kind taxonomy (F9, doc 45 §4.3), MCP
//! readOnlyHint/openWorldHint annotations, a **tool profile** (doc 55
//! mcp.rs: core/network/state/debug/tabs/mobile), typed argument schemas,
//! and **extraArgs parity** (doc 55 — arbitrary extra args forwarded to the
//! action engine). Paginated discovery (doc 55) is implemented here;
//! P6.7 builds the actual server over the official rust-sdk.

use serde::{Deserialize, Serialize};

pub mod attach;
pub mod hijack;
pub mod loopback;
pub mod manager;
pub mod npx;
pub mod preview;
pub mod protocol;
pub mod record;
pub mod remote;
pub mod server;
pub mod store;

pub use attach::{AttachError, AttachRequest, AttachedServer, sanitize_attach_name};
pub use hijack::{HijackError, ToolIdentity, ToolSource, validate_external_tool};
pub use loopback::{LoopbackPool, PoolStats};
pub use manager::{
    ALLOW_LIST, ChildHandle, InstallPlan, ManagedServer, McpServerManager, PlanError,
    ProcessSpawner, RegistryIndex, RegistryServer, ServerSpawner, ServerState, SpawnError,
    ToolSurface, install_plan, is_allowed, merge_into_catalog, verify_sha256,
};
pub use npx::{
    NpxError, NpxSource, ResolvedLaunch, npx_package_from_args, resolve_stdio_launch,
    resolve_stdio_launch_with, trusted_npx_package,
};
pub use preview::{
    ARTIFACT_URI_SCHEME, BoundedPreview, DEFAULT_PREVIEW_BYTES, artifact_ref, bounded_preview,
    default_preview,
};
pub use remote::{
    AuthServerMetadata, ClientRegistration, ConnectOptions, EraCache, EraNegotiation, EraSource,
    EraVerdict, HttpTransport, LEGACY_PROTOCOL_VERSION, MODERN_PROTOCOL_VERSION, McpEra,
    McpResponse, PROBE_BUDGET, PkceFlow, ProtectedResource, RemoteError, RemoteTarget,
    STDIO_ERA_KEY_PREFIX, StdioEraProbe, TokenResponse, UreqTransport, build_authorize_url,
    build_discover_request, build_request, cache_era, cached_era, classify_era, classify_era_body,
    clear_era_cache, connect, connect_with_options, discover_authorization_server,
    discover_probe_headers, discover_protected_resource, exchange_code, modern_headers,
    negotiate_era, negotiate_era_detailed, negotiate_stdio_era, origin_of, refresh_token,
    register_dynamic_client, rpc, rpc_in_era, stdio_era_key, tool_name,
};
pub use server::{
    DISCOVER_METHOD_NAME, DiscoverCapabilities, DiscoverToolCapability, ExternalTool, FacadeError,
    McpHttpLease, McpHttpListener, McpServer, McpServerLease, MrtrHandle,
    SUPPORTED_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS, ServerDiscoverResponse, ServerInfo,
    StatelessRequest, ToolAdmission, ToolCallError, ToolCallErrorKind, ToolCallHandler,
    ToolCatalog, ToolListEntry, ToolListResponse, server_discover, start_http_listener, tool_list,
    tool_list_shared_facades, tool_list_shared_plane,
};
pub use store::{ConnectConsent, ConnectFlow, StoreEntry, StoreIndex, StoreKind};

/// The shared wire constants for the dual-era MCP contract (DEC-030). Re-exported
/// so a client and this façade cannot drift on a revision or a header name.
pub use protocol::{
    DISCOVER_METHOD, LEGACY_PROTOCOL_REVISION, METHOD_HEADER, MODERN_PROTOCOL_REVISION,
    NAME_HEADER, PROTOCOL_VERSION_HEADER,
};

/// ACP tool-kind taxonomy (F9 — doc 45 §4.3): a shared vocabulary that maps
/// onto our F9 permission classes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

/// Tool profiles (doc 55 mcp.rs): each profile is a curated tool subset a
/// client can request instead of the full catalog.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolProfile {
    Core,
    Network,
    State,
    Debug,
    Tabs,
    React,
    Mobile,
    /// Everything.
    All,
}

/// One typed argument of a tool (doc 55: typed args + extraArgs parity).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArgDef {
    pub name: &'static str,
    pub kind: ArgKind,
    pub required: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArgKind {
    String,
    Number,
    Bool,
    StringArray,
    Object,
}

impl ArgDef {
    pub const fn new(
        name: &'static str,
        kind: ArgKind,
        required: bool,
        description: &'static str,
    ) -> Self {
        Self {
            name,
            kind,
            required,
            description,
        }
    }
}

/// One registered tool.
///
/// Note: not `Deserialize` — the catalog is a static registry; serialization
/// (for MCP discovery responses) uses `Serialize` only.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolDef {
    pub name: &'static str,
    pub kind: ToolKind,
    /// readOnlyHint annotation (MCP): true = never mutates.
    pub read_only: bool,
    /// openWorldHint annotation: true = may reach outside the workspace
    /// (run/evaluate are open-world and always permission-checked).
    pub open_world: bool,
    pub profile: ToolProfile,
    pub description: &'static str,
    pub args: &'static [ArgDef],
}

impl ToolDef {
    pub const fn new(
        name: &'static str,
        kind: ToolKind,
        read_only: bool,
        open_world: bool,
        profile: ToolProfile,
        description: &'static str,
        args: &'static [ArgDef],
    ) -> Self {
        Self {
            name,
            kind,
            read_only,
            open_world,
            profile,
            description,
            args,
        }
    }
}

// ---------------------------------------------------------------------------
// The 37-tool catalog
// ---------------------------------------------------------------------------

macro_rules! tools {
    ($( $name:literal, $kind:ident, $ro:literal, $ow:literal, $profile:ident, $desc:literal, $args:expr ),* $(,)?) => {
        &[
            $( ToolDef::new($name, ToolKind::$kind, $ro, $ow, ToolProfile::$profile, $desc, $args) ),*
        ]
    };
}

const STR_URL: ArgDef = ArgDef::new("url", ArgKind::String, true, "Page URL to navigate to");
const STR_REF: ArgDef = ArgDef::new(
    "ref_id",
    ArgKind::String,
    true,
    "Snapshot ref [ref=eN] to act on",
);
const STR_TEXT: ArgDef = ArgDef::new("text", ArgKind::String, false, "Text to type / wait for");
const STR_KEY: ArgDef = ArgDef::new(
    "key",
    ArgKind::String,
    true,
    "Keyboard key (Enter, Tab, Escape…)",
);
const STR_VALUE: ArgDef = ArgDef::new("value", ArgKind::String, false, "Select option value");
const STR_PATTERN: ArgDef = ArgDef::new("pattern", ArgKind::String, true, "Regex pattern");
const NUM_X: ArgDef = ArgDef::new("x", ArgKind::Number, true, "Viewport x (CSS px)");
const NUM_Y: ArgDef = ArgDef::new("y", ArgKind::Number, true, "Viewport y (CSS px)");
const NUM_MS: ArgDef = ArgDef::new("ms", ArgKind::Number, false, "Milliseconds to wait");
const NUM_QUALITY: ArgDef = ArgDef::new(
    "quality",
    ArgKind::Number,
    false,
    "JPEG quality 0-100 (default 80)",
);
const STR_SELECTOR: ArgDef = ArgDef::new("selector", ArgKind::String, true, "CSS selector");
const STR_EXPR: ArgDef = ArgDef::new(
    "expression",
    ArgKind::String,
    true,
    "JS expression to evaluate",
);
const STR_TITLE: ArgDef = ArgDef::new("title", ArgKind::String, true, "Bookmark title");
const STR_DIR: ArgDef = ArgDef::new("dir", ArgKind::String, false, "Download directory");
const ARR_FIELDS: ArgDef = ArgDef::new(
    "fields",
    ArgKind::Object,
    false,
    "Form fields [{ref_id, value}]",
);
const ARR_FILES: ArgDef = ArgDef::new("files", ArgKind::StringArray, true, "File paths to upload");
const STR_NAME: ArgDef = ArgDef::new("name", ArgKind::String, false, "Tab/group/window name");
const STR_ID: ArgDef = ArgDef::new("id", ArgKind::String, false, "Tab/group/window id");
const BOOL_HIDDEN: ArgDef = ArgDef::new("hidden", ArgKind::Bool, false, "Create hidden/background");
const STR_FILTER: ArgDef = ArgDef::new(
    "filter",
    ArgKind::String,
    false,
    "Keep lines matching regex",
);
const BOOL_OUTLINE: ArgDef = ArgDef::new("outline", ArgKind::Bool, false, "Headings + links only");
const BOOL_RAW: ArgDef = ArgDef::new("raw", ArgKind::Bool, false, "Raw text, no markdown syntax");
const STR_REF2: ArgDef = ArgDef::new("to_ref", ArgKind::String, false, "Drag target ref");
const STR_PATH: ArgDef = ArgDef::new("path", ArgKind::String, true, "Directory path to scan");
const STR_QUERY: ArgDef = ArgDef::new("query", ArgKind::String, true, "Filename search query");
const NUM_TOP_N: ArgDef = ArgDef::new(
    "top_n",
    ArgKind::Number,
    false,
    "Number of results (default 50)",
);

/// The 37-tool catalog (ARCH/08 §8.2: 34 + file_ops×3). Ordering: the
/// original 17 BrowserOS-semantic tools first (prompts/skills transfer),
/// then enhanced_snapshot, bookmarks×6, tab-groups×5, windows×5, file_ops×3.
pub const BROWSER_TOOLS: &[ToolDef] = tools!(
    "tabs",
    Read,
    true,
    false,
    Tabs,
    "List open tabs/targets",
    &[],
    "tab_groups",
    Read,
    true,
    false,
    Tabs,
    "List tab groups (requires fork/extension surface)",
    &[],
    "history",
    Read,
    true,
    false,
    State,
    "Page navigation history",
    &[],
    "navigate",
    Edit,
    false,
    false,
    Core,
    "Goto / back / forward / reload",
    &[STR_URL],
    "snapshot",
    Read,
    true,
    false,
    State,
    "Accessibility snapshot with [ref=eN]",
    &[],
    "diff",
    Read,
    true,
    false,
    State,
    "Line-diff of two snapshots",
    &[],
    "act",
    Edit,
    false,
    false,
    Core,
    "Input: click/type/fill/press/hover/select/scroll/drag/dialog",
    &[
        STR_REF, STR_TEXT, STR_KEY, STR_VALUE, ARR_FIELDS, NUM_X, NUM_Y, STR_REF2
    ],
    "download",
    Edit,
    false,
    false,
    Network,
    "Set download path / trigger download",
    &[STR_DIR],
    "upload",
    Edit,
    false,
    false,
    Network,
    "Set file input files by ref",
    &[STR_REF, ARR_FILES],
    "read",
    Read,
    true,
    false,
    Network,
    "Page → markdown (DOM walker / markdown negotiation)",
    &[STR_FILTER, BOOL_OUTLINE, BOOL_RAW],
    "grep",
    Search,
    true,
    false,
    Core,
    "Line matches in page text",
    &[STR_PATTERN],
    "screenshot",
    Read,
    true,
    false,
    Core,
    "JPEG screenshot (base64)",
    &[NUM_QUALITY],
    "pdf",
    Read,
    true,
    false,
    Core,
    "Print page to PDF (base64)",
    &[],
    "wait",
    Other,
    false,
    false,
    Core,
    "Wait for text/selector or ms",
    &[STR_TEXT, STR_SELECTOR, NUM_MS],
    "windows",
    Read,
    true,
    false,
    Tabs,
    "List browser windows",
    &[],
    "evaluate",
    Execute,
    false,
    true,
    Debug,
    "CDP Runtime.evaluate",
    &[STR_EXPR],
    "run",
    Execute,
    false,
    true,
    Debug,
    "Think-in-code script (P2.5 agentcowork-script)",
    &[STR_EXPR],
    "enhanced_snapshot",
    Read,
    true,
    false,
    State,
    "Snapshot + paint-order occlusion filter",
    &[],
    // bookmarks ×6 — Chrome CDP has no bookmarks domain; these need the
    // fork/extension surface (BrowserOS ships them in the Chromium fork).
    "get_bookmarks",
    Read,
    true,
    false,
    Core,
    "List bookmarks",
    &[],
    "create_bookmark",
    Edit,
    false,
    false,
    Core,
    "Create bookmark",
    &[STR_TITLE, STR_URL],
    "remove_bookmark",
    Delete,
    false,
    false,
    Core,
    "Remove bookmark",
    &[STR_ID],
    "update_bookmark",
    Edit,
    false,
    false,
    Core,
    "Update bookmark",
    &[STR_ID, STR_TITLE, STR_URL],
    "move_bookmark",
    Move,
    false,
    false,
    Core,
    "Move bookmark",
    &[STR_ID, STR_ID],
    "search_bookmarks",
    Search,
    true,
    false,
    Core,
    "Search bookmarks",
    &[STR_TEXT],
    // tab-groups ×5 — no CDP surface on stock Chrome (fork/extension needed).
    "list_tab_groups",
    Read,
    true,
    false,
    Tabs,
    "List tab groups",
    &[],
    "group_tabs",
    Edit,
    false,
    false,
    Tabs,
    "Group tabs",
    &[STR_ID, STR_NAME],
    "update_tab_group",
    Edit,
    false,
    false,
    Tabs,
    "Update tab group",
    &[STR_ID, STR_NAME],
    "ungroup_tabs",
    Edit,
    false,
    false,
    Tabs,
    "Ungroup tabs",
    &[STR_ID],
    "close_tab_group",
    Delete,
    false,
    false,
    Tabs,
    "Close tab group",
    &[STR_ID],
    // windows ×5 — CDP Target/Browser domains.
    "list_windows",
    Read,
    true,
    false,
    Tabs,
    "List windows (targets grouped by context)",
    &[],
    "create_window",
    Edit,
    false,
    false,
    Tabs,
    "Create a new window",
    &[BOOL_HIDDEN],
    "create_hidden_window",
    Edit,
    false,
    false,
    Tabs,
    "Create a hidden background window",
    &[],
    "close_window",
    Delete,
    false,
    false,
    Tabs,
    "Close a window by context id",
    &[STR_ID],
    "activate_window",
    Edit,
    false,
    false,
    Tabs,
    "Activate/focus a window",
    &[STR_ID],
    // file_ops ×3 — OutputFileAccess routing (E2 extension).
    "save_pdf_enhanced",
    Read,
    true,
    false,
    Core,
    "Print to PDF and route to file",
    &[STR_DIR],
    "save_screenshot_enhanced",
    Read,
    true,
    false,
    Core,
    "JPEG screenshot routed to file",
    &[STR_DIR, NUM_QUALITY],
    "download_file",
    Edit,
    false,
    false,
    Network,
    "Download file to temp dir",
    &[STR_URL, STR_DIR],
);

/// The storage-intelligence tool catalog (P4.8 — D9–D11, G7): the
/// `agentcowork-storage` primitives exposed as agent tools. All are **read-only
/// proposals** — the crate never deletes; cleanup goes through Guard-2.
/// Heavy scans respect J16 battery-awareness (the caller gates them).
pub const STORAGE_TOOLS: &[ToolDef] = tools!(
    "disk_scan",
    Read,
    true,
    false,
    State,
    "Scan a directory tree into an indexed arena (parallel work-stealing walker; battery-aware J16)",
    &[STR_PATH],
    "disk_duplicates",
    Search,
    true,
    false,
    State,
    "Find duplicate files (7-stage hash: size → xxHash3 → BLAKE3, hardlink-aware)",
    &[STR_PATH],
    "disk_large_files",
    Search,
    true,
    false,
    State,
    "Find largest files by size/age",
    &[STR_PATH, NUM_TOP_N],
    "disk_cleanup",
    Read,
    true,
    false,
    State,
    "Propose Guard-2-ticketed cleanup (recycle-bin-aware; NEVER deletes — proposal only)",
    &[STR_PATH],
    "filename_search",
    Search,
    true,
    false,
    State,
    "FTS5 filename search",
    &[STR_QUERY],
);

const STR_EDIT: ArgDef = ArgDef::new(
    "edit",
    ArgKind::Object,
    false,
    "Block-patch edit {path, blocks[]} (Guard-2 ticketed)",
);
const STR_MEMORY_QUERY: ArgDef = ArgDef::new(
    "query",
    ArgKind::String,
    true,
    "Memory retrieval query (multi-signal fusion)",
);
const STR_MEMORY_TEXT: ArgDef = ArgDef::new(
    "text",
    ArgKind::String,
    false,
    "Fact/text to store (taste-classified)",
);
const STR_SOURCES: ArgDef = ArgDef::new(
    "sources",
    ArgKind::StringArray,
    true,
    "Research sources (files/URLs/emails)",
);

/// Office surgical-editor tools (Channel B — `agentcowork-office` D1 block-
/// patch engine exposed over MCP; byte-stable OOXML, never lossy
/// re-serialize). Edits are Guard-2 ticketed at the protocol boundary.
pub const OFFICE_TOOLS: &[ToolDef] = tools!(
    "office_open",
    Read,
    true,
    false,
    State,
    "Open a docx/pptx/xlsx into the surgical editor (byte-stable open)",
    &[STR_PATH],
    "office_edit",
    Edit,
    false,
    false,
    State,
    "Apply a block-patch edit (Guard-2 ticket + diff card)",
    &[STR_PATH, STR_EDIT],
    "office_undo",
    Edit,
    false,
    false,
    State,
    "Roll back to the pre-edit snapshot (D7 one-click undo)",
    &[STR_PATH],
    "office_export",
    Read,
    true,
    false,
    State,
    "Export a docx as pdf (LibreOffice conformance oracle)",
    &[STR_PATH],
);

/// Memory tools (Channel B — `agentcowork-memory` C-series retrieval as MCP
/// tools): any MCP-consuming agent gets the memory fusion surface.
pub const MEMORY_TOOLS: &[ToolDef] = tools!(
    "memory_retrieve",
    Search,
    true,
    false,
    State,
    "Multi-signal memory retrieval (FTS5 + vector + graph RRF fusion)",
    &[STR_MEMORY_QUERY],
    "memory_store",
    Edit,
    false,
    false,
    State,
    "Store a fact/memory (taste-classified, deduped)",
    &[STR_MEMORY_TEXT],
    "memory_review_due",
    Read,
    true,
    false,
    State,
    "Due reinforcement reviews (FSRS queue)",
    &[],
);

/// Search/research tools (Channel B — `agentcowork-search` G8 cascade + G2
/// deep research over MCP).
#[rustfmt::skip]
pub const SEARCH_TOOLS: &[ToolDef] = tools!(
    "search_web",
    Fetch,
    true,
    true,
    Network,
    "G8 search cascade (parallel fetch, BM25 rerank)",
    &[STR_QUERY],
    "deep_research",
    Think,
    true,
    true,
    Network,
    "G2 deep research over selected sources (grounded, cited)",
    &[STR_SOURCES, STR_QUERY],
);

/// Channel B (doc 68 §4): every MCP-consuming agent gets the full inbuilt
/// capability set — browser 37 + office + memory + search + storage.
pub const INBUILT_TOOLS: &[&[ToolDef]] = &[
    BROWSER_TOOLS,
    OFFICE_TOOLS,
    MEMORY_TOOLS,
    SEARCH_TOOLS,
    STORAGE_TOOLS,
];

/// The unified agent tool registry: browser (37) + office (4) + memory (3)
/// + search (2) + storage (5). This is what the P6.x tool-catalog
///   reconciliation exposes to the agent loop.
pub fn all_tools() -> Vec<&'static ToolDef> {
    INBUILT_TOOLS.iter().flat_map(|cat| cat.iter()).collect()
}

/// The inbuilt catalog grouped by surface (Channel B discovery response).
pub fn inbuilt_catalog() -> Vec<(&'static str, Vec<&'static ToolDef>)> {
    vec![
        ("browser", BROWSER_TOOLS.iter().collect()),
        ("office", OFFICE_TOOLS.iter().collect()),
        ("memory", MEMORY_TOOLS.iter().collect()),
        ("search", SEARCH_TOOLS.iter().collect()),
        ("storage", STORAGE_TOOLS.iter().collect()),
    ]
}

/// Look up a browser tool by name.
pub fn find_tool(name: &str) -> Option<&'static ToolDef> {
    BROWSER_TOOLS.iter().find(|t| t.name == name)
}

/// Look up a storage tool by name.
pub fn find_storage_tool(name: &str) -> Option<&'static ToolDef> {
    STORAGE_TOOLS.iter().find(|t| t.name == name)
}

/// Look up any inbuilt tool (Channel B — full catalog).
pub fn find_inbuilt_tool(name: &str) -> Option<&'static ToolDef> {
    all_tools().into_iter().find(|t| t.name == name)
}

// ---------------------------------------------------------------------------
// P64.9 — Shared-plane task façades (SPEC F16, ARCH/17 §17.5)
// ---------------------------------------------------------------------------
//
// External agents never receive 51 raw primitives — they receive task-shaped
// façades over the SAME Rust implementations (one engine, two façades). This
// table is the MCP-side mirror of `agentcowork-core::tools::FACADE_ROUTES`
// (flat dot-hierarchy ids, `readOnly`/destructive hints, fan-out to canonical
// 51-tool names). Guard-2 + audit are unchanged: façades dispatch through the
// same ticketed executor.

/// P64.9 — one shared-plane façade over the inbuilt catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacadeDef {
    /// Flat unique id, e.g. `office.edit`.
    pub name: &'static str,
    pub description: &'static str,
    /// MCP `readOnlyHint` mirror (true = never mutates).
    pub read_only: bool,
    /// Destructive hint (true = can destroy data; always mutating).
    pub destructive: bool,
    /// Risk tier hint (`low`/`medium`/`high`).
    pub risk: &'static str,
    /// Canonical inbuilt tool names this façade fans out to.
    pub fans_out_to: &'static [&'static str],
    /// P71.1 — true when the façade is served by a kernel seam (the Work
    /// Gateway delegation bridge) rather than catalog fan-out. Kernel-routed
    /// façades carry an empty `fans_out_to` by design: delegation mints child
    /// Work through the one executor, never a parallel tool.
    pub kernel_route: bool,
}

/// P64.9 — the shared-plane façades (ARCH/17 §17.5): office 6 · browser 3
/// · computer-use 2 · workspace 1 · artifact 2 · work 2 · delegate 3
/// (P71.1 — kernel-routed to the Work Gateway delegation seam).
pub const SHARED_FACADES: &[FacadeDef] = &[
    FacadeDef {
        name: "office.open",
        description: "Open a document for reading (docx/xlsx/pptx/pdf)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["office_open"],
        kernel_route: false,
    },
    FacadeDef {
        name: "office.inspect",
        description: "Inspect document structure (outline, sheets, pages)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["office_open"],
        kernel_route: false,
    },
    FacadeDef {
        name: "office.edit",
        description: "Edit one document block (surgical patch)",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &["office_edit"],
        kernel_route: false,
    },
    FacadeDef {
        name: "office.calculate",
        description: "Recalculate a spreadsheet through the formula engine",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &["office_edit", "office_open"],
        kernel_route: false,
    },
    FacadeDef {
        name: "office.render",
        description: "Render or export a document (pdf export path)",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &["office_export", "office_open"],
        kernel_route: false,
    },
    FacadeDef {
        name: "office.verify",
        description: "Verify document conformance (open + inspect)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["office_open"],
        kernel_route: false,
    },
    FacadeDef {
        name: "browser.research",
        description: "Research the web (search + read + deep research)",
        read_only: true,
        destructive: false,
        risk: "medium",
        fans_out_to: &["search_web", "read", "deep_research"],
        kernel_route: false,
    },
    FacadeDef {
        name: "browser.operate",
        description: "Operate the browser (navigate + snapshot + act + wait)",
        read_only: false,
        destructive: false,
        risk: "high",
        fans_out_to: &["navigate", "snapshot", "act", "wait"],
        kernel_route: false,
    },
    FacadeDef {
        name: "browser.extract",
        description: "Extract page content (read + grep + pdf + screenshot)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["read", "grep", "pdf", "screenshot"],
        kernel_route: false,
    },
    FacadeDef {
        name: "computer_use.see",
        description: "Observe the desktop (windows + read)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["windows", "tabs"],
        kernel_route: false,
    },
    FacadeDef {
        name: "computer_use.act",
        description: "Act on the desktop (act + wait)",
        read_only: false,
        destructive: true,
        risk: "high",
        fans_out_to: &["act", "wait"],
        kernel_route: false,
    },
    FacadeDef {
        name: "workspace.map",
        description: "Map the workspace (scan + filename search)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["disk_scan", "filename_search"],
        kernel_route: false,
    },
    FacadeDef {
        name: "artifact.store",
        description: "Store an artifact (proposal + scan surface)",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &["disk_scan", "disk_cleanup"],
        kernel_route: false,
    },
    FacadeDef {
        name: "artifact.retrieve",
        description: "Retrieve an artifact (scan + search surface)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["disk_scan", "filename_search"],
        kernel_route: false,
    },
    FacadeDef {
        name: "artifact.retrieve_original",
        description: "Read a line range of a spooled tool output by its content address (hash)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &[],
        kernel_route: true,
    },
    FacadeDef {
        name: "work.create",
        description: "Create durable work (memory store + plan surface)",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &["memory_store", "disk_scan"],
        kernel_route: false,
    },
    FacadeDef {
        name: "work.status",
        description: "Read durable work status (retrieve + review surface)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &["memory_retrieve", "memory_review_due"],
        kernel_route: false,
    },
    // P71.1 — the delegation family. Delegation is a platform feature, not a
    // built-in engine's private ability: the primary agent chooses, AgentCowork
    // validates. These route to the Work Gateway child-Work seam.
    FacadeDef {
        name: "delegate.spawn",
        description: "Delegate a task to a child Work (spawn a subagent)",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &[],
        kernel_route: true,
    },
    FacadeDef {
        name: "delegate.status",
        description: "Read one delegated child's state (child Work + presence)",
        read_only: true,
        destructive: false,
        risk: "low",
        fans_out_to: &[],
        kernel_route: true,
    },
    FacadeDef {
        name: "delegate.cancel",
        description: "Cancel one delegated child (close its child Work)",
        read_only: false,
        destructive: false,
        risk: "medium",
        fans_out_to: &[],
        kernel_route: true,
    },
];

/// P64.9 — look up a façade by id.
pub fn find_facade(name: &str) -> Option<&'static FacadeDef> {
    SHARED_FACADES.iter().find(|f| f.name == name)
}

/// P64.9 — validate the façade table: flat unique dot-hierarchy ids,
/// `readOnly`/destructive consistency, and fan-out targets that all exist in
/// the 51-tool catalog. Fails closed with the first violation.
pub fn validate_facades() -> Result<(), String> {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for f in SHARED_FACADES {
        if !f.name.contains('.') {
            return Err(format!("façade {:?} needs dot hierarchy", f.name));
        }
        if !seen.insert(f.name) {
            return Err(format!("duplicate façade {:?}", f.name));
        }
        if f.destructive && f.read_only {
            return Err(format!(
                "façade {:?} cannot be destructive + readOnly",
                f.name
            ));
        }
        if f.kernel_route {
            // P71.1 — kernel-routed façades (`delegate.*`) reach the Work
            // Gateway delegation seam and deliberately fan out to no catalog
            // tool; a catalog target here would be the parallel path I4 forbids.
            if !f.fans_out_to.is_empty() {
                return Err(format!(
                    "façade {:?} is kernel-routed but fans out to catalog tools",
                    f.name
                ));
            }
        } else {
            if f.fans_out_to.is_empty() {
                return Err(format!("façade {:?} fans out to nothing", f.name));
            }
            for t in f.fans_out_to {
                if find_inbuilt_tool(t).is_none() {
                    return Err(format!(
                        "façade {:?} fans out to unknown tool {t:?}",
                        f.name
                    ));
                }
            }
        }
        // Destructive façades must be high-risk + mutating (same Guard path
        // as the native tool they wrap).
        if f.destructive && (f.read_only || f.risk != "high") {
            return Err(format!(
                "façade {:?} destructive must be mutating high-risk",
                f.name
            ));
        }
    }
    Ok(())
}

/// Tools belonging to a profile (doc 55: paginated discovery per profile).
pub fn tools_for_profile(profile: ToolProfile) -> Vec<&'static ToolDef> {
    BROWSER_TOOLS
        .iter()
        .filter(|t| profile == ToolProfile::All || t.profile == profile)
        .collect()
}

/// Paginated discovery: `page` is 0-based, `page_size` > 0. Returns the
/// slice for that page plus whether more pages follow (doc 55 mcp.rs).
pub fn paginate(
    tools: &[&'static ToolDef],
    page: usize,
    page_size: usize,
) -> (Vec<&'static ToolDef>, bool) {
    if page_size == 0 {
        return (Vec::new(), false);
    }
    let start = page * page_size;
    let end = (start + page_size).min(tools.len());
    let has_more = end < tools.len();
    let slice = tools.get(start..end).unwrap_or_default().to_vec();
    (slice, has_more)
}

/// extraArgs parity (doc 55): validate a call's args against the tool's
/// schema — required args present, unknown args allowed (forwarded).
pub fn validate_args(
    tool: &ToolDef,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    for a in tool.args {
        if a.required && !args.contains_key(a.name) {
            return Err(format!(
                "missing required arg '{}' for tool '{}'",
                a.name, tool.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_has_exactly_37_tools() {
        assert_eq!(BROWSER_TOOLS.len(), 37);
    }

    #[test]
    fn original_17_first_and_ordered() {
        let names: Vec<&str> = BROWSER_TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(
            &names[..17],
            &[
                "tabs",
                "tab_groups",
                "history",
                "navigate",
                "snapshot",
                "diff",
                "act",
                "download",
                "upload",
                "read",
                "grep",
                "screenshot",
                "pdf",
                "wait",
                "windows",
                "evaluate",
                "run"
            ]
        );
    }

    #[test]
    fn totals_per_group() {
        let names: Vec<&str> = BROWSER_TOOLS.iter().map(|t| t.name).collect();
        let bookmarks = names.iter().filter(|n| n.contains("bookmark")).count();
        // The 5 tab-group MANAGEMENT tools (excludes the original 17 `tab_groups`).
        let tab_groups = [
            "list_tab_groups",
            "group_tabs",
            "update_tab_group",
            "ungroup_tabs",
            "close_tab_group",
        ]
        .iter()
        .filter(|n| names.contains(n))
        .count();
        // The 5 window-MANAGEMENT tools (excludes the original 17 `windows`).
        let windows = [
            "list_windows",
            "create_window",
            "create_hidden_window",
            "close_window",
            "activate_window",
        ]
        .iter()
        .filter(|n| names.contains(n))
        .count();
        let file_ops = names
            .iter()
            .filter(|n| n.contains("save_") || n.contains("download_file"))
            .count();
        assert_eq!(bookmarks, 6);
        assert_eq!(tab_groups, 5);
        assert_eq!(windows, 5);
        assert_eq!(file_ops, 3);
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = BROWSER_TOOLS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 37);
    }

    #[test]
    fn read_tools_annotated_read_only() {
        assert!(find_tool("snapshot").unwrap().read_only);
        assert!(find_tool("read").unwrap().read_only);
        assert!(!find_tool("act").unwrap().read_only);
    }

    #[test]
    fn execute_tools_are_open_world() {
        assert!(find_tool("run").unwrap().open_world);
        assert!(find_tool("evaluate").unwrap().open_world);
        assert!(!find_tool("navigate").unwrap().open_world);
    }

    #[test]
    fn every_tool_has_profile_and_kind() {
        for t in BROWSER_TOOLS {
            let _kind = t.kind;
            let _ = t.profile; // presence is the assertion
            assert!(!t.description.is_empty());
        }
    }

    #[test]
    fn profiles_subset_catalog() {
        let debug = tools_for_profile(ToolProfile::Debug);
        assert_eq!(debug.len(), 2); // evaluate + run
        assert!(debug.iter().all(|t| t.profile == ToolProfile::Debug));
        // React has no tools yet — empty profile is valid (doc 55 list parity).
        assert!(tools_for_profile(ToolProfile::React).is_empty());
        let all = tools_for_profile(ToolProfile::All);
        assert_eq!(all.len(), 37);
    }

    #[test]
    fn pagination_returns_pages_and_has_more() {
        let all = tools_for_profile(ToolProfile::All);
        let (p1, more1) = paginate(&all, 0, 10);
        assert_eq!(p1.len(), 10);
        assert!(more1);
        let (p4, more4) = paginate(&all, 3, 10);
        assert_eq!(p4.len(), 7);
        assert!(!more4);
        let (empty, _) = paginate(&all, 99, 10);
        assert!(empty.is_empty());
        let (_, has_more) = paginate(&all, 0, 0);
        assert!(!has_more);
    }

    #[test]
    fn typed_args_validate_required() {
        let nav = find_tool("navigate").unwrap();
        let mut args = serde_json::Map::new();
        assert!(validate_args(nav, &args).is_err());
        args.insert("url".into(), json!("https://example.com"));
        assert!(validate_args(nav, &args).is_ok());
        // extraArgs parity: unknown args pass through.
        args.insert("extraArg".into(), json!(42));
        assert!(validate_args(nav, &args).is_ok());
    }

    #[test]
    fn act_has_full_typed_args() {
        let act = find_tool("act").unwrap();
        let names: Vec<&str> = act.args.iter().map(|a| a.name).collect();
        assert!(names.contains(&"ref_id"));
        assert!(names.contains(&"fields"));
        assert!(names.contains(&"x"));
    }

    #[test]
    fn storage_catalog_has_5_tools() {
        assert_eq!(STORAGE_TOOLS.len(), 5);
        let names: Vec<&str> = STORAGE_TOOLS.iter().map(|t| t.name).collect();
        assert!(names.contains(&"disk_scan"));
        assert!(names.contains(&"disk_duplicates"));
        assert!(names.contains(&"disk_large_files"));
        assert!(names.contains(&"disk_cleanup"));
        assert!(names.contains(&"filename_search"));
    }

    #[test]
    fn storage_tools_are_read_only_proposals() {
        for t in STORAGE_TOOLS {
            assert!(t.read_only, "{} must be read-only (never deletes)", t.name);
        }
        assert!(find_storage_tool("disk_cleanup").unwrap().read_only);
    }

    #[test]
    fn all_tools_merges_browser_and_storage() {
        let all = all_tools();
        assert_eq!(all.len(), 37 + 4 + 3 + 2 + 5); // browser + office + memory + search + storage
        // No name collision across the catalogs (Channel B unified registry).
        let mut names: Vec<&str> = all.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 51);
        assert!(find_inbuilt_tool("office_edit").is_some());
        assert!(find_inbuilt_tool("memory_retrieve").is_some());
        assert!(find_inbuilt_tool("deep_research").is_some());
        assert_eq!(inbuilt_catalog().len(), 5);
    }

    // --- P64.9 façades ------------------------------------------------------

    #[test]
    fn p64_facades_validate_against_51_tool_catalog() {
        assert!(validate_facades().is_ok());
        assert!(SHARED_FACADES.len() >= 14, "got {}", SHARED_FACADES.len());
    }

    #[test]
    fn p64_facade_ids_are_flat_unique_with_dot_hierarchy() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for f in SHARED_FACADES {
            assert!(f.name.contains('.'), "{:?} needs dot hierarchy", f.name);
            assert!(!f.name.starts_with('.') && !f.name.ends_with('.'));
            assert!(seen.insert(f.name), "duplicate façade {:?}", f.name);
            assert!(!f.description.is_empty());
        }
    }

    #[test]
    fn p64_facade_annotations_carry_readonly_destructive_hints() {
        // Read-only façades never mutate; destructive façades are high-risk.
        for f in SHARED_FACADES {
            assert!(!(f.destructive && f.read_only), "{:?}", f.name);
            if f.destructive {
                assert_eq!(f.risk, "high", "{:?}", f.name);
            }
        }
        assert!(!find_facade("office.edit").unwrap().read_only);
        assert!(find_facade("office.open").unwrap().read_only);
        assert!(find_facade("computer_use.act").unwrap().destructive);
        assert!(find_facade("browser.extract").unwrap().read_only);
    }

    #[test]
    fn p64_facade_fanout_targets_all_exist() {
        for f in SHARED_FACADES {
            for t in f.fans_out_to {
                assert!(
                    find_inbuilt_tool(t).is_some(),
                    "façade {:?} fans out to unknown tool {t:?}",
                    f.name
                );
            }
        }
        // Spot-check the ARCH/17 §17.5 fan-outs.
        assert!(
            find_facade("browser.research")
                .unwrap()
                .fans_out_to
                .contains(&"search_web")
        );
        assert!(
            find_facade("workspace.map")
                .unwrap()
                .fans_out_to
                .contains(&"disk_scan")
        );
        assert!(
            find_facade("office.edit")
                .unwrap()
                .fans_out_to
                .contains(&"office_edit")
        );
    }
}
