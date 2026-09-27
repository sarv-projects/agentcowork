//! P4.2 / D2 — Excel Tauri commands: `xlsx_open` (windowed calamine read),
//! `xlsx_recalc` (IronCalc truth engine), and the Guard-2-ticketed cell edit
//! split (`xlsx_edit_request` → decision package + ticket; `xlsx_edit_commit`
//! → `use_ticket` + surgical part-patch). A cell write is a renderable
//! approval card, never a silent mutation.

use agentcowork_core::GuardDecision;
use agentcowork_guard::{
    change_set_hash, BatchOperation, DecisionPackage, Operation as GuardOp, RiskLevel,
};
use agentcowork_office::CommitReceipt;
use agentcowork_office::xlsx::address::{parse_range, parse_ref};
use agentcowork_office::xlsx::dsl::{
    pivot_result, Operation as XlsxOp, PivotAgg, Scalar, WorkbookCommandBatch,
};
use agentcowork_office::xlsx::patch::apply_batch;
use agentcowork_office::xlsx::read::{self, CellValue, SheetMeta, SheetWindow};
use agentcowork_office::xlsx::recalc::{self, RecalcResult};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use tauri::State;

use crate::AppState;

/// Metadata for one opened workbook + one windowed slice of a sheet.
#[derive(Debug, serde::Serialize)]
pub struct XlsxWindowPayload {
    pub path: String,
    pub sheets: Vec<SheetMeta>,
    pub sheet: String,
    pub offset: u32,
    pub total_rows: u32,
    pub total_cols: u32,
    pub rows: Vec<Vec<CellValue>>,
}

/// Open a workbook and return a windowed slice of one sheet. `sheet` picks
/// the tab (defaults to the first); `offset`/`limit` window rows.
#[tauri::command]
pub fn xlsx_open(
    path: String,
    sheet: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<XlsxWindowPayload, String> {
    let path_buf = crate::control::floor_user_file(&path)?;
    let meta = read::open(&path_buf).map_err(|e| e.to_string())?;

    let sheet_name = match &sheet {
        Some(s) if meta.sheets.iter().any(|m| m.name == *s) => s.clone(),
        Some(s) => return Err(format!("sheet not found: {s}")),
        None => meta
            .sheets
            .first()
            .map(|m| m.name.clone())
            .ok_or_else(|| "workbook has no sheets".to_string())?,
    };

    let window: SheetWindow =
        read::read_window(&path_buf, &sheet_name, offset, limit).map_err(|e| e.to_string())?;

    Ok(XlsxWindowPayload {
        path: meta.path,
        sheets: meta.sheets,
        sheet: window.sheet,
        offset: window.offset,
        total_rows: window.total_rows,
        total_cols: window.total_cols,
        rows: window.rows,
    })
}

/// P4.2/D2 — run the IronCalc truth engine over a workbook and return every
/// engine-computed value. This is the "LLM never invents a number" surface:
/// the formula bar's Recalc action shows formula_cells + computed values.
#[tauri::command]
pub fn xlsx_recalc(path: String) -> Result<RecalcResult, String> {
    let path = crate::control::floor_user_file(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    recalc::recalc(&bytes).map_err(|e| e.to_string())
}

/// P4.7 — Guard-2 "plan-before-touch" for a cell edit. Nothing is written:
/// resolve the plan (goal + paths + risk), route it through the shared
/// [`agentcowork_core::GuardService`], and return `allow` (commit directly) or
/// `ask` (a ticket the approval card renders). [`xlsx_edit_commit`] is the
/// executor half.
#[tauri::command]
pub fn xlsx_edit_request(
    state: State<'_, AppState>,
    path: String,
    sheet: String,
    address: String,
    value: String,
) -> Result<serde_json::Value, String> {
    // Validate the address up-front so the card never shows a bad ref.
    parse_ref(&address).map_err(|e| e.to_string())?;
    let path = crate::control::floor_user_file(&path)?
        .display()
        .to_string();

    let decision = DecisionPackage::new(format!("Set {address} to {value}"))
        .with_risk(RiskLevel::Medium)
        .with_paths(vec![path.clone()]);

    let args_hash = edit_args_hash(&path, &sheet, &address, &value);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let verdict = guard.evaluate(
        "office",
        "everyaios",
        "office.xlsx_edit",
        GuardOp::GenericWrite,
        decision,
        &args_hash,
        0,
    );
    match verdict {
        GuardDecision::Allow { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            Ok(serde_json::json!({
                "action": "allow",
                "address": address,
                "value": value,
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        GuardDecision::Ask { ticket_id } => {
            let approval_nonce = guard.approval_nonce(&ticket_id).unwrap_or("").to_string();
            Ok(serde_json::json!({
                "action": "ask",
                "address": address,
                "value": value,
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        GuardDecision::Block { reason } => Err(format!("edit blocked: {reason}")),
    }
}

/// P4.7 — the executor half of a cell edit: consume the ticket (**mandatory**
/// — single-use + args-hash match; approval is a hard prerequisite), then read
/// → patch → write the workbook byte-preserving (atomic temp+rename). Formula
/// writes go through the recalc engine (the LLM never supplies a number); the
/// patch rewrites only the changed sheet part.
#[tauri::command]
pub fn xlsx_edit_commit(
    state: State<'_, AppState>,
    path: String,
    sheet: String,
    address: String,
    value: String,
    ticket_id: String,
) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?
        .display()
        .to_string();
    let (_, cell) = parse_ref(&address).map_err(|e| e.to_string())?;

    // The ticket is mandatory: no ticket, no mutation. `use_ticket` enforces
    // approval + single-use + args-hash match in one call.
    let args_hash = edit_args_hash(&path, &sheet, &address, &value);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    guard
        .use_ticket(&ticket_id, &args_hash)
        .map_err(|e| format!("edit ticket not consumable: {e}"))?;
    drop(guard);

    crate::control::snapshot_file(&state, "office", &path);
    let bytes = std::fs::read(PathBuf::from(&path)).map_err(|e| e.to_string())?;
    let mut batch = WorkbookCommandBatch::new(0, format!("Set {address} to {value}"));
    batch.operations.push(XlsxOp::SetCell {
        address: cell,
        value: parse_scalar(&value),
    });

    let outcome = apply_batch(&bytes, &batch, &sheet).map_err(|e| e.to_string())?;
    let receipt = commit_workbook(&path, &outcome.bytes, &ticket_id)?;

    let audit_seq = crate::control::record_mutation(
        &state,
        crate::control::AuthKind::AgentTicket,
        "office.xlsx_edit",
        serde_json::json!({
            "path": path,
            "sheet": sheet,
            "address": address,
            "ticketId": ticket_id,
            "commitStages": receipt.verification.stages,
            "commitDurable": receipt.verification.durable,
        }),
    );

    Ok(serde_json::json!({
        "address": address,
        "sheet": sheet,
        "changedParts": outcome.changed_parts,
        "auditSeq": audit_seq,
        "commitStages": receipt.verification.stages,
    }))
}

/// P4.7 — Guard-2 plan-before-touch for a **bulk** batch (fill / sort / …).
/// The batch is the typed DSL [`WorkbookCommandBatch`] (deserialized from the
/// UI); the goal on the card is the batch's summary line.
#[tauri::command]
pub fn xlsx_batch_request(
    state: State<'_, AppState>,
    path: String,
    sheet: String,
    batch: WorkbookCommandBatch,
) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?
        .display()
        .to_string();
    let decision = DecisionPackage::new(batch.summary.clone())
        .with_risk(RiskLevel::Medium)
        .with_paths(vec![path.clone()]);

    // P47.6 — the bulk path now mints a **BatchTicket**: the immutable
    // change set (one BatchOperation per DSL op, each with its own args
    // hash + resource identities) is what the human approves. Approval
    // covers exactly this set — adding/removing/reordering an op after
    // approval changes the change-set hash and the ticket refuses.
    let operations = batch_operations(&sheet, &batch);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    let verdict = guard.evaluate_batch("office", "everyaios", operations, decision, 0);
    match verdict {
        GuardDecision::Allow { ticket_id } => {
            let approval_nonce = guard.batch_approval_nonce(&ticket_id).unwrap_or_default();
            Ok(serde_json::json!({
                "action": "allow",
                "summary": batch.summary,
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        GuardDecision::Ask { ticket_id } => {
            let approval_nonce = guard.batch_approval_nonce(&ticket_id).unwrap_or_default();
            Ok(serde_json::json!({
                "action": "ask",
                "summary": batch.summary,
                "ticketId": ticket_id,
                "approvalNonce": approval_nonce,
            }))
        }
        GuardDecision::Block { reason } => Err(format!("batch blocked: {reason}")),
    }
}

/// P4.7 — the executor half of a bulk batch: consume the ticket (**mandatory**),
/// then apply the batch byte-preserving and write the changed part atomically.
#[tauri::command]
pub fn xlsx_batch_commit(
    state: State<'_, AppState>,
    path: String,
    sheet: String,
    batch: WorkbookCommandBatch,
    ticket_id: String,
) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?
        .display()
        .to_string();
    // P47.6 — recompute the identical immutable change set and consume the
    // batch ticket with it. If the ops differ in any way from what the human
    // approved (added/removed/reordered/mutated args), the change-set hash
    // won't match and the ticket refuses — the executor can never stretch a
    // "approve all" to a different or larger set.
    let operations = batch_operations(&sheet, &batch);
    let cs_hash = change_set_hash(&operations);
    let mut guard = state.guard_service.lock().map_err(|e| e.to_string())?;
    guard
        .use_batch_ticket(&ticket_id, &cs_hash)
        .map_err(|e| format!("batch ticket not consumable: {e}"))?;
    drop(guard);

    crate::control::snapshot_file(&state, "office", &path);
    let bytes = std::fs::read(PathBuf::from(&path)).map_err(|e| e.to_string())?;
    let outcome = apply_batch(&bytes, &batch, &sheet).map_err(|e| e.to_string())?;
    let receipt = commit_workbook(&path, &outcome.bytes, &ticket_id)?;

    let audit_seq = crate::control::record_mutation(
        &state,
        crate::control::AuthKind::AgentTicket,
        "office.xlsx_batch",
        serde_json::json!({
            "path": path,
            "sheet": sheet,
            "summary": batch.summary,
            "ticketId": ticket_id,
            "commitStages": receipt.verification.stages,
            "commitDurable": receipt.verification.durable,
        }),
    );

    Ok(serde_json::json!({
        "summary": batch.summary,
        "sheet": sheet,
        "changedParts": outcome.changed_parts,
        "auditSeq": audit_seq,
        "commitStages": receipt.verification.stages,
    }))
}

/// P47.6 — the commit path (FIX-16).
///
/// A workbook commit must be **staging package → fsync → atomic swap**, not
/// "write + rename": without the fsync the bytes can be renamed into place and
/// still be lost on power failure, which is the OfficeCLI trade-off
/// `ARCH/22` §4 explicitly refuses to copy. `agentcowork_office::commit_bytes`
/// is the single crash-safe commit path in the runtime — it records the stages
/// it actually ran, and a failure names the stage it died in, so there is no
/// partial commit and no bare io error.
///
/// The commit also runs under a short-lived exclusive writer lease
/// (`ARCH/22` §4, REQ-OFFICE-003): a second writer is refused with the
/// `in use` result instead of silently overwriting.
fn commit_workbook(path: &str, bytes: &[u8], ticket_id: &str) -> Result<CommitReceipt, String> {
    let p = PathBuf::from(path);
    let work_id = format!("office.xlsx:{ticket_id}");
    agentcowork_office::commit_under_lease(
        p.as_path(),
        bytes,
        &work_id,
        "tauri:xlsx_cmds",
        agentcowork_office::now_ms(),
    )
    .map_err(|e| match e {
        // The typed "in use" result: name the holder and the two options.
        agentcowork_office::ResidentError::InUse(c) => c.message(),
        other => other.to_string(),
    })
}

/// P4.7 — read-only pivot: group a source range and return the in-memory
/// summary. No write, so no Guard-2 ticket. `group_by`/`aggregate` are
/// 0-based column offsets *within* the source range.
#[tauri::command]
pub fn xlsx_pivot(
    path: String,
    sheet: String,
    source: String,
    group_by: usize,
    aggregate: usize,
    agg: String,
) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?;
    let (_, range) = parse_range(&source).map_err(|e| e.to_string())?;
    let agg = match agg.as_str() {
        "sum" => PivotAgg::Sum,
        "count" => PivotAgg::Count,
        _ => PivotAgg::Avg,
    };
    let rows = read::read_range(path.as_path(), &sheet, &range).map_err(|e| e.to_string())?;
    let out = pivot_result(&rows, group_by, aggregate, agg);
    serde_json::to_value(&out).map_err(|e| e.to_string())
}

/// Parse a UI-typed value string into the DSL scalar (number/bool/text).
fn parse_scalar(value: &str) -> Scalar {
    let t = value.trim();
    if t.eq_ignore_ascii_case("true") {
        Scalar::Bool(true)
    } else if t.eq_ignore_ascii_case("false") {
        Scalar::Bool(false)
    } else if let Ok(n) = t.parse::<f64>() {
        Scalar::Number(n)
    } else {
        Scalar::Text(value.to_string())
    }
}

/// Deterministic, scope-tagged args hash for the ticket match.
fn edit_args_hash(path: &str, sheet: &str, address: &str, value: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "office.xlsx_edit".hash(&mut h);
    path.hash(&mut h);
    sheet.hash(&mut h);
    address.hash(&mut h);
    value.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// P47.6 — map a bulk batch to the immutable change set the BatchTicket
/// binds. One [`BatchOperation`] per DSL op, each with its **own** args hash
/// (canonical JSON of that op — serde preserves variant/field order) and its
/// resource identities (address/range/sheet). The `sheet` scope is folded
/// into the resources of every op, so a ticket minted for one sheet can't be
/// replayed on another. Deterministic: the same batch always yields the same
/// change set (and therefore the same change-set hash).
fn batch_operations(sheet: &str, batch: &WorkbookCommandBatch) -> Vec<BatchOperation> {
    let mut ops = Vec::with_capacity(batch.operations.len());
    for op in &batch.operations {
        let (name, resources) = op_identity(sheet, op);
        let args_hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            "office.xlsx_batch".hash(&mut h);
            sheet.hash(&mut h);
            serde_json::to_string(op).unwrap_or_default().hash(&mut h);
            format!("{:016x}", h.finish())
        };
        ops.push(BatchOperation::new(
            "office.xlsx_batch",
            name,
            args_hash,
            resources,
        ));
    }
    ops
}

/// The operation name + resource identities for one DSL op (for the change
/// set). `sheet` is the scope every op touches.
fn op_identity(sheet: &str, op: &XlsxOp) -> (&'static str, Vec<String>) {
    let mut resources = vec![format!("sheet:{sheet}")];
    match op {
        XlsxOp::SetCell { address, .. } => {
            resources.push(format!("cell:{address}"));
            ("set_cell", resources)
        }
        XlsxOp::SetFormula { address, .. } => {
            resources.push(format!("cell:{address}"));
            ("set_formula", resources)
        }
        XlsxOp::ClearRange { range } => {
            resources.push(format!("range:{range}"));
            ("clear_range", resources)
        }
        XlsxOp::RenameSheet { from, to } => {
            resources.push(format!("sheet:{from}->{to}"));
            ("rename_sheet", resources)
        }
        XlsxOp::SortRange { range, .. } => {
            resources.push(format!("range:{range}"));
            ("sort_range", resources)
        }
        XlsxOp::FillRange { range, .. } => {
            resources.push(format!("range:{range}"));
            ("fill_range", resources)
        }
        XlsxOp::Shift { .. } => ("shift", resources),
        XlsxOp::Pivot { source, .. } => {
            resources.push(format!("range:{source}"));
            ("pivot", resources)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scalar_types() {
        assert_eq!(parse_scalar("42"), Scalar::Number(42.0));
        assert_eq!(parse_scalar("3.5"), Scalar::Number(3.5));
        assert_eq!(parse_scalar("true"), Scalar::Bool(true));
        assert_eq!(parse_scalar("FALSE"), Scalar::Bool(false));
        assert_eq!(parse_scalar("hello"), Scalar::Text("hello".into()));
    }

    #[test]
    fn edit_args_hash_is_deterministic_and_scoped() {
        let a = edit_args_hash("/w/a.xlsx", "Sheet1", "B4", "42");
        let b = edit_args_hash("/w/a.xlsx", "Sheet1", "B4", "42");
        assert_eq!(a, b);
        // Any change in any scope field changes the hash (ticket mismatch).
        assert_ne!(a, edit_args_hash("/w/a.xlsx", "Sheet1", "B4", "43"));
        assert_ne!(a, edit_args_hash("/w/a.xlsx", "Sheet1", "B5", "42"));
        assert_ne!(a, edit_args_hash("/w/a.xlsx", "Sheet2", "B4", "42"));
        assert_ne!(a, edit_args_hash("/w/b.xlsx", "Sheet1", "B4", "42"));
    }

    #[test]
    fn edit_request_rejects_bad_address() {
        // parse_ref must fail on an empty/garbage address — the card never
        // shows a bad ref.
        assert!(parse_ref("").is_err());
        assert!(parse_ref("!!").is_err());
        assert!(parse_ref("B4").is_ok());
        assert!(parse_ref("AA12").is_ok());
    }

    #[test]
    fn batch_operations_are_deterministic_per_op_and_sheet_scoped() {
        let mut b1 = WorkbookCommandBatch::new(0, "Fill B2:B10 with 5");
        b1.operations.push(XlsxOp::FillRange {
            range: parse_range("B2:B10").unwrap().1,
            mode: agentcowork_office::xlsx::dsl::FillMode::Constant,
            value: Some(Scalar::Number(5.0)),
        });

        // Deterministic: same batch → same change set → same change-set hash.
        let ops1 = batch_operations("Sheet1", &b1);
        assert_eq!(ops1.len(), 1);
        assert_eq!(ops1[0].tool_id, "office.xlsx_batch");
        assert_eq!(ops1[0].operation, "fill_range");
        assert!(ops1[0].resources.iter().any(|r| r == "range:B2:B10"));
        let h1 = change_set_hash(&ops1);
        assert_eq!(h1, change_set_hash(&batch_operations("Sheet1", &b1)));

        // Sheet scope is part of the change set (can't replay on another).
        assert_ne!(h1, change_set_hash(&batch_operations("Sheet2", &b1)));

        // A mutated op (different value) changes the change-set hash — the
        // approval can never be stretched to different args.
        let mut b2 = b1.clone();
        b2.operations[0] = XlsxOp::FillRange {
            range: parse_range("B2:B10").unwrap().1,
            mode: agentcowork_office::xlsx::dsl::FillMode::Constant,
            value: Some(Scalar::Number(6.0)),
        };
        assert_ne!(h1, change_set_hash(&batch_operations("Sheet1", &b2)));

        // An extra op added after approval changes the binding (approve-all
        // covers exactly the set, never a category).
        let mut b3 = b1.clone();
        b3.operations.push(XlsxOp::ClearRange {
            range: parse_range("C2:C5").unwrap().1,
        });
        assert_ne!(h1, change_set_hash(&batch_operations("Sheet1", &b3)));
    }

    #[test]
    fn the_workbook_commit_path_is_the_fsynced_one() {
        // FIX-16: the old local `atomic_write` was write + rename with **no
        // fsync**, so a power loss could lose the just-committed bytes. This
        // asserts the shell's commit helper routes through the single
        // crash-safe path and that the stages it reports are the required
        // order — which fails if the fsync is ever dropped from that path.
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-xlsx-commit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.xlsx");
        std::fs::write(&path, b"v1").unwrap();

        let before = agentcowork_office::fsync_calls();
        let receipt = commit_workbook(
            path.to_str().unwrap(),
            b"v2-with-fsync",
            "ticket-1",
        )
        .expect("the commit lands");
        assert!(
            agentcowork_office::fsync_calls() >= before + 1,
            "the shell's workbook commit did not fsync its staging package"
        );
        assert_eq!(
            receipt.verification.stages,
            vec![
                agentcowork_office::CommitStage::Staged,
                agentcowork_office::CommitStage::Fsynced,
                agentcowork_office::CommitStage::Swapped,
                agentcowork_office::CommitStage::DurablyRenamed
            ]
        );
        assert!(receipt.verification.durable);
        assert_eq!(std::fs::read(&path).unwrap(), b"v2-with-fsync");
        // The lease is released after the commit, so the next edit re-acquires.
        assert_eq!(receipt.work_id, "office.xlsx:ticket-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_workbook_commit_refuses_a_second_concurrent_writer() {
        // REQ-OFFICE-003: two writers on one document must produce the explicit
        // "in use" result, never a silent overwrite.
        let dir = std::env::temp_dir().join(format!(
            "agentcowork-xlsx-lease-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("busy.xlsx");
        std::fs::write(&path, b"v1").unwrap();
        let p = path.to_str().unwrap().to_string();

        // A long-lived lease held by another work item (as an open document
        // would hold) blocks the shell's commit with a named conflict.
        {
            let mut registry =
                agentcowork_office::resident::process_registry().lock().unwrap();
            let table = registry.table(agentcowork_office::DocFormat::Xlsx);
            table
                .open(std::path::Path::new(&p), "other-work", "other-session", 60_000, agentcowork_office::now_ms())
                .unwrap();
        }
        let err = commit_workbook(&p, b"v2", "ticket-2").unwrap_err();
        assert!(
            err.contains("in use by work other-work"),
            "expected the typed in-use result, got: {err}"
        );
        // The document is untouched.
        assert_eq!(std::fs::read(&path).unwrap(), b"v1");

        {
            let mut registry =
                agentcowork_office::resident::process_registry().lock().unwrap();
            let table = registry.table(agentcowork_office::DocFormat::Xlsx);
            let _ = table.close(std::path::Path::new(&p), "other-work", agentcowork_office::now_ms());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_ticket_flow_consumes_exact_change_set_only() {
        // Full loop through the GuardService, mirroring xlsx_batch_request →
        // xlsx_batch_commit: the request mints a BatchTicket over the change
        // set; the exact set consumes; a stretched set is refused (approve
        // all binds the set, never a category).
        use agentcowork_core::GuardService;
        use agentcowork_guard::{DecisionPackage as Dp, RiskLevel as Rl};

        let mut b1 = WorkbookCommandBatch::new(0, "Fill + clear");
        b1.operations.push(XlsxOp::FillRange {
            range: parse_range("B2:B10").unwrap().1,
            mode: agentcowork_office::xlsx::dsl::FillMode::Constant,
            value: Some(Scalar::Number(5.0)),
        });
        b1.operations.push(XlsxOp::ClearRange {
            range: parse_range("C2:C5").unwrap().1,
        });

        let mut guard = GuardService::new();
        let ops = batch_operations("Sheet1", &b1);
        let verdict = guard.evaluate_batch(
            "office",
            "everyaios",
            ops.clone(),
            Dp::new(b1.summary.clone()).with_risk(Rl::Medium),
            0,
        );
        let agentcowork_core::GuardDecision::Ask { ticket_id } = verdict else {
            panic!("expected Ask");
        };

        // Human approval (card-bound nonce), exactly like the UI flow.
        let nonce = guard.batch_approval_nonce(&ticket_id).unwrap();
        assert!(guard.approve_batch_with_nonce(&ticket_id, &nonce));

        // Exact change set consumes.
        let cs = change_set_hash(&ops);
        assert!(guard.use_batch_ticket(&ticket_id, &cs).is_ok());

        // A second ticket over a *larger* set (an extra op the human never
        // saw in the first approval): the executor presents the ORIGINAL
        // (smaller) change set — refused. The approval binds the exact set,
        // never a category the agent could stretch.
        let mut b2 = b1.clone();
        b2.operations.push(XlsxOp::RenameSheet {
            from: "Sheet1".into(),
            to: "Renamed".into(),
        });
        let mut guard2 = GuardService::new();
        let ops2 = batch_operations("Sheet1", &b2);
        let agentcowork_core::GuardDecision::Ask { ticket_id: t2 } = guard2.evaluate_batch(
            "office",
            "everyaios",
            ops2.clone(),
            Dp::new(b2.summary.clone()).with_risk(Rl::Medium),
            0,
        ) else {
            panic!("expected Ask");
        };
        let nonce2 = guard2.batch_approval_nonce(&t2).unwrap();
        assert!(guard2.approve_batch_with_nonce(&t2, &nonce2));
        // Presenting the original smaller set to the larger approval refuses.
        assert!(guard2.use_batch_ticket(&t2, &cs).is_err());
        // The exact (larger) set still works.
        assert!(guard2
            .use_batch_ticket(&t2, &change_set_hash(&ops2))
            .is_ok());
    }
}
