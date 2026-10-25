use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationSession {
    pub pid: u32,
    pub token: String,
    pub exe_path: String,
    pub config_dir: String,
    pub window_labels: Vec<String>,
    pub ready_window_labels: Vec<String>,
    pub started_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationRequest {
    pub request_id: String,
    pub token: String,
    pub window: String,
    pub action: UiAutomationAction,
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiAutomationAction {
    Query,
    Click,
    ContextClick,
    Hover,
    SetValue,
    TypeText,
    Backend,
    Terminal,
    Pointer,
    Key,
}

impl UiAutomationAction {
    /// #944 - exhaustive on purpose. A new variant is a COMPILE error here, and the
    /// `None` terminator bounds the walk, so a variant cannot be silently left out of
    /// the iteration the way a hand-written `[UiAutomationAction; N]` array lets it be
    /// (name the variant in the forced match arm, forget the array, and the parity test
    /// still passes).
    #[cfg(test)]
    fn next_variant(self) -> Option<Self> {
        Some(match self {
            Self::Query => Self::Click,
            Self::Click => Self::ContextClick,
            Self::ContextClick => Self::Hover,
            Self::Hover => Self::SetValue,
            Self::SetValue => Self::TypeText,
            Self::TypeText => Self::Backend,
            Self::Backend => Self::Terminal,
            Self::Terminal => Self::Pointer,
            Self::Pointer => Self::Key,
            Self::Key => return None,
        })
    }

    /// Residual hole, stated so nobody trusts this further than it goes: the walk is
    /// SEEDED by hand at `Self::Query`. A variant inserted at the HEAD (before `Query`,
    /// wired `New => Query`) is compile-forced into `next_variant` and into
    /// `action_wire_name`, and yet is never yielded here, so the parity test below would
    /// not see it. It only bites a Rust-ONLY head-insertion: add the same member to the
    /// types.ts union and the set comparison goes red at once. Appending a variant (the
    /// normal case, and what #944 itself did) is fully covered. Closing the hole outright
    /// needs a derive macro (strum) or a hand-written length, and a hand-written length is
    /// the exact weakness this walk replaced.
    #[cfg(test)]
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::Query), |action| action.next_variant())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationResponse {
    pub ok: bool,
    pub request_id: String,
    pub window: String,
    pub action: UiAutomationAction,
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_windows: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

impl UiAutomationResponse {
    pub(crate) fn minimal_error(
        request_id: &str,
        window: &str,
        action: UiAutomationAction,
        selector: &str,
        error: &str,
        message: &str,
    ) -> Self {
        Self {
            ok: false,
            request_id: request_id.to_string(),
            window: window.to_string(),
            action,
            selector: selector.to_string(),
            target: None,
            error: Some(error.to_string()),
            message: Some(message.to_string()),
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
        }
    }

    pub(crate) fn error_for_request(
        request: &UiAutomationRequest,
        error: &str,
        message: &str,
    ) -> Self {
        Self::minimal_error(
            &request.request_id,
            &request.window,
            request.action,
            &request.selector,
            error,
            message,
        )
    }
}

#[derive(Debug)]
pub(crate) struct CliRequest {
    pub(crate) window: String,
    pub(crate) selector: String,
    pub(crate) action: UiAutomationAction,
    pub(crate) value: Option<String>,
    pub(crate) timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiTerminalOperation {
    Query,
    Top,
    Bottom,
    Line(i32),
    Lines(i32),
    Pages(i32),
}

pub(crate) fn parse_terminal_operation(value: &str) -> Option<UiTerminalOperation> {
    match value {
        "query" => return Some(UiTerminalOperation::Query),
        "top" => return Some(UiTerminalOperation::Top),
        "bottom" => return Some(UiTerminalOperation::Bottom),
        _ => {}
    }

    let parse_unsigned = |operand: &str| {
        let bytes = operand.as_bytes();
        let canonical = bytes == b"0"
            || (bytes
                .first()
                .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
                && bytes[1..].iter().all(u8::is_ascii_digit));
        canonical.then(|| operand.parse::<i32>().ok()).flatten()
    };
    let parse_signed = |operand: &str| {
        let bytes = operand.as_bytes();
        let canonical_unsigned = bytes == b"0"
            || (bytes
                .first()
                .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
                && bytes[1..].iter().all(u8::is_ascii_digit));
        let canonical_negative = bytes.first() == Some(&b'-')
            && bytes.get(1).is_some_and(|byte| matches!(byte, b'1'..=b'9'))
            && bytes[2..].iter().all(u8::is_ascii_digit);
        (canonical_unsigned || canonical_negative)
            .then(|| operand.parse::<i32>().ok())
            .flatten()
    };

    if let Some(operand) = value.strip_prefix("line:") {
        return parse_unsigned(operand).map(UiTerminalOperation::Line);
    }
    if let Some(operand) = value.strip_prefix("lines:") {
        return parse_signed(operand).map(UiTerminalOperation::Lines);
    }
    if let Some(operand) = value.strip_prefix("pages:") {
        return parse_signed(operand).map(UiTerminalOperation::Pages);
    }
    None
}

pub(crate) fn valid_terminal_session_id(session_id: &str) -> bool {
    !session_id.is_empty() && session_id.len() <= 256 && !session_id.chars().any(char::is_control)
}

pub(crate) fn valid_pointer_operation(op: &str) -> bool {
    matches!(
        op,
        "down" | "move" | "move:top" | "move:bottom" | "up" | "cancel"
    )
}

pub(crate) fn valid_key_chord(chord: &str) -> bool {
    const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Meta"];
    const KEYS: [&str; 10] = [
        "Escape",
        "Enter",
        "Space",
        "Tab",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Home",
        "End",
    ];
    let mut parts: Vec<&str> = chord.split('+').collect();
    let Some(key) = parts.pop() else {
        return false;
    };
    // Each modifier at most once and in canonical order: a strictly increasing index.
    let mut next_modifier = 0;
    for part in parts {
        match MODIFIERS[next_modifier..].iter().position(|m| *m == part) {
            Some(offset) => next_modifier += offset + 1,
            None => return false,
        }
    }
    KEYS.contains(&key)
}
