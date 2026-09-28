//! Platform file identity — incarnation-aware (FIX-10 / `TASK-FILES-002`).
//!
//! `ARCH/25-FILES.md` §2 and `ARCH/21-WORLD-MODEL.md` §3 (DM-026) require the
//! identity `(volume, file id, incarnation)` on Windows and `(st_dev, st_ino)`
//! extended with incarnation evidence on POSIX, so that a *reused* id after a
//! delete never resumes the old identity (`REQ-FILES-001`).
//!
//! **The defect this module replaces.** `walk.rs` used to answer `0` for
//! `dev` and `ino` on every non-Unix host, so every Windows scan produced the
//! same `(0, 0)` pair. `dedup` keys hardlink groups on `(dev, ino)`, so all
//! Windows files collapsed into one "physical copy" and `wasted_bytes`
//! reported `0`; `cleanup` then proposed `freed_bytes: 0` for every duplicate.
//! A zero is *not* an identity. Here an id the OS would not give us is
//! [`FileIdentity::unknown`], and every downstream consumer must treat unknown
//! as "not provably the same object" instead of "identical".
//!
//! **Metadata-first.** Nothing here reads file *content*: the POSIX model is
//! `stat(2)` fields, and the Windows model asks the kernel for the volume
//! serial + file id of an attribute-only handle
//! (`ARCH/21-WORLD-MODEL.md` §5.3).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Which platform's identity model produced a [`FileIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IdentityPlatform {
    /// `(st_dev, st_ino)` + `st_nlink` (`ARCH/25-FILES.md` §2).
    Posix,
    /// `(VolumeSerialNumber, FILE_ID_128)`, 64-bit file index + serial as the
    /// fallback (`ARCH/25-FILES.md` §2).
    Windows,
    /// Neither model — identity is always unknown on this platform.
    Other,
}

impl IdentityPlatform {
    /// The model of the platform this build was compiled for.
    pub const fn current() -> Self {
        if cfg!(windows) {
            IdentityPlatform::Windows
        } else if cfg!(unix) {
            IdentityPlatform::Posix
        } else {
            IdentityPlatform::Other
        }
    }

    /// Stable lowercase label (index/`provenance` keys).
    pub const fn as_str(self) -> &'static str {
        match self {
            IdentityPlatform::Posix => "posix",
            IdentityPlatform::Windows => "windows",
            IdentityPlatform::Other => "other",
        }
    }
}

/// Reuse evidence that separates two *different* files that happen to carry the
/// same OS file id (file ids are reused after a delete — DM-026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Incarnation {
    /// No discriminator could be obtained (a filesystem whose file id carries
    /// no sequence, or the identity query was refused). Two records that share
    /// an id can therefore **not** be confirmed as the same file; callers must
    /// guard with size/mtime and must never treat them as one object.
    Unknown,
    /// NTFS: the 64-bit file reference number is `sequence:16 | mft_index:48`,
    /// so a reused MFT entry gets a new sequence and the id itself is already
    /// incarnation-safe.
    FileSequence(u16),
    /// A filesystem whose file id has no sequence component (ReFS, FAT): the
    /// file's creation timestamp separates delete → recreate.
    CreationTime(u64),
    /// POSIX: `(st_dev, st_ino)` is the accepted model, but inodes *are*
    /// reused after delete, so a `Same` verdict on POSIX still requires the
    /// `REQ-FILES-006` size/mtime guard before a write.
    Posix,
}

impl Incarnation {
    /// The discriminator used inside a [`FileKey`], or `None` when there is
    /// none — a key without a discriminator is *not* proof of sameness.
    const fn key(self) -> Option<Discriminator> {
        match self {
            Incarnation::Unknown => None,
            Incarnation::FileSequence(s) => Some(Discriminator::Sequence(s)),
            Incarnation::CreationTime(t) => Some(Discriminator::Created(t)),
            Incarnation::Posix => Some(Discriminator::Posix),
        }
    }
}

/// The comparable form of [`Incarnation`] used inside a [`FileKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Discriminator {
    /// NTFS MFT sequence number.
    Sequence(u16),
    /// Creation timestamp (raw 100 ns `FILETIME`, or the POSIX equivalent).
    Created(u64),
    /// "This platform's id is the model, and a size/mtime guard is mandatory."
    Posix,
}

/// The comparable key of a *known* identity (`DM-026` `identity_key`).
///
/// Two records with equal keys are the same object. A record with **no** key
/// is unknown and must never be equated with anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileKey {
    pub platform: IdentityPlatform,
    pub volume: u64,
    pub file_id: u128,
    pub incarnation: Discriminator,
}

impl FileKey {
    /// Stable string form for index rows and `identity_key` columns.
    pub fn to_identity_key(self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.platform.as_str(),
            self.volume,
            self.file_id,
            match self.incarnation {
                Discriminator::Sequence(s) => format!("seq:{s}"),
                Discriminator::Created(t) => format!("ct:{t}"),
                Discriminator::Posix => "posix".to_string(),
            }
        )
    }
}

/// The result of comparing two identities (`REQ-FILES-001` acceptance:
/// rename/move preserves identity; delete + recreate does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityVerdict {
    /// Same volume, same file id, and the incarnation evidence agrees.
    Same,
    /// Different volume, different id, or *contradicting* incarnation
    /// evidence (the same id with a different sequence/creation stamp is a
    /// delete + recreate, not a rename).
    Different,
    /// The ids match but no incarnation evidence exists, so the records cannot
    /// be confirmed as the same file. Never upgrade this to [`Self::Same`].
    Indeterminate,
}

/// How hard [`identity_from_metadata`] should work.
///
/// [`IdentityPolicy::Full`] resolves the platform identity for every entry.
/// [`IdentityPolicy::MetadataOnly`] skips the per-entry identity query, which
/// is what an elevated MFT/`FSCTL_ENUM_USN_DATA` inventory wants: that
/// collector already *has* the id in the record, so a second query per entry
/// would be pure cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdentityPolicy {
    /// Resolve the real platform identity (the default; never fabricates one).
    #[default]
    Full,
    /// Metadata only — the identity is reported as unknown.
    MetadataOnly,
}

/// A file's platform identity, with explicit "unknown" instead of zeros.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    platform: IdentityPlatform,
    volume: u64,
    /// `None` ⇒ the OS would not give us an id. Never `Some(0)`: a file id of
    /// zero is not a valid id and must not become a colliding key.
    file_id: Option<u128>,
    incarnation: Incarnation,
    nlink: u32,
}

impl Default for FileIdentity {
    fn default() -> Self {
        Self::unknown(IdentityPlatform::current(), 1)
    }
}

impl FileIdentity {
    /// An identity the OS would not give us. Honest, and safe downstream:
    /// [`Self::key`] and [`Self::hardlink_key`] are `None`.
    pub const fn unknown(platform: IdentityPlatform, nlink: u32) -> Self {
        Self {
            platform,
            volume: 0,
            file_id: None,
            incarnation: Incarnation::Unknown,
            nlink,
        }
    }

    /// Build an identity from resolved parts (used by the platform layer and
    /// by tests). `file_id: None` marks it unknown.
    pub const fn new(
        platform: IdentityPlatform,
        volume: u64,
        file_id: Option<u128>,
        incarnation: Incarnation,
        nlink: u32,
    ) -> Self {
        Self {
            platform,
            volume,
            file_id,
            incarnation,
            nlink,
        }
    }

    /// Reconstruct an identity from the legacy `(dev, ino, nlink)` triple.
    ///
    /// `dev` carries `st_dev` (POSIX) or the volume serial number (Windows)
    /// and `ino` carries the 64-bit file index (POSIX `st_ino`, Windows
    /// `nFileIndexHigh:Low`). An `ino` of `0` is not a valid file id on either
    /// platform, and `(0, 0)` is exactly the shape the pre-`FIX-10` non-Unix
    /// branch produced, so it is reported as **unknown** — never as an id that
    /// every file could collide on.
    ///
    /// On Windows the recovered 64-bit index is treated as an NTFS file
    /// reference number, so its high 16 bits are read as the MFT sequence
    /// number. That is exact on NTFS (the v1 primary target) and, on a
    /// filesystem that needs the 128-bit id, `ino == 0` collapses to unknown —
    /// i.e. conservative, never wrong.
    pub fn from_legacy_parts(platform: IdentityPlatform, dev: u64, ino: u64, nlink: u32) -> Self {
        if ino == 0 {
            return Self::unknown(platform, nlink);
        }
        let incarnation = match platform {
            IdentityPlatform::Windows => Incarnation::FileSequence((ino >> 48) as u16),
            IdentityPlatform::Posix => Incarnation::Posix,
            IdentityPlatform::Other => Incarnation::Unknown,
        };
        Self {
            platform,
            volume: dev,
            file_id: Some(u128::from(ino)),
            incarnation,
            nlink,
        }
    }

    /// The identity of the platform this build runs for, from legacy parts.
    pub fn from_legacy(dev: u64, ino: u64, nlink: u32) -> Self {
        Self::from_legacy_parts(IdentityPlatform::current(), dev, ino, nlink)
    }

    /// The platform whose model produced this identity.
    pub const fn platform(self) -> IdentityPlatform {
        self.platform
    }

    /// The volume: `st_dev` (POSIX) or `VolumeSerialNumber` (Windows).
    pub const fn volume(self) -> u64 {
        self.volume
    }

    /// The raw file id (`st_ino` / `FILE_ID_128`), or `None` when unknown.
    pub const fn file_id(self) -> Option<u128> {
        self.file_id
    }

    /// The reuse discriminator observed for this file.
    pub const fn incarnation(self) -> Incarnation {
        self.incarnation
    }

    /// The hardlink count (`st_nlink` / `NumberOfLinks`).
    pub const fn nlink(self) -> u32 {
        self.nlink
    }

    /// `true` when the OS gave us a real file id.
    pub const fn is_known(self) -> bool {
        self.file_id.is_some()
    }

    /// `true` when the identity is unknown.
    pub const fn is_unknown(self) -> bool {
        self.file_id.is_none()
    }

    /// The index/lease key, or `None` when the identity is unknown.
    pub fn key(self) -> Option<FileKey> {
        let file_id = self.file_id?;
        let incarnation = self.incarnation.key()?;
        Some(FileKey {
            platform: self.platform,
            volume: self.volume,
            file_id,
            incarnation,
        })
    }

    /// The key that proves "these two paths are the *same physical file*".
    ///
    /// `None` for two distinct reasons, both of which a caller must read as
    /// "do not claim these are the same object":
    /// 1. the identity is unknown; or
    /// 2. `nlink <= 1` — a file with a single link provably has no hardlink
    ///    twin, so it *is* its own physical copy.
    pub fn hardlink_key(self) -> Option<FileKey> {
        if self.nlink <= 1 {
            return None;
        }
        self.key()
    }

    /// The legacy `(dev, ino, nlink)` projection, kept for the record shapes
    /// that predate [`FileIdentity`]. `ino` is `0` iff the identity is
    /// unknown — never a fabricated id.
    pub fn legacy_parts(self) -> (u64, u64, u32) {
        (
            self.volume,
            self.file_id.map(|f| f as u64).unwrap_or(0),
            self.nlink,
        )
    }

    /// Compare two identities (`REQ-FILES-001` / `REQ-WORLD-003`).
    ///
    /// * `Same` — same volume, same id, agreeing incarnation evidence. On
    ///   POSIX this still needs the `REQ-FILES-006` size/mtime guard before a
    ///   write, because inodes are reused after delete.
    /// * `Different` — a different volume or id, *or* the same id with
    ///   contradicting incarnation evidence (delete + recreate).
    /// * `Indeterminate` — the ids match but there is no incarnation evidence.
    pub fn same_file(self, other: Self) -> IdentityVerdict {
        let (Some(a), Some(b)) = (self.file_id, other.file_id) else {
            // At least one side has no id: we can neither confirm nor refute
            // sameness, and a missing id must never be read as "different"
            // (that would drop live objects) nor as "same" (that would merge
            // a delete + recreate).
            return IdentityVerdict::Indeterminate;
        };
        if self.platform != other.platform || self.volume != other.volume {
            return IdentityVerdict::Different;
        }
        if a != b {
            return IdentityVerdict::Different;
        }
        match (self.incarnation.key(), other.incarnation.key()) {
            (None, _) | (_, None) => IdentityVerdict::Indeterminate,
            (Some(x), Some(y)) if x == y => IdentityVerdict::Same,
            (Some(_), Some(_)) => IdentityVerdict::Different,
        }
    }
}

/// Resolve the platform identity of one filesystem entry from metadata the
/// caller already holds.
///
/// * POSIX — `(st_dev, st_ino, st_nlink)` straight from `stat(2)`.
/// * Windows — `std::fs::Metadata` does **not** expose the volume serial or the
///   file id through stable APIs (`volume_serial_number` / `file_index` are
///   still `#[unstable]`), so an attribute-only handle is opened for the entry
///   and asked with `GetFileInformationByHandleEx(FileIdInfo)`, falling back to
///   `GetFileInformationByHandle` (`ARCH/25-FILES.md` §2). The handle asks for
///   `FILE_READ_ATTRIBUTES` only, so no file content is ever read.
/// * Anything else — [`FileIdentity::unknown`], explicitly.
///
/// A failed identity query yields [`FileIdentity::unknown`]; it never yields
/// zeros dressed up as an id.
pub fn identity_from_metadata(
    path: &Path,
    meta: &std::fs::Metadata,
    policy: IdentityPolicy,
) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = (path, policy);
        FileIdentity::new(
            IdentityPlatform::Posix,
            meta.dev(),
            Some(u128::from(meta.ino())),
            Incarnation::Posix,
            meta.nlink() as u32,
        )
    }
    #[cfg(windows)]
    {
        windows::file_identity(path, meta, policy)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        let _ = policy;
        FileIdentity::unknown(IdentityPlatform::Other, 1)
    }
}

/// Resolve the platform identity of the file represented by an already-open
/// handle and metadata obtained from that same handle.
///
/// This differs from [`identity_from_metadata`]: on Windows it queries the
/// supplied handle directly instead of reopening `path`. Callers validating a
/// file immediately before mutation should use this function so a path swap
/// cannot make the identity query describe a different file than the handle
/// they are about to modify.
pub fn identity_from_file(file: &std::fs::File, meta: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        FileIdentity::new(
            IdentityPlatform::Posix,
            meta.dev(),
            Some(u128::from(meta.ino())),
            Incarnation::Posix,
            meta.nlink() as u32,
        )
    }
    #[cfg(windows)]
    {
        windows::file_identity_from_file(file, meta)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, meta);
        FileIdentity::unknown(IdentityPlatform::Other, 1)
    }
}

#[cfg(windows)]
mod windows {
    //! The Windows identity query: volume serial + file id, metadata only.
    //!
    //! Preferred path is `GetFileInformationByHandleEx(FileIdInfo)`
    //! (`ARCH/25-FILES.md` §2 names `FileIdInfo`); the 64-bit index + serial
    //! fallback is `GetFileInformationByHandle`, whose `nFileIndexHigh:Low` is
    //! the NTFS file reference number (sequence in the high 16 bits).

    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx, OPEN_EXISTING,
    };

    use super::{FileIdentity, IdentityPlatform, IdentityPolicy, Incarnation};

    /// The raw 100 ns `FILETIME` creation stamp, as a reuse discriminator.
    const fn filetime_value(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    }

    /// Ask the kernel for `path`'s identity. `None` ⇒ unknown, never zeros.
    pub(super) fn file_identity(
        path: &Path,
        meta: &std::fs::Metadata,
        policy: IdentityPolicy,
    ) -> FileIdentity {
        if policy == IdentityPolicy::MetadataOnly {
            return FileIdentity::unknown(IdentityPlatform::Windows, 1);
        }
        // `std` has no stable accessor for these, so take the handle ourselves.
        let Some(handle) = open_attributes_handle(path) else {
            return FileIdentity::unknown(IdentityPlatform::Windows, 1);
        };
        // SAFETY: `handle` is a live handle we own; every buffer below is a
        // correctly sized, correctly aligned value we own.
        let id = unsafe { query_identity(handle, meta) };
        // SAFETY: the handle is valid and closed exactly once here.
        unsafe { CloseHandle(handle) };
        id
    }

    /// Query the identity of the already-open handle without resolving its
    /// path again. The caller retains ownership of the handle.
    pub(super) fn file_identity_from_file(
        file: &std::fs::File,
        meta: &std::fs::Metadata,
    ) -> FileIdentity {
        use std::os::windows::io::AsRawHandle;

        // std::fs::File owns this live handle for the duration of the call.
        // `query_identity` only asks the kernel for metadata and does not close
        // it.
        let handle = file.as_raw_handle() as HANDLE;
        // SAFETY: the handle is borrowed from a live File and query_identity
        // performs read-only metadata queries on it.
        unsafe { query_identity(handle, meta) }
    }

    /// `CreateFileW` with `FILE_READ_ATTRIBUTES` only — no data access, so no
    /// file content can be read through this handle.
    fn open_attributes_handle(path: &Path) -> Option<HANDLE> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; null
        // security attributes and no template file.
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(h)
        }
    }

    /// SAFETY: `handle` must be a live handle owned by the caller.
    unsafe fn query_identity(handle: HANDLE, meta: &std::fs::Metadata) -> FileIdentity {
        // Preferred: FileIdInfo — the 128-bit id the v1 identity model names.
        let mut info = FILE_ID_INFO::default();
        // SAFETY: `info` is a correctly sized FILE_ID_INFO.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        };
        if ok != 0 {
            let file_id = u128::from_le_bytes(info.FileId.Identifier);
            if file_id != 0 {
                let volume = info.VolumeSerialNumber;
                // The link count is not part of FILE_ID_INFO; the std
                // accessor is unstable, so the documented fallback supplies it.
                // SAFETY: `handle` is a live handle owned by the caller.
                let nlink = unsafe { by_handle_info(handle) }
                    .map(|b| b.nNumberOfLinks)
                    .filter(|&n| n > 0)
                    .unwrap_or(1);
                let incarnation = if info.FileId.Identifier[8..] == [0u8; 8] {
                    // NTFS: the 128-bit id is the 64-bit FRN in the low half,
                    // whose high 16 bits are the MFT sequence number.
                    Incarnation::FileSequence((file_id >> 48) as u16)
                } else {
                    // ReFS / FAT: no sequence in the id → creation time.
                    Incarnation::CreationTime(filetime_value(by_handle_time(meta, handle)))
                };
                return FileIdentity::new(
                    IdentityPlatform::Windows,
                    volume,
                    Some(file_id),
                    incarnation,
                    nlink,
                );
            }
        }

        // Fallback: 64-bit index + volume serial (ARCH/25-FILES.md §2).
        // SAFETY: `handle` is a live handle owned by the caller.
        if let Some(b) = unsafe { by_handle_info(handle) } {
            let frn = ((b.nFileIndexHigh as u64) << 32) | (b.nFileIndexLow as u64);
            if frn != 0 {
                return FileIdentity::new(
                    IdentityPlatform::Windows,
                    b.dwVolumeSerialNumber as u64,
                    Some(u128::from(frn)),
                    Incarnation::FileSequence((frn >> 48) as u16),
                    b.nNumberOfLinks.max(1),
                );
            }
        }

        // Neither query produced an id — report it honestly. `nlink` stays 1
        // (unknown) so nothing downstream may group this record as a hardlink.
        let _ = meta;
        FileIdentity::unknown(IdentityPlatform::Windows, 1)
    }

    /// SAFETY: `handle` must be a live handle owned by the caller.
    unsafe fn by_handle_info(handle: HANDLE) -> Option<BY_HANDLE_FILE_INFORMATION> {
        let mut b = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `b` is a correctly sized BY_HANDLE_FILE_INFORMATION.
        let ok = unsafe { GetFileInformationByHandle(handle, &mut b) };
        (ok != 0).then_some(b)
    }

    /// The creation stamp: the handle's own `FILETIME`, falling back to the
    /// std metadata value (same 100 ns units, both from `CreationTime`).
    fn by_handle_time(meta: &std::fs::Metadata, handle: HANDLE) -> FILETIME {
        // SAFETY: `handle` is a live handle owned by the caller.
        if let Some(b) = unsafe { by_handle_info(handle) } {
            return b.ftCreationTime;
        }
        // `std`'s `creation_time` is 100 ns since 1601 — same units.
        FILETIME {
            dwLowDateTime: meta.creation_time() as u32,
            dwHighDateTime: (meta.creation_time() >> 32) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posix(dev: u64, ino: u64, nlink: u32) -> FileIdentity {
        FileIdentity::new(
            IdentityPlatform::Posix,
            dev,
            Some(u128::from(ino)),
            Incarnation::Posix,
            nlink,
        )
    }

    fn ntfs(volume: u64, frn: u64, nlink: u32) -> FileIdentity {
        FileIdentity::new(
            IdentityPlatform::Windows,
            volume,
            Some(u128::from(frn)),
            Incarnation::FileSequence((frn >> 48) as u16),
            nlink,
        )
    }

    // --- identity semantics (platform independent) --------------------------

    #[test]
    fn unknown_identity_has_no_key_and_no_hardlink_key() {
        let u = FileIdentity::unknown(IdentityPlatform::Windows, 2);
        assert!(u.is_unknown());
        assert!(!u.is_known());
        assert_eq!(u.key(), None);
        assert_eq!(u.hardlink_key(), None);
        // A zeroed legacy projection, never a fabricated id.
        assert_eq!(u.legacy_parts(), (0, 0, 2));
    }

    #[test]
    fn zeroed_legacy_triple_is_unknown_not_a_colliding_id() {
        // The exact shape the pre-FIX-10 non-Unix walker branch produced.
        let u = FileIdentity::from_legacy(0, 0, 1);
        assert!(u.is_unknown());
        assert_eq!(u.key(), None);
        // And two such records must never be declared the same object.
        assert_eq!(u.same_file(u), IdentityVerdict::Indeterminate);
    }

    #[test]
    fn ino_zero_alone_is_unknown() {
        // Windows/ReFS needs the 128-bit id; a 0 low word is not an id.
        let u = FileIdentity::from_legacy_parts(IdentityPlatform::Windows, 0xABCD, 0, 1);
        assert!(u.is_unknown());
    }

    #[test]
    fn rename_move_preserves_identity() {
        // Same id, different path: identity survives (volume + id + evidence).
        let a = ntfs(0x1111, 0x0002_0000_0000_0042, 1);
        let b = ntfs(0x1111, 0x0002_0000_0000_0042, 1);
        assert_eq!(a.same_file(b), IdentityVerdict::Same);
    }

    #[test]
    fn delete_recreate_with_reused_mft_slot_is_a_different_file() {
        // Same MFT index, new sequence number → delete + recreate, not a rename.
        let before = ntfs(0x1111, 0x0001_0000_0000_0042, 1);
        let after = ntfs(0x1111, 0x0002_0000_0000_0042, 1);
        assert_eq!(before.incarnation(), Incarnation::FileSequence(1));
        assert_eq!(after.incarnation(), Incarnation::FileSequence(2));
        assert_eq!(before.same_file(after), IdentityVerdict::Different);
        assert_ne!(before.key(), after.key());
    }

    #[test]
    fn different_volume_is_always_different() {
        let a = ntfs(0x1111, 0x0001_0000_0000_0042, 1);
        let b = ntfs(0x2222, 0x0001_0000_0000_0042, 1);
        assert_eq!(a.same_file(b), IdentityVerdict::Different);
    }

    #[test]
    fn missing_incarnation_evidence_is_indeterminate_not_same() {
        // ReFS-style: the same 128-bit id with no sequence component.
        let a = FileIdentity::new(
            IdentityPlatform::Windows,
            7,
            Some(99),
            Incarnation::Unknown,
            1,
        );
        let b = FileIdentity::new(
            IdentityPlatform::Windows,
            7,
            Some(99),
            Incarnation::Unknown,
            1,
        );
        assert_eq!(a.same_file(b), IdentityVerdict::Indeterminate);
        assert_eq!(a.key(), None, "no discriminator → no key");
    }

    #[test]
    fn creation_time_separates_reuse_on_sequence_less_filesystems() {
        let a = FileIdentity::new(
            IdentityPlatform::Windows,
            7,
            Some(99),
            Incarnation::CreationTime(1_000),
            1,
        );
        let b = FileIdentity::new(
            IdentityPlatform::Windows,
            7,
            Some(99),
            Incarnation::CreationTime(2_000),
            1,
        );
        assert_eq!(a.same_file(b), IdentityVerdict::Different);
        let c = FileIdentity::new(
            IdentityPlatform::Windows,
            7,
            Some(99),
            Incarnation::CreationTime(1_000),
            1,
        );
        assert_eq!(a.same_file(c), IdentityVerdict::Same);
    }

    #[test]
    fn single_link_file_is_its_own_physical_copy() {
        // nlink == 1 provably has no hardlink twin, so it must not group.
        let a = posix(1, 100, 1);
        let b = posix(1, 100, 1);
        assert_eq!(a.key(), b.key());
        assert_eq!(a.hardlink_key(), None);
        assert_eq!(b.hardlink_key(), None);
    }

    #[test]
    fn hardlinked_paths_share_a_hardlink_key() {
        let a = posix(1, 100, 2);
        let b = posix(1, 100, 2);
        assert_eq!(a.hardlink_key(), b.hardlink_key());
        assert!(a.hardlink_key().is_some());
    }

    #[test]
    fn legacy_round_trip_keeps_the_key() {
        let id = ntfs(0x1111, 0x0002_0000_0000_0042, 2);
        let (dev, ino, nlink) = id.legacy_parts();
        let back = FileIdentity::from_legacy_parts(IdentityPlatform::Windows, dev, ino, nlink);
        assert_eq!(id.key(), back.key());
        assert_eq!(id.hardlink_key(), back.hardlink_key());
        assert_eq!(back.incarnation(), Incarnation::FileSequence(2));
    }

    #[test]
    fn identity_key_string_is_stable_and_distinguishing() {
        let a = ntfs(0x1111, 0x0002_0000_0000_0042, 1);
        let b = ntfs(0x1111, 0x0003_0000_0000_0042, 1);
        let ka = a.key().unwrap().to_identity_key();
        assert_eq!(ka, a.key().unwrap().to_identity_key());
        assert_ne!(ka, b.key().unwrap().to_identity_key());
        assert!(ka.starts_with("windows:4369:"), "{ka}");
    }

    // --- live filesystem resolution -----------------------------------------

    #[test]
    fn resolves_real_identity_from_the_filesystem() {
        let dir = std::env::temp_dir().join(format!("agentcowork-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, b"hello").unwrap();

        let meta = std::fs::symlink_metadata(&file).unwrap();
        let id = identity_from_metadata(&file, &meta, IdentityPolicy::Full);
        // Whatever the platform, a real file must never come back as an
        // all-zero fabricated identity.
        assert!(id.is_known(), "identity should resolve: {id:?}");
        assert!(id.volume() != 0 || id.file_id().is_some());
        assert_eq!(id.nlink(), 1);
        assert_eq!(id.hardlink_key(), None, "nlink == 1 → no twin");
        assert!(id.key().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opened_handle_identity_matches_path_identity() {
        let dir =
            std::env::temp_dir().join(format!("agentcowork-id-handle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, b"handle identity").unwrap();

        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let metadata = file.metadata().unwrap();
        let from_handle = identity_from_file(&file, &metadata);
        let from_path = identity_from_metadata(&path, &metadata, IdentityPolicy::Full);

        assert!(
            from_handle.is_known(),
            "handle identity should resolve: {from_handle:?}"
        );
        assert_eq!(from_handle.key(), from_path.key());
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_only_policy_reports_unknown_instead_of_inventing() {
        let dir = std::env::temp_dir().join(format!("agentcowork-id-mo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, b"hello").unwrap();
        let meta = std::fs::symlink_metadata(&file).unwrap();

        let skipped = identity_from_metadata(&file, &meta, IdentityPolicy::MetadataOnly);
        // On POSIX the id comes from stat(2), which the policy does not skip;
        // where the policy does skip a query the result must be unknown, never
        // zeros that a downstream key could collide on.
        if !cfg!(unix) {
            assert!(skipped.is_unknown());
            assert_eq!(skipped.legacy_parts(), (0, 0, 1));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_known_id_never_collides_with_an_unknown_one() {
        let known = posix(1, 100, 1);
        let unknown = FileIdentity::unknown(IdentityPlatform::Posix, 1);
        // Unknown never becomes "identical" to a real object, and never
        // masquerades as a different object either.
        assert_eq!(known.same_file(unknown), IdentityVerdict::Indeterminate);
        assert_eq!(unknown.same_file(known), IdentityVerdict::Indeterminate);
        assert_ne!(known.key(), unknown.key());
    }

    #[test]
    fn unknown_records_never_share_a_hardlink_key() {
        let a = FileIdentity::unknown(IdentityPlatform::Other, 4);
        let b = FileIdentity::unknown(IdentityPlatform::Other, 4);
        // Two unknowns must not collapse into one "physical copy".
        assert_eq!(a.hardlink_key(), None);
        assert_eq!(b.hardlink_key(), None);
    }
}
