use super::creation_trace::{CreationTrace, Origin, Outcome, Reason, Stage};
use super::matrix_copy::MatrixCopyHooks;
use super::matrix_layout::{
    parse_role_frontmatter, repo_dir_name_from_url, sanitize_name, selected_ac_root,
    validate_existing_name,
};
use super::repo_clone::git_clone_async;
use super::room_number::{determine_next_wg_number, room_dir_name, room_number_pad_width};
use super::room_task::prepare_new_room_task_content;
use super::team_config::{
    create_new_team_config_on_disk_guarded, read_team_config, resolve_context_alert_argument,
    ContextAlertWriteMode, TeamConfigMutationGuard,
};
use super::team_update::emit_coordinator_refresh;
use super::workgroup::initialize_workgroup_replicas_on_disk_traced;
use super::{
    AgentInfo, CloneError, CreatedEntityResult, RepoAssignment, TeamAgentSource, TeamConfigResult,
    WorkgroupCloneResult,
};
use crate::config::ac_root::existing_ac_root;
use crate::config::entity_creation::copies::copy_agent_matrix_strict_with_hooks;
#[cfg(test)]
pub(crate) use crate::config::entity_creation::copies::portable_source_ref;
pub(crate) use crate::config::entity_creation::copies::{
    preflight_team_sources, project_directory_identity, require_absent, verify_owned_entry,
    verify_team_copies,
};
use crate::config::entity_creation::teams::normalized_team_config_bytes;
use crate::filesystem::projects::entities::copies::{
    cleanup_owned_copy_artifacts, write_owned_team_file, MatrixCopyKind, MatrixCopyStage,
    OwnedCopyArtifacts, OwnedCopyEntry,
};
use crate::session::manager::SessionManager;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

/// List all agent matrices across multiple project paths.
/// Scans {project}/.ac/_agent_*/ and reads Role.md frontmatter.
#[tauri::command]
pub async fn list_all_agents(
    project_paths: Vec<String>,
    destination_project_path: Option<String>,
) -> Result<Vec<AgentInfo>, String> {
    list_all_agents_for_destination(&project_paths, destination_project_path.as_deref())
}

pub(super) fn list_all_agents_for_destination(
    project_paths: &[String],
    destination_project_path: Option<&str>,
) -> Result<Vec<AgentInfo>, String> {
    let destination = destination_project_path
        .map(|path| project_directory_identity(Path::new(path)).map(|(_, identity)| identity))
        .transpose()?;
    let mut agents: Vec<AgentInfo> = Vec::new();

    for project_path in project_paths {
        let Ok((project, identity)) = project_directory_identity(Path::new(project_path)) else {
            continue;
        };
        let base = project.as_path();
        let Some(ac_root) = existing_ac_root(base) else {
            continue;
        };

        let project_name = base
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

            if !dir_name.starts_with("_agent_") {
                continue;
            }

            let agent_name_from_dir = dir_name
                .strip_prefix("_agent_")
                .unwrap_or(&dir_name)
                .to_string();

            // Try to read Role.md frontmatter for richer metadata
            let role_path = path.join("Role.md");
            let (fm_name, fm_description) = if role_path.exists() {
                match std::fs::read_to_string(&role_path) {
                    Ok(content) => parse_role_frontmatter(&content),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };

            agents.push(AgentInfo {
                name: fm_name.unwrap_or(agent_name_from_dir),
                description: fm_description.unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                project_name: project_name.clone(),
                project_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
                    base,
                ),
                is_local: destination.map(|destination| destination == identity),
            });
        }
    }

    agents.sort_by_key(|a| a.name.to_lowercase());
    Ok(agents)
}

pub(super) fn publish_owned_team(
    owned: &mut OwnedCopyArtifacts,
    bytes: &[u8],
    copies: &[OwnedCopyArtifacts],
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<(), String> {
    owned.create_directory(Path::new(""))?;
    owned.create_directory(Path::new("memory"))?;
    hooks.run(MatrixCopyStage::TeamConventions, &owned.root)?;
    write_owned_team_file(owned, Path::new("conventions.md"), &[])?;
    let temporary = PathBuf::from(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    write_owned_team_file(owned, &temporary, bytes)?;
    hooks.run(MatrixCopyStage::TeamPublication, &owned.root)?;
    verify_team_copies(copies)?;
    for entry in &owned.entries {
        verify_owned_entry(owned, entry)?;
    }
    let temporary_entry = owned.entries.last().expect("temporary entry");
    let identity = temporary_entry.identity;
    let digest = temporary_entry.digest;
    std::fs::hard_link(owned.root.join(&temporary), owned.root.join("config.json"))
        .map_err(|e| format!("Failed to publish config.json: {e}"))?;
    owned.entries.push(OwnedCopyEntry {
        relative: PathBuf::from("config.json"),
        identity,
        kind: MatrixCopyKind::File,
        digest,
    });
    hooks.run(MatrixCopyStage::TeamUnlink, &owned.root)?;
    // Verify both names and their directory chain before removing our temporary.
    for entry in owned.entries.iter().rev().take(2) {
        verify_owned_entry(owned, entry)?;
    }
    std::fs::remove_file(owned.root.join(&temporary)).map_err(|e| e.to_string())?;
    owned.entries.retain(|entry| entry.relative != temporary);
    Ok(())
}

pub(super) fn create_team_from_sources(
    project: &Path,
    name: &str,
    mut config: TeamConfigResult,
    sources: Option<Vec<TeamAgentSource>>,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<PathBuf, String> {
    validate_existing_name(name, "Team")?;
    let ac = selected_ac_root(project)?;
    let guard = TeamConfigMutationGuard::acquire(&ac)?;
    let Some(sources) = sources else {
        return create_new_team_config_on_disk_guarded(&ac, name, &config, &guard);
    };
    let ac = ac.canonicalize().map_err(|e| e.to_string())?;
    let mut team = OwnedCopyArtifacts {
        root: ac.join(format!("_team_{name}")),
        entries: Vec::new(),
    };
    require_absent(&team.root)?;
    let foreign = preflight_team_sources(project, &ac, &mut config, sources)?;
    let mut copies = Vec::new();
    let result = (|| -> Result<(), String> {
        for (source_project, agent) in foreign {
            copies.push(copy_agent_matrix_strict_with_hooks(
                &source_project,
                &agent,
                &ac,
                &agent,
                hooks,
            )?);
        }
        let bytes = normalized_team_config_bytes(&ac, &config)?;
        publish_owned_team(&mut team, &bytes, &copies, hooks)
    })();
    if let Err(primary) = result {
        let mut errors = vec![primary];
        if let Err(error) = hooks.run(MatrixCopyStage::BeforeCleanup, &team.root) {
            errors.push(error);
        }
        for owned in std::iter::once(&team).chain(copies.iter().rev()) {
            if let Err(error) = cleanup_owned_copy_artifacts(owned) {
                errors.push(error);
            }
        }
        return Err(errors.join("; "));
    }
    Ok(team.root)
}

/// Create a team directory inside {project_path}/.ac/_team_{name}/
// Tauri command parameters plus injected state exceed Clippy's argument threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    name: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
    context_alert_percentages: Option<Vec<u8>>,
    agent_sources: Option<Vec<TeamAgentSource>>,
) -> Result<CreatedEntityResult, String> {
    let safe_name = sanitize_name(&name)?;
    let context_alert_percentages = resolve_context_alert_argument(
        ContextAlertWriteMode::Create,
        context_alert_percentages,
        None,
    )?;
    let config = TeamConfigResult {
        agents,
        coordinator,
        repos,
        context_alert_percentages,
    };
    let team_dir = tokio::task::spawn_blocking(move || {
        create_team_from_sources(
            Path::new(&project_path),
            &safe_name,
            config,
            agent_sources,
            &mut MatrixCopyHooks::default(),
        )
    })
    .await
    .map_err(|e| format!("Team creation worker failed: {e}"))??;

    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&team_dir);
    log::info!("[entity_creation] Created team: {}", result_path);
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(CreatedEntityResult { path: result_path })
}

/// Create a workgroup from an existing team.
/// Clones repos async — partial failures are reported but don't rollback the WG.
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[tauri::command]
pub async fn create_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
    task_title: String,
) -> Result<WorkgroupCloneResult, String> {
    let trace = CreationTrace::new(Origin::Gui);
    let mut operation = trace.operation();
    let mut physical = operation.child(Stage::Physical);
    let result = async {
        let prepare = physical.child(Stage::Prepare);
        let (safe_team, task_content, base, team_config) = prepare.run(|| {
            let safe_team = sanitize_name(&team_name)?;
            let task_content = prepare_new_room_task_content(&task_title)?;
            let base = selected_ac_root(Path::new(&project_path))?;
            physical.project(&base);
            operation.project(&base);

            // Ensure gitignore protects workgroup clones from parent repo operations
            if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&base) {
                log::warn!(
                    "[create_workgroup] Failed to ensure Project AC Root .gitignore: {}",
                    e
                );
            }

            let team_config = read_team_config(&base, &safe_team)?;

            Ok::<_, String>((safe_team, task_content, base, team_config))
        })?;

        let allocate = physical.child(Stage::Allocate);
        let (wg_dir, wg_number) = allocate.run(|| {
            // Determine next WG number
            let wg_number = determine_next_wg_number(&base);

            let wg_name = room_dir_name(wg_number, &safe_team, room_number_pad_width());
            let wg_dir = base.join(&wg_name);
            if wg_dir.exists() {
                return Err(format!("Room directory already exists: {}", wg_name));
            }
            std::fs::create_dir_all(&wg_dir)
                .map_err(|e| format!("Failed to create room directory: {}", e))?;
            std::fs::create_dir_all(wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME))
                .map_err(|e| format!("Failed to create messaging directory: {}", e))?;

            // Write the prepared Clean or user-owned task before replicas and clones.
            std::fs::write(wg_dir.join("TASK.md"), &task_content)
                .map_err(|e| format!("Failed to write TASK.md: {}", e))?;

            Ok::<_, String>((wg_dir, wg_number))
        })?;
        physical.room(wg_number);
        operation.room(wg_number);

        let team_agents = team_config.agents;
        let team_repos = team_config.repos;

        // Collect unique repo URLs and their directory names
        let mut unique_repos: Vec<(usize, String, String)> = Vec::new(); // (index, url, dir_name)
        let mut duplicate_indices = Vec::new();
        let mut seen_urls: HashSet<String> = HashSet::new();
        for (index, repo) in team_repos.iter().enumerate() {
            if seen_urls.insert(repo.url.clone()) {
                let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
                unique_repos.push((index + 1, repo.url.clone(), dir_name));
            } else {
                duplicate_indices.push(index + 1);
            }
        }

        // Create __agent_*/ replica dirs through the same guarded, lossless
        // initialization the CLI/on-disk path uses (#1939): a replica's tooling,
        // custom context and unknown keys survive re-creation.
        initialize_workgroup_replicas_on_disk_traced(
            &base,
            &wg_dir,
            &team_agents,
            &team_repos,
            Some(&physical),
        )?;

        // Clone repos (async, partial failures logged but don't rollback)
        let mut clone_errors: Vec<CloneError> = Vec::new();
        let mut repos_stage = physical.child(Stage::Repos);
        repos_stage.count(team_repos.len());
        for index in duplicate_indices {
            repos_stage.skip_repo(index, Reason::DuplicateUrl);
        }
        for (index, url, dir_name) in &unique_repos {
            let target = wg_dir.join(dir_name);
            let mut repo_stage = repos_stage.indexed_child(Stage::Repo, *index);
            let result = git_clone_async(url, &target, Some(&repo_stage)).await;
            if result.is_ok() {
                repo_stage.finish(Outcome::Ok, None);
            } else {
                repo_stage.finish(Outcome::Error, Some(Reason::Failed));
            }
            match result {
                Ok(_) => {
                    log::info!("[entity_creation] Cloned {} → {}", url, target.display());
                }
                Err(e) => {
                    log::error!("[entity_creation] Failed to clone {}: {}", url, e);
                    clone_errors.push(CloneError {
                        url: url.clone(),
                        error: e,
                    });
                }
            }
        }

        repos_stage.clone_errors(clone_errors.len());
        repos_stage.finish(Outcome::clones(clone_errors.len()), None);
        let result_path =
            crate::path_utils::path_to_string_without_windows_verbatim_prefix(&wg_dir);
        log::info!(
            "[entity_creation] Created workgroup: {} ({} clone errors)",
            result_path,
            clone_errors.len()
        );
        physical.clone_errors(clone_errors.len());
        physical.finish(Outcome::clones(clone_errors.len()), None);
        let mut refresh = operation.child(Stage::RefreshBackend);
        emit_coordinator_refresh(&app, session_mgr.inner()).await;
        refresh.finish(Outcome::Ok, None);
        Ok(WorkgroupCloneResult {
            path: result_path,
            clone_errors,
        })
    }
    .await;
    if result.is_err() {
        physical.finish(Outcome::Error, Some(Reason::Failed));
    }
    if let Ok(created) = &result {
        operation.clone_errors(created.clone_errors.len());
        operation.finish(Outcome::clones(created.clone_errors.len()), None);
    }
    operation.result(result)
}
