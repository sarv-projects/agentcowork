//! `FIX-17` / `TASK-CUA-001` — the UIA collector's identity, ambiguity,
//! elevation, timeout and gap rules, exercised with a fake provider on **every**
//! host.
//!
//! The Windows COM client is `#[cfg(windows)]` and cannot run here, so what these
//! tests prove is the *policy* the collector enforces, not that a real UIA provider
//! answers as the client expects. That split is the honest one: the policy is
//! where the v0 defects were, and the client is where the Windows acceptance
//! record has to come from.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use agentcowork_computeruse::types::{ActKind, Region, WindowInfo};
use agentcowork_computeruse::uia::{
    CollectConfig, CollectedElement, CollectionOutcome, Coverage, ElementQuery, NameMatch,
    ReadBounds, Resolution, SnapshotEpoch, UiaAnswer, UiaCall, UiaEpochTracker, UiaFault, UiaNode,
    UiaProvider, UiaRead, UiaReadStatus, UnknownRegionKind, assemble, collect, provider_hint,
    read_window_with, resolve_in, sanitize,
};

fn window(id: u64) -> WindowInfo {
    WindowInfo {
        id,
        title: "Editor".into(),
        app: "notepad".into(),
        x: 0,
        y: 0,
        width: 800,
        height: 600,
        has_a11y_tree: true,
    }
}

fn region(x: i32, y: i32, w: u32, h: u32) -> Region {
    Region {
        x,
        y,
        width: w,
        height: h,
    }
}

/// A scripted UI Automation client.
///
/// The script is a parent → ordered children map, which is exactly the shape the
/// collector walks, so a test can express a real UI (a toolbar with two "Save"
/// buttons, a password field) without any COM.
struct FakeUia {
    epoch: SnapshotEpoch,
    coverage: Coverage,
    /// parent handle → children in document order.
    tree: BTreeMap<u64, Vec<UiaNode>>,
    /// handle → the full node (what a `Properties` call answers).
    nodes: BTreeMap<u64, UiaNode>,
    /// Calls that sleep this long before answering (a hung provider).
    hang_on: Option<u64>,
    hang_for: Duration,
    calls: Mutex<Vec<UiaCall>>,
    /// Fault to return for a specific call kind.
    root_fault: Option<UiaFault>,
}

impl FakeUia {
    fn new() -> Self {
        Self {
            epoch: SnapshotEpoch(7),
            coverage: Coverage::Complete,
            tree: BTreeMap::new(),
            nodes: BTreeMap::new(),
            hang_on: None,
            hang_for: Duration::from_millis(0),
            calls: Mutex::new(Vec::new()),
            root_fault: None,
        }
    }

    /// Attach a child under `parent` (0 = the window root) and return its handle.
    fn add(&mut self, parent: u64, node: UiaNode) -> u64 {
        let handle = node.handle.0;
        self.tree.entry(parent).or_default().push(node.clone());
        self.nodes.insert(handle, node);
        handle
    }

    fn call_count(&self) -> usize {
        self.calls.lock().map(|c| c.len()).unwrap_or(0)
    }

    fn hang_after(&mut self, nth: u64, for_ms: u64) {
        self.hang_on = Some(nth);
        self.hang_for = Duration::from_millis(for_ms);
    }
}

impl UiaProvider for FakeUia {
    fn epoch(&self) -> SnapshotEpoch {
        self.epoch
    }

    fn coverage(&mut self, _window: &WindowInfo) -> Coverage {
        self.coverage
    }

    fn call(&mut self, _window: &WindowInfo, call: UiaCall) -> Result<UiaAnswer, UiaFault> {
        let count = {
            let mut g = self.calls.lock().unwrap();
            g.push(call);
            g.len() as u64
        };
        if let Some(nth) = self.hang_on
            && count == nth
        {
            // A real hung provider: the call never returns.
            std::thread::sleep(self.hang_for);
        }
        match call {
            UiaCall::Root => {
                if let Some(fault) = &self.root_fault {
                    return Err(fault.clone());
                }
                Ok(UiaAnswer::Element(Box::new(UiaNode::new(
                    0,
                    "Pane",
                    "Editor",
                    region(0, 0, 800, 600),
                ))))
            }
            UiaCall::Properties(handle) => match self.nodes.get(&handle.0) {
                Some(n) => Ok(UiaAnswer::Element(Box::new(n.clone()))),
                None => Ok(UiaAnswer::Absent),
            },
            UiaCall::FirstChild(parent) => {
                let first = self.tree.get(&parent.0).and_then(|c| c.first()).cloned();
                Ok(first
                    .map(|n| UiaAnswer::Element(Box::new(n)))
                    .unwrap_or(UiaAnswer::Absent))
            }
            UiaCall::NextSibling(handle) => {
                let found = self.tree.iter().find_map(|(parent, children)| {
                    children
                        .iter()
                        .position(|c| c.handle.0 == handle.0)
                        .map(|i| (parent, i))
                });
                let next = found.and_then(|(parent, i)| {
                    self.tree.get(parent).and_then(|c| c.get(i + 1)).cloned()
                });
                Ok(next
                    .map(|n| UiaAnswer::Element(Box::new(n)))
                    .unwrap_or(UiaAnswer::Absent))
            }
        }
    }
}

fn button(handle: u64, name: &str, x: i32, y: i32, automation_id: Option<&str>) -> UiaNode {
    let mut n = UiaNode::new(handle, "Button", name, region(x, y, 80, 24));
    n.automation_id = automation_id.map(str::to_string);
    n
}

fn observe(provider: FakeUia, w: &WindowInfo) -> UiaRead {
    read_window_with(
        provider,
        w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &UiaEpochTracker::new(),
    )
}

// ---------------------------------------------------------------------------
// 1. AutomationId is a hint, never a key
// ---------------------------------------------------------------------------

/// `FIX-17`, finding 1: two controls that share an `AutomationId` (which MS
/// documents as sibling-scoped and *not* build-stable) must not be collapsed into
/// one, and a name that matches both must be **rejected** rather than resolved to
/// whichever came first.
#[test]
fn an_automation_id_never_turns_two_controls_into_one() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, Some("btnSave")));
    fake.add(0, button(2, "Save", 10, 40, Some("btnSave")));
    let read = observe(fake, &window(1)).read;

    assert_eq!(read.status, UiaReadStatus::Complete);
    assert_eq!(read.elements.len(), 3, "the root plus both buttons");
    // The two candidates are distinct observations: different bounds, different
    // runtime ids. The AutomationId is carried as a hint on both.
    let saves: Vec<_> = read
        .elements
        .iter()
        .filter(|e| e.name() == "Save")
        .collect();
    assert_eq!(saves.len(), 2);
    assert!(
        saves
            .iter()
            .all(|e| e.automation_id_hint() == Some("btnSave"))
    );
    assert_ne!(saves[0].handle.bounds, saves[1].handle.bounds);

    // And the name is ambiguous, so the resolution refuses.
    match resolve_in(&read, &ElementQuery::named("Save")) {
        Resolution::Ambiguous {
            candidates, reason, ..
        } => {
            assert_eq!(candidates.len(), 2, "{reason}");
            assert!(reason.contains("rejected"), "{reason}");
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

/// The hint can *narrow*: given the same two candidates, a bounds filter resolves
/// to exactly one. That is the documented use — a hint that narrows an already
/// scoped query, never one that carries it.
#[test]
fn an_automation_id_or_bounds_narrows_a_query_but_never_carries_it() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, Some("btnSave")));
    fake.add(0, button(2, "Save", 10, 40, Some("btnSave")));
    let read = observe(fake, &window(1)).read;

    // With the hint and a bounds filter, exactly one survives.
    let narrowed = resolve_in(
        &read,
        &ElementQuery::named("Save")
            .with_automation_id("btnSave")
            .with_bounds(region(0, 0, 200, 30)),
    );
    let handle = narrowed
        .handle()
        .unwrap_or_else(|| panic!("expected a resolution, got {narrowed:?}"));
    assert_eq!(handle.element.bounds, region(10, 10, 80, 24));

    // With only the hint, still ambiguous: an AutomationId alone is never a key.
    assert!(
        matches!(
            resolve_in(
                &read,
                &ElementQuery::named("Save").with_automation_id("btnSave")
            ),
            Resolution::Ambiguous { .. }
        ),
        "an AutomationId hint must not resolve a two-candidate name on its own"
    );
}

/// A hint that disagrees with the strong fields excludes the candidate rather than
/// overriding it: `AutomationId` is not build-stable, so a stale value must not
/// outvote the observed role+name+bounds.
#[test]
fn a_stale_automation_id_hint_cannot_override_the_observed_fields() {
    let mut fake = FakeUia::new();
    let mut stale = button(1, "Save", 10, 10, Some("btnSave_v1"));
    // The app was rebuilt and the id changed; the name and role did not.
    fake.add(0, stale.clone());
    let read = observe(fake, &window(1)).read;
    stale.automation_id = Some("btnSave_v1".into());

    // Asking for the *new* id finds nothing rather than guessing at the old one.
    assert!(matches!(
        resolve_in(
            &read,
            &ElementQuery::named("Save").with_automation_id("btnSave_v2")
        ),
        Resolution::NotFound { .. }
    ));
    // The name still resolves: the identity is role+name+bounds, not the hint.
    assert!(resolve_in(&read, &ElementQuery::named("Save")).is_resolved());
}

// ---------------------------------------------------------------------------
// 2. Ambiguity is rejected, not guessed
// ---------------------------------------------------------------------------

/// Two "Save" buttons, and the ladder's act path: the resolution is rejected, so
/// no coordinate is produced at all.
#[test]
fn an_ambiguous_name_produces_no_coordinate() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, None));
    fake.add(0, button(2, "Save", 10, 40, None));
    let read = observe(fake, &window(1)).read;
    let resolution = resolve_in(&read, &ElementQuery::named("Save"));
    assert!(resolution.is_rejection());
    assert!(resolution.describe().contains("refusing to guess"));
    // The act vocabulary carries the same query shape.
    let from_act = ElementQuery::from_act(&ActKind::ClickByName {
        name: "Save".into(),
    });
    assert_eq!(from_act, Some(ElementQuery::named("Save")));
}

/// The strongest match wins outright, so a substring hit never competes with an
/// exact name.
#[test]
fn the_strongest_name_match_wins_and_ambiguity_is_judged_only_at_that_strength() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, None));
    // "Save all changes" contains "save" but does not equal it.
    fake.add(0, button(2, "Save all changes", 10, 40, None));
    fake.add(0, button(3, "Save", 10, 70, None));
    let read = observe(fake, &window(1)).read;

    match resolve_in(&read, &ElementQuery::named("Save")) {
        Resolution::Ambiguous {
            candidates, reason, ..
        } => {
            // Only the two *exact* "Save" candidates are ambiguous; the substring
            // match is not even in the candidate set.
            assert_eq!(candidates.len(), 2, "{reason}");
            assert!(candidates.iter().all(|c| c.name == "Save"), "{reason}");
            assert!(reason.contains("exact"), "{reason}");
        }
        other => panic!("expected Ambiguous at the exact strength, got {other:?}"),
    }

    // Narrowing to the exact control resolves.
    let exact = resolve_in(
        &read,
        &ElementQuery::named("Save").with_bounds(region(0, 60, 200, 40)),
    );
    assert!(exact.is_resolved(), "{exact:?}");

    // A prefix query reaches only the prefix candidates.
    match resolve_in(&read, &ElementQuery::named("Save all")) {
        Resolution::Resolved(h) => assert_eq!(h.element.name, "Save all changes"),
        other => panic!("expected Resolved, got {other:?}"),
    }
}

/// An empty query name must not resolve to "some element" — a caller's empty
/// string is not a licence to click the first control in the tree.
#[test]
fn an_empty_query_name_resolves_to_nothing() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, None));
    let read = observe(fake, &window(1)).read;
    assert!(
        matches!(
            resolve_in(&read, &ElementQuery::named("")),
            Resolution::NotFound { .. }
        ),
        "an empty name must not resolve by name"
    );
}

// ---------------------------------------------------------------------------
// 3. UIAccess / elevation degrades loudly
// ---------------------------------------------------------------------------

/// `FIX-17`, finding 2: an elevated target a medium-integrity client cannot read
/// must come back **unknown** with guidance — never as an empty tree that reads
/// like "this app has no accessibility tree".
#[test]
fn an_unreadable_elevated_window_is_unknown_not_empty() {
    let mut fake = FakeUia::new();
    fake.coverage = Coverage::Restricted { elevated: true };
    // The provider *would* happily answer if asked; the collector must not ask.
    fake.add(0, button(1, "Save", 10, 10, None));
    let read = observe(fake, &window(1)).read;

    assert_eq!(read.status.as_str(), "unknown");
    assert!(!read.may_infer_absence());
    assert!(read.tree.is_none());
    assert!(read.elements.is_empty());
    let kinds: Vec<_> = read.unknowns.iter().map(|u| u.kind).collect();
    assert_eq!(kinds, vec![UnknownRegionKind::Elevation]);
    let guidance = read.guidance(&window(1)).expect("guidance is mandatory");
    assert!(guidance.contains("UIAccess"), "{guidance}");
    assert!(
        guidance.contains("no input is synthesized into it"),
        "{guidance}"
    );
    // The anomaly is recorded, never silent.
    assert_eq!(read.anomalies.len(), 1);
    assert_eq!(read.anomalies[0].kind, "elevation_restricted");
    assert_eq!(read.anomalies[0].scope, "window:1");
}

/// The same window one integrity level down is a `complete` read — so the
/// difference between "nothing is there" and "I cannot look" is data, not
/// inference.
#[test]
fn a_readable_window_is_complete_and_may_infer_absence() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, None));
    let read = observe(fake, &window(1)).read;
    assert_eq!(read.status, UiaReadStatus::Complete);
    assert!(read.may_infer_absence());
    assert!(read.guidance(&window(1)).is_none());
    assert!(read.anomalies.is_empty());
}

/// An elevated target this process cannot read is **fully unknown** — not a
/// "partial but usable" read — and the resolution refuses to conclude absence
/// from it. (A read that *does* reach part of an elevated window is `partial`;
/// `assembly_maps_outcomes_to_the_honest_status` covers that half.)
#[test]
fn an_elevated_window_nothing_reachable_infers_no_absence() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 10, 10, None));
    fake.coverage = Coverage::Restricted { elevated: true };
    let read = observe(fake, &window(1)).read;
    // Coverage is checked before the walk, so nothing is walked.
    assert!(read.elements.is_empty());
    assert_eq!(read.status.as_str(), "unknown");
    assert!(!read.status.is_usable());
    assert!(!read.may_infer_absence());
    // And a name lookup on it cannot produce a confident "not found".
    match resolve_in(&read, &ElementQuery::named("Nope")) {
        Resolution::NotFound { reason, .. } => {
            assert!(reason.contains("absence cannot be inferred"), "{reason}");
        }
        other => panic!("expected a qualified NotFound, got {other:?}"),
    }
}

/// A dead handle is `unknown` too, with its own reason — not an empty tree.
#[test]
fn a_dead_window_handle_is_unknown_with_its_own_reason() {
    let mut fake = FakeUia::new();
    fake.root_fault = Some(UiaFault::TargetUnavailable {
        detail: "ElementFromHandle returned no element".into(),
    });
    let read = observe(fake, &window(9)).read;
    assert_eq!(read.status.as_str(), "unknown");
    assert_eq!(read.unknowns[0].kind, UnknownRegionKind::TargetUnavailable);
    assert!(!read.may_infer_absence());
    let guidance = read.guidance(&window(9)).expect("guidance");
    assert!(guidance.contains("closed"), "{guidance}");
}

/// A target that genuinely exposes no structural UI is `absent`, and absence **is**
/// inferable from it — that is the honest trigger for the OCR / vision rung.
#[test]
fn no_structural_ui_is_absent_and_permits_the_vision_rung() {
    let mut fake = FakeUia::new();
    fake.root_fault = Some(UiaFault::NoTree {
        detail: "the window exposes no automation peer".into(),
    });
    let read = observe(fake, &window(3)).read;
    assert_eq!(read.status.as_str(), "absent");
    assert!(read.may_infer_absence());
    assert!(read.unknowns.is_empty());
    let guidance = read.guidance(&window(3)).expect("guidance");
    assert!(guidance.contains("OCR / vision rung"), "{guidance}");
}

// ---------------------------------------------------------------------------
// 4. Bounded reads with per-call budget + worker isolation
// ---------------------------------------------------------------------------

/// `FIX-17`, finding 3: a provider that hangs inside a call must not stall the
/// caller. The walk happens on a worker; the caller keeps whatever was already
/// found and reports the read **partial**.
#[test]
fn a_hung_provider_yields_a_partial_read_and_never_stalls_the_caller() {
    let mut fake = FakeUia::new();
    // A wide tree so the walk is long enough to have streamed something.
    for i in 1..=400u64 {
        fake.add(
            0,
            button(i, &format!("Item {i}"), 10, (i as i32) * 24, None),
        );
    }
    // Hang deep inside the walk (well past the first few children).
    fake.hang_after(40, 4_000);

    let started = Instant::now();
    let read = observe(fake, &window(1)).read;
    let elapsed = started.elapsed();

    // The caller returned on its own budget, not the provider's 4s sleep.
    assert!(
        elapsed < Duration::from_millis(3_500),
        "the read must not wait for the hung provider, took {elapsed:?}"
    );
    assert_eq!(read.status.as_str(), "partial");
    let kinds: Vec<_> = read.unknowns.iter().map(|u| u.kind).collect();
    assert!(
        kinds.contains(&UnknownRegionKind::ProviderHung),
        "expected a provider_hung unknown, got {kinds:?}"
    );
    // And there is a partial tree to work with, not an empty one.
    assert!(read.node_count() > 0, "the partial stream should be usable");
    assert!(!read.may_infer_absence());
    // The gap is recorded, never silent.
    assert!(read.anomalies.iter().any(|a| a.kind == "provider_hung"));
}

/// The declared node bound is a real ceiling: hitting it is a `Partial` read with
/// a `provider_bound` unknown, never a "complete" tree that happens to be short.
#[test]
fn a_node_bound_produces_a_partial_read_named_as_a_bound() {
    let mut fake = FakeUia::new();
    for i in 1..=50u64 {
        fake.add(0, button(i, &format!("Row {i}"), 0, (i as i32) * 20, None));
    }
    let cfg = CollectConfig {
        bounds: ReadBounds {
            max_nodes: 10,
            ..ReadBounds::default()
        },
        deadline: Instant::now() + Duration::from_millis(900),
    };
    let read = read_window_with(fake, &window(1), &cfg, &UiaEpochTracker::new()).read;
    assert_eq!(read.status.as_str(), "partial");
    assert_eq!(read.unknowns[0].kind, UnknownRegionKind::ProviderBound);
    assert!(read.node_count() <= 10);
    assert!(read.anomalies.iter().any(|a| a.kind == "provider_bound"));
}

/// The declared depth bound behaves the same way.
#[test]
fn a_depth_bound_stops_the_walk_and_says_so() {
    let mut fake = FakeUia::new();
    // A chain 20 deep.
    let mut parent = 0u64;
    for depth in 1..=20u64 {
        parent = fake.add(
            parent,
            button(depth, &format!("Level {depth}"), 0, depth as i32 * 20, None),
        );
    }
    let cfg = CollectConfig {
        bounds: ReadBounds {
            max_depth: 4,
            ..ReadBounds::default()
        },
        deadline: Instant::now() + Duration::from_millis(900),
    };
    let read = read_window_with(fake, &window(1), &cfg, &UiaEpochTracker::new()).read;
    assert_eq!(read.status.as_str(), "partial");
    assert_eq!(read.unknowns[0].kind, UnknownRegionKind::ProviderBound);
}

/// The per-field text bound is applied and marked, so a consumer can tell a short
/// name from a long one that was cut.
#[test]
fn long_names_are_truncated_at_the_declared_bound_and_marked() {
    let mut fake = FakeUia::new();
    let long = "L".repeat(MAX_TEXT_OVERRIDE + 50);
    fake.add(0, button(1, &long, 0, 0, None));
    let cfg = CollectConfig {
        bounds: ReadBounds {
            max_text_chars: MAX_TEXT_OVERRIDE,
            ..ReadBounds::default()
        },
        deadline: Instant::now() + Duration::from_millis(900),
    };
    let read = read_window_with(fake, &window(1), &cfg, &UiaEpochTracker::new()).read;
    let child = read
        .elements
        .iter()
        .find(|e| e.role() == "Button")
        .expect("the button");
    assert_eq!(child.name().chars().count(), MAX_TEXT_OVERRIDE);
    assert_ne!(child.name(), long);
}

const MAX_TEXT_OVERRIDE: usize = 40;

/// A provider whose property read fails mid-walk is `partial` with a
/// `provider_error` unknown — a partial read, not a silently short one.
#[test]
fn a_mid_walk_property_failure_is_partial_and_recorded() {
    struct FailingProperties {
        inner: FakeUia,
    }
    impl UiaProvider for FailingProperties {
        fn epoch(&self) -> SnapshotEpoch {
            self.inner.epoch
        }
        fn coverage(&mut self, w: &WindowInfo) -> Coverage {
            self.inner.coverage(w)
        }
        fn call(&mut self, w: &WindowInfo, call: UiaCall) -> Result<UiaAnswer, UiaFault> {
            if let UiaCall::Properties(handle) = call
                && handle.0 == 2
            {
                return Err(UiaFault::ProviderError {
                    detail: "RPC_E_CALL_REJECTED — the provider is busy".into(),
                });
            }
            self.inner.call(w, call)
        }
    }
    let mut inner = FakeUia::new();
    inner.add(0, button(1, "Save", 0, 0, None));
    inner.add(0, button(2, "Cancel", 0, 30, None));
    inner.add(0, button(3, "Help", 0, 60, None));
    let read = read_window_with(
        FailingProperties { inner },
        &window(1),
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &UiaEpochTracker::new(),
    )
    .read;
    assert_eq!(read.status.as_str(), "partial");
    assert_eq!(read.unknowns[0].kind, UnknownRegionKind::ProviderError);
    assert!(!read.may_infer_absence());
    assert!(read.anomalies.iter().any(|a| a.kind == "provider_error"));
    // The elements found before the failure are still there to act on.
    assert!(read.node_count() >= 1);
}

// ---------------------------------------------------------------------------
// 5. Epoch discipline: an observation is valid for one action
// ---------------------------------------------------------------------------

/// A handle is stamped with the read's epoch, and a later read invalidates it.
#[test]
fn an_element_handle_is_valid_for_one_observation_only() {
    let tracker = UiaEpochTracker::new();
    let w = window(1);
    let mut first = FakeUia::new();
    first.add(0, button(1, "Save", 10, 10, None));
    let first = read_window_with(
        first,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &tracker,
    );
    let handle = resolve_in(&first.read, &ElementQuery::named("Save"))
        .handle()
        .expect("resolved")
        .clone();
    assert!(handle.validate(first.read.current_epoch(), 0).is_current);

    // A second read of the same UI: the *element* is the same, but the observation
    // is not — a UIA tree is lazy and changes, so structure is never identity.
    let mut second = FakeUia::new();
    second.add(0, button(1, "Save", 10, 10, None));
    let second = read_window_with(
        second,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &tracker,
    );
    let validity = handle.validate(second.read.current_epoch(), 0);
    assert!(
        !validity.is_current,
        "a previous read's handle must be stale"
    );
    let reason = validity.reason.expect("a reason");
    assert!(reason.contains("re-read"), "{reason}");
    assert!(reason.contains("not an identity"), "{reason}");

    // The strong identity survives the re-read even though the epoch did not.
    let fresh = resolve_in(&second.read, &ElementQuery::named("Save"));
    let fresh = fresh.handle().expect("resolved again").clone();
    assert!(handle.element.same_element(&fresh.element));
    assert!(!handle.element.is_current(second.read.epoch));
}

// ---------------------------------------------------------------------------
// 6. Cursor, epoch and gap → bounded rescan
// ---------------------------------------------------------------------------

/// The cursor row is the `ARCH/21` §4 shape, and it advances with every read.
#[test]
fn the_cursor_carries_source_scope_epoch_cursor_and_observed_at() {
    let tracker = UiaEpochTracker::new();
    let w = window(5);
    assert!(tracker.cursor(&w).is_none());
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 0, 0, None));
    let read = read_window_with(
        fake,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &tracker,
    );
    let cursor = read.cursor;
    assert_eq!(cursor.source, "uia");
    assert_eq!(cursor.scope, "window:5");
    assert_eq!(cursor.epoch, read.read.epoch);
    assert_eq!(cursor.cursor, 1);
    assert!(cursor.observed_at_ms > 0);
}

/// A gap is never silent: it forces a **bounded** rescan of the smallest known
/// scope and stays pending until a read that is not itself the gap.
#[test]
fn a_gap_forces_a_bounded_rescan_of_the_smallest_scope_and_is_recorded() {
    let tracker = UiaEpochTracker::new();
    let w = window(4);
    let request = tracker.note_gap(&w, "the provider hung inside a call");
    assert_eq!(request.scope, "window:4");
    assert!(request.reason.contains("hung"));
    assert_eq!(tracker.pending(&w), vec![request.clone()]);

    // A healthy read satisfies it.
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 0, 0, None));
    let read = read_window_with(
        fake,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &tracker,
    );
    assert!(read.pending_rescans().is_empty());
    assert!(tracker.pending(&w).is_empty());

    // A read that is itself a gap does **not** clear its own request.
    let mut hung = FakeUia::new();
    hung.add(0, button(1, "Save", 0, 0, None));
    hung.hang_after(3, 2_000);
    let read = read_window_with(
        hung,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(250)),
        &tracker,
    );
    assert_eq!(read.read.status.as_str(), "partial");
    assert!(!read.pending_rescans().is_empty());
    assert!(
        read.pending_rescans()[0].scope == "window:4",
        "the rescan is bounded to the window, not the whole desktop"
    );
}

/// A source whose generation does not advance is an **epoch reset**: the cursor is
/// discarded and the scope is rescanned (`ARCH/21` §4).
#[test]
fn an_epoch_reset_discards_the_cursor_and_demands_a_rescan() {
    let tracker = UiaEpochTracker::new();
    let w = window(6);
    // First read at source generation 7.
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 0, 0, None));
    let first = read_window_with(
        fake,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &tracker,
    );
    assert_eq!(first.read.epoch, SnapshotEpoch(7));
    assert!(first.transition.reset.is_none());

    // A source whose generation moves **backwards** has reset: its cursor cannot
    // be trusted across that, so it is discarded and the scope rescanned.
    let mut rewound = FakeUia::new();
    rewound.epoch = SnapshotEpoch(3);
    rewound.add(0, button(1, "Save", 0, 0, None));
    let second = read_window_with(
        rewound,
        &w,
        &CollectConfig::with_budget(Duration::from_millis(900)),
        &tracker,
    );
    let reset = second
        .transition
        .reset
        .as_deref()
        .expect("a rewound generation is an epoch reset");
    assert!(reset.contains("epoch reset"), "{reset}");
    assert!(reset.contains("7 -> 3"), "{reset}");
    assert!(!second.transition.pending.is_empty());
    assert_eq!(second.transition.pending[0].scope, "window:6");
    // And the stamped generation still advanced locally, so handles still expire.
    assert!(second.read.epoch.0 > first.read.epoch.0);
}

/// A source that reports a *constant* generation is not a reset — the local
/// generation carries the identity, and it never repeats.
#[test]
fn a_source_that_never_moves_still_gets_a_fresh_generation_per_read() {
    let tracker = UiaEpochTracker::new();
    let w = window(7);
    let epochs: Vec<SnapshotEpoch> = (0..3)
        .map(|_| {
            let mut fake = FakeUia::new();
            fake.epoch = SnapshotEpoch(42);
            fake.add(0, button(1, "Save", 0, 0, None));
            read_window_with(
                fake,
                &w,
                &CollectConfig::with_budget(Duration::from_millis(900)),
                &tracker,
            )
        })
        .map(|r| r.read.epoch)
        .collect();
    assert!(epochs[0] < epochs[1] && epochs[1] < epochs[2], "{epochs:?}");
    assert!(epochs.iter().all(|e| e.0 >= 42), "{epochs:?}");
}

// ---------------------------------------------------------------------------
// 7. Protected fields: screen content is untrusted input
// ---------------------------------------------------------------------------

/// `REQ-CUA-009` masked-field rule: a password field's text never reaches an
/// observation, and its subtree is not walked.
#[test]
fn a_protected_field_is_masked_and_its_subtree_is_not_walked() {
    let mut fake = FakeUia::new();
    let mut secret = UiaNode::new(1, "Edit", "hunter2-the-password", region(0, 0, 200, 20));
    secret.is_password = true;
    // A child that must never be read.
    fake.add(1, button(2, "clear", 0, 0, None));
    fake.add(0, secret);

    let read = observe(fake, &window(1)).read;
    let field = read
        .elements
        .iter()
        .find(|e| e.role() == "Edit")
        .expect("the field");
    assert_eq!(field.name(), agentcowork_computeruse::uia::MASKED);
    assert!(field.handle.protected_field);
    assert!(
        !read
            .elements
            .iter()
            .any(|e| e.name() == "hunter2-the-password"),
        "the protected text must not appear anywhere in the observation"
    );
    assert!(
        !read.elements.iter().any(|e| e.name() == "clear"),
        "a protected field's subtree is not walked"
    );
    // The masking is the collector's own rule, applied once, and re-checked here.
    let masked = sanitize(
        UiaNode {
            is_password: true,
            ..UiaNode::new(9, "Edit", "leak me", region(0, 0, 1, 1))
        },
        500,
    );
    assert_eq!(masked.name, agentcowork_computeruse::uia::MASKED);
}

// ---------------------------------------------------------------------------
// 8. Browser content belongs on the browser rung (CDP), not on UIA
// ---------------------------------------------------------------------------

/// Chromium's UIA provider is opt-in and its default surface is coarser than the
/// real tree, so a UIA read of a browser window is annotated with guidance toward
/// the browser rung.
#[test]
fn a_browser_window_carries_guidance_to_the_browser_rung() {
    let mut win = window(2);
    win.app = "Chrome_WidgetWin_1".into();
    win.title = "Docs — Chromium".into();
    let hint = provider_hint(&win).expect("a Chromium window is recognised");
    assert_eq!(hint.id, "browser");
    assert!(hint.guidance.contains("CDP"), "{}", hint.guidance);
    assert!(hint.guidance.contains("ARCH/23"), "{}", hint.guidance);

    // The annotation reaches the read's guidance too.
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "Save", 0, 0, None));
    let read = observe(fake, &win).read;
    let guidance = read.guidance(&win).expect("guidance");
    assert!(guidance.contains("CDP"), "{guidance}");

    // A native app gets no such annotation — the hint is not a blanket excuse.
    assert!(provider_hint(&window(1)).is_none());
}

// ---------------------------------------------------------------------------
// 9. Assembly: one implementation, no second path
// ---------------------------------------------------------------------------

/// `assemble` is the single place a walk becomes an observation, so its status
/// derivation is pinned directly.
#[test]
fn assembly_maps_outcomes_to_the_honest_status() {
    let w = window(1);
    let items = vec![CollectedElement {
        parent: None,
        node: button(1, "Save", 0, 0, None),
        path: "1".into(),
        depth: 0,
    }];
    // Complete.
    let read = assemble(
        &w,
        ReadBounds::default(),
        CollectionOutcome::Complete { nodes: 1 },
        items.clone(),
        SnapshotEpoch(1),
    );
    assert_eq!(read.status, UiaReadStatus::Complete);
    assert_eq!(read.node_count(), 1);
    assert!(read.tree.is_some());
    assert_eq!(read.tree.as_ref().unwrap().name, "Save");

    // Partial keeps what was collected.
    let read = assemble(
        &w,
        ReadBounds::default(),
        CollectionOutcome::Partial {
            nodes: 1,
            unknown: agentcowork_computeruse::uia::UnknownRegion {
                kind: UnknownRegionKind::ProviderBound,
                detail: "node budget exhausted".into(),
                observed_at_ms: 0,
            },
        },
        items,
        SnapshotEpoch(2),
    );
    assert_eq!(read.status.as_str(), "partial");
    assert_eq!(read.node_count(), 1);
    assert!(!read.may_infer_absence());

    // A partial with nothing collected is `unknown`, not a usable empty read.
    let read = assemble(
        &w,
        ReadBounds::default(),
        CollectionOutcome::Partial {
            nodes: 0,
            unknown: agentcowork_computeruse::uia::UnknownRegion {
                kind: UnknownRegionKind::ProviderHung,
                detail: "no node arrived".into(),
                observed_at_ms: 0,
            },
        },
        vec![],
        SnapshotEpoch(3),
    );
    assert_eq!(read.status.as_str(), "unknown");
    assert!(!read.status.is_usable());
}

/// The collector's own traversal is reachable and bounded independently of the
/// thread wrapper, which is what makes the bounds testable without timing luck.
#[test]
fn the_walk_streams_every_element_in_document_order() {
    let mut fake = FakeUia::new();
    fake.add(0, button(1, "File", 0, 0, None));
    fake.add(0, button(2, "Edit", 0, 30, None));
    fake.add(0, button(3, "Help", 0, 60, None));
    let sink = agentcowork_computeruse::uia::SharedSink::new();
    let outcome = collect(
        &mut fake,
        &window(1),
        &CollectConfig::with_deadline(Instant::now() + Duration::from_millis(500)),
        &sink,
    );
    assert!(matches!(outcome, CollectionOutcome::Complete { nodes: 4 }));
    let items = sink.take();
    assert_eq!(
        items.iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
        vec!["1", "1.1", "1.2", "1.3"]
    );
    assert!(fake.call_count() > 0, "the provider was actually walked");
    // The corpus a resolution matches is the same set the tree contains.
    let read = assemble(
        &window(1),
        ReadBounds::default(),
        outcome,
        items,
        SnapshotEpoch(1),
    );
    assert_eq!(read.node_count(), 4);
    assert_eq!(read.tree.as_ref().unwrap().children.len(), 3);
    let named = read
        .tree
        .as_ref()
        .unwrap()
        .children
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        named,
        vec!["File", "Edit", "Help"],
        "document order preserved"
    );
}

/// A nested tree is built as a tree, and the index paths address it.
#[test]
fn a_nested_tree_keeps_its_shape_and_index_paths() {
    let mut fake = FakeUia::new();
    let menu = fake.add(0, button(1, "File", 0, 0, None));
    fake.add(menu, button(2, "Open", 0, 20, None));
    fake.add(menu, button(3, "Save", 0, 50, None));
    let read = observe(fake, &window(1)).read;
    // `1` is the window root, `1.1` the File menu, its children `1.1.1`/`1.1.2`.
    let root = read.tree.as_ref().expect("tree");
    assert_eq!(root.role, "Pane");
    assert_eq!(root.children.len(), 1);
    let file = &root.children[0];
    assert_eq!(file.name, "File");
    assert_eq!(file.children.len(), 2);
    let paths: Vec<_> = read.elements.iter().map(|e| e.index_path.clone()).collect();
    assert!(paths.contains(&"1.1.2".to_string()), "{paths:?}");
    let save = resolve_in(&read, &ElementQuery::named("Save"));
    assert_eq!(save.handle().expect("resolved").index_path, "1.1.2");
}

/// `NameMatch` ordering is the resolution's strength rule, so it is pinned.
#[test]
fn name_match_strength_is_exact_then_prefix_then_substring() {
    assert!(NameMatch::Exact < NameMatch::Prefix);
    assert!(NameMatch::Prefix < NameMatch::Contains);
    let read = observe(
        {
            let mut f = FakeUia::new();
            f.add(0, button(1, "Save as", 0, 0, None));
            f
        },
        &window(1),
    )
    .read;
    // "Save" is a prefix of "Save as", and the only candidate, so it resolves.
    assert!(resolve_in(&read, &ElementQuery::named("Save")).is_resolved());
    // "sav" is a substring only — still the sole candidate at that strength.
    assert!(resolve_in(&read, &ElementQuery::named("sav")).is_resolved());
}
