use super::*;

mod enqueue;

#[test]
fn send_after_help_documents_enqueue_receipt() {
    // #1596: the after_help must document the `Queued:` enqueue receipt so
    // callers know a missing receipt line means NOT enqueued.
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let send = cmd
        .get_subcommands()
        .find(|c| c.get_name() == "send")
        .expect("send subcommand");
    let after = send
        .get_after_help()
        .expect("after_help present")
        .to_string();
    assert!(
        after.contains("On enqueue, the CLI prints `Queued: <message-id>` to stdout"),
        "after_help missing enqueue-receipt sentence"
    );
    assert!(
        after.contains("a missing `Queued:` line means the message was NOT enqueued"),
        "after_help missing NOT-enqueued sentence"
    );
}

fn make_verified_coordinator_fixture() -> (tempfile::TempDir, Vec<String>) {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let origin_tech_lead = ac_root.join("_agent_tech-lead");
    let origin_dev_rust = ac_root.join("_agent_dev-rust");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let tech_lead_replica = wg_dir.join("__agent_tech-lead");
    let dev_rust_replica = wg_dir.join("__agent_dev-rust");

    for dir in [
        &team_dir,
        &origin_tech_lead,
        &origin_dev_rust,
        &tech_lead_replica,
        &dev_rust_replica,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        tech_lead_replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .unwrap();
    std::fs::write(
        dev_rust_replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();

    let paths = vec![temp.path().to_string_lossy().to_string()];
    (temp, paths)
}

#[test]
fn root_agent_sender_uses_reserved_constant() {
    assert_eq!(
        sender_for_root("C:/tmp/agentscommander/ac-root-agent", true),
        crate::config::root_agent::ROOT_AGENT_SENDER
    );
    assert_ne!(
        sender_for_root("C:/tmp/agentscommander/_agent_root-agent", false),
        crate::config::root_agent::ROOT_AGENT_SENDER
    );
}

#[test]
fn root_agent_routing_rejects_origin_coordinator() {
    let (_temp, paths) = make_verified_coordinator_fixture();

    assert!(!root_agent_target_allowed("proj-a/tech-lead", &paths));
}

#[test]
fn send_rejects_send_path_for_wg() {
    let filename = r"messaging\20260524-040000-wg1-a-to-wg1-b-x.md";

    assert!(crate::phone::messaging::validate_filename_only(filename).is_err());
}

#[test]
fn send_rejects_send_path_for_root() {
    let filename = r"messaging\20260524-040000-root-to-wg1-tech-lead-x.md";

    assert!(crate::phone::messaging::validate_filename_only(filename).is_err());
}

#[test]
fn root_agent_send_accepts_verified_wg_coordinator() {
    let (_temp, paths) = make_verified_coordinator_fixture();

    assert!(root_agent_target_allowed(
        "proj-a:wg-1-dev-team/tech-lead",
        &paths
    ));
}

#[test]
fn coordinator_to_root_target_allowed_accepts_verified_coordinator() {
    let (_temp, paths) = make_verified_coordinator_fixture();
    assert!(coordinator_to_root_target_allowed(
        "proj-a:wg-1-dev-team/tech-lead",
        &paths
    ));
}

#[test]
fn coordinator_to_root_target_allowed_rejects_non_coordinator() {
    let (_temp, paths) = make_verified_coordinator_fixture();
    assert!(!coordinator_to_root_target_allowed(
        "proj-a:wg-1-dev-team/dev-rust",
        &paths
    ));
}

#[test]
fn coordinator_to_root_target_allowed_rejects_origin_agent() {
    let (_temp, paths) = make_verified_coordinator_fixture();
    assert!(!coordinator_to_root_target_allowed(
        "proj-a/tech-lead",
        &paths
    ));
}

#[test]
fn root_agent_command_clear_and_compact_are_rejected_before_outbox_write() {
    for command in ["clear", "compact"] {
        assert_eq!(
            validate_root_agent_delivery_kind(true, Some(command)),
            Err(
                "Root Agent messaging is file-based; use --send with a root-to-orchestrator \
                Markdown file, not --command"
            )
        );
    }
}

#[test]
fn non_root_command_behavior_is_unchanged() {
    assert_eq!(
        validate_root_agent_delivery_kind(false, Some("compact")),
        Ok(())
    );
}

#[test]
fn get_output_does_not_tighten_live_pty_clamp() {
    // The live wake path never injects the `--get-output` response markers
    // (`phone::mailbox` gates them on a non-interactive session, unreachable
    // today), so the CLI clamp must budget a `--get-output` send exactly like
    // a plain send. The live clamp calls `notification_pty_overhead(_, false)`;
    // this locks that a near-limit path is not rejected over marker bytes that
    // never reach the PTY, guarding against re-keying it on `args.get_output`.
    let sender_len = "project:wg-1-team/agent".len();
    let live_overhead = notification_pty_overhead(sender_len, false);
    let plain_overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender_len;

    // Live budget is the plain wrap: get-output adds nothing on the live path.
    assert_eq!(live_overhead, plain_overhead);

    // A body sized to exactly fill the plain-wrap budget must be ACCEPTED by
    // the clamp (`body.len() + overhead > PTY_SAFE_MAX` is false).
    let body_len = crate::phone::messaging::PTY_SAFE_MAX - plain_overhead;
    assert!(body_len + live_overhead <= crate::phone::messaging::PTY_SAFE_MAX);

    // Under the reverted tightening, counting the never-injected marker
    // overhead would have rejected this same body. Confirm the corrected live
    // path does not, so the tightening cannot silently return.
    let tightened_overhead =
        plain_overhead + crate::phone::messaging::PTY_RESPONSE_MARKER_FIXED + 2 * REQUEST_ID_LEN;
    assert!(body_len + tightened_overhead > crate::phone::messaging::PTY_SAFE_MAX);
}

#[test]
fn wait_for_delivery_confirmation_accepts_delivered_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let delivered_dir = temp.path().join("delivered");
    std::fs::create_dir_all(&delivered_dir).unwrap();
    std::fs::write(delivered_dir.join("msg-1.json"), "{}").unwrap();

    let result = wait_for_delivery_confirmation(
        temp.path(),
        "msg-1",
        "wake",
        "project:wg-1-team/agent",
        std::time::Duration::from_millis(10),
        std::time::Duration::from_millis(1),
    );

    assert!(result.is_ok());
}

/// #1635-2 D4: a minimal delivered message carrying the daemon's receipt
/// annotations (the shape `move_to_delivered` writes for a dispatch wake).
fn annotated_delivered_message() -> OutboxMessage {
    OutboxMessage {
        id: "msg-ann-1".to_string(),
        token: None,
        from: "proj:wg-1/tech-lead".to_string(),
        to: "proj:wg-1/dev-rust".to_string(),
        body: "wake body".to_string(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "codex".to_string(),
        requested_profile: Some("C".to_string()),
        effective_agent_id: Some("codex".to_string()),
        effective_profile: Some("B".to_string()),
        profile_fallback_applied: true,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: "2026-08-30T00:00:00Z".to_string(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

#[test]
fn delivered_receipt_prints_effective_values_and_fallback() {
    let line = delivered_receipt_line(
        "msg-ann-1",
        "wake",
        "proj:wg-1/dev-rust",
        Some(&annotated_delivered_message()),
    );
    assert!(line.starts_with("Delivered: msg-ann-1 (mode=wake, to=proj:wg-1/dev-rust)"));
    assert!(line.contains("agent=codex"), "line was: {line}");
    assert!(line.contains("profile=B"), "line was: {line}");
    assert!(line.contains("fallbackApplied"), "line was: {line}");
}

#[test]
fn delivered_receipt_prints_flags_not_applied_for_live() {
    let mut message = annotated_delivered_message();
    message.effective_agent_id = None;
    message.effective_profile = None;
    message.profile_fallback_applied = false;
    message.dispatch_not_applied = Some("live-target".to_string());
    let line = delivered_receipt_line("msg-live-1", "wake", "proj:wg-1/dev-rust", Some(&message));
    assert!(line.starts_with("Delivered: msg-live-1 (mode=wake, to=proj:wg-1/dev-rust)"));
    assert!(
        line.contains("flagsNotApplied=live-target"),
        "line was: {line}"
    );
    assert!(!line.contains("fallbackApplied"), "line was: {line}");
}

#[test]
fn delivered_receipt_unparseable_file_falls_back_to_legacy_line() {
    // A foreign/older daemon writes no annotation and the delivered JSON may
    // predate required fields; the confirmation must still succeed with the
    // legacy byte-identical line.
    let temp = tempfile::TempDir::new().unwrap();
    let delivered_dir = temp.path().join("delivered");
    std::fs::create_dir_all(&delivered_dir).unwrap();
    std::fs::write(delivered_dir.join("msg-legacy.json"), "{}").unwrap();

    let result = wait_for_delivery_confirmation(
        temp.path(),
        "msg-legacy",
        "wake",
        "proj:wg-1/dev-rust",
        std::time::Duration::from_millis(10),
        std::time::Duration::from_millis(1),
    );
    assert!(result.is_ok());
    assert_eq!(
        delivered_receipt_line("msg-legacy", "wake", "proj:wg-1/dev-rust", None),
        "Delivered: msg-legacy (mode=wake, to=proj:wg-1/dev-rust)"
    );
}

#[test]
fn wait_for_delivery_confirmation_reports_rejection_reason() {
    let temp = tempfile::TempDir::new().unwrap();
    let rejected_dir = temp.path().join("rejected");
    std::fs::create_dir_all(&rejected_dir).unwrap();
    std::fs::write(rejected_dir.join("msg-2.reason.txt"), "bad route").unwrap();

    let err = wait_for_delivery_confirmation(
        temp.path(),
        "msg-2",
        "wake",
        "project:wg-1-team/agent",
        std::time::Duration::from_millis(10),
        std::time::Duration::from_millis(1),
    )
    .unwrap_err();

    assert!(err.contains("message rejected: bad route"));
}

#[test]
fn wait_for_delivery_confirmation_reports_pending_outbox_on_timeout() {
    let temp = tempfile::TempDir::new().unwrap();

    let err = wait_for_delivery_confirmation(
        temp.path(),
        "msg-3",
        "wake",
        "project:wg-1-team/agent",
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(1),
    )
    .unwrap_err();

    assert!(err.contains("delivery confirmation timeout"));
    assert!(err.contains("msg-3"));
    assert!(err.contains(&temp.path().display().to_string()));
}

#[test]
fn wait_for_response_returns_present_file_at_zero_timeout() {
    // Regression lock for the ordering bug: a response already on disk must
    // be read even when the timeout has fully elapsed (timeout == 0). The old
    // timeout-first ordering dropped it and returned a timeout error instead.
    let temp = tempfile::TempDir::new().unwrap();
    let response_path = temp.path().join("resp.json");
    std::fs::write(&response_path, "{\"ok\":true}").unwrap();

    let result = wait_for_response(
        &response_path,
        std::time::Duration::from_secs(0),
        std::time::Duration::from_millis(1),
    );

    assert_eq!(result.unwrap(), "{\"ok\":true}");
}

#[test]
fn wait_for_response_returns_present_file_within_window() {
    let temp = tempfile::TempDir::new().unwrap();
    let response_path = temp.path().join("resp.json");
    std::fs::write(&response_path, "hello").unwrap();

    let result = wait_for_response(
        &response_path,
        std::time::Duration::from_millis(50),
        std::time::Duration::from_millis(1),
    );

    assert_eq!(result.unwrap(), "hello");
}

#[test]
fn wait_for_response_times_out_when_absent() {
    // The timeout must still fire (no infinite loop) when no file appears.
    let temp = tempfile::TempDir::new().unwrap();
    let missing = temp.path().join("never.json");

    let err = wait_for_response(
        &missing,
        std::time::Duration::from_millis(5),
        std::time::Duration::from_millis(1),
    )
    .unwrap_err();

    assert!(err.contains("timeout waiting for response"));
}

#[test]
fn derive_root_project_dir_walks_up_wg_replica_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-x");
    let agent_root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
    std::fs::create_dir_all(&agent_root).unwrap();

    // On Windows, std::fs::canonicalize returns `\\?\C:\...` extended-length
    // paths; compute the expected via canonicalize so the assertion holds on
    // both Windows and Unix. Mirror pattern from list_peers.rs:972-1010
    // (`extended_length_prefix_normalizes`).
    let expected = std::fs::canonicalize(&project)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let got = derive_root_project_dir(agent_root.to_str().unwrap())
        .unwrap()
        .expect("should derive project from WG replica path");
    assert_eq!(got, expected);
}

#[test]
fn derive_root_project_dir_accepts_ac_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-x");
    let root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
    std::fs::create_dir_all(&root).unwrap();

    let got = derive_root_project_dir(root.to_str().unwrap())
        .unwrap()
        .expect(".ac workspace should be accepted");
    let expected = std::fs::canonicalize(&project)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(got, expected);
}

#[test]
fn derive_root_project_dir_returns_none_for_non_wg_path() {
    let temp = tempfile::TempDir::new().unwrap();
    // A random subdir without the WG-replica shape.
    let random = temp.path().join("random").join("dir");
    std::fs::create_dir_all(&random).unwrap();
    assert!(derive_root_project_dir(random.to_str().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn derive_root_project_dir_returns_none_when_one_level_too_high() {
    let temp = tempfile::TempDir::new().unwrap();
    let wg_dir = temp.path().join("proj-x").join(".ac").join("wg-1-devs");
    std::fs::create_dir_all(&wg_dir).unwrap();
    // Pointing at the WG dir, not the __agent_* dir → must return None.
    assert!(derive_root_project_dir(wg_dir.to_str().unwrap())
        .unwrap()
        .is_none());
}

// ── clap parse smoke: #782 --confirm-timeout ──

fn parse_send_args(extra: &[&str]) -> SendArgs {
    use clap::Parser;
    let mut argv = vec![
        "agentscommander",
        "send",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--to",
        "proj-a/agent",
        "--send",
        "20260704-000000-wg1-a-to-wg1-b-x.md",
        "--root",
        "anything",
    ];
    argv.extend_from_slice(extra);
    let parsed = crate::cli::Cli::try_parse_from(argv).expect("clap should accept send args");
    match parsed.command.expect("subcommand present") {
        crate::cli::Commands::Send(args) => args,
        _ => panic!("expected Send subcommand"),
    }
}

fn try_parse_payload_args(payload: &[&str]) -> Result<SendArgs, clap::Error> {
    use clap::Parser;
    let mut argv = vec![
        "agentscommander",
        "send",
        "--token",
        "11111111-1111-4111-8111-111111111111",
        "--to",
        "proj-a:wg-1-dev-team/dev-rust",
        "--root",
        "anything",
    ];
    argv.extend_from_slice(payload);
    let parsed = crate::cli::Cli::try_parse_from(argv)?;
    match parsed.command.expect("subcommand present") {
        crate::cli::Commands::Send(args) => Ok(args),
        _ => panic!("expected Send subcommand"),
    }
}

#[test]
fn send_payload_group_accepts_four_single_forms_and_rejects_every_pair() {
    let forms: [&[&str]; 4] = [
        &["--send", "20260704-000000-wg1-a-to-wg1-b-x.md"],
        &["--command", "clear"],
        &["--pty-input", "-literal shell $(text)"],
        &["--pty-input-stdin"],
    ];
    for form in forms {
        assert!(try_parse_payload_args(form).is_ok(), "single form {form:?}");
    }
    assert!(try_parse_payload_args(&[]).is_err());
    for left in 0..forms.len() {
        for right in (left + 1)..forms.len() {
            let args: Vec<&str> = forms[left]
                .iter()
                .chain(forms[right].iter())
                .copied()
                .collect();
            assert!(
                try_parse_payload_args(&args).is_err(),
                "payload pair {left}/{right}"
            );
        }
    }
}

#[test]
fn pty_input_conflicts_and_bounded_reader_are_exact() {
    assert!(try_parse_payload_args(&["--pty-input", "x", "--get-output"]).is_err());
    assert!(try_parse_payload_args(&["--pty-input", "x", "--outbox", "custom",]).is_err());
    let exact = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES];
    assert_eq!(
        read_bounded_pty_input(exact.as_slice()).unwrap().len(),
        exact.len()
    );
    let oversized = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES + 1];
    assert_eq!(
        read_bounded_pty_input(oversized.as_slice()),
        Err("payload_too_large")
    );
    assert_eq!(read_bounded_pty_input(&[0xff][..]), Err("invalid_text"));
}

#[test]
fn host_artifact_validation_requires_every_correlation_field_and_directory() {
    use crate::phone::types::{
        canonical_pty_timestamp, PtyInputHostArtifact, PtyInputPublicStatus, PtyInputReason,
        PtyInputReasonCode, PtyInputResult, PtyInputSourcePlane,
    };

    let temp = tempfile::tempdir().unwrap();
    let injection_id = Uuid::new_v4().to_string();
    let issued = chrono::Utc::now();
    let issued_at = canonical_pty_timestamp(issued);
    let expires_at = canonical_pty_timestamp(
        issued + chrono::Duration::seconds(crate::phone::types::PTY_INPUT_TTL_SECS),
    );
    let payload_sha256 = crate::phone::types::sha256_hex(b"exact");
    let expected = ExpectedPtyArtifact {
        injection_id: &injection_id,
        op_id: &injection_id,
        sender: "project:wg-1-team/lead",
        target: "project:wg-1-team/member",
        payload_bytes: 5,
        payload_sha256: &payload_sha256,
        issued_at: &issued_at,
        expires_at: &expires_at,
        confirmation_tag: "confirmation",
    };
    let mut result = PtyInputResult::new(injection_id.clone(), PtyInputPublicStatus::Rejected);
    result.op_id = Some(injection_id.clone());
    result.sender = Some(expected.sender.to_string());
    result.target = Some(expected.target.to_string());
    result.payload_bytes = Some(expected.payload_bytes);
    result.payload_sha256 = Some(payload_sha256.clone());
    result.source_plane = Some(PtyInputSourcePlane::HostCli);
    result.issued_at = Some(issued_at.clone());
    result.expires_at = Some(expires_at.clone());
    result.queued_at = Some(canonical_pty_timestamp(issued));
    result.terminal_at = Some(canonical_pty_timestamp(issued));
    result.reason = Some(PtyInputReason::from_code(PtyInputReasonCode::Busy));
    let artifact = PtyInputHostArtifact {
        result,
        confirmation_tag: expected.confirmation_tag.to_string(),
    };
    let path = temp.path().join("artifact.json");
    let artifact_bytes = serde_json::to_vec(&artifact).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&artifact_bytes)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["confirmationTag".to_string(), "result".to_string()]
            .into_iter()
            .collect()
    );
    std::fs::write(&path, artifact_bytes).unwrap();
    assert!(validate_pty_artifact(&path, &expected).is_ok());

    let mut tampered = artifact.clone();
    tampered.confirmation_tag = "copied".to_string();
    std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert_eq!(
        validate_pty_artifact(&path, &expected),
        Err("invalid_artifact")
    );

    let delivered = temp.path().join("delivered");
    std::fs::create_dir(&delivered).unwrap();
    std::fs::write(
        delivered.join(format!("{injection_id}.json")),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    assert_eq!(
        find_pty_input_terminal_artifact(temp.path(), &expected),
        Err("invalid_artifact")
    );
}

#[test]
fn issue_3171_selector_uses_sender_identity() {
    let incident = "20261022-091917-root-agent-to-room21-fixer-reinstalled.md";
    assert!(validate_send_filename(incident, true).is_err());
    assert!(validate_send_filename(incident, false).is_ok());
    let root = "20261022-092418-root-to-room21-ac-tech-lead-v5-fixer-installation-confirmed.md";
    assert!(validate_send_filename(root, true).is_ok());
    assert!(crate::phone::messaging::validate_root_notification_filename(root).is_ok());
    for name in [
        "20261022-093000-room21-ac-dev-rust-v5-to-room21-ac-tech-lead-v5-status.1.md",
        "20261022-093000-wg21-ac-dev-rust-v5-to-room21-ac-tech-lead-v5-status.md",
    ] {
        assert!(validate_send_filename(name, false).is_ok(), "{name}");
        assert!(validate_send_filename(name, true).is_err(), "{name}");
    }
}

#[test]
fn issue_3171_long_help_documents_filename_and_receipts() {
    use clap::CommandFactory;
    let mut command = crate::cli::Cli::command();
    let send = command.find_subcommand_mut("send").unwrap();
    let help = send.render_long_help().to_string();
    for rule in [
        "YYYYMMDD-HHMMSS-<from>-to-<to>-<slug>[.N].md",
        "8/6 ASCII digits",
        "UTC convention",
        "calendar dates and time ranges are not checked",
        "non-empty [a-z0-9]+ segments",
        "at least one segment before and two after",
        "routing uses --to and the authenticated sender",
        "lowercase .md",
        "no /, backslash, colon, traversal",
        ".1, .01 and .99 pass; .0, .00 and .100 fail",
        "sanitize_slug generates up to 50 characters",
        "no validator slug-length limit",
        "root-agent-to is invalid for Root",
        "legacy wg21-ac-dev-rust-v5",
        "Queued confirms enqueue only",
        "Delivered confirms system delivery",
        "not reading or completed work",
        "delivered/<id>.json",
        "rejected/<id>.json",
        "rejected/<id>.reason.txt",
        "--outbox if supplied",
        "master token uses the app outbox",
        "Session tokens use <root>/<local-dir>/outbox",
        "local-dir is configured per instance",
        "on confirmation timeout the CLI exits 1",
    ] {
        assert!(help.contains(rule), "missing help rule: {rule}");
    }
    for (example, root) in [
        (
            "20261022-092418-root-to-room21-ac-tech-lead-v5-fixer-installation-confirmed.md",
            true,
        ),
        (
            "20261022-093000-room21-ac-dev-rust-v5-to-room21-ac-tech-lead-v5-status.1.md",
            false,
        ),
    ] {
        assert!(help.contains(example));
        assert!(validate_send_filename(example, root).is_ok());
    }
}

#[test]
fn issue_3171_shape_error_is_actionable_and_single_line() {
    let name = "20261022-091917-root-agent-to-room21-fixer-reinstalled.md";
    let error = validate_send_filename(name, true).unwrap_err();
    assert!(matches!(
        error,
        crate::phone::messaging::MessagingError::InvalidShape(_)
    ));
    let message = error.to_string();
    assert!(message.starts_with(&format!(
        "filename '{name}' does not match the required shape"
    )));
    assert!(message.contains("YYYYMMDD-HHMMSS-<from>-to-<to>-<slug>[.N].md"));
    assert!(message.contains("if the sender is Root:"));
    assert!(message.contains(
        "20261022-092418-root-to-room21-ac-tech-lead-v5-fixer-installation-confirmed.md"
    ));
    assert!(!message.contains(['\n', '\r']));
}

#[test]
fn p04_pty_publication_preserves_bytes_and_regular_collisions() {
    let directory = tempfile::tempdir().unwrap();
    let outbox = directory.path();
    let publish = |temp, final_path| {
        crate::filesystem::publish_cli_pty_request(
            outbox,
            temp,
            final_path,
            b"private payload",
            crate::path_identity::verify_directory(outbox).unwrap(),
            1024,
        )
    };
    let destination = outbox.join("published.json");
    assert_eq!(
        publish(outbox.join("temporary"), destination.clone()).unwrap(),
        destination
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"private payload");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        publish(outbox.join("unused"), destination.clone()).unwrap_err(),
        "publish_ambiguous"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"private payload");
    assert!(!outbox.join("unused").exists());
    let temporary = outbox.join("occupied");
    std::fs::write(&temporary, b"keep temporary").unwrap();
    let absent = outbox.join("absent.json");
    assert_eq!(
        publish(temporary.clone(), absent.clone()).unwrap_err(),
        "publish_failed"
    );
    assert_eq!(std::fs::read(temporary).unwrap(), b"keep temporary");
    assert!(!absent.exists());
}

#[test]
fn p04_pty_publication_rejects_replaced_outbox_identity() {
    let directory = tempfile::tempdir().unwrap();
    let outbox = directory.path().join("outbox");
    std::fs::create_dir(&outbox).unwrap();
    std::fs::write(outbox.join("retained"), b"original").unwrap();
    let identity = crate::path_identity::verify_directory(&outbox).unwrap();
    let previous = directory.path().join("previous");
    std::fs::rename(&outbox, &previous).unwrap();
    std::fs::create_dir(&outbox).unwrap();
    let final_path = outbox.join("request.json");
    assert_eq!(
        crate::filesystem::publish_cli_pty_request(
            &outbox,
            outbox.join("temporary"),
            final_path.clone(),
            b"payload",
            identity,
            1024,
        )
        .unwrap_err(),
        "publish_ambiguous"
    );
    assert!(!final_path.exists());
    assert_eq!(
        std::fs::read(previous.join("retained")).unwrap(),
        b"original"
    );
}
