use super::*;

mod readiness;
use readiness::retained_readiness_observer;

#[derive(Clone, Copy)]
struct RetainedSourceFlags {
    reclaim: bool,
    check_reacquire: bool,
    newer_b: bool,
    walk_terminal: bool,
    shutdown_executing: bool,
    seal_insert_first: bool,
    seed_scenario: bool,
    full: bool,
    under_commit: bool,
    inline: bool,
}

struct RetainedSourceResumes {
    terminal: tokio::sync::oneshot::Sender<()>,
    tail: tokio::sync::oneshot::Sender<()>,
    insertion: tokio::sync::oneshot::Sender<()>,
    native: tokio::sync::oneshot::Sender<()>,
    baseline: tokio::sync::oneshot::Sender<()>,
    commit: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Sender<()>,
    publication: tokio::sync::oneshot::Sender<()>,
}

struct RetainedSourceCtx<'a> {
    fixture: &'a MailboxFixture,
    app: &'a tauri::AppHandle<tauri::test::MockRuntime>,
    coordinator: &'a crate::session::selection::SelectionCoordinator,
    route: &'a crate::config::teams::VerifiedPtyInputRoute,
    sender: uuid::Uuid,
    previous: uuid::Uuid,
    scenario: &'a str,
    flags: RetainedSourceFlags,
    control: &'a Arc<crate::session::selection::ReplacementFixtureControl>,
    hooks: &'a MailboxTestHooks,
    poller: &'a MailboxPoller,
    manager: &'a SessionManager,
    state: &'a crate::api::message_store::MessageStoreState,
    store: &'a Arc<crate::api::message_store::MessageStore>,
    seed_source: &'a Path,
}

type RetainedSourceOutcome = (
    uuid::Uuid,
    Option<crate::session::session::SessionCommunication>,
    crate::session::selection::ReplacementFixtureOwnerWeak,
);

fn retained_source_flags(
    scenario: &str,
    pty_source: bool,
    inline: bool,
) -> (RetainedSourceFlags, &str) {
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
    let flags = RetainedSourceFlags {
        reclaim,
        check_reacquire,
        newer_b,
        walk_terminal,
        shutdown_executing,
        seal_insert_first,
        seed_scenario,
        full,
        under_commit,
        inline,
    };
    (flags, scenario)
}

fn retained_source_control(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    flags: RetainedSourceFlags,
    scenario: &str,
) -> (
    Arc<crate::session::selection::ReplacementFixtureControl>,
    RetainedSourceResumes,
) {
    use crate::session::selection::{ReplacementFixtureControl, ReplacementFixtureCut as Cut};
    let control = Arc::new(ReplacementFixtureControl::default());
    let (resume_terminal, terminal_gate) = tokio::sync::oneshot::channel();
    if flags.walk_terminal {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::BeforeTerminalRemoval,
                resume: terminal_gate,
            },
        );
    }
    let (resume_tail, tail_gate) = tokio::sync::oneshot::channel();
    if flags.shutdown_executing {
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
    control.ticket_only.store(!flags.inline, Ordering::Release);
    let (resume_insertion, insertion_gate) = tokio::sync::oneshot::channel();
    if scenario == "root-duplicate" || flags.seal_insert_first {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::SourceCoreEntered,
                resume: insertion_gate,
            },
        );
    }
    let (resume_native, native_gate) = tokio::sync::oneshot::channel();
    if flags.seed_scenario {
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
    if flags.under_commit {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::AfterDisarm,
                resume: commit_gate,
            },
        );
    }
    let (resume_release, release_gate) = tokio::sync::oneshot::channel();
    let (resume_publication, publication_gate) = tokio::sync::oneshot::channel();
    if flags.newer_b {
        control.actions.lock().unwrap().push(
            crate::session::selection::ReplacementFixtureAction::Pause {
                cut: Cut::BeforePublication,
                resume: publication_gate,
            },
        );
    }
    if flags.inline {
        *control.before_release_gate.lock().unwrap() = Some(release_gate);
    }
    let resumes = RetainedSourceResumes {
        terminal: resume_terminal,
        tail: resume_tail,
        insertion: resume_insertion,
        native: resume_native,
        baseline: resume_baseline,
        commit: resume_commit,
        release: resume_release,
        publication: resume_publication,
    };
    (control, resumes)
}

async fn retained_source_delivery(
    ctx: &RetainedSourceCtx<'_>,
    injection: Option<&str>,
    message: &OutboxMessage,
    delivery_finished: tokio::sync::oneshot::Sender<()>,
) {
    let (app, poller, control) = (ctx.app, ctx.poller, ctx.control);
    let (seal_insert_first, inline) = (ctx.flags.seal_insert_first, ctx.flags.inline);
    if let Some(injection) = injection {
        poller
            .dispatch_pty_input_operation(
                app,
                ctx.state,
                injection,
                crate::phone::types::PtyInputSourcePlane::HostCli,
                None,
            )
            .await;
    } else {
        let result = retained_fixture_ordinary_wake(poller, app, message, control).await;
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
        } else if !ctx.flags.newer_b {
            result
                .expect("actual ordinary result")
                .expect("actual ordinary source tail");
        }
    }
    let _ = delivery_finished.send(());
}

async fn retained_source_root_prelude(
    ctx: &RetainedSourceCtx<'_>,
    resume_insertion: tokio::sync::oneshot::Sender<()>,
) -> Option<RetainedSourceOutcome> {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let (control, coordinator) = (ctx.control, ctx.coordinator);
    let seal_insert_first = ctx.flags.seal_insert_first;
    if ctx.scenario == "root-duplicate" || seal_insert_first {
        let entered = retained_source_wait_cut(control, Cut::SourceCoreEntered).await;
        let weak = control.owner_weak(coordinator).unwrap();
        assert_eq!(coordinator.fixture_root_count(), Some(1));
        if seal_insert_first {
            assert_eq!(
                control.observe_owner(coordinator).unwrap().effects_started,
                0
            );
            let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
                .await
                .expect("real insertion precedes actual seal");
            assert!(
                !report.persistence_safe,
                "real admitted work is incomplete at seal"
            );
            resume_insertion.send(()).unwrap();
            return Some((entered.binding.unwrap().session_id(), None, weak));
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
        assert!(ctx
            .manager
            .get_pending_session(entered.binding.unwrap())
            .await
            .is_none());
        resume_insertion.send(()).unwrap();
    } else {
        drop(resume_insertion);
    }
    None
}

async fn retained_source_seed_and_commit(
    ctx: &RetainedSourceCtx<'_>,
    resume_native: tokio::sync::oneshot::Sender<()>,
    resume_commit: tokio::sync::oneshot::Sender<()>,
) -> Option<crate::session::session::SessionCommunication> {
    if ctx.flags.seed_scenario {
        observe_retained_seed_fixture(
            ctx.fixture,
            ctx.coordinator,
            ctx.control,
            ctx.manager,
            resume_native,
            ctx.scenario,
            ctx.seed_source,
        )
        .await;
    } else {
        drop(resume_native);
    }
    let mut expected_communication = None;
    if ctx.flags.under_commit {
        expected_communication =
            observe_retained_commit_setter(ctx.control, ctx.manager, resume_commit, ctx.flags.full)
                .await;
    } else {
        drop(resume_commit);
    }
    expected_communication
}

async fn retained_source_baseline_and_publication(
    ctx: &RetainedSourceCtx<'_>,
    expected_communication: &mut Option<crate::session::session::SessionCommunication>,
    wait_gate: tokio::sync::oneshot::Receiver<()>,
    resume_baseline: tokio::sync::oneshot::Sender<()>,
    resume_publication: tokio::sync::oneshot::Sender<()>,
) -> (
    uuid::Uuid,
    crate::session::selection::ReplacementFixtureOwnerWeak,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let (control, coordinator, manager) = (ctx.control, ctx.coordinator, ctx.manager);
    let (newer_b, under_commit) = (ctx.flags.newer_b, ctx.flags.under_commit);
    let baseline = retained_source_wait_cut(control, Cut::Baseline).await;
    let id = baseline.binding.unwrap().session_id();
    let owner_weak = control
        .owner_weak(coordinator)
        .expect("actual baseline owner Weak");
    assert_ne!(id, ctx.previous);
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
            *expected_communication =
                set_retained_fixture_communication(manager, id, ctx.flags.full).await;
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
    let idle = ctx
        .app
        .state::<Arc<crate::pty::idle_detector::IdleDetector>>();
    idle.register_session(id, crate::session::profile::IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(id);
    ctx.hooks.real_inject_sessions.lock().unwrap().insert(id);
    if ctx.flags.check_reacquire {
        wait_gate.await.expect("real not-ready observation");
    }
    resume_baseline.send(()).unwrap();
    if let Some(expected_version) = setter_version {
        retained_source_wait_cut(control, Cut::BeforePublication).await;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let actual_version = manager
            .with_current_carrier_snapshot(&permit, |current| current.versions[&id])
            .await;
        drop(permit);
        assert_retained_source_reconciliation(
            control,
            coordinator,
            actual_version,
            expected_version,
            baseline.initial_version,
        );
        resume_publication.send(()).unwrap();
    } else {
        drop(resume_publication);
    }
    (id, owner_weak)
}

async fn retained_source_inline_release(
    ctx: &RetainedSourceCtx<'_>,
    id: uuid::Uuid,
    resume_release: tokio::sync::oneshot::Sender<()>,
    resume_tail: tokio::sync::oneshot::Sender<()>,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    use std::future::Future;
    let (control, coordinator) = (ctx.control, ctx.coordinator);
    let shutdown_executing = ctx.flags.shutdown_executing;
    if ctx.flags.inline {
        retained_source_wait_cut(control, Cut::AdapterReturned).await;
        assert_retained_source_inline_boundary(
            control,
            coordinator,
            ctx.app,
            id,
            ctx.flags.newer_b,
        );
        if shutdown_executing {
            let close = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join());
            tokio::pin!(close);
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    close.as_mut().poll(cx).is_pending()
                ))
                .await,
                "actual worker still executes its admitted inline envelope"
            );
            resume_release.send(()).unwrap();
            retained_source_wait_cut(control, Cut::BeforeCarry).await;
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
}

async fn retained_source_walk_terminal(
    ctx: &RetainedSourceCtx<'_>,
    owner_weak: &crate::session::selection::ReplacementFixtureOwnerWeak,
    resume_terminal: tokio::sync::oneshot::Sender<()>,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let (control, coordinator) = (ctx.control, ctx.coordinator);
    if ctx.flags.walk_terminal {
        retained_source_wait_cut(control, Cut::BeforeTerminalRemoval).await;
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
}

async fn retained_source_controller(
    ctx: &RetainedSourceCtx<'_>,
    resumes: RetainedSourceResumes,
    wait_gate: tokio::sync::oneshot::Receiver<()>,
) -> RetainedSourceOutcome {
    let RetainedSourceResumes {
        terminal,
        tail,
        insertion,
        native,
        baseline,
        commit,
        release,
        publication,
    } = resumes;
    if let Some(outcome) = retained_source_root_prelude(ctx, insertion).await {
        return outcome;
    }
    let mut expected_communication = retained_source_seed_and_commit(ctx, native, commit).await;
    let (id, owner_weak) = retained_source_baseline_and_publication(
        ctx,
        &mut expected_communication,
        wait_gate,
        baseline,
        publication,
    )
    .await;
    retained_source_inline_release(ctx, id, release, tail).await;
    retained_source_walk_terminal(ctx, &owner_weak, terminal).await;
    (id, expected_communication, owner_weak)
}

async fn retained_source_finish(
    ctx: &RetainedSourceCtx<'_>,
    previous_slot: &mut uuid::Uuid,
    operation: usize,
    outcome: RetainedSourceOutcome,
    injection: Option<String>,
) -> bool {
    let (fixture, coordinator, control, manager) =
        (ctx.fixture, ctx.coordinator, ctx.control, ctx.manager);
    let (previous, flags) = (ctx.previous, ctx.flags);
    let (id, expected_communication, owner_weak) = outcome;
    if flags.seal_insert_first {
        assert_retained_source_sealed(
            fixture,
            coordinator,
            control,
            manager,
            previous,
            id,
            injection,
            ctx.store,
            &owner_weak,
        )
        .await;
        return true;
    }
    if flags.inline {
        assert_retained_source_receipt_order(control);
    }
    if flags.newer_b {
        assert_retained_source_newer_result(
            fixture,
            coordinator,
            control,
            manager,
            previous,
            id,
            flags.full,
            expected_communication,
        )
        .await;
    } else {
        assert_retained_source_positive(
            fixture, manager, previous, id, injection, ctx.store, ctx.hooks,
        )
        .await;
    }
    if flags.reclaim {
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
            retire_retained_fixture_member(fixture, coordinator, ctx.route, ctx.sender, id).await;
            *previous_slot = id;
        }
        return false;
    }
    let report = coordinator.close_and_join().await;
    assert_retained_source_lifetime(
        coordinator,
        &report,
        &owner_weak,
        flags.full,
        flags.shutdown_executing,
        flags.walk_terminal,
    );
    false
}

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
    let app = fixture.app.handle();
    let previous = *previous_slot;
    let (flags, scenario) = retained_source_flags(outer_scenario, pty_source, inline);
    let seed_source = fixture
        .target_cwd
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("default.p05-seed");
    if flags.seed_scenario {
        configure_retained_seed_fixture(fixture, &seed_source).await;
    }
    let (control, resumes) = retained_source_control(app, flags, scenario);
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
    let ctx = RetainedSourceCtx {
        fixture,
        app,
        coordinator,
        route,
        sender,
        previous,
        scenario,
        flags,
        control: &control,
        hooks: &hooks,
        poller: &poller,
        manager: &manager,
        state: &state,
        store: &store,
        seed_source: &seed_source,
    };
    let delivery =
        retained_source_delivery(&ctx, injection.as_deref(), &message, delivery_finished);
    let readiness_observer = retained_readiness_observer(&ctx, wait_observed, delivery_gate);
    let controller = retained_source_controller(&ctx, resumes, wait_gate);
    let ((), outcome, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(delivery, controller, readiness_observer)
    })
    .await
    .expect("source and controller both progress");
    retained_source_finish(&ctx, previous_slot, operation, outcome, injection).await
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

type NativeWaiterControl = (
    Arc<crate::session::selection::ReplacementFixtureControl>,
    crate::session::selection::ReplacementFixtureCut,
    crate::session::selection::ReplacementFixtureCut,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Sender<()>,
);

fn native_waiter_control(seed: bool) -> NativeWaiterControl {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
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
    (control, entered, returned, resume_native, panic_source)
}

async fn native_waiter_delivery(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    state: &crate::api::message_store::MessageStoreState,
    poller: &MailboxPoller,
    injection: Option<&str>,
    message: &OutboxMessage,
) {
    if let Some(injection) = injection {
        poller
            .dispatch_pty_input_operation(
                app,
                state,
                injection,
                crate::phone::types::PtyInputSourcePlane::HostCli,
                None,
            )
            .await;
    } else {
        let _ = poller.deliver_wake(app, message).await;
    }
}

async fn native_waiter_drain_and_result(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &crate::session::selection::ReplacementFixtureControl,
    returned: crate::session::selection::ReplacementFixtureCut,
    seed: bool,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        retained_source_wait_cut(control, returned).await;
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
    assert_retained_native_result(fixture, seed, &actual);
}

pub(super) async fn retained_exited_respawn_native_waiter_cell(pty: bool, seed: bool) {
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
    let (control, entered, returned, resume_native, panic_source) = native_waiter_control(seed);
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
        let delivery = native_waiter_delivery(app, &state, &poller, injection.as_deref(), &message);
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
    native_waiter_drain_and_result(&fixture, &coordinator, &control, returned, seed).await;
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
