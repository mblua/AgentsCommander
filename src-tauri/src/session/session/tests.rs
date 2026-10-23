use super::*;

fn sample_session(effective: Option<Vec<String>>) -> Session {
    Session {
        id: Uuid::nil(),
        name: "Session 1".to_string(),
        shell: "claude-mb".to_string(),
        shell_args: vec!["--dangerously-skip-permissions".to_string()],
        backend_kind: SessionBackendKind::LocalProcess,
        effective_shell_args: effective,
        created_at: Utc::now(),
        working_directory: "C:\\tmp".to_string(),
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: Vec::new(),
        is_coordinator: false,
        is_root_agent: false,
        git_repos_gen: 0,
        agent_turn_armed: false,
        token: Uuid::nil(),
        agent_kind: None,
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        resolved_claude_projects_dir: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

#[test]
fn persisted_is_working_agrees_with_is_working_on_every_status() {
    // Anti-drift net: every SessionStatus variant crossed with both
    // waiting_for_input values. Must enumerate all four, not a subset.
    for status in [
        SessionStatus::Active,
        SessionStatus::Running,
        SessionStatus::Idle,
        SessionStatus::Exited(0),
    ] {
        for waiting in [true, false] {
            let mut s = sample_session(None);
            s.status = status.clone();
            s.waiting_for_input = waiting;
            assert_eq!(
                persisted_is_working(Some(&s.status), Some(s.waiting_for_input)),
                is_working(&s),
                "disagreement for status={:?} waiting_for_input={waiting}",
                s.status
            );
        }
    }
}

#[test]
fn persisted_is_working_is_false_when_either_field_is_absent() {
    assert!(!persisted_is_working(None, Some(false)));
    assert!(!persisted_is_working(Some(&SessionStatus::Running), None));
    assert!(!persisted_is_working(None, None));
}

#[test]
fn session_info_from_session_copies_effective_shell_args_some() {
    let s = sample_session(Some(vec![
        "--dangerously-skip-permissions".to_string(),
        "--continue".to_string(),
    ]));
    let info = SessionInfo::from(&s);
    assert_eq!(
        info.effective_shell_args,
        Some(vec![
            "--dangerously-skip-permissions".to_string(),
            "--continue".to_string()
        ])
    );
}

#[test]
fn session_info_from_session_copies_effective_shell_args_none() {
    let s = sample_session(None);
    let info = SessionInfo::from(&s);
    assert_eq!(info.effective_shell_args, None);
}

#[test]
fn session_info_from_session_copies_effective_shell_args_empty() {
    let s = sample_session(Some(Vec::new()));
    let info = SessionInfo::from(&s);
    assert_eq!(info.effective_shell_args, Some(Vec::new()));
}

#[test]
fn session_info_from_session_copies_communication() {
    let mut s = sample_session(None);
    s.communication = Some(SessionCommunication {
        kind: SessionCommunicationKind::RaiseHand,
        visible: true,
        updated_at: "2026-06-28T17:00:00+00:00".to_string(),
        message: None,
    });
    let info = SessionInfo::from(&s);
    assert_eq!(info.communication, s.communication);
}

#[test]
fn session_info_from_session_copies_telegram_bot_id_internally() {
    let mut s = sample_session(None);
    s.telegram_bot_id = Some("bot-1".to_string());

    let info = SessionInfo::from(&s);

    assert_eq!(info.telegram_bot_id.as_deref(), Some("bot-1"));
}

// #2451 - match carrier plumbing: always on the wire, `null` when unset.
#[test]
fn session_info_serializes_match_tier_as_null_when_absent() {
    let json = serde_json::to_value(SessionInfo::from(&sample_session(None)))
        .expect("serialize SessionInfo");

    assert_eq!(json.get("matchTier"), Some(&serde_json::Value::Null));
    assert_eq!(
        json.get("originalProfileLetter"),
        Some(&serde_json::Value::Null)
    );
}

#[test]
fn session_info_deserializes_a_payload_without_the_new_keys() {
    let mut json = serde_json::to_value(SessionInfo::from(&sample_session(None)))
        .expect("serialize SessionInfo");
    let object = json.as_object_mut().expect("SessionInfo is an object");
    object.remove("matchTier");
    object.remove("originalProfileLetter");

    let info: SessionInfo = serde_json::from_value(json).expect("old payload loads");

    assert_eq!(info.match_tier, None);
    assert_eq!(info.original_profile_letter, None);
}

#[test]
fn from_session_copies_the_match_carrier() {
    let mut s = sample_session(None);
    s.match_tier = Some("labelAndLetter".to_string());
    s.original_profile_letter = Some("B".to_string());

    let info = SessionInfo::from(&s);

    assert_eq!(info.match_tier.as_deref(), Some("labelAndLetter"));
    assert_eq!(info.original_profile_letter.as_deref(), Some("B"));
}

#[test]
fn session_info_serialization_does_not_expose_telegram_bot_id() {
    let mut s = sample_session(None);
    s.telegram_bot_id = Some("bot-1".to_string());

    let json = serde_json::to_value(SessionInfo::from(&s)).expect("serialize SessionInfo");

    assert!(json.get("telegramBotId").is_none());
}

// #1088: context_percent is an internal carrier for snapshot_sessions,
// mapped by From<&Session> but kept off the frontend IPC wire (badge path
// untouched).
#[test]
fn session_info_from_session_copies_context_percent_internally() {
    let mut s = sample_session(None);
    s.context_percent = Some(7);

    let info = SessionInfo::from(&s);

    assert_eq!(info.context_percent, Some(7));
}

#[test]
fn session_info_serialization_does_not_expose_context_percent() {
    let mut s = sample_session(None);
    s.context_percent = Some(55);

    let json = serde_json::to_value(SessionInfo::from(&s)).expect("serialize SessionInfo");

    assert!(json.get("contextPercent").is_none());
}

#[test]
fn live_session_record_requires_id_and_non_exited_status() {
    assert!(is_live_session_record(true, Some(&SessionStatus::Active)));
    assert!(is_live_session_record(true, Some(&SessionStatus::Running)));
    assert!(is_live_session_record(true, Some(&SessionStatus::Idle)));
    assert!(is_live_session_record(true, None));
    assert!(!is_live_session_record(
        false,
        Some(&SessionStatus::Running)
    ));
    assert!(!is_live_session_record(
        true,
        Some(&SessionStatus::Exited(0))
    ));
}

// ── find_workgroup_task_path_for_cwd — issue #107 ──

#[test]
fn find_workgroup_task_path_returns_path_when_cwd_is_workgroup_root() {
    let p = find_workgroup_task_path_for_cwd(r"C:\proj\.ac\wg-3-team");
    assert_eq!(
        p,
        Some(std::path::PathBuf::from(r"C:\proj\.ac\wg-3-team\TASK.md"))
    );
}

#[test]
fn find_workgroup_task_path_walks_up_from_replica_dir() {
    let p = find_workgroup_task_path_for_cwd(r"C:\proj\.ac\wg-3-team\__agent_dev-rust");
    assert_eq!(
        p,
        Some(std::path::PathBuf::from(r"C:\proj\.ac\wg-3-team\TASK.md"))
    );
}

#[test]
fn find_workgroup_task_path_returns_none_outside_workgroup() {
    assert_eq!(find_workgroup_task_path_for_cwd(r"C:\Users\me\misc"), None);
}

#[test]
fn find_workgroup_task_path_handles_unc_prefix_input() {
    // The helper is a pure path walk; it does not strip `\\?\` itself —
    // §9.4 strips the prefix downstream when embedding into the prompt.
    // This test documents that the walk-up still finds the wg-* ancestor
    // even when the input carries the prefix.
    let p = find_workgroup_task_path_for_cwd(r"\\?\C:\proj\.ac\wg-3-team");
    assert!(p.is_some());
    let p = p.unwrap().to_string_lossy().to_string();
    assert!(p.ends_with(r"\wg-3-team\TASK.md"));
}

// --- #1028: SessionRepo.dirty is backend-authoritative and never restored ---

/// The whole point of `skip_deserializing`. A `sessions.json` written before the app
/// closed can hold `dirty: true` from a worktree state that is long gone; restoring
/// it would paint a red badge at launch that no watcher tick asked for. It must come
/// back `None` (violet, "not yet known") and wait for a real detection.
///
/// This also covers `create_session`, a `#[tauri::command]` that genuinely
/// deserializes `Vec<SessionRepo>` from frontend input: `dirty` is un-settable from a
/// command payload, so the badge cannot be spoofed. The frontend sends only
/// `{label, sourcePath}` today, so nothing is lost.
#[test]
fn session_repo_dirty_is_never_deserialized() {
    let stale = r#"{"label":"A","sourcePath":"C:/a","branch":"main","dirty":true}"#;
    let restored: SessionRepo = serde_json::from_str(stale).expect("deserialize");

    assert_eq!(
        restored.dirty, None,
        "a persisted `true` must NOT restore red"
    );
    assert_eq!(
        restored.branch.as_deref(),
        Some("main"),
        "branch still restores; only dirty is skipped"
    );
}

/// Back-compat: `sessions.json` from before #1028 has no `dirty` key at all.
#[test]
fn session_repo_without_dirty_key_restores_as_none() {
    let old = r#"{"label":"A","sourcePath":"C:/a","branch":"main"}"#;
    let restored: SessionRepo = serde_json::from_str(old).expect("deserialize");
    assert_eq!(restored.dirty, None);
}

/// Serialization is deliberately KEPT, so the wire and the on-disk file stay
/// self-describing: the key is always present, `None` as an explicit `null` rather
/// than an omitted key. The frontend reads `dirty === true`, and `skip_serializing_if`
/// would have stripped the key here while STILL letting a stale `true` through on
/// read, which is why it was rejected.
#[test]
fn session_repo_dirty_always_serializes_with_an_explicit_null() {
    let dirty = SessionRepo {
        label: "A".into(),
        source_path: "C:/a".into(),
        branch: Some("main".into()),
        dirty: Some(true),
    };
    assert_eq!(
        serde_json::to_value(&dirty).expect("serialize"),
        serde_json::json!({
            "label": "A",
            "sourcePath": "C:/a",
            "branch": "main",
            "dirty": true,
        })
    );

    let unknown = SessionRepo {
        label: "A".into(),
        source_path: "C:/a".into(),
        branch: None,
        dirty: None,
    };
    assert_eq!(
        serde_json::to_value(&unknown).expect("serialize")["dirty"],
        serde_json::json!(null),
        "key present as null, not omitted"
    );
}

#[test]
fn is_working_matches_the_documented_list_peers_predicate() {
    // Every point of the state space, against the definition `cli/list_peers.rs`
    // publishes: "true iff the peer has a Running/Active session not waiting
    // for input".
    let cases = [
        (SessionStatus::Active, false, true),
        (SessionStatus::Running, false, true),
        (SessionStatus::Idle, false, false),
        (SessionStatus::Exited(0), false, false),
        (SessionStatus::Active, true, false),
        (SessionStatus::Running, true, false),
        (SessionStatus::Idle, true, false),
        (SessionStatus::Exited(0), true, false),
    ];
    for (status, waiting_for_input, expected) in cases {
        let mut session = sample_session(None);
        session.status = status.clone();
        session.waiting_for_input = waiting_for_input;
        assert_eq!(
            is_working(&session),
            expected,
            "status={status:?} waiting_for_input={waiting_for_input}"
        );
    }
}

#[test]
fn is_working_ignores_pending_review() {
    for pending_review in [false, true] {
        let mut working = sample_session(None);
        working.pending_review = pending_review;
        assert!(is_working(&working));

        let mut idle = sample_session(None);
        idle.status = SessionStatus::Idle;
        idle.waiting_for_input = true;
        idle.pending_review = pending_review;
        assert!(!is_working(&idle));
    }
}
