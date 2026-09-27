//! Intent classifier (P5.4 — doc 63 §4.14, Vane pattern).
//!
//! Classifies a query into memory / fact / event / document, and emits the
//! Vane routing signals `(needs_research, needs_tools, needs_widgets,
//! rewrite_query)` so the coordinator can run the research + tool signals in
//! parallel and force the final answer to cite its sources.
//!
//! This is the deterministic, model-free core — the "cheap/open model:
//! classification" tier. An LLM classifier can override it; this provides
//! the always-available fallback and the shared type contract.

/// The high-level intent class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    /// About the user's own stored memory / past sessions.
    Memory,
    /// General-knowledge fact lookup.
    Fact,
    /// Something time- or scheduling-bound (meetings, deadlines, reminders).
    Event,
    /// About local documents/files/reports.
    Document,
}

/// A classified intent + its routing signals.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub kind: IntentKind,
    /// A research pass should run (web/corpus search) before answering.
    pub needs_research: bool,
    /// Tool calls are required (open/edit/create/send/search/run…).
    pub needs_tools: bool,
    /// The answer wants a visual (chart/table/dashboard/plot).
    pub needs_widgets: bool,
    /// A rewritten, tool/research-ready query (fillers stripped) when one can
    /// be derived, else `None`.
    pub rewrite_query: Option<String>,
}

impl Intent {
    pub fn new(kind: IntentKind) -> Self {
        Intent {
            kind,
            needs_research: false,
            needs_tools: false,
            needs_widgets: false,
            rewrite_query: None,
        }
    }
}

fn contains_any(lower: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| lower.contains(n))
}

/// Classify a raw query. Deterministic keyword heuristics: enough to route
/// cheaply; the LLM tier can override the class while keeping these signals.
pub fn classify(query: &str) -> Intent {
    let lower = query.to_lowercase();
    let mut intent = Intent::new(classify_kind(&lower));

    // Tool signal — imperative verbs / operations.
    intent.needs_tools = contains_any(
        &lower,
        &[
            "open ",
            "edit ",
            "create ",
            "make ",
            "send ",
            "rename ",
            "move ",
            "delete ",
            "run ",
            "search ",
            "find ",
            "download",
            "upload",
            "install",
            "uninstall",
            "write ",
            "append",
            "organize",
            "sort ",
            "clean up",
            "extract",
            "convert",
            "generate ",
        ],
    );

    // Research signal — external/unknown knowledge.
    intent.needs_research = contains_any(
        &lower,
        &[
            "what is",
            "what are",
            "how do",
            "how does",
            "why ",
            "compare",
            "best ",
            "latest",
            "research",
            "find out",
            "explain",
            "difference between",
            "vs ",
            "versus",
        ],
    );

    // Widget signal — visual output.
    intent.needs_widgets = contains_any(
        &lower,
        &[
            "chart",
            "plot",
            "graph",
            "table",
            "dashboard",
            "visualize",
            "show me",
            "display",
            "histogram",
            "pie ",
        ],
    );

    // Rewritten query: strip polite filler for the tool/research executor.
    intent.rewrite_query = rewrite_query(query);

    intent
}

/// A deterministic execution plan derived from the routing signals (doc 63
/// §4.14 — the parallel research + tool execution the coordinator drives).
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionPlan {
    /// Research-pass actions (web/corpus search) — run in parallel with tools.
    pub research_actions: Vec<String>,
    /// Tool actions (each must pass the permission gate before execution).
    pub tool_actions: Vec<String>,
    /// Widgets to render once the answer is assembled (charts/tables).
    pub widgets: Vec<String>,
}

impl ExecutionPlan {
    pub fn is_empty(&self) -> bool {
        self.research_actions.is_empty() && self.tool_actions.is_empty() && self.widgets.is_empty()
    }
}

/// Derive the execution plan for a classified intent. Research + tool actions
/// are produced as *parallel* batches (both signals run concurrently); widgets
/// render only after the answer is assembled.
pub fn plan_execution(intent: &Intent, query: &str) -> ExecutionPlan {
    let q = intent.rewrite_query.as_deref().unwrap_or(query);
    let mut plan = ExecutionPlan {
        research_actions: Vec::new(),
        tool_actions: Vec::new(),
        widgets: Vec::new(),
    };
    if intent.needs_research {
        plan.research_actions.push(format!("web_search({q})"));
        plan.research_actions.push(format!("corpus_search({q})"));
    }
    if intent.needs_tools {
        plan.tool_actions.push(format!("tool_execute({q})"));
    }
    if intent.needs_widgets {
        plan.widgets.push("render_widget(answer)".to_string());
    }
    plan
}

/// Group plan actions into parallel batches: the research + tool actions form
/// one concurrent group; widgets form a final, sequential-after group.
pub fn parallel_groups(plan: &ExecutionPlan) -> Vec<Vec<String>> {
    let mut groups = Vec::new();
    let mut first = Vec::new();
    first.extend(plan.research_actions.iter().cloned());
    first.extend(plan.tool_actions.iter().cloned());
    if !first.is_empty() {
        groups.push(first);
    }
    if !plan.widgets.is_empty() {
        groups.push(plan.widgets.clone());
    }
    groups
}

fn classify_kind(lower: &str) -> IntentKind {
    if contains_any(
        lower,
        &[
            "remember",
            "what did i say",
            "what did i do",
            "my notes",
            "recall",
            "what do i know",
            "previous",
            "last time",
            "earlier session",
            "my memory",
        ],
    ) {
        return IntentKind::Memory;
    }
    if contains_any(
        lower,
        &[
            "schedule",
            "meeting",
            "appointment",
            "remind",
            "tomorrow",
            "next week",
            "deadline",
            "calendar",
            "event",
            "at 3",
            "at 5pm",
            "on monday",
            "on tuesday",
            "on wednesday",
            "on thursday",
            "on friday",
            "by friday",
            "due ",
        ],
    ) {
        return IntentKind::Event;
    }
    if contains_any(
        lower,
        &[
            ".pdf",
            ".docx",
            ".xlsx",
            ".pptx",
            ".md",
            ".txt",
            ".csv",
            "file ",
            "document",
            "report",
            "spreadsheet",
            "slides",
            "folder",
            "the file",
            "in the doc",
            "attachment",
            "my document",
            "my report",
        ],
    ) {
        return IntentKind::Document;
    }
    IntentKind::Fact
}

/// Strip leading polite filler ("please", "could you", "hey") to produce a
/// cleaner executor query. Returns `None` when nothing changes.
fn rewrite_query(query: &str) -> Option<String> {
    let lower = query.to_lowercase();
    let prefixes: &[&str] = &[
        "please ",
        "could you ",
        "can you ",
        "would you ",
        "hey ",
        "hey, ",
        "hi ",
        "ok ",
        "okay ",
        "so ",
    ];
    for p in prefixes {
        if lower.starts_with(p) {
            let (_, rest) = query.split_at(p.len());
            let trimmed = rest.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_document_intent() {
        let i = classify("What does the Q3 report.docx say about revenue?");
        assert_eq!(i.kind, IntentKind::Document);
    }

    #[test]
    fn classifies_event_intent() {
        let i = classify("Remind me about the meeting tomorrow at 3pm");
        assert_eq!(i.kind, IntentKind::Event);
    }

    #[test]
    fn classifies_memory_intent() {
        let i = classify("What did I say about the launch plan last time?");
        assert_eq!(i.kind, IntentKind::Memory);
    }

    #[test]
    fn classifies_fact_intent() {
        let i = classify("What is the capital of France?");
        assert_eq!(i.kind, IntentKind::Fact);
        assert!(i.needs_research);
        assert!(!i.needs_tools);
    }

    #[test]
    fn tool_and_widget_signals() {
        let i = classify("Create a chart of the monthly sales data");
        assert!(i.needs_tools);
        assert!(i.needs_widgets);
    }

    #[test]
    fn research_signal_from_comparison() {
        let i = classify("Compare DeepSeek vs Qwen for coding agents");
        assert!(i.needs_research);
    }

    #[test]
    fn rewrite_strips_polite_filler() {
        let i = classify("please open the report.pdf");
        assert_eq!(i.rewrite_query.as_deref(), Some("open the report.pdf"));
        // No filler → no rewrite.
        let i2 = classify("open the report.pdf");
        assert_eq!(i2.rewrite_query, None);
    }

    #[test]
    fn plan_runs_research_and_tools_in_parallel() {
        let i = classify("Research and create a report on DeepSeek vs Qwen");
        let plan = plan_execution(&i, "Research and create a report on DeepSeek vs Qwen");
        assert_eq!(plan.research_actions.len(), 2);
        assert_eq!(plan.tool_actions.len(), 1);
        let groups = parallel_groups(&plan);
        // One concurrent group holding research + tool actions together.
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn plan_widgets_render_after_answer() {
        let i = classify("Create a chart of the monthly sales data");
        let plan = plan_execution(&i, "Create a chart of the monthly sales data");
        let groups = parallel_groups(&plan);
        // Group 1: tools; group 2: widgets (after the answer exists).
        assert_eq!(groups.len(), 2);
        assert!(groups[1][0].starts_with("render_widget"));
    }

    #[test]
    fn plan_empty_for_plain_fact_with_no_signals() {
        let i = classify("What is the capital of France?");
        assert!(i.needs_research);
        let plan = plan_execution(&i, "What is the capital of France?");
        assert_eq!(plan.research_actions.len(), 2);
        assert!(plan.tool_actions.is_empty());
        assert!(plan.widgets.is_empty());
    }
}
