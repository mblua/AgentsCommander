use super::*;

#[test]
fn p05_real_peer_discovery_uses_registered_remote_without_managed_writes() {
    let fixture = ReadonlyFixture::new();
    // A root outside the registered project cannot supply its path by augmentation.
    let root = fixture.config.join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    for verb in ["list-peers", "list-peers-lean"] {
        for caller in [
            root.to_str().unwrap(),
            fixture.origin.as_str(),
            fixture.peer.as_str(),
        ] {
            let peers = fixture.assert_reader(&[verb, "--token", P05_TOKEN, "--root", caller]);
            let coord = peers
                .as_array()
                .unwrap()
                .iter()
                .find(|peer| peer["name"] == "RegisteredRemote:room-01-devs/coord")
                .expect("registered coordinator peer");
            if verb == "list-peers" {
                assert_eq!(
                    std::fs::canonicalize(coord["path"].as_str().unwrap()).unwrap(),
                    std::fs::canonicalize(&fixture.coordinator).unwrap()
                );
            } else {
                assert!(coord.get("path").is_none(), "lean peers omit path");
                for field in ["working", "waitingForInput", "reachable"] {
                    assert!(coord[field].is_boolean(), "lean {field}: {coord}");
                }
                assert!(coord["sessionStatus"].is_string());
                assert!(coord["teams"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|team| team.is_string()));
            }
        }
    }
}

#[test]
fn p05_real_send_enqueues_and_close_resolves_before_coordinator_denial() {
    let fixture = ReadonlyFixture::new();
    let before = fixture.managed_bytes();
    let out = run_failure(
        &fixture.bin,
        &[
            "send",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--to",
            "RegisteredRemote:room-01-devs/coord",
            "--send",
            P05_MESSAGE,
            "--mode",
            "wake",
            "--confirm-timeout",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let receipt = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Queued: "))
        .expect("durable enqueue receipt");
    let id = receipt.split_whitespace().next().unwrap();
    uuid::Uuid::parse_str(id).unwrap();
    assert_eq!(receipt, id, "exact default enqueue receipt");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("delivery confirmation timeout after 0s"));
    let outbox = fixture.outbox_for_root(&fixture.peer);
    let queued_path = outbox.join(format!("{id}.json"));
    let queued: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&queued_path).unwrap()).unwrap();
    assert_eq!(queued["to"], "RegisteredRemote:room-01-devs/coord");
    assert_eq!(queued["from"], "RegisteredRemote:room-01-devs/peer");
    assert_eq!(queued["mode"], "wake");
    let message_path = fixture
        .project
        .join(".ac/room-01-devs/messaging")
        .join(P05_MESSAGE);
    let canonical_message = std::fs::canonicalize(&message_path).unwrap();
    let canonical_message = canonical_message.to_string_lossy();
    assert_eq!(
        queued["body"],
        format!(
            "Process this inter-agent message: {}",
            canonical_message.trim_start_matches(r"\\?\")
        )
    );
    assert_eq!(
        std::fs::read_to_string(&message_path).unwrap(),
        "P05 message"
    );
    assert_eq!(fixture.managed_bytes(), before);
    std::fs::remove_file(queued_path).unwrap();
    fixture.assert_denied(
        &[
            "close-session",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--target",
            "RegisteredRemote:room-01-devs/coord",
        ],
        "authorization denied",
    );
}

#[test]
fn p05_real_task_consumers_preserve_coordinator_and_denial_contracts() {
    let fixture = ReadonlyFixture::new();
    let before = fixture.managed_bytes();
    for (verb, flag, text) in [
        ("task-set-title", "--title", "P05"),
        ("task-append-body", "--text", "First"),
        ("task-set-body", "--text", "Replacement"),
    ] {
        run_success(
            &fixture.bin,
            &[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.coordinator,
                flag,
                text,
            ],
        );
        assert_eq!(fixture.managed_bytes(), before, "{verb}");
        let task = fixture.project.join(".ac/room-01-devs/TASK.md");
        let task_before = std::fs::read(&task).unwrap();
        fixture.assert_denied(
            &[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.peer,
                flag,
                "Denied",
            ],
            "authorization denied",
        );
        assert_eq!(std::fs::read(task).unwrap(), task_before);
    }
    let snapshot = fixture.assert_reader(&[
        "task-get",
        "--token",
        P05_TOKEN,
        "--root",
        &fixture.coordinator,
    ]);
    let revision = snapshot["revision"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let status = fixture.assert_reader(&[
        "task-status-set",
        "--token",
        P05_TOKEN,
        "--root",
        &fixture.coordinator,
        "--expected-revision",
        revision,
        "--request-id",
        &request,
        "--text",
        "P05 pending",
    ]);
    assert_eq!(status["status"], "P05 pending");
    for verb in ["task-get", "task-status-set"] {
        let mut args = vec![verb, "--token", P05_TOKEN, "--root", &fixture.peer];
        if verb == "task-status-set" {
            args.extend([
                "--expected-revision",
                revision,
                "--request-id",
                &request,
                "--text",
                "Denied",
            ]);
        }
        fixture.assert_denied(&args, "authorization_denied");
    }
    let after = fixture.assert_reader(&[
        "task-get",
        "--token",
        P05_TOKEN,
        "--root",
        &fixture.coordinator,
    ]);
    assert_eq!(after["status"], "P05 pending");
}

#[test]
fn p05_malformed_source_refuses_direct_and_task_consumers_before_outputs() {
    let fixture = ReadonlyFixture::new();
    std::fs::write(
        fixture.config.join("settings.30.instance.no-git.json"),
        b"{malformed",
    )
    .unwrap();
    for verb in ["list-peers", "list-peers-lean"] {
        fixture.assert_denied(
            &[verb, "--token", P05_TOKEN, "--root", &fixture.peer],
            "could not be parsed",
        );
    }
    fixture.assert_denied(
        &[
            "send",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--to",
            "RegisteredRemote:room-01-devs/coord",
            "--send",
            P05_MESSAGE,
            "--confirm-timeout",
            "0",
        ],
        "could not be parsed",
    );
    fixture.assert_denied(
        &[
            "close-session",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--target",
            "RegisteredRemote:room-01-devs/coord",
        ],
        "could not be parsed",
    );
    for (verb, flag) in [
        ("task-set-title", "--title"),
        ("task-append-body", "--text"),
        ("task-set-body", "--text"),
    ] {
        fixture.assert_denied(
            &[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.coordinator,
                flag,
                "Denied",
            ],
            "authorization denied",
        );
    }
    fixture.assert_denied(
        &[
            "task-get",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.coordinator,
        ],
        "authorization_denied",
    );
    fixture.assert_denied(
        &[
            "task-status-set",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.coordinator,
            "--expected-revision",
            "legacy:0",
            "--request-id",
            P05_TOKEN,
            "--text",
            "Denied",
        ],
        "authorization_denied",
    );
    assert!(!fixture.project.join(".ac/room-01-devs/TASK.md").exists());
    assert!(!fixture
        .project
        .join(".ac/room-01-devs/TASK-status.jsonl")
        .exists());
}

#[test]
fn p05_token_only_reads_preserve_root_master_and_uuid_behavior() {
    let fixture = ReadonlyFixture::new();
    for token in ["", "malformed-token"] {
        fixture.assert_denied(
            &["list-peers-lean", "--token", token, "--root", &fixture.peer],
            if token.is_empty() {
                "--token is required"
            } else {
                "invalid token supplied"
            },
        );
    }
    fixture.assert_denied(
        &["list-peers-lean", "--root", &fixture.peer],
        "--token is required",
    );
    // UUID is shape-valid but not privileged: the team gate still denies this peer.
    fixture.assert_denied(
        &[
            "task-set-title",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
            "--title",
            "Denied",
        ],
        "authorization denied",
    );
    let path = fixture.config.join("settings.30.instance.no-git.json");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    settings["rootToken"] = "persisted-root".into();
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
    let before = fixture.managed_bytes();
    run_success(
        &fixture.bin,
        &[
            "task-set-title",
            "--token",
            "persisted-root",
            "--root",
            &fixture.peer,
            "--title",
            "Root bypass",
        ],
    );
    assert_eq!(fixture.managed_bytes(), before);
    settings.as_object_mut().unwrap().remove("rootToken");
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
    std::fs::write(
        fixture.config.join("master-token.txt"),
        "persisted-master\n",
    )
    .unwrap();
    let before = fixture.managed_bytes();
    run_success(
        &fixture.bin,
        &[
            "task-set-title",
            "--token",
            "persisted-master",
            "--root",
            &fixture.peer,
            "--title",
            "Master bypass",
        ],
    );
    assert_eq!(fixture.managed_bytes(), before);
}

#[test]
fn p05_missing_and_valid_empty_settings_remain_readonly() {
    let fixture = ReadonlyFixture::new();
    let root = fixture.config.join("ac-root-agent");
    std::fs::create_dir_all(&root).unwrap();
    let path = fixture.config.join("settings.30.instance.no-git.json");
    for absent in [true, false] {
        if absent {
            std::fs::remove_file(&path).unwrap();
        } else {
            write_settings(&fixture.config, &[]);
        }
        let peers = fixture.assert_reader(&[
            "list-peers-lean",
            "--token",
            P05_TOKEN,
            "--root",
            root.to_str().unwrap(),
        ]);
        assert_eq!(peers, serde_json::json!([]));
        // WG still reports its own peer from the caller's room after a valid read.
        let peers = fixture.assert_reader(&[
            "list-peers-lean",
            "--token",
            P05_TOKEN,
            "--root",
            &fixture.peer,
        ]);
        assert!(peers
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "RegisteredRemote:room-01-devs/coord"));
    }
}

#[test]
fn p05_root_send_resolves_remote_registration_without_own_project_fallback() {
    let fixture = ReadonlyFixture::new();
    let root = fixture.config.join("ac-root-agent");
    std::fs::create_dir_all(root.join("messaging")).unwrap();
    std::fs::write(root.join("messaging").join(P05_MESSAGE), "Remote P05").unwrap();
    let before = fixture.managed_bytes();
    let out = run_failure(
        &fixture.bin,
        &[
            "send",
            "--token",
            P05_TOKEN,
            "--root",
            root.to_str().unwrap(),
            "--to",
            "RegisteredRemote:room-01-devs/coord",
            "--send",
            P05_MESSAGE,
            "--mode",
            "wake",
            "--confirm-timeout",
            "0",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Queued: "))
        .expect("exact enqueue receipt");
    uuid::Uuid::parse_str(id).unwrap();
    let queued: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .outbox_for_root(root.to_str().unwrap())
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(queued["to"], "RegisteredRemote:room-01-devs/coord");
    assert_eq!(queued["from"], "agentscommander://root-agent");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("delivery confirmation timeout after 0s"));
    assert_eq!(fixture.managed_bytes(), before);
    // Close resolves this same remote target before denying non-coordinator origin.
    let outsider = fixture._tmp.path().join("Unregistered/.ac/_agent_outsider");
    std::fs::create_dir_all(&outsider).unwrap();
    fixture.assert_denied(
        &[
            "close-session",
            "--token",
            P05_TOKEN,
            "--root",
            outsider.to_str().unwrap(),
            "--target",
            "RegisteredRemote:room-01-devs/coord",
        ],
        "authorization denied",
    );
}
