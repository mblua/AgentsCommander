use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const PROJECT_SETTINGS_FILE: &str = "project-settings.json";
pub const MAX_WORKGROUP_GROUPS: usize = 80;
pub const MAX_GROUP_ID_LEN: usize = 128;
pub const MAX_GROUP_NAME_LEN: usize = 80;
pub const MAX_GROUP_REGEX_LEN: usize = 1024;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkgroupGroup {
    pub id: String,
    pub name: String,
    pub regex: String,
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
}

impl Default for WorkgroupGroupsConfig {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            show_all: true,
            show_ungrouped: true,
        }
    }
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
        }
    }

    fn config_with_groups(groups: Vec<WorkgroupGroup>) -> WorkgroupGroupsConfig {
        WorkgroupGroupsConfig {
            groups,
            show_all: true,
            show_ungrouped: true,
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
}
