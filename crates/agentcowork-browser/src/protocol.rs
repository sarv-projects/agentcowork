//! E17 — multi-protocol action parsing (doc 63 §4.3, skyvern
//! `parse_actions.py` pattern).
//!
//! Per-provider action-protocol adapters map a BYOK provider's action format
//! (native / Anthropic CUA / OpenAI CUA / UI-TARS) onto one canonical
//! [`ParsedAction`], which the same browser layer lowers to [`ActKind`].
//! This is what lets **any** provider's action format drive the browser.

use crate::actions::{ActKind, ScrollDirection};
use serde_json::Value;
use thiserror::Error;

/// Which provider action protocol an input is encoded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionProtocol {
    /// Our own `ActKind` JSON (serde tag `kind`).
    Native,
    /// Anthropic `computer` tool input (`{"action": …}`).
    AnthropicCua,
    /// OpenAI `computer_call` action (`{"type": …}`).
    OpenAiCua,
    /// ByteDance UI-TARS action (`{"action": …}`).
    UiTars,
}

/// A canonical, provider-agnostic browser action.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedAction {
    /// Already-native `ActKind` (from `ActionProtocol::Native`).
    Act(ActKind),
    /// Click at (x, y); `count` is 1 or 2 (double-click).
    Click { x: f64, y: f64, count: u32 },
    /// Type `text`; `at = None` means "at the current focus".
    Type {
        text: String,
        at: Option<(f64, f64)>,
    },
    /// A key press (Enter, Tab, Escape, or a hotkey like `ctrl+a`).
    Press { key: String },
    /// Scroll by `amount` px in `direction` (provider default when 0).
    Scroll {
        direction: ScrollDirection,
        amount: f64,
    },
    /// Drag from → to.
    Drag { from: (f64, f64), to: (f64, f64) },
    /// Move the cursor to (x, y).
    Hover { x: f64, y: f64 },
    /// Take a screenshot (handled by the `screenshot` tool).
    Screenshot,
    /// Wait `ms`.
    Wait { ms: u64 },
    /// The provider signals the task is finished (UI-TARS `finished`).
    Finished,
}

impl ParsedAction {
    /// Lower to an `ActKind`. `cursor` supplies the coordinate for focus-based
    /// typing. Returns `None` for non-input actions (screenshot/wait/finished).
    pub fn to_act_kind(&self, cursor: (f64, f64)) -> Option<ActKind> {
        match self {
            ParsedAction::Act(act) => Some(act.clone()),
            ParsedAction::Click { x, y, .. } => Some(ActKind::ClickAt { x: *x, y: *y }),
            ParsedAction::Type { text, at } => {
                let (x, y) = at.unwrap_or(cursor);
                Some(ActKind::TypeAt {
                    x,
                    y,
                    text: text.clone(),
                })
            }
            ParsedAction::Press { key } => Some(ActKind::Press { key: key.clone() }),
            ParsedAction::Scroll { direction, .. } => Some(ActKind::Scroll {
                direction: *direction,
            }),
            ParsedAction::Drag { from, to } => Some(ActKind::DragAt {
                from_x: from.0,
                from_y: from.1,
                to_x: to.0,
                to_y: to.1,
            }),
            ParsedAction::Hover { x, y } => Some(ActKind::HoverAt { x: *x, y: *y }),
            ParsedAction::Screenshot | ParsedAction::Wait { .. } | ParsedAction::Finished => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ActionParseError {
    #[error("missing field {0}")]
    MissingField(&'static str),
    #[error("invalid coordinate: {0}")]
    InvalidCoordinate(String),
    #[error("unknown action {0:?}")]
    UnknownAction(String),
    #[error("unsupported action {0:?}")]
    Unsupported(String),
}

/// Parse a provider action into a canonical [`ParsedAction`].
pub fn parse_action(
    protocol: ActionProtocol,
    input: &Value,
) -> Result<ParsedAction, ActionParseError> {
    match protocol {
        ActionProtocol::Native => serde_json::from_value::<ActKind>(input.clone())
            .map(ParsedAction::Act)
            .map_err(|e| ActionParseError::UnknownAction(e.to_string())),
        ActionProtocol::AnthropicCua => parse_anthropic_cua(input),
        ActionProtocol::OpenAiCua => parse_openai_cua(input),
        ActionProtocol::UiTars => parse_ui_tars(input),
    }
}

// ---------------------------------------------------------------------------
// Coordinate helpers
// ---------------------------------------------------------------------------

fn coord_array(v: &Value) -> Result<(f64, f64), ActionParseError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ActionParseError::InvalidCoordinate(v.to_string()))?;
    if arr.len() < 2 {
        return Err(ActionParseError::InvalidCoordinate(v.to_string()));
    }
    let x = arr[0]
        .as_f64()
        .ok_or_else(|| ActionParseError::InvalidCoordinate(arr[0].to_string()))?;
    let y = arr[1]
        .as_f64()
        .ok_or_else(|| ActionParseError::InvalidCoordinate(arr[1].to_string()))?;
    Ok((x, y))
}

fn coord_fields(v: &Value) -> Result<(f64, f64), ActionParseError> {
    let x = v
        .get("x")
        .and_then(Value::as_f64)
        .ok_or(ActionParseError::MissingField("x"))?;
    let y = v
        .get("y")
        .and_then(Value::as_f64)
        .ok_or(ActionParseError::MissingField("y"))?;
    Ok((x, y))
}

/// A coordinate given either as `[x, y]` or `{"x":..,"y":..}`.
fn coord(v: &Value) -> Result<(f64, f64), ActionParseError> {
    if v.is_array() {
        coord_array(v)
    } else {
        coord_fields(v)
    }
}

fn text(v: &Value) -> Result<String, ActionParseError> {
    v.get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(ActionParseError::MissingField("text"))
}

fn scroll_direction(v: &Value) -> Result<ScrollDirection, ActionParseError> {
    let s = v
        .as_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| ActionParseError::InvalidCoordinate(v.to_string()))?;
    match s.as_str() {
        "up" => Ok(ScrollDirection::Up),
        "down" => Ok(ScrollDirection::Down),
        "left" => Ok(ScrollDirection::Left),
        "right" => Ok(ScrollDirection::Right),
        other => Err(ActionParseError::UnknownAction(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Anthropic CUA (`computer` tool input)
// ---------------------------------------------------------------------------

fn parse_anthropic_cua(input: &Value) -> Result<ParsedAction, ActionParseError> {
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or(ActionParseError::MissingField("action"))?;
    match action {
        "left_click" | "click" => {
            let (x, y) = coord(
                input
                    .get("coordinate")
                    .ok_or(ActionParseError::MissingField("coordinate"))?,
            )?;
            Ok(ParsedAction::Click { x, y, count: 1 })
        }
        "double_click" => {
            let (x, y) = coord(
                input
                    .get("coordinate")
                    .ok_or(ActionParseError::MissingField("coordinate"))?,
            )?;
            Ok(ParsedAction::Click { x, y, count: 2 })
        }
        "type" => Ok(ParsedAction::Type {
            text: text(input)?,
            at: None,
        }),
        "key" => Ok(ParsedAction::Press { key: text(input)? }),
        "scroll" => {
            let direction = scroll_direction(
                input
                    .get("scroll_direction")
                    .ok_or(ActionParseError::MissingField("scroll_direction"))?,
            )?;
            Ok(ParsedAction::Scroll {
                direction,
                amount: 0.0,
            })
        }
        "mouse_move" | "hover" => {
            let (x, y) = coord(
                input
                    .get("coordinate")
                    .ok_or(ActionParseError::MissingField("coordinate"))?,
            )?;
            Ok(ParsedAction::Hover { x, y })
        }
        "screenshot" => Ok(ParsedAction::Screenshot),
        "wait" => Ok(ParsedAction::Wait {
            ms: input.get("time").and_then(Value::as_u64).unwrap_or(0),
        }),
        "right_click" | "middle_click" | "left_click_drag" | "cursor_position" => {
            Err(ActionParseError::Unsupported(action.to_string()))
        }
        other => Err(ActionParseError::UnknownAction(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// OpenAI CUA (`computer_call` action)
// ---------------------------------------------------------------------------

fn parse_openai_cua(input: &Value) -> Result<ParsedAction, ActionParseError> {
    let typ = input
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ActionParseError::MissingField("type"))?;
    match typ {
        "click" => {
            let (x, y) = coord_fields(input)?;
            Ok(ParsedAction::Click { x, y, count: 1 })
        }
        "double_click" => {
            let (x, y) = coord_fields(input)?;
            Ok(ParsedAction::Click { x, y, count: 2 })
        }
        "type" => Ok(ParsedAction::Type {
            text: text(input)?,
            at: None,
        }),
        "keypress" => {
            let keys = input
                .get("keys")
                .and_then(Value::as_array)
                .ok_or(ActionParseError::MissingField("keys"))?;
            let key = keys
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("+");
            Ok(ParsedAction::Press { key })
        }
        "scroll" => {
            let sx = input.get("scroll_x").and_then(Value::as_f64).unwrap_or(0.0);
            let sy = input.get("scroll_y").and_then(Value::as_f64).unwrap_or(0.0);
            let direction = if sy < 0.0 {
                ScrollDirection::Up
            } else if sy > 0.0 {
                ScrollDirection::Down
            } else if sx < 0.0 {
                ScrollDirection::Left
            } else {
                ScrollDirection::Right
            };
            Ok(ParsedAction::Scroll {
                direction,
                amount: sy.abs().max(sx.abs()),
            })
        }
        "drag" => {
            let path = input
                .get("path")
                .and_then(Value::as_array)
                .ok_or(ActionParseError::MissingField("path"))?;
            if path.len() < 2 {
                return Err(ActionParseError::MissingField("path"));
            }
            let from = coord(&path[0])?;
            let to = coord(&path[path.len() - 1])?;
            Ok(ParsedAction::Drag { from, to })
        }
        "wait" => Ok(ParsedAction::Wait {
            ms: input.get("ms").and_then(Value::as_u64).unwrap_or(0),
        }),
        "screenshot" => Ok(ParsedAction::Screenshot),
        other => Err(ActionParseError::UnknownAction(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// UI-TARS
// ---------------------------------------------------------------------------

fn parse_ui_tars(input: &Value) -> Result<ParsedAction, ActionParseError> {
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or(ActionParseError::MissingField("action"))?;
    match action {
        "click" => {
            let (x, y) = coord(
                input
                    .get("coordinate")
                    .ok_or(ActionParseError::MissingField("coordinate"))?,
            )?;
            Ok(ParsedAction::Click { x, y, count: 1 })
        }
        "type" => Ok(ParsedAction::Type {
            text: text(input)?,
            at: None,
        }),
        "scroll" => {
            let direction = scroll_direction(
                input
                    .get("direction")
                    .ok_or(ActionParseError::MissingField("direction"))?,
            )?;
            Ok(ParsedAction::Scroll {
                direction,
                amount: 0.0,
            })
        }
        "hotkey" => {
            let keys = input
                .get("keys")
                .and_then(Value::as_array)
                .ok_or(ActionParseError::MissingField("keys"))?;
            let key = keys
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("+");
            Ok(ParsedAction::Press { key })
        }
        "drag" => {
            let from = coord(
                input
                    .get("coordinate")
                    .ok_or(ActionParseError::MissingField("coordinate"))?,
            )?;
            let to = input.get("to").map(coord).transpose()?.unwrap_or(from);
            Ok(ParsedAction::Drag { from, to })
        }
        "wait" => Ok(ParsedAction::Wait {
            ms: input.get("time").and_then(Value::as_u64).unwrap_or(0),
        }),
        "screenshot" => Ok(ParsedAction::Screenshot),
        "finished" => Ok(ParsedAction::Finished),
        other => Err(ActionParseError::UnknownAction(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_roundtrips_act_kind() {
        let input = json!({"kind": "click_at", "x": 10.0, "y": 20.0});
        let parsed = parse_action(ActionProtocol::Native, &input).unwrap();
        assert!(
            matches!(parsed, ParsedAction::Act(ActKind::ClickAt { x, y }) if x == 10.0 && y == 20.0)
        );
    }

    #[test]
    fn anthropic_click_and_type() {
        let click = json!({"action": "left_click", "coordinate": [5.0, 6.0]});
        assert_eq!(
            parse_action(ActionProtocol::AnthropicCua, &click).unwrap(),
            ParsedAction::Click {
                x: 5.0,
                y: 6.0,
                count: 1
            }
        );
        let type_ = json!({"action": "type", "text": "hello"});
        assert_eq!(
            parse_action(ActionProtocol::AnthropicCua, &type_).unwrap(),
            ParsedAction::Type {
                text: "hello".into(),
                at: None
            }
        );
    }

    #[test]
    fn anthropic_key_and_scroll() {
        let key = json!({"action": "key", "text": "Enter"});
        assert_eq!(
            parse_action(ActionProtocol::AnthropicCua, &key).unwrap(),
            ParsedAction::Press {
                key: "Enter".into()
            }
        );
        let scroll = json!({"action": "scroll", "scroll_direction": "down"});
        assert_eq!(
            parse_action(ActionProtocol::AnthropicCua, &scroll).unwrap(),
            ParsedAction::Scroll {
                direction: ScrollDirection::Down,
                amount: 0.0
            }
        );
    }

    #[test]
    fn anthropic_unsupported_right_click() {
        let input = json!({"action": "right_click", "coordinate": [1.0, 2.0]});
        assert!(matches!(
            parse_action(ActionProtocol::AnthropicCua, &input),
            Err(ActionParseError::Unsupported(_))
        ));
    }

    #[test]
    fn openai_click_type_keypress() {
        let click = json!({"type": "click", "x": 1.0, "y": 2.0, "button": "left"});
        assert_eq!(
            parse_action(ActionProtocol::OpenAiCua, &click).unwrap(),
            ParsedAction::Click {
                x: 1.0,
                y: 2.0,
                count: 1
            }
        );
        let keys = json!({"type": "keypress", "keys": ["ctrl", "a"]});
        assert_eq!(
            parse_action(ActionProtocol::OpenAiCua, &keys).unwrap(),
            ParsedAction::Press {
                key: "ctrl+a".into()
            }
        );
    }

    #[test]
    fn openai_scroll_and_drag() {
        let scroll = json!({"type": "scroll", "scroll_x": 0.0, "scroll_y": -100.0});
        assert_eq!(
            parse_action(ActionProtocol::OpenAiCua, &scroll).unwrap(),
            ParsedAction::Scroll {
                direction: ScrollDirection::Up,
                amount: 100.0
            }
        );
        let drag = json!({"type": "drag", "path": [{"x": 0.0, "y": 0.0}, {"x": 10.0, "y": 5.0}]});
        assert_eq!(
            parse_action(ActionProtocol::OpenAiCua, &drag).unwrap(),
            ParsedAction::Drag {
                from: (0.0, 0.0),
                to: (10.0, 5.0)
            }
        );
    }

    #[test]
    fn ui_tars_finished_and_hotkey() {
        let finished = json!({"action": "finished"});
        assert_eq!(
            parse_action(ActionProtocol::UiTars, &finished).unwrap(),
            ParsedAction::Finished
        );
        let hotkey = json!({"action": "hotkey", "keys": ["ctrl", "a"]});
        assert_eq!(
            parse_action(ActionProtocol::UiTars, &hotkey).unwrap(),
            ParsedAction::Press {
                key: "ctrl+a".into()
            }
        );
    }

    #[test]
    fn lower_maps_to_act_kind() {
        // Focus-based type uses the caller's cursor.
        let p = ParsedAction::Type {
            text: "hi".into(),
            at: None,
        };
        assert_eq!(
            p.to_act_kind((7.0, 8.0)),
            Some(ActKind::TypeAt {
                x: 7.0,
                y: 8.0,
                text: "hi".into()
            })
        );
        // Non-input actions lower to None.
        assert_eq!(ParsedAction::Screenshot.to_act_kind((0.0, 0.0)), None);
        assert_eq!(ParsedAction::Finished.to_act_kind((0.0, 0.0)), None);
    }

    #[test]
    fn missing_required_field_errors() {
        let bad = json!({"action": "left_click"});
        assert!(matches!(
            parse_action(ActionProtocol::AnthropicCua, &bad),
            Err(ActionParseError::MissingField("coordinate"))
        ));
    }
}
