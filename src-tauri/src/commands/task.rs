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

fn emit_task_updated(app: &AppHandle, wg_root: &Path, task: &Option<String>) {
    let _ = app.emit(
        "workgroup_task_updated",
        serde_json::json!({
            "workgroupRoot": strip_unc(wg_root),
            "task": task.clone(),
        }),
    );
}

fn read_task_at(wg_root: &Path) -> Option<String> {
    std::fs::read_to_string(wg_root.join("TASK.md"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
    let task = read_task_at(&wg_root);
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task);
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
    let task = read_task_at(&wg_root);
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task);
    Ok(result)
}
