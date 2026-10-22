use super::*;

#[test]
pub(crate) fn project_absence_preserves_direct_report_and_catalog() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"Personal"}]}"#,
    );
    assert_eq!(
        report_json(&project_report(dir.path())),
        report_json(&load_catalog_report(dir.path()))
    );
    assert!(!project_catalog_path(dir.path()).exists());
}

#[test]
pub(crate) fn project_then_personal_merges_fields_nulls_and_exact_order() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude", "codex", "pi"]),
        "stale",
        true,
    );
    let mut project = patch_rows(serde_json::json!([
        {"key":"claude","label":"Project","instructionsFilename":null,"idleBurst":{"maxBytes":10,
            "maxSecs":2},"installCommands":{"default":"install","windows":"win"}}, extra_row("team")
    ]));
    project["order"] = serde_json::json!(["team", "pi", "unknown"]);
    write_project_patch(dir.path(), project);
    let mut personal = patch_rows(serde_json::json!([
        {"key":"claude","label":"Personal","idleBurst":{"maxSecs":3},
            "installCommands":{"windows":null}}, extra_row("mine")
    ]));
    personal["order"] = serde_json::json!(["mine", "claude"]);
    write_local(dir.path(), &personal.to_string());
    let report = project_report(dir.path());
    assert_eq!(
        keys_of(&report.catalog),
        ["mine", "claude", "team", "pi", "codex"]
    );
    let claude = &report.catalog[1];
    assert_eq!(claude.label, "Personal");
    assert_eq!(claude.instructions_filename, None);
    assert_eq!(claude.idle_burst.as_ref().unwrap().max_bytes, Some(10));
    assert_eq!(claude.idle_burst.as_ref().unwrap().max_secs, Some(3.0));
    assert_eq!(claude.install_commands.as_ref().unwrap().default, "install");
    assert_eq!(claude.install_commands.as_ref().unwrap().windows, None);
    assert!(!report
        .warnings
        .iter()
        .any(|w| w.code == "projectInvalid" || w.code == "localInvalid"));
}

#[test]
pub(crate) fn project_invalid_schema_preserves_base_and_independent_personal() {
    let cases = [
        "<<<<<<< conflict",
        "{",
        r#"{"schemaVersion":999,"agents":[]}"#,
        r#"{"schemaVersion":1,"agents":[],"unexpected":true}"#,
        r#"{"schemaVersion":1,"schemaVersion":1,"agents":[]}"#,
        r#"{"schemaVersion":1,"agents":[{"key":"claude"},{"key":"claude"}]}"#,
        r#"{"schemaVersion":1,"agents":[],"order":["claude","claude"]}"#,
        concat!(
            r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"partial"},{"key":"new","#,
            r#""label":"incomplete"}]}"#,
        ),
    ];
    for bytes in cases {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        std::fs::write(project_catalog_path(dir.path()), bytes).unwrap();
        write_local(
            dir.path(),
            r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"Personal"}]}"#,
        );
        let report = project_report(dir.path());
        assert_eq!(report.catalog[0].label, "Personal", "{bytes}");
        let warning = report
            .warnings
            .iter()
            .find(|w| w.code == "projectInvalid")
            .unwrap();
        assert_eq!(
            warning.path,
            project_catalog_path(dir.path()).display().to_string()
        );
        assert!(!warning.reason.contains("local"), "{}", warning.reason);
        assert_eq!(
            std::fs::read(project_catalog_path(dir.path())).unwrap(),
            bytes.as_bytes()
        );
    }
}

#[test]
pub(crate) fn project_rejection_has_no_donor_and_personal_rejection_keeps_project() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_project_patch(
        dir.path(),
        patch_rows(serde_json::json!([extra_row("team"), {"key":"claude","command":""}])),
    );
    write_local(
        dir.path(),
        &patch_rows(serde_json::json!([{"key":"team","label":"partial"}])).to_string(),
    );
    let report = project_report(dir.path());
    assert!(!report.catalog.iter().any(|d| d.key == "team"));
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|w| w.code == "projectInvalid" || w.code == "localInvalid")
            .count(),
        2
    );
    write_local(
        dir.path(),
        &patch_rows(serde_json::json!([extra_row("team")])).to_string(),
    );
    assert!(project_report(dir.path())
        .catalog
        .iter()
        .any(|d| d.key == "team"));
    write_project_patch(
        dir.path(),
        patch_rows(serde_json::json!([{"key":"claude","label":"Team"}])),
    );
    write_local(dir.path(), "invalid");
    assert_eq!(project_report(dir.path()).catalog[0].label, "Team");
}
#[test]
fn project_removability_and_resurrection_use_accepted_lower_vector() {
    let dir = seed_dir();
    let mut row = extra_row("team");
    row["removable"] = serde_json::json!(false);
    write_managed_base(dir.path(), &[row], "stale", true);
    write_project_patch(
        dir.path(),
        patch_rows(serde_json::json!([{"key":"team","remove":true}])),
    );
    assert!(project_report(dir.path())
        .warnings
        .iter()
        .any(|w| w.code == "projectInvalid"));
    write_project_patch(
        dir.path(),
        patch_rows(serde_json::json!([{"key":"team","removable":true}])),
    );
    write_local(
        dir.path(),
        &patch_rows(serde_json::json!([{"key":"team","remove":true}])).to_string(),
    );
    assert!(project_report(dir.path()).catalog.is_empty());
    write_managed_base(dir.path(), &[extra_row("team")], "stale", true);
    write_project_patch(
        dir.path(),
        patch_rows(serde_json::json!([{"key":"team","remove":true}])),
    );
    write_local(
        dir.path(),
        &patch_rows(serde_json::json!([{"key":"team","label":"partial"}])).to_string(),
    );
    let report = project_report(dir.path());
    assert!(report.catalog.is_empty());
    assert!(report.warnings.iter().any(|w| w.code == "localInvalid"));
    write_local(
        dir.path(),
        &patch_rows(serde_json::json!([extra_row("team")])).to_string(),
    );
    assert_eq!(keys_of(&project_report(dir.path()).catalog), ["team"]);
}

#[test]
fn project_snapshot_observes_churn_absence_errors_and_excludes_instance() {
    let dir = seed_dir();
    for scope in [CatalogReadScope::Project, CatalogReadScope::Instance] {
        let mut calls = 0;
        let snapshot = read_catalog_snapshot_with(dir.path(), scope, |path, _| {
            if path == project_catalog_path(dir.path()) {
                calls += 1;
            }
            Ok(None)
        })
        .unwrap();
        assert_eq!(
            calls,
            if scope == CatalogReadScope::Project {
                2
            } else {
                0
            }
        );
        assert_eq!(
            snapshot.project.is_some(),
            scope == CatalogReadScope::Project
        );
    }
    for change in 0..3 {
        let mut calls = 0;
        let result =
            read_catalog_snapshot_with(dir.path(), CatalogReadScope::Project, |path, _| {
                if path == project_catalog_path(dir.path()) {
                    calls += 1;
                    return match change {
                        0 => Ok(Some(vec![calls])),
                        1 => {
                            if calls % 2 == 0 {
                                Ok(None)
                            } else {
                                Ok(Some(vec![]))
                            }
                        }
                        _ => Err(format!("read error {calls}")),
                    };
                }
                Ok(None)
            });
        assert_eq!(calls, 3);
        assert!(result.err().unwrap().contains("project overrides"));
    }
}

#[test]
fn project_settings_primary_only_and_direct_instance_matrix() {
    let primary = tempfile::tempdir().unwrap();
    let secondary = tempfile::tempdir().unwrap();
    let instance = tempfile::tempdir().unwrap();
    for root in [
        primary.path().join(".ac"),
        secondary.path().join(".ac"),
        instance.path().to_path_buf(),
    ] {
        ensure_seeded(&root, None);
        write_project_patch(
            &root,
            patch_rows(serde_json::json!([{"key":"claude","label":"Project"}])),
        );
        write_local(
            &root,
            r#"{"schemaVersion":1,"agents":[{"key":"codex","label":"Personal"}]}"#,
        );
        let direct = load_catalog_report(&root);
        assert_eq!(direct.catalog[0].label, "Claude Code");
        assert_eq!(direct.catalog[1].label, "Personal");
        assert_eq!(
            keys_of(&load_catalog(&root).unwrap()),
            keys_of(&direct.catalog)
        );
    }
    let mut settings = AppSettings {
        project_paths: vec![
            " ".into(),
            primary.path().display().to_string(),
            secondary.path().display().to_string(),
        ],
        project_path: Some(secondary.path().display().to_string()),
        ..Default::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(
        &settings,
        Some(instance.path().to_path_buf()),
    );
    assert_eq!(report.catalog[0].label, "Project");
    assert_eq!(
        report.primary_project_root,
        Some(primary.path().display().to_string())
    );
    std::fs::write(manifest_path(&primary.path().join(".ac")), "bad").unwrap();
    assert!(load_catalog_report_for_settings_with_config_dir(
        &settings,
        Some(instance.path().to_path_buf())
    )
    .unavailable
    .is_some());
    settings.project_paths.clear();
    settings.project_path = None;
    assert_eq!(
        load_catalog_report_for_settings_with_config_dir(
            &settings,
            Some(instance.path().to_path_buf())
        )
        .catalog[0]
            .label,
        "Claude Code"
    );
}

#[test]
fn project_nonregular_and_read_errors_degrade_only_project() {
    fn inventory(
        root: &std::path::Path,
    ) -> Vec<(
        std::path::PathBuf,
        bool,
        u64,
        bool,
        std::time::SystemTime,
        Vec<u8>,
    )> {
        #[cfg(test)]
        type TreeInventoryRows = Vec<(
            std::path::PathBuf,
            bool,
            u64,
            bool,
            std::time::SystemTime,
            Vec<u8>,
        )>;
        fn visit(root: &std::path::Path, path: &std::path::Path, rows: &mut TreeInventoryRows) {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            rows.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                metadata.is_dir(),
                metadata.len(),
                metadata.permissions().readonly(),
                metadata.modified().unwrap(),
                if metadata.is_file() {
                    std::fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
            ));
            if metadata.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                entries.sort();
                for entry in entries {
                    visit(root, &entry, rows);
                }
            }
        }
        let mut rows = Vec::new();
        visit(root, root, &mut rows);
        rows
    }
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let baseline = project_report(dir.path());
    std::fs::create_dir(project_catalog_path(dir.path())).unwrap();
    std::fs::write(project_catalog_path(dir.path()).join("keep.txt"), b"keep").unwrap();
    let before = inventory(dir.path());
    let report = project_report(dir.path());
    assert_eq!(report.catalog.len(), 8);
    assert!(report.warnings.iter().any(|w| w.code == "projectInvalid"));
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog, baseline.catalog);
    assert_eq!(keys_of(&report.catalog), keys_of(&baseline.catalog));
    assert!(report.warnings.iter().any(|w| w.code == "projectInvalid"
        && w.path == project_catalog_path(dir.path()).display().to_string()
        && !w.reason.is_empty()));
    for _ in 0..3 {
        assert_eq!(project_report(dir.path()), report);
    }
    assert_eq!(inventory(dir.path()), before);
    let snapshot = read_catalog_snapshot_with(
        dir.path(),
        CatalogReadScope::Project,
        |path, description| {
            if path == project_catalog_path(dir.path()) {
                Err("project read denied".into())
            } else {
                read_optional_regular_file(path, description)
            }
        },
    )
    .unwrap();
    let resolved = resolve_catalog_snapshot(dir.path(), snapshot, CatalogSourceContext::Project);
    assert_eq!(resolved.catalog.len(), 8);
    assert!(resolved
        .warnings
        .iter()
        .any(|w| w.code == "projectInvalid" && w.reason == "project read denied"));
    assert!(resolved.unavailable.is_none());
    assert_eq!(resolved.catalog, baseline.catalog);
    assert!(resolved.warnings.iter().any(|w| w.code == "projectInvalid"
        && w.path == project_catalog_path(dir.path()).display().to_string()
        && w.reason == "project read denied"));
    for _ in 0..3 {
        let snapshot = read_catalog_snapshot_with(
            dir.path(),
            CatalogReadScope::Project,
            |path, description| {
                if path == project_catalog_path(dir.path()) {
                    Err("project read denied".into())
                } else {
                    read_optional_regular_file(path, description)
                }
            },
        )
        .unwrap();
        let repeated =
            resolve_catalog_snapshot(dir.path(), snapshot, CatalogSourceContext::Project);
        assert_eq!(repeated.catalog, resolved.catalog);
        assert_eq!(repeated.warnings, resolved.warnings);
        assert_eq!(repeated.unavailable, resolved.unavailable);
        assert_eq!(
            repeated.base_verified_managed,
            resolved.base_verified_managed
        );
    }
    assert_eq!(inventory(dir.path()), before);
}
#[test]
fn project_symlink_and_dangling_link_remain_invalid_and_preserved() {
    for present in [true, false] {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let target = dir.path().join("project-target.json");
        if present {
            std::fs::write(&target, LOCAL_STUB_BYTES).unwrap();
        }
        let path = project_catalog_path(dir.path());
        create_manifest_symlink(&target, &path).expect("real project symlink fixture");
        let report = project_report(dir.path());
        assert_eq!(report.catalog.len(), 8);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.code == "projectInvalid" && w.path == path.display().to_string()));
        ensure_seeded(dir.path(), None);
        assert_eq!(std::fs::read_link(path).unwrap(), target);
        if present {
            assert_eq!(std::fs::read(target).unwrap(), LOCAL_STUB_BYTES);
        } else {
            assert!(!target.exists());
        }
    }
}

#[test]
pub(crate) fn project_ownership_support_and_unavailable_base_contract() {
    let dir = seed_dir();
    for owner in ["legacy", "foreign"] {
        write_legacy_base(
            dir.path(),
            &manifest_json(&serde_json::json!([extra_row("team")]).to_string()),
        );
        if owner == "foreign" {
            write_managed_base(dir.path(), &[extra_row("team")], "stale", true);
            let mut base = base_json(dir.path());
            base["managed"]["owner"] = serde_json::json!("other");
            std::fs::write(manifest_path(dir.path()), base.to_string()).unwrap();
        }
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([{"key":"team","label":"ignored"}])),
        );
        let report = project_report(dir.path());
        assert_ne!(report.catalog[0].label, "ignored");
        assert!(report.warnings.iter().any(|w| w.code == "migrationPending"
            && w.path == project_catalog_path(dir.path()).display().to_string()));
        std::fs::write(project_catalog_path(dir.path()), "bad").unwrap();
        assert!(project_report(dir.path())
            .warnings
            .iter()
            .any(|w| w.code == "projectInvalid"));
    }
    std::fs::remove_file(manifest_path(dir.path())).unwrap();
    let report = project_report(dir.path());
    assert!(report.unavailable.is_some());
    assert!(report.catalog.is_empty());
    std::fs::write(manifest_path(dir.path()), "invalid").unwrap();
    let report = project_report(dir.path());
    assert!(report.unavailable.is_some());
    assert!(report.warnings.is_empty());
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        write_managed_base(dir.path(), &[extra_row("team")], "stale", false);
        write_project_patch(
            dir.path(),
            patch_rows(serde_json::json!([extra_row("muse")])),
        );
        write_local(
            dir.path(),
            &patch_rows(serde_json::json!([{"key":"muse","label":"personal"}])).to_string(),
        );
        let report = project_report(dir.path());
        assert!(!report.catalog.iter().any(|d| d.key == "muse"));
        assert!(!report
            .warnings
            .iter()
            .any(|w| w.code == "localInvalid" || w.code == "projectInvalid"));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.code == "managedBaseEdited"));
    });
}

#[test]
fn project_interrupted_instance_journal_fallback_consumes_neither_patch() {
    let dir = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(
        legacy.path().join("agents.json"),
        manifest_json(&serde_json::json!([extra_row("mine")]).to_string()),
    )
    .unwrap();
    std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
    write_project_patch(
        dir.path(),
        patch_rows(serde_json::json!([{"key":"mine","label":"Project"}])),
    );
    with_failure_at("after_journal", || {
        ensure_seeded(dir.path(), Some(legacy.path()))
    });
    assert!(!manifest_path(dir.path()).exists());
    // The early legacy-source return ignores even malformed patches.
    write_local(dir.path(), "invalid personal");
    let report = project_report(dir.path());
    assert_eq!(keys_of(&report.catalog), ["mine"]);
    assert_ne!(report.catalog[0].label, "Project");
    assert!(!report
        .warnings
        .iter()
        .any(|w| w.code == "projectInvalid" || w.code == "localInvalid"));
}

#[test]
fn project_bytes_and_registered_snapshot_survive_initialization_refresh_recovery() {
    for contents in [LOCAL_STUB_BYTES, b"<<<<<<< malformed project".as_slice()] {
        let dir = seed_dir();
        std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
        std::fs::write(project_catalog_path(dir.path()), contents).unwrap();
        let registered = dir.path().join("agents.30.instance.no-git.json");
        let registered_bytes = b"{\"agents\":[{\"id\":\"registered-snapshot\"}]}";
        std::fs::write(&registered, registered_bytes).unwrap();
        ensure_seeded(dir.path(), None);
        reseed_master_for_command(dir.path(), "claude").unwrap();
        assert_eq!(
            std::fs::read(local_catalog_path(dir.path())).unwrap(),
            LOCAL_STUB_BYTES
        );
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            build_managed_base_bytes(&supported_shipped_definitions())
        );
        write_managed_base(dir.path(), &shipped_def_json(&["claude"]), "stale", true);
        ensure_seeded(dir.path(), None);
        assert_eq!(
            std::fs::read(project_catalog_path(dir.path())).unwrap(),
            contents
        );
        std::fs::remove_file(local_catalog_path(dir.path())).unwrap();
        // Recovery returns the saved legacy vector before either layer.
        write_legacy_base(
            dir.path(),
            &manifest_json(&serde_json::json!([extra_row("mine")]).to_string()),
        );
        with_failure_at("after_journal", || ensure_seeded(dir.path(), None));
        assert_eq!(keys_of(&project_report(dir.path()).catalog), ["mine"]);
        ensure_seeded(dir.path(), None);
        assert_eq!(
            std::fs::read(project_catalog_path(dir.path())).unwrap(),
            contents
        );
        std::fs::write(manifest_path(dir.path()), "invalid base").unwrap();
        ensure_seeded(dir.path(), None);
        project_report(dir.path());
        assert_eq!(
            std::fs::read(project_catalog_path(dir.path())).unwrap(),
            contents
        );
        assert_eq!(std::fs::read(&registered).unwrap(), registered_bytes);
    }
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    assert!(!project_catalog_path(dir.path()).exists());
}
