use super::*;

fn issue_2733_due_config() -> LoopConfigToml {
    let mut config = sample_config();
    config.trigger.expr = "* * * * *".to_string();
    config
}

fn issue_2733_make_due(dir: &Path) {
    let mut state = crate::config::loops::read_loop_state(dir).unwrap_or_default();
    state.last_checked_at = Some(Utc::now() - chrono::Duration::minutes(5));
    write_loop_state_atomic(dir, &state).expect("write due state");
}

fn issue_2733_state(dir: &Path) -> LoopState {
    crate::config::loops::read_loop_state(dir).expect("read state")
}

fn issue_2733_last_kind(dir: &Path) -> Option<String> {
    issue_2733_state(dir).last_result.map(|r| r.kind)
}

fn issue_2733_listen(app: &tauri::App) -> Arc<std::sync::Mutex<Vec<String>>> {
    use tauri::Listener;
    let kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&kinds);
    app.listen_any("loop_event", move |event| {
        let payload: serde_json::Value =
            serde_json::from_str(event.payload()).unwrap_or(serde_json::Value::Null);
        if let Some(kind) = payload["kind"].as_str() {
            sink.lock().expect("events").push(kind.to_string());
        }
    });
    kinds
}

fn issue_2733_count(kinds: &Arc<std::sync::Mutex<Vec<String>>>, kind: &str) -> usize {
    kinds
        .lock()
        .expect("events")
        .iter()
        .filter(|k| *k == kind)
        .count()
}

fn issue_2733_create_room(project: &Path) {
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let matrix = ac_root.join("_agent_tech-lead");
    let replica = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    for dir in [&team_dir, &matrix, &replica] {
        std::fs::create_dir_all(dir).expect("create room dir");
    }
    std::fs::write(matrix.join("Role.md"), "# Tech Lead\n").expect("role");
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["_agent_tech-lead"],"coordinator":"_agent_tech-lead","repos":[]}"#,
    )
    .expect("team config");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .expect("replica config");
}

/// T-B2 + T-B6 - one audit row and one event per transition, none on repeats.
#[tokio::test]
async fn issue_2733_unresolved_tick_notifies_once() {
    let config = issue_2733_due_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    issue_2733_make_due(&dir);
    let app = audit_writer_app();
    let kinds = issue_2733_listen(&app);
    let scheduler = LoopScheduler::new();

    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("scan 1");
    let after_1 = issue_2733_state(&dir);
    assert_eq!(
        after_1.last_result.as_ref().map(|r| r.kind.as_str()),
        Some("targetUnresolved")
    );
    assert!(after_1.next_due_at.expect("next due") > Utc::now());
    assert_eq!(after_1.pending_due_at, None);
    assert_eq!(after_1.pending_run_id, None);
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, LoopAuditKind::TargetUnresolved);
    assert_eq!(issue_2733_count(&kinds, "unresolved"), 1);

    issue_2733_make_due(&dir);
    let forced_check = issue_2733_state(&dir)
        .last_checked_at
        .expect("forced check");
    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("scan 2");
    let after_2 = issue_2733_state(&dir);
    assert_eq!(audit_rows(&dir).len(), 1, "audit rows still 1");
    assert_eq!(issue_2733_count(&kinds, "unresolved"), 1, "events still 1");
    assert!(after_2.last_checked_at.expect("checked") > forced_check);
    assert!(after_2.next_due_at.expect("next due") > Utc::now());
    assert_eq!(
        issue_2733_last_kind(&dir).as_deref(),
        Some("targetUnresolved")
    );
    assert!(!has_record(&scheduler, &dir));
}

/// T-B3 - unresolved ticks never write the synced config.
#[tokio::test]
async fn issue_2733_unresolved_ticks_never_touch_config() {
    let config = issue_2733_due_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    let config_path = dir.join(crate::config::loops::LOOP_CONFIG_FILE);
    let before = std::fs::read(&config_path).expect("config bytes");
    let app = audit_writer_app();
    let scheduler = LoopScheduler::new();

    for _ in 0..2 {
        issue_2733_make_due(&dir);
        scheduler
            .scan_loop(
                &SchedulerContext::Native(app.handle().clone()),
                tmp.path(),
                &dir,
                false,
                false,
            )
            .await
            .expect("scan");
    }

    assert_eq!(std::fs::read(&config_path).expect("config bytes"), before);
    assert!(read_loop_config(&dir).expect("config").loop_def.enabled);
}

/// T-B4 - a pending run whose room is missing is cleared.
#[tokio::test]
async fn issue_2733_pending_run_with_missing_room_is_cleared() {
    let (tmp, dir) = pending_loop_fixture();
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
        .expect("scan");

    let state = issue_2733_state(&dir);
    assert_eq!(state.pending_due_at, None);
    assert_eq!(state.pending_run_id, None);
    assert_eq!(
        issue_2733_last_kind(&dir).as_deref(),
        Some("targetUnresolved")
    );
}

/// T-B5 - dedupe keys on the previous kind, not on "ever seen".
#[tokio::test]
async fn issue_2733_transition_again_after_another_result() {
    let config = issue_2733_due_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    let state = LoopState {
        last_checked_at: Some(Utc::now() - chrono::Duration::minutes(5)),
        last_result: Some(LoopLastResult {
            kind: "delivered".to_string(),
            message: "earlier".to_string(),
        }),
        ..LoopState::default()
    };
    write_loop_state_atomic(&dir, &state).expect("write state");
    let app = audit_writer_app();
    let kinds = issue_2733_listen(&app);

    LoopScheduler::new()
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("scan");

    assert_eq!(audit_rows(&dir).len(), 1);
    assert_eq!(issue_2733_count(&kinds, "unresolved"), 1);
}

/// T-B8 - scheduler state machine: unresolved, room appears and the
/// (gated) delivery is recorded, then loss notifies again.
#[tokio::test]
async fn issue_2733_recovers_when_room_appears() {
    let config = issue_2733_due_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    issue_2733_make_due(&dir);
    let app = audit_writer_app();
    let kinds = issue_2733_listen(&app);
    let scheduler = LoopScheduler::new();

    // 1. Room absent, real delivery.
    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("scan 1");
    assert_eq!(
        issue_2733_last_kind(&dir).as_deref(),
        Some("targetUnresolved")
    );
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, LoopAuditKind::TargetUnresolved);
    assert_eq!(issue_2733_count(&kinds, "unresolved"), 1);
    let check_1 = issue_2733_state(&dir).last_checked_at.expect("check 1");

    // 2. The room appears.
    issue_2733_create_room(tmp.path());
    assert!(resolve_loop_target(tmp.path(), &config).is_ok());

    // 3. Due again, delivery gated to a Delivered report.
    issue_2733_make_due(&dir);
    install_report_gate(&scheduler, delivered_report());
    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("scan 2");

    // 4.
    assert!(!gate_is_untaken(&scheduler), "delivery was attempted");
    assert_eq!(issue_2733_last_kind(&dir).as_deref(), Some("delivered"));
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].kind, LoopAuditKind::Delivered);
    assert_eq!(issue_2733_count(&kinds, "delivered"), 1);
    assert_eq!(kinds.lock().expect("events").len(), 2);
    let after_2 = issue_2733_state(&dir);
    let next_due = after_2.next_due_at.expect("next due");
    assert!(next_due > Utc::now());
    assert!(next_due > check_1);
    assert_eq!(after_2.pending_due_at, None);
    assert_eq!(after_2.pending_run_id, None);
    assert!(!has_record(&scheduler, &dir));

    // 5. Loss after recovery notifies again.
    std::fs::remove_dir_all(tmp.path().join(".ac").join("wg-1-dev-team")).expect("remove room");
    issue_2733_make_due(&dir);
    scheduler
        .scan_loop(
            &SchedulerContext::Native(app.handle().clone()),
            tmp.path(),
            &dir,
            false,
            false,
        )
        .await
        .expect("scan 3");
    let rows = audit_rows(&dir);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].kind, LoopAuditKind::TargetUnresolved);
    assert_eq!(issue_2733_count(&kinds, "unresolved"), 2);
}
