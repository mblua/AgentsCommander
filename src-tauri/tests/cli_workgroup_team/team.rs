use super::{
    command_for_binary, config_dir_for_bin, copy_binary_into, project_with_agents, run_fail,
    run_json, spawn_lock, write_settings, Stdio, Tmp,
};

#[test]
fn team_create_refuses_existing_team() {
    let tmp = Tmp::new("cli-team-create-existing");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let _project = project_with_agents(tmp.path(), &["architect"]);

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
    let stderr = run_fail(
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
    assert!(stderr.contains("Team 'dev-team' already exists"));
}

#[test]
fn team_create_refuses_preexisting_empty_team_dir() {
    let tmp = Tmp::new("cli-team-create-empty-dir");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);
    std::fs::create_dir_all(project.join(".ac").join("_team_dev-team")).expect("team dir");

    let stderr = run_fail(
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
    assert!(stderr.contains("Team 'dev-team' already exists"));
}

#[test]
fn team_create_outputs_coordinator_first_roster() {
    let tmp = Tmp::new("cli-team-create-roster");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust", "qa"]);

    let json = run_json(
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
            "--agent",
            "dev-rust",
            "--agent",
            "architect",
            "--agent",
            "qa",
        ],
    );
    let expected = serde_json::json!(["_agent_architect", "_agent_dev-rust", "_agent_qa"]);
    assert_eq!(json["agents"], expected);

    let config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("config"))
            .expect("config json");
    assert_eq!(config["agents"], expected);
}

#[test]
fn workgroup_add_existing_team_refuses_legacy_flags_without_rewrite() {
    let tmp = Tmp::new("cli-wg-existing-refuses-legacy");
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
            "--agent",
            "dev-rust",
        ],
    );
    let config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let before = std::fs::read(&config_path).expect("before");
    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Second",
            "--coordinator",
            "dev-rust",
        ],
    );
    assert!(stderr.contains("room add` no longer updates team configuration"));
    assert_eq!(std::fs::read(&config_path).expect("after"), before);
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("config"))
            .expect("config json");
    assert_eq!(config["coordinator"], "_agent_architect");
}

#[test]
fn workgroup_add_existing_invalid_team_config_errors_without_rewrite() {
    let tmp = Tmp::new("cli-wg-invalid-team");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);
    let team_dir = project.join(".ac").join("_team_dev-team");
    std::fs::create_dir_all(&team_dir).expect("team dir");
    let config_path = team_dir.join("config.json");
    let original = b"{ not json".to_vec();
    std::fs::write(&config_path, &original).expect("write invalid config");

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Second",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ],
    );
    assert!(stderr.contains("Team config at"));
    assert!(stderr.contains("is malformed"));
    assert_eq!(std::fs::read(&config_path).expect("after"), original);
}

#[test]
fn workgroup_add_legacy_missing_team_still_bootstraps_with_warning() {
    let tmp = Tmp::new("cli-wg-legacy-bootstrap");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let mut command = command_for_binary(&bin);
    command
        .args([
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("created missing team configuration"));
    assert!(!stderr.to_ascii_lowercase().contains("deprecated"));
    assert!(project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json")
        .is_file());
}

#[test]
fn workgroup_add_missing_team_without_legacy_flags_errors() {
    let tmp = Tmp::new("cli-wg-missing-team");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let _project = project_with_agents(tmp.path(), &["architect"]);

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Missing Team",
            "--title",
            "Build",
        ],
    );
    assert!(stderr.contains("Team 'missing-team' config not found"));
    assert!(stderr.contains("Create it first with `team create`"));
}

// #1065 Stage F: these projects never spawn a session, so config seed never runs
// and no config scope is recorded. CLI removal therefore prunes nothing and, by the
// empty-transaction rule, never spuriously creates a seed manifest. The
// `workgroup_remove_prunes_recorded_config_scope` test below proves the prune fires
// when a scope IS recorded. Black-box: public outcomes only, no token or hook.
