use super::*;

#[test]
fn new_project_writes_project_registered_refresh() {
    let tmp = Tmp::new("cli-new-project-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);

    assert!(project.join(".ac").is_dir());
    let request = read_single_project_refresh_request(&config_dir);
    assert_registration_request(&request, &project);
}

#[test]
fn open_project_writes_project_registered_refresh_for_new_registration() {
    let tmp = Tmp::new("cli-open-project-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create project");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["open-project", &project_arg]);

    let request = read_single_project_refresh_request(&config_dir);
    assert_registration_request(&request, &project);
}

#[test]
fn open_project_noop_does_not_write_refresh() {
    let tmp = Tmp::new("cli-open-project-noop-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(project.join(".ac")).expect("create project");
    write_settings(&config_dir, &[&project]);
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["open-project", &project_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn new_project_bad_parent_path_does_not_write_settings_or_refresh() {
    let tmp = Tmp::new("cli-new-project-bad-parent");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let parent_file = tmp.path().join("parent-file");
    std::fs::write(&parent_file, "not a dir").expect("write parent file");
    let bad_project = parent_file.join("ChildProject");
    let bad_project_arg = bad_project.to_string_lossy().to_string();

    run_failure(&bin, &["new-project", &bad_project_arg]);

    assert!(!bad_project.exists());
    assert!(
        !config_dir.join("settings.30.instance.no-git.json").exists(),
        "settings.json should not be written on failed new-project"
    );
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_ac_side_effect_dirs(tmp.path());
}

#[test]
fn new_project_noop_does_not_write_refresh() {
    let tmp = Tmp::new("cli-new-project-noop-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();
    run_success(&bin, &["new-project", &project_arg]);
    clear_project_refresh_requests(&config_dir);

    run_success(&bin, &["new-project", &project_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

#[test]
fn open_project_invalid_path_does_not_write_refresh() {
    let tmp = Tmp::new("cli-open-project-invalid-refresh");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let missing = tmp.path().join("MissingProject");
    let missing_arg = missing.to_string_lossy().to_string();

    run_failure(&bin, &["open-project", &missing_arg]);

    assert!(project_refresh_request_paths(&config_dir).is_empty());
}

// #1065 Stage F ACTIVATED: registering a fresh project publishes its two project
// context templates, so `new-project` emits `.ac/seed-manifest.toml` with one
// `project_context_template` row each for `context:agentscommander` and
// `context:coordinator`. A subsequent open of the already-registered project is a
// no-op registration that must not backfill or rewrite the manifest (acceptance
// items 22/38). Public-outcome only: no activation token or private hook.
#[test]
fn new_project_emits_seed_manifest_and_open_does_not_backfill() {
    let tmp = Tmp::new("cli-project-registration-activated");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);
    assert!(project.join(".ac").is_dir());
    let manifest_path = project.join(".ac").join("seed-manifest.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .expect("fresh new-project must emit a seed manifest once Stage F is activated");
    assert!(
        manifest.contains("schema_version = 1"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("coverage_version = 2"),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:agentscommander\""),
        "manifest must record the project context template: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest must record the coordinator context template: {manifest}"
    );
    assert!(
        manifest.contains("kind = \"project_context_template\""),
        "manifest: {manifest}"
    );
    assert!(
        !manifest.contains("replica_config_file"),
        "registration publishes no config-folder rows: {manifest}"
    );

    // A subsequent open of the now-registered project is a no-op registration: the
    // templates already exist, so nothing is published and the manifest is unchanged.
    run_success(&bin, &["open-project", &project_arg]);
    let after_open = std::fs::read_to_string(&manifest_path).expect("manifest persists");
    assert_eq!(
        manifest, after_open,
        "open of an already-registered project must not rewrite or backfill the manifest"
    );
}

fn copy_dir_recursive(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create clone dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy file");
        }
    }
}

// #1065 Stage F: a cloned project (whose `.ac` was copied from an origin that a
// fresh `new-project` seeded) carries the origin's committed seed manifest
// byte-for-byte, and opening the clone (a no-op registration of an existing `.ac`)
// preserves it without backfilling, re-timestamping, or pruning (plan section 5.4
// clone row, acceptance items 19/38). Public-outcome only: no activation token,
// private hook, or helper barrier is used.
#[test]
fn cloned_project_open_preserves_the_manifest_byte_for_byte() {
    let tmp = Tmp::new("cli-project-clone-activated");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let origin = tmp.path().join("Origin");
    let origin_arg = origin.to_string_lossy().to_string();
    run_success(&bin, &["new-project", &origin_arg]);
    let origin_manifest = origin.join(".ac").join("seed-manifest.toml");
    let origin_bytes = std::fs::read(&origin_manifest)
        .expect("origin new-project emits a seed manifest once Stage F is activated");

    // Clone: copy the whole project (including `.ac` and its manifest) to a new
    // location.
    let clone = tmp.path().join("Clone");
    copy_dir_recursive(&origin, &clone);
    let clone_manifest = clone.join(".ac").join("seed-manifest.toml");
    assert_eq!(
        std::fs::read(&clone_manifest).expect("the clone carries its manifest"),
        origin_bytes,
        "the clone starts with the origin's manifest bytes"
    );

    let clone_arg = clone.to_string_lossy().to_string();
    run_success(&bin, &["open-project", &clone_arg]);

    assert_eq!(
        std::fs::read(&clone_manifest).expect("clone manifest persists"),
        origin_bytes,
        "opening a clone preserves the manifest byte-for-byte \
         (no backfill, reprune, or re-timestamp)"
    );
    assert_eq!(
        std::fs::read(&origin_manifest).expect("origin manifest persists"),
        origin_bytes,
        "the origin manifest is untouched by opening the clone"
    );
}
