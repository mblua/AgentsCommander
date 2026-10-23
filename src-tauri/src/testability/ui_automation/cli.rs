use super::{
    preflight_error, print_stdout_json, run_cli_request, UiAutomationAction, UiAutomationResponse,
    UiBackendArgs, UiClickArgs, UiContextClickArgs, UiHoverArgs, UiKeyArgs, UiPointerArgs,
    UiQueryArgs, UiSetArgs, UiTerminalArgs, UiTypeArgs, UiWaitArgs, BACKEND_AUTOMATION_WINDOW,
    POLL_MS,
};
use serde_json::Value;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug)]
pub(super) struct CliRequest {
    pub(super) window: String,
    pub(super) selector: String,
    pub(super) action: UiAutomationAction,
    pub(super) value: Option<String>,
    pub(super) timeout_ms: u64,
}

pub fn execute_query(args: UiQueryArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Query,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiTerminalOperation {
    Query,
    Top,
    Bottom,
    Line(i32),
    Lines(i32),
    Pages(i32),
}

pub(super) fn parse_terminal_operation(value: &str) -> Option<UiTerminalOperation> {
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

pub(super) fn valid_terminal_session_id(session_id: &str) -> bool {
    !session_id.is_empty() && session_id.len() <= 256 && !session_id.chars().any(char::is_control)
}

pub(super) fn terminal_cli_request(args: UiTerminalArgs) -> Result<CliRequest, Value> {
    if !valid_terminal_session_id(&args.session_id) {
        return Err(preflight_error(
            "invalid_terminal_session_id",
            "Terminal session id must contain 1 to 256 UTF-8 bytes and no control characters.",
            None,
        ));
    }
    if parse_terminal_operation(&args.operation).is_none() {
        return Err(preflight_error(
            "invalid_terminal_operation",
            "Terminal operation must be query, top, bottom, line:N, lines:N, or pages:N in the \
                signed 32-bit range.",
            None,
        ));
    }

    Ok(CliRequest {
        window: args.window,
        selector: format!("terminal.session.{}", args.session_id),
        action: UiAutomationAction::Terminal,
        value: Some(args.operation),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_terminal(args: UiTerminalArgs) -> i32 {
    match terminal_cli_request(args) {
        Ok(request) => execute_cli(request),
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
}

pub(super) fn valid_pointer_operation(op: &str) -> bool {
    matches!(
        op,
        "down" | "move" | "move:top" | "move:bottom" | "up" | "cancel"
    )
}

pub(super) fn valid_key_chord(chord: &str) -> bool {
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

pub(super) fn pointer_cli_request(args: UiPointerArgs) -> Result<CliRequest, Value> {
    if !valid_pointer_operation(&args.operation) {
        return Err(preflight_error(
            "invalid_pointer_operation",
            "Pointer operation must be down, move, move:top, move:bottom, up, or cancel.",
            None,
        ));
    }
    let target_free = matches!(args.operation.as_str(), "up" | "cancel");
    let selector = match (target_free, args.selector) {
        (false, Some(selector)) => selector,
        (false, None) => {
            return Err(preflight_error(
                "pointer_selector_required",
                "Pointer down and move operations require --selector.",
                None,
            ))
        }
        // Empty like `ui-hover --leave`; the frontend echoes it, so `complete()` still matches.
        (true, None) => String::new(),
        (true, Some(_)) => {
            return Err(preflight_error(
                "pointer_selector_not_allowed",
                "Pointer up and cancel operations do not accept --selector.",
                None,
            ))
        }
    };

    Ok(CliRequest {
        window: args.window,
        selector,
        action: UiAutomationAction::Pointer,
        value: Some(args.operation),
        timeout_ms: args.timeout_ms,
    })
}

pub(super) fn key_cli_request(args: UiKeyArgs) -> Result<CliRequest, Value> {
    if !valid_key_chord(&args.chord) {
        return Err(preflight_error(
            "invalid_key_chord",
            "Key chord must be optional Ctrl+, Alt+, Shift+, Meta+ in that order, then Escape, \
                Enter, Space, Tab, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, Home, or End.",
            None,
        ));
    }

    Ok(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Key,
        value: Some(args.chord),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_pointer(args: UiPointerArgs) -> i32 {
    match pointer_cli_request(args) {
        Ok(request) => execute_cli(request),
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
}

pub fn execute_key(args: UiKeyArgs) -> i32 {
    match key_cli_request(args) {
        Ok(request) => execute_cli(request),
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
}

pub fn execute_click(args: UiClickArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Click,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_context_click(args: UiContextClickArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::ContextClick,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_hover(args: UiHoverArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        // Empty for the target-free leave form. The bridge intercepts `value == "leave"`
        // before it resolves any selector, and the frontend echoes the request's selector
        // back, so the window/action/selector equality check in `complete()` still matches.
        // No line number on purpose: that pointer read :317-321, then :361-364, and rotted
        // both times inside the very commit that wrote it (this file grew above it each
        // time). `fn complete` is the stable anchor; a number here is a comment that lies
        // on a schedule.
        selector: args.selector.unwrap_or_default(),
        action: UiAutomationAction::Hover,
        value: args.leave.then(|| "leave".to_string()),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_set(args: UiSetArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::SetValue,
        value: Some(args.value),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_type(args: UiTypeArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::TypeText,
        value: Some(args.value),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_backend(args: UiBackendArgs) -> i32 {
    execute_cli(CliRequest {
        window: BACKEND_AUTOMATION_WINDOW.to_string(),
        selector: args.selector,
        action: UiAutomationAction::Backend,
        value: args.value,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_wait(args: UiWaitArgs) -> i32 {
    let deadline = Instant::now() + Duration::from_millis(args.timeout_ms);
    let mut last_response: Option<UiAutomationResponse> = None;

    loop {
        let remaining_ms = deadline
            .checked_duration_since(Instant::now())
            .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0);
        if remaining_ms == 0 {
            let timeout = last_response.unwrap_or_else(|| {
                UiAutomationResponse::minimal_error(
                    &Uuid::new_v4().to_string(),
                    &args.window,
                    UiAutomationAction::Query,
                    &args.selector,
                    "timeout",
                    "Automation wait timed out before the selector became available.",
                )
            });
            let mut timeout = timeout;
            timeout.ok = false;
            timeout.error = Some("timeout".to_string());
            timeout.message =
                Some("Automation wait timed out before the selector became available.".to_string());
            timeout.timeout_ms = Some(args.timeout_ms);
            timeout.phase = Some("wait_condition_not_met".to_string());
            print_stdout_json(&timeout);
            return 1;
        }

        let input = CliRequest {
            window: args.window.clone(),
            selector: args.selector.clone(),
            action: UiAutomationAction::Query,
            value: None,
            timeout_ms: remaining_ms.min(500),
        };

        match run_cli_request(&input) {
            Ok(response) if response.ok => {
                print_stdout_json(&response);
                return 0;
            }
            Ok(response) => {
                last_response = Some(response);
            }
            Err(error) => {
                print_stdout_json(&error);
                return 1;
            }
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

pub(super) fn execute_cli(input: CliRequest) -> i32 {
    match run_cli_request(&input) {
        Ok(response) if response.ok => {
            print_stdout_json(&response);
            0
        }
        Ok(response) => {
            print_stdout_json(&response);
            1
        }
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
}
