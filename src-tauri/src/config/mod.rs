pub mod agent_config;
pub mod claude_settings;
pub mod profile;
pub mod project_settings;
pub mod teams;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;

use std::path::PathBuf;
use std::sync::OnceLock;

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
/// Cached via OnceLock — resolved once at first call.
pub fn config_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        // Primary: portable config next to the binary
        if let Ok(exe_path) = std::env::current_exe() {
            match (exe_path.parent(), exe_path.file_stem()) {
                (Some(parent), Some(stem)) => {
                    return Some(parent.join(format!(".{}", stem.to_string_lossy())));
                }
                _ => {
                    log::warn!(
                        "[config_dir] current_exe() path has no parent or stem: {:?}, falling back to $HOME",
                        exe_path
                    );
                }
            }
        }
        // Fallback: old $HOME-based path
        dirs::home_dir().map(|home| home.join(profile::config_dir_name()))
    })
    .clone()
}
