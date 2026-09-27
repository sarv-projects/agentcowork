//! agentcowork-script — the `run`/`evaluate` sandbox (ARCH/08 §8.4, E4).
//!
//! P2.5: rquickjs async runtime with hard limits (64MB heap / 512KB stack /
//! 30s timeout / 1K log lines / 2MB return), the `browser` SDK surface, and
//! the **InnerCallHook** audit guarantee: every primitive inside a script is
//! (a) authorized against ownership + permissions, (b) recorded as a child
//! audit row, (c) page-creations claimed — scripts cannot bypass the audit
//! trail or touch foreign tabs.

pub mod artifact;
mod sandbox;
pub mod selfheal;

use serde::{Deserialize, Serialize};

pub use sandbox::Sandbox;

/// Hard limits the sandbox enforces (spec E4 / ARCH/08 §8.4):
/// 64MB heap, 512KB stack, 30s timeout, 1K log lines, 2MB return payload.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxLimits {
    pub max_heap_bytes: u64,
    pub max_stack_bytes: u64,
    pub timeout_secs: u64,
    pub max_log_lines: u64,
    pub max_return_bytes: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            max_heap_bytes: 64 * 1024 * 1024,
            max_stack_bytes: 512 * 1024,
            timeout_secs: 30,
            max_log_lines: 1024,
            max_return_bytes: 2 * 1024 * 1024,
        }
    }
}

/// One browser primitive invocation from inside a script. Every primitive
/// funnels through this shape so the InnerCallHook can authorize + record it
/// uniformly — there is no way for a script to reach a browser action that
/// does not pass through it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrimitiveCall {
    /// Stable dotted name, e.g. `pages.newPage`, `nav.goto`, `read`, `cdp`.
    pub name: String,
    /// Target page when the primitive is page-scoped (`args.pageId`).
    pub page_id: Option<String>,
    /// Normalized JSON arguments exactly as the script passed them.
    pub args: serde_json::Value,
}

/// Page ownership (ARCH/08 §8.4): scripts only act on pages they own.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PageOwnership {
    Mine,
    User,
    OtherAgent,
}

/// One page as the host sees it (`browser.pages.list()` / `getInfo`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    pub ownership: PageOwnership,
}

/// The host side of the `browser` SDK. The sandbox routes every primitive
/// through the InnerCallHook (authorize → exec → record → page-claim); the
/// host supplies the policy, the audit writer, and the real browser.
pub trait BrowserHost: Send + Sync {
    /// (a) Authorization: ownership + permissions check. Returning `Err`
    /// denies the primitive — it is still recorded (as `ok=false`) and the
    /// script sees a JS error.
    fn authorize(&self, call: &PrimitiveCall) -> Result<(), SandboxError>;

    /// (b) Record the primitive as a child audit row. Called for **every**
    /// attempt, denied ones included, so the trail cannot be bypassed.
    fn record(&self, call: &PrimitiveCall, ok: bool, error: &str) -> Result<(), SandboxError>;

    /// (c) Claim a page this script created (grouped like `tabs new`).
    fn on_page_created(
        &self,
        page_id: &str,
        created_from: &PrimitiveCall,
    ) -> Result<(), SandboxError>;

    /// All known pages with ownership labels (`browser.pages.list()`).
    fn pages(&self) -> Vec<PageInfo>;

    /// Execute the primitive; the JSON result is handed to the script.
    fn exec(&self, call: &PrimitiveCall) -> Result<serde_json::Value, SandboxError>;
}

/// The host side of the `data` SDK (P5.8 — E4 `data.query` over a ref
/// handle). The script asks for matching lines from a large payload by
/// handle; the host resolves the handle and returns only the hits — the
/// payload itself is never serialized into the sandbox. The real binding is
/// `agentcowork_memory::reference::query_ref`; this trait is the sandbox seam.
pub trait DataHost: Send + Sync {
    /// Return the matching lines (capped) for `term` in the payload named by
    /// `handle`. The result is a JSON-serializable value handed to the script.
    fn query(
        &self,
        handle: &str,
        term: &str,
        max_hits: usize,
    ) -> Result<serde_json::Value, SandboxError>;
}

/// The sandbox surface. `Sandbox` implements it over rquickjs; the trait
/// fixes the contract so callers (agentcowork-mcp `run` tool) don't change.
pub trait ScriptSandbox {
    /// Evaluate `code` (top-level `await` supported) and return a JSON
    /// string `{"result": <script return>, "logs": [...], "logs_truncated": bool}`.
    /// Every `browser` SDK primitive passes through the InnerCallHook.
    fn eval(&self, code: &str) -> Result<String, SandboxError>;

    /// The limits this sandbox instance enforces.
    fn limits(&self) -> SandboxLimits;
}

/// Failure modes the sandbox distinguishes (each maps to a clear model
/// signal — a truncated/errored script must never look like success).
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("script timeout after {0}s")]
    Timeout(u64),
    #[error("resource limit exceeded: {0}")]
    Limit(String),
    #[error("javascript error: {0}")]
    Js(String),
    #[error("return payload too large ({0} bytes, max {1})")]
    ReturnTooLarge(usize, u64),
    #[error("browser primitive `{0}` failed: {1}")]
    Primitive(String, String),
    #[error("sandbox runtime error: {0}")]
    Runtime(String),
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
