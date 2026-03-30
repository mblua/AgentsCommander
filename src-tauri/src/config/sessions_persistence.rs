use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// Remove duplicate sessions by name.
/// If multiple entries share the same name, keep the one with `was_active=true`;
/// if none (or both) are active, keep the last occurrence.
fn deduplicate(sessions: Vec<PersistedSession>) -> Vec<PersistedSession> {
    let total = sessions.len();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<PersistedSession> = Vec::with_capacity(total);

    for session in sessions {
        if let Some(&idx) = index.get(&session.name) {
            log::warn!("[sessions] Dropping duplicate session '{}'", session.name);
            // Replace unless existing is active and incoming is not
            if !result[idx].was_active || session.was_active {
                result[idx] = session;
            }
        } else {
            index.insert(session.name.clone(), result.len());
            result.push(session);
        }
    }

    if result.len() < total {
        log::info!("[sessions] Deduplicated: {} → {} sessions", total, result.len());
    }

    result
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
                let deduped = deduplicate(sessions);
                log::info!("Loaded {} persisted sessions from {:?}", deduped.len(), path);
                deduped
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

    // Atomic write: write to .tmp then rename, so a crash mid-write
    // cannot corrupt the existing sessions.json.
    let tmp_path = dir.join("sessions.json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write temp sessions file: {}", e))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename sessions file: {}", e))?;

    log::info!("Saved {} sessions to {:?}", sessions.len(), path);
    Ok(())
}

/// Snapshot current live sessions into the persisted format.
/// Strips auto-injected flags (--continue) so they are re-evaluated on next restore.
pub async fn snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession> {
    let sessions = mgr.list_sessions().await;
    let active_id = mgr.get_active().await.map(|id| id.to_string());

    let all: Vec<PersistedSession> = sessions
        .iter()
        .map(|s| PersistedSession {
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: strip_auto_injected_continue(&s.shell, &s.shell_args),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
        })
        .collect();

    deduplicate(all)
}

/// Strip `--continue` from Claude agent shell args before persisting.
/// This flag is auto-injected at session creation time (see commands/session.rs)
/// and must not be baked into the saved "recipe" — otherwise it self-perpetuates
/// across app restarts even when the conditions for injection no longer apply.
fn strip_auto_injected_continue(shell: &str, args: &[String]) -> Vec<String> {
    let is_claude = std::iter::once(shell)
        .chain(args.iter().map(|s| s.as_str()))
        .any(|t| {
            std::path::Path::new(t)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(t)
                .eq_ignore_ascii_case("claude")
        });

    if !is_claude {
        return args.to_vec();
    }

    // Strip standalone "--continue" args and " --continue" suffix (cmd wrapper path)
    args.iter()
        .filter(|a| !a.eq_ignore_ascii_case("--continue"))
        .map(|a| {
            if a.to_lowercase().ends_with(" --continue") {
                a[..a.len() - " --continue".len()].to_string()
            } else {
                a.clone()
            }
        })
        .collect()
}

/// Persist live sessions merged with entries that failed to restore.
/// Failed entries are appended so they survive for the next startup attempt.
pub async fn persist_merging_failed(
    mgr: &SessionManager,
    failed: &[PersistedSession],
) {
    let mut snapshot = snapshot_sessions(mgr).await;
    snapshot.extend(failed.iter().cloned());
    let snapshot = deduplicate(snapshot);
    if let Err(e) = save_sessions(&snapshot) {
        log::error!("Failed to persist sessions (with merge): {}", e);
    }
}

/// Convenience: snapshot and save in one call. Logs errors but never fails.
pub async fn persist_current_state(mgr: &SessionManager) {
    let snapshot = snapshot_sessions(mgr).await;
    if let Err(e) = save_sessions(&snapshot) {
        log::error!("Failed to persist sessions: {}", e);
    }
}
