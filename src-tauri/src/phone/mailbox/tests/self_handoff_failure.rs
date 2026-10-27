use super::*;
use crate::phone::mailbox::handoff::SELF_HANDOFF_FLAVOR_SWITCH;
use crate::phone::mailbox::self_handoff::failure::{
    consume_outcome, retry_notice, NoticeDelivery, SelfHandoffFailure, SelfHandoffOutcome,
    CONTINUATION_FAILURE,
};

fn failure_fixture(id: Uuid) -> SelfHandoffFailure {
    SelfHandoffFailure {
        action: "self-handoff-and-clear",
        attempt: Uuid::new_v4(),
        original: id,
        current: Some(id),
        root: PathBuf::from("root"),
        continuation: false,
        reason: "original failure".into(),
        forgotten: false,
    }
}

#[tokio::test]
async fn issue_3189_locked_persist_retains_and_reports() {
    let id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let matrix = project.join(".ac/_agent_dev-rust");
    let replica = project.join(".ac/room-1-test/__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(matrix.join("Role.md"), "# Test").unwrap();
    std::fs::write(
        replica.join("config.json"),
        serde_json::json!({
            "identity": "../../_agent_dev-rust",
            "tooling": { "currentCodingAgent": "codex", "profile": "A", "selectionLocked": true }
        })
        .to_string(),
    )
    .unwrap();
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into()],
        agents: vec![wake_agent("codex", "Codex", "codex")],
        ..AppSettings::default()
    };
    let reason = crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
        &settings, &replica, "codex", "B",
    )
    .unwrap_err();
    assert!(reason.contains("unlock it or use a reviewed force"));
    let notes = b"original handoff bytes\r\n";
    std::fs::write(replica.join("SELF-HANDOFF.md"), notes).unwrap();
    for locked in [true, false] {
        if !locked {
            let expected =
                crate::config::coding_agent_profiles::read_replica_selection_state(&replica)
                    .expectation()
                    .unwrap();
            crate::config::coding_agent_profiles::clear_replica_selection_lock(
                &settings, &replica, &expected,
            )
            .unwrap();
        }
        let settings = settings.clone();
        pending.0.lock().unwrap().insert(id);
        let reports = Arc::new(Mutex::new(Vec::new()));
        let seen = reports.clone();
        let clean = pending.clone();
        let driver = MailboxPoller::drive_self_switch_after_sustained_idle(
            id,
            replica.clone(),
            "codex".into(),
            "B".into(),
            None,
            SELF_HANDOFF_FLAVOR_SWITCH,
            pending.clone(),
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(1),
            |_| async { (true, true) },
            move |cwd, agent, profile| {
                let settings = settings.clone();
                async move {
                    crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                        &settings, &cwd, &agent, &profile,
                    )
                }
            },
            move |_, _, _| async move {
                assert!(!locked);
                Ok(new_id.to_string())
            },
            move |_, _| async move {
                assert!(!locked);
                Ok(())
            },
            move |_, _| async move {
                assert!(!locked);
            },
        );
        consume_outcome(driver, move |failure| async move {
            assert!(clean.0.lock().unwrap().is_empty());
            let _turn = tokio::time::timeout(
                Duration::from_secs(1),
                crate::session::selection::acquire_selection_operation_turn(),
            )
            .await
            .unwrap()
            .unwrap();
            seen.lock().unwrap().push(failure);
        })
        .await;
        let reports = reports.lock().unwrap();
        if locked {
            assert_eq!(reports.len(), 1);
            assert_eq!(reports[0].reason, reason);
            assert!(!reports[0].continuation);
            assert_eq!(reports[0].current, Some(id));
            assert_eq!(
                std::fs::read(replica.join("SELF-HANDOFF.md")).unwrap(),
                notes
            );
            assert!(!replica.join("self-clear").exists());
        } else {
            assert!(reports.is_empty());
            assert!(!replica.join("SELF-HANDOFF.md").exists());
            assert!(replica.join("self-clear").is_dir());
        }
    }
}

#[tokio::test]
async fn issue_3189_report_after_cleanup() {
    let id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.0.lock().unwrap().insert(id);
    let driver = MailboxPoller::drive_self_switch_after_sustained_idle(
        id,
        PathBuf::from("unused"),
        "codex".into(),
        "A".into(),
        None,
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending.clone(),
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(1),
        |_| async { (true, true) },
        |_, _, _| async { Err("locked".into()) },
        |_, _, _| async { panic!("restart on persist failure") },
        |_, _| async { panic!("inject on persist failure") },
        |_, _| async {},
    );
    consume_outcome(driver, |failure| async move {
        assert_eq!(failure.reason, "locked");
        assert!(pending.0.lock().unwrap().is_empty());
        let _turn = tokio::time::timeout(
            Duration::from_secs(1),
            crate::session::selection::acquire_selection_operation_turn(),
        )
        .await
        .unwrap()
        .unwrap();
    })
    .await;
}

#[tokio::test]
async fn issue_3189_clear_failure_reports_and_abandon_each_leg() {
    for mode in 0..5 {
        let id = Uuid::new_v4();
        let pending = Arc::new(crate::PendingSelfClear::default());
        pending.0.lock().unwrap().insert(id);
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("SELF-HANDOFF.md"), "notes").unwrap();
        let mut ticks = 0;
        let mut injections = 0;
        let outcome = MailboxPoller::drive_self_clear_after_sustained_idle(
            id,
            "/clear",
            root.path().into(),
            pending.clone(),
            None,
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(1),
            move |_| {
                ticks += 1;
                async move { (mode != 1 && !(mode == 2 && ticks > 1), true) }
            },
            move |_, _| {
                injections += 1;
                async move {
                    if mode == 0 || (mode == 3 && injections > 1) {
                        Err("write failed".into())
                    } else {
                        Ok(())
                    }
                }
            },
            |_, _| async {},
        )
        .await;
        assert!(pending.0.lock().unwrap().is_empty());
        match outcome {
            SelfHandoffOutcome::Failed(f) => {
                assert!(mode < 4);
                assert_eq!(f.continuation, mode == 2 || mode == 3);
                assert_eq!(f.current, Some(id));
                if mode == 0 {
                    assert_eq!(f.reason, "write failed");
                }
            }
            SelfHandoffOutcome::Completed => assert_eq!(mode, 4),
        }
    }
}

#[tokio::test]
async fn issue_3189_respawn_failures_and_success_both_flavors() {
    use crate::phone::mailbox::handoff::SELF_HANDOFF_FLAVOR_RESTART;
    for flavor in [SELF_HANDOFF_FLAVOR_SWITCH, SELF_HANDOFF_FLAVOR_RESTART] {
        for mode in 0..6 {
            let id = Uuid::new_v4();
            let new_id = Uuid::new_v4();
            let pending = Arc::new(crate::PendingSelfClear::default());
            pending.0.lock().unwrap().insert(id);
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("SELF-HANDOFF.md"), "notes").unwrap();
            let mut ticks = 0;
            let outcome = MailboxPoller::drive_self_switch_after_sustained_idle(
                id,
                root.path().into(),
                "codex".into(),
                "A".into(),
                None,
                flavor,
                pending.clone(),
                Duration::ZERO,
                Duration::ZERO,
                Duration::from_secs(1),
                move |_| {
                    ticks += 1;
                    async move { (mode != 2 && !(mode == 3 && ticks > 1), true) }
                },
                |_, _, _| async { Ok(()) },
                move |_, _, _| async move {
                    if mode == 0 {
                        Err("restart failed".into())
                    } else if mode == 1 {
                        Ok("malformed UUID".into())
                    } else {
                        Ok(new_id.to_string())
                    }
                },
                move |_, _| async move {
                    if mode == 5 {
                        Err("phase-2 write failed".into())
                    } else {
                        Ok(())
                    }
                },
                |_, _| async {},
            )
            .await;
            assert!(pending.0.lock().unwrap().is_empty());
            match outcome {
                SelfHandoffOutcome::Completed => assert_eq!(mode, 4),
                SelfHandoffOutcome::Failed(f) => {
                    assert_ne!(mode, 4);
                    assert_eq!(f.continuation, mode == 3 || mode == 5);
                    assert_eq!(
                        f.current,
                        match mode {
                            1 => None,
                            3 | 5 => Some(new_id),
                            _ => Some(id),
                        }
                    );
                    if mode == 5 {
                        assert_eq!(f.reason, CONTINUATION_FAILURE);
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn issue_3189_retry_pending_cancels() {
    let pending = Arc::new(AtomicBool::new(false));
    let mut calls = 0;
    let status = retry_notice(
        Duration::ZERO,
        Duration::from_secs(1),
        || pending.load(Ordering::SeqCst),
        || {
            calls += 1;
            pending.store(true, Ordering::SeqCst);
            async { Err(NoticeDelivery::Failed("defer".into())) }
        },
        |error| error == "defer",
        "cancelled",
    )
    .await;
    assert_eq!(status, NoticeDelivery::Cancelled);
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn issue_3189_retry_unknown_error_stops() {
    let mut calls = 0;
    let status = retry_notice(
        Duration::ZERO,
        Duration::from_secs(1),
        || false,
        || {
            calls += 1;
            async { Err(NoticeDelivery::Failed("unknown error".into())) }
        },
        |error| error.starts_with("defer"),
        "cancelled",
    )
    .await;
    assert_eq!(calls, 1);
    assert_eq!(status, NoticeDelivery::Failed("unknown error".into()));
}

#[tokio::test]
async fn issue_3189_retry_deadline_caps() {
    let window = Duration::from_millis(20);
    let started = tokio::time::Instant::now();
    let mut starts = Vec::new();
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        retry_notice(
            Duration::from_millis(5),
            window,
            || false,
            || {
                starts.push(tokio::time::Instant::now().duration_since(started));
                async { Err(NoticeDelivery::Failed("defer".into())) }
            },
            |_| true,
            "cancelled",
        ),
    )
    .await
    .expect("deadline must exhaust finitely");
    assert!(!starts.is_empty());
    assert!(starts.iter().all(|elapsed| *elapsed < window));
    assert_eq!(status, NoticeDelivery::Exhausted);
    let mut attempted = false;
    assert_eq!(
        retry_notice(
            Duration::ZERO,
            Duration::ZERO,
            || false,
            || {
                attempted = true;
                async { Ok(()) }
            },
            |_| false,
            "cancelled"
        )
        .await,
        NoticeDelivery::Exhausted
    );
    assert!(!attempted);
}

#[tokio::test]
async fn issue_3189_retry_historical_after_completed_clear() {
    let failure = failure_fixture(Uuid::new_v4());
    let old_attempt = failure.attempt;
    let later_attempt = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    let later_id = failure.original;
    let later_pending = pending.clone();
    let later_root = tempfile::tempdir().unwrap();
    std::fs::write(later_root.path().join("SELF-HANDOFF.md"), "later notes").unwrap();
    let mut calls = 0;
    let text = Arc::new(Mutex::new(String::new()));
    let delivered = text.clone();
    let status = retry_notice(
        Duration::from_millis(1),
        Duration::from_secs(1),
        || pending.0.lock().unwrap().contains(&later_id),
        || {
            calls += 1;
            let pending = later_pending.clone();
            let root = later_root.path().to_path_buf();
            let delivered = delivered.clone();
            let message = failure.message("2026-10-27T04:00:00Z");
            async move {
                if calls == 1 {
                    pending.0.lock().unwrap().insert(later_id);
                    let outcome = MailboxPoller::drive_self_clear_after_sustained_idle(
                        later_id,
                        "/clear",
                        root,
                        pending.clone(),
                        None,
                        Duration::ZERO,
                        Duration::ZERO,
                        Duration::from_secs(1),
                        |_| async { (true, true) },
                        |_, _| async { Ok(()) },
                        |_, _| async {},
                    )
                    .await;
                    assert_eq!(outcome, SelfHandoffOutcome::Completed);
                    assert!(pending.0.lock().unwrap().is_empty());
                    Err(NoticeDelivery::Failed("defer".into()))
                } else {
                    *delivered.lock().unwrap() = message;
                    Ok(())
                }
            }
        },
        |error| error == "defer",
        "cancelled",
    )
    .await;
    assert_eq!(status, NoticeDelivery::Delivered);
    assert_eq!(calls, 2);
    let text = text.lock().unwrap();
    assert!(text.contains(&old_attempt.to_string()));
    assert!(!text.contains(&later_attempt.to_string()));
    assert!(text.contains("it is not the status of a later operation"));
    assert!(text.contains("if no later attempt superseded it, retry"));
}

#[tokio::test]
async fn issue_3189_cap_each_leg_all_actions() {
    use crate::phone::mailbox::handoff::SELF_HANDOFF_FLAVOR_RESTART;
    for action in 0..3 {
        for second_leg in [false, true] {
            let id = Uuid::new_v4();
            let new_id = Uuid::new_v4();
            let pending = Arc::new(crate::PendingSelfClear::default());
            pending.0.lock().unwrap().insert(id);
            let phase_complete = Arc::new(AtomicBool::new(false));
            let phase_seen = phase_complete.clone();
            let session_state = move |_| {
                if second_leg && phase_seen.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1100));
                }
                async move { (true, true) }
            };
            let cap = if second_leg {
                Duration::from_secs(1)
            } else {
                Duration::ZERO
            };
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("SELF-HANDOFF.md"), "cap notes").unwrap();
            let outcome = if action == 0 {
                MailboxPoller::drive_self_clear_after_sustained_idle(
                    id,
                    "/clear",
                    root.path().into(),
                    pending.clone(),
                    None,
                    Duration::ZERO,
                    Duration::ZERO,
                    cap,
                    session_state,
                    move |_, _| {
                        phase_complete.store(true, Ordering::SeqCst);
                        async { Ok(()) }
                    },
                    |_, _| async {},
                )
                .await
            } else {
                let flavor = if action == 1 {
                    SELF_HANDOFF_FLAVOR_SWITCH
                } else {
                    SELF_HANDOFF_FLAVOR_RESTART
                };
                MailboxPoller::drive_self_switch_after_sustained_idle(
                    id,
                    root.path().into(),
                    "codex".into(),
                    "A".into(),
                    None,
                    flavor,
                    pending.clone(),
                    Duration::ZERO,
                    Duration::ZERO,
                    cap,
                    session_state,
                    |_, _, _| async { Ok(()) },
                    move |_, _, _| {
                        phase_complete.store(true, Ordering::SeqCst);
                        async move { Ok(new_id.to_string()) }
                    },
                    |_, _| async { Ok(()) },
                    |_, _| async {},
                )
                .await
            };
            let SelfHandoffOutcome::Failed(failure) = outcome else {
                panic!("cap must report failure");
            };
            assert_eq!(failure.continuation, second_leg);
            assert_eq!(
                failure.current,
                Some(if second_leg && action != 0 {
                    new_id
                } else {
                    id
                })
            );
            assert!(failure.reason.contains("MAX_DEFER cap"));
            assert!(pending.0.lock().unwrap().is_empty());
            assert_eq!(
                std::fs::read(root.path().join("SELF-HANDOFF.md")).unwrap(),
                b"cap notes"
            );
            assert!(!root.path().join("self-clear").exists());
        }
    }
}
