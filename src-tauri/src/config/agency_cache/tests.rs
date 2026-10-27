use super::*;
use crate::config::role_templates::AGENCY_MANIFEST_FILE;
use std::time::Duration;

fn manifest(template_count: usize) -> AgencyTemplatesManifest {
    AgencyTemplatesManifest {
        repo: "https://github.com/example/agency-agents".into(),
        reference: "main".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        template_count,
    }
}

fn write_valid_cache(root: &Path) {
    let role_dir = root.join("engineering").join("planner");
    fs::create_dir_all(&role_dir).expect("create role dir");
    fs::write(
        role_dir.join("Role.md"),
        "---\nname: Planner\n---\n\nPlan backend work.\n",
    )
    .expect("write role");
    fs::write(
        root.join(AGENCY_MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest(1)).expect("manifest json"),
    )
    .expect("write manifest");
}

#[test]
fn bounded_process_timeout_kills_and_reaps_child() {
    #[cfg(windows)]
    let (program, args) = (
        "powershell.exe",
        vec!["-NoProfile", "-Command", "Start-Sleep -Seconds 5"],
    );
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-c", "sleep 5"]);

    let err =
        run_process_with_timeout(program, None, &args, Duration::from_millis(100)).unwrap_err();

    assert!(err.contains("timed out"), "unexpected error: {err}");
    assert!(err.contains("killed and reaped"), "unexpected error: {err}");
}

#[test]
fn normalize_rejects_directory_named_md_role_entry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let extracted = tmp.path().join("extracted");
    let division = extracted.join("Engineering");
    let staging = tmp.path().join("staging");
    fs::create_dir_all(division.join("Planner.md")).expect("create directory named md");

    let err = normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0))
        .expect_err("directory named md must be rejected");

    assert!(err.contains("not a regular file"));
}

#[test]
fn normalize_skips_root_metadata_files_and_loads_valid_division() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let extracted = tmp.path().join("extracted");
    let division = extracted.join("Engineering");
    let staging = tmp.path().join("staging");
    fs::create_dir_all(&division).expect("create division");
    for name in [
        ".gitattributes",
        ".gitignore",
        "CONTRIBUTING.md",
        "LICENSE",
        "README.md",
        "SECURITY.md",
    ] {
        fs::write(extracted.join(name), "repo metadata\n").expect("write metadata file");
    }
    fs::write(
        division.join("Planner.md"),
        "---\nname: Planner\n---\n\nPlan backend work.\n",
    )
    .expect("write role");

    normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0)).expect("normalize repo");

    let templates = collect_agency_templates_from_dir(&staging).expect("collect templates");
    assert_eq!(templates.len(), 1);
    assert!(staging
        .join("engineering")
        .join("planner")
        .join("Role.md")
        .is_file());
}

#[test]
fn normalize_and_publish_handles_upstream_worktree_with_nested_roles() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path().join("config");
    let extracted = tmp.path().join("extracted");
    let direct = extracted.join("Game Development");
    let nested = direct.join("Unity");
    let staging = tmp.path().join("staging");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::create_dir_all(&nested).expect("create nested division");
    for name in [
        ".gitattributes",
        ".gitignore",
        "CONTRIBUTING.md",
        "LICENSE",
        "README.md",
        "SECURITY.md",
    ] {
        fs::write(extracted.join(name), "repo metadata\n").expect("write metadata file");
    }
    fs::write(
        direct.join("game-designer.md"),
        "---\nname: Game Designer\n---\n\nDesign game systems.\n",
    )
    .expect("write direct role");
    fs::write(
        nested.join("unity-architect.md"),
        "---\nname: Unity Architect\n---\n\nDesign\u{4} Unity architecture.\n",
    )
    .expect("write nested role");

    normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0)).expect("normalize repo");
    publish_staging(&config_dir, &staging).expect("publish cache");

    let live = agency_templates_dir(&config_dir);
    let manifest: AgencyTemplatesManifest = serde_json::from_str(
        &fs::read_to_string(live.join(AGENCY_MANIFEST_FILE)).expect("read manifest"),
    )
    .expect("parse manifest");
    assert_eq!(manifest.template_count, 2);
    assert!(live
        .join("game-development")
        .join("game-designer")
        .join("Role.md")
        .is_file());
    assert!(live
        .join("game-development")
        .join("unity-unity-architect")
        .join("Role.md")
        .is_file());
    let nested_body = fs::read_to_string(
        live.join("game-development")
            .join("unity-unity-architect")
            .join("Role.md"),
    )
    .expect("read nested role");
    assert!(!nested_body.contains('\u{4}'));
}

#[test]
fn normalize_rejects_symlinked_upstream_md_role_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let extracted = tmp.path().join("extracted");
    let division = extracted.join("Engineering");
    let staging = tmp.path().join("staging");
    fs::create_dir_all(&division).expect("create division");
    let outside = tmp.path().join("outside.md");
    fs::write(&outside, "External role body.\n").expect("write outside target");
    let link = division.join("Planner.md");
    if !try_symlink_file(&outside, &link) {
        return;
    }

    let err = normalize_extracted_repo_to_cache(&extracted, &staging, manifest(0))
        .expect_err("symlinked md must be rejected");

    assert!(err.contains("symlink") || err.contains("reparse"));
}

#[test]
fn legacy_stale_lock_with_missing_live_cache_recovers_one_valid_previous_cache() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let prev = config_dir.join(format!("{}.prev-one", AGENCY_TEMPLATES_DIR));
    write_valid_cache(&prev);
    let lock_path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
    fs::write(&lock_path, "").expect("write legacy stale lock");

    let lock = CacheLock::acquire(config_dir).expect("recover stale lock");
    assert!(lock_path.exists(), "new lock should be held after recovery");
    assert_eq!(
        recover_interrupted_publish(config_dir).expect("recover previous cache"),
        "recovered"
    );
    drop(lock);

    assert!(agency_templates_dir(config_dir).is_dir());
    assert!(!lock_path.exists(), "lock should be removed on drop");
}

#[test]
fn dead_owner_lock_is_replaced() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let lock_path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
    let dead_pid = 999_999;
    if process_is_running(dead_pid) {
        return;
    }
    fs::write(
        &lock_path,
        format!(r#"{{"pid":{},"createdUnixSecs":1}}"#, dead_pid),
    )
    .expect("write dead-owner lock");

    let lock = CacheLock::acquire(config_dir).expect("replace dead-owner lock");
    assert!(lock_path.exists(), "new lock should be held");
    drop(lock);

    assert!(!lock_path.exists(), "lock should be removed on drop");
}

#[test]
fn status_cache_at_recovers_valid_previous_cache() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let prev = config_dir.join(format!("{}.prev-one", AGENCY_TEMPLATES_DIR));
    write_valid_cache(&prev);

    let status = status_cache_at(config_dir).expect("status recovers previous cache");

    assert!(status.available);
    assert!(agency_templates_dir(config_dir).is_dir());
    assert!(!prev.exists(), "previous cache should be promoted");
    assert_eq!(status.template_count, Some(1));
}

#[test]
fn status_cache_at_reports_live_lock_as_locked() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let lock_path = config_dir.join(format!("{}.lock", AGENCY_TEMPLATES_DIR));
    fs::write(
        &lock_path,
        format!(r#"{{"pid":{},"createdUnixSecs":1}}"#, process::id()),
    )
    .expect("write live lock");

    let status = status_cache_at(config_dir).expect("locked status should be non-fatal");

    assert!(!status.available);
    assert_eq!(status.reason.as_deref(), Some("locked"));
    fs::remove_file(lock_path).expect("cleanup live lock");
}

#[test]
fn update_cache_at_reports_already_current_without_printed_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let live = agency_templates_dir(config_dir);
    write_valid_cache(&live);
    let current = current_manifest_if_valid(config_dir).expect("valid manifest");
    std::env::set_var("AGENTSCOMMANDER_AGENCY_TEST_COMMIT", &current.commit);

    let result = update_cache_at(
        config_dir,
        AgencyTemplatesUpdateArgs {
            repo: current.repo.clone(),
            reference: current.reference.clone(),
            json: true,
        },
    )
    .expect("already-current update should succeed");

    std::env::remove_var("AGENTSCOMMANDER_AGENCY_TEST_COMMIT");
    assert!(!result.updated);
    assert_eq!(result.repo, current.repo);
    assert_eq!(result.reference, current.reference);
    assert_eq!(result.commit, current.commit);
    assert_eq!(result.template_count, current.template_count);
    assert_eq!(result.path, live.to_string_lossy().to_string());
}

#[cfg(unix)]
fn try_symlink_file(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn try_symlink_file(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[cfg(not(any(unix, windows)))]
fn try_symlink_file(_target: &Path, _link: &Path) -> bool {
    false
}

#[test]
fn cached_manifest_keeps_count_mismatch_and_parse_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let live = agency_templates_dir(tmp.path());
    write_valid_cache(&live);
    let mut current = read_cached_agency_manifest(&live).unwrap();
    current.template_count = 9;
    write_staged_agency_manifest(&live, &current).unwrap();
    assert_eq!(
        current_manifest_if_valid(tmp.path()).unwrap_err(),
        "Cached manifest template count mismatch"
    );
    fs::write(live.join(AGENCY_MANIFEST_FILE), "{bad").unwrap();
    let error = serde_json::from_str::<AgencyTemplatesManifest>("{bad").unwrap_err();
    assert_eq!(
        current_manifest_if_valid(tmp.path()).unwrap_err(),
        format!("Failed to parse cached manifest: {error}")
    );
    fs::write(live.join(AGENCY_MANIFEST_FILE), [255]).unwrap();
    let error = fs::read_to_string(live.join(AGENCY_MANIFEST_FILE)).unwrap_err();
    assert_eq!(
        current_manifest_if_valid(tmp.path()).unwrap_err(),
        format!("Failed to read cached manifest: {error}")
    );
}

#[test]
fn staged_rewrite_persists_collected_count_before_publication() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let staging = tmp.path().join("staging");
    write_valid_cache(&staging);
    let mut current = read_staged_agency_manifest(&staging).unwrap();
    current.template_count = 9;
    write_staged_agency_manifest(&staging, &current).unwrap();
    let templates = collect_agency_templates_from_dir(&staging).unwrap();
    let mut current = read_staged_agency_manifest(&staging).unwrap();
    current.template_count = templates.len();
    write_staged_agency_manifest(&staging, &current).unwrap();
    collect_agency_templates_from_dir(&staging).unwrap();
    publish_staging(tmp.path(), &staging).unwrap();
    assert_eq!(
        current_manifest_if_valid(tmp.path())
            .unwrap()
            .template_count,
        1
    );
}
