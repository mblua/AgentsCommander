use super::*;

/// Never due in the test window (next: 2028-02-29).
const NEVER_DUE_EXPR: &str = "0 0 29 2 *";

/// Config-only rewrite: `state.json` is untouched.
fn edit_config(dir: &Path, edit: impl FnOnce(&mut LoopConfigToml)) -> LoopConfigToml {
    let mut config = read_loop_config(dir).expect("config");
    edit(&mut config);
    write_loop_config(dir.parent().expect("ac root"), &config).expect("write config");
    config
}

fn pending_state() -> LoopState {
    LoopState {
        last_checked_at: Some(Utc::now() - chrono::Duration::minutes(5)),
        pending_due_at: Some(Utc::now() - chrono::Duration::minutes(5)),
        pending_run_id: Some(Uuid::new_v4()),
        ..LoopState::default()
    }
}

/// `pending_loop_fixture` with a never-due trigger.
fn issue_2698_fixture() -> (tempfile::TempDir, PathBuf, Uuid) {
    let (tmp, dir) = pending_loop_fixture();
    edit_config(&dir, |c| c.trigger.expr = NEVER_DUE_EXPR.to_string());
    let run_id = crate::config::loops::read_loop_state(&dir)
        .expect("state")
        .pending_run_id
        .expect("pending run");
    (tmp, dir, run_id)
}

/// `t2b_fixture` (the CLI path resolves its target) seeded with a pending
/// run and a never-due trigger.
fn issue_2698_cli_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Uuid) {
    let (tmp, ac_root, dir) = t2b_fixture();
    edit_config(&dir, |c| c.trigger.expr = NEVER_DUE_EXPR.to_string());
    let state = pending_state();
    write_loop_state_atomic(&dir, &state).expect("write state");
    let run_id = state.pending_run_id.expect("pending run");
    (tmp, ac_root, dir, run_id)
}

/// T1 steps 2-3: a direct `scan_loop` that delivers `report`, then finds
/// the Loop lock held by the test and fails its commit.
async fn scan_with_the_loop_lock_held(
    scheduler: &Arc<LoopScheduler>,
    app: &tauri::App,
    project: &Path,
    dir: &Path,
    report: LoopDeliveryReport,
) -> Result<(), String> {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    scheduler.install_delivery_gate(LoopDeliveryGate {
        entered: Some(entered_tx),
        release: Some(release_rx),
        report: Some(report),
    });
    let scan = {
        let scheduler = Arc::clone(scheduler);
        let app = app.handle().clone();
        let (project, dir) = (project.to_path_buf(), dir.to_path_buf());
        tokio::spawn(async move {
            scheduler
                .scan_loop(
                    &SchedulerContext::Native(Arc::new(app)),
                    &project,
                    &dir,
                    false,
                    false,
                )
                .await
        })
    };
    entered_rx.await.expect("the scan entered the delivery");
    let lock = acquire_loop_lock_for_dir(dir, LOOP_LOCK_TIMEOUT).expect("test Loop lock");
    release_tx.send(()).expect("release the gate");
    let result = scan.await.expect("join scan");
    drop(lock);
    result
}

/// T1 steps 1-4, shared: returns the state bytes after the failed scan.
async fn failed_first_scan(
    scheduler: &Arc<LoopScheduler>,
    app: &tauri::App,
    project: &Path,
    dir: &Path,
    run_id: Uuid,
) -> Vec<u8> {
    let before = read_raw_loop_state(dir).expect("raw").expect("state");
    let err = scan_with_the_loop_lock_held(scheduler, app, project, dir, delivered_report())
        .await
        .expect_err("the commit fails on the held Loop lock");
    assert!(err.contains("loopLockTimeout"), "{err}");
    let after = read_raw_loop_state(dir).expect("raw").expect("state");
    assert_eq!(after, before, "state.json unchanged");
    let state = crate::config::loops::read_loop_state(dir).expect("state");
    assert_eq!(state.pending_run_id, Some(run_id), "run still pending");
    assert!(audit_rows(dir).is_empty(), "no audit row");
    assert!(has_record(scheduler, dir), "one record under the key");
    after
}

fn assert_old_run_not_recorded(dir: &Path, run_id: Uuid) {
    assert!(
        audit_rows(dir).iter().all(|row| row.run_id != run_id),
        "the old delivery was recorded against a changed Loop"
    );
    let state = crate::config::loops::read_loop_state(dir).expect("state");
    assert_eq!(state.last_delivered_at, None);
    assert_ne!(
        state.last_result.map(|r| r.kind),
        Some("delivered".to_string())
    );
}

#[tokio::test]
async fn issue_2698_failed_write_after_delivery_is_retried_not_redelivered() {
    let (tmp, dir, run_id) = issue_2698_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;

    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(gate_is_untaken(&scheduler), "the second scan re-delivered");
    let state = crate::config::loops::read_loop_state(&dir).expect("state");
    assert_eq!(state.pending_run_id, None);
    assert!(state.last_delivered_at.is_some());
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, LoopAuditKind::Delivered);
    assert_eq!(rows[0].run_id, run_id);
    assert!(!has_record(&scheduler, &dir), "record cleared");

    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("third scan");
    assert!(gate_is_untaken(&scheduler), "the third scan re-delivered");
    assert_eq!(audit_rows(&dir).len(), 1);
}

#[tokio::test]
async fn issue_2698_record_is_dropped_when_the_loop_generation_changes() {
    let (tmp, dir, run_id) = issue_2698_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
    scheduler.bump_loop_generation(&dir);

    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(!gate_is_untaken(&scheduler), "normal scan delivers");
    assert!(!has_record(&scheduler, &dir));
}

#[tokio::test]
async fn issue_2698_record_is_dropped_after_a_cli_edit() {
    let (tmp, ac_root, dir, run_id) = issue_2698_cli_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    let failed = failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
    let entry = recorded_entry(&scheduler, &dir);
    let generation = scheduler.loop_generation(&dir);

    crate::config::loops::update_loop_files(
        tmp.path(),
        &ac_root,
        "daily-sync",
        crate::config::loops::LoopUpdatePatch {
            prompt_body: Some("edited".into()),
            ..Default::default()
        },
    )
    .expect("CLI update");
    assert_eq!(scheduler.loop_generation(&dir), generation);
    let state = crate::config::loops::read_loop_state(&dir).expect("state");
    assert_eq!(state.pending_run_id, None, "reset to a baseline");
    assert_ne!(read_raw_loop_state(&dir).expect("raw"), Some(failed));

    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(!has_record(&scheduler, &dir));
    assert!(audit_rows(&dir)
        .iter()
        .all(|row| row.run_id != entry.run_id && row.started_at != entry.started_at));
}

#[tokio::test]
async fn issue_2698_record_is_dropped_after_a_config_only_edit() {
    let (tmp, dir, run_id) = issue_2698_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    let failed = failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
    let entry = recorded_entry(&scheduler, &dir);
    let generation = scheduler.loop_generation(&dir);

    edit_config(&dir, |c| c.prompt.body = "edited".to_string());
    assert_eq!(scheduler.loop_generation(&dir), generation);
    assert_eq!(read_raw_loop_state(&dir).expect("raw"), Some(failed));
    assert!(audit_rows(&dir).iter().all(|row| row.run_id != run_id));

    let t2 = Utc::now();
    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(!has_record(&scheduler, &dir));
    assert!(
        !gate_is_untaken(&scheduler),
        "the pending run is delivered anew"
    );
    let rows = audit_rows(&dir);
    let delivered: Vec<_> = rows
        .iter()
        .filter(|row| row.kind == LoopAuditKind::Delivered && row.run_id == run_id)
        .collect();
    assert_eq!(delivered.len(), 1, "{rows:?}");
    assert!(delivered[0].started_at >= t2);
    assert_ne!(delivered[0].started_at, entry.started_at);
    assert!(rows.iter().all(|row| row.started_at != entry.started_at));
}

#[tokio::test]
async fn issue_2698_audit_failure_after_state_write_is_replayed() {
    let (tmp, dir, run_id) = issue_2698_fixture();
    let app = audit_writer_app();
    let scheduler = LoopScheduler::new();
    let audit_path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
    std::fs::create_dir(&audit_path).expect("audit.jsonl as a directory");

    install_report_gate(&scheduler, delivered_report());
    let err = scheduler
        .scan_loop(
            &SchedulerContext::Native(Arc::new(app.handle().clone())),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect_err("the audit append fails");
    assert!(err.contains("Failed to read"), "{err}");
    let state = crate::config::loops::read_loop_state(&dir).expect("state");
    assert_eq!(state.pending_run_id, None, "the state was written");
    assert!(has_record(&scheduler, &dir));

    std::fs::remove_dir(&audit_path).expect("remove audit dir");
    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(gate_is_untaken(&scheduler), "the second scan re-delivered");
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, LoopAuditKind::Delivered);
    assert_eq!(rows[0].run_id, run_id);
    assert!(!has_record(&scheduler, &dir));

    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("third scan");
    assert!(gate_is_untaken(&scheduler));
    assert_eq!(audit_rows(&dir).len(), 1);
}

#[tokio::test]
async fn issue_2698_checked_commit_is_stale_when_config_changed_under_lock() {
    let (_tmp, dir, _) = issue_2698_fixture();
    let scheduler = LoopScheduler::new();
    let old_config = read_loop_config(&dir).expect("config");
    let (state, raw) = read_state_snapshot(&dir).expect("snapshot");
    let generation = scheduler.loop_generation(&dir);
    edit_config(&dir, |c| c.prompt.body = "edited".to_string());

    let checked = scheduler.commit_scan_section_checked(
        &dir,
        generation,
        raw.as_deref(),
        None,
        &state,
        Some(&old_config),
    );
    assert_eq!(checked, Ok(LoopStateWrite::Stale));
    assert_eq!(read_raw_loop_state(&dir).expect("raw"), raw);

    let unchecked =
        scheduler.commit_scan_section_checked(&dir, generation, raw.as_deref(), None, &state, None);
    assert_eq!(unchecked, Ok(LoopStateWrite::Written));
}

#[tokio::test]
async fn issue_2698_non_delivered_failure_keeps_old_behavior() {
    let (tmp, dir, _) = issue_2698_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    let failed = LoopDeliveryReport {
        kind: LoopAuditKind::DeliveryFailed,
        message: "gated failure".to_string(),
        error: Some("gated failure".to_string()),
        ..delivered_report()
    };
    let err = scan_with_the_loop_lock_held(&scheduler, &app, tmp.path(), &dir, failed)
        .await
        .expect_err("the commit fails on the held Loop lock");
    assert!(err.contains("loopLockTimeout"), "{err}");
    assert!(!has_record(&scheduler, &dir));

    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(!gate_is_untaken(&scheduler), "the run is delivered again");
}

async fn issue_2698_cli_toggle_case(scan_between: bool) {
    let (tmp, ac_root, dir, run_id) = issue_2698_cli_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
    let toggle = |enabled| {
        crate::config::loops::update_loop_files(
            tmp.path(),
            &ac_root,
            "daily-sync",
            crate::config::loops::LoopUpdatePatch {
                enabled: Some(enabled),
                ..Default::default()
            },
        )
        .expect("CLI toggle")
    };
    toggle(false);
    if scan_between {
        scheduler
            .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
            .await
            .expect("disabled scan");
        assert!(!has_record(&scheduler, &dir), "a disabled scan drops it");
    }
    toggle(true);
    let reenabled = crate::config::loops::read_loop_state(&dir).expect("state");

    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(!has_record(&scheduler, &dir));
    assert_old_run_not_recorded(&dir, run_id);
    let state = crate::config::loops::read_loop_state(&dir).expect("state");
    assert_eq!(
        (state.last_checked_at, state.next_due_at),
        (reenabled.last_checked_at, reenabled.next_due_at)
    );
}

#[tokio::test]
async fn issue_2698_record_is_dropped_after_cli_disable_and_reenable() {
    issue_2698_cli_toggle_case(true).await;
}

#[tokio::test]
async fn issue_2698_record_is_dropped_after_cli_disable_and_reenable_without_a_scan() {
    issue_2698_cli_toggle_case(false).await;
}

#[tokio::test]
async fn issue_2698_record_is_dropped_after_prompt_edit_away_and_back() {
    let (tmp, ac_root, dir, run_id) = issue_2698_cli_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;
    let recorded = scheduler.unrecorded_deliveries.lock().expect("records")[&generation_key(&dir)]
        .config
        .clone();
    let generation = scheduler.loop_generation(&dir);
    for body in ["edited".to_string(), recorded.prompt.body.clone()] {
        crate::config::loops::update_loop_files(
            tmp.path(),
            &ac_root,
            "daily-sync",
            crate::config::loops::LoopUpdatePatch {
                prompt_body: Some(body),
                ..Default::default()
            },
        )
        .expect("CLI update");
    }
    assert!(loop_delivery_config_matches(
        &read_loop_config(&dir).expect("config"),
        &recorded
    ));
    assert_eq!(scheduler.loop_generation(&dir), generation);
    let reset = crate::config::loops::read_loop_state(&dir).expect("state");

    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("second scan");
    assert!(!has_record(&scheduler, &dir));
    assert_old_run_not_recorded(&dir, run_id);
    let state = crate::config::loops::read_loop_state(&dir).expect("state");
    assert_eq!(
        (state.last_checked_at, state.next_due_at),
        (reset.last_checked_at, reset.next_due_at)
    );
}

/// Rework 1/2 - a replay cancelled while it waits for `io_lock` keeps
/// its record: the next scan still replays and does not deliver again.
/// The replay gate makes the order deterministic: the test takes
/// `io_lock` while the scan is parked in the gate, and `at_lock` arrives
/// only once the scan's task yields, which (current-thread runtime) is
/// at the `io_lock` wait, since no other await follows the gate.
#[tokio::test]
async fn issue_2698_cancelled_replay_keeps_the_record() {
    assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread
    );
    let (tmp, dir, run_id) = issue_2698_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    failed_first_scan(&scheduler, &app, tmp.path(), &dir, run_id).await;

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (at_lock_tx, at_lock_rx) = tokio::sync::oneshot::channel();
    scheduler.install_replay_gate(LoopReplayGate {
        reached: reached_tx,
        release: release_rx,
        at_lock: at_lock_tx,
    });
    let scan = {
        let scheduler = Arc::clone(&scheduler);
        let app = app.handle().clone();
        let (project, dir) = (tmp.path().to_path_buf(), dir.clone());
        tokio::spawn(async move {
            scheduler
                .scan_loop(
                    &SchedulerContext::Native(Arc::new(app)),
                    &project,
                    &dir,
                    false,
                    false,
                )
                .await
        })
    };
    reached_rx.await.expect("the scan reached the replay");
    let held = scheduler.io_lock.lock().await;
    release_tx.send(()).expect("release the replay gate");
    at_lock_rx.await.expect("the replay waits for io_lock");
    assert!(!scan.is_finished(), "the replay is parked on io_lock");
    scan.abort();
    assert!(scan.await.expect_err("aborted").is_cancelled());
    drop(held);
    assert!(
        has_record(&scheduler, &dir),
        "a cancelled replay lost its record"
    );

    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_project_under_scan_lock(app.handle().clone(), tmp.path().to_path_buf())
        .await
        .expect("next scan");
    assert!(gate_is_untaken(&scheduler), "the next scan re-delivered");
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].run_id, run_id);
    assert!(!has_record(&scheduler, &dir));
}

// #2733 - unresolved-target transitions.
