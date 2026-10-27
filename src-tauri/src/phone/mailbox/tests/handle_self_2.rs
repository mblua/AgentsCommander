use super::*;

/// #749 - switch variant of the rename-back contract: a failed Phase-2 inject returns the
/// archived handoff to the root of the queue-time cwd.
#[tokio::test]
async fn self_switch_driver_inject_failure_renames_handoff_back() {
    let original_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "switch resume notes").unwrap();

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move { Ok(()) };
    let restart = move |_session_id: Uuid, _agent: String, _profile: String| async move {
        Ok(new_id.to_string())
    };
    let inject = move |_session_id: Uuid, _prompt: String| async move {
        Err("pty write failed".to_string())
    };

    // (#756) F8: a failed switch handoff inject fires NO boundary events
    // (no Cleared by design, and no ContentInjected without a real inject).
    let boundary_events = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let events_seen = boundary_events.clone();
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let events_seen = events_seen.clone();
        async move {
            events_seen.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        original_id,
        temp.path().to_path_buf(),
        "claude".into(),
        "B".into(),
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending.clone(),
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    assert!(
        boundary_events.lock().unwrap().is_empty(),
        "a failed switch handoff must not drop the fresh intent"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("SELF-HANDOFF.md")).unwrap(),
        "switch resume notes",
        "a failed prompt inject must rename the archived handoff back to the root"
    );
    assert!(pending.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn self_switch_driver_restart_failure_skips_inject_and_cleans_pending() {
    let original_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let persist_calls = Arc::new(Mutex::new(0usize));
    let restart_calls = Arc::new(Mutex::new(0usize));
    let inject_calls = Arc::new(Mutex::new(0usize));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist_seen = persist_calls.clone();
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| {
        let persist_seen = persist_seen.clone();
        async move {
            *persist_seen.lock().unwrap() += 1;
            Ok(())
        }
    };
    let restart_seen = restart_calls.clone();
    let restart = move |_session_id: Uuid, _agent: String, _profile: String| {
        let restart_seen = restart_seen.clone();
        async move {
            *restart_seen.lock().unwrap() += 1;
            Err("destroyed but not recreated".to_string())
        }
    };
    let inject_seen = inject_calls.clone();
    let inject = move |_session_id: Uuid, _prompt: String| {
        let inject_seen = inject_seen.clone();
        async move {
            *inject_seen.lock().unwrap() += 1;
            Ok(())
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        original_id,
        PathBuf::from("C:/x/.ac/wg-1-dev-team/__agent_dev-rust"),
        "claude".into(),
        "B".into(),
        ForgottenSummary::from_raw("should not inject"),
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending.clone(),
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        persist,
        restart,
        inject,
        |_session_id: Uuid, _boundary: SelfClearBoundary| async {},
    )
    .await;

    assert_eq!(*persist_calls.lock().unwrap(), 1);
    assert_eq!(*restart_calls.lock().unwrap(), 1);
    assert_eq!(*inject_calls.lock().unwrap(), 0);
    assert!(pending.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn self_switch_driver_persist_failure_skips_restart_and_cleans_pending() {
    let original_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let restart_calls = Arc::new(Mutex::new(0usize));
    let inject_calls = Arc::new(Mutex::new(0usize));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move {
        Err("config write failed".to_string())
    };
    let restart_seen = restart_calls.clone();
    let restart = move |_session_id: Uuid, _agent: String, _profile: String| {
        let restart_seen = restart_seen.clone();
        async move {
            *restart_seen.lock().unwrap() += 1;
            Ok(Uuid::new_v4().to_string())
        }
    };
    let inject_seen = inject_calls.clone();
    let inject = move |_session_id: Uuid, _prompt: String| {
        let inject_seen = inject_seen.clone();
        async move {
            *inject_seen.lock().unwrap() += 1;
            Ok(())
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        original_id,
        PathBuf::from("C:/x/.ac/wg-1-dev-team/__agent_dev-rust"),
        "claude".into(),
        "B".into(),
        ForgottenSummary::from_raw("should not inject"),
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending.clone(),
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        persist,
        restart,
        inject,
        |_session_id: Uuid, _boundary: SelfClearBoundary| async {},
    )
    .await;

    assert_eq!(*restart_calls.lock().unwrap(), 0);
    assert_eq!(*inject_calls.lock().unwrap(), 0);
    assert!(pending.0.lock().unwrap().is_empty());
}

// ────────────────────────── #1632 self-handoff-and-restart ──────────────────────────
//
// Every test below that reaches `handle_self_restart` NAMES its seeding call and its
// `agent_id` argument explicitly. That is not a style rule: D1 replaces the landed
// self-clear handler's SHELL gate with an IDENTITY gate on `session.agent_id`, and
// `seed_self_clear_session` hardcodes `agent_id: None` whatever the shell string is
// (session/manager.rs positions 4 and 5). A test that mirrors a self-clear fixture
// literally is therefore rejected at step 4 and asserts T6's branch instead of its own.
// `seed_self_switch_session` is the only helper in this module that produces an identity.

/// T1
#[test]
fn self_restart_action_const_pins_wire_value() {
    assert_eq!(SELF_RESTART_ACTION, "self-handoff-and-restart");
}

/// T2
#[test]
fn self_restart_handoff_prompt_is_single_line_self_contained() {
    // Both forms: the exact archived path (normal) and the root-name fallback.
    for path in [
        "SELF-HANDOFF.md",
        "self-clear/20260702_181530_SELF-HANDOFF.md",
    ] {
        let prompt = self_restart_handoff_base_prompt(path);
        assert!(!prompt.is_empty());
        assert!(!prompt.contains('\n'));
        assert!(!prompt.contains('\u{2014}'));
        assert_eq!(
            prompt.matches(path).count(),
            2,
            "the prompt must name the file in the read instruction AND the missing-or-empty \
                clause: {prompt}"
        );
        assert!(prompt.contains("missing or empty"));
        assert!(prompt.contains("self-handoff-and-restart"), "{prompt}");
        assert_eq!(build_self_restart_handoff_prompt(path, None), prompt);
    }
}

/// T3 - the pure recipe resolver. Effective outranks requested; nothing is invented.
#[test]
fn resolve_self_restart_recipe_pins_live_agent_and_effective_profile() {
    let both = resolve_self_restart_recipe(Some("pi"), Some("b"), Some("c")).unwrap();
    assert_eq!(both.agent, "pi");
    assert_eq!(
        both.profile.as_deref(),
        Some("B"),
        "the EFFECTIVE profile wins over the requested one, normalized"
    );

    assert_eq!(
        resolve_self_restart_recipe(Some("pi"), None, Some("c"))
            .unwrap()
            .profile
            .as_deref(),
        Some("C")
    );
    // No "A" is invented and no replica cell is read.
    assert_eq!(
        resolve_self_restart_recipe(Some("pi"), None, None)
            .unwrap()
            .profile,
        None
    );
    let trimmed = resolve_self_restart_recipe(Some("  pi  "), Some("  "), None).unwrap();
    assert_eq!(trimmed.agent, "pi");
    assert_eq!(trimmed.profile, None);
    // A non-single-letter effective profile falls THROUGH the fallback rather than being
    // truncated or passed through. Identical to today's switch behavior.
    assert_eq!(
        resolve_self_restart_recipe(Some("pi"), Some("beta"), None)
            .unwrap()
            .profile,
        None
    );
    // D1: no coding-agent identity means no recipe to respawn.
    let err = resolve_self_restart_recipe(None, Some("A"), Some("A")).unwrap_err();
    assert!(err.contains("no configured coding-agent identity"), "{err}");
}

/// T20 - the driver seam's empty-string sentinel and its inverse, plus the operator
/// rendering that keeps the sentinel out of a log line as `profile ''`.
#[test]
fn restart_profile_seam_sentinel_round_trips() {
    assert_eq!(encode_restart_profile_for_seam(None), "");
    assert_eq!(encode_restart_profile_for_seam(Some("A")), "A");
    assert_eq!(decode_restart_profile_from_seam(""), None);
    assert_eq!(decode_restart_profile_from_seam("   "), None);
    assert_eq!(decode_restart_profile_from_seam("A"), Some("A".to_string()));
    for original in [None, Some("A"), Some("Z")] {
        assert_eq!(
            decode_restart_profile_from_seam(&encode_restart_profile_for_seam(original)),
            original.map(str::to_string),
            "encode and decode must be inverses for {original:?}"
        );
    }
    assert_eq!(display_profile_for_log(""), "<inherited>");
    assert_eq!(display_profile_for_log("   "), "<inherited>");
    assert_eq!(display_profile_for_log("A"), "A");
}

/// T4 - the happy path AND the Selection-UI invariant, both discriminating.
#[tokio::test]
async fn handle_self_restart_valid_token_queues_without_selection_write() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    // requested "C" / effective "A" DIFFER on purpose. `seed_self_switch_session` takes
    // (shell, agent_id, requested_profile, effective_profile) while
    // `resolve_self_restart_recipe` takes (agent, effective, requested), i.e. the two
    // letters in the opposite order. With both letters equal nothing would catch the
    // handler's step-4 call site being written with them swapped; with "C"/"A" the
    // `"profile":"A"` assertion below goes red on that one-token slip.
    let (session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("C"),
        Some("A"),
    )
    .await;
    seed_self_handoff(&fixture.replica);
    // Deliberately MISMATCHED Selection-UI cells, so both halves of this test are
    // discriminating rather than vacuous: the response must carry the LIVE recipe (never
    // the cell), and the cells must survive the call untouched.
    std::fs::write(
        fixture.replica.join("config.json"),
        concat!(
            r#"{"identity":"../../_agent_dev-rust","#,
            r#""tooling":{"currentCodingAgent":"codex","profile":"C"}}"#,
        ),
    )
    .unwrap();

    let (path, msg) = build_self_restart_message_with_from(
        &fixture.replica,
        "msg-sr-1",
        "rid-sr-1",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_restart(&app, &path, &msg, false)
        .await
        .unwrap();

    let response = read_response_json(&fixture.replica, "rid-sr-1").unwrap();
    assert_eq!(response["action"], SELF_RESTART_ACTION);
    assert_eq!(response["status"], "queued");
    assert_eq!(response["coding_agent"], "pi");
    assert_eq!(response["profile"], "A");
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    assert!(read_reject_reason(&fixture.replica, "msg-sr-1").is_none());

    // `set_replica_coding_agent_selection` is never called on this path, so the four
    // Selection-UI cells it owns are unchanged. NOT a claim that the `tooling` block is
    // byte-stable: a real spawn rewrites lastCodingAgent / codingAgents /
    // profileContentHash, and the handler runs under cfg(test) so no spawn occurs here.
    assert_eq!(
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(&fixture.replica),
        Some("codex".to_string()),
        "the replica currentCodingAgent cell must be neither consulted nor overwritten"
    );
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.replica.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["tooling"]["profile"], "C");
}

/// T5 - the existence gate. The present agent id is load-bearing: with `agent_id: None`
/// this request would reject at step 4 with the identity reason instead, and the
/// assertion on the reason string would fail for the right-looking wrong cause.
#[tokio::test]
async fn handle_self_restart_missing_self_handoff_is_rejected() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (_session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("A"),
    )
    .await;
    // NO seed_self_handoff call: that absence is the subject of this test.

    let (path, msg) = build_self_restart_message_with_from(
        &fixture.replica,
        "msg-sr-missing",
        "rid-sr-missing",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_restart(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&fixture.replica, "msg-sr-missing")
        .expect("a missing SELF-HANDOFF.md must be rejected with a reason file");
    assert!(reason.contains("SELF-HANDOFF.md"), "{reason}");
    assert_eq!(pending_self_clear_len(&app).await, 0);
    assert_eq!(count_forget_archives(&fixture.replica), 0);
}

/// T6 - the D1 identity gate. This is the ONE test that wants no identity, and it SAYS
/// so rather than inheriting it: `seed_self_clear_session` hardcodes `agent_id: None`.
/// The shell is deliberately "claude", a coding-agent stem, which makes the test
/// discriminating against a regression to a shell gate: a shell gate would ACCEPT this
/// session.
#[tokio::test]
async fn handle_self_restart_without_agent_identity_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (_session_id, token) =
        seed_self_clear_session(&app, &cwd.to_string_lossy(), "claude").await;
    seed_self_handoff(&cwd);

    let (path, msg) = build_self_restart_message_with_from(
        &cwd,
        "msg-sr-noid",
        "rid-sr-noid",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_restart(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&cwd, "msg-sr-noid")
        .expect("a session with no coding-agent identity must be rejected");
    assert!(
        reason.contains("no configured coding-agent identity"),
        "{reason}"
    );
    assert_eq!(pending_self_clear_len(&app).await, 0);
    assert_eq!(count_forget_archives(&cwd), 0);
}

/// T7 - idempotency. The second request collapses and the FIRST keeps ownership of the
/// forgotten summary, so exactly one archive exists.
#[tokio::test]
async fn handle_self_restart_second_request_is_already_queued() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("A"),
    )
    .await;
    seed_self_handoff(&fixture.replica);
    std::fs::write(
        fixture.replica.join("SELF-FORGET.md"),
        "topic to forget on restart",
    )
    .unwrap();

    let poller = MailboxPoller::default();
    let (first_path, first_msg) = build_self_restart_message_with_from(
        &fixture.replica,
        "msg-sr-dup-1",
        "rid-sr-dup-1",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &first_path, &first_msg, false)
        .await
        .unwrap();
    let (second_path, second_msg) = build_self_restart_message_with_from(
        &fixture.replica,
        "msg-sr-dup-2",
        "rid-sr-dup-2",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &second_path, &second_msg, false)
        .await
        .unwrap();

    assert_eq!(
        read_self_clear_response_status(&fixture.replica, "rid-sr-dup-1").as_deref(),
        Some("queued")
    );
    assert_eq!(
        read_self_clear_response_status(&fixture.replica, "rid-sr-dup-2").as_deref(),
        Some("already_queued")
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    assert_eq!(
        count_forget_archives(&fixture.replica),
        1,
        "only the first queued request captures and archives the forgotten summary"
    );
}

/// T8 - the Root Agent queues like any other coding-agent session.
///
/// This is a BEHAVIORAL mirror of `handle_self_clear_root_agent_is_allowed`, not a literal
/// one: same `set_is_root_agent`, same existence seed, same assertion, but seeded through
/// the only helper that produces an identity. A literal copy would reject at D1's step 4.
///
/// `set_is_root_agent` makes step 6's Root cwd substitution fire, so the pre-flight
/// resolves against `root_agent_dir()` rather than the seeded `TempDir`. That is exactly
/// the production behavior it mirrors and it does not affect this test's outcome, because
/// "pi" is NOT in `wake_agents()` (exactly ["codex", "claude"]), so
/// `build_configured_agent_spawn_for_cwd` returns `Ok(None)` at its membership check and
/// the request proceeds. Do not "fix" that by pinning a configured agent here; the
/// launchability branches belong to T19.
#[tokio::test]
async fn handle_self_restart_root_agent_is_allowed() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("agent-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (session_id, token) =
        seed_self_switch_session(&app, &cwd, "pi", Some("pi"), Some("A"), Some("A")).await;
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }
    seed_self_handoff(&cwd);

    let (path, msg) = build_self_restart_message_with_from(
        &cwd,
        "msg-sr-root",
        "rid-sr-root",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_restart(&app, &path, &msg, false)
        .await
        .unwrap();

    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sr-root").as_deref(),
        Some("queued")
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert_eq!(pending_self_clear_len(&app).await, 1);
    assert!(
        read_reject_reason(&cwd, "msg-sr-root").is_none(),
        "the Root Agent must NOT be rejected by self-handoff-and-restart"
    );
}

/// T9 - the discriminating test for the scope widening: ONE cwd and ONE session, rejected
/// by switch and queued by restart, so the difference cannot silently regress.
///
/// It cannot be one test over one message. `reject_message` writes
/// `rejected/<id>.reason.txt`, writes `rejected/<id>.json`, and then REMOVES the original
/// outbox file, so the switch half both poisons any directory-scoped "no rejection"
/// assertion and destroys the path. Two message ids, two paths, every existence assertion
/// scoped to its own id. Switch runs first: the reverse order would leave the pending set
/// non-empty when the switch half runs and make its rejection reason ambiguous.
#[tokio::test]
async fn handle_self_restart_origin_matrix_agent_is_allowed_where_switch_is_not() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    // An origin Agent Matrix agent: `_agent_*`, NOT the `__agent_*` a Room replica uses.
    let cwd = temp
        .path()
        .join("proj-a")
        .join(".ac")
        .join("_agent_ac-architect-v3");
    std::fs::create_dir_all(&cwd).unwrap();
    // The present agent id is required TWICE over: the restart half needs it to pass
    // step 4, and the switch half needs it because `resolve_switch_targets` reads the
    // same field.
    let (session_id, token) =
        seed_self_switch_session(&app, &cwd, "pi", Some("pi"), Some("A"), Some("A")).await;
    seed_self_handoff(&cwd);
    let poller = MailboxPoller::default();

    let (switch_path, switch_msg) = build_self_switch_message(
        &cwd,
        "msg-sr-origin-switch",
        "rid-sr-origin-switch",
        Some(token.to_string()),
        None,
        None,
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_handoff_switch(&app, &switch_path, &switch_msg, false)
        .await
        .unwrap();
    let switch_reason = read_reject_reason(&cwd, "msg-sr-origin-switch")
        .expect("switch must reject an origin Agent Matrix cwd");
    assert!(
        switch_reason.contains("only supported from a Room replica"),
        "{switch_reason}"
    );
    assert_eq!(pending_self_clear_len(&app).await, 0);

    let (restart_path, restart_msg) = build_self_restart_message_with_from(
        &cwd,
        "msg-sr-origin-restart",
        "rid-sr-origin-restart",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &restart_path, &restart_msg, false)
        .await
        .unwrap();
    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sr-origin-restart").as_deref(),
        Some("queued"),
        "restart must queue exactly where switch refuses"
    );
    assert!(pending_self_clear_contains(&app, session_id).await);
    assert!(
        read_reject_reason(&cwd, "msg-sr-origin-restart").is_none(),
        "the restart half must have no rejection of ITS OWN message id"
    );
}

/// T10 - token authorization. Seeding a fully valid session and withholding only the
/// token is what makes this discriminating: steps 1 and 2 are then the only reachable
/// rejections, so a pass cannot come from the identity gate or the existence gate firing
/// first.
#[tokio::test]
async fn handle_self_restart_invalid_token_is_rejected() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    let (_session_id, _token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("A"),
    )
    .await;
    seed_self_handoff(&fixture.replica);
    let poller = MailboxPoller::default();

    let (no_token_path, no_token_msg) = build_self_restart_message_with_from(
        &fixture.replica,
        "msg-sr-notoken",
        "rid-sr-notoken",
        None,
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &no_token_path, &no_token_msg, false)
        .await
        .unwrap();
    let no_token_reason = read_reject_reason(&fixture.replica, "msg-sr-notoken")
        .expect("a tokenless request must be rejected");
    assert!(
        no_token_reason.contains("requires a valid session token"),
        "{no_token_reason}"
    );

    let (unknown_path, unknown_msg) = build_self_restart_message_with_from(
        &fixture.replica,
        "msg-sr-unknown",
        "rid-sr-unknown",
        Some(Uuid::new_v4().to_string()),
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &unknown_path, &unknown_msg, false)
        .await
        .unwrap();
    let unknown_reason = read_reject_reason(&fixture.replica, "msg-sr-unknown")
        .expect("a well-formed but unowned token must be rejected");
    assert!(
        unknown_reason.contains("no live session owns this token"),
        "{unknown_reason}"
    );

    assert_eq!(pending_self_clear_len(&app).await, 0);
    assert_eq!(count_forget_archives(&fixture.replica), 0);
}
