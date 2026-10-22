use super::matrix_layout::{selected_ac_root, validate_existing_name};
use super::team_config::TeamConfigMutationGuard;
use super::team_update::emit_coordinator_refresh;
use super::workgroup::{
    acquire_lifecycle_project_gate, prune_team_workgroups_scope, remove_replica_dir,
    ReplicaRemovalOutcome,
};
use super::workgroup_delete::check_workgroup_repos_dirty;
use crate::config::seed_manifest::ManifestActivationToken;
use crate::session::manager::SessionManager;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// Committed-intent outcome of a team delete's workgroup cascade (#1063).
pub(super) struct TeamDeleteReport {
    /// Workgroup names whose directory was removed or was already absent at the
    /// explicit delete event: their config scopes are pruned and clocks cleaned.
    pub(super) removed_workgroups: Vec<String>,
    /// Workgroup names whose removal failed and whose original path remains: rows
    /// and clock keys are preserved.
    pub(super) failed_workgroups: Vec<String>,
}

/// Scan `base` for the present `room-N-<team>` and legacy `wg-N-<team>`
/// directories owned by `team_name`. Deleting a team removes both kinds, which
/// is the only coherent meaning of deleting a team (#1614).
pub(super) fn collect_team_workgroup_dirs(base: &Path, team_name: &str) -> Vec<PathBuf> {
    let wg_suffix = format!("-{}", team_name);
    let mut wg_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // #1614: the middle segment comes from the STRIPPED remainder, never
            // from an index. The literal 3 was "wg-".len(); "room-".len() is 5,
            // so an index form would mis-slice every Room name and, for a team
            // name long enough, panic on a reversed range.
            if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name_str) {
                if let Some(middle) = rest.strip_suffix(&wg_suffix) {
                    if middle.parse::<u32>().is_ok() {
                        wg_dirs.push(entry.path());
                    }
                }
            }
        }
    }
    wg_dirs
}

/// Delete a team directory from {project_path}/.ac/_team_{name}/
#[tauri::command]
pub async fn delete_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
) -> Result<(), String> {
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    delete_team_inner(
        &app,
        session_mgr.inner(),
        &project_path,
        &team_name,
        Some(ManifestActivationToken::production()),
        |_| {},
    )
    .await
}

/// GUI team deletion with the #1063 global lock order (plan sections 5.4/6.3).
///
/// Prompts, dirty checks, and the initial owned workgroup snapshot run outside
/// both guards. The blocking commit acquires the project gate then the #1056
/// team-config guard, revalidates the team directory under both, deletes it, and
/// attempts each workgroup while both are held. In the same manifest transaction
/// it prunes the committed intent (workgroups whose removal succeeded plus those
/// already absent at the explicit delete event) and preserves rows for any
/// workgroup whose removal failed and whose path remains. Both guards are dropped
/// before coordinator-clock locking/persistence and the async refresh, and no
/// synchronous guard crosses `.await`.
pub(crate) async fn delete_team_inner<H>(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    project_path: &str,
    team_name: &str,
    activation: Option<ManifestActivationToken>,
    after_project_before_team: H,
) -> Result<(), String>
where
    H: FnOnce(&Path) + Send + 'static,
{
    validate_existing_name(team_name, "Team")?;
    let base = selected_ac_root(Path::new(project_path))?;
    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    // Initial owned snapshot outside both guards: the present workgroup dirs and a
    // dirty-repo safety check.
    let wg_dirs = collect_team_workgroup_dirs(&base, team_name);
    let dirty_repos = check_workgroup_repos_dirty(&wg_dirs);
    if !dirty_repos.is_empty() {
        let list = dirty_repos
            .iter()
            .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "Cannot delete team: the following repos have pending work:\n{}\n\nCommit or push \
             changes before deleting.",
            list
        ));
    }

    // Blocking commit: project gate then the #1056 team-config guard. Neither
    // synchronous guard crosses an await.
    let project_root = Path::new(project_path).to_path_buf();
    let team = team_name.to_string();
    let base_owned = base.clone();
    let team_dir_owned = team_dir.clone();
    let wg_dirs_owned = wg_dirs.clone();
    let report = crate::config::seed_manifest::run_blocking_owned(
        move || -> Result<TeamDeleteReport, String> {
            let mut project_gate = acquire_lifecycle_project_gate(&project_root)?;
            after_project_before_team(&base_owned);
            let team_guard = TeamConfigMutationGuard::acquire(&base_owned)?;
            team_guard.require_ac_root(&base_owned)?;

            // Revalidate the team directory under both guards before the commit.
            match std::fs::symlink_metadata(&team_dir_owned) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    drop(team_guard);
                    drop(project_gate);
                    return Err(format!("Team '{}' not found", team));
                }
                Err(e) => {
                    drop(team_guard);
                    drop(project_gate);
                    return Err(format!("Failed to inspect team directory: {}", e));
                }
            }

            // Commit: remove the team directory, then attempt each workgroup while
            // both guards remain held.
            std::fs::remove_dir_all(&team_dir_owned)
                .map_err(|e| format!("Failed to delete team directory: {}", e))?;
            let mut removed_workgroups = Vec::new();
            let mut failed_workgroups = Vec::new();
            for wg in &wg_dirs_owned {
                let wg_name = wg
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match remove_replica_dir(wg) {
                    ReplicaRemovalOutcome::Removed | ReplicaRemovalOutcome::AlreadyAbsent => {
                        removed_workgroups.push(wg_name);
                    }
                    ReplicaRemovalOutcome::Failed(error) => {
                        log::warn!(
                            "[entity_creation] Failed to delete workgroup {}: {}",
                            wg_name,
                            error
                        );
                        failed_workgroups.push(wg_name);
                    }
                }
            }

            // Prune the committed intent (removed plus already-absent) under the
            // gate; failed workgroups keep their rows.
            prune_team_workgroups_scope(
                project_gate.as_mut(),
                activation.as_ref(),
                removed_workgroups.clone(),
            );
            drop(team_guard);
            drop(project_gate);
            Ok(TeamDeleteReport {
                removed_workgroups,
                failed_workgroups,
            })
        },
    )
    .await
    .map_err(|e| format!("Team delete blocking preparation failed: {e}"))??;

    log::info!(
        "[entity_creation] Deleted team: {} (workgroups removed={}, failed={})",
        team_name,
        report.removed_workgroups.len(),
        report.failed_workgroups.len()
    );

    // Post-commit (guards released): drop coordinator-clock keys for removed
    // workgroups and persist once, then refresh.
    let mut team_clock_removed = 0usize;
    if let Some(project_name) = Path::new(project_path).file_name().and_then(|n| n.to_str()) {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            for wg_name in &report.removed_workgroups {
                team_clock_removed += clocks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove_workgroup(project_name, wg_name);
            }
        }
    }
    if team_clock_removed > 0 {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
            if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
                log::warn!("[delete-team] clocks save failed: {}", e);
            }
        }
    }
    emit_coordinator_refresh(app, session_mgr).await;
    Ok(())
}
