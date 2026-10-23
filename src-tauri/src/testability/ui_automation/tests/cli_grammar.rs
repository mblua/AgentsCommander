use super::*;

#[test]
fn action_serializes_camel_case() {
    for action in UiAutomationAction::all() {
        assert_eq!(
            serde_json::to_string(&action).unwrap(),
            format!("\"{}\"", action_wire_name(action))
        );
    }
}

#[test]
fn terminal_operation_parser_accepts_exact_i32_boundaries() {
    assert_eq!(
        parse_terminal_operation("query"),
        Some(UiTerminalOperation::Query)
    );
    assert_eq!(
        parse_terminal_operation("top"),
        Some(UiTerminalOperation::Top)
    );
    assert_eq!(
        parse_terminal_operation("bottom"),
        Some(UiTerminalOperation::Bottom)
    );
    assert_eq!(
        parse_terminal_operation("line:0"),
        Some(UiTerminalOperation::Line(0))
    );
    assert_eq!(
        parse_terminal_operation("line:2147483647"),
        Some(UiTerminalOperation::Line(i32::MAX))
    );
    for (prefix, constructor) in [
        (
            "lines:",
            UiTerminalOperation::Lines as fn(i32) -> UiTerminalOperation,
        ),
        (
            "pages:",
            UiTerminalOperation::Pages as fn(i32) -> UiTerminalOperation,
        ),
    ] {
        assert_eq!(
            parse_terminal_operation(&format!("{prefix}-2147483648")),
            Some(constructor(i32::MIN))
        );
        assert_eq!(
            parse_terminal_operation(&format!("{prefix}0")),
            Some(constructor(0))
        );
        assert_eq!(
            parse_terminal_operation(&format!("{prefix}2147483647")),
            Some(constructor(i32::MAX))
        );
    }
}

#[test]
fn terminal_operation_parser_rejects_every_noncanonical_grammar_class() {
    for value in [
        "",
        "QUERY",
        " query",
        "query ",
        "line:",
        "line:+1",
        "line:-1",
        "line:00",
        "line:01",
        "line:-0",
        "line:1.0",
        "line:1e2",
        "line:1:2",
        "line:2147483648",
        "lines:",
        "lines:+1",
        "lines:01",
        "lines:-0",
        "lines:1.0",
        "lines:1e2",
        "lines:1:2",
        "lines:2147483648",
        "lines:-2147483649",
        "pages:+1",
        "pages:01",
        "pages:-0",
        "pages:2147483648",
        "pages:-2147483649",
        "unknown:1",
    ] {
        assert_eq!(
            parse_terminal_operation(value),
            None,
            "unexpectedly accepted {value:?}"
        );
    }
}

#[test]
fn terminal_session_id_validation_uses_exact_utf8_bytes_and_control_rule() {
    assert!(!valid_terminal_session_id(""));
    assert!(!valid_terminal_session_id("bad\nvalue"));
    assert!(!valid_terminal_session_id("bad\u{7f}value"));
    assert!(valid_terminal_session_id("a"));
    assert!(valid_terminal_session_id(&"a".repeat(256)));
    assert!(!valid_terminal_session_id(&"a".repeat(257)));
    assert!(valid_terminal_session_id(&"é".repeat(128)));
    assert!(!valid_terminal_session_id(&"é".repeat(129)));
}

#[test]
fn terminal_cli_request_preserves_session_and_derives_fixed_selector() {
    let session_id = " exact-é session ".to_string();
    let request = terminal_cli_request(UiTerminalArgs {
        window: "terminal".to_string(),
        session_id: session_id.clone(),
        timeout_ms: 4321,
        operation: "pages:-2".to_string(),
    })
    .expect("valid terminal request");

    assert_eq!(request.window, "terminal");
    assert_eq!(request.selector, format!("terminal.session.{session_id}"));
    assert_eq!(request.action, UiAutomationAction::Terminal);
    assert_eq!(request.value.as_deref(), Some("pages:-2"));
    assert_eq!(request.timeout_ms, 4321);
}

#[test]
fn terminal_cli_request_rejects_before_request_construction() {
    let invalid_session = terminal_cli_request(UiTerminalArgs {
        window: "main".to_string(),
        session_id: "bad\nvalue".to_string(),
        timeout_ms: DEFAULT_TIMEOUT_MS,
        operation: "query".to_string(),
    })
    .unwrap_err();
    assert_eq!(invalid_session["error"], "invalid_terminal_session_id");

    let invalid_operation = terminal_cli_request(UiTerminalArgs {
        window: "main".to_string(),
        session_id: "session-a".to_string(),
        timeout_ms: DEFAULT_TIMEOUT_MS,
        operation: "line:01".to_string(),
    })
    .unwrap_err();
    assert_eq!(invalid_operation["error"], "invalid_terminal_operation");
}

#[test]
fn pointer_operation_grammar_is_exact() {
    for op in ["down", "move", "move:top", "move:bottom", "up", "cancel"] {
        assert!(valid_pointer_operation(op), "{op}");
    }
    for op in ["", "drag", "Down", "move:", "move:left", "up ", "leave"] {
        assert!(!valid_pointer_operation(op), "{op}");
    }
}

#[test]
fn key_chord_grammar_is_exact() {
    for chord in [
        "Escape",
        "Alt+ArrowUp",
        "Ctrl+Shift+Tab",
        "Ctrl+Alt+Shift+Meta+End",
        "Meta+Home",
    ] {
        assert!(valid_key_chord(chord), "{chord}");
    }
    for chord in [
        "alt+ArrowUp",
        "Alt+Ctrl+Home",
        "Alt+Alt+End",
        "Alt+",
        "A",
        "",
        "+Escape",
        "Alt",
        "escape",
    ] {
        assert!(!valid_key_chord(chord), "{chord}");
    }
}

fn pointer_args(operation: &str, selector: Option<&str>) -> UiPointerArgs {
    UiPointerArgs {
        window: "main".to_string(),
        selector: selector.map(str::to_string),
        timeout_ms: DEFAULT_TIMEOUT_MS,
        operation: operation.to_string(),
    }
}

#[test]
fn pointer_cli_request_applies_selector_rule() {
    for op in ["down", "move", "move:top", "move:bottom"] {
        let request = pointer_cli_request(pointer_args(op, Some("row"))).expect(op);
        assert_eq!(request.action, UiAutomationAction::Pointer);
        assert_eq!(request.selector, "row");
        assert_eq!(request.value.as_deref(), Some(op));
        let error = pointer_cli_request(pointer_args(op, None)).unwrap_err();
        assert_eq!(error["error"], "pointer_selector_required");
    }
    for op in ["up", "cancel"] {
        let request = pointer_cli_request(pointer_args(op, None)).expect(op);
        assert_eq!(request.selector, "");
        assert_eq!(request.value.as_deref(), Some(op));
        let error = pointer_cli_request(pointer_args(op, Some("row"))).unwrap_err();
        assert_eq!(error["error"], "pointer_selector_not_allowed");
    }
    // Grammar is checked before the selector rule.
    let error = pointer_cli_request(pointer_args("drag", None)).unwrap_err();
    assert_eq!(error["error"], "invalid_pointer_operation");
}

#[test]
fn key_cli_request_carries_chord_verbatim() {
    let request = key_cli_request(UiKeyArgs {
        window: "main".to_string(),
        selector: "row".to_string(),
        timeout_ms: 77,
        chord: "Alt+ArrowUp".to_string(),
    })
    .expect("valid key request");
    assert_eq!(request.action, UiAutomationAction::Key);
    assert_eq!(request.selector, "row");
    assert_eq!(request.value.as_deref(), Some("Alt+ArrowUp"));
    assert_eq!(request.timeout_ms, 77);

    let error = key_cli_request(UiKeyArgs {
        window: "main".to_string(),
        selector: "row".to_string(),
        timeout_ms: DEFAULT_TIMEOUT_MS,
        chord: "alt+ArrowUp".to_string(),
    })
    .unwrap_err();
    assert_eq!(error["error"], "invalid_key_chord");
}

/// #944 - the Rust enum and the `UiAutomationAction` union in `src/shared/types.ts`
/// are two closed lists that MUST agree. They are not generated from one another
/// (no ts-rs / typeshare / specta in this crate), so nothing but this test stops
/// them drifting.
///
/// Drift is NOT a crash, and an earlier draft of this comment claimed it was: the
/// request file is written and read by the same Rust enum, so serde never sees an
/// action it does not know. What actually happens is worse in one way. A Rust-only
/// addition ships a verb that exists, is documented, and fails 100% of the time at
/// runtime, because the bridge's catch-all (automation-bridge.ts:181-188) answers
/// `unsupported_action`. Today that is caught only by a human running the acceptance
/// runbook. This test moves it to `cargo test`.
#[test]
fn ui_automation_action_wire_names_match_typescript_union() {
    // First test in this crate to read OUTSIDE the crate root. `CARGO_MANIFEST_DIR`
    // expands at COMPILE time to the absolute path of src-tauri, so the process cwd
    // is irrelevant, and CI checks out the full tree (no sparse-checkout).
    let types_ts = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/shared/types.ts"),
    )
    .expect("read src/shared/types.ts");

    let start = types_ts
        .find("export type UiAutomationAction =")
        .expect("UiAutomationAction union not found in types.ts");

    // Strip `//` comments FIRST, before anything else reads this text. Two hazards,
    // and the ORDER is what defuses them: a comment inside the union carrying a quote
    // would inject a phantom member, and one carrying a `;` would truncate the union
    // early. The first revision of this test stripped AFTER `find(';')`, which left the
    // truncation hazard fully live while this comment claimed it was handled. That is
    // precisely the defect #944 exists to kill (a comment asserting a protection the
    // code does not have), so it is called out here rather than quietly repaired:
    // strip, THEN find the terminator. Both are false REDs, and a parity test that
    // cries wolf is a parity test somebody deletes.
    let stripped: String = types_ts[start..]
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    let end = stripped
        .find(';')
        .expect("unterminated UiAutomationAction union");

    let members: HashSet<String> = stripped[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.to_string())
        .collect();

    assert!(
        !members.is_empty(),
        "parsed zero members from the UiAutomationAction union; the parser or the union format \
            changed"
    );

    let rust: HashSet<String> = UiAutomationAction::all()
        .map(|action| action_wire_name(action).to_string())
        .collect();

    assert_eq!(
        members, rust,
        "UiAutomationAction is out of sync between src/shared/types.ts and ui_automation.rs"
    );
}
