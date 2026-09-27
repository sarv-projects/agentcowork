//! P42.2 — Google Workspace connector (spec F15 v2): Drive + Docs + Sheets via
//! the official APIs. Gmail already has its own transport
//! ([`super::gmail::GmailConnector`]) — this module is the Drive/Docs/Sheets
//! delta of the Workspace surface, over the same injectable [`HttpTransport`]
//! seam with 401 → refresh → retry.
//!
//! Read-only-first (P42.3): every method here is a read; mutation goes through
//! the official OOXML export → office engine → Guard-2 write-back path (never
//! an API write from the connector). Scopes: `drive.readonly`,
//! `documents.readonly`, `spreadsheets.readonly` — see [`super::scopes`].

use super::{HttpTransport, TokenSource, TransportError, TransportErrorKind, VaultTokenRef};

const DRIVE_BASE: &str = "https://www.googleapis.com/drive/v3";
const DOCS_BASE: &str = "https://docs.googleapis.com/v1";
const SHEETS_BASE: &str = "https://sheets.googleapis.com/v4";

/// A Drive file (metadata).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceDriveFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub folder: bool,
    pub size: Option<String>,
    pub modified_time: Option<String>,
    /// `exportLinks` for Google-native docs (OOXML export → our engine).
    pub export_links: Vec<(String, String)>,
}

/// A Google Docs document's content (simplified).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceDoc {
    pub id: String,
    pub title: String,
    /// Concatenated paragraph text (agent-consumable; structural JSON is the
    /// raw `documents.get` response the caller keeps).
    pub text: String,
}

/// A Google Sheets spreadsheet's values for one range.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSheetValues {
    pub spreadsheet_id: String,
    pub range: String,
    pub values: Vec<Vec<String>>,
}

/// Google Workspace connector — Drive/Docs/Sheets reads over injected seams.
///
/// FIX-01: holds a [`VaultTokenRef`] + [`TokenSource`], never the access token.
pub struct WorkspaceConnector<T: HttpTransport, S: TokenSource> {
    transport: T,
    tokens: S,
    token_ref: VaultTokenRef,
}

impl<T: HttpTransport, S: TokenSource> WorkspaceConnector<T, S> {
    /// Bind the connector to a **vault reference**, not a token.
    pub fn new(transport: T, tokens: S, token_ref: VaultTokenRef) -> Self {
        Self {
            transport,
            tokens,
            token_ref,
        }
    }

    /// Run `f` with the live access token, in-custody.
    fn authenticated<R>(
        &self,
        f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
    ) -> Result<R, TransportError> {
        self.tokens.with_token(&self.token_ref, f)
    }

    fn get_with_refresh(&mut self, url: &str) -> Result<Vec<u8>, TransportError> {
        let transport = &self.transport;
        match self.authenticated(&mut |t| transport.get(url, &[("Authorization", t)])) {
            Err(e) if e.kind == TransportErrorKind::Auth => {
                self.tokens.refresh(&self.token_ref, &mut |t| {
                    transport.get(url, &[("Authorization", t)])
                })
            }
            other => other,
        }
    }

    /// `GET /drive/v3/files?q=...` — list files, `folder: true` for dirs.
    pub fn list_files(
        &mut self,
        page_size: u32,
    ) -> Result<Vec<WorkspaceDriveFile>, TransportError> {
        let url = format!(
            "{DRIVE_BASE}/files?pageSize={page_size}&fields=files(id,name,mimeType,size,modifiedTime,exportLinks)"
        );
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("drive list: {e}"),
        })?;
        Ok(v.get("files")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|f| WorkspaceDriveFile {
                        id: f["id"].as_str().unwrap_or_default().into(),
                        name: f["name"].as_str().unwrap_or_default().into(),
                        mime_type: f["mimeType"].as_str().unwrap_or_default().into(),
                        folder: f["mimeType"]
                            .as_str()
                            .map(|m| m == "application/vnd.google-apps.folder")
                            .unwrap_or(false),
                        size: f["size"].as_str().map(String::from),
                        modified_time: f["modifiedTime"].as_str().map(String::from),
                        export_links: f["exportLinks"]
                            .as_object()
                            .map(|m| {
                                m.iter()
                                    .map(|(k, val)| {
                                        (k.clone(), val.as_str().unwrap_or_default().to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `GET /drive/v3/files/{id}` — one file + its export links.
    pub fn get_file(&mut self, id: &str) -> Result<WorkspaceDriveFile, TransportError> {
        let url = format!(
            "{DRIVE_BASE}/files/{id}?fields=id,name,mimeType,size,modifiedTime,exportLinks"
        );
        let raw = self.get_with_refresh(&url)?;
        let f: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("drive get: {e}"),
        })?;
        Ok(WorkspaceDriveFile {
            id: f["id"].as_str().unwrap_or_default().into(),
            name: f["name"].as_str().unwrap_or_default().into(),
            mime_type: f["mimeType"].as_str().unwrap_or_default().into(),
            folder: f["mimeType"]
                .as_str()
                .map(|m| m == "application/vnd.google-apps.folder")
                .unwrap_or(false),
            size: f["size"].as_str().map(String::from),
            modified_time: f["modifiedTime"].as_str().map(String::from),
            export_links: f["exportLinks"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, val)| (k.clone(), val.as_str().unwrap_or_default().to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// `GET /docs/v1/documents/{id}` — paragraph text of a Google Doc.
    pub fn get_document(&mut self, id: &str) -> Result<WorkspaceDoc, TransportError> {
        let url = format!("{DOCS_BASE}/documents/{id}");
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("docs get: {e}"),
        })?;
        let title = v["title"].as_str().unwrap_or_default().to_string();
        // Flatten body content: paragraph elements → text runs, joined by \n.
        let text = v["body"]["content"]
            .as_array()
            .map(|els| {
                let mut out = String::new();
                for el in els {
                    let mut line = String::new();
                    if let Some(par) = el["paragraph"].as_object() {
                        if let Some(elements) = par["elements"].as_array() {
                            for e in elements {
                                if let Some(tr) = e["textRun"].as_object() {
                                    if let Some(s) = tr["content"].as_str() {
                                        line.push_str(s);
                                    }
                                }
                            }
                        }
                    }
                    out.push_str(&line);
                    out.push('\n');
                }
                out
            })
            .unwrap_or_default();
        Ok(WorkspaceDoc {
            id: id.to_string(),
            title,
            text,
        })
    }

    /// `GET /sheets/v4/spreadsheets/{id}/values/{range}` — cell values.
    pub fn get_sheet_values(
        &mut self,
        spreadsheet_id: &str,
        range: &str,
    ) -> Result<WorkspaceSheetValues, TransportError> {
        let url = format!("{SHEETS_BASE}/spreadsheets/{spreadsheet_id}/values/{range}");
        let raw = self.get_with_refresh(&url)?;
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| TransportError {
            kind: TransportErrorKind::InvalidResponse,
            message: format!("sheets get: {e}"),
        })?;
        Ok(WorkspaceSheetValues {
            spreadsheet_id: spreadsheet_id.to_string(),
            range: v["range"].as_str().unwrap_or(range).to_string(),
            values: v["values"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| {
                            row.as_array()
                                .map(|cells| {
                                    cells
                                        .iter()
                                        .map(|c| {
                                            if c.is_string() {
                                                c.as_str().unwrap_or_default().to_string()
                                            } else {
                                                c.to_string()
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTransport {
        responses: std::cell::RefCell<std::collections::VecDeque<(Vec<u8>, TransportErrorKind)>>,
    }
    impl MockTransport {
        fn ok(json: serde_json::Value) -> Self {
            let mut q = std::collections::VecDeque::new();
            q.push_back((
                serde_json::to_vec(&json).unwrap(),
                TransportErrorKind::Other,
            ));
            Self {
                responses: std::cell::RefCell::new(q),
            }
        }
        fn next(&self) -> Option<(Vec<u8>, TransportErrorKind)> {
            self.responses.borrow_mut().pop_front()
        }
    }
    impl HttpTransport for MockTransport {
        fn post_json(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<Vec<u8>, TransportError> {
            Err(TransportError {
                kind: TransportErrorKind::Other,
                message: "no posts".into(),
            })
        }
        fn get(&self, _url: &str, _headers: &[(&str, &str)]) -> Result<Vec<u8>, TransportError> {
            match self.next() {
                Some((_, TransportErrorKind::Auth)) => Err(TransportError {
                    kind: TransportErrorKind::Auth,
                    message: "401".into(),
                }),
                Some((bytes, _)) => Ok(bytes),
                None => Err(TransportError {
                    kind: TransportErrorKind::InvalidResponse,
                    message: "no mock response".into(),
                }),
            }
        }
    }

    /// FIX-01: a *source*, not a token holder.
    struct NoRefresh;
    impl TokenSource for NoRefresh {
        fn with_token<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("Bearer tok")
        }
        fn refresh<R>(
            &self,
            _ref_: &VaultTokenRef,
            f: &mut dyn FnMut(&str) -> Result<R, TransportError>,
        ) -> Result<R, TransportError> {
            f("Bearer new-token")
        }
    }

    fn workspace_tokens() -> VaultTokenRef {
        VaultTokenRef::new("k-workspace", "google-workspace")
    }

    #[test]
    fn drive_list_parses_and_flags_folders() {
        let t = MockTransport::ok(serde_json::json!({
            "files": [
                { "id": "d1", "name": "Q3.xlsx", "mimeType": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "size": "1024" },
                { "id": "d2", "name": "Docs", "mimeType": "application/vnd.google-apps.folder" }
            ]
        }));
        let mut c = WorkspaceConnector::new(t, NoRefresh, workspace_tokens());
        let files = c.list_files(10).unwrap();
        assert_eq!(files.len(), 2);
        assert!(!files[0].folder);
        assert!(files[1].folder);
        assert_eq!(files[0].size.as_deref(), Some("1024"));
    }

    #[test]
    fn drive_docs_export_links_flow_to_engine() {
        let t = MockTransport::ok(serde_json::json!({
            "id": "doc1",
            "name": "Plan",
            "mimeType": "application/vnd.google-apps.document",
            "exportLinks": {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document": "https://docs.google.com/export?id=doc1"
            }
        }));
        let mut c = WorkspaceConnector::new(t, NoRefresh, workspace_tokens());
        let f = c.get_file("doc1").unwrap();
        assert_eq!(f.name, "Plan");
        let docx = f
            .export_links
            .iter()
            .find(|(m, _)| m.contains("wordprocessingml"));
        assert!(docx.is_some(), "docx OOXML export link must be surfaced");
    }

    #[test]
    fn docs_flattens_paragraphs_and_sheets_parses_values() {
        let t = MockTransport::ok(serde_json::json!({
            "title": "Meeting notes",
            "body": { "content": [
                { "paragraph": { "elements": [ { "textRun": { "content": "Hello " } }, { "textRun": { "content": "world" } } ] } },
                { "paragraph": { "elements": [ { "textRun": { "content": "Second para" } } ] } }
            ] }
        }));
        let mut c = WorkspaceConnector::new(t, NoRefresh, workspace_tokens());
        let doc = c.get_document("doc1").unwrap();
        assert_eq!(doc.title, "Meeting notes");
        assert!(doc.text.contains("Hello world"));
        assert!(doc.text.contains("Second para"));

        let t2 = MockTransport::ok(serde_json::json!({
            "range": "Sheet1!A1:B2",
            "values": [ ["Name", "Qty"], ["Widgets", "3"] ]
        }));
        let mut c2 = WorkspaceConnector::new(t2, NoRefresh, workspace_tokens());
        let s = c2.get_sheet_values("sp1", "Sheet1!A1:B2").unwrap();
        assert_eq!(s.values[0], vec!["Name", "Qty"]);
        assert_eq!(s.values[1][1], "3");
    }
}
