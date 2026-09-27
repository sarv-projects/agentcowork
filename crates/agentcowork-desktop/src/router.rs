//! Layer-1-first routing (iyulab pattern): when a real API, filesystem,
//! shell, office engine, or CDP-browser path exists for a goal, the desktop
//! GUI is never pixel-driven. Desktop computer-use is the *last* layer.
//!
//! This is a deterministic classifier (no LLM in the hot path) so the
//! precedence is auditable and testable.

use serde::{Deserialize, Serialize};

/// Which layer should own a goal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Layer {
    /// Files / shell / office / storage / connector APIs exist — use them.
    Api,
    /// A browser goal — stays on the CDP stack (E1–E17), never pixel-guessed.
    BrowserCdp,
    /// A native desktop app with no API — desktop computer-use (E9) applies.
    DesktopGui,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteDecision {
    pub layer: Layer,
    /// Human-readable why (audit line).
    pub reason: String,
}

/// Browser-ecosystem signals that must stay on CDP.
const BROWSER_SIGNALS: &[&str] = &[
    "browser",
    "web page",
    "website",
    "http://",
    "https://",
    "chrome",
    "firefox",
    "edge",
    "web app",
    "online",
    "url",
    "search the web",
    "google",
    "website form",
    "webmail",
];

/// API/first-class signals: a real engine exists for these — never GUI.
const API_SIGNALS: &[&str] = &[
    "file",
    "folder",
    "directory",
    "document",
    "spreadsheet",
    "xlsx",
    "docx",
    "pdf",
    "pptx",
    "word",
    "excel",
    "powerpoint",
    "shell",
    "command",
    "terminal command",
    "script",
    "email",
    "calendar",
    "connector",
    "storage",
    "disk",
    "database",
    "sql",
    "csv",
    "search files",
    "rename file",
    "move file",
    "copy file",
    "delete file",
    "download",
    "upload",
    "send email",
    "create calendar",
];

/// Route a natural-language goal to the owning layer.
pub fn route(goal: &str) -> RouteDecision {
    let g = goal.to_ascii_lowercase();
    if BROWSER_SIGNALS.iter().any(|s| g.contains(s)) {
        return RouteDecision {
            layer: Layer::BrowserCdp,
            reason: "browser-ecosystem goal — CDP stack (E1–E17), never pixel-driven".into(),
        };
    }
    if API_SIGNALS.iter().any(|s| g.contains(s)) {
        return RouteDecision {
            layer: Layer::Api,
            reason: "first-class API/engine exists for this goal — desktop GUI not needed".into(),
        };
    }
    RouteDecision {
        layer: Layer::DesktopGui,
        reason: "native desktop app with no higher layer — E9 applies".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_goals_stay_cdp() {
        for g in [
            "open the website and fill the form",
            "search the web for pricing",
            "check my gmail in the browser",
            "navigate to https://example.com",
        ] {
            assert_eq!(route(g).layer, Layer::BrowserCdp, "{g}");
        }
    }

    #[test]
    fn api_goals_never_pixel_driven() {
        for g in [
            "rename the file report.docx",
            "sum column A of the spreadsheet",
            "run this shell command",
            "send an email to the team",
            "delete the csv file",
            "create a calendar event",
        ] {
            assert_eq!(route(g).layer, Layer::Api, "{g}");
        }
    }

    #[test]
    fn native_app_goals_fall_to_gui() {
        for g in [
            "click save in the drawing app",
            "open the calculator and add two numbers",
            "use the settings window of the installed program",
        ] {
            assert_eq!(route(g).layer, Layer::DesktopGui, "{g}");
        }
    }

    #[test]
    fn layer_precedence_is_cdp_then_api_then_gui() {
        // A browser goal mentioning a file still stays on CDP (browser wins
        // because the whole goal is a web interaction).
        assert_eq!(
            route("download the pdf from the website").layer,
            Layer::BrowserCdp
        );
    }
}
