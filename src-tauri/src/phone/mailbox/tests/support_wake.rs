use super::*;

pub(super) async fn retained_exited_respawn_queued_cell(pty: bool, inline: bool, scenario: &str) {
    use crate::session::selection::{ReplacementFixtureControl, ReplacementFixtureCut as Cut};
    use std::future::Future;
    let fixture = make_mailbox_fixture();
    // Both actors belong to one unchanged authority history, before restore.
    let (replica, target) = provision_retained_fixture_member(&fixture, "queued-member");
    let app = fixture.app.handle();
    let (coordinator, _, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    let (route, _) =
        prepare_additional_retained_member(&fixture, &coordinator, sender, &replica, &target).await;
    let first = Arc::new(ReplacementFixtureControl::default());
    let (release_first, first_gate) = tokio::sync::oneshot::channel();
    *first.before_release_gate.lock().unwrap() = Some(first_gate);
    let first_hooks = MailboxTestHooks::default();
    *first_hooks.replacement_fixture.lock().unwrap() = Some(first.clone());
    let first_poller = MailboxPoller::new_with_test_hooks(first_hooks.clone());
    let second = Arc::new(ReplacementFixtureControl::default());
    second.ticket_only.store(!inline, Ordering::Release);
    let race = scenario != "queued-drain";
    let (resume_finalizer, finalizer_gate) = tokio::sync::oneshot::channel();
    let (resume_disarmed, disarmed_gate) = tokio::sync::oneshot::channel();
    let (panic_source, source_signal) = tokio::sync::oneshot::channel();
    if race {
        *second.source_panic.lock().unwrap() = Some(source_signal);
        let mut actions = second.actions.lock().unwrap();
        if inline {
            actions.push(crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::BeforeFinalizer,
                resume: finalizer_gate,
            });
        }
        if scenario == "cleanup-finalize-first" {
            actions.push(crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::AfterDisarm,
                resume: disarmed_gate,
            });
        }
    }
    let second_hooks = MailboxTestHooks::default();
    *second_hooks.replacement_fixture.lock().unwrap() = Some(second.clone());
    let second_poller = MailboxPoller::new_with_test_hooks(second_hooks);
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection = pty.then(|| {
        enqueue_shared_pty_engine_operation(&store, &route, sender, b"queued-drain-source")
    });
    let mut first_message = wake_message_to_target();
    first_message.id = format!("drain-blocker-{}", Uuid::new_v4());
    let mut second_message = wake_message_to_target();
    second_message.id = format!("queued-drain-{}", Uuid::new_v4());
    second_message.to = target;
    let (start_second, second_gate) = tokio::sync::oneshot::channel();
    let (finish_outer, outer_finished) = tokio::sync::oneshot::channel();
    let first_delivery = async {
        let _ = retained_fixture_ordinary_wake(&first_poller, app, &first_message, &first).await;
    };
    let second_delivery = async {
        second_gate.await.unwrap();
        let actual_delivery = async {
            if let Some(injection) = injection.as_deref() {
                second_poller
                    .dispatch_pty_input_operation(
                        app,
                        &state,
                        injection,
                        crate::phone::types::PtyInputSourcePlane::HostCli,
                        None,
                    )
                    .await;
            } else {
                let _ =
                    retained_fixture_ordinary_wake(&second_poller, app, &second_message, &second)
                        .await;
            }
        };
        let housekeeping = async {
            if outer_finished.await.is_ok() {
                return;
            }
            std::future::pending::<()>().await;
        };
        tokio::select! {
            () = actual_delivery => {},
            () = housekeeping => assert!(second.observe_owner(&coordinator)
                .is_some_and(|owner| owner.source_stopped),
                "drop only the outer waiter after real accepted-core unwind"),
        }
    };
    let controller = async {
        let first_return = retained_source_wait_cut(&first, Cut::AdapterReturned).await;
        let first_id = first_return.binding.unwrap().session_id();
        let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
        idle.register_session(first_id, crate::session::profile::IdleTuning::DEFAULT);
        idle.set_pty_input_ready_for_test(first_id);
        first_hooks
            .real_inject_sessions
            .lock()
            .unwrap()
            .insert(first_id);
        start_second.send(()).unwrap();
        let queued = retained_source_wait_cut(
            &second,
            if inline {
                Cut::Submitted
            } else {
                Cut::TicketSubmitted
            },
        )
        .await;
        let binding = queued.binding.unwrap();
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        if !inline {
            assert!(
                manager.get_pending_session(binding).await.is_some(),
                "original ticket queued only after actual pending creation/native effects"
            );
        }
        assert!(!second.received.load(Ordering::Acquire));
        assert!(shared_engine_writes(app, binding.session_id()).is_empty());
        if race {
            retained_source_cleanup_race(
                app,
                &coordinator,
                &second,
                RetainedCleanupRaceGates {
                    first: release_first,
                    finalizer: resume_finalizer,
                    disarmed: resume_disarmed,
                    panic_source,
                    claim_first: scenario == "cleanup-claim-first",
                    runtime_changed: scenario == "cleanup-runtime-changed",
                    unknown: scenario == "cleanup-unknown-veto",
                },
                inline,
            )
            .await;
            if scenario == "cleanup-unknown-veto" {
                drop(finish_outer); // Let the actual retained source return normally.
            } else {
                let _ = finish_outer.send(());
            }
            return binding;
        }
        // Poll the same actual close future to publish shutdown while the
        // genuine earlier inline envelope still owns the worker.
        let close = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join());
        tokio::pin!(close);
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(close.as_mut().poll(cx).is_pending()))
                .await
        );
        release_first.send(()).unwrap();
        retained_source_wait_cut(
            &second,
            if inline {
                Cut::Drained
            } else {
                Cut::TicketDrained
            },
        )
        .await;
        retained_source_wait_cut(&second, Cut::ReceiverLost).await;
        let report = close.await.expect("actual queued-drain close");
        assert!(!report.persistence_safe);
        let owner = second.observe_owner(&coordinator).unwrap();
        assert!(!owner.terminal && !owner.transferred);
        if inline {
            assert_eq!(owner.effects_started, 0);
            assert!(manager.get_pending_session(binding).await.is_none());
        } else {
            assert!(
                manager.get_pending_session(binding).await.is_some(),
                "owned shutdown drain neither invents setup error nor rolls back B"
            );
            assert!(!owner.rollback_completed);
        }
        assert!(!second
            .receipts
            .lock()
            .unwrap()
            .iter()
            .any(|receipt| receipt.cut == Cut::Baseline || receipt.cut == Cut::SourceReceived));
        assert!(shared_engine_writes(app, binding.session_id()).is_empty());
        // A closed housekeeping signal is disabled above.
        drop(finish_outer);
        binding
    };
    let (_, _, binding) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(first_delivery, second_delivery, controller)
    })
    .await
    .expect("genuine earlier inline actor and queued source drain");
    if scenario != "cleanup-unknown-veto" {
        assert!(!second.observe_owner(&coordinator).unwrap().transferred);
    } else {
        assert!(second.observe_owner(&coordinator).is_none());
    }
    assert!(shared_engine_writes(app, binding.session_id()).is_empty());
    if let Some(injection) = injection {
        let actual = store
            .query_pty_input_by_injection(&injection)
            .unwrap()
            .unwrap();
        assert!(
            !actual.terminal,
            "drained retained work cannot claim delivery"
        );
    }
}

pub(super) async fn retained_exited_respawn_lifetime_cell(pty: bool, inline: bool) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    let probe = Arc::downgrade(fixture.app.state::<Arc<AppLifetimeProbe>>().inner());
    let retained_handle = fixture.app.handle().clone();
    let (coordinator, backend, owner) = {
        let app = fixture.app.handle();
        let (coordinator, route, sender, _) = prepare_retained_exited_respawn(&fixture).await;
        let backend = {
            let state = app.state::<Arc<Mutex<PtyManager>>>();
            let actual = state
                .lock()
                .unwrap()
                .backend_for_kind(SessionBackendKind::LocalProcess);
            assert!(actual.as_any().is::<MailboxMockPtyBackend>());
            Arc::downgrade(&actual)
        };
        let control = Arc::new(ReplacementFixtureControl::default());
        control.ticket_only.store(!inline, Ordering::Release);
        control.actions.lock().unwrap().push(Action::PanicOnce {
            cut: Cut::SourceCoreEntered,
        });
        let hooks = MailboxTestHooks::default();
        *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
        let poller = MailboxPoller::new_with_test_hooks(hooks);
        let state = app.state::<crate::api::message_store::MessageStoreState>();
        let store = state.store.as_ref().unwrap().clone();
        let injection = pty.then(|| {
            enqueue_shared_pty_engine_operation(&store, &route, sender, b"lifetime-source")
        });
        let mut message = wake_message_to_target();
        message.id = format!("lifetime-source-{}", Uuid::new_v4());
        let delivery = async {
            if let Some(injection) = injection.as_deref() {
                poller
                    .dispatch_pty_input_operation(
                        app,
                        &state,
                        injection,
                        crate::phone::types::PtyInputSourcePlane::HostCli,
                        None,
                    )
                    .await;
            } else {
                let _ = retained_fixture_ordinary_wake(&poller, app, &message, &control).await;
            }
        };
        let stopped = async {
            loop {
                if control
                    .observe_owner(&coordinator)
                    .is_some_and(|owner| owner.source_stopped)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::select! {
                () = delivery => panic!("source panic must not fabricate outer completion"),
                () = stopped => {},
            }
        })
        .await
        .expect("actual source stack unwind observation");
        let owner = control
            .owner_weak(&coordinator)
            .expect("actual retained owner");
        assert!(owner.is_alive());
        let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
            .await
            .expect("actual lifetime close");
        assert!(
            !report.persistence_safe,
            "incomplete Preparing remains unsafe"
        );
        assert!(control
            .observe_owner(&coordinator)
            .is_some_and(|owner| !owner.terminal && !owner.transferred));
        (coordinator, backend, owner)
    };
    drop(fixture);
    assert!(
        probe.upgrade().is_some(),
        "positive actual AppHandle retains managed state"
    );
    drop(retained_handle);
    tokio::time::timeout(Duration::from_secs(20), async {
        while probe.upgrade().is_some() || backend.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual managed app/backend captures expire after unwind finishes");
    assert!(
        owner.is_alive(),
        "incomplete owner survives solely in actual coordinator root"
    );
    drop(coordinator);
    tokio::time::timeout(Duration::from_secs(20), async {
        while owner.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual root destruction and finished transient drops release incomplete owner");
}

/// Parent-owned process and directory have one lifetime. Exit isolates
/// process statics only; the child must prove product disposition first.
pub(super) struct RetainedSourceChild {
    pub(super) child: std::process::Child,
    pub(super) directory: Option<tempfile::TempDir>,
    pub(super) reaped: bool,
}

impl RetainedSourceChild {
    pub(super) fn logs(&self) -> String {
        let Some(directory) = &self.directory else {
            return String::new();
        };
        ["stdout.log", "stderr.log"]
            .iter()
            .map(|name| {
                format!(
                    "{name}:\n{}",
                    std::fs::read_to_string(directory.path().join(name))
                        .unwrap_or_else(|error| format!("log unavailable: {error}"))
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn supervise(&mut self, identity: &str, scenario: &str) {
        let seconds = match scenario {
            "reclaim-66" | "entry-restart-reclaim" | "entry-root-reclaim" => 300,
            _ => 60,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    assert!(
                        status.success(),
                        "{identity}: child {status}\n{}",
                        self.logs()
                    );
                    let receipt = std::fs::read_to_string(
                        self.directory
                            .as_ref()
                            .unwrap()
                            .path()
                            .join("completion.txt"),
                    );
                    assert_eq!(
                        receipt.as_deref().ok(),
                        Some(identity),
                        "{identity}: missing/mismatched assertion receipt\n{}",
                        self.logs()
                    );
                    return;
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    // Supervision alone, never product timing evidence.
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => panic!("{identity}: child deadline\n{}", self.logs()),
                Err(error) => {
                    panic!("{identity}: child polling failed: {error}\n{}", self.logs())
                }
            }
        }
    }
}

impl Drop for RetainedSourceChild {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        let kill = self.child.kill();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped = true;
                    break;
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        if !self.reaped {
            if let Some(directory) = self.directory.take() {
                let retained = directory.keep();
                eprintln!(
                    "retained-source child cleanup uncertain; kill={kill:?}; logs retained at {}",
                    retained.display()
                );
            }
        }
    }
}

pub(super) fn enqueue_shared_pty_engine_operation_issued_at(
    store: &crate::api::message_store::MessageStore,
    route: &crate::config::teams::VerifiedPtyInputRoute,
    sender_id: Uuid,
    text: &[u8],
    issued_at: chrono::DateTime<chrono::Utc>,
) -> String {
    let injection_id = Uuid::new_v4().to_string();
    store
        .enqueue_pty_input(crate::api::message_store::PtyInputEnqueueRequest {
            injection_id: injection_id.clone(),
            sender_fqn: route.sender.canonical_fqn.clone(),
            target_fqn: route.target.canonical_fqn.clone(),
            op_id: injection_id.clone(),
            nonce_sha256: crate::phone::types::sha256_hex(Uuid::new_v4().to_string().as_bytes()),
            request_fingerprint: crate::phone::types::sha256_hex(
                format!("shared-engine:{injection_id}").as_bytes(),
            ),
            confirmation_tag: Some("d".repeat(64)),
            requested_agent_id: None,
            payload: text.to_vec(),
            source_plane: crate::phone::types::PtyInputSourcePlane::HostCli,
            sender_incarnation_fingerprint: route.sender.incarnation_fingerprint.clone(),
            sender_identity_fingerprint: route.sender.authority_fingerprint.clone(),
            target_identity_fingerprint: route.target.authority_fingerprint.clone(),
            authority_session_id: sender_id.to_string(),
            authority_client_id: None,
            authority_client_generation: None,
            issued_at: crate::phone::types::canonical_pty_timestamp(issued_at),
            expires_at: crate::phone::types::canonical_pty_timestamp(
                issued_at + chrono::Duration::minutes(10),
            ),
        })
        .unwrap();
    injection_id
}

pub(super) fn shared_engine_writes(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    session_id: Uuid,
) -> Vec<Vec<u8>> {
    let manager = app.state::<Arc<Mutex<PtyManager>>>();
    let manager = manager.lock().unwrap();
    manager
        .backend_for_kind(crate::pty::backend::SessionBackendKind::LocalProcess)
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .unwrap()
        .writes_for(session_id)
}

pub(super) async fn add_mailbox_session_with_shell<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &Path,
    name: &str,
    shell: &str,
    status: SessionStatus,
) -> Uuid {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            shell.to_string(),
            Vec::new(),
            cwd.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    mgr.rename_session(session.id, name.to_string())
        .await
        .unwrap();
    match status {
        SessionStatus::Active => mgr.switch_session(session.id).await.unwrap(),
        SessionStatus::Running => {}
        SessionStatus::Idle => mgr.mark_idle(session.id).await,
        SessionStatus::Exited(code) => {
            mgr.mark_exited(session.id, code).await;
        }
    }
    session.id
}

pub(super) fn register_mock_pty_route<R: tauri::Runtime>(app: &tauri::AppHandle<R>, id: Uuid) {
    let pty_mgr = app.state::<Arc<std::sync::Mutex<PtyManager>>>();
    let manager = pty_mgr.lock().unwrap();
    manager.record_route(id, SessionBackendKind::LocalProcess);
    let backend = manager.backend_for_kind(SessionBackendKind::LocalProcess);
    backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("mailbox mock PTY backend")
        .set_live(id);
}

pub(super) fn mock_pty_writes_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    id: Uuid,
) -> Vec<Vec<u8>> {
    let pty_mgr = app.state::<Arc<std::sync::Mutex<PtyManager>>>();
    let manager = pty_mgr.lock().unwrap();
    let backend = manager.backend_for_kind(SessionBackendKind::LocalProcess);
    backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("mailbox mock PTY backend")
        .writes_for(id)
}

/// #1883 — test-only write-failure switch. `MailboxFixture` keeps no backend
/// handle (`make_mailbox_app` discards it), so this downcast helper mirrors
/// `register_mock_pty_route` / `mock_pty_writes_for`.
pub(super) fn set_mock_pty_write_failure<R: tauri::Runtime>(app: &tauri::AppHandle<R>, fail: bool) {
    let pty_mgr = app.state::<Arc<std::sync::Mutex<PtyManager>>>();
    let manager = pty_mgr.lock().unwrap();
    let backend = manager.backend_for_kind(SessionBackendKind::LocalProcess);
    backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("mailbox mock PTY backend")
        .fail_write
        .store(fail, Ordering::SeqCst);
}

pub(super) fn write_wake_outbox_message(sender_cwd: &Path, msg_id: &str) -> PathBuf {
    write_wake_outbox_message_with_route(sender_cwd, msg_id, CANONICAL_WAKE_FROM, CANONICAL_WAKE_TO)
}

pub(super) fn write_wake_outbox_message_with_route(
    sender_cwd: &Path,
    msg_id: &str,
    from: &str,
    to: &str,
) -> PathBuf {
    let outbox_dir = sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let message_path = outbox_dir.join(format!("{}.json", msg_id));
    let msg = OutboxMessage {
        id: msg_id.into(),
        token: Some(MAILBOX_MASTER_TOKEN.into()),
        from: from.into(),
        to: to.into(),
        body: WAKE_BODY.into(),
        mode: "wake".into(),
        get_output: false,
        request_id: None,
        sender_agent: Some("codex".into()),
        preferred_agent: "codex".into(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-06-11T00:00:00Z".into(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    std::fs::write(&message_path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    message_path
}

pub(super) fn assert_no_spawn_or_destroy_events(hooks: &MailboxTestHooks) {
    let events = hooks.events.lock().unwrap().clone();
    assert!(
        events.iter().all(|event| !matches!(
            event,
            MailboxTestEvent::Spawn(_) | MailboxTestEvent::Destroy(_)
        )),
        "unexpected spawn or destroy event: {:?}",
        events
    );
}

pub(super) fn assert_inject_results_consumed(hooks: &MailboxTestHooks) {
    assert!(
        hooks.inject_results.lock().unwrap().is_empty(),
        "all scripted inject results should be consumed"
    );
}

pub(super) fn assert_spawn_call_matches_target(call: &MailboxSpawnCall, fixture: &MailboxFixture) {
    assert_eq!(call.to, CANONICAL_WAKE_TO);
    assert_eq!(call.cwd, fixture.target_cwd.to_string_lossy().to_string());
    assert_eq!(call.session_name, LOCAL_WAKE_TO);
    assert_eq!(call.shell, "codex");
    assert_eq!(call.shell_args, vec!["--yolo"]);
}

pub(super) async fn assert_spawned_session_matches_target<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    fixture: &MailboxFixture,
) {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let spawned = mgr.get_session(session_id).await.unwrap();
    assert_eq!(
        spawned.working_directory,
        fixture.target_cwd.to_string_lossy().to_string()
    );
    assert_eq!(spawned.name, LOCAL_WAKE_TO);
    assert_eq!(spawned.shell, "codex");
    assert_eq!(spawned.shell_args, vec!["--yolo"]);
    assert!(spawned.waiting_for_input);
}

pub(super) async fn run_mailbox_message<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    message_path: &Path,
    hooks: MailboxTestHooks,
) {
    let msg_id = message_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap()
        .to_string();
    let delivered = message_path
        .parent()
        .unwrap()
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    Box::pin(poller.process_message(app, message_path, OutboxOrigin::Replica))
        .await
        .expect("process mailbox message");
    assert!(!message_path.exists());
    assert!(delivered.exists());
    let delivered_msg: OutboxMessage =
        serde_json::from_str(&std::fs::read_to_string(&delivered).unwrap()).unwrap();
    assert_eq!(delivered_msg.id, msg_id);
    assert_eq!(delivered_msg.token, None);
    assert_eq!(delivered_msg.from, CANONICAL_WAKE_FROM);
    assert_eq!(delivered_msg.to, CANONICAL_WAKE_TO);
    assert_eq!(delivered_msg.body, WAKE_BODY);
    assert_eq!(delivered_msg.mode, "wake");
}

// live_settle_action gates the LIVE path on real-time activity_age (grinch P1),
// never the lagged waiting_for_input. idle_threshold 2500ms, settle 3500ms.
pub(super) const LSA_IDLE_THRESHOLD: Duration = Duration::from_millis(2500);

pub(super) const LSA_SETTLE: Duration = Duration::from_millis(3500);

pub(super) const LSA_RESIZE_GRACE: Duration = Duration::from_millis(4000);

pub(super) const LSA_MAX_WAIT: Duration = Duration::from_secs(10);

// ── #617 handle_self_clear harness tests (#[cfg(not(test))] spawn gate ⇒ no live poller) ──

pub(super) async fn seed_self_clear_session(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    cwd: &str,
    shell: &str,
) -> (Uuid, Uuid) {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            shell.into(),
            vec![],
            cwd.to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    (session.id, session.token)
}

/// #626 - seed the agent's `SELF-HANDOFF.md` so the existence gate passes. The gate (§4.2) rejects
/// any self-handoff-and-clear whose root has no `SELF-HANDOFF.md`, so every test that expects a
/// "queued"/"already_queued" outcome must seed it first.
pub(super) fn seed_self_handoff(cwd: &Path) {
    std::fs::write(cwd.join("SELF-HANDOFF.md"), "resume notes for the test").unwrap();
}

/// #626/#636 - count archived SELF-FORGET files in `cwd/self-clear/` (suffix match; the wall-clock
/// timestamp prefix in the real archive name is unpredictable, so the harness asserts by suffix,
/// not exact name). Returns 0 if `self-clear/` does not exist (read_dir errors -> unwrap_or(0)).
pub(super) fn count_forget_archives(cwd: &Path) -> usize {
    std::fs::read_dir(cwd.join("self-clear"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().ends_with("_SELF-FORGET.md"))
                .count()
        })
        .unwrap_or(0)
}

pub(super) fn read_only_forget_archive(cwd: &Path) -> String {
    let archives: Vec<PathBuf> = std::fs::read_dir(cwd.join("self-clear"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|name| name.to_string_lossy().ends_with("_SELF-FORGET.md"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        archives.len(),
        1,
        "expected exactly one SELF-FORGET archive"
    );
    std::fs::read_to_string(&archives[0]).unwrap()
}

pub(super) fn build_self_clear_message(
    cwd: &Path,
    msg_id: &str,
    request_id: &str,
    token: Option<String>,
) -> (PathBuf, OutboxMessage) {
    // Delegate to the parameterized builder with the fixed dev-rust sender, so the
    // OutboxMessage literal lives in exactly one place.
    build_self_clear_message_with_from(
        cwd,
        msg_id,
        request_id,
        token,
        "proj-a:wg-1-dev-team/dev-rust",
    )
}

pub(super) fn read_response_json(cwd: &Path, request_id: &str) -> Option<serde_json::Value> {
    let resp = cwd
        .join(crate::config::agent_local_dir_name())
        .join("responses")
        .join(format!("{}.json", request_id));
    let content = std::fs::read_to_string(&resp).ok()?;
    serde_json::from_str(&content).ok()
}

/// P18 uses the existing exact-child supervisor and its after-assertion receipt.
pub(super) fn p18_isolated_child_or_parent(row: &str) -> bool {
    const MARKER: &str = "AC_P18_ISOLATED_CHILD";
    const IDENTITY: &str = "AC_P18_ISOLATED_IDENTITY";
    assert!(matches!(
        row,
        "worker_rejects"
            | "input_error_no_replay"
            | "reject_io_pending"
            | "followup_eligibility"
    ));
    let wrapper = format!(
        "phone::mailbox::tests::wake_dispatch_2::issue_3263_minimal_retained_{row}"
    );
    if let Some(marked) = std::env::var_os(MARKER) {
        assert_eq!(marked.to_str(), Some(wrapper.as_str()));
        let identity = std::env::var(IDENTITY).expect("P18 child identity");
        let (run, expected) = identity.split_once('|').expect("run-bound P18 identity");
        Uuid::parse_str(run).expect("P18 parent nonce");
        assert_eq!(expected, wrapper);
        let config = PathBuf::from(
            std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR").expect("P18 child config"),
        );
        let test_config = PathBuf::from(
            std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("P18 test config"),
        );
        assert!(config.is_absolute());
        assert_eq!(config, test_config);
        assert_eq!(crate::config::config_dir().as_ref(), Some(&config));
        assert!(!config
            .join("completion.txt")
            .try_exists()
            .expect("inspect P18 receipt"));
        return false;
    }
    assert!(std::env::var_os(IDENTITY).is_none());
    let directory = tempfile::tempdir().expect("P18 child directory");
    let path = directory
        .path()
        .canonicalize()
        .expect("absolute P18 directory");
    let identity = format!("{}|{wrapper}", Uuid::new_v4());
    let stdout = std::fs::File::create(path.join("stdout.log")).expect("P18 stdout");
    let stderr = std::fs::File::create(path.join("stderr.log")).expect("P18 stderr");
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([wrapper.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .env(MARKER, &wrapper)
        .env(IDENTITY, &identity)
        .env("AGENTSCOMMANDER_CONFIG_DIR", &path)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command.spawn().expect("spawn exact P18 child");
    let mut owner = RetainedSourceChild {
        child,
        directory: Some(directory),
        reaped: false,
    };
    owner.supervise(&identity, "p18-isolated");
    true
}

pub(super) fn complete_p18_isolated_child(row: &str) {
    assert!(std::env::var_os("AC_P18_ISOLATED_CHILD").is_some());
    assert!(!p18_isolated_child_or_parent(row));
    let identity = std::env::var("AC_P18_ISOLATED_IDENTITY").expect("P18 completion identity");
    let config = PathBuf::from(
        std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR").expect("P18 completion config"),
    );
    std::fs::write(config.join("completion.txt"), identity).expect("P18 assertion receipt");
}
