use super::*;

#[cfg(any(windows, unix))]
#[tokio::test]
async fn target_panic_during_real_updater_settlement_retains_owner() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = UpdateTarget {
        command: "missing-updater-panic-target".to_string(),
        label: "Updater panic target".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (proof_ready_tx, proof_ready_rx) = tokio::sync::oneshot::channel();
    let (release_proof_tx, release_proof_rx) = tokio::sync::oneshot::channel();
    let target_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            with_target_settlement_hook(
                TargetSettlementHook {
                    before_native_proof: Some(proof_ready_tx),
                    release_native_proof: Some(release_proof_rx),
                    settlement_window: None,
                    native_observations: std::collections::VecDeque::from([
                        TargetNativeObservation::Error("injected pre-panic updater cleanup defect"),
                        TargetNativeObservation::Empty,
                    ]),
                    panic_after_native_proof_release: false,
                    panic_after_native_query_defect: true,
                    stdout_reader: None,
                },
                run_update_target(handle, gate, target),
            )
            .await
        }
    });
    proof_ready_rx
        .await
        .expect("real updater reached retained native proof");
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_command_started"
    );
    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let command = target.command.clone();
        async move { cancel_update(&handle, &gate, command).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    let response = tokio::time::timeout(Duration::from_secs(2), cancellation)
        .await
        .expect("row response is ready while updater proof remains held")
        .expect("row cancellation task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    assert!(!target_task.is_finished());
    assert!(gate.snapshot().results.is_empty());
    assert!(frames.try_recv().is_err(), "terminal remains proof-blocked");
    release_proof_tx
        .send(())
        .expect("release updater proof into injected panic");
    let result = target_task.await.expect("retained updater target task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(result.error.as_deref(), Some(CANCELLATION_CLEANUP_ERROR));
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
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "agent_update_command_finished")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "agent_updates_finished")
            .count(),
        1
    );
    assert_eq!(events[0]["payload"]["error"], CANCELLATION_CLEANUP_ERROR);
}

#[cfg(any(windows, unix))]
#[tokio::test]
async fn target_panic_during_real_updater_reader_settlement_recovers_same_reader() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = UpdateTarget {
        command: "missing-updater-reader-panic-target".to_string(),
        label: "Updater reader panic target".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (reader_ready_tx, reader_ready_rx) = tokio::sync::oneshot::channel();
    let (release_panic_tx, release_panic_rx) = tokio::sync::oneshot::channel();
    let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
    let (release_reader_tx, release_reader_rx) = tokio::sync::oneshot::channel();
    let target_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        let ends = ReaderPanicHookEnds {
            hold_completion: release_reader_rx,
            before_panic: reader_ready_tx,
            release_panic: release_panic_rx,
            retry_started: retry_started_tx,
        };
        async move { run_updater_reader_panic_target(handle, gate, target, ends).await }
    });
    reader_ready_rx
        .await
        .expect("real updater reader settlement began while reader remained blocked");
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_command_started"
    );
    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let command = target.command.clone();
        async move { cancel_update(&handle, &gate, command).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    let response = tokio::time::timeout(Duration::from_secs(2), cancellation)
        .await
        .expect("row response is ready while updater reader remains held")
        .expect("row cancellation task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    assert_target_blocked_without_results(
        &target_task,
        &gate,
        &mut frames,
        "terminal remains reader-blocked",
    );
    release_panic_tx
        .send(())
        .expect("release updater reader settlement into injected panic");
    tokio::time::timeout(Duration::from_secs(2), retry_started_rx)
        .await
        .expect("retained updater reader was polled again")
        .expect("retained updater reader retry signal");
    assert_target_blocked_without_results(
        &target_task,
        &gate,
        &mut frames,
        "terminal waits for retained reader",
    );
    release_reader_tx
        .send(())
        .expect("allow the retained updater reader to complete");
    let result = tokio::time::timeout(Duration::from_secs(15), target_task)
        .await
        .expect("retained updater reader settled")
        .expect("retained updater reader target task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(
        result.error.as_deref(),
        Some("Update task failed unexpectedly.")
    );
    finalize_and_assert_single_finish(
        handle,
        &gate,
        &mut frames,
        "Update task failed unexpectedly.",
    )
    .await;
}

fn assert_target_blocked_without_results<T>(
    target_task: &tokio::task::JoinHandle<T>,
    gate: &Arc<AgentUpdateGate>,
    frames: &mut tokio::sync::mpsc::Receiver<WsOutMsg>,
    blocked_message: &str,
) {
    assert!(!target_task.is_finished());
    assert!(gate.snapshot().results.is_empty());
    assert!(frames.try_recv().is_err(), "{}", blocked_message);
}

async fn finalize_and_assert_single_finish(
    handle: tauri::AppHandle,
    gate: &Arc<AgentUpdateGate>,
    frames: &mut tokio::sync::mpsc::Receiver<WsOutMsg>,
    expected_error: &str,
) {
    PassSupervisor {
        app: handle,
        gate: Arc::clone(gate),
    }
    .finalize(PassRuntime {
        started: true,
        ..PassRuntime::default()
    })
    .await;
    let events = drain_frames(frames);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "agent_update_command_finished")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "agent_updates_finished")
            .count(),
        1
    );
    assert_eq!(events[0]["payload"]["error"], expected_error);
}

struct ReaderPanicHookEnds {
    hold_completion: tokio::sync::oneshot::Receiver<()>,
    before_panic: tokio::sync::oneshot::Sender<()>,
    release_panic: tokio::sync::oneshot::Receiver<()>,
    retry_started: tokio::sync::oneshot::Sender<()>,
}

async fn run_updater_reader_panic_target(
    handle: tauri::AppHandle,
    gate: Arc<AgentUpdateGate>,
    target: UpdateTarget,
    ends: ReaderPanicHookEnds,
) -> AgentUpdateResult {
    with_target_settlement_hook(
        TargetSettlementHook {
            before_native_proof: None,
            release_native_proof: None,
            settlement_window: None,
            native_observations: std::collections::VecDeque::from([TargetNativeObservation::Empty]),
            panic_after_native_proof_release: false,
            panic_after_native_query_defect: false,
            stdout_reader: Some(TargetReaderSettlementHook {
                hold_completion: Some(ends.hold_completion),
                before_panic: Some(ends.before_panic),
                release_panic: Some(ends.release_panic),
                retry_started: Some(ends.retry_started),
            }),
        },
        run_update_target(handle, gate, target),
    )
    .await
}

async fn run_probe_reader_panic_target(
    handle: tauri::AppHandle,
    gate: Arc<AgentUpdateGate>,
    target: UpdateTarget,
    (stem, args): (&'static str, &'static [&'static str]),
    ends: ReaderPanicHookEnds,
) -> AgentUpdateResult {
    crate::agent_version::with_version_probe_args_for_test(
        stem,
        args,
        crate::agent_version::with_probe_settlement_hook_after_spawns(
            1,
            crate::agent_version::ProbeSettlementHook {
                before_native_proof: None,
                release_native_proof: None,
                settlement_window: None,
                native_observations: std::collections::VecDeque::from([
                    crate::agent_version::ProbeNativeObservation::Empty,
                ]),
                panic_after_native_proof_release: false,
                panic_after_native_query_defect: false,
                stdout_reader: Some(crate::agent_version::ProbeReaderSettlementHook {
                    hold_completion: Some(ends.hold_completion),
                    before_panic: Some(ends.before_panic),
                    release_panic: Some(ends.release_panic),
                    retry_started: Some(ends.retry_started),
                }),
            },
            run_update_target(handle, gate, target),
        ),
    )
    .await
}

fn probe_shell_fixture() -> (&'static str, &'static str, &'static [&'static str]) {
    let command = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let stem = if cfg!(windows) { "cmd" } else { "sh" };
    let args: &'static [&'static str] = if cfg!(windows) {
        &["/D", "/C", "echo 1.2.3"]
    } else {
        &["-c", "printf '1.2.3\\n'"]
    };
    (command, stem, args)
}

#[cfg(any(windows, unix))]
#[tokio::test]
async fn target_panic_during_real_probe_settlement_retains_owner() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let (command, stem, args) = probe_shell_fixture();
    let target = UpdateTarget {
        command: command.to_string(),
        label: "Probe panic target".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (proof_ready_tx, proof_ready_rx) = tokio::sync::oneshot::channel();
    let (release_proof_tx, release_proof_rx) = tokio::sync::oneshot::channel();
    let target_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            crate::agent_version::with_version_probe_args_for_test(
                stem,
                args,
                crate::agent_version::with_probe_settlement_hook_after_spawns(
                    1,
                    crate::agent_version::ProbeSettlementHook {
                        before_native_proof: Some(proof_ready_tx),
                        release_native_proof: Some(release_proof_rx),
                        settlement_window: None,
                        native_observations: std::collections::VecDeque::from([
                            crate::agent_version::ProbeNativeObservation::Error(
                                "injected pre-panic probe cleanup defect",
                            ),
                            crate::agent_version::ProbeNativeObservation::Empty,
                        ]),
                        panic_after_native_proof_release: false,
                        panic_after_native_query_defect: true,
                        stdout_reader: None,
                    },
                    run_update_target(handle, gate, target),
                ),
            )
            .await
        }
    });
    proof_ready_rx
        .await
        .expect("real post-probe reached retained native proof");
    let started = next_frame(&mut frames).await;
    let verifying = next_frame(&mut frames).await;
    assert_eq!(started["event"], "agent_update_command_started");
    assert_eq!(verifying["event"], "agent_update_command_verifying");
    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move { cancel_all_updates(&handle, &gate).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    let response = tokio::time::timeout(Duration::from_secs(2), cancellation)
        .await
        .expect("batch response is ready while probe proof remains held")
        .expect("batch cancellation task");
    assert_eq!(response.requested.len(), 1);
    assert_eq!(response.requested[0].command, target.command);
    assert_target_blocked_without_results(
        &target_task,
        &gate,
        &mut frames,
        "terminal remains proof-blocked",
    );
    release_proof_tx
        .send(())
        .expect("release probe proof into injected panic");
    let result = target_task.await.expect("retained probe target task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(result.error.as_deref(), Some(CANCELLATION_CLEANUP_ERROR));
    assert!(result.install_after.is_none());
    finalize_and_assert_single_finish(handle, &gate, &mut frames, CANCELLATION_CLEANUP_ERROR).await;
}

#[cfg(any(windows, unix))]
#[tokio::test]
async fn target_panic_during_real_probe_reader_settlement_recovers_same_reader() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let (command, stem, args) = probe_shell_fixture();
    let target = UpdateTarget {
        command: command.to_string(),
        label: "Probe reader panic target".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (reader_ready_tx, reader_ready_rx) = tokio::sync::oneshot::channel();
    let (release_panic_tx, release_panic_rx) = tokio::sync::oneshot::channel();
    let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
    let (release_reader_tx, release_reader_rx) = tokio::sync::oneshot::channel();
    let target_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        let ends = ReaderPanicHookEnds {
            hold_completion: release_reader_rx,
            before_panic: reader_ready_tx,
            release_panic: release_panic_rx,
            retry_started: retry_started_tx,
        };
        async move { run_probe_reader_panic_target(handle, gate, target, (stem, args), ends).await }
    });
    reader_ready_rx
        .await
        .expect("real probe reader settlement began while reader remained blocked");
    let started = next_frame(&mut frames).await;
    let verifying = next_frame(&mut frames).await;
    assert_eq!(started["event"], "agent_update_command_started");
    assert_eq!(verifying["event"], "agent_update_command_verifying");
    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        async move { cancel_all_updates(&handle, &gate).await }
    });
    assert_eq!(
        next_frame(&mut frames).await["event"],
        "agent_update_cancellation_changed"
    );
    let response = tokio::time::timeout(Duration::from_secs(2), cancellation)
        .await
        .expect("batch response is ready while probe reader remains held")
        .expect("batch cancellation task");
    assert_eq!(response.requested.len(), 1);
    assert_eq!(response.requested[0].command, target.command);
    assert_target_blocked_without_results(
        &target_task,
        &gate,
        &mut frames,
        "terminal remains reader-blocked",
    );
    release_panic_tx
        .send(())
        .expect("release probe reader settlement into injected panic");
    tokio::time::timeout(Duration::from_secs(2), retry_started_rx)
        .await
        .expect("retained probe reader was polled again")
        .expect("retained probe reader retry signal");
    assert_target_blocked_without_results(
        &target_task,
        &gate,
        &mut frames,
        "terminal waits for retained reader",
    );
    release_reader_tx
        .send(())
        .expect("allow the retained probe reader to complete");
    let result = tokio::time::timeout(Duration::from_secs(15), target_task)
        .await
        .expect("retained probe reader settled")
        .expect("retained probe reader target task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(
        result.error.as_deref(),
        Some("Update task failed unexpectedly.")
    );
    assert!(result.install_after.is_none());
    finalize_and_assert_single_finish(
        handle,
        &gate,
        &mut frames,
        "Update task failed unexpectedly.",
    )
    .await;
}
