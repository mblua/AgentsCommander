use super::*;

#[test]
fn issue_3294_store_order_positive_control() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path();
    let guard = TeamConfigMutationGuard::acquire(ac_root).unwrap();
    let config = TeamConfigResult {
        agents: Vec::new(),
        coordinator: String::new(),
        repos: Vec::new(),
        context_alert_percentages: vec![75, 50],
    };
    let team_dir = ac_root.join("_team_sample");
    std::fs::create_dir(&team_dir).unwrap();
    let path = config_path(ac_root, "sample");
    let previous = b"previous config bytes\n";
    std::fs::write(&path, previous).unwrap();
    let error = write_team_config_guarded_with_publisher(
        ac_root,
        "sample",
        &config,
        &guard,
        |target, bytes| {
            assert_eq!(target, path);
            assert!(team_dir.join("memory").is_dir());
            assert_eq!(std::fs::read(team_dir.join("conventions.md")).unwrap(), b"");
            assert_eq!(
                bytes,
                normalized_team_config_bytes(ac_root, &config).unwrap()
            );
            assert_eq!(std::fs::read(target).unwrap(), previous);
            Err("injected publisher failure".to_string())
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        "Failed to publish config.json: injected publisher failure"
    );
    assert!(team_dir.join("memory").is_dir());
    assert!(team_dir.join("conventions.md").is_file());
    assert_eq!(std::fs::read(&path).unwrap(), previous);

    let invalid = TeamConfigResult {
        context_alert_percentages: vec![0],
        ..config
    };
    let error =
        write_team_config_guarded_with_publisher(ac_root, "invalid", &invalid, &guard, |_, _| {
            panic!("invalid normalization must precede publication")
        })
        .unwrap_err();
    assert_eq!(
        error,
        "contextAlertPercentages values must be integer percentages from 1 through 100"
    );
    assert!(!ac_root.join("_team_invalid").exists());
}

#[test]
fn issue_3493_metadata_only_positive_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("_team_Valid-123");
    std::fs::create_dir(&root).unwrap();
    let config = root.join("config.json");
    for bytes in [b"{}".as_slice(), b"malformed", b"", &[0xff, 0xfe]] {
        std::fs::write(&config, bytes).unwrap();
        assert_eq!(
            inspect_team_skill_source(&root),
            TeamSkillSourceInspection::Accepted
        );
    }
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Rejected {
            name: "Valid-123".into(),
            reason: "not an ordinary unlinked source".into(),
        }
    );
}

#[test]
fn issue_3493_not_team_and_invalid_names_precede_io() {
    assert_eq!(
        inspect_team_skill_source(Path::new("/")),
        TeamSkillSourceInspection::NotTeam
    );
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        inspect_team_skill_source(&temp.path().join("ordinary")),
        TeamSkillSourceInspection::NotTeam
    );
    for name in ["", "é", "bad_name"] {
        let root = temp.path().join(format!("_team_{name}"));
        assert_eq!(
            inspect_team_skill_source(&root),
            TeamSkillSourceInspection::Rejected {
                name: name.into(),
                reason: "invalid team name".into(),
            }
        );
    }
}

#[test]
fn issue_3493_root_and_config_errors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("_team_sample");
    let missing_root = std::fs::symlink_metadata(&root).unwrap_err().to_string();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Rejected {
            name: "sample".into(),
            reason: missing_root,
        }
    );
    std::fs::write(&root, b"ordinary file").unwrap();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Rejected {
            name: "sample".into(),
            reason: "not an ordinary unlinked source".into(),
        }
    );
    std::fs::remove_file(&root).unwrap();
    std::fs::create_dir(&root).unwrap();
    let missing_config = std::fs::symlink_metadata(root.join("config.json"))
        .unwrap_err()
        .to_string();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Rejected {
            name: "sample".into(),
            reason: missing_config,
        }
    );
}

#[test]
fn issue_3493_non_utf8_basename_is_not_team() {
    #[cfg(windows)]
    let name = {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = "_team_".encode_utf16().collect();
        units.push(0xD800);
        std::ffi::OsString::from_wide(&units)
    };
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b"_team_\xff").to_os_string()
    };
    assert_eq!(
        inspect_team_skill_source(Path::new(&name)),
        TeamSkillSourceInspection::NotTeam
    );
}

#[test]
fn issue_3493_rejects_symlink_root() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("config.json"), b"{}").unwrap();
    let root = temp.path().join("_team_sample");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &root).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &root).unwrap();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Rejected {
            name: "sample".into(),
            reason: "not an ordinary unlinked source".into(),
        }
    );
}

#[test]
fn issue_3493_rejects_symlink_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("_team_sample");
    std::fs::create_dir(&root).unwrap();
    let target = temp.path().join("target.json");
    std::fs::write(&target, b"{}").unwrap();
    let config = root.join("config.json");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &config).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &config).unwrap();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Rejected {
            name: "sample".into(),
            reason: "not an ordinary unlinked source".into(),
        }
    );
}

#[test]
fn issue_3493_accepts_regular_hardlink_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("_team_sample");
    std::fs::create_dir(&root).unwrap();
    let target = temp.path().join("target.json");
    std::fs::write(&target, b"{}").unwrap();
    std::fs::hard_link(&target, root.join("config.json")).unwrap();
    assert_eq!(
        inspect_team_skill_source(&root),
        TeamSkillSourceInspection::Accepted
    );
}
