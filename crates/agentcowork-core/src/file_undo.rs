//! P64.7 — restore a file to a captured pre-mutation snapshot.
//!
//! The shell (`fs_undo_restore`) is the UI consumer; this is the bytes
//! function that path calls, so a real-repo fixture can prove restore
//! without Tauri. A missing snapshot is a hard error, never a no-op success.

use std::path::Path;

/// Write `before` bytes back (or delete the file when the snapshot was a
/// creation). Parents are created if needed.
pub fn restore_file_to_bytes(path: &Path, before: Option<&[u8]>) -> std::io::Result<()> {
    match before {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, bytes)
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn p64_restore_on_a_real_git_repo() {
        let dir = std::env::temp_dir().join(format!("p64-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let st = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .expect("git");
            assert!(st.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let file = dir.join("src/a.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let original = b"fn main() {}\n";
        std::fs::write(&file, original).unwrap();
        git(&["add", "src/a.rs"]);
        git(&["commit", "-qm", "base"]);
        std::fs::write(&file, b"fn main() { panic!(); }\n").unwrap();
        assert_ne!(std::fs::read(&file).unwrap(), original);
        restore_file_to_bytes(&file, Some(original)).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
