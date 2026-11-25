//! Tauri commands for the "new agent" flow: `pick_folder` and `create_agent_folder`.
//! Holds no state; the folder picker uses `rfd`, and on-disk creation is delegated to
//! `config::agent_creation`.

use std::path::PathBuf;

use crate::config::agent_creation;

/// Opens a native folder picker dialog and returns the selected path.
/// `title` is optional; without it the window keeps the "new agent" title.
#[tauri::command]
pub async fn pick_folder(
    default_path: Option<String>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    let title = title
        .as_deref()
        .unwrap_or("Select parent folder for the new agent");
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);

    if let Some(ref p) = default_path {
        let path = PathBuf::from(p);
        if agent_creation::existing_picker_default(&path) {
            dialog = dialog.set_directory(&path);
        }
    }

    let result = dialog.pick_folder().await;
    Ok(result.map(|h| h.path().to_string_lossy().to_string()))
}

/// Creates an agent folder with a CLAUDE.md inside it.
/// Returns the full path of the created folder.
#[tauri::command]
pub async fn create_agent_folder(
    parent_path: String,
    agent_name: String,
) -> Result<String, String> {
    let created = agent_creation::create_agent_folder_on_disk(&parent_path, &agent_name)?;
    Ok(created.agent_dir.to_string_lossy().to_string())
}
