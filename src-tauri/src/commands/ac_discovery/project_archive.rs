use super::project_registry::{
    broadcast_project_archive_changed, mutate_project_paths_with_settings_path, ArchiveChangeReason,
};
use crate::config::settings::SettingsState;
use crate::pty::manager::{PendingSpawn, PtyManager};
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionStatus};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::State;
use uuid::Uuid;

pub(crate) struct ArchiveSnapshot {
    pub(crate) sessions: Vec<(SessionInfo, bool)>,
    pub(crate) pending: Vec<PendingSpawn>,
}

pub(crate) async fn snapshot_archive_state(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
) -> ArchiveSnapshot {
    let sessions = {
        let mgr = session_mgr.read().await;
        mgr.list_sessions().await
    };
    let session_ids = sessions
        .iter()
        .map(|s| Uuid::parse_str(&s.id).ok())
        .collect::<Vec<_>>();
    let ids = session_ids.iter().flatten().copied().collect::<Vec<_>>();
    let (pending, live) = {
        let pty = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        pty.archive_liveness(&ids)
    };
    let live_by_id = ids.into_iter().zip(live).collect::<HashMap<_, _>>();
    let sessions = sessions
        .into_iter()
        .zip(session_ids)
        .map(|(s, id)| {
            let has_pty = id
                .and_then(|id| live_by_id.get(&id).copied())
                .unwrap_or(false);
            (s, has_pty)
        })
        .collect();
    ArchiveSnapshot { sessions, pending }
}

pub(crate) fn session_is_live(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => false,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

pub(crate) fn archive_scope_contains(cwd: &str, project_path: &str) -> bool {
    let scope = [project_path.to_string()];
    crate::config::sessions_persistence::working_directory_under_any_project_path(cwd, &scope)
}

pub(crate) fn is_root_agent_cwd(cwd: &str) -> bool {
    crate::config::root_agent::is_root_agent_dir_name(cwd)
        || crate::config::root_agent::is_root_agent_path(cwd)
}

pub(crate) fn archive_blockers(snapshot: &ArchiveSnapshot, project_path: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut blockers = Vec::new();

    for pending in &snapshot.pending {
        if is_root_agent_cwd(&pending.cwd) {
            continue;
        }
        if !archive_scope_contains(&pending.cwd, project_path) {
            continue;
        }
        if seen.insert(pending.label.clone()) {
            blockers.push(pending.label.clone());
        }
    }

    for (session, _) in snapshot
        .sessions
        .iter()
        .filter(|(s, _)| !(s.is_root_agent || is_root_agent_cwd(&s.working_directory)))
        .filter(|(s, has_pty)| session_is_live(&s.status, *has_pty))
        .filter(|(s, _)| archive_scope_contains(&s.working_directory, project_path))
    {
        if seen.insert(session.name.clone()) {
            blockers.push(session.name.clone());
        }
    }

    blockers
}

pub(crate) fn archive_blocked_message(blockers: &[String]) -> String {
    const MAX_NAMED: usize = 3;
    let named = blockers
        .iter()
        .take(MAX_NAMED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let overflow = blockers.len().saturating_sub(MAX_NAMED);
    if overflow > 0 {
        format!(
            "Cannot archive: {} open session(s) in this project ({}, and {} more). Close them \
                first.",
            blockers.len(),
            named,
            overflow
        )
    } else {
        format!(
            "Cannot archive: {} open session(s) in this project ({}). Close them first.",
            blockers.len(),
            named
        )
    }
}

pub(crate) async fn archive_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    path: &str,
) -> Result<(), String> {
    archive_project_inner_with_settings_path(app, settings, session_mgr, pty_mgr, path, None).await
}

pub(crate) async fn archive_project_inner_with_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(), String> {
    let path = crate::config::projects::absolutise(path)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let before = snapshot_archive_state(session_mgr, pty_mgr).await;
    let blockers = archive_blockers(&before, &path);
    if !blockers.is_empty() {
        return Err(archive_blocked_message(&blockers));
    }

    let mutation_path = path.to_string();
    mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
        let path = mutation_path.as_str();
        crate::config::projects::archive_project_path(s, path);
        Ok(())
    })
    .await?;

    let after = snapshot_archive_state(session_mgr, pty_mgr).await;
    let late = archive_blockers(&after, &path);
    if late.is_empty() {
        broadcast_project_archive_changed(app, &path, true, ArchiveChangeReason::Archive, None);
        return Ok(());
    }

    let mutation_path = path.clone();
    let restored: Result<bool, String> =
        mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
            let path = mutation_path.as_str();
            let key = crate::config::projects::normalize_for_compare(path);
            let still_archived = s
                .archived_project_paths
                .iter()
                .any(|p| crate::config::projects::normalize_for_compare(p) == key);
            if !still_archived {
                return Ok(false);
            }
            Ok(crate::config::projects::unarchive_project_path(s, path))
        })
        .await;

    match restored {
        Ok(false) => Err(archive_blocked_message(&late)),
        Ok(true) => {
            broadcast_project_archive_changed(
                app,
                &path,
                false,
                ArchiveChangeReason::Unarchive,
                None,
            );
            // #1318 - the rollback re-registered the project; seed its catalog
            // now so reads target the project `.ac` without a restart.
            let seed_path = path.clone();
            if let Err(error) = crate::config::seed_manifest::run_blocking_owned(move || {
                crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(
                    &seed_path,
                ));
            })
            .await
            {
                log::warn!(
                    "[ac-discovery] catalog seed after archive rollback failed: {}",
                    error
                );
            }
            Err(archive_blocked_message(&late))
        }
        Err(e) => {
            log::error!(
                "[archive] '{}' has {} live session(s) but could not be restored: {}",
                path,
                late.len(),
                e
            );
            Err(format!(
                "{} The project could not be restored automatically ({}). Open Archived Projects \
                    to restore it.",
                archive_blocked_message(&late),
                e
            ))
        }
    }
}

#[tauri::command]
pub async fn archive_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    path: String,
) -> Result<(), String> {
    archive_project_inner(
        &app,
        settings.inner(),
        session_mgr.inner(),
        pty_mgr.inner(),
        &path,
    )
    .await
}

pub(crate) async fn unarchive_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let mutation_path = path.to_string();
    let result = mutate_project_paths_with_settings_path(settings, None, move |s| {
        crate::config::projects::register_existing_project(s, &mutation_path)
            .map_err(|e| e.to_string())
    })
    .await?;
    broadcast_project_archive_changed(
        app,
        &result.path,
        false,
        ArchiveChangeReason::Unarchive,
        None,
    );
    // #1318 - unarchive is a re-registration; seed the catalog + masters now so
    // reads target the project `.ac` without a restart (the boot loop covers it
    // at the next boot either way). Fail-soft; gate acquire on a blocking thread.
    if result.registered {
        let path = result.path.clone();
        if let Err(error) = crate::config::seed_manifest::run_blocking_owned(move || {
            crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&path));
        })
        .await
        {
            log::warn!(
                "[ac-discovery] catalog seed after unarchive failed: {}",
                error
            );
        }
    }
    Ok(result)
}

#[tauri::command]
pub async fn unarchive_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    unarchive_project_inner(&app, settings.inner(), &path).await
}

pub(crate) async fn list_archived_projects_inner(
    settings: &SettingsState,
) -> Result<Vec<crate::config::projects::ArchivedProject>, String> {
    let paths = {
        let s = settings.read().await;
        s.archived_project_paths.clone()
    };
    tokio::task::spawn_blocking(move || {
        crate::config::projects::archived_projects_from_paths(&paths)
    })
    .await
    .map_err(|e| format!("archived project stat task failed: {}", e))
}

#[tauri::command]
pub async fn list_archived_projects(
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::config::projects::ArchivedProject>, String> {
    list_archived_projects_inner(settings.inner()).await
}
