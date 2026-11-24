//! Tauri commands for Settings > Coding Agents Export/Import.
//! Holds no state; dialogs use `rfd`, disk access is delegated to
//! `filesystem::coding_agents_transfer`.

use crate::filesystem::coding_agents_transfer::{
    read_transfer_file, write_transfer_file, MAX_TRANSFER_BYTES,
};

/// Opens the native Save dialog and writes `content` to the chosen file.
/// Returns the written path, or `None` when the user cancels.
#[tauri::command]
pub async fn coding_agents_export_pick_save(
    content: String,
    default_file_name: String,
) -> Result<Option<String>, String> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Export coding agents")
        .add_filter("JSON", &["json"])
        .set_file_name(&default_file_name)
        .save_file()
        .await;
    let Some(handle) = picked else {
        return Ok(None);
    };
    write_transfer_file(handle.path(), &content)?;
    Ok(Some(handle.path().to_string_lossy().to_string()))
}

/// Opens the native Open dialog and returns the chosen file's UTF-8 text.
/// Returns `None` when the user cancels.
#[tauri::command]
pub async fn coding_agents_import_pick_open() -> Result<Option<String>, String> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Import coding agents")
        .add_filter("JSON", &["json"])
        .pick_file()
        .await;
    let Some(handle) = picked else {
        return Ok(None);
    };
    read_transfer_file(handle.path(), MAX_TRANSFER_BYTES).map(Some)
}
