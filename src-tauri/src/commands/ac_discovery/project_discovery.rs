mod tree;

use super::agent_discovery::scan_project_context_templates_recorded;
use super::branch_watcher::DiscoveryBranchWatcher;
use super::gitignore::ensure_ac_root_gitignore_with_names;
use super::model::AcDiscoveryResult;
use super::DISCOVERY_CALL_ID;
use crate::config::ac_root::existing_ac_root;
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::Emitter;
use tauri::{AppHandle, State};

/// Discover AC agents/workgroups from a single project path.
/// Unlike discover_ac_agents which scans project_paths from settings,
/// this targets a specific folder.
#[tauri::command]
pub async fn discover_project(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    path: String,
    settings: State<'_, SettingsState>,
    branch_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
    coordinator_clocks: State<'_, crate::config::coordinator_clocks::CoordinatorClocksState>,
) -> Result<AcDiscoveryResult, String> {
    discover_project_inner(
        &app,
        session_mgr.inner(),
        &path,
        settings.inner(),
        branch_watcher.inner(),
        coordinator_clocks.inner(),
    )
    .await
}

pub(crate) async fn discover_project_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    path: &str,
    settings: &SettingsState,
    branch_watcher: &Arc<DiscoveryBranchWatcher>,
    coordinator_clocks: &crate::config::coordinator_clocks::CoordinatorClocksState,
) -> Result<AcDiscoveryResult, String> {
    let base = Path::new(path);
    if !crate::config::projects::configured_project_is_dir(base) {
        return Err(format!("Path is not a directory: {}", path));
    }

    let cfg = settings.read().await.clone();
    // #552 snapshot the persisted badge/auto-closed store once (see discover_ac_agents).
    let clocks_snapshot: std::collections::HashMap<
        String,
        crate::config::coordinator_clocks::ClockEntry,
    > = coordinator_clocks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot();

    let Some(ac_root) = existing_ac_root(base) else {
        return Ok(AcDiscoveryResult {
            agents: vec![],
            teams: vec![],
            workgroups: vec![],
            loops: vec![],
            context_template_updates: vec![],
        });
    };

    // Opportunistic: ensure gitignore protects workgroup clones
    let _ = ensure_ac_root_gitignore_with_names(
        &ac_root,
        &crate::config::agent_command::managed_instructions_filenames(&cfg),
    );
    let mut context_template_updates = scan_project_context_updates(base, &ac_root).await;

    // Discovery-wide team snapshot — see discover_ac_agents for rationale.
    // Lock-safe: the settings snapshot is owned and the read guard was dropped
    // before any filesystem scan.
    // Placed AFTER the workspace-missing early return so non-AC folders don't
    // pay a wasted filesystem scan (§15 Finding F1).
    let teams_snapshot = crate::config::teams::discover_teams();
    let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);

    let project_folder = base
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut loops = crate::config::loops::discover_loops_in_project(base);
    let tree::DiscoveryTree {
        mut agents,
        mut teams,
        mut workgroups,
    } = tree::discover_project_tree(&ac_root, &project_folder, &cfg, &clocks_snapshot, call_id)
        .await?;

    agents.sort_by_key(|a| a.name.to_lowercase());
    teams.sort_by_key(|a| a.name.to_lowercase());
    workgroups.sort_by_key(|a| a.name.to_lowercase());
    loops.sort_by_key(|a| a.name.to_lowercase());

    tree::associate_workgroups_with_teams(&mut workgroups, &teams);

    drop(cfg);
    // Update the branch watcher for THIS project only.
    branch_watcher.update_replicas_for_project(path, &workgroups);

    // Recompute coordinator flags on every live session against the hoisted team
    // snapshot. Spawned (not awaited) so populating the project tree NEVER blocks on
    // `session_mgr` / inner `sessions` lock contention during the boot-concurrent
    // session restore (#384 empty-tree hang fix). This refresh only emits
    // `session_coordinator_changed` for live sessions; it is not needed to build the
    // returned tree (each replica's `is_coordinator` is computed inline above), so
    // moving it off the return path is behavior-preserving for the tree while keeping
    // the coordinator-flag events flowing.
    {
        let coordinator_app = app.clone();
        let coordinator_session_mgr = Arc::clone(session_mgr);
        tokio::spawn(async move {
            let changes = {
                let mgr = coordinator_session_mgr.read().await;
                mgr.refresh_coordinator_flags(&teams_snapshot).await
            };
            for (id, is_coord) in changes {
                let _ = coordinator_app.emit(
                    "session_coordinator_changed",
                    crate::pty::git_watcher::CoordinatorChangedPayload {
                        session_id: id.to_string(),
                        is_coordinator: is_coord,
                    },
                );
            }
        });
    }

    log_project_discovery_summary(call_id, path, &workgroups, teams.len());
    crate::config::seeded_context_templates::dedupe_context_template_updates(
        &mut context_template_updates,
    );

    Ok(AcDiscoveryResult {
        agents,
        teams,
        workgroups,
        loops,
        context_template_updates,
    })
}

fn log_project_discovery_summary(
    call_id: u64,
    path: &str,
    workgroups: &[super::model::AcWorkgroup],
    team_count: usize,
) {
    let total_replicas: usize = workgroups.iter().map(|wg| wg.agents.len()).sum();
    let total_coordinator: usize = workgroups
        .iter()
        .flat_map(|wg| wg.agents.iter())
        .filter(|a| a.is_coordinator)
        .count();
    log::debug!(
        "[ac-discovery] call={} discover_project: summary — path='{}' workgroups={} teams={} \
            replicas={} coordinator={}",
        call_id,
        path,
        workgroups.len(),
        team_count,
        total_replicas,
        total_coordinator
    );
}

async fn scan_project_context_updates(
    base: &Path,
    ac_root: &Path,
) -> Vec<crate::config::seeded_context_templates::ContextTemplateUpdate> {
    let context_scan = crate::config::seed_manifest::run_blocking_owned({
        let base = base.to_path_buf();
        let ac_root = ac_root.to_path_buf();
        move || {
            // #1065 Stage F: production records recognized context updates under the
            // gate; a `#[cfg(test)]` lib build stays non-emitting.
            #[cfg(not(test))]
            let activation =
                Some(crate::config::seed_manifest::ManifestActivationToken::production());
            #[cfg(test)]
            let activation: Option<
                crate::config::seed_manifest::ManifestActivationToken,
            > = None;
            scan_project_context_templates_recorded(&base, &ac_root, activation.as_ref())
        }
    })
    .await;
    match context_scan {
        Ok(Ok(updates)) => updates,
        Ok(Err(e)) => {
            log::warn!(
                "[context-templates] scan failed for {}: {}",
                ac_root.display(),
                e
            );
            Vec::new()
        }
        Err(e) => {
            log::warn!(
                "[context-templates] blocking scan failed for {}: {}",
                ac_root.display(),
                e
            );
            Vec::new()
        }
    }
}
