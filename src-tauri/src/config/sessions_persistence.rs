use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::session::manager::SessionManager;
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};

/// Minimal session data needed to restore a session on next app start.
/// No UUID, no status - just the "recipe" to re-create it.
///
/// The optional runtime fields (id, status, waiting_for_input, created_at) are
/// populated during live snapshots so the CLI can read session state from the
/// file without requiring an HTTP request. They are ignored on restore.
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
    /// Path to detect git branch from (overrides working_directory for GitWatcher)
    #[serde(default)]
    pub git_branch_source: Option<String>,
    /// Prefix prepended to the detected branch (e.g., "agentscommander" → "agentscommander/main")
    #[serde(default)]
    pub git_branch_prefix: Option<String>,

    // ── Runtime fields (populated during live snapshots, ignored on restore) ──

    /// Session UUID (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Current session status (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    /// Whether the session is waiting for user input (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_input: Option<bool>,
    /// ISO 8601 creation timestamp (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Completion status (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_status: Option<String>,
}

fn sessions_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("sessions.json"))
}

/// Remove duplicate sessions by name AND working_directory.
/// When duplicates share the same key (name or CWD), keep the one with
/// `was_active=true`; if none (or both) are active, keep the last occurrence.
/// Note: callers are expected to filter out temp sessions before calling this.
fn deduplicate(sessions: Vec<PersistedSession>) -> Vec<PersistedSession> {
    let total = sessions.len();
    let mut name_index: HashMap<String, usize> = HashMap::new();
    let mut cwd_index: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<PersistedSession> = Vec::with_capacity(total);

    for session in sessions {
        let norm_cwd = session.working_directory.replace('\\', "/").to_lowercase();

        // Check name-based duplicate
        if let Some(&idx) = name_index.get(&session.name) {
            log::warn!("[sessions] Dropping duplicate session by name '{}'", session.name);
            if !result[idx].was_active || session.was_active {
                // Patch cwd_index if the CWD changed
                let old_cwd = result[idx].working_directory.replace('\\', "/").to_lowercase();
                if old_cwd != norm_cwd {
                    cwd_index.remove(&old_cwd);
                    cwd_index.insert(norm_cwd, idx);
                }
                result[idx] = session;
            }
            continue;
        }

        // Check CWD-based duplicate
        if let Some(&idx) = cwd_index.get(&norm_cwd) {
            log::warn!(
                "[sessions] Dropping duplicate session by CWD '{}' (existing='{}', incoming='{}')",
                session.working_directory, result[idx].name, session.name
            );
            if !result[idx].was_active || session.was_active {
                name_index.remove(&result[idx].name);
                name_index.insert(session.name.clone(), idx);
                result[idx] = session;
            }
            continue;
        }

        // New unique session
        name_index.insert(session.name.clone(), result.len());
        cwd_index.insert(norm_cwd, result.len());
        result.push(session);
    }

    if result.len() < total {
        log::info!("[sessions] Deduplicated: {} → {} sessions", total, result.len());
    }

    result
}

/// Load sessions from disk without deduplication or temp-session filtering.
/// Used by the CLI to read the live snapshot as-is.
pub fn load_sessions_raw() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => return vec![],
    };
    if !path.exists() {
        return vec![];
    }
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => vec![],
    }
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
                // Safety net: filter out [temp] sessions that should never survive a restart
                let temp_count = sessions.iter().filter(|s| s.name.starts_with(TEMP_SESSION_PREFIX)).count();
                let filtered: Vec<PersistedSession> = sessions
                    .into_iter()
                    .filter(|s| {
                        if s.name.starts_with(TEMP_SESSION_PREFIX) {
                            log::warn!("[sessions] Filtering out temp session '{}' from persistence", s.name);
                            false
                        } else {
                            true
                        }
                    })
                    .collect();
                if temp_count > 0 {
                    log::info!("[sessions] Removed {} temp sessions from persistence file", temp_count);
                }
                let deduped = deduplicate(filtered);
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
        .filter(|s| {
            if s.name.starts_with(TEMP_SESSION_PREFIX) {
                log::debug!("[sessions] Excluding temp session '{}' from snapshot", s.name);
                false
            } else {
                true
            }
        })
        .map(|s| PersistedSession {
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: strip_auto_injected_args(&s.shell, &s.shell_args),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
            git_branch_source: s.git_branch_source.clone(),
            git_branch_prefix: s.git_branch_prefix.clone(),
            // Runtime fields for CLI consumption
            id: Some(s.id.clone()),
            status: Some(s.status.clone()),
            waiting_for_input: Some(s.waiting_for_input),
            created_at: Some(s.created_at.clone()),
            completion_status: Some(s.completion_status.clone()),
        })
        .collect();

    deduplicate(all)
}

/// Strip auto-injected flags from Claude agent shell args.
/// Removes `--continue` and `--append-system-prompt-file <path>` which are
/// auto-injected at session creation time (see commands/session.rs).
/// These must not be baked into the saved "recipe" — otherwise they self-perpetuate
/// across app restarts (or session restarts) even when the conditions change.
///
/// Handles two injection modes:
/// - **Direct-exec**: flags are separate args: `["--continue", "--append-system-prompt-file", "/path"]`
/// - **cmd.exe wrapper**: flags are suffixed onto the last arg: `"claude --continue --append-system-prompt-file \"/path\""`
pub(crate) fn strip_auto_injected_args(shell: &str, args: &[String]) -> Vec<String> {
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

    let is_cmd = crate::commands::session::executable_basename(shell) == "cmd";

    if is_cmd {
        // cmd.exe wrapper mode: flags are embedded as suffixes in the last arg string.
        // e.g. "claude --continue --append-system-prompt-file \"/tmp/ctx.md\""
        args.iter()
            .map(|a| {
                let mut s = a.clone();
                // Strip " --continue" suffix
                if let Some(pos) = s.to_lowercase().rfind(" --continue") {
                    // Verify it's at the end or followed by " --append-system-prompt-file"
                    let after = &s[pos + " --continue".len()..];
                    if after.is_empty() || after.to_lowercase().starts_with(" --append-system-prompt-file") {
                        s = format!("{}{}", &s[..pos], after);
                    }
                }
                // Strip " --append-system-prompt-file ..." suffix (with quoted or unquoted path)
                if let Some(pos) = s.to_lowercase().rfind(" --append-system-prompt-file") {
                    s = s[..pos].to_string();
                }
                s
            })
            .collect()
    } else {
        // Direct-exec mode: flags are separate args.
        let mut result = Vec::with_capacity(args.len());
        let mut skip_next = false;
        for a in args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            if a.eq_ignore_ascii_case("--append-system-prompt-file") {
                skip_next = true; // skip the next arg (the file path)
                continue;
            }
            result.push(a.clone());
        }
        result
    }
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
