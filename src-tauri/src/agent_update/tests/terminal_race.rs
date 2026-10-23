use super::*;

#[tokio::test]
async fn post_probe_settled_cancel_wins_before_terminal() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let command = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let stem = if cfg!(windows) { "cmd" } else { "sh" };
    let args: &'static [&'static str] = if cfg!(windows) {
        &["/D", "/C", "echo 1.2.3"]
    } else {
        &["-c", "printf '1.2.3\\n'"]
    };
    let target = UpdateTarget {
        command: command.to_string(),
        label: "Post-probe cancel winner".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (settled_tx, settled_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    gate.state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .controls
        .get_mut(&target.command)
        .expect("target control")
        .post_probe_settled_hook = Some(PostProbeSettledTestHook {
        ready: Some(settled_tx),
        release: Some(release_rx),
    });
    let target_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            crate::agent_version::with_version_probe_args_for_test(
                stem,
                args,
                run_update_target(handle, gate, target),
            )
            .await
        }
    });
    settled_rx
        .await
        .expect("production owner returned and post-probe value was stored");
    let started = next_frame(&mut frames).await;
    let verifying = next_frame(&mut frames).await;
    assert_eq!(started["event"], "agent_update_command_started");
    assert_eq!(verifying["event"], "agent_update_command_verifying");
    assert!(gate.snapshot().results.is_empty());

    let cancel = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let command = target.command.clone();
        async move { cancel_update(&handle, &gate, command).await }
    });
    let cancellation_event = next_frame(&mut frames).await;
    assert_eq!(
        cancellation_event["event"],
        "agent_update_cancellation_changed"
    );
    assert!(gate.snapshot().results.is_empty());
    release_tx
        .send(())
        .expect("give cancellation the production gate race");
    let response = tokio::time::timeout(Duration::from_secs(2), cancel)
        .await
        .expect("owner-settled cancellation response")
        .expect("cancel task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let result = target_task.await.expect("production target task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Cancelled);
    assert!(result.install_after.is_none());

    PassSupervisor {
        app: handle,
        gate: Arc::clone(&gate),
    }
    .finalize(PassRuntime {
        started: true,
        ..PassRuntime::default()
    })
    .await;
    let command_finished = next_frame(&mut frames).await;
    let pass_finished = next_frame(&mut frames).await;
    assert_eq!(command_finished["event"], "agent_update_command_finished");
    assert_eq!(command_finished["payload"]["outcome"], "cancelled");
    assert!(command_finished["payload"]["installAfter"].is_null());
    assert_eq!(pass_finished["event"], "agent_updates_finished");
    assert_no_frame(&mut frames).await;
}

#[tokio::test]
async fn ordinary_failure_yields_to_prior_cancellation_reservation() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    seed_retained_target(&gate, "codex", "Codex");
    set_target_phase(&gate, "codex", TargetPhase::Verifying);
    let target = UpdateTarget {
        command: "codex".to_string(),
        label: "Codex".to_string(),
        commands: vec!["exit 7".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    let terminalizer = TargetTerminalizer::new(handle.clone(), Arc::clone(&gate), target.clone());
    let cancel = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move { cancel_update(&handle, &gate, "codex".to_string()).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    let terminal = tokio::spawn(async move {
        terminalizer
            .publish(failed_update_result(
                &target,
                Some(installed("1.0.0")),
                None,
                "exit code 7",
            ))
            .await
    });
    assert_eq!(
        cancel.await.expect("cancel task").disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let result = terminal.await.expect("terminalizer");
    assert_eq!(result.outcome, AgentUpdateOutcome::Cancelled);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn aborted_cancel_request_releases_terminal_fence() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    seed_retained_target(&gate, "codex", "Codex");
    set_target_phase(&gate, "codex", TargetPhase::Verifying);
    let target = UpdateTarget {
        command: "codex".to_string(),
        label: "Codex".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: cwd(),
    };
    let cancel = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move { cancel_update(&handle, &gate, "codex".to_string()).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    cancel.abort();
    assert!(cancel
        .await
        .expect_err("request was aborted")
        .is_cancelled());

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        TargetTerminalizer::new(handle, Arc::clone(&gate), target.clone()).publish(
            successful_update_result(&target, installed("1.0.0"), installed("2.0.0")),
        ),
    )
    .await
    .expect("aborted request cannot strand terminal publication");
    assert_eq!(result.outcome, AgentUpdateOutcome::Cancelled);
}

#[tokio::test]
async fn post_probe_settled_completion_wins_before_cancel() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let command = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let stem = if cfg!(windows) { "cmd" } else { "sh" };
    let args: &'static [&'static str] = if cfg!(windows) {
        &["/D", "/C", "echo 1.2.3"]
    } else {
        &["-c", "printf '1.2.3\\n'"]
    };
    let target = UpdateTarget {
        command: command.to_string(),
        label: "Post-probe completion winner".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (settled_tx, settled_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    gate.state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .controls
        .get_mut(&target.command)
        .expect("target control")
        .post_probe_settled_hook = Some(PostProbeSettledTestHook {
        ready: Some(settled_tx),
        release: Some(release_rx),
    });
    let target_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            crate::agent_version::with_version_probe_args_for_test(
                stem,
                args,
                run_update_target(handle, gate, target),
            )
            .await
        }
    });
    settled_rx
        .await
        .expect("production owner returned and post-probe value was stored");
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_command_started"
    );
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_command_verifying"
    );
    release_tx
        .send(())
        .expect("give completion the production gate race");
    let result = target_task.await.expect("production target task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Succeeded);
    let command_finished = next_frame(&mut frames).await;
    assert_eq!(command_finished["event"], "agent_update_command_finished");
    assert_eq!(command_finished["payload"]["outcome"], "succeeded");
    let response = cancel_update(&handle, &gate, target.command.clone()).await;
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::AlreadyTerminal
    );
    PassSupervisor {
        app: handle,
        gate: Arc::clone(&gate),
    }
    .finalize(PassRuntime {
        started: true,
        ..PassRuntime::default()
    })
    .await;
    let events = drain_frames(&mut frames);
    assert_eq!(events.len(), 1, "unexpected events: {events:?}");
    assert_eq!(events[0]["event"], "agent_updates_finished");
    assert!(!events
        .iter()
        .any(|event| event["event"] == "agent_update_cancellation_changed"));
}

async fn prompt_persist_success_serializes_cancel(batch: bool, enabled: bool) {
    let (app, _frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    seed_retained_target(&gate, "claude", "Claude");
    let rx = gate.register_prompt("claude", "Claude");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let answer = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        let writes = Arc::clone(&writes);
        async move {
            answer_prompt(&handle, &gate, "claude", enabled, move || async move {
                entered.notify_one();
                release.notified().await;
                writes.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            })
            .await
        }
    });
    entered.notified().await;
    let mut cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move {
            if batch {
                serde_json::to_value(cancel_all_updates(&handle, &gate).await).unwrap()
            } else {
                serde_json::to_value(cancel_update(&handle, &gate, "claude".to_string()).await)
                    .unwrap()
            }
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut cancellation)
            .await
            .is_err(),
        "cancellation must queue behind persistence"
    );
    release.notify_one();
    assert!(answer.await.expect("answer task").expect("answer"));
    assert_eq!(rx.await.expect("prompt decision"), enabled);
    let cancellation = cancellation.await.expect("cancellation task");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    if enabled {
        if batch {
            assert_eq!(cancellation["requested"][0]["command"], "claude");
        } else {
            assert_eq!(cancellation["disposition"], "requested");
        }
    } else if batch {
        assert_eq!(
            cancellation,
            json!({
                "requested": [],
                "alreadyRequested": [],
                "alreadyTerminal": []
            })
        );
    } else {
        assert_eq!(cancellation["disposition"], "not_in_pass");
    }
}

#[tokio::test]
async fn prompt_persist_success_serializes_row_cancel() {
    prompt_persist_success_serializes_cancel(false, true).await;
    prompt_persist_success_serializes_cancel(false, false).await;
}

#[tokio::test]
async fn prompt_persist_success_serializes_batch_cancel() {
    prompt_persist_success_serializes_cancel(true, true).await;
    prompt_persist_success_serializes_cancel(true, false).await;
}

async fn prompt_persist_failure_allows_cancel(batch: bool) {
    let (app, _frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    seed_retained_target(&gate, "claude", "Claude");
    let rx = gate.register_prompt("claude", "Claude");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let answer = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        let writes = Arc::clone(&writes);
        async move {
            answer_prompt(&handle, &gate, "claude", true, move || async move {
                entered.notify_one();
                release.notified().await;
                let _ = &writes;
                Err::<(), String>("disk full".to_string())
            })
            .await
        }
    });
    entered.notified().await;
    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move {
            if batch {
                serde_json::to_value(cancel_all_updates(&handle, &gate).await).unwrap()
            } else {
                serde_json::to_value(cancel_update(&handle, &gate, "claude".to_string()).await)
                    .unwrap()
            }
        }
    });
    release.notify_one();
    assert_eq!(
        answer.await.expect("answer task"),
        Err("disk full".to_string())
    );
    let cancellation = cancellation.await.expect("cancellation task");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert!(rx.await.is_err(), "cancellation closes the prompt");
    if batch {
        assert_eq!(cancellation["requested"][0]["command"], "claude");
    } else {
        assert_eq!(cancellation["disposition"], "requested");
    }
}

#[tokio::test]
async fn prompt_persist_failure_allows_row_cancel() {
    prompt_persist_failure_allows_cancel(false).await;
}

#[tokio::test]
async fn prompt_persist_failure_allows_batch_cancel() {
    prompt_persist_failure_allows_cancel(true).await;
}

#[tokio::test]
async fn prompt_cancel_does_not_persist_policy() {
    let (app, _frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    seed_retained_target(&gate, "claude", "Claude");
    let _rx = gate.register_prompt("claude", "Claude");
    let response = cancel_update(&handle, &gate, "claude".to_string()).await;
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let persisted = Arc::clone(&calls);
    assert!(!answer_prompt(&handle, &gate, "claude", true, move || {
        persisted.fetch_add(1, Ordering::SeqCst);
        async { Ok::<(), String>(()) }
    })
    .await
    .expect("late cancelled answer"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(gate.snapshot().answered.is_empty());
}
