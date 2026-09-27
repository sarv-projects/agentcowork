//! P4.7 — office viewer commands (H5): `docx_open`, `pptx_open`, `pdf_open`.
//!
//! These are **read surfaces** — the engines render their plain-text/outline
//! view for the webview. Surgical edits (P4.1–P4.4) stay behind their own
//! engines; the viewers here feed the H5 "open + read + chat-overlay" flow.

use agentcowork_office::docx::DocxEngine;
use agentcowork_office::pdf;
use agentcowork_office::pptx::PptxEngine;
use tauri::State;

use crate::AppState;

fn read_bytes(path: &str) -> Result<Vec<u8>, String> {
    let path = crate::control::floor_user_file(path)?;
    std::fs::read(path).map_err(|e| e.to_string())
}

/// One addressable block of a docx (for the viewer's block sidebar).
#[derive(Debug, serde::Serialize)]
pub struct DocxBlockInfo {
    pub address: String,
    pub kind: String,
    pub part: String,
}

#[derive(Debug, serde::Serialize)]
pub struct DocxPayload {
    pub path: String,
    pub text: String,
    pub blocks: Vec<DocxBlockInfo>,
}

/// Open a `.docx` and render its plain text + block tree.
#[tauri::command]
pub fn docx_open(path: String) -> Result<DocxPayload, String> {
    let bytes = read_bytes(&path)?;
    let engine = DocxEngine::open(bytes).map_err(|e| e.to_string())?;
    let text = engine.render_text().to_string();
    let blocks = engine
        .blocks()
        .iter()
        .map(|b| DocxBlockInfo {
            address: b.address.clone(),
            kind: format!("{:?}", b.kind),
            part: b.part.clone(),
        })
        .collect();
    Ok(DocxPayload { path, text, blocks })
}

#[derive(Debug, serde::Serialize)]
pub struct PptxSlideInfo {
    pub part: String,
    pub text: String,
}

#[derive(Debug, serde::Serialize)]
pub struct PptxPayload {
    pub path: String,
    pub slides: Vec<PptxSlideInfo>,
    pub deck: String,
}

/// Open a `.pptx` and render its deck outline + per-slide text.
#[tauri::command]
pub fn pptx_open(path: String) -> Result<PptxPayload, String> {
    let bytes = read_bytes(&path)?;
    let mut engine = PptxEngine::open(bytes).map_err(|e| e.to_string())?;
    let deck = engine.render_deck().map_err(|e| e.to_string())?;
    let parts: Vec<String> = engine.slides().iter().map(|s| s.part.clone()).collect();
    let slides = parts
        .iter()
        .map(|p| PptxSlideInfo {
            part: p.clone(),
            text: engine.render_slide(p).unwrap_or_default(),
        })
        .collect();
    Ok(PptxPayload { path, slides, deck })
}

#[derive(Debug, serde::Serialize)]
pub struct PdfPayload {
    pub path: String,
    pub pages: u32,
    pub texts: Vec<String>,
}

/// Open a `.pdf` and return the page count + per-page extracted text.
#[tauri::command]
pub fn pdf_open(path: String) -> Result<PdfPayload, String> {
    let bytes = read_bytes(&path)?;
    let info = pdf::inspect(&bytes).map_err(|e| e.to_string())?;
    Ok(PdfPayload {
        path,
        pages: info.pages,
        texts: info.texts,
    })
}

/// P4.4 — the raw PDF bytes as a base64 `data:application/pdf;base64,` URL,
/// so the webview's pdf.js canvas renderer can draw real pages (the text-only
/// `pdf_open` stays as the accessibility / extraction layer).
#[tauri::command]
pub fn pdf_bytes(path: String) -> Result<String, String> {
    let bytes = read_bytes(&path)?;
    Ok(format!(
        "data:application/pdf;base64,{}",
        base64_encode(&bytes)
    ))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Surgical block patch (agent + Word viewer).
/// v3.59: human-UI path — the click is the authorization, the mutation is
/// audited on the same Merkle chain as agent/ticket effects (spec §4.3 / P47.1).
#[tauri::command]
pub fn docx_patch(
    state: State<'_, AppState>,
    path: String,
    address: String,
    text: String,
) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let mut engine = DocxEngine::open(bytes).map_err(|e| e.to_string())?;
    engine
        .patch_block(&address, &text)
        .map_err(|e| e.to_string())?;
    let out = engine.save().map_err(|e| e.to_string())?;
    agentcowork_office::write_atomic(&path, &out).map_err(|e| e.to_string())?;
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "office.docx_patch",
        serde_json::json!({
            "path": path.display().to_string(),
            "address": address,
            "charDelta": text.chars().count(),
        }),
    );
    Ok(serde_json::json!({ "ok": true, "address": address }))
}

/// Tracked changes + comments for the Word viewer.
#[tauri::command]
pub fn docx_tracks(path: String) -> Result<serde_json::Value, String> {
    let bytes = read_bytes(&path)?;
    let mut archive =
        agentcowork_office::zip::OoxmlArchive::open(bytes).map_err(|e| e.to_string())?;
    let doc_xml = archive
        .read_part("word/document.xml")
        .map_err(|e| e.to_string())?;
    let doc_str = String::from_utf8_lossy(&doc_xml);
    let changes = agentcowork_office::docx::track::extract_tracked_changes(&doc_str)
        .map_err(|e| e.to_string())?;
    let comments_xml = archive.read_part("word/comments.xml").ok();
    let comments = comments_xml
        .as_ref()
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|s| agentcowork_office::docx::track::extract_comments(s).ok())
        .unwrap_or_default();
    Ok(serde_json::json!({
        "changes": changes.iter().map(|c| serde_json::json!({
            "kind": format!("{:?}", c.kind),
            "author": c.author,
            "text": c.text,
        })).collect::<Vec<_>>(),
        "comments": comments.iter().map(|c| serde_json::json!({
            "id": c.id,
            "author": c.author,
            "text": c.text,
        })).collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub fn pptx_notes(path: String) -> Result<serde_json::Value, String> {
    let bytes = read_bytes(&path)?;
    let mut archive =
        agentcowork_office::zip::OoxmlArchive::open(bytes).map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    for i in 1..=64 {
        let part = format!("ppt/notesSlides/notesSlide{i}.xml");
        if let Ok(xml) = archive.read_part(&part) {
            if let Ok(s) = std::str::from_utf8(&xml) {
                if let Ok(text) = agentcowork_office::pptx::notes::extract_notes_text(s) {
                    notes.push(serde_json::json!({ "slide": i, "talk": text }));
                }
            }
        }
    }
    Ok(serde_json::json!({ "notes": notes }))
}

#[tauri::command]
pub fn pdf_page_op(
    state: State<'_, AppState>,
    path: String,
    op: String,
    pages: Option<Vec<u32>>,
    delta: Option<i64>,
    other: Option<String>,
    out: Option<String>,
) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let pages = pages.unwrap_or_default();
    let dest = match out {
        Some(p) => crate::control::floor_user_file(&p)?,
        None => path.clone(),
    };
    // Each arm produces the new bytes plus whatever evidence the receipt needs.
    let (result, evidence) = match op.as_str() {
        "split" if pages.len() >= 2 => (
            agentcowork_office::split_pdf(&bytes, pages[0]..=pages[1])
                .map_err(|e| e.to_string())?,
            None,
        ),
        "extract" => (
            agentcowork_office::extract_pages(&bytes, &pages).map_err(|e| e.to_string())?,
            None,
        ),
        "reorder" => (
            agentcowork_office::reorder_pages(&bytes, &pages).map_err(|e| e.to_string())?,
            None,
        ),
        "delete" => (
            agentcowork_office::delete_pages(&bytes, &pages).map_err(|e| e.to_string())?,
            None,
        ),
        "rotate" => (
            agentcowork_office::rotate_pages(
                &bytes,
                delta.unwrap_or(90),
                if pages.is_empty() {
                    None
                } else {
                    Some(pages.as_slice())
                },
            )
            .map_err(|e| e.to_string())?,
            None,
        ),
        "merge" => {
            let other = other.ok_or("merge requires other")?;
            let other = crate::control::floor_user_file(&other)?;
            let b2 = std::fs::read(&other).map_err(|e| e.to_string())?;
            (
                agentcowork_office::merge_pdfs(&[bytes.clone(), b2]).map_err(|e| e.to_string())?,
                None,
            )
        }
        // P2.12 — surgical content ops wired to the same engine the agent
        // tools use: `other` carries a JSON payload.
        "form_fill" => {
            let raw = other.ok_or("form_fill requires a JSON fields payload")?;
            let fields: Vec<(String, String)> =
                serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                    .map_err(|e| format!("form_fill payload: {e}"))?
                    .into_iter()
                    .map(|v| {
                        (
                            v.get("field")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            v.get("value")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        )
                    })
                    .collect();
            if fields.is_empty() {
                return Err("form_fill requires at least one {field, value}".into());
            }
            (
                agentcowork_office::pdf::form::form_fill(&bytes, &fields)
                    .map_err(|e| e.to_string())?,
                None,
            )
        }
        "redact" => {
            let raw = other.ok_or("redact requires a JSON rects payload")?;
            let items: Vec<serde_json::Value> =
                serde_json::from_str(&raw).map_err(|e| format!("redact payload: {e}"))?;
            let rects: Vec<(u32, [f32; 4])> = items
                .iter()
                .cloned()
                .map(|v| {
                    let page = v
                        .get("page")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1) as u32;
                    let rect = v
                        .get("rect")
                        .and_then(serde_json::Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(serde_json::Value::as_f64)
                                .map(|f| f as f32)
                                .collect::<Vec<f32>>()
                        })
                        .unwrap_or_default();
                    if rect.len() != 4 {
                        return Err(format!("redact rect must be [x1,y1,x2,y2], got {rect:?}"));
                    }
                    Ok((page, [rect[0], rect[1], rect[2], rect[3]]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if rects.is_empty() {
                return Err("redact requires at least one {page, rect}".into());
            }
            // Optional post-op proof (REQ-OFFICE-008): a payload may name the
            // strings whose absence must be proven by text extraction *and* by
            // a raw scan of the saved bytes. A target that survives refuses the
            // whole operation — there is no partial "redaction".
            // Each item may carry `"verifyAbsent": "text"` or an array of
            // them; the post-op proof runs for every string found.
            let verify_absent: Vec<String> = items
                .iter()
                .filter_map(|v| v.get("verifyAbsent"))
                .flat_map(|v| match v {
                    serde_json::Value::String(s) => vec![s.clone()],
                    serde_json::Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => Vec::new(),
                })
                .filter(|s| !s.is_empty())
                .collect();

            // A payload may opt into the non-default policy; the default
            // refuses anything this engine cannot remove.
            let allow_unremovable = items
                .iter()
                .any(|v| v.get("allowUnremovable").and_then(|x| x.as_bool()) == Some(true));
            let opts = agentcowork_office::pdf::redact::RedactOptions {
                unremovable: if allow_unremovable {
                    agentcowork_office::pdf::redact::UnremovablePolicy::Report
                } else {
                    agentcowork_office::pdf::redact::UnremovablePolicy::Refuse
                },
                ..agentcowork_office::pdf::redact::RedactOptions::default()
            };
            let request =
                agentcowork_office::pdf::redact::RedactRequest::new(rects.clone())
                    .with_verify_absent(verify_absent);
            let report =
                agentcowork_office::pdf::redact::redact_checked(&bytes, &request, &opts)
                    .map_err(redact_error)?;
            // The evidence a receipt needs: what was removed, what the page
            // still says, and the engine's declared reach.
            let removed_chars = report.removed_chars();
            let evidence = serde_json::json!({
                "removals": report.removals,
                "removedChars": removed_chars,
                "survivingText": report.surviving_text,
                "unremovable": report.unremovable,
                "residuals": report.residuals,
                "noIntersection": report.no_intersection,
            });
            (report.bytes, Some(evidence))
        }
        "annotate" => {
            let raw = other.ok_or("annotate requires a JSON {page, rect, text?} payload")?;
            let v: serde_json::Value =
                serde_json::from_str(&raw).map_err(|e| format!("annotate payload: {e}"))?;
            let page = v
                .get("page")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1) as u32;
            let rect = v
                .get("rect")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(serde_json::Value::as_f64)
                        .map(|f| f as f32)
                        .collect::<Vec<f32>>()
                })
                .unwrap_or_default();
            if rect.len() != 4 {
                return Err(format!("annotate rect must be [x1,y1,x2,y2], got {rect:?}"));
            }
            let rect = [rect[0], rect[1], rect[2], rect[3]];
            let out = match v.get("text").and_then(serde_json::Value::as_str) {
                Some(text) if !text.is_empty() => {
                    agentcowork_office::pdf::annot::add_text_annotation(&bytes, page, rect, text)
                        .map_err(|e| e.to_string())?
                }
                _ => agentcowork_office::pdf::annot::add_highlight_annotation(&bytes, page, rect)
                    .map_err(|e| e.to_string())?,
            };
            (out, None)
        }
        other_op => return Err(format!("unknown pdf page op: {other_op}")),
    };

    // The commit path (FIX-16): staging package → fsync → atomic swap, under a
    // short-lived exclusive writer lease (REQ-OFFICE-003/004). A second writer
    // is refused with the "in use" result instead of overwriting.
    let receipt = agentcowork_office::commit_under_lease(
        dest.as_path(),
        &result,
        &format!("office.pdf_op:{op}"),
        "tauri:office_cmds",
        agentcowork_office::now_ms(),
    )
    .map_err(|e| match e {
        agentcowork_office::ResidentError::InUse(c) => c.message(),
        other => other.to_string(),
    })?;

    // v3.59 — human-UI path audit (spec §4.3 / P47.1).
    crate::control::record_mutation(
        &state,
        crate::control::AuthKind::HumanGesture,
        "office.pdf_op",
        serde_json::json!({
            "path": dest.display().to_string(),
            "op": op,
            "pages": pages,
            "commitStages": receipt.verification.stages,
            "commitDurable": receipt.verification.durable,
            "evidence": evidence,
        }),
    );
    Ok(serde_json::json!({
        "ok": true,
        "path": dest.display().to_string(),
        "commitStages": receipt.verification.stages,
        "evidence": evidence,
    }))
}

/// Render a redaction failure for the surface. The typed variants name what
/// happened, so the UI can offer a real action (shrink the rectangle, accept
/// the residual) instead of a generic error.
fn redact_error(e: agentcowork_office::PdfError) -> String {
    match e {
        agentcowork_office::PdfError::Unremovable(findings) => format!(
            "redaction refused — {} item(s) intersect the area but cannot be removed: {}",
            findings.len(),
            findings
                .iter()
                .map(|f| f.describe())
                .collect::<Vec<_>>()
                .join("; ")
        ),
        agentcowork_office::PdfError::RemovalUnproven(hits) => format!(
            "redaction refused — the target text is still present after the pass: {:?}",
            hits
        ),
        other => other.to_string(),
    }
}

/// Optional human-fidelity tier: open the file in the system LibreOffice.
#[tauri::command]
pub fn office_open_external(path: String) -> Result<serde_json::Value, String> {
    let path = crate::control::floor_user_file(&path)?;
    let soffice = agentcowork_office::find_soffice().ok_or_else(|| {
        "LibreOffice (soffice) is not installed — optional human-fidelity viewer".to_string()
    })?;
    std::process::Command::new(soffice)
        .arg(path.as_os_str())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "path": path.display().to_string() }))
}
