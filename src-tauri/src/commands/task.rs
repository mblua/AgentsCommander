//! Tauri commands wrapping `cli::task_ops::perform` for the TASK.md panel
//! action buttons (issue #162).
//!
//! Trust model: these commands run inside the GUI process under the
//! user's authority, same model as `rename_session`, `destroy_session`,
//! etc. No coordinator gate (the CLI verbs in `cli/task_*.rs` retain
//! their gate; this file does not call into them).

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::cli::task_ops::{self, TaskOp};
use crate::config::ac_root::is_ac_root_name; // gate 5 (path-based clean, #545)
use crate::config::settings::SettingsState;
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
    pub task_title: Option<String>,
    pub status: Option<String>,
    pub revision: String,
    pub tail_incomplete: bool,
}

/// Resolve the workgroup root for a session id, returning a user-facing
/// error string suitable for direct propagation through the Tauri command
/// `Result<_, String>` boundary.
async fn resolve_wg_root(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    let uuid = Uuid::parse_str(session_id).map_err(|e| format!("invalid session id: {}", e))?;
    let mgr = session_mgr.read().await.clone();
    let cwd = mgr
        .get_session(uuid)
        .await
        .map(|s| s.working_directory.clone())
        .ok_or_else(|| format!("session {} not found", session_id))?;
    drop(mgr);

    let task_path = find_workgroup_task_path_for_cwd(&cwd).ok_or_else(|| {
        format!(
            "session {} is not under a `room-*` or legacy `wg-*` Room directory",
            session_id
        )
    })?;
    let wg_root = task_path
        .parent()
        .ok_or_else(|| "room TASK.md path has no parent".to_string())?
        .to_path_buf();
    Ok(wg_root)
}

/// Validate a raw `wg-*` root path supplied by the frontend for the path-based
/// clean (#545). The session-based commands derive the root from a real session
/// cwd; this one trusts a caller-supplied string, so it must prove the path is a
/// real workgroup directory under a configured project root before any write, so
/// it cannot be coerced into writing a TASK.md anywhere on disk.
///
/// Returns the ORIGINAL (validated) path as a `PathBuf`, NOT the canonical form;
/// the canonical form is used ONLY for the five safety gates. The write/emit must
/// use the original arg because `std::fs::canonicalize` on Windows expands 8.3
/// short names (`PROGRA~1` -> `Program Files`) and resolves symlinks/junctions to
/// their real target, while discovery builds the stored `wg.path` from the raw
/// project path joined with on-disk names, with no canonicalization. So a
/// canonical emit key could diverge from the stored `wg.path` and the sidebar row
/// would silently fail to update. (This is NOT about the `\\?\` prefix, which is
/// already stripped both by backend `strip_unc` and by the frontend path
/// normalizer.) Do not "fix" this by switching the write/emit to canonical.
fn validate_wg_root(
    workgroup_root: &str,
    project_paths: &[String],
) -> Result<std::path::PathBuf, String> {
    // 1. Canonicalize: proves existence and resolves `..` + symlinks to a real
    //    absolute path. Mirrors open_in_explorer (window.rs). A traversal or a
    //    symlink that escapes is normalized here and then caught by gate 4.
    let canonical = std::fs::canonicalize(workgroup_root).map_err(|_| {
        format!(
            "room root does not exist or is inaccessible: {}",
            workgroup_root
        )
    })?;

    // 2. Must be a directory.
    if !canonical.is_dir() {
        return Err(format!("room root is not a directory: {}", workgroup_root));
    }

    // 3. Leaf name must start with `wg-`. Matches the discovery invariant and the
    //    session command's `wg-*` ancestor constraint. Blocks writing a TASK.md
    //    into a non-workgroup directory.
    let is_wg = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(crate::config::entity_prefix::has_entity_prefix);
    if !is_wg {
        return Err(format!(
            "room root is not a `room-*` or legacy `wg-*` Room directory: {}",
            workgroup_root
        ));
    }

    // 4. Must be a component-wise descendant of a configured project root. Fail
    //    closed: if no project path canonicalizes to an ancestor, reject.
    //    `Path::starts_with` is COMPONENT-wise (not a string prefix), and both
    //    sides are canonicalized (OS-consistent casing on Windows), so `C:\proj`
    //    does not match `C:\project-x` and separator/prefix mismatches cannot
    //    sneak through. Un-canonicalizable project entries are skipped, not fatal.
    let under_known_root = project_paths.iter().any(|base| {
        std::fs::canonicalize(base)
            .map(|canon_base| canonical.starts_with(&canon_base))
            .unwrap_or(false)
    });
    if !under_known_root {
        return Err(format!(
            "room root is not under a configured project path: {}",
            workgroup_root
        ));
    }

    // 5. The wg-* dir must be a DIRECT child of an `.ac` workspace dir, i.e. a
    //    genuine AC workgroup root, not merely a dir whose leaf starts with `wg-`.
    //    Discovery only ever surfaces workgroups as direct children of `<base>/.ac`,
    //    so this never rejects a real wg.path the frontend can send, and it blocks
    //    creating/clobbering a TASK.md (perform creates one when missing) in an
    //    arbitrary `wg-*`-named dir under a project root. `is_ac_root_name`
    //    is case-insensitive, matching `.ac`/`.AC` on Windows.
    let parent_is_ac_root = canonical
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .is_some_and(is_ac_root_name);
    if !parent_is_ac_root {
        return Err(format!(
            "room root is not a direct child of an .ac workspace dir: {}",
            workgroup_root
        ));
    }

    Ok(std::path::PathBuf::from(workgroup_root))
}

fn strip_unc(p: &Path) -> String {
    let raw = p.to_string_lossy();
    if cfg!(windows) {
        if let Some(rest) = raw
            .strip_prefix(r"\\?\UNC\")
            .or_else(|| raw.strip_prefix(r"\??\UNC\"))
        {
            return format!(r"\\{}", rest);
        }
        if let Some(rest) = raw
            .strip_prefix(r"\\?\")
            .or_else(|| raw.strip_prefix(r"\??\"))
        {
            return rest.to_string();
        }
    }
    raw.into_owned()
}

fn committed_task_error(stage: &str, error: impl std::fmt::Display) -> String {
    log::warn!("[task] mutation already committed; {stage} failed: {error}");
    format!("task mutation already committed; {stage} failed: {error}; do not repeat Clean")
}

fn task_result_from_snapshot(snapshot: task_ops::TaskSnapshot) -> TaskUpdateResult {
    TaskUpdateResult {
        workgroup_root: strip_unc(Path::new(&snapshot.workgroup_root)),
        task: snapshot
            .task
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
        task_title: snapshot.task_title,
        status: snapshot.status,
        revision: snapshot.revision,
        tail_incomplete: snapshot.tail_incomplete,
    }
}

fn emit_task_updated(app: &AppHandle, result: &TaskUpdateResult) -> Result<(), String> {
    app.emit(
        "workgroup_task_updated",
        serde_json::json!({
            "workgroupRoot": result.workgroup_root,
            "source": "manual",
            "task": result.task,
            "taskTitle": result.task_title,
            "status": result.status,
            "revision": result.revision,
            "tailIncomplete": result.tail_incomplete,
        }),
    )
    .map_err(|error| error.to_string())
}

async fn read_committed_task_snapshot(
    root: std::path::PathBuf,
) -> Result<TaskUpdateResult, String> {
    let snapshot = tokio::task::spawn_blocking(move || task_ops::read_snapshot(&root))
        .await
        .map_err(|error| committed_task_error("snapshot worker", error))?
        .map_err(|error| committed_task_error("snapshot read", error))?;
    Ok(task_result_from_snapshot(snapshot))
}

fn deliver_committed_task(
    result: TaskUpdateResult,
    emit: impl FnOnce(&TaskUpdateResult) -> Result<(), String>,
) -> Result<TaskUpdateResult, String> {
    emit(&result).map_err(|error| committed_task_error("event delivery", error))?;
    Ok(result)
}

async fn publish_committed_task(
    app: &AppHandle,
    root: std::path::PathBuf,
) -> Result<TaskUpdateResult, String> {
    let result = read_committed_task_snapshot(root).await?;
    deliver_committed_task(result, |result| emit_task_updated(app, result))
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

/// Shared title validator for both the session-based `task_set_title` and
/// the path-based `task_set_title_at`, so the two commands cannot drift.
/// Pure code motion: same checks, messages, and order as the original
/// inline block in `task_set_title`.
fn validate_user_title(title: &str) -> Result<(), String> {
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
    Ok(())
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
    validate_user_title(&title)?;
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let outcome =
        task_ops::perform(&wg_root, TaskOp::SetUserTitle(title)).map_err(|e| e.to_string())?;
    log::info!(
        "[task] set_title for session {} -> {:?}",
        session_id,
        outcome
    );

    publish_committed_task(&app, wg_root).await
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
    let outcome = task_ops::perform(&wg_root, TaskOp::Clean).map_err(|e| e.to_string())?;
    log::info!("[task] clean for session {} -> {:?}", session_id, outcome);

    publish_committed_task(&app, wg_root).await
}

/// Clear the workgroup TASK.md addressed directly by its `wg-*` root path, for
/// cold workgroups with no live/exited session to resolve from (#545). Mirrors
/// `task_clean` after the root is resolved: same perform + emit + TaskUpdateResult.
/// The raw path is validated by `validate_wg_root` before any write (see the
/// helper docs for the threat model).
#[tauri::command]
pub async fn task_clean_at(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    workgroup_root: String,
) -> Result<TaskUpdateResult, String> {
    // Clone the small Vec and drop the read guard immediately; perform() does
    // blocking IO and must not be held across the settings lock.
    let project_paths = {
        settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?
            .project_paths
            .clone()
    };
    let wg_root = validate_wg_root(&workgroup_root, &project_paths)?;

    let outcome = task_ops::perform(&wg_root, TaskOp::Clean).map_err(|e| e.to_string())?;
    log::info!("[task] clean_at for {} -> {:?}", workgroup_root, outcome);

    publish_committed_task(&app, wg_root).await
}

/// Set the YAML-frontmatter `title:` field of the workgroup TASK.md addressed
/// directly by its `wg-*` root path, for cold workgroups with no live/exited
/// session to resolve from (#1536). Mirrors `task_set_title` after validation:
/// same perform + emit + TaskUpdateResult. The raw path is validated by
/// `validate_wg_root` before any write (see the helper docs for the threat
/// model).
#[tauri::command]
pub async fn task_set_title_at(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    workgroup_root: String,
    title: String,
) -> Result<TaskUpdateResult, String> {
    validate_user_title(&title)?;
    // Clone the small Vec and drop the read guard immediately; perform() does
    // blocking IO and must not be held across the settings lock.
    let project_paths = {
        settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?
            .project_paths
            .clone()
    };
    let wg_root = validate_wg_root(&workgroup_root, &project_paths)?;

    let outcome =
        task_ops::perform(&wg_root, TaskOp::SetUserTitle(title)).map_err(|e| e.to_string())?;
    log::info!(
        "[task] set_title_at for {} -> {:?}",
        workgroup_root,
        outcome
    );

    publish_committed_task(&app, wg_root).await
}

#[cfg(test)]
mod tests;
