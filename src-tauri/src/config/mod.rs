pub mod claude_settings;
pub mod dark_factory;
pub mod profile;
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

/// Returns the app config directory based on build profile.
/// DEV: `~/.agentscommander-new-dev`
/// PROD/STAGE: `~/.agentscommander` (shared)
pub fn config_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(profile::config_dir_name()))
}
