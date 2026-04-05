pub mod agent_config;
pub mod claude_settings;
pub mod profile;
pub mod teams;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;

use std::path::PathBuf;

/// Returns the local agent directory name derived from the current binary name.
/// E.g., "agentscommander-stage.exe" → ".agentscommander-stage"
/// E.g., "agentscommander.exe" → ".agentscommander"
pub fn agent_local_dir_name() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());
    format!(".{}", exe)
}

/// Returns the app config directory — portable, next to the binary.
/// Pattern: `<binary_parent_dir>/.<binary_file_stem>/`
/// E.g., `C:\tools\agentscommander_standalone.exe` → `C:\tools\.agentscommander_standalone\`
/// Fallback: `$HOME/<profile::config_dir_name()>` if current_exe() fails.
pub fn config_dir() -> Option<PathBuf> {
    // Primary: portable config next to the binary
    if let Ok(exe_path) = std::env::current_exe() {
        if let (Some(parent), Some(stem)) = (exe_path.parent(), exe_path.file_stem()) {
            return Some(parent.join(format!(".{}", stem.to_string_lossy())));
        }
    }
    // Fallback: old $HOME-based path
    let home = dirs::home_dir()?;
    Some(home.join(profile::config_dir_name()))
}
