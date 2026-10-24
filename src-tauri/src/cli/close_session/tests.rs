use super::*;

// ── §224 D.1 — interpret_close_response_exit_code (non-vacuous) ──

#[test]
fn closed_status_returns_zero() {
    let resp = include_str!("tests/fixtures/closed.json");
    assert_eq!(interpret_close_response_exit_code(resp), 0);
}

#[test]
fn already_closed_status_returns_zero() {
    let resp = include_str!("tests/fixtures/already-closed.json");
    assert_eq!(interpret_close_response_exit_code(resp), 0);
}

#[test]
fn no_match_status_returns_zero() {
    let resp = include_str!("tests/fixtures/no-match.json");
    assert_eq!(interpret_close_response_exit_code(resp), 0);
}

#[test]
fn restore_in_progress_status_returns_zero() {
    let resp = include_str!("tests/fixtures/restore-in-progress.json");
    assert_eq!(interpret_close_response_exit_code(resp), 0);
}

#[test]
fn unparseable_json_returns_two() {
    assert_eq!(interpret_close_response_exit_code("not json"), 2);
    assert_eq!(interpret_close_response_exit_code(""), 2);
    assert_eq!(interpret_close_response_exit_code("{partial"), 2);
}

#[test]
fn missing_status_field_returns_two() {
    let resp = r#"{"sessions_closed":0,"target":"t","action":"close-session"}"#;
    assert_eq!(interpret_close_response_exit_code(resp), 2);
}

#[test]
fn unknown_status_returns_two() {
    let resp = r#"{"status":"weird_new_state","target":"t","action":"close-session"}"#;
    assert_eq!(interpret_close_response_exit_code(resp), 2);
}

#[test]
fn non_string_status_returns_two() {
    let resp = r#"{"status":42,"target":"t","action":"close-session"}"#;
    assert_eq!(interpret_close_response_exit_code(resp), 2);
}

/// §224 review: valid JSON that is not an object (array, scalar, null)
/// still fails the `.get("status")` lookup and must return exit 2 to
/// preserve the "daemon spoke incoherently" contract.
#[test]
fn non_object_json_returns_two() {
    assert_eq!(interpret_close_response_exit_code("null"), 2);
    assert_eq!(interpret_close_response_exit_code("[1,2,3]"), 2);
    assert_eq!(interpret_close_response_exit_code("\"closed\""), 2);
    assert_eq!(interpret_close_response_exit_code("42"), 2);
    assert_eq!(interpret_close_response_exit_code("true"), 2);
}

// ── §224 D.1 — print_status_prose panic-resistance smoke tests ──
// Subprocess test (D.8) covers the actual stdout content end-to-end.

#[test]
fn print_status_prose_does_not_panic_on_known_statuses() {
    for s in &[
        "closed",
        "already_closed",
        "no_match",
        "restore_in_progress",
    ] {
        let body = format!(r#"{{"status":"{}","target":"t"}}"#, s);
        print_status_prose(&body);
    }
}

#[test]
fn print_status_prose_does_not_panic_on_unknown_input() {
    print_status_prose("not json");
    print_status_prose("");
    print_status_prose(r#"{"status":"unknown"}"#);
    print_status_prose(r#"{"no_status_at_all":true}"#);
    print_status_prose(r#"{"status":42}"#);
}
// ── §1440 ──

// §1440 D.1: presupuesto derivado de --timeout
#[test]
fn response_wait_budget_adds_fixed_overhead() {
    assert_eq!(response_wait_budget(30).as_secs(), 90);
    assert_eq!(response_wait_budget(120).as_secs(), 180);
    assert_eq!(response_wait_budget(0).as_secs(), 60);
}

// §1440 D.2: los ids van etiquetados con su artefacto en ambos sabores
#[test]
fn no_response_error_labels_both_ids_in_both_flavors() {
    let m = format_no_response_error(90, "rid-1", "mid-1", false);
    assert!(m.contains("no confirmation within 90s"));
    assert!(m.contains("request rid-1"));
    assert!(m.contains("message mid-1"));
    let m = format_no_response_error(90, "rid-1", "mid-1", true);
    assert!(m.contains("did not write a response within 90s"));
    assert!(m.contains("request rid-1"));
    assert!(m.contains("message mid-1"));
}
