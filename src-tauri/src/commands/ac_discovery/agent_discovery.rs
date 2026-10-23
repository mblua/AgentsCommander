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
        if !base.is_dir() {
            continue;
        }

        // Also check children of the base path (same pattern as search_repos)
        let dirs_to_check: Vec<std::path::PathBuf> = {
            let mut dirs = vec![base.to_path_buf()];
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('.') {
                            dirs.push(p);
                        }
                    }
                }
            }
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
                &crate::config::agent_command::managed_instructions_filenames(&cfg),
            );
            let context_scan = crate::config::seed_manifest::run_blocking_owned({
                let repo_dir = repo_dir.clone();
                let ac_root = ac_root.clone();
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
                    scan_project_context_templates_recorded(
                        &repo_dir,
                        &ac_root,
                        activation.as_ref(),
                    )
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
            loops.extend(crate::config::loops::discover_loops_in_project(&repo_dir));

            let project_folder = repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let entries = match std::fs::read_dir(&ac_root) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Agent matrices: _agent_* (single underscore prefix)
                if dir_name.starts_with("_agent_") {
                    let display_name = agent_display_name(&project_folder, &dir_name);
                    let role_exists = path.join("Role.md").exists();

                    let preferred_agent_id = read_preferred_agent_id(&path, &cfg);

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
                    let (task, task_title) = read_task_fields(&path);

                    // Find first repo-* directory for CWD
                    let repo_path = std::fs::read_dir(&path)
                        .ok()
                        .and_then(|entries| {
                            entries.flatten().find(|e| {
                                let n = e.file_name();
                                let name = n.to_string_lossy();
                                name.starts_with("repo-") && e.path().is_dir()
                            })
                        })
                        .map(|e| projected_path_string(&e.path()));

                    // Scan __agent_* replicas inside the WG
                    let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
                    if let Ok(wg_entries) = std::fs::read_dir(&path) {
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

                                let identity_read =
                                    read_replica_config_with_valid_identity(&wg_path);

                                let identity_path = identity_read
                                    .identity
                                    .as_ref()
                                    .map(|identity| identity.identity.clone());

                                let origin_project = origin_project_for_replica_identity(
                                    &project_folder,
                                    &identity_read,
                                );

                                // Resolve identity to matrix dir and read its lastCodingAgent
                                let preferred_agent_id =
                                    identity_read.identity.as_ref().and_then(|identity| {
                                        read_preferred_agent_id(&identity.matrix_dir, &cfg)
                                    });
                                let current_coding_agent_id =
                                    read_replica_current_coding_agent(&wg_path).filter(|id| {
                                        cfg.agents.iter().any(|agent| agent.id == *id)
                                    });
                                let profile_read = read_replica_profile_result(&wg_path);
                                if let Some(warning) = &profile_read.warning {
                                    log::warn!("[ac-discovery] {}", warning);
                                }
                                let current_profile = profile_read.profile;

                                // Extract repos from config.json and resolve to absolute paths
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

                                // Detect git branch for single-repo replicas
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
                                let is_coordinator =
                                    crate::config::teams::resolve_wg_coordinator_replica(
                                        &ac_root, &path,
                                    )
                                    .map(|resolved| resolved.agent_name == replica_name)
                                    .unwrap_or(false);

                                log::debug!(
                                    "[ac-discovery] call={} replica — project='{}' wg='{}' \
                                        replica='{}' fqn='{}:{}/{}' is_coordinator={}",
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
                                let fqn =
                                    format!("{}:{}/{}", project_folder, dir_name, replica_name);
                                let clock_entry = if is_coordinator {
                                    clocks_snapshot.get(&fqn)
                                } else {
                                    None
                                };
                                // (#580) the badge field carries the unified idle
                                // anchor max(user, activity) (rename deferred,
                                // Decision C) so a dormant/just-loaded row shows the
                                // real accumulated idle (closed time included), not
                                // just the user-message age.
                                let last_user_message_at = clock_entry
                                    .and_then(|e| e.idle_anchor())
                                    .map(|dt| dt.to_rfc3339());
                                let auto_closed_at = clock_entry
                                    .and_then(|e| e.auto_closed_at)
                                    .map(|dt| dt.to_rfc3339());
                                let manually_closed_at = clock_entry
                                    .and_then(|e| e.manually_closed_at)
                                    .map(|dt| dt.to_rfc3339());

                                // #1941 - strict protection state from the same
                                // validated replica directory; an invalid state is
                                // surfaced as `invalid`, never as default false.
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
                        path: projected_path_string(&path),
                        task,
                        task_title,
                        agents: wg_agents,
                        repo_path,
                        team_name: None,
                    });
                    wg_project_map.insert(dir_name.clone(), repo_dir_str.clone());
                }

                // Teams: _team_*
                if dir_name.starts_with("_team_") {
                    let team_name = dir_name
                        .strip_prefix("_team_")
                        .unwrap_or(&dir_name)
                        .to_string();

                    let config_path = path.join("config.json");
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
        }
    }

    // Sort alphabetically
    agents.sort_by_key(|a| a.name.to_lowercase());
    teams.sort_by_key(|a| a.name.to_lowercase());
    workgroups.sort_by_key(|a| a.name.to_lowercase());
    loops.sort_by_key(|a| a.name.to_lowercase());
    drop(cfg);

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
