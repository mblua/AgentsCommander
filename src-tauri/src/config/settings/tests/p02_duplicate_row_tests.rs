use super::*;
use serde_json::json;

fn row(id: &str, command: &str, order: Value, marker: &str) -> Value {
    json!({"id":id,"label":marker,"color":"#123456","command":command,
            "order":order,"privatePayload":{"marker":marker}})
}

fn typed_candidate(raw: &Value) -> Value {
    let mut agents: Vec<AgentConfig> = serde_json::from_value(raw.clone()).unwrap();
    finalize_agent_order(&mut agents);
    json!({"agents":agents})
}

fn retain(after: &mut Value, raw: &Value) -> Result<(), String> {
    retain_configuration_commands(after, &json!({"agents":raw}), "id", false, true, false)
}

fn seed(raw: &Value, sidecar: bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETTINGS_FILE_NAME);
    let mut main = json!({"defaultShell":"test-shell","defaultShellArgs":[],"rootToken":"token"});
    if sidecar {
        std::fs::write(
            agents_instance_path(&path),
            serde_json::to_vec(&json!({"agents":raw})).unwrap(),
        )
        .unwrap();
    } else {
        main["agents"] = raw.clone();
    }
    std::fs::write(&path, serde_json::to_vec(&main).unwrap()).unwrap();
    (dir, path)
}

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.file_name().unwrap() != SETTINGS_LOCK_FILE_NAME)
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn p02_duplicate_full_row_permutation_retains_distinct_values_and_absence() {
    for orders in [
        (json!(1), json!(0)),
        (Value::Null, Value::Null),
        (json!(0), json!(0)),
    ] {
        let mut raw = json!([
            row("dup", "claude", orders.0, "claude-row"),
            row("dup", "codex", orders.1, "codex-row"),
            row("dup", "third", json!(2), "absent-old")
        ]);
        raw[2]["baseCommand"] = json!("third");
        raw[2].as_object_mut().unwrap().remove("command");
        let mut after = typed_candidate(&raw);
        retain(&mut after, &raw).unwrap();
        for candidate in after["agents"].as_array().unwrap() {
            let original = raw
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["label"] == candidate["label"])
                .unwrap();
            assert_eq!(candidate.get("command"), original.get("command"));
            assert_eq!(candidate["privatePayload"], original["privatePayload"]);
        }
        let saved = after.clone();
        retain_configuration_commands(&mut after, &saved, "id", false, true, false).unwrap();
        assert_eq!(after, saved);
    }
    let raw = json!([
        row("dup", "claude", json!(1), "claude"),
        row("dup", "codex", json!(0), "codex")
    ]);
    let mut after = typed_candidate(&raw);
    retain(&mut after, &raw).unwrap();
    assert_eq!(after["agents"][0]["command"], "codex");
    assert_eq!(after["agents"][0]["privatePayload"]["marker"], "codex");
    assert_eq!(after["agents"][1]["command"], "claude");
}

#[test]
fn p02_identical_effective_duplicates_keep_authoritative_unknown_order() {
    let mut raw = json!([
        row("dup", "tool", json!(0), "first"),
        row("dup", "tool", json!(1), "second")
    ]);
    raw[1]["label"] = raw[0]["label"].clone();
    let mut after = typed_candidate(&raw);
    after["agents"].as_array_mut().unwrap().swap(0, 1);
    retain(&mut after, &raw).unwrap();
    assert_eq!(after["agents"][0]["privatePayload"]["marker"], "first");
    assert_eq!(after["agents"][1]["privatePayload"]["marker"], "second");
    after["agents"][0]["privatePayload"]["marker"] = json!("second");
    assert!(retain(&mut after, &raw).is_err());
}

#[test]
fn p02_duplicate_ambiguity_refuses_before_all_ordinary_publishers() {
    let raw = json!([
        row("dup", "claude", json!(0), "a"),
        row("dup", "codex", json!(1), "b")
    ]);
    for sidecar in [false, true] {
        for mutation in ["edit", "reorder", "add", "partial-delete", "menu"] {
            let (dir, path) = seed(&raw, sidecar);
            let mut settings = load_settings_for_cli_from_path(&path);
            match mutation {
                "edit" => settings.agents[0].command = "edited".into(),
                "reorder" => settings.agents.swap(0, 1),
                "add" => settings.agents.push(settings.agents[0].clone()),
                "partial-delete" => {
                    settings.agents.pop();
                }
                "menu" => settings.agents[0].blocking_menus = Some(vec![]),
                _ => unreachable!(),
            }
            // Read-only CLI load must not create project/backup/export files.
            let before = files(dir.path());
            let error =
                save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap_err();
            assert!(
                matches!(&error.legacy_outward,
                    SettingsSaveLegacyOutward::DiskValidation(reason)
                        if reason.contains("duplicate agent ID \
                            'dup'") && reason.contains("unprovable row correspondence")),
                "{mutation}: {error}"
            );
            assert_eq!(files(dir.path()), before, "{sidecar}/{mutation}");
        }
    }
}

#[test]
fn p02_unique_incumbent_cannot_become_ambiguous_but_new_repeated_ids_keep_behavior() {
    let raw = json!([row("incumbent", "claude", json!(0), "old")]);
    let (dir, path) = seed(&raw, false);
    let mut settings = load_settings_for_cli_from_path(&path);
    let mut extra = settings.agents[0].clone();
    extra.command = "codex".into();
    settings.agents.push(extra);
    let before = files(dir.path());
    assert!(save_settings_to_path_preserving_project_paths_typed(&settings, &path).is_err());
    assert_eq!(files(dir.path()), before);
    let mut fresh =
        json!({"agents":[row("fresh", "one", json!(0), "a"), row("fresh", "two", json!(1), "b")]});
    retain(&mut fresh, &raw).unwrap();
    for row in fresh["agents"].as_array().unwrap() {
        assert!(row.get("command").is_none());
        assert!(row.get("baseCommand").is_some());
    }
    settings.agents[1].id = "fresh".into();
    let mut another = settings.agents[1].clone();
    another.command = "third".into();
    settings.agents.push(another);
    let written = save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap();
    assert_eq!(written.agents.len(), 3);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    for row in &saved["agents"].as_array().unwrap()[1..] {
        assert_eq!(row["id"], "fresh");
        assert!(row.get("command").is_none());
    }
}

#[test]
fn p02_duplicate_no_edit_save_move_interleaving_and_whole_group_delete() {
    let raw = json!([
        row("dup", "claude", json!(1), "a"),
        row("dup", "codex", json!(0), "b"),
        row("unique", "third", json!(2), "c")
    ]);
    for sidecar in [false, true] {
        let (_dir, path) = seed(&raw, sidecar);
        let mut settings = load_settings_for_cli_from_path(&path);
        settings.agents.swap(1, 2); // Unique row moves; duplicate subsequence is unchanged.
        settings.default_shell = "unrelated-edit".into();
        let written =
            save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
        assert_eq!(saved["agents"][0]["command"], "codex");
        assert_eq!(saved["agents"][2]["command"], "claude");
        assert_eq!(saved["agents"][0]["privatePayload"]["marker"], "b");
        assert_eq!(saved["agents"][2]["privatePayload"]["marker"], "a");
        save_settings_to_path_preserving_project_paths_typed(&written, &path).unwrap();
        let again: Value =
            serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
        assert_eq!(again, saved);
        let mut deleted = written;
        deleted.agents.retain(|agent| agent.id != "dup");
        assert!(save_settings_to_path_preserving_project_paths_typed(&deleted, &path).is_ok());
    }
}

#[test]
fn p02_startup_context_binds_raw_owner_and_original_sidecar_absence() {
    let raw = json!([
        row("dup", "claude", json!(1), "a"),
        row("dup", "codex", json!(0), "b")
    ]);
    for present_without_agents in [false, true] {
        let (dir, path) = seed(&raw, false);
        if present_without_agents {
            std::fs::write(agents_instance_path(&path), b"{}").unwrap();
        }
        let mut context = None;
        let (mut settings, _, _) =
            capture_project_settings_with_startup(&path, true, Some(&mut context)).unwrap();
        let context = context.unwrap();
        assert_eq!(context.raw, raw);
        assert_eq!(context.sidecar_absent, !present_without_agents);
        if !present_without_agents {
            assert!(export_agents_to_instance_file(&mut settings, &path));
        } else {
            std::fs::write(
                agents_instance_path(&path),
                serde_json::to_vec(&json!({"agents":raw})).unwrap(),
            )
            .unwrap();
        }
        let result = save_settings_value_with_startup(
            &settings,
            &path,
            ProjectWriteMode::Preserve,
            Some(&context),
        );
        assert_eq!(result.is_ok(), !present_without_agents);
        // A changed raw group never qualifies as the verbatim owner transition.
        let mut changed = raw.clone();
        changed[0]["privatePayload"]["marker"] = json!("changed-between-capture-and-lock");
        std::fs::write(
            agents_instance_path(&path),
            serde_json::to_vec(&json!({"agents":changed})).unwrap(),
        )
        .unwrap();
        let completed_effects = files(dir.path());
        assert!(save_settings_value_with_startup(
            &settings,
            &path,
            ProjectWriteMode::Preserve,
            Some(&context)
        )
        .is_err());
        assert_eq!(files(dir.path()), completed_effects);
        assert!(project_paths_path(&path).exists()); // Independent startup migration remains.
    }
}

#[test]
fn p02_startup_records_actual_1757_append_even_when_menu_export_aborts() {
    let mut raw = json!([
        row("dup", "codex", json!(0), "codex"),
        row("dup", "claude", json!(1), "claude")
    ]);
    for row in raw.as_array_mut().unwrap() {
        row["blockingMenus"] = json!([{"custom":"invalid-but-retained"}]);
    }
    let (_dir, path) = seed(&raw, false);
    let loaded = load_settings_from_path(&path);
    assert_eq!(loaded.agents[0].blocking_menus.as_ref().unwrap().len(), 2);
    assert_eq!(loaded.agents[1].blocking_menus.as_ref().unwrap().len(), 1);
    assert!(!blocking_menus_local_path(&path).exists());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    assert_eq!(saved["agents"][0]["command"], "codex");
    assert_eq!(saved["agents"][1]["command"], "claude");
    assert_eq!(
        saved["agents"][0]["blockingMenus"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        saved["agents"][0]["privatePayload"],
        raw[0]["privatePayload"]
    );
    // Ordinary saves cannot claim the startup-only append authority.
    let mut ordinary = typed_candidate(&raw);
    ordinary["agents"] = serde_json::to_value(&loaded.agents).unwrap();
    assert!(retain(&mut ordinary, &raw).is_err());
}
