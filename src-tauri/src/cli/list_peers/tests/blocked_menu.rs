use super::*;

#[test]
fn build_session_index_from_copies_blocked_menu_from_persisted() {
    let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
    row.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let index = build_session_index_from(&[row]);
    let candidates = index
        .get(&canon_or_norm(r"C:\work"))
        .expect("row indexed by its cwd");
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].blocked_menu);
    assert_eq!(
        candidates[0].blocked_menu_message.as_deref(),
        Some(BLOCKED_MENU_MESSAGE)
    );
}

#[test]
fn build_session_index_from_ignores_raise_hand_and_invisible_communication() {
    // Control 1: a VISIBLE raise-hand is a communication, but not a blocked
    // terminal.
    let mut raise_hand = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
    raise_hand.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some("the agent raised a hand".into()),
    });
    let index = build_session_index_from(&[raise_hand]);
    let candidates = index.get(&canon_or_norm(r"C:\work")).unwrap();
    assert!(!candidates[0].blocked_menu);
    assert_eq!(candidates[0].blocked_menu_message, None);

    // Control 2: a CLEARED (invisible) blocked-menu row is not blocked either.
    let mut invisible = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
    invisible.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: false,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let index = build_session_index_from(&[invisible]);
    let candidates = index.get(&canon_or_norm(r"C:\work")).unwrap();
    assert!(!candidates[0].blocked_menu);
    assert_eq!(candidates[0].blocked_menu_message, None);
}

#[test]
fn build_session_index_from_absent_communication_is_not_blocked() {
    let row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
    let index = build_session_index_from(&[row]);
    let candidates = index.get(&canon_or_norm(r"C:\work")).unwrap();
    assert!(!candidates[0].blocked_menu);
    assert_eq!(candidates[0].blocked_menu_message, None);
}

#[test]
fn compute_peer_status_returns_blocked_menu_of_the_chosen_session() {
    let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
    row.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let index = build_session_index_from(&[row]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(status.blocked_menu);
    assert_eq!(
        status.blocked_menu_message.as_deref(),
        Some(BLOCKED_MENU_MESSAGE)
    );
    // D5 regression net: the blocked condition is communicated ONLY through
    // the two new keys. Every pre-existing field keeps its current value.
    assert!(status.working);
    assert_eq!(status.session_status, "running");
    assert!(!status.waiting_for_input);
}

#[test]
fn compute_peer_status_suppresses_blocked_menu_for_an_exited_session() {
    // D4: `snapshot_sessions` preserves `communication` for exited rows
    // (#747), so a crash between the block and the exit leaves a stale
    // BlockedMenu whose terminal is gone. Suppress the pair.
    let mut row = ps_row(
        "Session 1",
        r"C:\work",
        Some(SessionStatus::Exited(0)),
        true,
    );
    row.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let index = build_session_index_from(&[row]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(!status.blocked_menu);
    assert_eq!(status.blocked_menu_message, None);
    assert_eq!(status.session_status, "exited");
    assert_eq!(status.exit_code, Some(0));
}

#[test]
fn compute_peer_status_blocked_without_message_keeps_the_flag() {
    let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
    row.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: None,
    });
    let index = build_session_index_from(&[row]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(status.blocked_menu);
    assert_eq!(status.blocked_menu_message, None);
}

#[test]
fn compute_peer_status_ignores_blocked_menu_of_an_unchosen_lower_priority_session() {
    // #1774 / D3: the pair must describe the session `max_by_key(priority)`
    // chose, not `filtered[0]`. The blocked row is first in the slice and has
    // the LOWER priority, so reading `filtered[0]` reports the wrong terminal.
    let mut blocked_idle = ps_row("Session 1", r"C:\work", Some(SessionStatus::Idle), true);
    blocked_idle.id = Some(OTHER_SESSION_ID.to_string());
    blocked_idle.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let mut chosen_active = ps_row("Session 2", r"C:\work", Some(SessionStatus::Active), true);
    chosen_active.id = Some(CHOSEN_SESSION_ID.to_string());
    let index = build_session_index_from(&[blocked_idle, chosen_active]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(!status.blocked_menu);
    assert_eq!(status.blocked_menu_message, None);
    assert_eq!(status.session_status, "active");
    assert_eq!(status.session_id.as_deref(), Some(CHOSEN_SESSION_ID));
}

#[test]
fn compute_peer_status_reports_blocked_menu_of_the_chosen_higher_priority_session() {
    // #1774 / D3, the inverse fixture: the unblocked row is first in the
    // slice and the blocked row is the one priority chooses, so reading
    // `filtered[0]` drops a real block and its message.
    let mut idle = ps_row("Session 1", r"C:\work", Some(SessionStatus::Idle), true);
    idle.id = Some(OTHER_SESSION_ID.to_string());
    let mut blocked_active = ps_row("Session 2", r"C:\work", Some(SessionStatus::Active), true);
    blocked_active.id = Some(CHOSEN_SESSION_ID.to_string());
    blocked_active.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let index = build_session_index_from(&[idle, blocked_active]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(status.blocked_menu);
    assert_eq!(
        status.blocked_menu_message.as_deref(),
        Some(BLOCKED_MENU_MESSAGE)
    );
    assert_eq!(status.session_status, "active");
    assert_eq!(status.session_id.as_deref(), Some(CHOSEN_SESSION_ID));
}

#[test]
fn compute_peer_status_reads_the_chosen_session_that_is_neither_first_nor_last() {
    // #1774 / D3: `chosen` is the MIDDLE row, so it is neither `filtered[0]`
    // nor `filtered.last()`. Both neighbours are unblocked and the chosen row
    // is blocked, so a read of either end reports no block and no message,
    // while `session_id` and `session_status` go on naming the chosen blocked
    // row. That disagreement is the D3 violation. The two neighbour ids
    // bracket `CHOSEN_SESSION_ID`, so a smallest-id or largest-id read lands
    // on an unblocked row too.
    // Priorities are 1, 3, 2, so `max_by_key` has no tie.
    let mut leading = ps_row("Session 1", r"C:\work", Some(SessionStatus::Idle), true);
    leading.id = Some(BELOW_CHOSEN_SESSION_ID.to_string());
    let mut blocked_active = ps_row("Session 2", r"C:\work", Some(SessionStatus::Active), true);
    blocked_active.id = Some(CHOSEN_SESSION_ID.to_string());
    blocked_active.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let mut trailing = ps_row("Session 3", r"C:\work", Some(SessionStatus::Running), true);
    trailing.id = Some(TRAILING_SESSION_ID.to_string());
    let index = build_session_index_from(&[leading, blocked_active, trailing]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(status.blocked_menu);
    assert_eq!(
        status.blocked_menu_message.as_deref(),
        Some(BLOCKED_MENU_MESSAGE)
    );
    assert_eq!(status.session_status, "active");
    assert_eq!(status.session_id.as_deref(), Some(CHOSEN_SESSION_ID));
}

#[test]
fn compute_peer_status_reports_the_message_of_the_chosen_blocked_session() {
    // #1774 / D3, the message half: ALL THREE rows are blocked and each
    // carries a different visible message, so the flag cannot tell them
    // apart. The chosen row is the MIDDLE blocked row, so it is neither the
    // first blocked row nor the last blocked one; its id is between the
    // other two ids and its message is between the other two messages. A
    // message read that takes the first blocked row, the last blocked row,
    // the smallest id, the largest id, the smallest message or the largest
    // message therefore reports a notice describing a different session
    // than `session_id` and `session_status` do. Priorities are 1, 3, 2, so
    // `max_by_key` has no tie.
    let mut first_blocked = ps_row("Session 1", r"C:\work", Some(SessionStatus::Idle), true);
    first_blocked.id = Some(BELOW_CHOSEN_SESSION_ID.to_string());
    first_blocked.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(OTHER_BLOCKED_MENU_MESSAGE.into()),
    });
    let mut chosen_blocked = ps_row("Session 2", r"C:\work", Some(SessionStatus::Active), true);
    chosen_blocked.id = Some(CHOSEN_SESSION_ID.to_string());
    chosen_blocked.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let mut last_blocked = ps_row("Session 3", r"C:\work", Some(SessionStatus::Running), true);
    last_blocked.id = Some(TRAILING_SESSION_ID.to_string());
    last_blocked.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(TRAILING_BLOCKED_MENU_MESSAGE.into()),
    });
    let index = build_session_index_from(&[first_blocked, chosen_blocked, last_blocked]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(status.blocked_menu);
    assert_eq!(
        status.blocked_menu_message.as_deref(),
        Some(BLOCKED_MENU_MESSAGE)
    );
    assert_eq!(status.session_status, "active");
    assert_eq!(status.session_id.as_deref(), Some(CHOSEN_SESSION_ID));
}

#[test]
fn compute_peer_status_follows_priority_not_a_status_lookalike_on_an_active_tie() {
    // #1822 / D3: BOTH rows are `Active`, so `priority` scores them equally
    // and `max_by_key` resolves the tie by taking the LAST maximum, which is
    // the unblocked row. The blocked row is FIRST, so a first-match status
    // predicate, `find(|c| matches!(c.status, SessionStatus::Active))`, lands
    // on it and reports a block that `session_id` and `session_status` do not
    // describe. This fixture pins that the pair follows `priority`'s ranking,
    // not a lookalike ranking that agrees with it only while a single row is
    // `Active`.
    let mut blocked_active = ps_row("Session 1", r"C:\work", Some(SessionStatus::Active), true);
    blocked_active.id = Some(OTHER_SESSION_ID.to_string());
    blocked_active.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::BlockedMenu,
        visible: true,
        updated_at: "2026-08-31T00:00:00Z".into(),
        message: Some(BLOCKED_MENU_MESSAGE.into()),
    });
    let mut chosen_active = ps_row("Session 2", r"C:\work", Some(SessionStatus::Active), true);
    chosen_active.id = Some(CHOSEN_SESSION_ID.to_string());
    let index = build_session_index_from(&[blocked_active, chosen_active]);
    let status = compute_peer_status(r"C:\work", None, &index);
    assert!(!status.blocked_menu);
    assert_eq!(status.blocked_menu_message, None);
    assert_eq!(status.session_status, "active");
    assert_eq!(status.session_id.as_deref(), Some(CHOSEN_SESSION_ID));
}

#[test]
fn full_peer_json_carries_blocked_menu_and_omits_when_unblocked() {
    let mut peer = sample_peer_info("p");
    peer.blocked_menu = true;
    peer.blocked_menu_message = Some(BLOCKED_MENU_MESSAGE.into());
    let json = serde_json::to_string(&peer).unwrap();
    assert!(json.contains("\"blockedMenu\":true"), "got {json}");
    let message_key = format!("\"blockedMenuMessage\":\"{BLOCKED_MENU_MESSAGE}\"");
    assert!(json.contains(&message_key), "got {json}");

    peer.blocked_menu = false;
    peer.blocked_menu_message = None;
    let json = serde_json::to_string(&peer).unwrap();
    assert!(
        !json.contains("blockedMenu"),
        "false must omit the key: {json}"
    );
    assert!(
        !json.contains("blockedMenuMessage"),
        "None must omit the key: {json}"
    );
}

#[test]
fn lean_peer_json_carries_blocked_menu_and_omits_when_unblocked() {
    let mut peer = sample_peer_info("p");
    peer.blocked_menu = true;
    peer.blocked_menu_message = Some(BLOCKED_MENU_MESSAGE.into());
    let lean = LeanPeerInfo::from(&peer);
    assert!(lean.blocked_menu);
    assert_eq!(
        lean.blocked_menu_message.as_deref(),
        Some(BLOCKED_MENU_MESSAGE)
    );
    let json = serde_json::to_string(&lean).unwrap();
    assert!(json.contains("\"blockedMenu\":true"), "got {json}");
    let message_key = format!("\"blockedMenuMessage\":\"{BLOCKED_MENU_MESSAGE}\"");
    assert!(json.contains(&message_key), "got {json}");

    peer.blocked_menu = false;
    peer.blocked_menu_message = None;
    let lean = LeanPeerInfo::from(&peer);
    let json = serde_json::to_string(&lean).unwrap();
    assert!(
        !json.contains("blockedMenu"),
        "false must omit the key: {json}"
    );
    assert!(
        !json.contains("blockedMenuMessage"),
        "None must omit the key: {json}"
    );
}

#[test]
fn list_peers_help_documents_blocked_menu() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let lp = cmd
        .get_subcommands()
        .find(|c| c.get_name() == "list-peers")
        .expect("list-peers subcommand");
    let after = lp.get_after_help().expect("after_help present").to_string();
    assert!(after.contains("blockedMenu"));
    assert!(after.contains("blockedMenuMessage"));
    assert!(after.contains("OMITTED"));
}

#[test]
fn list_peers_lean_help_documents_blocked_menu() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let lp = cmd
        .get_subcommands()
        .find(|c| c.get_name() == "list-peers-lean")
        .expect("list-peers-lean subcommand");
    let after = lp.get_after_help().expect("after_help present").to_string();
    assert!(after.contains("blockedMenu"));
    assert!(after.contains("blockedMenuMessage"));
    assert!(after.contains("OMITTED"));
}

// §6.3 — field preservation parity
