//! P62.5 / P69.G2 — shell-bias deflection.
//!
//! An external agent that reaches for a spreadsheet, the browser, or the
//! desktop through raw Python or a shell command is **refused** and pointed at
//! the shared façade instead. This module only classifies the command and
//! returns a structured verdict. It does not execute, and it does not
//! authorize — the caller (`agentcowork-core::ToolService::exec`, the one
//! pre-exec shell path) turns a `Some(..)` into a hard `action: "block"`
//! refusal, records it on the same Merkle audit chain as every other Guard-1
//! denial, and hands the model the façade tool to pivot onto.
//!
//! # What this check is
//!
//! A **token-aware substring scan** over a fixed needle list, run over the raw
//! command text after ASCII lower-casing and whitespace collapsing. A needle
//! fires only when it stands as its own token:
//!
//! * the byte **before** the match must not be an ASCII alphanumeric, `_`,
//!   `-`, `.`, or `/` — so `openpyxlish`, `my_python_docx_wrapper`,
//!   `python-pptx-tools`, and `report.pptx` do not fire on the inner needle;
//! * the byte **after** the match must not be an ASCII alphanumeric, `_`, or
//!   `-` — so `playwright-report` and `selenium_test.py` do not fire, while
//!   `openpyxl.Workbook` does (`.` is a boundary on the right, because
//!   attribute access is still use of the library).
//!
//! # What this check cannot see (read before trusting it)
//!
//! 1. **No AST.** `agentcowork-guard` has **no `tree-sitter` dependency**, and
//!    adding one is a separate, larger, separately-scoped change. This is a
//!    text scan, not the "Tree-Sitter AST Interception (SEC-4)" step of
//!    `ARCH/RECOVERY.md` §13 — that step stays *specified, not implemented*.
//!    Do not describe this module as AST inspection.
//! 2. **Dynamically-constructed names are invisible.** `import open"+"pyxl`,
//!    `__import__("open" + "pyxl")`, `getattr(mod, name)`, a base64/exec
//!    blob, or a shell-side name assembly (`m=open; python -c "import ${m}pyxl"`)
//!    all evade the scan, because the literal never appears in the command
//!    text. `dynamically_constructed_module_name_is_not_detected` in the test
//!    module below pins this limitation in code, not only in a document.
//! 3. **Bare `docx` is deliberately not a needle.** The import is
//!    `import docx`, which a needle for `docx` would also match inside every
//!    `report.docx` path and every `office.docx_open` tool id. The
//!    distribution name (`python-docx` / `python_docx`) is matched instead,
//!    so `pip install python-docx` is caught while `import docx` on its own is
//!    a known miss.
//! 4. **Only the shell string is scanned, not path or URL arguments.** A path
//!    that merely contains a needle (`ls playwright-report/`) is not a bypass,
//!    and scanning it would over-fire. Callers pass the command text only.
//! 5. **Over-inclusion remains possible in principle.** A command that names
//!    one of these libraries for a benign reason (a comment, a log line, a
//!    package name in an unrelated project) is still refused. That direction
//!    is deliberate: a refusal is recoverable — the agent is handed the façade
//!    to call — while a false negative is not.

use serde::{Deserialize, Serialize};

/// The audit row kind a refusal produced here is recorded under. This is the
/// same `guard.blocked` kind the audit crate documents for guard denials, so
/// the deflection lands on the existing trail instead of a second one.
pub const DEFLECTION_AUDIT_KIND: &str = "guard.blocked";

/// Which shared plane the command was trying to bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeflectionTarget {
    Office,
    Browser,
    Desktop,
}

impl DeflectionTarget {
    /// Stable lower-case name (the audit/`deflection.target` value).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Office => "office",
            Self::Browser => "browser",
            Self::Desktop => "desktop",
        }
    }

    /// The façade family the agent should call.
    pub fn facade(self) -> &'static str {
        match self {
            Self::Office => "office",
            Self::Browser => "browser",
            Self::Desktop => "computer_use",
        }
    }

    /// The concrete tool ids the agent may pivot onto, most specific first.
    pub fn suggested_tools(self) -> &'static [&'static str] {
        match self {
            Self::Office => &["office.edit", "office.calculate", "office.open"],
            Self::Browser => &["browser.operate", "browser.extract", "browser.research"],
            Self::Desktop => &["computer_use.see", "computer_use.act"],
        }
    }

    /// The single tool named in the refusal line the agent receives.
    pub fn suggested_tool(self) -> &'static str {
        self.suggested_tools()[0]
    }

    /// What the bypassed capability is, in the refusal prose.
    pub fn subject(self) -> &'static str {
        match self {
            Self::Office => "an office document",
            Self::Browser => "a browser",
            Self::Desktop => "the desktop",
        }
    }
}

/// A refusal the caller must honor, naming the capability to use instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeflectionNudge {
    /// The plane the command tried to bypass.
    pub target: DeflectionTarget,
    /// The needle that fired, verbatim from the (folded) command.
    pub matched: String,
    /// The façade family to call (`office` / `browser` / `computer_use`).
    pub facade: String,
    /// The concrete tool id to pivot onto.
    pub suggested_tool: String,
    /// The refusal line. Actionable on its own: it says what was refused and
    /// what to call instead, so the model can pivot without a human.
    pub message: String,
}

impl DeflectionNudge {
    /// The refusal line on its own, for a `reason` field or a log row.
    pub fn reason(&self) -> &str {
        &self.message
    }
}

/// Libraries and commands that mean "drive Office / the browser / the desktop
/// from a shell" rather than through the shared plane, with the plane each one
/// bypasses. Distribution names are matched alongside module names because the
/// two do not coincide (`python-pptx` installs a module called `pptx`).
const NEEDLES: &[(&str, DeflectionTarget)] = &[
    // Office.
    ("openpyxl", DeflectionTarget::Office),
    ("xlsxwriter", DeflectionTarget::Office),
    ("python-docx", DeflectionTarget::Office),
    ("python_docx", DeflectionTarget::Office),
    ("python-pptx", DeflectionTarget::Office),
    ("python_pptx", DeflectionTarget::Office),
    ("pptx", DeflectionTarget::Office),
    ("libreoffice --headless", DeflectionTarget::Office),
    ("soffice --headless", DeflectionTarget::Office),
    // Browser.
    ("puppeteer", DeflectionTarget::Browser),
    ("playwright", DeflectionTarget::Browser),
    ("selenium", DeflectionTarget::Browser),
    ("chromedriver", DeflectionTarget::Browser),
    ("geckodriver", DeflectionTarget::Browser),
    // Desktop.
    ("pyautogui", DeflectionTarget::Desktop),
    ("xdotool", DeflectionTarget::Desktop),
    ("sendinput", DeflectionTarget::Desktop),
    ("sendkeys", DeflectionTarget::Desktop),
    ("cliclick", DeflectionTarget::Desktop),
];

/// Inspect a shell command. `Some(..)` is a **refusal**: the caller must not
/// execute the command and must hand the model [`DeflectionNudge::message`].
/// `None` means the command is not one of the bypass shapes this gate knows
/// about — which is not the same as "clean", see the module limits above.
pub fn deflect_shell_bias(command: &str) -> Option<DeflectionNudge> {
    let folded = fold(command);
    NEEDLES
        .iter()
        .find(|(needle, _)| contains_token(&folded, needle))
        .map(|(needle, target)| nudge(*target, needle))
}

/// Lower-case ASCII + collapse whitespace runs to a single space, so a
/// multi-word needle such as `libreoffice --headless` still fires through a
/// double space, a tab, or a newline between the flags.
fn fold(command: &str) -> String {
    let lowered = command.to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_space = false;
    for ch in lowered.chars() {
        if ch.is_ascii_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// Bytes that may not *precede* a match: the start of a dotted or slashed
/// path, or the middle of a hyphen/underscore-joined identifier.
fn blocks_left(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/')
}

/// Bytes that may not *follow* a match. `.` is deliberately absent so
/// attribute access (`openpyxl.Workbook`) still fires.
fn blocks_right(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

/// Does `hay` contain `needle` as a whole token? Overlapping candidates are
/// re-checked so a rejected occurrence does not hide a later accepted one.
fn contains_token(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = hay.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = hay.get(from..).and_then(|s| s.find(needle)) {
        let start = from + rel;
        let end = start + needle.len();
        let left_ok = start == 0 || !blocks_left(bytes[start - 1]);
        let right_ok = end == bytes.len() || !blocks_right(bytes[end]);
        if left_ok && right_ok {
            return true;
        }
        from = next_char_boundary(hay, start + 1);
    }
    false
}

fn next_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn nudge(target: DeflectionTarget, matched: &str) -> DeflectionNudge {
    let tool = target.suggested_tool();
    DeflectionNudge {
        message: format!(
            "deflection_nudge: Guard-1 refused `{matched}` — it drives {} from a shell, outside \
             the shared plane. Call the `{facade}.*` façade instead (`{tool}`); Guard authorizes \
             that path and the command cannot be run.",
            target.subject(),
            facade = target.facade(),
        ),
        target,
        matched: matched.to_string(),
        facade: target.facade().to_string(),
        suggested_tool: tool.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_python_is_refused_with_the_office_facade() {
        let n = deflect_shell_bias("python -c 'import openpyxl'").unwrap();
        assert_eq!(n.target, DeflectionTarget::Office);
        assert_eq!(n.target.facade(), "office");
        assert_eq!(n.facade, "office");
        assert_eq!(n.suggested_tool, "office.edit");
        assert_eq!(n.matched, "openpyxl");
        assert!(n.message.contains("deflection_nudge"));
        assert!(n.message.contains("office.edit"), "{}", n.message);
    }

    #[test]
    fn browser_and_desktop_commands_pick_their_facade() {
        let b = deflect_shell_bias("npx playwright test").unwrap();
        assert_eq!(b.target, DeflectionTarget::Browser);
        assert_eq!(b.suggested_tool, "browser.operate");
        let d = deflect_shell_bias("xdotool click 1").unwrap();
        assert_eq!(d.target, DeflectionTarget::Desktop);
        assert_eq!(d.facade, "computer_use");
        assert_eq!(d.suggested_tool, "computer_use.see");
    }

    #[test]
    fn every_needle_in_the_table_fires_on_its_own_bare_form() {
        for (needle, target) in NEEDLES {
            let cmd = format!("run {needle} now");
            let n = deflect_shell_bias(&cmd)
                .unwrap_or_else(|| panic!("needle `{needle}` did not fire on `{cmd}`"));
            assert_eq!(&n.target, target, "needle `{needle}`");
            assert_eq!(&n.matched, needle);
        }
    }

    #[test]
    fn multi_word_needles_survive_whitespace_collapsing() {
        for cmd in [
            "libreoffice --headless --convert-to pdf x.docx",
            "libreoffice  --headless   x.docx",
            "soffice --headless --convert-to pdf x.docx",
            "soffice\t--headless x.docx",
        ] {
            assert_eq!(
                deflect_shell_bias(cmd).map(|n| n.target),
                Some(DeflectionTarget::Office),
                "{cmd}"
            );
        }
    }

    #[test]
    fn a_near_miss_identifier_does_not_fire() {
        for cmd in [
            // Suffix inside a longer identifier.
            "python -c 'import openpyxlish'",
            "echo selenite",
            "cat xlsxwriter_plus/notes.md",
            "grep -r playwrightwright .",
            // Hyphen/underscore-joined neighbours.
            "cat my_python_docx_wrapper.py",
            "ls playwright-report/index.html",
            "ls selenium_test.py",
            "cat xdotool_notes.md",
            // A path segment or a file extension, not a library use.
            "open report.pptx",
            "cat deck.pptx",
            "ls ./vendor/openpyxl-compat/README.md",
        ] {
            assert!(
                deflect_shell_bias(cmd).is_none(),
                "must not fire (false positive): {cmd}"
            );
        }
    }

    /// **Documented over-inclusion (P69.G2).** A command that names one of the
    /// libraries as a standalone token is refused even when the intent is
    /// innocent. This is the direction the check is deliberately biased
    /// towards: a refusal is recoverable (the model is handed the façade),
    /// a false negative is not. Pinned here so the trade-off stays visible
    /// rather than being discovered as a surprise.
    #[test]
    fn a_benign_command_naming_a_library_as_a_token_is_still_refused() {
        for cmd in [
            "grep -rn openpyxl .",
            "echo 'do not use pyautogui here'",
            "grep playwright package-lock.json",
        ] {
            let n = deflect_shell_bias(cmd)
                .unwrap_or_else(|| panic!("expected the documented over-fire: {cmd}"));
            assert!(n.message.contains("Guard-1 refused"), "{}", n.message);
        }
    }

    #[test]
    fn a_clean_command_passes_untouched() {
        for cmd in [
            "cargo test -p agentcowork-guard",
            "python -c 'print(1)'",
            "ls -la",
            "cat notes.md",
            "git status",
            "curl -sS https://example.test/data.json",
            "npm run build",
            "node -e 'console.log(process.version)'",
        ] {
            assert!(deflect_shell_bias(cmd).is_none(), "must pass: {cmd}");
        }
    }

    #[test]
    fn attribute_access_and_scoped_packages_still_fire() {
        // `.` is a right boundary, so a real use inside an expression fires.
        assert!(deflect_shell_bias("python -c 'openpyxl.Workbook()'").is_some());
        // `@` is not a left blocker, so the npm scope form fires.
        assert!(
            deflect_shell_bias("npx @playwright/test --version").map(|n| n.target)
                == Some(DeflectionTarget::Browser)
        );
    }

    /// **Documented blind spot (P69.G2).** This check sees command text, not
    /// program semantics, so a name assembled at runtime is invisible. If this
    /// test ever starts failing, the matcher gained a capability; if it passes,
    /// the limitation documented in the module header is real. The upgrade
    /// path is an AST pass over the command (ARCH/RECOVERY.md §13 step 1),
    /// which is explicitly still out of scope.
    #[test]
    fn dynamically_constructed_module_name_is_not_detected() {
        for cmd in [
            "python -c 'import open\" + \"pyxl'",
            "python -c '__import__(\"open\" + \"pyxl\")'",
            "python -c 'm=\"open\"+\"pyxl\"; __import__(m)'",
            "python -c 'exec(__import__(\"base64\").b64decode(\"aW1wb3J0IG9wZW5weGwi\"))'",
            "m=open; python -c \"import ${m}pyxl\"",
            "python -c 'getattr(__import__(\"open\"+\"pyxl\"), \"Workbook\")'",
        ] {
            assert!(
                deflect_shell_bias(cmd).is_none(),
                "known blind spot — if this fires, say so in the module limits: {cmd}"
            );
        }
    }

    #[test]
    fn the_deflection_audit_kind_is_the_guard_denial_kind() {
        assert_eq!(DEFLECTION_AUDIT_KIND, "guard.blocked");
    }

    #[test]
    fn the_nudge_serializes_with_its_pivot_fields() {
        let n = deflect_shell_bias("python -c 'import pyautogui'").unwrap();
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["target"], "desktop");
        assert_eq!(v["facade"], "computer_use");
        assert_eq!(v["suggested_tool"], "computer_use.see");
        assert_eq!(v["matched"], "pyautogui");
        let back: DeflectionNudge = serde_json::from_value(v).unwrap();
        assert_eq!(back, n);
    }
}
