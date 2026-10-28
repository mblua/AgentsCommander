use super::*;
use serde_json::json;

mod b4a;

fn project_with_ac_root() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".ac")).expect("create .ac");
    temp
}

// #2717 (B4a) - literal oracles: no test here composes either name from a
// production constant, so a rename to the wrong name cannot stay green.
fn settings_path(project: &Path) -> PathBuf {
    project.join(".ac").join("settings.50.personal.no-git.json")
}

fn legacy_settings_path(project: &Path) -> PathBuf {
    project.join(".ac").join("project-settings.json")
}

fn group(id: &str, name: &str, regex: &str) -> WorkgroupGroup {
    WorkgroupGroup {
        id: id.to_string(),
        name: name.to_string(),
        regex: regex.to_string(),
        favorite: false,
    }
}

fn config_with_groups(groups: Vec<WorkgroupGroup>) -> WorkgroupGroupsConfig {
    WorkgroupGroupsConfig {
        groups,
        show_all: true,
        show_ungrouped: true,
        non_stop: None,
    }
}

#[test]
fn legacy_group_json_defaults_favorite_false() {
    let legacy = r#"{"groups":[{"id":"bots","name":"BOTS","regex":"^(wg-9)$"}]}"#;
    let config: WorkgroupGroupsConfig = serde_json::from_str(legacy).expect("parse legacy");
    assert!(!config.groups[0].favorite);
}

#[test]
fn favorite_flag_round_trips_through_save_load() {
    let project = project_with_ac_root();
    let mut config = config_with_groups(vec![
        group("bots", "BOTS", "^(wg-9)$"),
        group("ui", "UI", "^(wg-1)$"),
    ]);
    config.groups[0].favorite = true;

    save_workgroup_groups(project.path(), config.clone()).expect("save");
    let reloaded = load_workgroup_groups(project.path()).expect("reload");

    assert_eq!(reloaded, config);
    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(settings_path(project.path())).expect("read"),
    )
    .expect("parse");
    assert_eq!(persisted["groups"][0]["favorite"], true);
    // (#965) A NON-favorited group must still emit `false` on disk. Guard against a
    // `skip_serializing_if` creeping in later and handing the frontend `undefined`
    // where it expects a concrete boolean.
    assert_eq!(persisted["groups"][1]["favorite"], false);
}

fn populated_non_stop() -> NonStopGroupConfig {
    NonStopGroupConfig {
        show: true,
        name: "Watchers".to_string(),
        regex: "^(wg-1)$".to_string(),
        tolerance_seconds: 45,
        telegram: NonStopTelegramConfig {
            enabled: true,
            bot_id: Some("bot-7".to_string()),
        },
        sound: NonStopSoundConfig {
            enabled: true,
            seconds: 5,
        },
        // (#1257) Deliberately `true`, not `false`: `false` here would be
        // indistinguishable from the serde default, so the existing round trips
        // would not actually exercise the new field.
        favorite: true,
    }
}

#[test]
fn missing_project_settings_returns_default_groups_config() {
    let project = project_with_ac_root();

    let loaded = load_workgroup_groups(project.path()).expect("load groups");

    assert_eq!(loaded, WorkgroupGroupsConfig::default());
    assert!(!settings_path(project.path()).exists());
}

#[test]
fn empty_object_deserializes_to_defaults() {
    let project = project_with_ac_root();
    std::fs::write(settings_path(project.path()), "{}").expect("write settings");

    let loaded = load_workgroup_groups(project.path()).expect("load groups");

    assert_eq!(loaded, WorkgroupGroupsConfig::default());
}

#[test]
fn partial_json_with_only_groups_defaults_toggles_true() {
    let project = project_with_ac_root();
    std::fs::write(
        settings_path(project.path()),
        r#"{"groups":[{"id":"bots","name":"BOTS","regex":"^(wg-9)$"}]}"#,
    )
    .expect("write settings");

    let loaded = load_workgroup_groups(project.path()).expect("load groups");

    assert_eq!(loaded.groups, vec![group("bots", "BOTS", "^(wg-9)$")]);
    assert!(loaded.show_all);
    assert!(loaded.show_ungrouped);
}

#[test]
fn load_both_toggles_false_normalizes_show_all_without_rewriting() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    let original = r#"{"groups":[],"showAll":false,"showUngrouped":false}"#;
    std::fs::write(&path, original).expect("write settings");

    let loaded = load_workgroup_groups(project.path()).expect("load groups");

    assert!(loaded.show_all);
    assert!(!loaded.show_ungrouped);
    assert_eq!(
        std::fs::read_to_string(path).expect("read settings"),
        original
    );
}

#[test]
fn save_rejects_both_toggles_false() {
    let project = project_with_ac_root();
    let config = WorkgroupGroupsConfig {
        groups: Vec::new(),
        show_all: false,
        show_ungrouped: false,
        non_stop: None,
    };

    let err = save_workgroup_groups(project.path(), config).expect_err("reject config");

    assert!(err.contains("showAll"), "{err}");
    assert!(!settings_path(project.path()).exists());
}

#[test]
fn save_rejects_invalid_group_structure() {
    let project = project_with_ac_root();
    let oversized_id = "i".repeat(MAX_GROUP_ID_LEN + 1);
    let oversized_name = "n".repeat(MAX_GROUP_NAME_LEN + 1);
    let oversized_regex = "r".repeat(MAX_GROUP_REGEX_LEN + 1);
    let too_many_groups = (0..=MAX_WORKGROUP_GROUPS)
        .map(|idx| group(&format!("g{idx}"), &format!("G{idx}"), ".*"))
        .collect::<Vec<_>>();
    let cases = vec![
        (
            config_with_groups(vec![group(" ", "Name", ".*")]),
            "Group id cannot be blank",
        ),
        (
            config_with_groups(vec![group("dup", "One", ".*"), group("dup", "Two", ".*")]),
            "Duplicate group id",
        ),
        (
            config_with_groups(vec![group("id", " ", ".*")]),
            "Group name cannot be blank",
        ),
        (
            config_with_groups(vec![group("a", "Bots", ".*"), group("b", " bots ", ".*")]),
            "Duplicate group name",
        ),
        (
            config_with_groups(vec![group(&oversized_id, "Name", ".*")]),
            "Group id cannot exceed",
        ),
        (
            config_with_groups(vec![group("id", &oversized_name, ".*")]),
            "Group name cannot exceed",
        ),
        (
            config_with_groups(vec![group("id", "Name", &oversized_regex)]),
            "Group regex cannot exceed",
        ),
        (
            config_with_groups(too_many_groups),
            "At most 80 groups are allowed",
        ),
    ];

    for (config, expected) in cases {
        let err = save_workgroup_groups(project.path(), config).expect_err("reject config");
        assert!(err.contains(expected), "expected {expected:?}, got {err:?}");
    }
    assert!(!settings_path(project.path()).exists());
}

#[test]
fn load_rejects_invalid_group_structure_after_defaults() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    let too_many = (0..=MAX_WORKGROUP_GROUPS)
        .map(|idx| json!({"id": format!("g{idx}"), "name": format!("G{idx}"), "regex": ".*"}))
        .collect::<Vec<_>>();
    let cases = vec![
        (
            json!({"groups":[
                {"id":"dup","name":"One","regex":".*"},
                {"id":"dup","name":"Two","regex":".*"}
            ]}),
            "Duplicate group id",
        ),
        (
            json!({"groups":[
                {"id":"a","name":"Ops","regex":".*"},
                {"id":"b","name":" ops ","regex":".*"}
            ]}),
            "Duplicate group name",
        ),
        (
            json!({"groups":[{"id":"","name":"Name","regex":".*"}]}),
            "Group id cannot be blank",
        ),
        (
            json!({"groups":[{"id":"id","name":"","regex":".*"}]}),
            "Group name cannot be blank",
        ),
        (json!({"groups": too_many}), "At most 80 groups are allowed"),
        (
            json!({"groups":[{"id":"i".repeat(MAX_GROUP_ID_LEN + 1),"name":"Name","regex":".*"}]}),
            "Group id cannot exceed",
        ),
        (
            json!({"groups":[{"id":"id","name":"n".repeat(MAX_GROUP_NAME_LEN + 1),"regex":".*"}]}),
            "Group name cannot exceed",
        ),
        (
            json!({"groups":[
                {"id":"id","name":"Name","regex":"r".repeat(MAX_GROUP_REGEX_LEN + 1)}
            ]}),
            "Group regex cannot exceed",
        ),
    ];

    for (value, expected) in cases {
        std::fs::write(&path, serde_json::to_string(&value).expect("json"))
            .expect("write settings");
        let err = load_workgroup_groups(project.path()).expect_err("reject loaded config");
        assert!(err.contains(expected), "expected {expected:?}, got {err:?}");
    }
}

#[test]
fn save_preserves_unknown_root_keys_and_documented_agents_key() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    let original = json!({
        "agents": [
            {
                "id": "agent_1",
                "label": "Claude Code",
                "command": "codex",
                "color": "#d97706"
            }
        ],
        "tooling": {
            "custom": true
        }
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&original).expect("json"),
    )
    .expect("write settings");
    let config = config_with_groups(vec![group("bots", "BOTS", "^(wg-9)$")]);

    let saved = save_workgroup_groups(project.path(), config).expect("save groups");

    assert_eq!(saved.groups[0].id, "bots");
    let persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read settings"))
            .expect("parse settings");
    assert_eq!(persisted["agents"], original["agents"]);
    assert_eq!(persisted["tooling"], original["tooling"]);
    assert_eq!(persisted["groups"][0]["name"], "BOTS");
    assert_eq!(persisted["showAll"], true);
    assert_eq!(persisted["showUngrouped"], true);
}

#[test]
fn missing_ac_root_returns_error() {
    let project = tempfile::tempdir().expect("tempdir");

    let err = load_workgroup_groups(project.path()).expect_err("missing .ac");

    assert!(err.contains("Project has no .ac directory"), "{err}");
}

#[test]
fn malformed_json_returns_error_and_save_does_not_clobber_file() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    let malformed = "{ invalid";
    std::fs::write(&path, malformed).expect("write settings");

    let load_err = load_workgroup_groups(project.path()).expect_err("reject malformed load");
    assert!(load_err.contains("Failed to parse"), "{load_err}");

    let save_err = save_workgroup_groups(project.path(), WorkgroupGroupsConfig::default())
        .expect_err("reject malformed save");
    assert!(save_err.contains("Failed to parse"), "{save_err}");
    assert_eq!(
        std::fs::read_to_string(path).expect("read settings"),
        malformed
    );
}

#[test]
fn round_trip_save_load_preserves_unknown_root_keys() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    std::fs::write(&path, r#"{"identity":"keep-me","metadata":{"nested":1}}"#)
        .expect("write settings");
    let config = config_with_groups(vec![group("dev", "Dev", "wg-.*")]);

    let saved = save_workgroup_groups(project.path(), config).expect("save groups");
    let loaded = load_workgroup_groups(project.path()).expect("load groups");
    save_workgroup_groups(project.path(), loaded).expect("save loaded groups");

    assert_eq!(saved.groups[0].regex, "wg-.*");
    let persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read settings"))
            .expect("parse settings");
    assert_eq!(persisted["identity"], "keep-me");
    assert_eq!(persisted["metadata"]["nested"], 1);
    assert_eq!(persisted["groups"][0]["id"], "dev");
}

#[test]
fn non_object_root_returns_clear_error() {
    let project = project_with_ac_root();
    std::fs::write(settings_path(project.path()), "[]").expect("write settings");

    let err = load_workgroup_groups(project.path()).expect_err("reject non-object root");

    assert!(err.contains("must be a JSON object"), "{err}");
}

// ---- #777 Non-stop config (Change B) ----

#[test]
fn non_stop_defaults_none_for_legacy_json() {
    let legacy = concat!(
        r#"{"groups":[{"id":"bots","name":"BOTS","regex":"^(wg-9)$"}],"#,
        r#""showAll":true,"showUngrouped":true}"#
    );
    let config: WorkgroupGroupsConfig = serde_json::from_str(legacy).expect("parse legacy");
    assert_eq!(config.non_stop, None);
}

#[test]
fn normalize_migrates_legacy_non_stop_default_name() {
    let project = project_with_ac_root();
    let raw = json!({
        "groups": [],
        "showAll": true,
        "showUngrouped": true,
        "nonStop": {
            "show": true,
            "name": LEGACY_NON_STOP_NAME,
            "regex": "(?!)",
            "toleranceSeconds": 30,
            "telegram": {"enabled": false},
            "sound": {"enabled": false, "seconds": 3}
        }
    });
    std::fs::write(
        settings_path(project.path()),
        serde_json::to_string(&raw).expect("json"),
    )
    .expect("write settings");

    let loaded = load_workgroup_groups(project.path()).expect("load groups");
    let ns = loaded.non_stop.expect("nonStop present");

    assert_eq!(ns.name, DEFAULT_NON_STOP_NAME);
}

#[test]
fn save_persists_non_stop_and_preserves_unknown_keys() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    std::fs::write(
        &path,
        r#"{"agents":[{"id":"a1"}],"tooling":{"custom":true}}"#,
    )
    .expect("write settings");
    let config = WorkgroupGroupsConfig {
        groups: vec![group("bots", "BOTS", "^(wg-9)$")],
        show_all: true,
        show_ungrouped: true,
        non_stop: Some(populated_non_stop()),
    };

    save_workgroup_groups(project.path(), config).expect("save groups");
    let reloaded = load_workgroup_groups(project.path()).expect("reload groups");

    assert_eq!(reloaded.non_stop, Some(populated_non_stop()));
    let persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read settings"))
            .expect("parse settings");
    assert_eq!(persisted["agents"][0]["id"], "a1");
    assert_eq!(persisted["tooling"]["custom"], true);
    assert_eq!(persisted["nonStop"]["name"], "Watchers");
    assert_eq!(persisted["nonStop"]["toleranceSeconds"], 45);
    assert_eq!(persisted["nonStop"]["telegram"]["botId"], "bot-7");
    assert_eq!(persisted["nonStop"]["sound"]["seconds"], 5);
}

#[test]
fn save_none_removes_stale_non_stop_key() {
    let project = project_with_ac_root();
    let path = settings_path(project.path());
    let with_ns = WorkgroupGroupsConfig {
        groups: Vec::new(),
        show_all: true,
        show_ungrouped: true,
        non_stop: Some(populated_non_stop()),
    };
    save_workgroup_groups(project.path(), with_ns).expect("save with nonStop");
    // Sanity: the key is on disk before removal.
    let before: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert!(before.get("nonStop").is_some());

    let without_ns = WorkgroupGroupsConfig {
        groups: Vec::new(),
        show_all: true,
        show_ungrouped: true,
        non_stop: None,
    };
    save_workgroup_groups(project.path(), without_ns).expect("save without nonStop");

    let after: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert!(
        after.get("nonStop").is_none(),
        "stale nonStop key must be removed, got {after}"
    );
    let reloaded = load_workgroup_groups(project.path()).expect("reload");
    assert_eq!(reloaded.non_stop, None);
}

#[test]
fn non_stop_round_trips() {
    let project = project_with_ac_root();
    let config = WorkgroupGroupsConfig {
        groups: vec![group("dev", "Dev", "wg-.*")],
        show_all: true,
        show_ungrouped: false,
        non_stop: Some(populated_non_stop()),
    };

    save_workgroup_groups(project.path(), config.clone()).expect("save");
    let reloaded = load_workgroup_groups(project.path()).expect("reload");

    assert_eq!(reloaded, config);
}

#[test]
fn legacy_non_stop_json_defaults_favorite_false() {
    let legacy = concat!(
        r#"{"groups":[],"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","#,
        r#""toleranceSeconds":30,"telegram":{"enabled":false},"#,
        r#""sound":{"enabled":false,"seconds":3}}}"#
    );
    let config: WorkgroupGroupsConfig = serde_json::from_str(legacy).expect("parse legacy");
    assert!(!config.non_stop.expect("nonStop present").favorite);
}

#[test]
fn non_stop_favorite_round_trips_and_emits_false_when_unset() {
    let project = project_with_ac_root();
    let config = WorkgroupGroupsConfig {
        groups: Vec::new(),
        show_all: true,
        show_ungrouped: true,
        non_stop: Some(populated_non_stop()), // favorite: true
    };

    save_workgroup_groups(project.path(), config.clone()).expect("save");
    let reloaded = load_workgroup_groups(project.path()).expect("reload");
    assert_eq!(reloaded, config);
    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(settings_path(project.path())).expect("read"),
    )
    .expect("parse");
    assert_eq!(persisted["nonStop"]["favorite"], true);

    // (#965 convention, #1257) A NON-favorited Non-stop must still emit an
    // explicit `false` on disk, or a later `skip_serializing_if` would hand the
    // frontend `undefined` where it expects a concrete boolean.
    let mut off = config;
    off.non_stop.as_mut().expect("nonStop").favorite = false;
    save_workgroup_groups(project.path(), off).expect("save non-favorite");
    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(settings_path(project.path())).expect("read"),
    )
    .expect("parse");
    assert_eq!(persisted["nonStop"]["favorite"], false);
}

#[test]
fn non_stop_favorite_survives_deserialization_from_frontend_json() {
    let project = project_with_ac_root();
    // The shape `update_project_groups` receives from the store, favorite included.
    let incoming = concat!(
        r#"{"groups":[],"showAll":true,"showUngrouped":true,"#,
        r#""nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","#,
        r#""toleranceSeconds":30,"telegram":{"enabled":false},"#,
        r#""sound":{"enabled":false,"seconds":3},"favorite":true}}"#
    );
    let config: WorkgroupGroupsConfig = serde_json::from_str(incoming).expect("parse incoming");
    assert!(config.non_stop.as_ref().expect("nonStop present").favorite);

    save_workgroup_groups(project.path(), config).expect("save");
    let reloaded = load_workgroup_groups(project.path()).expect("reload");
    assert!(reloaded.non_stop.expect("nonStop present").favorite);
}

#[test]
fn normalize_clamps_out_of_range_non_stop_and_keeps_groups() {
    let project = project_with_ac_root();
    // 2 valid groups + a nonStop with tolerance 0 and sound.seconds 999.
    let raw = json!({
        "groups": [
            {"id":"a","name":"Alpha","regex":"^(wg-1)$"},
            {"id":"b","name":"Beta","regex":"^(wg-2)$"}
        ],
        "showAll": true,
        "showUngrouped": true,
        "nonStop": {
            "show": true,
            "name": "Watch",
            "regex": "^(wg-1)$",
            "toleranceSeconds": 0,
            "telegram": {"enabled": false},
            "sound": {"enabled": true, "seconds": 999}
        }
    });
    std::fs::write(
        settings_path(project.path()),
        serde_json::to_string(&raw).expect("json"),
    )
    .expect("write settings");

    let loaded = load_workgroup_groups(project.path()).expect("load must not error");

    assert_eq!(loaded.groups.len(), 2, "groups must survive a bad nonStop");
    let ns = loaded.non_stop.expect("nonStop present");
    assert_eq!(ns.tolerance_seconds, 1, "tolerance clamped up to 1");
    assert_eq!(ns.sound.seconds, 60, "sound seconds clamped down to 60");
}

/// #2717 (B4a) E5: the three-way layer-50 chain. Deliberately absent from
/// `live_names_are_the_target_names` (`config/instance_gitignore.rs`): naming
/// `project_settings` there would widen the frozen super-reference table of
/// `tests/instance_gitignore_layering.rs`, which keeps that module at arm's
/// length from `project_settings`, a neighbour of the cycle through
/// `web::commands`.
#[test]
fn project_settings_file_is_the_layer_50_settings_name() {
    use crate::config::instance_artifacts::{
        PROJECT_SETTINGS_TARGET_NAME, SETTINGS_LOCAL_TARGET_NAME,
    };
    assert_eq!(PROJECT_SETTINGS_FILE, PROJECT_SETTINGS_TARGET_NAME);
    assert_eq!(PROJECT_SETTINGS_TARGET_NAME, SETTINGS_LOCAL_TARGET_NAME);
    assert_eq!(PROJECT_SETTINGS_FILE, "settings.50.personal.no-git.json");
}

#[test]
fn issue_3361_settings_ignore_rows_preserve_prefix_and_errors() {
    for prefix in [None, Some("sentinel\n"), Some("sentinel")] {
        let temp = project_with_ac_root();
        let ac_root = temp.path().join(".ac");
        let path = ac_root.join(".gitignore");
        if let Some(prefix) = prefix {
            std::fs::write(&path, prefix).unwrap();
        }
        ensure_project_settings_ignore_rows(&ac_root).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let expected_prefix = if prefix.is_some() { "sentinel\n" } else { "" };
        assert!(
            content.starts_with(expected_prefix),
            "prefix lost: {content:?}"
        );
        for (row, _) in naming_migration::project_settings_ignore_rows() {
            assert!(content.lines().any(|line| line == row));
        }
        ensure_project_settings_ignore_rows(&ac_root).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }
    let temp = project_with_ac_root();
    let ac_root = temp.path().join(".ac");
    let path = ac_root.join(".gitignore");
    std::fs::create_dir(&path).unwrap();
    let refusal = ensure_project_settings_ignore_rows(&ac_root).unwrap_err();
    match refusal {
        naming_migration::Refusal::Io(message) => {
            assert!(message.starts_with(&format!("failed to read {}:", path.display())));
        }
        other => panic!("unexpected refusal: {other:?}"),
    }
    assert!(path.is_dir());
    assert!(!settings_path(temp.path()).exists());
}
