use super::*;

fn create_admission_owner(
    tracker: crate::pty::manager::ResponseCloseState,
    target: &str,
    cwd: &std::path::Path,
) -> Arc<Mutex<super::super::OwnedCreateAdmission>> {
    Arc::new(Mutex::new(super::super::OwnedCreateAdmission {
        preheld_target: false,
        owned_replacement: None,
        replacement_pty: None,
        tracker,
        target: target.to_owned(),
        cwd: cwd.to_path_buf(),
        nonce: Uuid::new_v4(),
        binding: SessionManager::new().reserve_carrier_create_binding(),
        reservation: None,
        failed_foreign: false,
        owned_target: None,
        capacity: None,
        resource: None,
        blocked_bound: false,
        capacity_error: None,
    }))
}

#[tokio::test]
async fn create_foreign_owners_are_independent_after_first_waiter_loss() {
    let temp = tempfile::tempdir().unwrap();
    let tracker = crate::pty::manager::ResponseCloseState::default();
    let manager = SessionManager::new();
    let target = format!("foreign-create:{}", Uuid::new_v4());
    let first = create_admission_owner(tracker.clone(), &target, temp.path());
    let first_nonce = first.lock().unwrap().nonce;
    let task_manager = manager.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            let permit = super::super::acquire_create_publication_region(&first, &task_manager)
                .await
                .expect("Foreign region");
            assert!(first.lock().unwrap().reservation.is_some());
            drop(permit);
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            drop(first);
            finished_tx.send(()).unwrap();
        })
        .await;
    });
    started_rx.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let second = create_admission_owner(tracker.clone(), &target, temp.path());
    let permit = super::super::try_acquire_create_publication_region(&second, &manager)
        .await
        .expect("second Foreign create advances while first accepted core remains retained");
    assert!(second.lock().unwrap().reservation.is_some());
    use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseCloseError};
    assert!(matches!(
        tracker.reserve_create_owner(
            &target,
            first_nonce,
            CreateClassification::Foreign {
                publication_epoch: 0
            }
        ),
        Err(ResponseCloseError::Conflict)
    ));
    drop(permit);
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    assert!(matches!(
        tracker.reserve_create_owner(
            &target,
            first_nonce,
            CreateClassification::Foreign {
                publication_epoch: 0
            }
        ),
        Ok(CreateAdmission::Reserved(_))
    ));
}

#[tokio::test]
async fn create_e2_uses_same_permanently_failed_tracker_and_actual_foreign_scope() {
    let temp = tempfile::tempdir().unwrap();
    let tracker = crate::pty::manager::ResponseCloseState::default();
    tracker.fail_closed();
    let manager = SessionManager::new();
    let owner = create_admission_owner(tracker.clone(), "failed-foreign", temp.path());
    let permit = super::super::try_acquire_create_publication_region(&owner, &manager)
        .await
        .expect("actual ALL-scope Foreign can use E2 after exact tracker Unavailable");
    {
        let owner = owner.lock().unwrap();
        assert!(owner.tracker.same_state(&tracker));
        assert!(owner.failed_foreign);
        assert!(owner.reservation.is_none());
    }
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            assert!(!owner.lock().unwrap().reserve_current(
                crate::config::sessions_persistence::CreateCarrierScope::Waiting,
                current
            ));
            assert!(
                !owner.lock().unwrap().failed_foreign,
                "Waiting cannot retain previous E2 admission evidence"
            );
        })
        .await;
    tracker.apply_settings(true, 30, std::time::Instant::now());
    assert!(matches!(
        tracker.reserve_create_owner(
            "failed-foreign",
            Uuid::new_v4(),
            crate::pty::manager::CreateClassification::Foreign {
                publication_epoch: 0
            }
        ),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    ));
    drop(permit);
}

#[tokio::test]
async fn create_protected_owner_is_exclusive_and_failed_tracker_cannot_use_e2() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("member");
    std::fs::create_dir(&cwd).unwrap();
    let manager = SessionManager::new();
    let row = manager
        .create_session(
            "bash".into(),
            vec![],
            cwd.to_string_lossy().into_owned(),
            None,
            None,
            vec![],
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    manager.set_start_fresh_on_restore(row.id, true).await;
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let version = manager
        .with_current_carrier_snapshot(&permit, |current| current.versions[&row.id])
        .await;
    drop(permit);
    let target = format!("protected-create:{}", Uuid::new_v4());
    let carrier = crate::config::sessions_persistence::persist_initial_carrier(
        &manager,
        temp.path().join("config"),
        vec![temp.path().to_string_lossy().into_owned()],
        crate::config::sessions_persistence::InitialCarrierIntent::new(
            target.clone(),
            row.id,
            row.token,
            version,
            std::fs::canonicalize(&cwd).unwrap(),
        ),
    )
    .await
    .unwrap()
    .expect("actual current carrier confirmed");
    let tracker = crate::pty::manager::ResponseCloseState::default();
    tracker.register_route(row.id, 1, std::time::Instant::now());
    let first = create_admission_owner(tracker.clone(), &target, &cwd);
    assert!(
        super::super::try_acquire_create_publication_region(&first, &manager)
            .await
            .is_none(),
        "Protected awaits owned Preparing before any create effect"
    );
    assert!(
        first.lock().unwrap().reservation.is_none(),
        "generic Protected wait takes no exclusive claim"
    );
    let second = create_admission_owner(tracker.clone(), &target, &cwd);
    assert!(
        super::super::try_acquire_create_publication_region(&second, &manager)
            .await
            .is_none()
    );
    assert!(
        second.lock().unwrap().reservation.is_none(),
        "second generic waiter also takes no exclusive claim"
    );
    #[cfg(target_os = "windows")]
    let quota_waiter = {
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::new());
        let config = crate::config::settings::AppSettings {
            resource_monitor_enabled: true,
            max_concurrent_agent_processes: 1,
            ..Default::default()
        };
        let settings = crate::config::settings::new_settings_state(config.clone());
        let resource = Arc::new(Mutex::new(super::super::CreateResourceAdmission {
            monitor: monitor.clone(),
            settings: settings.clone(),
            permit: None,
            needed: true,
            in_region: false,
            released: false,
        }));
        let third = create_admission_owner(tracker.clone(), &target, &cwd);
        third.lock().unwrap().resource = Some(resource.clone());
        let quota = monitor
            .try_reserve_agent_slot(super::super::ResourceLimits::from(&config))
            .unwrap()
            .expect("Windows quota really reserves a slot");
        let manager = manager.clone();
        let waiter = tokio::spawn(async move {
            let mut local = Some(quota);
            let result = super::super::with_create_resource_region(
                &third,
                &mut local,
                super::super::acquire_create_publication_region(&third, &manager),
            )
            .await;
            (result.map(drop), local)
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !resource.lock().unwrap().released {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("blocked create releases its actual quota");
        let replacement_quota = monitor
            .try_reserve_agent_slot(super::super::ResourceLimits::from(&config))
            .unwrap()
            .expect("protected owner can use the released quota");
        monitor.release_unregistered_permit(replacement_quota);
        settings.write().await.max_concurrent_agent_processes = 0;
        waiter
    };
    tracker.fail_closed();
    assert!(
        super::super::try_acquire_create_publication_region(&first, &manager)
            .await
            .is_none()
    );
    assert!(
        !first.lock().unwrap().failed_foreign,
        "Protected cannot use E2"
    );
    drop(carrier);
    #[cfg(target_os = "windows")]
    {
        let (result, quota) = tokio::time::timeout(std::time::Duration::from_secs(5), quota_waiter)
            .await
            .expect("unblocked create completes its real fresh quota probe")
            .unwrap();
        assert!(result
            .unwrap_err()
            .contains("Resource Monitor cap reached: 0/0"));
        assert!(
            quota.is_none(),
            "failed readmission does not fabricate a permit"
        );
    }
    let permit = super::super::try_acquire_create_publication_region(&second, &manager)
        .await
        .expect(
            "same failed tracker permits only freshly classified Foreign after key lifetime ends",
        );
    assert!(second.lock().unwrap().failed_foreign);
    drop(permit);
}

// Stage E (#1064) selection-intent mapping sentinel (plan section 10.4 item
// 20, acceptance item 42, 10.6): the internal-alert background create keeps
// `CreateSelectionIntent::Background`, which maps to the Background trusted
// intent (never downgraded); Suppress finalizes nothing.
#[test]
fn stage_e_create_selection_intent_maps_background_to_trusted_background() {
    use crate::session::selection::TrustedCreateIntent;
    assert!(matches!(
        CreateSelectionIntent::Background.trusted(),
        Some(TrustedCreateIntent::Background)
    ));
    assert!(matches!(
        CreateSelectionIntent::User.trusted(),
        Some(TrustedCreateIntent::User)
    ));
    assert!(
        CreateSelectionIntent::Suppress.trusted().is_none(),
        "a suppressed create never becomes a trusted finalizing create"
    );
}

#[test]
fn count_working_members_counts_live_and_busy_only() {
    // tuples are (live /*PTY-backed*/, waiting_for_input)
    assert_eq!(count_working_members(&[]), 0, "empty -> 0");
    assert_eq!(
        count_working_members(&[(true, true), (true, true)]),
        0,
        "all live but idle (waiting) -> 0"
    );
    assert_eq!(
        count_working_members(&[(true, false), (true, false), (true, false)]),
        3,
        "all live and busy -> N"
    );
    assert_eq!(
        count_working_members(&[(true, true), (true, false)]),
        1,
        "only the live+busy member counts"
    );
    assert_eq!(
        count_working_members(&[(false, false), (false, false)]),
        0,
        "busy but dead (no live PTY) -> 0"
    );
    assert_eq!(
        count_working_members(&[(true, false), (false, false), (true, true)]),
        1,
        "mixed: one live+busy, one dead+busy, one live+idle -> 1"
    );
}
