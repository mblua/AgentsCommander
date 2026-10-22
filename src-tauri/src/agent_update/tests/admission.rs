use super::*;

// ---------------------------------------------------------------------
// #1551 - commit and announce
// ---------------------------------------------------------------------

#[tokio::test]
async fn probe_commit_announce_emits_the_committed_state_once() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let ticket = cache.try_begin("bob").expect("ticket");
    probe_commit_announce(&handle, "bob", ticket, |_command| async {
        InstallState::missing("gone".to_string())
    })
    .await;
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"], "agent_install_state_changed");
    assert_eq!(frames[0]["payload"]["install"]["seq"], 1);
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Fresh(_)
    ));
}

#[tokio::test]
async fn probe_commit_announce_reprobes_after_invalidation_and_emits_only_the_new_generation() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let ticket = cache.try_begin("bob").expect("ticket");
    cache.invalidate_all();
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = {
        let calls = Arc::clone(&calls);
        move |_command: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { InstallState::missing("gone".to_string()) }
        }
    };
    probe_commit_announce(&handle, "bob", ticket, probe).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["payload"]["install"]["seq"], 1);
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Fresh(_)
    ));
    assert_eq!(cache.generation(), 1);
}

#[tokio::test]
async fn probe_commit_announce_emits_nothing_when_the_retry_is_rejected() {
    let (app, cache, mut frames_rx) = app_with_cache();
    let handle = app.handle().clone();
    let ticket = cache.try_begin("bob").expect("ticket");
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = {
        let calls = Arc::clone(&calls);
        let cache = Arc::clone(&cache);
        move |_command: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            cache.invalidate_all();
            async { InstallState::missing("gone".to_string()) }
        }
    };
    probe_commit_announce(&handle, "bob", ticket, probe).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_no_frame(&mut frames_rx).await;
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Absent
    ));
    assert_eq!(cache.in_flight_len(), 0);
}

#[test]
fn agent_update_result_contract() {
    let target = UpdateTarget {
        command: "codex".to_string(),
        label: "Codex".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: cwd(),
    };

    let equal = serde_json::to_value(successful_update_result(
        &target,
        installed("1.2.3"),
        installed("1.2.3"),
    ))
    .expect("equal json");
    assert_eq!(equal["ok"], true);
    assert_eq!(equal["outcome"], "succeeded");
    assert_eq!(equal["change"], "unchanged");
    assert!(equal["installBefore"].is_object());
    assert!(equal["installAfter"].is_object());
    assert!(equal.get("error").is_none());
    assert!(equal.get("verificationError").is_none());

    let changed = serde_json::to_value(successful_update_result(
        &target,
        installed("1.2.3"),
        installed("2.0.0"),
    ))
    .expect("changed json");
    assert_eq!(changed["change"], "changed");
    assert!(changed.get("verificationError").is_none());

    let incomparable = serde_json::to_value(successful_update_result(
        &target,
        InstallState::missing("before detail".to_string()),
        InstallState::probe_failed(Path::new("/test/agent"), "after detail".to_string()),
    ))
    .expect("incomparable json");
    assert_eq!(incomparable["change"], "unknown");
    assert_eq!(incomparable["verificationError"], "after detail");
    assert!(incomparable.get("error").is_none());

    let before_fallback = serde_json::to_value(successful_update_result(
        &target,
        InstallState::missing("before detail".to_string()),
        installed("2.0.0"),
    ))
    .expect("before fallback json");
    assert_eq!(before_fallback["verificationError"], "before detail");

    let literal_fallback = serde_json::to_value(successful_update_result(
        &target,
        InstallState {
            status: InstallStatus::Installed,
            version: None,
            path: None,
            detail: None,
            seq: 0,
        },
        installed("2.0.0"),
    ))
    .expect("literal fallback json");
    assert_eq!(
        literal_fallback["verificationError"],
        "Version comparison unavailable."
    );

    let cancelled_result = cancelled_update_result(&target, Some(installed("1.2.3")));
    let cancelled = serde_json::to_value(&cancelled_result).expect("cancelled json");
    assert_eq!(cancelled["ok"], false);
    assert_eq!(cancelled["outcome"], "cancelled");
    assert!(cancelled["installBefore"].is_object());
    assert!(cancelled["installAfter"].is_null());
    assert!(cancelled.get("error").is_none());
    assert!(cancelled.get("verificationError").is_none());
    let cancelled_keys: std::collections::BTreeSet<&str> = cancelled
        .as_object()
        .expect("cancelled object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        cancelled_keys,
        std::collections::BTreeSet::from([
            "change",
            "command",
            "installAfter",
            "installBefore",
            "label",
            "ok",
            "outcome",
        ])
    );

    let failed = serde_json::to_value(failed_update_result(
        &target,
        None,
        None,
        "Update task failed unexpectedly.",
    ))
    .expect("failed json");
    assert_eq!(failed["outcome"], "failed");
    assert_eq!(failed["error"], "Update task failed unexpectedly.");
    assert!(failed["installBefore"].is_null());
    assert!(failed["installAfter"].is_null());
    assert!(failed.get("verificationError").is_none());

    let legacy: AgentUpdateResult = serde_json::from_value(json!({
        "command": "codex",
        "label": "Codex",
        "ok": false
    }))
    .expect("legacy result defaults");
    assert_eq!(legacy.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(legacy.change, AgentUpdateChange::Unknown);
    assert!(legacy.install_before.is_none());
    assert!(legacy.install_after.is_none());

    let status = AgentUpdateGate::new().snapshot();
    let status = serde_json::to_value(status).expect("status json");
    assert_eq!(status["verifying"], json!([]));
    assert_eq!(status["cancelRequested"], json!([]));
    assert_eq!(status["cancelAllRequested"], false);

    let gate = AgentUpdateGate::new();
    seed_retained_target(&gate, &target.command, &target.label);
    assert!(matches!(
        gate.publish_terminal(cancelled_result.clone(), false),
        TerminalPublish::Published
    ));
    gate.command_event_emitted(&target.command);
    gate.store_final_results(vec![cancelled_result]);
    let stored = gate.snapshot();
    assert!(
        stored.in_progress,
        "stored final snapshot remains admission-blocking"
    );
    assert_eq!(stored.results.len(), 1);
    assert_eq!(stored.results[0].outcome, AgentUpdateOutcome::Cancelled);
    assert!(stored.results[0].install_after.is_none());
    gate.release_finished();
    let remounted: AgentUpdateStatus =
        serde_json::from_value(serde_json::to_value(gate.snapshot()).expect("snapshot wire value"))
            .expect("snapshot survives serialization");
    assert!(!remounted.in_progress);
    assert_eq!(remounted.results[0].outcome, AgentUpdateOutcome::Cancelled);
    assert!(remounted.results[0].install_after.is_none());
}

#[tokio::test]
async fn step_admission_cancel_barrier_is_first_winner() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let dir = tempfile::tempdir().expect("tempdir");

    // Cancellation owns the real final-check boundary first. The production
    // sequence is held after `begin_step`; its response precedes release,
    // commit is refused, and the real shell marker never spawns.
    let gate = Arc::new(AgentUpdateGate::new());
    let cancelled_marker = dir.path().join("cancelled-first.marker");
    let cancelled_target = UpdateTarget {
        command: "cancel-first".to_string(),
        label: "Cancel first".to_string(),
        commands: vec!["echo forbidden>cancelled-first.marker".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &cancelled_target.command, &cancelled_target.label);
    let (began_tx, began_rx) = tokio::sync::oneshot::channel();
    let (release_commit_tx, release_commit_rx) = tokio::sync::oneshot::channel();
    gate.state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .controls
        .get_mut(&cancelled_target.command)
        .expect("cancel-first control")
        .step_admission_hook = Some(StepAdmissionTestHook {
        after_begin: Some(began_tx),
        release_commit: Some(release_commit_rx),
        after_commit: None,
        release_spawn: None,
    });
    let cancelled_sequence = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move {
            let mut owner = TargetOwnerSlot::default();
            run_update_sequence_cancellable(
                &cancelled_target,
                Duration::from_secs(30),
                &gate,
                &mut owner,
            )
            .await
        }
    });
    began_rx.await.expect("production begin-step barrier");
    let response = cancel_update(&handle, &gate, "cancel-first".to_string()).await;
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let reserved = next_frame(&mut frames).await;
    assert_eq!(reserved["event"], "agent_update_cancellation_changed");
    assert_eq!(
        reserved["payload"]["cancelRequested"][0]["command"],
        "cancel-first"
    );
    assert!(!cancelled_marker.exists());
    release_commit_tx.send(()).expect("release commit boundary");
    assert!(matches!(
        cancelled_sequence.await.expect("cancel-first sequence"),
        UpdaterStepOutcome::Cancelled
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !cancelled_marker.exists(),
        "no spawn can follow the response"
    );

    // Admission owns the same production boundary first. Hold immediately
    // after commit: cancellation must wait for the actual owner handoff. A
    // first-step marker may win before the response, but neither it nor a
    // later-step marker may appear after that response.
    let gate = Arc::new(AgentUpdateGate::new());
    let admitted_marker = dir.path().join("admitted-first.marker");
    let later_marker = dir.path().join("forbidden-later.marker");
    let admitted_step = if cfg!(windows) {
        "echo admitted>admitted-first.marker & ping -n 30 127.0.0.1 >NUL"
    } else {
        "echo admitted > admitted-first.marker; sleep 30"
    };
    let admitted_target = UpdateTarget {
        command: "admission-first".to_string(),
        label: "Admission first".to_string(),
        commands: vec![
            admitted_step.to_string(),
            "echo forbidden>forbidden-later.marker".to_string(),
        ],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &admitted_target.command, &admitted_target.label);
    let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
    let (release_spawn_tx, release_spawn_rx) = tokio::sync::oneshot::channel();
    gate.state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .controls
        .get_mut(&admitted_target.command)
        .expect("admission-first control")
        .step_admission_hook = Some(StepAdmissionTestHook {
        after_begin: None,
        release_commit: None,
        after_commit: Some(committed_tx),
        release_spawn: Some(release_spawn_rx),
    });
    let admitted_sequence = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move {
            let mut owner = TargetOwnerSlot::default();
            run_update_sequence_cancellable(
                &admitted_target,
                Duration::from_secs(30),
                &gate,
                &mut owner,
            )
            .await
        }
    });
    committed_rx.await.expect("production commit-step barrier");
    let request = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move { cancel_update(&handle, &gate, "admission-first".to_string()).await }
    });
    let reserved = next_frame(&mut frames).await;
    assert_eq!(reserved["event"], "agent_update_cancellation_changed");
    assert_eq!(
        reserved["payload"]["cancelRequested"][0]["command"],
        "admission-first"
    );
    assert!(
        !request.is_finished(),
        "response waits for the committed production owner handoff"
    );
    release_spawn_tx
        .send(())
        .expect("release real spawn boundary");
    let response = request.await.expect("cancel task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let marker_at_response = admitted_marker.exists();
    assert!(matches!(
        admitted_sequence.await.expect("admission-first sequence"),
        UpdaterStepOutcome::Cancelled
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        admitted_marker.exists(),
        marker_at_response,
        "the admitted marker cannot first appear after the response"
    );
    assert!(!later_marker.exists(), "no later step may be admitted");
}

#[cfg(windows)]
#[tokio::test]
async fn probe_owner_install_waits_for_real_spawn_success() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let target = UpdateTarget {
        command: "cmd.exe".to_string(),
        label: "Probe handoff".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: cwd(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let probe = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            crate::agent_version::with_version_probe_args_for_test(
                "cmd",
                &["/D", "/C", "echo 1.2.3"],
                crate::pty::job::with_contained_spawn_test_hook(
                    crate::pty::job::SpawnTestHook {
                        after_spawn: Some(spawned_tx),
                        release_spawn: Some(release_rx),
                        after_reap: None,
                        release_query: None,
                        failure: None,
                        settlement_window: None,
                    },
                    async move {
                        let mut owner = TargetOwnerSlot::default();
                        run_target_probe(&handle, &gate, &target, TargetWork::PostProbe, &mut owner)
                            .await
                    },
                ),
            )
            .await
        }
    });
    spawned_rx
        .await
        .expect("real probe spawned suspended before containment handoff");
    {
        let state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
        let control = state.controls.get(&target.command).expect("target control");
        assert!(matches!(
            control.phase,
            TargetPhase::SpawnCommitted(TargetWork::PostProbe, _)
        ));
        assert!(!control.acceptance.is_open());
    }
    assert!(gate.snapshot().verifying.is_empty());
    assert!(
        frames.try_recv().is_err(),
        "no verifying event before handoff"
    );

    let cancel = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let command = target.command.clone();
        async move { cancel_update(&handle, &gate, command).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    assert!(!cancel.is_finished());
    {
        let state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
        assert!(!state
            .controls
            .get(&target.command)
            .expect("target control")
            .acceptance
            .is_open());
    }
    release_tx.send(()).expect("release actual owner handoff");
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_command_verifying"
    );
    assert_eq!(
        cancel.await.expect("cancel task").disposition,
        AgentUpdateCancelDisposition::Requested
    );
    assert!(matches!(
        probe.await.expect("probe task"),
        TargetProbeOutcome::Cancelled
    ));
}

#[cfg(windows)]
#[tokio::test]
async fn rejected_spawn_cleanup_dominates_adapters_and_final_result() {
    use crate::pty::job::{InjectedFailure, SpawnTestHook};

    for failure in [InjectedFailure::Assignment, InjectedFailure::Resume] {
        let dir = tempfile::tempdir().expect("tempdir");
        let first_marker = dir.path().join("rejected-first.marker");
        let later_marker = dir.path().join("forbidden-later.marker");
        let target = UpdateTarget {
            command: "rejected-updater".to_string(),
            label: "Rejected updater".to_string(),
            commands: vec![
                "echo forbidden>rejected-first.marker".to_string(),
                "echo forbidden>forbidden-later.marker".to_string(),
            ],
            cwd: dir.path().to_path_buf(),
        };
        let gate = Arc::new(AgentUpdateGate::new());
        seed_retained_target(&gate, &target.command, &target.label);
        let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
        let (release_spawn_tx, release_spawn_rx) = tokio::sync::oneshot::channel();
        let (reaped_tx, reaped_rx) = tokio::sync::oneshot::channel();
        let (release_query_tx, release_query_rx) = tokio::sync::oneshot::channel();
        let adapter = tokio::spawn({
            let gate = Arc::clone(&gate);
            let target = target.clone();
            async move {
                crate::pty::job::with_contained_spawn_test_hook(
                    SpawnTestHook {
                        after_spawn: Some(spawned_tx),
                        release_spawn: Some(release_spawn_rx),
                        after_reap: Some(reaped_tx),
                        release_query: Some(release_query_rx),
                        failure: Some(failure),
                        settlement_window: Some(Duration::from_millis(60)),
                    },
                    async move {
                        let mut owner = TargetOwnerSlot::default();
                        run_update_sequence_cancellable(
                            &target,
                            Duration::from_secs(30),
                            &gate,
                            &mut owner,
                        )
                        .await
                    },
                )
                .await
            }
        });
        spawned_rx.await.expect("updater spawned suspended");
        {
            let state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
            assert!(matches!(
                state
                    .controls
                    .get(&target.command)
                    .expect("target control")
                    .phase,
                TargetPhase::SpawnCommitted(TargetWork::UpdaterStep(0), _)
            ));
        }
        assert!(!first_marker.exists());
        release_spawn_tx.send(()).expect("reject updater spawn");
        reaped_rx.await.expect("rejected updater child reaped");
        tokio::time::sleep(Duration::from_millis(75)).await;
        release_query_tx
            .send(())
            .expect("release updater native proof");
        let UpdaterStepOutcome::CleanupFailed(detail) =
            adapter.await.expect("updater adapter task")
        else {
            panic!("rejected updater cleanup did not dominate");
        };
        assert!(detail
            .contains("settlement attempt deadline exceeded during rejected-spawn proof barrier"));
        assert!(!first_marker.exists());
        assert!(!later_marker.exists(), "no later updater step may run");

        let (app, mut frames) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = Arc::new(AgentUpdateGate::new());
        let target_dir = tempfile::tempdir().expect("target tempdir");
        let target = UpdateTarget {
            command: "cmd.exe".to_string(),
            label: "Rejected post-probe".to_string(),
            commands: vec!["exit 0".to_string()],
            cwd: target_dir.path().to_path_buf(),
        };
        seed_retained_target(&gate, &target.command, &target.label);
        let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
        let (release_spawn_tx, release_spawn_rx) = tokio::sync::oneshot::channel();
        let (reaped_tx, reaped_rx) = tokio::sync::oneshot::channel();
        let (release_query_tx, release_query_rx) = tokio::sync::oneshot::channel();
        let target_task = tokio::spawn({
            let handle = handle.clone();
            let gate = Arc::clone(&gate);
            let target = target.clone();
            async move {
                crate::agent_version::with_version_probe_args_for_test(
                    "cmd",
                    &["/D", "/C", "echo 1.2.3"],
                    crate::pty::job::with_contained_spawn_test_hook_after_spawns(
                        2,
                        SpawnTestHook {
                            after_spawn: Some(spawned_tx),
                            release_spawn: Some(release_spawn_rx),
                            after_reap: Some(reaped_tx),
                            release_query: Some(release_query_rx),
                            failure: Some(failure),
                            settlement_window: Some(Duration::from_millis(60)),
                        },
                        run_update_target(handle, gate, target),
                    ),
                )
                .await
            }
        });
        if spawned_rx.await.is_err() {
            let early = target_task.await.expect("early target task");
            panic!("target completed before rejected post-probe barrier: {early:?}");
        }
        {
            let state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
            let control = state.controls.get(&target.command).expect("target control");
            assert!(matches!(
                control.phase,
                TargetPhase::SpawnCommitted(TargetWork::PostProbe, _)
            ));
            assert!(!control.acceptance.is_open());
        }
        let before_cancel = drain_frames(&mut frames);
        assert!(before_cancel
            .iter()
            .any(|event| event["event"] == "agent_update_command_started"));
        assert!(!before_cancel
            .iter()
            .any(|event| event["event"] == "agent_update_command_verifying"));
        let cancel = tokio::spawn({
            let handle = handle.clone();
            let gate = Arc::clone(&gate);
            let command = target.command.clone();
            async move { cancel_update(&handle, &gate, command).await }
        });
        assert_eq!(
            next_frame(&mut frames).await["event"],
            "agent_update_cancellation_changed"
        );
        assert!(!cancel.is_finished());
        release_spawn_tx.send(()).expect("reject post-probe spawn");
        reaped_rx.await.expect("rejected probe child reaped");
        tokio::time::sleep(Duration::from_millis(75)).await;
        release_query_tx
            .send(())
            .expect("release rejected probe native proof");
        assert_eq!(
            cancel.await.expect("cancel task").disposition,
            AgentUpdateCancelDisposition::Requested
        );
        let result = target_task.await.expect("target task");
        assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
        assert_eq!(result.error.as_deref(), Some(CANCELLATION_CLEANUP_ERROR));
        assert!(result.install_after.is_none());
        let remaining = drain_frames(&mut frames);
        assert!(!remaining
            .iter()
            .any(|event| event["event"] == "agent_update_command_verifying"));
        assert_eq!(
            remaining
                .iter()
                .filter(|event| event["event"] == "agent_update_command_finished")
                .count(),
            1
        );
    }
}
