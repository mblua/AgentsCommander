use agentscommander_lib::config::sessions_persistence::load_sessions_raw_from_dir_for_test;

use super::{
    assert_no_side_effect_dirs, assert_outside_sentinel_survives, assert_refresh_request,
    assert_same_path, config_dir_for_bin, copy_binary_into, create_basic_workgroup,
    create_dirty_repo, create_outside_sentinel, find_refresh_request, git_available,
    has_refresh_reason, project_refresh_request_paths, project_with_agents,
    read_project_refresh_requests, run_fail, run_json, run_json_machine, run_stdout,
    write_live_session, write_settings, SessionStatus, Tmp,
};
#[cfg(target_os = "windows")]
use super::{create_junction, open_without_delete_share, run_fail_output};

#[test]
fn team_help_describes_existing_agents_and_workgroup_scoped_membership() {
    let tmp = Tmp::new("cli-team-help");
    let bin = copy_binary_into(tmp.path());

    let team_help = run_stdout(&bin, &["team", "--help"]);
    assert!(team_help.contains("Manage teams and scoped room membership"));

    let create_help = run_stdout(&bin, &["team", "create", "--help"]);
    assert!(create_help.contains("Create a team configuration from existing agents"));
    assert!(create_help.contains("Existing agent matrix name or _agent_<name> reference"));
    assert!(create_help.contains("Define a repo available to the team when rooms are created"));
}

#[test]
fn workgroup_add_help_hides_team_definition_flags() {
    let tmp = Tmp::new("cli-workgroup-help");
    let bin = copy_binary_into(tmp.path());

    let help = run_stdout(&bin, &["workgroup", "add", "--help"]);
    assert!(help.contains("--project"));
    assert!(help.contains("--team"));
    assert!(help.contains("--title"));
    assert!(!help.contains("--coordinator"));
    assert!(!help.contains("--agent"));
    assert!(!help.contains("--repo"));
    assert!(!help.to_ascii_lowercase().contains("deprecated"));
}

#[test]
fn workgroup_list_missing_project_errors_without_writes() {
    let tmp = Tmp::new("cli-workgroup-list-missing-project");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let stderr = run_fail(&bin, &["workgroup", "list", "--project", "MissingProject"]);

    assert!(stderr.contains("MissingProject"), "stderr was:\n{}", stderr);
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_side_effect_dirs(tmp.path());
}

#[test]
fn team_list_missing_project_errors_without_writes() {
    let tmp = Tmp::new("cli-team-list-missing-project");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let stderr = run_fail(&bin, &["team", "list", "--project", "MissingProject"]);

    assert!(stderr.contains("MissingProject"), "stderr was:\n{}", stderr);
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_side_effect_dirs(tmp.path());
}

#[test]
fn workgroup_add_creates_task_messaging_replicas_and_lists() {
    let tmp = Tmp::new("cli-workgroup-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    let created_team = run_json(
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
    assert_eq!(created_team["team"], "dev-team");
    assert_eq!(
        created_team["agents"],
        serde_json::json!(["_agent_architect", "_agent_dev-rust"])
    );
    assert_eq!(
        created_team["contextAlertPercentages"],
        serde_json::json!([])
    );

    let team_config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let team_config_before = std::fs::read(&team_config_path).expect("team config before");

    let json = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build the thing",
        ],
    );

    let wg_dir = project.join(".ac").join("room-1-dev-team");
    assert_same_path(json["path"].as_str().expect("path"), &wg_dir);
    assert!(wg_dir.join("TASK.md").is_file());
    assert!(wg_dir.join("messaging").is_dir());
    assert!(wg_dir
        .join("__agent_architect")
        .join("config.json")
        .is_file());
    assert!(wg_dir
        .join("__agent_dev-rust")
        .join("config.json")
        .is_file());
    assert_eq!(
        std::fs::read(&team_config_path).expect("team config after"),
        team_config_before
    );
    let team_config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(team_config_path).expect("config"))
            .expect("team config json");
    assert_eq!(
        team_config["agents"],
        serde_json::json!(["_agent_architect", "_agent_dev-rust"])
    );
    assert_eq!(team_config["coordinator"], "_agent_architect");
    assert_eq!(
        team_config["contextAlertPercentages"],
        serde_json::json!([])
    );

    let list = run_json(&bin, &["workgroup", "list", "--project", "ProjectAlpha"]);
    assert_eq!(list.as_array().expect("array").len(), 1);
    assert_eq!(list[0]["name"], "room-1-dev-team");
    assert_eq!(list[0]["team"], "dev-team");
    assert_eq!(list[0]["hasTask"], true);
    assert_eq!(list[0]["hasMessaging"], true);

    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 2);
    assert_refresh_request(
        find_refresh_request(&requests, "teamCreated"),
        &project,
        ".ac/_team_dev-team",
        "dev-team",
        "teamCreated",
    );
    assert_refresh_request(
        find_refresh_request(&requests, "workgroupCreated"),
        &project,
        ".ac/room-1-dev-team",
        "room-1-dev-team",
        "workgroupCreated",
    );
}

#[test]
fn workgroup_add_uses_global_lowest_free_number() {
    let tmp = Tmp::new("cli-workgroup-numbering");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);
    std::fs::create_dir_all(project.join(".ac").join("room-1-dev-team")).expect("wg1");

    let _team = run_json(
        &bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "QA Team",
            "--coordinator",
            "architect",
        ],
    );
    let json = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "QA Team",
            "--title",
            "Test it",
        ],
    );

    assert!(json["path"]
        .as_str()
        .expect("path")
        .ends_with("room-2-qa-team"));
}

#[test]
fn workgroup_remove_deletes_and_reuses_number() {
    let tmp = Tmp::new("cli-workgroup-remove");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);

    let created = run_json(
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
            "--coordinator",
            "architect",
        ],
    );
    let wg_dir = project.join(".ac").join("room-1-dev-team");
    assert_same_path(created["path"].as_str().expect("path"), &wg_dir);
    assert!(wg_dir.is_dir());

    let removed = run_json_machine(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!wg_dir.exists());
    let requests = read_project_refresh_requests(&config_dir);
    assert_eq!(requests.len(), 2);
    assert_refresh_request(
        find_refresh_request(&requests, "workgroupRemoved"),
        &project,
        ".ac/room-1-dev-team",
        "room-1-dev-team",
        "workgroupRemoved",
    );

    let recreated = run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "QA Team",
            "--title",
            "Test it",
            "--coordinator",
            "architect",
        ],
    );
    assert!(recreated["path"]
        .as_str()
        .expect("path")
        .ends_with("room-1-qa-team"));
}

#[test]
fn workgroup_remove_refuses_dirty_repo_without_force() {
    if !git_available() {
        println!("skipping dirty repo workgroup regression because git is unavailable");
        return;
    }

    let tmp = Tmp::new("cli-workgroup-dirty-refuse");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "dirty-refuse");
    let dirty_file = wg_dir.join("repo-dirty").join("untracked.txt");
    create_dirty_repo(dirty_file.parent().expect("dirty repo parent"));

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
        ],
    );
    assert!(stderr.contains("pending work"));
    assert!(stderr.contains("repo-dirty"));
    assert!(
        wg_dir.is_dir(),
        "dirty refusal should leave workgroup intact"
    );
    assert!(dirty_file.is_file(), "dirty repo file should remain");
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_remove_force_dirty_deletes_dirty_repo() {
    if !git_available() {
        println!("skipping force dirty workgroup regression because git is unavailable");
        return;
    }

    let tmp = Tmp::new("cli-workgroup-dirty-force");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "dirty-force");
    create_dirty_repo(&wg_dir.join("repo-dirty"));

    let removed = run_json_machine(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--force-dirty",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!wg_dir.exists(), "force-dirty should delete workgroup");
    assert_outside_sentinel_survives(&outside);
    assert!(has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_remove_refuses_live_persisted_session_even_with_force() {
    let tmp = Tmp::new("cli-workgroup-live-refuse");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "live-refuse");
    let session_cwd = wg_dir.join("__agent_architect");
    write_live_session(&config_dir, &session_cwd);

    let parsed = load_sessions_raw_from_dir_for_test(&config_dir);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].status, Some(SessionStatus::Running));
    assert!(parsed[0].id.is_some(), "live session fixture must carry id");

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--force-dirty",
        ],
    );
    assert!(stderr.contains("Cannot delete while live sessions exist"));
    assert!(stderr.contains("Live Rust"));
    assert!(
        wg_dir.is_dir(),
        "live session refusal should leave workgroup intact"
    );
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[cfg(target_os = "windows")]
#[test]
fn workgroup_remove_locked_file_reports_blockers_without_refresh() {
    let tmp = Tmp::new("cli-workgroup-locked");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "locked");
    let locked = wg_dir.join("locked.bin");
    std::fs::write(&locked, b"locked").expect("write locked file");
    let _handle = open_without_delete_share(&locked);

    let (_stdout, stderr) = run_fail_output(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--force-dirty",
        ],
    );
    assert!(stderr.contains("file in use") || stderr.contains("Failed to delete workgroup"));
    assert!(wg_dir.is_dir(), "blocked workgroup should remain");
    assert!(locked.is_file(), "locked file should remain");
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_remove_deletes_long_path_tree() {
    let tmp = Tmp::new("cli-workgroup-long-path");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let mut deep = wg_dir.clone();
    for idx in 0..10 {
        deep = deep.join(format!("long-segment-{idx:02}-abcdef"));
    }
    if let Err(e) = std::fs::create_dir_all(&deep) {
        println!(
            concat!(
                "skipping long path workgroup regression; see ",
                "docs/testing/destructive-filesystem-regression.md",
                "#workgroup-long-path-check: {}"
            ),
            e
        );
        return;
    }
    std::fs::write(deep.join("payload.txt"), "payload").expect("write payload");
    let keep = wg_dir.parent().expect("wg parent").join("keep-sibling");
    std::fs::create_dir_all(&keep).expect("create keep sibling");

    let removed = run_json_machine(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--force-dirty",
        ],
    );
    assert_eq!(removed["removed"], true);
    assert!(!wg_dir.exists(), "workgroup should be deleted");
    assert!(keep.is_dir(), "sibling outside workgroup should remain");
}

#[cfg(target_os = "windows")]
#[test]
fn workgroup_remove_refuses_reparse_root() {
    let tmp = Tmp::new("cli-workgroup-reparse-root");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let outside = create_outside_sentinel(&project, "reparse-root");
    std::fs::remove_dir_all(&wg_dir).expect("remove real workgroup");
    let real = tmp.path().join("real-wg-target");
    std::fs::create_dir_all(&real).expect("create real target");
    std::fs::write(real.join("sentinel.txt"), "target").expect("write sentinel");
    if let Err(e) = create_junction(&wg_dir, &real) {
        println!(
            concat!(
                "skipping reparse root workgroup regression; see ",
                "docs/testing/destructive-filesystem-regression.md",
                "#workgroup-reparse-root-check: {}"
            ),
            e
        );
        return;
    }

    let stderr = run_fail(
        &bin,
        &[
            "workgroup",
            "remove",
            "--project",
            "ProjectAlpha",
            "--workgroup",
            "room-1-dev-team",
            "--force-dirty",
        ],
    );
    assert!(stderr.contains("delete_root_is_reparse_point"));
    assert!(wg_dir.exists(), "junction should remain");
    assert!(real.join("sentinel.txt").is_file(), "target should remain");
    assert_outside_sentinel_survives(&outside);
    assert!(!has_refresh_reason(&config_dir, "workgroupRemoved"));
}

#[test]
fn workgroup_add_normalizes_include_and_exclude_repo_assignments() {
    let tmp = Tmp::new("cli-workgroup-repos");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust", "qa"]);

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
            "--agent",
            "qa",
            "--repo",
            "https://example.test/all.git",
            "--repo-agents",
            "https://example.test/include.git=architect,dev-rust",
            "--repo-exclude-agents",
            "https://example.test/exclude.git=qa",
        ],
    );
    let _json = run_json(
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

    let config_path = project
        .join(".ac")
        .join("_team_dev-team")
        .join("config.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("config"))
            .expect("config json");
    let repos = config["repos"].as_array().expect("repos");
    assert_eq!(repos.len(), 3);
    assert_eq!(repos[0]["agents"].as_array().expect("agents").len(), 3);
    assert_eq!(
        repos[1]["agents"],
        serde_json::json!(["_agent_architect", "_agent_dev-rust"])
    );
    let exclude_agents = repos[2]["agents"].as_array().expect("exclude agents");
    assert_eq!(exclude_agents.len(), 2);
    assert!(exclude_agents
        .iter()
        .any(|agent| agent == "_agent_architect"));
    assert!(exclude_agents
        .iter()
        .any(|agent| agent == "_agent_dev-rust"));
}
