//! Self-healing tool-parse repair (P1.8 B5 — doc 62): fix malformed tool-call
//! JSON before the 2-parse-failure cloud escalation. Deterministic text
//! surgery, applied in order and each step reported so the caller can decide
//! whether to trust the repaired result:
//!
//! 1. strip markdown code fences (```json … ```),
//! 2. trim leading/trailing garbage to the first `{`/`[` and last `}`/`]`,
//! 3. balance unterminated braces/brackets,
//! 4. drop trailing commas before `}` / `]` (and consecutive `,,`),
//! 5. single→double quote swap **only** when the fragment has no double
//!    quotes at all (a common "quoted-with-single-quotes" failure).

/// The repair outcome: the (possibly fixed) JSON text + whether any step
/// changed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub json: String,
    pub repaired: bool,
}

/// Repair a malformed tool-call JSON string. Never panics on garbage input —
/// the worst case is the input returned unchanged.
pub fn repair_tool_json(raw: &str) -> Repair {
    let mut s = raw.trim().to_string();

    // 1. Code fences.
    s = strip_fences(&s);

    // 2. Trim to the JSON object/array span.
    s = trim_to_json(&s);

    // 5. Single-quote swap (only when no double quotes exist anywhere).
    if !s.contains('"') && s.contains('\'') {
        s = s.replace('\'', "\"");
    }

    // 4. Trailing commas + doubled commas (only inside the JSON span).
    s = strip_trailing_commas(&s);

    // 3. Balance braces/brackets (append missing closers).
    let balanced = balance(&s);
    let json = if balanced != s { balanced } else { s };

    let repaired = json != raw.trim();
    Repair { json, repaired }
}

/// Remove ```json / ``` / ```jsonc fences.
fn strip_fences(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("```jsonc")
        .or_else(|| s.strip_prefix("```json"))
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    s.trim_end_matches("```").trim().to_string()
}

/// Trim to the first `{`/`[` … last `}`/`]` span.
fn trim_to_json(s: &str) -> String {
    let start = s.find(['{', '[']);
    let end = s.rfind(['}', ']']);
    match (start, end) {
        (Some(a), Some(b)) if b >= a => s[a..=b].to_string(),
        _ => s.to_string(),
    }
}

/// Drop trailing commas before `}` / `]` and collapse `,,`.
fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            // Skip a comma that is followed by a closer, or by another comma.
            let followed_by_closer = j < chars.len() && (chars[j] == '}' || chars[j] == ']');
            let followed_by_comma = j < chars.len() && chars[j] == ',';
            if followed_by_closer || followed_by_comma {
                continue;
            }
        }
        out.push(c);
    }
    out.into_iter().collect()
}

/// Append the minimal closing brackets to balance `{`/`[` (ignoring those
/// inside string literals).
fn balance(s: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && in_string {
            escaped = true;
            continue;
        }
        match c {
            '"' => in_string = !in_string,
            '{' | '[' if !in_string => stack.push(c),
            '}' | ']' if !in_string => {
                let expect = if c == '}' { '{' } else { '[' };
                if stack.last() == Some(&expect) {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = s.to_string();
    for open in stack.iter().rev() {
        out.push(if *open == '{' { '}' } else { ']' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_json_is_unchanged() {
        let r = repair_tool_json(r#"{"tool":"WeatherTool"}"#);
        assert!(!r.repaired);
        assert_eq!(r.json, r#"{"tool":"WeatherTool"}"#);
    }

    #[test]
    fn strips_code_fences() {
        let raw = "```json\n{\"tool\":\"x\"}\n```";
        let r = repair_tool_json(raw);
        assert!(r.repaired);
        assert_eq!(r.json, r#"{"tool":"x"}"#);
    }

    #[test]
    fn trims_surrounding_prose() {
        let raw = "Here you go: {\"a\":1} hope that helps";
        let r = repair_tool_json(raw);
        assert_eq!(r.json, r#"{"a":1}"#);
        assert!(r.repaired);
    }

    #[test]
    fn balances_unterminated_braces() {
        let r = repair_tool_json(r#"{"a":{"b":1}"#);
        assert_eq!(r.json, r#"{"a":{"b":1}}"#);
        assert!(r.repaired);
    }

    #[test]
    fn balances_unterminated_brackets() {
        let r = repair_tool_json(r#"[1,2,[3]"#);
        assert_eq!(r.json, r#"[1,2,[3]]"#);
    }

    #[test]
    fn strips_trailing_comma_before_brace() {
        let r = repair_tool_json(r#"{"a":1,}"#);
        assert_eq!(r.json, r#"{"a":1}"#);
        assert!(r.repaired);
    }

    #[test]
    fn strips_trailing_comma_before_bracket() {
        let r = repair_tool_json(r#"[1,2,]"#);
        assert_eq!(r.json, r#"[1,2]"#);
    }

    #[test]
    fn swaps_single_quotes_when_no_double_quotes() {
        let r = repair_tool_json("{'tool':'Weather','q':'nyc'}");
        assert_eq!(r.json, r#"{"tool":"Weather","q":"nyc"}"#);
        assert!(serde_json::from_str::<serde_json::Value>(&r.json).is_ok());
    }

    #[test]
    fn does_not_swap_single_quotes_inside_double_quoted_strings() {
        // The apostrophe inside "don't" must survive.
        let raw = r#"{"q":"don't stop"}"#;
        let r = repair_tool_json(raw);
        assert_eq!(r.json, raw);
    }

    #[test]
    fn repaired_json_parses() {
        let malformed = "```json\n{'name':'city','temp':21,}\n```";
        let r = repair_tool_json(malformed);
        let v: serde_json::Value = serde_json::from_str(&r.json).unwrap();
        assert_eq!(v["name"], "city");
        assert_eq!(v["temp"], 21);
    }

    #[test]
    fn garbage_input_is_returned_not_panicking() {
        let r = repair_tool_json("not json at all");
        assert!(!r.json.is_empty());
    }
}
