use crate::config::project_settings::{self, ProjectSettings};

#[tauri::command]
pub async fn get_project_settings(
    project_path: String,
) -> Result<Option<ProjectSettings>, String> {
    Ok(project_settings::load_project_settings(&project_path))
}

#[tauri::command]
pub async fn update_project_settings(
    project_path: String,
    settings: ProjectSettings,
) -> Result<(), String> {
    project_settings::save_project_settings(&project_path, &settings)
}

#[tauri::command]
pub async fn delete_project_settings(project_path: String) -> Result<(), String> {
    project_settings::delete_project_settings(&project_path)
}
