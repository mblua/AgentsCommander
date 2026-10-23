use super::*;

/// T19 - the step-6 launchability pre-flight rejects SYNCHRONOUSLY, with a positive
/// control in the same test so it cannot pass against a handler that rejects everything.
///
/// The fixture is the only one in this change that needs machinery no landed test builds,
/// and it is built exactly this way and no other: agent `ghost` with an EMPTY base
/// command, cell "A" carrying `command: "pi"` and cell "B" `empty_profile_cell()`.
/// `compose_effective_command("", "")` returns the empty string, which
/// `normalize_legacy_agent_command` rejects and `resolve_agent_spawn_command` wraps as
/// `Invalid profile command for 'ghost:B'`; letter "A" composes "pi" and succeeds. The
/// PRESENCE of the empty "B" cell is load-bearing: `cell_for_letter` filters on `enabled`
/// (which `empty_profile_cell()` sets true) and `fallback_letters_from("B")` is
/// ["B", "A"], so an ABSENT "B" would silently fall back to "A" and make the test vacuous.
///
/// Two traps, both avoided here: the cwd must carry NO instance profile override, because
/// `resolve_profile` ranks the replica profile read ABOVE the requested letter (a bare
/// `TempDir` satisfies this, so T4's fixture must not be reused); and the session must NOT
/// be flagged root, because the step-6 Root substitution would then resolve the pre-flight
/// against the process-global `root_agent_dir()` rather than this `TempDir`.
#[tokio::test]
async fn handle_self_restart_unlaunchable_pinned_profile_is_rejected_synchronously() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("ghost-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    {
        // Append to the fixture's `wake_agents()` vector rather than replacing it, so the
        // rest of the mailbox fixture stays intact.
        let settings = app.state::<SettingsState>();
        let mut guard = settings.write().await;
        guard.agents.push(wake_agent("ghost", "Ghost", ""));
        let mut cells = std::collections::BTreeMap::new();
        cells.insert(
            "A".to_string(),
            crate::config::settings::ProfileCellConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                command: "pi".into(),
                ..crate::config::settings::empty_profile_cell()
            },
        );
        cells.insert(
            "B".to_string(),
            crate::config::settings::empty_profile_cell(),
        );
        guard
            .coding_agent_profiles
            .profiles_by_agent
            .insert("ghost".to_string(), cells);
    }
    seed_self_handoff(&cwd);
    let poller = MailboxPoller::new();

    // The unlaunchable half: letter "B" composes an empty effective command.
    let (_bad_id, bad_token) =
        seed_self_switch_session(&app, &cwd, "ghost", Some("ghost"), Some("B"), Some("B")).await;
    let (bad_path, bad_msg) = build_self_restart_message_with_from(
        &cwd,
        "msg-sr-unlaunchable",
        "rid-sr-unlaunchable",
        Some(bad_token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &bad_path, &bad_msg, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&cwd, "msg-sr-unlaunchable")
        .expect("an unlaunchable pinned recipe must be rejected synchronously");
    assert!(reason.contains("is not launchable from"), "{reason}");
    assert_eq!(
        pending_self_clear_len(&app).await,
        0,
        "a rejected request must queue nothing"
    );
    assert_eq!(
        count_forget_archives(&cwd),
        0,
        "a rejected request must archive nothing"
    );
    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sr-unlaunchable"),
        None,
        "a rejected request must not write a queued response"
    );

    // The control: the same fixture and the same settings, differing ONLY in the pinned
    // letter, must queue. Without it this test would pass against a handler that rejects
    // everything.
    let (good_id, good_token) =
        seed_self_switch_session(&app, &cwd, "ghost", Some("ghost"), Some("A"), Some("A")).await;
    let (good_path, good_msg) = build_self_restart_message_with_from(
        &cwd,
        "msg-sr-launchable",
        "rid-sr-launchable",
        Some(good_token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    poller
        .handle_self_restart(&app, &good_path, &good_msg, false)
        .await
        .unwrap();
    assert_eq!(
        read_self_clear_response_status(&cwd, "rid-sr-launchable").as_deref(),
        Some("queued")
    );
    assert!(pending_self_clear_contains(&app, good_id).await);
    assert!(read_reject_reason(&cwd, "msg-sr-launchable").is_none());
}

/// T21 - an inherited profile letter must serialize as JSON `null`, not `""` and not
/// absent. The stronger form matters: `serde_json` would render an accidental `Some("")`
/// as `""`, which a naive truthiness check would pass.
///
/// The agent id is present while BOTH profile fields are absent, and that pairing is the
/// whole point of the fixture: with `agent_id: None` the request rejects at step 4 and
/// never writes a response body at all, so the assertion would fail on a missing file
/// rather than on a wrong `"profile"` value.
#[tokio::test]
async fn handle_self_restart_inherited_profile_response_carries_null_profile() {
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let cwd = temp.path().join("inherited-cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (_session_id, token) =
        seed_self_switch_session(&app, &cwd, "pi", Some("pi"), None, None).await;
    seed_self_handoff(&cwd);

    let (path, msg) = build_self_restart_message_with_from(
        &cwd,
        "msg-sr-inherited",
        "rid-sr-inherited",
        Some(token.to_string()),
        SELF_RESTART_TEST_FROM,
    );
    let poller = MailboxPoller::new();
    poller
        .handle_self_restart(&app, &path, &msg, false)
        .await
        .unwrap();

    let response = read_response_json(&cwd, "rid-sr-inherited").unwrap();
    assert_eq!(response["status"], "queued");
    assert_eq!(response["coding_agent"], "pi");
    assert_eq!(
        response.get("profile"),
        Some(&serde_json::Value::Null),
        "an inherited letter must be reported as null, never as an invented letter or \"\""
    );
}

/// T11 - the restart flavor at the driver seam: one persist call, pinning the un-forked
/// control flow, the pinned pair on the restart seam, the prompt on the NEW id.
///
/// There is no PTY-writes clause here, because this seam has no PTY: `inject` is a
/// recorder closure and the staggered `\r` bytes come from the real
/// `inject_text_into_session`, which this test replaces outright. T25 carries that
/// assertion at the seam that actually has a PTY.
///
/// For the same structural reason there is no "persist saw ZERO calls" clause. The shared
/// driver calls `persist` exactly once in the `InjectClear` arm for BOTH flavors, and the
/// flavor refactor deliberately changed no control flow. What separates a restart from a
/// switch is not the CALL but the CLOSURE: `run_self_restart_after_sustained_idle` supplies
/// an inert one, so `set_replica_coding_agent_selection` is never reached. That closure is
/// caller-supplied, so NO driver-seam test can pin it; asserting zero here would assert
/// against the driver's own contract, and asserting zero on a closure this test itself
/// supplies would be vacuous. The invariant is pinned where it is observable: A3's grep over
/// `handle_self_restart`, `run_self_restart_after_sustained_idle` and the restart persist
/// closure, and T4's positive assertion that the Selection-UI cells survive the handler with
/// a deliberately mismatched value in place. Asserting the call count is `1` here is not
/// vacuous: it pins that the flavor parameter did not fork the shared control flow.
#[tokio::test]
async fn self_restart_driver_restarts_uses_new_id_and_cleans_pending() {
    let original_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "restart resume notes").unwrap();

    let persist_calls = Arc::new(Mutex::new(0usize));
    let restart_calls = Arc::new(Mutex::new(Vec::<(Uuid, String, String)>::new()));
    let injected = Arc::new(Mutex::new(Vec::<(Uuid, String)>::new()));
    let boundary_events = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));

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
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let restart_seen = restart_seen.clone();
        async move {
            restart_seen
                .lock()
                .unwrap()
                .push((session_id, agent, profile));
            Ok(new_id.to_string())
        }
    };
    let inject_seen = injected.clone();
    let inject = move |session_id: Uuid, prompt: String| {
        let inject_seen = inject_seen.clone();
        async move {
            inject_seen.lock().unwrap().push((session_id, prompt));
            Ok(())
        }
    };
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
        "pi".into(),
        "A".into(),
        None,
        SELF_HANDOFF_FLAVOR_RESTART,
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

    assert_eq!(
        *persist_calls.lock().unwrap(),
        1,
        "the shared driver calls persist once for BOTH flavors; the flavor parameter must not \
             fork the control flow. What makes a restart inert is the CLOSURE the runner supplies, \
             which A3 and T4 pin (see this test's doc comment)."
    );
    assert_eq!(
        *restart_calls.lock().unwrap(),
        vec![(original_id, "pi".to_string(), "A".to_string())],
        "the restart seam must receive the pinned live recipe"
    );
    let prompts = injected.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0].0, new_id,
        "the prompt lands on the NEW session id"
    );
    assert!(prompts[0].1.contains("self-clear/"), "{}", prompts[0].1);
    assert!(
        prompts[0].1.contains("self-handoff-and-restart"),
        "{}",
        prompts[0].1
    );
    assert_eq!(
        *boundary_events.lock().unwrap(),
        vec![(new_id, SelfClearBoundary::ContentInjected)]
    );
    assert!(pending.0.lock().unwrap().is_empty());
}

/// #1940 - the self-switch driver takes the shared selection operation turn only
/// for the publication -> restart -> alias span. The sustained-idle wait stays
/// outside it, a concurrent guarded operation cannot acknowledge while the
/// persist and its restart are in flight (and does acknowledge before the
/// phase-2 inject), and a failed persist/restart leaves no partial publication
/// and no leaked pending alias.
#[tokio::test]
async fn issue_1937_self_switch_turn() {
    use crate::session::selection::{
        acquire_selection_operation_turn, run_owned_selection_operation,
    };
    use tokio::sync::oneshot;

    let source_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(source_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "switch resume notes").unwrap();

    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let persisted_pair = Arc::new(Mutex::new(None::<(String, String)>));
    let concurrent_done = Arc::new(AtomicBool::new(false));

    // Phase 1: the sustained-idle wait must not hold the turn - a guarded
    // acquisition here would block forever if it did.
    let events_state = Arc::clone(&events);
    let session_state = move |_session_id: Uuid| {
        let events = Arc::clone(&events_state);
        async move {
            let guard =
                tokio::time::timeout(Duration::from_secs(5), acquire_selection_operation_turn())
                    .await
                    .expect("the sustained-idle wait must not hold the selection operation turn")
                    .expect("acquire the turn during the idle wait");
            drop(guard);
            events.lock().unwrap().push("idle:turn-free");
            (true, true)
        }
    };

    let (concurrent_acquired_tx, concurrent_acquired_rx) = oneshot::channel::<()>();
    let concurrent_slot = Arc::new(Mutex::new(Some(concurrent_acquired_tx)));
    let events_persist = Arc::clone(&events);
    let persisted_for_persist = Arc::clone(&persisted_pair);
    let done_for_concurrent = Arc::clone(&concurrent_done);
    let concurrent_for_persist = Arc::clone(&concurrent_slot);
    let persist = move |_cwd: PathBuf, agent: String, profile: String| {
        let events = Arc::clone(&events_persist);
        let persisted = Arc::clone(&persisted_for_persist);
        let done = Arc::clone(&done_for_concurrent);
        let acquired_tx = concurrent_for_persist.lock().unwrap().take();
        async move {
            events.lock().unwrap().push("persist:enter");
            let events_for_concurrent = Arc::clone(&events);
            let done_for_concurrent = Arc::clone(&done);
            let concurrent = tokio::spawn(run_owned_selection_operation(move || async move {
                events_for_concurrent
                    .lock()
                    .unwrap()
                    .push("concurrent:acquired");
                done_for_concurrent.store(true, Ordering::SeqCst);
                if let Some(tx) = acquired_tx {
                    let _ = tx.send(());
                }
                Ok::<(), String>(())
            }));
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert!(
                !concurrent.is_finished(),
                "a concurrent guarded operation must wait for the driver-owned turn"
            );
            assert!(
                !done.load(Ordering::SeqCst),
                "the concurrent guarded operation must not acquire during persist"
            );
            *persisted.lock().unwrap() = Some((agent, profile));
            events.lock().unwrap().push("persist:exit");
            Ok::<(), String>(())
        }
    };

    let events_restart = Arc::clone(&events);
    let done_for_restart = Arc::clone(&concurrent_done);
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let events = Arc::clone(&events_restart);
        let done = Arc::clone(&done_for_restart);
        async move {
            assert_eq!(session_id, source_id);
            assert_eq!((agent.as_str(), profile.as_str()), ("codex", "B"));
            assert!(
                !done.load(Ordering::SeqCst),
                "the restart must run under the driver-owned turn"
            );
            events.lock().unwrap().push("restart:enter");
            Ok(new_id.to_string())
        }
    };

    let acquired_rx_slot = Arc::new(Mutex::new(Some(concurrent_acquired_rx)));
    let events_inject = Arc::clone(&events);
    let inject = move |session_id: Uuid, _prompt: String| {
        let events = Arc::clone(&events_inject);
        let acquired = acquired_rx_slot.lock().unwrap().take();
        async move {
            if let Some(acquired) = acquired {
                tokio::time::timeout(Duration::from_secs(5), acquired)
                    .await
                    .expect("the concurrent guarded operation must acquire the turn before phase 2")
                    .expect("concurrent acquisition signal");
            }
            events.lock().unwrap().push("inject");
            assert_eq!(session_id, new_id);
            Ok::<(), String>(())
        }
    };
    let note_boundary = |_session_id: Uuid, _boundary: SelfClearBoundary| async {};

    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_id,
        temp.path().to_path_buf(),
        "codex".into(),
        "B".into(),
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending.clone(),
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(30),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    assert_eq!(
        *persisted_pair.lock().unwrap(),
        Some(("codex".into(), "B".into()))
    );
    let log = events.lock().unwrap().clone();
    let restart_index = log
        .iter()
        .position(|event| *event == "restart:enter")
        .expect("restart ran");
    let acquired_index = log
        .iter()
        .position(|event| *event == "concurrent:acquired")
        .expect("the concurrent guarded operation acquired the turn");
    let inject_index = log
        .iter()
        .position(|event| *event == "inject")
        .expect("phase-2 inject ran");
    assert!(
        restart_index < acquired_index,
        "the turn must be held across the restart reply: {log:?}"
    );
    assert!(
        acquired_index < inject_index,
        "the turn must be released before the phase-2 inject: {log:?}"
    );
    assert!(pending.0.lock().unwrap().is_empty());

    // Persist failure: no restart is attempted and the pending alias is cleaned.
    let source_persist_failure = Uuid::new_v4();
    let pending_persist_failure = Arc::new(crate::PendingSelfClear::default());
    pending_persist_failure
        .0
        .lock()
        .unwrap()
        .insert(source_persist_failure);
    let restart_calls_persist_failure = Arc::new(Mutex::new(0usize));
    let restart_seen = Arc::clone(&restart_calls_persist_failure);
    let persist_failure = move |_cwd: PathBuf, _agent: String, _profile: String| async move {
        Err::<(), String>("persist rejected".to_string())
    };
    let restart_after_persist_failure =
        move |_session_id: Uuid, _agent: String, _profile: String| {
            let restart_seen = Arc::clone(&restart_seen);
            async move {
                *restart_seen.lock().unwrap() += 1;
                Ok(Uuid::new_v4().to_string())
            }
        };
    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_persist_failure,
        temp.path().to_path_buf(),
        "codex".into(),
        "B".into(),
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending_persist_failure.clone(),
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(30),
        move |_session_id: Uuid| async move { (true, true) },
        persist_failure,
        restart_after_persist_failure,
        |_session_id: Uuid, _prompt: String| async { Ok(()) },
        note_boundary,
    )
    .await;
    assert_eq!(
        *restart_calls_persist_failure.lock().unwrap(),
        0,
        "a failed persist must never restart"
    );
    assert!(pending_persist_failure.0.lock().unwrap().is_empty());

    // Restart failure: the persisted pair survives and the pending alias is cleaned.
    let source_restart_failure = Uuid::new_v4();
    let pending_restart_failure = Arc::new(crate::PendingSelfClear::default());
    pending_restart_failure
        .0
        .lock()
        .unwrap()
        .insert(source_restart_failure);
    let persisted_restart_failure = Arc::new(Mutex::new(None::<(String, String)>));
    let persisted_for_restart_failure = Arc::clone(&persisted_restart_failure);
    let persist_ok = move |_cwd: PathBuf, agent: String, profile: String| {
        let persisted = Arc::clone(&persisted_for_restart_failure);
        async move {
            *persisted.lock().unwrap() = Some((agent, profile));
            Ok(())
        }
    };
    let restart_calls_restart_failure = Arc::new(Mutex::new(0usize));
    let restart_seen = Arc::clone(&restart_calls_restart_failure);
    let restart_failure = move |_session_id: Uuid, _agent: String, _profile: String| {
        let restart_seen = Arc::clone(&restart_seen);
        async move {
            *restart_seen.lock().unwrap() += 1;
            Err::<String, String>("restart rejected".to_string())
        }
    };
    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_restart_failure,
        temp.path().to_path_buf(),
        "codex".into(),
        "B".into(),
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending_restart_failure.clone(),
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(30),
        move |_session_id: Uuid| async move { (true, true) },
        persist_ok,
        restart_failure,
        |_session_id: Uuid, _prompt: String| async { Ok(()) },
        note_boundary,
    )
    .await;
    assert_eq!(
        *restart_calls_restart_failure.lock().unwrap(),
        1,
        "exactly one restart attempt"
    );
    assert_eq!(
        *persisted_restart_failure.lock().unwrap(),
        Some(("codex".into(), "B".into())),
        "a failed restart must preserve the persisted pair"
    );
    assert!(pending_restart_failure.0.lock().unwrap().is_empty());
}

/// T22 - the full absent-profile chain at the seam: recipe `None` encodes to `""`, the
/// driver carries `""`, and the runner's decode turns it back into `None` for
/// `requested_profile`.
#[tokio::test]
async fn self_restart_driver_passes_empty_sentinel_when_profile_absent() {
    let original_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "restart resume notes").unwrap();

    let restart_calls = Arc::new(Mutex::new(Vec::<(Uuid, String, String)>::new()));
    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move { Ok(()) };
    let restart_seen = restart_calls.clone();
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let restart_seen = restart_seen.clone();
        async move {
            restart_seen
                .lock()
                .unwrap()
                .push((session_id, agent, profile));
            Ok(new_id.to_string())
        }
    };
    let inject = move |_session_id: Uuid, _prompt: String| async move { Ok(()) };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        original_id,
        temp.path().to_path_buf(),
        "pi".into(),
        encode_restart_profile_for_seam(None),
        None,
        SELF_HANDOFF_FLAVOR_RESTART,
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

    let calls = restart_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].2, "",
        "an absent letter crosses the seam as the sentinel"
    );
    assert_eq!(
        decode_restart_profile_from_seam(&calls[0].2),
        None,
        "and the runner's decode turns the sentinel back into requested_profile: None"
    );
    assert!(pending.0.lock().unwrap().is_empty());
}

/// T12 - a failed respawn skips the injection entirely: nothing is archived and
/// `SELF-HANDOFF.md` stays in the root, so the agent can re-issue.
#[tokio::test]
async fn self_restart_driver_restart_failure_skips_inject_and_cleans_pending() {
    let original_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "restart resume notes").unwrap();
    let inject_calls = Arc::new(Mutex::new(0usize));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move { Ok(()) };
    let restart = move |_session_id: Uuid, _agent: String, _profile: String| async move {
        Err("destroyed but not recreated".to_string())
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
        temp.path().to_path_buf(),
        "pi".into(),
        "A".into(),
        None,
        SELF_HANDOFF_FLAVOR_RESTART,
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

    assert_eq!(*inject_calls.lock().unwrap(), 0);
    assert!(
        temp.path().join("SELF-HANDOFF.md").is_file(),
        "a failed respawn must leave the handoff file in the root so the agent can re-issue"
    );
    assert!(
        !temp.path().join("self-clear").exists(),
        "phase 2 never ran, so nothing was archived"
    );
    assert!(pending.0.lock().unwrap().is_empty());
}

/// T13 - restart variant of the rename-back contract: a failed phase-2 inject returns the
/// archived handoff to the root of the queue-time cwd and fires no boundary event.
#[tokio::test]
async fn self_restart_driver_inject_failure_renames_handoff_back() {
    let original_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "restart resume notes").unwrap();

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move { Ok(()) };
    let restart = move |_session_id: Uuid, _agent: String, _profile: String| async move {
        Ok(new_id.to_string())
    };
    let inject = move |_session_id: Uuid, _prompt: String| async move {
        Err("pty write failed".to_string())
    };
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
        "pi".into(),
        "A".into(),
        None,
        SELF_HANDOFF_FLAVOR_RESTART,
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

    assert_eq!(
        std::fs::read_to_string(temp.path().join("SELF-HANDOFF.md")).unwrap(),
        "restart resume notes",
        "a failed prompt inject must rename the archived handoff back to the root"
    );
    let leftovers = std::fs::read_dir(temp.path().join("self-clear"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .ends_with("_SELF-HANDOFF.md")
                })
                .count()
        })
        .unwrap_or(0);
    assert_eq!(
        leftovers, 0,
        "no archived handoff may remain under self-clear/"
    );
    assert!(
        boundary_events.lock().unwrap().is_empty(),
        "a failed restart handoff must not drop the fresh intent"
    );
    assert!(pending.0.lock().unwrap().is_empty());
}

/// T14 - the regression net for the section 6.3d flavor refactor. Driving with
/// `SELF_HANDOFF_FLAVOR_SWITCH` must still produce the SWITCH prompt, which is what stops
/// the flavor parameter from silently swapping the two.
#[tokio::test]
async fn self_switch_driver_flavor_preserves_switch_prompt() {
    let original_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(original_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "switch resume notes").unwrap();
    let injected = Arc::new(Mutex::new(Vec::<String>::new()));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move { Ok(()) };
    let restart = move |_session_id: Uuid, _agent: String, _profile: String| async move {
        Ok(new_id.to_string())
    };
    let inject_seen = injected.clone();
    let inject = move |_session_id: Uuid, prompt: String| {
        let inject_seen = inject_seen.clone();
        async move {
            inject_seen.lock().unwrap().push(prompt);
            Ok(())
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
        |_session_id: Uuid, _boundary: SelfClearBoundary| async {},
    )
    .await;

    let prompts = injected.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("self-handoff-and-switch"),
        "{}",
        prompts[0]
    );
    // ...and it is byte-identical to the switch builder's output for the exact archived
    // path, so a swapped `build_prompt` cannot pass on a substring alone.
    let archived: Vec<PathBuf> = std::fs::read_dir(temp.path().join("self-clear"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().ends_with("_SELF-HANDOFF.md"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(archived.len(), 1);
    let relative = format!(
        "self-clear/{}",
        archived[0].file_name().unwrap().to_string_lossy()
    );
    assert_eq!(
        prompts[0],
        build_self_switch_handoff_prompt(&relative, None)
    );
    assert_ne!(
        prompts[0],
        build_self_restart_handoff_prompt(&relative, None)
    );
}
