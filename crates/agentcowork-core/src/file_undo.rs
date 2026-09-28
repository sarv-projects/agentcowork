//! P64.7 — restore a file to a captured pre-mutation snapshot.
//!
//! The shell (`fs_undo_restore`) is the UI consumer; this is the bytes
//! function that path calls, so a real-repo fixture can prove restore
//! without Tauri. A missing snapshot is a hard error, never a no-op success.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Write `before` bytes back (or delete the file when the snapshot was a
/// creation). Parents are created if needed.
pub fn restore_file_to_bytes(path: &Path, before: Option<&[u8]>) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let parent = fs::canonicalize(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "undo path has no file name"))?;
    let target = parent.join(name);

    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to restore through a symlink",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    match before {
        Some(bytes) => {
            let (temp, mut file) = create_temp_sibling(&parent, name)?;
            let result = (|| {
                file.write_all(bytes)?;
                file.sync_all()?;
                fs::rename(&temp, &target)
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temp);
            }
            result
        }
        None => match fs::remove_file(target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

fn create_temp_sibling(parent: &Path, name: &std::ffi::OsStr) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..32 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temp_name = std::ffi::OsString::from(".");
        temp_name.push(name);
        temp_name.push(format!(
            ".agentcowork-undo-{}-{sequence}.tmp",
            std::process::id()
        ));
        let temp = parent.join(temp_name);
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique undo temporary file",
    ))
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

    #[cfg(unix)]
    #[test]
    fn restore_refuses_a_symlink_target_without_touching_its_referent() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("undo-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let referent = dir.join("outside.txt");
        let target = dir.join("target.txt");
        std::fs::write(&referent, b"outside").unwrap();
        symlink(&referent, &target).unwrap();

        let err = restore_file_to_bytes(&target, Some(b"restored")).unwrap_err();
        assert_eq!(std::fs::read(&referent).unwrap(), b"outside");
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
