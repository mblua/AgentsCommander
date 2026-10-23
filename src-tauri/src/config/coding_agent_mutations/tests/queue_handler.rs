use super::*;

// ---- queue helpers -----------------------------------------------------

#[test]
fn is_expired_backward_clock_and_old() {
    // future createdAt (backward clock) -> not expired
    assert!(!is_expired(1_000_000, 900_000));
    // fresh -> not expired
    assert!(!is_expired(1_000_000, 1_000_100));
    // genuinely old -> expired
    assert!(is_expired(0, REQUEST_EXPIRY_MS + 1));
}

#[test]
fn request_result_roundtrip_atomic() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let req = CodingAgentRequest {
        request_id: "req1".into(),
        created_at_ms: 123,
        op: add(agent("claude", "Claude", "claude")),
    };
    let path = write_coding_agent_request(&requests, &req).unwrap();
    assert!(path.exists());
    // no leftover temp file
    assert!(std::fs::read_dir(&requests).unwrap().all(|e| {
        let p = e.unwrap().path();
        p.extension().and_then(|s| s.to_str()) == Some("json")
    }));

    let results = requests.join(RESULTS_SUBDIR);
    write_coding_agent_result(
        &results,
        &CodingAgentResult {
            request_id: "req1".into(),
            ok: true,
            op: Some("add".into()),
            error: None,
            agent: None,
            removed_id: None,
        },
    )
    .unwrap();
    let read = read_coding_agent_result(&results, "req1").unwrap();
    assert!(read.ok);
    assert_eq!(read.op.as_deref(), Some("add"), "op round-trips");
}

#[test]
fn first_request_autocreates_results_dir() {
    // R4: brand-new tempdir, results/ does not exist; write_coding_agent_result
    // must create it.
    let dir = crate::config::settings::source_transition_test_root();
    let results = dir
        .path()
        .join(CODING_AGENT_REQUESTS_DIR)
        .join(RESULTS_SUBDIR);
    assert!(!results.exists());
    write_coding_agent_result(
        &results,
        &CodingAgentResult {
            request_id: "x".into(),
            ok: true,
            op: None,
            error: None,
            agent: None,
            removed_id: None,
        },
    )
    .unwrap();
    assert!(results.join("x.json").exists());
}

// ---- per-request handler ----------------------------------------------

#[test]
fn handler_applies_and_writes_result_and_deletes_processing() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(
        &requests,
        "r1",
        100,
        add(agent("claude", "Claude", "claude")),
    );

    let mut settings = AppSettings::default();
    let mut saved = false;
    let disp = {
        let mut save = |_s: &AppSettings| {
            saved = true;
            Ok(_s.clone())
        };
        process_coding_agent_request(&path, &results, 200, &mut settings, &mut save)
    };
    assert!(matches!(
        disp,
        RequestDisposition::Applied { op: "add", .. }
    ));
    assert!(saved, "save callback invoked");
    assert_eq!(settings.agents.len(), 1);
    // result written, request + processing gone
    assert!(read_coding_agent_result(&results, "r1").unwrap().ok);
    assert!(!path.exists());
    assert!(!processing_path_for(&path).exists());
}

#[test]
fn handler_applies_backend_image_patch_from_request_json() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let mut settings = AppSettings::default();
    apply_coding_agent_op(&mut settings, &add(agent("claude", "Claude", "claude"))).unwrap();
    let patch = AgentPatch {
        backend_kind: Some(SessionBackendKind::ContainerTransport),
        container_image: Some(" image:tag ".into()),
        ..Default::default()
    };
    let path = write_request_file(
        &requests,
        "patch",
        100,
        CodingAgentOp::Update {
            id: "claude".into(),
            patch,
        },
    );

    let disp = process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());

    assert!(matches!(disp, RequestDisposition::Applied { .. }));
    assert_eq!(
        settings.agents[0].backend.kind,
        SessionBackendKind::ContainerTransport
    );
    assert_eq!(
        settings.agents[0].backend.image.as_deref(),
        Some("image:tag")
    );
    assert!(read_coding_agent_result(&results, "patch").unwrap().ok);
}

#[test]
fn handler_skips_when_request_already_removed() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    std::fs::create_dir_all(&requests).unwrap();
    let missing = requests.join("gone.json");
    let mut settings = AppSettings::default();
    let disp =
        process_coding_agent_request(&missing, &results, 200, &mut settings, &mut noop_save());
    assert_eq!(disp, RequestDisposition::Skipped);
    assert!(settings.agents.is_empty());
}

#[test]
fn handler_rejects_expired_without_applying() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(
        &requests,
        "old",
        0,
        add(agent("claude", "Claude", "claude")),
    );
    let mut settings = AppSettings::default();
    let disp = process_coding_agent_request(
        &path,
        &results,
        REQUEST_EXPIRY_MS + 10,
        &mut settings,
        &mut noop_save(),
    );
    assert_eq!(disp, RequestDisposition::Rejected);
    assert!(settings.agents.is_empty());
    let res = read_coding_agent_result(&results, "old").unwrap();
    assert!(!res.ok);
    assert!(res.error.unwrap().contains("expired"));
}

#[test]
fn handler_rejects_validation_error() {
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(&requests, "bad", 100, add(agent("BadId", "L", "claude")));
    let mut settings = AppSettings::default();
    let disp = process_coding_agent_request(&path, &results, 150, &mut settings, &mut noop_save());
    assert_eq!(disp, RequestDisposition::Rejected);
    assert!(settings.agents.is_empty());
    assert!(!read_coding_agent_result(&results, "bad").unwrap().ok);
}

// ---- T10: daemon-path launch consumption (acceptance criterion 2) ------

#[test]
fn daemon_applied_agent_is_launch_resolvable() {
    // Apply an Add through the extracted per-request handler (the daemon-path
    // seam), then confirm the new agent is resolvable to a subsequent
    // `--launch` by id AND by label, and that a spawn command builds for it.
    let dir = crate::config::settings::source_transition_test_root();
    let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
    let results = requests.join(RESULTS_SUBDIR);
    let path = write_request_file(
        &requests,
        "r",
        100,
        add(agent("claude", "Claude Code", "claude")),
    );

    let mut settings = AppSettings::default();
    let disp = process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());
    assert!(matches!(disp, RequestDisposition::Applied { .. }));

    // find_launch_agent (create_agent.rs) resolves by exact id and by label.
    assert!(crate::cli::create_agent::find_launch_agent(&settings, "claude").is_some());
    assert!(crate::cli::create_agent::find_launch_agent(&settings, "Claude Code").is_some());

    // A spawn command builds against the same state (session.rs). Some(_)
    // means the agent was found and the command composed.
    let cwd = dir.path().to_string_lossy();
    let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
        &settings, "claude", &cwd, None,
    );
    assert!(spawn.is_ok(), "spawn build failed: {:?}", spawn.err());
    assert!(
        spawn.unwrap().is_some(),
        "expected a spawn command for the applied agent"
    );
}
