//! P36 (E2) — diagnostic reads: console / network / perf on the existing CDP
//! session. The collectors are pure accumulators over CDP-ish event payloads
//! (a `serde_json::Value` in, a typed row out), so the live Chrome session
//! and the mock tests share one code path.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticKind {
    Console,
    Network,
    Perf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleMessage {
    pub level: String,
    pub text: String,
    pub url: Option<String>,
    pub line: Option<u64>,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkRequest {
    /// CDP `requestId` — the durable key across the request lifecycle.
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub mime_type: Option<String>,
    /// Duration in ms once finished; 0 while in flight.
    pub at_ms: u64,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfMetric {
    pub name: String,
    pub value: f64,
}

/// The consolidated diagnostic surface for one tab session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticReader {
    pub console: Vec<ConsoleMessage>,
    pub network: Vec<NetworkRequest>,
    pub perf: Vec<PerfMetric>,
}

impl DiagnosticReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one CDP event (the raw params object of `Runtime.consoleAPICalled`
    /// / `Runtime.exceptionThrown` / `Network.requestWillBeSent` /
    /// `Network.responseReceived` / `Network.loadingFinished` /
    /// `Performance.metrics`).
    pub fn feed(&mut self, method: &str, params: &Value, at_ms: u64) {
        match method {
            "Runtime.consoleAPICalled" => {
                let level = params["type"].as_str().unwrap_or("log").to_string();
                let text = params["args"]
                    .as_array()
                    .map(|args| {
                        args.iter()
                            .map(|a| {
                                a["value"]
                                    .as_str()
                                    .unwrap_or(a["description"].as_str().unwrap_or(""))
                                    .to_string()
                            })
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    self.console.push(ConsoleMessage {
                        level: level.clone(),
                        text,
                        url: params["url"].as_str().map(String::from),
                        line: params["lineNumber"].as_u64().map(|l| l + 1),
                        at_ms,
                    });
                }
            }
            "Runtime.exceptionThrown" => {
                let desc = params["exceptionDetails"]["text"]
                    .as_str()
                    .unwrap_or("exception");
                let text = format!("Uncaught {desc}");
                self.console.push(ConsoleMessage {
                    level: "error".into(),
                    text,
                    url: params["exceptionDetails"]["url"].as_str().map(String::from),
                    line: params["exceptionDetails"]["lineNumber"]
                        .as_u64()
                        .map(|l| l + 1),
                    at_ms,
                });
            }
            "Network.requestWillBeSent" => {
                let request_id = params["requestId"].as_str().unwrap_or("").to_string();
                let url = params["request"]["url"].as_str().unwrap_or("").to_string();
                let method = params["request"]["method"]
                    .as_str()
                    .unwrap_or("GET")
                    .to_string();
                if let Some(existing) = self.network.iter_mut().find(|r| r.request_id == request_id)
                {
                    existing.started_ms = Some(at_ms);
                    existing.at_ms = at_ms;
                    return;
                }
                self.network.push(NetworkRequest {
                    request_id,
                    url,
                    method,
                    status: None,
                    mime_type: None,
                    at_ms,
                    started_ms: Some(at_ms),
                    finished_ms: None,
                });
            }
            "Network.responseReceived" => {
                let request_id = params["requestId"].as_str().unwrap_or("");
                if let Some(r) = self.network.iter_mut().find(|r| r.request_id == request_id) {
                    r.status = params["response"]["status"].as_u64().map(|s| s as u16);
                    r.mime_type = params["response"]["mimeType"].as_str().map(String::from);
                }
            }
            "Network.loadingFinished" => {
                let request_id = params["requestId"].as_str().unwrap_or("");
                if let Some(r) = self.network.iter_mut().find(|r| r.request_id == request_id) {
                    if let Some(started) = r.started_ms {
                        r.finished_ms = Some(at_ms);
                        r.at_ms = at_ms.saturating_sub(started);
                    }
                }
            }
            "Performance.metrics" => {
                if let Some(metrics) = params["metrics"].as_array() {
                    for m in metrics {
                        if let (Some(name), Some(value)) = (m["name"].as_str(), m["value"].as_f64())
                        {
                            self.perf.push(PerfMetric {
                                name: name.to_string(),
                                value,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The tab's current diagnostic reads.
    pub fn snapshot(&self) -> (Vec<ConsoleMessage>, Vec<NetworkRequest>, Vec<PerfMetric>) {
        (
            self.console.clone(),
            self.network.clone(),
            self.perf.clone(),
        )
    }

    /// Network requests that never completed (in-flight or failed).
    pub fn incomplete_requests(&self) -> Vec<&NetworkRequest> {
        self.network.iter().filter(|r| r.status.is_none()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn console_and_exception_collected() {
        let mut r = DiagnosticReader::new();
        r.feed(
            "Runtime.consoleAPICalled",
            &json!({"type": "warning", "args": [{"value": "deprecated"}, {"value": "api"}], "url": "https://x/app.js", "lineNumber": 3}),
            1,
        );
        r.feed("Runtime.exceptionThrown", &json!({"exceptionDetails": {"text": "TypeError: x is not a function", "url": "https://x/app.js", "lineNumber": 9}}), 2);
        assert_eq!(r.console.len(), 2);
        assert_eq!(r.console[0].level, "warning");
        assert_eq!(r.console[0].text, "deprecated api");
        assert_eq!(r.console[0].line, Some(4));
        assert_eq!(r.console[1].level, "error");
    }

    #[test]
    fn network_lifecycle_times() {
        let mut r = DiagnosticReader::new();
        r.feed(
            "Network.requestWillBeSent",
            &json!({"requestId": "1", "request": {"url": "https://x/a.js", "method": "GET"}}),
            100,
        );
        r.feed("Network.responseReceived", &json!({"requestId": "1", "response": {"status": 200, "mimeType": "application/javascript"}}), 150);
        r.feed("Network.loadingFinished", &json!({"requestId": "1"}), 300);
        let (_, net, _) = r.snapshot();
        assert_eq!(net.len(), 1);
        assert_eq!(net[0].status, Some(200));
        assert_eq!(net[0].at_ms, 200); // 300 - 100
        assert!(r.incomplete_requests().is_empty());
    }

    #[test]
    fn in_flight_requests_flagged() {
        let mut r = DiagnosticReader::new();
        r.feed(
            "Network.requestWillBeSent",
            &json!({"requestId": "p1", "request": {"url": "https://x/pending", "method": "GET"}}),
            1,
        );
        assert_eq!(r.incomplete_requests().len(), 1);
    }

    #[test]
    fn perf_metrics_collected() {
        let mut r = DiagnosticReader::new();
        r.feed("Performance.metrics", &json!({"metrics": [{"name": "TaskDuration", "value": 12.5}, {"name": "LayoutCount", "value": 3.0}]}), 1);
        assert_eq!(r.perf.len(), 2);
        assert_eq!(r.perf[0].name, "TaskDuration");
    }
}
