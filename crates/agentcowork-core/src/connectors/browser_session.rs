//! P6.6 — Browser-session connector.
//!
//! Drives Gmail/Notion/Linear through an existing Chrome CDP session. The agent
//! navigates to the service, reads DOM content, and interacts — no API keys
//! needed, just the user's authenticated browser session.
//!
//! Full protocol logic is tested with a mock [`CdpSession`] seam. The live
//! implementation requires a Chrome 136+ isolated profile with CDP enabled
//! (see E13 in the spec — default-profile attachment is not safe).

use super::{CdpSession, TransportError, TransportErrorKind};

/// Supported browser-session targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserTarget {
    Gmail,
    Notion,
    Linear,
    Outlook,
    Custom,
}

impl BrowserTarget {
    pub fn base_url(&self) -> &'static str {
        match self {
            Self::Gmail => "https://mail.google.com",
            Self::Notion => "https://www.notion.so",
            Self::Linear => "https://linear.app",
            Self::Outlook => "https://outlook.live.com",
            Self::Custom => "",
        }
    }
}

/// A browser-read message (from Gmail DOM or similar).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BrowserMessage {
    pub subject: String,
    pub from: String,
    pub snippet: String,
    pub date: String,
    pub url: Option<String>,
}

/// A browser-read page/document (from Notion or Linear).
#[derive(Debug, Clone)]
pub struct BrowserDocument {
    pub title: String,
    pub content: String,
    pub url: String,
    pub source: BrowserTarget,
}

/// Browser-session connector — stateless protocol logic over the CDP seam.
pub struct BrowserSessionConnector<S: CdpSession> {
    session: S,
    target: BrowserTarget,
}

impl<S: CdpSession> BrowserSessionConnector<S> {
    pub fn new(session: S, target: BrowserTarget) -> Self {
        Self { session, target }
    }

    /// Navigate to the target service.
    pub fn navigate_to_target(&self) -> Result<(), TransportError> {
        let url = self.target.base_url();
        if url.is_empty() {
            return Err(TransportError {
                kind: TransportErrorKind::InvalidResponse,
                message: "Custom target requires explicit URL".into(),
            });
        }
        self.session.navigate(url)
    }

    /// Read Gmail inbox — returns the latest messages visible in the DOM.
    pub fn read_gmail_inbox(
        &self,
        max_messages: usize,
    ) -> Result<Vec<BrowserMessage>, TransportError> {
        // Wait for the inbox to load, then extract message rows via JS.
        let js = format!(
            r#"
            (() => {{
                const rows = document.querySelectorAll('tr.zA');
                const msgs = [];
                for (let i = 0; i < Math.min(rows.length, {max}); i++) {{
                    const row = rows[i];
                    const subject = row.querySelector('.bog')?.textContent?.trim() || '';
                    const from = row.querySelector('.yW .bA4')?.getAttribute('name') || row.querySelector('.yW .zF')?.getAttribute('email') || '';
                    const snippet = row.querySelector('.y6')?.textContent?.trim() || '';
                    const date = row.querySelector('.xW.xY span[title]')?.getAttribute('title') || row.querySelector('.xW.xY span')?.textContent || '';
                    msgs.push({{ subject, from, snippet, date }});
                }}
                return JSON.stringify(msgs);
            }})()
            "#,
            max = max_messages
        );
        let result = self.session.evaluate(&js)?;
        let msgs: Vec<BrowserMessage> = serde_json::from_str(&result).unwrap_or_default();
        Ok(msgs)
    }

    /// Read the current Notion page content.
    pub fn read_notion_page(&self) -> Result<BrowserDocument, TransportError> {
        let js = r#"
            (() => {
                const title = document.querySelector('[placeholder="Untitled"]')?.textContent || document.title;
                const blocks = document.querySelectorAll('[data-block-id]');
                const content = Array.from(blocks).map(b => b.textContent?.trim()).filter(Boolean).join('\n');
                return JSON.stringify({ title, content, url: window.location.href });
            })()
        "#;
        let result = self.session.evaluate(js)?;
        let data: serde_json::Value =
            serde_json::from_str(&result).unwrap_or(serde_json::json!({}));
        Ok(BrowserDocument {
            title: data["title"].as_str().unwrap_or("").to_string(),
            content: data["content"].as_str().unwrap_or("").to_string(),
            url: data["url"].as_str().unwrap_or("").to_string(),
            source: BrowserTarget::Notion,
        })
    }

    /// Read the current Linear issue/task list.
    pub fn read_linear_issues(&self) -> Result<Vec<BrowserDocument>, TransportError> {
        let js = r#"
            (() => {
                const issues = document.querySelectorAll('[data-issue-id]');
                const items = [];
                for (const issue of issues) {
                    const title = issue.querySelector('a[href*="/issue/"]')?.textContent?.trim() || '';
                    const url = issue.querySelector('a[href*="/issue/"]')?.href || '';
                    const identifier = issue.querySelector('[class*="identifier"]')?.textContent?.trim() || '';
                    items.push({ title: identifier + ' ' + title, content: '', url, source: 'Linear' });
                }
                return JSON.stringify(items);
            })()
        "#;
        let result = self.session.evaluate(js)?;
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap_or_default();
        Ok(items
            .into_iter()
            .map(|d| BrowserDocument {
                title: d["title"].as_str().unwrap_or("").to_string(),
                content: d["content"].as_str().unwrap_or("").to_string(),
                url: d["url"].as_str().unwrap_or("").to_string(),
                source: BrowserTarget::Linear,
            })
            .collect())
    }

    /// Read Outlook inbox.
    pub fn read_outlook_inbox(
        &self,
        max_messages: usize,
    ) -> Result<Vec<BrowserMessage>, TransportError> {
        let js = format!(
            r#"
            (() => {{
                const items = document.querySelectorAll('[data-testid="wellurMessageList"] [role="row"]');
                const msgs = [];
                for (let i = 0; i < Math.min(items.length, {max}); i++) {{
                    const item = items[i];
                    const subject = item.querySelector('[data-testid="MessageSubject"]')?.textContent?.trim() || '';
                    const from = item.querySelector('[data-testid="MessageSender"]')?.textContent?.trim() || '';
                    const snippet = item.querySelector('[data-testid="MessagePreviewText"]')?.textContent?.trim() || '';
                    const date = item.querySelector('[data-testid="MessageSecondaryLine"]')?.textContent?.trim() || '';
                    msgs.push({{ subject, from, snippet, date }});
                }}
                return JSON.stringify(msgs);
            }})()
            "#,
            max = max_messages
        );
        let result = self.session.evaluate(&js)?;
        let msgs: Vec<BrowserMessage> = serde_json::from_str(&result).unwrap_or_default();
        Ok(msgs)
    }

    /// Click an element by selector (for Takeover/H21 interactions).
    pub fn click_element(&self, selector: &str) -> Result<(), TransportError> {
        let js = format!("document.querySelector('{selector}')?.click(); 'ok'");
        self.session.evaluate(&js)?;
        Ok(())
    }

    /// Type text into an input field.
    pub fn type_text(&self, selector: &str, text: &str) -> Result<(), TransportError> {
        let js = format!(
            "(() => {{ const el = document.querySelector('{selector}'); if(el) {{ el.focus(); el.value = '{text}'; el.dispatchEvent(new Event('input', {{bubbles:true}})); }} return 'ok'; }})()"
        );
        self.session.evaluate(&js)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockCdpSession {
        evaluations: RefCell<Vec<String>>,
    }

    impl MockCdpSession {
        fn with_responses(responses: Vec<String>) -> Self {
            Self {
                evaluations: RefCell::new(responses),
            }
        }
    }

    impl CdpSession for MockCdpSession {
        fn evaluate(&self, _expression: &str) -> Result<String, TransportError> {
            self.evaluations.borrow_mut().pop().ok_or(TransportError {
                kind: TransportErrorKind::Other,
                message: "empty".into(),
            })
        }
        fn navigate(&self, _url: &str) -> Result<(), TransportError> {
            Ok(())
        }
        fn send_command(
            &self,
            _method: &str,
            _params: serde_json::Value,
        ) -> Result<serde_json::Value, TransportError> {
            Ok(serde_json::json!({}))
        }
    }

    #[test]
    fn navigate_to_gmail() {
        let session = MockCdpSession::with_responses(vec![]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Gmail);
        assert!(conn.navigate_to_target().is_ok());
    }

    #[test]
    fn read_gmail_inbox() {
        let msgs_json = serde_json::json!([
            {"subject": "Q3 Report", "from": "alice@example.com", "snippet": "Here is...", "date": "2026-08-20"}
        ]);
        let session = MockCdpSession::with_responses(vec![msgs_json.to_string()]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Gmail);
        let msgs = conn.read_gmail_inbox(5).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].subject, "Q3 Report");
    }

    #[test]
    fn read_notion_page() {
        let page_json = serde_json::json!({
            "title": "My Notes",
            "content": "Some content here",
            "url": "https://notion.so/My-Notes-abc123"
        });
        let session = MockCdpSession::with_responses(vec![page_json.to_string()]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Notion);
        let doc = conn.read_notion_page().unwrap();
        assert_eq!(doc.title, "My Notes");
        assert_eq!(doc.source, BrowserTarget::Notion);
    }

    #[test]
    fn read_linear_issues() {
        let issues_json = serde_json::json!([
            {"title": "ENG-123 Fix login bug", "content": "", "url": "https://linear.app/team/issue/ENG-123", "source": "Linear"}
        ]);
        let session = MockCdpSession::with_responses(vec![issues_json.to_string()]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Linear);
        let issues = conn.read_linear_issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].title.contains("ENG-123"));
    }

    #[test]
    fn click_element() {
        let session = MockCdpSession::with_responses(vec!["ok".into()]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Gmail);
        assert!(conn.click_element("button.send").is_ok());
    }

    #[test]
    fn type_text() {
        let session = MockCdpSession::with_responses(vec!["ok".into()]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Gmail);
        assert!(conn.type_text("input[name=q]", "test query").is_ok());
    }

    #[test]
    fn custom_target_requires_explicit_url() {
        let session = MockCdpSession::with_responses(vec![]);
        let conn = BrowserSessionConnector::new(session, BrowserTarget::Custom);
        assert!(conn.navigate_to_target().is_err());
    }
}
