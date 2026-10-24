use super::{
    assert_no_stdout_on_error, assert_success_json_array, assert_success_json_object,
    config_dir_for_bin, copy_binary_as, copy_binary_into, create_send_fixture, run, Tmp,
    LEAK_PROBE_TOKEN, VALID_TOKEN,
};
use std::path::Path;

#[test]
fn root_help_lists_public_subcommands() {
    let bin = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let (code, stdout, stderr) = run(bin, &["--help"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stderr.trim().is_empty(),
        "help should not initialize logging: {stderr}"
    );
    for command in [
        "send",
        "list-peers",
        "list-peers-lean",
        "terminal-snapshot",
        "list-sessions",
        "agency-templates",
        "create-agent",
        "create-agent-matrix",
        "close-session",
        "raise-hand",
        "task-set-title",
        "task-append-body",
        "open-project",
        "new-project",
        "telegram-send-image",
        "telegram-send-message",
        // #1614: `room` is the canonical subcommand; `workgroup` remains an
        // accepted deprecated alias, and D9 hides an alias from help on purpose,
        // so root help must list `room` and must NOT list `workgroup`.
        "room",
        "team",
        "harness",
    ] {
        assert!(
            stdout.contains(command),
            "root help missing {command}:\n{stdout}"
        );
    }
    // #654 (`d20f5ea`) hid these internal verbs from `--help` via `hide = true`.
    // They must NOT appear in root help. (#657 test-debt fix: the expected list
    // above still required them after #654 landed, leaving this test red on main.)
    for hidden in ["role-experiment", "test-reset", "window-info"] {
        assert!(
            !stdout.contains(hidden),
            "root help must not list hidden verb {hidden}:\n{stdout}"
        );
    }
}

#[test]
fn unknown_root_subcommand_exits_one_with_usage() {
    let bin = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let (code, stdout, stderr) = run(bin, &["definitely-not-a-command"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(stderr.contains("unrecognized subcommand") || stderr.contains("Usage:"));
}

#[test]
fn public_subcommand_help_contracts() {
    let bin = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["send", "--help"],
            &["DELIVERY MODES", "--token", "--to", "--send", "--mode"],
        ),
        (
            &["list-peers", "--help"],
            &["OUTPUT", "--token", "--root", "--peer"],
        ),
        (
            &["list-peers-lean", "--help"],
            &["lean", "--token", "--root"],
        ),
        (
            &["terminal-snapshot", "--help"],
            &[
                "--token",
                "--root",
                "--to",
                "--format",
                "--output",
                "--timeout",
                "--snapshot-targets",
            ],
        ),
        (
            &["list-sessions", "--help"],
            &["OUTPUT", "--status", "raisedHand"],
        ),
        (
            &["agency-templates", "--help"],
            &["update", "list", "status"],
        ),
        (
            &["agency-templates", "update", "--help"],
            &["--repo", "--ref"],
        ),
        (
            &["agency-templates", "list", "--help"],
            &["--json", "--pretty"],
        ),
        (
            &["agency-templates", "status", "--help"],
            &["--json", "--pretty"],
        ),
        (
            &["create-agent", "--help"],
            &["--project", "--name", "--description"],
        ),
        (
            &["create-agent-matrix", "--help"],
            &["--project", "--name", "--description"],
        ),
        (
            &["role-experiment", "--help"],
            &[
                "init", "list", "show", "variant", "validate", "run", "report",
            ],
        ),
        (
            &["role-experiment", "init", "--help"],
            &["--project", "--source-agent"],
        ),
        (&["role-experiment", "list", "--help"], &["--project"]),
        (&["role-experiment", "show", "--help"], &["--experiment"]),
        (&["role-experiment", "variant", "--help"], &["set", "diff"]),
        (
            &["role-experiment", "variant", "set", "--help"],
            &["--experiment", "--role-file"],
        ),
        (
            &["role-experiment", "variant", "diff", "--help"],
            &["--experiment", "--against"],
        ),
        (
            &["role-experiment", "validate", "--help"],
            &["--experiment"],
        ),
        (&["role-experiment", "run", "--help"], &["--experiment"]),
        (&["role-experiment", "report", "--help"], &["--experiment"]),
        (
            &["close-session", "--help"],
            &["--token", "--root", "--target"],
        ),
        (&["raise-hand", "--help"], &["--token", "--root", "OUTPUT"]),
        (
            &["task-set-title", "--help"],
            &["--token", "--root", "--title"],
        ),
        (
            &["task-append-body", "--help"],
            &["--token", "--root", "--text"],
        ),
        (&["open-project", "--help"], &["PATH"]),
        (&["new-project", "--help"], &["PATH"]),
        (&["telegram-send-image", "--help"], &["--path", "--caption"]),
        (
            &["telegram-send-message", "--help"],
            &["--text", "--bot-id", "--bot-label"],
        ),
        (&["room", "--help"], &["list", "activity", "add", "remove"]),
        (&["room", "list", "--help"], &["--project"]),
        (&["room", "activity", "--help"], &["--project"]),
        (
            &["room", "add", "--help"],
            &["--project", "--team", "--title"],
        ),
        (&["room", "remove", "--help"], &["--project", "--room"]),
        // The deprecated spelling still reaches the same help, and help renders
        // the CANONICAL name for it (section 5.8 fact 4).
        (
            &["workgroup", "--help"],
            &["list", "activity", "add", "remove"],
        ),
        (&["workgroup", "activity", "--help"], &["--project"]),
        (&["workgroup", "remove", "--help"], &["--project", "--room"]),
        (
            &["team", "--help"],
            &["create", "list", "add-member", "remove-member"],
        ),
        (
            &["team", "create", "--help"],
            &["--project", "--team", "--coordinator"],
        ),
        (&["team", "list", "--help"], &["--project"]),
        (
            &["team", "add-member", "--help"],
            &["--project", "--room", "--agent"],
        ),
        (
            &["team", "remove-member", "--help"],
            &["--project", "--room", "--agent"],
        ),
        (&["harness", "--help"], &["--dry-run", "--raw-command"]),
        (&["test-reset", "--help"], &["--confirm-testeable"]),
        (&["window-info", "--help"], &["Usage:"]),
    ];

    for (args, markers) in cases {
        let (code, stdout, stderr) = run(bin, args);
        assert_eq!(
            code,
            Some(0),
            "args {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert!(
            stderr.trim().is_empty(),
            "help should not log for {args:?}: {stderr}"
        );
        assert!(
            stdout.contains("Usage:") || stdout.contains("Commands:"),
            "help missing usage/commands for {args:?}:\n{stdout}"
        );
        assert!(!stdout.to_ascii_lowercase().contains("panic"));
        for marker in *markers {
            assert!(
                stdout.contains(marker),
                "help {args:?} missing {marker}:\n{stdout}"
            );
        }
    }
}

#[test]
fn token_gated_commands_missing_token_contracts() {
    let tmp = Tmp::new("cli-missing-token");
    let bin = copy_binary_into(tmp.path());
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let root_s = root.to_string_lossy().to_string();
    let cases: &[&[&str]] = &[
        &[
            "send",
            "--to",
            "Project:wg-1/dev",
            "--root",
            &root_s,
            "--command",
            "clear",
        ],
        &["list-peers", "--root", &root_s],
        &["list-peers-lean", "--root", &root_s],
        &["raise-hand", "--root", &root_s],
        &[
            "close-session",
            "--root",
            &root_s,
            "--target",
            "Project:wg-1/dev",
        ],
        &["task-set-title", "--root", &root_s, "--title", "Title"],
        &["task-append-body", "--root", &root_s, "--text", "Body"],
    ];

    for args in cases {
        let (code, stdout, stderr) = run(&bin, args);
        assert_eq!(
            code,
            Some(1),
            "args {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_no_stdout_on_error(&stdout);
        assert!(
            stderr.contains("--token") || stderr.contains("AGENTSCOMMANDER_TOKEN"),
            "missing token stderr for {args:?}: {stderr}"
        );
    }
}

#[test]
fn token_gated_commands_invalid_token_redaction_contracts() {
    let tmp = Tmp::new("cli-invalid-token");
    let bin = copy_binary_into(tmp.path());
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let root_s = root.to_string_lossy().to_string();
    let cases: &[&[&str]] = &[
        &[
            "send",
            "--token",
            LEAK_PROBE_TOKEN,
            "--to",
            "Project:wg-1/dev",
            "--root",
            &root_s,
            "--command",
            "clear",
        ],
        &["list-peers", "--token", LEAK_PROBE_TOKEN, "--root", &root_s],
        &[
            "list-peers-lean",
            "--token",
            LEAK_PROBE_TOKEN,
            "--root",
            &root_s,
        ],
        &[
            "close-session",
            "--token",
            LEAK_PROBE_TOKEN,
            "--root",
            &root_s,
            "--target",
            "Project:wg-1/dev",
        ],
        &["raise-hand", "--token", LEAK_PROBE_TOKEN, "--root", &root_s],
        &[
            "task-set-title",
            "--token",
            LEAK_PROBE_TOKEN,
            "--root",
            &root_s,
            "--title",
            "Title",
        ],
        &[
            "task-append-body",
            "--token",
            LEAK_PROBE_TOKEN,
            "--root",
            &root_s,
            "--text",
            "Body",
        ],
    ];

    for args in cases {
        let (code, stdout, stderr) = run(&bin, args);
        assert_eq!(
            code,
            Some(1),
            "args {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_no_stdout_on_error(&stdout);
        assert!(
            stderr.contains("invalid token supplied"),
            "stderr: {stderr}"
        );
        assert!(
            !stderr.contains(LEAK_PROBE_TOKEN),
            "token leaked in stderr: {stderr}"
        );
        assert!(
            !stderr.contains("leakprobe-487"),
            "probe leaked in stderr: {stderr}"
        );
    }
}

#[test]
fn token_gated_commands_missing_root_contracts() {
    let tmp = Tmp::new("cli-missing-root");
    let bin = copy_binary_into(tmp.path());
    let cases: &[&[&str]] = &[
        &["list-peers", "--token", VALID_TOKEN],
        &["list-peers-lean", "--token", VALID_TOKEN],
        &["raise-hand", "--token", VALID_TOKEN],
        &[
            "close-session",
            "--token",
            VALID_TOKEN,
            "--target",
            "Project:wg-1/dev",
        ],
        &["task-set-title", "--token", VALID_TOKEN, "--title", "Title"],
        &["task-append-body", "--token", VALID_TOKEN, "--text", "Body"],
    ];
    for args in cases {
        let (code, stdout, stderr) = run(&bin, args);
        assert_eq!(
            code,
            Some(1),
            "args {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_no_stdout_on_error(&stdout);
        assert!(stderr.contains("--root"), "stderr: {stderr}");
    }
}

#[test]
fn read_only_json_commands_missing_config_contracts() {
    let tmp = Tmp::new("cli-json-missing-config");
    let bin = copy_binary_into(tmp.path());
    let root = tmp.path().join("empty-root");
    std::fs::create_dir_all(&root).expect("root");
    let root_s = root.to_string_lossy().to_string();

    let (code, stdout, stderr) = run(&bin, &["list-sessions"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(assert_success_json_array(&stdout), serde_json::json!([]));

    let (code, stdout, stderr) = run(&bin, &["agency-templates", "list", "--json"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(assert_success_json_array(&stdout), serde_json::json!([]));

    let (code, stdout, stderr) = run(&bin, &["agency-templates", "status", "--json"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let status = assert_success_json_object(&stdout);
    assert_eq!(status["available"], false);
    assert!(
        status.get("reason").is_some(),
        "status should include reason: {status}"
    );

    for args in [
        vec!["list-peers", "--token", VALID_TOKEN, "--root", &root_s],
        vec!["list-peers-lean", "--token", VALID_TOKEN, "--root", &root_s],
    ] {
        let (code, stdout, stderr) = run(&bin, &args);
        assert_eq!(
            code,
            Some(0),
            "args {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_success_json_array(&stdout);
    }
}

/// #698 raisedHand contract with negative cases; #747 flipped the exited row:
/// a coordinator row with `{"exited":0}` + visible raise-hand is EXACTLY the
/// dormant-restored shape (every real-exit path clears communication in
/// `mark_exited`, so it can only be produced by the #747 restore) and now
/// reports `raisedHand: true`.
#[test]
fn list_sessions_outputs_raised_hand_boolean_with_negative_cases() {
    let tmp = Tmp::new("cli-list-sessions-raised-hand");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    std::fs::create_dir_all(&config_dir).expect("config dir");

    let visible_raise_hand = serde_json::json!({
        "kind": "raiseHand",
        "visible": true,
        "updatedAt": "2026-06-30T11:00:00+00:00"
    });
    let hidden_raise_hand = serde_json::json!({
        "kind": "raiseHand",
        "visible": false,
        "updatedAt": "2026-06-30T11:00:00+00:00"
    });
    let sessions = serde_json::json!([
        {
            "name": "tech-lead",
            "shell": "codex",
            "shellArgs": [],
            "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_tech-lead",
            "isCoordinator": true,
            "id": "11111111-1111-1111-1111-111111111111",
            "status": "running",
            "waitingForInput": true,
            "communication": visible_raise_hand,
            "createdAt": "2026-06-30T10:00:00+00:00"
        },
        {
            "name": "coord-missing-communication",
            "shell": "codex",
            "shellArgs": [],
            "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_architect",
            "isCoordinator": true,
            "id": "22222222-2222-2222-2222-222222222222",
            "status": "running",
            "waitingForInput": false,
            "createdAt": "2026-06-30T10:05:00+00:00"
        },
        {
            "name": "coord-hidden-communication",
            "shell": "codex",
            "shellArgs": [],
            "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_planner",
            "isCoordinator": true,
            "id": "33333333-3333-3333-3333-333333333333",
            "status": "running",
            "waitingForInput": false,
            "communication": hidden_raise_hand,
            "createdAt": "2026-06-30T10:10:00+00:00"
        },
        {
            "name": "dev-rust",
            "shell": "codex",
            "shellArgs": [],
            "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_dev-rust",
            "isCoordinator": false,
            "id": "44444444-4444-4444-4444-444444444444",
            "status": "running",
            "waitingForInput": false,
            "communication": visible_raise_hand,
            "createdAt": "2026-06-30T10:15:00+00:00"
        },
        {
            "name": "coord-dormant-restored-hand",
            "shell": "codex",
            "shellArgs": [],
            "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_old-tech-lead",
            "isCoordinator": true,
            "id": "55555555-5555-5555-5555-555555555555",
            "status": { "exited": 0 },
            "waitingForInput": false,
            "communication": visible_raise_hand,
            "createdAt": "2026-06-30T10:20:00+00:00"
        }
    ]);
    std::fs::write(
        config_dir.join("sessions.json"),
        serde_json::to_string_pretty(&sessions).expect("sessions json"),
    )
    .expect("write sessions");

    let (code, stdout, stderr) = run(&bin, &["list-sessions"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let rows = assert_success_json_array(&stdout);
    assert_eq!(rows.as_array().unwrap().len(), 5);
    assert_eq!(rows[0]["raisedHand"], true);
    assert_eq!(rows[1]["raisedHand"], false);
    assert_eq!(rows[2]["raisedHand"], false);
    assert_eq!(rows[3]["raisedHand"], false);
    assert_eq!(rows[4]["raisedHand"], true);
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["raisedHand"].is_boolean()));

    let (code, stdout, stderr) = run(&bin, &["list-sessions", "--status", "running"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    let running = assert_success_json_array(&stdout);
    assert_eq!(running.as_array().unwrap().len(), 4);
    assert!(running
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["status"] == "running" && row.get("raisedHand").is_some()));
}

#[test]
fn simple_bad_path_contracts() {
    let tmp = Tmp::new("cli-bad-paths");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let sentinel = config_dir.join("sentinel.txt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(&sentinel, "keep").expect("sentinel");

    let missing_project = tmp.path().join("MissingProject");
    let missing_project_s = missing_project.to_string_lossy().to_string();
    let (code, stdout, stderr) = run(&bin, &["open-project", &missing_project_s]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(stderr.contains("Error:"));
    assert!(sentinel.is_file(), "config sentinel should remain");
    assert!(
        !config_dir.join("project-refresh-requests").exists(),
        "bad open-project must not write refresh requests"
    );

    let missing_image = tmp.path().join("missing.png");
    let missing_image_s = missing_image.to_string_lossy().to_string();
    let (code, stdout, stderr) = run(&bin, &["telegram-send-image", "--path", &missing_image_s]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(
        stderr.contains("No Telegram bots are configured"),
        "stderr: {stderr}"
    );

    let send_tmp = Tmp::new("cli-send-bad-path");
    let send_bin = copy_binary_into(send_tmp.path());
    let send_config = config_dir_for_bin(&send_bin);
    let send_local_dir = format!(
        ".{}",
        send_bin
            .file_stem()
            .expect("send bin stem")
            .to_string_lossy()
    );
    let (project, sender) = create_send_fixture(send_tmp.path(), &send_bin, &send_config);
    let sender_s = sender.to_string_lossy().to_string();
    let peer = "ProjectAlpha:room-1-dev-team/dev-rust";
    let (code, stdout, stderr) = run(
        &send_bin,
        &[
            "send",
            "--token",
            VALID_TOKEN,
            "--root",
            &sender_s,
            "--to",
            peer,
            "--send",
            "path/with/separator.md",
        ],
    );
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(
        stderr.contains("filename") || stderr.contains("separator") || stderr.contains("traversal"),
        "stderr: {stderr}"
    );
    assert!(
        !sender.join(&send_local_dir).join("outbox").exists(),
        "send filename refusal must not create sender outbox"
    );
    assert!(
        !send_config.join("outbox").exists() && !send_config.join("app-outbox-path.txt").exists(),
        "send filename refusal must not create app outbox artifacts"
    );
    assert!(
        !project
            .join(".ac")
            .join("room-1-dev-team")
            .join("__agent_dev-rust")
            .join(&send_local_dir)
            .join("outbox")
            .exists(),
        "send filename refusal must not create peer outbox"
    );
}

#[test]
fn missing_required_args_contracts() {
    let tmp = Tmp::new("cli-missing-required");
    let bin = copy_binary_into(tmp.path());
    let cases: &[&[&str]] = &[
        &["create-agent"],
        &["create-agent-matrix"],
        &["new-project"],
        &["open-project"],
        &["role-experiment"],
        &["workgroup", "list"],
        &["workgroup", "add", "--project", "ProjectAlpha"],
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
        ],
        &[
            "team",
            "add-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "wg-1-dev-team",
        ],
        &["harness"],
        &["telegram-send-image"],
        &["telegram-send-message"],
    ];

    for args in cases {
        let (code, stdout, stderr) = run(&bin, args);
        assert_eq!(
            code,
            Some(1),
            "args {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_no_stdout_on_error(&stdout);
        assert!(
            stderr.contains("Usage:") || stderr.contains("required") || stderr.contains("command"),
            "stderr for {args:?}: {stderr}"
        );
    }
}

#[test]
fn list_sessions_invalid_status_contract() {
    let tmp = Tmp::new("cli-list-sessions-invalid");
    let bin = copy_binary_into(tmp.path());
    let (code, stdout, stderr) = run(&bin, &["list-sessions", "--status", "bogus"]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(stderr.contains("valid statuses") || stderr.contains("running"));
}

#[test]
fn telegram_send_image_empty_and_missing_config_contracts() {
    let tmp = Tmp::new("cli-telegram-contracts");
    let bin = copy_binary_into(tmp.path());
    let (code, stdout, stderr) = run(&bin, &["telegram-send-image", "--path", "   "]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(stderr.contains("--path must not be empty"));

    let image = tmp.path().join("image.png");
    std::fs::write(&image, b"not really png").expect("write image");
    let image_s = image.to_string_lossy().to_string();
    let (code, stdout, stderr) = run(&bin, &["telegram-send-image", "--path", &image_s]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_no_stdout_on_error(&stdout);
    assert!(stderr.contains("No Telegram bots are configured"));
}

#[test]
fn window_info_no_gui_contract() {
    let tmp = Tmp::new("cli-window-info");
    let bin = copy_binary_as(tmp.path(), "agentscommander_testeable.exe");
    let (code, stdout, stderr) = run(&bin, &["window-info"]);
    #[cfg(target_os = "windows")]
    {
        assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
        let json = assert_success_json_object(&stdout);
        assert_eq!(json["ok"], true);
        assert_eq!(json["supported"], true);
        assert!(
            json["windows"].is_array(),
            "windows should be array: {json}"
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
        let json = assert_success_json_object(&stdout);
        assert_eq!(json["supported"], false);
        assert_eq!(json["error"], "window_info_unsupported_on_platform");
    }
}
