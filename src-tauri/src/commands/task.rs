//! Tauri commands wrapping `cli::task_ops::perform` for the BRIEF panel
//! action buttons (issue #162).
//!
//! Trust model: these commands run inside the GUI process under the
//! user's authority — same model as `rename_session`, `destroy_session`,
//! etc. No coordinator gate (the CLI verbs in `cli/brief_*.rs` retain
//! their gate; this file does not call into them).

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::cli::task_ops::{self, TaskOp};
use crate::session::manager::SessionManager;
use crate::session::session::find_workgroup_task_path_for_cwd;

/// Payload returned to the frontend after a successful task mutation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateResult {
    /// Absolute path of the workgroup root the TASK.md belongs to.
    /// Stripped of the Windows `\\?\` extended-length prefix when present.
    pub workgroup_root: String,
    /// Trimmed TASK.md content as displayed by the panel. `None` when the
    /// file is empty or missing post-edit (defensive — should not happen
    /// after a successful Wrote, but possible on race-deletion).
    pub task: Option<String>,
}

/// Resolve the workgroup root for a session id, returning a user-facing
/// error string suitable for direct propagation through the Tauri command
/// `Result<_, String>` boundary.
async fn resolve_wg_root(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    let uuid = Uuid::parse_str(session_id).map_err(|e| format!("invalid session id: {}", e))?;
    let mgr = session_mgr.read().await;
    let cwd = mgr
        .get_session(uuid)
        .await
        .map(|s| s.working_directory.clone())
        .ok_or_else(|| format!("session {} not found", session_id))?;
    drop(mgr);

    let task_path = find_workgroup_task_path_for_cwd(&cwd)
        .ok_or_else(|| format!("session {} is not under a wg-* ancestor", session_id))?;
    let wg_root = task_path
        .parent()
        .ok_or_else(|| "workgroup TASK.md path has no parent".to_string())?
        .to_path_buf();
    Ok(wg_root)
}

fn strip_unc(p: &Path) -> String {
    let raw = p.to_string_lossy().into_owned();
    raw.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(raw)
}

fn emit_task_updated(
    app: &AppHandle,
    wg_root: &Path,
    task: &Option<String>,
    task_title: &Option<String>,
) {
    let _ = app.emit(
        "workgroup_task_updated",
        serde_json::json!({
            "workgroupRoot": strip_unc(wg_root),
            "source": "manual",
            "task": task.clone(),
            "taskTitle": task_title.clone(),
        }),
    );
}

/// Read TASK.md once and return both the trimmed full body and the parsed
/// YAML `title:` value. Returning both from one read avoids torn results when
/// an external writer races us between two reads. Caller emits both fields so
/// the sidebar can update its title without waiting for the next 15s poll.
#[allow(dead_code)]
fn read_task_fields_at(wg_root: &Path) -> (Option<String>, Option<String>) {
    let Ok(content) = std::fs::read_to_string(wg_root.join("TASK.md")) else {
        return (None, None);
    };
    let task_title = crate::commands::entity_creation::parse_task_title(&content);
    let trimmed = content.trim();
    let task = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    (task, task_title)
}

/// Read the current YAML-frontmatter `title:` value of the workgroup
/// TASK.md for the given session. Returns `None` when there is no
/// frontmatter or no `title:` line.
#[tauri::command]
pub async fn task_get_title(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<Option<String>, String> {
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let task_path = wg_root.join("TASK.md");
    let content = match std::fs::read_to_string(&task_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read TASK.md: {}", e)),
    };
    let parsed = task_ops::parse_task(&content);
    Ok(task_ops::title_value_of(&parsed))
}

/// Set the YAML-frontmatter `title:` field of the workgroup TASK.md for
/// the given session. Returns the new (post-edit) trimmed TASK.md
/// content for direct local refresh, AND emits `workgroup_task_updated`
/// for sibling sessions/windows.
#[tauri::command]
pub async fn task_set_title(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
    title: String,
) -> Result<TaskUpdateResult, String> {
    if title.trim().is_empty() {
        return Err("title cannot be empty".to_string());
    }
    if title.chars().any(|c| c.is_control() && c != '\t') {
        return Err("title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
            .to_string());
    }
    // Round 2 (Grinch LOW-1): cap at 256 chars (typical YAML scalar
    // convention). Prevents a 1 MB pasted blob from becoming the title
    // and breaking panel layout / file ergonomics. Counts Unicode
    // scalars, not bytes — a 256-emoji title is allowed and renders
    // sensibly.
    if title.chars().count() > 256 {
        return Err("title is too long (max 256 characters)".to_string());
    }
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let outcome =
        task_ops::perform(&wg_root, TaskOp::SetTitle(title)).map_err(|e| e.to_string())?;
    log::info!(
        "[task] set_title for session {} -> {:?}",
        session_id,
        outcome
    );
    
    let (content, task_title) = match &outcome {
        task_ops::EditOutcome::Wrote { content, title, .. } => (content.clone(), title.clone()),
        task_ops::EditOutcome::NoOp { content, title } => (content.clone(), title.clone()),
    };
    let trimmed = content.trim();
    let task = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task, &task_title);
    Ok(result)
}


/// Clear the workgroup TASK.md for the given session to the canonical Clean state.
/// Returns the new (post-edit) trimmed TASK.md content for direct local refresh,
/// AND emits `workgroup_task_updated` for sibling sessions/windows.
#[tauri::command]
pub async fn task_clean(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<TaskUpdateResult, String> {
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let outcome =
        task_ops::perform(&wg_root, TaskOp::Clean).map_err(|e| e.to_string())?;
    log::info!(
        "[task] clean for session {} -> {:?}",
        session_id,
        outcome
    );
    
    let (content, task_title) = match &outcome {
        task_ops::EditOutcome::Wrote { content, title, .. } => (content.clone(), title.clone()),
        task_ops::EditOutcome::NoOp { content, title } => (content.clone(), title.clone()),
    };
    let trimmed = content.trim();
    let task = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task, &task_title);
    Ok(result)
}

#[cfg(test)]
mod tests {
    //! Covers the helper that issue #301 turns on: read_task_fields_at must
    //! return BOTH the trimmed body and the parsed YAML title from a single
    //! read of TASK.md, so the immediate emit on save/clean carries the title
    //! and the sidebar does not flicker until the next 15s poll.
    use super::read_task_fields_at;
    use crate::cli::task_ops::{perform, TaskOp};

    #[test]
    fn read_task_fields_at_missing_file_returns_none_pair() {
        let dir = tempfile::tempdir().unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(task, None);
        assert_eq!(title, None);
    }

    #[test]
    fn read_task_fields_at_empty_file_returns_none_pair() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TASK.md"), "").unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(task, None);
        assert_eq!(title, None);
    }

    #[test]
    fn read_task_fields_at_parses_frontmatter_title_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\ntitle: 'My Brief'\n---\nbody line\n";
        std::fs::write(dir.path().join("TASK.md"), content).unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(title.as_deref(), Some("My Brief"));
        assert_eq!(task.as_deref(), Some(content.trim()));
    }

    #[test]
    fn read_task_fields_at_body_only_no_frontmatter_returns_body_and_no_title() {
        // grinch LOW (PR #304 review): coverage gap — a file with body content
        // but no YAML frontmatter must return (Some(body), None), not (None, _).
        let dir = tempfile::tempdir().unwrap();
        let content = "Just a body line\nmore body\n";
        std::fs::write(dir.path().join("TASK.md"), content).unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(title, None);
        assert_eq!(task.as_deref(), Some(content.trim()));
    }

    #[test]
    fn set_title_via_task_ops_round_trip_returns_title() {
        // End-to-end mirror of the task_set_title body: perform() then
        // read_task_fields_at(). Validates that the path the Tauri command
        // takes ends up with a non-empty taskTitle in the payload.
        let dir = tempfile::tempdir().unwrap();
        perform(dir.path(), TaskOp::SetTitle("Hello World".to_string()))
            .expect("set title");
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(title.as_deref(), Some("Hello World"));
        assert!(task.is_some(), "task body should not be empty after set-title");
    }

    #[test]
    fn clean_via_task_ops_round_trip_returns_clean_title() {
        // After Clean, the canonical title is "Clean" (see TaskOp::Clean
        // docs). The important thing for issue #301 is the payload carries
        // the title at all (no None → no undefined → no spread-clobber).
        let dir = tempfile::tempdir().unwrap();
        perform(dir.path(), TaskOp::SetTitle("Old Title".to_string()))
            .expect("set initial title");
        perform(dir.path(), TaskOp::Clean).expect("clean");
        let (_task, title) = read_task_fields_at(dir.path());
        assert_eq!(title.as_deref(), Some("Clean"));
    }
}
