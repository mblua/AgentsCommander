use super::*;

// ---------------------------------------------------------------------
// #2736 - silent coding-agent install runner
// ---------------------------------------------------------------------

const INSTALL_FRAME_CAP: Duration = Duration::from_secs(30);

/// Next `coding_agent_install_finished` frame; every frame seen on the way
/// (including it) is appended to `seen`.
async fn next_install_frame(
    rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>,
    seen: &mut Vec<Value>,
) -> Value {
    let started = Instant::now();
    loop {
        if let Ok(frame) = rx.try_recv() {
            let frame = decode_frame(frame);
            seen.push(frame.clone());
            if frame["event"] == "coding_agent_install_finished" {
                return frame;
            }
            continue;
        }
        assert!(
            started.elapsed() < INSTALL_FRAME_CAP,
            "no install frame arrived within the bound"
        );
        tokio::time::sleep(POLL_STEP).await;
    }
}

fn install_in_flight(command: &str) -> bool {
    contains_install_command(command)
}

/// A shell command valid under the host shell: `windows` for cmd.exe /C,
/// `unix` for sh -c.
fn shell(windows: &str, unix: &str) -> String {
    if cfg!(windows) {
        windows.to_string()
    } else {
        unix.to_string()
    }
}

/// Appends `tag` to `spawned.marker`, then sleeps about two seconds. Each
/// caller passes its own tag, so it owns a distinct in-flight key.
fn short_sleep(tag: &str) -> String {
    shell(
        &format!("echo {tag}>>spawned.marker & ping -n 3 127.0.0.1 >NUL"),
        &format!("echo {tag} >> spawned.marker; sleep 2"),
    )
}

fn long_sleep(tag: &str) -> String {
    shell(
        &format!("echo {tag} & ping -n 30 127.0.0.1 >NUL"),
        &format!("echo {tag}; sleep 30"),
    )
}

fn install_run(ok: bool, exit_code: Option<i32>, detail: &str) -> AgentInstallRun {
    AgentInstallRun {
        ok,
        stdout: b"out-text".to_vec(),
        stderr: b"err-text".to_vec(),
        exit_code,
        detail: detail.to_string(),
    }
}

const TRUNCATION_MARKER: &str =
    "[... truncated; showing the last 8192 bytes. The full output is in the app log.]\n";

#[tokio::test]
async fn agent_install_2736_success_logs_full_output_and_emits_ok_true() {
    let (app, mut rx) = app_with_broadcaster();
    let dir = tempfile::tempdir().expect("tempdir");
    let command = shell(
        "echo FIRSTBYTE-2736& for /L %i in (1,1,300) do @echo \
        0123456789012345678901234567890123456789",
        "echo FIRSTBYTE-2736; i=0; while [ $i -lt 300 ]; do echo \
        0123456789012345678901234567890123456789; i=$((i+1)); done",
    );
    let run = run_agent_install(
        app.handle(),
        "claude",
        &command,
        dir.path(),
        UPDATE_STEP_TIMEOUT,
    )
    .await;
    assert!(run.ok, "detail: {}", run.detail);
    assert!(run.stdout.len() > OUTPUT_TAIL_BYTES);
    assert!(
        run.stdout.starts_with(b"FIRSTBYTE-2736"),
        "the complete output keeps the first byte output_tail would drop"
    );
    assert!(!output_tail(&run.stdout).starts_with("FIRSTBYTE-2736"));
    let lines = agent_install_log_lines("claude", &command, &run);
    let stdout_line = &lines[0].1;
    assert!(
        stdout_line.starts_with(&format!(
            "[agent-install] stdout for claude ({command}): FIRSTBYTE-2736"
        )),
        "the log line carries the first byte"
    );
    assert_eq!(
        stdout_line.len(),
        format!("[agent-install] stdout for claude ({command}): ").len() + run.stdout.len(),
        "the log line carries the COMPLETE stdout"
    );

    start_agent_install(
        app.handle(),
        "claude".to_string(),
        command.clone(),
        dir.path().to_path_buf(),
    )
    .expect("install accepted");
    let frame = next_install_frame(&mut rx, &mut Vec::new()).await;
    assert_eq!(frame["event"], "coding_agent_install_finished");
    assert_eq!(frame["payload"]["ok"], json!(true));
    assert_eq!(frame["payload"]["command"], json!(command));
}

#[tokio::test]
async fn agent_install_2736_non_zero_exit_emits_ok_false_and_logs_stderr() {
    let (app, mut rx) = app_with_broadcaster();
    let dir = tempfile::tempdir().expect("tempdir");
    let command = shell("echo boom-2736 1>&2 & exit 3", "echo boom-2736 >&2; exit 3");
    let run = run_agent_install(
        app.handle(),
        "claude",
        &command,
        dir.path(),
        UPDATE_STEP_TIMEOUT,
    )
    .await;
    assert!(!run.ok);
    assert_eq!(run.exit_code, Some(3));
    assert!(String::from_utf8_lossy(&run.stderr).contains("boom-2736"));
    let lines = agent_install_log_lines("claude", &command, &run);
    assert!(lines.iter().any(|(level, line)| *level == log::Level::Info
        && line.starts_with(&format!(
            "[agent-install] stderr for claude ({command}): boom-2736"
        ))));
    assert_eq!(
        lines.last().expect("outcome line"),
        &(
            log::Level::Warn,
            format!("[agent-install] claude ({command}) FAILED: exit code 3")
        )
    );

    start_agent_install(
        app.handle(),
        "claude".to_string(),
        command,
        dir.path().to_path_buf(),
    )
    .expect("install accepted");
    let frame = next_install_frame(&mut rx, &mut Vec::new()).await;
    assert_eq!(frame["payload"]["ok"], json!(false));
    assert_eq!(frame["payload"]["detail"], json!("exit code 3"));
    assert!(frame["payload"]["stderr"]
        .as_str()
        .expect("stderr field")
        .contains("boom-2736"));
}

#[tokio::test]
async fn agent_install_2736_timeout_kills_the_tree_and_emits_ok_false() {
    let app = build_mock_app(crate::test_support::test_builder());
    let dir = tempfile::tempdir().expect("tempdir");
    let started = Instant::now();
    let run = run_agent_install(
        app.handle(),
        "claude",
        &long_sleep("timeout-2736"),
        dir.path(),
        Duration::from_millis(200),
    )
    .await;
    assert!(!run.ok);
    assert_eq!(run.detail, "timed out after 0s (killed)");
    assert_eq!(run.exit_code, None);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "tree kill must be prompt, took {:?}",
        started.elapsed()
    );

    // The same outcome through the real emit path.
    let (app, mut rx) = app_with_broadcaster();
    let command = long_sleep("timeout-frame-2736");
    start_agent_install_with_timeout(
        app.handle(),
        "claude".to_string(),
        command.clone(),
        dir.path().to_path_buf(),
        Duration::from_millis(200),
    )
    .expect("install accepted");
    let frame = next_install_frame(&mut rx, &mut Vec::new()).await;
    assert_eq!(frame["payload"]["command"], json!(command));
    assert_eq!(frame["payload"]["ok"], json!(false));
    assert_eq!(
        frame["payload"]["detail"],
        json!("timed out after 0s (killed)")
    );
    assert_eq!(frame["payload"]["exitCode"], Value::Null);
}

#[tokio::test]
async fn agent_install_2736_second_install_for_the_same_command_is_refused() {
    let (app, mut rx) = app_with_broadcaster();
    let dir = tempfile::tempdir().expect("tempdir");
    let command = short_sleep("refused-2736");
    start_agent_install(
        app.handle(),
        "claude".to_string(),
        command.clone(),
        dir.path().to_path_buf(),
    )
    .expect("first install accepted");
    let second = start_agent_install(
        app.handle(),
        "other-key".to_string(),
        command.clone(),
        dir.path().to_path_buf(),
    );
    assert_eq!(
        second,
        Err("an install is already running for that command".to_string())
    );
    assert!(
        drain_frames(&mut rx).is_empty(),
        "the refusal answered while the first install was still running"
    );
    next_install_frame(&mut rx, &mut Vec::new()).await;
    let spawned =
        std::fs::read_to_string(dir.path().join("spawned.marker")).expect("the first install ran");
    assert_eq!(spawned.lines().count(), 1, "only ONE process was spawned");
}

#[tokio::test]
async fn agent_install_2736_in_flight_entry_is_released_on_every_outcome() {
    let (app, mut rx) = app_with_broadcaster();
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        (
            shell("echo released-ok-2736", "echo released-ok-2736"),
            true,
        ),
        (
            shell(
                "echo released-fail-2736 & exit 4",
                "echo released-fail-2736; exit 4",
            ),
            false,
        ),
        (long_sleep("released-timeout-2736"), false),
    ];
    for (command, ok) in cases {
        start_agent_install_with_timeout(
            app.handle(),
            "claude".to_string(),
            command.clone(),
            dir.path().to_path_buf(),
            Duration::from_millis(500),
        )
        .expect("install accepted");
        assert!(install_in_flight(&command));
        let frame = next_install_frame(&mut rx, &mut Vec::new()).await;
        assert_eq!(frame["payload"]["ok"], json!(ok), "{command}");
        assert!(!install_in_flight(&command), "{command} leaked its entry");
        drop(InstallInFlight::acquire(&command).expect("a retry is accepted"));
    }
}

#[tokio::test]
async fn agent_install_2736_two_different_commands_run_concurrently() {
    let (app, mut rx) = app_with_broadcaster();
    let dir = tempfile::tempdir().expect("tempdir");
    let first = short_sleep("concurrent-a-2736");
    let second = short_sleep("concurrent-b-2736");
    for command in [&first, &second] {
        start_agent_install(
            app.handle(),
            "claude".to_string(),
            command.clone(),
            dir.path().to_path_buf(),
        )
        .expect("install accepted");
    }
    assert!(install_in_flight(&first) && install_in_flight(&second));
    let mut seen = Vec::new();
    next_install_frame(&mut rx, &mut seen).await;
    next_install_frame(&mut rx, &mut seen).await;
    let mut commands: Vec<String> = seen
        .iter()
        .map(|frame| frame["payload"]["command"].as_str().unwrap().to_string())
        .collect();
    commands.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(commands, expected);
}

#[tokio::test]
async fn agent_install_2736_emits_exactly_one_frame_per_run() {
    let (app, mut rx) = app_with_broadcaster();
    let dir = tempfile::tempdir().expect("tempdir");
    start_agent_install(
        app.handle(),
        "claude".to_string(),
        shell("echo one-frame-2736", "echo one-frame-2736"),
        dir.path().to_path_buf(),
    )
    .expect("install accepted");
    let mut seen = Vec::new();
    next_install_frame(&mut rx, &mut seen).await;
    tokio::time::sleep(QUIET_WINDOW).await;
    seen.extend(drain_frames(&mut rx));
    assert_eq!(seen.len(), 1, "frames: {seen:?}");
}

#[tokio::test]
async fn agent_install_2736_never_emits_an_agent_update_event() {
    let broadcaster = WsBroadcaster::new();
    let mut rx = broadcaster.subscribe();
    let gate = Arc::new(AgentUpdateGate::new());
    let app = build_mock_app(
        crate::test_support::test_builder()
            .manage(broadcaster)
            .manage(Arc::clone(&gate)),
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let before = serde_json::to_value(gate.snapshot()).expect("status json");
    let mut seen = Vec::new();
    for command in [
        shell("echo no-update-ok-2736", "echo no-update-ok-2736"),
        shell(
            "echo no-update-fail-2736 & exit 5",
            "echo no-update-fail-2736; exit 5",
        ),
    ] {
        start_agent_install(
            app.handle(),
            "claude".to_string(),
            command,
            dir.path().to_path_buf(),
        )
        .expect("install accepted");
        next_install_frame(&mut rx, &mut seen).await;
    }
    tokio::time::sleep(QUIET_WINDOW).await;
    seen.extend(drain_frames(&mut rx));
    assert_eq!(seen.len(), 2, "frames: {seen:?}");
    seen.retain(|frame| {
        let event = frame["event"].as_str().unwrap_or_default();
        event.starts_with("agent_update") || event == "agent_install_state_changed"
    });
    assert!(seen.is_empty(), "update frames: {seen:?}");
    assert_eq!(
        serde_json::to_value(gate.snapshot()).expect("status json"),
        before
    );
}

#[test]
fn agent_install_2736_runner_uses_the_contained_silent_spawn() {
    let src = include_str!("../install.rs").replace("\r\n", "\n");
    let start = src
        .find("\npub(crate) async fn run_agent_install(")
        .expect("fn run_agent_install");
    let end = start
        + src[start..]
            .find("\n}\n")
            .expect("end of run_agent_install");
    let body = &src[start..end];
    assert!(body.contains("TargetProcessOwner::spawn_step("));
    assert!(body.contains("effective_search_path()"));
    assert!(!body.contains("build_update_step_command("));
    assert!(!body.contains("TargetProcessOwner::spawn("));
    assert!(!body.contains("Command::new"));
}

#[tokio::test]
async fn agent_install_2736_cleanup_defects_make_a_zero_exit_not_ok() {
    let app = build_mock_app(crate::test_support::test_builder());
    let dir = tempfile::tempdir().expect("tempdir");
    let defect_hook = || TargetSettlementHook {
        before_native_proof: None,
        release_native_proof: None,
        settlement_window: None,
        native_observations: std::collections::VecDeque::from([
            TargetNativeObservation::Error("injected install cleanup defect"),
            TargetNativeObservation::Empty,
        ]),
        panic_after_native_proof_release: false,
        panic_after_native_query_defect: false,
        stdout_reader: None,
    };
    let run = with_target_settlement_hook(
        defect_hook(),
        run_agent_install(
            app.handle(),
            "claude",
            "exit 0",
            dir.path(),
            UPDATE_STEP_TIMEOUT,
        ),
    )
    .await;
    assert!(!run.ok);
    assert!(
        run.detail.starts_with("process cleanup defective: ")
            && run.detail.contains("injected install cleanup defect"),
        "detail: {}",
        run.detail
    );
    assert_eq!(
        run.exit_code,
        Some(0),
        "the observed status survives the defect"
    );

    // The same outcome through the real emit path: the hook rides into
    // the detached task that `start_agent_install` spawns.
    let (app, mut rx) = app_with_broadcaster();
    let command = shell("echo defect-frame-2736", "echo defect-frame-2736");
    with_target_settlement_hook(defect_hook(), async {
        start_agent_install(
            app.handle(),
            "claude".to_string(),
            command.clone(),
            dir.path().to_path_buf(),
        )
    })
    .await
    .expect("install accepted");
    let frame = next_install_frame(&mut rx, &mut Vec::new()).await;
    assert_eq!(frame["payload"]["command"], json!(command));
    assert_eq!(frame["payload"]["ok"], json!(false));
    assert_eq!(frame["payload"]["exitCode"], json!(0));
    assert!(frame["payload"]["detail"]
        .as_str()
        .expect("detail")
        .contains("injected install cleanup defect"));
}

#[tokio::test]
async fn agent_install_2736_maps_every_outcome_arm() {
    let app = build_mock_app(crate::test_support::test_builder());
    let dir = tempfile::tempdir().expect("tempdir");
    let here = dir.path().to_path_buf();
    let missing = dir.path().join("missing-cwd-2736");
    // The same builder the runner uses, so the expected text is the OS
    // error for exactly this launch.
    let spawn_error = build_update_step_command("exit 0", &missing, &effective_search_path())
        .spawn()
        .expect_err("a missing cwd cannot be launched")
        .to_string();
    let rows = [
        (
            "exit 0".to_string(),
            here.clone(),
            UPDATE_STEP_TIMEOUT,
            true,
            Some(0),
            "ok".to_string(),
        ),
        (
            "exit 3".to_string(),
            here.clone(),
            UPDATE_STEP_TIMEOUT,
            false,
            Some(3),
            "exit code 3".to_string(),
        ),
        (
            long_sleep("arms-2736"),
            here,
            Duration::from_millis(200),
            false,
            None,
            "timed out after 0s (killed)".to_string(),
        ),
        (
            "exit 0".to_string(),
            missing,
            UPDATE_STEP_TIMEOUT,
            false,
            None,
            spawn_error,
        ),
    ];
    for (command, cwd, timeout, ok, exit_code, detail) in rows {
        let run = run_agent_install(app.handle(), "claude", &command, &cwd, timeout).await;
        assert_eq!(run.ok, ok, "{command}");
        assert_eq!(run.exit_code, exit_code, "{command}");
        assert_eq!(run.detail, detail, "{command}");
    }
}

#[test]
fn agent_install_2736_failure_payload_carries_detail_exit_code_and_both_streams() {
    let run = install_run(false, Some(3), "exit code 3");
    assert_eq!(
        install_finished_payload("claude", "npm i -g x", &run),
        json!({
            "key": "claude",
            "command": "npm i -g x",
            "ok": false,
            "detail": "exit code 3",
            "exitCode": 3,
            "stdout": "out-text",
            "stderr": "err-text",
        })
    );
}

#[test]
fn agent_install_2736_success_payload_carries_no_output() {
    let payload =
        install_finished_payload("claude", "npm i -g x", &install_run(true, Some(0), "ok"));
    assert_eq!(payload["ok"], json!(true));
    assert_eq!(payload["detail"], json!("ok"));
    assert_eq!(payload["exitCode"], json!(0));
    assert_eq!(payload["stdout"], json!(""));
    assert_eq!(payload["stderr"], json!(""));
}

#[test]
fn agent_install_2736_oversize_stream_is_tail_capped_with_the_marker() {
    let mut stdout = vec![b'F'];
    stdout.extend(std::iter::repeat_n(b'm', OUTPUT_TAIL_BYTES));
    stdout.push(b'L');
    let mut run = install_run(false, Some(1), "exit code 1");
    run.stdout = stdout.clone();
    let field = install_finished_payload("claude", "x", &run)["stdout"]
        .as_str()
        .expect("stdout field")
        .to_string();
    let rest = field
        .strip_prefix(TRUNCATION_MARKER)
        .expect("the field begins with the marker line");
    assert_eq!(rest.as_bytes(), &stdout[stdout.len() - OUTPUT_TAIL_BYTES..]);
    assert!(rest.ends_with('L'));
    assert!(!rest.contains('F'));

    run.stdout = vec![b'x'; OUTPUT_TAIL_BYTES];
    let exact = install_finished_payload("claude", "x", &run)["stdout"]
        .as_str()
        .expect("stdout field")
        .to_string();
    assert_eq!(exact.len(), OUTPUT_TAIL_BYTES);
    assert!(!exact.contains("truncated"), "no marker at exactly the cap");
}

#[test]
fn agent_install_2736_timeout_payload_has_a_null_exit_code() {
    let payload = install_finished_payload(
        "claude",
        "x",
        &install_run(false, None, "timed out after 300s (killed)"),
    );
    assert_eq!(payload["exitCode"], Value::Null);
    assert!(payload["detail"]
        .as_str()
        .expect("detail")
        .contains("timed out"));
}
