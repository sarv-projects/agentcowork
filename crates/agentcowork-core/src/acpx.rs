//! P51.19 — a small terminal driver for an AgentCowork session.
//!
//! The shape follows a named-session client: pick a session, queue a
//! follow-up, set the working directory, print JSON or stay quiet, and run
//! a saved flow. Nothing here launches an external agent. A failed doctor
//! check refuses every mutating command.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a receipt is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    Quiet,
}

impl OutputFormat {
    pub fn parse(value: &str) -> Result<Self, AcpxError> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "quiet" => Ok(Self::Quiet),
            other => Err(AcpxError::Usage(format!("unknown format {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpxError {
    DoctorFailed,
    Usage(String),
    UnknownSession(String),
    Closed(String),
    Io(String),
}

impl std::fmt::Display for AcpxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DoctorFailed => write!(f, "doctor check failed; refusing to start"),
            Self::Usage(msg) => write!(f, "{msg}"),
            Self::UnknownSession(name) => write!(f, "unknown session {name}"),
            Self::Closed(name) => write!(f, "session {name} is closed"),
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct QueuedPrompt {
    text: String,
    cwd: String,
    /// `true` when the caller asked not to wait.
    queued: bool,
    ran: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SessionRecord {
    name: String,
    cwd: String,
    closed: bool,
    queue: Vec<QueuedPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoreFile {
    sessions: BTreeMap<String, SessionRecord>,
}

/// One prompt the driver accepted. `dispatched` stays false: this binary
/// queues work for a named session and does not claim an agent launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptReceipt {
    pub session: String,
    pub cwd: String,
    pub text: String,
    pub queued: bool,
    pub dispatched: bool,
}

/// Named-session queue persisted as one JSON file.
pub struct AcpxStore {
    path: PathBuf,
    doctor_ok: bool,
    file: StoreFile,
}

impl AcpxStore {
    pub fn open(path: impl Into<PathBuf>, doctor_ok: bool) -> Result<Self, AcpxError> {
        let path = path.into();
        let file = if path.is_file() {
            let raw = fs::read_to_string(&path).map_err(|e| AcpxError::Io(e.to_string()))?;
            serde_json::from_str(&raw).unwrap_or(StoreFile {
                sessions: BTreeMap::new(),
            })
        } else {
            StoreFile {
                sessions: BTreeMap::new(),
            }
        };
        Ok(Self {
            path,
            doctor_ok,
            file,
        })
    }

    fn require_doctor(&self) -> Result<(), AcpxError> {
        if self.doctor_ok {
            Ok(())
        } else {
            Err(AcpxError::DoctorFailed)
        }
    }

    fn save(&self) -> Result<(), AcpxError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| AcpxError::Io(e.to_string()))?;
        }
        let raw =
            serde_json::to_string_pretty(&self.file).map_err(|e| AcpxError::Io(e.to_string()))?;
        fs::write(&self.path, raw).map_err(|e| AcpxError::Io(e.to_string()))
    }

    pub fn session_new(&mut self, name: &str, cwd: &str) -> Result<(), AcpxError> {
        self.require_doctor()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(AcpxError::Usage("session name is required".into()));
        }
        self.file.sessions.insert(
            name.into(),
            SessionRecord {
                name: name.into(),
                cwd: cwd.to_string(),
                closed: false,
                queue: Vec::new(),
            },
        );
        self.save()
    }

    pub fn session_names(&self) -> Result<Vec<String>, AcpxError> {
        self.require_doctor()?;
        Ok(self.file.sessions.keys().cloned().collect())
    }

    pub fn ensure(&mut self, name: &str, cwd: &str) -> Result<(), AcpxError> {
        self.require_doctor()?;
        if !self.file.sessions.contains_key(name) {
            self.session_new(name, cwd)?;
        }
        Ok(())
    }

    pub fn close(&mut self, name: &str) -> Result<(), AcpxError> {
        self.require_doctor()?;
        let session = self
            .file
            .sessions
            .get_mut(name)
            .ok_or_else(|| AcpxError::UnknownSession(name.into()))?;
        session.closed = true;
        self.save()
    }

    pub fn enqueue(
        &mut self,
        name: &str,
        text: &str,
        cwd: Option<&str>,
        no_wait: bool,
    ) -> Result<PromptReceipt, AcpxError> {
        self.require_doctor()?;
        let session = self
            .file
            .sessions
            .get_mut(name)
            .ok_or_else(|| AcpxError::UnknownSession(name.into()))?;
        if session.closed {
            return Err(AcpxError::Closed(name.into()));
        }
        if let Some(cwd) = cwd {
            session.cwd = cwd.to_string();
        }
        let cwd = session.cwd.clone();
        session.queue.push(QueuedPrompt {
            text: text.to_string(),
            cwd: cwd.clone(),
            queued: no_wait,
            ran: false,
        });
        self.save()?;
        Ok(PromptReceipt {
            session: name.into(),
            cwd,
            text: text.into(),
            queued: no_wait,
            dispatched: false,
        })
    }

    /// Run the queued prompts in order. Each becomes a receipt. This does
    /// not launch an agent; `dispatched` stays false.
    pub fn flow_run(&mut self, name: &str) -> Result<Vec<PromptReceipt>, AcpxError> {
        self.require_doctor()?;
        let session = self
            .file
            .sessions
            .get_mut(name)
            .ok_or_else(|| AcpxError::UnknownSession(name.into()))?;
        if session.closed {
            return Err(AcpxError::Closed(name.into()));
        }
        let mut receipts = Vec::new();
        for prompt in &mut session.queue {
            if prompt.ran {
                continue;
            }
            prompt.ran = true;
            receipts.push(PromptReceipt {
                session: name.into(),
                cwd: prompt.cwd.clone(),
                text: prompt.text.clone(),
                queued: prompt.queued,
                dispatched: false,
            });
        }
        self.save()?;
        Ok(receipts)
    }
}

pub fn render_receipt(receipt: &PromptReceipt, format: OutputFormat) -> String {
    match format {
        OutputFormat::Quiet => String::new(),
        OutputFormat::Json => serde_json::to_string(receipt).unwrap_or_else(|_| "{}".into()),
        OutputFormat::Text => format!(
            "session {} cwd {} queued {} — {}",
            receipt.session, receipt.cwd, receipt.queued, receipt.text
        ),
    }
}

/// Parse `acpx` argv after the subcommand name has been stripped.
pub fn run_acpx(args: &[String], store_path: &Path, doctor_ok: bool) -> Result<String, AcpxError> {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok("acpx sessions new|list|ensure|close <name> [--cwd DIR]\nacpx -s NAME [--no-wait] [--cwd DIR] [--format text|json|quiet] PROMPT\nacpx flow run NAME\n".into());
    }
    let mut store = AcpxStore::open(store_path, doctor_ok)?;
    if args[0] == "sessions" {
        let action = args.get(1).map(String::as_str).unwrap_or("");
        let name = args.get(2).cloned().unwrap_or_default();
        let cwd = flag(args, "--cwd").unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".into())
        });
        match action {
            "new" => {
                store.session_new(&name, &cwd)?;
                return Ok(format!("session {name}\n"));
            }
            "ensure" => {
                store.ensure(&name, &cwd)?;
                return Ok(format!("session {name}\n"));
            }
            "list" => {
                let names = store.session_names()?;
                return Ok(names.join("\n"));
            }
            "close" => {
                store.close(&name)?;
                return Ok(format!("closed {name}\n"));
            }
            _ => {
                return Err(AcpxError::Usage(
                    "sessions requires new|list|ensure|close".into(),
                ));
            }
        }
    }
    if args[0] == "flow" && args.get(1).map(String::as_str) == Some("run") {
        let name = args.get(2).cloned().unwrap_or_default();
        let receipts = store.flow_run(&name)?;
        let format = OutputFormat::parse(flag(args, "--format").as_deref().unwrap_or("text"))?;
        if format == OutputFormat::Quiet {
            return Ok(String::new());
        }
        if format == OutputFormat::Json {
            return Ok(serde_json::to_string(&receipts).unwrap_or_else(|_| "[]".into()));
        }
        return Ok(receipts
            .iter()
            .map(|r| render_receipt(r, OutputFormat::Text))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let name = flag(args, "-s")
        .or_else(|| flag(args, "--session"))
        .ok_or_else(|| AcpxError::Usage("session is required (-s/--session)".into()))?;
    let no_wait = args.iter().any(|a| a == "--no-wait");
    let cwd = flag(args, "--cwd");
    let format = OutputFormat::parse(flag(args, "--format").as_deref().unwrap_or("text"))?;
    let text = args
        .iter()
        .rev()
        .find(|a| !a.starts_with('-') && *a != &name)
        .cloned()
        .ok_or_else(|| AcpxError::Usage("prompt text is required".into()))?;
    let receipt = store.enqueue(&name, &text, cwd.as_deref(), no_wait)?;
    Ok(render_receipt(&receipt, format))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-acpx-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("sessions.json")
    }

    #[test]
    fn doctor_failure_refuses_a_new_session() {
        let file = path("doctor");
        let err = run_acpx(
            &["sessions".into(), "new".into(), "alpha".into()],
            &file,
            false,
        )
        .unwrap_err();
        assert_eq!(err, AcpxError::DoctorFailed);
    }

    #[test]
    fn named_session_queues_follow_up_and_flow_stays_undispatched() {
        let file = path("queue");
        run_acpx(
            &[
                "sessions".into(),
                "new".into(),
                "alpha".into(),
                "--cwd".into(),
                "/work".into(),
            ],
            &file,
            true,
        )
        .unwrap();
        let quiet = run_acpx(
            &[
                "-s".into(),
                "alpha".into(),
                "--no-wait".into(),
                "--format".into(),
                "quiet".into(),
                "next step".into(),
            ],
            &file,
            true,
        )
        .unwrap();
        assert!(quiet.is_empty());
        let flow = run_acpx(
            &[
                "flow".into(),
                "run".into(),
                "alpha".into(),
                "--format".into(),
                "json".into(),
            ],
            &file,
            true,
        )
        .unwrap();
        let receipts: Vec<PromptReceipt> = serde_json::from_str(&flow).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].cwd, "/work");
        assert!(receipts[0].queued);
        assert!(!receipts[0].dispatched);
    }
}
