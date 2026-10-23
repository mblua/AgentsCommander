use super::*;
use crate::pty::container_runtime::{
    ContainerRuntime, ContainerStartRequest, DEFAULT_API_HELPER_PATH, DEFAULT_CONTAINER_WORKDIR,
    SESSION_LABEL,
};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

fn request() -> ContainerStartRequest {
    ContainerStartRequest {
        session_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        image: "ac-bridge:test".to_string(),
        host_root: "C:/project/.ac/wg-1-team/__agent_dev".to_string(),
        container_workdir: DEFAULT_CONTAINER_WORKDIR.to_string(),
        api_url: "http://host.docker.internal:8765".to_string(),
        api_token: "api-secret".to_string(),
        registration_ticket: "ticket-secret".to_string(),
        local_dir: ".agentscommander_ac".to_string(),
        command: "codex".to_string(),
        args: vec!["--version".to_string()],
        child_env: vec![("CODEX_HOME".to_string(), "/workspace/.codex".to_string())],
        env_unset: vec!["CLAUDE_CONFIG_DIR".to_string()],
        cols: 120,
        rows: 30,
        repo_mounts: Vec::new(),
    }
}

#[test]
fn run_command_is_detached_labeled_and_has_expected_env_without_interactive_flags() {
    let runtime = DockerRuntime::with_program("docker-test");
    let spec = runtime.build_run_command(&request()).unwrap();
    let joined = spec.args.join(" ");

    assert_eq!(spec.program, "docker-test");
    assert!(spec.args.iter().any(|arg| arg == "-d"));
    assert!(joined.contains("--name agentscommander-11111111111141118111111111111111"));
    assert!(!spec.args.iter().any(|arg| arg == "--rm"));
    assert!(!spec
        .args
        .iter()
        .any(|arg| arg == "-it" || arg == "-i" || arg == "-t"));
    assert!(joined.contains(&format!(
        "{}=11111111-1111-4111-8111-111111111111",
        SESSION_LABEL
    )));
    assert!(joined.contains("AGENTSCOMMANDER_API_URL=http://host.docker.internal:8765"));
    assert!(joined.contains("--env AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN"));
    assert!(joined.contains("--env AGENTSCOMMANDER_API_TOKEN"));
    assert!(joined.contains("--env AGENTSCOMMANDER_ROOT"));
    assert!(!joined.contains("AGENTSCOMMANDER_ROOT=C:/project"));
    assert!(!joined.contains("ticket-secret"));
    assert!(!joined.contains("api-secret"));
    assert_eq!(
        spec.secret_env
            .get("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN")
            .map(String::as_str),
        Some("ticket-secret")
    );
    assert_eq!(
        spec.secret_env
            .get("AGENTSCOMMANDER_API_TOKEN")
            .map(String::as_str),
        Some("api-secret")
    );
    assert_eq!(
        spec.secret_env
            .get("AGENTSCOMMANDER_ROOT")
            .map(String::as_str),
        Some("C:/project/.ac/wg-1-team/__agent_dev")
    );
    assert!(joined.contains(&format!(
        "AGENTSCOMMANDER_BINARY_PATH={DEFAULT_API_HELPER_PATH}"
    )));
    assert!(joined.contains("--env AGENTSCOMMANDER_BRIDGE_COMMAND"));
    assert!(joined.contains("--env AGENTSCOMMANDER_BRIDGE_ARGS_JSON"));
    assert!(joined.contains("--env AGENTSCOMMANDER_BRIDGE_ENV_JSON"));
    assert!(!joined.contains("AGENTSCOMMANDER_BRIDGE_COMMAND=codex"));
    assert!(!joined.contains("CODEX_HOME"));
    assert_eq!(
        spec.secret_env
            .get("AGENTSCOMMANDER_BRIDGE_COMMAND")
            .map(String::as_str),
        Some("codex")
    );
    assert_eq!(
        spec.secret_env
            .get("AGENTSCOMMANDER_BRIDGE_ARGS_JSON")
            .map(String::as_str),
        Some("[\"--version\"]")
    );
    assert_eq!(
        spec.secret_env
            .get("AGENTSCOMMANDER_BRIDGE_ENV_JSON")
            .map(String::as_str),
        Some("[[\"CODEX_HOME\",\"/workspace/.codex\"]]")
    );
    assert!(joined.contains("AGENTSCOMMANDER_BRIDGE_ENV_UNSET_JSON=[\"CLAUDE_CONFIG_DIR\"]"));
    assert!(
        joined.contains("type=bind,source=C:/project/.ac/wg-1-team/__agent_dev,target=/workspace")
    );
    let image_idx = spec
        .args
        .iter()
        .position(|arg| arg == "ac-bridge:test")
        .expect("image arg");
    assert_eq!(spec.args[image_idx - 1], "--");
    assert_eq!(spec.args[image_idx + 1], DEFAULT_BRIDGE_ENTRYPOINT);
    assert!(!joined.contains("docker.sock"));
    assert!(!joined.contains("messaging"));
    assert!(!joined.contains("api-clients.json"));
}

#[test]
fn docker_command_debug_and_captured_output_redact_commands_paths_and_secret_values() {
    let runtime = DockerRuntime::with_program("docker-secret-program");
    let spec = runtime.build_run_command(&request()).unwrap();
    let rendered = format!("{spec:?}");
    for sentinel in [
        "docker-secret-program",
        "C:/project",
        "codex",
        "api-secret",
        "ticket-secret",
    ] {
        assert!(!rendered.contains(sentinel), "debug leaked {sentinel}");
    }
    assert!(rendered.contains("AGENTSCOMMANDER_API_TOKEN"));
    assert!(rendered.contains("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN"));

    let mut bytes =
        b"prefix api-secret middle ticket-secret path C:/project/.ac/wg-1-team/__agent_dev suffix"
            .to_vec();
    redact_command_bytes(&mut bytes, &spec);
    let output = String::from_utf8(bytes).unwrap();
    assert_eq!(
        output,
        "prefix [REDACTED] middle [REDACTED] path [REDACTED] suffix"
    );
}

#[test]
fn run_command_terminates_options_before_image() {
    let runtime = DockerRuntime::with_program("docker-test");
    let mut request = request();
    request.image = "--privileged".to_string();
    let spec = runtime.build_run_command(&request).unwrap();
    let image_idx = spec
        .args
        .iter()
        .position(|arg| arg == "--privileged")
        .expect("image arg");

    assert_eq!(spec.args[image_idx - 1], "--");
    assert_eq!(spec.args[image_idx + 1], DEFAULT_BRIDGE_ENTRYPOINT);
}

#[test]
fn run_command_appends_read_write_repo_mounts_before_image() {
    // #935 - each repo renders as its own --mount, AFTER the replica mount
    // and BEFORE the -- / image / entrypoint tail. If the loop were appended
    // after the tail, image_idx + 1 would be --mount and this test fails.
    use crate::pty::container_repos::ContainerRepoMount;
    let runtime = DockerRuntime::with_program("docker-test");
    let mut req = request();
    req.repo_mounts = vec![
        ContainerRepoMount {
            host_path: std::path::PathBuf::from("C:/project/.ac/wg-1-team/repo-AgentsCommander"),
            container_path: "/repos/repo-AgentsCommander".to_string(),
        },
        ContainerRepoMount {
            host_path: std::path::PathBuf::from("C:/project/.ac/wg-1-team/repo-webpage"),
            container_path: "/repos/repo-webpage".to_string(),
        },
    ];
    let spec = runtime.build_run_command(&req).unwrap();
    let joined = spec.args.join(" ");

    // Replica mount plus the two repos = three --mount flags.
    assert_eq!(spec.args.iter().filter(|a| *a == "--mount").count(), 3);
    let replica = spec
        .args
        .iter()
        .position(|a| a.contains("target=/workspace"))
        .expect("replica mount");
    let repo1 = spec
        .args
        .iter()
        .position(|a| a.contains("target=/repos/repo-AgentsCommander"))
        .expect("repo1 mount");
    let repo2 = spec
        .args
        .iter()
        .position(|a| a.contains("target=/repos/repo-webpage"))
        .expect("repo2 mount");
    // Order: replica first, then repos in config order.
    assert!(replica < repo1 && repo1 < repo2);
    assert!(joined.contains(
        "type=bind,source=C:/project/.ac/wg-1-team/repo-AgentsCommander,target=/repos/repo-AgentsCommander"
    ));
    // Read-write: no readonly token on the repo mounts (Q2 = RW).
    assert!(!joined.contains("readonly"));
    // The workdir stays the replica.
    let workdir_idx = spec
        .args
        .iter()
        .position(|a| a == "--workdir")
        .expect("workdir");
    assert_eq!(spec.args[workdir_idx + 1], "/workspace");
    // The repo mounts precede the image; the -- / entrypoint tail is intact.
    let image_idx = spec
        .args
        .iter()
        .position(|a| a == "ac-bridge:test")
        .expect("image arg");
    assert!(repo2 < image_idx, "repo mounts must precede the image");
    assert_eq!(spec.args[image_idx - 1], "--");
    assert_eq!(spec.args[image_idx + 1], DEFAULT_BRIDGE_ENTRYPOINT);
    // #993 - the resolver strips the Windows verbatim prefix, so the
    // rendered --mount args never carry \\?\ (dockerd rejects such a source).
    assert!(!joined.contains(r"\\?\"), "{joined}");
    // S7 free regression guard, now exercised WITH repo mounts.
    assert!(!joined.contains("messaging"));
}

#[test]
fn stop_and_orphan_list_commands_are_label_based() {
    let runtime = DockerRuntime::with_program("docker-test");
    let handle = ContainerRuntimeHandle {
        session_id: Uuid::nil(),
        container_id: "abc123".to_string(),
    };
    let stop = runtime.build_stop_command(&handle, Duration::from_secs(5));
    assert_eq!(stop.args, vec!["stop", "--time", "5", "abc123"]);

    let inspect = runtime.build_inspect_state_command(&handle);
    assert_eq!(
        inspect.args,
        vec!["inspect", "--format", "{{json .State}}", "abc123"]
    );

    let logs = runtime.build_logs_command(&handle, 80);
    assert_eq!(logs.args, vec!["logs", "--tail", "80", "abc123"]);

    let list = runtime.build_list_labeled_command();
    assert_eq!(list.args[0], "ps");
    assert!(list.args.contains(&format!("label={}", SESSION_LABEL)));
}

#[test]
fn stop_runs_force_remove_after_successful_stop() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let runtime = DockerRuntime::with_recorded_commands("docker-test", recorded.clone());
    let handle = ContainerRuntimeHandle {
        session_id: Uuid::nil(),
        container_id: "abc123".to_string(),
    };

    runtime
        .stop(
            &handle,
            Duration::from_secs(5),
            &ContainerRuntimeControl::default(),
        )
        .unwrap();

    let commands = recorded.lock().unwrap().clone();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].args, vec!["stop", "--time", "5", "abc123"]);
    assert_eq!(commands[1].args, vec!["rm", "-f", "abc123"]);
}

#[test]
fn ambiguous_start_outcomes_cleanup_the_deterministic_name() {
    let cases = vec![
        (
            "timeout",
            ScriptedDockerCommandResult::SpawnedError("primary timeout".to_string()),
            "primary timeout",
        ),
        (
            "command-error",
            ScriptedDockerCommandResult::SpawnedError("primary command error".to_string()),
            "primary command error",
        ),
        (
            "reader-error",
            ScriptedDockerCommandResult::ReaderError("primary reader error".to_string()),
            "primary reader error",
        ),
        (
            "empty-output",
            ScriptedDockerCommandResult::Output {
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            "empty container id",
        ),
        (
            "malformed-output",
            ScriptedDockerCommandResult::Output {
                stdout: b"not-a-container-id\n".to_vec(),
                stderr: Vec::new(),
            },
            "malformed container id",
        ),
    ];

    for (case, start_result, primary) in cases {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let runtime = DockerRuntime::with_scripted_commands(
            "docker-test",
            Arc::clone(&recorded),
            vec![
                start_result,
                ScriptedDockerCommandResult::Output {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
            ],
        );

        let error = runtime
            .start(request(), &ContainerRuntimeControl::default())
            .expect_err("ambiguous start must preserve the primary failure")
            .to_string();
        assert!(error.contains(primary), "case={case} error={error}");
        assert!(
            error.contains("state=terminal"),
            "case={case} error={error}"
        );
        assert!(runtime.retained_cleanup_contexts().is_empty(), "{case}");

        let commands = recorded
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(commands.len(), 2, "{case}");
        assert_eq!(commands[0].args.first().map(String::as_str), Some("run"));
        assert_eq!(
            commands[1].args,
            vec![
                "rm",
                "-f",
                "agentscommander-11111111111141118111111111111111"
            ],
            "{case}"
        );
    }
}

#[test]
fn ambiguous_start_cleanup_failure_is_retained_and_retried_once() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let runtime = DockerRuntime::with_scripted_commands(
        "docker-test",
        Arc::clone(&recorded),
        vec![
            ScriptedDockerCommandResult::SpawnedError("primary start timeout".to_string()),
            ScriptedDockerCommandResult::SpawnedError(
                "cleanup denied OPENAI_API_KEY=sk-proj-super-secret".to_string(),
            ),
            ScriptedDockerCommandResult::Output {
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ],
    );
    let session_id = request().session_id;

    let error = runtime
        .start(request(), &ContainerRuntimeControl::default())
        .expect_err("failed deterministic cleanup keeps the start failed")
        .to_string();
    assert!(error.starts_with("primary start timeout;"), "{error}");
    assert!(error.contains("state=retained"), "{error}");
    assert!(
        error.contains("cleanupError=cleanup denied OPENAI_API_KEY=[REDACTED]"),
        "{error}"
    );
    assert!(!error.contains("sk-proj-super-secret"), "{error}");
    let contexts = runtime.retained_cleanup_contexts();
    assert_eq!(contexts.len(), 1);
    let context = &contexts[0];
    assert_eq!(context.owner, "ambiguousStartCleanup");
    assert_eq!(context.session_id, Some(session_id));
    assert_eq!(context.reason, "ambiguous-start");
    assert_eq!(context.program, None);
    assert_eq!(context.runtime_handle, Some(true));
    assert_eq!(context.state, "retained");
    assert!(!context.in_flight);
    assert!(context
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("sk-proj-super-secret")));
    let diagnostic = context.diagnostic();
    assert!(diagnostic.contains("owner=ambiguousStartCleanup"));
    assert!(diagnostic.contains("state=retained"));
    assert!(diagnostic.contains("lastError=cleanup denied"));
    assert!(diagnostic.contains("OPENAI_API_KEY=[REDACTED]"));
    assert!(!diagnostic.contains("sk-proj-super-secret"));

    let retry_control = ContainerRuntimeControl::default();
    retry_control.request_shutdown(Instant::now() + Duration::from_secs(1));
    runtime.retry_retained_cleanups(&retry_control);
    assert!(runtime.retained_cleanup_contexts().is_empty());
    assert_eq!(
        recorded
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        3
    );

    runtime.retry_retained_cleanups(&retry_control);
    assert_eq!(
        recorded
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        3,
        "a terminal deterministic target must not be stopped twice"
    );
}

#[test]
fn permanently_blocked_command_readers_are_retained_at_the_deadline() {
    let runtime = DockerRuntime::new();
    let ready = Arc::new(std::sync::Barrier::new(3));
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let spawn_reader = |name: &'static str| {
        let ready = Arc::clone(&ready);
        let release = Arc::clone(&release);
        let active = Arc::clone(&runtime.active_readers);
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _active = ActiveDockerReaderGuard::new(active);
                ready.wait();
                let (released, changed) = &*release;
                let mut released = released.lock().unwrap_or_else(|error| error.into_inner());
                while !*released {
                    released = changed
                        .wait(released)
                        .unwrap_or_else(|error| error.into_inner());
                }
                Ok(CappedCommandStream::default())
            })
            .expect("spawn deterministic blocked Docker reader")
    };
    let mut owner = DockerCommandOwner {
        child: None,
        stdout: Some(spawn_reader("blocked-docker-stdout")),
        stderr: Some(spawn_reader("blocked-docker-stderr")),
        session_id: None,
        reason: "docker-command-reader",
        program: "docker-reader-fixture".to_string(),
        active_child: None,
    };
    ready.wait();
    assert_eq!(runtime.active_reader_count(), 2);

    let budget = Duration::from_millis(300);
    let started_at = Instant::now();
    assert!(owner.collect_output_until(started_at + budget).is_none());
    let error = runtime.retain_reader_owner_at_deadline(owner).to_string();
    let elapsed = started_at.elapsed();

    assert!(
        error.contains("readers exceeded the absolute deadline"),
        "{error}"
    );
    assert!(
        elapsed <= budget + Duration::from_millis(300),
        "blocked reader return exceeded its absolute deadline: {elapsed:?}"
    );
    assert_eq!(runtime.active_child_count(), 0);
    assert_eq!(runtime.active_reader_count(), 2);
    let contexts = runtime.retained_cleanup_contexts();
    assert_eq!(contexts.len(), 1);
    let context = &contexts[0];
    assert_eq!(context.owner, "dockerCommand");
    assert_eq!(context.session_id, None);
    assert_eq!(context.reason, "docker-command-reader");
    assert_eq!(context.program.as_deref(), Some("docker-reader-fixture"));
    assert_eq!(context.runtime_handle, None);
    assert_eq!(context.state, "retained");
    assert!(!context.in_flight);
    assert_eq!(
        context.last_error.as_deref(),
        Some("readers exceeded the absolute deadline program=docker-reader-fixture")
    );
    let diagnostic = context.diagnostic();
    assert!(diagnostic.contains("owner=dockerCommand"));
    assert!(diagnostic.contains("session=none"));
    assert!(diagnostic.contains("reason=docker-command-reader"));
    assert!(diagnostic.contains("program=docker-reader-fixture"));
    assert!(diagnostic.contains(
        "lastError=readers exceeded the absolute deadline program=docker-reader-fixture"
    ));
    assert!(!diagnostic.contains("00000000-0000-0000-0000-000000000000"));

    let (released, changed) = &*release;
    *released.lock().unwrap_or_else(|error| error.into_inner()) = true;
    changed.notify_all();
    runtime
        .command_ownership
        .retry_until(Instant::now() + Duration::from_secs(1));
    assert_eq!(runtime.active_reader_count(), 0);
    assert!(runtime.retained_cleanup_contexts().is_empty());
}

#[test]
fn shutdown_control_kills_and_reaps_command_child_before_deadline() {
    let runtime = Arc::new(DockerRuntime::new());
    let control = ContainerRuntimeControl::default();
    #[cfg(windows)]
    let spec = DockerCommandSpec {
        program: "powershell.exe".to_string(),
        args: vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            "Start-Sleep -Seconds 30".to_string(),
        ],
        secret_env: BTreeMap::new(),
        redacted_values: Vec::new(),
    };
    #[cfg(not(windows))]
    let spec = DockerCommandSpec {
        program: "sleep".to_string(),
        args: vec!["30".to_string()],
        secret_env: BTreeMap::new(),
        redacted_values: Vec::new(),
    };
    let worker_runtime = Arc::clone(&runtime);
    let worker_control = control.clone();
    let command = std::thread::Builder::new()
        .name("docker-runtime-cancellation-test".to_string())
        .spawn(move || {
            worker_runtime.run_command_output_with_control(
                spec,
                &worker_control,
                DockerCommandShutdownBehavior::CancelOnShutdown,
            )
        })
        .expect("spawn Docker cancellation test command");

    let child_deadline = Instant::now() + Duration::from_secs(5);
    while runtime.active_child_count() == 0 {
        assert!(
            Instant::now() < child_deadline,
            "Docker cancellation test child did not start"
        );
        std::thread::yield_now();
    }
    let shutdown_started = Instant::now();
    let shutdown_deadline = shutdown_started + Duration::from_millis(500);
    control.request_shutdown(shutdown_deadline);
    let error = command
        .join()
        .expect("join Docker cancellation test command")
        .expect_err("shutdown cancels the blocking command")
        .to_string();
    assert!(error.contains("canceled by shutdown"), "{error}");
    assert!(
        shutdown_started.elapsed() <= Duration::from_secs(1),
        "Docker child cancellation exceeded its deadline bound"
    );
    assert_eq!(runtime.active_child_count(), 0);
    assert_eq!(runtime.active_reader_count(), 0);
}

#[test]
fn read_capped_to_end_retains_limit_and_reports_truncation() {
    let oversized = vec![b'x'; DOCKER_COMMAND_OUTPUT_BYTE_LIMIT + 17];
    let capped =
        read_capped_to_end(Cursor::new(oversized), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT).unwrap();
    assert_eq!(capped.bytes.len(), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT);
    assert!(capped.truncated);

    let exact = vec![b'y'; DOCKER_COMMAND_OUTPUT_BYTE_LIMIT];
    let uncapped =
        read_capped_to_end(Cursor::new(exact), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT).unwrap();
    assert_eq!(uncapped.bytes.len(), DOCKER_COMMAND_OUTPUT_BYTE_LIMIT);
    assert!(!uncapped.truncated);
}

#[test]
fn parse_labeled_containers_ignores_malformed_rows() {
    let rows = "abc\t11111111-1111-4111-8111-111111111111\nbad-row\n\tmissing\n";
    assert_eq!(
        DockerRuntime::parse_labeled_containers(rows),
        vec![(
            "abc".to_string(),
            "11111111-1111-4111-8111-111111111111".to_string()
        )]
    );
}

#[test]
fn parse_container_state_reads_docker_inspect_state_json() {
    let raw = r#"{"Status":"exited","Running":false,"ExitCode":127,"Error":""}"#;
    let state = DockerRuntime::parse_container_state(raw).unwrap();

    assert_eq!(state.status.as_deref(), Some("exited"));
    assert_eq!(state.running, Some(false));
    assert_eq!(state.exit_code, Some(127));
    assert_eq!(state.error.as_deref(), Some(""));
}
