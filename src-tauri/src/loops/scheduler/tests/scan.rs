use super::*;

#[tokio::test]
async fn e2992_p43_invalid_no_candidate_action() {
    use crate::config::settings::{
        AppSettings, SettingsAvailability, SettingsOwner, SettingsPublisher,
    };

    let (project, dir) = pending_loop_fixture();
    let instance = tempfile::tempdir().unwrap();
    let path = instance.path().join("settings.json");
    let settings = AppSettings {
        project_paths: vec![project.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::ZERO).unwrap();
    let transitions = Arc::new(std::sync::Mutex::new(Vec::new()));
    let context = SchedulerContext::Headless {
        owner: Arc::clone(&owner),
        transitions: Arc::clone(&transitions),
    };
    let scheduler = Arc::new(LoopScheduler::new());
    assert!(!active_project_candidates_for_owner(&owner).await.is_empty());
    let pending = std::fs::read(dir.join(LOOP_STATE_FILE)).unwrap();
    install_report_gate(&scheduler, delivered_report());
    scheduler.scan_once(&context, false, false).await;
    assert!(
        !gate_is_untaken(&scheduler),
        "Ready delivers the eligible pending run"
    );
    let ready_transitions = {
        let recorded = transitions.lock().expect("scheduler transitions");
        assert!(!recorded.is_empty(), "Ready emits a real transition");
        let config = read_loop_config(&dir).unwrap();
        assert!(recorded
            .iter()
            .any(|(project_dir, loop_dir, loop_id, kind, summary, message)| {
                std::fs::canonicalize(project_dir).expect("recorded project exists")
                    == std::fs::canonicalize(project.path()).expect("fixture project exists")
                    && std::fs::canonicalize(loop_dir).expect("recorded loop exists")
                        == std::fs::canonicalize(&dir).expect("fixture loop exists")
                    && loop_id == &config.loop_def.id
                    && kind == "delivered"
                    && summary.is_some()
                    && message.as_deref() == Some("gated delivery")
            }));
        recorded.len()
    };
    // Keep the same eligible work pending for the Invalid negative control.
    std::fs::write(dir.join(LOOP_STATE_FILE), &pending).unwrap();

    let report = owner
        .with_operation("p43-invalid", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |path| {
                std::fs::write(path, b"{").map_err(|error| error.to_string())
            })
        })
        .await
        .unwrap();
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    let before = std::fs::read(dir.join(LOOP_STATE_FILE)).unwrap();
    install_report_gate(&scheduler, delivered_report());
    for _ in 0..2 {
        scheduler.scan_once(&context, false, false).await;
        assert!(
            gate_is_untaken(&scheduler),
            "Invalid never reaches delivery"
        );
        assert_eq!(std::fs::read(dir.join(LOOP_STATE_FILE)).unwrap(), before);
        assert_eq!(
            transitions.lock().expect("scheduler transitions").len(),
            ready_transitions
        );
    }
    assert!(active_project_candidates_for_owner(&owner).await.is_empty());
    assert_eq!(std::fs::read(dir.join(LOOP_STATE_FILE)).unwrap(), before);
    let shutdown = ShutdownSignal::new();
    let completed = scheduler
        .completed_scans
        .load(std::sync::atomic::Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    Arc::clone(&scheduler).start_with_context(context.clone(), shutdown.clone());
    // Observe the actual task's startup scan and first interval tick.
    tokio::time::timeout(Duration::from_secs(5), async {
        while scheduler
            .completed_scans
            .load(std::sync::atomic::Ordering::SeqCst)
            < completed + 2
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("Invalid preserves startup and interval scans");
    scheduler.request_scan();
    tokio::time::timeout(Duration::from_secs(5), async {
        while scheduler
            .completed_scans
            .load(std::sync::atomic::Ordering::SeqCst)
            < completed + 3
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("Invalid preserves requested scans");
    tokio::time::timeout(Duration::from_secs(35), async {
        while scheduler
            .completed_scans
            .load(std::sync::atomic::Ordering::SeqCst)
            < completed + 4
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Invalid preserves the next 30-second interval tick");
    assert!(started.elapsed() >= Duration::from_secs(30));
    shutdown.trigger();
    // The task owns the only other scheduler Arc. Its release proves exit,
    // unlike waiting on the token we just cancelled ourselves.
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&scheduler) != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("scheduler task exits after cancellation");
    assert_eq!(std::fs::read(dir.join(LOOP_STATE_FILE)).unwrap(), before);
    assert!(gate_is_untaken(&scheduler));
    assert_eq!(
        transitions.lock().expect("scheduler transitions").len(),
        ready_transitions
    );
    assert!(
        scheduler.scan_lock.try_lock().is_ok(),
        "scans release their lock"
    );
}

#[test]
fn retain_unarchived_candidates_drops_a_candidate_under_an_archived_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let active = temp.path().join("active");
    std::fs::create_dir_all(&archived).expect("archived");
    std::fs::create_dir_all(&active).expect("active");
    let roots =
        sessions_persistence::normalize_project_roots(&[archived.to_string_lossy().to_string()]);
    let archived_candidate = ProjectResolution {
        path: archived.clone(),
        folder_name: "archived".to_string(),
        registered: true,
    };
    let active_candidate = ProjectResolution {
        path: active.clone(),
        folder_name: "active".to_string(),
        registered: true,
    };

    let filtered =
        retain_unarchived_candidates(vec![archived_candidate, active_candidate.clone()], &roots);

    assert_eq!(filtered, vec![active_candidate]);
}

#[test]
fn retain_unarchived_candidates_returns_input_unchanged_when_archived_list_is_empty() {
    let candidate = ProjectResolution {
        path: PathBuf::from("Z:/does/not/exist"),
        folder_name: "missing".to_string(),
        registered: true,
    };
    let input = vec![candidate.clone()];
    let ptr = input.as_ptr();
    let capacity = input.capacity();

    let filtered = retain_unarchived_candidates(input, &[]);

    assert_eq!(filtered, vec![candidate]);
    assert_eq!(
        filtered.as_ptr(),
        ptr,
        "empty archived roots must skip the into_iter/collect round trip"
    );
    assert_eq!(filtered.capacity(), capacity);
}

#[test]
fn unresolved_targets_in_project_reports_a_missing_room() {
    let config = sample_config();
    let tmp = project_with_loop(&config);

    let alerts = unresolved_targets_in_project(tmp.path());

    assert_eq!(alerts.len(), 1, "one unresolved Loop target");
    let alert = &alerts[0];
    assert_eq!(alert.loop_id, config.loop_def.id);
    assert_eq!(alert.loop_name, config.loop_def.name);
    assert_eq!(alert.workgroup, config.target.workgroup);
    assert!(
        alert.error.starts_with("Room '") && alert.error.contains("not found in project"),
        "unexpected error: {}",
        alert.error
    );
}

#[test]
fn unresolved_targets_in_project_skips_disabled_loops() {
    let mut config = sample_config();
    config.loop_def.enabled = false;
    let tmp = project_with_loop(&config);

    assert!(unresolved_targets_in_project(tmp.path()).is_empty());
}

#[test]
fn unresolved_targets_in_project_reports_a_room_without_an_orchestrator() {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    std::fs::create_dir_all(tmp.path().join(".ac").join(&config.target.workgroup))
        .expect("create room dir");

    let alerts = unresolved_targets_in_project(tmp.path());

    assert_eq!(alerts.len(), 1);
    assert!(
        alerts[0]
            .error
            .contains("has no identity-verified orchestrator"),
        "unexpected error: {}",
        alerts[0].error
    );
}

#[test]
fn unresolved_targets_in_project_returns_empty_without_an_ac_root() {
    let tmp = tempfile::tempdir().expect("tempdir");

    assert!(unresolved_targets_in_project(tmp.path()).is_empty());
}

#[test]
fn unresolved_targets_in_project_orders_by_loop_dir() {
    let mut first = sample_config();
    first.loop_def.id = "aaa-sync".to_string();
    let mut second = sample_config();
    second.loop_def.id = "zzz-sync".to_string();

    let tmp = project_with_loop(&second);
    let ac_root = tmp.path().join(".ac");
    write_loop_config(&ac_root, &first).expect("write first config");

    let alerts = unresolved_targets_in_project(tmp.path());

    assert_eq!(
        alerts
            .iter()
            .map(|a| a.loop_id.as_str())
            .collect::<Vec<_>>(),
        vec!["aaa-sync", "zzz-sync"]
    );
}

#[test]
fn lib_registers_list_unresolved_loop_targets() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read lib.rs");

    assert!(
        source.contains("commands::loops::list_unresolved_loop_targets"),
        "lib.rs must register list_unresolved_loop_targets"
    );
}

#[test]
fn scan_once_calls_archived_candidate_filter() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/loops/scheduler.rs"
    ))
    .expect("read scheduler.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production scheduler source");
    let scan = production
        .split("    async fn scan_once(")
        .nth(1)
        .expect("scan_once definition")
        .split("    pub(crate) async fn scan_project_under_scan_lock(")
        .next()
        .expect("scan_once body");
    assert!(scan.contains("active_project_candidates_for_owner(&owner).await"));
    let wrapper = production
        .split("async fn active_project_candidates(app: &AppHandle) -> Vec<ProjectResolution> {")
        .nth(1)
        .expect("active_project_candidates definition")
        .split("async fn active_project_candidates_for_owner(")
        .next()
        .expect("active_project_candidates body");
    assert!(wrapper.contains("active_project_candidates_for_owner(&owner).await"));
    let state = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/loops/scheduler/state.rs"
    ))
    .expect("read scheduler/state.rs");
    let candidates = state
        .find(concat!(
            "async fn active_project_candidates_for_owner(\n",
            "    owner: &SettingsState,\n",
            ") -> Vec<ProjectResolution> {"
        ))
        .expect("active_project_candidates_for_owner definition");
    let body = state[candidates..]
        .split("fn unresolved_targets_in_project(")
        .next()
        .expect("active_project_candidates_for_owner body");
    let normalize = body
        .find("let archived_roots = sessions_persistence::normalize_project_roots(&archived);")
        .expect("archived root normalization");
    let retain = body
        .find("retain_unarchived_candidates(projects, &archived_roots)")
        .expect("retain_unarchived_candidates call");

    assert!(
        normalize < retain,
        "active_project_candidates_for_owner must subtract archived candidates after normalizing \
            archived roots"
    );
}

#[test]
fn revalidate_loop_current_detects_deleted_disabled_and_changed_configs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = sample_config();
    let dir = write_loop_config(tmp.path(), &config).expect("write config");

    assert_eq!(
        revalidate_loop_current(&dir, &config).expect("current"),
        LoopConfigRevalidation::Current
    );

    let mut renamed = config.clone();
    renamed.loop_def.name = "Renamed sync".to_string();
    write_loop_config(tmp.path(), &renamed).expect("write renamed config");
    assert_eq!(
        revalidate_loop_current(&dir, &config).expect("renamed"),
        LoopConfigRevalidation::Current
    );

    let mut retargeted = config.clone();
    retargeted.target.workgroup = "wg-2-dev-team".to_string();
    write_loop_config(tmp.path(), &retargeted).expect("write retargeted config");
    assert_eq!(
        revalidate_loop_current(&dir, &config).expect("retargeted"),
        LoopConfigRevalidation::Changed
    );

    let mut disabled = config.clone();
    disabled.loop_def.enabled = false;
    write_loop_config(tmp.path(), &disabled).expect("write disabled config");
    assert_eq!(
        revalidate_loop_current(&dir, &config).expect("disabled"),
        LoopConfigRevalidation::Disabled
    );

    std::fs::remove_dir_all(&dir).expect("remove loop dir");
    assert_eq!(
        revalidate_loop_current(&dir, &config).expect("gone"),
        LoopConfigRevalidation::Gone
    );
}

/// AC-17 test 1 - the coalesced-pending writer records the run's real
/// `sessionStart`. Every config field is set BEFORE `write_loop_config`:
/// the writer revalidates against the file first, so a later edit would
/// read `Changed` and no audit row would be written at all.
#[tokio::test]
async fn records_session_start_on_a_coalesced_pending_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("project");
    let ac_root = project_dir.join(".ac");
    let mut config = sample_config();
    config.policy.session_start = crate::config::loops::LoopSessionStart::Accumulate;
    config.trigger.expr = "* * * * *".to_string();
    let dir = write_loop_config(&ac_root, &config).expect("write config");

    let app = audit_writer_app();
    let scheduler = LoopScheduler::new();
    let mut state = LoopState {
        pending_due_at: Some(Utc::now() - chrono::Duration::minutes(30)),
        pending_run_id: Some(Uuid::new_v4()),
        ..LoopState::default()
    };

    scheduler
        .maybe_coalesce_pending(
            &SchedulerContext::Native(app.handle().clone()),
            &project_dir,
            &dir,
            &config,
            &mut state,
            0,
            None,
        )
        .await
        .expect("coalesce pending");

    let content = std::fs::read_to_string(dir.join(crate::config::loops::LOOP_AUDIT_FILE))
        .expect("audit read");
    let last = content
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("an audit row");
    let row: serde_json::Value = serde_json::from_str(last).expect("audit row as value");
    assert_eq!(row["kind"], serde_json::json!("coalescedPending"));
    assert_eq!(row["sessionStart"], serde_json::json!("accumulate"));
}

/// AC-17 test 2 - the missed-while-closed writer records the real value.
#[tokio::test]
async fn records_session_start_on_a_missed_while_closed_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("project");
    let ac_root = project_dir.join(".ac");
    let mut config = sample_config();
    config.policy.session_start = crate::config::loops::LoopSessionStart::Accumulate;
    let dir = write_loop_config(&ac_root, &config).expect("write config");

    let app = audit_writer_app();
    let scheduler = LoopScheduler::new();

    scheduler
        .record_missed_while_closed(
            &SchedulerContext::Native(app.handle().clone()),
            &project_dir,
            &dir,
            &config,
            LoopState::default(),
            Uuid::new_v4(),
            Utc::now() - chrono::Duration::hours(1),
            0,
            None,
        )
        .await
        .expect("record missed while closed");

    let content = std::fs::read_to_string(dir.join(crate::config::loops::LOOP_AUDIT_FILE))
        .expect("audit read");
    let last = content
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("an audit row");
    let row: serde_json::Value = serde_json::from_str(last).expect("audit row as value");
    assert_eq!(row["kind"], serde_json::json!("missedWhileClosed"));
    assert_eq!(row["sessionStart"], serde_json::json!("accumulate"));
}

/// AC-17 test 3 - the delivery-report writer records the real value. All
/// seven `LoopDeliveryReport` fields are spelled because the struct has no
/// `Default`; `completed_at: None` is deliberate.
#[tokio::test]
async fn records_session_start_on_a_delivery_report() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_dir = tmp.path().join("project");
    let ac_root = project_dir.join(".ac");
    let mut config = sample_config();
    config.policy.session_start = crate::config::loops::LoopSessionStart::Accumulate;
    let dir = write_loop_config(&ac_root, &config).expect("write config");

    let app = audit_writer_app();
    let scheduler = LoopScheduler::new();
    let now = Utc::now();

    scheduler
        .apply_delivery_report(
            &SchedulerContext::Native(app.handle().clone()),
            &project_dir,
            &dir,
            &config,
            LoopState::default(),
            LoopDeliveryReport {
                kind: LoopAuditKind::Delivered,
                message: "delivered".to_string(),
                target: None,
                session_id: None,
                error: None,
                prompt_snapshot: None,
                completed_at: None,
            },
            Uuid::new_v4(),
            now,
            now,
            0,
            None,
        )
        .await
        .expect("apply delivery report");

    let content = std::fs::read_to_string(dir.join(crate::config::loops::LOOP_AUDIT_FILE))
        .expect("audit read");
    let last = content
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("an audit row");
    let row: serde_json::Value = serde_json::from_str(last).expect("audit row as value");
    assert_eq!(row["kind"], serde_json::json!("delivered"));
    assert_eq!(row["sessionStart"], serde_json::json!("accumulate"));
}

fn tmp_files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read loop dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

/// #2695 round 2 - a non-UTF-8 `state.json` is unreadable state, not a
/// scan failure: the scan replaces it with a baseline, as before #2695.
#[tokio::test]
async fn scan_heals_a_non_utf8_state_file() {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    let state_path = dir.join(LOOP_STATE_FILE);
    std::fs::write(&state_path, [0xff, 0xfe, b'{']).expect("write invalid state");
    let app = audit_writer_app();

    LoopScheduler::new()
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("an unreadable state is not a scan failure");

    let state = crate::config::loops::read_loop_state(&dir).expect("healed state parses");
    assert!(state.last_checked_at.is_some(), "a baseline replaced it");
}

/// #2695 round 3 - the non-UTF-8 arm of `guarded_state_write` is still a
/// compare-and-swap: different invalid bytes on disk mean `Stale`.
#[test]
fn guarded_state_write_is_stale_when_non_utf8_bytes_changed() {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    let state_path = dir.join(LOOP_STATE_FILE);
    std::fs::write(&state_path, [0xff, 0xfe, b'A']).expect("write bytes A");
    let (_, s0_raw) = read_state_snapshot(&dir).expect("snapshot");
    let changed = [0xff, 0xfe, b'B'];
    std::fs::write(&state_path, changed).expect("write bytes B");

    let result = guarded_state_write(&dir, &LoopState::default(), s0_raw.as_deref());

    assert_eq!(result, Ok(LoopStateWrite::Stale));
    assert_eq!(std::fs::read(&state_path).expect("state bytes"), changed);
}

/// #2695 T3 - the scan lock was split, not deleted: a second scan cannot
/// start while the first is held inside a delivery.
#[tokio::test]
async fn a_second_scan_waits_while_the_first_is_inside_a_delivery() {
    let (tmp, dir) = pending_loop_fixture();
    let app = audit_writer_app();
    let scheduler = Arc::new(LoopScheduler::new());
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    scheduler.install_delivery_gate(LoopDeliveryGate {
        entered: Some(entered_tx),
        release: Some(release_rx),
        report: Some(delivered_report()),
    });
    let spawn_scan = || {
        let scheduler = Arc::clone(&scheduler);
        let app = app.handle().clone();
        let project = tmp.path().to_path_buf();
        tokio::spawn(async move { scheduler.scan_project_under_scan_lock(app, project).await })
    };
    let first = spawn_scan();
    entered_rx.await.expect("first scan entered the delivery");

    let mut second = spawn_scan();
    let started = std::time::Instant::now();
    let blocked = tokio::time::timeout(Duration::from_millis(200), &mut second).await;
    eprintln!(
        "T3: second scan still blocked after {:?}",
        started.elapsed()
    );
    assert!(blocked.is_err(), "the second scan must wait on scan_lock");

    release_tx.send(()).expect("release the gate");
    first.await.expect("join first").expect("first scan");
    second.await.expect("join second").expect("second scan");
    let state = crate::config::loops::read_loop_state(&dir).expect("state");
    assert!(
        state.pending_run_id.is_none(),
        "the first scan applied its report"
    );
}

/// #2695 T7 - a Loop gone at entry is a quiet skip with no write.
#[tokio::test]
async fn scan_loop_skips_a_missing_dir_or_config_quietly() {
    let app = audit_writer_app();
    let scheduler = LoopScheduler::new();
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let ac_root = tmp.path().join(".ac");

    let missing = loop_dir(&ac_root, "gone");
    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &missing,
            false,
            false,
        )
        .await
        .expect("a missing directory is Ok");
    assert!(!missing.exists(), "nothing recreated the missing directory");

    let dir = loop_dir(&ac_root, &config.loop_def.id);
    std::fs::remove_file(dir.join(crate::config::loops::LOOP_CONFIG_FILE)).expect("remove config");
    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("a missing config is Ok");
    assert_eq!(
        std::fs::read_dir(&dir).expect("read loop dir").count(),
        0,
        "no write attempted"
    );
}

/// #2695 T12 - the write-time presence recheck, isolated.
#[test]
fn guarded_state_write_is_stale_when_the_config_is_gone() {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    crate::config::loops::write_loop_state_atomic(&dir, &LoopState::default())
        .expect("write state");
    std::fs::remove_file(dir.join(crate::config::loops::LOOP_CONFIG_FILE)).expect("remove config");
    let (_, raw) = read_loop_state_with_raw(&dir).expect("read state");
    let state = LoopState {
        last_checked_at: Some(Utc::now()),
        ..LoopState::default()
    };

    let result = guarded_state_write(&dir, &state, raw.as_deref().map(str::as_bytes));

    assert_eq!(result, Ok(LoopStateWrite::Stale));
    assert_eq!(
        read_raw_loop_state(&dir).expect("raw"),
        raw.map(String::into_bytes)
    );
    assert!(tmp_files_in(&dir).is_empty(), "no tmp file left");
}

/// #2695 T13 - the generation key resists a spelling alias.
#[test]
fn generation_key_resists_a_spelling_alias() {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir_all(ac_root.join("x")).expect("sibling");
    let dir = loop_dir(&ac_root, &config.loop_def.id);
    let dir_name = dir.file_name().expect("loop dir name");
    let aliased = ac_root.join("x").join("..").join(dir_name);

    assert_eq!(generation_key(&dir), generation_key(&aliased));
    #[cfg(windows)]
    assert_eq!(
        generation_key(&dir),
        generation_key(Path::new(&dir.to_string_lossy().to_uppercase()))
    );

    let scheduler = LoopScheduler::new();
    let before = scheduler.loop_generation(&dir);
    scheduler.bump_loop_generation(&aliased);
    assert_ne!(scheduler.loop_generation(&dir), before);
}

/// #2695 T15 - the commit order, isolated: with the config gone, the
/// state write refuses before any audit append is attempted.
#[test]
fn commit_scan_section_writes_nothing_when_the_config_is_gone() {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    crate::config::loops::write_loop_state_atomic(
        &dir,
        &LoopState {
            last_checked_at: Some(Utc::now() - chrono::Duration::hours(1)),
            ..LoopState::default()
        },
    )
    .expect("write state");
    let audit_path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
    std::fs::write(&audit_path, "{\"row\":1}\n").expect("write audit");
    std::fs::remove_file(dir.join(crate::config::loops::LOOP_CONFIG_FILE)).expect("remove config");

    let scheduler = LoopScheduler::new();
    let (_, s0_raw) = read_loop_state_with_raw(&dir).expect("read state");
    let s0_generation = scheduler.loop_generation(&dir);
    let now = Utc::now();
    let entry = LoopAuditEntry {
        run_id: Uuid::new_v4(),
        loop_id: config.loop_def.id.clone(),
        project_path: tmp.path().to_string_lossy().to_string(),
        kind: LoopAuditKind::Delivered,
        due_at: now,
        started_at: now,
        completed_at: Some(now),
        target: None,
        session_id: None,
        busy_coordinator_policy: config.policy.busy_coordinator.clone(),
        session_start: Some(config.policy.session_start),
        error: None,
        prompt_snapshot: None,
    };
    let state = LoopState {
        last_checked_at: Some(now),
        ..LoopState::default()
    };

    let result = scheduler.commit_scan_section(
        &dir,
        s0_generation,
        s0_raw.as_deref().map(str::as_bytes),
        Some(&entry),
        &state,
    );

    assert_eq!(result, Ok(LoopStateWrite::Stale));
    assert_eq!(
        read_raw_loop_state(&dir).expect("raw"),
        s0_raw.map(String::into_bytes)
    );
    assert_eq!(
        std::fs::read_to_string(&audit_path).expect("audit"),
        "{\"row\":1}\n"
    );
    assert!(tmp_files_in(&dir).is_empty(), "no tmp file left");
}

/// #2679 - a failed state write appends no audit row. Windows-only: the
/// failure is forced with an open read handle on `state.json`, which
/// blocks the replace with `os error 5` on Windows but not on Unix. The
/// ordering contract is cross-platform; a Linux green is not coverage.
#[cfg(windows)]
#[test]
fn issue_2679_a_failed_state_write_appends_no_audit_row() {
    let (tmp, dir) = pending_loop_fixture();
    let config = sample_config();
    let state_path = dir.join(LOOP_STATE_FILE);
    let audit_path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
    let scheduler = LoopScheduler::new();
    let now = Utc::now();
    let entry_for = |run_id| LoopAuditEntry {
        run_id,
        loop_id: config.loop_def.id.clone(),
        project_path: tmp.path().to_string_lossy().to_string(),
        kind: LoopAuditKind::Delivered,
        due_at: now,
        started_at: now,
        completed_at: Some(now),
        target: None,
        session_id: None,
        busy_coordinator_policy: config.policy.busy_coordinator.clone(),
        session_start: Some(config.policy.session_start),
        error: None,
        prompt_snapshot: None,
    };
    let state = LoopState {
        last_checked_at: Some(now),
        ..LoopState::default()
    };

    // Positive control: with no holder the same commit writes and audits.
    let s0_raw = read_raw_loop_state(&dir)
        .expect("raw")
        .expect("state exists");
    let s0_generation = scheduler.loop_generation(&dir);
    let control = scheduler.commit_scan_section(
        &dir,
        s0_generation,
        Some(&s0_raw),
        Some(&entry_for(Uuid::new_v4())),
        &state,
    );
    assert_eq!(control, Ok(LoopStateWrite::Written));
    assert!(audit_path.exists(), "control leg appends the audit row");
    std::fs::remove_file(&audit_path).expect("remove control audit");
    std::fs::write(&state_path, &s0_raw).expect("restore state");

    // Failure leg: hold state.json open past the replace-retry budget.
    let s0_raw = read_raw_loop_state(&dir)
        .expect("raw")
        .expect("state exists");
    let s0_generation = scheduler.loop_generation(&dir);
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .open(&state_path)
        .expect("holder");
    let result = scheduler.commit_scan_section(
        &dir,
        s0_generation,
        Some(&s0_raw),
        Some(&entry_for(Uuid::new_v4())),
        &state,
    );
    drop(holder);

    assert!(!audit_path.exists(), "no audit row without its state write");
    let err = result.expect_err("the held state write fails");
    assert!(err.starts_with("Failed to finalize Loop state"), "{err}");
    assert_eq!(read_raw_loop_state(&dir).expect("raw"), Some(s0_raw));
    assert!(tmp_files_in(&dir).is_empty(), "{:?}", tmp_files_in(&dir));
}
