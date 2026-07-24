use std::path::{Path, PathBuf};

use crate::config::daemon_pid::{self, DaemonState};
use crate::session::session::is_live_session_record;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveSessionBlocker {
    pub name: String,
    pub working_directory: String,
    pub status: String,
    pub stale_state: bool,
}

pub(crate) fn find_live_sessions_under(
    root: &Path,
) -> Result<Vec<LiveSessionBlocker>, crate::errors::StartupError> {
    let root = canonicalize_for_compare(root);
    let daemon_state = daemon_pid::detect_daemon_state()?;
    let stale_state = !matches!(daemon_state, DaemonState::Running { .. });

    Ok(crate::config::sessions_persistence::load_sessions_raw()
        .into_iter()
        .filter(|session| is_live_session_record(session.id.is_some(), session.status.as_ref()))
        .filter_map(|session| {
            let cwd = canonicalize_for_compare(Path::new(&session.working_directory));
            if !path_is_under(&cwd, &root) {
                return None;
            }
            Some(LiveSessionBlocker {
                name: session.name,
                working_directory: session.working_directory,
                status: session
                    .status
                    .map(|s| format!("{:?}", s))
                    .unwrap_or_else(|| "Unknown".to_string()),
                stale_state,
            })
        })
        .collect())
}

pub(crate) fn ensure_no_live_sessions_under(root: &Path) -> Result<(), String> {
    let blockers = find_live_sessions_under(root).map_err(|error| error.to_string())?;
    if blockers.is_empty() {
        return Ok(());
    }
    let stale = blockers.iter().any(|b| b.stale_state);
    let list = blockers
        .iter()
        .map(|b| format!("  - {} at {} ({})", b.name, b.working_directory, b.status))
        .collect::<Vec<_>>()
        .join("\n");
    let stale_note = if stale {
        "\nDaemon state is not confirmed running, so this may be stale session state. Close sessions or clear stale state before retrying."
    } else {
        ""
    };
    Err(format!(
        "Cannot delete while live sessions exist under {}:\n{}{}",
        root.display(),
        list,
        stale_note
    ))
}

fn canonicalize_for_compare(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    PathBuf::from(strip_long_prefix(&canonical.to_string_lossy()))
}

fn strip_long_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(windows)]
fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', r"\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(not(windows))]
fn path_key(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('/').to_string()
}

fn path_is_under(candidate: &Path, root: &Path) -> bool {
    let candidate = path_key(candidate);
    let root = path_key(root);
    let sep = if cfg!(windows) { "\\" } else { "/" };
    candidate == root || candidate.starts_with(&format!("{}{}", root, sep))
}
