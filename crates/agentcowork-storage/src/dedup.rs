//! 7-stage duplicate detection (fclones + eDirStat ordering, doc 49 §4).
//!
//! Stages: (1) size bucket → (2) xxHash3 prefix → (3) xxHash3 suffix →
//! (4) BLAKE3 full → (5) hardlink grouping (platform file identity) →
//! (6) reflink eligibility (single volume) → (7) report ordered by wasted
//! bytes. The prefix/suffix xxHash3 passes avoid reading whole files when they
//! already differ.
//!
//! **FIX-10 / `REQ-FILES-001`:** stage 5 used to count distinct `(dev, ino)`
//! pairs. Every non-Unix scan answered `dev = 0, ino = 0`, so all Windows files
//! landed in one hardlink group and `wasted_bytes` reported `0` — the exact
//! inverse of the truth. Grouping now goes through
//! [`crate::identity::FileIdentity::hardlink_key`], which yields `None` for an
//! unknown identity and for any single-link file, and both of those count as
//! their *own* physical copy rather than as "identical to everything else".

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::StorageError;
use crate::identity::{FileIdentity, FileKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DedupOptions {
    pub min_size: u64,
    pub prefix_len: usize,
    pub suffix_len: usize,
}

impl Default for DedupOptions {
    fn default() -> Self {
        DedupOptions {
            min_size: 1,
            prefix_len: 64 * 1024,
            suffix_len: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DupCandidate {
    pub path: PathBuf,
    pub size: u64,
    /// Volume: `st_dev` (POSIX) or `VolumeSerialNumber` (Windows).
    pub dev: u64,
    /// 64-bit file index. `0` means **unknown**, never a real id
    /// (`ARCH/25-FILES.md` §2) — see [`DupCandidate::identity`].
    pub ino: u64,
    /// Hardlink count (`st_nlink` / `NumberOfLinks`). `0`/`1` means the file
    /// provably has no hardlink twin.
    pub nlink: u32,
}

impl DupCandidate {
    /// The candidate's incarnation-aware file identity (`REQ-FILES-001`).
    ///
    /// Built from the legacy triple: exact on POSIX (`st_dev`, `st_ino`,
    /// `st_nlink`) and on NTFS (volume serial + the 64-bit file reference
    /// number, whose high 16 bits are the MFT sequence). A zeroed triple — the
    /// shape the pre-`FIX-10` walker produced on Windows — is reported as an
    /// *unknown* identity, which downstream stages must never read as
    /// "identical to another unknown".
    pub fn identity(&self) -> FileIdentity {
        FileIdentity::from_legacy(self.dev, self.ino, self.nlink)
    }

    /// Build a candidate from a walker record, taking the full resolved
    /// identity when the record carries one.
    pub fn from_record(r: &crate::walk::FileRecord) -> Self {
        let (dev, ino, nlink) = r.identity.legacy_parts();
        // Prefer the record's resolved identity-derived triple; a record that
        // predates `FIX-10` (or a hand-built one) falls back to its raw fields.
        let (dev, ino, nlink) = if r.identity.is_known() {
            (dev, ino, nlink)
        } else {
            (r.dev, r.ino, r.nlink)
        };
        Self {
            path: r.path.clone(),
            size: r.size,
            dev,
            ino,
            nlink,
        }
    }

    /// The key that proves this path is the *same physical file* as another
    /// candidate, or `None` when that cannot be proven (unknown identity, or
    /// `nlink <= 1`).
    pub fn hardlink_key(&self) -> Option<FileKey> {
        self.identity().hardlink_key()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DupGroup {
    pub size: u64,
    pub files: Vec<DupCandidate>,
    pub wasted_bytes: u64,
    /// Distinct physical copies in `files` (see the stage-5 note in
    /// [`find_duplicates`]). Unknown-identity and single-link files each count
    /// as their own copy.
    pub hardlink_groups: usize,
    /// `true` only when **every** member's volume is known and they all agree.
    pub reflink_eligible: bool,
    /// `false` when at least one member's file identity is unknown, i.e.
    /// `wasted_bytes` is an upper bound rather than a measurement. A member
    /// with `nlink <= 1` does *not* clear this flag: a single-link file is
    /// provably its own physical copy. Surfaced so a consumer never presents
    /// an unprovable number as fact (metadata-mode honesty,
    /// `ARCH/21-WORLD-MODEL.md` §7).
    pub identity_backed: bool,
}

pub fn find_duplicates(
    cands: &[DupCandidate],
    opts: &DedupOptions,
) -> Result<Vec<DupGroup>, StorageError> {
    // Stage 1: size bucket.
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        if c.size >= opts.min_size && c.size > 0 {
            by_size.entry(c.size).or_default().push(i);
        }
    }

    let mut groups = Vec::new();
    for (_size, idxs) in by_size {
        if idxs.len() < 2 {
            continue;
        }

        // Stage 2: xxHash3 prefix.
        let mut by_prefix: HashMap<u64, Vec<usize>> = HashMap::new();
        for &i in &idxs {
            let h = prefix_hash(&cands[i].path, opts.prefix_len)?;
            by_prefix.entry(h).or_default().push(i);
        }
        for (_p, sub) in by_prefix {
            if sub.len() < 2 {
                continue;
            }

            // Stage 3: xxHash3 suffix.
            let mut by_suffix: HashMap<u64, Vec<usize>> = HashMap::new();
            for &i in &sub {
                let h = suffix_hash(&cands[i].path, opts.suffix_len)?;
                by_suffix.entry(h).or_default().push(i);
            }
            for (_s, sub2) in by_suffix {
                if sub2.len() < 2 {
                    continue;
                }

                // Stage 4: BLAKE3 full content.
                let mut by_full: HashMap<[u8; 32], Vec<usize>> = HashMap::new();
                for &i in &sub2 {
                    let h = full_hash(&cands[i].path)?;
                    by_full.entry(h).or_default().push(i);
                }
                for (_h, idxs3) in by_full {
                    if idxs3.len() < 2 {
                        continue;
                    }
                    let files: Vec<DupCandidate> =
                        idxs3.iter().map(|&i| cands[i].clone()).collect();
                    let size = files[0].size;

                    // Stage 5: distinct *physical* copies.
                    //
                    // A candidate contributes a shared group only when its
                    // identity proves it is the same physical file as another
                    // name (`hardlink_key() == Some`). Everything else — an
                    // unknown identity, or a single-link file, which provably
                    // has no twin — counts as its own copy. Pre-FIX-10 this
                    // collapsed every Windows candidate onto the constant key
                    // `(0, 0)`, so `wasted_bytes` came out as `0`.
                    let mut hardlinks: HashSet<FileKey> = HashSet::new();
                    let mut unkeyed = 0usize;
                    let mut identity_known = true;
                    for f in &files {
                        let id = f.identity();
                        // A missing id is the only reason the copy count can be
                        // unprovable; a single-link file is *provably* its own
                        // physical copy, so it does not weaken the claim.
                        if id.is_unknown() {
                            identity_known = false;
                        }
                        match f.hardlink_key() {
                            Some(k) => {
                                hardlinks.insert(k);
                            }
                            None => unkeyed += 1,
                        }
                    }
                    let hardlink_groups = hardlinks.len() + unkeyed;

                    // Stage 6: reflink eligible = every member on one known
                    // volume. An unknown volume is never assumed eligible.
                    let mut volumes: HashSet<u64> = HashSet::new();
                    let mut volume_known = true;
                    for f in &files {
                        let id = f.identity();
                        if id.is_known() {
                            volumes.insert(id.volume());
                        } else {
                            volume_known = false;
                        }
                    }
                    let reflink_eligible = volume_known && volumes.len() == 1;

                    // Stage 7: wasted bytes = distinct physical copies
                    // beyond the one we keep (hardlink names waste 0).
                    let wasted = size.saturating_mul(hardlink_groups.saturating_sub(1) as u64);
                    let identity_backed = identity_known;

                    groups.push(DupGroup {
                        size,
                        files,
                        wasted_bytes: wasted,
                        hardlink_groups,
                        reflink_eligible,
                        identity_backed,
                    });
                }
            }
        }
    }

    groups.sort_by_key(|g| std::cmp::Reverse(g.wasted_bytes));
    Ok(groups)
}

fn prefix_hash(path: &PathBuf, len: usize) -> Result<u64, StorageError> {
    let mut f = File::open(path)?;
    let mut out = Vec::with_capacity(len.min(1 << 20));
    let mut chunk = vec![0u8; 64 * 1024];
    let mut remaining = len;
    while remaining > 0 {
        let k = remaining.min(chunk.len());
        let n = f.read(&mut chunk[..k])?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n]);
        remaining -= n;
    }
    Ok(crate::xxh3(&out))
}

fn suffix_hash(path: &PathBuf, len: usize) -> Result<u64, StorageError> {
    let mut f = File::open(path)?;
    let flen = f.metadata()?.len();
    if flen == 0 {
        return Ok(crate::xxh3(&[]));
    }
    let start = flen.saturating_sub(len as u64);
    f.seek(SeekFrom::Start(start))?;
    let mut out = Vec::new();
    f.read_to_end(&mut out)?;
    Ok(crate::xxh3(&out))
}

fn full_hash(path: &PathBuf) -> Result<[u8; 32], StorageError> {
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "agentcowork-storage-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// `Path::ends_with` matches whole components, not suffixes.
    fn has_ext(p: &std::path::Path, ext: &str) -> bool {
        p.extension().and_then(|e| e.to_str()) == Some(ext)
    }

    #[test]
    fn finds_duplicates_not_distinct() {
        let root = tmpdir("dedup");
        fs::write(root.join("x1.bin"), vec![0u8; 4096]).unwrap();
        fs::write(root.join("x2.bin"), vec![0u8; 4096]).unwrap();
        fs::write(root.join("y1.bin"), vec![1u8; 4096]).unwrap();
        fs::write(root.join("y2.bin"), vec![1u8; 4096]).unwrap();
        fs::write(root.join("z.bin"), vec![2u8; 100]).unwrap();

        let paths = ["x1.bin", "x2.bin", "y1.bin", "y2.bin", "z.bin"];
        let cands: Vec<DupCandidate> = paths
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let path = root.join(name);
                let size = fs::metadata(&path).unwrap().len();
                DupCandidate {
                    path,
                    size,
                    dev: 0,
                    ino: (i + 1) as u64,
                    nlink: 1,
                }
            })
            .collect();

        let groups = find_duplicates(
            &cands,
            &DedupOptions {
                min_size: 1,
                prefix_len: 4096,
                suffix_len: 4096,
            },
        )
        .unwrap();

        assert_eq!(groups.len(), 2); // x pair + y pair; z singleton dropped
        for g in &groups {
            assert_eq!(g.size, 4096);
            assert_eq!(g.files.len(), 2);
            assert_eq!(g.wasted_bytes, 4096);
            assert_eq!(g.hardlink_groups, 2);
            assert!(g.reflink_eligible);
        }
        // Descending wasted order (equal here).
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn same_inode_is_one_hardlink_group() {
        let root = tmpdir("hardlink");
        fs::write(root.join("orig.bin"), vec![7u8; 1024]).unwrap();
        fs::hard_link(root.join("orig.bin"), root.join("link.bin")).unwrap();

        let files = vec![
            DupCandidate {
                path: root.join("orig.bin"),
                size: 1024,
                dev: 0,
                ino: 1,
                nlink: 2,
            },
            DupCandidate {
                path: root.join("link.bin"),
                size: 1024,
                dev: 0,
                ino: 1,
                nlink: 2,
            },
        ];
        let groups = find_duplicates(&files, &DedupOptions::default()).unwrap();
        assert_eq!(groups.len(), 1);
        // Same inode → one physical copy → nothing actually wasted.
        assert_eq!(groups[0].hardlink_groups, 1);
        assert_eq!(groups[0].wasted_bytes, 0);
        assert!(groups[0].identity_backed);

        let _ = fs::remove_dir_all(&root);
    }

    // --- FIX-10: an unknown identity is never "identical" -------------------

    #[test]
    fn zeroed_dev_ino_does_not_collapse_every_file_into_one_copy() {
        // The exact pre-fix Windows scan record: `dev = 0, ino = 0`. Pre-fix
        // this produced `hardlink_groups == 1` and `wasted_bytes == 0`.
        let root = tmpdir("zeroed");
        fs::write(root.join("a.bin"), vec![3u8; 2048]).unwrap();
        fs::write(root.join("b.bin"), vec![3u8; 2048]).unwrap();

        let files = vec![
            DupCandidate {
                path: root.join("a.bin"),
                size: 2048,
                dev: 0,
                ino: 0,
                nlink: 1,
            },
            DupCandidate {
                path: root.join("b.bin"),
                size: 2048,
                dev: 0,
                ino: 0,
                nlink: 1,
            },
        ];
        let groups = find_duplicates(&files, &DedupOptions::default()).unwrap();
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        // Two distinct physical copies, so one full size is reclaimable.
        assert_eq!(g.hardlink_groups, 2);
        assert_eq!(g.wasted_bytes, 2048);
        // ...and the number is flagged as an unprovable upper bound.
        assert!(!g.identity_backed);
        // An unknown volume is never assumed reflink-eligible.
        assert!(!g.reflink_eligible);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_volume_is_never_reflink_eligible() {
        let root = tmpdir("novol");
        fs::write(root.join("a.bin"), vec![5u8; 512]).unwrap();
        fs::write(root.join("b.bin"), vec![5u8; 512]).unwrap();
        let files = ["a.bin", "b.bin"]
            .iter()
            .map(|n| DupCandidate {
                path: root.join(n),
                size: 512,
                dev: 0,
                ino: 0,
                nlink: 1,
            })
            .collect::<Vec<_>>();
        let groups = find_duplicates(&files, &DedupOptions::default()).unwrap();
        assert!(!groups[0].reflink_eligible);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mixed_known_and_unknown_never_merge_into_one_group() {
        // `a` has a real id with nlink 2, `b` is the pre-fix zeroed record.
        // They must not be reported as one physical copy.
        let root = tmpdir("mixed");
        fs::write(root.join("a.bin"), vec![9u8; 256]).unwrap();
        fs::write(root.join("b.bin"), vec![9u8; 256]).unwrap();
        let files = vec![
            DupCandidate {
                path: root.join("a.bin"),
                size: 256,
                dev: 66,
                ino: 0x0002_0000_0000_0010,
                nlink: 2,
            },
            DupCandidate {
                path: root.join("b.bin"),
                size: 256,
                dev: 0,
                ino: 0,
                nlink: 1,
            },
        ];
        let groups = find_duplicates(&files, &DedupOptions::default()).unwrap();
        assert_eq!(groups[0].hardlink_groups, 2);
        assert_eq!(groups[0].wasted_bytes, 256);
        assert!(!groups[0].identity_backed);
        let _ = fs::remove_dir_all(&root);
    }

    // --- FIX-10: a live scan feeds dedup real identities -------------------

    #[test]
    fn scanned_carry_real_identity_and_detect_real_hardlinks() {
        let root = tmpdir("scan-identity");
        fs::write(root.join("orig.bin"), vec![11u8; 1024]).unwrap();
        fs::hard_link(root.join("orig.bin"), root.join("link.bin")).unwrap();
        fs::write(root.join("other.bin"), vec![11u8; 1024]).unwrap();

        let records = crate::walk::scan(
            &root,
            &crate::walk::ScanOptions {
                threads: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let cands: Vec<DupCandidate> = records
            .iter()
            .filter(|r| !r.is_dir && has_ext(&r.path, "bin"))
            .map(DupCandidate::from_record)
            .collect();
        assert_eq!(cands.len(), 3, "three .bin files in the scan");

        // The walker produced real ids, and the legacy triple agrees with the
        // resolved identity (FIX-10: never a fabricated zero).
        for c in &cands {
            assert!(
                c.ino != 0,
                "scan record has a fabricated zero id: {}",
                c.path.display()
            );
            assert!(c.identity().is_known());
            assert_eq!(c.identity().legacy_parts(), (c.dev, c.ino, c.nlink));
        }

        let groups = find_duplicates(
            &cands,
            &DedupOptions {
                min_size: 1,
                prefix_len: 1024,
                suffix_len: 1024,
            },
        )
        .unwrap();
        assert_eq!(groups.len(), 1, "one content group of 3 files");
        let g = &groups[0];
        assert_eq!(g.files.len(), 3);
        // orig.bin + link.bin share one physical copy; other.bin is a second.
        assert_eq!(g.hardlink_groups, 2);
        assert_eq!(g.wasted_bytes, 1024);
        assert!(g.identity_backed);
        assert!(g.reflink_eligible);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn metadata_only_scan_is_honest_about_unknown_identity() {
        let root = tmpdir("scan-metadata-only");
        fs::write(root.join("a.bin"), vec![1u8; 128]).unwrap();
        fs::write(root.join("b.bin"), vec![1u8; 128]).unwrap();

        let records = crate::walk::scan_with_policy(
            &root,
            &crate::walk::ScanOptions {
                threads: 1,
                ..Default::default()
            },
            crate::identity::IdentityPolicy::MetadataOnly,
        )
        .unwrap();
        let cands: Vec<DupCandidate> = records
            .iter()
            .filter(|r| !r.is_dir && has_ext(&r.path, "bin"))
            .map(DupCandidate::from_record)
            .collect();
        assert_eq!(cands.len(), 2);
        let groups = find_duplicates(
            &cands,
            &DedupOptions {
                min_size: 1,
                prefix_len: 128,
                suffix_len: 128,
            },
        )
        .unwrap();
        // Whatever the platform reports, the result must never claim two
        // unprovable files are the same physical copy.
        assert!(groups[0].hardlink_groups >= 1);
        assert!(groups[0].wasted_bytes <= 128);

        let _ = fs::remove_dir_all(&root);
    }
}
