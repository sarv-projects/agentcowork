//! H36 / P67 — terminal shell integration (VS Code's OSC 633 protocol).
//!
//! VS Code can tell *what* a terminal is doing because it cooperates with the
//! shell: an injected script emits private OSC sequences at prompt boundaries,
//! and the terminal parses them out of the byte stream. That is what makes
//! working-directory detection, exit-code decorations, command navigation,
//! "run recent command" and — crucially for us — **chat context** possible.
//!
//! Without it, a terminal is a picture of text. With it, it is a structured
//! log of `(command, cwd, exit, output)` records that the agent can read.
//!
//! ## Protocol (the parts we implement)
//!
//! All sequences are `ESC ] 633 ; <sub> [ ; <arg> ]* ST`, where `ST` is
//! `ESC \` or `BEL`. Plus the standard `ESC ] 7 ; file://<host><path> ST`.
//!
//! | Sub | Meaning | Payload |
//! |-----|---------|---------|
//! | `A` | prompt start | — |
//! | `B` | prompt end / command line being read | — |
//! | `C` | command executed (pre-exec) | — |
//! | `D` | command finished | optional exit code |
//! | `E` | explicit command line | command line, then nonce |
//! | `P` | property | `Name=Value` (`Cwd` is the interesting one) |
//! | `` (empty) | trust marker | the nonce |
//!
//! ## Why the nonce matters (and is not optional here)
//!
//! Command boundaries are *authority* in our design: the tracker tells the
//! agent "this is what ran and what it printed". A program printing
//! `\x1b]633;E;rm -rf /;...` could otherwise forge a command line into the
//! agent's context. So when a nonce is supplied at spawn, nothing is
//! **armed** until the shell has echoed that nonce back (a `633;;<nonce>`
//! marker or a matching-nonce `E`). Untrusted sequences are consumed but
//! never become events. This is the same defense VS Code shipped after the
//! 2023 shell-integration RCE report.
//!
//! ## What this module deliberately does NOT do
//!
//! It does not interpret VT output — that stays xterm.js's job. `feed()`
//! returns the bytes to render *minus* the integration sequences, so the
//! visible terminal is identical whether or not integration is active.

use std::collections::VecDeque;

/// `ESC ] 633 ;`
const OSC_633: &[u8] = b"\x1b]633;";
/// `ESC ] 7 ;` (cwd, the older de-facto standard)
const OSC_7: &[u8] = b"\x1b]7;";

/// Cap on retained commands per PTY session.
pub const MAX_TRACKED_COMMANDS: usize = 64;
/// Cap on retained output per command (chat context, not a log file).
pub const MAX_TRACKED_OUTPUT: usize = 8_192;

/// Environment variable carrying the per-session nonce into the shell.
/// Newly-spawned shells get the new spelling only; the generated scripts
/// below still accept the retired spelling as a read-only fallback so hooks
/// installed by a previous release keep working (DEC-053).
pub const NONCE_ENV: &str = "AGENTCOWORK_NONCE";
/// Environment variable that marks a shell as AgentCowork-integrated (the
/// scripts use it to stay inert under a plain terminal / other hosts).
/// Same new-only-writes, legacy-fallback-reads rule as [`NONCE_ENV`].
pub const INJECTION_ENV: &str = "AGENTCOWORK_INJECTION";
/// Retired spellings of [`NONCE_ENV`] / [`INJECTION_ENV`]: never written,
/// still honored by the generated scripts.
pub const LEGACY_NONCE_ENV: &str = "EVERYAIOS_NONCE";
/// Retired spelling of [`INJECTION_ENV`]: never written, still honored.
pub const LEGACY_INJECTION_ENV: &str = "EVERYAIOS_INJECTION";

/// A structured fact the shell reported about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationEvent {
    /// `633;A` — a prompt is about to be drawn.
    PromptStart,
    /// `633;B` — the prompt finished; the shell is reading a command line.
    PromptEnd,
    /// `633;C` — the command line was accepted and is about to execute.
    CommandExecuted,
    /// `633;D[;<exit>]` — the command finished.
    CommandFinished(Option<i32>),
    /// `633;E;<command>[;<nonce>]` — the command line itself.
    CommandLine { text: String, trusted: bool },
    /// Working directory (`633;P;Cwd=<path>` or OSC 7).
    Cwd(String),
    /// Any other `633;P;<name>=<value>` property (`IsWindows`, …).
    Property { name: String, value: String },
    /// `633;;<nonce>` — the trust marker. Arms the parser.
    Trusted,
}

/// One ordered piece of a fed chunk. Ordering matters: a single `read()` can
/// contain `633;C`, the command's first output line, and `633;D` together, so
/// the host must be able to attribute bytes to the command that was open at
/// the moment they arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Bytes to render.
    Bytes(Vec<u8>),
    /// A fact the shell reported.
    Event(IntegrationEvent),
}

/// Strips integration sequences out of a PTY byte stream and reports the
/// facts they carried. Split-safe: an OSC sequence may arrive across two
/// `read()` chunks.
pub struct IntegrationParser {
    enabled: bool,
    nonce: Option<String>,
    /// An incomplete OSC sequence held back from the previous chunk.
    carry: Vec<u8>,
    /// Armed only once the shell has proven it holds the nonce. With no nonce
    /// configured we are in degraded (but explicit) single-user mode.
    armed: bool,
}

impl IntegrationParser {
    pub fn new(enabled: bool, nonce: Option<String>) -> Self {
        let armed = nonce.is_none();
        Self {
            enabled,
            nonce,
            carry: Vec::new(),
            armed,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Has the shell proven it holds the nonce? Until this is true the
    /// tracker records nothing, no matter what the stream claims.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Feed one PTY chunk, preserving byte/event ordering.
    pub fn feed_segments(&mut self, chunk: &[u8]) -> Vec<Segment> {
        if !self.enabled {
            return vec![Segment::Bytes(chunk.to_vec())];
        }
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);

        let mut segs: Vec<Segment> = Vec::new();
        let mut plain: Vec<u8> = Vec::new();
        let mut i = 0usize;

        while i < buf.len() {
            if buf[i] == 0x1b {
                let rest = &buf[i..];
                if rest.starts_with(OSC_633) || rest.starts_with(OSC_7) {
                    match osc_end(rest) {
                        Some(end) => {
                            // Body excludes the leading `ESC ]` and terminator.
                            let body = &rest[2..end.body_end];
                            if let Some(ev) = self.interpret(body) {
                                match &ev {
                                    IntegrationEvent::Trusted => self.armed = true,
                                    IntegrationEvent::CommandLine { trusted, .. } if *trusted => {
                                        self.armed = true;
                                    }
                                    _ => {}
                                }
                                if self.armed {
                                    if !plain.is_empty() {
                                        segs.push(Segment::Bytes(std::mem::take(&mut plain)));
                                    }
                                    segs.push(Segment::Event(ev));
                                }
                            }
                            i += end.total_len;
                            continue;
                        }
                        None => {
                            // Incomplete — hold it for the next chunk.
                            self.carry = rest.to_vec();
                            if !plain.is_empty() {
                                segs.push(Segment::Bytes(plain));
                            }
                            return segs;
                        }
                    }
                }
            }
            plain.push(buf[i]);
            i += 1;
        }
        if !plain.is_empty() {
            segs.push(Segment::Bytes(plain));
        }
        segs
    }

    /// Convenience view: concatenated render bytes + every event. Loses
    /// ordering, so streaming hosts must use [`IntegrationParser::feed_segments`].
    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, Vec<IntegrationEvent>) {
        let mut bytes = Vec::with_capacity(chunk.len());
        let mut events = Vec::new();
        for seg in self.feed_segments(chunk) {
            match seg {
                Segment::Bytes(b) => bytes.extend_from_slice(&b),
                Segment::Event(e) => events.push(e),
            }
        }
        (bytes, events)
    }

    /// Any held-back partial sequence, handed back at teardown so a truncated
    /// marker is not silently swallowed.
    pub fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }

    fn interpret(&self, body: &[u8]) -> Option<IntegrationEvent> {
        let body = std::str::from_utf8(body).ok()?;
        if let Some(path) = body.strip_prefix("7;") {
            return Some(IntegrationEvent::Cwd(file_uri_path(path)?));
        }
        let rest = body.strip_prefix("633;")?;
        let mut parts = rest.split(';');
        let sub = parts.next().unwrap_or("");
        let args: Vec<&str> = parts.collect();
        match sub {
            "" => {
                let nonce = args.first().copied().unwrap_or("");
                if self.nonce.as_deref() == Some(nonce) {
                    Some(IntegrationEvent::Trusted)
                } else {
                    None
                }
            }
            "A" => Some(IntegrationEvent::PromptStart),
            "B" => Some(IntegrationEvent::PromptEnd),
            "C" => Some(IntegrationEvent::CommandExecuted),
            "D" => {
                let exit = args.first().and_then(|v| v.trim().parse::<i32>().ok());
                Some(IntegrationEvent::CommandFinished(exit))
            }
            "E" => {
                // `E;<command>[;<nonce>]`. The command line may itself contain
                // `;`, so only peel a trailing token when it is *our* nonce.
                let (text, trusted) = match (self.nonce.as_deref(), args.last()) {
                    (Some(expected), Some(last)) if *last == expected && args.len() >= 2 => {
                        (args[..args.len() - 1].join(";"), true)
                    }
                    _ => (args.join(";"), false),
                };
                Some(IntegrationEvent::CommandLine { text, trusted })
            }
            "P" => {
                let joined = args.join(";");
                let (name, value) = joined.split_once('=')?;
                if name == "Cwd" {
                    Some(IntegrationEvent::Cwd(value.to_string()))
                } else {
                    Some(IntegrationEvent::Property {
                        name: name.to_string(),
                        value: value.to_string(),
                    })
                }
            }
            _ => None,
        }
    }
}

/// `file://<host>/path` → `/path` (Windows paths arrive as `/C:/…`).
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let path = &rest[slash..];
    // `/C:/Users/x` is the URI form of `C:\Users\x`.
    let bytes = path.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
        return Some(path[1..].to_string());
    }
    Some(path.to_string())
}

struct OscEnd {
    /// Index of the first terminator byte, relative to the sequence start.
    body_end: usize,
    /// Whole sequence length including the terminator.
    total_len: usize,
}

/// Find the end of an OSC sequence starting at `seq[0] == ESC`.
fn osc_end(seq: &[u8]) -> Option<OscEnd> {
    let mut i = 2; // skip ESC ]
    while i < seq.len() {
        match seq[i] {
            0x07 => {
                return Some(OscEnd {
                    body_end: i,
                    total_len: i + 1,
                });
            }
            0x1b if seq.get(i + 1) == Some(&b'\\') => {
                return Some(OscEnd {
                    body_end: i,
                    total_len: i + 2,
                });
            }
            0x1b => return None, // malformed — never treat as complete
            _ => i += 1,
        }
    }
    None
}

/// One completed command, as reconstructed from integration events.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandRecord {
    /// The command line, exactly as the shell reported it.
    pub command: String,
    /// Working directory the command ran in.
    pub cwd: String,
    /// Exit code when the shell reported one.
    pub exit_code: Option<i32>,
    /// ANSI-stripped output (capped — this feeds chat context, not a log).
    pub output: String,
    /// `false` when the command line could not be attributed to a
    /// nonce-bearing sequence (never shown to the agent as fact).
    pub trusted: bool,
}

impl CommandRecord {
    /// Did the command fail, as far as we know?
    pub fn failed(&self) -> bool {
        matches!(self.exit_code, Some(c) if c != 0)
    }
}

/// Turns an event stream into an ordered list of [`CommandRecord`]s, and keeps
/// the shell's current working directory.
///
/// Boundary table (`A` always begins a new slot, `D` closes one):
///
/// ```text
/// A ──▶ B ──▶ (E: command line) ──▶ C ──▶ [output bytes] ──▶ D;exit ──▶ A …
/// ```
#[derive(Debug, Default)]
pub struct CommandTracker {
    cwd: String,
    pending: Option<CommandRecord>,
    done: VecDeque<CommandRecord>,
    /// True between `C` and `D` — the window in which output belongs to the
    /// pending command.
    capturing: bool,
    max_commands: usize,
    max_output: usize,
}

impl CommandTracker {
    pub fn new() -> Self {
        Self {
            cwd: String::new(),
            pending: None,
            done: VecDeque::new(),
            capturing: false,
            max_commands: MAX_TRACKED_COMMANDS,
            max_output: MAX_TRACKED_OUTPUT,
        }
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn last(&self) -> Option<&CommandRecord> {
        self.done.back()
    }

    /// Most recent `n` commands, oldest first.
    pub fn recent(&self, n: usize) -> Vec<&CommandRecord> {
        let skip = self.done.len().saturating_sub(n);
        self.done.iter().skip(skip).collect()
    }

    pub fn len(&self) -> usize {
        self.done.len()
    }

    pub fn is_empty(&self) -> bool {
        self.done.is_empty()
    }

    pub fn apply(&mut self, ev: &IntegrationEvent) {
        match ev {
            IntegrationEvent::PromptStart => {
                // A new prompt: whatever was pending never closed cleanly
                // (Ctrl+C, a killed shell). Keep it — its output is real —
                // but mark the exit code unknown.
                self.close_pending();
            }
            IntegrationEvent::PromptEnd => {
                self.capturing = false;
            }
            IntegrationEvent::CommandExecuted => {
                if self.pending.is_none() {
                    self.pending = Some(CommandRecord {
                        cwd: self.cwd.clone(),
                        trusted: true,
                        ..Default::default()
                    });
                }
                self.capturing = true;
            }
            IntegrationEvent::CommandLine { text, trusted } => {
                let slot = self.pending.get_or_insert_with(CommandRecord::default);
                slot.command = text.clone();
                slot.trusted = *trusted;
                if slot.cwd.is_empty() {
                    slot.cwd = self.cwd.clone();
                }
            }
            IntegrationEvent::CommandFinished(code) => {
                if let Some(slot) = self.pending.as_mut() {
                    slot.exit_code = *code;
                }
                self.capturing = false;
                self.close_pending();
            }
            IntegrationEvent::Cwd(path) => {
                self.cwd = path.clone();
            }
            IntegrationEvent::Property { .. } | IntegrationEvent::Trusted => {}
        }
    }

    /// Raw render bytes observed since `C` — the pending command's output.
    pub fn push_output(&mut self, bytes: &[u8]) {
        if !self.capturing {
            return;
        }
        let Some(slot) = self.pending.as_mut() else {
            return;
        };
        if slot.output.len() >= self.max_output {
            return;
        }
        let text = strip_ansi(bytes);
        if text.is_empty() {
            return;
        }
        let room = self.max_output - slot.output.len();
        if text.len() <= room {
            slot.output.push_str(&text);
        } else {
            // Truncate on a char boundary.
            let mut end = room;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            slot.output.push_str(&text[..end]);
            slot.output.push_str("\n… [output truncated]");
        }
    }

    fn close_pending(&mut self) {
        if let Some(mut rec) = self.pending.take() {
            // A slot with no command line is an artefact of prompt redraws.
            if !rec.command.is_empty() {
                rec.output = rec.output.trim_end().to_string();
                self.done.push_back(rec);
                while self.done.len() > self.max_commands {
                    self.done.pop_front();
                }
            }
        }
    }

    /// The `#terminalLastCommand` context block (Copilot parity): the last
    /// command, where it ran, how it ended, and what it printed.
    pub fn context_block(&self, max_chars: usize) -> Option<String> {
        let rec = self.last()?;
        if !rec.trusted || rec.command.trim().is_empty() {
            return None;
        }
        let mut s = String::new();
        s.push_str(&format!("$ {}\n", rec.command.trim()));
        if !rec.cwd.is_empty() {
            s.push_str(&format!("cwd: {}\n", rec.cwd));
        }
        if let Some(code) = rec.exit_code {
            s.push_str(&format!("exit: {code}\n"));
        }
        if !rec.output.is_empty() {
            s.push_str("output:\n");
            s.push_str(rec.output.trim_end());
            s.push('\n');
        }
        if s.len() > max_chars {
            let mut end = max_chars;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push_str("\n… [truncated]");
        }
        Some(s)
    }

    /// Every tracked command as a compact block (the "terminal history as
    /// context" case, e.g. "why did my last three commands fail?").
    pub fn history_block(&self, max_commands: usize, max_chars: usize) -> Option<String> {
        let recent = self.recent(max_commands);
        if recent.is_empty() {
            return None;
        }
        let mut s = String::new();
        for rec in recent {
            if !rec.trusted || rec.command.trim().is_empty() {
                continue;
            }
            let status = match rec.exit_code {
                Some(0) => "ok".to_string(),
                Some(c) => format!("exit {c}"),
                None => "unknown".to_string(),
            };
            s.push_str(&format!("$ {}   [{status}]\n", rec.command.trim()));
        }
        if s.is_empty() {
            return None;
        }
        if s.len() > max_chars {
            let mut end = max_chars;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push_str("… [truncated]");
        }
        Some(s)
    }
}

/// Remove non-printing control sequences from terminal output, so it can be
/// handed to a model as text. This is a text extractor, not a VT emulator:
/// it drops SGR/OSC/CSI, honours backspace erasure at a line level, and
/// normalises CRLF.
pub fn strip_ansi(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < input.len() {
        let b = input[i];
        if b == 0x1b {
            i += 1;
            match input.get(i) {
                Some(b'[') => {
                    // CSI: parameters until a final byte in @..~
                    i += 1;
                    while i < input.len() && !(0x40..=0x7e).contains(&input[i]) {
                        i += 1;
                    }
                    if i < input.len() {
                        i += 1; // consume the final byte
                    }
                }
                Some(b']') => {
                    // OSC: until BEL or ST
                    i += 1;
                    while i < input.len() {
                        if input[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if input[i] == 0x1b && input.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                Some(b'(') | Some(b')') | Some(b'*') | Some(b'+') => {
                    i += 2; // charset designation (ESC ( B)
                }
                Some(_) => i += 1,
                None => {}
            }
            continue;
        }
        if b == b'\r' {
            // CR without LF returns the cursor: drop the produced prefix.
            if input.get(i + 1) == Some(&b'\n') {
                out.push('\n');
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if b == 0x08 {
            // Backspace: erase the previous character where we can.
            out.pop();
            i += 1;
            continue;
        }
        if b == b'\t' || b == b'\n' || b >= 0x20 {
            // Multi-byte UTF-8 passes through byte-wise; it is already valid.
            out.push(b as char);
            i += 1;
            continue;
        }
        i += 1; // other control char
    }
    out
}

// ---------------------------------------------------------------------------
// Integration scripts — one per supported shell
// ---------------------------------------------------------------------------

/// The shells we can integrate, matching VS Code's supported set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationShell {
    Bash,
    Zsh,
    Fish,
    Pwsh,
}

impl IntegrationShell {
    /// Pick the script for a profile, from its executable name. `None` = no
    /// script (the terminal still works; it just has no structured facts).
    pub fn for_path(path: &str) -> Option<Self> {
        let name = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        // `sh`/`dash` are deliberately absent: `--init-file` is a bash-ism, and
        // injecting it into dash would abort the shell instead of integrating
        // it. An unintegrated terminal still works — it just has no structured
        // facts, which is the honest outcome.
        match name {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "fish" => Some(Self::Fish),
            "pwsh" | "powershell" => Some(Self::Pwsh),
            _ => None,
        }
    }

    pub fn script(self) -> &'static str {
        match self {
            Self::Bash => BASH_SCRIPT,
            Self::Zsh => ZSH_SCRIPT,
            Self::Fish => FISH_SCRIPT,
            Self::Pwsh => PWSH_SCRIPT,
        }
    }

    /// VS Code-compatible integration quality. `Rich` = command detection with
    /// exit status; `Basic` = boundaries but no exit status. We only ship
    /// `Rich` scripts, so an unsupported shell reports `None`.
    pub fn quality(self) -> &'static str {
        "Rich"
    }

    /// File name the script is written to in the per-session temp dir.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Bash => "integration.bash",
            Self::Zsh => "integration.zsh",
            Self::Fish => "integration.fish",
            Self::Pwsh => "integration.ps1",
        }
    }
}

/// bash: `C`+`E` from `PS0`, `D`+`A` from `PROMPT_COMMAND`.
///
/// `PS0` is expanded *after* the command line is read and *before* it runs —
/// bash's only hook at exactly that point. The obvious alternative, a `DEBUG`
/// trap, is wrong here: it also fires for commands executed *by* other
/// `PROMPT_COMMAND` hooks, so with a zoxide/direnv-style hook installed it
/// reports `__zoxide_hook` as "the command the user ran". Measured both ways
/// against a hostile rc that installs such a hook; `PS0` reported the real
/// command line and the trap did not.
///
/// The command line itself comes from `history 1`, because `BASH_COMMAND` is
/// only the first element of a pipeline and cannot represent `a | b > c`.
pub const BASH_SCRIPT: &str = r#"
# AgentCowork shell integration — generated per session. Do not edit.
if [ -n "${AGENTCOWORK_INJECTION:-${EVERYAIOS_INJECTION:-}}" ] && [ -z "${__eaios_hooked:-}" ]; then
  __eaios_hooked=1

  __eaios_osc() { printf '\033]633;%s\033\\' "$1"; }

  # `history 1` with its leading index and padding stripped.
  __eaios_hist_line() {
    local __eaios_l
    __eaios_l=$(HISTTIMEFORMAT= builtin history 1 2>/dev/null)
    printf '%s' "${__eaios_l#"${__eaios_l%%[![:space:]]*}"}"
  }

  __eaios_ps0() {
    local __eaios_line __eaios_num
    __eaios_line=$(__eaios_hist_line)
    __eaios_num="${__eaios_line%%[[:space:]]*}"
    __eaios_line="${__eaios_line#"$__eaios_num"}"
    __eaios_line="${__eaios_line#"${__eaios_line%%[![:space:]]*}"}"   # ltrim
    __eaios_osc "C"
    __eaios_osc "E;${__eaios_line};${AGENTCOWORK_NONCE:-${EVERYAIOS_NONCE:-}}"
  }

  # The exit status of the command that just finished is only observable at
  # the top of the next prompt.
  __eaios_prompt() {
    local __eaios_status=$?
    __eaios_osc "D;${__eaios_status}"
    __eaios_osc "A"
    __eaios_osc "P;Cwd=${PWD}"
  }

  # A user-supplied PS0 is appended, not replaced: PS0 is re-expanded on every
  # prompt, so keeping the original text keeps its behaviour.
  PS0='$(__eaios_ps0)'"${PS0:-}"
  case ";${PROMPT_COMMAND:-};" in
    *";__eaios_prompt;"*) ;;
    *) PROMPT_COMMAND="__eaios_prompt${PROMPT_COMMAND:+;${PROMPT_COMMAND}}" ;;
  esac

  # Arm the host parser before anything else can speak for this shell.
  __eaios_osc ";${AGENTCOWORK_NONCE:-${EVERYAIOS_NONCE:-}}"
  __eaios_osc "A"
  __eaios_osc "P;Cwd=${PWD}"
fi
"#;

/// zsh has real `precmd`/`preexec` hooks — much cleaner than bash's trap.
pub const ZSH_SCRIPT: &str = r#"
# AgentCowork shell integration — generated per session. Do not edit.
if [[ -n "${AGENTCOWORK_INJECTION:-${EVERYAIOS_INJECTION:-}}" ]]; then
  __eaios_osc() { printf '\033]633;%s\033\\' "$1"; }

  __eaios_preexec() {
    __eaios_osc "C"
    __eaios_osc "E;${1};${AGENTCOWORK_NONCE:-${EVERYAIOS_NONCE:-}}"
    __eaios_osc "P;Cwd=${PWD}"
  }

  __eaios_precmd() {
    local __eaios_status=$?
    __eaios_osc "D;${__eaios_status}"
    __eaios_osc "A"
  }

  autoload -Uz add-zsh-hook
  add-zsh-hook preexec __eaios_preexec
  add-zsh-hook precmd __eaios_precmd
  __eaios_osc ";${AGENTCOWORK_NONCE:-${EVERYAIOS_NONCE:-}}"
  __eaios_osc "A"
  __eaios_osc "P;Cwd=${PWD}"
fi
"#;

/// fish: `fish_preexec` / `fish_prompt` events carry the command line.
pub const FISH_SCRIPT: &str = r#"
# AgentCowork shell integration — generated per session. Do not edit.
if test -n "$AGENTCOWORK_INJECTION" -o -n "$EVERYAIOS_INJECTION"
    if set -q AGENTCOWORK_NONCE; set -l __eaios_nonce $AGENTCOWORK_NONCE; else; set -l __eaios_nonce $EVERYAIOS_NONCE; end
    function __eaios_osc
        printf '\033]633;%s\033\\' $argv[1]
    end

    function __eaios_preexec --on-event fish_preexec
        __eaios_osc "C"
        __eaios_osc "E;$argv[1];$__eaios_nonce"
        __eaios_osc "P;Cwd=$PWD"
    end

    function __eaios_prompt --on-event fish_prompt
        __eaios_osc "D;$status"
        __eaios_osc "A"
    end

    __eaios_osc ";$__eaios_nonce"
    __eaios_osc "A"
    __eaios_osc "P;Cwd=$PWD"
end
"#;

/// pwsh: `PSConsoleHostReadLine` gives us the typed line (preexec), and
/// wrapping `prompt` gives us the exit status of the command that just ran.
pub const PWSH_SCRIPT: &str = r#"
# AgentCowork shell integration — generated per session. Do not edit.
if ($env:AGENTCOWORK_INJECTION -or $env:EVERYAIOS_INJECTION) {
  $__eaios_nonce = if ($env:AGENTCOWORK_NONCE) { $env:AGENTCOWORK_NONCE } else { $env:EVERYAIOS_NONCE }
  function global:__EaiosOsc([string]$Payload) {
    [Console]::Out.Write("$([char]27)]633;$Payload$([char]27)\")
  }

  $global:__EaiosLastHistoryCount = -1
  $global:__EaiosPromptWrapped = $false

  $global:__EaiosReadLine = $function:PSConsoleHostReadLine
  function global:PSConsoleHostReadLine {
    $line = & $global:__EaiosReadLine
    if ($line -ne $null -and "$line".Trim().Length -gt 0) {
      __EaiosOsc "C"
      __EaiosOsc "E;$line;$__eaios_nonce"
      __EaiosOsc "P;Cwd=$($PWD.ProviderPath ?? $PWD.Path)"
    }
    return $line
  }

  $global:__EaiosPrompt = $function:prompt
  function global:prompt {
    $status = $global:?
    $code = if ($status) { 0 } else { 1 }
    if ($global:__EaiosPrompt) { $base = & $global:__EaiosPrompt } else { $base = "PS $($PWD.Path)> " }
    __EaiosOsc "D;$code"
    __EaiosOsc "A"
    __EaiosOsc "P;Cwd=$($PWD.ProviderPath ?? $PWD.Path)"
    return $base
  }

  __EaiosOsc ";$__eaios_nonce"
  __EaiosOsc "A"
  __EaiosOsc "P;Cwd=$($PWD.ProviderPath ?? $PWD.Path)"
}
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(nonce: &str) -> IntegrationParser {
        IntegrationParser::new(true, Some(nonce.to_string()))
    }

    fn seq(payload: &str) -> Vec<u8> {
        format!("\x1b]633;{payload}\x1b\\").into_bytes()
    }

    #[test]
    fn disabled_parser_is_a_pass_through() {
        let mut p = IntegrationParser::new(false, None);
        let (out, ev) = p.feed(b"hello \x1b]633;A\x1b\\ world");
        assert_eq!(out, b"hello \x1b]633;A\x1b\\ world");
        assert!(ev.is_empty());
    }

    #[test]
    fn strips_sequences_and_keeps_visible_bytes() {
        let mut p = parser("n1");
        let mut input = seq("A");
        input.extend_from_slice(b"visible");
        input.extend_from_slice(&seq("D;0"));
        let (out, ev) = p.feed(&input);
        assert_eq!(String::from_utf8(out).unwrap(), "visible");
        // Nothing is emitted before the nonce proves the source.
        assert!(ev.is_empty(), "unarmed parser must not emit events");
    }

    #[test]
    fn nonce_marker_arms_the_parser() {
        let mut p = parser("n1");
        let (_, ev) = p.feed(&seq(";n1"));
        assert_eq!(ev, vec![IntegrationEvent::Trusted]);
        assert!(p.armed());
        let (_, ev) = p.feed(&seq("A"));
        assert_eq!(ev, vec![IntegrationEvent::PromptStart]);
    }

    #[test]
    fn wrong_nonce_never_arms() {
        let mut p = parser("n1");
        let (_, ev) = p.feed(&seq(";spoofed"));
        assert!(ev.is_empty());
        assert!(!p.armed());
        let (_, ev) = p.feed(&seq("D;0"));
        assert!(ev.is_empty(), "untrusted D must not close a command");
    }

    #[test]
    fn command_line_trusted_only_with_the_nonce() {
        let mut p = parser("n1");
        let (_, ev) = p.feed(&seq("E;echo hi;n1"));
        assert_eq!(
            ev,
            vec![IntegrationEvent::CommandLine {
                text: "echo hi".into(),
                trusted: true
            }]
        );
        // A spoofed line from program output: armed, but not attributed to
        // the shell's nonce — recorded as untrusted, never as fact.
        let mut p = parser("n1");
        let (_, _) = p.feed(&seq(";n1"));
        let (_, ev) = p.feed(&seq("E;rm -rf /;attacker"));
        assert_eq!(
            ev,
            vec![IntegrationEvent::CommandLine {
                text: "rm -rf /;attacker".into(),
                trusted: false
            }]
        );
    }

    #[test]
    fn command_line_may_contain_semicolons() {
        let mut p = parser("n1");
        let (_, ev) = p.feed(&seq("E;echo a; echo b;n1"));
        assert_eq!(
            ev,
            vec![IntegrationEvent::CommandLine {
                text: "echo a; echo b".into(),
                trusted: true
            }]
        );
    }

    #[test]
    fn survives_a_sequence_split_across_chunks() {
        let mut p = parser("n1");
        let full = seq("E;npm test;n1");
        let (a, mid) = p.feed(&full[..6]);
        let (b, rest) = p.feed(&full[6..]);
        assert!(mid.is_empty());
        assert!(String::from_utf8(a).unwrap().is_empty());
        assert!(String::from_utf8(b).unwrap().is_empty());
        assert_eq!(
            rest,
            vec![IntegrationEvent::CommandLine {
                text: "npm test".into(),
                trusted: true
            }]
        );
    }

    #[test]
    fn flush_returns_a_truncated_sequence() {
        let mut p = parser("n1");
        let (_, ev) = p.feed(b"\x1b]633;A");
        assert!(ev.is_empty());
        assert_eq!(p.flush(), b"\x1b]633;A");
    }

    #[test]
    fn bel_terminator_is_accepted() {
        let mut p = parser("n1");
        let (out, ev) = p.feed(b"x\x1b]633;;n1\x07y");
        assert_eq!(String::from_utf8(out).unwrap(), "xy");
        assert_eq!(ev, vec![IntegrationEvent::Trusted]);
    }

    #[test]
    fn cwd_property_and_osc7_are_both_understood() {
        let mut p = parser("n1");
        let (_, _) = p.feed(&seq(";n1"));
        let (_, ev) = p.feed(&seq("P;Cwd=/home/me/proj"));
        assert_eq!(ev, vec![IntegrationEvent::Cwd("/home/me/proj".into())]);
        let (_, ev) = p.feed(b"\x1b]7;file://host/tmp/x\x1b\\");
        assert_eq!(ev, vec![IntegrationEvent::Cwd("/tmp/x".into())]);
        // Windows drive letters come through as `/C:/…`
        let (_, ev) = p.feed(b"\x1b]7;file://host/C:/Users/me\x1b\\");
        assert_eq!(ev, vec![IntegrationEvent::Cwd("C:/Users/me".into())]);
    }

    #[test]
    fn other_properties_are_reported_but_not_special_cased() {
        let mut p = parser("n1");
        let (_, _) = p.feed(&seq(";n1"));
        let (_, ev) = p.feed(&seq("P;IsWindows=True"));
        assert_eq!(
            ev,
            vec![IntegrationEvent::Property {
                name: "IsWindows".into(),
                value: "True".into()
            }]
        );
    }

    #[test]
    fn tracker_records_command_cwd_exit_and_output() {
        let mut p = parser("n1");
        let mut t = CommandTracker::new();
        let mut stream = Vec::new();
        stream.extend_from_slice(&seq(";n1"));
        stream.extend_from_slice(&seq("A"));
        stream.extend_from_slice(&seq("E;npm test;n1"));
        stream.extend_from_slice(&seq("C"));
        stream.extend_from_slice(b"\x1b[32mPASS\x1b[0m 42 tests\r\n");
        stream.extend_from_slice(&seq("D;0"));
        for e in p.feed(&stream).1 {
            // The host feeds filtered bytes to the tracker as it renders
            // them, so output has to land between `C` and `D` — never after.
            if e == IntegrationEvent::CommandExecuted {
                t.apply(&e);
                t.push_output(b"\x1b[32mPASS\x1b[0m 42 tests\r\n");
                continue;
            }
            t.apply(&e);
        }
        let last = t.last().expect("one command recorded");
        assert_eq!(last.command, "npm test");
        assert_eq!(last.exit_code, Some(0));
        assert!(!last.failed());
        assert_eq!(last.output, "PASS 42 tests");
    }

    #[test]
    fn tracker_marks_a_failure() {
        let mut t = CommandTracker::new();
        t.apply(&IntegrationEvent::Cwd("/w".into()));
        t.apply(&IntegrationEvent::CommandLine {
            text: "cargo test".into(),
            trusted: true,
        });
        t.apply(&IntegrationEvent::CommandExecuted);
        t.apply(&IntegrationEvent::CommandFinished(Some(101)));
        let last = t.last().unwrap();
        assert_eq!(last.cwd, "/w");
        assert!(last.failed());
    }

    #[test]
    fn tracker_ignores_output_outside_a_command_window() {
        let mut t = CommandTracker::new();
        t.push_output(b"prompt noise");
        t.apply(&IntegrationEvent::CommandLine {
            text: "ls".into(),
            trusted: true,
        });
        // No `C` yet → still not capturing.
        t.push_output(b"not output");
        assert_eq!(t.last(), None);
    }

    #[test]
    fn untrusted_command_lines_are_kept_but_never_offered_as_context() {
        let mut t = CommandTracker::new();
        t.apply(&IntegrationEvent::CommandLine {
            text: "echo legit".into(),
            trusted: true,
        });
        t.apply(&IntegrationEvent::CommandFinished(Some(0)));
        assert!(t.context_block(500).is_some());

        let mut t = CommandTracker::new();
        t.apply(&IntegrationEvent::CommandLine {
            text: "forged".into(),
            trusted: false,
        });
        t.apply(&IntegrationEvent::CommandFinished(Some(0)));
        assert_eq!(t.len(), 1);
        assert!(t.context_block(500).is_none());
    }

    #[test]
    fn tracker_never_drops_past_the_cap() {
        let mut t = CommandTracker::new();
        for i in 0..(MAX_TRACKED_COMMANDS + 10) {
            t.apply(&IntegrationEvent::CommandLine {
                text: format!("cmd {i}"),
                trusted: true,
            });
            t.apply(&IntegrationEvent::CommandFinished(Some(0)));
        }
        assert_eq!(t.len(), MAX_TRACKED_COMMANDS);
        assert_eq!(
            t.last().unwrap().command,
            format!("cmd {}", MAX_TRACKED_COMMANDS + 9)
        );
    }

    #[test]
    fn context_block_is_capped_on_a_char_boundary() {
        let mut t = CommandTracker::new();
        t.apply(&IntegrationEvent::CommandLine {
            text: "echo ünïcode".into(),
            trusted: true,
        });
        t.apply(&IntegrationEvent::CommandExecuted);
        for _ in 0..200 {
            t.push_output("ü".repeat(64).as_bytes());
        }
        t.apply(&IntegrationEvent::CommandFinished(Some(0)));
        let block = t.context_block(120).expect("last command is available");
        assert!(block.len() <= 160, "len={}", block.len());
        assert!(block.ends_with("[truncated]"));
    }

    #[test]
    fn history_block_lists_recent_commands_with_status() {
        let mut t = CommandTracker::new();
        for (cmd, code) in [("a", 0), ("b", 1), ("c", 0)] {
            t.apply(&IntegrationEvent::CommandLine {
                text: cmd.into(),
                trusted: true,
            });
            t.apply(&IntegrationEvent::CommandFinished(Some(code)));
        }
        let block = t.history_block(2, 500).unwrap();
        assert!(block.contains("$ b   [exit 1]"));
        assert!(block.contains("$ c   [ok]"));
        assert!(!block.contains("$ a"));
    }

    #[test]
    fn strip_ansi_removes_sgr_osc_and_control_noise() {
        assert_eq!(strip_ansi(b"\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi(b"a\x1b]0;title\x07b"), "ab");
        assert_eq!(strip_ansi(b"done\r\nnext"), "done\nnext");
        assert_eq!(strip_ansi(b"abc\x08d"), "abd");
        assert_eq!(strip_ansi(b"keep\ttabs"), "keep\ttabs");
    }

    #[test]
    fn shell_scripts_are_selected_by_executable_name() {
        assert_eq!(
            IntegrationShell::for_path(r"C:\Program Files\Git\bin\bash.exe"),
            Some(IntegrationShell::Bash)
        );
        assert_eq!(
            IntegrationShell::for_path("/bin/zsh"),
            Some(IntegrationShell::Zsh)
        );
        assert_eq!(
            IntegrationShell::for_path("pwsh.exe"),
            Some(IntegrationShell::Pwsh)
        );
        assert_eq!(
            IntegrationShell::for_path("/usr/bin/fish"),
            Some(IntegrationShell::Fish)
        );
        assert_eq!(
            IntegrationShell::for_path(r"C:\Windows\System32\cmd.exe"),
            None
        );
        // dash must not receive a bash-only init file.
        assert_eq!(IntegrationShell::for_path("/bin/sh"), None);
        assert_eq!(IntegrationShell::for_path("/bin/dash"), None);
    }

    #[test]
    fn feed_segments_preserves_order_of_bytes_and_events() {
        let mut p = parser("n1");
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&seq(";n1"));
        chunk.extend_from_slice(b"prompt");
        chunk.extend_from_slice(&seq("C"));
        chunk.extend_from_slice(b"out");
        chunk.extend_from_slice(&seq("D;0"));
        let segs = p.feed_segments(&chunk);
        assert_eq!(
            segs,
            vec![
                Segment::Event(IntegrationEvent::Trusted),
                Segment::Bytes(b"prompt".to_vec()),
                Segment::Event(IntegrationEvent::CommandExecuted),
                Segment::Bytes(b"out".to_vec()),
                Segment::Event(IntegrationEvent::CommandFinished(Some(0))),
            ]
        );
    }

    #[test]
    fn every_script_emits_the_protocol_it_claims() {
        // A cheap but real guard: the scripts must reference the odd sequence
        // prefix and the nonce env var, or integration is silently dead.
        for shell in [
            IntegrationShell::Bash,
            IntegrationShell::Zsh,
            IntegrationShell::Fish,
            IntegrationShell::Pwsh,
        ] {
            let s = shell.script();
            assert!(s.contains("633;"), "{:?} missing 633 protocol", shell);
            assert!(
                s.contains(NONCE_ENV),
                "{:?} missing the nonce env var",
                shell
            );
            assert!(
                s.contains(INJECTION_ENV),
                "{:?} missing the injection guard",
                shell
            );
        }
    }
}
