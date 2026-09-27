//! P2.10 — injected session recorder (E5, ARCH/08 §8.5, doc 33 §9).
//!
//! Installs a content script via CDP `Page.addScriptToEvaluateOnNewDocument`
//! that captures DOM mutations (debounced), clicks, input, keys, scrolls and
//! navigation, buffers them, and POSTs NDJSON batches to the agentcowork-audit
//! ingest with the `x-recording-*` header contract (doc 33 §9.2). A failed
//! flush flips a sticky gap flag reported on the next successful batch — no
//! fake-complete replays.
//!
//! The script is embedded with the endpoint, tab id and the *current* frame's
//! document id (from `Page.getFrameTree`, which is the chrome document id).
//! After navigation the coordinator re-installs (remove + install) so each
//! document gets its own segment — the same loop as re-snapshotting.

use crate::capture::CdpSession;
use agentcowork_cdp::CdpError;
use serde_json::{Value, json};

/// Marker the injected script exposes on `window` for diagnostics.
/// Renamed with the product (both the injected script and its reader live in
/// this crate, so there is no cross-version contract to preserve).
pub const RECORDER_GLOBAL: &str = "__agentcoworkRecorder";

/// The injected recorder source with the install-time values interpolated.
/// `endpoint`, `tab_id` and `document_id` are JSON-escaped so they can never
/// break out of the string literals.
pub fn recorder_script(endpoint: &str, tab_id: &str, document_id: &str) -> String {
    format!(
        r#"(function () {{
  // AgentCowork session recorder (P2.10) — capture → NDJSON batches → ingest.
  var ENDPOINT = {endpoint_json};
  var TAB_ID = {tab_id_json};
  var DOC_ID = {doc_id_json};
  var BATCH_HEADERS = {{
    'Content-Type': 'application/x-ndjson',
    'x-recording-tab-id': TAB_ID,
    'x-recording-document-id': DOC_ID
  }};
  var buf = [];
  var gap = false;
  var batchCounter = 0;
  var mutationTimer = null;
  var scrollScheduled = false;

  function push(kind, data) {{
    buf.push({{ ts_ms: Date.now(), kind: kind, data: data || {{}} }});
  }}

  function flush() {{
    if (buf.length === 0) {{ return; }}
    var lines = buf;
    buf = [];
    var batchId = DOC_ID + '-' + (++batchCounter) + '-' + Math.random().toString(36).slice(2, 10);
    var headers = Object.assign({{}}, BATCH_HEADERS, {{ 'x-recording-batch-id': batchId }});
    if (gap) {{
      headers['x-recording-gap'] = '1';
      gap = false;
    }}
    var body = lines.map(function (ev) {{
      return JSON.stringify({{ seq: 0, ts_ms: ev.ts_ms, kind: ev.kind, data: ev.data }});
    }}).join('\n');
    // Injected scripts are async; failures flip sticky gap (reported next batch).
    fetch(ENDPOINT, {{ method: 'POST', headers: headers, body: body, keepalive: true }})
      .then(function (r) {{ if (!r.ok) {{ gap = true; }} }})
      .catch(function () {{ gap = true; }});
  }}

  function scheduleMutation() {{
    if (mutationTimer) {{ return; }}
    mutationTimer = setTimeout(function () {{
      mutationTimer = null;
      push('dom_mutation', {{ depth: 1 }});
    }}, 400);
  }}

  document.addEventListener('click', function (e) {{
    var t = e.target;
    push('click', {{
      tag: t && t.tagName ? t.tagName.toLowerCase() : null,
      id: t && t.id ? t.id : null,
      cls: t && t.className && typeof t.className === 'string' ? t.className.slice(0, 120) : null
    }});
  }}, true);

  document.addEventListener('input', function (e) {{
    var t = e.target;
    push('input', {{
      tag: t && t.tagName ? t.tagName.toLowerCase() : null,
      name: t && t.name ? t.name : null
    }});
  }}, true);

  document.addEventListener('keydown', function (e) {{
    push('input', {{ key: e.key ? e.key.slice(0, 20) : null, code: e.code || null }});
  }}, true);

  window.addEventListener('scroll', function () {{
    if (scrollScheduled) {{ return; }}
    scrollScheduled = true;
    requestAnimationFrame(function () {{
      scrollScheduled = false;
      push('scroll', {{
        x: Math.round(window.scrollX),
        y: Math.round(window.scrollY),
        dh: document.documentElement ? document.documentElement.scrollHeight : null
      }});
    }});
  }}, true);

  var observer = null;
  if (typeof MutationObserver !== 'undefined') {{
    observer = new MutationObserver(scheduleMutation);
    observer.observe(document.documentElement, {{ childList: true, subtree: true, attributes: true }});
  }}

  window.addEventListener('pagehide', flush, true);
  window.addEventListener('beforeunload', flush, true);

  window[{global_json}] = {{ flush: flush, gap: function () {{ return gap; }} }};
}})();"#,
        endpoint_json = serde_json::to_string(endpoint).unwrap_or_default(),
        tab_id_json = serde_json::to_string(tab_id).unwrap_or_default(),
        doc_id_json = serde_json::to_string(document_id).unwrap_or_default(),
        global_json = serde_json::to_string(RECORDER_GLOBAL).unwrap_or_default(),
    )
}

/// Installs the recorder on a tab/frame session and returns the script
/// identifier (for later removal). Re-install after navigation to start a
/// new per-document segment.
pub fn install_recorder<C: CdpSession>(
    client: &C,
    session_id: Option<&str>,
    endpoint: &str,
    tab_id: &str,
) -> Result<(String, String), CdpError> {
    // The current frame id from the frame tree *is* the chrome document id
    // (doc 33 §9.2 validates these as chrome document ids).
    let document_id = current_document_id(client, session_id)?;
    let source = recorder_script(endpoint, tab_id, &document_id);
    let sid = match session_id {
        Some(s) => s.to_string(),
        None => {
            return Err(CdpError::Protocol {
                code: -1,
                message: "recorder requires a session (tab)".into(),
            });
        }
    };
    let resp = client.call_session(
        &sid,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": source }),
    )?;
    let identifier = resp
        .get("identifier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CdpError::Protocol {
            code: -1,
            message: "addScriptToEvaluateOnNewDocument returned no identifier".into(),
        })?
        .to_string();
    Ok((identifier, document_id))
}

/// Removes a previously installed recorder script.
pub fn remove_recorder<C: CdpSession>(
    client: &C,
    session_id: &str,
    identifier: &str,
) -> Result<(), CdpError> {
    client.call_session(
        session_id,
        "Page.removeScriptToEvaluateOnNewDocument",
        json!({ "identifier": identifier }),
    )?;
    Ok(())
}

/// The top frame's id via `Page.getFrameTree` (best-effort; empty on error).
fn current_document_id<C: CdpSession>(
    client: &C,
    session_id: Option<&str>,
) -> Result<String, CdpError> {
    let resp = match session_id {
        Some(sid) => client.call_session(sid, "Page.getFrameTree", Value::Null),
        None => client.call("Page.getFrameTree", Value::Null),
    }?;
    let id = resp
        .pointer("/frameTree/frame/id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Err(CdpError::Protocol {
            code: -1,
            message: "Page.getFrameTree returned no frame id".into(),
        });
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CdpSession;
    use agentcowork_cdp::Session;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockCdp {
        calls: Mutex<Vec<(Option<String>, String, Value)>>,
    }

    impl CdpSession for MockCdp {
        fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
            self.calls
                .lock()
                .unwrap()
                .push((None, method.into(), params));
            Ok(json!({}))
        }
        fn call_session(
            &self,
            session_id: &str,
            method: &str,
            params: Value,
        ) -> Result<Value, CdpError> {
            self.calls
                .lock()
                .unwrap()
                .push((Some(session_id.into()), method.into(), params));
            match method {
                "Page.getFrameTree" => Ok(json!({
                    "frameTree": { "frame": { "id": "DOC-ABC-123" } }
                })),
                "Page.addScriptToEvaluateOnNewDocument" => Ok(json!({ "identifier": "rec-1" })),
                _ => Ok(json!({})),
            }
        }
        fn attach(&self, _target_id: &str) -> Result<Session, CdpError> {
            Err(CdpError::Protocol {
                code: -1,
                message: "no attach in mock".into(),
            })
        }
        fn drain_events(&self) -> Vec<agentcowork_cdp::CdpEvent> {
            Vec::new()
        }
    }

    fn calls_map(m: &MockCdp) -> HashMap<String, Vec<Value>> {
        let mut map: HashMap<String, Vec<Value>> = HashMap::new();
        for (_, method, params) in m.calls.lock().unwrap().iter() {
            map.entry(method.clone()).or_default().push(params.clone());
        }
        map
    }

    #[test]
    fn install_embeds_endpoint_and_document_id() {
        let m = MockCdp::default();
        let (identifier, doc_id) =
            install_recorder(&m, Some("sess-1"), "http://127.0.0.1:8125/ingest", "tab-9").unwrap();
        assert_eq!(identifier, "rec-1");
        assert_eq!(doc_id, "DOC-ABC-123");
        let map = calls_map(&m);
        let params = &map["Page.addScriptToEvaluateOnNewDocument"][0];
        let source = params["source"].as_str().unwrap();
        assert!(source.contains("http://127.0.0.1:8125/ingest"));
        assert!(source.contains("DOC-ABC-123"));
        assert!(source.contains("tab-9"));
        // The full x-recording header contract is present.
        assert!(source.contains("x-recording-batch-id"));
        assert!(source.contains("x-recording-document-id"));
        assert!(source.contains("x-recording-gap"));
        assert!(source.contains("MutationObserver"));
        assert!(source.contains(RECORDER_GLOBAL));
        assert!(source.contains("keepalive"));
    }

    #[test]
    fn script_cannot_break_out_of_interpolation() {
        let src = recorder_script(
            "https://x.example/path\"); evil(); //",
            "tab\"-1",
            "doc\"-1",
        );
        assert!(src.contains("evil()"));
        // The malicious text must be inside a quoted string, not raw code:
        // find the ENDPOINT literal and confirm it is quoted.
        let marker = "var ENDPOINT = ";
        let rest = &src[src.find(marker).unwrap() + marker.len()..];
        assert!(rest.starts_with('"'), "endpoint must be a quoted literal");
    }

    #[test]
    fn remove_calls_remove_script() {
        let m = MockCdp::default();
        remove_recorder(&m, "sess-1", "rec-1").unwrap();
        let map = calls_map(&m);
        let params = &map["Page.removeScriptToEvaluateOnNewDocument"][0];
        assert_eq!(params["identifier"], "rec-1");
    }

    #[test]
    fn install_requires_session() {
        let m = MockCdp::default();
        assert!(install_recorder(&m, None, "http://x", "t").is_err());
    }

    #[test]
    fn recorder_script_has_capture_set_and_throttles() {
        let src = recorder_script("http://x", "t", "d");
        for marker in [
            "addEventListener('click'",
            "addEventListener('input'",
            "addEventListener('keydown'",
            "addEventListener('scroll'",
            "MutationObserver",
            "pagehide",
            "beforeunload",
        ] {
            assert!(src.contains(marker), "missing {marker}");
        }
        // Scroll is throttled via requestAnimationFrame; mutations debounced.
        assert!(src.contains("requestAnimationFrame"));
        assert!(src.contains("setTimeout"));
    }
}
