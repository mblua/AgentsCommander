use super::*;

#[tokio::test]
async fn gate_wait_returns_after_finish_and_releases_all_waiters() {
    let gate = Arc::new(AgentUpdateGate::new());
    let gate2 = Arc::clone(&gate);
    let waiter = tokio::spawn(async move { gate2.wait_until_done().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    gate.mark_finished(vec![]);
    tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("waiter released")
        .expect("waiter ok");

    // late waiter returns immediately
    let gate3 = Arc::clone(&gate);
    let late = tokio::spawn(async move { gate3.wait_until_done().await });
    tokio::time::timeout(Duration::from_secs(2), late)
        .await
        .expect("late waiter released")
        .expect("late waiter ok");
}

#[tokio::test]
async fn gate_prompt_registry_and_snapshot() {
    let gate = AgentUpdateGate::new();
    gate.mark_started();
    let rx = gate.register_prompt("claude", "Claude");
    assert!(gate.was_prompted("claude"));
    let snap = gate.snapshot();
    assert!(snap.in_progress);
    assert_eq!(
        snap.prompt.as_ref().map(|p| p.command.as_str()),
        Some("claude")
    );

    assert!(gate.resolve_answer("claude", true));
    assert!(
        !gate.resolve_answer("claude", true),
        "second resolve must fail"
    );
    assert!(rx.await.expect("answer delivered"));
    assert!(gate.snapshot().prompt.is_none(), "resolved prompt cleared");

    // dropped-receiver path (round-3 F1): the prompt timeout drops the
    // answer channel; a late resolve must return false without panicking
    let gate2 = AgentUpdateGate::new();
    let rx_dropped = gate2.register_prompt("codex", "Codex");
    drop(rx_dropped);
    assert!(!gate2.resolve_answer("codex", true));
    assert!(
        gate2.snapshot().prompt.is_none(),
        "a dead prompt must not be resurrected by the snapshot"
    );

    // drop_pending path: no answer, snapshot must not resurrect the prompt
    let rx2 = gate.register_prompt("codex", "Codex");
    gate.drop_pending("codex");
    assert!(!gate.resolve_answer("codex", false));
    assert!(rx2.await.is_err(), "sender dropped");
    assert!(gate.snapshot().prompt.is_none());

    // mark_finished clears pending prompt and pending senders
    let rx3 = gate.register_prompt("pi", "Pi");
    gate.mark_finished(vec![AgentUpdateResult {
        command: "pi".to_string(),
        label: "Pi".to_string(),
        ok: true,
        outcome: AgentUpdateOutcome::Succeeded,
        error: None,
        install_before: None,
        install_after: None,
        change: AgentUpdateChange::Unknown,
        verification_error: None,
    }]);
    assert!(rx3.await.is_err(), "sender dropped by mark_finished");
    let snap = gate.snapshot();
    assert!(!snap.in_progress);
    assert!(snap.prompt.is_none());
    assert_eq!(snap.results.len(), 1);
    assert!(snap.results[0].ok);

    // idempotent: repeated mark_finished keeps finished and replaces results
    gate.mark_finished(vec![]);
    assert!(!gate.snapshot().in_progress);
    assert!(gate.snapshot().results.is_empty());
}

/// Platform-shell-aware runner tests. Every command string below is a full
/// shell string valid under BOTH cmd.exe /C and sh -c.
async fn run_in_tmpdir(commands: Vec<&str>, step_timeout: Duration) -> AgentUpdateResult {
    let dir = tempfile::tempdir().expect("tempdir");
    run_update_sequence(
        &UpdateTarget {
            command: "test".to_string(),
            label: "Test".to_string(),
            commands: commands.into_iter().map(str::to_string).collect(),
            cwd: dir.path().to_path_buf(),
        },
        step_timeout,
    )
    .await
}

#[tokio::test]
async fn runner_exit_zero_ok() {
    let result = run_in_tmpdir(vec!["exit 0"], UPDATE_STEP_TIMEOUT).await;
    assert!(result.ok, "unexpected: {result:?}");
    assert!(result.error.is_none());
}

#[tokio::test]
async fn runner_exit_nonzero_fails_with_code() {
    let result = run_in_tmpdir(vec!["exit 3"], UPDATE_STEP_TIMEOUT).await;
    assert!(!result.ok);
    assert_eq!(result.error.as_deref(), Some("exit code 3"));
}

#[tokio::test]
async fn runner_fail_fast_stops_sequence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("marker.txt");
    let result = run_update_sequence(
        &UpdateTarget {
            command: "test".to_string(),
            label: "Test".to_string(),
            commands: vec![
                "exit 3".to_string(),
                format!("echo x > {}", marker.to_string_lossy().replace('\\', "/")),
            ],
            cwd: dir.path().to_path_buf(),
        },
        UPDATE_STEP_TIMEOUT,
    )
    .await;
    assert!(!result.ok);
    assert!(!marker.exists(), "later step must not run after a failure");
}

#[tokio::test]
async fn runner_timeout_kills_tree() {
    // Windows: ping ~30s (cmd has no sleep); Unix: sleep 30. The step
    // timeout (200ms) must kill the whole tree; the test itself is bounded.
    let step = if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    };
    let started = std::time::Instant::now();
    let result = run_in_tmpdir(vec![step], Duration::from_millis(200)).await;
    assert!(!result.ok);
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("timed out")),
        "unexpected: {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "tree kill must be prompt, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn runner_success_with_descendant_holding_pipe() {
    // The parent exits 0 quickly while a descendant keeps the pipes open:
    // the reader-join timeout arm runs (warn + tree-kill FIRST + second
    // bounded join) and the step is reported SUCCESS with the full tail.
    let step = if cfg!(windows) {
        "cmd /C start /B ping -n 30 127.0.0.1 >NUL"
    } else {
        "sh -c 'sleep 30 &'"
    };
    let started = std::time::Instant::now();
    let result = run_in_tmpdir(vec![step], UPDATE_STEP_TIMEOUT).await;
    assert!(result.ok, "unexpected: {result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "descendant must be reaped promptly, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn runner_success_with_detached_survivor() {
    // round-3 F2 pin (deliberate platform divergence): a fully detached
    // (non-pipe-holding) descendant of the update step is reaped on
    // Windows by the per-step JobObject drop (KILL_ON_JOB_CLOSE: no
    // survivors from an update step on Windows) but SURVIVES on Unix (no
    // job; the group-kill runs only on timeout/truncation). The step
    // itself reports success on both platforms, via the plain-success
    // path, NOT the R1 reader-truncation arm: the survivor's handles are
    // redirected away from the step's pipes, so the readers EOF when the
    // parent exits. The survivor writes its marker only after a delay that
    // exceeds the step.
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("marker.txt");
    let marker_str = marker.to_string_lossy().replace('\\', "/");
    let step = if cfg!(windows) {
        // The plan's pinned `cmd /C start /B "" cmd /C "<cmd>"` shape was
        // VERIFIED to hang layer-2 cmd at `start` when spawned through the
        // runner's `cmd.exe /C` chain (start mangles the nested quotes
        // into a recursive `start "" "..."` command, observed in the
        // process tree), so the implementer pinned a quoting-free
        // equivalent with the SAME semantics (plan 9.1: implementer
        // verifies both arms and pins the exact quoting): `start` an exe
        // target - powershell, whose -EncodedCommand carries the whole
        // script with no quotes/spaces - with the handles redirected to
        // NUL on the start line. PowerShell exits right after
        // Start-Process returns (no wait); the detached survivor (the
        // Start-Process child, in the step's job, NOT inheriting the
        // step's pipes) pings ~2s and only then writes the marker - after
        // the step end, so the per-step job drop must reap it before the
        // write.
        use base64::Engine as _;
        let script = format!(
            "Start-Process cmd -ArgumentList '/c','ping -n 3 127.0.0.1 >NUL & echo x > \
            {marker_str}' -WindowStyle Hidden"
        );
        let mut utf16 = Vec::new();
        for u in script.encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16);
        format!("start /B powershell -NoProfile -EncodedCommand {encoded} >NUL 2>NUL")
    } else {
        format!("sh -c '(sleep 1; echo x > {marker_str}) >/dev/null 2>&1 &'")
    };
    let started = std::time::Instant::now();
    let result = run_update_sequence(
        &UpdateTarget {
            command: "test".to_string(),
            label: "Test".to_string(),
            commands: vec![step],
            cwd: dir.path().to_path_buf(),
        },
        UPDATE_STEP_TIMEOUT,
    )
    .await;
    assert!(result.ok, "unexpected: {result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "completion bounded, took {:?}",
        started.elapsed()
    );
    if cfg!(windows) {
        // The per-step job drop reaped the survivor before its delayed
        // marker write.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!marker.exists(), "Windows survivor must have been reaped");
    } else {
        // No job on Unix: the detached descendant survives the step and
        // writes the marker within ~3s.
        for _ in 0..30 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(marker.exists(), "Unix survivor must write the marker");
    }
}

#[tokio::test]
async fn pending_prompt_release_requires_only_concurrent_runtime_work() {
    // #1341: pins the cooperative gate-release invariant the spawned
    // restore task relies on (the answerer task is the `agent_update_answer`
    // command; the waiter is the restore wake; both must progress on the
    // same runtime without the main thread doing anything). It does NOT
    // reproduce the #1341 freeze itself (the gate was cooperative pre-fix;
    // the bug was the lib.rs setup `block_on`, unreachable from a unit
    // test) - the freeze regression net is AC2 + AC8 + AC9 + AC11. If the
    // gate wait were ever made thread-blocking, this test would hang on the
    // single-threaded tokio test runtime and the timeout would fail.
    let gate = Arc::new(AgentUpdateGate::new());
    gate.mark_started();
    let _rx = gate.register_prompt("claude", "Claude");
    let waiter = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move { gate.wait_until_done().await }
    });
    let answerer = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert!(
                gate.resolve_answer("claude", false),
                "prompt must still be pending"
            );
            gate.mark_finished(vec![]);
        }
    });
    tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("gate waiter must be released by concurrent runtime work")
        .expect("waiter ok");
    answerer.await.expect("answerer ok");
}
