use super::*;

#[test]
fn catalog_report_serializes_exact_camel_case_wire_shape() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]}]"##,
        )),
    );
    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty());
    let value = serde_json::to_value(&report).expect("report value");
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "catalog",
            "primaryProjectRoot",
            "sourcePath",
            "unavailable",
            "warnings"
        ]
    );
    assert_eq!(value["primaryProjectRoot"], serde_json::Value::Null);
    assert_eq!(value["unavailable"], serde_json::Value::Null);
    assert_eq!(
        value["sourcePath"],
        serde_json::json!(manifest_path(dir.path()).display().to_string())
    );
    assert_eq!(value["catalog"][0]["key"], "mine");
    assert_eq!(value["catalog"][0]["updateCommands"], serde_json::json!([]));
    let back: CatalogReport = serde_json::from_value(value).expect("round trip");
    assert_eq!(back, report);
}

#[test]
fn catalog_report_missing_manifest_is_unavailable_without_creating_anything() {
    let dir = seed_dir();
    let before: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let expected = manifest_path(dir.path()).display().to_string();
    let report = report_for(dir.path());
    assert!(report.catalog.is_empty());
    assert_eq!(report.source_path.as_deref(), Some(expected.as_str()));
    let unavailable = report.unavailable.as_ref().expect("unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(unavailable.path, expected);
    // Nothing was created, not even the catalog directory.
    assert!(!catalog_dir(dir.path()).exists());
    let after: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn catalog_report_unreadable_path_is_unavailable() {
    let dir = seed_dir();
    // A regular file where the catalog DIRECTORY should be makes the
    // manifest path unreadable rather than merely missing.
    std::fs::write(dir.path().join("coding-agents"), b"not a directory").unwrap();
    let report = report_for(dir.path());
    assert!(report.catalog.is_empty());
    let unavailable = report.unavailable.as_ref().expect("unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(dir.path()).display().to_string()
    );
}

#[test]
fn catalog_report_corrupt_json_is_base_invalid_and_preserves_bytes() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    let garbage = b"{ this is not valid json";
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, garbage).unwrap();

    let report = report_for(dir.path());
    assert!(report.catalog.is_empty());
    let unavailable = report.unavailable.as_ref().expect("unavailable");
    assert_eq!(unavailable.code, "baseInvalid");
    assert_eq!(unavailable.path, path.display().to_string());
    assert_eq!(std::fs::read(&path).unwrap(), garbage);
}

#[test]
fn catalog_report_directory_at_manifest_is_unavailable() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(&path).unwrap();
    let report = report_for(dir.path());
    assert!(report.catalog.is_empty());
    assert_eq!(
        report.unavailable.as_ref().expect("unavailable").code,
        "baseUnavailable"
    );
}

#[test]
fn catalog_report_symlink_manifest_is_unavailable() {
    let dir = seed_dir();
    let target = dir.path().join("real-catalog.json");
    std::fs::write(
        &target,
        manifest_json(concat!(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]}]"##,
        )),
    )
    .unwrap();
    let link = manifest_path(dir.path());
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    if let Err(e) = create_manifest_symlink(&target, &link) {
        // Explicit, never silent: the link fixture could not be created on
        // this host (Windows symlink privilege), so the link assertion was
        // NOT exercised and must not be counted as a pass.
        eprintln!(
            "[catalog-report test] symlink creation unavailable on this host ({e}); link fixture \
                NOT exercised"
        );
        return;
    }
    let report = report_for(dir.path());
    assert!(report.catalog.is_empty());
    let unavailable = report.unavailable.as_ref().expect("unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert!(
        unavailable.reason.contains("symbolic link"),
        "{}",
        unavailable.reason
    );
}

#[test]
fn catalog_report_unsupported_schema_is_base_invalid() {
    for raw in [
        r##"{"schemaVersion":999,"agents":[]}"##,
        r##"{"schemaVersion":"1","agents":[]}"##,
        r##"{"schemaVersion":null,"agents":[]}"##,
    ] {
        let dir = seed_dir();
        write_report_manifest(dir.path(), raw);
        let report = report_for(dir.path());
        assert!(report.catalog.is_empty(), "raw={raw}");
        let unavailable = report.unavailable.as_ref().expect("unavailable");
        assert_eq!(unavailable.code, "baseInvalid", "raw={raw}");
        assert!(
            unavailable.reason.contains("schemaVersion"),
            "raw={raw}: {}",
            unavailable.reason
        );
    }
}

#[test]
fn catalog_report_missing_schema_version_and_empty_catalog_are_success() {
    for raw in [
        r##"{}"##,
        r##"{"schemaVersion":1}"##,
        r##"{"schemaVersion":1,"agents":[]}"##,
    ] {
        let dir = seed_dir();
        write_report_manifest(dir.path(), raw);
        let report = report_for(dir.path());
        assert!(report.unavailable.is_none(), "raw={raw}");
        assert!(report.catalog.is_empty(), "raw={raw}");
        assert!(report.warnings.is_empty(), "raw={raw}");
        assert_eq!(
            std::fs::read(manifest_path(dir.path())).unwrap(),
            raw.as_bytes()
        );
    }
}

#[test]
fn catalog_report_invalid_root_shape_is_base_invalid() {
    for raw in [
        r##"[]"##,
        r##""text""##,
        r##"{"schemaVersion":1,"agents":{}}"##,
        r##"{"schemaVersion":1,"agents":"nope"}"##,
    ] {
        let dir = seed_dir();
        write_report_manifest(dir.path(), raw);
        let report = report_for(dir.path());
        assert!(report.catalog.is_empty(), "raw={raw}");
        assert_eq!(
            report.unavailable.as_ref().expect("unavailable").code,
            "baseInvalid",
            "raw={raw}"
        );
    }
}

#[test]
fn catalog_report_missing_update_commands_warns_without_donor() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","##,
            r##""command":"claude","envs":[],"isolatedHome":false,"removable":true},
"##,
            r##"             {"key":"mine","label":"Mine","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true},
"##,
            r##"             {"key":"claude-beta","label":"Claude Beta","description":"d","##,
            r##""color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"##,
            r##""removable":true}]"##,
        )),
    );
    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(
        report
            .catalog
            .iter()
            .map(|d| d.key.as_str())
            .collect::<Vec<_>>(),
        ["claude", "mine", "claude-beta"]
    );
    assert!(report.catalog.iter().all(|d| d.update_commands.is_empty()));
    assert_eq!(report.warnings.len(), 3);
    let source = report.source_path.clone().expect("source path");
    for (row, warning) in report.catalog.iter().zip(report.warnings.iter()) {
        assert_eq!(warning.code, "migrationPending");
        assert_eq!(warning.path, source);
        assert!(warning.reason.contains(&row.key), "{}", warning.reason);
        assert!(
            warning.reason.contains("suppressed during reads"),
            "{}",
            warning.reason
        );
        assert!(
            warning.reason.contains("managed-catalog migration"),
            "{}",
            warning.reason
        );
    }
    // No embedded command may leak into a persisted-only report.
    let text = report_json(&report);
    for sentinel in [
        "claude --update",
        "pi update",
        "codex update",
        "hermes update --yes",
        "opencode upgrade",
        "agy update",
    ] {
        assert!(
            !text.contains(sentinel),
            "embedded command leaked into the report: {sentinel}"
        );
    }
}

#[test]
fn catalog_report_explicit_empty_has_no_missing_commands_warning() {
    let missing = manifest_json(concat!(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"         {"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true},
"##,
        r##"         {"key":"claude-beta","label":"Claude Beta","description":"d","##,
        r##""color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"##,
        r##""removable":true}]"##,
    ));
    let explicit = manifest_json(concat!(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","##,
        r##""command":"claude","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":[]},
"##,
        r##"         {"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":[]},
"##,
        r##"         {"key":"claude-beta","label":"Claude Beta","description":"d","##,
        r##""color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"##,
        r##""removable":true,"updateCommands":[]}]"##,
    ));
    let missing_dir = seed_dir();
    write_report_manifest(missing_dir.path(), &missing);
    let reported_missing = report_for(missing_dir.path());
    let explicit_dir = seed_dir();
    write_report_manifest(explicit_dir.path(), &explicit);
    let reported_explicit = report_for(explicit_dir.path());

    // Missing versus []: the resolved catalogs are identical; only the
    // missing field emits migrationPending.
    assert_eq!(reported_missing.catalog, reported_explicit.catalog);
    assert_eq!(reported_missing.warnings.len(), 3);
    assert!(reported_explicit.warnings.is_empty());
    assert!(reported_explicit
        .catalog
        .iter()
        .all(|d| d.update_commands.is_empty()));
    assert!(report_json(&reported_explicit).contains(r##""updateCommands":[]"##));

    // Custom and changed commands are preserved exactly, warn-free.
    let custom_dir = seed_dir();
    write_report_manifest(
        custom_dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["mytool up --channel beta"]},
"##,
            r##"             {"key":"claude-beta","label":"Claude Beta","description":"d","##,
            r##""color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":["claude update"]}]"##,
        )),
    );
    let reported_custom = report_for(custom_dir.path());
    assert!(reported_custom.warnings.is_empty());
    assert_eq!(
        reported_custom.catalog[0].update_commands,
        vec!["mytool up --channel beta".to_string()]
    );
    assert_eq!(
        reported_custom.catalog[1].update_commands,
        vec!["claude update".to_string()]
    );
    // Serialized transport parity: the wire value round-trips unchanged.
    let value = serde_json::to_value(&reported_custom).expect("report value");
    let back: CatalogReport = serde_json::from_value(value).expect("round trip");
    assert_eq!(back, reported_custom);
}

#[test]
fn catalog_report_duplicate_keys_warn_first_wins() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"mine","label":"First","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]},
"##,
            r##"             {"key":"mine","label":"Second","description":"d","##,
            r##""color":"#222","command":"mytool2","envs":[],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":[]}]"##,
        )),
    );
    let report = report_for(dir.path());
    assert_eq!(report.catalog.len(), 1);
    assert_eq!(report.catalog[0].label, "First");
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].code, "duplicateKey");
    assert!(
        report.warnings[0].reason.contains("first entry wins"),
        "{}",
        report.warnings[0].reason
    );
}

#[test]
fn catalog_report_invalid_definitions_drop_rows_without_echoing_commands() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"BAD KEY","label":"x","description":"d","color":"#000","##,
            r##""command":"x","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]},
"##,
            r##"             {"key":"cmd-continue","label":"x","description":"d","##,
            r##""color":"#000","command":"claude --continue","envs":[],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":[]},
"##,
            r##"             {"key":"nonstring","label":"x","description":"d","##,
            r##""color":"#000","command":"x","envs":[],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":["ok",123]},
"##,
            r##"             {"key":"blank","label":"x","description":"d","color":"#000","##,
            r##""command":"x","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["   "]},
"##,
            r##"             {"key":"ctrl","label":"x","description":"d","color":"#000","##,
            r##""command":"x","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["a\u0007b"]},
"##,
            r##"             {"key":"line-sep","label":"x","description":"d","color":"#000","##,
            r##""command":"x","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["a\u2028b"]},
"##,
            r##"             {"key":"para-sep","label":"x","description":"d","color":"#000","##,
            r##""command":"x","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["a\u2029b"]},
"##,
            r##"             {"key":"full","label":"Full","description":"d","color":"#333","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":["mytool --flag 'a b' && echo done"]}]"##,
        )),
    );
    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1);
    assert_eq!(report.catalog[0].key, "full");
    assert_eq!(
        report.catalog[0].update_commands,
        vec!["mytool --flag 'a b' && echo done".to_string()]
    );
    assert_eq!(report.warnings.len(), 7);
    assert!(report
        .warnings
        .iter()
        .all(|w| w.code == "invalidDefinition"));
    let text = report_json(&report);
    for leaked in [
        "BAD KEY",
        "claude --continue",
        "a\u{7}b",
        "a\u{2028}b",
        "a\u{2029}b",
    ] {
        assert!(
            !text.contains(leaked),
            "offending text leaked into the report: {leaked:?}"
        );
    }
    // The valid row's legal full command is present, preserved exactly.
    assert!(text.contains("mytool --flag 'a b' && echo done"));
}

#[test]
fn catalog_report_unknown_fields_warn_separately_without_values() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        concat!(
            r##"{"schemaVersion":1,"owner":"secret-root-value","agents":[
"##,
            r##"            {"key":"mine","label":"Mine","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[{"key":"GOOD_KEY","value":"secret-env-value","##,
            r##""enabled":true,"mystery":"secret-env-extra"}],"isolatedHome":false,"##,
            r##""removable":true,"updateCommands":[],"customField":"secret-row-value","##,
            r##""configSeed":{"enabled":true,"dest":".mine","extra":"secret-seed-value"}}]}"##,
        ),
    );
    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1);
    assert_eq!(report.catalog[0].envs.len(), 1);
    assert_eq!(report.catalog[0].envs[0].value, "secret-env-value");
    let codes: Vec<&str> = report.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.iter().all(|c| *c == "migrationPending"), "{codes:?}");
    assert_eq!(
        codes.len(),
        4,
        "{:?}",
        report
            .warnings
            .iter()
            .map(|w| &w.reason)
            .collect::<Vec<_>>()
    );
    let joined = report
        .warnings
        .iter()
        .map(|w| w.reason.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("owner"), "{joined}");
    assert!(joined.contains("customField"), "{joined}");
    assert!(joined.contains("extra"), "{joined}");
    assert!(joined.contains("mystery"), "{joined}");
    let text = report_json(&report);
    for secret in [
        "secret-root-value",
        "secret-row-value",
        "secret-seed-value",
        "secret-env-value",
        "secret-env-extra",
    ] {
        // The env VALUE is legitimately part of the resolved catalog; it must
        // never appear in a DIAGNOSTIC (no paths, no values in reasons).
        assert!(
            !joined.contains(secret),
            "value leaked into a diagnostic: {secret}"
        );
    }
    for unknown_value in [
        "secret-root-value",
        "secret-row-value",
        "secret-seed-value",
        "secret-env-extra",
    ] {
        assert!(
            !text.contains(unknown_value),
            "unknown-field value leaked into the report: {unknown_value}"
        );
    }
}

#[test]
fn catalog_report_two_project_isolation_and_no_fallback_after_failure() {
    let primary = seed_dir();
    let secondary = seed_dir();
    write_report_manifest(
        &primary.path().join(".ac"),
        &manifest_json(concat!(
            r##"[{"key":"primary","label":"Primary","description":"d","color":"#111","##,
            r##""command":"primary","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]}]"##,
        )),
    );
    write_report_manifest(
        &secondary.path().join(".ac"),
        &manifest_json(concat!(
            r##"[{"key":"secondary","label":"Secondary","description":"d","color":"#222","##,
            r##""command":"secondary","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]}]"##,
        )),
    );
    let primary_str = primary.path().to_string_lossy().to_string();
    let settings = AppSettings {
        project_paths: vec![
            primary_str.clone(),
            secondary.path().to_string_lossy().to_string(),
        ],
        ..AppSettings::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert_eq!(
        report.primary_project_root.as_deref(),
        Some(primary_str.as_str())
    );
    assert_eq!(
        report.source_path.as_deref(),
        Some(
            manifest_path(&primary.path().join(".ac"))
                .display()
                .to_string()
                .as_str()
        )
    );
    assert_eq!(
        report
            .catalog
            .iter()
            .map(|d| d.key.as_str())
            .collect::<Vec<_>>(),
        ["primary"]
    );
    assert!(!report_json(&report).contains("secondary"));

    // A failure in the primary project is never retried against the second.
    std::fs::remove_file(manifest_path(&primary.path().join(".ac"))).unwrap();
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert!(report.catalog.is_empty());
    assert_eq!(
        report.unavailable.as_ref().expect("unavailable").code,
        "baseUnavailable"
    );
    assert!(!report_json(&report).contains("secondary"));
}

#[test]
fn catalog_report_project_path_fallback_selects_trimmed_legacy_root() {
    let dir = seed_dir();
    write_report_manifest(
        &dir.path().join(".ac"),
        &manifest_json(concat!(
            r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","##,
            r##""command":"legacy","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]}]"##,
        )),
    );
    let trimmed = dir.path().to_string_lossy().to_string();
    let settings = AppSettings {
        project_paths: vec!["   ".to_string()],
        project_path: Some(format!("  {}  ", dir.path().display())),
        ..AppSettings::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert_eq!(
        report.primary_project_root.as_deref(),
        Some(trimmed.as_str())
    );
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog[0].key, "legacy");
}

#[test]
fn catalog_report_no_project_instance_read_and_absent_creates_nothing() {
    // Existing instance catalog: allowed and read-only.
    let instance = seed_dir();
    write_report_manifest(
        instance.path(),
        &manifest_json(concat!(
            r##"[{"key":"instance","label":"Instance","description":"d","color":"#111","##,
            r##""command":"instance","envs":[],"isolatedHome":false,"removable":true,"##,
            r##""updateCommands":[]}]"##,
        )),
    );
    let expected_source = manifest_path(instance.path()).display().to_string();
    let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();
    let settings = AppSettings::default();
    let report = load_catalog_report_for_settings_with_config_dir(
        &settings,
        Some(instance.path().to_path_buf()),
    );
    assert_eq!(report.primary_project_root, None);
    assert_eq!(
        report.source_path.as_deref(),
        Some(expected_source.as_str())
    );
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog[0].key, "instance");
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        bytes_before
    );

    // Absent instance catalog: unavailable, and repeated reads create
    // NOTHING on disk.
    let absent = seed_dir();
    let before: Vec<_> = std::fs::read_dir(absent.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    for _ in 0..3 {
        let report = load_catalog_report_for_settings_with_config_dir(
            &settings,
            Some(absent.path().to_path_buf()),
        );
        assert!(report.catalog.is_empty());
        assert_eq!(
            report.unavailable.as_ref().expect("unavailable").code,
            "baseUnavailable"
        );
    }
    assert!(!catalog_dir(absent.path()).exists());
    let after: Vec<_> = std::fs::read_dir(absent.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn catalog_report_config_dir_none_has_null_source_path() {
    let report = load_catalog_report_for_settings_with_config_dir(&AppSettings::default(), None);
    assert_eq!(report.primary_project_root, None);
    assert_eq!(report.source_path, None);
    assert!(report.catalog.is_empty());
    assert!(report.warnings.is_empty());
    let unavailable = report.unavailable.as_ref().expect("unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert!(unavailable.path.is_empty());
    let value = serde_json::to_value(&report).expect("report value");
    assert_eq!(value["sourcePath"], serde_json::Value::Null);
    assert_eq!(value["primaryProjectRoot"], serde_json::Value::Null);
}

#[test]
fn catalog_report_repeated_reads_leave_bytes_and_entries_unchanged() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(concat!(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","##,
            r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true},
"##,
            r##"             {"key":"mine2","label":"Mine2","description":"d","##,
            r##""color":"#222","command":"mytool2","envs":[],"isolatedHome":false,"##,
            r##""removable":true}]"##,
        )),
    );
    let path = manifest_path(dir.path());
    let bytes_before = std::fs::read(&path).unwrap();
    let mut entries_before: Vec<_> = std::fs::read_dir(catalog_dir(dir.path()))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    entries_before.sort();
    for _ in 0..3 {
        let report = report_for(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 2);
        assert_eq!(report.warnings.len(), 2);
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
    let mut entries_after: Vec<_> = std::fs::read_dir(catalog_dir(dir.path()))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    entries_after.sort();
    assert_eq!(entries_before, entries_after);
}

#[test]
fn catalog_report_desupported_builtin_omitted_with_diagnostic() {
    let manifest = manifest_json(concat!(
        r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","##,
        r##""command":"muse","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":[]},
"##,
        r##"         {"key":"mine","label":"Mine","description":"d","color":"#111","##,
        r##""command":"mytool","envs":[],"isolatedHome":false,"removable":true,"##,
        r##""updateCommands":[]}]"##,
    ));
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        write_report_manifest(dir.path(), &manifest);
        let report = report_for(dir.path());
        assert_eq!(
            report
                .catalog
                .iter()
                .map(|d| d.key.as_str())
                .collect::<Vec<_>>(),
            ["mine"]
        );
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].code, "invalidDefinition");
        assert!(
            report.warnings[0]
                .reason
                .contains("not supported by this build"),
            "{}",
            report.warnings[0].reason
        );
    });
}
