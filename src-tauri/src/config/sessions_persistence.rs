use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::session::manager::SessionManager;

/// Minimal session data needed to restore a session on next app start.
/// No UUID, no status - just the "recipe" to re-create it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub working_directory: String,
    /// True for the session that was active when the app closed
    #[serde(default)]
    pub was_active: bool,
}

fn sessions_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("sessions.json"))
}

/// Load persisted sessions from the app config directory (see config_dir()).
/// Returns empty vec on any error (missing file, corrupt JSON, etc.)
pub fn load_sessions() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    if !path.exists() {
        return vec![];
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<Vec<PersistedSession>>(&contents) {
            Ok(sessions) => {
                log::info!("Loaded {} persisted sessions from {:?}", sessions.len(), path);
                sessions
            }
            Err(e) => {
                log::error!("Failed to parse sessions file: {}", e);
                vec![]
            }
        },
        Err(e) => {
            log::error!("Failed to read sessions file: {}", e);
            vec![]
        }
    }
}

/// Save current sessions to the app config directory (see config_dir()).
pub fn save_sessions(sessions: &[PersistedSession]) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join("sessions.json");

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let json = serde_json::to_string_pretty(sessions)
        .map_err(|e| format!("Failed to serialize sessions: {}", e))?;

    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write sessions file: {}", e))?;

    log::info!("Saved {} sessions to {:?}", sessions.len(), path);
    Ok(())
}

/// Snapshot current live sessions into the persisted format.
pub async fn snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession> {
    let sessions = mgr.list_sessions().await;
    let active_id = mgr.get_active().await.map(|id| id.to_string());

    sessions
        .iter()
        .map(|s| PersistedSession {
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: s.shell_args.clone(),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
        })
        .collect()
}

/// Convenience: snapshot and save in one call. Logs errors but never fails.
pub async fn persist_current_state(mgr: &SessionManager) {
    let snapshot = snapshot_sessions(mgr).await;
    if let Err(e) = save_sessions(&snapshot) {
        log::error!("Failed to persist sessions: {}", e);
    }
}
