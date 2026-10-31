use super::super::model::{AcAgentMatrix, AcAgentReplica, AcTeam, AcWorkgroup};
use super::super::project_git::detect_repo_branch;
use super::super::replica_read::{
    agent_display_name, origin_project_for_replica_identity, projected_path_string,
    read_preferred_agent_id, read_replica_config_with_valid_identity, read_task_fields,
    replica_selection_wire_fields, resolve_agent_ref,
};
use crate::config::coding_agent_profiles::{
    read_replica_current_coding_agent, read_replica_profile_result,
};
use crate::config::coordinator_clocks::ClockEntry;
use crate::config::entity_creation::teams::read_discovery_team_config;
use crate::config::settings::AppSettings;
use crate::filesystem::ac_discovery::{
    canonical_discovery_repo, discovery_directory_entries, discovery_path_is_dir,
    discovery_role_exists,
};
use std::collections::HashMap;
use std::path::Path;

pub(crate) struct DiscoveryTree {
    pub(crate) agents: Vec<AcAgentMatrix>,
    pub(crate) teams: Vec<AcTeam>,
    pub(crate) workgroups: Vec<AcWorkgroup>,
}

pub(crate) async fn discover_project_tree(
    ac_root: &Path,
    project_folder: &str,
    cfg: &AppSettings,
    clocks_snapshot: &HashMap<String, ClockEntry>,
    call_id: u64,
) -> Result<DiscoveryTree, String> {
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();

    let entries = match discovery_directory_entries(ac_root) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to read Project AC Root directory: {}", e)),
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !discovery_path_is_dir(&entry_path) {
            continue;
        }

        let dir_name = match entry_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Agent matrices: _agent_*
        if dir_name.starts_with("_agent_") {
            let display_name = agent_display_name(project_folder, &dir_name);
            let role_exists = discovery_role_exists(&entry_path.join("Role.md"));

            let preferred_agent_id = read_preferred_agent_id(&entry_path, cfg);

            agents.push(AcAgentMatrix {
                name: display_name,
                path: projected_path_string(&entry_path),
                role_exists,
                preferred_agent_id,
            });
        }

        // Workgroups: wg-*
        if crate::config::entity_prefix::has_entity_prefix(&dir_name) {
            workgroups.push(
                discover_workgroup(
                    &entry_path,
                    &dir_name,
                    ac_root,
                    project_folder,
                    cfg,
                    clocks_snapshot,
                    call_id,
                )
                .await,
            );
        }

        // Teams: _team_*
        if dir_name.starts_with("_team_") {
            let team_name = dir_name
                .strip_prefix("_team_")
                .unwrap_or(&dir_name)
                .to_string();

            if let Some(parsed) = read_discovery_team_config(&entry_path) {
                let team_agents = parsed
                    .get("agents")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(|r| resolve_agent_ref(project_folder, r))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let coordinator = parsed
                    .get("coordinator")
                    .and_then(|c| c.as_str())
                    .map(|r| resolve_agent_ref(project_folder, r));

                teams.push(AcTeam {
                    name: team_name,
                    agents: team_agents,
                    coordinator,
                });
            }
        }
    }

    Ok(DiscoveryTree {
        agents,
        teams,
        workgroups,
    })
}

async fn discover_workgroup(
    entry_path: &Path,
    dir_name: &str,
    ac_root: &Path,
    project_folder: &str,
    cfg: &AppSettings,
    clocks_snapshot: &HashMap<String, ClockEntry>,
    call_id: u64,
) -> AcWorkgroup {
    let (task, task_title) = read_task_fields(entry_path);

    let repo_path = discovery_directory_entries(entry_path)
        .ok()
        .and_then(|entries| {
            entries.flatten().find(|e| {
                let n = e.file_name();
                let name = n.to_string_lossy();
                name.starts_with("repo-") && discovery_path_is_dir(&e.path())
            })
        })
        .map(|e| projected_path_string(&e.path()));

    let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
    if let Ok(wg_entries) = discovery_directory_entries(entry_path) {
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
                    origin_project_for_replica_identity(project_folder, &identity_read);

                let preferred_agent_id = identity_read
                    .identity
                    .as_ref()
                    .and_then(|identity| read_preferred_agent_id(&identity.matrix_dir, cfg));
                let current_coding_agent_id = read_replica_current_coding_agent(&wg_path)
                    .filter(|id| cfg.agents.iter().any(|agent| agent.id == *id));
                let profile_read = read_replica_profile_result(&wg_path);
                if let Some(warning) = &profile_read.warning {
                    log::warn!("[ac-discovery] {}", warning);
                }
                let current_profile = profile_read.profile;

                let (repo_paths, repo_branch) =
                    discover_replica_repositories(&wg_path, identity_read.config.as_ref()).await;

                // §AR2-strict: `is_coordinator` short-circuits on
                // unqualified names, so build the project-qualified FQN
                // (mirrors `agent_fqn_from_path`'s `<proj>:<wg>/<agent>`
                // shape). Covered by
                // teams::tests::is_any_coordinator_requires_qualified_fqn.
                let is_coordinator =
                    crate::config::teams::resolve_wg_coordinator_replica(ac_root, entry_path)
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

    AcWorkgroup {
        name: dir_name.to_string(),
        path: projected_path_string(entry_path),
        task,
        task_title,
        agents: wg_agents,
        repo_path,
        team_name: None,
    }
}

pub(crate) fn associate_workgroups_with_teams(workgroups: &mut [AcWorkgroup], teams: &[AcTeam]) {
    // Associate each workgroup with its team by matching replica membership.
    // Two-pass approach: exact match across ALL teams first, then suffix fallback.
    // This prevents a suffix hit on team T1 from shadowing an exact hit on T2.
    for wg in workgroups {
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
}

async fn discover_replica_repositories(
    wg_path: &Path,
    config: Option<&serde_json::Value>,
) -> (Vec<String>, Option<String>) {
    let repo_paths: Vec<String> = config
        .and_then(|v| v.get("repos")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.as_str())
        .filter_map(|rel| {
            let resolved = wg_path.join(rel);
            canonical_discovery_repo(&resolved)
                .ok()
                .map(|p| projected_path_string(&p))
        })
        .collect();

    let repo_branch = if repo_paths.len() == 1 {
        detect_repo_branch(&repo_paths[0]).await
    } else {
        None
    };
    (repo_paths, repo_branch)
}
