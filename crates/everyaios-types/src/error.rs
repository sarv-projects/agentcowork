//! `ARCH/10-KERNEL.md` §3 — **the canonical error taxonomy** for every kernel
//! boundary, and the one rule that keeps internals on this side of the wire.
//!
//! The taxonomy is a closed set of nine codes. Extending it requires a `DEC`
//! (`ARCH/10-KERNEL.md` §3); this module therefore *derives* nothing and adds
//! nothing: [`ErrorCode`] is exactly the spec's table, and
//! [`ErrorCode::retry_class`] is exactly its retryability column.
//!
//! # Two types, on purpose
//!
//! - [`KernelError`] is the **in-process** form. It carries the full cause
//!   chain and an optional internal diagnostic, and it is what a log, an audit
//!   row or a crash report reads. It never crosses a boundary.
//! - [`BoundaryError`] is the **wire** form: `{ code, message, retryable,
//!   cause?, next_steps? }` (`ARCH/10-KERNEL.md` §7). It is built *only* from a
//!   scrubbed message and a bounded, scrubbed cause label, so there is no field
//!   on it that can carry a store name, a stack, a path or a credential.
//!
//! The conversion is [`KernelError::into_boundary`]. It is the single place
//! where the decision "what may leave the process" is made, and the unit tests
//! pin the property with a secret-shaped corpus: a credential assignment and an
//! absolute path in the message or in the cause chain do not survive it.
//!
//! # No `thiserror`
//!
//! This crate carries only `serde`/`serde_json`: `Display` and
//! `std::error::Error` are written out by hand so adding the taxonomy does not
//! add a dependency to the crate every other crate already depends on.
//!
//! [`Code`]: ErrorCode

use std::error::Error as StdError;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// Longest message (in chars) allowed to cross a boundary. A boundary message
/// is a classification aid, not a log line: anything longer belongs in the
/// cause chain, which stays in-process.
pub const MAX_BOUNDARY_MESSAGE: usize = 240;

/// Longest cause label (in chars) allowed to cross a boundary.
pub const MAX_BOUNDARY_CAUSE: usize = 120;

/// Stand-in written where a value was removed on purpose, so a reader can tell
/// "redacted" from "absent".
pub const REDACTED: &str = "<redacted>";

/// Stand-in written where an absolute filesystem path was removed.
pub const REDACTED_PATH: &str = "<path>";

/// The canonical taxonomy codes (`ARCH/10-KERNEL.md` §3, `ARCH/07-CONTRACTS.md`
/// §0). The set is closed: a new situation is a new *context*, not a new code.
///
/// The wire spelling is the spec's own PascalCase, verbatim — the UI maps on
/// these strings, so they are a compatibility surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ErrorCode {
    /// Guard denied, or the ticket is missing / expired / insufficient. Not
    /// retryable: re-plan or ask.
    #[serde(rename = "AuthorizationDenied")]
    AuthorizationDenied,
    /// The target does not exist, or policy filters it out of the caller's
    /// view. Not retryable.
    #[serde(rename = "NotFound")]
    NotFound,
    /// A concurrent state change (lease, version, duplicate). Retryable, but
    /// only a bounded number of times.
    #[serde(rename = "Conflict")]
    Conflict,
    /// A provider, agent or environment is down or degraded. Retryable with
    /// backoff.
    #[serde(rename = "Unavailable")]
    Unavailable,
    /// The call exceeded its deadline. Retryable.
    #[serde(rename = "Timeout")]
    Timeout,
    /// The operation is not valid for the current state, including a stale
    /// epoch. Not retryable.
    #[serde(rename = "InvalidState")]
    InvalidState,
    /// The capability can proceed only after user setup. **A result with next
    /// steps, not a failure** (`ARCH/10-KERNEL.md` §3, `ARCH/13-CAPABILITY.md`).
    #[serde(rename = "GuidanceRequired")]
    GuidanceRequired,
    /// The call explicitly needs a human decision or input. **A result with
    /// next steps, not a failure** — it travels the approval path.
    #[serde(rename = "RequiresUserAction")]
    RequiresUserAction,
    /// A bug. Not retryable: report and log.
    #[serde(rename = "Internal")]
    Internal,
}

/// How a caller may treat a code — the spec's retryability column, kept as a
/// vocabulary rather than a bare `bool` so "retryable" never loses *how*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    /// Never retry: the call must change, not repeat.
    Never,
    /// Retry a bounded number of times (a lease race, a version conflict).
    Bounded,
    /// Retry with backoff (a provider is down or degraded).
    Backoff,
}

impl RetryClass {
    /// The wire-facing boolean: "may this call be retried as-is?".
    pub fn is_retryable(self) -> bool {
        !matches!(self, Self::Never)
    }

    /// Stable token for logs and audit rows.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Bounded => "bounded",
            Self::Backoff => "backoff",
        }
    }
}

impl ErrorCode {
    /// The spec's nine codes, in table order. One list, so a registry, a test
    /// or a UI mapping can enumerate the taxonomy instead of repeating it.
    pub const ALL: [ErrorCode; 9] = [
        ErrorCode::AuthorizationDenied,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::Unavailable,
        ErrorCode::Timeout,
        ErrorCode::InvalidState,
        ErrorCode::GuidanceRequired,
        ErrorCode::RequiresUserAction,
        ErrorCode::Internal,
    ];

    /// The stable wire spelling — the spec's own token, byte for byte.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationDenied => "AuthorizationDenied",
            Self::NotFound => "NotFound",
            Self::Conflict => "Conflict",
            Self::Unavailable => "Unavailable",
            Self::Timeout => "Timeout",
            Self::InvalidState => "InvalidState",
            Self::GuidanceRequired => "GuidanceRequired",
            Self::RequiresUserAction => "RequiresUserAction",
            Self::Internal => "Internal",
        }
    }

    /// Parse the stable wire spelling. Unknown text is `None`, never a guess:
    /// a peer that invents a code must be refused, not rounded to `Internal`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "AuthorizationDenied" => Self::AuthorizationDenied,
            "NotFound" => Self::NotFound,
            "Conflict" => Self::Conflict,
            "Unavailable" => Self::Unavailable,
            "Timeout" => Self::Timeout,
            "InvalidState" => Self::InvalidState,
            "GuidanceRequired" => Self::GuidanceRequired,
            "RequiresUserAction" => Self::RequiresUserAction,
            "Internal" => Self::Internal,
            _ => return None,
        })
    }

    /// The retryability of this code, per the spec table.
    pub const fn retry_class(self) -> RetryClass {
        match self {
            Self::AuthorizationDenied
            | Self::NotFound
            | Self::InvalidState
            | Self::GuidanceRequired
            | Self::RequiresUserAction
            | Self::Internal => RetryClass::Never,
            Self::Conflict => RetryClass::Bounded,
            Self::Unavailable => RetryClass::Backoff,
            Self::Timeout => RetryClass::Backoff,
        }
    }

    /// Whether a caller may retry the call unchanged.
    pub const fn is_retryable(self) -> bool {
        match self.retry_class() {
            RetryClass::Never => false,
            RetryClass::Bounded | RetryClass::Backoff => true,
        }
    }

    /// Whether this code is one of the two that must arrive **as a result with
    /// next steps**, never as a bare failure (`ARCH/10-KERNEL.md` §3).
    pub const fn is_actionable(self) -> bool {
        matches!(self, Self::GuidanceRequired | Self::RequiresUserAction)
    }

    /// The retry class of a code read off the wire, for a peer that sent an
    /// unknown code. `Internal` is the only safe reading of "I do not know what
    /// happened", and it is never retryable.
    pub const fn of_unknown() -> Self {
        Self::Internal
    }
}

impl ErrorCode {
    /// JSON-RPC parse error: the payload is not interpretable JSON.
    pub const JSONRPC_PARSE_ERROR: i64 = -32700;
    /// JSON-RPC invalid request: the payload is not a valid request object.
    pub const JSONRPC_INVALID_REQUEST: i64 = -32600;
    /// JSON-RPC method not found: the named method does not exist.
    pub const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
    /// JSON-RPC internal error: the standard range's catch-all.
    pub const JSONRPC_INTERNAL_ERROR: i64 = -32603;

    /// The first implementation-defined code, which is where the taxonomy's
    /// stable codes live (`-32000…-32099` is JSON-RPC's reserved range for
    /// "server-defined", so nothing here can collide with a future standard
    /// code).
    pub const JSONRPC_APPLICATION_BASE: i64 = -32000;

    // The eight stable application codes, named so the mapping is data both ways
    // and a peer can switch on the constant instead of the number.
    pub const JSONRPC_AUTHORIZATION_DENIED: i64 = Self::JSONRPC_APPLICATION_BASE - 1;
    pub const JSONRPC_NOT_FOUND: i64 = Self::JSONRPC_APPLICATION_BASE - 2;
    pub const JSONRPC_CONFLICT: i64 = Self::JSONRPC_APPLICATION_BASE - 3;
    pub const JSONRPC_UNAVAILABLE: i64 = Self::JSONRPC_APPLICATION_BASE - 4;
    pub const JSONRPC_TIMEOUT: i64 = Self::JSONRPC_APPLICATION_BASE - 5;
    pub const JSONRPC_INVALID_STATE: i64 = Self::JSONRPC_APPLICATION_BASE - 6;
    pub const JSONRPC_GUIDANCE_REQUIRED: i64 = Self::JSONRPC_APPLICATION_BASE - 7;
    pub const JSONRPC_REQUIRES_USER_ACTION: i64 = Self::JSONRPC_APPLICATION_BASE - 8;

    /// The stable JSON-RPC code for one taxonomy code. This mapping is kernel
    /// policy, not transport: the transport carries the integer, the kernel
    /// decides what it means.
    ///
    /// `Internal` reuses JSON-RPC's own `-32603` rather than taking an
    /// application code: it is the same fact, and a peer that special-cases the
    /// standard range keeps working.
    pub const fn jsonrpc_code(self) -> i64 {
        match self {
            Self::AuthorizationDenied => Self::JSONRPC_AUTHORIZATION_DENIED,
            Self::NotFound => Self::JSONRPC_NOT_FOUND,
            Self::Conflict => Self::JSONRPC_CONFLICT,
            Self::Unavailable => Self::JSONRPC_UNAVAILABLE,
            Self::Timeout => Self::JSONRPC_TIMEOUT,
            Self::InvalidState => Self::JSONRPC_INVALID_STATE,
            Self::GuidanceRequired => Self::JSONRPC_GUIDANCE_REQUIRED,
            Self::RequiresUserAction => Self::JSONRPC_REQUIRES_USER_ACTION,
            Self::Internal => Self::JSONRPC_INTERNAL_ERROR,
        }
    }

    /// The taxonomy code a JSON-RPC code denotes, when it denotes one.
    ///
    /// The three shared standard codes map by *meaning*, not by number
    /// coincidence: an unknown method is a target that does not exist, and an
    /// unparseable payload is a document this build cannot interpret. The
    /// remaining reserved codes have no counterpart and are `None`, so a caller
    /// is forced to decide what an unrecognised code means instead of inheriting
    /// a guess.
    pub const fn from_jsonrpc_code(code: i64) -> Option<Self> {
        match code {
            Self::JSONRPC_AUTHORIZATION_DENIED => Some(Self::AuthorizationDenied),
            Self::JSONRPC_NOT_FOUND => Some(Self::NotFound),
            Self::JSONRPC_CONFLICT => Some(Self::Conflict),
            Self::JSONRPC_UNAVAILABLE => Some(Self::Unavailable),
            Self::JSONRPC_TIMEOUT => Some(Self::Timeout),
            Self::JSONRPC_INVALID_STATE => Some(Self::InvalidState),
            Self::JSONRPC_GUIDANCE_REQUIRED => Some(Self::GuidanceRequired),
            Self::JSONRPC_REQUIRES_USER_ACTION => Some(Self::RequiresUserAction),
            Self::JSONRPC_METHOD_NOT_FOUND => Some(Self::NotFound),
            Self::JSONRPC_PARSE_ERROR | Self::JSONRPC_INVALID_REQUEST => Some(Self::InvalidState),
            Self::JSONRPC_INTERNAL_ERROR => Some(Self::Internal),
            _ => None,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One next step a caller can act on. Free-form on purpose: the taxonomy fixes
/// the *codes*, not a second vocabulary of setup verbs, and a step that cannot
/// be phrased is a step that has not been thought through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextStep {
    /// What the caller should do, in one sentence, addressed to the user.
    pub instruction: String,
    /// Optional pointer at the place that explains or performs it (a settings
    /// surface, a doc anchor). A reference, never an embedded secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl NextStep {
    /// A step with no target.
    pub fn new(instruction: impl Into<String>) -> Self {
        Self {
            instruction: instruction.into(),
            target: None,
        }
    }

    /// A step that points at the surface that performs it.
    pub fn at(instruction: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            instruction: instruction.into(),
            target: Some(target.into()),
        }
    }
}

/// The **wire** form of a boundary error (`ARCH/10-KERNEL.md` §7): a stable
/// code, a safe message, the derived retryability, a bounded cause label and
/// next steps for the two actionable codes.
///
/// Two invariants are enforced by construction *and* by parsing, so a peer
/// cannot hand us a payload that disagrees with the taxonomy:
///
/// 1. `retryable` is always the code's own [`RetryClass`] — a caller reads one
///    field and cannot be told to loop forever on `AuthorizationDenied`.
/// 2. [`Self::from_json`] refuses a document with neither `code` nor a usable
///    message, and a message that is only whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundaryError {
    /// The canonical code. The UI maps on this string.
    pub code: ErrorCode,
    /// A short, scrubbed, human-readable summary. Never a secret, never user
    /// content, never an internal name.
    pub message: String,
    /// Derived from `code`; never set independently.
    pub retryable: bool,
    /// A bounded, scrubbed label for the underlying cause. The **chain** stays
    /// in-process (`KernelError::cause_chain`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// Present for [`ErrorCode::is_actionable`] codes; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<NextStep>,
}

impl BoundaryError {
    /// Build a boundary error from a code and a message. The message is
    /// scrubbed here, so a careless caller cannot leak a path or a credential by
    /// forgetting to.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: scrub(&message.into(), MAX_BOUNDARY_MESSAGE),
            retryable: code.is_retryable(),
            cause: None,
            next_steps: Vec::new(),
        }
    }

    /// Attach a bounded, scrubbed cause label.
    #[must_use]
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        let cause = scrub(&cause.into(), MAX_BOUNDARY_CAUSE);
        self.cause = (!cause.is_empty()).then_some(cause);
        self
    }

    /// Attach next steps. Steps on a non-actionable code are refused: a
    /// `NotFound` that also says "here is what to do" is a guidance result
    /// wearing the wrong code, and the UI keys off the code.
    pub fn try_with_next_steps(
        mut self,
        steps: impl IntoIterator<Item = NextStep>,
    ) -> Result<Self, BoundaryError> {
        if !self.code.is_actionable() {
            return Err(BoundaryError::new(
                ErrorCode::Internal,
                "next steps were attached to a code that is not actionable",
            ));
        }
        self.next_steps = steps
            .into_iter()
            .map(|step| NextStep {
                instruction: scrub(&step.instruction, MAX_BOUNDARY_MESSAGE),
                target: step
                    .target
                    .map(|t| scrub(&t, MAX_BOUNDARY_CAUSE))
                    .filter(|t| !t.is_empty()),
            })
            .collect();
        Ok(self)
    }

    /// The retry class this payload was built with.
    pub fn retry_class(&self) -> RetryClass {
        self.code.retry_class()
    }

    /// The wire form as canonical bytes is the caller's job
    /// (`canonical::to_canonical`); this is the plain serde form.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a boundary error is always serializable")
    }

    /// Parse a wire payload, re-deriving `retryable` from the code and refusing
    /// a document that contradicts it.
    pub fn from_json(raw: &str) -> Result<Self, BoundaryError> {
        serde_json::from_str(raw).map_err(|err| {
            BoundaryError::new(
                ErrorCode::Internal,
                format!("error payload is not a canonical boundary error: {err}"),
            )
        })
    }
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(cause) = &self.cause {
            write!(f, " (cause: {cause})")?;
        }
        Ok(())
    }
}

impl StdError for BoundaryError {}

impl<'de> Deserialize<'de> for BoundaryError {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            code: ErrorCode,
            message: String,
            #[serde(default)]
            retryable: Option<bool>,
            #[serde(default)]
            cause: Option<String>,
            #[serde(default)]
            next_steps: Vec<NextStep>,
        }
        let raw = Raw::deserialize(de)?;
        if raw.message.trim().is_empty() {
            return Err(D::Error::custom("a boundary error needs a message"));
        }
        // Retryability is derived, never believed: a peer that disagrees with
        // the taxonomy is refused rather than obeyed.
        if let Some(claimed) = raw.retryable
            && claimed != raw.code.is_retryable()
        {
            return Err(D::Error::custom(format!(
                "retryable={claimed} contradicts the taxonomy for {}",
                raw.code
            )));
        }
        if !raw.next_steps.is_empty() && !raw.code.is_actionable() {
            return Err(D::Error::custom(format!(
                "{} is not an actionable code, so it carries no next steps",
                raw.code
            )));
        }
        Ok(Self {
            code: raw.code,
            message: scrub(&raw.message, MAX_BOUNDARY_MESSAGE),
            retryable: raw.code.is_retryable(),
            cause: raw
                .cause
                .map(|c| scrub(&c, MAX_BOUNDARY_CAUSE))
                .filter(|c| !c.is_empty()),
            next_steps: raw.next_steps,
        })
    }
}

/// The **in-process** form of a kernel error: the code, the safe message, the
/// cause chain, and an internal diagnostic that is deliberately not part of any
/// wire shape.
///
/// The diagnostic and the chain exist for logs, audit rows and crash reports.
/// [`Self::into_boundary`] is the only way out, and it copies neither.
#[derive(Debug)]
pub struct KernelError {
    code: ErrorCode,
    message: String,
    next_steps: Vec<NextStep>,
    diagnostic: Option<String>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl KernelError {
    /// A kernel error with a code and a message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            next_steps: Vec::new(),
            diagnostic: None,
            source: None,
        }
    }

    /// `AuthorizationDenied` — a guard refusal, or a ticket that is missing,
    /// expired or insufficient. Never retryable.
    pub fn authorization_denied(reason: impl Into<String>) -> Self {
        Self::new(ErrorCode::AuthorizationDenied, reason)
    }

    /// `NotFound` — the target does not exist, or policy hides it.
    pub fn not_found(target: impl fmt::Display) -> Self {
        Self::new(ErrorCode::NotFound, format!("{target} was not found"))
    }

    /// `Conflict` — a concurrent state change. Retryable, bounded.
    pub fn conflict(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, what)
    }

    /// `Unavailable` — a provider, agent or environment is down or degraded.
    pub fn unavailable(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, what)
    }

    /// `Timeout` — the call exceeded its deadline.
    pub fn timed_out(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, what)
    }

    /// `InvalidState` — not valid for the current state, including a stale
    /// epoch.
    pub fn invalid_state(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidState, what)
    }

    /// `Internal` — a bug. Carries the detail; the detail does not travel.
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, detail)
    }

    /// `GuidanceRequired` — the capability proceeds only after user setup, and
    /// arrives as a result with next steps.
    pub fn guidance(required: impl Into<String>, steps: Vec<NextStep>) -> Self {
        let mut err = Self::new(ErrorCode::GuidanceRequired, required);
        err.next_steps = steps;
        err
    }

    /// `RequiresUserAction` — a human decision or input is needed; it travels
    /// the approval path and arrives as a result with next steps.
    pub fn requires_user_action(needed: impl Into<String>, steps: Vec<NextStep>) -> Self {
        let mut err = Self::new(ErrorCode::RequiresUserAction, needed);
        err.next_steps = steps;
        err
    }

    /// Attach the underlying cause. The chain is preserved for diagnostics and
    /// is what [`Self::cause_chain`] walks.
    #[must_use]
    pub fn with_cause(mut self, cause: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(cause));
        self
    }

    /// Attach an internal diagnostic note (a stack-ish breadcrumb, a store or
    /// table name, a SQL fragment). It is **never** part of a boundary payload.
    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }

    /// The code.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// The message as written, before scrubbing. In-process only.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The internal diagnostic, when one was attached.
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    /// The next steps, when the code is actionable.
    pub fn next_steps(&self) -> &[NextStep] {
        &self.next_steps
    }

    /// The whole cause chain, outermost first, as `Display` text. Diagnostics
    /// only: it includes whatever the causes say, paths included.
    pub fn cause_chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = self.source.as_deref().map(|e| e as &dyn StdError);
        while let Some(err) = current {
            chain.push(err.to_string());
            current = err.source();
        }
        chain
    }

    /// The boundary projection. This is the crossing point, and the only place
    /// the decision about what may leave the process is made: the message and
    /// the top cause label are scrubbed and bounded, and the diagnostic and the
    /// rest of the chain are dropped.
    pub fn into_boundary(self) -> BoundaryError {
        let cause = self
            .cause_chain()
            .first()
            .map(|label| scrub(label, MAX_BOUNDARY_CAUSE))
            .filter(|label| !label.is_empty());
        let mut out = BoundaryError {
            code: self.code,
            message: scrub(&self.message, MAX_BOUNDARY_MESSAGE),
            retryable: self.code.is_retryable(),
            cause,
            next_steps: Vec::new(),
        };
        if self.code.is_actionable() {
            out.next_steps = self
                .next_steps
                .into_iter()
                .map(|step| NextStep {
                    instruction: scrub(&step.instruction, MAX_BOUNDARY_MESSAGE),
                    target: step
                        .target
                        .map(|t| scrub(&t, MAX_BOUNDARY_CAUSE))
                        .filter(|t| !t.is_empty()),
                })
                .collect();
        }
        out
    }

    /// The boundary projection without consuming the error, for a caller that
    /// must also log the original.
    pub fn boundary(&self) -> BoundaryError {
        BoundaryError {
            code: self.code,
            message: scrub(&self.message, MAX_BOUNDARY_MESSAGE),
            retryable: self.code.is_retryable(),
            cause: self
                .cause_chain()
                .first()
                .map(|label| scrub(label, MAX_BOUNDARY_CAUSE))
                .filter(|label| !label.is_empty()),
            next_steps: self
                .next_steps
                .iter()
                .map(|step| NextStep {
                    instruction: scrub(&step.instruction, MAX_BOUNDARY_MESSAGE),
                    target: step
                        .target
                        .as_deref()
                        .map(|t| scrub(t, MAX_BOUNDARY_CAUSE))
                        .filter(|t| !t.is_empty()),
                })
                .collect(),
        }
    }
}

impl fmt::Display for KernelError {
    /// The safe rendering: code + message. The cause chain and the diagnostic
    /// are reachable through [`StdError::source`] and
    /// [`KernelError::diagnostic`], so a `{}` in a log line cannot spill them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl StdError for KernelError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

impl From<std::io::Error> for KernelError {
    /// I/O is `Unavailable` — the thing the caller wanted is not reachable right
    /// now — and the `io::Error` stays in the cause chain.
    fn from(err: std::io::Error) -> Self {
        Self::new(ErrorCode::Unavailable, "an i/o operation failed").with_cause(err)
    }
}

impl From<serde_json::Error> for KernelError {
    /// A payload that could not be parsed is `InvalidState`: the bytes the peer
    /// sent are not a document this build can interpret, and re-sending them
    /// unchanged cannot help.
    fn from(err: serde_json::Error) -> Self {
        Self::new(ErrorCode::InvalidState, "a payload was not valid json").with_cause(err)
    }
}

impl From<KernelError> for BoundaryError {
    fn from(err: KernelError) -> Self {
        err.into_boundary()
    }
}

/// Scrub a string so it is safe to put in front of a caller.
///
/// Three passes, in this order:
///
/// 1. **Control characters** are dropped — a terminal escape sequence in an
///    error message is an injection, not a message.
/// 2. **Credential-shaped assignments** (`api_key=…`, `Authorization: Bearer
///    …`, `password: …`, …) have their value replaced with [`REDACTED`]. The
///    key name is kept, because "which credential was missing" is the useful
///    half of the fact.
/// 3. **Absolute filesystem paths** (`/…`, `~/…`, `C:\…`) are replaced with
///    [`REDACTED_PATH`], because a path is exactly the internal topology the
///    boundary must not publish (INV-11). URLs are left alone: `//` is not a
///    path start.
///
/// The result is bounded to `max` chars, cut on a char boundary.
pub fn scrub(input: &str, max: usize) -> String {
    let stripped: String = input.chars().filter(|c| !c.is_control()).collect();
    let redacted = redact_credentials(&stripped);
    let redacted = redact_paths(&redacted);
    truncate(&redacted, max)
}

/// The credential key names whose following value is a secret. Matched
/// case-insensitively on the token before the first `:` or `=`.
const CREDENTIAL_KEYS: [&str; 14] = [
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "token",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "pwd",
    "authorization",
    "bearer",
    "credential",
    "private_key",
];

/// Auth scheme words after which the secret is the *next* token rather than the
/// one just read. `Authorization: Bearer eyJ…` is the shape this exists for.
const AUTH_SCHEMES: [&str; 5] = ["bearer", "basic", "digest", "token", "negotiate"];

fn redact_credentials(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        // A key runs from the start of the token to the first separator.
        let start = i;
        while i < chars.len() && is_key_char(chars[i]) {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect();
        let is_key = is_credential_key(&key);
        if i < chars.len() && (chars[i] == '=' || chars[i] == ':') && is_key {
            let sep = chars[i];
            i += 1;
            // Skip the spaces a human writes after the separator.
            while i < chars.len() && chars[i] == ' ' {
                i += 1;
            }
            // The value runs to the next space — except after an auth scheme word,
            // where the secret is the *next* token (`Authorization: Bearer eyJ…`).
            // Stopping at the space is how a JWT survives a redactor, so the
            // scheme case consumes one more token.
            let value_start = i;
            while i < chars.len() && chars[i] != ' ' {
                i += 1;
            }
            let scheme: String = chars[value_start..i].iter().collect();
            if AUTH_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) && i < chars.len() {
                i += 1;
                while i < chars.len() && chars[i] != ' ' {
                    i += 1;
                }
            }
            out.push_str(&key);
            out.push(sep);
            if i > value_start {
                out.push(' ');
                out.push_str(REDACTED);
            }
            continue;
        }
        out.extend(&chars[start..i]);
        if i < chars.len() {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Whether an assignment key names a credential.
///
/// A suffix match on the separators removed, not an equality test: real messages
/// write `OPENAI_API_KEY`, `x-api-key` and `Authorization`, and a redactor that
/// only knows the bare word `api_key` is a redactor that misses the case that
/// matters. `monkey` does not end in `api_key`, so the false-positive rate stays
/// where a reader would not notice.
fn is_credential_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let normalized: String = key
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    CREDENTIAL_KEYS
        .iter()
        .any(|known| normalized.ends_with(&known.replace(['_', '-'], "")))
}

fn redact_paths(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        if let Some(len) = path_run(&chars, i) {
            out.push_str(REDACTED_PATH);
            i += len;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The length of the absolute path starting at `start`, or `None`.
///
/// A POSIX path starts at a `/` that begins a token and has a second `/`
/// later; `~/` counts; a Windows path is a drive letter then `\`. `//` in a URL
/// is never a path start, because the character before it is not a token
/// boundary.
fn path_run(chars: &[char], start: usize) -> Option<usize> {
    let windows = chars.get(start + 1) == Some(&'\\')
        && start + 2 < chars.len()
        && chars[start].is_ascii_alphabetic()
        && (start == 0 || !is_key_char(chars[start - 1]));
    if windows {
        let mut i = start + 2;
        while i < chars.len() && chars[i] != ' ' && chars[i] != '\'' && chars[i] != '"' {
            i += 1;
        }
        return Some(i - start);
    }
    let home = chars.get(start + 1) == Some(&'/') && start > 0 && chars[start] == '~';
    let posix = chars.get(start) == Some(&'/')
        && (start == 0 || matches!(chars[start - 1], ' ' | '\t' | '(' | '[' | '{' | '=' | ','))
        && (home || chars[start + 1..].contains(&'/'));
    if posix {
        let mut i = start + (if home { 2 } else { 1 });
        while i < chars.len() && chars[i] != ' ' && chars[i] != '\'' && chars[i] != '"' {
            i += 1;
        }
        return Some(i - start);
    }
    None
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    let kept: String = input.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn taxonomy_is_exactly_the_spec_table() {
        assert_eq!(ErrorCode::ALL.len(), 9);
        let names: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "AuthorizationDenied",
                "NotFound",
                "Conflict",
                "Unavailable",
                "Timeout",
                "InvalidState",
                "GuidanceRequired",
                "RequiresUserAction",
                "Internal",
            ]
        );
        // Round-trip: parse accepts every canonical spelling, and nothing else.
        for code in ErrorCode::ALL {
            assert_eq!(ErrorCode::parse(code.as_str()), Some(code));
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                format!("\"{}\"", code.as_str())
            );
        }
        assert_eq!(ErrorCode::parse("Not_Found"), None);
        assert_eq!(ErrorCode::parse(""), None);
    }

    #[test]
    fn retryability_matches_the_spec_column() {
        // no (re-plan or ask) / no / yes (bounded) / yes (backoff) / yes
        // / no / no (surface guidance) / no (approval path) / no (report + log)
        let expected = [
            (ErrorCode::AuthorizationDenied, RetryClass::Never),
            (ErrorCode::NotFound, RetryClass::Never),
            (ErrorCode::Conflict, RetryClass::Bounded),
            (ErrorCode::Unavailable, RetryClass::Backoff),
            (ErrorCode::Timeout, RetryClass::Backoff),
            (ErrorCode::InvalidState, RetryClass::Never),
            (ErrorCode::GuidanceRequired, RetryClass::Never),
            (ErrorCode::RequiresUserAction, RetryClass::Never),
            (ErrorCode::Internal, RetryClass::Never),
        ];
        for (code, class) in expected {
            assert_eq!(code.retry_class(), class, "{code}");
        }
        assert!(ErrorCode::Conflict.is_retryable());
        assert!(ErrorCode::Unavailable.is_retryable());
        assert!(ErrorCode::Timeout.is_retryable());
        for code in [
            ErrorCode::AuthorizationDenied,
            ErrorCode::NotFound,
            ErrorCode::InvalidState,
            ErrorCode::GuidanceRequired,
            ErrorCode::RequiresUserAction,
            ErrorCode::Internal,
        ] {
            assert!(!code.is_retryable(), "{code} must not be retryable");
        }
    }

    #[test]
    fn every_code_has_a_stable_json_rpc_code() {
        for code in ErrorCode::ALL {
            let mapped = code.jsonrpc_code();
            assert_eq!(
                ErrorCode::from_jsonrpc_code(mapped),
                Some(code),
                "{code} must round-trip"
            );
        }
        for code in ErrorCode::ALL {
            let mapped = code.jsonrpc_code();
            if code == ErrorCode::Internal {
                // `Internal` deliberately reuses the standard code: it is the
                // same fact, and a peer that special-cases `-32603` keeps working.
                assert_eq!(mapped, ErrorCode::JSONRPC_INTERNAL_ERROR);
                continue;
            }
            // The application range is reserved for server-defined codes, so a
            // future standard assignment cannot collide with ours.
            assert!(
                (-32099..=-32000).contains(&mapped),
                "{code} → {mapped} is outside the server-defined range"
            );
        }
        // The application codes are the exact integers peers already switch on.
        assert_eq!(ErrorCode::JSONRPC_AUTHORIZATION_DENIED, -32001);
        assert_eq!(ErrorCode::JSONRPC_REQUIRES_USER_ACTION, -32008);
        // The three shared standard codes map by meaning.
        assert_eq!(
            ErrorCode::from_jsonrpc_code(ErrorCode::JSONRPC_METHOD_NOT_FOUND),
            Some(ErrorCode::NotFound)
        );
        assert_eq!(
            ErrorCode::from_jsonrpc_code(ErrorCode::JSONRPC_PARSE_ERROR),
            Some(ErrorCode::InvalidState)
        );
        // A code with no counterpart is refused, not guessed.
        assert_eq!(ErrorCode::from_jsonrpc_code(-32099), None);
        assert_eq!(ErrorCode::from_jsonrpc_code(42), None);
        // The standard codes that do not map are genuinely unmapped.
        for reserved in [-32602, -32604, -32605] {
            assert_eq!(ErrorCode::from_jsonrpc_code(reserved), None, "{reserved}");
        }
    }

    #[test]
    fn guidance_codes_carry_next_steps_and_others_cannot() {
        let guidance = KernelError::guidance(
            "the drive connector is not connected",
            vec![NextStep::at(
                "Connect Google Drive",
                "settings/connectors/drive",
            )],
        );
        assert!(guidance.code().is_actionable());
        let wire = guidance.into_boundary();
        assert_eq!(wire.code, ErrorCode::GuidanceRequired);
        assert_eq!(wire.message, "the drive connector is not connected");
        assert_eq!(wire.next_steps.len(), 1);
        assert_eq!(wire.next_steps[0].instruction, "Connect Google Drive");
        assert_eq!(
            wire.next_steps[0].target.as_deref(),
            Some("settings/connectors/drive")
        );
        // The actionable codes are the two the spec names, and they are exactly
        // the two that carry steps.
        assert_eq!(
            ErrorCode::ALL.iter().filter(|c| c.is_actionable()).count(),
            2
        );
        // A non-actionable code refuses steps: the UI keys off the code, so
        // steps on `NotFound` would be a guidance result wearing a wrong label.
        let err = BoundaryError::new(ErrorCode::NotFound, "no such work")
            .try_with_next_steps(vec![NextStep::new("create it")]);
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().code, ErrorCode::Internal);
    }

    #[test]
    fn an_unknown_code_from_a_peer_is_refused_not_guessed() {
        let raw = r#"{"code":"Teapot","message":"nope","retryable":true}"#;
        let err = BoundaryError::from_json(raw).expect_err("an invented code is refused");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("not a canonical boundary error"),
            "{err}"
        );
    }

    #[test]
    fn a_payload_that_contradicts_the_taxonomy_is_refused() {
        // `NotFound` is never retryable; a peer claiming otherwise is refused.
        let raw = r#"{"code":"NotFound","message":"gone","retryable":true}"#;
        assert!(BoundaryError::from_json(raw).is_err());
        // …while a payload that omits the field is accepted and re-derived.
        let raw = r#"{"code":"Conflict","message":"lease moved"}"#;
        let parsed = BoundaryError::from_json(raw).expect("retryable is derived");
        assert!(parsed.retryable);
        assert_eq!(parsed.retry_class(), RetryClass::Bounded);
        // A blank message is not a message.
        let raw = r#"{"code":"Internal","message":"   "}"#;
        assert!(BoundaryError::from_json(raw).is_err());
    }

    #[test]
    fn cause_chains_are_preserved_in_process_and_bounded_on_the_wire() {
        /// A two-link chain, so the walk is actually a walk and not a single hop.
        #[derive(Debug)]
        struct Mid(std::io::Error);
        impl fmt::Display for Mid {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("vault.db unreadable")
            }
        }
        impl StdError for Mid {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&self.0)
            }
        }

        let err = KernelError::unavailable("the vault is not answering")
            .with_cause(Mid(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file",
            )))
            .with_diagnostic("rusqlite::Connection::open(/home/u/.everyaios/vault.db)");
        let chain = err.cause_chain();
        assert_eq!(chain.len(), 2, "the whole chain is walkable: {chain:?}");
        assert!(chain[0].contains("vault.db unreadable"));
        assert!(chain[1].contains("no such file"));
        assert_eq!(
            err.diagnostic(),
            Some("rusqlite::Connection::open(/home/u/.everyaios/vault.db)")
        );

        let wire = err.into_boundary();
        // The top cause label survives in a scrubbed, bounded form…
        assert!(wire.cause.as_deref().unwrap().contains("unreadable"));
        // …the path is gone, and the diagnostic never left the process.
        assert!(!wire.to_json().contains("/home/u"));
        assert!(!wire.to_json().contains("rusqlite"));
    }

    /// The secret-corpus scan, as a test: nothing credential-shaped or
    /// path-shaped survives the crossing, however the error was built.
    #[test]
    fn no_secret_or_internal_detail_crosses_the_boundary() {
        let secrets = [
            "api_key=sk-live-51H8xQ2eZvKYlo2C",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig",
            "password: hunter2",
            "client_secret: ab12cd34ef56",
            "OPENAI_API_KEY=zzz999",
        ];
        for secret in secrets {
            let err = KernelError::internal(format!("provider rejected the request: {secret}"))
                .with_diagnostic("reqwest::Response::json in vault/src/broker.rs:212");
            let wire = err.into_boundary();
            let json = wire.to_json();
            let (key, value) = secret.split_once(['=', ':']).expect("a credential shape");
            let value = value.trim_start_matches(['=', ':', ' ']);
            assert!(!json.contains(value), "{secret} leaked as {json}");
            assert!(
                json.contains(key),
                "the key name is the useful half: {json}"
            );
        }

        // A path in the message, and a path in the cause chain, are both removed.
        let err = KernelError::invalid_state("cannot read /home/sarv/.everyaios/vault.db")
            .with_cause(std::io::Error::other(
                "open C:\\Users\\sarv\\AppData\\vault.db: denied",
            ));
        let json = err.into_boundary().to_json();
        assert!(!json.contains("/home/sarv"), "{json}");
        assert!(!json.contains("C:\\Users"), "{json}");
        assert!(json.contains(REDACTED_PATH), "{json}");
    }

    #[test]
    fn messages_are_bounded_and_control_characters_are_dropped() {
        let long = "x".repeat(MAX_BOUNDARY_MESSAGE * 2);
        let wire = KernelError::internal(long).into_boundary();
        assert!(wire.message.chars().count() <= MAX_BOUNDARY_MESSAGE);
        assert!(wire.message.ends_with('…'));

        let mut injected = String::from("provider said: ");
        let _ = writeln!(injected, "\u{1b}[31mred\u{1b}[0m");
        let wire = KernelError::unavailable(injected).into_boundary();
        assert_eq!(wire.message, "provider said: [31mred[0m");
    }

    #[test]
    fn scrub_keeps_urls_and_ordinary_prose() {
        assert_eq!(
            scrub("provider https://api.example.com/v1 is unreachable", 240),
            "provider https://api.example.com/v1 is unreachable"
        );
        assert_eq!(scrub("3 / 4 layers applied", 240), "3 / 4 layers applied");
        assert_eq!(scrub("ratio 1/2", 240), "ratio 1/2");
        // A bare `/` at a token boundary with no second separator is not a path.
        assert_eq!(scrub("retry in 5/1 attempts", 240), "retry in 5/1 attempts");
    }

    #[test]
    fn conversion_from_std_errors_keeps_the_cause_in_process() {
        let io = KernelError::from(std::io::Error::other("connection reset by peer"));
        assert_eq!(io.code(), ErrorCode::Unavailable);
        assert_eq!(io.cause_chain().len(), 1);
        let wire = io.into_boundary();
        assert!(wire.retryable, "Unavailable is retryable with backoff");
        assert!(wire.cause.as_deref().unwrap().contains("connection reset"));

        let json_err = KernelError::from(serde_json::from_str::<u32>("nope").unwrap_err());
        assert_eq!(json_err.code(), ErrorCode::InvalidState);
        assert!(!json_err.into_boundary().retryable);
    }

    #[test]
    fn display_never_spills_the_diagnostic() {
        let err = KernelError::internal("provider registry is empty")
            .with_diagnostic("registry.rs:88 — the catalog sync never ran");
        let shown = err.to_string();
        assert!(shown.contains("Internal: provider registry is empty"));
        assert!(!shown.contains("registry.rs"), "{shown}");
    }

    #[test]
    fn boundary_payloads_round_trip_through_json() {
        // A cause is an `Error`, so a caller that wants to say "version 7 != 8"
        // has to mean it as one. This tiny newtype keeps the test honest about
        // the bound rather than reaching past it.
        #[derive(Debug)]
        struct VersionMismatch;
        impl fmt::Display for VersionMismatch {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("version 7 != 8")
            }
        }
        impl StdError for VersionMismatch {}

        let wire = KernelError::conflict("the lease moved to another owner")
            .with_cause(VersionMismatch)
            .into_boundary();
        assert_eq!(wire.cause.as_deref(), Some("version 7 != 8"));
        let parsed = BoundaryError::from_json(&wire.to_json()).expect("round-trip");
        assert_eq!(parsed, wire);
        assert!(parsed.retryable);
    }
}
