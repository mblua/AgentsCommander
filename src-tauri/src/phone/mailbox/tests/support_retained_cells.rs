use super::*;

mod root_full;
pub(super) use root_full::retained_exited_respawn_root_full_cell;

pub(super) async fn assert_retained_failure_close(
    coordinator: &crate::session::selection::SelectionCoordinator,
    setup_rolled_back: bool,
) {
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual failure close");
    if setup_rolled_back {
        assert!(
            report.persistence_safe,
            "actual returned setup failure must finish its own verified rollback/handoff: {:?}",
            report.retained
        );
        assert_eq!(coordinator.fixture_root_count(), Some(0));
    } else {
        assert!(
            !report.persistence_safe,
            "incomplete failure cannot certify safety"
        );
    }
}

async fn configure_failure_cell_fixture(
    fixture: &MailboxFixture,
    control: &crate::session::selection::ReplacementFixtureControl,
    scenario: &str,
) {
    if scenario == "native-seed-panic" {
        let seed_source = fixture
            .target_cwd
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("default.p05-seed");
        configure_retained_seed_fixture(fixture, &seed_source).await;
    }
    if scenario == "pre-context" {
        *control.context_notification.lock().unwrap() =
            crate::session::selection::FixtureContextNotification::Armed;
    }
    if scenario == "pre-context" || scenario == "native-context-panic" {
        configure_retained_context_fixture(fixture).await;
    }
}

fn failure_cell_physical_cut(
    scenario: &str,
) -> Option<crate::session::selection::ReplacementFixtureCut> {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    match scenario {
        "pre-finalize" => Some(Cut::BeforeFinalizer),
        "post-carry-runtime" => Some(Cut::BeforeCarry),
        "post-rename-physical" => Some(Cut::PublicationPrepared),
        "post-retire-runtime" => Some(Cut::BeforeRetirement),
        _ => None,
    }
}

async fn failure_cell_delivery(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    state: &crate::api::message_store::MessageStoreState,
    poller: &MailboxPoller,
    control: &crate::session::selection::ReplacementFixtureControl,
    injection: Option<&str>,
    message: &OutboxMessage,
) -> Option<Result<OutboxMessage, String>> {
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
        None
    } else {
        retained_fixture_ordinary_wake(poller, app, message, control).await
    }
}

async fn wait_failure_source_unwound(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
) {
    loop {
        if control
            .observe_owner(coordinator)
            .is_some_and(|owner| owner.source_unwound)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
}

async fn failure_cell_receipt(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    control: &crate::session::selection::ReplacementFixtureControl,
    manager: &SessionManager,
    previous: Uuid,
) -> (
    crate::session::selection::ReplacementFixtureReceipt,
    crate::session::manager::PendingCreateBinding,
) {
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
    (receipt, binding)
}

struct FailureCellObservation<'a> {
    scenario: &'a str,
    pty: bool,
    inline: bool,
    control: &'a crate::session::selection::ReplacementFixtureControl,
    coordinator: &'a crate::session::selection::SelectionCoordinator,
    manager: &'a SessionManager,
    pty_manager: &'a Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
    receipt: crate::session::selection::ReplacementFixtureReceipt,
    kills_before: usize,
    backend: &'a MailboxMockPtyBackend,
}

async fn assert_failure_cell_outcome(
    obs: &FailureCellObservation<'_>,
    setup_rolled_back: bool,
    ordinary: Option<Result<OutboxMessage, String>>,
    owner: Option<crate::session::selection::ReplacementFixtureOwnerObservation>,
    old_publication: Option<Vec<u8>>,
) {
    let (scenario, pty, control, manager) = (obs.scenario, obs.pty, obs.control, obs.manager);
    let (binding, backend, kills_before) = (obs.binding, obs.backend, obs.kills_before);
    if setup_rolled_back {
        assert_retained_setup_failure(
            ordinary,
            scenario,
            obs.inline,
            control,
            manager,
            obs.pty_manager,
            binding,
            obs.receipt,
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
                control,
                owner,
                manager,
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
            control,
            obs.coordinator,
            manager,
            obs.pty_manager,
            binding,
            owner,
            kills_before,
            backend,
            old_publication,
            obs.receipt,
        )
        .await;
    }
}

async fn failure_cell_native_controller(
    (scenario, physical_cut): (
        &str,
        Option<crate::session::selection::ReplacementFixtureCut>,
    ),
    fixture: &MailboxFixture,
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty_manager: &Arc<Mutex<PtyManager>>,
    (resume_physical, resume_context): (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Sender<()>,
    ),
) {
    retained_failure_physical_controller(
        scenario,
        physical_cut,
        control,
        coordinator,
        pty_manager,
        resume_physical,
    )
    .await;
    retained_failure_context_controller(
        scenario,
        fixture,
        coordinator,
        control,
        manager,
        pty_manager,
        resume_context,
    )
    .await;
}

fn failure_cell_setup_rolled_back(scenario: &str) -> bool {
    matches!(
        scenario,
        "pre-backend" | "pre-binder" | "pre-context" | "pre-finalize"
    )
}

fn failure_cell_old_publication(scenario: &str) -> Option<Vec<u8>> {
    (scenario == "post-rename-physical")
        .then(|| std::fs::read(crate::config::config_dir().unwrap().join("sessions.json")).unwrap())
}

fn failure_cell_poller(
    control: &Arc<crate::session::selection::ReplacementFixtureControl>,
) -> MailboxPoller {
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    MailboxPoller::new_with_test_hooks(hooks)
}

async fn failure_cell_ordinary(
    delivery: impl std::future::Future<Output = Option<Result<OutboxMessage, String>>>,
    unwound: impl std::future::Future<Output = ()>,
    native_controller: impl std::future::Future<Output = ()>,
) -> Option<Result<OutboxMessage, String>> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let source = async {
            tokio::select! { actual = delivery => actual, () = unwound => None }
        };
        let (actual, ()) = tokio::join!(source, native_controller);
        actual
    })
    .await
    .expect("real failure return or actual accepted-source unwind")
}

pub(super) async fn retained_exited_respawn_failure_cell(pty: bool, inline: bool, scenario: &str) {
    use crate::session::selection::ReplacementFixtureControl;
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
    configure_failure_cell_fixture(&fixture, &control, scenario).await;
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
    let physical_cut = failure_cell_physical_cut(scenario);
    let (resume_physical, physical_gate) = tokio::sync::oneshot::channel();
    let mut physical_gate = Some(physical_gate);
    let old_publication = failure_cell_old_publication(scenario);
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
    let poller = failure_cell_poller(&control);
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let injection =
        pty.then(|| enqueue_shared_pty_engine_operation(&store, &route, sender, b"failure-source"));
    let mut message = wake_message_to_target();
    message.id = format!("failure-source-{}", Uuid::new_v4());
    let delivery = failure_cell_delivery(
        app,
        &state,
        &poller,
        &control,
        injection.as_deref(),
        &message,
    );
    let unwound = wait_failure_source_unwound(&control, &coordinator);
    let native_controller = failure_cell_native_controller(
        (scenario, physical_cut),
        &fixture,
        &control,
        &coordinator,
        &manager,
        &pty_manager,
        (resume_physical, resume_context),
    );
    let ordinary = failure_cell_ordinary(delivery, unwound, native_controller).await;
    let (receipt, binding) = failure_cell_receipt(app, &control, &manager, previous).await;
    let owner = control.observe_owner(&coordinator);
    let setup_rolled_back = failure_cell_setup_rolled_back(scenario);
    let observation = FailureCellObservation {
        scenario,
        pty,
        inline,
        control: &control,
        coordinator: &coordinator,
        manager: &manager,
        pty_manager: &pty_manager,
        binding,
        receipt,
        kills_before,
        backend,
    };
    assert_failure_cell_outcome(
        &observation,
        setup_rolled_back,
        ordinary,
        owner,
        old_publication,
    )
    .await;
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

type PtyHeldGates = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Sender<()>,
    Option<tokio::sync::oneshot::Sender<()>>,
);

fn pty_held_control() -> (
    Arc<crate::session::selection::ReplacementFixtureControl>,
    MailboxPoller,
    PtyHeldGates,
) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let control = Arc::new(ReplacementFixtureControl::default());
    let (resume_validation, validation) = tokio::sync::oneshot::channel();
    let (resume_bridge, bridge) = tokio::sync::oneshot::channel();
    let (lose_receiver, receiver) = tokio::sync::oneshot::channel();
    let lose_receiver = Some(lose_receiver);
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
    (
        control,
        poller,
        (resume_validation, resume_bridge, lose_receiver),
    )
}

fn pty_held_fail_spawn(pty: &Arc<Mutex<PtyManager>>) {
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

async fn pty_held_change_recipe(app: &tauri::AppHandle<tauri::test::MockRuntime>) {
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

async fn wait_pty_held_restoration_failed(
    control: &crate::session::selection::ReplacementFixtureControl,
) {
    loop {
        if control.restoration_failed.load(Ordering::Acquire) {
            break;
        }
        tokio::task::yield_now().await;
    }
}

fn assert_pty_held_observed(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    scenario: &str,
) {
    use crate::session::selection::OwnedSourceStop;
    let observed = control
        .observe_owner(coordinator)
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
}

pub(super) async fn retained_exited_respawn_pty_held_cell(scenario: &str) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, route, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    assert!(app.manage(crate::shutdown::ShutdownSignal::new()));
    let (control, poller, (resume_validation, resume_bridge, mut lose_receiver)) =
        pty_held_control();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    if scenario == "pty-held-rollback" {
        pty_held_fail_spawn(&pty);
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
            pty_held_change_recipe(app).await;
        }
        if scenario == "pty-cancel-held" {
            app.state::<crate::shutdown::ShutdownSignal>().trigger();
            drop(lose_receiver.take());
            wait_pty_held_restoration_failed(&control).await;
            assert!(control.restoration_shared.load(Ordering::Acquire));
            resume_validation
                .send(())
                .expect("real worker still owns clone after source deadline cancellation");
        } else {
            resume_validation.send(()).unwrap();
        }
        retained_source_wait_cut(&control, Cut::AdapterOutcomeBeforeLeaseDrop).await;
        assert_pty_held_observed(&control, &coordinator, scenario);
        if scenario != "pty-cancel-held" {
            lose_receiver
                .take()
                .unwrap()
                .send(())
                .expect("actual live helper receiver");
            wait_pty_held_restoration_failed(&control).await;
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
