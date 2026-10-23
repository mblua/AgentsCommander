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
                "left on disk: .project-settings.json.lock (a lock sidecar is never renamed or \
                    deleted)",
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
mod tests;
