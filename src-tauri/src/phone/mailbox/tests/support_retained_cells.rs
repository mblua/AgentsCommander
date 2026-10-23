use super::*;

pub(super) async fn assert_retained_failure_close(
    coordinator: &crate::session::selection::SelectionCoordinator,
    setup_rolled_back: bool,
) {
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual failure close");
    if !setup_rolled_back {
        assert!(
            !report.persistence_safe,
            "incomplete failure cannot certify safety"
        );
    } else {
        assert!(
            report.persistence_safe,
            "actual returned setup failure must finish its own verified rollback/handoff: {:?}",
            report.retained
        );
        assert_eq!(coordinator.fixture_root_count(), Some(0));
    }
}

pub(super) async fn retained_exited_respawn_failure_cell(pty: bool, inline: bool, scenario: &str) {
    use crate::session::selection::{ReplacementFixtureControl, ReplacementFixtureCut as Cut};
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let control = Arc::new(ReplacementFixtureControl::default());
    control.ticket_only.store(!inline, Ordering::Release);
    if scenario == "native-seed-panic" {
        let seed_source = fixture
            .target_cwd
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("default.p05-seed");
        configure_retained_seed_fixture(&fixture, &seed_source).await;
    }
    if scenario == "pre-context" {
        *control.context_notification.lock().unwrap() =
            crate::session::selection::FixtureContextNotification::Armed;
    }
    if scenario == "pre-context" || scenario == "native-context-panic" {
        configure_retained_context_fixture(&fixture).await;
    }
    let (resume_context, context_gate) = tokio::sync::oneshot::channel();
    let mut context_gate = Some(context_gate);
    let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let backend = pty_manager
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("actual mock backend");
    control.mock_backend.store(true, Ordering::Release);
    let kills_before = backend.kill_attempts.load(Ordering::SeqCst);
    let physical_cut = match scenario {
        "pre-finalize" => Some(Cut::BeforeFinalizer),
        "post-carry-runtime" => Some(Cut::BeforeCarry),
        "post-rename-physical" => Some(Cut::PublicationPrepared),
        "post-retire-runtime" => Some(Cut::BeforeRetirement),
        _ => None,
    };
    let (resume_physical, physical_gate) = tokio::sync::oneshot::channel();
    let mut physical_gate = Some(physical_gate);
    let old_publication = (scenario == "post-rename-physical").then(|| {
        std::fs::read(crate::config::config_dir().unwrap().join("sessions.json")).unwrap()
    });
    let action = retained_failure_action(
        scenario,
        inline,
        &control,
        backend,
        physical_cut,
        &mut physical_gate,
        &mut context_gate,
    );
    if let Some(action) = action {
        control.actions.lock().unwrap().push(action);
    }
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection =
        pty.then(|| enqueue_shared_pty_engine_operation(&store, &route, sender, b"failure-source"));
    let mut message = wake_message_to_target();
    message.id = format!("failure-source-{}", Uuid::new_v4());
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
            None
        } else {
            retained_fixture_ordinary_wake(&poller, app, &message, &control).await
        }
    };
    let unwound = async {
        loop {
            if control
                .observe_owner(&coordinator)
                .is_some_and(|owner| owner.source_unwound)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    let native_controller = async {
        retained_failure_physical_controller(
            scenario,
            physical_cut,
            &control,
            &coordinator,
            &pty_manager,
            resume_physical,
        )
        .await;
        retained_failure_context_controller(
            scenario,
            &fixture,
            &coordinator,
            &control,
            &manager,
            &pty_manager,
            resume_context,
        )
        .await;
    };
    let ordinary = tokio::time::timeout(Duration::from_secs(20), async {
        let source = async {
            tokio::select! { actual = delivery => actual, () = unwound => None }
        };
        let (actual, ()) = tokio::join!(source, native_controller);
        actual
    })
    .await
    .expect("real failure return or actual accepted-source unwind");
    let receipt = control
        .receipts
        .lock()
        .unwrap()
        .first()
        .copied()
        .expect("actual operation receipt");
    let binding = receipt.binding.expect("actual B binding");
    assert!(
        shared_engine_writes(app, binding.session_id()).is_empty(),
        "no positive source input"
    );
    assert!(
        manager.get_session(previous).await.is_some(),
        "pre-retirement source A retained"
    );
    let owner = control.observe_owner(&coordinator);
    let setup_rolled_back = matches!(
        scenario,
        "pre-backend" | "pre-binder" | "pre-context" | "pre-finalize"
    );
    if setup_rolled_back {
        assert_retained_setup_failure(
            ordinary,
            scenario,
            inline,
            &control,
            &manager,
            &pty_manager,
            binding,
            receipt,
            kills_before,
            backend,
        )
        .await;
    } else {
        let owner = owner.as_ref().expect("incomplete owner remains retained");
        assert!(!owner.terminal && !owner.transferred);
        if scenario == "native-context-panic" || scenario == "native-seed-panic" {
            assert_retained_native_failure(
                &ordinary,
                scenario,
                &control,
                owner,
                &manager,
                binding,
                backend,
                kills_before,
            )
            .await;
        }
        if scenario == "pre-rollback-error" {
            assert_retained_failed_rollback(ordinary, pty, owner, backend, kills_before);
        }
        assert_retained_committed_failure(
            scenario,
            pty,
            &control,
            &coordinator,
            &manager,
            &pty_manager,
            binding,
            owner,
            kills_before,
            backend,
            old_publication,
            receipt,
        )
        .await;
    }
    assert_retained_failure_close(&coordinator, setup_rolled_back).await;
}

pub(super) async fn retained_fixture_ordinary_wake(
    poller: &MailboxPoller,
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    message: &OutboxMessage,
    control: &crate::session::selection::ReplacementFixtureControl,
) -> Option<Result<OutboxMessage, String>> {
    tokio::select! {
        biased;
        actual = poller.deliver_wake(app, message) => Some(actual),
        _ = retained_source_wait_cut(control,
            crate::session::selection::ReplacementFixtureCut::SourceRetainedReturn) => None,
    }
    // None means only that the real core returned Retained and its public
    // pending waiter was dropped. It is never a product result or success.
}

pub(super) async fn retained_source_wait_cut(
    control: &crate::session::selection::ReplacementFixtureControl,
    cut: crate::session::selection::ReplacementFixtureCut,
) -> crate::session::selection::ReplacementFixtureReceipt {
    loop {
        if let Some(receipt) = control
            .receipts
            .lock()
            .unwrap()
            .iter()
            .find(|receipt| receipt.cut == cut)
            .copied()
        {
            return receipt;
        }
        tokio::task::yield_now().await;
    }
}

pub(super) async fn retained_exited_respawn_pty_held_cell(scenario: &str) {
    use crate::session::selection::{
        OwnedSourceStop, ReplacementFixtureAction as Action, ReplacementFixtureControl,
        ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    assert!(app.manage(crate::shutdown::ShutdownSignal::new()));
    let control = Arc::new(ReplacementFixtureControl::default());
    let (resume_validation, validation) = tokio::sync::oneshot::channel();
    let (resume_bridge, bridge) = tokio::sync::oneshot::channel();
    let (lose_receiver, receiver) = tokio::sync::oneshot::channel();
    let mut lose_receiver = Some(lose_receiver);
    *control.live_receiver_drop.lock().unwrap() = Some(receiver);
    control.actions.lock().unwrap().extend([
        Action::Pause {
            cut: Cut::BeforeSourceValidation,
            resume: validation,
        },
        Action::Pause {
            cut: Cut::AdapterOutcomeBeforeLeaseDrop,
            resume: bridge,
        },
    ]);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    if scenario == "pty-held-rollback" {
        let backend = pty
            .lock()
            .unwrap()
            .backend_for_kind(SessionBackendKind::LocalProcess);
        backend
            .as_any()
            .downcast_ref::<MailboxMockPtyBackend>()
            .unwrap()
            .fail_spawn
            .store(true, Ordering::SeqCst);
    }
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection =
        enqueue_shared_pty_engine_operation(&store, &route, sender, b"held-clone-source");
    let delivery = poller.dispatch_pty_input_operation(
        app,
        &state,
        &injection,
        crate::phone::types::PtyInputSourcePlane::HostCli,
        None,
    );
    let controller = async {
        let entered = retained_source_wait_cut(&control, Cut::BeforeSourceValidation).await;
        let binding = entered.binding.expect("actual held source binding");
        if scenario == "pty-held-stop" {
            let settings = app.state::<SettingsState>();
            settings
                .write()
                .await
                .agents
                .iter_mut()
                .find(|agent| agent.id == "codex")
                .expect("actual selected agent")
                .command
                .push_str(" --changed-held-recipe");
        }
        if scenario == "pty-cancel-held" {
            app.state::<crate::shutdown::ShutdownSignal>().trigger();
            drop(lose_receiver.take());
            loop {
                if control.restoration_failed.load(Ordering::Acquire) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(control.restoration_shared.load(Ordering::Acquire));
            resume_validation
                .send(())
                .expect("real worker still owns clone after source deadline cancellation");
        } else {
            resume_validation.send(()).unwrap();
        }
        retained_source_wait_cut(&control, Cut::AdapterOutcomeBeforeLeaseDrop).await;
        let observed = control
            .observe_owner(&coordinator)
            .expect("actual retained owner");
        if scenario == "pty-held-stop" {
            assert_eq!(
                observed.source_stop,
                Some(OwnedSourceStop::AuthorityChanged)
            );
        } else if scenario == "pty-held-rollback" {
            assert!(
                observed.rollback_completed,
                "actual setup rollback completed before bridge pause"
            );
        }
        if scenario != "pty-cancel-held" {
            lose_receiver
                .take()
                .unwrap()
                .send(())
                .expect("actual live helper receiver");
            loop {
                if control.restoration_failed.load(Ordering::Acquire) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(
                control.restoration_shared.load(Ordering::Acquire),
                "worker clone genuinely prevents raw restoration"
            );
        }
        assert!(!control.received.load(Ordering::Acquire));
        assert!(shared_engine_writes(app, binding.session_id()).is_empty());
        assert!(!control.observe_owner(&coordinator).unwrap().transferred);
        resume_bridge.send(()).unwrap();
        binding
    };
    let ((), binding) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(delivery, controller)
    })
    .await
    .expect("actual held worker/source schedule");
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("held-source close");
    assert!(
        !report.persistence_safe,
        "lost source receipt never certifies work completion"
    );
    let current = store
        .query_pty_input_by_injection(&injection)
        .unwrap()
        .unwrap();
    assert!(
        !current.terminal,
        "inert restoration loss does not fabricate terminal/handoff"
    );
    assert!(shared_engine_writes(app, binding.session_id()).is_empty());
    assert!(!control.observe_owner(&coordinator).unwrap().transferred);
}

pub(super) async fn retained_exited_respawn_submit_refused_cell(pty: bool, close_first: bool) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    let control = Arc::new(ReplacementFixtureControl::default());
    let (resume_source, gate) = tokio::sync::oneshot::channel();
    control.actions.lock().unwrap().push(Action::Pause {
        cut: Cut::SourceCoreEntered,
        resume: gate,
    });
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection =
        pty.then(|| enqueue_shared_pty_engine_operation(&store, &route, sender, b"refused-source"));
    let mut message = wake_message_to_target();
    message.id = format!("submit-refused-{}", Uuid::new_v4());
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
    let controller = async {
        let entered = retained_source_wait_cut(&control, Cut::SourceCoreEntered).await;
        let mut reservations = Vec::new();
        let closed = if close_first {
            let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
                .await
                .expect("real close before reserve");
            assert!(
                !report.persistence_safe,
                "actual admitted source remains incomplete"
            );
            Some(report)
        } else {
            None
        };
        if !close_first {
            loop {
                match coordinator.reserve_auto_close() {
                    Ok(ticket) => reservations.push(ticket),
                    Err(error) => {
                        assert_eq!(
                            error,
                            crate::session::selection::SelectionCoordinatorError::Busy
                        );
                        break;
                    }
                }
            }
        }
        assert!(
            close_first || !reservations.is_empty(),
            "genuine envelope/slot capacity was reserved"
        );
        resume_source.send(()).unwrap();
        retained_source_wait_cut(&control, Cut::SubmitFailed).await;
        assert!(!control.submitted.load(Ordering::Acquire));
        assert!(!control.received.load(Ordering::Acquire));
        let owner = control
            .observe_owner(&coordinator)
            .expect("same admitted source owner");
        assert!(!owner.transferred && !owner.terminal);
        let binding = entered.binding.unwrap();
        assert!(shared_engine_writes(app, binding.session_id()).is_empty());
        drop(reservations);
        closed
    };
    let ((), closed) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(delivery, controller)
    })
    .await
    .expect("actual submit-refused schedule");
    let report = match closed {
        Some(report) => report,
        None => tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
            .await
            .expect("refused source close"),
    };
    assert!(
        !report.persistence_safe,
        "submission refusal retains accepted source work"
    );
}

pub(super) async fn retained_exited_respawn_entry_restart_reclaim_cell() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, _, _, mut current) = prepare_retained_exited_respawn(&fixture).await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    for _ in 0..66 {
        let previous = current;
        let actual = tokio::time::timeout(
            Duration::from_secs(20),
            coordinator.restart_lifecycle(
                app.clone(),
                crate::commands::session::RestartJobRequest {
                    session_id: previous,
                    agent_id: None,
                    requested_profile: None,
                    skip_auto_resume: Some(true),
                    activate_after: false,
                    intent: crate::session::selection::TrustedRestartIntent::Background,
                    communication_override: None,
                    enforcement:
                        crate::config::sessions_persistence::default_creation_gate_enforcement(),
                },
            ),
        )
        .await
        .expect("actual same-history Restart entry deadline")
        .expect("actual legacy restart result");
        current = Uuid::parse_str(&actual.id).unwrap();
        assert_ne!(current, previous);
        assert!(manager.get_session(previous).await.is_none());
        assert!(manager.get_session(current).await.is_some());
        assert!(!pty.lock().unwrap().has_session(previous));
        assert!(pty.lock().unwrap().has_session(current));
        assert_eq!(
            coordinator.fixture_root_count(),
            Some(0),
            "actual Restart EntryTerminal/effects/lease release precedes next admission"
        );
    }
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual Restart-entry reclamation close");
    assert!(
        report.persistence_safe,
        "Restart has its own actual terminal result/effects"
    );
}

pub(super) async fn retained_exited_respawn_entry_root_reclaim_cell() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let request = crate::commands::session::RootJobRequest {
        requested_agent_id: Some("codex".into()),
        requested_profile: None,
        skip_auto_resume_for_new_session: true,
        intent: crate::session::selection::TrustedCreateIntent::Background,
        select_after: false,
    };
    let first = tokio::time::timeout(
        Duration::from_secs(20),
        coordinator.root_lifecycle(app.clone(), request.clone()),
    )
    .await
    .expect("actual initial Root entry deadline")
    .expect("actual Root entry creates its root directory/session");
    let mut id = Uuid::parse_str(&first.id).unwrap();
    assert!(manager.get_session(id).await.unwrap().is_root_agent);
    assert!(app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap()
        .has_session(id));
    assert_eq!(
        coordinator.fixture_root_count(),
        Some(0),
        "actual initial EntryTerminal removed"
    );
    for operation in 0..66 {
        id = retained_root_reclaim_operation(
            &fixture,
            &coordinator,
            &manager,
            &request,
            id,
            operation,
        )
        .await;
    }
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual Root-entry reclamation close");
    assert!(
        report.persistence_safe,
        "Root entries use their own verified terminal contract"
    );
}

pub(super) async fn retained_exited_respawn_root_full_cell(pty: bool, inline: bool) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    // Provision the whole immutable team authority before the sole restore.
    let members: Vec<_> = (0..66)
        .map(|index| provision_retained_fixture_member(&fixture, &format!("root-full-{index}")))
        .collect();
    let app = fixture.app.handle();
    let (coordinator, _, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    // Retire every real prerequisite before any retained work is admitted;
    // an unrelated later close must not compete with earlier Preparing.
    let mut retired = Vec::with_capacity(members.len());
    for (replica, target) in &members {
        let (route, previous) = tokio::time::timeout(
            Duration::from_secs(20),
            prepare_additional_retained_member(&fixture, &coordinator, sender, replica, target),
        )
        .await
        .expect("actual retired member prerequisite");
        retired.push((route, previous, target.clone()));
    }
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    for (index, (route, previous, target)) in retired.iter().enumerate() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let control = Arc::new(ReplacementFixtureControl::default());
            control.ticket_only.store(!inline, Ordering::Release);
            if index < 65 {
                control.actions.lock().unwrap().push(Action::PanicOnce {
                    cut: Cut::SourceCoreEntered,
                });
            }
            let hooks = MailboxTestHooks::default();
            *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
            let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
            let injection = pty.then(|| {
                enqueue_shared_pty_engine_operation(
                    &store,
                    route,
                    sender,
                    b"root-full-retained-source",
                )
            });
            let mut message = wake_message_to_target();
            message.id = format!("root-full-{index}-{}", Uuid::new_v4());
            message.to = target.clone();
            let actual_delivery = async {
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
                    let actual = poller.deliver_wake(app, &message).await;
                    if index == 65 {
                        let busy = crate::session::selection::SelectionCoordinatorError::Busy;
                        assert!(
                            actual.unwrap_err().contains(&busy.to_string()),
                            "actual full root maps to Busy"
                        );
                    }
                }
            };
            if index < 65 {
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
                tokio::select! {
                    () = actual_delivery => panic!("real accepted-core panic leaves its outer \
                        waiter ambiguous"),
                    () = stopped => {},
                }
                let receipt = retained_source_wait_cut(&control, Cut::SourceCoreEntered).await;
                let owner = control.observe_owner(&coordinator).unwrap();
                assert!(owner.source_stopped && !owner.terminal && !owner.transferred);
                assert_eq!(
                    owner.effects_started, 0,
                    "no finalization/native capacity consumed"
                );
                assert!(manager
                    .get_pending_session(receipt.binding.unwrap())
                    .await
                    .is_none());
                assert!(!control.submitted.load(Ordering::Acquire));
                assert!(
                    shared_engine_writes(app, receipt.binding.unwrap().session_id()).is_empty()
                );
                assert_eq!(coordinator.fixture_root_count(), Some(index + 1));
            } else {
                actual_delivery.await;
                assert_eq!(coordinator.fixture_root_count(), Some(65));
                assert!(
                    control.receipts.lock().unwrap().is_empty(),
                    "refused attempt was \
                        never accepted"
                );
            }
            assert!(manager.get_session(*previous).await.is_some());
            assert!(hooks.spawn_calls.lock().unwrap().is_empty());
            assert!(hooks.destroy_calls.lock().unwrap().is_empty());
            if let Some(injection) = injection {
                let actual = store
                    .query_pty_input_by_injection(&injection)
                    .unwrap()
                    .unwrap();
                assert_ne!(
                    actual.status,
                    crate::phone::types::PtyInputPublicStatus::Injected
                );
                assert!(
                    !actual.terminal,
                    "retained work cannot claim a terminal handoff"
                );
                if index == 65 {
                    assert_eq!(
                        actual.reason.map(|reason| reason.code),
                        Some(crate::phone::types::PtyInputReasonCode::StoreTransient)
                    );
                }
            }
        })
        .await
        .expect("same-history root member admission/refusal");
    }
    assert_eq!(coordinator.fixture_root_count(), Some(65));
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual full retained root close");
    assert!(!report.persistence_safe);
    assert_eq!(
        coordinator.fixture_root_count(),
        Some(65),
        "no fake completion/reset during seal"
    );
}
