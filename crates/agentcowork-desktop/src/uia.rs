//! FIX-17 / `TASK-CUA-001` — the structured-UI (UIA) **collector**: identity,
//! bounds, isolation, elevation and gap discipline.
//!
//! `ARCH/24-COMPUTER-USE.md` §3 and `ARCH/21-WORLD-MODEL.md` §3–§5 are the
//! contract. Four findings from the v0 / world-model research
//! (`ARCH/42-EVIDENCE-MAP.md` §4 `FIX-17`; `ARCHIVE/v1-research/world-model-verification.md`
//! §1 claim A, §2.2) are re-verified against this tree and fixed here:
//!
//! 1. **`AutomationId` is a hint, never a key.** MS documents it as optional,
//!    sibling-scoped and *not stable across builds*. It is carried on
//!    [`UiaNode`] as a hint, may only *narrow* a candidate set, and is never the
//!    sole selector ([`resolve`]). The element identity is
//!    `(runtime_id | role+name+automationId+bounds)` scoped to one snapshot epoch
//!    ([`ElementHandle`]).
//! 2. **UIAccess / elevation.** A medium-integrity client cannot read elevated
//!    UI at all, and a provider fails with access-denied rather than returning
//!    "no children". That must degrade **loudly** — a typed
//!    [`UiaReadStatus::Unknown`] plus an [`UnknownRegion`] — never as a `None`
//!    tree that reads like "this app exposes no accessibility tree"
//!    (`REQ-CUA-006`; its failure case "unreachable region silently reported
//!    empty" is a defect).
//! 3. **Bounded, isolated reads.** There is no MS-documented client-side UIA
//!    timeout (finding F5: cross-process synchronous reads can stall or lag), so
//!    the collector owns a per-call budget and runs the walk on a worker whose
//!    incremental output the caller can read *while it is still running*. A hung
//!    provider therefore yields a **partial** read, never a stall
//!    (`REQ-CUA-004`).
//! 4. **Ambiguity is rejected, not guessed.** `ReadNode::find_by_name` returned
//!    the first `contains` match, so two "Save" buttons acted on whichever came
//!    first. [`resolve`] returns [`Resolution::Ambiguous`] with every candidate
//!    named, and the act path stops there (`REQ-CUA-003`; `ARCH/24` §8
//!    "Ambiguous element → Reject; re-read; escalate to user rather than guess").
//!
//! Everything here is platform-neutral and holds no COM: the real client lives
//! in [`crate::platform::win`] behind the [`UiaProvider`] seam, so the identity,
//! ambiguity, readiness and timeout rules in this module are exercised by tests
//! on **every** host with fakes. What stays Windows-only is the COM client
//! itself, and therefore its runtime behaviour.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::now_ms;
use crate::types::{ActKind, ReadNode, Region, WindowInfo};

/// Node ceiling for one bounded read.
///
/// The value is the open-codex bound the architecture cites (`ARCH/24` §3:
/// "mirrors open-codex bounds 1200/64/500"). A read that reaches the ceiling is
/// reported `Partial` with [`UnknownRegionKind::ProviderBound`], never as a
/// complete tree.
pub const MAX_NODES: u32 = 1200;

/// Depth ceiling for one bounded read (`ARCH/24` §3, the `64`).
pub const MAX_DEPTH: u32 = 64;

/// Per-field text ceiling, in characters (`ARCH/24` §3, the `500`). Cutting a
/// long name is recorded on the node ([`UiaNode::truncated`]) rather than being
/// silent.
pub const MAX_TEXT_CHARS: usize = 500;

/// Per-call budget for one structured read. No MS-documented UIA timeout exists,
/// so this is ours (`ARCH/24` §3, `OQ-WM-4`).
pub const READ_BUDGET: Duration = Duration::from_millis(1500);

/// The placeholder emitted instead of a protected field's text. Fixed, so a
/// consumer can recognise it and never has to infer "this was a secret".
pub const MASKED: &str = "••••••";

/// A unique, opaque element handle for the duration of one traversal — the
/// `runtime_id` in practice. Never an `AutomationId` (that is a hint).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UiaHandle(pub u64);

/// One element's readable properties, as the collector consumes them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UiaNode {
    /// Opaque per-traversal handle (`runtime_id` on Windows).
    pub handle: UiaHandle,
    /// Control-type name (`Button`, `Edit`, `Text`, …).
    pub role: String,
    /// Accessible name. Masked when [`Self::is_password`]; truncated to
    /// [`MAX_TEXT_CHARS`].
    pub name: String,
    /// `AutomationId` — a **hint only**: optional, sibling-scoped, not stable
    /// across builds. It may narrow a candidate set and is never the sole
    /// selector (see [`resolve`]).
    pub automation_id: Option<String>,
    /// The element's screen-space bounding rectangle.
    pub bounds: Region,
    /// `IsPassword` — a protected field. Its text is masked in the observation
    /// and its value is never emitted (`REQ-CUA-009` masked-field rule,
    /// `ARCH/21` §5.3).
    pub is_password: bool,
    /// `IsOffscreen` — providers may expose only visible nodes, so a lazy or
    /// virtualized subtree is *absent* rather than empty (finding F6).
    pub is_offscreen: bool,
    /// `IsEnabled`.
    pub is_enabled: bool,
    /// Set by the collector when a name was cut at [`MAX_TEXT_CHARS`], so a
    /// consumer can tell a short name from a long one that was truncated.
    pub truncated: bool,
}

impl UiaNode {
    /// A node with the usual defaults, for providers and tests.
    pub fn new(handle: u64, role: &str, name: &str, bounds: Region) -> Self {
        Self {
            handle: UiaHandle(handle),
            role: role.to_string(),
            name: name.to_string(),
            automation_id: None,
            bounds,
            is_password: false,
            is_offscreen: false,
            is_enabled: true,
            truncated: false,
        }
    }
}

/// The bounds applied to one read, recorded on the observation so a consumer
/// can tell a small window from a truncated one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadBounds {
    pub max_nodes: u32,
    pub max_depth: u32,
    pub max_text_chars: usize,
    pub budget_ms: u64,
}

impl Default for ReadBounds {
    fn default() -> Self {
        Self {
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            max_text_chars: MAX_TEXT_CHARS,
            budget_ms: READ_BUDGET.as_millis() as u64,
        }
    }
}

/// A snapshot generation — UIA's per-source epoch is the snapshot generation
/// (`ARCH/21` §4). Every observation is stamped with one, and a handle from a
/// different generation is stale by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotEpoch(pub u64);

impl std::fmt::Display for SnapshotEpoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The identity of one observable, as `ARCH/21` §3 (DM-026) states it: a
/// desktop UI element is **not persistent** — it is an epoch-scoped handle
/// `(runtime_id | role+name+automationId+bounds)` valid for one
/// observation/action, and `AutomationId` is a hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementHandle {
    /// The generation this handle was observed in. **One action only.**
    pub epoch: SnapshotEpoch,
    /// The provider's opaque element id (`runtime_id`) when it has one.
    pub runtime_id: Option<String>,
    /// Control type.
    pub role: String,
    /// Accessible name — already masked when the field is protected.
    pub name: String,
    /// `AutomationId` hint, carried for diagnosis only; never a selector.
    pub automation_id_hint: Option<String>,
    /// Screen-space bounds at observation time.
    pub bounds: Region,
    /// Wall-clock milliseconds when the observation was taken.
    pub observed_at_ms: u64,
    /// The provider's snapshot id for the source, kept beside [`Self::epoch`]
    /// because a bounded read advances the local generation on every call.
    pub snapshot: u64,
    /// Whether the provider marked this field protected (its text is masked).
    pub protected_field: bool,
}

impl ElementHandle {
    /// Build the handle form of a collected node.
    pub fn from_node(
        epoch: SnapshotEpoch,
        snapshot: u64,
        node: &UiaNode,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            epoch,
            runtime_id: Some(format!("runtime:{}", node.handle.0)),
            role: node.role.clone(),
            name: node.name.clone(),
            automation_id_hint: node.automation_id.clone(),
            bounds: node.bounds,
            observed_at_ms,
            snapshot,
            protected_field: node.is_password,
        }
    }

    /// The strong identity fields, `AutomationId` excluded.
    pub fn matches(&self, other: &Self) -> bool {
        self.role == other.role
            && self.name == other.name
            && self.bounds == other.bounds
            && self.runtime_id == other.runtime_id
    }

    /// The identity that survives a re-read of the same UI: strong fields equal
    /// **and** the same `AutomationId` hint when both have one.
    pub fn same_element(&self, other: &Self) -> bool {
        self.matches(other) && self.automation_id_hint == other.automation_id_hint
    }

    /// Is this handle the current observation?
    pub fn is_current(&self, current: SnapshotEpoch) -> bool {
        self.epoch == current
    }

    /// The age of this observation in milliseconds, for a freshness line.
    pub fn age_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.observed_at_ms)
    }

    /// One line naming the element and its age, for guidance and audit rows.
    pub fn describe(&self, now: u64) -> String {
        format!(
            "{}{} (automationId hint {:?}, observed {}ms ago, epoch {})",
            self.role,
            if self.name.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", self.name)
            },
            self.automation_id_hint,
            self.age_ms(now),
            self.epoch.0
        )
    }
}

/// A resolved element, valid for one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationHandle {
    pub element: ElementHandle,
    /// Where the element sits in the bounded tree (`1.3.2`), for re-resolution
    /// after a re-read.
    pub index_path: String,
}

impl ObservationHandle {
    /// Is this observation still the current one? A stale observation is
    /// re-validated before anything is actuated (`REQ-CUA-003`).
    pub fn validate(&self, current: SnapshotEpoch, now: u64) -> ObservationValidity {
        if self.element.is_current(current) {
            ObservationValidity {
                is_current: true,
                reason: None,
            }
        } else {
            ObservationValidity {
                is_current: false,
                reason: Some(format!(
                    "this observation is from epoch {} and the current epoch is {} — re-read the \
                     window and re-resolve; a UIA tree is lazy and changes, so a cached element is \
                     not an identity ({} observed {}ms ago)",
                    self.element.epoch.0,
                    current.0,
                    self.element.describe(now),
                    self.element.age_ms(now)
                )),
            }
        }
    }
}

/// Why an observation may or may not be trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationValidity {
    /// Is the observation the current one?
    pub is_current: bool,
    /// Why not, when it is not.
    pub reason: Option<String>,
}

/// Why a structured read could not deliver a complete, trustworthy tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum UiaFault {
    /// The client is not allowed to read this target (elevated window, SYSTEM
    /// UI, a protected surface). The region is **unknown** and no input may be
    /// synthesized into it.
    ElevationBlocked { detail: String },
    /// The provider hung, exceeded a bound, or a property read failed.
    ProviderUnresponsive { detail: String },
    /// The window handle could not be turned into a UI Automation element at all
    /// (destroyed window, reused handle, another desktop).
    TargetUnavailable { detail: String },
    /// The target exposes a structural UI, so the read is genuinely absent rather
    /// than blocked — the one fault whose honest answer is "no tree".
    NoTree { detail: String },
    /// The read failed for a reason with no better classification.
    ProviderError { detail: String },
}

impl UiaFault {
    /// Stable wire string for audit rows and the Tauri surface.
    pub fn kind(&self) -> &'static str {
        match self {
            UiaFault::ElevationBlocked { .. } => "elevation_blocked",
            UiaFault::ProviderUnresponsive { .. } => "provider_unresponsive",
            UiaFault::TargetUnavailable { .. } => "target_unavailable",
            UiaFault::NoTree { .. } => "no_tree",
            UiaFault::ProviderError { .. } => "provider_error",
        }
    }

    /// Does this fault mean "I could not look", rather than "there is nothing
    /// there"? Drives [`UiaReadStatus`]: the second is the vision-rung trigger,
    /// the first is a hard stop.
    pub fn is_blocked(&self) -> bool {
        matches!(
            self,
            UiaFault::ElevationBlocked { .. } | UiaFault::TargetUnavailable { .. }
        )
    }

    /// The region this fault makes unknown.
    pub fn unknown(&self) -> UnknownRegion {
        let now = now_ms();
        let (kind, detail) = match self {
            UiaFault::ElevationBlocked { detail } => (UnknownRegionKind::Elevation, detail),
            UiaFault::ProviderUnresponsive { detail } => (UnknownRegionKind::ProviderHung, detail),
            UiaFault::TargetUnavailable { detail } => {
                (UnknownRegionKind::TargetUnavailable, detail)
            }
            UiaFault::NoTree { detail } => (UnknownRegionKind::NoTree, detail),
            UiaFault::ProviderError { detail } => (UnknownRegionKind::ProviderError, detail),
        };
        UnknownRegion {
            kind,
            detail: detail.clone(),
            observed_at_ms: now,
        }
    }

    /// The raw detail this fault carries, for a receipt or an audit row.
    pub fn detail(&self) -> &str {
        match self {
            UiaFault::ElevationBlocked { detail }
            | UiaFault::ProviderUnresponsive { detail }
            | UiaFault::TargetUnavailable { detail }
            | UiaFault::NoTree { detail }
            | UiaFault::ProviderError { detail } => detail,
        }
    }

    /// One actionable sentence for a UI card / agent guidance. Never a bare error
    /// code.
    pub fn guidance(&self) -> String {
        match self {
            UiaFault::ElevationBlocked { .. } => {
                "this window runs elevated and this process is not \
                 UIAccess-enabled, so its UI cannot be read; the region is marked unknown — no \
                 input is synthesized into it, and that step needs a human in an elevated context"
                    .into()
            }
            UiaFault::ProviderUnresponsive { detail } => format!(
                "the accessibility provider did not answer inside the {}ms per-call budget \
                 ({detail}); the tree is partial and must be re-read — the OCR / vision rung may \
                 be used instead",
                READ_BUDGET.as_millis()
            ),
            UiaFault::TargetUnavailable { detail } => format!(
                "this window handle could not be resolved to a UI Automation element ({detail}); \
                 the target may have been closed, or its handle reused"
            ),
            UiaFault::NoTree { detail } => format!(
                "this window exposes no accessibility tree ({detail}) — use the OCR / vision rung \
                 (a capture of the window), not a structure that does not exist"
            ),
            UiaFault::ProviderError { detail } => {
                format!("the accessibility read failed ({detail})")
            }
        }
    }
}

/// What kind of thing a region of the target we could not describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownRegionKind {
    /// Beyond this process's integrity level (no UIAccess).
    Elevation,
    /// The provider never answered inside the budget.
    ProviderHung,
    /// A node or depth bound stopped the walk.
    ProviderBound,
    /// A read failed mid-walk.
    ProviderError,
    /// The window handle itself could not be resolved.
    TargetUnavailable,
    /// The provider only exposes visible content, so the offscreen/lazy subtree
    /// is absent rather than empty (finding F6).
    LazySubtree,
    /// The target genuinely has no structural UI — the honest "no tree".
    NoTree,
}

impl UnknownRegionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            UnknownRegionKind::Elevation => "elevation",
            UnknownRegionKind::ProviderHung => "provider_hung",
            UnknownRegionKind::ProviderBound => "provider_bound",
            UnknownRegionKind::ProviderError => "provider_error",
            UnknownRegionKind::TargetUnavailable => "target_unavailable",
            UnknownRegionKind::LazySubtree => "lazy_subtree",
            UnknownRegionKind::NoTree => "no_tree",
        }
    }
}

/// A part of the target this read could not describe, with the typed reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnknownRegion {
    pub kind: UnknownRegionKind,
    pub detail: String,
    pub observed_at_ms: u64,
}

impl UnknownRegion {
    /// One actionable sentence. This is the sentence a card or an agent
    /// projection shows; it is never an error code on its own.
    pub fn guidance(&self) -> String {
        match self.kind {
            UnknownRegionKind::Elevation => format!(
                "part of this window is elevated and unreadable without UIAccess ({}) — marked \
                 unknown, no input is synthesized into it",
                self.detail
            ),
            UnknownRegionKind::ProviderHung => format!(
                "the accessibility provider did not answer inside the {}ms budget ({}) — the tree \
                 is partial, re-read before trusting it",
                READ_BUDGET.as_millis(),
                self.detail
            ),
            UnknownRegionKind::ProviderBound => format!(
                "the bounded read stopped at a declared bound ({}) — the rest of the tree is \
                 unknown, scroll/paginate and re-read",
                self.detail
            ),
            UnknownRegionKind::ProviderError => format!(
                "a mid-walk accessibility read failed ({}) — the tree is partial, re-read",
                self.detail
            ),
            UnknownRegionKind::TargetUnavailable => format!(
                "this window handle resolved to no accessibility element ({}) — the target may \
                 have been closed or its handle reused",
                self.detail
            ),
            UnknownRegionKind::LazySubtree => format!(
                "this provider only exposes visible content, so the offscreen/lazy subtree is \
                 absent rather than empty ({})",
                self.detail
            ),
            UnknownRegionKind::NoTree => format!(
                "this window exposes no accessibility tree ({}) — use the OCR / vision rung",
                self.detail
            ),
        }
    }
}

/// How much of the target this process is entitled to describe.
///
/// Derived from the process's own integrity level, never from the emptiness of a
/// result: a region we are not allowed to see is unknown even when the walk also
/// found readable nodes (`ARCH/21` §7: "Collect what is reachable; mark elevated
/// regions unknown").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// Everything the target exposes to this process is readable.
    Complete,
    /// Part of the target is beyond this process's integrity level.
    Restricted { elevated: bool },
}

impl Coverage {
    /// Can a structured read describe the whole target?
    pub fn is_complete(&self) -> bool {
        matches!(self, Coverage::Complete)
    }

    /// The unknown region this coverage makes, if any.
    pub fn unknown(&self) -> Option<UnknownRegion> {
        match self {
            Coverage::Complete => None,
            Coverage::Restricted { elevated } => Some(UnknownRegion {
                kind: UnknownRegionKind::Elevation,
                detail: if *elevated {
                    "the target process runs at a higher integrity level than this client (medium \
                     IL without UIAccess cannot read elevated UI; SYSTEM UI is unreachable \
                     entirely)"
                        .into()
                } else {
                    "this client's integrity level is restricted for this target".into()
                },
                observed_at_ms: now_ms(),
            }),
        }
    }
}

/// The honesty state of one structured read.
///
/// `Option<ReadNode>` alone cannot express "I was not allowed to look", which is
/// how an elevation-blocked window used to come back as an empty tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum UiaReadStatus {
    /// A bounded, fully walked tree.
    Complete,
    /// Some of the target was walked; the rest is unknown (budget, bound, lazy or
    /// elevated region). Re-read before trusting it, and never treat the missing
    /// part as "absent".
    Partial { detail: String },
    /// The target exposes no structural UI at all. This is a **positive** fact,
    /// not a gap, and the documented next rung is OCR / vision
    /// (`ARCH/24` §8 "Tree empty / semantics poor → OCR patch → vision rung").
    Absent { detail: String },
    /// The target could not be described at all — permission, a dead handle, or a
    /// provider that never answered. The region is **unknown**: no absence may be
    /// inferred from it and no input may be synthesized into it.
    Unknown { detail: String },
}

impl UiaReadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            UiaReadStatus::Complete => "complete",
            UiaReadStatus::Partial { .. } => "partial",
            UiaReadStatus::Absent { .. } => "absent",
            UiaReadStatus::Unknown { .. } => "unknown",
        }
    }

    /// May a consumer infer "this element does not exist" from this read?
    ///
    /// Only from a read that covered the whole target: `Complete` (walked
    /// everything) or `Absent` (established there is no structural UI). This one
    /// predicate is what stops an elevation-blocked target from being reported as
    /// an empty dialog, and it is the read-side half of the ladder's `Unknown`
    /// rung delivery (`REQ-CUA-006`, `REQ-CUA-003`).
    pub fn may_infer_absence(&self) -> bool {
        matches!(self, UiaReadStatus::Complete | UiaReadStatus::Absent { .. })
    }

    /// Did we get enough structure to act on what we *did* see?
    pub fn is_usable(&self) -> bool {
        !matches!(
            self,
            UiaReadStatus::Absent { .. } | UiaReadStatus::Unknown { .. }
        )
    }
}

/// What a consumer may conclude from a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsencePolicy {
    /// The read covered the target, so "not present" is a fact.
    AbsenceInferable,
    /// The read was partial, so "not present" is unproven.
    AbsenceNotInferable,
    /// The read could not look; nothing at all may be concluded.
    RegionUnknown,
}

/// A freshness anomaly — `ARCH/21` §4: "Never a silent gap: record a freshness
/// anomaly event."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessAnomaly {
    /// Stable wire string.
    pub kind: String,
    /// The smallest known scope that has to be re-read.
    pub scope: String,
    /// What happened, verbatim.
    pub detail: String,
    /// When it was recorded.
    pub observed_at_ms: u64,
}

/// One element as the observation recorded it, with its place in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedElement {
    pub handle: ElementHandle,
    /// Index path in the bounded tree (`1.3.2`).
    pub index_path: String,
}

impl ObservedElement {
    /// The control type.
    pub fn role(&self) -> &str {
        &self.handle.role
    }
    /// The accessible name (masked for a protected field).
    pub fn name(&self) -> &str {
        &self.handle.name
    }
    /// The `AutomationId` hint — for diagnosis, never for selection.
    pub fn automation_id_hint(&self) -> Option<&str> {
        self.handle.automation_id_hint.as_deref()
    }
    /// The click point (centre of the observed bounds).
    pub fn center(&self) -> (i32, i32) {
        self.handle.bounds.center()
    }
    /// A one-line description for a refusal or an audit row.
    pub fn describe(&self) -> String {
        format!(
            "{}{} at [{}] (automationId hint {:?})",
            self.handle.role,
            if self.handle.name.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", self.handle.name)
            },
            self.index_path,
            self.handle.automation_id_hint
        )
    }
}

/// The result of one bounded structured read.
///
/// Every field is a `REQ-CUA-003` / `REQ-CUA-004` fact rather than a
/// convenience: the epoch, the bounds that were applied, the honesty state, the
/// regions that are unknown, and the anomalies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredRead {
    pub window_id: u64,
    /// The generation this observation belongs to — an element handle from any
    /// other epoch is stale.
    pub epoch: SnapshotEpoch,
    /// Milliseconds since the Unix epoch.
    pub observed_at_ms: u64,
    /// The bounds that were applied (nodes · depth · text · budget).
    pub bounds: ReadBounds,
    /// Complete / Partial / Absent / Unknown. Never inferred from the tree's
    /// emptiness.
    pub status: UiaReadStatus,
    /// The bounded tree, when anything at all was read.
    pub tree: Option<ReadNode>,
    /// Every element the walk observed, in document order — the corpus a
    /// resolution matches against.
    pub elements: Vec<ObservedElement>,
    /// Regions of the target this read could not describe.
    pub unknowns: Vec<UnknownRegion>,
    /// Freshness anomalies recorded during this read (a gap forces a bounded
    /// rescan and is never silent).
    pub anomalies: Vec<FreshnessAnomaly>,
}

impl StructuredRead {
    /// The epoch an element handle must match to be acted on.
    pub fn current_epoch(&self) -> SnapshotEpoch {
        self.epoch
    }

    /// What a consumer may conclude from this read.
    pub fn absence_policy(&self) -> AbsencePolicy {
        if self.status.may_infer_absence() {
            AbsencePolicy::AbsenceInferable
        } else if self.status.is_usable() {
            AbsencePolicy::AbsenceNotInferable
        } else {
            AbsencePolicy::RegionUnknown
        }
    }

    /// May absence be inferred? Named alias so call sites read the rule.
    pub fn may_infer_absence(&self) -> bool {
        may_infer_absence(&self.status)
    }

    /// Did this read mark any region unknown (elevation, bound, hang)?
    pub fn has_unknown_regions(&self) -> bool {
        !self.unknowns.is_empty()
    }

    /// The composed guidance for a UI card or an agent projection: the status
    /// sentence (when the read is not clean), then the first unknown region, then
    /// the provider hint. `None` only for a clean complete read. Derived rather
    /// than stored, so it cannot drift from the facts it describes.
    pub fn guidance(&self, window: &WindowInfo) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        match &self.status {
            UiaReadStatus::Complete => {}
            UiaReadStatus::Partial { detail }
            | UiaReadStatus::Absent { detail }
            | UiaReadStatus::Unknown { detail } => {
                if !parts.contains(detail) {
                    parts.push(detail.clone());
                }
            }
        }
        if let Some(u) = self.unknowns.first()
            && !parts.iter().any(|p| p.contains(u.kind.as_str()))
        {
            parts.push(u.guidance());
        }
        if let Some(hint) = provider_hint(window) {
            parts.push(hint.guidance);
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }

    /// The node count, for a bounded-read receipt.
    pub fn node_count(&self) -> usize {
        self.elements.len()
    }

    /// The status + epoch as a compact JSON value, for the Tauri surface.
    pub fn to_json(&self, window: &WindowInfo) -> serde_json::Value {
        serde_json::json!({
            "windowId": self.window_id,
            "epoch": self.epoch.0,
            "observedAtMs": self.observed_at_ms,
            "status": self.status.as_str(),
            "detail": self.status_detail(),
            "mayInferAbsence": self.may_infer_absence(),
            "nodeCount": self.node_count(),
            "bounds": {
                "maxNodes": self.bounds.max_nodes,
                "maxDepth": self.bounds.max_depth,
                "maxTextChars": self.bounds.max_text_chars,
                "budgetMs": self.bounds.budget_ms,
            },
            "unknownRegions": self
                .unknowns
                .iter()
                .map(|u| serde_json::json!({
                    "kind": u.kind.as_str(),
                    "detail": u.detail,
                }))
                .collect::<Vec<_>>(),
            "anomalies": self
                .anomalies
                .iter()
                .map(|a| serde_json::json!({
                    "kind": a.kind,
                    "scope": a.scope,
                    "detail": a.detail,
                }))
                .collect::<Vec<_>>(),
            "guidance": self.guidance(window),
        })
    }

    /// The status's own sentence, for a receipt.
    pub fn status_detail(&self) -> String {
        match &self.status {
            UiaReadStatus::Complete => "the whole readable target was walked".into(),
            UiaReadStatus::Partial { detail }
            | UiaReadStatus::Absent { detail }
            | UiaReadStatus::Unknown { detail } => detail.clone(),
        }
    }
}

/// May a consumer infer "this element does not exist" from a read with this
/// status? A thin alias so call sites name the rule, not the enum.
pub fn may_infer_absence(status: &UiaReadStatus) -> bool {
    status.may_infer_absence()
}

// ---------------------------------------------------------------------------
// The provider seam (FIX-17's injectable boundary)
// ---------------------------------------------------------------------------

/// One UI Automation call the collector can make. A real client maps a
/// control-view element plus `GetFirstChildElement` / `GetNextSiblingElement` /
/// `CurrentX` onto these; a fake answers from a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiaCall {
    /// The window's root element.
    Root,
    /// The first child of an element.
    FirstChild(UiaHandle),
    /// The next sibling of an element.
    NextSibling(UiaHandle),
    /// The properties of an element.
    Properties(UiaHandle),
}

/// A provider's answer to one call.
#[derive(Debug, Clone, PartialEq)]
pub enum UiaAnswer {
    /// No such element / end of the child list. An honest end, not a failure.
    Absent,
    Element(Box<UiaNode>),
}

/// A UI Automation client, narrowed to what the collector needs.
///
/// The real implementation lives in [`crate::platform::win`] (Windows-only, COM);
/// everything here is exercised on every host with fakes, which is what makes the
/// identity, ambiguity, readiness and timeout rules testable off Windows.
pub trait UiaProvider: Send {
    /// The source's current snapshot generation. Zero means "the source does not
    /// report one", in which case the collector stamps its own per-scope
    /// generation.
    fn epoch(&self) -> SnapshotEpoch;
    /// What this client may read of the target — from its own integrity level,
    /// never from the emptiness of a result.
    fn coverage(&mut self, window: &WindowInfo) -> Coverage;
    /// One bounded UIA call. `Err` is a typed fault, never a bare `None`.
    fn call(&mut self, window: &WindowInfo, call: UiaCall) -> Result<UiaAnswer, UiaFault>;
}

/// What a provider's own kind means for a structured read.
///
/// Chromium's UI Automation provider is opt-in (`--enable-features=UiaProvider`)
/// and its default Windows surface is MSAA/`IAccessible2`, which is coarser and
/// lags the real tree (finding F4). So browser content must not be put on the
/// UIA path (`ARCH/21` §10 item 4, §7): the read still happens, but the result
/// is annotated so page content is read through the browser rung (CDP,
/// `ARCH/23`) instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHint {
    /// Stable wire string.
    pub id: &'static str,
    /// The provider family as named by the OS.
    pub name: String,
    /// What to do instead.
    pub guidance: String,
}

/// Window classes whose content belongs on the browser rung.
const BROWSER_CLASS_SIGNALS: &[&str] = &["chrome_widgetwin", "mozillawindowclass", "electron"];
/// Process/title names whose content belongs on the browser rung.
const BROWSER_NAME_SIGNALS: &[&str] = &[
    "chrome", "chromium", "msedge", "firefox", "brave", "opera", "vivaldi", "electron",
];

/// Classify the target's provider family. Pure, and therefore testable on any
/// host: it is a classifier over the window's reported class (the Windows
/// backend's `app` field carries the window class) and title.
pub fn provider_hint(window: &WindowInfo) -> Option<ProviderHint> {
    let class = window.app.to_ascii_lowercase();
    let title = window.title.to_ascii_lowercase();
    if BROWSER_CLASS_SIGNALS.iter().any(|s| class.contains(s)) {
        return Some(browser_hint(
            window,
            "the target is a Chromium/Electron window",
        ));
    }
    if BROWSER_NAME_SIGNALS
        .iter()
        .any(|s| title.contains(s) || class.contains(s))
    {
        return Some(browser_hint(
            window,
            "the target looks like a browser window",
        ));
    }
    None
}

fn browser_hint(window: &WindowInfo, why: &str) -> ProviderHint {
    ProviderHint {
        id: "browser",
        name: if window.app.is_empty() {
            "browser".into()
        } else {
            window.app.clone()
        },
        guidance: format!(
            "{why}: its UI Automation provider is opt-in and its default surface is coarser than \
             the real tree, so page content must be read through the browser rung (CDP, ARCH/23) — \
             not through this UIA read"
        ),
    }
}

// ---------------------------------------------------------------------------
// The bounded, isolated walk
// ---------------------------------------------------------------------------

/// One element as the worker found it, before the tree is assembled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedElement {
    pub parent: Option<UiaHandle>,
    pub node: UiaNode,
    pub path: String,
    pub depth: u32,
}

/// Where the worker streams what it has found so far. The caller can drain this
/// while the walk is still running, which is what makes a hung provider a
/// *partial* read instead of a stall.
pub trait UiaSink {
    /// Record one element. `&self` on purpose: the sink is shared with the
    /// front-end, which reads it *while the worker is still running*.
    fn push(&self, element: CollectedElement);
}

/// A mutex-guarded sink the front-end shares with the worker.
#[derive(Debug, Default)]
pub struct SharedSink {
    items: Mutex<Vec<CollectedElement>>,
}

impl SharedSink {
    pub fn new() -> Self {
        Self::default()
    }
    /// Take everything found so far.
    pub fn take(&self) -> Vec<CollectedElement> {
        self.items.lock().map(|g| g.clone()).unwrap_or_default()
    }
    /// How many elements have been found so far.
    pub fn len(&self) -> usize {
        self.items.lock().map(|g| g.len()).unwrap_or(usize::MAX)
    }
    /// Has nothing been found yet?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl UiaSink for SharedSink {
    fn push(&self, element: CollectedElement) {
        if let Ok(mut g) = self.items.lock() {
            g.push(element);
        }
    }
}

/// How the walk ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionOutcome {
    /// The whole walkable target was read within the bounds.
    Complete { nodes: u32 },
    /// The walk stopped early; `unknown` says why and what is missing.
    Partial { nodes: u32, unknown: UnknownRegion },
    /// Nothing could be read; the fault is the reason.
    Failed { fault: UiaFault },
}

/// The collector's own configuration for one read.
#[derive(Debug, Clone, Copy)]
pub struct CollectConfig {
    pub bounds: ReadBounds,
    /// The instant the read must not outlive.
    pub deadline: Instant,
}

impl CollectConfig {
    /// A config with the declared bounds and an explicit deadline.
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            bounds: ReadBounds::default(),
            deadline,
        }
    }

    /// A config with a shorter budget — the knob a caller uses to bound one
    /// read on a busy host.
    pub fn with_budget(budget: Duration) -> Self {
        Self {
            bounds: ReadBounds::default(),
            deadline: Instant::now() + budget,
        }
    }
}

impl Default for CollectConfig {
    fn default() -> Self {
        Self::with_budget(READ_BUDGET)
    }
}

/// Walk one target with the declared bounds, streaming every element to `sink`
/// as it is found.
///
/// **This runs on the worker**, never on the caller's thread. The deadline is
/// checked between calls, so a provider that is merely slow yields a `Partial`
/// outcome here; a provider that hangs *inside* a call is handled by
/// [`ReadSession`], where the caller stops waiting and keeps the partial stream.
pub fn collect<P: UiaProvider + ?Sized>(
    provider: &mut P,
    window: &WindowInfo,
    cfg: &CollectConfig,
    sink: &dyn UiaSink,
) -> CollectionOutcome {
    // Coverage is known before the walk starts, so an unreadable target costs
    // nothing and says so immediately.
    if let Some(u) = provider.coverage(window).unknown() {
        return CollectionOutcome::Partial {
            nodes: 0,
            unknown: u,
        };
    }
    let root = match provider.call(window, UiaCall::Root) {
        Ok(UiaAnswer::Element(n)) => *n,
        Ok(UiaAnswer::Absent) => {
            return CollectionOutcome::Failed {
                fault: UiaFault::NoTree {
                    detail: "the window handle resolved to no root element".into(),
                },
            };
        }
        Err(e) => return CollectionOutcome::Failed { fault: e },
    };
    let mut budget = cfg.bounds.max_nodes;
    let mut nodes = 0u32;
    let mut stack: Vec<CollectedElement> = vec![CollectedElement {
        parent: None,
        node: root,
        path: "1".into(),
        depth: 0,
    }];
    let mut partial: Option<UnknownRegion> = None;

    while let Some(element) = stack.pop() {
        if Instant::now() >= cfg.deadline {
            partial.get_or_insert(UnknownRegion {
                kind: UnknownRegionKind::ProviderHung,
                detail: format!("the walk stopped after {nodes} node(s) at the per-call deadline"),
                observed_at_ms: now_ms(),
            });
            break;
        }
        if budget == 0 {
            partial.get_or_insert(UnknownRegion {
                kind: UnknownRegionKind::ProviderBound,
                detail: format!("node budget of {} exhausted", cfg.bounds.max_nodes),
                observed_at_ms: now_ms(),
            });
            break;
        }
        if element.depth > cfg.bounds.max_depth {
            partial.get_or_insert(UnknownRegion {
                kind: UnknownRegionKind::ProviderBound,
                detail: format!("depth budget of {} exhausted", cfg.bounds.max_depth),
                observed_at_ms: now_ms(),
            });
            break;
        }
        budget -= 1;
        nodes += 1;

        // Properties are their own call: this is the call that fails on a
        // hostile or lazy provider, and it is masked/bounded here so no raw
        // protected text can reach an observation.
        let handle = element.node.handle;
        let node = match provider.call(window, UiaCall::Properties(handle)) {
            Ok(UiaAnswer::Element(n)) => *n,
            // A provider that answers the walk but not the properties is still a
            // usable node: the name simply stays absent.
            Ok(UiaAnswer::Absent) => element.node.clone(),
            Err(UiaFault::ElevationBlocked { detail }) => {
                return CollectionOutcome::Partial {
                    nodes,
                    unknown: UnknownRegion {
                        kind: UnknownRegionKind::Elevation,
                        detail,
                        observed_at_ms: now_ms(),
                    },
                };
            }
            Err(fault) => {
                partial.get_or_insert(UnknownRegion {
                    kind: UnknownRegionKind::ProviderError,
                    detail: format!("{}: {}", fault.kind(), fault.detail()),
                    observed_at_ms: now_ms(),
                });
                break;
            }
        };
        let node = sanitize(node, cfg.bounds.max_text_chars);
        let path = element.path.clone();
        let parent = element.parent;
        let role = node.role.clone();
        let offscreen = node.is_offscreen;
        let is_password = node.is_password;
        sink.push(CollectedElement {
            parent,
            node,
            path: path.clone(),
            depth: element.depth,
        });

        if offscreen {
            partial.get_or_insert(UnknownRegion {
                kind: UnknownRegionKind::LazySubtree,
                detail: format!(
                    "an offscreen element at {path} ({role}) was not walked — this provider \
                     exposes visible content only"
                ),
                observed_at_ms: now_ms(),
            });
        }
        if is_password {
            // A protected field's subtree is not a target: its contents are never
            // read (`ARCH/21` §5.3, `REQ-CUA-009`).
            continue;
        }

        // Children: first child, then the sibling chain. Pushed in reverse so the
        // depth-first walk visits them in document order.
        let mut children: Vec<CollectedElement> = Vec::new();
        let mut next = match provider.call(window, UiaCall::FirstChild(handle)) {
            Ok(UiaAnswer::Element(n)) => Some(*n),
            Ok(UiaAnswer::Absent) => None,
            Err(fault) => {
                partial.get_or_insert(UnknownRegion {
                    kind: UnknownRegionKind::ProviderError,
                    detail: format!("first child: {}: {}", fault.kind(), fault.detail()),
                    observed_at_ms: now_ms(),
                });
                None
            }
        };
        let mut i = 0usize;
        while let Some(child) = next.take() {
            if i as u32 >= cfg.bounds.max_nodes {
                partial.get_or_insert(UnknownRegion {
                    kind: UnknownRegionKind::ProviderBound,
                    detail: format!("sibling run exceeded {} children", cfg.bounds.max_nodes),
                    observed_at_ms: now_ms(),
                });
                break;
            }
            let child_handle = child.handle;
            children.push(CollectedElement {
                parent: Some(handle),
                node: child,
                path: format!("{path}.{}", i + 1),
                depth: element.depth + 1,
            });
            i += 1;
            next = match provider.call(window, UiaCall::NextSibling(child_handle)) {
                Ok(UiaAnswer::Element(n)) => Some(*n),
                Ok(UiaAnswer::Absent) => None,
                Err(fault) => {
                    partial.get_or_insert(UnknownRegion {
                        kind: UnknownRegionKind::ProviderError,
                        detail: format!("next sibling: {}: {}", fault.kind(), fault.detail()),
                        observed_at_ms: now_ms(),
                    });
                    None
                }
            };
        }
        children.reverse();
        stack.extend(children);
    }

    match partial {
        Some(u) => CollectionOutcome::Partial { nodes, unknown: u },
        None => CollectionOutcome::Complete { nodes },
    }
}

/// Mask a protected field and bound its text.
///
/// `IsPassword` is honoured here, once, so no consumer downstream can receive the
/// contents of a password field (`REQ-CUA-009`: capture is consent-gated **with
/// protected fields masked**; `ARCH/21` §5.3: "`IsPassword`/protected fields
/// excluded or masked"). Screen content is untrusted input (`ARCH/24` §8), and
/// the mask also keeps a payload hidden in a password field out of the
/// observation.
pub fn sanitize(mut node: UiaNode, max_text_chars: usize) -> UiaNode {
    if node.is_password {
        node.name = MASKED.to_string();
        node.truncated = false;
    } else if node.name.chars().count() > max_text_chars {
        node.name = node.name.chars().take(max_text_chars).collect();
        node.truncated = true;
    }
    node
}

// ---------------------------------------------------------------------------
// Worker isolation
// ---------------------------------------------------------------------------

/// What the worker streams back to the caller.
enum WorkerMsg {
    /// The coverage verdict, sent before the walk so a blocked target is known
    /// immediately.
    Coverage(Coverage),
    /// The walk finished.
    Done(Box<CollectionOutcome>),
}

/// A front-end handle to a running bounded read.
struct IsolatedRead {
    rx: Receiver<WorkerMsg>,
    cancelled: Arc<AtomicBool>,
}

/// The front-end handle to a running bounded read.
///
/// The worker streams its findings into a shared sink the caller can read **at any
/// moment**, which is what turns "the provider hung" into a partial observation
/// instead of a stalled agent (`REQ-CUA-004`).
pub struct ReadSession {
    read: Option<IsolatedRead>,
    sink: Arc<SharedSink>,
    outcome: Option<Box<CollectionOutcome>>,
    /// The coverage verdict the worker reported before the walk started, so a
    /// caller polling early learns about an elevation block without waiting for
    /// the whole walk.
    coverage: Option<Coverage>,
    started: Instant,
}

impl ReadSession {
    /// Wait up to `wait` for the walk to finish, then return what it produced.
    ///
    /// `None` means the worker is still running and the wait expired; the caller
    /// then has [`Self::partial_elements`] and decides whether to keep waiting or
    /// to declare the read partial.
    pub fn poll(&mut self, wait: Duration) -> Option<&CollectionOutcome> {
        let Some(read) = self.read.as_ref() else {
            return self.outcome.as_deref();
        };
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match read.rx.recv_timeout(remaining) {
                Ok(WorkerMsg::Coverage(c)) => {
                    self.coverage = Some(c);
                    continue;
                }
                Ok(WorkerMsg::Done(outcome)) => {
                    self.outcome = Some(outcome);
                    read.cancelled.store(true, Ordering::SeqCst);
                    self.read = None;
                    return self.outcome.as_deref();
                }
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    // The worker is gone without a verdict: treat it as a hang so
                    // the observation is partial rather than silently empty.
                    self.read = None;
                    if self.outcome.is_none() {
                        self.outcome = Some(Box::new(CollectionOutcome::Partial {
                            nodes: 0,
                            unknown: UnknownRegion {
                                kind: UnknownRegionKind::ProviderHung,
                                detail: "the accessibility worker ended without a verdict".into(),
                                observed_at_ms: now_ms(),
                            },
                        }));
                    }
                    return self.outcome.as_deref();
                }
            }
        }
    }

    /// Everything the worker has found so far.
    pub fn partial_elements(&self) -> Vec<CollectedElement> {
        self.sink.take()
    }

    /// The coverage verdict, once the worker has reported one.
    pub fn coverage(&self) -> Option<Coverage> {
        self.coverage
    }

    /// The unknown region this read's coverage makes, if any.
    pub fn coverage_unknown(&self) -> Option<UnknownRegion> {
        self.coverage.and_then(|c| c.unknown())
    }

    /// How long this read has been running.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Ask the worker to stop at its next checkpoint.
    ///
    /// A worker already parked inside a hung provider call cannot observe this —
    /// the OS reclaims it when the process exits, and until then it holds only its
    /// own stack, never the caller's.
    pub fn cancel(&self) {
        if let Some(read) = self.read.as_ref() {
            read.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

/// Start a bounded read on a worker thread and return a handle to it.
///
/// `provider` is moved onto the worker, so the caller is never blocked by a
/// cross-process synchronous UIA call (`REQ-CUA-004`: "a per-call budget plus
/// worker isolation prevent a hung provider from stalling the agent").
pub fn start_read<P>(provider: P, window: WindowInfo, cfg: CollectConfig) -> ReadSession
where
    P: UiaProvider + 'static,
{
    let sink = Arc::new(SharedSink::new());
    let worker_sink = Arc::clone(&sink);
    let (tx, rx): (Sender<WorkerMsg>, Receiver<WorkerMsg>) = channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let coverage_window = window.clone();
    let walk_window = window;
    let moved = worker_sink;
    let _ = std::thread::Builder::new()
        .name("uia-read".into())
        .spawn(move || {
            let mut provider = provider;
            // The coverage verdict first, so a caller polling the session learns
            // about an elevation block without waiting for the whole walk.
            let _ = tx.send(WorkerMsg::Coverage(provider.coverage(&coverage_window)));
            let outcome = collect(&mut provider, &walk_window, &cfg, moved.as_ref());
            let _ = tx.send(WorkerMsg::Done(Box::new(outcome)));
        });
    ReadSession {
        read: Some(IsolatedRead { rx, cancelled }),
        sink,
        outcome: None,
        coverage: None,
        started,
    }
}

// ---------------------------------------------------------------------------
// Assembly: walk output → observation
// ---------------------------------------------------------------------------

/// Build a [`StructuredRead`] from a finished (or abandoned) walk.
///
/// Split out from the front-end loop so the tree/status/anomaly construction is
/// testable without a thread, and so one implementation serves every path.
pub fn assemble(
    window: &WindowInfo,
    bounds: ReadBounds,
    outcome: CollectionOutcome,
    items: Vec<CollectedElement>,
    epoch: SnapshotEpoch,
) -> StructuredRead {
    let observed_at_ms = now_ms();
    let snapshot = items.len() as u64;
    let scope = window_scope(window);

    let (status, unknowns) = match &outcome {
        CollectionOutcome::Complete { .. } => (UiaReadStatus::Complete, Vec::new()),
        CollectionOutcome::Partial { unknown, .. } => (
            UiaReadStatus::Partial {
                detail: unknown.guidance(),
            },
            vec![unknown.clone()],
        ),
        CollectionOutcome::Failed { fault } => match fault {
            // "No tree" is a positive fact about the target, not an unknown
            // region: the vision rung is the documented next step
            // (`ARCH/24` §8), and absence **may** be inferred from it.
            UiaFault::NoTree { .. } => (
                UiaReadStatus::Absent {
                    detail: fault.guidance(),
                },
                Vec::new(),
            ),
            other => (
                UiaReadStatus::Unknown {
                    detail: other.guidance(),
                },
                vec![other.unknown()],
            ),
        },
    };

    let (tree, elements) = build_tree(&items, epoch, snapshot, observed_at_ms);
    // A partial with nothing collected has no tree, and the elements corpus is
    // empty — the status is what a consumer must read.
    let status = if status.is_usable() && elements.is_empty() && !unknowns.is_empty() {
        UiaReadStatus::Unknown {
            detail: unknowns[0].guidance(),
        }
    } else {
        status
    };
    let anomalies: Vec<FreshnessAnomaly> = unknowns
        .iter()
        .filter(|u| {
            matches!(
                u.kind,
                UnknownRegionKind::ProviderHung
                    | UnknownRegionKind::ProviderBound
                    | UnknownRegionKind::ProviderError
                    | UnknownRegionKind::Elevation
            )
        })
        .map(|u| FreshnessAnomaly {
            kind: match u.kind {
                UnknownRegionKind::ProviderHung => "provider_hung",
                UnknownRegionKind::ProviderBound => "provider_bound",
                UnknownRegionKind::ProviderError => "provider_error",
                _ => "elevation_restricted",
            }
            .to_string(),
            scope: scope.clone(),
            detail: u.guidance(),
            observed_at_ms,
        })
        .collect();
    StructuredRead {
        window_id: window.id,
        epoch,
        observed_at_ms,
        bounds,
        status,
        tree,
        elements,
        unknowns,
        anomalies,
    }
}

/// Build the bounded tree **and** the resolution corpus from the walk's output.
///
/// The corpus and the tree are built from the same pass on purpose: a resolution
/// can only ever match an element that is also in the tree the agent saw.
fn build_tree(
    items: &[CollectedElement],
    epoch: SnapshotEpoch,
    snapshot: u64,
    observed_at_ms: u64,
) -> (Option<ReadNode>, Vec<ObservedElement>) {
    if items.is_empty() {
        return (None, Vec::new());
    }
    let mut index: BTreeMap<UiaHandle, usize> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        index.insert(item.node.handle, i);
    }
    let bases: Vec<ReadNode> = items
        .iter()
        .map(|item| ReadNode {
            index_path: item.path.clone(),
            role: item.node.role.clone(),
            name: item.node.name.clone(),
            // Carried as a hint only — see the module docs and `resolve`.
            automation_id: item.node.automation_id.clone(),
            x: item.node.bounds.x,
            y: item.node.bounds.y,
            width: item.node.bounds.width,
            height: item.node.bounds.height,
            // UIA actionability is a *pattern* question (`Invoke`, `Value`, …),
            // re-queried per action, so the observation only records what it
            // knows: the control is not inert text/pane/group/image content.
            actionable: !matches!(
                item.node.role.as_str(),
                "Text" | "Pane" | "Group" | "Image" | "Custom"
            ),
            children: Vec::new(),
        })
        .collect();
    let elements: Vec<ObservedElement> = items
        .iter()
        .map(|item| ObservedElement {
            handle: ElementHandle::from_node(epoch, snapshot, &item.node, observed_at_ms),
            index_path: item.path.clone(),
        })
        .collect();
    // Document order per parent, and the roots, both derived in one forward pass.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item.parent.and_then(|p| index.get(&p)).copied() {
            Some(p) if p != i => children[p].push(i),
            _ => roots.push(i),
        }
    }
    fn build(i: usize, bases: &[ReadNode], children: &[Vec<usize>]) -> ReadNode {
        let mut node = bases[i].clone();
        // Depth is bounded by `ReadBounds::max_depth` (64), so this recursion is
        // bounded by construction, not by hope.
        node.children = children[i]
            .iter()
            .map(|c| build(*c, bases, children))
            .collect();
        node
    }
    let forest: Vec<ReadNode> = roots.iter().map(|i| build(*i, &bases, &children)).collect();
    if forest.is_empty() {
        return (None, elements);
    }
    let mut forest = forest;
    // A single root is the tree; several roots are wrapped in a synthetic
    // container so the shape stays a tree with document order preserved.
    let tree = if forest.len() == 1 {
        forest.remove(0)
    } else {
        ReadNode {
            index_path: "0".into(),
            role: "Forest".into(),
            name: items
                .iter()
                .filter(|i| i.parent.is_none())
                .map(|i| i.node.name.clone())
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
                .join(" / "),
            automation_id: None,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            actionable: false,
            children: forest,
        }
    };
    (Some(tree), elements)
}

/// The smallest known scope for a rescan of this target (`ARCH/21` §4).
pub fn window_scope(window: &WindowInfo) -> String {
    format!("window:{}", window.id)
}

// ---------------------------------------------------------------------------
// Epochs, cursors and gap → bounded rescan
// ---------------------------------------------------------------------------

/// One collector's position in its source: `(source, scope, epoch, cursor,
/// observed_at)` — the row `ARCH/21` §4 mandates for every collector instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadCursor {
    /// `uia` — the source. A `String` rather than `&'static str` so the row
    /// round-trips through serde without a `'de: 'static` bound.
    pub source: String,
    /// The smallest known scope (`window:<id>`).
    pub scope: String,
    /// The generation the cursor belongs to. A change discards the cursor.
    pub epoch: SnapshotEpoch,
    /// The opaque position inside the source; a UIA snapshot is a whole
    /// generation, so the cursor advances with each bounded read.
    pub cursor: u64,
    /// Milliseconds since the Unix epoch.
    pub observed_at_ms: u64,
}

impl ReadCursor {
    /// A fresh cursor for a window, at epoch 0 / cursor 0.
    pub fn initial(window: &WindowInfo) -> Self {
        Self {
            source: "uia".to_string(),
            scope: window_scope(window),
            epoch: SnapshotEpoch(0),
            cursor: 0,
            observed_at_ms: now_ms(),
        }
    }
}

/// A request for the **smallest known scope** to be re-read, produced by a gap
/// or a freshness violation (`ARCH/21` §4: "Gaps abort and force a scoped
/// rescan").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedRescanRequest {
    pub scope: String,
    pub reason: String,
    pub requested_at_ms: u64,
}

/// What a completed read did to a scope's epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochTransition {
    /// The generation the observation is stamped with.
    pub epoch: SnapshotEpoch,
    /// The cursor after the read.
    pub cursor: ReadCursor,
    /// Rescans still unsatisfied after this read. A read that is itself the gap
    /// does not clear its own request.
    pub pending: Vec<BoundedRescanRequest>,
    /// `Some(reason)` when the source's generation did not advance — an **epoch
    /// reset**, which per `ARCH/21` §4 discards the cursor and forces a rescan.
    pub reset: Option<String>,
}

/// Per-window collector state: the cursor, the current epoch, and the rescan
/// requests a gap produced.
///
/// The epoch advances on **every** bounded read, because a UIA tree is lazy and
/// changes (`ARCH/21` §4, `ARCH/24` §3): an element handle from the previous
/// read is stale the moment the next one starts, which is the "re-read per step,
/// never cache structure as identity" rule expressed as a type.
#[derive(Debug, Default)]
pub struct UiaEpochTracker {
    scopes: Mutex<BTreeMap<String, EpochState>>,
}

/// One scope's epoch/cursor state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochState {
    pub cursor: ReadCursor,
    /// The source's own generation as of the last read, when it reports one. A
    /// rewind here is what an **epoch reset** looks like.
    pub source_generation: Option<SnapshotEpoch>,
    /// The epoch-reset reason the last read recorded, if any. Kept so the
    /// transition a caller receives names it.
    pub last_reset: Option<String>,
    /// Rescans a gap has demanded and that no read has satisfied yet.
    pub pending: Vec<BoundedRescanRequest>,
}

impl UiaEpochTracker {
    pub fn new() -> Self {
        Self {
            scopes: Mutex::new(BTreeMap::new()),
        }
    }

    /// The generation the next read of a window is stamped with.
    ///
    /// **Always strictly greater than this scope's last one**, so an element handle
    /// can never be "current" across two reads even when the provider reports a
    /// generation that does not move. That is the load-bearing half of
    /// `REQ-CUA-003`: a UIA tree is lazy and changes, so the previous read's
    /// observation expires when the next read starts — the local generation is what
    /// enforces it, and the source's number is only ever a floor.
    pub fn next_epoch(&self, window: &WindowInfo, source_epoch: SnapshotEpoch) -> SnapshotEpoch {
        let scope = window_scope(window);
        let guard = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let current = guard.get(&scope).map(|s| s.cursor.epoch.0).unwrap_or(0);
        SnapshotEpoch(source_epoch.0.max(current + 1))
    }

    /// Record a completed read for a window: the cursor advances to the read's
    /// generation, and a pending rescan for this scope is satisfied unless this
    /// read is itself the gap.
    ///
    /// An **epoch reset** is a source generation that moved *backwards* (or to a
    /// lower value than one already seen). Its cursor cannot be trusted across
    /// that, so it is discarded and the scope is rescanned (`ARCH/21` §4: "epoch
    /// reset discards the cursor and rescans"). A source that simply never moves
    /// is not a reset — the local generation then carries the identity, which is
    /// why [`Self::next_epoch`] never repeats one.
    pub fn observe(
        &self,
        window: &WindowInfo,
        read: &StructuredRead,
        source_epoch: SnapshotEpoch,
    ) -> EpochTransition {
        let scope = window_scope(window);
        let hung = read
            .unknowns
            .iter()
            .any(|u| u.kind == UnknownRegionKind::ProviderHung);
        let mut guard = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.entry(scope.clone()).or_insert_with(|| EpochState {
            cursor: ReadCursor::initial(window),
            source_generation: None,
            last_reset: None,
            pending: Vec::new(),
        });
        let (reset, reset_request) = match (entry.source_generation, source_epoch.0) {
            (Some(previous), reported) if reported != 0 && reported < previous.0 => {
                entry.cursor.cursor = 0;
                let reason = format!(
                    "the source's snapshot generation moved backwards ({previous} -> {reported}) — \
                     the cursor was discarded and this scope must be rescanned"
                );
                (
                    Some(format!(
                        "epoch reset: the source's generation moved backwards ({previous} -> \
                         {reported})"
                    )),
                    Some(BoundedRescanRequest {
                        scope: scope.clone(),
                        reason,
                        requested_at_ms: now_ms(),
                    }),
                )
            }
            _ => (None, None),
        };
        entry.last_reset = reset.clone();
        if source_epoch.0 != 0 {
            entry.source_generation = Some(source_epoch);
        }
        entry.cursor.epoch = read.epoch;
        entry.cursor.cursor += 1;
        entry.cursor.observed_at_ms = read.observed_at_ms;
        // A pending rescan for this scope is satisfied by a read that is not itself
        // a gap — and the rescan an epoch reset just demanded is re-queued *after*
        // that sweep, so it survives the read that discovered the reset.
        entry.pending.retain(|p| hung || p.scope != scope);
        entry.pending.extend(reset_request);
        EpochTransition {
            epoch: read.epoch,
            cursor: entry.cursor.clone(),
            pending: entry.pending.clone(),
            reset,
        }
    }

    /// Force a bounded rescan of the smallest known scope, recording a freshness
    /// anomaly. Never silent.
    pub fn note_gap(&self, window: &WindowInfo, reason: impl Into<String>) -> BoundedRescanRequest {
        let scope = window_scope(window);
        let request = BoundedRescanRequest {
            scope: scope.clone(),
            reason: reason.into(),
            requested_at_ms: now_ms(),
        };
        let mut guard = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(scope)
            .or_insert_with(|| EpochState {
                cursor: ReadCursor::initial(window),
                source_generation: None,
                last_reset: None,
                pending: Vec::new(),
            })
            .pending
            .push(request.clone());
        request
    }

    /// The current cursor for a scope, if the tracker has seen it.
    pub fn cursor(&self, window: &WindowInfo) -> Option<ReadCursor> {
        let scope = window_scope(window);
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&scope)
            .map(|s| s.cursor.clone())
    }

    /// The current epoch for a scope.
    pub fn epoch(&self, window: &WindowInfo) -> SnapshotEpoch {
        self.cursor(window)
            .map(|c| c.epoch)
            .unwrap_or(SnapshotEpoch(0))
    }

    /// The current transition state for a scope, without recording a read.
    ///
    /// Used after a gap has been noted, so the returned [`EpochTransition`] carries
    /// the freshly queued rescan requests.
    pub fn transition(&self, window: &WindowInfo, read: &StructuredRead) -> EpochTransition {
        let state = self.state_for(&window_scope(window));
        EpochTransition {
            epoch: read.epoch,
            reset: state.last_reset,
            cursor: state.cursor,
            pending: state.pending,
        }
    }

    /// Rescans a gap has demanded that no read has satisfied yet.
    pub fn pending(&self, window: &WindowInfo) -> Vec<BoundedRescanRequest> {
        let scope = window_scope(window);
        self.state_for(&scope).pending
    }

    fn state_for(&self, scope: &str) -> EpochState {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(scope)
            .cloned()
            .unwrap_or(EpochState {
                cursor: ReadCursor {
                    source: "uia".to_string(),
                    scope: scope.to_string(),
                    epoch: SnapshotEpoch(0),
                    cursor: 0,
                    observed_at_ms: 0,
                },
                source_generation: None,
                last_reset: None,
                pending: Vec::new(),
            })
    }
}

impl Clone for UiaEpochTracker {
    fn clone(&self) -> Self {
        Self {
            scopes: Mutex::new(
                self.scopes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution — ambiguity is rejected, not guessed
// ---------------------------------------------------------------------------

/// How a name matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameMatch {
    /// Case-insensitive equality after whitespace collapse.
    Exact,
    /// The candidate's name starts with the query.
    Prefix,
    /// The candidate's name contains the query.
    Contains,
}

impl NameMatch {
    fn strength(self) -> u8 {
        match self {
            NameMatch::Exact => 0,
            NameMatch::Prefix => 1,
            NameMatch::Contains => 2,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            NameMatch::Exact => "exact",
            NameMatch::Prefix => "prefix",
            NameMatch::Contains => "substring",
        }
    }
}

/// A target to resolve: a name, an optional role, an `AutomationId` **hint**,
/// and optionally the bounds the caller believes it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementQuery {
    pub name: String,
    /// Optional control-type filter.
    pub role: Option<String>,
    /// An `AutomationId` **hint**. It never resolves a query on its own — see
    /// [`resolve`].
    pub automation_id: Option<String>,
    /// Optional bounds the caller expects; a candidate must intersect them.
    pub bounds: Option<Region>,
}

impl ElementQuery {
    /// A name-only query — the form the act vocabulary carries
    /// (`ActKind::ClickByName`, `ActKind::SetValue`).
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            role: None,
            automation_id: None,
            bounds: None,
        }
    }

    /// Add a control-type filter.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Add an `AutomationId` hint. Documented as a *narrowing filter*: a query
    /// that only carries a hint still needs the strong fields to identify one
    /// element.
    pub fn with_automation_id(mut self, id: impl Into<String>) -> Self {
        self.automation_id = Some(id.into());
        self
    }

    /// Add a bounds filter.
    pub fn with_bounds(mut self, bounds: Region) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// The query an act implies, when the act is name-addressed at all.
    pub fn from_act(act: &ActKind) -> Option<Self> {
        match act {
            ActKind::ClickByName { name } | ActKind::SetValue { name, .. } => {
                Some(Self::named(name.clone()))
            }
            _ => None,
        }
    }
}

/// Normalize a name for comparison: trim, collapse whitespace, lowercase.
fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// One candidate in a resolution result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub index_path: String,
    pub role: String,
    pub name: String,
    /// The `AutomationId` hint, for the disambiguation card.
    pub automation_id: Option<String>,
    pub bounds: Region,
    /// Whether this is a protected field (whose text is masked).
    pub protected_field: bool,
}

impl Candidate {
    fn of(element: &ObservedElement) -> Self {
        Self {
            index_path: element.index_path.clone(),
            role: element.handle.role.clone(),
            name: element.handle.name.clone(),
            automation_id: element.handle.automation_id_hint.clone(),
            bounds: element.handle.bounds,
            protected_field: element.handle.protected_field,
        }
    }

    /// A one-line description for the ambiguity card.
    pub fn describe(&self) -> String {
        format!(
            "{}{} at [{}] bounds {},{} {}x{} (automationId hint {:?})",
            self.role,
            if self.name.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", self.name)
            },
            self.index_path,
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            self.automation_id
        )
    }
}

/// The outcome of resolving a query against one observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "detail")]
pub enum Resolution {
    /// Exactly one candidate: a handle valid for **one** action.
    Resolved(Box<ObservationHandle>),
    /// More than one candidate at the winning strength. Rejected, never guessed
    /// (`REQ-CUA-003`).
    Ambiguous {
        query: Box<ElementQuery>,
        candidates: Vec<Candidate>,
        reason: String,
    },
    /// No candidate at this strength. `nearest` carries the weaker matches so the
    /// caller can say what *was* there instead of "not found".
    NotFound {
        query: Box<ElementQuery>,
        nearest: Vec<Candidate>,
        reason: String,
    },
}

impl Resolution {
    /// The handle, when exactly one candidate matched.
    pub fn handle(&self) -> Option<&ObservationHandle> {
        match self {
            Resolution::Resolved(h) => Some(h),
            _ => None,
        }
    }

    /// Did this resolve to exactly one element?
    pub fn is_resolved(&self) -> bool {
        matches!(self, Resolution::Resolved(_))
    }

    /// Is this a rejection that must not be turned into an action?
    pub fn is_rejection(&self) -> bool {
        !self.is_resolved()
    }

    /// One sentence naming the outcome, for a refusal, a guidance card, or an
    /// audit row.
    pub fn describe(&self) -> String {
        match self {
            Resolution::Resolved(h) => format!("resolved {}", h.element.describe(now_ms())),
            Resolution::Ambiguous {
                query,
                candidates,
                reason,
            } => format!(
                "ambiguous: {} element(s) match {query:?} ({reason}) — refusing to guess; candidates: \
                 {}",
                candidates.len(),
                candidates
                    .iter()
                    .map(Candidate::describe)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Resolution::NotFound {
                query,
                nearest,
                reason,
            } => format!(
                "no element matches {query:?} ({reason}){}",
                if nearest.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; closest: {}",
                        nearest
                            .iter()
                            .map(Candidate::describe)
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                }
            ),
        }
    }
}

/// Score one element against a query, or `None` when it does not match.
fn score(element: &ObservedElement, query: &ElementQuery, needle: &str) -> Option<NameMatch> {
    if let Some(role) = query.role.as_deref()
        && norm(&element.handle.role) != norm(role)
    {
        return None;
    }
    // An `AutomationId` hint is a *filter*: it can exclude a candidate, and it can
    // never be the only thing that includes one (an empty query name below is
    // only matched by a role + bounds pair, never by the hint).
    if let Some(id) = query.automation_id.as_deref()
        && element
            .handle
            .automation_id_hint
            .as_deref()
            .map(norm)
            .as_deref()
            != Some(norm(id).as_str())
    {
        return None;
    }
    if let Some(bounds) = query.bounds.as_ref()
        && bounds.intersect(&element.handle.bounds).is_none()
    {
        return None;
    }
    let name = norm(&element.handle.name);
    if needle.is_empty() {
        // An empty name matches nothing by name. A nameless control is
        // addressable only through a role *and* a bounds hint together — a bare
        // `AutomationId` is never enough, because it is optional and not
        // build-stable.
        return match (query.role.as_ref(), query.bounds.as_ref()) {
            (Some(_), Some(_)) if name.is_empty() => Some(NameMatch::Exact),
            _ => None,
        };
    }
    if name.is_empty() {
        return None;
    }
    if name == needle {
        Some(NameMatch::Exact)
    } else if name.starts_with(needle) {
        Some(NameMatch::Prefix)
    } else if name.contains(needle) {
        Some(NameMatch::Contains)
    } else {
        None
    }
}

/// Resolve a query against one observation.
///
/// The rules, in order — and each one is a rule the v0 code did not have:
///
/// 1. candidates are filtered by the optional role, `AutomationId` hint and
///    bounds, then scored on the name (`Exact` > `Prefix` > `Contains`);
/// 2. only the **best** strength survives, so a weak substring hit never
///    competes with an exact name;
/// 3. **one** survivor is resolved; more than one is `Ambiguous`. That is the
///    rejection, and it is unconditional — the resolver never picks the first,
///    the last, or the closest;
/// 4. an empty query name matches nothing by name, so a caller's empty string
///    cannot resolve to "some element".
///
/// The `AutomationId` is deliberately **not** used to break a tie: a hint that
/// disagreed with the strong fields is exactly the "not stable across builds"
/// case, and resolving by it is the bug `FIX-17` exists to remove. A caller that
/// genuinely knows the `AutomationId` passes it as a filter and still gets
/// `Ambiguous` when more than one sibling matches.
pub fn resolve(elements: &[ObservedElement], query: &ElementQuery) -> Resolution {
    let needle = norm(&query.name);
    let mut scored: Vec<(NameMatch, &ObservedElement)> = Vec::new();
    for element in elements {
        if let Some(strength) = score(element, query, &needle) {
            scored.push((strength, element));
        }
    }
    let Some(best) = scored.iter().map(|(s, _)| s.strength()).min() else {
        return Resolution::NotFound {
            query: Box::new(query.clone()),
            nearest: Vec::new(),
            reason: if elements.is_empty() {
                "the observation contains no elements".into()
            } else {
                "no name, role, automationId hint or bounds matched".into()
            },
        };
    };
    let winners: Vec<&ObservedElement> = scored
        .iter()
        .filter(|(s, _)| s.strength() == best)
        .map(|(_, e)| *e)
        .collect();
    if winners.len() == 1 {
        let element = winners[0];
        return Resolution::Resolved(Box::new(ObservationHandle {
            element: element.handle.clone(),
            index_path: element.index_path.clone(),
        }));
    }
    // The rejection. Every candidate is named so the caller (or the user) can
    // choose by re-reading with a role or bounds hint — never by us picking one.
    let mut candidates: Vec<Candidate> = winners.iter().map(|e| Candidate::of(e)).collect();
    candidates.sort_by(|a, b| a.index_path.cmp(&b.index_path));
    let label = match best {
        0 => NameMatch::Exact,
        1 => NameMatch::Prefix,
        _ => NameMatch::Contains,
    };
    Resolution::Ambiguous {
        query: Box::new(query.clone()),
        reason: format!(
            "{} element(s) match equally well ({} name match) — an ambiguous element is rejected, \
             not guessed; re-read with a role or bounds hint",
            candidates.len(),
            label.as_str()
        ),
        candidates,
    }
}

/// Resolve against a read, honouring its absence policy: a read that may not
/// infer absence never produces a confident `NotFound` — it produces one that
/// says why.
pub fn resolve_in(read: &StructuredRead, query: &ElementQuery) -> Resolution {
    if read.may_infer_absence() {
        return resolve(&read.elements, query);
    }
    // A partial read can still resolve exactly one element — we may act on what
    // we did see — but absence is not concluded from it.
    match resolve(&read.elements, query) {
        Resolution::Resolved(h) => Resolution::Resolved(h),
        other => {
            let reason = format!(
                "the observation is {} — absence cannot be inferred from it: {}",
                read.status.as_str(),
                read.unknowns
                    .first()
                    .map(|u| u.guidance())
                    .unwrap_or_else(|| read.status_detail())
            );
            match other {
                Resolution::NotFound { query, nearest, .. } => Resolution::NotFound {
                    query,
                    nearest,
                    reason,
                },
                // An ambiguity in a partial read is still an ambiguity; only its
                // reason is enriched, because a partial read cannot clear it.
                Resolution::Ambiguous {
                    query, candidates, ..
                } => Resolution::Ambiguous {
                    query,
                    candidates,
                    reason,
                },
                resolved => resolved,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The read path
// ---------------------------------------------------------------------------

/// What [`read_window`] produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiaRead {
    /// The bounded, isolated read.
    pub read: StructuredRead,
    /// The cursor this read advanced.
    pub cursor: ReadCursor,
    /// The epoch transition, including an **epoch reset** when the source's
    /// generation did not advance (`ARCH/21` §4).
    pub transition: EpochTransition,
}

impl UiaRead {
    /// Rescans a gap has demanded that this read did not satisfy.
    pub fn pending_rescans(&self) -> &[BoundedRescanRequest] {
        &self.transition.pending
    }
}

/// One bounded, isolated, epoch-stamped structured read of a window.
///
/// This is the W3 collector entry point: the walk happens on a worker with a
/// per-call budget (`REQ-CUA-004`), the result carries its epoch so an element
/// handle cannot outlive its observation (`REQ-CUA-003`), an unreadable region is
/// typed rather than empty (`REQ-CUA-006`), and a gap forces a bounded rescan of
/// the smallest known scope instead of a silent gap (`REQ-WORLD-005`).
pub fn read_window<P>(provider: P, window: &WindowInfo) -> UiaRead
where
    P: UiaProvider + 'static,
{
    read_window_with(
        provider,
        window,
        &CollectConfig::default(),
        &UiaEpochTracker::new(),
    )
}

/// [`read_window`] with an explicit collector config and epoch tracker — the
/// seam the Windows backend and the tests use.
pub fn read_window_with<P>(
    provider: P,
    window: &WindowInfo,
    cfg: &CollectConfig,
    tracker: &UiaEpochTracker,
) -> UiaRead
where
    P: UiaProvider + 'static,
{
    let source_epoch = provider.epoch();
    let started = Instant::now();
    let mut session = start_read(provider, window.clone(), *cfg);
    let mut outcome: Option<CollectionOutcome> = None;
    while Instant::now() < cfg.deadline {
        let remaining = cfg.deadline.saturating_duration_since(Instant::now());
        if let Some(o) = session.poll(remaining.min(Duration::from_millis(20))) {
            outcome = Some(o.clone());
            break;
        }
    }
    let items = session.partial_elements();
    let outcome = outcome.unwrap_or(CollectionOutcome::Partial {
        nodes: 0,
        unknown: UnknownRegion {
            kind: UnknownRegionKind::ProviderHung,
            detail: format!(
                "the accessibility provider had not finished after {}ms ({} node(s) collected so \
                 far)",
                started.elapsed().as_millis(),
                items.len()
            ),
            observed_at_ms: now_ms(),
        },
    });
    session.cancel();
    let epoch = tracker.next_epoch(window, source_epoch);
    let read = assemble(window, cfg.bounds, outcome, items, epoch);
    tracker.observe(window, &read, source_epoch);
    // `ARCH/21` §4: "Gaps abort and force a scoped rescan" and "Never a silent gap:
    // record a freshness anomaly". A provider that hung or failed mid-walk **is** a
    // gap, so the smallest known scope (this window) is queued for a re-read —
    // recorded *after* the transition, so the read that hit the gap cannot satisfy
    // its own request.
    //
    // A declared bound is deliberately not a gap: re-reading the same window
    // reproduces the same bound. Its guidance is to change the scope (scroll,
    // paginate) and read again.
    for unknown in &read.unknowns {
        if matches!(
            unknown.kind,
            UnknownRegionKind::ProviderHung | UnknownRegionKind::ProviderError
        ) {
            tracker.note_gap(
                window,
                format!(
                    "{} during the bounded read of {}: {} — the smallest known scope must be \
                     re-read before this observation is trusted",
                    unknown.kind.as_str(),
                    window_scope(window),
                    unknown.detail
                ),
            );
        }
    }
    let transition = tracker.transition(window, &read);
    UiaRead {
        read,
        cursor: transition.cursor.clone(),
        transition,
    }
}
