use super::*;

#[test]
fn issue_3142_generic_independent_payload_and_multiple_copies() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    let selected = [
        "default",
        "defaults.json",
        "default.claude",
        "my-default",
        "default-tools/profiles/linux",
        "default-tools/profiles/windows",
        "skills/nested",
    ];
    for directory in selected {
        std::fs::create_dir_all(source.join(directory).join(".hidden/empty")).unwrap();
        std::fs::write(
            source.join(directory).join(".hidden/payload"),
            b"opaque\0\xff $AGENTSCOMMANDER_CONTEXT {{literal}}\r\n",
        )
        .unwrap();
    }
    for directory in [".claude", "memory/default-nested", "plans/default-only"] {
        std::fs::create_dir_all(source.join(directory)).unwrap();
        std::fs::write(source.join(directory).join("excluded"), b"private").unwrap();
    }
    std::fs::write(source.join("default-file"), b"not a directory").unwrap();
    std::fs::write(source.join("Role.md"), b"\xef\xbb\xbfrole\r\n\0\xff").unwrap();
    let before = issue_3068_inventory(&source);
    let first = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_first").unwrap();
    let second =
        copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_second").unwrap();
    assert_eq!(before, issue_3068_inventory(&source));
    for directory in selected {
        assert!(first.root.join(directory).join(".hidden/empty").is_dir());
        assert_eq!(
            std::fs::read(source.join(directory).join(".hidden/payload")).unwrap(),
            std::fs::read(first.root.join(directory).join(".hidden/payload")).unwrap()
        );
    }
    assert_eq!(
        std::fs::read(first.root.join("Role.md")).unwrap(),
        std::fs::read(source.join("Role.md")).unwrap()
    );
    for directory in ["memory", "plans", "inbox", "outbox"] {
        assert_eq!(
            std::fs::read_dir(first.root.join(directory))
                .unwrap()
                .count(),
            0
        );
    }
    let root_names = std::fs::read_dir(&first.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(root_names.iter().any(|name| name == "my-default"));
    for excluded in [
        ".claude",
        "memory-archive",
        "unknown",
        ".hidden",
        "empty",
        "default-file",
    ] {
        assert!(
            !root_names.iter().any(|name| name == excluded),
            "{excluded}"
        );
    }
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(first.root.join("config.json")).unwrap()).unwrap();
    assert_eq!(config, default_agent_matrix_config());
    assert_eq!(
        issue_3068_history(&first.root)["copies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    std::fs::write(first.root.join("Role.md"), b"destination edit").unwrap();
    assert_eq!(before, issue_3068_inventory(&source));
    assert_ne!(
        std::fs::read(first.root.join("Role.md")).unwrap(),
        std::fs::read(second.root.join("Role.md")).unwrap()
    );
    cleanup_owned_copy_artifacts(&second).unwrap();
    assert!(!second.root.exists());
}

#[test]
fn issue_3142_uppercase_default_excluded_in_independent_source() {
    // AC-3142-002: case aliases cannot coexist in a Windows source root.
    let (_tmp, project, source, destination) = issue_3068_fixture();
    std::fs::create_dir(source.join("My-Default")).unwrap();
    std::fs::write(source.join("My-Default/private"), b"excluded payload").unwrap();
    let before = issue_3068_inventory(&source);
    assert!(before.contains_key(Path::new("My-Default/private")));
    assert!(!before.contains_key(Path::new("my-default")));
    let owned = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap();
    let root_names = std::fs::read_dir(&owned.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(!root_names.iter().any(|name| name == "My-Default"));
    assert!(!root_names.iter().any(|name| name == "my-default"));
    assert_eq!(before, issue_3068_inventory(&source));
    cleanup_owned_copy_artifacts(&owned).unwrap();
}

#[test]
fn issue_3142_hops_start_fresh_history() {
    let (tmp, project, _source, destination) = issue_3068_fixture();
    let first = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_beta").unwrap();
    let mut history = issue_3068_history(&first.root);
    history["unknownDocument"] = serde_json::json!({"private": true});
    history["copies"][0]["unknownEvent"] = serde_json::json!("private");
    std::fs::write(
        first.root.join(COPY_PROVENANCE_FILE),
        serde_json::to_vec(&history).unwrap(),
    )
    .unwrap();
    let next_project = tmp.path().join("next-project");
    std::fs::create_dir_all(next_project.join(".ac")).unwrap();
    let before = issue_3068_inventory(&first.root);
    let second = copy_agent_matrix_strict(
        destination.parent().unwrap(),
        "beta",
        &next_project.join(".ac"),
        "_agent_gamma",
    )
    .unwrap();
    assert_eq!(before, issue_3068_inventory(&first.root));
    let fresh = issue_3068_history(&second.root);
    assert_eq!(fresh["copies"].as_array().unwrap().len(), 1);
    assert_eq!(fresh["copies"][0]["sourceAgentRef"], "_agent_beta");
    assert!(fresh.get("unknownDocument").is_none());
    assert!(fresh["copies"][0].get("unknownEvent").is_none());
    let text = fresh["copies"][0]["copiedAt"].as_str().unwrap();
    assert!(text.ends_with('Z'));
    chrono::DateTime::parse_from_rfc3339(text).unwrap();
    cleanup_owned_copy_artifacts(&second).unwrap();
}

#[test]
fn issue_3142_source_history_is_opaque_and_intact() {
    for history in [
        b"{\"schemaVersion\":1,\"copies\":[]}".as_slice(),
        b"not json".as_slice(),
        b"{\"schemaVersion\":2,\"copies\":[]}".as_slice(),
        concat!(
            r#"{"schemaVersion":1,"copies":[{"#,
            r#""sourceProjectName":"p","sourceProjectPath":"/p","#,
            r#""sourceAgentRef":"_agent_a","copiedAt":"yesterday"}]}"#,
        )
        .as_bytes(),
        concat!(
            r#"{"schemaVersion":1,"copies":[{"#,
            r#""sourceProjectName":"","sourceProjectPath":"/p","#,
            r#""sourceAgentRef":"_agent_a","copiedAt":"2026-01-01T00:00:00Z"}]}"#,
        )
        .as_bytes(),
        concat!(
            r#"{"schemaVersion":1,"copies":[{"#,
            r#""sourceProjectName":"p","sourceProjectPath":"/p","#,
            r#""sourceAgentRef":"../a","copiedAt":"2026-01-01T00:00:00Z"}]}"#,
        )
        .as_bytes(),
        concat!(
            r#"{"schemaVersion":1,"copies":[{"#,
            r#""sourceProjectName":"p","sourceProjectPath":"/p","#,
            r#""sourceAgentRef":"_agent_a","copiedAt":"2026-01-01T00:00:00+02:00"}]}"#,
        )
        .as_bytes(),
    ] {
        let (_tmp, project, source, destination) = issue_3068_fixture();
        std::fs::write(source.join(COPY_PROVENANCE_FILE), history).unwrap();
        let before = issue_3068_inventory(&source);
        let result = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy");
        let owned = result.unwrap();
        assert_eq!(
            issue_3068_history(&owned.root)["copies"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        cleanup_owned_copy_artifacts(&owned).unwrap();
        assert_eq!(before, issue_3068_inventory(&source));
    }
}

#[test]
fn issue_3142_excluded_changes_before_verify_succeed() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut callback = |stage, path: &Path| {
        if stage == MatrixCopyStage::BeforeVerify {
            std::fs::write(path.join(COPY_PROVENANCE_FILE), b"malformed history").unwrap();
            std::fs::write(path.join("config.json"), b"malformed config").unwrap();
            std::fs::create_dir(path.join("unrelated-added")).unwrap();
            std::fs::write(path.join("memory/new-private"), b"private").unwrap();
        }
        Ok(())
    };
    let owned = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!owned.root.join("unrelated-added").exists());
    assert!(!owned.root.join("memory/new-private").exists());
    assert_eq!(
        issue_3068_history(&owned.root)["copies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    cleanup_owned_copy_artifacts(&owned).unwrap();
}

#[test]
fn issue_3142_no_defaults_and_missing_skills_succeed() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    std::fs::remove_dir_all(source.join("default-tools")).unwrap();
    std::fs::remove_dir(source.join("skills")).unwrap();
    let before = issue_3068_inventory(&source);
    let owned = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap();
    for directory in AGENT_MATRIX_DIRS {
        assert_eq!(
            std::fs::read_dir(owned.root.join(directory))
                .unwrap()
                .count(),
            0
        );
    }
    assert_eq!(before, issue_3068_inventory(&source));
    cleanup_owned_copy_artifacts(&owned).unwrap();
}

#[test]
fn issue_3142_required_asset_kinds_and_exact_role_name() {
    for mutation in 0..4 {
        let (_tmp, project, source, destination) = issue_3068_fixture();
        match mutation {
            0 => std::fs::remove_file(source.join("Role.md")).unwrap(),
            1 => {
                std::fs::remove_file(source.join("Role.md")).unwrap();
                std::fs::create_dir(source.join("Role.md")).unwrap();
            }
            2 => {
                std::fs::remove_dir(source.join("skills")).unwrap();
                std::fs::write(source.join("skills"), b"wrong kind").unwrap();
            }
            _ => {
                std::fs::remove_file(source.join("Role.md")).unwrap();
                std::fs::write(source.join("role.md"), b"wrong case").unwrap();
            }
        }
        let before = issue_3068_inventory(&source);
        assert!(copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").is_err());
        assert!(!destination.join("_agent_copy").exists());
        assert_eq!(before, issue_3068_inventory(&source));
    }
}

#[test]
fn issue_3142_selected_replacement_and_root_replacement_fail() {
    for relative in ["", "Role.md", "skills", "default-tools"] {
        let (_tmp, project, source, destination) = issue_3068_fixture();
        let mut callback = |stage, _: &Path| {
            if stage == MatrixCopyStage::BeforeVerify {
                let selected = source.join(relative);
                let moved = project.join("held-original");
                std::fs::rename(&selected, &moved).unwrap();
                if relative == "Role.md" {
                    std::fs::copy(&moved, &selected).unwrap();
                } else {
                    std::fs::create_dir(&selected).unwrap();
                    if relative.is_empty() {
                        // AC-3142-001: preserve every selected child identity/stamp;
                        // only the source root identity may cause this rejection.
                        for child in ["Role.md", "skills", "default-tools"] {
                            std::fs::rename(moved.join(child), selected.join(child)).unwrap();
                        }
                    }
                }
            }
            Ok(())
        };
        let error = copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            },
        )
        .unwrap_err();
        if relative.is_empty() {
            assert!(
                error.contains("Source Matrix root replaced"),
                "AC-3142-001: {error}"
            );
        }
        assert!(!destination.join("_agent_copy").exists());
        assert!(project.join("held-original").exists());
    }
}

#[test]
fn issue_3142_selected_classification_failures_clean_output() {
    for target in ["skills", "Role.md", "default-tools"] {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        let mut triggered = false;
        let mut callback = |stage, path: &Path| {
            if stage == MatrixCopyStage::Classify && path.ends_with(target) {
                triggered = true;
                return Err("injected selected read failure".into());
            }
            Ok(())
        };
        let error = copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(triggered && error.contains("injected selected read failure"));
        assert!(!destination.join("_agent_copy").exists());
    }
}

#[test]
fn issue_3142_selection_rejects_unsafe_kinds_without_reading_exclusions() {
    let (_tmp, _project, source, _destination) = issue_3068_fixture();
    for classification in [
        (true, 0, true, false),
        (false, 0x400, true, false),
        (false, 0, false, false),
    ] {
        let hooks = MatrixCopyHooks {
            classification: Some(classification),
            ..Default::default()
        };
        for name in ["Role.md", "skills", "default-tools"] {
            assert!(
                OwnedCopyArtifacts::selected_root_entry(&source.join(name), name, &hooks).is_err()
            );
        }
        assert!(!OwnedCopyArtifacts::selected_root_entry(
            &source.join("not-present"),
            "not-present",
            &hooks
        )
        .unwrap());
    }
    assert!(OwnedCopyArtifacts::selected_root_entry(
        &source.join("missing-default"),
        "missing-default",
        &MatrixCopyHooks::default()
    )
    .is_err());
}

#[cfg(unix)]
#[test]
fn issue_3142_excluded_non_utf8_root_name_fails() {
    use std::os::unix::ffi::OsStringExt;
    let (_tmp, project, source, destination) = issue_3068_fixture();
    std::fs::write(
        source.join(std::ffi::OsString::from_vec(vec![255])),
        b"excluded",
    )
    .unwrap();
    let error =
        copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap_err();
    assert!(error.contains("Non-UTF-8"));
    assert!(!destination.join("_agent_copy").exists());
}

#[test]
fn issue_3142_config_collision_retains_foreign_output() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut callback = |stage, _: &Path| {
        if stage == MatrixCopyStage::BeforeVerify {
            std::fs::write(
                destination.join("_agent_copy/config.json"),
                b"external config",
            )
            .unwrap();
        }
        Ok(())
    };
    let error = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.contains("residues"));
    assert_eq!(
        std::fs::read(destination.join("_agent_copy/config.json")).unwrap(),
        b"external config"
    );
    assert!(!destination.join("_agent_copy/Role.md").exists());
}
