use super::*;

async fn exercise_contained_update_cancellation(
    work: TargetWork,
    hold_before_native_proof: bool,
) -> Vec<Value> {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let target = UpdateTarget {
        command: "contained-test".to_string(),
        label: "Contained".to_string(),
        commands: vec!["blocked".to_string()],
        cwd: cwd(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let install_before = InstallState::missing("test pre-probe".to_string());
    if matches!(work, TargetWork::UpdaterStep(_)) {
        let started =
            gate.mark_command_started(&target.command, &target.label, Some(install_before.clone()));
        emit_all(&handle, "agent_update_command_started", json!(started));
    }
    let (epoch, _) = gate
        .begin_step(&target.command, work)
        .expect("step reservation");
    let StepAdmission::Committed { epoch, mut cancel } =
        gate.commit_step(&target.command, work, epoch)
    else {
        panic!("step admission");
    };

    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/C").arg("cmd /C ping -n 30 127.0.0.1 >NUL");
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg("sh -c 'sleep 30' & wait");
        command
    };
    command.stdin(Stdio::null());
    if hold_before_native_proof {
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    } else {
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
    }
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut owner = match TargetProcessOwner::spawn(&mut command).await {
        Ok(owner) => owner,
        Err(_) => panic!("contained owner spawn"),
    };
    let (proof_ready, release_proof) = if hold_before_native_proof {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        owner.settlement_hook = Some(TargetSettlementHook {
            before_native_proof: Some(ready_tx),
            release_native_proof: Some(release_rx),
            settlement_window: None,
            native_observations: std::collections::VecDeque::new(),
            panic_after_native_proof_release: false,
            panic_after_native_query_defect: false,
            stdout_reader: None,
        });
        (Some(ready_rx), Some(release_tx))
    } else {
        (None, None)
    };
    gate.owner_installed(&target.command, work, epoch);
    if work == TargetWork::PostProbe {
        emit_all(
            &handle,
            "agent_update_command_verifying",
            json!(command_ref(&target.command, &target.label)),
        );
    }
    let mut terminalizer =
        TargetTerminalizer::new(handle.clone(), Arc::clone(&gate), target.clone());
    terminalizer.store_install_before(install_before.clone());
    let owner_task = tokio::spawn({
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            let waited = owner.wait(Duration::from_secs(30), &mut cancel).await;
            assert!(matches!(waited, TargetWait::Cancelled));
            gate.owner_observed_cancellation(&target.command, work, epoch);
            let settlement = owner.settle(&waited).await;
            assert!(
                settlement.defects.is_empty(),
                "unexpected cleanup defects: {:?}",
                settlement.defects
            );
            assert!(gate.owner_returned(&target.command, work, epoch));
            terminalizer
                .publish(cancelled_update_result(&target, Some(install_before)))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let response = cancel_update(&handle, &gate, target.command.clone()).await;
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    if let Some(proof_ready) = proof_ready {
        proof_ready
            .await
            .expect("owner reached the native settlement proof");
        assert!(
            !owner_task.is_finished(),
            "terminal publication must remain behind native settlement proof"
        );
        assert!(
            gate.snapshot().results.is_empty(),
            "no terminal result is visible before native settlement proof"
        );
        release_proof
            .expect("proof release sender")
            .send(())
            .expect("release native settlement proof");
    }
    let result = tokio::time::timeout(Duration::from_secs(15), owner_task)
        .await
        .expect("owner settles")
        .expect("owner task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Cancelled);
    PassSupervisor {
        app: handle,
        gate: Arc::clone(&gate),
    }
    .finalize(PassRuntime {
        started: true,
        ..PassRuntime::default()
    })
    .await;
    drain_frames(&mut frames)
}

#[cfg(any(windows, unix))]
async fn updater_settlement_cancellation_case(
    settlement_window: Option<Duration>,
    native_observations: std::collections::VecDeque<TargetNativeObservation>,
    hold_native_proof: bool,
) -> (AgentUpdateResult, Vec<String>, Vec<Value>) {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = UpdateTarget {
        command: "settling-update".to_string(),
        label: "Settling update".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let (epoch, _) = gate
        .begin_step(&target.command, TargetWork::UpdaterStep(0))
        .expect("updater begin");
    let StepAdmission::Committed { epoch, mut cancel } =
        gate.commit_step(&target.command, TargetWork::UpdaterStep(0), epoch)
    else {
        panic!("updater commit");
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/D").arg("/C").arg("exit 0");
        command
    };
    #[cfg(unix)]
    let mut command = {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg("exit 0");
        command.process_group(0);
        command
    };
    command.current_dir(dir.path());
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    let Ok(mut owner) = TargetProcessOwner::spawn(&mut command).await else {
        panic!("real contained updater owner");
    };
    let (proof_ready_tx, proof_ready_rx) = tokio::sync::oneshot::channel();
    let (release_proof_tx, release_proof_rx) = tokio::sync::oneshot::channel();
    owner.settlement_hook = Some(TargetSettlementHook {
        before_native_proof: Some(proof_ready_tx),
        release_native_proof: hold_native_proof.then_some(release_proof_rx),
        settlement_window,
        native_observations,
        panic_after_native_proof_release: false,
        panic_after_native_query_defect: false,
        stdout_reader: None,
    });
    gate.owner_installed(&target.command, TargetWork::UpdaterStep(0), epoch);
    let waited = owner.wait(Duration::from_secs(10), &mut cancel).await;
    assert!(matches!(waited, TargetWait::Exited(Ok(_))));
    let owner_task = tokio::spawn({
        let gate = Arc::clone(&gate);
        let target = target.clone();
        let handle = handle.clone();
        async move {
            let settlement = owner
                .settle_observing_cancellation(&waited, &mut cancel, || {
                    gate.owner_observed_cancellation(
                        &target.command,
                        TargetWork::UpdaterStep(0),
                        epoch,
                    );
                })
                .await;
            let defects = settlement.defects.clone();
            assert!(gate.owner_returned(&target.command, TargetWork::UpdaterStep(0), epoch));
            let candidate = if defects.is_empty() {
                cancelled_update_result(&target, Some(installed("1.0.0")))
            } else {
                failed_update_result(
                    &target,
                    Some(installed("1.0.0")),
                    None,
                    CANCELLATION_CLEANUP_ERROR,
                )
            };
            let result = TargetTerminalizer::new(handle, Arc::clone(&gate), target)
                .publish(candidate)
                .await;
            (result, defects)
        }
    });
    tokio::time::timeout(Duration::from_secs(5), proof_ready_rx)
        .await
        .expect("updater reached native proof")
        .expect("direct handle dropped before native proof");
    let cancellation = tokio::spawn({
        let gate = Arc::clone(&gate);
        let handle = handle.clone();
        let command = target.command.clone();
        async move { cancel_update(&handle, &gate, command).await }
    });
    let changed = next_frame(&mut frames).await;
    assert_eq!(changed["event"], "agent_update_cancellation_changed");
    let response = tokio::time::timeout(Duration::from_secs(2), cancellation)
        .await
        .expect("settlement cancellation response precedes native proof")
        .expect("settlement cancellation task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    assert!(!owner_task.is_finished());
    assert!(gate.snapshot().results.is_empty());
    if hold_native_proof {
        release_proof_tx.send(()).expect("release native proof");
    }
    let (result, defects) = tokio::time::timeout(Duration::from_secs(15), owner_task)
        .await
        .expect("settling owner completed after native proof")
        .expect("settling owner task");
    PassSupervisor {
        app: handle,
        gate: Arc::clone(&gate),
    }
    .finalize(PassRuntime {
        started: true,
        ..PassRuntime::default()
    })
    .await;
    let mut events = vec![changed];
    events.extend(drain_frames(&mut frames));
    (result, defects, events)
}

#[tokio::test]
async fn cancel_during_verifying_kills_probe_tree_and_terminalizes_cancelled() {
    let (app, mut frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = UpdateTarget {
        command: if cfg!(windows) {
            "cmd.exe".to_string()
        } else {
            "sh".to_string()
        },
        label: "Verification probe".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    seed_retained_target(&gate, &target.command, &target.label);
    let install_before = InstallState::missing("pre-probe state".to_string());
    let mut terminalizer =
        TargetTerminalizer::new(handle.clone(), Arc::clone(&gate), target.clone());
    terminalizer.store_install_before(install_before.clone());
    let (proof_ready_tx, proof_ready_rx) = tokio::sync::oneshot::channel();
    let (release_proof_tx, release_proof_rx) = tokio::sync::oneshot::channel();
    let probe_task = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let target = target.clone();
        async move {
            let stem = if cfg!(windows) { "cmd" } else { "sh" };
            let args: &'static [&'static str] = if cfg!(windows) {
                &["/D", "/C", "cmd /C ping -n 30 127.0.0.1 >NUL"]
            } else {
                &["-c", "sh -c 'sleep 30' & wait"]
            };
            let outcome = crate::agent_version::with_version_probe_args_for_test(
                stem,
                args,
                crate::agent_version::with_probe_settlement_hook(
                    crate::agent_version::ProbeSettlementHook {
                        before_native_proof: Some(proof_ready_tx),
                        release_native_proof: Some(release_proof_rx),
                        settlement_window: None,
                        native_observations: std::collections::VecDeque::new(),
                        panic_after_native_proof_release: false,
                        panic_after_native_query_defect: false,
                        stdout_reader: None,
                    },
                    async {
                        let mut owner = TargetOwnerSlot::default();
                        run_target_probe(&handle, &gate, &target, TargetWork::PostProbe, &mut owner)
                            .await
                    },
                ),
            )
            .await;
            assert!(matches!(outcome, TargetProbeOutcome::Cancelled));
            terminalizer
                .publish(cancelled_update_result(&target, Some(install_before)))
                .await
        }
    });

    let verifying = next_frame(&mut frames).await;
    assert_eq!(verifying["event"], "agent_update_command_verifying");
    assert_eq!(verifying["payload"]["command"], target.command);
    let cancellation = tokio::spawn({
        let handle = handle.clone();
        let gate = Arc::clone(&gate);
        let command = target.command.clone();
        async move { cancel_update(&handle, &gate, command).await }
    });
    let changed = next_frame(&mut frames).await;
    assert_eq!(changed["event"], "agent_update_cancellation_changed");
    proof_ready_rx
        .await
        .expect("real ProbeProcessOwner reached native proof");
    let response = tokio::time::timeout(Duration::from_secs(2), cancellation)
        .await
        .expect("cancellation response precedes native proof release")
        .expect("cancellation task");
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    assert!(!probe_task.is_finished());
    assert!(gate.snapshot().results.is_empty());
    assert!(
        frames.try_recv().is_err(),
        "terminal event remains proof-blocked"
    );
    release_proof_tx
        .send(())
        .expect("release probe native proof");
    let result = probe_task.await.expect("real verification probe task");
    assert_eq!(result.outcome, AgentUpdateOutcome::Cancelled);
    PassSupervisor {
        app: handle,
        gate: Arc::clone(&gate),
    }
    .finalize(PassRuntime {
        started: true,
        ..PassRuntime::default()
    })
    .await;
    let mut events = vec![verifying, changed];
    events.extend(drain_frames(&mut frames));
    let names: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event"].as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "agent_update_command_verifying",
            "agent_update_cancellation_changed",
            "agent_update_command_finished",
            "agent_updates_finished"
        ]
    );
    assert_eq!(events[2]["payload"]["outcome"], "cancelled");
    assert!(events[2]["payload"]["installAfter"].is_null());
}

#[cfg(windows)]
#[tokio::test]
async fn cancelled_contained_update() {
    let events = exercise_contained_update_cancellation(TargetWork::UpdaterStep(0), false).await;
    assert!(events.iter().any(|event| {
        event["event"] == "agent_update_command_finished"
            && event["payload"]["outcome"] == "cancelled"
    }));
}

#[cfg(windows)]
#[tokio::test]
async fn cancelled_contained_update_waits_for_job_active_zero_before_finish() {
    let events = exercise_contained_update_cancellation(TargetWork::UpdaterStep(0), true).await;
    let command = events
        .iter()
        .position(|event| event["event"] == "agent_update_command_finished")
        .expect("command terminal event");
    let pass = events
        .iter()
        .position(|event| event["event"] == "agent_updates_finished")
        .expect("pass terminal event");
    assert!(
        command < pass,
        "native active-zero proof precedes both events"
    );

    let observations = std::collections::VecDeque::from([
        TargetNativeObservation::Active,
        TargetNativeObservation::Active,
        TargetNativeObservation::Active,
        TargetNativeObservation::Error("injected query error"),
        TargetNativeObservation::Empty,
    ]);
    let (result, defects, events) =
        updater_settlement_cancellation_case(Some(Duration::from_millis(60)), observations, true)
            .await;
    assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(result.error.as_deref(), Some(CANCELLATION_CLEANUP_ERROR));
    assert!(defects.iter().any(|defect| {
        defect == "settlement attempt deadline exceeded during job accounting proof"
    }));
    assert!(
        defects
            .iter()
            .any(|defect| defect == "job accounting failed: injected query error"),
        "later-settled query defect was not retained: {defects:?}"
    );
    let command = events
        .iter()
        .position(|event| event["event"] == "agent_update_command_finished")
        .expect("defective command terminal event");
    let pass = events
        .iter()
        .position(|event| event["event"] == "agent_updates_finished")
        .expect("defective pass terminal event");
    assert!(command < pass);
    assert_eq!(events[command]["payload"]["outcome"], "failed");
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_update_kills_unix_process_group() {
    let events = exercise_contained_update_cancellation(TargetWork::UpdaterStep(0), false).await;
    assert!(events.iter().any(|event| {
        event["event"] == "agent_update_command_finished"
            && event["payload"]["outcome"] == "cancelled"
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_update_waits_for_group_esrch_before_finish() {
    let events = exercise_contained_update_cancellation(TargetWork::UpdaterStep(0), true).await;
    let command = events
        .iter()
        .position(|event| event["event"] == "agent_update_command_finished")
        .expect("command terminal event");
    let pass = events
        .iter()
        .position(|event| event["event"] == "agent_updates_finished")
        .expect("pass terminal event");
    assert!(command < pass, "group ESRCH proof precedes pass finish");

    let observations = std::collections::VecDeque::from([
        TargetNativeObservation::Active,
        TargetNativeObservation::Active,
        TargetNativeObservation::Active,
        TargetNativeObservation::Error("EPERM"),
        TargetNativeObservation::Empty,
    ]);
    let (result, defects, events) =
        updater_settlement_cancellation_case(Some(Duration::from_millis(60)), observations, true)
            .await;
    assert_eq!(result.outcome, AgentUpdateOutcome::Failed);
    assert_eq!(result.error.as_deref(), Some(CANCELLATION_CLEANUP_ERROR));
    assert!(defects.iter().any(|defect| {
        defect == "settlement attempt deadline exceeded during process-group accounting proof"
    }));
    assert!(
        defects
            .iter()
            .any(|defect| defect == "process-group accounting failed: EPERM"),
        "later-settled EPERM defect was not retained: {defects:?}"
    );
    let command = events
        .iter()
        .position(|event| event["event"] == "agent_update_command_finished")
        .expect("defective command terminal event");
    let pass = events
        .iter()
        .position(|event| event["event"] == "agent_updates_finished")
        .expect("defective pass terminal event");
    assert!(command < pass);
    assert_eq!(events[command]["payload"]["outcome"], "failed");
}

#[tokio::test]
async fn per_row_cancellation_preserves_peer_completion() {
    let (app, _frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let target_cwd = dir.path().to_path_buf();
    let slow_step = if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    };
    let slow = UpdateTarget {
        command: "slow-missing".to_string(),
        label: "Slow".to_string(),
        commands: vec![slow_step.to_string()],
        cwd: target_cwd.clone(),
    };
    let peer = UpdateTarget {
        command: "peer-missing".to_string(),
        label: "Peer".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: target_cwd,
    };
    gate.mark_started_with_nodes(vec![
        node(&slow.command, &slow.label, vec![slow_step]),
        node(&peer.command, &peer.label, vec!["exit 0"]),
    ]);
    let slow_task = tokio::spawn(run_update_target(
        handle.clone(),
        Arc::clone(&gate),
        slow.clone(),
    ));
    let peer_task = tokio::spawn(run_update_target(
        handle.clone(),
        Arc::clone(&gate),
        peer.clone(),
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gate
        .snapshot()
        .running
        .iter()
        .any(|entry| entry.command == slow.command)
    {
        assert!(Instant::now() < deadline, "slow owner did not start");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let response = cancel_update(&handle, &gate, slow.command.clone()).await;
    assert_eq!(
        response.disposition,
        AgentUpdateCancelDisposition::Requested
    );
    let slow_result = slow_task.await.expect("slow task");
    let peer_result = peer_task.await.expect("peer task");
    assert_eq!(slow_result.outcome, AgentUpdateOutcome::Cancelled);
    assert_eq!(peer_result.outcome, AgentUpdateOutcome::Succeeded);
}

#[tokio::test]
async fn cancel_all_is_pass_ordered_and_idempotent() {
    let (app, _frames) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    gate.mark_started_with_nodes(vec![
        node("pending", "Pending", vec!["exit 0"]),
        node("prompt", "Prompt", vec!["exit 0"]),
        node("starting", "Starting", vec!["exit 0"]),
        node("done", "Done", vec!["exit 0"]),
    ]);
    let _prompt_rx = gate.register_prompt("prompt", "Prompt");
    let _ = gate
        .begin_step("starting", TargetWork::UpdaterStep(0))
        .expect("starting target");
    let done = UpdateTarget {
        command: "done".to_string(),
        label: "Done".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: cwd(),
    };
    TargetTerminalizer::new(handle.clone(), Arc::clone(&gate), done.clone())
        .publish(successful_update_result(
            &done,
            installed("1.0.0"),
            installed("1.0.0"),
        ))
        .await;

    let first = cancel_all_updates(&handle, &gate).await;
    assert_eq!(
        first
            .requested
            .iter()
            .map(|entry| entry.command.as_str())
            .collect::<Vec<_>>(),
        vec!["pending", "prompt", "starting"]
    );
    assert_eq!(first.already_terminal, vec![command_ref("done", "Done")]);
    assert!(gate.snapshot().cancel_all_requested);
    let repeated = cancel_all_updates(&handle, &gate).await;
    assert!(repeated.requested.is_empty());
    assert_eq!(repeated.already_requested, first.requested);
    assert_eq!(repeated.already_terminal, first.already_terminal);

    for (command, label) in [
        ("pending", "Pending"),
        ("prompt", "Prompt"),
        ("starting", "Starting"),
    ] {
        let target = UpdateTarget {
            command: command.to_string(),
            label: label.to_string(),
            commands: vec!["exit 0".to_string()],
            cwd: cwd(),
        };
        TargetTerminalizer::new(handle.clone(), Arc::clone(&gate), target.clone())
            .publish(cancelled_update_result(&target, None))
            .await;
    }
    let all_terminal = cancel_all_updates(&handle, &gate).await;
    assert!(all_terminal.requested.is_empty());
    assert!(all_terminal.already_requested.is_empty());
    assert_eq!(
        all_terminal
            .already_terminal
            .iter()
            .map(|entry| entry.command.as_str())
            .collect::<Vec<_>>(),
        vec!["pending", "prompt", "starting", "done"]
    );
    assert!(gate.snapshot().cancel_all_requested);
}
