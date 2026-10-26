use super::*;

/// T25 - the app-backed restart seam, which is the one that actually has a PTY: the
/// resume prompt reaches the restarted session's PTY followed by the two staggered `\r`
/// bytes, and the persist recorder sees the single call the shared driver makes for BOTH
/// flavors (see T11's doc comment for why the count is 1 and not 0).
///
/// D1's no-Enter case is INHERITED, not re-tested here. This fixture's shell is `pi`, a
/// `needs_explicit_enter` stem, so it covers the Enter-appending half only. The other
/// half is decided entirely by `needs_explicit_enter` and is already covered by
/// `agent_clis_require_explicit_enter`, `plain_shells_do_not_require_explicit_enter` and
/// `direct_shell_capability_matrix` in `pty/inject.rs`, which this change does not edit.
#[tokio::test]
async fn self_restart_driver_app_backed_prompt_reaches_pty_with_enter() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .agents
            .push(wake_agent("pi", "Pi", "pi"));
    }
    let (source_id, _token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "pi",
        Some("pi"),
        Some("A"),
        Some("A"),
    )
    .await;
    {
        let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = state.read().await;
        mgr.mark_idle(source_id).await;
    }
    seed_self_handoff(&fixture.replica);
    let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();
    pending.seed_legacy(source_id);

    let persist_calls = Arc::new(Mutex::new(0usize));
    let persist_seen = persist_calls.clone();
    let persist = move |_cwd: PathBuf, _agent: String, _profile: String| {
        let persist_seen = persist_seen.clone();
        async move {
            *persist_seen.lock().unwrap() += 1;
            Ok(())
        }
    };

    let app_for_state = app.clone();
    let session_state = move |session_id: Uuid| {
        let app = app_for_state.clone();
        async move {
            let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = state.read().await;
            match mgr.get_session(session_id).await {
                Some(session) => (true, session.waiting_for_input),
                None => (false, false),
            }
        }
    };

    let restarted_id = Arc::new(Mutex::new(None::<Uuid>));
    let restarted_id_seen = Arc::clone(&restarted_id);
    let app_for_restart = app.clone();
    let replica_for_restart = fixture.replica.clone();
    let restart = move |session_id: Uuid, agent: String, profile: String| {
        let app = app_for_restart.clone();
        let replica = replica_for_restart.clone();
        let restarted_id = Arc::clone(&restarted_id_seen);
        async move {
            assert_eq!(session_id, source_id);
            assert_eq!(agent, "pi");
            assert_eq!(profile, "A");
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.destroy_session(session_id).await.unwrap();
            }
            let (new_id, _token) = seed_self_switch_session(
                &app,
                &replica,
                "pi",
                Some("pi"),
                Some(&profile),
                Some(&profile),
            )
            .await;
            {
                let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = state.read().await;
                mgr.mark_idle(new_id).await;
            }
            register_mock_pty_route(&app, new_id);
            *restarted_id.lock().unwrap() = Some(new_id);
            Ok(new_id.to_string())
        }
    };

    let injected_prompts = Arc::new(Mutex::new(Vec::<(Uuid, String)>::new()));
    let injected_prompts_seen = Arc::clone(&injected_prompts);
    let app_for_inject = app.clone();
    let inject = move |session_id: Uuid, prompt: String| {
        let app = app_for_inject.clone();
        let injected_prompts = Arc::clone(&injected_prompts_seen);
        async move {
            injected_prompts
                .lock()
                .unwrap()
                .push((session_id, prompt.clone()));
            crate::pty::inject::inject_text_into_session(&app, session_id, &prompt).await
        }
    };
    let boundaries = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let boundaries_seen = Arc::clone(&boundaries);
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let boundaries = Arc::clone(&boundaries_seen);
        async move {
            boundaries.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        source_id,
        fixture.replica.clone(),
        "pi".into(),
        "A".into(),
        None,
        SELF_HANDOFF_FLAVOR_RESTART,
        Arc::clone(&pending),
        Duration::ZERO,
        Duration::ZERO,
        // The real app-backed restart can exceed one second under the full parallel
        // suite. Only settle and poll need to be zero here.
        Duration::from_secs(30),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    // See T11's doc comment: the shared driver calls persist once for BOTH flavors, so the
    // count pins the un-forked control flow, not the inertness. The `None` read below does
    // NOT pin the inertness either, and is not claimed to: this test drives
    // `drive_self_switch_after_sustained_idle` directly, never
    // `run_self_restart_after_sustained_idle`, and every write-capable seam on its path is a
    // closure this test itself supplies, so the same read stays `None` under
    // SELF_HANDOFF_FLAVOR_SWITCH. It is kept only because it reads a real value cheaply. The
    // invariant rests on A3 and T4 alone.
    assert_eq!(
        *persist_calls.lock().unwrap(),
        1,
        "the flavor parameter must not fork the shared driver's control flow"
    );
    assert_eq!(
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(&fixture.replica),
        None,
        "no closure this test supplies writes the replica currentCodingAgent cell; per the \
             note above this does NOT discriminate the flavor"
    );
    let target_id = restarted_id
        .lock()
        .unwrap()
        .expect("restart seam must return the reseeded Pi session id");
    assert_ne!(target_id, source_id);
    let prompts = injected_prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, target_id);
    assert!(
        prompts[0].1.contains("self-handoff-and-restart"),
        "{}",
        prompts[0].1
    );
    assert_eq!(
        mock_pty_writes_for(&app, target_id),
        vec![
            prompts[0].1.as_bytes().to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    assert_eq!(
        *boundaries.lock().unwrap(),
        vec![(target_id, SelfClearBoundary::ContentInjected)]
    );
    assert!(pending.is_empty());
}

/// #1632 - mutual exclusion over the shared, NON-msg-id-scoped names in the process-global
/// Root Agent directory. `root_agent_dir()` is `OnceLock`-cached (config/root_agent.rs), so
/// every Root e2e in this test binary works against the SAME directory, and libtest runs them
/// on parallel threads. A `TempDir` cannot stand in, which is why a lock is needed at all.
///
/// SCOPE, stated wider than one filename on purpose, because stating it narrowly is how the
/// next person reintroduces this bug: the lock protects **every non-msg-id-scoped name under
/// the process-global Root dir**, currently `SELF-HANDOFF.md` and `self-clear/*_SELF-FORGET.md`.
/// Any test that **seeds or glob-removes** either one MUST hold this guard for its whole
/// seed-to-remove window. The second name matters even though it is inert today: nothing
/// seeds a Root-cwd `SELF-FORGET.md`, so `clear_root_self_clear_artifacts`'s trailing glob is
/// a no-op, and that helper has two deliberately UNLOCKED callers (the landed negative e2e
/// and the #1632 negative e2e, both of which reject at anti-spoof and are correctly barred
/// from taking the lock). Add a Root e2e with a forgotten summary and that glob starts
/// deleting another test's archive mid-flight, which is this bug's class exactly.
///
/// Tests that never touch either name must NOT take it: a lock nobody contends is only cost.
///
/// `tokio::sync::Mutex` rather than `std::sync::Mutex` for two reasons, the second decisive
/// on its own: the guard is held across `.await` by construction (`process_message(...).await`
/// sits inside the window), which `clippy::await_holding_lock` rejects under the
/// `-D warnings` CI gate; and `std::sync::Mutex` POISONS on panic, so one genuine failure
/// would become two, with the second pointing at the lock rather than at the defect.
/// The `OnceLock` wrapper avoids depending on `tokio::sync::Mutex::const_new`.
fn root_self_handoff_lock() -> &'static tokio::sync::Mutex<()> {
    static ROOT_SELF_HANDOFF_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
        std::sync::OnceLock::new();
    ROOT_SELF_HANDOFF_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Best-effort removal of one msg-id's outbox artifacts so the Root e2e tests are
/// idempotent across runs. They must write under the process-global
/// `root_agent_dir()` (a fixed path, NOT a throwaway TempDir - see the test docs
/// for why), so stale delivered/rejected/response files from a prior run could
/// otherwise skew assertions. Each of the four Root e2es uses a unique msg-id, so this is
/// safe even when they run in parallel.
///
/// #626/#1632 NOTE: this helper deliberately does NOT touch `SELF-HANDOFF.md`. That file is a
/// single SHARED (non-msg-id-scoped) name; if this start-of-test cleanup removed it, a negative
/// e2e (which rejects at anti-spoof and never seeds it) could delete a positive e2e's
/// freshly-seeded handoff file mid-flight when they run in parallel - a flaky failure.
/// `root_self_handoff_lock()` is what makes that safe: both seeders of the shared name hold it
/// across their whole seed-to-remove window. This helper is deliberately NOT lock-held, because
/// its two negative-e2e callers never seed the name; that is only sound while this helper keeps
/// its hands off it, which is why the rule above is stated rather than assumed.
/// `self-clear/*_SELF-FORGET.md` is glob-removed defensively (no Root e2e seeds SELF-FORGET.md,
/// so the archive is a no-op and nothing is normally created - this only guards against a stale
/// file from a crash). That glob is the lock's SECOND protected name: it is unscoped by message
/// id, so the moment any Root e2e does seed a forgotten summary, this sweep must move behind
/// `root_self_handoff_lock()` too.
fn clear_root_self_clear_artifacts(cwd: &Path, msg_id: &str, request_id: &str) {
    let outbox = cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    let _ = std::fs::remove_file(outbox.join(format!("{}.json", msg_id)));
    let _ = std::fs::remove_file(outbox.join("delivered").join(format!("{}.json", msg_id)));
    let _ = std::fs::remove_file(
        outbox
            .join("rejected")
            .join(format!("{}.reason.txt", msg_id)),
    );
    let _ = std::fs::remove_file(outbox.join("rejected").join(format!("{}.json", msg_id)));
    let _ = std::fs::remove_file(
        cwd.join(crate::config::agent_local_dir_name())
            .join("responses")
            .join(format!("{}.json", request_id)),
    );
    // Defensive: drop any stale <ts>_SELF-FORGET.md archive under the shared root's self-clear/
    // (none is created in the normal e2e flow since neither test seeds SELF-FORGET.md).
    if let Ok(rd) = std::fs::read_dir(cwd.join("self-clear")) {
        for entry in rd.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with("_SELF-FORGET.md")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// #617 HIGH-1 e2e (production-faithful): a token-authorized Root self-clear
/// travels the FULL `process_message` agent-outbox path (NOT `handle_self_clear`
/// directly) and ENQUEUES.
///
/// Fidelity, verified against the code (not assumed):
/// - A real Root Agent runs `self-clear --token <session-UUID>`. That UUID is
///   neither the root_token nor the master token, so `validate_cli_token`
///   (cli/mod.rs) returns is_root=false and `self_clear::execute` writes to the
///   Root's AGENT outbox; the poller processes agent outboxes with
///   `is_app_outbox=false`. So this test passes `false` and BOTH anti-spoof gates
///   run, exactly like production. (An earlier draft used `true`, which skips the
///   outbox-sender gate and is NOT the real path.)
/// - With the Root cwd both gates derive ROOT_AGENT_SENDER (outbox-sender via
///   `sender_name_for_session_cwd`, token-root via the root-flagged variant), so
///   the corrected sender passes both. The cwd MUST be the process-global
///   `root_agent_dir()` so `is_root_agent_path` returns true; `config_dir()` /
///   `root_agent_dir()` are OnceLock-cached, so a throwaway TempDir cannot stand
///   in. Under `cargo test`, `root_agent_dir()` resolves beneath the test
///   binary's target dir (never a real install) and the flow only ADDS files, so
///   it is isolated and non-destructive.
/// - `from` is computed by `resolve_self_clear_sender` - the SAME helper
///   `execute` uses - so this is a genuine gate: revert the fix and the helper
///   yields the path FQN, the daemon rejects it, and the enqueue assertion fails.
#[tokio::test]
async fn process_message_root_self_clear_with_canonical_sender_queues() {
    // (#1632) This test is NO LONGER the sole owner of SELF-HANDOFF.md in the shared
    // root_agent_dir: `process_message_root_self_restart_with_canonical_sender_queues` seeds
    // and removes the same single name. A single-owner convention cannot absorb a second
    // owner, so the convention became an enforced invariant: both seeders hold
    // `root_self_handoff_lock()` across their whole seed-to-remove window, and a lock only
    // one side takes is not a lock. Nothing else in this test changed.
    let _root_handoff_guard = root_self_handoff_lock().lock().await;
    let root_cwd =
        PathBuf::from(crate::config::root_agent::root_agent_dir().expect("resolve root agent dir"));
    std::fs::create_dir_all(&root_cwd).unwrap();
    clear_root_self_clear_artifacts(&root_cwd, "msg-sc-root-e2e-pos", "rid-sc-root-e2e-pos");
    // #626 existence gate: seed SELF-HANDOFF.md so the positive path still queues. The guard
    // taken above, not an ownership convention, is what makes the seed-to-remove window
    // exclusive against the other Root seeder. NOTE: deliberately do NOT seed SELF-FORGET.md
    // here (keep the archive a no-op so no timestamped litter in the shared dir).
    seed_self_handoff(&root_cwd);

    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let (session_id, token) =
        seed_self_clear_session(&app, &root_cwd.to_string_lossy(), "claude").await;
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }

    // The exact `from` the corrected CLI stamps for the Root cwd.
    let canonical_from =
        crate::cli::self_clear::resolve_self_clear_sender(&root_cwd.to_string_lossy());
    assert_eq!(
        canonical_from,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "fix precondition: the CLI must stamp ROOT_AGENT_SENDER for the Root cwd"
    );

    let (path, _msg) = build_self_clear_message_with_from(
        &root_cwd,
        "msg-sc-root-e2e-pos",
        "rid-sc-root-e2e-pos",
        Some(token.to_string()),
        &canonical_from,
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("process_message should succeed");

    // Enqueued: the queue-ack response is "queued", exactly one id is pending, and
    // no reject reason file was written by either anti-spoof gate.
    assert_eq!(
        read_self_clear_response_status(&root_cwd, "rid-sc-root-e2e-pos").as_deref(),
        Some("queued")
    );
    assert!(
        pending_self_clear_contains(&app, session_id).await,
        "canonical Root sender must enqueue the self-clear"
    );
    assert_eq!(pending_self_clear_len(&app).await, 1);
    let reason = root_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sc-root-e2e-pos.reason.txt");
    assert!(
        !reason.exists(),
        "canonical Root sender must NOT be rejected by either anti-spoof gate"
    );
    assert!(
        !path.exists(),
        "message must be consumed (moved to delivered/)"
    );

    // (#1632) Remove the shared name inside the guard's scope. This test no longer OWNS
    // SELF-HANDOFF.md: the #1632 Root restart e2e seeds and removes the same single name, and
    // `root_self_handoff_lock()`, taken at the top of this body, is what makes the
    // seed-to-remove window exclusive.
    let _ = std::fs::remove_file(root_cwd.join("SELF-HANDOFF.md"));
}

/// #617 HIGH-1 e2e negative (production-faithful): the EXACT buggy sender the old
/// CLI produced - `agent_name_from_root(<Root cwd>)`, a path-derived FQN, not
/// ROOT_AGENT_SENDER - is REJECTED on the same full `process_message` agent-outbox
/// path, so nothing queues. This is why the capability was dead in production
/// before the fix. In the agent-outbox path the outbox-sender gate is the first to
/// fire (the buggy `from` does not match the Root-derived outbox owner); the
/// token-root gate would reject it too. Proves the `from` value is load-bearing.
#[tokio::test]
async fn process_message_root_self_clear_with_buggy_sender_is_rejected() {
    let root_cwd =
        PathBuf::from(crate::config::root_agent::root_agent_dir().expect("resolve root agent dir"));
    std::fs::create_dir_all(&root_cwd).unwrap();
    clear_root_self_clear_artifacts(&root_cwd, "msg-sc-root-e2e-neg", "rid-sc-root-e2e-neg");

    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let (session_id, token) =
        seed_self_clear_session(&app, &root_cwd.to_string_lossy(), "claude").await;
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }

    // Reproduce the EXACT value the pre-fix CLI stamped for the Root cwd.
    let buggy_from = crate::cli::send::agent_name_from_root(&root_cwd.to_string_lossy());
    assert_ne!(
        buggy_from,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "precondition: the pre-fix sender must differ from the canonical Root sender"
    );

    let (path, _msg) = build_self_clear_message_with_from(
        &root_cwd,
        "msg-sc-root-e2e-neg",
        "rid-sc-root-e2e-neg",
        Some(token.to_string()),
        &buggy_from,
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("process_message returns Ok even when it rejects");

    // Rejected by an anti-spoof gate (mismatch); nothing queued.
    let reason_path = root_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sc-root-e2e-neg.reason.txt");
    let reason = std::fs::read_to_string(&reason_path)
        .expect("buggy Root sender must be rejected with a reason file");
    assert!(
        reason.contains("mismatch"),
        "expected an anti-spoof mismatch rejection, got: {reason}"
    );
    assert_eq!(pending_self_clear_len(&app).await, 0);
    assert!(
        !pending_self_clear_contains(&app, session_id).await,
        "buggy Root sender must not enqueue anything"
    );
}

/// T23 - #1632 Root e2e (production-faithful): a token-authorized Root
/// self-handoff-and-restart travels the FULL `process_message` agent-outbox path, NOT
/// `handle_self_restart` directly, and ENQUEUES.
///
/// This is the test that proves the Root Agent scope. T8 calls the handler directly and
/// therefore skips BOTH anti-spoof gates; this repository already documents why that is not
/// production-faithful (see the landed self-clear pair above and the warning on
/// `resolve_self_clear_sender`, which states that a wrong sender means the capability is
/// dead in production). #617 shipped exactly that regression once.
///
/// The seeding call is `seed_self_switch_session`, deliberately NOT the landed self-clear
/// e2e's own: `seed_self_clear_session` hardcodes `agent_id: None`, the landed test survives
/// that only because `handle_self_clear` gates on the SHELL stem, and D1's identity gate at
/// step 4 would reject a literal mirror deterministically on every machine, failing all four
/// assertions below and merely duplicating T6's branch.
///
/// `Some("pi")` rather than `Some("claude")` is also deliberate: "pi" is not in
/// `wake_agents()`, so `build_configured_agent_spawn_for_cwd` returns `Ok(None)` at its
/// membership check and **the step-6 pre-flight never touches the process-global Root dir at
/// all**. For an e2e whose subject is the anti-spoof gate pair and whose cwd is a shared
/// directory on a real machine, that independence is the property worth buying. The
/// tripwire below pins the fixture fact that buys it.
#[tokio::test]
async fn process_message_root_self_restart_with_canonical_sender_queues() {
    // (#1632 section 9.0) NON-NEGOTIABLE first line: this test is the second seeder of
    // SELF-HANDOFF.md in the process-global Root dir, and the landed positive self-clear
    // e2e is the first. Both hold this guard across their whole seed-to-remove window.
    let _root_handoff_guard = root_self_handoff_lock().lock().await;
    // Tripwire on this test's load-bearing fixture fact, which lives thousands of lines away
    // in a shared helper. T8, T21 and T23 all rest on "pi" being unconfigured. If anyone
    // later adds `wake_agent("pi", ...)` to `wake_agents()`, the step-6 pre-flight starts
    // running the real spawn builder against the process-global Root dir, which is a hazard
    // class this lock does NOT cover: the directories that builder creates are not under the
    // Root dir at all.
    assert!(
        !wake_agents().iter().any(|a| a.id == "pi"),
        "T23 requires 'pi' to be unconfigured so the step-6 pre-flight never touches the \
            shared Root dir"
    );

    let root_cwd =
        PathBuf::from(crate::config::root_agent::root_agent_dir().expect("resolve root agent dir"));
    std::fs::create_dir_all(&root_cwd).unwrap();
    clear_root_self_clear_artifacts(&root_cwd, "msg-sr-root-e2e-pos", "rid-sr-root-e2e-pos");
    // Existence gate. Deliberately do NOT seed SELF-FORGET.md: the archive stays a no-op so
    // the shared root dir gains no timestamped litter.
    seed_self_handoff(&root_cwd);

    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let (session_id, token) =
        seed_self_switch_session(&app, &root_cwd, "pi", Some("pi"), Some("A"), Some("A")).await;
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }

    // The exact `from` the CLI builder stamps for the Root cwd.
    let canonical_from =
        crate::cli::self_clear::resolve_self_clear_sender(&root_cwd.to_string_lossy());
    assert_eq!(
        canonical_from,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "fix precondition: the CLI must stamp ROOT_AGENT_SENDER for the Root cwd"
    );

    let (path, _msg) = build_self_restart_message_with_from(
        &root_cwd,
        "msg-sr-root-e2e-pos",
        "rid-sr-root-e2e-pos",
        Some(token.to_string()),
        &canonical_from,
    );
    let poller = MailboxPoller::new();
    // `false` is the real agent-outbox path, so BOTH anti-spoof gates run.
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("process_message should succeed");

    assert_eq!(
        read_self_clear_response_status(&root_cwd, "rid-sr-root-e2e-pos").as_deref(),
        Some("queued")
    );
    assert!(
        pending_self_clear_contains(&app, session_id).await,
        "canonical Root sender must enqueue the self-handoff-and-restart"
    );
    assert_eq!(pending_self_clear_len(&app).await, 1);
    let reason = root_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sr-root-e2e-pos.reason.txt");
    assert!(
        !reason.exists(),
        "canonical Root sender must NOT be rejected by either anti-spoof gate"
    );
    assert!(
        !path.exists(),
        "message must be consumed (moved to delivered/)"
    );

    // Remove the shared name inside the guard's scope.
    let _ = std::fs::remove_file(root_cwd.join("SELF-HANDOFF.md"));
}

/// T24 - the negative counterpart, and the control that makes `resolve_self_clear_sender`
/// load-bearing in a way a reviewer can check: without it, T23 could pass against a handler
/// that accepts any sender. The ONLY difference between the two tests is the `from` string.
///
/// T24 does NOT seed `SELF-HANDOFF.md` and does NOT take `root_self_handoff_lock()`. A third
/// seeder of that one shared name would reintroduce the race the lock exists to remove.
/// "Same fixture" means the same root cwd, the same session seeding call, and the same
/// `clear_root_self_clear_artifacts` call with this test's own ids; it does NOT include the
/// handoff seed. This test is rejected at the outbox-sender gate before `handle_self_restart`
/// is ever entered, so its assertions do not depend on that file existing, in either state.
/// Its agent id is likewise not load-bearing on its outcome; naming it anyway is what keeps
/// the pair a genuine same-fixture pair.
#[tokio::test]
async fn process_message_root_self_restart_with_buggy_sender_is_rejected() {
    let root_cwd =
        PathBuf::from(crate::config::root_agent::root_agent_dir().expect("resolve root agent dir"));
    std::fs::create_dir_all(&root_cwd).unwrap();
    clear_root_self_clear_artifacts(&root_cwd, "msg-sr-root-e2e-neg", "rid-sr-root-e2e-neg");

    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let (session_id, token) =
        seed_self_switch_session(&app, &root_cwd, "pi", Some("pi"), Some("A"), Some("A")).await;
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_is_root_agent(session_id, true).await;
    }

    // Reproduce the EXACT value a path-derived sender would stamp for the Root cwd.
    let buggy_from = crate::cli::send::agent_name_from_root(&root_cwd.to_string_lossy());
    assert_ne!(
        buggy_from,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        "precondition: the path-derived sender must differ from the canonical Root sender"
    );

    let (path, _msg) = build_self_restart_message_with_from(
        &root_cwd,
        "msg-sr-root-e2e-neg",
        "rid-sr-root-e2e-neg",
        Some(token.to_string()),
        &buggy_from,
    );
    let poller = MailboxPoller::new();
    poller
        .process_message(&app, &path, OutboxOrigin::Replica)
        .await
        .expect("process_message returns Ok even when it rejects");

    let reason_path = root_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox")
        .join("rejected")
        .join("msg-sr-root-e2e-neg.reason.txt");
    let reason = std::fs::read_to_string(&reason_path)
        .expect("buggy Root sender must be rejected with a reason file");
    assert!(
        reason.contains("mismatch"),
        "expected an anti-spoof mismatch rejection, got: {reason}"
    );
    assert_eq!(pending_self_clear_len(&app).await, 0);
    assert!(
        !pending_self_clear_contains(&app, session_id).await,
        "buggy Root sender must not enqueue anything"
    );
}

// ── err_is_pty_session_missing tests (issue #223) ──

#[test]
fn err_is_pty_session_missing_matches_inject_wrap() {
    // Exact shape emitted by pty::inject::inject_text_into_session
    // (pty/inject.rs:67) when PtyManager::write returns SessionNotFound.
    let e = "PTY write failed: Session not found: 2ced5ccf-1234-5678-9abc-def012345678";
    assert!(err_is_pty_session_missing(e));
}

#[test]
fn err_is_pty_session_missing_matches_bare_and_contextual_forms() {
    // Matches both the canonical bare error and command-branch context.
    assert!(err_is_pty_session_missing("Session not found: abcdef"));
    assert!(err_is_pty_session_missing(
        "Session not found: abcdef - cannot execute logical remote command 'clear'"
    ));
}

#[test]
fn err_is_pty_session_missing_rejects_unrelated_errors() {
    assert!(!err_is_pty_session_missing("PTY error: broken pipe"));
    assert!(!err_is_pty_session_missing(
        "Failed to read outbox file: foo"
    ));
    assert!(!err_is_pty_session_missing(""));
}

/// Pins the actual `AppError::SessionNotFound` Display format. If a future
/// refactor changes `errors.rs:5` to anything other than `"Session not
/// found: {0}"`, this test fails with a clear message — the sniff is the
/// load-bearing safety net for the candidate loop. (grinch G.M3.)
#[test]
fn err_is_pty_session_missing_matches_actual_apperror_format() {
    use crate::errors::AppError;
    let id = uuid::Uuid::new_v4();
    let raw = AppError::SessionNotFound(id.to_string()).to_string();
    assert!(
        err_is_pty_session_missing(&raw),
        "AppError::SessionNotFound display changed to {:?}; \
             err_is_pty_session_missing substring sniff broken — \
             update both the sniff and this test",
        raw
    );
}

/// #1883 — the classifier is the single severity decision for both
/// injection sites: only the canonical deferral prefix downgrades to
/// debug; everything else stays ERROR so real failures keep feeding the
/// Application Error sink.
#[test]
fn classify_injection_error_defers_menu_guard_and_fails_everything_else() {
    assert_eq!(
        classify_injection_error(
            "menu_guard_deferred: session 2ced5ccf-1234-5678-9abc-def012345678 is blocked by \
                interactive menu"
        ),
        (log::Level::Debug, "deferred")
    );
    // Positive control: a real PTY write failure stays ERROR.
    assert_eq!(
        classify_injection_error(
            "PTY write failed: Session not found: 2ced5ccf-1234-5678-9abc-def012345678"
        ),
        (log::Level::Error, "FAILED")
    );
    // The predicate is prefix-based: the token elsewhere in the text is
    // not a deferral.
    assert_eq!(
        classify_injection_error("PTY write failed: menu_guard_deferred note"),
        (log::Level::Error, "FAILED")
    );
}

// ── Anti-spoof / canonicalization pure-logic tests (AR2-tests 22, 23 + DR2-5) ──

/// §DR7 / AR2-tests #22: legacy-unqualified msg.from is accepted when its
/// local part matches the anti-spoof expected_from (local-only fallback).
#[test]
fn anti_spoof_legacy_msg_from_accepted_by_local_match() {
    // expected_from derived from repo path (canonical FQN).
    let expected = "proj-a:wg-1-devs/tech-lead";
    // msg.from is legacy unqualified with same local part.
    let msg_from = "wg-1-devs/tech-lead";
    assert!(anti_spoof_accept(msg_from, expected));
}

/// §DR7 / AR2-tests #23: qualified-but-wrong-project msg.from is rejected.
/// A naïve suffix match would accept this — the LOCAL-only fallback rejects.
#[test]
fn anti_spoof_cross_project_qualified_msg_from_rejected() {
    let expected = "proj-a:wg-1-devs/tech-lead";
    let spoofed = "proj-b:wg-1-devs/tech-lead";
    assert!(!anti_spoof_accept(spoofed, expected));
}

/// Exact-FQN match is trivially accepted (baseline).
#[test]
fn anti_spoof_exact_fqn_match_accepted() {
    let expected = "proj-a:wg-1-devs/tech-lead";
    assert!(anti_spoof_accept(expected, expected));
}

/// §DR2-5 / AR2-tests #25: §AR2-norm step (1) upgrades a legacy-unqualified
/// msg.from to the anti-spoof expected_from FQN. Without this upgrade,
/// grinch §G5's `resolve_repo_path(&msg.from)` response-dir lookup could
/// receive an ambiguous-local-part input.
#[test]
fn process_message_canonicalizes_legacy_msg_from() {
    let mut msg_from = "wg-1-devs/tech-lead".to_string();
    let expected_from = "proj-a:wg-1-devs/tech-lead";
    let upgraded = canonicalize_msg_from_in_place(&mut msg_from, Some(expected_from));
    assert!(upgraded);
    assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
}

/// Already-qualified msg.from is NOT overwritten by canonicalization.
#[test]
fn canonicalize_noop_for_already_qualified_msg_from() {
    let mut msg_from = "proj-a:wg-1-devs/tech-lead".to_string();
    let upgraded = canonicalize_msg_from_in_place(
        &mut msg_from,
        Some("proj-b:wg-1-devs/tech-lead"), // different project — don't overwrite!
    );
    assert!(!upgraded);
    assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
}

/// No expected_from (app outbox path) → canonicalization is a no-op.
#[test]
fn canonicalize_noop_when_expected_from_absent() {
    let mut msg_from = "wg-1-devs/alice".to_string();
    let upgraded = canonicalize_msg_from_in_place(&mut msg_from, None);
    assert!(!upgraded);
    assert_eq!(msg_from, "wg-1-devs/alice");
}

// ── Full mailbox-pipeline tests marked [INT] — placeholders for a future
// two-project fixture harness. Acknowledged to ship with the fix per
// tech-lead's must-apply directive; the pure-logic tests above cover the
// §G1, §G2, §G5 regression surface at the unit level, and §AR2-G1's
// close-session resolver gate is covered by the §AR2-shared resolver
// tests (config::teams::tests::resolve_agent_target_*). ──

/// §G9#1 / AR2-tests #17 — close-session with unqualified target from a
/// direct outbox write MUST NOT destroy sessions in an unauthorized
/// project. Covered at the resolver layer by
/// `resolve_agent_target_rejects_ambiguous` and
/// `resolve_agent_target_two_level_scan` in `config/teams.rs::tests`;
/// `handle_close_session`'s §AR2-G1 gate calls `resolve_agent_target`
/// before authorization, so rejecting Ambiguous at that layer blocks the
/// attack before any session is touched. Full end-to-end fixture needs a
/// Tauri `AppHandle` harness — follow-up.
#[test]
#[ignore = "integration: needs two-project Tauri AppHandle fixture"]
fn close_session_rejects_direct_outbox_write_with_unqualified_target() {
    // Full-pipeline assertion stub; logic-layer coverage lives in:
    //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
    //   - config::teams::tests::resolve_agent_target_two_level_scan
    //   - config::teams::tests::is_coordinator_rejects_legacy_unqualified_from
}

/// §G9#2 / AR2-tests #18 — wake with ambiguous unqualified `to` MUST be
/// rejected, not silently routed. Covered at the resolver layer by the
/// same `resolve_agent_target_rejects_ambiguous` test; `process_message`
/// calls `resolve_agent_target` on `msg.to` at §AR2-norm before mode
/// dispatch, so ambiguous `msg.to` becomes a rejected outbox message.
/// Full end-to-end fixture needs an AppHandle harness.
#[test]
#[ignore = "integration: needs two-project Tauri AppHandle fixture"]
fn deliver_wake_rejects_unqualified_to_with_cross_project_matches() {
    // Full-pipeline assertion stub; logic-layer coverage lives in:
    //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
    //   - config::teams::tests::resolve_agent_target_two_level_scan
}

/// §G9#3 / AR2-tests #19 — `resolve_repo_path` WG fallback with a
/// qualified target honors `target_project` (no cross-project leak even
/// when the base dir `rp` is another project's root). Logic covered by
/// the `project_matches` closure + `dirs_to_check` seeding in the
/// §AR2-G3 block; a full fixture-based test needs filesystem setup
/// under an AppHandle harness.
#[test]
#[ignore = "integration: needs filesystem fixture + AppHandle"]
fn resolve_repo_path_wg_fallback_honors_target_project() {}

/// §G9#4 / AR2-tests #20 — `resolve_repo_path` with an unqualified target
/// matching multiple projects returns `None` (refuses arbitrary pick).
/// §AR2-G4 collector pattern logic is covered by inspection — the
/// `matches.len()` match arm returns None on `_`. Full integration test
/// needs AppHandle + session-CWDs fixture.
#[test]
#[ignore = "integration: needs filesystem fixture + AppHandle"]
fn resolve_repo_path_returns_none_on_ambiguous_unqualified() {}
