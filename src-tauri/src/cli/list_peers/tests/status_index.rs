use super::*;

#[test]
fn no_session_yields_none() {
    let idx: HashMap<String, Vec<CandidateSession>> = HashMap::new();
    let ps = compute_peer_status(r"C:\does\not\exist", None, &idx);
    assert!(!ps.working);
    assert_eq!(ps.session_status, "none");
    assert_eq!(ps.status_legacy, "unknown");
    assert!(ps.session_id.is_none());
    assert!(!ps.waiting_for_input);
    assert!(ps.exit_code.is_none());
}

#[test]
fn running_session_is_working() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("any", SessionStatus::Running, false)],
    );
    let ps = compute_peer_status("C:/X", None, &idx);
    assert!(ps.working);
    assert_eq!(ps.session_status, "running");
    assert_eq!(ps.status_legacy, "active");
    assert!(ps.session_id.is_some());
}

#[test]
fn active_session_is_working() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("any", SessionStatus::Active, false)],
    );
    let ps = compute_peer_status(r"C:\X", None, &idx);
    assert!(ps.working);
    assert_eq!(ps.session_status, "active");
    assert_eq!(ps.status_legacy, "active");
}

#[test]
fn idle_session_is_not_working() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("any", SessionStatus::Idle, false)],
    );
    let ps = compute_peer_status("C:/X", None, &idx);
    assert!(!ps.working);
    assert_eq!(ps.session_status, "idle");
    assert_eq!(ps.status_legacy, "unknown");
}

#[test]
fn waiting_overrides_running() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("any", SessionStatus::Running, true)],
    );
    let ps = compute_peer_status("C:/X", None, &idx);
    assert!(!ps.working);
    assert_eq!(ps.session_status, "waiting");
    assert!(ps.waiting_for_input);
}

#[test]
fn exited_session_carries_exit_code() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("any", SessionStatus::Exited(42), false)],
    );
    let ps = compute_peer_status("C:/X", None, &idx);
    assert_eq!(ps.session_status, "exited");
    assert_eq!(ps.exit_code, Some(42));
    assert!(!ps.working);
}

#[test]
fn priority_picks_active_over_idle_at_same_cwd() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![
            cand("any", SessionStatus::Idle, false),
            cand("any", SessionStatus::Active, false),
        ],
    );
    let ps = compute_peer_status("C:/X", None, &idx);
    assert_eq!(ps.session_status, "active");
    assert!(ps.working);
}

#[test]
fn extended_length_prefix_normalizes() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("any", SessionStatus::Running, false)],
    );
    let ps = compute_peer_status(r"\\?\C:\X", None, &idx);
    assert_eq!(ps.session_status, "running");
}

// ── compute_peer_status with WG name filter (expected_name=Some) ─

#[test]
fn wg_name_filter_matches_only_named_session() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![
            cand("wg-20/dev", SessionStatus::Active, false),
            cand("[temp]-foo", SessionStatus::Active, false),
            cand("other-name", SessionStatus::Running, true),
        ],
    );
    let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
    assert_eq!(ps.session_status, "active");
    assert!(ps.working);
    assert!(!ps.waiting_for_input);
}

#[test]
fn wg_name_filter_returns_none_when_no_name_match() {
    let mut idx = HashMap::new();
    idx.insert(
        "c:/x".to_string(),
        vec![cand("other-name", SessionStatus::Active, false)],
    );
    let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
    assert_eq!(ps.session_status, "none");
    assert!(!ps.working);
}

// ── build_session_index_from filter tests ────────────────────────

#[test]
fn build_index_skips_temp_sessions() {
    let rows = vec![ps_row(
        "[temp]-dispatch",
        r"C:\X",
        Some(SessionStatus::Active),
        true,
    )];
    let idx = build_session_index_from(&rows);
    assert!(idx.is_empty(), "temp-prefixed sessions must be skipped");
}

#[test]
fn build_index_skips_rows_without_id() {
    let rows = vec![ps_row(
        "wg-20/dev",
        r"C:\X",
        Some(SessionStatus::Active),
        false,
    )];
    let idx = build_session_index_from(&rows);
    assert!(idx.is_empty(), "rows without id must be skipped");
}

#[test]
fn build_index_skips_rows_without_status() {
    let rows = vec![ps_row("wg-20/dev", r"C:\X", None, true)];
    let idx = build_session_index_from(&rows);
    assert!(idx.is_empty(), "rows without status must be skipped");
}

#[test]
fn build_index_normalizes_cwd_with_extended_prefix() {
    let rows = vec![ps_row(
        "wg-20/dev",
        r"\\?\C:\X",
        Some(SessionStatus::Active),
        true,
    )];
    let idx = build_session_index_from(&rows);
    assert!(idx.contains_key("c:/x"));
}

#[test]
fn build_index_groups_multiple_rows_at_same_cwd() {
    let rows = vec![
        ps_row("wg-20/dev", r"C:\X", Some(SessionStatus::Active), true),
        ps_row("other", r"C:/X", Some(SessionStatus::Idle), true),
    ];
    let idx = build_session_index_from(&rows);
    let bucket = idx.get("c:/x").expect("entries grouped under c:/x");
    assert_eq!(bucket.len(), 2);
}

// ── §6 / §12 list-peers-lean (issue #252) ──────────────────────────────
