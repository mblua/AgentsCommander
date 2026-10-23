use super::agent_discovery::scan_project_context_templates_recorded;
use super::branch_watcher::DiscoveryBranchWatcher;
use super::gitignore::ensure_ac_root_gitignore_with_names;
use super::model::{AcAgentMatrix, AcAgentReplica, AcDiscoveryResult, AcTeam, AcWorkgroup};
use super::project_git::detect_repo_branch;
use super::replica_read::{
    agent_display_name, origin_project_for_replica_identity, projected_path_string,
    read_preferred_agent_id, read_replica_config_with_valid_identity, read_task_fields,
    replica_selection_wire_fields, resolve_agent_ref,
};
use super::DISCOVERY_CALL_ID;
use crate::config::ac_root::existing_ac_root;
use crate::config::coding_agent_profiles::{
    read_replica_current_coding_agent, read_replica_profile_result,
};
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
    if !base.is_dir() {
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
    let context_scan = crate::config::seed_manifest::run_blocking_owned({
        let base = base.to_path_buf();
        let ac_root = ac_root.clone();
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
    let mut context_template_updates = match context_scan {
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
    };

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

    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();
    let mut loops = crate::config::loops::discover_loops_in_project(base);

    let entries = match std::fs::read_dir(&ac_root) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to read Project AC Root directory: {}", e)),
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let dir_name = match entry_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Agent matrices: _agent_*
        if dir_name.starts_with("_agent_") {
            let display_name = agent_display_name(&project_folder, &dir_name);
            let role_exists = entry_path.join("Role.md").exists();

            let preferred_agent_id = read_preferred_agent_id(&entry_path, &cfg);

            agents.push(AcAgentMatrix {
                name: display_name,
                path: projected_path_string(&entry_path),
                role_exists,
                preferred_agent_id,
            });
        }

        // Workgroups: wg-*
        if crate::config::entity_prefix::has_entity_prefix(&dir_name) {
            let (task, task_title) = read_task_fields(&entry_path);

            let repo_path = std::fs::read_dir(&entry_path)
                .ok()
                .and_then(|entries| {
                    entries.flatten().find(|e| {
                        let n = e.file_name();
                        let name = n.to_string_lossy();
                        name.starts_with("repo-") && e.path().is_dir()
                    })
                })
                .map(|e| projected_path_string(&e.path()));

            let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
            if let Ok(wg_entries) = std::fs::read_dir(&entry_path) {
                for wg_entry in wg_entries.flatten() {
                    let wg_path = wg_entry.path();
                    if !wg_path.is_dir() {
                        continue;
                    }
                    let wg_dir_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if wg_dir_name.starts_with("__agent_") {
                        let replica_name = wg_dir_name
                            .strip_prefix("__agent_")
                            .unwrap_or(&wg_dir_name)
                            .to_string();

                        let identity_read = read_replica_config_with_valid_identity(&wg_path);

                        let identity_path = identity_read
                            .identity
                            .as_ref()
                            .map(|identity| identity.identity.clone());

                        let origin_project =
                            origin_project_for_replica_identity(&project_folder, &identity_read);

                        let preferred_agent_id =
                            identity_read.identity.as_ref().and_then(|identity| {
                                read_preferred_agent_id(&identity.matrix_dir, &cfg)
                            });
                        let current_coding_agent_id = read_replica_current_coding_agent(&wg_path)
                            .filter(|id| cfg.agents.iter().any(|agent| agent.id == *id));
                        let profile_read = read_replica_profile_result(&wg_path);
                        if let Some(warning) = &profile_read.warning {
                            log::warn!("[ac-discovery] {}", warning);
                        }
                        let current_profile = profile_read.profile;

                        let repo_paths: Vec<String> = identity_read
                            .config
                            .as_ref()
                            .and_then(|v| v.get("repos")?.as_array().cloned())
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|r| r.as_str())
                            .filter_map(|rel| {
                                let resolved = wg_path.join(rel);
                                std::fs::canonicalize(&resolved)
                                    .ok()
                                    .map(|p| projected_path_string(&p))
                            })
                            .collect();

                        let repo_branch = if repo_paths.len() == 1 {
                            detect_repo_branch(&repo_paths[0]).await
                        } else {
                            None
                        };

                        // §AR2-strict: `is_coordinator` short-circuits on
                        // unqualified names, so build the project-qualified FQN
                        // (mirrors `agent_fqn_from_path`'s `<proj>:<wg>/<agent>`
                        // shape). Covered by
                        // teams::tests::is_any_coordinator_requires_qualified_fqn.
                        let is_coordinator = crate::config::teams::resolve_wg_coordinator_replica(
                            &ac_root,
                            &entry_path,
                        )
                        .map(|resolved| resolved.agent_name == replica_name)
                        .unwrap_or(false);

                        log::debug!(
                            "[ac-discovery] call={} replica — project='{}' wg='{}' replica='{}' \
                                fqn='{}:{}/{}' is_coordinator={}",
                            call_id,
                            project_folder,
                            dir_name,
                            replica_name,
                            project_folder,
                            dir_name,
                            replica_name,
                            is_coordinator
                        );

                        // #552 look up this coordinator's persisted clocks by FQN
                        // (same shape as agent_fqn_from_path: <project>:<wg>/<agent>).
                        let fqn = format!("{}:{}/{}", project_folder, dir_name, replica_name);
                        let clock_entry = if is_coordinator {
                            clocks_snapshot.get(&fqn)
                        } else {
                            None
                        };
                        // (#580) the badge field carries the unified idle anchor
                        // max(user, activity) (rename deferred, Decision C) so a
                        // dormant/just-loaded row shows the real accumulated idle
                        // (closed time included), not just the user-message age.
                        let last_user_message_at = clock_entry
                            .and_then(|e| e.idle_anchor())
                            .map(|dt| dt.to_rfc3339());
                        let auto_closed_at = clock_entry
                            .and_then(|e| e.auto_closed_at)
                            .map(|dt| dt.to_rfc3339());
                        let manually_closed_at = clock_entry
                            .and_then(|e| e.manually_closed_at)
                            .map(|dt| dt.to_rfc3339());

                        // #1941 - strict protection state from the same validated
                        // replica directory; an invalid state is surfaced as
                        // `invalid`, never as default false.
                        let (saved_pair, selection_state, selection_error) =
                            replica_selection_wire_fields(&wg_path);

                        wg_agents.push(AcAgentReplica {
                            name: replica_name,
                            path: projected_path_string(&wg_path),
                            identity_path,
                            origin_project,
                            preferred_agent_id,
                            current_coding_agent_id,
                            current_profile,
                            repo_paths,
                            repo_branch,
                            is_coordinator,
                            last_user_message_at,
                            auto_closed_at,
                            manually_closed_at,
                            saved_pair,
                            selection_state,
                            selection_error,
                        });
                    }
                }
            }
            wg_agents.sort_by_key(|a| a.name.to_lowercase());

            workgroups.push(AcWorkgroup {
                name: dir_name.clone(),
                path: projected_path_string(&entry_path),
                task,
                task_title,
                agents: wg_agents,
                repo_path,
                team_name: None,
            });
        }

        // Teams: _team_*
        if dir_name.starts_with("_team_") {
            let team_name = dir_name
                .strip_prefix("_team_")
                .unwrap_or(&dir_name)
                .to_string();

            let config_path = entry_path.join("config.json");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
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

    agents.sort_by_key(|a| a.name.to_lowercase());
    teams.sort_by_key(|a| a.name.to_lowercase());
    workgroups.sort_by_key(|a| a.name.to_lowercase());
    loops.sort_by_key(|a| a.name.to_lowercase());

    // Associate each workgroup with its team by matching replica membership.
    // Two-pass approach: exact match across ALL teams first, then suffix fallback.
    // This prevents a suffix hit on team T1 from shadowing an exact hit on T2.
    for wg in &mut workgroups {
        // Pass 1: exact match (origin_project/name == team agent ref)
        let exact = teams.iter().find(|t| {
            wg.agents.iter().any(|agent| {
                let full_ref = format!(
                    "{}/{}",
                    agent.origin_project.as_deref().unwrap_or("unknown"),
                    agent.name
                );
                t.agents.contains(&full_ref)
            })
        });
        if let Some(t) = exact {
            wg.team_name = Some(t.name.clone());
            log::info!("[discovery] Workgroup '{}' → team '{}'", wg.name, t.name);
        } else {
            // Pass 2: suffix fallback — covers missing/stale identity, canonicalize
            // failure, or absolute-path team refs from different projects
            let suffix = teams.iter().find(|t| {
                wg.agents.iter().any(|agent| {
                    t.agents.iter().any(|team_ref| {
                        team_ref.rsplit('/').next().is_some_and(|s| s == agent.name)
                    })
                })
            });
            wg.team_name = suffix.map(|t| t.name.clone());
            if let Some(ref name) = wg.team_name {
                log::warn!(
                    "[discovery] Workgroup '{}' → team '{}' (matched via name suffix, identity may \
                        be missing)",
                    wg.name, name
                );
            } else {
                log::info!("[discovery] Workgroup '{}' → no team matched", wg.name);
            }
        }
    }

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
