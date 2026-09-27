//! Guard-2 for desktop computer-use (E9): app allow-list, a confirmation
//! taxonomy (delete / money / install / CAPTCHA / transmit), hard denies
//! (Terminal, Run, Win-key, lock screen, UAC, password managers, AgentCowork's
//! own UI), a kill switch, per-minute rate limiting, safe zones (taskbar /
//! notification area), and a Merkle-audit sink.
//!
//! The human gate itself (`PermissionGate`) is a seam: the desktop host backs
//! it with `agentcowork-guard::TicketStore` (mint → approve → use) so every
//! effect rides the same dual-guard as every other effect in the product.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::types::{ActKind, Region};

/// What an action touches, classified for the confirmation card.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ConfirmClass {
    /// Destructive / data-loss (delete, overwrite, format…).
    Delete,
    /// Financial effect (pay, buy, send money, order…).
    Money,
    /// Installs software or makes system-level changes (install, uninstall, elevate…).
    Install,
    /// Anti-bot challenge (CAPTCHA) — always human.
    Captcha,
    /// Transmits data off-machine (send, upload, share, email…).
    Transmit,
    /// The **lowest rung of the click ladder**: injecting real pointer/keyboard
    /// input, which moves the user's cursor and takes focus. Never produced by
    /// [`ConfirmClass::classify`] (it is not a keyword class — it is a property
    /// of the *mechanism*) and never derived from the target; it is raised by
    /// [`DesktopGuard::authorize_rung`] when a click ladder reaches
    /// [`crate::ladder::ClickRung::RawInput`], so the decision rides the same
    /// policy → human gate → audit path as every other effect rather than a
    /// second authorization route.
    CursorTakeover,
    /// Ordinary navigation / read / benign click.
    Routine,
}

impl ConfirmClass {
    /// Keyword classification for a click target or key (deterministic).
    pub fn classify(target: &str, key: Option<&str>) -> ConfirmClass {
        let t = target.to_ascii_lowercase();
        let k = key.map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let hay = format!("{t} {k}");
        // Hard safety first: nothing below can override these.
        if [
            "delete",
            "remove",
            "uninstall",
            "format",
            "erase",
            "overwrite",
            "trash",
            "purge",
        ]
        .iter()
        .any(|w| hay.contains(w))
        {
            return ConfirmClass::Delete;
        }
        if [
            "buy",
            "purchase",
            "checkout",
            "pay",
            "payment",
            "transfer",
            "send money",
            "$",
            "price",
            "checkout",
        ]
        .iter()
        .any(|w| hay.contains(w))
        {
            return ConfirmClass::Money;
        }
        if [
            "install", "setup", "update", "upgrade", "elevate", "sudo", "admin",
        ]
        .iter()
        .any(|w| hay.contains(w))
        {
            return ConfirmClass::Install;
        }
        if [
            "captcha",
            "verify you are human",
            "i am not a robot",
            "challenge",
        ]
        .iter()
        .any(|w| hay.contains(w))
        {
            return ConfirmClass::Captcha;
        }
        if [
            "send", "submit", "post", "upload", "share", "email", "publish", "transfer", "export",
        ]
        .iter()
        .any(|w| hay.contains(w))
        {
            return ConfirmClass::Transmit;
        }
        ConfirmClass::Routine
    }
}

/// Patterns that are NEVER automated — hard denies regardless of allow-list.
const HARD_DENY_APP: &[&str] = &[
    "terminal",
    "windows terminal",
    "command prompt",
    "powershell",
    "run",
    "uac",
    "user account control",
    "password manager",
    "keepass",
    "bitwarden",
    "1password",
    "lastpass",
    "lock screen",
    "sign in",
    "login",
];
/// AgentCowork's own UI is never driven by the agent (no self-puppetry).
/// Both eras match (DEC-053): the current product name and the retired one,
/// so a renamed window is still denied and an old-branded one is too.
const APP_NAMES: &[&str] = &[
    "agentcowork",
    "agentcowork desktop",
    "everyaios",
    "everyaios desktop",
];

/// Keys that are never synthesised (Win-key, lock, UAC combos…).
const HARD_DENY_KEY: &[&str] = &[
    "super",
    "super_l",
    "super_r",
    "win",
    "ctrl+alt+del",
    "lock",
    "print",
];

/// Confirm classes that always need a human before execution.
const ALWAYS_CONFIRM: &[ConfirmClass] = &[
    ConfirmClass::Delete,
    ConfirmClass::Money,
    ConfirmClass::Install,
    ConfirmClass::Captcha,
    ConfirmClass::Transmit,
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GateDecision {
    /// Allow without a human (allow-listed + routine).
    Allow,
    /// Needs a human confirmation card.
    Confirm(ConfirmClass),
    /// Permanently denied by policy (hard deny / off allow-list in strict mode).
    Deny,
}

impl GateDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateDecision::Allow => "allow",
            GateDecision::Confirm(_) => "confirm",
            GateDecision::Deny => "deny",
        }
    }
}

/// The human-in-the-loop gate — host wires this to the TicketStore.
pub trait PermissionGate: Send + Sync {
    /// Ask for approval of a classified action. `expected_confirmation` must
    /// match the class the caller was told to confirm.
    fn request(&self, act: &ActKind, class: ConfirmClass) -> GateDecision;
}

/// A gate that approves nothing — fail-closed default.
pub struct DenyAllGate;
impl PermissionGate for DenyAllGate {
    fn request(&self, _act: &ActKind, _class: ConfirmClass) -> GateDecision {
        GateDecision::Deny
    }
}

/// Who initiated a desktop act.
///
/// The desktop engine is **one shared instance** (one platform backend, one
/// live policy applied from Settings), so provenance cannot live on the engine —
/// it is a property of the *caller*. The user's own click and the inbuilt
/// agent's tool call reach the same `act_with`; only this value distinguishes
/// them, and it is what prevents an agent-initiated desktop action from being
/// filed as a **human gesture** on the Merkle chain (the audit-lie class this
/// crate must not create).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActProvenance {
    /// The user's own gesture in the UI (Settings / Computer-use commands).
    #[default]
    HumanGesture,
    /// An inbuilt-agent tool call. The tool path already authorized the turn;
    /// the host files this under its agent authority class.
    Agent,
    /// Scheduler/automation-initiated (reserved — automation Work carries
    /// `AutomationProvenance` from the P71.3c Work factory, spec §4.3).
    Automation,
}

impl ActProvenance {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActProvenance::HumanGesture => "human_gesture",
            ActProvenance::Agent => "agent",
            ActProvenance::Automation => "automation",
        }
    }
}

/// An audit sink — the host backs it with `agentcowork-audit::AuditWriter`
/// (Merkle chain); a no-op sink exists for tests/unsupervised use.
pub trait AuditSink: Send + Sync {
    /// Record one Guard-2 desktop decision. `provenance` says who initiated the
    /// act, so the host files it under the correct authority class instead of
    /// assuming a human gesture.
    fn write(&self, kind: &str, payload: serde_json::Value, provenance: ActProvenance);
}

/// No-op audit (tests only — production must pass a real sink).
pub struct NoopSink;
impl AuditSink for NoopSink {
    fn write(&self, _kind: &str, _payload: serde_json::Value, _provenance: ActProvenance) {}
}

/// P57.3/P57.4 — how the driver is allowed to touch the desktop by default.
///
/// The spec's rule (Cua no-foreground contract) is **Background**: drive the
/// target without stealing the user's cursor, keyboard, or frontmost window.
/// Foreground is an escalation — a modal that ignores background input, the
/// user asking, or a challenge needing HITL — and it restores the previous
/// foreground afterwards (P57.4). It is a deliberate default change here, not
/// something an agent may flip.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum InteractionMode {
    #[default]
    Background,
    Foreground,
}

impl InteractionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            InteractionMode::Background => "background",
            InteractionMode::Foreground => "foreground",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "background" => Some(InteractionMode::Background),
            "foreground" => Some(InteractionMode::Foreground),
            _ => None,
        }
    }
}

/// Windows canonical paths arrive as `\\?\C:\…`; display and matching use the
/// plain form so a persisted file copied between shells still matches.
fn normalize_path(p: &str) -> String {
    let p = p.trim();
    let p = p.strip_prefix("\\\\?\\").unwrap_or(p);
    p.replace('\\', "/").to_ascii_lowercase()
}

/// The last path segment, minus a `.exe` / `.app` suffix — i.e. the name a
/// window list reports for that app (`/usr/bin/firefox` → `firefox`,
/// `/Applications/Safari.app` → `safari`).
fn stem_of(p: &str) -> String {
    let norm = normalize_path(p);
    let base = norm.rsplit('/').next().unwrap_or(norm.as_str());
    base.strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".app"))
        .unwrap_or(base)
        .to_string()
}

/// App allow-list: default-deny for unlisted apps in strict mode; in standard
/// mode unlisted apps are Confirm(Routine) — never silently allowed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppPolicy {
    /// Allowed app names / process names (case-insensitive).
    pub allow_list: Vec<String>,
    /// strict = unlisted apps are Deny; standard = unlisted are Confirm(Routine).
    pub strict: bool,
    /// Custom safe zones (screen rects never acted on), e.g. the taskbar.
    pub safe_zones: Vec<Region>,
    /// P57.2 — allow-listed **exact paths**: once a canonical path is here the
    /// agent may launch it without the user turning computer use on per
    /// session (risky classes still Guard-2). A window whose app name matches a
    /// listed path's file stem is covered too, which is how one Settings row
    /// covers both the launch and the window. `serde(default)` keeps a policy
    /// file written before this field readable.
    #[serde(default)]
    pub allow_paths: Vec<String>,
    /// P57.3 — the interaction default the engine enforces (see
    /// [`InteractionMode`]).
    #[serde(default)]
    pub interaction_mode: InteractionMode,
}

impl AppPolicy {
    pub fn allow(mut self, app: impl Into<String>) -> Self {
        self.allow_list.push(app.into().to_ascii_lowercase());
        self
    }

    pub fn with_safe_zone(mut self, zone: Region) -> Self {
        self.safe_zones.push(zone);
        self
    }

    /// P57.3 — may the driver raise/activate another window right now? False
    /// under the Background default: raising is a foreground escalation.
    pub fn allows_raising_windows(&self) -> bool {
        self.interaction_mode == InteractionMode::Foreground
    }

    /// P57.2 — add a canonical allow-listed path. Refuses a path that does not
    /// resolve, and refuses one whose program is on the hard-deny list
    /// (terminal / password manager / …), so the allow-list can never be used
    /// to make a never-automatable app automatable. Idempotent.
    pub fn add_path(&mut self, path: &str) -> Result<String, String> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err("empty path".into());
        }
        let canonical =
            std::fs::canonicalize(trimmed).map_err(|e| format!("resolve {trimmed}: {e}"))?;
        let meta = std::fs::metadata(&canonical)
            .map_err(|e| format!("stat {}: {e}", canonical.display()))?;
        if !(meta.is_file() || meta.is_dir()) {
            return Err(format!("{} is not a file or bundle", canonical.display()));
        }
        let canonical = canonical.to_string_lossy().into_owned();
        let name = stem_of(&canonical);
        if let Some(reason) = Self::hard_deny(&name, None) {
            return Err(reason);
        }
        if self
            .allow_paths
            .iter()
            .any(|p| normalize_path(p) == normalize_path(&canonical))
        {
            return Ok(canonical);
        }
        self.allow_paths.push(canonical.clone());
        Ok(canonical)
    }

    /// P57.2 — drop an allow-listed path (the Settings Remove action). Matches
    /// on the normalized form so a `\\?\`-prefixed or differently-separated
    /// entry still removes.
    pub fn remove_path(&mut self, path: &str) -> bool {
        let needle = normalize_path(path);
        let before = self.allow_paths.len();
        self.allow_paths.retain(|p| normalize_path(p) != needle);
        self.allow_paths.len() != before
    }

    /// P57.2 — allow-list membership: a name/env match (legacy) **or** an
    /// allow-listed path that matches either the reported full path or the
    /// bare app name a window list reports.
    fn is_allow_listed(&self, app: &str) -> bool {
        if app.trim().is_empty() {
            return false;
        }
        let app_lower = app.to_ascii_lowercase();
        if self.allow_list.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            app_lower.contains(&a) || a.contains(&app_lower)
        }) {
            return true;
        }
        self.matches_allowed_path(app)
    }

    fn matches_allowed_path(&self, app: &str) -> bool {
        let needle = normalize_path(app);
        if needle.is_empty() {
            return false;
        }
        let needle_stem = stem_of(&needle);
        self.allow_paths.iter().any(|p| {
            let p = normalize_path(p);
            if needle == p {
                return true;
            }
            // The caller reported a full path under a listed one (a helper
            // inside the bundle).
            if needle.starts_with('/') && p.ends_with(&needle) {
                return true;
            }
            // The caller reported the bare app name (window list). ≥3 chars so
            // a two-letter stem cannot match by accident.
            needle_stem.len() >= 3 && needle_stem == stem_of(&p)
        })
    }

    /// Hard-deny check: never automate terminal/run/password-managers/UAC/
    /// lock screen / AgentCowork itself, regardless of the allow-list.
    pub fn hard_deny(app: &str, key: Option<&str>) -> Option<String> {
        let app = app.to_ascii_lowercase();
        if HARD_DENY_APP.iter().any(|h| app.contains(h)) {
            return Some(format!("app \"{app}\" is on the hard-deny list"));
        }
        if APP_NAMES.iter().any(|e| app.contains(e)) {
            return Some("AgentCowork's own UI is never driven by the agent".into());
        }
        if let Some(k) = key {
            let k = k.to_ascii_lowercase();
            if HARD_DENY_KEY.iter().any(|h| k.contains(h)) {
                return Some(format!("key \"{k}\" is on the hard-deny list"));
            }
        }
        None
    }

    /// Classify one action against policy → the gate decision to surface.
    pub fn evaluate(
        &self,
        app: &str,
        act: &ActKind,
        key: Option<&str>,
    ) -> Result<GateDecision, String> {
        if let Some(reason) = Self::hard_deny(app, key) {
            return Err(reason);
        }
        let class = match act {
            ActKind::Click { x, y }
            | ActKind::Scroll { x, y, .. }
            | ActKind::Drag { from: (x, y), .. } => {
                let pt = (*x, *y);
                if self.safe_zones.iter().any(|z| z.contains(pt.0, pt.1)) {
                    return Err("point is inside a safe zone".into());
                }
                ConfirmClass::Routine
            }
            ActKind::ClickByName { name } | ActKind::SetValue { name, .. } => {
                ConfirmClass::classify(name, key)
            }
            ActKind::Type { .. } | ActKind::Press { .. } => ConfirmClass::Routine,
            // P57.1 — classify the launched program itself: the canonical path
            // when known (an `.exe` under an installer directory, a macOS
            // bundle), else the name.
            ActKind::LaunchApp { .. } => {
                ConfirmClass::classify(act.launch_target().unwrap_or_default(), None)
            }
            ActKind::ActivateWindow { .. } => ConfirmClass::Routine,
        };
        if ALWAYS_CONFIRM.contains(&class) {
            return Ok(GateDecision::Confirm(class));
        }
        if self.is_allow_listed(app) {
            return Ok(GateDecision::Allow);
        }
        if self.strict {
            Ok(GateDecision::Deny)
        } else {
            // Unlisted app, routine action → still confirm (never silent).
            Ok(GateDecision::Confirm(ConfirmClass::Routine))
        }
    }

    /// [`AppPolicy::evaluate`] for a rung that the click ladder reached, where
    /// the decision hinges on the **mechanism** rather than on the act's
    /// keyword class.
    ///
    /// Every check `evaluate` performs still applies — hard denies, safe zones,
    /// launch subject, allow-list, strict mode. The only difference: a rung
    /// whose gate is `HumanAuthorization` (today: the pointer-moving raw-input
    /// rung) is upgraded from `Allow` to `Confirm(CursorTakeover)`, so the
    /// human gate is always asked. An allow-listed app is a permission to
    /// *drive* it, not a permission to hijack the operator's mouse.
    pub fn evaluate_rung(
        &self,
        app: &str,
        act: &ActKind,
        key: Option<&str>,
    ) -> Result<GateDecision, String> {
        let decision = self.evaluate(app, act, key)?;
        Ok(match decision {
            GateDecision::Allow => GateDecision::Confirm(ConfirmClass::CursorTakeover),
            other => other,
        })
    }
}

/// Global kill switch — once stopped, every engine op fails closed.
#[derive(Debug, Default)]
pub struct KillSwitch {
    stopped: AtomicBool,
}

impl KillSwitch {
    pub fn new() -> Self {
        Self {
            stopped: AtomicBool::new(false),
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.stopped.store(false, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.is_stopped() {
            Err("emergency stop engaged — every desktop op fails closed".into())
        } else {
            Ok(())
        }
    }
}

/// Per-minute action rate limit (default 20 actions/min — ChatGPT-class
/// pacing; a runaway loop trips this before it can thrash the desktop).
#[derive(Debug)]
pub struct RateLimiter {
    max_per_minute: u32,
    window: Mutex<Vec<Instant>>,
}

impl RateLimiter {
    pub fn new(max_per_minute: u32) -> Self {
        Self {
            max_per_minute: max_per_minute.max(1),
            window: Mutex::new(Vec::new()),
        }
    }

    /// Returns Ok if the action may proceed; Err with the retry-after when
    /// the budget is exhausted.
    pub fn allow(&self) -> Result<(), Duration> {
        let now = Instant::now();
        let mut w = self.window.lock().unwrap();
        w.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        if w.len() >= self.max_per_minute as usize {
            let oldest = w.first().copied().unwrap_or(now);
            let retry = Duration::from_secs(60).saturating_sub(now.duration_since(oldest));
            return Err(retry);
        }
        w.push(now);
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.window.lock().unwrap().len()
    }
}

/// The assembled Guard-2 desktop gate.
///
/// P57.8 — the policy is behind a lock so the host (Settings → Computer use)
/// can apply an allow-list change to the **live** engine instead of only the
/// next process start; every preflight reads one consistent snapshot.
pub struct DesktopGuard {
    policy: Mutex<AppPolicy>,
    pub gate: Box<dyn PermissionGate>,
    pub kill: KillSwitch,
    pub limiter: RateLimiter,
    pub sink: Box<dyn AuditSink>,
}

impl DesktopGuard {
    pub fn new(policy: AppPolicy, gate: Box<dyn PermissionGate>, sink: Box<dyn AuditSink>) -> Self {
        Self {
            policy: Mutex::new(policy),
            gate,
            kill: KillSwitch::new(),
            limiter: RateLimiter::new(20),
            sink,
        }
    }

    /// The current policy (a clone — callers cannot mutate policy in place).
    pub fn policy(&self) -> AppPolicy {
        self.policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace the policy. Takes effect on the next preflight.
    pub fn set_policy(&self, policy: AppPolicy) {
        *self.policy.lock().unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Full pre-action gate: kill switch → rate limit → policy → human gate →
    /// audit. Returns the decision; on Allow the caller executes.
    ///
    /// This is the **human-gesture** form — the user's own UI action. An
    /// agent-initiated act must use [`DesktopGuard::preflight_with`] so the audit
    /// row carries the right provenance.
    pub fn preflight(
        &self,
        app: &str,
        act: &ActKind,
        key: Option<&str>,
    ) -> Result<GateDecision, String> {
        self.preflight_with(app, act, key, ActProvenance::HumanGesture)
    }

    /// [`DesktopGuard::preflight`] with explicit provenance. The gate logic is
    /// identical; only the audit row's authority class differs.
    pub fn preflight_with(
        &self,
        app: &str,
        act: &ActKind,
        key: Option<&str>,
        provenance: ActProvenance,
    ) -> Result<GateDecision, String> {
        self.kill.check()?;
        self.limiter
            .allow()
            .map_err(|retry_after| format!("rate limit: retry in {:?}", retry_after))?;
        let decision = self.policy().evaluate(app, act, key)?;
        let final_decision = match decision {
            GateDecision::Confirm(class) => self.gate.request(act, class),
            other => other,
        };
        self.audit(app, act, &final_decision, provenance);
        Ok(final_decision)
    }

    fn audit(&self, app: &str, act: &ActKind, decision: &GateDecision, provenance: ActProvenance) {
        let payload = serde_json::json!({
            "surface": "desktop",
            "app": app,
            "act": act.describe(),
            "interaction": self.policy().interaction_mode.as_str(),
            "decision": decision.as_str(),
            "provenance": provenance.as_str(),
            "class": match decision {
                GateDecision::Confirm(c) => format!("{c:?}"),
                _ => "routine".to_string(),
            },
        });
        self.sink.write("desktop.guard2", payload, provenance);
    }

    // ---- the click ladder's gate --------------------------------------

    /// Authorize (or refuse) one rung of the click ladder.
    ///
    /// This is the **same** path as [`DesktopGuard::preflight_with`] — kill
    /// switch, rate limit, policy evaluation, human gate, audit — with one
    /// addition: a rung whose [`RungGate`](crate::ladder::RungGate) is
    /// `HumanAuthorization` cannot come out `Allow` from the policy alone. It
    /// is raised to `Confirm(CursorTakeover)` and pushed through the host's
    /// `PermissionGate`, which is backed by the Guard ticket store. That is
    /// deliberate:
    ///
    /// - The architecture marks the pointer-moving rung as needing a human
    ///   decision, and an allow-listed *app* is not that decision.
    /// - Routing it here means the ladder cannot become a Guard bypass (I12):
    ///   the trait the ladder drives ([`crate::ladder::ClickLadderDriver`]) has
    ///   no other authorization method, so there is nowhere to go around it.
    /// - It costs a second rate-limit token and a second audit row, because it
    ///   genuinely is a second authorization decision, not a re-read of the
    ///   first.
    ///
    /// `Err(reason)` is the refusal the ladder surfaces verbatim; `Ok(())` is
    /// the grant. A hard policy error (hard-deny app, safe zone, kill switch)
    /// also arrives as `Err`, so the ladder stops there too.
    pub fn authorize_rung(
        &self,
        app: &str,
        act: &ActKind,
        key: Option<&str>,
        provenance: ActProvenance,
        rung: crate::ladder::ClickRung,
    ) -> Result<(), String> {
        self.kill.check()?;
        self.limiter.allow().map_err(|retry_after| {
            format!(
                "rate limit: retry in {:?} before rung {} runs",
                retry_after,
                rung.as_str()
            )
        })?;
        // A gated rung is classified by its mechanism, not by a keyword on the
        // target: the act has already been classified above and the escalation
        // question is "may we move this human's cursor", which is orthogonal to
        // "is this a Delete".
        let policy_decision = if rung.gate() == crate::ladder::RungGate::HumanAuthorization {
            self.policy().evaluate_rung(app, act, key)?
        } else {
            self.policy().evaluate(app, act, key)?
        };
        let decision = match policy_decision {
            GateDecision::Confirm(class) => self.gate.request(act, class),
            other => other,
        };
        self.audit_rung(app, act, rung, &decision, provenance);
        match decision {
            GateDecision::Allow => Ok(()),
            GateDecision::Confirm(class) => Err(format!(
                "rung {} needs a human decision: gate returned confirm({class:?})",
                rung.as_str()
            )),
            GateDecision::Deny => Err(format!(
                "rung {} refused: gate decision deny",
                rung.as_str()
            )),
        }
    }

    /// Audit one rung decision, then the ladder verdict itself. Both rows are
    /// on the same Merkle chain as every other desktop effect, so "which rung
    /// ran" and "who authorized it" are both reconstructable after the fact.
    pub fn audit_rung(
        &self,
        app: &str,
        act: &ActKind,
        rung: crate::ladder::ClickRung,
        decision: &GateDecision,
        provenance: ActProvenance,
    ) {
        let payload = serde_json::json!({
            "surface": "desktop",
            "kind": "click_ladder_rung",
            "app": app,
            "act": act.describe(),
            "rung": rung.as_str(),
            "rung_rank": rung.rank(),
            "rung_mechanism": rung.describe(),
            "moves_pointer": rung.moves_pointer(),
            "requires_foreground": rung.requires_foreground(),
            "gate": match rung.gate() {
                crate::ladder::RungGate::Automatic => "automatic",
                crate::ladder::RungGate::HumanAuthorization => "human_authorization",
            },
            "interaction": self.policy().interaction_mode.as_str(),
            "decision": decision.as_str(),
            "provenance": provenance.as_str(),
        });
        self.sink.write("desktop.guard2.rung", payload, provenance);
    }

    /// Record the verdict of a whole ladder walk: which rung ran, what was
    /// tried first, and whether the walk escalated.
    pub fn audit_ladder(
        &self,
        app: &str,
        act: &ActKind,
        verdict: &crate::ladder::LadderVerdict,
        provenance: ActProvenance,
    ) {
        self.sink.write(
            "desktop.ladder",
            serde_json::json!({
                "surface": "desktop",
                "app": app,
                "act": act.describe(),
                "interaction": self.policy().interaction_mode.as_str(),
                "provenance": provenance.as_str(),
                "verdict": verdict.to_json(),
            }),
            provenance,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AllowGate;
    impl PermissionGate for AllowGate {
        fn request(&self, _act: &ActKind, _class: ConfirmClass) -> GateDecision {
            GateDecision::Allow
        }
    }

    struct ConfirmAllGate;
    impl PermissionGate for ConfirmAllGate {
        fn request(&self, _act: &ActKind, _class: ConfirmClass) -> GateDecision {
            GateDecision::Confirm(_class)
        }
    }

    fn gate_allow() -> DesktopGuard {
        DesktopGuard::new(
            AppPolicy::default().allow("notepad"),
            Box::new(AllowGate),
            Box::new(NoopSink),
        )
    }

    #[test]
    fn taxonomy_classifies_risky_targets() {
        assert_eq!(
            ConfirmClass::classify("Delete file", None),
            ConfirmClass::Delete
        );
        assert_eq!(ConfirmClass::classify("Buy now", None), ConfirmClass::Money);
        assert_eq!(
            ConfirmClass::classify("Install package", None),
            ConfirmClass::Install
        );
        assert_eq!(
            ConfirmClass::classify("I am not a robot", None),
            ConfirmClass::Captcha
        );
        assert_eq!(
            ConfirmClass::classify("Send email", None),
            ConfirmClass::Transmit
        );
        assert_eq!(
            ConfirmClass::classify("Save document", None),
            ConfirmClass::Routine
        );
    }

    #[test]
    fn hard_deny_apps_never_automated() {
        assert!(AppPolicy::hard_deny("Windows Terminal", None).is_some());
        assert!(AppPolicy::hard_deny("1Password", None).is_some());
        assert!(AppPolicy::hard_deny("AgentCowork", None).is_some());
        assert!(AppPolicy::hard_deny("notepad", None).is_none());
        assert!(AppPolicy::hard_deny("notepad", Some("Super_L")).is_some());
        assert!(AppPolicy::hard_deny("notepad", Some("Enter")).is_none());
    }

    #[test]
    fn allow_listed_routine_is_allow() {
        let g = gate_allow();
        let d = g
            .preflight("notepad", &ActKind::Click { x: 10, y: 10 }, None)
            .unwrap();
        assert_eq!(d, GateDecision::Allow);
    }

    #[test]
    fn risky_action_reaches_the_human_gate_even_when_allow_listed() {
        // The policy classifies Delete as Confirm; the HUMAN gate then decides.
        let g = DesktopGuard::new(
            AppPolicy::default().allow("notepad"),
            Box::new(ConfirmAllGate),
            Box::new(NoopSink),
        );
        let d = g
            .preflight(
                "notepad",
                &ActKind::ClickByName {
                    name: "Delete".into(),
                },
                None,
            )
            .unwrap();
        assert_eq!(d, GateDecision::Confirm(ConfirmClass::Delete));
        // With an approving human gate the same action is allowed (audited).
        let g2 = gate_allow();
        let d2 = g2
            .preflight(
                "notepad",
                &ActKind::ClickByName {
                    name: "Delete".into(),
                },
                None,
            )
            .unwrap();
        assert_eq!(d2, GateDecision::Allow);
    }

    #[test]
    fn unlisted_app_confirms_or_denies_by_mode() {
        // standard: unlisted + routine → Confirm reaches the human gate.
        let g = DesktopGuard::new(
            AppPolicy::default(),
            Box::new(ConfirmAllGate),
            Box::new(NoopSink),
        );
        let d = g
            .preflight("random-app", &ActKind::Click { x: 5, y: 5 }, None)
            .unwrap();
        assert_eq!(d, GateDecision::Confirm(ConfirmClass::Routine));
        // strict: unlisted → Deny before any human gate.
        let strict = DesktopGuard::new(
            AppPolicy {
                strict: true,
                ..AppPolicy::default()
            },
            Box::new(AllowGate),
            Box::new(NoopSink),
        );
        let d = strict
            .preflight("random-app", &ActKind::Click { x: 5, y: 5 }, None)
            .unwrap();
        assert_eq!(d, GateDecision::Deny);
    }

    #[test]
    fn safe_zone_blocks_points() {
        let g = DesktopGuard::new(
            AppPolicy::default()
                .allow("notepad")
                .with_safe_zone(Region {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                }),
            Box::new(AllowGate),
            Box::new(NoopSink),
        );
        let err = g
            .preflight("notepad", &ActKind::Click { x: 50, y: 50 }, None)
            .unwrap_err();
        assert!(err.contains("safe zone"));
    }

    #[test]
    fn kill_switch_fails_closed() {
        let g = gate_allow();
        g.kill.stop();
        assert!(
            g.preflight("notepad", &ActKind::Click { x: 1, y: 1 }, None)
                .is_err()
        );
        g.kill.resume();
        assert!(
            g.preflight("notepad", &ActKind::Click { x: 1, y: 1 }, None)
                .is_ok()
        );
    }

    #[test]
    fn rate_limiter_bounds_actions_per_minute() {
        let l = RateLimiter::new(3);
        for _ in 0..3 {
            assert!(l.allow().is_ok());
        }
        assert!(l.allow().is_err());
        assert_eq!(l.count(), 3);
    }

    #[test]
    fn default_interaction_is_background_and_forbids_raising() {
        // P57.3 — the Cua no-foreground contract is the default, not an opt-in.
        let p = AppPolicy::default();
        assert_eq!(p.interaction_mode, InteractionMode::Background);
        assert!(!p.allows_raising_windows());
        assert!(
            AppPolicy {
                interaction_mode: InteractionMode::Foreground,
                ..AppPolicy::default()
            }
            .allows_raising_windows()
        );
        assert_eq!(
            InteractionMode::parse("foreground"),
            Some(InteractionMode::Foreground)
        );
        assert_eq!(
            InteractionMode::parse("Background"),
            Some(InteractionMode::Background)
        );
        assert_eq!(InteractionMode::parse("sometimes"), None);
    }

    #[test]
    fn allow_listed_path_covers_launch_and_window_name() {
        // P57.2 — one Settings row covers both the launched exe and the window
        // the window list reports for it.
        let mut p = AppPolicy::default();
        p.allow_paths.push("/usr/lib/firefox/firefox".into());
        assert!(p.is_allow_listed("/usr/lib/firefox/firefox"));
        assert!(p.is_allow_listed("firefox"));
        assert!(p.is_allow_listed("Firefox"));
        // A different app is still not allow-listed.
        assert!(!p.is_allow_listed("notepad"));
        assert!(!p.is_allow_listed("/usr/bin/gedit"));
        // A macOS bundle covers the bare app name a window list reports.
        let mut mac = AppPolicy::default();
        mac.allow_paths.push("/Applications/Safari.app".into());
        assert!(mac.is_allow_listed("Safari"));
        assert!(mac.is_allow_listed("/Applications/Safari.app"));
        assert!(!mac.is_allow_listed("Firefox"));
        // An empty app name is never a wildcard.
        assert!(!mac.is_allow_listed(""));
        assert!(!mac.is_allow_listed("   "));
    }

    #[test]
    fn allow_listed_path_gates_the_same_way_as_a_name() {
        let mut p = AppPolicy::default();
        p.allow_paths.push("/usr/bin/gedit".into());
        let g = DesktopGuard::new(p, Box::new(AllowGate), Box::new(NoopSink));
        let d = g
            .preflight("gedit", &ActKind::Click { x: 4, y: 4 }, None)
            .unwrap();
        assert_eq!(d, GateDecision::Allow);
        // A risky class still reaches the human gate, path-listed or not.
        let g2 = DesktopGuard::new(
            {
                let mut p = AppPolicy::default();
                p.allow_paths.push("/usr/bin/gedit".into());
                p
            },
            Box::new(ConfirmAllGate),
            Box::new(NoopSink),
        );
        let d2 = g2
            .preflight(
                "gedit",
                &ActKind::ClickByName {
                    name: "Delete file".into(),
                },
                None,
            )
            .unwrap();
        assert_eq!(d2, GateDecision::Confirm(ConfirmClass::Delete));
    }

    #[test]
    fn add_path_refuses_hard_denied_and_missing_paths() {
        // A terminal is on the hard-deny list: the allow-list must never be a
        // way to make it automatable (the UI cannot offer it either).
        let dir = std::env::temp_dir().join(format!("ea-policy-deny-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let term = dir.join("gnome-terminal");
        std::fs::write(&term, b"#!/bin/sh\n").unwrap();

        let mut p = AppPolicy::default();
        let err = p.add_path(term.to_str().unwrap()).unwrap_err();
        assert!(err.contains("hard-deny"), "got: {err}");
        let missing = p.add_path("/nope/definitely-not-here").unwrap_err();
        assert!(missing.contains("resolve"), "got: {missing}");
        assert_eq!(p.add_path("   ").unwrap_err(), "empty path");
        assert!(
            p.allow_paths.is_empty(),
            "nothing may be added by a failure"
        );

        // The persisted policy round-trips (Settings writes, next boot reads)
        // and a file written before these fields existed still loads.
        let editor = dir.join("my-editor");
        std::fs::write(&editor, b"#!/bin/sh\n").unwrap();
        let mut ok = AppPolicy {
            interaction_mode: InteractionMode::Foreground,
            ..AppPolicy::default()
        };
        ok.add_path(editor.to_str().unwrap()).unwrap();
        let json = serde_json::to_string(&ok).unwrap();
        let back: AppPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.interaction_mode, InteractionMode::Foreground);
        assert_eq!(back.allow_paths.len(), 1);
        let legacy: AppPolicy =
            serde_json::from_str(r#"{"allow_list":["notepad"],"strict":false,"safe_zones":[]}"#)
                .unwrap();
        assert_eq!(legacy.interaction_mode, InteractionMode::Background);
        assert!(legacy.allow_paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_and_remove_path_are_canonical_and_idempotent() {
        let dir = std::env::temp_dir().join(format!("ea-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("my-editor");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();

        let mut p = AppPolicy::default();
        let added = p.add_path(bin.to_str().unwrap()).unwrap();
        assert_eq!(p.allow_paths.len(), 1);
        // Adding the same file again is a no-op, not a duplicate row.
        assert_eq!(p.add_path(bin.to_str().unwrap()).unwrap(), added);
        assert_eq!(p.allow_paths.len(), 1);
        assert!(p.remove_path(&added));
        assert!(p.allow_paths.is_empty());
        // Removing something that is not listed reports honestly.
        assert!(!p.remove_path(&added));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P57.2 — the launch subject is the program: an allow-listed path launches
    /// without a per-session toggle, and an unlisted one still needs the human
    /// path (or is denied in strict mode).
    #[test]
    fn an_allow_listed_launch_target_is_allowed_and_an_unlisted_one_is_not() {
        let listed = "/usr/bin/gedit";
        let launch = ActKind::launch_path(listed);
        let mut policy = AppPolicy::default();
        policy.allow_paths.push(listed.into());
        let g = DesktopGuard::new(policy, Box::new(AllowGate), Box::new(NoopSink));
        let subject = launch.launch_target().unwrap();
        assert_eq!(subject, listed);
        assert_eq!(
            g.preflight(subject, &launch, None).unwrap(),
            GateDecision::Allow
        );
        // A different program is not covered by that row: the policy asks for a
        // human (the allow gate here would approve it, which is why this is
        // asserted on the policy decision itself).
        let other = ActKind::launch_path("/usr/bin/gimp");
        assert_eq!(
            g.policy()
                .evaluate(other.launch_target().unwrap(), &other, None)
                .unwrap(),
            GateDecision::Confirm(ConfirmClass::Routine)
        );
        // And a launch whose target is a risky class still needs the human,
        // allow-listed or not.
        // (Classification is keyword-based on the target, so an installer is
        // recognised by its name/path — `/usr/bin/dnf` is not one.)
        let installer = ActKind::launch_path("/opt/vendor/installer");
        assert_eq!(
            g.policy()
                .evaluate(installer.launch_target().unwrap(), &installer, None)
                .unwrap(),
            GateDecision::Confirm(ConfirmClass::Install)
        );
        // A hard-denied program is refused outright, whatever the allow-list.
        let term = AppPolicy::hard_deny("/usr/bin/gnome-terminal", None);
        assert!(term.is_some());
    }

    #[test]
    fn policy_updates_apply_to_the_live_guard() {
        // P57.8 — a Settings change must reach an already-attached engine, not
        // only the next process start.
        let g = DesktopGuard::new(
            AppPolicy {
                strict: true,
                ..AppPolicy::default()
            },
            Box::new(AllowGate),
            Box::new(NoopSink),
        );
        let before = g
            .preflight("gedit", &ActKind::Click { x: 1, y: 1 }, None)
            .unwrap();
        assert_eq!(before, GateDecision::Deny);
        let mut p = g.policy();
        p.allow_list.push("gedit".into());
        g.set_policy(p);
        let after = g
            .preflight("gedit", &ActKind::Click { x: 1, y: 1 }, None)
            .unwrap();
        assert_eq!(after, GateDecision::Allow);
        assert_eq!(g.policy().interaction_mode, InteractionMode::Background);
    }

    #[test]
    fn hard_deny_evaluates_to_error_not_confirm() {
        let g = gate_allow();
        let err = g
            .preflight(
                "Windows Terminal",
                &ActKind::Type {
                    text: "rm -rf /".into(),
                },
                None,
            )
            .unwrap_err();
        assert!(err.contains("hard-deny"));
    }

    // ---- the click-ladder gate ----------------------------------------

    /// The pointer-moving rung is **never** allowed by the policy alone, even
    /// on an allow-listed app with a routine act. An allow-list is permission to
    /// drive an app, not permission to hijack the operator's cursor.
    #[test]
    fn a_gated_rung_reaches_the_human_gate_even_on_an_allow_listed_app() {
        use crate::ladder::ClickRung;
        let p = AppPolicy::default().allow("notepad");
        let act = ActKind::Click { x: 10, y: 10 };
        // The ordinary act is allowed outright.
        assert_eq!(
            p.evaluate("notepad", &act, None).unwrap(),
            GateDecision::Allow
        );
        // The same act on the gated rung is a confirmation, always.
        assert_eq!(
            p.evaluate_rung("notepad", &act, None).unwrap(),
            GateDecision::Confirm(ConfirmClass::CursorTakeover)
        );
        // …and it is still only an Allow once the human gate says so.
        let allow = gate_allow();
        assert!(
            allow
                .authorize_rung(
                    "notepad",
                    &act,
                    None,
                    ActProvenance::Agent,
                    ClickRung::RawInput
                )
                .is_ok()
        );
        let confirm = DesktopGuard::new(
            AppPolicy::default().allow("notepad"),
            Box::new(ConfirmAllGate),
            Box::new(NoopSink),
        );
        let err = confirm
            .authorize_rung(
                "notepad",
                &act,
                None,
                ActProvenance::Agent,
                ClickRung::RawInput,
            )
            .unwrap_err();
        assert!(err.contains("human decision"), "got: {err}");
        assert!(err.contains("raw_input"), "got: {err}");
        // A deny-by-default host (the shipped Tauri gate today) refuses, so a
        // coordinate click never silently reaches the pointer-moving rung.
        let deny = DesktopGuard::new(
            AppPolicy::default().allow("notepad"),
            Box::new(DenyAllGate),
            Box::new(NoopSink),
        );
        let err = deny
            .authorize_rung(
                "notepad",
                &act,
                None,
                ActProvenance::Agent,
                ClickRung::RawInput,
            )
            .unwrap_err();
        assert!(err.contains("refused"), "got: {err}");
    }

    /// An ungated rung takes the ordinary path — no extra confirmation, so the
    /// ladder does not ask the human twice for a non-moving mechanism.
    #[test]
    fn an_ungated_rung_takes_the_ordinary_policy_path() {
        use crate::ladder::ClickRung;
        let mut p = AppPolicy::default().allow("notepad");
        p.allow_paths.clear();
        let act = ActKind::Click { x: 10, y: 10 };
        let decision = p.evaluate("notepad", &act, None).unwrap();
        assert_eq!(decision, GateDecision::Allow);
        for rung in [ClickRung::AccessibilityInvoke, ClickRung::SyntheticEvent] {
            assert_eq!(rung.gate(), crate::ladder::RungGate::Automatic);
        }
        // Even with a DenyAll gate, an ungated rung never consults it.
        let deny = DesktopGuard::new(p.clone(), Box::new(DenyAllGate), Box::new(NoopSink));
        assert!(
            deny.authorize_rung(
                "notepad",
                &act,
                None,
                ActProvenance::Agent,
                ClickRung::SyntheticEvent
            )
            .is_ok()
        );
    }

    /// The rung gate does not weaken any existing check: a hard-denied app, a
    /// safe zone, and the kill switch all still stop the gated rung first.
    #[test]
    fn the_rung_gate_never_relaxes_the_existing_checks() {
        use crate::ladder::ClickRung;
        let p = AppPolicy {
            safe_zones: vec![Region {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            }],
            ..AppPolicy::default()
        };
        // Safe zone, before any allow-list question.
        assert!(
            p.evaluate_rung("notepad", &ActKind::Click { x: 5, y: 5 }, None)
                .is_err()
        );
        // Hard-deny app.
        let err = p
            .evaluate_rung("Windows Terminal", &ActKind::Click { x: 500, y: 500 }, None)
            .unwrap_err();
        assert!(err.contains("hard-deny"), "got: {err}");
        // Kill switch.
        let g = gate_allow();
        g.kill.stop();
        let err = g
            .authorize_rung(
                "notepad",
                &ActKind::Click { x: 500, y: 500 },
                None,
                ActProvenance::Agent,
                ClickRung::RawInput,
            )
            .unwrap_err();
        assert!(err.contains("emergency stop"), "got: {err}");
    }

    /// The class is part of the same taxonomy and the same audit row, not a
    /// second authorization vocabulary.
    #[test]
    fn cursor_takeover_is_in_the_one_confirm_taxonomy() {
        assert_eq!(
            ConfirmClass::classify("Save", None),
            ConfirmClass::Routine,
            "classification is unchanged: a target name never produces the class"
        );
        assert!(!ALWAYS_CONFIRM.contains(&ConfirmClass::CursorTakeover));
        // It is a normal enum member: same serde, same gate, same audit.
        let json =
            serde_json::to_string(&GateDecision::Confirm(ConfirmClass::CursorTakeover)).unwrap();
        assert!(json.contains("CursorTakeover"), "{json}");
    }
}
