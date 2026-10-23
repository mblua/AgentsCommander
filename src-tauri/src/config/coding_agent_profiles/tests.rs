#[cfg(windows)]
use super::paths::strip_extended_prefix;
use super::paths::{canonical_existing_dir, same_canonical_path};
use super::selection_state::read_source_null_profile_result;
use super::selection_write::{
    clear_replica_selection_lock_with_publish, set_prepared_source_profile_override,
    validated_replica_dir, write_prepared_source_selection,
};
use super::source_selection::read_source_selection_target;
use super::*;
use crate::config::settings::AppSettings;

use crate::config::settings::{
    AgentConfig, ConfigurationRef, ProfileCellConfig, ProfileSlotConfig,
};

use std::collections::BTreeMap;

use std::path::{Path, PathBuf};

fn settings_with_cells(cells: &[(&str, Vec<&str>)]) -> AppSettings {
    let mut settings = AppSettings::default();
    settings.coding_agent_profiles.profile_slots = BTreeMap::from([
        (
            "A".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        ),
        (
            "B".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        ),
        (
            "C".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        ),
        (
            "D".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        ),
    ]);
    settings.coding_agent_profiles.profiles_by_agent = cells
        .iter()
        .map(|(agent_id, letters)| {
            (
                (*agent_id).to_string(),
                letters
                    .iter()
                    .map(|letter| {
                        (
                            (*letter).to_string(),
                            ProfileCellConfig {
                                id_hash: None,
                                id_hash_debug: None,
                                preflight: None,
                                preflight_timeout_seconds: None,
                                enabled: true,
                                command: format!("codex --{}", letter.to_ascii_lowercase()),
                                env: BTreeMap::new(),
                                notes: String::new(),
                            },
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    settings
}

fn settings_with_project(project: &Path) -> AppSettings {
    let mut settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
    settings.project_paths = vec![project.to_string_lossy().to_string()];
    settings.agents = vec![AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "codex".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Codex".to_string(),
        command: "codex".to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }];
    settings
}

const REPLICA_IDENTITY: &str = "../../_agent_dev-rust";

fn seed_selection_config(tooling_json: &str) -> String {
    format!(r#"{{"identity":"{REPLICA_IDENTITY}","tooling":{tooling_json}}}"#)
}

fn write_config(replica: &Path, bytes: &str) {
    std::fs::write(replica.join("config.json"), bytes).unwrap();
}

fn config_bytes(replica: &Path) -> String {
    std::fs::read_to_string(replica.join("config.json")).unwrap()
}

fn config_value(replica: &Path) -> Value {
    serde_json::from_str(&config_bytes(replica)).unwrap()
}

/// #2816 (C3) - the state file beside `dir`'s `config.json`.
fn state_path(dir: &Path) -> PathBuf {
    dir.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

fn state_value(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(state_path(dir)).unwrap()).unwrap()
}

/// #2816 (C3) - none of `keys` may remain in `dir`'s tracked `config.json`;
/// an absent tracked file holds none.
fn assert_untracked(dir: &Path, keys: &[&str]) {
    let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
        return;
    };
    let tracked: Value = serde_json::from_str(&raw).unwrap();
    for key in keys {
        assert!(
            tracked["tooling"].get(key).is_none(),
            "{key} is still tracked in {}",
            dir.display()
        );
    }
}

fn pair(coding_agent_id: &str, profile: &str) -> ReplicaSelectionPair {
    ReplicaSelectionPair {
        coding_agent_id: coding_agent_id.to_string(),
        requested_profile: profile.to_string(),
    }
}

mod c4_state;
struct SelectionFixture {
    _temp: tempfile::TempDir,
    matrix: PathBuf,
    replica: PathBuf,
    settings: AppSettings,
}

/// A minimal valid replica/matrix pair (`_agent_dev-rust` plus
/// `wg-7-dev-team/__agent_dev-rust`) so the strict read and the settings
/// path validation both accept the replica.
fn selection_fixture() -> SelectionFixture {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(matrix.join("Role.md"), "# Role\n").unwrap();
    let settings = settings_with_project(&project);
    SelectionFixture {
        _temp: temp,
        matrix,
        replica,
        settings,
    }
}

mod resolve;
mod selection_state;
mod selection_write;
mod transition_reference;
