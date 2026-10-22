use super::*;

mod e2432_catalog_profile_tests;
mod e2432_p02_catalog_publication_tests;
mod identity_schema2_tests;
mod p02_catalog_schema_tests;

fn write_project_patch(ac_dir: &Path, value: serde_json::Value) {
    let path = project_catalog_path(ac_dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn project_report(ac_dir: &Path) -> CatalogReport {
    load_catalog_report_with_context(
        ac_dir,
        CatalogSourceContext::Project,
        CatalogReadScope::Project,
    )
    .0
}

fn patch_rows(rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"schemaVersion":1,"agents":rows})
}

fn extra_row(key: &str) -> serde_json::Value {
    let mut row = shipped_def_json(&["claude"]).remove(0);
    row["key"] = serde_json::json!(key);
    row["removable"] = serde_json::json!(true);
    row
}

#[test]
fn project_absence_preserves_direct_report_and_catalog() {
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
fn project_then_personal_merges_fields_nulls_and_exact_order() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude", "codex", "pi"]),
        "stale",
        true,
    );
    let mut project = patch_rows(serde_json::json!([
        {"key":"claude","label":"Project","instructionsFilename":null,"idleBurst":{"maxBytes":10,"maxSecs":2},"installCommands":{"default":"install","windows":"win"}}, extra_row("team")
    ]));
    project["order"] = serde_json::json!(["team", "pi", "unknown"]);
    write_project_patch(dir.path(), project);
    let mut personal = patch_rows(serde_json::json!([
        {"key":"claude","label":"Personal","idleBurst":{"maxSecs":3},"installCommands":{"windows":null}}, extra_row("mine")
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
fn project_invalid_schema_preserves_base_and_independent_personal() {
    let cases = [
        "<<<<<<< conflict",
        "{",
        r#"{"schemaVersion":999,"agents":[]}"#,
        r#"{"schemaVersion":1,"agents":[],"unexpected":true}"#,
        r#"{"schemaVersion":1,"schemaVersion":1,"agents":[]}"#,
        r#"{"schemaVersion":1,"agents":[{"key":"claude"},{"key":"claude"}]}"#,
        r#"{"schemaVersion":1,"agents":[],"order":["claude","claude"]}"#,
        r#"{"schemaVersion":1,"agents":[{"key":"claude","label":"partial"},{"key":"new","label":"incomplete"}]}"#,
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
fn project_rejection_has_no_donor_and_personal_rejection_keeps_project() {
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
fn project_ownership_support_and_unavailable_base_contract() {
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

/// (key, label, description, color, command, instructionsFilename, seed dest)
/// for the nine current presets. The embedded default must match these
/// values. The frontend keeps a parallel `FALLBACK_CODING_AGENTS` test (E7).
#[allow(clippy::type_complexity)]
const EXPECTED_PRESETS: [(&str, &str, &str, &str, &str, Option<&str>, Option<&str>); 9] = [
    (
        "claude",
        "Claude Code",
        "Coding Agent by Anthropic",
        "#d97706",
        "claude",
        Some("CLAUDE.md"),
        Some(".claude"),
    ),
    (
        "codex",
        "Codex",
        "Coding Agent by OpenAI",
        "#10b981",
        "codex",
        Some("AGENTS.md"),
        Some(".codex"),
    ),
    (
        "hermes",
        "Hermes",
        "Coding Agent by Nous Research",
        "#8b5cf6",
        "hermes",
        Some("AGENTS.md"),
        None,
    ),
    (
        "cursor",
        "Cursor CLI",
        "Coding Agent by Cursor",
        "#22d3ee",
        "agent",
        Some("AGENTS.md"),
        None,
    ),
    (
        "pi",
        "Pi",
        "Coding Agent by Earendil Inc",
        "#ec4899",
        "pi",
        Some("AGENTS.md"),
        None,
    ),
    (
        "opencode",
        "OpenCode",
        "Open-source terminal coding agent by Anomaly",
        "#64748b",
        "opencode",
        Some("AGENTS.md"),
        Some(".opencode"),
    ),
    (
        "antigravity",
        "Antigravity",
        "Coding Agent by Google",
        "#4285F4",
        "agy",
        Some("AGENTS.md"),
        None,
    ),
    (
        "grok",
        "Grok Build",
        "Coding Agent by SpaceXAI",
        "#64748b",
        "grok",
        Some("AGENTS.md"),
        None,
    ),
    (
        "muse",
        "Muse Code",
        "Meta terminal coding agent (beta; macOS/Linux host only)",
        "#0668E1",
        "muse",
        None,
        None,
    ),
];

fn manifest_json(agents_json: &str) -> String {
    format!("{{\"schemaVersion\":1,\"agents\":{agents_json}}}")
}

fn seed_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// #1912 - support-override test tables: all 9 rows spelled out, in the
/// shipped order, exactly one `false` each. Reaching the PRODUCTION
/// wrappers through `with_builtin_agent_support_for_test` is the point: the
/// controls below prove behavior on the real call chain, not on copies.
/// `TABLE_MUSE_OFF` mirrors the shipped table (#1999 disables muse); the
/// all-enabled control drives the opposite direction.
const TABLE_MUSE_OFF: &[(&str, bool)] = &[
    ("claude", true),
    ("codex", true),
    ("hermes", true),
    ("cursor", true),
    ("pi", true),
    ("opencode", true),
    ("antigravity", true),
    ("grok", true),
    ("muse", false),
];
const TABLE_CLAUDE_OFF: &[(&str, bool)] = &[
    ("claude", false),
    ("codex", true),
    ("hermes", true),
    ("cursor", true),
    ("pi", true),
    ("opencode", true),
    ("antigravity", true),
    ("grok", true),
    ("muse", true),
];
const TABLE_ALL_ENABLED: &[(&str, bool)] = &[
    ("claude", true),
    ("codex", true),
    ("hermes", true),
    ("cursor", true),
    ("pi", true),
    ("opencode", true),
    ("antigravity", true),
    ("grok", true),
    ("muse", true),
];

#[test]
fn embedded_default_parses_with_nine_agents_in_order() {
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
    let keys: Vec<&str> = catalog.agents.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "claude",
            "codex",
            "hermes",
            "cursor",
            "pi",
            "opencode",
            "antigravity",
            "grok",
            "muse"
        ]
    );
    // Muse stays last, now immediately preceded by Grok.
    let last = catalog.agents.last().unwrap();
    assert_eq!(last.key, "muse");
    assert_eq!(last.command, "muse");
}

/// #2736 P1 - the shipped `installCommands` pin, kept OUT of
/// `embedded_default_matches_current_presets_exactly` so that test stays
/// under the pinned cognitive complexity threshold of 25.
fn assert_shipped_install_commands_2736(key: &str, def: &CodingAgentDefinition) {
    match scope_b_expected_commands_2800(key) {
        Some(expected) => assert_eq!(
            serde_json::to_value(
                def.install_commands
                    .as_ref()
                    .expect("matrix row install commands")
            )
            .unwrap(),
            expected,
            "{key}"
        ),
        None => assert!(
            def.install_commands.is_none(),
            "{key} excluded from install matrix"
        ),
    }
}

#[test]
fn embedded_default_matches_current_presets_exactly() {
    // Drift guard for every current preset field.
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.agents.len(), EXPECTED_PRESETS.len());
    for (def, (key, label, desc, color, command, filename, seed_dest)) in
        catalog.agents.iter().zip(EXPECTED_PRESETS)
    {
        assert_eq!(def.key, key);
        assert_eq!(def.label, label);
        assert_eq!(def.description, desc);
        assert_eq!(def.color, color);
        assert_eq!(def.command, command);
        assert_eq!(def.instructions_filename.as_deref(), filename);
        // #769 P2: Claude/Codex/OpenCode ship an active configSeed; the other
        // six ship none (no master, no re-seed button).
        match seed_dest {
            Some(dest) => {
                let cs = def
                    .config_seed
                    .as_ref()
                    .unwrap_or_else(|| panic!("{key} must ship configSeed"));
                assert!(cs.enabled, "{key} configSeed must be enabled");
                assert_eq!(cs.dest, dest, "{key} configSeed dest");
            }
            None => assert!(
                def.config_seed.is_none(),
                "{key} must ship configSeed UNSET"
            ),
        }
        assert!(def.removable, "{key} must be removable");
        assert!(def.envs.is_empty());
        assert!(!def.isolated_home);
        // #2124 - every shipped row carries the conservative burst filter.
        let burst = def
            .idle_burst
            .as_ref()
            .unwrap_or_else(|| panic!("{key} must ship idleBurst"));
        assert_eq!(burst.max_bytes, Some(1024), "{key} idleBurst maxBytes");
        assert_eq!(burst.max_secs, Some(3.0), "{key} idleBurst maxSecs");
        assert_eq!(
            burst.prior_silence_secs,
            Some(60.0),
            "{key} idleBurst priorSilenceSecs"
        );
        assert_shipped_install_commands_2736(key, def);
    }
    let raw: serde_json::Value = serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
    assert_eq!(raw["schemaVersion"], 1);
    assert_eq!(
        raw["agents"].as_array().unwrap().last().unwrap(),
        &serde_json::json!({
            "key": "muse",
            "label": "Muse Code",
            "description": "Meta terminal coding agent (beta; macOS/Linux host only)",
            "color": "#0668E1",
            "baseCommand": "muse",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": {
                "maxBytes": 1024,
                "maxSecs": 3.0,
                "priorSilenceSecs": 60.0
            }
        })
    );
    assert_eq!(
        catalog
            .agents
            .iter()
            .filter(|def| def.config_seed.is_some())
            .count(),
        3
    );
    assert_eq!(
        catalog
            .agents
            .iter()
            .filter(|def| def.config_seed.is_none())
            .count(),
        6
    );
}

#[test]
fn every_embedded_entry_validates() {
    for def in embedded_default_catalog().agents {
        validate_definition(&def).unwrap_or_else(|e| panic!("{}: {e}", def.key));
    }
}

/// #2124 T9 - serde, strict local layer and legacy migration for `idleBurst`.
#[test]
fn idle_burst_serde_and_local_layer() {
    // A definition without `idleBurst` deserializes `None` and does not
    // re-emit the key.
    let mut plain = shipped_def_json(&["claude"]).remove(0);
    plain.as_object_mut().unwrap().remove("idleBurst");
    let definition: CodingAgentDefinition = serde_json::from_value(plain).unwrap();
    assert!(definition.idle_burst.is_none());
    assert!(!serde_json::to_string(&definition)
        .unwrap()
        .contains("idleBurst"));

    // The embedded default ships the filter on all nine rows.
    let embedded = embedded_default_catalog();
    assert_eq!(embedded.agents.len(), 9);
    for row in &embedded.agents {
        let burst = row
            .idle_burst
            .as_ref()
            .unwrap_or_else(|| panic!("{} must ship idleBurst", row.key));
        assert_eq!(burst.max_bytes, Some(1024), "{}", row.key);
        assert_eq!(burst.max_secs, Some(3.0), "{}", row.key);
        assert_eq!(burst.prior_silence_secs, Some(60.0), "{}", row.key);
    }

    // Compose a managed base (1024/3/60) with a local object patch: the
    // stated subfield replaces, the absent ones inherit.
    let dir = seed_dir();
    let revision = managed_content_sha256(&supported_shipped_definitions());
    write_managed_base(dir.path(), &shipped_def_json(&["claude"]), &revision, true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":2048}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let claude = report
        .catalog
        .iter()
        .find(|def| def.key == "claude")
        .unwrap();
    let burst = claude.idle_burst.as_ref().expect("composed idleBurst");
    assert_eq!(burst.max_bytes, Some(2048));
    assert_eq!(burst.max_secs, Some(3.0), "absent maxSecs inherits");
    assert_eq!(burst.prior_silence_secs, Some(60.0));

    // Local `null` clears the whole object (filter off).
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":null}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.catalog[0].idle_burst.is_none());

    // Unknown subfields and invalid value shapes disable the whole local
    // layer; the base entry stays intact.
    for local in [
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"foo":1}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":-1}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":"3"}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":-1}}]}"##,
    ] {
        write_local(dir.path(), local);
        let report = load_catalog_report(dir.path());
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "localInvalid"),
            "{local}: {:?}",
            report.warnings
        );
        let claude = report
            .catalog
            .iter()
            .find(|def| def.key == "claude")
            .unwrap();
        assert_eq!(
            claude.idle_burst.as_ref().unwrap().max_bytes,
            Some(1024),
            "the base entry stays intact after a rejected local layer"
        );
    }

    // A legacy row that states `idleBurst` explicitly pins it in the
    // extracted local layer, presence only.
    let mut legacy_claude = shipped_def_json(&["claude"]).remove(0);
    legacy_claude["idleBurst"] = serde_json::json!({"maxSecs": 2.0});
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [legacy_claude]}).to_string();
    let extracted = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
        .expect("legacy extraction with explicit idleBurst");
    let local: serde_json::Value = serde_json::from_slice(&extracted).unwrap();
    assert_eq!(
        local["agents"][0]["idleBurst"],
        serde_json::json!({"maxSecs": 2.0})
    );
}

/// #2736 - the shipped `installCommands` value for `key`, if any.
fn expected_install_2736(key: &str) -> Option<&'static str> {
    match key {
        "claude" | "codex" | "hermes" | "cursor" | "pi" | "opencode" | "antigravity" | "grok" => {
            Some("echo No verified installer for this platform 1>&2 && exit 1")
        }
        _ => None,
    }
}

/// #2736 - a managed base of the shipped claude row, current revision.
fn managed_claude_base_2736(ac_dir: &Path) {
    let revision = managed_content_sha256(&supported_shipped_definitions());
    write_managed_base(ac_dir, &shipped_def_json(&["claude"]), &revision, true);
}

fn claude_of(report: &CatalogReport) -> &CodingAgentDefinition {
    report
        .catalog
        .iter()
        .find(|def| def.key == "claude")
        .expect("claude entry")
}

#[test]
fn install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse() {
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.agents.len(), 9);
    for def in &catalog.agents {
        assert_shipped_install_commands_2736(&def.key, def);
        assert_eq!(
            def.install_commands.as_ref().map(|ic| ic.default.as_str()),
            expected_install_2736(&def.key)
        );
    }
    assert_eq!(
        catalog
            .agents
            .iter()
            .filter(|def| def.install_commands.is_some())
            .count(),
        8
    );
}

#[test]
fn install_commands_2736_optional_absent_deserializes_to_none() {
    let mut plain = shipped_def_json(&["claude"]).remove(0);
    plain.as_object_mut().unwrap().remove("installCommands");
    let definition: CodingAgentDefinition = serde_json::from_value(plain).unwrap();
    assert!(definition.install_commands.is_none());
    assert!(validate_definition(&definition).is_ok());
    assert!(!serde_json::to_string(&definition)
        .unwrap()
        .contains("installCommands"));
}

#[test]
fn install_commands_2736_rejects_a_blank_or_control_character_value() {
    let base: CodingAgentDefinition =
        serde_json::from_value(shipped_def_json(&["claude"]).remove(0)).unwrap();
    let cases = [
        (
            InstallCommands {
                default: "   ".to_string(),
                windows: None,
                macos: None,
                linux: None,
            },
            "   ",
        ),
        (
            InstallCommands {
                default: "npm i -g ok".to_string(),
                windows: Some("secretwin\u{7}cmd".to_string()),
                macos: None,
                linux: None,
            },
            "secretwin",
        ),
        (
            InstallCommands {
                default: "npm i -g ok".to_string(),
                windows: None,
                macos: Some("secretmac\u{2028}cmd".to_string()),
                linux: None,
            },
            "secretmac",
        ),
    ];
    for (install, secret) in cases {
        let mut def = base.clone();
        def.install_commands = Some(install);
        let error = validate_definition(&def).expect_err("must be rejected");
        assert!(error.contains("installCommands."), "{error}");
        if !secret.trim().is_empty() {
            assert!(!error.contains(secret), "error echoes the value: {error}");
        }
        let reason = definition_problem_reason(&def);
        assert!(
            !reason.contains(secret) || secret.trim().is_empty(),
            "{reason}"
        );
        let raw = serde_json::to_value(&def).unwrap();
        let problem = raw_install_commands_problem(&raw).expect("raw problem");
        if !secret.trim().is_empty() {
            assert!(!problem.contains(secret), "{problem}");
        }
    }
}

#[test]
fn install_commands_2736_requires_default_when_the_object_is_present() {
    let error = serde_json::from_value::<InstallCommands>(serde_json::json!({"windows": "x"}))
        .expect_err("default is required");
    assert!(
        error.to_string().contains("missing field `default`"),
        "{error}"
    );

    let mut bad = shipped_def_json(&["claude"]).remove(0);
    bad["installCommands"] = serde_json::json!({"windows": "x"});
    let good = shipped_def_json(&["codex"]).remove(0);
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(&serde_json::to_string(&vec![bad, good]).unwrap()),
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert!(report.catalog.iter().all(|def| def.key != "claude"));
    assert!(report.catalog.iter().any(|def| def.key == "codex"));
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "invalidDefinition"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn install_commands_2736_base_entry_with_a_non_object_value_is_omitted_with_a_warning() {
    let mut raw = shipped_def_json(&["claude"]).remove(0);
    raw["installCommands"] = serde_json::json!("npm install -g secret-pkg");
    let problem = raw_install_commands_problem(&raw).expect("non-object is a problem");
    assert!(!problem.contains("secret-pkg"), "{problem}");
    for value in [
        serde_json::json!({"default": 7}),
        serde_json::json!({"default": "ok", "linux": 7}),
        serde_json::json!({"default": "ok", "linux": "  "}),
    ] {
        raw["installCommands"] = value.clone();
        assert!(raw_install_commands_problem(&raw).is_some(), "{value}");
    }
    raw["installCommands"] = serde_json::json!({"default": "ok"});
    assert!(raw_install_commands_problem(&raw).is_none());

    let mut bad = shipped_def_json(&["claude"]).remove(0);
    bad["installCommands"] = serde_json::json!("npm install -g secret-pkg");
    let good = shipped_def_json(&["codex"]).remove(0);
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(&serde_json::to_string(&vec![bad, good]).unwrap()),
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert_eq!(report.catalog.len(), 1);
    assert_eq!(report.catalog[0].key, "codex");
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "invalidDefinition")
        .expect("invalidDefinition");
    assert!(
        warning.reason.contains("installCommands"),
        "{}",
        warning.reason
    );
    assert!(!warning.reason.contains("secret-pkg"), "{}", warning.reason);
}

#[test]
fn install_commands_2736_local_layer_patches_windows_and_inherits_default() {
    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":{"windows":"winget install claude"}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let ic = claude_of(&report)
        .install_commands
        .as_ref()
        .expect("composed");
    assert_eq!(ic.default, expected_install_2736("claude").unwrap());
    assert_eq!(ic.windows.as_deref(), Some("winget install claude"));
    let expected = scope_b_expected_commands_2800("claude").unwrap();
    assert_eq!(ic.macos.as_deref(), expected["macos"].as_str());
    assert_eq!(ic.linux.as_deref(), expected["linux"].as_str());
}

#[test]
fn install_commands_2736_local_null_clears_the_object() {
    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":null}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(claude_of(&report).install_commands.is_none());
}

#[test]
fn install_commands_2736_local_null_on_one_os_key_clears_only_that_key() {
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["installCommands"] = serde_json::json!({
        "default": "npm install -g @anthropic-ai/claude-code",
        "windows": "win cmd",
        "linux": "linux cmd"
    });
    write_managed_base(dir.path(), &[claude], "stale-revision", true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","installCommands":{"windows":null}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(
        report.warnings.iter().all(|w| w.code != "localInvalid"),
        "{:?}",
        report.warnings
    );
    let ic = claude_of(&report).install_commands.as_ref().expect("kept");
    assert_eq!(ic.default, "npm install -g @anthropic-ai/claude-code");
    assert!(ic.windows.is_none(), "windows cleared");
    assert_eq!(ic.linux.as_deref(), Some("linux cmd"), "linux inherited");
}

#[test]
fn install_commands_2736_one_bad_local_value_discards_the_entire_local_layer() {
    let dir = seed_dir();
    let revision = managed_content_sha256(&supported_shipped_definitions());
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude", "codex"]),
        &revision,
        true,
    );
    for local in [
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"windows":"  "}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"default":7}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":"npm i"}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"codex","label":"MINE"},{"key":"claude","installCommands":{"default":""}}]}"##,
    ] {
        write_local(dir.path(), local);
        let report = load_catalog_report(dir.path());
        assert!(
            report.warnings.iter().any(|w| w.code == "localInvalid"),
            "{local}: {:?}",
            report.warnings
        );
        let codex = report
            .catalog
            .iter()
            .find(|def| def.key == "codex")
            .unwrap();
        assert_eq!(
            codex.label, "Codex",
            "{local}: the valid row is NOT applied"
        );
        assert_eq!(
            claude_of(&report)
                .install_commands
                .as_ref()
                .unwrap()
                .default,
            expected_install_2736("claude").unwrap(),
            "{local}: the base entry stays intact"
        );
    }
}

#[test]
fn install_commands_2736_new_local_key_without_install_commands_is_accepted() {
    let fields = parse_local_fields(
        serde_json::json!({
            "key": "mine", "label": "Mine", "description": "d", "color": "#111",
            "command": "mytool", "envs": [], "isolatedHome": false,
            "removable": true, "updateCommands": [], "autoUpdate": false
        })
        .as_object()
        .unwrap(),
        "local agents[0]",
    )
    .expect("valid new row");
    let definition = build_new_definition("mine", &fields, LayerDescription::Local)
        .expect("not missing anything");
    assert!(definition.install_commands.is_none());

    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let mine = report.catalog.iter().find(|def| def.key == "mine").unwrap();
    assert!(mine.install_commands.is_none());
}

#[test]
fn install_commands_2736_unknown_nested_field_is_a_whole_layer_error() {
    let dir = seed_dir();
    managed_claude_base_2736(dir.path());
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MINE","installCommands":{"default":"x","freebsd":"y"}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(
        report.warnings.iter().any(|w| w.code == "localInvalid"),
        "{:?}",
        report.warnings
    );
    assert_eq!(claude_of(&report).label, "Claude Code");
}

#[test]
fn install_commands_2736_managed_base_refreshes_from_the_previous_revision() {
    let old: Vec<CodingAgentDefinition> = supported_shipped_definitions()
        .into_iter()
        .map(|mut def| {
            def.install_commands = None;
            def
        })
        .collect();
    let old_revision = managed_content_sha256(&old);
    let new_revision = managed_content_sha256(&supported_shipped_definitions());
    assert_ne!(old_revision, new_revision);
    let old_json: Vec<serde_json::Value> = old
        .iter()
        .map(|def| serde_json::to_value(def).unwrap())
        .collect();
    let dir = seed_dir();
    write_managed_base(dir.path(), &old_json, &old_revision, true);

    let report = load_catalog_report(dir.path());
    assert!(
        report.warnings.iter().any(|w| w.code == "refreshFailed"),
        "stale revision is reported: {:?}",
        report.warnings
    );
    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "stale base refreshes"
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        build_managed_base_bytes(&supported_shipped_definitions())
    );
    assert_eq!(base_json(dir.path())["managed"]["revision"], new_revision);
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        claude_of(&report)
            .install_commands
            .as_ref()
            .unwrap()
            .default,
        expected_install_2736("claude").unwrap()
    );
}

#[test]
fn install_commands_2736_edited_managed_base_is_not_refreshed() {
    let old: Vec<CodingAgentDefinition> = supported_shipped_definitions()
        .into_iter()
        .map(|mut def| {
            def.install_commands = None;
            def
        })
        .collect();
    let old_revision = managed_content_sha256(&old);
    let mut edited = old.clone();
    edited[0].color = "#d97707".to_string();
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": edited,
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": old_revision,
            "contentSha256": old_revision,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let report = load_catalog_report(dir.path());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "managedBaseEdited"),
        "{:?}",
        report.warnings
    );
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn install_commands_2736_unknown_field_on_an_older_build_reports_migration_pending() {
    assert!(!KNOWN_DEFINITION_FIELDS.contains(&"futureField2736"));
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["futureField2736"] = serde_json::json!({"keep": "me"});
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(&serde_json::to_string(&vec![claude]).unwrap()),
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert_eq!(report.catalog.len(), 1, "the entry survives");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "migrationPending" && w.reason.contains("futureField2736")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn install_commands_2736_pin_wire_keeps_presence_semantics() {
    let definition: CodingAgentDefinition =
        serde_json::from_value(shipped_def_json(&["claude"]).remove(0)).unwrap();
    let mut with_key = serde_json::Map::new();
    with_key.insert("key".to_string(), serde_json::json!("claude"));
    with_key.insert(
        "installCommands".to_string(),
        serde_json::to_value(definition.install_commands.as_ref().unwrap()).unwrap(),
    );
    let mut without_key = serde_json::Map::new();
    without_key.insert("key".to_string(), serde_json::json!("claude"));

    for wire in [
        pin_explicit_legacy_fields(&definition, &with_key),
        materialize_complete_legacy_fields(&definition, &with_key),
    ] {
        let value = serde_json::to_value(&wire).unwrap();
        assert_eq!(
            value["installCommands"],
            serde_json::to_value(definition.install_commands.as_ref().unwrap()).unwrap()
        );
    }
    for wire in [
        pin_explicit_legacy_fields(&definition, &without_key),
        materialize_complete_legacy_fields(&definition, &without_key),
    ] {
        let value = serde_json::to_value(&wire).unwrap();
        assert!(
            value.get("installCommands").is_none(),
            "absent legacy key is never pinned or forced: {value}"
        );
    }

    let mut legacy_claude = shipped_def_json(&["claude"]).remove(0);
    legacy_claude["installCommands"] = serde_json::json!({"default": "npm i -g mine"});
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [legacy_claude]}).to_string();
    let extracted = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
        .expect("legacy extraction with explicit installCommands");
    let local: serde_json::Value = serde_json::from_slice(&extracted).unwrap();
    assert_eq!(
        local["agents"][0]["installCommands"],
        serde_json::json!({"default": "npm i -g mine"})
    );

    let mut ambiguous = shipped_def_json(&["claude"]).remove(0);
    ambiguous["installCommands"] = serde_json::json!({"default": " "});
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [ambiguous]}).to_string();
    let error = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
        .expect_err("ambiguous installCommands refuses the migration");
    assert!(error.contains("installCommands"), "{error}");
}

#[test]
fn cursor_cli_command_is_agent() {
    let catalog = embedded_default_catalog();
    let cursor = catalog.agents.iter().find(|a| a.key == "cursor").unwrap();
    assert_eq!(cursor.command, "agent");
}

#[test]
fn validate_catalog_key_accepts_allowlist_and_rejects_others() {
    for ok in ["claude", "cursor-cli", "pi", "a1", "opencode", "x-2-y"] {
        assert!(validate_catalog_key(ok).is_ok(), "should accept {ok:?}");
    }
    for bad in [
        "",
        "Claude",
        "cursor_cli",
        "cursor cli",
        "café",
        "a.b",
        "UP",
        "a/b",
    ] {
        assert!(validate_catalog_key(bad).is_err(), "should reject {bad:?}");
    }
}

#[test]
fn load_missing_manifest_is_unavailable_without_seeding() {
    // #1967 P4: a missing persisted catalog is unavailable; the embedded
    // default is NOT substituted and nothing is created on disk.
    let dir = seed_dir();
    let unavailable = load_catalog(dir.path()).expect_err("absent catalog is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(dir.path()).display().to_string()
    );
    assert!(unavailable.reason.contains("no persisted catalog"));
    assert!(
        !catalog_dir(dir.path()).exists(),
        "a catalog read must never create the catalog directory"
    );
}

#[test]
fn load_corrupt_manifest_is_base_invalid_and_preserves_file() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let garbage = b"{ this is not valid json";
    std::fs::write(&path, garbage).unwrap();

    let unavailable = load_catalog(dir.path()).expect_err("corrupt catalog is unavailable");
    assert_eq!(unavailable.code, "baseInvalid");
    assert_eq!(unavailable.path, path.display().to_string());
    // G3: the corrupt file is preserved byte-for-byte, never overwritten.
    assert_eq!(std::fs::read(&path).unwrap(), garbage);
}

#[test]
fn load_skips_invalid_entry_and_keeps_valid_rest() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // First entry is valid; second has an illegal key (uppercase); third is
    // valid. Only the two valid entries survive.
    let agents = r##"[
        {"key":"claude","label":"Claude","description":"d","color":"#000","command":"claude","envs":[],"isolatedHome":false,"removable":true},
        {"key":"BAD KEY","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true},
        {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}
    ]"##;
    std::fs::write(&path, manifest_json(agents)).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    let keys: Vec<&str> = loaded.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["claude", "mine"]);
}

#[test]
fn load_dedups_duplicate_keys_first_wins() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let agents = r##"[
        {"key":"claude","label":"First","description":"d","color":"#000","command":"claude","envs":[],"isolatedHome":false,"removable":true},
        {"key":"claude","label":"Second","description":"d","color":"#000","command":"claude","envs":[],"isolatedHome":false,"removable":true}
    ]"##;
    std::fs::write(&path, manifest_json(agents)).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].label, "First");
}

#[test]
fn load_honors_valid_empty_agents_list() {
    // A user who removed every built-in has a valid, empty manifest. It is
    // honored verbatim; the embedded default is NOT resurrected.
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, manifest_json("[]")).unwrap();

    assert!(load_catalog(dir.path())
        .expect("persisted catalog")
        .is_empty());
}

#[test]
fn ensure_seeded_writes_when_absent_then_is_idempotent() {
    let dir = seed_dir();
    let path = manifest_path(dir.path());
    assert!(!path.exists());

    ensure_seeded(dir.path(), None);
    assert!(path.exists(), "seed writes the manifest when absent");
    assert_eq!(load_catalog(dir.path()).expect("seeded catalog").len(), 8);

    // Idempotent + never clobbers a user edit: hand-edit to a single custom
    // agent, re-seed, and confirm the edit is preserved.
    let custom = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    std::fs::write(&path, &custom).unwrap();
    ensure_seeded(dir.path(), None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].key, "mine");
}

#[test]
fn seeded_manifest_leaves_no_temp_residue() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let catalog = catalog_dir(dir.path());
    let residue: Vec<_> = std::fs::read_dir(&catalog)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.contains(".tmp"))
                .unwrap_or(false)
        })
        .collect();
    assert!(residue.is_empty(), "no leftover temp files after seed");
}

// ---- #769 Phase 2: masters + re-seed --------------------------------

fn master(cmd: &str) -> &'static EmbeddedSeedMaster {
    EMBEDDED_SEED_MASTERS
        .iter()
        .find(|m| m.command_basename == cmd)
        .unwrap()
}

#[test]
fn reseedable_commands_are_claude_codex_opencode() {
    let mut got = reseedable_command_basenames();
    got.sort();
    assert_eq!(got, vec!["claude", "codex", "opencode"]);
}

#[test]
fn every_embedded_master_maps_to_a_catalog_def_with_matching_configseed() {
    let catalog = embedded_default_catalog();
    for m in EMBEDDED_SEED_MASTERS {
        let def = catalog
            .agents
            .iter()
            .find(|d| {
                command_executable_basename(&d.command).as_deref() == Some(m.command_basename)
            })
            .unwrap_or_else(|| panic!("no catalog def for master {}", m.command_basename));
        let cs = def
            .config_seed
            .as_ref()
            .unwrap_or_else(|| panic!("{} def missing configSeed", m.command_basename));
        assert!(cs.enabled);
        assert_eq!(
            cs.dest, m.dest,
            "master dest must match the def configSeed dest"
        );
        assert_eq!(
            def.key, m.key,
            "#1912 master key must match the catalog def key it maps to"
        );
        assert!(
            !m.files.is_empty(),
            "master {} must ship >=1 file",
            m.command_basename
        );
    }
}

#[test]
fn ensure_seeded_masters_creates_nonempty_masters_and_preserves_edits() {
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    for cmd in ["claude", "codex", "opencode"] {
        let m = master(cmd);
        let md = master_dir_for_dest(dir.path(), m.dest);
        assert!(
            crate::config::config_seed::is_nonempty_seed_dir(&md),
            "{cmd} master should be non-empty"
        );
        assert_eq!(
            std::fs::read(md.join(m.files[0].rel_path)).unwrap(),
            m.files[0].bytes
        );
    }
    // Idempotent + never clobbers a user edit.
    let m = master("claude");
    let file = master_dir_for_dest(dir.path(), m.dest).join(m.files[0].rel_path);
    std::fs::write(&file, b"USER EDIT").unwrap();
    ensure_seeded_masters(dir.path(), None);
    assert_eq!(std::fs::read(&file).unwrap(), b"USER EDIT");
}

#[test]
fn reseed_installs_embedded_default_and_backs_up_current() {
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    let m = master("claude");
    let master_dir = master_dir_for_dest(dir.path(), m.dest);
    let file = master_dir.join(m.files[0].rel_path);
    std::fs::write(&file, b"USER EDITED").unwrap();

    let result = reseed_master_for_command(dir.path(), "claude").unwrap();
    assert_eq!(result.dest, ".claude");
    assert!(!result.backup_path.is_empty());
    // Master restored to the embedded default.
    assert_eq!(std::fs::read(&file).unwrap(), m.files[0].bytes);
    // The `.bak` holds the user's prior edit (verbatim).
    let bak_file = Path::new(&result.backup_path).join(m.files[0].rel_path);
    assert_eq!(std::fs::read(&bak_file).unwrap(), b"USER EDITED");
}

#[test]
fn reseed_when_master_absent_installs_without_backup() {
    let dir = seed_dir();
    // Masters not seeded: the _seed dir is absent.
    let m = master("codex");
    let master_dir = master_dir_for_dest(dir.path(), m.dest);
    assert!(!master_dir.exists());

    let result = reseed_master_for_command(dir.path(), "codex").unwrap();
    assert_eq!(result.dest, ".codex");
    assert!(
        result.backup_path.is_empty(),
        "no backup when master was absent"
    );
    assert_eq!(
        std::fs::read(master_dir.join(m.files[0].rel_path)).unwrap(),
        m.files[0].bytes
    );
}

#[test]
fn reseed_gating_is_exact_basename_never_startswith() {
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    // `pi` and `agent` are real built-in commands but ship NO master; `pip`
    // and `clau` must not match `pi`/`claude` via any prefix rule; empty and
    // unknown are rejected.
    for bad in ["pi", "agent", "pip", "clau", "claudex", "notacommand", ""] {
        assert!(
            reseed_master_for_command(dir.path(), bad).is_err(),
            "should reject {bad:?}"
        );
    }
    // Path/extension forms of a real master command still resolve by basename.
    for good in [
        "claude",
        "codex",
        "opencode",
        "claude.exe",
        "codex --model x",
    ] {
        assert!(
            reseed_master_for_command(dir.path(), good).is_ok(),
            "should accept {good:?}"
        );
    }
}

#[test]
fn master_dir_for_dest_is_under_seed_subdir() {
    let dir = seed_dir();
    let p = master_dir_for_dest(dir.path(), ".claude");
    assert_eq!(
        p,
        dir.path()
            .join("coding-agents")
            .join("_seed")
            .join(".claude")
    );
}

// ---- #1318 relocation, migration, per-project seeding ----------------

fn legacy_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A hand-authored legacy catalog (custom entry, byte-distinct from the
/// embedded default). The tempdir IS the legacy `<config_dir>/coding-agents`
/// directory.
fn legacy_catalog_json() -> Vec<u8> {
    manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    )
    .into_bytes()
}

#[test]
fn managed_catalog_migrates_instance_legacy_into_absent_project_base() {
    // #1968: an absent project base imports ONLY the instance agents.json.
    // The ownership transfer keeps the source bytes exactly in the
    // project-local backup, and the instance source itself is never written.
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_bytes = legacy_catalog_json();
    std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

    let published = ensure_seeded(project.path(), Some(legacy.path()));
    assert!(published.is_some(), "a first migration publishes the base");

    let catalog = catalog_dir(project.path());
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy_bytes,
        "the backup keeps the source bytes exactly"
    );
    let base: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path(project.path())).unwrap()).unwrap();
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert_eq!(base["managed"]["version"], 1);
    assert_eq!(base["agents"].as_array().unwrap().len(), 8);

    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap())
            .unwrap();
    assert_eq!(journal["sourceKind"], "instance");
    assert_eq!(
        journal["sourcePath"],
        legacy
            .path()
            .join("agents.10.default.json")
            .display()
            .to_string()
    );
    assert_eq!(
        journal["managedBase"]["managed"]["revision"],
        base["managed"]["revision"]
    );

    // Extracted local layer: the custom entry is complete, the eight shipped
    // keys are tombstoned, and the legacy order is pinned.
    let local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(local_catalog_path(project.path())).unwrap())
            .unwrap();
    assert_eq!(local["order"], serde_json::json!(["mine"]));
    let rows = local["agents"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|row| row["key"] == "mine" && row["command"] == "mytool"));
    assert_eq!(rows.iter().filter(|row| row["remove"] == true).count(), 8);

    let loaded = load_catalog(project.path()).expect("migrated catalog");
    assert_eq!(keys_of(&loaded), ["mine"]);

    // Completed migration is idempotent: no re-extraction, no rewrite.
    let base_before = std::fs::read(manifest_path(project.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(project.path())).unwrap();
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
    assert_eq!(
        std::fs::read(manifest_path(project.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(project.path())).unwrap(),
        local_before
    );
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes,
        "the instance source bytes survive the rename, never rewritten"
    );
    assert!(!legacy.path().join("agents.json").exists());
}

#[test]
fn legacy_absent_or_not_a_file_seeds_embedded_default() {
    // Absent legacy dir -> embedded default.
    let project = seed_dir();
    let legacy = legacy_dir();
    ensure_seeded(project.path(), Some(legacy.path()));
    assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);

    // Legacy agents.json is a DIRECTORY -> not a regular file -> embedded.
    let project = seed_dir();
    std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
    ensure_seeded(project.path(), Some(legacy.path()));
    assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);

    // No legacy at all -> embedded default.
    let project = seed_dir();
    ensure_seeded(project.path(), None);
    assert_eq!(load_catalog(project.path()).expect("seeded").len(), 8);
}

#[test]
fn project_file_present_never_touched_even_when_legacy_differs() {
    let project = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();

    // First seed from legacy, then hand-edit the PROJECT file.
    ensure_seeded(project.path(), Some(legacy.path()));
    let project_file = manifest_path(project.path());
    let custom = manifest_json(
        r##"[{"key":"hand","label":"Hand","description":"d","color":"#222","command":"hand","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    std::fs::write(&project_file, &custom).unwrap();

    // A present project file is user-owned: a differing legacy must not win.
    ensure_seeded(project.path(), Some(legacy.path()));
    assert_eq!(std::fs::read_to_string(&project_file).unwrap(), custom);
}

#[test]
fn legacy_masters_tree_copied_per_builtin_dest_and_embedded_fallback() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_seed = legacy.path().join("_seed");
    // .claude and .codex masters exist in the legacy tree (one file each,
    // distinct bytes); .opencode does NOT (embedded default must win).
    std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap();
    std::fs::create_dir_all(legacy_seed.join(".codex")).unwrap();
    std::fs::write(legacy_seed.join(".claude/settings.json"), b"LEGACY CLAUDE").unwrap();
    std::fs::write(legacy_seed.join(".codex/config.toml"), b"LEGACY CODEX").unwrap();

    ensure_seeded_masters(project.path(), Some(legacy.path()));

    let claude_dir = master_dir_for_dest(project.path(), ".claude");
    assert_eq!(
        std::fs::read(claude_dir.join("settings.json")).unwrap(),
        b"LEGACY CLAUDE"
    );
    let codex_dir = master_dir_for_dest(project.path(), ".codex");
    assert_eq!(
        std::fs::read(codex_dir.join("config.toml")).unwrap(),
        b"LEGACY CODEX"
    );
    // .opencode: no legacy master -> embedded default staged.
    let opencode_dir = master_dir_for_dest(project.path(), ".opencode");
    let opencode_master = master("opencode");
    assert_eq!(
        std::fs::read(opencode_dir.join(opencode_master.files[0].rel_path)).unwrap(),
        opencode_master.files[0].bytes
    );
    // The legacy tree is untouched and re-running is a no-op (present).
    assert_eq!(
        std::fs::read(legacy_seed.join(".claude/settings.json")).unwrap(),
        b"LEGACY CLAUDE"
    );
    std::fs::write(legacy_seed.join(".claude/settings.json"), b"CHANGED").unwrap();
    ensure_seeded_masters(project.path(), Some(legacy.path()));
    assert_eq!(
        std::fs::read(claude_dir.join("settings.json")).unwrap(),
        b"LEGACY CLAUDE"
    );
}

#[test]
fn primary_project_root_first_entry_wins_legacy_fallback_none() {
    let mut settings = AppSettings::default();
    assert_eq!(primary_project_root(&settings), None);

    // project_paths first non-empty trimmed entry wins, whitespace padded.
    settings.project_paths = vec![
        "  ".to_string(),
        "  C:\\first\\project  ".to_string(),
        "C:\\second\\project".to_string(),
    ];
    assert_eq!(
        primary_project_root(&settings),
        Some(PathBuf::from("C:\\first\\project"))
    );

    // Empty project_paths -> legacy project_path fallback.
    settings.project_paths.clear();
    assert_eq!(primary_project_root(&settings), None);
    settings.project_path = Some("  C:\\legacy\\project ".to_string());
    assert_eq!(
        primary_project_root(&settings),
        Some(PathBuf::from("C:\\legacy\\project"))
    );

    // Whitespace-only legacy path -> None.
    settings.project_path = Some("   ".to_string());
    assert_eq!(primary_project_root(&settings), None);
}

#[test]
fn registered_project_roots_multi_and_legacy_only() {
    let mut settings = AppSettings::default();
    assert!(registered_project_roots(&settings).is_empty());

    settings.project_paths = vec![
        "  ".to_string(),
        "C:\\one".to_string(),
        "C:\\two".to_string(),
    ];
    assert_eq!(
        registered_project_roots(&settings),
        vec![PathBuf::from("C:\\one"), PathBuf::from("C:\\two")]
    );

    // Empty project_paths -> legacy project_path alone.
    settings.project_paths.clear();
    settings.project_path = Some("C:\\legacy".to_string());
    assert_eq!(
        registered_project_roots(&settings),
        vec![PathBuf::from("C:\\legacy")]
    );
    // Whitespace-only legacy path -> no roots.
    settings.project_path = Some(" ".to_string());
    assert!(registered_project_roots(&settings).is_empty());
}

#[test]
fn load_catalog_for_settings_primary_wins_and_never_falls_back() {
    let primary = seed_dir();
    let ac_dir = primary.path().join(".ac");
    let settings = AppSettings {
        project_paths: vec![primary.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };

    // Primary file absent -> unavailable: never an embedded default, never
    // the legacy instance copy.
    let unavailable = load_catalog_for_settings(&settings)
        .expect_err("absent primary is unavailable, never a self-heal");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(&ac_dir).display().to_string()
    );
    assert!(!catalog_dir(&ac_dir).exists());

    // Hand-edited primary file is observable (primary wins over everything).
    let custom = manifest_json(
        r##"[{"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(&ac_dir), &custom).unwrap();
    let loaded = load_catalog_for_settings(&settings).expect("persisted primary catalog");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].key, "custom");

    // Primary file DELETED -> unavailable again, never a legacy copy.
    std::fs::remove_file(manifest_path(&ac_dir)).unwrap();
    let unavailable =
        load_catalog_for_settings(&settings).expect_err("deleted primary is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
}

#[test]
fn load_catalog_unavailable_display_carries_code_path_and_reason() {
    let dir = seed_dir();
    let unavailable = load_catalog(dir.path()).expect_err("absent catalog");
    let rendered = unavailable.to_string();
    assert!(rendered.contains("baseUnavailable"), "{rendered}");
    assert!(
        rendered.contains(&manifest_path(dir.path()).display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("no persisted catalog"), "{rendered}");
    assert_eq!(
        rendered,
        format!(
            "catalog unavailable (baseUnavailable) at {}: {}",
            unavailable.path, unavailable.reason
        )
    );
}

#[test]
fn load_catalog_for_settings_two_projects_switch_order_and_isolation() {
    let alpha = seed_dir();
    let beta = seed_dir();
    write_report_manifest(
        &alpha.path().join(".ac"),
        &manifest_json(
            r##"[{"key":"alpha-sentinel","label":"Alpha","description":"d","color":"#111","command":"alpha-sentinel","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["alpha update"]}]"##,
        ),
    );
    write_report_manifest(
        &beta.path().join(".ac"),
        &manifest_json(
            r##"[{"key":"beta-sentinel","label":"Beta","description":"d","color":"#222","command":"beta-sentinel","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["beta update"]}]"##,
        ),
    );
    let alpha_path = alpha.path().to_string_lossy().to_string();
    let beta_path = beta.path().to_string_lossy().to_string();

    let settings = AppSettings {
        project_paths: vec![alpha_path.clone(), beta_path.clone()],
        ..AppSettings::default()
    };
    let loaded = load_catalog_for_settings_with_config_dir(&settings, None).expect("alpha catalog");
    assert_eq!(keys_of(&loaded), ["alpha-sentinel"]);

    // Switch order: the first entry is primary and nothing is merged in.
    let switched = AppSettings {
        project_paths: vec![beta_path.clone(), alpha_path.clone()],
        ..AppSettings::default()
    };
    let loaded = load_catalog_for_settings_with_config_dir(&switched, None).expect("beta catalog");
    assert_eq!(keys_of(&loaded), ["beta-sentinel"]);

    // A failing primary never falls back to the second project.
    let missing = seed_dir();
    let failed = AppSettings {
        project_paths: vec![
            missing.path().to_string_lossy().to_string(),
            alpha_path.clone(),
        ],
        ..AppSettings::default()
    };
    let unavailable = load_catalog_for_settings_with_config_dir(&failed, None)
        .expect_err("failing primary is unavailable");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert_eq!(
        unavailable.path,
        manifest_path(&missing.path().join(".ac"))
            .display()
            .to_string()
    );
}

#[test]
fn load_catalog_for_settings_project_path_fallback_and_no_project_instance() {
    let project = seed_dir();
    write_report_manifest(
        &project.path().join(".ac"),
        &manifest_json(
            r##"[{"key":"legacy-root","label":"Legacy root","description":"d","color":"#111","command":"legacy-root","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["legacy-root update"]}]"##,
        ),
    );
    let settings = AppSettings {
        project_paths: vec!["   ".to_string()],
        project_path: Some(format!("  {}  ", project.path().display())),
        ..AppSettings::default()
    };
    let loaded = load_catalog_for_settings_with_config_dir(&settings, None)
        .expect("trimmed legacy project_path catalog");
    assert_eq!(keys_of(&loaded), ["legacy-root"]);

    // No-project mode reads the existing instance catalog read-only.
    let instance = seed_dir();
    write_report_manifest(
        instance.path(),
        &manifest_json(
            r##"[{"key":"instance","label":"Instance","description":"d","color":"#111","command":"instance","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["instance update"]}]"##,
        ),
    );
    let before = std::fs::read(manifest_path(instance.path())).unwrap();
    let loaded = load_catalog_for_settings_with_config_dir(
        &AppSettings::default(),
        Some(instance.path().to_path_buf()),
    )
    .expect("instance catalog");
    assert_eq!(keys_of(&loaded), ["instance"]);
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        before
    );

    // Absent instance catalog: unavailable, never another project, and
    // repeated reads create nothing.
    let absent = seed_dir();
    for _ in 0..2 {
        let unavailable = load_catalog_for_settings_with_config_dir(
            &AppSettings::default(),
            Some(absent.path().to_path_buf()),
        )
        .expect_err("absent instance catalog");
        assert_eq!(unavailable.code, "baseUnavailable");
    }
    assert!(!catalog_dir(absent.path()).exists());

    // Unresolvable config dir: unavailable with an empty path.
    let unavailable = load_catalog_for_settings_with_config_dir(&AppSettings::default(), None)
        .expect_err("unresolvable config dir");
    assert_eq!(unavailable.code, "baseUnavailable");
    assert!(unavailable.path.is_empty());
}

#[test]
fn load_catalog_keeps_valid_entry_while_warning_about_a_legacy_one() {
    // A migrationPending warning never blocks usable persisted commands.
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(
            r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","command":"legacy","envs":[],"isolatedHome":false,"removable":true},
             {"key":"fresh","label":"Fresh","description":"d","color":"#222","command":"fresh","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["fresh update"]}]"##,
        ),
    );

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(keys_of(&loaded), ["legacy", "fresh"]);
    assert!(loaded[0].update_commands.is_empty());
    assert_eq!(loaded[1].update_commands, vec!["fresh update".to_string()]);

    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].code, "migrationPending");
}

#[test]
fn ensure_seeded_for_project_skips_missing_or_relative_root_without_writes() {
    let base = seed_dir();
    // Missing root: nothing must be created.
    let missing = base.path().join("does-not-exist");
    ensure_seeded_for_project(&missing);
    assert!(!missing.exists());

    // Relative root: nothing must be created relative to the process CWD.
    let relative = Path::new("some-relative-project");
    ensure_seeded_for_project(relative);
    assert!(!relative.exists());
}

#[test]
fn ensure_seeded_for_project_steady_state_precheck_skips_gate_and_writes() {
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    // First run seeds everything under the (test) ungated path.
    ensure_seeded_for_project(&root);
    assert!(manifest_path(&root.join(".ac")).is_file());
    for m in EMBEDDED_SEED_MASTERS {
        assert!(master_dir_for_dest(&root.join(".ac"), m.dest).is_dir());
    }
    let manifest_bytes = std::fs::read(manifest_path(&root.join(".ac"))).unwrap();

    // Steady state: re-running must not touch a byte (and must not even
    // need the manifest to parse; the pre-check is existence-only).
    ensure_seeded_for_project(&root);
    assert_eq!(
        std::fs::read(manifest_path(&root.join(".ac"))).unwrap(),
        manifest_bytes
    );

    // A missing master dir is re-seeded by the next run (masters self-heal).
    let claude_dir = master_dir_for_dest(&root.join(".ac"), ".claude");
    std::fs::remove_dir_all(&claude_dir).unwrap();
    ensure_seeded_for_project(&root);
    assert!(claude_dir.is_dir());
}

#[test]
fn managed_catalog_corrupt_instance_legacy_blocks_transfer_without_writes() {
    // An unreadable legacy source must not be "migrated" or trashed: the
    // transfer is refused, no base is published, and every byte stays put.
    let project = seed_dir();
    let legacy = legacy_dir();
    let garbage = b"{ this is not valid json".to_vec();
    std::fs::write(legacy.path().join("agents.json"), &garbage).unwrap();

    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
    let catalog = catalog_dir(project.path());
    assert!(
        !manifest_path(project.path()).exists(),
        "no base is published"
    );
    assert!(!catalog.join(MIGRATION_BACKUP_FILENAME).exists());
    assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    assert!(!local_catalog_path(project.path()).exists());
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        garbage
    );
    let unavailable = load_catalog(project.path()).expect_err("no project base");
    assert_eq!(unavailable.code, "baseUnavailable");

    // Recovery: remove the corrupt source; the next initialization seeds.
    std::fs::remove_file(legacy.path().join("agents.10.default.json")).unwrap();
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
    assert_eq!(
        load_catalog(project.path())
            .expect("fresh managed base")
            .len(),
        8
    );
}

#[test]
fn legacy_masters_empty_dir_copied_verbatim_and_inert() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_seed = legacy.path().join("_seed");
    std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap(); // EMPTY

    ensure_seeded_masters(project.path(), Some(legacy.path()));
    let claude_dir = master_dir_for_dest(project.path(), ".claude");
    assert!(
        claude_dir.is_dir(),
        "empty legacy master is copied verbatim"
    );
    assert!(
        !crate::config::config_seed::is_nonempty_seed_dir(&claude_dir),
        "present-but-empty master stays inert for the spawn tier"
    );
    assert_eq!(std::fs::read_dir(&claude_dir).unwrap().count(), 0);
    // The embedded default is NOT seeded (present = user-owned rule); the
    // Settings re-seed button restores it.
    reseed_master_for_command(project.path(), "claude").unwrap();
    assert!(crate::config::config_seed::is_nonempty_seed_dir(
        &claude_dir
    ));
}

#[test]
fn reseed_with_no_primary_targets_legacy_config_dir() {
    // The Settings re-seed button must keep working on pre-migration
    // installs with zero registered projects: no primary root -> the legacy
    // `<config_dir>/coding-agents/_seed/<dest>` masters are the target.
    //
    // #2001: this fixture dir stands in for the per-binary `config_dir()`.
    // This test is the ONLY writer of the real `<config_dir>/coding-agents`
    // tree, and separate test processes share that path; an in-process lock
    // cannot serialize processes, so the real path is never resolved here.
    let config_dir = seed_dir();
    let legacy_master = config_dir
        .path()
        .join("coding-agents")
        .join("_seed")
        .join(".claude");
    std::fs::create_dir_all(&legacy_master).unwrap();
    std::fs::write(legacy_master.join("marker"), b"x").unwrap();

    let result = reseed_master_for_command(config_dir.path(), "claude");
    assert!(result.is_ok(), "legacy reseed works: {result:?}");
    let m = master("claude");
    assert_eq!(
        std::fs::read(legacy_master.join(m.files[0].rel_path)).unwrap(),
        m.files[0].bytes
    );
}

#[test]
fn definition_defaults_update_commands_empty_auto_update_false_when_absent() {
    // An old agents.json (no new fields) parses with the documented
    // defaults: empty update commands, auto-update off.
    let json = r##"
    {
        "schemaVersion": 1,
        "agents": [
            {
                "key": "old",
                "label": "Old",
                "description": "d",
                "color": "#000",
                "command": "old",
                "envs": [],
                "isolatedHome": false,
                "removable": true
            }
        ]
    }
    "##;
    let parsed: CodingAgentCatalog = serde_json::from_str(json).expect("old manifest parses");
    let def = &parsed.agents[0];
    assert!(def.update_commands.is_empty());
    assert!(!def.auto_update);

    // camelCase round-trip: always serialize both fields.
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), json).unwrap();
    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded[0].update_commands.len(), 0);
    assert!(!loaded[0].auto_update);

    let mut def = loaded[0].clone();
    def.update_commands = vec!["claude --update".to_string()];
    def.auto_update = true;
    let round = serde_json::to_value(&def).expect("serialize def");
    assert_eq!(
        round["updateCommands"],
        serde_json::json!(["claude --update"])
    );
    assert_eq!(round["autoUpdate"], serde_json::json!(true));
}

#[test]
fn load_catalog_never_invents_update_commands_for_legacy_entries() {
    // A catalog seeded before the update-command era (#1325) carries no
    // updateCommands. #1967 P4: nothing is backfilled in memory - the
    // commands stay empty until the P5 migration persists defaults - the
    // persisted bytes are untouched, and the report explains the pending
    // migration per entry.
    let json = manifest_json(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
         {"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().all(|def| def.update_commands.is_empty()));
    // No-write proof: the manifest bytes are identical before/after the load.
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        json.as_bytes()
    );

    let report = report_for(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.warnings.len(), 2);
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code == "migrationPending"));
}

#[test]
fn load_catalog_preserves_persisted_update_commands() {
    // User-authored sequences are data: they are served exactly as written.
    let json = manifest_json(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["claude --custom"]}]"##,
    );
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(
        loaded[0].update_commands,
        vec!["claude --custom".to_string()]
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        json.as_bytes()
    );
}

#[test]
fn load_catalog_custom_command_without_sequence_stays_empty() {
    // Custom command `bob` has no persisted sequence -> stays empty (never
    // prompted nor updated). No embedded table is consulted.
    let json = manifest_json(
        r##"[{"key":"bob","label":"Bob","description":"d","color":"#333","command":"bob","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].update_commands.is_empty());
}

#[test]
fn load_catalog_duplicate_commands_keep_their_own_entries() {
    // Two profiles share the `pi` command under different keys; the persisted
    // data is served per entry. The effective (first) entry rule lives in
    // `agent_update`, which never borrows the second entry's sequence.
    let json = manifest_json(
        r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
         {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().all(|def| def.update_commands.is_empty()));
}

#[test]
fn load_catalog_mixed_duplicate_sequences_keep_persisted_values() {
    // Data-level preservation: the FIRST `pi` entry stays empty and the
    // SECOND keeps its custom sequence untouched. The read never rewrites
    // one entry from another (the effective first-entry rule is applied by
    // the updater, not by the read).
    let json = manifest_json(
        r##"[{"key":"pi","label":"Pi","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true},
         {"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi --custom"]}]"##,
    );
    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &json).unwrap();

    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert_eq!(loaded.len(), 2);
    assert!(loaded[0].update_commands.is_empty());
    assert_eq!(loaded[1].update_commands, vec!["pi --custom".to_string()]);
}

#[test]
fn embedded_default_ships_update_commands_for_all_but_cursor_and_muse() {
    // #1318/#1325/#1546 drift guard: claude, pi, codex, hermes, opencode,
    // and antigravity ship the update command; cursor, grok and Muse ship
    // none; every entry defaults autoUpdate to false.
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.agents.len(), 9);
    for def in &catalog.agents {
        assert!(
            !def.auto_update,
            "{} autoUpdate must default false",
            def.key
        );
        match def.key.as_str() {
            "claude" => assert_eq!(def.update_commands, vec!["claude --update".to_string()]),
            "pi" => assert_eq!(def.update_commands, vec!["pi update".to_string()]),
            "codex" => assert_eq!(def.update_commands, vec!["codex update".to_string()]),
            "hermes" => {
                assert_eq!(def.update_commands, vec!["hermes update --yes".to_string()])
            }
            "opencode" => assert_eq!(def.update_commands, vec!["opencode upgrade".to_string()]),
            "antigravity" => assert_eq!(def.update_commands, vec!["agy update".to_string()]),
            "cursor" | "grok" | "muse" => assert!(
                def.update_commands.is_empty(),
                "{} must ship no update command",
                def.key
            ),
            other => panic!("unexpected key {other:?}"),
        }
    }
}

// ---- #1912 support switch: read gate + seed gate --------------------

fn assert_no_key(agents: &[CodingAgentDefinition], key: &str) {
    assert!(!agents.iter().any(|a| a.key == key), "{key} must be absent");
}

fn keys_of(agents: &[CodingAgentDefinition]) -> Vec<&str> {
    agents.iter().map(|a| a.key.as_str()).collect()
}

#[test]
fn builtin_agent_support_table_matches_embedded_default_keys_in_order() {
    // R1: the table and the embedded JSON must list the same keys in the
    // same order (exact order implies set equality both ways and no dupes).
    let table_keys: Vec<&str> = BUILTIN_AGENT_SUPPORT.iter().map(|(k, _)| *k).collect();
    let catalog = embedded_default_catalog();
    let json_keys: Vec<&str> = catalog.agents.iter().map(|def| def.key.as_str()).collect();
    assert_eq!(table_keys, json_keys);
}

#[test]
fn builtin_agent_support_ships_only_muse_disabled() {
    // R2: #1999 flips muse off; every other row still ships enabled.
    let disabled: Vec<&str> = BUILTIN_AGENT_SUPPORT
        .iter()
        .filter(|(_, on)| !*on)
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(disabled, ["muse"], "only muse ships disabled");
}

#[test]
fn support_override_scopes_to_closure_and_restores_shipped_table() {
    // R3: the override reaches ensure_seeded + load_catalog and restores the
    // shipped table.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let before = load_catalog(dir.path()).expect("seeded catalog");
    assert_eq!(before.len(), 8);
    assert_no_key(&before, "muse");

    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "the table is part of the managed revision"
        );
        let inside = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(inside.len(), 9);
        assert!(inside.iter().any(|a| a.key == "muse"));
    });

    let after = load_catalog(dir.path()).expect("seeded catalog");
    assert_eq!(after.len(), 8);
    assert_no_key(&after, "muse");
}

#[test]
fn desupported_row_dropped_from_seeded_manifest_and_corrupt_is_unavailable() {
    // R4 + #1967 P4: the read gate drops a de-supported row from a seeded
    // manifest; a corrupt persisted file is `baseInvalid` (no self-heal) and
    // its bytes are preserved.
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        let agents = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(agents.len(), 8);
        assert_no_key(&agents, "muse");
        assert_eq!(agents[0].key, "claude");

        let dir = seed_dir();
        let path = manifest_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let garbage = b"{ this is not valid json";
        std::fs::write(&path, garbage).unwrap();
        let unavailable = load_catalog(dir.path()).expect_err("corrupt catalog is unavailable");
        assert_eq!(unavailable.code, "baseInvalid");
        assert_eq!(std::fs::read(&path).unwrap(), garbage);
    });
}

#[test]
fn desupported_row_dropped_from_parsed_manifest_and_dedup_cannot_resurrect_it() {
    // R5: a duplicate-bearing user manifest cannot resurrect a de-supported
    // key; a user entry with ANOTHER key and the same command is kept.
    let agents = r##"[
        {"key":"muse","label":"First","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
        {"key":"muse","label":"Second","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
        {"key":"mine","label":"Mine","description":"d","color":"#111","command":"muse","envs":[],"isolatedHome":false,"removable":true}
    ]"##;

    // Control: the all-enabled table keeps muse -> duplicate key dedups
    // first-wins, custom key kept.
    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(keys_of(&loaded), ["muse", "mine"]);
        assert_eq!(loaded[0].label, "First");
    });

    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), manifest_json(agents)).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert_eq!(keys_of(&loaded), ["mine"]);
        assert_eq!(loaded[0].command, "muse");
    });
}

#[test]
fn desupported_row_dropped_from_load_catalog_for_settings_primary() {
    // R6: the settings read root (primary project) honors the read gate on
    // the persisted file path, and an absent primary is unavailable.
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let primary = seed_dir();
        let ac_dir = primary.path().join(".ac");
        let settings = AppSettings {
            project_paths: vec![primary.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        };

        let unavailable =
            load_catalog_for_settings(&settings).expect_err("absent primary is unavailable");
        assert_eq!(unavailable.code, "baseUnavailable");

        let user_file = manifest_json(
            r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
            {"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
        );
        std::fs::create_dir_all(manifest_path(&ac_dir).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(&ac_dir), &user_file).unwrap();
        let loaded = load_catalog_for_settings(&settings).expect("persisted primary");
        assert_eq!(keys_of(&loaded), ["custom"]);
    });
}

#[test]
fn no_embedded_donor_backfills_a_custom_key_bound_to_a_builtin_command() {
    // #1967 P4: the embedded default is no longer a runtime command donor.
    // A custom key bound to the builtin `claude` command, in a manifest with
    // no updateCommands, reads as empty and the persisted bytes are kept;
    // the de-supported table changes nothing about that.
    let manifest = manifest_json(
        r##"[{"key":"my-claude","label":"My Claude","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
    );

    let dir = seed_dir();
    std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
    std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
    let loaded = load_catalog(dir.path()).expect("persisted catalog");
    assert!(loaded[0].update_commands.is_empty());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        manifest.as_bytes()
    );

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        let dir = seed_dir();
        std::fs::create_dir_all(manifest_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(manifest_path(dir.path()), &manifest).unwrap();
        let loaded = load_catalog(dir.path()).expect("persisted catalog");
        assert!(loaded[0].update_commands.is_empty());
    });
}

#[test]
fn managed_catalog_fresh_base_bytes_carry_only_enabled_rows() {
    // The fresh managed base carries the enabled shipped rows plus the
    // ownership marker: the shipped table seeds the enabled 8 (grok in,
    // muse out) and its mirror override seeds the same 8 while the read
    // gate hides the key.
    let dir = seed_dir();
    assert!(ensure_seeded(dir.path(), None).is_some());
    let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
    let base: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(base["agents"].as_array().unwrap().len(), 8);
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert!(
        base["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["key"] == "grok"),
        "the fresh base publishes grok"
    );

    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "a first seed publishes"
        );
        let bytes = std::fs::read(manifest_path(dir.path())).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("\"muse\""),
            "seeded bytes must not carry the de-supported key"
        );
        let catalog: CodingAgentCatalog =
            serde_json::from_slice(&bytes).expect("seeded manifest parses");
        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        assert_eq!(catalog.agents.len(), 8);
        let loaded = load_catalog(dir.path()).expect("seeded catalog");
        assert_eq!(loaded.len(), 8);
        assert_no_key(&loaded, "muse");
    });
}

#[test]
fn managed_catalog_migration_backup_keeps_a_desupported_key_verbatim() {
    // Under a false support row a migrated legacy source is preserved
    // byte-for-byte in the backup; the read gate hides the de-supported key
    // from the effective catalog while the custom key survives.
    let legacy_bytes = manifest_json(
        r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true},
        {"key":"custom","label":"Custom","description":"d","color":"#333","command":"custom","envs":[],"isolatedHome":false,"removable":true}]"##,
    )
    .into_bytes();

    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let project = seed_dir();
        let legacy = legacy_dir();
        std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
        assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
        assert_eq!(
            std::fs::read(catalog_dir(project.path()).join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            legacy_bytes,
            "the backup is user data: byte-for-byte even with a de-supported key"
        );
        let loaded = load_catalog(project.path()).expect("migrated catalog");
        assert_eq!(keys_of(&loaded), ["custom"]);
    });
}

#[test]
fn managed_catalog_false_row_is_part_of_the_revision_and_refreshes_only_the_base() {
    // #1968/#1999: the shipped false muse row is part of the managed
    // revision, so a base seeded all-enabled refreshes to the enabled
    // shipped set; the user-owned local layer is never touched and the key
    // stays hidden either way.
    let dir = seed_dir();
    let seeded = with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        ensure_seeded(dir.path(), None);
        assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 9);
        std::fs::read(manifest_path(dir.path())).unwrap()
    });
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "a support-gate change is part of the managed revision"
    );
    assert_ne!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
    assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before,
        "the user-owned local layer is never touched by a refresh"
    );
    let loaded = load_catalog(dir.path()).expect("seeded catalog");
    assert_eq!(loaded.len(), 8);
    assert_no_key(&loaded, "muse");

    // Back under the all-enabled table the base returns byte-for-byte.
    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(ensure_seeded(dir.path(), None).is_some());
        assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), seeded);
    });
}

#[test]
fn desupported_master_not_seeded_not_reseedable_and_reseed_refused() {
    // R11: a de-supported key's master is not seeded, is not listed as
    // reseedable, and its re-seed is refused server-side; the other masters
    // are unaffected.
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    for dest in [".claude", ".codex", ".opencode"] {
        assert!(
            crate::config::config_seed::is_nonempty_seed_dir(&master_dir_for_dest(
                dir.path(),
                dest
            )),
            "{dest} master should be non-empty"
        );
    }
    let mut got = reseedable_command_basenames();
    got.sort();
    assert_eq!(got, vec!["claude", "codex", "opencode"]);

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        let dir = seed_dir();
        ensure_seeded_masters(dir.path(), None);
        assert!(
            !master_dir_for_dest(dir.path(), ".claude").exists(),
            "de-supported master must not be seeded"
        );
        for dest in [".codex", ".opencode"] {
            assert!(
                crate::config::config_seed::is_nonempty_seed_dir(&master_dir_for_dest(
                    dir.path(),
                    dest
                )),
                "{dest} master must still be seeded"
            );
        }
        let mut got = reseedable_command_basenames();
        got.sort();
        assert_eq!(got, vec!["codex", "opencode"]);
        let err = reseed_master_for_command(dir.path(), "claude").unwrap_err();
        assert!(err.contains("not a recognized built-in"), "{err}");
        assert!(reseed_master_for_command(dir.path(), "codex").is_ok());
    });
}

#[test]
fn desupported_master_skips_legacy_tree_copy_too() {
    // R12: the legacy `_seed/<dest>` tree copy is skipped for a de-supported
    // key; supported masters are still copied verbatim; the legacy tree is
    // untouched.
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_seed = legacy.path().join("_seed");
    std::fs::create_dir_all(legacy_seed.join(".claude")).unwrap();
    std::fs::create_dir_all(legacy_seed.join(".codex")).unwrap();
    std::fs::write(legacy_seed.join(".claude/settings.json"), b"LEGACY CLAUDE").unwrap();
    std::fs::write(legacy_seed.join(".codex/config.toml"), b"LEGACY CODEX").unwrap();

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        ensure_seeded_masters(project.path(), Some(legacy.path()));
        assert!(
            !master_dir_for_dest(project.path(), ".claude").exists(),
            "de-supported master must not be copied from the legacy tree"
        );
        assert_eq!(
            std::fs::read(master_dir_for_dest(project.path(), ".codex").join("config.toml"))
                .unwrap(),
            b"LEGACY CODEX"
        );
        assert_eq!(
            std::fs::read(legacy_seed.join(".claude/settings.json")).unwrap(),
            b"LEGACY CLAUDE",
            "the legacy tree itself is never touched"
        );
    });
}

#[test]
fn managed_catalog_all_masters_present_ignores_desupported_master() {
    // R13 (#1968): only the MASTER existence optimization remains, and it
    // requires only SUPPORTED masters, so a claude-off install is steady
    // without `.claude`. The catalog itself always validates under the
    // gate/lock; no existence-only shortcut may skip that (proven by the
    // steady-state test below).
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        ensure_seeded(&ac_dir, None);
        ensure_seeded_masters(&ac_dir, None);
        assert!(manifest_path(&ac_dir).is_file());
        assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
        assert!(crate::config::config_seed::is_nonempty_seed_dir(
            &master_dir_for_dest(&ac_dir, ".codex")
        ));
        assert!(crate::config::config_seed::is_nonempty_seed_dir(
            &master_dir_for_dest(&ac_dir, ".opencode")
        ));
        assert!(
            all_masters_present(&ac_dir),
            "steady inside the override: `.claude` must not be required"
        );

        // A missing SUPPORTED master makes the predicate false and is
        // re-seeded by the plain project entry point; `.claude` stays absent.
        let codex_dir = master_dir_for_dest(&ac_dir, ".codex");
        std::fs::remove_dir_all(&codex_dir).unwrap();
        assert!(!all_masters_present(&ac_dir));
        ensure_seeded_for_project(&root);
        assert!(crate::config::config_seed::is_nonempty_seed_dir(&codex_dir));
        assert!(!master_dir_for_dest(&ac_dir, ".claude").exists());
        assert!(all_masters_present(&ac_dir));
    });

    // Outside the override the same tree has no `.claude`: the shipped
    // table requires it and the predicate is false.
    assert!(!all_masters_present(&ac_dir));
}

#[test]
fn managed_catalog_steady_state_takes_the_gate_and_keeps_bytes() {
    // #1968: the existence-only catalog shortcut is gone. A second
    // tokenized initialization takes the project gate (lock file appears)
    // and the catalog lock, yet an unedited base at the shipped revision is
    // NOT rewritten: byte identity is preserved.
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
    ensure_seeded(&ac_dir, None);
    ensure_seeded_masters(&ac_dir, None);

    let base_bytes = std::fs::read(manifest_path(&ac_dir)).unwrap();
    let local_bytes = std::fs::read(local_catalog_path(&ac_dir)).unwrap();
    let gate_lock = ac_dir.join(crate::config::seed_manifest::SEED_MANIFEST_LOCK_FILENAME);
    assert!(!gate_lock.exists(), "no gate lock from the ungated seed");

    let token = ManifestActivationToken::for_test();
    ensure_seeded_for_project_with_token(&root, Some(&token));
    assert!(
        gate_lock.is_file(),
        "the catalog has no existence shortcut: the gate must have been taken"
    );
    assert_eq!(
        std::fs::read(manifest_path(&ac_dir)).unwrap(),
        base_bytes,
        "an unedited base at the shipped revision is never rewritten"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        local_bytes,
        "the user-owned local layer is never rewritten"
    );

    // Second tokenized run: still byte-identical.
    ensure_seeded_for_project_with_token(&root, Some(&token));
    assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_bytes);
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        local_bytes
    );
}

#[test]
fn already_seeded_master_never_removed_by_a_false_row() {
    // R14: an already-present master is user-owned and never removed by a
    // false row; only the reseedable list reflects the switch.
    let dir = seed_dir();
    ensure_seeded_masters(dir.path(), None);
    let claude_file = master_dir_for_dest(dir.path(), ".claude").join("settings.json");
    let seeded = std::fs::read(&claude_file).unwrap();

    with_builtin_agent_support_for_test(TABLE_CLAUDE_OFF, || {
        ensure_seeded_masters(dir.path(), None);
        assert_eq!(
            std::fs::read(&claude_file).unwrap(),
            seeded,
            "an already-present master is never removed or rewritten"
        );
        let mut got = reseedable_command_basenames();
        got.sort();
        assert_eq!(got, vec!["codex", "opencode"]);
    });
}

#[test]
fn managed_catalog_fresh_base_bytes_carry_only_enabled_rows_with_nonregular_legacy() {
    // A non-regular instance source is NOT a source: fresh initialization
    // runs, and under a false support row only the enabled rows land in the
    // managed base.
    for shape in ["absent", "directory"] {
        let project = seed_dir();
        let legacy = legacy_dir();
        if shape == "directory" {
            std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
        }
        assert!(
            ensure_seeded(project.path(), Some(legacy.path())).is_some(),
            "{shape}: a first seed publishes"
        );
        let bytes = std::fs::read(manifest_path(project.path())).unwrap();
        let catalog: CodingAgentCatalog =
            serde_json::from_slice(&bytes).expect("seeded manifest parses");
        assert_eq!(catalog.agents.len(), 8, "{shape}: 8 agents seeded");

        with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
            let project = seed_dir();
            let legacy = legacy_dir();
            if shape == "directory" {
                std::fs::create_dir_all(legacy.path().join("agents.json")).unwrap();
            }
            assert!(
                ensure_seeded(project.path(), Some(legacy.path())).is_some(),
                "{shape}: a first seed publishes"
            );
            let bytes = std::fs::read(manifest_path(project.path())).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains("\"muse\""),
                "{shape}: seeded bytes must not carry the de-supported key"
            );
            let catalog: CodingAgentCatalog =
                serde_json::from_slice(&bytes).expect("seeded manifest parses");
            assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
            assert_eq!(catalog.agents.len(), 8, "{shape}: 8 agents seeded");
            let loaded = load_catalog(project.path()).expect("persisted catalog");
            assert_eq!(loaded.len(), 8);
            assert_no_key(&loaded, "muse");
            assert_eq!(loaded[0].key, "claude");
        });
    }
}

// ---- #1963 P1: persisted-only catalog report --------------------------

fn write_report_manifest(ac_dir: &Path, contents: &str) {
    let path = manifest_path(ac_dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
}

fn report_for(dir: &Path) -> CatalogReport {
    load_catalog_report(dir)
}

fn report_json(report: &CatalogReport) -> String {
    serde_json::to_string(report).expect("serialize report")
}

#[test]
fn catalog_report_serializes_exact_camel_case_wire_shape() {
    let dir = seed_dir();
    write_report_manifest(
        dir.path(),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
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

#[cfg(windows)]
fn create_manifest_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn create_manifest_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[test]
fn catalog_report_symlink_manifest_is_unavailable() {
    let dir = seed_dir();
    let target = dir.path().join("real-catalog.json");
    std::fs::write(
        &target,
        manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
    )
    .unwrap();
    let link = manifest_path(dir.path());
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    if let Err(e) = create_manifest_symlink(&target, &link) {
        // Explicit, never silent: the link fixture could not be created on
        // this host (Windows symlink privilege), so the link assertion was
        // NOT exercised and must not be counted as a pass.
        eprintln!(
            "[catalog-report test] symlink creation unavailable on this host ({e}); link fixture NOT exercised"
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
        &manifest_json(
            r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
             {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true},
             {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
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
    let missing = manifest_json(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true},
         {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true},
         {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    let explicit = manifest_json(
        r##"[{"key":"claude","label":"Claude Code","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
         {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
         {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
    );
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
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["mytool up --channel beta"]},
             {"key":"claude-beta","label":"Claude Beta","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["claude update"]}]"##,
        ),
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
        &manifest_json(
            r##"[{"key":"mine","label":"First","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"mine","label":"Second","description":"d","color":"#222","command":"mytool2","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
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
        &manifest_json(
            r##"[{"key":"BAD KEY","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"cmd-continue","label":"x","description":"d","color":"#000","command":"claude --continue","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
             {"key":"nonstring","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["ok",123]},
             {"key":"blank","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["   "]},
             {"key":"ctrl","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["a\u0007b"]},
             {"key":"line-sep","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["a\u2028b"]},
             {"key":"para-sep","label":"x","description":"d","color":"#000","command":"x","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["a\u2029b"]},
             {"key":"full","label":"Full","description":"d","color":"#333","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["mytool --flag 'a b' && echo done"]}]"##,
        ),
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
        r##"{"schemaVersion":1,"owner":"secret-root-value","agents":[
            {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[{"key":"GOOD_KEY","value":"secret-env-value","enabled":true,"mystery":"secret-env-extra"}],"isolatedHome":false,"removable":true,"updateCommands":[],"customField":"secret-row-value","configSeed":{"enabled":true,"dest":".mine","extra":"secret-seed-value"}}]}"##,
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
        &manifest_json(
            r##"[{"key":"primary","label":"Primary","description":"d","color":"#111","command":"primary","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
    );
    write_report_manifest(
        &secondary.path().join(".ac"),
        &manifest_json(
            r##"[{"key":"secondary","label":"Secondary","description":"d","color":"#222","command":"secondary","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
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
        &manifest_json(
            r##"[{"key":"legacy","label":"Legacy","description":"d","color":"#111","command":"legacy","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
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
        &manifest_json(
            r##"[{"key":"instance","label":"Instance","description":"d","color":"#111","command":"instance","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
        ),
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
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true},
             {"key":"mine2","label":"Mine2","description":"d","color":"#222","command":"mytool2","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
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
    let manifest = manifest_json(
        r##"[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
         {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]"##,
    );
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

// -----------------------------------------------------------------------
// #1968 P5 - managed base, local overrides, migration, recovery, locks.
// -----------------------------------------------------------------------

fn ac_dir_for(project: &Path) -> PathBuf {
    project.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn read_text(path: &Path) -> String {
    String::from_utf8(
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .expect("utf-8 fixture")
}

fn local_json(ac_dir: &Path) -> serde_json::Value {
    read_json(&local_catalog_path(ac_dir))
}

fn base_json(ac_dir: &Path) -> serde_json::Value {
    read_json(&manifest_path(ac_dir))
}

fn write_local(ac_dir: &Path, contents: &str) {
    let path = local_catalog_path(ac_dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
}

fn write_legacy_base(ac_dir: &Path, contents: &str) {
    let path = manifest_path(ac_dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
}

/// The shipped definitions for `keys`, as plain JSON values.
fn shipped_def_json(keys: &[&str]) -> Vec<serde_json::Value> {
    let shipped = supported_shipped_definitions();
    keys.iter()
        .map(|key| {
            let definition = shipped
                .iter()
                .find(|definition| definition.key == *key)
                .unwrap_or_else(|| panic!("no shipped key {key}"));
            serde_json::to_value(definition).unwrap()
        })
        .collect()
}

/// A managed base whose marker claims `revision`; the content hash is the
/// real hash of `agents` unless the caller mangles it.
fn write_managed_base(
    ac_dir: &Path,
    agents: &[serde_json::Value],
    revision: &str,
    correct_content_hash: bool,
) -> Vec<u8> {
    let definitions: Vec<CodingAgentDefinition> = agents
        .iter()
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    let content = if correct_content_hash {
        managed_content_sha256(&definitions)
    } else {
        "0".repeat(64)
    };
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": definitions,
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": revision,
            "contentSha256": content,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(ac_dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    bytes
}

fn with_failure_at<R>(point: &'static str, f: impl FnOnce() -> R) -> R {
    CATALOG_FAILURE_POINT.with(|cell| cell.set(Some(point)));
    let result = f();
    CATALOG_FAILURE_POINT.with(|cell| cell.set(None));
    result
}

fn dir_entries(dir: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn managed_catalog_fresh_init_seeds_verified_base_and_exact_stub() {
    let dir = seed_dir();
    assert!(ensure_seeded(dir.path(), None).is_some());
    let base = base_json(dir.path());
    let expected_revision = managed_content_sha256(&supported_shipped_definitions());
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert_eq!(base["managed"]["version"], 1);
    assert_eq!(base["managed"]["revision"], expected_revision);
    assert_eq!(base["managed"]["contentSha256"], expected_revision);
    assert_eq!(base["agents"].as_array().unwrap().len(), 8);
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        LOCAL_STUB_BYTES,
        "the local stub is exactly the documented bytes plus LF"
    );

    let base_bytes = std::fs::read(manifest_path(dir.path())).unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.catalog.len(), 8);
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "a verified base at the shipped revision is never rewritten"
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_bytes
    );
    let residue: Vec<std::ffi::OsString> = std::fs::read_dir(catalog_dir(dir.path()))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .map(|entry| entry.file_name())
        .collect();
    assert!(residue.is_empty(), "no publication temporaries survive");
}

#[test]
fn managed_catalog_refresh_reaches_unpinned_values_and_keeps_pinned_local() {
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("OLD LABEL");
    write_managed_base(dir.path(), &[claude], "stale-revision", true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MINE"}]}"##,
    );
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "a base at a stale revision refreshes"
    );
    let base = base_json(dir.path());
    assert_eq!(
        base["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    assert_eq!(
        base["agents"].as_array().unwrap().len(),
        8,
        "refresh publishes the whole shipped set"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before,
        "refresh never touches the local layer or the composed view"
    );

    let loaded = load_catalog(dir.path()).unwrap();
    let claude = loaded.iter().find(|d| d.key == "claude").unwrap();
    assert_eq!(claude.label, "MINE", "the pinned local value wins");
    assert_eq!(
        claude.description, "Coding Agent by Anthropic",
        "an unpinned value reaches the refreshed shipped default"
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn managed_catalog_support_gate_change_refreshes_the_base_both_ways() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let full_bytes = std::fs::read(manifest_path(dir.path())).unwrap();
    assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);

    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "a support-gate change is part of the managed revision"
        );
        let base = base_json(dir.path());
        assert_eq!(base["agents"].as_array().unwrap().len(), 9);
        assert_eq!(
            base["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        assert_eq!(load_catalog(dir.path()).unwrap().len(), 9);
    });

    // Back under the shipped table the revision changes again and the base
    // byte-for-byte returns to its original state.
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        full_bytes
    );
    assert_eq!(load_catalog(dir.path()).unwrap().len(), 8);
}

#[test]
fn managed_catalog_edited_base_is_readable_and_never_refreshed() {
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("HAND EDITED");
    let bytes = write_managed_base(dir.path(), &[claude], "stale-revision", false);

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog[0].label, "HAND EDITED");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "managedBaseEdited"),
        "{:?}",
        report.warnings
    );
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "an edited managed base is never auto-refreshed"
    );
    assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), bytes);
}

#[test]
fn managed_catalog_unknown_managed_fields_block_refresh_without_deletion() {
    let dir = seed_dir();
    let agents = shipped_def_json(&["claude"]);
    let definitions: Vec<CodingAgentDefinition> = agents
        .iter()
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    let content = managed_content_sha256(&definitions);
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": definitions,
        "futureRootField": {"keep": "me"},
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": "stale-revision",
            "contentSha256": content,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "unknown data blocks refresh rather than being deleted"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn managed_catalog_foreign_managed_marker_blocks_ownership() {
    let dir = seed_dir();
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": shipped_def_json(&["claude"]),
        "managed": {
            "owner": "somebody-else",
            "version": 1,
            "revision": "x",
            "contentSha256": "y",
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1, "a foreign base stays readable");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationConflict"));
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "a foreign marker neither refreshes nor migrates"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn managed_catalog_existing_local_entries_are_never_clobbered() {
    test09_directory_local_preserved();
    test09_regular_local_and_read_denied_preserved();
}

#[cfg(test)]
type TreeInventoryRows = Vec<(
    std::path::PathBuf,
    bool,
    u64,
    bool,
    std::time::SystemTime,
    Vec<u8>,
)>;

fn test09_visit(root: &std::path::Path, path: &std::path::Path, rows: &mut TreeInventoryRows) {
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
            test09_visit(root, &entry, rows);
        }
    }
}

fn test09_inventory(
    root: &std::path::Path,
) -> Vec<(
    std::path::PathBuf,
    bool,
    u64,
    bool,
    std::time::SystemTime,
    Vec<u8>,
)> {
    let mut rows = Vec::new();
    test09_visit(root, root, &mut rows);
    rows
}

fn test09_directory_local_preserved() {
    // A directory at the local path is preserved and disables the layer.
    let dir = seed_dir();
    let local = local_catalog_path(dir.path());
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("keep.txt"), b"keep").unwrap();
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert!(local.is_dir());
    assert_eq!(std::fs::read(local.join("keep.txt")).unwrap(), b"keep");
    let before = test09_inventory(dir.path());
    let report = load_catalog_report(dir.path());
    assert_eq!(report.catalog.len(), 8, "the valid base stays usable");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"),
        "{:?}",
        report.warnings
    );

    assert!(report.unavailable.is_none());
    assert!(report.warnings.iter().any(|w| w.code == "localInvalid"
        && w.path == local.display().to_string()
        && !w.reason.is_empty()));
    for _ in 0..3 {
        assert_eq!(load_catalog_report(dir.path()), report);
    }
    assert_eq!(test09_inventory(dir.path()), before);
}

fn test09_regular_local_and_read_denied_preserved() {
    // A user-authored regular local file is preserved byte-for-byte and
    // composes onto the fresh managed base.
    let dir = seed_dir();
    let user = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##;
    write_local(dir.path(), user);
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(read_text(&local_catalog_path(dir.path())), user);
    let before = test09_inventory(dir.path());
    let loaded = load_catalog(dir.path()).unwrap();
    assert_eq!(loaded.len(), 9);
    assert_eq!(loaded.last().unwrap().key, "mine");
    assert_eq!(loaded.last().unwrap().label, "Mine");
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog, loaded);
    for _ in 0..3 {
        assert_eq!(load_catalog_report(dir.path()), report);
    }
    let mut expected = loaded.clone();
    expected.retain(|row| row.key != "mine");
    let mut denied: Option<ResolvedCatalog> = None;
    for _ in 0..3 {
        let snapshot = read_catalog_snapshot_with(
            dir.path(),
            CatalogReadScope::Project,
            |path, description| {
                if path == local_catalog_path(dir.path()) {
                    Err("local read denied".into())
                } else {
                    read_optional_regular_file(path, description)
                }
            },
        )
        .unwrap();
        let degraded =
            resolve_catalog_snapshot(dir.path(), snapshot, CatalogSourceContext::Project);
        assert!(degraded.unavailable.is_none());
        assert_eq!(degraded.catalog, expected);
        assert_eq!(keys_of(&degraded.catalog), keys_of(&expected));
        assert!(!degraded.catalog.iter().any(|row| row.key == "mine"));
        assert!(degraded.warnings.iter().any(|w| w.code == "localInvalid"
            && w.path == local_catalog_path(dir.path()).display().to_string()
            && w.reason == "local read denied"));
        if let Some(previous) = &denied {
            assert_eq!(degraded.catalog, previous.catalog);
            assert_eq!(degraded.warnings, previous.warnings);
            assert_eq!(degraded.unavailable, previous.unavailable);
            assert_eq!(
                degraded.base_verified_managed,
                previous.base_verified_managed
            );
        }
        denied = Some(degraded);
    }
    assert_eq!(test09_inventory(dir.path()), before);
}

#[test]
fn managed_catalog_local_composes_every_field_and_explicit_false_empty() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[
            {"key":"claude","label":"My Claude","description":"D","color":"#010203","command":"claude","instructionsFilename":null,"envs":[{"key":"ZZ","value":"1"},{"key":"AA","value":"2","source":"system","enabled":false}],"isolatedHome":true,"configSeed":{"enabled":false},"removable":false,"updateCommands":[],"autoUpdate":true}
        ]}"##,
    );
    let loaded = load_catalog(dir.path()).unwrap();
    let claude = loaded.iter().find(|d| d.key == "claude").unwrap();
    assert_eq!(claude.label, "My Claude");
    assert_eq!(claude.description, "D");
    assert_eq!(claude.color, "#010203");
    assert_eq!(claude.instructions_filename, None, "explicit null clears");
    assert_eq!(claude.envs.len(), 2, "envs replace whole arrays in order");
    assert_eq!(claude.envs[0].key, "ZZ");
    assert_eq!(claude.envs[1].key, "AA");
    assert!(!claude.envs[1].enabled, "explicit false stays explicit");
    assert_eq!(claude.envs[1].source, CodingAgentEnvSource::System);
    assert!(claude.isolated_home);
    let seed = claude.config_seed.as_ref().expect("merged configSeed");
    assert!(!seed.enabled);
    assert_eq!(seed.dest, ".claude", "nested configSeed merges by presence");
    assert!(!claude.removable);
    assert!(claude.update_commands.is_empty(), "explicit [] stays empty");
    assert!(claude.auto_update);
    assert_eq!(loaded.len(), 8, "no other row changed");
}

#[test]
fn managed_catalog_local_new_row_order_and_tombstone() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[
            {"key":"zeta","label":"Zeta","description":"d","color":"#111","command":"zeta","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false},
            {"key":"grok","remove":true}
        ],"order":["zeta","claude"]}"##,
    );
    let loaded = load_catalog(dir.path()).unwrap();
    let keys = keys_of(&loaded);
    assert_eq!(keys[0], "zeta", "listed survivors come first in order");
    assert_eq!(keys[1], "claude");
    assert!(!keys.contains(&"grok"), "the tombstone removes the key");
    assert_eq!(keys.len(), 8, "8 shipped - 1 removed + 1 added");
}

#[test]
fn managed_catalog_local_invalid_layer_falls_back_as_a_whole() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let bad = r##"{"schemaVersion":1,"agents":[
        {"key":"claude","label":"Would Apply"},
        {"key":"broken","label":"Broken"}
    ]}"##;
    write_local(dir.path(), bad);
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 8);
    assert_eq!(
        report.catalog[0].label, "Claude Code",
        "the valid row must not be partially applied"
    );
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "localInvalid"));
    assert_eq!(
        read_text(&local_catalog_path(dir.path())),
        bad,
        "the offending file is never rewritten"
    );
}

#[test]
fn managed_catalog_local_schema_rejections_are_whole_layer() {
    let cases = [
        (
            "duplicate member",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","key":"pi"}]}"##,
        ),
        (
            "unknown field",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","mystery":1}]}"##,
        ),
        (
            "unsupported schema",
            r##"{"schemaVersion":999,"agents":[]}"##,
        ),
        (
            "remove false",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","remove":false}]}"##,
        ),
        (
            "tombstone extra field",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","remove":true,"label":"x"}]}"##,
        ),
        (
            "duplicate local key",
            r##"{"schemaVersion":1,"agents":[{"key":"claude"},{"key":"claude"}]}"##,
        ),
        (
            "duplicate order key",
            r##"{"schemaVersion":1,"agents":[],"order":["claude","claude"]}"##,
        ),
        (
            "control character in update command",
            "{\"schemaVersion\":1,\"agents\":[{\"key\":\"claude\",\"updateCommands\":[\"a\\u0007b\"]}]}",
        ),
    ];
    for (label, contents) in cases {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(dir.path(), contents);
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none(), "{label}");
        assert_eq!(report.catalog.len(), 8, "{label}: base rows intact");
        assert_eq!(report.catalog[0].label, "Claude Code", "{label}");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "localInvalid"),
            "{label}: {:?}",
            report.warnings
        );
        assert_eq!(
            read_text(&local_catalog_path(dir.path())),
            contents,
            "{label}"
        );
    }
}

#[test]
fn managed_catalog_local_removability_is_evaluated_against_the_base() {
    let dir = seed_dir();
    let mut grok = shipped_def_json(&["grok"]).remove(0);
    grok["removable"] = serde_json::json!(false);
    write_managed_base(dir.path(), &[grok], "stale", true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"grok","remove":true}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "localInvalid"));
    assert!(report.catalog.iter().any(|d| d.key == "grok"));
}

#[test]
fn managed_catalog_local_support_gate_attempts_stay_suppressed() {
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(!report.catalog.iter().any(|d| d.key == "muse"));
        assert!(report.warnings.iter().any(|warning| {
            warning.code == "invalidDefinition" && warning.reason.contains("not supported")
        }));
    });
}

#[test]
fn managed_catalog_local_unknown_tombstone_and_order_keys_are_ignored() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"future-key","remove":true}],"order":["ghost","claude"]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.catalog.len(), 8);
    assert_eq!(
        report.catalog[0].key, "claude",
        "unknown order keys are ignored"
    );
    assert!(
        read_text(&local_catalog_path(dir.path())).contains("future-key"),
        "an unknown tombstone is retained for a future shipped key"
    );
}

// ---- #1968 migration, recovery, locks and read-only proof ----------------

fn migration_journal_json(ac_dir: &Path) -> serde_json::Value {
    read_json(&migration_journal_path(ac_dir))
}

fn assert_reads_leave_state_unchanged(ac_dir: &Path) {
    let catalog = catalog_dir(ac_dir);
    let before_entries = dir_entries(&catalog);
    let before_base = std::fs::read(manifest_path(ac_dir)).ok();
    let before_local = std::fs::read(local_catalog_path(ac_dir)).ok();
    let before_backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).ok();
    let before_journal = std::fs::read(migration_journal_path(ac_dir)).ok();
    // Repeated reads are identical INCLUDING warnings: a read derives its
    // warnings from the same snapshot it serves, so nothing may drift.
    let first = report_json(&load_catalog_report(ac_dir));
    for _ in 0..3 {
        assert_eq!(report_json(&load_catalog_report(ac_dir)), first);
    }
    assert_eq!(dir_entries(&catalog), before_entries);
    assert_eq!(std::fs::read(manifest_path(ac_dir)).ok(), before_base);
    assert_eq!(std::fs::read(local_catalog_path(ac_dir)).ok(), before_local);
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).ok(),
        before_backup
    );
    assert_eq!(
        std::fs::read(migration_journal_path(ac_dir)).ok(),
        before_journal
    );
}

#[test]
fn managed_catalog_migrates_project_legacy_with_pins_changes_and_tombstones() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"claude","label":"My Claude","description":"d","color":"#d97706","command":"claude","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]},
         {"key":"pi","label":"Pi Custom","description":"d","color":"#ec4899","command":"pi-custom","envs":[],"isolatedHome":false,"removable":true},
         {"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    assert!(ensure_seeded(dir.path(), None).is_some());

    let journal = migration_journal_json(dir.path());
    assert_eq!(journal["version"], 1);
    assert_eq!(journal["sourceKind"], "project");
    assert_eq!(
        journal["sourcePath"],
        std::fs::canonicalize(manifest_path(dir.path()))
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        std::fs::read(catalog_dir(dir.path()).join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy.as_bytes(),
        "the backup is the byte-exact source"
    );

    let local = local_json(dir.path());
    assert_eq!(local["order"], serde_json::json!(["claude", "pi", "mine"]));
    let rows = local["agents"].as_array().unwrap();
    let claude = rows.iter().find(|row| row["key"] == "claude").unwrap();
    assert_eq!(claude["label"], "My Claude", "explicit values are pinned");
    assert_eq!(claude["updateCommands"], serde_json::json!([]));
    assert!(
        claude.get("instructionsFilename").is_none() && claude.get("configSeed").is_none(),
        "absent fields inherit the shipped default instead of being pinned"
    );
    let pi = rows.iter().find(|row| row["key"] == "pi").unwrap();
    assert_eq!(pi["command"], "pi-custom");
    assert_eq!(pi["updateCommands"], serde_json::json!([]));
    assert!(rows.iter().any(|row| row["key"] == "mine"));
    assert_eq!(
        rows.iter().filter(|row| row["remove"] == true).count(),
        6,
        "the six shipped keys absent from the legacy catalog are tombstoned"
    );

    let loaded = load_catalog(dir.path()).unwrap();
    assert_eq!(keys_of(&loaded), ["claude", "pi", "mine"]);
    assert_eq!(loaded[0].label, "My Claude");
    assert!(loaded[0].update_commands.is_empty(), "explicit [] clears");
    assert_eq!(loaded[1].command, "pi-custom");
    assert!(
        loaded[1].update_commands.is_empty(),
        "a changed command never inherits the shipped sequence"
    );

    // Completed migration: restart is a no-op and preserves every byte.
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before
    );
}

#[test]
fn managed_catalog_migration_refuses_an_existing_local() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let user_local = r##"{"schemaVersion":1,"agents":[]}"##;
    write_local(dir.path(), user_local);

    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(read_text(&local_catalog_path(dir.path())), user_local);
    assert!(!catalog_dir(dir.path())
        .join(MIGRATION_BACKUP_FILENAME)
        .exists());
    assert!(!catalog_dir(dir.path())
        .join(MIGRATION_JOURNAL_FILENAME)
        .exists());
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
}

#[test]
fn managed_catalog_migration_refuses_ambiguous_legacy_sources() {
    let cases = [
        (
            "unknown root field",
            r##"{"schemaVersion":1,"agents":[],"future":1}"##,
        ),
        (
            "unknown row field",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"x","description":"d","color":"#111","command":"claude","mystery":1}]}"##,
        ),
        (
            "invalid definition",
            r##"{"schemaVersion":1,"agents":[{"key":"BAD KEY","label":"x","description":"d","color":"#111","command":"x"}]}"##,
        ),
        (
            "duplicate keys",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"a","description":"d","color":"#111","command":"claude"},{"key":"claude","label":"b","description":"d","color":"#111","command":"claude"}]}"##,
        ),
        ("unsupported schema", r##"{"schemaVersion":2,"agents":[]}"##),
    ];
    for (label, legacy) in cases {
        let dir = seed_dir();
        write_legacy_base(dir.path(), legacy);
        assert!(ensure_seeded(dir.path(), None).is_none(), "{label}");
        assert_eq!(
            read_text(&manifest_path(dir.path())),
            legacy,
            "{label}: bytes preserved"
        );
        assert!(
            !catalog_dir(dir.path())
                .join(MIGRATION_BACKUP_FILENAME)
                .exists(),
            "{label}: no backup is written"
        );
        // The legacy base stays readable through the legacy resolver.
        let report = load_catalog_report(dir.path());
        assert!(
            report.unavailable.is_none()
                || report.unavailable.as_ref().unwrap().code == "baseInvalid",
            "{label}"
        );
    }
}

#[test]
fn managed_catalog_migration_of_an_empty_catalog_tombstones_every_shipped_key() {
    let dir = seed_dir();
    write_legacy_base(dir.path(), r##"{"schemaVersion":1,"agents":[]}"##);
    assert!(ensure_seeded(dir.path(), None).is_some());
    let local = local_json(dir.path());
    let rows = local["agents"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|row| row["remove"] == true));
    assert_eq!(local["order"], serde_json::json!([]));
    assert!(load_catalog(dir.path()).unwrap().is_empty());
}

#[test]
fn managed_catalog_migration_pins_explicit_values_even_equal_to_defaults() {
    let dir = seed_dir();
    let claude = shipped_def_json(&["claude"]).remove(0);
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [claude]}).to_string();
    write_legacy_base(dir.path(), &legacy);
    assert!(ensure_seeded(dir.path(), None).is_some());

    let local = local_json(dir.path());
    let row = local["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "claude")
        .unwrap();
    assert_eq!(row["label"], "Claude Code");
    assert_eq!(
        row["updateCommands"],
        serde_json::json!(["claude --update"])
    );
    assert_eq!(row["configSeed"]["dest"], ".claude");
    assert_eq!(row["instructionsFilename"], "CLAUDE.md");
    let loaded = load_catalog(dir.path()).unwrap();
    assert_eq!(
        loaded
            .iter()
            .find(|d| d.key == "claude")
            .unwrap()
            .update_commands,
        vec!["claude --update".to_string()]
    );
}

#[test]
fn managed_catalog_migration_never_imports_the_instance_local_layer() {
    let project = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();
    let instance_local = r##"{"schemaVersion":1,"agents":[{"key":"secret","label":"Secret","description":"d","color":"#111","command":"secret","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[]}]}"##;
    std::fs::write(legacy.path().join("agents.local.json"), instance_local).unwrap();

    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
    let local = read_text(&local_catalog_path(project.path()));
    assert!(
        !local.contains("secret"),
        "the instance local layer is never imported: {local}"
    );
    assert!(load_catalog(project.path())
        .unwrap()
        .iter()
        .all(|definition| definition.key != "secret"));
    assert_eq!(
        read_text(&legacy.path().join("agents.50.personal.no-git.json")),
        instance_local,
        "the instance local file is read-only"
    );
}

#[test]
fn managed_catalog_interrupt_after_backup_resumes_backup_only() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_backup", || ensure_seeded(dir.path(), None));

    let catalog = catalog_dir(dir.path());
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy.as_bytes()
    );
    assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    assert!(!local_catalog_path(dir.path()).exists());
    assert_eq!(
        read_text(&manifest_path(dir.path())),
        legacy,
        "base untouched"
    );
    assert!(
        load_catalog(dir.path()).is_ok(),
        "the readable legacy source stays usable while recovery is pending"
    );

    // Restart: the backup equals the current project source, so the
    // backup-only continuation completes deterministically.
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    assert!(
        !migration_journal_path(dir.path()).exists(),
        "a backup-only resume never invents a journal"
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        legacy.as_bytes(),
        "the backup is retained for audit"
    );
}

#[test]
fn managed_catalog_interrupt_after_local_resumes_from_the_journal() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));

    assert!(migration_journal_path(dir.path()).exists());
    assert!(local_catalog_path(dir.path()).exists());
    assert_eq!(
        read_text(&manifest_path(dir.path())),
        legacy,
        "the base was not published yet"
    );
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "the restart completes from the journal"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before,
        "no re-extraction: the verified local bytes are kept"
    );
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);

    // Third run: completed, idempotent.
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
}

#[test]
fn managed_catalog_interrupt_after_journal_recomputes_the_extraction() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_journal", || ensure_seeded(dir.path(), None));

    let catalog = catalog_dir(dir.path());
    assert!(catalog.join(MIGRATION_BACKUP_FILENAME).exists());
    assert!(migration_journal_path(dir.path()).exists());
    assert!(
        !local_catalog_path(dir.path()).exists(),
        "the local layer was not published"
    );
    assert_eq!(
        read_text(&manifest_path(dir.path())),
        legacy,
        "base untouched"
    );
    let backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
    assert_eq!(backup, legacy.as_bytes());

    // Restart: no local exists, so recovery RECOMPUTES the extraction from
    // the backup against the journal's saved managed base and verifies it
    // against the recorded local hash before publishing anything.
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert!(local_catalog_path(dir.path()).exists());
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        backup,
        "the backup is retained for audit"
    );
}

#[test]
fn managed_catalog_interrupt_before_base_resumes_from_the_journal() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("before_base", || ensure_seeded(dir.path(), None));
    assert!(migration_journal_path(dir.path()).exists());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);

    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
}

#[test]
fn managed_catalog_interrupt_after_base_completes_without_re_extraction() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_base", || ensure_seeded(dir.path(), None));
    assert!(migration_journal_path(dir.path()).exists());
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "a base matching the journal is already completed"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
    assert_eq!(keys_of(&load_catalog(dir.path()).unwrap()), ["mine"]);
}

#[test]
fn managed_catalog_interrupt_across_a_revision_change_completes_then_refreshes() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("before_base", || ensure_seeded(dir.path(), None));
    assert_eq!(
        migration_journal_json(dir.path())["managedBase"]["agents"]
            .as_array()
            .unwrap()
            .len(),
        8,
        "the journal saved the enabled shipped base"
    );

    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "the journal saved base is completed first"
        );
        let base = base_json(dir.path());
        assert_eq!(
            base["agents"].as_array().unwrap().len(),
            9,
            "then the normal verified refresh runs under the same lock"
        );
        assert!(
            base["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|agent| agent["key"] == "muse"),
            "the all-enabled refresh publishes muse"
        );
        assert_eq!(
            base["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
    });
}

#[test]
fn managed_catalog_interrupt_with_a_local_edit_blocks_recovery() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));
    let edited = r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"EDITED BETWEEN RUNS"}]}"##;
    write_local(dir.path(), edited);
    let entries_before = dir_entries(&catalog_dir(dir.path()));

    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&local_catalog_path(dir.path())), edited);
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(
        dir_entries(&catalog_dir(dir.path())),
        entries_before,
        "a conflicting recovery deletes nothing"
    );
    // The read stays readable on the legacy source and reports the pending
    // migration; the conflict itself blocks only continuation.
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
}

#[test]
fn managed_catalog_sidecar_collisions_block_without_cleanup() {
    let dir = seed_dir();
    let legacy = manifest_json(
        r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    write_legacy_base(dir.path(), &legacy);
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    std::fs::write(catalog.join(MIGRATION_BACKUP_FILENAME), b"foreign backup").unwrap();
    assert!(ensure_seeded(dir.path(), None).is_none());
    let entries_before = dir_entries(&catalog);
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(dir_entries(&catalog), entries_before);
    assert!(load_catalog(dir.path()).is_ok());

    // An invalid journal is also a conflict, and nothing is deleted.
    std::fs::remove_file(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
    std::fs::write(catalog.join(MIGRATION_JOURNAL_FILENAME), b"not json").unwrap();
    let entries_before = dir_entries(&catalog);
    assert!(ensure_seeded(dir.path(), None).is_none());
    assert_eq!(read_text(&manifest_path(dir.path())), legacy);
    assert_eq!(dir_entries(&catalog), entries_before);
}

#[test]
fn managed_catalog_reads_never_write_in_any_state() {
    // Managed base with local overrides.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"Mine"}]}"##,
    );
    assert_reads_leave_state_unchanged(dir.path());

    // Legacy base.
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    assert_reads_leave_state_unchanged(dir.path());

    // Absent base and no directory at all.
    let dir = seed_dir();
    assert_reads_leave_state_unchanged(dir.path());

    // Interrupted migration boundary (journal + local, base still legacy).
    let dir = seed_dir();
    write_legacy_base(
        dir.path(),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    let _ = with_failure_at("after_local", || ensure_seeded(dir.path(), None));
    assert_reads_leave_state_unchanged(dir.path());
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
}

#[test]
fn managed_catalog_no_embedded_commands_escape_a_failed_publication() {
    let project = seed_dir();
    let legacy = legacy_dir();
    std::fs::write(legacy.path().join("agents.json"), legacy_catalog_json()).unwrap();
    let _ = with_failure_at("publication_temp_synced", || {
        ensure_seeded(project.path(), Some(legacy.path()))
    });
    assert!(
        !manifest_path(project.path()).exists(),
        "no base was published"
    );
    assert!(!catalog_dir(project.path())
        .join(MIGRATION_JOURNAL_FILENAME)
        .exists());
    let report = load_catalog_report(project.path());
    assert!(report.unavailable.is_some());
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
            "embedded command leaked: {sentinel}"
        );
    }
    assert!(load_catalog(project.path()).is_err());
}

#[test]
fn managed_catalog_duplicate_command_identities_and_base_order_stay_intact() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"pi-max","label":"Pi Max","description":"d","color":"#ec4899","command":"pi","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi --custom"],"autoUpdate":false}]}"##,
    );
    let loaded = load_catalog(dir.path()).unwrap();
    let keys = keys_of(&loaded);
    assert_eq!(keys[0], "claude");
    assert_eq!(keys[4], "pi");
    assert_eq!(keys[5], "opencode");
    assert_eq!(keys[8], "pi-max", "local additions append after the base");
    let pi = loaded.iter().find(|d| d.key == "pi").unwrap();
    let pi_max = loaded.iter().find(|d| d.key == "pi-max").unwrap();
    assert_eq!(pi.update_commands, vec!["pi update".to_string()]);
    assert_eq!(pi_max.update_commands, vec!["pi --custom".to_string()]);
}

#[test]
fn managed_catalog_untracked_publication_is_retried_without_republishing() {
    let project = seed_dir();
    let root = project.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = ac_dir_for(&root);
    ensure_seeded(&ac_dir, None);
    let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
    assert!(
        !crate::config::seed_manifest::has_catalog_publication(&root).unwrap(),
        "the ungated seed recorded nothing"
    );

    let token = ManifestActivationToken::for_test();
    ensure_seeded_for_project_with_token(&root, Some(&token));
    assert!(
        crate::config::seed_manifest::has_catalog_publication(&root).unwrap(),
        "the verified current base is recorded on the next initialization"
    );
    assert_eq!(
        std::fs::read(manifest_path(&ac_dir)).unwrap(),
        base_before,
        "bookkeeping never republishes or rewrites the base"
    );
}

#[test]
fn managed_catalog_report_flags_an_untracked_verified_base() {
    let tracked = seed_dir();
    let root = tracked.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    ensure_seeded(&ac_dir_for(&root), None);
    let settings = AppSettings {
        project_paths: vec![root.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = report_json(&load_catalog_report_for_settings(&settings));
    assert!(report.contains("publicationUntracked"), "{report}");

    // A legacy base skips the bookkeeping query entirely.
    let legacy = seed_dir();
    write_legacy_base(
        &ac_dir_for(legacy.path()),
        &manifest_json(
            r##"[{"key":"mine","label":"Mine","description":"d","color":"#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]"##,
        ),
    );
    let settings = AppSettings {
        project_paths: vec![legacy.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = report_json(&load_catalog_report_for_settings(&settings));
    assert!(!report.contains("publicationUntracked"), "{report}");
}

// -----------------------------------------------------------------------
// R2 correction fixtures (F1 read warning, F3 OS failures, F5 stub)
// -----------------------------------------------------------------------

/// The canonical empty coverage-v2 manifest (the serializer's own bytes),
/// used as the repaired-manifest fixture.
const REPAIRED_EMPTY_MANIFEST: &[u8] = concat!(
    "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
    "schema_version = 1\n",
    "coverage_version = 2\n",
    "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
    "files = []\n",
)
.as_bytes();

/// Publication temporaries still present in the catalog directory.
fn temp_residue(ac_dir: &Path) -> Vec<String> {
    std::fs::read_dir(catalog_dir(ac_dir))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.contains(".tmp"))
                .collect()
        })
        .unwrap_or_default()
}

// ---- F1: read-path refresh warning with exact per-surface wording ------

#[test]
fn managed_catalog_stale_revision_warning_uses_the_exact_context_reason() {
    let agents = shipped_def_json(&["claude", "grok"]);

    // Direct: the public report wrapper keeps the neutral wording.
    let direct = seed_dir();
    write_managed_base(direct.path(), &agents, "stale-revision", true);
    let report = load_catalog_report(direct.path());
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("direct refreshFailed");
    assert_eq!(
        warning.reason,
        "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Catalog refresh runs during initialization."
    );
    assert_eq!(
        warning.path,
        manifest_path(direct.path()).display().to_string()
    );

    // Project: the settings project branch supplies the project wording.
    let project = seed_dir();
    write_managed_base(&ac_dir_for(project.path()), &agents, "stale-revision", true);
    let settings = AppSettings {
        project_paths: vec![project.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("project refreshFailed");
    assert_eq!(
        warning.reason,
        "The persisted managed catalog revision differs from this build. Current persisted entries remain usable. Restart retries project catalog refresh."
    );
    assert_eq!(
        warning.path,
        manifest_path(&ac_dir_for(project.path()))
            .display()
            .to_string()
    );

    // Instance: the no-project branch supplies the read-only wording
    // (deliberately NOT the public Direct wrapper).
    let instance = seed_dir();
    write_managed_base(instance.path(), &agents, "stale-revision", true);
    let report = load_catalog_report_for_settings_with_config_dir(
        &AppSettings::default(),
        Some(instance.path().to_path_buf()),
    );
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("instance refreshFailed");
    assert_eq!(
        warning.reason,
        "The persisted instance catalog revision differs from this build. Current persisted entries remain usable. Restart retries instance catalog refresh when no project is registered."
    );
    assert_eq!(
        warning.path,
        manifest_path(instance.path()).display().to_string()
    );
}

#[test]
fn managed_catalog_stale_revision_with_a_valid_pin_survives_repeated_reads() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"PINNED"}]}"##,
    );
    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    for _ in 0..3 {
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog[0].label, "PINNED");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "refreshFailed"));
    }
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before
    );
}

#[test]
fn managed_catalog_stale_revision_warning_clears_after_a_successful_restart() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "refreshFailed"));

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "the stale base refreshes on initialization"
    );
    let refreshed = load_catalog_report(dir.path());
    assert!(refreshed.unavailable.is_none());
    assert!(refreshed.warnings.is_empty(), "{:?}", refreshed.warnings);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
}

#[test]
fn managed_catalog_stale_revision_with_an_invalid_local_reports_both() {
    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"broken"}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1, "the verified base stays served");
    for code in ["refreshFailed", "localInvalid"] {
        assert!(
            report.warnings.iter().any(|warning| warning.code == code),
            "missing {code}: {:?}",
            report.warnings
        );
    }
}

#[test]
fn managed_catalog_non_stale_and_unverified_bases_never_promise_a_restart() {
    // A current verified base is silent and usable.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(!report_json(&report).contains("refreshFailed"));

    // Edited content keeps managedBaseEdited and adds no restart promise.
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("HAND EDITED");
    let bytes = write_managed_base(dir.path(), &[claude], "stale-revision", false);
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "managedBaseEdited"));
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code != "refreshFailed"));
    assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), bytes);

    // A foreign marker keeps migrationConflict and adds no restart promise.
    let dir = seed_dir();
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": shipped_def_json(&["claude"]),
        "managed": {
            "owner": "somebody-else",
            "version": 1,
            "revision": "stale-revision",
            "contentSha256": "y",
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationConflict"));
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code != "refreshFailed"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    // Unknown root fields block refresh and are preserved untouched.
    let dir = seed_dir();
    let definitions: Vec<CodingAgentDefinition> = shipped_def_json(&["claude"])
        .iter()
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    let content = managed_content_sha256(&definitions);
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": definitions,
        "futureRootField": { "keep": "me" },
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": "stale-revision",
            "contentSha256": content,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
    assert!(report
        .warnings
        .iter()
        .all(|warning| warning.code != "refreshFailed"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    // Corrupt bytes stay unavailable with no restart promise.
    let dir = seed_dir();
    std::fs::create_dir_all(catalog_dir(dir.path())).unwrap();
    std::fs::write(manifest_path(dir.path()), b"not a catalog at all").unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_some());
    assert!(!report_json(&report).contains("refreshFailed"));
}

// ---- F3: real OS failures and injected arm-level boundaries ------------

#[test]
fn managed_catalog_local_symlink_and_dangling_link_are_preserved_as_invalid() {
    // A real file symlink at the local path: the layer is disabled, the
    // link identity/target and the target bytes survive every read.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let target = dir.path().join("user-local-target.json");
    let target_bytes = br##"{"schemaVersion":1,"agents":[{"key":"claude","label":"LINKED"}]}"##;
    std::fs::write(&target, target_bytes).unwrap();
    let local = local_catalog_path(dir.path());
    std::fs::remove_file(&local).unwrap();
    create_manifest_symlink(&target, &local)
        .expect("real local symlink fixture (not a silent skip)");
    assert!(std::fs::symlink_metadata(&local)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&local).unwrap(), target);

    let base_before = std::fs::read(manifest_path(dir.path())).unwrap();
    for _ in 0..2 {
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none());
        assert_eq!(report.catalog.len(), 8, "the managed base stays usable");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "localInvalid"));
        assert!(!report.catalog.iter().any(|def| def.label == "LINKED"));
    }
    assert!(
        std::fs::symlink_metadata(&local)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is never replaced by a stub"
    );
    assert_eq!(std::fs::read_link(&local).unwrap(), target);
    assert_eq!(std::fs::read(&target).unwrap(), target_bytes);
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_before
    );

    // A dangling link: same degradation, link and absence preserved even
    // across a later initialization.
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let local = local_catalog_path(dir.path());
    std::fs::remove_file(&local).unwrap();
    let missing = dir.path().join("no-such-local-target.json");
    create_manifest_symlink(&missing, &local).expect("real dangling local link fixture");
    let report = load_catalog_report(dir.path());
    assert_eq!(report.catalog.len(), 8);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "localInvalid"));
    assert!(std::fs::symlink_metadata(&local)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&local).unwrap(), missing);
    assert!(!missing.exists());
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "the verified base needs no rewrite"
    );
    assert!(std::fs::symlink_metadata(&local)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!missing.exists());
}

#[test]
fn managed_catalog_hard_link_unsupported_leaves_prior_artifacts_and_resumes() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_bytes = legacy_catalog_json();
    std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();
    let base = manifest_path(project.path());
    let local = local_catalog_path(project.path());
    let backup = catalog_dir(project.path()).join(MIGRATION_BACKUP_FILENAME);
    let journal = migration_journal_path(project.path());

    // SIMULATED error result at the link boundary (arm-level proof); real
    // unsupported-filesystem execution is unmeasured. The fault is armed
    // for the BASE destination only, so the earlier backup, journal and
    // local publications run for real.
    arm_catalog_path_fault("hard_link", &base, std::io::ErrorKind::Unsupported);
    let outcome = run_catalog_initialization(project.path(), Some(legacy.path()), None);
    clear_catalog_path_faults();

    assert!(outcome.published_at.is_none(), "no base was published");
    let conflict = outcome
        .warnings
        .iter()
        .find(|warning| warning.code == "migrationConflict")
        .expect("the link failure surfaces");
    assert!(conflict.reason.contains("hard link"), "{}", conflict.reason);
    assert!(!base.exists(), "the destination stays absent");
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        legacy_bytes,
        "the backup keeps the source bytes exactly"
    );
    assert!(journal.is_file(), "the journal survives");
    let local_bytes = std::fs::read(&local).expect("the extracted local survives");
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes,
        "the source is never modified"
    );
    assert!(
        temp_residue(project.path()).is_empty(),
        "only this run's temp is removed: {:?}",
        temp_residue(project.path())
    );

    // Fault cleared: the interrupted transaction resumes and completes,
    // then a second initialization is idempotent.
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_some());
    let completed_base = std::fs::read(&base).unwrap();
    assert_eq!(
        std::fs::read(&local).unwrap(),
        local_bytes,
        "recovery never re-extracts or rewrites the local layer"
    );
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes
    );
    assert!(ensure_seeded(project.path(), Some(legacy.path())).is_none());
    assert_eq!(std::fs::read(&base).unwrap(), completed_base);
    let base_value: serde_json::Value = serde_json::from_slice(&completed_base).unwrap();
    assert_eq!(base_value["managed"]["owner"], "agentscommander");
}

#[test]
fn managed_catalog_exclusive_publication_collision_preserves_the_destination() {
    let scratch = seed_dir();
    let destination = scratch.path().join("agents.local.json");
    let sentinel = b"USER SENTINEL BYTES\n";
    std::fs::write(&destination, sentinel).unwrap();
    let source = scratch.path().join("source-bytes.json");
    std::fs::write(&source, b"REPLACEMENT CONTENT\n").unwrap();

    // The REAL (uninjected) hard link reports AlreadyExists on this pair;
    // the injected Unsupported arm covers the absent destination instead.
    let real = std::fs::hard_link(&source, &destination).unwrap_err();
    assert_eq!(real.kind(), std::io::ErrorKind::AlreadyExists);

    let error = publish_exclusive(&destination, b"new content").unwrap_err();
    assert!(error.contains("exclusive publication"), "{error}");
    assert_eq!(std::fs::read(&destination).unwrap(), sentinel);
    assert!(
        dir_entries(scratch.path())
            .iter()
            .all(|name| !name.to_string_lossy().contains(".tmp")),
        "no publication temp survives"
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"REPLACEMENT CONTENT\n");
}

#[cfg(windows)]
#[test]
fn managed_catalog_replace_share_denial_keeps_bytes_and_recovers() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let dir = seed_dir();
    write_managed_base(
        dir.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        true,
    );
    let base = manifest_path(dir.path());
    let base_before = std::fs::read(&base).unwrap();
    // A real destination handle that denies replacement: ReplaceFileW
    // fails through the production error path.
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&base)
        .unwrap();

    let outcome = run_catalog_initialization(dir.path(), None, None);
    assert!(outcome.published_at.is_none(), "ReplaceFileW must fail");
    assert!(
        outcome.warnings.iter().any(|warning| {
            warning.code == "refreshFailed" && warning.reason.contains("Failed to replace")
        }),
        "{:?}",
        outcome.warnings
    );
    assert_eq!(
        std::fs::read(&base).unwrap(),
        base_before,
        "the original bytes are unchanged"
    );
    assert!(
        temp_residue(dir.path()).is_empty(),
        "the failed publication removes its own temp: {:?}",
        temp_residue(dir.path())
    );

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(
        report.catalog.len(),
        1,
        "the stale verified base stays usable"
    );
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "refreshFailed"));

    // Release the denial: two restarts refresh once and then stay idempotent.
    drop(blocker);
    let refreshed = run_catalog_initialization(dir.path(), None, None);
    assert!(refreshed.published_at.is_some(), "release then refresh");
    let base_after = std::fs::read(&base).unwrap();
    let second = run_catalog_initialization(dir.path(), None, None);
    assert!(second.published_at.is_none(), "the refresh is idempotent");
    assert_eq!(std::fs::read(&base).unwrap(), base_after);
    assert_eq!(
        base_json(dir.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn managed_catalog_degraded_manifest_record_failure_repairs_without_republishing() {
    let temp = seed_dir();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join(".ac")).unwrap();
    let ac_dir = ac_dir_for(&root);
    let seed_manifest = root.join(".ac").join(SEED_MANIFEST_FILENAME);

    // A malformed canonical manifest is preserved read-only: the base still
    // publishes, but the row cannot be recorded (PublishedUnrecorded).
    std::fs::write(&seed_manifest, b"not = [valid toml").unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert!(
        manifest_path(&ac_dir).is_file(),
        "the base published anyway"
    );
    assert!(
        crate::config::seed_manifest::has_catalog_publication(&root).is_err(),
        "a malformed manifest is unreadable"
    );
    {
        let mut guard = ProjectSeedManifestGuard::acquire(&root)
            .expect("a malformed manifest is held read-only, not rejected");
        let row = PublishedManifestRow::coding_agent_catalog(
            ManifestPathIdentity::from_relative_path(Path::new(".ac/coding-agents/agents.json"))
                .unwrap(),
            Utc::now(),
        )
        .unwrap();
        let outcome = guard
            .publication_permit()
            .record_file(&ManifestActivationToken::for_test(), row);
        assert!(
            matches!(
                outcome,
                crate::config::seed_manifest::ManifestRecordOutcome::PublishedUnrecorded(_)
            ),
            "expected PublishedUnrecorded, got {outcome:?}"
        );
        guard.release();
    }

    let settings = AppSettings {
        project_paths: vec![root.to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert!(report.unavailable.is_none());
    assert!(!report.catalog.is_empty(), "the catalog stays usable");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "publicationUntracked"));
    let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
    let local_before = std::fs::read(local_catalog_path(&ac_dir)).unwrap();

    // Repair only the manifest: two initializations restore the row without
    // republishing the base or rewriting the local layer.
    std::fs::write(&seed_manifest, REPAIRED_EMPTY_MANIFEST).unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert!(crate::config::seed_manifest::has_catalog_publication(&root).unwrap());
    let manifest_after_repair = std::fs::read(&seed_manifest).unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert_eq!(
        std::fs::read(&seed_manifest).unwrap(),
        manifest_after_repair,
        "the second initialization neither re-records nor republishes"
    );
    assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_before);
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        local_before
    );
    let report = load_catalog_report_for_settings_with_config_dir(&settings, None);
    assert!(!report_json(&report).contains("publicationUntracked"));
}

#[test]
fn managed_catalog_reads_never_touch_an_interrupted_instance_source() {
    let project = seed_dir();
    let legacy = legacy_dir();
    let legacy_bytes = legacy_catalog_json();
    std::fs::write(legacy.path().join("agents.json"), &legacy_bytes).unwrap();

    // Stop the migration after the local layer: base absent, backup +
    // journal + local present, and the instance source still the only
    // donor the read may consult.
    let _ = with_failure_at("after_local", || {
        ensure_seeded(project.path(), Some(legacy.path()))
    });
    let catalog = catalog_dir(project.path());
    assert!(catalog.join(MIGRATION_BACKUP_FILENAME).is_file());
    assert!(migration_journal_path(project.path()).is_file());
    assert!(local_catalog_path(project.path()).is_file());
    assert!(!manifest_path(project.path()).exists());

    assert_reads_leave_state_unchanged(project.path());
    let report = load_catalog_report(project.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
    assert_eq!(
        std::fs::read(legacy.path().join("agents.10.default.json")).unwrap(),
        legacy_bytes,
        "reads never write the instance source; its bytes survive the rename at the new name"
    );
    assert!(!legacy.path().join("agents.json").exists());
}

// ---- F5: transient stub failure, no persistent state -------------------

#[test]
fn managed_catalog_stub_failure_is_nonfatal_and_never_retried() {
    let temp = seed_dir();
    let root = temp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let ac_dir = ac_dir_for(&root);
    let local = local_catalog_path(&ac_dir);

    // Destination-aware fault at the STUB publication only: the base link
    // runs for real because it targets a different path.
    arm_catalog_path_fault("hard_link", &local, std::io::ErrorKind::Unsupported);
    let outcome = run_catalog_initialization(&ac_dir, None, None);
    clear_catalog_path_faults();

    assert!(
        outcome.published_at.is_some(),
        "the base publication succeeds"
    );
    let canonical_local = std::fs::canonicalize(catalog_dir(&ac_dir))
        .unwrap()
        .join(LOCAL_CATALOG_FILENAME);
    let warning = outcome
        .warnings
        .iter()
        .find(|warning| warning.code == "refreshFailed")
        .expect("the transient stub warning");
    assert_eq!(warning.path, canonical_local.display().to_string());
    assert!(
        warning.reason.starts_with(
            "The managed catalog base is usable, but its local overrides stub could not be created: "
        ),
        "{}",
        warning.reason
    );
    assert!(
        warning
            .reason
            .ends_with("An absent local file is valid; no automatic stub retry is scheduled."),
        "{}",
        warning.reason
    );
    assert!(!local.exists(), "the stub stays absent");
    assert_eq!(
        load_catalog(&ac_dir).unwrap().len(),
        8,
        "the base stays usable"
    );
    assert!(
        temp_residue(&ac_dir).is_empty(),
        "{:?}",
        temp_residue(&ac_dir)
    );

    // No persistent report warning: the base is already at this revision.
    let report = load_catalog_report(&ac_dir);
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    // Bookkeeping still records the verified current base without
    // republishing it, and a later initialization leaves the absent local
    // absent (no creation-on-every-start).
    let base_before = std::fs::read(manifest_path(&ac_dir)).unwrap();
    ensure_seeded_for_project_with_token(&root, Some(&ManifestActivationToken::for_test()));
    assert!(crate::config::seed_manifest::has_catalog_publication(&root).unwrap());
    assert_eq!(std::fs::read(manifest_path(&ac_dir)).unwrap(), base_before);
    assert!(!local.exists());
    let _ = run_catalog_initialization(&ac_dir, None, None);
    assert!(!local.exists());
}

#[test]
fn managed_catalog_lock_timeout_is_bounded_and_named() {
    assert_eq!(CATALOG_LOCK_TIMEOUT, Duration::from_secs(5));
    let dir = seed_dir();
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let canonical = std::fs::canonicalize(&catalog).unwrap();
    let paths = CatalogPaths::new(&canonical);
    let held = acquire_catalog_lock(&paths).unwrap();

    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(150))));
    let started = Instant::now();
    let error = acquire_catalog_lock(&paths).unwrap_err();
    let elapsed = started.elapsed();
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));

    assert!(error.contains("catalogLockTimeout"), "{error}");
    assert!(error.contains(CATALOG_LOCK_FILENAME), "{error}");
    assert!(error.contains("150 ms"), "{error}");
    assert!(
        elapsed >= Duration::from_millis(150) && elapsed < Duration::from_secs(2),
        "bounded wait, elapsed {elapsed:?}"
    );
    drop(held);
    let reacquired = acquire_catalog_lock(&paths).unwrap();
    drop(reacquired);
    assert!(paths.lock.is_file(), "the lock file is never removed");
}

const LOCK_CHILD_ACTION_ENV: &str = "AC_1968_CATALOG_LOCK_CHILD_ACTION";
const LOCK_CHILD_DIR_ENV: &str = "AC_1968_CATALOG_LOCK_CHILD_DIR";
const LOCK_CHILD_TEST_FQN: &str =
    "config::coding_agents_catalog::tests::managed_catalog_lock_child";

fn wait_for_path(path: &Path, timeout: Duration, label: &str) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < timeout,
            "{label}: timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_catalog_lock_child(dir: &Path) -> (PathBuf, std::process::Child) {
    let canonical = std::fs::canonicalize(dir).expect("canonical catalog dir");
    let exe = std::env::current_exe().expect("current test exe");
    let mut command = std::process::Command::new(exe);
    command
        .args([
            "--exact",
            LOCK_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LOCK_CHILD_ACTION_ENV, "hold")
        .env(LOCK_CHILD_DIR_ENV, &canonical)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().expect("spawn catalog lock child");
    wait_for_path(
        &canonical.join("child-ready"),
        Duration::from_secs(30),
        "catalog lock child",
    );
    (canonical, child)
}

#[test]
fn managed_catalog_lock_child() {
    let Some(action) = std::env::var_os(LOCK_CHILD_ACTION_ENV) else {
        return;
    };
    let action = action.to_string_lossy().into_owned();
    let dir = PathBuf::from(std::env::var_os(LOCK_CHILD_DIR_ENV).expect("child dir env"));
    let paths = CatalogPaths::new(&dir);
    match action.as_str() {
        "hold" => {
            let _lock = acquire_catalog_lock(&paths).expect("child must acquire the lock");
            std::fs::write(dir.join("child-ready"), b"ready").expect("child announces ready");
            let deadline = Instant::now() + Duration::from_secs(60);
            while !dir.join("child-release").exists() {
                assert!(
                    Instant::now() < deadline,
                    "child hold exceeded its 60s bound"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        other => panic!("unknown child action {other}"),
    }
    println!("AC_1968_CATALOG_LOCK_CHILD_DONE action={action}");
}

#[test]
fn managed_catalog_process_lock_contention_times_out_and_recovers() {
    let dir = seed_dir();
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let canonical = std::fs::canonicalize(&catalog).unwrap();
    let (held_dir, mut child) = spawn_catalog_lock_child(&canonical);
    let paths = CatalogPaths::new(&held_dir);

    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_millis(250))));
    let error = acquire_catalog_lock(&paths).unwrap_err();
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
    assert!(error.contains("catalogLockTimeout"), "{error}");

    std::fs::write(held_dir.join("child-release"), b"release").unwrap();
    let started = Instant::now();
    loop {
        match acquire_catalog_lock(&paths) {
            Ok(_) => break,
            Err(error) => {
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "the released child lock must become acquirable: {error}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let status = child.wait().expect("reap the lock child");
    assert!(status.success(), "the lock child must exit cleanly");
}

#[test]
fn managed_catalog_process_lock_holder_death_releases_the_lock() {
    let dir = seed_dir();
    let catalog = catalog_dir(dir.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let canonical = std::fs::canonicalize(&catalog).unwrap();
    let exe = std::env::current_exe().expect("current test exe");
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            LOCK_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LOCK_CHILD_ACTION_ENV, "hold")
        .env(LOCK_CHILD_DIR_ENV, &canonical)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn catalog lock child");
    wait_for_path(
        &canonical.join("child-ready"),
        Duration::from_secs(30),
        "catalog lock child",
    );
    child.kill().expect("kill the lock holder");
    let _ = child.wait();

    let paths = CatalogPaths::new(&canonical);
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(Some(Duration::from_secs(3))));
    let acquired = acquire_catalog_lock(&paths);
    CATALOG_LOCK_TIMEOUT_OVERRIDE.with(|cell| cell.set(None));
    assert!(
        acquired.is_ok(),
        "a crashed holder releases the OS lock: {acquired:?}"
    );
}

// ---- #2715 (B2): the `agents.*` family rename -------------------------

const OLD_BASE: &str = "agents.json";
const OLD_LOCAL: &str = "agents.local.json";

/// A project root plus a config dir that holds the journal and the legacy
/// instance catalog, both fixtures the test controls.
struct B2Fixture {
    project: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl B2Fixture {
    fn new() -> Self {
        let fixture = B2Fixture {
            project: tempfile::tempdir().expect("project tempdir"),
            config: tempfile::tempdir().expect("config tempdir"),
        };
        std::fs::create_dir_all(fixture.project_catalog()).unwrap();
        fixture
    }
    fn root(&self) -> &Path {
        self.project.path()
    }
    fn ac_dir(&self) -> PathBuf {
        self.root()
            .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
    }
    fn project_catalog(&self) -> PathBuf {
        catalog_dir(&self.ac_dir())
    }
    fn journal_dir(&self) -> &Path {
        self.config.path()
    }
    fn legacy(&self) -> PathBuf {
        self.config.path().join(CATALOG_DIR_NAME)
    }
    fn seed(&self) {
        ensure_seeded_for_project_in(
            self.root(),
            None,
            Some(&self.legacy()),
            Some(self.journal_dir()),
        );
    }
    fn project_key(&self) -> String {
        catalog_scope_key(&std::fs::canonicalize(self.project_catalog()).unwrap())
    }
    fn legacy_key(&self) -> String {
        catalog_scope_key(&std::fs::canonicalize(self.legacy()).unwrap())
    }
    fn record(&self, key: &str) -> naming_migration::ScopeRecord {
        naming_migration::read_journal(Some(self.journal_dir()))
            .expect("journal readable")
            .expect("journal exists")
            .scope(key)
            .cloned()
            .unwrap_or_else(|| panic!("scope {key} recorded"))
    }
}

/// A managed base carrying a recognisable extra agent: an EDITED managed
/// base, so no initialization pass ever refreshes, migrates or replaces it.
fn preserved_base_bytes(key: &str) -> Vec<u8> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&build_managed_base_bytes(&supported_shipped_definitions()))
            .unwrap();
    value["agents"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "key": key, "label": key, "description": "d", "color": "#111",
            "command": key, "envs": [], "isolatedHome": false, "removable": true
        }));
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn local_bytes(tag: &str) -> Vec<u8> {
    format!("{{\"schemaVersion\":1,\"agents\":[],\"_tag\":\"{tag}\"}}\n").into_bytes()
}

fn scope_keys(journal_dir: &Path) -> Vec<String> {
    let journal = naming_migration::read_journal(Some(journal_dir))
        .unwrap()
        .expect("journal exists");
    let value = serde_json::to_value(&journal).unwrap();
    value["scopes"]
        .as_object()
        .map(|scopes| scopes.keys().cloned().collect())
        .unwrap_or_default()
}

fn deprecated_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.contains(".deprecated-"))
        .collect();
    names.sort();
    names
}

/// Leaves a genuinely interrupted #1968 project migration under the
/// PRE-migration names: the transaction stops at `point`.
fn interrupted_1968_migration(catalog: &Path, point: &'static str) -> Vec<u8> {
    let source = legacy_catalog_json();
    std::fs::write(catalog.join(OLD_BASE), &source).unwrap();
    let old = CatalogPaths::with_names(catalog, OLD_BASE, OLD_LOCAL);
    CATALOG_FAILURE_POINT.with(|cell| cell.set(Some(point)));
    let result = migrate_legacy(&old, MIGRATION_SOURCE_PROJECT, &old.base, &source);
    CATALOG_FAILURE_POINT.with(|cell| cell.set(None));
    assert!(
        result.is_err(),
        "the fixture migration must stop at {point}"
    );
    source
}

fn local_carries(path: &Path, key: &str) -> bool {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    value["agents"]
        .as_array()
        .is_some_and(|agents| agents.iter().any(|agent| agent["key"] == key))
}

#[test]
fn the_migration_runs_before_the_base_is_seeded_in_one_locked_pass() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let user = preserved_base_bytes("mine");
    std::fs::write(catalog.join(OLD_BASE), &user).unwrap();

    fx.seed();

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        user,
        "the user's catalog survives at the new name; no fresh base was written"
    );
    assert!(!catalog.join(OLD_BASE).exists());
    assert!(
        deprecated_entries(&catalog).is_empty(),
        "nothing was set aside"
    );
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );
}

#[test]
fn every_seeding_entry_point_migrates_before_it_creates() {
    // ensure_seeded_for_project_in, gated.
    let fx = B2Fixture::new();
    let user = preserved_base_bytes("mine");
    std::fs::write(fx.project_catalog().join(OLD_BASE), &user).unwrap();
    let token = ManifestActivationToken::for_test();
    ensure_seeded_for_project_in(
        fx.root(),
        Some(&token),
        Some(&fx.legacy()),
        Some(fx.journal_dir()),
    );
    // ensure_seeded_for_project_in, activation None.
    let plain = B2Fixture::new();
    std::fs::write(plain.project_catalog().join(OLD_BASE), &user).unwrap();
    plain.seed();
    for (label, catalog) in [
        ("gated", fx.project_catalog()),
        ("activation None", plain.project_catalog()),
    ] {
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            user,
            "{label}"
        );
        assert!(!catalog.join(OLD_BASE).exists(), "{label}");
        assert!(deprecated_entries(&catalog).is_empty(), "{label}");
    }
    // ensure_seeded_instance_in.
    let instance = tempfile::tempdir().unwrap();
    let catalog = catalog_dir(instance.path());
    std::fs::create_dir_all(&catalog).unwrap();
    std::fs::write(catalog.join(OLD_BASE), &user).unwrap();
    ensure_seeded_instance_in(instance.path(), Some(instance.path()));
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        user,
        "instance"
    );
    assert!(!catalog.join(OLD_BASE).exists(), "instance");
    assert!(deprecated_entries(&catalog).is_empty(), "instance");
}

#[test]
fn a_recoverable_sidecar_recovers_then_renames_in_one_pass() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    interrupted_1968_migration(&catalog, "after_local");
    let backup = std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap();
    let journal = std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap();

    fx.seed();

    assert!(!catalog.join(OLD_BASE).exists());
    assert!(!catalog.join(OLD_LOCAL).exists());
    let base: serde_json::Value =
        serde_json::from_slice(&std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap())
            .unwrap();
    assert_eq!(
        base["managed"]["owner"], "agentscommander",
        "the recovered base"
    );
    assert!(
        local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
        "the recovered agent is at the new local name"
    );
    let report = load_catalog_report(&fx.ac_dir());
    assert!(
        report.catalog.iter().any(|agent| agent.key == "mine"),
        "the effective catalog loaded from disk serves the recovered agent: {:?}",
        report.warnings
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
        backup
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
        journal
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    for sidecar in [MIGRATION_BACKUP_FILENAME, MIGRATION_JOURNAL_FILENAME] {
        assert!(
            record.notes.iter().any(|note| note.contains(sidecar)),
            "{sidecar} noted: {:?}",
            record.notes
        );
    }
    assert!(deprecated_entries(&catalog).is_empty());
}

/// E8e: data already at the new names beside a #1968 sidecar, with no
/// journal dir. F1 must not resurrect the old names; without its
/// `has_old_data` guard the set-aside ordinal grows by one every pass.
#[test]
fn a_sidecar_without_old_data_runs_no_recovery() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    interrupted_1968_migration(&catalog, "after_local");
    // Finish the data move by hand: the old names are gone.
    std::fs::rename(
        catalog.join(OLD_BASE),
        catalog.join(CATALOG_MANIFEST_FILENAME),
    )
    .unwrap();
    std::fs::rename(
        catalog.join(OLD_LOCAL),
        catalog.join(LOCAL_CATALOG_FILENAME),
    )
    .unwrap();
    assert!(catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    assert!(catalog.join(MIGRATION_BACKUP_FILENAME).exists());
    for pass in 1..=2 {
        ensure_seeded_for_project_in(fx.root(), None, None, None);
        assert!(
            !catalog.join(OLD_BASE).exists(),
            "pass {pass}: agents.json recreated"
        );
        assert!(
            !catalog.join(OLD_LOCAL).exists(),
            "pass {pass}: agents.local.json recreated"
        );
        assert_eq!(
            deprecated_entries(&catalog),
            Vec::<String>::new(),
            "pass {pass}: nothing set aside"
        );
    }
    fx.seed();
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );
    assert!(deprecated_entries(&catalog).is_empty());
}

#[test]
fn a_blocked_recovery_defers_and_renames_nothing() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let source = interrupted_1968_migration(&catalog, "after_local");
    let backup_path = catalog.join(MIGRATION_BACKUP_FILENAME);
    let backup = std::fs::read(&backup_path).unwrap();
    let local = std::fs::read(catalog.join(OLD_LOCAL)).unwrap();
    std::fs::remove_file(&backup_path).unwrap();

    let outcome = run_catalog_initialization_with_scope(
        &fx.ac_dir(),
        Some(&fx.legacy()),
        Some(fx.journal_dir()),
        InitScope::Project,
    );
    assert!(outcome
        .warnings
        .iter()
        .any(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT));
    assert_eq!(std::fs::read(catalog.join(OLD_BASE)).unwrap(), source);
    assert_eq!(std::fs::read(catalog.join(OLD_LOCAL)).unwrap(), local);
    assert!(
        !catalog.join(CATALOG_MANIFEST_FILENAME).exists(),
        "nothing seeded"
    );
    assert!(
        !catalog.join(LOCAL_CATALOG_FILENAME).exists(),
        "nothing renamed"
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Deferred);
    assert!(
        record.notes.iter().any(|note| note.contains("backup")),
        "{:?}",
        record.notes
    );

    // Restoring the backup heals it with no journal surgery.
    std::fs::write(&backup_path, &backup).unwrap();
    fx.seed();
    assert!(!catalog.join(OLD_BASE).exists());
    assert!(local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"));
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );

    // Backup only (follow-up #2709): both renames done, every byte kept,
    // and the pass ends with the one pre-existing MIGRATION_CONFLICT.
    let only = B2Fixture::new();
    let catalog = only.project_catalog();
    let source = interrupted_1968_migration(&catalog, "after_backup");
    assert!(!catalog.join(MIGRATION_JOURNAL_FILENAME).exists());
    for pass in 1..=2 {
        let outcome = run_catalog_initialization_with_scope(
            &only.ac_dir(),
            Some(&only.legacy()),
            Some(only.journal_dir()),
            InitScope::Project,
        );
        let conflicts: Vec<_> = outcome
            .warnings
            .iter()
            .filter(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
            .collect();
        assert_eq!(conflicts.len(), 1, "pass {pass}: {:?}", outcome.warnings);
        assert!(!catalog.join(OLD_BASE).exists(), "pass {pass}");
        assert!(!catalog.join(OLD_LOCAL).exists(), "pass {pass}");
        assert_eq!(
            std::fs::read(catalog.join(MIGRATION_BACKUP_FILENAME)).unwrap(),
            source,
            "pass {pass}"
        );
        let base: serde_json::Value = serde_json::from_slice(
            &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(base["managed"]["owner"], "agentscommander", "pass {pass}");
        assert!(
            local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
            "pass {pass}"
        );
        assert!(deprecated_entries(&catalog).is_empty(), "pass {pass}");
        assert_eq!(
            only.record(&only.project_key()).status,
            naming_migration::ScopeStatus::Complete,
            "pass {pass}"
        );
    }
}

#[test]
fn the_instance_directory_never_resumes_a_sidecar() {
    let config = tempfile::tempdir().unwrap();
    let catalog = catalog_dir(config.path());
    std::fs::create_dir_all(&catalog).unwrap();
    let base = legacy_catalog_json();
    let local = local_bytes("instance-local");
    let journal = b"{\"not\":\"ours\"}".to_vec();
    std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();
    std::fs::write(catalog.join(MIGRATION_JOURNAL_FILENAME), &journal).unwrap();

    ensure_seeded_instance_in(config.path(), Some(config.path()));

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert_eq!(
        std::fs::read(catalog.join(MIGRATION_JOURNAL_FILENAME)).unwrap(),
        journal
    );
    assert!(
        !catalog.join(MIGRATION_BACKUP_FILENAME).exists(),
        "no recovery ran"
    );
    let key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
    let record = naming_migration::read_journal(Some(config.path()))
        .unwrap()
        .unwrap()
        .scope(&key)
        .cloned()
        .unwrap();
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .notes
        .iter()
        .any(|note| note.contains(MIGRATION_JOURNAL_FILENAME)));
}

#[test]
fn happy_path_is_byte_preserving_and_idempotent() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let base = preserved_base_bytes("mine");
    let local = local_bytes("happy");
    std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();
    std::fs::write(catalog.join(RETIRED_CATALOG_LOCK_FILENAME), b"").unwrap();

    fx.seed();
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert!(
        catalog.join(RETIRED_CATALOG_LOCK_FILENAME).is_file(),
        "lock not renamed"
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .notes
        .iter()
        .any(|note| note.contains(RETIRED_CATALOG_LOCK_FILENAME)));

    let before = fx.record(&fx.project_key());
    fx.seed();
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert_eq!(
        fx.record(&fx.project_key()),
        before,
        "a second run is a no-op"
    );
    assert!(deprecated_entries(&catalog).is_empty());
}

#[test]
fn generated_project_policy_tracks_project_catalog_and_ignores_personal() {
    let temp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .output()
            .expect("git runs")
    };
    let init = git(&["init", "--quiet"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let ac = temp.path().join(".ac");
    std::fs::create_dir_all(ac.join("coding-agents")).unwrap();
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[]).unwrap();
    for (name, expected) in [
        (
            crate::config::instance_artifacts::CODING_AGENTS_PROJECT_TARGET_NAME,
            1,
        ),
        (LOCAL_CATALOG_FILENAME, 0),
    ] {
        let relative = format!(".ac/coding-agents/{name}");
        std::fs::write(temp.path().join(&relative), b"user owned").unwrap();
        let result = git(&["check-ignore", "-q", &relative]);
        assert_eq!(
            result.status.code(),
            Some(expected),
            "{relative}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let result = git(&["check-ignore", "-q", ".ac/.gitignore"]);
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn a_pre_existing_target_wins_and_the_old_catalog_is_set_aside() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let old = preserved_base_bytes("old");
    let winner = preserved_base_bytes("winner");
    std::fs::write(catalog.join(OLD_BASE), &old).unwrap();
    std::fs::write(catalog.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(
        &fx.ac_dir(),
        &["CLAUDE.md".to_string(), "AGENTS.md".to_string()],
    )
    .unwrap();

    fx.seed();

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        winner
    );
    assert!(!catalog.join(OLD_BASE).exists());
    let set_aside = catalog.join("agents.json.deprecated-1.no-git");
    assert_eq!(std::fs::read(&set_aside).unwrap(), old);
    assert_eq!(
        deprecated_entries(&catalog),
        ["agents.json.deprecated-1.no-git"]
    );
    let record = fx.record(&fx.project_key());
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .steps
        .iter()
        .any(|step| step.from == OLD_BASE && step.state == naming_migration::StepState::SetAside));

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(fx.root())
            .output()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).status.success());
    let ignored = git(&[
        "check-ignore",
        "-q",
        ".ac/coding-agents/agents.json.deprecated-1.no-git",
    ]);
    assert!(ignored.status.success(), "the set-aside catalog is ignored");
    let tracked = git(&[
        "check-ignore",
        "-q",
        ".ac/coding-agents/agents.10.default.json",
    ]);
    assert_eq!(
        tracked.status.code(),
        Some(1),
        "the new base stays trackable"
    );
}

#[test]
fn an_unreachable_catalog_dir_is_recorded_and_retried() {
    let journal = tempfile::tempdir().unwrap();
    let base = preserved_base_bytes("mine");
    // A root that does not exist, and one that is a file: skipped before
    // the chokepoint, no scope row, nothing Complete.
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("missing");
    let file_root = parent.path().join("file-root");
    std::fs::write(&file_root, b"x").unwrap();
    for root in [&missing, &file_root] {
        ensure_seeded_for_project_in(root, None, None, Some(journal.path()));
    }
    assert!(naming_migration::read_journal(Some(journal.path()))
        .unwrap()
        .is_none());
    // A root whose `.ac` cannot be created: recorded Unreachable.
    let blocked = tempfile::tempdir().unwrap();
    let ac = blocked
        .path()
        .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
    std::fs::write(&ac, b"not a directory").unwrap();
    ensure_seeded_for_project_in(blocked.path(), None, None, Some(journal.path()));
    let keys = scope_keys(journal.path());
    assert_eq!(keys.len(), 1, "{keys:?}");
    let journal_now = naming_migration::read_journal(Some(journal.path()))
        .unwrap()
        .unwrap();
    assert_eq!(
        naming_migration::status(&journal_now, &keys[0]),
        Some(naming_migration::ScopeStatus::Unreachable)
    );

    // Each made reachable migrates on the next pass.
    std::fs::create_dir_all(missing.join(".ac").join(CATALOG_DIR_NAME)).unwrap();
    std::fs::remove_file(&file_root).unwrap();
    std::fs::create_dir_all(file_root.join(".ac").join(CATALOG_DIR_NAME)).unwrap();
    std::fs::remove_file(&ac).unwrap();
    std::fs::create_dir_all(ac.join(CATALOG_DIR_NAME)).unwrap();
    for root in [missing.as_path(), file_root.as_path(), blocked.path()] {
        let catalog = catalog_dir(&root.join(".ac"));
        std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
        ensure_seeded_for_project_in(root, None, None, Some(journal.path()));
        assert_eq!(
            std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            base,
            "{}",
            root.display()
        );
        let key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
        let journal_now = naming_migration::read_journal(Some(journal.path()))
            .unwrap()
            .unwrap();
        assert_eq!(
            naming_migration::status(&journal_now, &key),
            Some(naming_migration::ScopeStatus::Complete)
        );
    }
}

#[test]
fn a_new_project_still_imports_the_legacy_instance_catalog() {
    let registered = B2Fixture::new();
    let legacy = registered.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    let instance = legacy_catalog_json();
    std::fs::write(legacy.join(OLD_BASE), &instance).unwrap();
    // A project already registered, with its own catalog.
    std::fs::write(
        registered.project_catalog().join(OLD_BASE),
        preserved_base_bytes("registered"),
    )
    .unwrap();
    registered.seed();

    // A second project created later, same instance.
    let second = tempfile::tempdir().unwrap();
    ensure_seeded_for_project_in(
        second.path(),
        None,
        Some(&legacy),
        Some(registered.journal_dir()),
    );
    let catalog = catalog_dir(&second.path().join(".ac"));
    assert!(
        local_carries(&catalog.join(LOCAL_CATALOG_FILENAME), "mine"),
        "the new project imported the real instance catalog"
    );
    assert_eq!(
        std::fs::read(legacy.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        instance
    );
    assert!(!legacy.join(OLD_BASE).exists());
    for key in [registered.legacy_key(), registered.project_key()] {
        assert_eq!(
            registered.record(&key).status,
            naming_migration::ScopeStatus::Complete
        );
    }
    let second_key = catalog_scope_key(&std::fs::canonicalize(&catalog).unwrap());
    assert_eq!(
        registered.record(&second_key).status,
        naming_migration::ScopeStatus::Complete
    );
}

#[test]
fn a_failed_legacy_scope_seeds_nothing() {
    let fx = B2Fixture::new();
    let legacy = fx.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    let instance = legacy_catalog_json();
    std::fs::write(legacy.join(OLD_BASE), &instance).unwrap();
    // The new lock sidecar is a directory: the scope's lock open fails
    // with an I/O refusal.
    std::fs::create_dir(legacy.join(CATALOG_LOCK_FILENAME)).unwrap();

    let outcome = run_catalog_initialization_with_scope(
        &fx.ac_dir(),
        Some(&legacy),
        Some(fx.journal_dir()),
        InitScope::Project,
    );
    let conflict = outcome
        .warnings
        .iter()
        .find(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
        .expect("a MIGRATION_CONFLICT outcome");
    assert!(conflict.reason.contains(OLD_BASE), "{}", conflict.reason);
    assert!(
        conflict.reason.contains(CATALOG_MANIFEST_FILENAME),
        "{}",
        conflict.reason
    );
    let catalog = fx.project_catalog();
    assert!(
        !catalog.join(CATALOG_MANIFEST_FILENAME).exists(),
        "no base written"
    );
    assert!(!catalog.join(LOCAL_CATALOG_FILENAME).exists());
    assert_eq!(std::fs::read(legacy.join(OLD_BASE)).unwrap(), instance);
    assert!(!legacy.join(CATALOG_MANIFEST_FILENAME).exists());

    // Both names present in the legacy dir: the winner is imported.
    let both = B2Fixture::new();
    let legacy = both.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join(OLD_BASE), legacy_catalog_json()).unwrap();
    let winner = manifest_json(
        r##"[{"key":"winner","label":"Winner","description":"d","color":"#111","command":"win","envs":[],"isolatedHome":false,"removable":true}]"##,
    );
    std::fs::write(legacy.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
    both.seed();
    assert_eq!(
        std::fs::read_to_string(legacy.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        winner
    );
    assert_eq!(
        deprecated_entries(&legacy),
        ["agents.json.deprecated-1.no-git"]
    );
    assert!(local_carries(
        &both.project_catalog().join(LOCAL_CATALOG_FILENAME),
        "winner"
    ));
}

#[test]
fn the_two_catalog_locks_never_nest() {
    let fx = B2Fixture::new();
    let legacy = fx.legacy();
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join(OLD_BASE), legacy_catalog_json()).unwrap();
    let canonical_legacy = std::fs::canonicalize(&legacy).unwrap();
    let project_catalog = std::fs::canonicalize(fx.project_catalog()).unwrap();
    let project_paths = CatalogPaths::new(&project_catalog);
    let armed = naming_migration::pause::arm("before_rename", &canonical_legacy);

    let root = fx.root().to_path_buf();
    let journal = fx.journal_dir().to_path_buf();
    let worker = std::thread::spawn(move || {
        ensure_seeded_for_project_in(&root, None, Some(&legacy), Some(&journal));
    });
    armed
        .reached
        .recv_timeout(Duration::from_secs(30))
        .expect("the legacy scope reached its rename");
    // Leg 1: the project's section has not started.
    assert!(
        !project_paths.lock.exists(),
        "acquire_catalog_lock for the project has not run"
    );
    // Leg 2: the project lock is free while the legacy scope is held.
    let probe = acquire_catalog_lock(&project_paths);
    assert!(probe.is_ok(), "the project lock is free: {:?}", probe.err());
    drop(probe);
    armed.release.send(()).unwrap();
    worker.join().expect("worker completes");
    assert!(project_catalog.join(CATALOG_MANIFEST_FILENAME).exists());
}

#[test]
fn a_recovered_publication_is_still_recorded() {
    for point in ["after_local", "after_backup"] {
        // The pass's outcome carries the recovery's instant.
        let fx = B2Fixture::new();
        interrupted_1968_migration(&fx.project_catalog(), point);
        let outcome = run_catalog_initialization_with_scope(
            &fx.ac_dir(),
            Some(&fx.legacy()),
            Some(fx.journal_dir()),
            InitScope::Project,
        );
        assert!(
            outcome.published_at.is_some(),
            "{point}: {:?}",
            outcome.warnings
        );
        let conflicts = outcome
            .warnings
            .iter()
            .filter(|warning| warning.code == REPORT_CODE_MIGRATION_CONFLICT)
            .count();
        assert_eq!(conflicts, usize::from(point == "after_backup"), "{point}");

        // Under the held project gate it is recorded exactly once.
        let gated = B2Fixture::new();
        interrupted_1968_migration(&gated.project_catalog(), point);
        let token = ManifestActivationToken::for_test();
        ensure_seeded_for_project_in(
            gated.root(),
            Some(&token),
            Some(&gated.legacy()),
            Some(gated.journal_dir()),
        );
        assert!(
            has_catalog_publication(gated.root()).unwrap(),
            "{point}: the publication row was recorded"
        );
        let manifest =
            std::fs::read_to_string(gated.ac_dir().join(SEED_MANIFEST_FILENAME)).unwrap();
        assert_eq!(
            manifest.matches("kind = \"coding_agent_catalog\"").count(),
            1,
            "{point}: {manifest}"
        );
    }
}

#[test]
fn the_migration_does_not_re_acquire_the_catalog_lock() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    std::fs::write(catalog.join(OLD_BASE), preserved_base_bytes("mine")).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), local_bytes("lock")).unwrap();
    let started = Instant::now();
    run_catalog_initialization_with_scope(
        &fx.ac_dir(),
        None,
        Some(fx.journal_dir()),
        InitScope::Project,
    );
    assert!(started.elapsed() < naming_migration::MIGRATION_LOCK_BUDGET);
    assert!(catalog.join(CATALOG_MANIFEST_FILENAME).is_file());
    assert!(catalog.join(LOCAL_CATALOG_FILENAME).is_file());
    assert!(!catalog.join(OLD_BASE).exists());
    assert!(!catalog.join(OLD_LOCAL).exists());
}

const B2_CHILD_ROOT_ENV: &str = "AC_2715_CATALOG_CHILD_ROOT";
const B2_CHILD_JOURNAL_ENV: &str = "AC_2715_CATALOG_CHILD_JOURNAL";
const B2_CHILD_TEST_FQN: &str = "config::coding_agents_catalog::tests::b2_catalog_migration_child";

#[test]
fn b2_catalog_migration_child() {
    let (Some(root), Some(journal)) = (
        std::env::var_os(B2_CHILD_ROOT_ENV),
        std::env::var_os(B2_CHILD_JOURNAL_ENV),
    ) else {
        return;
    };
    ensure_seeded_for_project_in(Path::new(&root), None, None, Some(Path::new(&journal)));
    println!("AC_2715_CATALOG_CHILD_DONE");
}

#[test]
fn two_catalog_scopes_keep_both_record_sets() {
    let journal = tempfile::tempdir().unwrap();
    let rendezvous = tempfile::tempdir().unwrap();
    let projects = [B2Fixture::new(), B2Fixture::new()];
    let bytes = [
        preserved_base_bytes("first"),
        preserved_base_bytes("second"),
    ];
    for (fx, base) in projects.iter().zip(&bytes) {
        std::fs::write(fx.project_catalog().join(OLD_BASE), base).unwrap();
        std::fs::write(fx.project_catalog().join(OLD_LOCAL), local_bytes("two")).unwrap();
    }
    // The child migrates the first project and pauses mid-scope, holding
    // that project's locks while its first journal records exist.
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            B2_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(B2_CHILD_ROOT_ENV, projects[0].root())
        .env(B2_CHILD_JOURNAL_ENV, journal.path())
        .env(naming_migration::pause::PAUSE_DIR_ENV, rendezvous.path())
        .env(naming_migration::pause::PAUSE_STAGE_ENV, "after_rename")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the migration child");
    wait_for_path(
        &rendezvous.path().join(naming_migration::pause::READY_FILE),
        Duration::from_secs(60),
        "migration child",
    );
    // This process migrates the second project in full meanwhile.
    ensure_seeded_for_project_in(projects[1].root(), None, None, Some(journal.path()));
    std::fs::write(
        rendezvous
            .path()
            .join(naming_migration::pause::RELEASE_FILE),
        b"go",
    )
    .unwrap();
    let output = child.wait_with_output().expect("reap the child");
    assert!(
        output.status.success(),
        "child: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("AC_2715_CATALOG_CHILD_DONE"));

    let journal_now = naming_migration::read_journal(Some(journal.path()))
        .unwrap()
        .unwrap();
    for (fx, base) in projects.iter().zip(&bytes) {
        let key = fx.project_key();
        let record = journal_now
            .scope(&key)
            .cloned()
            .expect("both scopes recorded");
        assert_eq!(
            record.status,
            naming_migration::ScopeStatus::Complete,
            "{key}"
        );
        for rename in &CATALOG_FAMILY_RENAMES {
            assert!(
                record.steps.iter().any(|step| step.from == rename.from
                    && step.state == naming_migration::StepState::Renamed),
                "{key}: {} step: {:?}",
                rename.from,
                record.steps
            );
        }
        assert_eq!(
            std::fs::read(fx.project_catalog().join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            *base
        );
    }
}

#[test]
fn the_instance_directory_has_one_scope_key() {
    let config = tempfile::tempdir().unwrap();
    let catalog = config.path().join(CATALOG_DIR_NAME);
    std::fs::create_dir_all(&catalog).unwrap();
    std::fs::write(catalog.join(OLD_BASE), legacy_catalog_json()).unwrap();
    let mut spellings = vec![
        catalog.clone(),
        config.path().join(".").join(CATALOG_DIR_NAME),
    ];
    if cfg!(windows) {
        spellings.push(PathBuf::from(catalog.to_string_lossy().to_uppercase()));
    }
    for legacy in &spellings {
        let project = tempfile::tempdir().unwrap();
        ensure_seeded_for_project_in(project.path(), None, Some(legacy), Some(config.path()));
    }
    ensure_seeded_instance_in(config.path(), Some(config.path()));
    let instance_key = catalog_scope_key(&ensure_catalog_dir(config.path()).unwrap());
    let keys = scope_keys(config.path());
    let for_instance: Vec<_> = keys
        .iter()
        .filter(|key| key.to_lowercase() == instance_key.to_lowercase())
        .collect();
    assert_eq!(for_instance, [&instance_key], "{keys:?}");
}

#[test]
fn a_complete_scope_whose_old_name_is_back_is_re_run() {
    for both_present in [false, true] {
        let fx = B2Fixture::new();
        let catalog = std::fs::canonicalize(fx.project_catalog()).unwrap();
        let key = fx.project_key();
        naming_migration::update_journal(Some(fx.journal_dir()), |journal| {
            for rename in &CATALOG_FAMILY_RENAMES {
                journal.record_step(
                    &key,
                    naming_migration::StepRecord::new(
                        &catalog,
                        rename.from,
                        rename.to,
                        naming_migration::StepState::Renamed,
                    ),
                );
            }
            journal.set_status(&key, naming_migration::ScopeStatus::Complete);
        })
        .unwrap();
        let old = preserved_base_bytes("back");
        std::fs::write(catalog.join(OLD_BASE), &old).unwrap();
        let winner = preserved_base_bytes("winner");
        if both_present {
            std::fs::write(catalog.join(CATALOG_MANIFEST_FILENAME), &winner).unwrap();
        }

        fx.seed();

        assert!(!catalog.join(OLD_BASE).exists(), "both={both_present}");
        let expected = if both_present { &winner } else { &old };
        assert_eq!(
            &std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
            expected,
            "both={both_present}: no fresh base"
        );
        if both_present {
            assert_eq!(
                std::fs::read(catalog.join("agents.json.deprecated-1.no-git")).unwrap(),
                old
            );
        } else {
            assert!(deprecated_entries(&catalog).is_empty());
        }
        assert_eq!(
            fx.record(&key).status,
            naming_migration::ScopeStatus::Complete
        );
    }
}

#[test]
fn no_config_dir_still_migrates_and_never_seeds_a_second_base() {
    let fx = B2Fixture::new();
    let catalog = fx.project_catalog();
    let base = preserved_base_bytes("mine");
    let local = local_bytes("none");
    std::fs::write(catalog.join(OLD_BASE), &base).unwrap();
    std::fs::write(catalog.join(OLD_LOCAL), &local).unwrap();

    ensure_seeded_for_project_in(fx.root(), None, None, None);

    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert_eq!(
        std::fs::read(catalog.join(LOCAL_CATALOG_FILENAME)).unwrap(),
        local
    );
    assert!(
        !catalog.join(OLD_BASE).exists(),
        "no base seeded beside agents.json"
    );
    let journal_names = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.contains("naming-migration"))
            .collect()
    };
    assert!(journal_names(&catalog).is_empty());
    assert!(journal_names(&fx.ac_dir()).is_empty());
    assert!(journal_names(fx.journal_dir()).is_empty());

    // A second pass with a real journal finds nothing to do.
    fx.seed();
    assert_eq!(
        std::fs::read(catalog.join(CATALOG_MANIFEST_FILENAME)).unwrap(),
        base
    );
    assert!(deprecated_entries(&catalog).is_empty(), "nothing demoted");
    assert_eq!(
        fx.record(&fx.project_key()).status,
        naming_migration::ScopeStatus::Complete
    );
}

// ---- #2021: no-project instance catalog seeding ------------------------

/// The no-project report as the settings wrapper builds it for `config_dir`.
fn instance_report(config_dir: &Path) -> CatalogReport {
    load_catalog_report_for_settings_with_config_dir(
        &AppSettings::default(),
        Some(config_dir.to_path_buf()),
    )
}

/// Catalog-directory entries minus the lock file that every initialization
/// creates.
fn non_lock_entries(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = dir_entries(&catalog_dir(root))
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    names.retain(|name| name != CATALOG_LOCK_FILENAME);
    names
}

/// AC-1: a fresh config dir with no project seeds the instance managed base
/// and the no-project report lists exactly the supported built-ins.
#[test]
fn ensure_seeded_instance_fresh_config_dir_seeds_the_supported_builtins() {
    let instance = seed_dir();
    let published = ensure_seeded_instance(instance.path());
    assert!(published.is_some(), "an absent instance base is created");

    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    let expected: Vec<String> = supported_shipped_definitions()
        .into_iter()
        .map(|definition| definition.key)
        .collect();
    let actual: Vec<String> = report
        .catalog
        .iter()
        .map(|definition| definition.key.clone())
        .collect();
    assert_eq!(
        actual, expected,
        "exactly the supported built-in keys in persisted order"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        LOCAL_STUB_BYTES,
        "the create-once local stub is part of a fresh instance seed"
    );
}

/// AC-2: a second call leaves the base bytes alone and publishes nothing.
#[test]
fn ensure_seeded_instance_is_idempotent() {
    let instance = seed_dir();
    assert!(ensure_seeded_instance(instance.path()).is_some());
    let base_before = std::fs::read(manifest_path(instance.path())).unwrap();
    let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        base_before
    );
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        local_before
    );
}

/// AC-3: a marker-less legacy instance file is never migrated, rewritten or
/// accompanied by sidecars.
#[test]
fn ensure_seeded_instance_leaves_a_legacy_instance_file_untouched() {
    let instance = seed_dir();
    let legacy = String::from_utf8(legacy_catalog_json()).expect("utf-8 legacy fixture");
    write_legacy_base(instance.path(), &legacy);
    let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        bytes_before
    );
    assert_eq!(
        non_lock_entries(instance.path()),
        vec![
            RETIRED_CATALOG_LOCK_FILENAME.to_string(),
            CATALOG_MANIFEST_FILENAME.to_string(),
        ],
        "a legacy instance file creates no local, backup or journal sidecar; the retired lock sidecar is the scope lock"
    );
    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none());
    assert_eq!(
        report
            .catalog
            .iter()
            .map(|definition| definition.key.as_str())
            .collect::<Vec<_>>(),
        ["mine"],
        "the legacy instance file stays readable as before"
    );
}

/// AC-4: corrupt instance bytes are preserved and the report keeps the same
/// unavailability code before and after the call.
#[test]
fn ensure_seeded_instance_leaves_a_corrupt_instance_file_untouched() {
    let instance = seed_dir();
    write_legacy_base(instance.path(), "{ not JSON");
    let bytes_before = std::fs::read(manifest_path(instance.path())).unwrap();
    let code_before = instance_report(instance.path())
        .unavailable
        .as_ref()
        .map(|diagnostic| diagnostic.code.clone());
    assert_eq!(code_before.as_deref(), Some("baseInvalid"));

    assert!(ensure_seeded_instance(instance.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(instance.path())).unwrap(),
        bytes_before
    );
    let code_after = instance_report(instance.path())
        .unavailable
        .as_ref()
        .map(|diagnostic| diagnostic.code.clone());
    assert_eq!(code_after, code_before);
}

/// AC-5: a MANAGED instance base must not be imported as legacy territory
/// by a later project; the project gets a fresh managed base instead of
/// MIGRATION_CONFLICT. This drives the same production initialization path
/// that `ensure_seeded_for_project_with_token` uses, with the instance
/// catalog dir injected explicitly because the process-global `config_dir()`
/// has no test seam.
#[test]
fn managed_instance_base_does_not_block_a_later_project_fresh_seed() {
    let instance = seed_dir();
    assert!(ensure_seeded_instance(instance.path()).is_some());
    assert_eq!(
        base_json(instance.path())["managed"]["owner"],
        "agentscommander"
    );

    let project = seed_dir();
    let ac_dir = ac_dir_for(project.path());
    let outcome = run_catalog_initialization(&ac_dir, Some(&catalog_dir(instance.path())), None);
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationConflict"),
        "a managed instance base must not be imported as legacy territory: {:?}",
        outcome.warnings
    );
    assert!(
        outcome.published_at.is_some(),
        "the project fresh-seeds its own managed base"
    );

    let base = base_json(&ac_dir);
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert_eq!(
        std::fs::read(local_catalog_path(&ac_dir)).unwrap(),
        LOCAL_STUB_BYTES,
        "the project's own local stub is published"
    );
    let entries = non_lock_entries(&ac_dir);
    assert!(
        !entries.contains(&MIGRATION_BACKUP_FILENAME.to_string())
            && !entries.contains(&MIGRATION_JOURNAL_FILENAME.to_string()),
        "no project backup or journal may be written: {entries:?}"
    );

    let report = load_catalog_report_for_settings_with_config_dir(
        &AppSettings {
            project_paths: vec![project.path().to_string_lossy().to_string()],
            ..AppSettings::default()
        },
        None,
    );
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == "migrationConflict"),
        "the managed instance base must not be imported: {:?}",
        report.warnings
    );
    assert_eq!(report.catalog.len(), 8);
}

/// AC-8: an existing instance local layer still wins over the seeded base
/// with no project registered.
#[test]
fn ensure_seeded_instance_local_overrides_win_with_no_project() {
    let instance = seed_dir();
    write_local(
        instance.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MY CLAUDE","updateCommands":["my-claude-update"]}]}"##,
    );
    let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_some());
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        local_before
    );

    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none());
    let claude = report
        .catalog
        .iter()
        .find(|definition| definition.key == "claude")
        .expect("claude present");
    assert_eq!(claude.label, "MY CLAUDE");
    assert_eq!(
        claude.update_commands,
        vec!["my-claude-update".to_string()],
        "the local override wins over the shipped command list"
    );
    let codex = report
        .catalog
        .iter()
        .find(|definition| definition.key == "codex")
        .expect("codex present");
    assert_eq!(codex.label, "Codex", "unpinned agents keep shipped values");
}

/// AC-9: a stale verified managed instance base is refreshed to the current
/// fresh-seed bytes; an edited one is never replaced.
#[test]
fn ensure_seeded_instance_refreshes_only_a_stale_verified_base() {
    let stale = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("OLD LABEL");
    write_managed_base(stale.path(), &[claude], "stale-revision", true);
    assert!(
        ensure_seeded_instance(stale.path()).is_some(),
        "a stale managed instance base refreshes"
    );
    let refreshed = std::fs::read(manifest_path(stale.path())).unwrap();
    assert_eq!(
        base_json(stale.path())["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    let fresh = seed_dir();
    assert!(ensure_seeded_instance(fresh.path()).is_some());
    assert_eq!(
        refreshed,
        std::fs::read(manifest_path(fresh.path())).unwrap(),
        "the refreshed bytes equal a fresh instance seed"
    );
    let report = instance_report(stale.path());
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == "refreshFailed"),
        "a successful refresh leaves no stale-revision warning: {:?}",
        report.warnings
    );
    assert_eq!(report.catalog.len(), 8);

    // Edited managed base: readable but never replaced.
    let edited = seed_dir();
    write_managed_base(
        edited.path(),
        &shipped_def_json(&["claude"]),
        "stale-revision",
        false,
    );
    let edited_before = std::fs::read(manifest_path(edited.path())).unwrap();
    assert!(ensure_seeded_instance(edited.path()).is_none());
    assert_eq!(
        std::fs::read(manifest_path(edited.path())).unwrap(),
        edited_before
    );
}

/// AC-10: an existing local layer with an absent base is kept verbatim and
/// its overrides show in the report.
#[test]
fn ensure_seeded_instance_keeps_an_existing_local_with_an_absent_base() {
    let instance = seed_dir();
    write_local(
        instance.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"pi","label":"PINNED PI"}]}"##,
    );
    let local_before = std::fs::read(local_catalog_path(instance.path())).unwrap();

    assert!(ensure_seeded_instance(instance.path()).is_some());
    assert_eq!(
        std::fs::read(local_catalog_path(instance.path())).unwrap(),
        local_before,
        "the create-once stub never overwrites an existing local layer"
    );
    let report = instance_report(instance.path());
    assert!(report.unavailable.is_none());
    let pi = report
        .catalog
        .iter()
        .find(|definition| definition.key == "pi")
        .expect("pi present");
    assert_eq!(pi.label, "PINNED PI");
}

/// 2.2 instance policy: a foreign journal beside the instance catalog is
/// never resumed, rewritten or removed; the base still fresh-seeds.
#[test]
fn ensure_seeded_instance_never_resumes_a_foreign_sidecar() {
    let instance = seed_dir();
    let journal_path = catalog_dir(instance.path()).join(MIGRATION_JOURNAL_FILENAME);
    std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    std::fs::write(&journal_path, b"{ foreign journal }").unwrap();
    let journal_before = std::fs::read(&journal_path).unwrap();

    assert!(
        ensure_seeded_instance(instance.path()).is_some(),
        "the instance base still fresh-seeds"
    );
    assert_eq!(
        std::fs::read(&journal_path).unwrap(),
        journal_before,
        "the instance never resumes or rewrites a foreign journal"
    );
}

#[test]
fn command_account_key_drops_arguments_and_keeps_the_program_token() {
    assert_eq!(command_account_key("claude"), Some("claude".to_string()));
    assert_eq!(
        command_account_key("claude --dangerously-skip-permissions"),
        Some("claude".to_string())
    );
}

#[test]
fn command_account_key_separates_two_paths_to_the_same_name() {
    assert_eq!(
        command_account_key("/a/claude"),
        Some("/a/claude".to_string())
    );
    assert_ne!(
        command_account_key("/a/claude"),
        command_account_key("claude")
    );
}

#[test]
fn command_account_key_is_none_for_an_empty_command() {
    assert_eq!(command_account_key(""), None);
    assert_eq!(command_account_key("   "), None);
}

#[test]
fn command_account_key_honours_quoting() {
    assert_eq!(
        command_account_key("\"C:/Program Files/a/claude.exe\" --x"),
        Some("C:/Program Files/a/claude.exe".to_string())
    );
}

fn welcome_def_2736(
    key: &str,
    command: &str,
    install: Option<serde_json::Value>,
) -> CodingAgentDefinition {
    let mut raw = serde_json::json!({
        "key": key,
        "label": key,
        "description": "d",
        "color": "#000000",
        "command": command,
        "envs": [],
        "isolatedHome": false,
        "removable": true
    });
    if let Some(install) = install {
        raw["installCommands"] = install;
    }
    serde_json::from_value(raw).expect("fixture definition")
}

#[test]
fn tested_level_2736_table_keys_equal_the_enabled_support_rows_in_order() {
    let tested: Vec<&str> = BUILTIN_TESTED_LEVEL.iter().map(|(key, _)| *key).collect();
    let enabled: Vec<&str> = BUILTIN_AGENT_SUPPORT
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(key, _)| *key)
        .collect();
    assert_eq!(tested, enabled);
}

#[test]
fn tested_level_2736_assigns_high_medium_low_exactly_as_specified() {
    for key in ["claude", "codex", "pi"] {
        assert_eq!(tested_level_for(key), Some(TestedLevel::High), "{key}");
    }
    assert_eq!(tested_level_for("antigravity"), Some(TestedLevel::Medium));
    for key in ["hermes", "cursor", "opencode", "grok"] {
        assert_eq!(tested_level_for(key), Some(TestedLevel::Low), "{key}");
    }
    assert_eq!(tested_level_for("muse"), None);
    assert_eq!(tested_level_for("my-agent"), None);
}

#[test]
fn tested_level_2736_serializes_to_lowercase_wire_strings() {
    assert_eq!(
        serde_json::to_value(TestedLevel::High).unwrap(),
        serde_json::json!("high")
    );
    assert_eq!(
        serde_json::to_value(TestedLevel::Medium).unwrap(),
        serde_json::json!("medium")
    );
    assert_eq!(
        serde_json::to_value(TestedLevel::Low).unwrap(),
        serde_json::json!("low")
    );
}

#[test]
fn install_command_2736_prefers_the_platform_key_over_default() {
    let commands = InstallCommands {
        default: "npm i -g x".to_string(),
        windows: Some("winget install x".to_string()),
        macos: None,
        linux: None,
    };
    assert_eq!(
        install_command_for_os(&commands, "windows"),
        "winget install x"
    );
    assert_eq!(install_command_for_os(&commands, "macos"), "npm i -g x");
    assert_eq!(install_command_for_os(&commands, "linux"), "npm i -g x");
    let all = InstallCommands {
        default: "d".to_string(),
        windows: Some("w".to_string()),
        macos: Some("m".to_string()),
        linux: Some("l".to_string()),
    };
    assert_eq!(install_command_for_os(&all, "windows"), "w");
    assert_eq!(install_command_for_os(&all, "macos"), "m");
    assert_eq!(install_command_for_os(&all, "linux"), "l");
}

#[test]
fn install_command_2736_unknown_os_name_falls_back_to_default() {
    let all = InstallCommands {
        default: "d".to_string(),
        windows: Some("w".to_string()),
        macos: Some("m".to_string()),
        linux: Some("l".to_string()),
    };
    assert_eq!(install_command_for_os(&all, "freebsd"), "d");
    assert_eq!(install_command_for_os(&all, ""), "d");
}

#[test]
fn command_is_present_2736_true_for_a_resolvable_token_false_otherwise() {
    let stub = |token: &str| (token == "claude").then(|| PathBuf::from("/bin/claude"));
    assert!(command_is_present_with("claude", stub));
    assert!(command_is_present_with("claude --flag", stub));
    assert!(!command_is_present_with("codex", stub));
    assert!(!command_is_present_with("", stub));
    assert!(!command_is_present_with("   ", stub));
}

#[test]
fn command_is_present_2736_executes_no_process() {
    // The injected closure is the ONLY resolution step: no version probe
    // (which would spawn a process) sits on this path.
    let mut calls = Vec::new();
    let present = command_is_present_with("\"my tool\" --x", |token| {
        calls.push(token.to_string());
        None
    });
    assert!(!present);
    assert_eq!(calls, vec!["my tool".to_string()]);
    let mut empty_calls = 0;
    assert!(!command_is_present_with("", |_| {
        empty_calls += 1;
        Some(PathBuf::from("x"))
    }));
    assert_eq!(empty_calls, 0);
}

#[test]
fn welcome_status_2736_row_per_entry_in_catalog_order_with_computed_fields() {
    let catalog = vec![
        welcome_def_2736(
            "codex",
            "codex",
            Some(serde_json::json!({ "default": "npm i -g codex" })),
        ),
        welcome_def_2736("claude", "claude --x", None),
        welcome_def_2736(
            "my-agent",
            "my-agent",
            Some(serde_json::json!({ "default": "d", "windows": "d", "macos": "d", "linux": "d" })),
        ),
    ];
    let rows = welcome_status_for_with(&catalog, |command| command == "claude --x");
    assert_eq!(
        rows,
        vec![
            CodingAgentWelcomeStatus {
                key: "codex".to_string(),
                installed: false,
                tested_level: Some(TestedLevel::High),
                install_command: Some("npm i -g codex".to_string()),
            },
            CodingAgentWelcomeStatus {
                key: "claude".to_string(),
                installed: true,
                tested_level: Some(TestedLevel::High),
                install_command: None,
            },
            CodingAgentWelcomeStatus {
                key: "my-agent".to_string(),
                installed: false,
                tested_level: None,
                install_command: Some("d".to_string()),
            },
        ]
    );
    let wire = serde_json::to_value(&rows).unwrap();
    assert_eq!(wire[0]["testedLevel"], serde_json::json!("high"));
    assert_eq!(wire[1]["installCommand"], serde_json::Value::Null);
    assert_eq!(wire[2]["testedLevel"], serde_json::Value::Null);
}

#[test]
fn welcome_status_2736_never_adds_a_field_to_the_persisted_definition() {
    let def = welcome_def_2736(
        "claude",
        "claude",
        Some(serde_json::json!({ "default": "d" })),
    );
    let value = serde_json::to_value(&def).unwrap();
    let object = value.as_object().expect("definition object");
    assert!(!object.contains_key("testedLevel"));
    assert!(!object.contains_key("installed"));
}

const SCOPE_B_EXPECTED_2800: &str = r#####"{"claude":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwBsAGEAdQBkAGUALgBhAGkALwBpAG4AcwB0AGEAbABsAC4AcABzADEAJwAgAC0AVABpAG0AZQBvAHUAdABTAGUAYwAgADEAMgAwACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwADsAIABpAGYAIAAoAFsAcwB0AHIAaQBuAGcAXQA6ADoASQBzAE4AdQBsAGwATwByAFcAaABpAHQAZQBTAHAAYQBjAGUAKAAkAGEAYwBfAGkAbgBzAHQAYQBsAGwAXwBzAGMAcgBpAHAAdAApACkAIAB7ACAAdABoAHIAbwB3ACAAJwBFAG0AcAB0AHkAIABpAG4AcwB0AGEAbABsAGUAcgAgAHIAZQBzAHAAbwBuAHMAZQAnACAAfQA7ACAAJABhAGMAXwBwAGEAdABoAD0ASgBvAGkAbgAtAFAAYQB0AGgAIAAoAFsASQBPAC4AUABhAHQAaABdADoAOgBHAGUAdABUAGUAbQBwAFAAYQB0AGgAKAApACkAIAAoACcAYQBjAC0AaQBuAHMAdABhAGwAbAAtADIANwA4ADcALQAnACsAWwBHAHUAaQBkAF0AOgA6AE4AZQB3AEcAdQBpAGQAKAApAC4AVABvAFMAdAByAGkAbgBnACgAJwBOACcAKQArACcALgBwAHMAMQAnACkAOwAgACQAYQBjAF8AZgBpAGwAZQA9AFsASQBPAC4ARgBpAGwAZQBdADoAOgBPAHAAZQBuACgAJABhAGMAXwBwAGEAdABoACwAWwBJAE8ALgBGAGkAbABlAE0AbwBkAGUAXQA6ADoAQwByAGUAYQB0AGUATgBlAHcALABbAEkATwAuAEYAaQBsAGUAQQBjAGMAZQBzAHMAXQA6ADoAVwByAGkAdABlACwAWwBJAE8ALgBGAGkAbABlAFMAaABhAHIAZQBdADoAOgBOAG8AbgBlACkAOwAgACQAYQBjAF8AYwByAGUAYQB0AGUAZAA9ACQAdAByAHUAZQA7ACAAdAByAHkAIAB7ACAAWwBiAHkAdABlAFsAXQBdACQAYQBjAF8AYgB5AHQAZQBzAD0AWwBUAGUAeAB0AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAVABGADgALgBHAGUAdABQAHIAZQBhAG0AYgBsAGUAKAApACsAWwBUAGUAeAB0AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAVABGADgALgBHAGUAdABCAHkAdABlAHMAKAAkAGEAYwBfAGkAbgBzAHQAYQBsAGwAXwBzAGMAcgBpAHAAdAApADsAIAAkAGEAYwBfAGYAaQBsAGUALgBXAHIAaQB0AGUAKAAkAGEAYwBfAGIAeQB0AGUAcwAsADAALAAkAGEAYwBfAGIAeQB0AGUAcwAuAEwAZQBuAGcAdABoACkAIAB9ACAAZgBpAG4AYQBsAGwAeQAgAHsAIAAkAGEAYwBfAGYAaQBsAGUALgBEAGkAcwBwAG8AcwBlACgAKQAgAH0AOwAgACQAZwBsAG8AYgBhAGwAOgBMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA9ACQAbgB1AGwAbAA7ACAAJgAgACgASgBvAGkAbgAtAFAAYQB0AGgAIAAkAFAAUwBIAE8ATQBFACAAJwBwAG8AdwBlAHIAcwBoAGUAbABsAC4AZQB4AGUAJwApACAALQBOAG8AUAByAG8AZgBpAGwAZQAgAC0ATgBvAG4ASQBuAHQAZQByAGEAYwB0AGkAdgBlACAALQBFAHgAZQBjAHUAdABpAG8AbgBQAG8AbABpAGMAeQAgAEIAeQBwAGEAcwBzACAALQBGAGkAbABlACAAJABhAGMAXwBwAGEAdABoADsAIAAkAGEAYwBfAGUAeABpAHQAPQAkAEwAQQBTAFQARQBYAEkAVABDAE8ARABFADsAIABpAGYAIAAoACQAbgB1AGwAbAAgAC0AZQBxACAAJABhAGMAXwBlAHgAaQB0ACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApADsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAaQBmACAAKAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAKQAgAHsAIAB0AHIAeQAgAHsAIABSAGUAbQBvAHYAZQAtAEkAdABlAG0AIAAtAEwAaQB0AGUAcgBhAGwAUABhAHQAaAAgACQAYQBjAF8AcABhAHQAaAAgAC0ARgBvAHIAYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAaQBmACAAKAAkAGEAYwBfAGUAeABpAHQAIAAtAGUAcQAgADAAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAH0AIAB9ACAAfQA7ACAAZQB4AGkAdAAgACQAYQBjAF8AZQB4AGkAdAA=","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://claude.ai/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://claude.ai/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"codex":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwBoAGEAdABnAHAAdAAuAGMAbwBtAC8AYwBvAGQAZQB4AC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://chatgpt.com/codex/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | sh","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://chatgpt.com/codex/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | sh","default":"echo No verified installer for this platform 1>&2 && exit 1"},"hermes":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AaABlAHIAbQBlAHMALQBhAGcAZQBuAHQALgBuAG8AdQBzAHIAZQBzAGUAYQByAGMAaAAuAGMAbwBtAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAAgAC0ATgBvAG4ASQBuAHQAZQByAGEAYwB0AGkAdgBlADsAIAAkAGEAYwBfAGUAeABpAHQAPQAkAEwAQQBTAFQARQBYAEkAVABDAE8ARABFADsAIABpAGYAIAAoACQAbgB1AGwAbAAgAC0AZQBxACAAJABhAGMAXwBlAHgAaQB0ACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApADsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAaQBmACAAKAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAKQAgAHsAIAB0AHIAeQAgAHsAIABSAGUAbQBvAHYAZQAtAEkAdABlAG0AIAAtAEwAaQB0AGUAcgBhAGwAUABhAHQAaAAgACQAYQBjAF8AcABhAHQAaAAgAC0ARgBvAHIAYwBlACAALQBFAHIAcgBvAHIAQQBjAHQAaQBvAG4AIABTAHQAbwBwACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAaQBmACAAKAAkAGEAYwBfAGUAeABpAHQAIAAtAGUAcQAgADAAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAH0AIAB9ACAAfQA7ACAAZQB4AGkAdAAgACQAYQBjAF8AZQB4AGkAdAA=","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://hermes-agent.nousresearch.com/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash -s -- --non-interactive","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://hermes-agent.nousresearch.com/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash -s -- --non-interactive","default":"echo No verified installer for this platform 1>&2 && exit 1"},"cursor":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYwB1AHIAcwBvAHIALgBjAG8AbQAvAGkAbgBzAHQAYQBsAGwAPwB3AGkAbgAzADIAPQB0AHIAdQBlACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://cursor.com/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://cursor.com/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"pi":{"windows":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-coding-agent","macos":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-coding-agent","linux":"npm install -g --ignore-scripts --include=optional @earendil-works/pi-coding-agent","default":"echo No verified installer for this platform 1>&2 && exit 1"},"opencode":{"windows":"npm install -g --ignore-scripts=false --include=optional --allow-scripts=opencode-ai opencode-ai","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://opencode.ai/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://opencode.ai/install') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"antigravity":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AYQBuAHQAaQBnAHIAYQB2AGkAdAB5AC4AZwBvAG8AZwBsAGUALwBjAGwAaQAvAGkAbgBzAHQAYQBsAGwALgBwAHMAMQAnACAALQBUAGkAbQBlAG8AdQB0AFMAZQBjACAAMQAyADAAIAAtAEUAcgByAG8AcgBBAGMAdABpAG8AbgAgAFMAdABvAHAAOwAgAGkAZgAgACgAWwBzAHQAcgBpAG4AZwBdADoAOgBJAHMATgB1AGwAbABPAHIAVwBoAGkAdABlAFMAcABhAGMAZQAoACQAYQBjAF8AaQBuAHMAdABhAGwAbABfAHMAYwByAGkAcAB0ACkAKQAgAHsAIAB0AGgAcgBvAHcAIAAnAEUAbQBwAHQAeQAgAGkAbgBzAHQAYQBsAGwAZQByACAAcgBlAHMAcABvAG4AcwBlACcAIAB9ADsAIAAkAGEAYwBfAHAAYQB0AGgAPQBKAG8AaQBuAC0AUABhAHQAaAAgACgAWwBJAE8ALgBQAGEAdABoAF0AOgA6AEcAZQB0AFQAZQBtAHAAUABhAHQAaAAoACkAKQAgACgAJwBhAGMALQBpAG4AcwB0AGEAbABsAC0AMgA3ADgANwAtACcAKwBbAEcAdQBpAGQAXQA6ADoATgBlAHcARwB1AGkAZAAoACkALgBUAG8AUwB0AHIAaQBuAGcAKAAnAE4AJwApACsAJwAuAHAAcwAxACcAKQA7ACAAJABhAGMAXwBmAGkAbABlAD0AWwBJAE8ALgBGAGkAbABlAF0AOgA6AE8AcABlAG4AKAAkAGEAYwBfAHAAYQB0AGgALABbAEkATwAuAEYAaQBsAGUATQBvAGQAZQBdADoAOgBDAHIAZQBhAHQAZQBOAGUAdwAsAFsASQBPAC4ARgBpAGwAZQBBAGMAYwBlAHMAcwBdADoAOgBXAHIAaQB0AGUALABbAEkATwAuAEYAaQBsAGUAUwBoAGEAcgBlAF0AOgA6AE4AbwBuAGUAKQA7ACAAJABhAGMAXwBjAHIAZQBhAHQAZQBkAD0AJAB0AHIAdQBlADsAIAB0AHIAeQAgAHsAIABbAGIAeQB0AGUAWwBdAF0AJABhAGMAXwBiAHkAdABlAHMAPQBbAFQAZQB4AHQALgBFAG4AYwBvAGQAaQBuAGcAXQA6ADoAVQBUAEYAOAAuAEcAZQB0AFAAcgBlAGEAbQBiAGwAZQAoACkAKwBbAFQAZQB4AHQALgBFAG4AYwBvAGQAaQBuAGcAXQA6ADoAVQBUAEYAOAAuAEcAZQB0AEIAeQB0AGUAcwAoACQAYQBjAF8AaQBuAHMAdABhAGwAbABfAHMAYwByAGkAcAB0ACkAOwAgACQAYQBjAF8AZgBpAGwAZQAuAFcAcgBpAHQAZQAoACQAYQBjAF8AYgB5AHQAZQBzACwAMAAsACQAYQBjAF8AYgB5AHQAZQBzAC4ATABlAG4AZwB0AGgAKQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgACQAYQBjAF8AZgBpAGwAZQAuAEQAaQBzAHAAbwBzAGUAKAApACAAfQA7ACAAJABnAGwAbwBiAGEAbAA6AEwAQQBTAFQARQBYAEkAVABDAE8ARABFAD0AJABuAHUAbABsADsAIAAmACAAKABKAG8AaQBuAC0AUABhAHQAaAAgACQAUABTAEgATwBNAEUAIAAnAHAAbwB3AGUAcgBzAGgAZQBsAGwALgBlAHgAZQAnACkAIAAtAE4AbwBQAHIAbwBmAGkAbABlACAALQBOAG8AbgBJAG4AdABlAHIAYQBjAHQAaQB2AGUAIAAtAEUAeABlAGMAdQB0AGkAbwBuAFAAbwBsAGkAYwB5ACAAQgB5AHAAYQBzAHMAIAAtAEYAaQBsAGUAIAAkAGEAYwBfAHAAYQB0AGgAOwAgACQAYQBjAF8AZQB4AGkAdAA9ACQATABBAFMAVABFAFgASQBUAEMATwBEAEUAOwAgAGkAZgAgACgAJABuAHUAbABsACAALQBlAHEAIAAkAGEAYwBfAGUAeABpAHQAKQAgAHsAIAAkAGEAYwBfAGUAeABpAHQAPQAxACAAfQAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAZgBpAG4AYQBsAGwAeQAgAHsAIABpAGYAIAAoACQAYQBjAF8AYwByAGUAYQB0AGUAZAApACAAewAgAHQAcgB5ACAAewAgAFIAZQBtAG8AdgBlAC0ASQB0AGUAbQAgAC0ATABpAHQAZQByAGEAbABQAGEAdABoACAAJABhAGMAXwBwAGEAdABoACAALQBGAG8AcgBjAGUAIAAtAEUAcgByAG8AcgBBAGMAdABpAG8AbgAgAFMAdABvAHAAIAB9ACAAYwBhAHQAYwBoACAAewAgAFsAQwBvAG4AcwBvAGwAZQBdADoAOgBFAHIAcgBvAHIALgBXAHIAaQB0AGUATABpAG4AZQAoACQAXwApADsAIABpAGYAIAAoACQAYQBjAF8AZQB4AGkAdAAgAC0AZQBxACAAMAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAH0AIAB9ADsAIABlAHgAaQB0ACAAJABhAGMAXwBlAHgAaQB0AA==","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://antigravity.google/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://antigravity.google/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"},"grok":{"windows":"powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAFIAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AeAAuAGEAaQAvAGMAbABpAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUwBlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBmACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAGEAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQAaAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8AbgBzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABbAEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAGEAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUAdwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJwApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAoACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAGEAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQAZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYwBfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBbAF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwByAGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAGUAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4ARABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQBUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQAFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAFAAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUAYwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQBjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBPAEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApACAAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMAbwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAgACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AGUALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgAIAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIABjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBpAHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAHEAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUAeABpAHQAIAAkAGEAYwBfAGUAeABpAHQA","macos":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","linux":"ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https://x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n' \"$ac_install_script\" | bash","default":"echo No verified installer for this platform 1>&2 && exit 1"}}"#####;
const SCOPE_B_NON_INSTALL_MAIN_2800: &str = r#####"{"schemaVersion":1,"agents":[{"key":"claude","label":"Claude Code","description":"Coding Agent by Anthropic","color":"#d97706","baseCommand":"claude","instructionsFilename":"CLAUDE.md","envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".claude"},"removable":true,"updateCommands":["claude --update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"codex","label":"Codex","description":"Coding Agent by OpenAI","color":"#10b981","baseCommand":"codex","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".codex"},"removable":true,"updateCommands":["codex update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"hermes","label":"Hermes","description":"Coding Agent by Nous Research","color":"#8b5cf6","baseCommand":"hermes","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["hermes update --yes"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"cursor","label":"Cursor CLI","description":"Coding Agent by Cursor","color":"#22d3ee","baseCommand":"agent","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"pi","label":"Pi","description":"Coding Agent by Earendil Inc","color":"#ec4899","baseCommand":"pi","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["pi update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"opencode","label":"OpenCode","description":"Open-source terminal coding agent by Anomaly","color":"#64748b","baseCommand":"opencode","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"configSeed":{"enabled":true,"dest":".opencode"},"removable":true,"updateCommands":["opencode upgrade"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"antigravity","label":"Antigravity","description":"Coding Agent by Google","color":"#4285F4","baseCommand":"agy","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"updateCommands":["agy update"],"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"grok","label":"Grok Build","description":"Coding Agent by SpaceXAI","color":"#64748b","baseCommand":"grok","instructionsFilename":"AGENTS.md","envs":[],"isolatedHome":false,"removable":true,"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}},{"key":"muse","label":"Muse Code","description":"Meta terminal coding agent (beta; macOS/Linux host only)","color":"#0668E1","baseCommand":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false,"idleBurst":{"maxBytes":1024,"maxSecs":3.0,"priorSilenceSecs":60.0}}]}"#####;

fn scope_b_expected_commands_2800(key: &str) -> Option<serde_json::Value> {
    let expected: serde_json::Value = serde_json::from_str(SCOPE_B_EXPECTED_2800).unwrap();
    expected.get(key).cloned()
}

#[test]
fn scope_b_catalog_2800_exact_32_cells() {
    let expected: serde_json::Value = serde_json::from_str(SCOPE_B_EXPECTED_2800).unwrap();
    let actual: serde_json::Value = serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
    assert_eq!(expected.as_object().unwrap().len(), 8);
    let mut cells = 0;
    for (key, commands) in expected.as_object().unwrap() {
        let row = actual["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key.as_str())
            .unwrap();
        assert_eq!(row["installCommands"], *commands, "{key}");
        assert_eq!(commands.as_object().unwrap().len(), 4);
        for os in ["windows", "macos", "linux", "default"] {
            assert!(!commands[os].as_str().unwrap().is_empty());
            cells += 1;
        }
        let definition = embedded_default_catalog()
            .agents
            .into_iter()
            .find(|row| row.key == *key)
            .unwrap();
        let typed = definition.install_commands.unwrap();
        for os in ["windows", "macos", "linux", "default"] {
            assert_eq!(
                install_command_for_os(&typed, os),
                commands[os].as_str().unwrap()
            );
        }
    }
    assert_eq!(cells, 32);
    embedded_default_matches_current_presets_exactly();
    install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse();
}

#[test]
fn scope_b_catalog_2800_non_install_fields_unchanged() {
    let expected: serde_json::Value = serde_json::from_str(SCOPE_B_NON_INSTALL_MAIN_2800).unwrap();
    let mut actual: serde_json::Value =
        serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
    for row in actual["agents"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("installCommands");
    }
    assert_eq!(
        actual, expected,
        "pinned main fields/schema/order/ninth row"
    );
}

#[test]
fn scope_b_catalog_2800_unknown_os_and_missing_override_use_default() {
    let mut commands = InstallCommands {
        default: "default sentinel".to_string(),
        windows: Some("windows sentinel".to_string()),
        macos: Some("macos sentinel".to_string()),
        linux: Some("linux sentinel".to_string()),
    };
    for unknown in ["unknown", "freebsd", "android", ""] {
        assert_eq!(
            install_command_for_os(&commands, unknown),
            "default sentinel"
        );
    }
    commands.windows = None;
    commands.macos = None;
    commands.linux = None;
    for os in ["windows", "macos", "linux"] {
        assert_eq!(install_command_for_os(&commands, os), "default sentinel");
    }
}

#[test]
fn scope_b_catalog_2800_host_selector_matches_supported_cfg() {
    let commands = InstallCommands {
        default: "default sentinel".to_string(),
        windows: Some("windows sentinel".to_string()),
        macos: Some("macos sentinel".to_string()),
        linux: Some("linux sentinel".to_string()),
    };
    let expected = if cfg!(target_os = "windows") {
        "windows sentinel"
    } else if cfg!(target_os = "macos") {
        "macos sentinel"
    } else if cfg!(target_os = "linux") {
        "linux sentinel"
    } else {
        "default sentinel"
    };
    assert_eq!(resolve_install_command(&commands), expected);
}

#[test]
fn scope_b_catalog_2800_project_and_personal_composition_preserved() {
    // Reuse the exact existing main regressions; no copied implementation/harness.
    project_absence_preserves_direct_report_and_catalog();
    project_then_personal_merges_fields_nulls_and_exact_order();
    install_commands_2736_local_layer_patches_windows_and_inherits_default();
    install_commands_2736_one_bad_local_value_discards_the_entire_local_layer();
    install_commands_2736_managed_base_refreshes_from_the_previous_revision();
    install_commands_2736_pin_wire_keeps_presence_semantics();
}

#[test]
fn scope_b_catalog_2800_malformed_project_and_muse_policy_preserved() {
    project_invalid_schema_preserves_base_and_independent_personal();
    project_rejection_has_no_donor_and_personal_rejection_keeps_project();
    project_ownership_support_and_unavailable_base_contract();
    builtin_agent_support_ships_only_muse_disabled();
}
