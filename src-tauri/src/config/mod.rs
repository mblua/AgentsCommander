pub mod agent_command;
pub mod agent_config;
pub mod agent_creation;
pub mod claude_settings;
pub mod daemon_pid;
pub mod profile;
pub mod projects;
pub mod replica_identity;
pub mod root_agent;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub mod teams;
pub mod workspace;

use std::path::PathBuf;
use std::sync::OnceLock;

fn profile_stem_from_exe_stem(stem: &str) -> &str {
    stem.strip_suffix("-cli").unwrap_or(stem)
}

fn session_local_dir_from_env() -> Option<PathBuf> {
    let raw = std::env::var(crate::pty::credentials::ENV_AGENTSCOMMANDER_LOCAL_DIR).ok()?;
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

/// Returns the local agent directory name derived from the current binary name.
/// E.g., "agentscommander-stage.exe" → ".agentscommander-stage"
/// E.g., "agentscommander.exe" → ".agentscommander"
pub fn agent_local_dir_name() -> String {
    if let Some(name) = session_local_dir_from_env()
        .and_then(|p| p.file_name().map(|name| name.to_string_lossy().to_string()))
    {
        return name;
    }

    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_stem()
                .map(|s| profile_stem_from_exe_stem(&s.to_string_lossy()).to_string())
        })
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
        if let Some(path) = session_local_dir_from_env() {
            return Some(path);
        }

        // Primary: portable config next to the binary
        if let Ok(exe_path) = std::env::current_exe() {
            match (exe_path.parent(), exe_path.file_stem()) {
                (Some(parent), Some(stem)) => {
                    let stem = stem.to_string_lossy();
                    return Some(parent.join(format!(".{}", profile_stem_from_exe_stem(&stem))));
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

#[cfg(test)]
mod tests {
    use super::profile_stem_from_exe_stem;

    #[test]
    fn profile_stem_strips_cli_sidecar_suffix() {
        assert_eq!(
            profile_stem_from_exe_stem("agentscommander_personal-cli"),
            "agentscommander_personal"
        );
    }

    #[test]
    fn profile_stem_leaves_gui_stem_unchanged() {
        assert_eq!(
            profile_stem_from_exe_stem("agentscommander_personal"),
            "agentscommander_personal"
        );
    }
}
