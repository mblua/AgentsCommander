use std::path::PathBuf;

use crate::config::agent_creation;

/// Opens a native folder picker dialog and returns the selected path.
#[tauri::command]
pub async fn pick_folder(default_path: Option<String>) -> Result<Option<String>, String> {
    let mut dialog =
        rfd::AsyncFileDialog::new().set_title("Select parent folder for the new agent");

    if let Some(ref p) = default_path {
        let path = PathBuf::from(p);
        if path.exists() {
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

/// Creates or updates .claude/settings.local.json with claudeMdExcludes in the given directory.
/// Works on any directory - both new agent folders and existing repos.
///
/// Issue #120 - also applies the rtk PreToolUse hook based on the global toggle.
/// Acquires `RtkSweepLockState` around the helper sequence (M8) so concurrent
/// sweeps cannot interleave a read-modify-write on the same file.
#[tauri::command]
pub async fn write_claude_settings_local(
    settings: tauri::State<'_, crate::config::settings::SettingsState>,
    sweep_lock: tauri::State<'_, crate::RtkSweepLockState>,
    agent_path: String,
) -> Result<(), String> {
    let dir = PathBuf::from(&agent_path);
    let inject = settings.read().await.inject_rtk_hook;
    let _guard = sweep_lock.lock().await;
    crate::config::claude_settings::ensure_claude_md_excludes(&dir)?;
    if let Err(e) = crate::config::claude_settings::ensure_rtk_pretool_hook(&dir, inject) {
        log::warn!(
            "[agent_creator] Failed to apply rtk hook (enabled={}) to {}: {}",
            inject,
            dir.display(),
            e
        );
    }
    Ok(())
}
