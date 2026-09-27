//! Large-file finder (D11, doc 49 §WinDirStat feature list): top-N by
//! size/age with include/exclude extension filters.

use std::collections::HashSet;

use crate::walk::{Arena, FileNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    SizeDesc,
    AgeNewest,
    AgeOldest,
}

#[derive(Debug, Clone)]
pub struct FinderOptions {
    pub top_n: usize,
    pub min_size: u64,
    /// Only files newer than this many seconds (mtime within window).
    pub max_age_secs: Option<u64>,
    /// Include-only extensions (lowercased, no leading dot).
    pub extensions: Option<Vec<String>>,
    pub exclude_extensions: Option<Vec<String>>,
}

impl Default for FinderOptions {
    fn default() -> Self {
        FinderOptions {
            top_n: 100,
            min_size: 0,
            max_age_secs: None,
            extensions: None,
            exclude_extensions: None,
        }
    }
}

fn ext_of(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn ext_matches(
    name: &str,
    include: &Option<HashSet<String>>,
    exclude: &Option<HashSet<String>>,
) -> bool {
    let ext = ext_of(name);
    if let Some(inc) = include {
        if !inc.contains(&ext) {
            return false;
        }
    }
    if let Some(exc) = exclude {
        if exc.contains(&ext) {
            return false;
        }
    }
    true
}

pub fn find_large_files(
    arena: &Arena,
    opts: &FinderOptions,
    sort: SortBy,
    now: u64,
) -> Vec<FileNode> {
    let include: Option<HashSet<String>> = opts.extensions.as_ref().map(|v| {
        v.iter()
            .map(|s| s.trim_start_matches('.').to_lowercase())
            .collect()
    });
    let exclude: Option<HashSet<String>> = opts.exclude_extensions.as_ref().map(|v| {
        v.iter()
            .map(|s| s.trim_start_matches('.').to_lowercase())
            .collect()
    });

    let mut files: Vec<&FileNode> = arena
        .nodes
        .iter()
        .filter(|n| !n.is_dir && n.size >= opts.min_size)
        .filter(|n| {
            if let Some(max) = opts.max_age_secs {
                if now.saturating_sub(n.mtime) > max {
                    return false;
                }
            }
            true
        })
        .filter(|n| ext_matches(&n.name, &include, &exclude))
        .collect();

    match sort {
        SortBy::SizeDesc => {
            files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)))
        }
        SortBy::AgeNewest => files.sort_by_key(|f| std::cmp::Reverse(f.mtime)),
        SortBy::AgeOldest => files.sort_by_key(|f| f.mtime),
    }

    files.truncate(opts.top_n);
    files.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{ScanOptions, build_arena, scan};
    use std::fs;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "agentcowork-storage-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn top_n_by_size_and_filters() {
        let root = tmpdir("find");
        fs::write(root.join("big.bin"), vec![0u8; 300]).unwrap();
        fs::write(root.join("mid.txt"), vec![0u8; 200]).unwrap();
        fs::write(root.join("small.txt"), vec![0u8; 100]).unwrap();

        let records = scan(&root, &ScanOptions::default()).unwrap();
        let arena = build_arena(records, &root);

        let top = find_large_files(
            &arena,
            &FinderOptions {
                top_n: 2,
                ..Default::default()
            },
            SortBy::SizeDesc,
            u64::MAX,
        );
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].name, "big.bin");
        assert_eq!(top[1].name, "mid.txt");

        let txt = find_large_files(
            &arena,
            &FinderOptions {
                top_n: 10,
                extensions: Some(vec!["txt".into()]),
                ..Default::default()
            },
            SortBy::SizeDesc,
            u64::MAX,
        );
        assert_eq!(txt.len(), 2);
        assert!(txt.iter().all(|f| f.name.ends_with(".txt")));

        let _ = fs::remove_dir_all(&root);
    }
}
