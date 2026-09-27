//! P36 (G7) — the Windows USN journal reader (the raw WinAPI path), wired as
//! the W1 delta source (`ARCH/21-WORLD-MODEL.md` §4, `ARCH/25-FILES.md` §3).
//!
//! Compiled only on `cfg(windows)`. This is the seam that was previously
//! documented-but-not-built: opening the volume handle with `CreateFileW`,
//! querying the journal with `FSCTL_QUERY_USN_JOURNAL`, reading deltas with
//! `FSCTL_READ_USN_JOURNAL`, and resolving parent FRNs to paths via
//! `FSCTL_ENUM_USN_DATA` (cached, incrementally updated).
//!
//! The byte-format parsing lives in the cross-platform `usn_reader` module
//! (unit-tested on every host); this module marshals WinAPI calls, enforces the
//! read-only capability set, and maps OS failures onto the typed
//! [`JournalError`]s that the cursor/epoch/gap policy in `usn.rs` acts on.
//!
//! **Least privilege (`ARCH/25-FILES.md` §1 metadata-first).** The volume
//! handle is opened with `FILE_READ_DATA | FILE_LIST_DIRECTORY` — the
//! directory-enumeration/read-data right and nothing else. `GENERIC_READ`
//! would also imply read-attribute/synchronize rights we do not need, and
//! there is no write right anywhere on this handle: the collector is
//! structurally incapable of mutating the volume it observes. No file content
//! is read — the only data touched is the journal itself.
//!
//! **API notes** (windows-sys 0.61.2 bindings, verified line-by-line):
//! - `DeviceIoControl` is in `Win32::System::IO`; the `FSCTL_*` constants +
//!   `*_DATA_V0` structs are in `Win32::System::Ioctl` (own feature gate).
//! - Struct fields are PascalCase (`StartUsn`, `ReasonMask`, …).
//! - The `ERROR_*` journal codes come from `Win32::Foundation`.

use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_HANDLE_EOF, ERROR_INVALID_PARAMETER, ERROR_JOURNAL_DELETE_IN_PROGRESS,
    ERROR_JOURNAL_NOT_ACTIVE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_READ_DATA, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, MFT_ENUM_DATA_V0,
    READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
};

use crate::usn::{JournalChunk, JournalError, JournalReader, JournalSource, UsnRecord};
use crate::usn_reader::{UsnRawRecord, assemble_path, parse_record_stream};

/// FSCTL_* response buffer size. 64 KiB is the standard journal read chunk;
/// the MFT enum pass uses the same buffer (records are ≤ ~1 KiB each).
const BUFFER_BYTES: u32 = 64 * 1024;

/// Open the volume (e.g. `C:\` or `\\.\C:`) with backup semantics so admin
/// rights are not required to read the journal.
///
/// Desired access is `FILE_READ_DATA | FILE_LIST_DIRECTORY` only — the
/// collector can enumerate and read journal data, and nothing else.
fn open_volume(volume: &str) -> Result<HANDLE, String> {
    let wide: Vec<u16> = Path::new(volume)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: CreateFileW with a NUL-terminated wide path; null security
    // attributes; no template file. FILE_FLAG_BACKUP_SEMANTICS lets us open
    // directories/volumes without admin rights.
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_DATA | FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        Err(format!(
            "CreateFileW({volume}) failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(h)
    }
}

/// Classify an OS failure from a journal ioctl into the typed [`JournalError`]
/// the delta driver turns into "gap" or "caught up".
///
/// * `ERROR_INVALID_PARAMETER` (87) is what `FSCTL_READ_USN_JOURNAL` returns
///   when `StartUsn` is below the journal's retained `FirstUsn` — the journal
///   wrapped. That is a **gap**, never "no changes".
/// * `ERROR_JOURNAL_DELETE_IN_PROGRESS` — the journal is being deleted; gap.
/// * `ERROR_HANDLE_EOF` (38) — no entries at/after `StartUsn`: we are caught
///   up. This is the one case that genuinely means "no changes".
/// * `ERROR_JOURNAL_NOT_ACTIVE` / anything else — the journal is off, the
///   volume is ineligible, or the device failed: non-admin fallback
///   territory, surfaced to the caller (`REQ-WORLD-010`), never silently
///   elevated.
fn classify_os_error(op: &str, err: &std::io::Error) -> JournalError {
    match err.raw_os_error() {
        Some(code) if code == ERROR_INVALID_PARAMETER as i32 => {
            // The exact USNs are not in the OS error; the driver re-checks the
            // watermark and re-reports the concrete numbers.
            JournalError::StartingPointTooOld { from: 0, first: 0 }
        }
        Some(code) if code == ERROR_JOURNAL_DELETE_IN_PROGRESS as i32 => {
            JournalError::JournalDeleted
        }
        Some(code) if code == ERROR_HANDLE_EOF as i32 => JournalError::CaughtUp,
        Some(code) if code == ERROR_JOURNAL_NOT_ACTIVE as i32 => {
            JournalError::Unavailable(format!("{op}: journal not active (os error {code})"))
        }
        Some(code) => JournalError::Unavailable(format!("{op}: os error {code} ({err})")),
        None => JournalError::Unavailable(format!("{op}: {err}")),
    }
}

/// `FSCTL_QUERY_USN_JOURNAL` → journal identity + watermarks.
fn query_journal(h: HANDLE) -> Result<USN_JOURNAL_DATA_V0, std::io::Error> {
    let mut data = USN_JOURNAL_DATA_V0::default();
    let mut bytes = 0u32;
    // SAFETY: output buffer is a valid USN_JOURNAL_DATA_V0; size matches.
    let ok = unsafe {
        DeviceIoControl(
            h,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            &mut data as *mut _ as *mut _,
            std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(data)
    }
}

/// `FSCTL_READ_USN_JOURNAL` — one deltas chunk starting at `usn`.
///
/// Returns `(records, next_usn)` where `next_usn` (the response header's
/// cursor field) is authoritative for the next call. A truncated tail record
/// is dropped by the parse layer and resumed from `next_usn` — progress, not a
/// gap.
fn read_journal_chunk(
    h: HANDLE,
    journal_id: u64,
    usn: u64,
    buffer: &mut [u8],
) -> Result<(Vec<UsnRawRecord>, u64), std::io::Error> {
    let mut input = READ_USN_JOURNAL_DATA_V0 {
        StartUsn: usn as i64,
        ReasonMask: u32::MAX, // read all reasons; the parse layer filters
        ReturnOnlyOnClose: 0, // surface records on every close, not only final
        Timeout: 0,
        BytesToWaitFor: 0,
        UsnJournalID: journal_id,
    };
    let mut bytes = 0u32;
    // SAFETY: input is a valid READ_USN_JOURNAL_DATA_V0; output buffer is
    // BUFFER_BYTES of writable memory.
    let ok = unsafe {
        DeviceIoControl(
            h,
            FSCTL_READ_USN_JOURNAL,
            &mut input as *mut _ as *mut _,
            std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
            buffer.as_mut_ptr() as *mut _,
            BUFFER_BYTES,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let (records, next_usn) = parse_record_stream(&buffer[..bytes as usize]);
    Ok((records, next_usn))
}

/// Parse the `FSCTL_ENUM_USN_DATA` output buffer: an `MFT_ENUM_DATA_V0`
/// header (16 bytes) followed by `USN_RECORD_V2` records. Returns the
/// records + the last FRN (the continuation point), or `(empty, start)`.
fn parse_enum_buffer(buf: &[u8]) -> (Vec<UsnRawRecord>, u64) {
    let (records, _) = parse_record_stream(buf);
    let last = records.last().map(|r| r.file_ref).unwrap_or(0);
    (records, last)
}

/// FRN → (name, parent FRN) index + assembled-path cache.
#[derive(Debug, Default)]
struct FrnIndex {
    entry_by_frn: HashMap<u64, (String, u64)>,
    path_cache: HashMap<u64, PathBuf>,
    volume: String,
}

impl FrnIndex {
    fn new(volume: &str) -> Self {
        Self {
            volume: volume.trim_end_matches('\\').to_string(),
            ..Default::default()
        }
    }

    /// Walk the MFT once (or from `start_frn`) to fill entries.
    fn enumerate(&mut self, h: HANDLE, mut start: u64) -> Result<(), std::io::Error> {
        let mut buffer = vec![0u8; BUFFER_BYTES as usize];
        loop {
            let mut input = MFT_ENUM_DATA_V0 {
                StartFileReferenceNumber: start,
                LowUsn: 0,
                HighUsn: i64::MAX,
            };
            let mut bytes = 0u32;
            // SAFETY: input/output follow the FSCTL_ENUM_USN_DATA contract.
            let ok = unsafe {
                DeviceIoControl(
                    h,
                    FSCTL_ENUM_USN_DATA,
                    &mut input as *const _ as *const _,
                    std::mem::size_of::<MFT_ENUM_DATA_V0>() as u32,
                    buffer.as_mut_ptr() as *mut _,
                    BUFFER_BYTES,
                    &mut bytes,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
                    return Ok(()); // normal end of enumeration
                }
                return Err(err);
            }
            let (records, next_frn) = parse_enum_buffer(&buffer[..bytes as usize]);
            if records.is_empty() || next_frn == start || next_frn == 0 {
                return Ok(()); // no progress → done
            }
            for rec in &records {
                self.entry_by_frn
                    .insert(rec.file_ref, (rec.name.clone(), rec.parent_ref));
            }
            start = next_frn;
        }
    }

    /// Resolve the full path of `file_ref` (walking parents). `None` when the
    /// chain is unknown — caller falls back to the raw name (honest).
    fn path_of(&mut self, file_ref: u64, name: &str) -> Option<PathBuf> {
        if let Some(p) = self.path_cache.get(&file_ref) {
            return Some(p.clone());
        }
        let mut chain_rev = vec![name];
        let mut frn = file_ref;
        let mut guard = 0;
        // Walk up: child → parent. Root FRN is 0.
        while frn != 0 {
            let (n, parent) = self.entry_by_frn.get(&frn)?;
            chain_rev.push(n.as_str());
            frn = *parent;
            guard += 1;
            if guard > 64 {
                return None; // cycle/runaway — never loop forever
            }
        }
        chain_rev.reverse();
        let p = assemble_path(&self.volume, &chain_rev);
        self.path_cache.insert(file_ref, p.clone());
        Some(p)
    }

    /// Learn FRNs from a read batch; the first read primes the whole MFT.
    fn learn(&mut self, h: HANDLE, records: &[UsnRawRecord]) -> Result<(), std::io::Error> {
        if self.entry_by_frn.is_empty() {
            self.enumerate(h, 0)?;
        }
        for rec in records {
            self.entry_by_frn
                .insert(rec.file_ref, (rec.name.clone(), rec.parent_ref));
        }
        Ok(())
    }
}

/// A live Windows USN journal reader bound to one volume.
///
/// Implements [`JournalReader`], so it plugs straight into
/// [`crate::usn::UsnDeltaSource`], which owns the cursor/epoch row and the gap
/// policy. The reader itself stays a thin, bounded ioctl seam.
///
/// ```no_run
/// use agentcowork_storage::usn::{DeltaOutcome, UsnDeltaSource};
/// use agentcowork_storage::usn_winapi::NtfsJournalReader;
///
/// fn main() -> Result<(), String> {
///     let reader = NtfsJournalReader::open("C:\\")?;
///     let mut source = UsnDeltaSource::fresh(reader);
///     match source.poll().map_err(|e| e.to_string())? {
///         DeltaOutcome::Applied { records, .. } => println!("{} deltas", records.len()),
///         DeltaOutcome::Idle => println!("no changes"),
///         // A gap forces a scoped rescan; it is never reported as "no changes".
///         DeltaOutcome::Gap(s) => eprintln!("gap {:?}: rescan {:?}", s.reason, s.rescan),
///     }
///     // Persist so a restart resumes from the same epoch + cursor.
///     source.state().save_to("cursor.json").map_err(|e| e.to_string())?;
///     Ok(())
/// }
/// ```
pub struct NtfsJournalReader {
    handle: HANDLE,
    journal_id: u64,
    volume: String,
    frn: FrnIndex,
    buffer: Vec<u8>,
}

impl NtfsJournalReader {
    /// Open the volume and query the journal identity. May fail when the
    /// volume has no journal yet (NTFS supports creating one via
    /// `FSCTL_CREATE_USN_JOURNAL` — out of scope here; the debounced-watch
    /// fallback covers that case).
    pub fn open(volume: &str) -> Result<Self, String> {
        let handle = open_volume(volume)?;
        match query_journal(handle) {
            Ok(info) => Ok(Self {
                handle,
                journal_id: info.UsnJournalID,
                volume: volume.to_string(),
                frn: FrnIndex::new(volume),
                buffer: vec![0u8; BUFFER_BYTES as usize],
            }),
            Err(e) => {
                // SAFETY: the handle is valid (we just opened it).
                unsafe { CloseHandle(handle) };
                Err(format!("FSCTL_QUERY_USN_JOURNAL failed: {e}"))
            }
        }
    }

    /// The journal id this reader is bound to — the epoch.
    pub fn journal_id(&self) -> u64 {
        self.journal_id
    }

    /// Read all delta records with `usn > last_usn`; returns
    /// `(records, next_usn)` — pass `next_usn` back on the next call.
    ///
    /// A thin, unbounded convenience wrapper kept for the existing API shape.
    /// Prefer [`JournalReader::read_from`] through
    /// [`crate::usn::UsnDeltaSource`]: it bounds the batch and turns failures
    /// into typed gap signals instead of opaque strings.
    pub fn read_since(&mut self, last_usn: u64) -> Result<(Vec<UsnRecord>, u64), String> {
        let (raws, next_usn) =
            read_journal_chunk(self.handle, self.journal_id, last_usn, &mut self.buffer)
                .map_err(|e| format!("FSCTL_READ_USN_JOURNAL failed: {e}"))?;
        if raws.is_empty() {
            return Ok((Vec::new(), next_usn));
        }
        self.frn
            .learn(self.handle, &raws)
            .map_err(|e| format!("FSCTL_ENUM_USN_DATA failed: {e}"))?;
        let out = self.resolve_records(raws);
        Ok((out, next_usn))
    }

    /// Resolve raw records into consumer records (path assembly + FRN).
    fn resolve_records(&mut self, raws: Vec<UsnRawRecord>) -> Vec<UsnRecord> {
        let mut out = Vec::with_capacity(raws.len());
        for raw in raws {
            let path = self
                .frn
                .path_of(raw.file_ref, &raw.name)
                .unwrap_or_else(|| {
                    // Unresolvable parent chain — raw name at the volume root.
                    PathBuf::from(format!("{}\\{}", self.frn.volume, raw.name))
                });
            out.push(UsnRecord {
                usn: raw.usn,
                reason: raw.reason,
                path,
                // The FRN is the event-side identity evidence; carrying it lets
                // a delta be matched to a file identity, not just a path.
                file_ref: Some(raw.file_ref),
            });
        }
        out
    }

    /// Refresh the journal watermarks, adopting a new journal id if the
    /// journal was recreated (which the driver turns into an epoch reset).
    fn refresh_watermarks(&mut self) -> Result<USN_JOURNAL_DATA_V0, JournalError> {
        let info = query_journal(self.handle)
            .map_err(|e| classify_os_error("FSCTL_QUERY_USN_JOURNAL", &e))?;
        self.journal_id = info.UsnJournalID;
        Ok(info)
    }
}

impl JournalReader for NtfsJournalReader {
    fn source(&self) -> JournalSource {
        JournalSource::Ntfs
    }

    fn scope(&self) -> String {
        self.volume.clone()
    }

    fn epoch(&self) -> u64 {
        self.journal_id
    }

    fn next_usn(&mut self) -> Result<u64, JournalError> {
        Ok(self.refresh_watermarks()?.NextUsn.max(0) as u64)
    }

    fn first_usn(&mut self) -> Result<u64, JournalError> {
        Ok(self.refresh_watermarks()?.FirstUsn.max(0) as u64)
    }

    /// One **bounded** read: at most `max_records` records, drained in 64 KiB
    /// chunks. Every failure is classified, so "caught up" and "history
    /// dropped" can never be confused (`REQ-FILES-004` / `REQ-WORLD-005`).
    fn read_from(&mut self, from: u64, max_records: usize) -> Result<JournalChunk, JournalError> {
        if max_records == 0 {
            return Ok(JournalChunk {
                records: Vec::new(),
                next_usn: from,
                exhausted: true,
            });
        }
        let mut out: Vec<UsnRecord> = Vec::new();
        let mut cursor = from;
        loop {
            let chunk =
                match read_journal_chunk(self.handle, self.journal_id, cursor, &mut self.buffer)
                    .map_err(|e| classify_os_error("FSCTL_READ_USN_JOURNAL", &e))
                {
                    Ok(c) => c,
                    // EOF after a partial drain is a drained batch, not a gap; EOF
                    // with nothing buffered is a legitimate "caught up".
                    Err(JournalError::CaughtUp) if !out.is_empty() => {
                        return Ok(JournalChunk {
                            records: out,
                            next_usn: cursor,
                            exhausted: true,
                        });
                    }
                    Err(e) => return Err(e),
                };
            let (raws, next_usn) = chunk;
            if raws.is_empty() {
                if out.is_empty() {
                    return Err(JournalError::CaughtUp);
                }
                return Ok(JournalChunk {
                    records: out,
                    next_usn,
                    exhausted: true,
                });
            }
            // A read that produced records is progress even if the header
            // cursor did not move; a repeated value must not spin the loop.
            if next_usn > cursor {
                cursor = next_usn;
            }
            self.frn
                .learn(self.handle, &raws)
                .map_err(|e| classify_os_error("FSCTL_ENUM_USN_DATA", &e))?;
            let last_usn = raws.last().map(|r| r.usn).unwrap_or(cursor);
            let previous_cursor = cursor;
            out.extend(self.resolve_records(raws));
            if out.len() >= max_records {
                return Ok(JournalChunk {
                    records: out,
                    next_usn: last_usn.max(previous_cursor),
                    exhausted: false,
                });
            }
            if last_usn <= previous_cursor {
                // No forward progress: stop rather than loop forever.
                return Ok(JournalChunk {
                    records: out,
                    next_usn: previous_cursor,
                    exhausted: true,
                });
            }
        }
    }
}

impl Drop for NtfsJournalReader {
    fn drop(&mut self) {
        // SAFETY: the handle is valid for the lifetime of this reader.
        unsafe { CloseHandle(self.handle) };
    }
}
