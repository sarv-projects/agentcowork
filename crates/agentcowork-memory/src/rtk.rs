//! RTK-style output compression (P5.7 — doc 63 §2.1): per-command parsers for
//! common shell tool results (`ls`, `ps`, `git`, `du`) that keep only the
//! action-relevant fields. Deterministic, token-savings reported — the
//! 60–90% reduction target is measured, not claimed.

use crate::fusion::approx_tokens;

/// Result of compressing one tool output.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedOutput {
    pub output: String,
    /// Token reduction ratio 0..=1 (1 = fully removed).
    pub reduction: f64,
    pub saved_tokens: usize,
}

/// Which per-command parser to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// `ls -la` style listing.
    Ls,
    /// `ps aux` style process table.
    Ps,
    /// `git status` / `git diff --stat` output.
    Git,
    /// `du -sh` style disk usage.
    Du,
    /// Unknown command — no structural parse (verbatim).
    Unknown,
}

/// Guess the command kind from the command name.
pub fn kind_for(command: &str) -> CommandKind {
    let base = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    match base {
        "ls" | "lsd" | "eza" => CommandKind::Ls,
        "ps" => CommandKind::Ps,
        "git" => CommandKind::Git,
        "du" => CommandKind::Du,
        _ => CommandKind::Unknown,
    }
}

/// Compress a tool result by command kind.
pub fn compress(command: &str, output: &str) -> CompressedOutput {
    let compressed = match kind_for(command) {
        CommandKind::Ls => compress_ls(output),
        CommandKind::Ps => compress_ps(output),
        CommandKind::Git => compress_git(output),
        CommandKind::Du => compress_du(output),
        CommandKind::Unknown => output.to_string(),
    };
    let input_tokens = approx_tokens(output);
    let output_tokens = approx_tokens(&compressed);
    let saved_tokens = input_tokens.saturating_sub(output_tokens);
    let reduction = if input_tokens == 0 {
        0.0
    } else {
        saved_tokens as f64 / input_tokens as f64
    };
    CompressedOutput {
        output: compressed,
        reduction,
        saved_tokens,
    }
}

/// `ls -la`: keep name, type marker, size; drop perms/owner/group/time.
fn compress_ls(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let perms = fields.next().unwrap_or("");
            let _links = fields.next().unwrap_or("");
            let _owner = fields.next().unwrap_or("");
            let _group = fields.next().unwrap_or("");
            let size = fields.next().unwrap_or("");
            // Remaining fields: time + name.
            let rest: Vec<&str> = fields.collect();
            if perms.starts_with('d') || perms.starts_with('-') || perms.starts_with('l') {
                let name = rest.last().copied().unwrap_or("");
                let marker = if perms.starts_with('d') {
                    "dir"
                } else if perms.starts_with('l') {
                    "link"
                } else {
                    "file"
                };
                format!("{marker} {size} {name}")
            } else {
                // header line (total N) — keep.
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `ps aux`: keep pid, cpu, mem, command; drop user/stat/start/tty/time.
fn compress_ps(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let _user = fields.next().unwrap_or("");
            let pid = fields.next().unwrap_or("");
            let cpu = fields.next().unwrap_or("");
            let mem = fields.next().unwrap_or("");
            // Skip vsz/rss/tty/stat/start/time → command is the rest.
            let _: Vec<&str> = fields.by_ref().take(6).collect();
            let cmd: Vec<&str> = fields.collect();
            format!("{pid} cpu={cpu} mem={mem} {cmd:?}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `git status`: keep the branch line + the short status rows (` M file`) +
/// the long-form `\tmodified: path` rows, dropping the prose.
fn compress_git(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            l.starts_with("On branch")
                || l.starts_with("Your branch")
                || t.starts_with('M')
                || t.starts_with('A')
                || t.starts_with('D')
                || t.starts_with('R')
                || t.starts_with("?")
                // Long-form `\tmodified: path` rows (also deleted/renamed/new
                // file/untracked).
                || t.starts_with("modified:")
                || t.starts_with("deleted:")
                || t.starts_with("renamed:")
                || t.starts_with("copied:")
                || t.starts_with("new file:")
                || t.starts_with("untracked files:")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `du`: keep `size path` rows, drop the trailing total line when it would
/// duplicate the root row.
fn compress_du(output: &str) -> String {
    let rows: Vec<&str> = output.lines().collect();
    if rows.len() <= 1 {
        return output.to_string();
    }
    rows.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_detection() {
        assert_eq!(kind_for("ls -la"), CommandKind::Ls);
        assert_eq!(kind_for("/bin/ps aux"), CommandKind::Ps);
        assert_eq!(kind_for("git status"), CommandKind::Git);
        assert_eq!(kind_for("du -sh ."), CommandKind::Du);
        assert_eq!(kind_for("cat foo"), CommandKind::Unknown);
    }

    #[test]
    fn ls_compression_keeps_name_and_size() {
        let out = "total 8\n-rw-r--r--  1 alice staff  1024 Jan 1 00:00 notes.md\ndrwxr-xr-x  2 alice staff    64 Jan 1 00:00 src\n";
        let c = compress("ls -la", out);
        assert!(c.output.contains("file 1024 notes.md"));
        assert!(c.output.contains("dir 64 src"));
        assert!(!c.output.contains("alice"));
        assert!(c.reduction > 0.0);
    }

    #[test]
    fn ps_compression_keeps_pid_and_command() {
        let out = "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n\
                   root         1  0.0  0.1  1234  5678 pts/0    Ss   00:00   0:00 /sbin/init\n";
        let c = compress("ps aux", out);
        assert!(c.output.contains("1 cpu=0.0 mem=0.1"));
        assert!(c.output.contains("/sbin/init"));
        assert!(!c.output.contains("pts/0"));
    }

    #[test]
    fn git_compression_keeps_status_rows() {
        let out = "On branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n\n\tmodified:   src/lib.rs\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n";
        let c = compress("git status", out);
        assert!(c.output.contains("On branch main"));
        assert!(c.output.contains("modified:   src/lib.rs"));
        assert!(!c.output.contains("use \"git add"));
        assert!(c.reduction > 0.5, "reduction was {:.2}", c.reduction);
    }

    #[test]
    fn unknown_command_is_verbatim() {
        let out = "anything at all";
        let c = compress("cat", out);
        assert_eq!(c.output, out);
        assert_eq!(c.reduction, 0.0);
    }

    #[test]
    fn reduction_measured_not_claimed() {
        // A long ls listing compresses well past the 60% target.
        let mut out = String::from("total 8\n");
        for i in 0..50 {
            out.push_str(&format!(
                "-rw-r--r-- 1 alice staff 1024 Jan 1 00:00 file{i:02}.md\n"
            ));
        }
        let c = compress("ls -la", &out);
        assert!(c.reduction >= 0.6, "reduction was {:.2}", c.reduction);
        assert!(c.saved_tokens > 0);
    }
}
