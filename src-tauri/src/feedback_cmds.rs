//! P11.6.1 — in-app beta feedback mechanism.
//!
//! A thin, honest surface: the Feedback panel composes a bug report or
//! feature request and this command appends it as a date-stamped markdown
//! entry to `<data_dir>/feedback/feedback.md` (the UI renders the human date
//! from the epoch-ms stamp). Nothing leaves the device — the user copies the
//! report and files it (GitHub issues / discussions) themselves. The file is
//! local so reports survive restarts and can be bulk-exported.

use std::fs::{self, OpenOptions};
use std::io::Write;

/// Append one feedback entry. Returns the absolute path written.
#[tauri::command]
pub fn feedback_submit(
    kind: String,
    title: String,
    body: String,
    category: Option<String>,
) -> Result<String, String> {
    let base = agentcowork_core::default_data_dir().join("feedback");
    write_feedback(&base, &kind, &title, &body, category.as_deref())
}

/// The append core, over an injected base dir — the Tauri command uses the
/// default data dir; tests use a temp dir. Validation lives here (single
/// enforcement point), the command just forwards.
fn write_feedback(
    base: &std::path::Path,
    kind: &str,
    title: &str,
    body: &str,
    category: Option<&str>,
) -> Result<String, String> {
    if !matches!(kind, "bug" | "feature") {
        return Err(format!(
            "unknown feedback kind: {kind} (expected bug|feature)"
        ));
    }
    if title.trim().is_empty() {
        return Err("feedback title is required".to_string());
    }
    if body.trim().is_empty() {
        return Err("feedback body is required".to_string());
    }
    fs::create_dir_all(base).map_err(|e| format!("feedback dir: {e}"))?;
    let file = base.join("feedback.md");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let category = category
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());

    let mut entry = String::new();
    entry.push_str(&format!("## {} — {}\n\n", kind, title.trim()));
    entry.push_str(&format!("- **ts:** {now}\n"));
    if let Some(cat) = category {
        entry.push_str(&format!("- **category:** {cat}\n"));
    }
    entry.push('\n');
    entry.push_str(body.trim());
    entry.push_str("\n\n---\n\n");

    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| format!("feedback open: {e}"))?;
    f.write_all(entry.as_bytes())
        .map_err(|e| format!("feedback write: {e}"))?;

    Ok(file.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::write_feedback;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "agentcowork-feedback-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_a_markdown_entry_and_appends() {
        let dir = temp_dir("a");
        let p1 = write_feedback(&dir, "bug", "Title one", "Body one", Some("chat")).unwrap();
        let p2 = write_feedback(&dir, "feature", "Title two", "Body two", None).unwrap();
        assert_eq!(p1, p2); // same monthly file
        let raw = std::fs::read_to_string(p2).unwrap();
        assert!(raw.contains("## bug — Title one"));
        assert!(raw.contains("- **category:** chat"));
        assert!(raw.contains("Body one"));
        assert!(raw.contains("## feature — Title two"));
        assert!(raw.contains("Body two"));
        // Only the first entry carries a category line (the second has none).
        assert_eq!(raw.matches("- **category:**").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_missing_title_or_body() {
        let dir = temp_dir("b");
        assert!(write_feedback(&dir, "bug", "", "body", None).is_err());
        assert!(write_feedback(&dir, "bug", "title", "", None).is_err());
        assert!(write_feedback(&dir, "nope", "t", "b", None).is_err());
        assert!(!dir.join("feedback.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
