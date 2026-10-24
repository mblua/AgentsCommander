use super::*;
use crate::config::settings::AppSettings;

#[test]
fn raw_project_path_containing_returns_the_matching_archived_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create agent dir");
    let archived = archived.to_string_lossy().to_string();

    assert_eq!(
        crate::config::sessions_persistence::raw_project_path_containing(
            &agent.to_string_lossy(),
            std::slice::from_ref(&archived)
        ),
        Some(archived)
    );
}

#[test]
fn raw_project_path_containing_returns_none_for_a_cwd_outside_every_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let other = temp.path().join("other").join(".ac").join("wg-1");
    std::fs::create_dir_all(&archived).expect("create archived");
    std::fs::create_dir_all(&other).expect("create other");
    let archived = archived.to_string_lossy().to_string();

    assert_eq!(
        crate::config::sessions_persistence::raw_project_path_containing(
            &other.to_string_lossy(),
            &[archived]
        ),
        None
    );
}

#[test]
fn raw_project_path_containing_returns_none_on_an_empty_root_list() {
    crate::config::sessions_persistence::reset_normalize_call_count();
    assert_eq!(
        crate::config::sessions_persistence::raw_project_path_containing(
            "Z:/does/not/exist/x",
            &[]
        ),
        None
    );
    assert_eq!(
        crate::config::sessions_persistence::normalize_call_count(),
        0,
        "empty roots must return before canonicalizing the candidate"
    );
}

#[test]
fn archived_root_for_cwd_returns_the_raw_archived_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("MixedCaseProject");
    let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&agent).expect("create agent dir");
    let raw = archived.to_string_lossy().replace('\\', "/");
    let cwd = agent.to_string_lossy().to_string();

    let matched = archived_root_for_cwd(&cwd, &[String::new(), raw.clone()])
        .expect("archived root should match");

    assert_eq!(matched, raw, "activation gate must preserve stored bytes");
}

#[test]
fn auto_unarchive_registration_validates_the_archived_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("missing-workspace-project");
    std::fs::create_dir_all(&project).expect("create project dir");
    let project = project.to_string_lossy().to_string();
    let mut settings = AppSettings {
        archived_project_paths: vec![project.clone()],
        ..AppSettings::default()
    };

    let err = auto_unarchive_registration(&mut settings, &project)
        .expect_err("missing Project AC Root must fail");

    assert!(matches!(
        err,
        crate::config::projects::ProjectError::AcRootMissing(_)
    ));
    assert!(settings.project_paths.is_empty());
    assert_eq!(settings.archived_project_paths, vec![project]);
}

#[test]
fn probe_spawn_refusal_rejects_an_archived_root_without_a_ac_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("archived-project");
    std::fs::create_dir_all(&project).expect("create project dir");

    let err = probe_spawn_refusal_for_archived_root(&project.to_string_lossy())
        .expect_err("missing workspace should reject");

    assert!(err.contains("archived-project"), "{err}");
}

#[test]
fn probe_spawn_refusal_accepts_an_archived_root_that_still_has_a_ac_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("archived-project");
    std::fs::create_dir_all(project.join(".ac")).expect("create workspace");

    probe_spawn_refusal_for_archived_root(&project.to_string_lossy())
        .expect("workspace should accept");
}

#[test]
fn auto_unarchive_registration_coalesces_when_project_is_no_longer_archived() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("archived-project");
    std::fs::create_dir_all(project.join(".ac")).expect("create workspace");
    let project = project.to_string_lossy().to_string();
    let mut settings = AppSettings {
        archived_project_paths: vec![project.clone()],
        ..AppSettings::default()
    };

    let first = auto_unarchive_registration(&mut settings, &project)
        .expect("first registration")
        .expect("first caller should unarchive");
    assert_eq!(first.path, project);
    assert!(settings.archived_project_paths.is_empty());
    assert_eq!(settings.project_paths.len(), 1);

    let second =
        auto_unarchive_registration(&mut settings, &first.path).expect("second registration");
    assert!(second.is_none(), "second caller must coalesce");
}
