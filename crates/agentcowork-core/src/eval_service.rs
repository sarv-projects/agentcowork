//! S0.7 EV1 runtime wiring — run the runtime verifier at task completion so a
//! K1 work receipt can carry a verified-conformance claim.
//!
//! Today EV1 is a crate-level verifier (`cargo test`). This service is the
//! JSON-RPC surface (`eval/verify`) the coordinator hits when a plan (or
//! other task) finishes.

use agentcowork_blueprint::verify::{
    Constraint, OutcomeCheck, SurfaceCheck, SurfaceContext, TaskManifest, VerificationReport,
    verify,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct EvalService {
    last: Option<VerificationReport>,
    /// P48.3 — per-surface verify results (shell/git/office/browser/desktop…
    /// beyond the filesystem-only EV1-at-plan check).
    last_surface: Option<agentcowork_blueprint::verify::SurfaceVerdict>,
}

impl EvalService {
    pub fn new() -> Self {
        Self {
            last: None,
            last_surface: None,
        }
    }

    pub fn last_report(&self) -> Option<&VerificationReport> {
        self.last.as_ref()
    }

    pub fn last_surface_verdict(&self) -> Option<&agentcowork_blueprint::verify::SurfaceVerdict> {
        self.last_surface.as_ref()
    }

    pub fn handle(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "eval/verify" => self.verify(params),
            "eval/verify_surface" => self.verify_surface(params),
            "eval/last" => Ok(json!({
                "report": self.last,
                "surfaceVerdict": self.last_surface,
            })),
            _ => Err(format!("method not found: {method}")),
        }
    }

    /// P48.3 — per-surface verify: the caller names the `check` (e.g.
    /// `shell_exit`, `browser_url`) and the engine-attached `context`
    /// observation. Missing context ⇒ honest `unverifiable`, never a fake
    /// pass. Returns the verdict + its EV1 status label.
    fn verify_surface(&mut self, params: &Value) -> Result<Value, String> {
        let check: SurfaceCheck = serde_json::from_value(
            params
                .get("check")
                .cloned()
                .ok_or_else(|| "missing check".to_string())?,
        )
        .map_err(|e| format!("bad check: {e}"))?;
        let ctx: SurfaceContext = serde_json::from_value(
            params
                .get("context")
                .cloned()
                .unwrap_or(serde_json::json!({})),
        )
        .map_err(|e| format!("bad context: {e}"))?;
        let verdict = agentcowork_blueprint::verify::verify_surface(&check, &ctx);
        let label = verdict.status_label().to_string();
        let verified = verdict.is_verified();
        self.last_surface = Some(verdict.clone());
        Ok(json!({
            "check": check.describe(),
            "surface": check.surface().as_str(),
            "verified": verified,
            "status": label,
            "verdict": verdict,
        }))
    }

    fn verify(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = params
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or("task")
            .to_string();
        let goal = params
            .get("goal")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let workspace = params
            .get("workspace")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut required_outcomes = Vec::new();
        if let Some(arr) = params.get("outcomes").and_then(Value::as_array) {
            for o in arr {
                if let Some(c) = outcome_from_json(o) {
                    required_outcomes.push(c);
                }
            }
        }
        if let Some(arr) = params.get("verify").and_then(Value::as_array) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    if looks_like_path(s) {
                        required_outcomes.push(OutcomeCheck::FileExists {
                            path: s.to_string(),
                        });
                    }
                }
            }
        }
        let manifest = TaskManifest {
            task_id: task_id.clone(),
            goal,
            required_outcomes,
            constraints: vec![Constraint::NeverSendToExternalDomains],
            budgets: Default::default(),
            evidence: vec![],
        };
        let report = verify(&manifest, Path::new(&workspace));
        let status_label = report.status.label().to_string();
        let verified = report.status.is_complete();
        self.last = Some(report.clone());
        Ok(json!({
            "taskId": task_id,
            "verified": verified,
            "status": status_label,
            "report": report,
        }))
    }
}

fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.contains('.') && !s.contains(' ')
}

fn outcome_from_json(v: &Value) -> Option<OutcomeCheck> {
    let check = v.get("check").and_then(Value::as_str)?;
    let path = v.get("path").and_then(Value::as_str)?.to_string();
    match check {
        "file_exists" | "exists" => Some(OutcomeCheck::FileExists { path }),
        "file_contains" | "contains" => Some(OutcomeCheck::FileContains {
            path,
            substring: v
                .get("substring")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "agentcowork-eval-svc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn verify_complete_when_file_exists() {
        let dir = tmp();
        fs::write(dir.join("out.txt"), "ok").unwrap();
        let mut svc = EvalService::new();
        let out = svc
            .handle(
                "eval/verify",
                &json!({
                    "taskId": "t1",
                    "workspace": dir.to_string_lossy(),
                    "goal": "write out.txt",
                    "outcomes": [{ "check": "file_exists", "path": "out.txt" }]
                }),
            )
            .unwrap();
        assert_eq!(out["verified"], true);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unverifiable_when_no_outcomes() {
        let mut svc = EvalService::new();
        let out = svc
            .handle("eval/verify", &json!({ "taskId": "t2", "goal": "chat" }))
            .unwrap();
        assert_eq!(out["verified"], false);
    }

    #[test]
    fn verify_surface_verifies_with_context_and_stays_honest_without() {
        let mut svc = EvalService::new();
        // Attached shell surface: exit 0 verifies.
        let out = svc
            .handle(
                "eval/verify_surface",
                &json!({
                    "check": { "check": "shell_exit", "expected": 0 },
                    "context": { "shell_exit": 0 },
                }),
            )
            .unwrap();
        assert_eq!(out["verified"], true);
        assert_eq!(out["status"], "verified_complete");
        assert_eq!(out["surface"], "shell");

        // Same check, no context: honestly unverifiable, never a fake pass.
        let out = svc
            .handle(
                "eval/verify_surface",
                &json!({ "check": { "check": "shell_exit", "expected": 0 } }),
            )
            .unwrap();
        assert_eq!(out["verified"], false);
        assert_eq!(out["status"], "unverifiable");
        assert_eq!(
            svc.last_surface_verdict().unwrap().status_label(),
            "unverifiable"
        );
    }
}
