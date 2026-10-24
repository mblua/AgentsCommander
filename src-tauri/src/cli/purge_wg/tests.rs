use super::*;

// ── interpret_purge_response_exit_code truth table ──

#[test]
fn purged_returns_zero() {
    let resp = r#"{"status":"purged","purged":3,"failed":0}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 0);
}

#[test]
fn dry_run_ready_returns_zero() {
    let resp = r#"{"status":"dry_run_ready","would_purge":true}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 0);
}

#[test]
fn dry_run_blocked_returns_three() {
    let resp = r#"{"status":"dry_run_blocked","would_purge":false}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 3);
}

#[test]
fn rejected_busy_returns_three() {
    let resp = r#"{"status":"rejected_busy","peers":[]}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 3);
}

#[test]
fn restore_in_progress_returns_three() {
    let resp = r#"{"status":"restore_in_progress"}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 3);
}

#[test]
fn partial_failure_returns_four() {
    let resp = r#"{"status":"partial_failure","purged":2,"failed":1}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 4);
}

#[test]
fn failed_root_guard_returns_four() {
    let resp = r#"{"status":"failed_root_guard","offending_session_id":"abc"}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 4);
}

#[test]
fn unparseable_json_returns_two() {
    assert_eq!(interpret_purge_response_exit_code("not json"), 2);
    assert_eq!(interpret_purge_response_exit_code(""), 2);
}

#[test]
fn missing_status_field_returns_two() {
    let resp = r#"{"purged":0}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 2);
}

#[test]
fn unknown_status_returns_two() {
    let resp = r#"{"status":"weird"}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 2);
}

#[test]
fn non_string_status_returns_two() {
    let resp = r#"{"status":42}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 2);
}

#[test]
fn non_object_json_returns_two() {
    assert_eq!(interpret_purge_response_exit_code("null"), 2);
    assert_eq!(interpret_purge_response_exit_code("[1,2,3]"), 2);
    assert_eq!(interpret_purge_response_exit_code("\"purged\""), 2);
    assert_eq!(interpret_purge_response_exit_code("42"), 2);
    assert_eq!(interpret_purge_response_exit_code("true"), 2);
}

// (#885 F-8) The exit-code function reads ONLY `status`, never `would_purge`.
#[test]
fn exit_code_never_reads_would_purge() {
    let resp = r#"{"status":"dry_run_blocked","would_purge":true}"#;
    assert_eq!(interpret_purge_response_exit_code(resp), 3);
}

#[test]
fn print_status_prose_does_not_panic() {
    print_status_prose("not json");
    print_status_prose("");
    print_status_prose(r#"{"status":"purged"}"#);
    print_status_prose(r#"{"status":"rejected_busy"}"#);
    print_status_prose(include_str!("tests/fixtures/root-guard.json"));
}
