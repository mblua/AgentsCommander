use super::replica_read::extract_task_first_line;
use crate::config::settings::SettingsState;
use crate::filesystem::projects as project_fs;
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::State;
use uuid::Uuid;

pub(crate) const BRANCH_POLL_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(crate) struct ReplicaBranchEntry {
    pub(crate) replica_path: String,
    /// (label, absolute repo path) pairs. Order = replica config.json `repos` array order.
    /// Never sort or dedupe — `Vec<SessionRepo>` equality in poll() depends on order.
    pub(crate) repos: Vec<(String, String)>,
    /// Session name format: "wg_name/replica_name"
    pub(crate) session_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiscoveryBranchPayload {
    pub(crate) replica_path: String,
    /// Single-repo shorthand, semantics unchanged: `None` for a multi-repo replica,
    /// so the AcDiscoveryPanel chip keeps hiding the branch there.
    pub(crate) branch: Option<String>,
    /// #943 B2 - per-repo branches, positionally parallel to `repo_paths` below and
    /// therefore to `AcAgentReplica.repo_paths` (`update_replicas_for_project` maps
    /// 1:1 over that vec and never sorts or dedupes it). `None` = branch unknown
    /// (detached HEAD, not a work tree, git missing, detection timed out).
    ///
    /// This is data `poll()` already computed for every replica, multi-repo included,
    /// and then threw away: the single-repo `branch` above was all it kept. Module B2
    /// stops discarding it. Zero new git spawns.
    pub(crate) repo_branches: Vec<Option<String>>,
    /// #943 B2 - the repo paths those branches belong to, byte-identical to the
    /// `AcAgentReplica.repo_paths` strings discovery already sent the frontend.
    ///
    /// Emitted so the consumer can key the merge by path instead of by position. The
    /// positional guarantee holds at emit time, but the consumer stores this payload
    /// and re-reads it against a LATER discovery result: edit a replica's config.json
    /// `repos` (reorder, remove) and for up to one 15s tick the stored branches belong
    /// to the previous repo list. A positional merge would then paint repo A's branch
    /// onto repo B, silently. Matching on the path cannot.
    pub(crate) repo_paths: Vec<String>,
    /// #1028/#1078 - per-repo local-work state, positionally parallel to `repo_paths`
    /// exactly as `repo_branches` is, and keyed by path by the consumer for the same
    /// reason. `Some(true)` means Git emitted a non-ignored worktree/index entry or the
    /// checked-out `HEAD` was not confirmed reachable from the configured cached
    /// `origin/*` upstream. `None` means never successfully detected for that path
    /// (rendered violet, like clean).
    ///
    /// This rides the feed `repo_branches` already established, which is what covers
    /// DORMANT coordinator rows: Gate A runs for every replica whether or not a session
    /// exists, so a dormant row's dirtiness was already being computed and thrown away.
    /// Gate B below cannot cover them (it is inside `if let Some(session_id)`).
    ///
    /// This struct's `PartialEq` gates the emit, so adding the field is also what makes
    /// a dirty flip re-emit with no gate change.
    pub(crate) repo_dirty: Vec<Option<bool>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionGitReposPayload {
    pub(crate) session_id: String,
    pub(crate) repos: Vec<SessionRepo>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StatSentinel {
    pub(crate) len: u64,
    pub(crate) mtime: Option<SystemTime>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TaskSentinels {
    pub(crate) task: Option<StatSentinel>,
    pub(crate) status: Option<StatSentinel>,
    pub(crate) journal: bool,
}

#[derive(Clone)]
pub(crate) struct TaskCacheEntry {
    pub(crate) sentinel: TaskSentinels,
    pub(crate) task: Option<String>,
    pub(crate) task_title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) revision: String,
    pub(crate) tail_incomplete: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskUpdatedPayload {
    pub(crate) workgroup_root: String,
    pub(crate) source: String,
    pub(crate) task: Option<String>,
    // Always serialize taskTitle, even when None, so the consumer receives an
    // explicit signal (null) rather than `undefined`. Matches the manual-emit
    // payload built by `commands::task::emit_task_updated`. Architect sign-off
    // on PR #304: normalize both emitters to explicit-null.
    pub(crate) task_title: Option<String>,
    pub(crate) session_ids: Vec<String>,
    pub(crate) status: Option<String>,
    pub(crate) revision: String,
    pub(crate) tail_incomplete: bool,
}

pub(crate) fn validate_snapshot_room(raw: &str, projects: &[String]) -> Result<PathBuf, String> {
    let canonical = project_fs::canonical_project_path(Path::new(raw))
        .map_err(|error| format!("invalid room root: {error}"))?;
    let room_name = canonical.file_name().and_then(|name| name.to_str());
    let workspace_name = canonical
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if !project_fs::project_path_is_dir(&canonical)
        || !room_name.is_some_and(crate::config::entity_prefix::has_entity_prefix)
        || !workspace_name.is_some_and(crate::config::ac_root::is_ac_root_name)
    {
        return Err(
            "room root must be a room-* or wg-* directory directly under an AC workspace".into(),
        );
    }
    let configured = projects
        .iter()
        .filter_map(|project| project_fs::canonical_project_path(Path::new(project)).ok())
        .any(|project| canonical.starts_with(project));
    if !configured {
        return Err("room root is outside configured project paths".into());
    }
    Ok(PathBuf::from(raw))
}

/// Shared Tauri/WS admission and bounded snapshot reader. All guards are dropped
/// before path IO, storage locking or recovery on the blocking worker.
pub(crate) async fn task_snapshot_inner(
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    session_id: Option<&str>,
    workgroup_root: Option<&str>,
) -> Result<serde_json::Value, String> {
    let (cwd, raw, projects) = match (session_id, workgroup_root) {
        (Some(id), None) => {
            let uuid =
                Uuid::parse_str(id).map_err(|error| format!("invalid session id: {error}"))?;
            let manager = manager.read().await.clone();
            let session = manager
                .get_session(uuid)
                .await
                .ok_or_else(|| "session not found".to_string())?;
            (Some(session.working_directory), None, Vec::new())
        }
        (None, Some(raw)) => {
            let projects = settings.read().await.project_paths.clone();
            (None, Some(raw.to_owned()), projects)
        }
        _ => return Err("exactly one sessionId or workgroupRoot is required".into()),
    };
    tokio::task::spawn_blocking(move || {
        let root = match cwd {
            Some(cwd) => crate::session::session::find_workgroup_task_path_for_cwd(&cwd)
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or_else(|| "session is outside a room".to_string())?,
            None => validate_snapshot_room(raw.as_deref().ok_or("missing room root")?, &projects)?,
        };
        crate::commands::entity_creation::read_room_task_snapshot_json(&root)
    })
    .await
    .map_err(|error| format!("snapshot worker failed: {error}"))?
}

#[tauri::command]
pub async fn task_get_snapshot(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    task_snapshot_inner(&session_mgr, &settings, Some(&session_id), None).await
}

#[tauri::command]
pub async fn task_get_snapshot_at(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    workgroup_root: String,
) -> Result<serde_json::Value, String> {
    task_snapshot_inner(&session_mgr, &settings, None, Some(&workgroup_root)).await
}

pub(crate) fn task_sentinels(root: &Path) -> Result<TaskSentinels, String> {
    let stat = |name| match project_fs::project_path_metadata(&root.join(name)) {
        Ok(metadata) => Ok(Some(StatSentinel {
            len: metadata.len(),
            mtime: metadata.modified().ok(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("snapshot sentinel read failed: {error}")),
    };
    Ok(TaskSentinels {
        task: stat("TASK.md")?,
        status: stat("TASK-status.jsonl")?,
        journal: stat("TASK-clean.pending.json")?.is_some(),
    })
}

pub(crate) fn poll_task_snapshot(
    root: &Path,
    previous: Option<TaskSentinels>,
) -> Result<Option<TaskCacheEntry>, String> {
    let before = task_sentinels(root)?;
    if !before.journal && previous.as_ref() == Some(&before) {
        return Ok(None);
    }
    let value = crate::commands::entity_creation::read_room_task_snapshot_json(root)?;
    let after = task_sentinels(root)?;
    if before != after {
        return Ok(None);
    }
    let task: Option<String> =
        serde_json::from_value(value["task"].clone()).map_err(|error| error.to_string())?;
    Ok(Some(TaskCacheEntry {
        sentinel: after,
        task: task.as_deref().and_then(extract_task_first_line),
        task_title: serde_json::from_value(value["taskTitle"].clone())
            .map_err(|error| error.to_string())?,
        status: serde_json::from_value(value["status"].clone())
            .map_err(|error| error.to_string())?,
        revision: value["revision"]
            .as_str()
            .ok_or("snapshot revision missing")?
            .to_owned(),
        tail_incomplete: value["tailIncomplete"]
            .as_bool()
            .ok_or("snapshot tail flag missing")?,
    }))
}

/// The caller commits the returned cache only on success. Failed emission leaves
/// the old sentinels intact, so an unchanged-stat next pass retries delivery.
pub(crate) fn deliver_poll_task(
    previous: Option<&TaskCacheEntry>,
    next: TaskCacheEntry,
    emit: impl FnOnce(&TaskCacheEntry) -> Result<(), String>,
) -> Result<TaskCacheEntry, String> {
    let changed = previous.is_none_or(|old| {
        old.task != next.task
            || old.task_title != next.task_title
            || old.status != next.status
            || old.revision != next.revision
            || old.tail_incomplete != next.tail_incomplete
    });
    if changed {
        emit(&next)?;
    }
    Ok(next)
}
