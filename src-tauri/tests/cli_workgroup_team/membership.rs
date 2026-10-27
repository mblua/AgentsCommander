use super::{
    assert_refresh_request, command_for_binary, config_dir_for_bin, copy_binary_into,
    find_refresh_request, project_with_agents, read_project_refresh_requests, run_json, spawn_lock,
    write_settings, Path, PathBuf, PersistedSession, SessionStatus, Stdio, Tmp,
};

#[test]
fn team_add_member_creates_replica_and_peer_is_reachable() {
    let tmp = Tmp::new("cli-team-add-member");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    let _wg = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );

    let added = run_json(
        &bin,
        &[
            "team",
            "add-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--agent",
            "dev-rust",
        ],
    );
    let replica = project
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_dev-rust");
    assert_eq!(added["added"], true);
    assert!(replica.join("config.json").is_file());

    let sender_root = project
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_architect");
    let mut command = command_for_binary(&bin);
    command
        .args([
            "list-peers-lean",
            "--root",
            &sender_root.to_string_lossy(),
            "--token",
            "00000000-0000-0000-0000-000000000000",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("list peers")
    };
    let out = child.wait_with_output().expect("collect output");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let peers: serde_json::Value = serde_json::from_slice(&out.stdout).expect("peers");
    let found = peers.as_array().expect("array").iter().any(|peer| {
        peer["name"] == "ProjectAlpha:room-1-dev-team/dev-rust" && peer["reachable"] == true
    });
    assert!(found, "added peer should be reachable: {}", peers);
    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 3);
    assert_refresh_request(
        find_refresh_request(&requests, "teamMembershipChanged"),
        &project,
        ".ac/room-1-dev-team/__agent_dev-rust",
        "room-1-dev-team/dev-rust",
        "teamMembershipChanged",
    );

    let removed = run_json(
        &bin,
        &[
            "team",
            "remove-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--agent",
            "dev-rust",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!replica.exists());
    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 4);
    assert_refresh_request(
        find_refresh_request(&requests, "teamMembershipRemoved"),
        &project,
        ".ac/room-1-dev-team/__agent_dev-rust",
        "room-1-dev-team/dev-rust",
        "teamMembershipRemoved",
    );
}

/// #1939 - write through the same #1938 guarded funnel production uses.
fn guarded_update(
    config_path: &Path,
    mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), String>,
) {
    agentscommander_lib::config::local_config_io::update_config_json_object(
        config_path,
        false,
        mutate,
    )
    .expect("guarded config update");
}

/// #1939 - a repeated `team add-member` must merge into the existing replica
/// config instead of replacing it: the protected pair, its flag, tooling
/// history, custom context and unknown keys survive, including a config write
/// that runs through the same guarded writer while the repeat is in flight.
#[test]
fn issue_1937_repeat_member_preserves_config() {
    let tmp = Tmp::new("cli-issue-1937-repeat-member");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    let _wg = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );
    let first = run_json(
        &bin,
        &[
            "team",
            "add-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--agent",
            "dev-rust",
        ],
    );
    assert_eq!(first["added"], true);

    let replica_dir = project
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_dev-rust");
    let config_path = replica_dir.join("config.json");

    guarded_update(&config_path, |obj| {
        obj.insert(
            "tooling".to_string(),
            serde_json::json!({
                "currentCodingAgent": "codex",
                "profile": "B",
                "instanceProfileOverride": "B",
                "instanceProfileOverrideSource": "manual",
                "selectionLocked": true,
                "lastCodingAgent": "claude"
            }),
        );
        obj.insert("customToolingKey".to_string(), serde_json::json!("keep-me"));
        obj.insert(
            "context".to_string(),
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "custom-notes.md"]),
        );
        Ok(())
    });

    // Repeat the member add while this test process performs guarded writes
    // concurrently: the cross-process sidecar lock serializes them, and the
    // creator's merge must not drop either writer's data.
    let mut command = command_for_binary(&bin);
    command
        .args([
            "team",
            "add-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--agent",
            "dev-rust",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn repeat add-member")
    };
    for index in 0..5 {
        guarded_update(&config_path, |obj| {
            obj.insert(
                "concurrentGuardMarker".to_string(),
                serde_json::json!(index),
            );
            Ok(())
        });
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let output = child.wait_with_output().expect("wait repeat add-member");
    assert!(
        output.status.success(),
        "repeat add-member failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read replica config"))
            .expect("parse replica config");
    assert_eq!(saved["identity"], "../../_agent_dev-rust");
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    assert_eq!(saved["tooling"]["instanceProfileOverride"], "B");
    assert_eq!(saved["tooling"]["selectionLocked"], true);
    // #2816 (C3) - the repeat add-member migrates the state key through site 6.
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(replica_dir.join("config.state.no-git.json"))
            .expect("read replica state file"),
    )
    .expect("parse replica state file");
    assert_eq!(state["tooling"]["lastCodingAgent"], "claude");
    assert!(saved["tooling"].get("lastCodingAgent").is_none());
    assert_eq!(saved["customToolingKey"], "keep-me");
    assert_eq!(saved["concurrentGuardMarker"], 4);
    let context: Vec<&str> = saved["context"]
        .as_array()
        .expect("context array")
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(context.contains(&"custom-notes.md"), "{context:?}");
    assert!(
        context.iter().any(|entry| entry.ends_with("/Role.md")),
        "{context:?}"
    );

    // #2816 (C3) E9c - the migration's journal note is under the directory this
    // fixture's own `AGENTSCOMMANDER_CONFIG_DIR` names, and nowhere else in the
    // fixture's tree.
    const JOURNAL_NAME: &str = "naming-migration.state.no-git.json";
    fn journals_under(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read fixture dir") {
            let path = entry.expect("fixture entry").path();
            if path.is_dir() {
                journals_under(&path, found);
            } else if path.file_name().is_some_and(|name| name == JOURNAL_NAME) {
                found.push(path);
            }
        }
    }
    let mut journals = Vec::new();
    journals_under(tmp.path(), &mut journals);
    assert_eq!(journals, vec![config_dir.join(JOURNAL_NAME)]);

    // The file alone proves nothing: every CLI start journals the instance
    // families there. The C3 record is this replica's own scope and its note.
    let journal: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config_dir.join(JOURNAL_NAME)).expect("read journal"),
    )
    .expect("parse journal");
    let scope = format!(
        "config-state:{}",
        std::fs::canonicalize(&config_path)
            .expect("canonical replica config")
            .display()
    );
    let notes = journal["scopes"][&scope]["notes"]
        .as_array()
        .unwrap_or_else(|| panic!("no `{scope}` scope with notes in {journal}"));
    assert!(
        notes
            .iter()
            .any(|note| note == "moved to the state file: lastCodingAgent"),
        "{notes:?}"
    );
}

// #1088: a live session whose name+cwd match a WG peer surfaces its context-usage
// percent as `contextPercent` in both `list-peers` and `list-peers-lean` (read from
// sessions.json on disk, no daemon), and peers without a matching reading omit it.
#[test]
fn list_peers_surfaces_context_percent_for_matching_live_session() {
    let tmp = Tmp::new("cli-peer-context-percent");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
        ],
    );
    let _wg = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );
    let _added = run_json(
        &bin,
        &[
            "team",
            "add-member",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--agent",
            "dev-rust",
        ],
    );

    let replica = project
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_dev-rust");
    assert!(replica.join("config.json").is_file());

    // A live session whose name (`<wg>/<agent>`) and cwd match the dev-rust WG peer,
    // carrying a context reading. `..PersistedSession::default()` leaves every other
    // field (incl. legacy) at its default; only context_percent is set here.
    let sessions = vec![PersistedSession {
        name: "room-1-dev-team/dev-rust".to_string(),
        shell: "codex".to_string(),
        shell_args: Vec::new(),
        working_directory: replica.to_string_lossy().to_string(),
        id: Some(uuid::Uuid::new_v4().to_string()),
        status: Some(SessionStatus::Running),
        waiting_for_input: Some(false),
        created_at: Some("2026-07-21T00:00:00Z".to_string()),
        context_percent: Some(63),
        ..PersistedSession::default()
    }];
    std::fs::write(
        config_dir.join("sessions.json"),
        serde_json::to_string_pretty(&sessions).expect("sessions json"),
    )
    .expect("write sessions");

    let sender_root = project
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_architect");

    for verb in ["list-peers", "list-peers-lean"] {
        let mut command = command_for_binary(&bin);
        command
            .args([
                verb,
                "--root",
                &sender_root.to_string_lossy(),
                "--token",
                "00000000-0000-0000-0000-000000000000",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = {
            let _guard = spawn_lock();
            command.spawn().expect("run peer verb")
        };
        let out = child.wait_with_output().expect("collect output");
        assert!(
            out.status.success(),
            "{verb} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let peers: serde_json::Value = serde_json::from_slice(&out.stdout).expect("peers json");
        let mut saw_dev = false;
        for peer in peers.as_array().expect("array") {
            if peer["name"] == "ProjectAlpha:room-1-dev-team/dev-rust" {
                saw_dev = true;
                assert_eq!(
                    peer["contextPercent"], 63,
                    "{verb}: matching live session must surface contextPercent: {peer}"
                );
            } else {
                assert!(
                    peer.get("contextPercent").is_none(),
                    "{verb}: peer {} without a matching reading must omit contextPercent: {peer}",
                    peer["name"]
                );
            }
        }
        assert!(
            saw_dev,
            "{verb}: dev-rust peer missing from output: {peers}"
        );
    }
}
