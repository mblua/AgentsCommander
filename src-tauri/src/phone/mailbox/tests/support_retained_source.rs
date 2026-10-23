use super::*;

#[expect(
    clippy::too_many_arguments,
    reason = "P05 reuses one operation within the same fixture and whole66 history using the \
        unchanged outer scenario."
)]
pub(super) async fn retained_source_operation(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    route: &crate::config::teams::VerifiedPtyInputRoute,
    sender: uuid::Uuid,
    previous_slot: &mut uuid::Uuid,
    pty_source: bool,
    inline: bool,
    outer_scenario: &str,
    operation: usize,
) -> bool {
    use crate::session::selection::{ReplacementFixtureControl, ReplacementFixtureCut as Cut};
    use std::future::Future;
    let app = fixture.app.handle();
    let previous = *previous_slot;
    let scenario = outer_scenario;
    let reclaim = scenario == "reclaim-66";
    let check_reacquire = pty_source && scenario == "positive";
    let newer_b = scenario.starts_with("newer-") || scenario.starts_with("under-commit-");
    let walk_terminal = scenario == "walk-terminal";
    let shutdown_executing = scenario == "shutdown-executing";
    let seal_insert_first = scenario == "seal-insert-first";
    let seed_scenario = scenario.starts_with("seed-");
    let full = scenario.ends_with("full");
    let under_commit = scenario.starts_with("under-commit");
    let scenario = if reclaim { "positive" } else { scenario };
    let seed_source = fixture
        .target_cwd
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("default.p05-seed");
    if seed_scenario {
        configure_retained_seed_fixture(fixture, &seed_source).await;
    }
    let control = Arc::new(ReplacementFixtureControl::default());
    let (resume_terminal, terminal_gate) = tokio::sync::oneshot::channel();
    if walk_terminal {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::BeforeTerminalRemoval,
                resume: terminal_gate,
            },
        );
    }
    let (resume_tail, tail_gate) = tokio::sync::oneshot::channel();
    if shutdown_executing {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::BeforeCarry,
                resume: tail_gate,
            },
        );
    }
    {
        let pty = app.state::<Arc<Mutex<PtyManager>>>();
        let backend = pty
            .lock()
            .unwrap()
            .backend_for_kind(SessionBackendKind::LocalProcess);
        assert!(backend.as_any().is::<MailboxMockPtyBackend>());
    }
    control.mock_backend.store(true, Ordering::Release);
    control.ticket_only.store(!inline, Ordering::Release);
    let (resume_insertion, insertion_gate) = tokio::sync::oneshot::channel();
    if scenario == "root-duplicate" || seal_insert_first {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::SourceCoreEntered,
                resume: insertion_gate,
            },
        );
    }
    let (resume_native, native_gate) = tokio::sync::oneshot::channel();
    if seed_scenario {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::SeedNativeEnter,
                resume: native_gate,
            },
        );
    }
    let (resume_baseline, baseline_gate) = tokio::sync::oneshot::channel();
    *control.baseline_gate.lock().unwrap() = Some(baseline_gate);
    let (resume_commit, commit_gate) = tokio::sync::oneshot::channel();
    if under_commit {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::AfterDisarm,
                resume: commit_gate,
            },
        );
    }
    let (resume_release, release_gate) = tokio::sync::oneshot::channel();
    let (resume_publication, publication_gate) = tokio::sync::oneshot::channel();
    if newer_b {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::BeforePublication,
                resume: publication_gate,
            },
        );
    }
    if inline {
        *control.before_release_gate.lock().unwrap() = Some(release_gate);
    }
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection = pty_source.then(|| {
        enqueue_shared_pty_engine_operation(&store, route, sender, b"retained-source-body")
    });
    let mut message = wake_message_to_target();
    message.id = format!("retained-source-{}", Uuid::new_v4());
    let (wait_observed, wait_gate) = tokio::sync::oneshot::channel();
    let (delivery_finished, delivery_gate) = tokio::sync::oneshot::channel();
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
            let result = retained_fixture_ordinary_wake(&poller, app, &message, &control).await;
            if seal_insert_first {
                if inline {
                    assert!(
                        result.is_none(),
                        "actual closed inline reserve returns retained"
                    );
                } else {
                    assert!(result
                        .expect("actual ticket setup result")
                        .unwrap_err()
                        .contains(
                            &crate::session::selection::SelectionCoordinatorError::Unavailable
                                .to_string()
                        ));
                }
            } else if !newer_b {
                result
                    .expect("actual ordinary result")
                    .expect("actual ordinary source tail");
            }
        }
        let _ = delivery_finished.send(());
    };
    let readiness_observer = async {
        if !check_reacquire {
            return;
        }
        let baseline = retained_source_wait_cut(&control, Cut::Baseline).await;
        let id = baseline.binding.unwrap().session_id();
        assert!(
            !pty_spawn_scope_may_resume(app, &route.target).await,
            "real Preparing B is not strict retired A"
        );
        let waiting_id =
            enqueue_shared_pty_engine_operation(&store, route, sender, b"waiting-preboundary");
        let initial = store
            .query_pty_input_by_injection(&waiting_id)
            .unwrap()
            .unwrap();
        let claimed = store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::HostCli,
                Some(&waiting_id),
                &Uuid::new_v4().to_string(),
                chrono::Utc::now(),
            )
            .unwrap()
            .unwrap();
        let expires =
            crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at).unwrap();
        let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
            store.clone(),
            waiting_id.clone(),
            claimed.lease_owner.clone(),
            expires,
        );
        let gate = state.target_gate.as_ref().unwrap().clone();
        let shutdown = tokio_util::sync::CancellationToken::new();
        {
            let waiting = reacquire_pty_dispatch_target(
                app,
                (&store, &initial, &claimed),
                (&gate, &route.target, PtyReacquireGoal::Spawn),
                (expires, &shutdown),
                &mut heartbeat,
            );
            tokio::pin!(waiting);
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    waiting.as_mut().poll(cx).is_pending()
                ))
                .await
            );
            wait_observed.send(()).unwrap();
            tokio::select! {
                biased;
                _ = &mut waiting => panic!("not-ready operation reacquired before real state \
                    change"),
                done = delivery_gate => done.expect("whole first source returned"),
            }
            // The first source has released its guards; the suspended
            // operation must still own none, including exact/U/M.
            let stripe = gate
                .try_target_lock(&route.target.canonical_fqn)
                .unwrap()
                .unwrap();
            let exact = gate.acquire_exact(&route.target.canonical_fqn).await;
            drop(exact);
            drop(stripe);
            assert!(!pty_spawn_scope_may_resume(app, &route.target).await);
            retire_retained_fixture_member(fixture, coordinator, route, sender, id).await;
            let (stripe, exact) = waiting
                .await
                .expect("real retirement resumes the SAME claim");
            let ownership = gate
                .target_ownership(&route.target.canonical_fqn, &stripe, &exact)
                .unwrap();
            let mut source = crate::session::selection::SourceExecutorGuard::unbound();
            let actual = poller
                .spawn_pty_input_target(
                    app,
                    &claimed,
                    &[],
                    &route.sender,
                    &route.target,
                    crate::phone::types::PtyInputSourcePlane::HostCli,
                    None,
                    &ownership,
                    &initial,
                    None,
                    &mut source,
                    None,
                )
                .await;
            assert!(matches!(
                actual,
                Err(crate::phone::types::PtyInputReasonCode::InconsistentSession)
            ));
            source.returned(false);
            drop(exact);
            drop(stripe);
        }
        assert!(
            finish_pty_input_before_boundary(
                &store,
                &mut heartbeat,
                &waiting_id,
                &claimed.lease_owner,
                crate::phone::types::PtyInputReasonCode::InconsistentSession
            )
            .await
        );
        let actual = store
            .query_pty_input_by_injection(&waiting_id)
            .unwrap()
            .unwrap();
        assert!(actual.terminal);
        assert_eq!(
            actual.reason.unwrap().code,
            crate::phone::types::PtyInputReasonCode::InconsistentSession
        );
        let connection =
            rusqlite::Connection::open(fixture._temp.path().join("retained-respawn.sqlite3"))
                .unwrap();
        let (attempt, retries): (i64, i64) = connection
            .query_row(
                "SELECT attempt,(SELECT count(*) FROM pty_input_audit WHERE injection_id=?1 \
                    AND status='retry') FROM pty_input_operations WHERE injection_id=?1",
                [&waiting_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((attempt, retries), (claimed.attempt, 0));
    };
    let controller = async {
        if scenario == "root-duplicate" || seal_insert_first {
            let entered = retained_source_wait_cut(&control, Cut::SourceCoreEntered).await;
            let weak = control.owner_weak(coordinator).unwrap();
            assert_eq!(coordinator.fixture_root_count(), Some(1));
            if seal_insert_first {
                assert_eq!(
                    control.observe_owner(coordinator).unwrap().effects_started,
                    0
                );
                let report =
                    tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
                        .await
                        .expect("real insertion precedes actual seal");
                assert!(
                    !report.persistence_safe,
                    "real admitted work is incomplete at seal"
                );
                resume_insertion.send(()).unwrap();
                return (entered.binding.unwrap().session_id(), None, weak);
            }
            assert_eq!(
                control.attempt_duplicate(coordinator).unwrap_err(),
                crate::session::selection::SelectionCoordinatorError::Busy.to_string()
            );
            assert_eq!(coordinator.fixture_root_count(), Some(1));
            assert!(weak.is_alive());
            assert_eq!(
                control.observe_owner(coordinator).unwrap().effects_started,
                0
            );
            assert!(manager
                .get_pending_session(entered.binding.unwrap())
                .await
                .is_none());
            resume_insertion.send(()).unwrap();
        } else {
            drop(resume_insertion);
        }
        if seed_scenario {
            observe_retained_seed_fixture(
                fixture,
                coordinator,
                &control,
                &manager,
                resume_native,
                scenario,
                &seed_source,
            )
            .await;
        } else {
            drop(resume_native);
        }
        let mut expected_communication = None;
        if under_commit {
            expected_communication =
                observe_retained_commit_setter(&control, &manager, resume_commit, full).await;
        } else {
            drop(resume_commit);
        }
        let baseline = retained_source_wait_cut(&control, Cut::Baseline).await;
        let id = baseline.binding.unwrap().session_id();
        let owner_weak = control
            .owner_weak(coordinator)
            .expect("actual baseline owner Weak");
        assert_ne!(id, previous);
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let version = manager
            .with_current_carrier_snapshot(&permit, |current| current.versions[&id])
            .await;
        drop(permit);
        if !under_commit {
            assert_eq!(baseline.initial_version, Some(version));
        }
        let setter_version = if newer_b {
            if !under_commit {
                expected_communication =
                    set_retained_fixture_communication(&manager, id, full).await;
            }
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let changed = manager
                .with_current_carrier_snapshot(&permit, |current| current.versions[&id])
                .await;
            drop(permit);
            assert_ne!(Some(changed), baseline.initial_version);
            Some(changed)
        } else {
            None
        };
        // Existing detector fixture stimulus; no Ready result is forged.
        let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
        idle.register_session(id, crate::session::profile::IdleTuning::DEFAULT);
        idle.set_pty_input_ready_for_test(id);
        hooks.real_inject_sessions.lock().unwrap().insert(id);
        if check_reacquire {
            wait_gate.await.expect("real not-ready observation");
        }
        resume_baseline.send(()).unwrap();
        if let Some(expected_version) = setter_version {
            retained_source_wait_cut(&control, Cut::BeforePublication).await;
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let actual_version = manager
                .with_current_carrier_snapshot(&permit, |current| current.versions[&id])
                .await;
            drop(permit);
            assert_retained_source_reconciliation(
                &control,
                coordinator,
                actual_version,
                expected_version,
                baseline.initial_version,
            );
            resume_publication.send(()).unwrap();
        } else {
            drop(resume_publication);
        }
        if inline {
            retained_source_wait_cut(&control, Cut::AdapterReturned).await;
            assert_retained_source_inline_boundary(&control, coordinator, app, id, newer_b);
            if shutdown_executing {
                let close =
                    tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join());
                tokio::pin!(close);
                assert!(
                    std::future::poll_fn(|cx| std::task::Poll::Ready(
                        close.as_mut().poll(cx).is_pending()
                    ))
                    .await,
                    "actual worker still executes its admitted inline envelope"
                );
                resume_release.send(()).unwrap();
                retained_source_wait_cut(&control, Cut::BeforeCarry).await;
                let report = close.await.expect("actual executing-inline shutdown/join");
                assert!(
                    !report.persistence_safe,
                    "actual source carry is still Running"
                );
                assert!(control.received.load(Ordering::Acquire));
                assert!(!control.observe_owner(coordinator).unwrap().transferred);
            } else {
                resume_release.send(()).unwrap();
            }
        } else {
            drop(resume_release);
        }
        if shutdown_executing {
            resume_tail.send(()).unwrap();
        } else {
            drop(resume_tail);
        }
        if walk_terminal {
            retained_source_wait_cut(&control, Cut::BeforeTerminalRemoval).await;
            let actual = control.observe_owner(coordinator).unwrap();
            assert!(
                actual.terminal && actual.transferred,
                "actual strict terminal proof precedes observer, outside U"
            );
            assert_eq!(coordinator.fixture_root_count(), Some(1));
            let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
                .await
                .expect("actual terminal entry seal/traversal");
            assert!(
                report.persistence_safe,
                "sealed traversal sees genuinely verified terminal owner"
            );
            assert!(owner_weak.is_alive());
            resume_terminal.send(()).unwrap();
        } else {
            drop(resume_terminal);
        }
        (id, expected_communication, owner_weak)
    };
    let ((), (id, expected_communication, owner_weak), ()) =
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(delivery, controller, readiness_observer)
        })
        .await
        .expect("source and controller both progress");
    if seal_insert_first {
        assert_retained_source_sealed(
            fixture,
            coordinator,
            &control,
            &manager,
            previous,
            id,
            injection,
            &store,
            &owner_weak,
        )
        .await;
        return true;
    }
    if inline {
        assert_retained_source_receipt_order(&control);
    }
    if newer_b {
        assert_retained_source_newer_result(
            fixture,
            coordinator,
            &control,
            &manager,
            previous,
            id,
            full,
            expected_communication,
        )
        .await;
    } else {
        assert_retained_source_positive(fixture, &manager, previous, id, injection, &store, &hooks)
            .await;
    }
    if reclaim {
        coordinator
            .snapshot()
            .await
            .expect("real worker barrier after actual source transfer");
        assert_eq!(
            coordinator.fixture_root_count(),
            Some(0),
            "actual terminal removal reclaims capacity"
        );
        if operation + 1 < 66 {
            retire_retained_fixture_member(fixture, coordinator, route, sender, id).await;
            *previous_slot = id;
        }
        return false;
    }
    let report = coordinator.close_and_join().await;
    assert_retained_source_lifetime(
        coordinator,
        &report,
        &owner_weak,
        full,
        shutdown_executing,
        walk_terminal,
    );
    false
}

pub(super) async fn retained_exited_respawn_source_cell(
    pty_source: bool,
    inline: bool,
    scenario: &str,
) {
    let reclaim = scenario == "reclaim-66";
    let fixture = make_mailbox_fixture();

    let (coordinator, route, sender, mut previous) =
        prepare_retained_exited_respawn(&fixture).await;
    if pty_source && scenario == "positive" {
        assert_retained_pty_preboundary_rejections(
            &fixture,
            &coordinator,
            &route,
            sender,
            previous,
        )
        .await;
    }
    for operation in 0..if reclaim { 66 } else { 1 } {
        if retained_source_operation(
            &fixture,
            &coordinator,
            &route,
            sender,
            &mut previous,
            pty_source,
            inline,
            scenario,
            operation,
        )
        .await
        {
            return;
        }
    }
    if reclaim {
        let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
            .await
            .expect("one final close after genuine whole66 history");
        assert!(
            report.persistence_safe,
            "whole66 actually completed: {:?}",
            report.retained
        );
    }
}

pub(super) async fn configure_retained_context_fixture(fixture: &MailboxFixture) {
    let app = fixture.app.handle();
    let origin = fixture
        .target_cwd
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("_agent_dev-rust");
    std::fs::write(
        origin.join("Role.md"),
        "# Fixture role\nActual native context content.\n",
    )
    .unwrap();
    let mut settings = app.state::<SettingsState>().read().await.clone();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .instructions_filename = Some("AGENTS.md".into());
    crate::config::settings::validate_and_repair_settings(&mut settings).unwrap();
    let written = crate::config::settings::save_settings(&settings).unwrap();
    *app.state::<SettingsState>().write().await = written;
}

pub(super) async fn assert_retained_native_paused(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty_manager: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
    backend: &MailboxMockPtyBackend,
    kills: usize,
) {
    assert!(
        !control
            .cleanup_observed_stopped(coordinator, manager, pty_manager)
            .await,
        "Started native vetoes stopped-source cleanup"
    );
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(
        !control
            .observe_owner(coordinator)
            .unwrap()
            .rollback_completed
    );
    assert!(!control.observe_owner(coordinator).unwrap().transferred);
    // The operation deadline above has ended. This is the real product five-second close.
    let report = coordinator.close_and_join().await;
    assert!(!report.persistence_safe);
    assert!(report
        .retained
        .iter()
        .any(|reason| reason == "reason=blocking-seed-transaction-await state=retained"));
    assert_eq!(coordinator.fixture_native_work_counts(), Some((1, 1)));
}

pub(super) async fn retained_exited_respawn_native_waiter_cell(pty: bool, seed: bool) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, previous) = prepare_retained_exited_respawn(&fixture).await;
    configure_retained_context_fixture(&fixture).await;
    let seed_source = fixture
        .target_cwd
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("default.p05-seed");
    if seed {
        configure_retained_seed_fixture(&fixture, &seed_source).await;
    }
    let control = Arc::new(ReplacementFixtureControl::default());
    control.ticket_only.store(true, Ordering::Release);
    control.mock_backend.store(true, Ordering::Release);
    let entered = if seed {
        Cut::SeedNativeEnter
    } else {
        Cut::ContextNativeEnter
    };
    let returned = if seed {
        Cut::SeedNativeReturned
    } else {
        Cut::ContextNativeReturned
    };
    let (resume_native, native_gate) = tokio::sync::oneshot::channel();
    control.actions.lock().unwrap().push(Action::Pause {
        cut: entered,
        resume: native_gate,
    });
    let (panic_source, panic_gate) = tokio::sync::oneshot::channel();
    *control.native_await_panic.lock().unwrap() = Some(if seed {
        crate::session::selection::NativeAwaitPanicOnce::Seed(panic_gate)
    } else {
        crate::session::selection::NativeAwaitPanicOnce::Context(panic_gate)
    });
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks);
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let backend = pty_manager
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .expect("actual mock backend");
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection = pty.then(|| {
        enqueue_shared_pty_engine_operation(&store, &route, sender, b"native-waiter-source")
    });
    let mut message = wake_message_to_target();
    message.id = format!("native-waiter-{}", Uuid::new_v4());
    let binding = {
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
                let _ = poller.deliver_wake(app, &message).await;
            }
        };
        tokio::pin!(delivery);
        let controller = observe_retained_native_source_unwind(
            seed,
            &fixture,
            &coordinator,
            &control,
            entered,
            panic_source,
        );
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::select! {
                biased;
                () = &mut delivery => panic!("source waiter unexpectedly returned before \
                    actual source unwind"),
                binding = controller => binding,
            }
        })
        .await
        .expect("actual native enter and accepted-source unwind")
        // Only now does the scope drop the outer result-cell waiter as housekeeping.
    };
    assert_retained_native_paused(
        &control,
        &coordinator,
        &manager,
        &pty_manager,
        binding,
        backend,
        kills,
    )
    .await;
    resume_native.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        retained_source_wait_cut(&control, returned).await;
        while coordinator.fixture_native_work_counts() != Some((0, 0)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("same native closure returns normally and actual tracker drains");
    let actual = control
        .native_results
        .lock()
        .unwrap()
        .iter()
        .find(|(cut, _)| *cut == returned)
        .unwrap()
        .1
        .clone();
    assert_retained_native_result(&fixture, seed, &actual);
    let after = coordinator.close_and_join().await;
    assert!(!after
        .retained
        .iter()
        .any(|reason| reason == "reason=blocking-seed-transaction-await state=retained"));
    assert!(manager.get_session(previous).await.is_some());
    assert!(shared_engine_writes(app, binding.session_id()).is_empty());
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
    if let Some(injection) = injection {
        assert!(
            !store
                .query_pty_input_by_injection(&injection)
                .unwrap()
                .unwrap()
                .terminal
        );
    }
}
