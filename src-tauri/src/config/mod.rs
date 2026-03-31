pub mod claude_settings;
pub mod dark_factory;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;

use std::path::PathBuf;

/// Returns the app config directory.
/// Uses `.agentscommander-dev` when running from a debug build (target\debug),
/// `.agentscommander` otherwise (production).
pub fn config_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir_name = if cfg!(debug_assertions) {
        ".agentscommander-dev"
    } else {
        ".agentscommander"
    };
    Some(home.join(dir_name))
}
