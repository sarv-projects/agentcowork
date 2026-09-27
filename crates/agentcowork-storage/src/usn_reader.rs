//! P36 (G7) — the real USN journal reader.
//!
//! Two layers, split so the testable part is tested everywhere and the
//! Windows-only part is a thin ioctl seam:
//!
//! 1. **Pure parse layer** (this file, compiled on every platform): the
//!    on-disk byte layout of `USN_RECORD_V2` + the reason-bit map, written as
//!    plain byte parsing with unit tests on Linux/macOS.
//! 2. **WinAPI layer** (`ntfs` submodule, `#[cfg(windows)]` only): opens the
//!    volume with `CreateFileW`, calls `FSCTL_QUERY_USN_JOURNAL` /
//!    `FSCTL_READ_USN_JOURNAL` via `DeviceIoControl`, and resolves parent
//!    file reference numbers to paths via `FSCTL_ENUM_USN_DATA` (cached FRN
//!    map, updated incrementally from each journal read).
//!
//! The consumer contract (`UsnCursor::ingest` gap-abort) already lives in
//! `usn.rs`; this module feeds it.

use std::path::PathBuf;

use crate::usn::{UsnReason, UsnRecord};

/// Maximum record length we accept (USN_RECORD_V2 with a bounded filename).
/// Real records are ≤ 60 + 2×255 + pad ≈ 578 bytes.
const MAX_RECORD_LEN: usize = 1024;

/// A parsed USN record in its raw form (before parent-FRN path resolution).
#[derive(Debug, Clone, PartialEq)]
pub struct UsnRawRecord {
    /// The journal cursor for THIS record — the value `read_since` uses to
    /// resume; monotonic across the journal.
    pub usn: u64,
    /// The file's reference number (FRN) — stable across renames.
    pub file_ref: u64,
    /// Parent directory FRN (needed for path assembly).
    pub parent_ref: u64,
    pub reason: UsnReason,
    /// The file name as stored in the record (UTF-16 on disk, decoded here).
    pub name: String,
}

/// Map USN reason bits onto our [`UsnReason`] subset. Priority order matches
/// how the journal reports a primary event; CLOSE (0x8000_0000) is a
/// completion marker, not a change, so it never wins over the underlying
/// reason. Returns `None` for reason masks we don't index (attribute-only
/// changes, security changes, …).
pub fn reason_from_bits(bits: u32) -> Option<UsnReason> {
    if bits & 0x0100 != 0 {
        Some(UsnReason::Create)
    } else if bits & 0x0200 != 0 {
        Some(UsnReason::Delete)
    } else if bits & 0x1000 != 0 {
        Some(UsnReason::RenameOld)
    } else if bits & 0x2000 != 0 {
        Some(UsnReason::RenameNew)
    } else if bits & 0x0001 != 0 {
        Some(UsnReason::DataOverwrite)
    } else if bits & 0x0002 != 0 {
        Some(UsnReason::DataExtend)
    } else if bits & 0x0004 != 0 {
        Some(UsnReason::DataTruncate)
    } else {
        None
    }
}

/// Parse a `USN_RECORD_V2` stream from an FSCTL_READ_USN_JOURNAL buffer.
///
/// The buffer layout is: `READ_USN_JOURNAL_DATA_V0` header (48 bytes; its
/// first QWORD is the `Usn` cursor — the authoritative "read next from here"
/// value for the NEXT call) followed by zero or more records.
///
/// Returns `(records, next_usn)` — records are walked by `RecordLength`; a
/// truncated tail record (the driver may cut a record at the buffer edge)
/// stops the walk and the header cursor resumes it on the next read.
pub fn parse_record_stream(buf: &[u8]) -> (Vec<UsnRawRecord>, u64) {
    let mut out = Vec::new();
    let mut next_usn = 0u64;
    if buf.len() < 8 {
        return (out, next_usn);
    }
    // Header's first QWORD is the USN cursor (READ_USN_JOURNAL_DATA_V0.Usn).
    next_usn = u64::from_le_bytes(buf[0..8].try_into().unwrap());
    let mut off = 48usize; // READ_USN_JOURNAL_DATA_V0 header size
    while off + 4 <= buf.len() {
        let record_len = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        if record_len < 60 {
            // Corrupt footer (padding zeros) — stop; never emit garbage.
            break;
        }
        if record_len > MAX_RECORD_LEN || off + record_len > buf.len() {
            // Truncated at the buffer edge: resume at `next_usn`, not here.
            break;
        }
        if let Some(rec) = parse_one_record(&buf[off..off + record_len]) {
            out.push(rec);
        }
        off += record_len;
    }
    (out, next_usn)
}

/// Parse one `USN_RECORD_V2` (60-byte fixed header + UTF-16 name + padding).
fn parse_one_record(rec: &[u8]) -> Option<UsnRawRecord> {
    // Header field offsets (all little-endian):
    //   0  RecordLength  u32
    //   4  MajorVersion  u16  (2)
    //   6  MinorVersion  u16
    //   8  FileReferenceNumber         u64
    //  16  ParentFileReferenceNumber   u64
    //  24  Usn                           i64
    //  32  TimeStamp                     i64 (FILETIME)
    //  40  Reason                        u32
    //  44  SourceInfo                    u32
    //  48  SecurityId                    u32
    //  52  FileAttributes                u32
    //  56  FileNameLength                u16 (bytes)
    //  58  FileNameOffset                u16 (bytes from record start)
    //  60  FileName[]
    if rec.len() < 60 {
        return None;
    }
    let major = u16::from_le_bytes(rec[4..6].try_into().unwrap());
    if !(1..=2).contains(&major) {
        return None; // Unknown version — skip defensively.
    }
    let usn = u64::from_le_bytes(rec[24..32].try_into().unwrap());
    let file_ref = u64::from_le_bytes(rec[8..16].try_into().unwrap());
    let parent_ref = u64::from_le_bytes(rec[16..24].try_into().unwrap());
    let reason_bits = u32::from_le_bytes(rec[40..44].try_into().unwrap());
    let name_len = u16::from_le_bytes(rec[56..58].try_into().unwrap()) as usize;
    let name_off = u16::from_le_bytes(rec[58..60].try_into().unwrap()) as usize;
    if name_off < 60 || name_off + name_len > rec.len() {
        return None;
    }
    let raw = &rec[name_off..name_off + name_len];
    // UTF-16-LE decoding; corrupt halves become U+FFFD, never a panic.
    let mut name = String::with_capacity(raw.len() / 2);
    for c in raw.chunks_exact(2) {
        let u = u16::from_le_bytes([c[0], c[1]]);
        match char::from_u32(u as u32) {
            Some(ch) => name.push(ch),
            None => name.push('\u{FFFD}'),
        }
    }
    let reason = reason_from_bits(reason_bits)?;
    Some(UsnRawRecord {
        usn,
        file_ref,
        parent_ref,
        reason,
        name,
    })
}

/// Assemble an absolute `PathBuf` from a chain of names (root-first).
pub fn assemble_path(volume: &str, chain: &[&str]) -> PathBuf {
    // Never strip the root down to a drive-relative prefix: `PathBuf::from`
    // + `push` on `"C:"` yields `C:child` (drive-relative, wrong volume).
    // Re-anchor with one separator so joins stay absolute on any platform.
    let base = volume.trim_end_matches(['\\', '/']);
    if chain.is_empty() {
        return PathBuf::from(base);
    }
    let mut p = PathBuf::from(format!("{base}{}", std::path::MAIN_SEPARATOR));
    for part in chain {
        p.push(part);
    }
    p
}

/// Convert a raw record into the consumer-facing [`UsnRecord`] given the
/// resolved parent chain (root-first — `["Users", "alice"]` for
/// `C:\Users\alice\file.txt`). Kept here so path assembly is unit-testable
/// on any platform.
///
/// The record's `file_ref` is carried through: on NTFS the 64-bit file
/// reference number is `sequence:16 | mft_index:48`, i.e. already
/// incarnation-aware, which is what lets a delta be correlated with a
/// [`crate::identity::FileIdentity`] instead of by path alone
/// (`ARCH/25-FILES.md` §2, `REQ-FILES-002`).
pub fn to_usn_record(volume: &str, raw: &UsnRawRecord, parent_chain: &[&str]) -> UsnRecord {
    let mut chain: Vec<&str> = parent_chain.to_vec();
    chain.push(&raw.name);
    let path = assemble_path(volume, &chain);
    UsnRecord {
        usn: raw.usn,
        reason: raw.reason,
        path,
        file_ref: Some(raw.file_ref),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- synthetic USN_RECORD_V2 builder ------------------------------------

    /// Append one synthetic USN_RECORD_V2 (padded to 8) to `buf`.
    fn push_record(buf: &mut Vec<u8>, file_ref: u64, parent_ref: u64, reason: u32, name: &str) {
        let name_bytes: Vec<u16> = name.encode_utf16().collect();
        let mut padded = 60 + name_bytes.len() * 2;
        padded = (padded + 7) & !7;
        let start = buf.len();
        buf.resize(start + padded, 0);
        let rec = &mut buf[start..start + padded];
        rec[0..4].copy_from_slice(&(padded as u32).to_le_bytes());
        rec[4..6].copy_from_slice(&2u16.to_le_bytes()); // major = 2
        rec[6..8].copy_from_slice(&0u16.to_le_bytes()); // minor = 0
        rec[8..16].copy_from_slice(&file_ref.to_le_bytes());
        rec[16..24].copy_from_slice(&parent_ref.to_le_bytes());
        rec[24..32].copy_from_slice(&(start as u64).to_le_bytes()); // usn (unique-ish)
        rec[32..40].copy_from_slice(&0i64.to_le_bytes()); // timestamp
        rec[40..44].copy_from_slice(&reason.to_le_bytes());
        rec[44..48].copy_from_slice(&0u32.to_le_bytes()); // source_info
        rec[48..52].copy_from_slice(&0u32.to_le_bytes()); // security_id
        rec[52..56].copy_from_slice(&0u32.to_le_bytes()); // attributes
        rec[56..58].copy_from_slice(&((name_bytes.len() * 2) as u16).to_le_bytes());
        rec[58..60].copy_from_slice(&60u16.to_le_bytes()); // name offset
        for (i, u) in name_bytes.iter().enumerate() {
            rec[60 + i * 2..60 + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
    }

    fn header(buf: &mut Vec<u8>, next_usn: u64) {
        buf.extend_from_slice(&next_usn.to_le_bytes());
        buf.resize(48, 0);
    }

    // ------------------------------------------------------------------------

    #[test]
    fn parses_batch_with_header_cursor() {
        let mut buf = Vec::new();
        header(&mut buf, 777);
        push_record(&mut buf, 1, 0, 0x0100, "a.txt");
        push_record(&mut buf, 2, 1, 0x0001, "b.bin");
        let (recs, next) = parse_record_stream(&buf);
        assert_eq!(next, 777, "header cursor is authoritative");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].name, "a.txt");
        assert_eq!(recs[0].reason, UsnReason::Create);
        assert_eq!(recs[0].parent_ref, 0);
        assert_eq!(recs[1].name, "b.bin");
        assert_eq!(recs[1].reason, UsnReason::DataOverwrite);
    }

    #[test]
    fn truncated_tail_record_stops_cleanly() {
        let mut buf = Vec::new();
        header(&mut buf, 5);
        push_record(&mut buf, 10, 0, 0x0200, "gone.txt");
        push_record(&mut buf, 11, 10, 0x0001, "second.bin");
        // Cut 3 bytes into the second record: the parser must keep the first
        // record and stop (not panic, not emit a partial second record).
        let second_start = (48 + (60 + 16 + 7)) & !7; // first record length
        buf.truncate(second_start + 10);
        let (recs, next) = parse_record_stream(&buf);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "gone.txt");
        assert_eq!(next, 5, "cursor comes from header even when tail is cut");
    }

    #[test]
    fn empty_or_too_short_buffer_is_safe() {
        assert_eq!(parse_record_stream(&[]).1, 0);
        assert_eq!(parse_record_stream(&[1, 2, 3]).1, 0);
    }

    #[test]
    fn reason_priority_and_close_bit() {
        assert_eq!(
            reason_from_bits(0x0100 | 0x8000_0000),
            Some(UsnReason::Create)
        );
        assert_eq!(
            reason_from_bits(0x2000 | 0x1000),
            Some(UsnReason::RenameOld)
        );
        assert_eq!(reason_from_bits(0x8000_0000), None); // close-only → filtered
        assert_eq!(reason_from_bits(0x0000), None);
        assert_eq!(reason_from_bits(0x4000), None); // EAs_CHANGE — not indexed
    }

    #[test]
    fn utf16_names_decode() {
        let mut buf = Vec::new();
        header(&mut buf, 1);
        push_record(&mut buf, 9, 0, 0x0002, "caf\u{00E9}.txt");
        let (recs, _) = parse_record_stream(&buf);
        assert_eq!(recs[0].name, "caf\u{00E9}.txt");
    }

    #[test]
    fn unknown_major_version_skipped() {
        let mut buf = Vec::new();
        header(&mut buf, 0);
        // Hand-craft a record with major version 99. The first record starts
        // at offset 48 (after the 48-byte header); version is bytes 4..6 of it.
        push_record(&mut buf, 3, 0, 0x0100, "x.txt");
        buf[48 + 4..48 + 6].copy_from_slice(&99u16.to_le_bytes());
        let (recs, _) = parse_record_stream(&buf);
        assert!(recs.is_empty());
    }

    #[test]
    fn assemble_path_joins_chain() {
        assert_eq!(
            assemble_path("C:\\", &["Users", "alice"]),
            std::path::PathBuf::from("C:/Users/alice")
        );
        assert_eq!(assemble_path("C:\\", &[]), std::path::PathBuf::from("C:"));
        assert_eq!(
            assemble_path("D:", &["a"]),
            std::path::PathBuf::from("D:/a")
        );
    }

    #[test]
    fn to_usn_record_builds_full_path() {
        let raw = UsnRawRecord {
            usn: 42,
            file_ref: 7,
            parent_ref: 6,
            reason: UsnReason::DataExtend,
            name: "report.docx".into(),
        };
        let rec = to_usn_record("C:\\", &raw, &["Users", "alice"]);
        assert_eq!(
            rec.path,
            std::path::PathBuf::from("C:/Users/alice/report.docx")
        );
        assert_eq!(rec.reason, UsnReason::DataExtend);
        assert_eq!(rec.usn, 42);
        // The FRN rides along so a delta can be correlated with a file
        // identity rather than by path.
        assert_eq!(rec.file_ref, Some(7));
    }
}
