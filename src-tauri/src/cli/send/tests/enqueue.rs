use super::*;

#[test]
fn send_args_confirm_timeout_defaults_to_90() {
    let args = parse_send_args(&[]);
    assert_eq!(
        args.confirm_timeout, 90,
        "--confirm-timeout must default to 90"
    );
    assert_eq!(
        confirm_timeout_from_args(&args),
        std::time::Duration::from_secs(90),
        "default must map to the 90s ceiling handed to wait_for_delivery_confirmation"
    );
    // The --get-output response wait keeps its own independent default.
    assert_eq!(args.timeout, 300);
}

#[test]
fn send_args_confirm_timeout_override_reaches_confirmation_wait() {
    let args = parse_send_args(&["--confirm-timeout", "7"]);
    assert_eq!(args.confirm_timeout, 7);
    assert_eq!(
        confirm_timeout_from_args(&args),
        std::time::Duration::from_secs(7),
        "override must be the exact Duration execute() hands to wait_for_delivery_confirmation"
    );
    // Overriding the confirmation wait must not touch the --get-output wait.
    assert_eq!(args.timeout, 300);
}

#[test]
fn send_args_timeout_flag_does_not_affect_confirm_timeout() {
    let args = parse_send_args(&["--timeout", "45"]);
    assert_eq!(args.timeout, 45);
    assert_eq!(
        args.confirm_timeout, 90,
        "--timeout bounds the --get-output wait only; it must not override --confirm-timeout"
    );
}

#[test]
fn send_help_documents_logical_actions_and_pi_mapping() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut subcommand = cmd
        .get_subcommands()
        .find(|cmd| cmd.get_name() == "send")
        .expect("send subcommand")
        .clone();
    let help = subcommand.render_long_help().to_string();

    assert!(help.contains("Logical PTY action"), "{help}");
    assert!(help.contains("clear"), "{help}");
    assert!(help.contains("/new"), "{help}");
    assert!(help.contains("/clear"), "{help}");
    assert!(help.contains("exact-stem direct Pi shell"), "{help}");
    assert!(help.contains("Pi compact"), "{help}");
    assert!(help.contains("must be idle"), "{help}");
    assert!(help.contains("terminally rejected before spawn"), "{help}");
    assert!(
        !help.contains("if Exited, respawn. Always delivers"),
        "logical actions must not inherit an unconditional delivery promise: {help}"
    );
}

#[test]
fn send_help_documents_confirm_timeout_semantics() {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut subcommand = cmd
        .get_subcommands()
        .find(|cmd| cmd.get_name() == "send")
        .expect("send subcommand")
        .clone();
    let help = subcommand.render_long_help().to_string();

    assert!(help.contains("--confirm-timeout"), "{help}");
    // Queued-vs-confirmed semantics: timeout does not mean the message was lost.
    assert!(help.contains("durably queued"), "{help}");
    assert!(
        help.contains("does NOT mean the message was lost"),
        "{help}"
    );
    // The two timeout flags must be distinguished from each other.
    assert!(help.contains("--get-output response wait"), "{help}");
}

// ── #1638 --profile: parse, validation, outbox JSON, receipt ──

/// Watch the outbox dir for the queued message file and fabricate the
/// `delivered/` artifact so `execute`'s confirmation wait resolves fast.
fn confirm_delivery_watcher(outbox: std::path::PathBuf) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if let Ok(entries) = std::fs::read_dir(&outbox) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("json") {
                        let id = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or_default()
                            .to_string();
                        let delivered = outbox.join("delivered");
                        let _ = std::fs::create_dir_all(&delivered);
                        let _ = std::fs::write(delivered.join(format!("{}.json", id)), "{}");
                        return;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    })
}

/// Run `execute` end-to-end against the verified-coordinator fixture with
/// the message file in place and a dedicated outbox dir. Returns
/// `(exit code, fixture tempdir, outbox tempdir)`.
fn enqueue_send_fixture(extra: &[&str]) -> (i32, tempfile::TempDir, tempfile::TempDir) {
    enqueue_send_fixture_prepared(extra, |_| {})
}

fn enqueue_send_fixture_prepared(
    extra: &[&str],
    prepare: impl FnOnce(&Path),
) -> (i32, tempfile::TempDir, tempfile::TempDir) {
    use clap::Parser;
    let (temp, _paths) = make_verified_coordinator_fixture();
    let wg_root = temp.path().join("proj-a").join(".ac").join("wg-1-dev-team");
    let messaging = wg_root.join("messaging");
    std::fs::create_dir_all(&messaging).unwrap();
    std::fs::write(
        messaging.join("20260704-000000-wg1-a-to-wg1-b-x.md"),
        "# Hello from fixture\n\nBody.",
    )
    .unwrap();
    prepare(&wg_root);
    let outbox = tempfile::TempDir::new().unwrap();
    let agent_root = wg_root.join("__agent_dev-rust");
    let mut argv = vec![
        "agentscommander",
        "send",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--to",
        "proj-a:wg-1-dev-team/dev-rust",
        "--send",
        "20260704-000000-wg1-a-to-wg1-b-x.md",
        "--root",
        agent_root.to_str().unwrap(),
        "--outbox",
        outbox.path().to_str().unwrap(),
    ];
    argv.extend_from_slice(extra);
    let parsed = crate::cli::Cli::try_parse_from(argv).expect("clap should accept send args");
    let args = match parsed.command.expect("subcommand present") {
        crate::cli::Commands::Send(args) => args,
        _ => panic!("expected Send subcommand"),
    };
    let watcher = confirm_delivery_watcher(outbox.path().to_path_buf());
    let code = execute(args);
    let _ = watcher.join();
    (code, temp, outbox)
}

#[test]
fn issue_3081_cli_archive_failure_keeps_enqueue_result() {
    // Positive control through execute: removing its archive call must fail.
    let (code, temp, _outbox) = enqueue_send_fixture(&[]);
    assert_eq!(code, 0);
    let archived = temp
        .path()
        .join("proj-a/.ac/message-archive/20260704/20260704-000000-wg1-a-to-wg1-b-x.md");
    assert_eq!(
        std::fs::read(archived).unwrap(),
        b"# Hello from fixture\n\nBody."
    );

    let (code, temp, outbox) = enqueue_send_fixture_prepared(&[], |room| {
        std::fs::write(room.parent().unwrap().join("message-archive"), b"blocked").unwrap();
    });
    assert_eq!(code, 0);
    let room = temp.path().join("proj-a/.ac/wg-1-dev-team");
    let source = room.join("messaging/20260704-000000-wg1-a-to-wg1-b-x.md");
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"# Hello from fixture\n\nBody."
    );
    let queued: Vec<_> = std::fs::read_dir(outbox.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .collect();
    assert_eq!(
        queued.len(),
        1,
        "archive failure must leave the message in the outbox"
    );
    let message: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&queued[0]).unwrap()).unwrap();
    assert_eq!(message["to"], "proj-a:wg-1-dev-team/dev-rust");
    let pointer = message["body"]
        .as_str()
        .unwrap()
        .strip_prefix("Process this inter-agent message: ")
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(pointer).unwrap(),
        std::fs::canonicalize(source).unwrap()
    );
}

#[test]
fn issue_3081_cli_uses_sender_project_and_verified_root_destination() {
    let (temp, paths) = make_verified_coordinator_fixture();
    let room = temp.path().join("proj-a/.ac/wg-1-dev-team");
    let replica = room.join("__agent_tech-lead");
    let name = "20261017-000000-wg1-a-to-wg1-b-snapshot.md";
    let source = room.join(name);
    std::fs::write(&source, b"\xff\r\n").unwrap();
    let source = std::fs::canonicalize(source).unwrap();
    let peer = "proj-a:wg-1-dev-team/dev-rust";
    let first = archive_send_file(&replica, false, peer, &paths, &source, name).unwrap();
    let to_root = archive_send_file(
        &replica,
        false,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        &paths,
        &source,
        name,
    )
    .unwrap();
    assert_eq!(first, to_root);
    let global = temp.path().join("global-root");
    std::fs::create_dir_all(&global).unwrap();
    let root_name = "20261017-000000-wg1-a-to-wg1-b-root-snapshot.md";
    let root_source = global.join(root_name);
    std::fs::write(&root_source, b"Root source").unwrap();
    let root_source = std::fs::canonicalize(root_source).unwrap();
    let from_root = archive_send_file(
        &global,
        true,
        "proj-a:wg-1-dev-team/tech-lead",
        &paths,
        &root_source,
        root_name,
    )
    .unwrap();
    assert_eq!(first.parent(), from_root.parent());
    let day = std::fs::canonicalize(room.parent().unwrap())
        .unwrap()
        .join("message-archive/20261017");
    assert_eq!(first, day.join(name));
    assert_eq!(from_root, day.join(root_name));
    assert_eq!(std::fs::read(from_root).unwrap(), b"Root source");
    assert!(first.starts_with(
        std::fs::canonicalize(room.parent().unwrap())
            .unwrap()
            .join("message-archive")
    ));
    assert!(!global.join("message-archive").exists());
    assert!(archive_send_file(
        &global,
        true,
        "unknown:wg-1-team/lead",
        &paths,
        &source,
        name
    )
    .is_err());
    assert_eq!(std::fs::read(first).unwrap(), b"\xff\r\n");
}

/// F5 (step 9): run `execute` with `--outbox` = `<wg_root>/<alias>` so the
/// guard's **call site** is under test; deleting only the function call in
/// `execute` would otherwise keep the old direct-function test green.
/// `prepare` runs against the fixture's WG root before `execute`, so a case
/// can create the intermediate components it wants.
fn execute_send_with_outbox_alias(
    alias: &str,
    prepare: impl FnOnce(&Path),
) -> (i32, tempfile::TempDir) {
    use clap::Parser;
    let (temp, _paths) = make_verified_coordinator_fixture();
    let wg_root = temp.path().join("proj-a").join(".ac").join("wg-1-dev-team");
    let messaging = wg_root.join("messaging");
    std::fs::create_dir_all(&messaging).unwrap();
    std::fs::write(
        messaging.join("20260704-000000-wg1-a-to-wg1-b-x.md"),
        "# Hello from fixture\n\nBody.",
    )
    .unwrap();
    prepare(&wg_root);
    let outbox = wg_root.join(alias);
    let agent_root = wg_root.join("__agent_dev-rust");
    let argv = vec![
        "agentscommander",
        "send",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--to",
        "proj-a:wg-1-dev-team/dev-rust",
        "--send",
        "20260704-000000-wg1-a-to-wg1-b-x.md",
        "--root",
        agent_root.to_str().unwrap(),
        "--outbox",
        outbox.to_str().unwrap(),
    ];
    let parsed = crate::cli::Cli::try_parse_from(argv).expect("clap should accept send args");
    let args = match parsed.command.expect("subcommand present") {
        crate::cli::Commands::Send(args) => args,
        _ => panic!("expected Send subcommand"),
    };
    (execute(args), temp)
}

fn outbox_json(outbox: &tempfile::TempDir) -> serde_json::Value {
    let entries: Vec<_> = std::fs::read_dir(outbox.path())
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one outbox message file");
    serde_json::from_str(&std::fs::read_to_string(entries[0].path()).unwrap()).unwrap()
}

#[test]
fn send_args_profile_flag_accepts_a_single_letter_and_normalizes_case() {
    let args = parse_send_args(&["--profile", "c"]);
    assert_eq!(args.profile.as_deref(), Some("c"));

    // execute-time validation normalizes to uppercase: the outbox JSON
    // carries `requestedProfile` = "C".
    let (code, _temp, outbox) = enqueue_send_fixture(&["--profile", "c"]);
    assert_eq!(code, 0, "enqueue path must succeed");
    let json = outbox_json(&outbox);
    assert_eq!(json["requestedProfile"], "C");
}

#[test]
fn send_rejects_invalid_profile_letter_before_enqueue() {
    use clap::Parser;
    for bad in ["3", "AB", ""] {
        // Build the fixture argv for the bad value. clap may reject the
        // empty string at parse time (still exit 1 pre-enqueue); otherwise
        // execute must reject it before writing anything.
        let (temp, _paths) = make_verified_coordinator_fixture();
        let wg_root = temp.path().join("proj-a").join(".ac").join("wg-1-dev-team");
        let messaging = wg_root.join("messaging");
        std::fs::create_dir_all(&messaging).unwrap();
        std::fs::write(
            messaging.join("20260704-000000-wg1-a-to-wg1-b-x.md"),
            "body",
        )
        .unwrap();
        let outbox = tempfile::TempDir::new().unwrap();
        let agent_root = wg_root.join("__agent_dev-rust");
        let argv = vec![
            "agentscommander",
            "send",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--to",
            "proj-a:wg-1-dev-team/dev-rust",
            "--send",
            "20260704-000000-wg1-a-to-wg1-b-x.md",
            "--root",
            agent_root.to_str().unwrap(),
            "--outbox",
            outbox.path().to_str().unwrap(),
            "--profile",
            bad,
        ];
        let code = match crate::cli::Cli::try_parse_from(argv) {
            Ok(parsed) => match parsed.command.expect("subcommand present") {
                crate::cli::Commands::Send(args) => execute(args),
                _ => panic!("expected Send subcommand"),
            },
            Err(_) => 1, // clap-level rejection: exit non-zero, nothing written
        };
        assert_eq!(code, 1, "--profile {bad:?} must exit 1");
        assert_eq!(
            std::fs::read_dir(outbox.path()).unwrap().count(),
            0,
            "--profile {bad:?} must not write any outbox file"
        );
    }
}

#[test]
fn send_profile_conflicts_with_pty_input() {
    assert!(try_parse_payload_args(&["--profile", "C", "--pty-input", "x"]).is_err());
    assert!(try_parse_payload_args(&["--profile", "C", "--pty-input-stdin"]).is_err());
    // clap rejects before execute: no outbox file can be written.
}

#[test]
fn send_without_flags_writes_no_requested_profile_key() {
    let (code, _temp, outbox) = enqueue_send_fixture(&[]);
    assert_eq!(code, 0, "enqueue path must succeed");
    let json = outbox_json(&outbox);
    let object = json.as_object().unwrap();
    assert!(
        !object.contains_key("requestedProfile"),
        "no-flag send must not write requestedProfile"
    );
    assert_eq!(json["preferredAgent"], "auto");
    // Round-2 Block C: none of the four annotation keys may appear either —
    // the no-flag outbox JSON is byte-identical to today.
    for key in [
        "effectiveAgentId",
        "effectiveProfile",
        "profileFallbackApplied",
        "dispatchNotApplied",
    ] {
        assert!(
            !object.contains_key(key),
            "{key} must be absent without flags"
        );
    }
}

#[test]
fn send_with_agent_and_profile_writes_requested_profile_field() {
    let (code, _temp, outbox) = enqueue_send_fixture(&["--agent", "codex", "--profile", "C"]);
    assert_eq!(code, 0, "enqueue path must succeed");
    let json = outbox_json(&outbox);
    assert_eq!(json["requestedProfile"], "C");
    assert_eq!(json["preferredAgent"], "codex");
    for key in [
        "effectiveAgentId",
        "effectiveProfile",
        "profileFallbackApplied",
        "dispatchNotApplied",
    ] {
        assert!(
            !json.as_object().unwrap().contains_key(key),
            "{key} must stay absent at enqueue time"
        );
    }
}

#[test]
fn queued_receipt_prints_requested_agent_and_profile_when_flags_present() {
    // Flag case: agent + profile.
    assert_eq!(
        queued_receipt_line("m-1", "codex", &Some("C".to_string())),
        "Queued: m-1 (agent=codex, profile=C)"
    );
    // Flag case: profile only — agent stays `auto` on the wire but is
    // still echoed for symmetry with the Delivered: line.
    assert_eq!(
        queued_receipt_line("m-1", "auto", &Some("C".to_string())),
        "Queued: m-1 (agent=auto, profile=C)"
    );
    // No-flag case: exact legacy shape, message id first token.
    assert_eq!(queued_receipt_line("m-1", "auto", &None), "Queued: m-1");
    // Agent-only case (pre-existing --agent semantics) keeps its shape.
    assert_eq!(
        queued_receipt_line("m-1", "codex", &None),
        "Queued: m-1 (agent=codex)"
    );
}

// #2232 phase 7 test 8: the Co-managed provenance queue is not an
// `--outbox` target. The queue is the only tokenless, self-authorized
// origin; allowing a token-holding caller to write there would let it
// forge that origin.
#[test]
fn outbox_guard_rejects_the_co_managed_queue() {
    // Shape-level checks, including F3's `..` aliases in both states.
    let temp = tempfile::TempDir::new().unwrap();
    let room = temp.path().join("room-1-dev-team");
    let queue = crate::config::co_managed::queue_dir(&room);

    let error =
        reject_comanaged_queue_outbox(&queue).expect_err("the queue directory must be rejected");
    assert!(error.contains("provenance queue"), "{error}");

    // An aliased spelling that canonicalizes to the same directory is
    // rejected too; the guard cannot be evaded with `./` or a symlink.
    std::fs::create_dir_all(&queue).unwrap();
    let aliased = room.join(".co-managed").join(".").join("queue");
    assert!(
        reject_comanaged_queue_outbox(&aliased).is_err(),
        "an aliased queue spelling must be rejected"
    );

    // F3: `..` resolved lexically even when the intermediate component
    // does not exist, so a failing `canonicalize` cannot accept the alias.
    let missing_dotdot = room
        .join(".co-managed")
        .join("missing")
        .join("..")
        .join("queue");
    assert!(
        reject_comanaged_queue_outbox(&missing_dotdot).is_err(),
        "a `..` alias with an absent component must be rejected"
    );
    // F3: and with every component present, when canonicalize resolves.
    std::fs::create_dir_all(room.join(".co-managed").join("present")).unwrap();
    let present_dotdot = room
        .join(".co-managed")
        .join("present")
        .join("..")
        .join("queue");
    assert!(
        reject_comanaged_queue_outbox(&present_dotdot).is_err(),
        "a `..` alias with every component present must be rejected"
    );
    // F3: the sibling `.` alias is caught even while the queue is absent
    // (canonicalize cannot resolve a directory that does not exist yet).
    std::fs::remove_dir_all(&queue).unwrap();
    let dot_alias = room.join(".co-managed").join(".").join("queue");
    assert!(
        reject_comanaged_queue_outbox(&dot_alias).is_err(),
        "a `.` alias with the queue absent must be rejected"
    );

    // Ordinary outboxes stay accepted, including one named `queue` that is
    // not under `.co-managed`.
    let ordinary = temp.path().join("project-a").join("outbox");
    reject_comanaged_queue_outbox(&ordinary).expect("an ordinary outbox is fine");
    let unrelated_queue = temp.path().join("queue");
    reject_comanaged_queue_outbox(&unrelated_queue)
        .expect("a directory named queue outside .co-managed is fine");

    // F5: the real `execute` call site must reject too. Deleting only the
    // call (not the function) makes the message land in the queue and
    // fails these assertions.
    let (code, canonical_temp) = execute_send_with_outbox_alias(".co-managed/queue", |_| {});
    assert_eq!(code, 1, "the queue must be rejected through execute");
    let canonical_wg = canonical_temp
        .path()
        .join("proj-a")
        .join(".ac")
        .join("wg-1-dev-team");
    assert!(
        !crate::config::co_managed::queue_dir(&canonical_wg).exists(),
        "a rejected target must not be created"
    );

    let (code, absent_temp) =
        execute_send_with_outbox_alias(".co-managed/missing/../queue", |_| {});
    assert_eq!(
        code, 1,
        "the `..` alias with a missing component must be rejected through execute"
    );
    let absent_wg = absent_temp
        .path()
        .join("proj-a")
        .join(".ac")
        .join("wg-1-dev-team");
    assert!(
        !absent_wg.join(".co-managed").join("missing").exists(),
        "the guard must fire before `create_dir_all` creates the alias path"
    );

    let (code, _present_temp) =
        execute_send_with_outbox_alias(".co-managed/present/../queue", |wg_root| {
            std::fs::create_dir_all(wg_root.join(".co-managed").join("present")).unwrap();
            std::fs::create_dir_all(wg_root.join(".co-managed").join("queue")).unwrap();
        });
    assert_eq!(
        code, 1,
        "the `..` alias with present components must be rejected through execute"
    );
}
