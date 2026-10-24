use super::*;

#[test]
fn issue_2868_team_leaf_requires_existing_ordinary_parent() {
    let temp = tempfile::tempdir().unwrap();
    let team = temp.path().join("_team_example");
    assert!(ensure_team_skills_dir(&team).is_err());
    assert!(!team.exists());
    std::fs::write(&team, "KEEP_PARENT").unwrap();
    assert!(ensure_team_skills_dir(&team).is_err());
    assert_eq!(std::fs::read_to_string(&team).unwrap(), "KEEP_PARENT");
    std::fs::remove_file(&team).unwrap();
    std::fs::create_dir(&team).unwrap();
    ensure_team_skills_dir(&team).unwrap();
    ensure_team_skills_dir(&team).unwrap();
    assert!(team.join("team-skills").is_dir());
    assert!(!team.join("config.json").exists());
}

#[test]
fn issue_2868_project_skills_missing_ordinary_and_file_are_not_clobbered() {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join(".ac");
    assert!(ensure_project_skills_dir(&ac).is_err());
    assert!(!ac.exists(), "helper must not create ancestors");
    std::fs::create_dir(&ac).unwrap();
    ensure_project_skills_dir(&ac).unwrap();
    ensure_project_skills_dir(&ac).unwrap();
    let source = ac.join("project-skills");
    assert!(source.is_dir());
    std::fs::remove_dir(&source).unwrap();
    std::fs::write(&source, "OCCUPIED").unwrap();
    assert_eq!(
        ensure_project_skills_dir(&ac).unwrap_err().to_string(),
        "not an ordinary directory"
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "OCCUPIED");
}

/// #1795 `T5`. The four names are asserted as LITERALS and the constant's
/// length is asserted separately. Iterating `PROJECT_SHARED_DIRS` to build the
/// expectation would make the oracle move with the constant, so control `C5`
/// (`"errors"` -> `"error"`) would leave this test green.
#[test]
fn create_project_shared_dirs_creates_all_four_and_is_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");

    assert_eq!(PROJECT_SHARED_DIRS.len(), 4);

    for run in 1..=2 {
        create_project_shared_dirs(&ac_root).expect("create project shared dirs");
        for name in ["plans", "tools", "errors", "project-shared"] {
            let path = ac_root.join(name);
            assert!(path.is_dir(), "run {run}: `{name}` must be a directory");
        }
    }
}

/// #1795 `T6a`. Same shape as `T5`, on the single room-level name.
#[test]
fn create_room_shared_dir_creates_and_is_idempotent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let room_root = temp.path().join("room-19-dev-team");

    for run in 1..=2 {
        create_room_shared_dir(&room_root).expect("create room shared dir");
        let path = room_root.join("room-shared");
        assert!(
            path.is_dir(),
            "run {run}: `room-shared` must be a directory"
        );
    }
}
