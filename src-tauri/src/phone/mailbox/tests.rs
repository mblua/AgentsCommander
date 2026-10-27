mod claim_and;
mod deliver_wake;
mod handle_self;
mod handle_self_2;
mod internal_live;
mod internal_no;
mod is_viable;
mod notice_prefilter;
mod owned_outcome;
#[cfg(windows)]
mod physical_cwd;
mod process_message;
mod project_refresh;
mod raise_hand;
mod remote_activity;
mod retain_unarchived;
mod retained_exited;
mod retry_prune;
mod self_clear;
mod self_handoff_failure;
mod self_restart;
mod self_switch;
mod shared_pty;
mod shared_pty_2;
mod support_commands;
mod support_core;
mod support_retained_asserts;
mod support_retained_cells;
mod support_retained_cleanup;
mod support_retained_failure;
mod support_retained_setup;
mod support_retained_source;
mod support_wake;
mod sustained_idle;
mod teardown;
mod test_settle;
mod the_pty;
mod wake_dispatch;
mod wake_dispatch_2;

use super::*;
use crate::config::settings::AppSettings;
use crate::phone::mailbox::handoff::{
    anti_spoof_accept, build_self_clear_handoff_prompt, build_self_restart_handoff_prompt,
    build_self_switch_handoff_prompt, canonicalize_msg_from_in_place, internal_wake_resumes,
    is_viable_root_recipient, is_viable_wake_candidate, next_sustained_idle_state,
    self_clear_handoff_base_prompt, self_restart_handoff_base_prompt,
    self_switch_handoff_base_prompt, wake_action_for, wake_spawn_skip_auto_resume,
    ForgottenSummary, SettleAction, WakeAction, SELF_FORGET_SUMMARY_MAX_CHARS,
};
use crate::phone::mailbox::notice::OutboxOrigin;
use crate::phone::mailbox::poller::{evaluate_gate, PurgeGatePeer, PurgeGateSession};
use crate::phone::mailbox::settle::{
    err_is_pty_session_missing, filter_sessions_by_fqn, live_settle_action, live_wake_route,
    self_clear_gate_advance, settle_tick, wait_for_restore_or_session, wake_settle_ready,
    LiveWakeRoute, RestoreWaitOutcome, SelfClearGateAction, SelfClearGateState, SelfClearPhase,
};
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::manager::PtyManager;
use crate::session::selection::{
    DestroyOutcome, DestroyRequest, DestructionSource, LifecycleAdmission, LifecycleEffectGuard,
    LifecycleEntryOutcome, LifecycleExecutor, LifecycleFuture, LifecycleLease, OpaqueFixtureInput,
    OpaqueFixtureOutput, RestartJobRequest, RootJobRequest, SelectionCoordinator,
    SelectionTransaction,
};
use crate::telegram::manager::TelegramBridgeManager;
use crate::{HashMap, MasterToken};
use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use support_commands::{
    build_raise_hand_message, build_self_clear_message_with_from,
    build_self_restart_message_with_from, build_self_switch_message, dispatch_wake_message,
    logical_command_message, make_gate_peer, make_readiness, make_self_switch_fixture,
    make_session_request_fixture, pending_self_clear_contains, pending_self_clear_len,
    read_reject_reason, read_self_clear_response_status, read_session_request_result_json,
    seed_raise_hand_session, seed_self_switch_session, session_request_file_for,
    session_request_result_file_for, wake_message_to_target, write_logical_command_message,
    write_raise_hand_task_title, write_session_request_for_test, write_wake_message,
    SELF_RESTART_TEST_FROM,
};
#[cfg(windows)]
use support_core::make_mailbox_fixture_in;
use support_core::{
    add_mailbox_session, add_mailbox_session_with_shape, app_handle,
    enqueue_shared_pty_engine_operation, make_mailbox_app, make_mailbox_fixture,
    make_root_route_fixture, make_session_info, make_test_pty_manager, path_string,
    prepare_retained_exited_respawn, remote_activity_notice, root_outbox_message,
    seed_shared_pty_engine_sessions, wake_agent, wake_agents, AppLifetimeProbe, MailboxFixture,
    MailboxMockPtyBackend, CANONICAL_WAKE_FROM, CANONICAL_WAKE_TO, LOCAL_WAKE_FROM, LOCAL_WAKE_TO,
    MAILBOX_MASTER_TOKEN, WAKE_BODY,
};
use support_retained_asserts::{
    assert_retained_cleanup_claimed, assert_retained_cleanup_close, assert_retained_cleanup_common,
    assert_retained_cleanup_finalized, assert_retained_cleanup_runtime_veto,
    assert_retained_committed_failure, assert_retained_failed_rollback,
    assert_retained_native_failure, assert_retained_setup_failure,
    assert_retained_source_inline_boundary, assert_retained_source_lifetime,
    assert_retained_source_newer_result, assert_retained_source_positive,
    assert_retained_source_receipt_order, assert_retained_source_reconciliation,
    assert_retained_source_sealed,
};
use support_retained_cells::{
    retained_exited_respawn_entry_restart_reclaim_cell,
    retained_exited_respawn_entry_root_reclaim_cell, retained_exited_respawn_failure_cell,
    retained_exited_respawn_pty_held_cell, retained_exited_respawn_root_full_cell,
    retained_exited_respawn_submit_refused_cell, retained_fixture_ordinary_wake,
    retained_source_wait_cut,
};
use support_retained_cleanup::{
    retained_exited_respawn_caller_cell, retained_exited_respawn_receive_after_release_cell,
    retained_exited_respawn_reservation_refusal_cell, retained_exited_respawn_root_refusal_cell,
    retained_source_cleanup_race, RetainedCleanupRaceGates,
};
use support_retained_failure::assert_retained_pty_preboundary_rejections;
use support_retained_failure::{
    assert_retained_native_result, observe_retained_native_source_unwind, retained_failure_action,
    retained_failure_context_controller, retained_failure_physical_controller,
    retained_root_reclaim_operation, run_retained_source_child_scenario,
};
use support_retained_setup::{
    configure_retained_seed_fixture, observe_retained_commit_setter, observe_retained_seed_fixture,
    prepare_additional_retained_member, provision_retained_fixture_member,
    retire_retained_fixture_member, set_retained_fixture_communication,
};
use support_retained_source::{
    configure_retained_context_fixture, retained_exited_respawn_native_waiter_cell,
    retained_exited_respawn_source_cell,
};
use support_wake::{
    add_mailbox_session_with_shell, assert_inject_results_consumed,
    assert_no_spawn_or_destroy_events, assert_spawn_call_matches_target,
    assert_spawned_session_matches_target, build_self_clear_message, count_forget_archives,
    enqueue_shared_pty_engine_operation_issued_at, mock_pty_writes_for, read_only_forget_archive,
    read_response_json, register_mock_pty_route, retained_exited_respawn_lifetime_cell,
    retained_exited_respawn_queued_cell, run_mailbox_message, seed_self_clear_session,
    seed_self_handoff, set_mock_pty_write_failure, shared_engine_writes, write_wake_outbox_message,
    write_wake_outbox_message_with_route, RetainedSourceChild, LSA_IDLE_THRESHOLD, LSA_MAX_WAIT,
    LSA_RESIZE_GRACE, LSA_SETTLE,
};
use tauri::Listener;
use tempfile::NamedTempFile;

type Issue3189Transaction = SelectionTransaction<tauri::test::MockRuntime>;

struct Issue3189TurnExecutor {
    id: Uuid,
    root: PathBuf,
    pending: Arc<crate::PendingSelfClear>,
    persists: Arc<AtomicU32>,
    outcome: Mutex<Option<tokio::sync::oneshot::Sender<self_handoff::failure::SelfHandoffOutcome>>>,
}

impl Issue3189TurnExecutor {
    async fn driver(&self) -> self_handoff::failure::SelfHandoffOutcome {
        use crate::phone::mailbox::handoff::SELF_HANDOFF_FLAVOR_SWITCH;
        let persists = self.persists.clone();
        MailboxPoller::drive_self_switch_after_sustained_idle(
            self.id,
            self.root.clone(),
            "codex".into(),
            "A".into(),
            None,
            SELF_HANDOFF_FLAVOR_SWITCH,
            self.pending.clone(),
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(2),
            |_| async { (true, true) },
            move |_, _, _| {
                persists.fetch_add(1, Ordering::SeqCst);
                async { Err("outside worker reached persistence".into()) }
            },
            |_, _, _| async { panic!("restart after turn/persist refusal") },
            |_, _| async { panic!("inject after turn/persist refusal") },
            |_, _| async { panic!("boundary after turn/persist refusal") },
        )
        .await
    }
}

impl LifecycleExecutor<Issue3189Transaction> for Issue3189TurnExecutor {
    fn prepare<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: Uuid,
        _: Option<Uuid>,
        _: bool,
        _: Option<&'a str>,
    ) -> LifecycleFuture<'a, Result<LifecycleAdmission, String>> {
        panic!("unused prepare")
    }
    fn validate<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: &'a mut LifecycleLease,
    ) -> LifecycleFuture<'a, bool> {
        panic!("unused validate")
    }
    fn has_claim(&self, _: &LifecycleLease) -> bool {
        panic!("unused has_claim")
    }
    fn release(&self, _: LifecycleLease) -> bool {
        panic!("unused release")
    }
    fn ensure_root_directory(&self, _: LifecycleEffectGuard) -> Result<String, String> {
        panic!("unused ensure_root_directory")
    }
    fn destroy<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: DestroyRequest,
    ) -> LifecycleFuture<'a, Result<DestroyOutcome, String>> {
        Box::pin(async move {
            let outcome = self.driver().await;
            self.outcome
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(outcome)
                .unwrap();
            Ok(DestroyOutcome::default())
        })
    }
    fn restart<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: RestartJobRequest,
        _: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        panic!("unused restart")
    }
    fn root<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: RootJobRequest,
        _: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        panic!("unused root")
    }
    fn conditional_close<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: crate::pty::manager::ResponseCloseTicket,
    ) -> LifecycleFuture<'a, Result<(), String>> {
        panic!("unused conditional_close")
    }
    fn reconcile<'a>(&'a self, _: &'a Issue3189Transaction) -> LifecycleFuture<'a, ()> {
        panic!("unused reconcile")
    }
    fn complete_retirement<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: Uuid,
    ) -> LifecycleFuture<'a, ()> {
        panic!("unused complete_retirement")
    }
    fn execute_fixture<'a>(
        &'a self,
        _: &'a Issue3189Transaction,
        _: OpaqueFixtureInput,
    ) -> LifecycleFuture<'a, OpaqueFixtureOutput> {
        panic!("unused execute_fixture")
    }
}

#[tokio::test]
async fn issue_3189_selection_turn_refusal_reports() {
    use self_handoff::failure::{consume_outcome, SelfHandoffOutcome};
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("SELF-HANDOFF.md"), "turn notes").unwrap();
    let id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    assert!(pending.seed_legacy(id));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let executor = Arc::new(Issue3189TurnExecutor {
        id,
        root: root.path().into(),
        pending: pending.clone(),
        persists: Arc::new(AtomicU32::new(0)),
        outcome: Mutex::new(Some(sender)),
    });
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator =
        SelectionCoordinator::new(manager.clone(), tokio_util::sync::CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    coordinator
        .start_with_executor(app.handle().clone(), Some(executor.clone()))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        coordinator.submit_restore_first().await.unwrap().finish();
        coordinator
            .destroy(DestroyRequest {
                ids: vec![],
                source: DestructionSource::ManualClose,
                force_destroy_root: false,
            })
            .await
            .unwrap();
        let outcome = receiver.await.unwrap();
        let mut reports = 0;
        consume_outcome(async { outcome }, |failure| {
            reports += 1;
            let pending = pending.clone();
            async move {
                assert_eq!(
                    failure.reason,
                    crate::session::selection::SelectionCoordinatorError::RecursiveSubmission
                        .to_string()
                );
                assert!(!failure.continuation);
                assert_eq!(failure.current, Some(id));
                assert!(pending.is_empty());
                let _turn = crate::session::selection::acquire_selection_operation_turn()
                    .await
                    .unwrap();
            }
        })
        .await;
        assert_eq!(reports, 1);
        coordinator.close_and_join().await;
    })
    .await
    .unwrap();
    assert_eq!(executor.persists.load(Ordering::SeqCst), 0);
    assert!(pending.seed_legacy(id));
    let outcome = tokio::time::timeout(Duration::from_secs(3), executor.driver())
        .await
        .unwrap();
    let SelfHandoffOutcome::Failed(failure) = outcome else {
        panic!("persist must fail")
    };
    assert_eq!(failure.reason, "outside worker reached persistence");
    assert_eq!(executor.persists.load(Ordering::SeqCst), 1);
    assert!(pending.is_empty());
    assert_eq!(
        std::fs::read(root.path().join("SELF-HANDOFF.md")).unwrap(),
        b"turn notes"
    );
    assert!(!root.path().join("self-clear").exists());
}

#[derive(Default)]
struct Issue3189Backend {
    writes: Mutex<Vec<Vec<u8>>>,
    fail_at: Mutex<Option<usize>>,
    attempts: AtomicU32,
}

impl PtyBackend for Issue3189Backend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn write(
        &self,
        _: &crate::pty::manager::BackendWriteAuthority,
        _: Uuid,
        bytes: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        let count = self.attempts.fetch_add(1, Ordering::SeqCst) as usize;
        if *self.fail_at.lock().unwrap() == Some(count) {
            return Err(crate::errors::AppError::PtyError(
                "scripted write rejection".into(),
            ));
        }
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }
    fn kill(&self, _: Uuid) -> Result<(), crate::errors::AppError> {
        Ok(())
    }
    fn has_session(&self, _: Uuid) -> bool {
        true
    }
    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {}
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

async fn issue_3189_notice_app() -> (
    tauri::App<tauri::test::MockRuntime>,
    Uuid,
    Arc<Issue3189Backend>,
) {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let id = {
        let mgr = manager.read().await;
        let session = mgr
            .create_session(
                "codex".into(),
                vec![],
                "C:\test".into(),
                Some("codex".into()),
                None,
                vec![],
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        mgr.mark_idle(session.id).await;
        session.id
    };
    let backend = Arc::new(Issue3189Backend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess);
    let idle = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    idle.register_session(id, crate::session::profile::IdleTuning::DEFAULT);
    let app = tauri::test::mock_builder()
        .manage(manager)
        .manage(pty)
        .manage(crate::config::settings::new_settings_state(
            AppSettings::default(),
        ))
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()))
        .manage(Arc::new(crate::PendingSelfClear::default()))
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(idle)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (app, id, backend)
}

fn issue_3189_failure(id: Uuid) -> self_handoff::failure::SelfHandoffFailure {
    self_handoff::failure::SelfHandoffFailure {
        action: "self-handoff-and-switch",
        attempt: Uuid::new_v4(),
        original: id,
        current: Some(id),
        root: PathBuf::from("C:/test"),
        continuation: false,
        reason: "selection locked: unlock it or use a reviewed force".into(),
        forgotten: true,
    }
}

fn issue_3189_warnings(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
) -> Vec<crate::session::warnings::SessionWarning> {
    app.state::<crate::session::warnings::SessionWarningState>()
        .lock()
        .unwrap()
        .drain(None)
}

#[tokio::test]
async fn issue_3189_real_warning_and_notice() {
    let (app, id, backend) = issue_3189_notice_app().await;
    let app = app.handle();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    app.listen(
        crate::session::warnings::SESSION_ENV_WARNING_EVENT,
        move |event| {
            seen.lock().unwrap().push(event.payload().to_string());
        },
    );
    MailboxPoller::report_self_handoff_failure(app, issue_3189_failure(id)).await;
    let warnings = issue_3189_warnings(app);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].kind, "self-handoff-failed");
    assert_eq!(events.lock().unwrap().len(), 1);
    let writes = backend.writes.lock().unwrap();
    assert_eq!(writes.len(), 3);
    let notice = String::from_utf8_lossy(&writes[0]);
    assert!(notice.starts_with("[AgentsCommander self-handoff failure]"));
    assert!(notice.contains("unlock it or use a reviewed force"));
    assert!(notice.contains("if no later attempt superseded it, retry"));
    assert_eq!(&writes[1..], &[b"\r".to_vec(), b"\r".to_vec()]);
}

#[tokio::test]
async fn issue_3189_retry_callback_pending_cancels() {
    for class in 0..4 {
        let cancel = class != 0;
        let (app, id, backend) = issue_3189_notice_app().await;
        let app = app.handle();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let permit = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
        let signal = Arc::new(tokio::sync::Notify::new());
        self_handoff::failure::register_inject_signal(id, Some(signal.clone()));
        let report = MailboxPoller::report_self_handoff_failure(app, issue_3189_failure(id));
        tokio::pin!(report);
        tokio::select! {
            _ = signal.notified() => {}
            _ = &mut report => panic!("report must wait for held input permit"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("injector not reached"),
        }
        // Poll with a dedicated waker until blocked. The permit release must wake it;
        // this proves it registered at acquire, rather than merely reaching the signal.
        let wake = Arc::new(Issue3189Wake(AtomicU32::new(0)));
        let waker = std::task::Waker::from(wake.clone());
        let mut context = std::task::Context::from_waker(&waker);
        assert!(std::future::Future::poll(report.as_mut(), &mut context).is_pending());
        assert!(backend.writes.lock().unwrap().is_empty());
        let pending = app.state::<Arc<crate::PendingSelfClear>>();
        if class == 1 {
            assert!(pending.seed_legacy(id));
        } else if class >= 2 {
            use crate::session::maintenance::{PlainIdentity, PlainReservation};
            let owner = PlainIdentity {
                session_id: id,
                request_id: Uuid::new_v4(),
                action: "self-clear".into(),
                requested_by: "test".into(),
            };
            let PlainReservation::New(lease) = pending.reserve_plain(owner) else {
                panic!("expected fresh Plain reservation");
            };
            if class == 3 {
                pending.arm_plain(&lease).unwrap();
            }
        }
        let before = wake.0.load(Ordering::SeqCst);
        drop(permit);
        assert!(
            wake.0.load(Ordering::SeqCst) > before,
            "injector must await held permit"
        );
        tokio::time::timeout(Duration::from_secs(5), report)
            .await
            .unwrap();
        self_handoff::failure::register_inject_signal(id, None);
        let warnings = issue_3189_warnings(app);
        if cancel {
            assert!(backend.writes.lock().unwrap().is_empty());
            assert_eq!(warnings.len(), 2);
            assert!(warnings[1].key.ends_with("delivery:cancelled"));
        } else {
            assert_eq!(backend.writes.lock().unwrap().len(), 3);
            assert_eq!(warnings.len(), 1);
        }
    }
}

#[tokio::test]
async fn issue_3189_retry_callback_waiting_defers() {
    {
        let (app, id, backend) = issue_3189_notice_app().await;
        let app = app.handle();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let permit = PtyManager::acquire_input_writer(&pty, id).await.unwrap();
        let signal = Arc::new(tokio::sync::Notify::new());
        self_handoff::failure::register_inject_signal(id, Some(signal.clone()));
        let report = MailboxPoller::report_self_handoff_failure(app, issue_3189_failure(id));
        tokio::pin!(report);
        tokio::select! {
            _ = signal.notified() => {}
            _ = &mut report => panic!("report must wait for held input permit"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("injector not reached"),
        }
        // Poll with a dedicated waker until blocked. The permit release must wake it;
        // this proves it registered at acquire, rather than merely reaching the signal.
        let wake = Arc::new(Issue3189Wake(AtomicU32::new(0)));
        let waker = std::task::Waker::from(wake.clone());
        let mut context = std::task::Context::from_waker(&waker);
        assert!(std::future::Future::poll(report.as_mut(), &mut context).is_pending());
        assert!(backend.writes.lock().unwrap().is_empty());
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        manager.read().await.mark_busy(id).await;
        let before = wake.0.load(Ordering::SeqCst);
        drop(permit);
        assert!(
            wake.0.load(Ordering::SeqCst) > before,
            "injector must await held permit"
        );
        assert!(futures::poll!(&mut report).is_pending());
        assert!(backend.writes.lock().unwrap().is_empty());
        manager.read().await.mark_idle(id).await;
        tokio::time::timeout(Duration::from_secs(5), report)
            .await
            .unwrap();
        self_handoff::failure::register_inject_signal(id, None);
        let warnings = issue_3189_warnings(app);

        assert_eq!(backend.writes.lock().unwrap().len(), 3);
        assert_eq!(warnings.len(), 1);
        let writes = backend.writes.lock().unwrap();
        assert!(String::from_utf8_lossy(&writes[0])
            .starts_with("[AgentsCommander self-handoff failure]"));
        assert_eq!(&writes[1..], &[b"\r".to_vec(), b"\r".to_vec()]);
    }
}

#[tokio::test]
async fn issue_3189_hard_write_and_enter() {
    for failure_at in [0, 1] {
        let (app, id, backend) = issue_3189_notice_app().await;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("SELF-HANDOFF.md"), "write failure notes").unwrap();
        let mut failure = issue_3189_failure(id);
        failure.root = root.path().into();
        *backend.fail_at.lock().unwrap() = Some(failure_at);
        let report = MailboxPoller::report_self_handoff_failure(app.handle(), failure);
        let observe_failure = async {
            while backend.attempts.load(Ordering::SeqCst) as usize <= failure_at {
                tokio::task::yield_now().await;
            }
            // An idle observation after the failed write must not permit retrying partial input.
            app.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .read()
                .await
                .mark_idle(id)
                .await;
        };
        tokio::time::timeout(Duration::from_secs(6), async {
            tokio::join!(report, observe_failure)
        })
        .await
        .unwrap();
        assert_eq!(
            backend.attempts.load(Ordering::SeqCst) as usize,
            failure_at + 1
        );
        assert_eq!(backend.writes.lock().unwrap().len(), failure_at);
        let warnings = issue_3189_warnings(app.handle());
        assert_eq!(warnings.len(), 2);
        assert!(warnings[1].key.ends_with("delivery:failed"));
        assert!(warnings[1]
            .message
            .contains("agent receipt is not confirmed"));
        assert_eq!(
            std::fs::read(root.path().join("SELF-HANDOFF.md")).unwrap(),
            b"write failure notes"
        );
        assert!(!root.path().join("self-clear").exists());
    }
}

#[tokio::test]
async fn issue_3189_retry_near_deadline_completes() {
    use self_handoff::failure::{retry_notice, NoticeDelivery};
    for held in [false, true] {
        let (app, id, backend) = issue_3189_notice_app().await;
        let app = app.handle();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let permit = if held {
            Some(PtyManager::acquire_input_writer(&pty, id).await.unwrap())
        } else {
            None
        };
        let signal = Arc::new(tokio::sync::Notify::new());
        let entered = signal.clone();
        let delivery = retry_notice(
            Duration::from_millis(1),
            Duration::from_millis(100),
            || false,
            || async {
                entered.notify_one();
                crate::pty::inject::inject_internal_notice_into_supported_agent_session(
                    app,
                    id,
                    "historical notice",
                    |_| Ok(()),
                )
                .await
                .map_err(NoticeDelivery::Failed)
            },
            |_| false,
            "cancelled",
        );
        tokio::pin!(delivery);
        tokio::select! {
            _ = signal.notified() => {}
            _ = &mut delivery => panic!("delivery completed before Enters"),
        }
        // Deadline passage is the subject here; synchronization above proves start first.
        let release = async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            drop(permit);
        };
        let (status, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(delivery, release)
        })
        .await
        .unwrap();
        assert_eq!(status, NoticeDelivery::Delivered);
        assert_eq!(
            *backend.writes.lock().unwrap(),
            vec![
                b"historical notice".to_vec(),
                b"\r".to_vec(),
                b"\r".to_vec()
            ]
        );
        assert!(issue_3189_warnings(app).is_empty());
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        assert!(
            manager
                .read()
                .await
                .get_session(id)
                .await
                .unwrap()
                .agent_turn_armed
        );
        assert!(app
            .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
            .control_write_age(id)
            .is_none());
    }
}

#[test]
fn issue_3189_adapter_wiring() {
    let source = include_str!("self_handoff.rs");
    for (start, end) in [
        (
            "pub(crate) async fn run_self_clear_after_sustained_idle",
            "pub(crate) async fn drive_self_clear_after_sustained_idle",
        ),
        (
            "async fn run_self_switch_after_sustained_idle",
            "pub(crate) async fn report_self_handoff_failure",
        ),
    ] {
        let body = source
            .split_once(start)
            .unwrap()
            .1
            .split_once(end)
            .unwrap()
            .0;
        assert!(body.contains("consume_outcome("));
        assert!(body.contains("Self::report_self_handoff_failure(app, failure)"));
    }
    let switch = source
        .split_once("pub(crate) async fn handle_self_handoff_switch")
        .unwrap()
        .1
        .split_once("pub(crate) async fn handle_self_restart")
        .unwrap()
        .0;
    let restart = source
        .split_once("pub(crate) async fn handle_self_restart")
        .unwrap()
        .1
        .split_once("pub(crate) async fn run_self_clear_after_sustained_idle")
        .unwrap()
        .0;
    for (body, flavor) in [
        (switch, "SELF_HANDOFF_FLAVOR_SWITCH"),
        (restart, "SELF_HANDOFF_FLAVOR_RESTART"),
    ] {
        let spawn = body.split_once("tauri::async_runtime::spawn").unwrap().1;
        assert!(spawn.contains("Self::run_self_switch_after_sustained_idle("));
        assert!(spawn.contains(flavor));
    }
    let adapter = source
        .split_once("async fn run_self_switch_after_sustained_idle")
        .unwrap()
        .1;
    let persist = adapter
        .split_once("let persist = move")
        .unwrap()
        .1
        .split_once("let app_for_restart")
        .unwrap()
        .0;
    let guard = persist
        .split_once("crate::config::coding_agent_profiles::set_replica_coding_agent_selection(")
        .unwrap()
        .0;
    assert!(guard.contains(
        "if flavor.action == SELF_HANDOFF_FLAVOR_RESTART.action {
                    return Ok(());"
    ));
    let restart = adapter
        .split_once("let restart = move")
        .unwrap()
        .1
        .split_once("let app_for_inject")
        .unwrap()
        .0;
    assert!(restart.contains(
        "if flavor.action == SELF_HANDOFF_FLAVOR_SWITCH.action {
                        Some(profile)"
    ));
    let decode = restart.split_once("} else {").unwrap().1;
    assert!(decode
        .trim_start()
        .starts_with("decode_restart_profile_from_seam(&profile)"));
    assert!(!source.contains("async fn run_self_restart_after_sustained_idle"));
}

#[tokio::test]
async fn issue_3189_guard_deferrals() {
    for guard in ["busy", "typing", "menu"] {
        let (app, id, backend) = issue_3189_notice_app().await;
        let app = app.handle();
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let typing = app.state::<crate::pty::input_activity::TypingHoldState>();
        let menu = app.state::<Arc<crate::pty::menu_guard::MenuGuard>>();
        match guard {
            "busy" => manager.read().await.mark_busy(id).await,
            "typing" => {
                typing
                    .lock()
                    .unwrap()
                    .toggle_manual(id, Duration::from_secs(30));
            }
            "menu" => {
                let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
                    crate::config::settings::BlockingMenuConfig {
                        pattern: "Trust".into(),
                        notification: "trust".into(),
                        enabled: true,
                        captured_against: None,
                    },
                )];
                assert!(
                    menu.evaluate_logical_rows(
                        id,
                        &[crate::pty::watchers::frame::LogicalRow {
                            text: "Trust".into(),
                            start: 0,
                            end: 0,
                        }],
                        &entries
                    )
                    .is_blocked
                );
            }
            _ => unreachable!(),
        }
        let report = MailboxPoller::report_self_handoff_failure(app, issue_3189_failure(id));
        tokio::pin!(report);
        assert!(futures::poll!(&mut report).is_pending());
        assert!(backend.writes.lock().unwrap().is_empty());
        match guard {
            "busy" => manager.read().await.mark_idle(id).await,
            "typing" => typing.lock().unwrap().reset(id),
            "menu" => {
                menu.evaluate_logical_rows(id, &[], &[]);
            }
            _ => unreachable!(),
        }
        tokio::time::timeout(Duration::from_secs(5), report)
            .await
            .unwrap();
        assert_eq!(backend.writes.lock().unwrap().len(), 3);
        assert_eq!(issue_3189_warnings(app).len(), 1);
    }
}

#[tokio::test]
async fn issue_3189_unavailable_ui_fallback() {
    for shape in ["root", "exited", "missing", "agentless", "unsupported"] {
        let (app, original, backend) = issue_3189_notice_app().await;
        let app = app.handle();
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = manager.read().await;
        let target = match shape {
            "root" => {
                mgr.set_is_root_agent(original, true).await;
                original
            }
            "exited" => {
                mgr.mark_exited(original, 1).await;
                original
            }
            "missing" => Uuid::new_v4(),
            "agentless" | "unsupported" => {
                mgr.create_session(
                    if shape == "unsupported" {
                        "bash"
                    } else {
                        "codex"
                    }
                    .into(),
                    vec![],
                    "C:\test".into(),
                    if shape == "agentless" {
                        None
                    } else {
                        Some("codex".into())
                    },
                    None,
                    vec![],
                    false,
                    crate::pty::backend::SessionBackendKind::LocalProcess,
                )
                .await
                .unwrap()
                .id
            }
            _ => unreachable!(),
        };
        drop(mgr);
        MailboxPoller::report_self_handoff_failure(app, issue_3189_failure(target)).await;
        assert!(backend.writes.lock().unwrap().is_empty());
        let warnings = issue_3189_warnings(app);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[1].key.ends_with("delivery:unavailable"));
        assert!(warnings[1]
            .message
            .contains("The agent could not be notified"));
    }
}

struct Issue3189Wake(AtomicU32);
impl std::task::Wake for Issue3189Wake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
