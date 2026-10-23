use super::*;

#[expect(
    clippy::too_many_arguments,
    reason = "P05 verifies the actual setup result and notification/rollback/runtime evidence \
        without bundling owners."
)]
pub(super) async fn assert_retained_setup_failure(
    ordinary: Option<Result<OutboxMessage, String>>,
    scenario: &str,
    inline: bool,
    control: &crate::session::selection::ReplacementFixtureControl,
    manager: &SessionManager,
    pty_manager: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
    receipt: crate::session::selection::ReplacementFixtureReceipt,
    kills_before: usize,
    backend: &MailboxMockPtyBackend,
) {
    if let Some(result) = ordinary {
        let error = result.expect_err("actual setup error");
        let expected = if scenario == "pre-backend" {
            "synthetic spawn failure"
        } else if scenario == "pre-context" {
            "is a real directory, not a file"
        } else if scenario == "pre-finalize" {
            "created session is not displayable"
        } else if inline {
            "inline create binding was already prepared"
        } else {
            "create finalization ticket was already bound"
        };
        assert!(error.contains(expected), "actual primary error: {error}");
    }
    if scenario == "pre-context" {
        let observed = control.context_notification.lock().unwrap();
        let crate::session::selection::FixtureContextNotification::Consumed {
            operation,
            executor,
            binding: observed_binding,
            error,
            title,
            message,
        } = &*observed
        else {
            panic!("explicit context notification arm was not consumed");
        };
        assert_eq!(*operation, receipt.operation);
        assert_ne!(*executor, Uuid::nil());
        assert_eq!(*observed_binding, binding);
        assert!(error.contains("is a real directory, not a file"));
        assert_eq!(title, "Session Launch Error");
        assert_eq!(
            message,
            &format!("Cannot launch session - failed to update replica config:\n\n{error}")
        );
    }
    assert!(manager.get_pending_session(binding).await.is_none());
    assert!(!pty_manager
        .lock()
        .unwrap()
        .has_session(binding.session_id()));
    assert_eq!(
        backend.kill_attempts.load(Ordering::SeqCst),
        kills_before + usize::from(scenario == "pre-finalize")
    );
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 keeps native failure, source result and actual pending/runtime evidence distinct."
)]
pub(super) async fn assert_retained_native_failure(
    ordinary: &Option<Result<OutboxMessage, String>>,
    scenario: &str,
    control: &crate::session::selection::ReplacementFixtureControl,
    owner: &crate::session::selection::ReplacementFixtureOwnerObservation,
    manager: &SessionManager,
    binding: crate::session::manager::PendingCreateBinding,
    backend: &MailboxMockPtyBackend,
    kills_before: usize,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let (entered, returned, expected) = if scenario == "native-seed-panic" {
        (
            Cut::SeedNativeEnter,
            Cut::SeedNativeReturned,
            "config seed blocking preparation failed",
        )
    } else {
        (
            Cut::ContextNativeEnter,
            Cut::ContextNativeReturned,
            "replica context blocking preparation failed",
        )
    };
    assert!(control
        .receipts
        .lock()
        .unwrap()
        .iter()
        .any(|receipt| receipt.cut == entered));
    assert!(!control
        .receipts
        .lock()
        .unwrap()
        .iter()
        .any(|receipt| receipt.cut == returned));
    if let Some(actual) = ordinary.as_ref() {
        assert!(
            actual.as_ref().unwrap_err().contains(expected),
            "actual native JoinError precedence"
        );
    }
    assert!(!owner.cleanup_claimed && !owner.rollback_completed);
    assert!(manager.get_pending_session(binding).await.is_some());
    assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills_before);
}

#[cfg(test)]
pub(super) fn assert_retained_failed_rollback(
    ordinary: Option<Result<OutboxMessage, String>>,
    pty: bool,
    owner: &crate::session::selection::ReplacementFixtureOwnerObservation,
    backend: &MailboxMockPtyBackend,
    kills_before: usize,
) {
    if pty {
        assert_eq!(
            owner.source_returned_failed,
            Some(true),
            "actual create Err survives retained step and entire PTY core"
        );
        assert!(!owner.source_unwound);
    }
    assert!(owner.cleanup_claimed && !owner.rollback_completed);
    assert_eq!(
        backend.kill_attempts.load(Ordering::SeqCst),
        kills_before + 1
    );
    if let Some(result) = ordinary {
        assert!(result
            .unwrap_err()
            .contains("simulated fixture precommit setup failure"));
    }
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 preserves each committed-loss and physical-publication assertion with original \
        owners borrowed."
)]
pub(super) async fn assert_retained_committed_failure(
    scenario: &str,
    pty: bool,
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty_manager: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
    owner: &crate::session::selection::ReplacementFixtureOwnerObservation,
    kills_before: usize,
    backend: &MailboxMockPtyBackend,
    old_publication: Option<Vec<u8>>,
    receipt: crate::session::selection::ReplacementFixtureReceipt,
) {
    if scenario == "panic-commit-before-disarm" {
        assert!(!owner.cleanup_disarmed);
        assert!(owner.initial_version.is_none());
        assert!(
            manager.get_pending_session(binding).await.is_none(),
            "real commit made B nonpending"
        );
        assert!(pty_manager
            .lock()
            .unwrap()
            .has_session(binding.session_id()));
        assert!(
            !control
                .cleanup_observed_stopped(coordinator, manager, pty_manager)
                .await
        );
        assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills_before);
    }
    if scenario == "base-missing" || scenario == "panic-baseline" {
        assert!(owner.cleanup_disarmed && owner.initial_version.is_none());
        assert_eq!(owner.carry_attempts, 0);
    }
    if pty && scenario == "base-continue-loss" {
        assert_eq!(
            owner.source_returned_failed,
            Some(false),
            "actual Ok(Retained) is not a create failure"
        );
    }
    if scenario == "post-rename-physical" {
        assert!(owner.rename_failed && !owner.checkpoint_confirmed);
        let directory = crate::config::config_dir().unwrap();
        let backup = directory.join(format!("p05-rename-backup-{}.json", receipt.operation));
        assert_eq!(std::fs::read(backup).unwrap(), old_publication.unwrap());
        assert_eq!(
            std::fs::read(directory.join("sessions.json/nonempty-fixture")).unwrap(),
            b"real rename blocker"
        );
    }
    if scenario == "post-retire-runtime" || scenario == "post-carry-runtime" {
        assert!(owner.cleanup_disarmed && !owner.rollback_completed);
        assert_eq!(
            backend.kill_attempts.load(Ordering::SeqCst),
            kills_before + 1
        );
    }
}

#[cfg(test)]
pub(super) async fn assert_retained_cleanup_runtime_veto(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
) {
    let owner = control.observe_owner(coordinator).unwrap();
    assert!(!owner.cleanup_claimed && !owner.cleanup_disarmed && !owner.rollback_completed);
    assert!(manager.get_pending_session(binding).await.is_some());
    assert!(pty.lock().unwrap().has_session(binding.session_id()));
}

#[cfg(test)]
pub(super) async fn assert_retained_cleanup_claimed(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
) {
    let owner = control.observe_owner(coordinator).unwrap();
    assert!(owner.cleanup_claimed && owner.rollback_completed);
    assert!(!owner.cleanup_disarmed && owner.initial_version.is_none());
    assert!(manager.get_pending_session(binding).await.is_none());
    assert!(!pty.lock().unwrap().has_session(binding.session_id()));
}

#[cfg(test)]
pub(super) async fn assert_retained_cleanup_finalized(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
) {
    assert!(manager.get_pending_session(binding).await.is_none());
    assert!(manager.get_session(binding.session_id()).await.is_some());
    assert!(pty.lock().unwrap().has_session(binding.session_id()));
    let owner = control.observe_owner(coordinator).unwrap();
    assert!(owner.cleanup_disarmed && !owner.cleanup_claimed);
    assert!(!owner.rollback_completed);
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 verifies actual stopped-source cleanup and runtime counters while original \
        gate/backend owners remain in caller."
)]
pub(super) async fn assert_retained_cleanup_common(
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &crate::session::selection::ReplacementFixtureControl,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
    binding: crate::session::manager::PendingCreateBinding,
    kills_before: usize,
    claim_first: bool,
    backend: &MailboxMockPtyBackend,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    assert!(
        !control
            .cleanup_observed_stopped(coordinator, manager, pty)
            .await,
        "same cleanup delegate cannot claim twice or kill finalized B"
    );
    assert_eq!(
        backend.kill_attempts.load(Ordering::SeqCst),
        kills_before + usize::from(claim_first),
        "exactly one real pending kill, or zero after finalize"
    );
    let owner = control.observe_owner(coordinator).unwrap();
    assert!(!owner.terminal && !owner.transferred);
    assert!(owner.source_stopped);
    assert!(!control.received.load(Ordering::Acquire));
    assert!(!control
        .receipts
        .lock()
        .unwrap()
        .iter()
        .any(|receipt| receipt.cut == Cut::SourceReceived));
    assert!(shared_engine_writes(app, binding.session_id()).is_empty());
}

#[cfg(test)]
pub(super) fn assert_retained_cleanup_close(
    report: &crate::session::selection::SelectionShutdownReport,
    backend: &MailboxMockPtyBackend,
    kills_before: usize,
    claim_first: bool,
) {
    assert!(
        !report.persistence_safe,
        "pending cleanup is not work-transfer authority"
    );
    assert_eq!(
        backend.kill_attempts.load(Ordering::SeqCst),
        kills_before + usize::from(claim_first),
        "after-join cleanup cannot kill a second time"
    );
}

#[cfg(test)]
pub(super) fn assert_retained_source_inline_boundary(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    app: &tauri::AppHandle<tauri::test::MockRuntime>,
    id: uuid::Uuid,
    newer_b: bool,
) {
    let (published, inner_released, work_started, transferred, held) = control
        .observe_retained(coordinator)
        .expect("owner retained before fixture admission release");
    let actual_owner = control
        .observe_owner(coordinator)
        .expect("matching operation remains in the actual root");
    assert!(
        !actual_owner.terminal && !actual_owner.transferred,
        "early adapter flags do not satisfy the actual terminal predicate"
    );
    if !newer_b {
        assert!(
            published && inner_released,
            "actual common inline flags are early"
        );
    }
    assert!(!transferred);
    assert!(!work_started);
    assert!(held >= 1, "actual initial fixture admission remains held");
    assert!(shared_engine_writes(app, id).is_empty());
    assert!(!control.received.load(Ordering::Acquire));
}

#[cfg(test)]
pub(super) fn assert_retained_source_reconciliation(
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    actual_version: u64,
    expected_version: u64,
    initial_version: Option<u64>,
) {
    assert_eq!(
        actual_version, expected_version,
        "recapture performs zero stale B assignments/version invalidations"
    );
    let observed = control.observe_owner(coordinator).unwrap();
    assert!(observed.carry_recaptured);
    assert_eq!(observed.carry_attempts, 1);
    assert_eq!(
        observed.initial_version, initial_version,
        "initial baseline remains immutable"
    );
}

#[cfg(test)]
pub(super) fn assert_retained_source_receipt_order(
    control: &crate::session::selection::ReplacementFixtureControl,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let receipts = control.receipts.lock().unwrap();
    let position = |cut| {
        receipts
            .iter()
            .position(|receipt| receipt.cut == cut)
            .unwrap()
    };
    assert!(position(Cut::AdapterReturned) < position(Cut::AdmissionReleased));
    assert!(position(Cut::AdmissionReleased) < position(Cut::SourceReceived));
    assert!(receipts
        .iter()
        .all(|receipt| receipt.operation == receipts[0].operation));
    assert!(receipts
        .iter()
        .find(|receipt| receipt.cut == Cut::AdapterReturned)
        .and_then(|receipt| receipt.outcome)
        .is_some());
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 compares current B communication and source ownership using the original actual \
        observations."
)]
pub(super) async fn assert_retained_source_newer_result(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &crate::session::selection::ReplacementFixtureControl,
    manager: &SessionManager,
    previous: uuid::Uuid,
    id: uuid::Uuid,
    full: bool,
    expected_communication: Option<SessionCommunication>,
) {
    let app = fixture.app.handle();
    if full {
        let actual = manager.get_session(id).await.unwrap().communication;
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected_communication).unwrap()
        );
    } else {
        assert_eq!(
            manager
                .get_session(id)
                .await
                .unwrap()
                .telegram_bot_id
                .as_deref(),
            Some("newer-b")
        );
    }
    if full {
        assert!(
            shared_engine_writes(app, id).is_empty(),
            "actual blocked menu vetoes input"
        );
        assert!(control.observe_retained(coordinator).is_some());
    } else {
        assert!(
            manager.get_session(previous).await.is_none(),
            "current B reconciliation retires A with fresh proof"
        );
        assert!(
            !shared_engine_writes(app, id).is_empty(),
            "current reconciled B receives actual source bytes"
        );
        assert!(
            control.observe_retained(coordinator).is_none(),
            "only actual fresh proof and work transfer allow removal"
        );
    }
}

#[cfg(test)]
pub(super) async fn assert_retained_source_positive(
    fixture: &MailboxFixture,
    manager: &SessionManager,
    previous: uuid::Uuid,
    id: uuid::Uuid,
    injection: Option<String>,
    store: &crate::api::message_store::MessageStore,
    hooks: &MailboxTestHooks,
) {
    let app = fixture.app.handle();
    assert!(manager.get_session(previous).await.is_none());
    assert!(
        !shared_engine_writes(app, id).is_empty(),
        "actual backend bytes"
    );
    if let Some(injection) = injection {
        let result = store
            .query_pty_input_by_injection(&injection)
            .unwrap()
            .unwrap();
        assert_eq!(
            result.status,
            crate::phone::types::PtyInputPublicStatus::Injected
        );
        assert_eq!(
            result.selected_session_id.as_deref(),
            Some(id.to_string().as_str())
        );
    }
    assert!(
        hooks.spawn_calls.lock().unwrap().is_empty(),
        "legacy fake spawn unused"
    );
    assert!(
        hooks.destroy_calls.lock().unwrap().is_empty(),
        "A retired by owned path"
    );
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 preserves sealed-entry assertions on the same fixture, source operation and \
        retained owner."
)]
pub(super) async fn assert_retained_source_sealed(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &crate::session::selection::ReplacementFixtureControl,
    manager: &SessionManager,
    previous: uuid::Uuid,
    id: uuid::Uuid,
    injection: Option<String>,
    store: &crate::api::message_store::MessageStore,
    owner_weak: &crate::session::selection::ReplacementFixtureOwnerWeak,
) {
    let app = fixture.app.handle();
    assert!(manager.get_session(previous).await.is_some());
    let binding = control.receipts.lock().unwrap()[0].binding.unwrap();
    assert!(manager.get_pending_session(binding).await.is_none());
    assert!(manager.get_session(id).await.is_none());
    assert!(shared_engine_writes(app, id).is_empty());
    assert!(owner_weak.is_alive());
    assert_eq!(coordinator.fixture_root_count(), Some(1));
    if let Some(injection) = injection {
        let actual = store
            .query_pty_input_by_injection(&injection)
            .unwrap()
            .unwrap();
        assert_ne!(
            actual.status,
            crate::phone::types::PtyInputPublicStatus::Injected
        );
        if actual.terminal {
            assert!(
                control.observe_owner(coordinator).unwrap().terminal,
                "only actual verified rollback/handoff can terminally reject retained PTY work"
            );
        }
    }
    assert!(
        !coordinator.close_and_join().await.persistence_safe,
        "original incomplete-at-seal close report remains conservative"
    );
}

#[cfg(test)]
pub(super) fn assert_retained_source_lifetime(
    coordinator: &crate::session::selection::SelectionCoordinator,
    report: &crate::session::selection::SelectionShutdownReport,
    owner_weak: &crate::session::selection::ReplacementFixtureOwnerWeak,
    full: bool,
    shutdown_executing: bool,
    walk_terminal: bool,
) {
    assert_eq!(
        report.persistence_safe,
        !full && !shutdown_executing,
        "actual retained-root close accounting: {:?}",
        report.retained
    );
    if walk_terminal || shutdown_executing {
        assert!(
            owner_weak.is_alive(),
            "sealed root retains its verified terminal owner"
        );
        assert_eq!(coordinator.fixture_root_count(), Some(1));
    } else if report.persistence_safe {
        assert!(
            !owner_weak.is_alive(),
            "real terminal removal releases actual owner after worker join"
        );
    } else {
        assert!(
            owner_weak.is_alive(),
            "actual incomplete root keeps owner after transient source ends"
        );
    }
}
