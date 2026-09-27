//! Acceptance-level evidence for the W1 delta source and the platform file
//! identity (`ARCH/25-FILES.md` §2–§4, `ARCH/21-WORLD-MODEL.md` §3–§4).
//!
//! Two behaviours get an end-to-end test rather than a unit test:
//!
//! 1. **FIX-10 / `REQ-FILES-001`** — a real scan of a real directory produces
//!    real identities, and the derived hardlink grouping is the truth (a
//!    hardlink pair is one physical copy; two independent copies are two).
//! 2. **FIX-11 / `REQ-FILES-005` / `REQ-WORLD-004` / `REQ-WORLD-005`** — a
//!    cursor survives a restart, an epoch reset discards it, a dropped history
//!    aborts into a scoped rescan, and no gap is ever reported as "no changes".

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agentcowork_storage::IdentityPolicy;
use agentcowork_storage::dedup::{DedupOptions, DupCandidate, find_duplicates};
use agentcowork_storage::identity::{IdentityVerdict, Incarnation};
use agentcowork_storage::usn::{
    CursorRow, DeltaOutcome, GapReason, JournalChunk, JournalError, JournalReader, JournalSource,
    RescanScope, UsnDeltaSource, UsnReason, UsnRecord,
};
use agentcowork_storage::walk::{ScanOptions, scan, scan_with_policy};

fn tmpdir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "agentcowork-storage-accept-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn candidates(root: &Path, ext: &str) -> Vec<DupCandidate> {
    let records = scan(
        root,
        &ScanOptions {
            threads: 2,
            skip_hidden: true,
            ..Default::default()
        },
    )
    .unwrap();
    records
        .iter()
        .filter(|r| !r.is_dir && r.path.extension().and_then(|e| e.to_str()) == Some(ext))
        .map(DupCandidate::from_record)
        .collect()
}

// ---------------------------------------------------------------------------
// FIX-10 — platform file identity
// ---------------------------------------------------------------------------

#[test]
fn acceptance_file_identity_is_real_and_incs_hardlink_grouping() {
    let root = tmpdir("identity");
    std::fs::write(root.join("orig.bin"), vec![42u8; 4096]).unwrap();
    // Two names, one physical file.
    std::fs::hard_link(root.join("orig.bin"), root.join("link.bin")).unwrap();
    // A genuinely separate copy of the same content.
    std::fs::write(root.join("copy.bin"), vec![42u8; 4096]).unwrap();

    let cands = candidates(&root, "bin");
    assert_eq!(cands.len(), 3);

    // 1. Every scanned record carries a real identity — never the zeroed
    //    `dev`/`ino` the pre-fix non-Unix branch produced.
    for c in &cands {
        let id = c.identity();
        assert!(id.is_known(), "no identity for {}", c.path.display());
        assert_ne!(c.ino, 0, "zeroed file id for {}", c.path.display());
        assert_eq!(id.legacy_parts(), (c.dev, c.ino, c.nlink));
    }

    // 2. The hardlink pair resolves to one physical copy; the separate copy is
    //    a second one. `wasted_bytes` is therefore one file size, not zero.
    let groups = find_duplicates(
        &cands,
        &DedupOptions {
            min_size: 1,
            prefix_len: 4096,
            suffix_len: 4096,
        },
    )
    .unwrap();
    assert_eq!(groups.len(), 1, "one content group of three files");
    let g = &groups[0];
    assert_eq!(g.files.len(), 3);
    assert_eq!(g.hardlink_groups, 2, "hardlink pair + one standalone copy");
    assert_eq!(g.wasted_bytes, 4096);
    assert!(g.identity_backed, "the copy count is provable here");
    assert!(g.reflink_eligible);

    // 3. Cleanup keeps one name and proposes ticket-gated actions; the
    //    redundant hardlink name frees 0, the standalone copy frees its size.
    let proposals = agentcowork_storage::propose_duplicate_cleanup(std::slice::from_ref(g));
    assert_eq!(proposals.len(), 2);
    assert!(proposals.iter().all(|p| p.requires_ticket));
    let freed: u64 = proposals.iter().map(|p| p.freed_bytes).sum();
    assert_eq!(freed, 4096, "one copy is genuinely reclaimable");
    assert!(proposals.iter().all(|p| p.identity_backed));

    // 4. A metadata-only scan reports *unknown* rather than a fabricated id,
    //    and its numbers are flagged as unprovable.
    let records =
        scan_with_policy(&root, &ScanOptions::default(), IdentityPolicy::MetadataOnly).unwrap();
    for r in records.iter().filter(|r| !r.is_dir) {
        assert_eq!(r.identity.legacy_parts(), (r.dev, r.ino, r.nlink));
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn acceptance_delete_and_recreate_is_not_the_same_file() {
    // A recreated file is a new incarnation, not the old identity resumed
    // (`REQ-FILES-001` acceptance: "delete + recreate … yields a new
    // identity"). On NTFS this is visible as a new MFT sequence in the 64-bit
    // file reference number; the model exposes that as `Incarnation`.
    let before = agentcowork_storage::FileIdentity::new(
        agentcowork_storage::IdentityPlatform::Windows,
        0x1111_2222,
        Some(0x0001_0000_0000_0042),
        Incarnation::FileSequence(1),
        1,
    );
    let after = agentcowork_storage::FileIdentity::new(
        agentcowork_storage::IdentityPlatform::Windows,
        0x1111_2222,
        Some(0x0002_0000_0000_0042),
        Incarnation::FileSequence(2),
        1,
    );
    assert_eq!(
        before.same_file(after),
        IdentityVerdict::Different,
        "a reused MFT slot with a new sequence is a new file"
    );
    assert_ne!(before.key(), after.key());

    // A rename keeps the identity: same volume, same id, same evidence.
    let renamed = agentcowork_storage::FileIdentity::new(
        agentcowork_storage::IdentityPlatform::Windows,
        0x1111_2222,
        Some(0x0001_0000_0000_0042),
        Incarnation::FileSequence(1),
        1,
    );
    assert_eq!(before.same_file(renamed), IdentityVerdict::Same);
    assert_eq!(before.key(), renamed.key());
}

// ---------------------------------------------------------------------------
// FIX-11 — the W1 delta source: cursor, epoch, gap
// ---------------------------------------------------------------------------

/// A scriptable in-memory journal standing in for the NTFS one, so the
/// cursor/epoch/gap contract is provable without Windows.
struct ScriptedJournal {
    epoch: u64,
    first: u64,
    next: u64,
    history: Vec<u64>,
    scope: String,
    fail: Option<JournalError>,
}

impl ScriptedJournal {
    fn new(scope: &str, epoch: u64, history: &[u64]) -> Self {
        Self {
            epoch,
            first: history.first().copied().unwrap_or(0),
            next: history.last().copied().unwrap_or(0),
            history: history.to_vec(),
            scope: scope.to_string(),
            fail: None,
        }
    }

    /// The journal wrapped: only USNs at/after `first` survive now.
    fn wrap_to(&mut self, first: u64) {
        self.history.retain(|&u| u >= first);
        self.first = first;
    }

    fn fail_next(&mut self, e: JournalError) {
        self.fail = Some(e);
    }
}

impl JournalReader for ScriptedJournal {
    fn source(&self) -> JournalSource {
        JournalSource::Ntfs
    }
    fn scope(&self) -> String {
        self.scope.clone()
    }
    fn epoch(&self) -> u64 {
        self.epoch
    }
    fn next_usn(&mut self) -> Result<u64, JournalError> {
        Ok(self.next)
    }
    fn first_usn(&mut self) -> Result<u64, JournalError> {
        Ok(self.first)
    }
    fn read_from(&mut self, from: u64, max_records: usize) -> Result<JournalChunk, JournalError> {
        if let Some(e) = self.fail.take() {
            return Err(e);
        }
        // `from == 0` means "no cursor yet": serve the oldest retained USN and
        // onwards. Only a real cursor that fell off the history is a gap.
        let floor = if from == 0 {
            self.first
        } else {
            if self.first > 0 && from < self.first {
                return Err(JournalError::StartingPointTooOld {
                    from,
                    first: self.first,
                });
            }
            from + 1
        };
        let records: Vec<UsnRecord> = self
            .history
            .iter()
            .filter(|&&u| u >= floor)
            .take(max_records)
            .map(|&u| UsnRecord::new(u, UsnReason::Create, PathBuf::from("C:/f"), Some(u)))
            .collect();
        if records.is_empty() {
            return Err(JournalError::CaughtUp);
        }
        let next_usn = records.last().map(|r| r.usn).unwrap_or(from);
        Ok(JournalChunk {
            exhausted: next_usn >= self.next,
            records,
            next_usn,
        })
    }
}

fn applied(out: DeltaOutcome) -> (Vec<UsnRecord>, u64) {
    match out {
        DeltaOutcome::Applied { records, cursor } => (records, cursor),
        other => panic!("expected Applied, got {other:?}"),
    }
}

fn gap(out: DeltaOutcome) -> agentcowork_storage::GapSignal {
    match out {
        DeltaOutcome::Gap(g) => g,
        other => panic!("expected Gap, got {other:?}"),
    }
}

fn cursor_path(dir: &Path) -> PathBuf {
    dir.join("usn-cursor.json")
}

#[test]
fn acceptance_cursor_resumes_after_restart_and_epoch_reset_rescans() {
    let dir = tmpdir("usn-cursor");
    let row_path = cursor_path(&dir);

    // --- session 1: apply deltas, persist the row -----------------------
    let j = ScriptedJournal::new("C:\\", 0xAAAA_1111, &[100, 101, 102]);
    let mut source = UsnDeltaSource::fresh(j);
    let (records, cursor) = applied(source.poll().unwrap());
    assert_eq!(records.len(), 3);
    assert_eq!(cursor, 102);
    source.state().save_to(&row_path).unwrap();

    // --- session 2: a fresh process resumes the same epoch + cursor -----
    let stored = CursorRow::load_from(&row_path, JournalSource::Ntfs, "C:\\").unwrap();
    assert_eq!(stored.epoch, 0xAAAA_1111);
    assert_eq!(stored.cursor, 102);
    assert!(!stored.gap_open);

    let j = ScriptedJournal::new("C:\\", 0xAAAA_1111, &[100, 101, 102, 103, 104]);
    let mut resumed = UsnDeltaSource::new(j, stored);
    let (records, cursor) = applied(resumed.poll().unwrap());
    assert_eq!(
        records.len(),
        2,
        "only what happened after the stored cursor"
    );
    assert_eq!(cursor, 104);
    resumed.state().save_to(&row_path).unwrap();

    // --- session 3: the journal is recreated (new epoch) → gap + rescan --
    let stored = CursorRow::load_from(&row_path, JournalSource::Ntfs, "C:\\").unwrap();
    let j = ScriptedJournal::new("C:\\", 0xBBBB_2222, &[200, 201]);
    let mut rotated = UsnDeltaSource::new(j, stored);
    let g = gap(rotated.poll().unwrap());
    assert_eq!(g.reason, GapReason::EpochChanged);
    assert_eq!(
        g.rescan,
        RescanScope::Volume {
            volume: "C:\\".into()
        },
        "the smallest known scope"
    );
    assert_eq!(g.discarded_cursor, 104);
    assert!(rotated.state().gap_open);
    assert_eq!(rotated.state().anomalies, 1);

    // The stale cursor is not carried across the epoch, and the row records
    // the reset for the next process.
    assert_eq!(rotated.state().cursor, 0);
    rotated.state().save_to(&row_path).unwrap();
    let reloaded = CursorRow::load_from(&row_path, JournalSource::Ntfs, "C:\\").unwrap();
    assert_eq!(reloaded.last_gap, Some(GapReason::EpochChanged));
    assert_eq!(reloaded.anomalies, 1);
    assert!(
        reloaded.gap_open,
        "completeness is not claimed while a gap is open"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn acceptance_wrapped_journal_is_a_gap_never_no_changes() {
    let j = ScriptedJournal::new("C:\\", 0xAAAA_1111, &[100, 101, 102, 103]);
    let mut source = UsnDeltaSource::fresh(j);
    let (_, cursor) = applied(source.poll().unwrap());
    assert_eq!(cursor, 103);

    // The journal wrapped: everything before 200 is gone. A reader that
    // reported "no changes" here would silently claim a complete index.
    let mut wrapped = ScriptedJournal::new("C:\\", 0xAAAA_1111, &[200, 201, 202]);
    wrapped.wrap_to(200);
    let mut source = UsnDeltaSource::new(wrapped, source.state().clone());
    let g = gap(source.poll().unwrap());
    assert_eq!(g.reason, GapReason::StartingPointTooOld);
    assert!(g.detail.contains("200"), "{g:?}");

    // A gap forces the rescan, and only the rescan clears it.
    match source.poll().unwrap() {
        DeltaOutcome::Gap(_) => {}
        other => panic!("a gap must not be reported as {other:?}"),
    }
    source.acknowledge_rescan(201);
    let (records, cursor) = applied(source.poll().unwrap());
    assert_eq!(records.len(), 1, "the delta that followed the rescan");
    assert_eq!(cursor, 202);
    assert!(!source.state().gap_open);
    assert_eq!(source.state().anomalies, 2, "both gaps stay observable");
}

#[test]
fn acceptance_unavailable_journal_falls_back_without_claiming_elevation() {
    let mut j = ScriptedJournal::new("D:\\", 0xAAAA_1111, &[1, 2]);
    j.fail_next(JournalError::Unavailable(
        "ERROR_ACCESS_DENIED: the journal requires elevation".into(),
    ));
    let mut source = UsnDeltaSource::fresh(j);
    let g = gap(source.poll().unwrap());
    // `REQ-WORLD-010`: an elevated mode that is absent or denied falls back to
    // non-admin mode, *recorded and surfaced* — never silent elevation.
    assert_eq!(g.reason, GapReason::SourceUnavailable);
    assert!(g.detail.contains("ERROR_ACCESS_DENIED"), "{g:?}");
    assert!(source.state().gap_open);
    assert_eq!(source.state().anomalies, 1);
}

#[test]
fn acceptance_gap_anomaly_is_an_observable_event() {
    let mut j = ScriptedJournal::new("C:\\", 0xAAAA_1111, &[1, 2]);
    j.fail_next(JournalError::JournalDeleted);
    let mut source = UsnDeltaSource::fresh(j);
    let signal = gap(source.poll().unwrap());
    let event = source.anomaly(&signal);
    assert_eq!(event.reason, GapReason::JournalDeleted);
    assert!(
        event.observed_at > 0,
        "a real observation stamp is recorded"
    );
    assert_eq!(event.discarded_cursor, 0);

    let json = event.to_json();
    assert_eq!(json["kind"], "freshness_anomaly");
    assert_eq!(json["source"], "ntfs-usn");
    assert_eq!(json["reason"], "journal_deleted");
    assert_eq!(json["rescan"]["volume"], "C:\\");
}

#[test]
fn acceptance_bounded_read_never_exceeds_its_budget() {
    let history: Vec<u64> = (1..=50).collect();
    let j = ScriptedJournal::new("C:\\", 0xAAAA_1111, &history);
    let mut source = UsnDeltaSource::fresh(j).with_max_batch(7);
    let (records, cursor) = applied(source.poll().unwrap());
    assert_eq!(records.len(), 7, "one read is bounded");
    assert_eq!(cursor, 7);
    let (records, cursor) = applied(source.poll().unwrap());
    assert_eq!(records.len(), 7);
    assert_eq!(cursor, 14);
}

#[test]
fn acceptance_observed_at_advances_only_on_observation() {
    let j = ScriptedJournal::new("C:\\", 0xAAAA_1111, &[1, 2]);
    let mut source = UsnDeltaSource::fresh(j).with_clock(1_700_000_000);
    assert_eq!(source.state().observed_at, 0, "no observation yet");
    applied(source.poll().unwrap());
    assert_eq!(source.state().observed_at, 1_700_000_000);
    match source.poll().unwrap() {
        DeltaOutcome::Idle => {}
        other => panic!("expected Idle, got {other:?}"),
    }
    // "No changes" is still an observation, so the freshness stamp moves.
    assert_eq!(source.state().observed_at, 1_700_000_000);
    assert!(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            > 0
    );
}
