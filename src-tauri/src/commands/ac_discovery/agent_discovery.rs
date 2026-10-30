mod replica;
mod teams;

use super::branch_watcher::DiscoveryBranchWatcher;
use super::gitignore::ensure_ac_root_gitignore_with_names;
use super::model::{AcAgentMatrix, AcAgentReplica, AcDiscoveryResult, AcTeam, AcWorkgroup};
use super::replica_read::{
    agent_display_name, projected_path_string, read_preferred_agent_id, read_task_fields,
    resolve_agent_ref,
};
use super::DISCOVERY_CALL_ID;
use crate::config::ac_root::existing_ac_root;
use crate::config::entity_creation::teams::read_discovery_team_config;
use crate::config::settings::SettingsState;
use crate::filesystem::ac_discovery::{
    discovery_directory_entries, discovery_path_is_dir, discovery_role_exists,
};
use crate::session::manager::SessionManager;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::Emitter;
use tauri::{AppHandle, State};

/// #1065 Stage F: run the project context-template scan under the seed-manifest
/// gate, recording each recognized create/update at its commit-point time.
///
/// A pre-contention gate degradation runs the scan untracked (writes proceed,
/// unrecorded); a contention/unsafe failure skips the scan so no write races a
/// cooperating writer (plan sections 5.2/6.1). `activation` is `None` for a
/// `#[cfg(test)]` lib build or an unactivated caller, which runs the plain scan
/// unchanged. Runs inside the discovery `run_blocking_owned` closure so the
/// 10-second gate poll never blocks a Tokio worker.
pub(crate) fn scan_project_context_templates_recorded(
    project_root: &Path,
    ac_root: &Path,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Vec<crate::config::seeded_context_templates::ContextTemplateUpdate>, String> {
    use crate::config::seed_manifest::SoftProjectGate;
    use crate::config::seeded_context_templates::{
        scan_project_context_template_updates,
        scan_project_context_template_updates_with_publications, ContextPublication,
    };
    let Some(token) = activation else {
        return scan_project_context_template_updates(project_root, ac_root);
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let result = {
                let mut on_publication =
                    |filename: &'static str, publication: ContextPublication| {
                        crate::config::session_context::record_project_context_publication(
                            &mut guard,
                            token,
                            filename,
                            publication.published_at,
                        );
                    };
                scan_project_context_template_updates_with_publications(
                    project_root,
                    ac_root,
                    &mut on_publication,
                )
            };
            guard.release();
            result
        }
        SoftProjectGate::DegradedUntracked => {
            scan_project_context_template_updates(project_root, ac_root)
        }
        SoftProjectGate::Unavailable(error) => {
            log::warn!(
                "[context-templates] skipping gated context scan for {}: {}",
                ac_root.display(),
                error
            );
            Ok(Vec::new())
        }
    }
}

/// Discover AC agent matrices from Project AC Root directories within configured repo paths.
#[tauri::command]
pub async fn discover_ac_agents(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    branch_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
    coordinator_clocks: State<'_, crate::config::coordinator_clocks::CoordinatorClocksState>,
) -> Result<AcDiscoveryResult, String> {
    let cfg = settings.read().await.clone();
    // #552 snapshot the persisted badge/auto-closed store once so the per-replica
    // scan below never locks inside the loop.
    let clocks_snapshot: std::collections::HashMap<
        String,
        crate::config::coordinator_clocks::ClockEntry,
    > = coordinator_clocks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot();
    // Discovery-wide team snapshot — used per-replica for is_coordinator
    // and at the end for refresh_coordinator_flags. Computed once so a
    // single discovery pass presents a coherent coordinator view.
    // Lock-safe: the settings snapshot is owned and the read guard was dropped
    // before any filesystem scan.
    let teams_snapshot = crate::config::teams::discover_teams();
    let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);
    let DiscoveryScan {
        agents,
        teams,
        mut workgroups,
        loops,
        mut context_template_updates,
        wg_project_map,
    } = scan_agents(&cfg, &clocks_snapshot, call_id).await;
    drop(cfg);
    teams::associate_teams(&mut workgroups, &teams);

    // Update the branch watcher per-project. Each Project AC Root-containing dir gets its own
    // slot so multi-project setups don't overwrite each other (Grinch #1 + #12).
    let mut by_project: HashMap<String, Vec<AcWorkgroup>> = HashMap::new();
    for wg in &workgroups {
        if let Some(proj) = wg_project_map.get(&wg.name) {
            by_project.entry(proj.clone()).or_default().push(wg.clone());
        }
    }
    for (proj, wgs) in &by_project {
        branch_watcher.update_replicas_for_project(proj, wgs);
    }

    // Recompute coordinator flags on every live session against the hoisted team snapshot.
    let changes = {
        let mgr = session_mgr.read().await;
        mgr.refresh_coordinator_flags(&teams_snapshot).await
    };
    for (id, is_coord) in changes {
        let _ = app.emit(
            "session_coordinator_changed",
            crate::pty::git_watcher::CoordinatorChangedPayload {
                session_id: id.to_string(),
                is_coordinator: is_coord,
            },
        );
    }

    let total_replicas: usize = workgroups.iter().map(|wg| wg.agents.len()).sum();
    let total_coordinator: usize = workgroups
        .iter()
        .flat_map(|wg| wg.agents.iter())
        .filter(|a| a.is_coordinator)
        .count();
    log::debug!(
        "[ac-discovery] call={} discover_ac_agents: summary — workgroups={} teams={} replicas={} \
            coordinator={}",
        call_id,
        workgroups.len(),
        teams.len(),
        total_replicas,
        total_coordinator
    );
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

async fn scan_agents(
    cfg: &crate::config::settings::AppSettings,
    clocks_snapshot: &HashMap<String, crate::config::coordinator_clocks::ClockEntry>,
    call_id: u64,
) -> DiscoveryScan {
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();
    let mut loops: Vec<crate::config::loops::AcLoopSummary> = Vec::new();
    let mut context_template_updates = Vec::new();
    // Track the Project AC Root-containing dir each workgroup originated from. Keys are
    // `wg.name` values (unique within a discovery run; workgroup dir names include
    // the team name which collides only intentionally across projects). Populated as
    // we push to `workgroups` so we can later call `update_replicas_for_project` once
    // per project rather than once globally (Grinch #1 + #12).
    let mut wg_project_map: HashMap<String, String> = HashMap::new();

    for base_path in &cfg.project_paths {
        let base = Path::new(base_path);
        if !crate::config::projects::configured_project_is_dir(base) {
            continue;
        }

        // Also check children of the base path (same pattern as search_repos)
        let dirs_to_check: Vec<std::path::PathBuf> = {
            let mut dirs = vec![base.to_path_buf()];
            crate::config::projects::visit_discovery_project_children(base, |p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') {
                    dirs.push(p);
                }
            });
            dirs
        };

        for repo_dir in dirs_to_check {
            let Some(ac_root) = existing_ac_root(&repo_dir) else {
                continue;
            };
            let repo_dir_str = projected_path_string(&repo_dir);

            // Opportunistic: ensure gitignore exists for existing projects
            let _ = ensure_ac_root_gitignore_with_names(
                &ac_root,
                &crate::config::agent_command::managed_instructions_filenames(cfg),
            );
            scan_context_templates(&repo_dir, &ac_root, &mut context_template_updates).await;
            loops.extend(crate::config::loops::discover_loops_in_project(&repo_dir));

            let project_folder = repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let entries = match discovery_directory_entries(&ac_root) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !discovery_path_is_dir(&path) {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Agent matrices: _agent_* (single underscore prefix)
                if dir_name.starts_with("_agent_") {
                    let display_name = agent_display_name(&project_folder, &dir_name);
                    let role_exists = discovery_role_exists(&path.join("Role.md"));

                    let preferred_agent_id = read_preferred_agent_id(&path, cfg);

                    log::info!(
                        "[ac-discovery] agent: dir={:?}, preferred_agent_id={:?}",
                        dir_name,
                        preferred_agent_id
                    );

                    agents.push(AcAgentMatrix {
                        name: display_name,
                        path: projected_path_string(&path),
                        role_exists,
                        preferred_agent_id,
                    });
                }

                // Workgroups: wg-*
                if crate::config::entity_prefix::has_entity_prefix(&dir_name) {
                    workgroups.push(
                        discover_workgroup(
                            &path,
                            &dir_name,
                            &project_folder,
                            &ac_root,
                            cfg,
                            clocks_snapshot,
                            call_id,
                        )
                        .await,
                    );
                    wg_project_map.insert(dir_name.clone(), repo_dir_str.clone());
                }

                // Teams: _team_*
                if dir_name.starts_with("_team_") {
                    let team_name = dir_name
                        .strip_prefix("_team_")
                        .unwrap_or(&dir_name)
                        .to_string();

                    if let Some(parsed) = read_discovery_team_config(&path) {
                        let team_agents = parsed
                            .get("agents")
                            .and_then(|a| a.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .map(|r| resolve_agent_ref(&project_folder, r))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();

                        let coordinator = parsed
                            .get("coordinator")
                            .and_then(|c| c.as_str())
                            .map(|r| resolve_agent_ref(&project_folder, r));

                        teams.push(AcTeam {
                            name: team_name,
                            agents: team_agents,
                            coordinator,
                        });
                    }
                }
            }
        }
    }

    // Sort alphabetically
    agents.sort_by_key(|a| a.name.to_lowercase());
    teams.sort_by_key(|a| a.name.to_lowercase());
    workgroups.sort_by_key(|a| a.name.to_lowercase());
    loops.sort_by_key(|a| a.name.to_lowercase());

    DiscoveryScan {
        agents,
        teams,
        workgroups,
        loops,
        context_template_updates,
        wg_project_map,
    }
}

async fn scan_context_templates(
    repo_dir: &Path,
    ac_root: &Path,
    context_template_updates: &mut Vec<
        crate::config::seeded_context_templates::ContextTemplateUpdate,
    >,
) {
    let context_scan = crate::config::seed_manifest::run_blocking_owned({
        let repo_dir = repo_dir.to_path_buf();
        let ac_root = ac_root.to_path_buf();
        move || {
            // #1065 Stage F: production records recognized context updates
            // under the gate; a `#[cfg(test)]` lib build stays non-emitting.
            #[cfg(not(test))]
            let activation =
                Some(crate::config::seed_manifest::ManifestActivationToken::production());
            #[cfg(test)]
            let activation: Option<
                crate::config::seed_manifest::ManifestActivationToken,
            > = None;
            scan_project_context_templates_recorded(&repo_dir, &ac_root, activation.as_ref())
        }
    })
    .await;
    match context_scan {
        Ok(Ok(mut updates)) => context_template_updates.append(&mut updates),
        Ok(Err(e)) => log::warn!(
            "[context-templates] scan failed for {}: {}",
            ac_root.display(),
            e
        ),
        Err(e) => log::warn!(
            "[context-templates] blocking scan failed for {}: {}",
            ac_root.display(),
            e
        ),
    }
}

async fn discover_workgroup(
    path: &Path,
    dir_name: &str,
    project_folder: &str,
    ac_root: &Path,
    cfg: &crate::config::settings::AppSettings,
    clocks_snapshot: &HashMap<String, crate::config::coordinator_clocks::ClockEntry>,
    call_id: u64,
) -> AcWorkgroup {
    let (task, task_title) = read_task_fields(path);

    // Find first repo-* directory for CWD
    let repo_path = discovery_directory_entries(path)
        .ok()
        .and_then(|entries| {
            entries.flatten().find(|e| {
                let n = e.file_name();
                let name = n.to_string_lossy();
                name.starts_with("repo-") && discovery_path_is_dir(&e.path())
            })
        })
        .map(|e| projected_path_string(&e.path()));

    // Scan __agent_* replicas inside the WG
    let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
    if let Ok(wg_entries) = discovery_directory_entries(path) {
        for wg_entry in wg_entries.flatten() {
            let wg_path = wg_entry.path();
            if !discovery_path_is_dir(&wg_path) {
                continue;
            }
            let wg_dir_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if wg_dir_name.starts_with("__agent_") {
                wg_agents.push(
                    replica::discover_replica(
                        &wg_path,
                        &wg_dir_name,
                        dir_name,
                        path,
                        &replica::DiscoveryContext {
                            project_folder,
                            ac_root,
                            cfg,
                            clocks_snapshot,
                            call_id,
                        },
                    )
                    .await,
                );
            }
        }
    }
    wg_agents.sort_by_key(|a| a.name.to_lowercase());

    AcWorkgroup {
        name: dir_name.to_string(),
        path: projected_path_string(path),
        task,
        task_title,
        agents: wg_agents,
        repo_path,
        team_name: None,
    }
}

struct DiscoveryScan {
    agents: Vec<AcAgentMatrix>,
    teams: Vec<AcTeam>,
    workgroups: Vec<AcWorkgroup>,
    loops: Vec<crate::config::loops::AcLoopSummary>,
    context_template_updates: Vec<crate::config::seeded_context_templates::ContextTemplateUpdate>,
    wg_project_map: HashMap<String, String>,
}
