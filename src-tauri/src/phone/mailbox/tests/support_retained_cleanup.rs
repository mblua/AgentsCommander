use super::*;

pub(super) async fn retained_exited_respawn_root_refusal_cell(
    pty: bool,
    inline: bool,
    poison: bool,
) {
    use crate::session::selection::ReplacementFixtureControl;
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    let control = Arc::new(ReplacementFixtureControl::default());
    control.ticket_only.store(!inline, Ordering::Release);
    let (resume, gate) = tokio::sync::oneshot::channel();
    *control.preaccept_gate.lock().unwrap() = Some(gate);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let mut before: Vec<_> = manager
        .list_sessions()
        .await
        .into_iter()
        .map(|row| row.id)
        .collect();
    before.sort();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection =
        pty.then(|| enqueue_shared_pty_engine_operation(&store, &route, sender, b"root-refused"));
    let mut message = wake_message_to_target();
    message.id = format!("root-refused-{}", Uuid::new_v4());
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
            let error = poller
                .deliver_wake(app, &message)
                .await
                .expect_err("actual unavailable source mapping");
            assert!(
                error.contains(
                    &crate::session::selection::SelectionCoordinatorError::Unavailable.to_string()
                ),
                "actual root refusal: {error}"
            );
        }
    };
    let controller = async {
        while !control.preaccept_entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        assert_eq!(coordinator.fixture_root_count(), Some(0));
        if poison {
            coordinator.fixture_poison_root();
        } else {
            let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
                .await
                .expect("actual root close-first");
            assert!(
                report.persistence_safe,
                "no admitted owner before actual seal"
            );
        }
        resume.send(()).unwrap();
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(delivery, controller);
    })
    .await
    .expect("actual producer refusal after root poison/seal");
    let mut after: Vec<_> = manager
        .list_sessions()
        .await
        .into_iter()
        .map(|row| row.id)
        .collect();
    after.sort();
    assert_eq!(after, before, "refusal added no row or pending B");
    assert!(manager.get_session(previous).await.is_some());
    assert!(
        control.receipts.lock().unwrap().is_empty(),
        "no accepted operation/effect receipt"
    );
    assert!(!control.submitted.load(Ordering::Acquire));
    assert!(!control.received.load(Ordering::Acquire));
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    if let Some(injection) = injection {
        let actual = store
            .query_pty_input_by_injection(&injection)
            .unwrap()
            .unwrap();
        assert_eq!(
            actual.status,
            crate::phone::types::PtyInputPublicStatus::Queued
        );
        assert!(!actual.terminal);
        assert_eq!(
            actual.reason.map(|reason| reason.code),
            Some(crate::phone::types::PtyInputReasonCode::StoreTransient)
        );
    }
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual root-refusal traversal");
    assert_eq!(
        report.persistence_safe, !poison,
        "poisoned root cannot be empty-safe"
    );
}

pub(super) async fn retained_exited_respawn_caller_cell(
    pty: bool,
    inline: bool,
    before_accept: bool,
) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    let control = Arc::new(ReplacementFixtureControl::default());
    control.ticket_only.store(!inline, Ordering::Release);
    let (resume, gate) = tokio::sync::oneshot::channel();
    let (resume_core, core_gate) = tokio::sync::oneshot::channel();
    if before_accept {
        *control.preaccept_gate.lock().unwrap() = Some(gate);
    } else {
        *control.baseline_gate.lock().unwrap() = Some(gate);
        if pty {
            *control.accepted_core_gate.lock().unwrap() = Some(core_gate);
        } else {
            control.actions.lock().unwrap().push(Action::Pause {
                cut: Cut::SourceCoreEntered,
                resume: core_gate,
            });
        }
    }
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection =
        pty.then(|| enqueue_shared_pty_engine_operation(&store, &route, sender, b"caller-source"));
    let mut message = wake_message_to_target();
    message.id = format!("caller-source-{}", Uuid::new_v4());
    let (cancel, canceled) = tokio::sync::oneshot::channel::<()>();
    let caller_dropped = AtomicBool::new(false);
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
    let caller = async {
        tokio::select! {
            biased;
            () = async { canceled.await.expect("controller canceled actual outer waiter"); } => {},
            () = delivery => panic!("fixture cancellation must precede source result"),
        }
        caller_dropped.store(true, Ordering::Release);
    };
    let controller = async {
        if before_accept {
            loop {
                if control.preaccept_entered.load(Ordering::Acquire) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(coordinator.fixture_root_count(), Some(0));
            cancel.send(()).unwrap();
            while !caller_dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
            // The canceled caller owns no accepted core. Its gate is
            // dropped as housekeeping, never resumed to manufacture one.
            drop(resume);
            drop(resume_core);
            assert!(!control.accepted_core_entered.load(Ordering::Acquire));
            assert!(control.receipts.lock().unwrap().is_empty());
            None
        } else {
            if pty {
                while !control.accepted_core_entered.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
                assert_eq!(
                    coordinator.fixture_root_count(),
                    Some(0),
                    "real PTY core acceptance precedes Replacement root insertion"
                );
            } else {
                retained_source_wait_cut(&control, Cut::SourceCoreEntered).await;
                assert_eq!(coordinator.fixture_root_count(), Some(1));
            }
            cancel.send(()).unwrap();
            while !caller_dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
            resume_core.send(()).unwrap();
            let baseline = retained_source_wait_cut(&control, Cut::Baseline).await;
            let id = baseline.binding.unwrap().session_id();
            assert_eq!(coordinator.fixture_root_count(), Some(1));
            let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
            idle.register_session(id, crate::session::profile::IdleTuning::DEFAULT);
            idle.set_pty_input_ready_for_test(id);
            hooks.real_inject_sessions.lock().unwrap().insert(id);
            resume.send(()).unwrap();
            loop {
                if coordinator.fixture_root_count() == Some(0) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(
                !shared_engine_writes(app, id).is_empty(),
                "accepted core performs actual source transfer after caller loss"
            );
            Some(id)
        }
    };
    let ((), new_id) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(caller, controller)
    })
    .await
    .expect("actual before/after acceptance caller cancellation");
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    if let Some(id) = new_id {
        assert!(manager.get_session(previous).await.is_none());
        assert!(manager.get_session(id).await.is_some());
    } else {
        assert!(manager.get_session(previous).await.is_some());
        assert!(control.receipts.lock().unwrap().is_empty());
    }
    if let Some(injection) = injection.as_deref() {
        let result = store
            .query_pty_input_by_injection(injection)
            .unwrap()
            .unwrap();
        if before_accept {
            assert_eq!(
                result.status,
                crate::phone::types::PtyInputPublicStatus::Queued
            );
        } else {
            assert_eq!(
                result.status,
                crate::phone::types::PtyInputPublicStatus::Injected
            );
        }
    }
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("caller-loss actual close");
    assert!(
        report.persistence_safe,
        "only observed complete/never-accepted work is safe: {:?}",
        report.retained
    );
}

pub(super) async fn retained_exited_respawn_reservation_refusal_cell() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let managers = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let coordinator =
        crate::session::selection::SelectionCoordinator::with_fixture_root_reservation(
            managers.inner().clone(),
            tokio_util::sync::CancellationToken::new(),
            crate::session::selection::FixtureRootReservation::RejectCapacity,
        );
    assert!(app.manage(coordinator.clone()));
    coordinator.start(app.clone()).unwrap();
    let error = coordinator
        .fixture_initial_reservation_refusal(crate::commands::session::RestartJobRequest {
            session_id: Uuid::new_v4(),
            agent_id: None,
            requested_profile: None,
            skip_auto_resume: None,
            activate_after: false,
            intent: crate::session::selection::TrustedRestartIntent::Background,
            communication_override: None,
            enforcement: crate::config::sessions_persistence::default_creation_gate_enforcement(),
        })
        .expect_err("actual capacity overflow makes actual root admission unavailable");
    assert_eq!(
        error,
        crate::session::selection::SelectionCoordinatorError::Unavailable.to_string()
    );
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    let manager = managers.read().await.clone();
    assert!(manager.list_sessions().await.is_empty());
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let pending_empty = manager
        .with_current_carrier_snapshot(&permit, |current| current.pending_ids.is_empty())
        .await;
    drop(permit);
    assert!(pending_empty);
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual unavailable-root close");
    assert!(!report.persistence_safe);
    assert!(report
        .retained
        .iter()
        .any(|reason| reason.contains("retained-root-unavailable")));
}

pub(super) async fn retained_exited_respawn_receive_after_release_cell(pty: bool) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    let control = Arc::new(ReplacementFixtureControl::default());
    let (resume_send, send_gate) = tokio::sync::oneshot::channel();
    let (drop_receiver, receiver_signal) = tokio::sync::oneshot::channel();
    control.actions.lock().unwrap().push(Action::Pause {
        cut: Cut::AfterReleaseBeforeSend,
        resume: send_gate,
    });
    *control.live_receiver_drop.lock().unwrap() = Some(receiver_signal);
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection = pty.then(|| {
        enqueue_shared_pty_engine_operation(&store, &route, sender, b"after-release-source")
    });
    let mut message = wake_message_to_target();
    message.id = format!("after-release-source-{}", Uuid::new_v4());
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
        let released = retained_source_wait_cut(&control, Cut::AfterReleaseBeforeSend).await;
        let (published, inner_released, work_started, transferred, held) =
            control.observe_retained(&coordinator).unwrap();
        assert!(published && inner_released);
        assert_eq!(held, 0, "actual initial envelope admission released");
        assert!(!work_started && !transferred);
        assert!(!control.observe_owner(&coordinator).unwrap().terminal);
        assert!(!control.received.load(Ordering::Acquire));
        drop_receiver.send(()).unwrap();
        retained_source_wait_cut(&control, Cut::ReceiverLost).await;
        assert!(shared_engine_writes(app, released.binding.unwrap().session_id()).is_empty());
        resume_send.send(()).unwrap();
        coordinator
            .snapshot()
            .await
            .expect("actual worker progressed past failed response send");
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(delivery, controller);
    })
    .await
    .expect("actual release-before-send live receiver loss");
    {
        let receipts = control.receipts.lock().unwrap();
        assert_eq!(
            receipts
                .iter()
                .filter(|receipt| receipt.cut == Cut::SourceReceived)
                .count(),
            0
        );
        assert_eq!(
            receipts
                .iter()
                .filter(|receipt| receipt.cut == Cut::ReceiverLost)
                .count(),
            2,
            "actual helper receive loss and actual worker send error are distinct"
        );
    }
    assert!(!control.observe_owner(&coordinator).unwrap().transferred);
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual lost-receiver close");
    assert!(!report.persistence_safe);
}

pub(super) struct RetainedCleanupRaceGates {
    pub(super) first: tokio::sync::oneshot::Sender<()>,
    pub(super) finalizer: tokio::sync::oneshot::Sender<()>,
    pub(super) disarmed: tokio::sync::oneshot::Sender<()>,
    pub(super) panic_source: tokio::sync::oneshot::Sender<()>,
    pub(super) claim_first: bool,
    pub(super) runtime_changed: bool,
    pub(super) unknown: bool,
}

pub(super) async fn retained_source_unknown_cleanup(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &Arc<crate::session::selection::ReplacementFixtureControl>,
    gates: RetainedCleanupRaceGates,
    inline: bool,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let mut first = Some(gates.first);
    if inline {
        first.take().unwrap().send(()).unwrap();
        retained_source_wait_cut(control, Cut::BeforeFinalizer).await;
    }
    let binding = control
        .receipts
        .lock()
        .unwrap()
        .first()
        .unwrap()
        .binding
        .unwrap();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let backend = pty
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .unwrap();
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let owner = control.owner_weak(coordinator).unwrap();
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(control.observe_owner(coordinator).unwrap().source_active);
    control.poison_observed_owner(coordinator);
    assert!(
        control.observe_owner(coordinator).is_none(),
        "actual proof is unreadable, never assigned Unknown"
    );
    assert!(
        !control
            .cleanup_observed_stopped(coordinator, &manager, &pty)
            .await
    );
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    drop(gates.panic_source); // Closed signal is disabled; source stays real/live.
    drop(gates.disarmed);
    if let Some(first) = first {
        first.send(()).unwrap();
    }
    if inline {
        gates.finalizer.send(()).unwrap();
    }
    coordinator
        .snapshot()
        .await
        .expect("real finalizer conservatively retained poisoned owner");
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(pty.lock().unwrap().has_session(binding.session_id()));
    assert!(shared_engine_writes(app, binding.session_id()).is_empty());
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual unreadable owner close");
    assert!(!report.persistence_safe);
    assert!(
        owner.is_alive(),
        "actual incomplete unreadable owner remains rooted"
    );
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
}

pub(super) fn release_retained_cleanup_worker(
    first: &mut Option<tokio::sync::oneshot::Sender<()>>,
    finalizer: &mut Option<tokio::sync::oneshot::Sender<()>>,
    inline: bool,
) {
    if let Some(first) = first.take() {
        first.send(()).unwrap();
    }
    if inline {
        finalizer.take().unwrap().send(()).unwrap();
    }
}

pub(super) async fn prepare_retained_cleanup_observation(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
    panic_source: tokio::sync::oneshot::Sender<()>,
    runtime_changed: bool,
) {
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(control.observe_owner(coordinator).unwrap().source_active);
    assert!(
        !control
            .cleanup_observed_stopped(coordinator, manager, pty)
            .await,
        "a real active source is not stopped-source cleanup evidence"
    );
    panic_source.send(()).unwrap();
    loop {
        if control
            .observe_owner(coordinator)
            .is_some_and(|owner| owner.source_stopped)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    if runtime_changed {
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        {
            let actual = pty.lock().unwrap();
            let (cwd, replica, generation) = actual.route_identities(binding.session_id()).unwrap();
            assert!(actual.has_session(binding.session_id()));
            actual.remove_route_if_kind(binding.session_id(), SessionBackendKind::LocalProcess);
            actual
                .record_route_with_identities(
                    binding.session_id(),
                    SessionBackendKind::LocalProcess,
                    cwd,
                    replica,
                )
                .unwrap();
            assert_ne!(
                actual.route_identities(binding.session_id()).unwrap().2,
                generation,
                "actual registry minted a later route generation"
            );
        }
        drop(permit);
    }
}

pub(super) async fn retained_source_cleanup_race(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &Arc<crate::session::selection::ReplacementFixtureControl>,
    gates: RetainedCleanupRaceGates,
    inline: bool,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    use std::future::Future;
    if gates.unknown {
        retained_source_unknown_cleanup(app, coordinator, control, gates, inline).await;
        return;
    }
    let RetainedCleanupRaceGates {
        first,
        finalizer,
        disarmed,
        panic_source,
        claim_first,
        runtime_changed,
        unknown: _,
    } = gates;
    let mut first = Some(first);
    let mut finalizer = Some(finalizer);
    if inline {
        first.take().unwrap().send(()).unwrap();
        retained_source_wait_cut(control, Cut::BeforeFinalizer).await;
    }
    // Ticket: earlier real worker still blocked, real B/native effects done.
    // Inline: same worker now holds B before its finalizer U.
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let backend = pty
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("actual cleanup race mock backend");
    let kills_before = backend.kill_attempts.load(Ordering::SeqCst);
    let binding = control
        .receipts
        .lock()
        .unwrap()
        .first()
        .unwrap()
        .binding
        .unwrap();
    prepare_retained_cleanup_observation(
        control,
        coordinator,
        &manager,
        &pty,
        binding,
        panic_source,
        runtime_changed,
    )
    .await;
    let cleanup = control.cleanup_observed_stopped(coordinator, &manager, &pty);
    if runtime_changed {
        assert!(
            !cleanup.await,
            "later actual runtime route vetoes stopped-source claim"
        );
        assert_retained_cleanup_runtime_veto(control, coordinator, &manager, &pty, binding).await;
        release_retained_cleanup_worker(&mut first, &mut finalizer, inline);
        drop(disarmed);
    } else if claim_first {
        assert!(cleanup.await, "actual stopped-source claim and rollback");
        assert_retained_cleanup_claimed(control, coordinator, &manager, &pty, binding).await;
        release_retained_cleanup_worker(&mut first, &mut finalizer, inline);
        drop(disarmed);
    } else {
        release_retained_cleanup_worker(&mut first, &mut finalizer, inline);
        retained_source_wait_cut(control, Cut::AfterDisarm).await;
        tokio::pin!(cleanup);
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                cleanup.as_mut().poll(cx).is_pending()
            ))
            .await,
            "same cleanup future waits for actual finalization U"
        );
        disarmed.send(()).unwrap();
        assert!(!cleanup.await, "real finalized/disarmed B vetoes cleanup");
        assert_retained_cleanup_finalized(control, coordinator, &manager, &pty, binding).await;
    }
    coordinator
        .snapshot()
        .await
        .expect("same real worker completed after race");
    assert_retained_cleanup_common(
        app,
        coordinator,
        control,
        &manager,
        &pty,
        binding,
        kills_before,
        claim_first,
        backend,
    )
    .await;
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual cleanup race close");
    assert_retained_cleanup_close(&report, backend, kills_before, claim_first);
}
