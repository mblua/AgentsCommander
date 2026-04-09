use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::settings::AgentConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<AgentConfig>>,
}

/// Returns the path to `<project>/.ac-new/project-settings.json`.
pub fn project_settings_path(project_path: &str) -> PathBuf {
    Path::new(project_path)
        .join(".ac-new")
        .join("project-settings.json")
}

/// Validates that `project_path` is a directory with an `.ac-new/` subdirectory,
/// then returns the settings file path. Prevents writes to arbitrary locations.
fn validated_settings_path(project_path: &str) -> Result<PathBuf, String> {
    let base = Path::new(project_path);
    if !base.is_dir() {
        return Err(format!(
            "Project path is not a directory: {}",
            project_path
        ));
    }
    let ac_dir = base.join(".ac-new");
    if !ac_dir.is_dir() {
        return Err(format!(
            "Not an AC project (no .ac-new/): {}",
            project_path
        ));
    }
    Ok(ac_dir.join("project-settings.json"))
}

/// Load project settings from `<project>/.ac-new/project-settings.json`.
/// Returns `None` if the file is missing or cannot be parsed.
pub fn load_project_settings(project_path: &str) -> Option<ProjectSettings> {
    let path = validated_settings_path(project_path).ok()?;
    if !path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "Failed to read project-settings.json at {:?}: {}",
                path,
                e
            );
            return None;
        }
    };
    match serde_json::from_str::<ProjectSettings>(&content) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!(
                "Failed to parse project-settings.json at {:?}: {}",
                path,
                e
            );
            None
        }
    }
}

/// Save project settings to `<project>/.ac-new/project-settings.json` (pretty-printed).
pub fn save_project_settings(
    project_path: &str,
    settings: &ProjectSettings,
) -> Result<(), String> {
    let path = validated_settings_path(project_path)?;

    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize project settings: {}", e))?;

    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write project-settings.json: {}", e))?;

    log::info!("Saved project settings to {:?}", path);
    Ok(())
}

/// Delete `<project>/.ac-new/project-settings.json`, reverting to global agents.
/// Idempotent: succeeds even if the file doesn't exist.
pub fn delete_project_settings(project_path: &str) -> Result<(), String> {
    let path = validated_settings_path(project_path)?;

    match std::fs::remove_file(&path) {
        Ok(()) => {
            log::info!("Deleted project settings at {:?}", path);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("Project settings already absent at {:?}", path);
            Ok(())
        }
        Err(e) => Err(format!("Failed to delete project-settings.json: {}", e)),
    }
}
