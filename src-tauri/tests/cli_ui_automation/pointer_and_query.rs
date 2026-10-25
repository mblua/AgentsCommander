use super::*;

#[test]
fn normal_binary_refuses_ui_hover_with_json_stdout_and_silent_stderr() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-hover-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-hover",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    // #944 - THIS is the assertion that catches a missing `Commands::UiHover(_)` arm in
    // main.rs's AC_MACHINE_OUTPUT allowlist, and it is that arm's only guard. Without the
    // arm the var stays unset and init_logger writes "[log] file logging to ..." plus
    // every log::* line to stderr. stdout is clean either way, because cli_println!
    // (cli/mod.rs:43-62) writes it unconditionally: it is a stderr contract, not a
    // stdout one, whatever this test's inherited name says.
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "refusing_non_testeable_binary");
}

/// #944 N5 - `--leave` is target-free (plan R5), and `conflicts_with = "selector"` is the
/// only thing enforcing it. Drop that attribute later and `--selector X --leave` would
/// SILENTLY IGNORE the selector: the bridge intercepts the leave form before it resolves
/// any node, so the caller would get a successful un-hover of whatever happened to be
/// hovered and never learn their selector was dead weight. Pin the intent, not just the
/// behavior.
#[test]
fn ui_hover_rejects_selector_together_with_leave() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-hover-conflict");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-hover",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--leave",
        ],
    );
    // clap rejects at parse time, before init_logger, so stderr carries the usage error
    // and nothing else. Exit is 1, not clap's default 2: main.rs maps it.
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stdout", &stdout);
    assert!(
        stderr.contains("cannot be used with"),
        "clap must reject --selector together with --leave, got: {stderr}"
    );
    assert!(
        stderr.contains("--selector") && stderr.contains("--leave"),
        "the conflict error must name both flags, got: {stderr}"
    );
}

/// #944 N5 - the other half of the same contract. A hover with no target and no `--leave`
/// is meaningless, so `required_unless_present = "leave"` must keep it un-runnable.
#[test]
fn ui_hover_requires_selector_unless_leave() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-hover-missing-selector");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(&bin, &["ui-hover", "--window", "main"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stdout", &stdout);
    assert!(
        stderr.contains("required arguments were not provided"),
        "clap must require --selector unless --leave, got: {stderr}"
    );
    assert!(
        stderr.contains("--selector"),
        "the error must name --selector, got: {stderr}"
    );
}

#[test]
fn fake_response_makes_ui_hover_succeed() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-hover-fake-response");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);

    let responder = spawn_fake_responder(&bin, move |request| {
        assert_eq!(request["window"], "main");
        assert_eq!(request["action"], "hover");
        assert_eq!(request["selector"], "replica.coord-a.menu.repo.0");
        // `value` is `skip_serializing_if = "Option::is_none"` (ui_automation.rs
        // :128-129), so a plain hover emits NO key at all. The bridge keys the
        // leave form on `value === "leave"`, so a stray key here would silently
        // invert the meaning of the verb.
        assert!(
            request.get("value").is_none(),
            "a plain hover must emit no `value` key, got: {request}"
        );
        let request_id = request["requestId"].as_str().unwrap();
        json!({
            "ok": true,
            "requestId": request_id,
            "window": "main",
            "action": "hover",
            "selector": "replica.coord-a.menu.repo.0",
            "target": {
                "testId": "replica.coord-a.menu.repo.0",
                "role": "menuitem",
                "state": "ready",
                "tag": "button",
                "visible": true,
                "disabled": false,
                "checked": null,
                "selected": null,
                "pressed": null,
                "expanded": null,
                "rect": null
            }
        })
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-hover",
            "--window",
            "main",
            "--selector",
            "replica.coord-a.menu.repo.0",
            "--timeout-ms",
            "3000",
        ],
    );
    responder.finish();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["target"]["testId"], "replica.coord-a.menu.repo.0");
}

#[test]
fn fake_response_makes_ui_hover_leave_succeed() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-hover-leave-fake-response");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);

    let responder = spawn_fake_responder(&bin, move |request| {
        assert_eq!(request["window"], "main");
        assert_eq!(request["action"], "hover");
        assert_eq!(request["value"], "leave");
        // Target-free (plan R5): --leave takes no --selector and conflicts with it
        // at the CLI, so the emitted selector is empty. The bridge intercepts
        // `value == "leave"` BEFORE it resolves any node, which is what makes the
        // leave form incapable of returning missing_selector / target_hidden /
        // target_obscured, and it echoes the selector back so `complete()`'s
        // window/action/selector equality check in `complete()` still matches on ""
        // (grep `fn complete`; the line number is deliberately omitted, it rotted
        // twice already: 317 -> 361 -> 370).
        assert_eq!(request["selector"], "");
        let request_id = request["requestId"].as_str().unwrap();
        json!({
            "ok": true,
            "requestId": request_id,
            "window": "main",
            "action": "hover",
            "selector": "",
            "target": {
                "testId": "",
                "role": null,
                "state": null,
                "tag": "",
                "visible": false,
                "disabled": false,
                "checked": null,
                "selected": null,
                "pressed": null,
                "expanded": null,
                "rect": null
            }
        })
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-hover",
            "--window",
            "main",
            "--leave",
            "--timeout-ms",
            "3000",
        ],
    );
    responder.finish();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(first_json(&stdout)["ok"], true);
}

#[test]
fn ui_query_timeout_reports_awaiting_gui_poller_phase() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-timeout-poller");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "100",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_gui_poller");
}

#[test]
fn ui_query_timeout_after_frontend_accepts_request_returns_bounded_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-query-inflight-timeout");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);

    let mover = spawn_inflight_mover(&bin, "query");

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "does.not.exist",
            "--timeout-ms",
            "250",
        ],
    );
    let seen_at = mover.finish();

    assert!(
        seen_at.elapsed() < Duration::from_secs(5),
        "ui-query should not hang after the frontend accepted the request"
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_frontend_response");
}

#[test]
fn ui_query_large_missing_selector_response_is_bounded_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-large-missing-selector");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);

    let responder = spawn_fake_responder(&bin, move |request| {
        let request_id = request["requestId"].as_str().unwrap();
        let available: Vec<Value> = (0..256)
            .map(|i| {
                json!({
                    "testId": format!("target.{i}"),
                    "role": "button",
                    "state": "ready",
                    "tag": "button",
                    "text": "x".repeat(2000),
                    "visible": true,
                    "disabled": false,
                    "checked": null,
                    "selected": null,
                    "pressed": null,
                    "expanded": null,
                    "rect": {
                        "x": i,
                        "y": i,
                        "width": 100,
                        "height": 30
                    }
                })
            })
            .collect();
        json!({
            "ok": false,
            "requestId": request_id,
            "window": "main",
            "action": "query",
            "selector": "does.not.exist",
            "error": "missing_selector",
            "message": "No automation target matched \
                data-ac-testid=\"does.not.exist\" in window \"main\".",
            "available": available,
            "diagnostics": {
                "devicePixelRatio": 1,
                "viewport": { "width": 1280, "height": 720 }
            }
        })
    });

    let (code, stdout, stderr, timed_out) = run_without_draining_output_until_exit(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "does.not.exist",
            "--timeout-ms",
            "3000",
        ],
        WATCHER_HARD_CAP,
    );
    let seen_at = responder.finish();

    assert!(!timed_out, "ui-query should not hang");
    assert!(seen_at.elapsed() < Duration::from_secs(5));
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert!(
        stdout.len() < 4096,
        "stdout should stay below common pipe buffers, got {} bytes",
        stdout.len()
    );
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "missing_selector");
    assert_eq!(parsed["available"].as_array().unwrap().len(), 8);
    assert_eq!(parsed["diagnostics"]["availableTotal"], 256);
    assert_eq!(parsed["diagnostics"]["availableLimit"], 8);
    assert_eq!(parsed["diagnostics"]["availableTruncated"], true);
    let text = parsed["available"][0]["text"].as_str().unwrap();
    assert!(text.ends_with("..."));
    assert!(text.len() <= 83);
}

#[test]
fn ui_click_timeout_removes_inflight_request_with_json_only_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-timeout-inflight");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &[]);
    let automation_dir = config_dir_for(&bin).join("ui-automation");
    let requests_dir = automation_dir.join("requests");

    let mover = spawn_inflight_mover(&bin, "click");

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-click",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "250",
        ],
    );
    mover.finish();
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_eq!(parsed["phase"], "awaiting_frontend_ready");

    let remaining: Vec<PathBuf> = std::fs::read_dir(&requests_dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    assert!(
        remaining.is_empty(),
        "timed-out request files should be removed: {remaining:?}"
    );
}

#[test]
fn ui_query_retries_transient_missing_session_file() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-session-read-race");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let writer_bin = bin.clone();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        write_session(&writer_bin, pid, &["main"]);
    });

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.confirm",
            "--timeout-ms",
            "100",
        ],
    );
    writer.join().unwrap();

    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "timeout");
    assert_ne!(parsed["error"], "automation_session_missing");
}

#[test]
fn stale_prior_session_with_running_daemon_reports_automation_not_enabled_on_stdout() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new("ui-disabled-stale-session");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, u32::MAX, &["main"]);
    write_daemon_pid(&bin, pid);

    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-query",
            "--window",
            "main",
            "--selector",
            "onboarding.modal",
        ],
    );

    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    let parsed = first_json(&stdout);
    assert_eq!(parsed["error"], "automation_not_enabled");
}

/// #1773: the guard must kill and reap the child on the panic path. The child is the
/// real binary running `ui-query` with no responder: it writes its request file
/// (`ui_automation.rs:1399`) and then polls for a response for the whole
/// `--timeout-ms` (`:1412-1464`); the only exits from that loop are a response file,
/// which nothing writes here, and the 30 s deadline. Readiness (request file present,
/// child alive) is asserted outside `catch_unwind`; the closure only moves the guard
/// and panics; the caught payload must be the sentinel. Cross-platform since
/// #2382 gave `pid_is_alive` a real Unix implementation.
#[test]
fn reap_on_drop_kills_the_child_when_the_caller_panics() {
    use agentscommander_lib::testability::ui_automation::pid_is_alive;
    const SENTINEL: &str = "1773 ReapOnDrop sentinel panic 2b9d";

    let _guard = test_lock();
    let pid = fake_live_pid().expect("fake_live_pid must yield a live session pid");
    let tmp = Tmp::new("ui-reap-on-drop");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let requests_dir = config_dir_for(&bin).join("ui-automation").join("requests");

    let guard = ReapOnDrop(
        command_for_binary(&bin)
            .args([
                "ui-query",
                "--window",
                "main",
                "--selector",
                "does.not.exist",
                "--timeout-ms",
                "30000",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn binary"),
    );
    let child_pid = guard.0.id();
    // Anchored after the spawn: this cap only has to catch a child that never
    // starts. The child's own budget is --timeout-ms 30000.
    let readiness_cap = Instant::now() + WATCHER_HARD_CAP;

    // Readiness: the child has written its request file and is therefore in, or
    // entering, its response-poll loop. Asserted here, outside catch_unwind.
    let (request_file, seen_at) = loop {
        let pending = std::fs::read_dir(&requests_dir)
            .expect("requests dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .find(|path| {
                path.extension().and_then(|extension| extension.to_str()) == Some("json")
                    && !path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(".inflight.json"))
            });
        if let Some(path) = pending {
            break (path, Instant::now());
        }
        assert!(
            Instant::now() < readiness_cap,
            "child {child_pid} never wrote its request file into {}",
            requests_dir.display()
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        pid_is_alive(child_pid),
        "child {child_pid} must be alive inside its request wait before the panic"
    );

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = guard;
        std::panic::panic_any(SENTINEL);
    }));

    let payload = outcome.expect_err("the closure must panic");
    assert_eq!(
        payload.downcast_ref::<&'static str>(),
        Some(&SENTINEL),
        "the caught panic must be the sentinel, not a failed assertion"
    );
    let unwound_after = seen_at.elapsed();
    assert!(
        unwound_after < Duration::from_secs(10),
        "the guard must kill the child, not wait for its 30 s timeout: {unwound_after:?}"
    );
    assert!(
        !pid_is_alive(child_pid),
        "the guard must kill and reap the child during unwinding (pid {child_pid})"
    );
    assert!(
        request_file.exists(),
        "a child that leaves the loop on its own removes its request file \
        (ui_automation.rs:1458); the file survives only because the child was terminated: {}",
        request_file.display()
    );
}

#[test]
fn normal_binary_refuses_ui_pointer_with_json_stdout_and_silent_stderr() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-pointer-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-pointer",
            "--selector",
            "settings.agentRow.0.dragHandle",
            "down",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(
        first_json(&stdout)["error"],
        "refusing_non_testeable_binary"
    );
}

#[test]
fn normal_binary_refuses_ui_key_with_json_stdout_and_silent_stderr() {
    let _guard = test_lock();
    let tmp = Tmp::new("ui-key-non-testable");
    let bin = copy_binary_as(tmp.path(), "agentscommander.exe");
    let (code, stdout, stderr) = run(
        &bin,
        &[
            "ui-key",
            "--selector",
            "settings.agentRow.0.dragHandle",
            "Escape",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(
        first_json(&stdout)["error"],
        "refusing_non_testeable_binary"
    );
}

fn ok_response(request: &Value) -> Value {
    json!({
        "ok": true,
        "requestId": request["requestId"],
        "window": request["window"],
        "action": request["action"],
        "selector": request["selector"],
        "target": null
    })
}

fn run_fake_success(label: &str, args: &[&str], check: fn(&Value)) {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    let tmp = Tmp::new(label);
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    write_session(&bin, pid, &["main"]);
    let responder = spawn_fake_responder(&bin, move |request| {
        assert_eq!(request["window"], "main");
        check(request);
        ok_response(request)
    });
    let (code, stdout, stderr) = run(&bin, args);
    responder.finish();
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_empty_output("stderr", &stderr);
    assert_eq!(first_json(&stdout)["ok"], true);
}

#[test]
fn fake_response_makes_ui_pointer_down_succeed() {
    run_fake_success(
        "ui-pointer-down-fake-response",
        &[
            "ui-pointer",
            "--selector",
            "settings.agentRow.0.dragHandle",
            "--timeout-ms",
            "3000",
            "down",
        ],
        |request| {
            assert_eq!(request["action"], "pointer");
            assert_eq!(request["selector"], "settings.agentRow.0.dragHandle");
            assert_eq!(request["value"], "down");
        },
    );
}

#[test]
fn fake_response_makes_ui_pointer_up_succeed() {
    run_fake_success(
        "ui-pointer-up-fake-response",
        &["ui-pointer", "--timeout-ms", "3000", "up"],
        |request| {
            assert_eq!(request["action"], "pointer");
            assert_eq!(request["selector"], "");
            assert_eq!(request["value"], "up");
        },
    );
}

#[test]
fn fake_response_makes_ui_key_succeed() {
    run_fake_success(
        "ui-key-fake-response",
        &[
            "ui-key",
            "--selector",
            "settings.agentRow.0.dragHandle",
            "--timeout-ms",
            "3000",
            "Alt+ArrowUp",
        ],
        |request| {
            assert_eq!(request["action"], "key");
            assert_eq!(request["selector"], "settings.agentRow.0.dragHandle");
            assert_eq!(request["value"], "Alt+ArrowUp");
        },
    );
}

#[test]
fn ui_pointer_and_ui_key_preflight_errors_create_no_request_file() {
    let _guard = test_lock();
    let cases: [(&str, &[&str], &str); 4] = [
        (
            "pointer-op",
            &["ui-pointer", "--selector", "x", "drag"],
            "invalid_pointer_operation",
        ),
        (
            "pointer-required",
            &["ui-pointer", "move"],
            "pointer_selector_required",
        ),
        (
            "pointer-not-allowed",
            &["ui-pointer", "--selector", "x", "up"],
            "pointer_selector_not_allowed",
        ),
        (
            "key-chord",
            &["ui-key", "--selector", "x", "alt+ArrowUp"],
            "invalid_key_chord",
        ),
    ];
    for (label, args, expected) in cases {
        let tmp = Tmp::new(&format!("ui-preflight-{label}"));
        let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
        let (code, stdout, stderr) = run(&bin, args);
        assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
        assert_empty_output("stderr", &stderr);
        assert_eq!(first_json(&stdout)["error"], expected);

        let requests_dir = config_dir_for(&bin).join("ui-automation").join("requests");
        let request_count = std::fs::read_dir(&requests_dir)
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0);
        assert_eq!(request_count, 0, "{label} created a request file");
    }
}

#[test]
fn fake_response_passes_pointer_frontend_error_through() {
    let _guard = test_lock();
    let Some(pid) = fake_live_pid() else {
        eprintln!("skip: no fake live pid available");
        return;
    };
    // Interim codes until P2 teaches the bridge the pointer action.
    let cases: [(&str, &[&str], &str); 2] = [
        (
            "unsupported_action",
            &[
                "ui-pointer",
                "--selector",
                "row",
                "--timeout-ms",
                "3000",
                "down",
            ],
            "row",
        ),
        (
            "missing_selector",
            &["ui-pointer", "--timeout-ms", "3000", "up"],
            "",
        ),
    ];
    for (error, args, selector) in cases {
        let tmp = Tmp::new(&format!("ui-pointer-{error}"));
        let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
        write_session(&bin, pid, &["main"]);
        let responder = spawn_fake_responder(&bin, move |request| {
            assert_eq!(request["action"], "pointer");
            json!({
                "ok": false,
                "requestId": request["requestId"],
                "window": "main",
                "action": "pointer",
                "selector": request["selector"],
                "error": error,
                "message": "pointer request failed"
            })
        });
        let (code, stdout, stderr) = run(&bin, args);
        responder.finish();
        assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
        assert_empty_output("stderr", &stderr);
        let parsed = first_json(&stdout);
        assert_eq!(parsed["error"], error);
        assert_eq!(parsed["selector"], selector);
    }
}
