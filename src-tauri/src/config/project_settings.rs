use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::naming_migration;

/// #2717 (B4a) - the layer-50 name, composed once in the registry.
pub const PROJECT_SETTINGS_FILE: &str =
    crate::config::instance_artifacts::PROJECT_SETTINGS_TARGET_NAME;
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

/// #2717 (B4a) - the pre-migration name, moved to `PROJECT_SETTINGS_FILE`.
const LEGACY_PROJECT_SETTINGS_RENAME: naming_migration::Rename = naming_migration::Rename {
    from: "project-settings.json",
    to: PROJECT_SETTINGS_FILE,
};

/// #2717 (B4a) - the pre-migration lock sidecar. Held beside the new one for the
/// whole scope, so a pre-migration binary is still excluded; never renamed.
const LEGACY_PROJECT_SETTINGS_LOCK: &str = ".project-settings.json.lock";

#[cfg(not(test))]
#[inline(always)]
fn sweep_pause_hook(_stage: &str, _dir: &Path) {}

/// Test-only pause point between the sweep's read and its append. Inert unless
/// a test arms it.
#[cfg(test)]
fn sweep_pause_hook(stage: &str, dir: &Path) {
    naming_migration::pause::hook(stage, dir);
}

/// #2717 (B4a) 4.4 - appends to `<ac_root>/.gitignore` the project-settings
/// rows it lacks. An appending write, never a rewrite, so no line another
/// writer added can be lost; retirement of the old row stays with the writer
/// at registration.
fn ensure_project_settings_ignore_rows(ac_root: &Path) -> Result<(), naming_migration::Refusal> {
    use std::io::Write as _;
    let path = ac_root.join(".gitignore");
    let io = |what: &str, e: std::io::Error| {
        naming_migration::Refusal::Io(format!("failed to {what} {}: {e}", path.display()))
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io("read", e)),
    };
    let blocks = naming_migration::missing_ignore_rows(
        &content,
        &naming_migration::project_settings_ignore_rows(),
    );
    if blocks.is_empty() {
        return Ok(());
    }
    sweep_pause_hook("before_gitignore_append", ac_root);
    let separator = if content.is_empty() || content.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(format!("{separator}{blocks}").as_bytes()))
        .map_err(|e| io("append to", e))
}

fn project_settings_scope_key(ac_root: &Path) -> String {
    format!("project-settings:{}", ac_root.display())
}

/// #2717 (B4a) - moves `project-settings.json` to `PROJECT_SETTINGS_FILE` in
/// `ac_root`, under both lock sidecars, journalled in `journal_dir`. `Ok` is
/// always the new name; every refusal is an `Err`, so no caller ever writes the
/// old name. Both names present: the new file wins and the old one is set aside.
fn migrate_project_settings_name(
    ac_root: &Path,
    journal_dir: Option<&Path>,
) -> Result<&'static str, String> {
    let key = project_settings_scope_key(ac_root);
    let refused = |refusal: naming_migration::Refusal| match refusal {
        naming_migration::Refusal::LockUnavailable => format!(
            "Another process is migrating the project settings in {}; try again",
            ac_root.display()
        ),
        naming_migration::Refusal::Io(message) => format!(
            "The project settings in {} could not be migrated: {message}",
            ac_root.display()
        ),
    };
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    let journal = naming_migration::read_journal(journal_dir).map_err(refused)?;
    if naming_migration::scope_is_settled(journal.as_ref(), &key) {
        return Ok(PROJECT_SETTINGS_FILE);
    }
    // The new sidecar is the one the migrated writers take (`.<name>.lock`).
    let new_lock = format!(".{PROJECT_SETTINGS_FILE}.lock");
    let held = naming_migration::lock_scope(
        ac_root,
        &new_lock,
        Some(LEGACY_PROJECT_SETTINGS_LOCK),
        naming_migration::MIGRATION_LOCK_BUDGET,
    )
    .map_err(refused)?;
    // #2717 4.4: the new names are ignored before anything takes them; a failed
    // sweep refuses, so nothing is renamed into an unignored state.
    ensure_project_settings_ignore_rows(ac_root).map_err(refused)?;
    if let naming_migration::Outcome::Refused(refusal) = naming_migration::rename_step(
        ac_root,
        &LEGACY_PROJECT_SETTINGS_RENAME,
        &key,
        journal_dir,
        &held,
    ) {
        return Err(refused(refusal));
    }
    let lock_left = ac_root.join(LEGACY_PROJECT_SETTINGS_LOCK).exists();
    naming_migration::update_journal(journal_dir, |j| {
        if lock_left {
            j.note(
                &key,
                "left on disk: .project-settings.json.lock (a lock sidecar is never renamed or deleted)",
            );
        }
        j.set_status(&key, naming_migration::ScopeStatus::Complete);
    })
    .map_err(refused)?;
    Ok(PROJECT_SETTINGS_FILE)
}

/// #2717 (B4a) - the chokepoint: both production entries resolve their path
/// here, so the migration runs before any read or write, and the name returned
/// is the one the migration decided.
fn project_settings_path_in(
    project_path: &Path,
    journal_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let ac_root = crate::config::ac_root::existing_ac_root(project_path)
        .ok_or_else(|| format!("Project has no .ac directory: {}", project_path.display()))?;
    let name = migrate_project_settings_name(&ac_root, journal_dir)?;
    Ok(ac_root.join(name))
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
    load_workgroup_groups_in(project_path, crate::config::config_dir().as_deref())
}

/// #2717 (B4a) - `load_workgroup_groups` with an explicit journal directory.
fn load_workgroup_groups_in(
    project_path: &Path,
    journal_dir: Option<&Path>,
) -> Result<WorkgroupGroupsConfig, String> {
    let path = project_settings_path_in(project_path, journal_dir)?;
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
    save_workgroup_groups_in(project_path, config, crate::config::config_dir().as_deref())
}

/// #2717 (B4a) - `save_workgroup_groups` with an explicit journal directory.
fn save_workgroup_groups_in(
    project_path: &Path,
    config: WorkgroupGroupsConfig,
    journal_dir: Option<&Path>,
) -> Result<WorkgroupGroupsConfig, String> {
    validate_groups_config_structure(&config)?;
    let path = project_settings_path_in(project_path, journal_dir)?;

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
        let legacy = r#"{"groups":[{"id":"bots","name":"BOTS","regex":"^(wg-9)$"}],"showAll":true,"showUngrouped":true}"#;
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
        let legacy = r#"{"groups":[],"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3}}}"#;
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
        let incoming = r#"{"groups":[],"showAll":true,"showUngrouped":true,"nonStop":{"show":true,"name":"Watchers","regex":"^(wg-1)$","toleranceSeconds":30,"telegram":{"enabled":false},"sound":{"enabled":false,"seconds":3},"favorite":true}}"#;
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

    // #2717 (B4a) - rows E6 to E13 of the phase plan. Every name below is a
    // literal (4.1): no fixture composes a project-settings name from a constant.

    const B4A_LEGACY_BYTES: &str = "{\n  \"groups\": [{\"id\": \"g1\", \"name\": \"Legacy\", \"regex\": \"^legacy-\"}],\n  \"showAll\": true,\n  \"showUngrouped\": false,\n  \"userKey\": 7\n}\n";
    const B4A_WINNER_BYTES: &str = "{\"groups\": [{\"id\": \"g2\", \"name\": \"Winner\", \"regex\": \"^win-\"}], \"showAll\": true, \"showUngrouped\": true}";
    const B4A_OLD_LOCK: &str = ".project-settings.json.lock";
    const B4A_NEW_LOCK: &str = ".settings.50.personal.no-git.json.lock";
    const B4A_SET_ASIDE: &str = "project-settings.json.deprecated-1.no-git";

    struct B4aFixture {
        _tmp: tempfile::TempDir,
        project: PathBuf,
        cfg: PathBuf,
    }

    fn b4a_fixture() -> B4aFixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
        std::fs::create_dir_all(&cfg).expect("create cfg");
        B4aFixture {
            _tmp: tmp,
            project,
            cfg,
        }
    }

    fn b4a_ac_root(project: &Path) -> PathBuf {
        crate::config::ac_root::existing_ac_root(project).expect("the fixture has a .ac")
    }

    fn b4a_key(project: &Path) -> String {
        format!("project-settings:{}", b4a_ac_root(project).display())
    }

    fn b4a_record(cfg: &Path, project: &Path) -> Option<naming_migration::ScopeRecord> {
        naming_migration::read_journal(Some(cfg))
            .expect("readable journal")
            .and_then(|journal| journal.scope(&b4a_key(project)).cloned())
    }

    fn b4a_names(config: &WorkgroupGroupsConfig) -> Vec<String> {
        config.groups.iter().map(|g| g.name.clone()).collect()
    }

    fn b4a_entries(dir: &Path) -> std::collections::BTreeSet<String> {
        std::fs::read_dir(dir)
            .expect("list dir")
            .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
            .collect()
    }

    fn b4a_saved() -> WorkgroupGroupsConfig {
        config_with_groups(vec![group("g3", "Saved", "^saved-")])
    }

    #[test]
    fn every_entry_point_migrates_before_it_resolves_the_path() {
        for entry in ["load", "save"] {
            let f = b4a_fixture();
            std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
            std::fs::write(f.project.join(".ac").join(B4A_OLD_LOCK), b"").unwrap();

            if entry == "load" {
                let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg))
                    .expect("load migrates and reads");
                assert_eq!(b4a_names(&loaded), ["Legacy"], "{entry}");
                assert_eq!(
                    std::fs::read(settings_path(&f.project)).expect("new name"),
                    B4A_LEGACY_BYTES.as_bytes(),
                    "{entry}: the bytes did not survive under the new name"
                );
            } else {
                save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg))
                    .expect("save migrates and writes");
                let disk: Value = serde_json::from_str(
                    &std::fs::read_to_string(settings_path(&f.project)).expect("new name"),
                )
                .unwrap();
                assert_eq!(disk["groups"][0]["name"], "Saved", "{entry}");
                assert_eq!(
                    disk["userKey"], 7,
                    "{entry}: the save wrote a fresh file instead of the migrated one"
                );
            }
            assert!(!legacy_settings_path(&f.project).exists(), "{entry}");
            assert!(
                f.project.join(".ac").join(B4A_OLD_LOCK).exists(),
                "{entry}: the old lock sidecar must stay"
            );
            let record = b4a_record(&f.cfg, &f.project).expect("scope recorded");
            assert_eq!(
                record.status,
                naming_migration::ScopeStatus::Complete,
                "{entry}"
            );
            assert!(
                record.notes.iter().any(|n| n.contains(B4A_OLD_LOCK)),
                "{entry}: the old lock is not noted: {:?}",
                record.notes
            );
        }
    }

    #[test]
    fn a_pre_existing_target_wins_and_the_old_file_is_set_aside() {
        let f = b4a_fixture();
        let ac = f.project.join(".ac");
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        std::fs::write(settings_path(&f.project), B4A_WINNER_BYTES).unwrap();
        std::fs::write(ac.join(B4A_OLD_LOCK), b"").unwrap();
        std::fs::write(ac.join(B4A_NEW_LOCK), b"").unwrap();
        std::fs::write(ac.join(".gitignore"), b"# user rules\n").unwrap();
        let before = b4a_entries(&ac);

        let loaded =
            load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("both present is Ok");

        assert_eq!(b4a_names(&loaded), ["Winner"]);
        assert_eq!(
            std::fs::read(settings_path(&f.project)).unwrap(),
            B4A_WINNER_BYTES.as_bytes()
        );
        assert!(!legacy_settings_path(&f.project).exists());
        assert_eq!(
            std::fs::read(ac.join(B4A_SET_ASIDE)).expect("set aside"),
            B4A_LEGACY_BYTES.as_bytes()
        );
        let mut expected = before.clone();
        expected.remove("project-settings.json");
        expected.insert(B4A_SET_ASIDE.to_string());
        assert_eq!(
            b4a_entries(&ac),
            expected,
            "the .ac directory changed otherwise"
        );
        let record = b4a_record(&f.cfg, &f.project).expect("scope recorded");
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        assert!(record
            .steps
            .iter()
            .any(|s| s.state == naming_migration::StepState::SetAside));

        let saved = save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg))
            .expect("the save after a set-aside is Ok");
        assert_eq!(b4a_names(&saved), ["Saved"]);
        assert!(
            !legacy_settings_path(&f.project).exists(),
            "a third file was created"
        );
        assert_eq!(b4a_entries(&ac), expected, "the save created another file");

        // The set-aside file is ignored in the user's repository.
        crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[])
            .expect("write .ac/.gitignore");
        assert!(b4a_git(&f.project, &["init", "--quiet"]).status.success());
        let set_aside = format!(".ac/{B4A_SET_ASIDE}");
        assert!(
            b4a_git(
                &f.project,
                &["check-ignore", "--no-index", "-q", "--", &set_aside]
            )
            .status
            .success(),
            "the set-aside file is not ignored"
        );
    }

    #[test]
    fn a_refused_lock_is_an_error_and_creates_nothing() {
        let f = b4a_fixture();
        let ac_root = b4a_ac_root(&f.project);
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        let held = naming_migration::lock_scope(
            &ac_root,
            B4A_OLD_LOCK,
            None,
            naming_migration::MIGRATION_LOCK_BUDGET,
        )
        .expect("hold the old sidecar");

        let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg));
        let saved = save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg));

        for (entry, error) in [("load", loaded.map(|_| ())), ("save", saved.map(|_| ()))] {
            let error = error.expect_err(entry);
            assert!(
                error.contains(&ac_root.display().to_string()),
                "{entry}: the error does not name the .ac directory: {error}"
            );
        }
        assert!(
            !settings_path(&f.project).exists(),
            "a refused migration created the destination"
        );
        assert_eq!(
            std::fs::read(legacy_settings_path(&f.project)).unwrap(),
            B4A_LEGACY_BYTES.as_bytes()
        );

        drop(held);
        let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("lock free");
        assert_eq!(b4a_names(&loaded), ["Legacy"]);
        assert!(!legacy_settings_path(&f.project).exists());
    }

    #[test]
    fn an_io_failure_after_the_rename_is_an_error_not_the_old_name() {
        let f = b4a_fixture();
        let ac_root = b4a_ac_root(&f.project);
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        let journal_file = f
            .cfg
            .join(crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME);
        let armed = naming_migration::pause::arm("after_rename", &ac_root);
        let (project, cfg) = (f.project.clone(), f.cfg.clone());
        let worker = std::thread::spawn(move || load_workgroup_groups_in(&project, Some(&cfg)));
        armed
            .reached
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the migration reached the rename");
        // Renamed on disk, and the `Renamed` commit is about to fail.
        std::fs::remove_file(&journal_file).expect("journal written by `Renaming`");
        std::fs::create_dir(&journal_file).expect("block the journal");
        armed.release.send(()).unwrap();

        let loaded = worker.join().unwrap();
        assert!(
            loaded.is_err(),
            "an Io after the rename must be an error, got {loaded:?}"
        );
        assert_eq!(
            std::fs::read(settings_path(&f.project)).unwrap(),
            B4A_LEGACY_BYTES.as_bytes()
        );
        assert!(
            project_settings_path_in(&f.project, Some(&f.cfg)).is_err(),
            "the path must be an error while the disk state is unknown"
        );
        assert!(
            save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg)).is_err(),
            "the save must refuse"
        );
        assert!(
            !legacy_settings_path(&f.project).exists(),
            "the create-if-missing writer rebuilt the legacy file"
        );

        std::fs::remove_dir(&journal_file).unwrap();
        let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("next call");
        assert_eq!(b4a_names(&loaded), ["Legacy"]);
        assert_eq!(
            b4a_record(&f.cfg, &f.project).expect("recorded").status,
            naming_migration::ScopeStatus::Complete
        );
    }

    #[test]
    fn an_unreachable_project_is_skipped_and_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let missing = tmp.path().join("missing");
        let a_file = tmp.path().join("a-file");
        std::fs::write(&a_file, b"not a directory").unwrap();
        let no_ac = tmp.path().join("no-ac");
        std::fs::create_dir_all(&no_ac).unwrap();
        // `.ac` present but unwritable for the scope: a Windows directory's
        // read-only attribute does not stop a create, so the new lock sidecar's
        // name is taken by a directory and the scope cannot open it.
        let blocked = tmp.path().join("blocked");
        std::fs::create_dir_all(blocked.join(".ac").join(B4A_NEW_LOCK)).unwrap();
        std::fs::write(legacy_settings_path(&blocked), B4A_LEGACY_BYTES).unwrap();

        for project in [&missing, &a_file, &no_ac, &blocked] {
            assert!(
                load_workgroup_groups_in(project, Some(&cfg)).is_err(),
                "{} must be refused",
                project.display()
            );
        }
        let status = b4a_record(&cfg, &blocked).map(|r| r.status);
        assert_ne!(status, Some(naming_migration::ScopeStatus::Complete));

        std::fs::remove_file(&a_file).unwrap();
        std::fs::remove_dir(blocked.join(".ac").join(B4A_NEW_LOCK)).unwrap();
        for project in [&missing, &a_file, &no_ac] {
            std::fs::create_dir_all(project.join(".ac")).unwrap();
            std::fs::write(legacy_settings_path(project), B4A_LEGACY_BYTES).unwrap();
        }
        for project in [&missing, &a_file, &no_ac, &blocked] {
            let loaded = load_workgroup_groups_in(project, Some(&cfg))
                .unwrap_or_else(|e| panic!("{} retried: {e}", project.display()));
            assert_eq!(b4a_names(&loaded), ["Legacy"]);
            assert!(!legacy_settings_path(project).exists());
            assert_eq!(
                b4a_record(&cfg, project).expect("recorded").status,
                naming_migration::ScopeStatus::Complete
            );
        }
    }

    #[test]
    fn no_journal_dir_still_migrates_and_never_creates_a_second_file() {
        let f = b4a_fixture();
        let ac = f.project.join(".ac");
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();

        let saved = save_workgroup_groups_in(&f.project, b4a_saved(), None).expect("Ok");

        assert_eq!(b4a_names(&saved), ["Saved"]);
        let disk: Value =
            serde_json::from_str(&std::fs::read_to_string(settings_path(&f.project)).unwrap())
                .unwrap();
        assert_eq!(disk["groups"][0]["name"], "Saved");
        assert_eq!(disk["userKey"], 7);
        assert!(!legacy_settings_path(&f.project).exists());
        assert!(
            b4a_entries(&f.cfg).is_empty(),
            "a journal-less run wrote into a configuration directory"
        );
        assert!(
            b4a_entries(&ac)
                .iter()
                .all(|n| !n.starts_with("naming-migration") && !n.ends_with(".tmp")),
            "a journal or temporary landed in the project: {:?}",
            b4a_entries(&ac)
        );

        let g = b4a_fixture();
        std::fs::write(settings_path(&g.project), B4A_WINNER_BYTES).unwrap();
        save_workgroup_groups_in(&g.project, b4a_saved(), None).expect("Ok(new)");
        assert!(
            !legacy_settings_path(&g.project).exists(),
            "a second file was created"
        );
    }

    fn b4a_stale_complete(cfg: &Path, project: &Path) {
        let ac_root = b4a_ac_root(project);
        let key = b4a_key(project);
        naming_migration::update_journal(Some(cfg), |j| {
            j.record_step(
                &key,
                naming_migration::StepRecord::new(
                    &ac_root,
                    "project-settings.json",
                    "settings.50.personal.no-git.json",
                    naming_migration::StepState::Renamed,
                ),
            );
            j.set_status(&key, naming_migration::ScopeStatus::Complete);
        })
        .expect("seed the journal");
    }

    #[test]
    fn a_complete_scope_whose_old_name_is_back_is_re_run() {
        let f = b4a_fixture();
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        b4a_stale_complete(&f.cfg, &f.project);

        let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("re-run");

        assert_eq!(
            b4a_names(&loaded),
            ["Legacy"],
            "the stale Complete was trusted"
        );
        assert_eq!(
            std::fs::read(settings_path(&f.project)).unwrap(),
            B4A_LEGACY_BYTES.as_bytes()
        );
        assert!(!legacy_settings_path(&f.project).exists());

        let g = b4a_fixture();
        std::fs::write(legacy_settings_path(&g.project), B4A_LEGACY_BYTES).unwrap();
        std::fs::write(settings_path(&g.project), B4A_WINNER_BYTES).unwrap();
        b4a_stale_complete(&g.cfg, &g.project);
        let loaded = load_workgroup_groups_in(&g.project, Some(&g.cfg)).expect("Ok(new)");
        assert_eq!(b4a_names(&loaded), ["Winner"]);
        assert_eq!(
            std::fs::read(settings_path(&g.project)).unwrap(),
            B4A_WINNER_BYTES.as_bytes()
        );
        assert_eq!(
            std::fs::read(g.project.join(".ac").join(B4A_SET_ASIDE)).unwrap(),
            B4A_LEGACY_BYTES.as_bytes()
        );
    }

    // -- cross-process rows, E7d and E13 --------------------------------------

    const B4A_CHILD_FQN: &str = "config::project_settings::tests::b4a_child_process_entry";
    const B4A_CHILD_ROLE: &str = "AC_2717_CHILD_ROLE";
    const B4A_CHILD_PROJECT: &str = "AC_2717_CHILD_PROJECT";
    const B4A_CHILD_CFG: &str = "AC_2717_CHILD_CFG";
    const B4A_CHILD_RENDEZVOUS: &str = "AC_2717_CHILD_RENDEZVOUS";
    const B4A_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

    fn b4a_spawn(
        role: &str,
        project: &Path,
        cfg: &Path,
        extra: &[(&str, &Path)],
    ) -> std::process::Child {
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("current test exe"));
        command
            .args(["--exact", B4A_CHILD_FQN, "--nocapture", "--test-threads=1"])
            .env(B4A_CHILD_ROLE, role)
            .env(B4A_CHILD_PROJECT, project)
            .env(B4A_CHILD_CFG, cfg)
            .env_remove(naming_migration::pause::PAUSE_DIR_ENV)
            .env_remove(naming_migration::pause::PAUSE_STAGE_ENV)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (key, value) in extra {
            command.env(key, value);
        }
        command.spawn().expect("spawn child")
    }

    fn b4a_wait_file(path: &Path) {
        let started = std::time::Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < B4A_WAIT,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn b4a_wait_child(child: std::process::Child) {
        let output = child.wait_with_output().expect("child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "child failed or ran nothing: {}\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Not a test of its own: the body a spawned child runs, inert otherwise.
    #[test]
    fn b4a_child_process_entry() {
        let Some(role) = std::env::var_os(B4A_CHILD_ROLE) else {
            return;
        };
        let project = PathBuf::from(std::env::var_os(B4A_CHILD_PROJECT).expect("project"));
        let cfg = PathBuf::from(std::env::var_os(B4A_CHILD_CFG).expect("cfg"));
        match role.to_str() {
            Some("migrate") => {
                load_workgroup_groups_in(&project, Some(&cfg)).expect("child migrates");
            }
            Some("timeout_then_save") => {
                let rendezvous =
                    PathBuf::from(std::env::var_os(B4A_CHILD_RENDEZVOUS).expect("rendezvous"));
                let first = save_workgroup_groups_in(&project, b4a_saved(), Some(&cfg));
                assert!(first.is_err(), "the first save must time out: {first:?}");
                std::fs::write(rendezvous.join("timed-out"), b"").unwrap();
                b4a_wait_file(&rendezvous.join("go"));
                let _ = save_workgroup_groups_in(&project, b4a_saved(), Some(&cfg));
            }
            other => panic!("unknown child role {other:?}"),
        }
    }

    #[test]
    fn a_timed_out_caller_never_writes_the_old_name_after_the_winner_finishes() {
        let f = b4a_fixture();
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        let rv_a = f.cfg.join("rv-a");
        let rv_b = f.cfg.join("rv-b");
        std::fs::create_dir_all(&rv_a).unwrap();
        std::fs::create_dir_all(&rv_b).unwrap();
        let stage = std::path::PathBuf::from("after_rename");

        let winner = b4a_spawn(
            "migrate",
            &f.project,
            &f.cfg,
            &[
                (naming_migration::pause::PAUSE_DIR_ENV, &rv_a),
                (naming_migration::pause::PAUSE_STAGE_ENV, &stage),
            ],
        );
        b4a_wait_file(&rv_a.join(naming_migration::pause::READY_FILE));
        let loser = b4a_spawn(
            "timeout_then_save",
            &f.project,
            &f.cfg,
            &[(B4A_CHILD_RENDEZVOUS, &rv_b)],
        );
        b4a_wait_file(&rv_b.join("timed-out"));
        std::fs::write(rv_a.join(naming_migration::pause::RELEASE_FILE), b"go").unwrap();
        b4a_wait_child(winner);
        std::fs::write(rv_b.join("go"), b"").unwrap();
        b4a_wait_child(loser);

        assert!(
            !legacy_settings_path(&f.project).exists(),
            "the old name was written after the handover"
        );
        let disk: Value =
            serde_json::from_str(&std::fs::read_to_string(settings_path(&f.project)).unwrap())
                .unwrap();
        let name = disk["groups"][0]["name"].as_str().unwrap_or_default();
        assert!(
            name == "Saved" || name == "Legacy",
            "unexpected content: {disk}"
        );
    }

    #[test]
    fn two_project_scopes_keep_both_record_sets() {
        let f = b4a_fixture();
        let other = f.cfg.parent().unwrap().join("other");
        std::fs::create_dir_all(other.join(".ac")).unwrap();
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        std::fs::write(legacy_settings_path(&other), B4A_WINNER_BYTES).unwrap();
        let rv = f.cfg.join("rv");
        std::fs::create_dir_all(&rv).unwrap();
        let stage = std::path::PathBuf::from("after_rename");

        let first = b4a_spawn(
            "migrate",
            &f.project,
            &f.cfg,
            &[
                (naming_migration::pause::PAUSE_DIR_ENV, &rv),
                (naming_migration::pause::PAUSE_STAGE_ENV, &stage),
            ],
        );
        b4a_wait_file(&rv.join(naming_migration::pause::READY_FILE));
        // While the first holds its own data locks, the second commits.
        b4a_wait_child(b4a_spawn("migrate", &other, &f.cfg, &[]));
        std::fs::write(rv.join(naming_migration::pause::RELEASE_FILE), b"go").unwrap();
        b4a_wait_child(first);

        for (project, bytes) in [(&f.project, B4A_LEGACY_BYTES), (&other, B4A_WINNER_BYTES)] {
            let record = b4a_record(&f.cfg, project).expect("both scopes recorded");
            assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
            assert!(
                record
                    .steps
                    .iter()
                    .any(|s| s.state == naming_migration::StepState::Renamed),
                "{}: {:?}",
                project.display(),
                record.steps
            );
            assert_eq!(
                std::fs::read(settings_path(project)).unwrap(),
                bytes.as_bytes()
            );
        }
    }

    /// E9 (r12): key independence and commit non-interference only. The foreign
    /// key's shape is copied from `catalog_scope_key` in
    /// `config/coding_agents_catalog.rs`; F1's own deferral stays B2's evidence.
    #[test]
    fn a_foreign_scope_record_survives_this_scope_commit() {
        let f = b4a_fixture();
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        let catalog = f.project.join(".ac").join("coding-agents");
        std::fs::create_dir_all(&catalog).unwrap();
        let catalog = std::fs::canonicalize(&catalog).unwrap();
        let foreign_key = format!("catalog:{}", catalog.display());
        naming_migration::update_journal(Some(&f.cfg), |j| {
            j.note(
                &foreign_key,
                "deferred: an interrupted #1968 migration is blocked",
            );
            j.set_status(&foreign_key, naming_migration::ScopeStatus::Deferred);
        })
        .expect("seed the foreign record");
        let foreign_before = naming_migration::read_journal(Some(&f.cfg))
            .unwrap()
            .unwrap()
            .scope(&foreign_key)
            .cloned()
            .expect("foreign record");

        load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("this scope runs");

        let own_key = b4a_key(&f.project);
        assert_ne!(own_key, foreign_key, "the two scope keys collide");
        let journal = naming_migration::read_journal(Some(&f.cfg))
            .expect("the journal still parses through the engine's reader")
            .expect("journal present");
        let foreign_after = journal
            .scope(&foreign_key)
            .cloned()
            .expect("foreign record kept");
        assert_eq!(
            serde_json::to_value(&foreign_after).unwrap(),
            serde_json::to_value(&foreign_before).unwrap(),
            "this scope's commit changed the foreign record"
        );
        assert_eq!(
            foreign_after.status,
            naming_migration::ScopeStatus::Deferred
        );
        assert_eq!(
            journal.scope(&own_key).expect("own record").status,
            naming_migration::ScopeStatus::Complete
        );
    }

    /// The one `git` site of these tests (E7, E17).
    fn b4a_git(cwd: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git must execute")
    }

    /// #2717 (B4a) 4.4 1b: each row pattern is composed from the registry, and
    /// is pinned against the constants, never against a typed-out name.
    #[test]
    fn the_ignore_rows_are_composed_from_the_registry() {
        use crate::config::instance_artifacts::{PROJECT_SETTINGS_TARGET_NAME, SET_ASIDE_GLOB};
        let [settings, lock, set_aside] = naming_migration::project_settings_ignore_rows();
        assert_eq!(settings.0, format!("/{PROJECT_SETTINGS_TARGET_NAME}"));
        assert_eq!(lock.0, format!("/.{PROJECT_SETTINGS_TARGET_NAME}.lock"));
        assert_eq!(set_aside.0, format!("/{SET_ASIDE_GLOB}"));
    }

    const B4A_ROWS: [(&str, &str); 3] = [
        (
            "/settings.50.personal.no-git.json",
            "# AgentsCommander: exclude generated project-local settings.",
        ),
        (
            "/.settings.50.personal.no-git.json.lock",
            "# AgentsCommander: exclude the project settings write-lock sidecar.",
        ),
        (
            "/*.deprecated-*.no-git",
            "# AgentsCommander: exclude project files the naming migration set aside.",
        ),
    ];

    fn b4a_gitignore(project: &Path) -> String {
        std::fs::read_to_string(project.join(".ac").join(".gitignore")).expect("read .gitignore")
    }

    fn b4a_assert_rows_once(content: &str, label: &str) {
        let lines: Vec<&str> = content.lines().collect();
        for (pattern, comment) in B4A_ROWS {
            let at: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == pattern).collect();
            assert_eq!(at.len(), 1, "{label}: {pattern} is not there exactly once");
            assert!(
                at[0] > 0 && lines[at[0] - 1] == comment,
                "{label}: {pattern} lacks its comment line directly above"
            );
        }
    }

    #[test]
    fn the_scope_ignores_the_new_names_before_it_renames_anything() {
        // The pre-upgrade shape, a project that is never registered.
        let f = b4a_fixture();
        let ac = f.project.join(".ac");
        std::fs::write(
            ac.join(".gitignore"),
            "# AgentsCommander: exclude generated project-local settings.\n/project-settings.json\n",
        )
        .unwrap();
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("load");
        save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg)).expect("save");
        let content = b4a_gitignore(&f.project);
        b4a_assert_rows_once(&content, "base");
        assert!(
            content.lines().any(|l| l == "/project-settings.json"),
            "the sweep is append-only; the old row stays until the writer retires it"
        );
        assert!(b4a_git(&f.project, &["init", "--quiet"]).status.success());
        std::fs::write(ac.join(B4A_SET_ASIDE), b"").unwrap();
        for (relative, rule) in [
            (".ac/settings.50.personal.no-git.json", B4A_ROWS[0].0),
            (".ac/.settings.50.personal.no-git.json.lock", B4A_ROWS[1].0),
            (
                ".ac/project-settings.json.deprecated-1.no-git",
                B4A_ROWS[2].0,
            ),
        ] {
            let output = b4a_git(
                &f.project,
                &["check-ignore", "-v", "--no-index", "--", relative],
            );
            assert!(output.status.success(), "{relative} is not ignored");
            let stdout = String::from_utf8(output.stdout).unwrap();
            let (source, _) = stdout.trim_end().split_once('\t').expect("verbose output");
            assert!(
                source.ends_with(&format!(":{rule}")),
                "{relative}: {source}"
            );
        }

        // A failed sweep refuses and renames nothing.
        let g = b4a_fixture();
        std::fs::create_dir(g.project.join(".ac").join(".gitignore")).unwrap();
        std::fs::write(legacy_settings_path(&g.project), B4A_LEGACY_BYTES).unwrap();
        assert!(load_workgroup_groups_in(&g.project, Some(&g.cfg)).is_err());
        assert_eq!(
            std::fs::read(legacy_settings_path(&g.project)).unwrap(),
            B4A_LEGACY_BYTES.as_bytes(),
            "a failed sweep must leave the data at the old name"
        );
        assert!(!settings_path(&g.project).exists());

        // The decoy: a leading-space line is not the rule.
        let h = b4a_fixture();
        std::fs::write(
            h.project.join(".ac").join(".gitignore"),
            " /settings.50.personal.no-git.json\n",
        )
        .unwrap();
        std::fs::write(legacy_settings_path(&h.project), B4A_LEGACY_BYTES).unwrap();
        load_workgroup_groups_in(&h.project, Some(&h.cfg)).expect("load");
        b4a_assert_rows_once(&b4a_gitignore(&h.project), "decoy");

        // Both orders of the sweep and the registration writer: every AC row
        // once, and a user line seeded before either run survives.
        for sweep_first in [true, false] {
            let k = b4a_fixture();
            let ac = k.project.join(".ac");
            std::fs::write(ac.join(".gitignore"), "user-line-2717\n").unwrap();
            std::fs::write(legacy_settings_path(&k.project), B4A_LEGACY_BYTES).unwrap();
            let writer = || {
                crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[])
                    .expect("writer")
            };
            if sweep_first {
                load_workgroup_groups_in(&k.project, Some(&k.cfg)).expect("sweep");
                writer();
            } else {
                writer();
                load_workgroup_groups_in(&k.project, Some(&k.cfg)).expect("sweep");
            }
            let content = b4a_gitignore(&k.project);
            let label = if sweep_first {
                "sweep, writer"
            } else {
                "writer, sweep"
            };
            b4a_assert_rows_once(&content, label);
            assert!(content.lines().any(|l| l == "user-line-2717"), "{label}");
            let rules: Vec<&str> = content
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .collect();
            let unique: std::collections::BTreeSet<&str> = rules.iter().copied().collect();
            assert_eq!(unique.len(), rules.len(), "{label}: a row appears twice");
        }

        // Interleaved: the registration writer and a user edit land between the
        // sweep's read and its append. Nothing is lost; a 4.4 row may repeat
        // once; no other rule repeats.
        let expected_rules: Vec<String> = {
            let fresh = b4a_fixture();
            let fresh_ac = fresh.project.join(".ac");
            crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&fresh_ac, &[])
                .expect("writer");
            b4a_gitignore(&fresh.project)
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect()
        };
        let m = b4a_fixture();
        let ac = m.project.join(".ac");
        let ac_root = b4a_ac_root(&m.project);
        std::fs::write(ac.join(".gitignore"), "user-line-2717\n").unwrap();
        std::fs::write(legacy_settings_path(&m.project), B4A_LEGACY_BYTES).unwrap();
        let armed = naming_migration::pause::arm("before_gitignore_append", &ac_root);
        let (project, cfg) = (m.project.clone(), m.cfg.clone());
        let sweep = std::thread::spawn(move || load_workgroup_groups_in(&project, Some(&cfg)));
        armed
            .reached
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the sweep reached its append");
        crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[])
            .expect("writer in the window");
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(ac.join(".gitignore"))
                .unwrap();
            file.write_all(b"user-line-late\n").unwrap();
        }
        armed.release.send(()).unwrap();
        sweep.join().unwrap().expect("the sweep completes");
        let content = b4a_gitignore(&m.project);
        let count = |rule: &str| content.lines().filter(|l| *l == rule).count();
        for user in ["user-line-2717", "user-line-late"] {
            assert_eq!(count(user), 1, "interleaved: the user line {user} was lost");
        }
        for rule in &expected_rules {
            let n = count(rule);
            let is_4_4 = B4A_ROWS.iter().any(|(pattern, _)| pattern == rule);
            assert!(n >= 1, "interleaved: the writer's rule {rule} was lost");
            assert!(
                n <= if is_4_4 { 2 } else { 1 },
                "interleaved: {rule} appears {n} times"
            );
        }
    }
}
