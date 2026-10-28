use super::{
    config_dir_for_bin, copy_binary_into, create_basic_workgroup, project_with_agents, run_json,
    run_json_machine, write_settings, Path, Tmp,
};

#[test]
fn workgroup_remove_does_not_create_a_manifest_when_no_scope_was_recorded() {
    let tmp = Tmp::new("cli-workgroup-remove-dormant");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);

    run_json(
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
    assert!(
        !project.join(".ac").join("seed-manifest.toml").exists(),
        "removing a workgroup with no recorded config scope must not create a seed manifest"
    );
}

#[test]
fn team_remove_member_does_not_create_a_manifest_when_no_scope_was_recorded() {
    let tmp = Tmp::new("cli-team-remove-member-noop");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    run_json(
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
    run_json(
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
    run_json(
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

    let removed = run_json_machine(
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
    assert!(
        !project.join(".ac").join("seed-manifest.toml").exists(),
        "removing a team member with no recorded config scope must not create a seed manifest"
    );
}

// #1065 Stage F: with a pre-recorded config scope under the member's replica,
// production CLI `team remove-member` acquires the project gate (then the
// team-config guard), removes the replica, and prunes that scope's rows in the same
// transaction (acceptance items 10/22/43). End-to-end proof that the sole production
// activation token is threaded through CLI member removal. Public-outcome only.
#[test]
fn team_remove_member_prunes_recorded_replica_config_scope() {
    let tmp = Tmp::new("cli-team-remove-member-prune");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect", "dev-rust"]);

    run_json(
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
    run_json(
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
    run_json(
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
    let manifest = project.join(".ac").join("seed-manifest.toml");
    write_recorded_config_manifest(&manifest, "room-1-dev-team", "dev-rust");

    let removed = run_json_machine(
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
    let after = std::fs::read_to_string(&manifest).expect("manifest persists after member removal");
    assert!(
        !after.contains("__agent_dev-rust"),
        "the member's replica config scope must be pruned: {after}"
    );
    assert!(
        after.contains("files = []"),
        "pruning the only row leaves a valid empty v1 manifest: {after}"
    );
}

// #1065 Stage F: creating a workgroup records no manifest (config seed runs at
// session spawn, not at workgroup creation), and removing it with no recorded scope
// is a byte-stable no-op (plan section 8.2 cli_workgroup_team row, acceptance items
// 10/22/46). Public-outcome only, no token or hook.
#[test]
fn workgroup_creation_and_scopeless_removal_leave_no_manifest() {
    let tmp = Tmp::new("cli-wg-remove-noop");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let manifest = project.join(".ac").join("seed-manifest.toml");
    assert!(
        !manifest.exists(),
        "workgroup creation must not emit a seed manifest"
    );

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
    assert!(!wg_dir.exists(), "the workgroup directory is removed");
    assert!(
        !manifest.exists(),
        "removing a workgroup with no recorded scope must not create or prune a manifest"
    );
}

/// Write a valid v1 seed manifest that records a single replica config-folder scope
/// under `<workgroup>/__agent_<agent>/.claude`, exactly as a real config-seed spawn
/// would have. Used to prove Stage F lifecycle removal prunes a recorded scope.
///
/// The literal is a v1 manifest on purpose: the upgrade path must prove it
/// handles a pre-upgrade manifest written by an old build.
fn write_recorded_config_manifest(manifest_path: &Path, workgroup: &str, agent: &str) {
    let manifest = format!(
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 1\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/{workgroup}/__agent_{agent}/.claude/settings.json\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"replica_config_file\"\n",
            "scope = \"config:.ac/{workgroup}/__agent_{agent}/.claude\"\n",
            "source = \"workspace_base\"\n",
            "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
        ),
        workgroup = workgroup,
        agent = agent
    );
    std::fs::write(manifest_path, manifest).expect("write recorded config manifest");
}

// #1065 Stage F: with a pre-recorded config scope under the workgroup, production
// CLI removal acquires the project gate, removes the workgroup, and prunes that
// scope's rows in the same transaction. Because it was the only row, the manifest
// becomes a valid empty v1 file. This is the end-to-end proof that the sole
// production activation token is threaded through CLI workgroup removal (acceptance
// items 10/22). Public-outcome only.
#[test]
fn workgroup_remove_prunes_recorded_config_scope() {
    let tmp = Tmp::new("cli-wg-remove-prune");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, wg_dir) = create_basic_workgroup(tmp.path(), &bin, &config_dir);
    let manifest = project.join(".ac").join("seed-manifest.toml");
    write_recorded_config_manifest(&manifest, "room-1-dev-team", "architect");

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
    assert!(!wg_dir.exists(), "the workgroup directory is removed");
    let after = std::fs::read_to_string(&manifest).expect("manifest persists after removal");
    assert!(
        !after.contains("room-1-dev-team"),
        "the workgroup's config scope must be pruned: {after}"
    );
    assert!(
        after.contains("files = []"),
        "pruning the only row leaves a valid empty v1 manifest: {after}"
    );
}
