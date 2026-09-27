//! agentcowork-office — surgical OOXML editing (P4, D1–D8).
//!
//! The core principle (ARCH/04 §4.1): an OOXML file is a ZIP of XML parts;
//! **byte-preserving editing = open the ZIP, patch only the targeted XML
//! part(s), write those parts back, and copy every other entry byte-for-byte.**
//!
//! P4.1 ships the Word block-patch engine (D1, GenOffice pattern doc 28):
//! - ZIP open + parts index (`zip`)
//! - anchored block tree with stable addresses (`p3`, `t1:r1c2:p1`,
//!   `hdr1:p1`, `sec1`) across body + headers/footers
//! - plain-text rendering (the LLM's edit surface)
//! - minimal `w:t` prefix/suffix patch — untouched bytes never re-serialized
//! - byte-preserving ZIP rewrite (`raw_copy_file` verbatim copy)
//!
//! P4.2 ships the Excel engine (D2, doc 28 §4–5):
//! - `xlsx::read` — calamine windowed reader (virtualized 100K+ row view)
//! - `xlsx::recalc` — IronCalc truth engine (100% math integrity: numeric
//!   claims come from IronCalc, never the LLM)
//! - `xlsx::dsl` — workbook DSL (cell-address, formula-shift, sort-range,
//!   flash-fill, pivot) with the Excel-accurate reference-rewrite engine
//! - `xlsx::planner` — deterministic regex NLP → DSL (zero-LLM common ops),
//!   `NeedsLlm` fallback (audit-flagged, permission-gated)
//! - `xlsx::patch` — surgical `sheetN.xml`/`sharedStrings.xml` part-patch
//!
//! P4.3 ships the PowerPoint part-editor (D3, ARCH/04 §PowerPoint):
//! - `pptx::parts` — package index (content types + presentation rels +
//!   `<p:sldIdLst>` slide order)
//! - `pptx::text` — slide shapes → paragraphs → `<a:t>` runs; render + minimal
//!   byte-surgery patch (bullets/line-breaks are read-only markers)
//! - `pptx::PptxEngine` — render/patch + add/remove slides (clone part + rels
//!   + `[Content_Types].xml` registration) + byte-preserving save
//!
//! P4.5 ships conformance + rollback (D6/D7, ARCH/04 §4.4):
//! - `atomic::write_atomic` — temp → fsync → rename (no half-written files)
//! - `rollback::Snapshot` — `snapshotBefore` pre-edit bytes, one-click undo
//! - `conformance::parts_diff` — zip-level diff of changed/added/removed parts
//! - `conformance::LibreOfficeOracle` — headless soffice "opens clean" check
//!
//! P4.6 ships legacy formats (D8, doc 29 §3a):
//! - `legacy` — `.doc`/`.xls`/`.ppt` detection + headless conversion to modern
//!   OOXML, surfaced read-only with an "edit as new" path
//!
//! P4.4 ships the PDF engine (D4, ARCH/04 §4.2 PDF):
//! - `pdf::form` — AcroForm form-fill (`/V`)
//! - `pdf::replace_text` — exact-match `Tj` text swap
//! - `pdf::redact` — mark-for-redact `/Redact` annotations
//! - `pdf::author` — re-author: build a new PDF from text
//!
//! P4.7b ships the office "perfectness" D-gaps (doc 63 §3):
//! - `docx::track` — track-changes + comments: extract `w:ins`/`w:del`/
//!   `w:comment`, emit a tracked change for a patch, add comments
//! - `docx::citation` — CSL citations (APA/IEEE/Chicago): render citation +
//!   reference + bibliography, `ReferenceLibrary` search, insert into a docx
//! - `xlsx::chart` — chart parts: extract series (name/category/value ranges),
//!   author a Bar/Line/Pie chart part (+ rels + content-type override)
//! - `pptx::transition` — slide transitions: extract + set `p:transition`
//! - `pptx::anim` — `p:timing` animations (Fade/Zoom/Appear) targeting `p:spTgt`
//! - `pptx::notes` — speaker notes: extract text, build notes, validate
//!   notes↔slides sync, plan rehearsal timing
//! - `pdf::annot` — PDF annotations: sticky-note text + highlight rects
//!
//! ARCH/04 §4.6 closes the three gaps the document used to claim in the
//! present tense but no code implemented (as TypeScript files, in a Rust-only
//! crate):
//! - `docx::field_balance` — `w:fldChar` `begin`/`separate`/`end` balance +
//!   nesting verification. Runs after every patch and again before commit; a
//!   violation refuses by naming the part and the field index, leaving the
//!   archive byte-identical. A field with no `separate` (a dirty field) is
//!   legal and is not rejected.
//! - `media_gc` — orphaned-media collection. For one part: collect the
//!   `r:`-namespace relationship ids its XML still names, find the media
//!   payloads and `_rels` entries that no longer resolve, remove both in the
//!   same atomic rebuild — or report them as cleanup candidates when removal
//!   would change a part the engine cannot safely rewrite.
//! - `limits` — the bounded-memory size policy. **This engine is DOM +
//!   byte-range by design and does not stream**; the honest form of the
//!   original "single-pass, <15MB on a 500-page document" claim is a
//!   fail-closed ceiling: over it, a patch refuses with a named reason
//!   instead of attempting a load it cannot bound. Every ceiling is
//!   configurable (`AGENTCOWORK_OFFICE_MAX_*` or an explicit `PatchLimits`).
//!
//! W0 (`ARCH/22` §10) closed three more code-phase gaps:
//! - `atomic` — the single crash-safe commit path (**staging package → fsync →
//!   atomic swap → durable directory entry**, FIX-16), with a `CommitTrace`
//!   recording the stages it actually ran and `recover_orphans` for the
//!   staging package a pre-swap crash leaves behind.
//! - `resident` — one resident context per open document with an **exclusive
//!   writer lease** bound to the work item, crash-safe lease expiry, the
//!   interval + dirty-marker flush policy, and idle eviction under a memory
//!   bound (FIX-14 / REQ-OFFICE-003). **Op-log replay is not implemented** —
//!   see the module docs.
//! - `pdf::redact` — **true content removal** (FIX-15): text-showing operators
//!   whose glyphs intersect a redaction rectangle are removed from the content
//!   stream, and a post-op text-extraction check proves the removal. The old
//!   mark-for-redact behaviour is still available, explicitly, as
//!   `pdf::redact::mark_for_redaction`.

pub mod atomic;
pub mod conformance;
pub mod docx;
pub mod legacy;
pub mod limits;
pub mod media_gc;
pub mod pdf;
pub mod pptx;
pub mod provenance;
pub mod resident;
pub mod rollback;
pub mod xlsx;
pub mod xml;
pub mod zip;

pub use atomic::{
    AtomicError, CommitError, CommitStage, CommitTrace, StagedOrphan, commit_bytes, fsync_calls,
    recover_orphans, verify_readback, write_atomic,
};
pub use conformance::{LibreOfficeOracle, PartsDiff, find_soffice, parts_diff};
pub use docx::field_balance::{FieldBalanceError, FieldCheckError, FieldReport};
pub use docx::{DocxEngine, OfficeError};
pub use legacy::{LegacyKind, LegacyOpen, convert_to_modern};
pub use limits::{LimitKind, LoadBudget, PatchLimits};
pub use media_gc::{CandidateReason, CleanupCandidate, MediaSweep, SweepPlan};
pub use pdf::pages::{
    PageOpError, delete_pages, extract_pages, merge as merge_pdfs, page_count,
    reorder as reorder_pages, rotate as rotate_pages, split as split_pdf,
};
pub use pdf::redact::{RedactOptions, RedactReport, RedactRequest, UnremovablePolicy};
pub use pdf::{PdfError, PdfInfo, inspect, replace_text};
pub use pptx::PptxEngine;
pub use pptx::author::{
    AuthorError, DeckBrief, DeckSlide, author_deck, speaker_notes as deck_speaker_notes,
};
pub use resident::{
    CommitReceipt, CommitVerification, DocFormat, DocRoots, FlushPolicy, LeaseConflict,
    LeaseHolder, ResidentContext, ResidentError, ResidentRegistry, ResidentTable, WriterLease,
    commit_under_lease, now_ms,
};
pub use rollback::Snapshot;

// D-gaps (doc 63 §3) — the "perfectness" additions.
pub use docx::citation::{
    CslStyle, Reference, ReferenceKind, ReferenceLibrary, insert_citation_into_docx,
    render_bibliography, render_citation, render_reference,
};
pub use docx::track::{
    Comment, TrackAuthor, TrackError, TrackedChange, TrackedChangeKind, add_comment,
    emit_tracked_change, extract_comments, extract_tracked_changes, render_comment_reference,
    render_del_run, render_ins_run,
};
pub use pdf::annot::{add_highlight_annotation, add_text_annotation};
pub use pptx::anim::{AnimError, AnimationEffect, build_timing_xml};
pub use pptx::notes::{
    NotesError, RehearsalTiming, SpeakerNotesEntry, build_speaker_notes, extract_notes_text,
    plan_rehearsal, validate_slides_notes_sync,
};
pub use pptx::transition::{
    Transition, TransitionError, TransitionKind, extract_transition, set_transition,
};
pub use xlsx::chart::{
    ChartError, ChartKind, ChartSeries, ChartSeriesSpec, build_chart_part,
    chart_content_type_override, chart_rel_fragment, extract_chart_series,
};
