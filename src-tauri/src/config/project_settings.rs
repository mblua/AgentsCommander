use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const PROJECT_SETTINGS_FILE: &str = "project-settings.json";
pub const MAX_WORKGROUP_GROUPS: usize = 80;
pub const MAX_GROUP_ID_LEN: usize = 128;
pub const MAX_GROUP_NAME_LEN: usize = 80;
pub const MAX_GROUP_REGEX_LEN: usize = 1024;
const DEFAULT_NON_STOP_NAME: &str = "Alert me!";
const LEGACY_NON_STOP_NAME: &str = "Non-stop";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkgroupGroup {
    pub id: String,
    pub name: String,
    pub regex: String,
    /// (#965) Pinned into the rail's cross-project `Favorites` section. Absent on
    /// legacy configs => false. Lives on the group record so it survives rename
    /// (`id` is a stable UUID), travels with reorder, and dies with the group.
    #[serde(default)]
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkgroupGroupsConfig {
    #[serde(default)]
    pub groups: Vec<WorkgroupGroup>,
    #[serde(default = "default_true")]
    pub show_all: bool,
    #[serde(default = "default_true")]
    pub show_ungrouped: bool,
    /// (#777) Built-in optional Non-stop watchdog group. Absent on legacy configs.
    #[serde(default)]
    pub non_stop: Option<NonStopGroupConfig>,
}

impl Default for WorkgroupGroupsConfig {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            show_all: true,
            show_ungrouped: true,
            non_stop: None,
        }
    }
}

// (#777) Non-stop watchdog config, mirrored to `NonStopGroupConfig` in
// `src/shared/types.ts`. All numeric ranges are repaired (clamped) on load by
// `normalize_groups_config`, never fatally rejected, so a bad/hand-edited value
// can never nuke the user's real groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NonStopTelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NonStopSoundConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_beep_seconds")]
    pub seconds: u32,
}
impl Default for NonStopSoundConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            seconds: default_beep_seconds(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NonStopGroupConfig {
    #[serde(default)]
    pub show: bool,
    #[serde(default = "default_non_stop_name")]
    pub name: String,
    #[serde(default = "default_match_none_regex")]
    pub regex: String,
    #[serde(default = "default_tolerance_seconds")]
    pub tolerance_seconds: u32,
    #[serde(default)]
    pub telegram: NonStopTelegramConfig,
    #[serde(default)]
    pub sound: NonStopSoundConfig,
    /// (#1257) Pinned into the rail's cross-project `Favorites` section, mirroring
    /// the group flag added by #965. Absent on legacy configs => false. Lives on
    /// the record, so it dies with it: `save_workgroup_groups` removes the whole
    /// `nonStop` key when `non_stop` is `None`, which makes an orphan favorite
    /// impossible. No `skip_serializing_if`: a non-favorite must still emit an
    /// explicit `false`, same convention as `WorkgroupGroup::favorite`.
    #[serde(default)]
    pub favorite: bool,
}
impl Default for NonStopGroupConfig {
    fn default() -> Self {
        Self {
            show: false,
            name: default_non_stop_name(),
            regex: default_match_none_regex(),
            tolerance_seconds: default_tolerance_seconds(),
            telegram: NonStopTelegramConfig::default(),
            sound: NonStopSoundConfig::default(),
            favorite: false,
        }
    }
}

fn default_beep_seconds() -> u32 {
    3
}
fn default_tolerance_seconds() -> u32 {
    30
}
fn default_non_stop_name() -> String {
    DEFAULT_NON_STOP_NAME.to_string()
}
fn default_match_none_regex() -> String {
    "(?!)".to_string()
}

fn project_settings_path(project_path: &Path) -> Result<PathBuf, String> {
    let workspace = crate::config::workspace::existing_workspace_dir(project_path)
        .ok_or_else(|| format!("Project has no .ac directory: {}", project_path.display()))?;
    Ok(workspace.join(PROJECT_SETTINGS_FILE))
}

fn normalize_groups_config(mut config: WorkgroupGroupsConfig) -> WorkgroupGroupsConfig {
    if !config.show_all && !config.show_ungrouped {
        config.show_all = true;
    }
    // (#777 B3) Repair an out-of-range Non-stop config in place instead of failing
    // validation. A bad `nonStop` must never nuke the user's real groups, so these
    // are lossless-to-the-user fixes: they only touch already-invalid data.
    if let Some(ns) = config.non_stop.as_mut() {
        ns.tolerance_seconds = ns.tolerance_seconds.clamp(1, 3600);
        ns.sound.seconds = ns.sound.seconds.clamp(1, 60);
        if ns.name.trim().is_empty() || ns.name.trim() == LEGACY_NON_STOP_NAME {
            ns.name = default_non_stop_name();
        } else if ns.name.chars().count() > MAX_GROUP_NAME_LEN {
            ns.name = ns.name.chars().take(MAX_GROUP_NAME_LEN).collect();
        }
        if ns.regex.chars().count() > MAX_GROUP_REGEX_LEN {
            ns.regex = default_match_none_regex();
        }
    }
    config
}

fn validate_groups_config_structure(config: &WorkgroupGroupsConfig) -> Result<(), String> {
    if !config.show_all && !config.show_ungrouped {
        return Err("At least one of showAll or showUngrouped must be true".to_string());
    }
    if config.groups.len() > MAX_WORKGROUP_GROUPS {
        return Err(format!("At most {MAX_WORKGROUP_GROUPS} groups are allowed"));
    }

    let mut ids = HashSet::new();
    let mut names = HashSet::new();

    for group in &config.groups {
        let id = group.id.trim();
        if id.is_empty() {
            return Err("Group id cannot be blank".to_string());
        }
        if group.id.chars().count() > MAX_GROUP_ID_LEN {
            return Err(format!(
                "Group id cannot exceed {MAX_GROUP_ID_LEN} characters"
            ));
        }
        if !ids.insert(id.to_string()) {
            return Err("Duplicate group id".to_string());
        }

        let name = group.name.trim();
        if name.is_empty() {
            return Err("Group name cannot be blank".to_string());
        }
        if group.name.chars().count() > MAX_GROUP_NAME_LEN {
            return Err(format!(
                "Group name cannot exceed {MAX_GROUP_NAME_LEN} characters"
            ));
        }
        if !names.insert(name.to_lowercase()) {
            return Err("Duplicate group name".to_string());
        }

        if group.regex.chars().count() > MAX_GROUP_REGEX_LEN {
            return Err(format!(
                "Group regex cannot exceed {MAX_GROUP_REGEX_LEN} characters"
            ));
        }
    }

    Ok(())
}

pub fn load_workgroup_groups(project_path: &Path) -> Result<WorkgroupGroupsConfig, String> {
    let path = project_settings_path(project_path)?;
    if !path.exists() {
        return Ok(WorkgroupGroupsConfig::default());
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let root: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    let obj = root
        .as_object()
        .ok_or_else(|| format!("Project settings {} must be a JSON object", path.display()))?;
    let config: WorkgroupGroupsConfig = serde_json::from_value(Value::Object(obj.clone()))
        .map_err(|e| {
            format!(
                "Failed to parse project groups from {}: {}",
                path.display(),
                e
            )
        })?;
    let config = normalize_groups_config(config);
    validate_groups_config_structure(&config)?;
    Ok(config)
}

pub fn save_workgroup_groups(
    project_path: &Path,
    config: WorkgroupGroupsConfig,
) -> Result<WorkgroupGroupsConfig, String> {
    validate_groups_config_structure(&config)?;
    let path = project_settings_path(project_path)?;

    // This is a last-successful-writer-wins update across processes. The shared
    // helper serializes in-process writes and prevents torn JSON, but it is not
    // a merge or compare-and-swap layer. If another process wins a first-create
    // race on Windows after the helper's existence precheck, surfacing that
    // filesystem error is acceptable and the caller keeps its prior state.
    crate::config::local_config_io::update_config_json_object(&path, true, |obj| {
        let groups = serde_json::to_value(&config.groups)
            .map_err(|e| format!("Failed to serialize project groups: {}", e))?;
        obj.insert("groups".to_string(), groups);
        obj.insert("showAll".to_string(), Value::Bool(config.show_all));
        obj.insert(
            "showUngrouped".to_string(),
            Value::Bool(config.show_ungrouped),
        );
        // (#777 G2) Persist the built-in Non-stop group. Some -> write it; None ->
        // REMOVE the on-disk key so it is truly absent on reload. The merge helper
        // would otherwise preserve a stale `nonStop` and resurrect it as `Some`.
        // `remove` is a no-op when the key was never present, so legacy configs
        // stay clean. (Product model: `show: false` is the only off-switch; `None`
        // only arises from legacy/manual JSON, but this arm keeps that path correct.)
        match &config.non_stop {
            Some(ns) => {
                obj.insert(
                    "nonStop".to_string(),
                    serde_json::to_value(ns).map_err(|e| e.to_string())?,
                );
            }
            None => {
                obj.remove("nonStop");
            }
        }
        Ok(())
    })?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project_with_workspace() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".ac")).expect("create .ac");
        temp
    }

    fn settings_path(project: &Path) -> PathBuf {
        project.join(".ac").join(PROJECT_SETTINGS_FILE)
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();

        let loaded = load_workgroup_groups(project.path()).expect("load groups");

        assert_eq!(loaded, WorkgroupGroupsConfig::default());
        assert!(!settings_path(project.path()).exists());
    }

    #[test]
    fn empty_object_deserializes_to_defaults() {
        let project = project_with_workspace();
        std::fs::write(settings_path(project.path()), "{}").expect("write settings");

        let loaded = load_workgroup_groups(project.path()).expect("load groups");

        assert_eq!(loaded, WorkgroupGroupsConfig::default());
    }

    #[test]
    fn partial_json_with_only_groups_defaults_toggles_true() {
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();
        let path = settings_path(project.path());
        let too_many = (0..=MAX_WORKGROUP_GROUPS)
            .map(|idx| json!({"id": format!("g{idx}"), "name": format!("G{idx}"), "regex": ".*"}))
            .collect::<Vec<_>>();
        let cases = vec![
            (
                json!({"groups":[{"id":"dup","name":"One","regex":".*"},{"id":"dup","name":"Two","regex":".*"}]}),
                "Duplicate group id",
            ),
            (
                json!({"groups":[{"id":"a","name":"Ops","regex":".*"},{"id":"b","name":" ops ","regex":".*"}]}),
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
                json!({"groups":[{"id":"id","name":"Name","regex":"r".repeat(MAX_GROUP_REGEX_LEN + 1)}]}),
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
        let project = project_with_workspace();
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
    fn missing_workspace_returns_error() {
        let project = tempfile::tempdir().expect("tempdir");

        let err = load_workgroup_groups(project.path()).expect_err("missing .ac");

        assert!(err.contains("Project has no .ac directory"), "{err}");
    }

    #[test]
    fn malformed_json_returns_error_and_save_does_not_clobber_file() {
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();
        std::fs::write(settings_path(project.path()), "[]").expect("write settings");

        let err = load_workgroup_groups(project.path()).expect_err("reject non-object root");

        assert!(err.contains("must be a JSON object"), "{err}");
    }

    // ---- #777 Non-stop config (Change B) ----

    #[test]
    fn non_stop_defaults_none_for_legacy_json() {
        let legacy = r#"{"groups":[{"id":"bots","name":"BOTS","regex":"^(wg-9)$"}],"showAll":true,"showUngrouped":true}"#;
        let config: WorkgroupGroupsConfig = serde_json::from_str(legacy).expect("parse legacy");
        assert_eq!(config.non_stop, None);
    }

    #[test]
    fn normalize_migrates_legacy_non_stop_default_name() {
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let project = project_with_workspace();
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
        let legacy = r#"{"groups":[],"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3}}}"#;
        let config: WorkgroupGroupsConfig = serde_json::from_str(legacy).expect("parse legacy");
        assert!(!config.non_stop.expect("nonStop present").favorite);
    }

    #[test]
    fn non_stop_favorite_round_trips_and_emits_false_when_unset() {
        let project = project_with_workspace();
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
        let project = project_with_workspace();
        // The shape `update_project_groups` receives from the store, favorite included.
        let incoming = r#"{"groups":[],"showAll":true,"showUngrouped":true,"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3},"favorite":true}}"#;
        let config: WorkgroupGroupsConfig = serde_json::from_str(incoming).expect("parse incoming");
        assert!(config.non_stop.as_ref().expect("nonStop present").favorite);

        save_workgroup_groups(project.path(), config).expect("save");
        let reloaded = load_workgroup_groups(project.path()).expect("reload");
        assert!(reloaded.non_stop.expect("nonStop present").favorite);
    }

    #[test]
    fn normalize_clamps_out_of_range_non_stop_and_keeps_groups() {
        let project = project_with_workspace();
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
}
